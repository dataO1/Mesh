//! Master safety clipper — ClipOnly2-style stateful clipper
//!
//! Based on the Airwindows ClipOnly2 algorithm by Chris Johnson.
//! Uses the Dottie number (fixed point of cos(x) = x ≈ 0.739) as an
//! interpolation ratio for smooth clip entry/exit transitions.
//!
//! Properties:
//! - Pure bypass when signal is below threshold (zero processing)
//! - State-machine per channel: only activates when clipping occurs
//! - Sample-rate aware spacing for consistent behavior across rates
//! - ~1 sample latency at 48 kHz

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::types::{StereoBuffer, SAMPLE_RATE};

/// Dottie number: the unique fixed point of cos(x) = x.
/// Used as the interpolation weight favoring the ceiling value.
const HARDNESS: f32 = 0.739_085_13;
/// 1.0 - HARDNESS: interpolation weight favoring the signal value.
const SOFTNESS: f32 = 1.0 - HARDNESS;

/// Maximum supported spacing (supports sample rates up to ~352.8 kHz)
const MAX_SPACING: usize = 8;

/// Master safety clipper using ClipOnly2-style stateful interpolation.
///
/// When the signal is below the threshold, samples pass through untouched.
/// When clipping occurs, the algorithm interpolates smoothly into and out
/// of the ceiling using the Dottie number as a blending ratio.
pub struct MasterClipper {
    /// Clip threshold in linear amplitude
    threshold: f32,
    /// Pre-computed: threshold * HARDNESS
    thresh_hard: f32,
    /// Pre-computed: threshold * SOFTNESS
    thresh_soft: f32,

    // Per-channel state (0 = left, 1 = right)
    last_sample: [f32; 2],
    was_pos_clip: [bool; 2],
    was_neg_clip: [bool; 2],

    /// Intermediate buffer for sample-rate-aware spacing delay
    intermediate: [[f32; MAX_SPACING]; 2],
    /// Spacing = floor(sample_rate / 44100), typically 1 at 48 kHz
    spacing: usize,

    /// Atomic flag set when clipping occurs (for UI indicator).
    /// Audio thread sets to true; UI thread reads and clears.
    clip_active: Arc<AtomicBool>,
    /// Track whether any sample clipped during the current buffer
    clipped_this_buffer: bool,
}

impl MasterClipper {
    /// Create a clipper with the default threshold of -0.3 dBFS
    pub fn new() -> Self {
        Self::with_threshold_db(-0.3)
    }

    /// Create a clipper with a custom threshold in dBFS
    pub fn with_threshold_db(db: f32) -> Self {
        let threshold = 10.0_f32.powf(db / 20.0);
        let spacing = (SAMPLE_RATE as f32 / 44100.0).floor() as usize;
        let spacing = spacing.clamp(1, MAX_SPACING);

        Self {
            threshold,
            thresh_hard: threshold * HARDNESS,
            thresh_soft: threshold * SOFTNESS,
            last_sample: [0.0; 2],
            was_pos_clip: [false; 2],
            was_neg_clip: [false; 2],
            intermediate: [[0.0; MAX_SPACING]; 2],
            spacing,
            clip_active: Arc::new(AtomicBool::new(false)),
            clipped_this_buffer: false,
        }
    }

    /// Latency in samples introduced by this clipper
    pub fn latency_samples(&self) -> usize {
        self.spacing
    }

