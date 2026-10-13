//! Builds the real window against a temporary home with a scripted engine
//! and synthetic audio, then drives it like a user. GTK must run on the main
//! thread, so this is one binary with its own `main` (harness = false).
//! Needs a display; the desktop session provides one.

#![allow(clippy::single_range_in_vec_init)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use fennec::audio::capture::{AudioSource, PcmSource};
use fennec::config::{Paths, Settings};
use fennec::engine::{EngineError, Segment, TranscribeOptions, Transcriber};
use fennec::store::{Paragraph, Store};
use fennec::ui::{self, Deps};
use fennec::utterance::{EnergyVad, FrameVad};
use fennec::vad::{SpeechDetector, WholeAudio};
use gtk::glib;
use gtk::prelude::*;
use support::{MockLlm, Recorded, Reply, Wire};

static mut FAILURES: usize = 0;

/// Every prompt the scripted engine was given.
static PROMPTS: std::sync::Mutex<Vec<Option<String>>> = std::sync::Mutex::new(Vec::new());

fn check(name: &str, ok: bool) {
    println!("{} {name}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        unsafe { FAILURES += 1 };
    }
}

fn pump_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let ctx = glib::MainContext::default();
    while !done() {
        if start.elapsed() > timeout {
            return false;
        }
        while ctx.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// Answers utterances with scripted lines.
/// The same utterance (previews, a quick pass, the accurate one) gets the
/// same line.
struct Scripted {
    lines: Vec<&'static str>,
    last: Option<(Vec<f32>, &'static str)>,
}

#[allow(non_snake_case)]
fn Scripted(lines: Vec<&'static str>) -> Scripted {
    Scripted { lines, last: None }
}

fn same_utterance(a: &[f32], b: &[f32]) -> bool {
    let n = a.len().min(b.len());
    n > 0 && a[..n] == b[..n]
}

impl Transcriber for Scripted {
    fn transcribe(&mut self, pcm: &[f32], o: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        // A real engine takes a while; the interface must not wait for it.
        std::thread::sleep(Duration::from_millis(200));
        PROMPTS.lock().unwrap().push(o.initial_prompt.clone());
        let text = match &self.last {
            // A preview's audio is the start of its utterance's.
            Some((last, text)) if same_utterance(last, pcm) => text,
            _ => {
                let text = if self.lines.is_empty() {
                    "Mere tekst."
                } else {
                    self.lines.remove(0)
                };
                self.last = Some((pcm.to_vec(), text));
                text
            }
        };
        Ok(vec![Segment {
            start_ms: 0,
            end_ms: 1,
            text: text.into(),
            low_confidence: vec![],
        }])
    }
}

fn tones(n: usize) -> Vec<f32> {
    let mut v = Vec::new();
    for _ in 0..n {
        v.extend((0..16_000).map(|i| (i as f32 * 0.07).sin() * 0.3));
        // Pauses long enough that these short sentences stay apart.
        v.extend(std::iter::repeat_n(0.0, 16_000 * 16 / 10));
    }
    v
}

/// Stands in for the punctuation model: a full stop after the last word.
struct EndsSentences;

impl fennec::punctuation::Punctuate for EndsSentences {
    fn labels(
        &self,
        words: &[String],
    ) -> Result<Vec<fennec::punctuation::Label>, fennec::punctuation::PunctuationError> {
        let mut labels = vec![fennec::punctuation::Label::NONE; words.len()];
        if let Some(last) = labels.last_mut() {
            last.mark = Some('.');
        }
        Ok(labels)
    }
}

fn deps(root: &std::path::Path, lines: Vec<&'static str>, engine_ok: bool) -> Deps {
    // Stand-ins for the punctuation model's files; EndsSentences does the work.
    let punct = Paths::under(root).models().join(fennec::punctuation::DIR);
    std::fs::create_dir_all(&punct).unwrap();
    for f in fennec::punctuation::FILES {
        std::fs::write(punct.join(f), b"stand-in").unwrap();
    }
    let models = Paths::under(root).models();
    std::fs::write(models.join(&Settings::default().model), b"stand-in").unwrap();
    std::fs::write(models.join(fennec::models::VAD_FILE), b"stand-in").unwrap();
    let n_tones = lines.len().max(3);
    let lines = Arc::new(lines);
    Deps {
        paths: Paths::under(root),
        settings: std::rc::Rc::new(std::cell::RefCell::new(Settings {
            keep_dictation_audio: false,
            show_preview: false,
            ..Default::default()
        })),
        engine: Arc::new(move |_, _| {
            if engine_ok {
                Ok(Box::new(Scripted((*lines).clone())) as Box<dyn Transcriber>)
            } else {
                Err("model file not found: /x/edda.bin".into())
            }
        }),
        audio: Arc::new(move |_| Ok(Box::new(PcmSource::new(tones(n_tones))) as Box<dyn AudioSource>)),
        vad: Arc::new(|_, _| Ok(Box::new(EnergyVad::default()) as Box<dyn FrameVad>)),
        file_vad: Arc::new(|_, _| Box::new(WholeAudio) as Box<dyn SpeechDetector>),
        punctuator: Arc::new(|_| Ok(Arc::new(EndsSentences) as Arc<dyn fennec::punctuation::Punctuate>)),
        setup: Arc::new(|p, s, progress| {
            // Stands in for the downloads: a few placeholder files.
            progress(fennec::models::Progress::Bytes {
                done: 1,
                total: Some(2),
            });
            std::fs::create_dir_all(p.models().join(fennec::punctuation::DIR)).unwrap();
            std::fs::write(s.model_path(p), b"stand-in").unwrap();
            std::fs::write(p.models().join(fennec::models::VAD_FILE), b"stand-in").unwrap();
            for f in fennec::punctuation::FILES {
                std::fs::write(p.models().join(fennec::punctuation::DIR).join(f), b"stand-in").unwrap();
            }
            Ok(())
        }),
        secrets: Arc::new(fennec::ai::MemorySecrets::default()),
        confirm_cloud: std::rc::Rc::new(|_, _, answer| answer(true)),
        dictation_live: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        phone_local_only: true,
    }
}

/// Renders `window` to a PNG when FENNEC_SCREENSHOT_DIR is set (visual review).
fn screenshot(window: &impl IsA<gtk::Widget>, name: &str) {
    let Some(dir) = std::env::var_os("FENNEC_SCREENSHOT_DIR") else {
        return;
    };
    let widget = window.as_ref();
    pump_until(Duration::from_millis(400), || false);
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let (w, h) = (widget.width() as f64, widget.height() as f64);
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, w, h);
    let Some(node) = snapshot.to_node() else {
        return println!("     (nothing to screenshot)");
    };
    let Some(renderer) = widget.native().and_then(|n| n.renderer()) else {
        return;
    };
    let texture = renderer.render_texture(&node, None);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    texture.save_to_png(&path).unwrap();
    println!("     screenshot {}", path.display());
}

fn main() {
    let tmp = tempfile::tempdir().unwrap();
    adw::init().expect("a display is available");
    ui::load_css();
    check("stylesheets parse", ui::css_errors().is_empty());
    for e in ui::css_errors() {
        println!("     {e}");
    }
    let theme = gtk::IconTheme::for_display(&gtk::gdk::Display::default().unwrap());
    check(
        "the mockup's icons ship with the app, whatever the icon theme",
        [
            "mic",
            "upload",
            "templates",
            "settings",
            "download",
            "cpu",
            "back",
        ]
        .iter()
        .all(|n| theme.has_icon(&format!("fennec-{n}-symbolic"))),
    );

    // --- first launch: a new document with the default template
    let root = tmp.path().join("a");
    let w = ui::build_window(deps(
        &root,
        vec![
            "Første sætning",
            "Punktum.",
            "Nyt afsnit.",
            "anden sætning",
            "Stopoptagelse.",
        ],
        true,
    ));
    check("opens on the Dictate screen", w.visible_page() == "dictate");
    check(
        "the speech model loads at launch, so the first recording starts at once",
        pump_until(Duration::from_secs(5), || w.dictation.model_loaded()),
    );
    let store = Store::open(&root.join("data/fennec.db")).unwrap();
    let docs = store.documents(&Default::default()).unwrap();
    check("first launch creates one document", docs.len() == 1);
    check(
        "default templates are installed",
        root.join("config/templates/notat.toml").exists(),
    );
    check(
        "Notat fields are shown",
        w.dictation.inspector.entry("sagsnr").is_some(),
    );
    check(
        "sidebar lists All documents and Unsorted",
        w.sidebar.project_names() == ["All documents", "Unsorted"],
    );

    // --- dictation through the real live pipeline
    w.dictation.start_recording();
    let mut placeholder_seen = false;
    let stopped = pump_until(Duration::from_secs(15), || {
        placeholder_seen |= w.dictation.editor.preview_text().as_deref() == Some("…");
        !w.dictation.is_recording() && w.dictation.dock.state_text() == "Ready"
    });
    check(
        "speech shows a placeholder at once, before any text is ready",
        placeholder_seen,
    );
    check(
        "no placeholder is left behind",
        w.dictation.editor.preview_text().is_none(),
    );
    check("saying «stop optagelse» stops the dictation", stopped);
    check(
        "the punctuation model ends the last sentence when dictation stops",
        pump_until(Duration::from_secs(5), || {
            w.dictation
                .editor
                .paragraphs()
                .last()
                .is_some_and(|p| p.text.ends_with('.'))
        }),
    );
    let texts: Vec<String> = w
        .dictation
        .editor
        .paragraphs()
        .into_iter()
        .map(|p| p.text)
        .collect();
    check(
        "spoken commands punctuate, split and capitalise the paragraphs",
        texts == ["Første sætning.", "Anden sætning."],
    );
    if texts != ["Første sætning.", "Anden sætning."] {
        println!(
            "     editor: {texts:?}; status: {}",
            w.dictation.dock.status_text()
        );
    }
    let doc = w.dictation.document().unwrap();
    let saved: Vec<String> = store
        .paragraphs(doc)
        .unwrap()
        .into_iter()
        .map(|p| p.text)
        .collect();
    check("dictated text is saved to the database", saved == texts);
    let prompts = PROMPTS.lock().unwrap().clone();
    check(
        "the model hears the text as it reads in the editor, with its marks",
        prompts.iter().flatten().any(|p| p.contains("Første sætning.")),
    );
    check(
        "Edda v0.2 hears the sentence before; spoken commands are not part of it",
        prompts.iter().flatten().any(|p| p.contains("Første sætning"))
            && !prompts
                .iter()
                .flatten()
                .any(|p| p.contains("Punktum") || p.contains("optagelse")),
    );
    w.window.present();
    screenshot(&w.window, "dictate");
    pump_until(Duration::from_millis(300), || w.sidebar.root.width() > 0);
    check(
        "every screen fits the mockup's 1280px window",
        w.window.measure(gtk::Orientation::Horizontal, -1).0 <= 1280,
    );
    check(
        "Dictate shows projects and tags in the sidebar",
        w.sidebar.context() == "projects",
    );
    check(
        "the sidebar keeps the mockup's 232px width",
        w.sidebar
            .root
            .compute_bounds(&w.window)
            .is_some_and(|b| b.width() == 232.0),
    );
    let first = &store.paragraphs(doc).unwrap()[0];
    check(
        "paragraphs keep their timestamps",
        first.start_ms.is_some() && first.end_ms.unwrap_or(0) > 500,
    );

    // --- typing keeps timestamps and is autosaved
    let buffer = w.dictation.editor.buffer().clone();
    let mut end = buffer.iter_at_line(0).unwrap();
    end.forward_to_line_end();
    buffer.insert(&mut end, " Tilføjet.");
    let autosaved = pump_until(Duration::from_secs(3), || {
        store
            .paragraphs(doc)
            .unwrap()
            .first()
            .is_some_and(|p| p.text.ends_with("Tilføjet."))
    });
    check("typing is autosaved after a pause", autosaved);
    check(
        "a typed edit keeps the paragraph time",
        store.paragraphs(doc).unwrap()[0].start_ms == first.start_ms,
    );

    // --- template fields are stored
    w.dictation
        .inspector
        .entry("sagsnr")
        .unwrap()
        .set_text("2026-0412");
    w.dictation.save_now();
    check(
        "field values are saved",
        store
            .document(doc)
            .unwrap()
            .fields
            .get("sagsnr")
            .map(String::as_str)
            == Some("2026-0412"),
    );

    // --- editor behaviour with unsure words and commands
    let e = &w.dictation.editor;
    let sentence = "Fugten kommer fra en utæt nedløbsbrønd.";
    let at = sentence.find("nedløbsbrønd").unwrap();
    e.load(&[Paragraph {
        start_ms: Some(0),
        end_ms: Some(2_000),
        low_confidence: vec![at..at + "nedløbsbrønd".len()],
        ..Paragraph::new(sentence)
    }]);
    w.dictation.save_now();
    check(
        "unsure words round-trip through the editor",
        e.unsure_words() == ["nedløbsbrønd"],
    );
    check(
        "the review panel counts unsure words",
        ui::texts_in(&w.dictation.inspector.root)
            .iter()
            .any(|t| t == "1 low-confidence word"),
    );
    e.insert_final("Det regner. Det sner.", 2_100, 3_000, &[]);
    e.apply(fennec::commands::Command::DeleteLastSentence);
    let last = e.paragraphs().pop().unwrap();
    check(
        "delete last sentence removes only that sentence",
        last.text.ends_with("Det regner."),
    );
    check(
        "joined text keeps the earliest start and latest end",
        last.start_ms == Some(0) && last.end_ms == Some(3_000),
    );
    e.apply(fennec::commands::Command::NewParagraph);
    e.insert_final("Nyt emne.", 3_200, 4_000, &[]);
    check(
        "new paragraph command starts a paragraph",
        e.paragraphs().len() == 2,
    );
    e.set_preview(Some("foreløbig"));
    check(
        "previews are not part of the saved text",
        !e.plain_text().contains("foreløbig"),
    );
    e.set_preview(None);

    // --- punctuation added in place: unsure words and previews survive
    let heard = "det regner i Aarhus i dag og vi bliver hjemme";
    let at = heard.find("Aarhus").unwrap();
    e.load(&[Paragraph {
        start_ms: Some(0),
        end_ms: Some(3_000),
        low_confidence: vec![at..at + "Aarhus".len()],
        ..Paragraph::new(heard)
    }]);
    e.set_preview(Some("foreløbig"));
    let fixed = "Det regner i Aarhus i dag, og vi bliver hjemme.";
    check(
        "punctuation goes into the paragraph in place",
        e.punctuate_paragraph(0, heard, fixed),
    );
    let p = e.paragraphs().remove(0);
    check(
        "the text, its unsure word, its times and the preview all survive",
        p.text == fixed
            && e.unsure_words() == ["Aarhus"]
            && p.start_ms == Some(0)
            && e.preview_text().as_deref() == Some("foreløbig"),
    );
    if p.text != fixed {
        println!("     got: {:?}", p.text);
    }
    check(
        "a paragraph edited meanwhile is left alone",
        !e.punctuate_paragraph(0, heard, fixed),
    );
    e.set_preview(None);

    // --- learning from corrections: the second identical fix is offered
    let before_learning = e.paragraphs();
    let fix_by_hand = |e: &fennec::ui::editor::Editor| {
        let b = e.buffer();
        let text = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
        let at = text.find("tonelighter").unwrap();
        let from = text[..at].chars().count() as i32;
        let mut s = b.iter_at_offset(from);
        let mut t = b.iter_at_offset(from + "tonelighter".chars().count() as i32);
        b.delete(&mut s, &mut t);
        let mut s = b.iter_at_offset(from);
        b.insert(&mut s, "toneleje");
    };
    let heard = "vi taler i et tonelighter ikke alt for højt";
    e.load(&[Paragraph::new(heard)]);
    fix_by_hand(e);
    w.dictation.save_now();
    check(
        "one correction is not offered yet",
        w.dictation.learn_offer().is_none(),
    );
    e.load(&[Paragraph::new(heard)]);
    // A machine change in between must not hide the user's fix.
    fix_by_hand(e);
    e.insert_final("Mere tekst.", 9_000, 9_500, &[]);
    w.dictation.save_now();
    check(
        "the second identical correction is offered as vocabulary",
        w.dictation.learn_offer() == Some(("tonelighter".into(), "toneleje".into())),
    );
    w.dictation.accept_learn();
    let saved = Settings::load(&root.join("config/settings.toml")).unwrap();
    check(
        "accepting adds a replacement that dictation then applies",
        w.dictation.learn_offer().is_none()
            && fennec::vocabulary::Vocabulary::parse(&saved.vocabulary)
                .correct("et tonelighter", &[])
                .0
                == "et toneleje",
    );
    // An accepted AI change is not the user's correction.
    e.load(&[Paragraph::new("han siger øh at det er fint")]);
    w.dictation.save_now();
    for _ in 0..2 {
        e.load(&[Paragraph::new("han siger øh at det er fint")]);
        e.replace_paragraph(0, "han siger øh at det er fint", "han fortæller at det er fint");
        w.dictation.save_now();
    }
    check(
        "AI clean-up is never learned as a correction",
        w.dictation.learn_offer().is_none(),
    );
    e.load(&before_learning);
    w.dictation.save_now();

    // --- file import through the real ingest thread and the shared engine
    let wav = tmp.path().join("interview.wav");
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wr = hound::WavWriter::create(&wav, spec).unwrap();
        for s in tones(2) {
            wr.write_sample((s * 32767.0) as i16).unwrap();
        }
        wr.finalize().unwrap();
    }
    w.sidebar.go(fennec::ui::Nav::Files);
    check("Files opens the import screen", w.visible_page() == "files");
    w.files.add_paths(std::slice::from_ref(&wav));
    let done = pump_until(Duration::from_secs(15), || {
        w.files.statuses().iter().all(|(_, st)| st == "Done")
    });
    check("an imported file is transcribed", done);
    if !done {
        println!("     statuses: {:?}", w.files.statuses());
    }
    let shown = w.files.shown_texts();
    check(
        "its transcript is shown",
        !shown.is_empty() && shown.iter().all(|t| !t.trim().is_empty()),
    );
    let file_docs = store
        .documents(&Default::default())
        .unwrap()
        .into_iter()
        .filter(|d| d.title == "interview")
        .count();
    check("the file became its own document", file_docs == 1);
    let loaded = pump_until(Duration::from_secs(10), || w.files.playback().is_some());
    check("the player loads the file's audio", loaded);
    w.files.seek_timeline(0.5);
    check(
        "clicking the timeline seeks",
        w.files
            .playback()
            .is_some_and(|(pos, total)| total > 0 && (pos - total / 2).abs() <= 50),
    );
    check("the speed button cycles", w.files.cycle_speed() == "1.25×");
    check("and wraps around", {
        for _ in 0..2 {
            w.files.cycle_speed();
        }
        w.files.cycle_speed() == "1.0×"
    });
    screenshot(&w.window, "files");

    // --- export: blocked while required fields are empty, then writes a DOCX
    let export_dir = tmp.path().join("exports");
    w.export.set_folder(export_dir.clone());
    w.dictation.inspector.entry("sagsnr").unwrap().set_text("");
    w.export_current();
    check("Export opens its screen", w.visible_page() == "export");
    check(
        "missing required fields are named",
        w.export.warning_text().is_some_and(|t| t.contains("Sagsnr.")),
    );
    w.export.set_format(fennec::export::Format::Pdf);
    w.export.refresh();
    check("the preview renders a page", w.export.preview_is_page());
    check(
        "the preview caption counts the pages",
        w.export.caption_text() == "Preview · A4 · 1 page",
    );
    screenshot(&w.window, "export");
    w.export.set_format(fennec::export::Format::Txt);
    w.export.refresh();
    check(
        "TXT previews as plain text",
        !w.export.preview_is_page() && w.export.caption_text() == "Preview · plain text",
    );
    screenshot(&w.window, "export-txt");
    w.sidebar.go(fennec::ui::Nav::Dictate);
    w.dictation
        .inspector
        .entry("sagsnr")
        .unwrap()
        .set_text("2026-0412");
    w.dictation
        .inspector
        .entry("udarbejdet_af")
        .unwrap()
        .set_text("Ane");
    w.export_current();
    w.export.set_format(fennec::export::Format::Docx);
    check(
        "filling the fields clears the warning",
        w.export.warning_text().is_none(),
    );
    let written = w.export.export_now();
    check(
        "export writes the DOCX named after case number and title",
        written.as_ref().is_some_and(|p| {
            p.exists() && p.file_name().unwrap().to_string_lossy().starts_with("2026-0412_")
        }),
    );

    // --- projects and tags group documents
    let harbour = store.create_project("Operation Harbour", "#1D4ED8").unwrap();
    w.sidebar.refresh();
    w.open_in_editor(doc);
    w.dictation.move_to_project(Some(harbour));
    w.dictation.change_tags(|t| t.push("vendor".into()));
    check(
        "the document moved to the project",
        store.document(doc).unwrap().project_id == Some(harbour),
    );
    check(
        "the tag was added",
        store.document(doc).unwrap().tags == ["vendor"],
    );
    check(
        "the breadcrumb shows the project",
        w.dictation.project_name() == "Operation Harbour",
    );
    w.sidebar
        .go(fennec::ui::Nav::Project(fennec::store::ProjectFilter::Project(
            harbour,
        )));
    check("a project opens the Project view", w.visible_page() == "project");
    let title = store.document(doc).unwrap().title;
    check(
        "the project lists its documents",
        w.project.shown_titles() == [title.clone()],
    );
    w.project.set_search("regner");
    check(
        "search narrows the list",
        w.project.shown_titles() == [title.clone()],
    );
    w.project.set_search("findes ikke");
    check(
        "search with no match shows nothing",
        w.project.shown_titles().is_empty(),
    );
    w.project.set_search("");
    w.project.set_tag_filter(Some("vendor"));
    check(
        "the tag filter keeps tagged documents",
        w.project.shown_titles().len() == 1,
    );
    w.project.set_local_only(true);
    check(
        "local only is saved",
        store
            .projects()
            .unwrap()
            .iter()
            .any(|p| p.id == harbour && p.local_only),
    );
    check(
        "the Project header reads Projects / name",
        w.header_crumbs() == (Some("Projects".to_string()), "Operation Harbour".to_string()),
    );
    screenshot(&w.window, "project");
    w.project.press_new_dictation();
    check(
        "New dictation starts a document in the project",
        w.visible_page() == "dictate" && w.dictation.project_name() == "Operation Harbour",
    );
    w.sidebar
        .go(fennec::ui::Nav::Project(fennec::store::ProjectFilter::Project(
            harbour,
        )));
    w.project.set_table_of_contents(false);
    w.project.press_export();
    check(
        "project export opens Export for its documents",
        w.visible_page() == "export",
    );
    check(
        "the project's Table of contents choice carries over to Export",
        !w.export.table_of_contents(),
    );
    {
        // Many tags wrap inside the sidebar instead of widening it.
        let other = store
            .create_document(&fennec::store::NewDocument::dictation("Mange mærker"))
            .unwrap();
        let tags: Vec<String> = [
            "inspection",
            "interview",
            "meeting",
            "moisture",
            "planning",
            "budget",
        ]
        .iter()
        .map(|t| t.to_string())
        .collect();
        store.set_tags(other, &tags).unwrap();
        w.sidebar.refresh();
        // Dictate leaves room to spare, which a greedy sidebar would take.
        w.sidebar.go(fennec::ui::Nav::Dictate);
        pump_until(Duration::from_millis(300), || false);
        check(
            "many tags wrap; the sidebar stays 232px",
            w.sidebar
                .root
                .compute_bounds(&w.window)
                .is_some_and(|b| b.width() == 232.0),
        );
        store.delete_document(other).unwrap();
        w.sidebar.refresh();
    }
    w.sidebar.go(fennec::ui::Nav::Tag("vendor".into()));
    check(
        "a tag shows documents across projects",
        w.project.shown_titles() == [title],
    );

    // --- templates: create, edit and save; invalid templates explain why
    w.sidebar.go(fennec::ui::Nav::Templates);
    check(
        "Templates puts its list in the sidebar, as in the mockup",
        w.sidebar.context() == "templates" && w.templates.list_panel.is_ancestor(&w.sidebar.root),
    );
    check(
        "Templates lists the defaults",
        w.templates.names() == ["Notat", "Mødereferat", "Afhøringsrapport"],
    );
    w.templates.press_new();
    w.templates.set_name("Besigtigelse");
    w.templates.add_field("Adresse", true);
    let saved_tpl = w.templates.save();
    check(
        "a new template is saved as TOML",
        saved_tpl.as_ref().is_ok_and(|p| p.exists()),
    );
    check(
        "it appears in the list",
        w.templates.names().contains(&"Besigtigelse".to_string()),
    );
    check("the template preview renders", w.templates.has_preview());
    w.templates.add_field("Dato", false);
    w.templates.move_field(1, false);
    check(
        "a field can be moved up the table",
        w.templates.field_labels() == ["Dato", "Adresse"],
    );
    screenshot(&w.window, "templates");
    w.templates.add_field("Adresse", false);
    check(
        "a duplicate field is refused with a reason",
        w.templates.save().is_err() && w.templates.error_text().contains("twice"),
    );
    w.templates.reload(Some("notat"));
    check(
        "opening a template shows its fields",
        w.templates.field_labels().len() == 4,
    );
    screenshot(&w.window, "templates-notat");

    // --- settings: models, compute, dictation; changes are saved
    w.sidebar.go(fennec::ui::Nav::Settings);
    check("Settings opens", w.visible_page() == "settings");
    check(
        "the catalog lists Edda first",
        w.settings.model_names().first().map(String::as_str) == Some("Edda v0.2"),
    );
    let custom = tmp.path().join("min-model.bin");
    std::fs::write(&custom, b"not a real model").unwrap();
    w.settings.add_custom(&custom);
    check(
        "a custom model is copied and selected",
        w.settings.settings().model == "min-model.bin",
    );
    check(
        "it is listed",
        w.settings.model_names().contains(&"min-model.bin".to_string()),
    );
    let on_disk = Settings::load(&root.join("config/settings.toml")).unwrap();
    check(
        "the choice is saved to settings.toml",
        on_disk.model == "min-model.bin",
    );
    w.settings.use_model("edda-v0.1-q5_0.bin");
    w.settings.run_speed_test();
    let measured = pump_until(Duration::from_secs(10), || {
        w.settings.speed_text().contains("real time")
    });
    check("the speed test reports a real-time factor", measured);
    check(
        "Settings lists its sections in the sidebar",
        w.sidebar.context() == "settings",
    );
    check(
        "each section shows its state under its name",
        w.settings
            .nav_summaries()
            .iter()
            .any(|(id, line)| id == "model" && line.starts_with("Edda v0.1")),
    );
    check(
        "the header names the open section",
        w.header_crumbs() == (Some("Settings".to_string()), "Speech model".to_string()),
    );
    w.settings.add_vocabulary("Leverandør A/S");
    check(
        "a vocabulary word is added as a chip and saved",
        w.settings
            .vocabulary_entries()
            .contains(&"Leverandør A/S".to_string())
            && Settings::load(&root.join("config/settings.toml"))
                .unwrap()
                .vocabulary
                .contains("Leverandør A/S"),
    );
    w.settings.remove_vocabulary("Leverandør A/S");
    check(
        "and removed again",
        !w.settings
            .vocabulary_entries()
            .contains(&"Leverandør A/S".to_string()),
    );
    let before = w.settings.vocabulary_entries().len();
    w.settings.add_vocabulary("Ærø, Fennec Kommune");
    w.settings.add_vocabulary("ærø");
    let entries = w.settings.vocabulary_entries();
    check(
        "typing two words adds two; a Danish case-only duplicate is left out",
        entries.len() == before + 2 && entries.iter().filter(|e| e.to_lowercase() == "ærø").count() == 1,
    );
    w.settings.remove_vocabulary("Ærø");
    w.settings.remove_vocabulary("Fennec Kommune");
    screenshot(&w.window, "settings");
    w.settings.show_section("dictation");
    let settings_text = ui::texts_in(&w.settings.root);
    check(
        "Settings lists spoken punctuation and the other ways to say a command",
        settings_text.iter().any(|t| t == "«punktum»")
            && settings_text
                .iter()
                .any(|t| t.contains("«stop diktat» or «stop optagelse»")),
    );
    check(
        "Settings offers automatic punctuation, on and ready",
        settings_text
            .iter()
            .any(|t| t == "Add commas, full stops and capitals")
            && settings_text
                .iter()
                .any(|t| t.contains("your words are never changed"))
            && Settings::default().punctuate,
    );
    screenshot(&w.window, "settings-dictation");
    w.settings.show_section("storage");
    w.settings.audio_retention.set_selected(2);
    let on_disk = Settings::load(&root.join("config/settings.toml")).unwrap();
    check(
        "Delete audio after is saved to settings.toml",
        on_disk.delete_audio_after_days == Some(90),
    );
    screenshot(&w.window, "settings-storage");
    w.settings.audio_retention.set_selected(0);
    w.settings.show_section("model");

    // --- first run: one download of everything dictation needs
    let root_s = tmp.path().join("setup");
    let d = deps(&root_s, vec![], true);
    let models = d.paths.models();
    std::fs::remove_file(models.join(&Settings::default().model)).unwrap();
    std::fs::remove_file(models.join(fennec::models::VAD_FILE)).unwrap();
    std::fs::remove_dir_all(models.join(fennec::punctuation::DIR)).unwrap();
    let ws = ui::build_window(d);
    check(
        "a fresh install offers to download what dictation needs",
        ws.dictation.setup_shown()
            && ui::texts_in(&ws.dictation.root)
                .iter()
                .any(|t| t.contains("speech model") && t.contains("punctuation model")),
    );
    ws.dictation.start_setup();
    check(
        "after the download the offer goes and dictation is ready",
        pump_until(Duration::from_secs(10), || {
            !ws.dictation.setup_shown() && ws.dictation.model_loaded() && ws.dictation.punctuator_loaded()
        }),
    );
    check(
        "with everything installed there is no offer",
        !w.dictation.setup_shown(),
    );

    // --- a missing model explains itself instead of failing silently
    let root_b = tmp.path().join("b");
    let w2 = ui::build_window(deps(&root_b, vec![], false));
    w2.dictation.start_recording();
    let told = pump_until(Duration::from_secs(5), || {
        w2.dictation.dock.status_text().contains("model file not found")
    });
    check("missing model shows the reason", told);
    check(
        "missing model leaves the dock ready",
        w2.dictation.dock.state_text() == "Ready" && !w2.dictation.is_recording(),
    );

    // --- projects appear in the sidebar
    let store_b = Store::open(&root_b.join("data/fennec.db")).unwrap();
    store_b.create_project("Operation Harbour", "#1D4ED8").unwrap();
    w2.sidebar.refresh();
    check(
        "new projects appear in the sidebar",
        w2.sidebar.project_names() == ["All documents", "Operation Harbour", "Unsorted"],
    );

    // --- AI: off by default, then every action through the real window
    ai_checks(&tmp.path().join("c"));

    // --- microphone: level test, input volume, clipping warning
    audio_checks(&tmp.path().join("d"));

    // --- documents in a project: select, move, tag, delete with Undo
    document_checks(&tmp.path().join("e"));

    // --- a phone pairs, sends a recording, and it is transcribed here
    phone_checks(&tmp.path().join("f"));

    // --- a phone plugged in over USB: import, transcribe, remove from the phone
    usb_checks(&tmp.path().join("g"));

    let failures = unsafe { FAILURES };
    if failures > 0 {
        println!("\n{failures} UI check(s) failed");
        std::process::exit(1);
    }
    println!("\nall UI checks passed");
}

