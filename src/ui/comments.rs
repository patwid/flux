//! Comment lists: comments loaded page by page, and the widget showing them.

use std::rc::Rc;

use relm4::adw::prelude::*;
use relm4::{adw, gtk};

use super::{format, images};
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

/// Shows [`Comments`] as rows: the comments, then a spinner while loading,
/// errors, and a button to load the next page.
pub struct CommentList {
    rows: adw::PreferencesGroup,
    spinner: adw::PreferencesRow,
    error: adw::ActionRow,
    more: adw::ButtonRow,
    /// Number of comments added.
    shown: usize,
    /// Opens the replies to the comment with the given index.
    on_replies: Option<Rc<dyn Fn(usize)>>,
}

impl CommentList {
    /// Adds the rows to the group `rows`. `on_more` is called when the user asks for
    /// the next page, and `on_replies` with a comment's index to open its
    /// replies; without it, replies cannot be opened.
    pub fn new(
        rows: adw::PreferencesGroup,
        on_more: impl Fn() + 'static,
        on_replies: Option<impl Fn(usize) + 'static>,
    ) -> Self {
        let spinner = adw::PreferencesRow::builder()
            .activatable(false)
            .child(
                &adw::Spinner::builder()
                    .height_request(24)
                    .margin_top(12)
                    .margin_bottom(12)
                    .build(),
            )
            .build();
        let error = adw::ActionRow::builder()
            .title("Cannot load comments")
            .use_markup(false)
            .build();
        let more = adw::ButtonRow::new();
        more.connect_activated(move |_| on_more());
        let mut view = CommentList {
            rows,
            spinner,
            error,
            more,
            shown: 0,
            on_replies: on_replies.map(|f| Rc::new(f) as Rc<dyn Fn(usize)>),
        };
        view.update(&Comments::default());
        view
    }

    /// Shows the current state of `comments`, which only ever gain items.
    pub fn update(&mut self, comments: &Comments) {
        // The status rows follow the comments, so they are added last.
        let status: [&gtk::Widget; 3] = [
            self.spinner.upcast_ref(),
            self.error.upcast_ref(),
            self.more.upcast_ref(),
        ];
        for row in status {
            if row.parent().is_some() {
                self.rows.remove(row);
            }
        }
        for (index, comment) in comments.items.iter().enumerate().skip(self.shown) {
            let on_replies = self
                .on_replies
                .clone()
                .map(|f| Box::new(move || f(index)) as Box<dyn Fn()>);
            self.rows.add(&comment_row(comment, on_replies));
        }
        self.shown = comments.items.len();

        if comments.loading {
            self.rows.add(&self.spinner);
        }
        if let Some(e) = &comments.error {
            self.error.set_subtitle(e);
            self.rows.add(&self.error);
        }
        self.more.set_title(if comments.error.is_some() {
            "Retry"
        } else {
            "Load More"
        });
        if comments.next.is_some()
            && !comments.loading
            && (!comments.items.is_empty() || comments.error.is_some())
        {
            self.rows.add(&self.more);
        }
    }
}

/// A non-activatable list row showing a comment and its author, with a
/// button calling `on_replies` if there are replies.
pub fn comment_row(comment: &Comment, on_replies: Option<Box<dyn Fn()>>) -> adw::PreferencesRow {
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
    adw::PreferencesRow::builder()
        .activatable(false)
        .child(&content)
        .build()
}
