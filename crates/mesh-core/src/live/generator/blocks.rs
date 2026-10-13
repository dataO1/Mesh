//! Reusable, pure pattern building blocks (design §6.2, research §3 and §7).
//!
//! Everything here is a pure function of its arguments, allocation-free and
//! cheap, so generators can call it per step. All randomness comes in as a
//! hash from [`crate::live::rng`]; nothing here draws its own.
//!
//! Steps are 16ths, 0-indexed (step 0 = beat 1). Importance levels are either
//! `u8` (0..=255, as stored in styles) or `f32` (0..=1).

use std::collections::BTreeMap;

use super::{BarCtx, MAX_GEN_PARAMS};
use crate::live::event::{EventKind, LiveEvent};
use crate::live::rng::{self, RngKey};
use crate::live::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// Longest level map a generator stores: 4 bars of 16 steps.
pub const MAX_MAP_STEPS: usize = 64;

/// Bar of the phrase that carries the variation (A3: A A A B).
pub const VARIATION_BAR: u8 = 3;

/// RNG stream shared by all generators: selects which steps of the variation
/// bar are re-drawn. Generator-private streams start at 0x100.
pub const STREAM_PHRASE_SELECT: u32 = 1;

// ─── Parameters ────────────────────────────────────────────────────────

/// Read a normalized generator parameter, clamped to `0..=1` (non-finite = 0).
#[inline]
pub fn param(ctx: &BarCtx, index: usize) -> f32 {
    let v = ctx.params[index];
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// Defaults for `names`, taken from the style's `generator.params` where
/// present and from `builtin` otherwise.
pub fn defaults_from_spec(
    names: &[&str],
    builtin: &[f32],
    spec: &BTreeMap<String, f32>,
) -> [f32; MAX_GEN_PARAMS] {
    let mut out = [0.0; MAX_GEN_PARAMS];
    for (i, name) in names.iter().enumerate().take(MAX_GEN_PARAMS) {
        let v = spec.get(*name).copied().unwrap_or_else(|| builtin.get(i).copied().unwrap_or(0.5));
        out[i] = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    }
    out
}

/// A construction-time option from the style's `generator.params` (keys that
/// are not performance parameters, e.g. `rule_a2`). Clamped to `0..=1`.
pub fn option(spec: &BTreeMap<String, f32>, key: &str, default: f32) -> f32 {
    match spec.get(key) {
        Some(v) if v.is_finite() => v.clamp(0.0, 1.0),
        _ => default,
    }
}

#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ─── Level maps ────────────────────────────────────────────────────────

/// Per-step importance levels (0..=255), 1 to 4 bars long. `Copy`, no heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelMap {
    len: u8,
    levels: [u8; MAX_MAP_STEPS],
}

impl LevelMap {
    /// A one-bar map.
    pub const fn bar(bar: [u8; STEPS_PER_BAR]) -> Self {
        let mut levels = [0u8; MAX_MAP_STEPS];
        let mut i = 0;
        while i < STEPS_PER_BAR {
            levels[i] = bar[i];
            i += 1;
        }
        Self { len: STEPS_PER_BAR as u8, levels }
    }

    /// All-zero one-bar map.
    pub const fn empty() -> Self {
        Self::bar([0; STEPS_PER_BAR])
    }

    /// Build from style data. The length is rounded up to whole bars (missing
    /// steps are 0) and capped at [`MAX_MAP_STEPS`]. Empty input = empty bar.
    pub fn from_slice(src: &[u8]) -> Self {
        let n = src.len().min(MAX_MAP_STEPS);
        let len = n.div_ceil(STEPS_PER_BAR).max(1) * STEPS_PER_BAR;
        let mut levels = [0u8; MAX_MAP_STEPS];
        levels[..n].copy_from_slice(&src[..n]);
        Self { len: len as u8, levels }
    }

    /// Number of steps (a multiple of 16).
    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.levels[..self.len as usize].iter().all(|&l| l == 0)
    }

    /// Level at `step` of `bar_in_phrase`. Maps longer than one bar advance
    /// with the bar and wrap.
    #[inline]
    pub fn at(&self, bar_in_phrase: u8, step: usize) -> u8 {
        let idx = (bar_in_phrase as usize * STEPS_PER_BAR + step) % self.len as usize;
        self.levels[idx]
    }

    /// The 16 levels of one bar.
    pub fn bar_levels(&self, bar_in_phrase: u8) -> [u8; STEPS_PER_BAR] {
        std::array::from_fn(|s| self.at(bar_in_phrase, s))
    }
}

