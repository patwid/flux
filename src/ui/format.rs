//! Human-readable formatting of model values.

use crate::core::model::Video;

/// Formats a duration as `m:ss` or `h:mm:ss`.
pub fn duration(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Secondary line for a video row: channel name and duration.
pub fn video_subtitle(video: &Video) -> String {
    let duration = video.duration_secs.map(duration);
    [Some(video.channel_name.clone()), duration]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
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
        assert_eq!(video_subtitle(&video), "Chan · 1:05");

        video.duration_secs = None;
        assert_eq!(video_subtitle(&video), "Chan");

        video.channel_name.clear();
        assert_eq!(video_subtitle(&video), "");
    }
}
