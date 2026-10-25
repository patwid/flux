//! Channel page: a channel's latest uploads and its playlists.

use relm4::adw::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use super::video::Load;
use super::{Services, format, images, log_db_error, widgets};
use crate::core::model::{Channel, Playlist, Video};

/// The lists of a channel, shown one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Videos,
    Playlists,
}

impl Tab {
    /// The name of the tab's toggle.
    pub fn name(self) -> &'static str {
        match self {
            Tab::Videos => "videos",
            Tab::Playlists => "playlists",
        }
    }

    /// The tab with toggle `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        [Tab::Videos, Tab::Playlists]
            .into_iter()
            .find(|tab| tab.name() == name)
    }
}

/// What is shown below the channel's header.
#[derive(Debug, PartialEq, Eq)]
pub enum Content<'a> {
    Loading,
    Failed(&'a str),
    Videos,
    Playlists,
    NoPlaylists,
}

impl Content<'_> {
    /// The name of the content's stack page.
    pub fn name(&self) -> &'static str {
        match self {
            Content::Loading => "loading",
            Content::Failed(_) => "failed",
            Content::Videos => "videos",
            Content::Playlists => "playlists",
            Content::NoPlaylists => "no-playlists",
        }
    }
}

/// GTK-independent state of a channel page.
#[derive(Debug)]
pub struct State {
    pub channel: Channel,
    pub videos: Load<Vec<Video>>,
    /// Loaded when their tab is first shown.
    pub playlists: Option<Load<Vec<Playlist>>>,
    pub subscribed: bool,
    pub tab: Tab,
}

#[derive(Debug)]
pub enum Msg {
    Loaded(Result<(Channel, Vec<Video>), String>),
    PlaylistsLoaded(Result<Vec<Playlist>, String>),
    /// Loads the shown tab again after it failed to load.
    Retry,
    ToggleSubscribe,
    SelectTab(Tab),
    Activate(usize),
    ActivatePlaylist(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Load(String),
    LoadPlaylists(String),
    Subscribe(Channel),
    Unsubscribe(String),
    /// Tells the user what happened.
    Toast(String),
    OpenVideo(Video),
    OpenPlaylist(Playlist),
}

impl State {
    pub fn new(channel: Channel, subscribed: bool) -> (Self, Vec<Effect>) {
        let effects = vec![Effect::Load(channel.id.clone())];
        let state = State {
            channel,
            videos: Load::Loading,
            playlists: None,
            subscribed,
            tab: Tab::Videos,
        };
        (state, effects)
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Loaded(Ok((channel, videos))) => {
                self.channel = channel;
                self.videos = Load::Loaded(videos);
                if self.subscribed {
                    return vec![Effect::Subscribe(self.channel.clone())];
                }
            }
            Msg::Loaded(Err(e)) => self.videos = Load::Failed(e),
            Msg::PlaylistsLoaded(result) => {
                self.playlists = Some(match result {
                    Ok(playlists) => Load::Loaded(playlists),
                    Err(e) => Load::Failed(e),
                });
            }
            Msg::Retry => match self.tab {
                Tab::Videos => {
                    if let Load::Failed(_) = self.videos {
                        self.videos = Load::Loading;
                        return vec![Effect::Load(self.channel.id.clone())];
                    }
                }
                Tab::Playlists => {
                    if let Some(Load::Failed(_)) = self.playlists {
                        return self.load_playlists();
                    }
                }
            },
            Msg::ToggleSubscribe => {
                self.subscribed = !self.subscribed;
                return vec![
                    if self.subscribed {
                        Effect::Subscribe(self.channel.clone())
                    } else {
                        Effect::Unsubscribe(self.channel.id.clone())
                    },
                    Effect::Toast(format::subscription(&self.channel.name, self.subscribed)),
                ];
            }
            Msg::SelectTab(tab) => {
                self.tab = tab;
                if tab == Tab::Playlists && self.playlists.is_none() {
                    return self.load_playlists();
                }
            }
            Msg::ActivatePlaylist(index) => {
                if let Some(Load::Loaded(playlists)) = &self.playlists
                    && let Some(playlist) = playlists.get(index)
                {
                    return vec![Effect::OpenPlaylist(playlist.clone())];
                }
            }
            Msg::Activate(index) => {
                if let Load::Loaded(videos) = &self.videos
                    && let Some(video) = videos.get(index)
                {
                    return vec![Effect::OpenVideo(video.clone())];
                }
            }
        }
        Vec::new()
    }

