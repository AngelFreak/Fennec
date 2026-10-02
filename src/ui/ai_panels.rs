//! Views of AI results, laid out as in the design: a summary (streamed,
//! editable, saved) and a table of action items. Used by the Dictate
//! screen's tabs and the Project view.

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
    /// "Summarise …" (empty state) and "Regenerate" both run this.
    pub run: gtk::Button,
    regenerate: gtk::Button,
    pub status: gtk::Label,
    pub badge: gtk::Label,
    meta: gtk::Label,
    badge_row: gtk::Box,
    content: gtk::Box,
    empty: gtk::Box,
    pub text: gtk::TextView,
    pub include: gtk::CheckButton,
    store: Rc<Store>,
    current: Cell<Option<i64>>,
    loading: Cell<bool>,
    save_timer: RefCell<Option<glib::SourceId>>,
    /// The summary as read: paragraphs, headings and bullets.
    rendered: gtk::Box,
    edit: gtk::Button,
    editing: Cell<bool>,
    place_of: RefCell<Option<PlaceOf>>,
}

impl SummaryPanel {
    pub fn new(store: Rc<Store>, run_label: &str) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 16);

        let empty = gtk::Box::new(gtk::Orientation::Vertical, 12);
        empty.set_halign(gtk::Align::Start);
        let empty_text = label(
            "No summary yet. The AI writes one from the text; you can edit it afterwards.",
            &["fx-body"],
        );
        empty_text.set_wrap(true);
        let run = gtk::Button::with_label(run_label);
        run.add_css_class("fx-secondary");
        run.set_halign(gtk::Align::Start);
        empty.append(&empty_text);
        empty.append(&run);

        let badge_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let badge = label("AI-GENERATED", &["fx-badge", "ai"]);
        badge.set_valign(gtk::Align::Center);
        let meta = label("", &["fx-stats"]);
        meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
        badge_row.append(&badge);
        badge_row.append(&meta);

        let text = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .css_classes(["fx-summary-text"])
            .pixels_below_lines(10)
            .build();
        text.update_property(&[gtk::accessible::Property::Label("Summary")]);
        // Read as formatted text; Edit swaps in the raw text to change it.
        text.set_visible(false);
        let rendered = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let boxed = gtk::Box::new(gtk::Orientation::Vertical, 0);
        boxed.add_css_class("fx-ai-box");
        boxed.append(&rendered);
        boxed.append(&text);

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let include = gtk::CheckButton::with_label("Include in export ({summary} slot)");
        include.set_hexpand(true);
        let regenerate = gtk::Button::with_label("Regenerate");
        regenerate.add_css_class("fx-secondary");
        let edit = gtk::Button::with_label("Edit");
        edit.add_css_class("fx-secondary");
        controls.append(&include);
        controls.append(&regenerate);
        controls.append(&edit);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.append(&badge_row);
        content.append(&boxed);
        content.append(&controls);

        let status = label("", &["fx-field-note"]);
        status.set_wrap(true);

        root.append(&empty);
        root.append(&content);
        root.append(&status);
        let panel = Rc::new(Self {
            root,
            run,
            regenerate,
            status,
            badge,
            meta,
            badge_row,
            content,
            empty,
            text,
            include,
            store,
            current: Cell::new(None),
            loading: Cell::new(false),
            save_timer: RefCell::default(),
            rendered,
            edit,
            editing: Cell::new(false),
            place_of: RefCell::default(),
        });
        let weak = Rc::downgrade(&panel);
        panel.edit.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.set_editing(!p.editing.get());
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.text.buffer().connect_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                if !p.editing.get() {
                    p.render();
                }
                p.schedule_save();
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.include.connect_toggled(move |_| {
            if let Some(p) = weak.upgrade() {
                p.save();
            }
        });
        let run = panel.run.clone();
        panel.regenerate.connect_clicked(move |_| run.emit_clicked());
        panel.load(None);
        panel
    }

    fn show_content(&self, yes: bool) {
        self.content.set_visible(yes);
        self.empty.set_visible(!yes);
    }

    fn set_meta(&self, provider: &str, model: &str, prompt: Option<&str>, at_ms: Option<i64>) {
        let place = self.place_of.borrow().as_ref().and_then(|f| f(provider));
        self.meta
            .set_text(&meta_line(provider, model, place.as_deref(), prompt, at_ms));
    }

    /// Tells the panel where a provider (by name) runs: "cloud", "network"…
    pub fn set_place_of(&self, f: impl Fn(&str) -> Option<String> + 'static) {
        *self.place_of.borrow_mut() = Some(Box::new(f));
    }

    /// Redraws the read-only view from the text.
    fn render(&self) {
        while let Some(c) = self.rendered.first_child() {
            self.rendered.remove(&c);
        }
        let mut previous: Option<Block> = None;
        for block in summary_blocks(&self.summary_text()) {
            // Paragraphs and headings are set apart; a list sits close
            // under its heading and its items close together.
            let gap = match (&previous, &block) {
                (None, _) => 0,
                (Some(Block::Bullet(_)), Block::Bullet(_)) => 0,
                (Some(Block::Heading(_)), Block::Bullet(_)) => 4,
                (_, Block::Heading(_)) => 14,
                _ => 12,
            };
            previous = Some(block.clone());
            let row: gtk::Widget = match block {
                Block::Paragraph(t) => markup_label(&t, &["fx-summary-para"]).upcast(),
                Block::Heading(t) => markup_label(&t, &["fx-summary-heading"]).upcast(),
                Block::Bullet(t) => {
                    let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                    b.add_css_class("fx-summary-bullet");
                    let dot = label("•", &["fx-summary-para"]);
                    dot.set_valign(gtk::Align::Start);
                    b.append(&dot);
                    let l = markup_label(&t, &["fx-summary-para"]);
                    l.set_hexpand(true);
                    b.append(&l);
                    b.upcast()
                }
            };
            row.set_margin_top(gap);
            self.rendered.append(&row);
        }
    }

    fn set_editing(&self, on: bool) {
        if !on && self.editing.get() {
            self.save();
        }
        self.editing.set(on);
        self.text.set_visible(on);
        self.rendered.set_visible(!on);
        self.edit.set_label(if on { "Done" } else { "Edit" });
        if on {
            self.text.grab_focus();
        } else {
            self.render();
        }
    }

    /// Edit / Done (tests).
    pub fn press_edit(&self) {
        self.edit.emit_clicked();
    }

    pub fn is_editing(&self) -> bool {
        self.editing.get()
    }

    /// The text as shown when not editing, one string per block (tests).
    pub fn rendered_blocks(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut c = self.rendered.first_child();
        while let Some(w) = c {
            let text = match w.downcast_ref::<gtk::Label>() {
                Some(l) => l.text().to_string(),
                None => w
                    .last_child()
                    .and_downcast::<gtk::Label>()
                    .map(|l| format!("• {}", l.text()))
                    .unwrap_or_default(),
            };
            out.push(text);
            c = w.next_sibling();
        }
        out
    }

    /// Shows the newest stored summary, or the empty state.
    pub fn load(&self, latest: Option<Summary>) {
        self.loading.set(true);
        match &latest {
            Some(s) => {
                self.text.buffer().set_text(&s.text);
                self.set_meta(&s.provider, &s.model, Some(&s.prompt_id), Some(s.created_at));
                self.include.set_active(s.include_in_export);
                self.status.set_text("");
                self.show_content(true);
            }
            None => {
                self.text.buffer().set_text("");
                self.meta.set_text("");
                self.include.set_active(true);
                self.status.set_text("");
                self.show_content(false);
            }
        }
        self.current.set(latest.map(|s| s.id));
        self.include.set_visible(self.current.get().is_some());
        self.loading.set(false);
    }

    pub fn begin(&self) {
        self.loading.set(true);
        self.text.buffer().set_text("");
        self.loading.set(false);
        self.show_content(true);
        self.meta.set_text("Writing…");
        self.status.set_text("");
        self.run.set_sensitive(false);
        self.regenerate.set_sensitive(false);
    }

    pub fn append(&self, delta: &str) {
        self.loading.set(true);
        let buffer = self.text.buffer();
        buffer.insert(&mut buffer.end_iter(), delta);
        self.loading.set(false);
    }

    pub fn finish(&self, result: Result<Summarized, AiError>) {
        self.run.set_sensitive(true);
        self.regenerate.set_sensitive(true);
        match result {
            Ok(s) => {
                self.loading.set(true);
                self.text.buffer().set_text(&s.text);
                self.loading.set(false);
                self.current.set(s.id);
                self.set_meta(&s.provider, &s.model, Some("summary"), Some(now_ms()));
                self.include.set_active(true);
                self.include.set_visible(s.id.is_some());
                self.status.set_text("");
                self.show_content(true);
            }
            Err(e) => {
                // No partial result: what streamed in is cleared.
                self.loading.set(true);
                self.text.buffer().set_text("");
                self.loading.set(false);
                self.show_content(self.current.get().is_some());
                self.status.set_text(&super::ai::error_text(&e));
            }
        }
    }

    pub fn summary_text(&self) -> String {
        let b = self.text.buffer();
        b.text(&b.start_iter(), &b.end_iter(), false).to_string()
    }

    /// "Claude (claude-opus-5-5) · today 10:52" (tests).
    pub fn meta_text(&self) -> String {
        self.meta.text().to_string()
    }

    pub fn badge_visible(&self) -> bool {
        self.badge_row.is_visible() && self.content.get_visible()
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

fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// "today 10:52", or a Danish date for older results.
fn when(ms: i64) -> String {
    let Some(t) = chrono::DateTime::from_timestamp_millis(ms) else {
        return String::new();
    };
    let t = t.with_timezone(&chrono::Local);
    if t.date_naive() == chrono::Local::now().date_naive() {
        format!("today {}", t.format("%H:%M"))
    } else {
        crate::text::danish_date(ms)
    }
}

/// A piece of a summary as the AI writes it: plain paragraphs, `**bold**`
/// lines as headings, and `-` or `*` bullets.
#[derive(Debug, Clone, PartialEq)]
enum Block {
    Paragraph(String),
    Heading(String),
    Bullet(String),
}

fn summary_blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(para.join("\n")));
            para.clear();
        }
    };
    for line in text.lines().map(str::trim_end) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            flush(&mut para, &mut out);
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            flush(&mut para, &mut out);
            out.push(Block::Bullet(item.trim().into()));
        } else if let Some(h) = trimmed
            .strip_prefix("**")
            .and_then(|t| t.strip_suffix("**"))
            .filter(|h| !h.is_empty() && !h.contains("**"))
        {
            flush(&mut para, &mut out);
            out.push(Block::Heading(h.trim().into()));
        } else {
            para.push(line);
        }
    }
    flush(&mut para, &mut out);
    out
}

