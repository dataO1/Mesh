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
// steady to busy. Each is a composed 64-bar pass, one line per 16 bars:
// a steady main base, C/D at the 16-bar breaking points, a breakout at the
// end of 64. `'` varies a bar's second half, `*` swaps the lane's sound
// (down to a single beat, `.25`). The second pass varies it (128 bars).
pub const DEFAULT_FORMS: [&str; 10] = [
    // Steady: long main runs, a swap tick and a swapped break.
    concat!("A15 C1 ", "A'7 A'*.25 A'.75 A7 D1 ", "B15 C*1 ", "A'7 B1 A6 C*1 D*1"),
    // Roller: A with B answers, half-bar swaps into the breaks.
    concat!(
        "A7 B1 A'6 A'*.5 A'.5 C1 ",
        "A7 B*1 A'6 C.5 D*.5 C1 ",
        "A3 B1 A3 B'1 A3 B1 A2 C*1 C.5 D.5 ",
        "A'7 B1 A6 C1 D*1"
    ),
    // B-led: the second half of each phrase moves to B.
    concat!("A7 B1 B'7 C1 ", "B7 A'1 B'6 C*.5 D*.5 C1 ", "A3 B1 A'3 B*1 A3 B1 A'2 C1 D1 ", "B7 B*1 A'6 D*2"),
    // Two-step build: 4-bar cells, swaps and half-bar breaks tighten.
    concat!(
        "A4 A'4 A4 A'3 C1 ",
        "B4 B'4 B3 B*1 B'3 D1 ",
        "A4 A'3.5 C*.5 A4 A'3 C.5 C*.5 ",
        "B4 A4 B'3.5 D*.5 A'2 C1 D*1"
    ),
    // EDM build: a long run, a 16-bar breakdown on the fillers, the drop back.
    concat!("A8 A'7 C1 ", "A8 B7 D*1 ", "C*4 C'4 D4 D*3.5 A.5 ", "A4 A'4 B4 A'2 C*1 D*1"),
    // Breakouts: fills at the 8-bar points too, swapped half bars.
    concat!(
        "A6 C1 A'7 D*.5 C*.5 C1 ",
        "B6 C1 B'7 D1 C*1 ",
        "A4 C*.5 A3.5 B4 D*.5 B'2.5 C1 ",
        "A'6 B*2 A6 C*1 D*1"
    ),
    // Call and answer: every 4th bar ends on a half-bar swap.
    concat!(
        "A3.5 A*.5 A3.5 B*.5 A'3.5 A'*.5 A3 C1 ",
        "B3.5 B*.5 B3.5 A*.5 B'3.5 B'*.5 B3 D1 ",
        "A7 C*1 A'7 D*1 ",
        "A3.5 A*.5 B3.5 B*.5 A'3 C.5 C*.5 A2 C*1 D*1"
    ),
    // Jungle chops: one-beat swaps and half-bar fills inside 4-bar cells.
    concat!(
        "A3 A'.75 C*.25 A3 B1 A3 A'.5 D*.5 A3 C1 ",
        "B3 B'.75 D*.25 B3 A1 B3 B'.5 C*.5 B2 C1 D1 ",
        "A3 A'.75 C*.25 A3 B1 A3 C.5 C*.5 A3 D*1 ",
        "B4 A4 B'3 C*.5 D*.5 A2 C1 D*1"
    ),
    // Sound per bar: the lane swaps its sound every other bar.
    concat!(
        "A1 A*1 A1 B*1 A'1 A'*1 A1 C*1 A1 A*1 A1 B*1 A'1 A'*1 A1 C*1 ",
        "A1 A*1 A1 B*1 A'1 A'*1 A1 C*1 A1 A*.5 A1.5 B*1 A'1 A'*1 C1 D*1 ",
        "B2 B*2 B'2 A*2 B2 B*2 B'3 D*1 ",
        "A1 A*1 A1 B*1 A'1 A'*1 A1 C*1 A1 A*1 A1 B*1 C1 C*1 D*2"
    ),
    // Sound per bar throughout, B answering in the third phrase.
    concat!(
        "A1 A*1 A1 B*1 A'1 A'*1 A1 C*1 A1 A*1 A1 B*1 A'1 A'*1 A1 C*1 ",
        "A1 A*1 A1 B*1 A'1 A'*1 A1 C*1 A1 A*1 A1 B*1 A'1 A'*1 A1 D*1 ",
        "B1 B*1 B1 A*1 B'1 B'*1 B1 D*1 B1 B*1 B1 A*1 B'1 B'*.5 C*.5 C1 D*1 ",
        "A1 A*1 A1 B*1 A'1 A'*1 A1 C*1 A1 A*1 A1 B*1 C1 C*1 D*2"
    ),
];

