//! Role high-pass and bass mono-below stages.
//!
//! Both reuse the shared native filters: [`SvfFilter`] (Simper SVF) for the
//! high-pass and [`LinkwitzRileyCrossover`] (LR24) for the mono split.

use crate::effect::native::{LinkwitzRileyCrossover, SvfFilter};
use crate::types::StereoSample;

use super::params::{MonoBelowParams, RoleHighPassParams, Slope};

/// Butterworth Qs of a 4th-order (24 dB/oct) filter built from two 2-pole stages.
const BUTTERWORTH4_Q: [f32; 2] = [0.541_196_1, 1.306_563];

/// Role high-pass: 12 dB/oct (one stage) or 24 dB/oct Butterworth (two stages).
/// −3 dB at `freq_hz` in both slopes.
pub struct RoleHighPass {
    stages: [SvfFilter; 2],
    active_stages: usize,
    enabled: bool,
}

impl RoleHighPass {
    pub fn new() -> Self {
        let mut hp = Self { stages: [SvfFilter::new(), SvfFilter::new()], active_stages: 0, enabled: false };
        hp.configure(&RoleHighPassParams::default());
        hp
    }

    /// Apply new settings. Filter state is kept so cutoff moves don't click.
    pub fn configure(&mut self, p: &RoleHighPassParams) {
        self.enabled = p.enabled;
        match p.slope {
            Slope::Db12 => {
                self.stages[0].set_frequency(p.freq_hz);
                self.active_stages = 1;
            }
            Slope::Db24 => {
                self.stages[0].set_params(p.freq_hz, BUTTERWORTH4_Q[0]);
                self.stages[1].set_params(p.freq_hz, BUTTERWORTH4_Q[1]);
                self.active_stages = 2;
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn process(&mut self, buf: &mut [StereoSample]) {
        if !self.enabled {
            return;
        }
        for s in buf.iter_mut() {
            let (mut l, mut r) = (s.left, s.right);
            for stage in &mut self.stages[..self.active_stages] {
                let o = stage.process(l, r);
                l = o.high_l;
                r = o.high_r;
            }
            s.left = l;
            s.right = r;
        }
    }

    pub fn reset(&mut self) {
        for s in &mut self.stages {
            s.reset();
        }
    }
}

impl Default for RoleHighPass {
    fn default() -> Self {
        Self::new()
    }
}

/// Bass mono-below: LR24 split, the low band is summed to mono, the high band
/// stays stereo. The two bands sum back flat in magnitude (LR allpass).
pub struct MonoBelow {
    crossover: LinkwitzRileyCrossover,
    enabled: bool,
}

impl MonoBelow {
    pub fn new() -> Self {
        let mut crossover = LinkwitzRileyCrossover::new();
        crossover.set_band_count(2);
        let mut m = Self { crossover, enabled: false };
        m.configure(&MonoBelowParams::default());
        m
    }

    pub fn configure(&mut self, p: &MonoBelowParams) {
        self.enabled = p.enabled;
        self.crossover.set_frequency(0, p.freq_hz);
    }

    pub fn process(&mut self, buf: &mut [StereoSample]) {
        if !self.enabled {
            return;
        }
        for s in buf.iter_mut() {
            let bands = self.crossover.process(*s);
            let mono = 0.5 * (bands[0].left + bands[0].right);
            s.left = mono + bands[1].left;
            s.right = mono + bands[1].right;
        }
    }

    pub fn reset(&mut self) {
        self.crossover.reset();
    }
}

impl Default for MonoBelow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;

    pub(crate) fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE as f32).sin())
            .collect()
    }

    /// RMS of the second half (after the filter settles).
    pub(crate) fn rms_tail(x: impl Iterator<Item = f32>, n: usize) -> f32 {
        let v: Vec<f32> = x.skip(n / 2).collect();
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn hp_gain_db(p: RoleHighPassParams, freq: f32) -> f32 {
        let mut hp = RoleHighPass::new();
        hp.configure(&p);
        let n = 48_000;
        let x = sine(freq, 0.5, n);
        let mut buf: Vec<StereoSample> = x.iter().map(|&v| StereoSample::new(v, v)).collect();
        hp.process(&mut buf);
        20.0 * (rms_tail(buf.iter().map(|s| s.left), n) / rms_tail(x.into_iter(), n)).log10()
    }

    #[test]
    fn high_pass_24db_response() {
        let p = RoleHighPassParams { enabled: true, freq_hz: 500.0, slope: Slope::Db24 };
        let at_fc = hp_gain_db(p, 500.0);
        assert!((at_fc + 3.0).abs() < 0.5, "−3 dB at cutoff, got {at_fc}");
        // Two octaves below: 4th-order Butterworth ≈ −48 dB.
        let below = hp_gain_db(p, 125.0);
        assert!(below < -44.0, "125 Hz should be ~−48 dB, got {below}");
        let above = hp_gain_db(p, 4000.0);
        assert!(above.abs() < 0.3, "passband flat, got {above}");
    }

    #[test]
    fn high_pass_12db_response() {
        let p = RoleHighPassParams { enabled: true, freq_hz: 300.0, slope: Slope::Db12 };
        let below = hp_gain_db(p, 75.0);
        assert!((below + 24.0).abs() < 1.5, "two octaves below ≈ −24 dB, got {below}");
        assert!(hp_gain_db(p, 3000.0).abs() < 0.3);
    }

    #[test]
    fn high_pass_disabled_is_bit_transparent() {
        let p = RoleHighPassParams { enabled: false, freq_hz: 500.0, slope: Slope::Db24 };
        let mut hp = RoleHighPass::new();
        hp.configure(&p);
        let mut buf = vec![StereoSample::new(0.3, -0.2); 64];
        hp.process(&mut buf);
        assert!(buf.iter().all(|s| s.left == 0.3 && s.right == -0.2));
    }

    /// Feed a pure side signal (L = −R) and return the side RMS ratio.
    fn side_ratio(freq: f32) -> f32 {
        let mut m = MonoBelow::new();
        let n = 48_000;
        let x = sine(freq, 0.5, n);
        let mut buf: Vec<StereoSample> = x.iter().map(|&v| StereoSample::new(v, -v)).collect();
        m.process(&mut buf);
        let side_out = rms_tail(buf.iter().map(|s| 0.5 * (s.left - s.right)), n);
        side_out / rms_tail(x.into_iter(), n)
    }

    #[test]
    fn mono_below_kills_side_in_lows_keeps_it_in_highs() {
        // 40 Hz is 1.6 octaves below 120 Hz: LR24 high band ≈ −38 dB.
        let low = side_ratio(40.0);
        assert!(low < 0.02, "L−R below crossover should vanish, ratio {low}");
        let high = side_ratio(2000.0);
        assert!((high - 1.0).abs() < 0.02, "L−R above crossover preserved, ratio {high}");
    }

    #[test]
    fn mono_below_keeps_mid_level() {
        // Mono content passes at unity magnitude across the crossover.
        for f in [40.0, 120.0, 1000.0] {
            let mut m = MonoBelow::new();
            let n = 48_000;
            let x = sine(f, 0.5, n);
            let mut buf: Vec<StereoSample> = x.iter().map(|&v| StereoSample::new(v, v)).collect();
            m.process(&mut buf);
            let ratio = rms_tail(buf.iter().map(|s| s.left), n) / rms_tail(x.into_iter(), n);
            assert!((ratio - 1.0).abs() < 0.02, "{f} Hz mono gain {ratio}");
        }
    }
}
