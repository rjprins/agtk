//! Requires a private compositor and WAYLAND_DISPLAY=$AGTK_TEST_DISPLAY.
#[allow(dead_code)]
#[path = "../src/ui/code_viewer.rs"]
mod code_viewer;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use agtk::changes;
use code_viewer::{CodeViewer, ViewerEvent};
use gtk::prelude::*;
use serde_json::json;
use webkit6::prelude::*;

#[test]
#[ignore = "requires AGTK_TEST_DISPLAY private Wayland compositor"]
fn images_render_resize_restore_and_switch_in_webkit() {
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
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("galaxy.png");
    let changes::FileDocumentResult::Image(document) =
        changes::read_file_document(&path, "galaxy.png").unwrap()
    else {
        panic!("expected image");
    };
    // Also exercises the command queue before the page is ready.
    viewer
        .show_file(&json!({
            "requestId": "image-request", "tabId": "image-tab",
            "path": document.path, "dataUrl": document.data_url,
        }))
        .unwrap();
    window.present();
    glib::MainContext::default().block_on(async {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !events.borrow().iter().any(|event| matches!(event,
            ViewerEvent::ImageRendered { width: 512, height: 512, .. })) {
            assert!(Instant::now() < deadline, "image did not render: {:?}", events.borrow());
            glib::timeout_future(Duration::from_millis(20)).await;
        }
        web_view.call_async_javascript_function_future(r#"
            const check = (value, message) => { if (!value) throw new Error(message); };
            const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
            const root = document.querySelector('#image-view');
            const viewport = document.querySelector('#image-viewport');
            const fit = document.querySelector('#image-fit');
            const actual = document.querySelector('#image-actual');
            let image = viewport.querySelector('img');
            check(!root.hidden && !image.hidden, 'image visible');
            check(image.clientWidth <= viewport.clientWidth && image.clientHeight <= viewport.clientHeight, 'fit within viewport');
            actual.focus();
            check(document.activeElement === actual, 'size controls focusable');
            actual.click();
            await frame();
            check(image.clientWidth === 512 && image.clientHeight === 512, 'actual size');
            check(viewport.scrollWidth > viewport.clientWidth, 'actual size scrolls');
            viewport.scrollLeft = 40;
            viewport.scrollTop = 50;
            const state = window.agtkViewer.saveViewState();
            check(state.agtkImage.actualSize && state.agtkImage.left === 40, 'save size and scroll');
            const dataUrl = image.src;
            window.agtkViewer.showFile({ tabId: 'restored', path: 'restored.png', dataUrl, viewState: state });
            image = viewport.querySelector('img');
            await image.decode();
            await frame();
            check(viewport.scrollLeft === 40 && viewport.scrollTop === 50, 'restore scroll');
            fit.click();
            await frame();
            check(fit.getAttribute('aria-pressed') === 'true', 'fit selected');
            check(image.clientHeight <= viewport.clientHeight, 'fit after actual size');
            window.agtkViewer.showFile({ tabId: 'text', path: 'main.rs', text: 'fn main() {}' });
            check(root.hidden && !viewport.querySelector('img'), 'text disposes image');
            window.agtkViewer.showFile({ tabId: 'markdown', path: 'readme.md', text: '# Hello', presentation: 'preview' });
            check(root.hidden && document.querySelector('#markdown').classList.contains('visible'), 'markdown replaces image');
            const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><rect width="1600" height="900" fill="red"/><script>parent.svgExecuted = true</script></svg>';
            window.agtkViewer.showFile({ tabId: 'svg', path: 'drawing.svg', dataUrl: 'data:image/svg+xml;base64,' + btoa(svg) });
            image = viewport.querySelector('img');
            await image.decode();
            await frame();
            check(document.querySelector('#image-status').textContent === '1600 × 900 px', 'SVG intrinsic dimensions');
            check(!window.svgExecuted, 'SVG scripts disabled');
            actual.click();
            await frame();
            check(image.clientWidth === 1600 && image.clientHeight === 900, 'SVG actual size');
            check(!document.querySelector('#markdown').classList.contains('visible'), 'image replaces markdown');
            window.agtkViewer.showFile({ tabId: 'bad', path: 'bad.png', dataUrl: 'data:image/png;base64,YQ==' });
            try { await viewport.querySelector('img').decode(); } catch {}
            await new Promise(resolve => setTimeout(resolve, 50));
            check(document.querySelector('#image-status').textContent.includes('Could not display'), 'decode error shown');
            check(actual.disabled && fit.disabled, 'controls disabled on error');
            window.agtkViewer.showFile({ tabId: 'pending', path: 'pending.png', dataUrl });
            window.agtkViewer.showDiff({ tabId: 'diff', path: 'main.rs', original: '', modified: 'hello' });
            await frame();
            check(root.hidden && !viewport.querySelector('img'), 'pending image cannot replace diff');
            check(document.querySelector('#editor').classList.contains('visible'), 'diff visible');
            return true;
        "#, None, None, None).await.expect("image viewer interactions must succeed");
    });
    window.close();
}
