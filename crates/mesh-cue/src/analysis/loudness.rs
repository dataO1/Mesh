//! Loudness measurement using Essentia's EBU R128 algorithm
//!
//! Provides "drop loudness" LUFS measurement for automatic gain staging.
//! Instead of integrated loudness (which averages the entire track including
//! quiet intros), we measure the top 10% of 3-second short-term loudness
//! windows. This captures the loudness of the drop/peak sections, ensuring
//! tracks are level-matched where it matters most for DJ performance.

use anyhow::{Context, Result};
use essentia::algorithm::loudness_dynamics::loudness_ebur_128::LoudnessEbur128;
use essentia::data::GetFromDataContainer;
use essentia::essentia::Essentia;

/// StereoSample-compatible struct for passing to Essentia
///
/// This has the same memory layout as `essentia_sys::ffi::StereoSample`
/// which has private fields. Both are simple structs with two f32 fields.
#[repr(C)]
#[derive(Clone, Copy)]
struct StereoSample {
    left: f32,
    right: f32,
}

/// Result of LUFS measurement containing both drop and integrated loudness.
#[derive(Debug, Clone, Copy)]
pub struct LufsResult {
    /// "Drop loudness": energy-averaged top 10% of 3-second short-term windows.
    /// Captures the loudness of the loudest sections (the drop), used for gain staging.
    pub drop_lufs: f32,
    /// Traditional EBU R128 integrated loudness over the entire track.
    /// Stored for future use but not used for gain compensation.
    pub integrated_lufs: f32,
}

/// Measure LUFS loudness of audio samples (both drop and integrated).
///
/// Returns a `LufsResult` with two values:
/// - `drop_lufs`: top 10% of 3-second short-term windows (energy-averaged).
///   This measures the loudness of the drop/peak sections for DJ gain staging.
/// - `integrated_lufs`: traditional EBU R128 whole-track average, stored for
///   future use.
///
/// If short-term data is unavailable or the track is very short, `drop_lufs`
/// falls back to `integrated_lufs`.
pub fn measure_lufs(samples: &[f32], sample_rate: f32) -> Result<LufsResult> {
    log::info!(
        "Starting LUFS measurement on {} samples ({:.1}s)",
        samples.len(),
        samples.len() as f64 / sample_rate as f64
    );

    // Convert mono to stereo (Essentia's LoudnessEBUR128 expects stereo input)
    // Each mono sample becomes a stereo pair with identical left/right values
    let stereo_samples: Vec<StereoSample> = samples
        .iter()
        .map(|&s| StereoSample { left: s, right: s })
        .collect();

    // Create Essentia instance
    let essentia = Essentia::new();

    // Create and configure LoudnessEBUR128 algorithm
    let mut loudness = essentia
        .create::<LoudnessEbur128>()
        .sample_rate(sample_rate)
        .context("Failed to set sample rate")?
        .configure()
        .context("Failed to configure LoudnessEBUR128")?;

    // SAFETY: Our StereoSample has the same layout as essentia_sys::ffi::StereoSample
    // Both are simple structs with two f32 fields (left, right) in the same order.
    // The essentia crate expects &[essentia_sys::ffi::StereoSample].
    let ffi_samples: &[essentia_sys::ffi::StereoSample] = unsafe {
        std::slice::from_raw_parts(
            stereo_samples.as_ptr() as *const essentia_sys::ffi::StereoSample,
            stereo_samples.len(),
        )
    };

    // Run the algorithm with stereo samples
    let result = loudness
        .compute(ffi_samples)
        .context("LoudnessEBUR128 computation failed")?;

    // Extract integrated LUFS (whole-track average)
    let integrated_lufs: f32 = result
        .integrated_loudness()
        .context("Failed to get integrated loudness output")?
        .get();

    // Extract short-term loudness (3-second windows) for drop measurement
    let drop_lufs = match result.short_term_loudness() {
        Ok(st_container) => {
            let st_values: Vec<f32> = st_container.get();
            compute_drop_loudness(&st_values, integrated_lufs)
        }
        Err(e) => {
            log::warn!("Short-term loudness unavailable ({}), using integrated", e);
            integrated_lufs
        }
    };

    log::info!(
        "LUFS measurement complete: drop={:.2} LUFS (integrated={:.2} LUFS, delta={:+.1} dB)",
        drop_lufs,
        integrated_lufs,
        drop_lufs - integrated_lufs,
    );

    Ok(LufsResult { drop_lufs, integrated_lufs })
}

/// Compute drop loudness from short-term LUFS windows.
///
/// Takes the top 10% of 3-second windows (minimum 3 windows = 9 seconds),
/// energy-averages them in the linear domain, and converts back to LUFS.
/// This captures the loudness of the drop/peak sections of a track.
fn compute_drop_loudness(st_values: &[f32], integrated_fallback: f32) -> f32 {
    // Filter out -inf/-200 silence windows (Essentia uses -200 for silence)
    let mut valid: Vec<f32> = st_values
        .iter()
        .copied()
        .filter(|&v| v > -70.0)
        .collect();

    if valid.len() < 3 {
        log::debug!(
            "Too few valid short-term windows ({}), using integrated LUFS",
            valid.len()
        );
        return integrated_fallback;
    }

    // Sort descending (loudest first)
    valid.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    // Take top 10%, minimum 3 windows (9 seconds)
    let top_count = (valid.len() / 10).max(3).min(valid.len());
    let top_windows = &valid[..top_count];

    // Energy-average in linear domain: LUFS → power → mean → LUFS
    // LUFS is defined as 10*log10(power) relative to reference, so:
    //   power = 10^(LUFS/10)
    //   mean_LUFS = 10 * log10(mean(powers))
    let power_sum: f64 = top_windows
        .iter()
        .map(|&lufs| 10.0_f64.powf(lufs as f64 / 10.0))
        .sum();
    let mean_power = power_sum / top_count as f64;
    let drop_lufs = (10.0 * mean_power.log10()) as f32;

    log::debug!(
        "Drop loudness: top {} of {} windows = {:.2} LUFS",
        top_count,
        valid.len(),
        drop_lufs,
    );

    drop_lufs
}

