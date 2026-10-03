//! The speech models Fennec knows about, where they live, how to get them,
//! which compute backends this build can use, and a speed test.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::config::{Backend, Paths};
use crate::engine::{SAMPLE_RATE, TranscribeOptions, Transcriber};

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A ready GGML file on Hugging Face.
    Ggml { repo: &'static str, file: &'static str },
    /// A Hugging Face checkpoint converted locally (needs Python with torch).
    Convert { repo: &'static str, quant: &'static str },
    /// A whole Hugging Face repository, run by the Python helper.
    Snapshot { repo: &'static str },
}

#[derive(Debug, Clone)]
pub struct CatalogModel {
    pub id: &'static str,
    pub name: &'static str,
    pub publisher: &'static str,
    pub description: &'static str,
    pub license: &'static str,
    pub non_commercial: bool,
    pub size: &'static str,
    /// Mean WER on the Danish ASR leaderboard (RyeAI), where measured.
    pub mean_wer: Option<&'static str>,
    pub file_name: &'static str,
    pub source: Source,
    /// Trained to take the previous sentence as a prompt. For other models
    /// a prompt does harm: Edda v0.1 loops (FLEURS WER 7.8% → 87.7%).
    pub takes_context: bool,
}

pub fn catalog() -> Vec<CatalogModel> {
    vec![
        CatalogModel {
            id: "edda",
            name: "Edda v0.2",
            publisher: "Danish Foundation Models",
            description: "Whisper large-v3-turbo, 0.8B parameters. Fast enough for live dictation, and hears the sentence before, which keeps its punctuation.",
            license: "Apache 2.0",
            non_commercial: false,
            size: "q5_0 · ~550 MB",
            mean_wer: Some("8.8%"),
            file_name: "edda-v0.2-q5_0.bin",
            source: Source::Convert {
                repo: "danish-foundation-models/edda-v0.2",
                quant: "q5_0",
            },
            takes_context: true,
        },
        CatalogModel {
            id: "edda-v0.1",
            name: "Edda v0.1",
            publisher: "Danish Foundation Models",
            description: "The first Edda. Leaves out punctuation in about half its sentences; v0.2 replaces it.",
            license: "Apache 2.0",
            non_commercial: false,
            size: "q5_0 · ~550 MB",
            mean_wer: Some("9.2%"),
            file_name: "edda-v0.1-q5_0.bin",
            source: Source::Convert {
                repo: "danish-foundation-models/edda-v0.1",
                quant: "q5_0",
            },
            takes_context: false,
        },
        CatalogModel {
            id: "hviske-v3",
            name: "Hviske v3 conversation",
            publisher: "syv.ai",
            description: "Whisper large-v3 (Røst), 1.5B parameters. Trained on conversations; strong on meetings and interviews.",
            license: "OpenRAIL, non-commercial",
            non_commercial: true,
            size: "q5_0 · ~1.1 GB",
            mean_wer: None,
            file_name: "hviske-v3-conversation-q5_0.bin",
            source: Source::Convert {
                repo: "syvai/hviske-v3-conversation",
                quant: "q5_0",
            },
            takes_context: false,
        },
        CatalogModel {
            id: "roest-v3",
            name: "Røst v3 Whisper 1.5B",
            publisher: "CoRal project",
            description: "Whisper large-v3, 1.5B parameters. Broadly trained on dialects and read-aloud speech.",
            license: "OpenRAIL",
            non_commercial: false,
            size: "q8_0 · 1.7 GB",
            mean_wer: Some("13.7%"),
            file_name: "roest-v3-q8_0.bin",
            source: Source::Ggml {
                repo: "alfanova/roest-v3-whisper-ggml",
                file: "roest-v3-q8_0.bin",
            },
            takes_context: false,
        },
        CatalogModel {
            id: "hviske-v6",
            name: "Hviske v6",
            publisher: "syv.ai",
            description: "Whisper encoder, Qwen3 decoder, 1.2B parameters. Runs in a Python helper; slow on a CPU.",
            license: "CC BY-NC 4.0",
            non_commercial: true,
            size: "full precision · 2.8 GB",
            mean_wer: Some("10.2%"),
            file_name: "hviske-v6",
            source: Source::Snapshot {
                repo: "syvai/hviske-v6",
            },
            takes_context: false,
        },
    ]
}

/// Whether the model in this file (a catalogue file name or a path) takes
/// the previous sentence as context.
pub fn takes_context(model: &str) -> bool {
    let name = Path::new(model)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(model);
    catalog().iter().any(|m| m.file_name == name && m.takes_context)
}

