//! Drum lane generator: major-style backbone, break layer, minor overlays
//! and fill bar (design §6.3, §6.4, D14/D23; research §7.2, A1-A5).
//!
//! One generator renders the whole drum lane:
//!
//! 1. **Backbone** — kick and snare from the major style's importance maps.
//!    `style` moves along the lane's ordered archetypes and either blends the
//!    two neighbours (Grids-style) or switches to the nearest. `density`
//!    reveals steps in order of importance.
//! 2. **Break layer** — the break's slices, in their original order by
//!    default. Mutations swap in a slice of the same type or repeat the
//!    previous one; slices on backbone hits are replaced by a slice of the
//!    hit's type. Random order only in the fill.
//! 3. **Overlays** — hats, ghosts, percussion. Each has its own density and
//!    only adds events where the backbone importance is low (A2).
//! 4. **Fill** — when `ctx.fill`, the second half of the bar is a snare fill
//!    over shuffled break slices.
//!
//! The same struct serves the idiom family: the archetype tables then come
//! from the style's level maps instead of the built-in ones.
//!
//! Performance parameters (`BarCtx::params`, in this order): `style`,
//! `density`, `overlay_hats`, `overlay_ghost`, `overlay_perc`, `variation`,
//! `chop`, `swing`.
//!
//! Construction options (style `generator.params`, not performable):
//! `blend` (1 = blend neighbours, 0 = hard switch), `break_layer` (0 = off),
//! and the rule scales `rule_a2` (backbone lock), `rule_a3` (phrase shape),
//! `rule_a4` (slices in order), `rule_a5` (velocity tiers). All default to 1.

use std::collections::BTreeMap;

use super::blocks::{
    blend_levels, defaults_from_spec, euclid, lerp, level_unit, option, param, ratchet, reveals,
    roll_velocity, step_tick, swing_offset, tier_for_level, tier_for_metric, trigger, velocity,
    LevelMap, PhraseRng, Tier, VEL_NORMAL,
};
use super::layout::DrumLayout;
use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::live::event::{flags, BarEvents};
use crate::live::rng;
use crate::live::style::SliceType;
use crate::live::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

pub const PARAM_NAMES: [&str; 8] =
    ["style", "density", "overlay_hats", "overlay_ghost", "overlay_perc", "variation", "chop", "swing"];
const P_STYLE: usize = 0;
const P_DENSITY: usize = 1;
const P_HATS: usize = 2;
const P_GHOST: usize = 3;
const P_PERC: usize = 4;
const P_VARIATION: usize = 5;
const P_CHOP: usize = 6;
const P_SWING: usize = 7;

// MUSICAL: first draft, tune by ear. Defaults: two-step at medium density
// with 8th hats, a few ghosts, little percussion, a lightly chopped break.
pub const PARAM_DEFAULTS: [f32; 8] = [0.25, 0.5, 0.5, 0.3, 0.2, 0.5, 0.3, 0.0];

/// Archetype order used when the lane gives none (research §7.2).
pub const DEFAULT_ARCHETYPE_ORDER: [&str; 5] = ["half_time", "two_step", "rolling", "jungle", "hardcore"];

// RNG streams (private to the drum lane).
const S_VEL_KICK: u32 = 0x100;
const S_VEL_SNARE: u32 = 0x101;
const S_HAT: u32 = 0x110;
const S_HAT_OPEN: u32 = 0x111;
const S_HAT_VEL: u32 = 0x112;
const S_GHOST: u32 = 0x120;
const S_GHOST_VEL: u32 = 0x121;
const S_PERC: u32 = 0x130;
const S_PERC_ROT: u32 = 0x131;
const S_PERC_PICK: u32 = 0x132;
const S_PERC_VEL: u32 = 0x133;
const S_BRK_MUT: u32 = 0x140;
const S_BRK_KIND: u32 = 0x141;
const S_BRK_ORDER: u32 = 0x142;
const S_BRK_PICK: u32 = 0x143;
const S_BRK_VEL: u32 = 0x144;
const S_BRK_FILL: u32 = 0x145;
const S_BRK_RATCHET: u32 = 0x146;
const S_FILL_CELL: u32 = 0x150;
const S_FILL_VEL: u32 = 0x151;
const S_PICKUP: u32 = 0x160;

// ─── Musical tables ────────────────────────────────────────────────────

// MUSICAL: first draft, tune by ear. Backbone importance per step (A1).
// Anchors (>= 230) are the research §7.2 backbones; lower levels are extra
// hits that `density` reveals. Levels below 128 stay hidden at density 0.5.
const HALF_TIME_KICK: [u8; 16] = [255, 0, 0, 0, 0, 0, 70, 0, 0, 0, 100, 0, 0, 0, 120, 0];
const HALF_TIME_SNARE: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 50];
const TWO_STEP_KICK: [u8; 16] = [255, 0, 0, 0, 0, 0, 80, 0, 0, 0, 250, 0, 0, 60, 0, 0];
const TWO_STEP_SNARE: [u8; 16] = [0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 70];
const ROLLING_KICK: [u8; 16] = [255, 0, 0, 0, 0, 0, 0, 150, 0, 0, 250, 0, 0, 0, 100, 0];
const ROLLING_SNARE: [u8; 16] = [0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 90];
const JUNGLE_KICK: [u8; 16] = [255, 0, 200, 0, 0, 0, 0, 0, 0, 0, 230, 180, 0, 0, 0, 0];
const JUNGLE_SNARE: [u8; 16] = [0, 0, 0, 0, 255, 0, 0, 150, 0, 160, 0, 0, 240, 0, 0, 140];
const HARDCORE_KICK: [u8; 16] = [255, 0, 0, 0, 250, 0, 0, 0, 250, 0, 0, 0, 250, 0, 100, 0];
const HARDCORE_SNARE: [u8; 16] = [0, 0, 0, 0, 230, 0, 0, 0, 0, 0, 0, 0, 230, 0, 0, 0];

// MUSICAL: first draft, tune by ear. Hat importance: offbeat 8ths first,
// then beats, then 16ths.
const HATS_8THS: [u8; 16] = [150, 50, 235, 60, 150, 50, 235, 70, 150, 50, 235, 60, 150, 50, 235, 80];
const HATS_RIDE: [u8; 16] = [220, 90, 235, 100, 220, 90, 235, 110, 220, 90, 235, 100, 220, 90, 235, 120];
const HATS_SPARSE: [u8; 16] = [120, 40, 200, 50, 120, 40, 200, 50, 120, 40, 200, 50, 120, 40, 200, 60];
const HATS_OFFBEAT: [u8; 16] = [100, 40, 245, 60, 100, 40, 245, 60, 100, 40, 245, 60, 100, 40, 245, 70];
const HATS_HALF: [u8; 16] = [200, 40, 120, 50, 180, 40, 120, 60, 200, 40, 120, 50, 180, 40, 150, 70];

// MUSICAL: first draft, tune by ear. Ghost-snare importance, strongest
// around the snares.
const GHOST_TWO_STEP: [u8; 16] = [0, 60, 40, 100, 0, 70, 110, 190, 30, 170, 60, 90, 0, 80, 120, 150];
const GHOST_ROLLING: [u8; 16] = [0, 90, 60, 150, 0, 110, 160, 220, 40, 210, 90, 140, 0, 120, 170, 200];
const GHOST_HALF: [u8; 16] = [0, 40, 60, 90, 70, 50, 100, 140, 0, 90, 110, 150, 80, 70, 160, 180];
const GHOST_JUNGLE: [u8; 16] = [0, 80, 0, 140, 0, 120, 170, 0, 90, 0, 0, 100, 0, 150, 190, 0];
const GHOST_HARDCORE: [u8; 16] = [0, 50, 90, 110, 0, 60, 100, 160, 0, 60, 100, 120, 0, 70, 130, 170];

