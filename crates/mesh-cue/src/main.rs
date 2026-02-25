// Hide console window on Windows (GUI application)
// This sets the executable subsystem to WINDOWS instead of CONSOLE
// Use --features console to show the console for debugging
#![cfg_attr(
    all(target_os = "windows", not(feature = "console")),
    windows_subsystem = "windows"
)]

//! Mesh Cue - Track preparation GUI application

use mesh_cue::ui::MeshCueApp;

fn title(_app: &MeshCueApp) -> String {
    String::from("Mesh")
}

fn main() -> iced::Result {
    // Initialize procspawn early, before any threads are created.
    // This is required for process-based parallelism to work correctly.
    // Essentia is not thread-safe, so we run analysis in subprocesses.
    procspawn::init();

    // Initialize logger - set RUST_LOG=debug for verbose output
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    log::info!("mesh-cue starting up");

    // Load config to get font selection for iced default_font
    let config_path = mesh_cue::config::default_config_path();
    let config: mesh_cue::config::Config = mesh_cue::config::load_config(&config_path);
    let selected_font = config.display.font;

    iced::application(MeshCueApp::new, MeshCueApp::update, MeshCueApp::view)
        .title(title)
        .window(iced::window::Settings {
            size: iced::Size::new(1200.0, 800.0),
            min_size: Some(iced::Size::new(960.0, 600.0)),
            icon: iced::window::icon::from_file_data(
                include_bytes!("../../../assets/grid.png"),
                None,
            ).ok(),
            ..Default::default()
        })
        .settings(iced::Settings {
            default_text_size: iced::Pixels(16.0 * selected_font.size_scale()),
            ..Default::default()
        })
        .font(mesh_widgets::AppFont::Hack.font_data())
        .font(mesh_widgets::AppFont::JetBrainsMono.font_data())
        .font(mesh_widgets::AppFont::PressStart2P.font_data())
        .font(mesh_widgets::AppFont::Exo.font_data())
        .font(mesh_widgets::AppFont::SpaceMono.font_data())
        .font(mesh_widgets::AppFont::SaxMono.font_data())
        .default_font(selected_font.to_iced_font())
        .theme(MeshCueApp::theme)
        .subscription(MeshCueApp::subscription)
        .run()
}