/// Calculate gain compensation in dB to reach target loudness
///
/// # Arguments
/// * `measured_lufs` - The track's measured integrated LUFS
/// * `target_lufs` - The desired target LUFS level
///
/// # Returns
/// Gain adjustment in dB (positive = boost, negative = cut)
///
/// # Example
/// ```
/// use mesh_cue::analysis::calculate_gain_compensation;
/// let measured = -10.0; // Track is at -10 LUFS
/// let target = -6.0;    // Target is -6 LUFS (loud DJ standard)
/// let gain_db = calculate_gain_compensation(measured, target);
/// assert!((gain_db - 4.0).abs() < 0.001); // 4.0 dB boost needed
/// ```
#[inline]
pub fn calculate_gain_compensation(measured_lufs: f32, target_lufs: f32) -> f32 {
    target_lufs - measured_lufs
}

/// Calculate clamped gain compensation with safety limits
///
/// # Arguments
/// * `measured_lufs` - The track's measured integrated LUFS
/// * `target_lufs` - The desired target LUFS level
/// * `min_gain_db` - Minimum gain (most negative cut, e.g., -24.0)
/// * `max_gain_db` - Maximum gain (most positive boost, e.g., 12.0)
///
/// # Returns
/// Gain adjustment in dB, clamped to safety limits
pub fn calculate_gain_compensation_clamped(
    measured_lufs: f32,
    target_lufs: f32,
    min_gain_db: f32,
    max_gain_db: f32,
) -> f32 {
    let gain_db = calculate_gain_compensation(measured_lufs, target_lufs);

    if gain_db > max_gain_db {
        log::warn!(
            "Track very quiet ({:.1} LUFS): clamping boost from {:.1} to {:.1} dB",
            measured_lufs,
            gain_db,
            max_gain_db
        );
    } else if gain_db < min_gain_db {
        log::warn!(
            "Track very loud ({:.1} LUFS): clamping cut from {:.1} to {:.1} dB",
            measured_lufs,
            gain_db,
            min_gain_db
        );
    }

    gain_db.clamp(min_gain_db, max_gain_db)
}

/// Convert decibels to linear gain factor
///
/// # Arguments
/// * `db` - Gain in decibels
///
/// # Returns
/// Linear gain multiplier (1.0 = unity, 2.0 = +6dB, 0.5 = -6dB)
#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Convert linear gain factor to decibels
///
/// # Arguments
/// * `linear` - Linear gain multiplier
///
/// # Returns
/// Gain in decibels
#[inline]
pub fn linear_to_db(linear: f32) -> f32 {
    20.0 * linear.log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gain_compensation() {
        // Track at -10 LUFS, target -6 LUFS = +4 dB boost
        assert!((calculate_gain_compensation(-10.0, -6.0) - 4.0).abs() < 0.001);

        // Track at -4 LUFS, target -6 LUFS = -2 dB cut
        assert!((calculate_gain_compensation(-4.0, -6.0) - (-2.0)).abs() < 0.001);

        // Track at target = no change
        assert!((calculate_gain_compensation(-6.0, -6.0) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_gain_compensation_clamped() {
        // Normal case - within limits
        assert!(
            (calculate_gain_compensation_clamped(-10.0, -6.0, -24.0, 12.0) - 4.0).abs() < 0.001
        );

        // Very quiet track - clamp boost
        assert!(
            (calculate_gain_compensation_clamped(-30.0, -6.0, -24.0, 12.0) - 12.0).abs() < 0.001
        );

        // Very loud track - clamp cut (track at +20 LUFS needs -26 dB, clamped to -24)
        assert!(
            (calculate_gain_compensation_clamped(20.0, -6.0, -24.0, 12.0) - (-24.0)).abs() < 0.001
        );
    }

    #[test]
    fn test_db_to_linear() {
        // Unity gain
        assert!((db_to_linear(0.0) - 1.0).abs() < 0.001);

        // +6 dB ≈ 2x
        assert!((db_to_linear(6.0) - 1.995).abs() < 0.01);

        // -6 dB ≈ 0.5x
        assert!((db_to_linear(-6.0) - 0.501).abs() < 0.01);

        // +20 dB = 10x
        assert!((db_to_linear(20.0) - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_linear_to_db() {
        // Unity gain
        assert!((linear_to_db(1.0) - 0.0).abs() < 0.001);

        // 2x ≈ +6 dB
        assert!((linear_to_db(2.0) - 6.02).abs() < 0.1);

        // 10x = +20 dB
        assert!((linear_to_db(10.0) - 20.0).abs() < 0.001);
    }

    #[test]
    fn test_db_linear_roundtrip() {
        for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
            let linear = db_to_linear(db);
            let back = linear_to_db(linear);
            assert!((db - back).abs() < 0.001, "Roundtrip failed for {} dB", db);
        }
    }
}
