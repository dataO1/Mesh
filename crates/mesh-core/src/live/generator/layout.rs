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
//! | `8..40` | `hats` | pool, sample `i` → target `8 + i`, tagged closed / open |
//! | `40..56` | `ride` | pool, tagged ride / crash |
//! | `56..72` | `shaker` | pool |
//! | `72..104` | `perc` | pool |
//! | `104..168` | `break` | every slice of every sample, in file order, on consecutive targets from 104 (`SampleBank::map_slices(104 + n, ..)`). A sample without slice markers counts as one slice of type `other` |
//! | `168..200` | `top` | pool of loops, one target per loop (whole sample, chopped by `start`) |
//!
//! Pool sub-voices (everything but backbone and break) keep their option when
//! the kit is browsed; the seed picks samples inside the pool. Samples beyond
//! a block's size are not addressable. Break slices are assumed to be an even
//! grid over the break: up to 16 slices are one bar, a multiple of 16 is that
//! many bars of 16ths.
//!
//! **Choke groups.** Break slice events carry the break sub-voice's `choke`
//! (falling back to its first sample's, then to [`DEFAULT_BREAK_CHOKE`]) so
//! slices cut each other. Top-loop chops carry [`DEFAULT_TOP_CHOKE`] (or the
//! top sub-voice's). Hat events carry the hats sub-voice's `choke`
//! (0 = the slot's own default). Other events carry 0.
//!
//! **Voices.** [`DrumVoice`] groups the targets into the 8 voices the mute
//! pads switch (design Q33).
//!
//! **Onset mask.** [`DrumLayout::onset_mask`] turns a rendered drum bar into
//! the `BarCtx::drum_mask` the other lanes read; see its docs for the
//! encoding.

use crate::live::event::{EventKind, LiveEvent};
use crate::live::style::{InstrumentSource, LaneSpec, SampleSpec, SampleTag, SliceType, SubVoiceKind, SubVoiceSpec};
use crate::live::{STEPS_PER_BAR, TICKS_PER_STEP};

/// First target of each block.
pub const BACKBONE_BASE: u8 = 0;
pub const HATS_BASE: u8 = 8;
pub const RIDE_BASE: u8 = 40;
pub const SHAKER_BASE: u8 = 56;
pub const PERC_BASE: u8 = 72;
pub const BREAK_BASE: u8 = 104;
pub const TOP_BASE: u8 = 168;
/// Targets of the backbone block.
pub const BACKBONE_LEN: u8 = 8;
pub const HATS_LEN: u8 = 32;
pub const RIDE_LEN: u8 = 16;
pub const SHAKER_LEN: u8 = 16;
pub const PERC_LEN: u8 = 32;
pub const TOP_LEN: u8 = 32;
/// Most break slices a layout addresses.
pub const MAX_BREAK_SLICES: usize = 64;
/// Choke group for break slices when the style sets none.
pub const DEFAULT_BREAK_CHOKE: u8 = 1;
/// Choke group of top-loop chops when the style sets none.
pub const DEFAULT_TOP_CHOKE: u8 = 3;
/// Longest top loop in 16th steps (8 bars).
pub const MAX_TOP_STEPS: u16 = 8 * STEPS_PER_BAR as u16;

/// `(first target, target count)` of a drum sub-voice.
pub const fn target_range(kind: SubVoiceKind) -> (u8, u8) {
    match kind {
        SubVoiceKind::Backbone => (BACKBONE_BASE, BACKBONE_LEN),
        SubVoiceKind::Hats => (HATS_BASE, HATS_LEN),
        SubVoiceKind::Ride => (RIDE_BASE, RIDE_LEN),
        SubVoiceKind::Shaker => (SHAKER_BASE, SHAKER_LEN),
        SubVoiceKind::Perc => (PERC_BASE, PERC_LEN),
        SubVoiceKind::Break => (BREAK_BASE, MAX_BREAK_SLICES as u8),
        SubVoiceKind::Top => (TOP_BASE, TOP_LEN),
    }
}

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

/// The 8 drum voices the mute pads switch, in pad order (Q33).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DrumVoice {
    Kick = 0,
    /// Snare and ghosts.
    Snare = 1,
    Hats = 2,
    /// Ride and crashes.
    Ride = 3,
    Shaker = 4,
    Perc = 5,
    Top = 6,
    Break = 7,
}

