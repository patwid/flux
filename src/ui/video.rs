//! Video page: player, details, comments, related videos.

use std::path::PathBuf;

use relm4::adw::prelude::*;
use relm4::gtk::gio;
use relm4::{
    Component, ComponentController, ComponentParts, ComponentSender, Controller, RelmWidgetExt,
    adw, gtk,
};

use super::comments::{CommentList, Comments};
use super::player_view::{self, PlayerView};
use super::{Services, format, images, log_db_error, widgets};
use crate::core::model::{
    Channel, Comment, CommentPage, DownloadKind, PlaybackSource, Streams, SubtitleTrack, Video,
    VideoDetails,
};
use crate::core::subtitles::{self, Cue, Placement};
use crate::core::{backend, streams};

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

/// The subtitle tracks of a video and the one shown.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Subtitles {
    pub tracks: Vec<SubtitleTrack>,
    /// Index of the chosen track; `None` when subtitles are off.
    pub selected: Option<usize>,
    /// The language to show when tracks arrive, if any.
    pub preferred: Option<String>,
}

/// The tabs below the description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Comments,
    Related,
}

impl Tab {
    /// The name of the tab's toggle and stack page.
    pub fn name(self) -> &'static str {
        match self {
            Tab::Comments => "comments",
            Tab::Related => "related",
        }
    }

    /// The tab with toggle and stack page `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        [Tab::Comments, Tab::Related]
            .into_iter()
            .find(|tab| tab.name() == name)
    }
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
    /// The tab shown below the description.
    pub tab: Tab,
    pub fullscreen: bool,
    /// Whether the page is narrower than [`WIDE_WIDTH_SP`].
    pub narrow: bool,
    /// Position to start playback at, in seconds.
    resume_at: u32,
    /// Last position written to the history.
    saved_position: Option<u32>,
    /// Whether the video is streamed rather than played from a download.
    streamed: bool,
    /// Recoveries from playback errors since playback last got past one.
    recoveries: u32,
    /// Position (seconds) of the last playback error.
    failed_at: Option<u32>,
    /// Whether to play once the video is loaded again after an error.
    resume_playing: bool,
}

/// Playback errors recovered from in a row before giving up.
const MAX_RECOVERIES: u32 = 2;

/// Seconds playback must get past an error before errors count anew.
const RECOVERED_AFTER_SECS: u32 = 10;

/// Everything known about a video when its page is opened.
#[derive(Debug, Clone)]
pub struct Init {
    pub video: Video,
    /// Saved position from the watch history.
    pub saved_position: Option<u32>,
    /// A completed download to play instead of streaming.
    pub local_file: Option<PathBuf>,
    /// Subtitles saved with `local_file`.
    pub local_subtitles: Vec<SubtitleTrack>,
    pub subscribed: bool,
    /// The language of the subtitles to show, if the user wants any.
    pub subtitle_language: Option<String>,
}

// See `PageMsg`.
#[expect(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Msg {
    Details(Result<VideoDetails, String>),
    /// Loads the details again after they failed to load.
    RetryDetails,
    Streams(Result<Streams, String>),
    /// The player failed at this position (seconds), while playing or not.
    PlaybackFailed {
        error: String,
        position: u32,
        playing: bool,
    },
    /// Playback reached this position (seconds).
    Progress(u32),
    /// The page is being closed at this position (seconds).
    Closed(u32),
    ToggleSubscribe,
    Download(DownloadKind),
    OpenRelated(usize),
    OpenChannel,
    /// The user chose a tab; the comments load their first page if needed.
    SelectTab(Tab),
    MoreComments,
    Comments(Result<CommentPage, String>),
    /// Opens the replies to the comment with this index.
    OpenReplies(usize),
    /// Shows the subtitle track with this index, or none.
    SelectSubtitles(Option<usize>),
    /// The cues of the track with this index were loaded.
    Subtitles(usize, Result<Vec<Cue>, String>),
    ToggleFullscreen,
    LeaveFullscreen,
    /// The page became narrower (true) or wider than [`WIDE_WIDTH_SP`].
    Narrow(bool),
}