pub fn path_of(model: &CatalogModel, paths: &Paths) -> PathBuf {
    paths.models().join(model.file_name)
}

pub fn is_installed(model: &CatalogModel, paths: &Paths) -> bool {
    let path = path_of(model, paths);
    match model.source {
        Source::Snapshot { .. } => path.join("model.safetensors").is_file(),
        _ => path.is_file(),
    }
}

/// Downloads a whole model repository into `dest` with Python's
/// `huggingface_hub` (resumable; progress comes back as lines).
pub fn fetch_snapshot(
    repo: &str,
    dest: &Path,
    python: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<(), String> {
    let code = "import sys\nfrom huggingface_hub import snapshot_download\n\
                print('Downloading', sys.argv[1], flush=True)\n\
                snapshot_download(sys.argv[1], local_dir=sys.argv[2])\nprint('done', flush=True)";
    let mut child = Command::new(python)
        .args(["-u", "-c", code, repo])
        .arg(dest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {python}: {e}"))?;
    // huggingface_hub reports progress on stderr.
    let stderr = child.stderr.take().expect("piped");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let mut tail = Vec::new();
        let mut buf = Vec::new();
        let mut r = std::io::BufReader::new(stderr);
        while r.read_until(b'\r', &mut buf).map(|n| n > 0).unwrap_or(false) {
            let line = String::from_utf8_lossy(&buf).trim().to_string();
            buf.clear();
            if !line.is_empty() {
                tail.push(line.clone());
                let _ = tx.send(line);
            }
        }
        tail.into_iter().rev().take(3).collect::<Vec<_>>()
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            return Err("cancelled".into());
        }
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(line) => progress(Progress::Line(line)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let tail = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(tail.into_iter().rev().collect::<Vec<_>>().join(" "));
    }
    Ok(())
}

/// GGML files in the models directory that are not in the catalog (and not the VAD).
pub fn custom_models(paths: &Paths) -> Vec<PathBuf> {
    let known: Vec<&str> = catalog().iter().map(|m| m.file_name).collect();
    let Ok(entries) = std::fs::read_dir(paths.models()) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "bin"))
        .filter(|p| {
            let n = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            !known.contains(&n.as_str()) && !n.contains("silero")
        })
        .collect();
    out.sort();
    out
}

pub fn hf_url(repo: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{file}")
}

/// The token saved by `hf auth login`, needed for gated repositories.
pub fn hf_token() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let t = std::fs::read_to_string(Path::new(&home).join(".cache/huggingface/token")).ok()?;
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Bytes { done: u64, total: Option<u64> },
    Line(String),
}

/// The Silero voice detector used for dictation and file import.
pub const VAD_REPO: &str = "ggml-org/whisper-vad";
pub const VAD_FILE: &str = "ggml-silero-v6.2.0.bin";

/// Downloads the voice detector if it is missing (about 1 MB). Without it
/// Fennec still works, with a cruder loudness detector.
pub fn ensure_vad(paths: &Paths, cancel: &AtomicBool, progress: impl FnMut(Progress)) -> Result<(), String> {
    let dest = paths.models().join(VAD_FILE);
    if dest.exists() {
        return Ok(());
    }
    download(&hf_url(VAD_REPO, VAD_FILE), &dest, cancel, progress)
}

