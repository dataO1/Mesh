//! Loudness measurement using Essentia's EBU R128 algorithm
//!
//! Provides integrated LUFS measurement for automatic gain staging.
//! The measured LUFS value is stored in track metadata, and gain
//! compensation is calculated at runtime based on the configured target.

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

/// Measure integrated LUFS loudness of audio samples
///
/// Uses EBU R128 algorithm which is the broadcast standard for loudness
/// normalization. Returns integrated loudness over the entire track.
///
/// # Arguments
/// * `samples` - Mono audio samples at 48kHz (converted internally to stereo)
/// * `sample_rate` - Sample rate of the audio (typically 48000.0)
///
/// # Returns
/// Integrated LUFS value (typically -24 to 0 for music)
pub fn measure_lufs(samples: &[f32], sample_rate: f32) -> Result<f32> {
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

    // Extract integrated LUFS value
    let integrated_lufs: f32 = result
        .integrated_loudness()
        .context("Failed to get integrated loudness output")?
        .get();

    log::info!("LUFS measurement complete: {:.2} LUFS", integrated_lufs);

    Ok(integrated_lufs)
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
/// let measured = -10.0; // Track is at -10 LUFS
/// let target = -6.0;    // Target is -6 LUFS (loud DJ standard)
/// let gain_db = calculate_gain_compensation(measured, target);
/// // gain_db = 4.0 dB boost needed
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

        // Very loud track - clamp cut
        assert!(
            (calculate_gain_compensation_clamped(10.0, -6.0, -24.0, 12.0) - (-24.0)).abs() < 0.001
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