#[derive(Debug, PartialEq)]
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
    /// Loads `source` at `start_at` seconds, and plays it if `play`.
    Play {
        source: PlaybackSource,
        start_at: u32,
        play: bool,
    },
    /// Shows that the video cannot be played.
    PlaybackFailed(String),
    RecordWatch {
        video: Video,
        position: u32,
    },
    Subscribe(Channel),
    Unsubscribe(String),
    /// Tells the user what happened.
    Toast(String),
    Download(Video, DownloadKind),
    OpenVideo(Video),
    OpenChannel(Channel),
    OpenReplies(Comment),
    /// Makes the page fullscreen, or not.
    SetFullscreen(bool),
}

/// The layout of the video page: the player above the scrolled details
/// (narrow), scrolling with them (wide), or alone (fullscreen).
pub fn layout_name(narrow: bool, fullscreen: bool) -> &'static str {
    match (narrow, fullscreen) {
        (_, true) => "fullscreen",
        (true, false) => "narrow",
        (false, false) => "wide",
    }
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
            tab: Tab::Comments,
            fullscreen: false,
            narrow: false,
            resume_at,
            saved_position: None,
            streamed: true,
            recoveries: 0,
            failed_at: None,
            resume_playing: false,
        };
        let mut effects = vec![Effect::LoadDetails(state.video.id.clone())];
        let local_uri = init
            .local_file
            .and_then(|path| url::Url::from_file_path(path).ok());
        match local_uri {
            Some(uri) => {
                state.streamed = false;
                effects.extend(state.play(PlaybackSource::Uri(uri.into())));
                effects.extend(state.offer_subtitles(init.local_subtitles));
            }
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
                self.comments = Comments::new(details.comments.clone());
                self.details = Load::Loaded(details);
                // Comments first, or the related videos if there are none.
                self.tab = if self.comments.available() {
                    Tab::Comments
                } else {
                    Tab::Related
                };
                return self.load_comments();
            }
            Msg::Details(Err(e)) => self.details = Load::Failed(e),
            Msg::RetryDetails => {
                if let Load::Failed(_) = self.details {
                    self.details = Load::Loading;
                    return vec![Effect::LoadDetails(self.video.id.clone())];
                }
            }
            Msg::Streams(Ok(streams)) => match streams::choose_source(&streams, MAX_HEIGHT) {
                Some(source) => {
                    let mut effects = self.play(source);
                    // Streams fetched again after an error offer the same tracks.
                    if self.subtitles.tracks.is_empty() {
                        effects.extend(self.offer_subtitles(streams.subtitles));
                    }
                    return effects;
                }
                None => return self.fail("No playable stream found".into()),
            },
            Msg::Streams(Err(e)) => return self.fail(e),
            Msg::PlaybackFailed {
                error,
                position,
                playing,
            } => return self.recover(error, position, playing),
            Msg::Progress(position) => {
                if self
                    .failed_at
                    .is_some_and(|at| position >= at + RECOVERED_AFTER_SECS)
                {
                    self.recoveries = 0;
                    self.failed_at = None;
                }
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
                    let channel = &details.channel;
                    return vec![
                        if self.subscribed {
                            Effect::Subscribe(channel.clone())
                        } else {
                            Effect::Unsubscribe(channel.id.clone())
                        },
                        Effect::Toast(format::subscription(&channel.name, self.subscribed)),
                    ];
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
            Msg::SelectTab(tab) => {
                self.tab = tab;
                if tab == Tab::Comments && self.comments.items.is_empty() {
                    return self.load_comments();
                }
            }
            Msg::MoreComments => return self.load_comments(),
            Msg::Comments(result) => self.comments.loaded(result),
            Msg::OpenReplies(index) => {
                if let Some(comment) = self.comments.items.get(index)
                    && comment.replies.is_some()
                {
                    return vec![Effect::OpenReplies(comment.clone())];
                }
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
                    // Auto-generated tracks place every cue at the left edge
                    // (`align:start position:0%`); YouTube centers them.
                    Ok(mut cues) if self.subtitles.tracks[index].auto_generated => {
                        for cue in &mut cues {
                            cue.placement = Placement::default();
                        }
                        Effect::ShowSubtitles(cues)
                    }
                    Ok(cues) => Effect::ShowSubtitles(cues),
                    Err(e) => {
                        self.subtitles.selected = None;
                        // The cues of an earlier track may still show.
                        return vec![
                            Effect::ShowSubtitles(Vec::new()),
                            Effect::SubtitlesFailed(e),
                        ];
                    }
                }];
            }
            // A previous choice.
            Msg::Subtitles(..) => {}
            Msg::LeaveFullscreen if !self.fullscreen => {}
            Msg::ToggleFullscreen | Msg::LeaveFullscreen => {
                self.fullscreen = !self.fullscreen;
                return vec![Effect::SetFullscreen(self.fullscreen)];
            }
            Msg::Narrow(narrow) => self.narrow = narrow,
        }
        Vec::new()
    }

    /// The name of the page's layout, see [`layout_name`].
    pub fn layout(&self) -> &'static str {
        layout_name(self.narrow, self.fullscreen)
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
        self.comments
            .load_next()
            .map(Effect::LoadComments)
            .into_iter()
            .collect()
    }

    /// Loads the video again at `position` after a playback error, playing
    /// it if it was `playing`. Streams are fetched again, as their URLs may
    /// have expired. Gives up after [`MAX_RECOVERIES`] errors in a row.
    fn recover(&mut self, error: String, position: u32, playing: bool) -> Vec<Effect> {
        if self.recoveries >= MAX_RECOVERIES {
            return self.fail(error);
        }
        self.recoveries += 1;
        self.failed_at = Some(position);
        self.resume_at = position;
        self.resume_playing = playing;
        if !self.streamed
            && let Load::Loaded(source) = &self.playback
        {
            return self.play(source.clone());
        }
        self.playback = Load::Loading;
        vec![Effect::LoadStreams(self.video.id.clone())]
    }

    /// Shows that the video cannot be played.
    fn fail(&mut self, error: String) -> Vec<Effect> {
        self.playback = Load::Failed(error.clone());
        vec![Effect::PlaybackFailed(error)]
    }

    fn play(&mut self, source: PlaybackSource) -> Vec<Effect> {
        self.playback = Load::Loaded(source.clone());
        vec![
            Effect::Play {
                source,
                start_at: self.resume_at,
                play: std::mem::take(&mut self.resume_playing),
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
    related: adw::PreferencesGroup,
    /// Rows of the related videos, to remove them again.
    related_rows: Vec<adw::ActionRow>,
    comment_list: CommentList,
    avatar: adw::Avatar,
    /// The window made fullscreen, to restore it when the page closes.
    window: Option<gtk::Window>,
}

// Messages are moved rarely; boxing `Msg` would only add noise.
#[expect(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum PageMsg {
    State(Msg),
    TogglePlay,
}

#[derive(Debug)]
pub enum Output {
    OpenVideo(Video),
    OpenChannel(Channel),
    Download(Video, DownloadKind),
    SubscriptionsChanged,
    OpenReplies(Comment),
    Toast(String),
}

#[relm4::component(pub)]
impl Component for VideoPage {
    type Init = (Services, Init);
    type Input = PageMsg;
    type Output = Output;
    type CommandOutput = Msg;

    view! {
        #[root]
        adw::ToolbarView {
            #[watch]
            set_reveal_top_bars: !model.state.fullscreen,

            add_top_bar = &adw::HeaderBar {
                pack_end = &gtk::MenuButton {
                    set_icon_name: "folder-download-symbolic",
                    set_tooltip_text: Some("Download"),
                    set_menu_model: Some(&download_menu()),
                },
            },
            add_top_bar = &adw::Banner {
                set_title: "Cannot load the video details",
                set_button_label: Some("Retry"),
                #[watch]
                set_revealed: matches!(model.state.details, Load::Failed(_)),
                connect_button_clicked => PageMsg::State(Msg::RetryDetails),
            },

            #[wrap(Some)]
            set_content = &adw::BreakpointBin {
                set_width_request: 360,
                set_height_request: 200,
                add_breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
                    adw::BreakpointConditionLengthType::MaxWidth,
                    WIDE_WIDTH_SP,
                    adw::LengthUnit::Sp,
                )) {
                    connect_apply => PageMsg::State(Msg::Narrow(true)),
                    connect_unapply => PageMsg::State(Msg::Narrow(false)),
                },

                #[name = "layouts"]
                #[wrap(Some)]
                set_child = &adw::MultiLayoutView {},
            },
        },

        // The title, channel, description, comments and related videos.
        #[name = "details"]
        gtk::Box {
            set_orientation: gtk::Orientation::Vertical,
            set_spacing: 12,

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
                    Load::Failed(e) => e.clone(),
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

            adw::PreferencesGroup {
                #[watch]
                set_visible: matches!(model.state.details, Load::Loaded(_)),

                adw::ExpanderRow {
                    set_title: "Description",
                    set_use_markup: false,
                    set_subtitle_lines: 1,
                    #[watch]
                    set_subtitle: model.description().lines().next().unwrap_or_default(),
                    #[watch]
                    set_visible: !model.description().is_empty(),

                    add_row = &adw::PreferencesRow {
                        set_activatable: false,

                        #[wrap(Some)]
                        set_child = &gtk::Label {
                            set_wrap: true,
                            set_wrap_mode: gtk::pango::WrapMode::WordChar,
                            set_xalign: 0.0,
                            set_selectable: true,
                            set_margin_all: 12,
                            #[watch]
                            set_label: model.description(),
                        },
                    },
                },

            },

            // Comments and related videos, shown one at a time.
            #[name = "tabs"]
            adw::ToggleGroup {
                set_homogeneous: true,
                #[watch]
                set_visible: matches!(model.state.details, Load::Loaded(_)),
                // Labelled and enabled by `update_comments_tab`.
                add: adw::Toggle::builder().name(Tab::Comments.name()).build(),
                add: adw::Toggle::builder().name(Tab::Related.name()).label("Related Videos").build(),
                #[watch]
                #[block_signal(tab_handler)]
                set_active_name: Some(model.state.tab.name()),
                connect_active_name_notify[sender] => move |tabs| {
                    if let Some(tab) = tabs.active_name().as_deref().and_then(Tab::from_name) {
                        sender.input(PageMsg::State(Msg::SelectTab(tab)));
                    }
                } @tab_handler,
            },

            #[name = "tab_stack"]
            gtk::Stack {
                set_vhomogeneous: false,
                #[watch]
                set_visible: matches!(model.state.details, Load::Loaded(_)),

                #[local_ref]
                add_named[Some(Tab::Comments.name())] = comments -> adw::PreferencesGroup {},
                #[local_ref]
                add_named[Some(Tab::Related.name())] = related -> adw::PreferencesGroup {
                    adw::ActionRow {
                        set_title: "No related videos",
                        #[watch]
                        set_visible: model.related_rows.is_empty(),
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
        let comments = adw::PreferencesGroup::new();
        let player =
            PlayerView::builder()
                .launch(())
                .forward(sender.input_sender(), |out| match out {
                    player_view::Output::Progress(p) => PageMsg::State(Msg::Progress(p)),
                    player_view::Output::Failed {
                        error,
                        position,
                        playing,
                    } => PageMsg::State(Msg::PlaybackFailed {
                        error,
                        position,
                        playing,
                    }),
                    player_view::Output::ToggleFullscreen => PageMsg::State(Msg::ToggleFullscreen),
                    player_view::Output::SelectSubtitles(index) => {
                        PageMsg::State(Msg::SelectSubtitles(index))
                    }
                });
        let mut model = VideoPage {
            state,
            services,
            player,
            related: adw::PreferencesGroup::new(),
            related_rows: Vec::new(),
            comment_list: CommentList::new(
                comments.clone(),
                {
                    let sender = sender.clone();
                    move || sender.input(PageMsg::State(Msg::MoreComments))
                },
                Some({
                    let sender = sender.clone();
                    move |index| sender.input(PageMsg::State(Msg::OpenReplies(index)))
                }),
            ),
            avatar: adw::Avatar::default(),
            window: None,
        };
        let related = &model.related;
        let comments = &comments;
        let avatar = &model.avatar;
        let widgets = view_output!();
        widgets
            .tabs
            .bind_property("active-name", &widgets.tab_stack, "visible-child-name")
            .sync_create()
            .build();
        model.update_comments_tab(&widgets);
        add_actions(&root, &sender);
        add_shortcuts(&root, &sender);
        add_layouts(&widgets.layouts);
        widgets.layouts.set_child("player", model.player.widget());
        widgets.layouts.set_child("details", &widgets.details);
        model.place_player(&widgets);
        model.run(effects, &sender, &root);
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
                let comments = matches!(
                    msg,
                    Msg::Comments(_) | Msg::SelectTab(_) | Msg::MoreComments
                );
                let effects = self.state.update(msg);
                self.run(effects, &sender, root);
                if details {
                    self.fill_related(&sender);
                }
                if comments || details {
                    self.comment_list.update(&self.state.comments);
                    self.update_comments_tab(widgets);
                }
            }
            PageMsg::TogglePlay => self.player.emit(player_view::Msg::TogglePlay),
        }
        self.place_player(widgets);
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
        if self.state.fullscreen
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
    /// Shows the number of comments on their tab, which is disabled when
    /// there are none.
    fn update_comments_tab(&self, widgets: &VideoPageWidgets) {
        if let Some(tab) = widgets.tabs.toggle_by_name(Tab::Comments.name()) {
            tab.set_label(Some(&format::comments_title(self.state.comments.count)));
            tab.set_enabled(self.state.comments.available());
        }
    }

    /// Shows the layout for the current width and fullscreen state.
    fn place_player(&self, widgets: &VideoPageWidgets) {
        widgets.layouts.set_layout_name(self.state.layout());
        self.player.widget().set_vexpand(self.state.fullscreen);
    }

    fn run(
        &mut self,
        effects: Vec<Effect>,
        sender: &ComponentSender<Self>,
        root: &adw::ToolbarView,
    ) {
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
                        let cues = backend::load_subtitles(backend.as_ref(), &url)
                            .await
                            .map_err(|e| e.to_string());
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
                Effect::Play {
                    source,
                    start_at,
                    play,
                } => {
                    self.player.emit(player_view::Msg::Load {
                        source,
                        start_at,
                        play,
                    });
                }
                Effect::PlaybackFailed(e) => self.player.emit(player_view::Msg::Fail(e)),
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
                Effect::Toast(text) => {
                    let _ = sender.output(Output::Toast(text));
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
                Effect::OpenReplies(comment) => {
                    let _ = sender.output(Output::OpenReplies(comment));
                }
                Effect::SetFullscreen(fullscreen) => {
                    self.player
                        .emit(player_view::Msg::SetFullscreen(fullscreen));
                    self.window = root.root().and_downcast::<gtk::Window>();
                    if let Some(window) = &self.window {
                        window.set_fullscreened(fullscreen);
                    }
                }
            }
        }
    }

    fn fill_related(&mut self, sender: &ComponentSender<Self>) {
        for row in self.related_rows.drain(..) {
            self.related.remove(&row);
        }
        if let Load::Loaded(details) = &self.state.details {
            images::set_avatar(&self.avatar, details.channel.avatar_url.as_deref());
            for (index, video) in details.related.iter().enumerate() {
                let row = widgets::with_chevron(widgets::video_row(video));
                let input = sender.input_sender().clone();
                row.connect_activated(move |_| {
                    input.emit(PageMsg::State(Msg::OpenRelated(index)));
                });
                self.related.add(&row);
                self.related_rows.push(row);
            }
        }
    }

    /// The video's description, once loaded.
    fn description(&self) -> &str {
        match &self.state.details {
            Load::Loaded(d) => &d.description,
            _ => "",
        }
    }
}

/// The download options, activating the page's `download` action.
fn download_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some("Download _Video"), Some("page.download::video"));
    menu.append(Some("Download _Audio"), Some("page.download::audio"));
    menu
}

