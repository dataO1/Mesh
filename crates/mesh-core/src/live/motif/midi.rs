//! Standard MIDI File reading and writing for motif clips.
//!
//! Type 0 and 1 files with any metrical resolution are read. Tempo is
//! ignored (a clip is bars, not seconds) and the meter is taken as 4/4.
//! Times are converted to engine ticks (96 per beat). Written files are type 0
//! at 480 ticks per quarter note, with the clip end as the track end so
//! trailing rests survive a round trip.

use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

use super::{Clip, RawNote};
use crate::live::TICKS_PER_BAR;

/// Resolution of written files.
pub const FILE_PPQ: u32 = 480;
/// Engine ticks per quarter note.
const ENGINE_PPQ: u32 = TICKS_PER_BAR as u32 / 4;
/// Tempo written into files (does not affect playback).
const FILE_BPM: u32 = 172;

#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    #[error("not a MIDI file: {0}")]
    Parse(String),
    #[error("SMPTE timing is not supported")]
    Timecode,
}

/// Engine ticks of `t` file ticks (rounded).
fn to_engine(t: u64, ppq: u32) -> u32 {
    ((t * ENGINE_PPQ as u64 * 2 + ppq as u64) / (ppq as u64 * 2)) as u32
}

/// Read the notes of a MIDI file in engine ticks, and the file's end (the
/// last track end or note end; 0 for an empty file).
pub fn parse(bytes: &[u8]) -> Result<(Vec<RawNote>, u32), MidiError> {
    let smf = Smf::parse(bytes).map_err(|e| MidiError::Parse(e.to_string()))?;
    let ppq = match smf.header.timing {
        Timing::Metrical(t) => (t.as_int() as u32).max(1),
        Timing::Timecode(..) => return Err(MidiError::Timecode),
    };
    let mut notes = Vec::new();
    let mut end = 0u32;
    for track in &smf.tracks {
        let mut now = 0u64;
        // Open notes by (channel, key): start tick and velocity.
        let mut open: Vec<(u8, u8, u64, u8)> = Vec::new();
        let finish = |open: &mut Vec<(u8, u8, u64, u8)>, channel: u8, key: u8, at: u64, notes: &mut Vec<RawNote>| {
            if let Some(i) = open.iter().position(|o| o.0 == channel && o.1 == key) {
                let (_, _, start, vel) = open.remove(i);
                let (s, e) = (to_engine(start, ppq), to_engine(at, ppq));
                notes.push(RawNote { start: s, len: e.saturating_sub(s).max(1), pitch: key, vel });
            }
        };
        for ev in track {
            now += ev.delta.as_int() as u64;
            match ev.kind {
                TrackEventKind::Midi { channel, message } => {
                    let channel = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            // A retriggered key ends the earlier note.
                            finish(&mut open, channel, key.as_int(), now, &mut notes);
                            open.push((channel, key.as_int(), now, vel.as_int()));
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            finish(&mut open, channel, key.as_int(), now, &mut notes);
                        }
                        _ => {}
                    }
                }
                TrackEventKind::Meta(MetaMessage::EndOfTrack) => end = end.max(to_engine(now, ppq)),
                _ => {}
            }
        }
        // Notes still sounding at the track end stop there.
        let stop = now;
        for (channel, key, ..) in open.clone() {
            finish(&mut open, channel, key, stop, &mut notes);
        }
    }
    let last = notes.iter().map(|n| n.start + n.len).max().unwrap_or(0);
    Ok((notes, end.max(last)))
}

/// Pitch bend and controller data of a MIDI file, in engine ticks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Expression {
    /// (tick, bend in cents), with the file's bend range (RPN 0, default
    /// ±2 semitones) applied.
    pub bends: Vec<(u32, i16)>,
    /// (tick, controller number, value 0..127), RPN and data-entry
    /// controllers excluded.
    pub ccs: Vec<(u32, u8, u8)>,
}