/// Downloads the Danish punctuation model if it is missing (about 440 MB).
pub fn ensure_punctuation(
    paths: &Paths,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<(), String> {
    let dir = paths.models().join(crate::punctuation::DIR);
    for file in crate::punctuation::FILES {
        let dest = dir.join(file);
        if !dest.exists() {
            download(
                &hf_url(crate::punctuation::REPO, file),
                &dest,
                cancel,
                &mut progress,
            )?;
        }
    }
    Ok(())
}

/// Downloads `url` to `dest`, resuming a previous `.part` file. `cancel`
/// stops it, leaving the partial file for next time.
pub fn download(
    url: &str,
    dest: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = dest.with_extension("bin.part");
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    if let Some(t) = hf_token() {
        req = req.bearer_auth(t);
    }
    let mut resp = req.send().map_err(|e| format!("could not reach {url}: {e}"))?;
    let status = resp.status();
    if !(status.is_success()) {
        return Err(match status.as_u16() {
            401 | 403 => format!(
                "{url}: access denied ({status}); log in with `hf auth login` and accept the model's terms"
            ),
            _ => format!("{url}: {status}"),
        });
    }
    let resumed = status.as_u16() == 206;
    let total = resp.content_length().map(|l| if resumed { l + have } else { l });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(resumed)
        .write(true)
        .truncate(!resumed)
        .open(&part)
        .map_err(|e| format!("{}: {e}", part.display()))?;
    let mut done = if resumed { have } else { 0 };
    let mut buf = vec![0u8; 1 << 16];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let n = resp
            .read(&mut buf)
            .map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(Progress::Bytes { done, total });
    }
    file.flush().map_err(|e| e.to_string())?;
    if total.is_some_and(|t| done < t) {
        return Err(format!(
            "download ended early ({done} of {} bytes); try again to resume",
            total.unwrap_or(0)
        ));
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

const CONVERT_SCRIPT: &str = include_str!("../scripts/convert_model.py");

/// Converts a Hugging Face checkpoint with the bundled script. Needs Python
/// with torch, transformers and huggingface_hub, and a whisper.cpp checkout
/// with `whisper-quantize` built (see the script).
pub fn convert(
    repo: &str,
    name: &str,
    quant: &str,
    python: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let script = dir.path().join("convert_model.py");
    std::fs::write(&script, CONVERT_SCRIPT).map_err(|e| e.to_string())?;
    let mut child = Command::new(python)
        .arg(&script)
        .args([repo, name, quant])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {python}: {e}"))?;
    let stdout = child.stdout.take().expect("piped");
    let mut last_ok = None;
    for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            return Err("cancelled".into());
        }
        if let Some(path) = line.strip_prefix("OK ") {
            last_ok = path.split(" (").next().map(PathBuf::from);
        }
        progress(Progress::Line(line));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(3).collect();
        return Err(tail.into_iter().rev().collect::<Vec<_>>().join(" "));
    }
    last_ok.ok_or_else(|| "the converter finished without reporting a file".into())
}

