//! Keep session usage under the terminal, with quota and system load at the edges.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct StatusBarLayout {
        pub center: std::cell::Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StatusBarLayout {
        const NAME: &'static str = "AgtkStatusBarLayout";
        type Type = super::StatusBarLayout;
        type ParentType = gtk::LayoutManager;
    }
    impl ObjectImpl for StatusBarLayout {}
    impl LayoutManagerImpl for StatusBarLayout {
        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let mut child = widget.first_child();
            let mut minimum = 0;
            let mut natural = 0;
            while let Some(current) = child {
                let (min, nat, _, _) = current.measure(orientation, for_size);
                if orientation == gtk::Orientation::Horizontal {
                    minimum += min;
                    natural += nat;
                } else {
                    minimum = minimum.max(min);
                    natural = natural.max(nat);
                }
                child = current.next_sibling();
            }
            (minimum, natural, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let Some(left) = widget.first_child() else {
                return;
            };
            let Some(center) = left.next_sibling() else {
                return;
            };
            let Some(right) = center.next_sibling() else {
                return;
            };
            let natural =
                |child: &gtk::Widget| child.measure(gtk::Orientation::Horizontal, height).1;
            let desired = if self.center.get() > 0 {
                self.center.get()
            } else {
                width / 2
            };
            // Shorten labels before moving the session off its terminal. The
            // popovers retain the full readings even in a narrow window.
            let center_min = center.measure(gtk::Orientation::Horizontal, height).0;
            let right_min = right.measure(gtk::Orientation::Horizontal, height).0;
            let right_width = natural(&right)
                .min(width / 3)
                .min((width - desired - center_min / 2).max(right_min));
            let center_width = natural(&center)
                .min(width / 3)
                .min((width - right_width - desired).max(0) * 2)
                .min(desired.max(0) * 2);
            let center_x = desired - center_width / 2;
            place(&left, 0, center_x, height);
            place(&center, center_x, center_width, height);
            place(&right, width - right_width, right_width, height);
        }
    }
}

glib::wrapper! {
    pub struct StatusBarLayout(ObjectSubclass<imp::StatusBarLayout>) @extends gtk::LayoutManager;
}

impl StatusBarLayout {
    pub fn new() -> Self {
        glib::Object::new()
    }
    pub fn set_center(&self, center: i32) {
        if self.imp().center.replace(center) != center {
            self.layout_changed();
        }
    }
}

fn place(widget: &gtk::Widget, x: i32, width: i32, height: i32) {
    let transform = gtk::gsk::Transform::new().translate(&gtk::graphene::Point::new(x as f32, 0.0));
    widget.allocate(width.max(0), height, -1, Some(transform));
}
