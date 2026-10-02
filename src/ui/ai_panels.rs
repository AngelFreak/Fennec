//! Panels that show AI results: a summary (streamed, editable, saved) and a
//! list of action items. Used by the Dictate screen and the Project view.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::label;
use crate::ai::AiError;
use crate::ai::service::Summarized;
use crate::store::{ActionItem, Store, Summary};

pub struct SummaryPanel {
    pub root: gtk::Box,
    pub run: gtk::Button,
    pub status: gtk::Label,
    pub badge: gtk::Label,
    pub text: gtk::TextView,
    pub include: gtk::CheckButton,
    store: Rc<Store>,
    current: Cell<Option<i64>>,
    loading: Cell<bool>,
    save_timer: RefCell<Option<glib::SourceId>>,
}

impl SummaryPanel {
    pub fn new(store: Rc<Store>, run_label: &str) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let run = gtk::Button::with_label(run_label);
        run.add_css_class("fx-secondary");
        let status = label("", &["fx-field-note"]);
        status.set_wrap(true);
        let badge = label("", &["fx-ai-badge"]);
        badge.set_wrap(true);
        let text = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .css_classes(["fx-summary-text"])
            .vexpand(true)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        text.update_property(&[gtk::accessible::Property::Label("Summary")]);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&text)
            .vexpand(true)
            .min_content_height(160)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let include = gtk::CheckButton::with_label("Include in export");
        root.append(&run);
        root.append(&status);
        root.append(&badge);
        root.append(&scroller);
        root.append(&include);
        let panel = Rc::new(Self {
            root,
            run,
            status,
            badge,
            text,
            include,
            store,
            current: Cell::new(None),
            loading: Cell::new(false),
            save_timer: RefCell::default(),
        });
        let weak = Rc::downgrade(&panel);
        panel.text.buffer().connect_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.schedule_save();
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.include.connect_toggled(move |_| {
            if let Some(p) = weak.upgrade() {
                p.save();
            }
        });
        panel.load(None);
        panel
    }

    /// Shows the newest stored summary, or an empty panel.
    pub fn load(&self, latest: Option<Summary>) {
        self.loading.set(true);
        match &latest {
            Some(s) => {
                self.text.buffer().set_text(&s.text);
                self.badge.set_text(&badge_text(&s.provider, &s.model));
                self.include.set_active(s.include_in_export);
                self.status.set_text("");
            }
            None => {
                self.text.buffer().set_text("");
                self.badge.set_text("");
                self.include.set_active(true);
                self.status.set_text("No summary yet.");
            }
        }
        self.current.set(latest.map(|s| s.id));
        self.badge.set_visible(self.current.get().is_some());
        self.include.set_visible(self.current.get().is_some());
        self.loading.set(false);
    }

    pub fn begin(&self) {
        self.loading.set(true);
        self.text.buffer().set_text("");
        self.loading.set(false);
        self.status.set_text("Writing…");
        self.run.set_sensitive(false);
    }

    pub fn append(&self, delta: &str) {
        self.loading.set(true);
        let buffer = self.text.buffer();
        buffer.insert(&mut buffer.end_iter(), delta);
        self.loading.set(false);
    }

    pub fn finish(&self, result: Result<Summarized, AiError>) {
        self.run.set_sensitive(true);
        match result {
            Ok(s) => {
                self.loading.set(true);
                self.text.buffer().set_text(&s.text);
                self.loading.set(false);
                self.current.set(s.id);
                self.badge.set_text(&badge_text(&s.provider, &s.model));
                self.badge.set_visible(true);
                self.include.set_active(true);
                self.include.set_visible(s.id.is_some());
                self.status.set_text("");
            }
            Err(e) => {
                // No partial result: what streamed in is cleared.
                self.loading.set(true);
                self.text.buffer().set_text("");
                self.loading.set(false);
                self.status.set_text(&super::ai::error_text(&e));
            }
        }
    }

    pub fn summary_text(&self) -> String {
        let b = self.text.buffer();
        b.text(&b.start_iter(), &b.end_iter(), false).to_string()
    }

    fn schedule_save(self: &Rc<Self>) {
        if self.loading.get() {
            return;
        }
        if let Some(t) = self.save_timer.take() {
            t.remove();
        }
        let weak = Rc::downgrade(self);
        *self.save_timer.borrow_mut() = Some(glib::timeout_add_local_once(
            std::time::Duration::from_millis(800),
            move || {
                if let Some(p) = weak.upgrade() {
                    p.save_timer.take();
                    p.save();
                }
            },
        ));
    }

    /// Writes edits and the export choice back to the stored summary.
    pub fn save(&self) {
        if self.loading.get() {
            return;
        }
        if let Some(t) = self.save_timer.take() {
            t.remove();
        }
        if let Some(id) = self.current.get()
            && let Err(e) = self
                .store
                .update_summary(id, &self.summary_text(), self.include.is_active())
        {
            tracing::error!("saving summary {id}: {e}");
            self.status.set_text(&format!("Not saved: {e}"));
        }
    }
}

