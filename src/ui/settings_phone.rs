//! Settings → Phone: receiving recordings from Fennec Recorder, pairing
//! phones with a QR code, and the phones already paired.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gtk::glib;
use gtk::prelude::*;

use super::phone::{LinkState, PhoneLink};
use super::settings_page::{group, page, plural, row, title_block};
use super::{Deps, label};
use crate::store::Store;
use crate::sync::PairingOffer;

/// QR modules plus the quiet zone around them.
struct Qr {
    width: usize,
    dark: Vec<bool>,
}

pub struct PhoneSettingsUi {
    deps: Deps,
    pub link: Rc<PhoneLink>,
    pub enabled: gtk::Switch,
    /// Import over USB.
    pub usb: gtk::Switch,
    status: gtk::Label,
    pub port: gtk::SpinButton,
    pub pair_button: gtk::Button,
    pair_idle: gtk::Box,
    offer_view: gtk::Box,
    qr: gtk::DrawingArea,
    qr_data: Rc<RefCell<Option<Qr>>>,
    address: gtk::Label,
    token: gtk::Label,
    expires: gtk::Label,
    waiting: gtk::Label,
    devices_box: gtk::Box,
    devices_count: gtk::Label,
    pub default_project: gtk::DropDown,
    /// Project ids in the dropdown's order; `None` is Unsorted.
    project_ids: RefCell<Vec<Option<i64>>>,
    countdown: RefCell<Option<glib::SourceId>>,
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
        let enabled = gtk::Switch::new();
        enabled.update_property(&[gtk::accessible::Property::Label("Receive recordings from phones")]);
        let default_project = gtk::DropDown::from_strings(&[]);
        default_project.update_property(&[gtk::accessible::Property::Label("Project for phone recordings")]);
        default_project.set_size_request(220, -1);
        let pair_button = gtk::Button::with_label("Pair a phone");
        pair_button.add_css_class("fx-secondary");
        let qr = gtk::DrawingArea::builder()
            .content_width(172)
            .content_height(172)
            .valign(gtk::Align::Start)
            .build();
        qr.add_css_class("fx-pair-qr");
        qr.update_property(&[gtk::accessible::Property::Label(
            "Pairing code to scan with Fennec Recorder",
        )]);
        let ui = Rc::new(Self {
            deps,
            link,
            enabled,
            usb: {
                let s = gtk::Switch::new();
                s.update_property(&[gtk::accessible::Property::Label("Import over USB")]);
                s
            },
            status: label("", &["fx-field-note"]),
            port,
            pair_button,
            pair_idle: gtk::Box::new(gtk::Orientation::Vertical, 0),
            offer_view: gtk::Box::new(gtk::Orientation::Horizontal, 20),
            qr,
            qr_data: Rc::default(),
            address: label("", &["fx-mono"]),
            token: label("", &["fx-mono", "fx-pair-token"]),
            expires: label("", &["fx-field-note"]),
            waiting: label("", &["fx-pair-waiting"]),
            devices_box: gtk::Box::new(gtk::Orientation::Vertical, 0),
            devices_count: label("", &["fx-count"]),
            default_project,
            project_ids: RefCell::default(),
            countdown: RefCell::default(),
            loading: Cell::new(false),
            on_changed: RefCell::default(),
        });
        let weak = Rc::downgrade(&ui);
        ui.link.connect_changed(move |()| {
            if let Some(ui) = weak.upgrade() {
                ui.refresh();
            }
        });
        ui
    }

    /// Something the sidebar line shows changed.
    pub fn connect_changed(&self, f: impl Fn(()) + 'static) {
        *self.on_changed.borrow_mut() = Some(Rc::new(f));
    }

    /// The line under "Phone" in the sidebar.
    pub fn summary(&self) -> String {
        let phones = Store::open(&self.deps.paths.database())
            .and_then(|s| s.devices())
            .map(|d| d.len())
            .unwrap_or(0);
        match self.link.state() {
            LinkState::Off => "Off".to_string(),
            LinkState::On(_) if phones == 0 => "On · no phones yet".to_string(),
            LinkState::On(_) => format!("On · {}", plural(phones, "phone")),
            LinkState::Failed(_) => "Not receiving".to_string(),
        }
    }

    pub fn section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(760), 28);
        b.append(&title_block(
            "Phone",
            "Record meetings and interviews with Fennec Recorder on your phone. Recordings come straight to this computer over your Wi-Fi network and are transcribed here.",
            None,
        ));
        b.append(&same_network_note());

        let (receiving, rows) = group("Receiving", None);
        let r = row("Receive recordings from phones", None, &self.enabled);
        if let Some(text) = r.first_child().and_downcast::<gtk::Box>() {
            self.status.set_wrap(true);
            text.append(&self.status);
        }
        rows.append(&r);
        self.port.set_valign(gtk::Align::Center);
        rows.append(&row(
            "Port",
            Some("Phones reach Fennec on this port. Change it only if another program uses it."),
            &self.port,
        ));
        b.append(&receiving);

        let (pairing, rows) = group("Pair a phone", None);
        let idle = row(
            "Add a phone",
            Some(
                "Shows a code to scan with Fennec Recorder, on a phone on the same Wi-Fi as this computer. The phone can send recordings once you press Allow here.",
            ),
            &self.pair_button,
        );
        self.pair_idle.append(&idle);
        rows.append(&self.pair_idle);
        rows.append(&self.offer_view());
        b.append(&pairing);

        let (devices, rows) = group("Paired phones", None);
        if let Some(head) = devices.first_child().and_downcast::<gtk::Box>() {
            head.append(&self.devices_count);
        }
        self.devices_box.set_orientation(gtk::Orientation::Vertical);
        rows.append(&self.devices_box);
        b.append(&devices);

        let (cable, rows) = group("USB cable", None);
        rows.append(&row(
            "Import over USB",
            Some("When a phone with Fennec Recorder is plugged in and set to File transfer, Fennec offers to import its recordings. No pairing or shared Wi-Fi needed."),
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
            "Recordings travel encrypted from the phone to this computer and nowhere else. Only phones you pair can send, and removing one stops it at once.",
            &["fx-field-note"],
        );
        privacy.set_wrap(true);
        b.append(&privacy);

        self.wire();
        self.refresh();
        outer
    }

    fn offer_view(self: &Rc<Self>) -> gtk::Box {
        let v = &self.offer_view;
        v.add_css_class("fx-set-row");
        v.add_css_class("fx-pair-offer");
        let data = Rc::clone(&self.qr_data);
        self.qr.set_draw_func(move |_, cr, w, h| {
            let Some(qr) = data.borrow().as_ref().map(|q| (q.width, q.dark.clone())) else {
                return;
            };
            let (width, dark) = qr;
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
        v.append(&self.qr);

        let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
        text.set_hexpand(true);
        text.append(&label("Scan with Fennec Recorder", &["fx-set-title"]));
        let how = label(
            "In the app, choose Pair with Fennec and point the camera at the code.",
            &["fx-field-note"],
        );
        how.set_wrap(true);
        text.append(&how);
        let manual = label("NO CAMERA", &["fx-section-title"]);
        manual.set_margin_top(8);
        text.append(&manual);
        text.append(&self.address);
        text.append(&self.token);
        text.append(&self.expires);
        self.waiting.set_wrap(true);
        self.waiting.set_visible(false);
        text.append(&self.waiting);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("fx-secondary");
        cancel.set_halign(gtk::Align::Start);
        cancel.set_margin_top(8);
        let weak = Rc::downgrade(self);
        cancel.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.link.cancel_pairing();
            }
        });
        text.append(&cancel);
        v.append(&text);
        v.set_visible(false);
        v.clone()
    }

    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.enabled.connect_active_notify(move |sw| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.loading.get() {
                return;
            }
            ui.deps.settings.borrow_mut().phone.enabled = sw.is_active();
            ui.save_and_apply();
        });
        let weak = Rc::downgrade(self);
        self.usb.connect_active_notify(move |sw| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.loading.get() {
                return;
            }
            ui.deps.settings.borrow_mut().phone.usb_import = sw.is_active();
            if let Err(e) = ui.deps.save_settings() {
                ui.status.set_text(&format!("Settings not saved: {e}"));
            }
        });
        let weak = Rc::downgrade(self);
        self.port.connect_value_changed(move |sb| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.loading.get() {
                return;
            }
            ui.deps.settings.borrow_mut().phone.port = sb.value() as u16;
            ui.save_and_apply();
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
            ui.save_and_apply();
        });
        let weak = Rc::downgrade(self);
        self.pair_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.start_pairing();
            }
        });
    }

    fn save_and_apply(&self) {
        if let Err(e) = self.deps.save_settings() {
            self.status.set_text(&format!("Settings not saved: {e}"));
        }
        self.link.apply();
        self.refresh();
    }

    /// Shows a fresh pairing code, turning receiving on if it was off.
    pub fn start_pairing(self: &Rc<Self>) {
        if self.link.offer_pairing().is_none() {
            self.refresh();
            return;
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_seconds_local(1, move || {
            let Some(ui) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            match ui.link.offer() {
                Some(o) if o.expires_at > Instant::now() => {
                    ui.show_countdown(&o);
                    glib::ControlFlow::Continue
                }
                Some(_) => {
                    ui.link.cancel_pairing();
                    ui.countdown.borrow_mut().take();
                    glib::ControlFlow::Break
                }
                None => {
                    ui.countdown.borrow_mut().take();
                    glib::ControlFlow::Break
                }
            }
        });
        if let Some(old) = self.countdown.borrow_mut().replace(id) {
            old.remove();
        }
        self.refresh();
    }

    fn show_countdown(&self, o: &PairingOffer) {
        let left = o.expires_at.saturating_duration_since(Instant::now());
        self.expires.set_text(&format!(
            "Expires in {}:{:02}",
            left.as_secs() / 60,
            left.as_secs() % 60
        ));
    }

    /// Reads everything shown from Settings, the link and the database.
    pub fn refresh(&self) {
        self.loading.set(true);
        let s = self.deps.settings();
        self.enabled.set_active(s.phone.enabled);
        self.usb.set_active(s.phone.usb_import);
        self.port.set_value(f64::from(s.phone.port));
        let state = self.link.state();
        self.status.set_text(&match &state {
            LinkState::Off => "Off. Phones cannot send recordings.".to_string(),
            LinkState::On(addr) => format!("Listening on port {}.", addr.port()),
            LinkState::Failed(e) => e.clone(),
        });
        if matches!(state, LinkState::Failed(_)) {
            self.status.add_css_class("fx-field-error");
        } else {
            self.status.remove_css_class("fx-field-error");
        }

        let offer = self.link.offer();
        self.pair_idle.set_visible(offer.is_none());
        self.offer_view.set_visible(offer.is_some());
        match &offer {
            Some(o) => {
                *self.qr_data.borrow_mut() = qr_modules(&o.uri());
                self.qr.queue_draw();
                self.address.set_text(&o.address);
                self.token.set_text(&format!("code {}", o.token_display()));
                self.show_countdown(o);
            }
            None => {
                self.qr_data.borrow_mut().take();
            }
        }
        match self.link.pending() {
            Some(p) => {
                self.waiting.set_text(&format!(
                    "{} is waiting. Allow it if it shows {}.",
                    p.device_name, p.code
                ));
                self.waiting.set_visible(true);
            }
            None => self.waiting.set_visible(false),
        }

        let store = Store::open(&self.deps.paths.database());
        self.render_devices(store.as_ref().ok());
        let projects = store.and_then(|s| s.projects()).unwrap_or_default();
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

    fn render_devices(&self, store: Option<&Store>) {
        while let Some(c) = self.devices_box.first_child() {
            self.devices_box.remove(&c);
        }
        let devices = store.and_then(|s| s.devices().ok()).unwrap_or_default();
        self.devices_count.set_text(&devices.len().to_string());
        if devices.is_empty() {
            let empty = label("No phones paired yet.", &["fx-field-note", "fx-set-empty"]);
            self.devices_box.append(&empty);
            return;
        }
        for d in devices {
            let remove = gtk::Button::with_label("Remove");
            remove.add_css_class("fx-secondary");
            remove.add_css_class("small");
            remove.add_css_class("danger");
            remove.update_property(&[gtk::accessible::Property::Label(&format!("Remove {}", d.name))]);
            let seen = match d.last_seen_at {
                Some(t) => format!("Last seen {}", ago(t)),
                None => format!("Paired {}", ago(d.paired_at)),
            };
            let note = format!("{seen} · {}", plural(d.recordings, "recording"));
            let r = row(&d.name, Some(&note), &remove);
            let (db, link, id) = (self.deps.paths.database(), Rc::clone(&self.link), d.id);
            remove.connect_clicked(move |_| {
                if let Err(e) = Store::open(&db).and_then(|s| s.remove_device(id)) {
                    tracing::warn!("removing a phone: {e}");
                }
                link.notify_changed();
            });
            self.devices_box.append(&r);
        }
    }
}

/// The rule people must know, at the top of the section.
fn same_network_note() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("fx-network-note");
    let text = label("", &["fx-network-note-text"]);
    text.set_markup(
        "<b>Same Wi-Fi network.</b> To send recordings by themselves, the phone and this computer \
         must be on the same Wi-Fi network, and Fennec must be open. Recordings made elsewhere wait \
         on the phone until then. No shared Wi-Fi? Plug the phone in with a USB cable and choose \
         File transfer on the phone.",
    );
    text.set_wrap(true);
    text.set_hexpand(true);
    b.append(&text);
    b
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

/// "just now", "5 min ago", "3 h ago", or the date.
fn ago(unix_ms: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as i64;
    let mins = (now - unix_ms).max(0) / 60_000;
    match mins {
        0 => "just now".into(),
        1..=59 => format!("{mins} min ago"),
        60..=1439 => format!("{} h ago", mins / 60),
        _ => crate::text::short_date(unix_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_code_has_a_quiet_zone_and_finder_patterns() {
        let qr = qr_modules("fennec://pair?h=192.168.1.20:47130&pin=x&t=12345678&n=a").unwrap();
        let at = |x: usize, y: usize| qr.dark[y * qr.width + x];
        assert!((0..qr.width).all(|i| !at(i, 0) && !at(0, i)), "quiet zone");
        // The top-left finder: a dark 7×7 ring starting at the quiet zone's edge.
        assert!((4..11).all(|i| at(i, 4) && at(4, i)));
        assert!(!at(5, 5) && at(6, 6));
    }

    #[test]
    fn times_since_read_like_a_person_would_say_them() {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 5 * 60_000), "5 min ago");
        assert_eq!(ago(now - 3 * 3_600_000), "3 h ago");
    }
}
