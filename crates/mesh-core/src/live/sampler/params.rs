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
        }
    }
}

/// Event `decay` byte (1..=255) to seconds, log scale 5 ms .. 10 s.
pub fn decay_secs(v: u8) -> f32 {
    log_map(v, 0.005, 10.0)
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
pub(crate) fn exp_coef(secs: f32, sr: f32) -> f32 {
    let n = (secs * sr).max(1.0);
    (-6.907_755 / n).exp()
}
