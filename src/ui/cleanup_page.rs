//! The Clean-up review: one card per changed paragraph with a word diff,
//! accept / keep original, and undo. The original stays until accepted.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use super::{Handler, label};
use crate::ai::AiError;
use crate::ai::actions::Cleaned;
use crate::ai::diff::{Change, word_diff};
use crate::store::ParagraphId;

/// Replaces paragraph text (`from` → `to`); false if the text has changed.
type ApplyHandler = RefCell<Option<Rc<dyn Fn(ParagraphId, &str, &str) -> bool>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Pending,
    Accepted,
    Kept,
}

struct Row {
    item: Cleaned,
    state: Cell<RowState>,
    accept: gtk::Button,
    keep: gtk::Button,
    undo: gtk::Button,
    note: gtk::Label,
}

pub struct CleanupPage {
    pub root: gtk::Box,
    /// Buttons for the window header while this screen shows.
    pub header_actions: gtk::Box,
    title: gtk::Label,
    pub status: gtk::Label,
    list: gtk::Box,
    pub accept_all: gtk::Button,
    pub done: gtk::Button,
    rows: RefCell<Vec<Rc<Row>>>,
    on_apply: ApplyHandler,
    on_done: Handler<()>,
}

impl CleanupPage {
    pub fn new() -> Rc<Self> {
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 4);
        titles.set_hexpand(true);
        let title = label("Clean up", &["fx-project-title"]);
        let status = label("", &["fx-status"]);
        status.set_wrap(true);
        titles.append(&title);
        titles.append(&status);
        let accept_all = gtk::Button::with_label("Accept all");
        accept_all.add_css_class("fx-secondary");
        accept_all.set_valign(gtk::Align::Center);
        let done = gtk::Button::with_label("Back to the document");
        done.add_css_class("fx-primary");
        done.set_valign(gtk::Align::Center);
        head.append(&titles);
        head.append(&accept_all);
        head.append(&done);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let clamp = adw::Clamp::builder().maximum_size(760).child(&list).build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&clamp)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 18);
        root.add_css_class("fx-project-main");
        root.set_hexpand(true);
        root.append(&head);
        root.append(&scroller);

        let page = Rc::new(Self {
            root,
            header_actions: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            title,
            status,
            list,
            accept_all,
            done,
            rows: RefCell::default(),
            on_apply: RefCell::default(),
            on_done: RefCell::default(),
        });
        let weak = Rc::downgrade(&page);
        page.accept_all.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.accept_all();
            }
        });
        let weak = Rc::downgrade(&page);
        page.done.connect_clicked(move |_| {
            if let Some(f) = weak.upgrade().and_then(|p| p.on_done.borrow().clone()) {
                f(());
            }
        });
        page
    }

    pub fn connect_apply(&self, f: impl Fn(ParagraphId, &str, &str) -> bool + 'static) {
        *self.on_apply.borrow_mut() = Some(Rc::new(f));
    }

    pub fn connect_done(&self, f: impl Fn(()) + 'static) {
        *self.on_done.borrow_mut() = Some(Rc::new(f));
    }

    fn clear(&self) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.rows.borrow_mut().clear();
    }

    pub fn begin(&self, doc_title: &str) {
        self.clear();
        self.title.set_text(&format!("Clean up · {doc_title}"));
        self.status.set_text("Asking the AI for a cleaned-up version…");
        self.accept_all.set_sensitive(false);
    }

    pub fn finish(self: &Rc<Self>, result: Result<Vec<Cleaned>, AiError>) {
        match result {
            Ok(items) => self.show(items),
            Err(e) => self.status.set_text(&super::ai::error_text(&e)),
        }
    }

    pub fn show(self: &Rc<Self>, items: Vec<Cleaned>) {
        self.clear();
        for item in items {
            let row = self.card(item);
            self.rows.borrow_mut().push(row);
        }
        self.refresh_status();
    }

    fn card(self: &Rc<Self>, item: Cleaned) -> Rc<Row> {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("fx-cleanup-card");
        let view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .css_classes(["fx-diff"])
            .build();
        let buffer = view.buffer();
        let removed = buffer
            .create_tag(
                Some("removed"),
                &[("strikethrough", &true), ("foreground", &"#B42318")],
            )
            .expect("new tag");
        let added = buffer
            .create_tag(
                Some("added"),
                &[
                    ("underline", &gtk::pango::Underline::Single),
                    ("foreground", &"#067647"),
                ],
            )
            .expect("new tag");
        for change in word_diff(&item.original, &item.cleaned) {
            let mut end = buffer.end_iter();
            match change {
                Change::Same(t) => buffer.insert(&mut end, &t),
                Change::Removed(t) => buffer.insert_with_tags(&mut end, &t, &[&removed]),
                Change::Added(t) => buffer.insert_with_tags(&mut end, &t, &[&added]),
            }
        }
        card.append(&view);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let note = label("", &["fx-field-note"]);
        note.set_hexpand(true);
        let accept = gtk::Button::with_label("Accept");
        accept.add_css_class("fx-primary");
        let keep = gtk::Button::with_label("Keep original");
        keep.add_css_class("fx-secondary");
        let undo = gtk::Button::with_label("Undo");
        undo.add_css_class("fx-secondary");
        undo.set_visible(false);
        buttons.append(&note);
        buttons.append(&undo);
        buttons.append(&keep);
        buttons.append(&accept);
        card.append(&buttons);
        self.list.append(&card);

        let row = Rc::new(Row {
            item,
            state: Cell::new(RowState::Pending),
            accept: accept.clone(),
            keep: keep.clone(),
            undo: undo.clone(),
            note,
        });
        for (button, action) in [(accept, 0), (keep, 1), (undo, 2)] {
            let weak = Rc::downgrade(self);
            let r = Rc::downgrade(&row);
            button.connect_clicked(move |_| {
                if let (Some(p), Some(r)) = (weak.upgrade(), r.upgrade()) {
                    match action {
                        0 => p.accept(&r),
                        1 => p.set_state(&r, RowState::Kept),
                        _ => p.undo(&r),
                    }
                }
            });
        }
        row
    }

    fn apply(&self, from: &str, to: &str, id: ParagraphId) -> bool {
        self.on_apply.borrow().clone().is_some_and(|f| f(id, from, to))
    }

    fn accept(&self, row: &Row) {
        if row.state.get() != RowState::Pending {
            return;
        }
        if self.apply(&row.item.original, &row.item.cleaned, row.item.paragraph_id) {
            self.set_state(row, RowState::Accepted);
        } else {
            row.note
                .set_text("The paragraph was edited meanwhile; not changed.");
        }
    }

    fn undo(&self, row: &Row) {
        if row.state.get() == RowState::Accepted
            && !self.apply(&row.item.cleaned, &row.item.original, row.item.paragraph_id)
        {
            row.note
                .set_text("The paragraph was edited meanwhile; cannot undo.");
            return;
        }
        self.set_state(row, RowState::Pending);
    }

    fn set_state(&self, row: &Row, state: RowState) {
        row.state.set(state);
        let pending = state == RowState::Pending;
        row.accept.set_visible(pending);
        row.keep.set_visible(pending);
        row.undo.set_visible(!pending);
        row.note.set_text(match state {
            RowState::Pending => "",
            RowState::Accepted => "Accepted",
            RowState::Kept => "Original kept",
        });
        self.refresh_status();
    }

    pub fn accept_all(&self) {
        let rows = self.rows.borrow().clone();
        for r in rows {
            self.accept(&r);
        }
    }

    /// Accepts row `i` (tests).
    pub fn accept_row(&self, i: usize) {
        let row = self.rows.borrow().get(i).cloned();
        if let Some(r) = row {
            self.accept(&r);
        }
    }

    /// Undoes row `i` (tests).
    pub fn undo_row(&self, i: usize) {
        let row = self.rows.borrow().get(i).cloned();
        if let Some(r) = row {
            self.undo(&r);
        }
    }

    pub fn states(&self) -> Vec<RowState> {
        self.rows.borrow().iter().map(|r| r.state.get()).collect()
    }

    fn refresh_status(&self) {
        let rows = self.rows.borrow();
        let pending = rows.iter().filter(|r| r.state.get() == RowState::Pending).count();
        self.accept_all.set_sensitive(pending > 0);
        self.status.set_text(&match (rows.len(), pending) {
            (0, _) => "Nothing to clean up — the text already reads well.".to_string(),
            (n, 0) => format!("All {n} suggestions reviewed."),
            (n, p) => format!("{p} of {n} suggestions to review. The original stays until you accept."),
        });
    }
}
