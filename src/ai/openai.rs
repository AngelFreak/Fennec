//! OpenAI-compatible chat completions: ChatGPT, Ollama, llama.cpp,
//! LM Studio, vLLM and similar servers.

use std::io::BufReader;

use serde_json::{Value, json};

use super::anthropic::model_ids;
use super::sse::read_events;
use super::{
    AiError, Cancel, LlmProvider, ProviderConfig, StructuredRequest, TextRequest, error_message, http_client,
    join, parse_json,
};

pub struct OpenAi {
    cfg: ProviderConfig,
    key: Option<String>,
    client: reqwest::blocking::Client,
}

impl OpenAi {
    pub fn new(cfg: ProviderConfig, key: Option<String>) -> Self {
        Self {
            cfg,
            key,
            client: http_client(),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::blocking::RequestBuilder {
        let r = self.client.request(method, join(&self.cfg.base_url, path));
        match &self.key {
            Some(k) if !k.is_empty() => r.bearer_auth(k),
            _ => r,
        }
    }

    fn error(&self, status: u16, body: &str) -> AiError {
        let message = error_message(body);
        let provider = self.cfg.name.clone();
        match status {
            401 | 403 => AiError::Auth { provider, status },
            413 => AiError::ContextTooLong { provider },
            400 if message.contains("context") && message.contains("length") => {
                AiError::ContextTooLong { provider }
            }
            _ => AiError::Http {
                provider,
                status,
                message,
            },
        }
    }

    fn stream(
        &self,
        mut body: Value,
        cancel: &Cancel,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, AiError> {
        body["stream"] = json!(true);
        let url = join(&self.cfg.base_url, "/chat/completions");
        let resp = self
            .request(reqwest::Method::POST, "/chat/completions")
            .json(&body)
            .send()
            .map_err(|e| AiError::Network {
                url: url.clone(),
                message: e.to_string(),
            })?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            return Err(self.error(status, &resp.text().unwrap_or_default()));
        }
        let mut text = String::new();
        let mut finish: Option<String> = None;
        let mut failure: Option<AiError> = None;
        read_events(BufReader::new(resp), |ev| {
            if cancel.is_cancelled() {
                failure = Some(AiError::Cancelled);
                return false;
            }
            if ev.data.trim() == "[DONE]" {
                return false;
            }
            let Ok(data) = serde_json::from_str::<Value>(&ev.data) else {
                return true;
            };
            if let Some(m) = data["error"]["message"].as_str() {
                failure = Some(AiError::Http {
                    provider: self.cfg.name.clone(),
                    status: 200,
                    message: m.to_string(),
                });
                return false;
            }
            let choice = &data["choices"][0];
            if let Some(t) = choice["delta"]["content"].as_str() {
                text.push_str(t);
                on_delta(t);
            }
            if let Some(r) = choice["delta"]["refusal"].as_str()
                && !r.is_empty()
            {
                finish = Some("refusal".into());
            }
            if let Some(r) = choice["finish_reason"].as_str() {
                finish.get_or_insert(r.to_string());
            }
            true
        })
        .map_err(|e| AiError::Network {
            url,
            message: format!("stream interrupted: {e}"),
        })?;
        if let Some(e) = failure {
            return Err(e);
        }
        let provider = self.cfg.name.clone();
        match finish.as_deref() {
            Some("refusal") | Some("content_filter") => Err(AiError::Refusal { provider }),
            Some("length") => Err(AiError::Truncated),
            _ => Ok(text),
        }
    }

    fn body(&self, system: &str, prompt: &str, max_tokens: u32) -> Value {
        let mut body = json!({
            "model": self.cfg.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": prompt},
            ],
        });
        // OpenAI's own API wants the newer name; local servers know `max_tokens`.
        let key = if self.cfg.base_url.contains("api.openai.com") {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        body[key] = json!(max_tokens);
        body
    }
}

impl LlmProvider for OpenAi {
    fn config(&self) -> &ProviderConfig {
        &self.cfg
    }

    fn stream_text(
        &self,
        req: &TextRequest,
        cancel: &Cancel,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, AiError> {
        self.stream(
            self.body(&req.system, &req.prompt, req.max_tokens),
            cancel,
            on_delta,
        )
    }

    /// Tries the JSON-schema response format first. A server that rejects
    /// it (HTTP 400/422) gets the schema in the prompt instead.
    fn structured(&self, req: &StructuredRequest, cancel: &Cancel) -> Result<Value, AiError> {
        let mut with_schema = self.body(&req.system, &req.prompt, req.max_tokens);
        with_schema["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {"name": req.schema_name, "schema": req.schema, "strict": true},
        });
        let prompt_only = self.body(
            &req.system,
            &format!(
                "{}\n\nSvar kun med JSON, der følger dette skema:\n{}",
                req.prompt, req.schema
            ),
            req.max_tokens,
        );
        let mut body = with_schema;
        let mut last = String::new();
        let mut attempts = 0;
        while attempts < 2 {
            match self.stream(body.clone(), cancel, &mut |_| {}) {
                Ok(text) => match parse_json(&text) {
                    Ok(v) => return Ok(v),
                    Err(e) => last = e,
                },
                Err(AiError::Http {
                    status: 400 | 422, ..
                }) if body.get("response_format").is_some() => {
                    tracing::info!(provider = %self.cfg.id, "no JSON-schema mode; asking in the prompt");
                    body = prompt_only.clone();
                    continue;
                }
                Err(e) => return Err(e),
            }
            attempts += 1;
        }
        Err(AiError::InvalidJson {
            provider: self.cfg.name.clone(),
            message: last,
        })
    }

    fn list_models(&self) -> Result<Vec<String>, AiError> {
        let url = join(&self.cfg.base_url, "/models");
        let resp = self
            .request(reqwest::Method::GET, "/models")
            .send()
            .map_err(|e| AiError::Network {
                url,
                message: e.to_string(),
            })?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            return Err(self.error(status, &resp.text().unwrap_or_default()));
        }
        let v: Value = resp.json().map_err(|e| AiError::InvalidJson {
            provider: self.cfg.name.clone(),
            message: e.to_string(),
        })?;
        let mut ids = model_ids(&v);
        ids.sort();
        Ok(ids)
    }
}
