//! Documents, projects, tags and everything attached to them, in SQLite.

pub mod retention;
mod schema;

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, ToSql, params, params_from_iter};

pub type ProjectId = i64;
pub type DocumentId = i64;
pub type ParagraphId = i64;
pub type ActionItemId = i64;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("corrupt JSON in the database: {0}")]
    Json(#[from] serde_json::Error),
    #[error("document {0} does not exist")]
    DocumentNotFound(DocumentId),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Dictation,
    File,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Dictation => "dictation",
            Source::File => "file",
        }
    }

    fn parse(s: &str) -> Source {
        if s == "file" {
            Source::File
        } else {
            Source::Dictation
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectFilter {
    #[default]
    All,
    /// Documents without a project.
    Unsorted,
    Project(ProjectId),
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub color: String,
    pub default_template: Option<String>,
    pub local_only: bool,
    pub document_count: usize,
}

#[derive(Debug, Clone)]
pub struct NewDocument {
    pub title: String,
    pub project_id: Option<ProjectId>,
    pub template_id: Option<String>,
    pub source: Source,
}

impl NewDocument {
    pub fn dictation(title: &str) -> Self {
        Self {
            title: title.into(),
            project_id: None,
            template_id: None,
            source: Source::Dictation,
        }
    }

    pub fn file(title: &str) -> Self {
        Self {
            source: Source::File,
            ..Self::dictation(title)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Document {
    pub id: DocumentId,
    pub title: String,
    pub project_id: Option<ProjectId>,
    pub template_id: Option<String>,
    pub fields: BTreeMap<String, String>,
    pub source: Source,
    pub audio_path: Option<PathBuf>,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A row in document lists.
#[derive(Debug, Clone)]
pub struct DocumentSummary {
    pub id: DocumentId,
    pub title: String,
    pub project_id: Option<ProjectId>,
    pub template_id: Option<String>,
    pub source: Source,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// End of the last timestamped paragraph.
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct DocumentFilter {
    pub project: ProjectFilter,
    pub tag: Option<String>,
    /// Matches paragraph text (word prefixes) or the title.
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    pub id: Option<ParagraphId>,
    pub text: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    /// Byte ranges in `text` the engine was unsure of.
    pub low_confidence: Vec<Range<usize>>,
    pub speaker: Option<String>,
}

impl Paragraph {
    pub fn new(text: &str) -> Self {
        Self {
            id: None,
            text: text.into(),
            start_ms: None,
            end_ms: None,
            low_confidence: Vec::new(),
            speaker: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub document_id: DocumentId,
    pub paragraph_id: ParagraphId,
    pub text: String,
    pub start_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewSummary {
    pub document_id: Option<DocumentId>,
    pub project_id: Option<ProjectId>,
    pub prompt_id: String,
    pub provider: String,
    pub model: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub id: i64,
    pub prompt_id: String,
    pub provider: String,
    pub model: String,
    pub text: String,
    pub include_in_export: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewActionItem {
    pub what: String,
    pub who: Option<String>,
    pub due: Option<String>,
    pub paragraph_id: Option<ParagraphId>,
    /// Name of the AI provider that found it.
    pub provider: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ActionItem {
    pub id: ActionItemId,
    pub document_id: DocumentId,
    pub paragraph_id: Option<ParagraphId>,
    pub what: String,
    pub who: Option<String>,
    pub due: Option<String>,
    pub done: bool,
    pub provider: Option<String>,
    pub created_at: i64,
}

pub struct Store {
    conn: Connection,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Builds an FTS5 query that matches every word of `input` as a prefix.
/// User text never reaches FTS5 syntax: each word is quoted.
fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.to_lowercase()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn normalize_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('#').to_lowercase()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            // A missing data directory is created; other failures surface on open.
            let _ = std::fs::create_dir_all(dir);
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        schema::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    // ---- projects ----

    pub fn create_project(&self, name: &str, color: &str) -> Result<ProjectId> {
        self.conn.execute(
            "INSERT INTO projects (name, color, created_at) VALUES (?1, ?2, ?3)",
            params![name, color, now_ms()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, p.color, p.default_template, p.local_only,
                    (SELECT COUNT(*) FROM documents d WHERE d.project_id = p.id)
             FROM projects p ORDER BY p.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Project {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                default_template: r.get(3)?,
                local_only: r.get(4)?,
                document_count: r.get::<_, i64>(5)? as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn update_project(
        &self,
        id: ProjectId,
        name: &str,
        color: &str,
        default_template: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET name = ?2, color = ?3, default_template = ?4 WHERE id = ?1",
            params![id, name, color, default_template],
        )?;
        Ok(())
    }

    pub fn set_project_local_only(&self, id: ProjectId, local_only: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET local_only = ?2 WHERE id = ?1",
            params![id, local_only],
        )?;
        Ok(())
    }

    /// Whether a document may be sent to cloud AI providers.
    pub fn document_is_local_only(&self, id: DocumentId) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT COALESCE(p.local_only, 0) FROM documents d LEFT JOIN projects p ON p.id = d.project_id
                 WHERE d.id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(StoreError::DocumentNotFound(id))
    }

    pub fn delete_project(&self, id: ProjectId) -> Result<()> {
        self.conn.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---- documents ----

    pub fn create_document(&self, doc: &NewDocument) -> Result<DocumentId> {
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO documents (project_id, title, template_id, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                doc.project_id,
                doc.title,
                doc.template_id,
                doc.source.as_str(),
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn document(&self, id: DocumentId) -> Result<Document> {
        let doc = self
            .conn
            .query_row(
                "SELECT id, title, project_id, template_id, fields_json, source, audio_path, created_at, updated_at
                 FROM documents WHERE id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, i64>(8)?,
                    ))
                },
            )
            .optional()?
            .ok_or(StoreError::DocumentNotFound(id))?;
        Ok(Document {
            id: doc.0,
            title: doc.1,
            project_id: doc.2,
            template_id: doc.3,
            fields: serde_json::from_str(&doc.4)?,
            source: Source::parse(&doc.5),
            audio_path: doc.6.map(PathBuf::from),
            tags: self.document_tags(id)?,
            created_at: doc.7,
            updated_at: doc.8,
        })
    }

    pub fn update_document(
        &self,
        id: DocumentId,
        title: &str,
        template_id: Option<&str>,
        fields: &BTreeMap<String, String>,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE documents SET title = ?2, template_id = ?3, fields_json = ?4, updated_at = ?5 WHERE id = ?1",
            params![id, title, template_id, serde_json::to_string(fields)?, now_ms()],
        )?;
        if changed == 0 {
            return Err(StoreError::DocumentNotFound(id));
        }
        Ok(())
    }

    pub fn move_document(&self, id: DocumentId, project: Option<ProjectId>) -> Result<()> {
        self.conn.execute(
            "UPDATE documents SET project_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, project, now_ms()],
        )?;
        Ok(())
    }

    /// Counts one correction of `heard` to `wanted`; returns how often it
    /// has been made, or 0 once the offer to learn it was dismissed.
    pub fn record_correction(&self, heard: &str, wanted: &str) -> Result<u32> {
        self.conn.execute(
            "INSERT INTO corrections (heard, wanted, count) VALUES (?1, ?2, 1)
             ON CONFLICT (heard, wanted) DO UPDATE SET count = count + 1",
            params![heard, wanted],
        )?;
        Ok(self.conn.query_row(
            "SELECT CASE WHEN dismissed THEN 0 ELSE count END FROM corrections WHERE heard = ?1 AND wanted = ?2",
            params![heard, wanted],
            |r| r.get(0),
        )?)
    }

    /// Never offer to learn this correction again.
    pub fn dismiss_correction(&self, heard: &str, wanted: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE corrections SET dismissed = 1 WHERE heard = ?1 AND wanted = ?2",
            params![heard, wanted],
        )?;
        Ok(())
    }

    pub fn set_audio_path(&self, id: DocumentId, path: Option<&Path>) -> Result<()> {
        self.conn.execute(
            "UPDATE documents SET audio_path = ?2 WHERE id = ?1",
            params![id, path.map(|p| p.to_string_lossy().into_owned())],
        )?;
        Ok(())
    }

    /// Every document that has an audio file, with its path.
    pub fn documents_with_audio(&self) -> Result<Vec<(DocumentId, PathBuf)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, audio_path FROM documents WHERE audio_path IS NOT NULL ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delete_document(&self, id: DocumentId) -> Result<()> {
        self.conn.execute("DELETE FROM documents WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Newest first.
    pub fn documents(&self, filter: &DocumentFilter) -> Result<Vec<DocumentSummary>> {
        let mut sql = String::from(
            "SELECT d.id, d.title, d.project_id, d.template_id, d.source, d.created_at, d.updated_at,
                    (SELECT MAX(p.end_ms) FROM paragraphs p WHERE p.document_id = d.id)
             FROM documents d WHERE 1 = 1",
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        match filter.project {
            ProjectFilter::All => {}
            ProjectFilter::Unsorted => sql.push_str(" AND d.project_id IS NULL"),
            ProjectFilter::Project(p) => {
                sql.push_str(" AND d.project_id = ?");
                args.push(Box::new(p));
            }
        }
        if let Some(tag) = &filter.tag {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM document_tags dt JOIN tags t ON t.id = dt.tag_id
                              WHERE dt.document_id = d.id AND t.name = ?)",
            );
            args.push(Box::new(normalize_tag(tag)));
        }
        if let Some(text) = &filter.text {
            let Some(q) = fts_query(text) else {
                return Ok(Vec::new());
            };
            sql.push_str(
                " AND (d.id IN (SELECT p.document_id FROM paragraphs_fts f JOIN paragraphs p ON p.id = f.rowid
                                WHERE paragraphs_fts MATCH ?)
                       OR d.title LIKE ?)",
            );
            args.push(Box::new(q));
            args.push(Box::new(format!("%{}%", text.trim())));
        }
        sql.push_str(" ORDER BY d.created_at DESC, d.id DESC");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                Ok(DocumentSummary {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    project_id: r.get(2)?,
                    template_id: r.get(3)?,
                    source: Source::parse(&r.get::<_, String>(4)?),
                    tags: Vec::new(),
                    created_at: r.get(5)?,
                    updated_at: r.get(6)?,
                    duration_ms: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|mut d| {
                d.tags = self.document_tags(d.id)?;
                Ok(d)
            })
            .collect()
    }

    // ---- tags ----

    /// Replaces a document's tags. Tags are lower-cased and de-duplicated.
    pub fn set_tags(&self, id: DocumentId, tags: &[String]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM document_tags WHERE document_id = ?1", [id])?;
        for tag in tags.iter().map(|t| normalize_tag(t)).filter(|t| !t.is_empty()) {
            tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [&tag])?;
            tx.execute(
                "INSERT OR IGNORE INTO document_tags (document_id, tag_id)
                 SELECT ?1, id FROM tags WHERE name = ?2",
                params![id, tag],
            )?;
        }
        tx.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM document_tags)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// All tags in use with their document counts, alphabetically.
    pub fn tags(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name, COUNT(dt.document_id) FROM tags t JOIN document_tags dt ON dt.tag_id = t.id
             GROUP BY t.id ORDER BY t.name",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn document_tags(&self, id: DocumentId) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name FROM tags t JOIN document_tags dt ON dt.tag_id = t.id
             WHERE dt.document_id = ?1 ORDER BY t.name",
        )?;
        let rows = stmt.query_map([id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- paragraphs ----

    pub fn paragraphs(&self, id: DocumentId) -> Result<Vec<Paragraph>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text, start_ms, end_ms, low_conf_json, speaker FROM paragraphs
             WHERE document_id = ?1 ORDER BY ord",
        )?;
        let rows = stmt
            .query_map([id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get::<_, String>(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, text, start_ms, end_ms, low, speaker)| {
                let spans: Vec<(usize, usize)> = serde_json::from_str(&low)?;
                Ok(Paragraph {
                    id: Some(id),
                    text,
                    start_ms,
                    end_ms,
                    low_confidence: spans.into_iter().map(|(s, e)| s..e).collect(),
                    speaker,
                })
            })
            .collect()
    }

    /// Replaces all paragraphs of a document in one transaction (editor save).
    pub fn replace_paragraphs(&self, id: DocumentId, paragraphs: &[Paragraph]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM paragraphs WHERE document_id = ?1", [id])?;
        for (ord, p) in paragraphs.iter().enumerate() {
            insert_paragraph(&tx, id, ord as i64, p)?;
        }
        tx.execute(
            "UPDATE documents SET updated_at = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Saves editor content, keeping paragraph ids stable by position so
    /// links to paragraphs (action items, citations) survive autosave.
    pub fn sync_paragraphs(&self, id: DocumentId, paragraphs: &[Paragraph]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let existing: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT id FROM paragraphs WHERE document_id = ?1 ORDER BY ord")?;
            stmt.query_map([id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (ord, p) in paragraphs.iter().enumerate() {
            let spans: Vec<(usize, usize)> = p.low_confidence.iter().map(|r| (r.start, r.end)).collect();
            let spans = serde_json::to_string(&spans)?;
            match existing.get(ord) {
                Some(pid) => {
                    tx.execute(
                        "UPDATE paragraphs SET ord = ?2, text = ?3, start_ms = ?4, end_ms = ?5, low_conf_json = ?6,
                         speaker = ?7 WHERE id = ?1",
                        params![pid, ord as i64, p.text, p.start_ms, p.end_ms, spans, p.speaker],
                    )?;
                }
                None => {
                    insert_paragraph(&tx, id, ord as i64, p)?;
                }
            }
        }
        for pid in existing.iter().skip(paragraphs.len()) {
            tx.execute("DELETE FROM paragraphs WHERE id = ?1", [pid])?;
        }
        tx.execute(
            "UPDATE documents SET updated_at = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Adds a paragraph at the end (committed dictation or ingest output).
    pub fn append_paragraph(&self, id: DocumentId, p: &Paragraph) -> Result<ParagraphId> {
        let tx = self.conn.unchecked_transaction()?;
        let ord: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ord) + 1, 0) FROM paragraphs WHERE document_id = ?1",
            [id],
            |r| r.get(0),
        )?;
        let pid = insert_paragraph(&tx, id, ord, p)?;
        tx.execute(
            "UPDATE documents SET updated_at = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        tx.commit()?;
        Ok(pid)
    }

    /// Paragraphs whose text contains every word of `query` (as prefixes).
    pub fn search(&self, query: &str, project: ProjectFilter) -> Result<Vec<SearchHit>> {
        let Some(q) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut sql = String::from(
            "SELECT p.document_id, p.id, p.text, p.start_ms FROM paragraphs_fts f
             JOIN paragraphs p ON p.id = f.rowid JOIN documents d ON d.id = p.document_id
             WHERE paragraphs_fts MATCH ?",
        );
        let mut args: Vec<Box<dyn ToSql>> = vec![Box::new(q)];
        match project {
            ProjectFilter::All => {}
            ProjectFilter::Unsorted => sql.push_str(" AND d.project_id IS NULL"),
            ProjectFilter::Project(id) => {
                sql.push_str(" AND d.project_id = ?");
                args.push(Box::new(id));
            }
        }
        sql.push_str(" ORDER BY bm25(paragraphs_fts) LIMIT 200");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            Ok(SearchHit {
                document_id: r.get(0)?,
                paragraph_id: r.get(1)?,
                text: r.get(2)?,
                start_ms: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- summaries ----

    pub fn add_summary(&self, s: &NewSummary) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO summaries (document_id, project_id, prompt_id, provider, model, text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                s.document_id,
                s.project_id,
                s.prompt_id,
                s.provider,
                s.model,
                s.text,
                now_ms()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Newest first.
    pub fn summaries_for_document(&self, id: DocumentId) -> Result<Vec<Summary>> {
        self.summaries_where("document_id = ?1", id)
    }

    pub fn summaries_for_project(&self, id: ProjectId) -> Result<Vec<Summary>> {
        self.summaries_where("project_id = ?1", id)
    }

    fn summaries_where(&self, cond: &str, id: i64) -> Result<Vec<Summary>> {
        let sql = format!(
            "SELECT id, prompt_id, provider, model, text, include_in_export, created_at FROM summaries
             WHERE {cond} ORDER BY created_at DESC, id DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([id], |r| {
            Ok(Summary {
                id: r.get(0)?,
                prompt_id: r.get(1)?,
                provider: r.get(2)?,
                model: r.get(3)?,
                text: r.get(4)?,
                include_in_export: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn update_summary(&self, id: i64, text: &str, include_in_export: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE summaries SET text = ?2, include_in_export = ?3 WHERE id = ?1",
            params![id, text, include_in_export],
        )?;
        Ok(())
    }

    // ---- action items ----

    /// Replaces the action items extracted from a document.
    pub fn replace_action_items(&self, id: DocumentId, items: &[NewActionItem]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM action_items WHERE document_id = ?1", [id])?;
        let now = now_ms();
        for item in items {
            tx.execute(
                "INSERT INTO action_items (document_id, paragraph_id, what, who, due, created_at, provider)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    item.paragraph_id,
                    item.what,
                    item.who,
                    item.due,
                    now,
                    item.provider
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Open items first, then in document and extraction order.
    pub fn action_items(&self, filter: ProjectFilter) -> Result<Vec<ActionItem>> {
        let mut sql = String::from(
            "SELECT a.id, a.document_id, a.paragraph_id, a.what, a.who, a.due, a.done, a.provider,
                    a.created_at
             FROM action_items a JOIN documents d ON d.id = a.document_id WHERE 1 = 1",
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        match filter {
            ProjectFilter::All => {}
            ProjectFilter::Unsorted => sql.push_str(" AND d.project_id IS NULL"),
            ProjectFilter::Project(p) => {
                sql.push_str(" AND d.project_id = ?");
                args.push(Box::new(p));
            }
        }
        sql.push_str(" ORDER BY a.done, d.created_at, a.id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            Ok(ActionItem {
                id: r.get(0)?,
                document_id: r.get(1)?,
                paragraph_id: r.get(2)?,
                what: r.get(3)?,
                who: r.get(4)?,
                due: r.get(5)?,
                done: r.get(6)?,
                provider: r.get(7)?,
                created_at: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn document_action_items(&self, id: DocumentId) -> Result<Vec<ActionItem>> {
        Ok(self
            .action_items(ProjectFilter::All)?
            .into_iter()
            .filter(|a| a.document_id == id)
            .collect())
    }

    pub fn set_action_done(&self, id: ActionItemId, done: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE action_items SET done = ?2 WHERE id = ?1",
            params![id, done],
        )?;
        Ok(())
    }

    // ---- cloud consents ----

    /// `scope` names what may be sent, e.g. `document:12` or `project:3`.
    pub fn has_cloud_consent(&self, scope: &str, provider_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM cloud_consents WHERE scope = ?1 AND provider_id = ?2)",
            params![scope, provider_id],
            |r| r.get(0),
        )?)
    }

    /// Forgets every confirmation; the next cloud send asks again.
    pub fn clear_cloud_consents(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM cloud_consents", [])?)
    }

    pub fn add_cloud_consent(&self, scope: &str, provider_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO cloud_consents (scope, provider_id, created_at) VALUES (?1, ?2, ?3)",
            params![scope, provider_id, now_ms()],
        )?;
        Ok(())
    }
}

fn insert_paragraph(conn: &Connection, doc: DocumentId, ord: i64, p: &Paragraph) -> Result<ParagraphId> {
    let spans: Vec<(usize, usize)> = p.low_confidence.iter().map(|r| (r.start, r.end)).collect();
    conn.execute(
        "INSERT INTO paragraphs (document_id, ord, text, start_ms, end_ms, low_conf_json, speaker)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            doc,
            ord,
            p.text,
            p.start_ms,
            p.end_ms,
            serde_json::to_string(&spans)?,
            p.speaker
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::fts_query;

    #[test]
    fn fts_query_quotes_each_word_as_a_prefix() {
        assert_eq!(fts_query("Kælder fugt").as_deref(), Some("\"kælder\"* \"fugt\"*"));
    }

    #[test]
    fn fts_query_drops_operators_and_punctuation() {
        assert_eq!(fts_query("\"OR\" AND (").as_deref(), Some("\"or\"* \"and\"*"));
        assert_eq!(fts_query(" - "), None);
    }
}
