//! Lead generator: motifs, arps and stabs (design §6.3 rule A7).
//!
//! One generator with three modes, selected by the `mode` parameter:
//!
//! - **Motif** (`mode` < 1/3): a 2-4 note motif drawn from the seed and
//!   repeated. The same motif plays in every bar of the phrase; the last bar
//!   varies it (sequence shift, turnaround note).
//! - **Arp** (< 2/3): chord tones at a fixed rate, going up, down, up-down or
//!   in a seeded loop.
//! - **Stab** (else): chord stabs on offbeats.
//!
//! In every mode the lead is sparse where the drums are busy: its density is
//! scaled down by the drum mask's density and steps with a drum onset lose
//! importance (A7). The generator cannot see the bass lane, so "the bass's
//! rests" is approximated by the drum gaps both lanes follow.
//!
//! The registry names `lead`, `arp`, `stab` and `pad` are this generator with
//! different defaults ([`LeadKind`]). The idiom family reads the rhythm
//! importance from a style rhythm cell.
//!
//! Performance parameters, in order: `density`, `complexity`, `variation`,
//! `mode`, `register` (octave down / base / up), `rate` (arp rate: quarters,
//! dotted 8ths, 8ths, 16ths), `gate`, `spread` (0 = few notes close together,
//! 1 = wide moves and octave jumps over several octaves).
//!
//! Construction options: `rule_a3` (phrase shape), `rule_a5` (velocity
//! tiers), `rule_a7` (sparseness against the drums). All default to 1.

use std::collections::BTreeMap;

use super::bass::{register_shift, root_octave};
use super::blocks::{
    deja_vu, lerp, level_unit, metric_level, metric_weight, note, option, param, reveals, step_tick,
    velocity, LevelMap, PhraseRng, Tier,
};
use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::live::event::BarEvents;
use crate::live::harmony::SCALE_LEN;
use crate::live::rng;
use crate::live::{STEPS_PER_BAR, TICKS_PER_STEP};

pub const PARAM_NAMES: [&str; 8] =
    ["density", "complexity", "variation", "mode", "register", "rate", "gate", "spread"];
const P_DENSITY: usize = 0;
const P_COMPLEXITY: usize = 1;
const P_VARIATION: usize = 2;
const P_MODE: usize = 3;
const P_REGISTER: usize = 4;
const P_RATE: usize = 5;
const P_GATE: usize = 6;
const P_SPREAD: usize = 7;

const S_CELL: u32 = 0x300;
const S_MOTIF: u32 = 0x301;
const S_RHYTHM: u32 = 0x302;
const S_VEL: u32 = 0x303;
const S_ARP: u32 = 0x304;
const S_INVERSION: u32 = 0x305;
const S_SEQUENCE: u32 = 0x306;
const S_TURN: u32 = 0x307;
const S_OCTAVE: u32 = 0x308;
const S_VOICING: u32 = 0x309;

/// Most simultaneous notes of one hit.
const MAX_CHORD: usize = 4;

// MUSICAL: first draft, tune by ear. Sparseness against the drums (A7).
/// Share of the density taken away by a full drum mask.
const LEAD_DRUM_DUCK: f32 = 0.6;
/// Importance factor lost on a step with a drum onset.
const ONSET_PENALTY: f32 = 0.4;

// MUSICAL: first draft, tune by ear. Motif.
/// Weights of the motif's start step (steps 0..8): offbeats first.
const MOTIF_START_WEIGHTS: [f32; 8] = [1.0, 1.0, 3.0, 2.0, 1.0, 1.0, 3.0, 2.0];
/// Steps between motif notes and their weights.
const MOTIF_INTERVALS: [(usize, f32); 3] = [(1, 0.25), (2, 0.45), (3, 0.3)];
/// First motif note: a chord tone (root, 3rd, 5th).
const MOTIF_FIRST: [(i32, f32); 3] = [(0, 3.0), (2, 2.0), (4, 2.0)];
/// Scale-degree moves between motif notes, weighted at spread 0 and at
/// spread 1: repeats and steps when narrow, leaps when wide.
const MOTIF_MOVES: [(i32, f32, f32); 9] = [
    (0, 2.0, 0.0),
    (-1, 2.0, 1.0),
    (1, 2.0, 1.0),
    (-2, 0.5, 1.0),
    (2, 0.5, 1.0),
    (-3, 0.0, 0.6),
    (3, 0.0, 0.6),
    (-4, 0.0, 1.0),
    (4, 0.0, 1.0),
];
/// Chance per motif note of an octave jump at spread 1.
const MOTIF_OCTAVE_MAX: f32 = 0.4;
/// Motif notes stay within this many degrees of the start, at spread 0 and 1.
const MOTIF_RANGE: (f32, f32) = (5.0, 19.0);
/// Importance of the motif's first statement and of its repeat half a bar
/// later, and what each later note of the motif loses.
const MOTIF_REPEAT_IMPORTANCE: [f32; 2] = [1.0, 0.55];
const MOTIF_NOTE_FALLOFF: f32 = 0.1;
const MOTIF_REPEAT_STEPS: usize = 8;
/// How strongly a start step is avoided per motif note that would land on a
/// drum onset.
const MOTIF_COLLISION_WEIGHT: f32 = 4.0;
/// Degree shifts of the repeat in the variation bar (sequence).
const SEQUENCE_SHIFTS: [i32; 3] = [-1, 1, 2];
/// Degree shifts of the phrase-end turnaround note.
const TURN_SHIFTS: [i32; 3] = [1, -1, 2];

