//! Templates: Notat open in the editor.

use fennec::ui::Nav;

use crate::Scene;

pub fn capture(s: &Scene) {
    s.w.sidebar.go(Nav::Templates);
    s.w.templates.reload(Some("notat"));
    s.shot("templates");
}
