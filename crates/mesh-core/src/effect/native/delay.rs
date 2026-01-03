//! Stereo Delay effect
//!
//! A tempo-synced stereo delay with:
//! - Delay time (in beats or ms)
//! - Feedback control
//! - Dry/wet mix
//! - Optional ping-pong mode

use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::{StereoBuffer, SAMPLE_RATE};

/// Maximum delay time in seconds
const MAX_DELAY_SECONDS: f32 = 2.0;
/// Maximum delay buffer size in samples per channel
const MAX_DELAY_SAMPLES: usize = (SAMPLE_RATE as f32 * MAX_DELAY_SECONDS) as usize;

/// Stereo delay line for the delay effect
struct DelayLine {
    /// Left channel buffer
    buffer_l: Vec<f32>,
    /// Right channel buffer
    buffer_r: Vec<f32>,
    /// Write position
    write_pos: usize,
    /// Current delay time in samples
    delay_samples: usize,
}

impl DelayLine {
    fn new() -> Self {
        Self {
            buffer_l: vec![0.0; MAX_DELAY_SAMPLES],
            buffer_r: vec![0.0; MAX_DELAY_SAMPLES],
            write_pos: 0,
            delay_samples: SAMPLE_RATE as usize / 4, // Default 250ms
        }
    }

    fn set_delay_samples(&mut self, samples: usize) {
        self.delay_samples = samples.min(MAX_DELAY_SAMPLES - 1);
    }

    /// Read from delay line at current position minus delay
    #[inline]
    fn read(&self) -> (f32, f32) {
        let read_pos = if self.write_pos >= self.delay_samples {
            self.write_pos - self.delay_samples
        } else {
            MAX_DELAY_SAMPLES - (self.delay_samples - self.write_pos)
        };
        (self.buffer_l[read_pos], self.buffer_r[read_pos])
    }

    /// Write to delay line and advance position
    #[inline]
    fn write(&mut self, left: f32, right: f32) {
        self.buffer_l[self.write_pos] = left;
        self.buffer_r[self.write_pos] = right;
        self.write_pos = (self.write_pos + 1) % MAX_DELAY_SAMPLES;
    }

    /// Process one sample through the delay with feedback
    #[inline]
    fn process(&mut self, left: f32, right: f32, feedback: f32, ping_pong: bool) -> (f32, f32) {
        let (delayed_l, delayed_r) = self.read();

        // Apply feedback - ping-pong swaps L/R in feedback path
        let (fb_l, fb_r) = if ping_pong {
            (delayed_r * feedback, delayed_l * feedback)
        } else {
            (delayed_l * feedback, delayed_r * feedback)
        };

        // Write input + feedback to delay line
        self.write(left + fb_l, right + fb_r);

        (delayed_l, delayed_r)
    }

    fn reset(&mut self) {
        self.buffer_l.fill(0.0);
        self.buffer_r.fill(0.0);
        self.write_pos = 0;
    }
}

/// Stereo delay effect with tempo sync capability
///
/// Parameters:
/// - Time: Delay time in ms (10-2000ms)
/// - Feedback: Amount of signal fed back (0-95%)
/// - Mix: Dry/wet balance (0% = dry, 100% = wet)
/// - Ping-pong: Enable ping-pong mode (L/R alternating)
///
/// This effect has latency equal to the delay time (reported for compensation).
pub struct DelayEffect {
    base: EffectBase,
    delay_line: DelayLine,
    /// Current BPM (for tempo sync, set externally)
    bpm: f64,
}

