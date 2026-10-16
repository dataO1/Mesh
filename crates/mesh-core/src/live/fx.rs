//! Native mix effects for live-mode FX slots: compressor, EQ, transient
//! shaper, multiband and a drum room reverb. Each is an [`Effect`], so it can sit anywhere in a lane, break,
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

// ─── Settings by name ─────────────────────────────────────────────────

/// Key of a parameter in a style's `settings`: the parameter name in lower
/// case, every run of other characters as one `_` ("Bell 1 freq" ->
/// "bell_1_freq").
pub fn setting_key(name: &str) -> String {
    let mut key = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c.to_ascii_lowercase());
        } else if !key.is_empty() && !key.ends_with('_') {
            key.push('_');
        }
    }
    key.trim_end_matches('_').to_string()
}

/// Parameter index and normalized value for a setting in real units
/// (the parameter's own range, clamped). `None` for an unknown name.
pub fn setting_param(info: &EffectInfo, name: &str, value: f32) -> Option<(usize, f32)> {
    let key = setting_key(name);
    let (index, p) = info.params.iter().enumerate().find(|(_, p)| setting_key(&p.name) == key)?;
    let normalized = if p.max > p.min { (value - p.min) / (p.max - p.min) } else { 0.0 };
    Some((index, normalized.clamp(0.0, 1.0)))
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

/// Parameter ranges: low cut Hz (12 dB/oct), bell 1 Hz / dB / Q, bell 2 Hz / dB / Q,
/// low shelf Hz / dB (gentle slope, Q 0.5).
pub const EQ_RANGES: [(f32, f32); 9] = [
    (20.0, 500.0), (50.0, 1000.0), (-12.0, 12.0), (0.3, 4.0), (200.0, 12000.0), (-12.0, 12.0), (0.3, 4.0),
    (20.0, 1000.0),
    (-12.0, 12.0),
];

/// Low cut, two bells (constant-Q, Simper SVF) and a low shelf.
pub struct EqEffect {
    base: EffectBase,
    low_cut: SvfFilter,
    bells: [SvfFilter; 2],
    /// `k × (gain − 1)` per bell: the band-pass amount added to the input.
    bell_amount: [f32; 2],
    /// Low shelf: SVF lowpass at Q 0.5 and the gain added through it (`A − 1`).
    shelf: SvfFilter,
    shelf_amount: f32,
}

impl EqEffect {
    pub fn new() -> Self {
        let d = [20.0, 200.0, 0.0, 1.0, 3500.0, 0.0, 0.8, 100.0, 0.0];
        let names = [
            ("Low cut", "Hz"),
            ("Bell 1 freq", "Hz"),
            ("Bell 1 gain", "dB"),
            ("Bell 1 Q", ""),
            ("Bell 2 freq", "Hz"),
            ("Bell 2 gain", "dB"),
            ("Bell 2 Q", ""),
            ("Low shelf freq", "Hz"),
            ("Low shelf gain", "dB"),
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
            shelf: SvfFilter::new(),
            shelf_amount: 0.0,
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
        self.shelf.set_params(a(7), SHELF_Q);
        self.shelf_amount = db_to_lin(a(8)) - 1.0;
    }
}

/// Low shelf Q: two gentle first-order slopes.
const SHELF_Q: f32 = 0.5;

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
        let shelf = self.shelf_amount != 0.0;
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
            if shelf {
                let o = self.shelf.process(l, r);
                l += self.shelf_amount * o.low_l;
                r += self.shelf_amount * o.low_r;
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
        self.shelf.reset();
    }

    effect_boilerplate!();
}

// ─── Peak clipper (`id: clipper`) ─────────────────────────────────────

/// Parameter ranges: threshold dBFS, knee dB (how far under the threshold
/// the curve starts to bend).
pub const CLIPPER_RANGES: [(f32, f32); 2] = [(-24.0, 12.0), (0.0, 12.0)];

/// Oversampling factor and FIR taps (per phase: `CLIP_TAPS / CLIP_OS`).
const CLIP_OS: usize = 4;
const CLIP_TAPS: usize = 96;
const CLIP_PHASE_TAPS: usize = CLIP_TAPS / CLIP_OS;
/// FIR cutoff as a share of the base-rate Nyquist.
const CLIP_PASSBAND: f64 = 0.95;

/// Soft-knee peak clipper, 4x oversampled: the signal passes untouched below
/// the knee, and only peaks above it bend smoothly into the threshold. For
/// shaving the rare highest peaks before compressors and the limiter.
pub struct ClipperEffect {
    base: EffectBase,
    /// Knee start and width (linear): `x` up to `a` passes, above it the
    /// curve approaches `a + b` (the threshold).
    a: f32,
    b: f32,
    /// Windowed-sinc lowpass at the 4x rate (unity DC gain).
    fir: [f32; CLIP_TAPS],
    /// Per channel: recent base-rate inputs (newest first) and a ring of
    /// clipped 4x samples.
    hist: [[f32; CLIP_PHASE_TAPS]; 2],
    ring: [[f32; CLIP_TAPS]; 2],
    pos: usize,
}

impl ClipperEffect {
    pub fn new() -> Self {
        let d = [0.0, 3.0];
        let names = [("Threshold", "dB"), ("Knee", "dB")];
        let mut info = EffectInfo::new("Clipper", "Dynamics");
        for (i, ((name, unit), (lo, hi))) in names.iter().zip(CLIPPER_RANGES).enumerate() {
            info = info.with_param(ParamInfo::new(*name, norm(d[i], lo, hi)).with_range(lo, hi).with_unit(*unit));
        }
        let fc = CLIP_PASSBAND * 0.5 / CLIP_OS as f64;
        let mid = (CLIP_TAPS - 1) as f64 / 2.0;
        let mut fir = [0.0f32; CLIP_TAPS];
        let mut sum = 0.0;
        for (i, h) in fir.iter_mut().enumerate() {
            let t = i as f64 - mid;
            let sinc = if t == 0.0 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * t).sin() / (std::f64::consts::PI * t) };
            let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (CLIP_TAPS - 1) as f64).cos()
                + 0.08 * (4.0 * std::f64::consts::PI * i as f64 / (CLIP_TAPS - 1) as f64).cos();
            *h = (sinc * w) as f32;
            sum += sinc * w;
        }
        for h in &mut fir {
            *h /= sum as f32;
        }
        let mut fx = Self {
            base: EffectBase::new(info),
            a: 1.0,
            b: 0.0,
            fir,
            hist: [[0.0; CLIP_PHASE_TAPS]; 2],
            ring: [[0.0; CLIP_TAPS]; 2],
            pos: 0,
        };
        fx.configure();
        fx
    }

    fn configure(&mut self) {
        let threshold = db_to_lin(self.base.param_actual(0));
        self.a = threshold * db_to_lin(-self.base.param_actual(1));
        self.b = threshold - self.a;
    }

    #[inline(always)]
    fn curve(&self, x: f32) -> f32 {
        let m = x.abs();
        if m <= self.a {
            x
        } else if self.b <= 1e-9 {
            self.a.copysign(x)
        } else {
            (self.a + self.b * ((m - self.a) / self.b).tanh()).copysign(x)
        }
    }
}

