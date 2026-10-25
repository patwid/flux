//! Channel playlists and playlist videos from YouTube's browse responses.
//!
//! YouTube now lists both as lockup view models, which rustypipe 0.11 does
//! not parse (and it requests the playlists tab with outdated parameters),
//! so the responses are fetched raw and parsed here.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::{BackendError, Result};
use crate::core::model::{Playlist, Video};

/// The JSON body of a browse request for a channel's playlists tab.
pub fn channel_playlists_request(channel_id: &str) -> Value {
    json!({ "browseId": channel_id, "params": "EglwbGF5bGlzdHPyBgQKAkIA" })
}

/// The JSON body of a browse request for a playlist.
pub fn playlist_request(playlist_id: &str) -> Value {
    json!({ "browseId": format!("VL{playlist_id}") })
}

/// The playlists in a response to [`channel_playlists_request`].
pub fn parse_playlists(json: &str) -> Result<Vec<Playlist>> {
    let response = parse(json)?;
    Ok(lockups(&response)?
        .into_iter()
        .filter(|l| l["contentType"] != VIDEO)
        .filter_map(playlist)
        .collect())
}

/// The videos in a response to [`playlist_request`]; their age is relative
/// to `now`.
pub fn parse_videos(json: &str, now: DateTime<Utc>) -> Result<Vec<Video>> {
    let response = parse(json)?;
    Ok(lockups(&response)?
        .into_iter()
        .filter(|l| l["contentType"] == VIDEO)
        .filter_map(|l| video(l, now))
        .collect())
}

const VIDEO: &str = "LOCKUP_CONTENT_TYPE_VIDEO";

fn parse(json: &str) -> Result<Value> {
    serde_json::from_str(json)
        .map_err(|e| BackendError::Other(format!("invalid browse response: {e}")))
}

/// The lockup view models in the contents of a browse response.
fn lockups(response: &Value) -> Result<Vec<&Value>> {
    let contents = response
        .get("contents")
        .ok_or_else(|| BackendError::Other("browse response without contents".into()))?;
    let mut found = Vec::new();
    find_all(contents, "lockupViewModel", &mut found);
    Ok(found)
}

/// Every value under `key` in `value`, in document order, not looking
/// inside the found values.
fn find_all<'a>(value: &'a Value, key: &str, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == key {
                    found.push(v);
                } else {
                    find_all(v, key, found);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| find_all(v, key, found)),
        _ => {}
    }
}

fn playlist(lockup: &Value) -> Option<Playlist> {
    Some(Playlist {
        id: lockup["contentId"].as_str()?.into(),
        name: title(lockup)?,
        thumbnail_url: thumbnail(lockup),
        video_count: badges(lockup).find_map(count),
    })
}

fn video(lockup: &Value, now: DateTime<Utc>) -> Option<Video> {
    let parts = metadata_parts(lockup);
    let (channel_id, channel_name) = parts
        .iter()
        .find_map(|part| {
            let id =
                part["commandRuns"][0]["onTap"]["innertubeCommand"]["browseEndpoint"]["browseId"]
                    .as_str()?;
            Some((id.to_string(), part["content"].as_str()?.to_string()))
        })
        .unwrap_or_default();
    Some(Video {
        id: lockup["contentId"].as_str()?.into(),
        title: title(lockup)?,
        channel_id,
        channel_name,
        published: parts
            .iter()
            .find_map(|part| age(part["content"].as_str()?, now)),
        duration_secs: badges(lockup).find_map(duration),
        thumbnail_url: thumbnail(lockup),
    })
}

fn title(lockup: &Value) -> Option<String> {
    let title = &lockup["metadata"]["lockupMetadataViewModel"]["title"]["content"];
    title.as_str().map(Into::into)
}

/// The texts of the metadata rows below the title.
fn metadata_parts(lockup: &Value) -> Vec<&Value> {
    let rows = &lockup["metadata"]["lockupMetadataViewModel"]["metadata"]["contentMetadataViewModel"]
        ["metadataRows"];
    rows.as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["metadataParts"].as_array())
        .flatten()
        .map(|part| &part["text"])
        .collect()
}

/// The URL of the largest image of the lockup's thumbnail.
fn thumbnail(lockup: &Value) -> Option<String> {
    let mut images = Vec::new();
    find_all(&lockup["contentImage"], "image", &mut images);
    images
        .first()?
        .get("sources")?
        .as_array()?
        .iter()
        .max_by_key(|s| s["width"].as_u64().unwrap_or(0) * s["height"].as_u64().unwrap_or(0))
        .and_then(|s| s["url"].as_str())
        .map(Into::into)
}

/// The texts of the badges on the lockup's thumbnail, e.g. its duration.
fn badges(lockup: &Value) -> impl Iterator<Item = &str> {
    let mut found = Vec::new();
    find_all(
        &lockup["contentImage"],
        "thumbnailBadgeViewModel",
        &mut found,
    );
    found.into_iter().filter_map(|badge| badge["text"].as_str())
}

