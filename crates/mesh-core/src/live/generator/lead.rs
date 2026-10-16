//! Lead generator: motifs, arps and stabs (design §6.3 rule A7).
//!
//! One generator with three modes, selected by the `mode` parameter:
//!
//! - **Motif** (`mode` < 1/3): a 2-4 note motif drawn from the seed and
//!   repeated. The same motif plays in every bar of the phrase; the last bar
//!   varies it (sequence shift, turnaround note). With the construction
//!   option `melody: 1` this mode plays **cells** instead: the seed builds a
//!   palette of 2-3 melodic cells (a rhythm figure, a contour and an
//!   articulation each, e.g. four triplet repeats of a high note, a
//!   staccato 8th run down, a long held note, a syncopated leap up and
//!   back) and orders them over the phrase in call and response (A A B A,
//!   A B A C, …). `spread` sets the range (a 5th at 0, about two octaves at
//!   1), `complexity` how syncopated and busy the figures are, `density` how
//!   many figures a bar holds.
//! - **Arp** (< 2/3): chord tones at a fixed rate, going up, down, up-down or
//!   in a seeded loop.
//! - **Stab** (else): chord stabs on offbeats.
//!
//! **Motif clips**: when the generator has motif clips (and the option
//! `motifs` is not 0) the motif mode plays them instead (see
//! [`super::motif`]): the seed picks a clip that interlocks with the bass's
//! clip, played as call and response, resting on drum fills.
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
//! dotted 8ths, 8ths, 16ths), `spread` (0 = few notes close together,
//! 1 = wide moves and octave jumps over several octaves).
//!
//! Note length and attack follow the density: sparse lines hold long notes
//! with a slow, rounded attack, dense lines play short notes with a sharp
//! one ([`density_attack`]). Notes carry a stream position for mudpie
//! samplers ([`stream_start`]).
//!
//! Construction options: `rule_a3` (phrase shape), `rule_a5` (velocity
//! tiers), `rule_a7` (sparseness against the drums), all defaulting to 1,
//! `melody` (1 = cells in motif mode, default 0) and `motif_chords` (1 = clips
//! follow the form's chords, default; 0 = clips play against the key as written)
//! and `motif_exact` (1 = clips keep every interval, moved to the key root by
//! semitones: reference riffs 1:1).

use std::collections::BTreeMap;

use super::bass::{register_shift, root_octave};
use super::blocks::{
    deja_vu, density_attack, lerp, level_unit, metric_level, metric_weight, note, option, param, reveals,
    step_tick, stream_start, LENGTH_BY_DENSITY,
    velocity, LevelMap, PhraseRng, Tier,
};
use super::motif::{self, Shape};
use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::live::event::BarEvents;
use crate::live::harmony::SCALE_LEN;
use crate::live::motif::{Motif, MAX_CLIPS};
use crate::live::rng;
use crate::live::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

pub const PARAM_NAMES: [&str; 8] =
    ["density", "complexity", "variation", "mode", "register", "rate", "spread", "stack"];
const P_DENSITY: usize = 0;
const P_COMPLEXITY: usize = 1;
const P_VARIATION: usize = 2;
const P_MODE: usize = 3;
const P_REGISTER: usize = 4;
const P_RATE: usize = 5;
const P_SPREAD: usize = 6;
const P_STACK: usize = 7;

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
const S_ORDER: u32 = 0x30A;
const S_FIG: u32 = 0x30B;
const S_PLACE: u32 = 0x30C;
const S_HOME: u32 = 0x30D;

// MUSICAL: first draft, tune by ear. Cells (melody mode).
/// A rhythm figure: onset ticks and the length of the last note.
struct Figure {
    onsets: &'static [u16],
    last: u16,
    /// Weight at complexity 0 and 1.
    weight: (f32, f32),
}

const FIGURES: [Figure; 8] = [
    // Four 8th-triplet repeats.
    Figure { onsets: &[0, 32, 64, 96], last: 32, weight: (0.3, 2.0) },
    // A run of 8ths.
    Figure { onsets: &[0, 48, 96, 144], last: 48, weight: (2.0, 1.0) },
    // One long note.
    Figure { onsets: &[0], last: 192, weight: (2.5, 0.6) },
    // A syncopated pair on the off 16ths.
    Figure { onsets: &[24, 72], last: 48, weight: (0.4, 2.0) },
    // Dotted 8ths.
    Figure { onsets: &[0, 72, 144], last: 48, weight: (1.0, 1.5) },
    // A short pair.
    Figure { onsets: &[0, 48], last: 24, weight: (1.5, 1.0) },
    // A 16th roll.
    Figure { onsets: &[0, 24, 48, 72], last: 24, weight: (0.2, 1.5) },
    // Quarter pulses.
    Figure { onsets: &[0, 96, 192], last: 72, weight: (2.0, 0.5) },
];

/// How a figure's pitches move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Contour {
    Repeat,
    Down,
    Up,
    /// Leap up, then back.
    LeapBack,
    Zigzag,
}

const CONTOURS: [(Contour, f32); 5] =
    [(Contour::Repeat, 2.0), (Contour::Down, 2.0), (Contour::Up, 1.5), (Contour::LeapBack, 1.5), (Contour::Zigzag, 1.0)];

/// Note length as a share of the distance to the next note.
const ARTICULATIONS: [(f32, f32); 3] = [
    (0.3, 1.5), // staccato
    (0.85, 2.0), // even
    (1.0, 1.0), // held to the next
];

/// Call-and-response orders of the palette over the phrase.
const CELL_ORDERS: [[usize; 4]; 6] = [[0, 0, 1, 0], [0, 1, 0, 2], [0, 1, 0, 1], [0, 0, 1, 2], [0, 1, 2, 1], [0, 1, 0, 0]];
/// Figures a bar holds: one up to density 0.3, then two, three, four.
const FIGURES_AT: [f32; 3] = [0.3, 0.55, 0.8];
/// Most figures in a bar.
const MAX_FIGURES: usize = 4;
/// Melody range in scale degrees at spread 0 and 1 (a 5th, about 2 octaves).
const MELODY_RANGE: (f32, f32) = (4.0, 14.0);
/// Step and leap sizes in degrees at spread 0 and 1.
const MELODY_STEP: (f32, f32) = (1.0, 2.5);
const MELODY_LEAP: (f32, f32) = (2.0, 7.0);
/// Beats a figure may start on, and the gap left after it.
const FIGURE_STARTS: [u16; 12] = [0, 96, 192, 288, 48, 144, 240, 336, 24, 120, 216, 312];
const FIGURE_GAP: u16 = 24;

/// One melodic cell: up to 3 figures with their placement and pitches.
#[derive(Clone, Copy, Default)]
struct CellFig {
    figure: usize,
    contour: usize,
    articulation: usize,
    /// First degree (chord-relative).
    anchor: i32,
}

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
// MUSICAL: first draft, tune by ear (user: stacks make chords out of the
// note, not octaves). Chord families over the played note, in the order a
// stack adds their tones (scale degrees, so they follow the key). The seed
// picks a family per chord of its progression.
const VOICINGS: [[i32; MAX_STACK]; 5] = [
    // 5th, 3rd, 7th, 9th, 11th.
    [0, 4, 2, 6, 8, 10],
    // sus2: 2nd, 5th, 7th, 9th (an octave over the 2nd), 11th.
    [0, 1, 4, 6, 8, 10],
    // sus4: 4th, 5th, 7th, 9th, 13th.
    [0, 3, 4, 6, 8, 12],
    // Stacked 4ths.
    [0, 3, 6, 9, 12, 15],
    // Open add9: 5th, 9th, 10th, 7th, 13th.
    [0, 4, 8, 9, 6, 12],
];
const S_FAMILY: u32 = 0x30E;
/// Velocity of a tone just faded in, as a share of the hit's.
const STACK_VEL_FLOOR: f32 = 0.35;
/// Most tones of one hit.
const MAX_STACK: usize = 6;
/// Below this key (C3) two tones closer than a 5th sound muddy.
const MUD_BELOW_KEY: u8 = 48;
const MUD_MIN_INTERVAL: u8 = 7;

