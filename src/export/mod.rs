//! Turning documents into TXT, DOCX and PDF through a template.

mod docx;
mod pdf;
mod txt;

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::store::{DocumentId, Store, StoreError};
use crate::template::Template;
use crate::text::{clock, danish_date};

pub use pdf::preview as preview_first_page;
pub use txt::render_txt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Txt,
    Docx,
    Pdf,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Txt => "txt",
            Format::Docx => "docx",
            Format::Pdf => "pdf",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    pub include_fields: bool,
    pub timestamps: bool,
    pub highlight_low_confidence: bool,
    /// Only used for project exports.
    pub table_of_contents: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_fields: true,
            timestamps: false,
            highlight_low_confidence: false,
            table_of_contents: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("required fields are empty: {}", .0.join(", "))]
    MissingFields(Vec<String>),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("could not write {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("logo {path} must be a PNG image")]
    LogoNotPng { path: PathBuf },
    #[error("PDF rendering failed: {0}")]
    Pdf(String),
}

/// Everything a writer needs, already resolved from the store and template.
#[derive(Debug, Clone)]
pub struct Report {
    pub heading: String,
    /// Project name for project exports; shown above the table of contents.
    pub collection_title: Option<String>,
    pub logo: Option<PathBuf>,
    pub footer: String,
    pub body_font: String,
    pub body_size_pt: f64,
    pub summary_heading: String,
    pub table_of_contents: bool,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub title: String,
    pub date: String,
    /// (label, value) in template order; empty when fields are excluded.
    pub fields: Vec<(String, String)>,
    pub summary: Option<String>,
    pub paragraphs: Vec<ReportParagraph>,
}

#[derive(Debug, Clone)]
pub struct ReportParagraph {
    pub text: String,
    pub timestamp: Option<String>,
    /// Byte ranges to highlight; empty unless highlighting is on.
    pub highlight: Vec<Range<usize>>,
}

impl Report {
    /// A single document. Fails if the template's required fields are empty.
    pub fn for_document(
        store: &Store,
        id: DocumentId,
        template: &Template,
        opts: ExportOptions,
    ) -> Result<Report, ExportError> {
        let doc = store.document(id)?;
        let missing = template.missing_required(&doc.fields);
        if !missing.is_empty() {
            return Err(ExportError::MissingFields(missing));
        }
        let section = build_section(store, id, template, opts)?;
        Ok(Report::with_sections(template, None, false, vec![section]))
    }

    /// Like [`Report::for_document`] but without the required-field check,
    /// for previews while the user is still filling in fields.
    pub fn draft_for_document(
        store: &Store,
        id: DocumentId,
        template: &Template,
        opts: ExportOptions,
    ) -> Result<Report, ExportError> {
        let section = build_section(store, id, template, opts)?;
        Ok(Report::with_sections(template, None, false, vec![section]))
    }

    /// Several documents as one report, oldest first.
    pub fn for_documents(
        store: &Store,
        ids: &[DocumentId],
        collection_title: &str,
        template: &Template,
        opts: ExportOptions,
    ) -> Result<Report, ExportError> {
        let mut docs = ids
            .iter()
            .map(|&id| store.document(id))
            .collect::<Result<Vec<_>, _>>()?;
        docs.sort_by_key(|d| (d.created_at, d.id));
        let sections = docs
            .iter()
            .map(|d| build_section(store, d.id, template, opts))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Report::with_sections(
            template,
            Some(collection_title.to_string()),
            opts.table_of_contents,
            sections,
        ))
    }

    /// A report with the template's layout and the given sections.
    pub fn from_template(t: &Template, sections: Vec<Section>) -> Report {
        Report::with_sections(t, None, false, sections)
    }

    fn with_sections(t: &Template, collection: Option<String>, toc: bool, sections: Vec<Section>) -> Report {
        Report {
            heading: t.heading.clone(),
            collection_title: collection,
            logo: t.logo_path(),
            footer: t.footer.clone(),
            body_font: t.body_font.clone(),
            body_size_pt: t.body_size_pt,
            summary_heading: t.summary_heading.clone(),
            table_of_contents: toc,
            sections,
        }
    }
}

fn build_section(
    store: &Store,
    id: DocumentId,
    template: &Template,
    opts: ExportOptions,
) -> Result<Section, ExportError> {
    let doc = store.document(id)?;
    let fields = if opts.include_fields {
        template
            .fields
            .iter()
            .map(|f| {
                (
                    f.label.clone(),
                    doc.fields.get(&f.key).cloned().unwrap_or_default(),
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let summary = store
        .summaries_for_document(id)?
        .into_iter()
        .find(|s| s.include_in_export)
        .map(|s| s.text);
    let paragraphs = store
        .paragraphs(id)?
        .into_iter()
        .filter(|p| !p.text.trim().is_empty())
        .map(|p| ReportParagraph {
            timestamp: opts.timestamps.then(|| p.start_ms.map(clock)).flatten(),
            highlight: if opts.highlight_low_confidence {
                p.low_confidence.clone()
            } else {
                Vec::new()
            },
            text: p.text,
        })
        .collect();
    Ok(Section {
        title: doc.title,
        date: danish_date(doc.created_at),
        fields,
        summary,
        paragraphs,
    })
}

/// A file name from the title (and case number, if any), safe on any file
/// system: Danish letters are spelled out (æ→ae, ø→oe, å→aa).
pub fn file_stem(title: &str, case_number: Option<&str>) -> String {
    let mut base = String::new();
    if let Some(c) = case_number.map(str::trim).filter(|c| !c.is_empty()) {
        base.push_str(c);
        base.push('_');
    }
    base.push_str(title);
    let mut out = String::new();
    for c in base.to_lowercase().chars() {
        match c {
            'æ' => out.push_str("ae"),
            'ø' => out.push_str("oe"),
            'å' => out.push_str("aa"),
            c if c.is_ascii_alphanumeric() || c == '-' => out.push(c),
            _ => {
                if !out.ends_with('_') && !out.is_empty() {
                    out.push('_');
                }
            }
        }
    }
    let out = out.trim_matches('_').chars().take(80).collect::<String>();
    if out.is_empty() { "fennec".into() } else { out }
}

/// Writes `report` to `path` in `format`. The file is written to a temporary
/// name first so a failed export never leaves a half-written file behind.
pub fn write(report: &Report, format: Format, path: &Path) -> Result<(), ExportError> {
    if let Some(logo) = &report.logo
        && format != Format::Txt
        && !logo.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return Err(ExportError::LogoNotPng { path: logo.clone() });
    }
    let tmp = path.with_extension(format!("{}.part", format.extension()));
    let io = |source| ExportError::Io {
        path: path.to_path_buf(),
        source,
    };
    let result = match format {
        Format::Txt => std::fs::write(&tmp, render_txt(report)).map_err(io),
        Format::Docx => docx::write(report, &tmp).map_err(io),
        Format::Pdf => pdf::write(report, &tmp),
    };
    match result {
        Ok(()) => std::fs::rename(&tmp, path).map_err(io),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::file_stem;

    #[test]
    fn file_stems_spell_out_danish_letters_and_drop_punctuation() {
        assert_eq!(
            file_stem("Besigtigelse Nørregade 14", Some("2026-0412")),
            "2026-0412_besigtigelse_noerregade_14"
        );
        assert_eq!(file_stem("Møde: Å/Æ?", None), "moede_aa_ae");
        assert_eq!(file_stem("!!!", None), "fennec");
    }
}
