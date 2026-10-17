//! Drum arrangement recipes: what a seed makes the drum lane play.
//!
//! A recipe is drawn from the bar's seeds and says:
//!
//! - **Voices**: which of ride, shaker, perc, top loop and break the seed
//!   uses (kick, snare and hats always). The others still render, muted
//!   until a pad enables them (the RT lane applies the mask, see
//!   [`DrumVoice`]).
//! - **Layers and their order**: at density 0 the lane plays its basic groove
//!   (the archetype's kick anchors, maybe leaner; the backbeat, maybe only
//!   every second one; offbeat hats). Raising the density brings in the
//!   layers one at a time, in a seeded order: the full kick and snare, ghosts,
//!   two hat upgrades, then the seed's extra voices and the break.
//! - **Patterns**: each voice plays one seeded pattern from a small
//!   vocabulary (a single ride on the 1, triplet rides through the second
//!   half, shaker 16ths, a top loop only in the last bar, …), not random
//!   steps.
//! - **Break roles per bar**: gaps (between kick and snare), layer (through,
//!   quieter, off the backbone) and tail (the second half of the phrase's
//!   last bar), combined per bar of the phrase.
//!
//! `mutate` nudges a bar's recipe: another pattern for one voice, a break
//! role flipped. Everything is a pure function of the seeds.

use crate::live::form::SeedClass;
use crate::live::rng;

use super::layout::DrumVoice;

/// Layers the density brings in, after the basic groove.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Every kick the archetype and the density reveal.
    Kick = 0,
    /// The full backbeat and extra snares.
    Snare = 1,
    Ghost = 2,
    /// Hats: the seed's pattern instead of offbeat 8ths.
    Hats2 = 3,
    /// Hats: 16ths and the open hat.
    Hats3 = 4,
    Ride = 5,
    Shaker = 6,
    Perc = 7,
    Top = 8,
    /// The break's full roles (before it: only the phrase tail).
    Break = 9,
}

pub const LAYERS: usize = 10;

impl Layer {
    pub const ALL: [Layer; LAYERS] = [
        Layer::Kick,
        Layer::Snare,
        Layer::Ghost,
        Layer::Hats2,
        Layer::Hats3,
        Layer::Ride,
        Layer::Shaker,
        Layer::Perc,
        Layer::Top,
        Layer::Break,
    ];

    /// The pad voice a layer belongs to.
    pub fn voice(self) -> DrumVoice {
        match self {
            Layer::Kick => DrumVoice::Kick,
            Layer::Snare | Layer::Ghost => DrumVoice::Snare,
            Layer::Hats2 | Layer::Hats3 => DrumVoice::Hats,
            Layer::Ride => DrumVoice::Ride,
            Layer::Shaker => DrumVoice::Shaker,
            Layer::Perc => DrumVoice::Perc,
            Layer::Top => DrumVoice::Top,
            Layer::Break => DrumVoice::Break,
        }
    }
}

/// Tick lists of the pattern vocabulary (96 ticks per beat; 32 = 8th triplet).
pub type Ticks = &'static [u16];

// MUSICAL: first draft, tune by ear. The pattern vocabulary.
/// Hats of the basic groove: offbeat 8ths.
pub const HATS_BASE: Ticks = &[48, 144, 240, 336];
/// Hat patterns of layer `Hats2`.
pub const HATS_PATTERNS: [Ticks; 5] = [
    &[0, 48, 96, 144, 192, 240, 288, 336],           // 8ths
    &[48, 144, 168, 240, 336, 360],                  // offbeats with 16th pickups
    &[48, 144, 240, 288, 320, 352],                  // offbeats, last beat in triplets
    &[48, 72, 144, 168, 240, 264, 336, 360],         // gallop
    &[0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352], // triplet 8ths
];
/// Open hat positions (layer `Hats3`).
pub const OPEN_HAT_TICKS: [u16; 3] = [144, 336, 240];
pub const RIDE_PATTERNS: [Ticks; 5] = [
    &[0],                                   // one ride on the 1
    &[0, 96, 192, 288],                     // quarters
    &[0, 192, 224, 256, 288, 320, 352],     // the 1, then triplets through the second half
    &[48, 144, 240, 336],                   // offbeat 8ths
    &[0, 192],                              // 1 and 3
];
pub const SHAKER_PATTERNS: [Ticks; 4] = [
    &[0, 24, 48, 72, 96, 120, 144, 168, 192, 216, 240, 264, 288, 312, 336, 360], // 16ths
    &[48, 144, 240, 336],                                                        // offbeats
    &[48, 72, 144, 168, 240, 264, 336, 360],                                     // pickups
    &[0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352],                    // triplets
];

