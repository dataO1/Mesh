//! The starter motif set: ten bass and ten lead clips written as compact
//! tables, which the starter-style builder writes out as `.mid` files.
//!
//! Each bar is a rhythm of 16 steps and the pitches of its onsets:
//!
//! - rhythm: `x` stab (half of its span, at least a step), `X` held for its
//!   whole span, `-` continues the note before it, `.` rest. A note's span
//!   runs to the next `x`, `X` or `.`.
//! - pitches: scale degree names over the clip's root with an optional flat
//!   or sharp (`1 b3 5 b7 8 9 b10`), one per onset in the bar. A leading `,`
//!   lowers the note an octave, `'` raises it.
//!
//! The rhythm cells and pitch templates follow common drum and bass recipes
//! (3-3-2, 3-3-3-3-4, offbeat 8ths, stab and hold, syncopated stabs, a long
//! note with a pickup, a call in the first half), over a root-heavy pitch
//! pool for the bass and question and answer phrases with rests for the lead.
//! Chord tones sit on the beats and the even 16ths, passing tones only on the
//! odd 16ths.

use super::{Clip, RawNote, Voice};
use crate::live::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// One bar: the 16-step rhythm and the pitches of its onsets.
pub type Bar = (&'static str, &'static str);

/// A starter clip.
#[derive(Debug, Clone, Copy)]
pub struct StarterClip {
    pub name: &'static str,
    /// Root pitch class (C = 0).
    pub root: u8,
    pub minor: bool,
    /// 1, 2 or 4 bars.
    pub bars: &'static [Bar],
}

const A: u8 = 9;
const C: u8 = 0;
const D: u8 = 2;
const E: u8 = 4;
const F: u8 = 5;

const fn minor(name: &'static str, root: u8, bars: &'static [Bar]) -> StarterClip {
    StarterClip { name, root, minor: true, bars }
}

/// Bass clips (root-heavy, with clear held notes and stabs).
pub const BASS: [StarterClip; 10] = [
    // 3-3-2 twice, root and octave bounce.
    minor("bass-332-bounce", A, &[("x--x--x-x--x--x-", "1 1 8 1 1 8")]),
    // 3-3-3-3-4, a pedal and then a drop to the 7th and 5th below.
    minor("bass-3334-pedal", D, &[("X--X--X--X--X---", "1 1 1 1 1"), ("X--X--X--X--X---", "1 1 1 ,b7 ,5")]),
    // Offbeat 8ths.
    minor("bass-offbeat-8ths", E, &[("--x---x---x---x-", "1 1 8 5")]),
    // Stab and hold.
    minor("bass-stab-hold", F, &[("x-----x---X-----", "1 8 ,5")]),
    // Syncopated stabs, the second bar answers.
    minor("bass-syncopated", A, &[("x--x-x---x-x--x-", "1 1 1 8 1 5"), ("x--x-x---x-x--x-", "1 1 1 8 b7 5")]),
    // A long note and a pickup.
    minor("bass-long-pickup", D, &[("X-----------x-x-", "1 1 5"), ("X-----------x-x-", "1 5 b7")]),
    // A call in the first half, then the full answer.
    minor("bass-call-answer", E, &[("x--x--x---------", "1 1 5"), ("x--x--x-x--x----", "1 1 b7 1 8")]),
    // Straight 8ths over a triad.
    minor("bass-eighths-triad", F, &[("x-x-x-x-x-x-x-x-", "1 1 5 1 b3 1 5 8")]),
    // Static quarters.
    minor("bass-static-quarters", A, &[("X---X---X---X---", "1 1 1 1"), ("X---X---X---X---", "1 1 b7 5")]),
    // Four bars of 3-3-2 with a moving fourth bar.
    minor(
        "bass-roller-walk",
        D,
        &[
            ("x--x--x-x--x--x-", "1 1 8 1 1 8"),
            ("x--x--x-x--x--x-", "1 1 8 1 5 8"),
            ("x--x--x-x--x--x-", "1 1 8 1 1 8"),
            ("x--x--x-x--x--x-", "1 1 8 b7 5 ,b7"),
        ],
    ),
];

