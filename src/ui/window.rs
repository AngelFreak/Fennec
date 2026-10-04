//! The main window: header, sidebar and the screen stack.
//!
//! The header changes with the screen, as in the design: the main screens
//! show the logo and breadcrumbs, sub-screens (Export, Clean up) a back
//! button and a title. Each screen can put buttons on the right through
//! its `header_actions` box.

use std::cell::RefCell;
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

/// Screens that replace the whole window body and come with a back button.
const SUB_PAGES: [&str; 2] = ["export", "cleanup"];

type PageSwitch = RefCell<Option<Rc<dyn Fn(&str)>>>;

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    pub stack: gtk::Stack,
    pub dictation: Rc<DictationPage>,
    pub files: Rc<FilesPage>,
    pub export: Rc<ExportPage>,
    pub project: Rc<ProjectPage>,
    pub templates: Rc<TemplatesPage>,
    pub settings: Rc<SettingsPage>,
    model_chips: Vec<gtk::Button>,
    pub sidebar: Rc<Sidebar>,
    pub store: Rc<Store>,
    outer: gtk::Stack,
    brand: gtk::Box,
    back: gtk::Button,
    crumb_project: gtk::Label,
    crumb_sep: gtk::Label,
    crumb_title: gtk::Label,
    saved: gtk::Label,
    header_actions: gtk::Stack,
    /// Where Back returns to from a sub-screen.
    return_to: RefCell<String>,
    sub_page_switch: PageSwitch,
}

impl MainWindow {
    pub fn new(deps: Deps) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .title("Fennec")
            .default_width(1280)
            .default_height(800)
            .width_request(1024)
            .height_request(600)
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
        brand.append(&logo());
        brand.append(&label("Fennec", &["fx-app-name"]));
        header.append(&brand);

        let back = icon_button("fennec-back-symbolic", "Back", &["fx-icon-button"]);
        back.set_size_request(40, 40);
        back.set_valign(gtk::Align::Center);
        back.set_visible(false);
        header.append(&back);

        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        crumbs.set_hexpand(true);
        crumbs.set_valign(gtk::Align::Center);
        let crumb_project = label("", &["fx-crumb"]);
        let crumb_sep = label("/", &["fx-crumb-sep"]);
        let crumb_title = label("", &["fx-crumb-current"]);
        crumb_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let saved = label("", &["fx-saved"]);
        saved.set_ellipsize(gtk::pango::EllipsizeMode::End);
        crumbs.append(&crumb_project);
        crumbs.append(&crumb_sep);
        crumbs.append(&crumb_title);
        crumbs.append(&saved);
        header.append(&crumbs);

        let header_actions = gtk::Stack::new();
        header_actions.set_hhomogeneous(false);
        header_actions.set_vhomogeneous(false);
        header_actions.set_interpolate_size(false);
        header_actions.set_valign(gtk::Align::Center);
        header.append(&header_actions);
        // The window-manager controls stay available.
        header.append(&gtk::WindowControls::new(gtk::PackType::End));
        let handle = gtk::WindowHandle::builder().child(&header).build();

        let sidebar = Sidebar::new(Rc::clone(&store));
        let engine = EngineHolder::new(deps.clone());
        // Load the model in the background now, so the first recording
        // starts at once. A failure shows when recording is tried.
        engine.with_worker(|r| {
            if let Err(e) = r {
                tracing::info!("the speech model is not ready: {e}");
            }
        });
        let dictation = DictationPage::new(Rc::clone(&store), deps.clone(), Rc::clone(&engine));
        let files = FilesPage::new(Rc::clone(&store), deps.clone(), Rc::clone(&engine));
        let export = ExportPage::new(Rc::clone(&store), deps.clone());
        let project = ProjectPage::new(Rc::clone(&store), deps.clone());
        let templates = TemplatesPage::new(deps.paths.templates());
        sidebar.add_context("templates", &templates.list_panel);
        let settings = SettingsPage::new(deps.clone(), Rc::clone(&engine));

