//! Video page: player, details, related videos.

use std::path::PathBuf;

use relm4::adw::prelude::*;
use relm4::{
    Component, ComponentController, ComponentParts, ComponentSender, Controller, RelmWidgetExt,
    adw, gtk,
};

use super::player_view::{self, PlayerView};
use super::{Services, format, images, log_db_error, widgets};
use crate::core::model::{
    Channel, Comment, CommentPage, DownloadKind, PlaybackSource, Streams, SubtitleTrack, Video,
    VideoDetails,
};
use crate::core::streams;
use crate::core::subtitles::{self, Cue};

use crate::core::streams::DEFAULT_MAX_HEIGHT as MAX_HEIGHT;

/// Pages at least this wide scroll the player with the details.
const WIDE_WIDTH_SP: f64 = 700.0;

/// Seconds of playback between two history updates.
const SAVE_INTERVAL_SECS: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load<T> {
    Loading,
    Loaded(T),
    Failed(String),
}

/// Comments of a video, loaded page by page on demand.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Comments {
    pub items: Vec<Comment>,
    /// Total number of comments, if known.
    pub count: Option<u64>,
    /// Token of the next page; `None` when all are loaded or comments are off.
    pub next: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
}

/// The subtitle tracks of a video and the one shown.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Subtitles {
    pub tracks: Vec<SubtitleTrack>,
    /// Index of the chosen track; `None` when subtitles are off.
    pub selected: Option<usize>,
    /// The language to show when tracks arrive, if any.
    pub preferred: Option<String>,
}

/// GTK-independent state of a video page.
#[derive(Debug)]
pub struct State {
    pub video: Video,
    pub details: Load<VideoDetails>,
    pub playback: Load<PlaybackSource>,
    pub subscribed: bool,
    pub comments: Comments,
    pub subtitles: Subtitles,
    /// Position to start playback at, in seconds.
    resume_at: u32,
    /// Last position written to the history.
    saved_position: Option<u32>,
}

/// Everything known about a video when its page is opened.
#[derive(Debug, Clone)]
pub struct Init {
    pub video: Video,
    /// Saved position from the watch history.
    pub saved_position: Option<u32>,
    /// A completed download to play instead of streaming.
    pub local_file: Option<PathBuf>,
    pub subscribed: bool,
    /// The language of the subtitles to show, if the user wants any.
    pub subtitle_language: Option<String>,
}

// See `PageMsg`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Msg {
    Details(Result<VideoDetails, String>),
    Streams(Result<Streams, String>),
    /// Playback reached this position (seconds).
    Progress(u32),
    /// The page is being closed at this position (seconds).
    Closed(u32),
    ToggleSubscribe,
    Download(DownloadKind),
    OpenRelated(usize),
    OpenChannel,
    /// The comments were opened: loads the first page if needed.
    ShowComments,
    MoreComments,
    Comments(Result<CommentPage, String>),
    /// Shows the subtitle track with this index, or none.
    SelectSubtitles(Option<usize>),
    /// The cues of the track with this index were loaded.
    Subtitles(usize, Result<Vec<Cue>, String>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    LoadDetails(String),
    LoadStreams(String),
    LoadComments(String),
    LoadSubtitles {
        index: usize,
        url: String,
    },
    /// Offers the subtitle tracks with these names in the player, with the
    /// one shown.
    ShowSubtitleTracks {
        names: Vec<String>,
        selected: Option<usize>,
    },
    /// Remembers the language of the subtitles chosen, or that they are off.
    SaveSubtitleLanguage(Option<String>),
    /// Shows these cues in the player; none turns subtitles off.
    ShowSubtitles(Vec<Cue>),
    SubtitlesFailed(String),
    Play {
        source: PlaybackSource,
        start_at: u32,
    },
    RecordWatch {
        video: Video,
        position: u32,
    },
    Subscribe(Channel),
    Unsubscribe(String),
    Download(Video, DownloadKind),
    OpenVideo(Video),
    OpenChannel(Channel),
}

/// Where to resume a video watched up to `saved` seconds. Videos watched to
/// (almost) the end, or barely started, restart from the beginning.
pub fn resume_point(saved: u32, duration: Option<u32>) -> u32 {
    // Within the last 10 s and past 90 % counts as finished.
    let near_end = duration.is_some_and(|d| saved + 10 >= d && saved * 10 >= d * 9);
    if saved < 5 || near_end { 0 } else { saved }
}

