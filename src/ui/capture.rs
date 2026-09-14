use std::error::Error;

use adw::prelude::*;
use gtk::gdk::prelude::TextureExt;

use crate::capture::store_png_capture;
use crate::control::CaptureResult;

use super::Workspace;

pub(super) fn capture_workspace(workspace: &Workspace) -> Result<CaptureResult, Box<dyn Error>> {
    let width = workspace.overlay.width();
    let height = workspace.overlay.height();
    if width <= 0 || height <= 0 {
        return Err("application content has no drawable size".into());
    }

    // WidgetPaintable snapshots only this GTK widget subtree, never desktop pixels.
    // Source: https://docs.gtk.org/gtk4/class.WidgetPaintable.html
    let paintable = gtk::WidgetPaintable::new(Some(&workspace.overlay));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot
        .to_node()
        .ok_or("GTK produced an empty render node")?;
    let renderer = workspace
        .window
        .renderer()
        .ok_or("application window has no renderer")?;
    let viewport = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);

    // GTK documents render_texture as rendering a scene graph into a GDK texture.
    // Source: https://docs.gtk.org/gsk4/method.Renderer.render_texture.html
    let texture = renderer.render_texture(&node, Some(&viewport));
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
