//! The GTK interface. Everything with side effects outside the app (the
//! speech engine, the microphone, voice detection) comes in through
//! [`Deps`], so tests can run the real window with fakes.

mod ai;
mod ai_panels;
mod app;
mod cleanup_page;
mod dictation;
mod dock;
pub mod editor;
mod engine;
mod export_page;
mod files;
mod inspector;
mod mic_test;
pub mod project;
mod settings_ai;
mod settings_page;
pub mod sidebar;
mod templates_page;
mod window;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gtk::prelude::*;

pub use ai::CloudSend;
pub use app::application;
pub use dictation::DictationPage;
pub use export_page::ExportPage;
pub use files::FilesPage;
pub use sidebar::Nav;
pub use window::MainWindow;

use crate::audio::capture::{AudioSource, MicSource};
use crate::config::{Paths, Settings};
use crate::engine::Transcriber;
use crate::utterance::{EnergyVad, FrameVad, SileroFrameVad};
use crate::vad::{SileroVad, SpeechDetector, WholeAudio};

/// A replaceable callback slot on a widget controller.
pub(crate) type Handler<A> = RefCell<Option<Rc<dyn Fn(A)>>>;

/// A button with an icon before its label, as the mockup's header buttons.
pub(crate) fn icon_text_button(icon: &str, text: &str, classes: &[&str]) -> gtk::Button {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.append(&gtk::Image::from_icon_name(icon));
    b.append(&gtk::Label::new(Some(text)));
    let button = gtk::Button::builder().child(&b).build();
    for c in classes {
        button.add_css_class(c);
    }
    button
}
pub(crate) type TextHandler = RefCell<Option<Rc<dyn Fn(&str)>>>;

pub type EngineFactory = Arc<dyn Fn(&Settings, &Paths) -> Result<Box<dyn Transcriber>, String> + Send + Sync>;
pub type AudioFactory = Arc<dyn Fn(&Settings) -> Result<Box<dyn AudioSource>, String> + Send + Sync>;
pub type FileVadFactory = Arc<dyn Fn(&Settings, &Paths) -> Box<dyn SpeechDetector> + Send + Sync>;
/// Asks whether text may go to a cloud provider; answers through the callback.
pub type ConfirmCloud = Rc<dyn Fn(&gtk::Widget, &CloudSend, Box<dyn FnOnce(bool)>)>;
pub type VadFactory = Arc<dyn Fn(&Settings, &Paths) -> Result<Box<dyn FrameVad>, String> + Send + Sync>;

#[derive(Clone)]
pub struct Deps {
    pub paths: Paths,
    /// Shared by every screen; Settings edits it and saves it to disk.
    pub settings: Rc<RefCell<Settings>>,
    pub engine: EngineFactory,
    pub audio: AudioFactory,
    pub vad: VadFactory,
    /// Voice detection for whole files (import).
    pub file_vad: FileVadFactory,
    /// Where AI provider API keys live.
    pub secrets: Arc<dyn crate::ai::SecretStore>,
    pub confirm_cloud: ConfirmCloud,
    /// True while dictation runs (local AI models wait for it).
    pub dictation_live: Arc<AtomicBool>,
}

