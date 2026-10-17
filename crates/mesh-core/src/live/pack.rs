//! Sound packs: the user's instrument and MIDI phrases, played by the
//! `phrase` generator (docs/plans/2026-10-16-live-sound-packs.md).
//!
//! ```text
//! <pack>/
//!   sound.sf2 | sound.sfz   the instrument (any one *.sf2 / *.sfz works too)
//!   phrases/*.mid           the phrases (*.mid next to the sound count too)
//!   pack.yaml               optional settings (PackSpec)
//! ```
//!
//! A style uses a pack as a lane option (`pack: packs/lead/reese`) or takes
//! every pack in a folder (`pack_dir: packs/lead`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::phrase::Phrase;
use super::sampler::{CcMap, ModConfig, SoundShape, CUTOFF_MAX_HZ, MAX_CC_MAPS, NO_CC};
use super::sfz::{FilterKind, Instrument, Shape};

pub const PACK_FILE: &str = "pack.yaml";
pub const PHRASES_FOLDER: &str = "phrases";
/// Most phrases read per pack.
pub const MAX_PHRASES: usize = 64;
/// A pack sample longer than this plays as a stream unless `stream` says
/// otherwise.
pub const STREAM_AUTO_SECS: f64 = 8.0;

/// `pack.yaml`. Every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PackSpec {
    /// Display name (default: the folder name).
    pub name: Option<String>,
    /// The instrument file, pack-relative (default: see [`find_sound`]).
    pub sound: Option<PathBuf>,
    /// Glide time in seconds for tied notes without CC5 in the phrase.
    pub glide: Option<f32>,
    /// The CC the Tone knob writes (default: the instrument's only cutoff
    /// CC, else 74).
    pub tone_cc: Option<u8>,
    /// The CC the Morph knob writes (default 20).
    pub morph_cc: Option<u8>,
    /// One voice at a time: overlapping notes slide (default: bass lanes).
    pub mono: Option<bool>,
    /// A mudpie: notes start where the generator's seed puts them across the
    /// whole sample, shifted by the Morph knob (default: samples longer than
    /// [`STREAM_AUTO_SECS`]).
    pub stream: Option<bool>,
    /// Level on top of the loudness normalization, dB.
    pub gain_db: f32,
}

/// A pack read from disk (audio not decoded yet; the loader does that).
#[derive(Debug, Clone)]
pub struct Pack {
    pub name: String,
    pub dir: PathBuf,
    pub spec: PackSpec,
    pub sound: PathBuf,
    pub instrument: Instrument,
    pub phrases: Vec<Phrase>,
    /// Unreadable phrases, unsupported opcodes and the like.
    pub warnings: Vec<String>,
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

/// The pack's instrument: `sound.sf2`, `sound.sfz`, else the first `*.sf2`,
/// else the first `*.sfz` in the folder.
pub fn find_sound(dir: &Path) -> Option<PathBuf> {
    for name in ["sound.sf2", "sound.sfz"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    let all = files(dir);
    all.iter().find(|p| has_ext(p, "sf2")).or_else(|| all.iter().find(|p| has_ext(p, "sfz"))).cloned()
}

/// Whether `dir` holds a pack (an instrument file).
pub fn is_pack(dir: &Path) -> bool {
    dir.join(PACK_FILE).is_file() || find_sound(dir).is_some()
}

/// The packs inside `dir` (its sub-folders that hold one), sorted by name.
pub fn scan(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && is_pack(p)).collect())
        .unwrap_or_default();
    out.sort();
    out
}

/// The phrase files: `phrases/*.mid` and `*.mid` next to the sound.
pub fn phrase_files(dir: &Path) -> Vec<PathBuf> {
    let mid = |p: &PathBuf| has_ext(p, "mid") || has_ext(p, "midi");
    let mut out: Vec<PathBuf> = files(&dir.join(PHRASES_FOLDER)).into_iter().filter(mid).collect();
    out.extend(files(dir).into_iter().filter(mid));
    out
}

