//! Shared State-Variable Filter (SVF)
//!
//! Cytomic/Simper SVF topology — numerically stable, provides simultaneous
//! lowpass, highpass, and bandpass outputs from a single 2-pole structure.
//! Cascade two of these for 24 dB/oct (Linkwitz-Riley style) slopes.

use crate::types::SAMPLE_RATE;

/// Simultaneous filter outputs from a single SVF tick
#[derive(Debug, Clone, Copy, Default)]
pub struct SvfOutput {
    pub low_l: f32,
    pub low_r: f32,
    pub high_l: f32,
    pub high_r: f32,
    pub band_l: f32,
    pub band_r: f32,
}

/// Stereo two-pole (12 dB/oct) state-variable filter
///
/// Based on Andrew Simper's linearized trapezoidal integrator SVF, which is
/// unconditionally stable and free of the coefficient cramping that plagues
/// bilinear-transform designs near Nyquist.
#[derive(Debug, Clone)]
pub struct SvfFilter {
    // Per-channel integrator state
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
    /// Create a new SVF at 1 kHz with Butterworth Q
    pub fn new() -> Self {
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
        f.set_frequency(1000.0);
        f
    }

    /// Set cutoff and resonance (Q)
    pub fn set_params(&mut self, cutoff: f32, q: f32) {
        let cutoff = cutoff.clamp(20.0, 20000.0);
        let q = q.clamp(0.1, 40.0);

        self.g = (std::f32::consts::PI * cutoff / SAMPLE_RATE as f32).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }

    /// Shorthand: set cutoff with Butterworth Q (1/sqrt(2) ≈ 0.707)
    pub fn set_frequency(&mut self, cutoff: f32) {
        self.set_params(cutoff, std::f32::consts::FRAC_1_SQRT_2);
    }

    /// Process one stereo sample, returning all three filter outputs
    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> SvfOutput {
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

        SvfOutput {
            low_l,
            low_r,
            high_l,
            high_r,
            band_l,
            band_r,
        }
    }

    /// Clear integrator state (call on track load / silence)
    pub fn reset(&mut self) {
        self.ic1eq_l = 0.0;
        self.ic2eq_l = 0.0;
        self.ic1eq_r = 0.0;
        self.ic2eq_r = 0.0;
    }
}

impl Default for SvfFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_svf_dc_lowpass() {
        let mut svf = SvfFilter::new();
        svf.set_frequency(1000.0);

        // Feed DC — should pass through lowpass, be rejected by highpass
        let mut last = SvfOutput::default();
        for _ in 0..10_000 {
            last = svf.process(1.0, 1.0);
        }
        assert!((last.low_l - 1.0).abs() < 0.01, "DC should pass LP: {}", last.low_l);
        assert!(last.high_l.abs() < 0.01, "DC should be rejected by HP: {}", last.high_l);
    }

    #[test]
    fn test_svf_reset() {
        let mut svf = SvfFilter::new();
        svf.set_frequency(500.0);

        for _ in 0..1000 {
            svf.process(1.0, -1.0);
        }

        svf.reset();
        assert_eq!(svf.ic1eq_l, 0.0);
        assert_eq!(svf.ic2eq_l, 0.0);
        assert_eq!(svf.ic1eq_r, 0.0);
        assert_eq!(svf.ic2eq_r, 0.0);
    }

    #[test]
    fn test_svf_adaptive_q() {
        let mut svf = SvfFilter::new();

        // Low Q — should be well-behaved
        svf.set_params(1000.0, 0.5);
        let out_low_q = svf.process(1.0, 1.0);
        svf.reset();

        // High Q — same input, bandpass should be larger
        svf.set_params(1000.0, 5.0);
        let out_high_q = svf.process(1.0, 1.0);

        // With higher Q the band output for a step is larger (more resonance)
        assert!(
            out_high_q.band_l.abs() >= out_low_q.band_l.abs(),
            "Higher Q should give more band energy"
        );
    }
}