// MUSICAL: first draft, tune by ear (research: DnB lead harmony, 2026-10-15).
/// Home degrees of the follow line over the chord root and their weights:
/// root, 5th, b7 (subtonic, open and driving), b3, 4th, b6.
const HOMES: [(i32, f32); 6] = [(0, 0.35), (4, 0.25), (6, 0.15), (2, 0.12), (3, 0.08), (5, 0.05)];
/// Spread flattens the home weights by this much (exponent 1 - x).
const HOME_ADVENTURE: f32 = 0.7;
/// Most notes of a follow-mode stack (at full spread).
const FOLLOW_STACK_MAX: usize = 4;
// MUSICAL: first draft, tune by ear (research: DnB lead recipes, 2026-10-15;
// user: the lead jumped around at random). Riff rhythm cells (bit = 16th
// step) and their weights at complexity 0 and 1.
const RIFF_CELLS: [(u16, f32, f32); 10] = [
    (mask16(b"x--x--x-x--x--x-"), 2.0, 3.0),  // 3-3-2 twice
    (mask16(b"x--x--x--x--x---"), 1.0, 2.5),  // 3-3-3-3-4
    (mask16(b"--x---x---x---x-"), 2.0, 1.0),  // offbeat 8ths
    (mask16(b"x-x-x-x-x-x-x-x-"), 2.5, 1.0),  // straight 8ths
    (mask16(b"x-----x---x-----"), 2.5, 1.0),  // stab and hold
    (mask16(b"x--x-x---x-x--x-"), 0.5, 2.0),  // syncopated stabs
    (mask16(b"x-----------x-x-"), 2.0, 1.0),  // long note and pickup
    (mask16(b"x-xx-xx-x-xx-xx-"), 0.2, 1.5),  // gallop
    (mask16(b"xxxxxxxxxxxxxxxx"), 0.2, 1.5),  // 16th arp
    (mask16(b"x--x--x---------"), 1.0, 1.5),  // call (first half)
];
/// A passing tone in a template: a step into the next chord tone.
const PASS: i32 = i32::MIN;
/// Pitch templates (scale degrees over the riff's home: 0 root, 2 3rd, 4
/// 5th, 6 7th, 7 octave, 8 9th) and their weights at spread 0 and 1.
const RIFF_TEMPLATES: [(&[i32], f32, f32); 10] = [
    (&[0, 7, 0, 7], 2.5, 2.0),                // root-octave bounce
    (&[0, 4, 0, 6, 0, 7, 0, 4], 1.5, 2.5),    // pedal and moving line
    (&[0, 2, 4, 7], 1.0, 2.5),                // triad up
    (&[0, 2, 4, 7, 4, 2], 0.8, 2.0),          // triad up and down
    (&[4, 4, PASS, 4], 2.5, 0.8),             // upper neighbour
    (&[7, 6, 4, PASS, 2], 1.0, 1.5),          // descending line
    (&[4, 4, PASS, 2], 2.0, 1.0),             // hold and fall
    (&[0, 0, 7, 4], 1.0, 2.5),                // octave call
    (&[0, 2, 4, 6, 8], 0.3, 1.5),             // minor 7th / 9th arp
    (&[0, 0, 0, 0], 2.5, 0.5),                // static, the rhythm carries it
];
/// Share of seeds whose riff runs two bars (call, then an answer ending on
/// the 5th).
const RIFF_TWO_BAR_PROB: f32 = 0.5;
/// Chance the phrase's last bar varies the riff, at complexity 0 and 1.
const RIFF_VARIANT_PROB: (f32, f32) = (0.3, 0.8);
/// How much the riff cell steps back from the bass's onsets at full cx.
const RIFF_BASS_AVOID: f32 = 0.7;
/// Importance of the weakest step against a downbeat (density thins weak
/// steps first).
const RIFF_METRIC_FLOOR: f32 = 0.35;
/// Importance of a bass onset outside the cell at cx 0 (plays with the bass).
const RIFF_UNISON: f32 = 0.9;
/// Density from which repeats fill the riff's gaps, and the fill rhythm
/// density at full density; a fill step's importance.
const RIFF_FILL_DENSITY: (f32, f32) = (0.45, 0.65);
const RIFF_FILL: f32 = 0.7;
const S_RIFF_CELL: u32 = 0x30F;
const S_RIFF_TEMPLATE: u32 = 0x310;
const S_RIFF_LEN: u32 = 0x311;
const S_RIFF_VARIANT: u32 = 0x312;

/// A 16-step mask from `x` (onset) and `-`.
const fn mask16(s: &[u8; 16]) -> u16 {
    let mut m = 0u16;
    let mut i = 0;
    while i < 16 {
        if s[i] == b'x' {
            m |= 1 << i;
        }
        i += 1;
    }
    m
}
/// Rhythm density at density 0 and 1 (user: busier than the cells).
const FOLLOW_RHYTHM_DENSITY: (f32, f32) = (0.55, 0.95);
/// Onsets to follow without a bass lane (two-step).
const FOLLOW_FALLBACK_ONSETS: u16 = 1 | 1 << 6 | 1 << 10;
/// Chance per upper chord tone of an octave up, and per hit of an octave
/// jump, at spread 1.
const STAB_OPEN_MAX: f32 = 0.5;
const STAB_JUMP_MAX: f32 = 0.3;
/// Stab rhythm randomness at complexity 1.
const STAB_CHAOS_MAX: f32 = 0.2;

// MUSICAL: first draft, tune by ear. Motif clips.
/// Most notes of a bar after the density's repeats.
const MOTIF_MAX_NOTES: usize = 10;
/// Lowest MIDI key of a motif bar (C2); lower bars move up an octave.
const MOTIF_MIN_KEY: i32 = 36;
/// Complexity from which clips are picked to miss the bass's onsets (below
/// it, to play with them).
const INTERLOCK_CX: f32 = 0.5;
/// Below this complexity a 1-bar clip is spread over the phrase (call in
/// bars 1 and 3, answer in 2 and 4); from it the call and answer share the bar.
const CALL_WITHIN_BAR_CX: f32 = 0.5;
/// From this complexity notes on the bass's steps may drop, and the chance at cx 1.
const YIELD_FROM_CX: f32 = 0.3;
const YIELD_MAX: f32 = 0.85;