// MUSICAL: first draft, tune by ear (user: the lead riff often does not play
// at all in the first or second 64 bars, or only in breaks; several complex
// drop forms with variety, coined to the synths). The synth lanes' form paired
// with each of DEFAULT_FORMS, one line per 16 bars: `-8` rests, `~A8` plays
// only in the drums' breaks. The second pass opens some rests into
// break-only answers and some break-only segments into the full riff.
pub const DEFAULT_LEAD_FORMS: [&str; 10] = [
    // Steady: enters after 16 bars, answers in the breaks, riff and B to the end.
    "-16 A15 C1 ~A16 A'8 B8",
    // Roller: break answers, then 8-bar riffs with rests between.
    "-8 ~A8 A7 C1 -8 A'8 B8 -8 A7 D1",
    // B-led: two 16-bar statements on B and A', 16 bars of rest before each.
    "-16 B8 B'8 -16 A'8 B8",
    // Two-step: 4-bar calls with 4-bar gaps, a rest, then a full 16.
    "A4 -4 A'4 -4 B4 -4 B'4 -4 -16 A4 A'4 B4 A'3 C1",
    // EDM build: riff in the first drop, break-only through the breakdown, riff back.
    "-16 A15 C1 ~A16 A'8 B7 D1",
    // Breakouts: break answers, short riff runs, a long rest at the end.
    "-8 ~B8 A6 C1 -1 -8 ~B8 A'7 D1 -8 -8",
    // Call and answer: a 3.5-bar call every 8 bars, the second half silent.
    "A3.5 -.5 -4 A'3.5 -.5 -4 B3.5 -.5 -4 A3 C1 -4 -32",
    // Jungle chops: rest, break answers, a 12-bar riff, rest.
    "-16 ~A16 A4 B4 A'4 -4 -16",
    // Second half only: silent for 32 bars, then riff, answer, riff.
    "-32 A8 B8 A'8 -8",
    // Alternating: 8 on, 8 off, break answers, riff.
    "A8 -8 B8 -8 ~A16 A'8 -8",
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

    /// The other letter of the same class (A and B, C and D).
    pub fn partner(self) -> Letter {
        match self {
            Letter::A => Letter::B,
            Letter::B => Letter::A,
            Letter::C => Letter::D,
            Letter::D => Letter::C,
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

/// Whether a segment plays (lead and extra forms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Play {
    /// The lane plays the letter.
    #[default]
    All,
    /// `-8`: the lane rests.
    Rest,
    /// `~A8`: the lane plays the letter only in the drums' break bars (their
    /// C and D segments) and rests elsewhere.
    Breaks,
}

/// One segment of a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub letter: Letter,
    pub play: Play,
    /// `A'`: same seed, different second half.
    pub prime: bool,
    /// `A*`: the lane plays another of its sounds here (instrument option or
    /// drum kit, seeded).
    pub swap: bool,
    /// Length in beats (quarter bars): `A7` is 28, `C*.5` is 2.
    pub beats: u16,
    /// Index of the written segment this one comes from, [`NO_SOURCE`] for a
    /// break the form's structure inserted.
    pub source: u8,
}

impl Segment {
    /// Bars the segment touches, rounded up.
    pub fn bars(&self) -> u16 {
        self.beats.div_ceil(BEATS_PER_BAR)
    }
}

/// [`Segment::source`] of an inserted break.
pub const NO_SOURCE: u8 = u8::MAX;
/// Form resolution: segments are whole beats (`.25` of a bar).
pub const BEATS_PER_BAR: u16 = 4;

/// A lane form, e.g. `A7 B1 A'6.5 C*.5 C1`. A pattern shorter than
/// [`FORM_BARS`] repeats to it, with breaks at the structure points
/// ([`STRUCTURE_BREAKS`]). The form then plays a second time as a seeded
/// variation ([`Form::pos_varied`]): every form is a 128-bar cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// As written (display).
    pattern: Vec<Segment>,
    /// The form of the synth lanes (lead and extra), written after a `|`:
    /// `A15 C1 | -16 A16 ~B16 A'16`. None: they play the main form.
    lead: Option<Box<Form>>,
    /// Expanded to [`FORM_BARS`].
    segments: Vec<Segment>,
    /// Beats of one pass.
    beats: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormError {
    #[error("empty form")]
    Empty,
    #[error("bad segment '{0}' (expected a letter A-D, optional ' and *, and a length in bars in quarters, like A7, B'4, C*.5 or D*.25; -8 rests, ~A8 plays only in the drums' breaks; a | starts the synth lanes' form)")]
    Segment(String),
    #[error("form is {0} bars, at most {MAX_FORM_BARS} allowed")]
    TooLong(u32),
}

/// Bars one pass of a form fills (the cycle is twice this).
pub const FORM_BARS: u16 = 64;

// MUSICAL: first draft, tune by ear (user: steady base, breaks at the
// 16-bar points, a bigger breakout at the end of 64, like an EDM build).
/// Breaks the structure puts over main bars of a pattern shorter than 64
/// bars: bar range in the pass, letter, and whether the lane swaps its sound.
pub const STRUCTURE_BREAKS: [(u16, u16, Letter, bool); 5] = [
    (15, 16, Letter::C, false),
    (30, 32, Letter::D, false),
    (47, 48, Letter::C, false),
    (60, 62, Letter::C, false),
    (62, 64, Letter::D, true),
];

/// One part of a bar that plays one segment: ticks `from..to` and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarPart {
    pub from: u16,
    pub to: u16,
    pub pos: FormPos,
}