// MUSICAL: first draft, tune by ear.
/// Randomness added to hat / ghost / percussion importance.
const HAT_CHAOS: f32 = 0.15;
const GHOST_CHAOS: f32 = 0.25;
const PERC_CHAOS: f32 = 0.2;
/// Importance of an open hat on an offbeat 8th (higher = opens earlier).
const OPEN_HAT_IMPORTANCE: f32 = 0.3;
const OPEN_HAT_CHAOS: f32 = 0.3;
/// Overlay density = overlay amount × lane density × this (so the defaults
/// 0.5 × 0.5 give 0.5).
const OVERLAY_DENSITY_GAIN: f32 = 2.0;
/// Most Euclid percussion pulses per bar.
const PERC_MAX_PULSES: f32 = 7.0;
/// With `rule_a2 = 1`, overlays stay off steps whose backbone level is at
/// least this (and off every step where the backbone fired).
const LOCK_LEVEL_MIN: f32 = 128.0;

// MUSICAL: first draft, tune by ear. Break layer.
/// `chop` at or above this chops in 16ths, below in 8ths.
const CHOP_RATE_SPLIT: f32 = 0.5;
/// Mutation probability per chop at `chop = 1`.
const BREAK_MUT_MAX: f32 = 0.35;
/// Extra mutation probability in the variation bar at `variation = 1`.
const BREAK_MUT_VARIATION: f32 = 0.35;
/// Share of mutations that repeat the previous chop (stutter).
const BREAK_STUTTER_SHARE: f32 = 0.4;
const BREAK_VEL: f32 = 104.0;
const BREAK_VEL_JITTER: f32 = 4.0;
/// Gain of a kick / snare slice that is not on a backbone hit (ghosted so the
/// break does not fight the backbone).
const BREAK_OFF_BACKBONE_GAIN: f32 = 0.55;
/// Chance that a fill chop is doubled to 32nds.
const BREAK_FILL_RATCHET_PROB: f32 = 0.3;

// MUSICAL: first draft, tune by ear. Fill.
/// The fill replaces the bar from this step.
const FILL_START_STEP: usize = 8;
/// Fill hits ramp from this share of their velocity up to full.
const FILL_RAMP_START: f32 = 0.75;

/// One step of a fill cell.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fill {
    Rest,
    Kick,
    Ghost,
    Snare,
    Accent,
    /// Two 32nd snares.
    Ruff,
}

// MUSICAL: first draft, tune by ear. Snare fills for the second half bar,
// picked by seed.
const FILL_CELLS: [[Fill; 8]; 4] = {
    use Fill::*;
    [
        [Snare, Rest, Ghost, Snare, Rest, Snare, Snare, Accent],
        [Kick, Rest, Snare, Rest, Snare, Snare, Ruff, Ruff],
        [Snare, Ghost, Ghost, Snare, Ghost, Ghost, Snare, Accent],
        [Rest, Rest, Snare, Snare, Kick, Snare, Ruff, Accent],
    ]
};

// MUSICAL: first draft, tune by ear. Buildup: snare roll interval per bar of
// the phrase (quarters, 8ths, 16ths, 16ths), 32nds from `BUILDUP_32ND_TICK`
// of the last bar and silence from `BUILDUP_SILENT_TICK` (one beat before the
// drop, research §7.2).
const BUILDUP_INTERVAL: [u16; 4] = [96, 48, 24, 24];
const BUILDUP_32ND_TICK: u16 = 192;
const BUILDUP_SILENT_TICK: u16 = 288;
const BUILDUP_VEL_START: f32 = 50.0;

// ─── Archetypes ────────────────────────────────────────────────────────

/// One major style: importance maps for the backbone and the overlays.
#[derive(Debug, Clone, PartialEq)]
pub struct Archetype {
    pub name: String,
    /// Special case: accelerating snare roll instead of the maps.
    pub buildup: bool,
    pub kick: LevelMap,
    pub snare: LevelMap,
    pub hats: LevelMap,
    pub ghost: LevelMap,
    /// `None` = procedural Euclid percussion.
    pub perc: Option<LevelMap>,
}

impl Archetype {
    /// An archetype with empty maps (filled from idiom data).
    pub fn blank(name: &str) -> Self {
        Self {
            name: name.to_string(),
            buildup: false,
            kick: LevelMap::empty(),
            snare: LevelMap::empty(),
            hats: LevelMap::empty(),
            ghost: LevelMap::empty(),
            perc: None,
        }
    }

    fn from_tables(name: &str, kick: [u8; 16], snare: [u8; 16], hats: [u8; 16], ghost: [u8; 16]) -> Self {
        Self {
            name: name.to_string(),
            buildup: false,
            kick: LevelMap::bar(kick),
            snare: LevelMap::bar(snare),
            hats: LevelMap::bar(hats),
            ghost: LevelMap::bar(ghost),
            perc: None,
        }
    }

    /// The built-in archetype of that name, if any.
    pub fn builtin(name: &str) -> Option<Self> {
        Some(match name {
            "half_time" => Self::from_tables(name, HALF_TIME_KICK, HALF_TIME_SNARE, HATS_HALF, GHOST_HALF),
            "two_step" => Self::from_tables(name, TWO_STEP_KICK, TWO_STEP_SNARE, HATS_8THS, GHOST_TWO_STEP),
            "rolling" => Self::from_tables(name, ROLLING_KICK, ROLLING_SNARE, HATS_RIDE, GHOST_ROLLING),
            "jungle" => Self::from_tables(name, JUNGLE_KICK, JUNGLE_SNARE, HATS_SPARSE, GHOST_JUNGLE),
            "hardcore" => Self::from_tables(name, HARDCORE_KICK, HARDCORE_SNARE, HATS_OFFBEAT, GHOST_HARDCORE),
            "buildup" => Self { buildup: true, ..Self::blank(name) },
            _ => return None,
        })
    }

    /// The built-in archetypes in research order.
    pub fn default_order() -> Vec<Self> {
        DEFAULT_ARCHETYPE_ORDER.iter().filter_map(|n| Self::builtin(n)).collect()
    }
}

/// Rule switches and scales, fixed at construction (design §6.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrumRules {
    /// Blend neighbouring archetypes (`true`) or switch to the nearest.
    pub blend: bool,
    /// Render the break layer.
    pub break_layer: bool,
    /// A2: backbone lock, 0 = overlays land anywhere.
    pub lock: f32,
    /// A3: bars 0-2 repeat, bar 3 varies.
    pub phrase: bool,
    /// A4: share of break swaps that keep the slice type (the rest pick any slice).
    pub order: f32,
    /// A5: velocity tier depth, 0 = flat.
    pub tiers: f32,
}

impl Default for DrumRules {
    fn default() -> Self {
        Self { blend: true, break_layer: true, lock: 1.0, phrase: true, order: 1.0, tiers: 1.0 }
    }
}

impl DrumRules {
    pub fn from_spec(params: &BTreeMap<String, f32>) -> Self {
        Self {
            blend: option(params, "blend", 1.0) >= 0.5,
            break_layer: option(params, "break_layer", 1.0) >= 0.5,
            lock: option(params, "rule_a2", 1.0),
            phrase: option(params, "rule_a3", 1.0) >= 0.5,
            order: option(params, "rule_a4", 1.0),
            tiers: option(params, "rule_a5", 1.0),
        }
    }
}

