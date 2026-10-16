//! Style loading into RAM (design §5, §9, D36).
//!
//! Turns a validated [`Style`] into everything the engine needs, on the
//! loader thread:
//!
//! - every sample decoded and resampled to the engine rate, shared as
//!   `basedrop::Shared<SampleData>` (a file used by several options is
//!   decoded once),
//! - one [`SampleBank`] per instrument option and per drum kit,
//! - CLAP instruments instantiated with their state files (at most
//!   [`MAX_CLAP_LANES`] lanes, D32),
//! - each lane's FX chain built through the existing effect host,
//! - control mappings resolved to indices, glue parameters mapped from the
//!   style's [`GlueSpec`].
//!
//! After this, kit and instrument swaps never touch disk.
//!
//! # Loudness
//!
//! Every sample is first normalized to [`SAMPLE_LOUDNESS_TARGET`] integrated
//! LUFS when it loads, so samples of one pool (hats, perc, ...) and the kick
//! and snare of a pair sit at the same level.
//!
//! An option's `lufs_trim_db` is applied at the instrument output, before the
//! lane FX: as the sampler's lane gain (instrument lanes), as each slot's
//! gain (drum kits, one trim per sub-voice option) or as the CLAP output
//! gain. `style-calibrate` measures at the same point, so a calibrated option
//! hits its role target going into the FX chain and the glue.
//!
//! # Drum kits
//!
//! The drum lane has one option list per sub-voice. Kit `k` selects, for the
//! backbone and break sub-voices, option `(default_option + k) % options.len()`,
//! so kit 0 is the style's default and turning the encoder cycles both. Pool
//! sub-voices (hats, ride, shaker, perc, top) always load their default
//! option; the seed picks samples inside it (Q4).

use crate::live::generator::layout::DrumVoice;
use std::path::PathBuf;
use std::sync::Arc;

use basedrop::Shared;

use crate::clap::ClapManager;
use crate::effect::Effect;
use crate::engine::gc::gc_handle;
use crate::music::MusicalKey;
use crate::pd::PdManager;
use crate::types::StereoBuffer;

use super::command::{drive_db, GlueControl};
use super::generator::{self, DrumLayout, LaneGenerator, MAX_GEN_PARAMS};
use super::glue::loudness::{measure_hit_lufs, measure_lufs};
use super::glue::{db_to_lin, GlueParams, Role};
use super::form::Form;
use super::harmony::Progression;
use super::fx::{airwindows_state, setting_key, setting_param, AIRWINDOWS_ID};
use super::lane::{
    drum_target_range, native_effect, ClapCell, FxSlot, Lane, LaneFx, LaneInit, LaneOption, OptionFamily,
};
use super::pack::{self, Pack};
use super::sampler::{Interpolation, SampleBank, SampleData, Slice, SliceType, SlotDef, SoundShape, ZoneTarget};
use super::sfz::{LoopMode, Source};
use super::style::{
    self, ControlSpec, ControlTarget, FxSource, FxSpec, GlueSpec, InstrumentOption, InstrumentSource, SampleSpec,
    SendBus, SendFxSpec, SendLevels, Style, TempoSpec, CONTROLS_PER_LANE,
};
use super::NUM_LANES;

/// Lanes that may host CLAP instruments. D32 capped this at 2 for the board;
/// user (2026-10-16): develop on the host without the cap, trim if the CPU
/// cannot hold it.
pub const MAX_CLAP_LANES: usize = NUM_LANES;
/// Voice pool size per lane.
// MUSICAL: starting budgets, to be fixed from the board benchmark (D32).
pub const DEFAULT_VOICES: [usize; NUM_LANES] = [24, 8, 12, 12];
/// Largest block the engine renders in one pass.
pub const DEFAULT_MAX_BLOCK: usize = 8192;
/// Root key assumed for samples without one (unpitched).
const UNPITCHED_ROOT: u8 = 60;

/// How CLAP plugins are found.
#[derive(Debug, Clone, Default)]
pub enum ClapMode {
    /// Skip CLAP entirely: CLAP options and effects become unavailable.
    #[default]
    Disabled,
    /// Scan the standard CLAP directories plus `extra_paths`.
    Scan { extra_paths: Vec<PathBuf> },
}

/// Loader settings.
pub struct LoadOptions {
    pub sample_rate: u32,
    pub max_block: usize,
    pub interpolation: Interpolation,
    pub voices: [usize; NUM_LANES],
    pub clap: ClapMode,
    /// Collection root, needed for Pure Data effects.
    pub collection: Option<PathBuf>,
    /// Replace a lane's generator (fixed patterns for sign-off and tests).
    pub generators: [Option<Box<dyn LaneGenerator>>; NUM_LANES],
    /// Loudness calibration: load every option at unity gain and without
    /// lane FX, so `style-calibrate` measures the raw instrument.
    pub calibration: bool,
}

impl LoadOptions {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            max_block: DEFAULT_MAX_BLOCK,
            interpolation: Interpolation::default(),
            voices: DEFAULT_VOICES,
            clap: ClapMode::Disabled,
            collection: None,
            generators: std::array::from_fn(|_| None),
            calibration: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("cannot decode sample {path}: {reason}")]
    Sample { path: PathBuf, reason: String },
    #[error("style has {0} lanes, expected {NUM_LANES}")]
    LaneCount(usize),
}

/// Where a performance control goes, resolved to indices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlRoute {
    /// Index into the lane generator's parameters.
    Generator(usize),
    /// Instrument parameter (CLAP param index or [`SamplerParam`](super::command::SamplerParam)).
    Instrument(u32),
    Fx { slot: u8, param: u32 },
    Glue(GlueControl),
    Send(SendBus),
    /// The style names something this build does not have.
    Unresolved,
}

/// One performance control of a lane.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedControl {
    pub label: String,
    pub spec: ControlSpec,
    pub route: ControlRoute,
    /// The `also` targets, resolved.
    pub also: Vec<(ControlSpec, ControlRoute)>,
}

/// One drum kit as the generator sees it (on a pack lane: one option and
/// its phrase generator).
pub struct DrumKit {
    /// Which targets exist in this kit (`generator::layout`).
    pub layout: DrumLayout,
    /// Generator built for this kit's layout. `None` = the lane's generator
    /// (kit 0, or a generator override).
    pub generator: Option<Box<dyn LaneGenerator>>,
}

/// Non-RT description of a loaded lane.
pub struct LaneMeta {
    pub name: String,
    pub role: Role,
    /// Display name per option / kit.
    pub option_names: Vec<String>,
    pub option_available: Vec<bool>,
    /// Kind of sound per option; a form swap (`*`) stays inside one kind.
    pub option_family: Vec<OptionFamily>,
    pub default_option: usize,
    pub controls: [ResolvedControl; CONTROLS_PER_LANE],
    /// Shift-layer controls (up to [`CONTROLS_PER_LANE`]).
    pub shift_controls: Vec<ResolvedControl>,
    /// The lane's centre speed (the rate pads go from here).
    /// Centre speed as a power of two (see [`super::style::RateSpec::exponent`]).
    pub base_rate: i8,
    pub sends: SendLevels,
    pub generator: Box<dyn LaneGenerator>,
    /// Generator parameters after the style's defaults.
    pub gen_params: [f32; MAX_GEN_PARAMS],
    /// Drum lane: target layout (and generator) per kit, index = option.
    pub drum_kits: Vec<DrumKit>,
}

/// A style loaded into RAM.
pub struct LoadedStyle {
    pub name: String,
    pub sample_rate: u32,
    pub max_block: usize,
    pub tempo: TempoSpec,
    pub key: MusicalKey,
    pub progressions: Vec<Progression>,
    pub default_progression: usize,
    /// Lane forms (never empty).
    pub forms: Vec<Form>,
    pub default_form: usize,
    pub glue: GlueParams,
    /// Insert chains of the drum bus and master (style `glue.drum_bus_fx` / `master_fx`).
    pub drum_bus_fx: LaneFx,
    pub master_fx: LaneFx,
    /// RT lanes, index = lane.
    pub lanes: Vec<Lane>,
    pub meta: Vec<LaneMeta>,
    /// Every CLAP instrument, for housekeeping (`ClapCell::idle`).
    pub claps: Vec<Arc<ClapCell>>,
    /// Things that could not be loaded. The style still plays.
    pub warnings: Vec<String>,
    /// Decoded sample memory in bytes.
    pub sample_bytes: usize,
}

// MUSICAL: tune by ear (user: every sound and lane at a consistent level).
/// Every sample is brought to this integrated loudness (LUFS) when the style
/// loads. Hits shorter than one 400 ms gating block are measured over their
/// own length ([`measure_hit_lufs`]). Option trims and the lane
/// calibration then set the balance on top, so a pool of hats plays every
/// hat at the same level.
pub const SAMPLE_LOUDNESS_TARGET: f64 = -12.0;
/// Largest normalization cut and boost in dB (a quiet file with a loud
/// click must not come up so far that the click dominates).
const SAMPLE_CUT_MAX_DB: f64 = 24.0;
const SAMPLE_BOOST_MAX_DB: f64 = 12.0;

