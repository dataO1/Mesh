//! Lane forms: which seed plays in which bar (design §6.6).
//!
//! Every lane has one **master seed**. Four letter seeds are derived from it:
//! A and B are **main** seeds (repetitive, mostly on the grid, normal sounds),
//! C and D are **filler** seeds (triplets, heavy swing, push and pull, busier
//! patterns, odd sample picks). A **form** says which letter plays in which
//! bar, as segments of a letter and a bar count: `A15 C1` is fifteen bars of
//! A and one bar of C, 16 bars in total, repeating. A primed letter (`A'`)
//! plays the same seed with a different second half, so long forms develop
//! without changing the idea.
//!
//! Forms are aligned to the global bar (`bar % form length`), so every lane's
//! form starts on the same bars. Everything here is pure and allocation-free
//! once a [`Form`] is built.

use std::fmt;

use super::event::{BarEvents, EventKind, LiveEvent};
use super::harmony::seeded_chord;
use super::rng;
use super::{TICKS_PER_BAR, TICKS_PER_STEP};

/// Longest form in bars.
pub const MAX_FORM_BARS: u16 = 64;
/// Bars of a phrase; a form change lands on the next phrase boundary.
pub const FORM_CHANGE_BARS: u64 = 4;

// MUSICAL: first draft, tune by ear on the hardware (Q30). Ordered from
// simple to detailed.
pub const DEFAULT_FORMS: [&str; 9] = [
    "A3 B1",
    "A7 C1",
    "A3 B1 A3 C1",
    "A6 C2",
    "A15 C1",
    "A7 B1 A'7 C1",
    "A4 B4 A6 C2",
    "A7 B1 A'7 C1 B7 A1 B'6 D2",
    "A15 C1 A'15 C1 B15 D1 A14 C2",
];

/// A seed letter of a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Letter {
    A,
    B,
    C,
    D,
}

impl Letter {
    pub const ALL: [Letter; 4] = [Letter::A, Letter::B, Letter::C, Letter::D];

    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn class(self) -> SeedClass {
        match self {
            Letter::A | Letter::B => SeedClass::Main,
            Letter::C | Letter::D => SeedClass::Filler,
        }
    }

    pub fn as_char(self) -> char {
        (b'A' + self as u8) as char
    }

    fn from_char(c: char) -> Option<Self> {
        Some(match c.to_ascii_uppercase() {
            'A' => Letter::A,
            'B' => Letter::B,
            'C' => Letter::C,
            'D' => Letter::D,
            _ => return None,
        })
    }
}

/// What kind of material a letter seed renders (Q23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SeedClass {
    /// Repetitive, mostly on the grid, normal sounds (A, B).
    #[default]
    Main,
    /// Triplets, swing, push and pull, busier, odd sample picks (C, D).
    Filler,
}

/// One segment of a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub letter: Letter,
    /// `A'`: same seed, different second half.
    pub prime: bool,
    pub bars: u16,
}

/// A lane form, e.g. `A7 B1 A'7 C1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    segments: Vec<Segment>,
    bars: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormError {
    #[error("empty form")]
    Empty,
    #[error("bad segment '{0}' (expected a letter A-D, an optional ' and a bar count, like A7 or B'4)")]
    Segment(String),
    #[error("form is {0} bars, at most {MAX_FORM_BARS} allowed")]
    TooLong(u32),
}

impl Form {
    /// Parse `"A7 B1 A'7 C1"`. A segment without a count is one bar.
    pub fn parse(text: &str) -> Result<Self, FormError> {
        let mut segments = Vec::new();
        let mut total = 0u32;
        for token in text.split_whitespace() {
            let bad = || FormError::Segment(token.to_string());
            let mut chars = token.chars();
            let letter = chars.next().and_then(Letter::from_char).ok_or_else(bad)?;
            let rest = chars.as_str();
            let (prime, count) = match rest.strip_prefix('\'').or_else(|| rest.strip_prefix('′')) {
                Some(r) => (true, r),
                None => (false, rest),
            };
            let bars: u16 = if count.is_empty() { 1 } else { count.parse().map_err(|_| bad())? };
            if bars == 0 {
                return Err(bad());
            }
            total += bars as u32;
            segments.push(Segment { letter, prime, bars });
        }
        if segments.is_empty() {
            return Err(FormError::Empty);
        }
        if total > MAX_FORM_BARS as u32 {
            return Err(FormError::TooLong(total));
        }
        Ok(Self { segments, bars: total as u16 })
    }