impl State {
    pub fn new(init: Init) -> (Self, Vec<Effect>) {
        let resume_at = resume_point(init.saved_position.unwrap_or(0), init.video.duration_secs);
        let mut state = State {
            video: init.video,
            details: Load::Loading,
            playback: Load::Loading,
            subscribed: init.subscribed,
            comments: Comments::default(),
            subtitles: Subtitles {
                preferred: init.subtitle_language,
                ..Subtitles::default()
            },
            resume_at,
            saved_position: None,
        };
        let mut effects = vec![Effect::LoadDetails(state.video.id.clone())];
        let local_uri = init
            .local_file
            .and_then(|path| url::Url::from_file_path(path).ok());
        match local_uri {
            Some(uri) => effects.extend(state.play(PlaybackSource::Uri(uri.into()))),
            None => effects.push(Effect::LoadStreams(state.video.id.clone())),
        }
        (state, effects)
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Details(Ok(details)) => {
                let duration = self.video.duration_secs;
                let thumbnail = self.video.thumbnail_url.take();
                self.video = details.video.clone();
                self.video.duration_secs = self.video.duration_secs.or(duration);
                self.video.thumbnail_url = self.video.thumbnail_url.take().or(thumbnail);
                self.comments = Comments {
                    count: details.comments.count,
                    next: details.comments.next.clone(),
                    ..Comments::default()
                };
                self.details = Load::Loaded(details);
            }
            Msg::Details(Err(e)) => self.details = Load::Failed(e),
            Msg::Streams(Ok(streams)) => match streams::choose_source(&streams, MAX_HEIGHT) {
                Some(source) => {
                    let mut effects = self.play(source);
                    effects.extend(self.offer_subtitles(streams.subtitles));
                    return effects;
                }
                None => self.playback = Load::Failed("No playable stream found".into()),
            },
            Msg::Streams(Err(e)) => self.playback = Load::Failed(e),
            Msg::Progress(position) => {
                if self
                    .saved_position
                    .is_some_and(|saved| position.abs_diff(saved) >= SAVE_INTERVAL_SECS)
                {
                    return vec![self.record(position)];
                }
            }
            Msg::Closed(position) => {
                if self.saved_position.is_some() {
                    return vec![self.record(position)];
                }
            }
            Msg::ToggleSubscribe => {
                if let Load::Loaded(details) = &self.details {
                    self.subscribed = !self.subscribed;
                    return vec![if self.subscribed {
                        Effect::Subscribe(details.channel.clone())
                    } else {
                        Effect::Unsubscribe(details.channel.id.clone())
                    }];
                }
            }
            Msg::Download(kind) => return vec![Effect::Download(self.video.clone(), kind)],
            Msg::OpenRelated(index) => {
                if let Load::Loaded(details) = &self.details
                    && let Some(video) = details.related.get(index)
                {
                    return vec![Effect::OpenVideo(video.clone())];
                }
            }
            Msg::OpenChannel => {
                if let Load::Loaded(details) = &self.details {
                    return vec![Effect::OpenChannel(details.channel.clone())];
                }
            }
            Msg::ShowComments if self.comments.items.is_empty() => return self.load_comments(),
            Msg::ShowComments => {}
            Msg::MoreComments => return self.load_comments(),
            Msg::Comments(Ok(page)) => {
                let comments = &mut self.comments;
                comments.items.extend(page.comments);
                comments.count = page.count.or(comments.count);
                comments.next = page.next;
                comments.loading = false;
            }
            Msg::Comments(Err(e)) => {
                self.comments.error = Some(e);
                self.comments.loading = false;
            }
            // Also the menu echoing a change made here.
            Msg::SelectSubtitles(index) if index == self.subtitles.selected => {}
            Msg::SelectSubtitles(None) => {
                self.subtitles.selected = None;
                return vec![
                    Effect::ShowSubtitles(Vec::new()),
                    Effect::SaveSubtitleLanguage(None),
                ];
            }
            Msg::SelectSubtitles(Some(index)) => {
                if let Some(track) = self.subtitles.tracks.get(index) {
                    let lang = track.lang.clone();
                    return vec![
                        self.load_subtitles(index),
                        Effect::SaveSubtitleLanguage(Some(lang)),
                    ];
                }
            }
            Msg::Subtitles(index, result) if self.subtitles.selected == Some(index) => {
                return vec![match result {
                    Ok(cues) => Effect::ShowSubtitles(cues),
                    Err(e) => {
                        self.subtitles.selected = None;
                        Effect::SubtitlesFailed(e)
                    }
                }];
            }
            // A previous choice.
            Msg::Subtitles(..) => {}
        }
        Vec::new()
    }

    /// Offers `tracks`, showing the one in the preferred language.
    fn offer_subtitles(&mut self, tracks: Vec<SubtitleTrack>) -> Vec<Effect> {
        if tracks.is_empty() {
            return Vec::new();
        }
        let preferred = self
            .subtitles
            .preferred
            .as_deref()
            .and_then(|lang| subtitles::preferred_track(&tracks, lang));
        let names = tracks.iter().map(|t| t.name.clone()).collect();
        self.subtitles.tracks = tracks;
        let mut effects = vec![Effect::ShowSubtitleTracks {
            names,
            selected: preferred,
        }];
        effects.extend(preferred.map(|index| self.load_subtitles(index)));
        effects
    }

    /// Selects the track at `index`, which must exist.
    fn load_subtitles(&mut self, index: usize) -> Effect {
        self.subtitles.selected = Some(index);
        Effect::LoadSubtitles {
            index,
            url: self.subtitles.tracks[index].url.clone(),
        }
    }

    fn load_comments(&mut self) -> Vec<Effect> {
        let comments = &mut self.comments;
        match &comments.next {
            Some(token) if !comments.loading => {
                comments.loading = true;
                comments.error = None;
                vec![Effect::LoadComments(token.clone())]
            }
            _ => Vec::new(),
        }
    }

    fn play(&mut self, source: PlaybackSource) -> Vec<Effect> {
        self.playback = Load::Loaded(source.clone());
        vec![
            Effect::Play {
                source,
                start_at: self.resume_at,
            },
            self.record(self.resume_at),
        ]
    }

    fn record(&mut self, position: u32) -> Effect {
        self.saved_position = Some(position);
        Effect::RecordWatch {
            video: self.video.clone(),
            position,
        }
    }
}

