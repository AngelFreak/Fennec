//! Recordings from Fennec Recorder over a USB cable: when a phone appears
//! (Android's File transfer mode mounts it through GVFS) and its transfer
//! folder holds recordings Fennec does not have, Fennec offers to import
//! them. Importing removes them from the phone, which is how the app learns
//! they arrived (see `sync::usb`).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use super::{Deps, Handler};
use crate::store::{DocumentId, Store};
use crate::sync::{Target, usb};

/// A phone with recordings to import.
#[derive(Debug, Clone)]
pub struct Offer {
    pub phone: String,
    pub folder: gio::File,
    pub ids: Vec<String>,
}

enum Msg {
    Found(Offer),
    Imported {
        arrived: Vec<(DocumentId, PathBuf)>,
        failed: Vec<String>,
    },
}

pub struct UsbImport {
    deps: Deps,
    monitor: gio::VolumeMonitor,
    tx: async_channel::Sender<Msg>,
    offer: RefCell<Option<Offer>>,
    /// Recordings the user said Not now to, until Fennec restarts.
    declined: RefCell<HashSet<String>>,
    /// A scan or an import is running.
    busy: Cell<bool>,
    dialog: RefCell<Option<adw::AlertDialog>>,
    parent: RefCell<Option<gtk::Widget>>,
    /// What the last import did, as shown to the user (tests read it too).
    pub last: RefCell<Option<String>>,
    on_received: Handler<(DocumentId, PathBuf)>,
    on_imported: Handler<()>,
}

