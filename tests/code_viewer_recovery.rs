//! Requires a private compositor and WAYLAND_DISPLAY=$AGTK_TEST_DISPLAY.
#[allow(dead_code)]
#[path = "../src/ui/code_viewer.rs"]
mod code_viewer;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use agtk::{changes, language_server};
use code_viewer::{CodeViewer, ViewerEvent};
use gtk::prelude::*;
use serde_json::json;
use webkit6::prelude::*;

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn shows_the_document_again_after_the_web_process_stops() {
    let display = std::env::var("AGTK_TEST_DISPLAY").expect("private display required");
    assert!(std::path::Path::new(&display).is_absolute());
    assert_eq!(std::env::var("WAYLAND_DISPLAY").unwrap(), display);
    gtk::init().unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    let viewer = CodeViewer::new(move |event| received.borrow_mut().push(event));
    let window = gtk::Window::builder()
        .default_width(400)
        .default_height(300)
        .child(&viewer.widget())
        .build();
    let web_view = viewer.widget().downcast::<webkit6::WebView>().unwrap();
    let show = |request_id: &str| {
        viewer
            .show_file(&json!({
                "requestId": request_id, "tabId": "tab", "path": "main.rs", "text": "fn main() {}",
            }))
            .unwrap();
    };
    let rendered = |request: &str| {
        events
            .borrow()
            .iter()
            .filter(|event| {
                matches!(event, ViewerEvent::FileRendered { request_id, .. } if request_id == request)
            })
            .count()
    };
    let stopped = |reload: bool| {
        events
            .borrow()
            .iter()
            .filter(
                |event| matches!(event, ViewerEvent::Stopped { reloading } if *reloading == reload),
            )
            .count()
    };
    let wait_for = |what: &str, done: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        glib::MainContext::default().block_on(async {
            while !done() {
                assert!(Instant::now() < deadline, "{what}: {:?}", events.borrow());
                glib::timeout_future(Duration::from_millis(20)).await;
            }
        });
    };
    show("first");
    window.present();
    wait_for("first render", &|| rendered("first") == 1);

    web_view.terminate_web_process();
    wait_for("render after reload", &|| rendered("first") == 2);
    assert_eq!((stopped(true), stopped(false)), (1, 0));

    // Stopping again this soon leaves the page down until something new shows.
    web_view.terminate_web_process();
    wait_for("staying down", &|| stopped(false) == 1);
    glib::MainContext::default().block_on(glib::timeout_future(Duration::from_millis(500)));
    assert_eq!(rendered("first"), 2);

    show("second");
    wait_for("render after retry", &|| rendered("second") == 1);
    window.close();
}
