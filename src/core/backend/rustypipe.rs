//! [`Backend`] implementation using the `rustypipe` YouTube client.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rustypipe::client::RustyPipe;
use rustypipe::model::{self as rp, richtext::ToPlaintext};

use super::{Backend, BackendError, Downloader, ProgressFn, Result, fetch, visionos};
use crate::core::model::SegmentRanges;
use crate::core::model::{
    AudioStream, Channel, Comment, CommentPage, DownloadKind, SavedDownload, SearchItem, Streams,
    SubtitleTrack, Video, VideoDetails, VideoStream,
};
use crate::core::streams::{self, DEFAULT_MAX_HEIGHT};
use crate::core::subtitles::{self, Cue};
use crate::core::{download, feed};

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

/// How long to wait for a connection to YouTube.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// How long a response may send nothing before the request fails (and a
/// download range is retried). Slow but steady responses never time out.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// How often a download fetches the stream URLs again when they are refused.
const URL_REFRESHES: u32 = 3;

pub struct RustyPipeBackend {
    client: RustyPipe,
    http: reqwest::Client,
}

impl RustyPipeBackend {
    /// Creates a backend that caches client data in `storage_dir`.
    pub fn new(storage_dir: &std::path::Path) -> Result<Self> {
        let client = RustyPipe::builder().storage_dir(storage_dir).build()?;
        Ok(Self {
            client,
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .build()?,
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
            comments: comment_page(d.top_comments),
        })
    }

    async fn streams(&self, video_id: &str) -> Result<Streams> {
        match self.visionos_streams(video_id).await {
            Ok(streams) if self.serves(&streams).await => return Ok(streams),
            Ok(_) => eprintln!("flux: visionOS stream URLs refused, using rustypipe"),
            Err(e) => eprintln!("flux: visionOS player failed, using rustypipe: {e}"),
        }
        let p = self.client.query().player(video_id).await?;
        Ok(Streams {
            muxed: p.video_streams.into_iter().map(video_stream).collect(),
            video_only: p.video_only_streams.into_iter().map(video_stream).collect(),
            audio: p.audio_streams.into_iter().map(audio_stream).collect(),
            hls_url: p.hls_manifest_url,
            is_live: p.details.is_live,
            duration_secs: Some(p.details.duration),
            subtitles: p.subtitles.into_iter().map(subtitle_track).collect(),
        })
    }

    async fn channel(&self, channel_id: &str) -> Result<(Channel, Vec<Video>)> {
        // Channel tabs lack publication dates; the RSS feed has them for the
        // latest uploads.
        let query = self.client.query();
        let (c, feed) = futures::join!(
            query.channel_videos(channel_id),
            self.channel_feed(channel_id)
        );
        let c = c?;
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
            .collect::<Vec<_>>();
        let mut videos = videos;
        feed::fill_dates(&mut videos, &feed.unwrap_or_default());
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

    async fn comments(&self, token: &str) -> Result<CommentPage> {
        let page = self.client.query().video_comments(token, None).await?;
        Ok(comment_page(page))
    }

    async fn subtitles(&self, url: &str) -> Result<Vec<Cue>> {
        let body = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(subtitles::parse(&body))
    }
}

impl RustyPipeBackend {
    /// Downloads `url` (of `size` bytes) to `path`.
    async fn fetch(
        &self,
        url: &str,
        size: Option<u64>,
        path: &std::path::Path,
        on_bytes: &mut impl FnMut(u64),
    ) -> Result<()> {
        let size = size.ok_or_else(|| BackendError::Other("unknown stream size".into()))?;
        fetch::fetch(
            &self.http,
            url,
            size,
            path,
            &fetch::Options::default(),
            on_bytes,
        )
        .await
    }

    /// Whether the URLs of `streams` serve whole streams, checked with one
    /// of them. Streams without sizes (e.g. live streams) are not checked.
    async fn serves(&self, streams: &Streams) -> bool {
        match streams::sample(streams) {
            Some((url, size)) => fetch::serves(&self.http, url, size).await,
            None => true,
        }
    }