/// Integrated loudness of a decoded sample (LUFS). Hits shorter than one
/// gating block are measured over their own length. `-inf` for silence.
pub fn measure_sample_lufs(data: &SampleData) -> f64 {
    let rate = data.sample_rate() as f64;
    let block = (rate * 0.4).ceil() as usize;
    let frames: Vec<crate::types::StereoSample> = (0..data.len()).map(|i| data.frame(i)).collect();
    // Short hits over their own length (padding would read them as quiet).
    if frames.len() < block { measure_hit_lufs(&frames, rate) } else { measure_lufs(&frames, rate) }
}

/// Loudness of a file as the loader sees it: decoded and resampled to `rate`.
pub fn analyse_sample_file(path: &std::path::Path, rate: u32) -> Result<f64, crate::audio_file::AudioFileError> {
    let data = SampleData::decode(path, UNPITCHED_ROOT)?.resampled_to(rate)?;
    Ok(measure_sample_lufs(&data))
}

/// Gain that brings a sample of loudness `measured` to [`SAMPLE_LOUDNESS_TARGET`].
fn gain_for_lufs(measured: f64) -> f32 {
    if !measured.is_finite() {
        return 1.0;
    }
    let db = (SAMPLE_LOUDNESS_TARGET - measured).clamp(-SAMPLE_CUT_MAX_DB, SAMPLE_BOOST_MAX_DB);
    10f64.powf(db / 20.0) as f32
}

/// Gain that brings `data` to [`SAMPLE_LOUDNESS_TARGET`] LUFS.
fn sample_gain(data: &SampleData) -> f32 {
    gain_for_lufs(measure_sample_lufs(data))
}

struct Ctx<'a> {
    style: &'a Style,
    rate: u32,
    max_block: usize,
    /// Decoded samples with their loudness gain.
    cache: Vec<(PathBuf, u8, Vec<style::SliceSpec>, Shared<SampleData>, f32)>,
    /// Normalize every sample to [`SAMPLE_LOUDNESS_TARGET`].
    normalize: bool,
    sample_bytes: usize,
    clap: Option<ClapManager>,
    clap_mode: ClapMode,
    pd: Option<PdManager>,
    collection: Option<PathBuf>,
    warnings: Vec<String>,
}