// MUSICAL: first draft, tune by ear. Arp.
/// Steps between arp notes by `rate`.
const ARP_INTERVALS: [usize; 4] = [4, 3, 2, 1];
/// Chord tones the arp can run over, low to high. Spread 0 uses the first 3
/// (one triad), spread 1 all of them (about two and a half octaves).
const ARP_TONES: [i32; 9] = [0, 2, 4, 7, 9, 11, 14, 16, 18];
const ARP_TONES_MIN: usize = 3;
/// Arp importance: this floor plus the rest by metric weight.
const ARP_IMPORTANCE_FLOOR: f32 = 0.6;
/// The random arp is a mostly locked loop of this many notes.
const ARP_LOOP_LEN: u8 = 8;
const ARP_DEJA_VU: f32 = 0.45;
/// The phrase-end arp note jumps an octave.
const ARP_TURN_SHIFT: i32 = 7;

// MUSICAL: first draft, tune by ear. Stabs and pads.
const STAB_IMPORTANCE: [u8; 16] = [60, 0, 200, 90, 0, 0, 230, 120, 70, 0, 200, 100, 0, 0, 255, 140];
const PAD_IMPORTANCE: [u8; 16] = [255, 0, 0, 0, 0, 0, 0, 0, 140, 0, 0, 0, 0, 0, 90, 0];
/// Chord tones by chord size (2, 3, 4 notes).
const STAB_CHORDS: [&[i32]; 3] = [&[0, 4], &[0, 2, 4], &[0, 2, 4, 6]];
/// Chance per upper chord tone of an octave up, and per hit of an octave
/// jump, at spread 1.
const STAB_OPEN_MAX: f32 = 0.5;
const STAB_JUMP_MAX: f32 = 0.3;
/// Stab rhythm randomness at complexity 1.
const STAB_CHAOS_MAX: f32 = 0.2;

// MUSICAL: first draft, tune by ear. Register and articulation.
/// Lowest MIDI key of the key's root at the base register (A3).
const LEAD_ROOT_MIN: i32 = 57;
const GATE_MIN_FRAC: f32 = 0.3;
const GATE_MIN_TICKS: f32 = 6.0;
/// Longest gate in steps for motif, arp and stab hits; pads hold to the next hit.
const GATE_MAX_STEPS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeadMode {
    Motif,
    Arp,
    Stab,
}

impl LeadMode {
    pub fn from_param(v: f32) -> Self {
        if v < 1.0 / 3.0 {
            LeadMode::Motif
        } else if v < 2.0 / 3.0 {
            LeadMode::Arp
        } else {
            LeadMode::Stab
        }
    }
}

/// The named presets of the lead generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeadKind {
    Lead,
    Arp,
    Stab,
    Pad,
}

impl LeadKind {
    // MUSICAL: first draft, tune by ear. Defaults per preset, in
    // `PARAM_NAMES` order.
    pub fn defaults(self) -> [f32; 8] {
        match self {
            LeadKind::Lead => [0.3, 0.3, 0.5, 0.0, 0.5, 0.6, 0.5, 0.4],
            LeadKind::Arp => [0.6, 0.1, 0.5, 0.5, 0.5, 0.6, 0.4, 0.3],
            LeadKind::Stab => [0.4, 0.4, 0.5, 1.0, 0.5, 0.6, 0.2, 0.3],
            LeadKind::Pad => [0.3, 0.5, 0.5, 1.0, 0.5, 0.6, 1.0, 0.2],
        }
    }

    /// Voice names of idiom level maps this preset reads.
    pub fn voices(self) -> &'static [&'static str] {
        match self {
            LeadKind::Lead => &["lead", "motif"],
            LeadKind::Arp => &["arp"],
            LeadKind::Stab => &["stab"],
            LeadKind::Pad => &["pad"],
        }
    }
}

/// Rule switches and scales, fixed at construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeadRules {
    /// A3: bars 0-2 repeat, bar 3 varies.
    pub phrase: bool,
    /// A5: velocity tier depth.
    pub tiers: f32,
    /// A7: how much the lead thins out against the drums.
    pub gaps: f32,
}

impl Default for LeadRules {
    fn default() -> Self {
        Self { phrase: true, tiers: 1.0, gaps: 1.0 }
    }
}

impl LeadRules {
    pub fn from_spec(params: &BTreeMap<String, f32>) -> Self {
        Self {
            phrase: option(params, "rule_a3", 1.0) >= 0.5,
            tiers: option(params, "rule_a5", 1.0),
            gaps: option(params, "rule_a7", 1.0),
        }
    }
}

