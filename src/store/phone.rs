//! Recordings sent by Fennec Recorder on a phone, over the network or USB.

use rusqlite::{OptionalExtension, Row, params};

use super::{DocumentId, ProjectId, Result, Store, now_ms};

/// Where a recording from the phone is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundState {
    /// Arriving in chunks.
    Receiving,
    /// Complete; its document waits in the Files queue.
    Queued,
    Transcribing,
    Done,
    Failed,
}

impl InboundState {
    pub fn as_str(self) -> &'static str {
        match self {
            InboundState::Receiving => "receiving",
            InboundState::Queued => "queued",
            InboundState::Transcribing => "transcribing",
            InboundState::Done => "done",
            InboundState::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "queued" => InboundState::Queued,
            "transcribing" => InboundState::Transcribing,
            "done" => InboundState::Done,
            "failed" => InboundState::Failed,
            _ => InboundState::Receiving,
        }
    }
}

/// What the phone says about a recording before sending it.
#[derive(Debug, Clone, PartialEq)]
pub struct NewInbound {
    pub uuid: String,
    pub title: String,
    /// When recording started, in ms since the epoch.
    pub recorded_at: i64,
    pub duration_ms: i64,
    pub project_id: Option<ProjectId>,
    pub template_id: Option<String>,
    /// File extension, without the dot.
    pub ext: String,
    pub size: u64,
    /// Lower-case hex SHA-256 of the whole file.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inbound {
    pub info: NewInbound,
    /// Bytes stored so far.
    pub received: u64,
    pub state: InboundState,
    pub document_id: Option<DocumentId>,
    pub error: Option<String>,
    pub updated_at: i64,
}

const INBOUND_COLUMNS: &str = "uuid, title, recorded_at, duration_ms, project_id, template_id,
     ext, size, sha256, received, state, document_id, error, updated_at";

fn inbound_row(r: &Row<'_>) -> rusqlite::Result<Inbound> {
    Ok(Inbound {
        info: NewInbound {
            uuid: r.get(0)?,
            title: r.get(1)?,
            recorded_at: r.get(2)?,
            duration_ms: r.get(3)?,
            project_id: r.get(4)?,
            template_id: r.get(5)?,
            ext: r.get(6)?,
            size: r.get::<_, i64>(7)? as u64,
            sha256: r.get(8)?,
        },
        received: r.get::<_, i64>(9)? as u64,
        state: InboundState::parse(&r.get::<_, String>(10)?),
        document_id: r.get(11)?,
        error: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

impl Store {
    // ---- recordings from phones ----

    pub fn add_inbound(&self, r: &NewInbound) -> Result<()> {
        self.conn.execute(
            "INSERT INTO inbound_recordings
                (uuid, title, recorded_at, duration_ms, project_id, template_id,
                 ext, size, sha256, state, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'receiving', ?10)",
            params![
                r.uuid,
                r.title,
                r.recorded_at,
                r.duration_ms,
                r.project_id,
                r.template_id,
                r.ext,
                r.size as i64,
                r.sha256,
                now_ms()
            ],
        )?;
        Ok(())
    }

    pub fn inbound(&self, uuid: &str) -> Result<Option<Inbound>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {INBOUND_COLUMNS} FROM inbound_recordings WHERE uuid = ?1"),
                [uuid],
                inbound_row,
            )
            .optional()?)
    }

    pub fn set_inbound_received(&self, uuid: &str, received: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE inbound_recordings SET received = ?2, updated_at = ?3 WHERE uuid = ?1",
            params![uuid, received as i64, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_inbound_state(
        &self,
        uuid: &str,
        state: InboundState,
        document_id: Option<DocumentId>,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE inbound_recordings
             SET state = ?2, document_id = COALESCE(?3, document_id), error = ?4, updated_at = ?5
             WHERE uuid = ?1",
            params![uuid, state.as_str(), document_id, error, now_ms()],
        )?;
        Ok(())
    }

    /// Follows the Files queue: the recording behind `doc`, if it came from
    /// a phone, takes the queue item's state. Other documents are untouched.
    pub fn set_inbound_state_for_document(
        &self,
        doc: DocumentId,
        state: InboundState,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE inbound_recordings SET state = ?2, error = ?3, updated_at = ?4
             WHERE document_id = ?1",
            params![doc, state.as_str(), error, now_ms()],
        )?;
        Ok(())
    }

    /// Recordings that arrived but were not transcribed to the end (Fennec
    /// closed meanwhile), oldest first.
    pub fn inbound_to_resume(&self) -> Result<Vec<Inbound>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {INBOUND_COLUMNS} FROM inbound_recordings
             WHERE state IN ('queued', 'transcribing') AND document_id IS NOT NULL
             ORDER BY updated_at, uuid"
        ))?;
        let rows = stmt.query_map([], inbound_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Uploads still arriving that were last touched before `before_ms`.
    pub fn stale_inbound(&self, before_ms: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT uuid FROM inbound_recordings WHERE state = 'receiving' AND updated_at < ?1")?;
        let rows = stmt.query_map([before_ms], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delete_inbound(&self, uuid: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM inbound_recordings WHERE uuid = ?1", [uuid])?;
        Ok(())
    }

    pub fn project_exists(&self, id: ProjectId) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM projects WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Dates a document by when its audio was recorded rather than when it
    /// reached Fennec.
    pub fn set_document_created_at(&self, id: DocumentId, created_at: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE documents SET created_at = ?2 WHERE id = ?1",
            params![id, created_at],
        )?;
        Ok(())
    }
}
