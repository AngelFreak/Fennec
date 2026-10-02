//! The right-hand panel: template choice, report fields, words to review.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gtk::prelude::*;

use super::label;
use crate::template::{FieldKind, Template};

struct FieldRow {
    key: String,
    entry: gtk::Entry,
    note: gtk::Label,
    required: bool,
    /// "Suggested by AI" card under the entry, with Accept / Dismiss.
    card: gtk::Box,
    card_value: gtk::Label,
    accept: gtk::Button,
}

pub struct Inspector {
    pub root: gtk::Box,
    pub template_choice: gtk::DropDown,
    pub suggest_status: gtk::Label,
    fields_box: gtk::Box,
    entries: RefCell<Vec<FieldRow>>,
    review_box: gtk::Box,
    review_count: gtk::Label,
    pub stats: gtk::Label,
    pub recorded: gtk::Label,
    templates: RefCell<Vec<Template>>,
    on_change: super::Handler<()>,
    on_review: super::Handler<usize>,
}

impl Inspector {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 16);

        root.append(&label("REPORT FIELDS", &["fx-section-title"]));
        let tpl_label = label("Template", &["fx-field-label"]);
        let template_choice = gtk::DropDown::from_strings(&[]);
        template_choice.update_property(&[gtk::accessible::Property::Label("Template")]);
        let tpl_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        tpl_box.append(&tpl_label);
        tpl_box.append(&template_choice);
        root.append(&tpl_box);
        let suggest_status = label("", &["fx-field-note"]);
        suggest_status.set_wrap(true);
        suggest_status.set_visible(false);
        root.append(&suggest_status);

        let fields_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
        root.append(&fields_box);

        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&label("REVIEW", &["fx-section-title"]));
        let review_count = label("No unsure words", &["fx-field-label"]);
        root.append(&review_count);
        let review_box = super::wrap::wrap_box();
        root.append(&review_box);

        // Word count on the left, recorded length on the right.
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        footer.set_vexpand(true);
        footer.set_valign(gtk::Align::End);
        let stats = label("", &["fx-stats"]);
        stats.set_hexpand(true);
        let recorded = label("", &["fx-stats"]);
        footer.append(&stats);
        footer.append(&recorded);
        root.append(&footer);

        Rc::new(Self {
            root,
            template_choice,
            suggest_status,
            fields_box,
            entries: RefCell::default(),
            review_box,
            review_count,
            stats,
            recorded,
            templates: RefCell::default(),
            on_change: RefCell::default(),
            on_review: RefCell::default(),
        })
    }

    pub fn connect_change(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Rc::new(move |()| f()));
    }

    pub fn connect_review(&self, f: impl Fn(usize) + 'static) {
        *self.on_review.borrow_mut() = Some(Rc::new(f));
    }

    pub fn set_templates(&self, templates: Vec<Template>, selected: &str) {
        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        self.template_choice
            .set_model(Some(&gtk::StringList::new(&names)));
        let idx = templates.iter().position(|t| t.id == selected).unwrap_or(0);
        *self.templates.borrow_mut() = templates;
        self.template_choice.set_selected(idx as u32);
    }

    pub fn selected_template(&self) -> Option<Template> {
        self.templates
            .borrow()
            .get(self.template_choice.selected() as usize)
            .cloned()
    }

    /// Builds one entry per template field, filled from `values`.
    pub fn show_fields(self: &Rc<Self>, template: &Template, values: &BTreeMap<String, String>) {
        while let Some(c) = self.fields_box.first_child() {
            self.fields_box.remove(&c);
        }
        let mut entries = Vec::new();
        for f in &template.fields {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
            let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let l = label(&f.label, &["fx-field-label"]);
            l.set_hexpand(true);
            head.append(&l);
            if f.kind == FieldKind::Date && f.default.contains("{today}") {
                head.append(&label("auto", &["fx-field-note"]));
            }
            b.append(&head);
            let entry = gtk::Entry::new();
            entry.add_css_class("fx-field");
            entry.set_text(values.get(&f.key).map(String::as_str).unwrap_or(""));
            entry.update_property(&[gtk::accessible::Property::Label(&f.label)]);
            // As in the mockup: dates Fennec fills itself are shaded, and a
            // field that takes the user's name asks for it.
            if f.kind == FieldKind::Date && f.default.contains("{today}") {
                entry.add_css_class("auto");
            }
            entry.set_placeholder_text(Some(if f.default.contains("{user}") {
                "Your name"
            } else {
                "Not filled in"
            }));
            b.append(&entry);
            let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
            card.add_css_class("fx-suggestion");
            card.set_visible(false);
            card.append(&label("SUGGESTED BY AI", &["fx-suggestion-title"]));
            let card_value = label("", &["fx-body"]);
            card_value.set_wrap(true);
            card_value.set_selectable(true);
            card.append(&card_value);
            let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let accept = gtk::Button::with_label("Accept");
            accept.add_css_class("fx-ink");
            let dismiss = gtk::Button::with_label("Dismiss");
            dismiss.add_css_class("fx-secondary");
            dismiss.add_css_class("small");
            buttons.append(&accept);
            buttons.append(&dismiss);
            card.append(&buttons);
            b.append(&card);
            {
                let entry = entry.clone();
                let value = card_value.clone();
                let card = card.clone();
                accept.connect_clicked(move |_| {
                    entry.set_text(&value.text());
                    card.set_visible(false);
                });
            }
            {
                let card = card.clone();
                dismiss.connect_clicked(move |_| card.set_visible(false));
            }
            let note = label("Required before export", &["fx-field-error"]);
            b.append(&note);
            let me = Rc::downgrade(self);
            entry.connect_changed(move |_| {
                if let Some(me) = me.upgrade() {
                    me.refresh_required();
                    if let Some(f) = me.on_change.borrow().clone() {
                        f(());
                    }
                }
            });
            self.fields_box.append(&b);
            entries.push(FieldRow {
                key: f.key.clone(),
                entry,
                note,
                required: f.required,
                card,
                card_value,
                accept,
            });
        }
        *self.entries.borrow_mut() = entries;
        self.refresh_required();
    }

    fn refresh_required(&self) {
        for row in self.entries.borrow().iter() {
            let missing = row.required && row.entry.text().trim().is_empty();
            row.note.set_visible(missing);
            if missing {
                row.entry.add_css_class("required-empty");
            } else {
                row.entry.remove_css_class("required-empty");
            }
            // A value the user typed makes the suggestion moot.
            if !row.entry.text().is_empty() {
                row.card.set_visible(false);
            }
        }
    }

    pub fn values(&self) -> BTreeMap<String, String> {
        self.entries
            .borrow()
            .iter()
            .map(|r| (r.key.clone(), r.entry.text().to_string()))
            .collect()
    }

    /// The entry for field `key` (tests).
    pub fn entry(&self, key: &str) -> Option<gtk::Entry> {
        self.entries
            .borrow()
            .iter()
            .find(|r| r.key == key)
            .map(|r| r.entry.clone())
    }

    /// Shows AI suggestions as cards under empty fields. Nothing is
    /// filled in until the user presses Accept.
    pub fn show_suggestions(&self, suggestions: &BTreeMap<String, String>) {
        for row in self.entries.borrow().iter() {
            match suggestions.get(&row.key) {
                Some(v) if row.entry.text().is_empty() => {
                    row.card_value.set_text(v);
                    row.card.set_visible(true);
                }
                _ => {}
            }
        }
    }

    /// The pending suggestion for `key` (tests).
    pub fn suggestion(&self, key: &str) -> Option<String> {
        self.entries
            .borrow()
            .iter()
            .find(|r| r.key == key && r.card.get_visible())
            .map(|r| r.card_value.text().to_string())
    }

    /// Accepts the suggestion for `key`, as its Accept button does.
    pub fn use_suggestion(&self, key: &str) {
        let button = self
            .entries
            .borrow()
            .iter()
            .find(|r| r.key == key)
            .map(|r| r.accept.clone());
        if let Some(b) = button {
            b.emit_clicked();
        }
    }

    pub fn set_unsure(self: &Rc<Self>, words: &[String]) {
        while let Some(c) = self.review_box.first_child() {
            self.review_box.remove(&c);
        }
        self.review_count.set_text(&match words.len() {
            0 => "No unsure words".to_string(),
            1 => "1 low-confidence word".to_string(),
            n => format!("{n} low-confidence words"),
        });
        for (i, w) in words.iter().enumerate() {
            let b = gtk::Button::with_label(w);
            b.add_css_class("fx-unsure-word");
            b.set_tooltip_text(Some("Select in the text"));
            let me = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(f) = me.upgrade().and_then(|m| m.on_review.borrow().clone()) {
                    f(i);
                }
            });
            self.review_box.append(&b);
        }
    }
}