/// Performance parameters of one bar.
struct Params {
    style: f32,
    density: f32,
    hats: f32,
    ghost: f32,
    perc: f32,
    variation: f32,
    chop: f32,
    swing: f32,
}

impl Params {
    fn read(ctx: &BarCtx) -> Self {
        Self {
            style: param(ctx, P_STYLE),
            density: param(ctx, P_DENSITY),
            hats: param(ctx, P_HATS),
            ghost: param(ctx, P_GHOST),
            perc: param(ctx, P_PERC),
            variation: param(ctx, P_VARIATION),
            chop: param(ctx, P_CHOP),
            swing: param(ctx, P_SWING),
        }
    }
}

/// The blended levels of one bar.
struct BarLevels {
    kick: [u8; STEPS_PER_BAR],
    snare: [u8; STEPS_PER_BAR],
    hats: [u8; STEPS_PER_BAR],
    ghost: [u8; STEPS_PER_BAR],
    perc: Option<[u8; STEPS_PER_BAR]>,
}

/// Backbone result of one bar, as step bit masks.
#[derive(Default)]
struct Backbone {
    kick: u16,
    snare: u16,
    /// Steps overlays must leave alone.
    locked: u16,
}

#[inline]
fn bit(step: usize) -> u16 {
    1 << step
}

#[inline]
fn overlay_density(density: f32, amount: f32) -> f32 {
    (density * amount * OVERLAY_DENSITY_GAIN).min(1.0)
}

/// The drum lane generator. See the module docs.
pub struct DrumGenerator {
    archetypes: Vec<Archetype>,
    /// Backbone used while `ctx.half_time` is set.
    half_time: Archetype,
    layout: DrumLayout,
    defaults: [f32; MAX_GEN_PARAMS],
    rules: DrumRules,
}

impl DrumGenerator {
    /// `archetypes` in major-style order (empty = the built-in order).
    pub fn new(archetypes: Vec<Archetype>, layout: DrumLayout, defaults: [f32; MAX_GEN_PARAMS], rules: DrumRules) -> Self {
        let archetypes = if archetypes.is_empty() { Archetype::default_order() } else { archetypes };
        let half_time = archetypes
            .iter()
            .find(|a| a.name == "half_time")
            .cloned()
            .or_else(|| Archetype::builtin("half_time"))
            .unwrap_or_else(|| Archetype::blank("half_time"));
        Self { archetypes, half_time, layout, defaults, rules }
    }

    /// Built-in archetypes and defaults for `layout`.
    pub fn with_layout(layout: DrumLayout) -> Self {
        let defaults = defaults_from_spec(&PARAM_NAMES, &PARAM_DEFAULTS, &BTreeMap::new());
        Self::new(Vec::new(), layout, defaults, DrumRules::default())
    }

    pub fn layout(&self) -> &DrumLayout {
        &self.layout
    }

    pub fn archetypes(&self) -> &[Archetype] {
        &self.archetypes
    }

    /// The two archetypes around `style` and the blend position between them.
    fn select(&self, style: f32, half_time: bool) -> (&Archetype, &Archetype, f32) {
        if half_time {
            return (&self.half_time, &self.half_time, 0.0);
        }
        let n = self.archetypes.len();
        if n == 1 {
            return (&self.archetypes[0], &self.archetypes[0], 0.0);
        }
        let pos = style * (n - 1) as f32;
        let i = (pos.floor() as usize).min(n - 2);
        let (a, b) = (&self.archetypes[i], &self.archetypes[i + 1]);
        let mut t = pos - i as f32;
        // A buildup has no maps to blend with.
        if !self.rules.blend || a.buildup || b.buildup {
            t = if t < 0.5 { 0.0 } else { 1.0 };
        }
        (a, b, t)
    }

    fn backbone(&self, p: &Params, rng: &PhraseRng, lv: &BarLevels, until: usize, out: &mut BarEvents) -> Backbone {
        let l = &self.layout;
        let mut bb = Backbone::default();
        for s in 0..until {
            let (k, sn) = (lv.kick[s], lv.snare[s]);
            if reveals(level_unit(k), p.density, 0.0, 0.0) {
                bb.kick |= bit(s);
                if let Some(target) = l.kick {
                    let tier = tier_for_level(k);
                    let mut ev = trigger(step_tick(s), target, velocity(tier, rng.unit(s as u8, S_VEL_KICK), self.rules.tiers));
                    if tier == Tier::Accent {
                        ev.flags |= flags::ACCENT;
                    }
                    out.push(ev);
                }
            }
            if reveals(level_unit(sn), p.density, 0.0, 0.0) {
                bb.snare |= bit(s);
                let tier = tier_for_level(sn);
                let target = if tier == Tier::Ghost { l.ghost.or(l.snare) } else { l.snare };
                if let Some(target) = target {
                    let mut ev = trigger(step_tick(s), target, velocity(tier, rng.unit(s as u8, S_VEL_SNARE), self.rules.tiers));
                    if tier == Tier::Accent {
                        ev.flags |= flags::ACCENT;
                    }
                    out.push(ev);
                }
            }
        }
        bb
    }

    /// Snare fill over the second half bar.
    fn fill(&self, ctx: &BarCtx, bb: &mut Backbone, out: &mut BarEvents) {
        let l = &self.layout;
        let key = rng::RngKey::new(ctx.seed, ctx.lane, ctx.bar_in_phrase);
        let cell = &FILL_CELLS[rng::below(key.at(0, S_FILL_CELL), FILL_CELLS.len() as u32) as usize];
        for (i, &code) in cell.iter().enumerate() {
            let s = FILL_START_STEP + i;
            let ramp = lerp(FILL_RAMP_START, 1.0, i as f32 / (cell.len() - 1) as f32);
            let r = key.unit(s as u8, S_FILL_VEL);
            let scaled = |tier: Tier| ((velocity(tier, r, self.rules.tiers) as f32 * ramp).round() as u8).max(1);
            match code {
                Fill::Rest => {}
                Fill::Kick => {
                    bb.kick |= bit(s);
                    if let Some(t) = l.kick {
                        out.push(trigger(step_tick(s), t, scaled(Tier::Normal)));
                    }
                }
                Fill::Ghost => {
                    bb.snare |= bit(s);
                    if let Some(t) = l.ghost.or(l.snare) {
                        out.push(trigger(step_tick(s), t, scaled(Tier::Ghost)));
                    }
                }
                Fill::Snare | Fill::Accent => {
                    bb.snare |= bit(s);
                    if let Some(t) = l.snare {
                        let tier = if code == Fill::Accent { Tier::Accent } else { Tier::Normal };
                        let mut ev = trigger(step_tick(s), t, scaled(tier));
                        if tier == Tier::Accent {
                            ev.flags |= flags::ACCENT;
                        }
                        out.push(ev);
                    }
                }
                Fill::Ruff => {
                    bb.snare |= bit(s);
                    if let Some(t) = l.snare {
                        let peak = scaled(Tier::Normal);
                        for (n, tick) in ratchet(step_tick(s), TICKS_PER_STEP, 2) {
                            out.push(trigger(tick, t, roll_velocity(n, 2, peak)));
                        }
                    }
                }
            }
        }
    }

