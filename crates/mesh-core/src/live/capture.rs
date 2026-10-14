//! Capture snapshots (design D16, §9).
//!
//! A capture stores what is needed to reproduce the current state from seeds:
//! per lane the 8 slot seeds, the loop length, the 4 control positions, the selected instrument
//! option and the on/off state, plus the global key, progression, tempo and
//! half-time flag. Generators are pure functions of these, so a capture
//! reproduces the same bars.
//!
//! Captures are written as YAML to
//! `<collection>/captures/<style>/<timestamp>.yaml`. During a set the
//! collection is the USB collection, so captures travel back with the stick.
//!
//! File IO here is non-RT (MIDI/UI thread side).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDateTime};
use serde::{Deserialize, Serialize};

use crate::music::MusicalKey;

use super::harmony::serde_key;
use super::style::{is_valid_style_name, CONTROLS_PER_LANE};
use super::{DEFAULT_LOOP_LEN, LOOP_LENGTHS, NUM_LANES, SLOTS_PER_CHAIN};

/// Folder under the collection root holding all captures.
pub const CAPTURES_FOLDER: &str = "captures";
/// Capture file format version. 2: 8 seeds per lane plus `loop_len`.
/// Version 1 files (4 seeds, no loop length) still load.
pub const CAPTURE_VERSION: u32 = 2;
/// Timestamp format used in the snapshot and its file name.
const TIMESTAMP_FORMAT: &str = "%Y-%m-%d_%H-%M-%S";

/// What the performer captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureScope {
    /// The whole state (Shift + Capture).
    #[default]
    All,
    /// One lane's chain (Capture). The snapshot still records every lane;
    /// recall applies only `lane`.
    Lane { lane: u8 },
}

/// One lane's captured state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneCapture {
    /// Seed of each chain slot. Shorter lists (version 1: 4 seeds) repeat
    /// to fill the chain.
    #[serde(deserialize_with = "deserialize_seeds")]
    pub seeds: [u32; SLOTS_PER_CHAIN],
    /// Bars the lane loops (1, 2, 4 or 8). Version 1 files: 4.
    #[serde(default = "default_loop_len")]
    pub loop_len: u8,
    /// Position of each performance control, `0..=1`.
    pub controls: [f32; CONTROLS_PER_LANE],
    /// Selected instrument option index.
    pub option: usize,
    /// Lane on/off.
    pub on: bool,
}

impl Default for LaneCapture {
    fn default() -> Self {
        Self { seeds: [0; SLOTS_PER_CHAIN], loop_len: DEFAULT_LOOP_LEN, controls: [0.5; CONTROLS_PER_LANE], option: 0, on: true }
    }
}

fn default_loop_len() -> u8 {
    DEFAULT_LOOP_LEN
}

/// 1..=8 seeds, repeated to fill the chain: a version-1 4-bar chain becomes
/// the same 4 bars twice, so it sounds the same at any loop length up to 4.
fn deserialize_seeds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[u32; SLOTS_PER_CHAIN], D::Error> {
    let seeds = Vec::<u32>::deserialize(d)?;
    if seeds.is_empty() || seeds.len() > SLOTS_PER_CHAIN {
        return Err(serde::de::Error::invalid_length(seeds.len(), &"1 to 8 seeds"));
    }
    Ok(std::array::from_fn(|i| seeds[i % seeds.len()]))
}

/// Captured global state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalCapture {
    /// Stored as a key name ("Am").
    #[serde(with = "serde_key")]
    pub key: MusicalKey,
    /// Index into the style's progressions.
    pub progression: usize,
    /// Tempo in BPM.
    pub tempo: f64,
    pub half_time: bool,
}

/// A saved snapshot of seeds and control values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub version: u32,
    /// Style the capture belongs to (folder name).
    pub style: String,
    /// Local time of capture, `YYYY-MM-DD_HH-MM-SS`.
    pub timestamp: String,
    #[serde(default)]
    pub scope: CaptureScope,
    pub global: GlobalCapture,
    pub lanes: [LaneCapture; NUM_LANES],
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("invalid style name '{0}'")]
    InvalidStyleName(String),
    #[error("invalid capture: {0}")]
    Invalid(String),
    #[error("capture I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse capture {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("cannot serialize capture: {0}")]
    Serialize(#[source] serde_yaml::Error),
}

