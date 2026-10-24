//! Channel page: a channel's latest uploads.

use super::video::Load;
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
