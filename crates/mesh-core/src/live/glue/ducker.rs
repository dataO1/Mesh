//! Drum-keyed full-band ducker.
//!
//! [`DuckEnvelope`] turns a key into a per-sample duck amount (0 = no cut,
//! 1 = full cut) with an attack → hold → release shape in milliseconds. The
//! key is trigger events (sample offsets), a sidechain bus, or both. A
//! sidechain crossing the threshold fires the same trigger as an event.
//!
//! The glue chain runs one envelope per key (main-kit kick, main-kit snare)
//! and release time (bass, other lanes), and turns the amounts into a gain:
//! `depth_db × amount`, summed over the keys (see [`super::params::DuckParams`]).

use crate::types::{StereoSample, SAMPLE_RATE};

use super::db_to_lin;

/// Rearm the sidechain detector once the key falls this far below threshold.
const REARM_RATIO: f32 = 0.5;
/// Release of the sidechain peak follower used for rearming.
const KEY_FOLLOWER_RELEASE_MS: f32 = 5.0;

fn ms_to_samples(ms: f32) -> u32 {
    (ms.max(0.0) * 0.001 * SAMPLE_RATE as f32).round() as u32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Attack,
    Hold,
    Release,
}

/// Attack / hold / release envelope driven by drum triggers.
pub struct DuckEnvelope {
    phase: Phase,
    level: f32,
    /// Level the current attack started from.
    from: f32,
    /// Samples elapsed in the current phase.
    pos: u32,
    attack_samples: u32,
    hold_samples: u32,
    release_samples: u32,
    // Sidechain detector
    threshold: f32,
    key_env: f32,
    key_release_coeff: f32,
    armed: bool,
}

impl DuckEnvelope {
    pub fn new() -> Self {
        let mut e = Self {
            phase: Phase::Idle,
            level: 0.0,
            from: 0.0,
            pos: 0,
            attack_samples: 1,
            hold_samples: 0,
            release_samples: 1,
            threshold: 1.0,
            key_env: 0.0,
            key_release_coeff: (-1.0 / (KEY_FOLLOWER_RELEASE_MS * 0.001 * SAMPLE_RATE as f32)).exp(),
            armed: true,
        };
        e.configure(0.5, 0.0, 50.0, -30.0);
        e
    }

    /// Envelope times in ms and the sidechain threshold in dBFS peak.
    pub fn configure(&mut self, attack_ms: f32, hold_ms: f32, release_ms: f32, key_threshold_db: f32) {
        self.attack_samples = ms_to_samples(attack_ms).max(1);
        self.hold_samples = ms_to_samples(hold_ms);
        self.release_samples = ms_to_samples(release_ms).max(1);
        // A shorter release mid-release must not jump past the new end.
        if self.phase == Phase::Release && self.pos >= self.release_samples {
            self.pos = self.release_samples - 1;
        }
        self.threshold = db_to_lin(key_threshold_db);
    }

    /// Release length in samples.
    pub fn release_samples(&self) -> u32 {
        self.release_samples
    }

    /// Start (or restart) the duck from the current level.
    #[inline]
    pub fn trigger(&mut self) {
        self.phase = Phase::Attack;
        self.from = self.level;
        self.pos = 0;
    }

    /// Render the duck amount for one block.
    ///
    /// - `sidechain`: the key bus for this block (same length as `out`), or `None`.
    /// - `triggers`: onsets as sample offsets into this block, ascending.
    ///   Offsets `>= out.len()` are ignored.
    pub fn render(&mut self, sidechain: Option<&[StereoSample]>, triggers: &[u32], out: &mut [f32]) {
        debug_assert!(triggers.windows(2).all(|w| w[0] <= w[1]), "triggers must be ascending");
        let mut next = 0;
        for (i, o) in out.iter_mut().enumerate() {
            while next < triggers.len() && triggers[next] as usize <= i {
                if triggers[next] as usize == i {
                    self.trigger();
                }
                next += 1;
            }
            if let Some(key) = sidechain {
                if let Some(k) = key.get(i) {
                    self.detect(k.left.abs().max(k.right.abs()));
                }
            }
            *o = self.tick();
        }
    }

    #[inline]
    fn detect(&mut self, peak: f32) {
        self.key_env = peak.max(self.key_env * self.key_release_coeff);
        if self.armed && peak >= self.threshold {
            self.armed = false;
            self.trigger();
        } else if !self.armed && self.key_env < self.threshold * REARM_RATIO {
            self.armed = true;
        }
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        // Levels come from step counters, not accumulation, so the envelope
        // lands exactly on 1.0 / 0.0 after the configured lengths.
        match self.phase {
            Phase::Idle => {}
            Phase::Attack => {
                self.pos += 1;
                if self.pos >= self.attack_samples {
                    self.level = 1.0;
                    self.pos = 0;
                    self.phase = if self.hold_samples > 0 { Phase::Hold } else { Phase::Release };
                } else {
                    let t = self.pos as f32 / self.attack_samples as f32;
                    self.level = self.from + (1.0 - self.from) * t;
                }
            }
            Phase::Hold => {
                self.pos += 1;
                if self.pos >= self.hold_samples {
                    self.pos = 0;
                    self.phase = Phase::Release;
                }
            }
            Phase::Release => {
                self.pos += 1;
                if self.pos >= self.release_samples {
                    self.level = 0.0;
                    self.phase = Phase::Idle;
                } else {
                    self.level = 1.0 - self.pos as f32 / self.release_samples as f32;
                }
            }
        }
        self.level
    }

    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.level = 0.0;
        self.pos = 0;
        self.key_env = 0.0;
        self.armed = true;
    }
}

