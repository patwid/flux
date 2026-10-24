pub mod app;
pub mod aspect;
pub mod channel;
pub mod feed;
pub mod format;
pub mod player_view;
pub mod search;
pub mod video;
pub mod widgets;

use std::rc::Rc;
use std::sync::Arc;

use crate::core::backend::Backend;
use crate::core::db::Db;

/// Shared services handed to the pages.
#[derive(Clone)]
pub struct Services {
    pub backend: Arc<dyn Backend>,
    pub db: Rc<Db>,
}

/// Logs a failed database operation; the UI keeps working with stale data.
pub fn log_db_error<T>(result: crate::core::db::Result<T>) -> Option<T> {
    result
        .inspect_err(|e| relm4::gtk::glib::g_warning!("flux", "database error: {e}"))
        .ok()
}
