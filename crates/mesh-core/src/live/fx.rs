//! Native mix effects for live-mode FX slots: compressor, EQ and transient
//! shaper. Each is an [`Effect`], so it can sit anywhere in a lane, break,
//! drum-bus or master FX chain next to CLAP plugins.
//!
//! Parameters are normalized like every [`Effect`]: a style writes 0..1 per
//! parameter index, mapped linearly onto the ranges below.

use crate::effect::native::SvfFilter;
use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::{StereoBuffer, SAMPLE_RATE};

use super::glue::{db_to_lin, Compressor, CompressorParams};

/// Normalized default for `actual` in `min..max`.
fn norm(actual: f32, min: f32, max: f32) -> f32 {
    ((actual - min) / (max - min)).clamp(0.0, 1.0)
}

/// One-pole smoothing coefficient for a time constant in ms.
fn coeff(ms: f32) -> f32 {
    let samples = ms.max(0.0) * 0.001 * SAMPLE_RATE as f32;
    if samples < 1.0 { 0.0 } else { (-1.0 / samples).exp() }
}

macro_rules! effect_boilerplate {
    () => {
        fn latency_samples(&self) -> u32 {
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
            self.configure();
        }

        fn set_bypass(&mut self, bypass: bool) {
            self.base.set_bypass(bypass);
        }

        fn is_bypassed(&self) -> bool {
            self.base.is_bypassed()
        }
    };
}

// ─── Airwindows ───────────────────────────────────────────────────────

/// CLAP id of Airwindows Consolidated, which hosts every Airwindows algorithm.
pub const AIRWINDOWS_ID: &str = "com.airwindows.consolidated";

/// Airwindows Consolidated state selecting algorithm `name` (its JUCE
/// state: `VC2!`, the XML length, the XML, a NUL).
pub fn airwindows_state(name: &str) -> Vec<u8> {
    let name = name.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<awconsolidated streamingVersion=\"8524\" currentProcessorName=\"{name}\"/>"
    );
    let mut state = b"VC2!".to_vec();
    state.extend_from_slice(&((xml.len() + 1) as u32).to_le_bytes());
    state.extend_from_slice(xml.as_bytes());
    state.push(0);
    state
}

// ─── Compressor (`id: compressor`) ────────────────────────────────────

/// Parameter ranges: threshold dB, ratio, attack ms, release ms, knee dB, makeup dB.
pub const COMP_RANGES: [(f32, f32); 6] = [(-60.0, 0.0), (1.0, 20.0), (0.1, 200.0), (5.0, 1000.0), (0.0, 24.0), (0.0, 24.0)];

/// Feed-forward compressor, the glue's [`Compressor`] as an effect.
pub struct CompressorEffect {
    base: EffectBase,
    comp: Compressor,
}

impl CompressorEffect {
    pub fn new() -> Self {
        let d = [-18.0, 4.0, 10.0, 100.0, 0.0, 0.0];
        let names = [("Threshold", "dB"), ("Ratio", ":1"), ("Attack", "ms"), ("Release", "ms"), ("Knee", "dB"), ("Makeup", "dB")];
        let mut info = EffectInfo::new("Compressor", "Dynamics");
        for (i, ((name, unit), (lo, hi))) in names.iter().zip(COMP_RANGES).enumerate() {
            info = info.with_param(ParamInfo::new(*name, norm(d[i], lo, hi)).with_range(lo, hi).with_unit(*unit));
        }
        let mut fx = Self { base: EffectBase::new(info), comp: Compressor::new() };
        fx.configure();
        fx
    }

    fn configure(&mut self) {
        let a = |i| self.base.param_actual(i);
        self.comp.configure(&CompressorParams {
            threshold_db: a(0),
            ratio: a(1),
            attack_ms: a(2),
            release_ms: a(3),
            knee_db: a(4),
            makeup_db: a(5),
        });
    }

    /// Peak gain reduction since the last call (metering).
    pub fn take_peak_gr_db(&mut self) -> f32 {
        self.comp.take_peak_gr_db()
    }
}

impl Default for CompressorEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for CompressorEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if !self.base.is_bypassed() {
            self.comp.process(buffer.as_mut_slice());
        }
    }

    fn reset(&mut self) {
        self.comp.reset();
    }

    effect_boilerplate!();
}

// ─── EQ (`id: eq`) ────────────────────────────────────────────────────

/// Parameter ranges: low cut Hz (12 dB/oct), bell 1 Hz / dB / Q, bell 2 Hz / dB / Q.
pub const EQ_RANGES: [(f32, f32); 7] =
    [(20.0, 500.0), (50.0, 1000.0), (-12.0, 12.0), (0.3, 4.0), (1000.0, 12000.0), (-12.0, 12.0), (0.3, 4.0)];

/// Low cut plus two bells (constant-Q, Simper SVF).
pub struct EqEffect {
    base: EffectBase,
    low_cut: SvfFilter,
    bells: [SvfFilter; 2],
    /// `k × (gain − 1)` per bell: the band-pass amount added to the input.
    bell_amount: [f32; 2],
}