impl Deps {
    /// The real engine, microphone and Silero VAD.
    pub fn real(paths: Paths, settings: Settings) -> Self {
        Self {
            paths,
            settings: Rc::new(RefCell::new(settings)),
            engine: Arc::new(|s, p| {
                let path = s.model_path(p);
                let gpu = crate::models::wants_gpu(s.backend);
                crate::engine::load_engine(&path, gpu)
                    .or_else(|e| {
                        if !gpu || crate::engine::sidecar::is_sidecar_model(&path) {
                            return Err(e);
                        }
                        // A GPU that fails to start must not stop dictation.
                        tracing::warn!("GPU backend failed ({e}); falling back to CPU");
                        crate::engine::load_engine(&path, false)
                    })
                    .map_err(|e| e.to_string())
            }),
            audio: Arc::new(|s| {
                let device = (!s.microphone.is_empty()).then_some(s.microphone.as_str());
                MicSource::open(device, s.input_gain_db)
                    .map(|m| Box::new(m) as Box<dyn AudioSource>)
                    .map_err(|e| e.to_string())
            }),
            file_vad: Arc::new(|s, p| match SileroVad::load(&s.vad_path(p), 2) {
                Ok(v) => Box::new(v) as Box<dyn SpeechDetector>,
                Err(e) => {
                    tracing::warn!("{e}; files will be cut into fixed chunks");
                    Box::new(WholeAudio)
                }
            }),
            vad: Arc::new(|s, p| match SileroFrameVad::load(&s.vad_path(p)) {
                Ok(v) => Ok(Box::new(v) as Box<dyn FrameVad>),
                Err(e) => {
                    tracing::warn!("{e}; using the loudness detector instead");
                    Ok(Box::new(EnergyVad::default()) as Box<dyn FrameVad>)
                }
            }),
            secrets: Arc::new(crate::ai::Keyring),
            confirm_cloud: Rc::new(ai::confirm_with_dialog),
            dictation_live: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Deps {
    /// A copy of the current settings (for threads and short reads).
    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    /// The AI service for the current settings.
    pub fn ai_service(&self) -> crate::ai::AiService {
        crate::ai::AiService::new(self.settings.borrow().ai.clone(), Arc::clone(&self.secrets))
            .with_dictation_flag(Arc::clone(&self.dictation_live))
    }

    /// Writes the current settings to `settings.toml`.
    pub fn save_settings(&self) -> Result<(), String> {
        self.settings
            .borrow()
            .save(&self.paths.settings_file())
            .map_err(|e| e.to_string())
    }
}

pub fn build_window(deps: Deps) -> Rc<MainWindow> {
    MainWindow::new(deps)
}

const STYLE: &str = concat!(
    include_str!("style.css"),
    include_str!("css/files.css"),
    include_str!("css/export.css"),
    include_str!("css/project.css"),
    include_str!("css/cleanup.css"),
    include_str!("css/templates.css"),
    include_str!("css/settings.css"),
);
const STYLE_LIGHT: &str = include_str!("style-light.css");
const STYLE_DARK: &str = include_str!("style-dark.css");

/// Registers the bundled icons (`fennec-*-symbolic`) with the icon theme.
fn load_icons(display: &gtk::gdk::Display) {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        gtk::gio::resources_register_include!("icons.gresource").expect("the bundled icons are valid");
    });
    gtk::IconTheme::for_display(display).add_resource_path("/io/github/fennec/Fennec/icons");
}

/// Loads the stylesheet and the light or dark colour tokens, swapping the
/// tokens when the system switches.
pub fn load_css() {
    let display = gtk::gdk::Display::default().expect("a display is available");
    load_icons(&display);
    let base = gtk::CssProvider::new();
    base.load_from_string(STYLE);
    let tokens = gtk::CssProvider::new();
    let manager = adw::StyleManager::default();
    let load_tokens = {
        let tokens = tokens.clone();
        move |m: &adw::StyleManager| {
            tokens.load_from_string(if m.is_dark() { STYLE_DARK } else { STYLE_LIGHT })
        }
    };
    load_tokens(&manager);
    manager.connect_dark_notify(load_tokens);
    for provider in [&tokens, &base] {
        gtk::style_context_add_provider_for_display(
            &display,
            provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Parse errors in the bundled stylesheets (for tests).
pub fn css_errors() -> Vec<String> {
    let errors = Rc::new(RefCell::new(Vec::new()));
    for (file, css) in [
        ("style.css", STYLE),
        ("style-light.css", STYLE_LIGHT),
        ("style-dark.css", STYLE_DARK),
    ] {
        let provider = gtk::CssProvider::new();
        let sink = errors.clone();
        provider.connect_parsing_error(move |_, section, error| {
            sink.borrow_mut()
                .push(format!("{file}: {section}: {}", error.message()));
        });
        // Tokens are referenced across files, so check each together with the tokens.
        provider.load_from_string(&format!("{STYLE_LIGHT}\n{css}"));
    }
    errors.take()
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn icon_button(icon: &str, tooltip: &str, classes: &[&str]) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    b.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

/// Collects every label's text under `widget` (tests and accessibility checks).
pub fn texts_in(widget: &impl IsA<gtk::Widget>) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![widget.clone().upcast::<gtk::Widget>()];
    while let Some(w) = stack.pop() {
        if let Some(l) = w.downcast_ref::<gtk::Label>() {
            out.push(l.text().to_string());
        }
        let mut child = w.last_child();
        while let Some(c) = child {
            child = c.prev_sibling();
            stack.push(c);
        }
    }
    out
}