pub struct VideoPage {
    state: State,
    services: Services,
    player: Controller<PlayerView>,
    related: gtk::ListBox,
    comment_list: gtk::ListBox,
    /// Number of comments shown in `comment_list`.
    shown_comments: usize,
    avatar: adw::Avatar,
    fullscreen: bool,
    /// Whether the page is narrower than [`WIDE_WIDTH_SP`].
    narrow: bool,
    /// The window made fullscreen, to restore it when the page closes.
    window: Option<gtk::Window>,
}

// Messages are moved rarely; boxing `Msg` would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum PageMsg {
    State(Msg),
    ToggleFullscreen,
    LeaveFullscreen,
    TogglePlay,
    /// The page became narrower (true) or wider than [`WIDE_WIDTH_SP`].
    Narrow(bool),
}

#[derive(Debug)]
pub enum Output {
    OpenVideo(Video),
    OpenChannel(Channel),
    Download(Video, DownloadKind),
    SubscriptionsChanged,
}

#[relm4::component(pub)]
impl Component for VideoPage {
    type Init = (Services, Init);
    type Input = PageMsg;
    type Output = Output;
    type CommandOutput = Msg;

    view! {
        adw::ToolbarView {
            #[watch]
            set_reveal_top_bars: !model.fullscreen,

            add_controller = gtk::EventControllerKey {
                set_propagation_phase: gtk::PropagationPhase::Capture,
                connect_key_pressed[sender] => move |_, key, _, modifiers| {
                    if !modifiers.is_empty() {
                        return gtk::glib::Propagation::Proceed;
                    }
                    let msg = match key {
                        gtk::gdk::Key::Escape => PageMsg::LeaveFullscreen,
                        gtk::gdk::Key::f => PageMsg::ToggleFullscreen,
                        gtk::gdk::Key::space => PageMsg::TogglePlay,
                        _ => return gtk::glib::Propagation::Proceed,
                    };
                    sender.input(msg);
                    gtk::glib::Propagation::Stop
                },
            },
            add_top_bar = &adw::HeaderBar {},

            #[wrap(Some)]
            set_content = &adw::BreakpointBin {
                set_width_request: 360,
                set_height_request: 200,
                add_breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
                    adw::BreakpointConditionLengthType::MaxWidth,
                    WIDE_WIDTH_SP,
                    adw::LengthUnit::Sp,
                )) {
                    connect_apply => PageMsg::Narrow(true),
                    connect_unapply => PageMsg::Narrow(false),
                },

                #[wrap(Some)]
                set_child = &gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                // Holds the player in the narrow and fullscreen layouts.
                #[name = "fixed_slot"]
                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    #[watch]
                    set_vexpand: model.fullscreen,
                },

                gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_hscrollbar_policy: gtk::PolicyType::Never,
                    #[watch]
                    set_visible: !model.fullscreen,

                    adw::Clamp {
                        set_maximum_size: 1000,

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 12,
                            set_margin_all: 12,

                            // Holds the player in the wide layout.
                            #[name = "scroll_slot"]
                            gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                add_css_class: "card",
                                set_overflow: gtk::Overflow::Hidden,
                                #[watch]
                                set_visible: !model.narrow && !model.fullscreen,
                            },

                            gtk::Label {
                                add_css_class: "title-3",
                                set_wrap: true,
                                set_xalign: 0.0,
                                #[watch]
                                set_label: &model.state.video.title,
                            },

                            gtk::Label {
                                add_css_class: "dimmed",
                                set_xalign: 0.0,
                                set_wrap: true,
                                #[watch]
                                set_label: &match &model.state.details {
                                    Load::Loaded(d) => format::details_meta(d),
                                    Load::Loading => "Loading…".into(),
                                    Load::Failed(e) => format!("Cannot load details: {e}"),
                                },
                            },

                            gtk::ListBox {
                                add_css_class: "boxed-list",
                                set_selection_mode: gtk::SelectionMode::None,
                                connect_row_activated[sender] => move |_, _| {
                                    sender.input(PageMsg::State(Msg::OpenChannel));
                                },

                                adw::ActionRow {
                                    set_activatable: true,
                                    set_use_markup: false,
                                    #[watch]
                                    set_title: &model.state.video.channel_name,
                                    #[local_ref]
                                    add_prefix = avatar -> adw::Avatar {
                                        set_size: 40,
                                        set_show_initials: true,
                                        #[watch]
                                        set_text: Some(&model.state.video.channel_name),
                                    },

                                    add_suffix = &gtk::Button {
                                        set_valign: gtk::Align::Center,
                                        #[watch]
                                        set_label: if model.state.subscribed { "Unsubscribe" } else { "Subscribe" },
                                        #[watch]
                                        set_css_classes: if model.state.subscribed { &[] } else { &["suggested-action"] },
                                        #[watch]
                                        set_sensitive: matches!(model.state.details, Load::Loaded(_)),
                                        connect_clicked => PageMsg::State(Msg::ToggleSubscribe),
                                    },
                                },
                            },