    /// One bar of A forever (a style without forms).
    pub fn single() -> Self {
        Self { segments: vec![Segment { letter: Letter::A, prime: false, bars: 1 }], bars: 1 }
    }

    /// The built-in forms ([`DEFAULT_FORMS`]).
    pub fn defaults() -> Vec<Self> {
        DEFAULT_FORMS.iter().map(|f| Self::parse(f).expect("built-in form parses")).collect()
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Length in bars.
    pub fn bars(&self) -> u16 {
        self.bars
    }

    /// Where global bar `bar` falls in this form.
    pub fn pos(&self, bar: u64) -> FormPos {
        let at = (bar % self.bars as u64) as u16;
        let mut start = 0u16;
        let mut main_start = 0u16;
        for (i, seg) in self.segments.iter().enumerate() {
            if seg.letter.class() == SeedClass::Main {
                main_start = start;
            }
            if at < start + seg.bars {
                let next = self.segments[(i + 1) % self.segments.len()];
                let in_segment = at - start;
                return FormPos {
                    segment: i as u8,
                    letter: seg.letter,
                    prime: seg.prime,
                    bar_in_segment: in_segment,
                    segment_bars: seg.bars,
                    // Filler bars keep counting from the last main segment.
                    bars_since_main: if seg.letter.class() == SeedClass::Main { in_segment } else { at - main_start },
                    last_of_segment: in_segment + 1 == seg.bars,
                    next_letter: next.letter,
                };
            }
            start += seg.bars;
        }
        // Unreachable for a parsed form (at < bars).
        FormPos::default()
    }
}

impl fmt::Display for Form {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.segments.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{}{}{}", s.letter.as_char(), if s.prime { "'" } else { "" }, s.bars)?;
        }
        Ok(())
    }
}

/// A bar's position inside a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormPos {
    pub segment: u8,
    pub letter: Letter,
    pub prime: bool,
    pub bar_in_segment: u16,
    pub segment_bars: u16,
    /// Bars since the last main segment (A/B) started. The chord
    /// progression counts from there.
    pub bars_since_main: u16,
    pub last_of_segment: bool,
    /// Letter of the segment after this one (wrapping).
    pub next_letter: Letter,
}

impl Default for FormPos {
    fn default() -> Self {
        Self {
            segment: 0,
            letter: Letter::A,
            prime: false,
            bar_in_segment: 0,
            segment_bars: 1,
            bars_since_main: 0,
            last_of_segment: true,
            next_letter: Letter::A,
        }
    }
}

// RNG streams (form level).
const S_LETTER: u32 = 0xF00;
const S_PRIME: u32 = 0xF01;
const S_FEEL_SWING: u32 = 0xF10;
const S_FEEL_SWING_AMT: u32 = 0xF11;
const S_FEEL_TRIPLET: u32 = 0xF12;
const S_FEEL_DRIFT: u32 = 0xF13;
const S_DRIFT_STEP: u32 = 0xF14;
const S_CHORD_BPC: u32 = 0xF20;
const S_CHORD_TURN: u32 = 0xF21;
const S_CHORD_PASS: u32 = 0xF22;
const S_CHORD_PASS_DIR: u32 = 0xF23;

/// Seed of `letter` for a lane's master seed. A keeps the master seed itself,
/// so a style's default seeds sound like the old single-seed lanes.
pub fn letter_seed(master: u32, lane: u8, letter: Letter) -> u32 {
    match letter {
        Letter::A => master,
        _ => rng::hash(master, lane, letter.index(), 0, S_LETTER) as u32,
    }
}

/// Seed of the second half of a primed letter.
pub fn prime_seed(seed: u32, lane: u8) -> u32 {
    rng::hash(seed, lane, 0, 0, S_PRIME) as u32
}

/// Timing feel a letter seed carries (Q23). Main seeds: mostly straight,
/// sometimes light swing, a few ms of drift. Filler seeds: triplets, heavy
/// swing, larger push and pull.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Feel {
    /// Swing of the off 16ths, 0..1 of [`MAX_SWING_TICKS`].
    pub swing: f32,
    /// Reflow the bar onto a triplet grid.
    pub triplet: bool,
    /// Largest timing drift of an event in ticks (seeded per step).
    pub drift: u16,
}

