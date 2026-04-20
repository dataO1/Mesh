// Hide console window on Windows (GUI application)
// This sets the executable subsystem to WINDOWS instead of CONSOLE
// Use --features console to show the console for debugging
#![cfg_attr(
    all(target_os = "windows", not(feature = "console")),
    windows_subsystem = "windows"
)]

//! Mesh DJ Player - 4-deck stem-based mixing with neural effects
//!
//! This is the main entry point for the GUI application. It:
//! 1. Starts the CPAL audio system in background threads
//! 2. Launches the iced GUI application
//! 3. Passes shared state between UI and audio
//!
//! ## Command line flags
//!
//! - `--midi-learn`: Start in MIDI learn mode for creating controller profiles

mod audio;
mod config;
mod direct_dispatch;
mod domain;
mod history;
mod loader;
mod plugin_gui;
mod suggestions;
mod ui;

use iced::{Size, Task};

use audio::{start_audio_system, start_audio_system_with_devices};
use mesh_core::db::DatabaseService;
use ui::{MeshApp, app::Message, midi_learn::MidiLearnMessage};

const CLIENT_NAME: &str = "mesh-player";

/// Real-time audio initialization for embedded builds (OrangePi 5 / RK3588).
///
/// All functions are no-ops on non-embedded builds via the `embedded-rt` feature gate.
/// These optimizations eliminate the most common sources of audio glitches:
/// - Page faults (mlockall)
/// - CPU idle state wakeup latency (cpu_dma_latency)
/// - Rayon worker preemption (SCHED_FIFO)
/// - Core contention (CPU affinity)
#[cfg(feature = "embedded-rt")]
mod rt_init {
    use std::io::Write;

