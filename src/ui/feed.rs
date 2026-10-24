//! Feed page: new videos from subscribed channels.

use chrono::{DateTime, Utc};
use relm4::adw::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use super::{Services, format, log_db_error, widgets};
use crate::core::backend::{self, FeedRefresh};
use crate::core::feed;
use crate::core::model::Video;

/// The feed is refreshed automatically when it is older than this.
pub const REFRESH_INTERVAL_MINS: i64 = 30;

/// Whether a feed last refreshed at `last` should be refreshed at `now`.
pub fn needs_refresh(last: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last.is_none_or(|last| now - last >= chrono::Duration::minutes(REFRESH_INTERVAL_MINS))
}

/// GTK-independent state of the feed page.
#[derive(Debug, Default)]
pub struct State {
    pub videos: Vec<Video>,
    pub refreshing: bool,
    /// Channels whose feed could not be fetched, with the error message.
    pub errors: Vec<(String, String)>,
    channel_ids: Vec<String>,
    loaded: bool,
}

#[derive(Debug)]
pub enum Msg {
    /// Cached feed and current subscriptions, on startup or when the
    /// subscriptions changed.
    Load {
        videos: Vec<Video>,
        refreshed_at: Option<DateTime<Utc>>,
        channel_ids: Vec<String>,
        now: DateTime<Utc>,
    },
    Refresh,
    Refreshed(FeedRefresh),
    Activate(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Refresh(Vec<String>),
    Store(Vec<Video>),
    OpenVideo(Video),
}

impl State {
    pub fn has_subscriptions(&self) -> bool {
        !self.channel_ids.is_empty()
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Load {
                videos,
                refreshed_at,
                channel_ids,
                now,
            } => {
                let added = channel_ids.iter().any(|id| !self.channel_ids.contains(id));
                let first_load = !self.loaded;
                self.loaded = true;
                self.videos = videos;
                self.channel_ids = channel_ids;
                if needs_refresh(refreshed_at, now) || (added && !first_load) {
                    return self.refresh();
                }
            }
            Msg::Refresh => return self.refresh(),
            Msg::Refreshed(refresh) => {
                self.refreshing = false;
                let failed: Vec<_> = refresh.errors.iter().map(|(id, _)| id).collect();
                let kept = self
                    .videos
                    .drain(..)
                    .filter(|v| failed.contains(&&v.channel_id))
                    .collect();
                self.videos = feed::aggregate([refresh.videos, kept]);
                self.errors = refresh.errors;
                return vec![Effect::Store(self.videos.clone())];
            }
            Msg::Activate(index) => {
                if let Some(video) = self.videos.get(index) {
                    return vec![Effect::OpenVideo(video.clone())];
                }
            }
        }
        Vec::new()
    }

    fn refresh(&mut self) -> Vec<Effect> {
        if self.refreshing || self.channel_ids.is_empty() {
            return Vec::new();
        }
        self.refreshing = true;
        vec![Effect::Refresh(self.channel_ids.clone())]
    }
}

pub struct FeedPage {
    state: State,
    services: Services,
    list: gtk::ListBox,
    refreshed_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub enum Input {
    State(Msg),
    /// Re-reads the cached feed and subscriptions from the database.
    Reload,
}

#[relm4::component(pub)]
impl Component for FeedPage {
    type Init = Services;
    type Input = Input;
    type Output = Video;
    type CommandOutput = FeedRefresh;

