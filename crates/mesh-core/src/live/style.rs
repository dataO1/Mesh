//! Style bundle schema, loader and validation (design §9, D10/D11, D29).
//!
//! A style is prepared sound design only: instruments, FX chains, control
//! mappings, ranges, archetype order, progressions, glue parameters and LUFS
//! trims. It never holds an arrangement.
//!
//! ```text
//! <collection>/styles/<name>/
//! ├── style.yaml
//! └── assets/        samples, breaks, stem phrases, CLAP presets
//! ```
//!
//! Every asset path in `style.yaml` is relative to the style folder and may not
//! leave it, so a style can be copied to USB as one self-contained folder.
//! The annotated reference for every field is `docs/live-style-reference.yaml`.
//!
//! Loading and validation run on the loader thread (non-RT). The only RT-safe
//! helpers here are [`Curve::apply`] and [`ControlSpec::map`].

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::music::MusicalKey;

use super::clock::{MAX_BPM, MIN_BPM};
use super::harmony::{serde_key, Progression, MAX_PROGRESSION_LEN};
use super::{NUM_LANES, STEPS_PER_BAR};

/// Schema version this build reads.
pub const STYLE_SCHEMA_VERSION: u32 = 1;
/// Style file name inside a style folder.
pub const STYLE_FILE: &str = "style.yaml";
/// Folder under the collection root holding all styles.
pub const STYLES_FOLDER: &str = "styles";
/// Number of performance controls per lane (design D34).
pub const CONTROLS_PER_LANE: usize = 4;
/// Accepted range for per-option LUFS trims, in dB.
pub const TRIM_RANGE_DB: (f32, f32) = (-48.0, 24.0);

// MUSICAL: per-role loudness targets (design §5.5), overridable per style.
pub const DEFAULT_LUFS_DRUMS: f32 = -12.0;
pub const DEFAULT_LUFS_BASS: f32 = -14.0;
pub const DEFAULT_LUFS_LEAD: f32 = -16.0;
pub const DEFAULT_LUFS_EXTRA: f32 = -18.0;

/// `<collection>/styles`.
pub fn styles_dir(collection: &Path) -> PathBuf {
    collection.join(STYLES_FOLDER)
}

/// `<collection>/styles/<name>`.
pub fn style_dir(collection: &Path, name: &str) -> PathBuf {
    styles_dir(collection).join(name)
}

/// A style or folder name that is safe to use as one path component.
pub fn is_valid_style_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
}

// ─── Schema ────────────────────────────────────────────────────────────

/// Top level of `style.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleSpec {
    /// Display name. Usually equals the folder name.
    pub name: String,
    /// Schema version, see [`STYLE_SCHEMA_VERSION`].
    pub version: u32,
    pub tempo: TempoSpec,
    /// Starting key as a name ("Am") or Camelot label ("8A").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_key: Option<String>,
    /// Chord progressions, selectable on the Shift layer (D30).
    pub progressions: Vec<ProgressionSpec>,
    /// Index into `progressions` used at load.
    #[serde(default)]
    pub default_progression: usize,
    /// Exactly [`NUM_LANES`] lanes. Lane 0 is the drum lane.
    pub lanes: Vec<LaneSpec>,
    /// Per-role loudness targets.
    #[serde(default)]
    pub loudness: LoudnessTargets,
    /// Glue chain parameters (design §5.6). All optional.
    #[serde(default)]
    pub glue: GlueSpec,
    /// Effects used for the shared delay and reverb sends (design §5.4).
    #[serde(default)]
    pub send_fx: SendFxSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TempoSpec {
    pub default: f64,
    pub min: f64,
    pub max: f64,
}

pub use super::glue::Role;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSpec {
    /// Display name in the code readout.
    pub name: String,
    pub role: Role,
    /// Instrument options (non-drum lanes). Browsed with lane button + encoder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<InstrumentOption>,
    /// Drum sub-voices (drum lane only), each with its own options.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_voices: Vec<SubVoiceSpec>,
    /// Option loaded at style load (non-drum lanes).
    #[serde(default)]
    pub default_option: usize,
    /// Lane FX chain, in processing order.
    #[serde(default)]
    pub fx: Vec<FxSpec>,
    /// Drum lane only: FX chain of the break sub-voice, before `fx`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub break_fx: Vec<FxSpec>,
    /// Exactly [`CONTROLS_PER_LANE`] performance controls.
    pub controls: Vec<ControlSpec>,
    pub generator: GeneratorSpec,
    /// Default send levels.
    #[serde(default)]
    pub sends: SendLevels,
    /// Drum lane only: backbone archetypes in major-style control order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub archetypes: Vec<String>,
}

/// One selectable instrument. Exactly one of `sampler` / `clap` is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentOption {
    pub name: String,
    /// Static gain from the LUFS calibration tool, in dB.
    #[serde(default)]
    pub lufs_trim_db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampler: Option<SamplerSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clap: Option<ClapSpec>,
}

