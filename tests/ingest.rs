//! File ingest through the real wiring: MP3 (44.1 kHz stereo) → symphonia →
//! resample → Silero VAD → whisper → paragraphs in the store → DOCX.
//! Needs `python scripts/fetch_models.py tiny vad` and ffmpeg.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use fennec::engine::{TranscribeOptions, WhisperEngine};
use fennec::export::{ExportOptions, Format, Report, write};
use fennec::ingest::{IngestError, IngestEvent, IngestOptions, Recognizer, ingest_file};
use fennec::store::{NewDocument, Store};
use fennec::template::Template;
use fennec::vad::SileroVad;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn model(name: &str) -> PathBuf {
    let p = fixture(&format!("models/{name}"));
    assert!(
        p.exists(),
        "missing {}; run `python scripts/fetch_models.py tiny vad`",
        p.display()
    );
    p
}

/// The Danish clip, 2.5 s of silence, the clip again — as 44.1 kHz stereo MP3.
fn two_utterances_mp3(dir: &Path) -> PathBuf {
    let out = dir.join("to_ytringer.mp3");
    let clip = fixture("da_fleurs_0.wav");
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y", "-i"])
        .arg(&clip)
        .args(["-f", "lavfi", "-t", "2.5", "-i", "anullsrc=r=16000:cl=mono", "-i"])
        .arg(&clip)
        .args([
            "-filter_complex",
            "[0:a][1:a][2:a]concat=n=3:v=0:a=1",
            "-ar",
            "44100",
            "-ac",
            "2",
        ])
        .arg(&out)
        .status()
        .expect("ffmpeg is installed");
    assert!(status.success());
    out
}

#[test]
fn mp3_becomes_timestamped_paragraphs_and_exports_to_docx() {
    let dir = tempfile::tempdir().unwrap();
    let mp3 = two_utterances_mp3(dir.path());
    let store = Store::open(&dir.path().join("fennec.db")).unwrap();
    let doc = store.create_document(&NewDocument::file("Interview")).unwrap();
    let mut engine = WhisperEngine::load(&model("ggml-tiny.bin"), false).unwrap();
    let mut vad = SileroVad::load(&model("ggml-silero-v6.2.0.bin"), 2).unwrap();
    let opts = IngestOptions {
        transcribe: TranscribeOptions {
            threads: 4,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut events = Vec::new();
    let saved = ingest_file(
        &mp3,
        doc,
        &store,
        Recognizer {
            engine: &mut engine,
            vad: &mut vad,
        },
        &opts,
        &AtomicBool::new(false),
        |e| events.push(e),
    )
    .unwrap();

    let paragraphs = store.paragraphs(doc).unwrap();
    assert_eq!(saved, paragraphs.len());
    assert!(
        paragraphs.len() >= 2,
        "the 2.5 s pause should split paragraphs: {paragraphs:#?}"
    );
    let first = &paragraphs[0];
    let last = paragraphs.last().unwrap();
    assert!(first.start_ms.unwrap() < 1_500, "{first:?}");
    // The second clip starts after 7.5 s of speech plus the 2.5 s pause.
    assert!(last.start_ms.unwrap() > 9_000, "{last:?}");
    assert!(last.end_ms.unwrap() <= 18_500, "{last:?}");

    assert!(
        matches!(events.first(), Some(IngestEvent::Decoded { duration_ms }) if (17_000..18_000).contains(duration_ms))
    );
    assert!(matches!(events.last(), Some(IngestEvent::Finished { paragraphs }) if *paragraphs == saved));
    let progress: Vec<i64> = events
        .iter()
        .filter_map(|e| {
            if let IngestEvent::Progress { done_ms, .. } = e {
                Some(*done_ms)
            } else {
                None
            }
        })
        .collect();
    assert!(
        progress.windows(2).all(|w| w[0] <= w[1]),
        "progress goes forward: {progress:?}"
    );
    assert_eq!(
        store.document(doc).unwrap().audio_path.as_deref(),
        Some(mp3.as_path())
    );

    let report = Report::for_document(&store, doc, &Template::blank(), ExportOptions::default()).unwrap();
    let docx = dir.path().join("interview.docx");
    write(&report, Format::Docx, &docx).unwrap();
    let mut xml = String::new();
    zip::ZipArchive::new(std::fs::File::open(&docx).unwrap())
        .unwrap()
        .by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    let probe: String = first
        .text
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        xml.contains(&probe),
        "{probe:?} from the transcript is in the DOCX"
    );
}

#[test]
fn cancelling_stops_before_transcribing_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let doc = store.create_document(&NewDocument::file("x")).unwrap();
    let mut engine = WhisperEngine::load(&model("ggml-tiny.bin"), false).unwrap();
    let mut vad = SileroVad::load(&model("ggml-silero-v6.2.0.bin"), 2).unwrap();
    let mp3 = two_utterances_mp3(dir.path());
    let result = ingest_file(
        &mp3,
        doc,
        &store,
        Recognizer {
            engine: &mut engine,
            vad: &mut vad,
        },
        &IngestOptions::default(),
        &AtomicBool::new(true),
        |_| {},
    );
    assert!(matches!(result, Err(IngestError::Cancelled)));
    assert!(store.paragraphs(doc).unwrap().is_empty());
}

#[test]
fn an_unreadable_file_fails_with_a_decode_error_and_leaves_the_document_empty() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("ødelagt.mp3");
    std::fs::write(&bad, b"not audio at all").unwrap();
    let store = Store::open_in_memory().unwrap();
    let doc = store.create_document(&NewDocument::file("x")).unwrap();
    let mut engine = WhisperEngine::load(&model("ggml-tiny.bin"), false).unwrap();
    let mut vad = SileroVad::load(&model("ggml-silero-v6.2.0.bin"), 2).unwrap();
    let err = ingest_file(
        &bad,
        doc,
        &store,
        Recognizer {
            engine: &mut engine,
            vad: &mut vad,
        },
        &IngestOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, IngestError::Decode(_)), "{err}");
    assert!(err.to_string().contains("ødelagt.mp3"), "{err}");
    assert!(store.paragraphs(doc).unwrap().is_empty());
}
