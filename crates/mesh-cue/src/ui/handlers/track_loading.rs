//! Track loading message handlers
//!
//! Handles progressive track loading with region-based streaming:
//! 1. TrackMetadataLoaded - Fast metadata from database, shows UI immediately
//! 2. CueTrackLoaded(RegionLoaded) - Incremental peak updates as regions load
//! 3. CueTrackLoaded(Complete) - All audio loaded, finalize state
//!
//! Also handles:
//! - LinkedStemLoaded - When a linked stem finishes loading
//! - TrackStemsLoaded - Legacy fallback (kept for safety)

use iced::Task;
use mesh_core::audio_file::{BeatGrid, LoadedTrack, TrackMetadata};
use mesh_core::types::{Stem, SAMPLE_RATE};
use mesh_widgets::SliceEditorState;
use std::sync::Arc;

use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::state::{CueTrackLoadedMsg, LinkedStemLoadedMsg, LoadedTrackState, StemsLoadResult};
use super::super::waveform::{
    CombinedWaveformView, WaveformView, ZoomedWaveformView,
};
use crate::loader::CueTrackLoadResult;

impl MeshCueApp {
    /// Handle TrackMetadataLoaded message
    ///
    /// Phase 1 of track loading: metadata loaded from database.
    /// Shows UI immediately while audio loads progressively in background.
    pub fn handle_track_metadata_loaded(
        &mut self,
        result: Result<(std::path::PathBuf, TrackMetadata), String>,
    ) -> Task<Message> {
        match result {
            Ok((path, metadata)) => {
                log::info!("TrackMetadataLoaded: Showing UI, starting progressive audio load");
                let bpm = metadata.bpm.unwrap_or(120.0);
                let key = metadata.key.clone().unwrap_or_else(|| "?".to_string());
                let cue_points = metadata.cue_points.clone();
                let beat_grid = metadata.beat_grid.beats.clone();

                // Create combined waveform view (both zoomed + overview in single canvas)
                let mut combined_waveform = CombinedWaveformView::new();
                // Initialize overview with beat markers from metadata
                combined_waveform.overview = WaveformView::from_metadata(&metadata, 0);
                // Apply grid density from config
                combined_waveform
                    .overview
                    .set_grid_bars(self.domain.config().display.grid_bars);
                // Initialize zoomed view (peaks will be computed when stems load)
                combined_waveform.zoomed = ZoomedWaveformView::from_metadata(
                    bpm,
                    beat_grid.clone(),
                    Vec::new(), // Cue markers will be added after duration is known
                );
                // Set drop marker on zoomed view (overview gets it from from_metadata)
                combined_waveform.zoomed.set_drop_marker(metadata.drop_marker);

                self.collection.loaded_track = Some(LoadedTrackState {
                    path: path.clone(),
                    track: None,
                    stems: None,
                    cue_points,
                    saved_loops: metadata.saved_loops.clone(),
                    bpm,
                    key,
                    beat_grid,
                    drop_marker: metadata.drop_marker,
                    lufs: metadata.lufs,
                    stem_links: metadata.stem_links.clone(),
                    duration_samples: 0, // Will be set when first audio arrives
                    modified: false,
                    combined_waveform,
                    loading_audio: true,
                    deck_atomics: self.audio.deck_atomics().clone(),
                    slice_editor: {
                        // Load presets from dedicated file (shared with mesh-player)
                        let slicer_config =
                            mesh_widgets::load_slicer_presets(&self.collection.collection_path);
                        let mut editor = SliceEditorState::new();
                        slicer_config.apply_to_editor_state(&mut editor);
                        editor
                    },
                });

                // Phase 2: Send request to background loader for progressive loading
                // The loader reads priority regions first (hot cues, drop, first beat)
                // then fills gaps — sending incremental peak updates as it goes.
                if let Err(e) = self.track_loader.load(path, metadata) {
                    log::error!("Failed to send track load request: {}", e);
                }

                Task::none()
            }
            Err(e) => {
                log::error!("Failed to load track metadata: {}", e);
                Task::none()
            }
        }
    }

    /// Handle CueTrackLoaded message (progressive loading results)
    ///
    /// Processes incremental region loads and final completion from CueTrackLoader.
    pub fn handle_cue_track_loaded(&mut self, msg: CueTrackLoadedMsg) -> Task<Message> {
        // Extract the result from Arc wrapper
        let result = match Arc::try_unwrap(msg.0) {
            Ok(r) => r,
            Err(arc) => {
                // Arc still shared (e.g. subscription held a ref) — clone the inner value
                // This shouldn't normally happen with mpsc_subscription
                log::debug!("CueTrackLoadResult Arc still shared, using ref");
                return self.handle_cue_track_result(&*arc);
            }
        };

        self.handle_cue_track_result(&result)
    }

