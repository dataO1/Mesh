//! Drum lane target layout: which event `target` is which drum sound.
//!
//! The drum generator and the style loader have to agree on target numbers.
//! This is the convention, derived purely from the drum lane's
//! [`LaneSpec::sub_voices`] (for each kind, the first sub-voice of that kind
//! and its selected sampler option):
//!
//! | Targets | Sub-voice | Mapping |
//! |---|---|---|
//! | `0..8` | `backbone` | sample `i` → target `i`. **0 = kick, 1 = snare**, 2 = ghost snare / rim (optional), 3+ unused by the generator |
//! | `8..16` | `hats` | sample `i` → target `8 + i`. **8 = closed, 9 = open** (optional) |
//! | `16..24` | `perc` | sample `i` → target `16 + i`, picked by seed |
//! | `32..96` | `break` | every slice of every sample, in file order, on consecutive targets from 32 (`SampleBank::map_slices(32 + n, ..)`). A sample without slice markers counts as one slice of type `other` |
//!
//! Samples beyond a block's size are not addressable. Slices are assumed to
//! be an even grid over the break: up to 16 slices are one bar, a multiple of
//! 16 is that many bars of 16ths.
//!
//! **Choke groups.** Break slice events carry the break sub-voice's `choke`
//! (falling back to its first sample's, then to [`DEFAULT_BREAK_CHOKE`]) so
//! slices cut each other. Hat events carry the hats sub-voice's `choke`
//! (0 = the slot's own default). Other events carry 0.
//!
//! **Onset mask.** [`DrumLayout::onset_mask`] turns a rendered drum bar into
//! the `BarCtx::drum_mask` the other lanes read; see its docs for the
//! encoding.

use crate::live::event::{EventKind, LiveEvent};
use crate::live::style::{InstrumentSource, LaneSpec, SliceType, SubVoiceKind, SubVoiceSpec};
use crate::live::{STEPS_PER_BAR, TICKS_PER_STEP};

/// First target of each block.
pub const BACKBONE_BASE: u8 = 0;
pub const HATS_BASE: u8 = 8;
pub const PERC_BASE: u8 = 16;
pub const BREAK_BASE: u8 = 32;
/// Targets per one-shot block.
pub const BLOCK_LEN: u8 = 8;
/// Most break slices a layout addresses.
pub const MAX_BREAK_SLICES: usize = 64;
/// Choke group for break slices when the style sets none.
pub const DEFAULT_BREAK_CHOKE: u8 = 1;

/// `drum_mask` values at or above this mark a kick on that step. Lower
/// non-zero values are other backbone onsets (snares).
pub const MASK_KICK_MIN: u8 = 96;
/// Break slices quieter than this are ghosted (off the backbone) and do not
/// count as onsets in the `drum_mask`.
pub const MASK_SLICE_MIN_VEL: u8 = 80;

// MUSICAL: first draft, tune by ear. Slice types of a 16-slice amen-style
// break, used when no style is given (debug tool, tests). The real positions
// come from the style's slice markers (research §7.2: unverified).
const STANDARD_BREAK: [SliceType; 16] = {
    use SliceType::*;
    [Kick, Hat, Kick, Hat, Snare, Hat, Hat, Ghost, Hat, Ghost, Kick, Kick, Snare, Hat, Hat, Ghost]
};

/// What a drum-lane target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrumClass {
    Kick,
    Snare,
    Ghost,
    HatClosed,
    HatOpen,
    Perc(u8),
    /// Break slice index and its type.
    Slice(u8, SliceType),
    Unknown,
}

/// The drum lane's target layout. `Copy`, no heap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrumLayout {
    pub kick: Option<u8>,
    pub snare: Option<u8>,
    /// Ghost snare / rim. `None` = ghosts use the snare.
    pub ghost: Option<u8>,
    pub hat_closed: Option<u8>,
    /// `None` = open hats use the closed hat.
    pub hat_open: Option<u8>,
    /// Number of percussion targets from [`PERC_BASE`].
    pub perc_count: u8,
    pub hats_choke: u8,
    pub break_choke: u8,
    slice_count: u8,
    slice_types: [SliceType; MAX_BREAK_SLICES],
}

