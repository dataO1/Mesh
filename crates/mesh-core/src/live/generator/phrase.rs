//! The `phrase` generator: plays a sound pack's MIDI phrases as written and
//! remixes them with the lane's knobs (docs/plans/2026-10-16-live-sound-packs.md).
//!
//! The letter seed picks a phrase; the phrase plays its bar at the global bar
//! (bar = global bar mod phrase length), moved to the lane's key by the
//! shortest transposition. Its pitch bends and controller curves go out as
//! `Bend` and `Cc` events (each bar starts with the values in force, so a
//! phrase entered mid-way sounds right).
//!
//! Parameters, all 0..1 with 0.5 = as written:
//! - `density`: below 0.5 thins the least important notes first (long
//!   notes, pickups and phrase starts stay, [`crate::live::phrase`]); at 0
//!   about a quarter remain, at least one per bar that has notes. Above 0.5
//!   it repeats notes into gaps of a quarter note or more, seeded.
//! - `length`: below 0.5 shortens notes towards stabs; above 0.5 holds them
//!   towards the next note; from [`LEGATO_FROM`] overlapping notes tie and
//!   glide.
//! - `expression`: scales the controller curves around their first value
//!   (0 = flat, 1 = double). Bends, vibrato and glide time are scaled by the
//!   sampler's Expression parameter (a lane control moves both).
//! - `tone`: kept for the control layout; the sampler's Tone parameter
//!   writes the pack's tone CC.
//! - `spread` (default 0): the bass generator's spread system on the written
//!   notes. Odd blocks: a seeded run of 1-3 notes per bar (never the bar's
//!   first) jumps up a 5th, an octave, an octave and a 3rd or an octave and
//!   a 5th (in scale steps; spread sets how often and how far). Weak notes
//!   lift an octave, more often in a fill bar's last beat.
//! - `variation` (default 0.3): chance that the phrase's variation bar (and
//!   fill bars) turns its last note to the chord's 5th or 7th.
//!
//! The arrangement moves `density`, `spread` and `variation` per letter and
//! pushes them up in filler segments ([`crate::live::form::LETTER_OFFSETS`],
//! [`crate::live::form::FILLER_BOOSTS`]), so the line varies by section.
//!
//! Glide: CC65 (portamento on) in the phrase ties the notes it covers, CC5
//! sets the glide time; without CC5 the pack's default glide applies.

use super::blocks::{note_param, VARIATION_BAR};
use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::live::event::{flags, BarEvents, EventKind, LiveEvent};
use crate::live::phrase::Phrase;
use crate::live::rng;
use crate::live::sampler::attack_byte;
use crate::live::{TICKS_PER_BAR, TICKS_PER_STEP};

pub const PARAM_NAMES: [&str; 6] = ["density", "length", "expression", "tone", "spread", "variation"];
/// Defaults: as written, no lifts; some phrase-end turnarounds.
pub const PARAM_DEFAULTS: [f32; 6] = [0.5, 0.5, 0.5, 0.5, 0.0, 0.3];
const P_DENSITY: usize = 0;
const P_LENGTH: usize = 1;
const P_EXPRESSION: usize = 2;
const P_SPREAD: usize = 4;
const P_VARIATION: usize = 5;

const S_PICK: u32 = 0x700;
const S_REPEAT: u32 = 0x701;
const S_SPREAD: u32 = 0x702;
const S_JUMP: u32 = 0x703;
const S_JUMP_SIZE: u32 = 0x704;
const S_JUMP_START: u32 = 0x705;
const S_JUMP_LEN: u32 = 0x706;
const S_TURN: u32 = 0x707;

