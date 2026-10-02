//! Optional AI: language-model providers and the actions built on them.
//!
//! Two wire protocols sit behind [`LlmProvider`]: the native Anthropic
//! Messages API and the OpenAI-compatible chat API (ChatGPT, Ollama,
//! llama.cpp, LM Studio, vLLM, …). Requests are blocking and run on worker
//! threads; nothing here touches GTK.

pub mod actions;
pub mod anthropic;
pub mod diff;
pub mod keys;
pub mod openai;
pub mod privacy;
pub mod service;
mod sse;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

pub use keys::{Keyring, MemorySecrets, SecretStore};
pub use service::AiService;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Anthropic,
    OpenAi,
}

/// Where a provider runs; decides what the privacy gate allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Locality {
    ThisComputer,
    Network,
    Cloud,
}

impl Locality {
    pub fn label(self) -> &'static str {
        match self {
            Locality::ThisComputer => "This computer",
            Locality::Network => "Local network",
            Locality::Cloud => "Cloud",
        }
    }
}

/// One configured provider. The API key lives in the keyring under `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    /// Anthropic: `https://api.anthropic.com`; OpenAI-style: the URL that
    /// `/chat/completions` hangs off, e.g. `http://localhost:11434/v1`.
    pub base_url: String,
    pub model: String,
    pub locality: Locality,
    /// Paragraph text budget per request, in characters. Longer inputs are
    /// split and combined (map-reduce).
    #[serde(default = "default_context_chars")]
    pub context_chars: usize,
}

fn default_context_chars() -> usize {
    60_000
}

/// A ready-made provider the user can add and adjust.
pub struct Preset {
    pub label: &'static str,
    pub config: ProviderConfig,
    pub needs_key: bool,
}

pub fn presets() -> Vec<Preset> {
    let make = |id: &str, name: &str, protocol, base: &str, model: &str, locality, chars| ProviderConfig {
        id: id.into(),
        name: name.into(),
        protocol,
        base_url: base.into(),
        model: model.into(),
        locality,
        context_chars: chars,
    };
    vec![
        Preset {
            label: "Claude (Anthropic)",
            config: make(
                "claude",
                "Claude",
                Protocol::Anthropic,
                "https://api.anthropic.com",
                "claude-opus-5-5",
                Locality::Cloud,
                400_000,
            ),
            needs_key: true,
        },
        Preset {
            label: "ChatGPT (OpenAI)",
            config: make(
                "chatgpt",
                "ChatGPT",
                Protocol::OpenAi,
                "https://api.openai.com/v1",
                "gpt-5",
                Locality::Cloud,
                200_000,
            ),
            needs_key: true,
        },
        Preset {
            label: "Ollama on this computer",
            config: make(
                "ollama",
                "Ollama",
                Protocol::OpenAi,
                "http://localhost:11434/v1",
                "",
                Locality::ThisComputer,
                24_000,
            ),
            needs_key: false,
        },
        Preset {
            label: "llama.cpp / LM Studio / vLLM",
            config: make(
                "local-server",
                "Local server",
                Protocol::OpenAi,
                "http://localhost:8080/v1",
                "",
                Locality::ThisComputer,
                24_000,
            ),
            needs_key: false,
        },
        Preset {
            label: "Server on my network",
            config: make(
                "network",
                "Network server",
                Protocol::OpenAi,
                "http://192.168.1.10:11434/v1",
                "",
                Locality::Network,
                24_000,
            ),
            needs_key: false,
        },
        Preset {
            label: "Other OpenAI-compatible cloud",
            config: make(
                "other-cloud",
                "Cloud service",
                Protocol::OpenAi,
                "https://",
                "",
                Locality::Cloud,
                100_000,
            ),
            needs_key: true,
        },
    ]
}

/// The kinds of AI work; each can use its own provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AiJob {
    Summaries,
    Cleanup,
    ActionItems,
    Ask,
    FillFields,
}