        // Header actions per screen; the model chip appears on two of them.
        let text = model_label(&deps.settings());
        let model_chips: Vec<gtk::Button> = (0..2).map(|_| model_chip(&text)).collect();
        let dictate_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        dictate_actions.append(&dictation.ai_menu);
        dictate_actions.append(&model_chips[0]);
        let new_doc = icon_button(
            "document-new-symbolic",
            "New dictation (Ctrl+N)",
            &["fx-header-chip"],
        );
        new_doc.set_valign(gtk::Align::Center);
        dictate_actions.append(&new_doc);
        let export_button = new_export_button();
        dictate_actions.append(&export_button);
        header_actions.add_named(&dictate_actions, Some("dictate"));
        let files_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        files_actions.append(&model_chips[1]);
        let files_export = new_export_button();
        files_actions.append(&files_export);
        files_actions.append(&files.header_actions);
        header_actions.add_named(&files_actions, Some("files"));
        header_actions.add_named(&export.header_actions, Some("export"));
        header_actions.add_named(&project.header_actions, Some("project"));
        header_actions.add_named(&templates.header_actions, Some("templates"));
        header_actions.add_named(&settings.header_actions, Some("settings"));
        header_actions.add_named(&dictation.cleanup.header_actions, Some("cleanup"));

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.add_named(&dictation.root, Some("dictate"));
        stack.add_named(&files.root, Some("files"));
        stack.add_named(&project.root, Some("project"));
        stack.add_named(&templates.root, Some("templates"));
        stack.add_named(&settings.root, Some("settings"));

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        body.append(&sidebar.root);
        body.append(&stack);
        // Sub-screens take the whole body, without the sidebar.
        let outer = gtk::Stack::new();
        outer.set_vexpand(true);
        outer.add_named(&body, Some("main"));
        outer.add_named(&export.root, Some("export"));
        outer.add_named(&dictation.cleanup.root, Some("cleanup"));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&handle);
        content.append(&outer);
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
            model_chips,
            sidebar,
            store,
            outer,
            brand,
            back,
            crumb_project,
            crumb_sep,
            crumb_title,
            saved,
            header_actions,
            return_to: RefCell::new("dictate".into()),
            sub_page_switch: RefCell::default(),
        });
        win.wire(&new_doc, &export_button, &files_export);
        win.update_header();
        if show {
            win.open_initial_document();
        }
        win
    }

    fn wire(self: &Rc<Self>, new_doc: &gtk::Button, export_button: &gtk::Button, files_export: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.stack.connect_visible_child_name_notify(move |_| {
            if let Some(w) = weak.upgrade() {
                w.update_header();
                w.update_sidebar_context();
            }
        });
        let weak = Rc::downgrade(self);
        self.back.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.close_sub_page();
            }
        });
        let weak = Rc::downgrade(self);
        *self.sub_page_switch.borrow_mut() = Some(Rc::new(move |page: &str| {
            if let Some(w) = weak.upgrade() {
                if w.outer.visible_child_name().as_deref() == Some("main") {
                    *w.return_to.borrow_mut() = w.visible_page_in_stack();
                }
                w.outer.set_visible_child_name(page);
                w.update_header();
            }
        }));

        let weak = Rc::downgrade(self);
        export_button.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.export_current();
            }
        });
        let weak = Rc::downgrade(self);
        files_export.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade()
                && let Some(doc) = w.files.current_document()
            {
                w.export.show(Target::Document(doc));
                w.open_sub_page("export");
            }
        });
        let weak = Rc::downgrade(self);
        self.settings.connect_model_changed(move |()| {
            if let Some(w) = weak.upgrade() {
                let text = model_label(&w.settings_snapshot());
                for chip in &w.model_chips {
                    set_chip_text(chip, &text);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.settings.connect_ai_changed(move |()| {
            if let Some(w) = weak.upgrade() {
                w.dictation.refresh_ai();
                w.project.refresh_ai();
            }
        });
        let weak = Rc::downgrade(self);
        self.dictation.connect_navigate(move |page| {
            if let Some(w) = weak.upgrade() {
                if SUB_PAGES.contains(&page) {
                    w.open_sub_page(page);
                } else if page == "settings" {
                    w.sidebar.go(Nav::Settings);
                } else {
                    w.close_sub_page();
                    w.sidebar.set_active(&Nav::Dictate);
                    w.stack.set_visible_child_name(page);
                }
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
                w.export.set_table_of_contents(w.project.table_of_contents());
                w.open_sub_page("export");
            }
        });
        let weak = Rc::downgrade(self);
        self.project.connect_changed(move |()| {
            if let Some(w) = weak.upgrade() {
                w.sidebar.refresh();
                w.update_header();
            }
        });
        let weak = Rc::downgrade(self);
        self.project.connect_editor_state(move || {
            weak.upgrade().map_or((None, false), |w| {
                (w.dictation.document(), w.dictation.is_recording())
            })
        });
        // The editor must not keep a document that was just deleted.
        let weak = Rc::downgrade(self);
        self.project.connect_deleted(move |ids| {
            let Some(w) = weak.upgrade() else { return };
            if w.dictation.document().is_some_and(|d| ids.contains(&d)) {
                w.open_initial_document();
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
        self.files.connect_header_changed(move || {
            if let Some(w) = weak.upgrade() {
                w.update_header();
            }
        });
        let weak = Rc::downgrade(self);
        self.sidebar.connect_nav(move |nav| {
            if let Some(w) = weak.upgrade() {
                w.show(nav);
            }
        });
        for chip in &self.model_chips {
            let weak = Rc::downgrade(self);
            chip.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.sidebar.go(Nav::Settings);
                    w.settings.show_section("model");
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.dictation.connect_saved(move |msg| {
            if let Some(w) = weak.upgrade()
                && w.visible_page() == "dictate"
            {
                w.saved.set_text(msg);
            }
        });
        let weak = Rc::downgrade(self);
        self.dictation.connect_document_changed(move || {
            if let Some(w) = weak.upgrade() {
                w.update_header();
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
        self.project.connect_new_dictation(move |project| {
            let Some(w) = weak.upgrade() else { return };
            match w.dictation.new_document(project) {
                Ok(_) => w.sidebar.go(Nav::Dictate),
                Err(e) => w.saved.set_text(&e),
            }
        });
        let weak = Rc::downgrade(self);
        self.project.connect_import(move |()| {
            let Some(w) = weak.upgrade() else { return };
            w.sidebar.go(Nav::Files);
            w.files.choose_files();
        });
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(w) = weak.upgrade() {
                w.before_close();
            }
            gtk::glib::Propagation::Proceed
        });
    }

    /// Saves the document and carries out a delete still waiting for Undo:
    /// on closing the window and on quitting (Ctrl+Q skips close-request).
    pub fn before_close(&self) {
        self.dictation.save_now();
        self.project.finish_delete();
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

    fn open_sub_page(&self, page: &str) {
        let switch = self.sub_page_switch.borrow().clone();
        if let Some(f) = switch {
            f(page);
        }
    }

    /// Leaves Export or Clean up for the screen it was opened from.
    pub fn close_sub_page(&self) {
        if self.outer.visible_child_name().as_deref() != Some("main") {
            let to = self.return_to.borrow().clone();
            self.outer.set_visible_child_name("main");
            self.stack.set_visible_child_name(&to);
        }
        self.update_header();
    }

    pub fn open_in_editor(&self, doc: crate::store::DocumentId) {
        match self.dictation.open_document(doc) {
            Ok(()) => {
                self.close_sub_page();
                self.sidebar.set_active(&Nav::Dictate);
                self.stack.set_visible_child_name("dictate");
                self.update_header();
            }
            Err(e) => self.saved.set_text(&e),
        }
    }

    /// Opens the Export screen for the document in the editor.
    pub fn export_current(&self) {
        self.dictation.save_now();
        if let Some(doc) = self.dictation.document() {
            self.export.show(Target::Document(doc));
            self.open_sub_page("export");
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
        self.update_header();
    }

    /// Breadcrumbs and actions for the visible screen.
    fn update_header(&self) {
        let page = self.visible_page();
        let sub = SUB_PAGES.contains(&page.as_str());
        self.brand.set_visible(!sub);
        self.back.set_visible(sub);
        if self.header_actions.child_by_name(&page).is_some() {
            self.header_actions.set_visible_child_name(&page);
        }
        let (project, title, meta) = match page.as_str() {
            "dictate" => (
                Some(self.dictation.project_name()),
                self.dictation.title.text().trim().to_string(),
                None,
            ),
            "files" => {
                let (p, t, m) = self.files.header_info();
                (p, t, Some(m))
            }
            "export" => (None, "Export".to_string(), Some(self.export.header_subtitle())),
            "cleanup" => (
                None,
                "Clean up text".to_string(),
                Some(self.dictation.title.text().trim().to_string()),
            ),
            "project" => (
                Some("Projects".to_string()),
                self.project.title_text(),
                Some(String::new()),
            ),
            "templates" => (None, "Templates".to_string(), Some(String::new())),
            "settings" => (None, "Settings".to_string(), Some(String::new())),
            _ => (None, String::new(), Some(String::new())),
        };
        self.crumb_project.set_visible(project.is_some());
        self.crumb_sep.set_visible(project.is_some());
        self.crumb_project.set_text(project.as_deref().unwrap_or(""));
        self.crumb_title.set_text(&title);
        // On Dictate the label shows the save state, set by the page.
        if let Some(meta) = meta {
            self.saved.set_text(&meta);
        }
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
        self.close_sub_page();
        self.stack.set_visible_child_name(page);
        self.update_header();
    }

    /// As in the mockup: Templates lists its templates in the sidebar,
    /// Settings has its own section list, the rest show projects and tags.
    fn update_sidebar_context(&self) {
        let context = match self.visible_page_in_stack().as_str() {
            "templates" => "templates",
            "settings" => "none",
            _ => "projects",
        };
        self.sidebar.show_context(context);
    }

    fn visible_page_in_stack(&self) -> String {
        self.stack
            .visible_child_name()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// The screen shown: a sub-screen when one is open, else the stack's.
    pub fn visible_page(&self) -> String {
        match self.outer.visible_child_name() {
            Some(name) if name != "main" => name.to_string(),
            _ => self.visible_page_in_stack(),
        }
    }

    /// The header's title and subtitle (tests).
    /// The breadcrumb: project (when shown) and title.
    pub fn header_crumbs(&self) -> (Option<String>, String) {
        let project = self
            .crumb_project
            .get_visible()
            .then(|| self.crumb_project.text().to_string());
        (project, self.crumb_title.text().to_string())
    }

    pub fn header_texts(&self) -> (String, String) {
        (self.crumb_title.text().to_string(), self.saved.text().to_string())
    }

    /// Model chip text (tests).
    pub fn model_chip_text(&self) -> String {
        chip_text(&self.model_chips[0])
    }
}

/// The fox-ear mark from the design, in an orange rounded square.
fn logo() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(26)
        .content_height(26)
        .valign(gtk::Align::Center)
        .css_classes(["fx-logo"])
        .build();
    area.update_property(&[gtk::accessible::Property::Label("Fennec")]);
    area.set_draw_func(|area, cr, w, h| {
        let c = area.color();
        let (w, h) = (f64::from(w), f64::from(h));
        let r = 7.0;
        use std::f64::consts::{FRAC_PI_2, PI};
        cr.new_sub_path();
        cr.arc(w - r, r, r, -FRAC_PI_2, 0.0);
        cr.arc(w - r, h - r, r, 0.0, FRAC_PI_2);
        cr.arc(r, h - r, r, FRAC_PI_2, PI);
        cr.arc(r, r, r, PI, 1.5 * PI);
        cr.close_path();
        cr.set_source_rgba(c.red().into(), c.green().into(), c.blue().into(), 1.0);
        let _ = cr.fill();
        // M4 20 L7 4 L12 12 L17 4 L20 20 Z, from a 24-unit box drawn 16 px wide.
        let s = 16.0 / 24.0;
        let (ox, oy) = ((w - 16.0) / 2.0, (h - 16.0) / 2.0);
        let pts = [(4.0, 20.0), (7.0, 4.0), (12.0, 12.0), (17.0, 4.0), (20.0, 20.0)];
        cr.move_to(ox + pts[0].0 * s, oy + pts[0].1 * s);
        for (x, y) in &pts[1..] {
            cr.line_to(ox + x * s, oy + y * s);
        }
        cr.close_path();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.fill();
    });
    area
}

fn model_chip(text: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("fx-header-chip");
    b.set_valign(gtk::Align::Center);
    b.set_tooltip_text(Some("Speech model and compute (Settings)"));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&gtk::Image::from_icon_name("fennec-cpu-symbolic"));
    row.append(&label(text, &[]));
    b.set_child(Some(&row));
    b
}

fn set_chip_text(chip: &gtk::Button, text: &str) {
    if let Some(l) = chip
        .child()
        .and_then(|row| row.last_child())
        .and_downcast::<gtk::Label>()
    {
        l.set_text(text);
    }
}

fn chip_text(chip: &gtk::Button) -> String {
    chip.child()
        .and_then(|row| row.last_child())
        .and_downcast::<gtk::Label>()
        .map(|l| l.text().to_string())
        .unwrap_or_default()
}

fn new_export_button() -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("fx-primary");
    b.set_valign(gtk::Align::Center);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&gtk::Image::from_icon_name("fennec-download-symbolic"));
    row.append(&label("Export", &[]));
    b.set_child(Some(&row));
    b
}

/// "Edda v0.2 · Vulkan": the model's catalog name where known, and the
/// compute it will use.
fn model_label(settings: &crate::config::Settings) -> String {
    let name = crate::models::catalog()
        .into_iter()
        .find(|m| m.file_name == settings.model)
        .map(|m| m.name.to_string())
        .unwrap_or_else(|| settings.model.trim_end_matches(".bin").to_string());
    let gpu = crate::models::wants_gpu(settings.backend);
    let backend = match settings.backend {
        crate::config::Backend::Cuda if gpu => "CUDA",
        crate::config::Backend::Vulkan if gpu => "Vulkan",
        crate::config::Backend::Auto if gpu && cfg!(feature = "cuda") => "CUDA",
        crate::config::Backend::Auto if gpu => "Vulkan",
        _ => "CPU",
    };
    format!("{name} · {backend}")
}
