//! The Export screen: format, template, content options, file name and a
//! rendered preview. Works for one document or several (a project).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::{Deps, label};
use crate::export::{
    ExportError, ExportOptions, Format, Report, file_stem, preview_first_page, render_txt, write,
};
use crate::store::{DocumentId, Store};
use crate::template::{Template, install_defaults, load_dir};

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Document(DocumentId),
    Documents { title: String, ids: Vec<DocumentId> },
}

pub struct ExportPage {
    pub root: gtk::Box,
    /// Buttons for the window header while this screen shows.
    pub header_actions: gtk::Box,
    store: Rc<Store>,
    deps: Deps,
    target: RefCell<Option<Target>>,
    format: Cell<Format>,
    format_buttons: Vec<(Format, gtk::ToggleButton)>,
    template: gtk::DropDown,
    templates: RefCell<Vec<Template>>,
    include_fields: gtk::CheckButton,
    timestamps: gtk::CheckButton,
    highlight: gtk::CheckButton,
    toc: gtk::CheckButton,
    filename: gtk::Entry,
    folder: RefCell<PathBuf>,
    folder_label: gtk::Label,
    warning: gtk::Label,
    export: gtk::Button,
    result: gtk::Label,
    open_result: gtk::Button,
    last_written: RefCell<Option<PathBuf>>,
    heading: gtk::Label,
    preview_stack: gtk::Stack,
    preview_picture: gtk::Picture,
    preview_text: gtk::TextView,
    caption: gtk::Label,
    building: Cell<bool>,
}

impl ExportPage {
    /// What is being exported, for the header.
    pub fn header_subtitle(&self) -> String {
        match &*self.target.borrow() {
            Some(Target::Document(id)) => self.store.document(*id).map(|d| d.title).unwrap_or_default(),
            Some(Target::Documents { title, ids }) => format!("{title} · {} documents", ids.len()),
            None => String::new(),
        }
    }

    pub fn new(store: Rc<Store>, deps: Deps) -> Rc<Self> {
        let settings = gtk::Box::new(gtk::Orientation::Vertical, 20);
        settings.add_css_class("fx-export-settings");
        settings.set_size_request(380, -1);
        let heading = label("Export", &["fx-crumb-current"]);
        settings.append(&heading);

        settings.append(&label("Format", &["fx-field-label"]));
        let formats = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        formats.add_css_class("linked");
        formats.set_homogeneous(true);
        let mut format_buttons = Vec::new();
        let mut first: Option<gtk::ToggleButton> = None;
        for (f, name) in [(Format::Txt, "TXT"), (Format::Docx, "DOCX"), (Format::Pdf, "PDF")] {
            let b = gtk::ToggleButton::with_label(name);
            if let Some(g) = &first {
                b.set_group(Some(g));
            } else {
                first = Some(b.clone());
            }
            formats.append(&b);
            format_buttons.push((f, b));
        }
        settings.append(&formats);

        let template = gtk::DropDown::from_strings(&[]);
        template.update_property(&[gtk::accessible::Property::Label("Template")]);
        settings.append(&field("Template", &template));

        settings.append(&label("Content", &["fx-field-label"]));
        let include_fields = gtk::CheckButton::with_label("Report fields at the top");
        include_fields.set_active(true);
        let timestamps = gtk::CheckButton::with_label("Timestamps on each paragraph");
        let highlight = gtk::CheckButton::with_label("Highlight low-confidence words");
        let toc = gtk::CheckButton::with_label("Table of contents");
        toc.set_active(true);
        for c in [&include_fields, &timestamps, &highlight, &toc] {
            settings.append(c);
        }

        let filename = gtk::Entry::new();
        filename.add_css_class("fx-field");
        filename.update_property(&[gtk::accessible::Property::Label("File name")]);
        settings.append(&field("File name", &filename));
        let folder_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let folder_label = label("", &["fx-field-note"]);
        folder_label.set_hexpand(true);
        folder_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let choose_folder = gtk::Button::with_label("Change…");
        choose_folder.add_css_class("fx-secondary");
        folder_row.append(&folder_label);
        folder_row.append(&choose_folder);
        settings.append(&folder_row);

        let warning = label("", &["fx-banner"]);
        warning.set_wrap(true);
        warning.set_visible(false);
        warning.set_vexpand(true);
        warning.set_valign(gtk::Align::End);
        settings.append(&warning);
        let export = gtk::Button::with_label("Export");
        export.add_css_class("fx-primary");
        settings.append(&export);
        let result_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let result = label("", &["fx-status"]);
        result.set_hexpand(true);
        result.set_wrap(true);
        let open_result = gtk::Button::with_label("Open");
        open_result.add_css_class("fx-secondary");
        open_result.set_visible(false);
        result_row.append(&result);
        result_row.append(&open_result);
        settings.append(&result_row);

        let preview_picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .build();
        preview_picture.add_css_class("fx-page-preview");
        let preview_text = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        preview_text.add_css_class("fx-page-preview");
        let text_scroll = gtk::ScrolledWindow::builder()
            .child(&preview_text)
            .vexpand(true)
            .build();
        let preview_stack = gtk::Stack::new();
        preview_stack.add_named(&preview_picture, Some("page"));
        preview_stack.add_named(&text_scroll, Some("text"));
        preview_stack.set_vexpand(true);
        let caption = label("", &["fx-stats"]);
        caption.set_xalign(0.5);
        let preview = gtk::Box::new(gtk::Orientation::Vertical, 12);
        preview.add_css_class("fx-preview-area");
        preview.set_hexpand(true);
        preview.append(&preview_stack);
        preview.append(&caption);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&settings);
        root.append(&preview);

