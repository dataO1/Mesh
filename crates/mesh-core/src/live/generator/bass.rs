//! Bass generator (design §6.3 rule A6; research §7.1.2, §7.1.7).
//!
//! - **Rhythm** comes from the gaps in the drums: metric importance, raised
//!   on steps the drum backbone leaves empty. Kick steps are either avoided,
//!   left alone, or doubled (`kick_mode`). `density` reveals steps in order of
//!   importance and `complexity` shifts the importance towards syncopation.
//! - **Pitch** is a root-heavy pool of chord-relative scale degrees (root,
//!   octave, 5th, 7th, 3rd — b7 and b3 in a minor key) resolved against the
//!   bar's chord, so the line follows the progression and any key change.
//! - **Articulation**: accents, gate length and slides (`TIE` + glide).
//! - **Phrase shape**: bars 0-2 repeat, bar 3 re-draws some steps and ends
//!   on a turnaround note.
//!
//! The idiom family uses the same generator with the rhythm importance read
//! from a style rhythm cell instead of the metric tables.
//!
//! Performance parameters, in order: `density`, `complexity`, `variation`,
//! `kick_mode` (< 1/3 avoid the kick, < 2/3 ignore it, else double it),
//! `register` (octave down / base / up), `slide`, `accent`, `gate`.
//!
//! Construction options: `rule_a3` (phrase shape), `rule_a5` (velocity
//! tiers), `rule_a6` (gap-following and root weighting). All default to 1.

use std::collections::BTreeMap;

use super::blocks::{
    lerp, level_unit, note, option, param, reveals, step_tick, velocity, LevelMap, PhraseRng, Tier,
};
use super::layout::MASK_KICK_MIN;
use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::live::event::{flags, BarEvents, LiveEvent};
use crate::live::harmony::SCALE_LEN;
use crate::live::rng;
use crate::live::{STEPS_PER_BAR, TICKS_PER_STEP};
use crate::music::MusicalKey;

pub const PARAM_NAMES: [&str; 8] =
    ["density", "complexity", "variation", "kick_mode", "register", "slide", "accent", "gate"];
const P_DENSITY: usize = 0;
const P_COMPLEXITY: usize = 1;
const P_VARIATION: usize = 2;
const P_KICK_MODE: usize = 3;
const P_REGISTER: usize = 4;
const P_SLIDE: usize = 5;
const P_ACCENT: usize = 6;
const P_GATE: usize = 7;

// MUSICAL: first draft, tune by ear.
pub const PARAM_DEFAULTS: [f32; 8] = [0.5, 0.4, 0.5, 0.0, 0.5, 0.5, 0.5, 0.6];

const S_CELL: u32 = 0x200;
const S_RHYTHM: u32 = 0x201;
const S_PITCH: u32 = 0x202;
const S_ACCENT: u32 = 0x203;
const S_SLIDE: u32 = 0x204;
const S_VEL: u32 = 0x205;
const S_TURN: u32 = 0x206;

// MUSICAL: first draft, tune by ear. Rhythm importance per step at
// complexity 0 (on the beat) and at complexity 1 (syncopated).
const BASS_SIMPLE: [f32; 16] =
    [1.0, 0.1, 0.3, 0.15, 0.6, 0.1, 0.45, 0.2, 0.8, 0.1, 0.35, 0.3, 0.55, 0.1, 0.45, 0.25];
const BASS_SYNC: [f32; 16] =
    [0.9, 0.2, 0.4, 0.7, 0.3, 0.2, 0.75, 0.55, 0.4, 0.3, 0.6, 0.7, 0.3, 0.3, 0.75, 0.5];
/// Importance added on steps the drum backbone leaves empty (A6).
const GAP_BONUS: f32 = 0.1;
/// Importance taken off steps with a snare.
const ONSET_PENALTY: f32 = 0.15;
/// Rhythm randomness at complexity 1.
const BASS_CHAOS_MAX: f32 = 0.35;

