//! The Project view: documents in a project (or with a tag), filtered by tag
//! and text, with the project's settings and a combined export. With AI on,
//! tabs add the project's action items and questions over its documents
//! (including a project summary).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use super::ai_panels::{ActionsPanel, SummaryPanel};
use super::{Deps, Handler, label};
use crate::ai::Locality;
use crate::ai::actions::{Answer, Citation};
use crate::ai::service::Scope as AiScope;
use crate::store::{DocumentFilter, DocumentId, DocumentSummary, ProjectFilter, ProjectId, Source, Store};
use crate::template::load_dir;
use crate::text::{duration, hours_minutes, short_date};

pub const COLORS: [&str; 5] = ["#C2410C", "#1D4ED8", "#0F766E", "#6B21A8", "#9AA1AE"];
const COLOR_NAMES: [&str; 5] = ["Orange", "Blue", "Teal", "Purple", "Grey"];
/// The colour square for "All documents" and "Unsorted" (as in the sidebar).
const NEUTRAL: &str = "#9AA1AE";

/// Table column widths (the document column takes the rest).
const TAGS_WIDTH: i32 = 168;
const DATE_WIDTH: i32 = 112;
const LENGTH_WIDTH: i32 = 64;

const SUGGESTIONS: [&str; 2] = ["What changed since last week?", "Open questions"];

/// How long Undo is offered after a delete.
const UNDO_TIME: Duration = Duration::from_secs(10);

/// Documents deleted but kept until Undo is no longer offered.
struct PendingDelete {
    ids: Vec<DocumentId>,
    timer: glib::SourceId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    Project(ProjectFilter),
    Tag(String),
}

