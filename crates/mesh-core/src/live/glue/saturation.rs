//! Soft saturation with first-order antiderivative anti-aliasing (ADAA).
//!
//! Waveshaper: `f(u) = tanh(u + b) − tanh(b)` with drive `u = g·x` and bias
//! `b` for asymmetry (even harmonics). `f(0) = 0`, `f` is monotonic and
//! bounded by `1 + |tanh b|`.
//!
//! **Anti-aliasing choice: ADAA, not oversampling.** First-order ADAA
//! (Parker, Zavalishin & Le Bivic, DAFx 2016) outputs the mean of `f` over
//! the segment between consecutive inputs, `(F(u_n) − F(u_{n−1})) / (u_n − u_{n−1})`
//! with `F(u) = ln cosh(u + b) − u·tanh(b)`. It suppresses aliasing of the
//! upper harmonics by roughly the same amount as 2× oversampling at a fraction
//! of the cost (no up/down-sampling filters), which matters on the A76 budget.
//! Costs: a half-sample delay and a gentle high-frequency roll-off
//! (−3 dB near 0.45·fs) on the wet path. `F` is evaluated in f64 because the
//! difference quotient cancels badly in f32.
//!
//! A DC blocker (10 Hz one-pole high-pass) follows the shaper when the bias
//! is non-zero, since asymmetric shaping adds DC.

use crate::types::{StereoSample, SAMPLE_RATE};

use super::db_to_lin;
use super::params::SaturationParams;

/// Below this input step the quotient is ill-conditioned; use the midpoint.
const ADAA_EPS: f64 = 1e-5;
const DC_BLOCK_HZ: f32 = 10.0;