// MUSICAL: first draft, tune by ear. Pitch pool as (chord-relative scale
// degree, weight): root, octave, 5th, 7th, 3rd (research §7.1.2: root-heavy).
const BASS_POOL: [(i32, f32); 5] = [(0, 6.0), (7, 2.0), (4, 2.0), (6, 1.0), (2, 1.0)];
/// Weight factor of the non-root degrees at complexity 0 and 1.
const POOL_SPREAD: (f32, f32) = (0.4, 1.6);
/// Degrees a phrase-end turnaround note may take (5th, 7th).
const TURNAROUND_DEGREES: [i32; 2] = [4, 6];
/// Steps tried for the turnaround note, last first.
const TURNAROUND_STEPS: [usize; 3] = [15, 14, 13];

// MUSICAL: first draft, tune by ear. Register and articulation.
/// Lowest MIDI key of the key's root at the base register (E1).
const BASS_ROOT_MIN: i32 = 28;
/// Notes more than this many semitones above the root drop an octave.
const BASS_SPAN: i32 = 14;
/// Accent / slide probability at parameter value 1 (the defaults of 0.5 give
/// the acid-style 30 % accents and 10 % slides).
const ACCENT_PROB_MAX: f32 = 0.6;
const SLIDE_PROB_MAX: f32 = 0.2;
/// Filter offset of an accented note.
const ACCENT_CUTOFF: i8 = 24;
/// A slide needs the previous note at most this many steps back.
const SLIDE_MAX_GAP_STEPS: usize = 4;
/// Glide time byte (log scale, about 60 ms).
const SLIDE_GLIDE: u8 = 138;
/// Ticks a slid-from note overlaps the next one so the tie finds it held.
const TIE_OVERLAP_TICKS: u16 = 2;
/// Gate as a share of the distance to the next note at `gate` 0.
const GATE_MIN_FRAC: f32 = 0.3;
const GATE_MIN_TICKS: f32 = 6.0;
/// Longest gate in steps.
const GATE_MAX_STEPS: usize = 8;

/// How the bass treats steps with a kick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KickMode {
    Avoid,
    Free,
    Double,
}

impl KickMode {
    pub fn from_param(v: f32) -> Self {
        if v < 1.0 / 3.0 {
            KickMode::Avoid
        } else if v < 2.0 / 3.0 {
            KickMode::Free
        } else {
            KickMode::Double
        }
    }
}

/// Rule switches and scales, fixed at construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BassRules {
    /// A3: bars 0-2 repeat, bar 3 varies.
    pub phrase: bool,
    /// A5: velocity tier depth.
    pub tiers: f32,
    /// A6: strength of gap-following (rhythm).
    pub gaps: f32,
    /// A6: share of notes drawn from the root-heavy pool (the rest are any
    /// scale degree).
    pub root: f32,
}

impl Default for BassRules {
    fn default() -> Self {
        Self { phrase: true, tiers: 1.0, gaps: 1.0, root: 1.0 }
    }
}

impl BassRules {
    pub fn from_spec(params: &BTreeMap<String, f32>) -> Self {
        let a6 = option(params, "rule_a6", 1.0);
        Self {
            phrase: option(params, "rule_a3", 1.0) >= 0.5,
            tiers: option(params, "rule_a5", 1.0),
            gaps: a6,
            root: a6,
        }
    }
}

/// Octave (for `HarmonyCtx::resolve`) that puts the key's root at or just
/// above `min_midi`.
pub(super) fn root_octave(key: MusicalKey, min_midi: i32) -> i32 {
    (min_midi - key.root as i32 + 11).div_euclid(12) - 1
}

/// Register parameter as an octave shift: −1, 0 or +1.
pub(super) fn register_shift(v: f32) -> i32 {
    (v * 2.0).round() as i32 - 1
}

/// Bass generator. See the module docs.
pub struct BassGenerator {
    defaults: [f32; MAX_GEN_PARAMS],
    rules: BassRules,
    /// Idiom rhythm cells. Empty = procedural rhythm.
    cells: Vec<LevelMap>,
}

impl BassGenerator {
    pub fn new(defaults: [f32; MAX_GEN_PARAMS], rules: BassRules, cells: Vec<LevelMap>) -> Self {
        Self { defaults, rules, cells }
    }

