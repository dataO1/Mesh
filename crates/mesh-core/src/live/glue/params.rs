//! Runtime glue parameters.
//!
//! Standalone from the style schema: the style loader deserializes into (or
//! converts to) [`GlueParams`]. Every field has a default, so a style only
//! lists what it changes. All structs are `Copy` so the RT thread can take
//! updates without allocating.

use serde::{Deserialize, Serialize};

use super::Role;

/// High-pass filter slope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Slope {
    /// 12 dB/oct (one Butterworth 2-pole stage).
    Db12,
    /// 24 dB/oct (4th-order Butterworth, two stages).
    #[default]
    Db24,
}

/// One role's high-pass filter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleHighPassParams {
    pub enabled: bool,
    pub freq_hz: f32,
    pub slope: Slope,
}

impl Default for RoleHighPassParams {
    fn default() -> Self {
        Self { enabled: false, freq_hz: 30.0, slope: Slope::Db24 }
    }
}

/// Stage 1: role high-pass. Bass never gets one (its lows are the point).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HighPassParams {
    pub drums: RoleHighPassParams,
    pub lead: RoleHighPassParams,
    pub extra: RoleHighPassParams,
}

impl Default for HighPassParams {
    fn default() -> Self {
        Self {
            // MUSICAL: drums keep their sub by default (the kick owns it).
            drums: RoleHighPassParams { enabled: false, freq_hz: 30.0, slope: Slope::Db24 },
            // MUSICAL: lead ~500 Hz, extra ~300 Hz (design §5.6).
            lead: RoleHighPassParams { enabled: true, freq_hz: 500.0, slope: Slope::Db24 },
            extra: RoleHighPassParams { enabled: true, freq_hz: 300.0, slope: Slope::Db24 },
        }
    }
}

impl HighPassParams {
    /// The high-pass settings for a role. Bass always returns a disabled filter.
    pub fn for_role(&self, role: Role) -> RoleHighPassParams {
        match role {
            Role::Drums => self.drums,
            Role::Lead => self.lead,
            Role::Extra => self.extra,
            Role::Bass => RoleHighPassParams { enabled: false, ..self.drums },
        }
    }
}

/// Stage 2: bass mono-below crossover.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonoBelowParams {
    pub enabled: bool,
    pub freq_hz: f32,
}

impl Default for MonoBelowParams {
    fn default() -> Self {
        // MUSICAL: mono below ~120 Hz (design §5.6).
        Self { enabled: true, freq_hz: 120.0 }
    }
}

/// Stage 3: full-band duck on bass / lead / extra, keyed by the main drum
/// kit's kick and snare (never by break slices).
///
/// Each ducked lane's cut is `kick_db × kick envelope + snare_db × snare
/// envelope`. The bass ducks mostly on the kick with a short release, lead
/// and extra mostly on the snare with a longer one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuckParams {
    pub enabled: bool,
    pub attack_ms: f32,
    pub bass: DuckRoleParams,
    /// Lead and extra.
    pub others: DuckRoleParams,
    /// Sidechain key level that fires the duck (dBFS peak).
    pub key_threshold_db: f32,
}

/// Cut and release of one group of ducked lanes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuckRoleParams {
    /// Full-duck cut on a main-kit kick.
    pub kick_db: f32,
    /// Full-duck cut on a main-kit snare.
    pub snare_db: f32,
    pub release_ms: f32,
}

impl Default for DuckRoleParams {
    fn default() -> Self {
        Self { kick_db: 0.0, snare_db: 0.0, release_ms: 50.0 }
    }
}

impl Default for DuckParams {
    fn default() -> Self {
        Self {
            enabled: true,
            // MUSICAL: user spec: fastest attack (0.5 ms, short enough to
            // read as instant without a click), bass 8 dB on the kick with
            // 50 ms release, lead/extra slight and slower, mostly on the snare.
            attack_ms: 0.5,
            bass: DuckRoleParams { kick_db: 8.0, snare_db: 2.0, release_ms: 50.0 },
            others: DuckRoleParams { kick_db: 2.0, snare_db: 4.0, release_ms: 120.0 },
            key_threshold_db: -30.0,
        }
    }
}

