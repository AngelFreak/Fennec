//! The Templates screen: list, editor (fields and layout) and a preview.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

use super::label;
use crate::export::{Report, ReportParagraph, Section, preview_first_page};
use crate::template::{Field, FieldKind, Template, TemplateError, key_from_label, load_dir};

const KINDS: [(FieldKind, &str); 4] = [
    (FieldKind::Text, "Text"),
    (FieldKind::Date, "Date"),
    (FieldKind::List, "List"),
    (FieldKind::Multiline, "Multi-line"),
];

struct FieldRow {
    root: gtk::Box,
    label: gtk::Entry,
    kind: gtk::DropDown,
    default: gtk::Entry,
    required: gtk::CheckButton,
    options: Vec<String>,
}

pub struct TemplatesPage {
    pub root: gtk::Box,
    /// Buttons for the window header while this screen shows.
    pub header_actions: gtk::Box,
    dir: PathBuf,
    list: gtk::ListBox,
    current: RefCell<Option<Template>>,
    name: gtk::Entry,
    heading: gtk::Entry,
    footer: gtk::Entry,
    font: gtk::Entry,
    logo_label: gtk::Label,
    logo: RefCell<Option<PathBuf>>,
    fields_box: gtk::Box,
    rows: RefCell<Vec<FieldRow>>,
    error: gtk::Label,
    path_label: gtk::Label,
    preview: gtk::Picture,
    loading: Cell<bool>,
}

impl TemplatesPage {
    pub fn new(dir: PathBuf) -> Rc<Self> {
        let left = gtk::Box::new(gtk::Orientation::Vertical, 8);
        left.add_css_class("fx-template-list");
        left.set_size_request(190, -1);
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let new_button = gtk::Button::with_label("New template");
        new_button.add_css_class("fx-secondary");
        left.append(&label("TEMPLATES", &["fx-section-title"]));
        left.append(&list);
        left.append(&new_button);

        let editor = gtk::Box::new(gtk::Orientation::Vertical, 18);
        editor.add_css_class("fx-template-editor");
        editor.set_hexpand(true);
        let name = entry("Name");
        let heading = entry("Document heading");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        top.set_homogeneous(true);
        top.append(&field("Name", &name));
        top.append(&field("Document heading", &heading));
        editor.append(&top);

        let fields_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ft = label("Fields", &["fx-crumb-current"]);
        ft.set_hexpand(true);
        let add_field = gtk::Button::with_label("Add field");
        add_field.add_css_class("fx-secondary");
        fields_head.append(&ft);
        fields_head.append(&add_field);
        editor.append(&fields_head);
        let fields_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        editor.append(&fields_box);
        editor.append(&label(
            "Defaults may use {today}, {user}, {duration} and {model}.",
            &["fx-field-note"],
        ));

        editor.append(&label("Layout", &["fx-crumb-current"]));
        let logo_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let logo_label = label("No logo", &["fx-field-note"]);
        logo_label.set_hexpand(true);
        let choose_logo = gtk::Button::with_label("Choose PNG…");
        choose_logo.add_css_class("fx-secondary");
        let clear_logo = gtk::Button::with_label("Remove");
        clear_logo.add_css_class("fx-secondary");
        logo_row.append(&logo_label);
        logo_row.append(&choose_logo);
        logo_row.append(&clear_logo);
        editor.append(&field("Logo", &logo_row));
        let font = entry("Body font");
        let footer = entry("Footer");
        footer.set_placeholder_text(Some("e.g. department, address or classification"));
        editor.append(&field("Body font", &font));
        editor.append(&field("Footer", &footer));

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let error = label("", &["fx-field-error"]);
        error.set_hexpand(true);
        error.set_wrap(true);
        let open_file = gtk::Button::with_label("Open as file");
        open_file.add_css_class("fx-secondary");
        let delete = gtk::Button::with_label("Delete");
        delete.add_css_class("fx-secondary");
        let save = gtk::Button::with_label("Save template");
        save.add_css_class("fx-primary");
        actions.set_halign(gtk::Align::End);
        editor.append(&error);
        actions.append(&delete);
        actions.append(&open_file);
        actions.append(&save);
        editor.append(&actions);
        let path_label = label("", &["fx-field-note"]);
        editor.append(&path_label);
        let editor_scroll = gtk::ScrolledWindow::builder()
            .child(&editor)
            .hexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        editor_scroll.set_size_request(560, -1);

        let preview = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .build();
        preview.add_css_class("fx-page-preview");
        let preview_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        preview_box.add_css_class("fx-preview-area");
        preview_box.set_size_request(260, -1);
        preview_box.append(&label("PREVIEW", &["fx-section-title"]));
        preview_box.append(&preview);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&left);
        root.append(&editor_scroll);
        root.append(&preview_box);

