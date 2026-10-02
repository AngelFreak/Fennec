//! The Project view: documents in a project (or with a tag), filtered by tag
//! and text, with the project's settings and a combined export. With AI on,
//! tabs add the project's action items and questions over its documents
//! (including a project summary).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;

use super::ai_panels::{ActionsPanel, SummaryPanel};
use super::{Deps, Handler, label};
use crate::ai::Locality;
use crate::ai::actions::{Answer, Citation};
use crate::ai::service::Scope as AiScope;
use crate::store::{DocumentFilter, DocumentId, DocumentSummary, ProjectFilter, Source, Store};
use crate::template::load_dir;
use crate::text::{danish_date, duration};

pub const COLORS: [&str; 5] = ["#C2410C", "#1D4ED8", "#0F766E", "#6B21A8", "#9AA1AE"];
const COLOR_NAMES: [&str; 5] = ["Orange", "Blue", "Teal", "Purple", "Grey"];
/// The colour square for "All documents" and "Unsorted" (as in the sidebar).
const NEUTRAL: &str = "#9AA1AE";

/// Table column widths (the document column takes the rest).
const TAGS_WIDTH: i32 = 168;
const DATE_WIDTH: i32 = 112;
const LENGTH_WIDTH: i32 = 64;

const SUGGESTIONS: [&str; 2] = ["What changed since last week?", "Open questions"];

#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    Project(ProjectFilter),
    Tag(String),
}