impl DuckParams {
    /// Duck settings for a role. Drums are never ducked.
    pub fn for_role(&self, role: Role) -> Option<&DuckRoleParams> {
        match role {
            Role::Drums => None,
            Role::Bass => Some(&self.bass),
            Role::Lead | Role::Extra => Some(&self.others),
        }
    }
}

/// Feed-forward compressor parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
    /// Soft-knee width in dB (0 = hard knee).
    pub knee_db: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 0.0,
            knee_db: 6.0,
        }
    }
}

/// Saturation parameters (see `saturation.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaturationParams {
    /// Pre-gain into the waveshaper.
    pub drive_db: f32,
    /// Bias into tanh, 0 = symmetric (odd harmonics only), up to ~0.5.
    pub asymmetry: f32,
    /// Dry/wet blend, 0..=1.
    pub mix: f32,
    pub output_db: f32,
}

impl Default for SaturationParams {
    fn default() -> Self {
        Self { drive_db: 6.0, asymmetry: 0.0, mix: 1.0, output_db: 0.0 }
    }
}

// MUSICAL: thresholds measured with `live-render` on the starter style
// (LUFS-calibrated lanes) for the target peak reduction; makeup restores
// about the average reduction.
const DRUM_COMP1_THRESHOLD_DB: f32 = -8.5;
const DRUM_COMP1_MAKEUP_DB: f32 = 2.0;
const DRUM_COMP2_THRESHOLD_DB: f32 = -12.0;
const DRUM_COMP2_MAKEUP_DB: f32 = 1.0;
/// Bass lanes peak near full scale, so their compressor sits higher.
const BASS_COMP_THRESHOLD_DB: f32 = -5.0;
const LEAD_COMP_THRESHOLD_DB: f32 = -14.0;
const EXTRA_COMP_THRESHOLD_DB: f32 = -18.0;
/// Master level into the saturator and limiter, so the limiter only
/// catches peaks (about 1-2 dB).
const MASTER_OUTPUT_DB: f32 = -4.0;

/// Stage 3b: per-lane compressor then saturation (bass, lead, extra; the
/// drums get theirs on the drum bus).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaneDynamicsParams {
    pub compressor_enabled: bool,
    pub compressor: CompressorParams,
    pub saturation_enabled: bool,
    pub saturation: SaturationParams,
}

impl Default for LaneDynamicsParams {
    fn default() -> Self {
        Self {
            compressor_enabled: true,
            // MUSICAL: light levelling, 2-3 dB peak reduction.
            compressor: CompressorParams {
                threshold_db: LEAD_COMP_THRESHOLD_DB,
                ratio: 3.0,
                attack_ms: 10.0,
                release_ms: 80.0,
                makeup_db: 1.0,
                knee_db: 6.0,
            },
            saturation_enabled: true,
            // MUSICAL: slight saturation, half wet.
            saturation: SaturationParams { drive_db: 3.0, asymmetry: 0.05, mix: 0.5, output_db: 0.0 },
        }
    }
}

/// Per-role lane dynamics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleDynamicsParams {
    pub bass: LaneDynamicsParams,
    pub lead: LaneDynamicsParams,
    pub extra: LaneDynamicsParams,
}

impl Default for RoleDynamicsParams {
    fn default() -> Self {
        let lane = LaneDynamicsParams::default();
        Self {
            // MUSICAL: the bass gets a bit more drive for harmonics on small speakers.
            bass: LaneDynamicsParams {
                compressor: CompressorParams { threshold_db: BASS_COMP_THRESHOLD_DB, ..lane.compressor },
                saturation: SaturationParams { drive_db: 4.0, ..lane.saturation },
                ..lane
            },
            lead: lane,
            extra: LaneDynamicsParams {
                compressor: CompressorParams { threshold_db: EXTRA_COMP_THRESHOLD_DB, ..lane.compressor },
                ..lane
            },
        }
    }
}

