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
//! - **Cubic**: Better quality, uses Catmull-Rom spline (4 samples).
//! - **Sinc**: Highest quality, band-limited interpolation (8 taps).
//!
//! ## Key Design: Continuous Read Position
//!
//! To avoid clicks at buffer boundaries, we maintain a **continuous read position**
//! that advances smoothly based on velocity. The UI target position is used only
//! to calculate velocity, not as a direct read position.
//!
//! ## References
//!
//! - EP1415297A2: "The smoothed position signal is differentiated and provides the playback speed"
//! - CCRMA Stanford: Digital Audio Resampling (https://ccrma.stanford.edu/~jos/resample/)

use crate::types::{StereoBuffer, StereoSample};
use serde::{Deserialize, Serialize};

/// Minimum velocity threshold to produce audio (as ratio)
/// Below this, output is silence. 0.01 ≈ 1% of normal speed
const VELOCITY_THRESHOLD: f64 = 0.01;

/// Velocity smoothing factor (exponential moving average)
/// Higher = more responsive, Lower = smoother
const VELOCITY_SMOOTHING: f64 = 0.4;

/// Position catch-up rate when read position drifts too far from target
/// This gently pulls the read position towards the target without causing clicks
const POSITION_CORRECTION_RATE: f64 = 0.001;

/// Maximum drift allowed between read position and target before correction kicks in
const MAX_POSITION_DRIFT: f64 = 44100.0; // ~1 second at 44.1kHz

/// Interpolation method for variable-speed playback
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterpolationMethod {
    /// Linear interpolation (2-point) - fast, acceptable quality
    Linear,
    /// Cubic Catmull-Rom interpolation (4-point) - better quality
    #[default]
    Cubic,
    /// Sinc interpolation (8-tap) - highest quality, most CPU
    Sinc,
}

impl InterpolationMethod {
    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Linear => "Linear (Fast)",
            Self::Cubic => "Cubic (Good)",
            Self::Sinc => "Sinc (Best)",
        }
    }

    /// Get all variants for UI dropdown
    pub fn all() -> &'static [Self] {
        &[Self::Linear, Self::Cubic, Self::Sinc]
    }
}

/// Scratch state for a single deck
#[derive(Debug, Clone)]
pub struct ScratchState {
    /// Whether scratch mode is active
    pub active: bool,
    /// Target position from UI (where the user is dragging to)
    target_position: f64,
    /// Previous target position (for velocity calculation)
    prev_target_position: f64,
    /// Continuous read position (advances smoothly to avoid clicks)
    read_position: f64,
    /// Current smoothed velocity (samples per output sample, 1.0 = normal speed)
    smoothed_velocity: f64,
    /// Current interpolation method
    pub interpolation: InterpolationMethod,
}

