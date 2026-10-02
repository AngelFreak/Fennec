//! The Templates screen: list, editor (fields and layout) and a preview.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::label;
use crate::export::{Report, ReportParagraph, Section, preview_first_page};
use crate::template::{Field, FieldKind, Template, TemplateError, key_from_label, load_dir};

const KINDS: [(FieldKind, &str); 4] = [
    (FieldKind::Text, "Text"),
    (FieldKind::Date, "Date"),
    (FieldKind::List, "List"),
    (FieldKind::Multiline, "Multi-line"),
];

/// Fixed widths of the field table's columns (label and default share the rest).
const HANDLE_W: i32 = 24;
const TYPE_W: i32 = 112;
const REQUIRED_W: i32 = 64;

/// A command from a field row's handle menu.
type RowAction = fn(&TemplatesPage, &gtk::Box);

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
    /// The template list; the window shows it in the sidebar.
    pub list_panel: gtk::ScrolledWindow,
    dir: PathBuf,
    list: gtk::Box,
    current: RefCell<Option<Template>>,
    name: gtk::Entry,
    heading: gtk::Entry,
    footer: gtk::Entry,
    font: gtk::Entry,
    size: gtk::SpinButton,
    font_label: gtk::Label,
    logo_slot: gtk::Stack,
    logo_picture: gtk::Picture,
    clear_logo: gtk::Button,
    logo: RefCell<Option<PathBuf>>,
    fields_box: gtk::Box,
    rows: RefCell<Vec<FieldRow>>,
    next_row: Cell<u32>,
    error: gtk::Label,
    path_label: gtk::Label,
    preview: gtk::Picture,
    loading: Cell<bool>,
}

