//! Band duck: the bass lane gives way to the synth lanes, band by band.
//!
//! The key is the sum of the lead and extra lanes. Both key and bass are split
//! into [`BANDS`] bands; where the synths are loud in a band, the bass is cut
//! in that band only (a multiband sidechain, Trackspacer-like with few bands).
//! A synth playing mids ducks the bass's mids, one with a bright top ducks the
//! bass's top. The bass bands are phase-aligned (allpass compensation), so
//! with no key they sum back flat.
//!
//! Timing is adaptive per band. Attack and release scale with the band (low
//! bands slow, so one cycle of a low tone never modulates the gain; high bands
//! fast). The release is program-dependent: while a band's key stays over the
//! threshold (a held note) the release stretches, so the bass returns gently;
//! after a stab it returns quickly. The detector smooths the gain reduction in
//! the log domain (smooth branching, Giannoulis et al. 2012).

use crate::live::fx::Lr4;
use crate::types::{StereoSample, SAMPLE_RATE};

use super::{db_to_lin, lin_to_db};
use super::dynamics::static_curve_db;
use super::params::{BandDuckParams, BAND_DUCK_BANDS as BANDS};

const SPLITS: usize = BANDS - 1;
/// Soft knee of the key curve in dB.
const KNEE_DB: f32 = 6.0;

fn time_coeff(ms: f32) -> f32 {
    let samples = ms.max(0.0) * 0.001 * SAMPLE_RATE as f32;
    if samples < 1.0 { 0.0 } else { (-1.0 / samples).exp() }
}

pub struct BandDuck {
    p: BandDuckParams,
    /// Per band: attack, release (short notes) and release (held notes) coefficients.
    attack: [f32; BANDS],
    release_fast: [f32; BANDS],
    release_slow: [f32; BANDS],
    /// Rise and fall coefficient of the hold measure.
    adapt: f32,
    /// Key splits (levels only, no compensation needed).
    key_splits: [Lr4; SPLITS],
    /// Bass splits and their allpass compensation (band `i` passes the
    /// splits after its own).
    splits: [Lr4; SPLITS],
    comp: [[Lr4; SPLITS]; SPLITS],
    /// How long each band's key has stayed over the threshold, 0..1.
    hold: [f32; BANDS],
    /// Gain reduction per band in dB (≥ 0).
    gr_db: [f32; BANDS],
    /// Largest reduction per band since the last [`BandDuck::take_peak_gr_db`].
    peak_gr_db: [f32; BANDS],
}

impl BandDuck {
    pub fn new() -> Self {
        let p = BandDuckParams::default();
        let lr = || std::array::from_fn(|i| Lr4::new(p.splits_hz[i]));
        let mut d = Self {
            p,
            attack: [0.0; BANDS],
            release_fast: [0.0; BANDS],
            release_slow: [0.0; BANDS],
            adapt: 0.0,
            key_splits: lr(),
            splits: lr(),
            comp: std::array::from_fn(|_| lr()),
            hold: [0.0; BANDS],
            gr_db: [0.0; BANDS],
            peak_gr_db: [0.0; BANDS],
        };
        d.configure(&p);
        d
    }