fn badge_text(provider: &str, model: &str) -> String {
    if model.is_empty() {
        format!("AI-generated · {provider}")
    } else {
        format!("AI-generated · {provider} · {model}")
    }
}

pub struct ActionsPanel {
    pub root: gtk::Box,
    pub run: gtk::Button,
    pub status: gtk::Label,
    list: gtk::Box,
    store: Rc<Store>,
    shown: RefCell<Vec<ActionItem>>,
}

impl ActionsPanel {
    pub fn new(store: Rc<Store>, run_label: Option<&str>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let run = gtk::Button::with_label(run_label.unwrap_or(""));
        run.add_css_class("fx-secondary");
        run.set_visible(run_label.is_some());
        let status = label("", &["fx-field-note"]);
        status.set_wrap(true);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        root.append(&run);
        root.append(&status);
        root.append(&scroller);
        let panel = Rc::new(Self {
            root,
            run,
            status,
            list,
            store,
            shown: RefCell::default(),
        });
        panel.show(Vec::new());
        panel
    }

    pub fn begin(&self) {
        self.run.set_sensitive(false);
        self.status.set_text("Looking for action items…");
    }

    pub fn finish(self: &Rc<Self>, result: Result<Vec<ActionItem>, AiError>) {
        self.run.set_sensitive(true);
        match result {
            Ok(items) => self.show(items),
            Err(e) => self.status.set_text(&super::ai::error_text(&e)),
        }
    }

    pub fn show(self: &Rc<Self>, items: Vec<ActionItem>) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.status.set_text(match items.len() {
            0 => "No action items.",
            _ => "",
        });
        for item in &items {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("fx-action-item");
            let check = gtk::CheckButton::new();
            check.set_active(item.done);
            check.set_valign(gtk::Align::Start);
            check.update_property(&[gtk::accessible::Property::Label(&item.what)]);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let what = label(&item.what, &["fx-action-what"]);
            what.set_wrap(true);
            text.append(&what);
            let meta: Vec<String> = [item.who.clone(), item.due.as_deref().map(due_text)]
                .into_iter()
                .flatten()
                .collect();
            if !meta.is_empty() {
                text.append(&label(&meta.join(" · "), &["fx-field-note"]));
            }
            row.append(&check);
            row.append(&text);
            let id = item.id;
            let weak = Rc::downgrade(self);
            check.connect_toggled(move |c| {
                if let Some(p) = weak.upgrade()
                    && let Err(e) = p.store.set_action_done(id, c.is_active())
                {
                    tracing::error!("saving action item {id}: {e}");
                }
            });
            self.list.append(&row);
        }
        *self.shown.borrow_mut() = items;
    }

    /// Texts of the shown items (tests).
    pub fn items(&self) -> Vec<String> {
        self.shown.borrow().iter().map(|i| i.what.clone()).collect()
    }
}

/// "9. oktober 2026" for an ISO date; anything else unchanged.
fn due_text(iso: &str) -> String {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map(crate::text::danish_day)
        .unwrap_or_else(|_| iso.to_string())
}
