//! Left column: screens, projects (with counts), tags, and Settings.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;

use super::label;
use crate::store::{ProjectFilter, Store};

#[derive(Debug, Clone, PartialEq)]
pub enum Nav {
    Dictate,
    Files,
    Templates,
    Settings,
    Project(ProjectFilter),
    Tag(String),
}

const PROJECT_COLORS: [&str; 5] = ["#C2410C", "#1D4ED8", "#0F766E", "#6B21A8", "#9AA1AE"];

pub struct Sidebar {
    pub root: gtk::Box,
    nav: HashMap<&'static str, gtk::Button>,
    projects: gtk::Box,
    tags: gtk::Box,
    /// What sits under the screens: projects and tags, a screen's own list,
    /// or nothing.
    context: gtk::Stack,
    store: Rc<Store>,
    on_nav: super::Handler<Nav>,
    active: RefCell<Nav>,
}

impl Sidebar {
    pub fn new(store: Rc<Store>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("fx-sidebar");
        root.set_size_request(232, -1);
        // Children that expand (the section titles) must not widen the sidebar.
        root.set_hexpand(false);

        let mut nav = HashMap::new();
        for (key, text, icon) in [
            ("dictate", "Dictate", "fennec-mic-symbolic"),
            ("files", "Files", "fennec-upload-symbolic"),
            ("templates", "Templates", "fennec-templates-symbolic"),
        ] {
            let b = nav_button(text, icon);
            root.append(&b);
            nav.insert(key, b);
        }

        let context = gtk::Stack::new();
        context.set_hhomogeneous(false);
        context.set_vhomogeneous(false);
        context.set_vexpand(true);
        context.set_margin_top(18);
        let projects_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        context.add_named(&projects_box, Some("projects"));
        context.add_named(&gtk::Box::new(gtk::Orientation::Vertical, 0), Some("none"));
        root.append(&context);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let title = label("PROJECTS", &["fx-section-title"]);
        title.set_hexpand(true);
        head.append(&title);
        let add = gtk::MenuButton::builder()
            .icon_name("fennec-add-symbolic")
            .tooltip_text("New project")
            .build();
        add.add_css_class("flat");
        add.set_valign(gtk::Align::End);
        add.update_property(&[gtk::accessible::Property::Label("New project")]);
        head.append(&add);
        projects_box.append(&head);

        let projects = gtk::Box::new(gtk::Orientation::Vertical, 2);
        projects_box.append(&projects);

        let tags_title = label("TAGS", &["fx-section-title"]);
        tags_title.set_margin_top(20);
        projects_box.append(&tags_title);
        let tags = super::wrap::wrap_box();
        tags.set_margin_start(12);
        tags.set_margin_end(12);
        projects_box.append(&tags);

        let settings = nav_button("Settings", "fennec-settings-symbolic");
        root.append(&settings);
        nav.insert("settings", settings);

        let sidebar = Rc::new(Self {
            root,
            nav,
            projects,
            tags,
            context,
            store,
            on_nav: RefCell::default(),
            active: RefCell::new(Nav::Dictate),
        });
        sidebar.wire(&add);
        sidebar.refresh();
        sidebar.set_active(&Nav::Dictate);
        sidebar
    }

    /// Adds a screen's own list, shown in place of projects and tags.
    pub fn add_context(&self, name: &str, widget: &impl IsA<gtk::Widget>) {
        self.context.add_named(widget, Some(name));
    }

    /// Shows "projects", "none" or a list added with [`Self::add_context`].
    pub fn show_context(&self, name: &str) {
        self.context.set_visible_child_name(name);
    }

    pub fn context(&self) -> String {
        self.context
            .visible_child_name()
            .map(|n| n.to_string())
            .unwrap_or_default()
    }