/// One onset: up to [`MAX_CHORD`] chord-relative degrees on a step.
#[derive(Clone, Copy, Default)]
struct Hit {
    degrees: [i32; MAX_CHORD],
    size: u8,
    accent: bool,
    /// The degrees the repeating bars play here (for the turnaround).
    repeated: [i32; MAX_CHORD],
}

impl Hit {
    fn single(degree: i32, accent: bool) -> Self {
        let mut h = Hit { size: 1, accent, ..Hit::default() };
        h.degrees[0] = degree;
        h.repeated[0] = degree;
        h
    }
}

/// The hits of one bar by step.
type Plan = [Option<Hit>; STEPS_PER_BAR];

/// Lead / arp / stab / pad generator. See the module docs.
pub struct LeadGenerator {
    kind: LeadKind,
    defaults: [f32; MAX_GEN_PARAMS],
    rules: LeadRules,
    /// Idiom rhythm cells. Empty = procedural rhythm.
    cells: Vec<LevelMap>,
}

/// What every mode needs to place its hits.
struct Frame<'a> {
    ctx: &'a BarCtx<'a>,
    rng: PhraseRng,
    /// Density after ducking against the drums.
    density: f32,
    complexity: f32,
    spread: f32,
    variation: f32,
    cell: Option<&'a LevelMap>,
    gaps: f32,
}

impl Frame<'_> {
    /// Importance after the drum-onset penalty.
    fn ducked(&self, step: usize, importance: f32) -> f32 {
        if self.ctx.drum_mask[step] > 0 {
            importance * (1.0 - self.gaps * ONSET_PENALTY)
        } else {
            importance
        }
    }

    fn cell_level(&self, step: usize) -> Option<f32> {
        self.cell.map(|c| level_unit(c.at(self.ctx.bar_in_phrase, step)))
    }
}

fn pick<T: Copy>(h: u64, table: &[(T, f32)]) -> T {
    let mut weights = [0.0f32; 8];
    for (w, entry) in weights.iter_mut().zip(table) {
        *w = entry.1;
    }
    table[rng::weighted_pick(h, &weights[..table.len()]).unwrap_or(0)].0
}

impl LeadGenerator {
    pub fn new(kind: LeadKind, defaults: [f32; MAX_GEN_PARAMS], rules: LeadRules, cells: Vec<LevelMap>) -> Self {
        Self { kind, defaults, rules, cells }
    }

    /// Built-in defaults and rules for a preset.
    pub fn preset(kind: LeadKind) -> Self {
        let mut defaults = [0.0; MAX_GEN_PARAMS];
        defaults[..PARAM_NAMES.len()].copy_from_slice(&kind.defaults());
        Self::new(kind, defaults, LeadRules::default(), Vec::new())
    }

    fn motif(&self, f: &Frame) -> Plan {
        let mut plan: Plan = [None; STEPS_PER_BAR];
        let notes = 2 + (f.complexity * 2.999) as usize;
        // The motif is drawn once per seed, from the repeating coordinates.
        let h = f.rng.base.at(0, S_MOTIF);
        let mut offsets = [0usize; MAX_CHORD];
        let mut degrees = [0i32; MAX_CHORD];
        degrees[0] = pick(rng::derive(h, 10), &MOTIF_FIRST);
        let mut move_weights = [0.0f32; MOTIF_MOVES.len()];
        for (w, &(_, narrow, wide)) in move_weights.iter_mut().zip(&MOTIF_MOVES) {
            *w = lerp(narrow, wide, f.spread);
        }
        let range = lerp(MOTIF_RANGE.0, MOTIF_RANGE.1, f.spread) as i32;
        for i in 1..notes {
            offsets[i] = offsets[i - 1] + pick(rng::derive(h, i as u32), &MOTIF_INTERVALS);
            let step = MOTIF_MOVES[rng::weighted_pick(rng::derive(h, 10 + i as u32), &move_weights).unwrap_or(1)].0;
            let mut d = degrees[i - 1] + step;
            let hj = rng::derive(h, 20 + i as u32);
            if rng::chance(hj, f.spread * MOTIF_OCTAVE_MAX) {
                d += if rng::chance(rng::derive(hj, 1), 0.5) { SCALE_LEN } else { -SCALE_LEN };
            }
            // Fold back into the range by octaves (same pitch class).
            while d > range {
                d -= SCALE_LEN;
            }
            while d < -range {
                d += SCALE_LEN;
            }
            degrees[i] = d;
        }
        // A7: start where the motif's notes miss the drum onsets.
        let mut start_weights = MOTIF_START_WEIGHTS;
        for (start, w) in start_weights.iter_mut().enumerate() {
            let collisions = (0..notes)
                .filter(|&i| f.ctx.drum_mask[(start + offsets[i]) % STEPS_PER_BAR] > 0)
                .count();
            *w /= 1.0 + f.gaps * MOTIF_COLLISION_WEIGHT * collisions as f32;
        }
        let start = rng::weighted_pick(rng::derive(h, 0), &start_weights).unwrap_or(0);

        // Importance and motif note per step.
        let mut importance = [0.0f32; STEPS_PER_BAR];
        let mut note_of = [0usize; STEPS_PER_BAR];
        let mut repeat_of = [0usize; STEPS_PER_BAR];
        if f.cell.is_some() {
            let mut rank = 0;
            for s in 0..STEPS_PER_BAR {
                importance[s] = f.cell_level(s).unwrap_or(0.0);
                if importance[s] > 0.0 {
                    note_of[s] = rank % notes;
                    repeat_of[s] = rank / notes % 2;
                    rank += 1;
                }
            }
        } else {
            for (rep, &rep_importance) in MOTIF_REPEAT_IMPORTANCE.iter().enumerate() {
                for i in 0..notes {
                    let s = (start + offsets[i] + rep * MOTIF_REPEAT_STEPS) % STEPS_PER_BAR;
                    let v = rep_importance - MOTIF_NOTE_FALLOFF * i as f32;
                    if v > importance[s] {
                        importance[s] = v;
                        note_of[s] = i;
                        repeat_of[s] = rep;
                    }
                }
            }
        }

        // Variation bar: the repeat may move up or down a few degrees.
        let sequence = if f.rng.is_variation_bar() && f.rng.bar.chance(0, S_SEQUENCE, f.variation) {
            SEQUENCE_SHIFTS[rng::below(f.rng.bar.at(1, S_SEQUENCE), SEQUENCE_SHIFTS.len() as u32) as usize]
        } else {
            0
        };
        for s in 0..STEPS_PER_BAR {
            // Procedural motifs dodge the drums by placement; cells are fixed
            // in place and lose importance on drum onsets instead.
            let imp = if f.cell.is_some() { f.ducked(s, importance[s]) } else { importance[s] };
            if imp > 0.0 && reveals(imp, f.density, 0.0, 0.0) {
                let base = degrees[note_of[s]];
                let mut hit = Hit::single(base + if repeat_of[s] == 1 { sequence } else { 0 }, note_of[s] == 0);
                hit.repeated[0] = base;
                plan[s] = Some(hit);
            }
        }
        plan
    }