    fn handle_cue_track_result(&mut self, result: &CueTrackLoadResult) -> Task<Message> {
        match result {
            CueTrackLoadResult::RegionLoaded {
                stems,
                duration_samples,
                overview_peaks,
                highres_peaks,
                path,
            } => {
                // Stale check: ensure this result is for the currently loaded track
                let is_current = self
                    .collection
                    .loaded_track
                    .as_ref()
                    .map_or(false, |t| t.path == *path);
                if !is_current {
                    log::debug!("Discarding stale RegionLoaded for {:?}", path.file_name());
                    return Task::none();
                }

                let state = self.collection.loaded_track.as_mut().unwrap();
                let duration = *duration_samples;

                // Update duration if not yet set
                if state.duration_samples == 0 && duration > 0 {
                    state.duration_samples = duration as u64;

                    // Initialize zoomed waveform with duration
                    state.combined_waveform.zoomed.set_duration(duration as u64);
                    state
                        .combined_waveform
                        .zoomed
                        .update_cue_markers(&state.cue_points);
                    // Apply zoom level from config
                    state
                        .combined_waveform
                        .zoomed
                        .set_zoom(self.domain.config().display.zoom_bars);
                }

                // Update overview waveform peaks (visual growth effect)
                // Rebuild GPU peak buffers so the shader reflects incremental loading
                state.combined_waveform.overview.overview_peak_buffer =
                    mesh_widgets::PeakBuffer::from_stem_peaks(overview_peaks);
                state.combined_waveform.overview.highres_peak_buffer =
                    mesh_widgets::PeakBuffer::from_stem_peaks(highres_peaks);
                state.combined_waveform.overview.stem_waveforms = overview_peaks.clone();
                state.combined_waveform.overview.highres_peaks = highres_peaks.clone();
                state.combined_waveform.overview.duration_samples = duration as u64;
                // First audio data arrived — stop loading pulse
                state.combined_waveform.overview.loading = false;

                // Upgrade engine stems if this message carries a playable buffer snapshot
                if let Some(ref stems) = stems {
                    // First playable stems → set up engine with full track metadata
                    if state.stems.is_none() {
                        log::info!("First playable stems received — setting up audio engine");

                        // Resume audio stream
                        if let Some(ref handle) = self.audio_handle {
                            handle.play();
                        }

                        // Create LoadedTrack for initial engine load
                        let duration_seconds = duration as f64 / SAMPLE_RATE as f64;
                        let loaded_track = LoadedTrack {
                            path: state.path.clone(),
                            stems: stems.clone(),
                            metadata: TrackMetadata {
                                name: None,
                                artist: None,
                                bpm: Some(state.bpm),
                                original_bpm: Some(state.bpm),
                                key: Some(state.key.clone()),
                                duration_seconds: Some(duration_seconds),
                                beat_grid: BeatGrid {
                                    beats: state.beat_grid.clone(),
                                    first_beat_sample: state.beat_grid.first().copied(),
                                },
                                cue_points: state.cue_points.clone(),
                                saved_loops: state.saved_loops.clone(),
                                drop_marker: state.drop_marker,
                                stem_links: state.stem_links.clone(),
                                lufs: None,
                            },
                            duration_samples: duration,
                            duration_seconds,
                        };

                        self.audio.load_track(loaded_track);
                        self.audio.set_global_bpm(state.bpm);
                        self.audio
                            .set_loop_length_index(self.domain.config().display.default_loop_length_index);
                    } else {
                        // Subsequent stems upgrade — just swap the buffer
                        self.audio.upgrade_stems(stems.clone(), duration);
                    }

                    state.stems = Some(stems.clone());
                    state.loading_audio = false;
                }

                Task::none()
            }

            CueTrackLoadResult::Complete {
                stems,
                duration_samples,
                path,
            } => {
                // Stale check
                let is_current = self
                    .collection
                    .loaded_track
                    .as_ref()
                    .map_or(false, |t| t.path == *path);
                if !is_current {
                    log::info!("Discarding stale Complete for {:?}", path.file_name());
                    return Task::none();
                }

                let state = self.collection.loaded_track.as_mut().unwrap();

                // Final stems upgrade
                self.audio.upgrade_stems(stems.clone(), *duration_samples);
                state.stems = Some(stems.clone());
                state.loading_audio = false;
                state.duration_samples = *duration_samples as u64;

                log::info!(
                    "Track loading complete: {:?} ({} samples)",
                    path.file_name().unwrap_or_default(),
                    duration_samples
                );

                Task::none()
            }

            CueTrackLoadResult::Error { error } => {
                log::error!("Progressive track load failed: {}", error);
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.loading_audio = false;
                }
                Task::none()
            }
        }
    }

    /// Handle TrackStemsLoaded message (legacy fallback)
    ///
    /// This is the old single-shot loading path. Kept for safety but no longer
    /// used in the normal flow — CueTrackLoaded handles progressive loading.
    pub fn handle_track_stems_loaded(&mut self, result: StemsLoadResult) -> Task<Message> {
        match result.0 {
            Ok(stems) => {
                log::info!("TrackStemsLoaded (legacy): Audio ready, generating waveform");
                if let Some(ref mut state) = self.collection.loaded_track {
                    let duration_samples = stems.len() as u64;
                    state.duration_samples = duration_samples;
                    state.loading_audio = false;

                    // Generate waveform from loaded stems (overview + highres)
                    let quality_level: u8 = 0;
                    let bpm = state.bpm;
                    let screen_width: u32 = 1920;
                    state
                        .combined_waveform
                        .overview
                        .set_stems(&stems, &state.cue_points, &state.beat_grid, bpm, screen_width, quality_level);

                    // Initialize zoomed waveform with stem data
                    state.combined_waveform.zoomed.set_duration(duration_samples);
                    state
                        .combined_waveform
                        .zoomed
                        .update_cue_markers(&state.cue_points);
                    state
                        .combined_waveform
                        .zoomed
                        .set_zoom(self.domain.config().display.zoom_bars);
                    state.stems = Some(stems.clone());

                    // Create LoadedTrack from metadata + stems for audio engine
                    let duration_seconds =
                        duration_samples as f64 / SAMPLE_RATE as f64;
                    let loaded_track = LoadedTrack {
                        path: state.path.clone(),
                        stems: stems.clone(),
                        metadata: TrackMetadata {
                            name: None,
                            artist: None,
                            bpm: Some(state.bpm),
                            original_bpm: Some(state.bpm),
                            key: Some(state.key.clone()),
                            duration_seconds: Some(duration_seconds),
                            beat_grid: BeatGrid {
                                beats: state.beat_grid.clone(),
                                first_beat_sample: state.beat_grid.first().copied(),
                            },
                            cue_points: state.cue_points.clone(),
                            saved_loops: state.saved_loops.clone(),
                            drop_marker: state.drop_marker,
                            stem_links: state.stem_links.clone(),
                            lufs: None,
                        },
                        duration_samples: duration_samples as usize,
                        duration_seconds,
                    };

                    if let Some(ref handle) = self.audio_handle {
                        handle.play();
                    }

                    self.audio.load_track(loaded_track);
                    self.audio.set_global_bpm(state.bpm);
                    self.audio
                        .set_loop_length_index(self.domain.config().display.default_loop_length_index);
                }
            }
            Err(e) => {
                log::error!("Failed to load track audio: {}", e);
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.loading_audio = false;
                }
            }
        }
        Task::none()
    }

    /// Handle LinkedStemLoaded message
    ///
    /// Called when a linked stem finishes loading from another track.
    /// Updates waveform display and sends data to audio engine.
    pub fn handle_linked_stem_loaded(&mut self, msg: LinkedStemLoadedMsg) -> Task<Message> {
        // Extract the result from Arc wrapper
        let result = match Arc::try_unwrap(msg.0) {
            Ok(r) => r,
            Err(_) => {
                log::warn!("LinkedStemLoadResult Arc still shared, skipping");
                return Task::none();
            }
        };

        match result.result {
            Ok(linked_data) => {
                log::info!(
                    "Linked stem {} loaded: {}",
                    result.stem_idx,
                    linked_data.track_name
                );

                // Store peaks for waveform display
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(peaks) = result.overview_peaks {
                        state
                            .combined_waveform
                            .overview
                            .set_linked_stem_peaks(result.stem_idx, peaks);
                    }
                    if let Some(peaks) = result.highres_peaks {
                        state
                            .combined_waveform
                            .overview
                            .set_linked_highres_peaks(result.stem_idx, peaks);
                    }

                    // Calculate and set LUFS gain for linked stem waveform
                    let linked_gain = self
                        .domain
                        .config()
                        .analysis
                        .loudness
                        .calculate_gain_linear(linked_data.lufs);
                    state
                        .combined_waveform
                        .overview
                        .set_linked_lufs_gain(result.stem_idx, linked_gain);
                    log::info!(
                        "[LINKED] Set LUFS gain for stem {}: linked_lufs={:?}, gain={:.3} ({:+.1}dB)",
                        result.stem_idx,
                        linked_data.lufs,
                        linked_gain,
                        20.0 * linked_gain.log10()
                    );
                }

                // Send LinkStem command to engine with host LUFS to avoid race conditions
                if let Some(stem) = Stem::from_index(result.stem_idx) {
                    let host_lufs = self.collection.loaded_track.as_ref().and_then(|t| t.lufs);
                    self.audio.link_stem(stem, linked_data, host_lufs);
                }
            }
            Err(e) => {
                log::error!("Failed to load linked stem {}: {}", result.stem_idx, e);
            }
        }
        Task::none()
    }
}
