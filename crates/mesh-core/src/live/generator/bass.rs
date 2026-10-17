//! Bass generator (design §6.3 rule A6; research §7.1.2, §7.1.7).
//!
//! - **Rhythm** comes from the gaps in the drums: metric importance, raised
//!   on steps the drum backbone leaves empty. Kick steps are either avoided,
//!   left alone, or doubled (`kick_mode`). `density` reveals steps in order of
//!   importance and `complexity` shifts the importance towards syncopation.
//! - **Pitch** is a root-heavy pool of chord-relative scale degrees (root,
//!   octave, 5th, 7th, 3rd — b7 and b3 in a minor key) resolved against the
//!   bar's chord, so the line follows the progression and any key change.
//! - **Articulation**: accents, slides (`TIE` + glide), and note length from
//!   the density: sparse lines hold long notes, dense lines play short ones.
//! - **Phrase shape**: bars 0-2 repeat, bar 3 re-draws some steps and ends
//!   on a turnaround note.
//!
//! The idiom family uses the same generator with the rhythm importance read
//! from a style rhythm cell instead of the metric tables.
//!
//! Performance parameters, in order: `density`, `complexity`, `variation`,
//! `kick_mode` (< 1/3 avoid the kick, < 2/3 ignore it, else double it),
//! `register` (octave down / base / up), `slide`, `accent`, `spread`
//! (size of the "odd block" jumps: most notes stay in one octave, and a
//! seeded run of 1-3 notes per bar may jump up; 0 = no jumps, then a 5th, an
//! octave, an octave and a 3rd, up to an octave and a 5th (+19 semitones)).
//! `length` and `expression` (0.5 = unchanged) act as on the `phrase`
//! generator, so a sound pack's shape control works on generated lines:
//! length shortens notes towards stabs or holds and ties them, expression
//! scales each note's filter offset. `stabs` (0.5 = as the density says)
//! sets the share of stab notes (flagged `STAB`: no sustain, the stab
//! accent) against held ones.
//!
//! **Motif mode**: with motif clips loaded ([`crate::live::motif`]) and the
//! construction option `motifs` not 0, the seed picks a clip and the line
//! plays it as written (see [`super::motif`]): `density` thins or repeats
//! notes and sets their length, `spread` lifts weak notes an octave,
//! `variation` changes the phrase's last bar, `register` shifts octaves.
//! The pitch and rhythm never change per bar. `complexity` and `kick_mode`
//! do not act. Accent, slide and the note filter still vary per note.
//!
//! Construction options: `rule_a3` (phrase shape), `rule_a5` (velocity
//! tiers), `rule_a6` (gap-following and root weighting). All default to 1.
//! `motifs` = 0 turns motif mode off.

use std::collections::BTreeMap;

use super::blocks::{
    density_attack_in, lerp, note_param, step_motif, level_unit, note, option, param, reveals, step_tick, stream_start, velocity, LevelMap,
    PhraseRng, Tier, BASS_ATTACK_BY_DENSITY,
};
use super::blocks::{VARIATION_BAR, VEL_ACCENT};
use super::layout::MASK_KICK_MIN;
use super::motif::{self, Plan, Play, Shape};
use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::live::event::{flags, BarEvents, LiveEvent};
use crate::live::harmony::SCALE_LEN;
use crate::live::motif::Motif;
use crate::live::rng;
use crate::live::sampler::CUTOFF_OFFSET_OCTAVES;
use crate::live::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};
use crate::music::MusicalKey;

pub const PARAM_NAMES: [&str; 11] =
    ["density", "complexity", "variation", "kick_mode", "register", "slide", "accent", "spread", "length", "expression", "stabs"];
const P_DENSITY: usize = 0;
const P_COMPLEXITY: usize = 1;
const P_VARIATION: usize = 2;
const P_KICK_MODE: usize = 3;
const P_REGISTER: usize = 4;
const P_SLIDE: usize = 5;
const P_ACCENT: usize = 6;
const P_SPREAD: usize = 7;
const P_LENGTH: usize = 8;
const P_EXPRESSION: usize = 9;
const P_STABS: usize = 10;

// MUSICAL: first draft, tune by ear.
pub const PARAM_DEFAULTS: [f32; MAX_GEN_PARAMS] = [0.5, 0.4, 0.5, 0.0, 0.5, 0.5, 0.5, 0.3, 0.5, 0.5, 0.5, 0.0];

const S_CELL: u32 = 0x200;
const S_RHYTHM: u32 = 0x201;
const S_PITCH: u32 = 0x202;
const S_ACCENT: u32 = 0x203;
const S_SLIDE: u32 = 0x204;
const S_VEL: u32 = 0x205;
const S_TURN: u32 = 0x206;
const S_LENGTH: u32 = 0x20B;
const S_PERSONA: u32 = 0x210;
const S_HALF: u32 = 0x211;
const S_RATCHET: u32 = 0x212;
const S_LEGATO: u32 = 0x213;
const S_FIGURE: u32 = 0x214;
const S_GROOVE: u32 = 0x215;
const S_REGION: u32 = 0x216;
const S_FILTER: u32 = 0x20D;
const S_JUMP: u32 = 0x207;
const S_JUMP_START: u32 = 0x208;
const S_JUMP_LEN: u32 = 0x209;
const S_JUMP_SIZE: u32 = 0x20A;

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
// Root, octave, 5th, 7th, 3rd above, and the 5th and 7th below the root.
const BASS_POOL: [(i32, f32); 7] = [(0, 6.0), (7, 1.5), (4, 2.0), (6, 1.0), (2, 1.0), (-3, 2.0), (-1, 1.5)];
/// Weight factor of the non-root degrees at spread 0 and 1 (spread moves the
/// line off the root more often).
const POOL_FACTOR: (f32, f32) = (0.2, 1.3);
/// Notes below the root may go this many semitones under it, and never more
/// than `BELOW_KEY_FLOOR` under the key's root in the register.
const BELOW_ROOT_SEMITONES: i32 = 8;
const BELOW_KEY_FLOOR: i32 = 4;
/// Rhythm density at density 0 and 1, eased in with density to this power
/// (user, 2026-10-17: at normal speed the middle plays about 4 notes a bar,
/// 2.7 at the bottom, 13 at the top).
const RHYTHM_DENSITY: (f32, f32) = (0.3, 1.0);
const RHYTHM_CURVE: f32 = 3.0;
/// Importance taken off the off-16th steps at density 0, so sparse lines sit
/// on the 8ths and hold their notes longer. Fades out by `THIN_DENSITY_MAX`.
const THIN_PENALTY: f32 = 0.35;
const THIN_DENSITY_MAX: f32 = 0.6;
/// Share of short, stabby notes at density 0 and 1; the rest are held.
const SHORT_SHARE: (f32, f32) = (0.2, 0.8);
/// `stabs` 0..1 moves the stab share by this much around the density's
/// (± half of it), within the range.
const STABS_SHIFT: f32 = 1.2;
const STABS_RANGE: (f32, f32) = (0.04, 0.96);
/// Short note: share of the gap at density 0 and 1, and longest length in steps.
const SHORT_LENGTH: (f32, f32, u16) = (0.6, 0.35, 2);
// MUSICAL: first draft, tune by ear. Odd-block jumps (spread = jump size).
/// Jump sizes in scale degrees: 5th, octave, octave + 3rd, octave + 5th.
const JUMPS: [i32; 4] = [4, 7, 9, 11];
/// Spread below this: no odd blocks (the walk keeps the line close).
const JUMP_SPREAD_MIN: f32 = 0.4;
/// Chance that a bar has an odd block.
const JUMP_BLOCK_PROB: (f32, f32) = (0.15, 0.7);
/// Longest odd block in notes.
const JUMP_BLOCK_MAX: u32 = 3;
/// Share of blocks that take the largest jump the spread allows.
const JUMP_LARGEST_SHARE: f32 = 0.6;
/// Degrees a phrase-end turnaround note may take (5th, 7th).
const TURNAROUND_DEGREES: [i32; 2] = [4, 6];
/// Steps tried for the turnaround note, last first.
const TURNAROUND_STEPS: [usize; 3] = [15, 14, 13];

