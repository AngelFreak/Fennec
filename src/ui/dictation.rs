//! The Dictate screen: title, chips, editor, record dock and inspector,
//! plus the live session that feeds the editor.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Instant;

use gtk::glib;
use gtk::prelude::*;

use super::dock::{Dock, DockState};
use super::editor::Editor;
use super::engine::EngineHolder;
use super::inspector::Inspector;
use super::{Deps, label};
use crate::live::{LiveConfig, LiveEvent, LiveSession};
use crate::store::{DocumentId, NewDocument, Store};
use crate::template::{PlaceholderContext, Template, install_defaults, load_dir};
use crate::text::{danish_today, duration};
use crate::utterance::UtteranceConfig;
use crate::worker::EngineWorker;

#[derive(Default)]
struct State {
    doc: Option<DocumentId>,
    session: Option<LiveSession>,
    started: Option<Instant>,
    save_timer: Option<glib::SourceId>,
    tick: Option<glib::SourceId>,
    recording: bool,
}

pub struct DictationPage {
    pub root: gtk::Box,
    pub editor: Rc<Editor>,
    pub dock: Dock,
    pub inspector: Rc<Inspector>,
    pub title: gtk::Entry,
    project_chip: gtk::Label,
    project_menu: gtk::MenuButton,
    tag_entry: gtk::Entry,
    tags: gtk::Box,
    store: Rc<Store>,
    deps: Deps,
    engine: Rc<EngineHolder>,
    state: RefCell<State>,
    loading_templates: Cell<bool>,
    on_saved: super::TextHandler,
    on_document_changed: super::Handler<()>,
}