/// Numerically stable `ln(cosh(v))`.
#[inline]
fn ln_cosh(v: f64) -> f64 {
    let a = v.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

/// Per-channel ADAA tanh state.
#[derive(Debug, Clone, Copy, Default)]
struct AdaaTanh {
    prev_u: f64,
    prev_f: f64,
}

impl AdaaTanh {
    #[inline]
    fn process(&mut self, u: f64, bias: f64, tanh_bias: f64) -> f64 {
        let big_f = ln_cosh(u + bias) - u * tanh_bias;
        let du = u - self.prev_u;
        let y = if du.abs() > ADAA_EPS {
            (big_f - self.prev_f) / du
        } else {
            ((0.5 * (u + self.prev_u) + bias).tanh()) - tanh_bias
        };
        self.prev_u = u;
        self.prev_f = big_f;
        y
    }

    /// Recompute the stored antiderivative after the bias changed, so the
    /// next difference quotient doesn't spike.
    fn rebias(&mut self, bias: f64, tanh_bias: f64) {
        self.prev_f = ln_cosh(self.prev_u + bias) - self.prev_u * tanh_bias;
    }
}

/// One-pole DC blocker: y = x − x₁ + R·y₁.
#[derive(Debug, Clone, Copy, Default)]
struct DcBlocker {
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    #[inline]
    fn process(&mut self, x: f32, r: f32) -> f32 {
        let y = x - self.x1 + r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

pub struct Saturator {
    drive: f64,
    bias: f64,
    tanh_bias: f64,
    /// Makes a full-scale input come out near full scale: 1 / f(g).
    makeup: f32,
    mix: f32,
    output: f32,
    shaper: [AdaaTanh; 2],
    dc: [DcBlocker; 2],
    dc_r: f32,
}

impl Saturator {
    pub fn new() -> Self {
        let mut s = Self {
            drive: 1.0,
            bias: 0.0,
            tanh_bias: 0.0,
            makeup: 1.0,
            mix: 1.0,
            output: 1.0,
            shaper: [AdaaTanh::default(); 2],
            dc: [DcBlocker::default(); 2],
            dc_r: 1.0 - 2.0 * std::f32::consts::PI * DC_BLOCK_HZ / SAMPLE_RATE as f32,
        };
        s.configure(&SaturationParams::default());
        s
    }

    pub fn configure(&mut self, p: &SaturationParams) {
        self.drive = db_to_lin(p.drive_db) as f64;
        self.bias = p.asymmetry.clamp(-0.9, 0.9) as f64;
        self.tanh_bias = self.bias.tanh();
        for sh in &mut self.shaper {
            sh.rebias(self.bias, self.tanh_bias);
        }
        let full_scale = (self.drive + self.bias).tanh() - self.tanh_bias;
        self.makeup = if full_scale > 1e-6 { (1.0 / full_scale) as f32 } else { 1.0 };
        self.mix = p.mix.clamp(0.0, 1.0);
        self.output = db_to_lin(p.output_db);
    }

    /// Upper bound of `|output|` for any input when `mix == 1`.
    pub fn wet_bound(&self) -> f32 {
        self.makeup * (1.0 + self.tanh_bias.abs() as f32) * self.output
    }

    /// The static (non-anti-aliased) transfer curve, for tests and metering.
    pub fn static_curve(&self, x: f32) -> f32 {
        let u = self.drive * x as f64;
        (((u + self.bias).tanh() - self.tanh_bias) as f32) * self.makeup * self.output
    }

    pub fn process(&mut self, buf: &mut [StereoSample]) {
        let (drive, bias, tb) = (self.drive, self.bias, self.tanh_bias);
        let dry = 1.0 - self.mix;
        let dc_on = bias != 0.0;
        for s in buf.iter_mut() {
            let mut wet_l = self.shaper[0].process(drive * s.left as f64, bias, tb) as f32;
            let mut wet_r = self.shaper[1].process(drive * s.right as f64, bias, tb) as f32;
            if dc_on {
                wet_l = self.dc[0].process(wet_l, self.dc_r);
                wet_r = self.dc[1].process(wet_r, self.dc_r);
            }
            s.left = (wet_l * self.makeup * self.mix + s.left * dry) * self.output;
            s.right = (wet_r * self.makeup * self.mix + s.right * dry) * self.output;
        }
    }

    pub fn reset(&mut self) {
        self.shaper = [AdaaTanh::default(); 2];
        for sh in &mut self.shaper {
            sh.rebias(self.bias, self.tanh_bias);
        }
        self.dc = [DcBlocker::default(); 2];
    }
}

impl Default for Saturator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::glue::filter::tests::sine;

    fn sat(drive_db: f32, asymmetry: f32) -> Saturator {
        let mut s = Saturator::new();
        s.configure(&SaturationParams { drive_db, asymmetry, mix: 1.0, output_db: 0.0 });
        s
    }

    #[test]
    fn bounded_for_huge_input() {
        for asym in [0.0, 0.4] {
            let mut s = sat(24.0, asym);
            let bound = s.wet_bound() + 1e-3;
            let mut buf: Vec<StereoSample> =
                sine(3000.0, 50.0, 4800).iter().map(|&v| StereoSample::new(v, -v)).collect();
            s.process(&mut buf);
            // The DC blocker can overshoot by the removed DC; allow its share.
            let slack = if asym != 0.0 { 1.0 } else { 0.0 };
            for x in &buf {
                assert!(x.left.is_finite() && x.left.abs() <= bound + slack, "{} > {bound}", x.left);
            }
        }
    }

    #[test]
    fn static_curve_monotonic_and_odd_when_symmetric() {
        let s = sat(12.0, 0.0);
        let mut prev = f32::NEG_INFINITY;
        for i in -200..=200 {
            let x = i as f32 * 0.01;
            let y = s.static_curve(x);
            assert!(y >= prev, "not monotonic at {x}");
            assert!((y + s.static_curve(-x)).abs() < 1e-6, "symmetric shaper must be odd");
            prev = y;
        }
        assert!((s.static_curve(1.0) - 1.0).abs() < 1e-5, "makeup maps full scale to full scale");
    }

    #[test]
    fn adaa_output_monotonic_for_ramp() {
        // ADAA output is the segment mean of a monotonic f, so a rising ramp stays rising.
        let mut s = sat(18.0, 0.0);
        let mut buf: Vec<StereoSample> =
            (0..2000).map(|i| { let v = -2.0 + i as f32 * 0.002; StereoSample::new(v, v) }).collect();
        s.process(&mut buf);
        for w in buf.windows(2).skip(1) {
            assert!(w[1].left >= w[0].left - 1e-6);
        }
    }

    /// Goertzel power at `freq` over the buffer.
    fn tone_power(x: &[f32], freq: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq / SAMPLE_RATE as f32;
        let c = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &v in x {
            let s0 = v + c * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - c * s1 * s2
    }

    #[test]
    fn adaa_reduces_aliasing() {
        // 7 kHz driven hard: the 7th harmonic (49 kHz) aliases to 1 kHz.
        let n = 48_000;
        let x = sine(7000.0, 0.9, n);
        let mut s = sat(18.0, 0.0);
        let mut buf: Vec<StereoSample> = x.iter().map(|&v| StereoSample::new(v, v)).collect();
        s.process(&mut buf);
        let adaa: Vec<f32> = buf.iter().map(|s| s.left).collect();
        let naive: Vec<f32> = x.iter().map(|&v| s.static_curve(v)).collect();
        let alias_adaa = tone_power(&adaa, 1000.0);
        let alias_naive = tone_power(&naive, 1000.0);
        assert!(
            alias_adaa < alias_naive * 0.5,
            "ADAA alias {alias_adaa} should be well below naive {alias_naive}"
        );
    }

    #[test]
    fn mix_zero_is_dry() {
        let mut s = Saturator::new();
        s.configure(&SaturationParams { drive_db: 20.0, asymmetry: 0.3, mix: 0.0, output_db: 0.0 });
        let mut buf = vec![StereoSample::new(0.7, -0.2); 32];
        s.process(&mut buf);
        assert!(buf.iter().all(|x| x.left == 0.7 && x.right == -0.2));
    }
}