/// Borrowed view of an option's instrument source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InstrumentSource<'a> {
    Sampler(&'a SamplerSpec),
    Clap(&'a ClapSpec),
}

impl InstrumentOption {
    /// The instrument source, or `None` if the option is invalid (neither or
    /// both set; validation reports it).
    pub fn source(&self) -> Option<InstrumentSource<'_>> {
        match (&self.sampler, &self.clap) {
            (Some(s), None) => Some(InstrumentSource::Sampler(s)),
            (None, Some(c)) => Some(InstrumentSource::Clap(c)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplerSpec {
    /// Sample slots. Event `target` indexes this list.
    pub samples: Vec<SampleSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleSpec {
    /// Path relative to the style folder.
    pub file: PathBuf,
    /// Pitch the sample was recorded at. `None` = unpitched (drums).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_key: Option<MidiNote>,
    /// Lane-local choke group, 0 = none.
    #[serde(default)]
    pub choke: u8,
    /// Slice markers (breaks, stem phrases), ascending by `start`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slices: Vec<SliceSpec>,
    /// Multisample zone. Unset = the sample covers every key and velocity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<ZoneSpec>,
}

/// One slice marker.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceSpec {
    /// Start position in frames at the file's own sample rate.
    pub start: u64,
    #[serde(rename = "type")]
    pub kind: SliceType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceType {
    Kick,
    Snare,
    Ghost,
    Hat,
    Perc,
    Other,
}

/// Key and velocity range a multisample covers (inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneSpec {
    pub keys: [MidiNote; 2],
    #[serde(default = "full_velocity")]
    pub velocities: [u8; 2],
}

fn full_velocity() -> [u8; 2] {
    [0, 127]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClapSpec {
    /// CLAP plugin id, as reported by discovery.
    pub plugin_id: String,
    /// Optional preset / state file, relative to the style folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<PathBuf>,
}

/// Drum lane sub-voice kind (design §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubVoiceKind {
    Backbone,
    Break,
    Hats,
    Perc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubVoiceSpec {
    pub voice: SubVoiceKind,
    /// Default choke group for this sub-voice's samples, 0 = none.
    #[serde(default)]
    pub choke: u8,
    pub options: Vec<InstrumentOption>,
    #[serde(default)]
    pub default_option: usize,
}

/// Effect source, same identifiers as Mesh's FX presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FxSource {
    Native,
    Clap,
    Pd,
    /// An Airwindows Consolidated algorithm; `id` is its name ("Pop2").
    /// Hosted through the `com.airwindows.consolidated` CLAP plugin.
    Airwindows,
}

/// One effect in a chain. `id` is the plugin id (CLAP), patch folder (PD) or
/// native effect id, as in `EffectBuildSpec::plugin_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FxSpec {
    pub id: String,
    pub source: FxSource,
    /// Normalized (0..1) parameter values by parameter index. Unlisted
    /// parameters keep the plugin default.
    #[serde(default)]
    pub params: BTreeMap<u32, f32>,
    #[serde(default)]
    pub bypassed: bool,
    #[serde(default = "one")]
    pub dry_wet: f32,
    /// CLAP state file (relative to the style folder), loaded before
    /// `params`. VST2 `.fxp` chunk presets work too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<PathBuf>,
}

/// Response curve of a control mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    #[default]
    Linear,
    /// x², fine control at the low end (cutoffs, sends).
    Exp,
    /// 1 − (1 − x)², fine control at the high end.
    Log,
    /// Smoothstep, fine control at both ends.
    S,
}

impl Curve {
    /// Shape a position in `[0, 1]` (clamped). RT-safe.
    #[inline]
    pub fn apply(self, x: f32) -> f32 {
        let x = if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 };
        match self {
            Curve::Linear => x,
            Curve::Exp => x * x,
            Curve::Log => 1.0 - (1.0 - x) * (1.0 - x),
            Curve::S => x * x * (3.0 - 2.0 * x),
        }
    }
}

/// Shared send bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendBus {
    Delay,
    Reverb,
}

/// What a performance control drives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlTarget {
    /// Generator parameter by name. Changes re-render the next bar.
    Generator { param: String },
    /// Instrument parameter by index (CLAP param index / sampler param).
    Instrument { param: u32 },
    /// Lane FX chain slot + parameter index.
    Fx { slot: u32, param: u32 },
    /// Glue parameter by name.
    Glue { param: String },
    /// Lane send level.
    Send { bus: SendBus },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlSpec {
    /// Short label for the code readout. Defaults to the target's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub target: ControlTarget,
    /// Value at control position 0.
    #[serde(default)]
    pub min: f32,
    /// Value at control position 1. May be below `min` (inverted).
    #[serde(default = "one")]
    pub max: f32,
    #[serde(default)]
    pub curve: Curve,
    /// Control position at load, `0..=1`.
    #[serde(default = "half")]
    pub default: f32,
}

impl ControlSpec {
    /// Map a control position in `[0, 1]` to the target value. RT-safe.
    #[inline]
    pub fn map(&self, position: f32) -> f32 {
        self.min + (self.max - self.min) * self.curve.apply(position)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratorFamily {
    /// On-the-fly procedural generation (default, D42).
    Procedural,
    /// Curated idiom data (fallback).
    Idiom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorSpec {
    pub family: GeneratorFamily,
    /// Generator implementation name.
    pub name: String,
    /// Default parameter values by name (normalized unless the generator
    /// documents otherwise).
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
    /// Idiom data, required for the idiom family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idiom: Option<IdiomSpec>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdiomSpec {
    /// Grids-style per-step importance maps.
    #[serde(default)]
    pub level_maps: Vec<LevelMapSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelMapSpec {
    pub name: String,
    /// Archetype this map belongs to (drum lane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archetype: Option<String>,
    /// Voice or sub-voice the map drives (e.g. "kick", "hats").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// One level (0..=255) per step, a multiple of 16 steps long.
    pub levels: Vec<u8>,
}

/// A progression in 1-based scale degrees as written by hand ("i-VI-iv-v" =
/// `[1, 6, 4, 5]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressionSpec {
    pub name: String,
    pub degrees: Vec<u8>,
    #[serde(default = "one_u8")]
    pub bars_per_chord: u8,
}

impl ProgressionSpec {
    /// Runtime form (0-based degrees). `None` when invalid.
    pub fn to_progression(&self) -> Option<Progression> {
        if self.degrees.iter().any(|&d| d == 0) {
            return None;
        }
        let zero_based: Vec<u8> = self.degrees.iter().map(|d| d - 1).collect();
        Progression::new(&zero_based, self.bars_per_chord)
    }
}

/// Integrated-loudness targets per role, in LUFS.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoudnessTargets {
    #[serde(default = "lufs_drums")]
    pub drums: f32,
    #[serde(default = "lufs_bass")]
    pub bass: f32,
    #[serde(default = "lufs_lead")]
    pub lead: f32,
    #[serde(default = "lufs_extra")]
    pub extra: f32,
}

impl Default for LoudnessTargets {
    fn default() -> Self {
        Self {
            drums: DEFAULT_LUFS_DRUMS,
            bass: DEFAULT_LUFS_BASS,
            lead: DEFAULT_LUFS_LEAD,
            extra: DEFAULT_LUFS_EXTRA,
        }
    }
}

impl LoudnessTargets {
    pub fn target(&self, role: Role) -> f32 {
        match role {
            Role::Drums => self.drums,
            Role::Bass => self.bass,
            Role::Lead => self.lead,
            Role::Extra => self.extra,
        }
    }
}

/// Default send levels of a lane, `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendLevels {
    #[serde(default)]
    pub delay: f32,
    #[serde(default)]
    pub reverb: f32,
}

/// Effects for the shared sends. Unset = engine default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendFxSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<FxSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverb: Option<FxSpec>,
}