    /// Phrase-end pickup in the variation bar: a 32nd ruff into the next
    /// phrase. Returns `false` when the kit has no one-shot to play it (the
    /// break layer then doubles its last slice instead).
    fn pickup(&self, p: &Params, rng: &PhraseRng, bb: &mut Backbone, out: &mut BarEvents) -> bool {
        let l = &self.layout;
        let Some(target) = l.ghost.or(l.snare).or(l.kick) else { return false };
        let s = STEPS_PER_BAR - 1;
        let r = rng.bar.unit(s as u8, S_PICKUP);
        let vel = lerp(
            velocity(Tier::Ghost, r, self.rules.tiers) as f32,
            velocity(Tier::Normal, r, self.rules.tiers) as f32,
            p.variation,
        )
        .round() as u8;
        let free = (bb.kick | bb.snare) & bit(s) == 0;
        for (n, tick) in ratchet(step_tick(s), TICKS_PER_STEP, 2) {
            // The 32nd offbeat is always free; the step itself only if the
            // backbone left it empty.
            if n == 1 || free {
                out.push(trigger(tick, target, roll_velocity(n, 2, vel)));
            }
        }
        bb.locked |= bit(s);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn break_layer(&self, ctx: &BarCtx, p: &Params, rng: &PhraseRng, bb: &Backbone, fill_from: usize, double_last: bool, out: &mut BarEvents) {
        let l = &self.layout;
        let types = l.slices();
        let n = types.len();
        if n == 0 {
            return;
        }
        let spb = l.slices_per_bar();
        let total = l.break_bars() * STEPS_PER_BAR;
        let bar_ofs = (ctx.bar_in_phrase as usize % l.break_bars()) * STEPS_PER_BAR;
        let unit = if p.chop >= CHOP_RATE_SPLIT { 1 } else { 2 };
        let units = (total / unit) as u32;
        // Slice and type at a 16th position of the break. A position inside a
        // slice (grid coarser than 16ths) is no onset: type `Other`.
        let slice_at = |q: usize| ((q * spb / STEPS_PER_BAR).min(n - 1), q * spb % STEPS_PER_BAR);
        let type_at = |q: usize| {
            let (slice, frac) = slice_at(q);
            if frac == 0 { types[slice] } else { SliceType::Other }
        };
        // Chop position of type `ty` nearest to `near` (ties: the earlier one).
        let nearest = |ty: SliceType, near: usize| {
            (0..total)
                .step_by(unit)
                .filter(|&j| type_at(j) == ty)
                .min_by_key(|&j| {
                    let d = j.abs_diff(near);
                    d.min(total - d)
                })
        };
        // A seeded chop position of type `ty` other than `not`.
        let same_type = |ty: SliceType, not: usize, h: u64| {
            let candidates = || (0..total).step_by(unit).filter(move |&j| j != not && type_at(j) == ty);
            let count = candidates().count() as u32;
            (count > 0).then(|| candidates().nth(rng::below(h, count) as usize)).flatten()
        };
        let mut_p = p.chop * BREAK_MUT_MAX
            + if rng.is_variation_bar() { p.variation * BREAK_MUT_VARIATION } else { 0.0 };

        // Source 16th position of the break for every step of the bar.
        let mut src = [0u8; STEPS_PER_BAR];
        for u in (0..STEPS_PER_BAR).step_by(unit) {
            let us = u as u8;
            let original = bar_ofs + u;
            let mut q = original;
            if u >= fill_from {
                // A4: random order only in the fill.
                q = rng::below(rng.bar.at(us, S_BRK_FILL), units) as usize * unit;
            } else {
                if u > 0 && rng.chance(us, S_BRK_MUT, mut_p) {
                    if rng.chance(us, S_BRK_KIND, BREAK_STUTTER_SHARE) {
                        q = src[u - unit] as usize;
                    } else if rng.chance(us, S_BRK_ORDER, self.rules.order) {
                        q = same_type(type_at(original), original, rng.at(us, S_BRK_PICK)).unwrap_or(original);
                    } else {
                        q = rng::below(rng.at(us, S_BRK_PICK), units) as usize * unit;
                    }
                }
                // A2: a backbone hit gets a slice of its own type.
                if self.rules.lock >= 0.5 {
                    let want = if bb.kick & bit(u) != 0 {
                        Some(SliceType::Kick)
                    } else if bb.snare & bit(u) != 0 {
                        Some(SliceType::Snare)
                    } else {
                        None
                    };
                    if let Some(ty) = want {
                        if type_at(q) != ty {
                            q = nearest(ty, original).unwrap_or(q);
                        }
                    }
                }
            }
            for k in 0..unit {
                src[u + k] = ((q + k) % total) as u8;
            }
        }

        for s in 0..STEPS_PER_BAR {
            let q = src[s] as usize;
            let (slice, frac) = slice_at(q);
            // Still inside the slice the previous step started: no new event.
            if s > 0 && q == src[s - 1] as usize + 1 && frac != 0 {
                continue;
            }
            let ty = if frac == 0 { types[slice] } else { SliceType::Other };
            let on_backbone = (ty == SliceType::Kick && bb.kick & bit(s) != 0)
                || (ty == SliceType::Snare && bb.snare & bit(s) != 0);
            let mut vel = lerp(
                VEL_NORMAL as f32,
                BREAK_VEL + (rng.unit(s as u8, S_BRK_VEL) * 2.0 - 1.0) * BREAK_VEL_JITTER,
                self.rules.tiers,
            );
            if matches!(ty, SliceType::Kick | SliceType::Snare) && !on_backbone && s < fill_from {
                vel *= lerp(1.0, BREAK_OFF_BACKBONE_GAIN, self.rules.lock);
            }
            let mut ev = trigger(step_tick(s), l.slice_target(slice), vel.round().clamp(1.0, 127.0) as u8);
            ev.choke = l.break_choke;
            ev.start = (frac * (65536 / STEPS_PER_BAR)) as u16;
            out.push(ev);
            let doubled = (s >= fill_from && rng.bar.chance(s as u8, S_BRK_RATCHET, BREAK_FILL_RATCHET_PROB))
                || (double_last && s == STEPS_PER_BAR - 1);
            if doubled {
                ev.tick += TICKS_PER_STEP / 2;
                out.push(ev);
            }
        }
    }

    fn hats(&self, p: &Params, rng: &PhraseRng, lv: &BarLevels, bb: &Backbone, out: &mut BarEvents) {
        let l = &self.layout;
        let Some(closed) = l.hat_closed else { return };
        let d = overlay_density(p.density, p.hats);
        for s in 0..STEPS_PER_BAR {
            let su = s as u8;
            if bb.locked & bit(s) != 0 || !reveals(level_unit(lv.hats[s]), d, HAT_CHAOS, rng.unit(su, S_HAT)) {
                continue;
            }
            // Open hats on offbeat 8ths, coming in as the density rises.
            let open = s % 4 == 2 && reveals(OPEN_HAT_IMPORTANCE, d, OPEN_HAT_CHAOS, rng.unit(su, S_HAT_OPEN));
            let target = if open { l.hat_open.unwrap_or(closed) } else { closed };
            let vel = velocity(tier_for_metric(s), rng.unit(su, S_HAT_VEL), self.rules.tiers);
            let mut ev = trigger(step_tick(s) + swing_offset(s, p.swing), target, vel);
            ev.choke = l.hats_choke;
            out.push(ev);
        }
    }

    fn ghosts(&self, p: &Params, rng: &PhraseRng, lv: &BarLevels, bb: &Backbone, out: &mut BarEvents) {
        let l = &self.layout;
        let Some(target) = l.ghost.or(l.snare) else { return };
        let d = overlay_density(p.density, p.ghost);
        for s in 0..STEPS_PER_BAR {
            let su = s as u8;
            if bb.locked & bit(s) != 0 || !reveals(level_unit(lv.ghost[s]), d, GHOST_CHAOS, rng.unit(su, S_GHOST)) {
                continue;
            }
            let vel = velocity(Tier::Ghost, rng.unit(su, S_GHOST_VEL), self.rules.tiers);
            out.push(trigger(step_tick(s) + swing_offset(s, p.swing), target, vel));
        }
    }

    fn perc(&self, p: &Params, rng: &PhraseRng, lv: &BarLevels, bb: &Backbone, out: &mut BarEvents) {
        let l = &self.layout;
        if l.perc_count == 0 {
            return;
        }
        let d = overlay_density(p.density, p.perc);
        // Without a map: a Euclid rhythm, pulse count from the density and
        // rotation from the seed.
        let euclid_mask = match lv.perc {
            Some(_) => 0,
            None => euclid(
                (d * PERC_MAX_PULSES).round() as u8,
                STEPS_PER_BAR as u8,
                rng::below(rng.base.at(0, S_PERC_ROT), STEPS_PER_BAR as u32) as u8,
            ),
        };
        for s in 0..STEPS_PER_BAR {
            let su = s as u8;
            if bb.locked & bit(s) != 0 {
                continue;
            }
            let fires = match &lv.perc {
                Some(map) => reveals(level_unit(map[s]), d, PERC_CHAOS, rng.unit(su, S_PERC)),
                None => euclid_mask & (1 << s) != 0,
            };
            if !fires {
                continue;
            }
            let target = super::layout::PERC_BASE + rng::below(rng.at(su, S_PERC_PICK), l.perc_count as u32) as u8;
            let vel = velocity(tier_for_metric(s), rng.unit(su, S_PERC_VEL), self.rules.tiers);
            out.push(trigger(step_tick(s), target, vel));
        }
    }

    /// Buildup: a snare roll that speeds up over the phrase, with a beat of
    /// silence before the next phrase.
    fn buildup(&self, ctx: &BarCtx, p: &Params, out: &mut BarEvents) {
        if p.density <= 0.0 {
            return;
        }
        let l = &self.layout;
        let bar = ctx.bar_in_phrase.min(3) as usize;
        if let (Some(kick), 0) = (l.kick, bar) {
            let mut ev = trigger(0, kick, velocity(Tier::Accent, 0.5, self.rules.tiers));
            ev.flags |= flags::ACCENT;
            out.push(ev);
        }
        let Some(target) = l.snare.or(l.ghost) else { return };
        let last = bar == BUILDUP_INTERVAL.len() - 1;
        let mut tick = 0u16;
        while tick < TICKS_PER_BAR && !(last && tick >= BUILDUP_SILENT_TICK) {
            let progress = (bar as f32 * TICKS_PER_BAR as f32 + tick as f32) / (4.0 * TICKS_PER_BAR as f32);
            let vel = lerp(VEL_NORMAL as f32, lerp(BUILDUP_VEL_START, 127.0, progress), self.rules.tiers);
            out.push(trigger(tick, target, vel.round().clamp(1.0, 127.0) as u8));
            tick += if last && tick >= BUILDUP_32ND_TICK { TICKS_PER_STEP / 2 } else { BUILDUP_INTERVAL[bar] };
        }
    }
}

impl LaneGenerator for DrumGenerator {
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let p = Params::read(ctx);
        let (a, b, t) = self.select(p.style, ctx.half_time);
        if (if t < 0.5 { a } else { b }).buildup {
            self.buildup(ctx, &p, out);
            out.sort();
            return;
        }
        let bar = ctx.bar_in_phrase;
        let lv = BarLevels {
            kick: blend_levels(&a.kick.bar_levels(bar), &b.kick.bar_levels(bar), t),
            snare: blend_levels(&a.snare.bar_levels(bar), &b.snare.bar_levels(bar), t),
            hats: blend_levels(&a.hats.bar_levels(bar), &b.hats.bar_levels(bar), t),
            ghost: blend_levels(&a.ghost.bar_levels(bar), &b.ghost.bar_levels(bar), t),
            perc: match (&a.perc, &b.perc) {
                (Some(x), Some(y)) => Some(blend_levels(&x.bar_levels(bar), &y.bar_levels(bar), t)),
                (Some(x), None) if t < 0.5 => Some(x.bar_levels(bar)),
                (None, Some(y)) if t >= 0.5 => Some(y.bar_levels(bar)),
                _ => None,
            },
        };
        let rng = PhraseRng::new(ctx, p.variation, self.rules.phrase);
        let fill_from = if ctx.fill { FILL_START_STEP } else { STEPS_PER_BAR };