impl Default for ClipperEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for ClipperEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }
        for s in buffer.as_mut_slice() {
            let mut out = [0.0f32; 2];
            for (c, x) in [s.left, s.right].into_iter().enumerate() {
                self.hist[c].copy_within(0..CLIP_PHASE_TAPS - 1, 1);
                self.hist[c][0] = x;
                let hist = self.hist[c];
                // Upsample (polyphase, gain 4), clip, push into the ring.
                for p in 0..CLIP_OS {
                    let mut u = 0.0;
                    for (k, &h) in hist.iter().enumerate() {
                        u += self.fir[p + CLIP_OS * k] * h;
                    }
                    let y = self.curve(u * CLIP_OS as f32);
                    self.ring[c][(self.pos + p) % CLIP_TAPS] = y;
                }
                // Decimate: lowpass at the newest 4x sample.
                let newest = self.pos + CLIP_OS - 1;
                let mut y = 0.0;
                for (j, &h) in self.fir.iter().enumerate() {
                    y += h * self.ring[c][(newest + CLIP_TAPS - j) % CLIP_TAPS];
                }
                out[c] = y;
            }
            self.pos = (self.pos + CLIP_OS) % CLIP_TAPS;
            s.left = out[0];
            s.right = out[1];
        }
    }

    fn reset(&mut self) {
        self.hist = [[0.0; CLIP_PHASE_TAPS]; 2];
        self.ring = [[0.0; CLIP_TAPS]; 2];
        self.pos = 0;
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

// ─── Multiband up/down compressor (`id: multiband`) ───────────────────

/// Bands of [`MultibandEffect`].
pub const MB_BANDS: usize = 6;
const MB_SPLITS: usize = MB_BANDS - 1;
/// Split ranges and defaults in Hz: sub | low bass | mids | high mids | highs | air.
const MB_SPLIT_RANGES: [(f32, f32, f32); MB_SPLITS] =
    [(30.0, 300.0, 90.0), (100.0, 800.0, 250.0), (500.0, 5000.0, 2000.0), (1500.0, 10000.0, 5000.0), (4000.0, 16000.0, 10000.0)];
/// Per-band parameters: name, range, default.
const MB_BAND_PARAMS: [(&str, &str, f32, f32, f32); 7] = [
    ("down threshold", "dB", -60.0, 0.0, -20.0),
    ("down ratio", ":1", 1.0, 20.0, 1.0),
    ("up threshold", "dB", -80.0, 0.0, -40.0),
    ("up ratio", ":1", 1.0, 10.0, 1.0),
    ("attack", "ms", 0.1, 200.0, 10.0),
    ("release", "ms", 5.0, 1000.0, 100.0),
    ("gain", "dB", -24.0, 24.0, 0.0),
];
/// Global parameters after the bands: most upward gain, level below which a
/// band is not lifted (keeps noise and tails down), output gain.
const MB_GLOBAL_PARAMS: [(&str, &str, f32, f32, f32); 3] =
    [("up max", "dB", 0.0, 24.0, 12.0), ("up gate", "dB", -100.0, -30.0, -70.0), ("output", "dB", -24.0, 24.0, 0.0)];
const MB_GLOBAL_BASE: usize = MB_SPLITS + MB_BANDS * MB_BAND_PARAMS.len();

/// One Linkwitz-Riley 4th-order split (two cascaded Butterworth stages).
pub(crate) struct Lr4 {
    first: SvfFilter,
    low: SvfFilter,
    high: SvfFilter,
}

impl Lr4 {
    pub(crate) fn new(hz: f32) -> Self {
        let mut s = Self { first: SvfFilter::new(), low: SvfFilter::new(), high: SvfFilter::new() };
        s.set(hz);
        s
    }

    pub(crate) fn set(&mut self, hz: f32) {
        self.first.set_frequency(hz);
        self.low.set_frequency(hz);
        self.high.set_frequency(hz);
    }

    /// `(low, high)`; their sum is an allpass of the input.
    #[inline]
    pub(crate) fn split(&mut self, l: f32, r: f32) -> ([f32; 2], [f32; 2]) {
        let a = self.first.process(l, r);
        let lo = self.low.process(a.low_l, a.low_r);
        let hi = self.high.process(a.high_l, a.high_r);
        ([lo.low_l, lo.low_r], [hi.high_l, hi.high_r])
    }

    pub(crate) fn reset(&mut self) {
        self.first.reset();
        self.low.reset();
        self.high.reset();
    }
}

#[derive(Clone, Copy, Default)]
struct BandDyn {
    down_threshold: f32,
    down_slope: f32,
    up_threshold: f32,
    up_slope: f32,
    attack: f32,
    release: f32,
    gain_db: f32,
    /// Band level envelope (linear peak, attack/release).
    env: f32,
    /// Smoothed gain change in dB.
    current_db: f32,
}

/// Six-band compressor with downward and upward compression per band
/// (OTT-style). LR4 crossovers with allpass compensation, so the bands sum
/// flat when nothing compresses.
pub struct MultibandEffect {
    base: EffectBase,
    splits: [Lr4; MB_SPLITS],
    /// Allpass compensation: band `i` passes the splits after its own.
    comp: Vec<Vec<Lr4>>,
    bands: [BandDyn; MB_BANDS],
    up_max: f32,
    up_gate: f32,
    output: f32,
    /// Gain smoothing after the envelope (against zipper noise).
    smooth: f32,
}

impl MultibandEffect {
    pub fn new() -> Self {
        let mut info = EffectInfo::new("Multiband", "Dynamics");
        for (i, (lo, hi, d)) in MB_SPLIT_RANGES.iter().enumerate() {
            info = info.with_param(ParamInfo::new(format!("Split {}", i + 1), norm(*d, *lo, *hi)).with_range(*lo, *hi).with_unit("Hz"));
        }
        for b in 0..MB_BANDS {
            for (name, unit, lo, hi, d) in MB_BAND_PARAMS {
                info = info.with_param(
                    ParamInfo::new(format!("Band {} {name}", b + 1), norm(d, lo, hi)).with_range(lo, hi).with_unit(unit),
                );
            }
        }
        for (name, unit, lo, hi, d) in MB_GLOBAL_PARAMS {
            info = info.with_param(ParamInfo::new(name, norm(d, lo, hi)).with_range(lo, hi).with_unit(unit));
        }
        let splits = std::array::from_fn(|i| Lr4::new(MB_SPLIT_RANGES[i].2));
        let comp = (0..MB_BANDS)
            .map(|b| ((b + 1)..MB_SPLITS).map(|j| Lr4::new(MB_SPLIT_RANGES[j].2)).collect())
            .collect();
        let mut fx = Self {
            base: EffectBase::new(info),
            splits,
            comp,
            bands: [BandDyn::default(); MB_BANDS],
            up_max: 0.0,
            up_gate: 0.0,
            output: 1.0,
            smooth: coeff(GAIN_SMOOTH_MS),
        };
        fx.configure();
        fx
    }

    fn configure(&mut self) {
        let a = |i| self.base.param_actual(i);
        let freqs: [f32; MB_SPLITS] = std::array::from_fn(|i| a(i));
        for (i, s) in self.splits.iter_mut().enumerate() {
            s.set(freqs[i]);
        }
        for (b, chain) in self.comp.iter_mut().enumerate() {
            for (k, s) in chain.iter_mut().enumerate() {
                s.set(freqs[b + 1 + k]);
            }
        }
        for b in 0..MB_BANDS {
            let p = |k: usize| self.base.param_actual(MB_SPLITS + b * MB_BAND_PARAMS.len() + k);
            let band = &mut self.bands[b];
            band.down_threshold = p(0);
            band.down_slope = 1.0 - 1.0 / p(1).max(1.0);
            band.up_threshold = p(2);
            band.up_slope = 1.0 - 1.0 / p(3).max(1.0);
            band.attack = coeff(p(4));
            band.release = coeff(p(5));
            band.gain_db = p(6);
        }
        self.up_max = a(MB_GLOBAL_BASE);
        self.up_gate = a(MB_GLOBAL_BASE + 1);
        self.output = db_to_lin(a(MB_GLOBAL_BASE + 2));
    }
}

impl Default for MultibandEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for MultibandEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }
        for s in buffer.as_mut_slice() {
            // Split: band i is the low side of split i, the last band the rest.
            let mut bands = [[0.0f32; 2]; MB_BANDS];
            let mut rest = [s.left, s.right];
            for (i, split) in self.splits.iter_mut().enumerate() {
                let (lo, hi) = split.split(rest[0], rest[1]);
                bands[i] = lo;
                rest = hi;
            }
            bands[MB_SPLITS] = rest;
            // Phase-align the early bands with the later splits.
            for (band, chain) in bands.iter_mut().zip(self.comp.iter_mut()) {
                for split in chain.iter_mut() {
                    let (lo, hi) = split.split(band[0], band[1]);
                    *band = [lo[0] + hi[0], lo[1] + hi[1]];
                }
            }
            let (mut l, mut r) = (0.0, 0.0);
            for (x, d) in bands.iter().zip(self.bands.iter_mut()) {
                d.env = follow(d.env, x[0].abs().max(x[1].abs()), d.attack, d.release);
                let level = 20.0 * d.env.max(FLOOR).log10();
                let down = (level - d.down_threshold).max(0.0) * d.down_slope;
                let up = if level > self.up_gate { ((d.up_threshold - level).max(0.0) * d.up_slope).min(self.up_max) } else { 0.0 };
                let target = up - down;
                d.current_db = self.smooth * d.current_db + (1.0 - self.smooth) * target;
                let g = db_to_lin(d.current_db + d.gain_db);
                l += x[0] * g;
                r += x[1] * g;
            }
            s.left = l * self.output;
            s.right = r * self.output;
        }
    }

    fn reset(&mut self) {
        for s in self.splits.iter_mut().chain(self.comp.iter_mut().flatten()) {
            s.reset();
        }
        for b in &mut self.bands {
            b.current_db = 0.0;
            b.env = 0.0;
        }
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
    fn clipper_passes_low_levels_and_shaves_peaks() {
        let n = 48_000;
        let run = |amp: f32, freq: f32| {
            let mut c = ClipperEffect::new();
            set(&mut c, 0, -6.0, CLIPPER_RANGES[0]);
            set(&mut c, 1, 3.0, CLIPPER_RANGES[1]);
            let mut b = sine(freq, amp, n);
            let dry = rms_db(&b, n / 2);
            c.process(&mut b);
            let peak = b.as_slice()[n / 2..].iter().map(|s| s.left.abs()).fold(0.0f32, f32::max);
            (rms_db(&b, n / 2) - dry, 20.0 * peak.log10())
        };
        // Below the knee (-9 dBFS): untouched, in level, up to 15 kHz.
        for f in [100.0, 1000.0, 15_000.0] {
            let (gain, _) = run(0.25, f);
            assert!(gain.abs() < 0.2, "{f} Hz: {gain} dB");
        }
        // A peak well over the threshold ends at it (a little overshoot
        // from the oversampling filter).
        let (_, peak) = run(1.0, 200.0);
        assert!(peak < -5.3 && peak > -7.0, "peak {peak} dBFS");
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
        let shelf = |eq: &mut EqEffect| {
            set(eq, 7, 100.0, EQ_RANGES[7]);
            set(eq, 8, 3.0, EQ_RANGES[8]);
        };
        assert!((gain(25.0, &shelf) - 3.0).abs() < 0.5, "shelf at 25 Hz: {}", gain(25.0, &shelf));
        assert!(gain(1000.0, &shelf).abs() < 0.2, "flat above: {}", gain(1000.0, &shelf));
        let mid = gain(100.0, &shelf);
        assert!(mid > 0.0 && mid < 3.0, "gentle slope: {mid}");
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

    fn gain_db(fx: &mut dyn Effect, freq: f32, amp: f32) -> f32 {
        let n = 48_000;
        let mut b = sine(freq, amp, n);
        let dry = rms_db(&b, n / 2);
        fx.process(&mut b);
        rms_db(&b, n / 2) - dry
    }

    fn setting(fx: &mut dyn Effect, name: &str, value: f32) {
        let (i, v) = setting_param(fx.info(), name, value).unwrap_or_else(|| panic!("no setting {name}"));
        fx.set_param(i, v);
    }

    #[test]
    fn multiband_is_flat_without_dynamics() {
        for freq in [40.0, 120.0, 400.0, 1500.0, 3500.0, 7000.0, 14000.0] {
            let g = gain_db(&mut MultibandEffect::new(), freq, 0.3);
            assert!(g.abs() < 0.3, "{freq} Hz: {g} dB");
        }
    }

    #[test]
    fn multiband_compresses_down_and_up_per_band() {
        let mut fx = MultibandEffect::new();
        // Mids: 8:1 above -30 dB. Air: lift quiet signal 3:1 below -30 dB.
        setting(&mut fx, "band_3_down_threshold", -30.0);
        setting(&mut fx, "band_3_down_ratio", 8.0);
        setting(&mut fx, "band_6_up_threshold", -30.0);
        setting(&mut fx, "band_6_up_ratio", 3.0);
        assert!(gain_db(&mut fx, 800.0, 0.5) < -10.0, "loud mids pushed down");
        assert!(gain_db(&mut fx, 60.0, 0.5).abs() < 0.5, "sub untouched");
        let quiet_air = gain_db(&mut fx, 14000.0, 0.01);
        assert!(quiet_air > 6.0 && quiet_air <= 12.5, "quiet air lifted, capped by up max: {quiet_air}");
        assert!(gain_db(&mut fx, 14000.0, 0.000_05).abs() < 0.5, "below the gate nothing is lifted");
    }

    #[test]
    fn settings_resolve_names_in_real_units() {
        assert_eq!(setting_key("Bell 1 freq"), "bell_1_freq");
        assert_eq!(setting_key("Band 3 down threshold"), "band_3_down_threshold");
        let eq = EqEffect::new();
        let (i, v) = setting_param(eq.info(), "Low cut", 120.0).unwrap();
        assert_eq!(i, 0);
        assert!((v - norm(120.0, EQ_RANGES[0].0, EQ_RANGES[0].1)).abs() < 1e-6);
        assert_eq!(setting_param(eq.info(), "low_cut", 1e6).unwrap().1, 1.0, "clamped");
        assert!(setting_param(eq.info(), "nope", 1.0).is_none());
    }
}

// ─── Room reverb (`id: room`) ─────────────────────────────────────────

/// Parameter ranges: decay s (RT60), pre-delay ms, low cut Hz, high cut Hz,
/// damping, size, mix, duck dB, duck release ms, key.
pub const ROOM_RANGES: [(f32, f32); 10] = [
    (0.1, 4.0),
    (0.0, 80.0),
    (50.0, 1000.0),
    (2000.0, 16000.0),
    (0.0, 1.0),
    (0.3, 1.5),
    (0.0, 1.0),
    (0.0, 24.0),
    (10.0, 500.0),
    (0.0, 1.0),
];
/// Index of the room's `Key` parameter: writing 1 ducks the tail now. The
/// drum lane writes it on every kick and snare it fires (sidechain).
pub const ROOM_KEY_PARAM: usize = 9;

/// Delay line lengths in ms at size 1 (mutually prime in samples, spread so
/// the echoes do not line up).
const ROOM_LINES_MS: [f32; 8] = [11.3, 13.7, 17.1, 19.9, 23.3, 27.1, 31.7, 37.9];
/// Longest pre-delay and line, for the buffers.
const ROOM_MAX_PREDELAY_MS: f32 = 80.0;
const ROOM_MAX_LINE_MS: f32 = 37.9 * 1.5;

/// Drum room reverb: pre-delay, low cut and high cut on the reverb path
/// only, then an 8-line feedback delay network (Householder mixing, a
/// damping low-pass in every line) whose feedback sets the decay time
/// directly in seconds. The dry signal passes untouched; `mix` adds the wet.
/// Buffers are sized for the longest settings at construction (RT-safe).
pub struct RoomEffect {
    base: EffectBase,
    predelay: Vec<f32>,
    pre_pos: usize,
    pre_len: usize,
    lines: [Vec<f32>; 8],
    pos: [usize; 8],
    len: [usize; 8],
    gain: [f32; 8],
    damp_state: [f32; 8],
    damp: f32,
    low_cut: SvfFilter,
    high_cut: SvfFilter,
    mix: f32,
    /// Duck depth (dB at the key) and envelope release coefficient.
    duck_db: f32,
    duck_release: f32,
    /// Duck envelope: 1 right at a key, decaying to 0.
    duck_env: f32,
}

impl RoomEffect {
    pub fn new() -> Self {
        // MUSICAL: DnB drum room (short decay, 20 ms pre-delay, the low end
        // and the fizz kept out of the tail). Tune by ear.
        let d = [0.8, 20.0, 300.0, 7000.0, 0.4, 1.0, 0.15, 12.0, 150.0, 0.0];
        let names = [
            ("Decay", "s"),
            ("Pre-delay", "ms"),
            ("Low cut", "Hz"),
            ("High cut", "Hz"),
            ("Damping", ""),
            ("Size", ""),
            ("Mix", ""),
            ("Duck", "dB"),
            ("Duck release", "ms"),
            ("Key", ""),
        ];
        let mut info = EffectInfo::new("Room", "Reverb");
        for (i, ((name, unit), (lo, hi))) in names.iter().zip(ROOM_RANGES).enumerate() {
            info = info.with_param(ParamInfo::new(*name, norm(d[i], lo, hi)).with_range(lo, hi).with_unit(*unit));
        }
        let sr = SAMPLE_RATE as f32;
        let line_cap = (ROOM_MAX_LINE_MS * 0.001 * sr) as usize + 2;
        let mut fx = Self {
            base: EffectBase::new(info),
            predelay: vec![0.0; (ROOM_MAX_PREDELAY_MS * 0.001 * sr) as usize + 2],
            pre_pos: 0,
            pre_len: 1,
            lines: std::array::from_fn(|_| vec![0.0; line_cap]),
            pos: [0; 8],
            len: [1; 8],
            gain: [0.0; 8],
            damp_state: [0.0; 8],
            damp: 0.0,
            low_cut: SvfFilter::new(),
            high_cut: SvfFilter::new(),
            mix: 0.0,
            duck_db: 0.0,
            duck_release: 0.0,
            duck_env: 0.0,
        };
        fx.configure();
        fx
    }

    fn configure(&mut self) {
        let a = |i| self.base.param_actual(i);
        let sr = SAMPLE_RATE as f32;
        let (decay, size) = (a(0).max(0.05), a(5));
        self.pre_len = ((a(1) * 0.001 * sr) as usize).clamp(1, self.predelay.len() - 1);
        for i in 0..8 {
            let n = ((ROOM_LINES_MS[i] * size * 0.001 * sr) as usize).clamp(2, self.lines[i].len() - 1);
            self.len[i] = n;
            // RT60: -60 dB after `decay` seconds, i.e. per trip through the line.
            self.gain[i] = 10f32.powf(-3.0 * n as f32 / (decay * sr));
        }
        self.low_cut.set_frequency(a(2));
        self.high_cut.set_frequency(a(3));
        self.damp = a(4) * 0.7;
        self.mix = a(6);
        self.duck_db = a(7);
        self.duck_release = coeff(a(8));
        // A key: duck from now on, and clear it for the next one.
        if a(ROOM_KEY_PARAM) >= 0.5 {
            self.duck_env = 1.0;
            self.base.set_param(ROOM_KEY_PARAM, 0.0);
        }
    }
}

impl Default for RoomEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for RoomEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() || self.mix <= 0.0 {
            return;
        }
        for s in buffer.as_mut_slice() {
            // Pre-delay, then the reverb-path filters.
            let input = 0.5 * (s.left + s.right);
            let read = (self.pre_pos + self.predelay.len() - self.pre_len) % self.predelay.len();
            let delayed = self.predelay[read];
            self.predelay[self.pre_pos] = input;
            self.pre_pos = (self.pre_pos + 1) % self.predelay.len();
            let hp = self.low_cut.process(delayed, delayed).high_l;
            let x = self.high_cut.process(hp, hp).low_l;

            // Line outputs, damped.
            let mut out = [0.0f32; 8];
            let mut sum = 0.0;
            for i in 0..8 {
                let cap = self.lines[i].len();
                let r = (self.pos[i] + cap - self.len[i]) % cap;
                let y = self.lines[i][r];
                self.damp_state[i] = y + self.damp * (self.damp_state[i] - y);
                out[i] = self.damp_state[i] * self.gain[i];
                sum += out[i];
            }
            // Householder feedback (I - 2/N): energy-preserving, dense mixing.
            let k = sum * (2.0 / 8.0);
            for i in 0..8 {
                let cap = self.lines[i].len();
                self.lines[i][self.pos[i]] = x + out[i] - k;
                self.pos[i] = (self.pos[i] + 1) % cap;
            }
            let wet_l = out[0] - out[2] + out[4] - out[6];
            let wet_r = out[1] - out[3] + out[5] - out[7];
            // Sidechain: the tail ducks under each kick and snare and comes back.
            let duck = if self.duck_env > 1e-4 { db_to_lin(-self.duck_db * self.duck_env) } else { 1.0 };
            self.duck_env *= self.duck_release;
            let g = self.mix * 0.5 * duck;
            s.left += g * wet_l;
            s.right += g * wet_r;
        }
    }

    fn reset(&mut self) {
        self.predelay.fill(0.0);
        for l in &mut self.lines {
            l.fill(0.0);
        }
        self.damp_state = [0.0; 8];
        self.low_cut.reset();
        self.high_cut.reset();
    }

    effect_boilerplate!();
}

