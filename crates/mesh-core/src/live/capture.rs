//! Capture snapshots (design D16, §9).
//!
//! A capture stores what is needed to reproduce the current state from seeds:
//! per lane the master seed, the form, the control positions (main and Shift
//! layer), the octave, the drum mutes, the selected instrument option and the
//! on/off state, plus the global key, progression, tempo and half-time flag. Generators are pure functions of these, so a capture
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
use super::NUM_LANES;

/// Folder under the collection root holding all captures.
pub const CAPTURES_FOLDER: &str = "captures";
/// Capture file format version. 3: one master seed and a form per lane,
/// Shift-layer controls, octave and drum mutes. Version 1 and 2 files (slot
/// seeds, loop length) still load: the first slot seed becomes the master
/// seed, everything new starts at its default.
pub const CAPTURE_VERSION: u32 = 3;
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
#[serde(deny_unknown_fields, from = "LaneCaptureFile")]
pub struct LaneCapture {
    /// Master seed.
    pub seed: u32,
    /// Form index.
    pub form: usize,
    /// Position of each performance control, `0..=1`.
    pub controls: [f32; CONTROLS_PER_LANE],
    /// Shift-layer control positions.
    pub shift_controls: [f32; CONTROLS_PER_LANE],
    /// Selected instrument option index.
    pub option: usize,
    /// Lane on/off.
    pub on: bool,
    /// Octave offset of pitched notes.
    pub octave: i8,
    /// Drum lane: muted voices.
    pub mutes: u8,
}

impl Default for LaneCapture {
    fn default() -> Self {
        Self {
            seed: 0,
            form: 0,
            controls: [0.5; CONTROLS_PER_LANE],
            shift_controls: [0.5; CONTROLS_PER_LANE],
            option: 0,
            on: true,
            octave: 0,
            mutes: 0,
        }
    }
}

/// A lane as any capture version stores it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneCaptureFile {
    #[serde(default)]
    seed: Option<u32>,
    /// Versions 1 and 2: slot seeds.
    #[serde(default)]
    seeds: Option<Vec<u32>>,
    /// Versions 1 and 2: loop length (ignored).
    #[serde(default, rename = "loop_len")]
    _loop_len: Option<u8>,
    #[serde(default)]
    form: usize,
    controls: [f32; CONTROLS_PER_LANE],
    #[serde(default = "half_controls")]
    shift_controls: [f32; CONTROLS_PER_LANE],
    option: usize,
    on: bool,
    #[serde(default)]
    octave: i8,
    #[serde(default)]
    mutes: u8,
}

fn half_controls() -> [f32; CONTROLS_PER_LANE] {
    [0.5; CONTROLS_PER_LANE]
}

impl From<LaneCaptureFile> for LaneCapture {
    fn from(f: LaneCaptureFile) -> Self {
        let seed = f.seed.or_else(|| f.seeds.as_ref().and_then(|s| s.first().copied())).unwrap_or(0);
        Self {
            seed,
            form: f.form,
            controls: f.controls,
            shift_controls: f.shift_controls,
            option: f.option,
            on: f.on,
            octave: f.octave,
            mutes: f.mutes,
        }
    }
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
            if !(-2..=2).contains(&lane.octave) {
                return bad(format!("lanes[{i}].octave = {} outside -2..=2", lane.octave));
            }
            for (c, &v) in lane.controls.iter().chain(&lane.shift_controls).enumerate() {
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
        lanes[0].seed = 77;
        lanes[0].mutes = 0b1010_0001;
        lanes[1] = LaneCapture {
            seed: u32::MAX,
            form: 3,
            controls: [0.0, 0.25, 0.5, 1.0],
            shift_controls: [1.0, 0.0, 0.5, 0.75],
            option: 1,
            on: false,
            octave: -2,
            mutes: 0,
        };
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
        b.lanes[0].seed = 999;
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
        c.version = CAPTURE_VERSION + 1;
        assert!(matches!(c.validate(), Err(CaptureError::Invalid(_))));

        let mut c = sample("2026-10-13_21-30-05");
        c.lanes[1].octave = 3;
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
        let cut = text.rfind("- seed:").unwrap();
        std::fs::write(&bad, &text[..cut]).unwrap();
        assert!(matches!(Capture::load(&bad), Err(CaptureError::Parse { .. })));
    }

    #[test]
    fn version_1_and_2_captures_load_with_the_first_seed() {
        let tmp = tempfile::tempdir().unwrap();
        let lane = |s: u32, on: bool, extra: &str| {
            format!(
                "- seeds: [{s}, {}, {}, {}]\n{extra}  controls: [0.5, 0.5, 0.5, 0.5]\n  option: 0\n  on: {on}\n",
                s + 1,
                s + 2,
                s + 3
            )
        };
        for (version, extra) in [(1, ""), (2, "  loop_len: 8\n")] {
            let text = format!(
                "version: {version}\nstyle: fixture-breaks\ntimestamp: 2026-10-13_21-30-05\nscope: all\n\
                 global:\n  key: Am\n  progression: 0\n  tempo: 172.0\n  half_time: false\nlanes:\n{}{}{}{}",
                lane(10, true, extra),
                lane(20, true, extra),
                lane(30, false, extra),
                lane(40, true, extra)
            );
            let path = tmp.path().join(format!("old-{version}.yaml"));
            std::fs::write(&path, text).unwrap();
            let c = Capture::load(&path).unwrap();
            assert_eq!(c.version, version);
            assert_eq!(c.lanes[0].seed, 10, "first slot seed becomes the master seed");
            assert_eq!((c.lanes[1].form, c.lanes[1].octave, c.lanes[0].mutes), (0, 0, 0));
            assert_eq!(c.lanes[1].shift_controls, [0.5; CONTROLS_PER_LANE]);
            assert!(!c.lanes[2].on);
        }
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
