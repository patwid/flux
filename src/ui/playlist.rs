//! Playlist page: the videos of a playlist.

use relm4::adw::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use super::video::Load;
use super::{Services, format, widgets};
use crate::core::model::{Playlist, Video};

/// GTK-independent state of a playlist page.
#[derive(Debug)]
pub struct State {
    pub playlist: Playlist,
    pub videos: Load<Vec<Video>>,
}

#[derive(Debug)]
pub enum Msg {
    Loaded(Result<Vec<Video>, String>),
    /// Loads the playlist again after it failed to load.
    Retry,
    Activate(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Load(String),
    OpenVideo(Video),
}

impl State {
    pub fn new(playlist: Playlist) -> (Self, Vec<Effect>) {
        let effects = vec![Effect::Load(playlist.id.clone())];
        let state = State {
            playlist,
            videos: Load::Loading,
        };
        (state, effects)
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Loaded(Ok(videos)) => self.videos = Load::Loaded(videos),
            Msg::Loaded(Err(e)) => self.videos = Load::Failed(e),
            Msg::Retry => {
                if let Load::Failed(_) = self.videos {
                    self.videos = Load::Loading;
                    return vec![Effect::Load(self.playlist.id.clone())];
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
}

pub struct PlaylistPage {
    state: State,
    services: Services,
    list: gtk::ListBox,
}

#[relm4::component(pub)]
impl Component for PlaylistPage {
    type Init = (Services, Playlist);
    type Input = Msg;
    type Output = Video;
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
                            set_spacing: 6,
                            set_margin_top: 12,

                            gtk::Label {
                                add_css_class: "title-2",
                                set_wrap: true,
                                set_justify: gtk::Justification::Center,
                                set_label: &model.state.playlist.name,
                            },
                            gtk::Label {
                                add_css_class: "dimmed",
                                set_visible: model.state.playlist.video_count.is_some(),
                                set_label: &model.state.playlist.video_count.map(format::videos).unwrap_or_default(),
                            },
                        },

                        gtk::Stack {
                            #[watch]
                            set_visible_child_name: match &model.state.videos {
                                Load::Loading => "loading",
                                Load::Loaded(videos) if videos.is_empty() => "empty",
                                Load::Loaded(_) => "list",
                                Load::Failed(_) => "failed",
                            },

                            add_named[Some("loading")] = &widgets::loading_page("") {
                                add_css_class: "compact",
                            },
                            add_named[Some("empty")] = &adw::StatusPage {
                                set_icon_name: Some("view-list-symbolic"),
                                set_title: "No Videos",
                                add_css_class: "compact",
                            },
                            add_named[Some("failed")] = &adw::StatusPage {
                                set_icon_name: Some("dialog-error-symbolic"),
                                set_title: "Cannot Load Playlist",
                                add_css_class: "compact",
                                #[watch]
                                set_description: match &model.state.videos {
                                    Load::Failed(e) => Some(e.as_str()),
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
        (services, playlist): Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let (state, effects) = State::new(playlist);
        let model = PlaylistPage {
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
                    self.list
                        .append(&widgets::with_chevron(widgets::video_row(video)));
                }
            }
        }
    }

    fn update_cmd(&mut self, msg: Msg, sender: ComponentSender<Self>, root: &Self::Root) {
        self.update(msg, sender, root);
    }
}

impl PlaylistPage {
    fn run(&self, effects: Vec<Effect>, sender: &ComponentSender<Self>) {
        for effect in effects {
            match effect {
                Effect::Load(id) => {
                    let backend = self.services.backend.clone();
                    sender.oneshot_command(async move {
                        Msg::Loaded(
                            backend
                                .playlist_videos(&id)
                                .await
                                .map_err(|e| e.to_string()),
                        )
                    });
                }
                Effect::OpenVideo(video) => {
                    let _ = sender.output(video);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist() -> Playlist {
        Playlist {
            id: "PL1".into(),
            name: "List".into(),
            thumbnail_url: None,
            video_count: Some(2),
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
    fn opening_loads_playlist() {
        let (state, effects) = State::new(playlist());
        assert_eq!(effects, [Effect::Load("PL1".into())]);
        assert_eq!(state.videos, Load::Loading);
    }

    #[test]
    fn loaded_videos_are_shown() {
        let (mut state, _) = State::new(playlist());
        assert!(state.update(Msg::Loaded(Ok(vec![video("a")]))).is_empty());
        assert_eq!(state.videos, Load::Loaded(vec![video("a")]));
    }

    #[test]
    fn failed_load_can_be_retried() {
        let (mut state, _) = State::new(playlist());
        assert!(state.update(Msg::Retry).is_empty(), "still loading");

        state.update(Msg::Loaded(Err("offline".into())));
        assert_eq!(state.videos, Load::Failed("offline".into()));
        assert_eq!(state.update(Msg::Retry), [Effect::Load("PL1".into())]);
        assert_eq!(state.videos, Load::Loading);
    }

    #[test]
    fn activating_opens_video() {
        let (mut state, _) = State::new(playlist());
        assert!(state.update(Msg::Activate(0)).is_empty());
        state.update(Msg::Loaded(Ok(vec![video("a"), video("b")])));
        assert_eq!(
            state.update(Msg::Activate(1)),
            [Effect::OpenVideo(video("b"))]
        );
    }
}
