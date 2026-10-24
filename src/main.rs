use std::rc::Rc;
use std::sync::Arc;

use flux::core::backend::rustypipe::RustyPipeBackend;
use flux::core::db::Db;
use flux::ui::Services;
use relm4::gtk::glib;

fn main() {
    gstreamer::init().expect("cannot initialize GStreamer");

    let data_dir = glib::user_data_dir().join("flux");
    std::fs::create_dir_all(&data_dir).expect("cannot create data directory");
    let db = Db::open(&data_dir.join("flux.db")).expect("cannot open database");
    db.requeue_interrupted_downloads()
        .expect("cannot update downloads");
    let backend = Arc::new(RustyPipeBackend::new(&data_dir).expect("cannot create backend"));
    let download_dir = glib::user_special_dir(glib::UserDirectory::Downloads)
        .unwrap_or_else(|| glib::home_dir().join("Downloads"))
        .join("Flux");

    let app = relm4::RelmApp::new(flux::APP_ID);
    app.run::<flux::ui::app::App>(Services {
        backend: backend.clone(),
        downloader: backend,
        db: Rc::new(db),
        download_dir,
    });
}
