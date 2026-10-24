//! Building DASH manifests for separate video and audio streams.

use super::model::{AudioStream, SegmentRanges, VideoStream};

/// The `codecs` parameter of a MIME type such as `video/mp4; codecs="avc1.4d401f"`.
pub fn codecs(mime: &str) -> Option<&str> {
    let (_, rest) = mime.split_once("codecs=\"")?;
    rest.split('"').next()
}

/// An on-demand DASH manifest for one video and one audio stream. Returns
/// `None` if a stream lacks segment ranges or codecs.
pub fn manifest(video: &VideoStream, audio: &AudioStream, duration_secs: u32) -> Option<String> {
    let video_set = adaptation_set(
        &video.mime,
        &format!(
            "<Representation id=\"video\" codecs=\"{}\" bandwidth=\"{}\" width=\"{}\" height=\"{}\" frameRate=\"{}\">",
            codecs(&video.mime)?,
            video.bitrate,
            video.width,
            video.height,
            video.fps
        ),
        &video.url,
        video.ranges.as_ref()?,
    );
    let audio_set = adaptation_set(
        &audio.mime,
        &format!(
            "<Representation id=\"audio\" codecs=\"{}\" bandwidth=\"{}\">",
            codecs(&audio.mime)?,
            audio.bitrate
        ),
        &audio.url,
        audio.ranges.as_ref()?,
    );
    Some(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" profiles="urn:mpeg:dash:profile:isoff-on-demand:2011" type="static" mediaPresentationDuration="PT{duration_secs}S" minBufferTime="PT2S">
<Period>
{video_set}{audio_set}</Period>
</MPD>
"#
    ))
}

fn adaptation_set(mime: &str, representation: &str, url: &str, ranges: &SegmentRanges) -> String {
    let mime_type = mime.split(';').next().unwrap_or(mime);
    let (init, index) = (ranges.init, ranges.index);
    format!(
        r#"<AdaptationSet mimeType="{mime_type}" subsegmentAlignment="true">
{representation}
<BaseURL>{}</BaseURL>
<SegmentBase indexRange="{}-{}"><Initialization range="{}-{}"/></SegmentBase>
</Representation>
</AdaptationSet>
"#,
        escape(url),
        index.0,
        index.1,
        init.0,
        init.1
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges() -> Option<SegmentRanges> {
        Some(SegmentRanges {
            init: (0, 739),
            index: (740, 1234),
        })
    }

    fn video() -> VideoStream {
        VideoStream {
            url: "https://v.example/videoplayback?a=1&b=2".into(),
            width: 1280,
            height: 720,
            fps: 25,
            bitrate: 3_000_000,
            mime: "video/mp4; codecs=\"avc1.4D401F\"".into(),
            ranges: ranges(),
            size: None,
        }
    }

    fn audio() -> AudioStream {
        AudioStream {
            url: "https://a.example/videoplayback?c=3".into(),
            bitrate: 130_000,
            mime: "audio/mp4; codecs=\"mp4a.40.2\"".into(),
            ranges: Some(SegmentRanges {
                init: (0, 631),
                index: (632, 999),
            }),
            size: None,
        }
    }

    #[test]
    fn parses_codecs() {
        assert_eq!(
            codecs("video/mp4; codecs=\"avc1.4D401F\""),
            Some("avc1.4D401F")
        );
        assert_eq!(codecs("audio/webm; codecs=\"opus\""), Some("opus"));
        assert_eq!(codecs("video/mp4"), None);
    }

    #[test]
    fn builds_manifest() {
        let mpd = manifest(&video(), &audio(), 213).unwrap();
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" profiles="urn:mpeg:dash:profile:isoff-on-demand:2011" type="static" mediaPresentationDuration="PT213S" minBufferTime="PT2S">
<Period>
<AdaptationSet mimeType="video/mp4" subsegmentAlignment="true">
<Representation id="video" codecs="avc1.4D401F" bandwidth="3000000" width="1280" height="720" frameRate="25">
<BaseURL>https://v.example/videoplayback?a=1&amp;b=2</BaseURL>
<SegmentBase indexRange="740-1234"><Initialization range="0-739"/></SegmentBase>
</Representation>
</AdaptationSet>
<AdaptationSet mimeType="audio/mp4" subsegmentAlignment="true">
<Representation id="audio" codecs="mp4a.40.2" bandwidth="130000">
<BaseURL>https://a.example/videoplayback?c=3</BaseURL>
<SegmentBase indexRange="632-999"><Initialization range="0-631"/></SegmentBase>
</Representation>
</AdaptationSet>
</Period>
</MPD>
"#;
        assert_eq!(mpd, expected);
    }

    #[test]
    fn needs_ranges_and_codecs() {
        let mut v = video();
        v.ranges = None;
        assert_eq!(manifest(&v, &audio(), 1), None);

        let mut a = audio();
        a.mime = "audio/mp4".into();
        assert_eq!(manifest(&video(), &a, 1), None);
    }
}