/// Pango markup for a line: `**bold**` pairs become bold, the rest is escaped.
fn inline_markup(text: &str) -> String {
    let parts: Vec<&str> = text.split("**").collect();
    if parts.len() < 3 {
        return glib::markup_escape_text(text).to_string();
    }
    let mut out = String::new();
    let pairs = (parts.len() - 1) / 2 * 2;
    for (i, part) in parts.iter().enumerate() {
        let escaped = glib::markup_escape_text(part);
        if i > 0 && i <= pairs {
            out.push_str(if i % 2 == 1 { "<b>" } else { "</b>" });
        } else if i > pairs {
            out.push_str("**");
        }
        out.push_str(&escaped);
    }
    out
}

/// "Claude (claude-opus-5-5) · cloud · prompt «Kort resumé» · today 10:52".
fn meta_line(
    provider: &str,
    model: &str,
    place: Option<&str>,
    prompt: Option<&str>,
    at_ms: Option<i64>,
) -> String {
    let mut parts = vec![if model.is_empty() {
        provider.to_string()
    } else {
        format!("{provider} ({model})")
    }];
    parts.extend(place.map(str::to_string));
    if let Some(name) = prompt.and_then(prompt_name) {
        parts.push(format!("prompt «{name}»"));
    }
    parts.extend(at_ms.map(when));
    parts.join(" · ")
}

