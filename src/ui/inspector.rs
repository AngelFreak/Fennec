//! The right-hand panel: template choice, report fields, words to review.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gtk::prelude::*;

use super::label;
use crate::template::{FieldKind, Template};

pub struct Inspector {
    pub root: gtk::Box,
    pub template_choice: gtk::DropDown,
    fields_box: gtk::Box,
    entries: RefCell<Vec<(String, gtk::Entry, gtk::Label, bool)>>,
    review_box: gtk::FlowBox,
    review_count: gtk::Label,
    pub stats: gtk::Label,
    templates: RefCell<Vec<Template>>,
    on_change: super::Handler<()>,
    on_review: super::Handler<usize>,
}

impl Inspector {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
        root.add_css_class("fx-inspector");
        root.set_size_request(300, -1);

        root.append(&label("REPORT FIELDS", &["fx-section-title"]));
        let tpl_label = label("Template", &["fx-field-label"]);
        let template_choice = gtk::DropDown::from_strings(&[]);
        template_choice.update_property(&[gtk::accessible::Property::Label("Template")]);
        let tpl_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        tpl_box.append(&tpl_label);
        tpl_box.append(&template_choice);
        root.append(&tpl_box);

        let fields_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
        root.append(&fields_box);

        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&label("REVIEW", &["fx-section-title"]));
        let review_count = label("No unsure words", &["fx-field-label"]);
        root.append(&review_count);
        let review_box = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(6)
            .row_spacing(6)
            .max_children_per_line(4)
            .build();
        root.append(&review_box);

        let stats = label("", &["fx-stats"]);
        stats.set_vexpand(true);
        stats.set_valign(gtk::Align::End);
        root.append(&stats);

        Rc::new(Self {
            root,
            template_choice,
            fields_box,
            entries: RefCell::default(),
            review_box,
            review_count,
            stats,
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
            b.append(&entry);
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
            entries.push((f.key.clone(), entry, note, f.required));
        }
        *self.entries.borrow_mut() = entries;
        self.refresh_required();
    }

    fn refresh_required(&self) {
        for (_, entry, note, required) in self.entries.borrow().iter() {
            let missing = *required && entry.text().trim().is_empty();
            note.set_visible(missing);
            if missing {
                entry.add_css_class("required-empty");
            } else {
                entry.remove_css_class("required-empty");
            }
        }
    }

    pub fn values(&self) -> BTreeMap<String, String> {
        self.entries
            .borrow()
            .iter()
            .map(|(k, e, _, _)| (k.clone(), e.text().to_string()))
            .collect()
    }

    /// The entry for field `key` (tests).
    pub fn entry(&self, key: &str) -> Option<gtk::Entry> {
        self.entries
            .borrow()
            .iter()
            .find(|(k, ..)| k == key)
            .map(|(_, e, ..)| e.clone())
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
            self.review_box.insert(&b, -1);
        }
    }
}