/// Where the top loop plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopPattern {
    Full,
    SecondHalf,
    /// Only the phrase's last bar.
    LastBar,
    /// Bars 2 and 4 of the phrase.
    EveryOther,
    /// The offbeat 8th and the 16th after it.
    Offbeats,
}

pub const TOP_PATTERNS: [TopPattern; 5] =
    [TopPattern::Full, TopPattern::SecondHalf, TopPattern::LastBar, TopPattern::EveryOther, TopPattern::Offbeats];

impl TopPattern {
    /// Does the loop play `step` of `bar_in_phrase`?
    pub fn plays(self, bar_in_phrase: u8, step: usize) -> bool {
        match self {
            TopPattern::Full => true,
            TopPattern::SecondHalf => step >= 8,
            TopPattern::LastBar => bar_in_phrase == 3,
            TopPattern::EveryOther => bar_in_phrase % 2 == 1,
            TopPattern::Offbeats => matches!(step % 4, 2 | 3),
        }
    }
}

/// Break role bits.
pub mod break_role {
    /// Slices only on steps the backbone leaves free.
    pub const GAPS: u8 = 1 << 0;
    /// The second half of the phrase's last bar, chopped.
    pub const TAIL: u8 = 1 << 1;
    /// Through the bar, quieter, off the backbone.
    pub const LAYER: u8 = 1 << 2;
}

// MUSICAL: first draft, tune by ear.
/// Share of seeds whose basic groove keeps only the strongest kicks.
const KICK_LEAN_PROB: f32 = 0.4;
/// Share of seeds whose basic groove plays every second snare.
const SNARE_HALF_PROB: f32 = 0.35;
/// Share of seeds with an extra syncopated kick in the kick layer, and where.
const KICK_EXTRA_PROB: f32 = 0.5;
const KICK_EXTRA_STEPS: [u8; 5] = [3, 7, 11, 13, 15];
/// Extra voices (ride, shaker, perc, top) a seed uses: 2 or 3.
const EXTRA_VOICES: [DrumVoice; 4] = [DrumVoice::Ride, DrumVoice::Shaker, DrumVoice::Perc, DrumVoice::Top];
/// Share of seeds that use the break.
const BREAK_PROB: f32 = 0.8;
/// Break roles of the first three bars and their weights.
const BREAK_BAR_ROLES: [(u8, f32); 5] = [
    (break_role::GAPS, 3.0),
    (break_role::LAYER, 2.0),
    (break_role::GAPS | break_role::LAYER, 1.0),
    (0, 1.5),
    (break_role::TAIL, 0.5),
];
/// Chance the last bar adds the tail.
const BREAK_TAIL_PROB: f32 = 0.8;
/// Density span of the layers: the first enters here, the last at the end.
const ENTRY_RANGE: (f32, f32) = (0.1, 0.9);
/// Euclid percussion pulses a seed picks from.
const PERC_PULSES: (u32, u32) = (3, 6);
/// Chance per bar and unit of `mutate` that one pattern changes.
const MUTATE_PATTERN: f32 = 0.6;
/// Chance per bar and unit of `mutate` that a break role flips.
const MUTATE_BREAK: f32 = 0.5;

// RNG streams (recipe).
const S_VOICES: u32 = 0x400;
const S_ORDER: u32 = 0x401;
const S_LEAN: u32 = 0x402;
const S_HALF: u32 = 0x403;
const S_KICK_EXTRA: u32 = 0x404;
const S_HATS: u32 = 0x405;
const S_OPEN: u32 = 0x406;
const S_RIDE: u32 = 0x407;
const S_SHAKER: u32 = 0x408;
const S_TOP: u32 = 0x409;
const S_PERC: u32 = 0x40A;
const S_BREAK: u32 = 0x40B;
const S_MUT: u32 = 0x40C;
const S_MUT_PICK: u32 = 0x40D;

