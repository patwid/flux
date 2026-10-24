//! Video page: player, details, related videos.

use std::path::PathBuf;

use crate::core::model::{Channel, DownloadKind, PlaybackSource, Streams, Video, VideoDetails};
use crate::core::streams;

/// Highest resolution played by default.
pub const MAX_HEIGHT: u32 = 720;

/// Seconds of playback between two history updates.
const SAVE_INTERVAL_SECS: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load<T> {
    Loading,
    Loaded(T),
    Failed(String),
}

/// GTK-independent state of a video page.
#[derive(Debug)]
pub struct State {
    pub video: Video,
    pub details: Load<VideoDetails>,
    pub playback: Load<PlaybackSource>,
    pub subscribed: bool,
    /// Position to start playback at, in seconds.
    resume_at: u32,
    /// Last position written to the history.
    saved_position: Option<u32>,
}

/// Everything known about a video when its page is opened.
#[derive(Debug, Clone)]
pub struct Init {
    pub video: Video,
    /// Saved position from the watch history.
    pub saved_position: Option<u32>,
    /// A completed download to play instead of streaming.
    pub local_file: Option<PathBuf>,
    pub subscribed: bool,
}

#[derive(Debug)]
pub enum Msg {
    Details(Result<VideoDetails, String>),
    Streams(Result<Streams, String>),
    /// Playback reached this position (seconds).
    Progress(u32),
    /// The page is being closed at this position (seconds).
    Closed(u32),
    ToggleSubscribe,
    Download(DownloadKind),
    OpenRelated(usize),
    OpenChannel,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    LoadDetails(String),
    LoadStreams(String),
    Play {
        source: PlaybackSource,
        start_at: u32,
    },
    RecordWatch {
        video: Video,
        position: u32,
    },
    Subscribe(Channel),
    Unsubscribe(String),
    Download(Video, DownloadKind),
    OpenVideo(Video),
    OpenChannel(Channel),
}

/// Where to resume a video watched up to `saved` seconds. Videos watched to
/// (almost) the end, or barely started, restart from the beginning.
pub fn resume_point(saved: u32, duration: Option<u32>) -> u32 {
    let near_end = duration.is_some_and(|d| saved + 10 >= d);
    if saved < 5 || near_end { 0 } else { saved }
}