#[cfg(test)]
mod room_tests {
    use super::*;
    use crate::types::StereoSample;

    fn impulse_energy(fx: &mut RoomEffect, from_s: f32, to_s: f32) -> f32 {
        let sr = SAMPLE_RATE as f32;
        let n = (to_s * sr) as usize;
        let mut buf = StereoBuffer::silence(n);
        buf.as_mut_slice()[0] = StereoSample { left: 1.0, right: 1.0 };
        fx.process(&mut buf);
        buf.as_slice()[(from_s * sr) as usize..].iter().map(|s| s.left * s.left + s.right * s.right).sum()
    }

    fn room(settings: &[(&str, f32)]) -> RoomEffect {
        let mut fx = RoomEffect::new();
        for (name, value) in settings {
            let (i, v) = setting_param(fx.info(), name, *value).unwrap();
            fx.set_param(i, v);
        }
        fx
    }

    #[test]
    fn longer_decay_rings_longer_and_mix_zero_is_dry() {
        let short = impulse_energy(&mut room(&[("decay", 0.3), ("mix", 1.0)]), 0.5, 1.5);
        let long = impulse_energy(&mut room(&[("decay", 2.0), ("mix", 1.0)]), 0.5, 1.5);
        assert!(long > 20.0 * short, "tail {short} vs {long}");
        let mut dry = room(&[("mix", 0.0)]);
        let mut buf = StereoBuffer::silence(64);
        buf.as_mut_slice()[3] = StereoSample { left: 0.7, right: -0.2 };
        dry.process(&mut buf);
        assert_eq!(buf.as_slice()[3], StereoSample { left: 0.7, right: -0.2 });
        assert!(buf.as_slice().iter().enumerate().all(|(i, s)| i == 3 || s.left == 0.0));
    }

