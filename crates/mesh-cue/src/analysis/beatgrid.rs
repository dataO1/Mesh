//! Beat grid generation
//!
//! Converts detected beat positions into a fixed-interval beat grid
//! suitable for DJ software synchronization.

use mesh_core::types::SAMPLE_RATE;

/// Generate a fixed-interval beat grid from detected beat positions
///
/// This creates a uniform beat grid starting from the first detected beat,
/// using the detected BPM to space beats evenly. This is preferred over
/// using the raw detected beats because:
/// 1. It ensures perfect tempo sync in the DJ player
/// 2. It handles tracks with slight tempo variations
/// 3. It produces consistent beat grid markers for the UI
///
/// # Arguments
/// * `bpm` - Detected BPM value
/// * `beat_ticks` - Raw beat positions in seconds from detection
/// * `duration_samples` - Total track duration in samples
///
/// # Returns
/// Vector of beat positions as sample indices at 44.1kHz
pub fn generate_beat_grid(bpm: f64, beat_ticks: &[f64], duration_samples: u64) -> Vec<u64> {
    if beat_ticks.is_empty() || duration_samples == 0 {
        return Vec::new();
    }

    // Find the first beat position
    let first_beat = beat_ticks[0];

    // Calculate samples per beat
    let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;

    // Generate fixed grid using actual track duration
    let first_beat_sample = (first_beat * SAMPLE_RATE as f64) as u64;
    let num_beats = ((duration_samples - first_beat_sample) / samples_per_beat) as usize;

    (0..=num_beats)
        .map(|i| first_beat_sample + (i as u64 * samples_per_beat))
        .collect()
}

/// Adjust beat grid start position (nudge first beat)
///
/// Used when the user manually adjusts the downbeat position
pub fn adjust_grid_start(grid: &[u64], offset_samples: i64) -> Vec<u64> {
    grid.iter()
        .map(|&pos| {
            if offset_samples >= 0 {
                pos.saturating_add(offset_samples as u64)
            } else {
                pos.saturating_sub((-offset_samples) as u64)
            }
        })
        .collect()
}

/// Regenerate beat grid with new BPM (user override)
pub fn regenerate_grid(first_beat_sample: u64, bpm: f64, duration_samples: u64) -> Vec<u64> {
    let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;
    let num_beats = ((duration_samples - first_beat_sample) / samples_per_beat) as usize;

    (0..=num_beats)
        .map(|i| first_beat_sample + (i as u64 * samples_per_beat))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_beat_grid_generation() {
        // 120 BPM = 0.5 seconds per beat = 22050 samples per beat at 44100Hz
        let bpm = 120.0;
        let beat_ticks = vec![0.0, 0.5, 1.0, 1.5, 2.0];
        // 10 seconds of audio at 44100 Hz
        let duration_samples = 44100 * 10;

        let grid = generate_beat_grid(bpm, &beat_ticks, duration_samples);

        assert!(!grid.is_empty());
        assert_eq!(grid[0], 0); // First beat at 0
        assert_eq!(grid[1], 22050); // Second beat at 0.5s
        assert_eq!(grid[2], 44100); // Third beat at 1.0s
        // Should have beats for full 10 seconds (20 beats at 120 BPM)
        assert_eq!(grid.len(), 21); // 0..=20 = 21 beats
    }

    #[test]
    fn test_grid_offset() {
        let grid = vec![0, 22050, 44100];
        let offset = adjust_grid_start(&grid, 1000);

        assert_eq!(offset[0], 1000);
        assert_eq!(offset[1], 23050);
        assert_eq!(offset[2], 45100);
    }
}
