//! An in-memory [`Backend`] for tests.

use std::collections::HashMap;

use async_trait::async_trait;

use super::{Backend, BackendError, Result};
use crate::core::model::{Channel, SearchItem, Streams, Video, VideoDetails};

/// A backend serving canned data. Unknown ids yield an error.
#[derive(Default)]
pub struct FakeBackend {
    pub search_results: HashMap<String, Vec<SearchItem>>,
    pub details: HashMap<String, VideoDetails>,
    pub streams: HashMap<String, Streams>,
    pub channels: HashMap<String, (Channel, Vec<Video>)>,
    pub feeds: HashMap<String, Vec<Video>>,
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

    pub fn with_feed(mut self, channel_id: &str, videos: Vec<Video>) -> Self {
        self.feeds.insert(channel_id.into(), videos);
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

    async fn channel_feed(&self, channel_id: &str) -> Result<Vec<Video>> {
        lookup(&self.feeds, channel_id)
    }
}
