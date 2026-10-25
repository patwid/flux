//! An in-memory [`Backend`] for tests.

use std::collections::HashMap;

use async_trait::async_trait;

use super::{Backend, BackendError, Downloader, ProgressFn, Result, saved_track};
use crate::core::model::{
    Channel, CommentPage, DownloadKind, Playlist, SavedDownload, SearchItem, Streams, Video,
    VideoDetails,
};
use crate::core::subtitles::Cue;
use crate::core::{download, subtitles};

/// A backend serving canned data. Unknown ids yield an error.
#[derive(Default)]
pub struct FakeBackend {
    pub search_results: HashMap<String, Vec<SearchItem>>,
    pub details: HashMap<String, VideoDetails>,
    pub streams: HashMap<String, Streams>,
    pub channels: HashMap<String, (Channel, Vec<Video>)>,
    pub feeds: HashMap<String, Vec<Video>>,
    /// Playlists by channel id.
    pub channel_playlists: HashMap<String, Vec<Playlist>>,
    /// Videos by playlist id.
    pub playlists: HashMap<String, Vec<Video>>,
    /// Comment pages by token.
    pub comments: HashMap<String, CommentPage>,
    /// Subtitle cues by URL.
    pub subtitles: HashMap<String, Vec<Cue>>,
}

impl FakeBackend {
    pub fn with_search(mut self, query: &str, items: Vec<SearchItem>) -> Self {
        self.search_results.insert(query.into(), items);
        self
    }

    pub fn with_details(mut self, details: VideoDetails) -> Self {
        self.details.insert(details.video.id.clone(), details);
        self
    }

    pub fn with_streams(mut self, video_id: &str, streams: Streams) -> Self {
        self.streams.insert(video_id.into(), streams);
        self
    }

    pub fn with_channel(mut self, channel: Channel, videos: Vec<Video>) -> Self {
        self.channels.insert(channel.id.clone(), (channel, videos));
        self
    }

    pub fn with_channel_playlists(mut self, channel_id: &str, playlists: Vec<Playlist>) -> Self {
        self.channel_playlists.insert(channel_id.into(), playlists);
        self
    }

    pub fn with_playlist(mut self, playlist_id: &str, videos: Vec<Video>) -> Self {
        self.playlists.insert(playlist_id.into(), videos);
        self
    }

    pub fn with_feed(mut self, channel_id: &str, videos: Vec<Video>) -> Self {
        self.feeds.insert(channel_id.into(), videos);
        self
    }

    pub fn with_subtitles(mut self, url: &str, cues: Vec<Cue>) -> Self {
        self.subtitles.insert(url.into(), cues);
        self
    }

    pub fn with_comments(mut self, token: &str, page: CommentPage) -> Self {
        self.comments.insert(token.into(), page);
        self
    }
}

fn lookup<T: Clone>(map: &HashMap<String, T>, key: &str) -> Result<T> {
    map.get(key)
        .cloned()
        .ok_or_else(|| BackendError::Other(format!("not found: {key}")))
}

#[async_trait]
impl Backend for FakeBackend {
    async fn search(&self, query: &str) -> Result<Vec<SearchItem>> {
        Ok(self.search_results.get(query).cloned().unwrap_or_default())
    }

    async fn video_details(&self, video_id: &str) -> Result<VideoDetails> {
        lookup(&self.details, video_id)
    }

    async fn streams(&self, video_id: &str) -> Result<Streams> {
        lookup(&self.streams, video_id)
    }

    async fn channel(&self, channel_id: &str) -> Result<(Channel, Vec<Video>)> {
        lookup(&self.channels, channel_id)
    }

    async fn channel_playlists(&self, channel_id: &str) -> Result<Vec<Playlist>> {
        lookup(&self.channel_playlists, channel_id)
    }

    async fn playlist_videos(&self, playlist_id: &str) -> Result<Vec<Video>> {
        lookup(&self.playlists, playlist_id)
    }