/// A seed's drum arrangement. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recipe {
    /// Voices on by default (bit per [`DrumVoice`]).
    pub voices: u8,
    /// Density where each layer enters, by [`Layer`] index.
    pub entry: [f32; LAYERS],
    pub kick_lean: bool,
    pub snare_half: bool,
    pub kick_extra: Option<u8>,
    pub hats: usize,
    pub open_hat: u16,
    pub ride: usize,
    pub shaker: usize,
    pub top: TopPattern,
    pub perc_pulses: u8,
    pub perc_rotation: u8,
    /// Break roles per bar of the phrase.
    pub breaks: [u8; 4],
}

impl Recipe {
    /// The recipe of a bar. `sound_seed` sets the voices and their order (it
    /// stays put through a lane's main bars), `seed` the patterns (A and B
    /// differ). `voice_allowed` removes voices the style turns off.
    pub fn new(sound_seed: u32, seed: u32, lane: u8, class: SeedClass, voice_allowed: u8) -> Self {
        let hs = |stream: u32, n: u32| rng::hash(sound_seed, lane, 0, n as u8, stream);
        let hp = |stream: u32, n: u32| rng::hash(seed, lane, 0, n as u8, stream);

        // Voices: kick, snare and hats always, then 2-3 of the extra voices.
        let mut voices = DrumVoice::Kick.bit() | DrumVoice::Snare.bit() | DrumVoice::Hats.bit();
        let mut extras = EXTRA_VOICES;
        shuffle(&mut extras, hs(S_VOICES, 0));
        let count = 2 + rng::below(hs(S_VOICES, 1), 2) as usize;
        for v in &extras[..count] {
            voices |= v.bit();
        }
        if rng::chance(hs(S_VOICES, 2), BREAK_PROB) {
            voices |= DrumVoice::Break.bit();
        }
        voices &= voice_allowed;
        // Every seed plays the break or percussion (user, 2026-10-17).
        let groove = DrumVoice::Break.bit() | DrumVoice::Perc.bit();
        if voices & groove == 0 {
            let pick = if rng::chance(hs(S_VOICES, 3), 0.5) { DrumVoice::Break } else { DrumVoice::Perc };
            let other = if pick == DrumVoice::Break { DrumVoice::Perc } else { DrumVoice::Break };
            voices |= [pick, other].into_iter().map(|v| v.bit()).find(|b| voice_allowed & b != 0).unwrap_or(0);
        }

        // Layer order: kick and snare first (either way round), then the
        // rest shuffled. Layers of voices the seed leaves out render from
        // density 0, so a pad that enables one hears its whole pattern.
        let mut order = [
            Layer::Kick,
            Layer::Snare,
            Layer::Ghost,
            Layer::Hats2,
            Layer::Hats3,
            Layer::Ride,
            Layer::Shaker,
            Layer::Perc,
            Layer::Top,
            Layer::Break,
        ];
        if rng::chance(hs(S_ORDER, 1), 0.5) {
            order.swap(0, 1);
        }
        shuffle(&mut order[2..], hs(S_ORDER, 0));
        // Hats3 after Hats2.
        let h2 = order.iter().position(|&l| l == Layer::Hats2).unwrap_or(0);
        let h3 = order.iter().position(|&l| l == Layer::Hats3).unwrap_or(0);
        if h3 < h2 {
            order.swap(h2, h3);
        }
        let on_count = order.iter().filter(|l| voices & l.voice().bit() != 0).count();
        let mut entry = [0.0f32; LAYERS];
        for (i, l) in order.iter().filter(|l| voices & l.voice().bit() != 0).enumerate() {
            let t = if on_count > 1 { i as f32 / (on_count - 1) as f32 } else { 0.0 };
            entry[*l as usize] = ENTRY_RANGE.0 + (ENTRY_RANGE.1 - ENTRY_RANGE.0) * t;
        }

        // Break roles per bar of the phrase.
        let mut weights = [0.0f32; BREAK_BAR_ROLES.len()];
        for (w, r) in weights.iter_mut().zip(&BREAK_BAR_ROLES) {
            *w = r.1;
        }
        let mut breaks = [0u8; 4];
        for (bar, roles) in breaks.iter_mut().enumerate() {
            let pick = rng::weighted_pick(hp(S_BREAK, bar as u32), &weights).unwrap_or(0);
            *roles = BREAK_BAR_ROLES[pick].0;
            if bar == 3 && rng::chance(hp(S_BREAK, 10), BREAK_TAIL_PROB) {
                *roles |= break_role::TAIL;
            }
        }
        // Filler bars lean on the break.
        if class == SeedClass::Filler {
            for roles in &mut breaks {
                *roles |= break_role::GAPS;
            }
        }

        Self {
            voices,
            entry,
            kick_lean: rng::chance(hs(S_LEAN, 0), KICK_LEAN_PROB),
            snare_half: rng::chance(hs(S_HALF, 0), SNARE_HALF_PROB),
            kick_extra: rng::chance(hp(S_KICK_EXTRA, 0), KICK_EXTRA_PROB)
                .then(|| KICK_EXTRA_STEPS[rng::below(hp(S_KICK_EXTRA, 1), KICK_EXTRA_STEPS.len() as u32) as usize]),
            hats: rng::below(hp(S_HATS, 0), HATS_PATTERNS.len() as u32) as usize,
            open_hat: OPEN_HAT_TICKS[rng::below(hp(S_OPEN, 0), OPEN_HAT_TICKS.len() as u32) as usize],
            ride: rng::below(hp(S_RIDE, 0), RIDE_PATTERNS.len() as u32) as usize,
            shaker: rng::below(hp(S_SHAKER, 0), SHAKER_PATTERNS.len() as u32) as usize,
            top: TOP_PATTERNS[rng::below(hp(S_TOP, 0), TOP_PATTERNS.len() as u32) as usize],
            perc_pulses: (PERC_PULSES.0 + rng::below(hp(S_PERC, 0), PERC_PULSES.1 - PERC_PULSES.0 + 1)) as u8,
            perc_rotation: rng::below(hp(S_PERC, 1), 16) as u8,
            breaks,
        }
    }

