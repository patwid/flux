//! Subscription feeds from YouTube's per-channel RSS (Atom) endpoint.

use super::model::Video;

#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("invalid feed: {0}")]
    Parse(#[from] feed_rs::parser::ParseFeedError),
}

/// The RSS feed URL of a channel.
pub fn feed_url(channel_id: &str) -> String {
    format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}")
}

/// Parses a YouTube channel feed into videos, in feed order.
pub fn parse_channel_feed(xml: &[u8]) -> Result<Vec<Video>, FeedError> {
    let feed = feed_rs::parser::parse(xml)?;
    Ok(feed
        .entries
        .into_iter()
        .filter_map(entry_to_video)
        .collect())
}

fn entry_to_video(entry: feed_rs::model::Entry) -> Option<Video> {
    let id = entry.id.strip_prefix("yt:video:")?.to_owned();
    // The feed-level `yt:channelId` lacks the "UC" prefix, so take the id
    // from the author's channel URL instead.
    let author = entry.authors.into_iter().next()?;
    let channel_id = author.uri?.rsplit('/').next()?.to_owned();
    let thumbnail_url = entry
        .media
        .iter()
        .flat_map(|m| &m.thumbnails)
        .next()
        .map(|t| t.image.uri.clone());
    Some(Video {
        id,
        title: entry.title.map(|t| t.content).unwrap_or_default(),
        channel_id,
        channel_name: author.name.unwrap_or_default(),
        published: entry.published,
        duration_secs: None,
        thumbnail_url,
    })
}

/// Merges the videos of several channel feeds into one feed, newest first,
/// without duplicates.
pub fn aggregate(feeds: impl IntoIterator<Item = Vec<Video>>) -> Vec<Video> {
    let mut seen = std::collections::HashSet::new();
    let mut videos: Vec<Video> = feeds
        .into_iter()
        .flatten()
        .filter(|v| seen.insert(v.id.clone()))
        .collect();
    // `None` sorts before `Some`, so reversing puts undated videos last.
    videos.sort_by_key(|v| std::cmp::Reverse(v.published));
    videos
}

/// Fills in missing publication dates of `videos` from feed entries with the
/// same id.
pub fn fill_dates(videos: &mut [Video], feed: &[Video]) {
    for video in videos.iter_mut().filter(|v| v.published.is_none()) {
        video.published = feed
            .iter()
            .find(|f| f.id == video.id)
            .and_then(|f| f.published);
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/channel_feed.xml");

    fn video(id: &str, published: i64) -> Video {
        Video {
            id: id.into(),
            title: id.into(),
            channel_id: "UC1".into(),
            channel_name: "Chan".into(),
            published: DateTime::from_timestamp(published, 0),
            duration_secs: None,
            thumbnail_url: None,
        }
    }

    #[test]
    fn builds_feed_url() {
        assert_eq!(
            feed_url("UCsXVk37bltHxD1rDPwtNM8Q"),
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCsXVk37bltHxD1rDPwtNM8Q"
        );
    }

    #[test]
    fn parses_youtube_channel_feed() {
        let videos = parse_channel_feed(FIXTURE).unwrap();

        assert_eq!(videos.len(), 2);
        assert_eq!(
            videos[0],
            Video {
                id: "dQw4w9WgXcQ".into(),
                title: "The Newest Video & More".into(),
                channel_id: "UCsXVk37bltHxD1rDPwtNM8Q".into(),
                channel_name: "Kurzgesagt – In a Nutshell".into(),
                published: Some("2026-10-20T14:00:07Z".parse().unwrap()),
                duration_secs: None,
                thumbnail_url: Some("https://i2.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg".into()),
            }
        );
        assert_eq!(videos[1].id, "9bZkp7q19f0");
    }

    #[test]
    fn rejects_invalid_xml() {
        assert!(parse_channel_feed(b"<html>not a feed").is_err());
    }

    #[test]
    fn aggregates_newest_first_without_duplicates() {
        let a = vec![video("a1", 100), video("a2", 300)];
        let b = vec![video("b1", 200), video("a2", 300), video("b2", 50)];

        let ids: Vec<_> = aggregate([a, b]).into_iter().map(|v| v.id).collect();
        assert_eq!(ids, ["a2", "b1", "a1", "b2"]);
    }

    #[test]
    fn aggregate_puts_undated_videos_last() {
        let mut undated = video("u", 0);
        undated.published = None;

        let ids: Vec<_> = aggregate([vec![undated, video("d", 1)]])
            .into_iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(ids, ["d", "u"]);
    }

    #[test]
    fn fills_missing_dates_from_feed() {
        let mut undated = video("a", 0);
        undated.published = None;
        let mut videos = vec![undated, video("b", 5), video("c", 0)];
        videos[2].published = None;

        fill_dates(&mut videos, &[video("a", 10), video("b", 99)]);

        assert_eq!(videos[0].published, DateTime::from_timestamp(10, 0));
        assert_eq!(videos[1].published, DateTime::from_timestamp(5, 0), "kept");
        assert_eq!(videos[2].published, None, "not in feed");
    }

    #[test]
    fn parses_real_feed() {
        let videos =
            parse_channel_feed(include_bytes!("../../tests/fixtures/channel_feed_real.xml"))
                .unwrap();
        assert_eq!(videos.len(), 15);
        assert!(
            videos
                .iter()
                .all(|v| v.channel_id == "UCsXVk37bltHxD1rDPwtNM8Q")
        );
        assert!(
            videos
                .iter()
                .all(|v| v.id.len() == 11 && v.published.is_some())
        );
    }
}
