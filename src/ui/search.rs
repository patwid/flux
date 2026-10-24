//! Search page.

use std::sync::Arc;

use relm4::adw::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use super::format;
use crate::core::backend::Backend;
use crate::core::model::{Channel, SearchItem, Video};

/// What the search page shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Idle,
    Loading,
    Loaded(Vec<SearchItem>),
    Failed(String),
}

/// GTK-independent state of the search page.
#[derive(Debug, Default)]
pub struct State {
    pub query: String,
    pub status: Status,
    /// Id of the latest search request; responses to older ones are dropped.
    request: u64,
}

#[derive(Debug)]
pub enum Msg {
    QueryChanged(String),
    Submit,
    Results {
        request: u64,
        result: Result<Vec<SearchItem>, String>,
    },
    /// The result at this index was activated.
    Activate(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Search { request: u64, query: String },
    OpenVideo(Video),
    OpenChannel(Channel),
}

impl State {
    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::QueryChanged(query) => self.query = query,
            Msg::Submit => {
                let query = self.query.trim();
                if !query.is_empty() {
                    self.request += 1;
                    self.status = Status::Loading;
                    return vec![Effect::Search {
                        request: self.request,
                        query: query.to_owned(),
                    }];
                }
            }
            Msg::Results { request, result } if request == self.request => {
                self.status = match result {
                    Ok(items) => Status::Loaded(items),
                    Err(e) => Status::Failed(e),
                };
            }
            Msg::Results { .. } => {}
            Msg::Activate(index) => {
                if let Status::Loaded(items) = &self.status {
                    match items.get(index) {
                        Some(SearchItem::Video(v)) => return vec![Effect::OpenVideo(v.clone())],
                        Some(SearchItem::Channel(c)) => {
                            return vec![Effect::OpenChannel(c.clone())];
                        }
                        None => {}
                    }
                }
            }
        }
        Vec::new()
    }
}

pub struct SearchPage {
    state: State,
    backend: Arc<dyn Backend>,
    results: gtk::ListBox,
}

#[derive(Debug)]
pub enum Output {
    OpenVideo(Video),
    OpenChannel(Channel),
}

#[relm4::component(pub)]
impl Component for SearchPage {
    type Init = Arc<dyn Backend>;
    type Input = Msg;
    type Output = Output;
    type CommandOutput = Msg;

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Vertical,

            adw::Clamp {
                set_maximum_size: 800,
                set_margin_all: 12,

                gtk::SearchEntry {
                    set_placeholder_text: Some("Search videos and channels"),
                    connect_search_changed[sender] => move |e| {
                        sender.input(Msg::QueryChanged(e.text().into()));
                    },
                    connect_activate => Msg::Submit,
                },
            },

            gtk::Stack {
                set_vexpand: true,
                #[watch]
                set_visible_child_name: match &model.state.status {
                    Status::Idle => "idle",
                    Status::Loading => "loading",
                    Status::Loaded(items) if items.is_empty() => "empty",
                    Status::Loaded(_) => "results",
                    Status::Failed(_) => "failed",
                },

                add_named[Some("idle")] = &adw::StatusPage {
                    set_icon_name: Some("system-search-symbolic"),
                    set_title: "Search",
                    set_description: Some("Find videos and channels"),
                },
                add_named[Some("loading")] = &gtk::Spinner {
                    set_spinning: true,
                    set_size_request: (32, 32),
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::Center,
                },
                add_named[Some("empty")] = &adw::StatusPage {
                    set_icon_name: Some("system-search-symbolic"),
                    set_title: "No Results",
                },
                add_named[Some("failed")] = &adw::StatusPage {
                    set_icon_name: Some("dialog-error-symbolic"),
                    set_title: "Search Failed",
                    #[watch]
                    set_description: match &model.state.status {
                        Status::Failed(e) => Some(e),
                        _ => None,
                    },
                },
                add_named[Some("results")] = &gtk::ScrolledWindow {
                    set_hscrollbar_policy: gtk::PolicyType::Never,

                    adw::Clamp {
                        set_maximum_size: 800,
                        set_margin_all: 12,

                        #[local_ref]
                        results -> gtk::ListBox {
                            add_css_class: "boxed-list",
                            set_selection_mode: gtk::SelectionMode::None,
                            set_valign: gtk::Align::Start,
                            connect_row_activated[sender] => move |_, row| {
                                sender.input(Msg::Activate(row.index() as usize));
                            },
                        },
                    },
                },
            },
        }
    }

    fn init(
        backend: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = SearchPage {
            state: State::default(),
            backend,
            results: gtk::ListBox::new(),
        };
        let results = &model.results;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        let reload = matches!(msg, Msg::Results { .. });
        for effect in self.state.update(msg) {
            match effect {
                Effect::Search { request, query } => {
                    let backend = self.backend.clone();
                    sender.oneshot_command(async move {
                        let result = backend.search(&query).await.map_err(|e| e.to_string());
                        Msg::Results { request, result }
                    });
                }
                Effect::OpenVideo(v) => sender.output(Output::OpenVideo(v)).unwrap_or_default(),
                Effect::OpenChannel(c) => {
                    sender.output(Output::OpenChannel(c)).unwrap_or_default();
                }
            }
        }
        if reload {
            self.fill_results();
        }
    }

    fn update_cmd(&mut self, msg: Msg, sender: ComponentSender<Self>, root: &Self::Root) {
        self.update(msg, sender, root);
    }
}

