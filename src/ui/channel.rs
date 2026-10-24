//! Channel page: a channel's latest uploads.

use relm4::adw::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use super::video::Load;
use super::{Services, log_db_error, widgets};
use crate::core::model::{Channel, Video};

/// GTK-independent state of a channel page.
#[derive(Debug)]
pub struct State {
    pub channel: Channel,
    pub videos: Load<Vec<Video>>,
    pub subscribed: bool,
}

#[derive(Debug)]
pub enum Msg {
    Loaded(Result<(Channel, Vec<Video>), String>),
    ToggleSubscribe,
    Activate(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Load(String),
    Subscribe(Channel),
    Unsubscribe(String),
    OpenVideo(Video),
}

impl State {
    pub fn new(channel: Channel, subscribed: bool) -> (Self, Vec<Effect>) {
        let effects = vec![Effect::Load(channel.id.clone())];
        let state = State {
            channel,
            videos: Load::Loading,
            subscribed,
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
            Msg::ToggleSubscribe => {
                self.subscribed = !self.subscribed;
                return vec![if self.subscribed {
                    Effect::Subscribe(self.channel.clone())
                } else {
                    Effect::Unsubscribe(self.channel.id.clone())
                }];
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
}

pub struct ChannelPage {
    state: State,
    services: Services,
    list: gtk::ListBox,
}

#[derive(Debug)]
pub enum Output {
    OpenVideo(Video),
    SubscriptionsChanged,
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

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 18,
                        set_margin_all: 12,

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 12,
                            set_margin_top: 12,

                            adw::Avatar {
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

                        gtk::Stack {
                            #[watch]
                            set_visible_child_name: match &model.state.videos {
                                Load::Loading => "loading",
                                Load::Loaded(_) => "list",
                                Load::Failed(_) => "failed",
                            },

                            add_named[Some("loading")] = &adw::Spinner {
                                set_size_request: (32, 32),
                            },
                            add_named[Some("failed")] = &adw::StatusPage {
                                set_icon_name: Some("dialog-error-symbolic"),
                                set_title: "Cannot Load Channel",
                                add_css_class: "compact",
                                #[watch]
                                set_description: match &model.state.videos {
                                    Load::Failed(e) => Some(e.as_str()),
                                    _ => None,
                                },
                            },
                            #[local_ref]
                            add_named[Some("list")] = list -> gtk::ListBox {
                                connect_row_activated[sender] => move |_, row| {
                                    sender.input(Msg::Activate(row.index() as usize));
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
        };
        let list = &model.list;
        let widgets = view_output!();
        model.run(effects, &sender);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        let loaded = matches!(msg, Msg::Loaded(_));
        let effects = self.state.update(msg);
        self.run(effects, &sender);
        if loaded {
            self.list.remove_all();
            if let Load::Loaded(videos) = &self.state.videos {
                for video in videos {
                    self.list.append(&widgets::video_row(video));
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
                Effect::Subscribe(channel) => {
                    log_db_error(self.services.db.subscribe(&channel));
                    let _ = sender.output(Output::SubscriptionsChanged);
                }
                Effect::Unsubscribe(id) => {
                    log_db_error(self.services.db.unsubscribe(&id));
                    let _ = sender.output(Output::SubscriptionsChanged);
                }
                Effect::OpenVideo(video) => {
                    let _ = sender.output(Output::OpenVideo(video));
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
    fn toggling_subscription() {
        let (mut state, _) = State::new(channel("Chan"), false);
        assert_eq!(
            state.update(Msg::ToggleSubscribe),
            [Effect::Subscribe(channel("Chan"))]
        );
        assert_eq!(
            state.update(Msg::ToggleSubscribe),
            [Effect::Unsubscribe("UC1".into())]
        );
        assert!(!state.subscribed);
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
