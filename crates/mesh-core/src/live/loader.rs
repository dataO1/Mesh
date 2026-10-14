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
//! An option's `lufs_trim_db` is applied at the instrument output, before the
//! lane FX: as the sampler's lane gain (instrument lanes), as each slot's
//! gain (drum kits, one trim per sub-voice option) or as the CLAP output
//! gain. `style-calibrate` measures at the same point, so a calibrated option
//! hits its role target going into the FX chain and the glue.
//!
//! # Drum kits
//!
//! The drum lane has one option list per sub-voice. Kit `k` selects, for each
//! sub-voice, option `(default_option + k) % options.len()`, so kit 0 is the
//! style's default and turning the encoder cycles every sub-voice.

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
use super::glue::{db_to_lin, GlueParams, Role};
use super::harmony::Progression;
use super::fx::{airwindows_state, setting_key, setting_param, AIRWINDOWS_ID};
use super::lane::{
    drum_target_range, native_effect, ClapCell, FxSlot, Lane, LaneFx, LaneInit, LaneOption,
};
use super::sampler::{Interpolation, SampleBank, SampleData, Slice, SliceType, SlotDef};
use super::style::{
    self, ControlSpec, ControlTarget, FxSource, FxSpec, GlueSpec, InstrumentOption, InstrumentSource, SampleSpec,
    SendBus, SendFxSpec, SendLevels, Style, TempoSpec, CONTROLS_PER_LANE,
};
use super::NUM_LANES;

/// Lanes that may host CLAP instruments (D32).
pub const MAX_CLAP_LANES: usize = 2;
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
}

/// One drum kit as the generator sees it.
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
    pub default_option: usize,
    pub controls: [ResolvedControl; CONTROLS_PER_LANE],
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

