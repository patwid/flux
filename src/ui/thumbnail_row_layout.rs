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
/// Space above a stacked thumbnail, matching the row's side margins.
const STACK_TOP: i32 = 12;
/// Space between the title and the buttons of a stacked row.
const TEXT_SPACING: i32 = 6;

/// Whether a row with content `width` wide stacks its thumbnail; it also
/// does when it is narrower than its side-by-side layout needs (`row_min`).
pub fn stacks(width: i32, row_min: i32) -> bool {
    width < STACK_BELOW_WIDTH.max(row_min)
}

/// The height of a 16:9 thumbnail `width` wide.
pub fn thumbnail_height(width: i32) -> i32 {
    (width as f64 / RATIO).round() as i32
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
    use std::cell::OnceCell;

    use super::*;

    /// Lays out an `AdwActionRow`'s header: its first child (the prefixes)
    /// in a row with the others, or, when narrow, above them.
    #[derive(Default)]
    pub struct ThumbnailRowLayout {
        /// The header's own layout, used when the row is wide.
        row: OnceCell<gtk::BoxLayout>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThumbnailRowLayout {
        const NAME: &'static str = "FluxThumbnailRowLayout";
        type Type = super::ThumbnailRowLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for ThumbnailRowLayout {}

    impl ThumbnailRowLayout {
        fn row(&self) -> &gtk::BoxLayout {
            self.row
                .get_or_init(|| gtk::BoxLayout::new(gtk::Orientation::Horizontal))
        }

        /// Whether `widget` stacks its thumbnail when `width` wide. The
        /// side-by-side layout must not be measured or allocated narrower
        /// than its minimum: GTK's box layout aborts.
        fn stacks(&self, widget: &gtk::Widget, width: i32) -> bool {
            let row_min = self
                .row()
                .measure(widget, gtk::Orientation::Horizontal, -1)
                .0;
            stacks(width, row_min)
        }
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

    /// The widths of the text and buttons below a stacked thumbnail.
    fn text_widths(text: &[gtk::Widget], width: i32) -> Vec<i32> {
        let naturals: Vec<_> = text
            .iter()
            .map(|c| {
                let (min, nat, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                (!c.compute_expand(gtk::Orientation::Horizontal)).then_some(nat.max(min))
            })
            .collect();
        split_width(width, TEXT_SPACING, &naturals)
    }

    /// The height of the text and buttons below a stacked thumbnail.
    fn text_height(text: &[gtk::Widget], widths: &[i32]) -> i32 {
        text.iter()
            .zip(widths)
            .map(|(c, &w)| c.measure(gtk::Orientation::Vertical, w).0)
            .max()
            .unwrap_or(0)
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
                let thumbnail = first.map_or(0, |c| c.measure(orientation, -1).0);
                let gaps = TEXT_SPACING * (text.len() as i32 - 1).max(0);
                let text = gaps
                    + text
                        .iter()
                        .map(|c| c.measure(orientation, -1).0)
                        .sum::<i32>();
                let natural = self.row().measure(widget, orientation, -1).1;
                return (thumbnail.max(text), natural, -1, -1);
            }
            if for_size < 0 || !self.stacks(widget, for_size) {
                let (min, nat, _, _) = self.row().measure(widget, orientation, for_size);
                return (min, nat, -1, -1);
            }
            let (_, text) = children(widget);
            let height = STACK_TOP
                + thumbnail_height(for_size)
                + STACK_SPACING
                + text_height(&text, &text_widths(&text, for_size));
            (height, height, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, baseline: i32) {
            if !self.stacks(widget, width) {
                self.row().allocate(widget, width, height, baseline);
                return;
            }
            let (first, text) = children(widget);
            let thumbnail = thumbnail_height(width);
            if let Some(first) = first {
                let at = gtk::gsk::Transform::new()
                    .translate(&gtk::graphene::Point::new(0.0, STACK_TOP as f32));
                first.allocate(width, thumbnail, -1, Some(at));
            }
            let top = STACK_TOP + thumbnail + STACK_SPACING;
            let widths = text_widths(&text, width);
            let text_height = (height - top).max(0);
            let mut x = 0;
            for (child, w) in text.iter().zip(widths) {
                let at = gtk::gsk::Transform::new()
                    .translate(&gtk::graphene::Point::new(x as f32, top as f32));
                child.allocate(w, text_height, -1, Some(at));
                x += w + TEXT_SPACING;
            }
        }
    }
}

mod thumbnail_imp {
    use super::*;

    /// Sizes a thumbnail to the given width (`width_request`) and 16:9,
    /// whatever its image's size, and fills it with its child.
    #[derive(Default)]
    pub struct ThumbnailLayout;

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
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let width = widget.width_request().max(0);
            let size = match orientation {
                gtk::Orientation::Horizontal => width,
                _ => thumbnail_height(if for_size < 0 { width } else { for_size }),
            };
            (size, size, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, baseline: i32) {
            let mut child = widget.first_child();
            while let Some(c) = child {
                // GTK expects children to be measured before allocation,
                // though the size comes from `width_request`, not the image.
                c.measure(gtk::Orientation::Horizontal, height);
                c.allocate(width, height, baseline, None);
                child = c.next_sibling();
            }
        }
    }
}

glib::wrapper! {
    /// Sizes its widget 16:9, `width_request` wide unless given more space.
    pub struct ThumbnailLayout(ObjectSubclass<thumbnail_imp::ThumbnailLayout>)
        @extends gtk::LayoutManager;
}

impl Default for ThumbnailLayout {
    fn default() -> Self {
        glib::Object::new()
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
