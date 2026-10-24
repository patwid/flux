use relm4::adw::prelude::*;
use relm4::{ComponentParts, ComponentSender, SimpleComponent, adw};

pub struct App;

#[relm4::component(pub)]
impl SimpleComponent for App {
    type Init = ();
    type Input = ();
    type Output = ();

    view! {
        adw::ApplicationWindow {
            set_title: Some("Flux"),
            set_default_width: 360,
            set_default_height: 720,

            adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {},

                #[wrap(Some)]
                set_content = &adw::StatusPage {
                    set_icon_name: Some("video-x-generic-symbolic"),
                    set_title: "Flux",
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = App;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }
}