/// Grids-style blend of two bars of levels: `t = 0` gives `a`, `1` gives `b`.
pub fn blend_levels(a: &[u8; STEPS_PER_BAR], b: &[u8; STEPS_PER_BAR], t: f32) -> [u8; STEPS_PER_BAR] {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|s| lerp(a[s] as f32, b[s] as f32, t).round() as u8)
}

#[inline]
pub fn level_unit(level: u8) -> f32 {
    level as f32 / 255.0
}

// ─── Euclid ────────────────────────────────────────────────────────────

/// Euclidean rhythm (Bjorklund / Toussaint 2005) as a bit mask: bit `i` set =
/// pulse on step `i`. `pulses` onsets spread as evenly as possible over
/// `steps` (1..=32), then rotated right by `rotation` steps.
///
/// Uses the Bresenham form, which yields the Bjorklund pattern up to rotation
/// (E(3,8) = `x..x..x.`).
pub fn euclid(pulses: u8, steps: u8, rotation: u8) -> u32 {
    let steps = steps.clamp(1, 32) as u32;
    let pulses = (pulses as u32).min(steps);
    let rot = rotation as u32 % steps;
    let mut mask = 0u32;
    for i in 0..steps {
        if (i * pulses) % steps < pulses {
            mask |= 1 << ((i + rot) % steps);
        }
    }
    mask
}

// ─── Metric weight ─────────────────────────────────────────────────────

/// Metric level of a 16th step: 0 = bar downbeat, 1 = half bar, 2 = beat,
/// 3 = 8th offbeat, 4 = 16th.
#[inline]
pub const fn metric_level(step: usize) -> u8 {
    let s = step % STEPS_PER_BAR;
    if s == 0 {
        0
    } else if s == 8 {
        1
    } else if s % 4 == 0 {
        2
    } else if s % 2 == 0 {
        3
    } else {
        4
    }
}

// MUSICAL: first draft, tune by ear. Importance per metric level (research
// §7.1.2: metric weighting as the cheap stand-in for a corpus).
pub const METRIC_WEIGHT: [f32; 5] = [1.0, 0.85, 0.7, 0.45, 0.2];

#[inline]
pub fn metric_weight(step: usize) -> f32 {
    METRIC_WEIGHT[metric_level(step) as usize]
}

// ─── Density threshold ─────────────────────────────────────────────────

/// Grids-style reveal: a step fires when
/// `importance + chaos · rand > 1 − density`.
///
/// Raising `density` reveals steps from most to least important and never
/// removes one (for a fixed `rand`). Zero density fires nothing, and a step
/// with zero importance and zero chaos never fires.
#[inline]
pub fn reveals(importance: f32, density: f32, chaos: f32, rand: f32) -> bool {
    density > 0.0 && importance + chaos * rand > 1.0 - density
}

// ─── Velocity tiers (A5) ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Ghost,
    Normal,
    Accent,
}

// MUSICAL: first draft, tune by ear. Velocity tiers (research §7.1.6: ghosts
// sit at 30-40 % of the accent velocity).
pub const VEL_ACCENT: u8 = 120;
pub const VEL_NORMAL: u8 = 96;
pub const GHOST_RATIO: f32 = 0.35;
/// Seeded velocity variation, ± this many steps of velocity.
pub const VEL_JITTER: f32 = 5.0;
/// Importance level from which a hit is an accent / a normal hit.
pub const TIER_ACCENT_LEVEL: u8 = 200;
pub const TIER_NORMAL_LEVEL: u8 = 90;

/// Tier by metric position: beats accent, 8ths normal, 16ths ghost.
#[inline]
pub fn tier_for_metric(step: usize) -> Tier {
    match metric_level(step) {
        0..=2 => Tier::Accent,
        3 => Tier::Normal,
        _ => Tier::Ghost,
    }
}

/// Tier by importance level.
#[inline]
pub fn tier_for_level(level: u8) -> Tier {
    if level >= TIER_ACCENT_LEVEL {
        Tier::Accent
    } else if level >= TIER_NORMAL_LEVEL {
        Tier::Normal
    } else {
        Tier::Ghost
    }
}

