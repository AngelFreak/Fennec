//! What the AI does with transcripts: summaries, clean-up, action items,
//! questions over a project and template field suggestions.
//!
//! Paragraphs are shown to the model with ids (`[p4]`, or `[d12:p4]` across
//! documents) so answers can point back into the text. Every id the model
//! returns is checked against the ids it was given.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Value, json};

use super::{AiError, Cancel, LlmProvider, StructuredRequest, TextRequest};
use crate::store::{DocumentId, NewActionItem, Paragraph, ParagraphId};
use crate::template::{Field, FieldKind};

/// A document as the actions see it.
#[derive(Debug, Clone)]
pub struct DocText {
    pub id: DocumentId,
    pub title: String,
    /// Milliseconds since the epoch; relative dates resolve against it.
    pub created_at: i64,
    pub paragraphs: Vec<Paragraph>,
}

impl DocText {
    fn date(&self) -> chrono::NaiveDate {
        chrono::DateTime::from_timestamp_millis(self.created_at)
            .map(|d| d.with_timezone(&chrono::Local).date_naive())
            .unwrap_or_default()
    }

    fn ids(&self) -> HashSet<ParagraphId> {
        self.paragraphs.iter().filter_map(|p| p.id).collect()
    }
}

const MAX_TOKENS: u32 = 16_000;

fn system(language: &str, task: &str) -> String {
    format!(
        "Du arbejder med transskriptioner af dansk tale, lavet af automatisk talegenkendelse. \
         Teksten kan indeholde fejlhørte ord. Svar på {language}. {task}"
    )
}

/// `[p4] tekst` lines; with `with_doc`, `[d12:p4] tekst`.
fn tagged_lines(doc: &DocText, with_doc: bool) -> Vec<String> {
    doc.paragraphs
        .iter()
        .filter(|p| !p.text.trim().is_empty())
        .map(|p| {
            let id = p.id.unwrap_or_default();
            if with_doc {
                format!("[d{}:p{id}] {}", doc.id, p.text.trim())
            } else {
                format!("[p{id}] {}", p.text.trim())
            }
        })
        .collect()
}

fn header(doc: &DocText) -> String {
    format!("## {} ({})", doc.title, crate::text::danish_date(doc.created_at))
}

/// Groups lines into chunks of at most `budget` characters (a single
/// longer line becomes its own chunk).
fn chunk(lines: &[String], budget: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in lines {
        if !current.is_empty() && current.len() + line.len() + 1 > budget {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

// ---- summary ----

/// Summarises one or more documents (oldest first), streaming the final
/// text. Too-long input is summarised in parts, then combined.
pub fn summarize(
    llm: &dyn LlmProvider,
    docs: &[DocText],
    language: &str,
    cancel: &Cancel,
    on_delta: &mut dyn FnMut(&str),
) -> Result<String, AiError> {
    let mut docs: Vec<&DocText> = docs.iter().collect();
    docs.sort_by_key(|d| d.created_at);
    let mut lines = Vec::new();
    for d in &docs {
        lines.push(header(d));
        lines.extend(
            d.paragraphs
                .iter()
                .filter(|p| !p.text.trim().is_empty())
                .map(|p| p.text.trim().to_string()),
        );
    }
    let task = if docs.len() > 1 {
        "Skriv et kort, sagligt resumé af forløbet på tværs af dokumenterne, i tidsorden. \
         Brug korte afsnit eller punkter. Opfind intet."
    } else {
        "Skriv et kort, sagligt resumé af dokumentet. Brug korte afsnit eller punkter. Opfind intet."
    };
    let sys = system(language, task);
    let chunks = chunk(&lines, llm.config().context_chars);
    let input = if chunks.len() <= 1 {
        chunks.into_iter().next().unwrap_or_default()
    } else {
        let mut parts = Vec::new();
        for (i, c) in chunks.iter().enumerate() {
            let req = TextRequest {
                system: system(
                    language,
                    "Resumér denne del af en længere tekst. Bevar navne, tal og beslutninger.",
                ),
                prompt: c.clone(),
                max_tokens: MAX_TOKENS,
            };
            let part = llm.stream_text(&req, cancel, &mut |_| {})?;
            parts.push(format!("### Del {}\n{}", i + 1, part.trim()));
        }
        format!(
            "Resuméer af tekstens dele, i rækkefølge:\n\n{}",
            parts.join("\n\n")
        )
    };
    let req = TextRequest {
        system: sys,
        prompt: input,
        max_tokens: MAX_TOKENS,
    };
    Ok(llm.stream_text(&req, cancel, on_delta)?.trim().to_string())
}

// ---- clean-up ----

#[derive(Debug, Clone, PartialEq)]
pub struct Cleaned {
    pub paragraph_id: ParagraphId,
    pub original: String,
    pub cleaned: String,
}

fn cleanup_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "paragraphs": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"id": {"type": "integer"}, "text": {"type": "string"}},
                    "required": ["id", "text"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["paragraphs"],
        "additionalProperties": false
    })
}

