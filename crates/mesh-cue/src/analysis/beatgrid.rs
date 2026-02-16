//! Beat grid generation
//!
//! Converts detected beat positions into a fixed-interval beat grid
//! suitable for DJ software synchronization.
//!
//! ## Phase Anchor Algorithm
//!
//! The phase anchor determines where the grid starts (the "downbeat offset").
//! We use a two-stage approach:
//!
//! ### Stage 1: Energy-gated circular median (from detected beats)
//! 1. **Energy gate**: Filter out beats in low-energy regions (intros, breakdowns)
//! 2. **Circular median**: Compute consensus phase offset using atan2 of mean sin/cos
//!
//! ### Stage 2: Onset-weighted phase search (if ODF available)
//! 1. For each candidate phase offset within one beat period at ODF resolution
//! 2. Place a hypothetical grid across the entire track
//! 3. Sum onset detection function values at each grid position
//! 4. The phase with the highest total onset energy wins
//!
//! Stage 2 refines stage 1 by searching within ±half a frame of the circular
//! median result, using direct rhythmic salience measurement.

use crate::analysis::bpm::OnsetFunctionResult;
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
/// Uses a two-stage phase anchor algorithm:
/// 1. Energy-gated circular median for initial phase estimate
/// 2. Onset-weighted phase search for sub-frame refinement (if ODF provided)
///
/// # Arguments
/// * `bpm` - Detected BPM value
/// * `beat_ticks` - Raw beat positions in seconds from Essentia
/// * `samples` - Audio samples at Essentia's rate (44100 Hz) for energy gating
/// * `duration_samples` - Total track duration in samples (at source rate)
/// * `onset_function` - Optional onset detection function for phase refinement
///
/// # Returns
/// Vector of beat positions as sample indices at the system sample rate (48kHz)
pub fn generate_beat_grid(
    bpm: f64,
    beat_ticks: &[f64],
    samples: &[f32],
    duration_samples: u64,
    onset_function: Option<&OnsetFunctionResult>,
) -> Vec<u64> {
    log::info!(
        "generate_beat_grid: bpm={:.1}, beat_ticks={}, duration_samples={}, has_odf={}",
        bpm,
        beat_ticks.len(),
        duration_samples,
        onset_function.is_some()
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
        compute_phase_anchor(bpm, beat_ticks, samples, onset_function)
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

/// Compute the phase anchor using energy-filtered circular median + onset refinement
///
/// Two-stage algorithm:
/// 1. Energy-gated circular median of beat tick phases (robust initial estimate)
/// 2. Onset-weighted phase search around that estimate (precise refinement)
///
/// Stage 2 searches candidate phases within one beat period at ODF frame resolution.
/// For each candidate, it places a hypothetical grid across the full track and sums
/// the ODF values at grid positions. The candidate with the highest cumulative onset
/// energy becomes the final phase anchor.
fn compute_phase_anchor(
    bpm: f64,
    beat_ticks: &[f64],
    samples: &[f32],
    onset_function: Option<&OnsetFunctionResult>,
) -> u64 {
    let samples_per_beat_output = SAMPLE_RATE as f64 * 60.0 / bpm;
    let num_samples = samples.len();

    // ── Stage 1: Energy-gated circular median ──────────────────────────

    // Step 1a: Compute RMS energy around each tick
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

    // Step 1b: Filter by energy threshold
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
        log::info!(
            "compute_phase_anchor: only {} ticks pass energy gate, using all {} ticks",
            active_ticks.len(),
            beat_ticks.len()
        );
        beat_ticks
    };

    // Step 1c: Circular median of phase offsets
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
    let mean_angle = if mean_angle < 0.0 {
        mean_angle + std::f64::consts::TAU
    } else {
        mean_angle
    };
    let circular_phase_frac = mean_angle / std::f64::consts::TAU;
    let circular_phase_samples = circular_phase_frac * samples_per_beat_output;

    log::info!(
        "compute_phase_anchor: Stage 1 circular mean phase={:.1}° ({:.1}ms)",
        circular_phase_frac * 360.0,
        circular_phase_samples / SAMPLE_RATE as f64 * 1000.0,
    );

    // ── Stage 2: Onset-weighted phase search ───────────────────────────

    let first_beat_sample = match onset_function {
        Some(odf) if !odf.values.is_empty() => {
            onset_weighted_phase_search(
                bpm,
                &odf.values,
                odf.frame_rate,
                circular_phase_frac,
            )
        }
        _ => {
            // No ODF available — use circular median result directly
            circular_phase_samples as u64
        }
    };

    log::info!(
        "compute_phase_anchor: final first_beat_sample={}",
        first_beat_sample
    );

    first_beat_sample
}

/// Refine the phase offset using onset detection function values
///
/// Searches within ±25% of one beat period around the circular median
/// estimate from Stage 1. For each candidate offset, places a grid across
/// the track and sums interpolated ODF values at grid positions.
/// The candidate with the highest cumulative onset energy is chosen.
///
/// This is a REFINEMENT — it trusts the circular median as approximately
/// correct and only fine-tunes within a narrow window.
fn onset_weighted_phase_search(
    bpm: f64,
    odf_values: &[f32],
    odf_frame_rate: f64,
    circular_phase_frac: f64,
) -> u64 {
    let beat_period_secs = 60.0 / bpm;
    let frames_per_beat = beat_period_secs * odf_frame_rate;
    let total_frames = odf_values.len();

    if frames_per_beat <= 0.0 || total_frames == 0 {
        return 0;
    }

    // Center of search = circular median phase (in ODF frames)
    let center_frame = circular_phase_frac * frames_per_beat;

    // Search window: ±25% of one beat period (wrapping around)
    let search_radius = (frames_per_beat * 0.25).ceil() as i64;
    let num_frames_per_beat = frames_per_beat.ceil() as i64;

    let mut best_phase_frame: f64 = center_frame;
    let mut best_score: f64 = f64::NEG_INFINITY;
    let mut candidates_searched = 0;

    for offset in -search_radius..=search_radius {
        // Wrap the candidate phase into [0, frames_per_beat)
        let raw = (center_frame.round() as i64) + offset;
        let wrapped = ((raw % num_frames_per_beat) + num_frames_per_beat) % num_frames_per_beat;
        let phase_frame = wrapped as f64;

        // Sum ODF values at all grid positions for this phase
        let mut score: f64 = 0.0;
        let mut grid_pos = phase_frame;

        while grid_pos < total_frames as f64 {
            let frame_lo = grid_pos.floor() as usize;
            let frame_hi = frame_lo + 1;
            let frac = grid_pos - frame_lo as f64;

            let val = if frame_hi < total_frames {
                let lo = odf_values[frame_lo] as f64;
                let hi = odf_values[frame_hi] as f64;
                lo + frac * (hi - lo)
            } else if frame_lo < total_frames {
                odf_values[frame_lo] as f64
            } else {
                0.0
            };

            score += val;
            grid_pos += frames_per_beat;
        }

        if score > best_score {
            best_score = score;
            best_phase_frame = phase_frame;
        }
        candidates_searched += 1;
    }

    // Convert best phase from ODF frames to output samples (48kHz)
    let best_phase_secs = best_phase_frame / odf_frame_rate;
    let best_phase_samples = best_phase_secs * SAMPLE_RATE as f64;

    log::info!(
        "onset_weighted_phase_search: center={:.1} frames, best={:.1} frames ({:.1}ms), \
         score={:.2}, searched {} candidates over {} ODF frames",
        center_frame,
        best_phase_frame,
        best_phase_secs * 1000.0,
        best_score,
        candidates_searched,
        total_frames
    );

    best_phase_samples as u64
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

        let grid = generate_beat_grid(bpm, &beat_ticks, &samples, duration_samples, None);

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
        let grid = generate_beat_grid(120.0, &[], &[], 0, None);
        assert!(grid.is_empty());
    }

    #[test]
    fn test_beat_grid_fallback_few_ticks() {
        // With fewer than MIN_TICKS_FOR_MEDIAN ticks, should use first tick
        let bpm = 174.0;
        let beat_ticks = vec![1.0, 1.345]; // Only 2 ticks
        let duration_samples = SAMPLE_RATE as u64 * 10;
        let samples = vec![0.5f32; (10.0 * ESSENTIA_SAMPLE_RATE) as usize];

        let grid = generate_beat_grid(bpm, &beat_ticks, &samples, duration_samples, None);

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
        let grid = generate_beat_grid(bpm, &beat_ticks, &samples, duration_samples, None);

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
        let grid = generate_beat_grid(bpm, &beat_ticks, &samples, duration_samples, None);

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

    #[test]
    fn test_onset_weighted_phase_refinement() {
        // Test that the ODF-based phase search refines the circular median
        // within the ±25% search window.
        let bpm = 120.0;
        let beat_period = 0.5; // seconds
        let odf_frame_rate = 44100.0 / 512.0; // ~86.13 fps
        let frames_per_beat = beat_period * odf_frame_rate; // ~43.07 frames

        // Create a synthetic ODF with strong peaks at a phase offset of 0.05s
        // (10% of beat period — well within the ±25% search window around 0.0)
        let target_phase_secs = 0.05;
        let target_phase_frame = target_phase_secs * odf_frame_rate; // ~4.3 frames

        let total_frames = (10.0 * odf_frame_rate) as usize; // 10 seconds
        let mut odf_values = vec![0.0f32; total_frames];

        // Place strong impulses at the target phase positions
        let mut pos = target_phase_frame;
        while (pos as usize) < total_frames {
            let idx = pos as usize;
            if idx < total_frames {
                odf_values[idx] = 1.0;
                if idx > 0 {
                    odf_values[idx - 1] = 0.3;
                }
                if idx + 1 < total_frames {
                    odf_values[idx + 1] = 0.3;
                }
            }
            pos += frames_per_beat;
        }

        let odf = OnsetFunctionResult {
            values: odf_values,
            frame_rate: odf_frame_rate,
        };

        // Create beat ticks at phase 0.0s — circular median will be ~0.0
        // The ODF should refine this to ~0.05s
        let beat_ticks: Vec<f64> = (0..20)
            .map(|i| i as f64 * beat_period)
            .collect();

        // Audio with energy everywhere (so energy gating doesn't filter anything)
        let audio_len = (10.0 * ESSENTIA_SAMPLE_RATE) as usize;
        let samples = vec![0.5f32; audio_len];
        let duration_samples = SAMPLE_RATE as u64 * 10;

        let grid = generate_beat_grid(
            bpm,
            &beat_ticks,
            &samples,
            duration_samples,
            Some(&odf),
        );

        assert!(!grid.is_empty());

        // The grid phase should be near 0.05s (from ODF refinement)
        let first_beat_secs = grid[0] as f64 / SAMPLE_RATE as f64;
        let phase_in_beat = first_beat_secs % beat_period;
        assert!(
            (phase_in_beat - target_phase_secs).abs() < 0.015,
            "ODF should refine circular median: expected phase ~{:.3}s, got {:.3}s",
            target_phase_secs,
            phase_in_beat
        );
    }

    #[test]
    fn test_onset_search_none_odf_matches_circular_median() {
        // When no ODF is provided, result should match pure circular median
        let bpm = 174.0;
        let beat_period = 60.0 / bpm;

        // Create beats at a specific phase offset (0.05s)
        let beat_ticks: Vec<f64> = (0..40)
            .map(|i| 0.05 + i as f64 * beat_period)
            .collect();

        let audio_len = (16.0 * ESSENTIA_SAMPLE_RATE) as usize;
        let samples = vec![0.5f32; audio_len]; // uniform energy
        let duration_samples = SAMPLE_RATE as u64 * 16;

        let grid_no_odf = generate_beat_grid(
            bpm,
            &beat_ticks,
            &samples,
            duration_samples,
            None,
        );

        assert!(!grid_no_odf.is_empty());

        // Phase should be ~0.05s (from the ticks)
        let first_beat_secs = grid_no_odf[0] as f64 / SAMPLE_RATE as f64;
        let phase_in_beat = first_beat_secs % beat_period;
        assert!(
            (phase_in_beat - 0.05).abs() < 0.01,
            "Without ODF, should use circular median: expected ~0.050s, got {:.3}s",
            phase_in_beat
        );
    }
}