/// Lead clips (question and answer phrases with rests).
pub const LEAD: [StarterClip; 10] = [
    // Call on 3-3, answer ends on the root.
    minor("lead-332-call", A, &[("x--x--x---------", "5 5 8"), ("x--x--x-X-------", "8 b7 5 1")]),
    // Triad up, triad down.
    minor("lead-triad-up", D, &[("x---x---x---x---", "1 b3 5 8"), ("X---x---X---....", "5 b3 1")]),
    // Upper neighbour on the 5th, then home.
    minor("lead-upper-neighbour", E, &[("x-x--x--X---x---", "5 5 b6 1 b3"), ("x-x--x--X---....", "5 b3 b6 1")]),
    // Hold and fall.
    minor("lead-hold-fall", F, &[("X-------.x-x-...", "5 4 b3"), ("x---X-------....", "b3 1")]),
    // Octave call.
    minor("lead-octave-call", A, &[("x-x-x-x---......", "1 1 8 5"), ("x-x-x-x-x---X---", "1 1 8 b7 5 1")]),
    // Syncopated hook.
    minor("lead-syncopated-hook", D, &[("x--x-x---x-x--x-", "5 5 b7 8 b7 5"), ("x--x-x--X-------", "8 b7 5 1")]),
    // A long note and a pickup into a falling arpeggio.
    minor("lead-long-pickup", E, &[("X-------....x-x-", "5 b7 8"), ("X-------x-x-X-..", "8 b7 5 b3")]),
    // Pedal on the root with a moving line.
    minor("lead-pedal-line", F, &[("x-x-x-x-x-x-x-..", "1 5 1 b7 1 8 1")]),
    // Static rhythm.
    minor("lead-static-hook", E, &[("x--x--x---x-----", "8 8 8 5")]),
    // Major: root-octave bounce, then the triad up.
    StarterClip {
        name: "lead-major-bounce",
        root: C,
        minor: false,
        bars: &[("x--x--x-x-------", "1 8 1 8"), ("x---x---X-......", "3 5 8")],
    },
];

/// Accent and beat velocities.
const VEL_DOWNBEAT: u8 = 118;
const VEL_BEAT: u8 = 108;
const VEL_OFF: u8 = 92;

/// Semitones of a pitch name such as `b3`, `8` or `,b7`.
fn semitones(token: &str) -> i32 {
    let mut octave = 0;
    let mut rest = token;
    while let Some(c) = rest.chars().next() {
        match c {
            ',' => octave -= 1,
            '\'' => octave += 1,
            _ => break,
        }
        rest = &rest[1..];
    }
    let (accidental, digits) = match rest.as_bytes().first() {
        Some(b'b') => (-1, &rest[1..]),
        Some(b'#') => (1, &rest[1..]),
        _ => (0, rest),
    };
    let n: i32 = digits.parse().unwrap_or_else(|_| panic!("bad pitch name '{token}'"));
    assert!((1..=14).contains(&n), "pitch name '{token}' out of range");
    const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    MAJOR[(n as usize - 1) % 7] + 12 * ((n - 1) / 7) + accidental + 12 * octave
}

