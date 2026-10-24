use std::rc::Rc;
use std::sync::Arc;

use flux::core::backend::rustypipe::RustyPipeBackend;
use flux::core::db::Db;
use flux::ui::Services;

fn main() {
    gstreamer::init().expect("cannot initialize GStreamer");

    let data_dir = relm4::gtk::glib::user_data_dir().join("flux");
    std::fs::create_dir_all(&data_dir).expect("cannot create data directory");
    let db = Db::open(&data_dir.join("flux.db")).expect("cannot open database");
    db.requeue_interrupted_downloads()
        .expect("cannot update downloads");
    let backend = RustyPipeBackend::new(&data_dir).expect("cannot create backend");

    let app = relm4::RelmApp::new(flux::APP_ID);
    app.run::<flux::ui::app::App>(Services {
        backend: Arc::new(backend),
        db: Rc::new(db),
    });
}