impl DictationPage {
    pub fn new(store: Rc<Store>, deps: Deps, engine: Rc<EngineHolder>) -> Rc<Self> {
        let editor = Editor::new();
        let dock = Dock::new();
        let inspector = Inspector::new();

        let title = gtk::Entry::builder()
            .css_classes(["fx-doc-title"])
            .hexpand(true)
            .build();
        title.update_property(&[gtk::accessible::Property::Label("Document title")]);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let project_chip = label("Unsorted", &["fx-chip"]);
        let project_menu = gtk::MenuButton::builder()
            .child(&project_chip)
            .tooltip_text("Move to project")
            .build();
        project_menu.add_css_class("flat");
        project_menu.set_popover(Some(&gtk::Popover::new()));
        let tags = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let add_tag = gtk::MenuButton::builder()
            .label("+ Tag")
            .tooltip_text("Add a tag")
            .build();
        add_tag.add_css_class("fx-tag");
        let tag_entry = gtk::Entry::builder()
            .placeholder_text("Tag, e.g. meeting")
            .build();
        let tag_pop = gtk::Popover::builder().child(&tag_entry).build();
        add_tag.set_popover(Some(&tag_pop));
        chips.append(&project_menu);
        chips.append(&tags);
        chips.append(&add_tag);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
        column.set_margin_top(28);
        column.set_margin_bottom(24);
        column.set_margin_start(56);
        column.set_margin_end(56);
        let clamp = adw::Clamp::builder().maximum_size(680).child(&column).build();
        column.append(&chips);
        column.append(&title);
        column.append(&editor.view);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&clamp)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["fx-editor-scroller"])
            .build();

        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.set_hexpand(true);
        main.append(&scroller);
        main.append(&dock.root);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&main);
        root.append(&inspector.root);

        let page = Rc::new(Self {
            root,
            editor,
            dock,
            inspector,
            title,
            project_chip,
            project_menu,
            tag_entry,
            tags,
            store,
            deps,
            engine,
            state: RefCell::default(),
            loading_templates: Cell::new(false),
            on_saved: RefCell::default(),
            on_document_changed: RefCell::default(),
        });
        page.editor.paragraph_gap_ms.set(3_000);
        page.wire();
        page
    }

    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.tag_entry.connect_activate(move |e| {
            let Some(p) = weak.upgrade() else { return };
            let tag = e.text().trim().to_string();
            e.set_text("");
            if let Some(pop) = e
                .parent()
                .and_then(|w| w.ancestor(gtk::Popover::static_type()))
                .and_downcast::<gtk::Popover>()
            {
                pop.popdown();
            }
            if !tag.is_empty() {
                p.change_tags(|tags| tags.push(tag));
            }
        });
        let weak = Rc::downgrade(self);
        self.dock.record.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.toggle_recording();
            }
        });
        let weak = Rc::downgrade(self);
        self.editor.buffer().connect_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.schedule_save();
            }
        });
        let weak = Rc::downgrade(self);
        self.title.connect_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.schedule_save();
            }
        });
        let weak = Rc::downgrade(self);
        self.inspector.connect_change(move || {
            if let Some(p) = weak.upgrade() {
                p.schedule_save();
            }
        });
        let weak = Rc::downgrade(self);
        self.inspector.connect_review(move |i| {
            if let Some(p) = weak.upgrade() {
                p.editor.select_unsure(i);
            }
        });
        let weak = Rc::downgrade(self);
        self.inspector.template_choice.connect_selected_notify(move |_| {
            if let Some(p) = weak.upgrade()
                && !p.loading_templates.get()
            {
                p.show_template_fields();
                p.schedule_save();
            }
        });
    }

    pub fn connect_saved(&self, f: impl Fn(&str) + 'static) {
        *self.on_saved.borrow_mut() = Some(Rc::new(f));
    }

    /// Called when the open document changes (title, project, tags).
    pub fn connect_document_changed(&self, f: impl Fn() + 'static) {
        *self.on_document_changed.borrow_mut() = Some(Rc::new(move |()| f()));
    }

    pub fn document(&self) -> Option<DocumentId> {
        self.state.borrow().doc
    }

    pub fn is_recording(&self) -> bool {
        self.state.borrow().recording
    }

    fn templates(&self) -> Vec<Template> {
        let dir = self.deps.paths.templates();
        if let Err(e) = install_defaults(&dir) {
            tracing::warn!("could not install default templates in {}: {e}", dir.display());
        }
        let mut list: Vec<Template> = load_dir(&dir)
            .into_iter()
            .filter_map(|t| t.map_err(|e| tracing::warn!("{e}")).ok())
            .collect();
        list.push(Template::blank());
        list
    }

    fn placeholders(&self) -> PlaceholderContext {
        PlaceholderContext {
            today: danish_today(),
            user: self.deps.settings.user_name.clone(),
            duration: String::new(),
            model: self.deps.settings.model.clone(),
        }
    }

    /// Creates and opens a new, empty dictation document.
    pub fn new_document(self: &Rc<Self>, project: Option<i64>) -> Result<DocumentId, String> {
        self.save_now();
        let templates = self.templates();
        let preferred = project
            .and_then(|p| {
                self.store
                    .projects()
                    .ok()?
                    .into_iter()
                    .find(|x| x.id == p)?
                    .default_template
            })
            .unwrap_or_else(|| self.deps.settings.default_template.clone());
        let template = templates
            .iter()
            .find(|t| t.id == preferred)
            .cloned()
            .unwrap_or_else(Template::blank);
        let id = self
            .store
            .create_document(&NewDocument {
                project_id: project,
                template_id: Some(template.id.clone()),
                ..NewDocument::dictation(&format!("Diktat {}", danish_today()))
            })
            .map_err(|e| e.to_string())?;
        let fields = template.initial_values(&self.placeholders());
        let title = self.store.document(id).map_err(|e| e.to_string())?.title;
        self.store
            .update_document(id, &title, Some(&template.id), &fields)
            .map_err(|e| e.to_string())?;
        self.open_document(id)?;
        Ok(id)
    }

    pub fn open_document(self: &Rc<Self>, id: DocumentId) -> Result<(), String> {
        if self.is_recording() {
            return Err("Stop dictation before opening another document.".into());
        }
        self.save_now();
        let doc = self.store.document(id).map_err(|e| e.to_string())?;
        let paragraphs = self.store.paragraphs(id).map_err(|e| e.to_string())?;
        self.state.borrow_mut().doc = Some(id);
        self.title.set_text(&doc.title);
        self.editor.load(&paragraphs);
        self.loading_templates.set(true);
        let templates = self.templates();
        let selected = doc
            .template_id
            .clone()
            .unwrap_or_else(|| self.deps.settings.default_template.clone());
        self.inspector.set_templates(templates, &selected);
        self.loading_templates.set(false);
        if let Some(t) = self.inspector.selected_template() {
            self.inspector.show_fields(&t, &doc.fields);
        }
        self.show_chips();
        self.refresh_review();
        // Loading fired change signals; nothing is unsaved.
        if let Some(t) = self.state.borrow_mut().save_timer.take() {
            t.remove();
        }
        Ok(())
    }

    fn show_template_fields(self: &Rc<Self>) {
        let Some(t) = self.inspector.selected_template() else {
            return;
        };
        let mut values = self.inspector.values();
        for (k, v) in t.initial_values(&self.placeholders()) {
            values.entry(k).or_insert(v);
        }
        self.inspector.show_fields(&t, &values);
    }

    fn show_chips(self: &Rc<Self>) {
        let Some(id) = self.document() else { return };
        let Ok(doc) = self.store.document(id) else { return };
        let projects = self.store.projects().unwrap_or_default();
        let current = doc
            .project_id
            .and_then(|p| projects.iter().find(|x| x.id == p))
            .map(|p| p.name.clone());
        self.project_chip
            .set_text(current.as_deref().unwrap_or("Unsorted"));
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let mut choices: Vec<(String, Option<i64>)> = vec![("Unsorted".into(), None)];
        choices.extend(projects.iter().map(|p| (p.name.clone(), Some(p.id))));
        for (name, pid) in choices {
            let b = gtk::Button::with_label(&name);
            b.add_css_class("flat");
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |b| {
                let Some(p) = weak.upgrade() else { return };
                if let Some(pop) = b
                    .ancestor(gtk::Popover::static_type())
                    .and_downcast::<gtk::Popover>()
                {
                    pop.popdown();
                }
                p.move_to_project(pid);
            });
            menu.append(&b);
        }
        if let Some(pop) = self.project_menu.popover() {
            pop.set_child(Some(&menu));
        }
        while let Some(c) = self.tags.first_child() {
            self.tags.remove(&c);
        }
        for t in &doc.tags {
            let b = gtk::Button::with_label(&format!("#{t}"));
            b.add_css_class("fx-tag");
            b.set_tooltip_text(Some("Remove tag"));
            let weak = Rc::downgrade(self);
            let tag = t.clone();
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.change_tags(|tags| tags.retain(|x| *x != tag));
                }
            });
            self.tags.append(&b);
        }
    }

    pub fn move_to_project(self: &Rc<Self>, project: Option<i64>) {
        let Some(id) = self.document() else { return };
        if let Err(e) = self.store.move_document(id, project) {
            tracing::error!("moving document {id}: {e}");
        }
        self.show_chips();
        if let Some(f) = self.on_document_changed.borrow().clone() {
            f(());
        }
    }

    pub fn change_tags(self: &Rc<Self>, edit: impl FnOnce(&mut Vec<String>)) {
        let Some(id) = self.document() else { return };
        let mut tags = self.store.document(id).map(|d| d.tags).unwrap_or_default();
        edit(&mut tags);
        if let Err(e) = self.store.set_tags(id, &tags) {
            tracing::error!("tagging document {id}: {e}");
        }
        self.show_chips();
        if let Some(f) = self.on_document_changed.borrow().clone() {
            f(());
        }
    }

    pub fn project_name(&self) -> String {
        self.project_chip.text().to_string()
    }

    fn schedule_save(self: &Rc<Self>) {
        let weak: Weak<Self> = Rc::downgrade(self);
        let mut st = self.state.borrow_mut();
        if let Some(t) = st.save_timer.take() {
            t.remove();
        }
        st.save_timer = Some(glib::timeout_add_local_once(
            std::time::Duration::from_secs(1),
            move || {
                if let Some(p) = weak.upgrade() {
                    p.state.borrow_mut().save_timer = None;
                    p.save_now();
                }
            },
        ));
    }

    /// Writes the editor, title and fields to the store.
    pub fn save_now(&self) {
        if let Some(t) = self.state.borrow_mut().save_timer.take() {
            t.remove();
        }
        let Some(id) = self.document() else { return };
        let paragraphs = self.editor.paragraphs();
        let template = self.inspector.selected_template().map(|t| t.id);
        let result = self.store.sync_paragraphs(id, &paragraphs).and_then(|_| {
            self.store.update_document(
                id,
                self.title.text().trim(),
                template.as_deref(),
                &self.inspector.values(),
            )
        });
        let message = match &result {
            Ok(()) => "Saved".to_string(),
            Err(e) => format!("Not saved: {e}"),
        };
        if let Err(e) = &result {
            tracing::error!("saving document {id}: {e}");
        }
        self.refresh_review();
        if let Some(f) = self.on_saved.borrow().clone() {
            f(&message);
        }
        if let Some(f) = self.on_document_changed.borrow().clone() {
            f(());
        }
    }

    fn refresh_review(&self) {
        let words = self.editor.unsure_words();
        // The inspector needs an Rc for its button callbacks.
        let inspector = Rc::clone(&self.inspector);
        inspector.set_unsure(&words);
        let paragraphs = self.editor.paragraphs();
        let count: usize = paragraphs.iter().map(|p| p.text.split_whitespace().count()).sum();
        let recorded = paragraphs.iter().filter_map(|p| p.end_ms).max().unwrap_or(0);
        self.inspector
            .stats
            .set_text(&format!("{count} words · Recorded {}", duration(recorded)));
    }

    pub fn toggle_recording(self: &Rc<Self>) {
        if self.is_recording() {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }

    pub fn start_recording(self: &Rc<Self>) {
        if self.is_recording() {
            return;
        }
        if self.document().is_none()
            && let Err(e) = self.new_document(None)
        {
            self.dock
                .set_status(&format!("Could not create a document: {e}"), true);
            return;
        }
        if !self.engine.is_loaded() {
            self.dock.set_state(DockState::Loading);
            self.dock.set_status("Loading the speech model…", false);
        }
        let weak = Rc::downgrade(self);
        self.engine.with_worker(move |result| {
            let Some(page) = weak.upgrade() else { return };
            match result {
                Ok(worker) => page.begin_session(worker),
                Err(e) => {
                    page.dock.set_state(DockState::Idle);
                    page.dock.set_status(
                        &format!("Could not load the speech model: {e}. Choose a model in Settings."),
                        true,
                    );
                }
            }
        });
    }

    fn begin_session(self: &Rc<Self>, worker: Arc<EngineWorker>) {
        let settings = &self.deps.settings;
        let source = match (self.deps.audio)(settings) {
            Ok(s) => s,
            Err(e) => {
                self.dock.set_state(DockState::Idle);
                self.dock.set_status(&format!("Microphone: {e}"), true);
                return;
            }
        };
        let vad = match (self.deps.vad)(settings, &self.deps.paths) {
            Ok(v) => v,
            Err(e) => {
                self.dock.set_state(DockState::Idle);
                self.dock.set_status(&format!("Voice detection: {e}"), true);
                return;
            }
        };
        let doc = self.document().expect("a document is open");
        let offset_ms = self
            .editor
            .paragraphs()
            .iter()
            .filter_map(|p| p.end_ms)
            .max()
            .unwrap_or(0);
        let record_to = settings.keep_dictation_audio.then(|| {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            self.deps.paths.audio().join(format!("doc{doc}-{stamp}.wav"))
        });
        if let Some(path) = &record_to
            && let Err(e) = self.store.set_audio_path(doc, Some(path))
        {
            tracing::warn!("could not record the audio path: {e}");
        }
        let cfg = LiveConfig {
            utterance: UtteranceConfig {
                pause_ms: settings.pause_ms,
                ..Default::default()
            },
            vocabulary: settings.vocabulary.clone(),
            commands: settings.commands.clone(),
            show_preview: settings.show_preview,
            record_to,
            offset_ms,
            ..Default::default()
        };
        let (tx, rx) = async_channel::unbounded();
        let session = LiveSession::start(
            source,
            vad,
            worker,
            cfg,
            Arc::new(move |e| {
                let _ = tx.send_blocking(e);
            }),
        );
        {
            let mut st = self.state.borrow_mut();
            st.session = Some(session);
            st.recording = true;
            st.started = Some(Instant::now());
        }
        self.dock.set_state(DockState::Recording);
        self.dock
            .set_status("Listening. Text is committed when you pause briefly.", false);
        self.dock.set_timer(0);
        let weak = Rc::downgrade(self);
        let tick = glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
            let Some(p) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let secs = p
                .state
                .borrow()
                .started
                .map(|s| s.elapsed().as_secs())
                .unwrap_or(0);
            p.dock.set_timer(secs);
            glib::ControlFlow::Continue
        });
        self.state.borrow_mut().tick = Some(tick);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                let Some(page) = weak.upgrade() else { return };
                page.handle(ev);
            }
        });
    }

    /// Stops listening; what was already said is still transcribed.
    pub fn stop_recording(self: &Rc<Self>) {
        let session = self.state.borrow_mut().session.take();
        if let Some(s) = session {
            self.dock.set_status("Finishing the last sentence…", false);
            self.dock.record.set_sensitive(false);
            // Joining waits for the engine; keep that off the UI thread.
            std::thread::spawn(move || s.stop());
        }
    }

    fn handle(self: &Rc<Self>, ev: LiveEvent) {
        match ev {
            LiveEvent::Level(l) => self.dock.push_level(l),
            LiveEvent::SpeechStarted => {}
            LiveEvent::Preview(t) => self.editor.set_preview(Some(&t)),
            LiveEvent::Final {
                text,
                start_ms,
                end_ms,
                low_confidence,
            } => {
                self.editor.insert_final(&text, start_ms, end_ms, &low_confidence);
                self.save_now();
            }
            LiveEvent::Command(crate::commands::Command::StopDictation) => self.stop_recording(),
            LiveEvent::Command(c) => {
                self.editor.apply(c);
                self.dock.set_status(&format!("Command: {}", c.label()), false);
            }
            LiveEvent::Lag(ms) if ms > 3_000 => self.dock.set_status(
                &format!("Behind by {} s. Nothing is lost; text will catch up.", ms / 1000),
                false,
            ),
            LiveEvent::Lag(_) => {}
            LiveEvent::Error(e) => self.dock.set_status(&e, true),
            LiveEvent::Stopped => {
                let mut st = self.state.borrow_mut();
                st.recording = false;
                st.session = None;
                st.started = None;
                if let Some(t) = st.tick.take() {
                    t.remove();
                }
                drop(st);
                self.editor.set_preview(None);
                self.dock.set_state(DockState::Idle);
                if !self.dock.status_text().starts_with("Microphone") {
                    self.dock
                        .set_status("Press the button or Ctrl+Space to continue.", false);
                }
                self.save_now();
            }
        }
    }
}
