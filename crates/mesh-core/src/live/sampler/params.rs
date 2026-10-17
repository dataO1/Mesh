//! Block-rate lane parameters and the event-field value curves.

use super::interp::Interpolation;

/// Choke fade length (design §5.1).
pub const CHOKE_FADE_SECS: f32 = 0.010;
/// Fade applied over the last frames before a one-shot reaches its slot edge,
/// so slices cut mid-waveform don't click.
pub const EDGE_FADE_SECS: f32 = 0.0007;
/// Envelope level below which a decaying or releasing voice is freed (−80 dB).
pub(crate) const ENV_SILENCE: f32 = 1e-4;
/// Bipolar `cutoff` event offset range in octaves (±127 → ±this).
pub const CUTOFF_OFFSET_OCTAVES: f32 = 4.0;
/// Lowest / highest filter cutoff in Hz (upper bound also capped at 0.45 · fs).
pub const CUTOFF_MIN_HZ: f32 = 20.0;
pub const CUTOFF_MAX_HZ: f32 = 20_000.0;

/// Per-lane sampler parameters, set once per audio block (design §4.5).
///
/// `Copy` and allocation-free: the lane copies a fresh value in with
/// [`super::Sampler::set_params`] whenever a mapped control moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneSamplerParams {
    /// Static lane gain (LUFS trim).
    pub gain: f32,
    /// Base pan, −1 (left) .. 1 (right).
    pub pan: f32,
    /// Lowpass base cutoff in Hz. Event `cutoff` offsets it in octaves.
    pub cutoff_hz: f32,
    /// Filter resonance 0..1 (0 = Butterworth, 1 = near self-oscillation).
    pub resonance: f32,
    /// Amp envelope attack in seconds (linear).
    pub attack: f32,
    /// Amp envelope decay in seconds to −60 dB of the distance to sustain.
    pub decay: f32,
    /// Sustain level 0..1.
    pub sustain: f32,
    /// Release in seconds to −60 dB.
    pub release: f32,
    /// Velocity → gain exponent: gain = (vel / 127) ^ curve.
    pub velocity_curve: f32,
    /// Linear gain multiplier for `flags::ACCENT` events.
    pub accent_gain: f32,
    pub interpolation: Interpolation,
    /// Slope morph of the lowpass between these cutoffs in Hz: 6 dB/oct at
    /// the low one, 12 in the middle, 24 at the high one (`None` = 12 dB/oct).
    pub filter_morph: Option<(f32, f32)>,
    /// Seconds a note's cutoff glides from the previous note's (0 = jump).
    pub cutoff_glide: f32,
    /// What pitch bend, CC and the vibrato do (a sound pack's SFZ sets it).
    pub modulation: ModConfig,
    /// The Tone knob, 0..1: the value of `modulation.tone_cc`. Negative = off
    /// (the CC follows its events).
    pub tone: f32,
    /// The Expression knob: scales bends, vibrato and glide, 0 (none) .. 1
    /// (as written) .. 2 (exaggerated).
    pub expression: f32,
    /// Decay and sustain as the sound was designed (the lane's, or a pack's
    /// shape): the Character macro scales from these.
    pub designed_env: (f32, f32),
    /// Release as designed: the Character macro lengthens it.
    pub designed_release: f32,
    /// Resonance as designed (the shape macro adds to it).
    pub designed_res: f32,
    /// The Morph knob, 0..1: the value of `modulation.morph_cc`. Negative =
    /// off.
    pub morph: f32,
    /// Stab accent 0..1 (the Character macro's stabby half): a gain kick at
    /// each note start and a deeper mod-envelope pitch move.
    pub punch: f32,
}

/// Per-note modulation envelope (SF2 "Mod env", SFZ `fileg_*` / `pitcheg_*`):
/// delay, linear attack, hold, exponential decay to `sustain`, exponential
/// release, in seconds; its depth on pitch and cutoff in cents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModEnv {
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    /// 0..1.
    pub sustain: f32,
    pub release: f32,
    pub to_pitch: f32,
    pub to_cutoff: f32,
}