    /// Apply new settings. Filter state is kept. Allocation-free.
    pub fn configure(&mut self, p: &BandDuckParams) {
        self.p = *p;
        for b in 0..BANDS {
            self.attack[b] = time_coeff(p.attack_ms[b]);
            self.release_fast[b] = time_coeff(p.release_ms[b]);
            self.release_slow[b] = time_coeff(p.release_ms[b] * (1.0 + p.adaptive_release.max(0.0)));
        }
        self.adapt = time_coeff(p.adapt_ms);
        for i in 0..SPLITS {
            self.key_splits[i].set(p.splits_hz[i]);
            self.splits[i].set(p.splits_hz[i]);
        }
        for (b, chain) in self.comp.iter_mut().enumerate() {
            for (k, s) in chain.iter_mut().enumerate() {
                // Band b is aligned with splits b+1..; unused stages stay idle.
                s.set(p.splits_hz[(b + 1 + k).min(SPLITS - 1)]);
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.p.enabled
    }

    /// Peak reduction per band since the last call (for metering).
    pub fn take_peak_gr_db(&mut self) -> [f32; BANDS] {
        std::mem::take(&mut self.peak_gr_db)
    }

    /// Duck `bass` by `key` (same length). RT-safe.
    pub fn process(&mut self, bass: &mut [StereoSample], key: &[StereoSample]) {
        let p = self.p;
        for (s, k) in bass.iter_mut().zip(key) {
            // Key level per band.
            let mut rest = [k.left, k.right];
            for b in 0..BANDS {
                let x = if b < SPLITS {
                    let (lo, hi) = self.key_splits[b].split(rest[0], rest[1]);
                    rest = hi;
                    lo
                } else {
                    rest
                };
                let x_db = lin_to_db(x[0].abs().max(x[1].abs()));
                let target = (x_db - static_curve_db(x_db, p.threshold_db, p.ratio, KNEE_DB)).min(p.max_db[b].max(0.0));
                // Hold rises while the key is over the threshold, falls otherwise.
                let over = if target > 0.0 { 1.0 } else { 0.0 };
                self.hold[b] = self.adapt * self.hold[b] + (1.0 - self.adapt) * over;
                let c = if target > self.gr_db[b] {
                    self.attack[b]
                } else {
                    let h = self.hold[b];
                    self.release_fast[b] + (self.release_slow[b] - self.release_fast[b]) * h
                };
                let gr = c * self.gr_db[b] + (1.0 - c) * target;
                self.gr_db[b] = gr;
                self.peak_gr_db[b] = self.peak_gr_db[b].max(gr);
            }

            // Bass bands, aligned, each cut by its reduction.
            let mut bands = [[0.0f32; 2]; BANDS];
            let mut rest = [s.left, s.right];
            for (i, split) in self.splits.iter_mut().enumerate() {
                let (lo, hi) = split.split(rest[0], rest[1]);
                bands[i] = lo;
                rest = hi;
            }
            bands[SPLITS] = rest;
            for (b, band) in bands.iter_mut().enumerate().take(SPLITS) {
                for split in self.comp[b].iter_mut().take(SPLITS - 1 - b) {
                    let (lo, hi) = split.split(band[0], band[1]);
                    *band = [lo[0] + hi[0], lo[1] + hi[1]];
                }
            }
            let (mut l, mut r) = (0.0, 0.0);
            for (x, &g) in bands.iter().zip(self.gr_db.iter()) {
                let lin = if g > 0.0 { db_to_lin(-g) } else { 1.0 };
                l += x[0] * lin;
                r += x[1] * lin;
            }
            s.left = l;
            s.right = r;
        }
    }

    pub fn reset(&mut self) {
        for s in self.key_splits.iter_mut().chain(self.splits.iter_mut()).chain(self.comp.iter_mut().flatten()) {
            s.reset();
        }
        self.hold = [0.0; BANDS];
        self.gr_db = [0.0; BANDS];
    }
}

impl Default for BandDuck {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, n: usize, amp: f32) -> Vec<StereoSample> {
        (0..n)
            .map(|i| {
                let v = amp * (std::f32::consts::TAU * hz * i as f32 / SAMPLE_RATE as f32).sin();
                StereoSample { left: v, right: v }
            })
            .collect()
    }

    fn rms(buf: &[StereoSample]) -> f32 {
        (buf.iter().map(|s| s.left * s.left).sum::<f32>() / buf.len() as f32).sqrt()
    }

    /// Bass made of a 50 Hz and a 1 kHz tone, ducked by `key`; returns the
    /// level ratio (after / before) of each tone over the second half.
    fn duck_tones(key: &[StereoSample]) -> (f32, f32) {
        let n = key.len();
        let tail = n / 2..n;
        let tone = |hz| {
            let mut bass = sine(hz, n, 0.3);
            let mut d = BandDuck::new();
            d.process(&mut bass, key);
            rms(&bass[tail.clone()]) / rms(&sine(hz, n, 0.3)[tail.clone()])
        };
        (tone(50.0), tone(1000.0))
    }

    #[test]
    fn a_mid_synth_ducks_the_bass_mids_only() {
        let n = SAMPLE_RATE as usize / 2;
        let (low, mid) = duck_tones(&sine(1000.0, n, 0.5));
        // Full key: the band reaches its deepest cut (4 dB, ratio 0.63).
        assert!((mid - db_to_lin(-BandDuckParams::default().max_db[2])).abs() < 0.05, "1 kHz bass ducked, ratio {mid}");
        assert!((low - 1.0).abs() < 0.05, "50 Hz bass untouched, ratio {low}");
    }

    /// Reduction of the 1 kHz band `ms` after a 1 kHz key of `key_ms` stops.
    fn gr_after(key_ms: usize, ms: usize) -> f32 {
        let at = |ms: usize| ms * SAMPLE_RATE as usize / 1000;
        let mut key = sine(1000.0, at(key_ms), 0.5);
        key.resize(at(key_ms + ms), StereoSample::default());
        let mut bass = vec![StereoSample::default(); key.len()];
        let mut d = BandDuck::new();
        d.process(&mut bass, &key);
        d.gr_db[2]
    }

    #[test]
    fn a_held_note_releases_slower_than_a_stab() {
        let (stab, held) = (gr_after(60, 150), gr_after(1500, 150));
        assert!(held > 1.5 * stab, "held {held} dB vs stab {stab} dB after 150 ms");
    }

    #[test]
    fn low_band_attacks_slower_than_high() {
        let p = BandDuckParams::default();
        assert!(p.attack_ms.windows(2).all(|w| w[0] >= w[1]) && p.release_ms.windows(2).all(|w| w[0] >= w[1]));
    }

    #[test]
    fn no_key_sums_flat() {
        let n = SAMPLE_RATE as usize / 2;
        let (low, mid) = duck_tones(&vec![StereoSample::default(); n]);
        assert!((low - 1.0).abs() < 0.05 && (mid - 1.0).abs() < 0.05, "ratios {low} {mid}");
        // A tone right at a split also passes at unity.
        let tail = n / 2..n;
        let mut bass = sine(400.0, n, 0.3);
        BandDuck::new().process(&mut bass, &vec![StereoSample::default(); n]);
        let ratio = rms(&bass[tail.clone()]) / rms(&sine(400.0, n, 0.3)[tail]);
        assert!((ratio - 1.0).abs() < 0.05, "400 Hz ratio {ratio}");
    }
}
