//! Phrases of a sound pack: the user's MIDI clips, kept as written
//! (docs/plans/2026-10-16-live-sound-packs.md).
//!
//! A phrase is up to [`MAX_PHRASE_BARS`] bars of notes (absolute pitch, any
//! number at once), pitch bends and controller curves. Its key is detected
//! from the notes; the `phrase` generator moves it to the lane's key by the
//! shortest transposition and remixes it with the lane's knobs.
//!
//! Each note gets an importance at load ([`PhraseNote::weight`]), so the
//! Density knob thins the least important notes first: long notes, notes
//! after a rest, pickups into a beat (steps 3, 7, 11, 15 of a bar and the
//! 8th before a beat) and the first note of the phrase count most. Thinning
//! by metric position would remove exactly the anticipations that carry a
//! DnB riff (`.claude/skills/synth-design/references/music-theory.md`,
//! sections 3 and 8).

use super::motif::key;
use super::motif::midi::{self, Expression};
use super::motif::RawNote;
use super::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// Longest phrase in bars (a drop and its variation).
pub const MAX_PHRASE_BARS: u32 = 64;
/// Most notes per phrase.
pub const MAX_PHRASE_NOTES: usize = 4096;
/// Bend and CC events closer than this are thinned (a 64th note).
pub const CURVE_STEP_TICKS: u32 = TICKS_PER_STEP as u32 / 4;

/// One note of a phrase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhraseNote {
    /// Ticks from the phrase start.
    pub tick: u32,
    pub len: u32,
    pub key: u8,
    pub vel: u8,
    /// Importance for density thinning, higher = kept longer (see the module docs).
    pub weight: f32,
}

/// A parsed phrase.
#[derive(Debug, Clone, PartialEq)]
pub struct Phrase {
    pub name: String,
    pub bars: u32,
    /// Detected tonic (pitch class) and mode.
    pub tonic: u8,
    pub minor: bool,
    /// Sorted by tick.
    pub notes: Vec<PhraseNote>,
    /// (tick, cents), sorted, thinned to [`CURVE_STEP_TICKS`].
    pub bends: Vec<(u32, i16)>,
    /// (tick, controller, value), sorted, thinned per controller.
    pub ccs: Vec<(u32, u8, u8)>,
}

impl Phrase {
    /// Parse a Standard MIDI File into a phrase. `None` without notes.
    pub fn from_midi(name: &str, bytes: &[u8]) -> Result<Self, String> {
        let (raw, expr, end) = midi::parse_expressive(bytes).map_err(|e| e.to_string())?;
        Self::from_parts(name, raw, expr, end).ok_or_else(|| "no notes".to_string())
    }

    /// Build from notes and expression in engine ticks; `end` is the clip end.
    pub fn from_parts(name: &str, mut raw: Vec<RawNote>, expr: Expression, end: u32) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        raw.sort_by_key(|n| (n.start, n.pitch));
        let bar = TICKS_PER_BAR as u32;
        let bars = end.div_ceil(bar).clamp(1, MAX_PHRASE_BARS);
        let limit = bars * bar;
        raw.retain(|n| n.start < limit);
        raw.truncate(MAX_PHRASE_NOTES);
        let est = key::detect(&raw)?;
        let mut notes: Vec<PhraseNote> = raw
            .iter()
            .map(|n| PhraseNote { tick: n.start, len: n.len.min(limit - n.start).max(1), key: n.pitch, vel: n.vel.max(1), weight: 0.0 })
            .collect();
        weigh(&mut notes);
        let thin = |pts: Vec<(u32, i16)>| {
            let mut out: Vec<(u32, i16)> = Vec::new();
            for (t, v) in pts.into_iter().filter(|p| p.0 < limit) {
                match out.last_mut() {
                    Some(last) if t < last.0 + CURVE_STEP_TICKS => last.1 = v,
                    _ => out.push((t, v)),
                }
            }
            out
        };
        let bends = thin(expr.bends);
        let mut ccs: Vec<(u32, u8, u8)> = Vec::new();
        let mut numbers: Vec<u8> = expr.ccs.iter().map(|c| c.1).collect();
        numbers.sort_unstable();
        numbers.dedup();
        for n in numbers {
            let pts = expr.ccs.iter().filter(|c| c.1 == n).map(|c| (c.0, c.2 as i16)).collect();
            ccs.extend(thin(pts).into_iter().map(|(t, v)| (t, n, v as u8)));
        }
        ccs.sort_by_key(|c| c.0);
        Some(Self { name: name.to_string(), bars, tonic: est.tonic, minor: est.minor, notes, bends, ccs })
    }

    /// Semitones that move the phrase to `root` (pitch class) by the shortest way.
    pub fn shift_to(&self, root: u8) -> i32 {
        let d = (root as i32 - self.tonic as i32).rem_euclid(12);
        if d > 6 { d - 12 } else { d }
    }

    /// Index range of the notes that start in phrase bar `bar`.
    pub fn bar_notes(&self, bar: u32) -> std::ops::Range<usize> {
        let (lo, hi) = (bar * TICKS_PER_BAR as u32, (bar + 1) * TICKS_PER_BAR as u32);
        let a = self.notes.partition_point(|n| n.tick < lo);
        let b = self.notes.partition_point(|n| n.tick < hi);
        a..b
    }

    /// The value of controller `cc` in force at `tick` (the last change at or
    /// before it), if the phrase sets it.
    pub fn cc_at(&self, cc: u8, tick: u32) -> Option<u8> {
        self.ccs.iter().filter(|c| c.1 == cc && c.0 <= tick).last().map(|c| c.2)
    }

    /// The bend in force at `tick`, in cents (0 before the first bend).
    pub fn bend_at(&self, tick: u32) -> i16 {
        self.bends.iter().filter(|b| b.0 <= tick).last().map_or(0, |b| b.1)
    }
}

