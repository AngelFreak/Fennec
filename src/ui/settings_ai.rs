//! Settings → AI providers, AI defaults and Privacy. AI is off until the
//! user turns it on; keys go to the keyring, everything else to
//! `settings.toml`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::settings_page::{field, narrow, page};
use super::{Deps, Handler, label};
use crate::ai::{AiError, AiJob, Locality, Protocol, ProviderConfig, connect, presets};
use crate::store::Store;

const LOCALITIES: [Locality; 3] = [Locality::ThisComputer, Locality::Network, Locality::Cloud];

/// The status line under a provider: colour class and text.
#[derive(Clone)]
struct Status {
    state: &'static str,
    text: String,
}

pub struct AiSettingsUi {
    deps: Deps,
    pub enabled: gtk::Switch,
    providers_box: gtk::Box,
    pub default_choice: gtk::DropDown,
    /// One dropdown per job; entry 0 is "Default provider".
    pub job_choices: Vec<(AiJob, gtk::DropDown)>,
    loading_default: Cell<bool>,
    pub message: gtk::Label,
    /// Results of "Test", by provider id; kept across re-renders.
    tested: RefCell<HashMap<String, Status>>,
    /// Providers whose editor is open.
    editing: RefCell<HashSet<String>>,
    projects_box: gtk::Box,
    on_changed: Handler<()>,
}

impl AiSettingsUi {
    pub fn new(deps: Deps) -> Rc<Self> {
        let message = label("", &["fx-field-error"]);
        message.set_wrap(true);
        let job_choices = AiJob::ALL
            .iter()
            .map(|j| {
                let dd = gtk::DropDown::from_strings(&[]);
                dd.update_property(&[gtk::accessible::Property::Label(&format!(
                    "Provider for {}",
                    j.label().to_lowercase()
                ))]);
                (*j, dd)
            })
            .collect();
        let ui = Rc::new(Self {
            enabled: gtk::Switch::builder()
                .active(deps.settings().ai.enabled)
                .valign(gtk::Align::Center)
                .build(),
            deps,
            providers_box: gtk::Box::new(gtk::Orientation::Vertical, 16),
            default_choice: gtk::DropDown::from_strings(&[]),
            job_choices,
            loading_default: Cell::new(false),
            message,
            tested: RefCell::default(),
            editing: RefCell::default(),
            projects_box: gtk::Box::new(gtk::Orientation::Vertical, 0),
            on_changed: RefCell::default(),
        });
        ui.enabled
            .update_property(&[gtk::accessible::Property::Label("Use AI")]);
        let weak = Rc::downgrade(&ui);
        ui.enabled.connect_active_notify(move |sw| {
            if let Some(u) = weak.upgrade() {
                u.edit(|ai| ai.enabled = sw.is_active());
            }
        });
        let weak = Rc::downgrade(&ui);
        ui.default_choice.connect_selected_notify(move |dd| {
            let Some(u) = weak.upgrade() else { return };
            if u.loading_default.get() {
                return;
            }
            let id = u
                .deps
                .settings()
                .ai
                .providers
                .get(dd.selected() as usize)
                .map(|p| p.id.clone());
            if let Some(id) = id {
                u.edit(|ai| ai.default_provider = id);
                // Rebuilding this dropdown inside its own handler would recurse.
                glib::idle_add_local_once(move || u.render_providers());
            }
        });
        for (job, dd) in &ui.job_choices {
            let weak = Rc::downgrade(&ui);
            let job = *job;
            dd.connect_selected_notify(move |dd| {
                let Some(u) = weak.upgrade() else { return };
                if u.loading_default.get() {
                    return;
                }
                let chosen = dd.selected().checked_sub(1).and_then(|i| {
                    u.deps
                        .settings()
                        .ai
                        .providers
                        .get(i as usize)
                        .map(|p| p.id.clone())
                });
                u.edit(|ai| match chosen {
                    Some(id) => {
                        ai.jobs.insert(job.key().to_string(), id);
                    }
                    None => {
                        ai.jobs.remove(job.key());
                    }
                });
            });
        }
        ui
    }

    pub fn connect_changed(&self, f: impl Fn(()) + 'static) {
        *self.on_changed.borrow_mut() = Some(Rc::new(f));
    }

