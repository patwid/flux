//! Layout managers for rows with a thumbnail (videos, playlists): on narrow
//! rows the thumbnail spans the row's width, above the title and buttons.

use relm4::gtk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;
use relm4::gtk::subclass::prelude::*;

use super::aspect::RATIO;

/// Rows whose content is narrower than this stack the thumbnail above the text.
const STACK_BELOW_WIDTH: i32 = 600;
/// Space between a stacked thumbnail and the text below it.
const STACK_SPACING: i32 = 6;
/// Space between the thumbnail, title and buttons beside each other.
const SPACING: i32 = 6;

/// Whether a row with content `width` wide stacks its thumbnail; it also
/// does when it is narrower than its side-by-side layout needs (`row_min`).
pub fn stacks(width: i32, row_min: i32) -> bool {
    width < STACK_BELOW_WIDTH.max(row_min)
}

/// The height of a 16:9 thumbnail `width` wide.
pub fn thumbnail_height(width: i32) -> i32 {
    (width as f64 / RATIO).round() as i32
}

/// The minimum and natural size of a thumbnail `width` wide, or narrower
/// if it `shrinks`, in `orientation` for the other one's `for_size`. Only
/// the natural height grows with the width: GTK expects a widget to need no
/// more height when given more width.
pub fn thumbnail_size(
    width: i32,
    shrinks: bool,
    orientation: gtk::Orientation,
    for_size: i32,
) -> (i32, i32) {
    let min_width = if shrinks { 0 } else { width };
    if orientation == gtk::Orientation::Horizontal {
        return (min_width, width);
    }
    let min = thumbnail_height(min_width);
    let natural = thumbnail_height(if for_size < 0 { width } else { for_size });
    (min, natural.max(min))
}

/// Splits `width` among a row of children, `spacing` apart: each gets its
/// natural width, except an expanding one (`None`), which gets the rest.
pub fn split_width(width: i32, spacing: i32, naturals: &[Option<i32>]) -> Vec<i32> {
    let gaps = spacing * (naturals.len() as i32 - 1).max(0);
    let fixed: i32 = naturals.iter().flatten().sum();
    let rest = (width - gaps - fixed).max(0);
    naturals.iter().map(|n| n.unwrap_or(rest)).collect()
}

mod imp {
    use super::*;

    /// Lays out an `AdwActionRow`'s header: its first child (the prefixes)
    /// in a row with the others, or, when narrow, above them. The prefixes
    /// get their natural width beside the others, as the thumbnail in them
    /// shrinks only to fit stacked rows.
    #[derive(Default)]
    pub struct ThumbnailRowLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for ThumbnailRowLayout {
        const NAME: &'static str = "FluxThumbnailRowLayout";
        type Type = super::ThumbnailRowLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for ThumbnailRowLayout {}

    /// Whether `widget` stacks its thumbnail when `width` wide.
    fn stacks_at(widget: &gtk::Widget, width: i32) -> bool {
        let (first, text) = children(widget);
        let first = first.map_or(0, |c| c.measure(gtk::Orientation::Horizontal, -1).1);
        let text: i32 = text
            .iter()
            .map(|c| c.measure(gtk::Orientation::Horizontal, -1).0 + SPACING)
            .sum();
        stacks(width, first + text)
    }

    /// The visible children of `widget`: the first one, then the others.
    fn children(widget: &gtk::Widget) -> (Option<gtk::Widget>, Vec<gtk::Widget>) {
        let mut all = Vec::new();
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            all.push(c);
        }
        let mut all = all.into_iter();
        let first = all.next().filter(|c| c.should_layout());
        (first, all.filter(|c| c.should_layout()).collect())
    }

