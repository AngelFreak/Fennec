//! Project: Operation Harbour's documents, a tag filter, the action
//! roll-up and an answered question.

use std::time::Duration;

use fennec::store::ProjectFilter;
use fennec::ui::Nav;

use crate::{Scene, pump_until};

pub fn capture(s: &Scene) {
    let p = &s.w.project;
    s.w.sidebar.go(Nav::Project(ProjectFilter::Project(s.harbour)));
    p.show_tab("docs");
    s.shot("project");
    p.set_tag_filter(Some("meeting"));
    s.shot("project-tag");
    p.set_tag_filter(None);
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
