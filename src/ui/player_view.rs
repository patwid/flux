//! Video player widget: renders an [`Engine`] and provides playback controls.

use std::time::Duration;

use gstreamer as gst;
use relm4::adw::prelude::*;
use relm4::gtk::gio;
use relm4::{Component, ComponentParts, ComponentSender, adw, gtk};

use super::aspect::AspectLayout;
use super::cue_layout::CueLayout;
use super::format;
use crate::core::model::PlaybackSource;
use crate::core::subtitles::{self, Cue, TextAlign};
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

/// Keeps the session from idling (blanking the screen) or suspending while
/// a video plays: the hardware decoder fails when the device suspends.
#[derive(Debug, Default)]
pub struct Inhibitor {
    /// The cookie of the inhibition held.
    cookie: Option<u32>,
}

impl Inhibitor {
    /// Inhibits while `playing`, and releases the inhibition otherwise.
    /// `inhibit` returns a cookie, or 0 if it failed (as GTK does);
    /// `uninhibit` releases a cookie.
    pub fn update(
        &mut self,
        playing: bool,
        inhibit: impl FnOnce() -> u32,
        uninhibit: impl FnOnce(u32),
    ) {
        match (playing, self.cookie) {
            (true, None) => self.cookie = Some(inhibit()).filter(|&cookie| cookie != 0),
            (false, Some(cookie)) => {
                uninhibit(cookie);
                self.cookie = None;
            }
            _ => {}
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

/// What the adapter reads from the pipeline and the widgets for an update.
#[derive(Debug, Default, Clone, Copy)]
pub struct Probe {
    /// The pipeline's position, if there is a pipeline and it knows it.
    pub position: Option<Duration>,
    pub duration: Option<Duration>,
    /// Whether the subtitle menu is open.
    pub menu_open: bool,
}

/// GTK-independent state of the player.
#[derive(Debug)]
pub struct State {
    /// Whether there is a pipeline.
    loaded: bool,
    playing: bool,
    position: Duration,
    duration: Option<Duration>,
    /// Seek to perform once the pipeline has prerolled.
    pending_seek: Option<Duration>,
    /// Whether to play once the pipeline is ready.
    play_when_ready: bool,
    status: Status,
    controls: Controls,
    fullscreen: bool,
    /// Cues of the shown subtitle track; empty when subtitles are off.
    cues: Vec<Cue>,
    /// The subtitle text at the current position.
    subtitle: Option<String>,
    /// The cues at the current position that have their own place.
    placed: Vec<Cue>,
    /// A message shown in place of subtitles, with the ticks it remains.
    notice: Option<(String, u32)>,
    has_subtitle_tracks: bool,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// Builds a paused pipeline for `source`, replacing the current one.
    /// If that fails, [`Msg::LoadFailed`] is to be sent.
    Build(PlaybackSource),
    /// Drops the pipeline.
    Drop,
    /// Plays or pauses; on success, [`Msg::Playing`] is to be sent.
    Play,
    Pause,
    Seek(Duration),
    /// Shows a label for each of these cues, at its place.
    ShowPlaced(Vec<Cue>),
    /// Offers subtitle tracks with these names in the menu, with the one shown.
    ShowSubtitleTracks {
        names: Vec<String>,
        selected: Option<usize>,
    },
    /// Marks the subtitle track with this index, or none, as chosen in the menu.
    MarkSubtitleTrack(Option<usize>),
    /// Updates the shown subtitle every [`SUBTITLE_INTERVAL`], or stops.
    SubtitleTimer(bool),
    Output(Output),
}

impl Default for State {
    fn default() -> Self {
        State {
            loaded: false,
            playing: false,
            position: Duration::ZERO,
            duration: None,
            pending_seek: None,
            play_when_ready: false,
            status: Status::Loading,
            controls: Controls::default(),
            fullscreen: false,
            cues: Vec::new(),
            subtitle: None,
            placed: Vec::new(),
            notice: None,
            has_subtitle_tracks: false,
        }
    }
}

impl State {
    pub fn update(&mut self, msg: Msg, probe: &Probe) -> Vec<Effect> {
        match msg {
            Msg::Load {
                source,
                start_at,
                play,
            } => {
                self.loaded = true;
                self.status = Status::Loading;
                self.playing = false;
                self.play_when_ready = play;
                self.position = Duration::from_secs(start_at.into());
                self.duration = None;
                self.pending_seek = (start_at > 0).then_some(self.position);
                return vec![Effect::Build(source)];
            }
            Msg::LoadFailed(e) => {
                self.loaded = false;
                self.status = Status::Failed(e);
            }
            // Keep the first error: later ones are usually consequences of it.
            Msg::Error(_) if !self.loaded || matches!(self.status, Status::Failed(_)) => {}
            Msg::Error(error) => {
                let position = match self.status {
                    Status::Ready => probe.position,
                    _ => None,
                }
                .unwrap_or(self.position);
                let playing = self.playing;
                // The page loads the video again or fails.
                self.loaded = false;
                self.status = Status::Loading;
                self.playing = false;
                return vec![
                    Effect::Drop,
                    Effect::Output(Output::Failed {
                        error,
                        position: position.as_secs() as u32,
                        playing,
                    }),
                ];
            }
            Msg::Fail(e) => {
                self.status = Status::Failed(e);
                self.playing = false;
                if self.loaded {
                    return vec![Effect::Pause];
                }
            }
            Msg::Prerolled => {
                // Async-done also follows user seeks; only startup is handled here.
                if !self.loaded || self.status != Status::Loading {
                    return Vec::new();
                }
                self.duration = probe.duration;
                match self.pending_seek.take() {
                    // Ready once the seek to the resume position has prerolled too.
                    Some(position) => return vec![Effect::Seek(position)],
                    // Videos never start by themselves: the user pressed play
                    // before, if at all.
                    None => {
                        self.status = Status::Ready;
                        if std::mem::take(&mut self.play_when_ready) {
                            return vec![Effect::Play];
                        }
                    }
                }
            }
            Msg::TogglePlay if !self.loaded => {}
            Msg::TogglePlay if self.playing => return vec![Effect::Pause],
            Msg::TogglePlay => return vec![Effect::Play],
            Msg::Playing(playing) => self.playing = playing,
            Msg::Seek(secs) => {
                if self.loaded {
                    self.position = Duration::from_secs_f64(secs.max(0.0));
                    return vec![Effect::Seek(self.position)];
                }
            }
            Msg::Tap => self.controls.tap(),
            Msg::Activity => self.controls.activity(),
            Msg::SetFullscreen(fullscreen) => self.fullscreen = fullscreen,
            Msg::Tick => {
                self.controls.tick(self.playing);
                // Keep the controls, and with them the open menu, visible.
                if probe.menu_open {
                    self.controls.activity();
                }
                if let Some((_, ticks)) = &mut self.notice {
                    *ticks -= 1;
                    if *ticks == 0 {
                        self.notice = None;
                    }
                }
                if self.loaded && self.status == Status::Ready {
                    if let Some(position) = probe.position {
                        self.position = position;
                    }
                    if self.duration.is_none() {
                        self.duration = probe.duration;
                    }
                    if self.playing {
                        return vec![Effect::Output(Output::Progress(self.position_secs()))];
                    }
                }
            }
            Msg::Ended => {
                self.playing = false;
                if self.loaded {
                    return vec![Effect::Pause, Effect::Seek(Duration::ZERO)];
                }
            }
            Msg::ToggleFullscreen => return vec![Effect::Output(Output::ToggleFullscreen)],
            Msg::SetSubtitleTracks { names, selected } => {
                self.has_subtitle_tracks = !names.is_empty();
                return vec![Effect::ShowSubtitleTracks { names, selected }];
            }
            Msg::SetSubtitles(cues) => {
                self.cues = cues;
                self.subtitle = None;
                self.placed = Vec::new();
                return vec![
                    Effect::ShowPlaced(Vec::new()),
                    Effect::SubtitleTimer(!self.cues.is_empty()),
                ];
            }
            Msg::SubtitlesFailed(e) => {
                self.notice = Some((format!("Cannot load subtitles: {e}"), NOTICE_TICKS));
                return vec![Effect::MarkSubtitleTrack(None)];
            }
            Msg::ChooseSubtitles(index) => {
                return vec![Effect::Output(Output::SelectSubtitles(index))];
            }
            Msg::SubtitleTick => {
                if let Some(position) = probe.position.filter(|_| self.loaded) {
                    self.subtitle = subtitles::text_at(&self.cues, position);
                    let placed = subtitles::placed_at(&self.cues, position);
                    if placed.iter().copied().ne(&self.placed) {
                        self.placed = placed.into_iter().cloned().collect();
                        return vec![Effect::ShowPlaced(self.placed.clone())];
                    }
                }
            }
        }
        Vec::new()
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Current position in whole seconds.
    pub fn position_secs(&self) -> u32 {
        self.position.as_secs() as u32
    }

    /// The text in the subtitle area.
    fn subtitle_text(&self) -> Option<&str> {
        match &self.notice {
            Some((notice, _)) => Some(notice),
            None => self.subtitle.as_deref(),
        }
    }

    /// The space below the subtitles: above the controls while they show.
    fn subtitle_margin(&self) -> i32 {
        if self.controls.visible() { 60 } else { 12 }
    }
}

pub struct PlayerView {
    state: State,
    engine: Option<Engine>,
    _bus_watch: Option<gst::bus::BusWatchGuard>,
    picture: gtk::Picture,
    inhibitor: Inhibitor,
    /// A label for each cue with its own place.
    cue_area: gtk::Box,
    cue_layout: CueLayout,
    subtitle_timer: Option<gtk::glib::SourceId>,
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
    /// Loads `source` at `start_at` seconds, and plays it if `play`.
    Load {
        source: PlaybackSource,
        start_at: u32,
        play: bool,
    },
    /// Shows an error instead of the video.
    Fail(String),
    /// The pipeline for the source could not be built.
    LoadFailed(String),
    TogglePlay,
    /// The pipeline started (true) or stopped playing.
    Playing(bool),
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

#[derive(Debug, PartialEq)]
pub enum Output {
    /// Current playback position in seconds.
    Progress(u32),
    /// Playback failed at `position` (seconds), while `playing` or not.
    /// The video is to be loaded again, or [`Msg::Fail`] sent.
    Failed {
        error: String,
        position: u32,
        playing: bool,
    },
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
            set_cursor_from_name: if model.state.controls.visible() { None } else { Some("none") },

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

                #[local_ref]
                add_overlay = cue_area -> gtk::Box {
                    set_layout_manager: Some(cue_layout.clone()),
                    set_can_target: false,
                    #[watch]
                    set_css_classes: if model.state.fullscreen { &["cues", "large"] } else { &["cues"] },
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
                    set_css_classes: if model.state.fullscreen { &["player-subtitle", "large"] } else { &["player-subtitle"] },
                    // Above the controls while they show.
                    #[watch]
                    set_margin_bottom: model.state.subtitle_margin(),
                    #[watch]
                    set_visible: model.state.subtitle_text().is_some(),
                    #[watch]
                    set_label: model.state.subtitle_text().unwrap_or_default(),
                },

                add_overlay = &adw::Spinner {
                    set_size_request: (48, 48),
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::Center,
                    #[watch]
                    set_visible: model.state.status == Status::Loading,
                },

                add_overlay = &adw::StatusPage {
                    set_icon_name: Some("dialog-error-symbolic"),
                    set_title: "Cannot Play Video",
                    add_css_class: "compact",
                    #[watch]
                    set_visible: matches!(model.state.status, Status::Failed(_)),
                    #[watch]
                    set_description: match &model.state.status {
                        Status::Failed(e) => Some(e),
                        _ => None,
                    },
                },

                add_overlay = &gtk::Revealer {
                    set_valign: gtk::Align::End,
                    set_transition_type: gtk::RevealerTransitionType::Crossfade,
                    #[watch]
                    set_reveal_child: match model.state.status {
                        Status::Ready => model.state.controls.visible(),
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
                    set_sensitive: model.state.status == Status::Ready,

                    gtk::Button {
                        #[watch]
                        set_icon_name: if model.state.playing {
                            "media-playback-pause-symbolic"
                        } else {
                            "media-playback-start-symbolic"
                        },
                        #[watch]
                        set_tooltip_text: Some(if model.state.playing { "Pause" } else { "Play" }),
                        connect_clicked => Msg::TogglePlay,
                    },

                    gtk::Scale {
                        set_hexpand: true,
                        set_draw_value: false,
                        #[watch]
                        set_range: (0.0, model.state.duration.map_or(1.0, |d| d.as_secs_f64())),
                        #[watch]
                        set_value: model.state.position.as_secs_f64(),
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
                            format::duration(model.state.position.as_secs() as u32),
                            model.state.duration.map_or("--:--".into(), |d| format::duration(d.as_secs() as u32)),
                        ),
                    },

                    #[local_ref]
                    subtitle_button -> gtk::MenuButton {
                        set_icon_name: "media-view-subtitles-symbolic",
                        set_tooltip_text: Some("Subtitles"),
                        set_direction: gtk::ArrowType::Up,
                        set_menu_model: Some(subtitle_menu),
                        #[watch]
                        set_visible: model.state.has_subtitle_tracks,
                    },

                    gtk::Button {
                        #[watch]
                        set_icon_name: if model.state.fullscreen {
                            "view-restore-symbolic"
                        } else {
                            "view-fullscreen-symbolic"
                        },
                        #[watch]
                        set_tooltip_text: Some(if model.state.fullscreen { "Leave Fullscreen" } else { "Fullscreen" }),
                        connect_clicked => Msg::ToggleFullscreen,
                    },
                },
                },
            },
        }
    }

    fn init(_: (), root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let model = PlayerView {
            state: State::default(),
            engine: None,
            _bus_watch: None,
            picture: gtk::Picture::new(),
            inhibitor: Inhibitor::default(),
            cue_area: gtk::Box::default(),
            cue_layout: CueLayout::default(),
            subtitle_timer: None,
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
        let cue_area = &model.cue_area;
        let cue_layout = &model.cue_layout;
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

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, root: &Self::Root) {
        // Effects may answer with messages, which are handled right away.
        let mut queue = std::collections::VecDeque::from([msg]);
        while let Some(msg) = queue.pop_front() {
            let probe = self.probe();
            for effect in self.state.update(msg, &probe) {
                queue.extend(self.run(effect, &sender));
            }
        }
        self.cue_layout
            .set_bottom_inset(self.state.subtitle_margin());
        let window = root.root().and_downcast::<gtk::Window>();
        self.inhibitor.update(
            self.state.playing,
            || {
                relm4::main_application().inhibit(
                    window.as_ref(),
                    gtk::ApplicationInhibitFlags::IDLE | gtk::ApplicationInhibitFlags::SUSPEND,
                    Some("Playing a video"),
                )
            },
            |cookie| relm4::main_application().uninhibit(cookie),
        );
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Output>) {
        self.inhibitor.update(
            false,
            || 0,
            |cookie| relm4::main_application().uninhibit(cookie),
        );
    }
}

impl PlayerView {
    /// Reads what the state needs from the pipeline and the widgets.
    fn probe(&self) -> Probe {
        Probe {
            position: self.engine.as_ref().and_then(Engine::position),
            duration: self.engine.as_ref().and_then(Engine::duration),
            menu_open: self.subtitle_button.is_active(),
        }
    }

    /// Runs `effect`, returning the message it answers with, if any.
    fn run(&mut self, effect: Effect, sender: &ComponentSender<Self>) -> Option<Msg> {
        match effect {
            Effect::Build(source) => {
                if let Err(e) = self.build(&source, sender) {
                    return Some(Msg::LoadFailed(e.to_string()));
                }
            }
            Effect::Drop => {
                self.engine = None;
                self._bus_watch = None;
            }
            Effect::Play | Effect::Pause => {
                let play = effect == Effect::Play;
                let engine = self.engine.as_ref()?;
                let result = if play { engine.play() } else { engine.pause() };
                return result.is_ok().then_some(Msg::Playing(play));
            }
            Effect::Seek(position) => {
                if let Some(engine) = &self.engine {
                    let _ = engine.seek(position);
                }
            }
            Effect::ShowPlaced(cues) => self.show_placed(&cues),
            Effect::ShowSubtitleTracks { names, selected } => {
                self.fill_subtitle_menu(&names, selected);
            }
            Effect::MarkSubtitleTrack(track) => {
                self.subtitle_action
                    .set_state(&subtitle_target(track).to_variant());
            }
            Effect::SubtitleTimer(on) => self.set_subtitle_timer(on, sender),
            Effect::Output(output) => {
                let _ = sender.output(output);
            }
        }
        None
    }

    /// Builds a paused pipeline for `source`, replacing the current one.
    fn build(
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
        self.cue_layout.set_paintable(Some(&paintable));
        let cue_area = self.cue_area.downgrade();
        paintable.connect_invalidate_size(move |_| {
            if let Some(cue_area) = cue_area.upgrade() {
                cue_area.queue_allocate();
            }
        });
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
                gst::MessageView::Error(e) => {
                    eprintln!(
                        "flux: playback error from {}: {} ({})",
                        msg.src()
                            .map(gst::prelude::GstObjectExt::path_string)
                            .unwrap_or_default(),
                        e.error(),
                        e.debug().unwrap_or_default()
                    );
                    Some(Msg::Error(e.error().to_string()))
                }
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

    /// Shows a label for each of the `placed` cues.
    fn show_placed(&self, placed: &[Cue]) {
        while let Some(label) = self.cue_area.first_child() {
            self.cue_area.remove(&label);
        }
        for cue in placed {
            let label = gtk::Label::builder()
                .label(&cue.text)
                .wrap(true)
                .justify(match cue.placement.cue_box().align {
                    TextAlign::Left => gtk::Justification::Left,
                    TextAlign::Center => gtk::Justification::Center,
                    TextAlign::Right => gtk::Justification::Right,
                })
                .css_classes(["player-subtitle"])
                .build();
            self.cue_area.append(&label);
        }
        self.cue_layout
            .set_boxes(placed.iter().map(|c| c.placement.cue_box()).collect());
    }

    /// Fills the subtitle menu with "Off" and the tracks, marking the one
    /// at `selected` (or "Off") as chosen.
    fn fill_subtitle_menu(&self, names: &[String], selected: Option<usize>) {
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
    }

    /// Sends [`Msg::SubtitleTick`] every [`SUBTITLE_INTERVAL`] while `on`.
    fn set_subtitle_timer(&mut self, on: bool, sender: &ComponentSender<Self>) {
        if !on {
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
        self.state.position_secs()
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

    fn source() -> PlaybackSource {
        PlaybackSource::Uri("https://v".into())
    }

    fn at(secs: u64) -> Probe {
        Probe {
            position: Some(Duration::from_secs(secs)),
            duration: Some(Duration::from_secs(600)),
            menu_open: false,
        }
    }

    fn load(state: &mut State, start_at: u32, play: bool) -> Vec<Effect> {
        state.update(
            Msg::Load {
                source: source(),
                start_at,
                play,
            },
            &Probe::default(),
        )
    }

    /// A player with a video loaded at the start and ready, playing or not.
    fn ready(playing: bool) -> State {
        let mut state = State::default();
        load(&mut state, 0, false);
        state.update(Msg::Prerolled, &at(0));
        if playing {
            state.update(Msg::Playing(true), &at(0));
        }
        state
    }

    #[test]
    fn videos_load_paused_at_the_resume_position() {
        let mut state = State::default();
        assert_eq!(load(&mut state, 30, false), [Effect::Build(source())]);
        assert_eq!(state.status, Status::Loading);
        assert_eq!(state.position_secs(), 30);

        assert_eq!(
            state.update(Msg::Prerolled, &at(0)),
            [Effect::Seek(Duration::from_secs(30))]
        );
        assert_eq!(state.status, Status::Loading, "until the seek prerolled");
        assert_eq!(state.duration, Some(Duration::from_secs(600)));

        assert!(state.update(Msg::Prerolled, &at(30)).is_empty());
        assert_eq!(state.status, Status::Ready);
        assert!(!state.playing());
    }

    #[test]
    fn videos_loaded_to_play_play_once_ready() {
        let mut state = State::default();
        load(&mut state, 0, true);
        assert_eq!(state.update(Msg::Prerolled, &at(0)), [Effect::Play]);
        assert!(!state.playing(), "until the pipeline plays");
        state.update(Msg::Playing(true), &at(0));
        assert!(state.playing());
    }

    #[test]
    fn prerolls_after_user_seeks_are_ignored() {
        let mut state = ready(false);
        assert_eq!(
            state.update(Msg::Seek(42.0), &at(0)),
            [Effect::Seek(Duration::from_secs(42))]
        );
        assert!(state.update(Msg::Prerolled, &at(42)).is_empty());
    }

    #[test]
    fn toggling_play_plays_and_pauses() {
        let mut state = State::default();
        assert!(state.update(Msg::TogglePlay, &at(0)).is_empty(), "no video");

        let mut state = ready(false);
        assert_eq!(state.update(Msg::TogglePlay, &at(0)), [Effect::Play]);
        state.update(Msg::Playing(true), &at(0));
        assert_eq!(state.update(Msg::TogglePlay, &at(0)), [Effect::Pause]);
    }

    #[test]
    fn the_first_error_is_reported_at_the_position() {
        let mut state = ready(true);
        assert_eq!(
            state.update(Msg::Error("decoder".into()), &at(42)),
            [
                Effect::Drop,
                Effect::Output(Output::Failed {
                    error: "decoder".into(),
                    position: 42,
                    playing: true,
                }),
            ]
        );
        assert_eq!(state.status, Status::Loading);
        assert!(!state.playing());
        assert!(state.update(Msg::Error("later".into()), &at(42)).is_empty());
    }

    #[test]
    fn errors_while_loading_report_the_resume_position() {
        let mut state = State::default();
        load(&mut state, 30, false);
        assert_eq!(
            state.update(Msg::Error("refused".into()), &at(0))[1],
            Effect::Output(Output::Failed {
                error: "refused".into(),
                position: 30,
                playing: false,
            })
        );
    }

    #[test]
    fn failing_shows_the_error_and_ignores_later_ones() {
        let mut state = ready(true);
        assert_eq!(
            state.update(Msg::Fail("gone".into()), &at(0)),
            [Effect::Pause]
        );
        assert_eq!(state.status, Status::Failed("gone".into()));
        assert!(!state.playing());
        assert!(state.update(Msg::Error("x".into()), &at(0)).is_empty());
    }

    #[test]
    fn unbuildable_pipelines_fail() {
        let mut state = State::default();
        load(&mut state, 0, false);
        assert!(
            state
                .update(Msg::LoadFailed("no sink".into()), &at(0))
                .is_empty()
        );
        assert_eq!(state.status, Status::Failed("no sink".into()));
        assert!(state.update(Msg::Error("x".into()), &at(0)).is_empty());
        assert!(state.update(Msg::TogglePlay, &at(0)).is_empty());
    }

    #[test]
    fn ended_videos_pause_at_the_start() {
        let mut state = ready(true);
        assert_eq!(
            state.update(Msg::Ended, &at(600)),
            [Effect::Pause, Effect::Seek(Duration::ZERO)]
        );
        assert!(!state.playing());
    }

    #[test]
    fn ticks_report_progress_while_playing() {
        let mut state = ready(false);
        assert!(state.update(Msg::Tick, &at(5)).is_empty());
        assert_eq!(state.position_secs(), 5);

        state.update(Msg::Playing(true), &at(5));
        assert_eq!(
            state.update(Msg::Tick, &at(6)),
            [Effect::Output(Output::Progress(6))]
        );
    }

    #[test]
    fn an_open_menu_keeps_the_controls() {
        let mut state = ready(true);
        let open = Probe {
            menu_open: true,
            ..at(0)
        };
        for _ in 0..HIDE_AFTER_TICKS {
            state.update(Msg::Tick, &open);
        }
        assert!(state.controls.visible());
        for _ in 0..HIDE_AFTER_TICKS {
            state.update(Msg::Tick, &at(0));
        }
        assert!(!state.controls.visible());
    }

    #[test]
    fn subtitle_errors_show_for_a_while() {
        let mut state = ready(false);
        assert_eq!(
            state.update(Msg::SubtitlesFailed("offline".into()), &at(0)),
            [Effect::MarkSubtitleTrack(None)]
        );
        assert_eq!(
            state.subtitle_text(),
            Some("Cannot load subtitles: offline")
        );
        for _ in 0..NOTICE_TICKS {
            state.update(Msg::Tick, &at(0));
        }
        assert_eq!(state.subtitle_text(), None);
    }

    fn cue(start: u64, end: u64, text: &str) -> Cue {
        Cue {
            start: Duration::from_secs(start),
            end: Duration::from_secs(end),
            text: text.into(),
            placement: Default::default(),
        }
    }

    #[test]
    fn subtitles_follow_the_position() {
        let mut state = ready(true);
        let placed = Cue {
            placement: subtitles::Placement {
                line: subtitles::Line::Number(0),
                ..Default::default()
            },
            ..cue(2, 4, "placed")
        };
        assert_eq!(
            state.update(
                Msg::SetSubtitles(vec![cue(0, 2, "hello"), placed.clone()]),
                &at(0)
            ),
            [Effect::ShowPlaced(vec![]), Effect::SubtitleTimer(true)]
        );
        assert!(state.update(Msg::SubtitleTick, &at(1)).is_empty());
        assert_eq!(state.subtitle_text(), Some("hello"));

        assert_eq!(
            state.update(Msg::SubtitleTick, &at(3)),
            [Effect::ShowPlaced(vec![placed])]
        );
        assert_eq!(state.subtitle_text(), None);
        assert!(
            state.update(Msg::SubtitleTick, &at(3)).is_empty(),
            "unchanged"
        );

        assert_eq!(
            state.update(Msg::SetSubtitles(vec![]), &at(3)),
            [Effect::ShowPlaced(vec![]), Effect::SubtitleTimer(false)]
        );
    }

    #[test]
    fn subtitle_tracks_are_offered_and_chosen() {
        let mut state = ready(false);
        let names = vec!["English".to_owned()];
        assert_eq!(
            state.update(
                Msg::SetSubtitleTracks {
                    names: names.clone(),
                    selected: Some(0)
                },
                &at(0)
            ),
            [Effect::ShowSubtitleTracks {
                names,
                selected: Some(0)
            }]
        );
        assert!(state.has_subtitle_tracks);
        assert_eq!(
            state.update(Msg::ChooseSubtitles(None), &at(0)),
            [Effect::Output(Output::SelectSubtitles(None))]
        );
    }

    fn ticks(controls: &mut Controls, n: u32, playing: bool) {
        for _ in 0..n {
            controls.tick(playing);
        }
    }

    /// Runs [`Inhibitor::update`], returning the calls made.
    fn inhibit(inhibitor: &mut Inhibitor, playing: bool, cookie: u32) -> Vec<String> {
        let calls = std::cell::RefCell::new(Vec::new());
        inhibitor.update(
            playing,
            || {
                calls.borrow_mut().push("inhibit".to_string());
                cookie
            },
            |c| calls.borrow_mut().push(format!("uninhibit {c}")),
        );
        calls.into_inner()
    }

    #[test]
    fn inhibits_idle_once_while_playing() {
        let mut inhibitor = Inhibitor::default();
        assert!(inhibit(&mut inhibitor, false, 7).is_empty());
        assert_eq!(inhibit(&mut inhibitor, true, 7), ["inhibit"]);
        assert!(inhibit(&mut inhibitor, true, 8).is_empty());
        assert_eq!(inhibit(&mut inhibitor, false, 8), ["uninhibit 7"]);
        assert!(inhibit(&mut inhibitor, false, 8).is_empty());
    }

    #[test]
    fn inhibiting_again_after_a_failure() {
        let mut inhibitor = Inhibitor::default();
        assert_eq!(inhibit(&mut inhibitor, true, 0), ["inhibit"]);
        assert_eq!(inhibit(&mut inhibitor, true, 3), ["inhibit"]);
        assert_eq!(inhibit(&mut inhibitor, false, 0), ["uninhibit 3"]);
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