/// Velocity for a tier with seeded variation.
///
/// `amount` scales rule A5: 1 = full tiers and variation, 0 = every hit at
/// [`VEL_NORMAL`] (flat, for the ear test).
pub fn velocity(tier: Tier, rand: f32, amount: f32) -> u8 {
    let base = match tier {
        Tier::Accent => VEL_ACCENT as f32,
        Tier::Normal => VEL_NORMAL as f32,
        Tier::Ghost => VEL_ACCENT as f32 * GHOST_RATIO,
    };
    let amount = amount.clamp(0.0, 1.0);
    let v = lerp(VEL_NORMAL as f32, base, amount) + (rand * 2.0 - 1.0) * VEL_JITTER * amount;
    v.round().clamp(1.0, 127.0) as u8
}

// ─── Phrase shape (A3) ─────────────────────────────────────────────────

// MUSICAL: first draft, tune by ear. Share of steps re-drawn in the
// variation bar at `variation = 1`.
pub const VARIATION_STEP_PROB: f32 = 0.5;

/// Phrase-shaped randomness: bars 0-2 of a phrase draw from the same
/// coordinates (so they repeat), the variation bar re-draws a share of its
/// steps set by `variation`.
///
/// With the phrase rule off every bar draws from its own coordinates.
#[derive(Debug, Clone, Copy)]
pub struct PhraseRng {
    /// Coordinates shared by the repeating bars.
    pub base: RngKey,
    /// Coordinates of this bar.
    pub bar: RngKey,
    /// Probability that a step of this bar is re-drawn (0 outside the
    /// variation bar).
    amount: f32,
    varied: bool,
}

impl PhraseRng {
    pub fn new(ctx: &BarCtx, variation: f32, phrase_rule: bool) -> Self {
        let bar = RngKey::new(ctx.seed, ctx.lane, ctx.bar_in_phrase);
        if !phrase_rule {
            return Self { base: bar, bar, amount: 0.0, varied: false };
        }
        let varied = ctx.bar_in_phrase == VARIATION_BAR && variation > 0.0;
        Self {
            base: RngKey::new(ctx.seed, ctx.lane, 0),
            bar,
            amount: if varied { variation.clamp(0.0, 1.0) * VARIATION_STEP_PROB } else { 0.0 },
            varied,
        }
    }

    /// `true` in the variation bar when `variation > 0`.
    pub fn is_variation_bar(&self) -> bool {
        self.varied
    }

    /// `true` when `step` is re-drawn in this bar. The choice is per step, so
    /// every decision on a varied step changes together.
    #[inline]
    pub fn step_varies(&self, step: u8) -> bool {
        self.varied && self.bar.chance(step, STREAM_PHRASE_SELECT, self.amount)
    }

    #[inline]
    pub fn at(&self, step: u8, stream: u32) -> u64 {
        if self.step_varies(step) { self.bar.at(step, stream) } else { self.base.at(step, stream) }
    }

    #[inline]
    pub fn unit(&self, step: u8, stream: u32) -> f32 {
        rng::unit_f32(self.at(step, stream))
    }

    #[inline]
    pub fn chance(&self, step: u8, stream: u32, p: f32) -> bool {
        rng::chance(self.at(step, stream), p)
    }
}

// ─── Déjà vu (Marbles) ─────────────────────────────────────────────────

/// Marbles-style "déjà vu", stateless: random bits for the `index`-th draw of
/// a sequence that loops over `loop_len` values.
///
/// - `amount = 0`: every draw is fresh.
/// - `0..0.5`: rising chance that a draw comes from the loop.
/// - `0.5`: fully locked, the sequence repeats every `loop_len` draws.
/// - `0.5..1`: locked, with a rising chance of jumping to a random loop position.
pub fn deja_vu(key: &RngKey, index: u8, loop_len: u8, amount: f32, stream: u32) -> u64 {
    let loop_len = loop_len.max(1);
    let amount = amount.clamp(0.0, 1.0);
    let looped = |pos: u8| key.at(pos % loop_len, stream);
    let select = key.at(index, stream ^ 0x8000_0000);
    if amount <= 0.5 {
        if rng::chance(select, 1.0 - 2.0 * amount) {
            // Fresh value: offset the coordinate so it never equals a loop slot.
            rng::derive(key.at(index, stream), 1)
        } else {
            looped(index)
        }
    } else if rng::chance(select, 2.0 * amount - 1.0) {
        looped(rng::below(rng::derive(select, 0), loop_len as u32) as u8)
    } else {
        looped(index)
    }
}