                            gtk::Box {
                                set_spacing: 6,

                                gtk::MenuButton {
                                    set_icon_name: "folder-download-symbolic",
                                    set_tooltip_text: Some("Download"),
                                    add_css_class: "circular",

                                    #[wrap(Some)]
                                    set_popover: download_popover = &gtk::Popover {
                                        gtk::Box {
                                            set_orientation: gtk::Orientation::Vertical,

                                            gtk::Button {
                                                set_label: "Download Video",
                                                add_css_class: "flat",
                                                connect_clicked[sender, download_popover] => move |_| {
                                                    download_popover.popdown();
                                                    sender.input(PageMsg::State(Msg::Download(DownloadKind::Video)));
                                                },
                                            },
                                            gtk::Button {
                                                set_label: "Download Audio",
                                                add_css_class: "flat",
                                                connect_clicked[sender, download_popover] => move |_| {
                                                    download_popover.popdown();
                                                    sender.input(PageMsg::State(Msg::Download(DownloadKind::Audio)));
                                                },
                                            },
                                        },
                                    },
                                },
                            },

                            gtk::Expander {
                                set_label: Some("Description"),
                                #[watch]
                                set_visible: matches!(&model.state.details, Load::Loaded(d) if !d.description.is_empty()),

                                gtk::Label {
                                    set_wrap: true,
                                    set_xalign: 0.0,
                                    set_selectable: true,
                                    set_margin_top: 6,
                                    #[watch]
                                    set_label: match &model.state.details {
                                        Load::Loaded(d) => &d.description,
                                        _ => "",
                                    },
                                },
                            },

                            gtk::Expander {
                                #[watch]
                                set_label: Some(&format::comments_title(model.state.comments.count)),
                                #[watch]
                                set_visible: matches!(model.state.details, Load::Loaded(_))
                                    && (model.state.comments.next.is_some() || !model.state.comments.items.is_empty()),
                                connect_expanded_notify[sender] => move |expander| {
                                    if expander.is_expanded() {
                                        sender.input(PageMsg::State(Msg::ShowComments));
                                    }
                                },

                                gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_spacing: 12,
                                    set_margin_top: 6,

                                    #[local_ref]
                                    comment_list -> gtk::ListBox {
                                        #[watch]
                                        set_visible: !model.state.comments.items.is_empty(),
                                    },

                                    adw::Spinner {
                                        #[watch]
                                        set_visible: model.state.comments.loading,
                                    },

                                    gtk::Label {
                                        add_css_class: "dimmed",
                                        set_wrap: true,
                                        set_xalign: 0.0,
                                        #[watch]
                                        set_visible: model.state.comments.error.is_some(),
                                        #[watch]
                                        set_label: &format!(
                                            "Cannot load comments: {}",
                                            model.state.comments.error.as_deref().unwrap_or_default()
                                        ),
                                    },

                                    gtk::Button {
                                        set_halign: gtk::Align::Center,
                                        add_css_class: "pill",
                                        #[watch]
                                        set_label: if model.state.comments.error.is_some() { "Retry" } else { "Load More" },
                                        #[watch]
                                        set_visible: model.state.comments.next.is_some()
                                            && !model.state.comments.loading
                                            && (!model.state.comments.items.is_empty() || model.state.comments.error.is_some()),
                                        connect_clicked => PageMsg::State(Msg::MoreComments),
                                    },
                                },
                            },

                            gtk::Label {
                                set_label: "Related Videos",
                                add_css_class: "heading",
                                set_xalign: 0.0,
                                #[watch]
                                set_visible: matches!(&model.state.details, Load::Loaded(d) if !d.related.is_empty()),
                            },

                            #[local_ref]
                            related -> gtk::ListBox {
                                connect_row_activated[sender] => move |_, row| {
                                    sender.input(PageMsg::State(Msg::OpenRelated(row.index() as usize)));
                                },
                            },
                        },
                    },
                },
                },
            },
        }
    }

    fn init(
        (services, init): Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let (state, effects) = State::new(init);
        let player =
            PlayerView::builder()
                .launch(())
                .forward(sender.input_sender(), |out| match out {
                    player_view::Output::Progress(p) => PageMsg::State(Msg::Progress(p)),
                    player_view::Output::ToggleFullscreen => PageMsg::ToggleFullscreen,
                    player_view::Output::SelectSubtitles(index) => {
                        PageMsg::State(Msg::SelectSubtitles(index))
                    }
                });
        let model = VideoPage {
            state,
            services,
            player,
            related: widgets::boxed_list(),
            comment_list: widgets::boxed_list(),
            shown_comments: 0,
            avatar: adw::Avatar::default(),
            fullscreen: false,
            narrow: false,
            window: None,
        };
        let related = &model.related;
        let comment_list = &model.comment_list;
        let avatar = &model.avatar;
        let widgets = view_output!();
        model.place_player(&widgets);
        model.run(effects, &sender);
        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: PageMsg,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        match msg {
            PageMsg::State(msg) => {
                let details = matches!(msg, Msg::Details(_));
                let streams = matches!(msg, Msg::Streams(_));
                let comments = matches!(msg, Msg::Comments(_));
                let effects = self.state.update(msg);
                self.run(effects, &sender);
                if details {
                    self.fill_related();
                }
                if comments {
                    self.append_comments();
                }
                if streams && let Load::Failed(e) = &self.state.playback {
                    self.player.emit(player_view::Msg::Fail(e.clone()));
                }
            }
            PageMsg::Narrow(narrow) => {
                self.narrow = narrow;
                self.place_player(widgets);
            }
            PageMsg::TogglePlay => self.player.emit(player_view::Msg::TogglePlay),
            PageMsg::LeaveFullscreen if self.fullscreen => {
                self.update_with_view(widgets, PageMsg::ToggleFullscreen, sender, root);
                return;
            }
            PageMsg::LeaveFullscreen => {}
            PageMsg::ToggleFullscreen => {
                self.fullscreen = !self.fullscreen;
                self.player
                    .emit(player_view::Msg::SetFullscreen(self.fullscreen));
                self.place_player(widgets);
                self.window = root.root().and_downcast::<gtk::Window>();
                if let Some(window) = &self.window {
                    window.set_fullscreened(self.fullscreen);
                }
            }
        }
        self.update_view(widgets, sender);
    }

    /// Called when the page is closed: saves the playback position.
    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Output>) {
        let position = self.player.model().position_secs();
        for effect in self.state.update(Msg::Closed(position)) {
            if let Effect::RecordWatch { video, position } = effect {
                log_db_error(
                    self.services
                        .db
                        .record_watch(&video, position, chrono::Utc::now()),
                );
            }
        }
        if self.fullscreen
            && let Some(window) = &self.window
        {
            window.set_fullscreened(false);
        }
    }

    fn update_cmd_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: Msg,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        self.update_with_view(widgets, PageMsg::State(msg), sender, root);
    }
}