    #[test]
    fn predelay_holds_the_tail_back_and_it_stays_finite() {
        let mut fx = room(&[("pre_delay", 40.0), ("mix", 1.0), ("decay", 4.0)]);
        let sr = SAMPLE_RATE as f32;
        let mut buf = StereoBuffer::silence((sr * 3.0) as usize);
        buf.as_mut_slice()[0] = StereoSample { left: 1.0, right: 1.0 };
        fx.process(&mut buf);
        let early: f32 = buf.as_slice()[1..(0.04 * sr) as usize].iter().map(|s| s.left.abs()).sum();
        assert!(early < 1e-6, "nothing before the pre-delay: {early}");
        assert!(buf.as_slice().iter().all(|s| s.left.is_finite() && s.left.abs() < 4.0));
    }

    #[test]
    fn a_key_ducks_the_tail_and_it_comes_back() {
        let sr = SAMPLE_RATE as f32;
        let tail = |key: bool| {
            let mut fx = room(&[("decay", 1.0), ("mix", 1.0), ("pre_delay", 0.0), ("duck", 18.0), ("duck_release", 100.0)]);
            let mut buf = StereoBuffer::silence((sr * 0.6) as usize);
            buf.as_mut_slice()[0] = StereoSample { left: 1.0, right: 1.0 };
            fx.process(&mut buf);
            if key {
                fx.set_param(ROOM_KEY_PARAM, 1.0);
            }
            let mut next = StereoBuffer::silence((sr * 0.6) as usize);
            fx.process(&mut next);
            let energy = |b: &[StereoSample]| b.iter().map(|s| s.left * s.left).sum::<f32>();
            (energy(&next.as_slice()[..(0.02 * sr) as usize]), energy(&next.as_slice()[(0.4 * sr) as usize..]))
        };
        let (plain_now, plain_late) = (tail(false).0, tail(false).1);
        let (keyed_now, keyed_late) = tail(true);
        assert!(keyed_now < plain_now * 0.1, "ducked right after the key: {keyed_now} vs {plain_now}");
        assert!(keyed_late > plain_late * 0.8, "back after the release: {keyed_late} vs {plain_late}");
    }
}