    fn load_playlists(&mut self) -> Vec<Effect> {
        self.playlists = Some(Load::Loading);
        vec![Effect::LoadPlaylists(self.channel.id.clone())]
    }

    pub fn content(&self) -> Content<'_> {
        match self.tab {
            Tab::Videos => match &self.videos {
                Load::Loading => Content::Loading,
                Load::Failed(e) => Content::Failed(e),
                Load::Loaded(_) => Content::Videos,
            },
            Tab::Playlists => match &self.playlists {
                None | Some(Load::Loading) => Content::Loading,
                Some(Load::Failed(e)) => Content::Failed(e),
                Some(Load::Loaded(playlists)) if playlists.is_empty() => Content::NoPlaylists,
                Some(Load::Loaded(_)) => Content::Playlists,
            },
        }
    }
}

pub struct ChannelPage {
    state: State,
    services: Services,
    list: gtk::ListBox,
    playlist_list: gtk::ListBox,
    avatar: adw::Avatar,
}

#[derive(Debug)]
pub enum Output {
    OpenVideo(Video),
    OpenPlaylist(Playlist),
    SubscriptionsChanged,
    Toast(String),
}

#[relm4::component(pub)]
impl Component for ChannelPage {
    type Init = (Services, Channel);
    type Input = Msg;
    type Output = Output;
    type CommandOutput = Msg;

    view! {
        adw::ToolbarView {
            add_top_bar = &adw::HeaderBar {},

            #[wrap(Some)]
            set_content = &gtk::ScrolledWindow {
                set_hscrollbar_policy: gtk::PolicyType::Never,

                adw::Clamp {
                    set_maximum_size: 800,
                    set_margin_all: 12,

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 18,

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 12,
                            set_margin_top: 12,

                            #[local_ref]
                            avatar -> adw::Avatar {
                                set_size: 96,
                                set_show_initials: true,
                                #[watch]
                                set_text: Some(&model.state.channel.name),
                            },
                            gtk::Label {
                                add_css_class: "title-2",
                                set_wrap: true,
                                set_justify: gtk::Justification::Center,
                                #[watch]
                                set_label: &model.state.channel.name,
                            },
                            gtk::Button {
                                set_halign: gtk::Align::Center,
                                add_css_class: "pill",
                                #[watch]
                                set_label: if model.state.subscribed { "Unsubscribe" } else { "Subscribe" },
                                #[watch]
                                set_css_classes: if model.state.subscribed { &["pill"] } else { &["pill", "suggested-action"] },
                                connect_clicked => Msg::ToggleSubscribe,
                            },
                        },

                        #[name = "tabs"]
                        adw::ToggleGroup {
                            set_homogeneous: true,
                            add: adw::Toggle::builder().name(Tab::Videos.name()).label("Videos").build(),
                            add: adw::Toggle::builder().name(Tab::Playlists.name()).label("Playlists").build(),
                            #[watch]
                            #[block_signal(tab_handler)]
                            set_active_name: Some(model.state.tab.name()),
                            connect_active_name_notify[sender] => move |tabs| {
                                if let Some(tab) = tabs.active_name().as_deref().and_then(Tab::from_name) {
                                    sender.input(Msg::SelectTab(tab));
                                }
                            } @tab_handler,
                        },

                        gtk::Stack {
                            set_vhomogeneous: false,
                            #[watch]
                            set_visible_child_name: model.state.content().name(),

                            add_named[Some("loading")] = &widgets::loading_page("") {
                                add_css_class: "compact",
                            },
                            add_named[Some("failed")] = &adw::StatusPage {
                                set_icon_name: Some("dialog-error-symbolic"),
                                add_css_class: "compact",
                                #[watch]
                                set_title: match model.state.tab {
                                    Tab::Videos => "Cannot Load Channel",
                                    Tab::Playlists => "Cannot Load Playlists",
                                },
                                #[watch]
                                set_description: match model.state.content() {
                                    Content::Failed(e) => Some(e),
                                    _ => None,
                                },

                                #[wrap(Some)]
                                set_child = &gtk::Button {
                                    set_label: "Retry",
                                    set_halign: gtk::Align::Center,
                                    add_css_class: "pill",
                                    connect_clicked => Msg::Retry,
                                },
                            },
                            add_named[Some("no-playlists")] = &adw::StatusPage {
                                set_icon_name: Some("view-list-symbolic"),
                                set_title: "No Playlists",
                                add_css_class: "compact",
                            },
                            #[local_ref]
                            add_named[Some("videos")] = list -> gtk::ListBox {
                                connect_row_activated[sender] => move |_, row| {
                                    sender.input(Msg::Activate(row.index() as usize));
                                },
                            },
                            #[local_ref]
                            add_named[Some("playlists")] = playlist_list -> gtk::ListBox {
                                connect_row_activated[sender] => move |_, row| {
                                    sender.input(Msg::ActivatePlaylist(row.index() as usize));
                                },
                            },
                        },
                    },
                },
            },
        }
    }

    fn init(
        (services, channel): Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let subscribed = log_db_error(services.db.is_subscribed(&channel.id)).unwrap_or(false);
        let (state, effects) = State::new(channel, subscribed);
        let model = ChannelPage {
            state,
            services,
            list: widgets::boxed_list(),
            playlist_list: widgets::boxed_list(),
            avatar: adw::Avatar::default(),
        };
        images::set_avatar(&model.avatar, model.state.channel.avatar_url.as_deref());
        let list = &model.list;
        let playlist_list = &model.playlist_list;
        let avatar = &model.avatar;
        let widgets = view_output!();
        model.run(effects, &sender);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        let loaded = matches!(msg, Msg::Loaded(_));
        let playlists_loaded = matches!(msg, Msg::PlaylistsLoaded(_));
        let effects = self.state.update(msg);
        self.run(effects, &sender);
        if loaded {
            images::set_avatar(&self.avatar, self.state.channel.avatar_url.as_deref());
            self.list.remove_all();
            if let Load::Loaded(videos) = &self.state.videos {
                for video in videos {
                    self.list
                        .append(&widgets::with_chevron(widgets::video_row(video)));
                }
            }
        }
        if playlists_loaded {
            self.playlist_list.remove_all();
            if let Some(Load::Loaded(playlists)) = &self.state.playlists {
                for playlist in playlists {
                    self.playlist_list.append(&widgets::playlist_row(playlist));
                }
            }
        }
    }

    fn update_cmd(&mut self, msg: Msg, sender: ComponentSender<Self>, root: &Self::Root) {
        self.update(msg, sender, root);
    }
}