// MUSICAL: first draft, tune by ear.
/// Largest swing delay of an off 16th (a third of a step).
pub const MAX_SWING_TICKS: u16 = TICKS_PER_STEP / 3;
const MAIN_SWING_PROB: f32 = 0.3;
const MAIN_SWING: (f32, f32) = (0.15, 0.4);
const MAIN_DRIFT_PROB: f32 = 0.5;
const MAIN_DRIFT_TICKS: u16 = 1;
const FILLER_TRIPLET_PROB: f32 = 0.4;
const FILLER_SWING_PROB: f32 = 0.6;
const FILLER_SWING: (f32, f32) = (0.45, 1.0);
const FILLER_DRIFT_TICKS: u16 = 3;

impl Feel {
    /// The feel a letter seed draws.
    pub fn of(seed: u32, lane: u8, class: SeedClass) -> Self {
        let h = |stream| rng::hash(seed, lane, 0, 0, stream);
        match class {
            SeedClass::Main => Self {
                swing: if rng::chance(h(S_FEEL_SWING), MAIN_SWING_PROB) {
                    rng::range_f32(h(S_FEEL_SWING_AMT), MAIN_SWING.0, MAIN_SWING.1)
                } else {
                    0.0
                },
                triplet: false,
                drift: if rng::chance(h(S_FEEL_DRIFT), MAIN_DRIFT_PROB) { MAIN_DRIFT_TICKS } else { 0 },
            },
            SeedClass::Filler => {
                let triplet = rng::chance(h(S_FEEL_TRIPLET), FILLER_TRIPLET_PROB);
                Self {
                    swing: if !triplet && rng::chance(h(S_FEEL_SWING), FILLER_SWING_PROB) {
                        rng::range_f32(h(S_FEEL_SWING_AMT), FILLER_SWING.0, FILLER_SWING.1)
                    } else {
                        0.0
                    },
                    triplet,
                    drift: FILLER_DRIFT_TICKS,
                }
            }
        }
    }

    /// Apply the feel to a rendered bar, keeping it sorted. The first event
    /// of every beat-one hit stays on the grid, so lanes keep locking.
    pub fn apply(&self, out: &mut BarEvents, seed: u32, lane: u8) {
        if self.swing <= 0.0 && !self.triplet && self.drift == 0 {
            return;
        }
        let beat = TICKS_PER_BAR / 4;
        let mut moved = BarEvents::default();
        let mut last: Option<(u16, u8, EventKind)> = None;
        for ev in out.as_slice() {
            let mut e: LiveEvent = *ev;
            let in_beat = e.tick % beat;
            let beat_start = e.tick - in_beat;
            if self.triplet {
                // 16ths of a beat onto 8th triplets: 0 → 0, 1 → ⅓, 2 and 3 → ⅔.
                let third = beat / 3;
                let slot = match in_beat * 4 / beat {
                    0 => 0,
                    1 => 1,
                    _ => 2,
                };
                e.tick = beat_start + slot * third + (in_beat % TICKS_PER_STEP) * 4 / 3;
                if e.gate > 0 {
                    e.gate = (e.gate as u32 * 4 / 3).min(u16::MAX as u32) as u16;
                }
            } else if self.swing > 0.0 && (e.tick / TICKS_PER_STEP) % 2 == 1 {
                e.tick += (self.swing * MAX_SWING_TICKS as f32).round() as u16;
            }
            if self.drift > 0 && e.tick % beat != 0 {
                let h = rng::hash(seed, lane, 0, (e.tick / TICKS_PER_STEP) as u8, S_DRIFT_STEP);
                let d = rng::range_i32(h, -(self.drift as i32), self.drift as i32);
                e.tick = (e.tick as i32 + d).max(0) as u16;
            }
            e.tick = e.tick.min(TICKS_PER_BAR - 1);
            e.gate = e.gate.min(TICKS_PER_BAR - e.tick);
            // Two onsets of one target merged onto the same triplet: keep one.
            if matches!(e.kind, EventKind::Trigger | EventKind::NoteOn) && last == Some((e.tick, e.target, e.kind)) {
                continue;
            }
            last = Some((e.tick, e.target, e.kind));
            moved.push(e);
        }
        moved.sort();
        *out = moved;
    }
}

