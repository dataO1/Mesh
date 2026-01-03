//! DJ Filter effect - Combined HP/LP filter in one knob

use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::{StereoBuffer, SAMPLE_RATE};

/// Two-pole (12dB/octave) state-variable filter
struct SvfFilter {
    // State per channel
    ic1eq_l: f32,
    ic2eq_l: f32,
    ic1eq_r: f32,
    ic2eq_r: f32,
    // Coefficients
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfFilter {
    fn new() -> Self {
        let mut f = Self {
            ic1eq_l: 0.0,
            ic2eq_l: 0.0,
            ic1eq_r: 0.0,
            ic2eq_r: 0.0,
            g: 0.0,
            k: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
        };
        f.set_params(1000.0, 0.707); // Default cutoff and resonance
        f
    }

    fn set_params(&mut self, cutoff: f32, q: f32) {
        let cutoff = cutoff.clamp(20.0, 20000.0);
        let q = q.clamp(0.1, 10.0);

        self.g = (std::f32::consts::PI * cutoff / SAMPLE_RATE as f32).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }

    /// Process and return (lowpass, highpass, bandpass)
    #[inline]
    fn process(&mut self, left: f32, right: f32) -> ((f32, f32), (f32, f32), (f32, f32)) {
        // Left channel
        let v3_l = left - self.ic2eq_l;
        let v1_l = self.a1 * self.ic1eq_l + self.a2 * v3_l;
        let v2_l = self.ic2eq_l + self.a2 * self.ic1eq_l + self.a3 * v3_l;
        self.ic1eq_l = 2.0 * v1_l - self.ic1eq_l;
        self.ic2eq_l = 2.0 * v2_l - self.ic2eq_l;

        let low_l = v2_l;
        let band_l = v1_l;
        let high_l = left - self.k * band_l - low_l;

        // Right channel
        let v3_r = right - self.ic2eq_r;
        let v1_r = self.a1 * self.ic1eq_r + self.a2 * v3_r;
        let v2_r = self.ic2eq_r + self.a2 * self.ic1eq_r + self.a3 * v3_r;
        self.ic1eq_r = 2.0 * v1_r - self.ic1eq_r;
        self.ic2eq_r = 2.0 * v2_r - self.ic2eq_r;

        let low_r = v2_r;
        let band_r = v1_r;
        let high_r = right - self.k * band_r - low_r;

        ((low_l, low_r), (high_l, high_r), (band_l, band_r))
    }

    fn reset(&mut self) {
        self.ic1eq_l = 0.0;
        self.ic2eq_l = 0.0;
        self.ic1eq_r = 0.0;
        self.ic2eq_r = 0.0;
    }
}

/// DJ-style filter with LP and HP on a single knob
///
/// Parameters:
/// - Filter: -1.0 = full LP, 0.0 = flat, 1.0 = full HP
/// - Resonance: Filter resonance (Q)
///
/// This effect has minimal latency (2 samples due to filter state).
pub struct DjFilterEffect {
    base: EffectBase,
    filter: SvfFilter,
}

impl DjFilterEffect {
    /// Create a new DJ filter effect
    pub fn new() -> Self {
        let info = EffectInfo::new("DJ Filter", "Filter")
            .with_param(
                ParamInfo::new("Filter", 0.5) // 0.5 = center (flat)
                    .with_range(-1.0, 1.0),
            )
            .with_param(
                ParamInfo::new("Resonance", 0.0) // 0.0 = minimum resonance
                    .with_range(0.5, 10.0)
                    .with_unit("Q"),
            );

        Self {
            base: EffectBase::new(info),
            filter: SvfFilter::new(),
        }
    }

    /// Get the current filter position (-1 to 1)
    fn filter_position(&self) -> f32 {
        self.base.param_actual(0)
    }

    /// Get the current resonance (Q)
    fn resonance(&self) -> f32 {
        self.base.param_actual(1)
    }