// MUSICAL: the bass generator's spread system (generator/bass.rs), on the
// written notes. Odd blocks: a seeded run of 1-3 notes per bar (never the
// bar's first) jumps up by scale steps; spread sets how often and how far.
/// Jump sizes in scale steps: 5th, octave, octave + 3rd, octave + 5th.
const JUMPS: [i32; 4] = [4, 7, 9, 11];
/// Spread below this: no odd blocks.
const JUMP_SPREAD_MIN: f32 = 0.15;
/// Chance that a bar has an odd block, at spread `JUMP_SPREAD_MIN` and 1.
const JUMP_BLOCK_PROB: (f32, f32) = (0.15, 0.7);
/// Longest odd block in notes.
const JUMP_BLOCK_MAX: u32 = 3;
/// Share of blocks that take the largest jump the spread allows.
const JUMP_LARGEST_SHARE: f32 = 0.6;
/// Turnaround: in the phrase's variation bar (and fill bars) the bar's last
/// note moves to the chord's 5th or 7th (chord-relative scale steps), with
/// a chance of `variation`.
const TURNAROUND_DEGREES: [i32; 2] = [4, 6];

// MUSICAL: first draft, tune by ear.
/// Chance of an octave lift at spread 1, for a weak note and a strong one
/// (the phrase's anchors mostly stay put).
const SPREAD_LIFT: (f32, f32) = (0.6, 0.15);
/// Weight below which a note counts as weak ([`crate::live::phrase`]).
const WEAK_WEIGHT: f32 = 1.0;
/// Extra lift chance for notes in the last beat of a fill bar (the
/// arrangement's turnarounds), times the spread.
const FILL_LIFT: f32 = 0.5;

// MUSICAL: first draft, tune by ear.
/// Share of notes kept at density 0.
const DENSITY_FLOOR: f32 = 0.25;
/// Shortest stab at length 0, in ticks (a 16th and a half).
const STAB_TICKS: u32 = 36;
/// From this length, overlapping notes tie and glide.
pub const LEGATO_FROM: f32 = 0.85;
/// Default glide when a tie has no CC5 in the phrase.
const DEFAULT_GLIDE_SECS: f32 = 0.06;
/// Most notes played per bar (more are dropped).
const MAX_BAR_NOTES: usize = 64;
/// Gaps (in steps) a repeat may fill at density 1.
const REPEAT_GAP_STEPS: u32 = 4;

pub struct PhraseGenerator {
    phrases: Vec<Phrase>,
    defaults: [f32; MAX_GEN_PARAMS],
    glide_secs: f32,
}

impl PhraseGenerator {
    pub fn new(phrases: Vec<Phrase>) -> Self {
        let mut defaults = [0.5; MAX_GEN_PARAMS];
        defaults[..PARAM_DEFAULTS.len()].copy_from_slice(&PARAM_DEFAULTS);
        Self { phrases, defaults, glide_secs: DEFAULT_GLIDE_SECS }
    }

    /// Parameter defaults (the style's `generator.params`).
    pub fn with_defaults(mut self, defaults: [f32; MAX_GEN_PARAMS]) -> Self {
        self.defaults = defaults;
        self
    }

    /// Default glide time of the pack (ties without CC5).
    pub fn with_glide(mut self, secs: f32) -> Self {
        self.glide_secs = secs.max(0.0);
        self
    }

    pub fn phrases(&self) -> &[Phrase] {
        &self.phrases
    }
}

/// `key` moved `steps` scale steps up in the scale of `root` (a note outside
/// the scale keeps its offset from the scale note below it).
fn diatonic_shift(key: i32, steps: i32, root: u8, scale: &[u8; 7]) -> i32 {
    let rel = key - root as i32;
    let (octave, pc) = (rel.div_euclid(12), rel.rem_euclid(12));
    let idx = scale.iter().rposition(|&s| s as i32 <= pc).unwrap_or(0);
    let off = pc - scale[idx] as i32;
    let j = idx as i32 + steps;
    root as i32 + 12 * (octave + j.div_euclid(7)) + scale[j.rem_euclid(7) as usize] as i32 + off
}

fn param(ctx: &BarCtx, i: usize) -> f32 {
    ctx.params[i].clamp(0.0, 1.0)
}

/// A NoteOn whose gate may run past the bar line (the sampler counts it down).
fn note_on(tick: u32, key: i32, vel: u8, gate: u32, id: u16) -> LiveEvent {
    LiveEvent {
        tick: tick.min(TICKS_PER_BAR as u32 - 1) as u16,
        kind: EventKind::NoteOn,
        key: key.clamp(0, 127) as u8,
        vel: vel.max(1),
        gate: gate.clamp(1, u16::MAX as u32) as u16,
        note_id: id,
        ..LiveEvent::default()
    }
}

