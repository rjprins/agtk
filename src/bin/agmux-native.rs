use adw::prelude::*;
use agmux_native::{APP_ID, ui};

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
        } else {
            ui::build(app);
        }
    });
    app.run()
}
