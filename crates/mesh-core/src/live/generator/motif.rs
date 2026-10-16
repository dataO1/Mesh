//! Motif playback for the bass and lead generators.
//!
//! A [`Motif`] is a curated clip (see [`crate::live::motif`]). The letter seed
//! picks a clip per lane and the notes and rhythm play as written. The
//! knobs only reshape it, and never per bar at random:
//!
//! - `density` thins the weak-beat notes when low (the strong beats always
//!   stay), adds repeats of the written notes in the gaps when high (capped,
//!   never every 16th), and sets the note length (sparse = held, dense =
//!   short, as written at the middle).
//! - `spread` lifts some weak notes an octave (bass) or doubles notes an
//!   octave up (lead). The choice is seeded per note, so a clip repeats.
//! - `variation` gives the last bar of the 4-bar phrase one seeded change:
//!   an octave lift of the last notes, an ending on the 5th or (lead) a
//!   16th run into the next phrase.
//! - the harmony re-pitches the degrees against key, mode and chord, and the
//!   whole bar moves an octave so the chord root stays nearest the key's.
//!
//! Everything works on fixed arrays: rendering allocates nothing.

use super::blocks::{lerp, metric_level, metric_weight};
use super::BarCtx;
use crate::live::motif::{Motif, MAX_CLIPS};
use crate::live::rng;
use crate::live::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// Most notes of one planned bar.
pub const MAX_PLAN: usize = 40;

const S_PICK: u32 = 0x400;
const S_SPREAD: u32 = 0x401;
const S_VARIANT: u32 = 0x402;
const S_VARIANT_KIND: u32 = 0x403;
const S_YIELD: u32 = 0x404;
const S_RUN_DIR: u32 = 0x405;

// MUSICAL: first draft, tune by ear. Density.
/// Density from which every written note plays; below it weak notes drop.
const FULL_FROM: f32 = 0.5;
/// Metric weight a note needs at density 0 (beats and half bars stay).
const THIN_MAX: f32 = 0.65;
/// Written notes this long (ticks) count as structural and are kept longer.
const LONG_NOTE: u16 = 4 * TICKS_PER_STEP;
const LONG_NOTE_WEIGHT: f32 = 0.8;
/// A repeat fills a gap at most this far (ticks) after the note it repeats.
const REPEAT_REACH: u16 = 4 * TICKS_PER_STEP;
/// Repeats play at this share of the note's velocity.
const REPEAT_VEL: f32 = 0.8;

// MUSICAL: first draft, tune by ear. Note length against the written one at
// density 0 (sparse, held) and 1 (dense, short); 1.0 at the middle.
const LENGTH_SCALE: (f32, f32) = (2.0, 0.55);
const GATE_MIN_TICKS: f32 = 6.0;

// MUSICAL: first draft, tune by ear. Spread.
/// Chance per weak bass note of an octave lift at spread 1.
const LIFT_MAX: f32 = 0.55;
/// Notes with a metric weight under this may lift.
const LIFT_BELOW_WEIGHT: f32 = 0.7;
/// Spread from which lead notes double an octave up, and the chance at spread 1.
const DOUBLE_FROM: f32 = 0.3;
const DOUBLE_MAX: f32 = 0.75;
/// Velocity of a doubling note against its note.
pub const DOUBLE_VEL: f32 = 0.6;

// MUSICAL: first draft, tune by ear. Variation bar.
/// Chance per seed that the last bar varies, at variation 0+ and 1.
const VARIANT_PROB: (f32, f32) = (0.5, 1.0);
/// Notes the octave lift moves.
const LIFT_LAST: usize = 2;
/// The 16th run over the last beat: first step, notes and ticks.
const RUN_FIRST_STEP: u16 = 12;
const RUN_NOTES: usize = 4;
const RUN_VEL: f32 = 0.9;

/// The chord root counts as nearest the key's root up to this many semitones above.
const FOLD_ABOVE: i32 = 6;

/// Which part of a clip bar plays (call and response).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Play {
    Whole,
    /// The call: the first half of the bar.
    First,
    /// The answer: the second half.
    Second,
}