// MUSICAL: first draft, tune by ear. Register and articulation.
/// Lowest MIDI key of the key's root at the base register (A3).
const LEAD_ROOT_MIN: i32 = 57;
/// The lead preset sits an octave under the others (user: centre lower).
const LEAD_KIND_ROOT_MIN: i32 = 45;
const GATE_MIN_TICKS: f32 = 6.0;
/// Longest note in steps for motif, arp and stab hits at density 0 and 1;
/// pads hold to the next hit.
const GATE_MAX_STEPS: (f32, f32) = (8.0, 2.0);
/// Pads: note length as a share of the gap at density 0 and 1 (they stay
/// longer than the other modes).
const PAD_LENGTH_BY_DENSITY: (f32, f32) = (1.0, 0.6);

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
            // Register at the bottom: the lead sits an octave below the
            // other presets (user: too high), the octave pads go further.
            LeadKind::Lead => [0.3, 0.3, 0.5, 0.0, 0.0, 0.6, 0.4, 0.4],
            LeadKind::Arp => [0.6, 0.1, 0.5, 0.5, 0.5, 0.6, 0.3, 0.4],
            LeadKind::Stab => [0.4, 0.4, 0.5, 1.0, 0.5, 0.6, 0.3, 0.4],
            LeadKind::Pad => [0.3, 0.5, 0.5, 1.0, 0.5, 0.6, 0.2, 0.4],
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
    /// Motif mode plays seeded melodic cells.
    pub melody: bool,
    /// Motif mode follows the bass lane (rhythm and harmony, see `follow`).
    pub follow: bool,
    /// Motif mode plays motif clips when the generator has them.
    pub motifs: bool,
    /// Clips follow the form's chord (re-pitched to each bar's chord root).
    /// Off: they play against the key only, as written (option `motif_chords`).
    pub motif_chords: bool,
    /// Clips keep every interval as written: moved to the lane's key root by
    /// semitones, ignoring the lane's mode and chord (option `motif_exact`,
    /// for playing reference riffs 1:1).
    pub motif_exact: bool,
}

impl Default for LeadRules {
    fn default() -> Self {
        Self { phrase: true, tiers: 1.0, gaps: 1.0, melody: false, follow: false, motifs: true, motif_chords: true, motif_exact: false }
    }
}

impl LeadRules {
    pub fn from_spec(params: &BTreeMap<String, f32>) -> Self {
        Self {
            phrase: option(params, "rule_a3", 1.0) >= 0.5,
            tiers: option(params, "rule_a5", 1.0),
            gaps: option(params, "rule_a7", 1.0),
            melody: option(params, "melody", 0.0) >= 0.5,
            follow: option(params, "follow", 0.0) >= 0.5,
            motifs: option(params, "motifs", 1.0) >= 0.5,
            motif_chords: option(params, "motif_chords", 1.0) >= 0.5,
            motif_exact: option(params, "motif_exact", 0.0) >= 0.5,
        }
    }
}

/// One onset: up to [`MAX_STACK`] chord-relative degrees on a step.
#[derive(Clone, Copy, Default)]
struct Hit {
    degrees: [i32; MAX_STACK],
    /// Loudness of each tone, 0..1 (stacked tones fade in).
    gains: [f32; MAX_STACK],
    size: u8,
    accent: bool,
    /// The degrees the repeating bars play here (for the turnaround).
    repeated: [i32; MAX_STACK],
}

impl Hit {
    fn single(degree: i32, accent: bool) -> Self {
        let mut h = Hit { size: 1, accent, ..Hit::default() };
        h.degrees[0] = degree;
        h.gains[0] = 1.0;
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
    /// Motif clips. Empty = the procedural modes.
    motifs: Vec<Motif>,
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

/// The chord family the seed plays on this bar's chord ([`VOICINGS`]).
fn voicing_family(ctx: &BarCtx) -> &'static [i32; MAX_STACK] {
    let h = rng::hash(ctx.seed, ctx.lane, ctx.harmony.chord_degree, 0, S_FAMILY);
    &VOICINGS[rng::below(h, VOICINGS.len() as u32) as usize]
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
        Self { kind, defaults, rules, cells, motifs: Vec::new() }
    }

    /// Play these motif clips in motif mode (when the rules allow it).
    pub fn with_motifs(mut self, motifs: Vec<Motif>) -> Self {
        self.motifs = motifs;
        self
    }

