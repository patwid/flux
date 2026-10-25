//! Widgets shared between pages.

use relm4::adw::prelude::*;
use relm4::gtk::gio;
use relm4::{adw, gtk};

use super::thumbnail_row_layout::{ThumbnailLayout, ThumbnailRowLayout};
use super::{format, images};
use crate::core::model::{Channel, Playlist, Video};

/// An activatable list row for a video.
pub fn video_row(video: &Video) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .activatable(true)
        .use_markup(false)
        .title_lines(2)
        .title(&video.title)
        .subtitle(format::video_subtitle(video, chrono::Utc::now()))
        .build();
    add_thumbnail(&row, video.thumbnail_url.as_deref());
    row
}

/// An activatable list row for a playlist, opening its page.
pub fn playlist_row(playlist: &Playlist) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .activatable(true)
        .use_markup(false)
        .title_lines(2)
        .title(&playlist.name)
        .subtitle(playlist.video_count.map(format::videos).unwrap_or_default())
        .build();
    add_thumbnail(&row, playlist.thumbnail_url.as_deref());
    with_chevron(row)
}

/// Adds a thumbnail to `row`, which on narrow rows spans the row above the
/// title. This relies on the row's child being its header, with the
/// prefixes first.
fn add_thumbnail(row: &adw::ActionRow, url: Option<&str>) {
    let thumbnail = thumbnail(url);
    row.add_prefix(&thumbnail);
    if let Some(header) = row.child() {
        header.set_layout_manager(Some(ThumbnailRowLayout::default()));
        if let Some(prefixes) = header.first_child() {
            // The thumbnail fills the prefixes, which only grow when stacked.
            prefixes.set_hexpand(false);
            thumbnail.set_hexpand(true);
        }
    }
}

/// Adds a chevron to a row that opens a page.
pub fn with_chevron(row: adw::ActionRow) -> adw::ActionRow {
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row
}

/// Width of the thumbnails in list rows.
const THUMBNAIL_WIDTH: i32 = 320;

/// A 16:9 thumbnail for list rows, loaded asynchronously.
pub fn thumbnail(url: Option<&str>) -> gtk::Widget {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .build();
    images::set_picture(&picture, url);
    // Sized by the layout rather than by the image, whose natural size is
    // taller than 16:9 for the feed's 4:3 thumbnails and for shorts.
    let thumbnail = gtk::Box::builder()
        .layout_manager(&ThumbnailLayout::default())
        .width_request(THUMBNAIL_WIDTH)
        .valign(gtk::Align::Center)
        .css_classes(["thumbnail"])
        .overflow(gtk::Overflow::Hidden)
        .build();
    thumbnail.append(&picture);
    thumbnail.upcast()
}

/// A channel avatar with initials until the image is loaded.
pub fn avatar(channel: &Channel, size: i32) -> adw::Avatar {
    let avatar = adw::Avatar::new(size, Some(&channel.name), true);
    images::set_avatar(&avatar, channel.avatar_url.as_deref());
    avatar
}

/// An activatable list row for a channel, opening its page.
pub fn channel_row(channel: &Channel, subtitle: Option<&str>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .activatable(true)
        .use_markup(false)
        .title(&channel.name)
        .subtitle(subtitle.unwrap_or_default())
        .build();
    row.add_prefix(&avatar(channel, 40));
    with_chevron(row)
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

/// A status page with a spinner, for content that is loading.
pub fn loading_page(title: &str) -> adw::StatusPage {
    let page = adw::StatusPage::builder().title(title).build();
    page.set_paintable(Some(&adw::SpinnerPaintable::new(Some(&page))));
    page
}
