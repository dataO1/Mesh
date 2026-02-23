//! Frame-synced tick handler — runs at display refresh rate (60Hz/120Hz/etc.)
//!
//! **PERFORMANCE CRITICAL**: This handler runs every frame. All work here must be:
//! - Lock-free (atomic reads only — never acquire mutexes or RwLocks)
//! - O(1) or bounded O(n) where n is small and fixed (4 decks, 4 stems)
//! - Free of allocations (no Vec::new, String::format, etc.)
//!
//! **DO NOT** add any of the following to this handler:
//! - Database queries, file I/O, or network calls
//! - Suggestion recomputation or collection queries
//! - Heavy string formatting or logging (use log::debug! sparingly, log::info! never)
//! - Anything that can be triggered by a discrete event instead (use message passing)
//!
//! If you need periodic-but-infrequent work (e.g. every ~1s), use a debounced
//! `Task::perform(tokio::time::sleep(...))` pattern instead. See `ScheduleSuggestionRefresh`
//! in `browser.rs` for the canonical example.
//!
//! Current tick responsibilities:
//! - MIDI input polling and routing (frame-synced — low-latency input)
//! - MIDI Learn event capture (frame-synced — responsive capture)
//! - Atomic state synchronization: deck positions, slicer, linked stems (frame-synced — smooth waveforms)
//!
//! Moved to separate subscriptions:
//! - LED feedback → `led_feedback.rs` (30Hz timer, `Message::UpdateLeds`)

use iced::Task;

use mesh_widgets::ZoomedViewMode;
use crate::ui::app::MeshApp;
use crate::ui::message::Message;