// ─── Trig conditions (Elektron) ────────────────────────────────────────

/// A condition on whether a placed step actually fires.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrigCond {
    Always,
    /// Fires with this probability.
    Prob(f32),
    /// `A:B` — fires on the `a`-th of every `b` bars (1-indexed).
    Ratio { a: u8, b: u8 },
    /// Only in the fill bar.
    Fill,
    /// Only outside the fill bar.
    NotFill,
    /// Only in the first bar of the phrase (1ST).
    First,
}

impl TrigCond {
    /// Evaluate for a bar. `h` is the random draw for [`TrigCond::Prob`];
    /// `bar` counts bars (the bar in the phrase, or an absolute bar count).
    pub fn passes(self, h: u64, bar: u32, fill: bool) -> bool {
        match self {
            TrigCond::Always => true,
            TrigCond::Prob(p) => rng::chance(h, p),
            TrigCond::Ratio { a, b } => b > 0 && bar % b as u32 + 1 == a as u32,
            TrigCond::Fill => fill,
            TrigCond::NotFill => !fill,
            TrigCond::First => bar == 0,
        }
    }
}

// ─── Timing ────────────────────────────────────────────────────────────

/// Tick of a 16th step.
#[inline]
pub const fn step_tick(step: usize) -> u16 {
    step as u16 * TICKS_PER_STEP
}

/// Step containing `tick`.
#[inline]
pub const fn tick_step(tick: u16) -> usize {
    let s = (tick / TICKS_PER_STEP) as usize;
    if s >= STEPS_PER_BAR { STEPS_PER_BAR - 1 } else { s }
}

/// Largest swing delay: offbeat 16ths land on the triplet (66 % swing).
pub const MAX_SWING_TICKS: u16 = TICKS_PER_STEP / 3;

/// Swing as a tick delay for offbeat 16ths (odd steps). `amount` 0 = straight
/// (the default, A5), 1 = triplet swing.
#[inline]
pub fn swing_offset(step: usize, amount: f32) -> u16 {
    if step % 2 == 1 {
        (amount.clamp(0.0, 1.0) * MAX_SWING_TICKS as f32).round() as u16
    } else {
        0
    }
}

/// Ticks of a ratchet / roll: `count` evenly spaced hits over `span` ticks
/// starting at `start`. Hits past the bar end are dropped.
pub fn ratchet(start: u16, span: u16, count: u8) -> impl Iterator<Item = (u8, u16)> {
    let count = count.max(1);
    (0..count)
        .map(move |i| (i, start + (span as u32 * i as u32 / count as u32) as u16))
        .filter(|&(_, t)| t < TICKS_PER_BAR)
}

// MUSICAL: first draft, tune by ear. A roll starts at this share of its
// final velocity and ramps up.
pub const ROLL_START_RATIO: f32 = 0.6;

/// Velocity of the `i`-th of `count` roll hits, ramping up to `peak`.
pub fn roll_velocity(i: u8, count: u8, peak: u8) -> u8 {
    if count <= 1 {
        return peak;
    }
    let t = i as f32 / (count - 1) as f32;
    (peak as f32 * lerp(ROLL_START_RATIO, 1.0, t)).round().clamp(1.0, 127.0) as u8
}

// ─── Events ────────────────────────────────────────────────────────────

/// Unpitched sampler trigger (`key` 0 = no pitch, `gate` 0 = one-shot).
#[inline]
pub fn trigger(tick: u16, target: u8, vel: u8) -> LiveEvent {
    LiveEvent { tick, kind: EventKind::Trigger, target, vel, ..LiveEvent::default() }
}

