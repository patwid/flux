use std::sync::Arc;

use flux::core::backend::rustypipe::RustyPipeBackend;

fn main() {
    let data_dir = relm4::gtk::glib::user_data_dir().join("flux");
    std::fs::create_dir_all(&data_dir).expect("cannot create data directory");
    let backend = RustyPipeBackend::new(&data_dir).expect("cannot create backend");

    let app = relm4::RelmApp::new(flux::APP_ID);
    app.run::<flux::ui::app::App>(Arc::new(backend));
}