impl DrumLayout {
    /// A layout with no targets: the generator renders nothing.
    pub const fn empty() -> Self {
        Self {
            kick: None,
            snare: None,
            ghost: None,
            hat_closed: None,
            hat_open: None,
            perc_count: 0,
            hats_choke: 0,
            break_choke: DEFAULT_BREAK_CHOKE,
            slice_count: 0,
            slice_types: [SliceType::Other; MAX_BREAK_SLICES],
        }
    }

    /// Kick, snare, closed and open hat, one percussion sound and a 16-slice
    /// break. Used when no lane spec is available.
    pub fn standard() -> Self {
        Self::empty()
            .with_backbone(2)
            .with_hats(2, 2)
            .with_perc(1)
            .with_break(&STANDARD_BREAK, DEFAULT_BREAK_CHOKE)
    }

    /// Backbone block with `samples` one-shots (kick, snare, ghost).
    pub fn with_backbone(mut self, samples: usize) -> Self {
        self.kick = (samples > 0).then_some(BACKBONE_BASE);
        self.snare = (samples > 1).then_some(BACKBONE_BASE + 1);
        self.ghost = (samples > 2).then_some(BACKBONE_BASE + 2);
        self
    }

    /// Hats block with `samples` one-shots (closed, open).
    pub fn with_hats(mut self, samples: usize, choke: u8) -> Self {
        self.hat_closed = (samples > 0).then_some(HATS_BASE);
        self.hat_open = (samples > 1).then_some(HATS_BASE + 1);
        self.hats_choke = choke;
        self
    }

    pub fn with_perc(mut self, samples: usize) -> Self {
        self.perc_count = samples.min(BLOCK_LEN as usize) as u8;
        self
    }

    /// Break block: one target per slice type, in order.
    pub fn with_break(mut self, types: &[SliceType], choke: u8) -> Self {
        let n = types.len().min(MAX_BREAK_SLICES);
        self.slice_types = [SliceType::Other; MAX_BREAK_SLICES];
        self.slice_types[..n].copy_from_slice(&types[..n]);
        self.slice_count = n as u8;
        self.break_choke = if choke != 0 { choke } else { DEFAULT_BREAK_CHOKE };
        self
    }

    /// Layout of a drum lane with every sub-voice on its default option.
    pub fn from_lane(lane: &LaneSpec) -> Self {
        Self::from_sub_voices(&lane.sub_voices, |sv| sv.default_option)
    }

    /// Layout for the option `select` picks per sub-voice (kit browsing).
    /// Out-of-range selections and non-sampler options give an empty block.
    pub fn from_sub_voices(subs: &[SubVoiceSpec], select: impl Fn(&SubVoiceSpec) -> usize) -> Self {
        let samples = |kind: SubVoiceKind| {
            let sv = subs.iter().find(|sv| sv.voice == kind)?;
            match sv.options.get(select(sv))?.source()? {
                InstrumentSource::Sampler(s) => Some((sv, &s.samples)),
                InstrumentSource::Clap(_) => None,
            }
        };
        let mut layout = Self::empty();
        if let Some((_, s)) = samples(SubVoiceKind::Backbone) {
            layout = layout.with_backbone(s.len());
        }
        if let Some((sv, s)) = samples(SubVoiceKind::Hats) {
            layout = layout.with_hats(s.len(), sv.choke);
        }
        if let Some((_, s)) = samples(SubVoiceKind::Perc) {
            layout = layout.with_perc(s.len());
        }
        if let Some((sv, s)) = samples(SubVoiceKind::Break) {
            let mut types = [SliceType::Other; MAX_BREAK_SLICES];
            let mut n = 0;
            for sample in s.iter() {
                if sample.slices.is_empty() {
                    if n < MAX_BREAK_SLICES {
                        n += 1;
                    }
                    continue;
                }
                for slice in &sample.slices {
                    if n < MAX_BREAK_SLICES {
                        types[n] = slice.kind;
                        n += 1;
                    }
                }
            }
            let choke = if sv.choke != 0 { sv.choke } else { s.first().map_or(0, |x| x.choke) };
            layout = layout.with_break(&types[..n], choke);
        }
        layout
    }