/// [`parse`] plus the file's pitch bends and controller changes (any channel).
pub fn parse_expressive(bytes: &[u8]) -> Result<(Vec<RawNote>, Expression, u32), MidiError> {
    let (notes, end) = parse(bytes)?;
    let smf = Smf::parse(bytes).map_err(|e| MidiError::Parse(e.to_string()))?;
    let ppq = match smf.header.timing {
        Timing::Metrical(t) => (t.as_int() as u32).max(1),
        Timing::Timecode(..) => return Err(MidiError::Timecode),
    };
    let mut x = Expression::default();
    for track in &smf.tracks {
        let mut now = 0u64;
        // RPN state per channel: (MSB, LSB) selected, bend range in semitones.
        let mut rpn = [(127u8, 127u8); 16];
        let mut range = [2.0f32; 16];
        for ev in track {
            now += ev.delta.as_int() as u64;
            let TrackEventKind::Midi { channel, message } = ev.kind else { continue };
            let ch = channel.as_int() as usize;
            let at = to_engine(now, ppq);
            match message {
                MidiMessage::PitchBend { bend } => {
                    let v = bend.as_int() as f32 / 8192.0;
                    x.bends.push((at, (v * range[ch] * 100.0).round() as i16));
                }
                MidiMessage::Controller { controller, value } => {
                    let (c, v) = (controller.as_int(), value.as_int());
                    match c {
                        101 => rpn[ch].0 = v,
                        100 => rpn[ch].1 = v,
                        6 if rpn[ch] == (0, 0) => range[ch] = v as f32,
                        38 if rpn[ch] == (0, 0) => range[ch] = range[ch].floor() + v as f32 / 100.0,
                        6 | 38 | 98 | 99 | 100 | 101 => {}
                        _ => x.ccs.push((at, c, v)),
                    }
                }
                _ => {}
            }
        }
    }
    x.bends.sort_by_key(|b| b.0);
    x.ccs.sort_by_key(|c| c.0);
    Ok((notes, x, end))
}

/// Write a clip as a type 0 file.
pub fn write(clip: &Clip, name: &str) -> Vec<u8> {
    let scale = FILE_PPQ / ENGINE_PPQ;
    let tempo = u24::new(60_000_000 / FILE_BPM);
    let mut events: Vec<(u32, u8, TrackEventKind)> = Vec::new();
    for n in &clip.notes {
        let on = TrackEventKind::Midi { channel: u4::new(0), message: MidiMessage::NoteOn { key: u7::new(n.pitch), vel: u7::new(n.vel.clamp(1, 127)) } };
        let off = TrackEventKind::Midi { channel: u4::new(0), message: MidiMessage::NoteOff { key: u7::new(n.pitch), vel: u7::new(0) } };
        events.push((n.start * scale, 1, on));
        events.push(((n.start + n.len) * scale, 0, off));
    }
    // Offs before ons at the same tick.
    events.sort_by_key(|e| (e.0, e.1));
    let mut track: Vec<TrackEvent> = vec![
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::Tempo(tempo)) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TimeSignature(4, 2, 24, 8)) },
    ];
    let mut at = 0u32;
    for (tick, _, kind) in events {
        track.push(TrackEvent { delta: u28::new(tick - at), kind });
        at = tick;
    }
    let end = clip.bars as u32 * TICKS_PER_BAR as u32 * scale;
    track.push(TrackEvent { delta: u28::new(end.saturating_sub(at)), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
    let smf = Smf { header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(FILE_PPQ as u16))), tracks: vec![track] };
    let mut out = Vec::new();
    smf.write_std(&mut out).expect("writing to a Vec cannot fail");
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_parse_round_trip() {
        let clip = Clip {
            bars: 2,
            notes: vec![
                RawNote { start: 0, len: 72, pitch: 33, vel: 118 },
                RawNote { start: 72, len: 24, pitch: 45, vel: 96 },
                RawNote { start: 384 + 48, len: 300, pitch: 40, vel: 100 },
            ],
        };
        let bytes = write(&clip, "round trip");
        let (notes, end) = parse(&bytes).unwrap();
        assert_eq!(notes, clip.notes);
        assert_eq!(end, 2 * TICKS_PER_BAR as u32, "the track end keeps the clip length");
        assert_eq!(Clip::from_notes(notes, end).unwrap(), clip);
    }

    #[test]
    fn other_resolutions_are_converted() {
        // A hand-built type 1 file at 960 ppq: a tempo track and a note track.
        let mut t0 = vec![TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(400_000))) }];
        t0.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
        let on = |d: u32, key: u8, vel: u8| TrackEvent {
            delta: u28::new(d),
            kind: TrackEventKind::Midi { channel: u4::new(2), message: MidiMessage::NoteOn { key: u7::new(key), vel: u7::new(vel) } },
        };
        // Note off as a zero-velocity note on, one beat later.
        let t1 = vec![
            on(0, 50, 90),
            on(960, 50, 0),
            on(480, 53, 100),
            on(960, 53, 0),
            TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) },
        ];
        let smf = Smf { header: Header::new(Format::Parallel, Timing::Metrical(u15::new(960))), tracks: vec![t0, t1] };
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        let (notes, end) = parse(&bytes).unwrap();
        assert_eq!(
            notes,
            vec![RawNote { start: 0, len: 96, pitch: 50, vel: 90 }, RawNote { start: 144, len: 96, pitch: 53, vel: 100 }]
        );
        assert_eq!(end, 240);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse(b"not midi").is_err());
        assert!(parse(&[]).is_err());
    }
}