    /// `mutate` for one bar: maybe another pattern for one voice and a
    /// flipped break role. `h` is the bar's random bits.
    pub fn mutated(mut self, amount: f32, bar_in_phrase: u8, h: impl Fn(u32, u32) -> u64) -> Self {
        if amount <= 0.0 {
            return self;
        }
        if rng::chance(h(S_MUT, 0), amount * MUTATE_PATTERN) {
            let r = h(S_MUT_PICK, 0);
            match rng::below(r, 5) {
                0 => self.hats = (self.hats + 1 + rng::below(rng::derive(r, 1), 3) as usize) % HATS_PATTERNS.len(),
                1 => self.ride = (self.ride + 1 + rng::below(rng::derive(r, 1), 3) as usize) % RIDE_PATTERNS.len(),
                2 => self.shaker = (self.shaker + 1 + rng::below(rng::derive(r, 1), 2) as usize) % SHAKER_PATTERNS.len(),
                3 => self.perc_rotation = (self.perc_rotation + 1 + rng::below(rng::derive(r, 1), 4) as u8) % 16,
                _ => self.kick_extra = Some(KICK_EXTRA_STEPS[rng::below(rng::derive(r, 1), KICK_EXTRA_STEPS.len() as u32) as usize]),
            }
        }
        if rng::chance(h(S_MUT, 1), amount * MUTATE_BREAK) {
            let roles = [break_role::GAPS, break_role::LAYER, break_role::TAIL];
            let flip = roles[rng::below(h(S_MUT_PICK, 1), roles.len() as u32) as usize];
            self.breaks[bar_in_phrase as usize & 3] ^= flip;
        }
        self
    }

    /// Has `layer` entered at `density`?
    pub fn entered(&self, layer: Layer, density: f32) -> bool {
        density >= self.entry[layer as usize]
    }

    /// Density inside a layer: 0 at its entry, 1 at full density. Negative
    /// before the entry.
    pub fn layer_density(&self, layer: Layer, density: f32) -> f32 {
        let e = self.entry[layer as usize];
        if density < e {
            -1.0
        } else if e >= 1.0 {
            1.0
        } else {
            ((density - e) / (1.0 - e)).clamp(0.0, 1.0)
        }
    }
}

