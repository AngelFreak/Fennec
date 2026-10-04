//! Project: Operation Harbour's documents, a tag filter, selected documents
//! with the bulk Tags menu, a row's ⋯ menu, the action roll-up and an
//! answered question.

use std::time::Duration;

use fennec::store::ProjectFilter;
use fennec::ui::Nav;

use crate::{Scene, pump, pump_until};

pub fn capture(s: &Scene) {
    let p = &s.w.project;
    s.w.sidebar.go(Nav::Project(ProjectFilter::Project(s.harbour)));
    p.show_tab("docs");
    s.shot("project");
    p.set_tag_filter(Some("meeting"));
    s.shot("project-tag");
    p.set_tag_filter(None);
    for id in p.shown_ids().into_iter().take(2) {
        p.select_document(id, true);
    }
    p.open_bulk_tags();
    s.shot("project-bulk");
    p.close_menu();
    p.clear_selection();
    pump(Duration::from_millis(300));
    p.open_row_menu(1);
    s.shot("project-row-menu");
    p.close_menu();
    p.show_tab("actions");
    s.shot("project-actions");
    p.set_question("Hvad lovede Acme om levering?");
    p.ask();
    let answered = pump_until(Duration::from_secs(10), || !p.citation_labels().is_empty());
    if !answered {
        println!("FAILED no answer: {}", p.ask_status.text());
    }
    s.shot("project-ask");
}