impl Ctx<'_> {
    fn warn(&mut self, msg: String) {
        log::warn!("[LIVE] {msg}");
        self.warnings.push(msg);
    }

    /// The lane's motif clips: the files its `motifs` field lists plus the
    /// style folder's `motifs/bass/*.mid` (bass lane) or `motifs/lead/*.mid`
    /// (lead lane). Unreadable files are warnings.
    fn motifs(&mut self, lane: &style::LaneSpec) -> Vec<crate::live::motif::Motif> {
        use crate::live::motif::{self, Voice};
        let folder = match lane.role {
            Role::Bass => Some("bass"),
            Role::Lead => Some("lead"),
            _ => None,
        };
        let mut paths: Vec<PathBuf> = lane.motifs.iter().map(|p| self.style.asset_path(p)).collect();
        if let Some(folder) = folder {
            paths.extend(motif::scan_dir(&self.style.dir.join("motifs").join(folder)));
        }
        if paths.is_empty() {
            return Vec::new();
        }
        let voice = if lane.role == Role::Bass { Voice::Lowest } else { Voice::Highest };
        let (motifs, warnings) = motif::load_files(&paths, voice);
        for w in warnings {
            self.warn(format!("lane '{}': {w}", lane.name));
        }
        motifs
    }

    /// Decoded, resampled sample with its slice table and the gain that
    /// brings it to [`SAMPLE_LOUDNESS_TARGET`]. Cached per
    /// `(file, root key, slices)`.
    fn sample(&mut self, spec: &SampleSpec) -> Result<(Shared<SampleData>, f32), LoadError> {
        let path = self.style.asset_path(&spec.file);
        let root = spec.root_key.map(|n| n.0).unwrap_or(UNPITCHED_ROOT);
        if let Some(hit) = self.cache.iter().find(|c| c.0 == path && c.1 == root && c.2 == spec.slices) {
            return Ok((hit.3.clone(), hit.4));
        }
        let fail = |e: crate::audio_file::AudioFileError| LoadError::Sample { path: path.clone(), reason: e.to_string() };
        let data = SampleData::decode(&path, root).map_err(fail)?;
        // A slice runs to the next slice's start, the last one to the file end.
        let len = data.len();
        let slices: Vec<Slice> = spec
            .slices
            .iter()
            .enumerate()
            .map(|(i, s)| Slice {
                start: s.start as usize,
                end: spec.slices.get(i + 1).map(|n| n.start as usize).unwrap_or(len),
                kind: slice_type(s.kind),
            })
            .collect();
        let data = data.with_slices(slices).resampled_to(self.rate).map_err(fail)?;
        self.sample_bytes += data.len() * std::mem::size_of::<crate::types::StereoSample>();
        let shared = Shared::new(&gc_handle(), data);
        // A stored measurement (style-calibrate --analyse) saves measuring at every load.
        let gain = match (self.normalize, spec.lufs) {
            (false, _) => 1.0,
            (true, Some(lufs)) => gain_for_lufs(lufs as f64),
            (true, None) => sample_gain(&shared),
        };
        self.cache.push((path, root, spec.slices.clone(), shared.clone(), gain));
        Ok((shared, gain))
    }

    /// Map an option's samples to consecutive targets from `first`. Returns
    /// the next free target.
    #[allow(clippy::too_many_arguments)]
    fn fill_bank(
        &mut self,
        bank: &mut SampleBank,
        first: u8,
        capacity: u8,
        samples: &[SampleSpec],
        default_choke: u8,
        gain: f32,
        mono_groups: bool,
    ) -> Result<u8, LoadError> {
        let end = first as usize + capacity as usize;
        let mut next = first as usize;
        let mut prev_zoned = false;
        for spec in samples {
            let (data, norm) = self.sample(spec)?;
            let gain = gain * norm * db_to_lin(spec.gain_db);
            let choke = if spec.choke != 0 { spec.choke } else { default_choke };
            if choke != 0 && mono_groups {
                bank.set_mono(choke, true);
            }
            let finish = |slot: SlotDef| {
                let slot = slot.with_choke(choke).with_gain(gain);
                let slot = if spec.stream { slot.streamed() } else { slot };
                if spec.root_key.is_some() { slot } else { slot.unpitched() }
            };
            if !spec.slices.is_empty() {
                prev_zoned = false;
                for i in 0..data.slices.len() {
                    if next >= end {
                        break;
                    }
                    if let Some(slot) = SlotDef::slice(data.clone(), i) {
                        bank.set(next as u8, finish(slot));
                        next += 1;
                    }
                }
            } else if let Some(zone) = spec.zone {
                // Consecutive zoned samples are key zones of one target.
                if !prev_zoned {
                    if next >= end {
                        break;
                    }
                    next += 1;
                }
                prev_zoned = true;
                bank.add_zone((next - 1) as u8, finish(SlotDef::whole(data)).with_keys(zone.keys[0].0, zone.keys[1].0));
            } else {
                prev_zoned = false;
                if next >= end {
                    break;
                }
                bank.set(next as u8, finish(SlotDef::whole(data)));
                next += 1;
            }
        }
        if next >= end && samples.len() > capacity as usize {
            self.warn(format!("more samples than the {capacity} targets available from target {first}"));
        }
        Ok(next as u8)
    }

    /// A sound pack as a lane option: one bank target holding every zone
    /// (keys, velocity layers, round robin, loops, tune, pan), one loudness
    /// gain for the whole pack (so velocity layers keep their balance), the
    /// instrument's shape, and a `phrase` generator over the pack's phrases.
    fn pack_option(
        &mut self,
        lane: &style::LaneSpec,
        dir: &std::path::Path,
    ) -> Result<(Pack, Shared<SampleBank>, SoundShape, generator::phrase::PhraseGenerator), String> {
        let pack = Pack::load(dir)?;
        for w in &pack.warnings {
            self.warn(format!("pack '{}': {w}", pack.name));
        }
        let rate = self.rate;
        // Decoded sources at the engine rate, with their frame scale.
        let mut files: Vec<(PathBuf, Shared<SampleData>, f64)> = Vec::new();
        let mut embedded: Vec<Option<(Shared<SampleData>, f64)>> = vec![None; pack.instrument.embedded.len()];
        let mut loudest = f64::NEG_INFINITY;
        let mut slots: Vec<SlotDef> = Vec::new();
        let mut release_zones = 0usize;
        let mut frame_scale = None;
        let mono = pack.spec.mono.unwrap_or(lane.role == Role::Bass);
        for region in &pack.instrument.regions {
            if region.release_trigger {
                release_zones += 1;
                continue;
            }
            let decoded: Result<(Shared<SampleData>, f64), String> = match &region.source {
                Source::File(path) => match files.iter().find(|f| &f.0 == path) {
                    Some(f) => Ok((f.1.clone(), f.2)),
                    None => SampleData::decode(path, UNPITCHED_ROOT)
                        .and_then(|d| {
                            let ratio = rate as f64 / d.sample_rate().max(1) as f64;
                            d.resampled_to(rate).map(|d| (d, ratio))
                        })
                        .map_err(|e| format!("{}: {e}", path.display()))
                        .map(|(d, ratio)| {
                            loudest = loudest.max(measure_sample_lufs(&d));
                            self.sample_bytes += d.len() * std::mem::size_of::<crate::types::StereoSample>();
                            let shared = Shared::new(&gc_handle(), d);
                            files.push((path.clone(), shared.clone(), ratio));
                            (shared, ratio)
                        }),
                },
                Source::Embedded(i) => match (embedded.get(*i).cloned().flatten(), pack.instrument.embedded.get(*i)) {
                    (Some(hit), _) => Ok(hit),
                    (None, Some(e)) => {
                        let right = e.right.as_deref().unwrap_or(&[]);
                        let ratio = rate as f64 / e.rate.max(1) as f64;
                        SampleData::from_channels(&e.left, right, e.rate.max(1), UNPITCHED_ROOT)
                            .resampled_to(rate)
                            .map_err(|err| format!("sample '{}': {err}", e.name))
                            .map(|d| {
                                loudest = loudest.max(measure_sample_lufs(&d));
                                self.sample_bytes += d.len() * std::mem::size_of::<crate::types::StereoSample>();
                                let shared = Shared::new(&gc_handle(), d);
                                embedded[*i] = Some((shared.clone(), ratio));
                                (shared, ratio)
                            })
                    }
                    (None, None) => Err(format!("zone points at missing sample {i}")),
                },
            };
            let (data, ratio) = match decoded {
                Ok(d) => d,
                Err(e) => {
                    self.warn(format!("pack '{}': {e}", pack.name));
                    continue;
                }
            };
            frame_scale.get_or_insert(ratio);
            let len = data.len();
            let at = |frames: u32| ((frames as f64 * ratio).round() as usize).min(len);
            let mut slot = SlotDef::whole(data);
            slot.root_key = region.keycenter;
            slot.root_cents = (-region.tune).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            slot.key_lo = region.lokey;
            slot.key_hi = region.hikey;
            slot.vel_lo = region.lovel;
            slot.vel_hi = region.hivel;
            slot.seq_len = region.seq_length.max(1);
            slot.seq_pos = region.seq_position.clamp(1, slot.seq_len);
            match region.loop_mode {
                LoopMode::Continuous | LoopMode::Sustain => {
                    let lo = at(region.loop_start.unwrap_or(0));
                    let hi = region.loop_end.map_or(len, |e| at(e.saturating_add(1)));
                    if hi > lo + 1 {
                        slot.loop_range = Some((lo, hi));
                        slot.loop_sustain = region.loop_mode == LoopMode::Sustain;
                    }
                }
                LoopMode::OneShot => slot.one_shot = true,
                LoopMode::NoLoop => {}
            }
            slot.start_offset = at(region.offset);
            slot.pan = (region.pan / 100.0).clamp(-1.0, 1.0);
            for (dst, m) in slot.zone_mods.iter_mut().zip(&region.mods) {
                *dst = *m;
                // Start offsets are in the file's frames.
                if m.target == ZoneTarget::Offset {
                    dst.src.amount = (m.src.amount as f64 * ratio) as f32;
                }
            }
            slot.gain = db_to_lin(region.volume);
            if mono {
                slot.choke = 1;
            }
            slots.push(slot);
        }
        if release_zones > 0 {
            self.warn(format!("pack '{}': {release_zones} release-trigger zones skipped", pack.name));
        }
        if slots.is_empty() {
            return Err(format!("pack '{}': no zone could be loaded", pack.name));
        }
        let gain = if self.normalize { gain_for_lufs(loudest) } else { 1.0 } * db_to_lin(pack.spec.gain_db);
        let mut bank = SampleBank::new();
        if mono {
            bank.set_mono(1, true);
        }
        for mut slot in slots {
            slot.gain *= gain;
            bank.add_zone(0, slot);
        }
        let shape = pack::sound_shape(
            &pack.instrument.shape,
            pack.tone_cc(),
            pack.spec.morph_cc.unwrap_or(20),
            frame_scale.unwrap_or(1.0),
        );
        let phrases = pack.phrases.clone();
        let gen = generator::registry::build_phrase(&lane.generator, phrases, pack.spec.glide);
        Ok((pack, Shared::new(&gc_handle(), bank), shape, gen))
    }

    fn clap_manager(&mut self) -> Option<&mut ClapManager> {
        if self.clap.is_none() {
            let ClapMode::Scan { extra_paths } = &self.clap_mode else { return None };
            let mut manager = ClapManager::new();
            for p in extra_paths {
                manager.add_search_path(p.clone());
            }
            // Plugins that travel with the collection (local or USB): the
            // configured one and the one the style folder sits in.
            let style_collection = self
                .style
                .dir
                .parent()
                .filter(|p| p.file_name().is_some_and(|n| n == super::style::STYLES_FOLDER))
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf());
            for collection in self.collection.iter().chain(style_collection.iter()) {
                for dir in ClapManager::collection_plugin_dirs(collection) {
                    manager.add_search_path(dir);
                }
            }
            manager.scan_plugins();
            self.clap = Some(manager);
        }
        self.clap.as_mut()
    }

    fn build_effect(&mut self, spec: &FxSpec) -> Result<Box<dyn Effect>, String> {
        // CLAP parameters as the plugin reports them after its state loaded
        // (an Airwindows algorithm renames them), for `settings` by name.
        let mut clap_info: Option<crate::effect::EffectInfo> = None;
        let mut fx: Box<dyn Effect> = match spec.source {
            FxSource::Native => native_effect(&spec.id).ok_or_else(|| "unknown native effect".to_string())?,
            FxSource::Clap | FxSource::Airwindows => {
                let airwindows = spec.source == FxSource::Airwindows;
                let id = if airwindows { AIRWINDOWS_ID } else { spec.id.as_str() };
                let preset = spec.preset.as_ref().map(|p| self.style.asset_path(p));
                let (fx, gui) = self
                    .clap_manager()
                    .ok_or_else(|| "CLAP is disabled".to_string())?
                    .create_effect_with_gui_handle(id)
                    .map_err(|e| e.to_string())?;
                let mut fx: Box<dyn Effect> = fx;
                let defaults = {
                    let mut w = gui.wrapper.lock().map_err(|_| "plugin lock poisoned".to_string())?;
                    let defaults = {
                    if airwindows {
                        w.load_state(&airwindows_state(&spec.id)).map_err(|e| e.to_string())?;
                    }
                    if let Some(path) = &preset {
                        w.load_state_from_file(path).map_err(|e| e.to_string())?;
                    }
                    // A state without parameter values leaves an Airwindows
                    // algorithm at 0 everywhere: start from its defaults.
                    if airwindows && preset.is_none() {
                        w.query_params()
                            .iter()
                            .map(|p| {
                                // Unused slots are named "-".
                                (p.name != "-" && p.max > p.min).then(|| ((p.default - p.min) / (p.max - p.min)) as f32)
                            })
                            .collect()
                    } else {
                        Vec::new()
                    }
                    };
                    let mut info = crate::effect::EffectInfo::new(id, "CLAP");
                    for p in w.query_params() {
                        info = info.with_param(crate::effect::ParamInfo::new(p.name, 0.0).with_range(p.min as f32, p.max as f32));
                    }
                    clap_info = Some(info);
                    defaults
                };
                for (i, d) in defaults.into_iter().enumerate() {
                    if let Some(d) = d {
                        fx.set_param(i, d);
                    }
                }
                fx
            }
            FxSource::Pd => {
                if self.pd.is_none() {
                    let collection = self.collection.clone().ok_or_else(|| "no collection path for PD effects".to_string())?;
                    self.pd = Some(PdManager::new(&collection).map_err(|e| e.to_string())?);
                }
                match self.pd.as_mut() {
                    Some(pd) => pd.create_effect(&spec.id).map_err(|e| e.to_string())?,
                    None => return Err("PD is unavailable".to_string()),
                }
            }
        };
        for (&index, &value) in &spec.params {
            fx.set_param(index as usize, value);
        }
        for (name, &value) in &spec.settings {
            let info = clap_info.as_ref().unwrap_or_else(|| fx.info());
            match setting_param(info, name, value) {
                Some((index, normalized)) => fx.set_param(index, normalized),
                None => {
                    let known: Vec<String> =
                        info.params.iter().map(|p| setting_key(&p.name)).filter(|k| !k.is_empty()).collect();
                    self.warn(format!("effect '{}': unknown setting '{name}' (known: {})", spec.id, known.join(", ")));
                }
            }
        }
        fx.set_bypass(spec.bypassed);
        // Let the effect size its buffers now instead of on the first RT block.
        let mut warm = StereoBuffer::silence(self.max_block);
        fx.process(&mut warm);
        fx.reset();
        Ok(fx)
    }

    fn build_fx(&mut self, lane: &str, specs: &[FxSpec]) -> LaneFx {
        let slots = specs
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let effect = match self.build_effect(spec) {
                    Ok(fx) => Some(fx),
                    Err(e) => {
                        self.warn(format!("lane '{lane}' fx[{i}] '{}' not loaded: {e}", spec.id));
                        None
                    }
                };
                FxSlot { effect, dry_wet: spec.dry_wet }
            })
            .collect();
        LaneFx::new(slots, self.max_block)
    }
}

