//! The native Anthropic Messages API (`/v1/messages`), always streamed.

use std::io::BufReader;

use serde_json::{Value, json};

use super::sse::read_events;
use super::{
    AiError, Cancel, LlmProvider, ProviderConfig, StructuredRequest, TextRequest, error_message, http_client,
    join, parse_json,
};

const VERSION: &str = "2023-06-01";

pub struct Anthropic {
    cfg: ProviderConfig,
    key: Option<String>,
    client: reqwest::blocking::Client,
}

impl Anthropic {
    pub fn new(cfg: ProviderConfig, key: Option<String>) -> Self {
        Self {
            cfg,
            key,
            client: http_client(),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::blocking::RequestBuilder {
        let mut r = self
            .client
            .request(method, join(&self.cfg.base_url, path))
            .header("anthropic-version", VERSION);
        if let Some(k) = &self.key {
            r = r.header("x-api-key", k);
        }
        r
    }

    fn check_status(
        &self,
        resp: reqwest::blocking::Response,
    ) -> Result<reqwest::blocking::Response, AiError> {
        let status = resp.status().as_u16();
        if resp.status().is_success() {
            return Ok(resp);
        }
        let body = resp.text().unwrap_or_default();
        let message = error_message(&body);
        let provider = self.cfg.name.clone();
        Err(match status {
            401 | 403 => AiError::Auth { provider, status },
            413 => AiError::ContextTooLong { provider },
            400 if message.contains("too long") => AiError::ContextTooLong { provider },
            _ => AiError::Http {
                provider,
                status,
                message,
            },
        })
    }

    /// Sends `body` with `stream: true`, feeding text deltas to `on_delta`.
    fn stream(
        &self,
        mut body: Value,
        cancel: &Cancel,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, AiError> {
        body["stream"] = json!(true);
        let url = join(&self.cfg.base_url, "/v1/messages");
        let resp = self
            .request(reqwest::Method::POST, "/v1/messages")
            .json(&body)
            .send()
            .map_err(|e| AiError::Network {
                url: url.clone(),
                message: e.to_string(),
            })?;
        let resp = self.check_status(resp)?;

        let mut text = String::new();
        let mut stop_reason: Option<String> = None;
        let mut failure: Option<AiError> = None;
        read_events(BufReader::new(resp), |ev| {
            if cancel.is_cancelled() {
                failure = Some(AiError::Cancelled);
                return false;
            }
            let Ok(data) = serde_json::from_str::<Value>(&ev.data) else {
                return true;
            };
            match data["type"].as_str().unwrap_or(&ev.event) {
                "content_block_delta" if data["delta"]["type"] == "text_delta" => {
                    if let Some(t) = data["delta"]["text"].as_str() {
                        text.push_str(t);
                        on_delta(t);
                    }
                }
                "message_delta" => {
                    if let Some(r) = data["delta"]["stop_reason"].as_str() {
                        stop_reason = Some(r.to_string());
                    }
                }
                "error" => {
                    failure = Some(AiError::Http {
                        provider: self.cfg.name.clone(),
                        status: 200,
                        message: data["error"]["message"]
                            .as_str()
                            .unwrap_or("stream error")
                            .to_string(),
                    });
                    return false;
                }
                "message_stop" => return false,
                _ => {}
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
        match stop_reason.as_deref() {
            Some("refusal") => Err(AiError::Refusal { provider }),
            Some("max_tokens") => Err(AiError::Truncated),
            Some("model_context_window_exceeded") => Err(AiError::ContextTooLong { provider }),
            _ => Ok(text),
        }
    }

    fn body(&self, system: &str, prompt: &str, max_tokens: u32) -> Value {
        json!({
            "model": self.cfg.model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{"role": "user", "content": prompt}],
        })
    }
}

impl LlmProvider for Anthropic {
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

    fn structured(&self, req: &StructuredRequest, cancel: &Cancel) -> Result<Value, AiError> {
        let mut body = self.body(&req.system, &req.prompt, req.max_tokens);
        body["output_config"] = json!({"format": {"type": "json_schema", "schema": req.schema}});
        let mut last = String::new();
        for _ in 0..2 {
            let text = self.stream(body.clone(), cancel, &mut |_| {})?;
            match parse_json(&text) {
                Ok(v) => return Ok(v),
                Err(e) => last = e,
            }
        }
        Err(AiError::InvalidJson {
            provider: self.cfg.name.clone(),
            message: last,
        })
    }

    fn list_models(&self) -> Result<Vec<String>, AiError> {
        let url = join(&self.cfg.base_url, "/v1/models");
        let resp = self
            .request(reqwest::Method::GET, "/v1/models?limit=100")
            .send()
            .map_err(|e| AiError::Network {
                url,
                message: e.to_string(),
            })?;
        let v: Value = self
            .check_status(resp)?
            .json()
            .map_err(|e| AiError::InvalidJson {
                provider: self.cfg.name.clone(),
                message: e.to_string(),
            })?;
        Ok(model_ids(&v))
    }
}

/// `data[].id` from a model list (both protocols use this shape).
pub(crate) fn model_ids(v: &Value) -> Vec<String> {
    v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
