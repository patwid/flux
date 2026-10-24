//! History page: previously watched videos.

use crate::core::model::{HistoryEntry, Video};

/// GTK-independent state of the history page.
#[derive(Debug, Default)]
pub struct State {
    pub entries: Vec<HistoryEntry>,
}

#[derive(Debug)]
pub enum Msg {
    Load(Vec<HistoryEntry>),
    Activate(usize),
    Remove(usize),
    /// Clear the whole history (after the user confirmed).
    Clear,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    OpenVideo(Video),
    Remove(String),
    Clear,
}

impl State {
    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Load(entries) => self.entries = entries,
            Msg::Activate(index) => {
                if let Some(entry) = self.entries.get(index) {
                    return vec![Effect::OpenVideo(entry.video.clone())];
                }
            }
            Msg::Remove(index) => {
                if index < self.entries.len() {
                    let entry = self.entries.remove(index);
                    return vec![Effect::Remove(entry.video.id)];
                }
            }
            Msg::Clear => {
                self.entries.clear();
                return vec![Effect::Clear];
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;

    fn entry(id: &str) -> HistoryEntry {
        HistoryEntry {
            video: Video {
                id: id.into(),
                title: id.into(),
                channel_id: "UC1".into(),
                channel_name: "Chan".into(),
                published: None,
                duration_secs: Some(600),
                thumbnail_url: None,
            },
            watched_at: DateTime::from_timestamp(0, 0).unwrap(),
            position_secs: 30,
        }
    }

    fn loaded() -> State {
        let mut state = State::default();
        state.update(Msg::Load(vec![entry("a"), entry("b")]));
        state
    }

    #[test]
    fn activating_opens_video() {
        let mut state = loaded();
        assert_eq!(
            state.update(Msg::Activate(1)),
            [Effect::OpenVideo(entry("b").video)]
        );
        assert!(state.update(Msg::Activate(2)).is_empty());
    }

    #[test]
    fn removing_entry() {
        let mut state = loaded();
        assert_eq!(state.update(Msg::Remove(0)), [Effect::Remove("a".into())]);
        assert_eq!(state.entries, [entry("b")]);
        assert!(state.update(Msg::Remove(5)).is_empty());
    }

    #[test]
    fn clearing_history() {
        let mut state = loaded();
        assert_eq!(state.update(Msg::Clear), [Effect::Clear]);
        assert!(state.entries.is_empty());
    }
}
