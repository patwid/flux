//! Widgets shared between pages.

use relm4::adw::prelude::*;
use relm4::gtk::gio;
use relm4::{adw, gtk};

use super::{format, images};
use crate::core::model::{Channel, Video};

/// An activatable list row for a video.
pub fn video_row(video: &Video) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .activatable(true)
        .use_markup(false)
        .title_lines(2)
        .title(&video.title)
        .subtitle(format::video_subtitle(video, chrono::Utc::now()))
        .build();
    row.add_prefix(&thumbnail(video.thumbnail_url.as_deref()));
    row
}

/// A small 16:9 thumbnail, loaded asynchronously.
pub fn thumbnail(url: Option<&str>) -> gtk::Widget {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .width_request(96)
        .height_request(54)
        .css_classes(["thumbnail"])
        .overflow(gtk::Overflow::Hidden)
        .build();
    images::set_picture(&picture, url);
    // The picture would otherwise request the image's natural size.
    adw::Clamp::builder()
        .maximum_size(96)
        .tightening_threshold(96)
        .valign(gtk::Align::Center)
        .child(&picture)
        .build()
        .upcast()
}

/// A channel avatar with initials until the image is loaded.
pub fn avatar(channel: &Channel, size: i32) -> adw::Avatar {
    let avatar = adw::Avatar::new(size, Some(&channel.name), true);
    images::set_avatar(&avatar, channel.avatar_url.as_deref());
    avatar
}

/// An activatable list row for a channel.
pub fn channel_row(channel: &Channel, subtitle: Option<&str>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .activatable(true)
        .use_markup(false)
        .title(&channel.name)
        .subtitle(subtitle.unwrap_or_default())
        .build();
    row.add_prefix(&avatar(channel, 40));
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

/// The primary menu button, shown at the end of each section's header bar.
pub fn primary_menu() -> gtk::MenuButton {
    let menu = gio::Menu::new();
    menu.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
    menu.append(Some("_About Flux"), Some("win.about"));
    gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
        .menu_model(&menu)
        .primary(true)
        .build()
}