    /// Slice types of the break, by slice index.
    pub fn slices(&self) -> &[SliceType] {
        &self.slice_types[..self.slice_count as usize]
    }

    /// Target of break slice `index`.
    #[inline]
    pub fn slice_target(&self, index: usize) -> u8 {
        BREAK_BASE + index as u8
    }

    /// Slices per bar of the break (see module docs).
    pub fn slices_per_bar(&self) -> usize {
        let n = self.slice_count as usize;
        if n > STEPS_PER_BAR && n % STEPS_PER_BAR == 0 { STEPS_PER_BAR } else { n }
    }

    /// Bars the break covers (at least 1).
    pub fn break_bars(&self) -> usize {
        match self.slices_per_bar() {
            0 => 1,
            spb => (self.slice_count as usize / spb).max(1),
        }
    }

    /// What `target` is in this layout.
    pub fn class(&self, target: u8) -> DrumClass {
        if Some(target) == self.kick {
            DrumClass::Kick
        } else if Some(target) == self.snare {
            DrumClass::Snare
        } else if Some(target) == self.ghost {
            DrumClass::Ghost
        } else if Some(target) == self.hat_closed {
            DrumClass::HatClosed
        } else if Some(target) == self.hat_open {
            DrumClass::HatOpen
        } else if (PERC_BASE..PERC_BASE + self.perc_count).contains(&target) {
            DrumClass::Perc(target - PERC_BASE)
        } else if target >= BREAK_BASE && ((target - BREAK_BASE) as usize) < self.slice_count as usize {
            let i = target - BREAK_BASE;
            DrumClass::Slice(i, self.slice_types[i as usize])
        } else {
            DrumClass::Unknown
        }
    }