    /// Get the clip indicator atomic (shared with UI thread)
    pub fn clip_indicator(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.clip_active)
    }

    /// Process a stereo buffer in-place
    pub fn process(&mut self, buffer: &mut StereoBuffer) {
        self.clipped_this_buffer = false;
        for sample in buffer.iter_mut() {
            sample.left = self.process_sample(sample.left, 0);
            sample.right = self.process_sample(sample.right, 1);
        }
        if self.clipped_this_buffer {
            self.clip_active.store(true, Ordering::Relaxed);
        }
    }

    /// Process a single sample for one channel.
    /// Returns the sample unchanged if below threshold, or smoothly clipped.
    #[inline]
    fn process_sample(&mut self, input: f32, ch: usize) -> f32 {
        // Clamp extreme values to prevent runaway math
        let mut sample = input.clamp(-4.0, 4.0);

        // --- Positive clip state machine ---
        if self.was_pos_clip[ch] {
            if sample < self.last_sample[ch] {
                // Signal falling away from clip ceiling: smooth exit
                self.last_sample[ch] = self.thresh_hard + sample * SOFTNESS;
            } else {
                // Signal still at or rising toward clip: hold near ceiling
                self.last_sample[ch] = self.thresh_soft + self.last_sample[ch] * HARDNESS;
            }
        }
        self.was_pos_clip[ch] = false;

        if sample > self.threshold {
            // Entering positive clip
            self.was_pos_clip[ch] = true;
            self.clipped_this_buffer = true;
            sample = self.thresh_hard + self.last_sample[ch] * SOFTNESS;
        }

        // --- Negative clip state machine ---
        if self.was_neg_clip[ch] {
            if sample > self.last_sample[ch] {
                // Signal rising away from negative clip: smooth exit
                self.last_sample[ch] = -self.thresh_hard + sample * SOFTNESS;
            } else {
                // Signal still at or falling toward negative clip: hold
                self.last_sample[ch] = -self.thresh_soft + self.last_sample[ch] * HARDNESS;
            }
        }
        self.was_neg_clip[ch] = false;

        if sample < -self.threshold {
            // Entering negative clip
            self.was_neg_clip[ch] = true;
            self.clipped_this_buffer = true;
            sample = -self.thresh_hard + self.last_sample[ch] * SOFTNESS;
        }

        // --- Sample-rate aware spacing delay ---
        self.intermediate[ch][self.spacing - 1] = sample;
        let output = self.last_sample[ch];
        self.last_sample[ch] = self.intermediate[ch][0];
        // Shift buffer down
        for x in 0..self.spacing - 1 {
            self.intermediate[ch][x] = self.intermediate[ch][x + 1];
        }

        output
    }
}

impl Default for MasterClipper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StereoSample;

    fn make_buffer(samples: &[(f32, f32)]) -> StereoBuffer {
        let mut buf = StereoBuffer::with_capacity(samples.len());
        buf.resize(samples.len());
        for (i, &(l, r)) in samples.iter().enumerate() {
            buf.as_mut_slice()[i] = StereoSample::new(l, r);
        }
        buf
    }

    #[test]
    fn test_below_threshold_bypass() {
        let mut clipper = MasterClipper::new();
        let threshold = clipper.threshold;

        // Feed a few zero samples to flush the 1-sample delay
        let mut warmup = make_buffer(&[(0.0, 0.0); 4]);
        clipper.process(&mut warmup);

        // Samples well below threshold should pass through unchanged
        let level = threshold * 0.5;
        let mut buf = make_buffer(&[(level, -level); 8]);
        clipper.process(&mut buf);

        // After the initial delay sample, all outputs should match input
        for i in 1..8 {
            let s = buf.as_slice()[i];
            assert!(
                (s.left - level).abs() < 1e-6,
                "left[{}] = {} expected {}",
                i, s.left, level
            );
            assert!(
                (s.right - (-level)).abs() < 1e-6,
                "right[{}] = {} expected {}",
                i, s.right, -level
            );
        }
    }

    #[test]
    fn test_clipping_reduces_level() {
        let mut clipper = MasterClipper::new();
        let threshold = clipper.threshold;

        // Warmup
        let mut warmup = make_buffer(&[(0.0, 0.0); 4]);
        clipper.process(&mut warmup);

        // Samples above threshold should be reduced
        let hot = threshold * 1.5;
        let mut buf = make_buffer(&[(hot, hot); 16]);
        clipper.process(&mut buf);

        // All output samples should be at or below threshold
        for i in 0..16 {
            let s = buf.as_slice()[i];
            assert!(
                s.left <= threshold + 0.01,
                "left[{}] = {} exceeds threshold {}",
                i, s.left, threshold
            );
            assert!(
                s.right <= threshold + 0.01,
                "right[{}] = {} exceeds threshold {}",
                i, s.right, threshold
            );
        }
    }

    #[test]
    fn test_negative_clipping() {
        let mut clipper = MasterClipper::new();
        let threshold = clipper.threshold;

        let mut warmup = make_buffer(&[(0.0, 0.0); 4]);
        clipper.process(&mut warmup);

        let hot = -threshold * 1.5;
        let mut buf = make_buffer(&[(hot, hot); 16]);
        clipper.process(&mut buf);

        for i in 0..16 {
            let s = buf.as_slice()[i];
            assert!(
                s.left >= -threshold - 0.01,
                "left[{}] = {} exceeds negative threshold {}",
                i, s.left, -threshold
            );
        }
    }

    #[test]
    fn test_latency() {
        let clipper = MasterClipper::new();
        // At 48 kHz, spacing = floor(48000/44100) = 1
        assert_eq!(clipper.latency_samples(), 1);
    }
}
