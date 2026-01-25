//! Beat grid manipulation utilities
//!
//! Functions for nudging, regenerating, and snapping to beat grids.

use crate::ui::state::LoadedTrackState;

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

    // Calculate samples per bar (4 beats)
    let samples_per_beat = (SAMPLE_RATE_F64 * 60.0 / state.bpm) as i64;
    let samples_per_bar = samples_per_beat * 4;

    // Get current first beat
    let first_beat = state.beat_grid[0] as i64;

    // Apply delta
    let mut new_first_beat = first_beat + delta_samples;

    // Wrap around one bar if out of bounds
    if new_first_beat < 0 {
        new_first_beat += samples_per_bar;
    } else if new_first_beat >= samples_per_bar {
        new_first_beat -= samples_per_bar;
    }

    // Regenerate beat grid from new first beat
    let new_first_beat = new_first_beat as u64;
    state.beat_grid = regenerate_beat_grid(new_first_beat, state.bpm, state.duration_samples);

    // Update waveform displays
    update_waveform_beat_grid(state);

    // Note: Caller is responsible for propagating beat grid to deck via
    // audio.set_beat_grid() so snapping operations use the updated grid.

    // Mark as modified for save
    state.modified = true;
}

/// Regenerate beat grid from a first beat position, BPM, and track duration
pub fn regenerate_beat_grid(first_beat: u64, bpm: f64, duration_samples: u64) -> Vec<u64> {
    if bpm <= 0.0 || duration_samples == 0 {
        return Vec::new();
    }

    let samples_per_beat = (SAMPLE_RATE_F64 * 60.0 / bpm) as u64;
    let mut beats = Vec::new();
    let mut pos = first_beat;

    while pos < duration_samples {
        beats.push(pos);
        pos += samples_per_beat;
    }

    beats
}

/// Update waveform beat grid markers after grid modification
pub fn update_waveform_beat_grid(state: &mut LoadedTrackState) {
    // Update zoomed view (uses sample positions directly)
    state.combined_waveform.zoomed.set_beat_grid(state.beat_grid.clone());

    // Update overview (uses normalized positions 0.0-1.0)
    if state.duration_samples > 0 {
        state.combined_waveform.overview.beat_markers = state.beat_grid
            .iter()
            .map(|&pos| pos as f64 / state.duration_samples as f64)
            .collect();
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