/// Most parts in a bar (one per beat).
pub const MAX_BAR_PARTS: usize = BEATS_PER_BAR as usize;

impl Form {
    /// Parse `"A7 B1 A'6.5 C*.5"`. A segment without a length is one bar;
    /// lengths are bars in quarters (`.25` = one beat).
    pub fn parse(text: &str) -> Result<Self, FormError> {
        if let Some((main, lead)) = text.split_once('|') {
            let mut form = Self::parse(main)?;
            form.lead = Some(Box::new(Self::parse(lead)?));
            return Ok(form);
        }
        let mut pattern = Vec::new();
        let mut total = 0u32;
        for token in text.split_whitespace() {
            let bad = || FormError::Segment(token.to_string());
            // `-8` rests, `~A8` plays only in the drums' breaks.
            let (play, body) = match token.strip_prefix('-') {
                Some(b) => (Play::Rest, b),
                None => match token.strip_prefix('~') {
                    Some(b) => (Play::Breaks, b),
                    None => (Play::All, token),
                },
            };
            let mut chars = body.chars();
            let letter = match chars.clone().next().and_then(Letter::from_char) {
                Some(l) => {
                    chars.next();
                    l
                }
                None if play == Play::Rest => Letter::A,
                None => return Err(bad()),
            };
            let mut rest = chars.as_str();
            let (mut prime, mut swap) = (false, false);
            loop {
                if let Some(r) = rest.strip_prefix('\'').or_else(|| rest.strip_prefix('′')) {
                    prime = true;
                    rest = r;
                } else if let Some(r) = rest.strip_prefix('*') {
                    swap = true;
                    rest = r;
                } else {
                    break;
                }
            }
            let beats = if rest.is_empty() {
                BEATS_PER_BAR as u32
            } else {
                let bars: f32 = rest.parse().map_err(|_| bad())?;
                let beats = bars * BEATS_PER_BAR as f32;
                if !beats.is_finite() || (beats - beats.round()).abs() > 1e-3 || beats.round() < 1.0 {
                    return Err(bad());
                }
                beats.round() as u32
            };
            total += beats;
            if total > (MAX_FORM_BARS * BEATS_PER_BAR) as u32 {
                return Err(FormError::TooLong(total.div_ceil(BEATS_PER_BAR as u32)));
            }
            let source = pattern.len().min(NO_SOURCE as usize - 1) as u8;
            pattern.push(Segment { letter, play, prime, swap, beats: beats as u16, source });
        }
        if pattern.is_empty() {
            return Err(FormError::Empty);
        }
        let segments = expand(&pattern, total as u16);
        Ok(Self { pattern, lead: None, segments, beats: FORM_BARS * BEATS_PER_BAR })
    }

    /// One bar of A forever (a style without forms).
    pub fn single() -> Self {
        let seg = Segment { letter: Letter::A, play: Play::All, prime: false, swap: false, beats: BEATS_PER_BAR, source: 0 };
        Self { pattern: vec![seg], lead: None, segments: vec![seg], beats: BEATS_PER_BAR }
    }

    /// The built-in forms ([`DEFAULT_FORMS`], each with its synth form from
    /// [`DEFAULT_LEAD_FORMS`]).
    pub fn defaults() -> Vec<Self> {
        DEFAULT_FORMS
            .iter()
            .zip(DEFAULT_LEAD_FORMS)
            .map(|(f, l)| Self::parse(&format!("{f} | {l}")).expect("built-in form parses"))
            .collect()
    }