    fn arp(&self, f: &Frame, rate: f32) -> Plan {
        let mut plan: Plan = [None; STEPS_PER_BAR];
        let interval = ARP_INTERVALS[((rate * 3.999) as usize).min(ARP_INTERVALS.len() - 1)];
        let tones = ARP_TONES_MIN + (f.spread * (ARP_TONES.len() - ARP_TONES_MIN) as f32).round() as usize;
        let mut n = 0usize;
        for s in 0..STEPS_PER_BAR {
            // Idiom: the cell's steps. Procedural: every `interval` steps,
            // restarting each bar.
            let importance = match f.cell_level(s) {
                Some(level) => level,
                None if s % interval == 0 => {
                    ARP_IMPORTANCE_FLOOR + (1.0 - ARP_IMPORTANCE_FLOOR) * metric_weight(s)
                }
                None => 0.0,
            };
            if importance <= 0.0 {
                continue;
            }
            let index = if f.complexity < 0.25 {
                n % tones
            } else if f.complexity < 0.5 {
                tones - 1 - n % tones
            } else if f.complexity < 0.75 {
                let cycle = 2 * tones - 2;
                let k = n % cycle;
                if k < tones { k } else { cycle - k }
            } else {
                let h = deja_vu(&f.rng.base, n as u8, ARP_LOOP_LEN, ARP_DEJA_VU, S_ARP);
                rng::below(h, tones as u32) as usize
            };
            n += 1;
            if reveals(f.ducked(s, importance), f.density, 0.0, 0.0) {
                plan[s] = Some(Hit::single(ARP_TONES[index], metric_level(s) <= 2));
            }
        }
        plan
    }

    fn stab(&self, f: &Frame) -> Plan {
        let mut plan: Plan = [None; STEPS_PER_BAR];
        let table = if self.kind == LeadKind::Pad { &PAD_IMPORTANCE } else { &STAB_IMPORTANCE };
        let chord = STAB_CHORDS[((f.complexity * 2.999) as usize).min(STAB_CHORDS.len() - 1)];
        // Seeded inversion: the lowest tones move up an octave.
        let inversion = rng::below(f.rng.base.at(0, S_INVERSION), chord.len() as u32) as usize;
        let mut hit = Hit { size: chord.len() as u8, ..Hit::default() };
        for (i, &degree) in chord.iter().enumerate() {
            hit.degrees[i] = if i < inversion { degree + 7 } else { degree };
            // Spread opens the voicing: upper tones move up an octave.
            if i > 0 && f.rng.base.chance(i as u8, S_VOICING, f.spread * STAB_OPEN_MAX) {
                hit.degrees[i] += SCALE_LEN;
            }
        }
        hit.repeated = hit.degrees;
        let base = hit;
        for s in 0..STEPS_PER_BAR {
            let importance = f.cell_level(s).unwrap_or(level_unit(table[s]));
            if importance <= 0.0 {
                continue;
            }
            let chaos = f.complexity * STAB_CHAOS_MAX;
            // Pads are sustained: a drum hit under their onset does not matter.
            let importance = if self.kind == LeadKind::Pad { importance } else { f.ducked(s, importance) };
            if reveals(importance, f.density, chaos, f.rng.unit(s as u8, S_RHYTHM)) {
                hit = base;
                hit.accent = metric_level(s) <= 2;
                // Spread: single hits jump an octave up or down.
                let h = f.rng.at(s as u8, S_OCTAVE);
                if rng::chance(h, f.spread * STAB_JUMP_MAX) {
                    let shift = if rng::chance(rng::derive(h, 1), 0.5) { SCALE_LEN } else { -SCALE_LEN };
                    for d in &mut hit.degrees[..hit.size as usize] {
                        *d += shift;
                    }
                }
                plan[s] = Some(hit);
            }
        }
        plan
    }
}