/// What the lane wants from the clip.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    pub density: f32,
    pub spread: f32,
    pub variation: f32,
    /// Most notes after repeats (written notes always play).
    pub max_notes: usize,
    /// Lead (octave doubling, 16th run) rather than bass (octave lift).
    pub lead: bool,
    pub play: Play,
    /// Notes landing on these steps (the bass's onsets) drop with
    /// `yield_prob`, seeded per note.
    pub yield_to: u16,
    pub yield_prob: f32,
}

/// One planned note of the bar.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlanNote {
    pub tick: u16,
    pub gate: u16,
    pub vel: u8,
    pub degree: i32,
    pub chroma: i8,
    /// Octaves added after resolving.
    pub lift: i8,
    /// Also play this note an octave up (lead).
    pub double: bool,
    /// Index in the clip, for seeding.
    pub id: u8,
}

impl PlanNote {
    pub fn step(&self) -> usize {
        ((self.tick / TICKS_PER_STEP) as usize).min(STEPS_PER_BAR - 1)
    }
}

/// The planned bar, sorted by tick.
pub struct Plan {
    notes: [PlanNote; MAX_PLAN],
    len: usize,
}

impl Plan {
    pub fn new() -> Self {
        Self { notes: [PlanNote::default(); MAX_PLAN], len: 0 }
    }

    pub fn as_slice(&self) -> &[PlanNote] {
        &self.notes[..self.len]
    }

    fn push(&mut self, n: PlanNote) -> bool {
        if self.len == MAX_PLAN {
            return false;
        }
        self.notes[self.len] = n;
        self.len += 1;
        true
    }

    fn sort(&mut self) {
        let s = &mut self.notes[..self.len];
        for i in 1..s.len() {
            let mut j = i;
            while j > 0 && s[j - 1].tick > s[j].tick {
                s.swap(j - 1, j);
                j -= 1;
            }
        }
    }
}

impl Default for Plan {
    fn default() -> Self {
        Self::new()
    }
}

/// The clip a letter seed plays: `weights` (one per clip) or uniform.
pub fn pick(motifs: &[Motif], seed: u32, lane: u8, weights: &[f32]) -> usize {
    let h = rng::hash(seed, lane, 0, 0, S_PICK);
    if weights.len() == motifs.len() {
        if let Some(i) = rng::weighted_pick(h, weights) {
            return i;
        }
    }
    rng::below(h, motifs.len().max(1) as u32) as usize
}

// MUSICAL: first draft, tune by ear. Lead clip choice against the bass clip:
// every clip keeps at least this weight.
const INTERLOCK_FLOOR: f32 = 0.15;

/// Weights for picking a lead clip against the bass's onsets over the phrase
/// (`bass[bar]` step masks, all 0 = no bass clip). High complexity favours
/// clips whose onsets miss the bass's (interlock), low complexity clips that
/// play with it (unison). Returns the number of weights written.
pub fn interlock_weights(motifs: &[Motif], bass: &[u16; 4], high_cx: bool, out: &mut [f32; MAX_CLIPS]) -> usize {
    let n = motifs.len().min(MAX_CLIPS);
    for (w, m) in out.iter_mut().zip(&motifs[..n]) {
        let (mut sum, mut bars) = (0.0f32, 0u32);
        for (b, &bass_on) in bass.iter().enumerate() {
            let lead_on = m.onsets(b % m.bars as usize);
            if lead_on == 0 {
                continue;
            }
            let together = (lead_on & bass_on).count_ones() as f32 / lead_on.count_ones() as f32;
            sum += if high_cx { 1.0 - together } else { together };
            bars += 1;
        }
        let score = if bars == 0 { 0.0 } else { sum / bars as f32 };
        *w = INTERLOCK_FLOOR + (1.0 - INTERLOCK_FLOOR) * score;
    }
    n
}

/// Which part of the clip bar plays. A 1-bar clip at low complexity is split
/// over the phrase: the call (first half) in bars 1 and 3, the answer in bars
/// 2 and 4. At high complexity, and for longer clips (written as call and
/// answer already), the bar plays whole.
pub fn call_response(m: &Motif, bar_in_phrase: u8, complexity: f32, split_below: f32) -> Play {
    if m.bars == 1 && complexity < split_below {
        let on = m.onsets(0);
        if on & 0x00FF != 0 && on & 0xFF00 != 0 {
            return if bar_in_phrase % 2 == 0 { Play::First } else { Play::Second };
        }
    }
    Play::Whole
}

