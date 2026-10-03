//! Drives the real window with the real model and microphone, and reports
//! every time the main loop is blocked long enough to drop frames. Needs a
//! display, a microphone and the default model, so it only runs when asked:
//!   FENNEC_RESPONSIVENESS=1 cargo test --release --features vulkan --test responsiveness

use std::cell::RefCell;
use std::time::{Duration, Instant};

use fennec::config::{Paths, Settings};
use fennec::ui::{self, Deps, Nav};
use gtk::glib;
use gtk::prelude::*;

/// Two frames at 60 Hz.
const STALL: Duration = Duration::from_millis(33);

thread_local! {
    static PHASE: RefCell<String> = RefCell::default();
    static STALLS: RefCell<Vec<(String, Duration)>> = RefCell::default();
}

fn phase(name: &str) {
    PHASE.with(|p| *p.borrow_mut() = name.into());
}

fn note(took: Duration, what: &str) {
    if took >= STALL {
        let at = PHASE.with(|p| p.borrow().clone());
        STALLS.with(|s| s.borrow_mut().push((format!("{at}: {what}"), took)));
    }
}

/// Runs `f` on the main thread and notes it if it blocked.
fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    note(t.elapsed(), what);
    out
}

fn pump_until(timeout: Duration, mut done: impl FnMut() -> bool) -> (bool, Duration) {
    let start = Instant::now();
    let ctx = glib::MainContext::default();
    loop {
        if done() {
            return (true, start.elapsed());
        }
        if start.elapsed() > timeout {
            return (false, start.elapsed());
        }
        let t = Instant::now();
        ctx.iteration(false);
        note(t.elapsed(), "main loop");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn pump_for(d: Duration) {
    pump_until(d, || false);
}

fn main() {
    if std::env::var_os("FENNEC_RESPONSIVENESS").is_none() {
        println!("skipped: set FENNEC_RESPONSIVENESS=1 (needs a display, a microphone and the model)");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let user = Paths::user();
    let paths = Paths::under(tmp.path());
    std::fs::create_dir_all(paths.models().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(user.models(), paths.models()).unwrap();
    adw::init().expect("a display");
    ui::load_css();

    phase("launch");
    let w = timed("build_window", || {
        ui::build_window(Deps::real(paths.clone(), Settings::default()))
    });
    w.window.present();
    pump_for(Duration::from_millis(500));

    for visit in ["first", "again"] {
        for (name, nav) in [
            ("files", Nav::Files),
            ("templates", Nav::Templates),
            ("settings", Nav::Settings),
            ("dictate", Nav::Dictate),
        ] {
            phase(&format!("open {name} ({visit})"));
            timed("show", || w.show(nav));
            pump_for(Duration::from_millis(300));
        }
    }

    let mut report = Vec::new();
    for round in ["cold", "warm"] {
        phase(&format!("start ({round})"));
        timed("start_recording", || w.dictation.start_recording());
        let (ok, took) = pump_until(Duration::from_secs(20), || w.dictation.is_recording());
        assert!(ok, "recording did not start: {}", w.dictation.dock.status_text());
        report.push(format!(
            "start ({round}): recording after {} ms",
            took.as_millis()
        ));
        phase(&format!("recording ({round})"));
        pump_for(Duration::from_secs(3));
        phase(&format!("stop ({round})"));
        timed("stop_recording", || w.dictation.stop_recording());
        let (ok, took) = pump_until(Duration::from_secs(20), || {
            w.dictation.dock.state_text() == "Ready"
        });
        assert!(ok, "did not stop");
        report.push(format!("stop ({round}): ready after {} ms", took.as_millis()));
    }

    for r in report {
        println!("{r}");
    }
    let stalls = STALLS.with(|s| s.borrow().clone());
    for (what, took) in &stalls {
        println!("STALL {:>5} ms  {what}", took.as_millis());
    }
    println!("{} stalls of {} ms or more", stalls.len(), STALL.as_millis());
}
