//! Access to the streaming service.

pub mod fake;
pub mod rustypipe;
pub mod visionos;

use async_trait::async_trait;

use super::feed;
use super::model::{
    Channel, CommentPage, DownloadKind, SavedDownload, SearchItem, Streams, SubtitleTrack, Video,
    VideoDetails,
};
use super::subtitles::Cue;

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for BackendError {
    fn from(e: std::io::Error) -> Self {
        BackendError::Other(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, BackendError>;

#[async_trait]
pub trait Backend: Send + Sync {
    async fn search(&self, query: &str) -> Result<Vec<SearchItem>>;
    async fn video_details(&self, video_id: &str) -> Result<VideoDetails>;
    async fn streams(&self, video_id: &str) -> Result<Streams>;
    /// A channel with its latest uploads.
    async fn channel(&self, channel_id: &str) -> Result<(Channel, Vec<Video>)>;
    /// The latest uploads of a channel from its RSS feed.
    async fn channel_feed(&self, channel_id: &str) -> Result<Vec<Video>>;
    /// The page of comments behind `token` (see [`CommentPage::next`]).
    async fn comments(&self, token: &str) -> Result<CommentPage>;
    /// The cues of a subtitle track (see [`SubtitleTrack::url`]).
    ///
    /// [`SubtitleTrack::url`]: super::model::SubtitleTrack::url
    async fn subtitles(&self, url: &str) -> Result<Vec<Cue>>;
}

/// Reports download progress from 0 to 1.
pub type ProgressFn = Box<dyn Fn(f64) + Send + Sync>;

#[async_trait]
pub trait Downloader: Send + Sync {
    /// Downloads a video to `path` (the extension may differ), with its
    /// subtitles if it is a video download.
    async fn download(
        &self,
        video_id: &str,
        kind: DownloadKind,
        path: &std::path::Path,
        progress: ProgressFn,
    ) -> Result<SavedDownload>;
}

/// The cues of a subtitle track, read from disk for `file://` URLs
/// (saved with a download) and fetched by `backend` otherwise.
pub async fn load_subtitles(backend: &dyn Backend, url: &str) -> Result<Vec<Cue>> {
    match url::Url::parse(url)
        .ok()
        .and_then(|u| u.to_file_path().ok())
    {
        Some(path) => {
            let vtt = tokio::fs::read_to_string(path).await?;
            Ok(super::subtitles::parse_vtt(&vtt))
        }
        None => backend.subtitles(url).await,
    }
}

/// `track` as saved at `path`.
pub fn saved_track(track: &SubtitleTrack, path: &std::path::Path) -> Result<SubtitleTrack> {
    let url = url::Url::from_file_path(path)
        .map_err(|()| BackendError::Other(format!("not an absolute path: {}", path.display())))?;
    Ok(SubtitleTrack {
        url: url.into(),
        ..track.clone()
    })
}

/// The result of refreshing the subscription feed.
#[derive(Debug, Default)]
pub struct FeedRefresh {
    /// Videos of all channels, newest first.
    pub videos: Vec<Video>,
    /// Channels whose feed could not be fetched, with the error message.
    pub errors: Vec<(String, String)>,
}

/// Fetches the feeds of all `channel_ids` concurrently and aggregates them.
/// A failing channel does not fail the whole refresh.
pub async fn refresh_feed(backend: &dyn Backend, channel_ids: &[String]) -> FeedRefresh {
    let results =
        futures::future::join_all(channel_ids.iter().map(|id| backend.channel_feed(id))).await;
    let mut feeds = Vec::new();
    let mut errors = Vec::new();
    for (id, result) in channel_ids.iter().zip(results) {
        match result {
            Ok(videos) => feeds.push(videos),
            Err(e) => errors.push((id.clone(), e.to_string())),
        }
    }
    FeedRefresh {
        videos: feed::aggregate(feeds),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeBackend;
    use super::*;

    fn video(id: &str, channel_id: &str, published: i64) -> Video {
        Video {
            id: id.into(),
            title: id.into(),
            channel_id: channel_id.into(),
            channel_name: channel_id.into(),
            published: chrono::DateTime::from_timestamp(published, 0),
            duration_secs: None,
            thumbnail_url: None,
        }
    }

    #[tokio::test]
    async fn refresh_feed_aggregates_all_channels() {
        let backend = FakeBackend::default()
            .with_feed("A", vec![video("a1", "A", 1), video("a2", "A", 3)])
            .with_feed("B", vec![video("b1", "B", 2)]);

        let refresh = refresh_feed(&backend, &["A".into(), "B".into()]).await;

        let ids: Vec<_> = refresh.videos.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["a2", "b1", "a1"]);
        assert!(refresh.errors.is_empty());
    }

    #[tokio::test]
    async fn load_subtitles_reads_local_files() {
        let cue = |text: &str| Cue {
            start: std::time::Duration::ZERO,
            end: std::time::Duration::from_secs(1),
            text: text.into(),
            placement: Default::default(),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.en.vtt");
        std::fs::write(&path, crate::core::subtitles::to_vtt(&[cue("local")])).unwrap();
        let backend = FakeBackend::default().with_subtitles("https://subs", vec![cue("remote")]);

        let local = url::Url::from_file_path(&path).unwrap();
        assert_eq!(
            load_subtitles(&backend, local.as_str()).await.unwrap(),
            [cue("local")]
        );
        assert_eq!(
            load_subtitles(&backend, "https://subs").await.unwrap(),
            [cue("remote")]
        );
        assert!(
            load_subtitles(&backend, "file:///missing.vtt")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn refresh_feed_reports_failing_channels() {
        let backend = FakeBackend::default().with_feed("A", vec![video("a1", "A", 1)]);

        let refresh = refresh_feed(&backend, &["A".into(), "missing".into()]).await;

        assert_eq!(refresh.videos.len(), 1);
        assert_eq!(refresh.errors.len(), 1);
        assert_eq!(refresh.errors[0].0, "missing");
    }
}