pub struct ProjectPage {
    pub root: gtk::Box,
    /// Buttons for the window header while this screen shows.
    pub header_actions: gtk::Box,
    store: Rc<Store>,
    deps: Deps,
    /// Documents / Actions / Ask.
    pub tabs: gtk::Stack,
    tab_buttons: Vec<(&'static str, gtk::Button)>,
    pub question: gtk::Entry,
    pub ask_button: gtk::Button,
    pub answer: gtk::Label,
    pub ask_status: gtk::Label,
    bubble: gtk::Label,
    answer_card: gtk::Box,
    answer_meta: gtk::Label,
    local_note: gtk::Label,
    citations: gtk::FlowBox,
    pub summary: Rc<SummaryPanel>,
    pub actions: Rc<ActionsPanel>,
    actions_note: gtk::Label,
    templates_dir: std::path::PathBuf,
    template_names: RefCell<HashMap<String, String>>,
    scope: RefCell<Scope>,
    title: gtk::Label,
    color: Rc<RefCell<Option<String>>>,
    color_square: gtk::DrawingArea,
    meta: gtk::Label,
    local_badge: gtk::Box,
    search: gtk::SearchEntry,
    chips: gtk::Box,
    tag_filter: RefCell<Option<String>>,
    count: gtk::Label,
    list: gtk::Box,
    table: gtk::Box,
    empty: gtk::Label,
    shown: RefCell<Vec<DocumentId>>,
    in_scope: RefCell<Vec<DocumentId>>,
    side: gtk::ScrolledWindow,
    project_fields: gtk::Box,
    name: gtk::Entry,
    swatches: Vec<(&'static str, gtk::Button)>,
    local_only: gtk::Switch,
    default_template: gtk::DropDown,
    template_ids: RefCell<Vec<String>>,
    only_shown: gtk::CheckButton,
    toc: gtk::CheckButton,
    export_button: gtk::Button,
    export_label: gtk::Label,
    loading: Cell<bool>,
    on_open: Handler<DocumentId>,
    on_export: Handler<(String, Vec<DocumentId>)>,
    on_changed: Handler<()>,
    on_new_dictation: Handler<Option<i64>>,
    on_import: Handler<()>,
    new_dictation: gtk::Button,
}

impl ProjectPage {
    pub fn new(store: Rc<Store>, deps: Deps) -> Rc<Self> {
        let templates_dir = deps.paths.templates();

        // Title, meta line and the local-only badge.
        let color: Rc<RefCell<Option<String>>> = Rc::default();
        let color_square = color_area(14, 4.0, {
            let color = Rc::clone(&color);
            move || color.borrow().clone()
        });
        color_square.set_valign(gtk::Align::Center);
        let title = label("", &["fx-project-title"]);
        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        title_row.append(&color_square);
        title_row.append(&title);
        let meta = label("", &["fx-status"]);
        let local_badge = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        local_badge.add_css_class("fx-local-badge");
        local_badge.set_valign(gtk::Align::Center);
        let lock = gtk::Image::from_icon_name("fennec-lock-symbolic");
        lock.set_pixel_size(12);
        local_badge.append(&lock);
        local_badge.append(&gtk::Label::new(Some("LOCAL ONLY")));
        let meta_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        meta_row.append(&meta);
        meta_row.append(&local_badge);
        let head = gtk::Box::new(gtk::Orientation::Vertical, 8);
        head.append(&title_row);
        head.append(&meta_row);

        // Tabs.
        let tab_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tab_bar.add_css_class("fx-tabs");
        tab_bar.add_css_class("flush");
        let mut tab_buttons = Vec::new();
        for (name, text) in [("documents", "Documents"), ("actions", "Actions"), ("ask", "Ask")] {
            let b = gtk::Button::with_label(text);
            b.add_css_class("fx-tab");
            tab_bar.append(&b);
            tab_buttons.push((name, b));
        }

        // Documents: search, tag chips, the table.
        let filters = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search text in this project")
            .width_request(260)
            .build();
        search.update_property(&[gtk::accessible::Property::Label("Search in project")]);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let count = label("", &["fx-stats"]);
        count.set_hexpand(true);
        count.set_xalign(1.0);
        filters.append(&search);
        filters.append(&chips);
        filters.append(&count);

        let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.add_css_class("fx-table");
        table.set_overflow(gtk::Overflow::Hidden);
        let head_row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        head_row.add_css_class("fx-table-head");
        head_row.append(&sized(gtk::Box::new(gtk::Orientation::Horizontal, 0), 28));
        let doc_head = label("DOCUMENT", &[]);
        doc_head.set_hexpand(true);
        head_row.append(&doc_head);
        head_row.append(&sized(label("TAGS", &[]), TAGS_WIDTH));
        head_row.append(&sized(label("DATE", &[]), DATE_WIDTH));
        head_row.append(&sized(label("LENGTH", &[]), LENGTH_WIDTH));
        table.append(&head_row);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.append(&list);
        let empty = label("", &["fx-body"]);
        empty.set_wrap(true);

        let documents = gtk::Box::new(gtk::Orientation::Vertical, 16);
        documents.append(&filters);
        documents.append(&table);
        documents.append(&empty);

        // Actions: the roll-up of every document's action items.
        let actions = ActionsPanel::new(Rc::clone(&store), None, "From", 200);
        // The panel's own count line is replaced by one that says where the
        // items come from.
        if let Some(top) = actions.root.first_child() {
            top.set_visible(false);
        }
        let actions_note = label("", &["fx-stats"]);
        let actions_view = gtk::Box::new(gtk::Orientation::Vertical, 10);
        actions_view.append(&actions_note);
        actions_view.append(&actions.root);

        // Ask: suggestions, the summary card, the question and its answer.
        let ask = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let suggestions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let summarize_chip = gtk::Button::with_label("Summarise project");
        summarize_chip.add_css_class("fx-chip-filter");
        suggestions.append(&summarize_chip);
        let mut suggestion_chips = Vec::new();
        for s in SUGGESTIONS {
            let b = gtk::Button::with_label(s);
            b.add_css_class("fx-chip-filter");
            suggestions.append(&b);
            suggestion_chips.push((s, b));
        }
        let summary = SummaryPanel::new(Rc::clone(&store), "Summarise project");
        summary.root.set_visible(false);
        let bubble = label("", &["fx-bubble"]);
        bubble.set_halign(gtk::Align::End);
        bubble.set_wrap(true);
        bubble.set_max_width_chars(60);
        bubble.set_visible(false);
        let answer_card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        answer_card.add_css_class("fx-answer-card");
        answer_card.set_visible(false);
        let badge_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let badge = label("AI-GENERATED", &["fx-badge", "ai"]);
        badge.set_valign(gtk::Align::Center);
        let answer_meta = label("", &["fx-stats"]);
        answer_meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
        badge_row.append(&badge);
        badge_row.append(&answer_meta);
        let answer = label("", &["fx-answer"]);
        answer.set_wrap(true);
        answer.set_selectable(true);
        answer.set_valign(gtk::Align::Start);
        let citations = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(6)
            .row_spacing(6)
            .max_children_per_line(8)
            .homogeneous(false)
            .build();
        citations.add_css_class("fx-citations");
        answer_card.append(&badge_row);
        answer_card.append(&answer);
        answer_card.append(&citations);
        let ask_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let question = gtk::Entry::builder()
            .placeholder_text("Ask about this project…")
            .hexpand(true)
            .css_classes(["fx-ask-entry"])
            .build();
        question.update_property(&[gtk::accessible::Property::Label(
            "Ask a question about this project",
        )]);
        let ask_button = gtk::Button::with_label("Ask");
        ask_button.add_css_class("fx-primary");
        ask_button.add_css_class("fx-ask-button");
        ask_row.append(&question);
        ask_row.append(&ask_button);
        let ask_status = label(
            "Answers come only from these documents and point to the paragraphs they use.",
            &["fx-field-note"],
        );
        ask_status.set_wrap(true);
        let local_note = label(
            "This project is local only, so cloud providers are not offered.",
            &["fx-field-note"],
        );
        local_note.set_wrap(true);
        ask.append(&suggestions);
        ask.append(&summary.root);
        ask.append(&bubble);
        ask.append(&answer_card);
        ask.append(&ask_row);
        ask.append(&ask_status);
        ask.append(&local_note);

        let tabs = gtk::Stack::new();
        tabs.set_vhomogeneous(false);
        tabs.add_named(&documents, Some("documents"));
        tabs.add_named(&actions_view, Some("actions"));
        tabs.add_named(&ask, Some("ask"));

        let main = gtk::Box::new(gtk::Orientation::Vertical, 18);
        main.add_css_class("fx-project-main");
        main.append(&head);
        main.append(&tab_bar);
        main.append(&tabs);
        let main_scroll = gtk::ScrolledWindow::builder()
            .child(&main)
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        // The right panel: project settings and the combined export.
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 18);
        panel.add_css_class("fx-inspector");
        let project_fields = gtk::Box::new(gtk::Orientation::Vertical, 18);
        project_fields.append(&label("PROJECT", &["fx-section-title"]));
        let name = gtk::Entry::new();
        name.add_css_class("fx-field");
        name.update_property(&[gtk::accessible::Property::Label("Project name")]);
        project_fields.append(&field("Name", &name, 6));
        let swatch_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let mut swatches = Vec::new();
        for (color, color_name) in COLORS.into_iter().zip(COLOR_NAMES) {
            let b = gtk::Button::new();
            b.add_css_class("fx-color-swatch");
            b.set_tooltip_text(Some(color_name));
            b.update_property(&[gtk::accessible::Property::Label(color_name)]);
            b.set_child(Some(&color_area(32, 8.0, move || Some(color.to_string()))));
            swatch_row.append(&b);
            swatches.push((color, b));
        }
        project_fields.append(&field("Color", &swatch_row, 8));
        let default_template = gtk::DropDown::from_strings(&[]);
        project_fields.append(&field("Default template for new documents", &default_template, 6));
        let local_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let local_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        local_text.set_hexpand(true);
        local_text.append(&label("Local only", &["fx-field-label"]));
        let local_hint = label("Never send these documents to a cloud AI.", &["fx-field-note"]);
        local_hint.set_wrap(true);
        local_text.append(&local_hint);
        let local_only = gtk::Switch::new();
        local_only.set_valign(gtk::Align::Center);
        local_only.update_property(&[gtk::accessible::Property::Label("Local only")]);
        local_row.append(&local_text);
        local_row.append(&local_only);
        project_fields.append(&local_row);
        project_fields.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        panel.append(&project_fields);

        let export = gtk::Box::new(gtk::Orientation::Vertical, 10);
        export.add_css_class("fx-project-export");
        export.append(&label("EXPORT PROJECT", &["fx-section-title"]));
        let export_note = label(
            "Combine the documents into one report, oldest first, with a table of contents.",
            &["fx-body", "fx-export-note"],
        );
        export_note.set_wrap(true);
        export.append(&export_note);
        let only_shown = gtk::CheckButton::with_label("Only the documents shown");
        only_shown.set_active(true);
        let toc = gtk::CheckButton::with_label("Table of contents");
        toc.set_active(true);
        export.append(&only_shown);
        export.append(&toc);
        let export_button = gtk::Button::new();
        export_button.add_css_class("fx-ink");
        export_button.add_css_class("large");
        let export_inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        export_inner.set_halign(gtk::Align::Center);
        export_inner.append(&gtk::Image::from_icon_name("fennec-download-symbolic"));
        let export_label = gtk::Label::new(Some("Export documents…"));
        export_inner.append(&export_label);
        export_button.set_child(Some(&export_inner));
        export.append(&export_button);
        panel.append(&export);

        let side = gtk::ScrolledWindow::builder()
            .child(&panel)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(300)
            .hexpand(false)
            .build();
        side.add_css_class("fx-inspector-scroll");

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&main_scroll);
        root.append(&side);

        let header_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let import = super::icon_text_button("fennec-upload-symbolic", "Import file", &["fx-secondary"]);
        let new_dictation = super::icon_text_button("fennec-mic-symbolic", "New dictation", &["fx-primary"]);
        header_actions.append(&import);
        header_actions.append(&new_dictation);

        let page = Rc::new(Self {
            root,
            header_actions,
            store,
            deps,
            tabs,
            tab_buttons,
            question,
            ask_button,
            answer,
            ask_status,
            bubble,
            answer_card,
            answer_meta,
            local_note,
            citations,
            summary,
            actions,
            actions_note,
            templates_dir,
            template_names: RefCell::default(),
            scope: RefCell::new(Scope::Project(ProjectFilter::All)),
            title,
            color,
            color_square,
            meta,
            local_badge,
            search,
            chips,
            tag_filter: RefCell::default(),
            count,
            list,
            table,
            empty,
            shown: RefCell::default(),
            in_scope: RefCell::default(),
            side,
            project_fields,
            name,
            swatches,
            local_only,
            default_template,
            template_ids: RefCell::default(),
            only_shown,
            toc,
            export_button,
            export_label,
            loading: Cell::new(false),
            on_open: RefCell::default(),
            on_export: RefCell::default(),
            on_changed: RefCell::default(),
            on_new_dictation: RefCell::default(),
            on_import: RefCell::default(),
            new_dictation: new_dictation.clone(),
        });
        let weak = Rc::downgrade(&page);
        summarize_chip.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.summarize();
            }
        });
        for (text, b) in suggestion_chips {
            let weak = Rc::downgrade(&page);
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.question.set_text(text);
                    p.ask();
                }
            });
        }
        page.wire();
        let weak = Rc::downgrade(&page);
        new_dictation.connect_clicked(move |_| {
            let Some(p) = weak.upgrade() else { return };
            let f = p.on_new_dictation.borrow().clone();
            if let Some(f) = f {
                f(p.project_id());
            }
        });
        let weak = Rc::downgrade(&page);
        import.connect_clicked(move |_| {
            let Some(p) = weak.upgrade() else { return };
            let f = p.on_import.borrow().clone();
            if let Some(f) = f {
                f(());
            }
        });
        page.show_tab("documents");
        page.refresh_ai();
        page
    }

    pub fn title_text(&self) -> String {
        self.title.text().to_string()
    }

    /// Shows the AI tabs only when AI is on.
    pub fn refresh_ai(&self) {
        let on = self.deps.settings().ai.enabled;
        for (name, b) in &self.tab_buttons {
            if *name != "documents" {
                b.set_visible(on);
            }
        }
        if !on {
            self.show_tab("documents");
        }
    }

    /// Switches the view and marks its tab.
    pub fn show_tab(&self, name: &str) {
        self.tabs.set_visible_child_name(name);
        for (n, b) in &self.tab_buttons {
            if *n == name {
                b.add_css_class("active");
            } else {
                b.remove_css_class("active");
            }
        }
    }

    fn set_tab_label(&self, name: &str, text: &str) {
        if let Some((_, b)) = self.tab_buttons.iter().find(|(n, _)| *n == name) {
            b.set_label(text);
        }
    }

    fn ai_scope(&self) -> AiScope {
        match &*self.scope.borrow() {
            Scope::Project(f) => AiScope::Project(*f),
            Scope::Tag(t) => AiScope::Tag(t.clone()),
        }
    }

    fn what(&self) -> String {
        format!("the documents in “{}”", self.title.text())
    }

    /// A callback that only runs if the view still shows `scope`.
    fn for_scope<T: 'static>(self: &Rc<Self>, f: impl Fn(&Rc<Self>, T) + 'static) -> Rc<dyn Fn(T)> {
        let weak = Rc::downgrade(self);
        let scope = self.scope.borrow().clone();
        Rc::new(move |v| {
            if let Some(p) = weak.upgrade()
                && *p.scope.borrow() == scope
            {
                f(&p, v);
            }
        })
    }

    /// "Workstation · network" for the active provider.
    fn provider_text(&self) -> String {
        match self.deps.settings().ai.active() {
            Some(p) => format!("{} · {}", p.name, locality_word(p.locality)),
            None => "No provider set up".into(),
        }
    }

    fn clear_answer(&self) {
        self.answer.set_text("");
        while let Some(c) = self.citations.first_child() {
            self.citations.remove(&c);
        }
    }

    pub fn ask(self: &Rc<Self>) {
        let question = self.question.text().trim().to_string();
        if question.is_empty() {
            return;
        }
        self.show_tab("ask");
        self.clear_answer();
        self.bubble.set_text(&question);
        self.bubble.set_visible(true);
        self.answer_card.set_visible(true);
        self.answer_meta.set_text(&format!(
            "{} · searched {}",
            self.provider_text(),
            plural(self.in_scope.borrow().len(), "document")
        ));
        self.question.set_text("");
        self.ask_status.set_text("Reading the documents…");
        self.ask_button.set_sensitive(false);
        let scope = self.ai_scope();
        let job_scope = scope.clone();
        let weak = Rc::downgrade(self);
        super::ai::run(
            self.root.upcast_ref(),
            &self.deps,
            scope,
            self.what(),
            Arc::new(move |ai, store, cancel, delta| ai.ask(store, &job_scope, &question, cancel, delta)),
            Rc::new(move |d: &str| {
                if let Some(p) = weak.upgrade() {
                    p.answer.set_text(&format!("{}{d}", p.answer.text()));
                }
            }),
            self.for_scope(|p, r: Result<Answer, crate::ai::AiError>| {
                p.ask_button.set_sensitive(true);
                match r {
                    Ok(a) => p.show_answer(&a),
                    Err(e) => {
                        p.answer.set_text("");
                        p.answer_card.set_visible(false);
                        p.ask_status.set_text(&super::ai::error_text(&e));
                    }
                }
            }),
        );
    }

    fn show_answer(self: &Rc<Self>, a: &Answer) {
        let accent = if adw::StyleManager::default().is_dark() {
            "#F59A6B"
        } else {
            "#C2410C"
        };
        self.answer
            .set_markup(&numbered_markup(&a.text, &a.citations, accent));
        self.ask_status.set_text(match a.citations.len() {
            0 => "No sources were cited; check the answer against the documents.",
            _ => "Answers come only from these documents. Open a source to check it.",
        });
        for (i, c) in a.citations.iter().enumerate() {
            let Ok(doc) = self.store.document(c.document_id) else {
                continue;
            };
            let paragraphs = self.store.paragraphs(c.document_id).unwrap_or_default();
            let at = paragraphs
                .iter()
                .enumerate()
                .find(|(_, p)| p.id == Some(c.paragraph_id));
            let place = match at {
                Some((_, p)) if p.start_ms.is_some() => duration(p.start_ms.unwrap_or(0)),
                Some((n, _)) => format!("¶{}", n + 1),
                None => String::new(),
            };
            let text = if place.is_empty() {
                doc.title.clone()
            } else {
                format!("{} · {place}", doc.title)
            };
            let b = gtk::Button::new();
            b.add_css_class("fx-citation");
            b.set_halign(gtk::Align::Start);
            b.set_tooltip_text(Some("Open the document"));
            let inner = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            inner.append(&label(&(i + 1).to_string(), &["num"]));
            let l = label(&text, &[]);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            l.set_max_width_chars(40);
            inner.append(&l);
            b.set_child(Some(&inner));
            let weak = Rc::downgrade(self);
            let id = c.document_id;
            b.connect_clicked(move |_| {
                if let Some(f) = weak.upgrade().and_then(|p| p.on_open.borrow().clone()) {
                    f(id);
                }
            });
            self.citations.append(&b);
        }
    }

    /// Cited sources as shown (tests).
    pub fn citation_labels(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut child = self.citations.first_child();
        while let Some(c) = child {
            out.push(super::texts_in(&c).join(" "));
            child = c.next_sibling();
        }
        out
    }

    /// Writes a summary of the project's documents into the card in Ask.
    pub fn summarize(self: &Rc<Self>) {
        self.show_tab("ask");
        self.summary.root.set_visible(true);
        self.summary.begin();
        let scope = self.ai_scope();
        let job_scope = scope.clone();
        let weak = Rc::downgrade(self);
        super::ai::run(
            self.root.upcast_ref(),
            &self.deps,
            scope,
            self.what(),
            Arc::new(move |ai, store, cancel, delta| ai.summarize(store, &job_scope, cancel, delta)),
            Rc::new(move |d: &str| {
                if let Some(p) = weak.upgrade() {
                    p.summary.append(d);
                }
            }),
            self.for_scope(|p, r| p.summary.finish(r)),
        );
    }

    fn load_ai_results(self: &Rc<Self>) {
        let latest = self
            .project_id()
            .and_then(|id| self.store.summaries_for_project(id).ok())
            .and_then(|s| s.into_iter().next());
        self.summary.root.set_visible(latest.is_some());
        self.summary.load(latest);
        let ids = self.in_scope.borrow().clone();
        let items: Vec<_> = self
            .store
            .action_items(ProjectFilter::All)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| ids.contains(&a.document_id))
            .collect();
        let open = items.iter().filter(|i| !i.done).count();
        let done = items.len() - open;
        self.actions_note.set_text(&if items.is_empty() {
            "No action items yet. Find them in each document's Actions tab.".to_string()
        } else {
            format!("{open} open, {done} done · collected from every document in the project")
        });
        self.set_tab_label("actions", &format!("Actions · {open}"));
        self.actions.show(items);
        self.clear_answer();
        self.bubble.set_visible(false);
        self.answer_card.set_visible(false);
    }

    fn wire(self: &Rc<Self>) {
        for (name, b) in &self.tab_buttons {
            let weak = Rc::downgrade(self);
            let name = *name;
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.show_tab(name);
                }
            });
        }
        for (color, b) in &self.swatches {
            let weak = Rc::downgrade(self);
            let color = *color;
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.save_project(Some(color));
                }
            });
        }
        // Action items link to the place in the document they came from.
        let weak = Rc::downgrade(self);
        self.actions.set_source(Rc::new(move |item| {
            let p = weak.upgrade()?;
            let doc = p.store.document(item.document_id).ok()?;
            let start = item.paragraph_id.and_then(|id| {
                p.store
                    .paragraphs(item.document_id)
                    .ok()?
                    .into_iter()
                    .find(|x| x.id == Some(id))?
                    .start_ms
            });
            let text = match start {
                Some(ms) => format!("{} · {}", doc.title, duration(ms)),
                None => doc.title,
            };
            let weak = Rc::downgrade(&p);
            let id = item.document_id;
            let go: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(f) = weak.upgrade().and_then(|p| p.on_open.borrow().clone()) {
                    f(id);
                }
            });
            Some((text, go))
        }));
        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.render_list();
            }
        });
        let weak = Rc::downgrade(self);
        self.ask_button.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.ask();
            }
        });
        let weak = Rc::downgrade(self);
        self.question.connect_activate(move |_| {
            if let Some(p) = weak.upgrade() {
                p.ask();
            }
        });
        let weak = Rc::downgrade(self);
        self.summary.run.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.summarize();
            }
        });
        let weak = Rc::downgrade(self);
        self.name.connect_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.save_project(None);
            }
        });
        let weak = Rc::downgrade(self);
        self.default_template.connect_selected_notify(move |_| {
            if let Some(p) = weak.upgrade() {
                p.save_project(None);
            }
        });
        let weak = Rc::downgrade(self);
        self.local_only.connect_active_notify(move |s| {
            let Some(p) = weak.upgrade() else { return };
            if p.loading.get() {
                return;
            }
            if let Some(id) = p.project_id() {
                if let Err(e) = p.store.set_project_local_only(id, s.is_active()) {
                    tracing::error!("saving local-only for project {id}: {e}");
                }
                p.local_badge.set_visible(s.is_active());
                p.local_note.set_visible(s.is_active());
                if let Some(f) = p.on_changed.borrow().clone() {
                    f(());
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.only_shown.connect_toggled(move |_| {
            if let Some(p) = weak.upgrade() {
                p.update_export_button();
            }
        });
        let weak = Rc::downgrade(self);
        self.export_button.connect_clicked(move |_| {
            let Some(p) = weak.upgrade() else { return };
            let ids = p.export_ids();
            if let Some(f) = p.on_export.borrow().clone() {
                f((p.title.text().to_string(), ids));
            }
        });
    }

    pub fn connect_open(&self, f: impl Fn(DocumentId) + 'static) {
        *self.on_open.borrow_mut() = Some(Rc::new(f));
    }
    pub fn connect_export(&self, f: impl Fn((String, Vec<DocumentId>)) + 'static) {
        *self.on_export.borrow_mut() = Some(Rc::new(f));
    }
    /// Project name, color or settings changed (the sidebar redraws).
    pub fn connect_changed(&self, f: impl Fn(()) + 'static) {
        *self.on_changed.borrow_mut() = Some(Rc::new(f));
    }

    /// "New dictation": the handler gets the project shown, if any.
    pub fn connect_new_dictation(&self, f: impl Fn(Option<i64>) + 'static) {
        *self.on_new_dictation.borrow_mut() = Some(Rc::new(f));
    }

    /// "Import file".
    pub fn connect_import(&self, f: impl Fn(()) + 'static) {
        *self.on_import.borrow_mut() = Some(Rc::new(f));
    }

    pub fn press_new_dictation(&self) {
        self.new_dictation.emit_clicked();
    }

    fn project_id(&self) -> Option<i64> {
        match &*self.scope.borrow() {
            Scope::Project(ProjectFilter::Project(id)) => Some(*id),
            _ => None,
        }
    }

    pub fn show(self: &Rc<Self>, scope: Scope) {
        self.loading.set(true);
        *self.scope.borrow_mut() = scope.clone();
        *self.tag_filter.borrow_mut() = None;
        self.search.set_text("");
        let project = self
            .project_id()
            .and_then(|id| self.store.projects().ok()?.into_iter().find(|p| p.id == id));
        let title = match (&scope, &project) {
            (_, Some(p)) => p.name.clone(),
            (Scope::Tag(t), _) => format!("#{t}"),
            (Scope::Project(ProjectFilter::Unsorted), _) => "Unsorted".into(),
            _ => "All documents".into(),
        };
        self.title.set_text(&title);
        *self.color.borrow_mut() = match (&scope, &project) {
            (_, Some(p)) => Some(p.color.clone()),
            (Scope::Tag(_), _) => None,
            (Scope::Project(ProjectFilter::Unsorted), _) => Some("#C9CED6".into()),
            _ => Some(NEUTRAL.into()),
        };
        self.color_square.set_visible(self.color.borrow().is_some());
        self.color_square.queue_draw();
        self.mark_swatch(project.as_ref().map(|p| p.color.as_str()));
        self.side
            .set_visible(project.is_some() || matches!(scope, Scope::Tag(_)));
        self.project_fields.set_visible(project.is_some());
        let templates: Vec<_> = load_dir(&self.templates_dir)
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        *self.template_names.borrow_mut() =
            templates.iter().map(|t| (t.id.clone(), t.name.clone())).collect();
        if let Some(p) = &project {
            self.name.set_text(&p.name);
            self.local_only.set_active(p.local_only);
            let mut names = vec!["(app default)".to_string()];
            names.extend(templates.iter().map(|t| t.name.clone()));
            let mut ids = vec![String::new()];
            ids.extend(templates.iter().map(|t| t.id.clone()));
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            self.default_template
                .set_model(Some(&gtk::StringList::new(&refs)));
            let sel = p
                .default_template
                .as_ref()
                .and_then(|d| ids.iter().position(|i| i == d))
                .unwrap_or(0);
            self.default_template.set_selected(sel as u32);
            *self.template_ids.borrow_mut() = ids;
        }
        let local = project.as_ref().is_some_and(|p| p.local_only);
        self.local_badge.set_visible(local);
        self.local_note.set_visible(local);
        self.loading.set(false);
        self.render_list();
        self.load_ai_results();
    }

    fn mark_swatch(&self, color: Option<&str>) {
        for (c, b) in &self.swatches {
            if color.is_some_and(|x| x.eq_ignore_ascii_case(c)) {
                b.add_css_class("selected");
            } else {
                b.remove_css_class("selected");
            }
        }
    }

    fn save_project(&self, color: Option<&str>) {
        if self.loading.get() {
            return;
        }
        let Some(id) = self.project_id() else { return };
        let Some(p) = self
            .store
            .projects()
            .ok()
            .and_then(|ps| ps.into_iter().find(|p| p.id == id))
        else {
            return;
        };
        let name = self.name.text().trim().to_string();
        let name = if name.is_empty() { p.name.clone() } else { name };
        let template = self
            .template_ids
            .borrow()
            .get(self.default_template.selected() as usize)
            .cloned()
            .filter(|t| !t.is_empty());
        let color = color.unwrap_or(&p.color);
        if let Err(e) = self.store.update_project(id, &name, color, template.as_deref()) {
            tracing::error!("saving project {id}: {e}");
        }
        self.title.set_text(&name);
        *self.color.borrow_mut() = Some(color.to_string());
        self.color_square.queue_draw();
        self.mark_swatch(Some(color));
        if let Some(f) = self.on_changed.borrow().clone() {
            f(());
        }
    }

    fn base_filter(&self) -> DocumentFilter {
        match &*self.scope.borrow() {
            Scope::Project(f) => DocumentFilter {
                project: *f,
                ..Default::default()
            },
            Scope::Tag(t) => DocumentFilter {
                tag: Some(t.clone()),
                ..Default::default()
            },
        }
    }

    /// Re-reads documents and redraws the list and tag chips.
    pub fn render_list(self: &Rc<Self>) {
        let all = self.store.documents(&self.base_filter()).unwrap_or_default();
        let mut filter = self.base_filter();
        let text = self.search.text().trim().to_string();
        if !text.is_empty() {
            filter.text = Some(text);
        }
        if let Some(t) = self.tag_filter.borrow().clone() {
            filter.tag = Some(t);
        }
        let mut docs = self.store.documents(&filter).unwrap_or_default();
        docs.retain(|d| all.iter().any(|a| a.id == d.id));
        self.render_chips(&all);
        self.count
            .set_text(&format!("Showing {} of {}", docs.len(), all.len()));
        self.set_tab_label("documents", &format!("Documents · {}", all.len()));
        let total: i64 = all.iter().filter_map(|d| d.duration_ms).sum();
        let span = match (
            all.iter().map(|d| d.created_at).min(),
            all.iter().map(|d| d.created_at).max(),
        ) {
            (Some(a), Some(b)) if danish_date(a) != danish_date(b) => {
                format!(" · {} – {}", danish_date(a), danish_date(b))
            }
            (Some(a), _) => format!(" · {}", danish_date(a)),
            _ => String::new(),
        };
        self.meta.set_text(&format!(
            "{} · {} of audio{span}",
            plural(all.len(), "document"),
            duration(total)
        ));
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        for d in &docs {
            self.list.append(&self.row(d));
        }
        self.table.set_visible(!docs.is_empty());
        self.empty.set_visible(docs.is_empty());
        self.empty.set_text(if all.is_empty() {
            "No documents here yet. Dictate or import a file and choose this project for it."
        } else {
            "No documents match the search or tag."
        });
        *self.shown.borrow_mut() = docs.iter().map(|d| d.id).collect();
        *self.in_scope.borrow_mut() = all.iter().map(|d| d.id).collect();
        self.update_export_button();
    }

    /// The documents an export covers: those shown, or all in the project.
    fn export_ids(&self) -> Vec<DocumentId> {
        if self.only_shown.is_active() {
            self.shown.borrow().clone()
        } else {
            self.in_scope.borrow().clone()
        }
    }

    fn update_export_button(&self) {
        let n = self.export_ids().len();
        self.export_label
            .set_text(&format!("Export {}…", plural(n, "document")));
        self.export_button.set_sensitive(n > 0);
    }

    /// Whether the export should start with a table of contents.
    pub fn table_of_contents(&self) -> bool {
        self.toc.is_active()
    }

    pub fn set_table_of_contents(&self, on: bool) {
        self.toc.set_active(on);
    }

    fn render_chips(self: &Rc<Self>, docs: &[DocumentSummary]) {
        while let Some(c) = self.chips.first_child() {
            self.chips.remove(&c);
        }
        let mut tags: Vec<(String, usize)> = Vec::new();
        for t in docs.iter().flat_map(|d| d.tags.iter()) {
            match tags.iter_mut().find(|(n, _)| n == t) {
                Some((_, c)) => *c += 1,
                None => tags.push((t.clone(), 1)),
            }
        }
        if tags.is_empty() || matches!(&*self.scope.borrow(), Scope::Tag(_)) {
            return;
        }
        let active = self.tag_filter.borrow().clone();
        let all = gtk::ToggleButton::with_label("All");
        all.add_css_class("fx-chip-filter");
        all.set_active(active.is_none());
        let mut buttons = vec![(None, all.clone())];
        for (tag, n) in tags {
            let b = gtk::ToggleButton::with_label(&format!("#{tag} · {n}"));
            b.add_css_class("fx-chip-filter");
            b.set_group(Some(&all));
            b.set_active(active.as_deref() == Some(tag.as_str()));
            buttons.push((Some(tag), b));
        }
        for (tag, b) in buttons {
            b.set_valign(gtk::Align::Center);
            self.chips.append(&b);
            let weak = Rc::downgrade(self);
            b.connect_toggled(move |b| {
                if b.is_active()
                    && let Some(p) = weak.upgrade()
                {
                    *p.tag_filter.borrow_mut() = tag.clone();
                    // Rebuilding the chips from inside their own handler is deferred.
                    gtk::glib::idle_add_local_once(move || p.render_list());
                }
            });
        }
    }

    fn row(&self, d: &DocumentSummary) -> gtk::Widget {
        let b = gtk::Button::new();
        b.add_css_class("fx-row-button");
        b.add_css_class("fx-project-doc-row");
        b.set_tooltip_text(Some("Open the document"));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tile.add_css_class("fx-doc-icon");
        tile.set_valign(gtk::Align::Center);
        let (icon, kind) = match d.source {
            Source::Dictation => ("fennec-mic-symbolic", "Dictation"),
            Source::File => ("fennec-file-symbolic", "From file"),
        };
        let image = gtk::Image::from_icon_name(icon);
        image.set_pixel_size(15);
        image.set_hexpand(true);
        image.set_halign(gtk::Align::Center);
        image.update_property(&[gtk::accessible::Property::Label(kind)]);
        tile.append(&image);
        row.append(&sized(tile, 28));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let title = label(&d.title, &["fx-doc-row-title"]);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&title);
        let template = d.template_id.as_ref().map(|t| {
            self.template_names
                .borrow()
                .get(t)
                .cloned()
                .unwrap_or_else(|| t.clone())
        });
        let sub = match template {
            Some(t) => format!("{kind} · {t}"),
            None => kind.to_string(),
        };
        let sub = label(&sub, &["fx-stats"]);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&sub);
        row.append(&text);
        let tags = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for t in &d.tags {
            tags.append(&label(&format!("#{t}"), &["fx-tag", "small", "fx-tag-pill"]));
        }
        row.append(&sized(tags, TAGS_WIDTH));
        row.append(&sized(
            label(&danish_date(d.created_at), &["fx-doc-date"]),
            DATE_WIDTH,
        ));
        row.append(&sized(
            label(
                &d.duration_ms.map(duration).unwrap_or_else(|| "–".into()),
                &["fx-mono", "fx-doc-length"],
            ),
            LENGTH_WIDTH,
        ));
        b.set_child(Some(&row));
        let on_open = self.on_open.borrow().clone();
        let id = d.id;
        b.connect_clicked(move |_| {
            if let Some(f) = &on_open {
                f(id);
            }
        });
        b.upcast()
    }

    /// Titles of the documents shown, in order (tests).
    pub fn shown_titles(&self) -> Vec<String> {
        self.shown
            .borrow()
            .iter()
            .filter_map(|id| self.store.document(*id).ok().map(|d| d.title))
            .collect()
    }

    pub fn set_search(self: &Rc<Self>, text: &str) {
        self.search.set_text(text);
        self.render_list();
    }

    pub fn set_tag_filter(self: &Rc<Self>, tag: Option<&str>) {
        *self.tag_filter.borrow_mut() = tag.map(str::to_string);
        self.render_list();
    }

    pub fn set_local_only(&self, on: bool) {
        self.local_only.set_active(on);
    }

    pub fn press_export(&self) {
        self.export_button.emit_clicked();
    }
}