impl LaneGenerator for PhraseGenerator {
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        if self.phrases.is_empty() {
            return;
        }
        let h = rng::hash(ctx.seed, ctx.lane, 0, 0, S_PICK);
        let p = &self.phrases[(h % self.phrases.len() as u64) as usize];
        let global = ctx.phrase as u64 * 4 + ctx.bar_in_phrase as u64;
        let pb = (global % p.bars as u64) as u32;
        let bar0 = pb * TICKS_PER_BAR as u32;
        let shift = p.shift_to(ctx.harmony.key.root);
        let (density, length, expr) = (param(ctx, P_DENSITY), param(ctx, P_LENGTH), param(ctx, P_EXPRESSION));

        // Controller and bend state at the bar start, then their changes.
        let scale = |cc: u8, v: u8| -> u8 {
            let base = p.ccs.iter().find(|c| c.1 == cc).map_or(v, |c| c.2) as f32;
            (base + (v as f32 - base) * expr * 2.0).round().clamp(0.0, 127.0) as u8
        };
        let mut seen: [bool; 128] = [false; 128];
        for &(_, cc, _) in &p.ccs {
            if (cc as usize) < 128 && !seen[cc as usize] {
                seen[cc as usize] = true;
                if let Some(v) = p.cc_at(cc, bar0) {
                    out.push(LiveEvent { tick: 0, kind: EventKind::Cc, target: cc, vel: scale(cc, v), ..LiveEvent::default() });
                }
            }
        }
        if !p.bends.is_empty() {
            out.push(LiveEvent { tick: 0, kind: EventKind::Bend, cents: p.bend_at(bar0), ..LiveEvent::default() });
        }
        let in_bar = |t: u32| t > bar0 && t < bar0 + TICKS_PER_BAR as u32;
        for &(t, v) in p.bends.iter().filter(|b| in_bar(b.0)) {
            out.push(LiveEvent { tick: (t - bar0) as u16, kind: EventKind::Bend, cents: v, ..LiveEvent::default() });
        }
        for &(t, cc, v) in p.ccs.iter().filter(|c| in_bar(c.0)) {
            out.push(LiveEvent { tick: (t - bar0) as u16, kind: EventKind::Cc, target: cc, vel: scale(cc, v), ..LiveEvent::default() });
        }

        // Notes: density thinning by importance.
        let range = p.bar_notes(pb);
        let notes = &p.notes[range.clone()];
        // Allocation-free (render runs on the audio thread): rank in a stack array.
        let notes = &notes[..notes.len().min(MAX_BAR_NOTES)];
        let mut buf = [0u16; MAX_BAR_NOTES];
        for (i, b) in buf.iter_mut().enumerate().take(notes.len()) {
            *b = i as u16;
        }
        let mut kept = notes.len();
        if density < 0.5 && !notes.is_empty() {
            let share = DENSITY_FLOOR + (1.0 - DENSITY_FLOOR) * density * 2.0;
            kept = ((notes.len() as f32 * share).ceil() as usize).clamp(1, notes.len());
            let rank = &mut buf[..notes.len()];
            rank.sort_unstable_by(|&a, &b| notes[b as usize].weight.total_cmp(&notes[a as usize].weight).then(a.cmp(&b)));
            rank[..kept].sort_unstable();
        }
        let keep = &buf[..kept];
        // The next onset after a note (the whole phrase, so the last note of
        // a bar holds towards the next bar's first note).
        let next_onset = |i: usize| -> u32 {
            let abs = range.start + i;
            p.notes[abs + 1..].iter().map(|n| n.tick).find(|&t| t > p.notes[abs].tick).unwrap_or(p.bars * TICKS_PER_BAR as u32)
        };
        // Odd block over the kept notes: (first index, length, scale steps).
        let spread = param(ctx, P_SPREAD);
        let h = |stream: u32, k: u8| rng::hash(ctx.seed, ctx.lane, pb as u8, k, stream);
        let block = (spread >= JUMP_SPREAD_MIN
            && keep.len() > 1
            && rng::chance(h(S_JUMP, 0), JUMP_BLOCK_PROB.0 + (JUMP_BLOCK_PROB.1 - JUMP_BLOCK_PROB.0) * spread))
            .then(|| {
                let largest = (((spread - JUMP_SPREAD_MIN) / (1.0 - JUMP_SPREAD_MIN)) * JUMPS.len() as f32 - 0.001) as usize;
                let largest = largest.min(JUMPS.len() - 1);
                let size = if rng::chance(h(S_JUMP_SIZE, 0), JUMP_LARGEST_SHARE) {
                    JUMPS[largest]
                } else {
                    JUMPS[rng::below(h(S_JUMP_SIZE, 1), largest as u32 + 1) as usize]
                };
                let start = 1 + rng::below(h(S_JUMP_START, 0), keep.len() as u32 - 1) as usize;
                let len = 1 + rng::below(h(S_JUMP_LEN, 0), JUMP_BLOCK_MAX) as usize;
                (start, len, size)
            });
        // Turnaround on the bar's last note (variation bar or a fill).
        let turnaround_bar = ctx.bar_in_phrase == VARIATION_BAR || ctx.fill || ctx.drum_fill;
        let turnaround = turnaround_bar
            && !keep.is_empty()
            && rng::chance(h(S_TURN, 0), param(ctx, P_VARIATION));
        let scale = ctx.harmony.scale();
        let root = ctx.harmony.key.root;