impl DrumVoice {
    pub const ALL: [DrumVoice; 8] = [
        DrumVoice::Kick,
        DrumVoice::Snare,
        DrumVoice::Hats,
        DrumVoice::Ride,
        DrumVoice::Shaker,
        DrumVoice::Perc,
        DrumVoice::Top,
        DrumVoice::Break,
    ];

    /// Bit in a mute mask.
    pub const fn bit(self) -> u8 {
        1 << self as u8
    }

    pub fn name(self) -> &'static str {
        match self {
            DrumVoice::Kick => "kick",
            DrumVoice::Snare => "snare",
            DrumVoice::Hats => "hats",
            DrumVoice::Ride => "ride",
            DrumVoice::Shaker => "shaker",
            DrumVoice::Perc => "perc",
            DrumVoice::Top => "top",
            DrumVoice::Break => "break",
        }
    }

    /// The voice a drum-lane target belongs to. RT-safe.
    pub const fn of_target(target: u8) -> Option<DrumVoice> {
        Some(match target {
            0 => DrumVoice::Kick,
            t if t < HATS_BASE => DrumVoice::Snare,
            t if t < RIDE_BASE => DrumVoice::Hats,
            t if t < SHAKER_BASE => DrumVoice::Ride,
            t if t < PERC_BASE => DrumVoice::Shaker,
            t if t < BREAK_BASE => DrumVoice::Perc,
            t if t < TOP_BASE => DrumVoice::Break,
            t if t < TOP_BASE + TOP_LEN => DrumVoice::Top,
            _ => return None,
        })
    }

    /// Choke group to fade when the voice is muted immediately (0 = none).
    pub fn choke(self, layout: &DrumLayout) -> u8 {
        match self {
            DrumVoice::Break => layout.break_choke,
            DrumVoice::Top => layout.top_choke,
            _ => 0,
        }
    }
}

/// Pad overrides of the drum voices, packed in a `u16`: the low byte forces
/// voices on, the high byte forces them off (bits per [`DrumVoice`]). The
/// voices playing are the bar's arrangement with the overrides applied.
pub mod voice_override {
    /// Voices forced on.
    pub const fn on(o: u16) -> u8 {
        o as u8
    }

    /// Voices forced off.
    pub const fn off(o: u16) -> u8 {
        (o >> 8) as u8
    }

    /// The voices playing for an arrangement `default`.
    pub const fn apply(default: u8, o: u16) -> u8 {
        (default | on(o)) & !off(o)
    }

    /// Turn voice `bit` off if it plays (`playing`), else on.
    pub const fn toggle(o: u16, bit: u8, playing: bool) -> u16 {
        let (mut on, mut off) = (on(o), off(o));
        if playing {
            off |= bit;
            on &= !bit;
        } else {
            on |= bit;
            off &= !bit;
        }
        on as u16 | (off as u16) << 8
    }
}

/// What a drum-lane target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrumClass {
    Kick,
    Snare,
    Ghost,
    HatClosed,
    HatOpen,
    Ride,
    Crash,
    Shaker(u8),
    Perc(u8),
    /// Break slice index and its type.
    Slice(u8, SliceType),
    /// Top loop index.
    Top(u8),
    Unknown,
}

/// The drum lane's target layout. `Copy`, no heap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrumLayout {
    pub kick: Option<u8>,
    pub snare: Option<u8>,
    /// Ghost snare / rim. `None` = ghosts use the snare.
    pub ghost: Option<u8>,
    /// Closed / open hats: bit `i` = target `HATS_BASE + i`.
    pub hats_closed: u32,
    pub hats_open: u32,
    /// Rides / crashes: bit `i` = target `RIDE_BASE + i`.
    pub rides: u16,
    pub crashes: u16,
    /// Number of shaker targets from [`SHAKER_BASE`].
    pub shaker_count: u8,
    /// Number of percussion targets from [`PERC_BASE`].
    pub perc_count: u8,
    /// Number of top loops from [`TOP_BASE`].
    pub top_count: u8,
    /// Length of each top loop in 16th steps.
    pub top_steps: [u16; TOP_LEN as usize],
    pub hats_choke: u8,
    pub break_choke: u8,
    pub top_choke: u8,
    slice_count: u8,
    slice_types: [SliceType; MAX_BREAK_SLICES],
}

/// Bits `0..n` of a mask.
const fn low_bits(n: usize) -> u32 {
    if n >= 32 { u32::MAX } else { (1u32 << n) - 1 }
}

