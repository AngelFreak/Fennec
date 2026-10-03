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
use crate::config::Backend;
use crate::models::{self, Progress, Source};
use crate::worker::Priority;

/// "Delete audio after" choices: label and days (`None`: never).
const RETENTION: [(&str, Option<u32>); 4] = [
    ("Never", None),
    ("30 days", Some(30)),
    ("90 days", Some(90)),
    ("1 year", Some(365)),
];

/// A model download or conversion in progress, shown inside its card.
struct Install {
    _cancel: Arc<AtomicBool>,
    bar: gtk::ProgressBar,
    status: gtk::Label,
    percent: gtk::Label,
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
    backend_box: gtk::Grid,
    backend_note: gtk::Label,
    speed_label: gtk::Label,
    message: gtk::Label,
    pub ai: Rc<AiSettingsUi>,
    pub mic_test: Rc<MicTest>,
    pub input_gain: RefCell<Option<gtk::Scale>>,
    /// "Delete audio after" (Storage).
    pub audio_retention: gtk::DropDown,
    on_model_changed: Handler<()>,
}

impl SettingsPage {
    pub fn new(deps: Deps, engine: Rc<EngineHolder>) -> Rc<Self> {
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        let nav_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        nav_box.add_css_class("fx-settings-nav");
        nav_box.set_size_request(220, -1);

        let models_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let backend_box = gtk::Grid::builder()
            .column_homogeneous(true)
            .row_homogeneous(true)
            .column_spacing(4)
            .row_spacing(4)
            .css_classes(["fx-segmented", "grid"])
            .build();
        backend_box.update_property(&[gtk::accessible::Property::Label("Compute")]);
        let backend_note = label("", &["fx-field-note"]);
        backend_note.set_wrap(true);
        let speed_label = label("Latency per sentence: not measured yet", &["fx-status"]);
        speed_label.set_wrap(true);
        speed_label.set_hexpand(true);
        let message = label("", &["fx-field-error"]);
        message.set_wrap(true);
        message.set_visible(false);
        let retention_labels: Vec<&str> = RETENTION.iter().map(|(l, _)| *l).collect();
        let audio_retention = gtk::DropDown::from_strings(&retention_labels);

        let ai = AiSettingsUi::new(deps.clone());
        let mic_test = MicTest::new(deps.clone());
        let page = Rc::new(Self {
            root: gtk::Box::new(gtk::Orientation::Horizontal, 0),
            header_actions: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            ai,
            mic_test,
            input_gain: RefCell::default(),
            audio_retention,
            deps,
            engine,
            stack,
            nav: RefCell::default(),
            models_box,
            installing: RefCell::default(),
            backend_box,
            backend_note,
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
            let b = gtk::Button::builder()
                .child(&label(title, &[]))
                .css_classes(["fx-settings-tab"])
                .build();
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

    fn show_message(&self, text: &str) {
        self.message.set_text(text);
        self.message.set_visible(!text.is_empty());
    }

    fn save(&self) {
        if let Err(e) = self.deps.save_settings() {
            self.show_message(&format!("Settings not saved: {e}"));
        }
    }

    // ---- Speech model ----

    fn model_section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(None, 22);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = label("Speech model", &["fx-h1"]);
        title.set_hexpand(true);
        let add = gtk::Button::with_label("Add custom GGML model…");
        add.add_css_class("fx-secondary");
        add.set_valign(gtk::Align::Center);
        add.set_tooltip_text(Some(
            "Copies a GGML .bin file into the models folder. Converting catalog models needs Python with torch.",
        ));
        head.append(&title);
        head.append(&add);
        b.append(&head);
        b.append(&self.models_box);
        b.append(&self.message);

        let bottom = gtk::Grid::builder()
            .column_homogeneous(true)
            .column_spacing(24)
            .build();
        let compute = gtk::Box::new(gtk::Orientation::Vertical, 10);
        compute.append(&label("Compute", &["fx-h2"]));
        compute.append(&self.backend_box);
        compute.append(&self.backend_note);
        let speed = gtk::Box::new(gtk::Orientation::Vertical, 10);
        speed.set_valign(gtk::Align::Start);
        speed.append(&label("Speed test", &["fx-h2"]));
        let speed_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        speed_row.add_css_class("fx-test-box");
        let run = gtk::Button::with_label("Run test");
        run.add_css_class("fx-secondary");
        run.set_valign(gtk::Align::Center);
        speed_row.append(&self.speed_label);
        speed_row.append(&run);
        speed.append(&speed_row);
        bottom.attach(&compute, 0, 0, 1, 1);
        bottom.attach(&speed, 1, 0, 1, 1);
        b.append(&bottom);

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
        outer
    }

    /// Copies a GGML file into the models folder and selects it.
    pub fn add_custom(self: &Rc<Self>, path: &std::path::Path) {
        let Some(name) = path.file_name() else { return };
        let dest = self.deps.paths.models().join(name);
        match std::fs::create_dir_all(self.deps.paths.models()).and_then(|_| std::fs::copy(path, &dest)) {
            Ok(_) => self.use_model(&name.to_string_lossy()),
            Err(e) => self.show_message(&format!("Could not copy {}: {e}", path.display())),
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
        let grid = gtk::Grid::builder()
            .column_homogeneous(true)
            .column_spacing(14)
            .row_spacing(14)
            .build();
        let mut cards = Vec::new();
        for m in models::catalog() {
            let installed = models::is_installed(&m, &self.deps.paths);
            let is_active = m.file_name == active;
            let card = card(m.name, m.publisher, m.description, &tags(&m), is_active);
            card.append(&self.model_action(m.file_name, installed, is_active, Some(m.clone())));
            cards.push(card);
        }
        for p in models::custom_models(&self.deps.paths) {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let is_active = name == active;
            let card = card(
                &name,
                "Custom",
                "A GGML model you added.",
                &[("GGML".to_string(), false)],
                is_active,
            );
            card.append(&self.model_action(&name, true, is_active, None));
            cards.push(card);
        }
        for (i, card) in cards.iter().enumerate() {
            grid.attach(card, (i % 3) as i32, (i / 3) as i32, 1, 1);
        }
        // Keep three columns even with fewer cards.
        for i in cards.len()..3 {
            grid.attach(&gtk::Box::new(gtk::Orientation::Vertical, 0), i as i32, 0, 1, 1);
        }
        self.models_box.append(&grid);
    }

    /// The bottom of a model card: Use / Download, or nothing when active.
    fn model_action(
        self: &Rc<Self>,
        file: &str,
        installed: bool,
        active: bool,
        entry: Option<models::CatalogModel>,
    ) -> gtk::Box {
        let area = gtk::Box::new(gtk::Orientation::Vertical, 6);
        if active && installed {
            return area;
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
        if active {
            b.set_tooltip_text(Some("This model is selected but not on this computer yet"));
        }
        area.append(&b);
        let file = file.to_string();
        let weak = Rc::downgrade(self);
        b.connect_clicked(move |b| {
            let Some(p) = weak.upgrade() else { return };
            match &entry {
                Some(m) if !installed => p.install(m.clone(), b),
                _ => p.use_model(&file),
            }
        });
        area
    }

    fn install(self: &Rc<Self>, m: models::CatalogModel, button: &gtk::Button) {
        if self.installing.borrow().is_some() {
            self.show_message("Another model is already being installed.");
            return;
        }
        let converting = matches!(m.source, Source::Convert { .. });
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let status = label(
            if converting {
                "Converting to GGML"
            } else {
                "Downloading"
            },
            &["fx-install-text"],
        );
        status.set_hexpand(true);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let percent = label("", &["fx-install-text"]);
        line.append(&status);
        line.append(&percent);
        let progress = gtk::ProgressBar::builder().css_classes(["thin", "ink"]).build();
        if let Some(area) = button.parent().and_downcast::<gtk::Box>() {
            area.remove(button);
            area.append(&line);
            area.append(&progress);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *self.installing.borrow_mut() = Some(Install {
            _cancel: Arc::clone(&cancel),
            bar: progress,
            status,
            percent,
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
                let Some(Install {
                    bar, status, percent, ..
                }) = guard.as_ref()
                else {
                    return;
                };
                match msg {
                    Ok(Progress::Bytes { done, total }) => {
                        if let Some(t) = total.filter(|t| *t > 0) {
                            let f = done as f64 / t as f64;
                            bar.set_fraction(f);
                            percent.set_text(&format!("{:.0}%", f * 100.0));
                        } else {
                            bar.pulse();
                            percent.set_text(&format!("{} MB", done / 1_000_000));
                        }
                    }
                    Ok(Progress::Line(line)) => {
                        bar.pulse();
                        status.set_text(&line.chars().take(90).collect::<String>());
                        status.set_tooltip_text(Some(&line));
                    }
                    Err(result) => {
                        drop(guard);
                        p.installing.borrow_mut().take();
                        match result {
                            Ok(()) => p.show_message(""),
                            Err(e) => p.show_message(&format!("Installing the model failed: {e}")),
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
        let infos = models::backends();
        let auto_uses = infos
            .iter()
            .find(|i| i.available && i.backend != Backend::Auto)
            .map(|i| i.label)
            .unwrap_or("CPU");
        let mut group: Option<gtk::ToggleButton> = None;
        for (i, info) in infos.iter().enumerate() {
            let sub = match info.backend {
                Backend::Auto => format!("Uses: {auto_uses}"),
                Backend::Cpu => cpu_name()
                    .map(|n| format!("{n} · {}", info.detail))
                    .unwrap_or_else(|| info.detail.clone()),
                _ => info.detail.clone(),
            };
            let inner = gtk::Box::new(gtk::Orientation::Vertical, 1);
            inner.set_valign(gtk::Align::Center);
            let title = gtk::Label::new(Some(info.label));
            let sub_label = gtk::Label::new(Some(&sub));
            sub_label.add_css_class("fx-seg-sub");
            sub_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            inner.append(&title);
            inner.append(&sub_label);
            let b = gtk::ToggleButton::builder()
                .child(&inner)
                .sensitive(info.available)
                .tooltip_text(info.detail.as_str())
                .build();
            b.update_property(&[gtk::accessible::Property::Label(info.label)]);
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
                    p.backend_note.set_text(backend_note(backend));
                    p.save();
                    p.engine.reset();
                    if let Some(f) = p.on_model_changed.borrow().clone() {
                        f(());
                    }
                }
            });
            self.backend_box.attach(&b, (i % 2) as i32, (i / 2) as i32, 1, 1);
        }
        self.backend_note.set_text(backend_note(current));
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

    // ---- Dictation ----

    fn dictation_section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(760), 20);
        b.append(&label("Dictation", &["fx-h1"]));
        let s = self.deps.settings();
        let grid = gtk::Grid::builder()
            .column_homogeneous(true)
            .column_spacing(20)
            .row_spacing(20)
            .build();

        let devices = input_devices();
        let mut names = vec!["Default microphone".to_string()];
        names.extend(devices.iter().map(|d| d.name.clone()));
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mic = gtk::DropDown::from_strings(&refs);
        mic.update_property(&[gtk::accessible::Property::Label("Microphone")]);
        mic.set_selected(
            devices
                .iter()
                .position(|d| d.id == s.microphone)
                .map(|i| i as u32 + 1)
                .unwrap_or(0),
        );
        grid.attach(&field("Microphone", &mic), 0, 0, 1, 1);
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
        pause.update_property(&[gtk::accessible::Property::Label(
            "Pause before text is committed, seconds",
        )]);
        let (pause_field, pause_value) = slider_field("Pause before text is committed", &pause);
        let show_pause = move |v: f64| pause_value.set_text(&format!("{v:.1} s"));
        show_pause(pause.value());
        grid.attach(&pause_field, 1, 0, 1, 1);
        let weak = Rc::downgrade(self);
        pause.connect_value_changed(move |sc| {
            show_pause(sc.value());
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().pause_ms = (sc.value() * 1000.0).round() as u32;
                p.save();
            }
        });

        let gain = gtk::Scale::with_range(gtk::Orientation::Horizontal, -20.0, 20.0, 1.0);
        gain.set_value(f64::from(s.input_gain_db));
        gain.update_property(&[gtk::accessible::Property::Label("Input volume, decibels")]);
        let (gain_field, gain_value) = slider_field("Input volume (added to the system level)", &gain);
        let show_gain = move |v: f64| gain_value.set_text(&format!("{v:+.0} dB"));
        show_gain(gain.value());
        grid.attach(&gain_field, 0, 1, 1, 1);
        *self.input_gain.borrow_mut() = Some(gain.clone());
        let weak = Rc::downgrade(self);
        gain.connect_value_changed(move |sc| {
            show_gain(sc.value());
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().input_gain_db = sc.value().round() as f32;
                p.save();
            }
        });

        let user = gtk::Entry::builder()
            .text(s.user_name.as_str())
            .placeholder_text("Used for {user} in templates")
            .build();
        grid.attach(&field("Your name", &user), 1, 1, 1, 1);
        let weak = Rc::downgrade(self);
        user.connect_changed(move |e| {
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().user_name = e.text().trim().to_string();
                p.save();
            }
        });
        b.append(&grid);

        self.mic_test.root.add_css_class("fx-test-box");
        b.append(&self.mic_test.root);

        let checks = gtk::Box::new(gtk::Orientation::Vertical, 0);
        checks.append(
            &self.check_row("Show live preview while I speak", s.show_preview, |s, v| {
                s.show_preview = v
            }),
        );
        checks.append(&self.check_row(
            "Keep the audio of dictations (needed for playback)",
            s.keep_dictation_audio,
            |s, v| s.keep_dictation_audio = v,
        ));
        b.append(&checks);

        let commands = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let title = label("Voice commands", &["fx-h2"]);
        title.set_hexpand(true);
        head.append(&title);
        head.append(&label(
            "Only trigger when the whole utterance is the command",
            &["fx-field-note"],
        ));
        commands.append(&head);
        let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.add_css_class("fx-table");
        table.set_overflow(gtk::Overflow::Hidden);
        let row = |a: &str, b: &str, classes: &[&str]| {
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            r.set_homogeneous(true);
            for c in classes {
                r.add_css_class(c);
            }
            for text in [a, b] {
                let l = label(text, &[]);
                l.set_wrap(true);
                l.set_xalign(0.0);
                r.append(&l);
            }
            r
        };
        table.append(&row("SAY", "ACTION", &["fx-table-head"]));
        let mut actions: Vec<(crate::commands::Command, Vec<String>)> = Vec::new();
        for (say, c) in &s.commands.phrases {
            match actions.iter_mut().find(|(a, _)| a == c) {
                Some((_, says)) => says.push(format!("«{say}»")),
                None => actions.push((*c, vec![format!("«{say}»")])),
            }
        }
        for (c, says) in actions {
            table.append(&row(&says.join(" or "), c.label(), &["fx-table-row", "cmd"]));
        }
        commands.append(&table);
        b.append(&commands);

        let vocab_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        vocab_box.append(&label("Vocabulary", &["fx-h2"]));
        vocab_box.append(&label(
            "Names and terms the model should know. Passed as context with each sentence.",
            &["fx-field-note"],
        ));
        let vocab = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .height_request(60)
            .css_classes(["fx-boxed"])
            .build();
        vocab.update_property(&[gtk::accessible::Property::Label("Vocabulary")]);
        vocab.buffer().set_text(&s.vocabulary);
        vocab_box.append(&vocab);
        b.append(&vocab_box);
        let weak = Rc::downgrade(self);
        vocab.buffer().connect_changed(move |buf| {
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().vocabulary =
                    buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
                p.save();
            }
        });
        let shortcut = label(
            "Shortcut: Ctrl+Space starts and stops dictation inside Fennec. A system-wide shortcut comes later.",
            &["fx-field-note"],
        );
        shortcut.set_wrap(true);
        b.append(&shortcut);
        outer
    }

    fn check_row(
        self: &Rc<Self>,
        text: &str,
        value: bool,
        set: fn(&mut crate::config::Settings, bool),
    ) -> gtk::CheckButton {
        let c = gtk::CheckButton::builder().label(text).active(value).build();
        let weak = Rc::downgrade(self);
        c.connect_toggled(move |c| {
            if let Some(p) = weak.upgrade() {
                set(&mut p.deps.settings.borrow_mut(), c.is_active());
                p.save();
            }
        });
        c
    }

    // ---- Storage ----

    fn storage_section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(760), 16);
        b.append(&label("Storage", &["fx-h1"]));
        let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.add_css_class("fx-table");
        table.set_overflow(gtk::Overflow::Hidden);
        let p = &self.deps.paths;
        for (i, (name, path)) in [
            ("Documents", p.database()),
            ("Audio", p.audio()),
            ("Speech models", p.models()),
            ("Templates & prompts", p.config_dir.clone()),
            ("Exports", p.exports()),
        ]
        .into_iter()
        .enumerate()
        {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("fx-storage-row");
            if i == 0 {
                row.add_css_class("first");
            }
            let n = label(name, &["fx-storage-name"]);
            n.set_size_request(160, -1);
            let mut shown =
                path.to_string_lossy()
                    .replacen(&std::env::var("HOME").unwrap_or_default(), "~", 1);
            if path.extension().is_none() {
                shown.push('/');
            }
            let v = label(&shown, &["fx-mono"]);
            v.set_hexpand(true);
            v.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            v.set_tooltip_text(Some(&shown));
            let open = gtk::Button::with_label("Open folder");
            open.add_css_class("fx-secondary");
            open.set_valign(gtk::Align::Center);
            open.update_property(&[gtk::accessible::Property::Label(&format!(
                "Open the {name} folder"
            ))]);
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
            table.append(&row);
        }
        b.append(&table);

        let days = self.deps.settings().delete_audio_after_days;
        let sel = RETENTION
            .iter()
            .position(|(_, d)| *d == days)
            .or_else(|| {
                days.map(|d| {
                    RETENTION
                        .iter()
                        .rposition(|(_, r)| r.is_some_and(|r| r <= d))
                        .unwrap_or(1)
                })
            })
            .unwrap_or(0);
        self.audio_retention.set_selected(sel as u32);
        self.audio_retention
            .update_property(&[gtk::accessible::Property::Label("Delete audio after")]);
        let retention = field("Delete audio after", &self.audio_retention);
        let note = label(
            "Text is kept. Playback and re-transcribing need the audio. Applied when Fennec starts.",
            &["fx-field-note"],
        );
        note.set_wrap(true);
        retention.append(&note);
        b.append(&narrow(&retention, 320));
        let weak = Rc::downgrade(self);
        self.audio_retention.connect_selected_notify(move |dd| {
            if let Some(p) = weak.upgrade() {
                let days = RETENTION.get(dd.selected() as usize).and_then(|(_, d)| *d);
                p.deps.settings.borrow_mut().delete_audio_after_days = days;
                p.save();
            }
        });
        outer
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

fn backend_note(b: Backend) -> &'static str {
    match b {
        Backend::Auto => {
            "Picks NVIDIA (CUDA) if present, then Vulkan, then CPU. Falls back to CPU if the GPU fails to start."
        }
        Backend::Cuda => "Fastest on NVIDIA GPUs. Falls back to CPU if the GPU fails to start.",
        Backend::Vulkan => "Works on AMD, Intel and NVIDIA GPUs.",
        Backend::Cpu => "Works everywhere. Slower: run the speed test to see if live dictation keeps up.",
    }
}

/// "Ryzen 5 7640U" from /proc/cpuinfo, without the vendor's boilerplate.
fn cpu_name() -> Option<String> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let name = info.lines().find_map(|l| {
        l.strip_prefix("model name")?
            .split_once(':')
            .map(|(_, v)| v.trim().to_string())
    })?;
    let name = name
        .replace("AMD ", "")
        .replace("Intel(R) Core(TM) ", "")
        .replace("(R)", "")
        .replace("(TM)", "");
    let name = name.split(" w/ ").next().unwrap_or(&name);
    let name = name.split(" with ").next().unwrap_or(name);
    let name = name.trim_end_matches(" CPU").trim_end_matches(" Processor");
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|w| !w.ends_with("-Core") && *w != "CPU" && !w.starts_with('@'))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Licence (warn style when non-commercial), size and WER.
fn tags(m: &models::CatalogModel) -> Vec<(String, bool)> {
    let licence = if m.non_commercial {
        "Non-commercial".to_string()
    } else {
        m.license.to_string()
    };
    let mut t = vec![(licence, m.non_commercial), (m.size.to_string(), false)];
    if let Some(w) = m.mean_wer {
        t.push((format!("Mean WER {w}"), false));
    }
    t
}

