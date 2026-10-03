//! The record bar: button, state and timer, level meter, status, shortcut.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::label;
use crate::audio::level::{Bars, Meter};

const BARS: usize = 16;

pub struct Dock {
    pub root: gtk::Box,
    pub record: gtk::Button,
    state: gtk::Label,
    timer: gtk::Label,
    status: gtk::Label,
    meter: gtk::DrawingArea,
    /// Bar heights, oldest first.
    levels: Rc<RefCell<Bars>>,
    meter_scale: RefCell<Meter>,
    /// Scrolls the bars each frame, while recording.
    ticking: RefCell<Option<gtk::TickCallbackId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockState {
    Idle,
    Loading,
    Recording,
    /// Stop was pressed; the last sentence is still being transcribed.
    Finishing,
}

impl Dock {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        root.add_css_class("fx-dock");

        let record = gtk::Button::from_icon_name("fennec-mic-symbolic");
        record.add_css_class("fx-record");
        record.set_valign(gtk::Align::Center);

        let state = label("Ready", &["fx-rec-state"]);
        let timer = label("00:00", &["fx-timer"]);
        let state_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        state_box.set_valign(gtk::Align::Center);
        state_box.set_size_request(92, -1);
        state_box.append(&state);
        state_box.append(&timer);

        let levels = Rc::new(RefCell::new(Bars::new(BARS)));
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
            for (i, height) in draw_levels.borrow().heights().iter().enumerate() {
                let bar_h = (f64::from(h) * f64::from(*height)).max(4.0);
                let x = i as f64 * 7.0;
                let y = (f64::from(h) - bar_h) / 2.0;
                cr.rectangle(x, y, 4.0, bar_h);
            }
            let _ = cr.fill();
        });

        let status = label("Press the button to start dictating.", &["fx-status"]);
        status.set_hexpand(true);
        // Up to two lines, as in the mockup; the full message is in the
        // tooltip when it does not fit.
        status.set_wrap(true);
        status.set_lines(2);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_width_chars(10);
        status.set_max_width_chars(40);

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
            meter_scale: RefCell::default(),
            ticking: RefCell::default(),
        };
        dock.set_state(DockState::Idle);
        dock
    }

    pub fn set_state(&self, s: DockState) {
        let (text, icon, tip, sensitive) = match s {
            DockState::Idle => ("Ready", "fennec-mic-symbolic", "Start dictation", true),
            DockState::Loading => (
                "Loading…",
                "content-loading-symbolic",
                "Loading the speech model",
                false,
            ),
            DockState::Recording => ("Recording", "fennec-stop-symbolic", "Stop dictation", true),
            DockState::Finishing => (
                "Finishing…",
                "fennec-mic-symbolic",
                "Finishing the last sentence",
                false,
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
            // A new session may use another microphone in another room.
            *self.meter_scale.borrow_mut() = Meter::default();
            self.start_ticking();
        } else {
            self.record.remove_css_class("recording");
            if let Some(t) = self.ticking.borrow_mut().take() {
                t.remove();
            }
            self.levels.borrow_mut().clear();
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

    /// Bar heights from 0 to 1, oldest first.
    pub fn bar_heights(&self) -> Vec<f32> {
        self.levels.borrow().heights()
    }

    pub fn status_text(&self) -> String {
        self.status.text().to_string()
    }

    pub fn set_timer(&self, secs: u64) {
        self.timer.set_text(&format!("{:02}:{:02}", secs / 60, secs % 60));
    }

    fn start_ticking(&self) {
        if self.ticking.borrow().is_some() {
            return;
        }
        let levels = Rc::clone(&self.levels);
        let last_frame = std::cell::Cell::new(None::<i64>);
        let id = self.meter.add_tick_callback(move |area, clock| {
            let now = clock.frame_time();
            let secs = last_frame
                .replace(Some(now))
                .map_or(0.0, |t| (now - t) as f64 / 1e6);
            if levels.borrow_mut().advance(secs) {
                area.queue_draw();
            }
            glib::ControlFlow::Continue
        });
        *self.ticking.borrow_mut() = Some(id);
    }

    pub fn push_level(&self, level: f32) {
        let height = self.meter_scale.borrow_mut().height(level);
        // Drawn by the frame clock (see `new`), at a steady pace.
        self.levels.borrow_mut().push(height);
    }
}