impl Pack {
    /// Read a pack: settings, instrument (SF2 audio included) and phrases.
    /// Fails only without a readable instrument.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let mut warnings = Vec::new();
        let spec = match std::fs::read_to_string(dir.join(PACK_FILE)) {
            Ok(text) => serde_yaml::from_str(&text).map_err(|e| format!("{PACK_FILE}: {e}"))?,
            Err(_) => PackSpec::default(),
        };
        let sound = match &spec.sound {
            Some(rel) => dir.join(rel),
            None => find_sound(dir).ok_or_else(|| "no .sf2 or .sfz file".to_string())?,
        };
        let instrument = Instrument::load_any(&sound)?;
        if instrument.regions.is_empty() {
            return Err(format!("{}: no playable zones", sound.display()));
        }
        warnings.extend(instrument.warnings.iter().cloned());
        if !instrument.unknown.is_empty() {
            let names: Vec<&str> = instrument.unknown.keys().map(String::as_str).collect();
            warnings.push(format!("ignored: {}", names.join(", ")));
        }
        let mut phrases = Vec::new();
        for path in phrase_files(dir) {
            if phrases.len() >= MAX_PHRASES {
                warnings.push(format!("more than {MAX_PHRASES} phrases; the rest are skipped"));
                break;
            }
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| Phrase::from_midi(&name, &b)) {
                Ok(p) => phrases.push(p),
                Err(e) => warnings.push(format!("phrase {}: {e}", path.display())),
            }
        }
        if phrases.is_empty() {
            warnings.push("no phrases: the lane stays silent".to_string());
        }
        let name = spec.name.clone().unwrap_or_else(|| {
            dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "pack".into())
        });
        Ok(Self { name, dir: dir.to_path_buf(), spec, sound, instrument, phrases, warnings })
    }

    /// The Tone knob's CC (see [`PackSpec::tone_cc`]): the one CC the
    /// instrument's cutoff maps, else the one CC its zone modulators use
    /// (a crossfade), else 74.
    pub fn tone_cc(&self) -> u8 {
        let one = |ccs: Vec<u8>| -> Option<u8> {
            let mut ccs = ccs;
            ccs.sort_unstable();
            ccs.dedup();
            (ccs.len() == 1).then(|| ccs[0])
        };
        let shape = &self.instrument.shape;
        self.spec.tone_cc.unwrap_or_else(|| {
            one(shape.cutoff_cc.iter().map(|m| m.cc).collect())
                .or_else(|| one(self.instrument.regions.iter().flat_map(|r| r.mods.iter().map(|m| m.src.cc)).collect()))
                .unwrap_or(74)
        })
    }
}

/// Sampler resonance (0..1) for an SFZ / SF2 resonance peak in dB.
fn resonance(db: f32) -> f32 {
    // The sampler's SVF damping is k = sqrt2 * (1 - 0.985 r), Q = 1 / k.
    // 0 dB is the plain (Butterworth, Q = 0.707) filter; each dB above
    // raises Q by that much.
    let q = std::f32::consts::FRAC_1_SQRT_2 * 10f32.powf(db.max(0.0) / 20.0);
    ((1.0 - 1.0 / (q * std::f32::consts::SQRT_2)) / 0.985).clamp(0.0, 1.0)
}

fn cc_map(map: &[CcMap], scale: f64) -> [CcMap; MAX_CC_MAPS] {
    let mut out = [CcMap::NONE; MAX_CC_MAPS];
    for (slot, m) in out.iter_mut().zip(map.iter().filter(|m| m.cc < 128)) {
        *slot = CcMap { amount: (m.amount as f64 * scale) as f32, ..*m };
    }
    out
}

