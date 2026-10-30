//! Settings → Phone: receiving recordings from Fennec Recorder, over Wi-Fi
//! with a QR code each time, or over a USB cable.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use super::phone::PhoneLink;
use super::settings_page::{group, page, row, title_block};
use super::{Deps, label};
use crate::store::Store;

pub struct PhoneSettingsUi {
    deps: Deps,
    pub link: Rc<PhoneLink>,
    /// Opens the Receive from phone dialog.
    pub receive: gtk::Button,
    /// Import over USB.
    pub usb: gtk::Switch,
    status: gtk::Label,
    pub port: gtk::SpinButton,
    pub default_project: gtk::DropDown,
    /// Project ids in the dropdown's order; `None` is Unsorted.
    project_ids: RefCell<Vec<Option<i64>>>,
    /// Set while the controls are filled in from Settings, so that is not
    /// taken for a change.
    loading: Cell<bool>,
    on_changed: super::Handler<()>,
}

impl PhoneSettingsUi {
    pub fn new(deps: Deps, link: Rc<PhoneLink>) -> Rc<Self> {
        let port = gtk::SpinButton::with_range(1024.0, 65535.0, 1.0);
        port.set_numeric(true);
        port.update_property(&[gtk::accessible::Property::Label("Port")]);
        let default_project = gtk::DropDown::from_strings(&[]);
        default_project.update_property(&[gtk::accessible::Property::Label("Project for phone recordings")]);
        default_project.set_size_request(220, -1);
        let receive = gtk::Button::with_label("Receive from phone");
        receive.add_css_class("fx-secondary");
        Rc::new(Self {
            deps,
            link,
            receive,
            usb: {
                let s = gtk::Switch::new();
                s.update_property(&[gtk::accessible::Property::Label("Import over USB")]);
                s
            },
            status: label("", &["fx-field-note"]),
            port,
            default_project,
            project_ids: RefCell::default(),
            loading: Cell::new(false),
            on_changed: RefCell::default(),
        })
    }

    /// Something the sidebar line shows changed.
    pub fn connect_changed(&self, f: impl Fn(()) + 'static) {
        *self.on_changed.borrow_mut() = Some(Rc::new(f));
    }

    /// The line under "Phone" in the sidebar.
    pub fn summary(&self) -> String {
        if self.deps.settings().phone.usb_import {
            "QR code or USB".to_string()
        } else {
            "QR code".to_string()
        }
    }

    pub fn section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(760), 28);
        b.append(&title_block(
            "Phone",
            "Record meetings and interviews with Fennec Recorder on your phone, then send them to this computer to be transcribed. Recordings go straight here and nowhere else.",
            None,
        ));

        let (wifi, rows) = group("Over Wi-Fi", None);
        rows.append(&row(
            "Send from the phone",
            Some(
                "Shows a code to scan with Send to Fennec in the app. The phone sends what it has not sent yet, then Fennec stops listening. The phone and this computer must be on the same Wi-Fi network.",
            ),
            &self.receive,
        ));
        self.port.set_valign(gtk::Align::Center);
        let r = row(
            "Port",
            Some(
                "Phones reach Fennec on this port while the code is shown. Change it only if another program uses it.",
            ),
            &self.port,
        );
        if let Some(text) = r.first_child().and_downcast::<gtk::Box>() {
            self.status.set_wrap(true);
            self.status.set_visible(false);
            text.append(&self.status);
        }
        rows.append(&r);
        b.append(&wifi);

        let (cable, rows) = group("USB cable", None);
        rows.append(&row(
            "Import over USB",
            Some("When a phone with Fennec Recorder is plugged in and set to File transfer, Fennec offers to import its recordings. No shared Wi-Fi needed."),
            &self.usb,
        ));
        b.append(&cable);

        let (recordings, rows) = group("Recordings", None);
        rows.append(&row(
            "Project for phone recordings",
            Some("Used when the phone names no project."),
            &self.default_project,
        ));
        b.append(&recordings);

        let privacy = label(
            "Recordings travel encrypted from the phone to this computer. Each code works once, for one phone, and only while it is shown; nothing stays paired.",
            &["fx-field-note"],
        );
        privacy.set_wrap(true);
        b.append(&privacy);

        self.wire();
        self.refresh();
        outer
    }

    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.usb.connect_active_notify(move |sw| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.loading.get() {
                return;
            }
            ui.deps.settings.borrow_mut().phone.usb_import = sw.is_active();
            ui.save();
        });
        let weak = Rc::downgrade(self);
        self.port.connect_value_changed(move |sb| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.loading.get() {
                return;
            }
            ui.deps.settings.borrow_mut().phone.port = sb.value() as u16;
            ui.save();
        });
        let weak = Rc::downgrade(self);
        self.default_project.connect_selected_notify(move |dd| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.loading.get() {
                return;
            }
            let id = ui
                .project_ids
                .borrow()
                .get(dd.selected() as usize)
                .copied()
                .flatten();
            ui.deps.settings.borrow_mut().phone.default_project = id;
            ui.save();
        });
        let link = Rc::clone(&self.link);
        self.receive.connect_clicked(move |_| link.receive());
    }

    fn save(&self) {
        match self.deps.save_settings() {
            Ok(()) => self.status.set_visible(false),
            Err(e) => {
                self.status.set_text(&format!("Settings not saved: {e}"));
                self.status.set_visible(true);
            }
        }
        if let Some(f) = self.on_changed.borrow().clone() {
            f(());
        }
    }

    /// Reads everything shown from Settings and the database.
    pub fn refresh(&self) {
        self.loading.set(true);
        let s = self.deps.settings();
        self.usb.set_active(s.phone.usb_import);
        self.port.set_value(f64::from(s.phone.port));

        let projects = Store::open(&self.deps.paths.database())
            .and_then(|s| s.projects())
            .unwrap_or_default();
        let mut names = vec!["Unsorted".to_string()];
        let mut ids = vec![None];
        for p in projects {
            names.push(p.name);
            ids.push(Some(p.id));
        }
        let selected = ids
            .iter()
            .position(|id| *id == s.phone.default_project)
            .unwrap_or(0);
        let model = gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
        self.default_project.set_model(Some(&model));
        self.default_project.set_selected(selected as u32);
        *self.project_ids.borrow_mut() = ids;
        self.loading.set(false);
        if let Some(f) = self.on_changed.borrow().clone() {
            f(());
        }
    }
}
