//! [`Backend`] implementation using the `rustypipe` YouTube client.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rustypipe::client::RustyPipe;
use rustypipe::model::{self as rp, richtext::ToPlaintext};

use super::{Backend, BackendError, Result};
use crate::core::feed;
use crate::core::model::{
    AudioStream, Channel, SearchItem, Streams, Video, VideoDetails, VideoStream,
};

impl From<rustypipe::error::Error> for BackendError {
    fn from(e: rustypipe::error::Error) -> Self {
        BackendError::Other(e.to_string())
    }
}

impl From<reqwest::Error> for BackendError {
    fn from(e: reqwest::Error) -> Self {
        BackendError::Other(e.to_string())
    }
}

pub struct RustyPipeBackend {
    client: RustyPipe,
    http: reqwest::Client,
}

impl RustyPipeBackend {
    /// Creates a backend that caches client data in `storage_dir`.
    pub fn new(storage_dir: &std::path::Path) -> Result<Self> {
        Ok(Self {
            client: RustyPipe::builder().storage_dir(storage_dir).build()?,
            http: reqwest::Client::new(),
        })
    }
}

#[async_trait]
impl Backend for RustyPipeBackend {
    async fn search(&self, query: &str) -> Result<Vec<SearchItem>> {
        let result = self
            .client
            .query()
            .search::<rp::YouTubeItem, _>(query)
            .await?;
        Ok(result
            .items
            .items
            .into_iter()
            .filter_map(search_item)
            .collect())
    }

    async fn video_details(&self, video_id: &str) -> Result<VideoDetails> {
        let d = self.client.query().video_details(video_id).await?;
        let channel = channel_from_tag(d.channel);
        Ok(VideoDetails {
            video: Video {
                id: d.id,
                title: d.name,
                channel_id: channel.id.clone(),
                channel_name: channel.name.clone(),
                published: d.publish_date.and_then(to_chrono),
                duration_secs: None,
                thumbnail_url: None,
            },
            channel,
            description: d.description.to_plaintext(),
            view_count: Some(d.view_count),
            like_count: d.like_count,
            related: d
                .recommended
                .items
                .into_iter()
                .map(video_from_item)
                .collect(),
        })
    }

    async fn streams(&self, video_id: &str) -> Result<Streams> {
        let p = self.client.query().player(video_id).await?;
        Ok(Streams {
            muxed: p.video_streams.into_iter().map(video_stream).collect(),
            video_only: p.video_only_streams.into_iter().map(video_stream).collect(),
            audio: p.audio_streams.into_iter().map(audio_stream).collect(),
            hls_url: p.hls_manifest_url,
        })
    }

    async fn channel(&self, channel_id: &str) -> Result<(Channel, Vec<Video>)> {
        let c = self.client.query().channel_videos(channel_id).await?;
        let channel = Channel {
            id: c.id,
            name: c.name,
            avatar_url: best_thumbnail(&c.avatar),
        };
        let videos = c
            .content
            .items
            .into_iter()
            .map(video_from_item)
            .map(|mut v| {
                // Channel tabs omit the channel on each item.
                v.channel_id.clone_from(&channel.id);
                v.channel_name.clone_from(&channel.name);
                v
            })
            .collect();
        Ok((channel, videos))
    }

    async fn channel_feed(&self, channel_id: &str) -> Result<Vec<Video>> {
        let xml = self
            .http
            .get(feed::feed_url(channel_id))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        feed::parse_channel_feed(&xml).map_err(|e| BackendError::Other(e.to_string()))
    }
}

fn to_chrono(t: time::OffsetDateTime) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(t.unix_timestamp(), t.nanosecond())
}

/// The URL of the largest thumbnail.
fn best_thumbnail(thumbnails: &[rp::Thumbnail]) -> Option<String> {
    thumbnails
        .iter()
        .max_by_key(|t| t.width * t.height)
        .map(|t| t.url.clone())
}

fn channel_from_tag(tag: rp::ChannelTag) -> Channel {
    Channel {
        avatar_url: best_thumbnail(&tag.avatar),
        id: tag.id,
        name: tag.name,
    }
}

fn video_from_item(item: rp::VideoItem) -> Video {
    let channel = item.channel.map(channel_from_tag);
    Video {
        thumbnail_url: best_thumbnail(&item.thumbnail),
        id: item.id,
        title: item.name,
        channel_id: channel.as_ref().map(|c| c.id.clone()).unwrap_or_default(),
        channel_name: channel.map(|c| c.name).unwrap_or_default(),
        published: item.publish_date.and_then(to_chrono),
        duration_secs: item.duration,
    }
}