impl RoleDynamicsParams {
    /// Dynamics of a role. Drums use the drum bus instead.
    pub fn for_role(&self, role: Role) -> Option<&LaneDynamicsParams> {
        match role {
            Role::Drums => None,
            Role::Bass => Some(&self.bass),
            Role::Lead => Some(&self.lead),
            Role::Extra => Some(&self.extra),
        }
    }
}

/// Stage 4: drum bus.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DrumBusParams {
    pub compressor_enabled: bool,
    /// First compressor: punch.
    pub compressor: CompressorParams,
    pub compressor2_enabled: bool,
    /// Second compressor: consistency.
    pub compressor2: CompressorParams,
    pub saturation_enabled: bool,
    pub saturation: SaturationParams,
}

impl Default for DrumBusParams {
    fn default() -> Self {
        Self {
            compressor_enabled: true,
            // MUSICAL: user spec: 4:1, 15 ms attack, 59 ms release, hard
            // knee, about 3-4 dB peak reduction.
            compressor: CompressorParams {
                threshold_db: DRUM_COMP1_THRESHOLD_DB,
                ratio: 4.0,
                attack_ms: 15.0,
                release_ms: 59.0,
                makeup_db: DRUM_COMP1_MAKEUP_DB,
                knee_db: 0.0,
            },
            compressor2_enabled: true,
            // MUSICAL: user spec: 2:1, 30 ms attack, 20 ms release, hard knee.
            compressor2: CompressorParams {
                threshold_db: DRUM_COMP2_THRESHOLD_DB,
                ratio: 2.0,
                attack_ms: 30.0,
                release_ms: 20.0,
                makeup_db: DRUM_COMP2_MAKEUP_DB,
                knee_db: 0.0,
            },
            saturation_enabled: true,
            // MUSICAL: light warmth, slight even-harmonic colour.
            saturation: SaturationParams { drive_db: 4.0, asymmetry: 0.1, mix: 0.6, output_db: 0.0 },
        }
    }
}

/// Stage 5: shared drive on the drums + bass sum.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveParams {
    pub enabled: bool,
    pub saturation: SaturationParams,
}

impl Default for DriveParams {
    fn default() -> Self {
        Self {
            enabled: true,
            // MUSICAL: parallel-style blend so the low end glues without smearing.
            saturation: SaturationParams { drive_db: 6.0, asymmetry: 0.0, mix: 0.35, output_db: 0.0 },
        }
    }
}

/// Stage 6: master compressor, output gain and saturation (before the
/// existing limiter / clipper).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MasterParams {
    pub compressor_enabled: bool,
    pub compressor: CompressorParams,
    /// Output gain applied after the compressor, before the saturator.
    pub output_db: f32,
    pub saturation_enabled: bool,
    pub saturation: SaturationParams,
}

impl Default for MasterParams {
    fn default() -> Self {
        Self {
            compressor_enabled: true,
            // MUSICAL: user spec: 4:1, 25 ms attack, 10 ms release, about
            // 4 dB peak reduction (threshold set per style).
            compressor: CompressorParams {
                threshold_db: -8.0,
                ratio: 4.0,
                attack_ms: 25.0,
                release_ms: 10.0,
                makeup_db: 0.0,
                knee_db: 6.0,
            },
            output_db: MASTER_OUTPUT_DB,
            saturation_enabled: true,
            // MUSICAL: slight final saturation, mostly dry.
            saturation: SaturationParams { drive_db: 2.0, asymmetry: 0.0, mix: 0.4, output_db: 0.0 },
        }
    }
}

/// Shared delay send.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DelayParams {
    /// Delay time in beats (0.75 = dotted 1/8). Clamped to the 2 s delay line.
    pub beats: f32,
    pub feedback: f32,
    pub ping_pong: bool,
    pub return_db: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        // MUSICAL: dotted-eighth ping-pong is the DnB default.
        Self { beats: 0.75, feedback: 0.35, ping_pong: true, return_db: 0.0 }
    }
}