/// Selects documents in a project, then moves, tags and deletes them the
/// way the bulk bar and row menus do.
fn document_checks(root: &std::path::Path) {
    use fennec::store::{NewDocument, ProjectFilter};
    use fennec::ui::Nav;
    let w = ui::build_window(deps(root, vec![], true));
    let store = Store::open(&root.join("data/fennec.db")).unwrap();
    let vendor = store.create_project("Leverandør", "#1D4ED8").unwrap();
    let meetings = store.create_project("Møder", "#0F766E").unwrap();
    let ids: Vec<_> = ["Møde om tilbud", "Prisforhandling", "Telefon med Jens"]
        .iter()
        .map(|t| {
            store
                .create_document(&NewDocument {
                    project_id: Some(vendor),
                    ..NewDocument::dictation(t)
                })
                .unwrap()
        })
        .collect();
    let count = |id| {
        store
            .projects()
            .unwrap()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.document_count)
            .unwrap_or(0)
    };
    w.sidebar.refresh();
    w.sidebar.go(Nav::Project(ProjectFilter::Project(vendor)));
    check(
        "nothing selected shows the table head",
        !w.project.bulk_bar_shown(),
    );
    w.project.select_document(ids[0], true);
    w.project.select_document(ids[1], true);
    check(
        "selecting documents shows the bulk bar",
        w.project.bulk_bar_shown() && w.project.selected_ids().len() == 2,
    );
    w.project.move_selected(Some(meetings));
    check(
        "Move to moves every selected document",
        store.document(ids[0]).unwrap().project_id == Some(meetings)
            && store.document(ids[1]).unwrap().project_id == Some(meetings),
    );
    check(
        "moved documents leave the list and the selection",
        w.project.shown_titles() == ["Telefon med Jens"] && !w.project.bulk_bar_shown(),
    );
    check(
        "project counts follow the move",
        count(vendor) == 1 && count(meetings) == 2,
    );

    w.sidebar.go(Nav::Project(ProjectFilter::Project(meetings)));
    w.project.select_all();
    w.project.set_tag_on_selected("Tilbud", true);
    check(
        "a tag added in bulk lands on every selected document",
        store.document(ids[0]).unwrap().tags == ["tilbud"]
            && store.document(ids[1]).unwrap().tags == ["tilbud"],
    );
    w.project.clear_selection();
    w.project.select_document(ids[1], true);
    w.project.set_tag_on_selected("tilbud", false);
    check(
        "a tag can be taken off again",
        store.document(ids[1]).unwrap().tags.is_empty() && store.document(ids[0]).unwrap().tags == ["tilbud"],
    );

    let audio = Paths::under(root).audio();
    std::fs::create_dir_all(&audio).unwrap();
    let recording = audio.join("tilbud.wav");
    std::fs::write(&recording, b"stand-in").unwrap();
    store.set_audio_path(ids[0], Some(&recording)).unwrap();
    w.project.clear_selection();
    w.project.select_document(ids[0], true);
    w.project.delete_selected();
    check(
        "Delete hides the document and offers Undo",
        !w.project.shown_titles().contains(&"Møde om tilbud".to_string())
            && w.project.toast_text().as_deref() == Some("Deleted «Møde om tilbud»")
            && store.document(ids[0]).is_ok(),
    );
    w.project.undo_delete();
    check(
        "Undo brings the document back",
        w.project.shown_titles().contains(&"Møde om tilbud".to_string()) && w.project.toast_text().is_none(),
    );
    w.project.select_document(ids[0], true);
    w.project.delete_selected();
    w.project.finish_delete();
    check(
        "without Undo the document is deleted, with its recording",
        store.document(ids[0]).is_err() && !recording.exists() && count(meetings) == 1,
    );

    // Closing the window (or quitting) carries out a delete waiting for Undo.
    w.project.select_document(ids[1], true);
    w.project.delete_selected();
    w.before_close();
    check(
        "closing the window carries out a delete waiting for Undo",
        store.document(ids[1]).is_err() && w.project.toast_text().is_none(),
    );

    // A row menu hangs from its row; redrawing the rows closes it first.
    w.sidebar.go(Nav::Project(ProjectFilter::Project(vendor)));
    w.project.open_row_menu(0);
    let opened = w.project.menu_open();
    w.project.render_list();
    check(
        "redrawing the list closes an open row menu",
        opened && !w.project.menu_open(),
    );

    // The document being dictated into stays.
    w.dictation.open_document(ids[2]).unwrap();
    w.dictation.start_recording();
    let recording_started = pump_until(Duration::from_secs(5), || w.dictation.is_recording());
    w.sidebar.go(Nav::Project(ProjectFilter::Project(vendor)));
    w.project.delete_documents(vec![ids[2]]);
    w.project.finish_delete();
    check(
        "the document being dictated into is not deleted, and says why",
        recording_started
            && store.document(ids[2]).is_ok()
            && w.project
                .toast_text()
                .is_some_and(|t| t.contains("Stop dictation")),
    );
    w.dictation.stop_recording();
    pump_until(Duration::from_secs(10), || !w.dictation.is_recording());

    // A document opened in the editor during the Undo time is kept.
    let late = store
        .create_document(&NewDocument {
            project_id: Some(vendor),
            ..NewDocument::dictation("Opfølgning")
        })
        .unwrap();
    w.project.render_list();
    w.project.delete_documents(vec![late]);
    w.dictation.open_document(late).unwrap();
    w.project.finish_delete();
    check(
        "a document opened in the editor while Undo shows is kept",
        store.document(late).is_ok(),
    );

    // Moving or tagging skips documents waiting to be deleted.
    w.sidebar.go(Nav::Project(ProjectFilter::Project(vendor)));
    w.project.delete_documents(vec![ids[2]]);
    w.project.move_documents(&[ids[2]], Some(meetings));
    check(
        "a document waiting for Undo is not moved",
        store.document(ids[2]).unwrap().project_id == Some(vendor),
    );
    w.project.undo_delete();
}

