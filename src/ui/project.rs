//! The Project view: documents in a project (or with a tag), filtered by tag
//! and text, with the project's settings and a combined export. With AI on,
//! tabs add questions over the documents, a summary and action items.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;

use super::ai_panels::{ActionsPanel, SummaryPanel};
use super::{Deps, Handler, label};
use crate::ai::actions::Answer;
use crate::ai::service::Scope as AiScope;
use crate::store::{DocumentFilter, DocumentId, DocumentSummary, ProjectFilter, Source, Store};
use crate::template::{Template, load_dir};
use crate::text::{danish_date, duration};

pub const COLORS: [&str; 5] = ["#C2410C", "#1D4ED8", "#0F766E", "#6B21A8", "#9AA1AE"];

#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    Project(ProjectFilter),
    Tag(String),
}

pub struct ProjectPage {
    pub root: gtk::Box,
    store: Rc<Store>,
    deps: Deps,
    /// Documents / Ask / Summary / Actions.
    pub tabs: gtk::Stack,
    tab_switcher: gtk::StackSwitcher,
    pub question: gtk::Entry,
    pub ask_button: gtk::Button,
    pub answer: gtk::Label,
    pub ask_status: gtk::Label,
    citations: gtk::Box,
    pub summary: Rc<SummaryPanel>,
    pub actions: Rc<ActionsPanel>,
    templates_dir: std::path::PathBuf,
    scope: RefCell<Scope>,
    title: gtk::Label,
    meta: gtk::Label,
    local_badge: gtk::Label,
    search: gtk::SearchEntry,
    chips: gtk::Box,
    tag_filter: RefCell<Option<String>>,
    count: gtk::Label,
    list: gtk::ListBox,
    shown: RefCell<Vec<DocumentId>>,
    panel: gtk::Box,
    name: gtk::Entry,
    local_only: gtk::Switch,
    default_template: gtk::DropDown,
    template_ids: RefCell<Vec<String>>,
    export_button: gtk::Button,
    loading: std::cell::Cell<bool>,
    on_open: Handler<DocumentId>,
    on_export: Handler<(String, Vec<DocumentId>)>,
    on_changed: Handler<()>,
}

