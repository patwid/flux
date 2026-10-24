//! Search page.

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
