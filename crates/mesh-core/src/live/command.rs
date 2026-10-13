//! Live-mode commands (design §4.4, §7).
//!
//! [`LiveCommand`] is what the UI and the controller mapping produce. The
//! [`LiveController`](super::engine::LiveController) applies each one to its
//! non-RT state and routes the effect:
//!
//! - structural inputs (seeds, generator parameters, key, progression,
//!   half-time) go to the bar renderer,
//! - everything the audio thread must act on goes over an `rtrb` ring as an
//!   [`RtMsg`], already resolved through the style's control mapping so the
//!   RT side only stores numbers.

use crate::music::MusicalKey;
use crate::types::StereoSample;

use super::capture::{Capture, CaptureScope};
use super::glue::{GlueParams, Role};
use super::sampler::{LaneSamplerParams, CUTOFF_MAX_HZ, CUTOFF_MIN_HZ};
use super::state::move_bits;
use super::style::SendBus;

/// Capacity of the controller → RT message ring.
pub const RT_MSG_CAPACITY: usize = 1024;

/// A momentary move, held on a pad (design §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveKind {
    /// Repeat the current window at the roll division.
    Stutter,
    /// Play the pre-rendered fill bar.
    Fill,
    /// Play the bar at double rate.
    Double,
    /// Mute the lane.
    Void,
}

impl MoveKind {
    pub const ALL: [MoveKind; 4] = [MoveKind::Stutter, MoveKind::Fill, MoveKind::Double, MoveKind::Void];

    /// Bit in [`LaneView::moves`](super::state::LaneView::moves).
    pub fn bit(self) -> u8 {
        match self {
            MoveKind::Stutter => move_bits::STUTTER,
            MoveKind::Fill => move_bits::FILL,
            MoveKind::Double => move_bits::DOUBLE,
            MoveKind::Void => move_bits::VOID,
        }
    }
}

/// A performance command from the UI or the controller surface.
pub enum LiveCommand {
    /// Pick the chain slot the seed encoder edits.
    SelectSlot { lane: u8, slot: u8 },
    /// Set one slot's seed. Lands on the next bar that plays the slot.
    SetSlotSeed { lane: u8, slot: u8, seed: u32 },
    /// Step the selected slot's seed (encoder).
    SeedDelta { lane: u8, delta: i32 },
    /// Move performance control `control` (0..4) to `value` (0..=1). Routed
    /// through the style's control mapping.
    SetControl { lane: u8, control: u8, value: f32 },
    SetLaneOn { lane: u8, on: bool },
    ToggleLane { lane: u8 },
    /// Lane toggles land on the bar (true, default) or immediately with a
    /// short fade (false) (D41).
    SetQuantize(bool),
    /// Step the browsed kit / instrument option (lane button held + encoder).
    /// Nothing loads until [`LiveCommand::CommitOption`].
    BrowseOption { lane: u8, delta: i32 },
    /// Load the browsed option at the next bar (lane button released).
    CommitOption { lane: u8 },
    /// Load an option at the next bar.
    SetOption { lane: u8, option: u16 },
    MovePress { lane: u8, kind: MoveKind },
    MoveRelease { lane: u8, kind: MoveKind },
    /// Stutter / roll window length in ticks (24 = one 16th).
    SetRollDivision { ticks: u16 },
    /// Key-intent slider position, −1..=1. Shows the target, arms nothing.
    SetKeyIntent(f32),
    /// Arm the intent target for the next phrase.
    CommitKey,
    /// Arm a key directly for the next phrase.
    SetKey(MusicalKey),
    /// Arm a progression for the next phrase.
    SelectProgression(usize),
    /// Step the armed progression (encoder).
    ProgressionDelta(i32),
    /// Ramp to `bpm` over `bars` bars, starting at the next bar (0 = jump).
    RequestTempo { bpm: f64, bars: u32 },
    SetHalfTime(bool),
    ToggleHalfTime,
    /// Snapshot the state into the capture bank and save it off-thread.
    Capture(CaptureScope),
    RecallCapture(Box<Capture>),
    /// Feed the master output into a set recording.
    StartRecording(rtrb::Producer<StereoSample>),
    StopRecording,
}

