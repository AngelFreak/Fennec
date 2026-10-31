//! The Clean-up review: one card per changed paragraph with a word diff,
//! accept / keep original, and undo. The original stays until accepted.
//! The document's paragraphs show at once; suggestions fill in as the AI
//! sends each part back, under a progress bar with a Stop button.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gtk::glib;
use gtk::prelude::*;

use super::project::locality_word;
use super::{Handler, label};
use crate::ai::actions::{Cleaned, Proofreading};
use crate::ai::diff::{Change, word_diff};
use crate::ai::{AiError, Cancel, Locality};
use crate::config::Settings;
use crate::store::{Paragraph, ParagraphId, Store};
use crate::text::clock;

/// Replaces paragraph text (`from` → `to`); false if the text has changed.
type ApplyHandler = RefCell<Option<Rc<dyn Fn(ParagraphId, &str, &str) -> bool>>>;

/// Highlight colours (background, text) for removed and added words, light
/// and dark; the same as the `fx_accent_soft`/`fx_ai_bg` tokens.
const REMOVED: [(&str, &str); 2] = [("#FDEBDD", "#9A3412"), ("#3A2318", "#F59A6B")];
const ADDED: [(&str, &str); 2] = [("#E8EEFC", "#1E3A8A"), ("#1E2A4A", "#A9C0F5")];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Pending,
    Accepted,
    Kept,
}

struct Row {
    item: Cleaned,
    state: Cell<RowState>,
    /// The paragraph with its changes marked (a label: text views report
    /// the wrong height while wrapping, which clipped long paragraphs).
    text: gtk::Label,
    accept: gtk::Button,
    keep: gtk::Button,
    undo: gtk::Button,
    outcome: gtk::Label,
    note: gtk::Label,
}

/// A paragraph's place in the review: waiting for the AI, then its card.
struct Slot {
    id: ParagraphId,
    text: String,
    place: String,
    holder: gtk::Box,
    done: Cell<bool>,
}

/// How far the AI has got.
#[derive(Default)]
struct Run {
    active: bool,
    loading: bool,
    /// When the parts went out.
    started: Option<Instant>,
    paragraphs: usize,
    done: usize,
    chars: usize,
    done_chars: usize,
    /// How the run ended early ("Stopped…", an error), if it did.
    ended: Option<String>,
}

pub struct CleanupPage {
    pub root: gtk::Box,
    /// Provider, Discard all and Accept remaining, for the window header.
    pub header_actions: gtk::Box,
    pub status: gtk::Label,
    list: gtk::Box,
    progress: gtk::Box,
    bar: gtk::ProgressBar,
    progress_label: gtk::Label,
    stop: gtk::Button,
    slots: RefCell<Vec<Slot>>,
    run: RefCell<Run>,
    cancel: RefCell<Option<Arc<Cancel>>>,
    provider_name: RefCell<String>,
    /// "Accept remaining".
    pub accept_all: gtk::Button,
    /// "Discard all": back to the document without the suggestions.
    pub done: gtk::Button,
    provider: gtk::Box,
    provider_label: gtk::Label,
    store: Rc<Store>,
    settings: Rc<RefCell<Settings>>,
    rows: RefCell<Vec<Rc<Row>>>,
    /// The document is in a local-only project (the provider pill says who
    /// really gets the text).
    local_only: Cell<bool>,
    on_apply: ApplyHandler,
    on_done: Handler<()>,
}