impl SearchPage {
    fn fill_results(&self) {
        self.results.remove_all();
        let Status::Loaded(items) = &self.state.status else {
            return;
        };
        for item in items {
            let row = adw::ActionRow::builder()
                .activatable(true)
                .use_markup(false)
                .title_lines(2);
            let row = match item {
                SearchItem::Video(v) => row
                    .title(&v.title)
                    .subtitle(format::video_subtitle(v))
                    .build(),
                SearchItem::Channel(c) => {
                    let row = row.title(&c.name).subtitle("Channel").build();
                    row.add_prefix(&gtk::Image::from_icon_name("avatar-default-symbolic"));
                    row
                }
            };
            self.results.append(&row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: &str) -> Video {
        Video {
            id: id.into(),
            title: id.into(),
            channel_id: "UC1".into(),
            channel_name: "Chan".into(),
            published: None,
            duration_secs: None,
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

    fn searched(state: &mut State, query: &str) -> u64 {
        state.update(Msg::QueryChanged(query.into()));
        match state.update(Msg::Submit).as_slice() {
            [Effect::Search { request, .. }] => *request,
            other => panic!("expected search effect, got {other:?}"),
        }
    }

    #[test]
    fn submit_starts_search_with_trimmed_query() {
        let mut state = State::default();
        state.update(Msg::QueryChanged("  rust  ".into()));

        let effects = state.update(Msg::Submit);

        assert!(matches!(effects.as_slice(), [Effect::Search { query, .. }] if query == "rust"));
        assert_eq!(state.status, Status::Loading);
    }

    #[test]
    fn submit_with_blank_query_does_nothing() {
        let mut state = State::default();
        state.update(Msg::QueryChanged("   ".into()));

        assert!(state.update(Msg::Submit).is_empty());
        assert_eq!(state.status, Status::Idle);
    }

    #[test]
    fn results_are_shown() {
        let mut state = State::default();
        let request = searched(&mut state, "rust");
        let items = vec![SearchItem::Video(video("a"))];

        state.update(Msg::Results {
            request,
            result: Ok(items.clone()),
        });

        assert_eq!(state.status, Status::Loaded(items));
    }

    #[test]
    fn errors_are_shown() {
        let mut state = State::default();
        let request = searched(&mut state, "rust");

        state.update(Msg::Results {
            request,
            result: Err("offline".into()),
        });

        assert_eq!(state.status, Status::Failed("offline".into()));
    }

    #[test]
    fn stale_results_are_ignored() {
        let mut state = State::default();
        let first = searched(&mut state, "first");
        let second = searched(&mut state, "second");
        assert_ne!(first, second);

        state.update(Msg::Results {
            request: first,
            result: Ok(vec![]),
        });
        assert_eq!(state.status, Status::Loading);

        state.update(Msg::Results {
            request: second,
            result: Ok(vec![]),
        });
        assert_eq!(state.status, Status::Loaded(vec![]));
    }

    #[test]
    fn activating_results_opens_them() {
        let mut state = State::default();
        let request = searched(&mut state, "rust");
        state.update(Msg::Results {
            request,
            result: Ok(vec![
                SearchItem::Video(video("a")),
                SearchItem::Channel(channel()),
            ]),
        });

        assert_eq!(
            state.update(Msg::Activate(0)),
            [Effect::OpenVideo(video("a"))]
        );
        assert_eq!(
            state.update(Msg::Activate(1)),
            [Effect::OpenChannel(channel())]
        );
        assert!(state.update(Msg::Activate(2)).is_empty());
    }
}