impl Default for ModEnv {
    fn default() -> Self {
        Self { delay: 0.0, attack: 0.0, hold: 0.0, decay: 0.0, sustain: 1.0, release: 0.0, to_pitch: 0.0, to_cutoff: 0.0 }
    }
}

impl ModEnv {
    pub fn is_on(&self) -> bool {
        self.to_pitch != 0.0 || self.to_cutoff != 0.0
    }
}

/// Per-note modulation LFO (SF2 "Mod LFO", SFZ `fillfo_*` / `amplfo_*`): a
/// triangle from the note start after `delay`; depth on pitch and cutoff in
/// cents, on volume in dB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModLfo {
    pub delay: f32,
    /// Hz.
    pub rate: f32,
    pub to_pitch: f32,
    pub to_cutoff: f32,
    pub to_volume_db: f32,
}

impl Default for ModLfo {
    fn default() -> Self {
        // SF2's default LFO frequency (0 absolute cents).
        Self { delay: 0.0, rate: 8.176, to_pitch: 0.0, to_cutoff: 0.0, to_volume_db: 0.0 }
    }
}

impl ModLfo {
    pub fn is_on(&self) -> bool {
        self.to_pitch != 0.0 || self.to_cutoff != 0.0 || self.to_volume_db != 0.0
    }
}

/// How a CC value (0..127) becomes a modulation amount, as in SF2
/// modulator sources: linear, concave (slow start, the SF2 volume curve),
/// convex, or a switch at the middle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CcCurve {
    #[default]
    Linear,
    Concave,
    Convex,
    Switch,
}

impl CcCurve {
    /// SF2 2.04 section 8.2.1 curve shapes on 0..1.
    pub fn shape(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        // -20/96 log10 of the remaining amplitude (FluidSynth's concave table).
        let concave = |x: f32| if x >= 1.0 { 1.0 } else { (-(200.0 / 960.0) * (1.0 - x).log10() * 2.0).min(1.0) };
        match self {
            CcCurve::Linear => x,
            CcCurve::Concave => concave(x),
            CcCurve::Convex => 1.0 - concave(1.0 - x),
            CcCurve::Switch => if x >= 0.5 { 1.0 } else { 0.0 },
        }
    }
}

/// One CC mapping: the amount at the curve's full value (CC 127, or CC 0
/// when `reversed`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CcMap {
    pub cc: u8,
    pub amount: f32,
    pub reversed: bool,
    pub curve: CcCurve,
}

impl CcMap {
    pub const NONE: Self = Self { cc: NO_CC, amount: 0.0, reversed: false, curve: CcCurve::Linear };

    /// A linear, forward mapping.
    pub fn new(cc: u8, amount: f32) -> Self {
        Self { cc, amount, ..Self::NONE }
    }

    #[inline]
    pub fn value(&self, cc: &[u8; 128]) -> f32 {
        if self.cc >= 128 {
            return 0.0;
        }
        let x = cc[self.cc as usize] as f32 / 127.0;
        self.amount * self.curve.shape(if self.reversed { 1.0 - x } else { x })
    }
}

/// What a zone's own CC modulator drives (SF2 instrument-zone modulators,
/// SFZ `xfin_*` / `xfout_*` crossfades).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoneTarget {
    /// Gain in dB (amount of the map).
    Volume,
    /// Cutoff in cents.
    Cutoff,
    /// Start position in frames (read at the note start).
    Offset,
    /// Equal-power crossfade on the CC: silent at `lo` .. full at `hi`
    /// (`out`: full at `lo` .. silent at `hi`).
    Fade { lo: u8, hi: u8, out: bool },
}

/// One per-zone modulator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneMod {
    pub src: CcMap,
    pub target: ZoneTarget,
}

/// Most modulators per zone.
pub const MAX_ZONE_MODS: usize = 4;

impl ZoneMod {
    pub const NONE: Self = Self { src: CcMap::NONE, target: ZoneTarget::Volume };