impl CleanupPage {
    pub fn new(store: Rc<Store>, settings: Rc<RefCell<Settings>>) -> Rc<Self> {
        let provider = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        provider.add_css_class("fx-pill");
        provider.set_valign(gtk::Align::Center);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("fx-dot");
        dot.set_valign(gtk::Align::Center);
        let provider_label = gtk::Label::new(None);
        provider.append(&dot);
        provider.append(&provider_label);
        let done = gtk::Button::with_label("Discard all");
        done.add_css_class("fx-secondary");
        done.set_valign(gtk::Align::Center);
        let accept_all = gtk::Button::with_label("Accept remaining");
        accept_all.add_css_class("fx-primary");
        accept_all.set_valign(gtk::Align::Center);
        let header_actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        header_actions.append(&provider);
        header_actions.append(&done);
        header_actions.append(&accept_all);

        let legend = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        legend.add_css_class("fx-cleanup-legend");
        legend.append(&label("removed", &["fx-legend-removed", "fx-legend-strike"]));
        legend.append(&label("added", &["fx-legend-added"]));
        let explain = label(
            "One suggestion per paragraph, so timestamps stay attached. The original is kept for undo.",
            &["fx-stats"],
        );
        explain.set_wrap(true);
        explain.set_hexpand(true);
        legend.append(&explain);
        let status = label("", &["fx-status"]);
        status.set_wrap(true);
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("thin");
        bar.set_hexpand(true);
        bar.set_valign(gtk::Align::Center);
        let progress_label = label("", &["fx-status"]);
        progress_label.set_wrap(true);
        let stop = gtk::Button::with_label("Stop");
        stop.add_css_class("fx-secondary");
        stop.set_valign(gtk::Align::Center);
        let progress_text = gtk::Box::new(gtk::Orientation::Vertical, 8);
        progress_text.set_hexpand(true);
        progress_text.append(&progress_label);
        progress_text.append(&bar);
        let progress = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        progress.add_css_class("fx-cleanup-progress");
        progress.append(&progress_text);
        progress.append(&stop);
        progress.set_visible(false);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
        content.add_css_class("fx-cleanup-page");
        content.append(&legend);
        content.append(&progress);
        content.append(&status);
        content.append(&list);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&content)
            .vexpand(true)
            .hexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.append(&scroller);

