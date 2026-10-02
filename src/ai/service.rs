//! The one entry point the UI uses for AI. It applies the on/off switch and
//! the privacy gate, reads from and writes to the store, and runs actions.
//! Every method blocks; call them from a worker thread with its own `Store`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::actions::{self, Answer, Citation, Cleaned, DocText};
use super::{AiError, AiSettings, Cancel, LlmProvider, Locality, SecretStore, connect, privacy};
use crate::store::{ActionItem, DocumentFilter, DocumentId, NewSummary, ProjectFilter, Store};
use crate::template::Template;

/// What an action reads; decides the privacy rules and the consent record.
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    Document(DocumentId),
    Project(ProjectFilter),
    Tag(String),
}

impl Scope {
    /// Key for the consent table.
    pub fn key(&self) -> String {
        match self {
            Scope::Document(id) => format!("document:{id}"),
            Scope::Project(ProjectFilter::Project(id)) => format!("project:{id}"),
            Scope::Project(ProjectFilter::Unsorted) => "project:unsorted".into(),
            Scope::Project(ProjectFilter::All) => "project:all".into(),
            Scope::Tag(t) => format!("tag:{t}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summarized {
    /// Stored summary id (documents and real projects only).
    pub id: Option<i64>,
    pub text: String,
    pub provider: String,
    pub model: String,
}

#[derive(Clone)]
pub struct AiService {
    settings: AiSettings,
    secrets: Arc<dyn SecretStore>,
    /// True while dictation runs; jobs for a model on this computer wait.
    dictation_live: Arc<AtomicBool>,
}

impl AiService {
    pub fn new(settings: AiSettings, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            settings,
            secrets,
            dictation_live: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_dictation_flag(mut self, live: Arc<AtomicBool>) -> Self {
        self.dictation_live = live;
        self
    }

    pub fn settings(&self) -> &AiSettings {
        &self.settings
    }

    pub fn enabled(&self) -> bool {
        self.settings.enabled && !self.settings.providers.is_empty()
    }

    /// The default provider, cleared for `scope` by the privacy gate.
    pub fn provider_for(&self, store: &Store, scope: &Scope) -> Result<Box<dyn LlmProvider>, AiError> {
        if !self.settings.enabled {
            return Err(AiError::Disabled);
        }
        let cfg = self.settings.active().ok_or(AiError::NoProvider)?;
        let docs = scope_documents(store, scope)?;
        let mut local_only = false;
        for id in &docs {
            local_only |= store.document_is_local_only(*id)?;
        }
        let consented = store.has_cloud_consent(&scope.key(), &cfg.id)?;
        privacy::check(cfg, local_only, consented)?;
        Ok(connect(cfg, self.secrets.as_ref()))
    }

    /// Records that the user agreed to send `scope` to the default provider.
    pub fn consent(&self, store: &Store, scope: &Scope) -> Result<(), AiError> {
        let cfg = self.settings.active().ok_or(AiError::NoProvider)?;
        store.add_cloud_consent(&scope.key(), &cfg.id)?;
        Ok(())
    }

    /// Lets dictation keep the CPU: waits while it runs if the model is local.
    fn wait_turn(&self, llm: &dyn LlmProvider, cancel: &Cancel) -> Result<(), AiError> {
        if llm.config().locality != Locality::ThisComputer {
            return Ok(());
        }
        while self.dictation_live.load(Ordering::Relaxed) {
            if cancel.is_cancelled() {
                return Err(AiError::Cancelled);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Ok(())
    }

    fn prepare(
        &self,
        store: &Store,
        scope: &Scope,
        cancel: &Cancel,
    ) -> Result<(Box<dyn LlmProvider>, Vec<DocText>), AiError> {
        let llm = self.provider_for(store, scope)?;
        let docs = scope_documents(store, scope)?
            .into_iter()
            .map(|id| doc_text(store, id))
            .collect::<Result<Vec<_>, _>>()?;
        self.wait_turn(llm.as_ref(), cancel)?;
        Ok((llm, docs))
    }

    /// Streams a summary and stores it (nothing is stored on failure).
    pub fn summarize(
        &self,
        store: &Store,
        scope: &Scope,
        cancel: &Cancel,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<Summarized, AiError> {
        let (llm, docs) = self.prepare(store, scope, cancel)?;
        let text = actions::summarize(llm.as_ref(), &docs, &self.settings.language, cancel, on_delta)?;
        let cfg = llm.config();
        let (document_id, project_id) = match scope {
            Scope::Document(id) => (Some(*id), None),
            Scope::Project(ProjectFilter::Project(id)) => (None, Some(*id)),
            _ => (None, None),
        };
        let id = if document_id.is_some() || project_id.is_some() {
            Some(store.add_summary(&NewSummary {
                document_id,
                project_id,
                prompt_id: "summary".into(),
                provider: cfg.name.clone(),
                model: cfg.model.clone(),
                text: text.clone(),
            })?)
        } else {
            None
        };
        Ok(Summarized {
            id,
            text,
            provider: cfg.name.clone(),
            model: cfg.model.clone(),
        })
    }

    /// Proposed clean-ups; the caller decides what to apply.
    pub fn cleanup(&self, store: &Store, doc: DocumentId, cancel: &Cancel) -> Result<Vec<Cleaned>, AiError> {
        let (llm, docs) = self.prepare(store, &Scope::Document(doc), cancel)?;
        actions::cleanup(llm.as_ref(), &docs[0], &self.settings.language, cancel)
    }

    /// Finds action items and replaces the document's stored list.
    pub fn action_items(
        &self,
        store: &Store,
        doc: DocumentId,
        cancel: &Cancel,
    ) -> Result<Vec<ActionItem>, AiError> {
        let (llm, docs) = self.prepare(store, &Scope::Document(doc), cancel)?;
        let items = actions::action_items(llm.as_ref(), &docs[0], &self.settings.language, cancel)?;
        store.replace_action_items(doc, &items)?;
        Ok(store.document_action_items(doc)?)
    }

    pub fn ask(
        &self,
        store: &Store,
        scope: &Scope,
        question: &str,
        cancel: &Cancel,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<Answer, AiError> {
        let (llm, docs) = self.prepare(store, scope, cancel)?;
        let hits = ranked_hits(store, scope, &docs, question)?;
        let context = actions::ask_context(&docs, &hits, llm.config().context_chars);
        actions::ask(
            llm.as_ref(),
            &docs,
            &context,
            question,
            &self.settings.language,
            cancel,
            on_delta,
        )
    }

    /// Suggestions for the document's empty template fields.
    pub fn suggest_fields(
        &self,
        store: &Store,
        doc: DocumentId,
        template: &Template,
        cancel: &Cancel,
    ) -> Result<BTreeMap<String, String>, AiError> {
        let (llm, docs) = self.prepare(store, &Scope::Document(doc), cancel)?;
        let values = store.document(doc)?.fields;
        actions::suggest_fields(
            llm.as_ref(),
            &docs[0],
            &template.fields,
            &values,
            &self.settings.language,
            cancel,
        )
    }
}

fn scope_documents(store: &Store, scope: &Scope) -> Result<Vec<DocumentId>, AiError> {
    let filter = match scope {
        Scope::Document(id) => return Ok(vec![*id]),
        Scope::Project(f) => DocumentFilter {
            project: *f,
            ..Default::default()
        },
        Scope::Tag(t) => DocumentFilter {
            tag: Some(t.clone()),
            ..Default::default()
        },
    };
    Ok(store.documents(&filter)?.into_iter().map(|d| d.id).collect())
}

pub fn doc_text(store: &Store, id: DocumentId) -> Result<DocText, AiError> {
    let d = store.document(id)?;
    Ok(DocText {
        id,
        title: d.title,
        created_at: d.created_at,
        paragraphs: store.paragraphs(id)?,
    })
}

/// Paragraphs matching the question's words, most matching words first.
fn ranked_hits(
    store: &Store,
    scope: &Scope,
    docs: &[DocText],
    question: &str,
) -> Result<Vec<Citation>, AiError> {
    let filter = match scope {
        Scope::Project(f) => *f,
        _ => ProjectFilter::All,
    };
    let in_scope: std::collections::HashSet<DocumentId> = docs.iter().map(|d| d.id).collect();
    let mut score: HashMap<Citation, usize> = HashMap::new();
    let words = question
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4);
    for w in words {
        for hit in store.search(w, filter)? {
            if in_scope.contains(&hit.document_id) {
                *score
                    .entry(Citation {
                        document_id: hit.document_id,
                        paragraph_id: hit.paragraph_id,
                    })
                    .or_default() += 1;
            }
        }
    }
    let mut hits: Vec<(Citation, usize)> = score.into_iter().collect();
    hits.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(a.0.document_id.cmp(&b.0.document_id))
            .then(a.0.paragraph_id.cmp(&b.0.paragraph_id))
    });
    Ok(hits.into_iter().map(|(c, _)| c).collect())
}
