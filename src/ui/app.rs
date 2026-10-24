use relm4::adw::prelude::*;
use relm4::gtk::gio;
use relm4::{
    Component, ComponentController, ComponentParts, ComponentSender, Controller, adw, gtk,
};

use super::channel::{self, ChannelPage};
use super::downloads::{self, DownloadsPage};
use super::feed::FeedPage;
use super::history::{self, HistoryPage};
use super::replies::RepliesPage;
use super::search::{self, SearchPage};
use super::subscriptions::{self, SubscriptionsPage};
use super::video::{self, VideoPage};
use super::{Services, log_db_error};
use crate::core::download;
use crate::core::model::{Channel, Comment, DownloadKind, HistoryEntry, Video};

/// Windows narrower than this use the mobile layout (bottom view switcher).
const NARROW_WIDTH_SP: f64 = 550.0;

pub struct App {
    services: Services,
    feed: Controller<FeedPage>,
    search: Controller<SearchPage>,
    subscriptions: Controller<SubscriptionsPage>,
    history: Controller<HistoryPage>,
    downloads: Controller<DownloadsPage>,
    toasts: adw::ToastOverlay,
    navigation: adw::NavigationView,
    /// The top-level sections.
    stack: adw::ViewStack,
    /// Pages pushed onto the navigation view, kept alive while shown.
    pages: Vec<(adw::NavigationPage, Page)>,
}

/// A page pushed onto the navigation view.
enum Page {
    Video(Controller<VideoPage>),
    Channel(Controller<ChannelPage>),
    Replies(Controller<RepliesPage>),
}

#[derive(Debug)]
pub enum Msg {
    OpenVideo(Video),
    OpenChannel(Channel),
    Download(Video, DownloadKind),
    SubscriptionsChanged,
    OpenReplies(Comment),
    Popped(adw::NavigationPage),
    Toast(String),
    /// Offers to play a finished download.
    DownloadCompleted(Video),
    /// Offers to retry the failed download with this id.
    DownloadFailed(i64, Video),
    /// Offers to undo removing this entry from the history.
    RemovedFromHistory(HistoryEntry),
    /// Shows the search section with its entry focused.
    Search,
    /// Refreshes the feed if it is shown.
    Refresh,
}

impl From<search::Output> for Msg {
    fn from(out: search::Output) -> Self {
        match out {
            search::Output::OpenVideo(v) => Msg::OpenVideo(v),
            search::Output::OpenChannel(c) => Msg::OpenChannel(c),
        }
    }
}

impl From<history::Output> for Msg {
    fn from(out: history::Output) -> Self {
        match out {
            history::Output::OpenVideo(v) => Msg::OpenVideo(v),
            history::Output::Removed(entry) => Msg::RemovedFromHistory(entry),
        }
    }
}

impl From<downloads::Output> for Msg {
    fn from(out: downloads::Output) -> Self {
        match out {
            downloads::Output::OpenVideo(v) => Msg::OpenVideo(v),
            downloads::Output::Completed(v) => Msg::DownloadCompleted(v),
            downloads::Output::Failed { id, video } => Msg::DownloadFailed(id, video),
        }
    }
}

impl From<channel::Output> for Msg {
    fn from(out: channel::Output) -> Self {
        match out {
            channel::Output::OpenVideo(v) => Msg::OpenVideo(v),
            channel::Output::SubscriptionsChanged => Msg::SubscriptionsChanged,
            channel::Output::Toast(text) => Msg::Toast(text),
        }
    }
}

impl From<video::Output> for Msg {
    fn from(out: video::Output) -> Self {
        match out {
            video::Output::OpenVideo(v) => Msg::OpenVideo(v),
            video::Output::OpenChannel(c) => Msg::OpenChannel(c),
            video::Output::Download(v, kind) => Msg::Download(v, kind),
            video::Output::SubscriptionsChanged => Msg::SubscriptionsChanged,
            video::Output::OpenReplies(c) => Msg::OpenReplies(c),
            video::Output::Toast(text) => Msg::Toast(text),
        }
    }
}

#[relm4::component(pub)]
impl Component for App {
    type Init = Services;
    type Input = Msg;
    type Output = ();
    type CommandOutput = ();

    view! {
        adw::ApplicationWindow {
            set_title: Some("Flux"),
            set_default_width: 360,
            set_default_height: 720,
            set_width_request: 360,
            set_height_request: 294,

            #[local_ref]
            toasts -> adw::ToastOverlay {
                #[local_ref]
                navigation -> adw::NavigationView {
                    connect_popped[sender] => move |_, page| {
                        sender.input(Msg::Popped(page.clone()));
                    },

                    adw::NavigationPage {
                        set_title: "Flux",
                        set_tag: Some("main"),

                        #[name = "layouts"]
                        adw::MultiLayoutView {},
                    },
                },
            },
        }
    }

