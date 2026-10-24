//! Video player widget: renders an [`Engine`] and provides playback controls.

use std::time::Duration;

use gstreamer as gst;
use relm4::adw::prelude::*;
use relm4::gtk::gio;
use relm4::{Component, ComponentParts, ComponentSender, adw, gtk};

use super::aspect::AspectLayout;
use super::format;
use crate::core::model::PlaybackSource;
use crate::core::subtitles::{self, Cue};
use crate::player::Engine;

/// Ticks (of the 500 ms position timer) without activity before the
/// controls hide during playback.
const HIDE_AFTER_TICKS: u32 = 6;

/// How often the shown subtitle is updated.
const SUBTITLE_INTERVAL: Duration = Duration::from_millis(100);

/// Ticks a notice (e.g. a subtitle error) stays in the subtitle area.
const NOTICE_TICKS: u32 = 8;

/// Visibility of the playback controls: they hide while playing after a
/// few seconds without user activity and always show while paused.
#[derive(Debug, Default)]
pub struct Controls {
    hidden: bool,
    idle_ticks: u32,
}

impl Controls {
    pub fn visible(&self) -> bool {
        !self.hidden
    }

    /// The user moved the pointer or touched the video.
    pub fn activity(&mut self) {
        self.hidden = false;
        self.idle_ticks = 0;
    }

    /// The user tapped the video: toggles the controls.
    pub fn tap(&mut self) {
        self.hidden = !self.hidden;
        self.idle_ticks = 0;
    }

    pub fn tick(&mut self, playing: bool) {
        if !playing {
            self.activity();
            return;
        }
        self.idle_ticks += 1;
        if self.idle_ticks >= HIDE_AFTER_TICKS {
            self.hidden = true;
        }
    }
}

/// The `player.subtitles` action target of a subtitle track, or of none.
fn subtitle_target(track: Option<usize>) -> String {
    track.map(|index| index.to_string()).unwrap_or_default()
}

/// The subtitle track of a `player.subtitles` action target.
fn subtitle_track(target: &str) -> Option<usize> {
    target.parse().ok()
}

/// A menu item label showing `text` as is, without mnemonics.
fn menu_label(text: &str) -> String {
    text.replace('_', "__")
}

pub struct PlayerView {
    engine: Option<Engine>,
    _bus_watch: Option<gst::bus::BusWatchGuard>,
    picture: gtk::Picture,
    playing: bool,
    position: Duration,
    duration: Option<Duration>,
    /// Seek to perform once the pipeline has prerolled.
    pending_seek: Option<Duration>,
    status: Status,
    controls: Controls,
    fullscreen: bool,
    /// Cues of the shown subtitle track; empty when subtitles are off.
    cues: Vec<Cue>,
    /// The subtitle text at the current position.
    subtitle: Option<String>,
    subtitle_timer: Option<gtk::glib::SourceId>,
    /// A message shown in place of subtitles, with the ticks it remains.
    notice: Option<(String, u32)>,
    has_subtitle_tracks: bool,
    subtitle_menu: gio::Menu,
    subtitle_button: gtk::MenuButton,
    /// The `player.subtitles` action; its state is the chosen track's
    /// [`subtitle_target`].
    subtitle_action: gio::SimpleAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    /// Waiting for a source (streams are being fetched) or prerolling.
    Loading,
    Ready,
    Failed(String),
}

#[derive(Debug)]
pub enum Msg {
    Load {
        source: PlaybackSource,
        start_at: u32,
    },
    /// Shows an error instead of the video.
    Fail(String),
    TogglePlay,
    /// The user moved the seek bar to this position (seconds).
    Seek(f64),
    Tick,
    Prerolled,
    Ended,
    Error(String),
    ToggleFullscreen,
    /// Tells the player whether the page is fullscreen.
    SetFullscreen(bool),
    /// The video was tapped or clicked once.
    Tap,
    /// The pointer moved over the video.
    Activity,
    /// Offers subtitle tracks with these names, with the one shown.
    SetSubtitleTracks {
        names: Vec<String>,
        selected: Option<usize>,
    },
    /// Shows these cues; none turns subtitles off.
    SetSubtitles(Vec<Cue>),
    SubtitlesFailed(String),
    /// The user chose a subtitle track, or none.
    ChooseSubtitles(Option<usize>),
    SubtitleTick,
}