impl VideoPage {
    /// Puts the player above the scrolled details (narrow, fullscreen) or
    /// into them (wide).
    fn place_player(&self, widgets: &VideoPageWidgets) {
        let player = self.player.widget();
        let slot = if self.narrow || self.fullscreen {
            &widgets.fixed_slot
        } else {
            &widgets.scroll_slot
        };
        if player.parent().as_ref() != Some(slot.upcast_ref()) {
            if let Some(parent) = player.parent().and_downcast::<gtk::Box>() {
                parent.remove(player);
            }
            slot.append(player);
        }
        player.set_vexpand(self.fullscreen);
    }

    fn run(&self, effects: Vec<Effect>, sender: &ComponentSender<Self>) {
        for effect in effects {
            match effect {
                Effect::LoadDetails(id) => {
                    let backend = self.services.backend.clone();
                    sender.oneshot_command(async move {
                        Msg::Details(backend.video_details(&id).await.map_err(|e| e.to_string()))
                    });
                }
                Effect::LoadStreams(id) => {
                    let backend = self.services.backend.clone();
                    sender.oneshot_command(async move {
                        Msg::Streams(backend.streams(&id).await.map_err(|e| e.to_string()))
                    });
                }
                Effect::LoadComments(token) => {
                    let backend = self.services.backend.clone();
                    sender.oneshot_command(async move {
                        Msg::Comments(backend.comments(&token).await.map_err(|e| e.to_string()))
                    });
                }
                Effect::LoadSubtitles { index, url } => {
                    let backend = self.services.backend.clone();
                    sender.oneshot_command(async move {
                        let cues = backend.subtitles(&url).await.map_err(|e| e.to_string());
                        Msg::Subtitles(index, cues)
                    });
                }
                Effect::ShowSubtitleTracks { names, selected } => {
                    self.player
                        .emit(player_view::Msg::SetSubtitleTracks { names, selected });
                }
                Effect::SaveSubtitleLanguage(lang) => {
                    log_db_error(self.services.db.set_subtitle_language(lang.as_deref()));
                }
                Effect::ShowSubtitles(cues) => {
                    self.player.emit(player_view::Msg::SetSubtitles(cues));
                }
                Effect::SubtitlesFailed(e) => {
                    self.player.emit(player_view::Msg::SubtitlesFailed(e));
                }
                Effect::Play { source, start_at } => {
                    self.player
                        .emit(player_view::Msg::Load { source, start_at });
                }
                Effect::RecordWatch { video, position } => {
                    log_db_error(self.services.db.record_watch(
                        &video,
                        position,
                        chrono::Utc::now(),
                    ));
                }
                Effect::Subscribe(channel) => {
                    log_db_error(self.services.db.subscribe(&channel));
                    let _ = sender.output(Output::SubscriptionsChanged);
                }
                Effect::Unsubscribe(id) => {
                    log_db_error(self.services.db.unsubscribe(&id));
                    let _ = sender.output(Output::SubscriptionsChanged);
                }
                Effect::Download(video, kind) => {
                    let _ = sender.output(Output::Download(video, kind));
                }
                Effect::OpenVideo(video) => {
                    let _ = sender.output(Output::OpenVideo(video));
                }
                Effect::OpenChannel(channel) => {
                    let _ = sender.output(Output::OpenChannel(channel));
                }
            }
        }
    }