/// Glue chain parameters (design §5.6). Every field is optional; unset fields
/// keep the glue runtime's defaults. This schema is independent of the glue
/// runtime so either side can evolve.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlueSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_hpf: Option<RoleHpfSpec>,
    /// Bass is summed to mono below this frequency (Hz).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bass_mono_hz: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duck: Option<DuckSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drum_bus: Option<DrumBusSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_drive: Option<SharedDriveSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master: Option<CompSpec>,
    /// FX chain at the end of the drum bus (after its compressors and saturation).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drum_bus_fx: Vec<FxSpec>,
    /// FX chain on the master, after its compressor and saturation, before the limiter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub master_fx: Vec<FxSpec>,
}

/// High-pass cutoff per non-bass role (Hz).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleHpfSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drums: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<f32>,
}

/// Full-band duck on bass, lead and extra, keyed by the main drum kit's
/// kick and snare. Unset fields keep the glue defaults.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuckSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack_ms: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bass: Option<DuckGroupSpec>,
    /// Lead and extra.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub others: Option<DuckGroupSpec>,
}

/// Duck of one group of lanes: cut in dB on a main-kit kick and snare.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuckGroupSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kick_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snare_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ms: Option<f32>,
}

/// Compressor settings.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack_ms: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ms: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub makeup_db: Option<f32>,
}

/// Drum bus: compressor plus saturation.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrumBusSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comp: Option<CompSpec>,
    /// Saturation drive, `0..=1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<f32>,
}

/// One saturation stage shared by the drum and bass buses.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedDriveSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Drive amount, `0..=1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<f32>,
    /// Wet mix, `0..=1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix: Option<f32>,
}

/// A MIDI note number, written in YAML as a number (`45`) or a name (`"A2"`,
/// `"C#4"`, `"Bb1"`, with C4 = 60).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "NoteRepr", into = "NoteRepr")]
pub struct MidiNote(pub u8);

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum NoteRepr {
    Number(i64),
    Name(String),
}

impl TryFrom<NoteRepr> for MidiNote {
    type Error = String;

    fn try_from(r: NoteRepr) -> Result<Self, String> {
        match r {
            NoteRepr::Number(n) if (0..=127).contains(&n) => Ok(MidiNote(n as u8)),
            NoteRepr::Number(n) => Err(format!("MIDI note {n} out of range 0..=127")),
            NoteRepr::Name(s) => parse_note_name(&s).map(MidiNote).ok_or_else(|| format!("invalid note name '{s}'")),
        }
    }
}

impl From<MidiNote> for NoteRepr {
    fn from(n: MidiNote) -> Self {
        NoteRepr::Number(n.0 as i64)
    }
}

/// Parse "A2", "C#4", "Bb-1" into a MIDI note number (C4 = 60).
pub fn parse_note_name(s: &str) -> Option<u8> {
    let s = s.trim();
    let mut chars = s.char_indices();
    let (_, letter) = chars.next()?;
    let base: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let mut rest = &s[letter.len_utf8()..];
    let mut pc = base;
    if let Some(r) = rest.strip_prefix('#') {
        pc += 1;
        rest = r;
    } else if let Some(r) = rest.strip_prefix('b') {
        pc -= 1;
        rest = r;
    }
    let octave: i32 = rest.parse().ok()?;
    let midi = (octave + 1) * 12 + pc;
    (0..=127).contains(&midi).then_some(midi as u8)
}

fn one() -> f32 {
    1.0
}
fn half() -> f32 {
    0.5
}
fn one_u8() -> u8 {
    1
}
fn lufs_drums() -> f32 {
    DEFAULT_LUFS_DRUMS
}
fn lufs_bass() -> f32 {
    DEFAULT_LUFS_BASS
}
fn lufs_lead() -> f32 {
    DEFAULT_LUFS_LEAD
}
fn lufs_extra() -> f32 {
    DEFAULT_LUFS_EXTRA
}

// ─── Errors ────────────────────────────────────────────────────────────

/// One validation finding, located by a field path such as
/// `lanes[2].controls[1].target`.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleIssue {
    pub at: String,
    pub problem: Problem,
}

impl fmt::Display for StyleIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.problem)
    }
}

/// What is wrong with a style field.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Problem {
    #[error("unsupported schema version {found} (this build reads {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("must not be empty")]
    Empty,
    #[error("style name '{0}' is not a valid folder name")]
    InvalidName(String),
    #[error("expected exactly {expected} lanes, found {found}")]
    LaneCount { expected: usize, found: usize },
    #[error("lane 0 must have role drums")]
    FirstLaneNotDrums,
    #[error("only lane 0 may have role drums")]
    ExtraDrumLane,
    #[error("tempo must satisfy {} <= min <= default <= max <= {}", MIN_BPM, MAX_BPM)]
    TempoRange,
    #[error("drum lane uses sub_voices, not options")]
    OptionsOnDrumLane,
    #[error("only the drum lane has sub_voices")]
    SubVoicesOnInstrumentLane,
    #[error("only the drum lane has a break_fx chain")]
    BreakFxOnInstrumentLane,
    #[error("index {index} out of range (len {len})")]
    IndexOutOfRange { index: usize, len: usize },
    #[error("exactly one of sampler / clap must be set")]
    SourceCount,
    #[error("asset not found: {0}")]
    AssetMissing(PathBuf),
    #[error("asset path must be relative to the style folder: {0}")]
    AssetPathAbsolute(PathBuf),
    #[error("asset path must stay inside the style folder: {0}")]
    AssetPathEscapes(PathBuf),
    #[error("slice starts must be strictly ascending")]
    SlicesUnordered,
    #[error("range [{lo}, {hi}] must satisfy lo <= hi <= 127")]
    BadRange { lo: u8, hi: u8 },
    #[error("expected exactly {expected} controls, found {found}")]
    ControlCount { expected: usize, found: usize },
    #[error("value {0} must be finite")]
    NotFinite(f32),
    #[error("value {value} outside {min}..={max}")]
    OutOfRange { value: f32, min: f32, max: f32 },
    #[error("scale degree {0} outside 1..=7")]
    DegreeOutOfRange(u8),
    #[error("progression longer than {} chords", MAX_PROGRESSION_LEN)]
    ProgressionTooLong,
    #[error("bars_per_chord must be >= 1")]
    BarsPerChord,
    #[error("drum lane needs at least one archetype")]
    NoArchetypes,
    #[error("only the drum lane has archetypes")]
    ArchetypesOnInstrumentLane,
    #[error("level map length {0} is not a non-zero multiple of 16 steps")]
    LevelMapLength(usize),
    #[error("idiom family needs idiom data")]
    IdiomDataMissing,
    #[error("invalid key '{0}'")]
    InvalidKey(String),
}