fn search_item(item: rp::YouTubeItem) -> Option<SearchItem> {
    match item {
        rp::YouTubeItem::Video(v) => Some(SearchItem::Video(video_from_item(v))),
        rp::YouTubeItem::Channel(c) => Some(SearchItem::Channel(Channel {
            avatar_url: best_thumbnail(&c.avatar),
            id: c.id,
            name: c.name,
        })),
        rp::YouTubeItem::Playlist(_) => None,
    }
}

fn video_stream(s: rp::VideoStream) -> VideoStream {
    VideoStream {
        url: s.url,
        width: s.width,
        height: s.height,
        fps: s.fps,
        bitrate: s.bitrate,
        mime: s.mime,
    }
}

fn audio_stream(s: rp::AudioStream) -> AudioStream {
    AudioStream {
        url: s.url,
        bitrate: s.bitrate,
        mime: s.mime,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn video_item_json() -> serde_json::Value {
        json!({
            "id": "dQw4w9WgXcQ",
            "name": "A Video",
            "duration": 212,
            "thumbnail": [
                {"url": "https://i.ytimg.com/small.jpg", "width": 168, "height": 94},
                {"url": "https://i.ytimg.com/large.jpg", "width": 336, "height": 188}
            ],
            "channel": {
                "id": "UC123",
                "name": "A Channel",
                "avatar": [{"url": "https://yt3.ggpht.com/avatar.jpg", "width": 68, "height": 68}],
                "verification": "none",
                "subscriber_count": null
            },
            "publish_date": "2026-10-20T14:00:07Z",
            "publish_date_txt": "4 days ago",
            "view_count": 1000,
            "is_live": false,
            "is_short": false,
            "is_upcoming": false,
            "short_description": null
        })
    }

    #[test]
    fn maps_video_item() {
        let item: rp::VideoItem = serde_json::from_value(video_item_json()).unwrap();

        assert_eq!(
            video_from_item(item),
            Video {
                id: "dQw4w9WgXcQ".into(),
                title: "A Video".into(),
                channel_id: "UC123".into(),
                channel_name: "A Channel".into(),
                published: Some("2026-10-20T14:00:07Z".parse().unwrap()),
                duration_secs: Some(212),
                thumbnail_url: Some("https://i.ytimg.com/large.jpg".into()),
            }
        );
    }

    #[test]
    fn maps_video_item_without_channel() {
        let mut json = video_item_json();
        json["channel"] = serde_json::Value::Null;
        json["publish_date"] = serde_json::Value::Null;
        let item: rp::VideoItem = serde_json::from_value(json).unwrap();

        let video = video_from_item(item);
        assert_eq!(video.channel_id, "");
        assert_eq!(video.published, None);
    }

    #[test]
    fn maps_search_items() {
        let video: rp::YouTubeItem =
            serde_json::from_value(json!({ "Video": video_item_json() })).unwrap();
        let channel: rp::YouTubeItem = serde_json::from_value(json!({ "Channel": {
            "id": "UC123",
            "name": "A Channel",
            "handle": "@chan",
            "avatar": [{"url": "https://yt3.ggpht.com/avatar.jpg", "width": 68, "height": 68}],
            "verification": "verified",
            "subscriber_count": 5,
            "short_description": ""
        }}))
        .unwrap();
        let playlist: rp::YouTubeItem = serde_json::from_value(json!({ "Playlist": {
            "id": "PL1", "name": "P", "thumbnail": [], "channel": null, "video_count": 3
        }}))
        .unwrap();

        assert!(matches!(search_item(video), Some(SearchItem::Video(v)) if v.id == "dQw4w9WgXcQ"));
        assert_eq!(
            search_item(channel),
            Some(SearchItem::Channel(Channel {
                id: "UC123".into(),
                name: "A Channel".into(),
                avatar_url: Some("https://yt3.ggpht.com/avatar.jpg".into()),
            }))
        );
        assert_eq!(search_item(playlist), None);
    }

    #[test]
    fn best_thumbnail_of_empty_list_is_none() {
        assert_eq!(best_thumbnail(&[]), None);
    }

    /// Smoke test against the real service: `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "needs network"]
    async fn search_real_service() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RustyPipeBackend::new(dir.path()).unwrap();
        let items = backend.search("kurzgesagt").await.unwrap();
        assert!(!items.is_empty());
    }
}