        let mut id = 1u16;
        let mut prev_end_tick: Option<u32> = None;
        for (k, &i) in keep.iter().enumerate() {
            let i = i as usize;
            let n = notes[i];
            let gap = next_onset(i).saturating_sub(n.tick);
            let gate = if length < 0.5 {
                let stab = n.len.min(STAB_TICKS);
                stab + ((n.len - stab) as f32 * length * 2.0) as u32
            } else {
                n.len + (gap.saturating_sub(n.len) as f32 * (length - 0.5) * 2.0) as u32
            };
            let legato_knob = length >= LEGATO_FROM;
            let porta = p.cc_at(65, n.tick).is_some_and(|v| v >= 64);
            // Spread: seeded octave lifts, mostly on weak notes and on the last
            // beat of fill bars. The arrangement moves spread per letter and
            // boosts it in filler segments, so the bass varies by section.
            let sp = note_param(ctx, P_SPREAD, i as u8);
            let in_last_beat = n.tick - bar0 >= 3 * TICKS_PER_BAR as u32 / 4;
            let mut lift_p = sp * if n.weight < WEAK_WEIGHT { SPREAD_LIFT.0 } else { SPREAD_LIFT.1 };
            if (ctx.fill || ctx.drum_fill) && in_last_beat {
                lift_p += sp * FILL_LIFT;
            }
            let lift = if sp > 0.0 && rng::chance(rng::hash(ctx.seed, ctx.lane, pb as u8, i as u8, S_SPREAD), lift_p) { 12 } else { 0 };
            let mut key = n.key as i32 + shift;
            if turnaround && k == keep.len() - 1 {
                // The chord's 5th or 7th, in the octave nearest the written note.
                let d = TURNAROUND_DEGREES[rng::below(h(S_TURN, 1), TURNAROUND_DEGREES.len() as u32) as usize];
                let t = ctx.harmony.resolve(d, key / 12 - 1) as i32;
                key = [t - 12, t, t + 12].into_iter().min_by_key(|c| (c - key).abs()).unwrap_or(key);
            } else if let Some((_, _, size)) = block.filter(|b| (b.0..b.0 + b.1).contains(&k)) {
                key = diatonic_shift(key, size, root, scale);
            } else if k > 0 {
                // The bar's first note stays home.
                key += lift;
            }
            let mut ev = note_on(n.tick - bar0, key, n.vel, gate + if legato_knob { 12 } else { 0 }, id);
            if (legato_knob || porta) && prev_end_tick.is_some_and(|e| e >= n.tick) {
                ev.flags |= flags::TIE;
                let secs = p.cc_at(5, n.tick).map_or(self.glide_secs, |v| 0.005 + v as f32 / 127.0 * 0.5);
                ev.glide = attack_byte(secs);
            }
            out.push(ev);
            id = id.wrapping_add(1).max(1);
            prev_end_tick = Some(n.tick + gate + if legato_knob { 12 } else { 0 });

            // Repeats into the gap after a note (density above 0.5).
            if density > 0.5 {
                let step = TICKS_PER_STEP as u32;
                let free = gap.saturating_sub(n.len.max(2 * step));
                if free >= REPEAT_GAP_STEPS * step {
                    let r = rng::hash(ctx.seed, ctx.lane, pb as u8, i as u8, S_REPEAT);
                    if rng::chance(r, (density - 0.5) * 2.0) {
                        // On the 8th off-beat inside the gap.
                        let start = n.tick + n.len.max(2 * step);
                        let t = (start.div_ceil(2 * step) * 2 + 1) * step;
                        if t < n.tick + gap && t < bar0 + TICKS_PER_BAR as u32 {
                            let vel = (n.vel as f32 * 0.8) as u8;
                            out.push(note_on(t - bar0, n.key as i32 + shift, vel, (2 * step).min(gate), id));
                            id = id.wrapping_add(1).max(1);
                        }
                    }
                }
            }
        }
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
    use crate::live::generator::test_util::ctx;
    use crate::live::motif::midi::Expression;
    use crate::live::motif::RawNote;

