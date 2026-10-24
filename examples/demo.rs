//! Runs Flux offline against a fake backend with generated test videos.
//!
//! `cargo run --example demo`, then search for "demo".

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use chrono::Utc;
use flux::core::backend::fake::FakeBackend;
use flux::core::db::Db;
use flux::core::model::{
    Channel, Comment, CommentPage, SearchItem, Streams, SubtitleTrack, Video, VideoDetails,
    VideoStream,
};
use flux::core::subtitles::Cue;
use flux::ui::Services;
use gstreamer as gst;
use gstreamer::prelude::*;

/// Encodes a 10 s test video with audio, unless it already exists.
fn test_video(path: &Path, pattern: &str) {
    if path.exists() {
        return;
    }
    let description = format!(
        "webmmux name=mux ! filesink location={} \
         videotestsrc pattern={pattern} num-buffers=300 ! video/x-raw,framerate=30/1,width=640,height=360 \
           ! vp8enc deadline=1 keyframe-max-dist=30 ! mux. \
         audiotestsrc wave=ticks num-buffers=500 samplesperbuffer=960 ! audio/x-raw,rate=48000 \
           ! opusenc ! mux.",
        path.display()
    );
    let pipeline = gst::parse::launch(&description).expect("valid pipeline");
    pipeline.set_state(gst::State::Playing).unwrap();
    pipeline.bus().unwrap().timed_pop_filtered(
        gst::ClockTime::NONE,
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    pipeline.set_state(gst::State::Null).unwrap();
}

/// Renders a PNG thumbnail of a test pattern, unless it already exists.
fn thumbnail(path: &Path, pattern: &str) {
    if path.exists() {
        return;
    }
    let description = format!(
        "videotestsrc pattern={pattern} num-buffers=1 ! video/x-raw,width=320,height=180 \
         ! pngenc ! filesink location={}",
        path.display()
    );
    let pipeline = gst::parse::launch(&description).expect("valid pipeline");
    pipeline.set_state(gst::State::Playing).unwrap();
    pipeline.bus().unwrap().timed_pop_filtered(
        gst::ClockTime::NONE,
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    pipeline.set_state(gst::State::Null).unwrap();
}

/// Subtitle tracks for every demo video; the last one fails to load.
fn subtitle_tracks() -> Vec<SubtitleTrack> {
    [
        ("English", "en"),
        ("German (auto-generated)", "de"),
        ("Unavailable", "xx"),
    ]
    .into_iter()
    .map(|(name, lang)| SubtitleTrack {
        name: name.into(),
        lang: lang.into(),
        url: format!("demo-subtitles-{lang}"),
        auto_generated: name.contains("auto-generated"),
    })
    .collect()
}

/// One cue per two seconds, counting up.
fn subtitle_cues(prefix: &str) -> Vec<Cue> {
    (0..5)
        .map(|i| Cue {
            start: std::time::Duration::from_secs(i * 2),
            end: std::time::Duration::from_secs(i * 2 + 2),
            text: if i == 2 {
                format!("{prefix} {i}, a longer subtitle\nwith a second line")
            } else {
                format!("{prefix} {i}")
            },
        })
        .collect()
}

/// Generated comments by page token: two pages of comments, the first
/// one by the channel and pinned, and pages of replies.
fn comment_pages(channel: &Channel) -> Vec<(String, CommentPage)> {
    let author = |i: u32| {
        Some(if i == 0 {
            channel.clone()
        } else {
            Channel {
                id: format!("UCviewer{i}"),
                name: format!("@viewer{i}"),
                avatar_url: None,
            }
        })
    };
    let replies = |i: u32| if i == 0 { 12 } else { i % 4 };
    let comment = |i: u32| Comment {
        author: author(i),
        text: if i % 3 == 1 {
            "A longer comment that wraps over several lines on a phone, to check that \
             the text is laid out properly next to the avatar."
                .into()
        } else {
            format!("Comment number {i}.")
        },
        published: Some(Utc::now() - chrono::Duration::hours(i64::from(i * 7))),
        like_count: Some(1_500 / (i + 1)),
        reply_count: replies(i),
        replies: (replies(i) > 0).then(|| format!("demo-replies-{i}-0")),
        pinned: i == 0,
        by_owner: i == 0,
    };
    let reply = |i: u32, n: u32| Comment {
        author: author(if n.is_multiple_of(2) { 20 + n } else { 0 }),
        text: format!("Reply number {n} to comment {i}."),
        published: Some(Utc::now() - chrono::Duration::minutes(i64::from(n * 13))),
        like_count: Some(n),
        reply_count: 0,
        replies: None,
        pinned: false,
        by_owner: !n.is_multiple_of(2),
    };
    let mut pages = vec![
        (
            "demo-comments-1".to_string(),
            CommentPage {
                comments: (0..10).map(comment).collect(),
                count: Some(15),
                next: Some("demo-comments-2".into()),
            },
        ),
        (
            "demo-comments-2".to_string(),
            CommentPage {
                comments: (10..15).map(comment).collect(),
                count: Some(15),
                next: None,
            },
        ),
    ];
    // Replies in pages of ten.
    for i in 0..15 {
        let total = replies(i);
        for (page, start) in (0..total).step_by(10).enumerate() {
            let end = (start + 10).min(total);
            pages.push((
                format!("demo-replies-{i}-{page}"),
                CommentPage {
                    comments: (start..end).map(|n| reply(i, n)).collect(),
                    count: Some(total.into()),
                    next: (end < total).then(|| format!("demo-replies-{i}-{}", page + 1)),
                },
            ));
        }
    }
    pages
}

fn main() {
    gst::init().unwrap();
    let dir = std::env::temp_dir().join("flux-demo");
    std::fs::create_dir_all(&dir).unwrap();

    let channel = Channel {
        id: "UCdemo".into(),
        name: "Demo Channel".into(),
        avatar_url: None,
    };
    let mut backend = FakeBackend::default();
    let mut videos = Vec::new();
    for (i, pattern) in ["ball", "smpte", "pinwheel", "snow"]
        .into_iter()
        .enumerate()
    {
        let path = dir.join(format!("{pattern}.webm"));
        test_video(&path, pattern);
        let thumb = dir.join(format!("{pattern}.png"));
        thumbnail(&thumb, pattern);
        let video = Video {
            id: format!("demo{i}"),
            title: format!("Test pattern “{pattern}” – a demo video with a rather long title"),
            channel_id: channel.id.clone(),
            channel_name: channel.name.clone(),
            published: Some(Utc::now() - chrono::Duration::days(i as i64 * 3 + 1)),
            duration_secs: Some(10),
            thumbnail_url: Some(url::Url::from_file_path(&thumb).unwrap().into()),
        };
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        backend = backend.with_streams(
            &video.id,
            Streams {
                muxed: vec![VideoStream {
                    url: uri,
                    width: 640,
                    height: 360,
                    fps: 30,
                    bitrate: 1,
                    mime: "video/webm; codecs=\"vp8\"".into(),
                    ranges: None,
                    size: None,
                }],
                subtitles: subtitle_tracks(),
                ..Default::default()
            },
        );
        videos.push(video);
    }
    for video in &videos {
        backend = backend.with_details(VideoDetails {
            video: video.clone(),
            channel: channel.clone(),
            description: "A generated test video.\nPlayed from a local file.".into(),
            view_count: Some(123_456),
            like_count: Some(42),
            related: videos
                .iter()
                .filter(|v| v.id != video.id)
                .cloned()
                .collect(),
            comments: CommentPage {
                count: Some(15),
                next: Some("demo-comments-1".into()),
                ..Default::default()
            },
        });
    }
    backend = backend
        .with_subtitles("demo-subtitles-en", subtitle_cues("Subtitle"))
        .with_subtitles("demo-subtitles-de", subtitle_cues("Untertitel"));
    for (token, page) in comment_pages(&channel) {
        backend = backend.with_comments(&token, page);
    }
    backend = backend.with_feed(&channel.id, videos.clone());
    backend = backend.with_channel(channel.clone(), videos.clone());
    let mut results: Vec<_> = videos.iter().cloned().map(SearchItem::Video).collect();
    results.insert(0, SearchItem::Channel(channel));
    backend = backend.with_search("demo", results);

    let app = relm4::RelmApp::new("io.github.patwid.Flux.Demo");
    let backend = Arc::new(backend);
    app.run::<flux::ui::app::App>(Services {
        backend: backend.clone(),
        downloader: backend,
        db: Rc::new(Db::open_in_memory().unwrap()),
        download_dir: dir.join("downloads"),
    });
}