/// What a row menu item does, given the page and the row's ⋯ button.
type MenuAction = Rc<dyn Fn(&Rc<ProjectPage>, &gtk::Button)>;

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
    /// The table head, or the bulk bar while documents are selected.
    head: gtk::Stack,
    head_check: gtk::CheckButton,
    bulk_check: gtk::CheckButton,
    bulk_count: gtk::Label,
    bulk_tags: gtk::Button,
    selected: RefCell<Vec<DocumentId>>,
    /// Each row's checkbox and row, to show the selection.
    row_checks: RefCell<Vec<(DocumentId, gtk::CheckButton, gtk::Widget)>>,
    row_menus: RefCell<Vec<gtk::Button>>,
    /// Set while the checkboxes are updated from the selection.
    syncing: Cell<bool>,
    pending_delete: RefCell<Option<PendingDelete>>,
    /// The row or bulk menu that is open.
    menu: RefCell<Option<gtk::Popover>>,
    toast: gtk::Box,
    toast_label: gtk::Label,
    side: gtk::ScrolledWindow,
    project_fields: gtk::Box,
    name: gtk::Entry,
    swatches: Vec<(&'static str, gtk::Button)>,
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
    on_deleted: Handler<Vec<DocumentId>>,
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
        // As in the mockup: the search box, then the tag chips (wrapping)
        // with the count at the right.
        let filters = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search text in this project")
            .width_request(260)
            .halign(gtk::Align::Start)
            .build();
        search.update_property(&[gtk::accessible::Property::Label("Search in project")]);
        let chips = super::wrap::wrap_box();
        chips.set_hexpand(true);
        let count = label("", &["fx-stats"]);
        count.set_valign(gtk::Align::Center);
        let chip_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        chip_row.append(&chips);
        chip_row.append(&count);
        filters.append(&search);
        filters.append(&chip_row);

        let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.add_css_class("fx-table");
        table.set_overflow(gtk::Overflow::Hidden);
        // Columns as in a row: checkbox, the document's cells, its ⋯ menu.
        let head_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        head_row.add_css_class("fx-table-head");
        let head_check = gtk::CheckButton::new();
        head_check.set_valign(gtk::Align::Center);
        head_check.update_property(&[gtk::accessible::Property::Label("Select all")]);
        head_row.append(&head_check);
        let columns = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        columns.set_hexpand(true);
        columns.append(&sized(gtk::Box::new(gtk::Orientation::Horizontal, 0), 28));
        let doc_head = label("DOCUMENT", &[]);
        doc_head.set_hexpand(true);
        columns.append(&doc_head);
        columns.append(&sized(label("TAGS", &[]), TAGS_WIDTH));
        columns.append(&sized(label("DATE", &[]), DATE_WIDTH));
        columns.append(&sized(label("LENGTH", &[]), LENGTH_WIDTH));
        head_row.append(&columns);
        head_row.append(&sized(gtk::Box::new(gtk::Orientation::Horizontal, 0), 28));

        // The bulk bar replaces the head while documents are selected.
        let bulk = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bulk.add_css_class("fx-table-head");
        bulk.add_css_class("fx-bulk-bar");
        let bulk_check = gtk::CheckButton::new();
        bulk_check.set_valign(gtk::Align::Center);
        bulk_check.update_property(&[gtk::accessible::Property::Label("Select all")]);
        let bulk_count = label("", &["fx-bulk-count"]);
        bulk_count.set_valign(gtk::Align::Center);
        bulk_count.set_margin_start(4);
        bulk_count.set_margin_end(8);
        bulk.append(&bulk_check);
        bulk.append(&bulk_count);
        let bulk_move = bulk_button("Move to…", &[]);
        let bulk_tags = bulk_button("Tags", &[]);
        let bulk_export = bulk_button("Export…", &[]);
        let bulk_delete = bulk_button("Delete", &["danger"]);
        let bulk_clear = bulk_button("Clear selection", &["flat"]);
        bulk_clear.set_hexpand(true);
        bulk_clear.set_halign(gtk::Align::End);
        for b in [&bulk_move, &bulk_tags, &bulk_export, &bulk_delete, &bulk_clear] {
            bulk.append(b);
        }
        let table_head = gtk::Stack::new();
        table_head.add_named(&head_row, Some("head"));
        table_head.add_named(&bulk, Some("bulk"));
        table.append(&table_head);
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
        // Local only is set in Settings → Privacy; here it shows as a badge,
        // as in the mockup.
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

        // "Deleted …  Undo", over the bottom of the page.
        let toast = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        toast.add_css_class("fx-toast");
        toast.set_halign(gtk::Align::Center);
        toast.set_valign(gtk::Align::End);
        toast.set_margin_bottom(24);
        toast.set_visible(false);
        let toast_label = label("", &[]);
        toast_label.set_valign(gtk::Align::Center);
        let undo = gtk::Button::with_label("Undo");
        undo.set_valign(gtk::Align::Center);
        toast.append(&toast_label);
        toast.append(&undo);
        let overlay = gtk::Overlay::new();
        overlay.set_hexpand(true);
        overlay.set_child(Some(&main_scroll));
        overlay.add_overlay(&toast);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&overlay);
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
            head: table_head,
            head_check,
            bulk_check,
            bulk_count,
            bulk_tags,
            selected: RefCell::default(),
            row_checks: RefCell::default(),
            row_menus: RefCell::default(),
            syncing: Cell::new(false),
            pending_delete: RefCell::default(),
            menu: RefCell::default(),
            toast,
            toast_label,
            side,
            project_fields,
            name,
            swatches,
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
            on_deleted: RefCell::default(),
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
        page.wire_selection(&bulk_move, &bulk_export, &bulk_delete, &bulk_clear, &undo);
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
    /// Who answers questions here, after the local-only rule.
    fn provider_text(&self) -> String {
        let local = self.local_badge.get_visible();
        match self
            .deps
            .settings()
            .ai
            .effective_for(crate::ai::AiJob::Ask, local)
        {
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

    /// Types a question into the Ask box (tests).
    pub fn set_question(&self, text: &str) {
        self.question.set_text(text);
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
            _ => "",
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
        self.selected.borrow_mut().clear();
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
        // Documents waiting for Undo are already gone from the list.
        let hidden = self.hidden_ids();
        let mut all = self.store.documents(&self.base_filter()).unwrap_or_default();
        all.retain(|d| !hidden.contains(&d.id));
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
            (Some(a), Some(b)) if short_date(a) != short_date(b) => {
                format!(" · {} – {}", span_date(a), span_date(b))
            }
            (Some(a), _) => format!(" · {}", span_date(a)),
            _ => String::new(),
        };
        self.meta.set_text(&format!(
            "{} · {} of audio{span}",
            plural(all.len(), "document"),
            hours_minutes(total)
        ));
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.row_checks.borrow_mut().clear();
        self.row_menus.borrow_mut().clear();
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
        // Only documents in the list stay selected.
        let shown = self.shown.borrow().clone();
        self.selected.borrow_mut().retain(|id| shown.contains(id));
        self.sync_selection();
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
        tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
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

    fn row(self: &Rc<Self>, d: &DocumentSummary) -> gtk::Widget {
        // A checkbox, the document (a button that opens it) and its ⋯ menu.
        let outer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        outer.add_css_class("fx-project-doc-row");
        let check = gtk::CheckButton::new();
        check.set_valign(gtk::Align::Center);
        check.update_property(&[gtk::accessible::Property::Label(&format!("Select {}", d.title))]);
        outer.append(&check);
        let b = gtk::Button::new();
        b.add_css_class("fx-doc-open");
        b.set_hexpand(true);
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
            label(&short_date(d.created_at), &["fx-doc-date"]),
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
        outer.append(&b);
        let more = gtk::Button::with_label("⋯");
        more.add_css_class("fx-icon-button");
        more.add_css_class("fx-doc-more");
        more.set_valign(gtk::Align::Center);
        more.set_size_request(28, 28);
        more.set_tooltip_text(Some("More actions"));
        more.update_property(&[gtk::accessible::Property::Label(&format!(
            "More actions for {}",
            d.title
        ))]);
        outer.append(&more);
        let weak = Rc::downgrade(self);
        check.connect_toggled(move |c| {
            if let Some(p) = weak.upgrade()
                && !p.syncing.get()
            {
                p.select_document(id, c.is_active());
            }
        });
        let weak = Rc::downgrade(self);
        more.connect_clicked(move |m| {
            if let Some(p) = weak.upgrade() {
                p.row_menu(m, id);
            }
        });
        self.row_checks
            .borrow_mut()
            .push((id, check, outer.clone().upcast()));
        self.row_menus.borrow_mut().push(more);
        outer.upcast()
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

    /// Marks the shown project local only (or not) and saves it.
    pub fn set_local_only(&self, on: bool) {
        let Some(id) = self.project_id() else { return };
        if let Err(e) = self.store.set_project_local_only(id, on) {
            tracing::error!("saving local-only for project {id}: {e}");
        }
        self.local_badge.set_visible(on);
        self.local_note.set_visible(on);
        if let Some(f) = self.on_changed.borrow().clone() {
            f(());
        }
    }

    pub fn press_export(&self) {
        self.export_button.emit_clicked();
    }
}

/// Selecting documents, and moving, tagging, exporting and deleting them.
impl ProjectPage {
    fn wire_selection(
        self: &Rc<Self>,
        bulk_move: &gtk::Button,
        bulk_export: &gtk::Button,
        bulk_delete: &gtk::Button,
        bulk_clear: &gtk::Button,
        undo: &gtk::Button,
    ) {
        let weak = Rc::downgrade(self);
        self.head_check.connect_toggled(move |c| {
            if let Some(p) = weak.upgrade()
                && !p.syncing.get()
                && c.is_active()
            {
                p.select_all();
            }
        });
        let weak = Rc::downgrade(self);
        self.bulk_check.connect_toggled(move |_| {
            let Some(p) = weak.upgrade() else { return };
            if p.syncing.get() {
                return;
            }
            if p.selected.borrow().len() == p.shown.borrow().len() {
                p.clear_selection();
            } else {
                p.select_all();
            }
        });
        let weak = Rc::downgrade(self);
        bulk_move.connect_clicked(move |b| {
            if let Some(p) = weak.upgrade() {
                let ids = p.selected_ids();
                p.move_menu(b, ids);
            }
        });
        let weak = Rc::downgrade(self);
        self.bulk_tags.connect_clicked(move |b| {
            if let Some(p) = weak.upgrade() {
                let ids = p.selected_ids();
                p.tags_menu(b, ids);
            }
        });
        let weak = Rc::downgrade(self);
        bulk_export.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                let ids = p.selected_ids();
                p.export_documents(p.title.text().to_string(), ids);
            }
        });
        let weak = Rc::downgrade(self);
        bulk_delete.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.delete_selected();
            }
        });
        let weak = Rc::downgrade(self);
        bulk_clear.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.clear_selection();
            }
        });
        let weak = Rc::downgrade(self);
        undo.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.undo_delete();
            }
        });
    }

    /// Documents were deleted for good (the editor closes them if open).
    pub fn connect_deleted(&self, f: impl Fn(Vec<DocumentId>) + 'static) {
        *self.on_deleted.borrow_mut() = Some(Rc::new(f));
    }

    pub fn select_document(&self, id: DocumentId, on: bool) {
        {
            let mut selected = self.selected.borrow_mut();
            selected.retain(|s| *s != id);
            if on && self.shown.borrow().contains(&id) {
                selected.push(id);
            }
        }
        self.sync_selection();
    }

    pub fn select_all(&self) {
        *self.selected.borrow_mut() = self.shown.borrow().clone();
        self.sync_selection();
    }

    pub fn clear_selection(&self) {
        self.selected.borrow_mut().clear();
        self.sync_selection();
    }

    /// Selected documents, in list order.
    pub fn selected_ids(&self) -> Vec<DocumentId> {
        let selected = self.selected.borrow();
        self.shown
            .borrow()
            .iter()
            .copied()
            .filter(|id| selected.contains(id))
            .collect()
    }

    /// Whether the bulk bar is in place of the table head (tests).
    pub fn bulk_bar_shown(&self) -> bool {
        self.head.visible_child_name().as_deref() == Some("bulk")
    }

    /// Checkboxes, row highlights and the head follow the selection.
    fn sync_selection(&self) {
        self.syncing.set(true);
        let selected = self.selected.borrow().clone();
        for (id, check, row) in self.row_checks.borrow().iter() {
            let on = selected.contains(id);
            check.set_active(on);
            if on {
                row.add_css_class("selected");
            } else {
                row.remove_css_class("selected");
            }
        }
        let (n, shown) = (selected.len(), self.shown.borrow().len());
        self.head
            .set_visible_child_name(if n > 0 { "bulk" } else { "head" });
        self.bulk_count.set_text(&format!("{n} selected"));
        self.bulk_check.set_active(n > 0 && n == shown);
        self.bulk_check.set_inconsistent(n > 0 && n < shown);
        self.head_check.set_active(false);
        self.syncing.set(false);
    }

    /// Redraws the list and lets the sidebar update its counts and tags.
    fn changed(self: &Rc<Self>) {
        self.render_list();
        self.notify_changed();
    }

    fn notify_changed(&self) {
        if let Some(f) = self.on_changed.borrow().clone() {
            f(());
        }
    }

    fn export_documents(&self, title: String, ids: Vec<DocumentId>) {
        if ids.is_empty() {
            return;
        }
        if let Some(f) = self.on_export.borrow().clone() {
            f((title, ids));
        }
    }

    /// Moves documents to a project (`None`: Unsorted).
    pub fn move_documents(self: &Rc<Self>, ids: &[DocumentId], project: Option<ProjectId>) {
        for id in ids {
            if let Err(e) = self.store.move_document(*id, project) {
                tracing::error!("moving document {id}: {e}");
            }
        }
        self.changed();
    }

    pub fn move_selected(self: &Rc<Self>, project: Option<ProjectId>) {
        let ids = self.selected_ids();
        self.move_documents(&ids, project);
    }

    /// Adds `tag` to the documents, or takes it off. Only the store changes;
    /// the list redraws when the tag menu closes.
    fn tag_documents(&self, ids: &[DocumentId], tag: &str, on: bool) {
        let tag = tag.trim().trim_start_matches('#').to_lowercase();
        if tag.is_empty() {
            return;
        }
        for id in ids {
            let Ok(doc) = self.store.document(*id) else {
                continue;
            };
            let mut tags = doc.tags;
            tags.retain(|t| *t != tag);
            if on {
                tags.push(tag.clone());
            }
            if let Err(e) = self.store.set_tags(*id, &tags) {
                tracing::error!("tagging document {id}: {e}");
            }
        }
    }

    pub fn set_tag_on_selected(self: &Rc<Self>, tag: &str, on: bool) {
        self.tag_documents(&self.selected_ids(), tag, on);
        self.changed();
    }

    pub fn rename_document(self: &Rc<Self>, id: DocumentId, title: &str) {
        let title = title.trim();
        let Ok(doc) = self.store.document(id) else { return };
        if title.is_empty() || title == doc.title {
            return;
        }
        if let Err(e) = self
            .store
            .update_document(id, title, doc.template_id.as_deref(), &doc.fields)
        {
            tracing::error!("renaming document {id}: {e}");
        }
        self.changed();
    }

    pub fn delete_selected(self: &Rc<Self>) {
        let ids = self.selected_ids();
        self.delete_documents(ids);
    }

    /// Hides the documents and offers Undo; they are deleted when the offer
    /// runs out, another delete starts, or the window closes.
    pub fn delete_documents(self: &Rc<Self>, ids: Vec<DocumentId>) {
        if ids.is_empty() {
            return;
        }
        self.finish_delete();
        let text = match ids.as_slice() {
            [id] => format!(
                "Deleted «{}»",
                self.store.document(*id).map(|d| d.title).unwrap_or_default()
            ),
            _ => format!("Deleted {}", plural(ids.len(), "document")),
        };
        self.selected.borrow_mut().retain(|s| !ids.contains(s));
        let weak = Rc::downgrade(self);
        // The timer's own run takes the pending delete, so it is never removed twice.
        let timer = glib::timeout_add_local_once(UNDO_TIME, move || {
            if let Some(p) = weak.upgrade() {
                p.commit_delete(false);
            }
        });
        *self.pending_delete.borrow_mut() = Some(PendingDelete { ids, timer });
        self.toast_label.set_text(&text);
        self.toast.set_visible(true);
        self.render_list();
    }

    pub fn undo_delete(self: &Rc<Self>) {
        let Some(pending) = self.pending_delete.borrow_mut().take() else {
            return;
        };
        pending.timer.remove();
        self.toast.set_visible(false);
        self.render_list();
    }

    /// Deletes the documents waiting for Undo now.
    pub fn finish_delete(&self) {
        self.commit_delete(true);
    }

    /// Deletes the text, and audio that Fennec recorded; imported originals
    /// belong to the user and stay.
    fn commit_delete(&self, cancel_timer: bool) {
        let Some(pending) = self.pending_delete.borrow_mut().take() else {
            return;
        };
        if cancel_timer {
            pending.timer.remove();
        }
        self.toast.set_visible(false);
        let audio_dir = self.deps.paths.audio();
        for id in &pending.ids {
            if let Ok(Some(path)) = self.store.document(*id).map(|d| d.audio_path)
                && path.starts_with(&audio_dir)
                && let Err(e) = std::fs::remove_file(&path)
            {
                tracing::warn!("could not delete the audio {}: {e}", path.display());
            }
            if let Err(e) = self.store.delete_document(*id) {
                tracing::error!("deleting document {id}: {e}");
            }
        }
        if let Some(f) = self.on_deleted.borrow().clone() {
            f(pending.ids);
        }
        self.notify_changed();
    }

    fn hidden_ids(&self) -> Vec<DocumentId> {
        self.pending_delete
            .borrow()
            .as_ref()
            .map(|p| p.ids.clone())
            .unwrap_or_default()
    }

    /// The Undo bar's text while it shows (tests).
    pub fn toast_text(&self) -> Option<String> {
        self.toast
            .get_visible()
            .then(|| self.toast_label.text().to_string())
    }

    fn menu(&self, anchor: &gtk::Button, content: &gtk::Box, width: i32) -> gtk::Popover {
        self.close_menu();
        let pop = popover(anchor, content, width);
        *self.menu.borrow_mut() = Some(pop.clone());
        pop
    }

    /// Closes the open row or bulk menu, if any.
    pub fn close_menu(&self) {
        if let Some(pop) = self.menu.borrow_mut().take() {
            pop.popdown();
            // The page keeps one menu at a time; it leaves when the next opens.
            if pop.parent().is_some() {
                pop.unparent();
            }
        }
    }

    /// Ids of the documents shown, in order (tests).
    pub fn shown_ids(&self) -> Vec<DocumentId> {
        self.shown.borrow().clone()
    }

    /// Opens the bulk bar's Tags menu (screenshots).
    pub fn open_bulk_tags(&self) {
        self.bulk_tags.emit_clicked();
    }

    /// Opens the ⋯ menu of the `index`th row (screenshots).
    pub fn open_row_menu(&self, index: usize) {
        let more = self.row_menus.borrow().get(index).cloned();
        if let Some(m) = more {
            m.emit_clicked();
        }
    }

    /// A row's ⋯ menu: Open, Rename, Move to, Tags, Export, Delete.
    fn row_menu(self: &Rc<Self>, anchor: &gtk::Button, id: DocumentId) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let open = menu_item("Open", None);
        let rename = menu_item("Rename…", None);
        let move_to = menu_item("Move to", Some("›"));
        let tags = menu_item("Tags…", None);
        let export = menu_item("Export…", None);
        let delete = menu_item("Delete", None);
        delete.add_css_class("danger");
        for b in [&open, &rename, &move_to, &tags, &export] {
            content.append(b);
        }
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&delete);
        let pop = self.menu(anchor, &content, 220);

        // Each item closes the menu first; the work runs once it is gone.
        let item = |b: &gtk::Button, run: MenuAction| {
            let weak = Rc::downgrade(self);
            let pop = pop.downgrade();
            let anchor = anchor.clone();
            b.connect_clicked(move |_| {
                if let Some(pop) = pop.upgrade() {
                    pop.popdown();
                }
                let weak = weak.clone();
                let run = Rc::clone(&run);
                let anchor = anchor.clone();
                glib::idle_add_local_once(move || {
                    if let Some(p) = weak.upgrade() {
                        run(&p, &anchor);
                    }
                });
            });
        };
        item(
            &open,
            Rc::new(move |p: &Rc<Self>, _: &gtk::Button| {
                if let Some(f) = p.on_open.borrow().clone() {
                    f(id);
                }
            }),
        );
        item(
            &rename,
            Rc::new(move |p: &Rc<Self>, a: &gtk::Button| p.rename_menu(a, id)),
        );
        item(
            &move_to,
            Rc::new(move |p: &Rc<Self>, a: &gtk::Button| p.move_menu(a, vec![id])),
        );
        item(
            &tags,
            Rc::new(move |p: &Rc<Self>, a: &gtk::Button| p.tags_menu(a, vec![id])),
        );
        item(
            &export,
            Rc::new(move |p: &Rc<Self>, _: &gtk::Button| {
                let title = p.store.document(id).map(|d| d.title).unwrap_or_default();
                p.export_documents(title, vec![id]);
            }),
        );
        item(
            &delete,
            Rc::new(move |p: &Rc<Self>, _: &gtk::Button| p.delete_documents(vec![id])),
        );
    }

    fn rename_menu(self: &Rc<Self>, anchor: &gtk::Button, id: DocumentId) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let entry = gtk::Entry::builder()
            .text(self.store.document(id).map(|d| d.title).unwrap_or_default())
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Document title")]);
        let save = gtk::Button::with_label("Rename");
        save.add_css_class("fx-primary");
        content.append(&entry);
        content.append(&save);
        let pop = self.menu(anchor, &content, 260);
        entry.grab_focus();
        let weak = Rc::downgrade(self);
        let pop = pop.downgrade();
        let done = Rc::new(move |text: String| {
            if let Some(pop) = pop.upgrade() {
                pop.popdown();
            }
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(p) = weak.upgrade() {
                    p.rename_document(id, &text);
                }
            });
        });
        let d = Rc::clone(&done);
        let e = entry.clone();
        save.connect_clicked(move |_| d(e.text().to_string()));
        entry.connect_activate(move |e| done(e.text().to_string()));
    }

    /// The projects (and Unsorted) to move `ids` to; the current one is ticked.
    fn move_menu(self: &Rc<Self>, anchor: &gtk::Button, ids: Vec<DocumentId>) {
        if ids.is_empty() {
            return;
        }
        let current: Vec<Option<ProjectId>> = ids
            .iter()
            .filter_map(|id| self.store.document(*id).ok().map(|d| d.project_id))
            .collect();
        let shared = current
            .first()
            .copied()
            .filter(|c| current.iter().all(|x| x == c));
        let mut targets: Vec<(String, String, Option<ProjectId>)> = self
            .store
            .projects()
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.name, p.color, Some(p.id)))
            .collect();
        targets.push(("Unsorted".into(), "#C9CED6".into(), None));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        content.append(&label("MOVE TO", &["fx-section-title", "fx-menu-head"]));
        let pop = self.menu(anchor, &content, 220);
        for (name, color, target) in targets {
            let b = gtk::Button::new();
            b.add_css_class("fx-menu-item");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let square = color_area(10, 2.0, move || Some(color.clone()));
            square.set_valign(gtk::Align::Center);
            row.append(&square);
            let l = label(&name, &["fx-menu-title"]);
            l.set_hexpand(true);
            row.append(&l);
            if shared == Some(target) {
                row.append(&gtk::Image::from_icon_name("fennec-check-symbolic"));
            }
            b.set_child(Some(&row));
            b.update_property(&[gtk::accessible::Property::Label(&format!("Move to {name}"))]);
            let weak = Rc::downgrade(self);
            let ids = ids.clone();
            let pop = pop.downgrade();
            b.connect_clicked(move |_| {
                if let Some(pop) = pop.upgrade() {
                    pop.popdown();
                }
                let weak = weak.clone();
                let ids = ids.clone();
                glib::idle_add_local_once(move || {
                    if let Some(p) = weak.upgrade() {
                        p.move_documents(&ids, target);
                    }
                });
            });
            content.append(&b);
        }
    }

    /// Every tag, ticked when all of `ids` have it and mixed when some do;
    /// the entry finds a tag or, with Enter, creates and adds it.
    fn tags_menu(self: &Rc<Self>, anchor: &gtk::Button, ids: Vec<DocumentId>) {
        if ids.is_empty() {
            return;
        }
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let entry = gtk::Entry::builder()
            .placeholder_text("Find or create a tag")
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Find or create a tag")]);
        content.append(&entry);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&list);
        let pop = self.menu(anchor, &content, 260);
        let ids = Rc::new(ids);
        let fill: Rc<dyn Fn()> = {
            let weak = Rc::downgrade(self);
            let list = list.clone();
            let ids = Rc::clone(&ids);
            Rc::new(move || {
                let Some(p) = weak.upgrade() else { return };
                p.fill_tags(&list, &ids);
            })
        };
        fill();
        let l = list.clone();
        entry.connect_changed(move |e| {
            let find = e.text().trim().trim_start_matches('#').to_lowercase();
            let mut row = l.first_child();
            while let Some(r) = row {
                r.set_visible(r.widget_name().contains(find.as_str()));
                row = r.next_sibling();
            }
        });
        let weak = Rc::downgrade(self);
        let i = Rc::clone(&ids);
        entry.connect_activate(move |e| {
            let Some(p) = weak.upgrade() else { return };
            p.tag_documents(&i, &e.text(), true);
            e.set_text("");
            fill();
        });
        // The list and sidebar catch up once the menu closes.
        let weak = Rc::downgrade(self);
        pop.connect_closed(move |_| {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(p) = weak.upgrade() {
                    p.changed();
                }
            });
        });
    }

    fn fill_tags(self: &Rc<Self>, list: &gtk::Box, ids: &Rc<Vec<DocumentId>>) {
        while let Some(c) = list.first_child() {
            list.remove(&c);
        }
        let docs: Vec<Vec<String>> = ids
            .iter()
            .filter_map(|id| self.store.document(*id).ok().map(|d| d.tags))
            .collect();
        let n = docs.len();
        let tags = self.store.tags().unwrap_or_default();
        if tags.is_empty() {
            list.append(&label(
                "No tags yet. Type one and press Enter.",
                &["fx-menu-note"],
            ));
        }
        for (tag, _) in tags {
            let have = docs.iter().filter(|t| t.contains(&tag)).count();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.add_css_class("fx-tag-choice");
            row.set_widget_name(&tag);
            let check = gtk::CheckButton::with_label(&tag);
            check.set_hexpand(true);
            check.set_active(have == n);
            check.set_inconsistent(have > 0 && have < n);
            let count = label(
                &if n > 1 && have > 0 {
                    format!("{have} of {n}")
                } else {
                    String::new()
                },
                &["fx-tag-count"],
            );
            count.set_valign(gtk::Align::Center);
            row.append(&check);
            row.append(&count);
            let weak = Rc::downgrade(self);
            let ids = Rc::clone(ids);
            check.connect_toggled(move |c| {
                let Some(p) = weak.upgrade() else { return };
                c.set_inconsistent(false);
                p.tag_documents(&ids, &tag, c.is_active());
                count.set_text(&if n > 1 && c.is_active() {
                    format!("{n} of {n}")
                } else {
                    String::new()
                });
            });
            list.append(&row);
        }
    }
}