#[derive(Debug)]
pub enum Output {
    /// Current playback position in seconds.
    Progress(u32),
    ToggleFullscreen,
    /// The user chose the subtitle track with this index, or none.
    SelectSubtitles(Option<usize>),
}

#[relm4::component(pub)]
impl Component for PlayerView {
    type Init = ();
    type Input = Msg;
    type Output = Output;
    type CommandOutput = ();

    view! {
        // Sized 16:9 for the available width; videos are letterboxed.
        gtk::Box {
            set_layout_manager: Some(AspectLayout::default()),
            add_css_class: "player",

            gtk::Overlay {
            set_overflow: gtk::Overflow::Hidden,
            #[watch]
            set_cursor_from_name: if model.controls.visible() { None } else { Some("none") },

            add_controller = gtk::GestureClick {
                // Touch: a tap toggles the controls. Mouse: a click plays or
                // pauses (the controls follow the pointer). Double: fullscreen.
                connect_released[sender] => move |gesture, n_press, _, _| {
                    let touch = gesture
                        .current_event_device()
                        .is_some_and(|d| d.source() == gtk::gdk::InputSource::Touchscreen);
                    sender.input(match (n_press, touch) {
                        (2, _) => Msg::ToggleFullscreen,
                        (_, true) => Msg::Tap,
                        (_, false) => Msg::TogglePlay,
                    });
                },
            },
            add_controller = gtk::EventControllerMotion {
                connect_motion[sender] => move |_, _, _| sender.input(Msg::Activity),
            },

                #[local_ref]
                picture -> gtk::Picture {
                    set_content_fit: gtk::ContentFit::Contain,
                    set_can_shrink: true,
                    set_hexpand: true,
                },

                add_overlay = &gtk::Label {
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::End,
                    set_margin_start: 12,
                    set_margin_end: 12,
                    set_wrap: true,
                    set_justify: gtk::Justification::Center,
                    set_can_target: false,
                    #[watch]
                    set_css_classes: if model.fullscreen { &["player-subtitle", "large"] } else { &["player-subtitle"] },
                    // Above the controls while they show.
                    #[watch]
                    set_margin_bottom: if model.controls.visible() { 60 } else { 12 },
                    #[watch]
                    set_visible: model.subtitle_text().is_some(),
                    #[watch]
                    set_label: model.subtitle_text().unwrap_or_default(),
                },

                add_overlay = &adw::Spinner {
                    set_size_request: (48, 48),
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::Center,
                    #[watch]
                    set_visible: model.status == Status::Loading,
                },

                add_overlay = &adw::StatusPage {
                    set_icon_name: Some("dialog-error-symbolic"),
                    set_title: "Cannot Play Video",
                    add_css_class: "compact",
                    #[watch]
                    set_visible: matches!(model.status, Status::Failed(_)),
                    #[watch]
                    set_description: match &model.status {
                        Status::Failed(e) => Some(e),
                        _ => None,
                    },
                },

                add_overlay = &gtk::Revealer {
                    set_valign: gtk::Align::End,
                    set_transition_type: gtk::RevealerTransitionType::Crossfade,
                    #[watch]
                    set_reveal_child: match model.status {
                        Status::Ready => model.controls.visible(),
                        Status::Loading => true,
                        Status::Failed(_) => false,
                    },

                gtk::Box {
                    set_spacing: 6,
                    set_margin_start: 6,
                    set_margin_end: 6,
                    set_margin_bottom: 6,
                    add_css_class: "osd",
                    add_css_class: "toolbar",
                    #[watch]
                    set_sensitive: model.status == Status::Ready,

                    gtk::Button {
                        #[watch]
                        set_icon_name: if model.playing {
                            "media-playback-pause-symbolic"
                        } else {
                            "media-playback-start-symbolic"
                        },
                        #[watch]
                        set_tooltip_text: Some(if model.playing { "Pause" } else { "Play" }),
                        connect_clicked => Msg::TogglePlay,
                    },

                    gtk::Scale {
                        set_hexpand: true,
                        set_draw_value: false,
                        #[watch]
                        set_range: (0.0, model.duration.map_or(1.0, |d| d.as_secs_f64())),
                        #[watch]
                        set_value: model.position.as_secs_f64(),
                        connect_change_value[sender] => move |_, _, value| {
                            sender.input(Msg::Seek(value));
                            gtk::glib::Propagation::Proceed
                        },
                    },

                    gtk::Label {
                        add_css_class: "numeric",
                        #[watch]
                        set_label: &format!(
                            "{} / {}",
                            format::duration(model.position.as_secs() as u32),
                            model.duration.map_or("--:--".into(), |d| format::duration(d.as_secs() as u32)),
                        ),
                    },

                    #[local_ref]
                    subtitle_button -> gtk::MenuButton {
                        set_icon_name: "media-view-subtitles-symbolic",
                        set_tooltip_text: Some("Subtitles"),
                        set_direction: gtk::ArrowType::Up,
                        set_menu_model: Some(subtitle_menu),
                        #[watch]
                        set_visible: model.has_subtitle_tracks,
                    },

                    gtk::Button {
                        #[watch]
                        set_icon_name: if model.fullscreen {
                            "view-restore-symbolic"
                        } else {
                            "view-fullscreen-symbolic"
                        },
                        #[watch]
                        set_tooltip_text: Some(if model.fullscreen { "Leave Fullscreen" } else { "Fullscreen" }),
                        connect_clicked => Msg::ToggleFullscreen,
                    },
                },
                },
            },
        }
    }