/// Answers like a model would, by recognising each action's instructions.
fn fake_model(rec: &Recorded) -> Reply {
    let prompt = rec.prompt_text();
    if prompt.contains("Ret hvert afsnit") {
        let paragraphs: Vec<serde_json::Value> = prompt
            .lines()
            .filter_map(|l| {
                let (id, text) = l.strip_prefix("[p")?.split_once("] ")?;
                Some(serde_json::json!({"id": id.parse::<i64>().ok()?, "text": text.replace("øh ", "")}))
            })
            .collect();
        return Reply::Text(serde_json::json!({ "paragraphs": paragraphs }).to_string());
    }
    if prompt.contains("Find konkrete opgaver") {
        return Reply::Text(
            r#"{"items": [{"what": "Send tilbud", "who": "Jens", "due": "2026-10-09", "paragraph": null}]}"#
                .into(),
        );
    }
    if prompt.contains("Udfyld felterne") {
        let mut out = serde_json::Map::new();
        let schema = rec.body["response_format"]["json_schema"]["schema"]["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for key in schema.keys() {
            let v = if key == "emne" {
                "Tilbud til kunden".into()
            } else {
                serde_json::Value::Null
            };
            out.insert(key.clone(), v);
        }
        return Reply::Text(serde_json::Value::Object(out).to_string());
    }
    if prompt.contains("Besvar spørgsmålet") {
        // The first paragraph line (the instructions contain an example marker).
        let start = prompt.find("\n[d").map_or(0, |i| i + 1);
        let marker = &prompt[start..start + prompt[start..].find(']').map_or(0, |e| e + 1)];
        return Reply::Text(format!(
            "Jens sender tilbuddet {marker}, og rabatten er 5 % [d999:p1]."
        ));
    }
    Reply::Text("Resumé: Jens sender et tilbud på fredag.\n\n**Opfølgning**\n- Ring til Jens.".into())
}