/// Note length scale for a density.
pub fn length_scale(density: f32) -> f32 {
    let d = density.clamp(0.0, 1.0);
    if d <= FULL_FROM {
        lerp(LENGTH_SCALE.0, 1.0, d / FULL_FROM)
    } else {
        lerp(1.0, LENGTH_SCALE.1, (d - FULL_FROM) / (1.0 - FULL_FROM))
    }
}

/// Plan this bar of motif `m`. See the module docs.
pub fn plan_bar(m: &Motif, ctx: &BarCtx, s: &Shape, plan: &mut Plan) {
    plan.len = 0;
    let clip_bar = ctx.bar_in_phrase as usize % m.bars as usize;
    let range = m.bar_range(clip_bar);
    let bar_start = clip_bar as u16 * TICKS_PER_BAR;
    let half = TICKS_PER_BAR / 2;
    let (lo, hi) = match s.play {
        Play::Whole => (0, TICKS_PER_BAR),
        Play::First => (0, half),
        Play::Second => (half, TICKS_PER_BAR),
    };
    let density = s.density.clamp(0.0, 1.0);
    let thin = THIN_MAX * (1.0 - density / FULL_FROM).max(0.0);

    // Written notes: thin the weak ones, let the bass's notes through or not.
    let mut first = true;
    let mut fallback: Option<PlanNote> = None;
    for i in range {
        let n = m.notes[i];
        let tick = n.tick - bar_start;
        if tick < lo || tick >= hi {
            continue;
        }
        let step = (tick / TICKS_PER_STEP) as usize;
        let mut weight = metric_weight(step.min(STEPS_PER_BAR - 1));
        if n.len >= LONG_NOTE {
            weight = weight.max(LONG_NOTE_WEIGHT);
        }
        if first {
            weight = 1.0;
        }
        first = false;
        if weight < thin {
            continue;
        }
        let note = PlanNote { tick, gate: n.len, vel: n.vel, degree: n.degree as i32, chroma: n.chroma, lift: 0, double: false, id: i as u8 };
        fallback.get_or_insert(note);
        // Space for the bass: its onset steps may drop this note (seeded per
        // note, so a clip repeats).
        let near = ((tick + TICKS_PER_STEP / 2) / TICKS_PER_STEP) as usize;
        if s.yield_prob > 0.0
            && near < STEPS_PER_BAR
            && s.yield_to & (1 << near) != 0
            && rng::chance(rng::hash(ctx.seed, ctx.lane, clip_bar as u8, note.id, S_YIELD), s.yield_prob)
        {
            continue;
        }
        plan.push(note);
    }
    // Never an empty bar when the clip has notes here.
    if plan.len == 0 {
        if let Some(n) = fallback {
            plan.push(n);
        }
    }
    if plan.len == 0 {
        return;
    }

    // High density: repeats of the written notes in the gaps, strongest
    // steps first, capped so the bar never fills every 16th.
    let fill = ((density - FULL_FROM) / (1.0 - FULL_FROM)).clamp(0.0, 1.0);
    if fill > 0.0 {
        let written = plan.len;
        let cap = s.max_notes.max(written);
        let mut taken = 0u16;
        for n in &plan.notes[..written] {
            taken |= 1 << n.step();
        }
        'levels: for level in 0..=4u8 {
            for step in 0..STEPS_PER_BAR {
                if metric_level(step) != level || taken & (1 << step) != 0 || !(metric_weight(step) > 1.0 - fill) {
                    continue;
                }
                if plan.len >= cap {
                    break 'levels;
                }
                let tick = step as u16 * TICKS_PER_STEP;
                let carrier = plan.notes[..written].iter().rev().find(|n| n.tick < tick);
                if let Some(&c) = carrier {
                    if tick - c.tick <= REPEAT_REACH {
                        let vel = ((c.vel as f32 * REPEAT_VEL).round() as u8).max(1);
                        plan.push(PlanNote { tick, vel, ..c });
                        taken |= 1 << step;
                    }
                }
            }
        }
        plan.sort();
    }

    // Spread: octave lifts (bass) or doublings (lead), seeded per clip note.
    if s.spread > 0.0 {
        for n in &mut plan.notes[..plan.len] {
            let h = rng::hash(ctx.seed, ctx.lane, clip_bar as u8, n.id, S_SPREAD);
            if s.lead {
                let p = ((s.spread - DOUBLE_FROM) / (1.0 - DOUBLE_FROM)).max(0.0) * DOUBLE_MAX;
                n.double = rng::chance(h, p);
            } else if metric_weight(n.step()) < LIFT_BELOW_WEIGHT && rng::chance(h, s.spread * LIFT_MAX) {
                n.lift = 1;
            }
        }
    }

    // The last bar of the phrase changes once, by seed.
    if ctx.bar_in_phrase == 3 && s.variation > 0.0 {
        let h = rng::hash(ctx.seed, ctx.lane, 3, 0, S_VARIANT);
        if rng::chance(h, lerp(VARIANT_PROB.0, VARIANT_PROB.1, s.variation)) {
            let kinds = if s.lead { 3 } else { 2 };
            vary_last_bar(plan, rng::below(rng::hash(ctx.seed, ctx.lane, 3, 0, S_VARIANT_KIND), kinds), ctx);
        }
    }

    // Lengths: the written ones scaled by the density, never into the next note.
    let scale = length_scale(density);
    for i in 0..plan.len {
        let end = if i + 1 < plan.len { plan.notes[i + 1].tick } else { TICKS_PER_BAR };
        let tick = plan.notes[i].tick;
        let gap = end - tick;
        let scaled = (plan.notes[i].gate as f32 * scale).round().max(GATE_MIN_TICKS) as u16;
        plan.notes[i].gate = scaled.min(gap).max(1);
    }
}