fn slice_type(kind: style::SliceType) -> SliceType {
    match kind {
        style::SliceType::Kick => SliceType::Kick,
        style::SliceType::Snare => SliceType::Snare,
        style::SliceType::Ghost => SliceType::Ghost,
        style::SliceType::Hat => SliceType::Hat,
        style::SliceType::Perc => SliceType::Perc,
        style::SliceType::Other => SliceType::Other,
    }
}

/// Map the style's glue section onto the runtime parameters. Unset fields
/// keep the runtime defaults.
pub fn glue_params(spec: &GlueSpec, send_fx: &SendFxSpec, bpm: f64) -> (GlueParams, Vec<String>) {
    let mut p = GlueParams::default();
    let mut warnings = Vec::new();
    if let Some(h) = &spec.role_hpf {
        for (freq, hp) in [(h.drums, &mut p.high_pass.drums), (h.lead, &mut p.high_pass.lead), (h.extra, &mut p.high_pass.extra)] {
            if let Some(hz) = freq {
                hp.enabled = true;
                hp.freq_hz = hz;
            }
        }
    }
    if let Some(hz) = spec.bass_mono_hz {
        p.mono_below.enabled = true;
        p.mono_below.freq_hz = hz;
    }
    if let Some(d) = &spec.duck {
        if let Some(on) = d.enabled {
            p.duck.enabled = on;
        }
        if let Some(ms) = d.attack_ms {
            p.duck.attack_ms = ms;
        }
        for (group, out) in [(&d.bass, &mut p.duck.bass), (&d.others, &mut p.duck.others)] {
            if let Some(g) = group {
                if let Some(db) = g.kick_db {
                    out.kick_db = db;
                }
                if let Some(db) = g.snare_db {
                    out.snare_db = db;
                }
                if let Some(ms) = g.release_ms {
                    out.release_ms = ms;
                }
            }
        }
    }
    if let Some(b) = &spec.band_duck {
        let o = &mut p.band_duck;
        o.enabled = b.enabled.unwrap_or(o.enabled);
        o.splits_hz = b.splits_hz.unwrap_or(o.splits_hz);
        o.threshold_db = b.threshold_db.unwrap_or(o.threshold_db);
        o.ratio = b.ratio.unwrap_or(o.ratio);
        o.max_db = b.max_db.unwrap_or(o.max_db);
        o.attack_ms = b.attack_ms.unwrap_or(o.attack_ms);
        o.release_ms = b.release_ms.unwrap_or(o.release_ms);
        o.adaptive_release = b.adaptive_release.unwrap_or(o.adaptive_release);
        o.adapt_ms = b.adapt_ms.unwrap_or(o.adapt_ms);
    }
    let comp = |c: &style::CompSpec, out: &mut super::glue::CompressorParams| {
        out.threshold_db = c.threshold_db.unwrap_or(out.threshold_db);
        out.ratio = c.ratio.unwrap_or(out.ratio);
        out.attack_ms = c.attack_ms.unwrap_or(out.attack_ms);
        out.release_ms = c.release_ms.unwrap_or(out.release_ms);
        out.makeup_db = c.makeup_db.unwrap_or(out.makeup_db);
    };
    if let Some(b) = &spec.drum_bus {
        if let Some(c) = &b.comp {
            comp(c, &mut p.drum_bus.compressor);
        }
        if let Some(c) = &b.comp2 {
            comp(c, &mut p.drum_bus.compressor2);
        }
        if let Some(drive) = b.drive {
            p.drum_bus.saturation_enabled = drive > 0.0;
            p.drum_bus.saturation.drive_db = drive_db(drive);
        }
    }
    if let Some(sd) = &spec.shared_drive {
        p.drive.enabled = sd.enabled.unwrap_or(p.drive.enabled);
        if let Some(drive) = sd.drive {
            p.drive.saturation.drive_db = drive_db(drive);
        }
        p.drive.saturation.mix = sd.mix.unwrap_or(p.drive.saturation.mix);
    }
    if let Some(c) = &spec.master {
        comp(c, &mut p.master.compressor);
    }

    // The shared sends are the glue chain's built-in delay and reverb. A
    // native send_fx entry sets their parameters; other sources are not hosted.
    if let Some(fx) = &send_fx.delay {
        if fx.source == FxSource::Native && fx.id == "delay" {
            if let Some(&time) = fx.params.get(&0) {
                // DelayEffect time is 10..2000 ms; the send is tempo-synced.
                let ms = 10.0 + time * 1990.0;
                p.delay.beats = (ms / 1000.0 * bpm as f32 / 60.0).clamp(0.03125, 4.0);
            }
            if let Some(&fb) = fx.params.get(&1) {
                p.delay.feedback = fb * 0.95;
            }
            if let Some(&pp) = fx.params.get(&3) {
                p.delay.ping_pong = pp >= 0.5;
            }
        } else {
            warnings.push(format!("send_fx.delay '{}' is not hosted; using the built-in delay", fx.id));
        }
    }
    if let Some(fx) = &send_fx.reverb {
        if fx.source == FxSource::Native && fx.id == "reverb" {
            p.reverb.room_size = fx.params.get(&0).copied().unwrap_or(p.reverb.room_size);
            p.reverb.damping = fx.params.get(&1).copied().unwrap_or(p.reverb.damping);
            p.reverb.width = fx.params.get(&2).copied().unwrap_or(p.reverb.width);
        } else {
            warnings.push(format!("send_fx.reverb '{}' is not hosted; using the built-in reverb", fx.id));
        }
    }
    (p, warnings)
}

fn resolve_control(spec: &ControlSpec, generator: &dyn LaneGenerator, fx_len: usize) -> (ResolvedControl, Option<String>) {
    let mut problem = None;
    let (route, name) = match &spec.target {
        ControlTarget::Generator { param } => {
            let route = match generator.param_names().iter().position(|n| n == param) {
                Some(i) if i < MAX_GEN_PARAMS => ControlRoute::Generator(i),
                _ => {
                    problem = Some(format!("generator has no parameter '{param}'"));
                    ControlRoute::Unresolved
                }
            };
            (route, param.clone())
        }
        ControlTarget::Instrument { param } => (ControlRoute::Instrument(*param), format!("inst{param}")),
        ControlTarget::Fx { slot, param } => {
            let route = if (*slot as usize) < fx_len && *slot <= u8::MAX as u32 {
                ControlRoute::Fx { slot: *slot as u8, param: *param }
            } else {
                problem = Some(format!("fx slot {slot} out of range"));
                ControlRoute::Unresolved
            };
            (route, format!("fx{slot}.{param}"))
        }
        ControlTarget::Glue { param } => {
            let route = match GlueControl::from_name(param) {
                Some(c) => ControlRoute::Glue(c),
                None => {
                    problem = Some(format!("unknown glue parameter '{param}'"));
                    ControlRoute::Unresolved
                }
            };
            (route, param.clone())
        }
        ControlTarget::Send { bus } => (
            ControlRoute::Send(*bus),
            match bus {
                SendBus::Delay => "delay".to_string(),
                SendBus::Reverb => "reverb".to_string(),
            },
        ),
    };
    let label = spec.label.clone().unwrap_or(name);
    let mut also = Vec::new();
    for extra in &spec.also {
        let (r, p) = resolve_control(&ControlSpec { also: Vec::new(), ..extra.clone() }, generator, fx_len);
        if p.is_some() && problem.is_none() {
            problem = p.map(|p| format!("also: {p}"));
        }
        also.push((extra.clone(), r.route));
    }
    (ResolvedControl { label, spec: spec.clone(), route, also }, problem)
}