/// Handle the tick message (called ~60fps).
///
/// **WARNING**: This is a hot path. Read the module-level docs before adding code here.
/// Prefer event-driven message passing for anything that doesn't need per-frame updates.
pub fn handle(app: &mut MeshApp) -> Task<Message> {
    // FPS counter: increment each frame, update display value once per second
    app.fps_frame_count += 1;
    if app.fps_last_second.elapsed() >= std::time::Duration::from_secs(1) {
        app.fps_display = app.fps_frame_count;
        app.fps_frame_count = 0;
        app.fps_last_second = std::time::Instant::now();
    }

    // Poll MIDI input (non-blocking)
    // MIDI messages are processed at 60fps, providing ~16ms latency
    // Collect first to release borrow before calling handle_midi_message
    let midi_messages: Vec<_> = app
        .controller
        .as_mut()
        .map(|m| m.drain())
        .unwrap_or_default();

    // Collect Tasks from MIDI message handling (most return Task::none, but scroll needs it)
    let mut midi_tasks = Vec::new();
    for midi_msg in midi_messages {
        let task = app.handle_midi_message(midi_msg);
        midi_tasks.push(task);
    }

    // MIDI Learn mode: capture raw events when waiting for input
    if app.midi_learn.is_active {
        poll_midi_learn_events(app);
    }

    // Read master clipper clip indicator (LOCK-FREE)
    // Swap to false so we know if any new clipping happens next tick
    if let Some(ref clip_indicator) = app.clip_indicator {
        if clip_indicator.swap(false, std::sync::atomic::Ordering::Relaxed) {
            // Clipping detected — set hold timer (~150ms at 60fps = 9 frames)
            app.clip_hold_frames = 9;
        }
    }
    // Decrement hold timer each tick
    app.clip_hold_frames = app.clip_hold_frames.saturating_sub(1);

    // Read deck positions from atomics (LOCK-FREE - never blocks audio thread)
    // Position/state reads happen ~60Hz with zero contention
    if let Some(ref atomics) = app.deck_atomics {
        for i in 0..4 {
            let position = atomics[i].position();
            let is_playing = atomics[i].is_playing() || atomics[i].is_cueing();
            let timestamp_ns = atomics[i].position_timestamp_ns();
            let playback_rate = atomics[i].playback_rate();
            let loop_active = atomics[i].loop_active();
            let loop_start = atomics[i].loop_start();
            let loop_end = atomics[i].loop_end();
            let is_master = atomics[i].is_master();

            // Update playhead state with audio-thread timestamp for smooth interpolation
            app.player_canvas_state.set_playhead(
                i, position, is_playing, timestamp_ns, playback_rate,
            );

            // Update master status for UI indicator
            app.player_canvas_state.set_master(i, is_master);

            // Update key matching state for header display
            let key_match_enabled = atomics[i].key_match_enabled.load(std::sync::atomic::Ordering::Relaxed);
            let current_transpose = atomics[i].current_transpose.load(std::sync::atomic::Ordering::Relaxed);
            app.player_canvas_state.set_key_match_enabled(i, key_match_enabled);
            app.player_canvas_state.set_transpose(i, current_transpose);

            // Visual LUFS gain: always scale waveforms to -9 LUFS for full vertical fill.
            // Computed directly from track's measured LUFS, independent of audio target.
            let visual_lufs_gain = match atomics[i].track_lufs() {
                Some(track_lufs) => 10.0_f32.powf((-9.0 - track_lufs) / 20.0),
                None => 1.0,
            };
            app.player_canvas_state.decks[i].zoomed.set_lufs_gain(visual_lufs_gain);

            // Read precomputed gain dB from atomics (computed once on track load, not per tick)
            app.player_canvas_state.set_lufs_gain_db(i, atomics[i].lufs_gain_db());

            // Update deck view state from atomics
            app.deck_views[i].sync_play_state(atomics[i].play_state());
            app.deck_views[i].sync_loop_length_index(atomics[i].loop_length_index());

            // Sync loop length and active state to canvas
            let has_track = app.player_canvas_state.decks[i].overview.has_track;
            if has_track {
                app.player_canvas_state.set_loop_length_beats(i, Some(app.deck_views[i].loop_length_beats()));
                app.player_canvas_state.set_loop_active(i, loop_active);
            } else {
                app.player_canvas_state.set_loop_length_beats(i, None);
                app.player_canvas_state.set_loop_active(i, false);
            }

            // Sync channel volume from mixer for waveform dimming
            app.player_canvas_state.set_volume(i, app.mixer_view.channel_volume(i));

            // Sync stem active states to canvas
            // Check if any stem is soloed
            let any_soloed = (0..4).any(|s| app.deck_views[i].is_stem_soloed(s));
            for stem_idx in 0..4 {
                let is_muted = app.deck_views[i].is_stem_muted(stem_idx);
                let is_soloed = app.deck_views[i].is_stem_soloed(stem_idx);
                // If any stem is soloed, only soloed stems are active
                // Otherwise, non-muted stems are active
                let is_active = if any_soloed {
                    is_soloed && !is_muted
                } else {
                    !is_muted
                };
                app.player_canvas_state.set_stem_active(i, stem_idx, is_active);
            }

            // Update position and loop display in waveform
            let duration = app.player_canvas_state.decks[i].overview.duration_samples;
            if duration > 0 {
                let pos_normalized = position as f64 / duration as f64;
                app.player_canvas_state.decks[i]
                    .overview
                    .set_position(pos_normalized);

                if loop_active {
                    let start = loop_start as f64 / duration as f64;
                    let end = loop_end as f64 / duration as f64;
                    app.player_canvas_state.decks[i]
                        .overview
                        .set_loop_region(Some((start, end)));
                    app.player_canvas_state.decks[i]
                        .zoomed
                        .set_loop_region(Some((start, end)));
                } else {
                    app.player_canvas_state.decks[i]
                        .overview
                        .set_loop_region(None);
                    app.player_canvas_state.decks[i]
                        .zoomed
                        .set_loop_region(None);
                }
            }
        }
    }

    // Sync global BPM to canvas for BPM-aligned overview waveforms
    // When multiple decks play at different BPMs, this stretches overview rendering
    // so beat grids align visually across all decks
    let global_bpm = app.domain.global_bpm();
    for i in 0..4 {
        if global_bpm > 0.0 {
            app.player_canvas_state.set_display_bpm(i, Some(global_bpm));
        } else {
            app.player_canvas_state.set_display_bpm(i, None);
        }
    }

    // Sync slicer state from atomics (LOCK-FREE - never blocks audio thread)
    // Updates slicer active state, queue, and current slice for UI display
    if let Some(ref slicer_atomics) = app.slicer_atomics {
        for i in 0..4 {
            let sa = &slicer_atomics[i];
            let active = sa.active.load(std::sync::atomic::Ordering::Relaxed);
            let current_slice = sa.current_slice.load(std::sync::atomic::Ordering::Relaxed);
            let queue = sa.queue();

            // Sync to deck view for button display
            app.deck_views[i].sync_slicer_state(active, current_slice, queue);

            // Sync to canvas for waveform overlay
            let duration = app.player_canvas_state.decks[i].overview.duration_samples;
            if active && duration > 0 {
                let buffer_start = sa.buffer_start.load(std::sync::atomic::Ordering::Relaxed);
                let buffer_end = sa.buffer_end.load(std::sync::atomic::Ordering::Relaxed);

                // Convert to normalized positions
                let start_norm = buffer_start as f64 / duration as f64;
                let end_norm = buffer_end as f64 / duration as f64;

                app.player_canvas_state.decks[i]
                    .overview
                    .set_slicer_region(Some((start_norm, end_norm)), Some(current_slice));
                app.player_canvas_state.decks[i]
                    .zoomed
                    .set_slicer_region(Some((start_norm, end_norm)), Some(current_slice));
                // Set fixed buffer view mode for slicer
                app.player_canvas_state.decks[i]
                    .zoomed
                    .set_fixed_buffer_bounds(Some((buffer_start as u64, buffer_end as u64)));
                app.player_canvas_state.decks[i]
                    .zoomed
                    .set_view_mode(ZoomedViewMode::FixedBuffer);
                // Set zoom level based on slicer buffer size for optimal resolution
                app.player_canvas_state.decks[i]
                    .zoomed
                    .set_fixed_buffer_zoom(app.config.slicer.validated_buffer_bars());
            } else {
                app.player_canvas_state.decks[i]
                    .overview
                    .set_slicer_region(None, None);
                app.player_canvas_state.decks[i]
                    .zoomed
                    .set_slicer_region(None, None);
                // Restore scrolling view mode
                app.player_canvas_state.decks[i]
                    .zoomed
                    .set_fixed_buffer_bounds(None);
                app.player_canvas_state.decks[i]
                    .zoomed
                    .set_view_mode(ZoomedViewMode::Scrolling);
            }
        }
    }

    // Sync linked stem state from atomics (LOCK-FREE - never blocks audio thread)
    // Updates which stems have links and whether links are active for UI display
    if let Some(ref linked_atomics) = app.linked_stem_atomics {
        for i in 0..4 {
            let la = &linked_atomics[i];
            for stem_idx in 0..4 {
                let has_linked = la.has_linked[stem_idx].load(std::sync::atomic::Ordering::Relaxed);
                let is_active = la.use_linked[stem_idx].load(std::sync::atomic::Ordering::Relaxed);
                app.player_canvas_state.set_linked_stem(i, stem_idx, has_linked, is_active);
            }
        }
    }

    // Browser overlay auto-hide countdown (runs every tick at 60Hz)
    if app.browser_hide_countdown > 0 {
        app.browser_hide_countdown -= 1;
        if app.browser_hide_countdown == 0 {
            app.browser_visible = false;
        }
    }

    // Return batched MIDI tasks (scroll operations need to be executed by iced runtime)
    Task::batch(midi_tasks)
}

