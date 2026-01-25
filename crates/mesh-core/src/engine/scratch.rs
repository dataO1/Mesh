//! Vinyl scratch emulation
//!
//! Provides velocity-based audio playback that emulates vinyl scratching.
//! When the "vinyl" is stationary, output is silence. When moving, audio
//! plays at a speed proportional to the velocity, with proper interpolation
//! for smooth variable-speed playback.
//!
//! ## Interpolation Methods
//!
//! - **Linear**: Fast, acceptable quality. Interpolates between 2 adjacent samples.
//! - **Cubic**: Better quality, slightly more CPU. Uses Catmull-Rom spline (4 samples).
//!
//! ## References
//!
//! - EP1415297A2: "The smoothed position signal is differentiated and provides the playback speed"
//! - CCRMA Stanford: Digital Audio Resampling (https://ccrma.stanford.edu/~jos/resample/)

use crate::types::{StereoBuffer, StereoSample};
use serde::{Deserialize, Serialize};

/// Minimum velocity threshold to produce audio (samples per frame)
/// Below this, output is silence. ~50 samples at 44.1kHz ≈ 1ms
const VELOCITY_THRESHOLD: i64 = 50;

/// Interpolation method for variable-speed playback
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterpolationMethod {
    /// Linear interpolation (2-point) - fast, acceptable quality
    #[default]
    Linear,
    /// Cubic Catmull-Rom interpolation (4-point) - better quality, more CPU
    Cubic,
}

impl InterpolationMethod {
    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Linear => "Linear (Fast)",
            Self::Cubic => "Cubic (Quality)",
        }
    }

    /// Get all variants for UI dropdown
    pub fn all() -> &'static [Self] {
        &[Self::Linear, Self::Cubic]
    }
}

/// Scratch state for a single deck
#[derive(Debug, Clone)]
pub struct ScratchState {
    /// Whether scratch mode is active
    pub active: bool,
    /// Position when scratch started (to restore on end)
    pub start_position: usize,
    /// Last known position for velocity calculation
    pub last_position: usize,
    /// Fractional position accumulator for sub-sample accuracy
    pub fractional_position: f64,
    /// Current interpolation method
    pub interpolation: InterpolationMethod,
}

impl Default for ScratchState {
    fn default() -> Self {
        Self {
            active: false,
            start_position: 0,
            last_position: 0,
            fractional_position: 0.0,
            interpolation: InterpolationMethod::default(),
        }
    }
}

impl ScratchState {
    /// Create new scratch state
    pub fn new() -> Self {
        Self::default()
    }

    /// Enter scratch mode at current position
    pub fn start(&mut self, position: usize) {
        self.active = true;
        self.start_position = position;
        self.last_position = position;
        self.fractional_position = position as f64;
    }

    /// Update scratch position (called on mouse move)
    ///
    /// Note: The actual position update is done via the Deck's position field.
    /// This method exists for potential future use (e.g., smoothing).
    pub fn move_to(&mut self, _position: usize) {
        // Position is managed by Deck - this is a placeholder for future smoothing
    }

    /// Exit scratch mode
    pub fn end(&mut self) {
        self.active = false;
    }

    /// Set interpolation method
    pub fn set_interpolation(&mut self, method: InterpolationMethod) {
        self.interpolation = method;
    }

    /// Calculate velocity and determine if we should output audio
    ///
    /// Returns (should_output, samples_to_generate, is_reverse)
    pub fn calculate_velocity(&mut self, current_position: usize, output_len: usize) -> (bool, usize, bool) {
        let velocity = current_position as i64 - self.last_position as i64;
        self.last_position = current_position;

        if velocity.abs() < VELOCITY_THRESHOLD {
            // Stationary: output silence
            return (false, 0, false);
        }

        // Calculate how many samples to generate based on velocity
        // Clamp to reasonable range to prevent buffer issues
        let samples = (velocity.abs() as usize).clamp(1, output_len * 2);
        let reverse = velocity < 0;

        (true, samples, reverse)
    }
}

/// Linear interpolation between two samples
///
/// Simple and fast - interpolates linearly between sample[0] and sample[1]
/// based on fractional position t (0.0 to 1.0)
#[inline]
fn lerp_sample(s0: StereoSample, s1: StereoSample, t: f32) -> StereoSample {
    StereoSample {
        left: s0.left + (s1.left - s0.left) * t,
        right: s0.right + (s1.right - s0.right) * t,
    }
}