impl TemplatesPage {
    pub fn new(dir: PathBuf) -> Rc<Self> {
        // Template list, shown in the sidebar while this screen is open.
        let left = gtk::Box::new(gtk::Orientation::Vertical, 4);
        left.add_css_class("fx-template-list");
        let list_head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        list_head.add_css_class("fx-template-list-head");
        let list_title = label("TEMPLATES", &["fx-section-title"]);
        list_title.set_hexpand(true);
        let new_button = super::icon_button("fennec-add-symbolic", "New template", &["fx-icon-button"]);
        list_head.append(&list_title);
        list_head.append(&new_button);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        left.append(&list_head);
        left.append(&list);
        let left_scroll = gtk::ScrolledWindow::builder()
            .child(&left)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        left_scroll.add_css_class("fx-template-list-scroll");
        left_scroll.set_vexpand(true);

        let editor = gtk::Box::new(gtk::Orientation::Vertical, 24);
        editor.add_css_class("fx-template-editor");
        editor.set_hexpand(true);
        let error = label("", &["fx-callout"]);
        error.set_wrap(true);
        error.set_visible(false);
        editor.append(&error);
        let name = entry("Name");
        let heading = entry("Document heading");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        top.set_homogeneous(true);
        top.append(&field("Name", &name));
        top.append(&field("Document heading", &heading));
        editor.append(&top);

        // Fields: a table of inline-editable rows.
        let fields_section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let fields_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ft = label("Fields", &["fx-h3"]);
        ft.set_hexpand(true);
        let add_field = icon_text_button("fennec-add-symbolic", "Add field");
        add_field.add_css_class("fx-secondary");
        add_field.add_css_class("fx-add-field");
        fields_head.append(&ft);
        fields_head.append(&add_field);
        fields_section.append(&fields_head);
        let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.add_css_class("fx-table");
        table.add_css_class("fx-field-table");
        table.set_overflow(gtk::Overflow::Hidden);
        let head = row_box(&["fx-table-head"]);
        head.append(&cell(
            gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast(),
            HANDLE_W,
        ));
        let head_label = label("LABEL", &["fx-cell-pad"]);
        head_label.set_hexpand(true);
        head.append(&head_label);
        head.append(&cell(label("TYPE", &["fx-cell-pad"]).upcast(), TYPE_W));
        let head_default = label("DEFAULT", &["fx-cell-pad"]);
        head_default.set_hexpand(true);
        head.append(&head_default);
        head.append(&cell(label("REQUIRED", &[]).upcast(), REQUIRED_W));
        table.append(&head);
        let fields_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.append(&fields_box);
        fields_section.append(&table);
        let help = label("", &["fx-field-note"]);
        help.set_wrap(true);
        let mono = |p: &str| format!("<span font_family=\"IBM Plex Mono\">{p}</span>");
        help.set_markup(&format!(
            "Types: text, date, list, multi-line. Placeholders: {}, {}, {}, {}",
            mono("{today}"),
            mono("{user}"),
            mono("{duration}"),
            mono("{model}")
        ));
        fields_section.append(&help);
        editor.append(&fields_section);

        // Layout: logo, body font, footer.
        let layout = gtk::Box::new(gtk::Orientation::Vertical, 12);
        layout.append(&label("Layout", &["fx-h3"]));
        let layout_top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        layout_top.set_homogeneous(true);
        let logo_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let logo_slot = gtk::Stack::new();
        logo_slot.add_css_class("fx-logo-slot");
        logo_slot.set_size_request(72, 40);
        logo_slot.set_valign(gtk::Align::Center);
        let empty_logo = label("[LOGO]", &[]);
        empty_logo.set_xalign(0.5);
        logo_slot.add_named(&empty_logo, Some("empty"));
        let logo_picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .build();
        logo_slot.add_named(&logo_picture, Some("logo"));
        let choose_logo = gtk::Button::with_label("Choose image…");
        choose_logo.add_css_class("fx-secondary");
        let clear_logo = gtk::Button::with_label("Remove");
        clear_logo.add_css_class("fx-quiet");
        logo_row.append(&logo_slot);
        logo_row.append(&choose_logo);
        logo_row.append(&clear_logo);
        layout_top.append(&field("Logo", &logo_row));

        let font = entry("Body font");
        let size = gtk::SpinButton::with_range(6.0, 24.0, 0.5);
        size.set_digits(1);
        size.update_property(&[gtk::accessible::Property::Label("Body size in points")]);
        let font_pop_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        font_pop_box.add_css_class("fx-font-popover");
        font_pop_box.append(&field("Font", &font));
        font_pop_box.append(&field("Size (pt)", &size));
        let font_pop = gtk::Popover::builder().child(&font_pop_box).build();
        let font_label = label("", &[]);
        font_label.set_hexpand(true);
        font_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let font_child = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        font_child.append(&font_label);
        let chevron = gtk::Image::from_icon_name("pan-down-symbolic");
        chevron.add_css_class("fx-chevron");
        font_child.append(&chevron);
        let font_button = gtk::MenuButton::builder()
            .child(&font_child)
            .popover(&font_pop)
            .build();
        font_button.add_css_class("fx-select");
        font_button.update_property(&[gtk::accessible::Property::Label("Body font")]);
        layout_top.append(&field("Body font", &font_button));
        layout.append(&layout_top);
        let footer = entry("Footer");
        footer.set_placeholder_text(Some("e.g. department, address or classification"));
        layout.append(&field("Footer", &footer));
        editor.append(&layout);

        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let path_label = label("", &["fx-field-note", "fx-mono"]);
        path_label.set_hexpand(true);
        path_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let delete = gtk::Button::with_label("Delete template");
        delete.add_css_class("fx-quiet");
        bottom.append(&path_label);
        bottom.append(&delete);
        editor.append(&bottom);
        let editor_scroll = gtk::ScrolledWindow::builder()
            .child(&editor)
            .hexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        // Preview: the first page as the export renders it, on the canvas.
        let preview = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .build();
        // A non-scrolling frame keeps the picture's natural size out of the
        // layout, so the aside stays 340px wide.
        let page_frame = gtk::ScrolledWindow::builder()
            .child(&preview)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .halign(gtk::Align::Center)
            .build();
        page_frame.set_size_request(292, 413);
        page_frame.add_css_class("fx-page-preview");
        let preview_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        preview_box.add_css_class("fx-preview-area");
        preview_box.add_css_class("fx-template-preview");
        preview_box.set_size_request(340, -1);
        preview_box.set_hexpand(false);
        preview_box.append(&label("PREVIEW", &["fx-section-title"]));
        preview_box.append(&page_frame);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&editor_scroll);
        root.append(&preview_box);

        let header_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let open_file = gtk::Button::with_label("Open as file");
        open_file.add_css_class("fx-secondary");
        let save = gtk::Button::with_label("Save template");
        save.add_css_class("fx-primary");
        header_actions.append(&open_file);
        header_actions.append(&save);