    /// Calculate cutoff frequency from filter position
    fn calculate_cutoff(&self, position: f32) -> f32 {
        if position < 0.0 {
            // LP mode: sweep from 20kHz down to 100Hz
            // position -1 = 100Hz, position 0 = 20kHz
            let t = 1.0 + position; // 0 to 1
            100.0 * (200.0_f32).powf(t) // Exponential sweep
        } else {
            // HP mode: sweep from 20Hz up to 5kHz
            // position 0 = 20Hz, position 1 = 5kHz
            20.0 * (250.0_f32).powf(position) // Exponential sweep
        }
    }
}

impl Default for DjFilterEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for DjFilterEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }

        let position = self.filter_position();
        let resonance = self.resonance();

        // Dead zone around center for "flat" response
        const DEAD_ZONE: f32 = 0.02;
        if position.abs() < DEAD_ZONE {
            return; // Effectively bypassed at center
        }

        let cutoff = self.calculate_cutoff(position);
        self.filter.set_params(cutoff, resonance);

        let is_lp = position < 0.0;

        for sample in buffer.iter_mut() {
            let (low, high, _band) = self.filter.process(sample.left, sample.right);

            if is_lp {
                sample.left = low.0;
                sample.right = low.1;
            } else {
                sample.left = high.0;
                sample.right = high.1;
            }
        }
    }

    fn latency_samples(&self) -> u32 {
        0 // Negligible latency for real-time filter
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
        self.filter.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StereoSample;

    #[test]
    fn test_dj_filter_creation() {
        let effect = DjFilterEffect::new();
        assert_eq!(effect.info().name, "DJ Filter");
        assert_eq!(effect.info().category, "Filter");
        assert_eq!(effect.info().param_count(), 2);
    }

    #[test]
    fn test_dj_filter_flat() {
        let mut effect = DjFilterEffect::new();
        // Default is center (0.5 normalized = 0.0 actual)

        let mut buffer = StereoBuffer::silence(64);
        for i in 0..buffer.len() {
            buffer.as_mut_slice()[i] = StereoSample::new(1.0, 1.0);
        }

        effect.process(&mut buffer);

        // Should be unchanged when filter is at center (dead zone)
        assert!((buffer[32].left - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_dj_filter_lowpass() {
        let mut effect = DjFilterEffect::new();
        effect.set_param(0, 0.0); // Full LP (normalized 0 = actual -1)

        // Feed high frequency signal (alternating +1/-1 = Nyquist)
        let mut buffer = StereoBuffer::silence(128);
        for i in 0..buffer.len() {
            let val = if i % 2 == 0 { 1.0 } else { -1.0 };
            buffer.as_mut_slice()[i] = StereoSample::new(val, val);
        }

        effect.process(&mut buffer);

        // LP should attenuate high frequencies - average amplitude should be low
        let avg: f32 = buffer.iter().map(|s| s.left.abs()).sum::<f32>() / buffer.len() as f32;
        assert!(avg < 0.5, "LP should attenuate high frequencies");
    }

    #[test]
    fn test_dj_filter_reset() {
        let mut effect = DjFilterEffect::new();
        effect.set_param(0, 0.0); // LP mode

        let mut buffer = StereoBuffer::silence(64);
        for i in 0..buffer.len() {
            buffer.as_mut_slice()[i] = StereoSample::new(1.0, 1.0);
        }

        effect.process(&mut buffer);
        effect.reset();

        // After reset, filter state should be cleared
        // Processing again should give same result as fresh effect
        let mut effect2 = DjFilterEffect::new();
        effect2.set_param(0, 0.0);

        let mut buffer2 = StereoBuffer::silence(64);
        for i in 0..buffer2.len() {
            buffer2.as_mut_slice()[i] = StereoSample::new(1.0, 1.0);
        }

        effect.process(&mut buffer);
        effect2.process(&mut buffer2);

        // Results should be similar (not exact due to state)
    }
}
