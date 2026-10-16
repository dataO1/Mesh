//! Curated motif clips for the bass and lead lanes.
//!
//! The style folder holds MIDI clips (`motifs/bass/*.mid`, `motifs/lead/*.mid`,
//! plus any files a lane lists in its `motifs` field). A clip is 1, 2 or 4
//! bars of 4/4. At load time each clip is parsed ([`midi`]), its key and mode
//! are estimated from its notes ([`key`]) and its notes become scale degrees
//! with a chromatic offset, relative to the detected tonic. The generators
//! ([`crate::live::generator`]) re-pitch those degrees against the live key,
//! mode and chord, so a clip written in D minor plays in any key and follows
//! the progression, while its rhythm and contour stay as written.
//!
//! Everything here runs at load time on the loader thread. Rendering only
//! reads the built [`Motif`]s.

pub mod key;
pub mod midi;
pub mod starter;

use std::path::{Path, PathBuf};

use super::{TICKS_PER_BAR, TICKS_PER_STEP};

/// Clip lengths in bars.
pub const CLIP_BARS: [u8; 3] = [1, 2, 4];
/// Most clips a lane uses (the rest are ignored).
pub const MAX_CLIPS: usize = 32;
/// Most notes of one clip (the rest are ignored).
pub const MAX_CLIP_NOTES: usize = 128;
/// Onsets this close (ticks) count as one chord: one voice survives.
const CHORD_TICKS: u32 = 6;
/// A last note ending this far past a bar line does not open another bar.
const BAR_TOLERANCE: u32 = TICKS_PER_STEP as u32 / 2;
/// Lowest scale degree (below the tonic) a bass clip's lowest note may sit
/// at, and the same for a lead clip. The clip is moved by whole octaves into
/// the range `floor ..= floor + 6`, so the lane's register decides the rest.
const BASS_FLOOR_DEGREE: i32 = -3;
const LEAD_FLOOR_DEGREE: i32 = -1;

/// A note as a MIDI file holds it, in engine ticks from the clip start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawNote {
    pub start: u32,
    pub len: u32,
    pub pitch: u8,
    pub vel: u8,
}

/// A parsed clip: whole bars and its notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    /// 1, 2 or 4.
    pub bars: u8,
    /// Sorted by start.
    pub notes: Vec<RawNote>,
}

impl Clip {
    /// Build a clip from notes. `end` is the clip's written end in engine
    /// ticks (the MIDI track end; 0 = unknown). The length rounds up to whole
    /// bars (3 becomes 4) and caps at 4; notes starting later are dropped and
    /// the others cut at the clip end. `None` without notes.
    pub fn from_notes(mut notes: Vec<RawNote>, end: u32) -> Option<Self> {
        notes.sort_by_key(|n| (n.start, n.pitch));
        let bar = TICKS_PER_BAR as u32;
        let span = notes.iter().map(|n| n.start + n.len).max()?.max(end);
        let last_start = notes.last()?.start;
        let needed = span.saturating_sub(BAR_TOLERANCE).div_ceil(bar).max(last_start / bar + 1);
        let bars = CLIP_BARS.iter().copied().find(|&b| b as u32 >= needed).unwrap_or(4);
        let limit = bars as u32 * bar;
        notes.retain(|n| n.start < limit);
        for n in &mut notes {
            n.len = n.len.min(limit - n.start);
        }
        notes.truncate(MAX_CLIP_NOTES);
        (!notes.is_empty()).then_some(Self { bars, notes })
    }
}

/// Which voice survives when a clip has chords: bass keeps the lowest note,
/// lead the highest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    Lowest,
    Highest,
}

/// One note of a motif, ready to re-pitch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotifNote {
    /// Ticks from the clip start.
    pub tick: u16,
    /// Written length in ticks (cut at the next note).
    pub len: u16,
    pub vel: u8,
    /// Scale degree from the detected tonic (7 per octave; the clip's lowest
    /// note sits in the voice's floor octave).
    pub degree: i16,
    /// Semitones above that scale degree for notes outside the detected scale.
    pub chroma: i8,
}

/// A clip as the generators play it.
#[derive(Debug, Clone, PartialEq)]
pub struct Motif {
    pub name: String,
    /// 1, 2 or 4.
    pub bars: u8,
    /// Detected tonic (pitch class) and mode.
    pub tonic: u8,
    pub minor: bool,
    /// Monophonic, sorted by tick.
    pub notes: Vec<MotifNote>,
}

