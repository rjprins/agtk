use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;

/// Share of the monitor a dialog may take before its content has to scroll.
const SCREEN_SHARE: f64 = 0.9;

/// A resizable modal window over the workspace that fits its content and remembers
/// the size the user drags it to.
#[derive(Clone)]
pub(super) struct Modal {
    window: adw::Window,
    parent: adw::ApplicationWindow,
    header: adw::HeaderBar,
    content: gtk::Widget,
    default_size: (i32, i32),
    /// The size the user chose, restored on every open.
    saved_size: Rc<Cell<Option<(i32, i32)>>>,
    /// The size last set by fitting; any other size on close came from the user.
    fitted_size: Rc<Cell<Option<(i32, i32)>>>,
    refit_queued: Rc<Cell<bool>>,
    on_resized: Rc<RefCell<Option<Box<dyn Fn(i32, i32)>>>>,
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
        let window = adw::Window::builder()
            .title(title)
            .modal(true)
            .transient_for(parent)
            .destroy_with_parent(true)
            .hide_on_close(true)
            .content(&toolbar)
            .build();
        if let Some(application) = parent.application() {
            window.set_application(Some(&application));
        }
        let escape = gtk::ShortcutController::new();
        escape.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::NamedAction::new("window.close")),
        ));
        window.add_controller(escape);

        let modal = Self {
            window,
            parent: parent.clone(),
            header,
            content: content.clone().upcast(),
            default_size: (width, height),
            saved_size: Rc::default(),
            fitted_size: Rc::default(),
            refit_queued: Rc::default(),
            on_resized: Rc::default(),
        };
        modal.follow_content_growth();
        let closing = modal.clone();
        modal.window.connect_visible_notify(move |window| {
            if !window.is_visible() {
                closing.remember_user_size();
            }
        });
        modal
    }

    /// The title doubles as the key for the remembered size.
    pub(super) fn title(&self) -> String {
        self.window.title().map(String::from).unwrap_or_default()
    }

    pub(super) fn window(&self) -> &adw::Window {
        &self.window
    }

    pub(super) fn set_saved_size(&self, size: Option<(i32, i32)>) {
        self.saved_size.set(size.filter(|(w, h)| *w > 0 && *h > 0));
    }

    pub(super) fn connect_resized(&self, callback: impl Fn(i32, i32) + 'static) {
        *self.on_resized.borrow_mut() = Some(Box::new(callback));
    }

    pub(super) fn present(&self) {
        if self.window.is_visible() {
            return;
        }
        let (width, height) = self.saved_size.get().unwrap_or(self.default_size);
        let (max_width, max_height) = self.screen_limit();
        let width = width.min(max_width);
        let height = if self.saved_size.get().is_some() {
            height
        } else {
            height.max(self.natural_height(width))
        }
        .min(max_height);
        self.window.set_default_size(width, height);
        self.fitted_size.set(Some((width, height)));
        self.window.present();
    }

    pub(super) fn hide(&self) {
        self.window.set_visible(false);
    }

    pub(super) fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    /// Runs each time the dialog opens.
    pub(super) fn connect_show(&self, callback: impl Fn() + 'static) {
        self.window.connect_map(move |_| callback());
    }

    pub(super) fn connect_hide(&self, callback: impl Fn() + 'static) {
        self.window.connect_visible_notify(move |window| {
            if !window.is_visible() {
                callback();
            }
        });
    }

    pub(super) fn add_controller(&self, controller: impl IsA<gtk::EventController>) {
        self.window.add_controller(controller);
    }

    pub(super) fn header(&self) -> &adw::HeaderBar {
        &self.header
    }

    pub(super) fn set_default_widget(&self, widget: &impl IsA<gtk::Widget>) {
        self.window.set_default_widget(Some(widget));
    }

    fn remember_user_size(&self) {
        let (width, height) = self.window.default_size();
        if width <= 0 || height <= 0 || Some((width, height)) == self.fitted_size.get() {
            return;
        }
        self.saved_size.set(Some((width, height)));
        if let Some(callback) = self.on_resized.borrow().as_ref() {
            callback(width, height);
        }
    }

    /// Scrolled areas report their full content height, so fitting sees everything.
    fn follow_content_growth(&self) {
        for scroller in scrolled_windows(&self.content) {
            scroller.set_propagate_natural_height(true);
            let modal = self.clone();
            scroller
                .vadjustment()
                .connect_upper_notify(move |_| modal.queue_refit());
        }
    }

    /// Grows an open, not user-sized dialog when content arrives after it opened.
    fn queue_refit(&self) {
        if !self.window.is_visible() || self.saved_size.get().is_some() || self.refit_queued.get() {
            return;
        }
        self.refit_queued.set(true);
        let modal = self.clone();
        glib::idle_add_local_once(move || {
            modal.refit_queued.set(false);
            let current = modal.window.default_size();
            if Some(current) != modal.fitted_size.get() {
                return;
            }
            let (width, height) = current;
            let (_, max_height) = modal.screen_limit();
            let wanted = modal.natural_height(width).min(max_height);
            if wanted > height {
                modal.window.set_default_size(width, wanted);
                modal.fitted_size.set(Some((width, wanted)));
            }
        });
    }

    fn natural_height(&self, width: i32) -> i32 {
        let Some(child) = self.window.content() else {
            return 0;
        };
        child.measure(gtk::Orientation::Vertical, width).1
    }

    fn screen_limit(&self) -> (i32, i32) {
        let display = gtk::prelude::WidgetExt::display(&self.parent);
        let monitor = self
            .parent
            .surface()
            .and_then(|surface| display.monitor_at_surface(&surface));
        let (width, height) = match monitor {
            Some(monitor) => {
                let geometry = monitor.geometry();
                (geometry.width(), geometry.height())
            }
            None => (self.parent.width(), self.parent.height()),
        };
        (
            (f64::from(width) * SCREEN_SHARE) as i32,
            (f64::from(height) * SCREEN_SHARE) as i32,
        )
    }
}

fn scrolled_windows(root: &gtk::Widget) -> Vec<gtk::ScrolledWindow> {
    let mut found = Vec::new();
    if let Some(scroller) = root.downcast_ref::<gtk::ScrolledWindow>() {
        found.push(scroller.clone());
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        found.extend(scrolled_windows(&widget));
        child = widget.next_sibling();
    }
    found
}
