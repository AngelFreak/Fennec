//! The GTK interface. Everything with side effects outside the app (the
//! speech engine, the microphone, voice detection) comes in through
//! [`Deps`], so tests can run the real window with fakes.

mod app;
mod dictation;
mod dock;
pub mod editor;
mod inspector;
mod sidebar;
mod window;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;

pub use app::application;
pub use dictation::DictationPage;
pub use window::MainWindow;

use crate::audio::capture::{AudioSource, MicSource};
use crate::config::{Backend, Paths, Settings};
use crate::engine::{Transcriber, WhisperEngine};
use crate::utterance::{EnergyVad, FrameVad, SileroFrameVad};

/// A replaceable callback slot on a widget controller.
pub(crate) type Handler<A> = RefCell<Option<Rc<dyn Fn(A)>>>;
pub(crate) type TextHandler = RefCell<Option<Rc<dyn Fn(&str)>>>;

pub type EngineFactory = Arc<dyn Fn(&Settings, &Paths) -> Result<Box<dyn Transcriber>, String> + Send + Sync>;
pub type AudioFactory = Arc<dyn Fn(&Settings) -> Result<Box<dyn AudioSource>, String> + Send + Sync>;
pub type VadFactory = Arc<dyn Fn(&Settings, &Paths) -> Result<Box<dyn FrameVad>, String> + Send + Sync>;

#[derive(Clone)]
pub struct Deps {
    pub paths: Paths,
    pub settings: Settings,
    pub engine: EngineFactory,
    pub audio: AudioFactory,
    pub vad: VadFactory,
}

impl Deps {
    /// The real engine, microphone and Silero VAD.
    pub fn real(paths: Paths, settings: Settings) -> Self {
        Self {
            paths,
            settings,
            engine: Arc::new(|s, p| {
                let gpu = s.backend != Backend::Cpu;
                WhisperEngine::load(&s.model_path(p), gpu)
                    .map(|e| Box::new(e) as Box<dyn Transcriber>)
                    .map_err(|e| e.to_string())
            }),
            audio: Arc::new(|s| {
                let device = (!s.microphone.is_empty()).then_some(s.microphone.as_str());
                MicSource::open(device)
                    .map(|m| Box::new(m) as Box<dyn AudioSource>)
                    .map_err(|e| e.to_string())
            }),
            vad: Arc::new(|s, p| match SileroFrameVad::load(&s.vad_path(p)) {
                Ok(v) => Ok(Box::new(v) as Box<dyn FrameVad>),
                Err(e) => {
                    tracing::warn!("{e}; using the loudness detector instead");
                    Ok(Box::new(EnergyVad::default()) as Box<dyn FrameVad>)
                }
            }),
        }
    }
}

pub fn build_window(deps: Deps) -> Rc<MainWindow> {
    MainWindow::new(deps)
}

const STYLE: &str = include_str!("style.css");
const STYLE_LIGHT: &str = include_str!("style-light.css");
const STYLE_DARK: &str = include_str!("style-dark.css");

/// Loads the stylesheet and the light or dark colour tokens, swapping the
/// tokens when the system switches.
pub fn load_css() {
    let display = gtk::gdk::Display::default().expect("a display is available");
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
