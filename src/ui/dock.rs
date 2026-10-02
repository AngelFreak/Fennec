//! The record bar: button, state and timer, level meter, status, shortcut.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::prelude::*;

use super::label;

const BARS: usize = 16;

pub struct Dock {
    pub root: gtk::Box,
    pub record: gtk::Button,
    state: gtk::Label,
    timer: gtk::Label,
    status: gtk::Label,
    meter: gtk::DrawingArea,
    levels: Rc<RefCell<VecDeque<f32>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockState {
    Idle,
    Loading,
    Recording,
}

impl Dock {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        root.add_css_class("fx-dock");

        let record = gtk::Button::from_icon_name("audio-input-microphone-symbolic");
        record.add_css_class("fx-record");
        record.set_valign(gtk::Align::Center);

        let state = label("Ready", &["fx-rec-state"]);
        let timer = label("00:00", &["fx-timer"]);
        let state_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        state_box.set_valign(gtk::Align::Center);
        state_box.set_size_request(92, -1);
        state_box.append(&state);
        state_box.append(&timer);

        let levels = Rc::new(RefCell::new(VecDeque::from(vec![0.0; BARS])));
        let meter = gtk::DrawingArea::builder()
            .content_width(BARS as i32 * 7)
            .content_height(48)
            .build();
        meter.set_valign(gtk::Align::Center);
        meter.update_property(&[gtk::accessible::Property::Label("Input level")]);
        let draw_levels = Rc::clone(&levels);
        let draw_record = record.clone();
        meter.set_draw_func(move |area, cr, _w, h| {
            let color = area.color();
            let recording = draw_record.has_css_class("recording");
            let (r, g, b) = if recording {
                (0.76, 0.25, 0.05)
            } else {
                (
                    f64::from(color.red()),
                    f64::from(color.green()),
                    f64::from(color.blue()),
                )
            };
            cr.set_source_rgba(r, g, b, if recording { 1.0 } else { 0.25 });
            for (i, level) in draw_levels.borrow().iter().enumerate() {
                // Speech sits around 0.01–0.1 RMS; a log scale makes it visible.
                let v = ((f64::from(*level).max(1e-4).log10() + 4.0) / 3.0).clamp(0.08, 1.0);
                let bar_h = (f64::from(h) * v).max(4.0);
                let x = i as f64 * 7.0;
                let y = (f64::from(h) - bar_h) / 2.0;
                cr.rectangle(x, y, 4.0, bar_h);
            }
            let _ = cr.fill();
        });

        let status = label(
            "Press the button or Ctrl+Space to start dictating.",
            &["fx-status"],
        );
        status.set_hexpand(true);
        // One line; the full message is in the tooltip when it does not fit.
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_width_chars(10);

        let kbd = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        kbd.set_valign(gtk::Align::Center);
        kbd.append(&label("Ctrl", &["fx-kbd"]));
        kbd.append(&label("+", &["fx-status"]));
        kbd.append(&label("Space", &["fx-kbd"]));

        root.append(&record);
        root.append(&state_box);
        root.append(&meter);
        root.append(&status);
        root.append(&kbd);

        let dock = Self {
            root,
            record,
            state,
            timer,
            status,
            meter,
            levels,
        };
        dock.set_state(DockState::Idle);
        dock
    }

    pub fn set_state(&self, s: DockState) {
        let (text, icon, tip, sensitive) = match s {
            DockState::Idle => (
                "Ready",
                "audio-input-microphone-symbolic",
                "Start dictation",
                true,
            ),
            DockState::Loading => (
                "Loading…",
                "content-loading-symbolic",
                "Loading the speech model",
                false,
            ),
            DockState::Recording => (
                "Recording",
                "media-playback-stop-symbolic",
                "Stop dictation",
                true,
            ),
        };
        self.state.set_text(text);
        self.record.set_icon_name(icon);
        self.record.set_tooltip_text(Some(tip));
        self.record
            .update_property(&[gtk::accessible::Property::Label(tip)]);
        self.record.set_sensitive(sensitive);
        if s == DockState::Recording {
            self.record.add_css_class("recording");
        } else {
            self.record.remove_css_class("recording");
            self.levels.borrow_mut().iter_mut().for_each(|l| *l = 0.0);
            self.meter.queue_draw();
        }
    }

    pub fn state_text(&self) -> String {
        self.state.text().to_string()
    }

    pub fn set_status(&self, text: &str, error: bool) {
        self.status.set_text(text);
        self.status.set_tooltip_text(Some(text));
        if error {
            self.status.add_css_class("error");
        } else {
            self.status.remove_css_class("error");
        }
    }

    /// Whether the status shows a problem the user should still see.
    pub fn status_is_error(&self) -> bool {
        self.status.has_css_class("error")
    }

    pub fn status_text(&self) -> String {
        self.status.text().to_string()
    }

    pub fn set_timer(&self, secs: u64) {
        self.timer.set_text(&format!("{:02}:{:02}", secs / 60, secs % 60));
    }

    pub fn push_level(&self, level: f32) {
        let mut l = self.levels.borrow_mut();
        l.pop_front();
        l.push_back(level);
        drop(l);
        self.meter.queue_draw();
    }
}
