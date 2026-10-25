pub mod app;
pub mod aspect;
pub mod channel;
pub mod comments;
pub mod cue_layout;
pub mod downloads;
pub mod feed;
pub mod format;
pub mod history;
pub mod images;
pub mod player_view;
pub mod playlist;
pub mod playlists;
pub mod replies;
pub mod search;
pub mod subscriptions;
pub mod video;
pub mod widgets;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::core::backend::{Backend, Downloader};
use crate::core::db::Db;

/// Shared services handed to the pages.
#[derive(Clone)]
pub struct Services {
    pub backend: Arc<dyn Backend>,
    pub downloader: Arc<dyn Downloader>,
    pub db: Rc<Db>,
    /// Where downloads are saved.
    pub download_dir: PathBuf,
}

/// Logs a failed database operation; the UI keeps working with stale data.
pub fn log_db_error<T>(result: crate::core::db::Result<T>) -> Option<T> {
    result
        .inspect_err(|e| relm4::gtk::glib::g_warning!("flux", "database error: {e}"))
        .ok()
}
