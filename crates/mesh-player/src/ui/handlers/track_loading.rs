//! Track loading result handlers
//!
//! Handles async results from the track loading pipeline:
//! - TrackLoaded: Main track loaded with stems and metadata
//! - PeaksComputed: Background waveform peak computation complete
//! - LinkedStemLoaded: Linked stem loaded from another track

use std::sync::Arc;
use iced::Task;

use crate::ui::app::MeshApp;
use crate::ui::handlers::browser::trigger_suggestion_query;
use crate::ui::message::Message;
use crate::ui::state::{LinkedStemLoadedMsg, StemLinkState, TrackLoadedMsg};

/// Handle track loaded message (main track with stems)
pub fn handle_track_loaded(app: &mut MeshApp, msg: TrackLoadedMsg) -> Task<Message> {
    // Extract the result from Arc wrapper
    // We use Arc::try_unwrap to get ownership if we're the sole owner,
    // otherwise we need to handle the shared case
    let result = match Arc::try_unwrap(msg.0) {
        Ok(r) => r,
        Err(_arc) => {
            // Still shared - this shouldn't happen in practice since
            // subscriptions deliver to one handler, but handle gracefully
            log::warn!("TrackLoadResult Arc still shared, skipping");
            return Task::none();
        }
    };

    let deck_idx = result.deck_idx;

    match result.result {
        Ok(prepared) => {
            let track = &prepared.track;
            let filename = track.path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown")
                .to_string();

            log::info!(
                "[TRACK] Loaded {} to deck {} ({} samples)",
                filename,
                deck_idx + 1,
                track.duration_samples
            );

            // ─────────────────────────────────────────────────
            // UI State Updates (waveforms, display)
            // ─────────────────────────────────────────────────

            // Apply pre-computed waveform states (expensive work already done in loader)
            app.player_canvas_state.decks[deck_idx].overview = result.overview_state;
            app.player_canvas_state.decks[deck_idx].zoomed = result.zoomed_state;

            // Set track info on player canvas state (for header display)
            app.player_canvas_state.set_track_name(deck_idx, filename.clone());
            app.player_canvas_state.set_track_key(
                deck_idx,
                track.metadata.key.clone().unwrap_or_default()
            );
            app.player_canvas_state.set_track_bpm(deck_idx, track.metadata.bpm);

            // Sync hot cues to deck view for display
            for (slot, hot_cue) in prepared.hot_cues.iter().enumerate() {
                app.deck_views[deck_idx].set_hot_cue_position(
                    slot,
                    hot_cue.as_ref().map(|hc| hc.position as u64)
                );
            }

            // Store loaded track path for suggestion seed queries
            app.deck_views[deck_idx].set_loaded_track_path(
                Some(track.path.to_string_lossy().to_string())
            );

            // Reset stem mute/solo state for the deck (all stems active)
            for stem_idx in 0..4 {
                app.deck_views[deck_idx].set_stem_muted(stem_idx, false);
                app.deck_views[deck_idx].set_stem_soloed(stem_idx, false);
                app.player_canvas_state.set_stem_active(deck_idx, stem_idx, true);
                // Clear linked stem visual state
                app.player_canvas_state.set_linked_stem(deck_idx, stem_idx, false, false);
            }

            // ─────────────────────────────────────────────────
            // Domain State Updates (encapsulated)
            // ─────────────────────────────────────────────────

            // Domain handles: stem buffers, LUFS cache, linked stem cleanup, engine send
            app.domain.apply_loaded_track(
                deck_idx,
                result.stems,
                track.metadata.lufs,
                prepared,
            );

            app.status = format!("Loaded {} to deck {}", filename, deck_idx + 1);

            // Auto-refresh suggestions if enabled (new seed available)
            if app.collection_browser.is_suggestions_enabled() {
                app.collection_browser.set_suggestion_loading(true);
                return trigger_suggestion_query(app);
            }
        }
        Err(e) => {
            log::error!("Failed to load track to deck {}: {}", deck_idx + 1, e);
            app.status = format!("Error loading track: {}", e);
        }
    }

    Task::none()
}

/// Handle peaks computed message (background waveform computation)
pub fn handle_peaks_computed(app: &mut MeshApp, result: mesh_widgets::PeaksComputeResult) -> Task<Message> {
    // Apply computed peaks to the appropriate deck's zoomed waveform state
    if result.id < 4 {
        let zoomed = &mut app.player_canvas_state.decks[result.id].zoomed;
        zoomed.apply_computed_peaks(result);
    }
    Task::none()
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

            // Calculate LUFS gain for linked stem waveform
            let linked_gain = app.config.audio.loudness.calculate_gain_linear(linked_data.lufs);
            app.player_canvas_state
                .deck_mut(deck_idx)
                .overview
                .set_linked_lufs_gain(stem_idx, linked_gain);
            log::info!(
                "[LINKED] Set LUFS gain for deck {} stem {}: linked_lufs={:?}, gain={:.3} ({:+.1}dB)",
                deck_idx,
                stem_idx,
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