/// The phrase-end change: 0 lifts the last notes an octave, 1 ends on the
/// 5th, 2 (lead) plays a 16th run into the downbeat.
fn vary_last_bar(plan: &mut Plan, kind: u32, ctx: &BarCtx) {
    let len = plan.len;
    match kind {
        0 => {
            for n in &mut plan.notes[len.saturating_sub(LIFT_LAST)..len] {
                n.lift += 1;
            }
        }
        1 => {
            let last = &mut plan.notes[len - 1];
            let delta = {
                let d = (4 - last.degree.rem_euclid(7)).rem_euclid(7);
                if d > 3 { d - 7 } else { d }
            };
            if delta == 0 {
                last.lift += 1;
            } else {
                last.degree += delta;
                last.chroma = 0;
            }
        }
        _ => {
            let end = plan.notes[len - 1];
            let from = RUN_FIRST_STEP * TICKS_PER_STEP;
            let kept = plan.notes[..len].iter().take_while(|n| n.tick < from).count();
            plan.len = kept;
            // The run walks into the last note's degree, from above or below.
            let dir = if rng::chance(rng::hash(ctx.seed, ctx.lane, 3, 0, S_RUN_DIR), 0.5) { 1 } else { -1 };
            for k in 0..RUN_NOTES {
                let back = (RUN_NOTES - 1 - k) as i32;
                plan.push(PlanNote {
                    tick: from + k as u16 * TICKS_PER_STEP,
                    gate: TICKS_PER_STEP,
                    vel: ((end.vel as f32 * RUN_VEL).round() as u8).max(1),
                    degree: end.degree + dir * back,
                    chroma: 0,
                    lift: end.lift,
                    double: false,
                    id: end.id,
                });
            }
        }
    }
}

/// Octave shift (0 or -12) and floor for the bar: the chord root stays
/// nearest the key's root, and the lowest note stays above `min_key`.
pub fn fold(ctx: &BarCtx, octave: i32, plan: &Plan, min_key: i32) -> i32 {
    let chord_root = ctx.harmony.resolve(0, octave) as i32;
    let home = ctx.harmony.resolve_in_key(0, octave) as i32;
    let mut fold = if chord_root - home > FOLD_ABOVE { -12 } else { 0 };
    if let Some(lowest) = plan.as_slice().iter().map(|n| note_key(ctx, octave, 0, n)).min() {
        while lowest + fold < min_key && fold < 48 {
            fold += 12;
        }
    }
    fold
}