#[derive(Debug, thiserror::Error)]
pub enum StyleError {
    #[error("invalid style name '{0}'")]
    InvalidName(String),
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("invalid style {}:\n{}", path.display(), list_issues(issues))]
    Invalid { path: PathBuf, issues: Vec<StyleIssue> },
}

fn list_issues(issues: &[StyleIssue]) -> String {
    issues.iter().map(|i| format!("  - {i}")).collect::<Vec<_>>().join("\n")
}

// ─── Loaded style ─────────────────────────────────────────────────────

/// A parsed and validated style with its folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    /// Style folder; asset paths resolve against it.
    pub dir: PathBuf,
    pub spec: StyleSpec,
}

impl Style {
    /// Load `<dir>/style.yaml` and validate it against the real filesystem.
    pub fn load(dir: &Path) -> Result<Self, StyleError> {
        Self::load_with(dir, &|p: &Path| p.is_file())
    }

    /// Load `<collection>/styles/<name>`.
    pub fn load_named(collection: &Path, name: &str) -> Result<Self, StyleError> {
        if !is_valid_style_name(name) {
            return Err(StyleError::InvalidName(name.to_string()));
        }
        Self::load(&style_dir(collection, name))
    }

    /// Load with a custom asset-existence check (tests, export dry runs).
    pub fn load_with(dir: &Path, exists: &dyn Fn(&Path) -> bool) -> Result<Self, StyleError> {
        let path = dir.join(STYLE_FILE);
        let text = std::fs::read_to_string(&path).map_err(|source| StyleError::Io { path: path.clone(), source })?;
        let spec = StyleSpec::from_yaml(&text).map_err(|source| StyleError::Parse { path: path.clone(), source })?;
        let issues = spec.validate(dir, exists);
        if !issues.is_empty() {
            return Err(StyleError::Invalid { path, issues });
        }
        Ok(Self { dir: dir.to_path_buf(), spec })
    }

    /// Resolve a style-relative asset path.
    pub fn asset_path(&self, rel: &Path) -> PathBuf {
        self.dir.join(rel)
    }

    /// Absolute paths of every asset the style references, deduplicated, in
    /// first-reference order (preload list).
    pub fn asset_files(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        self.spec.for_each_asset(&mut |_, rel| {
            let p = self.dir.join(rel);
            if !out.contains(&p) {
                out.push(p);
            }
        });
        out
    }

    /// Runtime progressions (validated, so none are dropped).
    pub fn progressions(&self) -> Vec<Progression> {
        self.spec.progressions.iter().filter_map(ProgressionSpec::to_progression).collect()
    }

    /// Starting key, if the style sets one.
    pub fn default_key(&self) -> Option<MusicalKey> {
        self.spec.default_key.as_deref().and_then(serde_key::parse)
    }
}

impl StyleSpec {
    pub fn from_yaml(text: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(text)
    }

    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }

    /// Visit every asset path with its field location.
    pub fn for_each_asset(&self, f: &mut dyn FnMut(String, &Path)) {
        let mut fx_assets = |at: String, chain: &[FxSpec]| {
            for (i, fx) in chain.iter().enumerate() {
                if let Some(p) = &fx.preset {
                    f(format!("{at}[{i}].preset"), p);
                }
            }
        };
        for (li, lane) in self.lanes.iter().enumerate() {
            fx_assets(format!("lanes[{li}].fx"), &lane.fx);
            fx_assets(format!("lanes[{li}].break_fx"), &lane.break_fx);
        }
        fx_assets("glue.drum_bus_fx".into(), &self.glue.drum_bus_fx);
        fx_assets("glue.master_fx".into(), &self.glue.master_fx);
        for (li, lane) in self.lanes.iter().enumerate() {
            for (oi, opt) in lane.options.iter().enumerate() {
                option_assets(&format!("lanes[{li}].options[{oi}]"), opt, f);
            }
            for (vi, sv) in lane.sub_voices.iter().enumerate() {
                for (oi, opt) in sv.options.iter().enumerate() {
                    option_assets(&format!("lanes[{li}].sub_voices[{vi}].options[{oi}]"), opt, f);
                }
            }
        }
    }

    /// Validate everything. `dir` is the style folder; `exists` checks a
    /// resolved asset path. Returns all issues found (empty = valid).
    pub fn validate(&self, dir: &Path, exists: &dyn Fn(&Path) -> bool) -> Vec<StyleIssue> {
        let mut v = Validator { issues: Vec::new() };
        v.style(self);
        self.for_each_asset(&mut |at, rel| {
            if rel.as_os_str().is_empty() {
                v.push(at, Problem::Empty);
            } else if rel.is_absolute() || rel.has_root() {
                v.push(at, Problem::AssetPathAbsolute(rel.to_path_buf()));
            } else if rel.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
                v.push(at, Problem::AssetPathEscapes(rel.to_path_buf()));
            } else if !exists(&dir.join(rel)) {
                v.push(at, Problem::AssetMissing(rel.to_path_buf()));
            }
        });
        v.issues
    }
}

fn option_assets(at: &str, opt: &InstrumentOption, f: &mut dyn FnMut(String, &Path)) {
    if let Some(s) = &opt.sampler {
        for (si, sample) in s.samples.iter().enumerate() {
            f(format!("{at}.sampler.samples[{si}].file"), &sample.file);
        }
    }
    if let Some(c) = &opt.clap {
        if let Some(p) = &c.preset {
            f(format!("{at}.clap.preset"), p);
        }
    }
}

// ─── Validation ───────────────────────────────────────────────────────

struct Validator {
    issues: Vec<StyleIssue>,
}

impl Validator {
    fn push(&mut self, at: impl Into<String>, problem: Problem) {
        self.issues.push(StyleIssue { at: at.into(), problem });
    }

    fn non_empty(&mut self, at: impl Into<String>, s: &str) {
        if s.trim().is_empty() {
            self.push(at, Problem::Empty);
        }
    }

    fn in_range(&mut self, at: impl Into<String>, value: f32, min: f32, max: f32) {
        if !value.is_finite() {
            self.push(at, Problem::NotFinite(value));
        } else if value < min || value > max {
            self.push(at, Problem::OutOfRange { value, min, max });
        }
    }

    fn opt_range(&mut self, at: impl Into<String>, value: Option<f32>, min: f32, max: f32) {
        if let Some(v) = value {
            self.in_range(at, v, min, max);
        }
    }

    fn index(&mut self, at: impl Into<String>, index: usize, len: usize) {
        if index >= len {
            self.push(at, Problem::IndexOutOfRange { index, len });
        }
    }

