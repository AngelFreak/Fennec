//! Builds the real window against a temporary home with a scripted engine
//! and synthetic audio, then drives it like a user. GTK must run on the main
//! thread, so this is one binary with its own `main` (harness = false).
//! Needs a display; the desktop session provides one.

#![allow(clippy::single_range_in_vec_init)]

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

static mut FAILURES: usize = 0;

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
struct Scripted(Vec<&'static str>);
impl Transcriber for Scripted {
    fn transcribe(&mut self, _: &[f32], _: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        let text = if self.0.is_empty() {
            "Mere tekst."
        } else {
            self.0.remove(0)
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
        v.extend(std::iter::repeat_n(0.0, 16_000));
    }
    v
}

fn deps(root: &std::path::Path, lines: Vec<&'static str>, engine_ok: bool) -> Deps {
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
        audio: Arc::new(|_| Ok(Box::new(PcmSource::new(tones(3))) as Box<dyn AudioSource>)),
        vad: Arc::new(|_, _| Ok(Box::new(EnergyVad::default()) as Box<dyn FrameVad>)),
        file_vad: Arc::new(|_, _| Box::new(WholeAudio) as Box<dyn SpeechDetector>),
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

    // --- first launch: a new document with the default template
    let root = tmp.path().join("a");
    let w = ui::build_window(deps(
        &root,
        vec!["Første sætning.", "Nyt afsnit.", "Anden sætning."],
        true,
    ));
    check("opens on the Dictate screen", w.visible_page() == "dictate");
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
    let stopped = pump_until(Duration::from_secs(15), || {
        !w.dictation.is_recording() && w.dictation.dock.state_text() == "Ready"
    });
    check("dictation runs to the end of the audio", stopped);
    let texts: Vec<String> = w
        .dictation
        .editor
        .paragraphs()
        .into_iter()
        .map(|p| p.text)
        .collect();
    check(
        "spoken command split the paragraphs",
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
    w.window.present();
    screenshot(&w.window, "dictate");
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
    screenshot(&w.window, "export");
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
    screenshot(&w.window, "project");
    w.project.press_export();
    check(
        "project export opens Export for its documents",
        w.visible_page() == "export",
    );
    w.sidebar.go(fennec::ui::Nav::Tag("vendor".into()));
    check(
        "a tag shows documents across projects",
        w.project.shown_titles() == [title],
    );

    // --- templates: create, edit and save; invalid templates explain why
    w.sidebar.go(fennec::ui::Nav::Templates);
    check(
        "Templates lists the defaults",
        w.templates.names() == ["Afhøringsrapport", "Mødereferat", "Notat"],
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
    screenshot(&w.window, "templates");
    w.templates.add_field("Adresse", false);
    check(
        "a duplicate field is refused with a reason",
        w.templates.save().is_err() && w.templates.error_text().contains("twice"),
    );

    // --- settings: models, compute, dictation; changes are saved
    w.sidebar.go(fennec::ui::Nav::Settings);
    check("Settings opens", w.visible_page() == "settings");
    check(
        "the catalog lists Edda first",
        w.settings.model_names().first().map(String::as_str) == Some("Edda v0.1"),
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
    screenshot(&w.window, "settings");

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

    let failures = unsafe { FAILURES };
    if failures > 0 {
        println!("\n{failures} UI check(s) failed");
        std::process::exit(1);
    }
    println!("\nall UI checks passed");
}
