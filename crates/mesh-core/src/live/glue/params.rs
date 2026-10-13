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

/// Stage 3: kick-keyed low-band duck on bass / lead / extra.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuckParams {
    pub enabled: bool,
    /// Upper edge of the ducked band. Everything below it is cut (the
    /// 50–120 Hz kick region plus the sub under it).
    pub crossover_hz: f32,
    /// Full-duck cut on the low band, per role.
    pub bass_depth_db: f32,
    pub lead_depth_db: f32,
    pub extra_depth_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    /// Release in beats, converted with the current BPM every block.
    pub release_beats: f32,
    /// Sidechain key level that fires the duck (dBFS peak).
    pub key_threshold_db: f32,
}

impl Default for DuckParams {
    fn default() -> Self {
        Self {
            enabled: true,
            // MUSICAL: kick band 50–120 Hz (R§7.1.9).
            crossover_hz: 120.0,
            // MUSICAL: bass ducks hard, lead/extra have little energy there after their HPF.
            bass_depth_db: 12.0,
            lead_depth_db: 6.0,
            extra_depth_db: 6.0,
            // MUSICAL: fast attack, short hold, 1/8-note release.
            attack_ms: 2.0,
            hold_ms: 20.0,
            release_beats: 0.5,
            key_threshold_db: -30.0,
        }
    }
}

impl DuckParams {
    /// Full-duck depth for a role. Drums are never ducked.
    pub fn depth_db(&self, role: Role) -> f32 {
        match role {
            Role::Drums => 0.0,
            Role::Bass => self.bass_depth_db,
            Role::Lead => self.lead_depth_db,
            Role::Extra => self.extra_depth_db,
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

/// Stage 4: drum bus.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DrumBusParams {
    pub compressor_enabled: bool,
    pub compressor: CompressorParams,
    pub saturation_enabled: bool,
    pub saturation: SaturationParams,
}

impl Default for DrumBusParams {
    fn default() -> Self {
        Self {
            compressor_enabled: true,
            // MUSICAL: glue compression, slow-ish attack keeps transients.
            compressor: CompressorParams {
                threshold_db: -16.0,
                ratio: 4.0,
                attack_ms: 10.0,
                release_ms: 80.0,
                makeup_db: 2.0,
                knee_db: 6.0,
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

/// Stage 6: master compressor (before the existing limiter / clipper).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MasterParams {
    pub compressor_enabled: bool,
    pub compressor: CompressorParams,
    /// Output gain applied after the compressor, before the limiter.
    pub output_db: f32,
}

impl Default for MasterParams {
    fn default() -> Self {
        Self {
            compressor_enabled: true,
            // MUSICAL: gentle bus glue, 1–3 dB of reduction on a full mix.
            compressor: CompressorParams {
                threshold_db: -10.0,
                ratio: 2.0,
                attack_ms: 30.0,
                release_ms: 150.0,
                makeup_db: 0.0,
                knee_db: 6.0,
            },
            output_db: 0.0,
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
            "high_pass:\n  lead:\n    freq_hz: 800\n    slope: db12\nduck:\n  release_beats: 1.0\n",
        )
        .unwrap();
        assert_eq!(p.high_pass.lead.freq_hz, 800.0);
        assert_eq!(p.high_pass.lead.slope, Slope::Db12);
        assert!(p.high_pass.lead.enabled, "unlisted field keeps its default");
        assert_eq!(p.high_pass.extra, HighPassParams::default().extra);
        assert_eq!(p.duck.release_beats, 1.0);
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
        assert_eq!(p.duck.depth_db(Role::Drums), 0.0);
    }
}