    fn phrase() -> Phrase {
        let s = TICKS_PER_STEP as u32;
        let n = |step: u32, len: u32, key: u8| RawNote { start: step * s, len: len * s, pitch: key, vel: 100 };
        // Bar 0: long G (anchor), short on-beat G, pickup Bb on 15. Bar 1: F held.
        let raw = vec![n(1, 8, 43), n(12, 1, 43), n(15, 1, 46), n(16, 12, 41)];
        let mut x = Expression::default();
        x.ccs.push((0, 74, 40));
        x.ccs.push((200, 74, 100));
        x.bends.push((100, 200));
        Phrase::from_parts("p", raw, x, 2 * TICKS_PER_BAR as u32).unwrap()
    }

    fn render(params: [f32; 4], bar: u8) -> Vec<LiveEvent> {
        let g = PhraseGenerator::new(vec![phrase()]);
        let mut p = [0.5; MAX_GEN_PARAMS];
        p[..4].copy_from_slice(&params);
        let mask = [0u8; crate::live::STEPS_PER_BAR];
        let c = ctx(1, bar, &p, &mask);
        let mut out = BarEvents::default();
        g.render_bar(&c, &mut out);
        out.as_slice().to_vec()
    }

    fn notes(ev: &[LiveEvent]) -> Vec<LiveEvent> {
        ev.iter().filter(|e| e.kind == EventKind::NoteOn).copied().collect()
    }

    #[test]
    fn plays_as_written_at_the_middle() {
        let ev = render([0.5; 4], 0);
        let n = notes(&ev);
        assert_eq!(n.len(), 3);
        assert_eq!(n.iter().map(|e| e.tick).collect::<Vec<_>>(), vec![24, 288, 360]);
        assert_eq!(n[0].gate, 8 * TICKS_PER_STEP);
        assert!(ev.iter().any(|e| e.kind == EventKind::Cc && e.target == 74 && e.vel == 40 && e.tick == 0));
        assert!(ev.iter().any(|e| e.kind == EventKind::Bend && e.cents == 200 && e.tick == 100));
        // Bar 1 is the phrase's second bar: the F held over the bar.
        let n1 = notes(&render([0.5; 4], 1));
        assert_eq!(n1.len(), 1);
        assert_eq!(n1[0].gate, 12 * TICKS_PER_STEP);
    }

    #[test]
    fn density_keeps_anchors_and_length_shapes_gates() {
        let thin = notes(&render([0.0, 0.5, 0.5, 0.5], 0));
        assert_eq!(thin.len(), 1, "a quarter of three notes, rounded up");
        assert_eq!(thin[0].tick, 24, "the long first note survives");
        let stabs = notes(&render([0.5, 0.0, 0.5, 0.5], 0));
        assert!(stabs.iter().all(|e| e.gate as u32 <= STAB_TICKS));
        let held = notes(&render([0.5, 1.0, 0.5, 0.5], 0));
        assert!(held[0].gate as u32 >= 11 * TICKS_PER_STEP as u32, "held towards the next note");
        assert!(held[1].has(flags::TIE), "legato ties overlapping notes");
    }