    fn init(_: (), root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let model = PlayerView {
            engine: None,
            _bus_watch: None,
            picture: gtk::Picture::new(),
            playing: false,
            position: Duration::ZERO,
            duration: None,
            pending_seek: None,
            status: Status::Loading,
            controls: Controls::default(),
            fullscreen: false,
            cues: Vec::new(),
            subtitle: None,
            subtitle_timer: None,
            notice: None,
            has_subtitle_tracks: false,
            subtitle_menu: gio::Menu::new(),
            subtitle_button: gtk::MenuButton::new(),
            subtitle_action: gio::SimpleAction::new_stateful(
                "subtitles",
                Some(gtk::glib::VariantTy::STRING),
                &subtitle_target(None).to_variant(),
            ),
        };
        let picture = &model.picture;
        let subtitle_menu = &model.subtitle_menu;
        let subtitle_button = &model.subtitle_button;
        let widgets = view_output!();

        // Menu items request a state change, i.e. the user chose a track;
        // the player sets the state directly when tracks are offered.
        let input = sender.input_sender().clone();
        model
            .subtitle_action
            .connect_change_state(move |action, target| {
                if let Some(target) = target {
                    action.set_state(target);
                    let track = target.str().and_then(subtitle_track);
                    let _ = input.send(Msg::ChooseSubtitles(track));
                }
            });
        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&model.subtitle_action);
        root.insert_action_group("player", Some(&actions));

