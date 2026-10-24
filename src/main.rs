fn main() {
    let app = relm4::RelmApp::new(flux::APP_ID);
    app.run::<flux::ui::app::App>(());
}
