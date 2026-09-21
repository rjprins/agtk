use adw::prelude::*;
use agmux_native::instance::{InstanceName, InstancePaths};
use agmux_native::ui;

fn main() -> glib::ExitCode {
    let instance = match InstanceName::from_environment() {
        Ok(instance) => instance,
        Err(error) => {
            eprintln!("Invalid AGMUX_INSTANCE: {error}");
            return glib::ExitCode::FAILURE;
        }
    };
    let paths = InstancePaths::from_environment(instance.clone());
    let app = adw::Application::builder()
        .application_id(instance.application_id())
        .build();
    app.connect_activate(move |app| {
        let mut main_window = None;
        for window in app.windows() {
            if window.is::<adw::ApplicationWindow>() {
                main_window = Some(
                    window
                        .downcast::<adw::ApplicationWindow>()
                        .expect("window type checked before downcast"),
                );
            } else {
                window.set_visible(false);
            }
        }
        if let Some(window) = main_window {
            window.present();
        } else {
            ui::build(app, paths.clone());
        }
    });
    app.run()
}