    fn append_comments(&mut self) {
        for comment in &self.state.comments.items[self.shown_comments..] {
            self.comment_list.append(&widgets::comment_row(comment));
        }
        self.shown_comments = self.state.comments.items.len();
    }

    fn fill_related(&self) {
        self.related.remove_all();
        if let Load::Loaded(details) = &self.state.details {
            images::set_avatar(&self.avatar, details.channel.avatar_url.as_deref());
            for video in &details.related {
                self.related.append(&widgets::video_row(video));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{AudioStream, Comment, CommentPage, SubtitleTrack, VideoStream};
    use crate::core::subtitles::Cue;

    fn video(id: &str) -> Video {
        Video {
            id: id.into(),
            title: id.into(),
            channel_id: "UC1".into(),
            channel_name: "Chan".into(),
            published: None,
            duration_secs: Some(600),
            thumbnail_url: None,
        }
    }

    fn channel() -> Channel {
        Channel {
            id: "UC1".into(),
            name: "Chan".into(),
            avatar_url: None,
        }
    }

    fn details() -> VideoDetails {
        VideoDetails {
            video: video("v"),
            channel: channel(),
            description: "desc".into(),
            view_count: Some(1),
            like_count: None,
            related: vec![video("r0"), video("r1")],
            comments: Default::default(),
        }
    }

    fn init() -> Init {
        Init {
            video: video("v"),
            saved_position: None,
            local_file: None,
            subscribed: false,
            subtitle_language: None,
        }
    }

    fn muxed() -> Streams {
        Streams {
            muxed: vec![VideoStream {
                url: "https://muxed".into(),
                width: 640,
                height: 360,
                fps: 30,
                bitrate: 1,
                mime: "video/mp4".into(),
                ranges: None,
                size: None,
            }],
            video_only: vec![],
            audio: vec![AudioStream {
                url: "https://a".into(),
                bitrate: 1,
                mime: "audio/webm".into(),
                ranges: None,
                size: None,
            }],
            hls_url: None,
            is_live: false,
            duration_secs: Some(600),
            subtitles: vec![],
        }
    }

    #[test]
    fn resume_point_restarts_finished_videos() {
        assert_eq!(resume_point(120, Some(600)), 120);
        assert_eq!(resume_point(590, Some(600)), 0);
        assert_eq!(resume_point(120, None), 120);
        assert_eq!(resume_point(3, Some(600)), 0);
        // Short videos: "near the end" is relative to the duration.
        assert_eq!(resume_point(5, Some(10)), 5);
        assert_eq!(resume_point(9, Some(10)), 0);
    }

    #[test]
    fn opening_loads_details_and_streams() {
        let (state, effects) = State::new(init());
        assert_eq!(
            effects,
            [
                Effect::LoadDetails("v".into()),
                Effect::LoadStreams("v".into())
            ]
        );
        assert_eq!(state.details, Load::Loading);
        assert_eq!(state.playback, Load::Loading);
    }

    #[test]
    fn opening_downloaded_video_plays_local_file() {
        let (state, effects) = State::new(Init {
            local_file: Some("/videos/v.mp4".into()),
            saved_position: Some(60),
            ..init()
        });
        let source = PlaybackSource::Uri("file:///videos/v.mp4".into());
        assert_eq!(
            effects,
            [
                Effect::LoadDetails("v".into()),
                Effect::Play {
                    source: source.clone(),
                    start_at: 60
                },
                Effect::RecordWatch {
                    video: video("v"),
                    position: 60
                },
            ]
        );
        assert_eq!(state.playback, Load::Loaded(source));
    }

    #[test]
    fn streams_start_playback_at_resume_point() {
        let (mut state, _) = State::new(Init {
            saved_position: Some(42),
            ..init()
        });
        let effects = state.update(Msg::Streams(Ok(muxed())));
        let source = PlaybackSource::Uri("https://muxed".into());
        assert_eq!(
            effects,
            [
                Effect::Play {
                    source: source.clone(),
                    start_at: 42
                },
                Effect::RecordWatch {
                    video: video("v"),
                    position: 42
                },
            ]
        );
        assert_eq!(state.playback, Load::Loaded(source));
    }

    #[test]
    fn unplayable_streams_fail() {
        let (mut state, _) = State::new(init());
        assert!(
            state
                .update(Msg::Streams(Ok(Streams::default())))
                .is_empty()
        );
        assert!(matches!(state.playback, Load::Failed(_)));

        state.update(Msg::Streams(Err("blocked".into())));
        assert_eq!(state.playback, Load::Failed("blocked".into()));
    }

    #[test]
    fn details_update_video_and_failures_are_kept() {
        let (mut state, _) = State::new(init());
        let mut d = details();
        d.video.title = "Full title".into();
        state.update(Msg::Details(Ok(d.clone())));
        assert_eq!(state.video.title, "Full title");
        // Duration is not part of the details response; keep the known one.
        assert_eq!(state.video.duration_secs, Some(600));
        assert!(matches!(state.details, Load::Loaded(_)));

        state.update(Msg::Details(Err("x".into())));
        assert_eq!(state.details, Load::Failed("x".into()));
    }

    #[test]
    fn progress_is_saved_every_few_seconds() {
        let (mut state, _) = State::new(init());
        state.update(Msg::Streams(Ok(muxed())));

        assert!(state.update(Msg::Progress(2)).is_empty());
        assert_eq!(
            state.update(Msg::Progress(5)),
            [Effect::RecordWatch {
                video: video("v"),
                position: 5
            }]
        );
        assert!(state.update(Msg::Progress(7)).is_empty());
        assert_eq!(
            state.update(Msg::Closed(8)),
            [Effect::RecordWatch {
                video: video("v"),
                position: 8
            }]
        );
    }

    #[test]
    fn closing_before_playback_records_nothing() {
        let (mut state, _) = State::new(init());
        assert!(state.update(Msg::Closed(0)).is_empty());
    }

    #[test]
    fn toggling_subscription() {
        let (mut state, _) = State::new(init());
        assert!(
            state.update(Msg::ToggleSubscribe).is_empty(),
            "needs channel details"
        );

        state.update(Msg::Details(Ok(details())));
        assert_eq!(
            state.update(Msg::ToggleSubscribe),
            [Effect::Subscribe(channel())]
        );
        assert!(state.subscribed);
        assert_eq!(
            state.update(Msg::ToggleSubscribe),
            [Effect::Unsubscribe("UC1".into())]
        );
        assert!(!state.subscribed);
    }

    #[test]
    fn actions_open_and_download() {
        let (mut state, _) = State::new(init());
        state.update(Msg::Details(Ok(details())));

        assert_eq!(
            state.update(Msg::OpenRelated(1)),
            [Effect::OpenVideo(video("r1"))]
        );
        assert!(state.update(Msg::OpenRelated(5)).is_empty());
        assert_eq!(
            state.update(Msg::OpenChannel),
            [Effect::OpenChannel(channel())]
        );
        assert_eq!(
            state.update(Msg::Download(DownloadKind::Audio)),
            [Effect::Download(state.video.clone(), DownloadKind::Audio)]
        );
    }

    fn comment(text: &str) -> Comment {
        Comment {
            author: None,
            text: text.into(),
            published: None,
            like_count: None,
            reply_count: 0,
            pinned: false,
            by_owner: false,
        }
    }

    fn page(texts: &[&str], next: Option<&str>) -> CommentPage {
        CommentPage {
            comments: texts.iter().map(|t| comment(t)).collect(),
            count: Some(3),
            next: next.map(Into::into),
        }
    }

    fn with_comments() -> State {
        let (mut state, _) = State::new(init());
        let mut d = details();
        d.comments = page(&[], Some("p1"));
        state.update(Msg::Details(Ok(d)));
        state
    }

    #[test]
    fn showing_comments_loads_first_page_once() {
        let mut state = with_comments();
        assert_eq!(state.comments.count, Some(3));
        assert!(state.comments.items.is_empty(), "loaded on demand");

        assert_eq!(
            state.update(Msg::ShowComments),
            [Effect::LoadComments("p1".into())]
        );
        assert!(state.comments.loading);
        assert!(
            state.update(Msg::ShowComments).is_empty(),
            "already loading"
        );

        state.update(Msg::Comments(Ok(page(&["a", "b"], Some("p2")))));
        assert!(!state.comments.loading);
        assert_eq!(state.comments.items, [comment("a"), comment("b")]);
        assert!(state.update(Msg::ShowComments).is_empty(), "already loaded");
    }

    #[test]
    fn more_comments_are_appended_until_last_page() {
        let mut state = with_comments();
        state.update(Msg::ShowComments);
        state.update(Msg::Comments(Ok(page(&["a"], Some("p2")))));

        assert_eq!(
            state.update(Msg::MoreComments),
            [Effect::LoadComments("p2".into())]
        );
        state.update(Msg::Comments(Ok(page(&["b"], None))));
        assert_eq!(state.comments.items, [comment("a"), comment("b")]);
        assert_eq!(state.comments.next, None);
        assert!(state.update(Msg::MoreComments).is_empty());
    }

    #[test]
    fn failed_comments_can_be_retried() {
        let mut state = with_comments();
        state.update(Msg::ShowComments);
        state.update(Msg::Comments(Err("offline".into())));
        assert_eq!(state.comments.error.as_deref(), Some("offline"));
        assert!(!state.comments.loading);

        assert_eq!(
            state.update(Msg::ShowComments),
            [Effect::LoadComments("p1".into())]
        );
        assert_eq!(state.comments.error, None);
    }

    #[test]
    fn comments_need_details() {
        let (mut state, _) = State::new(init());
        assert!(state.update(Msg::ShowComments).is_empty());
        assert!(state.update(Msg::MoreComments).is_empty());
    }

    fn track(lang: &str) -> SubtitleTrack {
        SubtitleTrack {
            name: lang.to_uppercase(),
            lang: lang.into(),
            url: format!("https://subs/{lang}"),
            auto_generated: false,
        }
    }

    fn cues() -> Vec<Cue> {
        vec![Cue {
            start: std::time::Duration::ZERO,
            end: std::time::Duration::from_secs(1),
            text: "Hi".into(),
        }]
    }

    fn with_tracks() -> State {
        let (mut state, _) = State::new(init());
        let effects = state.update(Msg::Streams(Ok(Streams {
            subtitles: vec![track("en"), track("de")],
            ..muxed()
        })));
        assert_eq!(
            effects.last(),
            Some(&Effect::ShowSubtitleTracks {
                names: vec!["EN".into(), "DE".into()],
                selected: None,
            })
        );
        state
    }

    #[test]
    fn selecting_subtitles_loads_and_shows_them() {
        let mut state = with_tracks();
        assert_eq!(state.subtitles.selected, None, "off by default");

        assert_eq!(
            state.update(Msg::SelectSubtitles(Some(1))),
            [
                Effect::LoadSubtitles {
                    index: 1,
                    url: "https://subs/de".into()
                },
                Effect::SaveSubtitleLanguage(Some("de".into())),
            ]
        );
        assert_eq!(
            state.update(Msg::Subtitles(1, Ok(cues()))),
            [Effect::ShowSubtitles(cues())]
        );

        assert_eq!(
            state.update(Msg::SelectSubtitles(None)),
            [
                Effect::ShowSubtitles(vec![]),
                Effect::SaveSubtitleLanguage(None),
            ]
        );
        assert_eq!(state.subtitles.selected, None);
    }

    #[test]
    fn preferred_language_is_shown_without_saving() {
        let (mut state, _) = State::new(Init {
            subtitle_language: Some("de-DE".into()),
            ..init()
        });
        let effects = state.update(Msg::Streams(Ok(Streams {
            subtitles: vec![track("en"), track("de")],
            ..muxed()
        })));

        assert_eq!(
            effects[2..],
            [
                Effect::ShowSubtitleTracks {
                    names: vec!["EN".into(), "DE".into()],
                    selected: Some(1),
                },
                Effect::LoadSubtitles {
                    index: 1,
                    url: "https://subs/de".into()
                },
            ]
        );
        assert_eq!(state.subtitles.selected, Some(1));
        // The menu echoes the choice back.
        assert!(state.update(Msg::SelectSubtitles(Some(1))).is_empty());
    }

    #[test]
    fn unavailable_preferred_language_keeps_subtitles_off() {
        let (mut state, _) = State::new(Init {
            subtitle_language: Some("fr".into()),
            ..init()
        });
        state.update(Msg::Streams(Ok(Streams {
            subtitles: vec![track("en")],
            ..muxed()
        })));
        assert_eq!(state.subtitles.selected, None);
    }

    #[test]
    fn failed_subtitles_keep_the_preference() {
        let mut state = with_tracks();
        state.update(Msg::SelectSubtitles(Some(0)));
        state.update(Msg::Subtitles(0, Err("HTTP 429".into())));

        // The menu switching back to "Off" is no choice of the user.
        assert!(state.update(Msg::SelectSubtitles(None)).is_empty());
    }

    #[test]
    fn subtitles_of_a_previous_choice_are_ignored() {
        let mut state = with_tracks();
        state.update(Msg::SelectSubtitles(Some(0)));
        state.update(Msg::SelectSubtitles(Some(1)));

        assert!(state.update(Msg::Subtitles(0, Ok(cues()))).is_empty());
        state.update(Msg::SelectSubtitles(None));
        assert!(state.update(Msg::Subtitles(1, Ok(cues()))).is_empty());
    }

    #[test]
    fn failed_subtitles_turn_them_off() {
        let mut state = with_tracks();
        state.update(Msg::SelectSubtitles(Some(0)));

        assert_eq!(
            state.update(Msg::Subtitles(0, Err("HTTP 429".into()))),
            [Effect::SubtitlesFailed("HTTP 429".into())]
        );
        assert_eq!(state.subtitles.selected, None);
    }

    #[test]
    fn unknown_tracks_are_ignored() {
        let mut state = with_tracks();
        assert!(state.update(Msg::SelectSubtitles(Some(5))).is_empty());
        assert_eq!(state.subtitles.selected, None);
    }

    #[test]
    fn videos_without_subtitles_show_no_tracks() {
        let (mut state, _) = State::new(init());
        let effects = state.update(Msg::Streams(Ok(muxed())));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::ShowSubtitleTracks { .. }))
        );
    }
}
