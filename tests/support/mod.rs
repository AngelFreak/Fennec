//! A tiny HTTP server that speaks enough of the Anthropic and OpenAI chat
//! protocols for tests. It records every request it receives.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wire {
    Anthropic,
    OpenAi,
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// All prompt text the model was sent (system + user).
    pub fn prompt_text(&self) -> String {
        let mut out = self.body["system"].as_str().unwrap_or_default().to_string();
        for m in self.body["messages"].as_array().into_iter().flatten() {
            out.push('\n');
            out.push_str(m["content"].as_str().unwrap_or_default());
        }
        out
    }

    pub fn wants_json(&self) -> bool {
        self.body.get("output_config").is_some() || self.body.get("response_format").is_some()
    }
}

pub enum Reply {
    /// A streamed answer, split into a few deltas.
    Text(String),
    /// A streamed answer that ends with this stop reason.
    Stop(String, &'static str),
    Status(u16, String),
    Models(Vec<&'static str>),
}

type Responder = dyn Fn(&Recorded) -> Reply + Send + Sync;

pub struct MockLlm {
    pub url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl MockLlm {
    pub fn start(wire: Wire, respond: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        let respond: Arc<Responder> = Arc::new(respond);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                let respond = Arc::clone(&respond);
                std::thread::spawn(move || handle(stream, wire, &log, respond.as_ref()));
            }
        });
        let url = match wire {
            Wire::Anthropic => format!("http://{addr}"),
            Wire::OpenAi => format!("http://{addr}/v1"),
        };
        Self { url, requests }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

fn handle(stream: TcpStream, wire: Wire, log: &Mutex<Vec<Recorded>>, respond: &Responder) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let (k, v) = (k.trim().to_string(), v.trim().to_string());
            if k.eq_ignore_ascii_case("content-length") {
                length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    let rec = Recorded {
        method,
        path,
        headers,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    };
    log.lock().unwrap().push(rec.clone());
    let reply = respond(&rec);
    let mut out = stream;
    let (status, ctype, body) = match reply {
        Reply::Status(code, msg) => (
            code,
            "application/json",
            json!({"error": {"message": msg}}).to_string(),
        ),
        Reply::Models(ids) => (
            200,
            "application/json",
            json!({"data": ids.iter().map(|id| json!({"id": id})).collect::<Vec<_>>()}).to_string(),
        ),
        Reply::Text(text) => (200, "text/event-stream", sse(wire, &text, None)),
        Reply::Stop(text, reason) => (200, "text/event-stream", sse(wire, &text, Some(reason))),
    };
    let _ = write!(
        out,
        "HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn pieces(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mid = chars.len() / 2;
    vec![chars[..mid].iter().collect(), chars[mid..].iter().collect()]
}

fn sse(wire: Wire, text: &str, stop: Option<&str>) -> String {
    let mut out = String::new();
    match wire {
        Wire::Anthropic => {
            let ev = |out: &mut String, name: &str, data: Value| {
                out.push_str(&format!("event: {name}\ndata: {data}\n\n"));
            };
            ev(
                &mut out,
                "message_start",
                json!({"type": "message_start", "message": {"id": "m"}}),
            );
            ev(
                &mut out,
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            );
            out.push_str(": ping\n\n");
            for p in pieces(text) {
                ev(
                    &mut out,
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": p}}),
                );
            }
            ev(
                &mut out,
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            );
            ev(
                &mut out,
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": stop.unwrap_or("end_turn")}}),
            );
            ev(&mut out, "message_stop", json!({"type": "message_stop"}));
        }
        Wire::OpenAi => {
            for p in pieces(text) {
                out.push_str(&format!(
                    "data: {}\n\n",
                    json!({"choices": [{"index": 0, "delta": {"content": p}, "finish_reason": null}]})
                ));
            }
            out.push_str(&format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices": [{"index": 0, "delta": {}, "finish_reason": stop.unwrap_or("stop")}]})
            ));
        }
    }
    out
}
