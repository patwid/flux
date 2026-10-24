//! Choosing which streams to play or download.

use super::model::{AudioStream, VideoStream};

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
        }
    }

    fn audio(bitrate: u32) -> AudioStream {
        AudioStream {
            url: bitrate.to_string(),
            bitrate,
            mime: "audio/webm; codecs=\"opus\"".into(),
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
}
