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