/// Note importance (see the module docs).
fn weigh(notes: &mut [PhraseNote]) {
    let step = TICKS_PER_STEP as u32;
    let mut prev_end = 0u32;
    for (i, n) in notes.iter_mut().enumerate() {
        let s = (n.tick / step) as usize % STEPS_PER_BAR;
        let mut w = 0.0;
        if n.len >= 4 * step {
            w += 1.0;
        } else if n.len >= 2 * step {
            w += 0.4;
        }
        if i == 0 || n.tick >= prev_end + 2 * step {
            w += 0.6;
        }
        if matches!(s, 3 | 7 | 11 | 15) || matches!(s, 2 | 6 | 10 | 14) && n.len >= 2 * step {
            w += 0.5;
        }
        if i == 0 {
            w += 1.0;
        }
        w += 0.3 * n.vel as f32 / 127.0;
        n.weight = w;
        prev_end = prev_end.max(n.tick + n.len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(step: u32, len_steps: u32, key: u8) -> RawNote {
        RawNote { start: step * TICKS_PER_STEP as u32, len: len_steps * TICKS_PER_STEP as u32, pitch: key, vel: 100 }
    }

    #[test]
    fn keeps_long_phrases_and_weighs_anchors() {
        // 20 bars; a long note, a pickup on step 15, a short on-beat note.
        let raw = vec![note(1, 8, 43), note(15, 1, 46), note(16 + 4, 1, 43), note(16 * 19, 4, 41)];
        let p = Phrase::from_parts("x", raw, Expression::default(), 20 * TICKS_PER_BAR as u32).unwrap();
        assert_eq!(p.bars, 20, "beyond the old 4-bar clip limit");
        let w = |i: usize| p.notes[i].weight;
        assert!(w(0) > w(2), "the long first note outweighs a short on-beat note");
        assert!(w(1) > w(2), "the pickup outweighs a short on-beat note");
        assert_eq!(p.bar_notes(1), 2..3);
        assert_eq!(p.bar_notes(19), 3..4);
    }

    #[test]
    fn transposes_by_the_shortest_way_and_thins_curves() {
        let raw = vec![note(0, 8, 43), note(8, 4, 46), note(16, 8, 43)]; // around G
        let mut x = Expression::default();
        for t in 0..20 {
            x.bends.push((t, t as i16 * 10));
        }
        x.ccs.push((0, 74, 10));
        x.ccs.push((200, 74, 90));
        let p = Phrase::from_parts("x", raw, x, TICKS_PER_BAR as u32).unwrap();
        assert_eq!(p.tonic, 7);
        assert_eq!(p.shift_to(5), -2, "G to F moves down a tone");
        assert_eq!(p.shift_to(1), 6);
        assert!(p.bends.len() <= 4, "bends thinned to the curve grid: {}", p.bends.len());
        assert_eq!(p.cc_at(74, 100), Some(10));
        assert_eq!(p.cc_at(74, 300), Some(90));
        assert_eq!(p.cc_at(1, 300), None);
    }
}