impl DelayEffect {
    /// Create a new delay effect
    pub fn new() -> Self {
        let info = EffectInfo::new("Stereo Delay", "Delay")
            .with_param(
                ParamInfo::new("Time", 0.25) // Default ~375ms
                    .with_range(10.0, 2000.0)
                    .with_unit("ms"),
            )
            .with_param(
                ParamInfo::new("Feedback", 0.4) // 40%
                    .with_range(0.0, 0.95),
            )
            .with_param(
                ParamInfo::new("Mix", 0.3) // 30% wet
                    .with_range(0.0, 1.0),
            )
            .with_param(
                ParamInfo::new("Ping-Pong", 0.0) // Off by default
                    .with_range(0.0, 1.0),
            );

        let mut effect = Self {
            base: EffectBase::new(info),
            delay_line: DelayLine::new(),
            bpm: 120.0,
        };

        // Initialize delay time from default parameter
        effect.update_delay_time();
        effect
    }

    /// Set BPM for tempo-sync calculations (called from engine)
    pub fn set_bpm(&mut self, bpm: f64) {
        self.bpm = bpm.max(30.0).min(300.0);
    }

    /// Get delay time in ms
    fn delay_time_ms(&self) -> f32 {
        self.base.param_actual(0)
    }

    /// Get feedback amount (0.0-0.95)
    fn feedback(&self) -> f32 {
        self.base.param_actual(1)
    }

    /// Get dry/wet mix (0.0-1.0)
    fn mix(&self) -> f32 {
        self.base.param_actual(2)
    }

    /// Check if ping-pong mode is enabled
    fn ping_pong(&self) -> bool {
        self.base.param_actual(3) > 0.5
    }

    /// Update internal delay time from parameters
    fn update_delay_time(&mut self) {
        let time_ms = self.delay_time_ms();
        let samples = (time_ms / 1000.0 * SAMPLE_RATE as f32) as usize;
        self.delay_line.set_delay_samples(samples);
    }
}

impl Default for DelayEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for DelayEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }

        // Update delay time in case it changed
        self.update_delay_time();

        let feedback = self.feedback();
        let mix = self.mix();
        let ping_pong = self.ping_pong();
        let dry = 1.0 - mix;

        for sample in buffer.iter_mut() {
            let (delayed_l, delayed_r) =
                self.delay_line
                    .process(sample.left, sample.right, feedback, ping_pong);

            // Mix dry and wet signals
            sample.left = sample.left * dry + delayed_l * mix;
            sample.right = sample.right * dry + delayed_r * mix;
        }
    }

    fn latency_samples(&self) -> u32 {
        // Delay doesn't add processing latency, only time shift
        // The delay time itself is intentional, not compensated
        0
    }

    fn info(&self) -> &EffectInfo {
        self.base.info()
    }

    fn get_params(&self) -> &[ParamValue] {
        self.base.get_params()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.base.set_param(index, value);
        if index == 0 {
            // Time parameter changed, update delay line
            self.update_delay_time();
        }
    }

    fn set_bypass(&mut self, bypass: bool) {
        self.base.set_bypass(bypass);
    }

    fn is_bypassed(&self) -> bool {
        self.base.is_bypassed()
    }

    fn reset(&mut self) {
        self.delay_line.reset();
    }
}

/// Tempo-synced delay times (in beats)
/// Common DJ delay times for beat-locked effects
pub const TEMPO_SYNC_VALUES: [(f32, &str); 8] = [
    (0.125, "1/32"),
    (0.25, "1/16"),
    (0.333, "1/8T"),
    (0.5, "1/8"),
    (0.667, "1/4T"),
    (1.0, "1/4"),
    (1.5, "3/8"),
    (2.0, "1/2"),
];