/// Load a style into RAM. Non-RT; decodes every sample and instantiates
/// every plugin the style references.
pub fn load_style(style: &Style, opts: LoadOptions) -> Result<LoadedStyle, LoadError> {
    let spec = &style.spec;
    if spec.lanes.len() != NUM_LANES {
        return Err(LoadError::LaneCount(spec.lanes.len()));
    }
    let mut ctx = Ctx {
        style,
        rate: opts.sample_rate,
        max_block: opts.max_block.max(1),
        cache: Vec::new(),
        normalize: true,
        sample_bytes: 0,
        clap: None,
        clap_mode: opts.clap,
        pd: None,
        collection: opts.collection,
        warnings: Vec::new(),
    };
    let calibration = opts.calibration;
    let mut generators = opts.generators;
    let mut lanes = Vec::with_capacity(NUM_LANES);
    let mut meta = Vec::with_capacity(NUM_LANES);
    let mut all_claps: Vec<Arc<ClapCell>> = Vec::new();
    let mut clap_lanes = 0usize;

    for (li, lane) in spec.lanes.iter().enumerate() {
        let mut options: Vec<LaneOption> = Vec::new();
        let mut drum_kits: Vec<DrumKit> = Vec::new();
        let overridden = generators[li].is_some();
        let mut option_names: Vec<String> = Vec::new();
        let mut option_family: Vec<OptionFamily> = Vec::new();
        let mut claps: Vec<Arc<ClapCell>> = Vec::new();
        let mut clap_own_envelope: Vec<bool> = Vec::new();
        let default_option;

        if lane.role == Role::Drums {
            let kits = lane
                .sub_voices
                .iter()
                .filter(|sv| !sv.voice.is_pool())
                .map(|sv| sv.options.len())
                .max()
                .unwrap_or(0)
                .max(1);
            // Kit `kit` steps the backbone and break; pools keep their default.
            let pick_for = |sv: &style::SubVoiceSpec, kit: usize| {
                let step = if sv.voice.is_pool() { 0 } else { kit };
                (sv.default_option + step) % sv.options.len().max(1)
            };
            for kit in 0..kits {
                let mut bank = SampleBank::new();
                let mut names: Vec<&str> = Vec::new();
                let mut next = [None::<u8>; 7];
                for sv in &lane.sub_voices {
                    if sv.options.is_empty() {
                        continue;
                    }
                    let opt: &InstrumentOption = &sv.options[pick_for(sv, kit)];
                    let (base, capacity) = drum_target_range(sv.voice);
                    // A second sub-voice of the same kind continues after the first.
                    let first = next[sv.voice as usize].unwrap_or(base);
                    let left = (base + capacity).saturating_sub(first);
                    match opt.source() {
                        Some(InstrumentSource::Sampler(s)) => {
                            let gain = if calibration { 1.0 } else { db_to_lin(opt.lufs_trim_db) };
                            let end = ctx.fill_bank(&mut bank, first, left, &s.samples, sv.choke, gain, false)?;
                            next[sv.voice as usize] = Some(end);
                            names.push(&opt.name);
                        }
                        _ => ctx.warn(format!("drum sub-voice option '{}' is not a sampler; skipped", opt.name)),
                    }
                }
                options.push(LaneOption::Sampler { bank: Shared::new(&gc_handle(), bank), gain: 1.0, shape: None });
                option_names.push(names.join("+"));
                // The generator follows the kit: sample and slice counts differ per kit.
                let layout = DrumLayout::from_sub_voices(&lane.sub_voices, |sv| pick_for(sv, kit));
                let generator = (kit > 0 && !overridden)
                    .then(|| generator::build_with(&lane.generator, lane.role, &layout, &lane.archetypes));
                drum_kits.push(DrumKit { layout, generator });
            }
            default_option = 0;
        } else {
            // The style's options, then every pack in `pack_dir`.
            let mut lane_options: Vec<InstrumentOption> = lane.options.clone();
            if let Some(dir) = &lane.pack_dir {
                let found = pack::scan(&style.asset_path(dir));
                if found.is_empty() {
                    ctx.warn(format!("lane '{}': no sound packs in {}", lane.name, dir.display()));
                }
                for p in found {
                    lane_options.push(InstrumentOption {
                        name: String::new(),
                        lufs_trim_db: 0.0,
                        sampler: None,
                        clap: None,
                        pack: Some(p),
                    });
                }
            }
            let is_phrase = generator::registry::GeneratorKind::from_name(&lane.generator.name)
                == Some(generator::registry::GeneratorKind::Phrase);
            if lane_options.iter().any(|o| o.pack.is_some()) && !is_phrase {
                ctx.warn(format!("lane '{}': sound packs play with `generator: phrase`", lane.name));
            }
            let mut pack_gens: Vec<Option<Box<dyn LaneGenerator>>> = Vec::new();
            let wants_clap = lane_options.iter().any(|o| o.clap.is_some());
            let clap_allowed = wants_clap && clap_lanes < MAX_CLAP_LANES;
            if wants_clap {
                if clap_allowed {
                    clap_lanes += 1;
                } else {
                    ctx.warn(format!("lane '{}': only {MAX_CLAP_LANES} lanes may host CLAP instruments", lane.name));
                }
            }
            for opt in &lane_options {
                let gain = if calibration { 1.0 } else { db_to_lin(opt.lufs_trim_db) };
                let mut name = opt.name.clone();
                let mut pack_gen: Option<Box<dyn LaneGenerator>> = None;
                let loaded = match opt.source() {
                    Some(InstrumentSource::Pack(dir)) => match ctx.pack_option(lane, &style.asset_path(dir)) {
                        Ok((pack, bank, shape, gen)) => {
                            if name.is_empty() {
                                name = pack.name;
                            }
                            pack_gen = Some(Box::new(gen));
                            LaneOption::Sampler { bank, gain, shape: Some(shape) }
                        }
                        Err(e) => {
                            ctx.warn(format!("lane '{}': pack {} not loaded: {e}", lane.name, dir.display()));
                            if name.is_empty() {
                                name = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                            }
                            LaneOption::Unavailable
                        }
                    },
                    Some(InstrumentSource::Sampler(s)) => {
                        let mut bank = SampleBank::new();
                        // MUSICAL: a bass choke group is mono, so overlapping
                        // notes slide instead of stacking.
                        let mono = lane.role == Role::Bass;
                        ctx.fill_bank(&mut bank, 0, u8::MAX, &s.samples, 0, 1.0, mono)?;
                        LaneOption::Sampler { bank: Shared::new(&gc_handle(), bank), gain, shape: None }
                    }
                    Some(InstrumentSource::Clap(c)) if clap_allowed => {
                        let rate = ctx.rate;
                        let created = match ctx.clap_manager() {
                            Some(m) => m.create_instrument(&c.plugin_id, rate).map_err(|e| e.to_string()),
                            None => Err("CLAP is disabled".to_string()),
                        };
                        match created {
                            Ok(mut inst) => {
                                if let Some(preset) = &c.preset {
                                    if let Err(e) = inst.load_state_from_file(&style.asset_path(preset)) {
                                        ctx.warn(format!("option '{}': preset not loaded: {e}", opt.name));
                                    }
                                }
                                claps.push(ClapCell::new(inst));
                                clap_own_envelope.push(c.own_envelope);
                                LaneOption::Clap { slot: claps.len() - 1, gain }
                            }
                            Err(e) => {
                                ctx.warn(format!("option '{}': CLAP instrument '{}' not loaded: {e}", opt.name, c.plugin_id));
                                LaneOption::Unavailable
                            }
                        }
                    }
                    _ => LaneOption::Unavailable,
                };
                options.push(loaded);
                option_names.push(name);
                pack_gens.push(pack_gen);
                option_family.push(match opt.source() {
                    Some(InstrumentSource::Pack(_)) => OptionFamily::Pack,
                    Some(InstrumentSource::Clap(_)) => OptionFamily::Synth,
                    Some(InstrumentSource::Sampler(s)) if s.samples.iter().any(|x| x.stream) => OptionFamily::Stream,
                    _ => OptionFamily::Sampler,
                });
            }
            // Start on the style's default, or the first option that loaded.
            let usable = |o: &LaneOption| !matches!(o, LaneOption::Unavailable);
            default_option = if options.get(lane.default_option).is_some_and(usable) {
                lane.default_option
            } else {
                options.iter().position(usable).unwrap_or(0)
            };
            // Each pack plays its own phrases (per-option generators, like drum kits).
            if !overridden && pack_gens.iter().any(Option::is_some) {
                drum_kits = pack_gens
                    .into_iter()
                    .map(|generator| DrumKit { layout: DrumLayout::standard(), generator })
                    .collect();
            }
        }

        let fx_specs: &[FxSpec] = if calibration { &[] } else { &lane.fx };
        let fx = ctx.build_fx(&lane.name, fx_specs);
        let break_specs: &[FxSpec] = if calibration { &[] } else { &lane.break_fx };
        let break_fx = ctx.build_fx(&format!("{} break", lane.name), break_specs);
        // Drum voice chains, then the break's: each gets a bus.
        let mut voice_fx: Vec<(DrumVoice, LaneFx)> = Vec::new();
        if !calibration {
            for (voice, specs) in lane.voice_fx.chains() {
                if !specs.is_empty() {
                    voice_fx.push((voice, ctx.build_fx(&format!("{} {}", lane.name, voice.name()), specs)));
                }
            }
        }
        if !break_fx.is_empty() {
            voice_fx.push((DrumVoice::Break, break_fx));
        }
        // Resolve against the style's chain (calibration loads it empty; unknown slots are ignored).
        let fx_len = lane.fx.len();
        let generator = generators[li].take().unwrap_or_else(|| {
            let motifs = ctx.motifs(lane);
            generator::build_lane_motifs(lane, motifs)
        });

        let mut gen_params = generator.param_defaults();
        for (name, &value) in &lane.generator.params {
            if let Some(i) = generator.param_names().iter().position(|n| n == name) {
                if i < MAX_GEN_PARAMS {
                    gen_params[i] = value;
                }
            }
        }
        let mut problems = Vec::new();
        let controls: [ResolvedControl; CONTROLS_PER_LANE] = std::array::from_fn(|ci| {
            let fallback = ControlSpec {
                label: None,
                target: ControlTarget::Generator { param: String::new() },
                min: 0.0,
                max: 1.0,
                curve: style::Curve::Linear,
                default: 0.5,
                also: Vec::new(),
            };
            let spec = lane.controls.get(ci).unwrap_or(&fallback);
            let (control, problem) = resolve_control(spec, generator.as_ref(), fx_len);
            if let Some(p) = problem {
                problems.push(format!("lane '{}' control {ci}: {p}", lane.name));
            }
            control
        });
        let shift_controls: Vec<ResolvedControl> = lane
            .shift_controls
            .iter()
            .take(CONTROLS_PER_LANE)
            .enumerate()
            .map(|(ci, spec)| match spec {
                Some(spec) => {
                    let (control, problem) = resolve_control(spec, generator.as_ref(), fx_len);
                    if let Some(p) = problem {
                        problems.push(format!("lane '{}' shift control {ci}: {p}", lane.name));
                    }
                    control
                }
                // An empty slot: the control does nothing.
                None => ResolvedControl {
                    label: String::new(),
                    spec: ControlSpec {
                        label: None,
                        target: ControlTarget::Generator { param: String::new() },
                        min: 0.0,
                        max: 1.0,
                        curve: style::Curve::Linear,
                        default: 0.5,
                        also: Vec::new(),
                    },
                    route: ControlRoute::Unresolved,
                    also: Vec::new(),
                },
            })
            .collect();
        for p in problems {
            ctx.warn(p);
        }

        let option_available = options.iter().map(|o| !matches!(o, LaneOption::Unavailable)).collect();
        all_claps.extend(claps.iter().cloned());
        lanes.push(Lane::new(LaneInit {
            role: lane.role,
            sample_rate: opts.sample_rate,
            max_block: ctx.max_block,
            max_voices: opts.voices[li],
            interpolation: opts.interpolation,
            options,
            claps,
            clap_own_envelope,
            fx,
            voice_fx,
            output_gains: if calibration { Vec::new() } else { lane.output_trims_db.iter().map(|&db| super::glue::db_to_lin(db)).collect() },
            default_option,
            kit_layouts: if lane.role == Role::Drums { drum_kits.iter().map(|k| k.layout).collect() } else { Vec::new() },
        }));
        meta.push(LaneMeta {
            name: lane.name.clone(),
            role: lane.role,
            option_names,
            option_available,
            option_family,
            default_option,
            controls,
            shift_controls,
            base_rate: if lane.role == Role::Drums { 0 } else { lane.base_rate.exponent() },
            sends: lane.sends,
            generator,
            gen_params,
            drum_kits,
        });
    }

    let no_fx: &[FxSpec] = &[];
    let drum_bus_fx = ctx.build_fx("drum bus", if calibration { no_fx } else { &spec.glue.drum_bus_fx });
    let master_fx = ctx.build_fx("master", if calibration { no_fx } else { &spec.glue.master_fx });
    let (glue, glue_warnings) = glue_params(&spec.glue, &spec.send_fx, spec.tempo.default);
    for w in glue_warnings {
        ctx.warn(w);
    }
    Ok(LoadedStyle {
        name: spec.name.clone(),
        sample_rate: opts.sample_rate,
        max_block: ctx.max_block,
        tempo: spec.tempo,
        // MUSICAL: A minor (8A) when the style sets no key.
        key: style.default_key().unwrap_or(MusicalKey::new(9, true)),
        progressions: style.progressions(),
        default_progression: spec.default_progression,
        forms: style.forms(),
        default_form: spec.default_form,
        glue,
        drum_bus_fx,
        master_fx,
        lanes,
        meta,
        claps: all_claps,
        warnings: ctx.warnings,
        sample_bytes: ctx.sample_bytes,
    })
}

