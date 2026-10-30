//! Fennec's side of Fennec Recorder: the Receive from phone dialog. It
//! starts the receiver and shows a fresh QR code; the phone that scans it
//! sends its recordings, which join the Files queue. Closing the dialog
//! stops the receiver, so nothing listens on the network in between.

use std::cell::RefCell;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::{Deps, Handler, label};
use crate::store::DocumentId;
use crate::sync::{Receiver, ReceiverConfig, SendOffer, SyncEvent};

/// A code nobody uses, or a phone that stops sending, ends the transfer
/// after this long.
const IDLE: Duration = Duration::from_secs(10 * 60);

/// A recording the phone is sending.
#[derive(Debug, Clone, PartialEq)]
pub struct Incoming {
    pub id: String,
    pub title: String,
    pub received: u64,
    pub size: u64,
    /// Arrived in full and queued.
    pub arrived: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Transfer {
    Idle,
    /// The code is shown; no phone has scanned it yet.
    Waiting(SendOffer),
    Sending {
        phone: String,
        recordings: Vec<Incoming>,
    },
    /// The phone sent everything, or the transfer was stopped.
    Finished {
        phone: String,
        recordings: Vec<Incoming>,
        stopped: bool,
    },
    Failed(String),
}

pub struct PhoneLink {
    deps: Deps,
    receiver: RefCell<Option<Receiver>>,
    transfer: RefCell<Transfer>,
    dialog: RefCell<Option<Rc<ReceiveDialog>>>,
    /// Where the dialog appears.
    parent: RefCell<Option<gtk::Widget>>,
    events: async_channel::Sender<SyncEvent>,
    tick: RefCell<Option<glib::SourceId>>,
    on_received: Handler<(DocumentId, PathBuf)>,
    on_show_files: Handler<()>,
}

impl PhoneLink {
    pub fn new(deps: Deps) -> Rc<Self> {
        let (tx, rx) = async_channel::unbounded();
        let link = Rc::new(Self {
            deps,
            receiver: RefCell::default(),
            transfer: RefCell::new(Transfer::Idle),
            dialog: RefCell::default(),
            parent: RefCell::default(),
            events: tx,
            tick: RefCell::default(),
            on_received: RefCell::default(),
            on_show_files: RefCell::default(),
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

    /// A recording arrived and has a document.
    pub fn connect_received(&self, f: impl Fn((DocumentId, PathBuf)) + 'static) {
        *self.on_received.borrow_mut() = Some(Rc::new(f));
    }

    /// Show in Files was pressed.
    pub fn connect_show_files(&self, f: impl Fn(()) + 'static) {
        *self.on_show_files.borrow_mut() = Some(Rc::new(f));
    }

    pub fn set_parent(&self, w: &impl IsA<gtk::Widget>) {
        *self.parent.borrow_mut() = Some(w.clone().upcast());
    }

    pub fn transfer(&self) -> Transfer {
        self.transfer.borrow().clone()
    }

    /// What the QR code holds, while it is shown.
    pub fn offer(&self) -> Option<SendOffer> {
        match &*self.transfer.borrow() {
            Transfer::Waiting(o) => Some(o.clone()),
            _ => None,
        }
    }

    /// Whether the receiver is listening.
    pub fn is_listening(&self) -> bool {
        self.receiver.borrow().is_some()
    }

    /// Which page the dialog shows, if it is open (tests).
    pub fn dialog_page(&self) -> Option<String> {
        let d = self.dialog.borrow().clone()?;
        d.stack.visible_child_name().map(|n| n.to_string())
    }

    /// The open dialog (tests read its texts).
    pub fn dialog(&self) -> Option<adw::Dialog> {
        self.dialog.borrow().as_ref().map(|d| d.dialog.clone())
    }

    /// Presses Show in Files (tests).
    pub fn press_show_files(&self) {
        let d = self.dialog.borrow().clone();
        if let Some(d) = d {
            d.show_files.emit_clicked();
        }
    }

    /// Opens the Receive from phone dialog with a fresh code.
    pub fn receive(self: &Rc<Self>) {
        self.start();
        if self.dialog.borrow().is_some() {
            return;
        }
        let d = ReceiveDialog::new(self);
        d.render(&self.transfer());
        if let Some(parent) = self.parent.borrow().clone() {
            d.dialog.present(Some(&parent));
        }
        *self.dialog.borrow_mut() = Some(d);
    }

    /// Starts the receiver with a new code, ending any transfer before it.
    pub fn start(self: &Rc<Self>) {
        self.receiver.borrow_mut().take();
        let s = self.deps.settings();
        let mut config = ReceiverConfig::from_settings(&self.deps.paths, &s);
        if self.deps.phone_local_only {
            config.bind = SocketAddr::from(([127, 0, 0, 1], s.phone.port));
        }
        let tx = self.events.clone();
        let sink = Arc::new(move |e| {
            let _ = tx.send_blocking(e);
        });
        let transfer = match Receiver::start(config, sink) {
            Ok(r) => match r.offer() {
                Some(o) => {
                    *self.receiver.borrow_mut() = Some(r);
                    Transfer::Waiting(o)
                }
                None => Transfer::Failed("Fennec could not make a code.".into()),
            },
            Err(e) => {
                tracing::warn!("phone receiver: {e}");
                Transfer::Failed(e)
            }
        };
        self.set(transfer);
        self.watch_idle();
    }

    /// Stops listening. Recordings already here stay; one halfway resumes
    /// with the next code.
    pub fn stop(&self) {
        self.receiver.borrow_mut().take();
        if let Some(id) = self.tick.borrow_mut().take() {
            id.remove();
        }
        let next = match self.transfer() {
            Transfer::Sending { phone, recordings } => Transfer::Finished {
                phone,
                recordings,
                stopped: true,
            },
            Transfer::Waiting(_) => Transfer::Idle,
            other => other,
        };
        self.set(next);
    }

    /// The dialog closed: stop, and start from nothing next time.
    fn closed(&self) {
        self.dialog.borrow_mut().take();
        self.stop();
        self.set(Transfer::Idle);
    }

    /// Closes the dialog (tests, and Show in Files).
    pub fn close(&self) {
        let d = self.dialog.borrow().clone();
        match d {
            Some(d) => {
                d.dialog.force_close();
            }
            None => self.closed(),
        }
    }

    fn show_files(&self) {
        self.close();
        if let Some(f) = self.on_show_files.borrow().clone() {
            f(());
        }
    }

    fn set(&self, t: Transfer) {
        *self.transfer.borrow_mut() = t;
        let d = self.dialog.borrow().clone();
        if let Some(d) = d {
            d.render(&self.transfer.borrow());
        }
    }

    /// Ends a transfer nobody has touched for [`IDLE`].
    fn watch_idle(self: &Rc<Self>) {
        if let Some(id) = self.tick.borrow_mut().take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_seconds_local(5, move || {
            let Some(link) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let idle = link
                .receiver
                .borrow()
                .as_ref()
                .is_some_and(|r| r.last_activity().elapsed() > IDLE);
            if !idle {
                return glib::ControlFlow::Continue;
            }
            link.tick.borrow_mut().take();
            let waiting = matches!(link.transfer(), Transfer::Waiting(_));
            link.stop();
            if waiting {
                link.set(Transfer::Failed(
                    "The code expired after 10 minutes. Show a new one to send from the phone.".into(),
                ));
            }
            glib::ControlFlow::Break
        });
        *self.tick.borrow_mut() = Some(id);
    }

    fn handle(&self, event: SyncEvent) {
        let mut t = self.transfer();
        match event {
            SyncEvent::Connected { device_name } => {
                t = Transfer::Sending {
                    phone: device_name,
                    recordings: Vec::new(),
                };
            }
            SyncEvent::Arriving {
                id,
                title,
                received,
                size,
            } => {
                if let Transfer::Sending { recordings, .. } = &mut t {
                    match recordings.iter_mut().find(|r| r.id == id) {
                        Some(r) => {
                            r.received = received;
                            r.size = size;
                        }
                        None => recordings.push(Incoming {
                            id,
                            title,
                            received,
                            size,
                            arrived: false,
                        }),
                    }
                }
            }
            SyncEvent::Received {
                id,
                document_id,
                path,
            } => {
                if let Transfer::Sending { recordings, .. } = &mut t
                    && let Some(r) = recordings.iter_mut().find(|r| r.id == id)
                {
                    r.received = r.size;
                    r.arrived = true;
                }
                if let Some(f) = self.on_received.borrow().clone() {
                    f((document_id, path));
                }
            }
            SyncEvent::Finished => {
                if let Transfer::Sending { phone, recordings } = t {
                    // Nothing more will come; stop listening now.
                    self.receiver.borrow_mut().take();
                    if let Some(id) = self.tick.borrow_mut().take() {
                        id.remove();
                    }
                    t = Transfer::Finished {
                        phone,
                        recordings,
                        stopped: false,
                    };
                }
            }
            SyncEvent::CodeClosed => {
                self.receiver.borrow_mut().take();
                t = Transfer::Failed(
                    "Someone tried too many wrong codes, so this one no longer works. Show a new one.".into(),
                );
            }
        }
        self.set(t);
    }
}

/// QR modules plus the quiet zone around them.
struct Qr {
    width: usize,
    dark: Vec<bool>,
}

/// The Receive from phone dialog.
struct ReceiveDialog {
    dialog: adw::Dialog,
    stack: gtk::Stack,
    qr: gtk::DrawingArea,
    qr_data: Rc<RefCell<Option<Qr>>>,
    address: gtk::Label,
    sending_title: gtk::Label,
    list: gtk::Box,
    finished_title: gtk::Label,
    finished_note: gtk::Label,
    show_files: gtk::Button,
    error: gtk::Label,
}

impl ReceiveDialog {
    fn new(link: &Rc<PhoneLink>) -> Rc<Self> {
        let dialog = adw::Dialog::builder()
            .title("Receive from phone")
            // As tall as the page shown; the clamp keeps it narrow.
            .follows_content_size(true)
            .build();
        let stack = gtk::Stack::new();
        stack.set_vhomogeneous(false);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);

        // Waiting for a phone.
        let qr = gtk::DrawingArea::builder()
            .content_width(232)
            .content_height(232)
            .halign(gtk::Align::Center)
            .build();
        qr.add_css_class("fx-pair-qr");
        qr.update_property(&[gtk::accessible::Property::Label(
            "Code to scan with Fennec Recorder",
        )]);
        let qr_data: Rc<RefCell<Option<Qr>>> = Rc::default();
        let data = Rc::clone(&qr_data);
        qr.set_draw_func(move |_, cr, w, h| {
            let Some((width, dark)) = data.borrow().as_ref().map(|q| (q.width, q.dark.clone())) else {
                return;
            };
            // Black on white whatever the theme: phone cameras expect it.
            let size = f64::from(w.min(h));
            let module = size / width as f64;
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.rectangle(0.0, 0.0, size, size);
            let _ = cr.fill();
            cr.set_source_rgb(0.08, 0.09, 0.11);
            for (i, on) in dark.iter().enumerate() {
                if *on {
                    let (x, y) = ((i % width) as f64, (i / width) as f64);
                    cr.rectangle(x * module, y * module, module + 0.3, module + 0.3);
                }
            }
            let _ = cr.fill();
        });
        let waiting = page_box();
        waiting.append(&qr);
        waiting.append(&centered(label("Scan with Fennec Recorder", &["fx-set-title"])));
        waiting.append(&centered(note(
            "On the phone, tap Send to Fennec and point the camera at this code. The phone needs to be on the same Wi-Fi network as this computer.",
        )));
        let address = label("", &["fx-mono", "fx-field-note"]);
        waiting.append(&centered(address.clone()));
        waiting.append(&centered(note(
            "No shared Wi-Fi? Plug the phone in with a USB cable and choose File transfer on the phone.",
        )));
        stack.add_named(&waiting, Some("waiting"));

        // A phone is sending.
        let sending = page_box();
        let sending_title = label("", &["fx-set-title"]);
        sending_title.set_wrap(true);
        sending_title.set_max_width_chars(40);
        sending.append(&sending_title);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
        list.add_css_class("fx-receive-list");
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .propagate_natural_height(true)
            .max_content_height(320)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        sending.append(&scroll);
        let stop = gtk::Button::with_label("Stop");
        stop.add_css_class("fx-secondary");
        stop.set_halign(gtk::Align::End);
        sending.append(&stop);
        stack.add_named(&sending, Some("sending"));

        // Done.
        let finished = page_box();
        let check = gtk::Image::from_icon_name("fennec-check-symbolic");
        check.set_pixel_size(32);
        check.add_css_class("fx-receive-done");
        finished.append(&check);
        let finished_title = label("", &["fx-set-title"]);
        finished_title.set_wrap(true);
        finished_title.set_max_width_chars(40);
        finished_title.set_justify(gtk::Justification::Center);
        finished.append(&centered(finished_title.clone()));
        let finished_note = note("");
        finished.append(&centered(finished_note.clone()));
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::Center);
        let close = gtk::Button::with_label("Close");
        close.add_css_class("fx-secondary");
        let show_files = gtk::Button::with_label("Show in Files");
        show_files.add_css_class("fx-primary");
        buttons.append(&close);
        buttons.append(&show_files);
        finished.append(&buttons);
        stack.add_named(&finished, Some("finished"));

        // Could not start, or the code expired.
        let failed = page_box();
        let error = note("");
        failed.append(&error);
        let again = gtk::Button::with_label("Show a new code");
        again.add_css_class("fx-primary");
        again.set_halign(gtk::Align::Start);
        failed.append(&again);
        stack.add_named(&failed, Some("failed"));

        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        let clamp = adw::Clamp::builder()
            .maximum_size(440)
            .tightening_threshold(440)
            .child(&stack)
            .build();
        clamp.set_width_request(440);
        view.set_content(Some(&clamp));
        dialog.set_child(Some(&view));

        let d = Rc::new(Self {
            dialog,
            stack,
            qr,
            qr_data,
            address,
            sending_title,
            list,
            finished_title,
            finished_note,
            show_files,
            error,
        });

        let weak = Rc::downgrade(link);
        d.dialog.connect_closed(move |_| {
            if let Some(link) = weak.upgrade() {
                link.closed();
            }
        });
        let weak = Rc::downgrade(link);
        stop.connect_clicked(move |_| {
            if let Some(link) = weak.upgrade() {
                link.stop();
            }
        });
        let weak = Rc::downgrade(link);
        again.connect_clicked(move |_| {
            if let Some(link) = weak.upgrade() {
                link.start();
            }
        });
        let weak = Rc::downgrade(link);
        close.connect_clicked(move |_| {
            if let Some(link) = weak.upgrade() {
                link.close();
            }
        });
        let weak = Rc::downgrade(link);
        d.show_files.connect_clicked(move |_| {
            if let Some(link) = weak.upgrade() {
                link.show_files();
            }
        });
        d
    }

    fn render(&self, t: &Transfer) {
        match t {
            Transfer::Idle => {}
            Transfer::Waiting(o) => {
                *self.qr_data.borrow_mut() = qr_modules(&o.uri());
                self.qr.queue_draw();
                self.address.set_text(&format!("{} · {}", o.name, o.address));
                self.stack.set_visible_child_name("waiting");
            }
            Transfer::Sending { phone, recordings } => {
                let left = recordings.iter().filter(|r| !r.arrived).count();
                self.sending_title.set_text(&match (recordings.len(), left) {
                    (0, _) => format!("{phone} is connected…"),
                    (_, 0) => format!("{phone} is sending…"),
                    (_, n) => format!("{phone} is sending · {n} to go"),
                });
                self.fill(recordings);
                self.stack.set_visible_child_name("sending");
            }
            Transfer::Finished {
                phone,
                recordings,
                stopped,
            } => {
                let arrived = recordings.iter().filter(|r| r.arrived).count();
                let missing = recordings.len() - arrived;
                self.finished_title.set_text(&match arrived {
                    0 if !stopped => format!("{phone} had nothing new to send"),
                    0 => "Nothing arrived".to_string(),
                    n => format!(
                        "Received {} from {phone}",
                        super::settings_page::plural(n, "recording")
                    ),
                });
                let mut note = String::new();
                match arrived {
                    0 => {}
                    1 => note.push_str("It is in Files, waiting to be transcribed."),
                    _ => note.push_str("They are in Files, waiting to be transcribed."),
                }
                if *stopped && missing > 0 {
                    if !note.is_empty() {
                        note.push(' ');
                    }
                    note.push_str(&format!(
                        "{} did not arrive; scan a new code to send the rest.",
                        super::settings_page::plural(missing, "recording")
                    ));
                }
                self.finished_note.set_text(&note);
                self.finished_note.set_visible(!note.is_empty());
                self.show_files.set_visible(arrived > 0);
                self.stack.set_visible_child_name("finished");
            }
            Transfer::Failed(e) => {
                self.error.set_text(e);
                self.stack.set_visible_child_name("failed");
            }
        }
    }

    fn fill(&self, recordings: &[Incoming]) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        for r in recordings {
            let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
            row.add_css_class("fx-receive-row");
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let title = label(&r.title, &["fx-receive-title"]);
            title.set_hexpand(true);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let amount = if r.arrived {
                "Received".to_string()
            } else {
                format!("{} of {}", megabytes(r.received), megabytes(r.size))
            };
            top.append(&title);
            top.append(&label(&amount, &["fx-status"]));
            row.append(&top);
            let bar = gtk::ProgressBar::new();
            bar.set_fraction(if r.size == 0 {
                0.0
            } else {
                r.received as f64 / r.size as f64
            });
            row.append(&bar);
            self.list.append(&row);
        }
    }
}

fn page_box() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.set_margin_start(24);
    b.set_margin_end(24);
    b.set_margin_bottom(24);
    b.set_margin_top(4);
    b
}

