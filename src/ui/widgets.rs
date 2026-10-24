//! Widgets shared between pages.

use relm4::adw::prelude::*;
use relm4::{adw, gtk};

use super::{format, images};
use crate::core::model::{Channel, Comment, Video};

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
pub fn channel_row(channel: &Channel) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .activatable(true)
        .use_markup(false)
        .title(&channel.name)
        .subtitle("Channel")
        .build();
    row.add_prefix(&avatar(channel, 40));
    row
}

/// A non-activatable list row showing a comment and its author.
pub fn comment_row(comment: &Comment) -> gtk::ListBoxRow {
    let author = comment.author.as_ref();
    let name = author.map_or("Unknown", |a| a.name.as_str());
    // Initials of the handle, not "@".
    let avatar = adw::Avatar::new(32, Some(name.trim_start_matches('@')), author.is_some());
    images::set_avatar(&avatar, author.and_then(|a| a.avatar_url.as_deref()));
    avatar.set_valign(gtk::Align::Start);
    let author = gtk::Label::builder()
        .label(name)
        .css_classes(["heading"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .xalign(0.0)
        .build();
    if comment.by_owner {
        author.add_css_class("accent");
    }
    let text = gtk::Label::builder()
        .label(&comment.text)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .xalign(0.0)
        .build();
    let meta = format::comment_meta(comment, chrono::Utc::now());
    let meta = gtk::Label::builder()
        .visible(!meta.is_empty())
        .label(meta)
        .css_classes(["dimmed", "caption"])
        .wrap(true)
        .xalign(0.0)
        .build();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 3);
    body.set_hexpand(true);
    body.append(&author);
    body.append(&text);
    body.append(&meta);
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.append(&avatar);
    content.append(&body);
    gtk::ListBoxRow::builder()
        .activatable(false)
        .child(&content)
        .build()
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