fn card(name: &str, publisher: &str, description: &str, tags: &[(String, bool)], active: bool) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 10);
    c.add_css_class("fx-model-card");
    c.set_hexpand(true);
    if active {
        c.add_css_class("active");
    }
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    titles.set_hexpand(true);
    let n = label(name, &["fx-model-name"]);
    n.set_wrap(true);
    titles.append(&n);
    titles.append(&label(publisher, &["fx-model-publisher"]));
    top.append(&titles);
    if active {
        let pill = label("Active", &["fx-badge", "active"]);
        pill.set_valign(gtk::Align::Start);
        top.append(&pill);
    }
    c.append(&top);
    let d = label(description, &["fx-model-desc"]);
    d.set_wrap(true);
    d.set_width_chars(20);
    d.set_max_width_chars(40);
    d.set_yalign(0.0);
    c.append(&d);
    let chips = super::wrap::wrap_box();
    for (tag, warn) in tags {
        chips.append(&label(tag, &["fx-badge", if *warn { "warn" } else { "plain" }]));
    }
    c.append(&chips);
    // The action follows the chips, as in the mockup; cards are not stretched
    // to a common bottom edge.
    c
}

/// A settings page: padded, at most `max_width` wide. Returns the page and
/// the box its content goes in.
pub(super) fn page(max_width: Option<i32>, spacing: i32) -> (gtk::Box, gtk::Box) {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.add_css_class("fx-settings-section");
    let content = gtk::Box::new(gtk::Orientation::Vertical, spacing);
    match max_width {
        Some(w) => {
            let clamp = adw::Clamp::builder()
                .maximum_size(w)
                .tightening_threshold(w)
                .child(&content)
                .build();
            // Left-aligned like the mockup, not centred.
            clamp.set_halign(gtk::Align::Start);
            outer.append(&clamp);
        }
        None => outer.append(&content),
    }
    (outer, content)
}

/// `w` at most `width` wide, on the left.
pub(super) fn narrow(w: &impl IsA<gtk::Widget>, width: i32) -> adw::Clamp {
    let clamp = adw::Clamp::builder()
        .maximum_size(width)
        .tightening_threshold(width)
        .child(w)
        .build();
    clamp.set_halign(gtk::Align::Start);
    clamp.set_size_request(width.min(240), -1);
    clamp
}

pub(super) fn field(name: &str, w: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&label(name, &["fx-field-label"]));
    b.append(w);
    b
}

/// A labelled slider with its value in mono on the right.
fn slider_field(name: &str, scale: &gtk::Scale) -> (gtk::Box, gtk::Label) {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let l = label(name, &["fx-field-label"]);
    l.set_hexpand(true);
    let value = label("", &["fx-mono-value"]);
    head.append(&l);
    head.append(&value);
    b.append(&head);
    scale.set_draw_value(false);
    b.append(scale);
    (b, value)
}
