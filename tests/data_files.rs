//! Checks that the desktop integration files match the application id.

use std::path::Path;

fn read(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn desktop_file_matches_app_id() {
    let desktop = read(&format!("data/{}.desktop", flux::APP_ID));
    assert!(desktop.contains(&format!("Icon={}\n", flux::APP_ID)));
    assert!(desktop.contains("Exec=flux\n"));
    assert!(desktop.contains("X-Purism-FormFactor=Workstation;Mobile;"));
}

#[test]
fn metainfo_matches_app_id() {
    let metainfo = read(&format!("data/{}.metainfo.xml", flux::APP_ID));
    assert!(metainfo.contains(&format!("<id>{}</id>", flux::APP_ID)));
    assert!(metainfo.contains(&format!(
        "<launchable type=\"desktop-id\">{}.desktop</launchable>",
        flux::APP_ID
    )));
}

#[test]
fn icon_exists() {
    let icon = read(&format!("data/icons/{}.svg", flux::APP_ID));
    assert!(icon.starts_with("<svg") || icon.starts_with("<?xml"));
}