impl EqEffect {
    pub fn new() -> Self {
        let d = [20.0, 200.0, 0.0, 1.0, 3500.0, 0.0, 0.8];
        let names = [
            ("Low cut", "Hz"),
            ("Bell 1 freq", "Hz"),
            ("Bell 1 gain", "dB"),
            ("Bell 1 Q", ""),
            ("Bell 2 freq", "Hz"),
            ("Bell 2 gain", "dB"),
            ("Bell 2 Q", ""),
        ];
        let mut info = EffectInfo::new("EQ", "Filter");
        for (i, ((name, unit), (lo, hi))) in names.iter().zip(EQ_RANGES).enumerate() {
            info = info.with_param(ParamInfo::new(*name, norm(d[i], lo, hi)).with_range(lo, hi).with_unit(*unit));
        }
        let mut fx = Self {
            base: EffectBase::new(info),
            low_cut: SvfFilter::new(),
            bells: [SvfFilter::new(), SvfFilter::new()],
            bell_amount: [0.0; 2],
        };
        fx.configure();
        fx
    }

    fn configure(&mut self) {
        let a = |i| self.base.param_actual(i);
        self.low_cut.set_frequency(a(0));
        for (b, base) in [1usize, 4].into_iter().enumerate() {
            let q = a(base + 2);
            self.bells[b].set_params(a(base), q);
            self.bell_amount[b] = (db_to_lin(a(base + 1)) - 1.0) / q;
        }
    }
}

impl Default for EqEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for EqEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }
        let cut = self.base.param_actual(0) > EQ_RANGES[0].0;
        for s in buffer.as_mut_slice() {
            let (mut l, mut r) = (s.left, s.right);
            if cut {
                let o = self.low_cut.process(l, r);
                (l, r) = (o.high_l, o.high_r);
            }
            for (bell, &amount) in self.bells.iter_mut().zip(&self.bell_amount) {
                let o = bell.process(l, r);
                l += amount * o.band_l;
                r += amount * o.band_r;
            }
            s.left = l;
            s.right = r;
        }
    }

    fn reset(&mut self) {
        self.low_cut.reset();
        for b in &mut self.bells {
            b.reset();
        }
    }

    effect_boilerplate!();
}

// ─── Transient shaper (`id: transient`) ───────────────────────────────

/// Parameter ranges: attack dB, sustain dB (both at full envelope difference).
pub const TRANSIENT_RANGES: [(f32, f32); 2] = [(-12.0, 12.0), (-12.0, 12.0)];

// Differential-envelope design (SPL Transient Designer): a fast and a slow
// envelope follower; their difference in dB marks attacks (fast above slow)
// and tails (slow-release above fast-release).
const FAST_ATTACK_MS: f32 = 0.5;
const SLOW_ATTACK_MS: f32 = 20.0;
const FAST_RELEASE_MS: f32 = 30.0;
const SLOW_RELEASE_MS: f32 = 300.0;
/// Envelope difference (dB) that applies the full attack / sustain gain.
const FULL_SCALE_DB: f32 = 12.0;
/// Gain smoothing.
const GAIN_SMOOTH_MS: f32 = 1.0;
const FLOOR: f32 = 1e-6;

pub struct TransientEffect {
    base: EffectBase,
    attack_db: f32,
    sustain_db: f32,
    fast: f32,
    slow: f32,
    tail_fast: f32,
    tail_slow: f32,
    gain_db: f32,
    c: [f32; 5],
}

impl TransientEffect {
    pub fn new() -> Self {
        let mut info = EffectInfo::new("Transient", "Dynamics");
        for (name, (lo, hi)) in ["Attack", "Sustain"].into_iter().zip(TRANSIENT_RANGES) {
            info = info.with_param(ParamInfo::new(name, 0.5).with_range(lo, hi).with_unit("dB"));
        }
        let mut fx = Self {
            base: EffectBase::new(info),
            attack_db: 0.0,
            sustain_db: 0.0,
            fast: 0.0,
            slow: 0.0,
            tail_fast: 0.0,
            tail_slow: 0.0,
            gain_db: 0.0,
            c: [
                coeff(FAST_ATTACK_MS),
                coeff(SLOW_ATTACK_MS),
                coeff(FAST_RELEASE_MS),
                coeff(SLOW_RELEASE_MS),
                coeff(GAIN_SMOOTH_MS),
            ],
        };
        fx.configure();
        fx
    }

    fn configure(&mut self) {
        self.attack_db = self.base.param_actual(0);
        self.sustain_db = self.base.param_actual(1);
    }
}

