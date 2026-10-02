//! Running AI actions from the UI: each job runs on a worker thread with
//! its own store connection and reports back on the main loop. A first
//! cloud send asks the user (through `Deps::confirm_cloud`) and then reruns.

use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;

use super::Deps;
use crate::ai::service::Scope;
use crate::ai::{AiError, AiService, Cancel, privacy::PrivacyError};
use crate::store::Store;

/// What the confirmation dialog tells the user.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudSend {
    pub provider: String,
    pub host: String,
    pub what: String,
}

pub type Job<T> =
    Arc<dyn Fn(&AiService, &Store, &Cancel, &mut dyn FnMut(&str)) -> Result<T, AiError> + Send + Sync>;

enum Msg<T> {
    Delta(String),
    Done(Result<T, AiError>),
}

/// The real confirmation: a dialog naming where the text goes.
pub fn confirm_with_dialog(parent: &gtk::Widget, send: &CloudSend, answer: Box<dyn FnOnce(bool)>) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Send to {}?", send.provider)),
        Some(&format!(
            "The text of {} will be sent to {} ({}) for this and later AI actions. \
             Audio never leaves this computer.",
            send.what, send.provider, send.host
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("send", "Send");
    dialog.set_response_appearance("send", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let answer = std::cell::Cell::new(Some(answer));
    dialog.connect_response(None, move |_, response| {
        if let Some(f) = answer.take() {
            f(response == "send");
        }
    });
    dialog.present(Some(parent));
}

/// Runs `job` for `scope`. `on_delta` gets streamed text; `on_done` the
/// result. Returns the job's cancel flag.
pub fn run<T: Send + 'static>(
    parent: &gtk::Widget,
    deps: &Deps,
    scope: Scope,
    what: String,
    job: Job<T>,
    on_delta: Rc<dyn Fn(&str)>,
    on_done: Rc<dyn Fn(Result<T, AiError>)>,
) -> Arc<Cancel> {
    let cancel = Arc::new(Cancel::default());
    let service = deps.ai_service();
    let db = deps.paths.database();
    let (tx, rx) = async_channel::unbounded::<Msg<T>>();
    {
        let cancel = Arc::clone(&cancel);
        let job = Arc::clone(&job);
        let scope = scope.clone();
        std::thread::spawn(move || {
            let result = Store::open(&db).map_err(AiError::from).and_then(|store| {
                let delta_tx = tx.clone();
                job(&service, &store, &cancel, &mut |d| {
                    let _ = delta_tx.send_blocking(Msg::Delta(d.to_string()));
                })
            });
            if let Err(e) = &result {
                tracing::warn!(?scope, "AI job failed: {e}");
            }
            let _ = tx.send_blocking(Msg::Done(result));
        });
    }
    let parent = parent.clone();
    let deps = deps.clone();
    let returned = Arc::clone(&cancel);
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                Msg::Delta(d) => on_delta(&d),
                Msg::Done(Err(AiError::Privacy(PrivacyError::NeedsConsent { provider }))) => {
                    ask_consent(
                        &parent,
                        &deps,
                        scope.clone(),
                        what.clone(),
                        provider,
                        job.clone(),
                        on_delta.clone(),
                        on_done.clone(),
                    );
                    break;
                }
                Msg::Done(result) => {
                    on_done(result);
                    break;
                }
            }
        }
    });
    returned
}

#[allow(clippy::too_many_arguments)]
fn ask_consent<T: Send + 'static>(
    parent: &gtk::Widget,
    deps: &Deps,
    scope: Scope,
    what: String,
    provider: String,
    job: Job<T>,
    on_delta: Rc<dyn Fn(&str)>,
    on_done: Rc<dyn Fn(Result<T, AiError>)>,
) {
    let host = deps
        .settings()
        .ai
        .active()
        .map(|p| host_of(&p.base_url))
        .unwrap_or_default();
    let send = CloudSend {
        provider: provider.clone(),
        host,
        what: what.clone(),
    };
    let parent2 = parent.clone();
    let deps2 = deps.clone();
    (deps.confirm_cloud)(
        parent,
        &send,
        Box::new(move |yes| {
            if !yes {
                on_done(Err(AiError::Cancelled));
                return;
            }
            let recorded = Store::open(&deps2.paths.database())
                .map_err(AiError::from)
                .and_then(|store| deps2.ai_service().consent(&store, &scope));
            match recorded {
                Ok(()) => {
                    run(&parent2, &deps2, scope, what, job, on_delta, on_done);
                }
                Err(e) => on_done(Err(e)),
            }
        }),
    );
}

/// `api.anthropic.com` from `https://api.anthropic.com/v1`.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest).to_string()
}

/// One line for an error shown next to an action.
pub fn error_text(e: &AiError) -> String {
    match e {
        AiError::Cancelled => "Cancelled.".into(),
        e => {
            let s = e.to_string();
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str() + ".",
                None => s,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_come_from_base_urls() {
        assert_eq!(host_of("https://api.anthropic.com"), "api.anthropic.com");
        assert_eq!(host_of("http://192.168.1.10:11434/v1"), "192.168.1.10:11434");
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(error_text(&AiError::Disabled), "AI is turned off in Settings.");
    }
}