fn note(text: &str) -> gtk::Label {
    let l = label(text, &["fx-field-note"]);
    l.set_wrap(true);
    l.set_max_width_chars(50);
    l
}

fn centered(l: gtk::Label) -> gtk::Label {
    l.set_xalign(0.5);
    l.set_justify(gtk::Justification::Center);
    l
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// The QR code for `text` with a four-module quiet zone.
fn qr_modules(text: &str) -> Option<Qr> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let n = code.width();
    let quiet = 4;
    let width = n + 2 * quiet;
    let colors = code.to_colors();
    let mut dark = vec![false; width * width];
    for y in 0..n {
        for x in 0..n {
            dark[(y + quiet) * width + x + quiet] = colors[y * n + x] == qrcode::Color::Dark;
        }
    }
    Some(Qr { width, dark })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_code_has_a_quiet_zone_and_finder_patterns() {
        let qr = qr_modules("fennec://send?h=192.168.1.20:47130&pin=x&t=q9Xh2kLmN0pQrStUvWxYz1&n=a").unwrap();
        let at = |x: usize, y: usize| qr.dark[y * qr.width + x];
        assert!((0..qr.width).all(|i| !at(i, 0) && !at(0, i)), "quiet zone");
        // The top-left finder: a dark 7×7 ring starting at the quiet zone's edge.
        assert!((4..11).all(|i| at(i, 4) && at(4, i)));
        assert!(!at(5, 5) && at(6, 6));
    }

    #[test]
    fn sizes_read_in_megabytes() {
        assert_eq!(megabytes(3_200_000), "3.2 MB");
        assert_eq!(megabytes(0), "0.0 MB");
    }
}