impl AiJob {
    pub const ALL: [AiJob; 5] = [
        AiJob::Summaries,
        AiJob::Cleanup,
        AiJob::ActionItems,
        AiJob::Ask,
        AiJob::FillFields,
    ];

    /// Key in `settings.toml` (`[ai.jobs]`).
    pub fn key(self) -> &'static str {
        match self {
            AiJob::Summaries => "summaries",
            AiJob::Cleanup => "cleanup",
            AiJob::ActionItems => "action-items",
            AiJob::Ask => "ask",
            AiJob::FillFields => "fill-fields",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AiJob::Summaries => "Summaries",
            AiJob::Cleanup => "Clean up text",
            AiJob::ActionItems => "Action items",
            AiJob::Ask => "Ask the project",
            AiJob::FillFields => "Fill template fields",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            AiJob::Summaries => "Document and project summaries",
            AiJob::Cleanup => "Grammar and filler words, shown as a diff",
            AiJob::ActionItems => "What, who, when",
            AiJob::Ask => "Questions answered with citations",
            AiJob::FillFields => "Suggestions you accept one by one",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// Off by default: no request is ever made until the user turns this on.
    pub enabled: bool,
    pub providers: Vec<ProviderConfig>,
    /// `id` of the provider actions use.
    pub default_provider: String,
    /// Language the model should answer in.
    pub language: String,
    /// Provider per job ([`AiJob::key`] → provider `id`). A job that is not
    /// listed, or whose provider is gone, uses the default provider.
    pub jobs: BTreeMap<String, String>,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            providers: Vec::new(),
            default_provider: String::new(),
            language: "dansk".into(),
            jobs: BTreeMap::new(),
        }
    }
}

impl AiSettings {
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn active(&self) -> Option<&ProviderConfig> {
        self.provider(&self.default_provider)
            .or_else(|| self.providers.first())
    }

    /// The provider that runs `job`: its own choice, else the default.
    pub fn for_job(&self, job: AiJob) -> Option<&ProviderConfig> {
        self.jobs
            .get(job.key())
            .and_then(|id| self.provider(id))
            .or_else(|| self.active())
    }

    /// A fresh id for a provider made from `base`, not clashing with existing ones.
    pub fn unique_id(&self, base: &str) -> String {
        if self.provider(base).is_none() {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| self.provider(id).is_none())
            .expect("an unused id exists")
    }
}

