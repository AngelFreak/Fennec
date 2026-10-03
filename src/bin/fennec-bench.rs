//! Compares Whisper models on a Danish test set.
//!
//! ```text
//! fennec-bench --manifest set.tsv [--skip N] [--limit N] [--speed N] [--gpu]
//!              [--prompt TEXT] [--fast] [--pairs] [--context] [--vocab TERMS] [--dump FILE] model.bin...
//! ```
//!
//! `--pairs` joins the clips two by two with a short pause, like dictating
//! two sentences in one breath, so sentence ends inside a transcript are
//! scored. Punctuation is reported as F1 for commas and sentence ends.
//!
//! The manifest has one `wav_path<TAB>reference` per line. Accuracy (WER/CER)
//! runs all models at the same time, splitting the CPU between them: contention
//! slows them down but does not change their output. Speed is then measured
//! one model at a time on the first `--speed` clips (default 10), because
//! parallel runs would distort the timings.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use fennec::audio::read_wav_16k_mono;
use fennec::engine::{SAMPLE_RATE, TranscribeOptions, default_threads, load_engine};
use fennec::eval::{ErrorCount, Punctuation, char_errors, punctuation, word_errors};

struct Args {
    manifest: PathBuf,
    /// Clips skipped from the start, for a held-out set.
    skip: usize,
    limit: Option<usize>,
    speed_clips: usize,
    gpu: bool,
    prompt: Option<String>,
    fast: bool,
    pairs: bool,
    /// Prompts each clip with another clip's text, as live dictation would
    /// with the sentence before.
    context: bool,
    /// Corrects transcripts with this vocabulary, as the app does.
    vocab: Option<String>,
    /// Writes every `reference<TAB>hypothesis` here.
    dump: Option<PathBuf>,
    models: Vec<PathBuf>,
}

struct Clip {
    pcm: Vec<f32>,
    reference: String,
}

struct Accuracy {
    model: PathBuf,
    wer: ErrorCount,
    cer: ErrorCount,
    punct: Punctuation,
    worst: Vec<(f64, String, String)>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let args = parse_args()?;
    let mut clips = load_manifest(&args.manifest, args.skip, args.limit)?;
    if args.pairs {
        clips = pairs(clips);
    }
    let audio_secs: f64 = clips
        .iter()
        .map(|c| c.pcm.len() as f64 / SAMPLE_RATE as f64)
        .sum();
    eprintln!(
        "{} clips, {:.1} min of audio, {} models",
        clips.len(),
        audio_secs / 60.0,
        args.models.len()
    );

    let accuracy = if args.gpu {
        // Several whisper.cpp contexts on one Vulkan device at once crash
        // inside ggml, so GPU models take turns.
        args.models
            .iter()
            .map(|m| accuracy_of(m, &args, &clips, default_threads()))
            .collect::<Result<Vec<_>>>()?
    } else {
        run_accuracy_in_parallel(&args, &clips)?
    };
    let speed = run_speed_sequentially(&args, &clips)?;

    println!(
        "\n{:<34} {:>7} {:>7} {:>9} {:>9} {:>9} {:>9}",
        "model", "WER", "CER", "load s", "RTF", "comma F1", "end F1"
    );
    for (a, (load, rtf)) in accuracy.iter().zip(&speed) {
        let rtf = if rtf.is_finite() {
            format!("{rtf:.3}")
        } else {
            "–".to_string()
        };
        println!(
            "{:<34} {:>6.1}% {:>6.1}% {:>9.1} {:>9} {:>9.2} {:>9.2}",
            file_name(&a.model),
            a.wer.rate() * 100.0,
            a.cer.rate() * 100.0,
            load,
            rtf,
            a.punct.commas.f1(),
            a.punct.ends.f1()
        );
        let (c, e) = (a.punct.commas, a.punct.ends);
        println!(
            "    commas: {} right, {} missed, {} extra; sentence ends: {} right, {} missed, {} extra",
            c.right, c.missed, c.extra, e.right, e.missed, e.extra
        );
    }
    println!("\nRTF = processing time / audio time (lower is faster; below 1 keeps up with speech).");
    for a in &accuracy {
        println!("\nWorst clips for {}:", file_name(&a.model));
        for (wer, reference, hypothesis) in a.worst.iter().take(3) {
            println!(
                "  WER {:.0}%\n    ref: {reference}\n    hyp: {hypothesis}",
                wer * 100.0
            );
        }
    }
    Ok(())
}

fn run_accuracy_in_parallel(args: &Args, clips: &[Clip]) -> Result<Vec<Accuracy>> {
    let threads_each = (default_threads() / args.models.len()).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = args
            .models
            .iter()
            .map(|model| scope.spawn(move || accuracy_of(model, args, clips, threads_each)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("benchmark thread panicked"))
            .collect()
    })
}

