//! Dictate: recording into "Besigtigelse Nørregade 14", as the mockup shows
//! it, then the AI menu, the Summary and the Actions views.

use std::collections::BTreeMap;
use std::time::Duration;

use fennec::ui::DockState;
use fennec::ui::Nav;
use gtk::prelude::*;

use crate::{PREVIEW, Scene, pump};

pub fn capture(s: &Scene) {
    let d = &s.w.dictation;
    s.w.sidebar.go(Nav::Dictate);
    d.open_document(s.besigtigelse).unwrap();
    // Mid-dictation: the dock records, the last sentence is still a preview.
    d.dock.set_state(DockState::Recording);
    d.dock.set_timer(4 * 60 + 12);
    d.dock
        .set_status("Listening. Text is committed when you pause briefly.", false);
    for level in [
        0.002, 0.006, 0.003, 0.012, 0.004, 0.002, 0.008, 0.02, 0.006, 0.003, 0.01, 0.004, 0.007, 0.003,
        0.005, 0.002,
    ] {
        d.dock.push_level(level);
    }
    // The mockup catches the moment after a pause: a new paragraph has begun.
    let buffer = d.editor.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, "\n");
    buffer.place_cursor(&end);
    d.editor.set_preview(Some(PREVIEW));
    let mut suggestions = BTreeMap::new();
    suggestions.insert("emne".to_string(), "Fugt i kælder og revner i puds".to_string());
    d.inspector.show_suggestions(&suggestions);
    d.show_view("transcript");
    s.shot("dictate");

    d.ai_menu.popup();
    pump(Duration::from_millis(300));
    s.shot("dictate-ai-menu");
    d.ai_menu.popdown();

    d.show_view("summary");
    s.shot("summary");
    d.show_view("actions");
    s.shot("actions");
    d.show_view("transcript");
}
