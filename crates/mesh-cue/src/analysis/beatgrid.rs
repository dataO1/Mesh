//! Beat grid generation
//!
//! Converts detected beat positions into a fixed-interval beat grid
//! suitable for DJ software synchronization.
//!
//! ## Phase Anchor Algorithm
//!
//! Instead of naively using the first detected beat as the grid anchor
//! (which is unreliable for tracks with ambient intros), we:
//!
//! 1. **Energy gate**: Filter out beats in low-energy regions (intros, breakdowns)
//! 2. **Circular median**: Compute the consensus phase offset across all
//!    remaining beats using circular statistics (atan2 of mean sin/cos)
//!
//! This gives a statistically robust phase anchor that represents where
//! beats actually land across the entire track, not just the first detected event.

use mesh_core::types::SAMPLE_RATE;

/// Essentia always operates at 44100 Hz — beat ticks are relative to this rate
const ESSENTIA_SAMPLE_RATE: f64 = 44100.0;

/// RMS window half-width around each tick for energy measurement (±25ms)
const ENERGY_WINDOW_SAMPLES: usize = 1103; // 0.025 * 44100

/// Ticks with RMS below this fraction of the track's peak tick RMS are discarded.
/// This filters out ghost beats in silent intros/outros.
const ENERGY_THRESHOLD_RATIO: f64 = 0.1;

/// Minimum number of ticks required to use the circular median algorithm.
/// Below this, fall back to the first tick.
const MIN_TICKS_FOR_MEDIAN: usize = 4;

/// Generate a fixed-interval beat grid from detected beat positions
///
/// Uses a circular median phase anchor computed from energy-filtered beat ticks.
/// This is preferred over using the first detected beat because:
/// 1. It uses statistical consensus across ALL detected beats
/// 2. It filters out phantom beats in silent intros/breakdowns
/// 3. It produces more accurate phase alignment for DJ synchronization
///
/// # Arguments
/// * `bpm` - Detected BPM value
/// * `beat_ticks` - Raw beat positions in seconds from Essentia
/// * `samples` - Audio samples at Essentia's rate (44100 Hz) for energy gating
/// * `duration_samples` - Total track duration in samples (at source rate)
///
/// # Returns
/// Vector of beat positions as sample indices at the system sample rate (48kHz)
pub fn generate_beat_grid(
    bpm: f64,
    beat_ticks: &[f64],
    samples: &[f32],
    duration_samples: u64,
) -> Vec<u64> {
    log::info!(
        "generate_beat_grid: bpm={:.1}, beat_ticks={}, duration_samples={}",
        bpm,
        beat_ticks.len(),
        duration_samples
    );

    if beat_ticks.is_empty() || duration_samples == 0 {
        log::warn!("generate_beat_grid: empty input, returning empty grid");
        return Vec::new();
    }

    // Calculate samples per beat at the system rate (48kHz)
    let samples_per_beat_f64 = SAMPLE_RATE as f64 * 60.0 / bpm;
    let samples_per_beat = samples_per_beat_f64 as u64;

    // Compute the phase anchor using energy-filtered circular median
    let first_beat_sample = if beat_ticks.len() >= MIN_TICKS_FOR_MEDIAN && !samples.is_empty() {
        compute_phase_anchor(bpm, beat_ticks, samples)
    } else {
        // Fallback: use first tick directly (old behavior)
        let first_beat = beat_ticks[0];
        (first_beat * SAMPLE_RATE as f64) as u64
    };

    // Generate uniform grid from the phase anchor
    let num_beats = if first_beat_sample < duration_samples {
        ((duration_samples - first_beat_sample) / samples_per_beat) as usize
    } else {
        0
    };

    log::info!(
        "generate_beat_grid: first_beat_sample={}, samples_per_beat={}, num_beats={}",
        first_beat_sample,
        samples_per_beat,
        num_beats
    );

    (0..=num_beats)
        .map(|i| first_beat_sample + (i as u64 * samples_per_beat))
        .collect()
}