    fn style(&mut self, s: &StyleSpec) {
        if s.version == 0 || s.version > STYLE_SCHEMA_VERSION {
            self.push("version", Problem::UnsupportedVersion { found: s.version, supported: STYLE_SCHEMA_VERSION });
        }
        if s.name.trim().is_empty() {
            self.push("name", Problem::Empty);
        } else if !is_valid_style_name(&s.name) {
            self.push("name", Problem::InvalidName(s.name.clone()));
        }
        let t = s.tempo;
        let ok = [t.min, t.default, t.max].iter().all(|v| v.is_finite())
            && MIN_BPM <= t.min
            && t.min <= t.default
            && t.default <= t.max
            && t.max <= MAX_BPM;
        if !ok {
            self.push("tempo", Problem::TempoRange);
        }
        if let Some(k) = &s.default_key {
            if serde_key::parse(k).is_none() {
                self.push("default_key", Problem::InvalidKey(k.clone()));
            }
        }

        if s.progressions.is_empty() {
            self.push("progressions", Problem::Empty);
        }
        for (i, p) in s.progressions.iter().enumerate() {
            self.progression(&format!("progressions[{i}]"), p);
        }
        if !s.progressions.is_empty() {
            self.index("default_progression", s.default_progression, s.progressions.len());
        }

        if s.lanes.len() != NUM_LANES {
            self.push("lanes", Problem::LaneCount { expected: NUM_LANES, found: s.lanes.len() });
        }
        if let Some(first) = s.lanes.first() {
            if first.role != Role::Drums {
                self.push("lanes[0].role", Problem::FirstLaneNotDrums);
            }
        }
        for (i, lane) in s.lanes.iter().enumerate() {
            if i > 0 && lane.role == Role::Drums {
                self.push(format!("lanes[{i}].role"), Problem::ExtraDrumLane);
            }
            self.lane(&format!("lanes[{i}]"), lane);
        }

        let l = s.loudness;
        for (name, v) in [("drums", l.drums), ("bass", l.bass), ("lead", l.lead), ("extra", l.extra)] {
            self.in_range(format!("loudness.{name}"), v, -60.0, 0.0);
        }
        self.glue(&s.glue);
        for (i, fx) in s.glue.drum_bus_fx.iter().enumerate() {
            self.fx(&format!("glue.drum_bus_fx[{i}]"), fx);
        }
        for (i, fx) in s.glue.master_fx.iter().enumerate() {
            self.fx(&format!("glue.master_fx[{i}]"), fx);
        }
        if let Some(fx) = &s.send_fx.delay {
            self.fx("send_fx.delay", fx);
        }
        if let Some(fx) = &s.send_fx.reverb {
            self.fx("send_fx.reverb", fx);
        }
    }

    fn progression(&mut self, at: &str, p: &ProgressionSpec) {
        self.non_empty(format!("{at}.name"), &p.name);
        if p.degrees.is_empty() {
            self.push(format!("{at}.degrees"), Problem::Empty);
        } else if p.degrees.len() > MAX_PROGRESSION_LEN {
            self.push(format!("{at}.degrees"), Problem::ProgressionTooLong);
        }
        for (i, &d) in p.degrees.iter().enumerate() {
            if !(1..=7).contains(&d) {
                self.push(format!("{at}.degrees[{i}]"), Problem::DegreeOutOfRange(d));
            }
        }
        if p.bars_per_chord == 0 {
            self.push(format!("{at}.bars_per_chord"), Problem::BarsPerChord);
        }
    }

    fn lane(&mut self, at: &str, lane: &LaneSpec) {
        self.non_empty(format!("{at}.name"), &lane.name);
        if lane.role == Role::Drums {
            if !lane.options.is_empty() {
                self.push(format!("{at}.options"), Problem::OptionsOnDrumLane);
            }
            if lane.sub_voices.is_empty() {
                self.push(format!("{at}.sub_voices"), Problem::Empty);
            }
            for (i, sv) in lane.sub_voices.iter().enumerate() {
                let sat = format!("{at}.sub_voices[{i}]");
                if sv.options.is_empty() {
                    self.push(format!("{sat}.options"), Problem::Empty);
                } else {
                    self.index(format!("{sat}.default_option"), sv.default_option, sv.options.len());
                }
                for (oi, opt) in sv.options.iter().enumerate() {
                    self.option(&format!("{sat}.options[{oi}]"), opt);
                }
            }
            if lane.archetypes.is_empty() {
                self.push(format!("{at}.archetypes"), Problem::NoArchetypes);
            }
            for (i, a) in lane.archetypes.iter().enumerate() {
                self.non_empty(format!("{at}.archetypes[{i}]"), a);
            }
        } else {
            if !lane.sub_voices.is_empty() {
                self.push(format!("{at}.sub_voices"), Problem::SubVoicesOnInstrumentLane);
            }
            if !lane.archetypes.is_empty() {
                self.push(format!("{at}.archetypes"), Problem::ArchetypesOnInstrumentLane);
            }
            if lane.options.is_empty() {
                self.push(format!("{at}.options"), Problem::Empty);
            } else {
                self.index(format!("{at}.default_option"), lane.default_option, lane.options.len());
            }
            for (oi, opt) in lane.options.iter().enumerate() {
                self.option(&format!("{at}.options[{oi}]"), opt);
            }
        }

        for (i, fx) in lane.fx.iter().enumerate() {
            self.fx(&format!("{at}.fx[{i}]"), fx);
        }
        for (i, fx) in lane.break_fx.iter().enumerate() {
            self.fx(&format!("{at}.break_fx[{i}]"), fx);
        }
        if !lane.break_fx.is_empty() && lane.role != Role::Drums {
            self.push(format!("{at}.break_fx"), Problem::BreakFxOnInstrumentLane);
        }
        if lane.controls.len() != CONTROLS_PER_LANE {
            self.push(
                format!("{at}.controls"),
                Problem::ControlCount { expected: CONTROLS_PER_LANE, found: lane.controls.len() },
            );
        }
        for (i, c) in lane.controls.iter().enumerate() {
            self.control(&format!("{at}.controls[{i}]"), c, lane.fx.len());
        }
        self.generator(&format!("{at}.generator"), &lane.generator);
        self.in_range(format!("{at}.sends.delay"), lane.sends.delay, 0.0, 1.0);
        self.in_range(format!("{at}.sends.reverb"), lane.sends.reverb, 0.0, 1.0);
    }

