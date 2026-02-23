//! Track loading result handlers
//!
//! Handles async results from the track loading pipeline:
//! - TrackLoaded: Main track loaded with stems and metadata
//! - LinkedStemLoaded: Linked stem loaded from another track

use std::sync::Arc;
use iced::Task;

use crate::ui::app::MeshApp;
use crate::ui::message::Message;
use crate::ui::state::{LinkedStemLoadedMsg, StemLinkState, TrackLoadedMsg};

/// Handle track loaded message (streaming: RegionLoaded, Complete, or Error)
pub fn handle_track_loaded(app: &mut MeshApp, msg: TrackLoadedMsg) -> Task<Message> {
    // Extract the result from Arc wrapper
    let result = match Arc::try_unwrap(msg.0) {
        Ok(r) => r,
        Err(_arc) => {
            log::warn!("TrackLoadResult Arc still shared, skipping");
            return Task::none();
        }
    };

    use crate::loader::TrackLoadResult;

    match result {
        TrackLoadResult::RegionLoaded { deck_idx, stems, duration_samples,
                                         overview_peaks, highres_peaks, path } => {
            // Stale check: a different track may have been loaded since
            let path_str = path.to_string_lossy().to_string();
            if app.deck_views[deck_idx].loaded_track_path() != Some(path_str.as_str()) {
                return Task::none();
            }

            // Upgrade engine stems if this message carries a buffer snapshot
            if let Some(stems) = stems {
                app.domain.upgrade_loaded_stems(deck_idx, stems, duration_samples);
            }

            // Update overview waveform peaks (visual growth effect)
            // Rebuild GPU peak buffers so the shader reflects incremental loading
            let overview = &mut app.player_canvas_state.decks[deck_idx].overview;
            overview.overview_peak_buffer =
                mesh_widgets::PeakBuffer::from_stem_peaks(&overview_peaks);
            overview.highres_peak_buffer =
                mesh_widgets::PeakBuffer::from_stem_peaks(&highres_peaks);
            overview.stem_waveforms = overview_peaks;
            overview.highres_peaks = highres_peaks;
            // First audio data arrived — stop loading pulse (user can play now)
            overview.loading = false;
            Task::none()
        }

        TrackLoadResult::Complete { deck_idx, result: track_result, overview_state, zoomed_state,
                                     stems, duration_samples, path, incremental } => {
            // Stale check
            let path_str = path.to_string_lossy().to_string();
            if app.deck_views[deck_idx].loaded_track_path() != Some(path_str.as_str()) {
                log::info!("[TRACK] Discarding stale Complete for deck {}", deck_idx + 1);
                return Task::none();
            }

            match track_result {
                Ok(_prepared) => {
                    let filename = path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Unknown")
                        .to_string();

                    if !incremental {
                        // Full-load path (resampling): deliver stems and replace waveform state
                        app.domain.upgrade_loaded_stems(deck_idx, stems, duration_samples);

                        // Preserve linked stem data that may have arrived from async loader
                        // before this Complete message (race: LinkedStemLoaded can beat Complete)
                        let old_overview = &app.player_canvas_state.decks[deck_idx].overview;
                        let linked_waveforms = old_overview.linked_stem_waveforms.clone();
                        let linked_drops = old_overview.linked_drop_markers;
                        let linked_durs = old_overview.linked_durations;
                        let linked_hr = old_overview.linked_highres_peaks.clone();
                        let linked_gains = old_overview.linked_lufs_gains;

                        let overview = &mut app.player_canvas_state.decks[deck_idx].overview;
                        *overview = overview_state;

                        // Restore linked stem data and rebuild GPU buffers if any were present
                        let has_linked = linked_waveforms.iter().any(|o| o.is_some());
                        if has_linked {
                            overview.linked_stem_waveforms = linked_waveforms;
                            overview.linked_drop_markers = linked_drops;
                            overview.linked_durations = linked_durs;
                            overview.linked_highres_peaks = linked_hr;
                            overview.linked_lufs_gains = linked_gains;
                            overview.rebuild_linked_buffers();
                        }

                        app.player_canvas_state.decks[deck_idx].zoomed = zoomed_state;

                        // Apply user display config
                        app.player_canvas_state.decks[deck_idx]
                            .overview.set_grid_bars(app.config.display.grid_bars);
                        app.player_canvas_state.decks[deck_idx]
                            .zoomed.set_zoom(app.config.display.default_zoom_bars);
                    }
                    // else: incremental path — stems already current from last RegionLoaded,
                    // overview/zoomed already built by skeleton + incremental peak updates

                    app.deck_views[deck_idx].set_audio_loading(false);
                    app.status = format!("Loaded {} to deck {}", filename, deck_idx + 1);
                    log::info!("[TRACK] Full audio ready for deck {}{}", deck_idx + 1,
                        if incremental { " (incremental)" } else { "" });

                    // Schedule debounced suggestion refresh
                    return Task::done(Message::ScheduleSuggestionRefresh);
                }
                Err(e) => {
                    app.deck_views[deck_idx].set_audio_loading(false);
                    log::error!("Failed to load track to deck {}: {}", deck_idx + 1, e);
                    app.status = format!("Error loading track: {}", e);
                }
            }
            Task::none()
        }

        TrackLoadResult::Error { deck_idx, error } => {
            app.deck_views[deck_idx].set_audio_loading(false);
            log::error!("Failed to load track to deck {}: {}", deck_idx + 1, error);
            app.status = format!("Error loading audio: {}", error);
            Task::none()
        }
    }
}