impl Default for ScratchState {
    fn default() -> Self {
        Self {
            active: false,
            target_position: 0.0,
            prev_target_position: 0.0,
            read_position: 0.0,
            smoothed_velocity: 0.0,
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
        self.target_position = position as f64;
        self.prev_target_position = position as f64;
        self.read_position = position as f64;
        self.smoothed_velocity = 0.0;
    }

    /// Update target scratch position (called on mouse move)
    pub fn move_to(&mut self, position: usize) {
        self.target_position = position as f64;
    }

    /// Exit scratch mode
    pub fn end(&mut self) {
        self.active = false;
        self.smoothed_velocity = 0.0;
    }

    /// Set interpolation method
    pub fn set_interpolation(&mut self, method: InterpolationMethod) {
        self.interpolation = method;
    }

    /// Update scratch state and calculate playback parameters
    ///
    /// This should be called once per audio frame (in process()).
    /// It calculates velocity from target position changes and advances
    /// the continuous read position.
    ///
    /// Returns (should_output, velocity_ratio, read_position)
    /// - should_output: false if velocity is below threshold (output silence)
    /// - velocity_ratio: playback speed (1.0 = normal, 2.0 = 2x, -1.0 = reverse normal)
    /// - read_position: starting position for this frame (continuous, no clicks)
    pub fn update(&mut self, output_len: usize) -> (bool, f64, f64) {
        // Calculate raw velocity from target position change
        let target_delta = self.target_position - self.prev_target_position;
        let raw_velocity = target_delta / output_len as f64;
        self.prev_target_position = self.target_position;

        // Smooth velocity to reduce jitter
        self.smoothed_velocity += (raw_velocity - self.smoothed_velocity) * VELOCITY_SMOOTHING;

        // Check if velocity is above threshold
        if self.smoothed_velocity.abs() < VELOCITY_THRESHOLD {
            // Below threshold: output silence, but keep read position ready
            return (false, 0.0, self.read_position);
        }

        // Store current read position for this frame
        let frame_start_position = self.read_position;

        // Advance read position by velocity * buffer_size
        // This is the key: read position advances continuously, not jumping to target
        self.read_position += self.smoothed_velocity * output_len as f64;

        // Gentle position correction if we've drifted too far from target
        // This prevents the read position from getting permanently out of sync
        let drift = self.target_position - self.read_position;
        if drift.abs() > MAX_POSITION_DRIFT {
            // Apply gentle correction towards target
            self.read_position += drift * POSITION_CORRECTION_RATE;
        }

        (true, self.smoothed_velocity, frame_start_position)
    }

    /// Get current read position (for deck position sync)
    pub fn current_position(&self) -> usize {
        self.read_position.max(0.0) as usize
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Interpolation Functions
// ─────────────────────────────────────────────────────────────────────────────

/// Linear interpolation between two samples
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
#[inline]
fn cubic_interpolate(s0: StereoSample, s1: StereoSample, s2: StereoSample, s3: StereoSample, t: f32) -> StereoSample {
    let t2 = t * t;
    let t3 = t2 * t;

    // Catmull-Rom basis functions (tension = 0.5)
    let c0 = -0.5 * t3 + t2 - 0.5 * t;
    let c1 = 1.5 * t3 - 2.5 * t2 + 1.0;
    let c2 = -1.5 * t3 + 2.0 * t2 + 0.5 * t;
    let c3 = 0.5 * t3 - 0.5 * t2;

    StereoSample {
        left: s0.left * c0 + s1.left * c1 + s2.left * c2 + s3.left * c3,
        right: s0.right * c0 + s1.right * c1 + s2.right * c2 + s3.right * c3,
    }
}

/// Sinc function with Blackman-Harris window
///
/// This provides high-quality band-limited interpolation.
/// The window reduces ripple artifacts from truncating the sinc.
#[inline]
fn windowed_sinc(x: f64) -> f64 {
    if x.abs() < 1e-10 {
        return 1.0;
    }

    let sinc = (x * std::f64::consts::PI).sin() / (x * std::f64::consts::PI);

    // Blackman-Harris window (better stopband than Hann)
    // Window width is 8 samples (-4 to +4)
    let n = (x + 4.0) / 8.0; // Normalize to 0..1
    if n < 0.0 || n > 1.0 {
        return 0.0;
    }

    let a0 = 0.35875;
    let a1 = 0.48829;
    let a2 = 0.14128;
    let a3 = 0.01168;
    let tau = 2.0 * std::f64::consts::PI;

    let window = a0 - a1 * (tau * n).cos() + a2 * (2.0 * tau * n).cos() - a3 * (3.0 * tau * n).cos();

    sinc * window
}

/// 8-tap sinc interpolation with Blackman-Harris window
///
/// Highest quality interpolation - properly band-limited resampling.
/// Uses 8 samples (4 on each side) with windowed sinc kernel.
#[inline]
fn sinc_interpolate(samples: &[StereoSample; 8], t: f64) -> StereoSample {
    let mut left = 0.0f64;
    let mut right = 0.0f64;
    let mut weight_sum = 0.0f64;

    for (i, sample) in samples.iter().enumerate() {
        // Distance from interpolation point
        // samples[0..8] correspond to positions [-3, -2, -1, 0, 1, 2, 3, 4] relative to floor
        let x = (i as f64 - 3.0) - t;
        let weight = windowed_sinc(x);
        weight_sum += weight;
        left += sample.left as f64 * weight;
        right += sample.right as f64 * weight;
    }

    // Normalize (should be close to 1.0 but normalize for safety)
    if weight_sum.abs() > 1e-10 {
        left /= weight_sum;
        right /= weight_sum;
    }

    StereoSample {
        left: left as f32,
        right: right as f32,
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
        InterpolationMethod::Sinc => {
            // Gather 8 samples for sinc interpolation
            let samples: [StereoSample; 8] = [
                get_sample(data, index - 3, len),
                get_sample(data, index - 2, len),
                get_sample(data, index - 1, len),
                get_sample(data, index, len),
                get_sample(data, index + 1, len),
                get_sample(data, index + 2, len),
                get_sample(data, index + 3, len),
                get_sample(data, index + 4, len),
            ];
            sinc_interpolate(&samples, frac as f64)
        }
    }
}

/// Process scratch audio for a single stem
///
/// Generates `output_len` samples by reading from `source` at variable speed.
/// The velocity determines playback speed: 1.0 = normal, 2.0 = double speed,
/// -1.0 = reverse normal speed.
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
        // Read with interpolation at current position
        if pos >= 0.0 && pos < source_len as f64 {
            *sample = read_interpolated(source, pos, method);
        } else {
            *sample = StereoSample::silence();
        }

        // Advance position by velocity
        pos += velocity;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_interpolation() {
        let s0 = StereoSample { left: 0.0, right: 0.0 };
        let s1 = StereoSample { left: 1.0, right: 1.0 };

        let mid = lerp_sample(s0, s1, 0.5);
        assert!((mid.left - 0.5).abs() < 0.001);
        assert!((mid.right - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_continuous_read_position() {
        let mut state = ScratchState::new();
        state.start(1000);

        // Simulate steady movement at ~1x speed (256 samples per 256-sample buffer)
        state.move_to(1256);
        let (_, vel1, pos1) = state.update(256);

        state.move_to(1512);
        let (_, vel2, pos2) = state.update(256);

        state.move_to(1768);
        let (_, vel3, pos3) = state.update(256);

        // Read positions should be continuous (no jumps)
        // pos2 should be close to pos1 + (vel1 * 256)
        // This ensures no clicks at buffer boundaries
        let expected_pos2 = pos1 + vel1 * 256.0;
        assert!((pos2 - expected_pos2).abs() < 1.0, "pos2={} expected={}", pos2, expected_pos2);
    }

    #[test]
    fn test_sinc_at_integer() {
        // At integer positions, sinc should return the exact sample
        let samples = [
            StereoSample { left: 0.1, right: 0.1 },
            StereoSample { left: 0.2, right: 0.2 },
            StereoSample { left: 0.3, right: 0.3 },
            StereoSample { left: 0.5, right: 0.5 }, // This is the "center" sample at t=0
            StereoSample { left: 0.7, right: 0.7 },
            StereoSample { left: 0.8, right: 0.8 },
            StereoSample { left: 0.9, right: 0.9 },
            StereoSample { left: 1.0, right: 1.0 },
        ];

        let result = sinc_interpolate(&samples, 0.0);
        // Should be close to samples[3] (0.5)
        assert!((result.left - 0.5).abs() < 0.05, "Got {}", result.left);
    }
}