fn ai_checks(root: &std::path::Path) {
    use fennec::ai::{Locality, Protocol, ProviderConfig};
    let local = MockLlm::start(Wire::OpenAi, fake_model);
    let cloud = MockLlm::start(Wire::Anthropic, fake_model);
    let provider = |id: &str, protocol, url: &str, locality| ProviderConfig {
        id: id.into(),
        name: id.into(),
        protocol,
        base_url: url.into(),
        model: "m".into(),
        locality,
        context_chars: 60_000,
    };

    // A document to work on, created before the window opens it.
    {
        let store = Store::open(&{
            std::fs::create_dir_all(root.join("data")).unwrap();
            root.join("data/fennec.db")
        })
        .unwrap();
        let project = store.create_project("Leverandør", "#1D4ED8").unwrap();
        let doc = store
            .create_document(&fennec::store::NewDocument {
                project_id: Some(project),
                template_id: Some("notat".into()),
                ..fennec::store::NewDocument::dictation("Møde om tilbud")
            })
            .unwrap();
        store
            .replace_paragraphs(
                doc,
                &[
                    Paragraph::new("Jens sender øh tilbuddet på fredag."),
                    Paragraph::new("Alt andet er på plads."),
                ],
            )
            .unwrap();
    }

    let mut d = deps(root, vec![], true);
    {
        let mut s = d.settings.borrow_mut();
        s.ai.providers = vec![
            provider("ollama", Protocol::OpenAi, &local.url, Locality::ThisComputer),
            provider("claude", Protocol::Anthropic, &cloud.url, Locality::Cloud),
        ];
        s.ai.default_provider = "ollama".into();
    }
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::<fennec::ui::CloudSend>::new()));
    {
        let asked = asked.clone();
        d.confirm_cloud = std::rc::Rc::new(move |_, send, answer| {
            asked.borrow_mut().push(send.clone());
            answer(true);
        });
    }
    let w = ui::build_window(d);
    w.window.present();
    let store = Store::open(&root.join("data/fennec.db")).unwrap();
    let doc = w.dictation.document().unwrap();

    check(
        "AI controls are hidden while AI is off",
        !w.dictation.ai_menu.get_visible(),
    );
    w.dictation.ai_summarize();
    pump_until(Duration::from_millis(500), || false);
    check(
        "with AI off, nothing is sent",
        local.count() == 0 && cloud.count() == 0 && w.dictation.summary.status.text().contains("turned off"),
    );

    // Turning AI on in Settings shows the controls and saves the choice.
    w.sidebar.go(ui::Nav::Settings);
    w.settings.show_section("ai");
    w.settings.ai.enabled.set_active(true);
    check(
        "turning AI on shows the AI menu",
        w.dictation.ai_menu.get_visible(),
    );
    let saved = std::fs::read_to_string(root.join("config/settings.toml")).unwrap_or_default();
    check("the AI switch is saved", saved.contains("enabled = true"));
    screenshot(&w.window, "settings-ai");
    w.settings.ai.set_editing("claude", true);
    screenshot(&w.window, "settings-ai-edit");
    w.settings.ai.set_editing("claude", false);
    w.settings.show_section("ai-defaults");
    screenshot(&w.window, "settings-ai-defaults");
    w.settings.show_section("privacy");
    let rows = w.settings.ai.local_only_rows();
    check(
        "Privacy lists the projects, none local only yet",
        !rows.is_empty() && rows.iter().all(|(_, on)| !on),
    );
    screenshot(&w.window, "settings-privacy");
    w.sidebar.go(ui::Nav::Dictate);

    // Summary streams into the panel and is stored.
    w.dictation.ai_summarize();
    let summarized = pump_until(Duration::from_secs(5), || {
        w.dictation.summary.summary_text().starts_with("Resumé")
            && w.dictation.summary.meta_text().contains("ollama")
    });
    check("a summary appears with its provider", summarized);
    check(
        "the summary is stored",
        store.summaries_for_document(doc).map(|s| s.len()).unwrap_or(0) == 1,
    );
    check(
        "the summary reads as paragraphs, headings and bullets",
        w.dictation.summary.rendered_blocks()
            == [
                "Resumé: Jens sender et tilbud på fredag.",
                "Opfølgning",
                "• Ring til Jens.",
            ],
    );
    check(
        "its line names where it ran and the prompt",
        w.dictation
            .summary
            .meta_text()
            .contains("ollama (m) · this computer · prompt «Kort resumé»"),
    );
    w.dictation.summary.press_edit();
    let editing = w.dictation.summary.is_editing();
    w.dictation.summary.press_edit();
    check(
        "Edit opens the raw text and Done saves it",
        editing
            && !w.dictation.summary.is_editing()
            && store.summaries_for_document(doc).unwrap()[0].text == w.dictation.summary.summary_text(),
    );
    screenshot(&w.window, "summary");

    // Clean-up: review, accept, undo.
    w.dictation.ai_cleanup();
    check("clean-up opens its review screen", w.visible_page() == "cleanup");
    let proposed = pump_until(Duration::from_secs(5), || w.dictation.cleanup.states().len() == 1);
    check("clean-up proposes the changed paragraph only", proposed);
    check(
        "the paragraph left alone is listed as unchanged",
        w.dictation.cleanup.unchanged_count() == 1,
    );
    check(
        "clean-up names the provider and where it runs",
        w.dictation.cleanup.provider_text().contains(" · "),
    );
    screenshot(&w.window, "cleanup");
    w.dictation.cleanup.accept_row(0);
    let first = |w: &ui::MainWindow| w.dictation.editor.paragraphs()[0].text.clone();
    check(
        "accepting changes the editor",
        first(&w) == "Jens sender tilbuddet på fredag.",
    );
    check(
        "the accepted text is saved",
        store.paragraphs(doc).unwrap()[0].text == "Jens sender tilbuddet på fredag.",
    );
    w.dictation.cleanup.undo_row(0);
    check(
        "undo restores the original",
        first(&w) == "Jens sender øh tilbuddet på fredag.",
    );
    w.dictation.cleanup.done.emit_clicked();
    check("Back returns to the document", w.visible_page() == "dictate");

    // Action items.
    w.dictation.ai_action_items();
    let found = pump_until(Duration::from_secs(5), || {
        w.dictation.actions.items() == ["Send tilbud"]
    });
    check(
        "action items are listed and stored",
        found && store.document_action_items(doc).unwrap().len() == 1,
    );
    check(
        "they are marked AI-generated with who found them, and when",
        w.dictation.actions.badge_visible()
            && w.dictation
                .actions
                .summary_text()
                .starts_with("1 action item · ollama · this computer · today "),
    );

    // Field suggestions stay suggestions until accepted.
    w.dictation.ai_suggest_fields();
    let suggested = pump_until(Duration::from_secs(5), || {
        w.dictation.inspector.suggestion("emne").as_deref() == Some("Tilbud til kunden")
    });
    let emne = w.dictation.inspector.entry("emne").unwrap();
    check(
        "a field value is suggested, not filled in",
        suggested && emne.text().is_empty(),
    );
    if !suggested {
        println!("     status: {}", w.dictation.inspector.suggest_status.text());
    }
    w.dictation.inspector.use_suggestion("emne");
    check("Use fills the field", emne.text() == "Tilbud til kunden");

    // Ask the project, with citations checked.
    let project = store.projects().unwrap()[0].id;
    w.sidebar
        .go(ui::Nav::Project(fennec::store::ProjectFilter::Project(project)));
    w.project.question.set_text("Hvem sender tilbuddet?");
    w.project.ask();
    let answered = pump_until(Duration::from_secs(5), || !w.project.citation_labels().is_empty());
    check("Ask answers with a source", answered);
    if !answered {
        println!(
            "     status: {} answer: {}",
            w.project.ask_status.text(),
            w.project.answer.text()
        );
    }
    check(
        "unknown citations are dropped",
        !w.project.answer.text().contains("d999") && w.project.citation_labels().len() == 1,
    );
    check(
        "the answer numbers its source like the citation chip",
        !w.project.answer.text().contains("[d")
            && w.project
                .citation_labels()
                .first()
                .is_some_and(|c| c.starts_with("1 ")),
    );
    screenshot(&w.window, "ask");
    w.project.show_tab("actions");
    screenshot(&w.window, "project-actions");
    check("no request went to the cloud", cloud.count() == 0);

    // A cloud provider asks first, once per document.
    w.settings.ai.default_choice.set_selected(1);
    pump_until(Duration::from_millis(100), || false);
    w.sidebar.go(ui::Nav::Dictate);
    w.dictation.ai_summarize();
    let sent = pump_until(Duration::from_secs(5), || {
        cloud.count() == 1 && w.dictation.summary.meta_text().contains("claude")
    });
    check(
        "the first cloud send asks, naming where it goes",
        sent && asked.borrow().len() == 1 && asked.borrow()[0].host.starts_with("127.0.0.1"),
    );
    w.dictation.ai_summarize();
    pump_until(Duration::from_secs(5), || cloud.count() == 2);
    check("the next send does not ask again", asked.borrow().len() == 1);

    // A job can use its own provider: summaries on the local one, the
    // default staying on the cloud.
    let summaries = &w.settings.ai.job_choices[0];
    check(
        "the first job row is Summaries",
        summaries.0 == fennec::ai::AiJob::Summaries,
    );
    summaries.1.set_selected(1);
    let saved = std::fs::read_to_string(root.join("config/settings.toml")).unwrap_or_default();
    check(
        "the job's provider is saved",
        saved.contains("summaries = \"ollama\""),
    );
    let local_before = local.count();
    w.dictation.ai_summarize();
    let routed = pump_until(Duration::from_secs(5), || {
        local.count() > local_before && w.dictation.summary.meta_text().contains("ollama")
    });
    check(
        "a summary goes to the provider set for summaries",
        routed && cloud.count() == 2,
    );
    summaries.1.set_selected(0);
    check(
        "Default provider clears the job's choice",
        !w.settings.settings().ai.jobs.contains_key("summaries"),
    );

    // Local-only projects never reach the cloud; the box is in Settings → Privacy.
    let name = store.projects().unwrap()[0].name.clone();
    w.settings.ai.render_projects();
    w.settings.ai.set_local_only(&name, true);
    check(
        "the Privacy switch marks the project local only",
        store.projects().unwrap()[0].local_only,
    );
    check(
        "the sidebar line under Privacy counts it at once",
        w.settings
            .nav_summaries()
            .iter()
            .any(|(id, line)| id == "privacy" && line == "1 project local-only"),
    );
    let local_before = local.count();
    w.dictation.ai_summarize();
    let rerouted = pump_until(Duration::from_secs(5), || local.count() > local_before);
    check(
        "a local-only project's summary goes to the local provider, never the cloud",
        rerouted && cloud.count() == 2,
    );
    check(
        "AI defaults names the provider local-only projects use",
        w.settings.ai.local_note_text() == "Local-only projects always use ollama, whatever is set here.",
    );
}

