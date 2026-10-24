use std::sync::Arc;

use relm4::adw::prelude::*;
use relm4::{
    Component, ComponentController, ComponentParts, ComponentSender, Controller, adw, gtk,
};

use super::search::{self, SearchPage};
use crate::core::backend::Backend;

/// Windows narrower than this use the mobile layout (bottom view switcher).
const NARROW_WIDTH_SP: f64 = 550.0;

pub struct App {
    search: Controller<SearchPage>,
    toasts: adw::ToastOverlay,
}

#[derive(Debug)]
pub enum Msg {
    Search(search::Output),
}

#[relm4::component(pub)]
impl Component for App {
    type Init = Arc<dyn Backend>;
    type Input = Msg;
    type Output = ();
    type CommandOutput = ();

    view! {
        adw::ApplicationWindow {
            set_title: Some("Flux"),
            set_default_width: 360,
            set_default_height: 720,
            set_width_request: 360,
            set_height_request: 294,

            #[local_ref]
            toasts -> adw::ToastOverlay {
                adw::ToolbarView {
                    #[name = "header"]
                    add_top_bar = &adw::HeaderBar {
                        #[wrap(Some)]
                        #[name = "switcher"]
                        set_title_widget = &adw::ViewSwitcher {
                            set_policy: adw::ViewSwitcherPolicy::Wide,
                            set_stack: Some(&stack),
                        },
                    },

                    #[wrap(Some)]
                    #[name = "stack"]
                    set_content = &adw::ViewStack {
                        add_titled_with_icon[Some("feed"), "Feed", "view-list-symbolic"] = &adw::StatusPage {
                            set_icon_name: Some("view-list-symbolic"),
                            set_title: "Feed",
                            set_description: Some("New videos from your subscriptions"),
                        },
                        add_titled_with_icon[Some("search"), "Search", "system-search-symbolic"] = model.search.widget(),
                        add_titled_with_icon[Some("subscriptions"), "Subscriptions", "starred-symbolic"] = &adw::StatusPage {
                            set_icon_name: Some("starred-symbolic"),
                            set_title: "Subscriptions",
                        },
                        add_titled_with_icon[Some("history"), "History", "document-open-recent-symbolic"] = &adw::StatusPage {
                            set_icon_name: Some("document-open-recent-symbolic"),
                            set_title: "History",
                        },
                        add_titled_with_icon[Some("downloads"), "Downloads", "folder-download-symbolic"] = &adw::StatusPage {
                            set_icon_name: Some("folder-download-symbolic"),
                            set_title: "Downloads",
                        },
                    },

                    #[name = "switcher_bar"]
                    add_bottom_bar = &adw::ViewSwitcherBar {
                        set_stack: Some(&stack),
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
        let search = SearchPage::builder()
            .launch(backend)
            .forward(sender.input_sender(), Msg::Search);
        let model = App {
            search,
            toasts: adw::ToastOverlay::new(),
        };
        let toasts = &model.toasts;
        let widgets = view_output!();

        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            NARROW_WIDTH_SP,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&widgets.switcher_bar, "reveal", Some(&true.to_value()));
        narrow.add_setter(
            &widgets.header,
            "title-widget",
            Some(&None::<gtk::Widget>.to_value()),
        );
        root.add_breakpoint(narrow);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, _sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::Search(search::Output::OpenVideo(v)) => {
                self.toasts.add_toast(adw::Toast::new(&format!(
                    "Playback not implemented yet: {}",
                    v.title
                )));
            }
            Msg::Search(search::Output::OpenChannel(c)) => {
                self.toasts.add_toast(adw::Toast::new(&format!(
                    "Channels not implemented yet: {}",
                    c.name
                )));
            }
        }
    }
}