impl Motif {
    /// Build from a clip: one voice, detected key, degrees. `None` for a
    /// clip without notes.
    pub fn from_clip(name: &str, clip: &Clip, voice: Voice) -> Option<Self> {
        // One voice: chords keep the lowest or highest note, and no note
        // runs into the next.
        let mut mono: Vec<RawNote> = Vec::with_capacity(clip.notes.len());
        for n in &clip.notes {
            match mono.last_mut() {
                Some(last) if n.start - last.start <= CHORD_TICKS => {
                    let better = match voice {
                        Voice::Lowest => n.pitch < last.pitch,
                        Voice::Highest => n.pitch > last.pitch,
                    };
                    if better {
                        *last = *n;
                    }
                }
                _ => mono.push(*n),
            }
        }
        let limit = clip.bars as u32 * TICKS_PER_BAR as u32;
        for i in 0..mono.len() {
            let next = mono.get(i + 1).map_or(limit, |n| n.start);
            mono[i].len = mono[i].len.min(next - mono[i].start).max(1);
        }
        let est = key::detect(&mono)?;
        let mut notes: Vec<MotifNote> = mono
            .iter()
            .map(|n| {
                let (degree, chroma) = key::to_degree(n.pitch, est.tonic, est.minor);
                MotifNote { tick: n.start as u16, len: n.len as u16, vel: n.vel.clamp(1, 127), degree: degree as i16, chroma }
            })
            .collect();
        // Whole octaves, so the lowest note lands in the voice's floor octave.
        let floor = match voice {
            Voice::Lowest => BASS_FLOOR_DEGREE,
            Voice::Highest => LEAD_FLOOR_DEGREE,
        };
        let lowest = notes.iter().map(|n| n.degree as i32).min()?;
        let shift = floor + (lowest - floor).rem_euclid(7) - lowest;
        for n in &mut notes {
            n.degree = (n.degree as i32 + shift) as i16;
        }
        Some(Self { name: name.to_string(), bars: clip.bars, tonic: est.tonic, minor: est.minor, notes })
    }

    /// Parse a MIDI file's bytes into a motif.
    pub fn from_midi(name: &str, bytes: &[u8], voice: Voice) -> Result<Self, String> {
        let (notes, end) = midi::parse(bytes).map_err(|e| e.to_string())?;
        let clip = Clip::from_notes(notes, end).ok_or_else(|| "no notes".to_string())?;
        Self::from_clip(name, &clip, voice).ok_or_else(|| "no notes".to_string())
    }

    /// Index range in [`Motif::notes`] of the notes starting in clip bar `bar`.
    pub fn bar_range(&self, bar: usize) -> std::ops::Range<usize> {
        let (lo, hi) = (bar as u16 * TICKS_PER_BAR, (bar as u16 + 1) * TICKS_PER_BAR);
        self.notes.partition_point(|n| n.tick < lo)..self.notes.partition_point(|n| n.tick < hi)
    }

    /// Onsets of clip bar `bar` as a step mask (bit per 16th, nearest step).
    pub fn onsets(&self, bar: usize) -> u16 {
        let mut bits = 0u16;
        for n in &self.notes[self.bar_range(bar)] {
            let in_bar = n.tick % TICKS_PER_BAR;
            let step = ((in_bar + TICKS_PER_STEP / 2) / TICKS_PER_STEP) as usize;
            if step < 16 {
                bits |= 1 << step;
            }
        }
        bits
    }
}

/// `*.mid` / `*.midi` files directly inside `dir`, sorted by name (missing
/// folder = none).
pub fn scan_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = read
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
        })
        .collect();
    files.sort();
    files
}