/// Glue parameter a performance control can drive (`type: glue` in the style).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlueControl {
    /// Duck depth of the lane's role. 0..1 → 0..[`MAX_DUCK_DB`] dB.
    DuckDepth,
    /// Drum-bus saturation drive. 0..1 → 0..[`MAX_DRIVE_DB`] dB.
    DrumDrive,
    /// Drum-bus compressor threshold. 0..1 → 0..−36 dB.
    DrumComp,
    /// Shared drive amount. 0..1 → 0..[`MAX_DRIVE_DB`] dB.
    SharedDrive,
    /// Shared drive wet mix, 0..1.
    SharedDriveMix,
    /// Master compressor threshold. 0..1 → 0..−24 dB.
    MasterComp,
    /// High-pass of the lane's role. 0..1 → 20 Hz..2 kHz (log).
    HighPass,
    /// Shared delay feedback, 0..1 → 0..0.95.
    DelayFeedback,
    /// Shared delay time in beats (used as given).
    DelayBeats,
    /// Shared reverb room size, 0..1.
    ReverbSize,
    /// Shared reverb damping, 0..1.
    ReverbDamping,
}

// MUSICAL: full-scale ranges of the style's normalized glue amounts.
/// Duck depth at amount 1.0.
pub const MAX_DUCK_DB: f32 = 24.0;
/// Saturation pre-gain at drive amount 1.0.
pub const MAX_DRIVE_DB: f32 = 24.0;

/// Saturation pre-gain in dB for a normalized drive amount.
pub fn drive_db(amount: f32) -> f32 {
    amount.clamp(0.0, 1.0) * MAX_DRIVE_DB
}

impl GlueControl {
    /// Names accepted in `ControlTarget::Glue { param }`.
    pub const NAMES: [(&'static str, GlueControl); 11] = [
        ("duck_depth", GlueControl::DuckDepth),
        ("drum_drive", GlueControl::DrumDrive),
        ("drum_comp", GlueControl::DrumComp),
        ("shared_drive", GlueControl::SharedDrive),
        ("shared_drive_mix", GlueControl::SharedDriveMix),
        ("master_comp", GlueControl::MasterComp),
        ("hpf", GlueControl::HighPass),
        ("delay_feedback", GlueControl::DelayFeedback),
        ("delay_beats", GlueControl::DelayBeats),
        ("reverb_size", GlueControl::ReverbSize),
        ("reverb_damping", GlueControl::ReverbDamping),
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Self::NAMES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
    }

    /// Write `value` into `params`. `role` is the role of the lane whose
    /// control moved (per-role parameters). Allocation-free.
    pub fn apply(self, params: &mut GlueParams, role: Role, value: f32) {
        let unit = value.clamp(0.0, 1.0);
        match self {
            GlueControl::DuckDepth => {
                let db = unit * MAX_DUCK_DB;
                match role {
                    Role::Bass => params.duck.bass_depth_db = db,
                    Role::Lead => params.duck.lead_depth_db = db,
                    Role::Extra => params.duck.extra_depth_db = db,
                    // Drums are the duck key and never ducked: move every role.
                    Role::Drums => {
                        params.duck.bass_depth_db = db;
                        params.duck.lead_depth_db = db * 0.5;
                        params.duck.extra_depth_db = db * 0.5;
                    }
                }
            }
            GlueControl::DrumDrive => params.drum_bus.saturation.drive_db = drive_db(unit),
            GlueControl::DrumComp => params.drum_bus.compressor.threshold_db = -36.0 * unit,
            GlueControl::SharedDrive => params.drive.saturation.drive_db = drive_db(unit),
            GlueControl::SharedDriveMix => params.drive.saturation.mix = unit,
            GlueControl::MasterComp => params.master.compressor.threshold_db = -24.0 * unit,
            GlueControl::HighPass => {
                let hz = 20.0 * 100.0f32.powf(unit);
                let hp = match role {
                    Role::Drums => &mut params.high_pass.drums,
                    Role::Lead => &mut params.high_pass.lead,
                    Role::Extra => &mut params.high_pass.extra,
                    Role::Bass => return,
                };
                hp.enabled = true;
                hp.freq_hz = hz;
            }
            GlueControl::DelayFeedback => params.delay.feedback = unit * 0.95,
            GlueControl::DelayBeats => params.delay.beats = value.clamp(0.03125, 4.0),
            GlueControl::ReverbSize => params.reverb.room_size = unit,
            GlueControl::ReverbDamping => params.reverb.damping = unit,
        }
    }
}

/// Sampler parameter addressed by `ControlTarget::Instrument { param }` on a
/// sampler lane (CLAP lanes use the plugin's own parameter index).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplerParam {
    /// Lowpass cutoff, 0..1 → 20 Hz..20 kHz (log).
    Cutoff = 0,
    Resonance = 1,
    /// 0..1 → 0.5 ms..2 s (log).
    Attack = 2,
    /// 0..1 → 5 ms..10 s (log).
    Decay = 3,
    Sustain = 4,
    /// 0..1 → 5 ms..10 s (log).
    Release = 5,
    /// 0..1 → left..right.
    Pan = 6,
}

