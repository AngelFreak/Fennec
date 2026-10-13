//! Just enough HTTP/1.1 for the phone protocol: one request per connection,
//! bodies with a Content-Length, JSON answers.

use std::io::{self, Read, Write};

/// Header block limit; the phone sends a handful of short headers.
const MAX_HEAD: usize = 16 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// Path without the query, split on `/` (empty segments dropped).
    pub segments: Vec<String>,
    pub query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The `Authorization: Bearer` secret.
    pub fn bearer(&self) -> Option<&str> {
        let v = self.header("authorization")?;
        let (scheme, token) = v.split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
    }
}

#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    /// Malformed or unsupported; answered with 400.
    Bad(&'static str),
    /// Answered with 413.
    TooLarge,
}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> Self {
        ReadError::Io(e)
    }
}

pub fn read_request(stream: &mut impl Read, max_body: usize) -> Result<Request, ReadError> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_HEAD {
            return Err(ReadError::Bad("headers too long"));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(ReadError::Io(io::ErrorKind::UnexpectedEof.into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    match req.parse(&buf[..head_end]) {
        Ok(httparse::Status::Complete(_)) => {}
        _ => return Err(ReadError::Bad("malformed request")),
    }
    let method = req.method.unwrap_or_default().to_string();
    let target = req.path.unwrap_or_default();
    let headers: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|h| {
            (
                h.name.to_string(),
                String::from_utf8_lossy(h.value).trim().to_string(),
            )
        })
        .collect();
    if headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("transfer-encoding"))
    {
        return Err(ReadError::Bad("send a Content-Length, not chunked encoding"));
    }
    let length = match headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
    {
        Some((_, v)) => v
            .parse::<usize>()
            .map_err(|_| ReadError::Bad("bad Content-Length"))?,
        None => 0,
    };
    if length > max_body {
        return Err(ReadError::TooLarge);
    }

    let mut body = buf[head_end..].to_vec();
    if body.len() > length {
        return Err(ReadError::Bad("more data than Content-Length"));
    }
    let have = body.len();
    body.resize(length, 0);
    stream.read_exact(&mut body[have..])?;

    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Ok(Request {
        method,
        segments: path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(percent_decode)
            .collect(),
        query: query
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                (percent_decode(k), percent_decode(v))
            })
            .collect(),
        headers,
        body,
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match escaped {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: serde_json::Value,
}

impl Response {
    pub fn ok(body: serde_json::Value) -> Self {
        Self { status: 200, body }
    }

    /// An error the phone can show: `{"error": code, "message": text}`.
    pub fn error(status: u16, code: &str, message: &str) -> Self {
        Self {
            status,
            body: serde_json::json!({ "error": code, "message": message }),
        }
    }
}

pub fn write_response(stream: &mut impl Write, r: &Response) -> io::Result<()> {
    let body = serde_json::to_vec(&r.body).expect("JSON values serialize");
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        r.status,
        reason(r.status),
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &[u8]) -> Result<Request, ReadError> {
        read_request(&mut &raw[..], 1024)
    }

    #[test]
    fn a_request_with_a_body_query_and_bearer_is_read() {
        let r = parse(
            b"PUT /v1/recordings/ab-12/audio?offset=4096&x=a%2Cb HTTP/1.1\r\n\
              Authorization: Bearer s3cret\r\nContent-Length: 5\r\n\r\nhello",
        )
        .unwrap();
        assert_eq!(r.method, "PUT");
        assert_eq!(r.segments, ["v1", "recordings", "ab-12", "audio"]);
        assert_eq!(r.query("offset"), Some("4096"));
        assert_eq!(r.query("x"), Some("a,b"));
        assert_eq!(r.bearer(), Some("s3cret"));
        assert_eq!(r.body, b"hello");
    }

    #[test]
    fn oversized_chunked_truncated_and_malformed_requests_are_refused() {
        assert!(matches!(
            parse(b"PUT / HTTP/1.1\r\nContent-Length: 4096\r\n\r\n"),
            Err(ReadError::TooLarge)
        ));
        assert!(matches!(
            parse(b"PUT / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Err(ReadError::Bad(_))
        ));
        assert!(matches!(
            parse(b"PUT / HTTP/1.1\r\nContent-Length: 10\r\n\r\nshort"),
            Err(ReadError::Io(_))
        ));
        assert!(matches!(
            parse(b"\x00\x01 nonsense\r\n\r\n"),
            Err(ReadError::Bad(_))
        ));
        let endless = [b'a'; MAX_HEAD + 10];
        assert!(matches!(parse(&endless), Err(ReadError::Bad(_))));
    }

    #[test]
    fn percent_signs_that_are_not_escapes_are_kept() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
        assert_eq!(percent_decode("%C3%A6"), "æ");
    }

    #[test]
    fn responses_carry_length_and_json() {
        let mut out = Vec::new();
        write_response(&mut out, &Response::error(409, "offset", "Resend from 10")).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 409 Conflict\r\n"));
        let body = text.split("\r\n\r\n").nth(1).unwrap();
        assert!(text.contains(&format!("Content-Length: {}\r\n", body.len())));
        assert!(body.contains("\"error\":\"offset\""));
    }
}