/// Cubic Catmull-Rom interpolation (4-point)
///
/// Higher quality than linear - uses 4 samples to create a smooth curve.
/// The Catmull-Rom spline passes through all control points and has
/// continuous first derivatives.
///
/// Samples: s0 = [i-1], s1 = [i], s2 = [i+1], s3 = [i+2]
/// t = fractional position between s1 and s2 (0.0 to 1.0)
#[inline]
fn cubic_interpolate(s0: StereoSample, s1: StereoSample, s2: StereoSample, s3: StereoSample, t: f32) -> StereoSample {
    // Catmull-Rom coefficients
    let t2 = t * t;
    let t3 = t2 * t;

    // Catmull-Rom basis functions
    let c0 = -0.5 * t3 + t2 - 0.5 * t;
    let c1 = 1.5 * t3 - 2.5 * t2 + 1.0;
    let c2 = -1.5 * t3 + 2.0 * t2 + 0.5 * t;
    let c3 = 0.5 * t3 - 0.5 * t2;

    StereoSample {
        left: s0.left * c0 + s1.left * c1 + s2.left * c2 + s3.left * c3,
        right: s0.right * c0 + s1.right * c1 + s2.right * c2 + s3.right * c3,
    }
}

/// Get a sample from audio data with bounds checking
#[inline]
fn get_sample(data: &[StereoSample], index: i64, len: usize) -> StereoSample {
    if index < 0 || index >= len as i64 {
        StereoSample::silence()
    } else {
        data[index as usize]
    }
}

/// Read audio with interpolation at a fractional position
///
/// This is the core function for smooth variable-speed playback.
/// It reads audio at non-integer sample positions using interpolation.
pub fn read_interpolated(
    data: &[StereoSample],
    position: f64,
    method: InterpolationMethod,
) -> StereoSample {
    let len = data.len();
    if len == 0 {
        return StereoSample::silence();
    }

    let index = position.floor() as i64;
    let frac = (position - position.floor()) as f32;

    match method {
        InterpolationMethod::Linear => {
            let s0 = get_sample(data, index, len);
            let s1 = get_sample(data, index + 1, len);
            lerp_sample(s0, s1, frac)
        }
        InterpolationMethod::Cubic => {
            let s0 = get_sample(data, index - 1, len);
            let s1 = get_sample(data, index, len);
            let s2 = get_sample(data, index + 1, len);
            let s3 = get_sample(data, index + 2, len);
            cubic_interpolate(s0, s1, s2, s3, frac)
        }
    }
}

/// Process scratch audio for a single stem
///
/// Generates `output_len` samples by reading from `source` at variable speed
/// based on the velocity (position delta). Uses interpolation for smooth
/// variable-speed playback.
///
/// # Arguments
/// * `output` - Buffer to fill with scratch audio
/// * `source` - Source audio data (stem)
/// * `start_pos` - Starting position in source (fractional)
/// * `velocity` - Samples per output sample (can be negative for reverse)
/// * `method` - Interpolation method to use
pub fn process_scratch_stem(
    output: &mut StereoBuffer,
    source: &[StereoSample],
    start_pos: f64,
    velocity: f64,
    method: InterpolationMethod,
) {
    let output_slice = output.as_mut_slice();
    let source_len = source.len();

    if source_len == 0 {
        output.fill_silence();
        return;
    }

    let mut pos = start_pos;

    for sample in output_slice.iter_mut() {
        // Handle reverse playback (negative velocity)
        let read_pos = if velocity < 0.0 {
            // When going backward, we read forward from a position that decreases
            pos
        } else {
            pos
        };

        *sample = read_interpolated(source, read_pos.max(0.0), method);

        // Advance position by velocity
        pos += velocity;

        // Clamp to valid range
        if pos < 0.0 {
            pos = 0.0;
        } else if pos >= source_len as f64 {
            pos = (source_len - 1) as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_interpolation() {
        let s0 = StereoSample { left: 0.0, right: 0.0 };
        let s1 = StereoSample { left: 1.0, right: 1.0 };

        // Midpoint should be 0.5
        let mid = lerp_sample(s0, s1, 0.5);
        assert!((mid.left - 0.5).abs() < 0.001);
        assert!((mid.right - 0.5).abs() < 0.001);

        // t=0 should be s0
        let at_0 = lerp_sample(s0, s1, 0.0);
        assert!((at_0.left - 0.0).abs() < 0.001);

        // t=1 should be s1
        let at_1 = lerp_sample(s0, s1, 1.0);
        assert!((at_1.left - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_velocity_threshold() {
        let mut state = ScratchState::new();
        state.start(1000);

        // Small movement should be silence
        let (should_output, _, _) = state.calculate_velocity(1010, 256);
        assert!(!should_output);

        // Large movement should output
        state.last_position = 1000;
        let (should_output, samples, reverse) = state.calculate_velocity(1200, 256);
        assert!(should_output);
        assert_eq!(samples, 200);
        assert!(!reverse);

        // Backward movement should be reverse
        state.last_position = 1200;
        let (should_output, _, reverse) = state.calculate_velocity(1000, 256);
        assert!(should_output);
        assert!(reverse);
    }
}
