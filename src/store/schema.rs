//! Schema migrations, applied in order and tracked with `PRAGMA user_version`.

use rusqlite::Connection;

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
];

pub(super) fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: usize = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i + 1)?;
        tx.commit()?;
    }
    Ok(())
}