    fn option(&mut self, at: &str, opt: &InstrumentOption) {
        self.non_empty(format!("{at}.name"), &opt.name);
        self.in_range(format!("{at}.lufs_trim_db"), opt.lufs_trim_db, TRIM_RANGE_DB.0, TRIM_RANGE_DB.1);
        match opt.source() {
            None => self.push(at.to_string(), Problem::SourceCount),
            Some(InstrumentSource::Clap(c)) => self.non_empty(format!("{at}.clap.plugin_id"), &c.plugin_id),
            Some(InstrumentSource::Sampler(s)) => {
                if s.samples.is_empty() {
                    self.push(format!("{at}.sampler.samples"), Problem::Empty);
                }
                for (i, sample) in s.samples.iter().enumerate() {
                    let sat = format!("{at}.sampler.samples[{i}]");
                    if sample.slices.windows(2).any(|w| w[0].start >= w[1].start) {
                        self.push(format!("{sat}.slices"), Problem::SlicesUnordered);
                    }
                    if let Some(z) = sample.zone {
                        let (lo, hi) = (z.keys[0].0, z.keys[1].0);
                        if lo > hi {
                            self.push(format!("{sat}.zone.keys"), Problem::BadRange { lo, hi });
                        }
                        let (vlo, vhi) = (z.velocities[0], z.velocities[1]);
                        if vlo > vhi || vhi > 127 {
                            self.push(format!("{sat}.zone.velocities"), Problem::BadRange { lo: vlo, hi: vhi });
                        }
                    }
                }
            }
        }
    }

    fn fx(&mut self, at: &str, fx: &FxSpec) {
        self.non_empty(format!("{at}.id"), &fx.id);
        for (idx, &v) in &fx.params {
            self.in_range(format!("{at}.params.{idx}"), v, 0.0, 1.0);
        }
        self.in_range(format!("{at}.dry_wet"), fx.dry_wet, 0.0, 1.0);
    }

    fn control(&mut self, at: &str, c: &ControlSpec, fx_len: usize) {
        for (name, v) in [("min", c.min), ("max", c.max)] {
            if !v.is_finite() {
                self.push(format!("{at}.{name}"), Problem::NotFinite(v));
            }
        }
        self.in_range(format!("{at}.default"), c.default, 0.0, 1.0);
        let tat = format!("{at}.target");
        match &c.target {
            ControlTarget::Generator { param } | ControlTarget::Glue { param } => self.non_empty(tat, param),
            ControlTarget::Fx { slot, .. } => self.index(format!("{tat}.slot"), *slot as usize, fx_len),
            ControlTarget::Instrument { .. } => {}
            ControlTarget::Send { .. } => {
                self.in_range(format!("{at}.min"), c.min, 0.0, 1.0);
                self.in_range(format!("{at}.max"), c.max, 0.0, 1.0);
            }
        }
    }

    fn generator(&mut self, at: &str, g: &GeneratorSpec) {
        self.non_empty(format!("{at}.name"), &g.name);
        for (name, &v) in &g.params {
            self.non_empty(format!("{at}.params"), name);
            if !v.is_finite() {
                self.push(format!("{at}.params.{name}"), Problem::NotFinite(v));
            }
        }
        if g.family == GeneratorFamily::Idiom && g.idiom.as_ref().is_none_or(|i| i.level_maps.is_empty()) {
            self.push(format!("{at}.idiom"), Problem::IdiomDataMissing);
        }
        if let Some(idiom) = &g.idiom {
            for (i, m) in idiom.level_maps.iter().enumerate() {
                let mat = format!("{at}.idiom.level_maps[{i}]");
                self.non_empty(format!("{mat}.name"), &m.name);
                if m.levels.is_empty() || m.levels.len() % STEPS_PER_BAR != 0 {
                    self.push(format!("{mat}.levels"), Problem::LevelMapLength(m.levels.len()));
                }
            }
        }
    }

    fn glue(&mut self, g: &GlueSpec) {
        const HZ: (f32, f32) = (10.0, 20_000.0);
        if let Some(h) = &g.role_hpf {
            self.opt_range("glue.role_hpf.drums", h.drums, HZ.0, HZ.1);
            self.opt_range("glue.role_hpf.lead", h.lead, HZ.0, HZ.1);
            self.opt_range("glue.role_hpf.extra", h.extra, HZ.0, HZ.1);
        }
        self.opt_range("glue.bass_mono_hz", g.bass_mono_hz, HZ.0, HZ.1);
        if let Some(d) = &g.duck {
            self.opt_range("glue.duck.attack_ms", d.attack_ms, 0.0, 100.0);
            for (name, group) in [("bass", &d.bass), ("others", &d.others)] {
                if let Some(g) = group {
                    self.opt_range(&format!("glue.duck.{name}.kick_db"), g.kick_db, 0.0, 30.0);
                    self.opt_range(&format!("glue.duck.{name}.snare_db"), g.snare_db, 0.0, 30.0);
                    self.opt_range(&format!("glue.duck.{name}.release_ms"), g.release_ms, 1.0, 2000.0);
                }
            }
        }
        if let Some(b) = &g.drum_bus {
            if let Some(c) = &b.comp {
                self.comp("glue.drum_bus.comp", c);
            }
            self.opt_range("glue.drum_bus.drive", b.drive, 0.0, 1.0);
        }
        if let Some(sd) = &g.shared_drive {
            self.opt_range("glue.shared_drive.drive", sd.drive, 0.0, 1.0);
            self.opt_range("glue.shared_drive.mix", sd.mix, 0.0, 1.0);
        }
        if let Some(c) = &g.master {
            self.comp("glue.master", c);
        }
    }