    /// The form a lane plays: synth lanes (lead, extra) play the `|` part when
    /// the form has one.
    pub fn for_lane(&self, synth: bool) -> &Form {
        match &self.lead {
            Some(lead) if synth => lead,
            _ => self,
        }
    }

    /// The synth lanes' form, if the form writes one.
    pub fn lead(&self) -> Option<&Form> {
        self.lead.as_deref()
    }

    /// The segments one pass plays (the pattern repeated, with breaks).
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The segments as written.
    pub fn pattern(&self) -> &[Segment] {
        &self.pattern
    }

    /// Length of one pass in bars.
    pub fn bars(&self) -> u16 {
        self.beats / BEATS_PER_BAR
    }

    /// Where the start of global bar `bar` falls in the form's first pass.
    pub fn pos(&self, bar: u64) -> FormPos {
        self.pos_at(bar * BEATS_PER_BAR as u64)
    }

    /// Where global beat `beat` falls in the form's first pass.
    pub fn pos_at(&self, beat: u64) -> FormPos {
        let at = (beat % self.beats as u64) as u16;
        let mut start = 0u16;
        let mut main_start = 0u16;
        for (i, seg) in self.segments.iter().enumerate() {
            if seg.letter.class() == SeedClass::Main {
                main_start = start;
            }
            if at < start + seg.beats {
                let next = self.segments[(i + 1) % self.segments.len()];
                let in_segment = (at - start) / BEATS_PER_BAR;
                return FormPos {
                    segment: i as u8,
                    source: seg.source,
                    letter: seg.letter,
                    play: seg.play,
                    prime: seg.prime,
                    swap: seg.swap,
                    bar_in_segment: in_segment,
                    segment_bars: seg.bars(),
                    // Filler bars keep counting from the last main segment.
                    bars_since_main: if seg.letter.class() == SeedClass::Main {
                        in_segment
                    } else {
                        (at - main_start) / BEATS_PER_BAR
                    },
                    // The bar that holds the segment's last beat.
                    last_of_segment: (start + seg.beats - 1) / BEATS_PER_BAR == at / BEATS_PER_BAR,
                    next_letter: next.letter,
                    second: false,
                };
            }
            start += seg.beats;
        }
        // Unreachable for a parsed form (at < beats).
        FormPos::default()
    }

    /// Where global bar `bar` falls in the lane's form cycle: the form as
    /// written, then once more as a seeded variation of the same length
    /// (a song-like second half). The variation keeps the segments and the
    /// letter seeds and swaps parts of segments to the other letter of the
    /// same class: `A3 C1` may come back as `A1 B1 A1 D1`. A one-bar form
    /// (a style without forms) never varies.
    pub fn pos_varied(&self, bar: u64, seed: u32, lane: u8) -> FormPos {
        self.pos_varied_at(bar * BEATS_PER_BAR as u64, seed, lane)
    }

    /// [`Self::pos_varied`] at global beat `beat`.
    pub fn pos_varied_at(&self, beat: u64, seed: u32, lane: u8) -> FormPos {
        let len = self.beats as u64;
        let mut pos = self.pos_at(beat);
        if len <= BEATS_PER_BAR as u64 || (beat % (2 * len)) < len {
            return pos;
        }
        pos.second = true;
        let forced = !(0..self.segments.len()).any(|i| vary_pattern(seed, lane, i, self.segments[i], false) != Vary::Keep);
        let i = pos.segment as usize;
        let pattern = vary_pattern(seed, lane, i, self.segments[i], forced && i == 0);
        if pattern.swaps(pos.bar_in_segment, pos.segment_bars) {
            pos.letter = pos.letter.partner();
        }
        pos.play = vary_play(seed, lane, i, pos.play);
        pos
    }

    /// The parts of global bar `bar`: one per segment it touches, in order,
    /// with their tick ranges. Returns the parts and how many there are.
    pub fn parts(&self, bar: u64, seed: u32, lane: u8) -> ([BarPart; MAX_BAR_PARTS], usize) {
        let mut parts = [BarPart { from: 0, to: TICKS_PER_BAR, pos: FormPos::default() }; MAX_BAR_PARTS];
        let mut n = 0usize;
        let beat_ticks = TICKS_PER_BAR / BEATS_PER_BAR;
        for b in 0..BEATS_PER_BAR {
            let pos = self.pos_varied_at(bar * BEATS_PER_BAR as u64 + b as u64, seed, lane);
            let from = b * beat_ticks;
            if n > 0 {
                let last = &mut parts[n - 1];
                if (last.pos.segment, last.pos.letter) == (pos.segment, pos.letter) {
                    last.to = from + beat_ticks;
                    continue;
                }
            }
            parts[n] = BarPart { from, to: from + beat_ticks, pos };
            n += 1;
        }
        parts[n - 1].to = TICKS_PER_BAR;
        (parts, n)
    }
}

