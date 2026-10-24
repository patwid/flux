//! GStreamer playback engine.

use std::time::Duration;

use gst::prelude::*;
use gstreamer as gst;

use crate::core::model::PlaybackSource;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Glib(#[from] gst::glib::Error),
    #[error(transparent)]
    Bool(#[from] gst::glib::BoolError),
    #[error(transparent)]
    StateChange(#[from] gst::StateChangeError),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Plays a [`PlaybackSource`] into the given sinks.
pub struct Engine {
    pipeline: gst::Element,
}

impl Engine {
    pub fn new(
        source: &PlaybackSource,
        video_sink: gst::Element,
        audio_sink: gst::Element,
    ) -> Result<Self> {
        let pipeline = match source {
            PlaybackSource::Uri(uri) => gst::ElementFactory::make("playbin3")
                .property("uri", uri)
                .property("video-sink", &video_sink)
                .property("audio-sink", &audio_sink)
                .build()?,
            PlaybackSource::Split { video, audio } => {
                let pipeline = gst::Pipeline::new();
                add_branch(&pipeline, video, video_sink, "videoconvert")?;
                add_branch(&pipeline, audio, audio_sink, "audioconvert ! audioresample")?;
                pipeline.upcast()
            }
        };
        Ok(Self { pipeline })
    }

    pub fn play(&self) -> Result<()> {
        self.pipeline.set_state(gst::State::Playing)?;
        Ok(())
    }

    pub fn pause(&self) -> Result<()> {
        self.pipeline.set_state(gst::State::Paused)?;
        Ok(())
    }

    /// Seeks to `position`, snapping to the nearest key frame.
    pub fn seek(&self, position: Duration) -> Result<()> {
        self.pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT | gst::SeekFlags::SNAP_NEAREST,
            gst::ClockTime::from_nseconds(position.as_nanos() as u64),
        )?;
        Ok(())
    }

    pub fn position(&self) -> Option<Duration> {
        self.pipeline
            .query_position::<gst::ClockTime>()
            .map(Into::into)
    }

    pub fn duration(&self) -> Option<Duration> {
        self.pipeline
            .query_duration::<gst::ClockTime>()
            .map(Into::into)
    }

    /// The pipeline bus, for watching end-of-stream and errors.
    pub fn bus(&self) -> gst::Bus {
        self.pipeline.bus().expect("pipelines have a bus")
    }
}

/// Adds `uridecodebin3 uri=<uri> ! <convert> ! <sink>` to `pipeline`. The
/// decoder's pad is linked once it appears.
fn add_branch(
    pipeline: &gst::Pipeline,
    uri: &str,
    sink: gst::Element,
    convert: &str,
) -> Result<()> {
    let decode = gst::ElementFactory::make("uridecodebin3")
        .property("uri", uri)
        .build()?;
    let convert = gst::parse::bin_from_description(convert, true)?;
    pipeline.add_many([&decode, convert.upcast_ref(), &sink])?;
    convert.link(&sink)?;
    decode.connect_pad_added(move |_, pad| {
        let target = convert.static_pad("sink").expect("ghost sink pad");
        if !target.is_linked() {
            let _ = pad.link(&target);
        }
    });
    Ok(())
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    /// Encodes the given encoded streams (gst-launch syntax) into a WebM file.
    fn encode(dir: &Path, name: &str, streams: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut description = format!("webmmux name=mux ! filesink location={}", path.display());
        for stream in streams {
            description += &format!(" {stream} ! mux.");
        }
        let pipeline = gst::parse::launch(&description).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        let msg = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(30),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .unwrap();
        assert!(matches!(msg.view(), gst::MessageView::Eos(_)), "{msg:?}");
        pipeline.set_state(gst::State::Null).unwrap();
        path
    }

    const VIDEO: &str = "videotestsrc num-buffers=30 ! video/x-raw,framerate=30/1,width=160,height=90 ! vp8enc deadline=1 keyframe-max-dist=5";
    const AUDIO: &str =
        "audiotestsrc num-buffers=50 samplesperbuffer=960 ! audio/x-raw,rate=48000 ! opusenc";

    fn uri(path: &Path) -> String {
        gst::glib::filename_to_uri(path, None).unwrap().into()
    }

    fn sink() -> gst::Element {
        gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .build()
            .unwrap()
    }

    fn wait_for(engine: &Engine, types: &[gst::MessageType]) -> gst::Message {
        engine
            .bus()
            .timed_pop_filtered(gst::ClockTime::from_seconds(30), types)
            .expect("timed out")
    }

    fn assert_plays_to_end(engine: &Engine) {
        engine.play().unwrap();
        let msg = wait_for(engine, &[gst::MessageType::Eos, gst::MessageType::Error]);
        assert!(matches!(msg.view(), gst::MessageView::Eos(_)), "{msg:?}");
    }

    #[test]
    fn plays_single_uri() {
        gst::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = encode(dir.path(), "av.webm", &[VIDEO, AUDIO]);

        let engine = Engine::new(&PlaybackSource::Uri(uri(&file)), sink(), sink()).unwrap();
        assert_plays_to_end(&engine);
    }

    #[test]
    fn plays_split_streams() {
        gst::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let video = encode(dir.path(), "v.webm", &[VIDEO]);
        let audio = encode(dir.path(), "a.webm", &[AUDIO]);

        let source = PlaybackSource::Split {
            video: uri(&video),
            audio: uri(&audio),
        };
        let engine = Engine::new(&source, sink(), sink()).unwrap();
        assert_plays_to_end(&engine);
    }

    #[test]
    fn reports_duration_and_seeks() {
        gst::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = encode(dir.path(), "v.webm", &[VIDEO]);
        let engine = Engine::new(&PlaybackSource::Uri(uri(&file)), sink(), sink()).unwrap();

        engine.pause().unwrap();
        wait_for(&engine, &[gst::MessageType::AsyncDone]);
        let duration = engine.duration().unwrap();
        assert!(duration >= Duration::from_millis(900), "{duration:?}");

        engine.seek(Duration::from_millis(500)).unwrap();
        wait_for(&engine, &[gst::MessageType::AsyncDone]);
        let position = engine.position().unwrap();
        assert!(position >= Duration::from_millis(400), "{position:?}");
    }

    #[test]
    fn reports_errors_on_bus() {
        gst::init().unwrap();
        let source = PlaybackSource::Uri("file:///nonexistent/video.webm".into());
        let engine = Engine::new(&source, sink(), sink()).unwrap();

        let _ = engine.play();
        let msg = wait_for(&engine, &[gst::MessageType::Error]);
        assert!(matches!(msg.view(), gst::MessageView::Error(_)));
    }
}