    /// Draw a chord-relative degree from the pool.
    fn pick_degree(&self, h: u64, complexity: f32) -> i32 {
        if !rng::chance(rng::derive(h, 1), self.rules.root) {
            return rng::below(h, SCALE_LEN as u32) as i32;
        }
        let spread = lerp(POOL_SPREAD.0, POOL_SPREAD.1, complexity);
        let mut weights = [0.0f32; BASS_POOL.len()];
        for (i, &(degree, w)) in BASS_POOL.iter().enumerate() {
            weights[i] = if degree == 0 { w } else { w * spread };
        }
        BASS_POOL[rng::weighted_pick(h, &weights).unwrap_or(0)].0
    }
}

impl LaneGenerator for BassGenerator {
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let density = param(ctx, P_DENSITY);
        if density <= 0.0 {
            return;
        }
        let complexity = param(ctx, P_COMPLEXITY);
        let variation = param(ctx, P_VARIATION);
        let mode = KickMode::from_param(param(ctx, P_KICK_MODE));
        let rng = PhraseRng::new(ctx, variation, self.rules.phrase);
        let cell = (!self.cells.is_empty())
            .then(|| &self.cells[rng::below(rng.base.at(0, S_CELL), self.cells.len() as u32) as usize]);
        let is_kick = |s: usize| ctx.drum_mask[s] >= MASK_KICK_MIN;

        // Rhythm.
        let mut onsets = 0u16;
        for s in 0..STEPS_PER_BAR {
            let kick = is_kick(s);
            if kick && mode == KickMode::Avoid {
                continue;
            }
            let mut importance = match cell {
                Some(c) => level_unit(c.at(ctx.bar_in_phrase, s)),
                None => {
                    let gap = if ctx.drum_mask[s] == 0 {
                        GAP_BONUS
                    } else if kick {
                        0.0
                    } else {
                        -ONSET_PENALTY
                    };
                    lerp(BASS_SIMPLE[s], BASS_SYNC[s], complexity) + self.rules.gaps * gap
                }
            };
            if kick && mode == KickMode::Double {
                importance = 1.0;
            }
            if importance > 0.0
                && reveals(importance, density, complexity * BASS_CHAOS_MAX, rng.unit(s as u8, S_RHYTHM))
            {
                onsets |= 1 << s;
            }
        }

        // Phrase-end turnaround: one more note on the last free step, or, in
        // a full bar, a new pitch on the last note.
        let mut turnaround = None;
        if rng.is_variation_bar() {
            turnaround = TURNAROUND_STEPS
                .iter()
                .copied()
                .find(|&s| onsets & (1 << s) == 0 && !(mode == KickMode::Avoid && is_kick(s)));
            match turnaround {
                Some(s) => onsets |= 1 << s,
                None if onsets != 0 => turnaround = Some(15 - onsets.leading_zeros() as usize),
                None => {}
            }
        }