/// Repeat `pattern` (`total` beats) to [`FORM_BARS`] and put the structure
/// breaks over its main bars. A pattern that already fills the pass keeps
/// its own breaks.
fn expand(pattern: &[Segment], total: u16) -> Vec<Segment> {
    let pass = (FORM_BARS * BEATS_PER_BAR) as usize;
    let mut beats: Vec<Segment> = Vec::with_capacity(pass);
    while beats.len() < pass {
        for seg in pattern {
            for _ in 0..seg.beats {
                beats.push(Segment { beats: 1, ..*seg });
            }
        }
    }
    beats.truncate(pass);
    let repeats = (total as usize) < pass;
    if repeats {
        for (from, to, letter, swap) in STRUCTURE_BREAKS {
            for b in &mut beats[(from * BEATS_PER_BAR) as usize..(to * BEATS_PER_BAR) as usize] {
                if b.letter.class() == SeedClass::Main && b.play == Play::All {
                    *b = Segment { letter, play: Play::All, prime: false, swap, beats: 1, source: NO_SOURCE };
                }
            }
        }
    }
    // Merge runs of the same segment; a repeat of the pattern starts anew.
    let mut out: Vec<Segment> = Vec::new();
    let mut run_start = 0usize;
    for (i, b) in beats.iter().enumerate() {
        let restart = repeats && i % total as usize == 0 && i > 0;
        match out.last_mut() {
            Some(last)
                if !restart
                    && (last.letter, last.play, last.prime, last.swap, last.source) == (b.letter, b.play, b.prime, b.swap, b.source)
                    && pattern_beats(pattern, last.source) > (i - run_start) as u16 =>
            {
                last.beats += 1;
            }
            _ => {
                out.push(*b);
                run_start = i;
            }
        }
    }
    out
}

/// Beats of written segment `source` (inserted breaks: no limit).
fn pattern_beats(pattern: &[Segment], source: u8) -> u16 {
    pattern.get(source as usize).map_or(u16::MAX, |s| s.beats)
}

/// How the second pass changes one segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vary {
    Keep,
    /// The whole segment takes the partner letter.
    Swap,
    /// Every second bar.
    Alternate,
    /// The last quarter of the segment (at least one bar).
    Last,
    /// The second half.
    SecondHalf,
}

impl Vary {
    fn swaps(self, bar: u16, bars: u16) -> bool {
        match self {
            Vary::Keep => false,
            Vary::Swap => true,
            Vary::Alternate => bar % 2 == 1,
            Vary::Last => bar >= bars - (bars / 4).max(1),
            Vary::SecondHalf => bar >= bars / 2,
        }
    }
}

// MUSICAL: first draft, tune by ear (user: song-like second half).
/// Chance a one-bar (or shorter) main segment swaps letters in the second
/// pass, and a filler segment.
const VARY_MAIN_SWAP: f32 = 0.35;
const VARY_FILLER_SWAP: f32 = 0.5;
/// Weights of keep, alternate, last and second half for longer main segments.
const VARY_MAIN: [(Vary, f32); 4] = [(Vary::Keep, 1.0), (Vary::Alternate, 1.5), (Vary::Last, 1.0), (Vary::SecondHalf, 1.0)];

/// The second-pass change of segment `i`. `force`: never keep.
fn vary_pattern(seed: u32, lane: u8, i: usize, seg: Segment, force: bool) -> Vary {
    let h = rng::hash(seed, lane, i as u8, 0, S_VARY);
    if seg.letter.class() == SeedClass::Filler {
        return if force || rng::chance(h, VARY_FILLER_SWAP) { Vary::Swap } else { Vary::Keep };
    }
    if seg.bars() == 1 {
        return if force || rng::chance(h, VARY_MAIN_SWAP) { Vary::Swap } else { Vary::Keep };
    }
    let mut weights = [0.0f32; VARY_MAIN.len()];
    for (w, &(v, weight)) in weights.iter_mut().zip(&VARY_MAIN) {
        // Halves need four bars; a forced change never keeps.
        *w = if (v == Vary::SecondHalf && seg.bars() < 4) || (force && v == Vary::Keep) { 0.0 } else { weight };
    }
    VARY_MAIN[rng::weighted_pick(rng::derive(h, 1), &weights).unwrap_or(1)].0
}