impl UsbImport {
    pub fn new(deps: Deps) -> Rc<Self> {
        let (tx, rx) = async_channel::unbounded();
        let u = Rc::new(Self {
            deps,
            monitor: gio::VolumeMonitor::get(),
            tx,
            offer: RefCell::default(),
            declined: RefCell::default(),
            busy: Cell::new(false),
            dialog: RefCell::default(),
            parent: RefCell::default(),
            last: RefCell::default(),
            on_received: RefCell::default(),
            on_imported: RefCell::default(),
        });
        let weak = Rc::downgrade(&u);
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let Some(u) = weak.upgrade() else { return };
                u.handle(msg);
            }
        });
        u
    }

    pub fn set_parent(&self, w: &impl IsA<gtk::Widget>) {
        *self.parent.borrow_mut() = Some(w.clone().upcast());
    }

    /// An imported recording, for the Files queue.
    pub fn connect_received(&self, f: impl Fn((DocumentId, PathBuf)) + 'static) {
        *self.on_received.borrow_mut() = Some(Rc::new(f));
    }

    /// An import finished with something new (the window shows Files).
    pub fn connect_imported(&self, f: impl Fn(()) + 'static) {
        *self.on_imported.borrow_mut() = Some(Rc::new(f));
    }

    /// Watches for phones being plugged in, and looks every few seconds at
    /// those already mounted (the phone may get new recordings meanwhile).
    pub fn watch(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.monitor.connect_mount_added(move |_, mount| {
            if let Some(u) = weak.upgrade() {
                u.check(mount.root(), mount.name().to_string());
            }
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(10, move || {
            let Some(u) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            u.check_mounts();
            glib::ControlFlow::Continue
        });
        self.check_mounts();
    }

    fn check_mounts(self: &Rc<Self>) {
        for m in self.monitor.mounts() {
            // Phones in File transfer mode; not disks and network shares.
            if m.root().uri_scheme().is_some_and(|s| s == "mtp") {
                self.check(m.root(), m.name().to_string());
            }
        }
    }

    pub fn offer(&self) -> Option<Offer> {
        self.offer.borrow().clone()
    }

    /// Looks for the app's folder under `root` (off the main thread: a phone
    /// answers slowly). Recordings Fennec already has are removed from the
    /// phone at once; new ones are offered.
    pub fn check(self: &Rc<Self>, root: gio::File, phone: String) {
        if !self.deps.settings.borrow().phone.usb_import || self.busy.get() || self.offer.borrow().is_some() {
            return;
        }
        self.busy.set(true);
        let (db, tx) = (self.deps.paths.database(), self.tx.clone());
        let declined = self.declined.borrow().clone();
        std::thread::spawn(move || {
            let found = usb::recorder_folder(&root).and_then(|folder| {
                let store = Store::open(&db).ok()?;
                let mut ids = Vec::new();
                for w in usb::waiting(&folder) {
                    if usb::known(&store, &w) {
                        if let Err(e) = usb::remove_from_phone(&w) {
                            tracing::warn!("removing a recording Fennec has from the phone: {e}");
                        }
                    } else if !declined.contains(&w.meta.id) {
                        ids.push(w.meta.id.clone());
                    }
                }
                (!ids.is_empty()).then(|| Offer {
                    phone,
                    folder: folder.clone(),
                    ids,
                })
            });
            let _ = match found {
                Some(o) => tx.send_blocking(Msg::Found(o)),
                None => tx.send_blocking(Msg::Imported {
                    arrived: Vec::new(),
                    failed: Vec::new(),
                }),
            };
        });
    }

    fn handle(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Found(o) => {
                self.busy.set(false);
                *self.offer.borrow_mut() = Some(o);
                self.ask();
            }
            Msg::Imported { arrived, failed } => {
                self.busy.set(false);
                let had_offer = self.offer.borrow_mut().take().is_some();
                if let Some(f) = self.on_received.borrow().clone() {
                    for a in &arrived {
                        f(a.clone());
                    }
                }
                if had_offer {
                    let mut text = match arrived.len() {
                        0 => "Nothing was imported from the phone.".to_string(),
                        1 => "1 recording imported from the phone. It is in the Files queue.".to_string(),
                        n => format!("{n} recordings imported from the phone. They are in the Files queue."),
                    };
                    for f in &failed {
                        text.push('\n');
                        text.push_str(f);
                    }
                    if !arrived.is_empty()
                        && let Some(f) = self.on_imported.borrow().clone()
                    {
                        f(());
                    }
                    if !failed.is_empty()
                        && let Some(parent) = self.parent.borrow().clone()
                    {
                        let d = adw::AlertDialog::new(Some("Some recordings were not imported"), Some(&text));
                        d.add_response("ok", "OK");
                        d.present(Some(&parent));
                    }
                    *self.last.borrow_mut() = Some(text);
                }
            }
        }
    }

    fn ask(self: &Rc<Self>) {
        let (Some(o), Some(parent)) = (self.offer(), self.parent.borrow().clone()) else {
            return;
        };
        let n = o.ids.len();
        let dialog = adw::AlertDialog::new(
            Some(&format!("Import recordings from {}?", o.phone)),
            Some(&format!(
                "Fennec Recorder has {} that Fennec does not have yet. {} copied here and transcribed, then removed from the phone's transfer folder, so the phone marks {} sent.",
                if n == 1 {
                    "1 recording".to_string()
                } else {
                    format!("{n} recordings")
                },
                if n == 1 { "It is" } else { "They are" },
                if n == 1 { "it" } else { "them" },
            )),
        );
        dialog.add_response("later", "Not now");
        dialog.add_response("import", "Import");
        dialog.set_response_appearance("import", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("import"));
        dialog.set_close_response("later");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(u) = weak.upgrade() else { return };
            // Closed from code (answer() already took it): not an answer.
            let open = u.dialog.borrow_mut().take().is_some();
            if open {
                u.answer(response == "import");
            }
        });
        dialog.present(Some(&parent));
        *self.dialog.borrow_mut() = Some(dialog);
    }

    /// Import or Not now for the open offer.
    pub fn answer(self: &Rc<Self>, import: bool) {
        // Taken before closing: closing fires the close response.
        let open = self.dialog.borrow_mut().take();
        if let Some(d) = open {
            d.force_close();
        }
        let Some(o) = self.offer() else { return };
        if !import {
            self.declined.borrow_mut().extend(o.ids.iter().cloned());
            self.offer.borrow_mut().take();
            return;
        }
        self.busy.set(true);
        let s = self.deps.settings();
        let target = Target {
            incoming: self.deps.paths.data_dir.join("sync/incoming"),
            audio: self.deps.paths.audio(),
            default_project: s.phone.default_project,
            default_template: s.default_template.clone(),
        };
        let (db, tx, folder, ids) = (self.deps.paths.database(), self.tx.clone(), o.folder, o.ids);
        std::thread::spawn(move || {
            let mut arrived = Vec::new();
            let mut failed = Vec::new();
            match Store::open(&db) {
                Ok(store) => {
                    for w in usb::waiting(&folder) {
                        if !ids.contains(&w.meta.id) {
                            continue;
                        }
                        match usb::import(&store, &target, &w) {
                            Ok(Some(a)) => arrived.push((a.document_id, a.path)),
                            Ok(None) => {}
                            Err(e) => failed.push(e),
                        }
                    }
                }
                Err(e) => failed.push(format!("Fennec could not open its database: {e}")),
            }
            let _ = tx.send_blocking(Msg::Imported { arrived, failed });
        });
    }
}