fn clipped(secs: usize) -> Vec<f32> {
    (0..secs * 16_000)
        .map(|i| if (i / 20) % 2 == 0 { 1.0 } else { -1.0 })
        .collect()
}

fn audio_checks(root: &std::path::Path) {
    let mut d = deps(root, vec!["Hej."], true);
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let flag = std::sync::Arc::clone(&flag);
        d.audio = Arc::new(move |_| {
            let pcm = if flag.load(std::sync::atomic::Ordering::Relaxed) {
                clipped(3)
            } else {
                tones(3)
            };
            Ok(Box::new(PcmSource::new(pcm)) as Box<dyn AudioSource>)
        });
    }
    let w = ui::build_window(d);
    w.window.present();
    w.sidebar.go(ui::Nav::Settings);
    w.settings.show_section("dictation");

    w.settings.mic_test.run();
    let judged = pump_until(Duration::from_secs(10), || {
        w.settings.mic_test.verdict().is_some()
    });
    check(
        "the microphone test judges a normal level as good",
        judged && w.settings.mic_test.verdict() == Some(fennec::audio::level::Verdict::Good),
    );
    check(
        "the test recording can be played back",
        w.settings.mic_test.play.is_sensitive(),
    );
    screenshot(&w.window, "settings-microphone");

    flag.store(true, std::sync::atomic::Ordering::Relaxed);
    w.settings.mic_test.run();
    pump_until(Duration::from_secs(10), || {
        w.settings.mic_test.verdict() == Some(fennec::audio::level::Verdict::TooLoud)
    });
    check(
        "the microphone test flags clipping",
        w.settings.mic_test.verdict() == Some(fennec::audio::level::Verdict::TooLoud)
            && w.settings
                .mic_test
                .verdict
                .text()
                .contains("Lower the input volume"),
    );

    if let Some(gain) = w.settings.input_gain.borrow().as_ref() {
        gain.set_value(-6.0);
    }
    let saved = std::fs::read_to_string(root.join("config/settings.toml")).unwrap_or_default();
    check(
        "the input volume is saved",
        saved.contains("input_gain_db = -6.0"),
    );

    w.sidebar.go(ui::Nav::Dictate);
    w.dictation.start_recording();
    let warned = pump_until(Duration::from_secs(10), || {
        w.dictation.dock.status_text().contains("too loud and clips")
    });
    check("dictating with a clipping microphone warns in the dock", warned);
    pump_until(Duration::from_secs(10), || !w.dictation.is_recording());

    // --- the level meter: room noise low, speech high
    let dock = &w.dictation.dock;
    dock.set_state(ui::DockState::Recording);
    for _ in 0..40 {
        dock.push_level(0.012);
    }
    dock.push_level(0.045);
    // The bars scroll in on the frame clock.
    pump_until(Duration::from_secs(2), || {
        dock.bar_heights().last().is_some_and(|h| *h > 0.0)
    });
    let bars = dock.bar_heights();
    check(
        "the level meter shows room noise low and speech clearly higher",
        bars[bars.len() - 2] < 0.15 && bars[bars.len() - 1] > 0.4,
    );
    if !(bars[bars.len() - 2] < 0.15 && bars[bars.len() - 1] > 0.4) {
        println!("     bars: {bars:?}");
    }
    dock.set_state(ui::DockState::Idle);

    // --- stopping shows at once, while the last sentence is transcribed
    w.dictation.start_recording();
    pump_until(Duration::from_secs(5), || w.dictation.is_recording());
    w.dictation.stop_recording();
    check(
        "pressing stop shows Finishing at once",
        w.dictation.dock.state_text() == "Finishing…",
    );
    check(
        "the dock is Ready once the last sentence is in",
        pump_until(Duration::from_secs(10), || {
            w.dictation.dock.state_text() == "Ready"
        }),
    );
}