impl LaneGenerator for LeadGenerator {
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let density = param(ctx, P_DENSITY);
        if density <= 0.0 {
            return;
        }
        let variation = param(ctx, P_VARIATION);
        let mode = LeadMode::from_param(param(ctx, P_MODE));
        let rng = PhraseRng::new(ctx, variation, self.rules.phrase);
        let drum_density = ctx.drum_mask.iter().filter(|&&v| v > 0).count() as f32 / STEPS_PER_BAR as f32;
        let frame = Frame {
            ctx,
            rng,
            density: density * (1.0 - self.rules.gaps * LEAD_DRUM_DUCK * drum_density),
            complexity: param(ctx, P_COMPLEXITY),
            spread: param(ctx, P_SPREAD),
            variation,
            cell: (!self.cells.is_empty())
                .then(|| &self.cells[rng::below(rng.base.at(0, S_CELL), self.cells.len() as u32) as usize]),
            gaps: self.rules.gaps,
        };
        let mut plan = match mode {
            LeadMode::Motif => self.motif(&frame),
            LeadMode::Arp => self.arp(&frame, param(ctx, P_RATE)),
            LeadMode::Stab => self.stab(&frame),
        };

        // Phrase-end turnaround: the last hit moves off what the repeating
        // bars play there.
        if rng.is_variation_bar() {
            if let Some(hit) = plan.iter_mut().rev().flatten().next() {
                let shift = match mode {
                    LeadMode::Arp => ARP_TURN_SHIFT,
                    _ => TURN_SHIFTS[rng::below(rng.bar.at(0, S_TURN), TURN_SHIFTS.len() as u32) as usize],
                };
                for i in 0..hit.size as usize {
                    hit.degrees[i] = hit.repeated[i] + shift;
                }
            }
        }