/// Fisher-Yates with seeded bits.
fn shuffle<T>(items: &mut [T], h: u64) {
    for i in (1..items.len()).rev() {
        let j = rng::below(rng::derive(h, i as u32), i as u32 + 1) as usize;
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_pick_different_arrangements() {
        let recipes: Vec<Recipe> = (0..200).map(|s| Recipe::new(s, s, 0, SeedClass::Main, 0xFF)).collect();
        for r in &recipes {
            assert_eq!(r.voices & 0b111, 0b111, "kick, snare and hats always");
            let extras = EXTRA_VOICES.iter().filter(|v| r.voices & v.bit() != 0).count();
            // 2-3, one more when the seed needs percussion (no break).
            assert!((2..=4).contains(&extras), "{extras}");
            assert!(r.voices & (DrumVoice::Break.bit() | DrumVoice::Perc.bit()) != 0, "break or percussion");
            assert!(r.entry[Layer::Kick as usize] < 0.3 && r.entry[Layer::Snare as usize] < 0.3, "backbone first");
            assert!(r.entry[Layer::Hats2 as usize] <= r.entry[Layer::Hats3 as usize]);
        }
        let distinct = |f: fn(&Recipe) -> usize| recipes.iter().map(f).collect::<std::collections::HashSet<_>>().len();
        assert_eq!(distinct(|r| r.hats), HATS_PATTERNS.len());
        assert_eq!(distinct(|r| r.ride), RIDE_PATTERNS.len());
        assert!(distinct(|r| r.voices as usize) > 6);
        assert!(recipes.iter().any(|r| r.snare_half) && recipes.iter().any(|r| !r.snare_half));
    }

    #[test]
    fn seed_off_voices_render_from_zero_and_style_can_remove_voices() {
        for s in 0..50 {
            let r = Recipe::new(s, s, 0, SeedClass::Main, 0xFF);
            for l in Layer::ALL {
                if r.voices & l.voice().bit() == 0 {
                    assert_eq!(r.entry[l as usize], 0.0, "{l:?} off by seed renders for its pad");
                }
            }
            let no_top = Recipe::new(s, s, 0, SeedClass::Main, !DrumVoice::Top.bit());
            assert_eq!(no_top.voices & DrumVoice::Top.bit(), 0);
        }
    }

    #[test]
    fn layers_enter_in_order_with_density() {
        let r = Recipe::new(7, 7, 0, SeedClass::Main, 0xFF);
        let entered = |d: f32| Layer::ALL.iter().filter(|&&l| r.voices & l.voice().bit() != 0 && r.entered(l, d)).count();
        assert_eq!(entered(0.0), 0);
        assert!(entered(0.5) > 1);
        assert_eq!(entered(1.0), Layer::ALL.iter().filter(|l| r.voices & l.voice().bit() != 0).count());
        assert!(r.layer_density(Layer::Kick, 0.0) < 0.0);
        assert_eq!(r.layer_density(Layer::Kick, 1.0), 1.0);
    }

    #[test]
    fn mutate_changes_bars_a_little() {
        let r = Recipe::new(3, 3, 0, SeedClass::Main, 0xFF);
        assert_eq!(r.mutated(0.0, 1, |s, n| rng::hash(1, 0, 1, n as u8, s)), r);
        let changed = (0..100u32)
            .filter(|&b| r.mutated(1.0, 1, |s, n| rng::hash(b, 0, 1, n as u8, s)) != r)
            .count();
        assert!((40..100).contains(&changed), "{changed}");
    }

    #[test]
    fn top_patterns() {
        assert!(TopPattern::LastBar.plays(3, 0) && !TopPattern::LastBar.plays(2, 0));
        assert!(TopPattern::SecondHalf.plays(0, 8) && !TopPattern::SecondHalf.plays(0, 7));
        assert!(TopPattern::Offbeats.plays(0, 2) && TopPattern::Offbeats.plays(0, 3) && !TopPattern::Offbeats.plays(0, 4));
    }
}