/// Pitched note with a gate, clamped to end inside the bar.
#[inline]
pub fn note(tick: u16, key: u8, vel: u8, gate: u16, note_id: u16) -> LiveEvent {
    let gate = gate.clamp(1, TICKS_PER_BAR - tick.min(TICKS_PER_BAR - 1));
    LiveEvent { tick, kind: EventKind::NoteOn, key, vel, gate, note_id, ..LiveEvent::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(mask: u32, steps: u8) -> String {
        (0..steps).map(|i| if mask & (1 << i) != 0 { 'x' } else { '.' }).collect()
    }

    #[test]
    fn euclid_known_patterns() {
        assert_eq!(pattern(euclid(3, 8, 0), 8), "x..x..x.");
        assert_eq!(pattern(euclid(4, 16, 0), 16), "x...x...x...x...");
        assert_eq!(pattern(euclid(2, 5, 0), 5), "x..x.");
        assert_eq!(pattern(euclid(0, 8, 0), 8), "........");
        assert_eq!(pattern(euclid(8, 8, 0), 8), "xxxxxxxx");
        // More pulses than steps saturates.
        assert_eq!(euclid(20, 8, 0), 0xFF);
    }

    #[test]
    fn euclid_pulse_count_and_rotation() {
        for steps in 1..=32u8 {
            for pulses in 0..=steps {
                for rot in [0u8, 1, 5, 40] {
                    assert_eq!(euclid(pulses, steps, rot).count_ones(), pulses as u32);
                }
            }
        }
        assert_eq!(pattern(euclid(3, 8, 1), 8), ".x..x..x");
        assert_eq!(euclid(3, 8, 8), euclid(3, 8, 0));
    }

    #[test]
    fn euclid_is_maximally_even() {
        // Gaps between pulses differ by at most one step.
        for (pulses, steps) in [(3u8, 8u8), (5, 8), (5, 16), (7, 16), (9, 16), (11, 24)] {
            let m = euclid(pulses, steps, 0);
            let pos: Vec<u32> = (0..steps as u32).filter(|i| m & (1 << i) != 0).collect();
            let gaps: Vec<u32> = (0..pos.len())
                .map(|i| (pos[(i + 1) % pos.len()] + steps as u32 - pos[i] - 1) % steps as u32 + 1)
                .collect();
            let (lo, hi) = (gaps.iter().min().unwrap(), gaps.iter().max().unwrap());
            assert!(hi - lo <= 1, "E({pulses},{steps}) gaps {gaps:?}");
        }
    }

    #[test]
    fn metric_levels() {
        let levels: Vec<u8> = (0..16).map(metric_level).collect();
        assert_eq!(levels, [0, 4, 3, 4, 2, 4, 3, 4, 1, 4, 3, 4, 2, 4, 3, 4]);
        assert!(metric_weight(0) > metric_weight(8));
        assert!(metric_weight(8) > metric_weight(4));
        assert!(metric_weight(4) > metric_weight(2));
        assert!(metric_weight(2) > metric_weight(1));
    }

    #[test]
    fn reveal_is_monotonic_in_density() {
        for i in 0..200u32 {
            let h = rng::hash(i, 0, 0, 0, 0);
            let imp = rng::unit_f32(h);
            let r = rng::unit_f32(rng::derive(h, 1));
            let mut fired = false;
            for d in 0..=20 {
                let now = reveals(imp, d as f32 / 20.0, 0.3, r);
                assert!(now || !fired, "step vanished as density rose");
                fired = now;
            }
        }
        assert!(!reveals(1.0, 0.0, 1.0, 1.0));
        assert!(!reveals(0.0, 1.0, 0.0, 0.5));
        assert!(reveals(1.0, 0.01, 0.0, 0.0));
    }

    #[test]
    fn reveal_orders_by_importance() {
        // Without chaos, a more important step fires at or before a lesser one.
        for d in 0..=20 {
            let d = d as f32 / 20.0;
            assert!(reveals(0.8, d, 0.0, 0.0) || !reveals(0.4, d, 0.0, 0.0));
        }
    }

    #[test]
    fn velocity_tiers() {
        let ghost = velocity(Tier::Ghost, 0.5, 1.0);
        let normal = velocity(Tier::Normal, 0.5, 1.0);
        let accent = velocity(Tier::Accent, 0.5, 1.0);
        assert!(ghost < normal && normal < accent);
        let ratio = ghost as f32 / accent as f32;
        assert!((0.3..=0.4).contains(&ratio), "ghost ratio {ratio}");
        // Variation stays small and in range.
        for i in 0..100u32 {
            let r = rng::unit_f32(rng::hash(i, 0, 0, 0, 0));
            for tier in [Tier::Ghost, Tier::Normal, Tier::Accent] {
                let v = velocity(tier, r, 1.0) as f32;
                let mid = velocity(tier, 0.5, 1.0) as f32;
                assert!((v - mid).abs() <= VEL_JITTER + 1.0);
                assert!((1.0..=127.0).contains(&v));
            }
        }
        // A5 off: flat.
        assert_eq!(velocity(Tier::Ghost, 0.9, 0.0), VEL_NORMAL);
        assert_eq!(velocity(Tier::Accent, 0.1, 0.0), VEL_NORMAL);
    }

    #[test]
    fn tiers_by_level_and_metric() {
        assert_eq!(tier_for_level(255), Tier::Accent);
        assert_eq!(tier_for_level(120), Tier::Normal);
        assert_eq!(tier_for_level(40), Tier::Ghost);
        assert_eq!(tier_for_metric(0), Tier::Accent);
        assert_eq!(tier_for_metric(4), Tier::Accent);
        assert_eq!(tier_for_metric(2), Tier::Normal);
        assert_eq!(tier_for_metric(3), Tier::Ghost);
    }

    fn ctx_with<'a>(
        seed: u32,
        bar: u8,
        params: &'a [f32; MAX_GEN_PARAMS],
        mask: &'a [u8; STEPS_PER_BAR],
    ) -> BarCtx<'a> {
        crate::live::generator::test_util::ctx(seed, bar, params, mask)
    }

    #[test]
    fn phrase_rng_repeats_then_varies() {
        let params = [0.0; MAX_GEN_PARAMS];
        let mask = [0u8; STEPS_PER_BAR];
        let draw = |bar: u8, variation: f32, rule: bool| -> Vec<u64> {
            let r = PhraseRng::new(&ctx_with(9, bar, &params, &mask), variation, rule);
            (0..16u8).map(|s| r.at(s, 0x100)).collect()
        };
        let b0 = draw(0, 1.0, true);
        assert_eq!(b0, draw(1, 1.0, true));
        assert_eq!(b0, draw(2, 1.0, true));
        let b3 = draw(3, 1.0, true);
        assert_ne!(b0, b3);
        // Some steps of the variation bar still repeat.
        assert!(b0.iter().zip(&b3).any(|(a, b)| a == b));
        // No variation: bar 3 repeats too.
        assert_eq!(b0, draw(3, 0.0, true));
        // Rule off: every bar differs.
        assert_ne!(draw(0, 1.0, false), draw(1, 1.0, false));
    }

    #[test]
    fn phrase_rng_variation_grows_monotonically() {
        let params = [0.0; MAX_GEN_PARAMS];
        let mask = [0u8; STEPS_PER_BAR];
        let c = ctx_with(4, VARIATION_BAR, &params, &mask);
        let mut prev = 0;
        for v in 0..=10 {
            let r = PhraseRng::new(&c, v as f32 / 10.0, true);
            let n = (0..16u8).filter(|&s| r.step_varies(s)).count();
            assert!(n >= prev);
            prev = n;
        }
        assert!(prev > 0);
    }

    #[test]
    fn deja_vu_locks_at_half() {
        let key = RngKey::new(3, 2, 0);
        let seq = |amount: f32| -> Vec<u64> { (0..32u8).map(|i| deja_vu(&key, i, 8, amount, 0x200)).collect() };
        let locked = seq(0.5);
        for i in 0..32 {
            assert_eq!(locked[i], locked[i % 8]);
        }
        let fresh = seq(0.0);
        let repeats = (8..32).filter(|&i| fresh[i] == fresh[i % 8]).count();
        assert_eq!(repeats, 0);
        // Above 0.5 every value still comes from the loop.
        let jumpy = seq(1.0);
        assert!(jumpy.iter().all(|v| locked[..8].contains(v)));
        assert_ne!(jumpy, locked);
    }

    #[test]
    fn trig_conditions() {
        assert!(TrigCond::Always.passes(0, 0, false));
        assert!(TrigCond::Fill.passes(0, 0, true) && !TrigCond::Fill.passes(0, 0, false));
        assert!(TrigCond::NotFill.passes(0, 0, false) && !TrigCond::NotFill.passes(0, 0, true));
        assert!(TrigCond::First.passes(0, 0, false) && !TrigCond::First.passes(0, 1, false));
        let r = TrigCond::Ratio { a: 2, b: 4 };
        let hits: Vec<bool> = (0..8).map(|b| r.passes(0, b, false)).collect();
        assert_eq!(hits, [false, true, false, false, false, true, false, false]);
        assert!(!TrigCond::Ratio { a: 1, b: 0 }.passes(0, 0, false));
        assert!(TrigCond::Prob(1.0).passes(u64::MAX, 0, false));
        assert!(!TrigCond::Prob(0.0).passes(0, 0, false));
    }

    #[test]
    fn swing_and_ticks() {
        assert_eq!(step_tick(4), 96);
        assert_eq!(tick_step(95), 3);
        assert_eq!(tick_step(383), 15);
        for s in 0..16 {
            assert_eq!(swing_offset(s, 0.0), 0);
        }
        assert_eq!(swing_offset(0, 1.0), 0);
        assert_eq!(swing_offset(1, 1.0), MAX_SWING_TICKS);
        assert_eq!(swing_offset(3, 0.5), MAX_SWING_TICKS / 2);
        // Swung steps stay inside their own step.
        assert!(step_tick(15) + swing_offset(15, 1.0) < TICKS_PER_BAR);
    }

    #[test]
    fn ratchet_expansion() {
        let ticks: Vec<u16> = ratchet(48, 24, 2).map(|(_, t)| t).collect();
        assert_eq!(ticks, [48, 60]);
        let ticks: Vec<u16> = ratchet(0, 96, 4).map(|(_, t)| t).collect();
        assert_eq!(ticks, [0, 24, 48, 72]);
        assert_eq!(ratchet(10, 24, 0).count(), 1);
        // Clipped at the bar end.
        assert!(ratchet(372, 48, 4).all(|(_, t)| t < TICKS_PER_BAR));
        assert_eq!(ratchet(372, 48, 4).count(), 1);
        assert!(roll_velocity(0, 4, 100) < roll_velocity(3, 4, 100));
        assert_eq!(roll_velocity(3, 4, 100), 100);
        assert_eq!(roll_velocity(0, 1, 100), 100);
    }

    #[test]
    fn level_map_shapes() {
        let m = LevelMap::bar([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(m.len(), 16);
        assert_eq!(m.at(0, 3), 4);
        assert_eq!(m.at(3, 3), 4);
        let two: Vec<u8> = (0..32).collect();
        let m = LevelMap::from_slice(&two);
        assert_eq!(m.len(), 32);
        assert_eq!(m.at(1, 0), 16);
        assert_eq!(m.at(2, 0), 0);
        // Ragged and oversized input.
        assert_eq!(LevelMap::from_slice(&[9; 20]).len(), 32);
        assert_eq!(LevelMap::from_slice(&[9; 20]).at(1, 5), 0);
        assert_eq!(LevelMap::from_slice(&[9; 200]).len(), MAX_MAP_STEPS);
        assert!(LevelMap::from_slice(&[]).is_empty());
        let a = [0u8; 16];
        let b = [200u8; 16];
        assert_eq!(blend_levels(&a, &b, 0.5)[0], 100);
        assert_eq!(blend_levels(&a, &b, 0.0), a);
        assert_eq!(blend_levels(&a, &b, 1.0), b);
    }

    #[test]
    fn spec_defaults_and_options() {
        let mut spec = BTreeMap::new();
        spec.insert("density".to_string(), 0.8);
        spec.insert("rule_a2".to_string(), 0.0);
        spec.insert("bad".to_string(), f32::NAN);
        let d = defaults_from_spec(&["style", "density"], &[0.25, 0.5], &spec);
        assert_eq!(d[0], 0.25);
        assert_eq!(d[1], 0.8);
        assert_eq!(option(&spec, "rule_a2", 1.0), 0.0);
        assert_eq!(option(&spec, "rule_a3", 1.0), 1.0);
        assert_eq!(option(&spec, "bad", 0.7), 0.7);
    }

    #[test]
    fn note_gate_stays_in_bar() {
        let n = note(380, 60, 100, 100, 1);
        assert_eq!(n.tick + n.gate, TICKS_PER_BAR);
        assert_eq!(note(0, 60, 100, 0, 1).gate, 1);
    }
}