    async fn visionos_streams(&self, video_id: &str) -> Result<Streams> {
        let visitor_data = self.client.query().get_visitor_data(false).await?;
        visionos::fetch_streams(&self.http, video_id, &visitor_data).await
    }
}

#[async_trait]
impl Downloader for RustyPipeBackend {
    async fn download(
        &self,
        video_id: &str,
        kind: DownloadKind,
        path: &std::path::Path,
        progress: ProgressFn,
    ) -> Result<SavedDownload> {
        // Each attempt resumes the parts of the previous ones.
        super::with_fresh_urls(URL_REFRESHES, || {
            self.download_once(video_id, kind, path, &progress)
        })
        .await
    }
}

impl RustyPipeBackend {
    /// Downloads a video with freshly fetched stream URLs.
    async fn download_once(
        &self,
        video_id: &str,
        kind: DownloadKind,
        path: &std::path::Path,
        progress: &ProgressFn,
    ) -> Result<SavedDownload> {
        let streams = self.streams(video_id).await?;
        let plan = download::plan(&streams, kind, DEFAULT_MAX_HEIGHT)
            .ok_or_else(|| BackendError::Other("no downloadable stream".into()))?;
        let output = path.with_extension(plan.extension);
        // The sizes are known: `total` below fails otherwise.
        let part =
            |name, size: Option<u64>| download::part_path(&output, name, size.unwrap_or_default());

        let mut sizes = Vec::new();
        if let Some(video) = &plan.video {
            sizes.push(video.size);
        }
        sizes.push(plan.audio.size);
        let total: u64 = sizes
            .into_iter()
            .map(|s| s.ok_or_else(|| BackendError::Other("unknown stream size".into())))
            .sum::<Result<_>>()?;
        let (mut done, mut percent) = (0, 0);
        // Report whole percents only; network chunks are small.
        let mut on_bytes = |n: u64| {
            done += n;
            let now = done * 100 / total.max(1);
            if now > percent {
                percent = now;
                progress(done as f64 / total.max(1) as f64);
            }
        };

        match &plan.video {
            Some(video) => {
                // Failed downloads keep their parts, so that a retry resumes them.
                let video_part = part("video", video.size);
                let audio_part = part("audio", plan.audio.size);
                self.fetch(&video.url, video.size, &video_part, &mut on_bytes)
                    .await?;
                self.fetch(&plan.audio.url, plan.audio.size, &audio_part, &mut on_bytes)
                    .await?;
                mux(&video_part, &audio_part, &output).await?;
                let _ = tokio::fs::remove_file(&video_part).await;
                let _ = tokio::fs::remove_file(&audio_part).await;
            }
            None => {
                let audio_part = part("audio", plan.audio.size);
                self.fetch(&plan.audio.url, plan.audio.size, &audio_part, &mut on_bytes)
                    .await?;
                tokio::fs::rename(&audio_part, &output).await?;
            }
        }
        let subtitles = match kind {
            DownloadKind::Video => self.save_subtitles(&streams.subtitles, &output).await,
            DownloadKind::Audio => Vec::new(),
        };
        Ok(SavedDownload {
            path: output,
            subtitles,
        })
    }
}

impl RustyPipeBackend {
    /// Saves `tracks` next to the video at `video`. A track that cannot be
    /// saved is left out rather than failing the download.
    async fn save_subtitles(
        &self,
        tracks: &[SubtitleTrack],
        video: &std::path::Path,
    ) -> Vec<SubtitleTrack> {
        let mut saved = Vec::new();
        for (track, path) in tracks.iter().zip(download::subtitle_paths(video, tracks)) {
            let result = async {
                let body = self
                    .http
                    .get(&track.url)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;
                tokio::fs::write(&path, subtitles::as_vtt(&body).as_bytes()).await?;
                super::saved_track(track, &path)
            }
            .await;
            match result {
                Ok(track) => saved.push(track),
                Err(e) => eprintln!("flux: cannot save subtitles {}: {e}", track.name),
            }
        }
        saved
    }
}

/// Muxes video and audio files into `output` with ffmpeg.
async fn mux(
    video: &std::path::Path,
    audio: &std::path::Path,
    output: &std::path::Path,
) -> Result<()> {
    let result = tokio::process::Command::new("ffmpeg")
        .args(download::mux_args(video, audio, output))
        .output()
        .await
        .map_err(|e| BackendError::Other(format!("cannot run ffmpeg: {e}")))?;
    if !result.status.success() {
        return Err(BackendError::Other(format!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(())
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

fn comment_page(p: rp::paginator::Paginator<rp::Comment>) -> CommentPage {
    CommentPage {
        comments: p
            .items
            .into_iter()
            .map(|c| Comment {
                author: c.author.map(channel_from_tag),
                text: c.text.to_plaintext(),
                published: c.publish_date.and_then(to_chrono),
                like_count: c.like_count,
                reply_count: c.reply_count,
                replies: c.replies.ctoken,
                pinned: c.pinned,
                by_owner: c.by_owner,
            })
            .collect(),
        count: p.count,
        next: p.ctoken,
    }
}

fn subtitle_track(s: rp::Subtitle) -> SubtitleTrack {
    SubtitleTrack {
        name: if s.auto_generated {
            format!("{} (auto-generated)", s.lang_name)
        } else {
            s.lang_name
        },
        lang: s.lang,
        url: visionos::timedtext_url(&s.url, s.auto_generated),
        auto_generated: s.auto_generated,
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
        ranges: segment_ranges(s.init_range, s.index_range),
        size: s.size,
    }
}

fn segment_ranges(
    init: Option<std::ops::Range<u32>>,
    index: Option<std::ops::Range<u32>>,
) -> Option<SegmentRanges> {
    Some(SegmentRanges {
        init: init.map(|r| (r.start, r.end))?,
        index: index.map(|r| (r.start, r.end))?,
    })
}

fn audio_stream(s: rp::AudioStream) -> AudioStream {
    AudioStream {
        url: s.url,
        bitrate: s.bitrate,
        mime: s.mime,
        ranges: segment_ranges(s.init_range, s.index_range),
        size: Some(s.size),
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

    fn comment_json() -> serde_json::Value {
        json!({
            "id": "Ugy1",
            "text": [{"Text": {"text": "Great "}}, {"Web": {"text": "link", "url": "https://x.org"}}],
            "author": {
                "id": "UC9",
                "name": "@viewer",
                "avatar": [{"url": "https://yt3.ggpht.com/a.jpg", "width": 48, "height": 48}],
                "verification": "none",
                "subscriber_count": null
            },
            "publish_date": "2026-10-20T14:00:00Z",
            "publish_date_txt": "4 days ago",
            "like_count": 12,
            "reply_count": 3,
            "replies": {"count": 3, "items": [], "ctoken": "r", "endpoint": "next"},
            "by_owner": false,
            "pinned": true,
            "hearted": false
        })
    }

    #[test]
    fn maps_comment_page() {
        let mut deleted = comment_json();
        deleted["author"] = serde_json::Value::Null;
        let page: rp::paginator::Paginator<rp::Comment> = serde_json::from_value(json!({
            "count": 1200,
            "items": [comment_json(), deleted],
            "ctoken": "next-page",
            "endpoint": "next"
        }))
        .unwrap();

        let page = comment_page(page);

        assert_eq!(page.count, Some(1200));
        assert_eq!(page.next.as_deref(), Some("next-page"));
        assert_eq!(
            page.comments[0],
            Comment {
                author: Some(Channel {
                    id: "UC9".into(),
                    name: "@viewer".into(),
                    avatar_url: Some("https://yt3.ggpht.com/a.jpg".into()),
                }),
                text: "Great https://x.org".into(),
                published: Some("2026-10-20T14:00:00Z".parse().unwrap()),
                like_count: Some(12),
                reply_count: 3,
                replies: Some("r".into()),
                pinned: true,
                by_owner: false,
            }
        );
        assert_eq!(page.comments[1].author, None);
    }

    #[test]
    fn maps_subtitle_tracks() {
        let track = |auto: bool| -> rp::Subtitle {
            serde_json::from_value(json!({
                "url": "https://www.youtube.com/api/timedtext?v=x&lang=en",
                "lang": "en",
                "lang_name": "English",
                "auto_generated": auto
            }))
            .unwrap()
        };

        assert_eq!(
            subtitle_track(track(false)),
            SubtitleTrack {
                name: "English".into(),
                lang: "en".into(),
                url: "https://www.youtube.com/api/timedtext?v=x&lang=en&fmt=srv3".into(),
                auto_generated: false,
            }
        );
        let auto = subtitle_track(track(true));
        assert_eq!(auto.name, "English (auto-generated)");
        assert!(auto.auto_generated);
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

    #[tokio::test]
    #[ignore = "needs network"]
    async fn subtitles_real_service() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RustyPipeBackend::new(dir.path()).unwrap();
        let streams = backend.streams("dQw4w9WgXcQ").await.unwrap();
        let track = streams
            .subtitles
            .iter()
            .find(|t| t.lang == "en")
            .expect("English subtitles");
        let cues = backend.subtitles(&track.url).await.unwrap();
        assert!(cues.len() > 10, "{cues:?}");
    }

    #[tokio::test]
    #[ignore = "needs network"]
    async fn comments_real_service() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RustyPipeBackend::new(dir.path()).unwrap();
        let details = backend.video_details("dQw4w9WgXcQ").await.unwrap();
        let token = details.comments.next.expect("comments token");
        let page = backend.comments(&token).await.unwrap();
        assert!(!page.comments.is_empty());
        assert!(page.next.is_some());

        let comment = page
            .comments
            .iter()
            .find(|c| c.reply_count > 0)
            .expect("a comment with replies");
        let replies = backend
            .comments(comment.replies.as_deref().expect("replies token"))
            .await
            .unwrap();
        assert!(!replies.comments.is_empty());
        assert!(
            replies.comments.iter().all(|r| r.replies.is_none()),
            "one level"
        );
    }

    #[tokio::test]
    #[ignore = "needs network"]
    async fn channel_real_service() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RustyPipeBackend::new(dir.path()).unwrap();
        let (channel, videos) = backend.channel("UCsXVk37bltHxD1rDPwtNM8Q").await.unwrap();
        assert_eq!(channel.id, "UCsXVk37bltHxD1rDPwtNM8Q");
        assert!(!videos.is_empty());
        assert!(
            videos.iter().any(|v| v.published.is_some()),
            "dates from RSS"
        );
    }

    /// Stream URLs must serve the whole file, not only its beginning.
    #[tokio::test]
    #[ignore = "needs network"]
    async fn streams_serve_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RustyPipeBackend::new(dir.path()).unwrap();
        let streams = backend.streams("HPjZDlQexeY").await.unwrap();
        let video = streams
            .video_only
            .iter()
            .find(|v| v.size.is_some_and(|s| s > 40_000_000))
            .expect("a large stream");
        let status = reqwest::Client::new()
            .get(&video.url)
            .header("Range", "bytes=30000000-30000999")
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, reqwest::StatusCode::PARTIAL_CONTENT);
    }

    #[tokio::test]
    #[ignore = "needs network and ffmpeg"]
    async fn downloads_video_and_audio() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RustyPipeBackend::new(dir.path()).unwrap();
        for (kind, ext) in [(DownloadKind::Video, "mp4"), (DownloadKind::Audio, "m4a")] {
            let last = std::sync::Arc::new(std::sync::Mutex::new(0.0));
            let progress = last.clone();
            let saved = backend
                .download(
                    "dQw4w9WgXcQ",
                    kind,
                    &dir.path().join("rick.mp4"),
                    Box::new(move |p| *progress.lock().unwrap() = p),
                )
                .await
                .unwrap();
            assert_eq!(saved.path.extension().unwrap(), ext);
            assert!(std::fs::metadata(&saved.path).unwrap().len() > 1_000_000);
            assert_eq!(*last.lock().unwrap(), 1.0);
            assert_eq!(saved.subtitles.is_empty(), kind == DownloadKind::Audio);
            for track in &saved.subtitles {
                let cues = super::super::load_subtitles(&backend, &track.url)
                    .await
                    .unwrap();
                assert!(!cues.is_empty(), "{}", track.name);
            }
        }
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