        // Pitch and articulation.
        let octave = root_octave(ctx.harmony.key, BASS_ROOT_MIN) + register_shift(param(ctx, P_REGISTER));
        let ceiling = ctx.harmony.resolve_in_key(0, octave) as i32 + BASS_SPAN;
        let gate = param(ctx, P_GATE);
        let (slide, accent) = (param(ctx, P_SLIDE), param(ctx, P_ACCENT));
        let mut events = [LiveEvent::default(); STEPS_PER_BAR];
        let mut n = 0usize;
        // Step and degree of the previous note.
        let mut prev: Option<(usize, i32)> = None;
        for s in 0..STEPS_PER_BAR {
            if onsets & (1 << s) == 0 {
                continue;
            }
            let su = s as u8;
            let first = prev.is_none() && self.rules.root >= 0.5;
            let mut degree = if first { 0 } else { self.pick_degree(rng.at(su, S_PITCH), complexity) };
            if turnaround == Some(s) {
                // Differ from what the repeating bars play on this step.
                let repeated = if first { 0 } else { self.pick_degree(rng.base.at(su, S_PITCH), complexity) };
                let i = rng::below(rng.bar.at(su, S_TURN), TURNAROUND_DEGREES.len() as u32) as usize;
                degree = if TURNAROUND_DEGREES[i] == repeated {
                    TURNAROUND_DEGREES[(i + 1) % TURNAROUND_DEGREES.len()]
                } else {
                    TURNAROUND_DEGREES[i]
                };
            }
            let mut key = ctx.harmony.resolve(degree, octave) as i32;
            while key > ceiling {
                key -= 12;
            }

            let next = (s + 1..STEPS_PER_BAR).find(|&t| onsets & (1 << t) != 0).unwrap_or(STEPS_PER_BAR);
            let span = ((next - s).min(GATE_MAX_STEPS) as u16 * TICKS_PER_STEP) as f32;
            let gate_ticks = (span * lerp(GATE_MIN_FRAC, 1.0, gate)).max(GATE_MIN_TICKS).round() as u16;

            let accented = rng.chance(su, S_ACCENT, accent * ACCENT_PROB_MAX);
            let tier = if accented { Tier::Accent } else { Tier::Normal };
            let vel = velocity(tier, rng.unit(su, S_VEL), self.rules.tiers);
            let mut ev = note(step_tick(s), key.clamp(0, 127) as u8, vel, gate_ticks, s as u16 + 1);
            if accented {
                ev.flags |= flags::ACCENT;
                ev.cutoff = ACCENT_CUTOFF;
            }
            if let Some((prev_step, prev_degree)) = prev {
                if s - prev_step <= SLIDE_MAX_GAP_STEPS
                    && degree != prev_degree
                    && rng.chance(su, S_SLIDE, slide * SLIDE_PROB_MAX)
                {
                    ev.flags |= flags::TIE;
                    ev.glide = SLIDE_GLIDE;
                    // Hold the previous note into this one.
                    events[n - 1].gate = (s - prev_step) as u16 * TICKS_PER_STEP + TIE_OVERLAP_TICKS;
                }
            }
            events[n] = ev;
            n += 1;
            prev = Some((s, degree));
        }
        for ev in &events[..n] {
            out.push(*ev);
        }
    }

    fn param_names(&self) -> &'static [&'static str] {
        &PARAM_NAMES
    }

    fn param_defaults(&self) -> [f32; MAX_GEN_PARAMS] {
        self.defaults
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::event::EventKind;
    use crate::live::generator::test_util::{assert_well_formed, ctx, notes, render};
    use crate::live::harmony::scale_for;
    use crate::live::TICKS_PER_BAR;

    /// Two-step backbone: kicks on steps 0 and 10, snares on 4 and 12.
    const TWO_STEP: [u8; 16] = [120, 0, 0, 0, 80, 0, 0, 0, 0, 0, 118, 0, 80, 0, 0, 0];
    const EMPTY: [u8; 16] = [0; 16];

    fn generator() -> BassGenerator {
        BassGenerator::new(PARAM_DEFAULTS, BassRules::default(), Vec::new())
    }

    fn params(edit: impl FnOnce(&mut [f32; 8])) -> [f32; 8] {
        let mut p = PARAM_DEFAULTS;
        edit(&mut p);
        p
    }

    fn bar(g: &BassGenerator, seed: u32, bar: u8, p: &[f32; 8], mask: &[u8; 16]) -> Vec<LiveEvent> {
        let mut c = ctx(seed, bar, p, mask);
        c.lane = 1;
        render(g, &c).as_slice().to_vec()
    }

    fn steps(ev: &[LiveEvent]) -> Vec<usize> {
        ev.iter().map(|e| (e.tick / TICKS_PER_STEP) as usize).collect()
    }

    #[test]
    fn deterministic_and_well_formed() {
        let g = generator();
        for seed in 0..60u32 {
            for b in 0..4u8 {
                let p = params(|p| {
                    p[P_DENSITY] = (seed % 5) as f32 / 4.0;
                    p[P_COMPLEXITY] = (seed % 4) as f32 / 3.0;
                    p[P_KICK_MODE] = (seed % 3) as f32 / 2.0;
                    p[P_REGISTER] = (seed % 3) as f32 / 2.0;
                    p[P_SLIDE] = 1.0;
                    p[P_GATE] = (seed % 2) as f32;
                });
                let mut c = ctx(seed, b, &p, &TWO_STEP);
                c.lane = 1;
                let one = render(&g, &c);
                assert_eq!(one.as_slice(), render(&g, &c).as_slice());
                assert_well_formed(&one);
                for e in one.as_slice() {
                    assert_eq!(e.kind, EventKind::NoteOn);
                    assert!(e.gate > 0 && e.note_id > 0);
                }
                // One note per step at most, ids unique.
                let mut ids: Vec<u16> = one.as_slice().iter().map(|e| e.note_id).collect();
                ids.dedup();
                assert_eq!(ids.len(), one.len());
            }
        }
    }

    #[test]
    fn notes_are_in_scale_in_a_bass_register() {
        let g = generator();
        for root in 0..12u8 {
            for minor in [true, false] {
                let key = MusicalKey::new(root, minor);
                for chord in 0..7u8 {
                    let p = params(|p| p[P_COMPLEXITY] = 1.0);
                    let mut c = ctx(root as u32 * 7 + chord as u32, 3, &p, &TWO_STEP);
                    c.lane = 1;
                    c.harmony.key = key;
                    c.harmony.chord_degree = chord;
                    for e in render(&g, &c).as_slice() {
                        let pc = (e.key as i32 - root as i32).rem_euclid(12) as u8;
                        assert!(scale_for(key).contains(&pc), "{} not in {key}", e.key);
                        assert!((28..=28 + 11 + BASS_SPAN as u8).contains(&e.key), "key {} out of register", e.key);
                    }
                }
            }
        }
    }

    #[test]
    fn register_shifts_by_octaves() {
        let g = generator();
        let at = |r: f32| bar(&g, 5, 0, &params(|p| p[P_REGISTER] = r), &TWO_STEP);
        let (low, mid, high) = (at(0.0), at(0.5), at(1.0));
        assert_eq!(steps(&low), steps(&mid));
        for ((l, m), h) in low.iter().zip(&mid).zip(&high) {
            assert_eq!(m.key - l.key, 12);
            assert_eq!(h.key - m.key, 12);
        }
    }

    #[test]
    fn root_heavy() {
        let g = generator();
        let (mut root, mut total) = (0, 0);
        for seed in 0..200 {
            for e in bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP) {
                total += 1;
                // A minor, chord on the root: pitch class A.
                root += (e.key % 12 == 9) as usize;
            }
        }
        let share = root as f32 / total as f32;
        assert!(total > 400);
        assert!(share > 0.55, "root share {share}");
        // The first note of every bar is the root.
        for seed in 0..50 {
            assert_eq!(bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP)[0].key % 12, 9);
        }
    }

    #[test]
    fn root_rule_off_spreads_pitches() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { root: 0.0, ..BassRules::default() }, Vec::new());
        let (mut root, mut total) = (0, 0);
        for seed in 0..200 {
            for e in bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP) {
                total += 1;
                root += (e.key % 12 == 9) as usize;
            }
        }
        assert!((root as f32 / total as f32) < 0.3);
    }

    #[test]
    fn follows_the_chord() {
        let g = generator();
        let mut c = ctx(11, 0, &PARAM_DEFAULTS, &TWO_STEP);
        c.lane = 1;
        let on_root = render(&g, &c);
        c.harmony.chord_degree = 5; // VI in A minor = F
        let on_six = render(&g, &c);
        assert_eq!(steps(on_root.as_slice()), steps(on_six.as_slice()));
        assert_eq!(on_root.as_slice()[0].key % 12, 9);
        assert_eq!(on_six.as_slice()[0].key % 12, 5);
    }

    #[test]
    fn key_change_keeps_the_rhythm() {
        let g = generator();
        for seed in 0..30 {
            let p = params(|p| p[P_SLIDE] = 1.0);
            let mut c = ctx(seed, 3, &p, &TWO_STEP);
            c.lane = 1;
            let am = render(&g, &c);
            c.harmony.key = MusicalKey::new(4, true); // E minor
            let em = render(&g, &c);
            assert_eq!(am.len(), em.len());
            let mut moved = false;
            for (a, e) in am.as_slice().iter().zip(em.as_slice()) {
                assert_eq!((a.tick, a.gate, a.vel, a.flags, a.note_id), (e.tick, e.gate, e.vel, e.flags, e.note_id));
                // Same degree a fourth lower or a fifth higher.
                let shift = (e.key as i32 - a.key as i32).rem_euclid(12);
                assert_eq!(shift, 7);
                moved |= a.key != e.key;
            }
            assert!(moved);
        }
    }

    #[test]
    fn kick_modes() {
        let g = generator();
        let kicks = [0usize, 10];
        for seed in 0..100 {
            for b in 0..4 {
                for density in [0.3, 0.6, 1.0] {
                    let avoid = params(|p| p[P_DENSITY] = density);
                    let s = steps(&bar(&g, seed, b, &avoid, &TWO_STEP));
                    assert!(kicks.iter().all(|k| !s.contains(k)), "co-onset in avoid mode (seed {seed})");
                    let double = params(|p| {
                        p[P_DENSITY] = density;
                        p[P_KICK_MODE] = 1.0;
                    });
                    let s = steps(&bar(&g, seed, b, &double, &TWO_STEP));
                    assert!(kicks.iter().all(|k| s.contains(k)), "kick not doubled (seed {seed})");
                }
            }
        }
        // Free mode: the downbeat is simply the most important step.
        let free = params(|p| p[P_KICK_MODE] = 0.5);
        assert!(steps(&bar(&g, 1, 0, &free, &TWO_STEP)).contains(&0));
    }

    #[test]
    fn prefers_drum_gaps() {
        // Snares on every odd step: with gap-following, notes move off them.
        let busy: [u8; 16] = std::array::from_fn(|s| if s % 2 == 1 { 80 } else { 0 });
        let count_on_odd = |g: &BassGenerator| {
            (0..200)
                .flat_map(|seed| steps(&bar(g, seed, 0, &params(|p| p[P_COMPLEXITY] = 1.0), &busy)))
                .filter(|s| s % 2 == 1)
                .count()
        };
        let follow = generator();
        let ignore = BassGenerator::new(PARAM_DEFAULTS, BassRules { gaps: 0.0, ..BassRules::default() }, Vec::new());
        assert!(count_on_odd(&follow) < count_on_odd(&ignore));
    }

    #[test]
    fn density_is_monotonic() {
        let g = generator();
        for seed in 0..30 {
            let mut prev: Vec<usize> = Vec::new();
            for d in 0..=10 {
                let now = steps(&bar(&g, seed, 0, &params(|p| p[P_DENSITY] = d as f32 / 10.0), &TWO_STEP));
                assert!(prev.iter().all(|s| now.contains(s)));
                prev = now;
            }
            assert!(prev.len() >= 8);
        }
        assert!(bar(&g, 1, 0, &params(|p| p[P_DENSITY] = 0.0), &TWO_STEP).is_empty());
    }

    #[test]
    fn phrase_shape() {
        let g = generator();
        for seed in 0..60 {
            for density in [0.2, 0.5, 1.0] {
                let p = params(|p| p[P_DENSITY] = density);
                let b0 = bar(&g, seed, 0, &p, &TWO_STEP);
                assert_eq!(b0, bar(&g, seed, 1, &p, &TWO_STEP));
                assert_eq!(b0, bar(&g, seed, 2, &p, &TWO_STEP));
                assert_ne!(b0, bar(&g, seed, 3, &p, &TWO_STEP), "seed {seed} density {density}");
                let still = params(|p| {
                    p[P_DENSITY] = density;
                    p[P_VARIATION] = 0.0;
                });
                assert_eq!(bar(&g, seed, 0, &still, &TWO_STEP), bar(&g, seed, 3, &still, &TWO_STEP));
            }
        }
    }

    #[test]
    fn slides_tie_into_the_next_note() {
        let g = generator();
        let p = params(|p| {
            p[P_SLIDE] = 1.0;
            p[P_DENSITY] = 0.8;
            p[P_GATE] = 0.0;
        });
        let mut ties = 0;
        for seed in 0..100 {
            let ev = bar(&g, seed, 0, &p, &EMPTY);
            for w in ev.windows(2) {
                if w[1].flags & flags::TIE != 0 {
                    ties += 1;
                    assert!(w[1].glide > 0);
                    assert_ne!(w[0].key, w[1].key);
                    // The previous note is still held when the tie arrives.
                    assert!(w[0].tick + w[0].gate > w[1].tick || w[0].tick + w[0].gate == TICKS_PER_BAR);
                }
            }
            assert!(ev[0].flags & flags::TIE == 0);
        }
        assert!(ties > 20);
        let none = params(|p| p[P_SLIDE] = 0.0);
        assert!((0..50).all(|seed| bar(&g, seed, 0, &none, &EMPTY).iter().all(|e| e.flags & flags::TIE == 0)));
    }

    #[test]
    fn accents_and_gates() {
        let g = generator();
        let (mut accents, mut total) = (0, 0);
        for seed in 0..200 {
            for e in bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP) {
                total += 1;
                if e.flags & flags::ACCENT != 0 {
                    accents += 1;
                    assert!(e.cutoff > 0 && e.vel > 105);
                }
            }
        }
        let share = accents as f32 / total as f32;
        assert!((0.2..0.4).contains(&share), "accent share {share}");
        // Longer gate parameter, longer notes.
        let total_gate = |v: f32| -> u32 {
            let p = params(|p| {
                p[P_GATE] = v;
                p[P_SLIDE] = 0.0;
            });
            bar(&g, 3, 0, &p, &TWO_STEP).iter().map(|e| e.gate as u32).sum()
        };
        assert!(total_gate(0.0) < total_gate(0.5) && total_gate(0.5) < total_gate(1.0));
    }

    #[test]
    fn idiom_cell_sets_the_rhythm() {
        let mut levels = [0u8; 16];
        for (s, l) in [(0, 255), (3, 200), (6, 150), (11, 90)] {
            levels[s] = l;
        }
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules::default(), vec![LevelMap::bar(levels)]);
        let free = |d: f32| {
            params(|p| {
                p[P_DENSITY] = d;
                p[P_KICK_MODE] = 0.5;
                p[P_COMPLEXITY] = 0.0;
                p[P_VARIATION] = 0.0;
            })
        };
        assert_eq!(steps(&bar(&g, 1, 0, &free(0.5), &EMPTY)), [0, 3, 6]);
        assert_eq!(steps(&bar(&g, 1, 0, &free(0.9), &EMPTY)), [0, 3, 6, 11]);
        assert_eq!(steps(&bar(&g, 1, 0, &free(0.1), &EMPTY)), [0]);
        // Never outside the cell, even at full density and complexity.
        let wild = params(|p| {
            p[P_DENSITY] = 1.0;
            p[P_COMPLEXITY] = 1.0;
            p[P_KICK_MODE] = 0.5;
            p[P_VARIATION] = 0.0;
        });
        for seed in 0..20 {
            assert_eq!(steps(&bar(&g, seed, 0, &wild, &EMPTY)), [0, 3, 6, 11]);
        }
    }

    #[test]
    fn golden_bars() {
        // Pinned output: a change here changes how saved seeds sound.
        let g = generator();
        let got: Vec<String> = [1u32, 2, 3].iter().map(|&s| notes(&bar(&g, s, 0, &PARAM_DEFAULTS, &TWO_STEP))).collect();
        assert_eq!(got, GOLDEN, "\n{got:#?}");
    }

    const GOLDEN: [&str; 3] = [
        "3:33 6:33 7:40 8:33 11:33 14:33 15:33",
        "2:33 3:33 6:33 7:33 8:40 11:40 14:45 15:40",
        "3:33 6:33 7:36 8:33 11:33 14:33",
    ];

    #[test]
    fn helpers() {
        assert_eq!(root_octave(MusicalKey::new(9, true), BASS_ROOT_MIN), 1); // A1 = 33
        assert_eq!(root_octave(MusicalKey::new(4, true), BASS_ROOT_MIN), 1); // E1 = 28
        assert_eq!(root_octave(MusicalKey::new(3, true), BASS_ROOT_MIN), 2); // D#2 = 39
        assert_eq!([register_shift(0.0), register_shift(0.5), register_shift(1.0)], [-1, 0, 1]);
        assert_eq!(KickMode::from_param(0.0), KickMode::Avoid);
        assert_eq!(KickMode::from_param(0.5), KickMode::Free);
        assert_eq!(KickMode::from_param(1.0), KickMode::Double);
    }
}
