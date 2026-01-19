//! Track loading message handlers
//!
//! Handles the two-phase track loading process:
//! 1. TrackMetadataLoaded - Fast metadata from database, shows UI immediately
//! 2. TrackStemsLoaded - Slow audio loading (~3s), enables playback
//!
//! Also handles:
//! - LinkedStemLoaded - When a linked stem finishes loading
//! - TrackLoaded - Legacy single-phase loading (deprecated)

use basedrop::Shared;
use iced::Task;
use mesh_core::audio_file::{BeatGrid, LoadedTrack, TrackMetadata};
use mesh_core::types::Stem;
use mesh_widgets::{SliceEditorState, HIGHRES_WIDTH};
use std::sync::Arc;

use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::state::{LinkedStemLoadedMsg, LoadedTrackState, StemsLoadResult};
use super::super::waveform::{
    generate_peaks, CombinedWaveformView, WaveformView, ZoomedWaveformView,
};

impl MeshCueApp {
    /// Handle TrackMetadataLoaded message
    ///
    /// Phase 1 of track loading: metadata loaded from database.
    /// Shows UI immediately while audio loads in background.
    pub fn handle_track_metadata_loaded(
        &mut self,
        result: Result<(std::path::PathBuf, TrackMetadata), String>,
    ) -> Task<Message> {
        match result {
            Ok((path, metadata)) => {
                log::info!("TrackMetadataLoaded: Showing UI, starting audio load");
                let bpm = metadata.bpm.unwrap_or(120.0);
                let key = metadata.key.clone().unwrap_or_else(|| "?".to_string());
                let cue_points = metadata.cue_points.clone();
                let beat_grid = metadata.beat_grid.beats.clone();

                // Create combined waveform view (both zoomed + overview in single canvas)
                let mut combined_waveform = CombinedWaveformView::new();
                // Initialize overview with beat markers from metadata
                combined_waveform.overview = WaveformView::from_metadata(&metadata);
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
                    duration_samples: 0, // Will be set when audio loads
                    modified: false,
                    combined_waveform,
                    loading_audio: true,
                    deck_atomics: self.audio.deck_atomics().clone(),
                    last_playhead_update: std::time::Instant::now(),
                    slice_editor: {
                        // Load presets from dedicated file (shared with mesh-player)
                        let slicer_config =
                            mesh_widgets::load_slicer_presets(&self.collection.collection_path);
                        let mut editor = SliceEditorState::new();
                        slicer_config.apply_to_editor_state(&mut editor);
                        editor
                    },
                });

                // Phase 2: Load audio stems in background (slow, ~3s)
                Task::perform(
                    async move {
                        LoadedTrack::load_stems(&path)
                            .map(|stems| Shared::new(&mesh_core::engine::gc::gc_handle(), stems))
                            .map_err(|e| e.to_string())
                    },
                    |result| Message::TrackStemsLoaded(StemsLoadResult(result)),
                )
            }
            Err(e) => {
                log::error!("Failed to load track metadata: {}", e);
                Task::none()
            }
        }
    }

    /// Handle TrackStemsLoaded message
    ///
    /// Phase 2 of track loading: audio stems loaded from disk.
    /// Enables playback and generates waveform visualization.
    pub fn handle_track_stems_loaded(&mut self, result: StemsLoadResult) -> Task<Message> {
        match result.0 {
            Ok(stems) => {
                log::info!("TrackStemsLoaded: Audio ready, generating waveform");
                if let Some(ref mut state) = self.collection.loaded_track {
                    let duration_samples = stems.len() as u64;
                    state.duration_samples = duration_samples;
                    state.loading_audio = false;

                    // Generate waveform from loaded stems (overview)
                    state
                        .combined_waveform
                        .overview
                        .set_stems(&stems, &state.cue_points, &state.beat_grid);

                    // Compute high-resolution peaks for stable zoomed waveform rendering
                    let highres_start = std::time::Instant::now();
                    let highres_peaks = generate_peaks(&stems, HIGHRES_WIDTH);
                    log::info!(
                        "[PERF] mesh-cue highres peaks: {} samples → {} peaks in {:?}",
                        duration_samples,
                        HIGHRES_WIDTH,
                        highres_start.elapsed()
                    );
                    state
                        .combined_waveform
                        .overview
                        .set_highres_peaks(highres_peaks);

                    // Initialize zoomed waveform with stem data
                    state.combined_waveform.zoomed.set_duration(duration_samples);
                    state
                        .combined_waveform
                        .zoomed
                        .update_cue_markers(&state.cue_points);
                    // Apply zoom level from config
                    state
                        .combined_waveform
                        .zoomed
                        .set_zoom(self.domain.config().display.zoom_bars);
                    state
                        .combined_waveform
                        .zoomed
                        .compute_peaks(&stems, 0, 1600);

                    state.stems = Some(stems.clone());

                    // Create LoadedTrack from metadata + stems for audio engine
                    let duration_seconds =
                        duration_samples as f64 / mesh_core::types::SAMPLE_RATE as f64;
                    let loaded_track = LoadedTrack {
                        path: state.path.clone(),
                        stems: stems.clone(),
                        metadata: TrackMetadata {
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
                            waveform_preview: None, // Using live-generated waveform
                            drop_marker: state.drop_marker,
                            stem_links: state.stem_links.clone(),
                            lufs: None, // LUFS read from track, passed to Deck separately
                        },
                        duration_samples: duration_samples as usize,
                        duration_seconds,
                    };

                    // Load track into audio engine (creates PreparedTrack internally)
                    self.audio.load_track(loaded_track);
                    // Set global BPM to track's BPM for original-speed playback (no time-stretching)
                    self.audio.set_global_bpm(state.bpm);
                    // Set default loop length from config
                    self.audio
                        .set_loop_length_index(self.domain.config().display.default_loop_length_index);
                    // Linked stems are auto-loaded by engine from track metadata
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
                    // This matches what mesh-player does to ensure visual consistency
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

    /// Handle TrackLoaded message (legacy)
    ///
    /// Single-phase track loading (deprecated). Kept for compatibility.
    /// New code should use the two-phase approach via LoadTrackByPath.
    pub fn handle_track_loaded(
        &mut self,
        result: Result<Arc<LoadedTrack>, String>,
    ) -> Task<Message> {
        match result {
            Ok(track) => {
                let path = track.path.clone();
                let bpm = track.bpm();
                let key = track.key().to_string();
                let cue_points = track.metadata.cue_points.clone();
                let beat_grid = track.metadata.beat_grid.beats.clone();
                let duration_samples = track.duration_samples as u64;
                let stems = track.stems.clone();

                // Create combined waveform with full track data
                let mut combined_waveform = CombinedWaveformView::new();
                combined_waveform.overview = WaveformView::from_track(&track, &cue_points);
                // Apply grid density from config
                combined_waveform
                    .overview
                    .set_grid_bars(self.domain.config().display.grid_bars);

                // Compute high-resolution peaks for stable zoomed waveform rendering
                let highres_start = std::time::Instant::now();
                let highres_peaks = generate_peaks(&stems, HIGHRES_WIDTH);
                log::info!(
                    "[PERF] mesh-cue highres peaks: {} samples → {} peaks in {:?}",
                    duration_samples,
                    HIGHRES_WIDTH,
                    highres_start.elapsed()
                );
                combined_waveform.overview.set_highres_peaks(highres_peaks);

                combined_waveform.zoomed =
                    ZoomedWaveformView::from_metadata(bpm, beat_grid.clone(), Vec::new());
                combined_waveform.zoomed.set_duration(duration_samples);
                combined_waveform
                    .zoomed
                    .set_drop_marker(track.metadata.drop_marker);
                combined_waveform.zoomed.compute_peaks(&stems, 0, 1600);

                // Load track into audio engine (creates PreparedTrack internally)
                let track_for_audio = LoadedTrack {
                    path: track.path.clone(),
                    stems: track.stems.clone(),
                    metadata: track.metadata.clone(),
                    duration_samples: track.duration_samples,
                    duration_seconds: track.duration_seconds,
                };
                self.audio.load_track(track_for_audio);
                // Set global BPM to track's BPM for original-speed playback (no time-stretching)
                self.audio.set_global_bpm(bpm);
                self.audio
                    .set_loop_length_index(self.domain.config().display.default_loop_length_index);

                self.collection.loaded_track = Some(LoadedTrackState {
                    path,
                    track: Some(track.clone()),
                    stems: Some(stems),
                    cue_points,
                    saved_loops: track.metadata.saved_loops.clone(),
                    bpm,
                    key,
                    beat_grid,
                    drop_marker: track.metadata.drop_marker,
                    lufs: track.metadata.lufs,
                    stem_links: track.metadata.stem_links.clone(),
                    duration_samples,
                    modified: false,
                    combined_waveform,
                    loading_audio: false,
                    deck_atomics: self.audio.deck_atomics().clone(),
                    last_playhead_update: std::time::Instant::now(),
                    slice_editor: {
                        // Load presets from dedicated file (shared with mesh-player)
                        let slicer_config =
                            mesh_widgets::load_slicer_presets(&self.collection.collection_path);
                        let mut editor = SliceEditorState::new();
                        slicer_config.apply_to_editor_state(&mut editor);
                        editor
                    },
                });
            }
            Err(e) => {
                log::error!("Failed to load track: {}", e);
            }
        }
        Task::none()
    }
}