impl Default for DuckEnvelope {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply `gain_db(i)` dB of cut to every sample of `buf` (full band).
#[inline]
pub fn duck_full_band(buf: &mut [StereoSample], kick: &[f32], snare: &[f32], kick_db: f32, snare_db: f32) {
    if kick_db <= 0.0 && snare_db <= 0.0 {
        return;
    }
    // gain = 10^(−dB/20) = exp(−dB·ln10/20)
    let k = -std::f32::consts::LN_10 / 20.0;
    for ((s, &a), &b) in buf.iter_mut().zip(kick).zip(snare) {
        let g = (k * (kick_db * a + snare_db * b)).exp();
        s.left *= g;
        s.right *= g;
    }
}

/// [`duck_full_band`] inside a band: the band (an SVF band-pass around its
/// centre) is cut by the duck gain, the rest passes, like a dynamic bell.
pub fn duck_band(
    buf: &mut [StereoSample],
    kick: &[f32],
    snare: &[f32],
    kick_db: f32,
    snare_db: f32,
    band: &mut crate::effect::native::SvfFilter,
    q: f32,
) {
    if kick_db <= 0.0 && snare_db <= 0.0 {
        return;
    }
    let k = -std::f32::consts::LN_10 / 20.0;
    for ((s, &a), &b) in buf.iter_mut().zip(kick).zip(snare) {
        let g = (k * (kick_db * a + snare_db * b)).exp();
        let o = band.process(s.left, s.right);
        // The SVF band output peaks at `q`; scale it to unity.
        let cut = (g - 1.0) / q;
        s.left += cut * o.band_l;
        s.right += cut * o.band_r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_duck_cuts_the_mids_and_keeps_the_top() {
        let sr = 48_000.0;
        let level = |freq: f32| {
            let mut band = crate::effect::native::SvfFilter::new();
            band.set_params(900.0, 0.6);
            let n = 9600;
            let mut buf: Vec<StereoSample> = (0..n)
                .map(|i| {
                    let v = (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin() * 0.5;
                    StereoSample::new(v, v)
                })
                .collect();
            let full = vec![1.0f32; n];
            duck_band(&mut buf, &full, &full, 6.0, 0.0, &mut band, 0.6);
            let rms = (buf[n / 2..].iter().map(|s| s.left * s.left).sum::<f32>() / (n / 2) as f32).sqrt();
            20.0 * (rms / (0.5 / std::f32::consts::SQRT_2)).log10()
        };
        assert!((level(900.0) + 6.0).abs() < 0.7, "900 Hz: {}", level(900.0));
        assert!(level(8000.0) > -1.0, "8 kHz kept: {}", level(8000.0));
    }

    fn render_trigger_at_zero(attack_ms: f32, release_ms: f32, n: usize) -> Vec<f32> {
        let mut env = DuckEnvelope::new();
        env.configure(attack_ms, 0.0, release_ms, -30.0);
        let mut out = vec![0.0; n];
        env.render(None, &[0], &mut out);
        out
    }

    #[test]
    fn envelope_recovers_after_its_release() {
        for release_ms in [50.0, 120.0] {
            let attack = 24; // 0.5 ms
            let release = (release_ms * 48.0) as usize;
            let end = attack + release;
            let out = render_trigger_at_zero(0.5, release_ms, end + 100);
            assert_eq!(out[attack - 1], 1.0, "full duck after attack");
            assert!(out[attack + release / 2] > 0.45 && out[attack + release / 2] < 0.55);
            assert!(out[end - 2] > 0.0, "still releasing just before the end");
            assert_eq!(out[end], 0.0, "fully recovered after {release_ms} ms");
        }
    }

    #[test]
    fn sidechain_key_fires_like_a_trigger() {
        let mut env = DuckEnvelope::new();
        env.configure(1.0, 10.0, 100.0, -30.0);
        let mut key = vec![StereoSample::default(); 4800];
        for s in &mut key[1000..1200] {
            *s = StereoSample::new(0.8, 0.8);
        }
        let mut out = vec![0.0; 4800];
        env.render(Some(&key), &[], &mut out);
        assert_eq!(out[999], 0.0, "no duck before the kick");
        assert_eq!(out[1000 + 47], 1.0, "attack completes 1 ms after the kick");
        // Below-threshold key never fires.
        let mut env2 = DuckEnvelope::new();
        env2.configure(1.0, 10.0, 100.0, -30.0);
        let quiet = vec![StereoSample::new(0.01, 0.01); 4800];
        env2.render(Some(&quiet), &[], &mut out);
        assert!(out.iter().all(|&a| a == 0.0));
    }

    #[test]
    fn full_band_cut_sums_the_keys() {
        let mut buf = vec![StereoSample::new(1.0, -1.0); 3];
        duck_full_band(&mut buf, &[1.0, 0.0, 1.0], &[0.0, 1.0, 1.0], 8.0, 4.0);
        let db = |v: f32| 20.0 * v.abs().log10();
        assert!((db(buf[0].left) + 8.0).abs() < 1e-3);
        assert!((db(buf[1].right) + 4.0).abs() < 1e-3);
        assert!((db(buf[2].left) + 12.0).abs() < 1e-3);
        let mut open = vec![StereoSample::new(0.5, 0.5); 3];
        duck_full_band(&mut open, &[0.0; 3], &[0.0; 3], 8.0, 4.0);
        assert!(open.iter().all(|s| s.left == 0.5));
    }
}
