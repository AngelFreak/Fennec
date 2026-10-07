//! Fennec's side of Fennec Recorder: runs the receiver while Settings →
//! Phone says so, asks Allow or Deny when a phone pairs, and hands arriving
//! recordings to the Files queue.

use std::cell::{Cell, RefCell};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;

use super::{Deps, Handler, label};
use crate::store::DocumentId;
use crate::sync::{PairingOffer, Receiver, ReceiverConfig, SyncEvent};

#[derive(Debug, Clone, PartialEq)]
pub enum LinkState {
    Off,
    /// Listening on this address.
    On(SocketAddr),
    /// Receiving is on in Settings but could not start.
    Failed(String),
}

/// A phone waiting for Allow.
#[derive(Debug, Clone, PartialEq)]
pub struct PairRequest {
    pub request: u64,
    pub device_name: String,
    pub code: String,
}

pub struct PhoneLink {
    deps: Deps,
    receiver: RefCell<Option<Receiver>>,
    /// Port the running receiver was started on.
    port: Cell<u16>,
    state: RefCell<LinkState>,
    offer: RefCell<Option<PairingOffer>>,
    pending: RefCell<Option<PairRequest>>,
    dialog: RefCell<Option<adw::AlertDialog>>,
    /// Where the Allow dialog appears.
    parent: RefCell<Option<gtk::Widget>>,
    events: async_channel::Sender<SyncEvent>,
    on_changed: Handler<()>,
    on_received: Handler<(DocumentId, PathBuf)>,
    on_projects_changed: Handler<()>,
}