    /// The widths of `children` side by side in `width`: the title gets
    /// what the others leave.
    fn widths(children: &[gtk::Widget], width: i32) -> Vec<i32> {
        let naturals: Vec<_> = children
            .iter()
            .map(|c| {
                let (min, nat, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                (!c.compute_expand(gtk::Orientation::Horizontal)).then_some(nat.max(min))
            })
            .collect();
        split_width(width, SPACING, &naturals)
    }

    /// The height of the title and buttons, `widths` wide.
    fn text_height(text: &[gtk::Widget], widths: &[i32]) -> i32 {
        text.iter()
            .zip(widths)
            .map(|(c, &w)| c.measure(gtk::Orientation::Vertical, w).0)
            .max()
            .unwrap_or(0)
    }

    /// Allocates `children` side by side, `widths` wide and `height` tall,
    /// `top` from the top.
    fn allocate_row(children: &[gtk::Widget], widths: &[i32], top: i32, height: i32) {
        let mut x = 0;
        for (child, &w) in children.iter().zip(widths) {
            let at = gtk::gsk::Transform::new()
                .translate(&gtk::graphene::Point::new(x as f32, top as f32));
            child.allocate(w, height, -1, Some(at));
            x += w + SPACING;
        }
    }

    impl LayoutManagerImpl for ThumbnailRowLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Horizontal {
                // Rows stack when narrow, so they only need as much width
                // as the widest part of the stacked layout.
                let (first, text) = children(widget);
                let measure = |c: &gtk::Widget| c.measure(orientation, -1);
                let thumbnail = first.as_ref().map_or(0, |c| measure(c).0);
                let gaps = SPACING * (text.len() as i32 - 1).max(0);
                let text_min = gaps + text.iter().map(|c| measure(c).0).sum::<i32>();
                let natural = first.iter().chain(&text).map(|c| measure(c).1).sum::<i32>()
                    + SPACING * text.len() as i32;
                return (thumbnail.max(text_min), natural, -1, -1);
            }
            let (first, text) = children(widget);
            // Measured without a width, a row is as tall as at its minimum
            // width, where it stacks.
            let width = if for_size < 0 {
                self.measure(widget, gtk::Orientation::Horizontal, -1).0
            } else {
                for_size
            };
            let height = if stacks_at(widget, width) {
                thumbnail_height(width) + STACK_SPACING + text_height(&text, &widths(&text, width))
            } else {
                let all: Vec<_> = first.iter().chain(&text).cloned().collect();
                let widths = widths(&all, width);
                // The thumbnail only shrinks when stacked, so it gets its
                // natural height here.
                let thumbnail = first.map_or(0, |c| c.measure(orientation, widths[0]).1);
                thumbnail.max(text_height(&text, &widths[1..]))
            };
            (height, height, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let (first, text) = children(widget);
            if !stacks_at(widget, width) {
                let all: Vec<_> = first.into_iter().chain(text).collect();
                allocate_row(&all, &widths(&all, width), 0, height);
                return;
            }
            let thumbnail = thumbnail_height(width);
            if let Some(first) = first {
                first.allocate(width, thumbnail, -1, None);
            }
            let top = thumbnail + STACK_SPACING;
            allocate_row(&text, &widths(&text, width), top, (height - top).max(0));
        }
    }
}

mod thumbnail_imp {
    use std::cell::Cell;

    use super::*;

    /// Sizes a thumbnail to its width and 16:9, whatever its image's
    /// size, and fills it with its child. Given more width, its natural
    /// height stays 16:9, its minimum height does not.
    #[derive(Default)]
    pub struct ThumbnailLayout {
        /// The thumbnail's natural width.
        pub width: Cell<i32>,
        /// Whether the thumbnail may be narrower than `width`.
        pub shrinks: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThumbnailLayout {
        const NAME: &'static str = "FluxThumbnailLayout";
        type Type = super::ThumbnailLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for ThumbnailLayout {}

    impl LayoutManagerImpl for ThumbnailLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            _widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let (min, natural) =
                thumbnail_size(self.width.get(), self.shrinks.get(), orientation, for_size);
            (min, natural, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, baseline: i32) {
            let mut child = widget.first_child();
            while let Some(c) = child {
                // GTK expects children to be measured before allocation,
                // though the size comes from the layout, not the image.
                c.measure(gtk::Orientation::Horizontal, height);
                c.allocate(width, height, baseline, None);
                child = c.next_sibling();
            }
        }
    }
}

glib::wrapper! {
    /// Sizes its widget 16:9, `width` wide unless given more space.
    pub struct ThumbnailLayout(ObjectSubclass<thumbnail_imp::ThumbnailLayout>)
        @extends gtk::LayoutManager;
}

impl ThumbnailLayout {
    /// A layout for a thumbnail `width` wide.
    pub fn new(width: i32) -> Self {
        let layout: Self = glib::Object::new();
        layout.imp().width.set(width);
        layout
    }