/// Adds the `page` actions to the page.
fn add_actions(root: &adw::ToolbarView, sender: &ComponentSender<VideoPage>) {
    let actions = gio::SimpleActionGroup::new();
    let download = gio::SimpleAction::new("download", Some(gtk::glib::VariantTy::STRING));
    let input = sender.input_sender().clone();
    download.connect_activate(move |_, kind| {
        let kind = match kind.and_then(|k| k.str()) {
            Some("audio") => DownloadKind::Audio,
            _ => DownloadKind::Video,
        };
        input.emit(PageMsg::State(Msg::Download(kind)));
    });
    actions.add_action(&download);
    root.insert_action_group("page", Some(&actions));
}

/// Adds the player's keyboard shortcuts to the page. They are handled after
/// the focused widget, so that e.g. Space still activates a focused button.
fn add_shortcuts(root: &adw::ToolbarView, sender: &ComponentSender<VideoPage>) {
    let controller = gtk::ShortcutController::new();
    let add = |trigger: &str, msg: fn() -> PageMsg| {
        let input = sender.input_sender().clone();
        controller.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string(trigger),
            Some(gtk::CallbackAction::new(move |_, _| {
                input.emit(msg());
                gtk::glib::Propagation::Stop
            })),
        ));
    };
    add("space", || PageMsg::TogglePlay);
    add("f", || PageMsg::State(Msg::ToggleFullscreen));
    add("Escape", || PageMsg::State(Msg::LeaveFullscreen));
    root.add_controller(controller);
}

