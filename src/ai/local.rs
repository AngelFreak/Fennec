//! Built-in AI: language models Fennec downloads like its speech models and
//! runs on this computer with llama.cpp's server (`llama-server`, shipped in
//! the package). The server starts on the first request, on 127.0.0.1 at a
//! free port, and stops after [`IDLE`] without use, so the model's memory
//! (5–8 GB) is only taken while the AI works.
//!
//! A built-in provider is a [`ProviderConfig`] with [`Protocol::Builtin`]
//! whose `base_url` is the model file; requests go to the server through the
//! OpenAI-compatible client.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use super::openai::OpenAi;
use super::{
    AiError, Cancel, LlmProvider, Locality, Protocol, ProviderConfig, StructuredRequest, TextRequest,
};

/// A model Fennec offers to download for the AI jobs.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalModel {
    pub id: &'static str,
    pub name: &'static str,
    pub publisher: &'static str,
    pub description: &'static str,
    pub license: &'static str,
    /// "q4_0 · 7.0 GB"
    pub size: &'static str,
    pub repo: &'static str,
    pub file: &'static str,
    pub recommended: bool,
}

/// Both Gemma 4 in Google's own quantisation-aware q4_0 builds (Apache 2.0).
/// On the EuroEval Danish leaderboard (October 2026) the 12B is the best open
/// model of its size; Danish fine-tunes of it scored no better. On a Radeon
/// 780M, meeting notes for a 600-word Danish meeting took about 100 s with
/// either: the 12B writes at 10 tokens/s against the E4B's 24, but less. The
/// 12B got decisions, names and dates right where the E4B slipped; the E4B
/// proofreads as well and 2.5× faster.
pub fn catalog() -> Vec<LocalModel> {
    vec![
        LocalModel {
            id: "gemma-4-12b",
            name: "Gemma 4 12B",
            publisher: "Google",
            description: "The most accurate in Danish of the models that run on a laptop. Best for meeting notes.",
            license: "Apache 2.0",
            size: "q4_0 · 7.0 GB",
            repo: "google/gemma-4-12B-it-qat-q4_0-gguf",
            file: "gemma-4-12b-it-qat-q4_0.gguf",
            recommended: true,
        },
        LocalModel {
            id: "gemma-4-e4b",
            name: "Gemma 4 E4B",
            publisher: "Google",
            description: "Smaller and twice as fast. Proofreads as well; its meeting notes need a closer check.",
            license: "Apache 2.0",
            size: "q4_0 · 5.2 GB",
            repo: "google/gemma-4-E4B-it-qat-q4_0-gguf",
            file: "gemma-4-E4B_q4_0-it.gguf",
            recommended: false,
        },
    ]
}

pub fn path_of(m: &LocalModel, models_dir: &Path) -> PathBuf {
    models_dir.join("ai").join(m.file)
}

pub fn is_installed(m: &LocalModel, models_dir: &Path) -> bool {
    path_of(m, models_dir).is_file()
}

/// The provider for an installed model.
pub fn provider_for(m: &LocalModel, models_dir: &Path) -> ProviderConfig {
    ProviderConfig {
        id: format!("fennec-{}", m.id),
        name: m.name.to_string(),
        protocol: Protocol::Builtin,
        base_url: path_of(m, models_dir).to_string_lossy().into_owned(),
        model: m.id.to_string(),
        locality: Locality::ThisComputer,
        // Proofreading writes back as much as it reads; with a 32k-token
        // window that leaves room for about 30,000 characters each way.
        context_chars: 30_000,
    }
}

/// Tokens the server keeps in view: the prompt and the answer together.
const CONTEXT_TOKENS: u32 = 32_768;
/// How long the server stays up after the last request.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// Loading 7 GB from a slow disk can take a while.
const START_TIMEOUT: Duration = Duration::from_secs(180);

/// Where `llama-server` is: `$FENNEC_LLAMA_SERVER`, the package's copy, or
/// one on the PATH.
pub fn server_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FENNEC_LLAMA_SERVER") {
        return Some(PathBuf::from(p));
    }
    let packaged = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|bin| bin.join("../lib/fennec/llama/llama-server"))
        })
        .filter(|p| p.is_file());
    packaged.or_else(|| {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join("llama-server"))
                .find(|p| p.is_file())
        })
    })
}