/// The lane sampler settings of an instrument's shape. `frame_scale` turns
/// the file's sample frames into engine frames (the `offset` CC maps).
pub fn sound_shape(shape: &Shape, tone_cc: u8, morph_cc: u8, frame_scale: f64) -> SoundShape {
    let mut cc_init = [(NO_CC, 0u8); 8];
    for (slot, (&cc, &v)) in cc_init.iter_mut().zip(shape.cc_init.iter().filter(|(cc, _)| **cc < 128)) {
        *slot = (cc, v.min(127));
    }
    let modulation = ModConfig {
        vib_depth: shape.vibrato_depth,
        vib_rate: if shape.vibrato_rate > 0.0 { shape.vibrato_rate } else { ModConfig::default().vib_rate },
        vib_delay: shape.vibrato_delay,
        vib_fade: shape.vibrato_fade,
        cutoff_cc: cc_map(&shape.cutoff_cc, 1.0),
        volume_cc: cc_map(&shape.volume_cc, 1.0),
        vibrato_cc: cc_map(&shape.vibrato_cc, 1.0),
        offset_cc: cc_map(&shape.offset_cc, frame_scale),
        tone_cc: tone_cc.min(127),
        morph_cc: morph_cc.min(127),
        cc_init,
        env: shape.mod_env,
        lfo: shape.mod_lfo,
    };
    let filter_morph = match shape.filter {
        FilterKind::None | FilterKind::Unsupported => None,
        // Slope morph pinned to one end: 6 dB/oct below the low corner, 24 above the high one.
        FilterKind::Lowpass1 => Some(Some((CUTOFF_MAX_HZ * 4.0, CUTOFF_MAX_HZ * 8.0))),
        FilterKind::Lowpass2 => Some(None),
        FilterKind::Lowpass4 => Some(Some((1.0, 2.0))),
    };
    let filtered = !matches!(shape.filter, FilterKind::Unsupported);
    SoundShape {
        // SF2 / SFZ hold sits at full level before the decay; folded into the attack.
        attack: shape.attack.map(|a| a + shape.hold.unwrap_or(0.0)),
        decay: shape.decay,
        sustain: shape.sustain.map(|s| s.clamp(0.0, 1.0)),
        release: shape.release,
        cutoff_hz: shape.cutoff.filter(|_| filtered).map(|c| c.clamp(20.0, CUTOFF_MAX_HZ)),
        resonance: shape.resonance.filter(|_| filtered).map(resonance),
        filter_morph,
        modulation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mesh-pack-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_sounds_and_scans_folders() {
        let root = tmp("scan");
        std::fs::create_dir_all(root.join("a/phrases")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("a/sound.sfz"), "<region> sample=x.wav").unwrap();
        std::fs::write(root.join("a/phrases/riff.mid"), b"").unwrap();
        std::fs::write(root.join("a/loose.MID"), b"").unwrap();
        std::fs::write(root.join("b/Reese.sf2"), b"").unwrap();
        assert_eq!(scan(&root), vec![root.join("a"), root.join("b")]);
        assert_eq!(find_sound(&root.join("b")), Some(root.join("b/Reese.sf2")));
        assert_eq!(phrase_files(&root.join("a")), vec![root.join("a/phrases/riff.mid"), root.join("a/loose.MID")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn loads_an_sfz_pack_with_phrases_and_settings() {
        use crate::live::motif::midi;
        let root = tmp("load");
        std::fs::create_dir_all(root.join("phrases")).unwrap();
        std::fs::write(
            root.join("sound.sfz"),
            "<control> label_cc20=Bite\n<global> cutoff=2000 cutoff_oncc20=2400 ampeg_release=0.3\n<region> sample=s.wav pitch_keycenter=48",
        )
        .unwrap();
        std::fs::write(root.join(PACK_FILE), "glide: 0.1\ngain_db: -3\n").unwrap();
        use crate::live::motif::{Clip, RawNote};
        let step = crate::live::TICKS_PER_STEP as u32;
        let clip = Clip { bars: 1, notes: vec![RawNote { start: 0, len: 4 * step, pitch: 48, vel: 100 }, RawNote { start: 8 * step, len: 4 * step, pitch: 51, vel: 90 }] };
        std::fs::write(root.join("phrases/one.mid"), midi::write(&clip, "one")).unwrap();
        std::fs::write(root.join("phrases/bad.mid"), b"nope").unwrap();
        let pack = Pack::load(&root).unwrap();
        assert_eq!(pack.phrases.len(), 1);
        assert!(pack.warnings.iter().any(|w| w.contains("bad.mid")));
        assert_eq!(pack.tone_cc(), 20, "the only cutoff CC drives Tone");
        assert_eq!(pack.spec.glide, Some(0.1));
        let shape = sound_shape(&pack.instrument.shape, pack.tone_cc(), 20, 1.0);
        assert_eq!(shape.cutoff_hz, Some(2000.0));
        assert_eq!(shape.release, Some(0.3));
        assert_eq!(shape.modulation.cutoff_cc[0], CcMap::new(20, 2400.0));
        assert_eq!(shape.modulation.tone_cc, 20);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resonance_maps_peak_db() {
        assert_eq!(resonance(0.0), 0.0);
        assert!(resonance(12.0) > 0.7 && resonance(12.0) < 1.0);
        assert!(resonance(40.0) <= 1.0);
    }
}
