//! Tick message handler
//!
//! Handles the 60fps periodic tick for:
//! - MIDI input polling and routing
//! - MIDI Learn event capture
//! - Atomic state synchronization (deck positions, slicer, linked stems)
//! - Zoomed waveform peak recomputation requests
//! - MIDI LED feedback updates

use iced::Task;

use mesh_widgets::{PeaksComputeRequest, ZoomedViewMode};
use crate::ui::app::{MeshApp, convert_midi_event_to_captured};
use crate::ui::message::Message;
use crate::ui::midi_learn::{LearnPhase, SetupStep};

/// Handle the tick message (called ~60fps)
pub fn handle(app: &mut MeshApp) -> Task<Message> {
    // Poll MIDI input (non-blocking)
    // MIDI messages are processed at 60fps, providing ~16ms latency
    // Collect first to release borrow before calling handle_midi_message
    let midi_messages: Vec<_> = app
        .midi_controller
        .as_ref()
        .map(|m| m.drain().collect())
        .unwrap_or_default();

    // Collect Tasks from MIDI message handling (most return Task::none, but scroll needs it)
    let mut midi_tasks = Vec::new();
    for midi_msg in midi_messages {
        let task = app.handle_midi_message(midi_msg);
        midi_tasks.push(task);
    }

    // MIDI Learn mode: capture raw events when waiting for input
    // This happens before normal MIDI routing so we can intercept events
    if app.midi_learn.is_active {
        let needs_capture = match app.midi_learn.phase {
            LearnPhase::Setup => {
                // Only capture during ShiftButton step
                app.midi_learn.setup_step == SetupStep::ShiftButton
            }
            LearnPhase::Review => false,
            // All other phases need MIDI capture
            _ => true,
        };

        if needs_capture {
            if let Some(ref controller) = app.midi_controller {
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

                        // Check if this event should be captured (debounce + Note Off filter)
                        if !app.midi_learn.should_capture(&captured) {
                            continue; // Skip this event, check next
                        }

                        // Mark capture time for debouncing
                        app.midi_learn.mark_captured();

                        // Handle based on current phase
                        if app.midi_learn.phase == LearnPhase::Setup {
                            // Shift button detection - auto-advance
                            app.midi_learn.shift_mapping = Some(captured);
                            app.midi_learn.advance();
                        } else {
                            // Mapping phase - start hardware detection
                            // record_mapping creates buffer, adds first sample
                            // For buttons (Note events), it completes immediately
                            app.midi_learn.record_mapping(captured);
                        }

                        // Only start one capture per tick
                        break;
                    }
                }

                // Check if detection timed out (1 second elapsed)
                if app.midi_learn.is_detection_complete() {
                    app.midi_learn.finalize_mapping();
                }
            }
        }
    }

    // Update highlight targets for MIDI learn mode
    // Each deck/mixer view needs to know if one of its elements should be highlighted
    let highlight = app.midi_learn.highlight_target;
    for i in 0..4 {
        app.deck_views[i].set_highlight(highlight);
    }
    app.mixer_view.set_highlight(highlight);

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
    let mut deck_positions: [Option<u64>; 4] = [None; 4];

    if let Some(ref atomics) = app.deck_atomics {
        for i in 0..4 {
            let position = atomics[i].position();
            let is_playing = atomics[i].is_playing();
            let loop_active = atomics[i].loop_active();
            let loop_start = atomics[i].loop_start();
            let loop_end = atomics[i].loop_end();
            let is_master = atomics[i].is_master();

            deck_positions[i] = Some(position);

            // Update playhead state for smooth interpolation
            app.player_canvas_state.set_playhead(i, position, is_playing);

            // Update master status for UI indicator
            app.player_canvas_state.set_master(i, is_master);

            // Update key matching state for header display
            let key_match_enabled = atomics[i].key_match_enabled.load(std::sync::atomic::Ordering::Relaxed);
            let current_transpose = atomics[i].current_transpose.load(std::sync::atomic::Ordering::Relaxed);
            app.player_canvas_state.set_key_match_enabled(i, key_match_enabled);
            app.player_canvas_state.set_transpose(i, current_transpose);

            // Sync LUFS gain from engine for waveform scaling (single source of truth)
            let lufs_gain = atomics[i].lufs_gain();
            app.player_canvas_state.decks[i].zoomed.set_lufs_gain(lufs_gain);

            // Update gain display in deck header (dB)
            let lufs_gain_db = if (lufs_gain - 1.0).abs() < 0.001 {
                None // No gain compensation (unity or disabled)
            } else {
                Some(20.0 * lufs_gain.log10())
            };
            app.player_canvas_state.set_lufs_gain_db(i, lufs_gain_db);

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

    // Request zoomed waveform peak recomputation in background thread
    // This expensive operation (10-50ms) is fully async - UI never blocks
    for i in 0..4 {
        if let Some(position) = deck_positions[i] {
            // Get linked stem active state from atomics (needed for cache invalidation check)
            let linked_active = if let Some(ref linked_atomics) = app.linked_stem_atomics {
                let la = &linked_atomics[i];
                [
                    la.has_linked[0].load(std::sync::atomic::Ordering::Relaxed)
                        && la.use_linked[0].load(std::sync::atomic::Ordering::Relaxed),
                    la.has_linked[1].load(std::sync::atomic::Ordering::Relaxed)
                        && la.use_linked[1].load(std::sync::atomic::Ordering::Relaxed),
                    la.has_linked[2].load(std::sync::atomic::Ordering::Relaxed)
                        && la.use_linked[2].load(std::sync::atomic::Ordering::Relaxed),
                    la.has_linked[3].load(std::sync::atomic::Ordering::Relaxed)
                        && la.use_linked[3].load(std::sync::atomic::Ordering::Relaxed),
                ]
            } else {
                [false, false, false, false]
            };

            let zoomed = &app.player_canvas_state.decks[i].zoomed;
            if zoomed.needs_recompute(position, &linked_active) && zoomed.has_track {
                if let Some(ref stems) = app.domain.deck_stems()[i] {
                    // Clone linked stem buffer references (cheap Shared clone)
                    let linked_stems = [
                        app.domain.deck_linked_stem(i, 0).cloned(),
                        app.domain.deck_linked_stem(i, 1).cloned(),
                        app.domain.deck_linked_stem(i, 2).cloned(),
                        app.domain.deck_linked_stem(i, 3).cloned(),
                    ];

                    let _ = app.domain.request_peaks_compute(PeaksComputeRequest {
                        id: i,
                        playhead: position,
                        stems: stems.clone(),
                        width: 1600,
                        zoom_bars: zoomed.zoom_bars,
                        duration_samples: zoomed.duration_samples,
                        bpm: zoomed.bpm,
                        view_mode: zoomed.view_mode,
                        fixed_buffer_bounds: zoomed.fixed_buffer_bounds,
                        linked_stems,
                        linked_active,
                    });
                }
            }
        }
    }

    // Update MIDI LED feedback (send state to controller LEDs)
    // Only runs if controller has output connection and feedback mappings
    if let Some(ref mut controller) = app.midi_controller {
        let mut feedback = mesh_midi::FeedbackState::default();

        for deck_idx in 0..4 {
            // Get play state and loop active from atomics
            if let Some(ref atomics) = app.deck_atomics {
                feedback.decks[deck_idx].is_playing = atomics[deck_idx].is_playing();
                feedback.decks[deck_idx].loop_active = atomics[deck_idx].loop_active();
                feedback.decks[deck_idx].key_match_enabled =
                    atomics[deck_idx].key_match_enabled.load(std::sync::atomic::Ordering::Relaxed);
            }

            // Get slicer state
            if let Some(ref slicer_atomics) = app.slicer_atomics {
                feedback.decks[deck_idx].slicer_active =
                    slicer_atomics[deck_idx].active.load(std::sync::atomic::Ordering::Relaxed);
                feedback.decks[deck_idx].slicer_current_slice =
                    slicer_atomics[deck_idx].current_slice.load(std::sync::atomic::Ordering::Relaxed);
            }

            // Get deck view state (hot cues, slip, stem mutes, action mode)
            feedback.decks[deck_idx].hot_cues_set = app.deck_views[deck_idx].hot_cues_bitmap();
            feedback.decks[deck_idx].slip_active = app.deck_views[deck_idx].slip_enabled();
            feedback.decks[deck_idx].stems_muted = app.deck_views[deck_idx].stems_muted_bitmap();

            // Set action mode for LED feedback
            use crate::ui::deck_view::ActionButtonMode;
            feedback.decks[deck_idx].action_mode = match app.deck_views[deck_idx].action_mode() {
                ActionButtonMode::HotCue => mesh_midi::ActionMode::HotCue,
                ActionButtonMode::Slicer => mesh_midi::ActionMode::Slicer,
            };

            // Get mixer cue (PFL) state
            feedback.mixer[deck_idx].cue_enabled = app.mixer_view.cue_enabled(deck_idx);
        }

        controller.update_feedback(&feedback);
    }

    // Return batched MIDI tasks (scroll operations need to be executed by iced runtime)
    Task::batch(midi_tasks)
}