/// Proposes a cleaned version of each paragraph. Only paragraphs that
/// actually changed are returned; unknown ids are dropped.
pub fn cleanup(
    llm: &dyn LlmProvider,
    doc: &DocText,
    language: &str,
    cancel: &Cancel,
) -> Result<Vec<Cleaned>, AiError> {
    let originals: BTreeMap<ParagraphId, &str> = doc
        .paragraphs
        .iter()
        .filter_map(|p| Some((p.id?, p.text.as_str())))
        .collect();
    let task = "Ret hvert afsnit: tegnsætning, store bogstaver, fyldord (øh, øhm), gentagelser og \
                tydelige fejlhøringer. Bevar betydning, ordvalg og rækkefølge; omskriv ikke. \
                Returnér hvert afsnit med dets id (tallet efter p).";
    let mut out = Vec::new();
    for c in chunk(&tagged_lines(doc, false), llm.config().context_chars) {
        let req = StructuredRequest {
            system: system(language, task),
            prompt: c,
            schema_name: "cleanup".into(),
            schema: cleanup_schema(),
            max_tokens: MAX_TOKENS,
        };
        let v = llm.structured(&req, cancel)?;
        for item in v["paragraphs"].as_array().into_iter().flatten() {
            let (Some(id), Some(text)) = (item["id"].as_i64(), item["text"].as_str()) else {
                continue;
            };
            let Some(original) = originals.get(&id) else {
                tracing::warn!(id, "clean-up returned an unknown paragraph id");
                continue;
            };
            let text = text.trim();
            if !text.is_empty() && text != original.trim() {
                out.push(Cleaned {
                    paragraph_id: id,
                    original: original.to_string(),
                    cleaned: text.to_string(),
                });
            }
        }
    }
    Ok(out)
}

// ---- action items ----

fn action_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "what": {"type": "string"},
                        "who": {"type": ["string", "null"]},
                        "due": {"type": ["string", "null"]},
                        "paragraph": {"type": ["integer", "null"]}
                    },
                    "required": ["what", "who", "due", "paragraph"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["items"],
        "additionalProperties": false
    })
}

/// Finds tasks, owners and deadlines. Relative dates ("på fredag") are
/// resolved against the document date; anything not a valid date is dropped.
pub fn action_items(
    llm: &dyn LlmProvider,
    doc: &DocText,
    language: &str,
    cancel: &Cancel,
) -> Result<Vec<NewActionItem>, AiError> {
    let date = doc.date();
    let task = format!(
        "Find konkrete opgaver og aftaler i teksten. For hver: hvad (kort, i bydeform), \
         hvem (navn eller null), frist som ÅÅÅÅ-MM-DD eller null, og id på afsnittet hvor den nævnes \
         (tallet efter p). Dokumentet er fra {} ({}); regn relative datoer ud fra den dag.",
        crate::text::danish_date(doc.created_at),
        date.format("%Y-%m-%d (%A)")
    );
    let ids = doc.ids();
    let mut out = Vec::new();
    for c in chunk(&tagged_lines(doc, false), llm.config().context_chars) {
        let req = StructuredRequest {
            system: system(language, &task),
            prompt: c,
            schema_name: "action_items".into(),
            schema: action_schema(),
            max_tokens: MAX_TOKENS,
        };
        let v = llm.structured(&req, cancel)?;
        for item in v["items"].as_array().into_iter().flatten() {
            let Some(what) = item["what"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                continue;
            };
            let text = |k: &str| {
                item[k]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let due = text("due").filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok());
            out.push(NewActionItem {
                what: what.to_string(),
                who: text("who"),
                due,
                paragraph_id: item["paragraph"].as_i64().filter(|id| ids.contains(id)),
                provider: Some(llm.config().name.clone()),
            });
        }
    }
    Ok(out)
}