        let folder = deps.paths.exports();
        let page = Rc::new(Self {
            root,
            header_actions: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            store,
            deps,
            target: RefCell::default(),
            format: Cell::new(Format::Docx),
            format_buttons,
            template,
            templates: RefCell::default(),
            include_fields,
            timestamps,
            highlight,
            toc,
            filename,
            folder: RefCell::new(folder),
            folder_label,
            warning,
            export,
            result,
            open_result,
            last_written: RefCell::default(),
            heading,
            preview_stack,
            preview_picture,
            preview_text,
            caption,
            building: Cell::new(false),
        });
        page.wire(&choose_folder);
        page.set_format(Format::Docx);
        page
    }

    fn wire(self: &Rc<Self>, choose_folder: &gtk::Button) {
        for (f, b) in &self.format_buttons {
            let weak = Rc::downgrade(self);
            let f = *f;
            b.connect_toggled(move |b| {
                if b.is_active()
                    && let Some(p) = weak.upgrade()
                {
                    p.format.set(f);
                    p.update_filename_extension();
                    p.refresh();
                }
            });
        }
        for c in [&self.include_fields, &self.timestamps, &self.highlight, &self.toc] {
            let weak = Rc::downgrade(self);
            c.connect_toggled(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.refresh();
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.template.connect_selected_notify(move |_| {
            if let Some(p) = weak.upgrade()
                && !p.building.get()
            {
                p.refresh();
            }
        });
        let weak = Rc::downgrade(self);
        self.export.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.export_now();
            }
        });
        let weak = Rc::downgrade(self);
        self.open_result.connect_clicked(move |b| {
            let Some(p) = weak.upgrade() else { return };
            let Some(path) = p.last_written.borrow().clone() else {
                return;
            };
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
            let window = b.root().and_downcast::<gtk::Window>();
            launcher.launch(window.as_ref(), gio::Cancellable::NONE, |r| {
                if let Err(e) = r {
                    tracing::warn!("opening the export: {e}");
                }
            });
        });
        let weak = Rc::downgrade(self);
        choose_folder.connect_clicked(move |b| {
            let Some(p) = weak.upgrade() else { return };
            let dialog = gtk::FileDialog::builder()
                .title("Export to folder")
                .modal(true)
                .build();
            dialog.set_initial_folder(Some(&gio::File::for_path(&*p.folder.borrow())));
            let window = b.root().and_downcast::<gtk::Window>();
            let weak = Rc::downgrade(&p);
            dialog.select_folder(window.as_ref(), gio::Cancellable::NONE, move |res| {
                if let (Ok(f), Some(p)) = (res, weak.upgrade())
                    && let Some(path) = f.path()
                {
                    *p.folder.borrow_mut() = path;
                    p.show_folder();
                }
            });
        });
    }

    pub fn set_format(&self, f: Format) {
        self.format.set(f);
        for (ff, b) in &self.format_buttons {
            if *ff == f {
                b.set_active(true);
            }
        }
    }

    /// Prepares the screen for `target`, defaulting to its template.
    pub fn show(&self, target: Target) {
        self.building.set(true);
        let dir = self.deps.paths.templates();
        let _ = install_defaults(&dir);
        let mut templates: Vec<Template> = load_dir(&dir).into_iter().filter_map(Result::ok).collect();
        templates.push(Template::blank());
        let preferred = match &target {
            Target::Document(id) => self.store.document(*id).ok().and_then(|d| d.template_id),
            Target::Documents { .. } => None,
        }
        .unwrap_or_else(|| self.deps.settings.borrow().default_template.clone());
        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        self.template.set_model(Some(&gtk::StringList::new(&names)));
        self.template
            .set_selected(templates.iter().position(|t| t.id == preferred).unwrap_or(0) as u32);
        *self.templates.borrow_mut() = templates;
        let (heading, stem, many) = match &target {
            Target::Document(id) => {
                let doc = self.store.document(*id).ok();
                let title = doc.as_ref().map(|d| d.title.clone()).unwrap_or_default();
                let case = doc.as_ref().and_then(|d| d.fields.get("sagsnr").cloned());
                (
                    format!("Export · {title}"),
                    file_stem(&title, case.as_deref()),
                    false,
                )
            }
            Target::Documents { title, ids } => (
                format!("Export {} documents · {title}", ids.len()),
                file_stem(title, None),
                true,
            ),
        };
        self.heading.set_text(&heading);
        self.toc.set_visible(many);
        self.filename
            .set_text(&format!("{stem}.{}", self.format.get().extension()));
        *self.target.borrow_mut() = Some(target);
        self.result.set_text("");
        self.open_result.set_visible(false);
        self.show_folder();
        self.building.set(false);
        self.refresh();
    }

    fn show_folder(&self) {
        let folder = self.folder.borrow();
        let home = std::env::var("HOME").unwrap_or_default();
        let shown = folder.to_string_lossy().replacen(&home, "~", 1);
        self.folder_label.set_text(&format!("Saved to {shown}"));
    }

    fn update_filename_extension(&self) {
        let name = self.filename.text().to_string();
        let stem = name.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(name);
        self.filename
            .set_text(&format!("{stem}.{}", self.format.get().extension()));
    }

    fn options(&self) -> ExportOptions {
        ExportOptions {
            include_fields: self.include_fields.is_active(),
            timestamps: self.timestamps.is_active(),
            highlight_low_confidence: self.highlight.is_active(),
            table_of_contents: self.toc.is_active(),
        }
    }

    fn selected_template(&self) -> Template {
        self.templates
            .borrow()
            .get(self.template.selected() as usize)
            .cloned()
            .unwrap_or_else(Template::blank)
    }

    fn report(&self, checked: bool) -> Result<Report, ExportError> {
        let target = self
            .target
            .borrow()
            .clone()
            .ok_or(ExportError::MissingFields(vec![]))?;
        let t = self.selected_template();
        match target {
            Target::Document(id) if checked => Report::for_document(&self.store, id, &t, self.options()),
            Target::Document(id) => Report::draft_for_document(&self.store, id, &t, self.options()),
            Target::Documents { title, ids } => {
                Report::for_documents(&self.store, &ids, &title, &t, self.options())
            }
        }
    }

    /// Re-renders the preview and the required-fields warning.
    pub fn refresh(&self) {
        if self.building.get() || self.target.borrow().is_none() {
            return;
        }
        let missing = match self.report(true) {
            Err(ExportError::MissingFields(f)) => f,
            _ => Vec::new(),
        };
        self.warning.set_visible(!missing.is_empty());
        self.warning
            .set_text(&format!("Required fields are empty: {}", missing.join(", ")));
        self.export.set_sensitive(missing.is_empty());
        self.export.set_label(&format!(
            "Export {}",
            self.format.get().extension().to_uppercase()
        ));
        let report = match self.report(false) {
            Ok(r) => r,
            Err(e) => {
                self.caption.set_text(&e.to_string());
                return;
            }
        };
        match self.format.get() {
            Format::Txt => {
                self.preview_text.buffer().set_text(&render_txt(&report));
                self.preview_stack.set_visible_child_name("text");
                self.caption.set_text("Preview · plain text");
            }
            Format::Docx | Format::Pdf => match preview_first_page(&report, 900) {
                Ok(surface) => {
                    self.preview_picture.set_paintable(Some(&texture(surface)));
                    self.preview_stack.set_visible_child_name("page");
                    self.caption.set_text("Preview · A4 · first page");
                }
                Err(e) => self.caption.set_text(&format!("Preview failed: {e}")),
            },
        }
    }

    /// Writes the file; returns where it went.
    pub fn export_now(&self) -> Option<PathBuf> {
        let report = match self.report(true) {
            Ok(r) => r,
            Err(e) => {
                self.result.set_text(&e.to_string());
                return None;
            }
        };
        let folder = self.folder.borrow().clone();
        if let Err(e) = std::fs::create_dir_all(&folder) {
            self.result
                .set_text(&format!("Could not create {}: {e}", folder.display()));
            return None;
        }
        let name = self.filename.text().trim().to_string();
        let path = folder.join(if name.is_empty() {
            format!("fennec.{}", self.format.get().extension())
        } else {
            name
        });
        match write(&report, self.format.get(), &path) {
            Ok(()) => {
                self.result.set_text(&format!(
                    "Saved {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                self.open_result.set_visible(true);
                *self.last_written.borrow_mut() = Some(path.clone());
                Some(path)
            }
            Err(e) => {
                self.result.set_text(&e.to_string());
                None
            }
        }
    }

    /// Point the export at another folder (tests, or a remembered choice).
    pub fn set_folder(&self, folder: PathBuf) {
        *self.folder.borrow_mut() = folder;
        self.show_folder();
    }

    pub fn warning_text(&self) -> Option<String> {
        self.warning.is_visible().then(|| self.warning.text().to_string())
    }

    pub fn preview_is_page(&self) -> bool {
        self.preview_stack.visible_child_name().as_deref() == Some("page")
            && self.preview_picture.paintable().is_some()
    }
}

fn field(name: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&label(name, &["fx-field-label"]));
    b.append(widget);
    b
}

/// A cairo ARGB32 image as a GDK texture.
pub(super) fn texture(surface: cairo::ImageSurface) -> gdk::Texture {
    let (w, h, stride) = (surface.width(), surface.height(), surface.stride() as usize);
    let mut surface = surface;
    let bytes = {
        let data = surface.data().expect("the preview surface is not shared");
        glib::Bytes::from(&data[..])
    };
    // Cairo's ARGB32 is premultiplied BGRA in memory on little-endian machines.
    gdk::MemoryTexture::new(w, h, gdk::MemoryFormat::B8g8r8a8Premultiplied, &bytes, stride).upcast()
}
