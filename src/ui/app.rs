//! The GTK application: one window, keyboard shortcuts.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use super::{Deps, MainWindow, build_window, load_css};

pub fn application(app_id: &str, deps: impl Fn() -> Deps + 'static) -> adw::Application {
    let app = adw::Application::builder().application_id(app_id).build();
    app.connect_startup(|app| {
        load_css();
        let quit = gio::SimpleAction::new("quit", None);
        let a = app.clone();
        quit.connect_activate(move |_, _| a.quit());
        app.add_action(&quit);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
    });
    // The one window; the application owns its controller for its lifetime.
    let current: Rc<RefCell<Option<Rc<MainWindow>>>> = Rc::default();
    app.connect_activate(move |app| {
        if let Some(win) = current.borrow().as_ref() {
            win.window.present();
            return;
        }
        let win = build_window(deps());
        win.window.set_application(Some(app));
        win.install_accels(app);
        win.window.present();
        *current.borrow_mut() = Some(win);
    });
    app
}