// ---- ask the project ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Citation {
    pub document_id: DocumentId,
    pub paragraph_id: ParagraphId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The answer with only valid `[d12:p4]` markers left in.
    pub text: String,
    /// Valid citations, in order of first use.
    pub citations: Vec<Citation>,
}

/// Paragraph lines to give the model: everything if it fits, otherwise the
/// best search hits for the question.
pub fn ask_context(docs: &[DocText], hits: &[Citation], budget: usize) -> String {
    let mut all = Vec::new();
    for d in docs {
        all.push(header(d));
        all.extend(tagged_lines(d, true));
    }
    let total: usize = all.iter().map(|l| l.len() + 1).sum();
    if total <= budget {
        return all.join("\n");
    }
    let mut out = String::new();
    for hit in hits {
        let Some(d) = docs.iter().find(|d| d.id == hit.document_id) else {
            continue;
        };
        let Some(p) = d.paragraphs.iter().find(|p| p.id == Some(hit.paragraph_id)) else {
            continue;
        };
        let line = format!(
            "[d{}:p{}] ({}) {}\n",
            d.id,
            hit.paragraph_id,
            d.title,
            p.text.trim()
        );
        if out.len() + line.len() > budget {
            break;
        }
        out.push_str(&line);
    }
    out
}

/// Answers `question` from `context`, streaming; citations are validated
/// against the paragraphs in `docs`.
pub fn ask(
    llm: &dyn LlmProvider,
    docs: &[DocText],
    context: &str,
    question: &str,
    language: &str,
    cancel: &Cancel,
    on_delta: &mut dyn FnMut(&str),
) -> Result<Answer, AiError> {
    let task = "Besvar spørgsmålet ud fra teksten alene. Henvis til kilden efter hver påstand med \
                afsnittets mærke, fx [d3:p12]. Står svaret ikke i teksten, så sig det.";
    let req = TextRequest {
        system: system(language, task),
        prompt: format!("Tekst:\n{context}\n\nSpørgsmål: {question}"),
        max_tokens: MAX_TOKENS,
    };
    let raw = llm.stream_text(&req, cancel, on_delta)?;
    let known: HashSet<Citation> = docs
        .iter()
        .flat_map(|d| {
            d.paragraphs.iter().filter_map(move |p| {
                Some(Citation {
                    document_id: d.id,
                    paragraph_id: p.id?,
                })
            })
        })
        .collect();
    Ok(validate_citations(&raw, &known))
}

/// Keeps `[d12:p4]` markers that name a known paragraph and removes the rest.
pub fn validate_citations(raw: &str, known: &HashSet<Citation>) -> Answer {
    let mut text = String::new();
    let mut citations = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("[d") {
        text.push_str(&rest[..start]);
        let after = &rest[start..];
        match parse_marker(after) {
            Some((c, len)) => {
                if known.contains(&c) {
                    text.push_str(&after[..len]);
                    if !citations.contains(&c) {
                        citations.push(c);
                    }
                } else {
                    tracing::debug!(?c, "dropping unknown citation");
                    // Avoid leaving a double space where the marker was.
                    if text.ends_with(' ') && after[len..].starts_with([' ', '.', ',']) {
                        text.pop();
                    }
                }
                rest = &after[len..];
            }
            None => {
                text.push_str("[d");
                rest = &after[2..];
            }
        }
    }
    text.push_str(rest);
    Answer {
        text: text.trim().to_string(),
        citations,
    }
}

/// Parses `[d<num>:p<num>]` at the start of `s`, returning it and its length.
fn parse_marker(s: &str) -> Option<(Citation, usize)> {
    let end = s.find(']')?;
    let inner = s.get(2..end)?;
    let (d, p) = inner.split_once(":p")?;
    Some((
        Citation {
            document_id: d.parse().ok()?,
            paragraph_id: p.parse().ok()?,
        },
        end + 1,
    ))
}

// ---- template fields ----

