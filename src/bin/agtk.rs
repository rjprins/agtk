use adw::prelude::*;
use agtk::instance::{InstanceName, InstancePaths};
use agtk::ui;
use std::os::unix::process::CommandExt;

fn main() -> glib::ExitCode {
    let instance = match InstanceName::from_environment() {
        Ok(instance) => instance,
        Err(error) => {
            eprintln!("Invalid AGTK_INSTANCE: {error}");
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
            // Reactivation brings back the workspace, not a leftover dialog.
            for _ in 0..16 {
                let Some(dialog) = window.visible_dialog() else {
                    break;
                };
                dialog.force_close();
            }
            window.present();
        } else {
            ui::build(app, paths.clone());
        }
    });
    let exit_code = app.run();
    if let Some(executable) = ui::take_restart_executable() {
        // exec keeps the environment and PID; independent session hosts keep running.
        let error = std::process::Command::new(executable)
            .args(std::env::args_os().skip(1))
            .exec();
        eprintln!("Could not restart agtk: {error}");
        return glib::ExitCode::FAILURE;
    }
    exit_code
}
