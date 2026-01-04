//! Mesh Cue - Track preparation GUI application

use mesh_cue::ui::MeshCueApp;

fn title(_app: &MeshCueApp) -> String {
    String::from("mesh-cue - Track Preparation")
}

fn main() -> iced::Result {
    // Initialize logger - set RUST_LOG=debug for verbose output
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    log::info!("mesh-cue starting up");

    iced::application(MeshCueApp::new, MeshCueApp::update, MeshCueApp::view)
        .title(title)
        .window_size(iced::Size::new(1200.0, 800.0))
        .theme(MeshCueApp::theme)
        .subscription(MeshCueApp::subscription)
        .run()
}
