use super::*;
use crate::rebuild::RebuildPlan;

thread_local! {
    static RESTART_EXECUTABLE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Called by the executable after GTK releases this instance's application name.
pub fn take_restart_executable() -> Option<PathBuf> {
    RESTART_EXECUTABLE.with(|path| path.borrow_mut().take())
}

impl Workspace {
    pub(super) fn rebuild_and_restart(&self) {
        let plan = std::env::current_exe()
            .map_err(|error| error.to_string())
            .and_then(|executable| {
                RebuildPlan::new(Path::new(env!("CARGO_MANIFEST_DIR")), &executable)
            });
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                self.show_error(&error);
                return;
            }
        };
        self.menus.rebuild.set_enabled(false);
        let progress =
            plain_toast("Rebuilding agtk… The window will restart when the build succeeds.");
        progress.set_timeout(0);
        self.overlay.add_toast(progress.clone());
        // The desktop's PATH may omit rustup's Cargo, even though the checkout builds with it.
        let cargo = std::env::var_os("CARGO")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".cargo/bin/cargo"))
                    .filter(|path| path.is_file())
            })
            .unwrap_or_else(|| PathBuf::from("cargo"));
        self.run_slow(
            move || plan.run(&cargo).map_err(Into::into),
            move |workspace, result| {
                progress.dismiss();
                match result {
                    Ok(executable) => {
                        // Let queued settings and session writes finish before replacing the window.
                        workspace.run_io(
                            || Ok(()),
                            move |workspace, result| {
                                if let Err(error) = result {
                                    workspace.menus.rebuild.set_enabled(true);
                                    workspace
                                        .show_error(&format!("Could not prepare restart: {error}"));
                                    return;
                                }
                                workspace.save_window_size(true);
                                // Stop the old listener before the replacement binds this instance's socket.
                                workspace.control_server.borrow_mut().take();
                                RESTART_EXECUTABLE
                                    .with(|path| *path.borrow_mut() = Some(executable));
                                workspace.application.quit();
                            },
                        );
                    }
                    Err(error) => {
                        workspace.menus.rebuild.set_enabled(true);
                        let log = gtk::TextView::builder()
                            .editable(false)
                            .cursor_visible(false)
                            .monospace(true)
                            .wrap_mode(gtk::WrapMode::WordChar)
                            .build();
                        log.buffer().set_text(&error.to_string());
                        let scroll = gtk::ScrolledWindow::builder()
                            .min_content_height(220)
                            .max_content_height(420)
                            .min_content_width(560)
                            .hscrollbar_policy(gtk::PolicyType::Never)
                            .child(&log)
                            .build();
                        let dialog = adw::AlertDialog::builder()
                            .heading("Could not rebuild agtk")
                            .body("The window and agent sessions are still running.")
                            .extra_child(&scroll)
                            .build();
                        dialog.add_response("close", "Close");
                        dialog.set_default_response(Some("close"));
                        dialog.set_close_response("close");
                        dialog.present(Some(&workspace.window));
                    }
                }
            },
        );
    }
}
