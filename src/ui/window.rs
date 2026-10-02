//! The main window: header, sidebar and the screen stack.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use super::dictation::DictationPage;
use super::sidebar::{Nav, Sidebar};
use super::{Deps, icon_button, label};
use crate::store::{DocumentFilter, Store};

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    pub stack: gtk::Stack,
    pub dictation: Rc<DictationPage>,
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

        let model_chip = gtk::Button::with_label(&model_label(&deps));
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
        // The window-manager controls stay available.
        header.append(&gtk::WindowControls::new(gtk::PackType::End));
        let handle = gtk::WindowHandle::builder().child(&header).build();

        let sidebar = Sidebar::new(Rc::clone(&store));
        let dictation = DictationPage::new(Rc::clone(&store), deps.clone());

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.add_named(&dictation.root, Some("dictate"));
        for (name, title) in [
            ("files", "Files"),
            ("templates", "Templates"),
            ("project", "Project"),
            ("settings", "Settings"),
        ] {
            let page = adw::StatusPage::builder()
                .title(title)
                .description("This screen is being built.")
                .build();
            stack.add_named(&page, Some(name));
        }

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
            sidebar,
            store,
            crumb_project,
            crumb_title,
            saved,
        });
        win.wire(&model_chip, &new_doc);
        if show {
            win.open_initial_document();
        }
        win
    }

    fn wire(self: &Rc<Self>, model_chip: &gtk::Button, new_doc: &gtk::Button) {
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

fn model_label(deps: &Deps) -> String {
    let name = deps.settings.model.trim_end_matches(".bin").to_string();
    let backend = match deps.settings.backend {
        crate::config::Backend::Auto => "Auto",
        crate::config::Backend::Cuda => "CUDA",
        crate::config::Backend::Vulkan => "Vulkan",
        crate::config::Backend::Cpu => "CPU",
    };
    format!("{name} · {backend}")
}
