use gtk::prelude::*;

use fennec::config::{Paths, Settings};
use fennec::ui::{Deps, application};

fn main() -> gtk::glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("fennec=info,whisper_rs=warn")),
        )
        .init();
    let app = application(fennec::APP_ID, || {
        let paths = Paths::user();
        let settings = Settings::load(&paths.settings_file()).unwrap_or_else(|e| {
            tracing::error!("{e}; using default settings for this session");
            Settings::default()
        });
        Deps::real(paths, settings)
    });
    app.run()
}