/// Fixture style used by the live-mode tests.
#[cfg(test)]
pub(crate) fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live-style")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::lane::{drum_target_range, DRUM_TARGET_BREAK, DRUM_TARGET_HATS, DRUM_TARGET_PERC};

    fn load() -> LoadedStyle {
        let style = Style::load(&fixture_dir()).unwrap_or_else(|e| panic!("{e}"));
        load_style(&style, LoadOptions::new(48_000)).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Rewrites the fixture's WAV files (short synthetic hits and tones).
    /// Run once after changing them:
    /// `cargo test -p mesh-core --lib live::loader::tests::regenerate_fixture_wavs -- --ignored`
    #[test]
    #[ignore = "writes the fixture assets"]
    fn regenerate_fixture_wavs() {
        use std::f32::consts::TAU;
        let noise = |i: usize, salt: u64| crate::live::rng::unit_f32(crate::live::rng::mix64(i as u64 ^ salt << 32)) * 2.0 - 1.0;
        let write = |rel: &str, rate: u32, samples: Vec<f32>| {
            let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
            let mut w = hound::WavWriter::create(fixture_dir().join(rel), spec).unwrap();
            for s in samples {
                w.write_sample((s.clamp(-1.0, 1.0) * 32000.0) as i16).unwrap();
            }
            w.finalize().unwrap();
        };
        // Exponential decay reaching -60 dB at the end of `n` frames.
        let decay = |i: usize, n: usize| (-6.9 * i as f32 / n as f32).exp();
        let kick = |n: usize| -> Vec<f32> {
            let mut phase = 0.0f32;
            (0..n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    phase += (45.0 + 110.0 * (-8.0 * t).exp()) / 48_000.0;
                    (phase * TAU).sin() * decay(i, n) * 0.9
                })
                .collect()
        };
        let snare = |n: usize, level: f32| -> Vec<f32> {
            (0..n)
                .map(|i| ((i as f32 * 185.0 / 48_000.0 * TAU).sin() * 0.4 + noise(i, 1) * 0.6) * decay(i, n) * level)
                .collect()
        };
        // First difference of white noise: a cheap high-pass.
        let hat = |n: usize, salt: u64| -> Vec<f32> {
            (0..n).map(|i| (noise(i + 1, salt) - noise(i, salt)) * 0.5 * decay(i, n) * 0.7).collect()
        };
        let saw = |i: usize, hz: f32, rate: f32| ((i as f32 * hz / rate).fract() * 2.0 - 1.0) * 0.5;

        write("assets/drums/kick.wav", 48_000, kick(4800));
        write("assets/drums/snare.wav", 48_000, snare(3840, 0.8));
        write("assets/drums/hat-closed.wav", 48_000, hat(1440, 2));
        write("assets/drums/hat-open.wav", 48_000, hat(5760, 3));
        write("assets/drums/shaker.wav", 48_000, hat(2400, 4));
        // Break: kick, hat, snare, ghost snare, 1800 frames each (slices in style.yaml).
        let mut amen = kick(1800);
        amen.extend(hat(1800, 5));
        amen.extend(snare(1800, 0.8));
        amen.extend(snare(1800, 0.3));
        write("assets/breaks/amen.wav", 48_000, amen);
        // Pitched material at 24 kHz, so loading also covers resampling.
        let tone = |n: usize, hz: f32, shape: &dyn Fn(usize, usize) -> f32| -> Vec<f32> {
            (0..n).map(|i| (saw(i, hz, 24_000.0) + saw(i, hz * 1.007, 24_000.0)) * shape(i, n)).collect()
        };
        let fade = |i: usize, n: usize| (i.min(n - i) as f32 / 240.0).min(1.0);
        write("assets/bass/reese-a1.wav", 24_000, tone(9600, 55.0, &fade));
        write("assets/lead/stab-a3.wav", 24_000, tone(4800, 220.0, &decay));
        write("assets/lead/stab-a4.wav", 24_000, tone(4800, 440.0, &decay));
        write("assets/pad/pad-a3.wav", 24_000, tone(12_000, 220.0, &|i, n| fade(i, n) * 0.6));
    }

    #[test]
    fn fixture_loads_with_banks_controls_and_glue() {
        let loaded = load();
        assert_eq!(loaded.name, "fixture-breaks");
        assert_eq!(loaded.lanes.len(), NUM_LANES);
        assert_eq!(loaded.key, MusicalKey::new(9, true));
        assert_eq!(loaded.progressions.len(), 2);
        assert!(loaded.sample_bytes > 0);

        // Drums: one kit (every sub-voice has one option).
        assert_eq!(loaded.meta[0].option_names, vec!["clean-kit+amen+closed-open+shaker"]);
        // Bass: the CLAP option is unavailable with CLAP disabled; the sampler one plays.
        assert_eq!(loaded.meta[1].option_available, vec![true, false]);
        assert_eq!(loaded.meta[1].default_option, 0);
        assert!(loaded.claps.is_empty());

        // Controls resolve to routes.
        let drums = &loaded.meta[0].controls;
        assert_eq!(drums[2].route, ControlRoute::Fx { slot: 0, param: 0 });
        assert_eq!(drums[3].route, ControlRoute::Send(SendBus::Reverb));
        assert_eq!(loaded.meta[1].controls[3].route, ControlRoute::Glue(GlueControl::MasterComp));
        assert_eq!(loaded.meta[2].controls[2].route, ControlRoute::Instrument(0));
        assert_eq!(drums[0].label, "style");

        // Glue mapped from the style.
        assert_eq!(loaded.glue.high_pass.lead.freq_hz, 500.0);
        assert_eq!((loaded.glue.duck.bass.kick_db, loaded.glue.duck.bass.release_ms), (6.0, 60.0));
        assert_eq!(loaded.glue.duck.bass.snare_db, crate::live::glue::DuckParams::default().bass.snare_db, "unset keeps the default");
        assert_eq!(loaded.glue.duck.others.snare_db, 3.0);
        assert_eq!(loaded.glue.drum_bus.compressor.threshold_db, -12.0);
        assert!(!loaded.glue.drive.enabled);
        assert_eq!(loaded.glue.master.compressor.ratio, 2.0);
        assert!((loaded.glue.delay.feedback - 0.38).abs() < 1e-6);
    }

    #[test]
    fn motif_clips_load_from_the_folder_and_the_lane_list() {
        use crate::live::motif::{midi, starter, Voice};
        let dir = tempfile::tempdir().unwrap();
        for e in walkdir::WalkDir::new(fixture_dir()).into_iter().filter_map(|e| e.ok()) {
            let rel = e.path().strip_prefix(fixture_dir()).unwrap();
            if e.file_type().is_dir() {
                std::fs::create_dir_all(dir.path().join(rel)).unwrap();
            } else {
                std::fs::copy(e.path(), dir.path().join(rel)).unwrap();
            }
        }
        let write = |rel: &str, bytes: &[u8]| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        let clip = |c: &starter::StarterClip, v| midi::write(&c.clip(v), c.name);
        write("motifs/bass/a.mid", &clip(&starter::BASS[0], Voice::Lowest));
        write("motifs/bass/b.mid", &clip(&starter::BASS[1], Voice::Lowest));
        write("motifs/lead/broken.mid", b"not midi");
        write("listed/lead.mid", &clip(&starter::LEAD[0], Voice::Highest));
        let yaml = std::fs::read_to_string(dir.path().join("style.yaml")).unwrap();
        let yaml = yaml.replace("  - name: lead\n    role: lead\n", "  - name: lead\n    role: lead\n    motifs: [listed/lead.mid]\n");
        assert!(yaml.contains("listed/lead.mid"));
        std::fs::write(dir.path().join("style.yaml"), yaml).unwrap();

        let style = Style::load(dir.path()).unwrap_or_else(|e| panic!("{e}"));
        let loaded = load_style(&style, LoadOptions::new(48_000)).unwrap_or_else(|e| panic!("{e}"));
        // Bass: the folder's two clips (one of them is a 2-bar clip).
        let phrase = loaded.meta[1].generator.motif_phrase(1, 1);
        assert!(phrase.is_some(), "the bass lane has clips");
        // Lead: the listed clip loads, the broken file is a warning.
        assert!(loaded.warnings.iter().any(|w| w.contains("broken.mid")), "{:?}", loaded.warnings);
        assert!(loaded.meta[2].generator.param_names().len() > 0);
        // A missing listed file fails validation.
        let missing = std::fs::read_to_string(dir.path().join("style.yaml")).unwrap().replace("listed/lead.mid", "listed/gone.mid");
        std::fs::write(dir.path().join("style.yaml"), missing).unwrap();
        assert!(Style::load(dir.path()).is_err());
    }

    #[test]
    fn sound_packs_load_as_options_with_their_own_phrases() {
        use crate::live::event::EventKind;
        use crate::live::motif::{midi, Clip, RawNote};
        let dir = tempfile::tempdir().unwrap();
        for e in walkdir::WalkDir::new(fixture_dir()).into_iter().filter_map(|e| e.ok()) {
            let rel = e.path().strip_prefix(fixture_dir()).unwrap();
            if e.file_type().is_dir() {
                std::fs::create_dir_all(dir.path().join(rel)).unwrap();
            } else {
                std::fs::copy(e.path(), dir.path().join(rel)).unwrap();
            }
        }
        let pack = dir.path().join("packs/tiny");
        std::fs::create_dir_all(pack.join("phrases")).unwrap();
        std::fs::write(pack.join("sound.sf2"), crate::live::sf2::tests::tiny_sf2()).unwrap();
        let step = crate::live::TICKS_PER_STEP as u32;
        let clip = Clip { bars: 1, notes: vec![RawNote { start: 0, len: 8 * step, pitch: 48, vel: 100 }, RawNote { start: 8 * step, len: 8 * step, pitch: 51, vel: 100 }] };
        std::fs::write(pack.join("phrases/riff.mid"), midi::write(&clip, "riff")).unwrap();
        // A folder without an instrument is not a pack.
        std::fs::create_dir_all(dir.path().join("packs/notes")).unwrap();
        let yaml = std::fs::read_to_string(dir.path().join("style.yaml")).unwrap();
        let yaml = yaml.replace(
            "  - name: lead\n    role: lead\n",
            "  - name: lead\n    role: lead\n    pack_dir: packs\n",
        );
        let yaml = yaml.replace("      family: procedural\n      name: lead\n", "      family: procedural\n      name: phrase\n");
        assert!(yaml.contains("pack_dir: packs") && yaml.contains("name: phrase"));
        std::fs::write(dir.path().join("style.yaml"), yaml).unwrap();

        let style = Style::load(dir.path()).unwrap_or_else(|e| panic!("{e}"));
        let loaded = load_style(&style, LoadOptions::new(48_000)).unwrap_or_else(|e| panic!("{e}"));
        let lead = &loaded.meta[2];
        assert_eq!(lead.option_names.len(), 3, "two style options and one pack: {:?}", lead.option_names);
        assert_eq!(lead.option_names[2], "tiny");
        assert!(lead.option_available[2]);
        assert_eq!(lead.option_family[2], OptionFamily::Pack);
        assert_eq!(lead.drum_kits.len(), 3);
        assert!(lead.drum_kits[0].generator.is_none() && lead.drum_kits[2].generator.is_some());
        // The pack's generator plays its phrase.
        let params = [0.5; MAX_GEN_PARAMS];
        let mask = [0u8; crate::live::STEPS_PER_BAR];
        let ctx = crate::live::generator::test_util::ctx(1, 0, &params, &mask);
        let mut out = crate::live::event::BarEvents::default();
        lead.drum_kits[2].generator.as_deref().unwrap().render_bar(&ctx, &mut out);
        assert_eq!(out.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn).count(), 2);
        // The style's own generator has no phrases.
        let mut silent = crate::live::event::BarEvents::default();
        lead.generator.render_bar(&ctx, &mut silent);
        assert!(silent.as_slice().is_empty());
    }

    #[test]
    fn samples_are_normalized_to_one_loudness() {
        let tone = |amp: f32, secs: f32| {
            let n = (48_000.0 * secs) as usize;
            let x: Vec<f32> = (0..n).map(|i| amp * (i as f32 * 0.0577).sin()).collect();
            SampleData::from_channels(&x, &x, 48_000, 60)
        };
        // A loud short hit and a quiet long loop end at the same LUFS.
        for (amp, secs) in [(0.9, 0.15), (0.15, 2.0), (0.3, 0.6)] {
            let data = tone(amp, secs);
            let g = sample_gain(&data);
            let frames: Vec<crate::types::StereoSample> = (0..data.len()).map(|i| data.frame(i) * g).collect();
            // Short hits are measured over their own length.
            let got = if frames.len() < 19_200 { measure_hit_lufs(&frames, 48_000.0) } else { measure_lufs(&frames, 48_000.0) };
            assert!((got - SAMPLE_LOUDNESS_TARGET).abs() < 0.5, "amp {amp}: {got}");
        }
        // Silence keeps unity gain; a very quiet file comes up at most 12 dB.
        assert_eq!(sample_gain(&tone(0.0, 0.5)), 1.0);
        let quiet = sample_gain(&tone(0.001, 0.05));
        assert!((20.0 * quiet.log10() - SAMPLE_BOOST_MAX_DB as f32).abs() < 0.01, "{quiet}");
        // A 50 ms hit reads as loud as it sounds, not 9 dB quieter.
        let hit = 20.0 * sample_gain(&tone(0.3, 0.05)).log10();
        let held = 20.0 * sample_gain(&tone(0.3, 0.6)).log10();
        assert!((hit - held).abs() < 1.0, "hit {hit} dB vs held {held} dB");
    }

    #[test]
    fn drum_bank_follows_the_target_layout() {
        let style = Style::load(&fixture_dir()).unwrap();
        let mut ctx = Ctx {
            style: &style,
            rate: 48_000,
            max_block: 64,
            cache: Vec::new(),
            normalize: false,
            sample_bytes: 0,
            clap: None,
            clap_mode: ClapMode::Disabled,
            pd: None,
            collection: None,
            warnings: Vec::new(),
        };
        let mut bank = SampleBank::new();
        for sv in &style.spec.lanes[0].sub_voices {
            let (base, cap) = drum_target_range(sv.voice);
            let Some(InstrumentSource::Sampler(s)) = sv.options[0].source() else { panic!() };
            ctx.fill_bank(&mut bank, base, cap, &s.samples, sv.choke, 0.5, false).unwrap();
        }
        // Backbone: kick, snare. Break: 4 slices. Hats: closed, open. Perc: shaker.
        for t in [0, 1, DRUM_TARGET_BREAK, DRUM_TARGET_BREAK + 3, DRUM_TARGET_HATS, DRUM_TARGET_HATS + 1, DRUM_TARGET_PERC] {
            assert!(bank.resolve(t, 0).is_some(), "target {t}");
        }
        assert!(bank.resolve(2, 0).is_none());
        assert!(bank.resolve(DRUM_TARGET_BREAK + 4, 0).is_none());
        let slice = bank.resolve(DRUM_TARGET_BREAK + 1, 0).unwrap();
        assert_eq!((slice.choke, slice.gain, slice.pitched), (1, 0.5, false));
        assert_eq!(slice.kind, SliceType::Hat);
        assert_eq!(bank.resolve(DRUM_TARGET_HATS, 0).unwrap().choke, 2);
        // Same numbering as the generators' layout for this kit.
        let layout = DrumLayout::from_lane(&style.spec.lanes[0]);
        assert_eq!((layout.kick, layout.snare, layout.hat_closed(), layout.hat_open()), (Some(0), Some(1), Some(8), Some(9)));
        assert_eq!((layout.perc_count, layout.slices().len()), (1, 4));
        for i in 0..layout.slices().len() {
            assert!(bank.resolve(layout.slice_target(i), 0).is_some());
        }
        // The amen is decoded once although the cache is asked per option.
        assert_eq!(ctx.cache.iter().filter(|c| c.0.ends_with("amen.wav")).count(), 1);
    }

    #[test]
    fn stored_loudness_replaces_the_load_time_measurement() {
        let style = Style::load(&fixture_dir()).unwrap();
        let Some(InstrumentSource::Sampler(s)) = style.spec.lanes[2].options[0].source() else { panic!() };
        let spec = s.samples[0].clone();
        let ctx = || Ctx {
            style: &style,
            rate: 48_000,
            max_block: 64,
            cache: Vec::new(),
            normalize: true,
            sample_bytes: 0,
            clap: None,
            clap_mode: ClapMode::Disabled,
            pd: None,
            collection: None,
            warnings: Vec::new(),
        };
        // No stored value: measured at load.
        let (data, measured_gain) = ctx().sample(&SampleSpec { lufs: None, ..spec.clone() }).unwrap();
        assert_eq!(measured_gain, sample_gain(&data));
        // The stored value decides the gain without measuring.
        let stored = -20.0f32;
        let (_, gain) = ctx().sample(&SampleSpec { lufs: Some(stored), ..spec.clone() }).unwrap();
        assert_eq!(gain, gain_for_lufs(stored as f64));
        assert!((20.0 * gain.log10() - (SAMPLE_LOUDNESS_TARGET as f32 - stored)).abs() < 0.01);
        // The file analysis gives the load-time measurement.
        let analysed = analyse_sample_file(&style.asset_path(&spec.file), 48_000).unwrap();
        assert_eq!(gain_for_lufs(analysed), measured_gain);
    }
    #[test]
    fn multisample_zones_share_one_target() {
        let style = Style::load(&fixture_dir()).unwrap();
        let mut ctx = Ctx {
            style: &style,
            rate: 48_000,
            max_block: 64,
            cache: Vec::new(),
            normalize: false,
            sample_bytes: 0,
            clap: None,
            clap_mode: ClapMode::Disabled,
            pd: None,
            collection: None,
            warnings: Vec::new(),
        };
        let Some(InstrumentSource::Sampler(s)) = style.spec.lanes[2].options[0].source() else { panic!() };
        let mut bank = SampleBank::new();
        let next = ctx.fill_bank(&mut bank, 0, u8::MAX, &s.samples, 0, 1.0, false).unwrap();
        assert_eq!(next, 1, "two zones, one target");
        assert_eq!(bank.resolve(0, 57).unwrap().root_key, 57);
        assert_eq!(bank.resolve(0, 72).unwrap().root_key, 69);
        assert!(bank.resolve(0, 60).unwrap().pitched);
    }

    #[test]
    fn missing_generator_param_and_fx_are_warnings_not_errors() {
        let style = Style::load(&fixture_dir()).unwrap();
        let mut opts = LoadOptions::new(48_000);
        // A generator without parameters: its lane's generator controls cannot resolve.
        opts.generators[1] = Some(Box::new(crate::live::generator::fixed::FixedPattern::default()));
        let loaded = load_style(&style, opts).unwrap();
        assert_eq!(loaded.meta[1].controls[0].route, ControlRoute::Unresolved);
        assert!(loaded.warnings.iter().any(|w| w.contains("no parameter 'complexity'")), "{:?}", loaded.warnings);
        assert!(loaded.warnings.iter().any(|w| w.contains("surge")), "{:?}", loaded.warnings);
        // The real generators resolve the style's names.
        let loaded = load();
        assert_eq!(loaded.meta[0].controls[0].route, ControlRoute::Generator(0), "drums: style");
        assert_eq!(loaded.meta[0].controls[1].route, ControlRoute::Generator(1), "drums: density");
        assert!(matches!(loaded.meta[1].controls[0].route, ControlRoute::Generator(_)), "bass: complexity");
        assert_eq!(loaded.meta[0].drum_kits.len(), 1);
    }

    #[test]
    fn a_control_drives_its_also_targets() {
        use crate::live::style::{ControlSpec, ControlTarget, Curve};
        let style = Style::load(&fixture_dir()).unwrap();
        let generator = crate::live::generator::build_lane(&style.spec.lanes[0]);
        let spec = ControlSpec {
            label: None,
            target: ControlTarget::Generator { param: "density".into() },
            min: 0.0,
            max: 1.0,
            curve: Curve::Linear,
            default: 0.5,
            also: vec![
                ControlSpec {
                    label: None,
                    target: ControlTarget::Fx { slot: 0, param: 6 },
                    min: 0.22,
                    max: 0.08,
                    curve: Curve::Linear,
                    default: 0.5,
                    also: Vec::new(),
                },
                ControlSpec {
                    label: None,
                    target: ControlTarget::Fx { slot: 9, param: 0 },
                    min: 0.0,
                    max: 1.0,
                    curve: Curve::Linear,
                    default: 0.5,
                    also: Vec::new(),
                },
            ],
        };
        let (c, problem) = resolve_control(&spec, generator.as_ref(), 1);
        assert!(matches!(c.route, ControlRoute::Generator(_)));
        assert_eq!(c.also.len(), 2);
        assert_eq!(c.also[0].1, ControlRoute::Fx { slot: 0, param: 6 });
        assert!((c.also[0].0.map(1.0) - 0.08).abs() < 1e-6, "its own range");
        assert_eq!(c.also[1].1, ControlRoute::Unresolved);
        assert!(problem.is_some_and(|p| p.contains("also")), "a bad extra target is reported");
    }
}

#[cfg(test)]
mod user_style_probe {
    /// `MESH_STYLE=dir cargo test -p mesh-core --lib probe_style -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_style() {
        let dir = std::path::PathBuf::from(std::env::var("MESH_STYLE").expect("MESH_STYLE"));
        let style = super::Style::load(&dir).unwrap_or_else(|e| panic!("{e}"));
        let loaded = super::load_style(&style, super::LoadOptions::new(48_000)).unwrap_or_else(|e| panic!("{e}"));
        for w in &loaded.warnings {
            println!("warning: {w}");
        }
        for (i, m) in loaded.meta.iter().enumerate() {
            println!("lane {i} {} options {:?} families {:?}", m.name, m.option_names, m.option_family);
            for c in &m.controls {
                println!("  {} -> {:?} also {:?}", c.label, c.route, c.also.iter().map(|a| a.1).collect::<Vec<_>>());
            }
        }
        for (i, o) in loaded.lanes[1].options_debug().iter().enumerate() {
            println!("bass option {i}: {o}");
        }
    }
}
