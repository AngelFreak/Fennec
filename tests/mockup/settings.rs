//! Settings: one shot per section, named as the mockup's.

use fennec::ui::Nav;

use crate::Scene;

pub fn capture(s: &Scene) {
    s.w.sidebar.go(Nav::Settings);
    for (id, shot) in [
        ("model", "settings-speech-model"),
        ("dictation", "settings-dictation"),
        ("ai", "settings-ai-providers"),
        ("ai-defaults", "settings-ai-defaults"),
        ("privacy", "settings-privacy"),
        ("storage", "settings-storage"),
    ] {
        s.w.settings.show_section(id);
        s.shot(shot);
    }
}