// MUSICAL: first draft, tune by ear (user, 2026-10-17: each seed its own
// rhythmic character). Seed personality, drawn once per letter seed:
/// Syncopation offset added to `complexity`.
const PERSONA_SYNC: (f32, f32) = (-0.2, 0.35);
/// Slide probability multiplier.
const PERSONA_SLIDE: (f32, f32) = (0.3, 2.5);
/// Most likely share of beats played at half speed (only the beat's first
/// note, held), and of notes doubled into two 32nds (ratchets).
const PERSONA_HALF_MAX: f32 = 0.35;
const PERSONA_RATCHET_MAX: f32 = 0.3;
/// How much tonal stability steers the pitch walk (so seeds differ: some
/// circle the chord tones, some wander the scale).
const PERSONA_TONAL: (f32, f32) = (0.3, 2.0);

// MUSICAL: first draft, tune by ear. Motif mode.
/// Lowest MIDI key a motif bar may reach (C1); lower bars move up an octave.
const MOTIF_MIN_KEY: i32 = 24;
/// Most notes of a bar after the density's repeats.
const MOTIF_MAX_NOTES: usize = 12;
/// Written velocity from which a note is an accent.
const MOTIF_ACCENT_VEL: u8 = 110;

// MUSICAL: first draft, tune by ear. Register and articulation.
/// Lowest MIDI key of the key's root at the base register (E1).
const BASS_ROOT_MIN: i32 = 28;
/// Notes more than this many semitones above the root drop an octave (odd
/// blocks are exempt).
const BASS_SPAN: f32 = 12.0;
/// Accent / slide probability at parameter value 1 (the defaults of 0.5 give
/// the acid-style 30 % accents and 10 % slides).
const ACCENT_PROB_MAX: f32 = 0.6;
const SLIDE_PROB_MAX: f32 = 0.2;
// MUSICAL: first draft, tune by ear (user: a seeded, moving lowpass).
/// Where a note's lowpass sits between the lane's filter range (0 = 700 Hz,
/// 1 = 15 kHz, see `lane::BASS_FILTER`): the seed's base, then the density,
/// a repeating step motif, the pitch (per octave above the root) and the
/// accent push it up.
const FILTER_BASE: (f32, f32) = (0.35, 0.8);
const FILTER_DENSITY: f32 = 0.25;
const FILTER_MOTIF: f32 = 0.7;
const FILTER_PITCH: f32 = 0.15;
const FILTER_ACCENT: f32 = 0.15;
/// Half the filter range in octaves (700 Hz .. 15 kHz around 3.24 kHz).
const FILTER_HALF_OCTAVES: f32 = 2.21;
/// A slide needs the previous note at most this many steps back.
const SLIDE_MAX_GAP_STEPS: usize = 4;
/// Glide time byte (log scale, about 60 ms).
const SLIDE_GLIDE: u8 = 138;
/// Ticks a slid-from note overlaps the next one so the tie finds it held.
const TIE_OVERLAP_TICKS: u16 = 2;
// MUSICAL: first draft, tune by ear (user: note length follows the density).
/// Note length as a share of the distance to the next note, at density 0
/// (sustained) and 1 (short).
const GATE_BY_DENSITY: (f32, f32) = (1.0, 0.3);
/// Longest note in steps at density 0 and 1.
const GATE_MAX_BY_DENSITY: (f32, f32) = (16.0, 4.0);
const GATE_MIN_TICKS: f32 = 6.0;
// MUSICAL: the phrase generator's length and expression (generator/phrase.rs),
// so a sound pack's shape control acts the same on generated lines.
/// Shortest stab at length 0, in ticks (a 16th and a half).
const STAB_TICKS: u16 = 36;
/// From this length held notes start to tie into the next one and glide;
/// at length 1 all of them do (user, 2026-10-17: the sustained end of shape
/// forms one line).
const LEGATO_FROM: f32 = 0.75;

/// Share of held notes that tie into the next at `length`.
fn legato_share(length: f32) -> f32 {
    ((length - LEGATO_FROM) / (1.0 - LEGATO_FROM)).clamp(0.0, 1.0)
}

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
    /// Play motif clips when the generator has them.
    pub motifs: bool,
    /// Each seed's rhythmic character ([`Persona`]): syncopation, slides,
    /// half-speed beats and ratchets.
    pub persona: bool,
    /// Pitch as a walk from the previous note, its steps sized by `spread`
    /// ([`walk_step`]); off: each note drawn from the root-heavy pool.
    pub walk: bool,
    /// Rhythm from the built-in groove vocabulary ([`GROOVES`]) and repeats
    /// only as fills; off: the step-importance rhythm everywhere.
    pub scheme: bool,
}

impl Default for BassRules {
    fn default() -> Self {
        Self { phrase: true, tiers: 1.0, gaps: 1.0, root: 1.0, motifs: true, persona: true, walk: true, scheme: true }
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
            motifs: option(params, "motifs", 1.0) >= 0.5,
            persona: option(params, "persona", 1.0) >= 0.5,
            walk: option(params, "walk", 1.0) >= 0.5,
            scheme: option(params, "scheme", 1.0) >= 0.5,
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
    /// Motif clips. Empty = the procedural line.
    motifs: Vec<Motif>,
}

impl BassGenerator {
    pub fn new(defaults: [f32; MAX_GEN_PARAMS], rules: BassRules, cells: Vec<LevelMap>) -> Self {
        Self { defaults, rules, cells, motifs: Vec::new() }
    }

    /// Play these motif clips (when the rules allow it).
    pub fn with_motifs(mut self, motifs: Vec<Motif>) -> Self {
        self.motifs = motifs;
        self
    }

    fn uses_motifs(&self) -> bool {
        self.rules.motifs && !self.motifs.is_empty()
    }

    /// Draw a chord-relative degree from the pool.
    /// Event `cutoff` of a note on `step`, `above` semitones over the root.
    fn cutoff(&self, ctx: &BarCtx, step: u8, above: i32, accented: bool) -> i8 {
        let base = rng::range_f32(rng::hash(ctx.seed, ctx.lane, 0, 0, S_FILTER), FILTER_BASE.0, FILTER_BASE.1);
        let u = base
            + FILTER_DENSITY * note_param(ctx, P_DENSITY, step)
            + FILTER_MOTIF * (step_motif(ctx, step, S_FILTER) - 0.5)
            + FILTER_PITCH * above as f32 / 12.0
            + if accented { FILTER_ACCENT } else { 0.0 };
        let octaves = (u.clamp(0.0, 1.0) * 2.0 - 1.0) * FILTER_HALF_OCTAVES;
        (octaves / CUTOFF_OFFSET_OCTAVES * 127.0).round().clamp(-127.0, 127.0) as i8
    }

    /// Motif mode: the seed's clip, re-pitched to the bar's key, mode and
    /// chord. Only slide, accent and the note filter vary per note.
    fn motif_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let density = param(ctx, P_DENSITY);
        let variation = param(ctx, P_VARIATION);
        let clip = &self.motifs[motif::pick(&self.motifs, ctx.seed, ctx.lane, &[])];
        let shape = Shape {
            density,
            spread: param(ctx, P_SPREAD),
            variation,
            max_notes: MOTIF_MAX_NOTES,
            lead: false,
            play: Play::Whole,
            yield_to: 0,
            yield_prob: 0.0,
        };
        let mut plan = Plan::new();
        motif::plan_bar(clip, ctx, &shape, &mut plan);
        if plan.as_slice().is_empty() {
            return;
        }
        let octave = root_octave(ctx.harmony.key, BASS_ROOT_MIN) + register_shift(param(ctx, P_REGISTER));
        let fold = motif::fold(ctx, octave, &plan, MOTIF_MIN_KEY);
        let root_key = ctx.harmony.resolve(0, octave) as i32 + fold;
        let rng = PhraseRng::new(ctx, variation, self.rules.phrase);
        let attack = density_attack_in(density, BASS_ATTACK_BY_DENSITY);
        // (tick, key) of the previous note, for slides.
        let mut prev: Option<(u16, i32)> = None;
        let mut events = [LiveEvent::default(); motif::MAX_PLAN];
        let mut n = 0usize;
        for (i, p) in plan.as_slice().iter().enumerate() {
            let step = p.step() as u8;
            let key = motif::note_key(ctx, octave, fold, p).clamp(0, 127);
            let random_accent = rng.chance(step, S_ACCENT, note_param(ctx, P_ACCENT, step) * ACCENT_PROB_MAX);
            let accented = random_accent || p.vel >= MOTIF_ACCENT_VEL;
            let vel = if random_accent { p.vel.max(VEL_ACCENT) } else { p.vel };
            let mut ev = note(p.tick, key as u8, vel, p.gate, i as u16 + 1);
            ev.attack = attack;
            ev.start = stream_start(ctx, step, 0);
            ev.cutoff = self.cutoff(ctx, step, key - root_key, accented);
            if accented {
                ev.flags |= flags::ACCENT;
            }
            if let Some((prev_tick, prev_key)) = prev {
                if p.tick - prev_tick <= SLIDE_MAX_GAP_STEPS as u16 * TICKS_PER_STEP
                    && key != prev_key
                    && rng.chance(step, S_SLIDE, note_param(ctx, P_SLIDE, step) * SLIDE_PROB_MAX)
                {
                    ev.flags |= flags::TIE;
                    ev.glide = SLIDE_GLIDE;
                    // Hold the previous note into this one.
                    events[n - 1].gate = p.tick - prev_tick + TIE_OVERLAP_TICKS;
                }
            }
            events[n] = ev;
            n += 1;
            prev = Some((p.tick, key));
        }
        shape_notes(&mut events[..n], param(ctx, P_LENGTH), param(ctx, P_EXPRESSION), |step| rng.unit(step, S_LEGATO));
        for ev in &events[..n] {
            out.push(*ev);
        }
    }

