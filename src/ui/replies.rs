//! Replies page: a comment and the replies to it.

use relm4::adw::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use super::comments::{self, CommentList, Comments, Rows};
use super::{Services, format};
use crate::core::model::{Comment, CommentPage};

/// GTK-independent state of a replies page.
#[derive(Debug)]
pub struct State {
    pub comment: Comment,
    pub replies: Comments,
}

#[derive(Debug)]
pub enum Msg {
    More,
    Loaded(Result<CommentPage, String>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Load(String),
}

impl State {
    /// Opens the replies to `comment`, loading the first page.
    pub fn new(comment: Comment) -> (Self, Vec<Effect>) {
        let replies = Comments::new(CommentPage {
            comments: Vec::new(),
            count: Some(comment.reply_count.into()),
            next: comment.replies.clone(),
        });
        let mut state = State { comment, replies };
        let effects = state.load_next();
        (state, effects)
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::More => return self.load_next(),
            Msg::Loaded(result) => self.replies.loaded(result),
        }
        Vec::new()
    }

    fn load_next(&mut self) -> Vec<Effect> {
        self.replies
            .load_next()
            .map(Effect::Load)
            .into_iter()
            .collect()
    }
}

pub struct RepliesPage {
    state: State,
    services: Services,
    list: CommentList,
}

#[relm4::component(pub)]
impl Component for RepliesPage {
    type Init = (Services, Comment);
    type Input = Msg;
    type Output = ();
    type CommandOutput = Msg;

    view! {
        adw::ToolbarView {
            add_top_bar = &adw::HeaderBar {},

            #[wrap(Some)]
            set_content = &gtk::ScrolledWindow {
                set_hscrollbar_policy: gtk::PolicyType::Never,

                adw::Clamp {
                    set_maximum_size: 800,

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 24,
                        set_margin_all: 12,

                        #[local_ref]
                        parent -> adw::PreferencesGroup {},

                        #[local_ref]
                        replies -> adw::PreferencesGroup {
                            set_title: &format::replies(model.state.comment.reply_count),
                        },
                    },
                },
            },
        }
    }

    fn init(
        (services, comment): Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let (state, effects) = State::new(comment);
        let more = sender.input_sender().clone();
        let replies = adw::PreferencesGroup::new();
        let mut model = RepliesPage {
            state,
            services,
            // Replies have no replies of their own to open.
            list: CommentList::new(
                Rows::Group(replies.clone()),
                move || more.emit(Msg::More),
                None::<fn(usize)>,
            ),
        };
        let parent = adw::PreferencesGroup::new();
        parent.add(&comments::comment_row(&model.state.comment, None));
        let replies = &replies;
        let parent = &parent;
        let widgets = view_output!();
        model.run(effects, &sender);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        let effects = self.state.update(msg);
        self.run(effects, &sender);
    }

    fn update_cmd(&mut self, msg: Msg, sender: ComponentSender<Self>, root: &Self::Root) {
        self.update(msg, sender, root);
    }
}

impl RepliesPage {
    fn run(&mut self, effects: Vec<Effect>, sender: &ComponentSender<Self>) {
        for effect in effects {
            match effect {
                Effect::Load(token) => {
                    let backend = self.services.backend.clone();
                    sender.oneshot_command(async move {
                        Msg::Loaded(backend.comments(&token).await.map_err(|e| e.to_string()))
                    });
                }
            }
        }
        self.list.update(&self.state.replies);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(text: &str) -> Comment {
        Comment {
            author: None,
            text: text.into(),
            published: None,
            like_count: None,
            reply_count: 3,
            replies: Some("r1".into()),
            pinned: false,
            by_owner: false,
        }
    }

    fn page(texts: &[&str], next: Option<&str>) -> CommentPage {
        CommentPage {
            comments: texts.iter().map(|t| comment(t)).collect(),
            count: None,
            next: next.map(Into::into),
        }
    }

    #[test]
    fn opening_loads_the_first_page() {
        let (state, effects) = State::new(comment("parent"));
        assert_eq!(effects, [Effect::Load("r1".into())]);
        assert!(state.replies.loading);
        assert_eq!(state.replies.count, Some(3));
    }

    #[test]
    fn more_replies_are_appended_until_last_page() {
        let (mut state, _) = State::new(comment("parent"));
        assert!(state.update(Msg::More).is_empty(), "still loading");

        state.update(Msg::Loaded(Ok(page(&["a"], Some("r2")))));
        assert_eq!(state.update(Msg::More), [Effect::Load("r2".into())]);
        state.update(Msg::Loaded(Ok(page(&["b"], None))));

        let texts: Vec<_> = state
            .replies
            .items
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(texts, ["a", "b"]);
        assert!(state.update(Msg::More).is_empty());
    }

    #[test]
    fn failed_replies_can_be_retried() {
        let (mut state, _) = State::new(comment("parent"));
        state.update(Msg::Loaded(Err("offline".into())));
        assert_eq!(state.replies.error.as_deref(), Some("offline"));
        assert_eq!(state.update(Msg::More), [Effect::Load("r1".into())]);
    }
}