/// Handle linked stem loaded message (stem from another track)
pub fn handle_linked_stem_loaded(app: &mut MeshApp, msg: LinkedStemLoadedMsg) -> Task<Message> {
    // Extract the result from Arc wrapper
    let linked_result = match Arc::try_unwrap(msg.0) {
        Ok(r) => r,
        Err(_) => {
            log::warn!("LinkedStemLoadResult Arc still shared, skipping");
            return Task::none();
        }
    };

    let deck_idx = linked_result.host_deck_idx;
    let stem_idx = linked_result.stem_idx;

    match linked_result.result {
        Ok(linked_data) => {
            // Store shared buffer reference for zoomed waveform visualization
            if let Some(shared_buffer) = linked_result.shared_buffer {
                log::info!(
                    "[LINKED] Storing shared buffer for deck {} stem {} ({} samples)",
                    deck_idx,
                    stem_idx,
                    shared_buffer.len()
                );
                app.domain.set_deck_linked_stem(deck_idx, stem_idx, Some(shared_buffer));
            }

            // Store linked stem overview peaks in waveform state for visualization
            if let Some(peaks) = linked_result.overview_peaks {
                log::info!(
                    "[LINKED] Storing {} overview peaks for deck {} stem {}",
                    peaks.len(),
                    deck_idx,
                    stem_idx
                );
                app.player_canvas_state
                    .deck_mut(deck_idx)
                    .overview
                    .set_linked_stem_peaks(stem_idx, peaks);
            }

            // Store linked stem highres peaks for stable zoomed view rendering
            if let Some(peaks) = linked_result.highres_peaks {
                log::info!(
                    "[LINKED] Storing {} highres peaks for deck {} stem {}",
                    peaks.len(),
                    deck_idx,
                    stem_idx
                );
                app.player_canvas_state
                    .deck_mut(deck_idx)
                    .overview
                    .set_linked_highres_peaks(stem_idx, peaks);
            }

            // Store linked stem metadata for split-view alignment
            // Use STRETCHED values to match audio engine alignment
            if let Some(stretched_duration) = linked_result.linked_duration {
                let host_duration = app.player_canvas_state.decks[deck_idx].overview.duration_samples;
                let host_drop = app.player_canvas_state.decks[deck_idx].overview.drop_marker;
                log::info!(
                    "[LINKED] Visual alignment for deck {} stem {}: stretched_drop={}, stretched_dur={}, host_drop={:?}, host_dur={}, ratio={:.3}",
                    deck_idx,
                    stem_idx,
                    linked_data.drop_marker,
                    stretched_duration,
                    host_drop,
                    host_duration,
                    stretched_duration as f64 / host_duration as f64
                );
                app.player_canvas_state
                    .deck_mut(deck_idx)
                    .overview
                    .set_linked_stem_metadata(
                        stem_idx,
                        linked_data.drop_marker,
                        stretched_duration,
                    );
            }

            // Visual LUFS gain for linked stem: normalize to host track's level.
            // The shader already applies height_scale = 10^((-9 - host_lufs) / 20)
            // uniformly to all stems (original and linked). So we only need to
            // normalize linked→host here; the shader handles host→-9 LUFS.
            // Using -9→linked directly would cause double correction.
            let host_lufs = app.domain.track_lufs(deck_idx);
            let linked_gain = match (host_lufs, linked_data.lufs) {
                (Some(host), Some(linked)) => 10.0_f32.powf((host - linked) / 20.0),
                _ => 1.0,
            };
            app.player_canvas_state
                .deck_mut(deck_idx)
                .overview
                .set_linked_lufs_gain(stem_idx, linked_gain);
            log::info!(
                "[LINKED] Set visual LUFS gain for deck {} stem {}: host_lufs={:?}, linked_lufs={:?}, gain={:.3} ({:+.1}dB)",
                deck_idx,
                stem_idx,
                host_lufs,
                linked_data.lufs,
                linked_gain,
                20.0 * linked_gain.log10()
            );

            // Mark stem as having a linked stem (enables split-view)
            app.player_canvas_state.set_linked_stem(deck_idx, stem_idx, true, false);

            // Send linked stem to audio engine via domain
            if let Some(stem) = mesh_core::types::Stem::from_index(stem_idx) {
                let track_name = linked_data.track_name.clone();
                let host_lufs = app.domain.track_lufs(deck_idx);
                log::info!(
                    "[LUFS] app.rs LinkStem: deck {} stem {} - host_lufs={:?}, linked_lufs={:?}",
                    deck_idx,
                    stem_idx,
                    host_lufs,
                    linked_data.lufs
                );
                app.domain.link_stem(deck_idx, stem, linked_data, host_lufs);
                app.status = format!(
                    "Linked {} stem on deck {} from {}",
                    stem.name(),
                    deck_idx + 1,
                    track_name
                );
            }

            // Transition from Loading to Idle - linked stem is ready
            if matches!(
                app.stem_link_state,
                StemLinkState::Loading { deck, stem, .. }
                if deck == deck_idx && stem == stem_idx
            ) {
                app.stem_link_state = StemLinkState::Idle;
                log::info!(
                    "Linked stem ready: deck={}, stem={} - shift+stem to toggle",
                    deck_idx,
                    stem_idx
                );
            }
        }
        Err(e) => {
            app.status = format!("Error loading linked stem: {}", e);
            app.stem_link_state = StemLinkState::Idle;
        }
    }

    Task::none()
}