    fn pick_degree(&self, h: u64, spread: f32) -> i32 {
        if !rng::chance(rng::derive(h, 1), self.rules.root) {
            return rng::below(h, SCALE_LEN as u32) as i32;
        }
        let factor = lerp(POOL_FACTOR.0, POOL_FACTOR.1, spread);
        let mut weights = [0.0f32; BASS_POOL.len()];
        for (i, &(degree, w)) in BASS_POOL.iter().enumerate() {
            weights[i] = if degree == 0 { w } else { w * factor };
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
        if self.uses_motifs() {
            self.motif_bar(ctx, out);
            return;
        }
        let complexity = param(ctx, P_COMPLEXITY);
        let persona = if self.rules.persona { Persona::new(ctx) } else { Persona::PLAIN };
        let variation = param(ctx, P_VARIATION);
        let mode = KickMode::from_param(param(ctx, P_KICK_MODE));
        let rng = PhraseRng::new(ctx, variation, self.rules.phrase);
        let cell = (!self.cells.is_empty())
            .then(|| &self.cells[rng::below(rng.base.at(0, S_CELL), self.cells.len() as u32) as usize]);
        // The built-in groove scheme (user, 2026-10-17): a seeded figure from
        // the bass vocabulary, picked by syncopation (complexity).
        let groove = (self.rules.scheme && cell.is_none())
            .then(|| pick_groove(rng.base.at(0, S_GROOVE), (complexity + persona.sync).clamp(0.0, 1.0)));
        let is_kick = |s: usize| ctx.drum_mask[s] >= MASK_KICK_MIN;
        // Each step reads its own density (per-note variation).
        let step_density = |d: f32| lerp(RHYTHM_DENSITY.0, RHYTHM_DENSITY.1, d.powf(RHYTHM_CURVE));
        let thin = THIN_PENALTY * (1.0 - density / THIN_DENSITY_MAX).max(0.0);

        // Rhythm.
        let mut onsets = 0u16;
        if let Some(gv) = groove {
            // Density reveals the figure's levels: the core first, then the
            // answers, the anticipations last.
            // Stabs leave holes, so a stabby, dense line also fills the
            // figure's empty 8ths (user, 2026-10-17).
            // The fill comes from the figure itself: its own hits again a beat or
            // half a bar later (by the seed), on the 8th grid, then any empty 8th.
            let stab_fill = (2.0 * param(ctx, P_STABS) - 1.0).max(0.0);
            let shift = if rng::chance(rng.base.at(0, S_GROOVE + 1), 0.5) { 4 } else { 8 };
            for s in 0..STEPS_PER_BAR {
                let echo = if s % 2 == 0 { (gv.levels[(s + STEPS_PER_BAR - shift) % STEPS_PER_BAR] as f32 * GROOVE_ECHO) as u8 } else { 0 };
                let own = gv.levels[s];
                let level = if own == 0 && s % 2 == 0 { echo.max(GROOVE_FILL_LEVEL) } else { own };
                if level == 0 {
                    continue;
                }
                let d = note_param(ctx, P_DENSITY, s as u8);
                let threshold =
                    (lerp(GROOVE_REVEAL.0, GROOVE_REVEAL.1, d) - (GROOVE_FILL_REACH + GROOVE_STAB_REACH * stab_fill) * d) as i32;
                // The core plays with the kick; the rest leaves it room.
                let kick_clash = is_kick(s) && mode == KickMode::Avoid && level < 255;
                if level as i32 >= threshold && !kick_clash {
                    onsets |= 1 << s;
                }
            }
        }
        for s in 0..STEPS_PER_BAR {
            if groove.is_some() {
                break;
            }
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
                    let cx = (note_param(ctx, P_COMPLEXITY, s as u8) + persona.sync).clamp(0.0, 1.0);
                    lerp(BASS_SIMPLE[s], BASS_SYNC[s], cx) + self.rules.gaps * gap
                        - if s % 2 == 1 { thin } else { 0.0 }
                }
            };
            if kick && mode == KickMode::Double {
                importance = 1.0;
            }
            if importance > 0.0
                && reveals(importance, step_density(note_param(ctx, P_DENSITY, s as u8)), complexity * BASS_CHAOS_MAX, rng.unit(s as u8, S_RHYTHM))
            {
                onsets |= 1 << s;
            }
        }

