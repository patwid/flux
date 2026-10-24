//! A layout manager that sizes its widget to a fixed aspect ratio.

use relm4::gtk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;
use relm4::gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AspectLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for AspectLayout {
        const NAME: &'static str = "FluxAspectLayout";
        type Type = super::AspectLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for AspectLayout {}

    impl LayoutManagerImpl for AspectLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let mut min = 0;
            let mut nat = 0;
            let mut child = widget.first_child();
            while let Some(c) = child {
                let (child_min, child_nat, _, _) = c.measure(orientation, -1);
                min = min.max(child_min);
                nat = nat.max(child_nat);
                child = c.next_sibling();
            }
            if orientation == gtk::Orientation::Vertical && for_size >= 0 {
                let height = (for_size as f64 / super::RATIO).round() as i32;
                return (min.max(height), min.max(height), -1, -1);
            }
            (min, nat, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, baseline: i32) {
            let mut child = widget.first_child();
            while let Some(c) = child {
                c.allocate(width, height, baseline, None);
                child = c.next_sibling();
            }
        }
    }
}

/// Width-to-height ratio of the layout (16:9).
pub const RATIO: f64 = 16.0 / 9.0;

glib::wrapper! {
    /// Requests a height of `width / RATIO`; children fill the allocation.
    pub struct AspectLayout(ObjectSubclass<imp::AspectLayout>)
        @extends gtk::LayoutManager;
}

impl Default for AspectLayout {
    fn default() -> Self {
        glib::Object::new()
    }
}
