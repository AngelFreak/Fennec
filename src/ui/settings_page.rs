//! Settings: speech model and compute, dictation, AI, privacy, storage. Changes apply to
//! the shared settings and are saved to `settings.toml` right away.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::engine::EngineHolder;
use super::mic_test::MicTest;
use super::settings_ai::AiSettingsUi;
use super::{Deps, Handler, label};
use crate::audio::capture::input_devices;
use crate::models::{self, Progress, Source};
use crate::worker::Priority;

/// A model download or conversion in progress.
struct Install {
    _cancel: Arc<AtomicBool>,
    bar: gtk::ProgressBar,
    status: gtk::Label,
}

pub struct SettingsPage {
    pub root: gtk::Box,
    /// Buttons for the window header while this screen shows.
    pub header_actions: gtk::Box,
    deps: Deps,
    engine: Rc<EngineHolder>,
    stack: gtk::Stack,
    nav: RefCell<Vec<(String, gtk::Button)>>,
    models_box: gtk::Box,
    installing: RefCell<Option<Install>>,
    backend_box: gtk::Box,
    speed_label: gtk::Label,
    message: gtk::Label,
    pub ai: Rc<AiSettingsUi>,
    pub mic_test: Rc<MicTest>,
    pub input_gain: RefCell<Option<gtk::Scale>>,
    on_model_changed: Handler<()>,
}