impl Drop for ProjectPage {
    /// A delete waiting for Undo is carried out when the window closes.
    fn drop(&mut self) {
        self.commit_delete(true);
    }
}

/// A small button in the bulk bar.
fn bulk_button(text: &str, classes: &[&str]) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("fx-secondary");
    b.add_css_class("small");
    for c in classes {
        b.add_css_class(c);
    }
    b.set_valign(gtk::Align::Center);
    b
}

/// A menu row with its text and, optionally, a mark at the right.
fn menu_item(text: &str, mark: Option<&str>) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("fx-menu-item");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let l = label(text, &["fx-menu-title"]);
    l.set_hexpand(true);
    row.append(&l);
    if let Some(m) = mark {
        row.append(&label(m, &["fx-menu-note"]));
    }
    b.set_child(Some(&row));
    b
}

/// A popover on `anchor` holding `child`, taken down again once closed.
fn popover(anchor: &impl IsA<gtk::Widget>, child: &impl IsA<gtk::Widget>, width: i32) -> gtk::Popover {
    let pop = gtk::Popover::new();
    pop.add_css_class("fx-doc-menu");
    pop.set_has_arrow(false);
    pop.set_position(gtk::PositionType::Bottom);
    child.set_size_request(width, -1);
    pop.set_child(Some(child));
    pop.set_parent(anchor);
    pop.popup();
    pop
}

/// The short word for where a provider runs, as in "Workstation · network".
/// "Sep 12" (never "Today": a span reads "Sep 12 – Oct 2").
fn span_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%b %-d").to_string())
        .unwrap_or_default()
}

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
                "<sup><span font_family=\"IBM Plex Sans\" weight=\"600\" underline=\"single\" foreground=\"{accent}\">{}</span></sup>",
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