        let page = Rc::new(Self {
            root,
            header_actions,
            status,
            list,
            progress,
            bar,
            progress_label,
            stop,
            slots: RefCell::default(),
            run: RefCell::default(),
            cancel: RefCell::default(),
            provider_name: RefCell::default(),
            accept_all,
            done,
            provider,
            provider_label,
            store,
            settings,
            rows: RefCell::default(),
            local_only: Cell::new(false),
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
            if let Some(p) = weak.upgrade() {
                p.discard_all();
            }
        });
        let weak = Rc::downgrade(&page);
        page.stop.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.stop();
            }
        });
        // Leaving the review (Back, Discard all) stops the AI too.
        let weak = Rc::downgrade(&page);
        page.root.connect_unmap(move |_| {
            if let Some(p) = weak.upgrade() {
                p.stop();
            }
        });
        let weak = Rc::downgrade(&page);
        adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(p) = weak.upgrade() {
                for r in p.rows.borrow().iter() {
                    render_text(r);
                }
            }
        });
        page.refresh_provider();
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
        self.slots.borrow_mut().clear();
    }

    /// The pill naming the provider that does the clean-up.
    fn refresh_provider(&self) {
        let settings = self.settings.borrow();
        let active = settings
            .ai
            .effective_for(crate::ai::AiJob::Cleanup, self.local_only.get());
        for c in ["network", "cloud", "local"] {
            self.provider.remove_css_class(c);
        }
        self.provider.add_css_class(match active.map(|p| p.locality) {
            Some(Locality::Network) => "network",
            Some(Locality::Cloud) => "cloud",
            _ => "local",
        });
        self.provider_label.set_text(&match active {
            Some(p) => format!("{} · {}", p.name, locality_word(p.locality)),
            None => "No provider set up".into(),
        });
        *self.provider_name.borrow_mut() = active.map(|p| p.name.clone()).unwrap_or_else(|| "the AI".into());
    }

    /// The provider pill's text (tests).
    pub fn provider_text(&self) -> String {
        self.provider_label.text().to_string()
    }

    /// Whether the document under review is local only.
    pub fn set_local_only(&self, on: bool) {
        self.local_only.set(on);
        self.refresh_provider();
    }

    /// Starts a review of `paragraphs`: they show at once, waiting.
    pub fn begin(self: &Rc<Self>, paragraphs: &[Paragraph]) {
        self.clear();
        self.refresh_provider();
        self.lay_out(paragraphs);
        *self.run.borrow_mut() = Run {
            active: true,
            paragraphs: self.slots.borrow().len(),
            chars: self.slots.borrow().iter().map(|s| s.text.len()).sum(),
            ..Run::default()
        };
        self.progress.set_visible(true);
        self.stop.set_sensitive(true);
        self.stop.set_label("Stop");
        self.bar.set_fraction(0.0);
        self.refresh_progress();
        self.refresh_status();
        // The clock and the bar move between parts.
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            match weak.upgrade() {
                Some(p) if p.run.borrow().active => {
                    p.refresh_progress();
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            }
        });
    }

    /// The flag that stops the running job.
    pub fn set_cancel(&self, cancel: Arc<Cancel>) {
        *self.cancel.borrow_mut() = Some(cancel);
    }

    /// Stops the AI; what came back so far stays to review.
    pub fn stop(&self) {
        if !self.run.borrow().active {
            return;
        }
        if let Some(c) = self.cancel.borrow().as_ref() {
            c.cancel();
        }
        self.stop.set_sensitive(false);
        self.stop.set_label("Stopping…");
    }

    /// Whether the AI is still working (tests).
    pub fn is_running(&self) -> bool {
        self.run.borrow().active
    }

    /// The progress line (tests).
    pub fn progress_text(&self) -> String {
        self.progress_label.text().to_string()
    }

    pub fn progress(self: &Rc<Self>, event: Proofreading) {
        if !self.run.borrow().active {
            return;
        }
        match event {
            Proofreading::Starting => self.run.borrow_mut().loading = true,
            Proofreading::Working { .. } => {
                let mut run = self.run.borrow_mut();
                run.loading = false;
                run.started = Some(Instant::now());
            }
            Proofreading::Part { covered, changed } => {
                let chars: usize = self
                    .slots
                    .borrow()
                    .iter()
                    .filter(|s| covered.contains(&s.id))
                    .map(|s| s.text.len())
                    .sum();
                {
                    let mut run = self.run.borrow_mut();
                    run.done += covered.len();
                    run.done_chars += chars;
                }
                self.fill(&covered, changed);
            }
        }
        self.refresh_progress();
    }

    pub fn finish(self: &Rc<Self>, result: Result<Vec<Cleaned>, AiError>) {
        let ended = match result {
            Ok(items) => {
                // Anything not yet shown part by part.
                let waiting: Vec<ParagraphId> = self
                    .slots
                    .borrow()
                    .iter()
                    .filter(|s| !s.done.get())
                    .map(|s| s.id)
                    .collect();
                self.fill(&waiting, items);
                None
            }
            Err(e) => {
                let run = self.run.borrow();
                Some(match e {
                    AiError::Cancelled => {
                        format!("Stopped after {} of {} paragraphs.", run.done, run.paragraphs)
                    }
                    e => super::ai::error_text(&e),
                })
            }
        };
        for slot in self.slots.borrow().iter().filter(|s| !s.done.get()) {
            set_side_note(slot, "Not proofread");
        }
        {
            let mut run = self.run.borrow_mut();
            run.active = false;
            run.ended = ended;
        }
        *self.cancel.borrow_mut() = None;
        self.progress.set_visible(false);
        self.refresh_status();
    }

    /// Shows a finished set of suggestions for the document they belong to.
    pub fn show(self: &Rc<Self>, items: Vec<Cleaned>) {
        self.clear();
        let paragraphs = self.document_paragraphs(items.first().map(|i| i.paragraph_id));
        self.lay_out(&paragraphs);
        *self.run.borrow_mut() = Run::default();
        let all: Vec<ParagraphId> = self.slots.borrow().iter().map(|s| s.id).collect();
        self.fill(&all, items);
        self.refresh_status();
    }

    /// One waiting card per paragraph, in document order.
    fn lay_out(&self, paragraphs: &[Paragraph]) {
        let mut slots = self.slots.borrow_mut();
        for (i, p) in paragraphs.iter().enumerate() {
            let Some(id) = p.id else { continue };
            if p.text.trim().is_empty() {
                continue;
            }
            let place = p.start_ms.map_or_else(|| format!("¶{}", i + 1), clock);
            let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
            holder.append(&plain_card(&p.text, &place, "Waiting…", "waiting"));
            self.list.append(&holder);
            slots.push(Slot {
                id,
                text: p.text.clone(),
                place,
                holder,
                done: Cell::new(false),
            });
        }
    }

    /// Fills the `covered` paragraphs' cards: a suggestion where one came
    /// back, otherwise "No changes suggested".
    fn fill(self: &Rc<Self>, covered: &[ParagraphId], changed: Vec<Cleaned>) {
        let mut changed: HashMap<ParagraphId, Cleaned> =
            changed.into_iter().map(|c| (c.paragraph_id, c)).collect();
        let slots = self.slots.borrow();
        for slot in slots.iter() {
            if slot.done.get() || !(covered.contains(&slot.id) || changed.contains_key(&slot.id)) {
                continue;
            }
            slot.done.set(true);
            while let Some(c) = slot.holder.first_child() {
                slot.holder.remove(&c);
            }
            match changed.remove(&slot.id) {
                Some(item) => {
                    let (row, card) = self.card(item, &slot.place);
                    slot.holder.append(&card);
                    self.rows.borrow_mut().push(row);
                }
                None => slot.holder.append(&plain_card(
                    &slot.text,
                    &slot.place,
                    "No changes suggested",
                    "unchanged",
                )),
            }
        }
        // A paragraph that is not in the list still gets its card.
        for item in changed.into_values() {
            if slots.iter().any(|s| s.id == item.paragraph_id) {
                continue;
            }
            let (row, card) = self.card(item, "");
            self.list.append(&card);
            self.rows.borrow_mut().push(row);
        }
        // Rows follow the text, whatever order the parts came back in.
        let order: HashMap<ParagraphId, usize> = slots.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
        self.rows
            .borrow_mut()
            .sort_by_key(|r| order.get(&r.item.paragraph_id).copied().unwrap_or(usize::MAX));
        drop(slots);
        self.refresh_status();
    }

    /// The line over the bar: loading, then how far, then about how long.
    fn refresh_progress(&self) {
        let run = self.run.borrow();
        if !run.active {
            return;
        }
        let name = self.provider_name.borrow().clone();
        let text = if run.loading {
            self.bar.pulse();
            format!("Loading {name}. The first proofread after a while waits for the model to load.")
        } else if let Some(started) = run.started.filter(|_| run.done > 0) {
            self.bar
                .set_fraction(run.done_chars as f64 / run.chars.max(1) as f64);
            let elapsed = started.elapsed().as_secs_f64();
            let left =
                elapsed * (run.chars - run.done_chars.min(run.chars)) as f64 / run.done_chars.max(1) as f64;
            let left = if left < 60.0 {
                "under a minute left".to_string()
            } else {
                format!("about {} min left", (left / 60.0).round() as u64)
            };
            format!("{} of {} paragraphs proofread · {left}", run.done, run.paragraphs)
        } else {
            self.bar.pulse();
            let waited = run.started.map_or(0, |s| s.elapsed().as_secs());
            format!(
                "{name} is proofreading {} paragraphs · {}:{:02}",
                run.paragraphs,
                waited / 60,
                waited % 60
            )
        };
        self.progress_label.set_text(&text);
    }

    /// The paragraphs of the document holding `paragraph`.
    fn document_paragraphs(&self, paragraph: Option<ParagraphId>) -> Vec<crate::store::Paragraph> {
        let Some(paragraph) = paragraph else {
            return Vec::new();
        };
        match self.store.paragraph_document(paragraph) {
            Ok(Some(doc)) => self.store.paragraphs(doc).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn card(self: &Rc<Self>, item: Cleaned, place: &str) -> (Rc<Row>, gtk::Box) {
        let card = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        card.add_css_class("fx-diff-card");
        let ts = label(place, &["fx-cleanup-ts"]);
        ts.set_size_request(72, -1);
        ts.set_valign(gtk::Align::Start);
        card.append(&ts);

        let text = label("", &["fx-diff"]);
        text.set_wrap(true);
        text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        text.set_xalign(0.0);
        text.set_valign(gtk::Align::Start);
        text.set_hexpand(true);
        text.set_selectable(true);
        text.update_property(&[gtk::accessible::Property::Label("Suggested change")]);
        card.append(&text);

        let buttons = gtk::Box::new(gtk::Orientation::Vertical, 6);
        buttons.set_size_request(200, -1);
        buttons.set_hexpand(false);
        buttons.add_css_class("fx-cleanup-buttons");
        let accept = gtk::Button::with_label("Accept");
        accept.add_css_class("fx-ink");
        let keep = gtk::Button::with_label("Keep original");
        keep.add_css_class("fx-secondary");
        let outcome = label("", &["fx-cleanup-outcome"]);
        outcome.set_visible(false);
        let undo = gtk::Button::with_label("Undo");
        undo.add_css_class("fx-secondary");
        undo.add_css_class("fx-cleanup-undo");
        undo.set_visible(false);
        let note = label("", &["fx-field-note"]);
        note.set_wrap(true);
        note.set_visible(false);
        // As in the mockup: the two choices stacked full width, then the
        // outcome with Undo once one is made.
        buttons.append(&accept);
        buttons.append(&keep);
        buttons.append(&outcome);
        buttons.append(&undo);
        buttons.append(&note);
        card.append(&buttons);

        let row = Rc::new(Row {
            item,
            state: Cell::new(RowState::Pending),
            text,
            accept: accept.clone(),
            keep: keep.clone(),
            undo: undo.clone(),
            outcome,
            note,
        });
        render_text(&row);
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
        (row, card)
    }

    fn apply(&self, from: &str, to: &str, id: ParagraphId) -> bool {
        self.on_apply.borrow().clone().is_some_and(|f| f(id, from, to))
    }

    fn set_note(row: &Row, text: &str) {
        row.note.set_text(text);
        row.note.set_visible(!text.is_empty());
    }

    fn accept(&self, row: &Row) {
        if row.state.get() != RowState::Pending {
            return;
        }
        if self.apply(&row.item.original, &row.item.cleaned, row.item.paragraph_id) {
            self.set_state(row, RowState::Accepted);
        } else {
            Self::set_note(row, "The paragraph was edited meanwhile; not changed.");
        }
    }

    fn undo(&self, row: &Row) {
        if row.state.get() == RowState::Accepted
            && !self.apply(&row.item.cleaned, &row.item.original, row.item.paragraph_id)
        {
            Self::set_note(row, "The paragraph was edited meanwhile; cannot undo.");
            return;
        }
        self.set_state(row, RowState::Pending);
    }

    fn set_state(&self, row: &Row, state: RowState) {
        row.state.set(state);
        let pending = state == RowState::Pending;
        row.accept.set_visible(pending);
        row.keep.set_visible(pending);
        row.outcome.set_visible(!pending);
        row.undo.set_visible(!pending);
        row.outcome.set_text(match state {
            RowState::Pending => "",
            RowState::Accepted => "Accepted",
            RowState::Kept => "Kept original",
        });
        Self::set_note(row, "");
        render_text(row);
        self.refresh_status();
    }

    pub fn accept_all(&self) {
        let rows = self.rows.borrow().clone();
        for r in rows {
            self.accept(&r);
        }
    }

    /// Leaves the review with the document as it was: accepted suggestions
    /// are undone. Once nothing is left to review, this is "Done".
    fn discard_all(&self) {
        let rows = self.rows.borrow().clone();
        if rows.iter().any(|r| r.state.get() == RowState::Pending) {
            for r in rows.iter().filter(|r| r.state.get() == RowState::Accepted) {
                self.undo(r);
            }
        }
        if let Some(f) = self.on_done.borrow().clone() {
            f(());
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

    /// Cards for paragraphs without suggestions (tests).
    pub fn unchanged_count(&self) -> usize {
        self.slots
            .borrow()
            .iter()
            .filter(|s| {
                s.holder
                    .first_child()
                    .is_some_and(|c| c.has_css_class("unchanged"))
            })
            .count()
    }

    pub fn states(&self) -> Vec<RowState> {
        self.rows.borrow().iter().map(|r| r.state.get()).collect()
    }

    fn refresh_status(&self) {
        let rows = self.rows.borrow();
        let run = self.run.borrow();
        let pending = rows.iter().filter(|r| r.state.get() == RowState::Pending).count();
        self.accept_all.set_sensitive(pending > 0);
        self.done
            .set_label(if pending > 0 || rows.is_empty() || run.active {
                "Discard all"
            } else {
                "Done"
            });
        if run.active {
            self.status.set_visible(false);
            return;
        }
        let summary = match (rows.len(), pending, &run.ended) {
            (0, _, Some(_)) => String::new(),
            (0, _, None) => "Nothing to clean up — the text already reads well.".to_string(),
            (n, 0, _) => format!("All {n} suggestions reviewed."),
            (n, p, _) => format!("{p} of {n} suggestions to review. The original stays until you accept."),
        };
        self.status.set_visible(pending == 0 || run.ended.is_some());
        self.status.set_text(
            match &run.ended {
                Some(e) => format!("{e} {summary}"),
                None => summary,
            }
            .trim(),
        );
    }
}

/// A paragraph without a suggestion (yet): its text and a note beside it.
fn plain_card(text: &str, place: &str, note: &str, class: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    card.add_css_class("fx-diff-card");
    card.add_css_class(class);
    let ts = label(place, &["fx-cleanup-ts"]);
    ts.set_size_request(72, -1);
    ts.set_valign(gtk::Align::Start);
    card.append(&ts);
    let t = label(text, &["fx-diff"]);
    t.set_wrap(true);
    t.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    t.set_xalign(0.0);
    t.set_valign(gtk::Align::Start);
    t.set_hexpand(true);
    card.append(&t);
    let side = gtk::Box::new(gtk::Orientation::Vertical, 6);
    side.set_size_request(200, -1);
    side.append(&label(note, &["fx-cleanup-outcome"]));
    card.append(&side);
    card
}

/// Changes a waiting card's note ("Not proofread").
fn set_side_note(slot: &Slot, note: &str) {
    if let Some(l) = slot
        .holder
        .first_child()
        .and_then(|card| card.last_child())
        .and_then(|side| side.first_child())
        .and_downcast::<gtk::Label>()
    {
        l.set_text(note);
    }
}

/// Fills the card's text: the diff while pending, otherwise the chosen text.
fn render_text(row: &Row) {
    use gtk::glib::markup_escape_text as esc;
    let markup =
        match row.state.get() {
            RowState::Pending => {
                // Highlight colours for the current light or dark style.
                let i = usize::from(adw::StyleManager::default().is_dark());
                let ((rbg, rfg), (abg, afg)) = (REMOVED[i], ADDED[i]);
                word_diff(&row.item.original, &row.item.cleaned)
                .into_iter()
                .map(|change| match change {
                    Change::Same(t) => esc(&t).to_string(),
                    Change::Removed(t) => format!(
                        "<span strikethrough=\"true\" background=\"{rbg}\" foreground=\"{rfg}\">{}</span>",
                        esc(&t)
                    ),
                    Change::Added(t) => {
                        format!("<span background=\"{abg}\" foreground=\"{afg}\">{}</span>", esc(&t))
                    }
                })
                .collect()
            }
            RowState::Accepted => esc(&row.item.cleaned).to_string(),
            RowState::Kept => esc(&row.item.original).to_string(),
        };
    row.text.set_markup(&markup);
}