/// Semitones of a planned note above the clip's own tonic, exactly as
/// written (the clip's detected scale plus its chromatic offset).
pub fn clip_semitones(clip: &Motif, n: &PlanNote) -> i32 {
    use crate::live::harmony::{MAJOR_SCALE, MINOR_SCALE};
    let scale = if clip.minor { &MINOR_SCALE } else { &MAJOR_SCALE };
    scale[n.degree.rem_euclid(7) as usize] as i32 + 12 * n.degree.div_euclid(7) + n.chroma as i32
}

/// MIDI key of a planned note against this bar's key, mode and chord.
pub fn note_key(ctx: &BarCtx, octave: i32, fold: i32, n: &PlanNote) -> i32 {
    ctx.harmony.resolve(n.degree, octave) as i32 + n.chroma as i32 + 12 * n.lift as i32 + fold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::generator::test_util::{ctx, starter_motifs};
    use crate::live::generator::MAX_GEN_PARAMS;

    const NONE: [u8; 16] = [0; 16];
    const P: [f32; MAX_GEN_PARAMS] = [0.5; MAX_GEN_PARAMS];

    fn shape(density: f32) -> Shape {
        Shape { density, spread: 0.0, variation: 0.0, max_notes: 10, lead: false, play: Play::Whole, yield_to: 0, yield_prob: 0.0 }
    }

    fn motif(name: &str) -> Motif {
        starter_motifs().into_iter().find(|m| m.name == name).unwrap()
    }

    fn planned(m: &Motif, bar: u8, s: &Shape) -> Vec<PlanNote> {
        let c = ctx(7, bar, &P, &NONE);
        let mut plan = Plan::new();
        plan_bar(m, &c, s, &mut plan);
        plan.as_slice().to_vec()
    }

    #[test]
    fn written_rhythm_and_pitch_repeat_every_bar() {
        let m = motif("bass-332-bounce");
        let a = planned(&m, 0, &shape(0.5));
        assert_eq!(a.len(), 6);
        for bar in 1..3 {
            assert_eq!(planned(&m, bar, &shape(0.5)), a, "bar {bar}");
        }
        // A 2-bar clip alternates bars.
        let two = motif("bass-syncopated");
        assert_eq!(planned(&two, 0, &shape(0.5)), planned(&two, 2, &shape(0.5)));
        assert_ne!(planned(&two, 0, &shape(0.5)), planned(&two, 1, &shape(0.5)));
    }

    #[test]
    fn low_density_thins_weak_notes_but_keeps_the_strong_beats() {
        let m = motif("bass-syncopated");
        let full = planned(&m, 0, &shape(0.5));
        let thin = planned(&m, 0, &shape(0.1));
        assert!(thin.len() < full.len(), "{} vs {}", thin.len(), full.len());
        // The downbeat note stays, and every kept note is one of the written ones.
        assert_eq!(thin[0].tick, 0);
        for n in &thin {
            assert!(full.iter().any(|f| f.tick == n.tick && f.degree == n.degree));
            assert!(metric_weight(n.step()) >= 0.65 * (1.0 - 0.1 / 0.5) || n.tick == 0, "step {}", n.step());
        }
        // Thinning only removes: more density never drops a note.
        let mut prev: Vec<u16> = Vec::new();
        for d in 1..=10 {
            let now: Vec<u16> = planned(&m, 0, &shape(d as f32 / 20.0)).iter().map(|n| n.tick).collect();
            assert!(prev.iter().all(|t| now.contains(t)));
            prev = now;
        }
    }

    #[test]
    fn high_density_repeats_notes_but_never_fills_every_sixteenth() {
        let m = motif("bass-3334-pedal");
        let written = planned(&m, 0, &shape(0.5));
        let dense = planned(&m, 0, &shape(1.0));
        assert!(dense.len() > written.len());
        assert!(dense.len() <= 10, "capped: {}", dense.len());
        for w in &written {
            assert!(dense.iter().any(|d| d.tick == w.tick && d.degree == w.degree));
        }
        // Repeats copy a written pitch.
        for d in &dense {
            assert!(written.iter().any(|w| w.degree == d.degree));
        }
        // Even a clip of 16ths is never padded beyond what it holds.
        let sixteenths = motif("lead-pedal-line");
        let mut s = shape(1.0);
        s.max_notes = 8;
        assert!(planned(&sixteenths, 0, &s).len() <= 8.max(planned(&sixteenths, 0, &shape(0.5)).len()));
    }

    #[test]
    fn note_length_follows_the_density() {
        let m = motif("bass-stab-hold");
        let mean = |d: f32| {
            let p = planned(&m, 0, &shape(d));
            p.iter().map(|n| n.gate as f32).sum::<f32>() / p.len() as f32
        };
        assert!(mean(0.2) > mean(0.5), "{} {}", mean(0.2), mean(0.5));
        assert!(mean(0.5) > mean(0.8) * 0.95);
        for n in planned(&m, 0, &shape(0.3)) {
            assert!(n.gate >= 1 && n.tick + n.gate <= TICKS_PER_BAR);
        }
        assert!((length_scale(0.5) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn re_pitches_to_the_chord_key_and_mode() {
        let m = motif("bass-332-bounce");
        let p = planned(&m, 0, &shape(0.5));
        let mut c = ctx(7, 0, &P, &NONE);
        let mut plan = Plan::new();
        plan_bar(&m, &c, &shape(0.5), &mut plan);
        let keys = |c: &BarCtx| -> Vec<i32> {
            let f = fold(c, 1, &plan, 24);
            plan.as_slice().iter().map(|n| note_key(c, 1, f, n)).collect()
        };
        let on_root = keys(&c);
        // The clip's root plays the key's root (A minor), its octave 12 above.
        assert_eq!(on_root[0] % 12, 9);
        assert_eq!(on_root[2] - on_root[0], 12);
        assert_eq!(p.len(), on_root.len());
        // Chord VI (F): the same shape on F, still nearest the key's root.
        c.harmony.chord_degree = 5;
        let on_six = keys(&c);
        assert_eq!(on_six[0] % 12, 5);
        assert!((on_six[0] - on_root[0]).abs() <= 6, "{} {}", on_six[0], on_root[0]);
        for (a, b) in on_root.iter().zip(&on_six) {
            assert_eq!((a - on_root[0]) % 12, (b - on_six[0]) % 12, "same shape");
        }
        // Another key: the same degrees a fifth up.
        c.harmony.chord_degree = 0;
        c.harmony.key = crate::music::MusicalKey::new(4, true);
        let in_e = keys(&c);
        assert_eq!(in_e[0] % 12, 4);
        // A mode changes the colour tones: dorian raises the 6th, the 7th stays.
        let m2 = motif("bass-3334-pedal");
        let mut c = ctx(7, 1, &P, &NONE);
        let mut plan2 = Plan::new();
        plan_bar(&m2, &c, &shape(0.5), &mut plan2);
        let aeolian = plan2.as_slice().iter().map(|n| note_key(&c, 1, 0, n)).collect::<Vec<_>>();
        c.harmony.mode = 1;
        let dorian = plan2.as_slice().iter().map(|n| note_key(&c, 1, 0, n)).collect::<Vec<_>>();
        assert_eq!(aeolian, dorian, "no 6th in the clip: unchanged");
    }

    #[test]
    fn call_and_response_splits_a_one_bar_clip_over_the_phrase() {
        let m = motif("lead-static-hook");
        assert_eq!(m.bars, 1);
        assert_eq!(call_response(&m, 0, 0.2, 0.5), Play::First);
        assert_eq!(call_response(&m, 1, 0.2, 0.5), Play::Second);
        assert_eq!(call_response(&m, 2, 0.2, 0.5), Play::First);
        assert_eq!(call_response(&m, 1, 0.8, 0.5), Play::Whole, "within the bar at high complexity");
        // Two-bar clips already are question and answer.
        assert_eq!(call_response(&motif("lead-332-call"), 1, 0.2, 0.5), Play::Whole);
        // The call has the first half, the answer the second, and rests elsewhere.
        let mut s = shape(0.5);
        s.play = Play::First;
        let call = planned(&m, 0, &s);
        s.play = Play::Second;
        let answer = planned(&m, 1, &s);
        assert!(!call.is_empty() && !answer.is_empty());
        assert!(call.iter().all(|n| n.tick < 192) && answer.iter().all(|n| n.tick >= 192));
        // A clip with an empty half plays whole.
        assert_eq!(call_response(&motif("bass-call-answer"), 0, 0.2, 0.5), Play::Whole);
    }

    #[test]
    fn notes_on_the_bass_yield_at_high_complexity() {
        let m = motif("lead-octave-call");
        let free = planned(&m, 0, &shape(0.5));
        let mut s = shape(0.5);
        s.yield_to = 0xFFFF;
        s.yield_prob = 0.8;
        let yielded = planned(&m, 0, &s);
        assert!(!yielded.is_empty() && yielded.len() < free.len(), "{} vs {}", yielded.len(), free.len());
        s.yield_prob = 0.0;
        assert_eq!(planned(&m, 0, &s), free);
        // A seed always plays the same notes.
        s.yield_prob = 0.8;
        assert_eq!(planned(&m, 0, &s), yielded);
    }

    #[test]
    fn spread_lifts_bass_notes_and_doubles_lead_notes() {
        let m = motif("bass-eighths-triad");
        let mut s = shape(0.5);
        s.spread = 1.0;
        let wide = planned(&m, 0, &s);
        assert!(wide.iter().any(|n| n.lift == 1));
        assert_eq!(wide[0].lift, 0, "the downbeat stays home");
        assert!(planned(&m, 0, &shape(0.5)).iter().all(|n| n.lift == 0));
        let lead = motif("lead-pedal-line");
        s.lead = true;
        assert!(planned(&lead, 0, &s).iter().any(|n| n.double));
        s.spread = 0.2;
        assert!(planned(&lead, 0, &s).iter().all(|n| !n.double));
    }

    #[test]
    fn the_last_bar_varies_by_seed_and_other_bars_do_not() {
        let m = motif("bass-332-bounce");
        let mut s = shape(0.5);
        s.variation = 1.0;
        let plain = planned(&m, 0, &s);
        assert_eq!(planned(&m, 1, &s), plain);
        assert_eq!(planned(&m, 2, &s), plain);
        let mut changed = 0;
        for seed in 0..40u32 {
            let mut c = ctx(seed, 3, &P, &NONE);
            c.lane = 1;
            let mut plan = Plan::new();
            plan_bar(&m, &c, &s, &mut plan);
            changed += (plan.as_slice() != &plain[..]) as u32;
            assert!(plan.as_slice().iter().all(|n| n.tick + n.gate <= TICKS_PER_BAR));
        }
        assert!(changed >= 30, "most seeds vary the last bar: {changed}");
        // No variation: bar 4 repeats.
        s.variation = 0.0;
        assert_eq!(planned(&m, 3, &s), plain);
        // The lead's run adds 16ths over the last beat.
        let lead = motif("lead-332-call");
        s.variation = 1.0;
        s.lead = true;
        let runs = (0..60u32)
            .filter(|&seed| {
                let mut c = ctx(seed, 3, &P, &NONE);
                c.lane = 2;
                let mut plan = Plan::new();
                plan_bar(&lead, &c, &s, &mut plan);
                plan.as_slice().iter().filter(|n| n.tick >= 288).count() == RUN_NOTES
            })
            .count();
        assert!(runs > 5, "{runs}");
    }

    #[test]
    fn interlock_prefers_offbeats_at_high_cx_and_unison_at_low() {
        let motifs = starter_motifs();
        let leads: Vec<Motif> = motifs.iter().filter(|m| m.name.starts_with("lead-")).cloned().collect();
        // A bass playing the 3-3-2 figure.
        let bass = [0b0100_0100_1001_0010u16 | 1; 4];
        let mut hi = [0.0; MAX_CLIPS];
        let mut lo = [0.0; MAX_CLIPS];
        let n = interlock_weights(&leads, &bass, true, &mut hi);
        interlock_weights(&leads, &bass, false, &mut lo);
        assert_eq!(n, leads.len());
        let most = |w: &[f32]| w[..n].iter().cloned().enumerate().fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a }).0;
        let (best_hi, best_lo) = (most(&hi), most(&lo));
        let together = |m: &Motif| (0..4).map(|b| (m.onsets(b % m.bars as usize) & bass[b]).count_ones()).sum::<u32>();
        assert!(together(&leads[best_hi]) <= together(&leads[best_lo]));
        assert!(hi[..n].iter().all(|&w| w >= INTERLOCK_FLOOR));
    }
}