impl SettingsPage {
    pub fn new(deps: Deps, engine: Rc<EngineHolder>) -> Rc<Self> {
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        let nav_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        nav_box.add_css_class("fx-settings-nav");
        nav_box.set_size_request(200, -1);

        let models_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let backend_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        backend_box.add_css_class("linked");
        backend_box.set_homogeneous(true);
        let speed_label = label("Latency per sentence: not measured yet", &["fx-status"]);
        speed_label.set_wrap(true);
        speed_label.set_hexpand(true);
        let message = label("", &["fx-field-error"]);
        message.set_wrap(true);

        let ai = AiSettingsUi::new(deps.clone());
        let mic_test = MicTest::new(deps.clone());
        let page = Rc::new(Self {
            root: gtk::Box::new(gtk::Orientation::Horizontal, 0),
            header_actions: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            ai,
            mic_test,
            input_gain: RefCell::default(),
            deps,
            engine,
            stack,
            nav: RefCell::default(),
            models_box,
            installing: RefCell::default(),
            backend_box,
            speed_label,
            message,
            on_model_changed: RefCell::default(),
        });
        let model_section = page.model_section();
        let dictation_section = page.dictation_section();
        let storage_section = page.storage_section();
        let mut nav = Vec::new();
        for (id, title, w) in [
            ("model", "Speech model", model_section),
            ("dictation", "Dictation", dictation_section),
            ("ai", "AI providers", page.ai.providers_section()),
            ("ai-defaults", "AI defaults", page.ai.defaults_section()),
            ("privacy", "Privacy", page.ai.privacy_section()),
            ("storage", "Storage", storage_section),
        ] {
            let scroll = gtk::ScrolledWindow::builder()
                .child(&w)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build();
            page.stack.add_named(&scroll, Some(id));
            let b = gtk::Button::with_label(title);
            b.add_css_class("fx-nav");
            nav_box.append(&b);
            nav.push((id.to_string(), b));
        }
        *page.nav.borrow_mut() = nav;
        for (id, b) in page.nav.borrow().iter() {
            let weak = Rc::downgrade(&page);
            let id = id.clone();
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.show_section(&id);
                }
            });
        }
        page.root.append(&nav_box);
        page.root.append(&page.stack);
        page.render_models();
        page.render_backends();
        page.show_section("model");
        page
    }

    /// AI settings changed (on/off, providers, default).
    pub fn connect_ai_changed(&self, f: impl Fn(()) + 'static) {
        self.ai.connect_changed(f);
    }

    pub fn connect_model_changed(&self, f: impl Fn(()) + 'static) {
        *self.on_model_changed.borrow_mut() = Some(Rc::new(f));
    }

    pub fn show_section(&self, id: &str) {
        self.stack.set_visible_child_name(id);
        for (nid, b) in self.nav.borrow().iter() {
            if nid == id {
                b.add_css_class("active");
            } else {
                b.remove_css_class("active");
            }
        }
    }

    fn save(&self) {
        if let Err(e) = self.deps.save_settings() {
            self.message.set_text(&format!("Settings not saved: {e}"));
        }
    }

    fn model_section(self: &Rc<Self>) -> gtk::Box {
        let b = section("Speech model");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let hint = label(
            "Models live in the models folder; conversions need Python with torch.",
            &["fx-field-note"],
        );
        hint.set_hexpand(true);
        hint.set_wrap(true);
        let add = gtk::Button::with_label("Add custom GGML model…");
        add.add_css_class("fx-secondary");
        head.append(&hint);
        head.append(&add);
        b.append(&head);
        b.append(&self.models_box);
        b.append(&self.message);
        b.append(&label("Compute", &["fx-crumb-current"]));
        b.append(&self.backend_box);
        let speed_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let run = gtk::Button::with_label("Run speed test");
        run.add_css_class("fx-secondary");
        speed_row.append(&self.speed_label);
        speed_row.append(&run);
        b.append(&speed_row);

        let weak = Rc::downgrade(self);
        run.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.run_speed_test();
            }
        });
        let weak = Rc::downgrade(self);
        add.connect_clicked(move |btn| {
            let Some(p) = weak.upgrade() else { return };
            let dialog = gtk::FileDialog::builder()
                .title("Choose a GGML model (.bin)")
                .modal(true)
                .build();
            let weak = Rc::downgrade(&p);
            dialog.open(
                btn.root().and_downcast::<gtk::Window>().as_ref(),
                gio::Cancellable::NONE,
                move |res| {
                    if let (Ok(f), Some(p)) = (res, weak.upgrade())
                        && let Some(path) = f.path()
                    {
                        p.add_custom(&path);
                    }
                },
            );
        });
        b
    }

    /// Copies a GGML file into the models folder and selects it.
    pub fn add_custom(self: &Rc<Self>, path: &std::path::Path) {
        let Some(name) = path.file_name() else { return };
        let dest = self.deps.paths.models().join(name);
        match std::fs::create_dir_all(self.deps.paths.models()).and_then(|_| std::fs::copy(path, &dest)) {
            Ok(_) => self.use_model(&name.to_string_lossy()),
            Err(e) => self
                .message
                .set_text(&format!("Could not copy {}: {e}", path.display())),
        }
    }

    /// Makes `file_name` the active model; the next dictation loads it.
    pub fn use_model(self: &Rc<Self>, file_name: &str) {
        self.deps.settings.borrow_mut().model = file_name.to_string();
        self.save();
        self.engine.reset();
        self.render_models();
        if let Some(f) = self.on_model_changed.borrow().clone() {
            f(());
        }
    }

    fn render_models(self: &Rc<Self>) {
        while let Some(c) = self.models_box.first_child() {
            self.models_box.remove(&c);
        }
        let active = self.deps.settings.borrow().model.clone();
        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(3)
            .min_children_per_line(1)
            .column_spacing(14)
            .row_spacing(14)
            .homogeneous(true)
            .build();
        for m in models::catalog() {
            let installed = models::is_installed(&m, &self.deps.paths);
            let card = card(m.name, m.publisher, m.description, &tags(&m));
            if m.file_name == active {
                card.add_css_class("active");
            }
            card.append(&self.model_action(m.file_name, installed, m.file_name == active, Some(m.clone())));
            grid.insert(&card, -1);
        }
        for p in models::custom_models(&self.deps.paths) {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let card = card(&name, "Custom", "A GGML model you added.", &[]);
            if name == active {
                card.add_css_class("active");
            }
            card.append(&self.model_action(&name, true, name == active, None));
            grid.insert(&card, -1);
        }
        self.models_box.append(&grid);
    }

    fn model_action(
        self: &Rc<Self>,
        file: &str,
        installed: bool,
        active: bool,
        entry: Option<models::CatalogModel>,
    ) -> gtk::Widget {
        if active && installed {
            return label("Active", &["fx-rec-state"]).upcast();
        }
        let b = gtk::Button::with_label(if installed {
            "Use"
        } else if matches!(entry.as_ref().map(|e| &e.source), Some(Source::Convert { .. })) {
            "Download & convert"
        } else if matches!(entry.as_ref().map(|e| &e.source), Some(Source::Snapshot { .. })) {
            "Download (2.8 GB)"
        } else {
            "Download"
        });
        b.add_css_class("fx-secondary");
        b.set_halign(gtk::Align::Start);
        if active {
            b.set_tooltip_text(Some("This model is selected but not on this computer yet"));
        }
        let file = file.to_string();
        let weak = Rc::downgrade(self);
        b.connect_clicked(move |b| {
            let Some(p) = weak.upgrade() else { return };
            match &entry {
                Some(m) if !installed => p.install(m.clone(), b),
                _ => p.use_model(&file),
            }
        });
        b.upcast()
    }

    fn install(self: &Rc<Self>, m: models::CatalogModel, button: &gtk::Button) {
        if self.installing.borrow().is_some() {
            self.message.set_text("Another model is already being installed.");
            return;
        }
        let progress = gtk::ProgressBar::new();
        progress.set_show_text(true);
        let status = label("Starting…", &["fx-field-note"]);
        if let Some(parent) = button.parent().and_downcast::<gtk::Box>() {
            parent.append(&progress);
            parent.append(&status);
        }
        button.set_sensitive(false);
        let cancel = Arc::new(AtomicBool::new(false));
        *self.installing.borrow_mut() = Some(Install {
            _cancel: Arc::clone(&cancel),
            bar: progress,
            status,
        });
        let (tx, rx) = async_channel::unbounded::<Result<Progress, Result<(), String>>>();
        let paths = self.deps.paths.clone();
        std::thread::spawn(move || {
            let send = |p| {
                let _ = tx.send_blocking(Ok(p));
            };
            // The voice detector comes with the first model.
            if let Err(e) = models::ensure_vad(&paths, &cancel, send) {
                tracing::warn!("could not fetch the voice detector: {e}");
            }
            let result = match &m.source {
                Source::Ggml { repo, file } => models::download(
                    &models::hf_url(repo, file),
                    &models::path_of(&m, &paths),
                    &cancel,
                    send,
                ),
                Source::Snapshot { repo } => models::fetch_snapshot(
                    repo,
                    &models::path_of(&m, &paths),
                    &models::python(),
                    &cancel,
                    send,
                ),
                Source::Convert { repo, quant } => {
                    let stem = m
                        .file_name
                        .trim_end_matches(".bin")
                        .trim_end_matches(&format!("-{quant}"))
                        .to_string();
                    models::convert(repo, &stem, quant, &models::python(), &cancel, send).map(|_| ())
                }
            };
            let _ = tx.send_blocking(Err(result));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let Some(p) = weak.upgrade() else { return };
                let guard = p.installing.borrow();
                let Some(Install { bar, status, .. }) = guard.as_ref() else {
                    return;
                };
                match msg {
                    Ok(Progress::Bytes { done, total }) => {
                        if let Some(t) = total.filter(|t| *t > 0) {
                            bar.set_fraction(done as f64 / t as f64);
                        } else {
                            bar.pulse();
                        }
                        bar.set_text(Some(&format!("{} MB", done / 1_000_000)));
                    }
                    Ok(Progress::Line(line)) => {
                        bar.pulse();
                        status.set_text(&line.chars().take(90).collect::<String>());
                    }
                    Err(result) => {
                        drop(guard);
                        p.installing.borrow_mut().take();
                        match result {
                            Ok(()) => p.message.set_text(""),
                            Err(e) => p.message.set_text(&format!("Installing the model failed: {e}")),
                        }
                        p.render_models();
                        return;
                    }
                }
            }
        });
    }

    fn render_backends(self: &Rc<Self>) {
        while let Some(c) = self.backend_box.first_child() {
            self.backend_box.remove(&c);
        }
        let current = self.deps.settings.borrow().backend;
        let mut group: Option<gtk::ToggleButton> = None;
        for info in models::backends() {
            let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
            inner.append(&label(info.label, &["fx-field-label"]));
            inner.append(&label(&info.detail, &["fx-field-note"]));
            let b = gtk::ToggleButton::builder()
                .child(&inner)
                .sensitive(info.available)
                .tooltip_text(info.detail.as_str())
                .build();
            if let Some(g) = &group {
                b.set_group(Some(g));
            } else {
                group = Some(b.clone());
            }
            b.set_active(info.backend == current);
            let weak = Rc::downgrade(self);
            let backend = info.backend;
            b.connect_toggled(move |b| {
                if b.is_active()
                    && let Some(p) = weak.upgrade()
                    && p.deps.settings.borrow().backend != backend
                {
                    p.deps.settings.borrow_mut().backend = backend;
                    p.save();
                    p.engine.reset();
                    if let Some(f) = p.on_model_changed.borrow().clone() {
                        f(());
                    }
                }
            });
            self.backend_box.append(&b);
        }
    }

    pub fn run_speed_test(self: &Rc<Self>) {
        self.speed_label
            .set_text("Loading the model and timing a Danish sentence…");
        let (tx, rx) = async_channel::bounded(1);
        self.engine.with_worker(move |res| match res {
            Ok(worker) => {
                let mut t = worker.transcriber(Priority::LiveFinal);
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(models::speed_test(&mut t));
                });
            }
            Err(e) => {
                let _ = tx.send_blocking(Err(e));
            }
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            let Some(p) = weak.upgrade() else { return };
            p.speed_label.set_text(&match result {
                Ok(r) => {
                    let verdict = if r.real_time_factor() < 0.5 {
                        "fast enough for live dictation."
                    } else if r.real_time_factor() < 1.0 {
                        "usable for dictation; expect a short wait after each pause."
                    } else {
                        "too slow for live dictation. Try a GPU backend, a smaller or q4 model, or use file transcription."
                    };
                    format!("{:.1} s of speech took {:.1} s ({:.2}× real time): {verdict}", r.audio_secs, r.elapsed_secs, r.real_time_factor())
                }
                Err(e) => format!("Speed test failed: {e}"),
            });
        });
    }

    pub fn settings(&self) -> crate::config::Settings {
        self.deps.settings()
    }

    pub fn speed_text(&self) -> String {
        self.speed_label.text().to_string()
    }

    fn dictation_section(self: &Rc<Self>) -> gtk::Box {
        let b = section("Dictation");
        let s = self.deps.settings();
        let devices = input_devices();
        let mut names = vec!["Default microphone".to_string()];
        names.extend(devices.iter().map(|d| d.name.clone()));
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mic = gtk::DropDown::from_strings(&refs);
        mic.set_selected(
            devices
                .iter()
                .position(|d| d.id == s.microphone)
                .map(|i| i as u32 + 1)
                .unwrap_or(0),
        );
        b.append(&field("Microphone", &mic));

        let gain = gtk::Scale::with_range(gtk::Orientation::Horizontal, -20.0, 20.0, 1.0);
        gain.set_value(f64::from(s.input_gain_db));
        gain.set_draw_value(true);
        gain.set_value_pos(gtk::PositionType::Right);
        gain.set_format_value_func(|_, v| format!("{v:+.0} dB"));
        gain.add_mark(0.0, gtk::PositionType::Bottom, None);
        gain.update_property(&[gtk::accessible::Property::Label("Input volume, decibels")]);
        b.append(&field("Input volume (added to the system level)", &gain));
        *self.input_gain.borrow_mut() = Some(gain.clone());
        let weak = Rc::downgrade(self);
        gain.connect_value_changed(move |sc| {
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().input_gain_db = sc.value().round() as f32;
                p.save();
            }
        });
        b.append(&self.mic_test.root);
        let weak = Rc::downgrade(self);
        mic.connect_selected_notify(move |dd| {
            let Some(p) = weak.upgrade() else { return };
            let id = dd
                .selected()
                .checked_sub(1)
                .and_then(|i| devices.get(i as usize))
                .map(|d| d.id.clone())
                .unwrap_or_default();
            p.deps.settings.borrow_mut().microphone = id;
            p.save();
        });

        let pause = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.3, 2.0, 0.1);
        pause.set_value(f64::from(s.pause_ms) / 1000.0);
        pause.set_draw_value(true);
        pause.set_value_pos(gtk::PositionType::Right);
        pause.update_property(&[gtk::accessible::Property::Label(
            "Pause before text is committed, seconds",
        )]);
        b.append(&field("Pause before text is committed (seconds)", &pause));
        let weak = Rc::downgrade(self);
        pause.connect_value_changed(move |sc| {
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().pause_ms = (sc.value() * 1000.0).round() as u32;
                p.save();
            }
        });

        b.append(
            &self.switch_row("Show a live preview while I speak", s.show_preview, |s, v| {
                s.show_preview = v
            }),
        );
        b.append(&self.switch_row(
            "Keep the audio of dictations (for playback)",
            s.keep_dictation_audio,
            |s, v| s.keep_dictation_audio = v,
        ));

        let user = gtk::Entry::builder()
            .text(s.user_name.as_str())
            .placeholder_text("Used for {user} in templates")
            .build();
        b.append(&field("Your name", &user));
        let weak = Rc::downgrade(self);
        user.connect_changed(move |e| {
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().user_name = e.text().trim().to_string();
                p.save();
            }
        });

        let vocab = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .height_request(80)
            .build();
        vocab.add_css_class("fx-vocab");
        vocab.buffer().set_text(&s.vocabulary);
        b.append(&field(
            "Vocabulary: names and terms the model should know",
            &vocab,
        ));
        let weak = Rc::downgrade(self);
        vocab.buffer().connect_changed(move |buf| {
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().vocabulary =
                    buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
                p.save();
            }
        });
        let cmds: Vec<String> = s
            .commands
            .phrases
            .iter()
            .map(|(say, c)| format!("«{say}» → {}", c.label()))
            .collect();
        let note = label(
            &format!("Voice commands (whole utterance only): {}", cmds.join(" · ")),
            &["fx-field-note"],
        );
        note.set_wrap(true);
        b.append(&note);
        b.append(&label(
            "Shortcut: Ctrl+Space starts and stops dictation inside Fennec.",
            &["fx-field-note"],
        ));
        b
    }

    fn switch_row(
        self: &Rc<Self>,
        text: &str,
        value: bool,
        set: fn(&mut crate::config::Settings, bool),
    ) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let l = label(text, &["fx-field-label"]);
        l.set_hexpand(true);
        let sw = gtk::Switch::builder()
            .active(value)
            .valign(gtk::Align::Center)
            .build();
        sw.update_property(&[gtk::accessible::Property::Label(text)]);
        row.append(&l);
        row.append(&sw);
        let weak = Rc::downgrade(self);
        sw.connect_active_notify(move |sw| {
            if let Some(p) = weak.upgrade() {
                set(&mut p.deps.settings.borrow_mut(), sw.is_active());
                p.save();
            }
        });
        row
    }

    fn storage_section(&self) -> gtk::Box {
        let b = section("Storage");
        let p = &self.deps.paths;
        for (name, path) in [
            ("Documents", p.database()),
            ("Audio", p.audio()),
            ("Speech models", p.models()),
            ("Templates & prompts", p.config_dir.clone()),
            ("Exports", p.exports()),
        ] {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("fx-storage-row");
            let n = label(name, &["fx-field-label"]);
            n.set_size_request(160, -1);
            let shown = path
                .to_string_lossy()
                .replacen(&std::env::var("HOME").unwrap_or_default(), "~", 1);
            let v = label(&shown, &["fx-stats"]);
            v.set_hexpand(true);
            v.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            let open = gtk::Button::with_label("Open folder");
            open.add_css_class("fx-secondary");
            let folder: PathBuf = if path.extension().is_some() {
                path.parent().map(PathBuf::from).unwrap_or(path.clone())
            } else {
                path.clone()
            };
            open.connect_clicked(move |b| {
                let _ = std::fs::create_dir_all(&folder);
                let l = gtk::FileLauncher::new(Some(&gio::File::for_path(&folder)));
                l.launch(
                    b.root().and_downcast::<gtk::Window>().as_ref(),
                    gio::Cancellable::NONE,
                    |_| {},
                );
            });
            row.append(&n);
            row.append(&v);
            row.append(&open);
            b.append(&row);
        }
        b
    }

    /// Model card titles, in order (tests).
    pub fn model_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![self.models_box.clone().upcast::<gtk::Widget>()];
        while let Some(w) = stack.pop() {
            if w.has_css_class("fx-model-card") {
                if let Some(t) = super::texts_in(&w).first() {
                    out.push(t.clone());
                }
                continue;
            }
            let mut c = w.last_child();
            while let Some(x) = c {
                c = x.prev_sibling();
                stack.push(x);
            }
        }
        out
    }
}

fn tags(m: &models::CatalogModel) -> Vec<String> {
    let mut t = vec![m.license.to_string(), m.size.to_string()];
    if let Some(w) = m.mean_wer {
        t.push(format!("Mean WER {w}"));
    }
    t
}

fn card(name: &str, publisher: &str, description: &str, tags: &[String]) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 8);
    c.add_css_class("fx-model-card");
    c.append(&label(name, &["fx-crumb-current"]));
    c.append(&label(publisher, &["fx-stats"]));
    let d = label(description, &["fx-field-note"]);
    d.set_wrap(true);
    d.set_max_width_chars(36);
    d.set_width_chars(24);
    c.append(&d);
    let t = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for tag in tags {
        t.append(&label(tag, &["fx-chip"]));
    }
    c.append(&t);
    c
}

fn section(title: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 16);
    b.add_css_class("fx-settings-section");
    b.append(&label(title, &["fx-project-title"]));
    b
}

fn field(name: &str, w: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&label(name, &["fx-field-label"]));
    b.append(w);
    b
}
