//! Access to the streaming service.

pub mod fake;
pub mod rustypipe;
pub mod visionos;

use async_trait::async_trait;

use super::feed;
use super::model::{Channel, DownloadKind, SearchItem, Streams, Video, VideoDetails};

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
}

/// Reports download progress from 0 to 1.
pub type ProgressFn = Box<dyn Fn(f64) + Send + Sync>;

#[async_trait]
pub trait Downloader: Send + Sync {
    /// Downloads a video to `path` and returns the path actually written
    /// (the extension may differ).
    async fn download(
        &self,
        video_id: &str,
        kind: DownloadKind,
        path: &std::path::Path,
        progress: ProgressFn,
    ) -> Result<std::path::PathBuf>;
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
    async fn refresh_feed_reports_failing_channels() {
        let backend = FakeBackend::default().with_feed("A", vec![video("a1", "A", 1)]);

        let refresh = refresh_feed(&backend, &["A".into(), "missing".into()]).await;

        assert_eq!(refresh.videos.len(), 1);
        assert_eq!(refresh.errors.len(), 1);
        assert_eq!(refresh.errors[0].0, "missing");
    }
}