        // Half speed: some beats keep only their first note, held.
        for beat in 0..STEPS_PER_BAR / 4 {
            if groove.is_some() {
                break;
            }
            let mask = 0xFu16 << (beat * 4);
            let first = (onsets & mask).trailing_zeros();
            if (onsets & mask).count_ones() > 1 && rng.chance((beat * 4) as u8, S_HALF, persona.half) {
                onsets = (onsets & !mask) | 1 << first;
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
        let spread = param(ctx, P_SPREAD);
        let ceiling = ctx.harmony.resolve_in_key(0, octave) as i32 + BASS_SPAN as i32;
        // Odd block: notes `start..start + len` of the bar jump up by `size`.
        // The bar's first note stays home (the root), so a block starts later.
        let block_prob = lerp(JUMP_BLOCK_PROB.0, JUMP_BLOCK_PROB.1, spread);
        let block = (spread >= JUMP_SPREAD_MIN && rng.chance(0, S_JUMP, block_prob) && onsets.count_ones() > 1).then(|| {
            let largest = (((spread - JUMP_SPREAD_MIN) / (1.0 - JUMP_SPREAD_MIN)) * JUMPS.len() as f32 - 0.001) as usize;
            let largest = largest.min(JUMPS.len() - 1);
            let size = if rng.chance(0, S_JUMP_SIZE, JUMP_LARGEST_SHARE) {
                JUMPS[largest]
            } else {
                JUMPS[rng::below(rng.at(1, S_JUMP_SIZE), largest as u32 + 1) as usize]
            };
            let start = 1 + rng::below(rng.at(0, S_JUMP_START), onsets.count_ones() - 1) as usize;
            let len = 1 + rng::below(rng.at(0, S_JUMP_LEN), JUMP_BLOCK_MAX) as usize;
            (start, len, size)
        });
        let mut note_index = 0usize;
        let gate_frac = lerp(GATE_BY_DENSITY.0, GATE_BY_DENSITY.1, density);
        let gate_max = lerp(GATE_MAX_BY_DENSITY.0, GATE_MAX_BY_DENSITY.1, density).round() as usize;
        let root_key = ctx.harmony.resolve(0, octave) as i32;
        let floor = (root_key - BELOW_ROOT_SEMITONES).max(ctx.harmony.resolve_in_key(0, octave) as i32 - BELOW_KEY_FLOOR);

        // Room for repeats (the array holds two per step on average).
        let mut events = [LiveEvent::default(); 2 * STEPS_PER_BAR];
        let mut n = 0usize;
        // Step and degree of the previous note.
        let mut prev: Option<(usize, i32)> = None;
        // The walk's degree (before odd blocks and turnarounds).
        let mut walked = 0i32;
        let mut last_move = 0i32;
        for s in 0..STEPS_PER_BAR {
            if onsets & (1 << s) == 0 {
                continue;
            }
            let su = s as u8;
            let first = prev.is_none() && self.rules.root >= 0.5;
            let note_spread = note_param(ctx, P_SPREAD, su);
            let mut degree = if first {
                0
            } else if self.rules.walk {
                walk_step(walked, last_move, rng.at(su, S_PITCH), note_spread, persona.tonal)
            } else {
                self.pick_degree(rng.at(su, S_PITCH), note_spread)
            };
            last_move = if first { 0 } else { degree - walked };
            walked = degree;
            if turnaround == Some(s) {
                // Differ from what the repeating bars play on this step.
                let repeated = if first {
                    0
                } else if self.rules.walk {
                    degree
                } else {
                    self.pick_degree(rng.base.at(su, S_PITCH), spread)
                };
                let i = rng::below(rng.bar.at(su, S_TURN), TURNAROUND_DEGREES.len() as u32) as usize;
                degree = if TURNAROUND_DEGREES[i] == repeated {
                    TURNAROUND_DEGREES[(i + 1) % TURNAROUND_DEGREES.len()]
                } else {
                    TURNAROUND_DEGREES[i]
                };
            }
            let mut key = ctx.harmony.resolve(degree, octave) as i32;
            if self.rules.walk && turnaround != Some(s) {
                // The walk steps back inside the register instead of
                // folding an octave (a fold would be a jump).
                while key < floor && degree < WALK_RANGE.1 {
                    degree += 1;
                    key = ctx.harmony.resolve(degree, octave) as i32;
                }
                while key > ceiling && degree > WALK_RANGE.0 {
                    degree -= 1;
                    key = ctx.harmony.resolve(degree, octave) as i32;
                }
                walked = degree;
            }
            while key > ceiling {
                key -= 12;
            }
            // Below the root: no further than a 6th under it.
            while key < floor {
                key += 12;
            }
            // The odd block jumps from where the line would have been.
            if let Some((start, len, size)) = block {
                if (start..start + len).contains(&note_index) {
                    key = ctx.harmony.resolve(degree + size, octave) as i32;
                    while key > ceiling + 12 * (size / SCALE_LEN + 1) {
                        key -= 12;
                    }
                    degree += size;
                }
            }
            note_index += 1;

            let next = (s + 1..STEPS_PER_BAR).find(|&t| onsets & (1 << t) != 0).unwrap_or(STEPS_PER_BAR);
            // A seeded mix of short stabs and held notes: density sets the
            // balance, `stabs` moves it (0.5 = as the density says).
            let stab_share = lerp(SHORT_SHARE.0, SHORT_SHARE.1, note_param(ctx, P_DENSITY, su))
                + STABS_SHIFT * (note_param(ctx, P_STABS, su) - 0.5);
            let short = rng.chance(su, S_LENGTH, stab_share.clamp(STABS_RANGE.0, STABS_RANGE.1));
            let gate_ticks = if short {
                let span = ((next - s).min(SHORT_LENGTH.2 as usize) as u16 * TICKS_PER_STEP) as f32;
                (span * lerp(SHORT_LENGTH.0, SHORT_LENGTH.1, density)).max(GATE_MIN_TICKS).round() as u16
            } else {
                let span = ((next - s).min(gate_max) as u16 * TICKS_PER_STEP) as f32;
                (span * gate_frac.max(0.85)).max(GATE_MIN_TICKS).round() as u16
            };

            let accented = rng.chance(su, S_ACCENT, note_param(ctx, P_ACCENT, su) * ACCENT_PROB_MAX);
            let tier = if accented { Tier::Accent } else { Tier::Normal };
            let vel = velocity(tier, rng.unit(su, S_VEL), self.rules.tiers);
            let mut ev = note(step_tick(s), key.clamp(0, 127) as u8, vel, gate_ticks, s as u16 + 1);
            ev.attack = density_attack_in(density, BASS_ATTACK_BY_DENSITY);
            ev.start = stream_region(ctx, su);
            ev.cutoff = self.cutoff(ctx, su, key - root_key, accented);
            if accented {
                ev.flags |= flags::ACCENT;
            }
            if short {
                ev.flags |= flags::STAB;
            }
            if let Some((prev_step, prev_degree)) = prev {
                if s - prev_step <= SLIDE_MAX_GAP_STEPS
                    && degree != prev_degree
                    && key != events[n - 1].key as i32
                    && rng.chance(su, S_SLIDE, note_param(ctx, P_SLIDE, su) * SLIDE_PROB_MAX * persona.slide)
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
        // Ratchets belong to the stabby side: none at the sustained end. With
        // the groove scheme they are fills: the phrase's last beat and fill
        // bars only.
        let ratchet = persona.ratchet * (2.0 * param(ctx, P_STABS)).min(1.0);
        let fill_bar = ctx.fill || ctx.drum_fill;
        let ratchet_at = |step: u8| {
            !self.rules.scheme || fill_bar || (ctx.bar_in_phrase == VARIATION_BAR && step >= RATCHET_FROM_STEP)
        };
        let figure_weights: [f32; 4] = std::array::from_fn(|k| {
            FIGURE_WEIGHTS[k] * lerp(FIGURE_SEED_SCALE.0, FIGURE_SEED_SCALE.1, rng::unit_f32(rng::hash(ctx.seed, ctx.lane, 0, k as u8, S_FIGURE)))
        });
        n = ratchets(
            &mut events,
            n,
            |step, key| ratchet_at(step) && rng.chance(step, S_RATCHET, RATCHET_FILL_BOOST * ratchet * repeat_height(key as i32 - root_key)),
            |step| rng::weighted_pick(rng.at(step, S_FIGURE), &figure_weights).unwrap_or(0),
        );
        shape_notes(&mut events[..n], param(ctx, P_LENGTH), param(ctx, P_EXPRESSION), |step| rng.unit(step, S_LEGATO));
        for ev in &events[..n] {
            out.push(*ev);
        }
    }

    fn param_names(&self) -> &'static [&'static str] {
        &PARAM_NAMES
    }

    fn motif_phrase(&self, seed: u32, lane: u8) -> Option<[u16; 4]> {
        if !self.uses_motifs() {
            return None;
        }
        let m = &self.motifs[motif::pick(&self.motifs, seed, lane, &[])];
        Some(std::array::from_fn(|b| m.onsets(b % m.bars as usize)))
    }

    fn param_defaults(&self) -> [f32; MAX_GEN_PARAMS] {
        self.defaults
    }
}

// MUSICAL: pitch walk (user, 2026-10-17: at the lowest spread the line moves
// at most a scale step; the top allows big jumps, most still small).
/// Largest step in scale degrees at spread 0 and 1.
const WALK_MAX: (f32, f32) = (1.0, 9.0);
/// Step size falloff (degrees per 1/e) at spread 0 and 1.
const WALK_TAU: (f32, f32) = (0.6, 2.4);
/// Weight of staying on the same note against a one-step move.
const WALK_STAY: f32 = 0.8;
/// The walk stays within these chord-relative degrees (a 6th under the
/// root.. the octave; the register floor may push it up further), turning
/// back at the edges.
const WALK_RANGE: (i32, i32) = (-2, 7);
/// Pull towards the root per degree above it, at spread 0 and 1 (user,
/// 2026-10-17: too many high notes).
const WALK_GRAVITY: (f32, f32) = (0.45, 0.22);

/// Tonal stability of the chord-relative degrees 0..6: the Krumhansl-Kessler
/// minor key profile at the natural-minor degrees, over the tonic's rating
/// (root, 3rd and 5th most stable).
const STABILITY: [f32; 7] = [1.0, 0.556, 0.85, 0.558, 0.75, 0.629, 0.528];
/// Preference of a move by its size in degrees: steps, then the consonant
/// leaps (4th, 5th, octave) well above 6ths, 7ths and 9ths.
const LEAP_PREF: [f32; 10] = [1.0, 1.0, 0.9, 1.0, 1.1, 0.35, 0.2, 0.9, 0.2, 0.3];
/// A leap is a move of at least this many degrees; after one, the next move
/// turns back by step (gap fill): moves on in the same direction and further
/// leaps are this much less likely.
const LEAP_MIN: i32 = 3;
const GAP_FILL_SAME_WAY: f32 = 0.25;
const GAP_FILL_LEAP: f32 = 0.3;

/// The next degree of the walk from `from` (`last` the previous move): up to
/// `max` degrees, small moves far more likely, leaps to consonant intervals
/// and stable tones (`tonal`: how much stability counts, per seed), a leap
/// followed by a step back, within the range.
fn walk_step(from: i32, last: i32, h: u64, spread: f32, tonal: f32) -> i32 {
    let spread = spread.clamp(0.0, 1.0);
    let max = lerp(WALK_MAX.0, WALK_MAX.1, spread * spread).round() as i32;
    let tau = lerp(WALK_TAU.0, WALK_TAU.1, spread);
    let after_leap = last.abs() >= LEAP_MIN;
    let gravity = lerp(WALK_GRAVITY.0, WALK_GRAVITY.1, spread);
    let mut weights = [0.0f32; 19];
    for (k, w) in weights.iter_mut().enumerate() {
        let d = k as i32 - 9;
        let to = from + d;
        if d.abs() > max || to < WALK_RANGE.0 || to > WALK_RANGE.1 {
            continue;
        }
        let size = d.unsigned_abs() as usize;
        let mut x = (-(size as f32) / tau).exp() * LEAP_PREF[size.min(9)] * if d == 0 { WALK_STAY } else { 1.0 };
        x *= STABILITY[to.rem_euclid(SCALE_LEN) as usize].powf(tonal);
        // Pull back towards the root: lines circle home, high notes are rare.
        x *= (-(to.max(0) as f32) * gravity).exp();
        if after_leap && d != 0 {
            if d.signum() == last.signum() {
                x *= GAP_FILL_SAME_WAY;
            }
            if d.abs() >= LEAP_MIN {
                x *= GAP_FILL_LEAP;
            }
        }
        *w = x;
    }
    match rng::weighted_pick(h, &weights) {
        Some(k) => from + k as i32 - 9,
        None => from,
    }
}

/// A seed's rhythmic character (per letter seed, the same in every bar).
struct Persona {
    /// Added to `complexity`: more or less syncopation.
    sync: f32,
    /// Slide probability multiplier.
    slide: f32,
    /// Chance per beat of half speed, per note of a ratchet.
    half: f32,
    ratchet: f32,
    /// How much tonal stability steers the walk (exponent on [`STABILITY`]).
    tonal: f32,
}

impl Persona {
    /// No character: the rhythm and slides as the parameters say.
    const PLAIN: Self = Self { sync: 0.0, slide: 1.0, half: 0.0, ratchet: 0.0, tonal: 1.0 };

    fn new(ctx: &BarCtx) -> Self {
        let u = |k: u8| rng::unit_f32(rng::hash(ctx.seed, ctx.lane, 0, k, S_PERSONA));
        // Squared: most seeds stay plain, a few go far.
        Self {
            sync: lerp(PERSONA_SYNC.0, PERSONA_SYNC.1, u(0)),
            slide: lerp(PERSONA_SLIDE.0, PERSONA_SLIDE.1, u(1)),
            half: PERSONA_HALF_MAX * u(2) * u(2),
            ratchet: PERSONA_RATCHET_MAX * u(3) * u(3),
            tonal: lerp(PERSONA_TONAL.0, PERSONA_TONAL.1, u(4)),
        }
    }
}

// MUSICAL (user, 2026-10-17): repeats are mostly an 8th later, some a
// 16th, some syncopated (a dotted 8th), some a triplet; each seed weighs
// them its own way. High notes repeat much less.
/// Repeat offsets in ticks after the note, per figure: 8th, 16th, dotted
/// 8th, 8th triplet (two repeats).
const FIGURES: [&[u16]; 4] = [&[2 * TICKS_PER_STEP], &[TICKS_PER_STEP], &[3 * TICKS_PER_STEP], &[32, 64]];
/// Base weights of the figures, each scaled per seed by 0.4 .. 1.8.
const FIGURE_WEIGHTS: [f32; 4] = [0.5, 0.2, 0.15, 0.15];
const FIGURE_SEED_SCALE: (f32, f32) = (0.4, 1.8);
/// Repeats keep their chance up to this many semitones over the root, then
/// fade (halving about every this many more).
const REPEAT_HIGH_FROM: i32 = 5;
const REPEAT_HIGH_HALVING: f32 = 3.5;

// MUSICAL (user, 2026-10-17: rhythm and fast notes must follow a musical
// scheme, not chance). Bass figures on an 8th grid with a few 16th
// anticipations; levels: 255 the core (with the kick), ~200 the answer,
// ~150 the second answer, ~100 anticipations and pickups. `sync`: how
// syncopated the figure is (complexity picks near it).
/// One bass figure.
#[derive(Clone, Copy)]
struct Groove {
    levels: [u8; 16],
    sync: f32,
}

const fn groove(hits: &[(usize, u8)], sync: f32) -> Groove {
    let mut levels = [0u8; 16];
    let mut i = 0;
    while i < hits.len() {
        levels[hits[i].0] = hits[i].1;
        i += 1;
    }
    Groove { levels, sync }
}

const GROOVES: [Groove; 10] = [
    // Long root, answer on the "and" of 3.
    groove(&[(0, 255), (10, 200), (14, 110)], 0.1),
    // Half bar: 1 and 3, a pickup into the next bar.
    groove(&[(0, 255), (8, 210), (14, 120), (12, 90)], 0.0),
    // Dotted quarters.
    groove(&[(0, 255), (6, 215), (12, 190), (3, 90)], 0.4),
    // Two-step answer: the "a" of 2, then 3's "and".
    groove(&[(0, 255), (7, 200), (10, 170), (13, 100)], 0.6),
    // Rolling 8ths.
    groove(&[(0, 255), (6, 200), (10, 220), (14, 160), (4, 130), (12, 110)], 0.35),
    // Offbeat 8ths.
    groove(&[(2, 255), (6, 190), (10, 230), (14, 170), (0, 120)], 0.7),
    // Syncopated stab.
    groove(&[(0, 255), (3, 210), (6, 150), (11, 200), (14, 105)], 0.8),
    // Push into the backbeat.
    groove(&[(0, 255), (11, 210), (3, 140), (14, 120)], 0.55),
    // Call and answer across the bar.
    groove(&[(0, 255), (5, 180), (8, 210), (13, 140), (15, 90)], 0.5),
    // Off the one: anticipated downbeat.
    groove(&[(0, 230), (6, 255), (10, 190), (15, 140), (3, 100)], 0.9),
];
/// Level a step needs at density 0 and 1 (0: the core only; 0.5: the core
/// and its answer; 1: everything).
const GROOVE_REVEAL: (f32, f32) = (230.0, 70.0);
/// Level of the figure's empty 8ths, and how far a fully stabby line lowers
/// the reveal at full density (stabs leave gaps, so dense stabs fill them).
const GROOVE_FILL_LEVEL: u8 = 75;
/// How far the density lowers the reveal at full density on its own, and on
/// top for a fully stabby line (user, 2026-10-17: fuller).
const GROOVE_FILL_REACH: f32 = 30.0;
const GROOVE_STAB_REACH: f32 = 50.0;
/// Level of the figure's own hits repeated a beat or half a bar later (the
/// first fill a dense, stabby line takes).
const GROOVE_ECHO: f32 = 0.7;
/// Spread of the figure choice around the complexity.
const GROOVE_PICK_WIDTH: f32 = 0.25;
/// Fills: repeats from this step of the phrase's last bar, and their chance
/// boost there.
const RATCHET_FROM_STEP: u8 = 12;
const RATCHET_FILL_BOOST: f32 = 2.5;

/// A seeded figure, mostly ones whose syncopation is near `sync`.
fn pick_groove(h: u64, sync: f32) -> Groove {
    let mut weights = [0.0f32; GROOVES.len()];
    for (w, g) in weights.iter_mut().zip(&GROOVES) {
        *w = (-((g.sync - sync) / GROOVE_PICK_WIDTH).powi(2)).exp() + 0.02;
    }
    GROOVES[rng::weighted_pick(h, &weights).unwrap_or(0)]
}

// MUSICAL (user, 2026-10-17): on a mudpie every note of a letter seed
// starts at the seed's one position, anywhere in the sample; the Morph knob
// moves it within a region around that (the sampler). Per-note positions
// sounded random.
/// Start position of a note on a stream: the letter seed's position.
fn stream_region(ctx: &BarCtx, _step: u8) -> u16 {
    (rng::unit_f32(rng::hash(ctx.seed, ctx.lane, 0, 0, S_REGION)) * u16::MAX as f32) as u16
}

/// Chance factor of repeating a note `above` semitones over the root.
fn repeat_height(above: i32) -> f32 {
    if above <= REPEAT_HIGH_FROM { 1.0 } else { 0.5f32.powf((above - REPEAT_HIGH_FROM) as f32 / REPEAT_HIGH_HALVING) }
}

/// Repeat some notes (ratchets): where `pick(step, key)` says so, a note
/// plays again by the figure `figure(step)` ([`FIGURES`]) when the next note
/// is later than the figure's last repeat. Notes in a slide stay whole.
/// Returns the new count.
fn ratchets(
    events: &mut [LiveEvent; 2 * STEPS_PER_BAR],
    n: usize,
    pick: impl Fn(u8, u8) -> bool,
    figure: impl Fn(u8) -> usize,
) -> usize {
    let mut out = [LiveEvent::default(); 2 * STEPS_PER_BAR];
    let mut m = 0;
    for i in 0..n {
        let e = events[i];
        let tied = e.flags & flags::TIE != 0 || events[..n].get(i + 1).is_some_and(|x| x.flags & flags::TIE != 0);
        let step = (e.tick / TICKS_PER_STEP) as u8;
        let next = events[..n].get(i + 1).map_or(TICKS_PER_BAR, |x| x.tick);
        let offsets = FIGURES[figure(step).min(FIGURES.len() - 1)];
        let last = *offsets.last().unwrap_or(&0);
        let gap = offsets[0];
        let room = next > e.tick + last + gap / 2 && e.tick + last < TICKS_PER_BAR;
        if tied || !room || !pick(step, e.key) || m + 1 + offsets.len() > out.len() {
            if m < out.len() {
                out[m] = e;
                m += 1;
            }
            continue;
        }
        let mut a = e;
        a.gate = a.gate.min(gap.saturating_sub(2).max(1));
        out[m] = a;
        m += 1;
        for (k, &off) in offsets.iter().enumerate() {
            let mut b = e;
            b.tick = e.tick + off;
            b.gate = e.gate.min(gap);
            b.note_id = e.note_id + (STEPS_PER_BAR * (k + 1)) as u16;
            b.flags &= !flags::ACCENT;
            out[m] = b;
            m += 1;
        }
    }
    *events = out;
    m
}

/// `length` and `expression` (0.5 = unchanged). Length below 0.5 shortens
/// notes towards stabs, above 0.5 holds them towards the next onset, from
/// [`LEGATO_FROM`] a rising share of held notes ties into it and glides. Expression scales each note's
/// filter offset (0 = none, 1 = double).
fn shape_notes(events: &mut [LiveEvent], length: f32, expression: f32, roll: impl Fn(u8) -> f32) {
    if expression != 0.5 {
        for ev in events.iter_mut() {
            ev.cutoff = (ev.cutoff as f32 * expression * 2.0).round().clamp(-127.0, 127.0) as i8;
        }
    }
    if length == 0.5 {
        return;
    }
    for i in 0..events.len() {
        let next = events.get(i + 1).map_or(TICKS_PER_BAR, |e| e.tick);
        // A slid-from note already holds into the next one.
        if events.get(i + 1).is_some_and(|e| e.flags & flags::TIE != 0) {
            continue;
        }
        let has_next = i + 1 < events.len();
        let ev = &mut events[i];
        let gap = next.saturating_sub(ev.tick);
        if length < 0.5 {
            let stab = ev.gate.min(STAB_TICKS);
            ev.gate = stab + ((ev.gate - stab) as f32 * length * 2.0) as u16;
        } else if has_next && ev.flags & flags::STAB == 0 && roll((ev.tick / TICKS_PER_STEP) as u8) < legato_share(length) {
            ev.gate = gap + TIE_OVERLAP_TICKS;
            let next = &mut events[i + 1];
            next.flags |= flags::TIE;
            next.glide = SLIDE_GLIDE;
        } else if gap > ev.gate {
            ev.gate += ((gap - ev.gate) as f32 * (length - 0.5) * 2.0) as u16;
        }
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

    /// The plain generator (no seed character, pool pitches), for the rule tests.
    fn generator() -> BassGenerator {
        BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, walk: false, scheme: false, ..BassRules::default() }, Vec::new())
    }

    fn params(edit: impl FnOnce(&mut [f32; MAX_GEN_PARAMS])) -> [f32; MAX_GEN_PARAMS] {
        let mut p = PARAM_DEFAULTS;
        edit(&mut p);
        p
    }

    fn bar(g: &BassGenerator, seed: u32, bar: u8, p: &[f32; MAX_GEN_PARAMS], mask: &[u8; 16]) -> Vec<LiveEvent> {
        let mut c = ctx(seed, bar, p, mask);
        c.lane = 1;
        // Stream positions vary on every note by design; the musical
        // content is everything else.
        render(g, &c).as_slice().iter().map(|e| LiveEvent { start: 0, ..*e }).collect()
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
                        assert!((24..=28 + 11 + BASS_SPAN as u8 + 19).contains(&e.key), "key {} out of register", e.key);
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
    fn spread_sets_the_odd_block_jump_size() {
        let g = generator();
        // Largest distance above the bar's lowest note, over many seeds.
        let widest = |spread: f32| {
            (0..200)
                .map(|seed| {
                    let p = params(|p| {
                        p[P_SPREAD] = spread;
                        p[P_DENSITY] = 0.8;
                    });
                    let keys: Vec<i32> = bar(&g, seed, 0, &p, &TWO_STEP).iter().map(|e| e.key as i32).collect();
                    keys.iter().max().unwrap_or(&0) - keys.iter().min().unwrap_or(&0)
                })
                .max()
                .unwrap()
        };
        assert!(widest(0.0) <= 22, "no jumps: around the root, {}", widest(0.0));
        assert!(widest(1.0) >= 19, "full spread reaches +19: {}", widest(1.0));
        // Spread moves notes off the root more often (above and below).
        let off_root = |spread: f32| -> usize {
            let p = params(|p| p[P_SPREAD] = spread);
            (0..100).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).filter(|e| e.key % 12 != 9).count()
        };
        assert!(off_root(0.0) * 2 < off_root(1.0), "{} vs {}", off_root(0.0), off_root(1.0));
        let below = (0..300)
            .flat_map(|seed| bar(&g, seed, 0, &params(|p| p[P_SPREAD] = 1.0), &TWO_STEP))
            .filter(|e| (e.key as i32) < 33)
            .count();
        assert!(below > 20, "notes under the root (A1 = 33): {below}");
        // Jumps are blocks: most notes still sit within an octave of the root.
        let p = params(|p| p[P_SPREAD] = 1.0);
        let ev: Vec<LiveEvent> = (0..200).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).collect();
        let near = ev.iter().filter(|e| (e.key as i32 - 33).abs() <= 12).count();
        assert!(near * 10 > ev.len() * 7, "{near} of {}", ev.len());
    }