fn accuracy_of(model: &Path, args: &Args, clips: &[Clip], threads: usize) -> Result<Accuracy> {
    let mut engine = load_engine(model, args.gpu)?;
    let opts = TranscribeOptions {
        threads,
        initial_prompt: args.prompt.clone(),
        fast: args.fast,
        ..Default::default()
    };
    let mut acc = Accuracy {
        model: model.to_path_buf(),
        wer: ErrorCount::default(),
        cer: ErrorCount::default(),
        punct: Punctuation::default(),
        worst: Vec::new(),
    };
    for (i, clip) in clips.iter().enumerate() {
        let mut opts = opts.clone();
        if args.context {
            opts.initial_prompt = Some(context_for(clips, i));
        }
        let mut hyp = join_text(&engine.transcribe(&clip.pcm, &opts)?);
        if let Some(v) = &args.vocab {
            hyp = fennec::vocabulary::Vocabulary::parse(v).correct(&hyp, &[]).0;
        }
        let w = word_errors(&clip.reference, &hyp);
        acc.wer.add(w);
        acc.cer.add(char_errors(&clip.reference, &hyp));
        acc.punct.add(punctuation(&clip.reference, &hyp));
        acc.worst.push((w.rate(), clip.reference.clone(), hyp));
        eprintln!("[{}] {}/{}", file_name(model), i + 1, clips.len());
    }
    if let Some(path) = &args.dump {
        let lines: String = acc.worst.iter().map(|(_, r, h)| format!("{r}\t{h}\n")).collect();
        std::fs::write(path, lines).with_context(|| format!("writing {}", path.display()))?;
    }
    acc.worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(acc)
}

/// Returns (load seconds, real-time factor) per model.
fn run_speed_sequentially(args: &Args, clips: &[Clip]) -> Result<Vec<(f64, f64)>> {
    let sample = &clips[..args.speed_clips.min(clips.len())];
    let audio: f64 = sample
        .iter()
        .map(|c| c.pcm.len() as f64 / SAMPLE_RATE as f64)
        .sum();
    let opts = TranscribeOptions::default();
    args.models
        .iter()
        .map(|model| {
            eprintln!("[{}] timing {} clips alone", file_name(model), sample.len());
            let t = Instant::now();
            let mut engine = load_engine(model, args.gpu)?;
            let load = t.elapsed().as_secs_f64();
            let t = Instant::now();
            for clip in sample {
                engine.transcribe(&clip.pcm, &opts)?;
            }
            Ok((load, t.elapsed().as_secs_f64() / audio))
        })
        .collect()
}

fn join_text(segments: &[fennec::engine::Segment]) -> String {
    segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The last 200 characters of a different sentence: neighbouring FLEURS
/// clips are often the same sentence read again, which would leak the answer.
fn context_for(clips: &[Clip], i: usize) -> String {
    let n = clips.len();
    let other = (1..n)
        .map(|k| &clips[(i + n / 3 + k) % n].reference)
        .find(|r| **r != clips[i].reference)
        .cloned()
        .unwrap_or_default();
    let chars: Vec<char> = other.chars().collect();
    chars[chars.len().saturating_sub(200)..].iter().collect()
}

/// Clip i joined with clip i + n/2 (neighbours are often the same
/// sentence read by another speaker), 0.4 s apart: two sentences in one
/// breath.
fn pairs(clips: Vec<Clip>) -> Vec<Clip> {
    let half = clips.len() / 2;
    (0..half)
        .map(|i| {
            let (a, b) = (&clips[i], &clips[i + half]);
            let mut pcm = a.pcm.clone();
            pcm.extend(std::iter::repeat_n(0.0, SAMPLE_RATE as usize * 2 / 5));
            pcm.extend_from_slice(&b.pcm);
            let first = a.reference.trim_end();
            let end = if first.ends_with(['.', '?', '!']) { "" } else { "." };
            Clip {
                pcm,
                reference: format!("{first}{end} {}", b.reference),
            }
        })
        .collect()
}

fn load_manifest(path: &Path, skip: usize, limit: Option<usize>) -> Result<Vec<Clip>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let mut clips = Vec::new();
    for (n, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .skip(skip)
    {
        let Some((wav, reference)) = line.split_once('\t') else {
            bail!("{}:{}: expected `wav_path<TAB>reference`", path.display(), n + 1);
        };
        let pcm = read_wav_16k_mono(&base.join(wav))?;
        clips.push(Clip {
            pcm,
            reference: reference.to_string(),
        });
        if limit.is_some_and(|l| clips.len() >= l) {
            break;
        }
    }
    if clips.is_empty() {
        bail!("{} has no clips", path.display());
    }
    Ok(clips)
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        manifest: PathBuf::new(),
        skip: 0,
        limit: None,
        speed_clips: 10,
        gpu: false,
        prompt: None,
        fast: false,
        pairs: false,
        context: false,
        vocab: None,
        dump: None,
        models: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--manifest" => args.manifest = it.next().context("--manifest needs a path")?.into(),
            "--skip" => args.skip = it.next().context("--skip needs a number")?.parse()?,
            "--limit" => args.limit = Some(it.next().context("--limit needs a number")?.parse()?),
            "--speed" => args.speed_clips = it.next().context("--speed needs a number")?.parse()?,
            "--gpu" => args.gpu = true,
            "--prompt" => args.prompt = Some(it.next().context("--prompt needs a text")?),
            "--fast" => args.fast = true,
            "--pairs" => args.pairs = true,
            "--context" => args.context = true,
            "--vocab" => args.vocab = Some(it.next().context("--vocab needs terms")?),
            "--dump" => args.dump = Some(it.next().context("--dump needs a path")?.into()),
            _ if a.starts_with("--") => bail!("unknown option {a}"),
            _ => args.models.push(a.into()),
        }
    }
    if args.manifest.as_os_str().is_empty() || args.models.is_empty() {
        bail!("usage: fennec-bench --manifest set.tsv [--limit N] [--speed N] [--gpu] model.bin...");
    }
    Ok(args)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}