/// The server's command line for `model` on `port`.
pub fn server_args(model: &Path, port: u16) -> Vec<String> {
    [
        "--model",
        &model.to_string_lossy(),
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--ctx-size",
        &CONTEXT_TOKENS.to_string(),
        // Up to four requests decode together (proofreading sends its parts
        // at once), sharing one window that a single request can fill.
        "--parallel",
        "4",
        "--kv-unified",
        // Every layer on the GPU if there is one; llama.cpp uses the CPU otherwise.
        "--n-gpu-layers",
        "99",
        // The model's own chat template, which can turn its thinking off.
        "--jinja",
        "--reasoning-budget",
        "0",
        "--no-webui",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

struct Running {
    child: Child,
    model: PathBuf,
    url: String,
    /// Requests in flight; the server is not stopped under them.
    busy: usize,
    last_used: Instant,
}

static SERVER: Mutex<Option<Running>> = Mutex::new(None);

/// The URL of a server running `model`, starting it if needed. Hold the
/// lease for the length of the request.
fn lease(model: &Path, cancel: &Cancel) -> Result<Lease, AiError> {
    // For testing against a server started by hand.
    if let Ok(url) = std::env::var("FENNEC_LLAMA_SERVER_URL") {
        return Ok(Lease { url, counted: false });
    }
    if !model.is_file() {
        return Err(start_error(
            model,
            "the model file is missing; download it again in Settings",
        ));
    }
    let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    let alive = guard
        .as_mut()
        .is_some_and(|r| r.model == model && matches!(r.child.try_wait(), Ok(None)));
    if !alive {
        if let Some(mut old) = guard.take() {
            stop(&mut old.child);
        }
        *guard = Some(start(model, cancel)?);
        watch_idle();
    }
    let r = guard.as_mut().expect("started above");
    r.busy += 1;
    Ok(Lease {
        url: r.url.clone(),
        counted: true,
    })
}

/// Whether a server is up with `model`, so a request starts at once.
fn is_running(model: &Path) -> bool {
    if std::env::var_os("FENNEC_LLAMA_SERVER_URL").is_some() {
        return true;
    }
    let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .as_mut()
        .is_some_and(|r| r.model == model && matches!(r.child.try_wait(), Ok(None)))
}

struct Lease {
    url: String,
    counted: bool,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if !self.counted {
            return;
        }
        let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = guard.as_mut() {
            r.busy = r.busy.saturating_sub(1);
            r.last_used = Instant::now();
        }
    }
}

fn start_error(model: &Path, message: &str) -> AiError {
    AiError::Network {
        url: format!(
            "the built-in AI ({})",
            model.file_name().unwrap_or_default().to_string_lossy()
        ),
        message: message.to_string(),
    }
}

fn start(model: &Path, cancel: &Cancel) -> Result<Running, AiError> {
    let binary = server_binary().ok_or_else(|| {
        start_error(
            model,
            "llama-server is not installed; reinstall Fennec or set FENNEC_LLAMA_SERVER",
        )
    })?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| start_error(model, &format!("no free port: {e}")))?;
    let mut cmd = Command::new(&binary);
    cmd.args(server_args(model, port))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = spawn_tied(cmd).map_err(|e| start_error(model, &format!("{}: {e}", binary.display())))?;
    // Its log goes to ours; the last lines explain a failed start.
    let tail = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    if let Some(err) = child.stderr.take() {
        let tail = std::sync::Arc::clone(&tail);
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                tracing::debug!(target: "llama_server", "{line}");
                let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
                t.push(line);
                if t.len() > 5 {
                    t.remove(0);
                }
            }
        });
    }
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("TLS backend available");
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            stop(&mut child);
            return Err(AiError::Cancelled);
        }
        if let Ok(Some(status)) = child.try_wait() {
            let last = tail.lock().unwrap_or_else(|e| e.into_inner()).join(" / ");
            return Err(start_error(
                model,
                &format!("llama-server stopped ({status}): {last}"),
            ));
        }
        // 503 while the model loads, 200 once it is ready.
        if client
            .get(format!("{url}/health"))
            .send()
            .is_ok_and(|r| r.status().is_success())
        {
            tracing::info!(
                "built-in AI ready on {url} after {:.1} s",
                started.elapsed().as_secs_f32()
            );
            return Ok(Running {
                child,
                model: model.to_path_buf(),
                url: format!("{url}/v1"),
                busy: 0,
                last_used: Instant::now(),
            });
        }
        if started.elapsed() > START_TIMEOUT {
            stop(&mut child);
            return Err(start_error(model, "llama-server did not get ready in time"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Starts the server from one long-lived thread with a parent-death signal,
/// so it ends with Fennec even after a crash. (The signal fires when the
/// spawning thread ends, so it cannot be a worker that comes and goes.)
fn spawn_tied(cmd: Command) -> std::io::Result<Child> {
    type Job = (Command, mpsc::Sender<std::io::Result<Child>>);
    static SPAWNER: Mutex<Option<mpsc::Sender<Job>>> = Mutex::new(None);
    let (reply, answer) = mpsc::channel();
    {
        let mut guard = SPAWNER.lock().unwrap_or_else(|e| e.into_inner());
        let tx = guard.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("llama-spawner".into())
                .spawn(move || {
                    for (mut cmd, reply) in rx {
                        use std::os::unix::process::CommandExt;
                        // SAFETY: prctl is async-signal-safe and touches no memory.
                        unsafe {
                            cmd.pre_exec(|| {
                                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                                Ok(())
                            });
                        }
                        let _ = reply.send(cmd.spawn());
                    }
                })
                .expect("thread starts");
            tx
        });
        tx.send((cmd, reply))
            .map_err(|_| std::io::Error::other("the spawner thread is gone"))?;
    }
    answer
        .recv()
        .map_err(|_| std::io::Error::other("the spawner thread is gone"))?
}

/// Stops the server once it has been idle for [`IDLE`].
fn watch_idle() {
    static WATCH: Once = Once::new();
    WATCH.call_once(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
                if guard
                    .as_ref()
                    .is_some_and(|r| r.busy == 0 && r.last_used.elapsed() > IDLE)
                    && let Some(mut r) = guard.take()
                {
                    tracing::info!("stopping the idle built-in AI");
                    stop(&mut r.child);
                }
            }
        });
    });
}