/// Load motifs from MIDI files, skipping duplicates and unreadable files.
/// Returns the motifs and a warning per skipped file.
pub fn load_files(paths: &[PathBuf], voice: Voice) -> (Vec<Motif>, Vec<String>) {
    let mut motifs: Vec<Motif> = Vec::new();
    let mut seen: Vec<&PathBuf> = Vec::new();
    let mut warnings = Vec::new();
    for path in paths {
        if seen.contains(&path) {
            continue;
        }
        seen.push(path);
        if motifs.len() >= MAX_CLIPS {
            warnings.push(format!("motif {}: more than {MAX_CLIPS} clips, ignored", path.display()));
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("motif");
        match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| Motif::from_midi(name, &b, voice)) {
            Ok(m) => motifs.push(m),
            Err(e) => warnings.push(format!("motif {}: {e}", path.display())),
        }
    }
    (motifs, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(start: u32, len: u32, pitch: u8) -> RawNote {
        RawNote { start, len, pitch, vel: 100 }
    }

    #[test]
    fn clip_length_rounds_up_to_whole_bars() {
        let bar = TICKS_PER_BAR as u32;
        let one = Clip::from_notes(vec![raw(0, 50, 57), raw(200, 50, 60)], 0).unwrap();
        assert_eq!(one.bars, 1);
        // The track end sets the length: trailing rests count.
        assert_eq!(Clip::from_notes(vec![raw(0, 50, 57)], bar + 10).unwrap().bars, 1, "a hair past the bar line");
        assert_eq!(Clip::from_notes(vec![raw(0, 50, 57)], 2 * bar).unwrap().bars, 2);
        assert_eq!(Clip::from_notes(vec![raw(0, 50, 57), raw(bar + 5, 50, 60)], 0).unwrap().bars, 2);
        assert_eq!(Clip::from_notes(vec![raw(2 * bar + 5, 50, 60)], 0).unwrap().bars, 4, "three bars round up");
        // Longer tails are ignored.
        let long = Clip::from_notes(vec![raw(0, 50, 57), raw(5 * bar, 50, 60), raw(3 * bar, 4 * bar, 62)], 8 * bar).unwrap();
        assert_eq!(long.bars, 4);
        assert_eq!(long.notes.len(), 2);
        assert_eq!(long.notes[1].len, bar, "the last note is cut at the clip end");
        assert!(Clip::from_notes(Vec::new(), 100).is_none());
    }

    #[test]
    fn chords_keep_one_voice_and_notes_do_not_overlap() {
        let notes = vec![raw(0, 96, 45), raw(2, 96, 57), raw(2, 96, 52), raw(48, 96, 47)];
        let clip = Clip::from_notes(notes, 0).unwrap();
        let low = Motif::from_clip("t", &clip, Voice::Lowest).unwrap();
        assert_eq!(low.notes.len(), 2);
        assert_eq!(low.notes[0].len, 48, "cut at the next note");
        let high = Motif::from_clip("t", &clip, Voice::Highest).unwrap();
        assert_eq!(high.notes.len(), 2);
    }

    #[test]
    fn lowest_note_sits_in_the_floor_octave() {
        // A minor, lowest note A5 or A2: the same degrees either way.
        let at = |base: u8| {
            let notes = vec![raw(0, 48, base), raw(96, 48, base + 7), raw(192, 48, base + 12)];
            Motif::from_clip("t", &Clip::from_notes(notes, 0).unwrap(), Voice::Lowest).unwrap()
        };
        let (a, b) = (at(33), at(81));
        assert_eq!(a.notes.iter().map(|n| n.degree).collect::<Vec<_>>(), b.notes.iter().map(|n| n.degree).collect::<Vec<_>>());
        assert_eq!(a.notes[0].degree as i32, 0);
        // A clip that starts on its 5th keeps it a 5th below the tonic.
        let notes = vec![raw(0, 48, 40), raw(96, 48, 45)];
        let m = Motif::from_clip("t", &Clip::from_notes(notes, 0).unwrap(), Voice::Lowest).unwrap();
        assert_eq!(m.notes[0].degree, -3);
        assert_eq!(m.notes[1].degree, 0);
    }

    #[test]
    fn bar_ranges_and_onsets() {
        let bar = TICKS_PER_BAR as u32;
        let notes = vec![raw(0, 48, 57), raw(72, 24, 60), raw(bar + 96, 48, 64)];
        let m = Motif::from_clip("t", &Clip::from_notes(notes, 0).unwrap(), Voice::Lowest).unwrap();
        assert_eq!(m.bars, 2);
        assert_eq!(m.bar_range(0), 0..2);
        assert_eq!(m.bar_range(1), 2..3);
        assert_eq!(m.onsets(0), 1 | 1 << 3);
        assert_eq!(m.onsets(1), 1 << 4);
    }
}
