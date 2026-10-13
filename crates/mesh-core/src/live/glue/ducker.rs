//! Kick-keyed low-band ducker.
//!
//! Split in two parts so the key is analysed once per block:
//!
//! - [`DuckEnvelope`] turns the key into a per-sample duck amount (0 = no cut,
//!   1 = full cut). The key is the drum lane's kick sub-voice bus, trigger
//!   events (sample offsets), or both. A sidechain crossing the threshold
//!   fires the same trigger as an event, so both key sources produce the same
//!   attack → hold → release shape and the release is exactly tempo-relative.
//! - [`LowBandDucker`] (one per lane) splits the lane at the crossover (LR24),
//!   applies `depth × amount` dB of cut to the low band only, and sums back.

use crate::effect::native::LinkwitzRileyCrossover;
use crate::types::{StereoSample, SAMPLE_RATE};

use super::db_to_lin;
use super::params::DuckParams;

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

/// Attack / hold / release envelope driven by kick triggers.
pub struct DuckEnvelope {
    phase: Phase,
    level: f32,
    /// Level the current attack started from.
    from: f32,
    /// Samples elapsed in the current phase.
    pos: u32,
    attack_samples: u32,
    hold_samples: u32,
    release_beats: f32,
    release_samples: u32,
    bpm: f64,
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
            release_beats: 0.5,
            release_samples: 1,
            bpm: 120.0,
            threshold: 1.0,
            key_env: 0.0,
            key_release_coeff: (-1.0 / (KEY_FOLLOWER_RELEASE_MS * 0.001 * SAMPLE_RATE as f32)).exp(),
            armed: true,
        };
        e.configure(&DuckParams::default());
        e
    }

    pub fn configure(&mut self, p: &DuckParams) {
        self.attack_samples = ms_to_samples(p.attack_ms).max(1);
        self.hold_samples = ms_to_samples(p.hold_ms);
        self.release_beats = p.release_beats.max(0.0);
        self.threshold = db_to_lin(p.key_threshold_db);
        self.update_release();
    }

    /// Set the tempo the release is measured against.
    pub fn set_bpm(&mut self, bpm: f64) {
        if bpm != self.bpm && bpm > 0.0 {
            self.bpm = bpm;
            self.update_release();
        }
    }

    /// Release length in samples at the current tempo.
    pub fn release_samples(&self) -> u32 {
        self.release_samples
    }

    fn update_release(&mut self) {
        let r = self.release_beats as f64 * 60.0 / self.bpm * SAMPLE_RATE as f64;
        self.release_samples = (r.round() as u32).max(1);
        // A retempo mid-release must not jump past the new end.
        if self.phase == Phase::Release && self.pos >= self.release_samples {
            self.pos = self.release_samples - 1;
        }
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
    /// - `sidechain`: the kick bus for this block (same length as `out`), or `None`.
    /// - `triggers`: kick onsets as sample offsets into this block, ascending.
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

/// Per-lane dynamic low-band cut.
pub struct LowBandDucker {
    crossover: LinkwitzRileyCrossover,
}

impl LowBandDucker {
    pub fn new() -> Self {
        let mut crossover = LinkwitzRileyCrossover::new();
        crossover.set_band_count(2);
        let mut d = Self { crossover };
        d.set_crossover_hz(DuckParams::default().crossover_hz);
        d
    }

    pub fn set_crossover_hz(&mut self, hz: f32) {
        self.crossover.set_frequency(0, hz);
    }

    /// Cut the low band by `depth_db × amount[i]`. `depth_db <= 0` bypasses
    /// the split entirely (no LR phase shift on the lane).
    pub fn process(&mut self, buf: &mut [StereoSample], amount: &[f32], depth_db: f32) {
        if depth_db <= 0.0 {
            return;
        }
        // gain = 10^(−depth·a/20) = exp(−depth·a·ln10/20)
        let k = -depth_db * std::f32::consts::LN_10 / 20.0;
        for (s, &a) in buf.iter_mut().zip(amount) {
            let bands = self.crossover.process(*s);
            let g = (k * a).exp();
            s.left = bands[0].left * g + bands[1].left;
            s.right = bands[0].right * g + bands[1].right;
        }
    }

    pub fn reset(&mut self) {
        self.crossover.reset();
    }
}

impl Default for LowBandDucker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::glue::filter::tests::{rms_tail, sine};

    fn params() -> DuckParams {
        DuckParams { attack_ms: 1.0, hold_ms: 10.0, release_beats: 0.5, ..DuckParams::default() }
    }

    fn render_trigger_at_zero(bpm: f64, n: usize) -> Vec<f32> {
        let mut env = DuckEnvelope::new();
        env.configure(&params());
        env.set_bpm(bpm);
        let mut out = vec![0.0; n];
        env.render(None, &[0], &mut out);
        out
    }

    #[test]
    fn envelope_recovers_after_tempo_relative_release() {
        for bpm in [120.0, 174.0] {
            let attack = 48;
            let hold = 480;
            let release = (0.5 * 60.0 / bpm * 48_000.0_f64).round() as usize;
            let end = attack + hold + release;
            let out = render_trigger_at_zero(bpm, end + 100);
            assert_eq!(out[attack - 1], 1.0, "full duck after attack");
            assert_eq!(out[attack + hold - 1], 1.0, "held");
            assert!(out[attack + hold + release / 2] > 0.45 && out[attack + hold + release / 2] < 0.55);
            assert!(out[end - 2] > 0.0, "still releasing just before the end");
            assert_eq!(out[end], 0.0, "fully recovered at {bpm} BPM");
        }
    }

    #[test]
    fn release_scales_with_tempo() {
        let slow = render_trigger_at_zero(85.0, 20_000);
        let fast = render_trigger_at_zero(170.0, 20_000);
        let recovered = |v: &[f32]| v.iter().skip(10).position(|&a| a == 0.0).unwrap() + 10;
        let ratio = (recovered(&slow) - 528) as f32 / (recovered(&fast) - 528) as f32;
        assert!((ratio - 2.0).abs() < 0.01, "half tempo → double release, ratio {ratio}");
    }

    #[test]
    fn sidechain_key_fires_like_a_trigger() {
        let mut env = DuckEnvelope::new();
        env.configure(&params());
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
        env2.configure(&params());
        let quiet = vec![StereoSample::new(0.01, 0.01); 4800];
        env2.render(Some(&quiet), &[], &mut out);
        assert!(out.iter().all(|&a| a == 0.0));
    }

    #[test]
    fn ducker_cuts_low_band_only() {
        let n = 48_000;
        let full = vec![1.0; n];
        // 40 Hz sits 1.6 octaves under the 120 Hz split, where the LR24 high band is ~−38 dB.
        for (freq, expect_db) in [(40.0, -12.0), (2000.0, 0.0)] {
            let mut d = LowBandDucker::new();
            let x = sine(freq, 0.5, n);
            let mut buf: Vec<StereoSample> = x.iter().map(|&v| StereoSample::new(v, v)).collect();
            d.process(&mut buf, &full, 12.0);
            let got = 20.0 * (rms_tail(buf.iter().map(|s| s.left), n) / rms_tail(x.into_iter(), n)).log10();
            assert!((got - expect_db).abs() < 0.5, "{freq} Hz: expected {expect_db} dB, got {got}");
        }
    }

    #[test]
    fn ducker_unity_when_amount_zero() {
        let n = 48_000;
        let mut d = LowBandDucker::new();
        let x = sine(60.0, 0.5, n);
        let mut buf: Vec<StereoSample> = x.iter().map(|&v| StereoSample::new(v, v)).collect();
        d.process(&mut buf, &vec![0.0; n], 12.0);
        let ratio = rms_tail(buf.iter().map(|s| s.left), n) / rms_tail(x.into_iter(), n);
        assert!((ratio - 1.0).abs() < 0.02);
    }
}