impl PhoneLink {
    pub fn new(deps: Deps) -> Rc<Self> {
        let (tx, rx) = async_channel::unbounded();
        let link = Rc::new(Self {
            deps,
            receiver: RefCell::default(),
            port: Cell::new(0),
            state: RefCell::new(LinkState::Off),
            offer: RefCell::default(),
            pending: RefCell::default(),
            dialog: RefCell::default(),
            parent: RefCell::default(),
            events: tx,
            on_changed: RefCell::default(),
            on_received: RefCell::default(),
            on_projects_changed: RefCell::default(),
        });
        let weak = Rc::downgrade(&link);
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                let Some(link) = weak.upgrade() else { return };
                link.handle(event);
            }
        });
        link
    }

    /// Something shown in Settings → Phone changed.
    pub fn connect_changed(&self, f: impl Fn(()) + 'static) {
        *self.on_changed.borrow_mut() = Some(Rc::new(f));
    }

    /// A recording arrived and has a document.
    pub fn connect_received(&self, f: impl Fn((DocumentId, PathBuf)) + 'static) {
        *self.on_received.borrow_mut() = Some(Rc::new(f));
    }

    /// A phone added or changed a project.
    pub fn connect_projects_changed(&self, f: impl Fn(()) + 'static) {
        *self.on_projects_changed.borrow_mut() = Some(Rc::new(f));
    }

    pub fn set_parent(&self, w: &impl IsA<gtk::Widget>) {
        *self.parent.borrow_mut() = Some(w.clone().upcast());
    }

    /// Tells Settings → Phone to redraw (also after changes made there).
    pub fn notify_changed(&self) {
        if let Some(f) = self.on_changed.borrow().clone() {
            f(());
        }
    }

    pub fn state(&self) -> LinkState {
        self.state.borrow().clone()
    }

    pub fn offer(&self) -> Option<PairingOffer> {
        self.offer.borrow().clone()
    }

    pub fn pending(&self) -> Option<PairRequest> {
        self.pending.borrow().clone()
    }

    /// Starts, stops or restarts the receiver to match Settings.
    pub fn apply(&self) {
        let s = self.deps.settings();
        let running = self.receiver.borrow().is_some();
        if !s.phone.enabled {
            if running || *self.state.borrow() != LinkState::Off {
                self.stop();
            }
            return;
        }
        if running && self.port.get() == s.phone.port {
            if let Some(r) = self.receiver.borrow().as_ref() {
                r.set_defaults(s.phone.default_project, &s.default_template);
            }
            return;
        }
        self.stop();
        let mut config = ReceiverConfig::from_settings(&self.deps.paths, &s);
        if self.deps.phone_local_only {
            config.bind = SocketAddr::from(([127, 0, 0, 1], s.phone.port));
            config.advertise = false;
        }
        let tx = self.events.clone();
        let sink = Arc::new(move |e| {
            let _ = tx.send_blocking(e);
        });
        *self.state.borrow_mut() = match Receiver::start(config, sink) {
            Ok(r) => {
                let addr = r.local_addr();
                *self.receiver.borrow_mut() = Some(r);
                self.port.set(s.phone.port);
                LinkState::On(addr)
            }
            Err(e) => {
                tracing::warn!("phone receiver: {e}");
                LinkState::Failed(e)
            }
        };
        self.notify_changed();
    }

    fn stop(&self) {
        // Dropping the receiver ends the offer and denies a waiting phone.
        self.receiver.borrow_mut().take();
        self.offer.borrow_mut().take();
        self.pending.borrow_mut().take();
        self.close_dialog();
        *self.state.borrow_mut() = LinkState::Off;
        self.notify_changed();
    }

    /// Opens a pairing offer for the QR code, turning receiving on first if
    /// needed. `None` if the receiver cannot start.
    pub fn offer_pairing(&self) -> Option<PairingOffer> {
        if !self.deps.settings.borrow().phone.enabled {
            self.deps.settings.borrow_mut().phone.enabled = true;
            if let Err(e) = self.deps.save_settings() {
                tracing::warn!("saving settings: {e}");
            }
        }
        self.apply();
        let offer = self.receiver.borrow().as_ref()?.offer_pairing();
        *self.offer.borrow_mut() = Some(offer.clone());
        self.notify_changed();
        Some(offer)
    }

    pub fn cancel_pairing(&self) {
        if let Some(r) = self.receiver.borrow().as_ref() {
            r.cancel_pairing();
        }
        self.offer.borrow_mut().take();
        self.pending.borrow_mut().take();
        self.close_dialog();
        self.notify_changed();
    }

    /// Allow or Deny the phone waiting in [`PhoneLink::pending`].
    pub fn decide(&self, allow: bool) {
        let Some(p) = self.pending.borrow_mut().take() else {
            return;
        };
        if let Some(r) = self.receiver.borrow().as_ref() {
            r.decide(p.request, allow);
        }
        self.close_dialog();
        self.notify_changed();
    }

    fn close_dialog(&self) {
        if let Some(d) = self.dialog.borrow_mut().take() {
            d.force_close();
        }
    }

    fn handle(self: &Rc<Self>, event: SyncEvent) {
        match event {
            SyncEvent::PairRequest {
                request,
                device_name,
                code,
            } => {
                *self.pending.borrow_mut() = Some(PairRequest {
                    request,
                    device_name,
                    code,
                });
                self.ask();
                self.notify_changed();
            }
            SyncEvent::PairingEnded => {
                self.offer.borrow_mut().take();
                self.pending.borrow_mut().take();
                self.close_dialog();
                self.notify_changed();
            }
            SyncEvent::DevicesChanged => self.notify_changed(),
            SyncEvent::ProjectsChanged => {
                if let Some(f) = self.on_projects_changed.borrow().clone() {
                    f(());
                }
                self.notify_changed();
            }
            SyncEvent::Received { document_id, path } => {
                if let Some(f) = self.on_received.borrow().clone() {
                    f((document_id, path));
                }
                // The phone's recording count and last-seen time changed.
                self.notify_changed();
            }
        }
    }

    /// Asks whether the waiting phone may pair, wherever Fennec is.
    fn ask(self: &Rc<Self>) {
        let (Some(p), Some(parent)) = (self.pending(), self.parent.borrow().clone()) else {
            return;
        };
        self.close_dialog();
        let dialog = adw::AlertDialog::new(
            Some(&format!("Pair with {}?", p.device_name)),
            Some(
                "Allow only if the phone shows the same code. Once paired, it can send recordings to this computer.",
            ),
        );
        let code = label(&p.code, &["fx-bignum", "fx-pair-code"]);
        code.set_xalign(0.5);
        code.update_property(&[gtk::accessible::Property::Label(&format!(
            "Pairing code {}",
            p.code
        ))]);
        dialog.set_extra_child(Some(&code));
        dialog.add_response("deny", "Deny");
        dialog.add_response("allow", "Allow");
        dialog.set_response_appearance("allow", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("deny"));
        dialog.set_close_response("deny");
        let weak = Rc::downgrade(self);
        let request = p.request;
        dialog.connect_response(None, move |_, response| {
            let Some(link) = weak.upgrade() else { return };
            // A dialog closed because the phone gave up answers nothing.
            if link.pending().is_some_and(|p| p.request == request) {
                link.dialog.borrow_mut().take();
                link.decide(response == "allow");
            }
        });
        dialog.present(Some(&parent));
        *self.dialog.borrow_mut() = Some(dialog);
    }
}