impl State {
    pub fn new(init: Init) -> (Self, Vec<Effect>) {
        let resume_at = resume_point(init.saved_position.unwrap_or(0), init.video.duration_secs);
        let mut state = State {
            video: init.video,
            details: Load::Loading,
            playback: Load::Loading,
            subscribed: init.subscribed,
            resume_at,
            saved_position: None,
        };
        let mut effects = vec![Effect::LoadDetails(state.video.id.clone())];
        let local_uri = init
            .local_file
            .and_then(|path| url::Url::from_file_path(path).ok());
        match local_uri {
            Some(uri) => effects.extend(state.play(PlaybackSource::Uri(uri.into()))),
            None => effects.push(Effect::LoadStreams(state.video.id.clone())),
        }
        (state, effects)
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Details(Ok(details)) => {
                let duration = self.video.duration_secs;
                let thumbnail = self.video.thumbnail_url.take();
                self.video = details.video.clone();
                self.video.duration_secs = self.video.duration_secs.or(duration);
                self.video.thumbnail_url = self.video.thumbnail_url.take().or(thumbnail);
                self.details = Load::Loaded(details);
            }
            Msg::Details(Err(e)) => self.details = Load::Failed(e),
            Msg::Streams(Ok(streams)) => match streams::choose_source(&streams, MAX_HEIGHT) {
                Some(source) => return self.play(source),
                None => self.playback = Load::Failed("No playable stream found".into()),
            },
            Msg::Streams(Err(e)) => self.playback = Load::Failed(e),
            Msg::Progress(position) => {
                if self
                    .saved_position
                    .is_some_and(|saved| position.abs_diff(saved) >= SAVE_INTERVAL_SECS)
                {
                    return vec![self.record(position)];
                }
            }
            Msg::Closed(position) => {
                if self.saved_position.is_some() {
                    return vec![self.record(position)];
                }
            }
            Msg::ToggleSubscribe => {
                if let Load::Loaded(details) = &self.details {
                    self.subscribed = !self.subscribed;
                    return vec![if self.subscribed {
                        Effect::Subscribe(details.channel.clone())
                    } else {
                        Effect::Unsubscribe(details.channel.id.clone())
                    }];
                }
            }
            Msg::Download(kind) => return vec![Effect::Download(self.video.clone(), kind)],
            Msg::OpenRelated(index) => {
                if let Load::Loaded(details) = &self.details
                    && let Some(video) = details.related.get(index)
                {
                    return vec![Effect::OpenVideo(video.clone())];
                }
            }
            Msg::OpenChannel => {
                if let Load::Loaded(details) = &self.details {
                    return vec![Effect::OpenChannel(details.channel.clone())];
                }
            }
        }
        Vec::new()
    }

    fn play(&mut self, source: PlaybackSource) -> Vec<Effect> {
        self.playback = Load::Loaded(source.clone());
        vec![
            Effect::Play {
                source,
                start_at: self.resume_at,
            },
            self.record(self.resume_at),
        ]
    }

    fn record(&mut self, position: u32) -> Effect {
        self.saved_position = Some(position);
        Effect::RecordWatch {
            video: self.video.clone(),
            position,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{AudioStream, VideoStream};

    fn video(id: &str) -> Video {
        Video {
            id: id.into(),
            title: id.into(),
            channel_id: "UC1".into(),
            channel_name: "Chan".into(),
            published: None,
            duration_secs: Some(600),
            thumbnail_url: None,
        }
    }

    fn channel() -> Channel {
        Channel {
            id: "UC1".into(),
            name: "Chan".into(),
            avatar_url: None,
        }
    }

    fn details() -> VideoDetails {
        VideoDetails {
            video: video("v"),
            channel: channel(),
            description: "desc".into(),
            view_count: Some(1),
            like_count: None,
            related: vec![video("r0"), video("r1")],
        }
    }

    fn init() -> Init {
        Init {
            video: video("v"),
            saved_position: None,
            local_file: None,
            subscribed: false,
        }
    }

    fn muxed() -> Streams {
        Streams {
            muxed: vec![VideoStream {
                url: "https://muxed".into(),
                width: 640,
                height: 360,
                fps: 30,
                bitrate: 1,
                mime: "video/mp4".into(),
            }],
            video_only: vec![],
            audio: vec![AudioStream {
                url: "https://a".into(),
                bitrate: 1,
                mime: "audio/webm".into(),
            }],
            hls_url: None,
        }
    }

    #[test]
    fn resume_point_restarts_finished_videos() {
        assert_eq!(resume_point(120, Some(600)), 120);
        assert_eq!(resume_point(590, Some(600)), 0);
        assert_eq!(resume_point(120, None), 120);
        assert_eq!(resume_point(3, Some(600)), 0);
    }

    #[test]
    fn opening_loads_details_and_streams() {
        let (state, effects) = State::new(init());
        assert_eq!(
            effects,
            [
                Effect::LoadDetails("v".into()),
                Effect::LoadStreams("v".into())
            ]
        );
        assert_eq!(state.details, Load::Loading);
        assert_eq!(state.playback, Load::Loading);
    }

    #[test]
    fn opening_downloaded_video_plays_local_file() {
        let (state, effects) = State::new(Init {
            local_file: Some("/videos/v.mp4".into()),
            saved_position: Some(60),
            ..init()
        });
        let source = PlaybackSource::Uri("file:///videos/v.mp4".into());
        assert_eq!(
            effects,
            [
                Effect::LoadDetails("v".into()),
                Effect::Play {
                    source: source.clone(),
                    start_at: 60
                },
                Effect::RecordWatch {
                    video: video("v"),
                    position: 60
                },
            ]
        );
        assert_eq!(state.playback, Load::Loaded(source));
    }

    #[test]
    fn streams_start_playback_at_resume_point() {
        let (mut state, _) = State::new(Init {
            saved_position: Some(42),
            ..init()
        });
        let effects = state.update(Msg::Streams(Ok(muxed())));
        let source = PlaybackSource::Uri("https://muxed".into());
        assert_eq!(
            effects,
            [
                Effect::Play {
                    source: source.clone(),
                    start_at: 42
                },
                Effect::RecordWatch {
                    video: video("v"),
                    position: 42
                },
            ]
        );
        assert_eq!(state.playback, Load::Loaded(source));
    }

    #[test]
    fn unplayable_streams_fail() {
        let (mut state, _) = State::new(init());
        assert!(
            state
                .update(Msg::Streams(Ok(Streams::default())))
                .is_empty()
        );
        assert!(matches!(state.playback, Load::Failed(_)));

        state.update(Msg::Streams(Err("blocked".into())));
        assert_eq!(state.playback, Load::Failed("blocked".into()));
    }

    #[test]
    fn details_update_video_and_failures_are_kept() {
        let (mut state, _) = State::new(init());
        let mut d = details();
        d.video.title = "Full title".into();
        state.update(Msg::Details(Ok(d.clone())));
        assert_eq!(state.video.title, "Full title");
        // Duration is not part of the details response; keep the known one.
        assert_eq!(state.video.duration_secs, Some(600));
        assert!(matches!(state.details, Load::Loaded(_)));

        state.update(Msg::Details(Err("x".into())));
        assert_eq!(state.details, Load::Failed("x".into()));
    }

    #[test]
    fn progress_is_saved_every_few_seconds() {
        let (mut state, _) = State::new(init());
        state.update(Msg::Streams(Ok(muxed())));

        assert!(state.update(Msg::Progress(2)).is_empty());
        assert_eq!(
            state.update(Msg::Progress(5)),
            [Effect::RecordWatch {
                video: video("v"),
                position: 5
            }]
        );
        assert!(state.update(Msg::Progress(7)).is_empty());
        assert_eq!(
            state.update(Msg::Closed(8)),
            [Effect::RecordWatch {
                video: video("v"),
                position: 8
            }]
        );
    }

    #[test]
    fn closing_before_playback_records_nothing() {
        let (mut state, _) = State::new(init());
        assert!(state.update(Msg::Closed(0)).is_empty());
    }

    #[test]
    fn toggling_subscription() {
        let (mut state, _) = State::new(init());
        assert!(
            state.update(Msg::ToggleSubscribe).is_empty(),
            "needs channel details"
        );

        state.update(Msg::Details(Ok(details())));
        assert_eq!(
            state.update(Msg::ToggleSubscribe),
            [Effect::Subscribe(channel())]
        );
        assert!(state.subscribed);
        assert_eq!(
            state.update(Msg::ToggleSubscribe),
            [Effect::Unsubscribe("UC1".into())]
        );
        assert!(!state.subscribed);
    }

    #[test]
    fn actions_open_and_download() {
        let (mut state, _) = State::new(init());
        state.update(Msg::Details(Ok(details())));

        assert_eq!(
            state.update(Msg::OpenRelated(1)),
            [Effect::OpenVideo(video("r1"))]
        );
        assert!(state.update(Msg::OpenRelated(5)).is_empty());
        assert_eq!(
            state.update(Msg::OpenChannel),
            [Effect::OpenChannel(channel())]
        );
        assert_eq!(
            state.update(Msg::Download(DownloadKind::Audio)),
            [Effect::Download(state.video.clone(), DownloadKind::Audio)]
        );
    }
}