/// `<collection>/captures`.
pub fn captures_root(collection: &Path) -> PathBuf {
    collection.join(CAPTURES_FOLDER)
}

/// `<collection>/captures/<style>`.
pub fn captures_dir(collection: &Path, style: &str) -> Result<PathBuf, CaptureError> {
    if !is_valid_style_name(style) {
        return Err(CaptureError::InvalidStyleName(style.to_string()));
    }
    Ok(captures_root(collection).join(style))
}

/// Format a time the way captures store it.
pub fn format_timestamp(time: DateTime<Local>) -> String {
    time.format(TIMESTAMP_FORMAT).to_string()
}

impl Capture {
    /// New snapshot stamped with the current local time.
    pub fn new(
        style: impl Into<String>,
        scope: CaptureScope,
        global: GlobalCapture,
        lanes: [LaneCapture; NUM_LANES],
    ) -> Self {
        Self::with_timestamp(style, format_timestamp(Local::now()), scope, global, lanes)
    }

    /// New snapshot with an explicit timestamp (tests, imports).
    pub fn with_timestamp(
        style: impl Into<String>,
        timestamp: impl Into<String>,
        scope: CaptureScope,
        global: GlobalCapture,
        lanes: [LaneCapture; NUM_LANES],
    ) -> Self {
        Self { version: CAPTURE_VERSION, style: style.into(), timestamp: timestamp.into(), scope, global, lanes }
    }

    /// Check the snapshot is usable: known version, sane names and ranges.
    pub fn validate(&self) -> Result<(), CaptureError> {
        let bad = |msg: String| Err(CaptureError::Invalid(msg));
        if self.version == 0 || self.version > CAPTURE_VERSION {
            return bad(format!("unsupported version {} (this build reads {CAPTURE_VERSION})", self.version));
        }
        if !is_valid_style_name(&self.style) {
            return Err(CaptureError::InvalidStyleName(self.style.clone()));
        }
        if NaiveDateTime::parse_from_str(&self.timestamp, TIMESTAMP_FORMAT).is_err() {
            return bad(format!("timestamp '{}' is not YYYY-MM-DD_HH-MM-SS", self.timestamp));
        }
        if !self.global.tempo.is_finite() || self.global.tempo <= 0.0 {
            return bad(format!("tempo {}", self.global.tempo));
        }
        if let CaptureScope::Lane { lane } = self.scope {
            if lane as usize >= NUM_LANES {
                return bad(format!("scope lane {lane} out of range"));
            }
        }
        for (i, lane) in self.lanes.iter().enumerate() {
            if !LOOP_LENGTHS.contains(&lane.loop_len) {
                return bad(format!("lanes[{i}].loop_len = {} is not 1, 2, 4 or 8", lane.loop_len));
            }
            for (c, &v) in lane.controls.iter().enumerate() {
                if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                    return bad(format!("lanes[{i}].controls[{c}] = {v} outside 0..=1"));
                }
            }
        }
        Ok(())
    }

    pub fn to_yaml(&self) -> Result<String, CaptureError> {
        serde_yaml::to_string(self).map_err(CaptureError::Serialize)
    }

    /// Save under `<collection>/captures/<style>/<timestamp>.yaml`. If that
    /// name is taken (two captures in one second), `-2`, `-3`, … is appended.
    /// Never overwrites. Returns the path written.
    pub fn save(&self, collection: &Path) -> Result<PathBuf, CaptureError> {
        self.validate()?;
        let dir = captures_dir(collection, &self.style)?;
        std::fs::create_dir_all(&dir).map_err(|source| CaptureError::Io { path: dir.clone(), source })?;
        let text = self.to_yaml()?;
        let mut n = 1u32;
        loop {
            let name = if n == 1 { format!("{}.yaml", self.timestamp) } else { format!("{}-{n}.yaml", self.timestamp) };
            let path = dir.join(name);
            // create_new makes the existence check and the create one step.
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    use std::io::Write;
                    file.write_all(text.as_bytes())
                        .and_then(|_| file.sync_all())
                        .map_err(|source| CaptureError::Io { path: path.clone(), source })?;
                    return Ok(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                Err(source) => return Err(CaptureError::Io { path, source }),
            }
        }
    }

    /// Load and validate one capture file.
    pub fn load(path: &Path) -> Result<Self, CaptureError> {
        let text =
            std::fs::read_to_string(path).map_err(|source| CaptureError::Io { path: path.to_path_buf(), source })?;
        let capture: Capture =
            serde_yaml::from_str(&text).map_err(|source| CaptureError::Parse { path: path.to_path_buf(), source })?;
        capture.validate()?;
        Ok(capture)
    }
}