    fn init(
        services: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        relm4::set_global_css(include_str!("style.css"));
        let feed = FeedPage::builder()
            .launch(services.clone())
            .forward(sender.input_sender(), Msg::OpenVideo);
        let search = SearchPage::builder()
            .launch(services.backend.clone())
            .forward(sender.input_sender(), Msg::from);
        let subscriptions = SubscriptionsPage::builder()
            .launch(services.clone())
            .forward(sender.input_sender(), Msg::OpenChannel);
        let history = HistoryPage::builder()
            .launch(services.clone())
            .forward(sender.input_sender(), Msg::from);
        let downloads = DownloadsPage::builder()
            .launch(services.clone())
            .forward(sender.input_sender(), Msg::from);
        let model = App {
            services,
            feed,
            search,
            subscriptions,
            history,
            downloads,
            toasts: adw::ToastOverlay::new(),
            navigation: adw::NavigationView::new(),
            stack: adw::ViewStack::new(),
            pages: Vec::new(),
        };
        let toasts = &model.toasts;
        let navigation = &model.navigation;
        let widgets = view_output!();

        let stack = &model.stack;
        let add_page = |name: &str, title: &str, icon: &str, child: &gtk::Widget| {
            stack.add_titled_with_icon(child, Some(name), title, icon);
        };
        add_page(
            "feed",
            "Feed",
            "view-list-symbolic",
            model.feed.widget().upcast_ref(),
        );
        add_page(
            "search",
            "Search",
            "system-search-symbolic",
            model.search.widget().upcast_ref(),
        );
        add_page(
            "subscriptions",
            "Subscriptions",
            "starred-symbolic",
            model.subscriptions.widget().upcast_ref(),
        );
        add_page(
            "history",
            "History",
            "document-open-recent-symbolic",
            model.history.widget().upcast_ref(),
        );
        add_page(
            "downloads",
            "Downloads",
            "folder-download-symbolic",
            model.downloads.widget().upcast_ref(),
        );

        let entry = model.search.model().entry.clone();
        stack.connect_visible_child_name_notify(move |stack| {
            if stack.visible_child_name().as_deref() == Some("search") {
                entry.grab_focus();
            }
        });

        widgets.layouts.add_layout(wide_layout(stack));
        widgets.layouts.add_layout(narrow_layout(stack));
        widgets.layouts.set_child("content", stack);
        widgets.layouts.set_layout_name("wide");

        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            NARROW_WIDTH_SP,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&widgets.layouts, "layout-name", Some(&"narrow".to_value()));
        root.add_breakpoint(narrow);
        add_actions(&root, &sender);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::OpenVideo(video) => {
                self.open_video(video, &sender);
                self.history.emit(history::Input::Reload);
            }
            Msg::OpenChannel(channel) => self.open_channel(channel, &sender),
            Msg::OpenReplies(comment) => self.open_replies(comment),
            Msg::Download(video, kind) => {
                self.toast(&format!("Downloading “{}”", video.title));
                self.downloads.emit(downloads::Input::Add(video, kind));
            }
            Msg::SubscriptionsChanged => {
                self.feed.emit(super::feed::Input::Reload);
                self.subscriptions.emit(subscriptions::Msg::Reload);
            }
            Msg::Popped(page) => {
                // Dropping a video page saves its position to the history.
                self.pages.retain(|(p, _)| *p != page);
                self.history.emit(history::Input::Reload);
            }
            Msg::Toast(text) => self.toast(&text),
            Msg::DownloadCompleted(video) => {
                let toast = adw::Toast::builder()
                    .title(format!("Downloaded “{}”", video.title))
                    .button_label("_Play")
                    .build();
                let input = sender.input_sender().clone();
                toast.connect_button_clicked(move |_| input.emit(Msg::OpenVideo(video.clone())));
                self.toasts.add_toast(toast);
            }
            Msg::DownloadFailed(id, video) => {
                let toast = adw::Toast::builder()
                    .title(format!("Cannot download “{}”", video.title))
                    .button_label("_Retry")
                    .build();
                let downloads = self.downloads.sender().clone();
                toast.connect_button_clicked(move |_| {
                    downloads.emit(downloads::Input::Queue(download::Msg::Retry(id)));
                });
                self.toasts.add_toast(toast);
            }
            Msg::RemovedFromHistory(entry) => {
                let toast = adw::Toast::builder()
                    .title("Removed from history")
                    .button_label("_Undo")
                    .build();
                let history = self.history.sender().clone();
                toast.connect_button_clicked(move |_| {
                    history.emit(history::Input::State(history::Msg::Restore(entry.clone())));
                });
                self.toasts.add_toast(toast);
            }
            Msg::Search => {
                self.navigation.pop_to_tag("main");
                self.stack.set_visible_child_name("search");
                self.search.model().entry.grab_focus();
            }
            Msg::Refresh => {
                let main = self.navigation.visible_page().and_then(|p| p.tag());
                if main.as_deref() == Some("main")
                    && self.stack.visible_child_name().as_deref() == Some("feed")
                {
                    self.feed
                        .emit(super::feed::Input::State(super::feed::Msg::Refresh));
                }
            }
        }
    }
}

