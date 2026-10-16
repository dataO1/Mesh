//! Integrated loudness (ITU-R BS.1770-4 / EBU R128) for style calibration.
//!
//! Offline only (allocates); the style tool uses it to render each instrument
//! option and write a static trim (design §5.5). mesh-cue measures tracks with
//! Essentia's `LoudnessEBUR128` in a subprocess, which mesh-core can't link,
//! so this is a native implementation of the same standard.
//!
//! **Channel convention**: BS.1770 sums the K-weighted power of L and R
//! (weights 1.0), the same as Essentia. A 1 kHz sine at −20 dBFS on one
//! channel reads −23 LUFS; the same sine on both channels (dual mono, which
//! is how mesh-cue feeds mono audio to Essentia) reads −20 LUFS.

use crate::types::StereoSample;

/// Absolute gate (LUFS).
const ABSOLUTE_GATE: f64 = -70.0;
/// Relative gate below the ungated mean (LU).
const RELATIVE_GATE: f64 = -10.0;
/// Gating block 400 ms with 75 % overlap → 100 ms hop.
const HOPS_PER_BLOCK: usize = 4;

/// Direct-form-I biquad in f64.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    #[inline]
    fn process(&mut self, x0: f64) -> f64 {
        let y0 = self.b[0] * x0 + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x0, self.x[0]];
        self.y = [y0, self.y[0]];
        y0
    }
}

/// K-weighting (pre-filter shelf + RLB high-pass), derived for any sample
/// rate with the bilinear-transform formulas used by libebur128. At 48 kHz
/// these reproduce the BS.1770 table coefficients.
fn k_weighting(sample_rate: f64) -> [Biquad; 2] {
    let pi = std::f64::consts::PI;

    // Stage 1: high shelf, +4 dB above ~1.5 kHz.
    let f0 = 1681.974_450_955_533;
    let g = 3.999_843_853_973_347;
    let q = 0.707_175_236_955_419_6;
    let k = (pi * f0 / sample_rate).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        x: [0.0; 2],
        y: [0.0; 2],
    };

    // Stage 2: RLB high-pass at ~38 Hz.
    let f0 = 38.135_470_876_024_44;
    let q = 0.500_327_037_323_877_3;
    let k = (pi * f0 / sample_rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let hp = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        x: [0.0; 2],
        y: [0.0; 2],
    };
    [shelf, hp]
}

#[inline]
fn power_to_lufs(p: f64) -> f64 {
    -0.691 + 10.0 * p.log10()
}

/// Integrated loudness in LUFS of a stereo signal.
///
/// Returns `f64::NEG_INFINITY` for silence or input shorter than one 400 ms
/// gating block.
pub fn measure_lufs(stereo: &[StereoSample], sample_rate: f64) -> f64 {
    let hop = (sample_rate * 0.1).round() as usize;
    if hop == 0 || stereo.len() < hop * HOPS_PER_BLOCK {
        return f64::NEG_INFINITY;
    }

    // Mean K-weighted power (L + R) of each 100 ms hop.
    let [mut shelf_l, mut hp_l] = k_weighting(sample_rate);
    let [mut shelf_r, mut hp_r] = k_weighting(sample_rate);
    let mut hop_power = Vec::with_capacity(stereo.len() / hop);
    let mut acc = 0.0;
    for (i, s) in stereo.iter().enumerate() {
        let l = hp_l.process(shelf_l.process(s.left as f64));
        let r = hp_r.process(shelf_r.process(s.right as f64));
        acc += l * l + r * r;
        if (i + 1) % hop == 0 {
            hop_power.push(acc / hop as f64);
            acc = 0.0;
        }
    }

    // 400 ms blocks = 4 consecutive hops.
    let blocks: Vec<f64> = hop_power
        .windows(HOPS_PER_BLOCK)
        .map(|w| w.iter().sum::<f64>() / HOPS_PER_BLOCK as f64)
        .collect();

    let gated_mean = |threshold: f64| -> Option<f64> {
        let (sum, n) = blocks
            .iter()
            .filter(|&&p| power_to_lufs(p) > threshold)
            .fold((0.0, 0usize), |(s, n), &p| (s + p, n + 1));
        (n > 0).then(|| sum / n as f64)
    };

    let Some(abs_mean) = gated_mean(ABSOLUTE_GATE) else {
        return f64::NEG_INFINITY;
    };
    let relative = power_to_lufs(abs_mean) + RELATIVE_GATE;
    match gated_mean(relative.max(ABSOLUTE_GATE)) {
        Some(p) => power_to_lufs(p),
        None => f64::NEG_INFINITY,
    }
}