/// Splice: keep `out` before the half bar, take `second` from it on.
pub fn splice_second_half(out: &mut BarEvents, second: &BarEvents) {
    let half = TICKS_PER_BAR / 2;
    let mut spliced = BarEvents::default();
    for e in out.as_slice().iter().filter(|e| e.tick < half) {
        spliced.push(*e);
    }
    for e in second.as_slice().iter().filter(|e| e.tick >= half) {
        spliced.push(*e);
    }
    *out = spliced;
}

// MUSICAL: first draft, tune by ear (Q32).
/// Chords in a lane's seeded progression.
const PROGRESSION_CHORDS: u8 = 4;
/// Share of filler bars that play a passing chord.
const PASSING_CHORD_PROB: f32 = 0.35;
/// Turnaround degrees: IV and V.
const TURNAROUND_DEGREES: [u8; 2] = [3, 4];
/// Degrees per octave of a diatonic scale.
const SCALE_DEGREES: u8 = 7;

/// The chord degree a pitched lane plays in a bar (Q32).
///
/// The master seed draws a 4-chord progression, 1 or 2 bars per chord,
/// starting near the tonic. It restarts at every main segment (A/B) and keeps
/// running through filler segments, which may add a passing chord. The bar
/// before the form returns to A plays a turnaround (IV or V) so A lands on
/// the tonic again.
pub fn form_chord(master: u32, lane: u8, pos: &FormPos, letter_seed: u32, minor: bool) -> u8 {
    let dim = if minor { 1 } else { 6 };
    let turnaround = pos.last_of_segment && pos.next_letter == Letter::A && pos.letter != Letter::A;
    if turnaround {
        let h = rng::hash(master, lane, 0, 0, S_CHORD_TURN);
        return TURNAROUND_DEGREES[rng::below(h, TURNAROUND_DEGREES.len() as u32) as usize];
    }
    let bars_per_chord = 1 + rng::below(rng::hash(master, lane, 0, 0, S_CHORD_BPC), 2) as u16;
    let index = (pos.bars_since_main / bars_per_chord) as u8 % PROGRESSION_CHORDS;
    let degree = seeded_chord(master, lane, index, minor);
    if pos.letter.class() == SeedClass::Filler {
        let step = pos.bar_in_segment as u8;
        if rng::chance(rng::hash(letter_seed, lane, 0, step, S_CHORD_PASS), PASSING_CHORD_PROB) {
            let up = rng::chance(rng::hash(letter_seed, lane, 0, step, S_CHORD_PASS_DIR), 0.5);
            let mut passing = if up { (degree + 1) % SCALE_DEGREES } else { (degree + SCALE_DEGREES - 1) % SCALE_DEGREES };
            if passing == dim {
                passing = if up { (passing + 1) % SCALE_DEGREES } else { (passing + SCALE_DEGREES - 1) % SCALE_DEGREES };
            }
            return passing;
        }
    }
    degree
}

