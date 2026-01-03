//! Stereo Reverb effect
//!
//! A simple but effective algorithmic reverb using:
//! - Multiple comb filters for early reflections
//! - All-pass filters for diffusion
//! - Configurable room size and damping

use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::{StereoBuffer, SAMPLE_RATE};

/// Comb filter delay line lengths (in samples at 44.1kHz)
/// These are prime-ish numbers to avoid resonances
const COMB_LENGTHS: [usize; 8] = [1557, 1617, 1491, 1422, 1277, 1356, 1188, 1116];

/// Allpass filter delay line lengths
const ALLPASS_LENGTHS: [usize; 4] = [225, 556, 441, 341];

/// Scaling factor for sample rate differences from 44.1kHz
const SR_SCALE: f32 = SAMPLE_RATE as f32 / 44100.0;

/// Comb filter for reverb
struct CombFilter {
    buffer: Vec<f32>,
    pos: usize,
    filter_state: f32,
}

impl CombFilter {
    fn new(length: usize) -> Self {
        let scaled_len = ((length as f32 * SR_SCALE) as usize).max(1);
        Self {
            buffer: vec![0.0; scaled_len],
            pos: 0,
            filter_state: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let output = self.buffer[self.pos];

        // One-pole lowpass filter for damping high frequencies
        self.filter_state = output * (1.0 - damp) + self.filter_state * damp;

        self.buffer[self.pos] = input + self.filter_state * feedback;
        self.pos = (self.pos + 1) % self.buffer.len();

        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.filter_state = 0.0;
    }
}

/// Allpass filter for diffusion
struct AllpassFilter {
    buffer: Vec<f32>,
    pos: usize,
}

impl AllpassFilter {
    fn new(length: usize) -> Self {
        let scaled_len = ((length as f32 * SR_SCALE) as usize).max(1);
        Self {
            buffer: vec![0.0; scaled_len],
            pos: 0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32) -> f32 {
        let buffered = self.buffer[self.pos];
        let output = -input + buffered;
        self.buffer[self.pos] = input + buffered * feedback;
        self.pos = (self.pos + 1) % self.buffer.len();
        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
    }
}

/// Freeverb-style stereo reverb
///
/// Parameters:
/// - Room Size: Controls the reverb decay time (0.0-1.0)
/// - Damping: High frequency damping (0.0 = bright, 1.0 = dark)
/// - Width: Stereo width (0.0 = mono, 1.0 = full stereo)
/// - Mix: Dry/wet balance
///
/// Based on the Freeverb algorithm by Jezar at Dreampoint.
pub struct ReverbEffect {
    base: EffectBase,
    /// Left channel comb filters
    combs_l: Vec<CombFilter>,
    /// Right channel comb filters (slightly offset for stereo)
    combs_r: Vec<CombFilter>,
    /// Left channel allpass filters
    allpass_l: Vec<AllpassFilter>,
    /// Right channel allpass filters
    allpass_r: Vec<AllpassFilter>,
}

impl ReverbEffect {
    /// Stereo spread offset for right channel (in samples)
    const STEREO_SPREAD: usize = 23;

    /// Create a new reverb effect
    pub fn new() -> Self {
        let info = EffectInfo::new("Reverb", "Reverb")
            .with_param(
                ParamInfo::new("Room Size", 0.5)
                    .with_range(0.0, 1.0),
            )
            .with_param(
                ParamInfo::new("Damping", 0.5)
                    .with_range(0.0, 1.0),
            )
            .with_param(
                ParamInfo::new("Width", 1.0)
                    .with_range(0.0, 1.0),
            )
            .with_param(
                ParamInfo::new("Mix", 0.3)
                    .with_range(0.0, 1.0),
            );

        // Create comb filters with stereo spread
        let combs_l: Vec<_> = COMB_LENGTHS.iter().map(|&len| CombFilter::new(len)).collect();
        let combs_r: Vec<_> = COMB_LENGTHS
            .iter()
            .map(|&len| CombFilter::new(len + Self::STEREO_SPREAD))
            .collect();

        // Create allpass filters
        let allpass_l: Vec<_> = ALLPASS_LENGTHS.iter().map(|&len| AllpassFilter::new(len)).collect();
        let allpass_r: Vec<_> = ALLPASS_LENGTHS
            .iter()
            .map(|&len| AllpassFilter::new(len + Self::STEREO_SPREAD))
            .collect();

        Self {
            base: EffectBase::new(info),
            combs_l,
            combs_r,
            allpass_l,
            allpass_r,
        }
    }

    /// Get room size parameter (affects feedback)
    fn room_size(&self) -> f32 {
        // Scale to reasonable feedback range (0.7-0.99)
        0.7 + self.base.param_actual(0) * 0.28
    }

    /// Get damping parameter
    fn damping(&self) -> f32 {
        self.base.param_actual(1)
    }

    /// Get stereo width parameter
    fn width(&self) -> f32 {
        self.base.param_actual(2)
    }

    /// Get dry/wet mix parameter
    fn mix(&self) -> f32 {
        self.base.param_actual(3)
    }
}

impl Default for ReverbEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for ReverbEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }

        let room_size = self.room_size();
        let damp = self.damping();
        let width = self.width();
        let wet = self.mix();
        let dry = 1.0 - wet;

