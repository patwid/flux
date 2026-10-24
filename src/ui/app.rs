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
                #[name = "layouts"]
                adw::MultiLayoutView {},
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

        let stack = adw::ViewStack::new();
        let placeholder = |title: &str, icon: &str| {
            adw::StatusPage::builder()
                .title(title)
                .icon_name(icon)
                .build()
        };
        let add_page = |name: &str, title: &str, icon: &str, child: &gtk::Widget| {
            stack.add_titled_with_icon(child, Some(name), title, icon);
        };
        add_page(
            "feed",
            "Feed",
            "view-list-symbolic",
            placeholder("Feed", "view-list-symbolic").upcast_ref(),
        );
        add_page(
            "search",
            "Search",
            "system-search-symbolic",
            model.search.widget().upcast_ref(),
        );
        for (name, title, icon) in [
            ("subscriptions", "Subscriptions", "starred-symbolic"),
            ("history", "History", "document-open-recent-symbolic"),
            ("downloads", "Downloads", "folder-download-symbolic"),
        ] {
            add_page(name, title, icon, placeholder(title, icon).upcast_ref());
        }

        widgets.layouts.add_layout(wide_layout(&stack));
        widgets.layouts.add_layout(narrow_layout(&stack));
        widgets.layouts.set_child("content", &stack);
        widgets.layouts.set_layout_name("wide");

        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            NARROW_WIDTH_SP,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&widgets.layouts, "layout-name", Some(&"narrow".to_value()));
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

/// Desktop layout: sections in a sidebar next to the content.
fn wide_layout(stack: &adw::ViewStack) -> adw::Layout {
    let sidebar = adw::ToolbarView::new();
    sidebar.add_top_bar(
        &adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .title_widget(&adw::WindowTitle::new("Flux", ""))
            .build(),
    );
    sidebar.set_content(Some(
        &adw::ViewSwitcherSidebar::builder().stack(stack).build(),
    ));

    let content = adw::ToolbarView::new();
    let title = adw::WindowTitle::new("", "");
    stack
        .bind_property("visible-child-name", &title, "title")
        .transform_to(|binding, _: Option<String>| {
            let stack = binding.source()?.downcast::<adw::ViewStack>().ok()?;
            stack.page(&stack.visible_child()?).title()
        })
        .sync_create()
        .build();
    content.add_top_bar(&adw::HeaderBar::builder().title_widget(&title).build());
    content.set_content(Some(&adw::LayoutSlot::new("content")));

    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar)
        .content(&content)
        .build();
    let layout = adw::Layout::new(&split);
    layout.set_name(Some("wide"));
    layout
}

/// Mobile layout: sections in a bottom bar.
fn narrow_layout(stack: &adw::ViewStack) -> adw::Layout {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&adw::LayoutSlot::new("content")));
    view.add_bottom_bar(
        &adw::ViewSwitcherBar::builder()
            .stack(stack)
            .reveal(true)
            .build(),
    );
    let layout = adw::Layout::new(&view);
    layout.set_name(Some("narrow"));
    layout
}