/// Loudness in LUFS of a hit shorter than one 400 ms gating block: the
/// K-weighted mean power over the hit's own length (no gating). Padding a
/// short hit with silence instead reads it as quiet and over-boosts it.
/// `f64::NEG_INFINITY` for silence.
pub fn measure_hit_lufs(stereo: &[StereoSample], sample_rate: f64) -> f64 {
    if stereo.is_empty() {
        return f64::NEG_INFINITY;
    }
    let [mut shelf_l, mut hp_l] = k_weighting(sample_rate);
    let [mut shelf_r, mut hp_r] = k_weighting(sample_rate);
    let mut acc = 0.0;
    for s in stereo {
        let l = hp_l.process(shelf_l.process(s.left as f64));
        let r = hp_r.process(shelf_r.process(s.right as f64));
        acc += l * l + r * r;
    }
    let p = acc / stereo.len() as f64;
    if p > 0.0 { power_to_lufs(p) } else { f64::NEG_INFINITY }
}

/// Static trim in dB that brings `measured` LUFS to `target` LUFS.
/// Unmeasurable input (silence) gets no trim.
pub fn trim_db(measured: f64, target: f64) -> f64 {
    if measured.is_finite() { target - measured } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(sr: f64, secs: f64, dbfs_l: Option<f64>, dbfs_r: Option<f64>) -> Vec<StereoSample> {
        let n = (sr * secs) as usize;
        let amp = |d: Option<f64>| d.map(|d| 10f64.powf(d / 20.0)).unwrap_or(0.0);
        let (al, ar) = (amp(dbfs_l), amp(dbfs_r));
        (0..n)
            .map(|i| {
                let v = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / sr).sin();
                StereoSample::new((al * v) as f32, (ar * v) as f32)
            })
            .collect()
    }

    #[test]
    fn ebu_tech3341_case1_and_2_stereo_sine() {
        // EBU Tech 3341 test 1/2: stereo 1 kHz sine at −23 / −33 dBFS → −23 / −33 LUFS.
        for sr in [48_000.0, 44_100.0] {
            for level in [-23.0, -33.0] {
                let x = tone(sr, 20.0, Some(level), Some(level));
                let got = measure_lufs(&x, sr);
                assert!((got - level).abs() < 0.1, "{sr} Hz, {level} dBFS → {got}");
            }
        }
    }

    #[test]
    fn single_channel_reference_tone() {
        // BS.1770: 1 kHz at −20 dBFS on one channel → −23 LUFS (+3 dB when dual mono).
        let mono = tone(48_000.0, 10.0, Some(-20.0), None);
        assert!((measure_lufs(&mono, 48_000.0) - -23.0).abs() < 0.1);
        let dual = tone(48_000.0, 10.0, Some(-20.0), Some(-20.0));
        assert!((measure_lufs(&dual, 48_000.0) - -20.0).abs() < 0.1);
    }

    #[test]
    fn ebu_tech3341_case3_relative_gate() {
        // 10 s at −36, 60 s at −23, 10 s at −36 dBFS → −23 LUFS (quiet parts gated out).
        let sr = 48_000.0;
        let mut x = tone(sr, 10.0, Some(-36.0), Some(-36.0));
        x.extend(tone(sr, 60.0, Some(-23.0), Some(-23.0)));
        x.extend(tone(sr, 10.0, Some(-36.0), Some(-36.0)));
        let got = measure_lufs(&x, sr);
        assert!((got - -23.0).abs() < 0.1, "got {got}");
    }

    #[test]
    fn ebu_tech3341_case4_absolute_gate() {
        // −72 / −36 / −23 / −36 / −72 dBFS (10/10/60/10/10 s) → −23 LUFS.
        let sr = 48_000.0;
        let mut x = Vec::new();
        for (secs, db) in [(10.0, -72.0), (10.0, -36.0), (60.0, -23.0), (10.0, -36.0), (10.0, -72.0)] {
            x.extend(tone(sr, secs, Some(db), Some(db)));
        }
        let got = measure_lufs(&x, sr);
        assert!((got - -23.0).abs() < 0.1, "got {got}");
    }

    #[test]
    fn silence_and_short_input() {
        assert_eq!(measure_lufs(&vec![StereoSample::default(); 48_000], 48_000.0), f64::NEG_INFINITY);
        assert_eq!(measure_lufs(&tone(48_000.0, 0.3, Some(-10.0), None), 48_000.0), f64::NEG_INFINITY);
    }

    #[test]
    fn trim_reaches_target() {
        assert_eq!(trim_db(-20.0, -14.0), 6.0);
        assert_eq!(trim_db(-9.0, -14.0), -5.0);
        assert_eq!(trim_db(f64::NEG_INFINITY, -14.0), 0.0);
    }
}