    async fn channel_feed(&self, channel_id: &str) -> Result<Vec<Video>> {
        lookup(&self.feeds, channel_id)
    }

    async fn comments(&self, token: &str) -> Result<CommentPage> {
        lookup(&self.comments, token)
    }

    async fn subtitles(&self, url: &str) -> Result<Vec<Cue>> {
        lookup(&self.subtitles, url)
    }
}

/// Downloads by copying the file behind the first muxed `file://` stream,
/// reporting progress in steps. Saves the subtitle tracks that have cues.
#[async_trait]
impl Downloader for FakeBackend {
    async fn download(
        &self,
        video_id: &str,
        kind: DownloadKind,
        path: &std::path::Path,
        progress: ProgressFn,
    ) -> Result<SavedDownload> {
        let streams = lookup(&self.streams, video_id)?;
        let source = streams
            .muxed
            .first()
            .and_then(|s| url::Url::parse(&s.url).ok())
            .and_then(|u| u.to_file_path().ok())
            .ok_or_else(|| BackendError::Other("no local stream".into()))?;
        for step in 1..=10 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            progress(f64::from(step) / 10.0);
        }
        let path = path.with_extension(source.extension().unwrap_or_default());
        std::fs::copy(&source, &path).map_err(|e| BackendError::Other(e.to_string()))?;
        let mut saved = Vec::new();
        if kind == DownloadKind::Video {
            let tracks = &streams.subtitles;
            for (track, file) in tracks.iter().zip(download::subtitle_paths(&path, tracks)) {
                if let Some(cues) = self.subtitles.get(&track.url) {
                    std::fs::write(&file, subtitles::to_vtt(cues))?;
                    saved.push(saved_track(track, &file)?);
                }
            }
        }
        Ok(SavedDownload {
            path,
            subtitles: saved,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::core::model::{SubtitleTrack, VideoStream};
    use crate::core::subtitles::{self, Cue};

    #[tokio::test]
    async fn fake_download_copies_local_stream() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.webm");
        std::fs::write(&source, b"video").unwrap();
        let backend = FakeBackend::default()
            .with_streams(
                "v",
                Streams {
                    muxed: vec![VideoStream {
                        url: url::Url::from_file_path(&source).unwrap().into(),
                        width: 1,
                        height: 1,
                        fps: 1,
                        bitrate: 1,
                        mime: "video/webm".into(),
                        ranges: None,
                        size: None,
                    }],
                    subtitles: ["en", "de"]
                        .map(|lang| SubtitleTrack {
                            name: lang.into(),
                            lang: lang.into(),
                            url: format!("https://subs/{lang}"),
                            auto_generated: false,
                        })
                        .into(),
                    ..Default::default()
                },
            )
            // Only English loads.
            .with_subtitles("https://subs/en", cues());
        let reported = Arc::new(Mutex::new(Vec::new()));
        let progress = reported.clone();

        let saved = backend
            .download(
                "v",
                DownloadKind::Video,
                &dir.path().join("out.mp4"),
                Box::new(move |p| progress.lock().unwrap().push(p)),
            )
            .await
            .unwrap();

        assert_eq!(saved.path, dir.path().join("out.webm"));
        assert_eq!(std::fs::read(&saved.path).unwrap(), b"video");
        assert_eq!(reported.lock().unwrap().last(), Some(&1.0));

        let vtt = dir.path().join("out.en.vtt");
        assert_eq!(
            saved.subtitles,
            [SubtitleTrack {
                name: "en".into(),
                lang: "en".into(),
                url: url::Url::from_file_path(&vtt).unwrap().into(),
                auto_generated: false,
            }]
        );
        assert_eq!(
            subtitles::parse_vtt(&std::fs::read_to_string(vtt).unwrap()),
            cues()
        );
    }

    fn cues() -> Vec<Cue> {
        vec![Cue {
            start: std::time::Duration::ZERO,
            end: std::time::Duration::from_secs(1),
            text: "Hi".into(),
            placement: Default::default(),
        }]
    }
}