    /// The `BarCtx::drum_mask` for a rendered drum bar.
    ///
    /// Only kick and snare class hits count: backbone one-shots, and break
    /// slices of those types at [`MASK_SLICE_MIN_VEL`] or louder. Hats, ghosts,
    /// percussion and ghosted slices leave the step 0, so the bass and lead
    /// see them as gaps. Encoding per step:
    ///
    /// - `0`: no backbone onset.
    /// - `1..MASK_KICK_MIN`: a snare, scaled from its velocity.
    /// - `MASK_KICK_MIN..=127`: a kick, scaled from its velocity.
    pub fn onset_mask(&self, events: &[LiveEvent]) -> [u8; STEPS_PER_BAR] {
        let mut mask = [0u8; STEPS_PER_BAR];
        for ev in events {
            if ev.kind != EventKind::Trigger || ev.vel == 0 {
                continue;
            }
            let step = (ev.tick / TICKS_PER_STEP) as usize;
            if step >= STEPS_PER_BAR {
                continue;
            }
            let vel = ev.vel.min(127) as u16;
            let class = match self.class(ev.target) {
                DrumClass::Slice(..) if ev.vel < MASK_SLICE_MIN_VEL => continue,
                DrumClass::Slice(_, SliceType::Kick) => DrumClass::Kick,
                DrumClass::Slice(_, SliceType::Snare) => DrumClass::Snare,
                other => other,
            };
            let value = match class {
                DrumClass::Kick => MASK_KICK_MIN + (vel * (127 - MASK_KICK_MIN as u16) / 127) as u8,
                DrumClass::Snare => 1 + (vel * (MASK_KICK_MIN as u16 - 2) / 127) as u8,
                _ => 0,
            };
            mask[step] = mask[step].max(value);
        }
        mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::generator::blocks::trigger;
    use crate::live::style::StyleSpec;

    fn fixture_lane() -> LaneSpec {
        let text = include_str!("../../../tests/fixtures/live-style/style.yaml");
        StyleSpec::from_yaml(text).unwrap().lanes.remove(0)
    }

    #[test]
    fn derived_from_the_fixture_style() {
        let l = DrumLayout::from_lane(&fixture_lane());
        assert_eq!(l.kick, Some(0));
        assert_eq!(l.snare, Some(1));
        assert_eq!(l.ghost, None);
        assert_eq!(l.hat_closed, Some(8));
        assert_eq!(l.hat_open, Some(9));
        assert_eq!(l.hats_choke, 2);
        assert_eq!(l.perc_count, 1);
        assert_eq!(l.break_choke, 1);
        assert_eq!(l.slices(), [SliceType::Kick, SliceType::Hat, SliceType::Snare, SliceType::Ghost]);
        assert_eq!(l.slice_target(2), 34);
        assert_eq!(l.slices_per_bar(), 4);
        assert_eq!(l.break_bars(), 1);
    }

    #[test]
    fn classes() {
        let l = DrumLayout::standard();
        assert_eq!(l.class(0), DrumClass::Kick);
        assert_eq!(l.class(1), DrumClass::Snare);
        assert_eq!(l.class(2), DrumClass::Unknown);
        assert_eq!(l.class(8), DrumClass::HatClosed);
        assert_eq!(l.class(9), DrumClass::HatOpen);
        assert_eq!(l.class(16), DrumClass::Perc(0));
        assert_eq!(l.class(17), DrumClass::Unknown);
        assert_eq!(l.class(32), DrumClass::Slice(0, SliceType::Kick));
        assert_eq!(l.class(36), DrumClass::Slice(4, SliceType::Snare));
        assert_eq!(l.class(48), DrumClass::Unknown);
        assert_eq!(DrumLayout::empty().class(0), DrumClass::Unknown);
    }

    #[test]
    fn break_grid() {
        let l = DrumLayout::empty().with_break(&[SliceType::Other; 32], 0);
        assert_eq!((l.slices_per_bar(), l.break_bars()), (16, 2));
        assert_eq!(l.break_choke, DEFAULT_BREAK_CHOKE);
        let l = DrumLayout::empty().with_break(&[SliceType::Other; 8], 3);
        assert_eq!((l.slices_per_bar(), l.break_bars()), (8, 1));
        assert_eq!(l.break_choke, 3);
        let l = DrumLayout::empty().with_break(&[SliceType::Other; 200], 0);
        assert_eq!(l.slices().len(), MAX_BREAK_SLICES);
        assert_eq!(DrumLayout::empty().break_bars(), 1);
    }

    #[test]
    fn option_selection_and_missing_voices() {
        let mut lane = fixture_lane();
        // An out-of-range option leaves the block empty instead of panicking.
        let l = DrumLayout::from_sub_voices(&lane.sub_voices, |_| 9);
        assert_eq!(l, DrumLayout::empty());
        lane.sub_voices.retain(|sv| sv.voice != SubVoiceKind::Break);
        let l = DrumLayout::from_lane(&lane);
        assert!(l.slices().is_empty());
        assert_eq!(l.kick, Some(0));
    }

    #[test]
    fn onset_mask_encoding() {
        let l = DrumLayout::standard();
        let events = [
            trigger(0, 0, 120),    // kick
            trigger(96, 1, 120),   // snare
            trigger(48, 8, 127),   // hat: not in the mask
            trigger(240, 42, 100), // break slice 10 (kick)
            trigger(288, 36, 90),  // break slice 4 (snare)
            trigger(300, 0, 0),    // silent
            trigger(336, 42, 60),  // ghosted kick slice: not in the mask
        ];
        let m = l.onset_mask(&events);
        assert!(m[0] >= MASK_KICK_MIN);
        assert!(m[4] > 0 && m[4] < MASK_KICK_MIN);
        assert_eq!(m[2], 0);
        assert!(m[10] >= MASK_KICK_MIN);
        assert!(m[12] > 0 && m[12] < m[4]);
        assert_eq!(m[14], 0);
        assert_eq!(m.iter().filter(|&&v| v > 0).count(), 4);
        // Full-velocity snare stays below the kick range, quiet kick inside it.
        assert!(l.onset_mask(&[trigger(0, 1, 127)])[0] < MASK_KICK_MIN);
        assert!(l.onset_mask(&[trigger(0, 0, 1)])[0] >= MASK_KICK_MIN);
    }
}