/// A wrapping, selectable label showing `text` with its `**bold**` parts.
fn markup_label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = label("", classes);
    l.set_markup(&inline_markup(text));
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_xalign(0.0);
    l.set_selectable(true);
    l
}

/// What the summary prompts are called where people see them.
pub(crate) fn prompt_name(id: &str) -> Option<&'static str> {
    (id == "summary").then_some("Kort resumé")
}

/// Where a provider (by name) runs: "cloud", "network", "this computer".
type PlaceOf = Box<dyn Fn(&str) -> Option<String>>;

/// Where an action item came from, and what clicking it does.
pub type SourceFn = Rc<dyn Fn(&ActionItem) -> Option<(String, Rc<dyn Fn()>)>>;

const WHO_WIDTH: i32 = 140;
const DUE_WIDTH: i32 = 110;

pub struct ActionsPanel {
    pub root: gtk::Box,
    pub run: gtk::Button,
    pub status: gtk::Label,
    summary: gtk::Label,
    badge: gtk::Label,
    /// A document's own list (it has a run button), not a project roll-up.
    per_document: bool,
    place_of: RefCell<Option<PlaceOf>>,
    table: gtk::Box,
    list: gtk::Box,
    source_title: String,
    source_width: i32,
    source: RefCell<Option<SourceFn>>,
    store: Rc<Store>,
    shown: RefCell<Vec<ActionItem>>,
}