impl StarterClip {
    /// The clip as notes. A bass clip's root sits at the lowest MIDI key
    /// from E1 up, a lead's from A3 up.
    pub fn clip(&self, voice: Voice) -> Clip {
        let floor = match voice {
            Voice::Lowest => 28,
            Voice::Highest => 57,
        };
        let tonic = floor + (self.root as i32 - floor).rem_euclid(12);
        let mut notes = Vec::new();
        for (bar, (rhythm, pitches)) in self.bars.iter().enumerate() {
            let cells = rhythm.as_bytes();
            assert_eq!(cells.len(), STEPS_PER_BAR, "{}: bar {bar} needs 16 steps", self.name);
            let mut names = pitches.split_whitespace();
            for (s, &c) in cells.iter().enumerate() {
                if c != b'x' && c != b'X' {
                    continue;
                }
                let span = 1 + cells[s + 1..].iter().take_while(|&&n| n == b'-').count();
                let steps = if c == b'X' { span } else { span.div_ceil(2) };
                let name = names.next().unwrap_or_else(|| panic!("{}: bar {bar} has too few pitches", self.name));
                notes.push(RawNote {
                    start: (bar * TICKS_PER_BAR as usize + s * TICKS_PER_STEP as usize) as u32,
                    len: (steps * TICKS_PER_STEP as usize) as u32,
                    pitch: (tonic + semitones(name)).clamp(0, 127) as u8,
                    vel: match s {
                        0 => VEL_DOWNBEAT,
                        _ if s % 4 == 0 => VEL_BEAT,
                        _ => VEL_OFF,
                    },
                });
            }
            assert!(names.next().is_none(), "{}: bar {bar} has too many pitches", self.name);
        }
        Clip { bars: self.bars.len() as u8, notes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::motif::{key, midi, Motif};

    fn all() -> impl Iterator<Item = (&'static StarterClip, Voice)> {
        BASS.iter().map(|c| (c, Voice::Lowest)).chain(LEAD.iter().map(|c| (c, Voice::Highest)))
    }

    #[test]
    fn names_and_lengths_are_valid() {
        assert_eq!((BASS.len(), LEAD.len()), (10, 10));
        let mut names: Vec<&str> = all().map(|(c, _)| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 20, "names are unique (they become file names)");
        for (c, voice) in all() {
            assert!(matches!(c.bars.len(), 1 | 2 | 4), "{}", c.name);
            // Builds without panicking: 16 steps and matching pitch counts.
            let clip = c.clip(voice);
            assert_eq!(clip.bars as usize, c.bars.len());
            assert!(!clip.notes.is_empty());
        }
    }

    #[test]
    fn written_files_read_back_to_the_same_clip_in_the_same_key() {
        let keys: std::collections::BTreeSet<(u8, bool)> = all().map(|(c, _)| (c.root, c.minor)).collect();
        assert!(keys.len() >= 5, "varied keys: {keys:?}");
        for (c, voice) in all() {
            let clip = c.clip(voice);
            let bytes = midi::write(&clip, c.name);
            let (notes, end) = midi::parse(&bytes).unwrap();
            let back = Clip::from_notes(notes, end).unwrap();
            assert_eq!(back, clip, "{}", c.name);
            // The detector finds the key each clip was written in.
            let est = key::detect(&back.notes).unwrap();
            assert_eq!((est.tonic, est.minor), (c.root, c.minor), "{}: {est:?}", c.name);
            let motif = Motif::from_clip(c.name, &back, voice).unwrap();
            assert_eq!((motif.tonic, motif.minor), (c.root, c.minor));
        }
    }

    /// Pitch classes of the chord (triad and 7th) over the root.
    fn chord_tones(minor: bool) -> [u8; 4] {
        if minor { [0, 3, 7, 10] } else { [0, 4, 7, 11] }
    }

    #[test]
    fn chord_tones_on_the_beats_passing_tones_on_odd_sixteenths() {
        for (c, voice) in all() {
            for n in &c.clip(voice).notes {
                let step = (n.start % TICKS_PER_BAR as u32 / TICKS_PER_STEP as u32) as usize;
                let pc = ((n.pitch as i32 - c.root as i32).rem_euclid(12)) as u8;
                if step % 2 == 0 {
                    assert!(chord_tones(c.minor).contains(&pc), "{}: pc {pc} on step {step}", c.name);
                } else {
                    let scale = if c.minor { crate::live::harmony::MINOR_SCALE } else { crate::live::harmony::MAJOR_SCALE };
                    assert!(scale.contains(&pc), "{}: pc {pc} on step {step} is not in the scale", c.name);
                }
            }
        }
    }

    #[test]
    fn bass_is_root_heavy_and_leads_rest() {
        for c in &BASS {
            let notes = c.clip(Voice::Lowest).notes;
            let roots = notes.iter().filter(|n| (n.pitch as i32 - c.root as i32).rem_euclid(12) == 0).count();
            assert!(roots * 2 >= notes.len(), "{}: {roots} of {} on the root", c.name, notes.len());
            // Mostly the root, octave, 5th and 7th.
            assert!(notes.iter().all(|n| [0, 7, 10, 3].contains(&((n.pitch as i32 - c.root as i32).rem_euclid(12)))), "{}", c.name);
        }
        for c in &LEAD {
            let clip = c.clip(Voice::Highest);
            let sounding: u32 = clip.notes.iter().map(|n| n.len).sum();
            assert!(sounding < clip.bars as u32 * TICKS_PER_BAR as u32 * 3 / 4, "{} leaves space", c.name);
        }
    }
}