/// Capture files of a style, oldest first (file names sort by time). A missing
/// folder is an empty list.
pub fn list_captures(collection: &Path, style: &str) -> Result<Vec<PathBuf>, CaptureError> {
    let dir = captures_dir(collection, style)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(CaptureError::Io { path: dir, source }),
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "yaml"))
        .collect();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts: &str) -> Capture {
        let mut lanes = [LaneCapture::default(); NUM_LANES];
        lanes[0].seeds = [50, 50, 51, 77, 50, 50, 51, 78];
        lanes[0].loop_len = 8;
        lanes[1] = LaneCapture { seeds: [1, 2, 3, u32::MAX, 5, 6, 7, 8], loop_len: 2, controls: [0.0, 0.25, 0.5, 1.0], option: 1, on: false };
        Capture::with_timestamp(
            "fixture-breaks",
            ts,
            CaptureScope::All,
            GlobalCapture { key: MusicalKey::new(1, true), progression: 1, tempo: 172.5, half_time: true },
            lanes,
        )
    }

    #[test]
    fn yaml_round_trip() {
        let c = sample("2026-10-13_21-30-05");
        let text = c.to_yaml().unwrap();
        assert!(text.contains("key: C#m"), "{text}");
        let back: Capture = serde_yaml::from_str(&text).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let c = sample("2026-10-13_21-30-05");
        let path = c.save(tmp.path()).unwrap();
        assert_eq!(path, tmp.path().join("captures/fixture-breaks/2026-10-13_21-30-05.yaml"));
        assert_eq!(Capture::load(&path).unwrap(), c);
    }

    #[test]
    fn same_second_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let a = sample("2026-10-13_21-30-05");
        let mut b = a.clone();
        b.lanes[0].seeds[0] = 999;
        let pa = a.save(tmp.path()).unwrap();
        let pb = b.save(tmp.path()).unwrap();
        assert_ne!(pa, pb);
        assert!(pb.ends_with("2026-10-13_21-30-05-2.yaml"));
        assert_eq!(Capture::load(&pa).unwrap(), a);
        assert_eq!(Capture::load(&pb).unwrap(), b);
    }

    #[test]
    fn list_is_chronological_and_per_style() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list_captures(tmp.path(), "fixture-breaks").unwrap().is_empty());
        for ts in ["2026-10-13_22-00-00", "2026-10-13_21-00-00", "2026-10-14_01-00-00"] {
            sample(ts).save(tmp.path()).unwrap();
        }
        let mut other = sample("2026-10-13_20-00-00");
        other.style = "other".into();
        other.save(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("captures/fixture-breaks/notes.txt"), "x").unwrap();

        let names: Vec<String> = list_captures(tmp.path(), "fixture-breaks")
            .unwrap()
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["2026-10-13_21-00-00", "2026-10-13_22-00-00", "2026-10-14_01-00-00"]);
        assert_eq!(list_captures(tmp.path(), "other").unwrap().len(), 1);
    }

    #[test]
    fn lane_scope_round_trip() {
        let mut c = sample("2026-10-13_21-30-05");
        c.scope = CaptureScope::Lane { lane: 2 };
        let back: Capture = serde_yaml::from_str(&c.to_yaml().unwrap()).unwrap();
        assert_eq!(back.scope, CaptureScope::Lane { lane: 2 });
        c.scope = CaptureScope::Lane { lane: 4 };
        assert!(matches!(c.validate(), Err(CaptureError::Invalid(_))));
    }

    #[test]
    fn validation_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = sample("2026-10-13_21-30-05");
        c.style = "../escape".into();
        assert!(matches!(c.save(tmp.path()), Err(CaptureError::InvalidStyleName(_))));
        assert!(matches!(captures_dir(tmp.path(), ""), Err(CaptureError::InvalidStyleName(_))));

        let mut c = sample("2026-10-13_21-30-05");
        c.timestamp = "../../x".into();
        assert!(matches!(c.save(tmp.path()), Err(CaptureError::Invalid(_))));

        let mut c = sample("2026-10-13_21-30-05");
        c.lanes[3].controls[2] = 1.5;
        assert!(matches!(c.validate(), Err(CaptureError::Invalid(_))));

        let mut c = sample("2026-10-13_21-30-05");
        c.version = 3;
        assert!(matches!(c.validate(), Err(CaptureError::Invalid(_))));

        let mut c = sample("2026-10-13_21-30-05");
        c.global.tempo = f64::NAN;
        assert!(matches!(c.validate(), Err(CaptureError::Invalid(_))));
        // Nothing was written by the failed saves.
        assert!(!captures_root(tmp.path()).exists());
    }

    #[test]
    fn load_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.yaml");
        assert!(matches!(Capture::load(&missing), Err(CaptureError::Io { .. })));
        let bad = tmp.path().join("bad.yaml");
        std::fs::write(&bad, "version: 1\nstyle: x\n").unwrap();
        assert!(matches!(Capture::load(&bad), Err(CaptureError::Parse { .. })));
        // Wrong lane count is a parse error (fixed-size array).
        let text = sample("2026-10-13_21-30-05").to_yaml().unwrap();
        let cut = text.rfind("- seeds:").unwrap();
        std::fs::write(&bad, &text[..cut]).unwrap();
        assert!(matches!(Capture::load(&bad), Err(CaptureError::Parse { .. })));
    }

    #[test]
    fn version_1_capture_with_4_seeds_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let lane = |s: u32, on: bool| {
            format!("- seeds: [{s}, {}, {}, {}]\n  controls: [0.5, 0.5, 0.5, 0.5]\n  option: 0\n  on: {on}\n", s + 1, s + 2, s + 3)
        };
        let text = format!(
            "version: 1\nstyle: fixture-breaks\ntimestamp: 2026-10-13_21-30-05\nscope: all\n\
             global:\n  key: Am\n  progression: 0\n  tempo: 172.0\n  half_time: false\nlanes:\n{}{}{}{}",
            lane(10, true),
            lane(20, true),
            lane(30, false),
            lane(40, true)
        );
        let path = tmp.path().join("old.yaml");
        std::fs::write(&path, text).unwrap();
        let c = Capture::load(&path).unwrap();
        assert_eq!(c.version, 1);
        assert_eq!(c.lanes[0].seeds, [10, 11, 12, 13, 10, 11, 12, 13], "4 seeds repeat to fill 8 slots");
        assert!(c.lanes.iter().all(|l| l.loop_len == DEFAULT_LOOP_LEN));
        assert!(!c.lanes[2].on);

        // More than 8 seeds or a loop length outside 1/2/4/8 are rejected.
        let mut bad = sample("2026-10-13_21-30-05");
        bad.lanes[1].loop_len = 3;
        assert!(matches!(bad.validate(), Err(CaptureError::Invalid(_))));
        let nine = text_with_seeds("[1, 2, 3, 4, 5, 6, 7, 8, 9]");
        assert!(serde_yaml::from_str::<Capture>(&nine).is_err());
    }

    fn text_with_seeds(seeds: &str) -> String {
        let text = sample("2026-10-13_21-30-05").to_yaml().unwrap();
        let start = text.find("seeds:").unwrap();
        let end = start + text[start..].find("loop_len").unwrap();
        // Flow-style list replaces the block list of lane 0.
        let indent = &text[text[..end].rfind('\n').unwrap() + 1..end];
        format!("{}seeds: {seeds}\n{indent}{}", &text[..start], &text[end..])
    }

    #[test]
    fn new_stamps_a_valid_timestamp() {
        let c = Capture::new(
            "s",
            CaptureScope::All,
            GlobalCapture { key: MusicalKey::new(0, false), progression: 0, tempo: 170.0, half_time: false },
            [LaneCapture::default(); NUM_LANES],
        );
        c.validate().unwrap();
        assert_eq!(c.version, CAPTURE_VERSION);
    }
}