        let mut bb = self.backbone(&p, &rng, &lv, fill_from, out);
        if ctx.fill {
            self.fill(ctx, &mut bb, out);
        }
        let wants_pickup = rng.is_variation_bar() && !ctx.fill;
        let picked_up = wants_pickup && self.pickup(&p, &rng, &mut bb, out);

        // A2: overlays stay off fired steps and off steps the backbone rates
        // as important, even while the density hides them.
        if self.rules.lock > 0.0 {
            let lock_level = 256.0 - self.rules.lock * (256.0 - LOCK_LEVEL_MIN);
            bb.locked |= bb.kick | bb.snare;
            for s in 0..fill_from {
                if lv.kick[s].max(lv.snare[s]) as f32 >= lock_level {
                    bb.locked |= bit(s);
                }
            }
        }

        if self.rules.break_layer {
            self.break_layer(ctx, &p, &rng, &bb, fill_from, wants_pickup && !picked_up, out);
        }
        self.hats(&p, &rng, &lv, &bb, out);
        self.ghosts(&p, &rng, &lv, &bb, out);
        self.perc(&p, &rng, &lv, &bb, out);
        out.sort();
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
    use crate::live::event::LiveEvent;
    use crate::live::generator::layout::{DrumClass, BREAK_BASE};
    use crate::live::generator::test_util::{assert_well_formed, ctx, render};

    const EMPTY: [u8; 16] = [0; 16];

    fn generator() -> DrumGenerator {
        DrumGenerator::with_layout(DrumLayout::standard())
    }

    fn with_rules(rules: DrumRules) -> DrumGenerator {
        DrumGenerator::new(Vec::new(), DrumLayout::standard(), PARAM_DEFAULTS, rules)
    }

    /// Kit without a break, so backbone tests see only one-shots.
    fn kit_only() -> DrumGenerator {
        DrumGenerator::with_layout(DrumLayout::empty().with_backbone(2).with_hats(2, 2).with_perc(2))
    }

    fn params(edit: impl FnOnce(&mut [f32; 8])) -> [f32; 8] {
        let mut p = PARAM_DEFAULTS;
        edit(&mut p);
        p
    }

    /// Steps (0-indexed) with an on-grid hit on `target`.
    fn steps(events: &[LiveEvent], target: u8) -> Vec<usize> {
        events
            .iter()
            .filter(|e| e.target == target && e.tick % TICKS_PER_STEP == 0)
            .map(|e| (e.tick / TICKS_PER_STEP) as usize)
            .collect()
    }

