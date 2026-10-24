//! Widgets shared between pages.

use relm4::adw::prelude::*;
use relm4::{adw, gtk};

use super::format;
use crate::core::model::{Channel, Video};

/// An activatable list row for a video.
pub fn video_row(video: &Video) -> adw::ActionRow {
    adw::ActionRow::builder()
        .activatable(true)
        .use_markup(false)
        .title_lines(2)
        .title(&video.title)
        .subtitle(format::video_subtitle(video))
        .build()
}

/// An activatable list row for a channel.
pub fn channel_row(channel: &Channel) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .activatable(true)
        .use_markup(false)
        .title(&channel.name)
        .subtitle("Channel")
        .build();
    row.add_prefix(&gtk::Image::from_icon_name("avatar-default-symbolic"));
    row
}

/// A boxed list for rows of videos or channels.
pub fn boxed_list() -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .valign(gtk::Align::Start)
        .build();
    list.add_css_class("boxed-list");
    list
}
