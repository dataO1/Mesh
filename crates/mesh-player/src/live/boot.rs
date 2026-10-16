//! Live-mode process startup (D35), called from `main` when `mode: live`.
//!
//! Skips everything DJ-only (StemBufferPool, database, DJ engine and domain),
//! loads the configured style, starts the audio backend with a
//! [`LiveEngine`] built at the device's sample rate and runs [`LiveApp`].

use std::cell::RefCell;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use iced::Size;
use mesh_core::audio::{DeviceId, RtEngine};
use mesh_core::live::engine::{LiveEngine, LiveHandles};
use mesh_core::live::loader::{ClapMode, LoadError, LoadOptions};
use mesh_core::live::style::Style;
use mesh_core::types::{StereoBuffer, StereoSample};

use crate::audio::start_engine_with_devices;
use crate::config::PlayerConfig;

use super::app::{LiveApp, LiveBoot};
use super::startup::{collection_for_style_dir, load_problem, resolve_style, resolve_style_dir, StyleSource};

/// Audio-thread engine of live mode. Empty when the style failed to load
/// at the device rate, so the backend still has something to run.
struct LiveRt {
    engine: Option<LiveEngine>,
    no_recordings: Vec<rtrb::Producer<StereoSample>>,
}

impl RtEngine for LiveRt {
    fn process_block(&mut self, master: &mut StereoBuffer, cue: &mut StereoBuffer) {
        match &mut self.engine {
            Some(e) => e.process_block(master, cue),
            None => {
                master.fill_silence();
                cue.fill_silence();
            }
        }
    }

    fn recording_producers(&mut self) -> &mut Vec<rtrb::Producer<StereoSample>> {
        match &mut self.engine {
            Some(e) => e.recording_producers(),
            None => &mut self.no_recordings,
        }
    }

    fn output_latency_samples(&self) -> Arc<AtomicU64> {
        match &self.engine {
            Some(e) => e.output_latency_samples(),
            None => Arc::new(AtomicU64::new(0)),
        }
    }
}

fn load_options(sample_rate: u32, collection: &std::path::Path) -> LoadOptions {
    let mut opts = LoadOptions::new(sample_rate);
    opts.clap = ClapMode::Scan { extra_paths: Vec::new() };
    opts.collection = Some(collection.to_path_buf());
    opts
}

/// Load the style and start audio. Returns the app boot state and the audio
/// handle, which must outlive the UI.
///
/// `source` overrides `live.style` (`--live STYLE`). A style loaded by path
/// sends captures and recordings to [`collection_for_style_dir`]; the
/// configured collection still provides the theme and Pure Data effects.
fn boot(
    config: &PlayerConfig,
    source: Option<StyleSource>,
    master: Option<DeviceId>,
    cue: Option<DeviceId>,
) -> (LiveBoot, Option<mesh_core::audio::AudioHandle>) {
    let library = config.collection_path.clone();
    let themes = mesh_widgets::theme::load_themes(&library.join("theme.yaml"));
    let theme = mesh_widgets::theme::find_theme(&themes, &config.display.theme).clone();

    let (resolved, collection) = match source {
        Some(StyleSource::Path(dir)) => {
            let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
            let collection = collection_for_style_dir(&dir, &library);
            (resolve_style_dir(&dir, &collection), collection)
        }
        Some(StyleSource::Named(name)) => (resolve_style(&library, Some(&name)), library.clone()),
        None => (resolve_style(&library, config.live.style.as_deref()), library.clone()),
    };
    log::info!("[LIVE] captures and recordings go to {}", collection.display());
    let failed = |problem, theme| LiveBoot::Failed { problem, collection: collection.clone(), theme };

    let style = match resolved {
        Ok(s) => s,
        Err(problem) => {
            log::error!("[LIVE] {}: {:?}", problem.title, problem.details);
            return (failed(problem, theme), None);
        }
    };
    let name = style.spec.name.clone();
    let running = |style: Style, handles: LiveHandles, sample_rate, audio_error, theme| LiveBoot::Running {
        style,
        handles,
        sample_rate,
        audio_error,
        collection: collection.clone(),
        theme,
        left_serial: config.live.f1_left_serial.clone(),
        auto_record: config.live.auto_record,
    };

    log::info!("[LIVE] loading style '{name}'");
    let started = start_engine_with_devices(master, cue, |rate| {
        match LiveEngine::start(&style, load_options(rate, &library)) {
            Ok((engine, handles)) => (LiveRt { engine: Some(engine), no_recordings: Vec::new() }, Ok(handles)),
            Err(e) => (LiveRt { engine: None, no_recordings: Vec::new() }, Err::<LiveHandles, LoadError>(e)),
        }
    });
    match started {
        Ok(system) => {
            println!("Audio system started ({} Hz, client: {})", system.sample_rate, system.client_name);
            match system.handles {
                Ok(handles) => {
                    let boot = running(style, handles, system.sample_rate, None, theme);
                    (boot, Some(system.handle))
                }
                Err(e) => (failed(load_problem(&collection, &name, &e), theme), None),
            }
        }
        Err(e) => {
            // No audio device: run the UI on an engine nobody drives, so the
            // screen and controls can still be developed.
            log::warn!("[LIVE] audio unavailable: {e}");
            let rate = mesh_core::types::SAMPLE_RATE;
            match LiveEngine::start(&style, load_options(rate, &library)) {
                Ok((_engine, handles)) => (running(style, handles, rate, Some(e.to_string()), theme), None),
                Err(le) => (failed(load_problem(&collection, &name, &le), theme), None),
            }
        }
    }
}

/// Run live mode until the window closes.
pub fn run(
    config: PlayerConfig,
    source: Option<StyleSource>,
    master: Option<DeviceId>,
    cue: Option<DeviceId>,
) -> iced::Result {
    let (boot, audio_handle) = boot(&config, source, master, cue);
    let boot = RefCell::new(Some(boot));

    let result = iced::application(
        move || LiveApp::new(boot.borrow_mut().take().expect("live app booted once")),
        LiveApp::update,
        LiveApp::view,
    )
    .subscription(LiveApp::subscription)
    .theme(LiveApp::theme)
    .title("Mesh Live")
    .settings(iced::Settings { antialiasing: true, ..Default::default() })
    .window(iced::window::Settings {
        size: Size::new(1920.0, 640.0),
        min_size: Some(Size::new(960.0, 360.0)),
        icon: iced::window::icon::from_file_data(include_bytes!("../../../../assets/grid.png"), None).ok(),
        ..Default::default()
    })
    .font(mesh_widgets::AppFont::Hack.font_data())
    .font(mesh_widgets::AppFont::JetBrainsMono.font_data())
    .font(mesh_widgets::AppFont::PressStart2P.font_data())
    .font(mesh_widgets::AppFont::Exo.font_data())
    .default_font(config.display.font.to_iced_font())
    .run();

    drop(audio_handle);
    result
}