    /// Lock all memory to prevent page faults in the audio callback.
    /// A single major page fault takes 1-10ms — far exceeding the 5.33ms buffer period.
    pub fn mlockall() {
        unsafe {
            let ret = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
            if ret == 0 {
                log::info!("[RT] Memory locked (mlockall MCL_CURRENT|MCL_FUTURE)");
            } else {
                log::warn!(
                    "[RT] mlockall failed: {} — page faults may cause xruns",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    /// Disable CPU idle states by writing 0 to /dev/cpu_dma_latency.
    /// The file handle MUST be kept open for the process lifetime — dropping re-enables C-states.
    /// C-state wakeup on ARM can take 200µs-2ms, stealing from the 5.33ms audio budget.
    pub fn disable_cpu_idle() -> Option<std::fs::File> {
        match std::fs::File::create("/dev/cpu_dma_latency") {
            Ok(mut f) => {
                if f.write_all(&0i32.to_le_bytes()).is_ok() {
                    log::info!("[RT] CPU idle states disabled via /dev/cpu_dma_latency");
                    Some(f)
                } else {
                    log::warn!("[RT] Failed to write to /dev/cpu_dma_latency");
                    None
                }
            }
            Err(e) => {
                log::warn!("[RT] Could not open /dev/cpu_dma_latency: {}", e);
                None
            }
        }
    }

    /// Log RT capability limits for diagnostics.
    pub fn verify_rt_caps() {
        unsafe {
            let mut rlim: libc::rlimit = std::mem::zeroed();
            if libc::getrlimit(libc::RLIMIT_RTPRIO, &mut rlim) == 0 {
                log::info!("[RT] RLIMIT_RTPRIO: soft={}, hard={}", rlim.rlim_cur, rlim.rlim_max);
                if rlim.rlim_cur < 80 {
                    log::warn!("[RT] RT priority limit too low for audio threads (need >= 80)");
                }
            }
            if libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut rlim) == 0 {
                log::info!("[RT] RLIMIT_MEMLOCK: soft={}, hard={}", rlim.rlim_cur, rlim.rlim_max);
            }
        }
    }

    /// Pin the main/render thread to an A76 big core.
    ///
    /// The A76 is 2-3x faster for single-threaded work than A55. Without pinning,
    /// the scheduler can place the render thread on A55 cores (1-3) where it competes
    /// with audio rayon workers. Core 4 = first A76 on RK3588S.
    pub fn pin_render_thread() {
        unsafe {
            let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_ZERO(&mut cpuset);
            libc::CPU_SET(4, &mut cpuset); // Core 4 = first A76
            let ret = libc::sched_setaffinity(
                0,
                std::mem::size_of::<libc::cpu_set_t>(),
                &cpuset,
            );
            if ret != 0 {
                log::warn!(
                    "[RT] sched_setaffinity to core 4 (A76) failed for render thread: {}",
                    std::io::Error::last_os_error()
                );
            } else {
                log::info!("[RT] Render thread pinned to core 4 (A76)");
            }
        }
    }

    /// Configure a rayon worker thread for RT audio processing.
    /// Called from rayon's start_handler on each worker thread.
    /// - Pins each worker to a dedicated A55 core (1, 2, or 3 by round-robin)
    ///   Core 0 is reserved for JACK RT thread
    /// - Sets SCHED_FIFO priority 70 (below PipeWire's 88, above normal tasks)
    /// - Pre-faults 512KB of stack to avoid minor page faults during processing
    pub fn setup_rayon_worker(thread_idx: usize) {
        // Pin each worker to its own A55 LITTLE core (1, 2, 3)
        // Core 0 is reserved for JACK RT — one worker per core eliminates contention
        // A55 in-order pipeline gives deterministic WCET (1.2-1.5x avg vs A76's 2-5x)
        let core = 1 + (thread_idx % 3);
        unsafe {
            let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_ZERO(&mut cpuset);
            libc::CPU_SET(core, &mut cpuset);
            let ret = libc::sched_setaffinity(
                0,
                std::mem::size_of::<libc::cpu_set_t>(),
                &cpuset,
            );
            if ret != 0 {
                log::warn!(
                    "[RT] sched_setaffinity to core {} failed for rayon worker {}: {}",
                    core, thread_idx, std::io::Error::last_os_error()
                );
            } else {
                log::info!("[RT] rayon-audio-{} pinned to core {} (A55)", thread_idx, core);
            }
        }

        // Set SCHED_FIFO priority 70 — prevents preemption by non-RT tasks during audio callback
        unsafe {
            let param = libc::sched_param { sched_priority: 70 };
            let ret = libc::sched_setscheduler(0, libc::SCHED_FIFO, &param);
            if ret != 0 {
                log::warn!(
                    "[RT] sched_setscheduler(FIFO, 70) failed for rayon worker: {}",
                    std::io::Error::last_os_error()
                );
            }
        }

        // Pre-fault 512KB of stack pages to avoid minor faults during audio processing.
        // mlockall(MCL_FUTURE) locks pages on first touch, but the first touch still
        // causes a minor fault (kernel allocates the page).
        let stack = vec![0u8; 512 * 1024];
        std::hint::black_box(&stack);
    }
}

fn main() -> iced::Result {
    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    let start_midi_learn = args.iter().any(|arg| arg == "--midi-learn");

    // Initialize logger - set RUST_LOG=debug for verbose output
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    log::info!("mesh-player starting up");
    if start_midi_learn {
        log::info!("Mapping mode requested via --midi-learn flag");
    } else {
        log::info!("Starting in performance mode (use --midi-learn for full UI)");
    }

    // === Embedded RT initialization (feature-gated, only on aarch64 embedded builds) ===
    // Must run before any threads are created for mlockall to cover all future allocations
    #[cfg(feature = "embedded-rt")]
    {
        rt_init::verify_rt_caps();
        rt_init::mlockall();
    }
    // Hold cpu_dma_latency fd open for the entire process lifetime
    #[cfg(feature = "embedded-rt")]
    let _cpu_dma_latency_guard = rt_init::disable_cpu_idle();

    // Initialize Rayon thread pool before audio starts
    // This prevents lazy initialization from causing latency in the audio callback
    // (Rayon's default lazy init would happen on first parallel call, which is in audio callback)
    rayon::ThreadPoolBuilder::new()
        .num_threads(3) // 3 workers on A55 cores 1-3 (core 0 = JACK RT)
        .thread_name(|i| format!("rayon-audio-{}", i))
        .start_handler(|_thread_idx| {
            #[cfg(feature = "embedded-rt")]
            rt_init::setup_rayon_worker(_thread_idx);
        })
        .build_global()
        .expect("Failed to initialize Rayon thread pool");
    log::info!("Rayon thread pool initialized with 3 threads");

    // Pin render/main thread to A76 big core for 2-3x better single-threaded perf
    #[cfg(feature = "embedded-rt")]
    rt_init::pin_render_thread();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                          Mesh                                  ║");
    println!("║              4-deck stem-based mixing                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Load config to get collection path for database service
    let config_path = config::default_config_path();
    let config: config::PlayerConfig = config::load_config(&config_path);

    // Set global font size scale (used by sz() helper across all widget code)
    mesh_widgets::set_font_scale(config.display.font_size.scale() * config.display.font.size_scale());

    // Create database service (required for track metadata loading)
    let db_service = DatabaseService::new(&config.collection_path)
        .expect("Failed to create database service - this is required for mesh-player");
    log::info!("Database service initialized at {:?}", config.collection_path);

    // Resolve saved audio device indices from config to DeviceIds.
    // The config stores a device list index (usize); we enumerate devices at startup
    // and look up the saved index so the configured device is used from the first frame.
    let startup_audio_devices = mesh_core::audio::get_available_output_devices();
    let master_device_id = config.audio.outputs.master_device
        .and_then(|idx| startup_audio_devices.get(idx))
        .map(|d| d.id.clone());
    let cue_device_id = config.audio.outputs.cue_device
        .and_then(|idx| startup_audio_devices.get(idx))
        .map(|d| d.id.clone());
    if master_device_id.is_some() || cue_device_id.is_some() {
        log::info!(
            "Using configured audio devices: master={:?}, cue={:?}",
            master_device_id.as_ref().map(|d| &d.name),
            cue_device_id.as_ref().map(|d| &d.name),
        );
    }

    // Try to start audio system
    // Returns AudioHandle, CommandSender (lock-free queue), DeckAtomics, SlicerAtomics,
    // LinkedStemAtomics, LinkedStemResultReceiver, and sample rate
    let audio_start_result = if master_device_id.is_some() || cue_device_id.is_some() {
        start_audio_system_with_devices(db_service.clone(), master_device_id, cue_device_id)
    } else {
        start_audio_system(CLIENT_NAME, db_service.clone())
    };
    let (audio_handle, command_sender, deck_atomics, slicer_atomics, linked_stem_atomics, linked_stem_receiver, clip_indicator, level_atomics, audio_sample_rate, audio_client_name, output_latency_samples, internal_latency_samples, direct_command_producer) =
        match audio_start_result {
            Ok((handle, sender, deck_atomics, slicer_atomics, linked_stem_atomics, linked_stem_receiver, clip_indicator, level_atomics, sample_rate, client_name, output_lat, internal_lat, direct_producer)) => {
                println!("Audio system started successfully ({} Hz, client: {})", sample_rate, client_name);
                (Some(handle), Some(sender), Some(deck_atomics), Some(slicer_atomics), Some(linked_stem_atomics), Some(linked_stem_receiver), Some(clip_indicator), Some(level_atomics), sample_rate, client_name, Some(output_lat), Some(internal_lat), Some(direct_producer))
            }
            Err(e) => {
                eprintln!("Warning: Could not start audio system: {}", e);
                eprintln!("Running in UI-only mode (no audio output)");
                eprintln!();
                eprintln!("Check that audio devices are available and not in use by other applications.");
                // Default to 44100 Hz when audio is not available
                (None, None, None, None, None, None, None, None, 44100, "mesh-player".to_string(), None, None, None)
            }
        };

    println!();
    println!("Starting Mesh GUI...");

    // Wrap resources in cells so the boot closure can be Fn (required by iced)
    // The boot function is only called once, but iced requires Fn for API consistency
    let db_service_cell = std::cell::RefCell::new(Some(db_service));
    let command_sender_cell = std::cell::RefCell::new(command_sender);
    let deck_atomics_cell = std::cell::RefCell::new(deck_atomics);
    let slicer_atomics_cell = std::cell::RefCell::new(slicer_atomics);
    let linked_stem_atomics_cell = std::cell::RefCell::new(linked_stem_atomics);
    let linked_stem_receiver_cell = std::cell::RefCell::new(linked_stem_receiver);
    let clip_indicator_cell = std::cell::RefCell::new(clip_indicator);
    let level_atomics_cell = std::cell::RefCell::new(level_atomics);
    let output_latency_cell = std::cell::RefCell::new(output_latency_samples);
    let internal_latency_cell = std::cell::RefCell::new(internal_latency_samples);
    let direct_producer_cell = std::cell::RefCell::new(direct_command_producer);

    // Pre-allocate StemBuffer pool for zero-allocation track loading.
    // 5 buffers × 10 min @ 48kHz ≈ 4.4 GB — 4 for active decks + 1 spare for priority snapshots.
    // Buffers are automatically recycled via StemBuffers::drop() when the engine
    // replaces old stems, so 5 buffers sustain unlimited track loads.
    let buffer_pool = {
        let max_samples = 10 * 60 * 48000; // 10 minutes at 48kHz
        log::info!("[POOL] Pre-allocating StemBuffer pool (5 x {} samples)...", max_samples);
        let pool = std::sync::Arc::new(mesh_core::buffer_pool::StemBufferPool::new(5, max_samples));
        // Register global pool for automatic recycling in StemBuffers::drop()
        mesh_core::buffer_pool::set_global_pool(pool.clone());
        log::info!("[POOL] StemBuffer pool ready (cyclic recycling enabled)");
        Some(pool)
    };

    // Run the iced application using the functional API
    let result = iced::application(
        move || {
            // Boot function: creates initial state with lock-free command channel
            // Take ownership from the cells (only called once)
            let db_service = db_service_cell.borrow_mut().take().expect("db_service already taken");
            let sender = command_sender_cell.borrow_mut().take();
            let deck_atomics = deck_atomics_cell.borrow_mut().take();
            let slicer_atomics = slicer_atomics_cell.borrow_mut().take();
            let linked_stem_atomics = linked_stem_atomics_cell.borrow_mut().take();
            let linked_stem_receiver = linked_stem_receiver_cell.borrow_mut().take();
            let clip_indicator = clip_indicator_cell.borrow_mut().take();
            let level_atomics = level_atomics_cell.borrow_mut().take();
            let output_latency = output_latency_cell.borrow_mut().take();
            let internal_latency = internal_latency_cell.borrow_mut().take();
            let direct_producer = direct_producer_cell.borrow_mut().take();

            // Create direct dispatch for timing-critical MIDI→engine path
            let direct_dispatch = direct_producer.map(|producer| {
                std::sync::Arc::new(direct_dispatch::EngineDirectDispatch::new(producer))
                    as std::sync::Arc<dyn mesh_midi::DirectDispatch>
            });

            // Auto-start MIDI learn if no midi.yaml exists
            let auto_learn = !mesh_midi::default_midi_config_path().exists();
            if auto_learn {
                log::info!("No midi.yaml found — will auto-start MIDI learn mode");
            }
            // --midi-learn flag → full mapping UI + learn drawer
            // auto_learn (no midi.yaml) → performance UI + learn drawer
            let show_mapping_ui = start_midi_learn;
            let start_learn = start_midi_learn || auto_learn;

            let mut app = MeshApp::new(db_service, sender, deck_atomics, slicer_atomics, linked_stem_atomics, linked_stem_receiver, clip_indicator, level_atomics, audio_sample_rate, audio_client_name.clone(), show_mapping_ui, start_learn, output_latency, internal_latency, buffer_pool.clone());

            // Wire direct dispatch to controller for bypassing iced tick on timing-critical commands
            if let (Some(ref mut controller), Some(dispatch)) = (&mut app.controller, direct_dispatch) {
                controller.set_direct_dispatch(dispatch);
                log::info!("Direct MIDI→engine dispatch enabled");
            }

            // Query monitor size for auto-resolution
            // Use oldest() to get the main window Id, then chain monitor_size query
            let monitor_task = iced::window::oldest().then(|opt_id| {
                if let Some(id) = opt_id {
                    iced::window::monitor_size(id).map(Message::GotMonitorSize)
                } else {
                    Task::done(Message::GotMonitorSize(None))
                }
            });

            // Background t-SNE + clustering for the graph view
            let graph_db = app.collection_browser.db_service_arc();
            app.collection_browser.graph_building = true;
            let graph_task = Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        use mesh_core::graph_compute;
                        use mesh_widgets::graph_view::TrackMeta;

                        let all_pca = graph_db.get_all_pca_with_tracks().unwrap_or_default();
                        let pca_data: Vec<(i64, Vec<f32>)> = all_pca.iter()
                            .filter_map(|(t, v)| Some((t.id?, v.clone())))
                            .collect();

                        if pca_data.len() < 10 {
                            return None;
                        }

                        let positions = graph_compute::compute_tsne_layout(&pca_data, false);
                        let cluster_result = graph_compute::run_consensus_clustering(&positions);

                        // Build track metadata
                        let track_meta: std::collections::HashMap<i64, TrackMeta> = all_pca.iter()
                            .filter_map(|(t, _)| {
                                let id = t.id?;
                                Some((id, TrackMeta {
                                    id,
                                    title: t.title.clone(),
                                    artist: t.artist.clone(),
                                    key: t.key.clone(),
                                    bpm: t.bpm,
                                }))
                            })
                            .collect();

                        Some(ui::message::GraphData {
                            positions,
                            clusters: cluster_result.clusters,
                            confidence: cluster_result.confidence,
                            colors: cluster_result.colors,
                            track_meta,
                        })
                    })
                    .await
                    .ok()
                    .flatten()
                },
                |data: Option<ui::message::GraphData>| match data {
                    Some(d) => Message::GraphDataReady(std::sync::Arc::new(d)),
                    None => Message::RefreshResourceStats, // no-op reuse
                },
            );

            // If --midi-learn flag was passed or no midi.yaml exists, start MIDI learn mode
            let startup_task = if start_learn {
                Task::batch([
                    monitor_task,
                    graph_task,
                    Task::done(Message::MidiLearn(MidiLearnMessage::Start)),
                ])
            } else {
                Task::batch([monitor_task, graph_task])
            };

            (app, startup_task)
        },
        update,
        view,
    )
    .subscription(subscription)
    .theme(theme)
    .title("Mesh")
    .settings(iced::Settings {
        default_text_size: iced::Pixels(16.0 * config.display.font.size_scale() * config.display.font_size.scale()),
        antialiasing: true,
        ..Default::default()
    })
    .window(iced::window::Settings {
        size: Size::new(1920.0, 1080.0),
        min_size: Some(Size::new(1280.0, 720.0)),
        icon: iced::window::icon::from_file_data(
            include_bytes!("../../../assets/grid.png"),
            None,
        ).ok(),
        ..Default::default()
    })
    .font(mesh_widgets::AppFont::Hack.font_data())
    .font(mesh_widgets::AppFont::JetBrainsMono.font_data())
    .font(mesh_widgets::AppFont::PressStart2P.font_data())
    .font(mesh_widgets::AppFont::Exo.font_data())
    .default_font(config.display.font.to_iced_font())
    .run();

    // Keep audio handle alive until we're done (it will be dropped here)
    drop(audio_handle);
    println!("Mesh stopped.");

    // Force-exit to terminate any lingering background threads (history write threads,
    // rayon pool, etc.) that would otherwise keep the process alive during cleanup.
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}

/// Update function for iced
fn update(app: &mut MeshApp, message: Message) -> Task<Message> {
    app.update(message)
}

/// View function for iced
fn view(app: &MeshApp) -> iced::Element<'_, Message> {
    app.view()
}

/// Subscription function for iced
fn subscription(app: &MeshApp) -> iced::Subscription<Message> {
    app.subscription()
}

/// Theme function for iced
fn theme(app: &MeshApp) -> iced::Theme {
    app.theme()
}
