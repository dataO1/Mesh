//! What the 8 pads of a lane do (design Q6, Q12, Q14, Q27, Q33).
//!
//! One table shared by the F1 mapping, its LEDs, the keyboard fallback and
//! the on-screen pad mirror. Pads are numbered 0..8 row-major inside a
//! lane's 2×4 half: `0 1 / 2 3 / 4 5 / 6 7`.
//!
//! ```text
//!  drums            drums + Shift     bass / lead / extra
//!  ┌──────┬──────┐  ┌──────┬──────┐   ┌──────┬──────┐
//!  │ kick │snare │  │      │      │   │ oct− │ oct+ │
//!  │ hats │ ride │  │      │      │   │ half │ dbl  │
//!  │shaker│ perc │  │      │      │   │form◀ │form▶ │
//!  │ top  │break │  │form◀ │form▶ │   │      │      │
//!  └──────┴──────┘  └──────┴──────┘   └──────┴──────┘
//! ```

use mesh_core::live::generator::layout::DrumVoice;
use mesh_core::live::state::{LaneView, Rate};

/// Pads per lane half.
pub const PADS: usize = 8;

/// The function of one lane pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadFn {
    /// Drum lane: mute toggle of one voice.
    Mute(DrumVoice),
    OctaveDown,
    OctaveUp,
    /// Latched rate toggle (half or double).
    Rate(Rate),
    FormPrev,
    FormNext,
    None,
}

/// What pad `pad` (0..8) of a lane does. `shift` is the unit's Shift.
pub const fn pad_fn(drums: bool, pad: usize, shift: bool) -> PadFn {
    if drums {
        if shift {
            return match pad {
                6 => PadFn::FormPrev,
                7 => PadFn::FormNext,
                _ => PadFn::None,
            };
        }
        if pad < PADS {
            return PadFn::Mute(DrumVoice::ALL[pad]);
        }
        return PadFn::None;
    }
    match pad {
        0 => PadFn::OctaveDown,
        1 => PadFn::OctaveUp,
        2 => PadFn::Rate(Rate::Half),
        3 => PadFn::Rate(Rate::Double),
        4 => PadFn::FormPrev,
        5 => PadFn::FormNext,
        _ => PadFn::None,
    }
}

/// Short label for the screen.
pub fn label(f: PadFn) -> &'static str {
    match f {
        PadFn::Mute(v) => v.name(),
        PadFn::OctaveDown => "oct−",
        PadFn::OctaveUp => "oct+",
        PadFn::Rate(Rate::Half) => "half",
        PadFn::Rate(Rate::Double) => "dbl",
        PadFn::Rate(Rate::Normal) => "norm",
        PadFn::FormPrev => "form◀",
        PadFn::FormNext => "form▶",
        PadFn::None => "",
    }
}

/// How a pad shows its state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PadLook {
    /// Brightness 0..=1 relative to the lane colour (0 = dark).
    pub level: f32,
    /// A change is queued for the next bar / phrase: blink.
    pub pending: bool,
}

// PLAYABILITY: tune on hardware. Pad brightness relative to the lane colour.
pub const PAD_ON: f32 = 1.0;
pub const PAD_IDLE: f32 = 0.15;
pub const PAD_MUTED: f32 = 0.04;

/// State of a pad's function on a lane.
pub fn look(view: &LaneView, f: PadFn) -> PadLook {
    match f {
        PadFn::Mute(v) => {
            let requested = view.mutes & v.bit() != 0;
            let playing = view.mutes_playing & v.bit() != 0;
            PadLook { level: if requested { PAD_MUTED } else { PAD_ON }, pending: requested != playing }
        }
        PadFn::OctaveDown => PadLook { level: octave_level(-view.octave), pending: false },
        PadFn::OctaveUp => PadLook { level: octave_level(view.octave), pending: false },
        PadFn::Rate(r) => PadLook { level: if view.rate == r { PAD_ON } else { PAD_IDLE }, pending: false },
        PadFn::FormPrev | PadFn::FormNext => PadLook { level: PAD_IDLE, pending: view.form_pending.is_some() },
        PadFn::None => PadLook { level: 0.0, pending: false },
    }
}

/// Brightness to show: a pending pad blinks between its state and the
/// opposite one (lit ↔ dark), so the change stands out.
pub fn shown_level(look: PadLook, blink_on: bool) -> f32 {
    if look.pending && !blink_on {
        if look.level >= 0.5 { PAD_MUTED } else { PAD_ON }
    } else {
        look.level
    }
}

/// Octave pads: idle at 0, half lit one octave away, full at two.
fn octave_level(steps: i8) -> f32 {
    match steps {
        s if s >= 2 => PAD_ON,
        1 => 0.5,
        _ => PAD_IDLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drum_pads_follow_the_voice_order() {
        let fns: Vec<PadFn> = (0..PADS).map(|p| pad_fn(true, p, false)).collect();
        assert_eq!(fns, DrumVoice::ALL.map(PadFn::Mute).to_vec());
        assert_eq!(pad_fn(true, 6, true), PadFn::FormPrev);
        assert_eq!(pad_fn(true, 7, true), PadFn::FormNext);
        assert_eq!(pad_fn(true, 0, true), PadFn::None, "Shift leaves the mutes alone");
    }

    #[test]
    fn pitched_pads() {
        assert_eq!(pad_fn(false, 0, false), PadFn::OctaveDown);
        assert_eq!(pad_fn(false, 3, false), PadFn::Rate(Rate::Double));
        assert_eq!(pad_fn(false, 5, true), PadFn::FormNext, "Shift does not change pitched pads");
        assert_eq!(pad_fn(false, 7, false), PadFn::None);
    }
}
