//! Settings: speech model and compute, dictation, AI, privacy, storage. Changes apply to
//! the shared settings and are saved to `settings.toml` right away.
//!
//! Laid out like the other screens: the sections are listed in the sidebar
//! (each with a line on its current state), the section fills the middle,
//! and a panel on the right holds what goes with it (compute and the speed
//! test, the microphone test, where AI text goes, disk use).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk::prelude::*;
use gtk::{gio, glib};

use super::engine::EngineHolder;
use super::mic_test::MicTest;
use super::settings_ai::AiSettingsUi;
use super::{Deps, Handler, label};
use crate::audio::capture::input_devices;
use crate::config::Backend;
use crate::models::{self, Progress, Source};
use crate::store::Store;
use crate::worker::Priority;

/// "Delete audio after" choices: label and days (`None`: never).
const RETENTION: [(&str, Option<u32>); 4] = [
    ("Never", None),
    ("30 days", Some(30)),
    ("90 days", Some(90)),
    ("1 year", Some(365)),
];

/// The sections: id and title, in sidebar order.
const SECTIONS: [(&str, &str); 6] = [
    ("model", "Speech model"),
    ("dictation", "Dictation"),
    ("ai", "AI providers"),
    ("ai-defaults", "AI defaults"),
    ("privacy", "Privacy"),
    ("storage", "Storage"),
];

/// A model download or conversion in progress, shown in its row.
struct Install {
    cancel: Arc<AtomicBool>,
    bar: gtk::ProgressBar,
    status: gtk::Label,
    percent: gtk::Label,
}

const PUNCT_READY: &str = "Uses a Danish punctuation model on this computer; your words are never changed.";

/// The speed test's result, kept for the test accessor.
struct SpeedTest {
    factor: gtk::Label,
    unit: gtk::Label,
    detail: gtk::Label,
    verdict: gtk::Box,
    verdict_text: gtk::Label,
    run: gtk::Button,
}

/// Disk use per kind, for the Storage panel: models, audio, documents, other.
type DiskUse = Rc<RefCell<[u64; 4]>>;

pub struct SettingsPage {
    pub root: gtk::Box,
    /// Buttons for the window header while this screen shows.
    pub header_actions: gtk::Box,
    /// The section list, shown in the sidebar while Settings is open.
    pub nav_panel: gtk::Box,
    deps: Deps,
    engine: Rc<EngineHolder>,
    stack: gtk::Stack,
    inspector: gtk::Stack,
    inspector_scroll: gtk::ScrolledWindow,
    /// Section id, its sidebar button and the line under its title.
    nav: RefCell<Vec<(String, gtk::Button, gtk::Label)>>,
    models_box: gtk::Box,
    installing: RefCell<Option<Install>>,
    backend_box: gtk::Box,
    backend_note: gtk::Label,
    speed: SpeedTest,
    message: gtk::Label,
    vocab_chips: gtk::Box,
    /// Microphones as listed when Settings opened: id and name.
    microphones: RefCell<Vec<(String, String)>>,
    local_projects: gtk::Label,
    /// How many projects are local-only (read with the project list).
    local_count: std::cell::Cell<usize>,
    storage_sizes: RefCell<Vec<(PathBuf, gtk::Label)>>,
    disk_use: DiskUse,
    disk_total: gtk::Label,
    disk_bar: gtk::DrawingArea,
    disk_legend: Vec<gtk::Label>,
    pub ai: Rc<AiSettingsUi>,
    pub mic_test: Rc<MicTest>,
    pub input_gain: RefCell<Option<gtk::Scale>>,
    /// "Delete audio after" (Storage).
    pub audio_retention: gtk::DropDown,
    on_model_changed: Handler<()>,
    on_section_changed: Handler<()>,
}

impl SettingsPage {
    pub fn new(deps: Deps, engine: Rc<EngineHolder>) -> Rc<Self> {
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        let inspector = gtk::Stack::new();
        inspector.set_vhomogeneous(false);
        let inspector_scroll = gtk::ScrolledWindow::builder()
            .child(&inspector)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["fx-inspector", "fx-settings-inspector"])
            .build();
        inspector_scroll.set_size_request(300, -1);

        let nav_panel = gtk::Box::new(gtk::Orientation::Vertical, 2);
        nav_panel.add_css_class("fx-settings-list");
        let title = label("SETTINGS", &["fx-section-title"]);
        title.set_margin_bottom(6);
        nav_panel.append(&title);

        let backend_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        backend_box.add_css_class("fx-set-box");
        backend_box.set_overflow(gtk::Overflow::Hidden);
        backend_box.update_property(&[gtk::accessible::Property::Label("Compute")]);
        let backend_note = label("", &["fx-field-note"]);
        backend_note.set_wrap(true);
        let message = label("", &["fx-field-error"]);
        message.set_wrap(true);
        message.set_visible(false);
        let retention_labels: Vec<&str> = RETENTION.iter().map(|(l, _)| *l).collect();
        let audio_retention = gtk::DropDown::from_strings(&retention_labels);

        let speed = SpeedTest {
            factor: label("–", &["fx-bignum"]),
            unit: label("real time", &["fx-field-note"]),
            detail: label(
                "Not measured yet. Times a Danish sentence with the selected model.",
                &["fx-field-note"],
            ),
            verdict: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            verdict_text: label("", &[]),
            run: gtk::Button::with_label("Run test"),
        };
        speed.detail.set_wrap(true);
        speed.unit.set_visible(false);
        speed.verdict_text.set_wrap(true);
        speed.verdict.set_visible(false);