/// Suggests values for template fields that are still empty. Values the
/// user typed are never touched; list fields only accept their options.
pub fn suggest_fields(
    llm: &dyn LlmProvider,
    doc: &DocText,
    fields: &[Field],
    values: &BTreeMap<String, String>,
    language: &str,
    cancel: &Cancel,
) -> Result<BTreeMap<String, String>, AiError> {
    let empty: Vec<&Field> = fields
        .iter()
        .filter(|f| values.get(&f.key).is_none_or(|v| v.trim().is_empty()))
        .collect();
    if empty.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut properties = serde_json::Map::new();
    let mut describe = Vec::new();
    for f in &empty {
        properties.insert(f.key.clone(), json!({"type": ["string", "null"]}));
        let hint = match f.kind {
            FieldKind::Date => " (dato, ÅÅÅÅ-MM-DD)".to_string(),
            FieldKind::List => format!(" (en af: {})", f.options.join(", ")),
            FieldKind::Multiline => " (kan være flere linjer)".to_string(),
            FieldKind::Text => String::new(),
        };
        describe.push(format!("- {}: {}{hint}", f.key, f.label));
    }
    let schema = json!({
        "type": "object",
        "properties": properties,
        "required": empty.iter().map(|f| f.key.clone()).collect::<Vec<_>>(),
        "additionalProperties": false
    });
    let task = format!(
        "Udfyld felterne ud fra teksten. Brug null, hvis teksten ikke siger det. Felter:\n{}",
        describe.join("\n")
    );
    let text: Vec<String> = doc.paragraphs.iter().map(|p| p.text.trim().to_string()).collect();
    let input = chunk(&text, llm.config().context_chars)
        .into_iter()
        .next()
        .unwrap_or_default();
    let req = StructuredRequest {
        system: system(language, &task),
        prompt: input,
        schema_name: "fields".into(),
        schema,
        max_tokens: 4_000,
    };
    let v = llm.structured(&req, cancel)?;
    let mut out = BTreeMap::new();
    for f in empty {
        let Some(value) = v[&f.key].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let accepted = match f.kind {
            FieldKind::List => f.options.iter().any(|o| o == value).then(|| value.to_string()),
            // Date fields hold Danish dates, like `{today}` gives.
            FieldKind::Date => chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .map(crate::text::danish_day),
            FieldKind::Text | FieldKind::Multiline => Some(value.to_string()),
        };
        if let Some(v) = accepted {
            out.insert(f.key.clone(), v);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(pairs: &[(i64, i64)]) -> HashSet<Citation> {
        pairs
            .iter()
            .map(|&(d, p)| Citation {
                document_id: d,
                paragraph_id: p,
            })
            .collect()
    }

    #[test]
    fn valid_citations_are_kept_and_listed_once() {
        let a = validate_citations("Ja [d1:p2]. Også [d1:p2] og [d3:p4].", &known(&[(1, 2), (3, 4)]));
        assert_eq!(a.text, "Ja [d1:p2]. Også [d1:p2] og [d3:p4].");
        assert_eq!(a.citations.len(), 2);
    }

    #[test]
    fn unknown_citations_are_removed_from_the_text() {
        let a = validate_citations("Prisen er 10 kr [d9:p9]. Mødet [d1:p2].", &known(&[(1, 2)]));
        assert_eq!(a.text, "Prisen er 10 kr. Mødet [d1:p2].");
        assert_eq!(
            a.citations,
            vec![Citation {
                document_id: 1,
                paragraph_id: 2
            }]
        );
    }

    #[test]
    fn brackets_that_are_not_markers_stay() {
        let a = validate_citations("Se [dokumentet] og [d1:px].", &known(&[]));
        assert_eq!(a.text, "Se [dokumentet] og [d1:px].");
    }

    #[test]
    fn chunks_respect_the_budget() {
        let lines: Vec<String> = (0..10).map(|i| format!("linje {i}")).collect();
        let chunks = chunk(&lines, 20);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.len() <= 20));
        assert_eq!(chunks.concat().lines().count(), 10);
    }

    #[test]
    fn ask_context_falls_back_to_hits_when_too_long() {
        let doc = DocText {
            id: 1,
            title: "Møde".into(),
            created_at: 0,
            paragraphs: (1..=50)
                .map(|i| Paragraph {
                    id: Some(i),
                    ..Paragraph::new(&format!("afsnit nummer {i} med noget tekst"))
                })
                .collect(),
        };
        let hits = [Citation {
            document_id: 1,
            paragraph_id: 7,
        }];
        let full = ask_context(std::slice::from_ref(&doc), &hits, 100_000);
        assert!(full.contains("[d1:p50]"));
        let small = ask_context(std::slice::from_ref(&doc), &hits, 200);
        assert!(
            small.contains("[d1:p7]") && !small.contains("[d1:p50]"),
            "{small}"
        );
    }
}
