//! Settings → Dictation: test the microphone. Records five seconds through
//! the same audio path dictation uses, shows the level as it comes in,
//! judges it, and plays the recording back.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::{Deps, label};
use crate::audio::level::{Verdict, assess, measure};
use crate::engine::SAMPLE_RATE;

const SECONDS: usize = 5;

enum Msg {
    Level(f32),
    Done(Result<(Vec<(f32, f32)>, PathBuf), String>),
}

pub struct MicTest {
    pub root: gtk::Box,
    pub button: gtk::Button,
    pub meter: gtk::LevelBar,
    pub verdict: gtk::Label,
    pub play: gtk::Button,
    deps: Deps,
    recording: RefCell<Option<PathBuf>>,
    media: RefCell<Option<gtk::MediaFile>>,
    last: RefCell<Option<Verdict>>,
}

impl MicTest {
    pub fn new(deps: Deps) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let button = gtk::Button::with_label("Test microphone");
        button.add_css_class("fx-secondary");
        let meter = gtk::LevelBar::for_interval(0.0, 1.0);
        meter.set_hexpand(true);
        meter.set_valign(gtk::Align::Center);
        meter.add_css_class("fx-meter");
        meter.update_property(&[gtk::accessible::Property::Label("Microphone level")]);
        // Below the offsets the bar is green-ish; near the top it warns.
        meter.add_offset_value("fx-low", 0.25);
        meter.add_offset_value("fx-ok", 0.85);
        meter.add_offset_value("fx-hot", 1.0);
        let play = gtk::Button::with_label("Play back");
        play.add_css_class("fx-secondary");
        play.set_sensitive(false);
        row.append(&button);
        row.append(&meter);
        row.append(&play);
        let verdict = label(
            "Speak normally for five seconds to check the level.",
            &["fx-field-note"],
        );
        verdict.set_wrap(true);
        root.append(&row);
        root.append(&verdict);
        let t = Rc::new(Self {
            root,
            button,
            meter,
            verdict,
            play,
            deps,
            recording: RefCell::default(),
            media: RefCell::default(),
            last: RefCell::default(),
        });
        let weak = Rc::downgrade(&t);
        t.button.connect_clicked(move |_| {
            if let Some(t) = weak.upgrade() {
                t.run();
            }
        });
        let weak = Rc::downgrade(&t);
        t.play.connect_clicked(move |_| {
            if let Some(t) = weak.upgrade() {
                t.play_back();
            }
        });
        t
    }

    /// The last test's result (tests).
    pub fn verdict(&self) -> Option<Verdict> {
        *self.last.borrow()
    }

    pub fn run(self: &Rc<Self>) {
        self.button.set_sensitive(false);
        self.play.set_sensitive(false);
        self.verdict.remove_css_class("fx-field-error");
        self.verdict.set_text("Listening… speak normally.");
        let settings = self.deps.settings();
        let audio = std::sync::Arc::clone(&self.deps.audio);
        let path = std::env::temp_dir().join(format!("fennec-mic-test-{}.wav", std::process::id()));
        let (tx, rx) = async_channel::unbounded::<Msg>();
        std::thread::spawn(move || {
            let result = (|| -> Result<(Vec<(f32, f32)>, PathBuf), String> {
                let mut source = audio(&settings)?;
                let mut samples = Vec::new();
                let mut blocks = Vec::new();
                while samples.len() < SECONDS * SAMPLE_RATE as usize {
                    match source.next_chunk().map_err(|e| e.to_string())? {
                        Some(chunk) => {
                            let (peak, rms) = measure(&chunk);
                            blocks.push((peak, rms));
                            let _ = tx.send_blocking(Msg::Level(rms));
                            samples.extend(chunk);
                        }
                        None => break,
                    }
                }
                crate::audio::write_wav_16k_mono(&path, &samples).map_err(|e| e.to_string())?;
                Ok((blocks, path))
            })();
            let _ = tx.send_blocking(Msg::Done(result));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let Some(t) = weak.upgrade() else { return };
                match msg {
                    Msg::Level(rms) => t.meter.set_value(meter_value(rms)),
                    Msg::Done(result) => {
                        t.finish(result);
                        return;
                    }
                }
            }
        });
    }

    fn finish(&self, result: Result<(Vec<(f32, f32)>, PathBuf), String>) {
        self.button.set_sensitive(true);
        self.meter.set_value(0.0);
        match result {
            Ok((blocks, path)) => {
                let v = assess(&blocks);
                self.verdict.set_text(v.message());
                if v != Verdict::Good {
                    self.verdict.add_css_class("fx-field-error");
                }
                *self.last.borrow_mut() = Some(v);
                *self.recording.borrow_mut() = Some(path);
                self.play.set_sensitive(true);
            }
            Err(e) => {
                self.verdict.add_css_class("fx-field-error");
                self.verdict
                    .set_text(&format!("The microphone could not be opened: {e}"));
                *self.last.borrow_mut() = None;
            }
        }
    }

    fn play_back(&self) {
        let Some(path) = self.recording.borrow().clone() else {
            return;
        };
        let media = gtk::MediaFile::for_filename(&path);
        media.play();
        *self.media.borrow_mut() = Some(media);
    }
}

/// RMS on a decibel scale: -60 dBFS is empty, 0 dBFS is full.
fn meter_value(rms: f32) -> f64 {
    let db = 20.0 * rms.max(1e-6).log10();
    (f64::from(db) + 60.0).clamp(0.0, 60.0) / 60.0
}

#[cfg(test)]
mod tests {
    use super::meter_value;

    #[test]
    fn the_meter_follows_decibels() {
        assert_eq!(meter_value(0.0), 0.0);
        assert_eq!(meter_value(1.0), 1.0);
        assert!((meter_value(0.1) - 2.0 / 3.0).abs() < 1e-6);
    }
}