        let page = Rc::new(Self {
            root,
            header_actions,
            list_panel: left_scroll,
            dir,
            list,
            current: RefCell::default(),
            name,
            heading,
            footer,
            font,
            size,
            font_label,
            logo_slot,
            logo_picture,
            clear_logo: clear_logo.clone(),
            logo: RefCell::default(),
            fields_box,
            rows: RefCell::default(),
            next_row: Cell::new(0),
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
                    p.show_font();
                    p.update_preview();
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.size.connect_value_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.show_font();
                p.update_preview();
            }
        });
    }

    /// Reloads the template list and opens the first (or the given) one.
    pub fn reload(self: &Rc<Self>, select: Option<&str>) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let default_id = self.default_id();
        let loaded = load_dir(&self.dir);
        // As in the mockup: the default first, then simpler before longer.
        let mut order: Vec<usize> = (0..loaded.len()).collect();
        order.sort_by_key(|&i| match &loaded[i] {
            Ok(t) => (t.id != default_id, t.fields.len(), t.name.to_lowercase()),
            Err(_) => (true, usize::MAX, String::new()),
        });
        for i in order.iter().copied() {
            let t = &loaded[i];
            let (title, sub) = match t {
                Ok(t) => {
                    let count = match t.fields.len() {
                        0 => "Text only".to_string(),
                        1 => "1 field".to_string(),
                        n => format!("{n} fields"),
                    };
                    let sub = if t.id == default_id {
                        format!("Default · {count}")
                    } else {
                        count
                    };
                    (t.name.clone(), sub)
                }
                Err(e) => (file_label(e), "Cannot be read: open the file to fix it".into()),
            };
            let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
            b.append(&label(&title, &["fx-template-name"]));
            let sub_label = label(&sub, &["fx-stats"]);
            sub_label.set_wrap(true);
            b.append(&sub_label);
            let item = gtk::Button::builder().child(&b).build();
            item.add_css_class("fx-template-item");
            // Rows are sorted; the name ties each to its place in `load_dir`.
            item.set_widget_name(&format!("template-{i}"));
            let weak = Rc::downgrade(self);
            item.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.open_index(i);
                }
            });
            self.list.append(&item);
        }
        let index = select
            .and_then(|id| loaded.iter().position(|t| t.as_ref().is_ok_and(|t| t.id == id)))
            .or_else(|| loaded.iter().position(Result::is_ok));
        match index {
            Some(i) => self.open_index(i),
            None => self.new_template(),
        }
    }

    /// The id of the template new documents use (from the settings file).
    fn default_id(&self) -> String {
        self.dir
            .parent()
            .and_then(|config| crate::config::Settings::load(&config.join("settings.toml")).ok())
            .map(|s| s.default_template)
            .unwrap_or_else(|| crate::config::Settings::default().default_template)
    }

    fn mark_active(&self, index: Option<usize>) {
        let wanted = index.map(|i| format!("template-{i}"));
        let mut child = self.list.first_child();
        while let Some(c) = child {
            if wanted.as_deref() == Some(c.widget_name().as_str()) {
                c.add_css_class("active");
            } else {
                c.remove_css_class("active");
            }
            child = c.next_sibling();
        }
    }

    fn open_index(self: &Rc<Self>, i: usize) {
        match load_dir(&self.dir).into_iter().nth(i) {
            Some(Ok(t)) => {
                self.mark_active(Some(i));
                self.open(t);
            }
            Some(Err(e)) => self.set_error(&e.to_string()),
            None => {}
        }
    }

    fn open(self: &Rc<Self>, t: Template) {
        self.loading.set(true);
        self.name.set_text(&t.name);
        self.heading.set_text(&t.heading);
        self.footer.set_text(&t.footer);
        self.font.set_text(&t.body_font);
        self.size.set_value(t.body_size_pt);
        self.show_font();
        *self.logo.borrow_mut() = t.logo_path();
        self.show_logo();
        while let Some(c) = self.fields_box.first_child() {
            self.fields_box.remove(&c);
        }
        self.rows.borrow_mut().clear();
        for f in &t.fields {
            self.add_field_row(f);
        }
        self.set_error("");
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
        self.mark_active(None);
        self.open(t);
    }

    fn add_field_row(self: &Rc<Self>, f: &Field) {
        let root = row_box(&["fx-table-row"]);
        let id = self.next_row.get();
        self.next_row.set(id + 1);
        root.set_widget_name(&format!("field-row-{id}"));

        let handle = self.row_handle(&root);
        root.append(&cell(handle.upcast(), HANDLE_W));
        let label_e = gtk::Entry::builder()
            .text(f.label.as_str())
            .hexpand(true)
            .width_chars(4)
            .build();
        label_e.add_css_class("fx-cell-entry");
        label_e.update_property(&[gtk::accessible::Property::Label("Field label")]);
        root.append(&label_e);
        let names: Vec<&str> = KINDS.iter().map(|(_, n)| *n).collect();
        let kind = gtk::DropDown::from_strings(&names);
        kind.add_css_class("fx-cell-select");
        kind.set_selected(KINDS.iter().position(|(k, _)| *k == f.kind).unwrap_or(0) as u32);
        kind.update_property(&[gtk::accessible::Property::Label("Field type")]);
        root.append(&cell(kind.clone().upcast(), TYPE_W));
        let default = gtk::Entry::builder()
            .text(f.default.as_str())
            .placeholder_text("–")
            .hexpand(true)
            .width_chars(4)
            .build();
        default.add_css_class("fx-cell-entry");
        default.update_property(&[gtk::accessible::Property::Label("Default value")]);
        mark_placeholder(&default);
        root.append(&default);
        let required = gtk::CheckButton::new();
        required.set_active(f.required);
        required.update_property(&[gtk::accessible::Property::Label("Required")]);
        let required_cell = cell(required.clone().upcast(), REQUIRED_W);
        required_cell.set_halign(gtk::Align::Start);
        root.append(&required_cell);

        // Drop another row's handle here to move that row above this one.
        let drop = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        let target = root.clone();
        drop.connect_drop(move |_, value, _, _| {
            let (Some(p), Ok(name)) = (weak.upgrade(), value.get::<String>()) else {
                return false;
            };
            p.move_row_to(&name, &target);
            true
        });
        root.add_controller(drop);

        self.fields_box.append(&root);
        for e in [&label_e, &default] {
            let weak = Rc::downgrade(self);
            e.connect_changed(move |e| {
                mark_placeholder(e);
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

    /// The ⋮⋮ handle: drag it to reorder, click it for move and remove.
    fn row_handle(self: &Rc<Self>, row: &gtk::Box) -> gtk::MenuButton {
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let pop = gtk::Popover::builder().child(&menu).build();
        let actions: [(&str, RowAction); 3] = [
            ("Move up", |p, r| p.shift_row(r, -1)),
            ("Move down", |p, r| p.shift_row(r, 1)),
            ("Remove field", |p, r| p.remove_row(r)),
        ];
        for (text, f) in actions {
            let b = gtk::Button::with_label(text);
            b.add_css_class("fx-menu-item");
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let weak = Rc::downgrade(self);
            let row = row.clone();
            let pop = pop.clone();
            b.connect_clicked(move |_| {
                pop.popdown();
                if let Some(p) = weak.upgrade() {
                    f(&p, &row);
                }
            });
            menu.append(&b);
        }
        let handle = gtk::MenuButton::builder()
            .child(&label("⋮⋮", &[]))
            .popover(&pop)
            .tooltip_text("Drag to reorder")
            .build();
        handle.add_css_class("fx-handle");
        handle.update_property(&[gtk::accessible::Property::Label("Move or remove field")]);
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        let name = row.widget_name().to_string();
        drag.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&name.to_value())));
        handle.add_controller(drag);
        handle
    }

    fn row_index(&self, row: &gtk::Box) -> Option<usize> {
        self.rows.borrow().iter().position(|r| &r.root == row)
    }

    /// Moves the row named `name` to where `target` is.
    fn move_row_to(&self, name: &str, target: &gtk::Box) {
        let from = self
            .rows
            .borrow()
            .iter()
            .position(|r| r.root.widget_name() == name);
        if let (Some(from), Some(to)) = (from, self.row_index(target)) {
            self.reorder(from, to);
        }
    }

    fn shift_row(&self, row: &gtk::Box, by: isize) {
        let Some(from) = self.row_index(row) else { return };
        let to = from as isize + by;
        if to >= 0 && (to as usize) < self.rows.borrow().len() {
            self.reorder(from, to as usize);
        }
    }

    fn reorder(&self, from: usize, to: usize) {
        if from == to {
            return;
        }
        {
            let mut rows = self.rows.borrow_mut();
            let r = rows.remove(from);
            rows.insert(to, r);
            let mut prev: Option<gtk::Widget> = None;
            for r in rows.iter() {
                self.fields_box.reorder_child_after(&r.root, prev.as_ref());
                prev = Some(r.root.clone().upcast());
            }
        }
        self.update_preview();
    }

    fn remove_row(&self, row: &gtk::Box) {
        self.fields_box.remove(row);
        self.rows.borrow_mut().retain(|r| &r.root != row);
        self.update_preview();
    }

    fn set_logo(self: &Rc<Self>, path: &std::path::Path) {
        let Some(id) = self.current.borrow().as_ref().map(|t| t.id.clone()) else {
            return;
        };
        let target = self.dir.join(format!("{id}-logo.png"));
        match std::fs::create_dir_all(&self.dir).and_then(|_| std::fs::copy(path, &target)) {
            Ok(_) => *self.logo.borrow_mut() = Some(target),
            Err(e) => self.set_error(&format!("Could not copy the logo: {e}")),
        }
        self.show_logo();
        self.update_preview();
    }

    fn show_logo(&self) {
        let logo = self.logo.borrow().clone().filter(|p| p.exists());
        match &logo {
            Some(p) => {
                self.logo_picture.set_filename(Some(p));
                self.logo_slot.set_visible_child_name("logo");
                self.logo_slot.add_css_class("filled");
            }
            None => {
                self.logo_picture.set_paintable(gdk::Paintable::NONE);
                self.logo_slot.set_visible_child_name("empty");
                self.logo_slot.remove_css_class("filled");
            }
        }
        let name = self
            .logo
            .borrow()
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        self.logo_slot.set_tooltip_text(name.as_deref());
        self.clear_logo.set_visible(self.logo.borrow().is_some());
    }

    fn show_font(&self) {
        let font = self.font.text();
        let font = font.trim();
        let font = if font.is_empty() { "Source Serif 4" } else { font };
        let size = self.size.value();
        let size = if size.fract() == 0.0 {
            format!("{size:.0}")
        } else {
            format!("{size:.1}")
        };
        self.font_label.set_text(&format!("{font} · {size} pt"));
    }

    fn set_error(&self, msg: &str) {
        self.error.set_text(msg);
        self.error.set_visible(!msg.is_empty());
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
        t.body_size_pt = self.size.value();
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
                self.set_error("");
                let id = t.id.clone();
                self.reload(Some(&id));
                Ok(p)
            }
            Err(e) => {
                let msg = e.to_string();
                self.set_error(msg.rsplit(": ").next().unwrap_or(&msg));
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
            self.set_error(&format!("Could not delete {}: {e}", path.display()));
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
            Err(e) => self.set_error(&e.to_string()),
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
    /// Labels of the open template's fields, in order (tests).
    pub fn field_labels(&self) -> Vec<String> {
        self.rows
            .borrow()
            .iter()
            .map(|r| r.label.text().to_string())
            .collect()
    }
    /// Moves a field one step like the handle's menu does (tests).
    pub fn move_field(&self, index: usize, down: bool) {
        let row = self.rows.borrow().get(index).map(|r| r.root.clone());
        if let Some(row) = row {
            self.shift_row(&row, if down { 1 } else { -1 });
        }
    }
}

/// Defaults that are placeholders ({today}) show in the mono accent style.
fn mark_placeholder(e: &gtk::Entry) {
    if e.text().contains('{') {
        e.add_css_class("placeholder-value");
    } else {
        e.remove_css_class("placeholder-value");
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

fn icon_text_button(icon: &str, text: &str) -> gtk::Button {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.append(&gtk::Image::from_icon_name(icon));
    b.append(&gtk::Label::new(Some(text)));
    gtk::Button::builder().child(&b).build()
}

fn row_box(classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

fn cell(w: gtk::Widget, width: i32) -> gtk::Widget {
    w.set_size_request(width, -1);
    w.set_hexpand(false);
    w.set_valign(gtk::Align::Center);
    if let Some(l) = w.downcast_ref::<gtk::Label>() {
        l.set_xalign(0.0);
    }
    w
}

fn file_label(e: &TemplateError) -> String {
    match e {
        TemplateError::Invalid { path, .. } | TemplateError::Io { path, .. } => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}