/// Poll raw MIDI/HID events for MIDI Learn capture.
///
/// Called every frame when learn mode is active. Captures raw controller events,
/// detects hardware type (button vs encoder vs fader), and routes to the appropriate
/// learn phase handler (setup buttons, mapping phases).
fn poll_midi_learn_events(app: &mut MeshApp) {
    use crate::ui::app::{convert_midi_event_to_captured, convert_hid_event_to_captured};
    use crate::ui::midi_learn::{LearnPhase, SetupStep};

    let needs_capture = match app.midi_learn.phase {
        LearnPhase::Setup => {
            // Capture during shift and toggle button steps
            matches!(
                app.midi_learn.setup_step,
                SetupStep::ShiftButtonLeft
                    | SetupStep::ShiftButtonRight
                    | SetupStep::ToggleButtonLeft
                    | SetupStep::ToggleButtonRight
            )
        }
        LearnPhase::Review => false,
        // All other phases need MIDI capture
        _ => true,
    };

    if needs_capture {
        if let Some(ref controller) = app.controller {
            // Check if we're in hardware detection mode (sampling in progress)
            let sampling_active = app.midi_learn.detection_buffer.is_some();

            // Drain raw events with source device info (for port name capture)
            for (raw_event, source_port) in controller.drain_raw_events_with_source() {
                let captured = convert_midi_event_to_captured(&raw_event);

                // Capture the port name on first event (for device identification)
                if app.midi_learn.captured_port_name.is_none() {
                    log::info!("MIDI Learn: Captured source port '{}'", source_port);
                    app.midi_learn.captured_port_name = Some(source_port);
                }

                // Always update display so user sees what's happening
                app.midi_learn.last_captured = Some(captured.clone());

                if sampling_active {
                    // Add sample to detection buffer
                    if app.midi_learn.add_detection_sample(&captured) {
                        // Buffer is complete - finalize mapping
                        app.midi_learn.finalize_mapping();
                        break;
                    }
                } else {
                    // Not sampling yet - check if we should start
                    if !app.midi_learn.should_capture(&captured) {
                        continue;
                    }

                    app.midi_learn.mark_captured();

                    if app.midi_learn.phase == LearnPhase::Setup {
                        match app.midi_learn.setup_step {
                            SetupStep::ShiftButtonLeft => {
                                app.midi_learn.shift_mapping_left = Some(captured);
                            }
                            SetupStep::ShiftButtonRight => {
                                app.midi_learn.shift_mapping_right = Some(captured);
                            }
                            SetupStep::ToggleButtonLeft => {
                                app.midi_learn.toggle_mapping_left = Some(captured);
                            }
                            SetupStep::ToggleButtonRight => {
                                app.midi_learn.toggle_mapping_right = Some(captured);
                            }
                            _ => {}
                        }
                        app.midi_learn.advance();
                    } else {
                        app.midi_learn.record_mapping(captured);
                    }

                    break; // Only start one capture per tick
                }
            }

            // Check if detection timed out (1 second elapsed)
            if app.midi_learn.is_detection_complete() {
                app.midi_learn.finalize_mapping();
            }

            // HID event capture for learn mode
            if !sampling_active {
                for hid_event in controller.drain_hid_events() {
                    let descriptor = controller.hid_descriptor_for(&hid_event.address);
                    let device_name = controller.first_hid_device_name().unwrap_or("HID Device");
                    let captured = convert_hid_event_to_captured(
                        &hid_event,
                        descriptor,
                        device_name,
                    );

                    if app.midi_learn.captured_port_name.is_none() {
                        if let Some(ref name) = captured.source_device {
                            log::info!("Learn: Captured HID source device '{}'", name);
                            app.midi_learn.captured_port_name = Some(name.to_string());
                        }
                    }

                    log::info!("[HID Learn] Captured: {}", captured.display());
                    app.midi_learn.last_captured = Some(captured.clone());

                    if !app.midi_learn.should_capture(&captured) {
                        continue;
                    }

                    log::info!("[HID Learn] Accepted: {} (phase={:?})", captured.display(), app.midi_learn.phase);
                    app.midi_learn.mark_captured();

                    if app.midi_learn.phase == LearnPhase::Setup {
                        match app.midi_learn.setup_step {
                            SetupStep::ShiftButtonLeft => {
                                app.midi_learn.shift_mapping_left = Some(captured);
                            }
                            SetupStep::ShiftButtonRight => {
                                app.midi_learn.shift_mapping_right = Some(captured);
                            }
                            SetupStep::ToggleButtonLeft => {
                                app.midi_learn.toggle_mapping_left = Some(captured);
                            }
                            SetupStep::ToggleButtonRight => {
                                app.midi_learn.toggle_mapping_right = Some(captured);
                            }
                            _ => {}
                        }
                        app.midi_learn.advance();
                    } else {
                        app.midi_learn.record_mapping(captured);
                    }

                    break; // One capture per tick
                }
            } else {
                // Sampling active (MIDI detection in progress) — drain HID to prevent overflow
                for _hid_event in controller.drain_hid_events() {}
            }
        }
    }

    // Update highlight targets for views (only when learn is active)
    let highlight = app.midi_learn.highlight_target;
    for i in 0..4 {
        app.deck_views[i].set_highlight(highlight);
    }
    app.mixer_view.set_highlight(highlight);
}