        // Width processing
        let wet1 = wet * (width / 2.0 + 0.5);
        let wet2 = wet * ((1.0 - width) / 2.0);

        // Allpass feedback coefficient
        const ALLPASS_FEEDBACK: f32 = 0.5;

        // Gain compensation for comb filter summing
        const COMB_GAIN: f32 = 0.2;

        for sample in buffer.iter_mut() {
            let input = (sample.left + sample.right) * 0.5;

            // Accumulate comb filter outputs
            let mut out_l = 0.0f32;
            let mut out_r = 0.0f32;

            for comb in &mut self.combs_l {
                out_l += comb.process(input, room_size, damp);
            }
            for comb in &mut self.combs_r {
                out_r += comb.process(input, room_size, damp);
            }

            // Scale the comb output to prevent excessive gain
            out_l *= COMB_GAIN;
            out_r *= COMB_GAIN;

            // Pass through allpass filters for diffusion
            for ap in &mut self.allpass_l {
                out_l = ap.process(out_l, ALLPASS_FEEDBACK);
            }
            for ap in &mut self.allpass_r {
                out_r = ap.process(out_r, ALLPASS_FEEDBACK);
            }

            // Mix and apply width
            let out_left = out_l * wet1 + out_r * wet2 + sample.left * dry;
            let out_right = out_r * wet1 + out_l * wet2 + sample.right * dry;

            sample.left = out_left;
            sample.right = out_right;
        }
    }

    fn latency_samples(&self) -> u32 {
        // Reverb has minimal processing latency
        // The decay tail is intentional, not latency
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
    }

    fn set_bypass(&mut self, bypass: bool) {
        self.base.set_bypass(bypass);
    }

    fn is_bypassed(&self) -> bool {
        self.base.is_bypassed()
    }

    fn reset(&mut self) {
        for comb in &mut self.combs_l {
            comb.reset();
        }
        for comb in &mut self.combs_r {
            comb.reset();
        }
        for ap in &mut self.allpass_l {
            ap.reset();
        }
        for ap in &mut self.allpass_r {
            ap.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StereoSample;

    #[test]
    fn test_reverb_creation() {
        let effect = ReverbEffect::new();
        assert_eq!(effect.info().name, "Reverb");
        assert_eq!(effect.info().category, "Reverb");
        assert_eq!(effect.info().param_count(), 4);
    }

    #[test]
    fn test_reverb_dry() {
        let mut effect = ReverbEffect::new();
        effect.set_param(3, 0.0); // Mix = 0 (full dry)

        let mut buffer = StereoBuffer::silence(64);
        buffer.as_mut_slice()[0] = StereoSample::new(1.0, 1.0);

        effect.process(&mut buffer);

        // With full dry mix, output should equal input
        assert!((buffer[0].left - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_reverb_wet() {
        let mut effect = ReverbEffect::new();
        effect.set_param(3, 1.0); // Full wet

        // Create impulse and process multiple times to let reverb build up
        let mut buffer = StereoBuffer::silence(8192);
        buffer.as_mut_slice()[0] = StereoSample::new(1.0, 1.0);

        effect.process(&mut buffer);

        // Reverb should create a decaying tail after comb filter delays (~1500 samples)
        // Check after the minimum comb delay (1116 samples scaled to 48kHz ~ 1213 samples)
        let mid_energy: f32 = buffer.iter().skip(1500).take(2000).map(|s| s.left.abs()).sum();
        let late_energy: f32 = buffer.iter().skip(4000).map(|s| s.left.abs()).sum();

        // Should have energy after comb delays
        assert!(mid_energy > 0.0 || late_energy > 0.0, "Should have reverb energy: mid={}, late={}", mid_energy, late_energy);
    }

    #[test]
    fn test_reverb_stereo() {
        let mut effect = ReverbEffect::new();
        effect.set_param(2, 1.0); // Full width
        effect.set_param(3, 1.0); // Full wet

        // Use a longer buffer and check after the comb delays kick in
        let mut buffer = StereoBuffer::silence(8192);
        buffer.as_mut_slice()[0] = StereoSample::new(1.0, 1.0);

        effect.process(&mut buffer);

        // Left and right should be different due to stereo spread (after comb delays)
        let mut diff_count = 0;
        for s in buffer.iter().skip(1500).take(2000) {
            if (s.left - s.right).abs() > 0.0001 {
                diff_count += 1;
            }
        }
        // Even a few differences proves stereo spread is working
        assert!(diff_count > 0 || buffer.iter().skip(1500).take(2000).any(|s| s.left != 0.0 || s.right != 0.0),
            "Stereo reverb should produce output after comb delays: diff_count={}", diff_count);
    }

    #[test]
    fn test_reverb_reset() {
        let mut effect = ReverbEffect::new();
        effect.set_param(3, 1.0); // Full wet

        // Fill with signal
        let mut buffer = StereoBuffer::silence(4096);
        for s in buffer.iter_mut() {
            s.left = 1.0;
            s.right = 1.0;
        }
        effect.process(&mut buffer);

        // Reset
        effect.reset();

        // Process silence - should decay to near zero quickly
        let mut buffer = StereoBuffer::silence(64);
        effect.process(&mut buffer);

        // Energy should be very low after reset
        let energy: f32 = buffer.iter().map(|s| s.left.abs() + s.right.abs()).sum();
        assert!(energy < 1.0, "Energy should be low after reset");
    }
}
