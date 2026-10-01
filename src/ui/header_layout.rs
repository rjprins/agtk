//! Lays out the content header: the tab strip at the start and the session
//! title centred in the row, until the tabs push it aside and finally onto a
//! second line.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

const SPACING: i32 = 8;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct HeaderLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for HeaderLayout {
        const NAME: &'static str = "AgtkHeaderLayout";
        type Type = super::HeaderLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for HeaderLayout {}

    impl LayoutManagerImpl for HeaderLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let Some((tabs, title)) = pair(widget) else {
                return match widget.first_child() {
                    Some(child) => child.measure(orientation, for_size),
                    None => (0, 0, -1, -1),
                };
            };
            let (minimum, natural) = if orientation == gtk::Orientation::Horizontal {
                let (tabs_min, tabs_nat) = width(&tabs);
                let (title_min, title_nat) = width(&title);
                // The header bar centres this widget at its natural width, so
                // claim the tabs' width on both sides of the title to keep the
                // title itself in the middle of the bar.
                (
                    tabs_min.max(title_min),
                    title_nat + 2 * (tabs_nat + SPACING),
                )
            } else {
                let (tabs_min, tabs_nat, _, _) = tabs.measure(gtk::Orientation::Vertical, -1);
                let (title_min, title_nat, _, _) = title.measure(gtk::Orientation::Vertical, -1);
                if for_size < 0 || fits_one_row(&tabs, &title, for_size) {
                    (tabs_min.max(title_min), tabs_nat.max(title_nat))
                } else {
                    (
                        tabs_min + SPACING + title_min,
                        tabs_nat + SPACING + title_nat,
                    )
                }
            };
            (minimum, natural, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let Some((tabs, title)) = pair(widget) else {
                if let Some(child) = widget.first_child() {
                    place(&child, 0, 0, width, height);
                }
                return;
            };
            let (_, tabs_natural) = super::width(&tabs);
            let (_, title_natural) = super::width(&title);
            let title_width = title_natural.min(width);
            if fits_one_row(&tabs, &title, width) {
                // Centre the title unless the tabs reach into that space.
                let title_x = ((width - title_width) / 2).max(tabs_natural + SPACING);
                place(&tabs, 0, 0, title_x - SPACING, height);
                place(&title, title_x, 0, title_width, height);
            } else {
                let (_, tabs_height, _, _) = tabs.measure(gtk::Orientation::Vertical, width);
                place(&tabs, 0, 0, width, tabs_height);
                let title_y = tabs_height + SPACING;
                place(
                    &title,
                    (width - title_width) / 2,
                    title_y,
                    title_width,
                    height - title_y,
                );
            }
        }
    }
}

glib::wrapper! {
    pub struct HeaderLayout(ObjectSubclass<imp::HeaderLayout>) @extends gtk::LayoutManager;
}

impl HeaderLayout {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

/// The tab strip and the title, when both take part in the layout.
fn pair(widget: &gtk::Widget) -> Option<(gtk::Widget, gtk::Widget)> {
    let mut children = Vec::new();
    let mut child = widget.first_child();
    while let Some(current) = child {
        if current.should_layout() {
            children.push(current.clone());
        }
        child = current.next_sibling();
    }
    match children.as_slice() {
        [tabs, title] => Some((tabs.clone(), title.clone())),
        _ => None,
    }
}

fn width(child: &gtk::Widget) -> (i32, i32) {
    let (minimum, natural, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
    (minimum, natural)
}

fn fits_one_row(tabs: &gtk::Widget, title: &gtk::Widget, for_size: i32) -> bool {
    width(tabs).1 + SPACING + width(title).1 <= for_size
}

fn place(child: &gtk::Widget, x: i32, y: i32, width: i32, height: i32) {
    child.size_allocate(&gtk::Allocation::new(x, y, width.max(0), height.max(0)), -1);
}