/// Shared reverb send (normalized `ReverbEffect` parameters).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverbParams {
    pub room_size: f32,
    pub damping: f32,
    pub width: f32,
    pub return_db: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        // MUSICAL: medium dark room.
        Self { room_size: 0.6, damping: 0.6, width: 1.0, return_db: 0.0 }
    }
}

/// All glue-chain parameters.
///
/// Deserializing is a deep merge over [`GlueParams::default`]: a style that
/// sets only `drum_bus.compressor.ratio` keeps the drum-bus threshold, not the
/// generic [`CompressorParams`] one.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct GlueParams {
    pub high_pass: HighPassParams,
    pub mono_below: MonoBelowParams,
    pub duck: DuckParams,
    pub lane_dynamics: RoleDynamicsParams,
    pub drum_bus: DrumBusParams,
    pub drive: DriveParams,
    pub master: MasterParams,
    pub delay: DelayParams,
    pub reverb: ReverbParams,
}

/// Complete form of [`GlueParams`], deserialized after the merge.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FullGlueParams {
    high_pass: HighPassParams,
    mono_below: MonoBelowParams,
    duck: DuckParams,
    lane_dynamics: RoleDynamicsParams,
    drum_bus: DrumBusParams,
    drive: DriveParams,
    master: MasterParams,
    delay: DelayParams,
    reverb: ReverbParams,
}

/// Recursively overlay `patch` onto `base` (maps merge, everything else replaces).
fn merge(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(b), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, p) => *b = p,
    }
}

impl<'de> Deserialize<'de> for GlueParams {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let patch = serde_json::Value::deserialize(d)?;
        let mut merged = serde_json::to_value(GlueParams::default()).map_err(D::Error::custom)?;
        if !patch.is_null() {
            merge(&mut merged, patch);
        }
        let f: FullGlueParams = serde_json::from_value(merged).map_err(D::Error::custom)?;
        Ok(Self {
            high_pass: f.high_pass,
            mono_below: f.mono_below,
            duck: f.duck,
            lane_dynamics: f.lane_dynamics,
            drum_bus: f.drum_bus,
            drive: f.drive,
            master: f.master,
            delay: f.delay,
            reverb: f.reverb,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_yaml_keeps_defaults() {
        let p: GlueParams = serde_yaml::from_str(
            "high_pass:\n  lead:\n    freq_hz: 800\n    slope: db12\nduck:\n  bass:\n    release_ms: 60\n",
        )
        .unwrap();
        assert_eq!(p.high_pass.lead.freq_hz, 800.0);
        assert_eq!(p.high_pass.lead.slope, Slope::Db12);
        assert!(p.high_pass.lead.enabled, "unlisted field keeps its default");
        assert_eq!(p.high_pass.extra, HighPassParams::default().extra);
        assert_eq!(p.duck.bass.release_ms, 60.0);
        assert_eq!(p.duck.bass.kick_db, DuckParams::default().bass.kick_db, "nested field keeps its default");
        assert_eq!(p.master, MasterParams::default());
    }

    #[test]
    fn nested_partial_keeps_context_defaults() {
        let p: GlueParams = serde_yaml::from_str("drum_bus:\n  compressor:\n    ratio: 6\n").unwrap();
        let d = DrumBusParams::default();
        assert_eq!(p.drum_bus.compressor.ratio, 6.0);
        assert_eq!(p.drum_bus.compressor.threshold_db, d.compressor.threshold_db);
        assert_eq!(p.drum_bus.saturation, d.saturation);
        // Empty document = all defaults; unknown top-level keys are rejected.
        assert_eq!(serde_yaml::from_str::<GlueParams>("{}").unwrap(), GlueParams::default());
        assert!(serde_yaml::from_str::<GlueParams>("bogus: 1\n").is_err());
    }

    #[test]
    fn bass_never_high_passed_or_drums_ducked() {
        let mut p = GlueParams::default();
        p.high_pass.drums.enabled = true;
        assert!(!p.high_pass.for_role(Role::Bass).enabled);
        assert!(p.duck.for_role(Role::Drums).is_none());
    }
}
