//! The main window: header, sidebar and the screen stack.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use super::dictation::DictationPage;
use super::engine::EngineHolder;
use super::export_page::{ExportPage, Target};
use super::files::FilesPage;
use super::project::{ProjectPage, Scope};
use super::settings_page::SettingsPage;
use super::sidebar::{Nav, Sidebar};
use super::templates_page::TemplatesPage;
use super::{Deps, icon_button, label};
use crate::store::{DocumentFilter, Store};

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    pub stack: gtk::Stack,
    pub dictation: Rc<DictationPage>,
    pub files: Rc<FilesPage>,
    pub export: Rc<ExportPage>,
    pub project: Rc<ProjectPage>,
    pub templates: Rc<TemplatesPage>,
    pub settings: Rc<SettingsPage>,
    model_chip: gtk::Button,
    pub sidebar: Rc<Sidebar>,
    pub store: Rc<Store>,
    crumb_project: gtk::Label,
    crumb_title: gtk::Label,
    saved: gtk::Label,
}

impl MainWindow {
    pub fn new(deps: Deps) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .title("Fennec")
            .default_width(1280)
            .default_height(800)
            .build();
        window.add_css_class("fennec");

        let store = match Store::open(&deps.paths.database()) {
            Ok(s) => Rc::new(s),
            Err(e) => {
                let page = adw::StatusPage::builder()
                    .icon_name("dialog-error-symbolic")
                    .title("Fennec could not open its database")
                    .description(format!("{}\n\n{e}", deps.paths.database().display()))
                    .build();
                window.set_content(Some(&page));
                // A throwaway in-memory store keeps the types simple; nothing is shown from it.
                let store = Rc::new(Store::open_in_memory().expect("in-memory SQLite"));
                return Self::shell(window, store, deps, false);
            }
        };
        Self::shell(window, store, deps, true)
    }

    fn shell(window: adw::ApplicationWindow, store: Rc<Store>, deps: Deps, show: bool) -> Rc<Self> {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        header.add_css_class("fx-header");
        let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        brand.set_size_request(204, -1);
        let logo = label("F", &["fx-logo"]);
        logo.set_xalign(0.5);
        logo.set_valign(gtk::Align::Center);
        brand.append(&logo);
        brand.append(&label("Fennec", &["fx-app-name"]));
        header.append(&brand);

        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        crumbs.set_hexpand(true);
        let crumb_project = label("", &["fx-crumb"]);
        let crumb_title = label("", &["fx-crumb-current"]);
        crumb_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let saved = label("", &["fx-saved"]);
        crumbs.append(&crumb_project);
        crumbs.append(&label("/", &["fx-crumb"]));
        crumbs.append(&crumb_title);
        crumbs.append(&saved);
        header.append(&crumbs);

        let model_chip = gtk::Button::with_label(&model_label(&deps.settings()));
        model_chip.add_css_class("fx-secondary");
        model_chip.set_valign(gtk::Align::Center);
        header.append(&model_chip);
        let new_doc = icon_button(
            "document-new-symbolic",
            "New dictation (Ctrl+N)",
            &["fx-secondary"],
        );
        new_doc.set_valign(gtk::Align::Center);
        header.append(&new_doc);
        let export_button = gtk::Button::with_label("Export");
        export_button.add_css_class("fx-primary");
        export_button.set_valign(gtk::Align::Center);
        header.append(&export_button);
        // The window-manager controls stay available.
        header.append(&gtk::WindowControls::new(gtk::PackType::End));
        let handle = gtk::WindowHandle::builder().child(&header).build();

        let sidebar = Sidebar::new(Rc::clone(&store));
        let engine = EngineHolder::new(deps.clone());
        let dictation = DictationPage::new(Rc::clone(&store), deps.clone(), Rc::clone(&engine));
        let files = FilesPage::new(Rc::clone(&store), deps.clone(), Rc::clone(&engine));
        let export = ExportPage::new(Rc::clone(&store), deps.clone());
        let project = ProjectPage::new(Rc::clone(&store), deps.paths.templates());
        let templates = TemplatesPage::new(deps.paths.templates());
        let settings = SettingsPage::new(deps.clone(), Rc::clone(&engine));

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.add_named(&dictation.root, Some("dictate"));
        stack.add_named(&files.root, Some("files"));
        stack.add_named(&export.root, Some("export"));
        stack.add_named(&project.root, Some("project"));
        stack.add_named(&templates.root, Some("templates"));
        stack.add_named(&settings.root, Some("settings"));

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        body.append(&sidebar.root);
        body.append(&stack);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&handle);
        content.append(&body);
        if show {
            window.set_content(Some(&content));
        }

        let win = Rc::new(Self {
            window,
            stack,
            dictation,
            files,
            export,
            project,
            templates,
            settings,
            model_chip: model_chip.clone(),
            sidebar,
            store,
            crumb_project,
            crumb_title,
            saved,
        });
        win.wire(&model_chip, &new_doc, &export_button);
        if show {
            win.open_initial_document();
        }
        win
    }

    fn wire(self: &Rc<Self>, model_chip: &gtk::Button, new_doc: &gtk::Button, export_button: &gtk::Button) {
        let weak = Rc::downgrade(self);
        export_button.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.export_current();
            }
        });
        let weak = Rc::downgrade(self);
        self.settings.connect_model_changed(move |()| {
            if let Some(w) = weak.upgrade() {
                w.model_chip.set_label(&model_label(&w.settings_snapshot()));
            }
        });
        let weak = Rc::downgrade(self);
        self.project.connect_open(move |doc| {
            if let Some(w) = weak.upgrade() {
                w.open_in_editor(doc);
            }
        });
        let weak = Rc::downgrade(self);
        self.project.connect_export(move |(title, ids)| {
            if let Some(w) = weak.upgrade() {
                w.export.show(Target::Documents { title, ids });
                w.stack.set_visible_child_name("export");
            }
        });
        let weak = Rc::downgrade(self);
        self.project.connect_changed(move |()| {
            if let Some(w) = weak.upgrade() {
                w.sidebar.refresh();
            }
        });
        let weak = Rc::downgrade(self);
        self.files.connect_open_in_editor(move |doc| {
            if let Some(w) = weak.upgrade() {
                match w.dictation.open_document(doc) {
                    Ok(()) => w.sidebar.go(Nav::Dictate),
                    Err(e) => w.saved.set_text(&e),
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.sidebar.connect_nav(move |nav| {
            if let Some(w) = weak.upgrade() {
                w.show(nav);
            }
        });
        let weak = Rc::downgrade(self);
        model_chip.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.sidebar.go(Nav::Settings);
            }
        });
        let weak = Rc::downgrade(self);
        self.dictation.connect_saved(move |msg| {
            if let Some(w) = weak.upgrade() {
                w.saved.set_text(msg);
            }
        });
        let weak = Rc::downgrade(self);
        self.dictation.connect_document_changed(move || {
            if let Some(w) = weak.upgrade() {
                w.update_crumbs();
                w.sidebar.refresh();
            }
        });

        self.add_action("toggle-dictation", &["<Control>space"], |w| {
            w.sidebar.go(Nav::Dictate);
            w.dictation.toggle_recording();
        });
        self.add_action("new-document", &["<Control>n"], |w| w.new_document());
        let weak = Rc::downgrade(self);
        new_doc.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.new_document();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(w) = weak.upgrade() {
                w.dictation.save_now();
            }
            gtk::glib::Propagation::Proceed
        });
    }

    fn add_action(self: &Rc<Self>, name: &str, accels: &[&str], run: impl Fn(&Rc<Self>) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let weak = Rc::downgrade(self);
        action.connect_activate(move |_, _| {
            if let Some(w) = weak.upgrade() {
                run(&w);
            }
        });
        self.window.add_action(&action);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }
    }

    /// Registers keyboard shortcuts once the window has an application.
    pub fn install_accels(&self, app: &adw::Application) {
        app.set_accels_for_action("win.toggle-dictation", &["<Control>space"]);
        app.set_accels_for_action("win.new-document", &["<Control>n"]);
    }

    fn settings_snapshot(&self) -> crate::config::Settings {
        self.settings.settings()
    }

    pub fn open_in_editor(&self, doc: crate::store::DocumentId) {
        match self.dictation.open_document(doc) {
            Ok(()) => {
                self.sidebar.set_active(&Nav::Dictate);
                self.stack.set_visible_child_name("dictate");
                self.update_crumbs();
            }
            Err(e) => self.saved.set_text(&e),
        }
    }

    /// Opens the Export screen for the document in the editor.
    pub fn export_current(&self) {
        self.dictation.save_now();
        if let Some(doc) = self.dictation.document() {
            self.export.show(Target::Document(doc));
            self.sidebar.set_active(&Nav::Dictate);
            self.stack.set_visible_child_name("export");
        }
    }

    fn new_document(self: &Rc<Self>) {
        match self.dictation.new_document(None) {
            Ok(_) => self.sidebar.go(Nav::Dictate),
            Err(e) => self.saved.set_text(&e),
        }
    }

    fn open_initial_document(self: &Rc<Self>) {
        let latest = self
            .store
            .documents(&DocumentFilter::default())
            .ok()
            .and_then(|d| d.first().map(|d| d.id));
        let result = match latest {
            Some(id) => self.dictation.open_document(id),
            None => self.dictation.new_document(None).map(|_| ()),
        };
        if let Err(e) = result {
            self.saved.set_text(&e);
        }
        self.update_crumbs();
    }

    fn update_crumbs(&self) {
        self.crumb_project.set_text(&self.dictation.project_name());
        self.crumb_title.set_text(self.dictation.title.text().trim());
    }

    pub fn show(&self, nav: Nav) {
        match &nav {
            Nav::Project(f) => self.project.show(Scope::Project(*f)),
            Nav::Tag(t) => self.project.show(Scope::Tag(t.clone())),
            Nav::Templates => self.templates.reload(None),
            _ => {}
        }
        let page = match nav {
            Nav::Dictate => "dictate",
            Nav::Files => "files",
            Nav::Templates => "templates",
            Nav::Settings => "settings",
            Nav::Project(_) | Nav::Tag(_) => "project",
        };
        self.stack.set_visible_child_name(page);
    }

    pub fn visible_page(&self) -> String {
        self.stack
            .visible_child_name()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }
}

fn model_label(settings: &crate::config::Settings) -> String {
    let name = settings.model.trim_end_matches(".bin").to_string();
    let backend = match settings.backend {
        crate::config::Backend::Auto => "Auto",
        crate::config::Backend::Cuda => "CUDA",
        crate::config::Backend::Vulkan => "Vulkan",
        crate::config::Backend::Cpu => "CPU",
    };
    format!("{name} · {backend}")
}
