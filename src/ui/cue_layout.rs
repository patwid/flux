//! A layout manager that places subtitle labels in the video as their
//! cue settings say.

use std::cell::{Cell, RefCell};

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::gtk::subclass::prelude::*;
use relm4::gtk::{gdk, glib};

use crate::core::subtitles::{Anchor, CueBox, TextAlign, Vertical};

/// Distance of the cues from the edges of the video, in pixels.
const MARGIN: f64 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn bottom(&self) -> f64 {
        self.y + self.height
    }

    /// The part of `self` that is also in `other`.
    fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Rect {
            x,
            y,
            width: (self.right().min(other.right()) - x).max(0.0),
            height: (self.bottom().min(other.bottom()) - y).max(0.0),
        }
    }
}

/// Where a video of width-to-height `ratio` is shown, scaled to fit an area
/// of `width`×`height` and centered; the whole area if `ratio` is unknown.
pub fn video_rect(width: f64, height: f64, ratio: Option<f64>) -> Rect {
    let (video_width, video_height) = match ratio {
        Some(ratio) if width / height > ratio => (height * ratio, height),
        Some(ratio) => (width, width / ratio),
        None => (width, height),
    };
    Rect {
        x: (width - video_width) / 2.0,
        y: (height - video_height) / 2.0,
        width: video_width,
        height: video_height,
    }
}

