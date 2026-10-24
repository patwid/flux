//! Choosing which streams to play or download.

use super::dash;
use super::model::{AudioStream, PlaybackSource, Streams, VideoStream};

/// Highest resolution played and downloaded by default.
pub const DEFAULT_MAX_HEIGHT: u32 = 720;

/// Picks the best video stream not exceeding `max_height`. If every stream
/// is taller, the smallest one is used. Among equal heights H.264 is
/// preferred (hardware decoding on mobile devices), then higher fps, then
/// higher bitrate.
pub fn select_video(streams: &[VideoStream], max_height: u32) -> Option<&VideoStream> {
    let rank = |s: &&VideoStream| (s.height, s.mime.contains("avc1"), s.fps, s.bitrate);
    let fitting = streams
        .iter()
        .filter(|s| s.height <= max_height)
        .max_by_key(rank);
    fitting.or_else(|| {
        let min_height = streams.iter().map(|s| s.height).min()?;
        streams
            .iter()
            .filter(|s| s.height == min_height)
            .max_by_key(rank)
    })
}

/// Picks the audio stream with the highest bitrate.
pub fn select_audio(streams: &[AudioStream]) -> Option<&AudioStream> {
    streams.iter().max_by_key(|s| s.bitrate)
}

/// Chooses how to play a video: live streams use HLS (also the fallback when
/// nothing else is available). Otherwise a DASH manifest of MP4 streams is
/// preferred (YouTube only serves some videos in bounded byte ranges); else separate
/// video and audio streams are used when they offer a higher resolution
/// (up to `max_height`) than the best muxed stream.
pub fn choose_source(streams: &Streams, max_height: u32) -> Option<PlaybackSource> {
    let hls = streams.hls_url.clone().map(PlaybackSource::Uri);
    if streams.is_live {
        return hls;
    }
    let muxed = select_video(&streams.muxed, max_height);
    if let Some((height, manifest)) = dash_source(streams, max_height)
        && muxed.is_none_or(|m| height > m.height)
    {
        return Some(PlaybackSource::Dash(manifest));
    }
    let split = select_video(&streams.video_only, max_height).zip(select_audio(&streams.audio));
    match (muxed, split) {
        (Some(m), Some((v, _))) if m.height >= v.height => Some(PlaybackSource::Uri(m.url.clone())),
        (_, Some((v, a))) => Some(PlaybackSource::Split {
            video: v.url.clone(),
            audio: a.url.clone(),
        }),
        (Some(m), None) => Some(PlaybackSource::Uri(m.url.clone())),
        (None, None) => hls,
    }
}