/// Adds the window actions and their keyboard shortcuts.
fn add_actions(window: &adw::ApplicationWindow, sender: &ComponentSender<App>) {
    let action = |name: &str, accels: &[&str], activate: Box<dyn Fn()>| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| activate());
        window.add_action(&action);
        relm4::main_application().set_accels_for_action(&format!("win.{name}"), accels);
    };
    let window_ = window.clone();
    action(
        "about",
        &[],
        Box::new(move || about_dialog().present(Some(&window_))),
    );
    let window_ = window.clone();
    action(
        "shortcuts",
        &["<Control>question"],
        Box::new(move || shortcuts_dialog().present(Some(&window_))),
    );
    let input = sender.input_sender().clone();
    action(
        "search",
        &["<Control>f"],
        Box::new(move || input.emit(Msg::Search)),
    );
    let input = sender.input_sender().clone();
    action(
        "refresh",
        &["<Control>r", "F5"],
        Box::new(move || input.emit(Msg::Refresh)),
    );
}

fn about_dialog() -> adw::AboutDialog {
    adw::AboutDialog::builder()
        .application_name("Flux")
        .application_icon(crate::APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Patrick Widmer")
        .license_type(gtk::License::Gpl30)
        .build()
}

fn shortcuts_dialog() -> adw::ShortcutsDialog {
    let dialog = adw::ShortcutsDialog::new();
    let section = |title: &str, items: &[(&str, &str)]| {
        let section = adw::ShortcutsSection::new(Some(title));
        for (title, accel) in items {
            section.add(adw::ShortcutsItem::new(title, accel));
        }
        dialog.add(section);
    };
    section(
        "General",
        &[
            ("Search", "<Control>f"),
            ("Refresh Feed", "<Control>r"),
            ("Keyboard Shortcuts", "<Control>question"),
        ],
    );
    section(
        "Video",
        &[
            ("Play or Pause", "space"),
            ("Toggle Fullscreen", "f"),
            ("Leave Fullscreen", "Escape"),
        ],
    );
    dialog
}

impl App {
    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn open_video(&mut self, video: Video, sender: &ComponentSender<Self>) {
        let db = &self.services.db;
        let download = log_db_error(db.completed_download(&video.id)).flatten();
        let init = video::Init {
            saved_position: log_db_error(db.resume_position(&video.id)).flatten(),
            local_file: download.as_ref().map(|d| d.path.clone()),
            local_subtitles: download
                .and_then(|d| log_db_error(db.download_subtitles(d.id)))
                .unwrap_or_default(),
            subscribed: log_db_error(db.is_subscribed(&video.channel_id)).unwrap_or(false),
            subtitle_language: log_db_error(db.subtitle_language()).flatten(),
            video,
        };
        let title = init.video.title.clone();
        let controller = VideoPage::builder()
            .launch((self.services.clone(), init))
            .forward(sender.input_sender(), Msg::from);
        self.push(&title, Page::Video(controller));
    }

    fn open_channel(&mut self, channel: Channel, sender: &ComponentSender<Self>) {
        let title = channel.name.clone();
        let controller = ChannelPage::builder()
            .launch((self.services.clone(), channel))
            .forward(sender.input_sender(), Msg::from);
        self.push(&title, Page::Channel(controller));
    }

    fn open_replies(&mut self, comment: Comment) {
        let controller = RepliesPage::builder()
            .launch((self.services.clone(), comment))
            .detach();
        self.push("Replies", Page::Replies(controller));
    }

    fn push(&mut self, title: &str, page: Page) {
        let widget: gtk::Widget = match &page {
            Page::Video(c) => c.widget().clone().upcast(),
            Page::Channel(c) => c.widget().clone().upcast(),
            Page::Replies(c) => c.widget().clone().upcast(),
        };
        let nav_page = adw::NavigationPage::new(&widget, title);
        self.navigation.push(&nav_page);
        self.pages.push((nav_page, page));
    }
}

/// Desktop layout: sections in a sidebar next to the content.
fn wide_layout(stack: &adw::ViewStack) -> adw::Layout {
    let sidebar = adw::ToolbarView::new();
    sidebar.add_top_bar(
        &adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new("Flux", ""))
            .build(),
    );
    sidebar.set_content(Some(
        &adw::ViewSwitcherSidebar::builder().stack(stack).build(),
    ));

    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar)
        .content(&adw::LayoutSlot::new("content"))
        .build();
    let layout = adw::Layout::new(&split);
    layout.set_name(Some("wide"));
    layout
}

/// Mobile layout: sections in a bottom bar.
fn narrow_layout(stack: &adw::ViewStack) -> adw::Layout {
    let view = adw::ToolbarView::new();
    view.set_content(Some(&adw::LayoutSlot::new("content")));
    view.add_bottom_bar(
        &adw::ViewSwitcherBar::builder()
            .stack(stack)
            .reveal(true)
            .build(),
    );
    let layout = adw::Layout::new(&view);
    layout.set_name(Some("narrow"));
    layout
}
