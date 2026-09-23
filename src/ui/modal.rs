use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;

/// A libadwaita dialog that keeps the small window-like API the workspace uses.
#[derive(Clone)]
pub(super) struct Modal {
    dialog: adw::Dialog,
    parent: adw::ApplicationWindow,
    header: adw::HeaderBar,
    toolbar: adw::ToolbarView,
    open: Rc<Cell<bool>>,
}

impl Modal {
    pub(super) fn new(
        parent: &adw::ApplicationWindow,
        title: &str,
        width: i32,
        height: i32,
        content: &impl IsA<gtk::Widget>,
    ) -> Self {
        let header = adw::HeaderBar::new();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(content));
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(width)
            .content_height(height)
            .child(&toolbar)
            .build();
        let open = Rc::new(Cell::new(false));
        let closed = open.clone();
        dialog.connect_closed(move |_| closed.set(false));
        Self {
            dialog,
            parent: parent.clone(),
            header,
            toolbar,
            open,
        }
    }

    pub(super) fn present(&self) {
        if !self.open.get() {
            self.open.set(true);
            self.dialog.present(Some(&self.parent));
        }
    }

    pub(super) fn hide(&self) {
        if self.open.get() {
            self.dialog.force_close();
        }
    }

    pub(super) fn is_visible(&self) -> bool {
        self.open.get()
    }

    /// Runs each time the dialog opens.
    pub(super) fn connect_show(&self, callback: impl Fn() + 'static) {
        self.dialog.connect_map(move |_| callback());
    }

    pub(super) fn connect_hide(&self, callback: impl Fn() + 'static) {
        self.dialog.connect_closed(move |_| callback());
    }

    pub(super) fn add_controller(&self, controller: impl IsA<gtk::EventController>) {
        self.dialog.add_controller(controller);
    }

    pub(super) fn header(&self) -> &adw::HeaderBar {
        &self.header
    }

    pub(super) fn add_bottom_bar(&self, bar: &impl IsA<gtk::Widget>) {
        self.toolbar.add_bottom_bar(bar);
    }

    pub(super) fn set_default_widget(&self, widget: &impl IsA<gtk::Widget>) {
        self.dialog.set_default_widget(Some(widget));
    }

    pub(super) fn set_focus(&self, widget: &impl IsA<gtk::Widget>) {
        self.dialog.set_focus(Some(widget));
    }
}