#[derive(Debug, Clone)]
pub struct TextRequest {
    pub system: String,
    pub prompt: String,
    pub max_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct StructuredRequest {
    pub system: String,
    pub prompt: String,
    pub schema_name: String,
    /// JSON Schema; objects must set `additionalProperties: false`.
    pub schema: serde_json::Value,
    pub max_tokens: u32,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AiError {
    #[error("AI is turned off in Settings")]
    Disabled,
    #[error("no AI provider is set up")]
    NoProvider,
    #[error("could not reach {url}: {message}")]
    Network { url: String, message: String },
    #[error("{provider} rejected the API key (HTTP {status}); check it in Settings")]
    Auth { provider: String, status: u16 },
    #[error("{provider} declined to answer this request")]
    Refusal { provider: String },
    #[error("the text is too long for {provider}; lower its context size in Settings")]
    ContextTooLong { provider: String },
    #[error("the answer was cut off before it finished")]
    Truncated,
    #[error("{provider} did not return valid data: {message}")]
    InvalidJson { provider: String, message: String },
    #[error("{provider} returned HTTP {status}: {message}")]
    Http {
        provider: String,
        status: u16,
        message: String,
    },
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Privacy(#[from] privacy::PrivacyError),
    #[error("{0}")]
    Store(String),
}

impl From<crate::store::StoreError> for AiError {
    fn from(e: crate::store::StoreError) -> Self {
        AiError::Store(e.to_string())
    }
}

/// A cancel flag shared between the UI and a running request.
#[derive(Debug, Default)]
pub struct Cancel(AtomicBool);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

pub trait LlmProvider: Send + Sync {
    fn config(&self) -> &ProviderConfig;

    /// Streams text, calling `on_delta` per piece; returns the whole text.
    fn stream_text(
        &self,
        req: &TextRequest,
        cancel: &Cancel,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, AiError>;

    /// Returns JSON matching `req.schema`, using the API's schema mode where
    /// it has one. Invalid JSON is retried once, then reported.
    fn structured(&self, req: &StructuredRequest, cancel: &Cancel) -> Result<serde_json::Value, AiError>;

    fn list_models(&self) -> Result<Vec<String>, AiError>;
}

/// Builds the provider for `cfg`, reading its key from `secrets`.
pub fn connect(cfg: &ProviderConfig, secrets: &dyn SecretStore) -> Box<dyn LlmProvider> {
    let key = secrets.get(&cfg.id);
    match cfg.protocol {
        Protocol::Anthropic => Box::new(anthropic::Anthropic::new(cfg.clone(), key)),
        Protocol::OpenAi => Box::new(openai::OpenAi::new(cfg.clone(), key)),
    }
}

pub(crate) fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        // Long answers stream for minutes.
        .timeout(None)
        .build()
        .expect("TLS backend available")
}

/// Joins a base URL and a path without doubling or dropping the slash.
pub(crate) fn join(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

/// Short message from an error body: the API's `error.message` if present.
pub(crate) fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(300).collect())
}

/// Parses JSON a model wrote, tolerating a ```json fence around it.
pub(crate) fn parse_json(text: &str) -> Result<serde_json::Value, String> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(t)
        .trim();
    serde_json::from_str(t).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_is_off_by_default() {
        assert!(!AiSettings::default().enabled);
    }

    #[test]
    fn unique_ids_do_not_clash() {
        let mut s = AiSettings::default();
        s.providers.push(presets().remove(0).config);
        assert_eq!(s.unique_id("claude"), "claude-2");
        assert_eq!(s.unique_id("ollama"), "ollama");
    }

    #[test]
    fn the_default_provider_falls_back_to_the_first() {
        let mut s = AiSettings::default();
        assert!(s.active().is_none());
        s.providers.push(presets().remove(2).config);
        assert_eq!(s.active().unwrap().id, "ollama");
    }

    #[test]
    fn a_job_uses_its_own_provider_else_the_default() {
        let mut s = AiSettings::default();
        s.providers.push(presets().remove(0).config);
        s.providers.push(presets().remove(2).config);
        s.default_provider = "claude".into();
        s.jobs.insert(AiJob::Cleanup.key().into(), "ollama".into());
        s.jobs.insert(AiJob::Ask.key().into(), "gone".into());
        assert_eq!(s.for_job(AiJob::Cleanup).unwrap().id, "ollama");
        assert_eq!(s.for_job(AiJob::Summaries).unwrap().id, "claude");
        assert_eq!(
            s.for_job(AiJob::Ask).unwrap().id,
            "claude",
            "a removed provider falls back to the default"
        );
    }

    #[test]
    fn job_choices_survive_a_round_trip_and_old_files_have_none() {
        let mut s = AiSettings::default();
        s.jobs.insert(AiJob::ActionItems.key().into(), "x".into());
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<AiSettings>(&text).unwrap(), s);
        let old: AiSettings = toml::from_str("enabled = true\n").unwrap();
        assert!(old.jobs.is_empty());
    }

    #[test]
    fn json_in_a_code_fence_is_accepted() {
        assert_eq!(parse_json("```json\n{\"a\": 1}\n```").unwrap()["a"], 1);
        assert!(parse_json("not json").is_err());
    }

    #[test]
    fn urls_join_with_one_slash() {
        assert_eq!(join("http://h/v1/", "/models"), "http://h/v1/models");
    }

    #[test]
    fn error_bodies_yield_the_api_message() {
        assert_eq!(error_message(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(error_message("plain"), "plain");
    }
}