impl ActionsPanel {
    /// `run_label`: the button that asks the AI (none for a roll-up);
    /// `source_title`: the last column's heading ("Source", "From").
    pub fn new(store: Rc<Store>, run_label: Option<&str>, source_title: &str, source_width: i32) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let badge = label("AI-GENERATED", &["fx-badge", "ai"]);
        badge.set_valign(gtk::Align::Center);
        badge.set_visible(false);
        top.append(&badge);
        let summary = label("", &["fx-stats"]);
        summary.set_hexpand(true);
        summary.set_xalign(0.0);
        let run = gtk::Button::with_label(run_label.unwrap_or(""));
        run.add_css_class("fx-secondary");
        run.set_visible(run_label.is_some());
        top.append(&summary);
        top.append(&run);

        let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.add_css_class("fx-table");
        table.set_overflow(gtk::Overflow::Hidden);
        let head = row_box(&["fx-table-head"]);
        head.append(&cell(gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast(), 28));
        let what = label("WHAT", &[]);
        what.set_hexpand(true);
        head.append(&what);
        head.append(&cell(label("WHO", &[]).upcast(), WHO_WIDTH));
        head.append(&cell(label("DUE", &[]).upcast(), DUE_WIDTH));
        head.append(&cell(
            label(&source_title.to_uppercase(), &[]).upcast(),
            source_width,
        ));
        table.append(&head);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.append(&list);

