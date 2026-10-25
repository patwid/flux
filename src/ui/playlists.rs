//! Playlists page: the playlists saved to the library.

use relm4::adw::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use super::{Services, log_db_error, widgets};
use crate::core::model::Playlist;

pub struct PlaylistsPage {
    services: Services,
    playlists: Vec<Playlist>,
    list: gtk::ListBox,
}

#[derive(Debug)]
pub enum Msg {
    /// Re-reads the saved playlists from the database.
    Reload,
    Activate(usize),
}

#[relm4::component(pub)]
impl Component for PlaylistsPage {
    type Init = Services;
    type Input = Msg;
    type Output = Playlist;
    type CommandOutput = ();

    view! {
        adw::ToolbarView {
            add_top_bar = &adw::HeaderBar {
                pack_end: &widgets::primary_menu(),
                #[wrap(Some)]
                set_title_widget = &adw::WindowTitle {
                    set_title: "Playlists",
                },
            },

            #[wrap(Some)]
            set_content = &gtk::Stack {
                add_named[Some("empty")] = &adw::StatusPage {
                    set_icon_name: Some("view-list-bullet-symbolic"),
                    set_title: "No Saved Playlists",
                    set_description: Some("Playlists you save appear here"),
                },
                add_named[Some("list")] = &gtk::ScrolledWindow {
                    set_hscrollbar_policy: gtk::PolicyType::Never,

                    adw::Clamp {
                        set_maximum_size: 800,
                        set_margin_all: 12,

                        #[local_ref]
                        list -> gtk::ListBox {
                            connect_row_activated[sender] => move |_, row| {
                                sender.input(Msg::Activate(row.index() as usize));
                            },
                        },
                    },
                },

                #[watch]
                set_visible_child_name: if model.playlists.is_empty() { "empty" } else { "list" },
            },
        }
    }

    fn init(
        services: Services,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = PlaylistsPage {
            services,
            playlists: Vec::new(),
            list: widgets::boxed_list(),
        };
        let list = &model.list;
        let widgets = view_output!();
        sender.input(Msg::Reload);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::Reload => {
                self.playlists =
                    log_db_error(self.services.db.saved_playlists()).unwrap_or_default();
                self.list.remove_all();
                for playlist in &self.playlists {
                    self.list.append(&widgets::playlist_row(playlist));
                }
            }
            Msg::Activate(index) => {
                if let Some(playlist) = self.playlists.get(index) {
                    let _ = sender.output(playlist.clone());
                }
            }
        }
    }
}