    fn wire(self: &Rc<Self>, add: &gtk::MenuButton) {
        for (key, nav) in [
            ("dictate", Nav::Dictate),
            ("files", Nav::Files),
            ("templates", Nav::Templates),
            ("settings", Nav::Settings),
        ] {
            let weak = Rc::downgrade(self);
            self.nav[key].connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.go(nav.clone());
                }
            });
        }

        let pop = gtk::Popover::new();
        let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
        b.set_margin_top(8);
        b.set_margin_bottom(8);
        b.set_margin_start(8);
        b.set_margin_end(8);
        let entry = gtk::Entry::builder().placeholder_text("Project name").build();
        let create = gtk::Button::with_label("Create project");
        create.add_css_class("fx-primary");
        b.append(&entry);
        b.append(&create);
        pop.set_child(Some(&b));
        add.set_popover(Some(&pop));
        let weak = Rc::downgrade(self);
        let make = {
            let entry = entry.clone();
            let pop = pop.clone();
            move || {
                let name = entry.text().trim().to_string();
                let Some(s) = weak.upgrade() else { return };
                if name.is_empty() {
                    return;
                }
                let color =
                    PROJECT_COLORS[s.store.projects().map(|p| p.len()).unwrap_or(0) % PROJECT_COLORS.len()];
                match s.store.create_project(&name, color) {
                    Ok(id) => {
                        entry.set_text("");
                        pop.popdown();
                        s.refresh();
                        s.go(Nav::Project(ProjectFilter::Project(id)));
                    }
                    Err(e) => tracing::error!("creating project {name:?}: {e}"),
                }
            }
        };
        let m = make.clone();
        create.connect_clicked(move |_| m());
        entry.connect_activate(move |_| make());
    }

    pub fn connect_nav(&self, f: impl Fn(Nav) + 'static) {
        *self.on_nav.borrow_mut() = Some(Rc::new(f));
    }

    pub fn go(&self, nav: Nav) {
        self.set_active(&nav);
        if let Some(f) = self.on_nav.borrow().clone() {
            f(nav);
        }
    }

    pub fn set_active(&self, nav: &Nav) {
        *self.active.borrow_mut() = nav.clone();
        let key = match nav {
            Nav::Dictate => Some("dictate"),
            Nav::Files => Some("files"),
            Nav::Templates => Some("templates"),
            Nav::Settings => Some("settings"),
            _ => None,
        };
        for (k, b) in &self.nav {
            if Some(*k) == key {
                b.add_css_class("active");
            } else {
                b.remove_css_class("active");
            }
        }
        self.refresh();
    }

    /// Rebuilds the project and tag lists from the store.
    pub fn refresh(&self) {
        while let Some(c) = self.projects.first_child() {
            self.projects.remove(&c);
        }
        let projects = self.store.projects().unwrap_or_default();
        let all = self
            .store
            .documents(&Default::default())
            .map(|d| d.len())
            .unwrap_or(0);
        let unsorted: usize = all - projects.iter().map(|p| p.document_count).sum::<usize>();
        let active = self.active.borrow().clone();
        let mut rows: Vec<(String, String, usize, ProjectFilter)> =
            vec![("All documents".into(), "#9AA1AE".into(), all, ProjectFilter::All)];
        rows.extend(
            projects
                .into_iter()
                .map(|p| (p.name, p.color, p.document_count, ProjectFilter::Project(p.id))),
        );
        rows.push((
            "Unsorted".into(),
            "#C9CED6".into(),
            unsorted,
            ProjectFilter::Unsorted,
        ));
        for (name, color, count, filter) in rows {
            let row = gtk::Button::new();
            row.add_css_class("fx-project-row");
            if active == Nav::Project(filter) {
                row.add_css_class("active");
            }
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let swatch = gtk::DrawingArea::builder()
                .content_width(10)
                .content_height(10)
                .valign(gtk::Align::Center)
                .build();
            let rgba = gtk::gdk::RGBA::parse(color.as_str()).unwrap_or(gtk::gdk::RGBA::BLACK);
            swatch.set_draw_func(move |_, cr, w, h| {
                cr.set_source_rgba(rgba.red().into(), rgba.green().into(), rgba.blue().into(), 1.0);
                cr.rectangle(0.0, 0.0, w.into(), h.into());
                let _ = cr.fill();
            });
            let name_label = label(&name, &[]);
            name_label.set_hexpand(true);
            name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            b.append(&swatch);
            b.append(&name_label);
            b.append(&label(&count.to_string(), &["fx-count"]));
            row.set_child(Some(&b));
            row.update_property(&[gtk::accessible::Property::Label(&format!(
                "{name}, {count} documents"
            ))]);
            let on_nav = self.on_nav.borrow().clone();
            let nav = Nav::Project(filter);
            row.connect_clicked(move |_| {
                if let Some(f) = &on_nav {
                    f(nav.clone());
                }
            });
            self.projects.append(&row);
        }

        while let Some(c) = self.tags.first_child() {
            self.tags.remove(&c);
        }
        // Most used first, as in the mockup.
        let mut tags = self.store.tags().unwrap_or_default();
        tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (tag, _count) in tags {
            let b = gtk::Button::with_label(&format!("#{tag}"));
            b.add_css_class("fx-tag");
            let on_nav = self.on_nav.borrow().clone();
            b.connect_clicked(move |_| {
                if let Some(f) = &on_nav {
                    f(Nav::Tag(tag.clone()));
                }
            });
            self.tags.append(&b);
        }
    }

    /// Project row names in order (tests).
    pub fn project_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut child = self.projects.first_child();
        while let Some(c) = child {
            if let Some(name) = super::texts_in(&c).first() {
                out.push(name.clone());
            }
            child = c.next_sibling();
        }
        out
    }
}

fn nav_button(text: &str, icon: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("fx-nav");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.append(&gtk::Image::from_icon_name(icon));
    row.append(&label(text, &[]));
    b.set_child(Some(&row));
    b
}
