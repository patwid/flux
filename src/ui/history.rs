//! History page: previously watched videos.

use relm4::adw::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use super::{Services, format, log_db_error, widgets};
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
    /// Puts back an entry removed before.
    Restore(HistoryEntry),
    /// Clear the whole history (after the user confirmed).
    Clear,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    OpenVideo(Video),
    Remove(HistoryEntry),
    Restore(HistoryEntry),
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
                    return vec![Effect::Remove(self.entries.remove(index))];
                }
            }
            Msg::Restore(entry) => {
                // Most recently watched first.
                let index = self
                    .entries
                    .partition_point(|e| e.watched_at >= entry.watched_at);
                self.entries.insert(index, entry.clone());
                return vec![Effect::Restore(entry)];
            }
            Msg::Clear => {
                self.entries.clear();
                return vec![Effect::Clear];
            }
        }
        Vec::new()
    }
}

pub struct HistoryPage {
    state: State,
    services: Services,
    list: gtk::ListBox,
}

#[derive(Debug)]
pub enum Input {
    State(Msg),
    /// Re-reads the history from the database.
    Reload,
    /// Asks the user to confirm clearing the history.
    ConfirmClear,
}

#[derive(Debug)]
pub enum Output {
    OpenVideo(Video),
    /// An entry was removed; it can be restored with [`Msg::Restore`].
    Removed(HistoryEntry),
}

#[relm4::component(pub)]
impl Component for HistoryPage {
    type Init = Services;
    type Input = Input;
    type Output = Output;
    type CommandOutput = ();

    view! {
        adw::ToolbarView {
            add_top_bar = &adw::HeaderBar {
                pack_end: &widgets::primary_menu(),
                #[wrap(Some)]
                set_title_widget = &adw::WindowTitle {
                    set_title: "History",
                },
                pack_end = &gtk::Button {
                    set_icon_name: "edit-clear-all-symbolic",
                    set_tooltip_text: Some("Clear History"),
                    #[watch]
                    set_visible: !model.state.entries.is_empty(),
                    connect_clicked => Input::ConfirmClear,
                },
            },

            #[wrap(Some)]
            set_content = &gtk::Stack {
                add_named[Some("empty")] = &adw::StatusPage {
                    set_icon_name: Some("document-open-recent-symbolic"),
                    set_title: "No History",
                    set_description: Some("Videos you watch appear here"),
                },
                add_named[Some("list")] = &gtk::ScrolledWindow {
                    set_hscrollbar_policy: gtk::PolicyType::Never,

                    adw::Clamp {
                        set_maximum_size: 800,
                        set_margin_all: 12,

                        #[local_ref]
                        list -> gtk::ListBox {
                            connect_row_activated[sender] => move |_, row| {
                                sender.input(Input::State(Msg::Activate(row.index() as usize)));
                            },
                        },
                    },
                },

                #[watch]
                set_visible_child_name: if model.state.entries.is_empty() { "empty" } else { "list" },
            },
        }
    }

    fn init(
        services: Services,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = HistoryPage {
            state: State::default(),
            services,
            list: widgets::card_list(),
        };
        let list = &model.list;
        let widgets = view_output!();
        sender.input(Input::Reload);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, input: Input, sender: ComponentSender<Self>, root: &Self::Root) {
        let msg = match input {
            Input::State(msg) => msg,
            Input::Reload => {
                Msg::Load(log_db_error(self.services.db.history()).unwrap_or_default())
            }
            Input::ConfirmClear => {
                let dialog = adw::AlertDialog::new(
                    Some("Clear History?"),
                    Some("All watched videos and resume positions will be forgotten."),
                );
                dialog.add_responses(&[("cancel", "_Cancel"), ("clear", "_Clear")]);
                dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
                dialog.set_close_response("cancel");
                let input = sender.input_sender().clone();
                dialog.connect_response(Some("clear"), move |_, _| {
                    let _ = input.send(Input::State(Msg::Clear));
                });
                dialog.present(Some(root));
                return;
            }
        };
        for effect in self.state.update(msg) {
            match effect {
                Effect::OpenVideo(video) => {
                    let _ = sender.output(Output::OpenVideo(video));
                }
                Effect::Remove(entry) => {
                    log_db_error(self.services.db.remove_from_history(&entry.video.id));
                    let _ = sender.output(Output::Removed(entry));
                }
                Effect::Restore(entry) => {
                    log_db_error(self.services.db.record_watch(
                        &entry.video,
                        entry.position_secs,
                        entry.watched_at,
                    ));
                }
                Effect::Clear => {
                    log_db_error(self.services.db.clear_history());
                }
            }
        }
        self.fill(&sender);
    }
}

impl HistoryPage {
    fn fill(&self, sender: &ComponentSender<Self>) {
        self.list.remove_all();
        let now = chrono::Utc::now();
        for (index, entry) in self.state.entries.iter().enumerate() {
            let row = widgets::compact_video_row(&entry.video);
            row.set_subtitle(&format::history_subtitle(entry, now));
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove From History")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let input = sender.input_sender().clone();
            remove.connect_clicked(move |_| {
                let _ = input.send(Input::State(Msg::Remove(index)));
            });
            row.add_suffix(&remove);
            self.list.append(&row);
        }
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
        assert_eq!(state.update(Msg::Remove(0)), [Effect::Remove(entry("a"))]);
        assert_eq!(state.entries, [entry("b")]);
        assert!(state.update(Msg::Remove(5)).is_empty());
    }

    #[test]
    fn removed_entry_can_be_restored_in_place() {
        let at = |secs| DateTime::from_timestamp(secs, 0).unwrap();
        let entries: Vec<_> = [("a", 3), ("b", 2), ("c", 1)]
            .into_iter()
            .map(|(id, secs)| HistoryEntry {
                watched_at: at(secs),
                ..entry(id)
            })
            .collect();
        let mut state = State::default();
        state.update(Msg::Load(entries.clone()));
        state.update(Msg::Remove(1));

        assert_eq!(
            state.update(Msg::Restore(entries[1].clone())),
            [Effect::Restore(entries[1].clone())]
        );
        assert_eq!(state.entries, entries);
    }

    #[test]
    fn clearing_history() {
        let mut state = loaded();
        assert_eq!(state.update(Msg::Clear), [Effect::Clear]);
        assert!(state.entries.is_empty());
    }
}