    /// Changes the AI settings, saves them and tells the window.
    fn edit(&self, f: impl FnOnce(&mut crate::ai::AiSettings)) {
        f(&mut self.deps.settings.borrow_mut().ai);
        match self.deps.save_settings() {
            Ok(()) => self.message.set_text(""),
            Err(e) => self.message.set_text(&format!("Settings not saved: {e}")),
        }
        if let Some(cb) = self.on_changed.borrow().clone() {
            cb(());
        }
    }

    fn edit_provider(&self, id: &str, f: impl FnOnce(&mut ProviderConfig)) {
        self.edit(|ai| {
            if let Some(p) = ai.providers.iter_mut().find(|p| p.id == id) {
                f(p);
            }
        });
    }

    // ---- AI providers ----

    pub fn providers_section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(820), 16);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let title = label("AI providers", &["fx-h1"]);
        title.set_hexpand(true);
        head.append(&title);

        let add_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        add_content.append(&gtk::Image::from_icon_name("fennec-add-symbolic"));
        add_content.append(&gtk::Label::new(Some("Add provider")));
        let add = gtk::MenuButton::builder()
            .child(&add_content)
            .always_show_arrow(false)
            .css_classes(["fx-add"])
            .valign(gtk::Align::Center)
            .build();
        add.update_property(&[gtk::accessible::Property::Label("Add provider")]);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let pop = gtk::Popover::builder().child(&menu).build();
        for (i, preset) in presets().into_iter().enumerate() {
            let item = gtk::Button::builder()
                .child(&label(preset.label, &["fx-menu-title"]))
                .css_classes(["fx-menu-item"])
                .build();
            let weak = Rc::downgrade(self);
            let pop2 = pop.clone();
            item.connect_clicked(move |_| {
                pop2.popdown();
                if let Some(u) = weak.upgrade() {
                    u.add_preset(i);
                }
            });
            menu.append(&item);
        }
        add.set_popover(Some(&pop));
        head.append(&add);
        let intro = gtk::Box::new(gtk::Orientation::Vertical, 8);
        intro.append(&head);
        let note = label(
            "Presets: Claude, ChatGPT, Ollama, llama.cpp / LM Studio / vLLM, network server, other \
             OpenAI-compatible cloud.",
            &["fx-status"],
        );
        note.set_wrap(true);
        intro.append(&note);
        b.append(&intro);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("fx-switch-row");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.append(&label("Use AI", &["fx-h3"]));
        let use_note = label(
            "Off by default: nothing is sent anywhere until you turn this on and start an action yourself.",
            &["fx-field-note"],
        );
        use_note.set_wrap(true);
        text.append(&use_note);
        row.append(&text);
        row.append(&self.enabled);
        b.append(&row);

        b.append(&self.providers_box);
        b.append(&self.message);
        self.render_providers();
        outer
    }

    pub fn add_preset(self: &Rc<Self>, index: usize) {
        let Some(preset) = presets().into_iter().nth(index) else {
            return;
        };
        let mut cfg = preset.config;
        self.edit(|ai| {
            cfg.id = ai.unique_id(&cfg.id);
            if ai.providers.is_empty() {
                ai.default_provider = cfg.id.clone();
            }
            ai.providers.push(cfg);
        });
        self.render_providers();
    }

    pub fn render_providers(self: &Rc<Self>) {
        while let Some(c) = self.providers_box.first_child() {
            self.providers_box.remove(&c);
        }
        let ai = self.deps.settings().ai;
        if ai.providers.is_empty() {
            let empty = label(
                "No providers yet. Add Claude or ChatGPT (cloud, needs an API key), or a model \
                 running on this computer or your network (Ollama, llama.cpp, LM Studio, vLLM).",
                &["fx-field-note"],
            );
            empty.set_wrap(true);
            self.providers_box.append(&empty);
        }
        let active = ai.active().map(|p| p.id.clone());
        for p in &ai.providers {
            let card = self.provider_card(p, active.as_deref() == Some(p.id.as_str()));
            self.providers_box.append(&card);
        }
        self.render_default_choice();
    }

    /// Opens or closes a provider's editor ("Edit" / "Done").
    pub fn set_editing(self: &Rc<Self>, id: &str, open: bool) {
        if open {
            self.editing.borrow_mut().insert(id.to_string());
        } else {
            self.editing.borrow_mut().remove(id);
        }
        // Re-render so the buttons and status follow a new key or name.
        self.render_providers();
    }

    /// The status line before any test: key state.
    fn initial_status(&self, p: &ProviderConfig) -> Status {
        if let Some(s) = self.tested.borrow().get(&p.id) {
            return s.clone();
        }
        let has_key = self.deps.secrets.get(&p.id).is_some();
        if p.locality == Locality::Cloud && !has_key {
            let mut text = "No API key.".to_string();
            if p.base_url.contains("api.openai.com") {
                text.push_str(" A ChatGPT Plus/Pro subscription doesn't include API access.");
            }
            return Status { state: "bad", text };
        }
        Status {
            state: "idle",
            text: if has_key {
                "Key saved in the keyring · not tested".into()
            } else {
                "Not tested".into()
            },
        }
    }

    fn provider_card(self: &Rc<Self>, p: &ProviderConfig, is_default: bool) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("fx-card");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let info = gtk::Box::new(gtk::Orientation::Vertical, 6);
        info.set_hexpand(true);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = label(&p.name, &["fx-h2"]);
        head.append(&title);
        let (badge, kind) = match p.locality {
            Locality::Cloud => ("CLOUD", "cloud"),
            Locality::Network => ("NETWORK", "network"),
            Locality::ThisComputer => ("THIS COMPUTER", "local"),
        };
        let badge = label(badge, &["fx-badge", kind]);
        badge.set_valign(gtk::Align::Center);
        head.append(&badge);
        let mut proto = match p.protocol {
            Protocol::Anthropic => "Anthropic · native API".to_string(),
            Protocol::OpenAi if p.base_url.contains("api.openai.com") => "OpenAI API".to_string(),
            Protocol::OpenAi => "OpenAI-compatible".to_string(),
        };
        if is_default {
            proto.push_str(" · default");
        }
        head.append(&label(&proto, &["fx-provider-proto"]));
        info.append(&head);
        let address = label(&address_text(p), &["fx-mono", "fx-provider-addr"]);
        address.set_ellipsize(gtk::pango::EllipsizeMode::End);
        info.append(&address);
        let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let dot = gtk::Box::builder()
            .css_classes(["fx-dot"])
            .valign(gtk::Align::Center)
            .build();
        let status = label("", &[]);
        status.set_wrap(true);
        status_row.append(&dot);
        status_row.append(&status);
        info.append(&status_row);
        let show_status = {
            let row = status_row.clone();
            let status = status.clone();
            move |s: &Status| {
                row.set_css_classes(&["fx-status-line", s.state]);
                status.set_text(&s.text);
            }
        };
        show_status(&self.initial_status(p));
        top.append(&info);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_valign(gtk::Align::Center);
        let needs_key = p.locality == Locality::Cloud && self.deps.secrets.get(&p.id).is_none();
        let test = gtk::Button::with_label("Test");
        test.add_css_class("fx-secondary");
        test.update_property(&[gtk::accessible::Property::Label(&format!("Test {}", p.name))]);
        let editing = self.editing.borrow().contains(&p.id);
        let edit = gtk::Button::with_label(if editing {
            "Done"
        } else if needs_key {
            "Add API key…"
        } else {
            "Edit"
        });
        edit.add_css_class("fx-secondary");
        test.set_visible(!needs_key);
        buttons.append(&test);
        buttons.append(&edit);
        top.append(&buttons);
        card.append(&top);

        let editor = self.provider_editor(p, is_default, &title, &address);
        let revealer = gtk::Revealer::builder()
            .child(&editor)
            .reveal_child(editing)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .build();
        card.append(&revealer);

        let id = p.id.clone();
        let weak = Rc::downgrade(self);
        edit.connect_clicked(glib::clone!(
            #[strong]
            id,
            move |_| {
                let Some(u) = weak.upgrade() else { return };
                let open = !u.editing.borrow().contains(&id);
                u.set_editing(&id, open);
            }
        ));
        let weak = Rc::downgrade(self);
        test.connect_clicked(move |b| {
            let Some(u) = weak.upgrade() else { return };
            let Some(cfg) = u.deps.settings().ai.provider(&id).cloned() else {
                return;
            };
            b.set_sensitive(false);
            show_status(&Status {
                state: "idle",
                text: "Testing…".into(),
            });
            let secrets = std::sync::Arc::clone(&u.deps.secrets);
            let has_key = secrets.get(&cfg.id).is_some();
            let (tx, rx) = async_channel::bounded::<Result<Vec<String>, AiError>>(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(connect(&cfg, secrets.as_ref()).list_models());
            });
            let weak = Rc::downgrade(&u);
            let id = id.clone();
            let show_status = show_status.clone();
            let b = b.clone();
            glib::spawn_future_local(async move {
                let Ok(result) = rx.recv().await else { return };
                let s = match result {
                    Ok(models) => {
                        let mut text = format!(
                            "Connected · {} model{}",
                            models.len(),
                            if models.len() == 1 { "" } else { "s" }
                        );
                        if has_key {
                            text.push_str(" · key in the keyring");
                        }
                        b.set_label("Test");
                        Status { state: "ok", text }
                    }
                    Err(e) => {
                        b.set_label("Retry");
                        Status {
                            state: "bad",
                            text: super::ai::error_text(&e),
                        }
                    }
                };
                b.set_sensitive(true);
                show_status(&s);
                if let Some(u) = weak.upgrade() {
                    u.tested.borrow_mut().insert(id, s);
                }
            });
        });
        card
    }

    /// The fields behind "Edit": name, address, model, key, locality, size.
    fn provider_editor(
        self: &Rc<Self>,
        p: &ProviderConfig,
        is_default: bool,
        title: &gtk::Label,
        address: &gtk::Label,
    ) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
        card.add_css_class("fx-provider-editor");
        let grid = gtk::Grid::builder()
            .column_homogeneous(true)
            .column_spacing(16)
            .row_spacing(12)
            .build();
        let name = gtk::Entry::builder().text(p.name.as_str()).build();
        name.update_property(&[gtk::accessible::Property::Label("Provider name")]);
        grid.attach(&field("Name", &name), 0, 0, 1, 1);
        let locality = gtk::DropDown::from_strings(&LOCALITIES.map(Locality::label));
        locality.set_selected(LOCALITIES.iter().position(|l| *l == p.locality).unwrap_or(2) as u32);
        grid.attach(&field("Runs on", &locality), 1, 0, 1, 1);

        let url = gtk::Entry::builder().text(p.base_url.as_str()).build();
        url.add_css_class("fx-mono");
        grid.attach(&field("Address", &url), 0, 1, 1, 1);

        let model_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let model = gtk::Entry::builder()
            .text(p.model.as_str())
            .placeholder_text("Model name")
            .hexpand(true)
            .build();
        model.add_css_class("fx-mono");
        let fetch = gtk::MenuButton::builder().label("Fetch list").build();
        fetch.add_css_class("fx-secondary");
        fetch.set_tooltip_text(Some("Ask the provider which models it has"));
        let models_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let models_scroll = gtk::ScrolledWindow::builder()
            .child(&models_box)
            .max_content_height(320)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        fetch.set_popover(Some(&gtk::Popover::builder().child(&models_scroll).build()));
        model_row.append(&model);
        model_row.append(&fetch);
        grid.attach(&field("Model", &model_row), 1, 1, 1, 1);

        let key_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let key = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .hexpand(true)
            .build();
        let has_key = self.deps.secrets.get(&p.id).is_some();
        key.set_placeholder_text(Some(if has_key {
            "Saved in the keyring"
        } else {
            "Paste the API key"
        }));
        key.update_property(&[gtk::accessible::Property::Label("API key")]);
        let save_key = gtk::Button::with_label("Save key");
        save_key.add_css_class("fx-secondary");
        key_row.append(&key);
        key_row.append(&save_key);
        let key_status = label("", &["fx-field-note"]);
        let key_field = field("API key (optional for local servers)", &key_row);
        key_field.append(&key_status);
        grid.attach(&key_field, 0, 2, 1, 1);

        let context = gtk::SpinButton::with_range(2_000.0, 1_000_000.0, 1_000.0);
        context.set_value(p.context_chars as f64);
        grid.attach(&field("Text per request (characters)", &context), 1, 2, 1, 1);
        card.append(&grid);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if !is_default {
            let make_default = gtk::Button::with_label("Make default");
            make_default.add_css_class("fx-secondary");
            let weak = Rc::downgrade(self);
            let id = p.id.clone();
            make_default.connect_clicked(move |_| {
                if let Some(u) = weak.upgrade() {
                    u.edit(|ai| ai.default_provider = id.clone());
                    u.render_providers();
                }
            });
            actions.append(&make_default);
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        actions.append(&spacer);
        let remove = gtk::Button::with_label("Remove provider");
        remove.add_css_class("fx-secondary");
        actions.append(&remove);
        card.append(&actions);

        let id = p.id.clone();
        let weak = Rc::downgrade(self);
        name.connect_changed(glib::clone!(
            #[strong]
            id,
            #[strong]
            title,
            move |e| {
                if let Some(u) = weak.upgrade() {
                    let v = e.text().trim().to_string();
                    title.set_text(&v);
                    u.edit_provider(&id, |p| p.name = v);
                    u.render_default_choice();
                }
            }
        ));
        let weak = Rc::downgrade(self);
        let refresh_address = glib::clone!(
            #[strong]
            id,
            #[strong]
            address,
            move |u: &Self| {
                if let Some(p) = u.deps.settings().ai.provider(&id) {
                    address.set_text(&address_text(p));
                }
            }
        );
        let refresh = refresh_address.clone();
        url.connect_changed(glib::clone!(
            #[strong]
            id,
            move |e| {
                if let Some(u) = weak.upgrade() {
                    u.edit_provider(&id, |p| p.base_url = e.text().trim().to_string());
                    refresh(&u);
                }
            }
        ));
        let weak = Rc::downgrade(self);
        model.connect_changed(glib::clone!(
            #[strong]
            id,
            move |e| {
                if let Some(u) = weak.upgrade() {
                    u.edit_provider(&id, |p| p.model = e.text().trim().to_string());
                    refresh_address(&u);
                }
            }
        ));
        let weak = Rc::downgrade(self);
        locality.connect_selected_notify(glib::clone!(
            #[strong]
            id,
            move |dd| {
                if let Some(u) = weak.upgrade() {
                    let l = LOCALITIES[(dd.selected() as usize).min(2)];
                    u.edit_provider(&id, |p| p.locality = l);
                    glib::idle_add_local_once(move || u.render_providers());
                }
            }
        ));
        let weak = Rc::downgrade(self);
        context.connect_value_changed(glib::clone!(
            #[strong]
            id,
            move |sp| {
                if let Some(u) = weak.upgrade() {
                    u.edit_provider(&id, |p| p.context_chars = sp.value() as usize);
                }
            }
        ));
        let weak = Rc::downgrade(self);
        save_key.connect_clicked(glib::clone!(
            #[strong]
            id,
            #[strong]
            key,
            #[strong]
            key_status,
            move |_| {
                let Some(u) = weak.upgrade() else { return };
                let text = key.text().trim().to_string();
                let result = if text.is_empty() {
                    u.deps.secrets.delete(&id).map(|()| "Key removed.")
                } else {
                    u.deps
                        .secrets
                        .set(&id, &text)
                        .map(|()| "Key saved in the keyring.")
                };
                match result {
                    Ok(msg) => {
                        key.set_text("");
                        key.set_placeholder_text(Some(if text.is_empty() {
                            "Paste the API key"
                        } else {
                            "Saved in the keyring"
                        }));
                        key_status.set_text(msg);
                        u.tested.borrow_mut().remove(&id);
                    }
                    Err(e) => key_status.set_text(&e),
                }
            }
        ));
        let weak = Rc::downgrade(self);
        remove.connect_clicked(glib::clone!(
            #[strong]
            id,
            move |_| {
                let Some(u) = weak.upgrade() else { return };
                if let Err(e) = u.deps.secrets.delete(&id) {
                    tracing::warn!("removing key for {id}: {e}");
                }
                u.edit(|ai| {
                    ai.providers.retain(|p| p.id != id);
                    ai.jobs.retain(|_, p| *p != id);
                    if ai.default_provider == id {
                        ai.default_provider = ai.providers.first().map(|p| p.id.clone()).unwrap_or_default();
                    }
                });
                u.editing.borrow_mut().remove(&id);
                u.tested.borrow_mut().remove(&id);
                u.render_providers();
            }
        ));
        let weak = Rc::downgrade(self);
        fetch.connect_notify_local(
            Some("active"),
            glib::clone!(
                #[strong]
                id,
                #[strong]
                model,
                #[strong]
                models_box,
                move |mb, _| {
                    if !mb.is_active() {
                        return;
                    }
                    if let Some(u) = weak.upgrade() {
                        u.fetch_models(&id, &models_box, &model);
                    }
                }
            ),
        );
        card
    }

    /// Lists the provider's models in `into`; picking one fills `entry`.
    fn fetch_models(&self, id: &str, into: &gtk::Box, entry: &gtk::Entry) {
        while let Some(c) = into.first_child() {
            into.remove(&c);
        }
        into.append(&label("Asking the provider…", &["fx-field-note"]));
        let Some(cfg) = self.deps.settings().ai.provider(id).cloned() else {
            return;
        };
        let secrets = std::sync::Arc::clone(&self.deps.secrets);
        let (tx, rx) = async_channel::bounded::<Result<Vec<String>, AiError>>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(connect(&cfg, secrets.as_ref()).list_models());
        });
        let into = into.clone();
        let entry = entry.clone();
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            while let Some(c) = into.first_child() {
                into.remove(&c);
            }
            match result {
                Ok(models) if models.is_empty() => {
                    into.append(&label("The provider lists no models.", &["fx-field-note"]))
                }
                Ok(models) => {
                    for m in models {
                        let b = gtk::Button::with_label(&m);
                        b.add_css_class("flat");
                        let entry = entry.clone();
                        b.connect_clicked(move |b| {
                            entry.set_text(&m);
                            if let Some(pop) = b
                                .ancestor(gtk::Popover::static_type())
                                .and_downcast::<gtk::Popover>()
                            {
                                pop.popdown();
                            }
                        });
                        into.append(&b);
                    }
                }
                Err(e) => {
                    let l = label(&super::ai::error_text(&e), &["fx-field-error"]);
                    l.set_wrap(true);
                    l.set_max_width_chars(40);
                    into.append(&l);
                }
            }
        });
    }

    // ---- AI defaults ----

    pub fn defaults_section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(720), 16);
        let intro = gtk::Box::new(gtk::Orientation::Vertical, 8);
        intro.append(&label("AI defaults", &["fx-h1"]));
        intro.append(&label("Which provider handles each job.", &["fx-status"]));
        b.append(&intro);

        let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table.add_css_class("fx-table");
        self.default_choice
            .update_property(&[gtk::accessible::Property::Label("Default provider")]);
        table.append(&job_row(
            "Default provider",
            "Used by every job left on “Default provider”",
            &self.default_choice,
            true,
        ));
        for (job, dd) in &self.job_choices {
            table.append(&job_row(job.label(), job.note(), dd, false));
        }
        b.append(&table);

        let language = gtk::Entry::builder()
            .text(self.deps.settings().ai.language.as_str())
            .placeholder_text("dansk")
            .build();
        let lang = field("Language of AI answers", &language);
        b.append(&narrow(&lang, 320));
        let weak = Rc::downgrade(self);
        language.connect_changed(move |e| {
            if let Some(u) = weak.upgrade() {
                let v = e.text().trim().to_string();
                u.edit(|ai| ai.language = if v.is_empty() { "dansk".into() } else { v });
            }
        });
        let note = label(
            "Local-only projects never use cloud providers, whatever is set here.",
            &["fx-field-note"],
        );
        note.set_wrap(true);
        b.append(&note);
        self.render_default_choice();
        outer
    }

    /// Refills the default and per-job dropdowns from the provider list.
    fn render_default_choice(&self) {
        let ai = self.deps.settings().ai;
        let names: Vec<String> = ai.providers.iter().map(choice_label).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        self.loading_default.set(true);
        self.default_choice.set_model(Some(&gtk::StringList::new(&refs)));
        let sel = ai
            .active()
            .and_then(|a| ai.providers.iter().position(|p| p.id == a.id))
            .unwrap_or(0);
        self.default_choice.set_selected(sel as u32);
        self.default_choice.set_sensitive(!ai.providers.is_empty());
        let mut with_default = vec!["Default provider"];
        with_default.extend(refs.iter().copied());
        for (job, dd) in &self.job_choices {
            dd.set_model(Some(&gtk::StringList::new(&with_default)));
            let sel = ai
                .jobs
                .get(job.key())
                .and_then(|id| ai.providers.iter().position(|p| &p.id == id))
                .map_or(0, |i| i + 1);
            dd.set_selected(sel as u32);
            dd.set_sensitive(!ai.providers.is_empty());
        }
        self.loading_default.set(false);
    }

    // ---- Privacy ----

    pub fn privacy_section(self: &Rc<Self>) -> gtk::Box {
        let (outer, b) = page(Some(720), 18);
        b.append(&label("Privacy", &["fx-h1"]));
        let callout = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        callout.add_css_class("fx-callout");
        callout.add_css_class("neutral");
        callout.append(&gtk::Image::from_icon_name("changes-prevent-symbolic"));
        let text = label(
            "Audio and transcription never leave this computer. Only text is sent, and only to an AI \
             provider you choose, when you press an AI action.",
            &[],
        );
        text.set_wrap(true);
        text.set_hexpand(true);
        callout.append(&text);
        b.append(&callout);

        let ask = gtk::CheckButton::builder()
            .label("Ask before a document is sent to a cloud provider for the first time")
            .active(true)
            .sensitive(false)
            .tooltip_text("Always on: Fennec never sends to a cloud provider without asking first.")
            .build();
        b.append(&ask);

        let local = gtk::Box::new(gtk::Orientation::Vertical, 8);
        local.append(&label("Local-only projects", &["fx-h2"]));
        let note = label(
            "Cloud providers are disabled for these projects, even if you said yes before. Local and \
             network providers still work.",
            &["fx-field-note"],
        );
        note.set_wrap(true);
        local.append(&note);
        self.projects_box.add_css_class("fx-table");
        self.projects_box.set_overflow(gtk::Overflow::Hidden);
        local.append(&self.projects_box);
        b.append(&local);
        // Projects change elsewhere; list them afresh each time this shows.
        let weak = Rc::downgrade(self);
        self.projects_box.connect_map(move |_| {
            if let Some(u) = weak.upgrade() {
                u.render_projects();
            }
        });

        let forget_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let forget = gtk::Button::with_label("Forget my cloud confirmations");
        forget.add_css_class("fx-secondary");
        let status = label("", &["fx-field-note"]);
        status.set_wrap(true);
        status.set_hexpand(true);
        forget_row.append(&forget);
        forget_row.append(&status);
        let db = self.deps.paths.database();
        forget.connect_clicked(glib::clone!(
            #[strong]
            status,
            move |_| {
                match Store::open(&db).and_then(|s| s.clear_cloud_consents()) {
                    Ok(n) => status.set_text(&format!(
                        "Forgotten ({n}). Fennec will ask again before the next cloud send."
                    )),
                    Err(e) => status.set_text(&format!("Could not forget: {e}")),
                }
            }
        ));
        b.append(&forget_row);
        self.render_projects();
        outer
    }

    /// One row per project with its Local only checkbox.
    pub fn render_projects(self: &Rc<Self>) {
        while let Some(c) = self.projects_box.first_child() {
            self.projects_box.remove(&c);
        }
        let db = self.deps.paths.database();
        let projects = match Store::open(&db).and_then(|s| s.projects()) {
            Ok(p) => p,
            Err(e) => {
                let row = project_row_box(true);
                row.append(&label(
                    &format!("Could not read the projects: {e}"),
                    &["fx-field-error"],
                ));
                self.projects_box.append(&row);
                return;
            }
        };
        if projects.is_empty() {
            let row = project_row_box(true);
            row.append(&label("No projects yet.", &["fx-field-note"]));
            self.projects_box.append(&row);
        }
        for (i, p) in projects.into_iter().enumerate() {
            let row = project_row_box(i == 0);
            let swatch = gtk::DrawingArea::builder()
                .content_width(10)
                .content_height(10)
                .valign(gtk::Align::Center)
                .build();
            let rgba = gtk::gdk::RGBA::parse(&p.color).unwrap_or(gtk::gdk::RGBA::BLACK);
            swatch.set_draw_func(move |_, cr, w, h| {
                let (w, h) = (f64::from(w), f64::from(h));
                let r = 3.0;
                cr.new_sub_path();
                cr.arc(w - r, r, r, -std::f64::consts::FRAC_PI_2, 0.0);
                cr.arc(w - r, h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
                cr.arc(r, h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
                cr.arc(r, r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
                cr.close_path();
                cr.set_source_rgba(rgba.red().into(), rgba.green().into(), rgba.blue().into(), 1.0);
                let _ = cr.fill();
            });
            row.append(&swatch);
            let name = label(&p.name, &[]);
            name.set_hexpand(true);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&name);
            let check = gtk::CheckButton::builder().active(p.local_only).build();
            check.update_property(&[gtk::accessible::Property::Label(&format!(
                "{} local only",
                p.name
            ))]);
            let db = db.clone();
            let id = p.id;
            let weak = Rc::downgrade(self);
            check.connect_toggled(move |c| {
                let Some(u) = weak.upgrade() else { return };
                match Store::open(&db).and_then(|s| s.set_project_local_only(id, c.is_active())) {
                    Ok(()) => {
                        if let Some(cb) = u.on_changed.borrow().clone() {
                            cb(());
                        }
                    }
                    Err(e) => u.message.set_text(&format!("Could not save: {e}")),
                }
            });
            row.append(&check);
            self.projects_box.append(&row);
        }
    }

    /// Project names with their Local only state, as listed (tests).
    pub fn local_only_rows(&self) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        let mut row = self.projects_box.first_child();
        while let Some(r) = row {
            let name = r
                .first_child()
                .and_then(|w| w.next_sibling())
                .and_downcast::<gtk::Label>();
            let check = r.last_child().and_downcast::<gtk::CheckButton>();
            if let (Some(n), Some(c)) = (name, check) {
                out.push((n.text().to_string(), c.is_active()));
            }
            row = r.next_sibling();
        }
        out
    }

    /// Ticks or unticks a project's Local only box (tests).
    pub fn set_local_only(&self, name: &str, on: bool) {
        let mut row = self.projects_box.first_child();
        while let Some(r) = row {
            let is_it = r
                .first_child()
                .and_then(|w| w.next_sibling())
                .and_downcast::<gtk::Label>()
                .is_some_and(|l| l.text() == name);
            if is_it && let Some(c) = r.last_child().and_downcast::<gtk::CheckButton>() {
                c.set_active(on);
            }
            row = r.next_sibling();
        }
    }
}

fn project_row_box(first: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("fx-project-check");
    if first {
        row.add_css_class("first");
    }
    row
}

/// "Claude · cloud" for the provider dropdowns.
fn choice_label(p: &ProviderConfig) -> String {
    let place = match p.locality {
        Locality::Cloud => "cloud",
        Locality::Network => "network",
        Locality::ThisComputer => "this computer",
    };
    format!("{} · {place}", p.name)
}

/// `base_url · model`, or just the address while no model is chosen.
fn address_text(p: &ProviderConfig) -> String {
    if p.model.is_empty() {
        p.base_url.clone()
    } else {
        format!("{} · {}", p.base_url, p.model)
    }
}

/// A row in the AI defaults table: job name and note, then its dropdown.
fn job_row(title: &str, note: &str, dd: &gtk::DropDown, first: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("fx-job-row");
    if first {
        row.add_css_class("first");
    }
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&label(title, &["fx-job-title"]));
    let n = label(note, &["fx-field-note"]);
    n.set_wrap(true);
    text.append(&n);
    row.append(&text);
    dd.set_size_request(300, -1);
    dd.set_valign(gtk::Align::Center);
    row.append(dd);
    row
}
