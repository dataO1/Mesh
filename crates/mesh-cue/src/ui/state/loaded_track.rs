//! Loaded track state for the track editor

use basedrop::Shared;
use mesh_core::audio_file::{CuePoint, LoadedTrack, SavedLoop, StemBuffers, StemLinkReference};
use mesh_core::engine::{DeckAtomics, LOOP_LENGTHS};
use mesh_core::types::PlayState;
use mesh_widgets::SliceEditorState;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::ui::waveform::CombinedWaveformView;

/// State for a loaded track being edited
///
/// Note: Manual Debug impl because Deck doesn't implement Debug
pub struct LoadedTrackState {
    /// Path to the track file
    pub path: PathBuf,
    /// Loaded audio data (wrapped in Arc for efficient cloning in messages)
    /// None while audio is loading asynchronously
    pub track: Option<Arc<LoadedTrack>>,
    /// Loaded stems (Shared for RT-safe deallocation)
    pub stems: Option<Shared<StemBuffers>>,
    /// Current cue points (may be modified)
    pub cue_points: Vec<CuePoint>,
    /// Saved loops (up to 8 loop slots)
    pub saved_loops: Vec<SavedLoop>,
    /// Modified BPM (user override)
    pub bpm: f64,
    /// Modified key (user override)
    pub key: String,
    /// Beat grid from metadata
    pub beat_grid: Vec<u64>,
    /// Drop marker sample position (for linked stem alignment)
    pub drop_marker: Option<u64>,
    /// Stem links for prepared mode (stored in mslk chunk)
    pub stem_links: Vec<StemLinkReference>,
    /// Duration in samples (from metadata or computed)
    pub duration_samples: u64,
    /// Whether there are unsaved changes
    pub modified: bool,
    /// Combined waveform display (both zoomed detail and full overview in one canvas)
    /// This works around iced bug #3040 where multiple Canvas widgets don't render properly
    pub combined_waveform: CombinedWaveformView,
    /// Whether audio is currently loading in the background
    pub loading_audio: bool,
    /// Atomics for reading deck state from audio engine (position, play state, loop state)
    /// These are cloned from AudioState when track is loaded
    pub deck_atomics: Arc<DeckAtomics>,
    /// Last time the playhead position was updated (for smooth interpolation)
    pub last_playhead_update: std::time::Instant,
    /// Slice editor state for editing slicer presets
    pub slice_editor: SliceEditorState,
}

impl LoadedTrackState {
    /// Get current playhead position (from deck atomics)
    pub fn playhead_position(&self) -> u64 {
        self.deck_atomics.position()
    }

    /// Get interpolated playhead position for smooth waveform rendering
    ///
    /// When playing, this estimates the current position based on elapsed time
    /// since the last update. This eliminates visible "chunking" in waveform
    /// movement caused by the UI polling rate (16ms) being different from
    /// the audio buffer rate (5.8ms).
    pub fn interpolated_playhead_position(&self) -> u64 {
        let base_position = self.playhead_position();

        // Only interpolate when audio is active (playing or cueing)
        if !self.is_audio_active() {
            return base_position;
        }

        // Calculate samples elapsed since last update
        let elapsed = self.last_playhead_update.elapsed();
        let samples_elapsed = (elapsed.as_secs_f64() * mesh_core::types::SAMPLE_RATE as f64) as u64;

        // Return interpolated position (clamped to duration)
        base_position.saturating_add(samples_elapsed).min(self.duration_samples)
    }

    /// Update the playhead timestamp (call this when position is updated from audio thread)
    pub fn touch_playhead(&mut self) {
        self.last_playhead_update = std::time::Instant::now();
    }

    /// Check if audio is currently playing
    pub fn is_playing(&self) -> bool {
        self.deck_atomics.play_state() == PlayState::Playing
    }

    /// Check if audio is cueing (hot cue/cue preview)
    pub fn is_cueing(&self) -> bool {
        self.deck_atomics.play_state() == PlayState::Cueing
    }

    /// Check if audio is active (playing or cueing) - used for waveform animation
    pub fn is_audio_active(&self) -> bool {
        matches!(self.deck_atomics.play_state(), PlayState::Playing | PlayState::Cueing)
    }

    /// Get play state
    pub fn play_state(&self) -> PlayState {
        self.deck_atomics.play_state()
    }

    /// Check if loop is active
    pub fn is_loop_active(&self) -> bool {
        self.deck_atomics.loop_active.load(Ordering::Relaxed)
    }

    /// Get cue point position
    pub fn cue_point(&self) -> u64 {
        self.deck_atomics.cue_point()
    }

    /// Get beat jump size (same as loop length in beats, minimum 1)
    pub fn beat_jump_size(&self) -> i32 {
        self.loop_length_beats().max(1.0) as i32
    }

    /// Get loop length in beats (from loop_length_index atomic)
    pub fn loop_length_beats(&self) -> f64 {
        let index = self.deck_atomics.loop_length_index() as usize;
        LOOP_LENGTHS.get(index).copied().unwrap_or(4.0)
    }

    /// Get loop bounds (start, end) in samples
    pub fn loop_bounds(&self) -> (u64, u64) {
        (self.deck_atomics.loop_start(), self.deck_atomics.loop_end())
    }

    /// Update zoomed waveform cache if needed for new playhead position
    ///
    /// Call this after any operation that changes the playhead position
    /// (Seek, Stop, BeatJump, JumpToCue, etc.) to ensure the zoomed
    /// waveform displays correctly.
    pub fn update_zoomed_waveform_cache(&mut self, playhead: u64) {
        // mesh-cue doesn't use linked stems, so pass all-false array
        if self.combined_waveform.zoomed.needs_recompute(playhead, &[false; 4]) {
            if let Some(ref stems) = self.stems {
                self.combined_waveform.zoomed.compute_peaks(stems, playhead, 1600);
            }
        }
    }
}

impl std::fmt::Debug for LoadedTrackState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedTrackState")
            .field("path", &self.path)
            .field("cue_points", &self.cue_points)
            .field("bpm", &self.bpm)
            .field("key", &self.key)
            .field("duration_samples", &self.duration_samples)
            .field("modified", &self.modified)
            .field("loading_audio", &self.loading_audio)
            .field("position", &self.deck_atomics.position())
            .finish_non_exhaustive()
    }
}

/// Wrapper for stems load result - provides Debug impl for Shared<StemBuffers>
///
/// basedrop::Shared doesn't implement Debug, so we need this wrapper
/// for the Message enum to derive Debug.
#[derive(Clone)]
pub struct StemsLoadResult(pub Result<Shared<StemBuffers>, String>);

impl std::fmt::Debug for StemsLoadResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Ok(stems) => write!(f, "StemsLoadResult(Ok(<{} frames>))", stems.len()),
            Err(e) => write!(f, "StemsLoadResult(Err({}))", e),
        }
    }
}

/// Wrapper for LinkedStemLoadResult enabling use in Message enum
/// Uses Arc for cheap cloning, manual Debug impl for simplicity
#[derive(Clone)]
pub struct LinkedStemLoadedMsg(pub Arc<mesh_core::loader::LinkedStemLoadResult>);

impl std::fmt::Debug for LinkedStemLoadedMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkedStemLoadedMsg")
            .field("stem_idx", &self.0.stem_idx)
            .finish_non_exhaustive()
    }
}