impl DrumLayout {
    /// A layout with no targets: the generator renders nothing.
    pub const fn empty() -> Self {
        Self {
            kick: None,
            snare: None,
            ghost: None,
            hats_closed: 0,
            hats_open: 0,
            rides: 0,
            crashes: 0,
            shaker_count: 0,
            perc_count: 0,
            top_count: 0,
            top_steps: [0; TOP_LEN as usize],
            hats_choke: 0,
            break_choke: DEFAULT_BREAK_CHOKE,
            top_choke: DEFAULT_TOP_CHOKE,
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

    /// Hats block with `samples` untagged one-shots: the second is open, the
    /// rest closed.
    pub fn with_hats(mut self, samples: usize, choke: u8) -> Self {
        let n = samples.min(HATS_LEN as usize);
        let all = low_bits(n);
        self.hats_open = if n > 1 { 0b10 } else { 0 };
        self.hats_closed = all & !self.hats_open;
        self.hats_choke = choke;
        self
    }

    /// Hats block from explicit closed / open masks.
    pub fn with_hat_masks(mut self, closed: u32, open: u32, choke: u8) -> Self {
        self.hats_closed = closed;
        self.hats_open = open & !closed;
        self.hats_choke = choke;
        self
    }

    /// Ride block from ride / crash masks.
    pub fn with_rides(mut self, rides: u16, crashes: u16) -> Self {
        self.rides = rides;
        self.crashes = crashes & !rides;
        self
    }

    pub fn with_shakers(mut self, samples: usize) -> Self {
        self.shaker_count = samples.min(SHAKER_LEN as usize) as u8;
        self
    }

    pub fn with_perc(mut self, samples: usize) -> Self {
        self.perc_count = samples.min(PERC_LEN as usize) as u8;
        self
    }

    /// Top loops, by length in 16th steps.
    pub fn with_tops(mut self, steps: &[u16], choke: u8) -> Self {
        let n = steps.len().min(TOP_LEN as usize);
        self.top_steps = [0; TOP_LEN as usize];
        for (dst, &st) in self.top_steps.iter_mut().zip(&steps[..n]) {
            *dst = st.clamp(1, MAX_TOP_STEPS);
        }
        self.top_count = n as u8;
        self.top_choke = if choke != 0 { choke } else { DEFAULT_TOP_CHOKE };
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
                InstrumentSource::Clap(_) | InstrumentSource::Pack(_) => None,
            }
        };
        let tagged = |s: &[SampleSpec], len: u8, tag: SampleTag| {
            s.iter().take(len as usize).enumerate().filter(|(_, x)| x.tag == Some(tag)).fold(0u32, |m, (i, _)| m | 1 << i)
        };
        let mut layout = Self::empty();
        if let Some((_, s)) = samples(SubVoiceKind::Backbone) {
            layout = layout.with_backbone(s.len());
        }
        if let Some((sv, s)) = samples(SubVoiceKind::Hats) {
            if s.iter().any(|x| x.tag.is_some()) {
                let open = tagged(s, HATS_LEN, SampleTag::Open);
                let closed = low_bits(s.len().min(HATS_LEN as usize)) & !open;
                layout = layout.with_hat_masks(closed, open, sv.choke);
            } else {
                layout = layout.with_hats(s.len(), sv.choke);
            }
        }
        if let Some((_, s)) = samples(SubVoiceKind::Ride) {
            let crashes = tagged(s, RIDE_LEN, SampleTag::Crash) as u16;
            let rides = low_bits(s.len().min(RIDE_LEN as usize)) as u16 & !crashes;
            layout = layout.with_rides(rides, crashes);
        }
        if let Some((_, s)) = samples(SubVoiceKind::Shaker) {
            layout = layout.with_shakers(s.len());
        }
        if let Some((_, s)) = samples(SubVoiceKind::Perc) {
            layout = layout.with_perc(s.len());
        }
        if let Some((sv, s)) = samples(SubVoiceKind::Top) {
            let steps: Vec<u16> =
                s.iter().take(TOP_LEN as usize).map(|x| x.bars.unwrap_or(1).max(1) as u16 * STEPS_PER_BAR as u16).collect();
            layout = layout.with_tops(&steps, sv.choke);
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

    /// First closed hat target.
    pub fn hat_closed(&self) -> Option<u8> {
        (self.hats_closed != 0).then(|| HATS_BASE + self.hats_closed.trailing_zeros() as u8)
    }

    /// First open hat target.
    pub fn hat_open(&self) -> Option<u8> {
        (self.hats_open != 0).then(|| HATS_BASE + self.hats_open.trailing_zeros() as u8)
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
        let in_mask = |base: u8, len: u8, mask: u32| {
            target >= base && target < base + len && mask & 1 << (target - base) != 0
        };
        if Some(target) == self.kick {
            DrumClass::Kick
        } else if Some(target) == self.snare {
            DrumClass::Snare
        } else if Some(target) == self.ghost {
            DrumClass::Ghost
        } else if in_mask(HATS_BASE, HATS_LEN, self.hats_closed) {
            DrumClass::HatClosed
        } else if in_mask(HATS_BASE, HATS_LEN, self.hats_open) {
            DrumClass::HatOpen
        } else if in_mask(RIDE_BASE, RIDE_LEN, self.rides as u32) {
            DrumClass::Ride
        } else if in_mask(RIDE_BASE, RIDE_LEN, self.crashes as u32) {
            DrumClass::Crash
        } else if (SHAKER_BASE..SHAKER_BASE + self.shaker_count).contains(&target) {
            DrumClass::Shaker(target - SHAKER_BASE)
        } else if (PERC_BASE..PERC_BASE + self.perc_count).contains(&target) {
            DrumClass::Perc(target - PERC_BASE)
        } else if target >= BREAK_BASE && target < TOP_BASE && ((target - BREAK_BASE) as usize) < self.slice_count as usize {
            let i = target - BREAK_BASE;
            DrumClass::Slice(i, self.slice_types[i as usize])
        } else if (TOP_BASE..TOP_BASE + self.top_count).contains(&target) {
            DrumClass::Top(target - TOP_BASE)
        } else {
            DrumClass::Unknown
        }
    }

    /// The `BarCtx::drum_mask` for a rendered drum bar.
    ///
    /// Only kick and snare class hits count: backbone one-shots, and break
    /// slices of those types at [`MASK_SLICE_MIN_VEL`] or louder. Hats, ghosts,
    /// percussion, loops and ghosted slices leave the step 0, so the bass and
    /// lead see them as gaps. Encoding per step:
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
            // Nearest step: swing and timing drift (`form::Feel`) move hits a
            // few ticks off the grid in either direction.
            let step = ((ev.tick + TICKS_PER_STEP / 2) / TICKS_PER_STEP) as usize;
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
        assert_eq!(l.hat_closed(), Some(8));
        assert_eq!(l.hat_open(), Some(9));
        assert_eq!(l.hats_choke, 2);
        assert_eq!(l.perc_count, 1);
        assert_eq!(l.break_choke, 1);
        assert_eq!(l.slices(), [SliceType::Kick, SliceType::Hat, SliceType::Snare, SliceType::Ghost]);
        assert_eq!(l.slice_target(2), BREAK_BASE + 2);
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
        assert_eq!(l.class(10), DrumClass::Unknown);
        assert_eq!(l.class(PERC_BASE), DrumClass::Perc(0));
        assert_eq!(l.class(PERC_BASE + 1), DrumClass::Unknown);
        assert_eq!(l.class(BREAK_BASE), DrumClass::Slice(0, SliceType::Kick));
        assert_eq!(l.class(BREAK_BASE + 4), DrumClass::Slice(4, SliceType::Snare));
        assert_eq!(l.class(BREAK_BASE + 16), DrumClass::Unknown);
        let pools = DrumLayout::empty().with_rides(0b01, 0b10).with_shakers(2).with_tops(&[32, 64], 0);
        assert_eq!(pools.class(RIDE_BASE), DrumClass::Ride);
        assert_eq!(pools.class(RIDE_BASE + 1), DrumClass::Crash);
        assert_eq!(pools.class(SHAKER_BASE + 1), DrumClass::Shaker(1));
        assert_eq!(pools.class(TOP_BASE + 1), DrumClass::Top(1));
        assert_eq!(pools.top_choke, DEFAULT_TOP_CHOKE);
        assert_eq!(pools.class(TOP_BASE + 2), DrumClass::Unknown);
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
    fn voices_cover_the_target_space() {
        assert_eq!(DrumVoice::of_target(0), Some(DrumVoice::Kick));
        assert_eq!(DrumVoice::of_target(1), Some(DrumVoice::Snare));
        assert_eq!(DrumVoice::of_target(2), Some(DrumVoice::Snare));
        assert_eq!(DrumVoice::of_target(HATS_BASE + 31), Some(DrumVoice::Hats));
        assert_eq!(DrumVoice::of_target(RIDE_BASE), Some(DrumVoice::Ride));
        assert_eq!(DrumVoice::of_target(SHAKER_BASE), Some(DrumVoice::Shaker));
        assert_eq!(DrumVoice::of_target(PERC_BASE), Some(DrumVoice::Perc));
        assert_eq!(DrumVoice::of_target(BREAK_BASE + 63), Some(DrumVoice::Break));
        assert_eq!(DrumVoice::of_target(TOP_BASE), Some(DrumVoice::Top));
        assert_eq!(DrumVoice::of_target(TOP_BASE + TOP_LEN), None);
        for (i, v) in DrumVoice::ALL.iter().enumerate() {
            assert_eq!(v.bit(), 1 << i);
        }
        assert_eq!(target_range(SubVoiceKind::Top), (TOP_BASE, TOP_LEN));
    }

    #[test]
    fn tagged_pools_from_the_spec() {
        use crate::live::style::{InstrumentOption, SamplerSpec};
        let sample = |tag: Option<SampleTag>, bars: Option<u8>| SampleSpec {
            file: "x.wav".into(),
            root_key: None,
            choke: 0,
            slices: Vec::new(),
            zone: None,
            tag,
            bars,
            stream: false,
            gain_db: 0.0,
            lufs: None,
        };
        let sub = |voice, samples| SubVoiceSpec {
            voice,
            choke: 0,
            options: vec![InstrumentOption {
                name: "pool".into(),
                lufs_trim_db: 0.0,
                sampler: Some(SamplerSpec { samples }),
                clap: None,
                pack: None,
            }],
            default_option: 0,
        };
        let subs = vec![
            sub(SubVoiceKind::Hats, vec![sample(Some(SampleTag::Closed), None), sample(Some(SampleTag::Open), None), sample(Some(SampleTag::Closed), None)]),
            sub(SubVoiceKind::Ride, vec![sample(Some(SampleTag::Crash), None), sample(None, None)]),
            sub(SubVoiceKind::Top, vec![sample(None, Some(2)), sample(None, Some(4))]),
        ];
        let l = DrumLayout::from_sub_voices(&subs, |sv| sv.default_option);
        assert_eq!((l.hats_closed, l.hats_open), (0b101, 0b010));
        assert_eq!((l.rides, l.crashes), (0b10, 0b01));
        assert_eq!((l.top_count, l.top_steps[0], l.top_steps[1]), (2, 32, 64));
    }

    #[test]
    fn onset_mask_encoding() {
        let l = DrumLayout::standard();
        let events = [
            trigger(0, 0, 120),    // kick
            trigger(96, 1, 120),   // snare
            trigger(48, 8, 127),   // hat: not in the mask
            trigger(240, BREAK_BASE + 10, 100), // break slice 10 (kick)
            trigger(288, BREAK_BASE + 4, 90),   // break slice 4 (snare)
            trigger(300, 0, 0),                 // silent
            trigger(336, BREAK_BASE + 10, 60),  // ghosted kick slice: not in the mask
        ];
        let m = l.onset_mask(&events);
        assert!(m[0] >= MASK_KICK_MIN);
        assert!(m[4] > 0 && m[4] < MASK_KICK_MIN);
        assert_eq!(m[2], 0);
        assert!(m[10] >= MASK_KICK_MIN);
        assert!(m[12] > 0 && m[12] < m[4]);
        assert_eq!(m[14], 0);
        assert_eq!(m.iter().filter(|&&v| v > 0).count(), 4);
        // Off-grid hits count on their nearest step.
        assert!(l.onset_mask(&[trigger(239, 0, 120)])[10] >= MASK_KICK_MIN, "drifted a tick early");
        assert!(l.onset_mask(&[trigger(248, 0, 120)])[10] >= MASK_KICK_MIN, "swung late");
        // Full-velocity snare stays below the kick range, quiet kick inside it.
        assert!(l.onset_mask(&[trigger(0, 1, 127)])[0] < MASK_KICK_MIN);
        assert!(l.onset_mask(&[trigger(0, 0, 1)])[0] >= MASK_KICK_MIN);
    }
}
