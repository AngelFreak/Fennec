//! Clean-up: suggestions for the inspection, reviewed paragraph by paragraph.

use std::time::Duration;

use fennec::ui::Nav;

use crate::{Scene, pump_until};

pub fn capture(s: &Scene) {
    s.w.sidebar.go(Nav::Dictate);
    s.w.dictation.open_document(s.besigtigelse).unwrap();
    s.w.dictation.ai_cleanup();
    let proposed = pump_until(Duration::from_secs(10), || {
        !s.w.dictation.cleanup.states().is_empty()
    });
    if !proposed {
        println!("FAILED no clean-up suggestions");
    }
    s.shot("cleanup");
}