    view! {
        adw::ToolbarView {
            add_top_bar = &adw::HeaderBar {
                pack_end: &widgets::primary_menu(),
                #[wrap(Some)]
                set_title_widget = &adw::WindowTitle {
                    set_title: "Feed",
                    #[watch]
                    set_subtitle: &model.status_text(),
                },
                pack_start = &gtk::Button {
                    set_icon_name: "view-refresh-symbolic",
                    set_tooltip_text: Some("Refresh"),
                    #[watch]
                    set_visible: model.state.has_subscriptions(),
                    #[watch]
                    set_sensitive: !model.state.refreshing,
                    connect_clicked => Input::State(Msg::Refresh),
                },
            },
            add_top_bar = &adw::Banner {
                set_button_label: Some("Retry"),
                #[watch]
                set_title: &format::failed_channels(model.state.errors.len()),
                #[watch]
                set_revealed: !model.state.errors.is_empty() && !model.state.refreshing,
                connect_button_clicked => Input::State(Msg::Refresh),
            },

            #[wrap(Some)]
            set_content = &gtk::Stack {
                add_named[Some("empty")] = &adw::StatusPage {
                    set_icon_name: Some("view-list-symbolic"),
                    set_title: "No Subscriptions",
                    set_description: Some("Subscribe to channels to see their new videos here"),
                },
                add_named[Some("loading")] = &widgets::loading_page("Loading Feed"),
                add_named[Some("list")] = &gtk::ScrolledWindow {
                    set_hscrollbar_policy: gtk::PolicyType::Never,

                    adw::Clamp {
                        set_maximum_size: 800,
                        set_margin_all: 12,

                        #[local_ref]
                        list -> gtk::ListBox {
                            connect_row_activated[sender] => move |_, row| {
                                sender.input(Input::State(Msg::Activate(row.index() as usize)));
                            },
                        },
                    },
                },

                #[watch]
                set_visible_child_name: if !model.state.has_subscriptions() {
                    "empty"
                } else if model.state.videos.is_empty() && model.state.refreshing {
                    "loading"
                } else {
                    "list"
                },
            },
        }
    }

    fn init(
        services: Services,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = FeedPage {
            state: State::default(),
            services,
            list: widgets::boxed_list(),
            refreshed_at: None,
        };
        let list = &model.list;
        let widgets = view_output!();
        sender.input(Input::Reload);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, input: Input, sender: ComponentSender<Self>, _root: &Self::Root) {
        let msg = match input {
            Input::State(msg) => msg,
            Input::Reload => {
                let db = &self.services.db;
                self.refreshed_at = log_db_error(db.feed_refreshed_at()).flatten();
                Msg::Load {
                    videos: log_db_error(db.feed()).unwrap_or_default(),
                    refreshed_at: self.refreshed_at,
                    channel_ids: log_db_error(db.subscriptions())
                        .unwrap_or_default()
                        .into_iter()
                        .map(|c| c.id)
                        .collect(),
                    now: Utc::now(),
                }
            }
        };
        let reload = matches!(msg, Msg::Load { .. } | Msg::Refreshed(_));
        for effect in self.state.update(msg) {
            match effect {
                Effect::Refresh(ids) => {
                    let backend = self.services.backend.clone();
                    sender.oneshot_command(
                        async move { backend::refresh_feed(&*backend, &ids).await },
                    );
                }
                Effect::Store(videos) => {
                    let now = Utc::now();
                    log_db_error(self.services.db.store_feed(&videos, now));
                    self.refreshed_at = Some(now);
                }
                Effect::OpenVideo(video) => {
                    let _ = sender.output(video);
                }
            }
        }
        if reload {
            self.list.remove_all();
            for video in &self.state.videos {
                self.list
                    .append(&widgets::with_chevron(widgets::video_row(video)));
            }
        }
    }

