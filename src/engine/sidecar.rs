//! Engines that run in a Python helper process (models whisper.cpp cannot
//! load, such as hviske-v6). See `scripts/hviske_sidecar.py` for the
//! protocol: one JSON line per message, audio as raw little-endian f32.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

use super::{EngineError, SAMPLE_RATE, Segment, TranscribeOptions, Transcriber};

const HELPER: &str = include_str!("../../scripts/hviske_sidecar.py");

/// Longest piece sent in one request; the model reads at most 30 s.
const MAX_PIECE: usize = 28 * SAMPLE_RATE as usize;

/// Whether `dir` holds a model that needs the helper (hviske-v6 ships its
/// own `processing_whisper_qwen.py`).
pub fn is_sidecar_model(dir: &Path) -> bool {
    dir.join("processing_whisper_qwen.py").is_file()
}

pub struct SidecarEngine {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    /// Keeps the written helper script alive while the process runs.
    _script: Option<tempfile::TempDir>,
    pub device: String,
}

impl SidecarEngine {
    /// Starts the bundled helper with `python` for the model in `model_dir`.
    pub fn hviske(python: &str, model_dir: &Path) -> Result<Self, EngineError> {
        if !model_dir.is_dir() {
            return Err(EngineError::ModelMissing(model_dir.to_path_buf()));
        }
        let dir = tempfile::tempdir().map_err(|e| EngineError::Other(e.to_string()))?;
        let script = dir.path().join("hviske_sidecar.py");
        std::fs::write(&script, HELPER).map_err(|e| EngineError::Other(e.to_string()))?;
        let mut cmd = Command::new(python);
        cmd.arg("-u").arg(&script).arg(model_dir);
        let mut engine = Self::spawn(cmd, model_dir)?;
        engine._script = Some(dir);
        Ok(engine)
    }

    /// Starts `cmd` and waits until it reports that the model is loaded.
    pub fn spawn(mut cmd: Command, model: &Path) -> Result<Self, EngineError> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| EngineError::Other(format!("could not start the model helper: {e}")))?;
        forward_logs(&mut child);
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        let mut engine = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            _script: None,
            device: String::new(),
        };
        let hello = engine.read_message().map_err(|e| load_error(model, &e))?;
        if let Some(e) = hello["error"].as_str() {
            return Err(load_error(model, e));
        }
        if hello["ready"] != true {
            return Err(load_error(model, &format!("unexpected reply {hello}")));
        }
        engine.device = hello["device"].as_str().unwrap_or_default().to_string();
        tracing::info!(model = %model.display(), device = %engine.device, "model helper ready");
        Ok(engine)
    }

    fn read_message(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.stdout.read_line(&mut line).map_err(|e| e.to_string())?;
            if n == 0 {
                let status = self.child.wait().map(|s| s.to_string()).unwrap_or_default();
                return Err(format!("the model helper stopped ({status})"));
            }
            // Libraries sometimes print to stdout; only JSON objects are ours.
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && v.is_object()
            {
                return Ok(v);
            }
            tracing::debug!(target: "fennec::sidecar", "{}", line.trim_end());
        }
    }

    fn request(&mut self, pcm: &[f32]) -> Result<String, EngineError> {
        let id = self.next_id;
        self.next_id += 1;
        let header = json!({"id": id, "samples": pcm.len(), "punctuated": true});
        let mut bytes = Vec::with_capacity(pcm.len() * 4 + 64);
        bytes.extend_from_slice(header.to_string().as_bytes());
        bytes.push(b'\n');
        for s in pcm {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let stdin = self.stdin.as_mut().ok_or(EngineError::WorkerStopped)?;
        if let Err(e) = stdin.write_all(&bytes).and_then(|()| stdin.flush()) {
            let why = self.read_message().err().unwrap_or_else(|| e.to_string());
            return Err(EngineError::Other(why));
        }
        let reply = self.read_message().map_err(EngineError::Other)?;
        if reply["id"].as_u64() != Some(id) {
            return Err(EngineError::Other(format!(
                "the model helper answered out of turn: {reply}"
            )));
        }
        if let Some(e) = reply["error"].as_str() {
            return Err(EngineError::Other(format!("the model helper failed: {e}")));
        }
        Ok(reply["text"].as_str().unwrap_or_default().trim().to_string())
    }
}

impl Transcriber for SidecarEngine {
    /// One segment per ≤28 s piece, spanning the piece; this model gives no
    /// word timings or confidences.
    fn transcribe(&mut self, pcm: &[f32], _opts: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        let mut out = Vec::new();
        for (i, piece) in pcm.chunks(MAX_PIECE).enumerate() {
            let text = self.request(piece)?;
            if text.is_empty() {
                continue;
            }
            let start = i * MAX_PIECE;
            out.push(Segment {
                start_ms: samples_to_ms(start),
                end_ms: samples_to_ms(start + piece.len()),
                text,
                low_confidence: Vec::new(),
            });
        }
        Ok(out)
    }
}

impl Drop for SidecarEngine {
    fn drop(&mut self) {
        // Closing stdin ends the helper's loop; kill it if it lingers.
        self.stdin.take();
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn samples_to_ms(n: usize) -> i64 {
    (n as i64 * 1000) / i64::from(SAMPLE_RATE)
}

fn load_error(model: &Path, why: &str) -> EngineError {
    EngineError::Other(format!("could not load {}: {why}", model.display()))
}

/// Helper stderr goes to the log, so Python tracebacks are not lost.
fn forward_logs(child: &mut Child) {
    if let Some(err) = child.stderr.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                tracing::debug!(target: "fennec::sidecar", "{line}");
            }
        });
    }
}