impl ProjectPage {
    pub fn new(store: Rc<Store>, deps: Deps) -> Rc<Self> {
        let templates_dir = deps.paths.templates();
        let head = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let title = label("", &["fx-project-title"]);
        let meta_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let meta = label("", &["fx-status"]);
        let local_badge = label("LOCAL ONLY", &["fx-chip"]);
        meta_row.append(&meta);
        meta_row.append(&local_badge);
        head.append(&title);
        head.append(&meta_row);

        let filters = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search text in these documents")
            .width_chars(28)
            .build();
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let count = label("", &["fx-stats"]);
        count.set_hexpand(true);
        count.set_xalign(1.0);
        filters.append(&search);
        filters.append(&chips);
        filters.append(&count);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("fx-doc-list");
        let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();

        let documents = gtk::Box::new(gtk::Orientation::Vertical, 18);
        documents.append(&filters);
        documents.append(&scroller);

        let ask = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let ask_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let question = gtk::Entry::builder()
            .placeholder_text("Ask about these documents, e.g. Hvad blev prisen?")
            .hexpand(true)
            .build();
        question.update_property(&[gtk::accessible::Property::Label("Question")]);
        let ask_button = gtk::Button::with_label("Ask");
        ask_button.add_css_class("fx-primary");
        ask_row.append(&question);
        ask_row.append(&ask_button);
        let ask_status = label(
            "Answers come only from these documents and point to the paragraphs they use.",
            &["fx-field-note"],
        );
        ask_status.set_wrap(true);
        let answer = label("", &["fx-answer"]);
        answer.set_wrap(true);
        answer.set_selectable(true);
        answer.set_valign(gtk::Align::Start);
        let citations = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let answer_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        answer_box.append(&answer);
        answer_box.append(&citations);
        let answer_scroll = gtk::ScrolledWindow::builder()
            .child(&answer_box)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        ask.append(&ask_row);
        ask.append(&ask_status);
        ask.append(&answer_scroll);

        let summary = SummaryPanel::new(Rc::clone(&store), "Summarize these documents");
        let actions = ActionsPanel::new(Rc::clone(&store), None);
        let tabs = gtk::Stack::new();
        tabs.set_vexpand(true);
        tabs.add_titled(&documents, Some("documents"), "Documents");
        tabs.add_titled(&ask, Some("ask"), "Ask");
        tabs.add_titled(&summary.root, Some("summary"), "Summary");
        tabs.add_titled(&actions.root, Some("actions"), "Action items");
        let tab_switcher = gtk::StackSwitcher::builder()
            .stack(&tabs)
            .halign(gtk::Align::Start)
            .build();

        let main = gtk::Box::new(gtk::Orientation::Vertical, 18);
        main.add_css_class("fx-project-main");
        main.set_hexpand(true);
        main.append(&head);
        main.append(&tab_switcher);
        main.append(&tabs);

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 16);
        panel.add_css_class("fx-inspector");
        panel.set_size_request(300, -1);
        panel.append(&label("PROJECT", &["fx-section-title"]));
        let name = gtk::Entry::new();
        name.add_css_class("fx-field");
        name.update_property(&[gtk::accessible::Property::Label("Project name")]);
        panel.append(&field("Name", &name));
        let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        panel.append(&field("Color", &swatches));
        let default_template = gtk::DropDown::from_strings(&[]);
        panel.append(&field("Default template for new documents", &default_template));
        let local_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let local_text = label("Local only (no cloud AI)", &["fx-field-label"]);
        local_text.set_hexpand(true);
        let local_only = gtk::Switch::new();
        local_only.update_property(&[gtk::accessible::Property::Label("Local only")]);
        local_row.append(&local_text);
        local_row.append(&local_only);
        panel.append(&local_row);
        panel.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        panel.append(&label("EXPORT", &["fx-section-title"]));
        let export_note = label(
            "Combine the shown documents into one report, oldest first.",
            &["fx-field-note"],
        );
        export_note.set_wrap(true);
        panel.append(&export_note);
        let export_button = gtk::Button::with_label("Export documents…");
        export_button.add_css_class("fx-primary");
        panel.append(&export_button);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&main);
        root.append(&panel);

        let page = Rc::new(Self {
            root,
            store,
            deps,
            tabs,
            tab_switcher,
            question,
            ask_button,
            answer,
            ask_status,
            citations,
            summary,
            actions,
            templates_dir,
            scope: RefCell::new(Scope::Project(ProjectFilter::All)),
            title,
            meta,
            local_badge,
            search,
            chips,
            tag_filter: RefCell::default(),
            count,
            list,
            shown: RefCell::default(),
            panel,
            name,
            local_only,
            default_template,
            template_ids: RefCell::default(),
            export_button,
            loading: std::cell::Cell::new(false),
            on_open: RefCell::default(),
            on_export: RefCell::default(),
            on_changed: RefCell::default(),
        });
        for color in COLORS {
            let b = gtk::Button::new();
            b.add_css_class("fx-color-swatch");
            b.set_size_request(32, 32);
            b.set_tooltip_text(Some(color));
            let area = gtk::DrawingArea::builder()
                .content_width(20)
                .content_height(20)
                .build();
            let rgba = gtk::gdk::RGBA::parse(color).unwrap_or(gtk::gdk::RGBA::BLACK);
            area.set_draw_func(move |_, cr, w, h| {
                cr.set_source_rgba(rgba.red().into(), rgba.green().into(), rgba.blue().into(), 1.0);
                cr.rectangle(0.0, 0.0, w.into(), h.into());
                let _ = cr.fill();
            });
            b.set_child(Some(&area));
            let weak = Rc::downgrade(&page);
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.save_project(Some(color));
                }
            });
            swatches.append(&b);
        }
        page.wire();
        page.refresh_ai();
        page
    }

    /// Shows the AI tabs only when AI is on.
    pub fn refresh_ai(&self) {
        let on = self.deps.settings().ai.enabled;
        self.tab_switcher.set_visible(on);
        if !on {
            self.tabs.set_visible_child_name("documents");
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

    pub fn ask(self: &Rc<Self>) {
        let question = self.question.text().trim().to_string();
        if question.is_empty() {
            return;
        }
        self.tabs.set_visible_child_name("ask");
        self.answer.set_text("");
        while let Some(c) = self.citations.first_child() {
            self.citations.remove(&c);
        }
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
                        p.ask_status.set_text(&super::ai::error_text(&e));
                    }
                }
            }),
        );
    }

    fn show_answer(self: &Rc<Self>, a: &Answer) {
        self.answer.set_text(&a.text);
        self.ask_status.set_text(match a.citations.len() {
            0 => "No sources were cited; check the answer against the documents.",
            _ => "Sources:",
        });
        for c in &a.citations {
            let Ok(doc) = self.store.document(c.document_id) else {
                continue;
            };
            let n = self
                .store
                .paragraphs(c.document_id)
                .ok()
                .and_then(|ps| ps.iter().position(|p| p.id == Some(c.paragraph_id)))
                .map_or(0, |i| i + 1);
            let b = gtk::Button::with_label(&format!(
                "[d{}:p{}]  {} · paragraph {n}",
                c.document_id, c.paragraph_id, doc.title
            ));
            b.add_css_class("flat");
            b.set_halign(gtk::Align::Start);
            b.set_tooltip_text(Some("Open the document"));
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
            if let Some(b) = c.downcast_ref::<gtk::Button>() {
                out.push(b.label().map(|l| l.to_string()).unwrap_or_default());
            }
            child = c.next_sibling();
        }
        out
    }

    pub fn summarize(self: &Rc<Self>) {
        self.tabs.set_visible_child_name("summary");
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
        self.summary.load(latest);
        let ids: Vec<DocumentId> = self
            .store
            .documents(&self.base_filter())
            .unwrap_or_default()
            .iter()
            .map(|d| d.id)
            .collect();
        let items = self
            .store
            .action_items(ProjectFilter::All)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| ids.contains(&a.document_id))
            .collect();
        self.actions.show(items);
        self.answer.set_text("");
        while let Some(c) = self.citations.first_child() {
            self.citations.remove(&c);
        }
    }

    fn wire(self: &Rc<Self>) {
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
            }
        });
        let weak = Rc::downgrade(self);
        self.export_button.connect_clicked(move |_| {
            let Some(p) = weak.upgrade() else { return };
            let ids = p.shown.borrow().clone();
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
        self.panel
            .set_visible(project.is_some() || matches!(scope, Scope::Tag(_)));
        for w in [
            self.name.upcast_ref::<gtk::Widget>(),
            self.default_template.upcast_ref(),
            self.local_only.upcast_ref(),
        ] {
            if let Some(row) = w.parent() {
                row.set_visible(project.is_some());
            }
        }
        if let Some(p) = &project {
            self.name.set_text(&p.name);
            self.local_only.set_active(p.local_only);
            let templates: Vec<Template> = load_dir(&self.templates_dir)
                .into_iter()
                .filter_map(Result::ok)
                .collect();
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
        self.local_badge
            .set_visible(project.as_ref().is_some_and(|p| p.local_only));
        self.loading.set(false);
        self.render_list();
        self.load_ai_results();
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
        if let Err(e) = self
            .store
            .update_project(id, &name, color.unwrap_or(&p.color), template.as_deref())
        {
            tracing::error!("saving project {id}: {e}");
        }
        self.title.set_text(&name);
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
        self.export_button
            .set_label(&format!("Export {}…", plural(docs.len(), "document")));
        self.export_button.set_sensitive(!docs.is_empty());
        *self.shown.borrow_mut() = docs.iter().map(|d| d.id).collect();
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
        all.add_css_class("fx-tag");
        all.set_active(active.is_none());
        let mut buttons = vec![(None, all.clone())];
        for (tag, n) in tags {
            let b = gtk::ToggleButton::with_label(&format!("#{tag} · {n}"));
            b.add_css_class("fx-tag");
            b.set_group(Some(&all));
            b.set_active(active.as_deref() == Some(tag.as_str()));
            buttons.push((Some(tag), b));
        }
        for (tag, b) in buttons {
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
        b.add_css_class("fx-doc-row");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        let icon = gtk::Image::from_icon_name(match d.source {
            Source::Dictation => "audio-input-microphone-symbolic",
            Source::File => "text-x-generic-symbolic",
        });
        row.append(&icon);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.append(&label(&d.title, &["fx-field-label"]));
        let source = match d.source {
            Source::Dictation => "Dictation",
            Source::File => "From file",
        };
        let sub = match &d.template_id {
            Some(t) => format!("{source} · {t}"),
            None => source.to_string(),
        };
        text.append(&label(&sub, &["fx-stats"]));
        row.append(&text);
        let tags = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tags.set_valign(gtk::Align::Center);
        for t in &d.tags {
            tags.append(&label(&format!("#{t}"), &["fx-tag"]));
        }
        row.append(&tags);
        row.append(&label(&danish_date(d.created_at), &["fx-stats"]));
        row.append(&label(
            &d.duration_ms.map(duration).unwrap_or_default(),
            &["fx-stats"],
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

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn field(name: &str, w: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&label(name, &["fx-field-label"]));
    b.append(w);
    b
}