struct Ctx<'a> {
    style: &'a Style,
    rate: u32,
    max_block: usize,
    cache: Vec<(PathBuf, u8, Vec<style::SliceSpec>, Shared<SampleData>)>,
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

    /// Decoded, resampled sample with its slice table. Cached per
    /// `(file, root key, slices)`.
    fn sample(&mut self, spec: &SampleSpec) -> Result<Shared<SampleData>, LoadError> {
        let path = self.style.asset_path(&spec.file);
        let root = spec.root_key.map(|n| n.0).unwrap_or(UNPITCHED_ROOT);
        if let Some(hit) = self.cache.iter().find(|c| c.0 == path && c.1 == root && c.2 == spec.slices) {
            return Ok(hit.3.clone());
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
        self.cache.push((path, root, spec.slices.clone(), shared.clone()));
        Ok(shared)
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
            let data = self.sample(spec)?;
            let choke = if spec.choke != 0 { spec.choke } else { default_choke };
            if choke != 0 && mono_groups {
                bank.set_mono(choke, true);
            }
            let finish = |slot: SlotDef| {
                let slot = slot.with_choke(choke).with_gain(gain);
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
    (ResolvedControl { label, spec: spec.clone(), route }, problem)
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
        let mut claps: Vec<Arc<ClapCell>> = Vec::new();
        let default_option;

        if lane.role == Role::Drums {
            let kits = lane.sub_voices.iter().map(|sv| sv.options.len()).max().unwrap_or(0).max(1);
            for kit in 0..kits {
                let mut bank = SampleBank::new();
                let mut names: Vec<&str> = Vec::new();
                let mut next = [None::<u8>; 4];
                for sv in &lane.sub_voices {
                    if sv.options.is_empty() {
                        continue;
                    }
                    let opt: &InstrumentOption = &sv.options[(sv.default_option + kit) % sv.options.len()];
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
                options.push(LaneOption::Sampler { bank: Shared::new(&gc_handle(), bank), gain: 1.0 });
                option_names.push(names.join("+"));
                // The generator follows the kit: sample and slice counts differ per kit.
                let pick = |sv: &style::SubVoiceSpec| (sv.default_option + kit) % sv.options.len().max(1);
                let layout = DrumLayout::from_sub_voices(&lane.sub_voices, pick);
                let generator = (kit > 0 && !overridden)
                    .then(|| generator::build_with(&lane.generator, lane.role, &layout, &lane.archetypes));
                drum_kits.push(DrumKit { layout, generator });
            }
            default_option = 0;
        } else {
            let wants_clap = lane.options.iter().any(|o| o.clap.is_some());
            let clap_allowed = wants_clap && clap_lanes < MAX_CLAP_LANES;
            if wants_clap {
                if clap_allowed {
                    clap_lanes += 1;
                } else {
                    ctx.warn(format!("lane '{}': only {MAX_CLAP_LANES} lanes may host CLAP instruments", lane.name));
                }
            }
            for opt in &lane.options {
                let gain = if calibration { 1.0 } else { db_to_lin(opt.lufs_trim_db) };
                let loaded = match opt.source() {
                    Some(InstrumentSource::Sampler(s)) => {
                        let mut bank = SampleBank::new();
                        // MUSICAL: a bass choke group is mono, so overlapping
                        // notes slide instead of stacking.
                        let mono = lane.role == Role::Bass;
                        ctx.fill_bank(&mut bank, 0, u8::MAX, &s.samples, 0, 1.0, mono)?;
                        LaneOption::Sampler { bank: Shared::new(&gc_handle(), bank), gain }
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
                option_names.push(opt.name.clone());
            }
            // Start on the style's default, or the first option that loaded.
            let usable = |o: &LaneOption| !matches!(o, LaneOption::Unavailable);
            default_option = if options.get(lane.default_option).is_some_and(usable) {
                lane.default_option
            } else {
                options.iter().position(usable).unwrap_or(0)
            };
        }

        let fx_specs: &[FxSpec] = if calibration { &[] } else { &lane.fx };
        let fx = ctx.build_fx(&lane.name, fx_specs);
        let break_specs: &[FxSpec] = if calibration { &[] } else { &lane.break_fx };
        let break_fx = ctx.build_fx(&format!("{} break", lane.name), break_specs);
        // Resolve against the style's chain (calibration loads it empty; unknown slots are ignored).
        let fx_len = lane.fx.len();
        let generator = generators[li].take().unwrap_or_else(|| generator::build_lane(lane));

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
            };
            let spec = lane.controls.get(ci).unwrap_or(&fallback);
            let (control, problem) = resolve_control(spec, generator.as_ref(), fx_len);
            if let Some(p) = problem {
                problems.push(format!("lane '{}' control {ci}: {p}", lane.name));
            }
            control
        });
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
            fx,
            break_fx,
            output_gains: if calibration { Vec::new() } else { lane.output_trims_db.iter().map(|&db| super::glue::db_to_lin(db)).collect() },
            default_option,
        }));
        meta.push(LaneMeta {
            name: lane.name.clone(),
            role: lane.role,
            option_names,
            option_available,
            default_option,
            controls,
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
    fn drum_bank_follows_the_target_layout() {
        let style = Style::load(&fixture_dir()).unwrap();
        let mut ctx = Ctx {
            style: &style,
            rate: 48_000,
            max_block: 64,
            cache: Vec::new(),
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
        assert_eq!((layout.kick, layout.snare, layout.hat_closed, layout.hat_open), (Some(0), Some(1), Some(8), Some(9)));
        assert_eq!((layout.perc_count, layout.slices().len()), (1, 4));
        for i in 0..layout.slices().len() {
            assert!(bank.resolve(layout.slice_target(i), 0).is_some());
        }
        // The amen is decoded once although the cache is asked per option.
        assert_eq!(ctx.cache.iter().filter(|c| c.0.ends_with("amen.wav")).count(), 1);
    }

    #[test]
    fn multisample_zones_share_one_target() {
        let style = Style::load(&fixture_dir()).unwrap();
        let mut ctx = Ctx {
            style: &style,
            rate: 48_000,
            max_block: 64,
            cache: Vec::new(),
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
}