    fn update_cmd(
        &mut self,
        refresh: FeedRefresh,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        self.update(Input::State(Msg::Refreshed(refresh)), sender, root);
    }
}

impl FeedPage {
    fn status_text(&self) -> String {
        if !self.state.has_subscriptions() {
            return String::new();
        }
        if self.state.refreshing {
            return "Refreshing…".into();
        }
        match self.refreshed_at {
            Some(at) => format!("Updated {}", format::relative_date(at, Utc::now())),
            None => "Not refreshed yet".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-10-24T12:00:00Z".parse().unwrap()
    }

    fn mins_ago(mins: i64) -> Option<DateTime<Utc>> {
        Some(now() - chrono::Duration::minutes(mins))
    }

    fn video(id: &str, channel_id: &str, published: i64) -> Video {
        Video {
            id: id.into(),
            title: id.into(),
            channel_id: channel_id.into(),
            channel_name: channel_id.into(),
            published: DateTime::from_timestamp(published, 0),
            duration_secs: None,
            thumbnail_url: None,
        }
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn load(
        state: &mut State,
        refreshed_at: Option<DateTime<Utc>>,
        channels: &[&str],
    ) -> Vec<Effect> {
        state.update(Msg::Load {
            videos: vec![video("old", "A", 1)],
            refreshed_at,
            channel_ids: ids(channels),
            now: now(),
        })
    }

    #[test]
    fn refresh_policy() {
        assert!(needs_refresh(None, now()));
        assert!(!needs_refresh(mins_ago(29), now()));
        assert!(needs_refresh(mins_ago(30), now()));
    }

    #[test]
    fn loading_shows_cached_feed_and_refreshes_when_stale() {
        let mut state = State::default();
        assert_eq!(
            load(&mut state, mins_ago(60), &["A", "B"]),
            [Effect::Refresh(ids(&["A", "B"]))]
        );
        assert_eq!(state.videos, [video("old", "A", 1)]);
        assert!(state.refreshing);
    }

    #[test]
    fn loading_fresh_feed_does_not_refresh() {
        let mut state = State::default();
        assert!(load(&mut state, mins_ago(5), &["A"]).is_empty());
        assert!(!state.refreshing);
    }

    #[test]
    fn no_subscriptions_never_refresh() {
        let mut state = State::default();
        assert!(load(&mut state, None, &[]).is_empty());
        assert!(!state.has_subscriptions());
        assert!(state.update(Msg::Refresh).is_empty());
    }

    #[test]
    fn new_subscription_triggers_refresh() {
        let mut state = State::default();
        load(&mut state, mins_ago(5), &["A"]);
        assert!(
            load(&mut state, mins_ago(5), &[]).is_empty(),
            "unsubscribing"
        );
        assert_eq!(
            load(&mut state, mins_ago(5), &["A"]),
            [Effect::Refresh(ids(&["A"]))]
        );
    }

    #[test]
    fn manual_refresh_only_once_at_a_time() {
        let mut state = State::default();
        load(&mut state, mins_ago(5), &["A"]);
        assert_eq!(state.update(Msg::Refresh), [Effect::Refresh(ids(&["A"]))]);
        assert!(state.update(Msg::Refresh).is_empty());
    }

    #[test]
    fn refreshed_feed_is_shown_and_stored() {
        let mut state = State::default();
        load(&mut state, None, &["A"]);
        let videos = vec![video("new", "A", 2)];

        let effects = state.update(Msg::Refreshed(FeedRefresh {
            videos: videos.clone(),
            errors: vec![],
        }));

        assert_eq!(effects, [Effect::Store(videos.clone())]);
        assert_eq!(state.videos, videos);
        assert!(!state.refreshing);
    }

    #[test]
    fn failed_channels_keep_their_cached_videos() {
        let mut state = State::default();
        load(&mut state, None, &["A", "B"]);
        let refresh = FeedRefresh {
            videos: vec![video("b1", "B", 5)],
            errors: vec![("A".into(), "offline".into())],
        };

        let effects = state.update(Msg::Refreshed(refresh));

        let expected = vec![video("b1", "B", 5), video("old", "A", 1)];
        assert_eq!(effects, [Effect::Store(expected.clone())]);
        assert_eq!(state.videos, expected);
        assert_eq!(state.errors, [("A".to_string(), "offline".to_string())]);
    }

    #[test]
    fn activating_opens_video() {
        let mut state = State::default();
        load(&mut state, mins_ago(5), &["A"]);
        assert_eq!(
            state.update(Msg::Activate(0)),
            [Effect::OpenVideo(video("old", "A", 1))]
        );
        assert!(state.update(Msg::Activate(1)).is_empty());
    }
}