/// Convert beats to milliseconds at given BPM
pub fn beats_to_ms(beats: f32, bpm: f64) -> f32 {
    let beat_duration_ms = 60_000.0 / bpm as f32;
    beats * beat_duration_ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StereoSample;

    #[test]
    fn test_delay_creation() {
        let effect = DelayEffect::new();
        assert_eq!(effect.info().name, "Stereo Delay");
        assert_eq!(effect.info().category, "Delay");
        assert_eq!(effect.info().param_count(), 4);
    }

    #[test]
    fn test_delay_dry() {
        let mut effect = DelayEffect::new();
        effect.set_param(2, 0.0); // Mix = 0 (full dry)

        let mut buffer = StereoBuffer::silence(64);
        buffer.as_mut_slice()[0] = StereoSample::new(1.0, 1.0);

        effect.process(&mut buffer);

        // With full dry mix, output should equal input
        assert!((buffer[0].left - 1.0).abs() < 0.01);
        // Later samples should be silent (no wet signal)
        assert!(buffer[32].left.abs() < 0.01);
    }

    #[test]
    fn test_delay_wet() {
        let mut effect = DelayEffect::new();
        effect.set_param(0, 0.05); // ~100ms delay (normalized)
        effect.set_param(1, 0.0); // No feedback
        effect.set_param(2, 1.0); // Full wet

        // Create impulse
        let mut buffer = StereoBuffer::silence(8192);
        buffer.as_mut_slice()[0] = StereoSample::new(1.0, 1.0);

        effect.process(&mut buffer);

        // First sample should be from delay (which is empty = 0)
        assert!(buffer[0].left.abs() < 0.01);

        // There should be a delayed impulse later
        let delay_samples = (100.0 / 1000.0 * SAMPLE_RATE as f32) as usize;
        if delay_samples < buffer.len() {
            // The impulse should appear around the delay time
            let found_impulse = buffer.iter().skip(delay_samples / 2).take(delay_samples).any(|s| s.left.abs() > 0.5);
            assert!(found_impulse, "Should find delayed impulse");
        }
    }

    #[test]
    fn test_delay_feedback() {
        let mut effect = DelayEffect::new();
        // Set a 50ms delay (normalized: 50ms in range 10-2000ms = (50-10)/(2000-10) ≈ 0.02)
        effect.set_param(0, 0.02);
        effect.set_param(1, 0.5); // 50% feedback (normalized -> ~47.5% actual)
        effect.set_param(2, 1.0); // 100% wet to clearly see delayed signal

        // We need a buffer at least 2-3x the delay time to see feedback repeats
        // 50ms at 48kHz = 2400 samples, so use 8192 samples
        let mut buffer = StereoBuffer::silence(8192);
        buffer.as_mut_slice()[0] = StereoSample::new(1.0, 1.0);

        effect.process(&mut buffer);

        // Calculate expected delay in samples (50ms = 2400 samples at 48kHz)
        let delay_samples = (50.0 / 1000.0 * SAMPLE_RATE as f32) as usize;

        // Check for energy after the delay point (should have at least the first echo)
        let after_delay_energy: f32 = buffer.iter()
            .skip(delay_samples)
            .take(delay_samples)
            .map(|s| s.left.abs())
            .sum();

        assert!(after_delay_energy > 0.1,
            "Should have delayed signal after {}ms (samples {}): energy={}",
            50.0, delay_samples, after_delay_energy);
    }

    #[test]
    fn test_delay_reset() {
        let mut effect = DelayEffect::new();
        effect.set_param(0, 0.1); // ~200ms
        effect.set_param(2, 1.0); // Full wet

        // Fill delay buffer with signal
        let mut buffer = StereoBuffer::silence(4096);
        for s in buffer.iter_mut() {
            s.left = 1.0;
            s.right = 1.0;
        }
        effect.process(&mut buffer);

        // Reset
        effect.reset();

        // Process silence - should get silence out
        let mut buffer = StereoBuffer::silence(64);
        effect.process(&mut buffer);

        // All samples should be near zero after reset
        for s in buffer.iter() {
            assert!(s.left.abs() < 0.01, "Buffer should be clear after reset");
        }
    }

    #[test]
    fn test_tempo_sync() {
        // Test beats to ms conversion
        let ms = beats_to_ms(1.0, 120.0); // 1 beat at 120 BPM = 500ms
        assert!((ms - 500.0).abs() < 0.1);

        let ms = beats_to_ms(0.5, 120.0); // Half beat = 250ms
        assert!((ms - 250.0).abs() < 0.1);
    }
}