/// Adds the layouts named by [`layout_name`], with slots for the player and
/// the details.
fn add_layouts(view: &adw::MultiLayoutView) {
    // The details, below `player` if given, in a scrolled clamp.
    let scrolled = |player: Option<&gtk::Widget>| {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        if let Some(player) = player {
            content.append(player);
        }
        content.append(&adw::LayoutSlot::new("details"));
        gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(
                &adw::Clamp::builder()
                    .maximum_size(1000)
                    .margin_top(12)
                    .margin_bottom(12)
                    .margin_start(12)
                    .margin_end(12)
                    .child(&content)
                    .build(),
            )
            .build()
    };
    let layout = |name: &str, content: &gtk::Widget| {
        let layout = adw::Layout::new(content);
        layout.set_name(Some(name));
        view.add_layout(layout);
    };

    // The player scrolls with the details, in a card.
    let card = gtk::Box::builder()
        .css_classes(["card"])
        .overflow(gtk::Overflow::Hidden)
        .build();
    card.append(&adw::LayoutSlot::new("player"));
    layout("wide", scrolled(Some(card.upcast_ref())).upcast_ref());

    // The player stays above the scrolled details.
    let narrow = gtk::Box::new(gtk::Orientation::Vertical, 0);
    narrow.append(&adw::LayoutSlot::new("player"));
    narrow.append(&scrolled(None));
    layout("narrow", narrow.upcast_ref());

    layout("fullscreen", adw::LayoutSlot::new("player").upcast_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{AudioStream, Comment, CommentPage, SubtitleTrack, VideoStream};
    use crate::core::subtitles::Cue;

    #[test]
    fn player_layout() {
        assert_eq!(layout_name(false, false), "wide");
        assert_eq!(layout_name(true, false), "narrow");
        assert_eq!(layout_name(false, true), "fullscreen");
        assert_eq!(layout_name(true, true), "fullscreen");
    }

    #[test]
    fn fullscreen_is_toggled_and_left() {
        let (mut state, _) = State::new(init());
        assert!(
            state.update(Msg::LeaveFullscreen).is_empty(),
            "not fullscreen"
        );
        assert_eq!(state.layout(), "wide");

        assert_eq!(
            state.update(Msg::ToggleFullscreen),
            [Effect::SetFullscreen(true)]
        );
        assert!(state.fullscreen);
        assert_eq!(state.layout(), "fullscreen");

        assert_eq!(
            state.update(Msg::LeaveFullscreen),
            [Effect::SetFullscreen(false)]
        );
        assert!(!state.fullscreen);
        assert_eq!(
            state.update(Msg::ToggleFullscreen),
            [Effect::SetFullscreen(true)]
        );
        assert_eq!(
            state.update(Msg::ToggleFullscreen),
            [Effect::SetFullscreen(false)]
        );
    }

    #[test]
    fn narrow_pages_fix_the_player_above_the_details() {
        let (mut state, _) = State::new(init());
        assert!(state.update(Msg::Narrow(true)).is_empty());
        assert_eq!(state.layout(), "narrow");
        state.update(Msg::ToggleFullscreen);
        assert_eq!(state.layout(), "fullscreen");
        state.update(Msg::ToggleFullscreen);
        state.update(Msg::Narrow(false));
        assert_eq!(state.layout(), "wide");
    }

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
            local_subtitles: vec![],
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
                    start_at: 60,
                    play: false,
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
    fn downloaded_video_offers_saved_subtitles() {
        let (state, effects) = State::new(Init {
            local_file: Some("/videos/v.mp4".into()),
            local_subtitles: vec![track("en"), track("de")],
            subtitle_language: Some("de".into()),
            ..init()
        });
        assert_eq!(
            effects[3..],
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
                    start_at: 42,
                    play: false,
                },
                Effect::RecordWatch {
                    video: video("v"),
                    position: 42
                },
            ]
        );
        assert_eq!(state.playback, Load::Loaded(source));
    }

    fn failed(position: u32, playing: bool) -> Msg {
        Msg::PlaybackFailed {
            error: "decoding failed".into(),
            position,
            playing,
        }
    }

    #[test]
    fn playback_errors_reload_fresh_streams_at_the_position() {
        let (mut state, _) = State::new(init());
        let streams = Streams {
            subtitles: vec![track("en")],
            ..muxed()
        };
        state.update(Msg::Streams(Ok(streams.clone())));

        assert_eq!(
            state.update(failed(300, true)),
            [Effect::LoadStreams("v".into())]
        );
        assert_eq!(state.playback, Load::Loading);
        // The subtitle tracks are offered already.
        assert_eq!(
            state.update(Msg::Streams(Ok(streams))),
            [
                Effect::Play {
                    source: PlaybackSource::Uri("https://muxed".into()),
                    start_at: 300,
                    play: true,
                },
                Effect::RecordWatch {
                    video: video("v"),
                    position: 300
                },
            ]
        );
    }

    #[test]
    fn downloaded_video_reloads_the_file_after_an_error() {
        let (mut state, _) = State::new(Init {
            local_file: Some("/videos/v.mp4".into()),
            ..init()
        });
        let effects = state.update(failed(60, false));
        assert_eq!(
            effects[0],
            Effect::Play {
                source: PlaybackSource::Uri("file:///videos/v.mp4".into()),
                start_at: 60,
                play: false,
            }
        );
    }

    #[test]
    fn repeated_playback_errors_fail() {
        let (mut state, _) = State::new(init());
        state.update(Msg::Streams(Ok(muxed())));
        for _ in 0..MAX_RECOVERIES {
            assert_eq!(
                state.update(failed(100, true))[0],
                Effect::LoadStreams("v".into())
            );
            state.update(Msg::Streams(Ok(muxed())));
            state.update(Msg::Progress(105));
        }
        assert_eq!(
            state.update(failed(106, true)),
            [Effect::PlaybackFailed("decoding failed".into())]
        );
        assert_eq!(state.playback, Load::Failed("decoding failed".into()));
    }

    #[test]
    fn playing_on_past_an_error_allows_new_recoveries() {
        let (mut state, _) = State::new(init());
        state.update(Msg::Streams(Ok(muxed())));
        for position in [100, 200, 300] {
            assert_eq!(
                state.update(failed(position, true))[0],
                Effect::LoadStreams("v".into())
            );
            state.update(Msg::Streams(Ok(muxed())));
            state.update(Msg::Progress(position + RECOVERED_AFTER_SECS));
        }
    }

    #[test]
    fn unplayable_streams_fail() {
        let (mut state, _) = State::new(init());
        let none = "No playable stream found".to_owned();
        assert_eq!(
            state.update(Msg::Streams(Ok(Streams::default()))),
            [Effect::PlaybackFailed(none.clone())]
        );
        assert_eq!(state.playback, Load::Failed(none));

        assert_eq!(
            state.update(Msg::Streams(Err("blocked".into()))),
            [Effect::PlaybackFailed("blocked".into())]
        );
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
    fn failed_details_can_be_retried() {
        let (mut state, _) = State::new(init());
        assert!(state.update(Msg::RetryDetails).is_empty(), "still loading");

        state.update(Msg::Details(Err("x".into())));
        assert_eq!(
            state.update(Msg::RetryDetails),
            [Effect::LoadDetails("v".into())]
        );
        assert_eq!(state.details, Load::Loading);
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
            [
                Effect::Subscribe(channel()),
                Effect::Toast("Subscribed to “Chan”".into())
            ]
        );
        assert!(state.subscribed);
        assert_eq!(
            state.update(Msg::ToggleSubscribe),
            [
                Effect::Unsubscribe("UC1".into()),
                Effect::Toast("Unsubscribed from “Chan”".into())
            ]
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
            replies: None,
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
    fn details_load_first_comment_page_once() {
        let (mut state, _) = State::new(init());
        let mut d = details();
        d.comments = page(&[], Some("p1"));
        assert_eq!(
            state.update(Msg::Details(Ok(d))),
            [Effect::LoadComments("p1".into())]
        );
        assert_eq!(state.comments.count, Some(3));
        assert!(state.comments.loading);
        assert!(
            state.update(Msg::SelectTab(Tab::Comments)).is_empty(),
            "already loading"
        );

        state.update(Msg::Comments(Ok(page(&["a", "b"], Some("p2")))));
        assert!(!state.comments.loading);
        assert_eq!(state.comments.items, [comment("a"), comment("b")]);
        assert!(
            state.update(Msg::SelectTab(Tab::Comments)).is_empty(),
            "already loaded"
        );
    }

    #[test]
    fn more_comments_are_appended_until_last_page() {
        let mut state = with_comments();
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
    fn opens_replies_of_comments_that_have_some() {
        let mut state = with_comments();
        let with_replies = Comment {
            reply_count: 2,
            replies: Some("r".into()),
            ..comment("b")
        };
        state.update(Msg::Comments(Ok(CommentPage {
            comments: vec![comment("a"), with_replies.clone()],
            ..page(&[], None)
        })));

        assert_eq!(
            state.update(Msg::OpenReplies(1)),
            [Effect::OpenReplies(with_replies)]
        );
        assert!(state.update(Msg::OpenReplies(0)).is_empty(), "no replies");
        assert!(state.update(Msg::OpenReplies(2)).is_empty());
    }

    #[test]
    fn failed_comments_can_be_retried() {
        let mut state = with_comments();
        state.update(Msg::Comments(Err("offline".into())));
        assert_eq!(state.comments.error.as_deref(), Some("offline"));
        assert!(!state.comments.loading);

        assert_eq!(
            state.update(Msg::SelectTab(Tab::Comments)),
            [Effect::LoadComments("p1".into())]
        );
        assert_eq!(state.comments.error, None);
    }

    #[test]
    fn details_show_the_comments_tab_if_there_are_comments() {
        let state = with_comments();
        assert_eq!(state.tab, Tab::Comments);

        let (mut state, _) = State::new(init());
        state.update(Msg::Details(Ok(details())));
        assert_eq!(state.tab, Tab::Related);
    }

    #[test]
    fn switching_tabs_keeps_the_loaded_comments() {
        let mut state = with_comments();
        state.update(Msg::Comments(Ok(page(&["a"], Some("p2")))));
        assert!(state.update(Msg::SelectTab(Tab::Related)).is_empty());
        assert_eq!(state.tab, Tab::Related);
        assert!(state.update(Msg::SelectTab(Tab::Comments)).is_empty());
        assert_eq!(state.tab, Tab::Comments);
        assert_eq!(state.comments.items, [comment("a")]);
    }

    #[test]
    fn comments_need_details() {
        let (mut state, _) = State::new(init());
        assert!(state.update(Msg::SelectTab(Tab::Comments)).is_empty());
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
            placement: Default::default(),
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
    fn auto_generated_subtitles_are_shown_unplaced() {
        let (mut state, _) = State::new(init());
        state.update(Msg::Streams(Ok(Streams {
            subtitles: vec![
                track("en"),
                SubtitleTrack {
                    auto_generated: true,
                    ..track("de")
                },
            ],
            ..muxed()
        })));
        let placed = || {
            let vtt = "WEBVTT\n\n00:00.000 --> 00:01.000 align:start position:0%\nHi\n";
            subtitles::parse_vtt(vtt)
        };

        state.update(Msg::SelectSubtitles(Some(0)));
        assert_eq!(
            state.update(Msg::Subtitles(0, Ok(placed()))),
            [Effect::ShowSubtitles(placed())]
        );
        state.update(Msg::SelectSubtitles(Some(1)));
        assert_eq!(
            state.update(Msg::Subtitles(1, Ok(placed()))),
            [Effect::ShowSubtitles(cues())]
        );
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
            [
                Effect::ShowSubtitles(Vec::new()),
                Effect::SubtitlesFailed("HTTP 429".into())
            ]
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