impl Default for TransientEffect {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn follow(env: f32, x: f32, attack: f32, release: f32) -> f32 {
    let c = if x > env { attack } else { release };
    c * env + (1.0 - c) * x
}

impl Effect for TransientEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }
        let [fa, sa, fr, sr, gs] = self.c;
        for s in buffer.as_mut_slice() {
            let x = s.left.abs().max(s.right.abs());
            // Attack: same release, fast vs slow attack.
            self.fast = follow(self.fast, x, fa, fr);
            self.slow = follow(self.slow, x, sa, fr);
            // Sustain: same attack, slow vs fast release.
            self.tail_fast = follow(self.tail_fast, x, fa, fr);
            self.tail_slow = follow(self.tail_slow, x, fa, sr);
            let attack = (20.0 * (self.fast.max(FLOOR) / self.slow.max(FLOOR)).log10()).clamp(0.0, FULL_SCALE_DB);
            let tail = (20.0 * (self.tail_slow.max(FLOOR) / self.tail_fast.max(FLOOR)).log10()).clamp(0.0, FULL_SCALE_DB);
            let target = (self.attack_db * attack + self.sustain_db * tail) / FULL_SCALE_DB;
            self.gain_db = gs * self.gain_db + (1.0 - gs) * target;
            let g = db_to_lin(self.gain_db);
            s.left *= g;
            s.right *= g;
        }
    }

    fn reset(&mut self) {
        self.fast = 0.0;
        self.slow = 0.0;
        self.tail_fast = 0.0;
        self.tail_slow = 0.0;
        self.gain_db = 0.0;
    }

    effect_boilerplate!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StereoSample;

    fn sine(freq: f32, amp: f32, n: usize) -> StereoBuffer {
        StereoBuffer::from_vec(
            (0..n)
                .map(|i| {
                    let v = amp * (std::f32::consts::TAU * freq * i as f32 / SAMPLE_RATE as f32).sin();
                    StereoSample::new(v, v)
                })
                .collect(),
        )
    }

    fn rms_db(b: &StereoBuffer, from: usize) -> f32 {
        let x = &b.as_slice()[from..];
        20.0 * (x.iter().map(|s| s.left * s.left).sum::<f32>() / x.len() as f32).sqrt().log10()
    }

    fn set(fx: &mut dyn Effect, i: usize, actual: f32, range: (f32, f32)) {
        fx.set_param(i, norm(actual, range.0, range.1));
    }

    #[test]
    fn eq_low_cut_and_bells() {
        let n = 48_000;
        let gain = |freq: f32, setup: &dyn Fn(&mut EqEffect)| {
            let mut eq = EqEffect::new();
            setup(&mut eq);
            let mut b = sine(freq, 0.5, n);
            let dry = rms_db(&b, n / 2);
            eq.process(&mut b);
            rms_db(&b, n / 2) - dry
        };
        let flat = |_: &mut EqEffect| {};
        assert!(gain(1000.0, &flat).abs() < 0.1, "default is flat");
        let cut = |eq: &mut EqEffect| set(eq, 0, 120.0, EQ_RANGES[0]);
        assert!(gain(30.0, &cut) < -20.0, "12 dB/oct below 120 Hz: {}", gain(30.0, &cut));
        assert!(gain(2000.0, &cut).abs() < 0.2);
        let bells = |eq: &mut EqEffect| {
            set(eq, 2, -4.0, EQ_RANGES[2]);
            set(eq, 5, 3.0, EQ_RANGES[5]);
        };
        assert!((gain(200.0, &bells) + 4.0).abs() < 0.3, "cut at 200 Hz: {}", gain(200.0, &bells));
        assert!((gain(3500.0, &bells) - 3.0).abs() < 0.3, "boost at 3.5 kHz: {}", gain(3500.0, &bells));
    }

    #[test]
    fn compressor_effect_reduces_loud_input() {
        let mut c = CompressorEffect::new();
        let mut b = sine(200.0, 0.9, 48_000);
        let dry = rms_db(&b, 24_000);
        c.process(&mut b);
        assert!(rms_db(&b, 24_000) < dry - 6.0);
        assert!(c.take_peak_gr_db() > 6.0);
    }

    /// Bursts: 5 ms of tone every 100 ms, then silence.
    fn hits(n: usize) -> StereoBuffer {
        let mut b = sine(300.0, 0.5, n);
        for (i, s) in b.as_mut_slice().iter_mut().enumerate() {
            let t = i % 4800;
            let env = if t < 240 { 1.0 } else { (-((t - 240) as f32) / 1200.0).exp() };
            *s *= env;
        }
        b
    }

    #[test]
    fn transient_shaper_lifts_attacks_and_cuts_tails() {
        let n = 48_000;
        let peak_and_tail = |attack: f32, sustain: f32| {
            let mut t = TransientEffect::new();
            set(&mut t, 0, attack, TRANSIENT_RANGES[0]);
            set(&mut t, 1, sustain, TRANSIENT_RANGES[1]);
            let mut b = hits(n);
            t.process(&mut b);
            let x = b.as_slice();
            let hit = 24_000;
            let peak = x[hit..hit + 240].iter().fold(0.0f32, |m, s| m.max(s.left.abs()));
            let tail = x[hit + 2400..hit + 4000].iter().fold(0.0f32, |m, s| m.max(s.left.abs()));
            (peak, tail)
        };
        let (p0, t0) = peak_and_tail(0.0, 0.0);
        let (p1, _) = peak_and_tail(12.0, 0.0);
        let (_, t1) = peak_and_tail(0.0, -12.0);
        assert!(p1 > p0 * 1.4, "attack lifted: {p0} -> {p1}");
        assert!(t1 < t0 * 0.7, "tail cut: {t0} -> {t1}");
    }
}
