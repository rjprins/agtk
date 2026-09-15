use std::error::Error;
use std::time::Duration;

use adw::prelude::*;
use gtk::gdk::prelude::TextureExt;

use crate::capture::store_png_capture;
use crate::control::{CaptureResult, ControlResponse, ErrorCode, PendingRequest};

use super::Workspace;

pub(super) fn capture_controlled(workspace: Workspace, pending: PendingRequest) {
    // Let presentation and popover mapping reach the next frame before taking a
    // widget-only snapshot. This remains asynchronous, so GTK can render it.
    schedule_capture(workspace, pending, 0);
}

fn schedule_capture(workspace: Workspace, pending: PendingRequest, attempt: u8) {
    glib::timeout_add_local_once(Duration::from_millis(32), move || {
        let result = capture_workspace(&workspace)
            .and_then(|capture| serde_json::to_value(capture).map_err(Into::into));
        match result {
            Ok(capture) => {
                let request_id = pending.request.id.clone();
                let _ = pending.respond(ControlResponse::success(request_id, capture));
            }
            Err(_) if attempt < 5 => schedule_capture(workspace, pending, attempt + 1),
            Err(error) => {
                let request_id = pending.request.id.clone();
                let response = ControlResponse::failure(
                    request_id,
                    ErrorCode::InternalError,
                    "Could not capture application content",
                    Some(serde_json::json!({ "reason": error.to_string() })),
                );
                let _ = pending.respond(response);
            }
        }
    });
}

pub(super) fn capture_workspace(workspace: &Workspace) -> Result<CaptureResult, Box<dyn Error>> {
    let target = active_surface(workspace);
    let width = target.width();
    let height = target.height();
    if width <= 0 || height <= 0 {
        return Err("application content has no drawable size".into());
    }

    // WidgetPaintable snapshots only this GTK widget subtree, never desktop pixels.
    // Source: https://docs.gtk.org/gtk4/class.WidgetPaintable.html
    let paintable = gtk::WidgetPaintable::new(Some(&target));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot
        .to_node()
        .ok_or("GTK produced an empty render node")?;
    let viewport = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);

    // A dedicated Cairo renderer keeps capture deterministic when the live window uses
    // Vulkan or GL. GTK requires unrealize before the renderer is destroyed.
    // https://docs.gtk.org/gsk4/class.CairoRenderer.html
    // https://docs.gtk.org/gsk4/method.Renderer.realize.html
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize_for_display(&gtk::prelude::WidgetExt::display(&workspace.window))?;
    // GTK documents render_texture as rendering a scene graph into a GDK texture.
    // Source: https://docs.gtk.org/gsk4/method.Renderer.render_texture.html
    let texture = renderer.render_texture(&node, Some(&viewport));
    renderer.unrealize();
    // save_to_png_bytes is the in-memory counterpart of GTK's debugging and testing PNG API.
    // Source: https://docs.gtk.org/gdk4/method.Texture.save_to_png.html
    let png = texture.save_to_png_bytes();
    store_png_capture(
        &workspace.paths.captures_dir(),
        png.as_ref(),
        texture.width(),
        texture.height(),
    )
    .map_err(Into::into)
}

fn active_surface(workspace: &Workspace) -> gtk::Widget {
    for popover in [
        &workspace.launch_popover,
        &workspace.shortcut_popover,
        &workspace.theme_popover,
        &workspace.history_popover,
        &workspace.search_popover,
        &workspace.worktree_popover,
    ] {
        if popover.is_mapped() {
            return popover
                .child()
                .expect("visible agmux popover has capture content");
        }
    }
    workspace.overlay.clone().upcast()
}
