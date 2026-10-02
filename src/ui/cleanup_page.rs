//! The Clean-up review: one card per changed paragraph with a word diff,
//! accept / keep original, and undo. The original stays until accepted.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;

use super::project::locality_word;
use super::{Handler, label};
use crate::ai::actions::Cleaned;
use crate::ai::diff::{Change, word_diff};
use crate::ai::{AiError, Locality};
use crate::config::Settings;
use crate::store::{DocumentFilter, ParagraphId, Store};
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

pub struct CleanupPage {
    pub root: gtk::Box,
    /// Provider, Discard all and Accept remaining, for the window header.
    pub header_actions: gtk::Box,
    pub status: gtk::Label,
    list: gtk::Box,
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

        let list = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
        content.add_css_class("fx-cleanup-page");
        content.append(&legend);
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

    pub fn begin(&self, _doc_title: &str) {
        self.clear();
        self.refresh_provider();
        self.status.set_visible(true);
        self.status.set_text("Asking the AI for a cleaned-up version…");
        self.accept_all.set_sensitive(false);
        self.done.set_label("Discard all");
    }

    pub fn finish(self: &Rc<Self>, result: Result<Vec<Cleaned>, AiError>) {
        match result {
            Ok(items) => self.show(items),
            Err(e) => {
                self.status.set_visible(true);
                self.status.set_text(&super::ai::error_text(&e));
            }
        }
    }

    pub fn show(self: &Rc<Self>, items: Vec<Cleaned>) {
        self.clear();
        let places = self.places(&items);
        // In document order, as in the mockup: paragraphs the AI left alone
        // are listed too, marked as such, so the review reads like the text.
        let mut items: HashMap<ParagraphId, Cleaned> =
            items.into_iter().map(|i| (i.paragraph_id, i)).collect();
        for p in self.document_paragraphs(items.keys().next().copied()) {
            let Some(id) = p.id else { continue };
            let place = places.get(&id).cloned().unwrap_or_default();
            match items.remove(&id) {
                Some(item) => {
                    let row = self.card(item, &place);
                    self.rows.borrow_mut().push(row);
                }
                None if !p.text.trim().is_empty() => self.unchanged_card(&p.text, &place),
                None => {}
            }
        }
        // Anything not found in the document still gets its card.
        for item in items.into_values() {
            let row = self.card(item, "");
            self.rows.borrow_mut().push(row);
        }
        self.refresh_status();
    }

    /// The paragraphs of the document holding `paragraph`.
    fn document_paragraphs(&self, paragraph: Option<ParagraphId>) -> Vec<crate::store::Paragraph> {
        let Some(paragraph) = paragraph else {
            return Vec::new();
        };
        self.store
            .documents(&DocumentFilter::default())
            .unwrap_or_default()
            .into_iter()
            .map(|d| self.store.paragraphs(d.id).unwrap_or_default())
            .find(|ps| ps.iter().any(|p| p.id == Some(paragraph)))
            .unwrap_or_default()
    }

    /// A paragraph the AI suggested nothing for.
    fn unchanged_card(&self, text: &str, place: &str) {
        let card = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        card.add_css_class("fx-diff-card");
        card.add_css_class("unchanged");
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
        side.append(&label("No changes suggested", &["fx-cleanup-outcome"]));
        card.append(&side);
        self.list.append(&card);
    }

    /// Where each paragraph sits: its start time, or ¶n without audio.
    fn places(&self, items: &[Cleaned]) -> HashMap<ParagraphId, String> {
        let mut out = HashMap::new();
        let Some(first) = items.first() else {
            return out;
        };
        // Clean-up works on one document; find it by its paragraphs.
        let docs = self
            .store
            .documents(&DocumentFilter::default())
            .unwrap_or_default();
        for d in docs {
            let paragraphs = self.store.paragraphs(d.id).unwrap_or_default();
            if !paragraphs.iter().any(|p| p.id == Some(first.paragraph_id)) {
                continue;
            }
            for (i, p) in paragraphs.iter().enumerate() {
                if let Some(id) = p.id {
                    let place = p.start_ms.map_or_else(|| format!("¶{}", i + 1), clock);
                    out.insert(id, place);
                }
            }
            break;
        }
        out
    }

    fn card(self: &Rc<Self>, item: Cleaned, place: &str) -> Rc<Row> {
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
        self.list.append(&card);

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
        row
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
        let mut n = 0;
        let mut c = self.list.first_child();
        while let Some(w) = c {
            n += usize::from(w.has_css_class("unchanged"));
            c = w.next_sibling();
        }
        n
    }

    pub fn states(&self) -> Vec<RowState> {
        self.rows.borrow().iter().map(|r| r.state.get()).collect()
    }

    fn refresh_status(&self) {
        let rows = self.rows.borrow();
        let pending = rows.iter().filter(|r| r.state.get() == RowState::Pending).count();
        self.accept_all.set_sensitive(pending > 0);
        self.done.set_label(if pending > 0 || rows.is_empty() {
            "Discard all"
        } else {
            "Done"
        });
        self.status.set_visible(pending == 0);
        self.status.set_text(&match (rows.len(), pending) {
            (0, _) => "Nothing to clean up — the text already reads well.".to_string(),
            (n, 0) => format!("All {n} suggestions reviewed."),
            (n, p) => format!("{p} of {n} suggestions to review. The original stays until you accept."),
        });
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