/// Python used for conversion: `FENNEC_PYTHON`, else `python3`.
pub fn python() -> String {
    std::env::var("FENNEC_PYTHON").unwrap_or_else(|_| "python3".into())
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendInfo {
    pub backend: Backend,
    pub label: &'static str,
    pub available: bool,
    /// Device name when available, otherwise why not.
    pub detail: String,
}

/// What this build and machine can run. Unavailable options say why.
pub fn backends() -> Vec<BackendInfo> {
    let nvidia = Path::new("/proc/driver/nvidia/version").exists();
    let cuda_detail = match (cfg!(feature = "cuda"), nvidia) {
        (true, true) => "NVIDIA GPU".to_string(),
        (_, false) => "No NVIDIA GPU found".to_string(),
        (false, true) => "This build has no CUDA support (cargo feature `cuda`)".to_string(),
    };
    let vulkan_detail = if cfg!(feature = "vulkan") {
        "AMD, Intel or NVIDIA GPU".to_string()
    } else {
        "This build has no Vulkan support (cargo feature `vulkan`)".to_string()
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    vec![
        BackendInfo {
            backend: Backend::Auto,
            label: "Automatic",
            available: true,
            detail: "Best available, falls back to CPU".into(),
        },
        BackendInfo {
            backend: Backend::Cuda,
            label: "NVIDIA (CUDA)",
            available: cfg!(feature = "cuda") && nvidia,
            detail: cuda_detail,
        },
        BackendInfo {
            backend: Backend::Vulkan,
            label: "GPU (Vulkan)",
            available: cfg!(feature = "vulkan"),
            detail: vulkan_detail,
        },
        BackendInfo {
            backend: Backend::Cpu,
            label: "CPU",
            available: true,
            detail: format!("{threads} threads"),
        },
    ]
}

/// Whether to ask whisper.cpp for a GPU.
pub fn wants_gpu(b: Backend) -> bool {
    b != Backend::Cpu && (cfg!(feature = "vulkan") || cfg!(feature = "cuda"))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedResult {
    pub audio_secs: f64,
    pub elapsed_secs: f64,
}

impl SpeedResult {
    /// Processing time ÷ audio time; below 1 keeps up with speech.
    pub fn real_time_factor(&self) -> f64 {
        self.elapsed_secs / self.audio_secs
    }
}

const SPEED_CLIP: &[u8] = include_bytes!("../data/speedtest-da.wav");

/// Transcribes a bundled 7.5 s Danish clip (FLEURS, CC BY 4.0) and times it.
pub fn speed_test(engine: &mut dyn Transcriber) -> Result<SpeedResult, String> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(SPEED_CLIP)).map_err(|e| e.to_string())?;
    let pcm: Vec<f32> = reader
        .samples::<f32>()
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let t = Instant::now();
    engine
        .transcribe(&pcm, &TranscribeOptions::default())
        .map_err(|e| e.to_string())?;
    Ok(SpeedResult {
        audio_secs: pcm.len() as f64 / f64::from(SAMPLE_RATE),
        elapsed_secs: t.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_files_are_unique_and_edda_is_first() {
        let c = catalog();
        assert_eq!(c[0].id, "edda");
        let mut files: Vec<&str> = c.iter().map(|m| m.file_name).collect();
        files.dedup();
        assert_eq!(files.len(), c.len());
        assert_eq!(
            c[0].file_name,
            crate::config::Settings::default().model,
            "the default setting names Edda"
        );
    }

    #[test]
    fn only_models_trained_with_the_previous_sentence_take_context() {
        assert!(takes_context("edda-v0.2-q5_0.bin"));
        assert!(takes_context("/somewhere/models/edda-v0.2-q5_0.bin"));
        // A prompt makes Edda v0.1 loop (FLEURS WER 7.8% → 87.7%).
        assert!(!takes_context("edda-v0.1-q5_0.bin"));
        assert!(!takes_context("roest-v3-q8_0.bin"));
        assert!(!takes_context("min-model.bin"));
    }

    #[test]
    fn hviske_v6_counts_as_installed_only_with_its_weights() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let v6 = catalog().into_iter().find(|m| m.id == "hviske-v6").unwrap();
        assert!(matches!(v6.source, Source::Snapshot { .. }));
        let folder = path_of(&v6, &paths);
        std::fs::create_dir_all(&folder).unwrap();
        assert!(!is_installed(&v6, &paths), "a half-finished download");
        std::fs::write(folder.join("model.safetensors"), b"x").unwrap();
        assert!(is_installed(&v6, &paths));
    }

    #[test]
    fn an_installed_vad_is_not_downloaded_again() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(paths.models()).unwrap();
        std::fs::write(paths.models().join(VAD_FILE), b"x").unwrap();
        let mut calls = 0;
        ensure_vad(&paths, &AtomicBool::new(false), |_| calls += 1).unwrap();
        assert_eq!(calls, 0);
    }

    #[test]
    fn the_vad_lives_where_settings_look_for_it() {
        let paths = Paths::under(Path::new("/r"));
        assert_eq!(
            crate::config::Settings::default().vad_path(&paths),
            paths.models().join(VAD_FILE)
        );
    }

    #[test]
    fn custom_models_skip_catalog_files_and_the_vad() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(paths.models()).unwrap();
        for f in [
            "edda-v0.2-q5_0.bin",
            "ggml-silero-v6.2.0.bin",
            "min-model.bin",
            "notes.txt",
        ] {
            std::fs::write(paths.models().join(f), b"x").unwrap();
        }
        let names: Vec<String> = custom_models(&paths)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["min-model.bin"]);
        assert!(is_installed(&catalog()[0], &paths));
    }

    #[test]
    fn cpu_and_automatic_are_always_available() {
        let b = backends();
        assert!(b.iter().find(|x| x.backend == Backend::Cpu).unwrap().available);
        assert!(b.iter().find(|x| x.backend == Backend::Auto).unwrap().available);
        assert!(
            b.iter().all(|x| x.available || !x.detail.is_empty()),
            "unavailable options explain why"
        );
    }

    #[test]
    fn a_failed_download_names_the_url() {
        let dir = tempfile::tempdir().unwrap();
        let err = download(
            "http://127.0.0.1:9/none.bin",
            &dir.path().join("x.bin"),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(err.contains("127.0.0.1:9"), "{err}");
    }

    #[test]
    fn speed_test_reports_audio_length() {
        struct Instant0;
        impl Transcriber for Instant0 {
            fn transcribe(
                &mut self,
                _: &[f32],
                _: &TranscribeOptions,
            ) -> Result<Vec<crate::engine::Segment>, crate::engine::EngineError> {
                Ok(vec![])
            }
        }
        let r = speed_test(&mut Instant0).unwrap();
        assert!((7.0..8.0).contains(&r.audio_secs), "{r:?}");
        assert!(r.real_time_factor() < 1.0);
    }
}