    #[test]
    fn expression_scales_cc_curves() {
        let flat = render([0.5, 0.5, 0.0, 0.5], 0);
        assert!(flat.iter().filter(|e| e.kind == EventKind::Cc).all(|e| e.vel == 40), "no movement at 0");
        let wide = render([0.5, 0.5, 1.0, 0.5], 0);
        assert!(wide.iter().any(|e| e.kind == EventKind::Cc && e.vel == 127), "doubled movement, clamped");
    }

    #[test]
    fn spread_lifts_weak_notes_and_fill_turnarounds() {
        let p = phrase();
        let g = PhraseGenerator::new(vec![p]);
        let keys = |spread: f32, fill: bool| -> Vec<u8> {
            let mut params = [0.5; MAX_GEN_PARAMS];
            params[P_SPREAD] = spread;
            params[P_VARIATION] = 0.0;
            let mask = [0u8; crate::live::STEPS_PER_BAR];
            let mut all = Vec::new();
            for seed in 0..40u32 {
                let mut c = ctx(seed, 0, &params, &mask);
                c.fill = fill;
                let mut out = BarEvents::default();
                g.render_bar(&c, &mut out);
                all.extend(out.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn).map(|e| e.key));
            }
            all
        };
        let base = keys(0.0, false);
        let lowest = *base.iter().min().unwrap();
        assert!(base.iter().all(|&k| k < lowest + 12), "no lifts at spread 0");
        let lifted = |ks: &[u8]| ks.iter().filter(|&&k| k >= lowest + 12).count();
        let (some, fills) = (lifted(&keys(1.0, false)), lifted(&keys(1.0, true)));
        assert!(some > 0, "spread 1 lifts some notes");
        assert!(fills > some, "fill bars lift more: {some} -> {fills}");
    }

    #[test]
    fn odd_blocks_jump_by_scale_steps_and_turnarounds_vary_the_last_note() {
        // A minor: A3 up a 5th (4 steps) is E4, an octave + 5th (11) is E5.
        let am = [0u8, 2, 3, 5, 7, 8, 10];
        assert_eq!(diatonic_shift(57, 4, 9, &am), 64);
        assert_eq!(diatonic_shift(57, 11, 9, &am), 76);
        assert_eq!(diatonic_shift(60, 7, 9, &am), 72);

        let g = PhraseGenerator::new(vec![phrase()]);
        let mask = [0u8; crate::live::STEPS_PER_BAR];
        let run = |spread: f32, variation: f32, bar: u8| -> Vec<Vec<u8>> {
            let mut params = [0.5; MAX_GEN_PARAMS];
            params[P_SPREAD] = spread;
            params[P_VARIATION] = variation;
            (0..60u32)
                .map(|seed| {
                    let c = ctx(seed, bar, &params, &mask);
                    let mut out = BarEvents::default();
                    g.render_bar(&c, &mut out);
                    out.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn).map(|e| e.key).collect()
                })
                .collect()
        };
        let plain = run(0.0, 0.0, 0);
        assert!(plain.windows(2).all(|w| w[0] == w[1]), "spread 0, no variation: as written every time");
        let wide = run(1.0, 0.0, 0);
        let jumped = wide.iter().filter(|bar| bar != &&plain[0]).count();
        assert!(jumped > 20, "spread 1 jumps in most bars: {jumped}/60");
        assert!(wide.iter().all(|bar| bar[0] == plain[0][0]), "the bar's first note stays home");
        let turned = run(0.0, 1.0, VARIATION_BAR);
        let plain3 = run(0.0, 0.0, VARIATION_BAR);
        let moved = turned.iter().zip(&plain3).filter(|(t, p)| t.last() != p.last()).count();
        assert!(moved > 50, "variation 1 turns the variation bar's last note around: {moved}/60");
        assert!(turned.iter().zip(&plain3).all(|(t, p)| t[..t.len() - 1] == p[..p.len() - 1]), "only the last note moves");
    }
}