        let octave = root_octave(ctx.harmony.key, LEAD_ROOT_MIN) + register_shift(param(ctx, P_REGISTER));
        let gate = param(ctx, P_GATE);
        let max_gate_steps = if self.kind == LeadKind::Pad { STEPS_PER_BAR } else { GATE_MAX_STEPS };
        for s in 0..STEPS_PER_BAR {
            let Some(hit) = plan[s] else { continue };
            let next = (s + 1..STEPS_PER_BAR).find(|&t| plan[t].is_some()).unwrap_or(STEPS_PER_BAR);
            let span = ((next - s).min(max_gate_steps) as u16 * TICKS_PER_STEP) as f32;
            let gate_ticks = (span * lerp(GATE_MIN_FRAC, 1.0, gate)).max(GATE_MIN_TICKS).round() as u16;
            let tier = if hit.accent { Tier::Accent } else { Tier::Normal };
            let vel = velocity(tier, rng.unit(s as u8, S_VEL), self.rules.tiers);
            for i in 0..hit.size as usize {
                let key = ctx.harmony.resolve(hit.degrees[i], octave);
                out.push(note(step_tick(s), key, vel, gate_ticks, (s * MAX_CHORD + i) as u16 + 1));
            }
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
    use crate::live::event::{EventKind, LiveEvent};
    use crate::live::generator::test_util::{assert_well_formed, ctx, notes, render};
    use crate::live::harmony::scale_for;
    use crate::music::MusicalKey;

    const EMPTY: [u8; 16] = [0; 16];
    const TWO_STEP: [u8; 16] = [120, 0, 0, 0, 80, 0, 0, 0, 0, 0, 118, 0, 80, 0, 0, 0];
    const FULL: [u8; 16] = [100; 16];

    fn params(kind: LeadKind, edit: impl FnOnce(&mut [f32; MAX_GEN_PARAMS])) -> [f32; MAX_GEN_PARAMS] {
        let mut p = LeadGenerator::preset(kind).param_defaults();
        edit(&mut p);
        p
    }

    fn bar(g: &LeadGenerator, seed: u32, bar: u8, p: &[f32; MAX_GEN_PARAMS], mask: &[u8; 16]) -> Vec<LiveEvent> {
        let mut c = ctx(seed, bar, p, mask);
        c.lane = 2;
        render(g, &c).as_slice().to_vec()
    }

    fn steps(ev: &[LiveEvent]) -> Vec<usize> {
        let mut s: Vec<usize> = ev.iter().map(|e| (e.tick / TICKS_PER_STEP) as usize).collect();
        s.dedup();
        s
    }

    const KINDS: [LeadKind; 4] = [LeadKind::Lead, LeadKind::Arp, LeadKind::Stab, LeadKind::Pad];

    #[test]
    fn deterministic_well_formed_and_in_scale() {
        for kind in KINDS {
            let g = LeadGenerator::preset(kind);
            for seed in 0..40u32 {
                for b in 0..4u8 {
                    let p = params(kind, |p| {
                        p[P_DENSITY] = (seed % 5) as f32 / 4.0;
                        p[P_COMPLEXITY] = (seed % 4) as f32 / 3.0;
                        p[P_REGISTER] = (seed % 3) as f32 / 2.0;
                        p[P_RATE] = (seed % 7) as f32 / 6.0;
                    });
                    let key = MusicalKey::new((seed % 12) as u8, seed % 2 == 0);
                    let mut c = ctx(seed, b, &p, &TWO_STEP);
                    c.lane = 2;
                    c.harmony.key = key;
                    c.harmony.chord_degree = (seed % 7) as u8;
                    let one = render(&g, &c);
                    assert_eq!(one.as_slice(), render(&g, &c).as_slice());
                    assert_well_formed(&one);
                    let mut ids = std::collections::BTreeSet::new();
                    for e in one.as_slice() {
                        assert_eq!(e.kind, EventKind::NoteOn);
                        assert!(e.gate > 0);
                        assert!(ids.insert(e.note_id), "duplicate note id");
                        let pc = (e.key as i32 - key.root as i32).rem_euclid(12) as u8;
                        assert!(scale_for(key).contains(&pc));
                        assert!((30..=120).contains(&e.key), "key {} out of register", e.key);
                    }
                }
            }
        }
    }

    #[test]
    fn motif_is_short_and_repeats() {
        let g = LeadGenerator::preset(LeadKind::Lead);
        for seed in 0..40 {
            for (complexity, notes) in [(0.0, 2), (0.5, 3), (1.0, 4)] {
                // Low density: only the first statement.
                let sparse = params(LeadKind::Lead, |p| {
                    p[P_COMPLEXITY] = complexity;
                    p[P_DENSITY] = 0.4;
                });
                let first = bar(&g, seed, 0, &sparse, &EMPTY);
                assert_eq!(first.len(), notes, "seed {seed}");
                // High density: stated twice, half a bar apart, same pitches.
                let dense = params(LeadKind::Lead, |p| {
                    p[P_COMPLEXITY] = complexity;
                    p[P_DENSITY] = 1.0;
                });
                let both = bar(&g, seed, 0, &dense, &EMPTY);
                let mut keys: Vec<u8> = both.iter().map(|e| e.key).collect();
                keys.sort_unstable();
                keys.dedup();
                let mut motif_keys: Vec<u8> = first.iter().map(|e| e.key).collect();
                motif_keys.sort_unstable();
                motif_keys.dedup();
                assert_eq!(keys, motif_keys);
                assert!(both.len() > first.len());
            }
        }
    }

    #[test]
    fn density_drops_as_the_drums_get_busier() {
        for kind in KINDS {
            let g = LeadGenerator::preset(kind);
            let total = |mask: &[u8; 16]| -> usize {
                (0..60)
                    .map(|seed| {
                        let p = params(kind, |p| {
                            p[P_DENSITY] = 0.6;
                            p[P_VARIATION] = 0.0;
                        });
                        steps(&bar(&g, seed, 0, &p, mask)).len()
                    })
                    .sum()
            };
            let (quiet, medium, busy) = (total(&EMPTY), total(&TWO_STEP), total(&FULL));
            assert!(quiet > medium && medium > busy, "{kind:?}: {quiet} {medium} {busy}");
        }
        // Per seed it never rises (motif and arp have no rhythm randomness).
        for kind in [LeadKind::Lead, LeadKind::Arp] {
            let g = LeadGenerator::preset(kind);
            for seed in 0..60 {
                let p = params(kind, |p| p[P_DENSITY] = 0.6);
                let n = |mask: &[u8; 16]| bar(&g, seed, 0, &p, mask).len();
                assert!(n(&EMPTY) >= n(&TWO_STEP) && n(&TWO_STEP) >= n(&FULL));
            }
        }
    }

    #[test]
    fn drum_rule_off_ignores_the_mask() {
        let g = LeadGenerator::new(
            LeadKind::Lead,
            LeadGenerator::preset(LeadKind::Lead).param_defaults(),
            LeadRules { gaps: 0.0, ..LeadRules::default() },
            Vec::new(),
        );
        let p = params(LeadKind::Lead, |p| p[P_DENSITY] = 0.6);
        for seed in 0..20 {
            assert_eq!(bar(&g, seed, 0, &p, &EMPTY), bar(&g, seed, 0, &p, &FULL));
        }
    }

    #[test]
    fn key_change_keeps_the_rhythm() {
        for kind in KINDS {
            let g = LeadGenerator::preset(kind);
            for seed in 0..20 {
                let p = params(kind, |p| p[P_DENSITY] = 0.8);
                let mut c = ctx(seed, 3, &p, &TWO_STEP);
                c.lane = 2;
                let am = render(&g, &c);
                c.harmony.key = MusicalKey::new(4, true); // E minor
                let em = render(&g, &c);
                assert_eq!(am.len(), em.len());
                assert!(!am.is_empty());
                for (a, e) in am.as_slice().iter().zip(em.as_slice()) {
                    assert_eq!((a.tick, a.gate, a.vel, a.note_id), (e.tick, e.gate, e.vel, e.note_id));
                    assert_eq!((e.key as i32 - a.key as i32).rem_euclid(12), 7);
                }
            }
        }
    }

    #[test]
    fn phrase_shape() {
        for kind in KINDS {
            let g = LeadGenerator::preset(kind);
            for seed in 0..40 {
                let p = params(kind, |p| p[P_DENSITY] = 0.7);
                let b0 = bar(&g, seed, 0, &p, &TWO_STEP);
                assert!(!b0.is_empty());
                assert_eq!(b0, bar(&g, seed, 1, &p, &TWO_STEP));
                assert_eq!(b0, bar(&g, seed, 2, &p, &TWO_STEP));
                assert_ne!(b0, bar(&g, seed, 3, &p, &TWO_STEP), "{kind:?} seed {seed}");
                let still = params(kind, |p| {
                    p[P_DENSITY] = 0.7;
                    p[P_VARIATION] = 0.0;
                });
                assert_eq!(bar(&g, seed, 0, &still, &TWO_STEP), bar(&g, seed, 3, &still, &TWO_STEP));
            }
        }
    }

    /// Chord-tone index (0 = A, 1 = C, 2 = E) of each arp note in A minor.
    fn arp_tones(ev: &[LiveEvent]) -> Vec<u8> {
        ev.iter()
            .map(|e| match e.key % 12 {
                9 => 0,
                0 => 1,
                4 => 2,
                pc => panic!("not a chord tone: pitch class {pc}"),
            })
            .collect()
    }

    #[test]
    fn arp_directions_and_rates() {
        let g = LeadGenerator::preset(LeadKind::Arp);
        let at = |complexity: f32, rate: f32| {
            bar(&g, 1, 0, &params(LeadKind::Arp, |p| {
                p[P_DENSITY] = 1.0;
                p[P_COMPLEXITY] = complexity;
                p[P_RATE] = rate;
                // Four tones: root, 3rd, 5th, octave.
                p[P_SPREAD] = 1.0 / 6.0;
            }), &EMPTY)
        };
        // Rates: quarters, dotted 8ths, 8ths, 16ths.
        assert_eq!(steps(&at(0.0, 0.0)), [0, 4, 8, 12]);
        assert_eq!(steps(&at(0.0, 0.3)), [0, 3, 6, 9, 12, 15]);
        assert_eq!(steps(&at(0.0, 0.6)), [0, 2, 4, 6, 8, 10, 12, 14]);
        assert_eq!(at(0.0, 1.0).len(), 16);
        // Up: A C E A', rising keys within each cycle of four.
        let up = at(0.0, 0.6);
        assert_eq!(arp_tones(&up), [0, 1, 2, 0, 0, 1, 2, 0]);
        assert!(up[0].key < up[1].key && up[1].key < up[2].key && up[2].key < up[3].key);
        assert_eq!(up[3].key - up[0].key, 12);
        assert_eq!(up[4].key, up[0].key);
        // Down.
        let down = at(0.3, 0.6);
        assert!(down[0].key > down[1].key && down[1].key > down[2].key && down[2].key > down[3].key);
        // Up-down: peaks on the fourth note, back at the root on the seventh.
        let ud: Vec<u8> = at(0.6, 0.6).iter().map(|e| e.key).collect();
        assert_eq!(ud[4], ud[2]);
        assert_eq!(ud[6], ud[0]);
        // Random: chord tones only (checked by `arp_tones`), differs by seed.
        arp_tones(&at(1.0, 0.6));
        let random = |seed| bar(&g, seed, 0, &params(LeadKind::Arp, |p| {
            p[P_DENSITY] = 1.0;
            p[P_COMPLEXITY] = 1.0;
        }), &EMPTY);
        assert_ne!(random(1), random(2));
    }

    #[test]
    fn arp_thins_from_the_weak_steps() {
        let g = LeadGenerator::preset(LeadKind::Arp);
        let at = |density: f32| {
            steps(&bar(&g, 1, 0, &params(LeadKind::Arp, |p| {
                p[P_DENSITY] = density;
                p[P_RATE] = 1.0;
            }), &EMPTY))
        };
        assert_eq!(at(0.2), [0, 4, 8, 12]);
        assert_eq!(at(0.3), [0, 2, 4, 6, 8, 10, 12, 14]);
        assert_eq!(at(0.5).len(), 16);
    }

    #[test]
    fn stabs_are_offbeat_chords() {
        let g = LeadGenerator::preset(LeadKind::Stab);
        for seed in 0..20 {
            for (complexity, size) in [(0.0, 2), (0.5, 3), (1.0, 4)] {
                let p = params(LeadKind::Stab, |p| {
                    p[P_COMPLEXITY] = complexity;
                    p[P_DENSITY] = 0.25;
                    p[P_VARIATION] = 0.0;
                });
                let ev = bar(&g, seed, 0, &p, &EMPTY);
                assert!(!ev.is_empty());
                let hit_steps = steps(&ev);
                assert_eq!(ev.len(), hit_steps.len() * size);
                // The strongest stab steps are offbeat 8ths.
                assert!(hit_steps.iter().all(|s| s % 4 == 2), "seed {seed}: {hit_steps:?}");
                // Notes of one stab are distinct chord tones.
                for chunk in ev.chunks(size) {
                    let mut keys: Vec<u8> = chunk.iter().map(|e| e.key).collect();
                    keys.sort_unstable();
                    keys.dedup();
                    assert_eq!(keys.len(), size);
                    assert!(chunk.iter().all(|e| e.tick == chunk[0].tick && e.gate == chunk[0].gate));
                }
            }
        }
    }

    #[test]
    fn pads_hold_long_chords() {
        let g = LeadGenerator::preset(LeadKind::Pad);
        let p = params(LeadKind::Pad, |p| p[P_VARIATION] = 0.0);
        let ev = bar(&g, 1, 0, &p, &EMPTY);
        assert_eq!(steps(&ev), [0]);
        assert!(ev.iter().all(|e| e.gate == crate::live::TICKS_PER_BAR));
        let stab = LeadGenerator::preset(LeadKind::Stab);
        let short = bar(&stab, 1, 0, &params(LeadKind::Stab, |_| {}), &EMPTY);
        assert!(short.iter().all(|e| e.gate < 2 * TICKS_PER_STEP));
    }

    #[test]
    fn register_shifts_by_octaves() {
        for kind in KINDS {
            let g = LeadGenerator::preset(kind);
            let at = |r: f32| bar(&g, 5, 0, &params(kind, |p| p[P_REGISTER] = r), &EMPTY);
            let (low, mid, high) = (at(0.0), at(0.5), at(1.0));
            assert!(!mid.is_empty());
            for ((l, m), h) in low.iter().zip(&mid).zip(&high) {
                assert_eq!((m.key - l.key, h.key - m.key), (12, 12));
            }
        }
    }

    #[test]
    fn idiom_cell_sets_the_rhythm() {
        let mut levels = [0u8; 16];
        for (s, l) in [(2, 255), (5, 200), (8, 150), (13, 90)] {
            levels[s] = l;
        }
        for kind in KINDS {
            let g = LeadGenerator::new(
                kind,
                LeadGenerator::preset(kind).param_defaults(),
                LeadRules::default(),
                vec![LevelMap::bar(levels)],
            );
            let at = |d: f32| {
                steps(&bar(&g, 3, 0, &params(kind, |p| {
                    p[P_DENSITY] = d;
                    p[P_COMPLEXITY] = 0.0;
                    p[P_VARIATION] = 0.0;
                }), &EMPTY))
            };
            assert_eq!(at(0.5), [2, 5, 8], "{kind:?}");
            assert_eq!(at(1.0), [2, 5, 8, 13], "{kind:?}");
        }
    }

    #[test]
    fn spread_widens_the_pitch_range() {
        for (mode, kind) in [(0.0, LeadKind::Lead), (0.5, LeadKind::Arp), (1.0, LeadKind::Stab)] {
            let g = LeadGenerator::preset(kind);
            let range = |spread: f32| -> i32 {
                (0..100u32)
                    .map(|seed| {
                        let p = params(kind, |p| {
                            p[P_MODE] = mode;
                            p[P_DENSITY] = 0.8;
                            p[P_COMPLEXITY] = 1.0;
                            p[P_SPREAD] = spread;
                        });
                        let keys: Vec<i32> = bar(&g, seed, 0, &p, &EMPTY).iter().map(|e| e.key as i32).collect();
                        keys.iter().max().unwrap_or(&0) - keys.iter().min().unwrap_or(&0)
                    })
                    .sum()
            };
            let (narrow, wide) = (range(0.0), range(1.0));
            assert!(narrow * 3 < wide * 2, "{kind:?}: range {narrow} vs {wide}");
        }
    }

    #[test]
    fn golden_bars() {
        // Pinned output: a change here changes how saved seeds sound.
        let got: Vec<String> = KINDS
            .iter()
            .map(|&kind| {
                let g = LeadGenerator::preset(kind);
                notes(&bar(&g, 1, 0, &params(kind, |_| {}), &TWO_STEP))
            })
            .collect();
        assert_eq!(got, GOLDEN, "\n{got:#?}");
    }

    const GOLDEN: [&str; 4] = [
        "3:64 6:65",
        "0:57 2:60 4:64 6:69 8:72 12:60 14:64",
        "2:69 2:60 2:64 6:69 6:60 6:64 14:69 14:60 14:64",
        "0:69 0:60 0:64",
    ];

    #[test]
    fn modes_from_param() {
        assert_eq!(LeadMode::from_param(0.0), LeadMode::Motif);
        assert_eq!(LeadMode::from_param(0.5), LeadMode::Arp);
        assert_eq!(LeadMode::from_param(1.0), LeadMode::Stab);
        for kind in KINDS {
            let g = LeadGenerator::preset(kind);
            assert_eq!(g.param_names(), PARAM_NAMES);
            assert_eq!(g.param_defaults()[..8], kind.defaults());
        }
    }
}
