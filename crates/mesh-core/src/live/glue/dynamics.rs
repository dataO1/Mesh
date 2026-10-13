//! Feed-forward bus compressor (drum bus and master).
//!
//! Log-domain design after Giannoulis, Massberg & Reiss (JAES 2012):
//! stereo-linked sample peak → dB → soft-knee static curve → gain reduction
//! smoothed by a branching attack/release one-pole → makeup.

use crate::types::{StereoSample, SAMPLE_RATE};

use super::{db_to_lin, lin_to_db};
use super::params::CompressorParams;

/// Static compressor curve: output level in dB for input level `x_db`.
pub fn static_curve_db(x_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let ratio = ratio.max(1.0);
    let over = x_db - threshold_db;
    if knee_db > 0.0 && 2.0 * over.abs() <= knee_db {
        let t = over + knee_db / 2.0;
        x_db + (1.0 / ratio - 1.0) * t * t / (2.0 * knee_db)
    } else if over > 0.0 {
        threshold_db + over / ratio
    } else {
        x_db
    }
}

fn time_coeff(ms: f32) -> f32 {
    let samples = ms.max(0.0) * 0.001 * SAMPLE_RATE as f32;
    if samples < 1.0 { 0.0 } else { (-1.0 / samples).exp() }
}

pub struct Compressor {
    p: CompressorParams,
    attack: f32,
    release: f32,
    makeup: f32,
    /// Smoothed gain reduction in dB (≥ 0).
    gr_db: f32,
}

impl Compressor {
    pub fn new() -> Self {
        let mut c = Self { p: CompressorParams::default(), attack: 0.0, release: 0.0, makeup: 1.0, gr_db: 0.0 };
        c.configure(&CompressorParams::default());
        c
    }

    pub fn configure(&mut self, p: &CompressorParams) {
        self.p = *p;
        self.attack = time_coeff(p.attack_ms);
        self.release = time_coeff(p.release_ms);
        self.makeup = db_to_lin(p.makeup_db);
    }

    /// Current gain reduction in dB (for metering).
    pub fn gain_reduction_db(&self) -> f32 {
        self.gr_db
    }

    pub fn process(&mut self, buf: &mut [StereoSample]) {
        let p = self.p;
        for s in buf.iter_mut() {
            let x_db = lin_to_db(s.left.abs().max(s.right.abs()));
            let target = x_db - static_curve_db(x_db, p.threshold_db, p.ratio, p.knee_db);
            let coeff = if target > self.gr_db { self.attack } else { self.release };
            self.gr_db = coeff * self.gr_db + (1.0 - coeff) * target;
            let g = db_to_lin(-self.gr_db) * self.makeup;
            s.left *= g;
            s.right *= g;
        }
    }

    pub fn reset(&mut self) {
        self.gr_db = 0.0;
    }
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_curve_hard_knee() {
        assert_eq!(static_curve_db(-30.0, -20.0, 4.0, 0.0), -30.0);
        assert_eq!(static_curve_db(-20.0, -20.0, 4.0, 0.0), -20.0);
        assert!((static_curve_db(-10.0, -20.0, 4.0, 0.0) - -17.5).abs() < 1e-5);
        assert!((static_curve_db(0.0, -20.0, 2.0, 0.0) - -10.0).abs() < 1e-5);
    }

    #[test]
    fn static_curve_soft_knee_is_continuous_and_monotonic() {
        // At threshold with a 6 dB knee: −20 + (1/4 − 1)·3²/12 = −20.5625.
        assert!((static_curve_db(-20.0, -20.0, 4.0, 6.0) - -20.5625).abs() < 1e-4);
        // Knee edges meet the linear segments.
        assert!((static_curve_db(-23.0, -20.0, 4.0, 6.0) - -23.0).abs() < 1e-4);
        assert!((static_curve_db(-17.0, -20.0, 4.0, 6.0) - -19.25).abs() < 1e-4);
        let mut prev = f32::NEG_INFINITY;
        for i in 0..400 {
            let y = static_curve_db(-40.0 + i as f32 * 0.1, -20.0, 4.0, 6.0);
            assert!(y >= prev);
            prev = y;
        }
    }

    #[test]
    fn steady_state_follows_static_curve() {
        let mut c = Compressor::new();
        let p = CompressorParams { threshold_db: -20.0, ratio: 4.0, attack_ms: 1.0, release_ms: 50.0, makeup_db: 3.0, knee_db: 0.0 };
        c.configure(&p);
        // Constant −6 dBFS level → expected output −20 + 14/4 + 3 = −13.5 dBFS.
        let mut buf = vec![StereoSample::new(0.5, -0.5); 9600];
        c.process(&mut buf);
        let out_db = lin_to_db(buf.last().unwrap().left.abs());
        assert!((out_db - -13.5).abs() < 0.05, "got {out_db}");
        assert!((c.gain_reduction_db() - 10.5).abs() < 0.05);
    }

    #[test]
    fn below_threshold_is_unity() {
        let mut c = Compressor::new();
        c.configure(&CompressorParams { threshold_db: -10.0, knee_db: 0.0, makeup_db: 0.0, ..CompressorParams::default() });
        let mut buf = vec![StereoSample::new(0.1, 0.1); 1000];
        c.process(&mut buf);
        assert!(buf.iter().all(|s| (s.left - 0.1).abs() < 1e-6));
    }
}
