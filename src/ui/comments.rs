//! Comment lists: comments loaded page by page, and the widget showing them.

use std::rc::Rc;

use relm4::adw::prelude::*;
use relm4::{adw, gtk};

use super::{format, images, widgets};
use crate::core::model::{Comment, CommentPage};

/// Comments loaded page by page on demand.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Comments {
    pub items: Vec<Comment>,
    /// Total number of comments, if known.
    pub count: Option<u64>,
    /// Token of the next page; `None` when all are loaded or comments are off.
    pub next: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Comments {
    pub fn new(page: CommentPage) -> Self {
        Comments {
            items: page.comments,
            count: page.count,
            next: page.next,
            ..Comments::default()
        }
    }

    /// Whether there are comments to show or to load.
    pub fn available(&self) -> bool {
        self.next.is_some() || !self.items.is_empty()
    }

    /// The token of the next page to load, unless all are loaded or one is
    /// loading already.
    pub fn load_next(&mut self) -> Option<String> {
        let token = self.next.clone().filter(|_| !self.loading)?;
        self.loading = true;
        self.error = None;
        Some(token)
    }

    /// A page requested with [`Comments::load_next`] arrived.
    pub fn loaded(&mut self, result: Result<CommentPage, String>) {
        self.loading = false;
        match result {
            Ok(page) => {
                self.items.extend(page.comments);
                self.count = page.count.or(self.count);
                self.next = page.next;
            }
            Err(e) => self.error = Some(e),
        }
    }
}

/// Shows [`Comments`]: their rows, a spinner while loading, errors, and a
/// button to load the next page.
pub struct CommentList {
    root: gtk::Box,
    list: gtk::ListBox,
    spinner: adw::Spinner,
    error: gtk::Label,
    more: gtk::Button,
    /// Number of comments in `list`.
    shown: usize,
    /// Opens the replies to the comment with the given index.
    on_replies: Option<Rc<dyn Fn(usize)>>,
}

impl CommentList {
    /// `on_more` is called when the user asks for the next page, and
    /// `on_replies` with a comment's index to open its replies; without it,
    /// replies cannot be opened.
    pub fn new(on_more: impl Fn() + 'static, on_replies: Option<impl Fn(usize) + 'static>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let list = widgets::boxed_list();
        let spinner = adw::Spinner::new();
        let error = gtk::Label::builder()
            .css_classes(["dimmed"])
            .wrap(true)
            .xalign(0.0)
            .build();
        let more = gtk::Button::builder()
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        more.connect_clicked(move |_| on_more());
        for widget in [
            list.upcast_ref::<gtk::Widget>(),
            spinner.upcast_ref(),
            error.upcast_ref(),
            more.upcast_ref(),
        ] {
            root.append(widget);
        }
        let mut view = CommentList {
            root,
            list,
            spinner,
            error,
            more,
            shown: 0,
            on_replies: on_replies.map(|f| Rc::new(f) as Rc<dyn Fn(usize)>),
        };
        view.update(&Comments::default());
        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows the current state of `comments`, which only ever gain items.
    pub fn update(&mut self, comments: &Comments) {
        for (index, comment) in comments.items.iter().enumerate().skip(self.shown) {
            let on_replies = self
                .on_replies
                .clone()
                .map(|f| Box::new(move || f(index)) as Box<dyn Fn()>);
            self.list.append(&comment_row(comment, on_replies));
        }
        self.shown = comments.items.len();

        self.list.set_visible(!comments.items.is_empty());
        self.spinner.set_visible(comments.loading);
        self.error.set_visible(comments.error.is_some());
        if let Some(e) = &comments.error {
            self.error.set_label(&format!("Cannot load comments: {e}"));
        }
        self.more.set_label(if comments.error.is_some() {
            "Retry"
        } else {
            "Load More"
        });
        self.more.set_visible(
            comments.next.is_some()
                && !comments.loading
                && (!comments.items.is_empty() || comments.error.is_some()),
        );
    }
}

/// A non-activatable list row showing a comment and its author, with a
/// button calling `on_replies` if there are replies.
pub fn comment_row(comment: &Comment, on_replies: Option<Box<dyn Fn()>>) -> gtk::ListBoxRow {
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
    if let Some(on_replies) = on_replies
        && comment.replies.is_some()
        && comment.reply_count > 0
    {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        content.append(&gtk::Label::new(Some(&format::replies(
            comment.reply_count,
        ))));
        content.append(&gtk::Image::from_icon_name("go-next-symbolic"));
        let button = gtk::Button::builder()
            .child(&content)
            .halign(gtk::Align::Start)
            .css_classes(["flat", "replies"])
            .build();
        button.connect_clicked(move |_| on_replies());
        body.append(&button);
    }
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
