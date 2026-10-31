//! Schema migrations, applied in order and tracked with `PRAGMA user_version`.

use rusqlite::{Connection, TransactionBehavior};

const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r#"
    CREATE TABLE projects (
        id               INTEGER PRIMARY KEY,
        name             TEXT NOT NULL,
        color            TEXT NOT NULL,
        default_template TEXT,
        local_only       INTEGER NOT NULL DEFAULT 0,
        created_at       INTEGER NOT NULL
    );
    CREATE TABLE documents (
        id          INTEGER PRIMARY KEY,
        project_id  INTEGER REFERENCES projects(id) ON DELETE SET NULL,
        title       TEXT NOT NULL,
        template_id TEXT,
        fields_json TEXT NOT NULL DEFAULT '{}',
        source      TEXT NOT NULL CHECK (source IN ('dictation', 'file')),
        audio_path  TEXT,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
    );
    CREATE INDEX documents_project ON documents(project_id);
    CREATE TABLE tags (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    );
    CREATE TABLE document_tags (
        document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        tag_id      INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
        PRIMARY KEY (document_id, tag_id)
    );
    CREATE TABLE paragraphs (
        id            INTEGER PRIMARY KEY,
        document_id   INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        ord           INTEGER NOT NULL,
        text          TEXT NOT NULL,
        start_ms      INTEGER,
        end_ms        INTEGER,
        low_conf_json TEXT NOT NULL DEFAULT '[]',
        speaker       TEXT
    );
    CREATE INDEX paragraphs_document ON paragraphs(document_id, ord);
    -- remove_diacritics 0 keeps å distinct from a.
    CREATE VIRTUAL TABLE paragraphs_fts USING fts5(
        text, content='paragraphs', content_rowid='id',
        tokenize = "unicode61 remove_diacritics 0"
    );
    CREATE TRIGGER paragraphs_ai AFTER INSERT ON paragraphs BEGIN
        INSERT INTO paragraphs_fts(rowid, text) VALUES (new.id, new.text);
    END;
    CREATE TRIGGER paragraphs_ad AFTER DELETE ON paragraphs BEGIN
        INSERT INTO paragraphs_fts(paragraphs_fts, rowid, text) VALUES ('delete', old.id, old.text);
    END;
    CREATE TRIGGER paragraphs_au AFTER UPDATE OF text ON paragraphs BEGIN
        INSERT INTO paragraphs_fts(paragraphs_fts, rowid, text) VALUES ('delete', old.id, old.text);
        INSERT INTO paragraphs_fts(rowid, text) VALUES (new.id, new.text);
    END;
    CREATE TABLE summaries (
        id                INTEGER PRIMARY KEY,
        document_id       INTEGER REFERENCES documents(id) ON DELETE CASCADE,
        project_id        INTEGER REFERENCES projects(id) ON DELETE CASCADE,
        prompt_id         TEXT NOT NULL,
        provider          TEXT NOT NULL,
        model             TEXT NOT NULL,
        text              TEXT NOT NULL,
        include_in_export INTEGER NOT NULL DEFAULT 1,
        created_at        INTEGER NOT NULL
    );
    CREATE TABLE action_items (
        id           INTEGER PRIMARY KEY,
        document_id  INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        paragraph_id INTEGER REFERENCES paragraphs(id) ON DELETE SET NULL,
        what         TEXT NOT NULL,
        who          TEXT,
        due          TEXT,
        done         INTEGER NOT NULL DEFAULT 0,
        created_at   INTEGER NOT NULL
    );
    "#,
    // 2: confirmations to send text to a cloud AI provider
    r#"
    CREATE TABLE cloud_consents (
        scope       TEXT NOT NULL,
        provider_id TEXT NOT NULL,
        created_at  INTEGER NOT NULL,
        PRIMARY KEY (scope, provider_id)
    );
    "#,
    // 3: which AI provider found an action item
    r#"
    ALTER TABLE action_items ADD COLUMN provider TEXT;
    "#,
    // 4: words the user corrected in dictated text, to offer as vocabulary
    r#"
    CREATE TABLE corrections (
        heard     TEXT NOT NULL,
        wanted    TEXT NOT NULL,
        count     INTEGER NOT NULL DEFAULT 0,
        dismissed INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (heard, wanted)
    );
    "#,
    // 5: phones paired with Fennec Recorder and the recordings they send
    r#"
    CREATE TABLE devices (
        id           INTEGER PRIMARY KEY,
        name         TEXT NOT NULL,
        secret_hash  TEXT NOT NULL UNIQUE,
        paired_at    INTEGER NOT NULL,
        last_seen_at INTEGER
    );
    CREATE TABLE inbound_recordings (
        uuid        TEXT PRIMARY KEY,
        device_id   INTEGER REFERENCES devices(id) ON DELETE SET NULL,
        title       TEXT NOT NULL,
        recorded_at INTEGER NOT NULL,
        duration_ms INTEGER NOT NULL,
        project_id  INTEGER REFERENCES projects(id) ON DELETE SET NULL,
        template_id TEXT,
        ext         TEXT NOT NULL,
        size        INTEGER NOT NULL,
        sha256      TEXT NOT NULL,
        received    INTEGER NOT NULL DEFAULT 0,
        state       TEXT NOT NULL CHECK (state IN ('receiving', 'queued', 'transcribing', 'done', 'failed')),
        document_id INTEGER REFERENCES documents(id) ON DELETE SET NULL,
        error       TEXT,
        updated_at  INTEGER NOT NULL
    );
    CREATE INDEX inbound_document ON inbound_recordings(document_id);
    "#,
    // 6: no more paired phones; each transfer starts with a QR code
    r#"
    CREATE TABLE inbound_new (
        uuid        TEXT PRIMARY KEY,
        title       TEXT NOT NULL,
        recorded_at INTEGER NOT NULL,
        duration_ms INTEGER NOT NULL,
        project_id  INTEGER REFERENCES projects(id) ON DELETE SET NULL,
        template_id TEXT,
        ext         TEXT NOT NULL,
        size        INTEGER NOT NULL,
        sha256      TEXT NOT NULL,
        received    INTEGER NOT NULL DEFAULT 0,
        state       TEXT NOT NULL CHECK (state IN ('receiving', 'queued', 'transcribing', 'done', 'failed')),
        document_id INTEGER REFERENCES documents(id) ON DELETE SET NULL,
        error       TEXT,
        updated_at  INTEGER NOT NULL
    );
    INSERT INTO inbound_new
        SELECT uuid, title, recorded_at, duration_ms, project_id, template_id, ext, size,
               sha256, received, state, document_id, error, updated_at
        FROM inbound_recordings;
    DROP TABLE inbound_recordings;
    ALTER TABLE inbound_new RENAME TO inbound_recordings;
    CREATE INDEX inbound_document ON inbound_recordings(document_id);
    DROP TABLE devices;
    "#,
    // 7: the dictation a meeting note was rewritten from
    r#"
    ALTER TABLE documents ADD COLUMN rewritten_from INTEGER REFERENCES documents(id) ON DELETE SET NULL;
    CREATE INDEX documents_rewritten_from ON documents(rewritten_from);
    "#,
    // 8: one meeting note per dictation, rewritten in place; earlier ones
    // become documents of their own
    r#"
    UPDATE documents SET rewritten_from = NULL
    WHERE rewritten_from IS NOT NULL
      AND id <> (SELECT n.id FROM documents n WHERE n.rewritten_from = documents.rewritten_from
                 ORDER BY n.created_at DESC, n.id DESC LIMIT 1);
    "#,
];

/// Brings the schema up to date. Threads may open a new database at the
/// same moment (the window, audio clean-up, the phone receiver): the write
/// lock is taken before the version is read, so only one of them migrates.
pub(super) fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: usize = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i + 1)?;
    }
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_phones_are_forgotten_and_their_uploads_kept() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute_batch(
            "INSERT INTO devices (id, name, secret_hash, paired_at) VALUES (7, 'Pixel 8', 'h', 1);
             INSERT INTO inbound_recordings
                 (uuid, device_id, title, recorded_at, duration_ms, ext, size, sha256, received, state, updated_at)
             VALUES ('abcdef12', 7, 'Møde', 1, 2, 'aac', 10, 's', 4, 'receiving', 3);",
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        let row: (String, i64, String) = conn
            .query_row(
                "SELECT title, received, state FROM inbound_recordings WHERE uuid = 'abcdef12'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("Møde".into(), 4, "receiving".into()));
        let devices: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'devices'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(devices, 0);
        conn.execute_batch("PRAGMA foreign_key_check;").unwrap();
    }
}