        let tick = sender.input_sender().clone();
        gtk::glib::timeout_add_local(Duration::from_millis(500), move || {
            if tick.send(Msg::Tick).is_ok() {
                gtk::glib::ControlFlow::Continue
            } else {
                gtk::glib::ControlFlow::Break
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::Load { source, start_at } => {
                self.status = Status::Loading;
                self.playing = false;
                self.position = Duration::from_secs(start_at.into());
                self.duration = None;
                self.pending_seek = (start_at > 0).then_some(self.position);
                if let Err(e) = self.load(&source, &sender) {
                    self.status = Status::Failed(e.to_string());
                }
            }
            // Keep the first error: later ones are usually consequences of it.
            Msg::Error(_) if matches!(self.status, Status::Failed(_)) => {}
            Msg::Fail(e) | Msg::Error(e) => {
                self.status = Status::Failed(e);
                self.playing = false;
                if let Some(engine) = &self.engine {
                    let _ = engine.pause();
                }
            }
            Msg::Prerolled => {
                // Async-done also follows user seeks; only startup is handled here.
                let Some(engine) = &self.engine else { return };
                if self.status != Status::Loading {
                    return;
                }
                self.duration = engine.duration();
                match self.pending_seek.take() {
                    // Ready once the seek to the resume position has prerolled too.
                    Some(position) => {
                        let _ = engine.seek(position);
                    }
                    // Videos never start by themselves; the user presses play.
                    None => self.status = Status::Ready,
                }
            }
            Msg::TogglePlay => self.set_playing(!self.playing),
            Msg::Seek(secs) => {
                if let Some(engine) = &self.engine {
                    self.position = Duration::from_secs_f64(secs.max(0.0));
                    let _ = engine.seek(self.position);
                }
            }
            Msg::Tap => self.controls.tap(),
            Msg::Activity => self.controls.activity(),
            Msg::SetFullscreen(fullscreen) => self.fullscreen = fullscreen,
            Msg::Tick => {
                self.controls.tick(self.playing);
                // Keep the controls, and with them the open menu, visible.
                if self.subtitle_button.is_active() {
                    self.controls.activity();
                }
                if let Some((_, ticks)) = &mut self.notice {
                    *ticks -= 1;
                    if *ticks == 0 {
                        self.notice = None;
                    }
                }
                if let Some(engine) = &self.engine
                    && self.status == Status::Ready
                {
                    if let Some(position) = engine.position() {
                        self.position = position;
                    }
                    if self.duration.is_none() {
                        self.duration = engine.duration();
                    }
                    if self.playing {
                        let _ = sender.output(Output::Progress(self.position.as_secs() as u32));
                    }
                }
            }
            Msg::Ended => {
                self.playing = false;
                if let Some(engine) = &self.engine {
                    let _ = engine.pause();
                    let _ = engine.seek(Duration::ZERO);
                }
            }
            Msg::ToggleFullscreen => {
                let _ = sender.output(Output::ToggleFullscreen);
            }
            Msg::SetSubtitleTracks { names, selected } => {
                self.fill_subtitle_menu(&names, selected);
            }
            Msg::SetSubtitles(cues) => self.set_cues(cues, &sender),
            Msg::SubtitlesFailed(e) => {
                self.notice = Some((format!("Cannot load subtitles: {e}"), NOTICE_TICKS));
                self.subtitle_action
                    .set_state(&subtitle_target(None).to_variant());
            }
            Msg::ChooseSubtitles(index) => {
                let _ = sender.output(Output::SelectSubtitles(index));
            }
            Msg::SubtitleTick => {
                if let Some(position) = self.engine.as_ref().and_then(Engine::position) {
                    self.subtitle = subtitles::text_at(&self.cues, position);
                }
            }
        }
    }
}

impl PlayerView {
    fn load(
        &mut self,
        source: &PlaybackSource,
        sender: &ComponentSender<Self>,
    ) -> crate::player::Result<()> {
        // Drop the old pipeline before building a new one.
        self.engine = None;
        self._bus_watch = None;

        let sink = gst::ElementFactory::make("gtk4paintablesink").build()?;
        let paintable = sink.property::<gtk::gdk::Paintable>("paintable");
        self.picture.set_paintable(Some(&paintable));
        // Use GL when GTK provides a context, as recommended by the sink.
        let video_sink = if paintable
            .property::<Option<gtk::gdk::GLContext>>("gl-context")
            .is_some()
        {
            gst::ElementFactory::make("glsinkbin")
                .property("sink", &sink)
                .build()?
        } else {
            sink
        };
        let audio_sink = gst::ElementFactory::make("autoaudiosink").build()?;

        let engine = Engine::new(source, video_sink, audio_sink)?;
        let input = sender.input_sender().clone();
        let watch = engine.bus().add_watch_local(move |_, msg| {
            let msg = match msg.view() {
                gst::MessageView::AsyncDone(_) => Some(Msg::Prerolled),
                gst::MessageView::Eos(_) => Some(Msg::Ended),
                gst::MessageView::Error(e) => Some(Msg::Error(e.error().to_string())),
                _ => None,
            };
            if let Some(msg) = msg {
                let _ = input.send(msg);
            }
            gtk::glib::ControlFlow::Continue
        })?;
        engine.pause()?;
        self.engine = Some(engine);
        self._bus_watch = Some(watch);
        Ok(())
    }

