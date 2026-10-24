use std::path::PathBuf;

use chrono::{DateTime, Utc};

/// A YouTube channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

/// A video as shown in lists (search results, feed, history).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub channel_name: String,
    pub published: Option<DateTime<Utc>>,
    pub duration_secs: Option<u32>,
    pub thumbnail_url: Option<String>,
}

/// A watched video together with when it was last watched and where playback stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub video: Video,
    pub watched_at: DateTime<Utc>,
    pub position_secs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadStatus {
    Queued,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

/// A download as persisted in the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub id: i64,
    pub video: Video,
    pub kind: DownloadKind,
    pub path: PathBuf,
    pub status: DownloadStatus,
    pub created_at: DateTime<Utc>,
}

/// Byte ranges locating the initialization data and the segment index in a
/// stream file, needed to stream it with DASH.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentRanges {
    pub init: (u32, u32),
    pub index: (u32, u32),
}

/// A stream containing video (and, if muxed, audio).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoStream {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub fps: u8,
    pub bitrate: u32,
    /// MIME type including codecs, e.g. `video/mp4; codecs="avc1.4d401e"`.
    pub mime: String,
    pub ranges: Option<SegmentRanges>,
    /// File size in bytes, if known.
    pub size: Option<u64>,
}

/// An audio-only stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioStream {
    pub url: String,
    pub bitrate: u32,
    pub mime: String,
    pub ranges: Option<SegmentRanges>,
    /// File size in bytes, if known.
    pub size: Option<u64>,
}

/// Everything needed to play a video.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Streams {
    /// Streams with both video and audio.
    pub muxed: Vec<VideoStream>,
    pub video_only: Vec<VideoStream>,
    pub audio: Vec<AudioStream>,
    /// HLS manifest (always present for live streams, sometimes for others).
    pub hls_url: Option<String>,
    pub is_live: bool,
    pub duration_secs: Option<u32>,
}

/// An item in search results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchItem {
    Video(Video),
    Channel(Channel),
}

/// Full information about a video, shown on its detail page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoDetails {
    pub video: Video,
    pub channel: Channel,
    pub description: String,
    pub view_count: Option<u64>,
    pub like_count: Option<u32>,
    pub related: Vec<Video>,
    /// The comment count and the token of the first page; no comments yet.
    pub comments: CommentPage,
}

/// A comment under a video.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// Missing for some comments, e.g. of deleted accounts.
    pub author: Option<Channel>,
    pub text: String,
    pub published: Option<DateTime<Utc>>,
    pub like_count: Option<u32>,
    pub reply_count: u32,
    pub pinned: bool,
    /// Written by the channel that uploaded the video.
    pub by_owner: bool,
}

/// A page of comments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommentPage {
    pub comments: Vec<Comment>,
    /// Total number of comments, if known.
    pub count: Option<u64>,
    /// Token to fetch the next page with; `None` on the last page.
    pub next: Option<String>,
}

/// What the player should open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackSource {
    /// A single URI with video and audio (muxed stream, HLS manifest or local file).
    Uri(String),
    /// Separate video and audio streams played in sync.
    Split { video: String, audio: String },
    /// A DASH manifest (MPD XML). Lets the player fetch streams in bounded
    /// byte ranges, which YouTube requires for some videos.
    Dash(String),
}