/// Where the label of a cue goes: `size` gives the label's size for a
/// maximum width. It is placed in `video` by `cue_box`, with lines of
/// `line_height`, and then moved into `safe` where needed.
pub fn cue_rect(
    cue_box: &CueBox,
    video: Rect,
    safe: Rect,
    line_height: f64,
    size: impl Fn(f64) -> (f64, f64),
) -> Rect {
    let box_left = video.x + cue_box.left * video.width;
    let box_width = cue_box.width * video.width;
    let (width, height) = size(box_width.min(safe.width));
    let x = match cue_box.align {
        TextAlign::Left => box_left,
        TextAlign::Center => box_left + (box_width - width) / 2.0,
        TextAlign::Right => box_left + box_width - width,
    };
    let y = match cue_box.vertical {
        Vertical::Line(n) if n >= 0 => video.y + f64::from(n) * line_height,
        Vertical::Line(n) => video.bottom() + f64::from(n + 1) * line_height - height,
        Vertical::At(at, anchor) => {
            let at = video.y + at * video.height;
            match anchor {
                Anchor::Start => at,
                Anchor::Center => at - height / 2.0,
                Anchor::End => at - height,
            }
        }
    };
    Rect {
        x: x.min(safe.right() - width).max(safe.x),
        y: y.min(safe.bottom() - height).max(safe.y),
        width,
        height,
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CueLayout {
        /// The cue box of each child, in order.
        pub boxes: RefCell<Vec<CueBox>>,
        /// The video, for its aspect ratio.
        pub paintable: RefCell<Option<gdk::Paintable>>,
        /// Space kept free at the bottom, e.g. for controls.
        pub bottom_inset: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CueLayout {
        const NAME: &'static str = "FluxCueLayout";
        type Type = super::CueLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for CueLayout {}

    impl LayoutManagerImpl for CueLayout {
        fn measure(
            &self,
            _widget: &gtk::Widget,
            _orientation: gtk::Orientation,
            _for_size: i32,
        ) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let (width, height) = (f64::from(width), f64::from(height));
            let ratio = self
                .paintable
                .borrow()
                .as_ref()
                .map(|p| p.intrinsic_aspect_ratio())
                .filter(|ratio| *ratio > 0.0);
            let video = video_rect(width, height, ratio);
            let area = Rect {
                x: MARGIN,
                y: MARGIN,
                width: width - 2.0 * MARGIN,
                height: height - MARGIN - f64::from(self.bottom_inset.get()),
            };
            let video_inset = Rect {
                x: video.x + MARGIN,
                y: video.y + MARGIN,
                width: video.width - 2.0 * MARGIN,
                height: video.height - 2.0 * MARGIN,
            };
            let safe = area.intersect(&video_inset);

            let boxes = self.boxes.borrow();
            let mut child = widget.first_child();
            for cue_box in boxes.iter() {
                let Some(c) = child else { break };
                let metrics = c.pango_context().metrics(None, None);
                let line_height = f64::from(metrics.height()) / f64::from(gtk::pango::SCALE);
                let rect = cue_rect(cue_box, video, safe, line_height, |max_width| {
                    let max_width = max_width as i32;
                    let (min, natural, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                    let width = natural.min(max_width).max(min);
                    let (_, height, _, _) = c.measure(gtk::Orientation::Vertical, width);
                    (f64::from(width), f64::from(height))
                });
                let transform = gtk::gsk::Transform::new()
                    .translate(&gtk::graphene::Point::new(rect.x as f32, rect.y as f32));
                c.allocate(rect.width as i32, rect.height as i32, -1, Some(transform));
                child = c.next_sibling();
            }
        }
    }
}

glib::wrapper! {
    /// Places each child label by its [`CueBox`] in the video.
    pub struct CueLayout(ObjectSubclass<imp::CueLayout>)
        @extends gtk::LayoutManager;
}

impl Default for CueLayout {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl CueLayout {
    /// Sets the cue boxes of the children, in order.
    pub fn set_boxes(&self, boxes: Vec<CueBox>) {
        *self.imp().boxes.borrow_mut() = boxes;
        self.layout_changed();
    }

    /// Sets the video whose aspect ratio places the cues.
    pub fn set_paintable(&self, paintable: Option<&gdk::Paintable>) {
        *self.imp().paintable.borrow_mut() = paintable.cloned();
        self.layout_changed();
    }

    /// Keeps `inset` pixels at the bottom free.
    pub fn set_bottom_inset(&self, inset: i32) {
        if self.imp().bottom_inset.replace(inset) != inset {
            self.layout_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn video_is_letterboxed() {
        assert_eq!(
            video_rect(1600.0, 900.0, None),
            rect(0.0, 0.0, 1600.0, 900.0)
        );
        assert_eq!(
            video_rect(1600.0, 900.0, Some(16.0 / 9.0)),
            rect(0.0, 0.0, 1600.0, 900.0)
        );
        assert_eq!(
            video_rect(1600.0, 900.0, Some(1.0)),
            rect(350.0, 0.0, 900.0, 900.0)
        );
        assert_eq!(
            video_rect(1600.0, 900.0, Some(4.0)),
            rect(0.0, 250.0, 1600.0, 400.0)
        );
    }

    const VIDEO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1000.0,
        height: 500.0,
    };
    const SAFE: Rect = Rect {
        x: 10.0,
        y: 10.0,
        width: 980.0,
        height: 430.0,
    };

    /// A label 200 pixels wide in one line of 40, or wrapped.
    fn label(max_width: f64) -> (f64, f64) {
        if max_width >= 200.0 {
            (200.0, 40.0)
        } else {
            (max_width, 80.0)
        }
    }

    fn placed(left: f64, width: f64, align: TextAlign, vertical: Vertical) -> Rect {
        let cue_box = CueBox {
            left,
            width,
            align,
            vertical,
        };
        cue_rect(&cue_box, VIDEO, SAFE, 30.0, label)
    }

    #[test]
    fn cues_are_aligned_in_their_box() {
        let at = Vertical::At(0.5, Anchor::Start);
        assert_eq!(placed(0.3, 0.4, TextAlign::Left, at).x, 300.0);
        assert_eq!(placed(0.3, 0.4, TextAlign::Center, at).x, 400.0);
        assert_eq!(placed(0.3, 0.4, TextAlign::Right, at).x, 500.0);
    }

    #[test]
    fn cues_wrap_in_narrow_boxes() {
        let at = Vertical::At(0.5, Anchor::Start);
        assert_eq!(
            placed(0.3, 0.15, TextAlign::Left, at),
            rect(300.0, 250.0, 150.0, 80.0)
        );
    }

    #[test]
    fn cues_are_placed_at_lines() {
        let y = |vertical| placed(0.0, 1.0, TextAlign::Center, vertical).y;
        assert_eq!(y(Vertical::Line(2)), 60.0);
        assert_eq!(y(Vertical::Line(-3)), 400.0);
        assert_eq!(y(Vertical::At(0.2, Anchor::Start)), 100.0);
        assert_eq!(y(Vertical::At(0.2, Anchor::Center)), 80.0);
        assert_eq!(y(Vertical::At(0.2, Anchor::End)), 60.0);
    }

    #[test]
    fn cues_stay_in_the_safe_area() {
        assert_eq!(
            placed(0.0, 0.35, TextAlign::Left, Vertical::Line(0)),
            rect(10.0, 10.0, 200.0, 40.0)
        );
        assert_eq!(
            placed(0.65, 0.35, TextAlign::Right, Vertical::Line(-1)),
            rect(790.0, 400.0, 200.0, 40.0)
        );
    }
}