/// A DASH manifest for the best MP4 video and audio streams with known
/// segment ranges, with the video height.
fn dash_source(streams: &Streams, max_height: u32) -> Option<(u32, String)> {
    let videos: Vec<_> = streams
        .video_only
        .iter()
        .filter(|s| s.mime.starts_with("video/mp4") && s.ranges.is_some())
        .cloned()
        .collect();
    let audios: Vec<_> = streams
        .audio
        .iter()
        .filter(|s| s.mime.starts_with("audio/mp4") && s.ranges.is_some())
        .cloned()
        .collect();
    let video = select_video(&videos, max_height)?;
    let audio = select_audio(&audios)?;
    let manifest = dash::manifest(video, audio, streams.duration_secs?)?;
    Some((video.height, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(height: u32, fps: u8, bitrate: u32, codec: &str) -> VideoStream {
        VideoStream {
            url: format!("{height}-{fps}-{bitrate}-{codec}"),
            width: height * 16 / 9,
            height,
            fps,
            bitrate,
            mime: format!("video/mp4; codecs=\"{codec}\""),
            ranges: None,
            size: None,
        }
    }

    fn audio(bitrate: u32) -> AudioStream {
        AudioStream {
            url: bitrate.to_string(),
            bitrate,
            mime: "audio/webm; codecs=\"opus\"".into(),
            ranges: None,
            size: None,
        }
    }

    #[test]
    fn no_streams_selects_nothing() {
        assert_eq!(select_video(&[], 720), None);
        assert_eq!(select_audio(&[]), None);
    }

    #[test]
    fn selects_highest_height_within_limit() {
        let streams = [
            video(360, 30, 1, "avc1"),
            video(1080, 30, 1, "avc1"),
            video(720, 30, 1, "avc1"),
        ];
        assert_eq!(select_video(&streams, 720).unwrap().height, 720);
        assert_eq!(select_video(&streams, 2160).unwrap().height, 1080);
    }

    #[test]
    fn falls_back_to_smallest_when_all_exceed_limit() {
        let streams = [video(1080, 30, 1, "avc1"), video(720, 30, 1, "avc1")];
        assert_eq!(select_video(&streams, 360).unwrap().height, 720);
    }

    #[test]
    fn prefers_h264_then_fps_then_bitrate() {
        let vp9 = video(720, 60, 9, "vp9");
        let avc_30 = video(720, 30, 5, "avc1.4d401f");
        let avc_60_low = video(720, 60, 3, "avc1.4d4020");
        let avc_60_high = video(720, 60, 4, "avc1.4d4020");
        let streams = [vp9, avc_30, avc_60_low, avc_60_high.clone()];

        assert_eq!(select_video(&streams, 720), Some(&avc_60_high));
    }

    #[test]
    fn selects_highest_audio_bitrate() {
        let streams = [audio(64), audio(160), audio(128)];
        assert_eq!(select_audio(&streams).unwrap().bitrate, 160);
    }

    fn streams(muxed: &[u32], video_only: &[u32], audio_bitrates: &[u32]) -> Streams {
        Streams {
            muxed: muxed.iter().map(|&h| video(h, 30, 1, "avc1")).collect(),
            video_only: video_only
                .iter()
                .map(|&h| video(h, 30, 2, "avc1"))
                .collect(),
            audio: audio_bitrates.iter().map(|&b| audio(b)).collect(),
            hls_url: None,
            is_live: false,
            duration_secs: Some(600),
            subtitles: vec![],
        }
    }

    #[test]
    fn live_streams_use_hls() {
        let mut s = streams(&[360], &[1080], &[128]);
        s.hls_url = Some("https://hls".into());
        s.is_live = true;
        assert_eq!(
            choose_source(&s, 720),
            Some(PlaybackSource::Uri("https://hls".into()))
        );
    }

    #[test]
    fn regular_videos_prefer_streams_over_hls() {
        let mut s = streams(&[], &[720], &[128]);
        s.hls_url = Some("https://hls".into());
        assert!(matches!(
            choose_source(&s, 720),
            Some(PlaybackSource::Split { .. })
        ));
    }

    #[test]
    fn hls_is_the_fallback() {
        let s = Streams {
            hls_url: Some("https://hls".into()),
            ..Default::default()
        };
        assert_eq!(
            choose_source(&s, 720),
            Some(PlaybackSource::Uri("https://hls".into()))
        );
    }

    #[test]
    fn split_streams_when_higher_resolution() {
        let s = streams(&[360], &[480, 720, 1080], &[64, 128]);
        assert_eq!(
            choose_source(&s, 720),
            Some(PlaybackSource::Split {
                video: "720-30-2-avc1".into(),
                audio: "128".into()
            })
        );
    }

    #[test]
    fn muxed_when_not_worse() {
        let s = streams(&[360], &[360], &[128]);
        assert_eq!(
            choose_source(&s, 720),
            Some(PlaybackSource::Uri("360-30-1-avc1".into()))
        );
    }

    #[test]
    fn muxed_when_no_audio_stream() {
        let s = streams(&[360], &[1080], &[]);
        assert_eq!(
            choose_source(&s, 1080),
            Some(PlaybackSource::Uri("360-30-1-avc1".into()))
        );
    }

    #[test]
    fn split_when_no_muxed_stream() {
        let s = streams(&[], &[1080], &[128]);
        assert!(matches!(
            choose_source(&s, 720),
            Some(PlaybackSource::Split { .. })
        ));
    }

    #[test]
    fn nothing_playable() {
        assert_eq!(choose_source(&Streams::default(), 720), None);
    }

    fn with_ranges(mut s: Streams) -> Streams {
        let ranges = Some(crate::core::model::SegmentRanges {
            init: (0, 10),
            index: (11, 20),
        });
        for v in &mut s.video_only {
            v.ranges = ranges.clone();
        }
        for a in &mut s.audio {
            a.ranges = ranges.clone();
        }
        s
    }

    #[test]
    fn dash_when_segment_ranges_are_known() {
        let mut s = with_ranges(streams(&[360], &[480, 720], &[64]));
        s.audio[0].mime = "audio/mp4; codecs=\"mp4a.40.2\"".into();
        let expected = crate::core::dash::manifest(&s.video_only[1], &s.audio[0], 600).unwrap();
        assert_eq!(choose_source(&s, 720), Some(PlaybackSource::Dash(expected)));
    }

    #[test]
    fn dash_uses_mp4_streams_only() {
        let mut s = with_ranges(streams(&[], &[720], &[160, 128]));
        s.audio[1].mime = "audio/mp4; codecs=\"mp4a.40.2\"".into();
        let mut webm = s.video_only[0].clone();
        webm.mime = "video/webm; codecs=\"vp9\"".into();
        webm.fps = 60;
        s.video_only.push(webm);

        let Some(PlaybackSource::Dash(mpd)) = choose_source(&s, 720) else {
            panic!("expected DASH");
        };
        assert!(mpd.contains("mp4a.40.2") && mpd.contains("avc1") && !mpd.contains("vp9"));
    }

    #[test]
    fn split_when_no_mp4_audio_for_dash() {
        let s = with_ranges(streams(&[], &[720], &[128]));
        assert!(matches!(
            choose_source(&s, 720),
            Some(PlaybackSource::Split { .. })
        ));
    }
}
