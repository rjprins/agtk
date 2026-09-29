// The preview only uses part of the viewer API.
#[allow(dead_code)]
#[path = "../src/ui/diff_viewer.rs"]
mod diff_viewer;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use webkit6::prelude::*;
use diff_viewer::{DiffViewer, ViewerEvent};

fn main() -> glib::ExitCode {
    let display = std::env::var("AGMUX_TEST_DISPLAY")
        .expect("AGMUX_TEST_DISPLAY must name the private Wayland socket before GTK starts");
    assert!(
        PathBuf::from(display).is_absolute(),
        "AGMUX_TEST_DISPLAY must be an absolute path"
    );

    let app = adw::Application::builder()
        .application_id("nl.rutger.AgmuxNative.ChangesViewerPreview")
        .build();
    app.connect_activate(|app| {
        let viewer = Rc::new(DiffViewer::new(|event| match &event {
            ViewerEvent::Ready { .. } => println!("Changes viewer is ready"),
            ViewerEvent::DiffRendered {
                line_changes,
                character_changes,
                ..
            } => println!(
                "Diff rendered with {line_changes} line changes and {character_changes} character changes"
            ),
            ViewerEvent::ViewState { .. } => {}
            ViewerEvent::Error { message, .. } => eprintln!("Viewer error: {message}"),
        }));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Changes viewer preview")
            .default_width(1100)
            .default_height(760)
            .content(&viewer.widget())
            .build();
        viewer.set_appearance("light", "Monospace", 13);
        viewer
            .show_diff(&serde_json::json!({
                "requestId": "preview-request",
                "tabId": "preview-tab",
                "path": "src/lib.rs",
                "originalLabel": "commit 0123456",
                "modifiedLabel": "Working tree",
                "language": "rust",
                "original": "pub fn greet(name: &str) -> String {\n    format!(\"Hello, {name}!\")\n}\n",
                "modified": "pub fn greet(name: &str) -> String {\n    let note = \"</script> `quoted` <img src=x>\";\n    format!(\"Hi, {name}! {note}\")\n}\n",
            }))
            .expect("preview fixture should fit in the bridge limit");
        window.present();

        let viewer = viewer.clone();
        let app = app.clone();
        let capture_path = std::env::var_os("AGMUX_DIFF_CAPTURE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/agmux-diff-viewer-preview.png"));
        glib::timeout_add_local_once(Duration::from_secs(5), move || {
            // The app never captures the viewer, so the preview snapshots the WebView itself.
            let web_view = viewer
                .widget()
                .downcast::<webkit6::WebView>()
                .expect("the viewer widget is a WebView");
            web_view.snapshot(
                webkit6::SnapshotRegion::Visible,
                webkit6::SnapshotOptions::NONE,
                None::<&gio::Cancellable>,
                move |result| {
                    let result = result
                        .map_err(|error| error.to_string())
                        .and_then(|texture| {
                            texture
                                .save_to_png(&capture_path)
                                .map_err(|error| error.to_string())
                        });
                    match result {
                        Ok(()) => println!("Captured viewer to {}", capture_path.display()),
                        Err(error) => eprintln!("Could not capture viewer: {error}"),
                    }
                    app.quit();
                },
            );
        });
    });
    app.run()
}