        let page = Rc::new(Self {
            root,
            header_actions: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            dir,
            list,
            current: RefCell::default(),
            name,
            heading,
            footer,
            font,
            logo_label,
            logo: RefCell::default(),
            fields_box,
            rows: RefCell::default(),
            error,
            path_label,
            preview,
            loading: Cell::new(false),
        });
        page.wire(
            &new_button,
            &add_field,
            &save,
            &delete,
            &open_file,
            &choose_logo,
            &clear_logo,
        );
        page
    }

    #[allow(clippy::too_many_arguments)]
    fn wire(
        self: &Rc<Self>,
        new_button: &gtk::Button,
        add_field: &gtk::Button,
        save: &gtk::Button,
        delete: &gtk::Button,
        open_file: &gtk::Button,
        choose_logo: &gtk::Button,
        clear_logo: &gtk::Button,
    ) {
        let on = |b: &gtk::Button, f: fn(&Rc<Self>)| {
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    f(&p);
                }
            });
        };
        on(new_button, |p| p.new_template());
        on(add_field, |p| {
            p.add_field_row(&Field {
                key: String::new(),
                label: "Nyt felt".into(),
                kind: FieldKind::Text,
                default: String::new(),
                required: false,
                options: vec![],
            });
            p.update_preview();
        });
        on(save, |p| {
            let _ = p.save();
        });
        on(delete, |p| p.delete());
        on(clear_logo, |p| {
            *p.logo.borrow_mut() = None;
            p.show_logo();
            p.update_preview();
        });
        let weak = Rc::downgrade(self);
        open_file.connect_clicked(move |b| {
            let Some(p) = weak.upgrade() else { return };
            let Some(t) = p.current.borrow().clone() else {
                return;
            };
            let launcher =
                gtk::FileLauncher::new(Some(&gio::File::for_path(p.dir.join(format!("{}.toml", t.id)))));
            launcher.launch(
                b.root().and_downcast::<gtk::Window>().as_ref(),
                gio::Cancellable::NONE,
                |_| {},
            );
        });
        let weak = Rc::downgrade(self);
        choose_logo.connect_clicked(move |b| {
            let Some(p) = weak.upgrade() else { return };
            let filter = gtk::FileFilter::new();
            filter.add_mime_type("image/png");
            filter.set_name(Some("PNG images"));
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder()
                .title("Choose a logo")
                .filters(&filters)
                .modal(true)
                .build();
            let weak = Rc::downgrade(&p);
            dialog.open(
                b.root().and_downcast::<gtk::Window>().as_ref(),
                gio::Cancellable::NONE,
                move |res| {
                    if let (Ok(f), Some(p)) = (res, weak.upgrade())
                        && let Some(path) = f.path()
                    {
                        p.set_logo(&path);
                    }
                },
            );
        });
        for e in [&self.name, &self.heading, &self.footer, &self.font] {
            let weak = Rc::downgrade(self);
            e.connect_changed(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.update_preview();
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(p) = weak.upgrade() {
                p.open_index(row.index() as usize);
            }
        });
    }

    /// Reloads the template list and opens the first (or the given) one.
    pub fn reload(self: &Rc<Self>, select: Option<&str>) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let loaded = load_dir(&self.dir);
        for t in &loaded {
            let (title, sub) = match t {
                Ok(t) => (
                    t.name.clone(),
                    if t.fields.len() == 1 {
                        "1 field".into()
                    } else {
                        format!("{} fields", t.fields.len())
                    },
                ),
                Err(e) => (file_label(e), "Cannot be read: open the file to fix it".into()),
            };
            let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
            b.append(&label(&title, &["fx-field-label"]));
            b.append(&label(&sub, &["fx-stats"]));
            self.list.append(&b);
        }
        let index = select
            .and_then(|id| loaded.iter().position(|t| t.as_ref().is_ok_and(|t| t.id == id)))
            .or_else(|| loaded.iter().position(Result::is_ok));
        match index {
            Some(i) => self.open_index(i),
            None => self.new_template(),
        }
    }

    fn open_index(self: &Rc<Self>, i: usize) {
        let Some(Ok(t)) = load_dir(&self.dir).into_iter().nth(i) else {
            return;
        };
        if let Some(row) = self.list.row_at_index(i as i32) {
            self.list.select_row(Some(&row));
        }
        self.open(t);
    }

    fn open(self: &Rc<Self>, t: Template) {
        self.loading.set(true);
        self.name.set_text(&t.name);
        self.heading.set_text(&t.heading);
        self.footer.set_text(&t.footer);
        self.font.set_text(&t.body_font);
        *self.logo.borrow_mut() = t.logo_path();
        self.show_logo();
        while let Some(c) = self.fields_box.first_child() {
            self.fields_box.remove(&c);
        }
        self.rows.borrow_mut().clear();
        for f in &t.fields {
            self.add_field_row(f);
        }
        self.error.set_text("");
        self.path_label.set_text(
            &self
                .dir
                .join(format!("{}.toml", t.id))
                .to_string_lossy()
                .replacen(&std::env::var("HOME").unwrap_or_default(), "~", 1),
        );
        *self.current.borrow_mut() = Some(t);
        self.loading.set(false);
        self.update_preview();
    }

    fn new_template(self: &Rc<Self>) {
        let mut id = "ny_skabelon".to_string();
        let mut n = 2;
        while self.dir.join(format!("{id}.toml")).exists() {
            id = format!("ny_skabelon_{n}");
            n += 1;
        }
        let t = Template {
            id,
            name: "Ny skabelon".into(),
            heading: "NOTAT".into(),
            ..Template::blank()
        };
        self.list.unselect_all();
        self.open(t);
    }

    fn add_field_row(self: &Rc<Self>, f: &Field) {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        root.add_css_class("fx-field-row");
        let label_e = gtk::Entry::builder()
            .text(f.label.as_str())
            .hexpand(true)
            .width_chars(14)
            .build();
        label_e.update_property(&[gtk::accessible::Property::Label("Field label")]);
        let names: Vec<&str> = KINDS.iter().map(|(_, n)| *n).collect();
        let kind = gtk::DropDown::from_strings(&names);
        kind.set_selected(KINDS.iter().position(|(k, _)| *k == f.kind).unwrap_or(0) as u32);
        let default = gtk::Entry::builder()
            .text(f.default.as_str())
            .placeholder_text("Default")
            .width_chars(10)
            .build();
        let required = gtk::CheckButton::with_label("Required");
        required.set_active(f.required);
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.set_tooltip_text(Some("Remove field"));
        remove.add_css_class("flat");
        for w in [
            label_e.upcast_ref::<gtk::Widget>(),
            kind.upcast_ref(),
            default.upcast_ref(),
            required.upcast_ref(),
            remove.upcast_ref(),
        ] {
            root.append(w);
        }
        self.fields_box.append(&root);
        let weak = Rc::downgrade(self);
        let row_root = root.clone();
        remove.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.fields_box.remove(&row_root);
                p.rows.borrow_mut().retain(|r| r.root != row_root);
                p.update_preview();
            }
        });
        for e in [&label_e, &default] {
            let weak = Rc::downgrade(self);
            e.connect_changed(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.update_preview();
                }
            });
        }
        self.rows.borrow_mut().push(FieldRow {
            root,
            label: label_e,
            kind,
            default,
            required,
            options: f.options.clone(),
        });
    }

    fn set_logo(self: &Rc<Self>, path: &std::path::Path) {
        let Some(id) = self.current.borrow().as_ref().map(|t| t.id.clone()) else {
            return;
        };
        let target = self.dir.join(format!("{id}-logo.png"));
        match std::fs::create_dir_all(&self.dir).and_then(|_| std::fs::copy(path, &target)) {
            Ok(_) => *self.logo.borrow_mut() = Some(target),
            Err(e) => self.error.set_text(&format!("Could not copy the logo: {e}")),
        }
        self.show_logo();
        self.update_preview();
    }

    fn show_logo(&self) {
        let text = self
            .logo
            .borrow()
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        self.logo_label.set_text(text.as_deref().unwrap_or("No logo"));
    }

    /// The template as edited (not yet validated).
    fn edited(&self) -> Option<Template> {
        let mut t = self.current.borrow().clone()?;
        t.name = self.name.text().trim().to_string();
        t.heading = self.heading.text().trim().to_string();
        t.footer = self.footer.text().trim().to_string();
        let font = self.font.text().trim().to_string();
        t.body_font = if font.is_empty() {
            "Source Serif 4".into()
        } else {
            font
        };
        t.logo = self
            .logo
            .borrow()
            .as_ref()
            .map(|p| p.file_name().map(PathBuf::from).unwrap_or_else(|| p.clone()));
        t.dir = Some(self.dir.clone());
        let old = t.fields.clone();
        t.fields = self
            .rows
            .borrow()
            .iter()
            .map(|r| {
                let label = r.label.text().trim().to_string();
                let key = old
                    .iter()
                    .find(|f| f.label == label)
                    .map(|f| f.key.clone())
                    .unwrap_or_else(|| key_from_label(&label));
                Field {
                    key,
                    label,
                    kind: KINDS[(r.kind.selected() as usize).min(KINDS.len() - 1)].0,
                    default: r.default.text().to_string(),
                    required: r.required.is_active(),
                    options: r.options.clone(),
                }
            })
            .collect();
        Some(t)
    }

    /// Validates and writes the template; shows the problem if it is invalid.
    pub fn save(self: &Rc<Self>) -> Result<PathBuf, TemplateError> {
        let Some(t) = self.edited() else {
            return Err(TemplateError::Invalid {
                path: self.dir.clone(),
                message: "no template is open".into(),
            });
        };
        let path = self.dir.join(format!("{}.toml", t.id));
        let checked = Template::parse(&t.id, &t.to_toml(), &path);
        match checked.and_then(|_| t.save(&self.dir)) {
            Ok(p) => {
                self.error.set_text("");
                let id = t.id.clone();
                self.reload(Some(&id));
                Ok(p)
            }
            Err(e) => {
                let msg = e.to_string();
                self.error.set_text(msg.rsplit(": ").next().unwrap_or(&msg));
                Err(e)
            }
        }
    }

    fn delete(self: &Rc<Self>) {
        let Some(t) = self.current.borrow().clone() else {
            return;
        };
        let path = self.dir.join(format!("{}.toml", t.id));
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            self.error
                .set_text(&format!("Could not delete {}: {e}", path.display()));
            return;
        }
        self.reload(None);
    }

    fn update_preview(&self) {
        if self.loading.get() {
            return;
        }
        let Some(t) = self.edited() else { return };
        let section = Section {
            title: "Dokumentets titel".into(),
            date: String::new(),
            fields: t.fields.iter().map(|f| (f.label.clone(), format!("«{}»", f.key))).collect(),
            summary: None,
            paragraphs: vec![ReportParagraph {
                text: "Transskriptionen indsættes her. Hvert afsnit fra dikteringen bliver et afsnit i rapporten.".into(),
                timestamp: None,
                highlight: vec![],
            }],
        };
        let report = Report::from_template(&t, vec![section]);
        match preview_first_page(&report, 600) {
            Ok(s) => self.preview.set_paintable(Some(&super::export_page::texture(s))),
            Err(e) => self.error.set_text(&e.to_string()),
        }
    }

    pub fn names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut row = self.list.first_child();
        while let Some(r) = row {
            if let Some(t) = super::texts_in(&r).first() {
                out.push(t.clone());
            }
            row = r.next_sibling();
        }
        out
    }

    pub fn error_text(&self) -> String {
        self.error.text().to_string()
    }

    /// Test hooks: edit the open template like a user would.
    pub fn set_name(&self, name: &str) {
        self.name.set_text(name);
    }
    pub fn press_new(self: &Rc<Self>) {
        self.new_template();
    }
    pub fn add_field(self: &Rc<Self>, label: &str, required: bool) {
        self.add_field_row(&Field {
            key: String::new(),
            label: label.into(),
            kind: FieldKind::Text,
            default: String::new(),
            required,
            options: vec![],
        });
    }
    pub fn has_preview(&self) -> bool {
        self.preview.paintable().is_some()
    }
}

fn entry(name: &str) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.add_css_class("fx-field");
    e.update_property(&[gtk::accessible::Property::Label(name)]);
    e
}

fn field(name: &str, w: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&label(name, &["fx-field-label"]));
    b.append(w);
    b
}

fn file_label(e: &TemplateError) -> String {
    match e {
        TemplateError::Invalid { path, .. } | TemplateError::Io { path, .. } => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}
