//! Beat grid manipulation utilities
//!
//! Functions for nudging, regenerating, and snapping to beat grids.

use crate::ui::state::LoadedTrackState;
use mesh_core::audio_file::BeatGrid;

/// Nudge amount in samples (~2.5ms at 48kHz for fine-grained control)
pub const BEAT_GRID_NUDGE_SAMPLES: i64 = 120;

/// Sample rate constant (matches mesh_core::types::SAMPLE_RATE)
const SAMPLE_RATE_F64: f64 = mesh_core::types::SAMPLE_RATE as f64;

/// Nudge the beat grid by a delta amount of samples
///
/// The grid is shifted by moving the first beat position, then regenerating
/// all subsequent beats. If the first beat would go negative or beyond one bar,
/// it wraps around to stay within a single bar range.
pub fn nudge_beat_grid(state: &mut LoadedTrackState, delta_samples: i64) {
    if state.beat_grid.is_empty() || state.bpm <= 0.0 {
        return;
    }

    // Nudge operates on the user-anchored downbeat (state.first_beat_sample),
    // NOT state.beat_grid[0] — after backfill, beat_grid[0] is a beat near
    // sample 0, not the user's anchor.
    let new_anchor_i64 = (state.first_beat_sample as i64).saturating_add(delta_samples);
    if new_anchor_i64 < 0 {
        return;
    }
    let new_anchor = new_anchor_i64 as u64;

    // Regenerate beat grid from new anchor
    let (beats, anchor_idx) = regenerate_beat_grid(new_anchor, state.bpm, state.duration_samples);
    state.beat_grid = beats;

    // Update waveform displays
    update_waveform_beat_grid(state, anchor_idx);

    // Note: Caller is responsible for propagating beat grid to deck via
    // audio.set_beat_grid() so snapping operations use the updated grid.

    // Mark as modified for save
    state.modified = true;
}

/// Regenerate beat grid from an anchor downbeat, BPM, and track duration.
///
/// Backfills beats before the anchor so `beats[0]` lies in `[0, samples_per_beat)`.
/// This lets beat-jump and snap reach pre-anchor positions. Returns
/// `(beats, anchor_idx)` where `anchor_idx` is the position of the user's
/// chosen downbeat within the returned Vec.
///
/// Uses f64 accumulation to prevent truncation drift (±0.5 samples max error
/// regardless of beat count).
pub fn regenerate_beat_grid(anchor_sample: u64, bpm: f64, duration_samples: u64) -> (Vec<u64>, usize) {
    if bpm <= 0.0 || duration_samples == 0 {
        return (Vec::new(), 0);
    }
    let spb = SAMPLE_RATE_F64 * 60.0 / bpm;
    if spb <= 0.0 {
        return (Vec::new(), 0);
    }
    // Walk backward from the anchor by whole beats until in [0, spb).
    let mut start = anchor_sample as f64;
    let mut anchor_idx: usize = 0;
    while start > spb {
        start -= spb;
        anchor_idx += 1;
    }
    let effective_first_beat = start.round() as u64;
    let beats = BeatGrid::regenerate(effective_first_beat, bpm, duration_samples).beats;
    (beats, anchor_idx)
}

/// Update waveform beat grid markers after grid modification.
///
/// `anchor_idx` is the index in `state.beat_grid` of the user's anchored
/// downbeat. The shader uses this index to align red and phrase markers.
pub fn update_waveform_beat_grid(state: &mut LoadedTrackState, anchor_idx: usize) {
    // Update zoomed view (uses sample positions directly)
    state.combined_waveform.zoomed.set_beat_grid(state.beat_grid.clone());

    // Persist the user-anchored downbeat on LoadedTrackState so it round-trips
    // through save (BeatGrid.first_beat_sample) and reload.
    if let Some(&anchor_pos) = state.beat_grid.get(anchor_idx) {
        state.first_beat_sample = anchor_pos;
    }

    // Update overview (uses normalized positions 0.0-1.0)
    if state.duration_samples > 0 {
        state.combined_waveform.overview.beat_markers = state.beat_grid
            .iter()
            .map(|&pos| pos as f64 / state.duration_samples as f64)
            .collect();
        let max_idx = state.combined_waveform.overview.beat_markers.len().saturating_sub(1);
        state.combined_waveform.overview.beat_anchor_idx = anchor_idx.min(max_idx);
    }
}

/// Snap a position to the nearest beat in the beat grid
pub fn snap_to_nearest_beat(position: u64, beat_grid: &[u64]) -> u64 {
    if beat_grid.is_empty() {
        return position;
    }
    beat_grid
        .iter()
        .min_by_key(|&&b| (b as i64 - position as i64).unsigned_abs())
        .copied()
        .unwrap_or(position)
}

/// Find the nearest beat to a position, returning both its index and position
///
/// Returns (index, position) of the beat closest to the given sample position.
/// If the grid is empty, returns (0, 0).
pub fn find_nearest_beat_with_index(beat_grid: &[u64], position: u64) -> (usize, u64) {
    if beat_grid.is_empty() {
        return (0, 0);
    }

    // Binary search to find insertion point
    match beat_grid.binary_search(&position) {
        Ok(idx) => (idx, beat_grid[idx]),
        Err(idx) => {
            if idx == 0 {
                (0, beat_grid[0])
            } else if idx >= beat_grid.len() {
                let last_idx = beat_grid.len() - 1;
                (last_idx, beat_grid[last_idx])
            } else {
                // Between two beats - pick the closer one
                let before = beat_grid[idx - 1];
                let after = beat_grid[idx];
                if position - before <= after - position {
                    (idx - 1, before)
                } else {
                    (idx, after)
                }
            }
        }
    }
}