// MUSICAL: first draft (user: the riff often does not play at all in the
// first or the second 64 bars, or only in breaks).
/// Second pass: chance a rest becomes break-only, and break-only becomes
/// fully played.
const VARY_REST_TO_BREAKS: f32 = 0.4;
const VARY_BREAKS_TO_ALL: f32 = 0.4;

/// The second-pass play of segment `i`: some rests open into break-only
/// answers, some break-only segments into the full riff, so the two halves
/// differ in where the synth plays.
fn vary_play(seed: u32, lane: u8, i: usize, play: Play) -> Play {
    let h = rng::hash(seed, lane, i as u8, 1, S_VARY);
    match play {
        Play::Rest if rng::chance(h, VARY_REST_TO_BREAKS) => Play::Breaks,
        Play::Breaks if rng::chance(h, VARY_BREAKS_TO_ALL) => Play::All,
        p => p,
    }
}

/// A length in beats as bars in quarters: `7`, `6.5`, `.25`.
fn fmt_beats(beats: u16) -> String {
    let (whole, frac) = (beats / BEATS_PER_BAR, beats % BEATS_PER_BAR);
    let frac = ["", ".25", ".5", ".75"][frac as usize];
    if whole == 0 { frac.to_string() } else { format!("{whole}{frac}") }
}

impl fmt::Display for Form {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.pattern.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            let (prime, swap) = (if s.prime { "'" } else { "" }, if s.swap { "*" } else { "" });
            match s.play {
                Play::Rest => write!(f, "-{}", fmt_beats(s.beats))?,
                Play::Breaks => write!(f, "~{}{prime}{swap}{}", s.letter.as_char(), fmt_beats(s.beats))?,
                Play::All => write!(f, "{}{prime}{swap}{}", s.letter.as_char(), fmt_beats(s.beats))?,
            }
        }
        if let Some(lead) = &self.lead {
            write!(f, " | {lead}")?;
        }
        Ok(())
    }
}

/// A bar's position inside a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormPos {
    pub segment: u8,
    /// Written segment it comes from ([`NO_SOURCE`] for a structure break).
    pub source: u8,
    pub letter: Letter,
    /// Whether the lane plays here (rests and break-only segments).
    pub play: Play,
    pub prime: bool,
    /// The lane plays another of its sounds (`*`).
    pub swap: bool,
    pub bar_in_segment: u16,
    pub segment_bars: u16,
    /// Bars since the last main segment (A/B) started. The chord
    /// progression counts from there.
    pub bars_since_main: u16,
    pub last_of_segment: bool,
    /// Letter of the segment after this one (wrapping).
    pub next_letter: Letter,
    /// In the second pass of the form (its variation, see [`Form::pos_varied`]).
    pub second: bool,
}