        let status = label("", &["fx-field-note"]);
        status.set_wrap(true);
        root.append(&top);
        root.append(&table);
        root.append(&status);
        let panel = Rc::new(Self {
            root,
            run,
            status,
            summary,
            badge,
            per_document: run_label.is_some(),
            place_of: RefCell::default(),
            table,
            list,
            source_title: source_title.to_string(),
            source_width,
            source: RefCell::default(),
            store,
            shown: RefCell::default(),
        });
        panel.show(Vec::new());
        panel
    }

    /// Tells the panel where a provider (by name) runs: "cloud", "network"…
    pub fn set_place_of(&self, f: impl Fn(&str) -> Option<String> + 'static) {
        *self.place_of.borrow_mut() = Some(Box::new(f));
    }

    /// The line above the table (tests).
    pub fn summary_text(&self) -> String {
        self.summary.text().to_string()
    }

    pub fn badge_visible(&self) -> bool {
        self.badge.get_visible()
    }

    pub fn set_source(&self, f: SourceFn) {
        *self.source.borrow_mut() = Some(f);
    }

    pub fn begin(&self) {
        self.run.set_sensitive(false);
        self.status.set_text("Looking for action items…");
    }

    pub fn finish(self: &Rc<Self>, result: Result<Vec<ActionItem>, AiError>) {
        self.run.set_sensitive(true);
        match result {
            Ok(items) => {
                self.status.set_text("");
                self.show(items);
            }
            Err(e) => self.status.set_text(&super::ai::error_text(&e)),
        }
    }

    pub fn show(self: &Rc<Self>, items: Vec<ActionItem>) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let open = items.iter().filter(|i| !i.done).count();
        let done = items.len() - open;
        self.summary.set_text(&if items.is_empty() {
            "No action items yet.".to_string()
        } else if self.per_document {
            // "3 action items · Claude · cloud · today 10:53", as in the mockup.
            let mut parts = vec![plural(items.len(), "action item", "action items")];
            if let Some(name) = items.iter().find_map(|i| i.provider.clone()) {
                let place = self.place_of.borrow().as_ref().and_then(|f| f(&name));
                parts.push(name);
                parts.extend(place);
            }
            parts.extend(items.iter().map(|i| i.created_at).max().map(when));
            parts.join(" · ")
        } else {
            format!("{open} open, {done} done · collected from every document in the project")
        });
        self.badge.set_visible(self.per_document && !items.is_empty());
        // The AI menu runs it again; the button is for the empty state.
        self.run.set_visible(self.per_document && items.is_empty());
        self.table.set_visible(!items.is_empty());
        let source = self.source.borrow().clone();
        for item in &items {
            let row = row_box(&["fx-table-row"]);
            let check = gtk::CheckButton::new();
            check.set_active(item.done);
            check.update_property(&[gtk::accessible::Property::Label("Done")]);
            row.append(&cell(check.clone().upcast(), 28));
            let what = label(&item.what, &[]);
            what.set_wrap(true);
            what.set_hexpand(true);
            if item.done {
                what.add_css_class("fx-done");
            }
            row.append(&what);
            let dash = |v: Option<&str>| {
                let l = label(v.unwrap_or("–"), if v.is_some() { &[] } else { &["fx-stats"] });
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                l
            };
            row.append(&cell(dash(item.who.as_deref()).upcast(), WHO_WIDTH));
            let due = item.due.as_deref().map(due_text);
            row.append(&cell(dash(due.as_deref()).upcast(), DUE_WIDTH));
            let src: gtk::Widget = match source.as_ref().and_then(|f| f(item)) {
                Some((text, go)) => {
                    let b = gtk::Button::with_label(&text);
                    b.add_css_class("fx-link");
                    b.set_halign(gtk::Align::Start);
                    if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                        l.add_css_class("fx-mono");
                    }
                    b.connect_clicked(move |_| go());
                    b.upcast()
                }
                None => dash(None).upcast(),
            };
            row.append(&cell(src, self.source_width));
            let id = item.id;
            let weak = Rc::downgrade(self);
            let what_label = what.clone();
            check.connect_toggled(move |c| {
                if c.is_active() {
                    what_label.add_css_class("fx-done");
                } else {
                    what_label.remove_css_class("fx-done");
                }
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

    /// The last column's heading (tests).
    pub fn source_title(&self) -> &str {
        &self.source_title
    }
}

fn row_box(classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

fn cell(w: gtk::Widget, width: i32) -> gtk::Widget {
    w.set_size_request(width, -1);
    w.set_hexpand(false);
    w.set_valign(gtk::Align::Center);
    if let Some(l) = w.downcast_ref::<gtk::Label>() {
        l.set_xalign(0.0);
    }
    w
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "9. oktober 2026" for an ISO date; anything else unchanged.
fn due_text(iso: &str) -> String {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map(crate::text::danish_day)
        .unwrap_or_else(|_| iso.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_render_paragraphs_headings_and_bullets() {
        let text = "Første afsnit\nfortsætter her.\n\n**Opfølgning**\n- Undersøg brønden.\n* Følg revnerne.";
        assert_eq!(
            summary_blocks(text),
            vec![
                Block::Paragraph("Første afsnit\nfortsætter her.".into()),
                Block::Heading("Opfølgning".into()),
                Block::Bullet("Undersøg brønden.".into()),
                Block::Bullet("Følg revnerne.".into()),
            ]
        );
    }

    #[test]
    fn inline_bold_becomes_markup_and_the_rest_is_escaped() {
        assert_eq!(inline_markup("a **b** <c> & d"), "a <b>b</b> &lt;c&gt; &amp; d");
        assert_eq!(inline_markup("one ** left"), "one ** left");
    }

    #[test]
    fn the_meta_line_names_provider_place_prompt_and_time() {
        assert_eq!(
            meta_line("Claude", "claude-opus-5-5", Some("cloud"), Some("summary"), None),
            "Claude (claude-opus-5-5) · cloud · prompt «Kort resumé»"
        );
        assert_eq!(meta_line("ollama", "", None, None, None), "ollama");
    }
}