    fn uses_motifs(&self) -> bool {
        self.rules.motifs && self.kind == LeadKind::Lead && !self.motifs.is_empty()
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
        // Chord tones from the stack: each further tone fades in.
        let stack = param(f.ctx, P_STACK) * (MAX_STACK - 1) as f32;
        let size = 1 + (stack.ceil() as usize).min(MAX_STACK - 1);
        let chord = &voicing_family(f.ctx)[..size];
        // Seeded inversion: the lowest tones move up an octave.
        let inversion = rng::below(f.rng.base.at(0, S_INVERSION), chord.len() as u32) as usize;
        let mut hit = Hit { size: chord.len() as u8, ..Hit::default() };
        for (i, &degree) in chord.iter().enumerate() {
            hit.degrees[i] = if i < inversion { degree + 7 } else { degree };
            hit.gains[i] = if i == 0 { 1.0 } else { (stack - (i - 1) as f32).clamp(0.0, 1.0) };
            // Spread opens the voicing: upper tones move up an octave.
            if i > 0 && f.rng.base.chance(i as u8, S_VOICING, f.spread * STAB_OPEN_MAX) {
                hit.degrees[i] += SCALE_LEN;
            }
            // Never twice the same tone (the octave and a raised root).
            while hit.degrees[..i].contains(&hit.degrees[i]) {
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

impl LeadGenerator {
    /// Lowest MIDI key of the key's root at the base register.
    fn root_min(&self) -> i32 {
        if self.kind == LeadKind::Lead { LEAD_KIND_ROOT_MIN } else { LEAD_ROOT_MIN }
    }

    /// Motif mode with clips: the seed picks a clip that interlocks with the
    /// bass's clip, and it plays as a question and answer phrase (see
    /// [`super::motif`]). The lead rests on drum fills and, at high
    /// complexity, leaves the steps where the bass plays.
    fn motif_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        if ctx.fill || ctx.drum_fill {
            return;
        }
        let mut keyed = *ctx;
        if !self.rules.motif_chords {
            keyed.harmony.chord_degree = 0;
        }
        let ctx = &keyed;
        let density = param(ctx, P_DENSITY);
        let cx = param(ctx, P_COMPLEXITY);
        let mut weights = [0.0f32; MAX_CLIPS];
        let n = motif::interlock_weights(&self.motifs, &ctx.bass_phrase, cx >= INTERLOCK_CX, &mut weights);
        let weights: &[f32] = if ctx.bass_phrase == [0; 4] { &[] } else { &weights[..n] };
        let clip = &self.motifs[motif::pick(&self.motifs, ctx.seed, ctx.lane, weights)];
        let shape = Shape {
            density,
            spread: param(ctx, P_SPREAD),
            variation: param(ctx, P_VARIATION),
            max_notes: MOTIF_MAX_NOTES,
            lead: true,
            play: motif::call_response(clip, ctx.bar_in_phrase, cx, CALL_WITHIN_BAR_CX),
            yield_to: ctx.bass_onsets,
            yield_prob: ((cx - YIELD_FROM_CX) / (1.0 - YIELD_FROM_CX)).max(0.0) * YIELD_MAX,
        };
        let mut plan = motif::Plan::new();
        motif::plan_bar(clip, ctx, &shape, &mut plan);
        if plan.as_slice().is_empty() {
            return;
        }
        let octave = root_octave(ctx.harmony.key, self.root_min()) + register_shift(param(ctx, P_REGISTER));
        let exact = self.rules.motif_exact;
        let pitch = |fold: i32, p: &motif::PlanNote| {
            if exact {
                ctx.harmony.resolve_in_key(0, octave) as i32 + motif::clip_semitones(clip, p) + 12 * p.lift as i32 + fold
            } else {
                motif::note_key(ctx, octave, fold, p)
            }
        };
        let fold = if exact {
            let lowest = plan.as_slice().iter().map(|p| pitch(0, p)).min().unwrap_or(MOTIF_MIN_KEY);
            let mut f = 0;
            while lowest + f < MOTIF_MIN_KEY && f < 48 {
                f += 12;
            }
            f
        } else {
            motif::fold(ctx, octave, &plan, MOTIF_MIN_KEY)
        };
        let attack = density_attack(density);
        for (i, p) in plan.as_slice().iter().enumerate() {
            let key = pitch(fold, p).clamp(0, 127);
            let step = p.step() as u8;
            let mut ev = note(p.tick, key as u8, p.vel, p.gate, 2 * i as u16 + 1);
            ev.attack = attack;
            ev.start = stream_start(ctx, step, 0);
            out.push(ev);
            if p.double && key + 12 <= 127 {
                let vel = ((p.vel as f32 * motif::DOUBLE_VEL).round() as u8).max(1);
                let mut up = note(p.tick, (key + 12) as u8, vel, p.gate, 2 * i as u16 + 2);
                up.attack = attack;
                up.start = stream_start(ctx, step, 1);
                out.push(up);
            }
        }
    }

    /// Follow mode: a seeded riff that plays with the bass. The letter seed
    /// draws a rhythm cell, a pitch template of chord tones and a riff length
    /// (1 or 2 bars); the riff repeats exactly and varies only in the last bar
    /// of the phrase. `complexity` moves the rhythm from the bass's own onsets
    /// (unison) to the riff cell in the bass's rests (counter-rhythm) and
    /// favours the busier cells; `spread` picks wider templates, the home
    /// degree and chord stacks. Research: DnB lead recipes, 2026-10-15.
    fn follow(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let density = param(ctx, P_DENSITY);
        let spread = param(ctx, P_SPREAD);
        let cx = param(ctx, P_COMPLEXITY);
        let rng = PhraseRng::new(ctx, param(ctx, P_VARIATION), self.rules.phrase);
        let base = rng::RngKey::new(ctx.seed, ctx.lane, 0);

        // The riff: home degree, rhythm cell, pitch template, length.
        let adventure = 1.0 - HOME_ADVENTURE * spread;
        let home_weights: [f32; HOMES.len()] = std::array::from_fn(|i| HOMES[i].1.powf(adventure));
        let home = HOMES[rng::weighted_pick(base.at(0, S_HOME), &home_weights).unwrap_or(0)].0;
        let cell_weights: [f32; RIFF_CELLS.len()] = std::array::from_fn(|i| lerp(RIFF_CELLS[i].1, RIFF_CELLS[i].2, cx));
        let cell = RIFF_CELLS[rng::weighted_pick(base.at(0, S_RIFF_CELL), &cell_weights).unwrap_or(0)].0;
        let tpl_weights: [f32; RIFF_TEMPLATES.len()] =
            std::array::from_fn(|i| lerp(RIFF_TEMPLATES[i].1, RIFF_TEMPLATES[i].2, spread));
        let template = RIFF_TEMPLATES[rng::weighted_pick(base.at(0, S_RIFF_TEMPLATE), &tpl_weights).unwrap_or(0)].0;
        let two_bars = base.chance(0, S_RIFF_LEN, RIFF_TWO_BAR_PROB);
        // Bar 2 of a 2-bar riff answers: the template from its middle,
        // ending on the 5th.
        let answer = two_bars && ctx.bar_in_phrase % 2 == 1;
        let offset = if answer { template.len() / 2 } else { 0 };
        // The phrase's last bar may vary the riff.
        let variant = (rng.is_variation_bar() && rng.bar.chance(0, S_RIFF_VARIANT, lerp(RIFF_VARIANT_PROB.0, RIFF_VARIANT_PROB.1, cx)))
            .then(|| rng::below(rng.bar.at(1, S_RIFF_VARIANT), 4));
        // High chords voice the riff an octave down, so it stays near the root.
        let revoice = if ctx.harmony.chord_degree >= 4 { -SCALE_LEN } else { 0 };

        // Rhythm: the riff cell joined by the bass's onsets (cx 0) or kept
        // out of them (cx 1); strong steps survive a low density.
        let bass = if ctx.bass_onsets != 0 { ctx.bass_onsets } else { FOLLOW_FALLBACK_ONSETS };
        let rhythm = lerp(FOLLOW_RHYTHM_DENSITY.0, FOLLOW_RHYTHM_DENSITY.1, density);
        let mut onsets = [false; STEPS_PER_BAR];
        for (s, on) in onsets.iter_mut().enumerate() {
            let on_bass = (bass & (1 << s) != 0) as u8 as f32;
            let in_cell = (cell & (1 << s) != 0) as u8 as f32;
            // The cell, plus (low cx) the bass's onsets or (high cx) minus them.
            let importance = (in_cell * (1.0 - RIFF_BASS_AVOID * on_bass * cx)).max(on_bass * RIFF_UNISON * (1.0 - cx))
                * lerp(RIFF_METRIC_FLOOR, 1.0, metric_weight(s));
            *on = importance > 0.0 && reveals(importance, rhythm, 0.0, 0.0);
        }
        // Dense: repeats of the riff's notes fill the gaps, so density adds
        // notes without changing the riff.
        let fill_rhythm = lerp(0.0, RIFF_FILL_DENSITY.1, ((density - RIFF_FILL_DENSITY.0) / (1.0 - RIFF_FILL_DENSITY.0)).max(0.0));
        let mut fills = [false; STEPS_PER_BAR];
        for s in 0..STEPS_PER_BAR {
            if !onsets[s] && fill_rhythm > 0.0 {
                fills[s] = reveals(RIFF_FILL * lerp(RIFF_METRIC_FLOOR, 1.0, metric_weight(s)), fill_rhythm, 0.0, 0.0);
            }
        }
        if variant == Some(0) {
            // Variant: a 16th run over the last beat.
            for on in &mut onsets[12..] {
                *on = true;
            }
        }

        // Pitches: the template over the onsets, passing tones only on odd
        // steps (a chord tone elsewhere), stepping into the next tone.
        let mut plan: Plan = [None; STEPS_PER_BAR];
        let n_onsets = onsets.iter().filter(|&&o| o).count();
        let mut k = 0usize;
        let mut prev = home;
        let mut last_hit: Option<Hit> = None;
        for s in 0..STEPS_PER_BAR {
            if fills[s] {
                // A fill repeats the note before it (none before the first).
                if let Some(mut hit) = last_hit {
                    hit.accent = false;
                    plan[s] = Some(hit);
                }
                continue;
            }
            if !onsets[s] {
                continue;
            }
            let tone = |i: usize| template[(i + offset) % template.len()];
            let mut degree = match tone(k) {
                PASS => {
                    let next = (1..template.len()).map(|j| tone(k + j)).find(|&t| t != PASS).unwrap_or(0);
                    if s % 2 == 1 { next + if next >= prev - home { 1 } else { -1 } } else { next }
                }
                t => t,
            } + home
                + revoice;
            let last = k + 1 == n_onsets;
            match variant {
                Some(1) if k + 2 >= n_onsets => degree += SCALE_LEN,
                Some(2) if last => degree = home + revoice + if rng.bar.chance(2, S_RIFF_VARIANT, 0.5) { 4 } else { 6 },
                Some(3) => degree += 2,
                _ => {}
            }
            if answer && last {
                degree = home + revoice + 4;
            }
            prev = degree;
            // Stack: 1 to 4 notes by the spread, a chord built on the note;
            // seeded per riff note, so it repeats with the riff.
            let most = 1 + (spread * (FOLLOW_STACK_MAX - 1) as f32 + 0.5) as usize;
            let size = 1 + rng::below(base.at(k as u8, S_VOICING), most as u32) as usize;
            let voicing = &voicing_family(ctx)[..size];
            let on_bass = bass & (1 << s) != 0;
            let mut hit = Hit { size: voicing.len() as u8, accent: metric_level(s) <= 1 || (on_bass && metric_level(s) <= 2), ..Hit::default() };
            for (i, &d) in voicing.iter().enumerate() {
                hit.degrees[i] = degree + d;
                hit.gains[i] = 1.0;
            }
            hit.repeated = hit.degrees;
            plan[s] = Some(hit);
            last_hit = Some(hit);
            k += 1;
        }
        self.emit(ctx, &rng, &plan, density, out);
    }

    /// Turn a plan into notes: length and attack from the density, stacked
    /// tones at their gains, no close intervals low down (mud).
    fn emit(&self, ctx: &BarCtx, rng: &PhraseRng, plan: &Plan, density: f32, out: &mut BarEvents) {
        let octave = root_octave(ctx.harmony.key, self.root_min()) + register_shift(param(ctx, P_REGISTER));
        let (length, max_gate_steps) = if self.kind == LeadKind::Pad {
            (lerp(PAD_LENGTH_BY_DENSITY.0, PAD_LENGTH_BY_DENSITY.1, density), STEPS_PER_BAR)
        } else {
            (lerp(LENGTH_BY_DENSITY.0, LENGTH_BY_DENSITY.1, density), lerp(GATE_MAX_STEPS.0, GATE_MAX_STEPS.1, density).round() as usize)
        };
        let attack = density_attack(density);
        for s in 0..STEPS_PER_BAR {
            let Some(hit) = plan[s] else { continue };
            let next = (s + 1..STEPS_PER_BAR).find(|&t| plan[t].is_some()).unwrap_or(STEPS_PER_BAR);
            let span = ((next - s).min(max_gate_steps) as u16 * TICKS_PER_STEP) as f32;
            let gate_ticks = (span * length).max(GATE_MIN_TICKS).round() as u16;
            let tier = if hit.accent { Tier::Accent } else { Tier::Normal };
            let vel = velocity(tier, rng.unit(s as u8, S_VEL), self.rules.tiers);
            let mut keys = [0u8; MAX_STACK];
            for i in 0..hit.size as usize {
                keys[i] = ctx.harmony.resolve(hit.degrees[i], octave);
            }
            for i in 0..hit.size as usize {
                let key = keys[i];
                // Low down, only 5ths and wider sit clean over a lower tone.
                let muddy = keys[..hit.size as usize]
                    .iter()
                    .any(|&k| k < key && k < MUD_BELOW_KEY && key - k < MUD_MIN_INTERVAL);
                if (i > 0 && muddy) || keys[..i].contains(&key) {
                    continue;
                }
                let gain = STACK_VEL_FLOOR + (1.0 - STACK_VEL_FLOOR) * hit.gains[i];
                let id = (s * MAX_STACK + i) as u16 + 1;
                let mut ev = note(step_tick(s), key, ((vel as f32 * gain).round() as u8).max(1), gate_ticks, id);
                ev.attack = attack;
                ev.start = stream_start(ctx, s as u8, i as u8);
                out.push(ev);
            }
        }
    }

    /// Melody mode: the bar's cell from the seed's palette, as events.
    fn cells(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let density = param(ctx, P_DENSITY);
        let complexity = param(ctx, P_COMPLEXITY);
        let spread = param(ctx, P_SPREAD);
        // The palette and its order are the seed's, the same in every bar.
        let base = rng::RngKey::new(ctx.seed, ctx.lane, 0);
        let order = CELL_ORDERS[rng::below(base.at(0, S_ORDER), CELL_ORDERS.len() as u32) as usize];
        let cell = order[ctx.bar_in_phrase as usize & 3];
        let ch = |n: u8, stream: u32| base.at(cell as u8 * 16 + n, stream);

        let range = lerp(MELODY_RANGE.0, MELODY_RANGE.1, spread);
        let step = lerp(MELODY_STEP.0, MELODY_STEP.1, spread).round().max(1.0) as i32;
        let leap = lerp(MELODY_LEAP.0, MELODY_LEAP.1, spread).round() as i32;
        let (lo, hi) = (-(range / 3.0).round() as i32, (range * 2.0 / 3.0).round() as i32);
        let fig_weights: [f32; FIGURES.len()] = std::array::from_fn(|i| lerp(FIGURES[i].weight.0, FIGURES[i].weight.1, complexity));
        let contour_weights: [f32; CONTOURS.len()] = std::array::from_fn(|i| CONTOURS[i].1);
        let art_weights: [f32; ARTICULATIONS.len()] = std::array::from_fn(|i| ARTICULATIONS[i].1);
        let count = 1 + FIGURES_AT.iter().filter(|&&d| density > d).count();

        // Figures of the cell, placed on free beats in a seeded order.
        let mut figs = [CellFig::default(); MAX_FIGURES];
        let mut starts = [0u16; MAX_FIGURES];
        let mut placed = 0usize;
        let mut taken: [(u16, u16); MAX_FIGURES] = [(0, 0); MAX_FIGURES];
        let first_start = rng::below(ch(9, S_PLACE), 4) as usize;
        for k in 0..FIGURE_STARTS.len() {
            if placed == count {
                break;
            }
            // Step 5 is coprime with 12: every start is tried once.
            let start = FIGURE_STARTS[(first_start + k * 5) % FIGURE_STARTS.len()];
            let n = placed as u8;
            let figure = rng::weighted_pick(ch(n, S_FIG), &fig_weights).unwrap_or(1);
            let f = &FIGURES[figure];
            let end = start + f.onsets[f.onsets.len() - 1] + f.last;
            if end > TICKS_PER_BAR || taken[..placed].iter().any(|&(a, b)| start < b + FIGURE_GAP && a < end + FIGURE_GAP) {
                continue;
            }
            // Anchor on a chord tone inside the range; repeats lean high.
            let contour = rng::weighted_pick(ch(n + 4, S_FIG), &contour_weights).unwrap_or(0);
            let tones: [i32; 6] = [0, 2, 4, 7, 9, 11];
            let usable: usize = tones.iter().filter(|&&t| t >= lo && t <= hi).count().max(1);
            let mut ti = rng::below(ch(n + 8, S_FIG), usable as u32) as usize;
            if CONTOURS[contour].0 == Contour::Repeat {
                ti = ti.max(usable / 2);
            }
            let anchor = tones.iter().copied().filter(|&t| t >= lo && t <= hi).nth(ti).unwrap_or(0);
            figs[placed] = CellFig {
                figure,
                contour,
                articulation: rng::weighted_pick(ch(n + 12, S_FIG), &art_weights).unwrap_or(1),
                anchor,
            };
            starts[placed] = start;
            taken[placed] = (start, end);
            placed += 1;
        }

        // Nothing fitted (long figures late in the bar): a run of 8ths on the 1.
        if placed == 0 {
            figs[0] = CellFig { figure: 1, contour: 1, articulation: 1, anchor: 0 };
            starts[0] = 0;
            placed = 1;
        }
        let octave = root_octave(ctx.harmony.key, self.root_min()) + register_shift(param(ctx, P_REGISTER));
        // The density shortens or lengthens every articulation (sparse = long).
        let length = lerp(1.3, 0.8, density);
        let attack = density_attack(density);
        let mut id = 1u16;
        let mut events = [crate::live::event::LiveEvent::default(); MAX_FIGURES * 4];
        let mut n_ev = 0usize;
        for k in 0..placed {
            let fig = figs[k];
            let f = &FIGURES[fig.figure];
            let contour = CONTOURS[fig.contour].0;
            let notes = f.onsets.len();
            for (i, &o) in f.onsets.iter().enumerate() {
                let i32i = i as i32;
                let degree = match contour {
                    Contour::Repeat => fig.anchor,
                    Contour::Down => fig.anchor - step * i32i,
                    Contour::Up => fig.anchor + step * i32i,
                    Contour::LeapBack => {
                        if i == 0 || i + 1 == notes {
                            fig.anchor
                        } else {
                            fig.anchor + leap
                        }
                    }
                    Contour::Zigzag => fig.anchor + if i % 2 == 1 { step } else { 0 } - i32i / 2,
                };
                let degree = degree.clamp(lo - SCALE_LEN / 2, hi + SCALE_LEN / 2);
                let tick = starts[k] + o;
                let span = if i + 1 < notes { f.onsets[i + 1] - o } else { f.last };
                let frac = (ARTICULATIONS[fig.articulation].0 * length).min(1.0);
                let gate_ticks = ((span as f32 * frac).round() as u16).max(GATE_MIN_TICKS as u16).min(TICKS_PER_BAR - tick);
                let tier = if i == 0 { Tier::Accent } else { Tier::Normal };
                let vel = velocity(tier, rng::unit_f32(ch(n_ev as u8, S_VEL)), self.rules.tiers);
                let key = ctx.harmony.resolve(degree, octave);
                if n_ev < events.len() {
                    let mut ev = note(tick, key, vel, gate_ticks, id);
                    ev.attack = attack;
                    ev.start = stream_start(ctx, (tick / TICKS_PER_STEP) as u8, 0);
                    events[n_ev] = ev;
                    n_ev += 1;
                    id += 1;
                }
            }
        }
        // Placement order is not tick order.
        let evs = &mut events[..n_ev];
        evs.sort_unstable_by_key(|e| e.tick);
        for e in evs.iter() {
            out.push(*e);
        }
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
        if mode == LeadMode::Motif && self.uses_motifs() {
            self.motif_bar(ctx, out);
            return;
        }
        if mode == LeadMode::Motif && self.rules.follow && self.kind == LeadKind::Lead {
            self.follow(ctx, out);
            return;
        }
        if mode == LeadMode::Motif && self.rules.melody && self.kind == LeadKind::Lead {
            self.cells(ctx, out);
            return;
        }
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
        self.emit(ctx, &rng, &plan, density, out);
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
        // Stream positions vary on every note by design; the musical
        // content is everything else.
        render(g, &c).as_slice().iter().map(|e| LiveEvent { start: 0, ..*e }).collect()
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
                assert!(!am.is_empty());
                // Low tones may drop out of a stack (mud), so compare each
                // hit's first note.
                let firsts = |b: &BarEvents| -> Vec<(u16, u16, u8, u16, u8)> {
                    let mut v: Vec<_> = b.as_slice().iter().map(|e| (e.tick, e.gate, e.vel, e.note_id, e.key)).collect();
                    v.dedup_by_key(|e| e.0);
                    v
                };
                for (a, e) in firsts(&am).iter().zip(&firsts(&em)) {
                    assert_eq!((a.0, a.1, a.2, a.3), (e.0, e.1, e.2, e.3));
                    assert_eq!((e.4 as i32 - a.4 as i32).rem_euclid(12), 7);
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
            for (stack, size) in [(0.19, 2), (0.39, 3), (0.59, 4)] {
                let p = params(LeadKind::Stab, |p| {
                    p[P_STACK] = stack;
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
        // Sparse: held to the bar end with a slow attack.
        let p = params(LeadKind::Pad, |p| {
            p[P_VARIATION] = 0.0;
            p[P_DENSITY] = 0.0001;
        });
        let ev = bar(&g, 1, 0, &p, &EMPTY);
        assert_eq!(steps(&ev), [0]);
        assert!(ev.iter().all(|e| e.gate >= crate::live::TICKS_PER_BAR - 1));
        assert!(ev.iter().all(|e| crate::live::sampler::attack_secs(e.attack) > 0.07));
        // Dense stabs are short with a sharp attack.
        let stab = LeadGenerator::preset(LeadKind::Stab);
        let short = bar(&stab, 1, 0, &params(LeadKind::Stab, |p| p[P_DENSITY] = 1.0), &EMPTY);
        assert!(!short.is_empty());
        assert!(short.iter().all(|e| e.gate < 2 * TICKS_PER_STEP), "{short:?}");
        assert!(short.iter().all(|e| crate::live::sampler::attack_secs(e.attack) < 0.012));
    }

    #[test]
    fn register_shifts_by_octaves() {
        for kind in KINDS {
            let g = LeadGenerator::preset(kind);
            let at = |r: f32| bar(&g, 5, 0, &params(kind, |p| p[P_REGISTER] = r), &EMPTY);
            let (low, mid, high) = (at(0.0), at(0.5), at(1.0));
            assert!(!mid.is_empty());
            // The first (lowest) note of each hit moves by octaves; upper
            // stack tones may drop out low down (mud).
            let first = |b: &[LiveEvent]| -> Vec<(u16, u8)> {
                let mut v: Vec<_> = b.iter().map(|e| (e.tick, e.key)).collect();
                v.dedup_by_key(|e| e.0);
                v
            };
            for ((l, m), h) in first(&low).iter().zip(&first(&mid)).zip(&first(&high)) {
                assert_eq!((m.1 - l.1, h.1 - m.1), (12, 12));
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

    fn follower() -> LeadGenerator {
        let mut defaults = [0.0; MAX_GEN_PARAMS];
        defaults[..PARAM_NAMES.len()].copy_from_slice(&LeadKind::Lead.defaults());
        LeadGenerator::new(LeadKind::Lead, defaults, LeadRules { follow: true, ..LeadRules::default() }, Vec::new())
    }

    /// Bass onsets the follow tests play against (two-step bass).
    const BASS: u16 = 1 | 1 << 3 | 1 << 6 | 1 << 10;

    fn follow_bar(seed: u32, bar: u8, edit: impl FnOnce(&mut [f32; MAX_GEN_PARAMS])) -> Vec<LiveEvent> {
        let g = follower();
        let p = params(LeadKind::Lead, |p| {
            p[P_VARIATION] = 0.0;
            edit(p);
        });
        let mut c = ctx(seed, bar, &p, &EMPTY);
        c.lane = 2;
        c.bass_onsets = BASS;
        let out = render(&g, &c);
        assert_well_formed(&out);
        out.as_slice().iter().map(|e| LiveEvent { start: 0, ..*e }).collect()
    }

    #[test]
    fn follow_plays_with_or_against_the_bass() {
        let on_bass_share = |cx: f32| {
            let (mut on, mut all) = (0, 0);
            for seed in 0..60 {
                for s in steps(&follow_bar(seed, 0, |p| {
                    p[P_COMPLEXITY] = cx;
                    p[P_SPREAD] = 0.0;
                })) {
                    all += 1;
                    on += (BASS & (1 << s) != 0) as u32;
                }
            }
            on as f32 / all as f32
        };
        // cx 0: the riff joins the bass on (nearly) all its onsets.
        let covered = (0..60u32)
            .map(|seed| {
                let s = steps(&follow_bar(seed, 0, |p| {
                    p[P_COMPLEXITY] = 0.0;
                    p[P_SPREAD] = 0.0;
                }));
                (0..16).filter(|&b| BASS & (1 << b) != 0 && s.contains(&b)).count()
            })
            .sum::<usize>() as f32
            / (60 * BASS.count_ones()) as f32;
        assert!(covered > 0.85, "cx 0 plays with the bass: {covered}");
        let (locked, free) = (on_bass_share(0.0), on_bass_share(1.0));
        assert!(free < 0.35 && free < locked, "cx 1 plays in its rests: {free} (cx 0: {locked})");
    }

    #[test]
    fn follow_riffs_repeat_and_stay_on_chord_tones() {
        for seed in 0..60 {
            let b0 = follow_bar(seed, 0, |_| {});
            assert!(!b0.is_empty());
            // The riff repeats exactly (two-bar riffs: bars 0 and 2).
            assert_eq!(b0, follow_bar(seed, 2, |_| {}), "seed {seed}");
        }
        // Downbeats sit mostly on chord tones of the bar's chord (A C E G B;
        // a seed's home may be the 4th or 6th).
        let (mut on, mut all) = (0, 0);
        for seed in 0..100 {
            for e in follow_bar(seed, 0, |_| {}).iter().filter(|e| e.tick % 96 == 0) {
                all += 1;
                on += [9, 0, 4, 7, 11].contains(&(e.key % 12)) as u32;
            }
        }
        assert!(on as f32 / all as f32 > 0.75, "{on} of {all} downbeats on chord tones");
        // Lines move mostly by small steps, not random leaps.
        let (mut jumps, mut n) = (0i32, 0i32);
        for seed in 0..100 {
            let mut firsts: Vec<(u16, u8)> = follow_bar(seed, 0, |p| p[P_SPREAD] = 0.0).iter().map(|e| (e.tick, e.key)).collect();
            firsts.dedup_by_key(|e| e.0);
            for w in firsts.windows(2) {
                jumps += (w[1].1 as i32 - w[0].1 as i32).abs();
                n += 1;
            }
        }
        assert!((jumps as f32 / n as f32) < 6.0, "mean jump {}", jumps as f32 / n as f32);
    }

    #[test]
    fn follow_density_and_stacks() {
        let per_bar = |d: f32| {
            (0..60).map(|seed| steps(&follow_bar(seed, 0, |p| p[P_DENSITY] = d)).len()).sum::<usize>() as f32 / 60.0
        };
        let (low, high) = (per_bar(0.02), per_bar(1.0));
        assert!(low >= 3.0 && high > low + 2.0, "hits per bar {low} .. {high}");
        // Spread 0: single notes. Spread 1: chords of up to four.
        let widest = |spread: f32| {
            (0..40)
                .flat_map(|seed| {
                    let ev = follow_bar(seed, 0, |p| p[P_SPREAD] = spread);
                    let mut counts = std::collections::BTreeMap::<u16, usize>::new();
                    for e in &ev {
                        *counts.entry(e.tick).or_default() += 1;
                    }
                    counts.into_values()
                })
                .max()
                .unwrap_or(0)
        };
        assert_eq!(widest(0.0), 1);
        assert!(widest(1.0) >= 3);
    }

    #[test]
    fn follow_homes_open_up_with_spread() {
        // A minor, chord on the root: the home on the 1 is A (root) or, with
        // spread, also G (b7) and others.
        let homes = |spread: f32| -> Vec<u8> {
            (0..80)
                .filter_map(|seed| {
                    follow_bar(seed, 0, |p| {
                        p[P_SPREAD] = spread;
                        p[P_DENSITY] = 1.0;
                    })
                    .iter()
                    .find(|e| e.tick == 0)
                    .map(|e| e.key % 12)
                })
                .collect()
        };
        let narrow = homes(0.0);
        let wide = homes(1.0);
        let share = |v: &[u8], pc: u8| v.iter().filter(|&&k| k == pc).count() as f32 / v.len() as f32;
        assert!(share(&narrow, 9) > share(&wide, 9), "spread moves the home off the root");
        assert!(wide.contains(&7), "b7 (G) is a home at full spread: {wide:?}");
    }

    #[test]
    fn stacks_build_chords_not_octaves() {
        let g = LeadGenerator::preset(LeadKind::Pad);
        let mut families = std::collections::BTreeSet::new();
        for seed in 0..40 {
            let ev = bar(&g, seed, 0, &params(LeadKind::Pad, |p| {
                p[P_STACK] = 1.0;
                p[P_SPREAD] = 0.0;
                p[P_VARIATION] = 0.0;
            }), &EMPTY);
            let chord: Vec<u8> = ev.iter().filter(|e| e.tick == ev[0].tick).map(|e| e.key).collect();
            let classes: std::collections::BTreeSet<u8> = chord.iter().map(|k| k % 12).collect();
            assert!(classes.len() >= 4, "seed {seed}: {chord:?} doubles notes");
            families.insert(classes.into_iter().collect::<Vec<_>>());
        }
        assert!(families.len() >= 3, "seeds pick different chord families: {families:?}");
    }

    #[test]
    fn stack_fades_tones_in() {
        let g = LeadGenerator::preset(LeadKind::Pad);
        let at = |stack: f32| {
            bar(&g, 3, 0, &params(LeadKind::Pad, |p| {
                p[P_STACK] = stack;
                p[P_VARIATION] = 0.0;
                p[P_SPREAD] = 0.0;
            }), &EMPTY)
        };
        let first_tick = |ev: &[LiveEvent]| ev.iter().filter(|e| e.tick == ev[0].tick).count();
        assert_eq!(first_tick(&at(0.0)), 1);
        assert_eq!(first_tick(&at(1.0)), MAX_STACK);
        // A tone just past its threshold is quiet, then grows.
        let vel_of_last = |stack: f32| *at(stack).iter().filter(|e| e.tick == 0).map(|e| e.vel).collect::<Vec<_>>().last().unwrap();
        assert!(vel_of_last(0.22) < vel_of_last(0.38), "{} {}", vel_of_last(0.22), vel_of_last(0.38));
    }

    // ── Motif clips ──

    fn motif_lead(names: &[&str]) -> LeadGenerator {
        let motifs = crate::live::generator::test_util::starter_motifs().into_iter().filter(|m| names.contains(&m.name.as_str())).collect();
        LeadGenerator::new(LeadKind::Lead, LeadGenerator::preset(LeadKind::Lead).param_defaults(), LeadRules::default(), Vec::new()).with_motifs(motifs)
    }

    fn lead_bar(g: &LeadGenerator, bar: u8, p: &[f32; MAX_GEN_PARAMS], edit: impl FnOnce(&mut BarCtx)) -> Vec<LiveEvent> {
        let mut c = ctx(5, bar, p, &EMPTY);
        c.lane = 2;
        edit(&mut c);
        render(g, &c).as_slice().iter().map(|e| LiveEvent { start: 0, ..*e }).collect()
    }

    #[test]
    fn motif_lead_plays_call_and_response_and_rests_on_fills() {
        let g = motif_lead(&["lead-static-hook"]);
        let low_cx = params(LeadKind::Lead, |p| {
            p[P_COMPLEXITY] = 0.1;
            p[P_DENSITY] = 0.5;
            p[P_MODE] = 0.0;
            p[P_VARIATION] = 0.0;
        });
        let ticks = |ev: &[LiveEvent]| ev.iter().map(|e| e.tick).collect::<Vec<_>>();
        let call = lead_bar(&g, 0, &low_cx, |_| {});
        let answer = lead_bar(&g, 1, &low_cx, |_| {});
        assert!(!call.is_empty() && call.iter().all(|e| e.tick < 192), "{:?}", ticks(&call));
        assert!(!answer.is_empty() && answer.iter().all(|e| e.tick >= 192), "{:?}", ticks(&answer));
        assert_eq!(call, lead_bar(&g, 2, &low_cx, |_| {}), "the call returns in bar 3");
        assert_eq!(answer, lead_bar(&g, 3, &low_cx, |_| {}), "the answer returns in bar 4");
        // High complexity: call and answer share the bar.
        let high_cx = params(LeadKind::Lead, |p| p[P_COMPLEXITY] = 0.9);
        let whole = lead_bar(&g, 0, &high_cx, |_| {});
        assert!(whole.iter().any(|e| e.tick < 192) && whole.iter().any(|e| e.tick >= 192));
        // Rests on a drum fill, in the fill variant and on a filler's last bar.
        assert!(lead_bar(&g, 0, &low_cx, |c| c.fill = true).is_empty());
        assert!(lead_bar(&g, 0, &high_cx, |c| c.drum_fill = true).is_empty());
    }

    #[test]
    fn motif_lead_leaves_the_basss_steps_at_high_complexity() {
        let g = motif_lead(&["lead-octave-call"]);
        let p = params(LeadKind::Lead, |p| p[P_COMPLEXITY] = 1.0);
        let free = lead_bar(&g, 0, &p, |_| {});
        let bass = lead_bar(&g, 0, &p, |c| c.bass_onsets = 0xFFFF);
        assert!(!bass.is_empty() && bass.len() < free.len(), "{} vs {}", bass.len(), free.len());
        // At low complexity it plays with the bass.
        let unison = params(LeadKind::Lead, |p| p[P_COMPLEXITY] = 0.1);
        assert_eq!(lead_bar(&g, 0, &unison, |c| c.bass_onsets = 0xFFFF).len(), lead_bar(&g, 0, &unison, |_| {}).len());
    }

    #[test]
    fn motif_lead_picks_clips_that_interlock_with_the_bass() {
        let all = crate::live::generator::test_util::starter_motifs();
        let names: Vec<&str> = all.iter().filter(|m| m.name.starts_with("lead-")).map(|m| m.name.as_str()).collect();
        let g = motif_lead(&names);
        let p = params(LeadKind::Lead, |p| p[P_COMPLEXITY] = 0.9);
        // Bass on every 8th: a lead on the off 16ths interlocks, one on the 8ths does not.
        let bass = [0x5555u16; 4];
        let coincident = |bass_phrase: [u16; 4]| -> u32 {
            (0..60u32)
                .map(|seed| {
                    let mut c = ctx(seed, 0, &p, &EMPTY);
                    c.lane = 2;
                    c.bass_phrase = bass_phrase;
                    let ev = render(&g, &c);
                    ev.as_slice().iter().filter(|e| (e.tick / 24) % 2 == 0).count() as u32
                })
                .sum()
        };
        assert!(coincident(bass) < coincident([0; 4]), "clips avoid the bass's steps when it is known");
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
        "3:40 6:41",
        "0:57 2:60 4:64 6:69 8:72 12:60 14:64",
        "2:69 2:62 2:67 6:69 6:62 6:67 14:69 14:62 14:67",
        "0:69 0:62 0:67",
    ];

    #[test]
    fn modes_from_param() {
        assert_eq!(LeadMode::from_param(0.0), LeadMode::Motif);
        assert_eq!(LeadMode::from_param(0.5), LeadMode::Arp);
        assert_eq!(LeadMode::from_param(1.0), LeadMode::Stab);
        for kind in KINDS {
            let g = LeadGenerator::preset(kind);
            assert_eq!(g.param_names(), PARAM_NAMES);
            assert_eq!(g.param_defaults()[..PARAM_NAMES.len()], kind.defaults());
        }
    }

    fn melody() -> LeadGenerator {
        let mut defaults = [0.0; MAX_GEN_PARAMS];
        defaults[..PARAM_NAMES.len()].copy_from_slice(&LeadKind::Lead.defaults());
        LeadGenerator::new(LeadKind::Lead, defaults, LeadRules { melody: true, ..LeadRules::default() }, Vec::new())
    }

    #[test]
    fn melody_cells_are_well_formed_and_deterministic() {
        let g = melody();
        for seed in 0..100 {
            for b in 0..4 {
                let p = params(LeadKind::Lead, |p| p[P_DENSITY] = 0.9);
                let c = ctx(seed, b, &p, &TWO_STEP);
                let out = render(&g, &c);
                assert_well_formed(&out);
                assert_eq!(out.as_slice(), render(&g, &c).as_slice());
                let scale = scale_for(MusicalKey::new(9, true));
                for e in out.as_slice() {
                    assert!(scale.contains(&((e.key as i32 - 9).rem_euclid(12) as u8)), "out of key");
                }
            }
        }
    }

    #[test]
    fn melody_spread_sets_the_range() {
        let g = melody();
        let widest = |spread: f32| {
            (0..100)
                .map(|seed| {
                    let p = params(LeadKind::Lead, |p| {
                        p[P_SPREAD] = spread;
                        p[P_DENSITY] = 1.0;
                    });
                    let keys: Vec<i32> = (0..4).flat_map(|b| bar(&g, seed, b, &p, &EMPTY)).map(|e| e.key as i32).collect();
                    keys.iter().max().unwrap_or(&0) - keys.iter().min().unwrap_or(&0)
                })
                .max()
                .unwrap()
        };
        assert!(widest(0.0) <= 14, "{}", widest(0.0));
        assert!(widest(1.0) >= 20, "about two octaves at full spread: {}", widest(1.0));
    }

    #[test]
    fn melody_seeds_vary_rhythm_length_and_triplets() {
        let g = melody();
        let p = params(LeadKind::Lead, |p| {
            p[P_DENSITY] = 0.9;
            p[P_COMPLEXITY] = 0.5;
        });
        let mut rhythms = std::collections::HashSet::new();
        let (mut triplets, mut short, mut long) = (0, 0, 0);
        for seed in 0..60 {
            let ev = bar(&g, seed, 0, &p, &EMPTY);
            rhythms.insert(ev.iter().map(|e| e.tick).collect::<Vec<_>>());
            triplets += ev.iter().any(|e| e.tick % 32 == 0 && e.tick % 24 != 0) as usize;
            short += ev.iter().any(|e| e.gate <= 20) as usize;
            long += ev.iter().any(|e| e.gate >= 96) as usize;
        }
        assert!(rhythms.len() > 20, "{}", rhythms.len());
        assert!(triplets >= 3 && short >= 3 && long >= 3, "triplets {triplets} short {short} long {long}");
    }

    #[test]
    fn melody_bars_follow_a_call_and_response_order() {
        let g = melody();
        let p = params(LeadKind::Lead, |p| p[P_DENSITY] = 0.7);
        let mut repeats = 0;
        for seed in 0..50 {
            let bars: Vec<Vec<u16>> = (0..4).map(|b| bar(&g, seed, b, &p, &EMPTY).iter().map(|e| e.tick).collect()).collect();
            // Every order repeats a cell.
            repeats += (0..4).any(|i| (i + 1..4).any(|j| bars[i] == bars[j])) as usize;
            assert!(bars.iter().all(|b| !b.is_empty()), "seed {seed}: every bar plays");
            assert!(bars.iter().any(|b| b != &bars[0]), "seed {seed}: a phrase has more than one cell");
        }
        assert_eq!(repeats, 50);
    }

    #[test]
    fn melody_does_not_allocate() {
        let g = melody();
        let p = params(LeadKind::Lead, |p| p[P_DENSITY] = 1.0);
        let (_, allocs) = crate::live::test_support::count_allocs(|| {
            for seed in 0..20 {
                let _ = render(&g, &ctx(seed, 1, &p, &EMPTY));
            }
        });
        assert_eq!(allocs, 0);
    }
}