    fn bar(g: &DrumGenerator, seed: u32, bar: u8, p: &[f32; 8]) -> Vec<LiveEvent> {
        render(g, &ctx(seed, bar, p, &EMPTY)).as_slice().to_vec()
    }

    #[test]
    fn two_step_anchors() {
        // Research §7.2, 1-indexed: K 1, 11 / S 5, 13.
        let g = kit_only();
        for seed in 0..20 {
            let ev = bar(&g, seed, 0, &PARAM_DEFAULTS);
            assert_eq!(steps(&ev, 0), [0, 10], "kick, seed {seed}");
            assert!(steps(&ev, 1).contains(&4) && steps(&ev, 1).contains(&12), "snare, seed {seed}");
        }
    }

    #[test]
    fn half_time_anchors() {
        // K 1 / S 9.
        let g = kit_only();
        let p = params(|p| p[P_STYLE] = 0.0);
        let ev = bar(&g, 3, 0, &p);
        assert_eq!(steps(&ev, 0), [0]);
        assert!(steps(&ev, 1).contains(&8));
        assert!(!steps(&ev, 1).contains(&4));
    }

    #[test]
    fn other_archetype_anchors() {
        let g = kit_only();
        let at = |style: f32| bar(&g, 1, 0, &params(|p| p[P_STYLE] = style));
        // Rolling keeps the two-step backbone and adds a kick.
        let rolling = at(0.5);
        assert_eq!(steps(&rolling, 0), [0, 7, 10]);
        // Jungle: amen-style K 1, 3, 11, 12.
        assert_eq!(steps(&at(0.75), 0), [0, 2, 10, 11]);
        // Hardcore: four on the floor.
        assert_eq!(steps(&at(1.0), 0), [0, 4, 8, 12]);
    }

    #[test]
    fn half_time_flag_forces_the_feel() {
        let g = kit_only();
        for style in [0.25, 0.75, 1.0] {
            let p = params(|p| p[P_STYLE] = style);
            let mut c = ctx(5, 0, &p, &EMPTY);
            c.half_time = true;
            let out = render(&g, &c);
            assert_eq!(steps(out.as_slice(), 0), [0], "style {style}");
            let snares = steps(out.as_slice(), 1);
            assert!(snares.contains(&8) && !snares.contains(&4) && !snares.contains(&12));
        }
    }

    #[test]
    fn blend_and_hard_switch() {
        // Halfway between half-time and two-step.
        let p = params(|p| {
            p[P_STYLE] = 0.125;
            p[P_DENSITY] = 0.9;
            p[P_GHOST] = 0.0;
        });
        let kit = DrumLayout::empty().with_backbone(2);
        let blended = DrumGenerator::new(Vec::new(), kit, PARAM_DEFAULTS, DrumRules::default());
        let hard = DrumGenerator::new(Vec::new(), kit, PARAM_DEFAULTS, DrumRules { blend: false, ..DrumRules::default() });
        let b = bar(&blended, 1, 0, &p);
        let h = bar(&hard, 1, 0, &p);
        // Blended: both archetypes' snares show through.
        assert!(steps(&b, 1).contains(&4) && steps(&b, 1).contains(&8));
        // Hard switch at the midpoint is exactly the upper neighbour.
        let two_step = bar(&hard, 1, 0, &params(|q| {
            q[P_STYLE] = 0.25;
            q[P_DENSITY] = 0.9;
            q[P_GHOST] = 0.0;
        }));
        assert_eq!(h, two_step);
        assert_ne!(b, h);
        // On an archetype both modes agree.
        let on = params(|q| q[P_STYLE] = 0.5);
        assert_eq!(bar(&blended, 1, 0, &on), bar(&hard, 1, 0, &on));
    }

    #[test]
    fn deterministic_and_well_formed() {
        let g = generator();
        for seed in 0..40u32 {
            for b in 0..4u8 {
                for fill in [false, true] {
                    let p = params(|p| {
                        p[P_STYLE] = (seed % 9) as f32 / 8.0;
                        p[P_DENSITY] = (seed % 5) as f32 / 4.0;
                        p[P_CHOP] = (seed % 4) as f32 / 3.0;
                        p[P_SWING] = (seed % 3) as f32 / 2.0;
                    });
                    let mut c = ctx(seed, b, &p, &EMPTY);
                    c.fill = fill;
                    let one = render(&g, &c);
                    let two = render(&g, &c);
                    assert_eq!(one.as_slice(), two.as_slice());
                    assert_well_formed(&one);
                }
            }
        }
    }

    #[test]
    fn densest_bar_fits_the_capacity() {
        let g = generator();
        let p = [1.0; 8];
        for seed in 0..20 {
            let mut c = ctx(seed, 3, &p, &EMPTY);
            c.fill = true;
            let out = render(&g, &c);
            assert!(out.len() < crate::live::MAX_EVENTS_PER_BAR / 2, "{} events", out.len());
            assert_well_formed(&out);
        }
    }

    #[test]
    fn seeds_differ() {
        let g = generator();
        let p = params(|p| p[P_CHOP] = 0.8);
        assert_ne!(bar(&g, 1, 0, &p), bar(&g, 2, 0, &p));
    }