/// A free port on localhost for the phone receiver.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// What the stand-in phone saw, sent back to the main thread.
#[derive(Debug, Default)]
struct PhoneRun {
    paired: bool,
    document_id: Option<i64>,
    final_state: String,
}

/// Plays Fennec Recorder against the real window: pairs (someone presses
/// Allow), sends a WAV in chunks and waits until it is transcribed.
fn phone_checks(root: &std::path::Path) {
    use sha2::Digest;

    let d = deps(root, vec!["Hej fra telefonen."], true);
    let port = free_port();
    d.settings.borrow_mut().phone.port = port;
    let w = ui::build_window(d);
    w.window.present();
    w.sidebar.go(ui::Nav::Settings);
    w.settings.show_section("phone");
    let summaries = w.settings.nav_summaries();
    check(
        "Settings lists Phone, off until turned on",
        summaries.iter().any(|(id, line)| id == "phone" && line == "Off"),
    );
    check(
        "receiving is off by default: nothing listens",
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
    );

    w.settings.phone.start_pairing();
    let offer = w.phone.offer();
    check(
        "Pair a phone turns receiving on and shows a code",
        offer.is_some() && w.settings.phone.enabled.is_active(),
    );
    let Some(offer) = offer else { return };
    let texts = ui::texts_in(&w.settings.root);
    check(
        "the address and code are shown for phones without a camera",
        texts.iter().any(|t| t == &offer.address) && texts.iter().any(|t| t.contains(&offer.token_display())),
    );
    screenshot(&w.window, "settings-phone-pairing");

    let wav = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/da_fleurs_0.wav"
    ))
    .unwrap();
    let token = offer.token.clone();
    let base = format!("https://{}", offer.address);
    let phone = std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap();
        let mut run = PhoneRun::default();
        let paired: serde_json::Value = client
            .post(format!("{base}/v1/pair"))
            .json(&serde_json::json!({
                "token": token, "device_name": "Pixel 8", "nonce": "ui-test-nonce-0123456789"
            }))
            .send()
            .and_then(|r| r.json())
            .unwrap_or_default();
        let Some(secret) = paired["secret"].as_str().map(str::to_string) else {
            return run;
        };
        run.paired = true;
        let id = "11111111-2222-3333-4444-555555555555";
        let sum: String = sha2::Sha256::digest(&wav)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let call = |m: reqwest::Method, path: &str, body: Vec<u8>| -> serde_json::Value {
            client
                .request(m, format!("{base}{path}"))
                .bearer_auth(&secret)
                .body(body)
                .send()
                .and_then(|r| r.json())
                .unwrap_or_default()
        };
        call(
            reqwest::Method::POST,
            "/v1/projects",
            serde_json::to_vec(&serde_json::json!({ "name": "Fra telefonen" })).unwrap(),
        );
        let meta = serde_json::json!({
            "title": "Telefonmøde", "recorded_at": 1_791_000_000_000_i64, "duration_ms": 5000,
            "ext": "wav", "size": wav.len(), "sha256": sum,
        });
        call(
            reqwest::Method::PUT,
            &format!("/v1/recordings/{id}"),
            serde_json::to_vec(&meta).unwrap(),
        );
        for (i, piece) in wav.chunks(64 * 1024).enumerate() {
            call(
                reqwest::Method::PUT,
                &format!("/v1/recordings/{id}/audio?offset={}", i * 64 * 1024),
                piece.to_vec(),
            );
        }
        run.document_id = call(
            reqwest::Method::POST,
            &format!("/v1/recordings/{id}/complete"),
            vec![],
        )["document_id"]
            .as_i64();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(30) {
            let st = call(reqwest::Method::GET, &format!("/v1/recordings?ids={id}"), vec![]);
            run.final_state = st["recordings"][0]["state"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if run.final_state == "done" || run.final_state == "failed" {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        run
    });

    let asked = pump_until(Duration::from_secs(10), || w.phone.pending().is_some());
    let pending = w.phone.pending();
    check(
        "a phone with the right code waits for Allow, with the code to compare",
        asked
            && pending.as_ref().is_some_and(|p| {
                p.device_name == "Pixel 8"
                    && p.code
                        == fennec::sync::pairing_code(&offer.pin, &offer.token, "ui-test-nonce-0123456789")
            }),
    );
    screenshot(&w.window, "settings-phone-allow");
    w.phone.decide(true);
    pump_until(Duration::from_secs(60), || phone.is_finished());
    let run = phone.join().unwrap();
    check("the phone is paired after Allow", run.paired);
    check(
        "a project added on the phone appears in the sidebar",
        pump_until(Duration::from_secs(5), || {
            w.sidebar.project_names().iter().any(|n| n == "Fra telefonen")
        }),
    );
    check(
        "the paired phone is listed",
        ui::texts_in(&w.settings.root).iter().any(|t| t == "Pixel 8"),
    );
    check(
        "the recording becomes a document in the Files queue",
        run.document_id.is_some()
            && w.files
                .statuses()
                .iter()
                .any(|(name, _)| name.starts_with("phone-")),
    );
    let queue = ui::texts_in(&w.files.root);
    check(
        "the queue shows the phone recording by its title, not its file name",
        queue.iter().any(|t| t == "Telefonmøde") && !queue.iter().any(|t| t.starts_with("phone-")),
    );
    check(
        "the phone sees its recording transcribed",
        run.final_state == "done",
    );
    let store = Store::open(&root.join("data/fennec.db")).unwrap();
    let text = run
        .document_id
        .and_then(|doc| store.paragraphs(doc).ok())
        .unwrap_or_default();
    check(
        "the transcript is saved under the phone's title",
        text.iter().any(|p| p.text.contains("Hej fra telefonen"))
            && run
                .document_id
                .and_then(|doc| store.document(doc).ok())
                .is_some_and(|d| d.title == "Telefonmøde"),
    );
    check(
        "the phone's row counts what it sent",
        ui::texts_in(&w.settings.root)
            .iter()
            .any(|t| t.starts_with("Last seen") && t.ends_with("1 recording")),
    );
    check(
        "the Settings line counts the phone",
        w.settings
            .nav_summaries()
            .iter()
            .any(|(id, line)| id == "phone" && line == "On · 1 phone"),
    );
    screenshot(&w.window, "settings-phone-paired");

    w.settings.phone.enabled.set_active(false);
    check(
        "turning receiving off stops listening",
        pump_until(Duration::from_secs(5), || {
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
        }),
    );

    // Fennec closed halfway through transcribing a phone recording.
    let Some(doc) = run.document_id else { return };
    store
        .set_inbound_state_for_document(doc, fennec::store::InboundState::Transcribing, None)
        .unwrap();
    store
        .replace_paragraphs(doc, &[Paragraph::new("Halvt færdig")])
        .unwrap();
    drop(w);
    let w = ui::build_window(deps(root, vec!["Hej fra telefonen."], true));
    let resumed = pump_until(Duration::from_secs(20), || {
        store
            .inbound("11111111-2222-3333-4444-555555555555")
            .ok()
            .flatten()
            .is_some_and(|r| r.state == fennec::store::InboundState::Done)
    });
    let texts: Vec<String> = store
        .paragraphs(doc)
        .unwrap()
        .into_iter()
        .map(|p| p.text)
        .collect();
    check(
        "a phone recording cut off by closing Fennec is transcribed again from the start",
        resumed
            && texts.iter().any(|t| t.contains("Hej fra telefonen"))
            && !texts.iter().any(|t| t == "Halvt færdig"),
    );
    check(
        "resuming does not need receiving to be on",
        matches!(w.phone.state(), ui::phone::LinkState::Off),
    );
}