/// Stops the server now (Fennec is quitting).
pub fn shutdown() {
    if let Some(mut r) = SERVER.lock().unwrap_or_else(|e| e.into_inner()).take() {
        stop(&mut r.child);
    }
}

/// A built-in model as a provider: starts the server, then speaks to it.
pub struct Builtin {
    cfg: ProviderConfig,
}

impl Builtin {
    pub fn new(cfg: ProviderConfig) -> Self {
        Self { cfg }
    }

    fn client(&self, cancel: &Cancel) -> Result<(Lease, OpenAi), AiError> {
        let lease = lease(Path::new(&self.cfg.base_url), cancel)?;
        let cfg = ProviderConfig {
            base_url: lease.url.clone(),
            ..self.cfg.clone()
        };
        Ok((lease, OpenAi::new(cfg, None).without_thinking()))
    }
}

impl LlmProvider for Builtin {
    fn config(&self) -> &ProviderConfig {
        &self.cfg
    }

    fn stream_text(
        &self,
        req: &TextRequest,
        cancel: &Cancel,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, AiError> {
        let (_lease, client) = self.client(cancel)?;
        client.stream_text(req, cancel, on_delta)
    }

    fn structured(&self, req: &StructuredRequest, cancel: &Cancel) -> Result<serde_json::Value, AiError> {
        let (_lease, client) = self.client(cancel)?;
        client.structured(req, cancel)
    }

    fn is_ready(&self) -> bool {
        is_running(Path::new(&self.cfg.base_url))
    }

    fn warm_up(&self, cancel: &Cancel) -> Result<(), AiError> {
        lease(Path::new(&self.cfg.base_url), cancel).map(drop)
    }

    fn list_models(&self) -> Result<Vec<String>, AiError> {
        if Path::new(&self.cfg.base_url).is_file() {
            Ok(vec![self.cfg.model.clone()])
        } else {
            Err(start_error(
                Path::new(&self.cfg.base_url),
                "the model file is missing",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_gets_the_whole_window_on_the_gpu_without_thinking() {
        let args = server_args(Path::new("/m/gemma.gguf"), 4321);
        let pair = |k: &str| {
            let i = args.iter().position(|a| a == k).unwrap();
            args[i + 1].clone()
        };
        assert_eq!(pair("--model"), "/m/gemma.gguf");
        assert_eq!(pair("--host"), "127.0.0.1");
        assert_eq!(pair("--port"), "4321");
        assert_eq!(pair("--parallel"), "4");
        assert!(args.contains(&"--kv-unified".to_string()));
        assert_eq!(pair("--reasoning-budget"), "0");
        assert!(args.contains(&"--jinja".to_string()));
    }

    #[test]
    fn a_downloaded_model_becomes_a_provider_on_this_computer() {
        let m = &catalog()[0];
        assert!(m.recommended);
        let cfg = provider_for(m, Path::new("/data/models"));
        assert_eq!(cfg.protocol, Protocol::Builtin);
        assert_eq!(cfg.locality, Locality::ThisComputer);
        assert_eq!(cfg.base_url, "/data/models/ai/gemma-4-12b-it-qat-q4_0.gguf");
        assert_eq!(cfg.id, "fennec-gemma-4-12b");
    }

    #[test]
    fn a_missing_model_says_so_before_starting_anything() {
        let err = lease(Path::new("/nonexistent/model.gguf"), &Cancel::default())
            .err()
            .unwrap();
        assert!(err.to_string().contains("missing"), "{err}");
    }
}