/// Compute the phase anchor using energy-filtered circular median
///
/// 1. Compute RMS energy around each tick to filter out silent-region ghosts
/// 2. Compute each tick's phase offset modulo one beat period
/// 3. Use circular statistics (atan2) to find the consensus phase
/// 4. Find the earliest grid-aligned beat position using that phase
fn compute_phase_anchor(bpm: f64, beat_ticks: &[f64], samples: &[f32]) -> u64 {
    let samples_per_beat_output = SAMPLE_RATE as f64 * 60.0 / bpm;
    let num_samples = samples.len();

    // Step 1: Compute RMS energy around each tick
    let tick_energies: Vec<(f64, f64)> = beat_ticks
        .iter()
        .map(|&tick_secs| {
            let center = (tick_secs * ESSENTIA_SAMPLE_RATE) as usize;
            let start = center.saturating_sub(ENERGY_WINDOW_SAMPLES);
            let end = (center + ENERGY_WINDOW_SAMPLES).min(num_samples);

            if start >= end {
                return (tick_secs, 0.0);
            }

            let sum_sq: f64 = samples[start..end]
                .iter()
                .map(|&s| (s as f64) * (s as f64))
                .sum();
            let rms = (sum_sq / (end - start) as f64).sqrt();
            (tick_secs, rms)
        })
        .collect();

    // Step 2: Filter by energy threshold
    let peak_rms = tick_energies
        .iter()
        .map(|&(_, rms)| rms)
        .fold(0.0_f64, f64::max);

    let threshold = peak_rms * ENERGY_THRESHOLD_RATIO;

    let active_ticks: Vec<f64> = tick_energies
        .iter()
        .filter(|&&(_, rms)| rms > threshold)
        .map(|&(tick, _)| tick)
        .collect();

    let ticks_to_use = if active_ticks.len() >= MIN_TICKS_FOR_MEDIAN {
        log::info!(
            "compute_phase_anchor: {} of {} ticks pass energy gate (threshold RMS={:.4})",
            active_ticks.len(),
            beat_ticks.len(),
            threshold
        );
        &active_ticks
    } else {
        // Not enough active ticks — use all ticks
        log::info!(
            "compute_phase_anchor: only {} ticks pass energy gate, using all {} ticks",
            active_ticks.len(),
            beat_ticks.len()
        );
        // Can't use active_ticks as a reference to a local, so we need a different approach
        // We'll handle this by collecting beat_ticks into a vec
        beat_ticks
    };

    // Step 3: Circular median of phase offsets
    // Each tick's phase = (tick_seconds mod beat_period) / beat_period * 2π
    let beat_period_secs = 60.0 / bpm;

    let (sin_sum, cos_sum) = ticks_to_use
        .iter()
        .map(|&tick_secs| {
            let phase_frac = (tick_secs % beat_period_secs) / beat_period_secs;
            let angle = phase_frac * std::f64::consts::TAU;
            (angle.sin(), angle.cos())
        })
        .fold((0.0, 0.0), |(s, c), (si, ci)| (s + si, c + ci));

    let mean_angle = sin_sum.atan2(cos_sum);
    // Normalize to [0, 2π)
    let mean_angle = if mean_angle < 0.0 {
        mean_angle + std::f64::consts::TAU
    } else {
        mean_angle
    };
    let phase_frac = mean_angle / std::f64::consts::TAU;
    let phase_samples = phase_frac * samples_per_beat_output;

    // Step 4: Find the earliest grid-aligned beat position
    // The phase tells us the offset within one beat period. We need to find
    // the first beat position in the track that has this phase.
    // Start from the phase offset itself (the first beat at or near the start)
    let first_beat_sample = phase_samples as u64;

    log::info!(
        "compute_phase_anchor: circular mean phase={:.1}° ({:.1}ms), first_beat_sample={}",
        phase_frac * 360.0,
        phase_samples / SAMPLE_RATE as f64 * 1000.0,
        first_beat_sample
    );

    first_beat_sample
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
    fn test_beat_grid_generation_basic() {
        // 120 BPM = 0.5 seconds per beat
        let bpm = 120.0;
        let beat_ticks = vec![0.0, 0.5, 1.0, 1.5, 2.0];
        // 10 seconds of audio at 48000 Hz (SAMPLE_RATE)
        let duration_samples = SAMPLE_RATE as u64 * 10;

        // Create synthetic audio with clicks at beat positions (at 44100 Hz)
        let audio_len = (10.0 * ESSENTIA_SAMPLE_RATE) as usize;
        let mut samples = vec![0.0f32; audio_len];
        for &tick in &beat_ticks {
            let idx = (tick * ESSENTIA_SAMPLE_RATE) as usize;
            if idx < audio_len {
                // Short click impulse
                for i in 0..100.min(audio_len - idx) {
                    samples[idx + i] = 0.8;
                }
            }
        }

        let grid = generate_beat_grid(bpm, &beat_ticks, &samples, duration_samples);

        let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;

        assert!(!grid.is_empty());
        // Grid should be evenly spaced
        for i in 1..grid.len() {
            assert_eq!(grid[i] - grid[i - 1], samples_per_beat);
        }
        // Should have beats for full 10 seconds (20 beats at 120 BPM)
        assert_eq!(grid.len(), 21); // 0..=20 = 21 beats
    }

    #[test]
    fn test_beat_grid_empty_input() {
        let grid = generate_beat_grid(120.0, &[], &[], 0);
        assert!(grid.is_empty());
    }

    #[test]
    fn test_beat_grid_fallback_few_ticks() {
        // With fewer than MIN_TICKS_FOR_MEDIAN ticks, should use first tick
        let bpm = 174.0;
        let beat_ticks = vec![1.0, 1.345]; // Only 2 ticks
        let duration_samples = SAMPLE_RATE as u64 * 10;
        let samples = vec![0.5f32; (10.0 * ESSENTIA_SAMPLE_RATE) as usize];

        let grid = generate_beat_grid(bpm, &beat_ticks, &samples, duration_samples);

        // First beat should be at tick[0] converted to 48kHz
        let expected_first = (1.0 * SAMPLE_RATE as f64) as u64;
        assert_eq!(grid[0], expected_first);
    }

    #[test]
    fn test_energy_gating_skips_silent_intro() {
        let bpm = 120.0;
        let beat_period = 60.0 / bpm; // 0.5 seconds

        // Simulate: 2 phantom beats in silence at 0.0 and 0.5s,
        // then real beats starting at 5.0s (offset by 0.1s from the phantom phase)
        let mut beat_ticks = vec![0.0, 0.5]; // phantom beats in silence
        // Real beats starting at 5.1s (phase offset 0.1s into beat)
        for i in 0..20 {
            beat_ticks.push(5.1 + i as f64 * beat_period);
        }

        let audio_len = (16.0 * ESSENTIA_SAMPLE_RATE) as usize;
        let mut samples = vec![0.0f32; audio_len];

        // Only put energy at the real beat positions (starting at 5.1s)
        for i in 0..20 {
            let tick = 5.1 + i as f64 * beat_period;
            let idx = (tick * ESSENTIA_SAMPLE_RATE) as usize;
            if idx < audio_len {
                for j in 0..200.min(audio_len - idx) {
                    samples[idx + j] = 0.9;
                }
            }
        }

        let duration_samples = SAMPLE_RATE as u64 * 16;
        let grid = generate_beat_grid(bpm, &beat_ticks, &samples, duration_samples);

        // The first beat should NOT be at 0 (phantom), it should be near the
        // phase of the real beats (0.1s offset)
        let first_beat_secs = grid[0] as f64 / SAMPLE_RATE as f64;

        // Phase should be ~0.1s (the real beats' offset modulo beat period)
        // Allow some tolerance for circular median computation
        let phase_in_beat = first_beat_secs % beat_period;
        assert!(
            (phase_in_beat - 0.1).abs() < 0.02,
            "Phase should be ~0.1s, got {:.3}s (first_beat at {:.3}s)",
            phase_in_beat,
            first_beat_secs
        );
    }

    #[test]
    fn test_grid_offset() {
        // Samples per beat at 48kHz, 120 BPM
        let spb = (SAMPLE_RATE as f64 * 60.0 / 120.0) as u64;
        let grid = vec![0, spb, spb * 2];
        let offset = adjust_grid_start(&grid, 1000);

        assert_eq!(offset[0], 1000);
        assert_eq!(offset[1], spb + 1000);
        assert_eq!(offset[2], spb * 2 + 1000);
    }

    #[test]
    fn test_circular_median_wraparound() {
        // Test that circular statistics correctly handle phase near 0/period boundary
        let bpm = 120.0;
        let beat_period = 0.5; // seconds

        // Beats near the end and start of the period (should average to ~0)
        // Phase at 0.49s = 98% into period, phase at 0.01s = 2% into period
        // Circular mean should be near 0% (i.e., on the beat boundary)
        let beat_ticks: Vec<f64> = (0..20)
            .map(|i| {
                let base = i as f64 * beat_period;
                // Alternate between slightly before and slightly after the beat
                if i % 2 == 0 {
                    base + 0.01
                } else {
                    base - 0.01
                }
            })
            .filter(|&t| t >= 0.0)
            .collect();

        let audio_len = (12.0 * ESSENTIA_SAMPLE_RATE) as usize;
        let mut samples = vec![0.0f32; audio_len];
        for &tick in &beat_ticks {
            let idx = (tick * ESSENTIA_SAMPLE_RATE) as usize;
            if idx < audio_len {
                for j in 0..100.min(audio_len - idx) {
                    samples[idx + j] = 0.8;
                }
            }
        }

        let duration_samples = SAMPLE_RATE as u64 * 12;
        let grid = generate_beat_grid(bpm, &beat_ticks, &samples, duration_samples);

        // Phase should be near 0 (within ~10ms)
        let first_beat_secs = grid[0] as f64 / SAMPLE_RATE as f64;
        let phase_in_beat = first_beat_secs % beat_period;
        // Phase near 0 could be very small or very close to beat_period
        let phase_distance = phase_in_beat.min(beat_period - phase_in_beat);
        assert!(
            phase_distance < 0.015,
            "Phase should be near 0, got {:.3}s (distance {:.3}s)",
            phase_in_beat,
            phase_distance
        );
    }
}
