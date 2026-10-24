use relm4::adw::prelude::*;
use relm4::{
    Component, ComponentController, ComponentParts, ComponentSender, Controller, adw, gtk,
};

use super::channel::{self, ChannelPage};
use super::feed::FeedPage;
use super::search::{self, SearchPage};
use super::subscriptions::{self, SubscriptionsPage};
use super::video::{self, VideoPage};
use super::{Services, log_db_error};
use crate::core::model::{Channel, DownloadKind, Video};

/// Windows narrower than this use the mobile layout (bottom view switcher).
const NARROW_WIDTH_SP: f64 = 550.0;

pub struct App {
    services: Services,
    feed: Controller<FeedPage>,
    search: Controller<SearchPage>,
    subscriptions: Controller<SubscriptionsPage>,
    toasts: adw::ToastOverlay,
    navigation: adw::NavigationView,
    /// Pages pushed onto the navigation view, kept alive while shown.
    pages: Vec<(adw::NavigationPage, Page)>,
}

/// A page pushed onto the navigation view.
enum Page {
    Video(Controller<VideoPage>),
    Channel(Controller<ChannelPage>),
}

#[derive(Debug)]
pub enum Msg {
    OpenVideo(Video),
    OpenChannel(Channel),
    Download(Video, DownloadKind),
    SubscriptionsChanged,
    Popped(adw::NavigationPage),
}

impl From<search::Output> for Msg {
    fn from(out: search::Output) -> Self {
        match out {
            search::Output::OpenVideo(v) => Msg::OpenVideo(v),
            search::Output::OpenChannel(c) => Msg::OpenChannel(c),
        }
    }
}

impl From<channel::Output> for Msg {
    fn from(out: channel::Output) -> Self {
        match out {
            channel::Output::OpenVideo(v) => Msg::OpenVideo(v),
            channel::Output::SubscriptionsChanged => Msg::SubscriptionsChanged,
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
        let model = App {
            services,
            feed,
            search,
            subscriptions,
            toasts: adw::ToastOverlay::new(),
            navigation: adw::NavigationView::new(),
            pages: Vec::new(),
        };
        let toasts = &model.toasts;
        let navigation = &model.navigation;
        let widgets = view_output!();

        let stack = adw::ViewStack::new();
        let placeholder = |title: &str, icon: &str| {
            adw::StatusPage::builder()
                .title(title)
                .icon_name(icon)
                .build()
        };
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
        for (name, title, icon) in [
            ("history", "History", "document-open-recent-symbolic"),
            ("downloads", "Downloads", "folder-download-symbolic"),
        ] {
            add_page(name, title, icon, placeholder(title, icon).upcast_ref());
        }

        widgets.layouts.add_layout(wide_layout(&stack));
        widgets.layouts.add_layout(narrow_layout(&stack));
        widgets.layouts.set_child("content", &stack);
        widgets.layouts.set_layout_name("wide");

        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            NARROW_WIDTH_SP,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&widgets.layouts, "layout-name", Some(&"narrow".to_value()));
        root.add_breakpoint(narrow);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::OpenVideo(video) => self.open_video(video, &sender),
            Msg::OpenChannel(channel) => self.open_channel(channel, &sender),
            Msg::Download(v, _) => {
                self.toast(&format!("Downloads not implemented yet: {}", v.title));
            }
            Msg::SubscriptionsChanged => {
                self.feed.emit(super::feed::Input::Reload);
                self.subscriptions.emit(subscriptions::Msg::Reload);
            }
            Msg::Popped(page) => self.pages.retain(|(p, _)| *p != page),
        }
    }
}

impl App {
    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn open_video(&mut self, video: Video, sender: &ComponentSender<Self>) {
        let db = &self.services.db;
        let init = video::Init {
            saved_position: log_db_error(db.resume_position(&video.id)).flatten(),
            local_file: log_db_error(db.completed_download(&video.id))
                .flatten()
                .map(|d| d.path),
            subscribed: log_db_error(db.is_subscribed(&video.channel_id)).unwrap_or(false),
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

    fn push(&mut self, title: &str, page: Page) {
        let widget: gtk::Widget = match &page {
            Page::Video(c) => c.widget().clone().upcast(),
            Page::Channel(c) => c.widget().clone().upcast(),
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
            .show_end_title_buttons(false)
            .title_widget(&adw::WindowTitle::new("Flux", ""))
            .build(),
    );
    sidebar.set_content(Some(
        &adw::ViewSwitcherSidebar::builder().stack(stack).build(),
    ));

    let content = adw::ToolbarView::new();
    let title = adw::WindowTitle::new("", "");
    stack
        .bind_property("visible-child-name", &title, "title")
        .transform_to(|binding, _: Option<String>| {
            let stack = binding.source()?.downcast::<adw::ViewStack>().ok()?;
            stack.page(&stack.visible_child()?).title()
        })
        .sync_create()
        .build();
    content.add_top_bar(&adw::HeaderBar::builder().title_widget(&title).build());
    content.set_content(Some(&adw::LayoutSlot::new("content")));

    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar)
        .content(&content)
        .build();
    let layout = adw::Layout::new(&split);
    layout.set_name(Some("wide"));
    layout
}

/// Mobile layout: sections in a bottom bar.
fn narrow_layout(stack: &adw::ViewStack) -> adw::Layout {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
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