/// Puts a recording in a fake phone's transfer folder as Fennec Recorder does.
fn put_on_phone(folder: &std::path::Path, id: &str, title: &str, audio: &[u8]) {
    use sha2::Digest;
    let sum: String = sha2::Sha256::digest(audio)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(folder.join(format!("{id}.wav")), audio).unwrap();
    let meta = serde_json::json!({
        "fennec_recorder": 1, "id": id, "title": title, "recorded_at": 1_791_000_000_000_i64,
        "duration_ms": 5000, "ext": "wav", "size": audio.len(), "sha256": sum,
    });
    std::fs::write(folder.join(format!("{id}.json")), meta.to_string()).unwrap();
}

/// A phone in File transfer mode, as a folder: the offer, Import, the
/// transcript, and the copy gone from the phone (which tells the app).
fn usb_checks(root: &std::path::Path) {
    let w = ui::build_window(deps(root, vec!["Hej over kablet."], true));
    w.window.present();
    let phone = root.join("phone");
    let folder = phone.join("Internal shared storage/Download/Fennec Recorder");
    std::fs::create_dir_all(&folder).unwrap();
    let wav = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/da_fleurs_0.wav"
    ))
    .unwrap();
    put_on_phone(
        &folder,
        "aaaaaaaa-1111-2222-3333-444444444444",
        "Over kablet",
        &wav,
    );

    let root_file = gtk::gio::File::for_path(&phone);
    w.usb.check(root_file.clone(), "Pixel 8".into());
    check(
        "a phone with recordings Fennec lacks gets an offer to import",
        pump_until(Duration::from_secs(10), || {
            w.usb.offer().is_some_and(|o| o.ids.len() == 1)
        }),
    );
    screenshot(&w.window, "usb-offer");
    w.usb.answer(true);
    let done = pump_until(Duration::from_secs(10), || w.usb.last.borrow().is_some());
    check(
        "Import says what it did",
        done && w
            .usb
            .last
            .borrow()
            .as_deref()
            .is_some_and(|t| t.starts_with("1 recording imported")),
    );
    check(
        "Fennec shows the Files queue after importing",
        w.visible_page() == "files",
    );
    check(
        "the imported recording is removed from the phone, so the app marks it sent",
        std::fs::read_dir(&folder).unwrap().next().is_none(),
    );
    let store = Store::open(&root.join("data/fennec.db")).unwrap();
    let transcribed = pump_until(Duration::from_secs(20), || {
        store
            .inbound("aaaaaaaa-1111-2222-3333-444444444444")
            .ok()
            .flatten()
            .is_some_and(|r| r.state == fennec::store::InboundState::Done)
    });
    let doc = store
        .inbound("aaaaaaaa-1111-2222-3333-444444444444")
        .unwrap()
        .and_then(|r| r.document_id);
    check(
        "it is transcribed under its title from the phone",
        transcribed
            && doc.is_some_and(|d| {
                store.document(d).is_ok_and(|x| x.title == "Over kablet")
                    && store
                        .paragraphs(d)
                        .unwrap()
                        .iter()
                        .any(|p| p.text.contains("Hej over kablet"))
            }),
    );

    // Not now: not asked again for that recording.
    put_on_phone(&folder, "bbbbbbbb-1111-2222-3333-444444444444", "Senere", &wav);
    w.usb.check(root_file.clone(), "Pixel 8".into());
    pump_until(Duration::from_secs(10), || w.usb.offer().is_some());
    w.usb.answer(false);
    w.usb.check(root_file, "Pixel 8".into());
    pump_until(Duration::from_millis(800), || false);
    check(
        "after Not now the same recording is not offered again",
        w.usb.offer().is_none() && folder.join("bbbbbbbb-1111-2222-3333-444444444444.wav").exists(),
    );
}