// MUSICAL: first draft, tune by ear. Generator parameters a filler bar
// pushes up (by name, added to the control value, clamped to 0..1).
pub const FILLER_BOOSTS: [(&str, f32); 7] = [
    ("density", 0.15),
    ("mutate", 0.45),
    ("chop", 0.35),
    ("variation", 0.5),
    ("complexity", 0.3),
    ("spread", 0.25),
    ("accent", 0.2),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::generator::blocks::trigger;

    #[test]
    fn parses_and_prints_forms() {
        let f = Form::parse("A7 B1 A'7 C1").unwrap();
        assert_eq!(f.bars(), 16);
        assert_eq!(f.to_string(), "A7 B1 A'7 C1");
        assert_eq!(Form::parse("a b2").unwrap().to_string(), "A1 B2");
        assert!(matches!(Form::parse(""), Err(FormError::Empty)));
        assert!(matches!(Form::parse("E4"), Err(FormError::Segment(_))));
        assert!(matches!(Form::parse("A0"), Err(FormError::Segment(_))));
        assert!(matches!(Form::parse("A60 C5"), Err(FormError::TooLong(65))));
        for f in DEFAULT_FORMS {
            assert_eq!(Form::parse(f).unwrap().to_string(), f, "built-in forms round-trip");
        }
    }

    #[test]
    fn positions_follow_the_segments() {
        let f = Form::parse("A3 B1 A'3 C1").unwrap();
        let letters: String = (0..9).map(|b| f.pos(b).letter.as_char()).collect();
        assert_eq!(letters, "AAABAAACA");
        let p = f.pos(4);
        assert!(p.prime && p.bar_in_segment == 0 && p.segment == 2);
        let c = f.pos(7);
        assert!(c.last_of_segment && c.next_letter == Letter::A);
        assert_eq!(c.bars_since_main, 3, "filler counts from the last main start");
        assert_eq!(f.pos(3).bars_since_main, 0, "B is main and restarts");
    }

    #[test]
    fn letter_seeds_and_classes() {
        assert_eq!(letter_seed(50, 1, Letter::A), 50);
        let b = letter_seed(50, 1, Letter::B);
        assert_ne!(b, 50);
        assert_eq!(b, letter_seed(50, 1, Letter::B));
        assert_ne!(b, letter_seed(50, 2, Letter::B));
        assert_eq!(Letter::C.class(), SeedClass::Filler);
        assert_eq!(Letter::B.class(), SeedClass::Main);
    }

    #[test]
    fn filler_feels_are_looser_than_main() {
        let mut triplets = 0;
        for seed in 0..400 {
            let m = Feel::of(seed, 0, SeedClass::Main);
            assert!(!m.triplet && m.drift <= MAIN_DRIFT_TICKS && m.swing <= MAIN_SWING.1);
            let f = Feel::of(seed, 0, SeedClass::Filler);
            triplets += f.triplet as u32;
            assert_eq!(f.drift, FILLER_DRIFT_TICKS);
        }
        assert!((100..220).contains(&triplets), "{triplets}");
    }

    #[test]
    fn triplet_feel_reflows_and_merges() {
        let mut bar = BarEvents::default();
        for s in 0..4u16 {
            bar.push(trigger(s * TICKS_PER_STEP, 8, 100));
        }
        Feel { swing: 0.0, triplet: true, drift: 0 }.apply(&mut bar, 1, 0);
        let ticks: Vec<u16> = bar.as_slice().iter().map(|e| e.tick).collect();
        assert_eq!(ticks, [0, 32, 64], "steps 3 and 4 share the last triplet");
    }

    #[test]
    fn swing_and_drift_keep_beats_and_order() {
        let mut bar = BarEvents::default();
        for s in 0..16u16 {
            bar.push(trigger(s * TICKS_PER_STEP, 8, 100));
        }
        Feel { swing: 1.0, triplet: false, drift: 3 }.apply(&mut bar, 7, 0);
        let ev = bar.as_slice();
        assert_eq!(ev.len(), 16);
        assert!(ev.windows(2).all(|w| w[0].tick <= w[1].tick));
        for b in 0..4 {
            assert!(ev.iter().any(|e| e.tick == b * 96), "beat {b} stays on the grid");
        }
        assert!(ev.iter().any(|e| e.tick % TICKS_PER_STEP != 0));
    }

    #[test]
    fn chords_restart_at_main_segments_and_turn_around() {
        let f = Form::parse("A3 B1 A3 C1").unwrap();
        for master in 0..50 {
            let at = |bar: u64| {
                let p = f.pos(bar);
                form_chord(master, 1, &p, letter_seed(master, 1, p.letter), true)
            };
            assert_eq!(at(0), at(4), "A restarts the progression");
            assert!(TURNAROUND_DEGREES.contains(&at(3)), "B before A turns around");
            assert!(TURNAROUND_DEGREES.contains(&at(7)), "C before A turns around");
            for bar in 0..8 {
                assert!(at(bar) < 7 && at(bar) != 1, "diatonic, no diminished");
            }
        }
    }

    #[test]
    fn splice_takes_the_second_half_from_the_other_bar() {
        let mut a = BarEvents::default();
        let mut b = BarEvents::default();
        for t in [0u16, 96, 192, 288] {
            a.push(trigger(t, 1, 100));
            b.push(trigger(t, 2, 100));
        }
        splice_second_half(&mut a, &b);
        let targets: Vec<u8> = a.as_slice().iter().map(|e| e.target).collect();
        assert_eq!(targets, [1, 1, 2, 2]);
    }
}