    fn comp(&mut self, at: &str, c: &CompSpec) {
        self.opt_range(format!("{at}.threshold_db"), c.threshold_db, -60.0, 0.0);
        self.opt_range(format!("{at}.ratio"), c.ratio, 1.0, 100.0);
        self.opt_range(format!("{at}.attack_ms"), c.attack_ms, 0.0, 1000.0);
        self.opt_range(format!("{at}.release_ms"), c.release_ms, 0.0, 10_000.0);
        self.opt_range(format!("{at}.makeup_db"), c.makeup_db, -24.0, 24.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live-style")
    }

    fn any_exists(_: &Path) -> bool {
        true
    }

    fn fixture_spec() -> StyleSpec {
        let text = std::fs::read_to_string(fixture_dir().join(STYLE_FILE)).unwrap();
        StyleSpec::from_yaml(&text).unwrap()
    }

    fn problems(spec: &StyleSpec) -> Vec<Problem> {
        spec.validate(Path::new("/style"), &any_exists).into_iter().map(|i| i.problem).collect()
    }

    #[test]
    fn fixture_loads_from_disk() {
        let style = Style::load(&fixture_dir()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(style.spec.name, "fixture-breaks");
        assert_eq!(style.spec.lanes.len(), NUM_LANES);
        assert_eq!(style.spec.lanes[0].role, Role::Drums);
        assert_eq!(style.default_key(), Some(MusicalKey::new(9, true)));
        let files = style.asset_files();
        assert!(!files.is_empty());
        assert!(files.iter().all(|p| p.is_file()), "{files:?}");
        let progs = style.progressions();
        assert_eq!(progs.len(), style.spec.progressions.len());
        assert_eq!(progs[0].degrees(), &[0, 5, 3, 4]);
    }

    #[test]
    fn load_named_resolves_collection_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let dst = style_dir(tmp.path(), "fixture-breaks");
        copy_dir(&fixture_dir(), &dst);
        let style = Style::load_named(tmp.path(), "fixture-breaks").unwrap();
        assert_eq!(style.dir, dst);
        assert!(matches!(Style::load_named(tmp.path(), "../etc"), Err(StyleError::InvalidName(_))));
        assert!(matches!(Style::load_named(tmp.path(), "missing"), Err(StyleError::Io { .. })));
    }

    fn copy_dir(src: &Path, dst: &Path) {
        for entry in walkdir::WalkDir::new(src) {
            let entry = entry.unwrap();
            let rel = entry.path().strip_prefix(src).unwrap();
            let target = dst.join(rel);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&target).unwrap();
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    #[test]
    fn reference_template_parses_and_validates() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/live-style-reference.yaml");
        let text = std::fs::read_to_string(&path).unwrap();
        let spec = StyleSpec::from_yaml(&text).unwrap_or_else(|e| panic!("{e}"));
        let issues = spec.validate(Path::new("/style"), &any_exists);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn yaml_round_trip() {
        let spec = fixture_spec();
        let text = spec.to_yaml().unwrap();
        let back = StyleSpec::from_yaml(&text).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn missing_assets_reported() {
        let spec = fixture_spec();
        let issues = spec.validate(Path::new("/nowhere"), &|_| false);
        assert!(!issues.is_empty());
        assert!(issues.iter().all(|i| matches!(i.problem, Problem::AssetMissing(_))));
        assert!(issues.iter().any(|i| i.at.contains("clap.preset")));
        assert!(issues[0].at.starts_with("lanes[0].sub_voices[0]"));
    }

    #[test]
    fn missing_file_gives_load_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::copy(fixture_dir().join(STYLE_FILE), tmp.path().join(STYLE_FILE)).unwrap();
        match Style::load(tmp.path()) {
            Err(StyleError::Invalid { issues, .. }) => {
                assert!(issues.iter().any(|i| matches!(i.problem, Problem::AssetMissing(_))));
            }
            other => panic!("{other:?}"),
        }
        let err = Style::load(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("asset not found"), "{err}");
    }

    #[test]
    fn parse_error_carries_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(STYLE_FILE), "name: x\nbogus_field: 1\n").unwrap();
        match Style::load(tmp.path()) {
            Err(StyleError::Parse { path, .. }) => assert!(path.ends_with(STYLE_FILE)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_fields_rejected_everywhere() {
        let text = std::fs::read_to_string(fixture_dir().join(STYLE_FILE)).unwrap();
        let bad = text.replacen("plugin_id:", "plugin_idd:", 1);
        assert!(StyleSpec::from_yaml(&bad).is_err());
        let bad = text.replacen("param: style }", "param: style, extra: 1 }", 1);
        assert_ne!(bad, text);
        assert!(StyleSpec::from_yaml(&bad).is_err());
    }

    #[test]
    fn lane_structure_rules() {
        let mut s = fixture_spec();
        s.lanes.pop();
        assert!(problems(&s).contains(&Problem::LaneCount { expected: 4, found: 3 }));

        let mut s = fixture_spec();
        s.lanes.swap(0, 1);
        let p = problems(&s);
        assert!(p.contains(&Problem::FirstLaneNotDrums));
        assert!(p.contains(&Problem::ExtraDrumLane));

        let mut s = fixture_spec();
        let opts = s.lanes[1].options.clone();
        s.lanes[0].options = opts;
        s.lanes[1].archetypes = vec!["x".into()];
        let p = problems(&s);
        assert!(p.contains(&Problem::OptionsOnDrumLane));
        assert!(p.contains(&Problem::ArchetypesOnInstrumentLane));

        let mut s = fixture_spec();
        s.lanes[0].archetypes.clear();
        assert!(problems(&s).contains(&Problem::NoArchetypes));

        let mut s = fixture_spec();
        s.lanes[2].default_option = 99;
        assert!(problems(&s).iter().any(|p| matches!(p, Problem::IndexOutOfRange { index: 99, .. })));
    }

    #[test]
    fn option_and_control_rules() {
        let mut s = fixture_spec();
        let clap = ClapSpec { plugin_id: "x".into(), preset: None };
        s.lanes[1].options[0].clap = Some(clap);
        assert!(problems(&s).contains(&Problem::SourceCount));

        let mut s = fixture_spec();
        s.lanes[1].controls.pop();
        assert!(problems(&s).contains(&Problem::ControlCount { expected: 4, found: 3 }));

        let mut s = fixture_spec();
        s.lanes[1].controls[3].target = ControlTarget::Fx { slot: 9, param: 0 };
        s.lanes[1].controls[2].default = 2.0;
        s.lanes[1].controls[1].max = f32::NAN;
        s.lanes[1].options[0].lufs_trim_db = 60.0;
        let p = problems(&s);
        assert!(p.iter().any(|p| matches!(p, Problem::IndexOutOfRange { index: 9, .. })));
        assert!(p.iter().any(|p| matches!(p, Problem::OutOfRange { value, .. } if *value == 2.0)));
        assert!(p.iter().any(|p| matches!(p, Problem::NotFinite(_))));
        assert!(p.iter().any(|p| matches!(p, Problem::OutOfRange { value, .. } if *value == 60.0)));
    }

    #[test]
    fn sample_rules() {
        let mut s = fixture_spec();
        let sample = &mut s.lanes[0].sub_voices[1].options[0].sampler.as_mut().unwrap().samples[0];
        sample.slices.swap(0, 1);
        sample.zone = Some(ZoneSpec { keys: [MidiNote(60), MidiNote(40)], velocities: [0, 127] });
        let p = problems(&s);
        assert!(p.contains(&Problem::SlicesUnordered));
        assert!(p.contains(&Problem::BadRange { lo: 60, hi: 40 }));
    }

    #[test]
    fn asset_path_rules() {
        let mut s = fixture_spec();
        let samples = &mut s.lanes[1].options[0].sampler.as_mut().unwrap().samples;
        samples[0].file = PathBuf::from("/etc/passwd");
        samples.push(SampleSpec { file: PathBuf::from("../other/x.wav"), ..samples[0].clone() });
        let p = problems(&s);
        assert!(p.contains(&Problem::AssetPathAbsolute(PathBuf::from("/etc/passwd"))));
        assert!(p.contains(&Problem::AssetPathEscapes(PathBuf::from("../other/x.wav"))));
    }

    #[test]
    fn global_rules() {
        let mut s = fixture_spec();
        s.version = 99;
        s.tempo.min = 200.0;
        s.default_key = Some("H#".into());
        s.progressions[0].degrees = vec![1, 8];
        s.progressions[0].bars_per_chord = 0;
        s.default_progression = 50;
        s.name = "a/b".into();
        let p = problems(&s);
        assert!(p.contains(&Problem::UnsupportedVersion { found: 99, supported: 1 }));
        assert!(p.contains(&Problem::TempoRange));
        assert!(p.contains(&Problem::InvalidKey("H#".into())));
        assert!(p.contains(&Problem::DegreeOutOfRange(8)));
        assert!(p.contains(&Problem::BarsPerChord));
        assert!(p.iter().any(|p| matches!(p, Problem::IndexOutOfRange { index: 50, .. })));
        assert!(p.contains(&Problem::InvalidName("a/b".into())));

        let mut s = fixture_spec();
        s.progressions.clear();
        assert!(problems(&s).contains(&Problem::Empty));
    }

    #[test]
    fn generator_rules() {
        let mut s = fixture_spec();
        s.lanes[2].generator.family = GeneratorFamily::Idiom;
        s.lanes[2].generator.idiom = None;
        assert!(problems(&s).contains(&Problem::IdiomDataMissing));
        s.lanes[2].generator.idiom = Some(IdiomSpec {
            level_maps: vec![LevelMapSpec { name: "m".into(), archetype: None, voice: None, levels: vec![0; 15] }],
        });
        assert!(problems(&s).contains(&Problem::LevelMapLength(15)));
    }

    #[test]
    fn glue_rules() {
        let mut s = fixture_spec();
        s.glue.bass_mono_hz = Some(-5.0);
        s.glue.master = Some(CompSpec { ratio: Some(0.5), ..Default::default() });
        let p = problems(&s);
        assert!(p.iter().any(|p| matches!(p, Problem::OutOfRange { value, .. } if *value == -5.0)));
        assert!(p.iter().any(|p| matches!(p, Problem::OutOfRange { value, .. } if *value == 0.5)));
    }

    #[test]
    fn glue_is_all_optional() {
        let g: GlueSpec = serde_yaml::from_str("{}").unwrap();
        assert_eq!(g, GlueSpec::default());
        let g: GlueSpec = serde_yaml::from_str("duck: { bass: { kick_db: 9 } }").unwrap();
        assert_eq!(g.duck.unwrap().bass.unwrap().kick_db, Some(9.0));
        assert_eq!(g.duck.unwrap().others, None);
    }

    #[test]
    fn issues_display_with_location() {
        let mut s = fixture_spec();
        s.lanes[3].sends.delay = 3.0;
        let issues = s.validate(Path::new("/s"), &any_exists);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].to_string(), "lanes[3].sends.delay: value 3 outside 0..=1");
    }

    #[test]
    fn note_names() {
        assert_eq!(parse_note_name("C4"), Some(60));
        assert_eq!(parse_note_name("A2"), Some(45));
        assert_eq!(parse_note_name("C#4"), Some(61));
        assert_eq!(parse_note_name("Bb1"), Some(34));
        assert_eq!(parse_note_name("C-1"), Some(0));
        assert_eq!(parse_note_name("G9"), Some(127));
        assert_eq!(parse_note_name("G#9"), None);
        assert_eq!(parse_note_name("X4"), None);
        assert_eq!(parse_note_name("C"), None);
        let n: MidiNote = serde_yaml::from_str("\"A2\"").unwrap();
        assert_eq!(n, MidiNote(45));
        let n: MidiNote = serde_yaml::from_str("45").unwrap();
        assert_eq!(n, MidiNote(45));
        assert!(serde_yaml::from_str::<MidiNote>("200").is_err());
    }

    #[test]
    fn curves_and_mapping() {
        for c in [Curve::Linear, Curve::Exp, Curve::Log, Curve::S] {
            assert_eq!(c.apply(0.0), 0.0);
            assert_eq!(c.apply(1.0), 1.0);
            assert_eq!(c.apply(-3.0), 0.0);
            assert_eq!(c.apply(f32::NAN), 0.0);
            let mut prev = -1.0;
            for i in 0..=100 {
                let y = c.apply(i as f32 / 100.0);
                assert!(y >= prev);
                prev = y;
            }
        }
        assert!(Curve::Exp.apply(0.5) < 0.5 && Curve::Log.apply(0.5) > 0.5);
        let ctl = ControlSpec {
            label: None,
            target: ControlTarget::Send { bus: SendBus::Delay },
            min: 1.0,
            max: 0.0,
            curve: Curve::Linear,
            default: 0.5,
        };
        assert_eq!(ctl.map(0.25), 0.75);
    }

    #[test]
    fn control_target_yaml_shapes() {
        let t: ControlTarget = serde_yaml::from_str("{ type: fx, slot: 1, param: 7 }").unwrap();
        assert_eq!(t, ControlTarget::Fx { slot: 1, param: 7 });
        let t: ControlTarget = serde_yaml::from_str("{ type: send, bus: reverb }").unwrap();
        assert_eq!(t, ControlTarget::Send { bus: SendBus::Reverb });
        assert!(serde_yaml::from_str::<ControlTarget>("{ type: fx, slot: 1 }").is_err());
    }

    #[test]
    fn style_names() {
        assert!(is_valid_style_name("amen-rollers"));
        for bad in ["", " ", ".", "..", "a/b", "a\\b"] {
            assert!(!is_valid_style_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn loudness_defaults() {
        let l: LoudnessTargets = serde_yaml::from_str("bass: -13").unwrap();
        assert_eq!(l.target(Role::Bass), -13.0);
        assert_eq!(l.target(Role::Drums), DEFAULT_LUFS_DRUMS);
    }
}
