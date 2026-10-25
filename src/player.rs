//! GStreamer playback engine.

use std::time::Duration;

use gst::prelude::*;
use gstreamer as gst;

use crate::core::download::RetryPolicy;
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
            PlaybackSource::Dash(manifest) => gst::ElementFactory::make("playbin3")
                .property("uri", dash_uri(manifest))
                .property("video-sink", &video_sink)
                .property("audio-sink", &audio_sink)
                .build()?,
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
        pipeline
            .downcast_ref::<gst::Bin>()
            .expect("pipelines are bins")
            .connect_deep_element_added(|_, _, element| configure_http(element));
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

/// A `data:` URI containing a DASH manifest.
pub fn dash_uri(manifest: &str) -> String {
    format!(
        "data:application/dash+xml;base64,{}",
        gst::glib::base64_encode(manifest.as_bytes())
    )
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

/// Makes `element` retry failed HTTP requests as downloads do (see
/// [`RetryPolicy`]), if it makes any. GStreamer's sources and adaptive
/// demuxers retry only 3 times by default, without waiting in between.
fn configure_http(element: &gst::Element) {
    let Some(retries) = ["max-retries", "retries"]
        .into_iter()
        .find(|p| element.has_property(p))
        .filter(|_| element.has_property("retry-backoff-factor"))
    else {
        return;
    };
    let policy = RetryPolicy::default();
    element.set_property(retries, policy.retries as i32);
    element.set_property("retry-backoff-factor", policy.first_delay.as_secs_f64());
    element.set_property("retry-backoff-max", policy.max_delay.as_secs_f64());
}

/// Whether `factory` makes decoders that decode in hardware, e.g.
/// `v4l2slh264dec` or `vah264dec`. (Its types are matched if any is.)
fn decodes_in_hardware(factory: &gst::ElementFactory) -> bool {
    factory.has_type(gst::ElementFactoryType::DECODER)
        && factory.has_type(gst::ElementFactoryType::HARDWARE)
}

/// Whether `element` decodes in hardware.
pub fn is_hardware_decoder(element: &gst::Element) -> bool {
    element.factory().is_some_and(|f| decodes_in_hardware(&f))
}

/// Makes engines created from now on decode in software. Hardware decoders
/// can fail for good mid-stream: `v4l2slh264dec` gives up on a frame the
/// hardware takes over a second to decode, and on every frame after it.
pub fn disable_hardware_decoders() {
    demote(
        gst::ElementFactory::factories_with_type(gst::ElementFactoryType::DECODER, gst::Rank::NONE)
            .into_iter()
            .filter(decodes_in_hardware),
    );
}

/// Keeps decoders from choosing the elements of `factories`.
fn demote(factories: impl IntoIterator<Item = gst::ElementFactory>) {
    for factory in factories {
        factory.set_rank(gst::Rank::NONE);
    }
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
    fn dash_manifest_is_passed_as_data_uri() {
        let uri = dash_uri("<MPD/>");
        assert_eq!(uri, "data:application/dash+xml;base64,PE1QRC8+");
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

    /// The retry properties of `element`.
    fn retries(element: &gst::Element) -> (i32, f64, f64) {
        let retries = ["max-retries", "retries"]
            .into_iter()
            .find(|p| element.has_property(p))
            .unwrap();
        (
            element.property(retries),
            element.property("retry-backoff-factor"),
            element.property("retry-backoff-max"),
        )
    }

    #[test]
    fn http_elements_retry_with_backoff() {
        gst::init().unwrap();
        for factory in ["souphttpsrc", "dashdemux2", "hlsdemux2"] {
            let element = gst::ElementFactory::make(factory).build().unwrap();
            configure_http(&element);
            assert_eq!(retries(&element), (10, 1.0, 30.0), "{factory}");
        }
        // Other elements are left alone.
        configure_http(&sink());
    }

    #[test]
    fn configures_the_http_elements_it_creates() {
        gst::init().unwrap();
        // Nothing listens on port 9; the source is created all the same.
        let source = PlaybackSource::Uri("http://127.0.0.1:9/video.webm".into());
        let engine = Engine::new(&source, sink(), sink()).unwrap();
        let _ = engine.pause();

        let bin = engine.pipeline.downcast_ref::<gst::Bin>().unwrap();
        let http = bin
            .iterate_recurse()
            .into_iter()
            .flatten()
            .find(|e| e.factory().is_some_and(|f| f.name() == "souphttpsrc"))
            .expect("an HTTP source");
        assert_eq!(retries(&http).0, 10);
    }

    #[test]
    fn software_decoders_are_not_hardware_decoders() {
        gst::init().unwrap();
        let decoder = gst::ElementFactory::make("vp8dec").build().unwrap();
        assert!(!is_hardware_decoder(&decoder));
    }

    #[test]
    fn engines_do_not_use_demoted_decoders() {
        gst::init().unwrap();
        // No other test decodes H.264, which this one demotes the decoder of.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.mkv");
        let pipeline = gst::parse::launch(&format!(
            "videotestsrc num-buffers=10 ! openh264enc ! h264parse ! matroskamux ! filesink location={}",
            path.display()
        ))
        .unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        pipeline
            .bus()
            .unwrap()
            .timed_pop_filtered(gst::ClockTime::from_seconds(30), &[gst::MessageType::Eos]);
        pipeline.set_state(gst::State::Null).unwrap();

        let decoders = |engine: &Engine| -> Vec<String> {
            let bin = engine.pipeline.downcast_ref::<gst::Bin>().unwrap();
            bin.iterate_recurse()
                .into_iter()
                .flatten()
                .filter_map(|e| e.factory())
                .filter(|f| f.has_type(gst::ElementFactoryType::DECODER))
                .map(|f| f.name().to_string())
                .collect()
        };
        let play = || {
            let engine = Engine::new(&PlaybackSource::Uri(uri(&path)), sink(), sink()).unwrap();
            engine.pause().unwrap();
            wait_for(&engine, &[gst::MessageType::AsyncDone]);
            engine
        };

        let preferred = decoders(&play()).pop().expect("a decoder");
        let factory = gst::ElementFactory::find(&preferred).unwrap();
        let rank = factory.rank();
        demote([factory.clone()]);
        let fallback = decoders(&play()).pop();
        factory.set_rank(rank);

        assert!(fallback.is_some_and(|f| f != preferred), "{preferred}");
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