    #[test]
    fn root_rule_off_spreads_pitches() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { root: 0.0, walk: false, scheme: false, ..BassRules::default() }, Vec::new());
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
        let ignore = BassGenerator::new(PARAM_DEFAULTS, BassRules { gaps: 0.0, scheme: false, ..BassRules::default() }, Vec::new());
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
    fn spread_sizes_the_walk() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, ..BassRules::default() }, Vec::new());
        let intervals = |spread: f32| -> Vec<i32> {
            let p = params(|p| {
                p[P_SPREAD] = spread;
                p[P_DENSITY] = 0.7;
            });
            // Bar 0: not the variation bar, so no turnaround.
            (0..200)
                .flat_map(|seed| {
                    let ev = bar(&g, seed, 0, &p, &TWO_STEP);
                    ev.windows(2).map(|w| (w[1].key as i32 - w[0].key as i32).abs()).collect::<Vec<_>>()
                })
                .collect()
        };
        let low = intervals(0.0);
        assert!(low.iter().all(|&i| i <= 2), "at most a scale step: {:?}", low.iter().max());
        let high = intervals(1.0);
        assert!(high.iter().any(|&i| i >= 7), "big jumps exist");
        let small = high.iter().filter(|&&i| i <= 3).count() as f32 / high.len() as f32;
        assert!(small > 0.5, "most moves stay small: {small}");
    }

    #[test]
    fn leaps_favour_consonant_intervals_and_turn_back() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, ..BassRules::default() }, Vec::new());
        let p = params(|p| {
            p[P_SPREAD] = 1.0;
            p[P_DENSITY] = 0.8;
        });
        let (mut consonant, mut dissonant, mut turned, mut leaps) = (0, 0, 0, 0);
        for seed in 0..400 {
            let keys: Vec<i32> = bar(&g, seed, 0, &p, &TWO_STEP).iter().map(|e| e.key as i32).collect();
            for w in keys.windows(2) {
                match (w[1] - w[0]).abs() {
                    5 | 7 | 12 => consonant += 1,
                    6 | 8..=11 | 13..=16 => dissonant += 1,
                    _ => {}
                }
            }
            for w in keys.windows(3) {
                let (a, b) = (w[1] - w[0], w[2] - w[1]);
                if a.abs() >= 5 && b != 0 {
                    leaps += 1;
                    turned += (a.signum() != b.signum()) as usize;
                }
            }
        }
        assert!(consonant > dissonant, "{consonant} vs {dissonant}");
        assert!(turned * 10 > leaps * 6, "gap fill: {turned} of {leaps}");
    }

    #[test]
    fn high_notes_repeat_less() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { scheme: false, ..BassRules::default() }, Vec::new());
        let p = params(|p| {
            p[P_SPREAD] = 1.0;
            p[P_DENSITY] = 0.6;
        });
        // Repeat rate of notes low (within a 4th of the root A1 = 33) and high (an octave up).
        let (mut low, mut low_rep, mut high, mut high_rep) = (0, 0, 0, 0);
        for seed in 0..600 {
            for e in bar(&g, seed, 0, &p, &TWO_STEP) {
                let repeat = e.note_id > STEPS_PER_BAR as u16;
                match e.key as i32 - 33 {
                    0..=5 => {
                        low += 1;
                        low_rep += repeat as usize;
                    }
                    11..=20 => {
                        high += 1;
                        high_rep += repeat as usize;
                    }
                    _ => {}
                }
            }
        }
        let (lr, hr) = (low_rep as f32 / low as f32, high_rep as f32 / high.max(1) as f32);
        assert!(hr < lr * 0.5, "low {lr} high {hr}");
    }

    #[test]
    fn grooves_follow_the_vocabulary_and_fill_only_at_phrase_ends() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, ..BassRules::default() }, Vec::new());
        let count = |density: f32| -> f32 {
            let p = params(|p| p[P_DENSITY] = density);
            (0..200).map(|seed| bar(&g, seed, 0, &p, &EMPTY).len()).sum::<usize>() as f32 / 200.0
        };
        let (low, mid, high) = (count(0.02), count(0.5), count(1.0));
        assert!(low >= 0.9 && low <= 1.6 && (3.0..5.5).contains(&mid) && high > mid && high <= 10.0, "{low} {mid} {high}");
        // Dense and stabby: the gaps fill sooner (at full density everything
        // plays either way).
        let stabby = |stabs: f32| -> f32 {
            let p = params(|p| {
                p[P_DENSITY] = 0.75;
                p[P_STABS] = stabs;
            });
            (0..200).map(|seed| bar(&g, seed, 0, &p, &EMPTY).len()).sum::<usize>() as f32 / 200.0
        };
        assert!(stabby(1.0) > stabby(0.5) + 1.0, "{} vs {}", stabby(1.0), stabby(0.5));
        // Every onset is a step of the figure vocabulary; most on the 8th grid.
        let p = params(|p| p[P_DENSITY] = 0.7);
        let ev: Vec<LiveEvent> = (0..200).flat_map(|seed| bar(&g, seed, 0, &p, &EMPTY)).collect();
        let on_8ths = ev.iter().filter(|e| (e.tick / TICKS_PER_STEP) % 2 == 0).count();
        assert!(on_8ths * 10 > ev.len() * 7, "{on_8ths} of {}", ev.len());
        // Repeats only as fills: none in bar 0, some at the phrase's end.
        let stab = params(|p| {
            p[P_STABS] = 1.0;
            p[P_DENSITY] = 0.8;
        });
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules::default(), Vec::new());
        let reps = |bar_in: u8| (0..300).flat_map(|seed| bar(&g, seed, bar_in, &stab, &EMPTY)).filter(|e| e.note_id > STEPS_PER_BAR as u16).count();
        assert_eq!(reps(0), 0);
        assert!(reps(VARIATION_BAR) > 0);
    }

    #[test]
    fn stabs_set_the_share_of_stab_notes() {
        let g = generator();
        let share = |stabs: f32| {
            let p = params(|p| p[P_STABS] = stabs);
            let ev: Vec<LiveEvent> = (0..100).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).collect();
            ev.iter().filter(|e| e.flags & flags::STAB != 0).count() as f32 / ev.len() as f32
        };
        let (lo, mid, hi) = (share(0.0), share(0.5), share(1.0));
        assert!(lo < 0.15 && hi > 0.85 && lo < mid && mid < hi, "{lo} {mid} {hi}");
    }

    #[test]
    fn seeds_carry_their_own_rhythmic_character() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { scheme: false, ..BassRules::default() }, Vec::new());
        let p = params(|p| p[P_DENSITY] = 0.7);
        let (mut ratchets, mut half_beats, mut slides) = (0, 0, Vec::new());
        let mut figures_seen = false;
        for seed in 0..300 {
            let ev = bar(&g, seed, 0, &p, &EMPTY);
            ratchets += ev.iter().filter(|e| e.note_id > STEPS_PER_BAR as u16).count();
            figures_seen |= ev.iter().filter(|e| e.tick % TICKS_PER_STEP != 0).count() > 0;
            for beat in 0..4u16 {
                let n = ev.iter().filter(|e| e.tick / (4 * TICKS_PER_STEP) == beat).count();
                half_beats += (n == 1) as usize;
            }
            slides.push(ev.iter().filter(|e| e.flags & flags::TIE != 0).count());
            // Ratchets keep the bar well formed.
            for w in ev.windows(2) {
                assert!(w[0].tick < w[1].tick && w[0].tick + w[0].gate <= w[1].tick + 2 || w[1].flags & flags::TIE != 0);
            }
        }
        assert!(ratchets > 20, "{ratchets}");
        assert!(figures_seen, "triplet repeats on some seeds");
        assert!(half_beats > 50, "{half_beats}");
        // Some seeds slide a lot, many barely.
        assert!(slides.iter().max().unwrap() >= &3 && slides.iter().filter(|&&s| s == 0).count() > 50);
    }

    #[test]
    fn length_and_expression_shape_the_notes() {
        let g = generator();
        let mean_gate = |length: f32| -> f32 {
            let p = params(|p| p[P_LENGTH] = length);
            let ev: Vec<LiveEvent> = (0..50).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).collect();
            ev.iter().map(|e| e.gate as f32).sum::<f32>() / ev.len() as f32
        };
        assert!(mean_gate(0.0) < mean_gate(0.5) && mean_gate(0.5) < mean_gate(0.8), "{} {} {}", mean_gate(0.0), mean_gate(0.5), mean_gate(0.8));
        // Legato: every held note holds into the next one, which glides
        // (stabs stay stabs).
        let legato = params(|p| p[P_LENGTH] = 1.0);
        for seed in 0..50 {
            let ev = bar(&g, seed, 0, &legato, &TWO_STEP);
            for w in ev.windows(2).filter(|w| w[0].flags & flags::STAB == 0) {
                assert!(w[0].tick + w[0].gate > w[1].tick && w[1].flags & flags::TIE != 0 && w[1].glide > 0);
            }
        }
        // Part way up, part of the held notes tie.
        let some = params(|p| p[P_LENGTH] = 0.875);
        let (mut tied, mut held) = (0, 0);
        for seed in 0..100 {
            let ev = bar(&g, seed, 0, &some, &TWO_STEP);
            for w in ev.windows(2).filter(|w| w[0].flags & flags::STAB == 0) {
                held += 1;
                tied += (w[1].flags & flags::TIE != 0) as usize;
            }
        }
        let share = tied as f32 / held as f32;
        assert!((0.35..0.75).contains(&share), "{share}");
        // Expression scales the filter offsets; 0.5 changes nothing.
        let flat = params(|p| p[P_EXPRESSION] = 0.0);
        let double = params(|p| p[P_EXPRESSION] = 1.0);
        for seed in 0..20 {
            let base = bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP);
            assert!(bar(&g, seed, 0, &flat, &TWO_STEP).iter().all(|e| e.cutoff == 0));
            for (a, b) in base.iter().zip(bar(&g, seed, 0, &double, &TWO_STEP)) {
                assert_eq!((a.cutoff as i32 * 2).clamp(-127, 127), b.cutoff as i32);
            }
        }
    }

    #[test]
    fn slides_tie_into_the_next_note() {
        let g = generator();
        let p = params(|p| {
            p[P_SLIDE] = 1.0;
            p[P_DENSITY] = 0.8;
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
    fn accents_and_note_length_from_density() {
        let g = generator();
        let (mut accents, mut total) = (0, 0);
        for seed in 0..200 {
            for e in bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP) {
                total += 1;
                if e.flags & flags::ACCENT != 0 {
                    accents += 1;
                    assert!(e.vel > 105);
                }
            }
        }
        let share = accents as f32 / total as f32;
        assert!((0.2..0.4).contains(&share), "accent share {share}");
        // Sparse lines hold their notes longer than dense ones (per note).
        let mean_gate = |d: f32| -> f32 {
            let p = params(|p| {
                p[P_DENSITY] = d;
                p[P_SLIDE] = 0.0;
            });
            let ev: Vec<LiveEvent> = (0..40).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).collect();
            ev.iter().map(|e| e.gate as f32).sum::<f32>() / ev.len() as f32
        };
        assert!(mean_gate(0.2) > mean_gate(0.5) && mean_gate(0.5) > mean_gate(0.9), "{} {} {}", mean_gate(0.2), mean_gate(0.5), mean_gate(0.9));
    }

    #[test]
    fn seeded_filter_moves_per_note() {
        let g = generator();
        let span = (FILTER_HALF_OCTAVES / CUTOFF_OFFSET_OCTAVES * 127.0).round() as i8 + 1;
        let mut distinct = std::collections::BTreeSet::new();
        for seed in 0..40 {
            let ev = bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP);
            assert!(ev.iter().all(|e| e.cutoff.abs() <= span));
            distinct.extend(ev.iter().map(|e| e.cutoff));
            // The motif repeats bar to bar.
            let again: Vec<i8> = bar(&g, seed, 1, &PARAM_DEFAULTS, &TWO_STEP).iter().map(|e| e.cutoff).collect();
            assert_eq!(again, ev.iter().map(|e| e.cutoff).collect::<Vec<_>>());
        }
        assert!(distinct.len() > 20, "cutoffs vary by seed and note: {distinct:?}");
        // Denser lines sit brighter on average.
        let mean = |d: f32| {
            let ev: Vec<LiveEvent> = (0..40).flat_map(|s| bar(&g, s, 0, &params(|p| p[P_DENSITY] = d), &TWO_STEP)).collect();
            ev.iter().map(|e| e.cutoff as f32).sum::<f32>() / ev.len() as f32
        };
        assert!(mean(0.9) > mean(0.1) + 10.0);
    }

    #[test]
    fn idiom_cell_sets_the_rhythm() {
        let mut levels = [0u8; 16];
        for (s, l) in [(0, 255), (3, 200), (6, 150), (11, 90)] {
            levels[s] = l;
        }
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, walk: false, scheme: false, ..BassRules::default() }, vec![LevelMap::bar(levels)]);
        let free = |d: f32| {
            params(|p| {
                p[P_DENSITY] = d;
                p[P_KICK_MODE] = 0.5;
                p[P_COMPLEXITY] = 0.0;
                p[P_VARIATION] = 0.0;
            })
        };
        // Density reveals the cell's steps by level; the floor
        // (RHYTHM_DENSITY) keeps a sparse bass grooving.
        assert_eq!(steps(&bar(&g, 1, 0, &free(0.9), &EMPTY)), [0, 3, 6, 11]);
        let mid = steps(&bar(&g, 1, 0, &free(0.5), &EMPTY));
        assert!(mid.len() >= 2 && mid[..2] == [0, 3], "{mid:?}");
        let low = steps(&bar(&g, 1, 0, &free(0.05), &EMPTY));
        assert!(!low.is_empty() && low.len() <= mid.len() && low[0] == 0, "{low:?}");
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

    // ── Motif mode ──

    fn motif_generator(names: &[&str]) -> BassGenerator {
        let motifs = crate::live::generator::test_util::starter_motifs().into_iter().filter(|m| names.contains(&m.name.as_str())).collect();
        BassGenerator::new(PARAM_DEFAULTS, BassRules::default(), Vec::new()).with_motifs(motifs)
    }

    #[test]
    fn motif_mode_plays_the_clip_unchanged_bar_to_bar() {
        let g = motif_generator(&["bass-332-bounce"]);
        let still = params(|p| p[P_VARIATION] = 0.0);
        for seed in 0..20 {
            let mut c = ctx(seed, 0, &still, &TWO_STEP);
            c.lane = 1;
            // Per-note jitter may move accent, slide and filter, never pitch or rhythm.
            c.jitter = [0.4; MAX_GEN_PARAMS];
            let first: Vec<(u16, u8)> = render(&g, &c).as_slice().iter().map(|e| (e.tick, e.key)).collect();
            assert_eq!(steps_of(&first), [0, 3, 6, 8, 11, 14]);
            assert_eq!(first[0].1 % 12, 9, "root first");
            for b in 1..4 {
                c.bar_in_phrase = b;
                let now: Vec<(u16, u8)> = render(&g, &c).as_slice().iter().map(|e| (e.tick, e.key)).collect();
                assert_eq!(now, first, "seed {seed} bar {b}");
            }
        }
    }

    fn steps_of(ev: &[(u16, u8)]) -> Vec<usize> {
        ev.iter().map(|e| (e.0 / TICKS_PER_STEP) as usize).collect()
    }

    #[test]
    fn motif_mode_sets_cutoff_and_stream_start_and_follows_the_chord() {
        let g = motif_generator(&["bass-roller-walk", "bass-stab-hold", "bass-syncopated"]);
        let mut cutoffs = std::collections::BTreeSet::new();
        for seed in 0..30 {
            let mut c = ctx(seed, 0, &PARAM_DEFAULTS, &TWO_STEP);
            c.lane = 1;
            let on_root = render(&g, &c);
            assert_well_formed(&on_root);
            assert!(!on_root.is_empty());
            cutoffs.extend(on_root.as_slice().iter().map(|e| e.cutoff));
            assert!(on_root.as_slice().iter().all(|e| e.kind == EventKind::NoteOn && e.attack > 0));
            c.harmony.chord_degree = 5;
            let on_six = render(&g, &c);
            assert_eq!(on_root.len(), on_six.len());
            assert_eq!(on_six.as_slice()[0].key % 12, 5, "F under chord VI");
        }
        assert!(cutoffs.len() > 5, "the filter still moves per note: {cutoffs:?}");
        let p = g.motif_phrase(3, 1).unwrap();
        assert!(p.iter().all(|&m| m != 0));
        assert!(BassGenerator::new(PARAM_DEFAULTS, BassRules::default(), Vec::new()).motif_phrase(3, 1).is_none());
    }

    #[test]
    fn motifs_can_be_switched_off_by_the_rules() {
        let motifs = crate::live::generator::test_util::starter_motifs();
        let off = BassGenerator::new(PARAM_DEFAULTS, BassRules { motifs: false, persona: false, walk: false, scheme: false, ..BassRules::default() }, Vec::new()).with_motifs(motifs);
        assert!(off.motif_phrase(1, 1).is_none());
        assert_eq!(bar(&off, 1, 0, &PARAM_DEFAULTS, &TWO_STEP), bar(&generator(), 1, 0, &PARAM_DEFAULTS, &TWO_STEP));
    }

    #[test]
    fn golden_bars() {
        // Pinned output: a change here changes how saved seeds sound.
        let g = generator();
        let got: Vec<String> = [1u32, 2, 3].iter().map(|&s| notes(&bar(&g, s, 0, &PARAM_DEFAULTS, &TWO_STEP))).collect();
        assert_eq!(got, GOLDEN, "\n{got:#?}");
    }

    const GOLDEN: [&str; 3] = [
        // 2026-10-17: the lower density floor (about 4 notes a bar in the middle).
        "6:33 8:33 14:33",
        "6:33 8:43 14:40",
        "6:33 8:33 11:33 14:33",
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