    /// A layout for a thumbnail `width` wide, which may also be narrower,
    /// down to nothing.
    pub fn shrinking(width: i32) -> Self {
        let layout = Self::new(width);
        layout.imp().shrinks.set(true);
        layout
    }
}

glib::wrapper! {
    /// Lays out an `AdwActionRow`'s header (prefixes, title, suffixes) in a
    /// row, or with the prefixes stacked above the rest on narrow rows.
    pub struct ThumbnailRowLayout(ObjectSubclass<imp::ThumbnailRowLayout>)
        @extends gtk::LayoutManager;
}

impl Default for ThumbnailRowLayout {
    fn default() -> Self {
        glib::Object::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_rows_stack() {
        assert!(stacks(500, 200));
        assert!(!stacks(700, 200));
    }

    #[test]
    fn rows_too_narrow_for_side_by_side_stack() {
        assert!(stacks(700, 800));
    }

    /// GTK's debug builds warn when a widget fitting `min_width` at a
    /// height needs more than that height when given at least that width.
    #[test]
    fn thumbnail_sizes_are_consistent() {
        use gtk::Orientation::{Horizontal, Vertical};

        for shrinks in [false, true] {
            let (min_height, _) = thumbnail_size(320, shrinks, Vertical, -1);
            for height in [min_height, 180, 291, 400] {
                let (min_width, _) = thumbnail_size(320, shrinks, Horizontal, height);
                for width in [0, 160, 320, 518, 800] {
                    let (min_height, _) = thumbnail_size(320, shrinks, Vertical, width);
                    assert!(
                        min_width > width || min_height <= height,
                        "min width {min_width} for height {height}, \
                         but min height {min_height} for width {width}"
                    );
                }
            }
        }
    }

    #[test]
    fn thumbnails_are_16_9_at_their_natural_width() {
        use gtk::Orientation::{Horizontal, Vertical};

        assert_eq!(thumbnail_size(320, false, Horizontal, -1), (320, 320));
        assert_eq!(thumbnail_size(320, true, Horizontal, -1), (0, 320));
        assert_eq!(thumbnail_size(320, false, Vertical, 518), (180, 291));
        assert_eq!(thumbnail_size(320, true, Vertical, 296), (0, 167));
    }

    /// Needs a display: `xvfb-run cargo test -- --ignored rows_lay_out`.
    #[test]
    #[ignore]
    fn rows_lay_out_at_any_width_they_accept() {
        use relm4::adw;
        use relm4::adw::prelude::*;

        use crate::core::model::{Playlist, Video};
        use crate::ui::widgets;

        adw::init().unwrap();
        let long = "Unbreakable".repeat(12);
        let video = Video {
            id: "v".into(),
            title: format!("{long} {long}"),
            channel_id: "UC1".into(),
            channel_name: long.clone(),
            published: None,
            duration_secs: Some(65),
            thumbnail_url: None,
        };
        let playlist = Playlist {
            id: "p".into(),
            name: format!("{long} {long}"),
            thumbnail_url: None,
            video_count: Some(12),
        };
        let video_row = widgets::with_chevron(widgets::video_row(&video));
        for _ in 0..3 {
            video_row.add_suffix(&gtk::Button::from_icon_name("user-trash-symbolic"));
        }
        for row in [video_row, widgets::playlist_row(&playlist)] {
            let window = gtk::Window::builder().child(&row).build();
            window.present();
            let (min, nat, _, _) = row.measure(gtk::Orientation::Horizontal, -1);
            for width in min..=nat.max(min + 1000) {
                let (height, _, _, _) = row.measure(gtk::Orientation::Vertical, width);
                row.allocate(width, height, -1, None);
            }
        }
    }

    #[test]
    fn thumbnails_are_16_by_9() {
        assert_eq!(thumbnail_height(320), 180);
        assert_eq!(thumbnail_height(336), 189);
    }

    #[test]
    fn the_expanding_child_gets_the_rest() {
        assert_eq!(
            split_width(300, 6, &[None, Some(34), Some(16)]),
            vec![238, 34, 16]
        );
        assert_eq!(split_width(30, 6, &[None, Some(34)]), vec![0, 34]);
        assert_eq!(split_width(300, 6, &[None]), vec![300]);
    }
}