        let disk_legend = (0..4).map(|_| label("", &["fx-mono"])).collect();
        let ai = AiSettingsUi::new(deps.clone());
        let mic_test = MicTest::new(deps.clone());
        let page = Rc::new(Self {
            root: gtk::Box::new(gtk::Orientation::Horizontal, 0),
            header_actions: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            nav_panel,
            ai,
            mic_test,
            input_gain: RefCell::default(),
            audio_retention,
            deps,
            engine,
            stack,
            inspector,
            inspector_scroll,
            nav: RefCell::default(),
            models_box: gtk::Box::new(gtk::Orientation::Vertical, 28),
            installing: RefCell::default(),
            backend_box,
            backend_note,
            speed,
            message,
            vocab_chips: super::wrap::wrap_box(),
            microphones: RefCell::default(),
            local_projects: label("", &["fx-field-note"]),
            local_count: std::cell::Cell::new(0),
            storage_sizes: RefCell::default(),
            disk_use: Rc::default(),
            disk_total: label("…", &["fx-bignum"]),
            disk_bar: gtk::DrawingArea::new(),
            disk_legend,
            on_model_changed: RefCell::default(),
            on_section_changed: RefCell::default(),
        });
        let sections = [
            page.model_section(),
            page.dictation_section(),
            page.ai.providers_section(),
            page.ai.defaults_section(),
            page.ai.privacy_section(),
            page.storage_section(),
        ];
        let mut nav = Vec::new();
        for ((id, title), w) in SECTIONS.into_iter().zip(sections) {
            let scroll = gtk::ScrolledWindow::builder()
                .child(&w)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build();
            page.stack.add_named(&scroll, Some(id));
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.append(&label(title, &["fx-settings-item-name"]));
            let sub = label("", &["fx-stats"]);
            sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&sub);
            let b = gtk::Button::builder()
                .child(&text)
                .css_classes(["fx-settings-item"])
                .build();
            page.nav_panel.append(&b);
            nav.push((id.to_string(), b, sub));
        }
        *page.nav.borrow_mut() = nav;
        for (id, b, _) in page.nav.borrow().iter() {
            let weak = Rc::downgrade(&page);
            let id = id.clone();
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.show_section(&id);
                }
            });
        }
        page.inspector.add_named(&page.model_panel(), Some("model"));
        page.inspector
            .add_named(&page.dictation_panel(), Some("dictation"));
        page.inspector.add_named(&page.ai_panel(), Some("ai"));
        page.inspector.add_named(&page.storage_panel(), Some("storage"));
        page.root.append(&page.stack);
        page.root.append(&page.inspector_scroll);

        // The lines under the section names follow every change. The project
        // list is read from the database only when it can have changed.
        let weak = Rc::downgrade(&page);
        page.ai.connect_changed(move |()| {
            if let Some(p) = weak.upgrade() {
                p.refresh_nav();
            }
        });
        let weak = Rc::downgrade(&page);
        page.ai.connect_local_only_changed(move |()| {
            if let Some(p) = weak.upgrade() {
                p.refresh_local_projects();
                p.refresh_nav();
            }
        });
        let weak = Rc::downgrade(&page);
        page.root.connect_map(move |_| {
            if let Some(p) = weak.upgrade() {
                p.refresh_local_projects();
                p.refresh_nav();
            }
        });
        page.render_models();
        page.render_backends();
        page.show_section("model");
        page
    }

    /// AI settings changed (on/off, providers, default).
    pub fn connect_ai_changed(self: &Rc<Self>, f: impl Fn(()) + 'static) {
        let weak = Rc::downgrade(self);
        self.ai.connect_changed(move |()| {
            if let Some(p) = weak.upgrade() {
                p.refresh_nav();
            }
            f(());
        });
    }

    pub fn connect_model_changed(&self, f: impl Fn(()) + 'static) {
        *self.on_model_changed.borrow_mut() = Some(Rc::new(f));
    }

    /// Another section was opened (the header shows its name).
    pub fn connect_section_changed(&self, f: impl Fn(()) + 'static) {
        *self.on_section_changed.borrow_mut() = Some(Rc::new(f));
    }

    pub fn show_section(self: &Rc<Self>, id: &str) {
        self.stack.set_visible_child_name(id);
        for (nid, b, _) in self.nav.borrow().iter() {
            if nid == id {
                b.add_css_class("active");
            } else {
                b.remove_css_class("active");
            }
        }
        // AI defaults and Privacy have nothing to put beside them.
        let has_panel = self.inspector.child_by_name(id).is_some();
        self.inspector_scroll.set_visible(has_panel);
        if has_panel {
            self.inspector.set_visible_child_name(id);
        }
        match id {
            "ai" => self.refresh_local_projects(),
            "storage" => self.measure_storage(),
            _ => {}
        }
        if let Some(f) = self.on_section_changed.borrow().clone() {
            f(());
        }
    }

    /// The open section's title, for the header.
    pub fn section_title(&self) -> String {
        let id = self.stack.visible_child_name().unwrap_or_default();
        SECTIONS
            .iter()
            .find(|(s, _)| *s == id.as_str())
            .map_or("Settings", |(_, t)| t)
            .to_string()
    }

    /// The line under each section name in the sidebar, as listed (tests).
    pub fn nav_summaries(&self) -> Vec<(String, String)> {
        self.nav
            .borrow()
            .iter()
            .map(|(id, _, sub)| (id.clone(), sub.text().to_string()))
            .collect()
    }

    /// Writes each section's current state under its name in the sidebar.
    fn refresh_nav(&self) {
        let s = self.deps.settings();
        let mic = self
            .microphones
            .borrow()
            .iter()
            .find(|(id, _)| !s.microphone.is_empty() && *id == s.microphone)
            .map_or_else(|| "Default microphone".to_string(), |(_, name)| name.clone());
        let ai = &s.ai;
        let providers = plural(ai.providers.len(), "provider");
        let ai_line = if ai.enabled {
            format!("On · {providers}")
        } else {
            format!("Off · {providers}")
        };
        let defaults = match ai.active() {
            None => "No providers yet".to_string(),
            Some(p) if ai.jobs.is_empty() => format!("{} for every job", p.name),
            Some(p) => format!("{} by default", p.name),
        };
        let privacy = match self.local_count.get() {
            0 => "No local-only projects".to_string(),
            n => format!("{} local-only", plural(n, "project")),
        };
        let storage = match s.delete_audio_after_days {
            None => "Audio kept until you delete it".to_string(),
            Some(d) => format!(
                "Audio kept for {}",
                RETENTION
                    .iter()
                    .find(|(_, r)| *r == Some(d))
                    .map_or_else(|| plural(d as usize, "day"), |(l, _)| l.to_string())
            ),
        };
        for (id, _, sub) in self.nav.borrow().iter() {
            sub.set_text(&match id.as_str() {
                "model" => model_label(&s),
                "dictation" => format!("{mic} · {:.1} s pause", f64::from(s.pause_ms) / 1000.0),
                "ai" => ai_line.clone(),
                "ai-defaults" => defaults.clone(),
                "privacy" => privacy.clone(),
                _ => storage.clone(),
            });
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
        self.refresh_nav();
    }

    // ---- Speech model ----

    fn model_section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(760), 28);
        let add = gtk::Button::new();
        add.set_child(Some(&{
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&gtk::Image::from_icon_name("fennec-add-symbolic"));
            row.append(&gtk::Label::new(Some("Add GGML model…")));
            row
        }));
        add.add_css_class("fx-secondary");
        add.set_valign(gtk::Align::Start);
        add.set_tooltip_text(Some(
            "Copies a GGML .bin file into the models folder. Converting catalog models needs Python with torch.",
        ));
        b.append(&title_block(
            "Speech model",
            "The model that turns speech into text. All of them run on this computer.",
            Some(&add),
        ));
        b.append(&self.message);
        b.append(&self.models_box);

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

    /// Compute and the speed test, beside the model list.
    fn model_panel(self: &Rc<Self>) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 24);
        let compute = panel_group("COMPUTE");
        compute.append(&self.backend_box);
        compute.append(&self.backend_note);
        b.append(&compute);

        let speed = panel_group("SPEED TEST");
        let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
        card.add_css_class("fx-set-box");
        card.add_css_class("padded");
        let figure = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        self.speed.unit.set_valign(gtk::Align::BaselineFill);
        self.speed.factor.set_valign(gtk::Align::BaselineFill);
        figure.append(&self.speed.factor);
        figure.append(&self.speed.unit);
        card.append(&figure);
        card.append(&self.speed.detail);
        self.speed.verdict.add_css_class("fx-status-line");
        let dot = gtk::Box::builder()
            .css_classes(["fx-dot"])
            .valign(gtk::Align::Center)
            .build();
        self.speed.verdict.append(&dot);
        self.speed.verdict.append(&self.speed.verdict_text);
        card.append(&self.speed.verdict);
        self.speed.run.add_css_class("fx-secondary");
        card.append(&self.speed.run);
        speed.append(&card);
        b.append(&speed);

        let wer = label(
            "WER is the share of words a model gets wrong on Danish test speech. Lower is better.",
            &["fx-field-note"],
        );
        wer.set_wrap(true);
        b.append(&wer);

        let weak = Rc::downgrade(self);
        self.speed.run.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.run_speed_test();
            }
        });
        b
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

    /// Two lists: the models on this computer (the one in use first), then
    /// the catalog models that can be downloaded.
    fn render_models(self: &Rc<Self>) {
        while let Some(c) = self.models_box.first_child() {
            self.models_box.remove(&c);
        }
        let active = self.deps.settings.borrow().model.clone();
        let mut here = Vec::new();
        let mut away = Vec::new();
        for m in models::catalog() {
            let installed = models::is_installed(&m, &self.deps.paths);
            let is_active = m.file_name == active;
            let (arch, description) = split_description(m.description);
            let meta = match arch {
                Some(a) => format!("{} · {a}", m.publisher),
                None => m.publisher.to_string(),
            };
            let row = model_row(
                m.name,
                &meta,
                description,
                &tags(&m),
                m.mean_wer,
                is_active,
                installed,
            );
            row.append(&self.model_action(m.file_name, installed, is_active, Some(m.clone())));
            if installed {
                here.push((is_active, row));
            } else {
                away.push((is_active, row));
            }
        }
        for p in models::custom_models(&self.deps.paths) {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let is_active = name == active;
            let row = model_row(
                &name,
                "Custom · added from a file",
                "A GGML model you added.",
                &[("GGML".to_string(), false)],
                None,
                is_active,
                true,
            );
            row.append(&self.model_action(&name, true, is_active, None));
            here.push((is_active, row));
        }
        // The model in use leads its list.
        here.sort_by_key(|(active, _)| !active);
        for (title, rows) in [("On this computer", here), ("Available to download", away)] {
            if rows.is_empty() {
                continue;
            }
            let (group, list) = group(title, Some(&rows.len().to_string()));
            for (_, row) in rows {
                list.append(&row);
            }
            self.models_box.append(&group);
        }
    }

    /// The right of a model row: Use / Download, or nothing for the model in use.
    fn model_action(
        self: &Rc<Self>,
        file: &str,
        installed: bool,
        active: bool,
        entry: Option<models::CatalogModel>,
    ) -> gtk::Box {
        let area = gtk::Box::new(gtk::Orientation::Vertical, 6);
        area.add_css_class("fx-model-action");
        area.set_size_request(180, -1);
        area.set_valign(gtk::Align::Start);
        if active && installed {
            return area;
        }
        let b = if installed {
            gtk::Button::with_label("Use this model")
        } else {
            let size = entry.as_ref().map(download_size).unwrap_or_default();
            super::icon_text_button("fennec-download-symbolic", &format!("Download · {size}"), &[])
        };
        b.add_css_class("fx-secondary");
        b.set_halign(gtk::Align::End);
        // Screen readers hear which model the button is for.
        let name = entry.as_ref().map_or(file, |e| e.name);
        b.update_property(&[gtk::accessible::Property::Label(&if installed {
            format!("Use {name}")
        } else {
            format!("Download {name}")
        })]);
        if !installed && matches!(entry.as_ref().map(|e| &e.source), Some(Source::Convert { .. })) {
            b.set_tooltip_text(Some(
                "Downloads the model and converts it to GGML (needs Python with torch)",
            ));
        }
        if active && !installed {
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
        let percent = label("", &["fx-install-text", "fx-mono"]);
        line.append(&status);
        line.append(&percent);
        let progress = gtk::ProgressBar::builder().css_classes(["thin"]).build();
        let cancel_button = gtk::Button::with_label("Cancel");
        cancel_button.add_css_class("fx-quiet");
        cancel_button.add_css_class("small");
        cancel_button.set_halign(gtk::Align::End);
        if let Some(area) = button.parent().and_downcast::<gtk::Box>() {
            area.remove(button);
            area.append(&line);
            area.append(&progress);
            area.append(&cancel_button);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *self.installing.borrow_mut() = Some(Install {
            cancel: Arc::clone(&cancel),
            bar: progress,
            status,
            percent,
        });
        let weak = Rc::downgrade(self);
        cancel_button.connect_clicked(move |b| {
            if let Some(p) = weak.upgrade()
                && let Some(i) = p.installing.borrow().as_ref()
            {
                i.cancel.store(true, Ordering::Relaxed);
                i.status.set_text("Cancelling…");
                b.set_sensitive(false);
            }
        });
        let (tx, rx) = async_channel::unbounded::<Result<Progress, Result<(), String>>>();
        let paths = self.deps.paths.clone();
        let punctuate = self.deps.settings().punctuate;
        std::thread::spawn(move || {
            let send = |p| {
                let _ = tx.send_blocking(Ok(p));
            };
            // The voice detector (and the punctuation model) come with the first model.
            send(Progress::Line("Fetching the voice detector".into()));
            if let Err(e) = models::ensure_vad(&paths, &cancel, send) {
                tracing::warn!("could not fetch the voice detector: {e}");
            }
            if punctuate && !cancel.load(Ordering::Relaxed) {
                send(Progress::Line("Fetching the punctuation model".into()));
                if let Err(e) = models::ensure_punctuation(&paths, &cancel, send) {
                    tracing::warn!("could not fetch the punctuation model: {e}");
                }
            }
            // A cancel before the model starts must not touch its partial file.
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send_blocking(Err(Err("cancelled".into())));
                return;
            }
            send(Progress::Line(
                if matches!(m.source, Source::Convert { .. }) {
                    "Converting to GGML"
                } else {
                    "Downloading"
                }
                .into(),
            ));
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
                    cancel,
                    bar,
                    status,
                    percent,
                }) = guard.as_ref()
                else {
                    return;
                };
                match msg {
                    Ok(Progress::Bytes { done, total }) => {
                        if let Some(t) = total.filter(|t| *t > 0) {
                            let f = done as f64 / t as f64;
                            bar.set_fraction(f);
                            percent.set_text(&format!("{:.0} %", f * 100.0));
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
                        let cancelled = cancel.load(Ordering::Relaxed);
                        drop(guard);
                        p.installing.borrow_mut().take();
                        match result {
                            Err(e) if !cancelled => {
                                p.show_message(&format!("Installing the model failed: {e}"))
                            }
                            _ => p.show_message(""),
                        }
                        p.render_models();
                        return;
                    }
                }
            }
        });
    }

    /// Compute choices as a list of options, the current one marked.
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
        for info in &infos {
            let sub = match info.backend {
                Backend::Auto => format!("Uses {auto_uses} now"),
                Backend::Cpu => cpu_name()
                    .map(|n| format!("{n} · {}", info.detail))
                    .unwrap_or_else(|| info.detail.clone()),
                _ => info.detail.clone(),
            };
            let inner = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let radio = gtk::Box::builder()
                .css_classes(["fx-radio"])
                .valign(gtk::Align::Center)
                .build();
            inner.append(&radio);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text.set_valign(gtk::Align::Center);
            let title = label(info.label, &["fx-option-title"]);
            let sub_label = label(&sub, &["fx-seg-sub"]);
            sub_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&title);
            text.append(&sub_label);
            inner.append(&text);
            let b = gtk::ToggleButton::builder()
                .child(&inner)
                .sensitive(info.available)
                .tooltip_text(info.detail.as_str())
                .css_classes(["fx-option"])
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
            self.backend_box.append(&b);
        }
        self.backend_note.set_text(backend_note(current));
    }

    pub fn run_speed_test(self: &Rc<Self>) {
        let sp = &self.speed;
        sp.factor.set_text("…");
        sp.unit.set_visible(false);
        sp.verdict.set_visible(false);
        sp.run.set_sensitive(false);
        sp.detail
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
            let sp = &p.speed;
            sp.run.set_sensitive(true);
            match result {
                Ok(r) => {
                    let rtf = r.real_time_factor();
                    let (state, verdict) = if rtf < 0.5 {
                        ("ok", "Fast enough for live dictation")
                    } else if rtf < 1.0 {
                        (
                            "idle",
                            "Usable for dictation; expect a short wait after each pause",
                        )
                    } else {
                        (
                            "bad",
                            "Too slow for live dictation. Try a GPU backend, a smaller or q4 model, or use file transcription.",
                        )
                    };
                    sp.factor.set_text(&format!("{rtf:.2}×"));
                    sp.unit.set_visible(true);
                    sp.detail.set_text(&format!(
                        "{:.1} s of Danish speech took {:.1} s with {}.",
                        r.audio_secs,
                        r.elapsed_secs,
                        model_label(&p.deps.settings())
                            .split(" · ")
                            .next()
                            .unwrap_or_default()
                    ));
                    sp.verdict.set_css_classes(&["fx-status-line", state]);
                    sp.verdict_text.set_text(verdict);
                    sp.verdict.set_visible(true);
                    sp.run.set_label("Run again");
                }
                Err(e) => {
                    sp.factor.set_text("–");
                    sp.detail.set_text(&format!("Speed test failed: {e}"));
                }
            }
        });
    }

    pub fn settings(&self) -> crate::config::Settings {
        self.deps.settings()
    }

    /// The speed test's result as one line (tests).
    pub fn speed_text(&self) -> String {
        let sp = &self.speed;
        if sp.unit.get_visible() {
            format!("{} real time · {}", sp.factor.text(), sp.detail.text())
        } else {
            sp.detail.text().to_string()
        }
    }

    // ---- Dictation ----

    fn dictation_section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(760), 28);
        b.append(&title_block(
            "Dictation",
            "How Fennec listens, when it writes, and the words it should know.",
            None,
        ));
        let s = self.deps.settings();

        let (input, rows) = group("Input", None);
        let devices = input_devices();
        *self.microphones.borrow_mut() = devices.iter().map(|d| (d.id.clone(), d.name.clone())).collect();
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
        mic.set_size_request(260, -1);
        rows.append(&row("Microphone", None, &mic));
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

        let gain = gtk::Scale::with_range(gtk::Orientation::Horizontal, -20.0, 20.0, 1.0);
        gain.set_value(f64::from(s.input_gain_db));
        gain.update_property(&[gtk::accessible::Property::Label("Input volume, decibels")]);
        let (gain_box, gain_value) = slider(&gain);
        let show_gain = move |v: f64| gain_value.set_text(&format!("{v:+.0} dB"));
        show_gain(gain.value());
        rows.append(&row(
            "Input volume",
            Some("Added to the system level."),
            &gain_box,
        ));
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
            .placeholder_text("e.g. Jens Hansen")
            .width_request(260)
            .valign(gtk::Align::Center)
            .build();
        user.update_property(&[gtk::accessible::Property::Label("Your name")]);
        rows.append(&row("Your name", Some("Fills {user} in templates."), &user));
        let weak = Rc::downgrade(self);
        user.connect_changed(move |e| {
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().user_name = e.text().trim().to_string();
                p.save();
            }
        });
        b.append(&input);

        let (text_group, rows) = group("Text", None);
        let pause = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.3, 2.0, 0.1);
        pause.set_value(f64::from(s.pause_ms) / 1000.0);
        pause.update_property(&[gtk::accessible::Property::Label(
            "Pause before text is written, seconds",
        )]);
        let (pause_box, pause_value) = slider(&pause);
        let show_pause = move |v: f64| pause_value.set_text(&format!("{v:.1} s"));
        show_pause(pause.value());
        rows.append(&row(
            "Pause before text is written",
            Some("Shorter feels quicker; longer keeps sentences together."),
            &pause_box,
        ));
        let weak = Rc::downgrade(self);
        pause.connect_value_changed(move |sc| {
            show_pause(sc.value());
            if let Some(p) = weak.upgrade() {
                p.deps.settings.borrow_mut().pause_ms = (sc.value() * 1000.0).round() as u32;
                p.save();
            }
        });

        let dir = self.deps.paths.models().join(crate::punctuation::DIR);
        let (punct_row, punctuate, note) = self.switch_row(
            "Add commas, full stops and capitals",
            Some(if crate::punctuation::installed(&dir) {
                PUNCT_READY
            } else {
                "Uses a Danish punctuation model (440 MB), downloaded when this is turned on."
            }),
            s.punctuate,
            |s, v| s.punctuate = v,
        );
        rows.append(&punct_row);
        if let Some(note) = note {
            let weak = Rc::downgrade(self);
            punctuate.connect_active_notify(move |sw| {
                if let Some(p) = weak.upgrade()
                    && sw.is_active()
                {
                    p.download_punctuation(&note);
                }
            });
        }
        rows.append(
            &self
                .switch_row(
                    "Show a live preview while I speak",
                    None,
                    s.show_preview,
                    |s, v| s.show_preview = v,
                )
                .0,
        );
        rows.append(
            &self
                .switch_row(
                    "Keep the audio of dictations",
                    Some("Needed for playback and re-transcribing."),
                    s.keep_dictation_audio,
                    |s, v| s.keep_dictation_audio = v,
                )
                .0,
        );
        b.append(&text_group);

        let mut actions: Vec<(crate::commands::Command, Vec<String>)> = Vec::new();
        for (say, c) in &s.commands.phrases {
            match actions.iter_mut().find(|(a, _)| a == c) {
                Some((_, says)) => says.push(format!("«{say}»")),
                None => actions.push((*c, vec![format!("«{say}»")])),
            }
        }
        let (commands, table) = group("Voice commands", Some(&actions.len().to_string()));
        let note = label(
            "A command only works when it is all you say: “…og så punktum” is written as words.",
            &["fx-field-note"],
        );
        note.set_wrap(true);
        commands.insert_child_after(&note, commands.first_child().as_ref());
        let table_row = |a: &str, b: &str, classes: &[&str]| {
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            for c in classes {
                r.add_css_class(c);
            }
            let say = label(a, &["fx-say"]);
            say.set_wrap(true);
            say.set_xalign(0.0);
            say.set_size_request(260, -1);
            let action = label(b, &[]);
            action.set_wrap(true);
            action.set_xalign(0.0);
            action.set_hexpand(true);
            r.append(&say);
            r.append(&action);
            r
        };
        table.append(&table_row("SAY", "ACTION", &["fx-table-head"]));
        for (c, says) in actions {
            table.append(&table_row(
                &says.join(" or "),
                c.label(),
                &["fx-table-row", "cmd"],
            ));
        }
        b.append(&commands);

        let (vocab, list) = group("Vocabulary", None);
        list.add_css_class("padded");
        list.set_spacing(12);
        let vnote = label(
            "Names and terms spelled your way. Near-misses in the text are corrected to them, and \
             Fennec offers to add a word when you correct it.",
            &["fx-field-note"],
        );
        vnote.set_wrap(true);
        list.append(&vnote);
        list.append(&self.vocab_chips);
        let add = gtk::Entry::builder()
            .placeholder_text("Add a word, or “heard -> wanted”…")
            .build();
        add.update_property(&[gtk::accessible::Property::Label("Add a word to the vocabulary")]);
        list.append(&add);
        let weak = Rc::downgrade(self);
        add.connect_activate(move |e| {
            if let Some(p) = weak.upgrade() {
                p.add_vocabulary(e.text().trim());
                e.set_text("");
            }
        });
        b.append(&vocab);
        self.render_vocabulary();
        outer
    }

    /// The microphone test and the shortcut, beside the dictation settings.
    fn dictation_panel(self: &Rc<Self>) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 24);
        let test = panel_group("MICROPHONE TEST");
        self.mic_test.root.add_css_class("fx-set-box");
        self.mic_test.root.add_css_class("padded");
        test.append(&self.mic_test.root);
        let note = label(
            "Speak a sentence at your normal volume. The bar should reach about halfway.",
            &["fx-field-note"],
        );
        note.set_wrap(true);
        test.append(&note);
        b.append(&test);

        let shortcut = panel_group("SHORTCUT");
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("fx-set-box");
        card.add_css_class("padded");
        let keys = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        keys.append(&label("Ctrl", &["fx-kbd"]));
        keys.append(&label("+", &["fx-field-note"]));
        keys.append(&label("Space", &["fx-kbd"]));
        card.append(&keys);
        let note = label(
            "Starts and stops dictation inside Fennec. A system-wide shortcut comes later.",
            &["fx-field-note"],
        );
        note.set_wrap(true);
        card.append(&note);
        shortcut.append(&card);
        b.append(&shortcut);
        b
    }

    /// The vocabulary's entries (as written in the settings), in order.
    pub fn vocabulary_entries(&self) -> Vec<String> {
        split_vocabulary(&self.deps.settings().vocabulary)
    }

    /// Adds what was typed; "a, b" adds two entries. Entries already there
    /// (in any case, "Ærø" or "ærø") are left out.
    pub fn add_vocabulary(self: &Rc<Self>, text: &str) {
        let mut entries = self.vocabulary_entries();
        let before = entries.len();
        for entry in split_vocabulary(text) {
            if !entries.iter().any(|e| e.to_lowercase() == entry.to_lowercase()) {
                entries.push(entry);
            }
        }
        if entries.len() != before {
            self.set_vocabulary(&entries);
        }
    }

    pub fn remove_vocabulary(self: &Rc<Self>, entry: &str) {
        let mut entries = self.vocabulary_entries();
        entries.retain(|e| e != entry);
        self.set_vocabulary(&entries);
    }

    fn set_vocabulary(self: &Rc<Self>, entries: &[String]) {
        self.deps.settings.borrow_mut().vocabulary = entries.join(", ");
        self.save();
        self.render_vocabulary();
    }

    /// One removable chip per vocabulary entry.
    fn render_vocabulary(self: &Rc<Self>) {
        while let Some(c) = self.vocab_chips.first_child() {
            self.vocab_chips.remove(&c);
        }
        let entries = self.vocabulary_entries();
        if entries.is_empty() {
            self.vocab_chips
                .append(&label("No words yet.", &["fx-field-note"]));
        }
        for entry in entries {
            let chip = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            chip.add_css_class("fx-vocab-chip");
            chip.append(&label(&entry, &[]));
            let remove = super::icon_button("window-close-symbolic", &format!("Remove {entry}"), &[]);
            remove.add_css_class("fx-vocab-remove");
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.remove_vocabulary(&entry);
                }
            });
            chip.append(&remove);
            self.vocab_chips.append(&chip);
        }
    }

    /// Fetches the punctuation model if it is missing, reporting in `note`.
    fn download_punctuation(self: &Rc<Self>, note: &gtk::Label) {
        if crate::punctuation::installed(&self.deps.paths.models().join(crate::punctuation::DIR)) {
            note.set_text(PUNCT_READY);
            return;
        }
        let (tx, rx) = async_channel::unbounded::<Result<Progress, Result<(), String>>>();
        let paths = self.deps.paths.clone();
        std::thread::spawn(move || {
            let send = |p| {
                let _ = tx.send_blocking(Ok(p));
            };
            let result = models::ensure_punctuation(&paths, &AtomicBool::new(false), send);
            let _ = tx.send_blocking(Err(result));
        });
        let note = note.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    Ok(Progress::Bytes { done, total: Some(t) }) if t > 0 => note.set_text(&format!(
                        "Downloading the punctuation model… {} %",
                        done * 100 / t
                    )),
                    Ok(_) => {}
                    Err(Ok(())) => note.set_text(PUNCT_READY),
                    Err(Err(e)) => note.set_text(&format!("Could not download the punctuation model: {e}")),
                }
            }
        });
    }

    /// A row with a switch; returns the row, the switch and the note label.
    fn switch_row(
        self: &Rc<Self>,
        title: &str,
        note: Option<&str>,
        value: bool,
        set: fn(&mut crate::config::Settings, bool),
    ) -> (gtk::Box, gtk::Switch, Option<gtk::Label>) {
        let sw = gtk::Switch::builder()
            .active(value)
            .valign(gtk::Align::Center)
            .build();
        sw.update_property(&[gtk::accessible::Property::Label(title)]);
        let r = row(title, note, &sw);
        let note = note.and_then(|_| row_note(&r));
        let weak = Rc::downgrade(self);
        sw.connect_active_notify(move |sw| {
            if let Some(p) = weak.upgrade() {
                set(&mut p.deps.settings.borrow_mut(), sw.is_active());
                p.save();
            }
        });
        (r, sw, note)
    }

    // ---- AI providers ----

    /// Where AI text goes, and which projects never use the cloud.
    fn ai_panel(self: &Rc<Self>) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 24);
        let places = panel_group("WHERE TEXT GOES");
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("fx-set-box");
        for (badge, kind, text) in [
            ("THIS COMPUTER", "local", "Never leaves the machine."),
            ("NETWORK", "network", "A server you run on your own network."),
            (
                "CLOUD",
                "cloud",
                "Sent to the provider. Fennec asks before a document goes there the first time.",
            ),
        ] {
            let r = gtk::Box::new(gtk::Orientation::Vertical, 6);
            r.add_css_class("fx-legend-row");
            let badge = label(badge, &["fx-badge", kind]);
            badge.set_halign(gtk::Align::Start);
            r.append(&badge);
            let t = label(text, &["fx-field-note"]);
            t.set_wrap(true);
            r.append(&t);
            card.append(&r);
        }
        places.append(&card);
        b.append(&places);

        let local = panel_group("LOCAL-ONLY PROJECTS");
        self.local_projects.set_wrap(true);
        local.append(&self.local_projects);
        let link = gtk::Button::with_label("Change in Privacy");
        link.add_css_class("fx-link");
        link.set_halign(gtk::Align::Start);
        let weak = Rc::downgrade(self);
        link.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.show_section("privacy");
            }
        });
        local.append(&link);
        b.append(&local);
        b
    }

    fn refresh_local_projects(&self) {
        let names: Vec<String> = match Store::open(&self.deps.paths.database()).and_then(|s| s.projects()) {
            Ok(ps) => ps.into_iter().filter(|p| p.local_only).map(|p| p.name).collect(),
            Err(e) => {
                // Keep the last count; claiming "none" would be a wrong privacy statement.
                self.local_projects
                    .set_text(&format!("Could not read the projects: {e}"));
                return;
            }
        };
        self.local_count.set(names.len());
        self.local_projects.set_text(&match names.as_slice() {
            [] => "None. Mark a project local-only in Privacy to keep it off cloud providers.".to_string(),
            [one] => format!("{one} never uses cloud providers, whatever is set here."),
            many => format!(
                "{} never use cloud providers, whatever is set here.",
                many.join(", ")
            ),
        });
    }

    // ---- Storage ----

    fn storage_section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(760), 28);
        b.append(&title_block(
            "Storage",
            "Everything Fennec keeps is in plain folders on this computer.",
            None,
        ));
        let (folders, rows) = group("Folders", None);
        let p = &self.deps.paths;
        let mut sizes = Vec::new();
        for (name, path) in [
            ("Documents", p.database()),
            ("Audio", p.audio()),
            ("Speech models", p.models()),
            ("Templates & prompts", p.config_dir.clone()),
            ("Exports", p.exports()),
        ] {
            let mut shown = path.to_string_lossy().into_owned();
            if let Ok(home) = std::env::var("HOME")
                && !home.is_empty()
                && let Some(rest) = shown.strip_prefix(&home)
            {
                shown = format!("~{rest}");
            }
            if path.extension().is_none() {
                shown.push('/');
            }
            let open = gtk::Button::with_label("Open folder");
            open.add_css_class("fx-secondary");
            open.add_css_class("small");
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
            let size = label("", &["fx-mono", "fx-storage-size"]);
            size.set_xalign(1.0);
            size.set_size_request(80, -1);
            let controls = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            controls.append(&size);
            controls.append(&open);
            let r = row(name, Some(shown.as_str()), &controls);
            if let Some(note) = row_note(&r) {
                note.add_css_class("fx-mono");
                note.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                note.set_wrap(false);
                note.set_tooltip_text(Some(&shown));
            }
            rows.append(&r);
            sizes.push((path, size));
        }
        *self.storage_sizes.borrow_mut() = sizes;
        b.append(&folders);

        let (audio, rows) = group("Audio", None);
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
        self.audio_retention.set_size_request(220, -1);
        self.audio_retention.set_valign(gtk::Align::Center);
        rows.append(&row(
            "Delete audio after",
            Some("Text is kept. Playback and re-transcribing need the audio. Applied when Fennec starts."),
            &self.audio_retention,
        ));
        b.append(&audio);
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

    /// Disk use: a total, a bar split by kind and a legend.
    fn storage_panel(self: &Rc<Self>) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 24);
        let usage = panel_group("DISK USE");
        let total = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        self.disk_total.set_valign(gtk::Align::BaselineFill);
        total.append(&self.disk_total);
        let in_all = label("in all", &["fx-field-note"]);
        in_all.set_valign(gtk::Align::BaselineFill);
        total.append(&in_all);
        usage.append(&total);
        self.disk_bar.set_content_height(10);
        self.disk_bar.add_css_class("fx-disk-bar");
        let use_ = Rc::clone(&self.disk_use);
        self.disk_bar.set_draw_func(move |area, cr, w, h| {
            let c = area.color();
            let sizes = *use_.borrow();
            let sum: u64 = sizes.iter().sum();
            let (w, h) = (f64::from(w), f64::from(h));
            rounded(cr, 0.0, w, h);
            cr.clip();
            cr.set_source_rgba(c.red().into(), c.green().into(), c.blue().into(), 0.12);
            let _ = cr.paint();
            if sum == 0 {
                return;
            }
            let mut x = 0.0;
            for (i, s) in sizes.iter().enumerate() {
                let part = w * *s as f64 / sum as f64;
                cr.rectangle(x, 0.0, part, h);
                cr.set_source_rgba(c.red().into(), c.green().into(), c.blue().into(), SHADES[i]);
                let _ = cr.fill();
                x += part;
            }
        });
        usage.append(&self.disk_bar);
        for (i, (name, value)) in ["Speech models", "Audio", "Documents", "Other"]
            .iter()
            .zip(&self.disk_legend)
            .enumerate()
        {
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let swatch = gtk::DrawingArea::builder()
                .content_width(10)
                .content_height(10)
                .valign(gtk::Align::Center)
                .css_classes(["fx-disk-bar"])
                .build();
            swatch.set_draw_func(move |area, cr, w, h| {
                let c = area.color();
                rounded(cr, 0.0, f64::from(w), f64::from(h));
                cr.set_source_rgba(c.red().into(), c.green().into(), c.blue().into(), SHADES[i]);
                let _ = cr.fill();
            });
            r.append(&swatch);
            let n = label(name, &[]);
            n.set_hexpand(true);
            r.append(&n);
            r.append(value);
            usage.append(&r);
        }
        b.append(&usage);
        let note = label(
            "Removing a speech model you don't use frees the most space. Models can be downloaded again at any time.",
            &["fx-field-note"],
        );
        note.set_wrap(true);
        b.append(&note);
        b
    }

    /// Measures the folders off the main thread and fills in the sizes.
    fn measure_storage(self: &Rc<Self>) {
        let paths: Vec<PathBuf> = self
            .storage_sizes
            .borrow()
            .iter()
            .map(|(p, _)| p.clone())
            .collect();
        let (tx, rx) = async_channel::bounded::<Vec<u64>>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(paths.iter().map(|p| disk_size(p)).collect());
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(sizes) = rx.recv().await else { return };
            let Some(p) = weak.upgrade() else { return };
            for ((_, l), s) in p.storage_sizes.borrow().iter().zip(&sizes) {
                l.set_text(&human_size(*s));
            }
            // Folders: documents, audio, models, templates, exports.
            let get = |i: usize| sizes.get(i).copied().unwrap_or(0);
            let kinds = [get(2), get(1), get(0), get(3) + get(4)];
            *p.disk_use.borrow_mut() = kinds;
            for (l, s) in p.disk_legend.iter().zip(kinds) {
                l.set_text(&human_size(s));
            }
            p.disk_total.set_text(&human_size(kinds.iter().sum()));
            p.disk_bar.queue_draw();
        });
    }

    /// Model row titles, in order (tests).
    pub fn model_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![self.models_box.clone().upcast::<gtk::Widget>()];
        while let Some(w) = stack.pop() {
            if w.has_css_class("fx-model-row") {
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

/// How strongly each kind is drawn in the disk-use bar (of the accent).
const SHADES: [f64; 4] = [1.0, 0.55, 0.3, 0.16];

fn rounded(cr: &gtk::cairo::Context, x: f64, w: f64, h: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    let r = (h / 2.0).min(3.0);
    cr.new_sub_path();
    cr.arc(x + w - r, r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// Bytes used by a file, or by everything under a folder. A folder that is
/// a link (models moved to another disk) is measured where it points; links
/// inside it are not followed.
fn disk_size(path: &Path) -> u64 {
    size_of(path, std::fs::metadata(path))
}

fn size_of(path: &Path, meta: std::io::Result<std::fs::Metadata>) -> u64 {
    let Ok(meta) = meta else {
        return 0;
    };
    if !meta.is_dir() {
        // SQLite keeps recent changes beside the database.
        let wal = path.with_extension(format!(
            "{}-wal",
            path.extension().and_then(|e| e.to_str()).unwrap_or_default()
        ));
        return meta.len() + std::fs::metadata(wal).map(|m| m.len()).unwrap_or(0);
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| {
                    let p = e.path();
                    size_of(&p, std::fs::symlink_metadata(&p))
                })
                .sum()
        })
        .unwrap_or(0)
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return if bytes == 0 { "0 KB".into() } else { "1 KB".into() };
    }
    let mut v = bytes as f64 / 1000.0;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if v < 10.0 && unit > 0 {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// The vocabulary setting's entries: separated by commas, semicolons or lines.
fn split_vocabulary(text: &str) -> Vec<String> {
    text.split([',', '\n', ';'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// "Edda v0.2 · Vulkan": the model's catalog name where known, and the
/// compute it will use.
pub(super) fn model_label(settings: &crate::config::Settings) -> String {
    let name = models::catalog()
        .into_iter()
        .find(|m| m.file_name == settings.model)
        .map(|m| m.name.to_string())
        .unwrap_or_else(|| settings.model.trim_end_matches(".bin").to_string());
    let gpu = models::wants_gpu(settings.backend);
    let backend = match settings.backend {
        Backend::Cuda if gpu => "CUDA",
        Backend::Vulkan if gpu => "Vulkan",
        Backend::Auto if gpu && cfg!(feature = "cuda") => "CUDA",
        Backend::Auto if gpu => "Vulkan",
        _ => "CPU",
    };
    format!("{name} · {backend}")
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

/// Licence (warn style when non-commercial) and size.
fn tags(m: &models::CatalogModel) -> Vec<(String, bool)> {
    let licence = if m.non_commercial {
        "Non-commercial".to_string()
    } else {
        m.license.to_string()
    };
    vec![(licence, m.non_commercial), (m.size.replace('~', ""), false)]
}

/// "550 MB" from a catalog size such as "q5_0 · ~550 MB".
fn download_size(m: &models::CatalogModel) -> String {
    m.size
        .rsplit(" · ")
        .next()
        .unwrap_or(m.size)
        .trim_start_matches('~')
        .to_string()
}

/// The catalog description's first sentence when it names the architecture
/// ("Whisper large-v3-turbo, 0.8B parameters"), and the rest.
fn split_description(d: &str) -> (Option<&str>, &str) {
    match d.split_once(". ") {
        Some((arch, rest)) if arch.contains("parameters") => (Some(arch), rest),
        _ => (None, d),
    }
}

fn model_row(
    name: &str,
    meta: &str,
    description: &str,
    tags: &[(String, bool)],
    wer: Option<&str>,
    active: bool,
    installed: bool,
) -> gtk::Box {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    r.add_css_class("fx-model-row");
    if active {
        r.add_css_class("active");
    }
    let radio = gtk::Box::builder()
        .css_classes(["fx-radio"])
        .valign(gtk::Align::Start)
        .build();
    if active {
        radio.add_css_class("on");
    }
    // Only models on this computer can be chosen.
    radio.set_opacity(if installed { 1.0 } else { 0.0 });
    r.append(&radio);

    let info = gtk::Box::new(gtk::Orientation::Vertical, 2);
    info.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let n = label(name, &["fx-model-name"]);
    n.set_wrap(true);
    head.append(&n);
    if active {
        let (text, kind) = if installed {
            ("IN USE", "active")
        } else {
            ("SELECTED · NOT DOWNLOADED", "warn")
        };
        let pill = label(text, &["fx-badge", kind]);
        pill.set_valign(gtk::Align::Center);
        head.append(&pill);
    }
    info.append(&head);
    let m = label(meta, &["fx-model-publisher"]);
    m.set_wrap(true);
    info.append(&m);
    let d = label(description, &["fx-model-desc"]);
    d.set_wrap(true);
    d.set_max_width_chars(60);
    info.append(&d);
    let chips = super::wrap::wrap_box();
    for (tag, warn) in tags {
        chips.append(&label(tag, &["fx-badge", if *warn { "warn" } else { "plain" }]));
    }
    info.append(&chips);
    r.append(&info);

    let score = gtk::Box::new(gtk::Orientation::Vertical, 2);
    score.add_css_class("fx-wer");
    score.set_size_request(64, -1);
    score.set_valign(gtk::Align::Start);
    let value = label(wer.unwrap_or("–"), &["fx-wer-value"]);
    value.set_xalign(1.0);
    let caption = label("WER", &["fx-wer-caption"]);
    caption.set_xalign(1.0);
    score.append(&value);
    score.append(&caption);
    r.append(&score);
    r
}

/// A section's heading and a line under it, with an optional button on the right.
fn title_block(title: &str, lede: &str, action: Option<&gtk::Button>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    text.append(&label(title, &["fx-h1"]));
    let l = label(lede, &["fx-status"]);
    l.set_wrap(true);
    text.append(&l);
    b.append(&text);
    if let Some(a) = action {
        b.append(a);
    }
    b
}

/// A titled group: a heading (and a count beside it) over a bordered box
/// of rows. Returns the group and the box the rows go in.
pub(super) fn group(title: &str, count: Option<&str>) -> (gtk::Box, gtk::Box) {
    let g = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.append(&label(title, &["fx-h3"]));
    if let Some(c) = count {
        head.append(&label(c, &["fx-count"]));
    }
    g.append(&head);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.add_css_class("fx-set-box");
    rows.set_overflow(gtk::Overflow::Hidden);
    g.append(&rows);
    (g, rows)
}

/// A settings row: the name (and a note under it) on the left, the
/// control on the right.
pub(super) fn row(title: &str, note: Option<&str>, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    r.add_css_class("fx-set-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&label(title, &["fx-set-title"]));
    if let Some(n) = note {
        let l = label(n, &["fx-field-note"]);
        l.set_wrap(true);
        text.append(&l);
    }
    r.append(&text);
    control.upcast_ref::<gtk::Widget>().set_valign(gtk::Align::Center);
    r.append(control);
    r
}

/// The note label of a [`row`], if it has one.
fn row_note(r: &gtk::Box) -> Option<gtk::Label> {
    r.first_child()?
        .last_child()?
        .downcast::<gtk::Label>()
        .ok()
        .filter(|l| l.has_css_class("fx-field-note"))
}

/// A heading in the right-hand panel and the box under it.
pub(super) fn panel_group(title: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
    b.append(&label(title, &["fx-section-title"]));
    b
}

/// A slider with its value in mono on the right.
fn slider(scale: &gtk::Scale) -> (gtk::Box, gtk::Label) {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    scale.set_draw_value(false);
    scale.set_size_request(200, -1);
    scale.set_valign(gtk::Align::Center);
    let value = label("", &["fx-mono-value"]);
    value.set_xalign(1.0);
    value.set_size_request(52, -1);
    b.append(scale);
    b.append(&value);
    (b, value)
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
            clamp.set_hexpand(true);
            outer.append(&clamp);
        }
        None => outer.append(&content),
    }
    (outer, content)
}

pub(super) fn field(name: &str, w: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&label(name, &["fx-field-label"]));
    b.append(w);
    b
}

#[cfg(test)]
mod tests {
    use super::{disk_size, download_size, human_size, split_description, split_vocabulary};

    #[test]
    fn catalog_text_splits_into_architecture_and_description() {
        let (arch, rest) =
            split_description("Whisper large-v3-turbo, 0.8B parameters. Fast enough for live dictation.");
        assert_eq!(arch, Some("Whisper large-v3-turbo, 0.8B parameters"));
        assert_eq!(rest, "Fast enough for live dictation.");
        let (arch, rest) = split_description("The first Edda. Leaves out punctuation.");
        assert_eq!(arch, None);
        assert_eq!(rest, "The first Edda. Leaves out punctuation.");
    }

    #[test]
    fn download_buttons_show_the_size() {
        let m = crate::models::catalog().into_iter().next().unwrap();
        assert_eq!(download_size(&m), "550 MB");
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(human_size(0), "0 KB");
        assert_eq!(human_size(1_500_000), "1.5 MB");
        assert_eq!(human_size(550_000_000), "550 MB");
        assert_eq!(human_size(2_800_000_000), "2.8 GB");
    }

    #[test]
    fn a_linked_folder_is_measured_where_it_points_but_inner_links_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("disk2/models");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("model.bin"), vec![0u8; 5000]).unwrap();
        let link = tmp.path().join("models");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(disk_size(&link), 5000);
        // A link inside counts as the link, not what it points to.
        std::os::unix::fs::symlink(&real, real.join("loop")).unwrap();
        assert!(disk_size(&link) < 5100);
        assert_eq!(disk_size(&tmp.path().join("missing")), 0);
    }

    #[test]
    fn vocabulary_entries_split_on_commas_and_lines() {
        assert_eq!(
            split_vocabulary("Fennec, Sagsnr.\nsags nr -> Sagsnr.;"),
            ["Fennec", "Sagsnr.", "sags nr -> Sagsnr."]
        );
    }
}
