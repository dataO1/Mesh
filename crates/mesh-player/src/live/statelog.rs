//! Per-bar state log written next to a set recording (`<name>.state.tsv`).
//!
//! One tab-separated row per bar: time into the recording, bar, tempo, key
//! and mode, progression, then per lane whether it plays, its sound, seed,
//! form, the form letter and written segment playing, octave, rate and the
//! main and Shift-layer control values. Lines up a moment of the recording
//! with the exact settings, so it can be rendered again offline.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use mesh_core::live::harmony::{camelot_label, mode_name};
use mesh_core::live::state::LiveShared;
use mesh_core::live::NUM_LANES;

use super::readout::LaneInfo;

pub struct StateLog {
    out: BufWriter<File>,
    last_bar: Option<u64>,
}

impl StateLog {
    /// Open `<recording>.state.tsv` and write the header. `None` (logged)
    /// when the file cannot be created.
    pub fn create(recording: &Path, lanes: &[LaneInfo]) -> Option<Self> {
        let path = recording.with_extension("state.tsv");
        let file = match File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("[LIVE] state log not written ({}): {e}", path.display());
                return None;
            }
        };
        let mut out = BufWriter::new(file);
        let mut head = String::from("time_s\tbar\tbpm\tkey\tmode\tprogression");
        for lane in lanes.iter().take(NUM_LANES) {
            let n = &lane.name;
            let _ = write!(head, "\t{n}.on\t{n}.sound\t{n}.seed\t{n}.form\t{n}.letter\t{n}.segment\t{n}.oct\t{n}.rate");
            for c in &lane.controls {
                let _ = write!(head, "\t{n}.{}", c.label);
            }
            let _ = write!(head, "\t{n}.shift");
        }
        let _ = writeln!(out, "{head}");
        log::info!("[LIVE] state log {}", path.display());
        Some(Self { out, last_bar: None })
    }

    /// Write a row when a new bar started since the last call.
    pub fn bar(&mut self, shared: &LiveShared, lanes: &[LaneInfo], elapsed: Duration) {
        let clock = shared.clock();
        if self.last_bar == Some(clock.bar) {
            return;
        }
        self.last_bar = Some(clock.bar);
        let key = shared.key();
        let mut row = format!(
            "{:.2}\t{}\t{:.1}\t{} {}\t{}\t{}",
            elapsed.as_secs_f64(),
            clock.bar,
            clock.bpm,
            camelot_label(key),
            key,
            mode_name(key, shared.mode()),
            shared.progression(),
        );
        for (li, lane) in lanes.iter().enumerate().take(NUM_LANES) {
            let v = shared.lane(li).view();
            let sound = lane.option_names.get(v.option as usize).map(String::as_str).unwrap_or("?");
            let letter = (b'A' + (v.pos.letter & 3)) as char;
            let _ = write!(
                row,
                "\t{}\t{sound}\t{}\t{}\t{letter}{}\t{}\t{:+}\t{:?}",
                v.on as u8,
                v.seed,
                v.form,
                if v.pos.prime { "'" } else { "" },
                v.pos.segment,
                v.octave,
                v.rate
            );
            for c in v.controls.iter().take(lane.controls.len()) {
                let _ = write!(row, "\t{c:.2}");
            }
            let shift: Vec<String> = v.shift_controls.iter().map(|c| format!("{c:.2}")).collect();
            let _ = write!(row, "\t{}", shift.join(","));
        }
        if writeln!(self.out, "{row}").and_then(|_| self.out.flush()).is_err() {
            log::warn!("[LIVE] state log write failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_core::music::MusicalKey;

    #[test]
    fn one_row_per_bar_next_to_the_recording() {
        let dir = std::env::temp_dir().join(format!("mesh-statelog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("set.wav");
        let shared = LiveShared::new(170.0, MusicalKey::new(9, true), 0);
        let mut log = StateLog::create(&wav, &[]).unwrap();
        log.bar(&shared, &[], Duration::from_secs(1));
        log.bar(&shared, &[], Duration::from_secs(1));
        drop(log);
        let text = std::fs::read_to_string(dir.join("set.state.tsv")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "header and one row: {text}");
        assert!(lines[0].starts_with("time_s\tbar\tbpm\tkey\tmode"));
        assert!(lines[1].contains("aeolian"), "{}", lines[1]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
