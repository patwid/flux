//! Human-readable formatting of model values.

use chrono::{DateTime, Utc};

use crate::core::model::{
    Download, DownloadKind, DownloadStatus, HistoryEntry, Video, VideoDetails,
};

/// Formats a duration as `m:ss` or `h:mm:ss`.
pub fn duration(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Secondary line for a video row: channel name, age and duration.
pub fn video_subtitle(video: &Video, now: DateTime<Utc>) -> String {
    let duration = video.duration_secs.map(duration);
    let published = video.published.map(|p| relative_date(p, now));
    [Some(video.channel_name.clone()), published, duration]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Formats a count compactly, e.g. `999`, `1.2K`, `3.4M`, `1.1B`.
pub fn count(n: u64) -> String {
    let Some((unit, suffix)) = [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")]
        .into_iter()
        .find(|(unit, _)| n >= *unit)
    else {
        return n.to_string();
    };
    // Truncate like YouTube does: 1,999 is "1.9K", not "2K".
    let tenths = n / (unit / 10);
    if tenths < 100 && !tenths.is_multiple_of(10) {
        format!("{}.{}{suffix}", tenths / 10, tenths % 10)
    } else {
        format!("{}{suffix}", n / unit)
    }
}

/// Formats how long ago `time` was, e.g. `just now`, `5 minutes ago`, `2 years ago`.
pub fn relative_date(time: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - time).num_seconds();
    let days = secs / 86_400;
    let (n, unit) = if secs < 60 {
        return "just now".into();
    } else if secs < 3600 {
        (secs / 60, "minute")
    } else if secs < 86_400 {
        (secs / 3600, "hour")
    } else if days < 7 {
        (days, "day")
    } else if days < 30 {
        (days / 7, "week")
    } else if days < 365 {
        (days / 30, "month")
    } else {
        (days / 365, "year")
    };
    let plural = if n == 1 { "" } else { "s" };
    format!("{n} {unit}{plural} ago")
}

/// Meta line of a video page: view count and publication date.
pub fn details_meta(details: &VideoDetails) -> String {
    let views = details.view_count.map(|n| format!("{} views", count(n)));
    let published = details
        .video
        .published
        .map(|p| relative_date(p, Utc::now()));
    [views, published]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Secondary line for a history row: channel, when it was watched and how far.
pub fn history_subtitle(entry: &HistoryEntry, now: DateTime<Utc>) -> String {
    let mut progress = duration(entry.position_secs);
    if let Some(total) = entry.video.duration_secs {
        progress = format!("{progress} / {}", duration(total));
    }
    [
        entry.video.channel_name.clone(),
        relative_date(entry.watched_at, now),
        progress,
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

/// Secondary line for a download row; `progress` is from 0 to 1.
pub fn download_status(download: &Download, progress: Option<f64>) -> String {
    match &download.status {
        DownloadStatus::Queued => "Queued".into(),
        DownloadStatus::Running => match progress {
            Some(p) => format!("Downloading · {} %", (p * 100.0).floor() as u32),
            None => "Downloading…".into(),
        },
        DownloadStatus::Completed => {
            let kind = match download.kind {
                DownloadKind::Video => "Video",
                DownloadKind::Audio => "Audio",
            };
            format!("{} · {kind}", download.video.channel_name)
        }
        DownloadStatus::Failed(e) => format!("Failed: {e}"),
        DownloadStatus::Cancelled => "Cancelled".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(duration(0), "0:00");
        assert_eq!(duration(65), "1:05");
        assert_eq!(duration(3600 + 2 * 60 + 3), "1:02:03");
    }

    #[test]
    fn video_subtitle_joins_known_parts() {
        let mut video = Video {
            id: "a".into(),
            title: "t".into(),
            channel_id: "UC1".into(),
            channel_name: "Chan".into(),
            published: None,
            duration_secs: Some(65),
            thumbnail_url: None,
        };
        let now: DateTime<Utc> = "2026-10-24T12:00:00Z".parse().unwrap();
        assert_eq!(video_subtitle(&video, now), "Chan · 1:05");

        video.published = Some(now - chrono::Duration::days(3));
        assert_eq!(video_subtitle(&video, now), "Chan · 3 days ago · 1:05");

        video.duration_secs = None;
        video.published = None;
        assert_eq!(video_subtitle(&video, now), "Chan");

        video.channel_name.clear();
        assert_eq!(video_subtitle(&video, now), "");
    }

    #[test]
    fn formats_counts() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000), "1K");
        assert_eq!(count(1_234), "1.2K");
        assert_eq!(count(56_789), "56K");
        assert_eq!(count(3_400_000), "3.4M");
        assert_eq!(count(1_100_000_000), "1.1B");
    }

    #[test]
    fn formats_relative_dates() {
        let now: DateTime<Utc> = "2026-10-24T12:00:00Z".parse().unwrap();
        let ago = |secs: i64| relative_date(now - chrono::Duration::seconds(secs), now);
        assert_eq!(ago(-30), "just now");
        assert_eq!(ago(30), "just now");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(5 * 60), "5 minutes ago");
        assert_eq!(ago(3 * 3600), "3 hours ago");
        assert_eq!(ago(86_400), "1 day ago");
        assert_eq!(ago(3 * 7 * 86_400), "3 weeks ago");
        assert_eq!(ago(65 * 86_400), "2 months ago");
        assert_eq!(ago(800 * 86_400), "2 years ago");
    }

    #[test]
    fn history_subtitle_shows_progress() {
        let now: DateTime<Utc> = "2026-10-24T12:00:00Z".parse().unwrap();
        let mut entry = HistoryEntry {
            video: Video {
                id: "a".into(),
                title: "t".into(),
                channel_id: "UC1".into(),
                channel_name: "Chan".into(),
                published: None,
                duration_secs: Some(600),
                thumbnail_url: None,
            },
            watched_at: now - chrono::Duration::days(2),
            position_secs: 200,
        };
        assert_eq!(
            history_subtitle(&entry, now),
            "Chan · 2 days ago · 3:20 / 10:00"
        );

        entry.video.duration_secs = None;
        assert_eq!(history_subtitle(&entry, now), "Chan · 2 days ago · 3:20");
    }

    #[test]
    fn download_status_text() {
        let mut d = Download {
            id: 1,
            video: Video {
                id: "a".into(),
                title: "t".into(),
                channel_id: "UC1".into(),
                channel_name: "Chan".into(),
                published: None,
                duration_secs: None,
                thumbnail_url: None,
            },
            kind: DownloadKind::Audio,
            path: "/d/a.m4a".into(),
            status: DownloadStatus::Queued,
            created_at: DateTime::from_timestamp(0, 0).unwrap(),
        };
        assert_eq!(download_status(&d, None), "Queued");
        d.status = DownloadStatus::Running;
        assert_eq!(download_status(&d, None), "Downloading…");
        assert_eq!(download_status(&d, Some(0.456)), "Downloading · 45 %");
        d.status = DownloadStatus::Completed;
        assert_eq!(download_status(&d, None), "Chan · Audio");
        d.kind = DownloadKind::Video;
        assert_eq!(download_status(&d, None), "Chan · Video");
        d.status = DownloadStatus::Failed("HTTP 403".into());
        assert_eq!(download_status(&d, None), "Failed: HTTP 403");
        d.status = DownloadStatus::Cancelled;
        assert_eq!(download_status(&d, None), "Cancelled");
    }
}
