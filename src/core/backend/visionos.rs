//! Stream URLs from YouTube's visionOS client.
//!
//! Stream URLs of rustypipe's clients only serve the beginning of a video
//! without a proof-of-origin token, while the visionOS client's URLs serve
//! the whole file. Only the player request is made with this client.

use serde::Deserialize;
use serde_json::json;

use super::{BackendError, Result};
use crate::core::model::{AudioStream, SegmentRanges, Streams, SubtitleTrack, VideoStream};

pub const PLAYER_URL: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
pub const CLIENT_NAME: &str = "VISIONOS";
/// Numeric client id, sent as `X-YouTube-Client-Name`.
pub const CLIENT_ID: &str = "101";
pub const CLIENT_VERSION: &str = "1.02";
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

/// The JSON body of a player request.
pub fn player_request(video_id: &str, visitor_data: &str) -> serde_json::Value {
    json!({
        "context": {
            "client": {
                "clientName": CLIENT_NAME,
                "clientVersion": CLIENT_VERSION,
                "deviceMake": "Apple",
                "deviceModel": "RealityDevice17,1",
                "userAgent": USER_AGENT,
                "osName": "visionOS",
                "osVersion": "26.5.23O471",
                "hl": "en",
                "gl": "US",
                "visitorData": visitor_data,
            }
        },
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    playability_status: PlayabilityStatus,
    #[serde(default)]
    streaming_data: StreamingData,
    video_details: Option<Details>,
    captions: Option<Captions>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Captions {
    player_captions_tracklist_renderer: CaptionTracklist,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptionTracklist {
    #[serde(default)]
    caption_tracks: Vec<CaptionTrack>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptionTrack {
    base_url: String,
    name: Text,
    language_code: String,
    /// `asr` for auto-generated tracks.
    kind: Option<String>,
}

/// Text in either of YouTube's two representations.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Text {
    simple_text: Option<String>,
    #[serde(default)]
    runs: Vec<TextRun>,
}

#[derive(Deserialize)]
struct TextRun {
    text: String,
}

#[derive(Deserialize)]
struct PlayabilityStatus {
    status: String,
    reason: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamingData {
    #[serde(default)]
    formats: Vec<Format>,
    #[serde(default)]
    adaptive_formats: Vec<Format>,
    hls_manifest_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Details {
    length_seconds: Option<String>,
    #[serde(default)]
    is_live: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Format {
    url: Option<String>,
    mime_type: String,
    #[serde(default)]
    bitrate: u32,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<u8>,
    init_range: Option<Range>,
    index_range: Option<Range>,
    content_length: Option<String>,
}

#[derive(Deserialize)]
struct Range {
    start: String,
    end: String,
}

impl Range {
    fn bounds(&self) -> Option<(u32, u32)> {
        Some((self.start.parse().ok()?, self.end.parse().ok()?))
    }
}

impl Format {
    fn ranges(&self) -> Option<SegmentRanges> {
        Some(SegmentRanges {
            init: self.init_range.as_ref()?.bounds()?,
            index: self.index_range.as_ref()?.bounds()?,
        })
    }

    fn video(&self) -> Option<VideoStream> {
        Some(VideoStream {
            url: self.url.clone()?,
            width: self.width?,
            height: self.height?,
            fps: self.fps.unwrap_or(30),
            bitrate: self.bitrate,
            mime: self.mime_type.clone(),
            ranges: self.ranges(),
            size: self.content_length.as_ref().and_then(|l| l.parse().ok()),
        })
    }

    fn audio(&self) -> Option<AudioStream> {
        Some(AudioStream {
            url: self.url.clone()?,
            bitrate: self.bitrate,
            mime: self.mime_type.clone(),
            ranges: self.ranges(),
            size: self.content_length.as_ref().and_then(|l| l.parse().ok()),
        })
    }
}

/// Parses a player response into streams.
pub fn parse_player_response(body: &[u8]) -> Result<Streams> {
    let response: Response = serde_json::from_slice(body)
        .map_err(|e| BackendError::Other(format!("invalid player response: {e}")))?;
    let status = response.playability_status;
    if status.status != "OK" {
        return Err(BackendError::Other(status.reason.unwrap_or(status.status)));
    }
    let data = response.streaming_data;
    let (video, audio): (Vec<_>, Vec<_>) = data
        .adaptive_formats
        .iter()
        .partition(|f| f.mime_type.starts_with("video/"));
    let details = response.video_details;
    Ok(Streams {
        muxed: data.formats.iter().filter_map(Format::video).collect(),
        video_only: video.into_iter().filter_map(Format::video).collect(),
        audio: audio.into_iter().filter_map(Format::audio).collect(),
        hls_url: data.hls_manifest_url,
        is_live: details.as_ref().is_some_and(|d| d.is_live),
        duration_secs: details
            .and_then(|d| d.length_seconds)
            .and_then(|l| l.parse().ok()),
        subtitles: response
            .captions
            .map(|c| c.player_captions_tracklist_renderer.caption_tracks)
            .unwrap_or_default()
            .into_iter()
            .map(|track| SubtitleTrack {
                name: track
                    .name
                    .simple_text
                    .unwrap_or_else(|| track.name.runs.into_iter().map(|run| run.text).collect()),
                lang: track.language_code,
                url: vtt_url(&track.base_url),
                auto_generated: track.kind.as_deref() == Some("asr"),
            })
            .collect(),
    })
}

/// `url` of a timed text track, changed to request WebVTT.
pub fn vtt_url(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.into();
    };
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| key != "fmt")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    parsed
        .query_pairs_mut()
        .clear()
        .extend_pairs(pairs)
        .append_pair("fmt", "vtt");
    parsed.into()
}

/// Requests the streams of a video.
pub async fn fetch_streams(
    http: &reqwest::Client,
    video_id: &str,
    visitor_data: &str,
) -> Result<Streams> {
    let body = http
        .post(PLAYER_URL)
        .header("User-Agent", USER_AGENT)
        .header("X-YouTube-Client-Name", CLIENT_ID)
        .header("X-YouTube-Client-Version", CLIENT_VERSION)
        .header("X-Goog-Visitor-Id", visitor_data)
        .header("Origin", "https://www.youtube.com")
        .json(&player_request(video_id, visitor_data))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    parse_player_response(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/visionos_player.json");

    #[test]
    fn builds_player_request() {
        let body = player_request("abc", "VD");
        assert_eq!(body["videoId"], "abc");
        assert_eq!(body["context"]["client"]["clientName"], CLIENT_NAME);
        assert_eq!(body["context"]["client"]["clientVersion"], CLIENT_VERSION);
        assert_eq!(body["context"]["client"]["visitorData"], "VD");
        assert_eq!(body["contentCheckOk"], true);
    }

    #[test]
    fn parses_adaptive_streams() {
        let streams = parse_player_response(FIXTURE).unwrap();

        assert_eq!(streams.duration_secs, Some(1018));
        assert!(!streams.is_live);
        assert!(streams.muxed.is_empty());
        let heights: Vec<_> = streams.video_only.iter().map(|v| v.height).collect();
        assert_eq!(heights, [720, 720, 480]);
        assert_eq!(
            streams.video_only[0],
            VideoStream {
                url: "https://rr.example/videoplayback?itag=298&a=1".into(),
                width: 1280,
                height: 720,
                fps: 60,
                bitrate: streams.video_only[0].bitrate,
                mime: "video/mp4; codecs=\"avc1.4D4020\"".into(),
                ranges: Some(SegmentRanges {
                    init: (0, 739),
                    index: (740, 3087)
                }),
                size: streams.video_only[0].size,
            }
        );
        assert!(streams.video_only[0].bitrate > 0);
        assert!(streams.video_only[0].size.unwrap() > 100_000_000);
        let mimes: Vec<_> = streams.audio.iter().map(|a| a.mime.as_str()).collect();
        assert_eq!(
            mimes,
            [
                "audio/mp4; codecs=\"mp4a.40.2\"",
                "audio/webm; codecs=\"opus\""
            ]
        );
        assert!(
            streams
                .audio
                .iter()
                .all(|a| a.ranges.is_some() && a.size.is_some())
        );
    }

    #[test]
    fn parses_subtitle_tracks() {
        let body = serde_json::to_vec(&json!({
            "playabilityStatus": {"status": "OK"},
            "captions": {"playerCaptionsTracklistRenderer": {"captionTracks": [
                {
                    "baseUrl": "https://www.youtube.com/api/timedtext?v=x&lang=en",
                    "name": {"runs": [{"text": "English"}]},
                    "languageCode": "en"
                },
                {
                    "baseUrl": "https://www.youtube.com/api/timedtext?v=x&kind=asr&lang=de&fmt=srv3",
                    "name": {"simpleText": "German (auto-generated)"},
                    "languageCode": "de",
                    "kind": "asr"
                }
            ]}}
        }))
        .unwrap();

        let streams = parse_player_response(&body).unwrap();

        assert_eq!(
            streams.subtitles,
            [
                SubtitleTrack {
                    name: "English".into(),
                    lang: "en".into(),
                    url: "https://www.youtube.com/api/timedtext?v=x&lang=en&fmt=vtt".into(),
                    auto_generated: false,
                },
                SubtitleTrack {
                    name: "German (auto-generated)".into(),
                    lang: "de".into(),
                    url: "https://www.youtube.com/api/timedtext?v=x&kind=asr&lang=de&fmt=vtt"
                        .into(),
                    auto_generated: true,
                },
            ]
        );
        assert!(parse_player_response(FIXTURE).unwrap().subtitles.is_empty());
    }

    #[test]
    fn unplayable_videos_are_errors() {
        let body = br#"{"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":"Sign in to confirm you're not a bot"}}"#;
        let err = parse_player_response(body).unwrap_err();
        assert!(err.to_string().contains("Sign in"), "{err}");
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse_player_response(b"<html>").is_err());
    }
}
