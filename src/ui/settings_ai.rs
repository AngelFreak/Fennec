//! Settings → AI providers, AI defaults and Privacy. AI is off until the
//! user turns it on; keys go to the keyring, everything else to
//! `settings.toml`.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::{Deps, Handler, label};
use crate::ai::{AiError, Locality, ProviderConfig, connect, presets};
use crate::store::Store;

const LOCALITIES: [Locality; 3] = [Locality::ThisComputer, Locality::Network, Locality::Cloud];

pub struct AiSettingsUi {
    deps: Deps,
    pub enabled: gtk::Switch,
    providers_box: gtk::Box,
    pub default_choice: gtk::DropDown,
    loading_default: std::cell::Cell<bool>,
    pub message: gtk::Label,
    on_changed: Handler<()>,
}

impl AiSettingsUi {
    pub fn new(deps: Deps) -> Rc<Self> {
        let message = label("", &["fx-field-error"]);
        message.set_wrap(true);
        let ui = Rc::new(Self {
            enabled: gtk::Switch::builder()
                .active(deps.settings().ai.enabled)
                .valign(gtk::Align::Center)
                .build(),
            deps,
            providers_box: gtk::Box::new(gtk::Orientation::Vertical, 12),
            default_choice: gtk::DropDown::from_strings(&[]),
            loading_default: std::cell::Cell::new(false),
            message,
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
        let b = section("AI providers");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text.set_hexpand(true);
        text.append(&label("Use AI", &["fx-crumb-current"]));
        let note = label(
            "Summaries, clean-up, action items, questions and field suggestions. Off by default: \
             nothing is sent anywhere until you turn this on and start an action yourself.",
            &["fx-field-note"],
        );
        note.set_wrap(true);
        text.append(&note);
        row.append(&text);
        row.append(&self.enabled);
        b.append(&row);

        let add = gtk::MenuButton::builder().label("Add provider").build();
        add.add_css_class("fx-secondary");
        add.set_halign(gtk::Align::Start);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let pop = gtk::Popover::builder().child(&menu).build();
        for (i, preset) in presets().into_iter().enumerate() {
            let item = gtk::Button::with_label(preset.label);
            item.add_css_class("flat");
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
        b.append(&self.providers_box);
        b.append(&add);
        b.append(&self.message);
        self.render_providers();
        b
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

    fn provider_card(self: &Rc<Self>, p: &ProviderConfig, is_default: bool) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("fx-model-card");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Entry::builder().text(p.name.as_str()).hexpand(true).build();
        name.update_property(&[gtk::accessible::Property::Label("Provider name")]);
        head.append(&name);
        head.append(&label(p.locality.label(), &["fx-chip"]));
        if is_default {
            head.append(&label("Default", &["fx-chip"]));
        } else {
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
            head.append(&make_default);
        }
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.set_tooltip_text(Some("Remove provider"));
        remove.update_property(&[gtk::accessible::Property::Label("Remove provider")]);
        remove.add_css_class("fx-secondary");
        head.append(&remove);
        card.append(&head);

        let url = gtk::Entry::builder().text(p.base_url.as_str()).build();
        card.append(&field("Address", &url));

        let model_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let model = gtk::Entry::builder()
            .text(p.model.as_str())
            .placeholder_text("Model name")
            .hexpand(true)
            .build();
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
        card.append(&field("Model", &model_row));

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
        card.append(&key_field);

        let locality = gtk::DropDown::from_strings(&LOCALITIES.map(Locality::label));
        locality.set_selected(LOCALITIES.iter().position(|l| *l == p.locality).unwrap_or(2) as u32);
        card.append(&field("Runs on", &locality));

        let context = gtk::SpinButton::with_range(2_000.0, 1_000_000.0, 1_000.0);
        context.set_value(p.context_chars as f64);
        card.append(&field("Text per request (characters)", &context));

        let id = p.id.clone();
        let weak = Rc::downgrade(self);
        name.connect_changed(glib::clone!(
            #[strong]
            id,
            move |e| {
                if let Some(u) = weak.upgrade() {
                    u.edit_provider(&id, |p| p.name = e.text().trim().to_string());
                }
            }
        ));
        let weak = Rc::downgrade(self);
        url.connect_changed(glib::clone!(
            #[strong]
            id,
            move |e| {
                if let Some(u) = weak.upgrade() {
                    u.edit_provider(&id, |p| p.base_url = e.text().trim().to_string());
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
                    if ai.default_provider == id {
                        ai.default_provider = ai.providers.first().map(|p| p.id.clone()).unwrap_or_default();
                    }
                });
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
        let b = section("AI defaults");
        self.default_choice
            .update_property(&[gtk::accessible::Property::Label("Default provider")]);
        b.append(&field("Provider used for AI actions", &self.default_choice));
        let language = gtk::Entry::builder()
            .text(self.deps.settings().ai.language.as_str())
            .placeholder_text("dansk")
            .build();
        b.append(&field("Language of AI answers", &language));
        let weak = Rc::downgrade(self);
        language.connect_changed(move |e| {
            if let Some(u) = weak.upgrade() {
                let v = e.text().trim().to_string();
                u.edit(|ai| ai.language = if v.is_empty() { "dansk".into() } else { v });
            }
        });
        let note = label(
            "AI results are labelled with the provider and model that wrote them. Summaries can be \
             edited and are included in exports unless you untick them.",
            &["fx-field-note"],
        );
        note.set_wrap(true);
        b.append(&note);
        self.render_default_choice();
        b
    }

    fn render_default_choice(&self) {
        let ai = self.deps.settings().ai;
        let names: Vec<String> = ai
            .providers
            .iter()
            .map(|p| format!("{} · {}", p.name, p.locality.label()))
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        self.loading_default.set(true);
        self.default_choice.set_model(Some(&gtk::StringList::new(&refs)));
        let sel = ai
            .active()
            .and_then(|a| ai.providers.iter().position(|p| p.id == a.id))
            .unwrap_or(0);
        self.default_choice.set_selected(sel as u32);
        self.default_choice.set_sensitive(!ai.providers.is_empty());
        self.loading_default.set(false);
    }

    // ---- Privacy ----

    pub fn privacy_section(self: &Rc<Self>) -> gtk::Box {
        let b = section("Privacy");
        for (title, text) in [
            (
                "Audio stays here",
                "Speech is transcribed on this computer. Audio is never sent to an AI provider.",
            ),
            (
                "Nothing runs by itself",
                "Text goes to an AI provider only when you start an action.",
            ),
            (
                "Cloud providers ask first",
                "The first time a document or project goes to a cloud provider, Fennec asks and names \
                 where it goes. Your answer is remembered for that document and provider.",
            ),
            (
                "Local-only projects",
                "Projects marked Local only never go to a cloud provider, even if you said yes before. \
                 Set it in the project's panel.",
            ),
            (
                "Local models wait for dictation",
                "Jobs on a model running on this computer wait while you dictate, so dictation keeps up.",
            ),
        ] {
            let item = gtk::Box::new(gtk::Orientation::Vertical, 4);
            item.append(&label(title, &["fx-field-label"]));
            let t = label(text, &["fx-field-note"]);
            t.set_wrap(true);
            item.append(&t);
            b.append(&item);
        }
        let forget = gtk::Button::with_label("Forget my cloud confirmations");
        forget.add_css_class("fx-secondary");
        forget.set_halign(gtk::Align::Start);
        let status = label("", &["fx-field-note"]);
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
        b.append(&forget);
        b.append(&status);
        b
    }
}

fn section(title: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 16);
    b.add_css_class("fx-settings-section");
    b.append(&label(title, &["fx-project-title"]));
    b
}

fn field(name: &str, w: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&label(name, &["fx-field-label"]));
    b.append(w);
    b
}