    #[test]
    fn density_never_removes_backbone_hits() {
        let g = kit_only();
        for style in 0..=8 {
            let mut prev: (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
            for d in 0..=20 {
                let p = params(|p| {
                    p[P_STYLE] = style as f32 / 8.0;
                    p[P_DENSITY] = d as f32 / 20.0;
                    // Ghosts share the snare target in this kit.
                    p[P_GHOST] = 0.0;
                });
                let ev = bar(&g, 7, 0, &p);
                let now = (steps(&ev, 0), steps(&ev, 1));
                assert!(prev.0.iter().all(|s| now.0.contains(s)), "kick lost at density {d}");
                assert!(prev.1.iter().all(|s| now.1.contains(s)), "snare lost at density {d}");
                prev = now;
            }
            assert!(!prev.0.is_empty());
        }
        // Zero density is silence.
        assert!(bar(&g, 7, 0, &params(|p| p[P_DENSITY] = 0.0)).is_empty());
    }

    #[test]
    fn overlays_grow_with_their_density() {
        let g = kit_only();
        let count = |idx: usize, v: f32, target: u8| {
            let p = params(|p| {
                p[P_HATS] = 0.0;
                p[P_GHOST] = 0.0;
                p[P_PERC] = 0.0;
                p[idx] = v;
            });
            (0..10).map(|seed| bar(&g, seed, 0, &p).iter().filter(|e| e.target == target).count()).sum::<usize>()
        };
        // Hats on target 8/9, percussion on 16/17.
        let hats = |v| count(P_HATS, v, 8) + count(P_HATS, v, 9);
        assert_eq!(hats(0.0), 0);
        assert!(hats(0.3) < hats(0.6) && hats(0.6) < hats(1.0));
        let perc = |v| count(P_PERC, v, 16) + count(P_PERC, v, 17);
        assert_eq!(perc(0.0), 0);
        assert!(perc(0.3) < perc(1.0));
        // Ghosts share the snare target: more ghosts = more snare-target hits.
        assert!(count(P_GHOST, 0.0, 1) < count(P_GHOST, 1.0, 1));
    }

    /// Steps the backbone occupies in a bar rendered without overlays.
    fn backbone_steps(g: &DrumGenerator, seed: u32, b: u8, p: &[f32; 8]) -> Vec<usize> {
        let bare = params(|q| {
            *q = *p;
            q[P_HATS] = 0.0;
            q[P_GHOST] = 0.0;
            q[P_PERC] = 0.0;
        });
        let ev = bar(g, seed, b, &bare);
        let mut s = steps(&ev, 0);
        s.extend(steps(&ev, 1));
        s
    }

    #[test]
    fn overlays_stay_off_the_backbone() {
        let layout = DrumLayout::empty().with_backbone(3).with_hats(2, 2).with_perc(2);
        let g = DrumGenerator::with_layout(layout);
        for seed in 0..30u32 {
            for style in [0.0, 0.25, 0.4, 0.5, 0.75, 1.0] {
                let p = params(|p| {
                    p[P_STYLE] = style;
                    p[P_HATS] = 1.0;
                    p[P_GHOST] = 1.0;
                    p[P_PERC] = 1.0;
                    p[P_VARIATION] = 0.0;
                });
                let locked = backbone_steps(&g, seed, 0, &p);
                assert!(!locked.is_empty());
                let ev = bar(&g, seed, 0, &p);
                let overlays: Vec<usize> = ev
                    .iter()
                    .filter(|e| !matches!(g.layout().class(e.target), DrumClass::Kick | DrumClass::Snare))
                    .map(|e| (e.tick / TICKS_PER_STEP) as usize)
                    .collect();
                assert!(!overlays.is_empty());
                for s in &overlays {
                    assert!(!locked.contains(s), "overlay on backbone step {s} (seed {seed}, style {style})");
                }
            }
        }
    }

    #[test]
    fn lock_off_lets_overlays_onto_the_backbone() {
        let layout = DrumLayout::empty().with_backbone(3).with_hats(2, 2);
        let g = DrumGenerator::new(Vec::new(), layout, PARAM_DEFAULTS, DrumRules { lock: 0.0, ..DrumRules::default() });
        let p = params(|p| p[P_HATS] = 1.0);
        let ev = bar(&g, 1, 0, &p);
        let hats: Vec<usize> = ev.iter().filter(|e| e.target >= 8).map(|e| (e.tick / TICKS_PER_STEP) as usize).collect();
        assert!(hats.contains(&0) && hats.contains(&4));
    }

    #[test]
    fn fill_bar_differs_and_keeps_the_first_half() {
        for g in [generator(), kit_only()] {
            for seed in 0..20 {
                let c = ctx(seed, 1, &PARAM_DEFAULTS, &EMPTY);
                let mut f = c;
                f.fill = true;
                let (normal, fill) = (render(&g, &c), render(&g, &f));
                assert_ne!(normal.as_slice(), fill.as_slice(), "seed {seed}");
                let first_half = |b: &BarEvents| -> Vec<LiveEvent> {
                    b.as_slice().iter().filter(|e| e.tick < step_tick(FILL_START_STEP)).copied().collect()
                };
                assert_eq!(first_half(&normal), first_half(&fill));
                // The fill is busier on the snare in its second half.
                let late_snares = |b: &BarEvents| {
                    b.as_slice().iter().filter(|e| e.target == 1 && e.tick >= step_tick(FILL_START_STEP)).count()
                };
                assert!(late_snares(&fill) > late_snares(&normal));
            }
        }
    }

    #[test]
    fn phrase_shape() {
        for g in [generator(), kit_only(), DrumGenerator::with_layout(DrumLayout::empty().with_break(&[SliceType::Other; 16], 1))] {
            for seed in 0..20 {
                let p = params(|p| p[P_CHOP] = 0.7);
                let b0 = bar(&g, seed, 0, &p);
                assert!(!b0.is_empty());
                assert_eq!(b0, bar(&g, seed, 1, &p));
                assert_eq!(b0, bar(&g, seed, 2, &p));
                assert_ne!(b0, bar(&g, seed, 3, &p), "seed {seed}");
                let still = params(|p| {
                    p[P_CHOP] = 0.7;
                    p[P_VARIATION] = 0.0;
                });
                assert_eq!(bar(&g, seed, 0, &still), bar(&g, seed, 3, &still));
            }
        }
    }

    #[test]
    fn phrase_rule_off_varies_every_bar() {
        let g = with_rules(DrumRules { phrase: false, ..DrumRules::default() });
        let p = params(|p| p[P_CHOP] = 0.9);
        assert_ne!(bar(&g, 1, 0, &p), bar(&g, 1, 1, &p));
    }

    fn slices(g: &DrumGenerator, ev: &[LiveEvent]) -> Vec<(usize, u8)> {
        ev.iter()
            .filter_map(|e| match g.layout().class(e.target) {
                DrumClass::Slice(i, _) => Some(((e.tick / TICKS_PER_STEP) as usize, i)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn break_plays_in_order_without_chop() {
        // No backbone one-shots and no density: nothing to align to.
        let g = DrumGenerator::with_layout(DrumLayout::empty().with_break(&[SliceType::Other; 16], 1));
        let p = params(|p| {
            p[P_CHOP] = 0.0;
            p[P_VARIATION] = 0.0;
        });
        let ev = bar(&g, 4, 0, &p);
        let expect: Vec<(usize, u8)> = (0..16).map(|s| (s, s as u8)).collect();
        assert_eq!(slices(&g, &ev), expect);
        assert!(ev.iter().all(|e| e.choke == 1 && e.key == 0 && e.gate == 0 && e.start == 0));
    }

    #[test]
    fn coarse_break_grid_uses_start_offsets() {
        // 8 slices per bar: in order, one event per slice.
        let g = DrumGenerator::with_layout(DrumLayout::empty().with_break(&[SliceType::Other; 8], 1));
        let still = params(|p| {
            p[P_CHOP] = 0.0;
            p[P_VARIATION] = 0.0;
        });
        let ev = bar(&g, 4, 0, &still);
        let expect: Vec<(usize, u8)> = (0..8).map(|i| (i * 2, i as u8)).collect();
        assert_eq!(slices(&g, &ev), expect);
        // Chopped in 16ths: second halves of slices appear, via `start`.
        let chopped = params(|p| p[P_CHOP] = 1.0);
        let mut saw_offset = false;
        for seed in 0..20 {
            for e in bar(&g, seed, 0, &chopped) {
                assert!(e.start == 0 || e.start == 32768);
                saw_offset |= e.start != 0;
            }
        }
        assert!(saw_offset);
    }

    #[test]
    fn two_bar_break_advances_with_the_bar() {
        let g = DrumGenerator::with_layout(DrumLayout::empty().with_break(&[SliceType::Other; 32], 1));
        let p = params(|p| {
            p[P_CHOP] = 0.0;
            p[P_VARIATION] = 0.0;
        });
        assert_eq!(slices(&g, &bar(&g, 1, 0, &p))[0], (0, 0));
        assert_eq!(slices(&g, &bar(&g, 1, 1, &p))[0], (0, 16));
        assert_eq!(slices(&g, &bar(&g, 1, 2, &p))[15], (15, 15));
    }

    #[test]
    fn break_mutations_keep_slice_types() {
        // No backbone, so only mutations move slices. With A4 on, a swapped
        // slice has the type of the original; a stutter repeats its neighbour.
        let layout = DrumLayout::empty().with_break(DrumLayout::standard().slices(), 1);
        let g = DrumGenerator::with_layout(layout);
        let types = g.layout().slices().to_vec();
        let p = params(|p| {
            p[P_CHOP] = 1.0;
            p[P_VARIATION] = 0.0;
        });
        let (mut moved, mut total) = (0, 0);
        for seed in 0..50 {
            let sl = slices(&g, &bar(&g, seed, 0, &p));
            assert_eq!(sl.len(), 16);
            for (i, &(step, slice)) in sl.iter().enumerate() {
                total += 1;
                if slice as usize == step {
                    continue;
                }
                moved += 1;
                let stutter = i > 0 && sl[i - 1].1 == slice;
                assert!(stutter || types[slice as usize] == types[step], "seed {seed} step {step}");
            }
        }
        // Mostly in order (A4), but audibly chopped.
        let share = moved as f32 / total as f32;
        assert!((0.1..0.5).contains(&share), "moved share {share}");
    }

    #[test]
    fn break_aligns_to_the_backbone() {
        let g = generator();
        let types = g.layout().slices().to_vec();
        for seed in 0..30 {
            for style in [0.0, 0.25, 0.5, 1.0] {
                let p = params(|p| {
                    p[P_STYLE] = style;
                    p[P_CHOP] = 1.0;
                });
                let ev = bar(&g, seed, 0, &p);
                let kicks = steps(&ev, 0);
                let snares: Vec<usize> = ev
                    .iter()
                    .filter(|e| e.target == 1 && e.flags & flags::ACCENT != 0)
                    .map(|e| (e.tick / TICKS_PER_STEP) as usize)
                    .collect();
                for (step, slice) in slices(&g, &ev) {
                    if kicks.contains(&step) {
                        assert_eq!(types[slice as usize], SliceType::Kick, "seed {seed} step {step}");
                    } else if snares.contains(&step) {
                        assert_eq!(types[slice as usize], SliceType::Snare, "seed {seed} step {step}");
                    }
                }
            }
        }
    }

    #[test]
    fn fill_shuffles_the_break() {
        let layout = DrumLayout::empty().with_break(&[SliceType::Other; 16], 1);
        let g = DrumGenerator::with_layout(layout);
        let p = params(|p| {
            p[P_CHOP] = 0.0;
            p[P_VARIATION] = 0.0;
        });
        let mut shuffled = 0;
        for seed in 0..20 {
            let mut c = ctx(seed, 0, &p, &EMPTY);
            c.fill = true;
            let out = render(&g, &c);
            for (step, slice) in slices(&g, out.as_slice()) {
                if step < FILL_START_STEP {
                    assert_eq!(slice as usize, step);
                } else if slice as usize != step {
                    shuffled += 1;
                }
            }
        }
        assert!(shuffled > 100);
    }

    #[test]
    fn break_layer_can_be_switched_off() {
        let g = with_rules(DrumRules { break_layer: false, ..DrumRules::default() });
        assert!(bar(&g, 1, 0, &PARAM_DEFAULTS).iter().all(|e| e.target < BREAK_BASE));
    }

    #[test]
    fn velocity_tiers_and_flat_mode() {
        let layout = DrumLayout::empty().with_backbone(3).with_hats(2, 2);
        let g = DrumGenerator::with_layout(layout);
        let p = params(|p| {
            p[P_HATS] = 1.0;
            p[P_GHOST] = 1.0;
        });
        let ev = bar(&g, 2, 0, &p);
        let max_of = |t: u8| ev.iter().filter(|e| e.target == t).map(|e| e.vel).max().unwrap();
        // Ghosts (target 2) sit well below the backbone snare.
        assert!((max_of(2) as f32) < max_of(1) as f32 * 0.5);
        // Hats vary by metric position.
        let hat_vels: std::collections::BTreeSet<u8> = ev.iter().filter(|e| e.target == 8).map(|e| e.vel / 16).collect();
        assert!(hat_vels.len() >= 2);
        let flat = DrumGenerator::new(Vec::new(), layout, PARAM_DEFAULTS, DrumRules { tiers: 0.0, ..DrumRules::default() });
        assert!(bar(&flat, 2, 0, &p).iter().all(|e| e.vel == VEL_NORMAL));
    }

    #[test]
    fn swing_moves_only_offbeat_hats_and_ghosts() {
        let layout = DrumLayout::empty().with_backbone(3).with_hats(2, 2);
        let g = DrumGenerator::with_layout(layout);
        let straight = params(|p| {
            p[P_HATS] = 1.0;
            p[P_GHOST] = 1.0;
            p[P_VARIATION] = 0.0;
        });
        let swung = params(|p| {
            *p = straight;
            p[P_SWING] = 1.0;
        });
        let a = bar(&g, 3, 0, &straight);
        assert!(a.iter().all(|e| e.tick % TICKS_PER_STEP == 0));
        let b = bar(&g, 3, 0, &swung);
        assert_eq!(a.len(), b.len());
        let mut moved = 0;
        for e in &b {
            let off = e.tick % TICKS_PER_STEP;
            if off != 0 {
                moved += 1;
                assert!(e.target >= 2, "backbone must not swing");
                assert_eq!((e.tick / TICKS_PER_STEP) % 2, 1);
            }
        }
        assert!(moved > 0);
    }

    #[test]
    fn buildup_accelerates_into_silence() {
        let order = ["two_step", "buildup"].iter().filter_map(|n| Archetype::builtin(n)).collect();
        let g = DrumGenerator::new(order, DrumLayout::standard(), PARAM_DEFAULTS, DrumRules::default());
        let p = params(|p| p[P_STYLE] = 1.0);
        let counts: Vec<usize> = (0..4).map(|b| bar(&g, 1, b, &p).iter().filter(|e| e.target == 1).count()).collect();
        assert_eq!(counts, [4, 8, 16, 16]);
        let last = bar(&g, 1, 3, &p);
        assert!(last.iter().all(|e| e.tick < BUILDUP_SILENT_TICK));
        assert!(last.iter().any(|e| e.tick % TICKS_PER_STEP != 0), "32nds in the last bar");
        // Crescendo across the phrase.
        let first = bar(&g, 1, 0, &p);
        let snare_vel = |ev: &[LiveEvent]| ev.iter().filter(|e| e.target == 1).map(|e| e.vel).max().unwrap();
        assert!(snare_vel(&first) < snare_vel(&last));
        // No break and no hats during the roll.
        assert!(last.iter().all(|e| e.target <= 1));
        // Below the switch point the two-step plays unchanged.
        let below = bar(&g, 1, 0, &params(|p| p[P_STYLE] = 0.4));
        assert_eq!(steps(&below, 0), [0, 10]);
    }

    #[test]
    fn empty_layout_renders_nothing() {
        let g = DrumGenerator::with_layout(DrumLayout::empty());
        for b in 0..4 {
            assert!(bar(&g, 1, b, &[1.0; 8]).is_empty());
        }
    }

    #[test]
    fn golden_two_step_bar() {
        // Pinned output: a change here changes how saved seeds sound.
        let g = kit_only();
        let p = params(|p| p[P_PERC] = 0.0);
        let ev = bar(&g, 1, 0, &p);
        let grid = crate::live::generator::test_util::grid(&ev, &[(0, 'K'), (1, 'S'), (8, 'h'), (9, 'o')]);
        assert_eq!(grid, GOLDEN_SEED_1, "\n{grid}");
    }

    const GOLDEN_SEED_1: &str = "\
K X.........X.....
S ....X..o.o..X...
h ..x...x.X.......
o ..............x.
";

    #[test]
    fn params_declared() {
        let g = generator();
        assert_eq!(g.param_names(), PARAM_NAMES);
        assert_eq!(g.param_defaults(), PARAM_DEFAULTS);
        assert!(g.param_names().len() <= MAX_GEN_PARAMS);
    }
}