impl ChannelPage {
    fn run(&self, effects: Vec<Effect>, sender: &ComponentSender<Self>) {
        for effect in effects {
            match effect {
                Effect::Load(id) => {
                    let backend = self.services.backend.clone();
                    sender.oneshot_command(async move {
                        Msg::Loaded(backend.channel(&id).await.map_err(|e| e.to_string()))
                    });
                }
                Effect::LoadPlaylists(id) => {
                    let backend = self.services.backend.clone();
                    sender.oneshot_command(async move {
                        Msg::PlaylistsLoaded(
                            backend
                                .channel_playlists(&id)
                                .await
                                .map_err(|e| e.to_string()),
                        )
                    });
                }
                Effect::Subscribe(channel) => {
                    log_db_error(self.services.db.subscribe(&channel));
                    let _ = sender.output(Output::SubscriptionsChanged);
                }
                Effect::Unsubscribe(id) => {
                    log_db_error(self.services.db.unsubscribe(&id));
                    let _ = sender.output(Output::SubscriptionsChanged);
                }
                Effect::Toast(text) => {
                    let _ = sender.output(Output::Toast(text));
                }
                Effect::OpenVideo(video) => {
                    let _ = sender.output(Output::OpenVideo(video));
                }
                Effect::OpenPlaylist(playlist) => {
                    let _ = sender.output(Output::OpenPlaylist(playlist));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(name: &str) -> Channel {
        Channel {
            id: "UC1".into(),
            name: name.into(),
            avatar_url: None,
        }
    }

    fn video(id: &str) -> Video {
        Video {
            id: id.into(),
            title: id.into(),
            channel_id: "UC1".into(),
            channel_name: "Chan".into(),
            published: None,
            duration_secs: None,
            thumbnail_url: None,
        }
    }

    #[test]
    fn opening_loads_channel() {
        let (state, effects) = State::new(channel("Chan"), true);
        assert_eq!(effects, [Effect::Load("UC1".into())]);
        assert_eq!(state.videos, Load::Loading);
        assert!(state.subscribed);
    }

    #[test]
    fn loaded_channel_replaces_partial_info() {
        let (mut state, _) = State::new(channel("Partial"), false);
        let mut full = channel("Full");
        full.avatar_url = Some("https://a".into());

        state.update(Msg::Loaded(Ok((full.clone(), vec![video("a")]))));

        assert_eq!(state.channel, full);
        assert_eq!(state.videos, Load::Loaded(vec![video("a")]));
    }

    #[test]
    fn subscribed_channel_is_updated_when_loaded() {
        let (mut state, _) = State::new(channel("Old"), true);
        let effects = state.update(Msg::Loaded(Ok((channel("New"), vec![]))));
        assert_eq!(
            effects,
            [Effect::Subscribe(channel("New"))],
            "refresh stored name"
        );
    }

    #[test]
    fn load_failure_is_shown() {
        let (mut state, _) = State::new(channel("Chan"), false);
        state.update(Msg::Loaded(Err("offline".into())));
        assert_eq!(state.videos, Load::Failed("offline".into()));
    }

    #[test]
    fn failed_load_can_be_retried() {
        let (mut state, _) = State::new(channel("Chan"), false);
        assert!(state.update(Msg::Retry).is_empty(), "still loading");

        state.update(Msg::Loaded(Err("offline".into())));
        assert_eq!(state.update(Msg::Retry), [Effect::Load("UC1".into())]);
        assert_eq!(state.videos, Load::Loading);
    }

    #[test]
    fn toggling_subscription() {
        let (mut state, _) = State::new(channel("Chan"), false);
        assert_eq!(
            state.update(Msg::ToggleSubscribe),
            [
                Effect::Subscribe(channel("Chan")),
                Effect::Toast("Subscribed to “Chan”".into())
            ]
        );
        assert_eq!(
            state.update(Msg::ToggleSubscribe),
            [
                Effect::Unsubscribe("UC1".into()),
                Effect::Toast("Unsubscribed from “Chan”".into())
            ]
        );
        assert!(!state.subscribed);
    }

    fn playlist(id: &str) -> Playlist {
        Playlist {
            id: id.into(),
            name: id.into(),
            thumbnail_url: None,
            video_count: None,
        }
    }

    #[test]
    fn videos_are_shown_first() {
        let (state, _) = State::new(channel("Chan"), false);
        assert_eq!(state.tab, Tab::Videos);
        assert_eq!(state.content(), Content::Loading);
    }

    #[test]
    fn playlists_load_when_first_shown() {
        let (mut state, _) = State::new(channel("Chan"), false);
        assert_eq!(
            state.update(Msg::SelectTab(Tab::Playlists)),
            [Effect::LoadPlaylists("UC1".into())]
        );
        assert_eq!(state.content(), Content::Loading);

        state.update(Msg::PlaylistsLoaded(Ok(vec![playlist("PL1")])));
        assert_eq!(state.content(), Content::Playlists);

        assert!(state.update(Msg::SelectTab(Tab::Videos)).is_empty());
        assert!(
            state.update(Msg::SelectTab(Tab::Playlists)).is_empty(),
            "already loaded"
        );
    }

    #[test]
    fn channel_without_playlists() {
        let (mut state, _) = State::new(channel("Chan"), false);
        state.update(Msg::SelectTab(Tab::Playlists));
        state.update(Msg::PlaylistsLoaded(Ok(vec![])));
        assert_eq!(state.content(), Content::NoPlaylists);
    }

    #[test]
    fn retry_loads_the_failed_tab() {
        let (mut state, _) = State::new(channel("Chan"), false);
        state.update(Msg::Loaded(Ok((channel("Chan"), vec![video("a")]))));
        state.update(Msg::SelectTab(Tab::Playlists));
        state.update(Msg::PlaylistsLoaded(Err("offline".into())));
        assert_eq!(state.content(), Content::Failed("offline"));

        assert_eq!(
            state.update(Msg::Retry),
            [Effect::LoadPlaylists("UC1".into())]
        );
        assert_eq!(state.content(), Content::Loading);

        state.update(Msg::SelectTab(Tab::Videos));
        assert!(state.update(Msg::Retry).is_empty(), "videos loaded");
    }

    #[test]
    fn activating_opens_playlist() {
        let (mut state, _) = State::new(channel("Chan"), false);
        state.update(Msg::SelectTab(Tab::Playlists));
        assert!(state.update(Msg::ActivatePlaylist(0)).is_empty());
        state.update(Msg::PlaylistsLoaded(Ok(vec![playlist("PL1")])));
        assert_eq!(
            state.update(Msg::ActivatePlaylist(0)),
            [Effect::OpenPlaylist(playlist("PL1"))]
        );
    }

    #[test]
    fn activating_opens_video() {
        let (mut state, _) = State::new(channel("Chan"), false);
        assert!(state.update(Msg::Activate(0)).is_empty());
        state.update(Msg::Loaded(Ok((channel("Chan"), vec![video("a")]))));
        assert_eq!(
            state.update(Msg::Activate(0)),
            [Effect::OpenVideo(video("a"))]
        );
    }
}