impl SamplerParam {
    pub fn from_index(index: u32) -> Option<Self> {
        Some(match index {
            0 => Self::Cutoff,
            1 => Self::Resonance,
            2 => Self::Attack,
            3 => Self::Decay,
            4 => Self::Sustain,
            5 => Self::Release,
            6 => Self::Pan,
            _ => return None,
        })
    }

    /// Write a normalized value into the lane's sampler parameters.
    pub fn apply(self, p: &mut LaneSamplerParams, value: f32) {
        let v = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        let log = |lo: f32, hi: f32| lo * (hi / lo).powf(v);
        match self {
            Self::Cutoff => p.cutoff_hz = log(CUTOFF_MIN_HZ, CUTOFF_MAX_HZ),
            Self::Resonance => p.resonance = v,
            Self::Attack => p.attack = log(0.0005, 2.0),
            Self::Decay => p.decay = log(0.005, 10.0),
            Self::Sustain => p.sustain = v,
            Self::Release => p.release = log(0.005, 10.0),
            Self::Pan => p.pan = v * 2.0 - 1.0,
        }
    }
}

/// Message from the controller to the RT engine. Everything is resolved to
/// indices and numbers; handling one never allocates.
pub enum RtMsg {
    /// Switch a lane on or off, at the next bar (`quantized`) or now.
    LaneOn { lane: u8, on: bool, quantized: bool },
    /// Load a preloaded instrument option / kit at the next bar.
    Option { lane: u8, option: u16 },
    /// Instrument parameter driven by performance control `control`.
    InstrumentParam { lane: u8, control: u8, param: u32, value: f32 },
    FxParam { lane: u8, slot: u8, param: u32, value: f32 },
    Glue { lane: u8, control: GlueControl, value: f32 },
    Send { lane: u8, bus: SendBus, level: f32 },
    MovePress { lane: u8, kind: MoveKind },
    MoveRelease { lane: u8, kind: MoveKind },
    RollDivision { ticks: u16 },
    Tempo { bpm: f64, bars: u32 },
    StartRecording(rtrb::Producer<StereoSample>),
    StopRecording,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glue_names_resolve_and_apply() {
        for (name, control) in GlueControl::NAMES {
            assert_eq!(GlueControl::from_name(name), Some(control));
        }
        assert_eq!(GlueControl::from_name("nope"), None);

        let mut p = GlueParams::default();
        GlueControl::DuckDepth.apply(&mut p, Role::Bass, 0.5);
        assert_eq!(p.duck.bass_depth_db, 12.0);
        assert_eq!(p.duck.lead_depth_db, GlueParams::default().duck.lead_depth_db, "other roles untouched");
        GlueControl::HighPass.apply(&mut p, Role::Bass, 1.0);
        assert_eq!(p.high_pass, GlueParams::default().high_pass, "bass has no high-pass");
        GlueControl::HighPass.apply(&mut p, Role::Lead, 1.0);
        assert!((p.high_pass.lead.freq_hz - 2000.0).abs() < 1.0);
    }

    #[test]
    fn sampler_params_cover_their_ranges() {
        let mut p = LaneSamplerParams::default();
        SamplerParam::Cutoff.apply(&mut p, 0.0);
        assert!((p.cutoff_hz - CUTOFF_MIN_HZ).abs() < 1e-3);
        SamplerParam::Cutoff.apply(&mut p, 1.0);
        assert!((p.cutoff_hz - CUTOFF_MAX_HZ).abs() < 1.0);
        SamplerParam::Pan.apply(&mut p, 0.0);
        assert_eq!(p.pan, -1.0);
        SamplerParam::Sustain.apply(&mut p, f32::NAN);
        assert_eq!(p.sustain, 0.0);
        assert_eq!(SamplerParam::from_index(6), Some(SamplerParam::Pan));
        assert_eq!(SamplerParam::from_index(7), None);
    }
}