/// The short word for where a provider runs, as in "Workstation · network".
pub fn locality_word(l: Locality) -> &'static str {
    match l {
        Locality::ThisComputer => "this computer",
        Locality::Network => "network",
        Locality::Cloud => "cloud",
    }
}

/// The answer as Pango markup, with each `[d12:p4]` marker replaced by the
/// number of its citation chip.
fn numbered_markup(text: &str, citations: &[Citation], accent: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("[d") {
        let Some(len) = rest[start..].find(']') else {
            break;
        };
        let marker = &rest[start + 2..start + len];
        let number = marker.split_once(":p").and_then(|(d, p)| {
            let (d, p) = (d.parse::<i64>().ok()?, p.parse::<i64>().ok()?);
            citations
                .iter()
                .position(|c| c.document_id == d && c.paragraph_id == p)
        });
        // The number sits right after the word it supports.
        let before = match number {
            Some(_) => rest[..start].trim_end(),
            None => &rest[..start],
        };
        out.push_str(&gtk::glib::markup_escape_text(before));
        match number {
            Some(n) => out.push_str(&format!(
                "<sup><span font_family=\"IBM Plex Sans\" weight=\"600\" foreground=\"{accent}\">{}</span></sup>",
                n + 1
            )),
            None => out.push_str(&gtk::glib::markup_escape_text(&rest[start..=start + len])),
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(&gtk::glib::markup_escape_text(rest));
    out
}

/// A rounded square filled with the colour `color` returns (none: nothing).
fn color_area(size: i32, radius: f64, color: impl Fn() -> Option<String> + 'static) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(size)
        .content_height(size)
        .build();
    area.set_draw_func(move |_, cr, w, h| {
        let Some(c) = color().and_then(|c| gtk::gdk::RGBA::parse(c.as_str()).ok()) else {
            return;
        };
        let (w, h) = (f64::from(w), f64::from(h));
        let r = radius.min(w / 2.0).min(h / 2.0);
        use std::f64::consts::PI;
        cr.new_sub_path();
        cr.arc(w - r, r, r, -PI / 2.0, 0.0);
        cr.arc(w - r, h - r, r, 0.0, PI / 2.0);
        cr.arc(r, h - r, r, PI / 2.0, PI);
        cr.arc(r, r, r, PI, 1.5 * PI);
        cr.close_path();
        cr.set_source_rgba(c.red().into(), c.green().into(), c.blue().into(), 1.0);
        let _ = cr.fill();
    });
    area
}

/// A fixed-width table cell.
fn sized(w: impl IsA<gtk::Widget>, width: i32) -> gtk::Widget {
    w.set_size_request(width, -1);
    w.set_hexpand(false);
    w.set_valign(gtk::Align::Center);
    if let Some(l) = w.dynamic_cast_ref::<gtk::Label>() {
        l.set_xalign(0.0);
    }
    w.upcast()
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn field(name: &str, w: &impl IsA<gtk::Widget>, spacing: i32) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, spacing);
    b.append(&label(name, &["fx-field-label"]));
    b.append(w);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cite(document_id: i64, paragraph_id: i64) -> Citation {
        Citation {
            document_id,
            paragraph_id,
        }
    }

    #[test]
    fn markers_become_citation_numbers() {
        let m = numbered_markup("A [d1:p2], B [d3:p4].", &[cite(1, 2), cite(3, 4)], "#000");
        assert!(m.starts_with("A<sup>"), "{m}");
        assert!(
            m.contains(">1</span></sup>,") && m.contains(">2</span></sup>."),
            "{m}"
        );
        assert!(!m.contains("[d"), "{m}");
    }

    #[test]
    fn unknown_markers_and_markup_are_kept_as_text() {
        let m = numbered_markup("x < y [d9:p9]", &[], "#000");
        assert_eq!(m, "x &lt; y [d9:p9]");
    }
}