    /// Gain factor of a zone's modulators.
    pub fn gain(mods: &[ZoneMod; MAX_ZONE_MODS], cc: &[u8; 128]) -> f32 {
        let mut g = 1.0;
        for m in mods.iter().filter(|m| m.src.cc < 128) {
            match m.target {
                ZoneTarget::Volume => g *= 10f32.powf(m.src.value(cc) / 20.0),
                ZoneTarget::Fade { lo, hi, out } => {
                    let v = cc[m.src.cc as usize];
                    let t = if hi > lo { ((v as f32 - lo as f32) / (hi - lo) as f32).clamp(0.0, 1.0) } else if v >= hi { 1.0 } else { 0.0 };
                    let t = if out { 1.0 - t } else { t };
                    g *= (t * std::f32::consts::FRAC_PI_2).sin();
                }
                _ => {}
            }
        }
        g
    }

    /// Sum of the modulators driving `pick` (cutoff cents or offset frames).
    pub fn sum(mods: &[ZoneMod; MAX_ZONE_MODS], cc: &[u8; 128], pick: ZoneTarget) -> f32 {
        mods.iter().filter(|m| m.target == pick).map(|m| m.src.value(cc)).sum()
    }
}

/// Most CC mappings per target in [`ModConfig`].
pub const MAX_CC_MAPS: usize = 4;

/// Lane modulation from a sound pack: vibrato, and what CCs drive
/// (SFZ `*_onccN`). `Copy` and fixed-size for the RT thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModConfig {
    /// Vibrato depth in cents, rate in Hz, delay and fade-in in seconds
    /// from the note start.
    pub vib_depth: f32,
    pub vib_rate: f32,
    pub vib_delay: f32,
    pub vib_fade: f32,
    /// (CC, cents at CC 127) for the cutoff, (CC, dB) for the volume,
    /// (CC, cents of vibrato depth), (CC, start frames) for the start offset.
    pub cutoff_cc: [CcMap; MAX_CC_MAPS],
    pub volume_cc: [CcMap; MAX_CC_MAPS],
    pub vibrato_cc: [CcMap; MAX_CC_MAPS],
    pub offset_cc: [CcMap; MAX_CC_MAPS],
    /// The CC the Tone knob writes.
    pub tone_cc: u8,
    /// The CC the Morph knob writes.
    pub morph_cc: u8,
    /// Initial CC values (CC, value 0..127), at most 8.
    pub cc_init: [(u8, u8); 8],
    /// Per-note modulation envelope and LFO.
    pub env: ModEnv,
    pub lfo: ModLfo,
}

/// An unused CC map entry.
pub const NO_CC: u8 = u8::MAX;

impl Default for ModConfig {
    fn default() -> Self {
        Self {
            vib_depth: 0.0,
            vib_rate: 5.5,
            vib_delay: 0.0,
            vib_fade: 0.0,
            cutoff_cc: [CcMap::NONE; MAX_CC_MAPS],
            volume_cc: [CcMap::NONE; MAX_CC_MAPS],
            vibrato_cc: [CcMap::NONE; MAX_CC_MAPS],
            offset_cc: [CcMap::NONE; MAX_CC_MAPS],
            // MUSICAL: CC74 is the usual "brightness" controller.
            tone_cc: 74,
            // MUSICAL: CC20 is unassigned in General MIDI; packs map it.
            morph_cc: 20,
            cc_init: [(NO_CC, 0); 8],
            env: ModEnv::default(),
            lfo: ModLfo::default(),
        }
    }
}

impl ModConfig {
    /// Sum of `map` against the CC values.
    #[inline]
    pub fn sum(map: &[CcMap; MAX_CC_MAPS], cc: &[u8; 128]) -> f32 {
        map.iter().map(|m| m.value(cc)).sum()
    }
}

