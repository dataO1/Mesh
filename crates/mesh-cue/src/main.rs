//! Mesh Cue - Track preparation GUI application

use mesh_cue::ui::MeshCueApp;

fn title(_app: &MeshCueApp) -> String {
    String::from("mesh-cue - Track Preparation")
}

fn main() -> iced::Result {
    iced::application(MeshCueApp::new, MeshCueApp::update, MeshCueApp::view)
        .title(title)
        .window_size(iced::Size::new(1200.0, 800.0))
        .theme(MeshCueApp::theme)
        .run()
}