impl Default for FormPos {
    fn default() -> Self {
        Self {
            segment: 0,
            source: 0,
            letter: Letter::A,
            play: Play::All,
            prime: false,
            swap: false,
            bar_in_segment: 0,
            segment_bars: 1,
            bars_since_main: 0,
            last_of_segment: true,
            next_letter: Letter::A,
            second: false,
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
const S_LETTER_PARAM: u32 = 0xF30;
const S_LETTER_PARAM_SIGN: u32 = 0xF31;
const S_LETTER_PARAM_SECOND: u32 = 0xF32;
const S_VARY: u32 = 0xF40;

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
const MAIN_SWING_PROB: f32 = 0.5;
const MAIN_SWING: (f32, f32) = (0.1, 0.55);
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

// MUSICAL: first draft, tune by ear. Generator parameters a letter other
// than A moves away from the control value (by name, largest offset). A
// plays the controls as set; B, C and D sit around them, so each block
// sounds like a different setting, and the controls still move all of them.
pub const LETTER_OFFSETS: [(&str, f32); 9] = [
    ("density", 0.25),
    ("complexity", 0.6),
    ("variation", 0.35),
    ("slide", 0.45),
    ("spread", 0.4),
    ("accent", 0.3),
    ("swing", 0.35),
    ("chop", 0.3),
    ("mutate", 0.3),
];
/// Share of the largest letter offset each note reads its parameters away
/// by (user: notes within a block differ too).
pub const NOTE_JITTER_SHARE: f32 = 0.5;

/// Smallest share of the largest offset a letter moves a parameter, so a
/// block never lands next to the control value by chance.
const LETTER_OFFSET_MIN: f32 = 0.4;

/// Share of the largest offset the second pass of a form moves a letter on
/// top (A too): same seeds, slightly different settings.
const SECOND_PASS_SHARE: f32 = 0.35;

/// Seeded offset of parameter `param` (index into [`LETTER_OFFSETS`]) for a
/// letter seed. Zero for A in the first pass. `second`: the form's second
/// pass adds a smaller seeded move.
pub fn letter_offset(seed: u32, lane: u8, letter: Letter, param: usize, second: bool) -> f32 {
    let max = LETTER_OFFSETS[param].1;
    let p = param as u8;
    let mut offset = 0.0;
    if letter != Letter::A {
        let size = rng::range_f32(rng::hash(seed, lane, p, 0, S_LETTER_PARAM), LETTER_OFFSET_MIN * max, max);
        offset = if rng::chance(rng::hash(seed, lane, p, 0, S_LETTER_PARAM_SIGN), 0.5) { size } else { -size };
    }
    if second {
        let u = rng::unit_f32(rng::hash(seed, lane, p, 0, S_LETTER_PARAM_SECOND));
        offset += SECOND_PASS_SHARE * max * (2.0 * u - 1.0);
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::generator::blocks::trigger;

    #[test]
    fn synth_forms_rest_and_play_only_in_breaks() {
        let f = Form::parse("A15 C1 A16 B16 A15 D1 | -16 ~A16 B8 -8 A'16").unwrap();
        assert_eq!(f.to_string(), "A15 C1 A16 B16 A15 D1 | -16 ~A16 B8 -8 A'16");
        let lead = f.for_lane(true);
        assert_eq!(lead.bars(), FORM_BARS);
        assert_eq!(f.for_lane(false).pattern().len(), 6, "drums and bass play the main form");
        assert_eq!(lead.pos(0).play, Play::Rest);
        assert_eq!((lead.pos(20).play, lead.pos(20).letter), (Play::Breaks, Letter::A));
        assert_eq!((lead.pos(33).play, lead.pos(33).letter), (Play::All, Letter::B));
        // A pattern with rests keeps them: no structure break is put over a rest.
        let short = Form::parse("-8 A8").unwrap();
        assert!(short.segments().iter().all(|s| s.play == Play::Rest || s.letter == Letter::A || s.letter.class() == SeedClass::Filler));
        assert_eq!(short.pos(15).play, Play::All);
        assert_eq!(short.pos(16).play, Play::Rest);
        assert!(Form::parse("~8").is_err(), "break-only needs a letter");
    }

    #[test]
    fn second_pass_opens_some_rests() {
        let f = Form::parse("-16 A16 -16 A16").unwrap();
        let mut opened = 0;
        for seed in 0..64u32 {
            let p = f.pos_varied(FORM_BARS as u64, seed, 2);
            assert!(p.second);
            if p.play != Play::Rest {
                opened += 1;
            }
        }
        assert!(opened > 5 && opened < 59, "some rests open, most stay: {opened}");
    }

    #[test]
    fn built_in_synth_forms_parse_and_fill_64_bars() {
        for f in Form::defaults() {
            let lead = f.lead().expect("every built-in form has a synth form");
            assert_eq!(lead.bars(), FORM_BARS, "{lead}");
            let rest = (0..FORM_BARS as u64).filter(|&b| lead.pos(b).play != Play::All).count();
            assert!(rest >= 8, "{lead}: the synth rests or answers somewhere");
        }
    }

    #[test]
    fn parses_and_prints_forms() {
        let f = Form::parse("A7 B1 A'7 C1").unwrap();
        assert_eq!(f.bars(), FORM_BARS, "every form fills 64 bars (128 with its second pass)");
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
    fn beat_resolution_parts() {
        let f = Form::parse("A6.5 C*.5 B.25 D*.75").unwrap();
        assert_eq!(f.to_string(), "A6.5 C*.5 B.25 D*.75");
        assert!(matches!(Form::parse("A.3"), Err(FormError::Segment(_))));
        // Bar 6: A for two beats, then the swapped C.
        let (parts, n) = f.parts(6, 1, 1);
        assert_eq!(n, 2);
        assert_eq!((parts[0].from, parts[0].to, parts[0].pos.letter), (0, 192, Letter::A));
        assert_eq!((parts[1].from, parts[1].to, parts[1].pos.letter, parts[1].pos.swap), (192, 384, Letter::C, true));
        // Bar 7: one beat of B, three of swapped D.
        let (parts, n) = f.parts(7, 1, 1);
        assert_eq!(n, 2);
        assert_eq!((parts[0].to, parts[1].pos.letter, parts[1].pos.swap), (96, Letter::D, true));
        assert!(parts[0].pos.last_of_segment && parts[1].pos.last_of_segment);
        // Whole bars are one part.
        assert_eq!(f.parts(0, 1, 1).1, 1);
        // Every built-in form is a written 64 bars.
        for text in DEFAULT_FORMS {
            let form = Form::parse(text).unwrap();
            assert_eq!(form.pattern().iter().map(|s| s.beats as u32).sum::<u32>(), 256, "{text}");
        }
    }

    #[test]
    fn forms_repeat_to_64_bars_with_structure_breaks() {
        let f = Form::parse("A3 B1").unwrap();
        let letters: Vec<(Letter, bool)> = (0..64).map(|b| { let p = f.pos(b); (p.letter, p.swap) }).collect();
        assert_eq!(f.segments().iter().map(|s| s.beats as u32).sum::<u32>(), 256);
        assert_eq!(letters[0..4], [(Letter::A, false), (Letter::A, false), (Letter::A, false), (Letter::B, false)]);
        assert_eq!(letters[15].0, Letter::C, "a break at the end of 16");
        assert_eq!((letters[30].0, letters[31].0), (Letter::D, Letter::D), "a longer one at 32");
        assert_eq!(letters[47].0, Letter::C);
        assert_eq!(letters[60..64], [(Letter::C, false), (Letter::C, false), (Letter::D, true), (Letter::D, true)], "the breakout at 64 swaps the sound");
        assert_eq!(f.pos(15).source, NO_SOURCE);
        assert_eq!(f.pos(4).source, 0, "the pattern repeats from its first segment");
        // Swaps parse and print.
        let s = Form::parse("A1 A*1 A'1 A'*1").unwrap();
        assert_eq!(s.to_string(), "A1 A*1 A'1 A'*1");
        assert!(s.pos(1).swap && s.pos(3).swap && s.pos(3).prime && !s.pos(2).swap);
        // A written 64-bar form keeps its own breaks.
        let full = Form::parse("A15 C1 A'15 C1 B15 D1 A14 C*2").unwrap();
        assert_eq!((full.pos(30).letter, full.pos(31).letter), (Letter::A, Letter::C), "no D break inserted");
        assert!(full.pos(63).swap);
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
    fn second_pass_varies_the_form() {
        let form = Form::parse("A3 C1 A'3 D1").unwrap();
        let mut changed = 0;
        for seed in 0..50u32 {
            let mut differs = false;
            for bar in 0..64u64 {
                let first = form.pos_varied(bar, seed, 2);
                assert!(!first.second);
                assert_eq!(first, form.pos(bar), "the first pass is the form as written");
                let second = form.pos_varied(bar + 64, seed, 2);
                assert!(second.second);
                assert_eq!(second.letter.class(), first.letter.class(), "letters keep their class");
                assert_eq!((second.segment, second.prime), (first.segment, first.prime));
                differs |= second.letter != first.letter;
            }
            assert!(differs, "seed {seed}: the second pass always changes something");
            changed += differs as u32;
            // The cycle is twice the form.
            assert_eq!(form.pos_varied(128, seed, 2), form.pos(0));
        }
        assert_eq!(changed, 50);
        // One-bar forms (no forms) never vary.
        assert!((0..4).all(|b| Form::single().pos_varied(b, 3, 1).letter == Letter::A));
        // Letter offsets: the second pass moves A too, slightly.
        let k = 0;
        let o = letter_offset(7, 1, Letter::A, k, true);
        assert!(o.abs() <= SECOND_PASS_SHARE * LETTER_OFFSETS[k].1 + 1e-6);
    }

    #[test]
    fn letters_move_parameters_around_the_controls() {
        for k in 0..LETTER_OFFSETS.len() {
            assert_eq!(letter_offset(7, 1, Letter::A, k, false), 0.0, "A plays the controls");
        }
        let mut signs = [0usize; 2];
        for letter in [Letter::B, Letter::C, Letter::D] {
            for k in 0..LETTER_OFFSETS.len() {
                let max = LETTER_OFFSETS[k].1;
                let o = letter_offset(letter_seed(7, 1, letter), 1, letter, k, false);
                assert!(o.abs() >= LETTER_OFFSET_MIN * max - 1e-6 && o.abs() <= max + 1e-6, "{o}");
                signs[(o > 0.0) as usize] += 1;
            }
        }
        assert!(signs[0] > 3 && signs[1] > 3, "offsets go both ways: {signs:?}");
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