/// A sound pack's settings on top of the lane's sampler params: envelope,
/// filter and modulation from its SF2 / SFZ. `None` fields keep the lane's.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SoundShape {
    pub attack: Option<f32>,
    pub decay: Option<f32>,
    pub sustain: Option<f32>,
    pub release: Option<f32>,
    pub cutoff_hz: Option<f32>,
    pub resonance: Option<f32>,
    /// `Some(None)` = plain 12 dB/oct, `Some(Some(..))` = a slope (see
    /// [`LaneSamplerParams::filter_morph`]).
    pub filter_morph: Option<Option<(f32, f32)>>,
    pub modulation: ModConfig,
}

impl SoundShape {
    pub fn apply(&self, p: &mut LaneSamplerParams) {
        let set = |dst: &mut f32, v: Option<f32>| {
            if let Some(v) = v {
                *dst = v;
            }
        };
        set(&mut p.attack, self.attack);
        set(&mut p.decay, self.decay);
        set(&mut p.sustain, self.sustain);
        set(&mut p.release, self.release);
        set(&mut p.cutoff_hz, self.cutoff_hz);
        set(&mut p.resonance, self.resonance);
        if let Some(m) = self.filter_morph {
            p.filter_morph = m;
        }
        p.modulation = self.modulation;
        p.designed_env = (p.decay, p.sustain);
        p.designed_release = p.release;
        p.designed_res = p.resonance;
    }
}

impl Default for LaneSamplerParams {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            cutoff_hz: CUTOFF_MAX_HZ,
            resonance: 0.0,
            // MUSICAL: 0.5 ms keeps drum transients intact but declicks
            // mid-waveform `start` offsets.
            attack: 0.0005,
            // MUSICAL: only audible when sustain < 1 or an event sets `decay`.
            decay: 0.3,
            // MUSICAL: full sustain so one-shots and slices play their whole
            // length by default; per-hit shortening uses the event `decay`.
            sustain: 1.0,
            // MUSICAL: short release, tight for drums and plucked bass.
            release: 0.04,
            // MUSICAL: 1.5 puts a 35 % ghost velocity at about −14 dB (A5 tiers).
            velocity_curve: 1.5,
            // MUSICAL: accent = +3 dB.
            accent_gain: std::f32::consts::SQRT_2,
            interpolation: Interpolation::default(),
            filter_morph: None,
            cutoff_glide: 0.0,
            modulation: ModConfig::default(),
            tone: -1.0,
            expression: 1.0,
            designed_env: (0.3, 1.0),
            designed_release: 0.04,
            designed_res: 0.0,
            morph: -1.0,
            punch: 0.0,
        }
    }
}

/// Event `decay` byte (1..=255) to seconds, log scale 5 ms .. 10 s.
pub fn decay_secs(v: u8) -> f32 {
    log_map(v, 0.005, 10.0)
}

/// Event `attack` byte (1..=255) to seconds, log scale 1 ms .. 2 s.
pub fn attack_secs(v: u8) -> f32 {
    log_map(v, 0.001, 2.0)
}

/// Seconds to the nearest event `attack` byte (never 0, which means "lane default").
pub fn attack_byte(secs: f32) -> u8 {
    let t = (secs.clamp(0.001, 2.0) / 0.001).ln() / (2.0f32 / 0.001).ln();
    (1.0 + t * 254.0).round().clamp(1.0, 255.0) as u8
}

/// Event `glide` byte to seconds, log scale 1 ms .. 2 s. 0 = instant retarget.
pub fn glide_secs(v: u8) -> f32 {
    if v == 0 {
        0.0
    } else {
        log_map(v, 0.001, 2.0)
    }
}

fn log_map(v: u8, lo: f32, hi: f32) -> f32 {
    let t = (v.max(1) - 1) as f32 / 254.0;
    lo * (hi / lo).powf(t)
}

/// Exponential coefficient that falls to −60 dB in `secs`.
/// Rise time of the sampler pitch envelope.
pub(crate) const PITCH_ENV_ATTACK_SECS: f32 = 0.004;

pub(crate) fn exp_coef(secs: f32, sr: f32) -> f32 {
    let n = (secs * sr).max(1.0);
    (-6.907_755 / n).exp()
}