/// Seconds of a duration such as `8:18` or `1:02:03`.
fn duration(text: &str) -> Option<u32> {
    text.split(':')
        .try_fold(0, |secs, part| Some(secs * 60 + part.parse::<u32>().ok()?))
}

/// The number at the start of a text such as `1,234 videos`.
fn count(text: &str) -> Option<u64> {
    let digits: String = text
        .split_whitespace()
        .next()?
        .chars()
        .filter(|&c| c != ',')
        .collect();
    digits.parse().ok()
}

/// The time of an English relative date such as `3 weeks ago`.
fn age(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let words: Vec<_> = text.split_whitespace().collect();
    let [.., n, unit, "ago"] = words[..] else {
        return None;
    };
    let n: u32 = n.parse().ok()?;
    let months = |m: u32| now.checked_sub_months(chrono::Months::new(m));
    match unit.trim_end_matches('s') {
        "second" => Some(now - chrono::Duration::seconds(n.into())),
        "minute" => Some(now - chrono::Duration::minutes(n.into())),
        "hour" => Some(now - chrono::Duration::hours(n.into())),
        "day" => Some(now - chrono::Duration::days(n.into())),
        "week" => Some(now - chrono::Duration::weeks(n.into())),
        "month" => months(n),
        "year" => months(n * 12),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYLISTS: &str = include_str!("../../../tests/fixtures/channel_playlists.json");
    const VIDEOS: &str = include_str!("../../../tests/fixtures/playlist.json");

    fn now() -> DateTime<Utc> {
        "2026-10-25T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn parses_playlists_of_every_kind() {
        let playlists = parse_playlists(PLAYLISTS).unwrap();
        let summary: Vec<_> = playlists
            .iter()
            .map(|p| (p.id.as_str(), p.name.as_str(), p.video_count))
            .collect();
        assert_eq!(
            summary,
            [
                ("PL96C35uN7xGIhE_SS9C4PT0RrhU2DVYR3", "England", Some(35)),
                (
                    "PL96C35uN7xGLr6zFFMabALFOFy6RnZhir",
                    "📶 Disconnected",
                    Some(4)
                ),
                (
                    "PL96C35uN7xGJu6skU4TBYrIWxggkZBrF5",
                    "How To Build An App: Everything You Didn't Know You Needed To Know",
                    Some(16)
                ),
            ]
        );
        assert!(
            playlists[0]
                .thumbnail_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://i.ytimg.com/")),
            "{:?}",
            playlists[0].thumbnail_url
        );
    }

    #[test]
    fn parses_playlist_videos() {
        let videos = parse_videos(VIDEOS, now()).unwrap();
        assert_eq!(videos.len(), 3);
        let video = &videos[0];
        assert_eq!(video.id, "UjtOGPJ0URM");
        assert_eq!(
            video.title,
            "Why Alien Life Would be our Doom - The Great Filter"
        );
        assert_eq!(video.channel_id, "UCsXVk37bltHxD1rDPwtNM8Q");
        assert_eq!(video.channel_name, "Kurzgesagt – In a Nutshell");
        assert_eq!(video.duration_secs, Some(8 * 60 + 18));
        assert_eq!(
            video.published,
            Some("2018-10-25T12:00:00Z".parse().unwrap()),
            "8 years ago"
        );
        assert!(
            video
                .thumbnail_url
                .as_deref()
                .unwrap()
                .contains("UjtOGPJ0URM")
        );
    }

    #[test]
    fn other_responses_are_errors() {
        assert!(parse_playlists("not json").is_err());
        assert!(parse_videos("{}", now()).is_err());
    }

    #[test]
    fn parses_durations() {
        assert_eq!(duration("8:18"), Some(498));
        assert_eq!(duration("1:02:03"), Some(3723));
        assert_eq!(duration("LIVE"), None);
    }

    #[test]
    fn parses_counts() {
        assert_eq!(count("35 episodes"), Some(35));
        assert_eq!(count("1,234 videos"), Some(1234));
        assert_eq!(count("Mix"), None);
    }

    #[test]
    fn parses_ages() {
        let at = |text: &str| age(text, now()).map(|t| t.to_rfc3339());
        assert_eq!(at("1 day ago").unwrap(), "2026-10-24T12:00:00+00:00");
        assert_eq!(at("3 weeks ago").unwrap(), "2026-10-04T12:00:00+00:00");
        assert_eq!(
            at("Streamed 2 months ago").unwrap(),
            "2026-08-25T12:00:00+00:00"
        );
        assert_eq!(at("1 year ago").unwrap(), "2025-10-25T12:00:00+00:00");
        assert_eq!(at("27M views"), None);
    }
}