    fn set_playing(&mut self, playing: bool) {
        let Some(engine) = &self.engine else { return };
        let result = if playing {
            engine.play()
        } else {
            engine.pause()
        };
        if result.is_ok() {
            self.playing = playing;
        }
    }

    /// The text in the subtitle area.
    fn subtitle_text(&self) -> Option<&str> {
        match &self.notice {
            Some((notice, _)) => Some(notice),
            None => self.subtitle.as_deref(),
        }
    }

    /// Fills the subtitle menu with "Off" and the tracks, marking the one
    /// at `selected` (or "Off") as chosen.
    fn fill_subtitle_menu(&mut self, names: &[String], selected: Option<usize>) {
        self.subtitle_menu.remove_all();
        let entries = std::iter::once((None, "Off".to_owned()))
            .chain((0..).map(Some).zip(names.iter().map(|n| menu_label(n))));
        for (track, label) in entries {
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(
                Some("player.subtitles"),
                Some(&subtitle_target(track).to_variant()),
            );
            self.subtitle_menu.append_item(&item);
        }
        self.subtitle_action
            .set_state(&subtitle_target(selected).to_variant());
        self.has_subtitle_tracks = !names.is_empty();
    }

    /// Shows `cues`, updating the text while there are any.
    fn set_cues(&mut self, cues: Vec<Cue>, sender: &ComponentSender<Self>) {
        self.cues = cues;
        self.subtitle = None;
        if self.cues.is_empty() {
            if let Some(timer) = self.subtitle_timer.take() {
                timer.remove();
            }
        } else if self.subtitle_timer.is_none() {
            let input = sender.input_sender().clone();
            self.subtitle_timer =
                Some(gtk::glib::timeout_add_local(SUBTITLE_INTERVAL, move || {
                    if input.send(Msg::SubtitleTick).is_ok() {
                        gtk::glib::ControlFlow::Continue
                    } else {
                        gtk::glib::ControlFlow::Break
                    }
                }));
        }
    }

    /// Current position in whole seconds.
    pub fn position_secs(&self) -> u32 {
        self.position.as_secs() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_tracks_round_trip_through_action_targets() {
        for track in [None, Some(0), Some(12)] {
            assert_eq!(subtitle_track(&subtitle_target(track)), track);
        }
        assert_eq!(subtitle_track("nonsense"), None);
    }

    #[test]
    fn menu_labels_keep_underscores() {
        assert_eq!(menu_label("en_US"), "en__US");
    }

    fn ticks(controls: &mut Controls, n: u32, playing: bool) {
        for _ in 0..n {
            controls.tick(playing);
        }
    }

    #[test]
    fn controls_hide_while_playing() {
        let mut controls = Controls::default();
        ticks(&mut controls, HIDE_AFTER_TICKS - 1, true);
        assert!(controls.visible());
        controls.tick(true);
        assert!(!controls.visible());
    }

    #[test]
    fn controls_stay_while_paused() {
        let mut controls = Controls::default();
        ticks(&mut controls, HIDE_AFTER_TICKS * 2, false);
        assert!(controls.visible());
    }

    #[test]
    fn activity_shows_controls_and_restarts_timer() {
        let mut controls = Controls::default();
        ticks(&mut controls, HIDE_AFTER_TICKS, true);
        controls.activity();
        assert!(controls.visible());
        ticks(&mut controls, HIDE_AFTER_TICKS - 1, true);
        assert!(controls.visible());
    }

    #[test]
    fn tap_toggles_controls() {
        let mut controls = Controls::default();
        controls.tap();
        assert!(!controls.visible());
        controls.tap();
        assert!(controls.visible());
        ticks(&mut controls, HIDE_AFTER_TICKS - 1, true);
        assert!(controls.visible(), "showing restarts the timer");
    }
}
