//! Runs Flux offline against a fake backend with generated test videos.
//!
//! `cargo run --example demo`, then search for "demo".

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use chrono::Utc;
use flux::core::backend::fake::FakeBackend;
use flux::core::db::Db;
use flux::core::model::{Channel, SearchItem, Streams, Video, VideoDetails, VideoStream};
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
        let video = Video {
            id: format!("demo{i}"),
            title: format!("Test pattern “{pattern}” – a demo video with a rather long title"),
            channel_id: channel.id.clone(),
            channel_name: channel.name.clone(),
            published: Some(Utc::now() - chrono::Duration::days(i as i64 * 3 + 1)),
            duration_secs: Some(10),
            thumbnail_url: None,
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
                }],
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
        });
    }
    backend = backend.with_feed(&channel.id, videos.clone());
    backend = backend.with_channel(channel.clone(), videos.clone());
    let mut results: Vec<_> = videos.iter().cloned().map(SearchItem::Video).collect();
    results.insert(0, SearchItem::Channel(channel));
    backend = backend.with_search("demo", results);

    let app = relm4::RelmApp::new("io.github.patwid.Flux.Demo");
    app.run::<flux::ui::app::App>(Services {
        backend: Arc::new(backend),
        db: Rc::new(Db::open_in_memory().unwrap()),
    });
}
