//! Key and mode estimation for motif clips.
//!
//! Krumhansl-Schmuckler: the clip's pitch-class histogram, weighted by note
//! length, is correlated with the major and minor key profiles in all 12
//! rotations. Two tie-breakers suit riffs, which are short and root-heavy:
//! the tonic must carry weight and likely starts or ends the clip, and a
//! clip without a third (a bass on the root and 5th) reads as minor, the
//! genre's default.

use super::RawNote;
use crate::live::harmony::{MAJOR_SCALE, MINOR_SCALE};

/// Krumhansl-Kessler key profiles, tonic first.
const MAJOR_PROFILE: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

// MUSICAL: first draft, tune by ear. Bonus per key for the share of the
// clip's weight on its tonic, for starting on it and for ending on it.
const TONIC_WEIGHT: f64 = 0.35;
const FIRST_BONUS: f64 = 0.06;
const LAST_BONUS: f64 = 0.04;
/// A clip with less than this share on both thirds reads as minor when the
/// minor key is at most `NO_THIRD_MARGIN` behind.
const THIRD_MIN_SHARE: f64 = 0.03;
const NO_THIRD_MARGIN: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyEstimate {
    /// Pitch class 0..12 (C = 0).
    pub tonic: u8,
    pub minor: bool,
    /// Correlation plus the bonuses; higher is surer.
    pub score: f32,
}

fn correlation(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let (ma, mb) = (a.iter().sum::<f64>() / 12.0, b.iter().sum::<f64>() / 12.0);
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        num += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da <= 0.0 || db <= 0.0 { 0.0 } else { num / (da * db).sqrt() }
}

/// Estimate a clip's key and mode from its notes (weighted by length).
/// `None` for no notes.
pub fn detect(notes: &[RawNote]) -> Option<KeyEstimate> {
    let mut weight = [0.0f64; 12];
    for n in notes {
        weight[n.pitch as usize % 12] += n.len.max(1) as f64;
    }
    let total: f64 = weight.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let first = notes.iter().min_by_key(|n| n.start)?.pitch as usize % 12;
    let last = notes.iter().max_by_key(|n| n.start)?.pitch as usize % 12;
    let score = |tonic: usize, minor: bool| -> f64 {
        let profile = if minor { &MINOR_PROFILE } else { &MAJOR_PROFILE };
        let rotated: [f64; 12] = std::array::from_fn(|i| profile[(i + 12 - tonic) % 12]);
        correlation(&weight, &rotated)
            + TONIC_WEIGHT * weight[tonic] / total
            + if first == tonic { FIRST_BONUS } else { 0.0 }
            + if last == tonic { LAST_BONUS } else { 0.0 }
    };
    let mut best = (0usize, true, f64::MIN);
    for tonic in 0..12 {
        for minor in [true, false] {
            let s = score(tonic, minor);
            if s > best.2 {
                best = (tonic, minor, s);
            }
        }
    }
    let (tonic, mut minor, mut s) = best;
    // No third to tell them apart: minor, unless the major reading is clearly ahead.
    let thirds = (weight[(tonic + 3) % 12] + weight[(tonic + 4) % 12]) / total;
    if !minor && thirds < THIRD_MIN_SHARE {
        let alt = score(tonic, true);
        if alt >= s - NO_THIRD_MARGIN {
            minor = true;
            s = alt;
        }
    }
    Some(KeyEstimate { tonic: tonic as u8, minor, score: s as f32 })
}

/// A MIDI pitch as a scale degree from `tonic` (7 per octave, counted from
/// the octave of MIDI 0) in the natural minor or major scale, plus the
/// semitones above that degree for a note outside the scale.
pub fn to_degree(pitch: u8, tonic: u8, minor: bool) -> (i32, i8) {
    let scale = if minor { &MINOR_SCALE } else { &MAJOR_SCALE };
    let rel = pitch as i32 - tonic as i32;
    let (octave, pc) = (rel.div_euclid(12), rel.rem_euclid(12) as u8);
    // The scale tone at or below the pitch class: it always exists (degree 0 is 0).
    let index = scale.iter().rposition(|&s| s <= pc).unwrap_or(0);
    (octave * 7 + index as i32, (pc - scale[index]) as i8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(pitches: &[(u8, u32)]) -> Vec<RawNote> {
        pitches
            .iter()
            .enumerate()
            .map(|(i, &(pitch, len))| RawNote { start: i as u32 * 96, len, pitch, vel: 100 })
            .collect()
    }

    #[test]
    fn a_minor_line_reads_as_a_minor() {
        // A C E D E C A G A
        let n = notes(&[(57, 96), (60, 48), (64, 96), (62, 48), (64, 48), (60, 48), (57, 96), (55, 48), (57, 192)]);
        let k = detect(&n).unwrap();
        assert_eq!((k.tonic, k.minor), (9, true));
    }

    #[test]
    fn c_major_line_reads_as_c_major() {
        // C E G E C D E G C
        let n = notes(&[(60, 96), (64, 96), (67, 96), (64, 48), (60, 48), (62, 48), (64, 48), (67, 96), (72, 192)]);
        let k = detect(&n).unwrap();
        assert_eq!((k.tonic, k.minor), (0, false));
    }

    #[test]
    fn every_key_is_found_from_its_own_scale_line() {
        for tonic in 0..12u8 {
            for (minor, scale) in [(true, MINOR_SCALE), (false, MAJOR_SCALE)] {
                // Root, 3rd, 5th, 6th, 7th, 5th, 3rd, root (an up and down line).
                let line: Vec<(u8, u32)> = [0usize, 2, 4, 5, 6, 4, 2, 0]
                    .iter()
                    .map(|&d| (48 + tonic + scale[d], if d == 0 { 192 } else { 96 }))
                    .collect();
                let k = detect(&notes(&line)).unwrap();
                assert_eq!((k.tonic, k.minor), (tonic, minor), "tonic {tonic} minor {minor}");
            }
        }
    }

    #[test]
    fn a_root_and_fifth_bass_is_minor_on_its_root() {
        for tonic in [9u8, 2, 4, 5] {
            let n = notes(&[(24 + tonic, 96), (24 + tonic, 96), (36 + tonic, 96), (31 + tonic, 48), (24 + tonic, 96)]);
            let k = detect(&n).unwrap();
            assert_eq!((k.tonic, k.minor), (tonic, true));
        }
    }

    #[test]
    fn degrees_and_chromatic_offsets() {
        // A minor, tonic pc 9. Scale tones give plain degrees.
        assert_eq!(to_degree(45, 9, true), (3 * 7, 0)); // A2: (45-9)=36 -> 3 octaves
        assert_eq!(to_degree(48, 9, true), (3 * 7 + 2, 0)); // C3 = b3
        assert_eq!(to_degree(52, 9, true), (3 * 7 + 4, 0)); // E3 = 5th
        assert_eq!(to_degree(55, 9, true), (3 * 7 + 6, 0)); // G3 = b7
        // Outside the scale: the degree below, plus a semitone.
        assert_eq!(to_degree(49, 9, true), (3 * 7 + 2, 1)); // C#3 = major 3rd
        assert_eq!(to_degree(56, 9, true), (3 * 7 + 6, 1)); // G#3 = leading tone
        // Below the tonic's octave start.
        assert_eq!(to_degree(43, 9, true), (2 * 7 + 6, 0)); // G2 = b7 below
        // Major.
        assert_eq!(to_degree(64, 0, false), (5 * 7 + 2, 0)); // E4 in C
        assert_eq!(to_degree(70, 0, false), (5 * 7 + 5, 1)); // Bb4 in C: the 6th, raised
    }
}
