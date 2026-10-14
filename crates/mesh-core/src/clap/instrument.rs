//! CLAP instrument hosting (live mode, design §5.2)
//!
//! The effect path ([`super::ClapEffect`]) feeds a fixed stereo input and only
//! `PARAM_VALUE` events. Instruments need a different shape:
//!
//! - **Audio ports** are queried through `audio-ports`. Any input ports the
//!   plugin declares (e.g. Surge XT's audio-in) get silent buffers; the main
//!   output port is copied into the caller's [`StereoBuffer`] (mono outputs are
//!   duplicated to both sides). Extra output ports are rendered and discarded.
//! - **Notes** go through the first input `note-ports` port. The CLAP dialect is
//!   preferred; plugins that only speak MIDI get MIDI 1.0 note on/off instead.
//! - **Transport** (tempo, playing, song position, bar) is sent with every
//!   block from a [`TransportInfo`].
//! - **NOTE_END** output events are collected per block ([`ClapInstrument::ended_notes`])
//!   so the caller can release voice bookkeeping when the synth's release tail is over.
//!
//! # Threading
//!
//! [`ClapInstrument`] owns both the plugin instance and its started audio
//! processor. Methods documented as *main thread* (state, param queries,
//! [`ClapInstrument::idle`]) call into the plugin's main-thread API and must
//! not run concurrently with [`ClapInstrument::process`]; `&mut self` enforces
//! that. Live mode loads and configures the instrument on the loader thread,
//! then moves it to the RT thread.
//!
//! # RT safety
//!
//! [`ClapInstrument::process`] does not allocate or lock: event buffers, note
//! scratch space, pending-param and ended-note lists are all pre-allocated with
//! fixed capacities. Anything beyond capacity is dropped and counted
//! ([`ClapInstrument::dropped_events`]).
//!
//! # Ties and glide
//!
//! CLAP has no glide event. To tie two notes, send the new `On` *before* the
//! previous note's `Off`, at the same or an earlier offset. Notes are stably
//! sorted by offset, so caller order is kept for equal offsets, and a mono /
//! legato patch sees overlapping notes and applies its own portamento.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfoBuffer, HostAudioPorts, HostAudioPortsImpl, PluginAudioPorts,
    RescanType,
};
use clack_extensions::note_ports::{
    HostNotePorts, HostNotePortsImpl, NoteDialect, NoteDialects, NotePortInfoBuffer,
    NotePortRescanFlags, PluginNotePorts,
};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamInfoBuffer,
    ParamInfoFlags, ParamRescanFlags, PluginParams,
};
use clack_extensions::state::{HostState, HostStateImpl, PluginState};
use clack_extensions::voice_info::{HostVoiceInfo, HostVoiceInfoImpl, PluginVoiceInfo, VoiceInfoFlags};
use clack_host::events::event_types::{
    MidiEvent, NoteChokeEvent, NoteEndEvent, NoteExpressionEvent, NoteExpressionType,
    NoteOffEvent, NoteOnEvent, ParamValueEvent, TransportEvent, TransportFlags,
};
use clack_host::events::{EventFlags, Match};
use clack_host::prelude::*;
use clack_host::process::StartedPluginAudioProcessor;
use clack_host::utils::{BeatTime, Cookie, SecondsTime};

use crate::types::{StereoBuffer, StereoSample};

use super::discovery::{ClapDiscovery, DiscoveredClapPlugin};
use super::error::{ClapError, ClapResult};

/// Largest block handed to the plugin in one `process` call. Longer caller
/// blocks are split into sub-blocks of at most this many frames.
pub const INSTRUMENT_MAX_FRAMES: usize = 4096;

/// Note events accepted per `process` call (excess is dropped and counted).
pub const MAX_NOTES_PER_BLOCK: usize = 512;

/// Pending block-rate param changes between two `process` calls.
pub const MAX_PENDING_PARAMS: usize = 256;

/// NOTE_END events kept per `process` call.
pub const MAX_ENDED_NOTES: usize = 256;

/// Input event capacity per sub-block (notes + params).
const MAX_INPUT_EVENTS: usize = MAX_NOTES_PER_BLOCK + MAX_PENDING_PARAMS;

/// Output event capacity per sub-block (NOTE_END, PARAM_VALUE from the GUI, ...).
const MAX_OUTPUT_EVENTS: usize = 1024;

// ============================================================================
// Public event / transport types
// ============================================================================

/// CLAP note expression kinds (absolute, per note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteExpr {
    /// Gain, 0..4 (linear, 1 = unity).
    Volume,
    /// 0 = left, 0.5 = centre, 1 = right.
    Pan,
    /// Semitones, -120..120.
    Tuning,
    /// 0..1
    Vibrato,
    /// 0..1
    Expression,
    /// 0..1
    Brightness,
    /// 0..1
    Pressure,
}

impl NoteExpr {
    fn to_clap(self) -> NoteExpressionType {
        match self {
            NoteExpr::Volume => NoteExpressionType::Volume,
            NoteExpr::Pan => NoteExpressionType::Pan,
            NoteExpr::Tuning => NoteExpressionType::Tuning,
            NoteExpr::Vibrato => NoteExpressionType::Vibrato,
            NoteExpr::Expression => NoteExpressionType::Expression,
            NoteExpr::Brightness => NoteExpressionType::Brightness,
            NoteExpr::Pressure => NoteExpressionType::Pressure,
        }
    }
}

/// What an [`InstrumentNote`] does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InstrumentNoteKind {
    /// Start a note (CLAP NOTE_ON / MIDI 0x9n).
    On,
    /// Release a note (CLAP NOTE_OFF / MIDI 0x8n).
    Off,
    /// Cut a note without release (CLAP NOTE_CHOKE). MIDI dialect: sent as a
    /// note-off with velocity 0, since MIDI 1.0 has no choke.
    Choke,
    /// Set a note expression on a held note (CLAP dialect only, dropped for MIDI).
    Expression(NoteExpr, f64),
}

/// One note event for a block, with a sample offset inside that block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstrumentNote {
    /// Sample offset within the block passed to [`ClapInstrument::process`].
    /// Offsets past the block end are clamped to the last frame.
    pub offset: u32,
    pub kind: InstrumentNoteKind,
    /// MIDI key 0..=127.
    pub key: u8,
    /// Velocity 0.0..=1.0 (ignored for `Choke` / `Expression`).
    pub velocity: f32,
    /// CLAP note id. `None` = wildcard (matches any note on this key/channel).
    pub note_id: Option<u32>,
    /// Channel 0..=15.
    pub channel: u8,
}

impl InstrumentNote {
    pub fn on(offset: u32, key: u8, velocity: f32, note_id: u32) -> Self {
        Self { offset, kind: InstrumentNoteKind::On, key, velocity, note_id: Some(note_id), channel: 0 }
    }

    pub fn off(offset: u32, key: u8, note_id: u32) -> Self {
        Self { offset, kind: InstrumentNoteKind::Off, key, velocity: 0.0, note_id: Some(note_id), channel: 0 }
    }

    pub fn choke(offset: u32, key: u8, note_id: Option<u32>) -> Self {
        Self { offset, kind: InstrumentNoteKind::Choke, key, velocity: 0.0, note_id, channel: 0 }
    }

    pub fn expression(offset: u32, key: u8, note_id: u32, expr: NoteExpr, value: f64) -> Self {
        Self {
            offset,
            kind: InstrumentNoteKind::Expression(expr, value),
            key,
            velocity: 0.0,
            note_id: Some(note_id),
            channel: 0,
        }
    }
}

/// A NOTE_END reported by the plugin: the voice for this note is fully silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndedNote {
    /// Sample offset within the block passed to `process`.
    pub offset: u32,
    pub note_id: Option<u32>,
    pub key: Option<u8>,
    pub channel: Option<u8>,
}

/// Host transport state for one block (sent as a CLAP transport event).
///
/// Positions are in quarter-note beats, as CLAP defines them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportInfo {
    pub tempo_bpm: f64,
    pub playing: bool,
    /// Song position at the first frame of the block, in beats.
    pub song_pos_beats: f64,
    pub time_sig_num: u16,
    pub time_sig_den: u16,
}

impl Default for TransportInfo {
    fn default() -> Self {
        Self { tempo_bpm: 120.0, playing: false, song_pos_beats: 0.0, time_sig_num: 4, time_sig_den: 4 }
    }
}

impl TransportInfo {
    pub fn new(tempo_bpm: f64, playing: bool, song_pos_beats: f64) -> Self {
        Self { tempo_bpm, playing, song_pos_beats, ..Self::default() }
    }

    /// Bar length in quarter-note beats.
    pub fn beats_per_bar(&self) -> f64 {
        let den = self.time_sig_den.max(1) as f64;
        self.time_sig_num.max(1) as f64 * 4.0 / den
    }

    /// Zero-based bar index containing `song_pos_beats`.
    pub fn bar_number(&self) -> i32 {
        (self.song_pos_beats / self.beats_per_bar()).floor() as i32
    }

    /// Start of the current bar, in beats.
    pub fn bar_start_beats(&self) -> f64 {
        self.bar_number() as f64 * self.beats_per_bar()
    }

    /// Transport after `frames` samples (position only moves while playing).
    pub fn advanced(&self, frames: usize, sample_rate: f64) -> Self {
        let mut next = *self;
        if self.playing && sample_rate > 0.0 {
            next.song_pos_beats += frames as f64 / sample_rate * self.tempo_bpm / 60.0;
        }
        next
    }

    fn to_clap(self) -> TransportEvent {
        let mut flags = TransportFlags::HAS_TEMPO
            | TransportFlags::HAS_BEATS_TIMELINE
            | TransportFlags::HAS_TIME_SIGNATURE;
        if self.playing {
            flags |= TransportFlags::IS_PLAYING;
        }
        TransportEvent {
            header: EventHeader::new_core(0, EventFlags::empty()),
            flags,
            song_pos_beats: BeatTime::from_float(self.song_pos_beats),
            song_pos_seconds: SecondsTime::from_int(0),
            tempo: self.tempo_bpm,
            tempo_inc: 0.0,
            loop_start_beats: BeatTime::from_int(0),
            loop_end_beats: BeatTime::from_int(0),
            loop_start_seconds: SecondsTime::from_int(0),
            loop_end_seconds: SecondsTime::from_int(0),
            bar_start: BeatTime::from_float(self.bar_start_beats()),
            bar_number: self.bar_number(),
            time_signature_numerator: self.time_sig_num,
            time_signature_denominator: self.time_sig_den,
        }
    }
}

/// Which note dialect the instrument is driven with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentNoteDialect {
    Clap,
    Midi,
}

/// Parameter metadata for an instrument.
#[derive(Debug, Clone)]
pub struct InstrumentParamInfo {
    /// CLAP parameter id.
    pub id: u32,
    pub name: String,
    /// Module path ("Osc 1/..."), may be empty.
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub automatable: bool,
    pub modulatable: bool,
    /// Accepts `PARAM_MOD` targeted at a single note id.
    pub modulatable_per_note_id: bool,
    /// Accepts `PARAM_VALUE` targeted at a single note id.
    pub automatable_per_note_id: bool,
    pub stepped: bool,
    pub hidden: bool,
    pub readonly: bool,
}

/// RT-side processing failure (no allocation, `Copy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentProcessError {
    /// The plugin is not active (e.g. reactivation failed after a restart).
    NotActive,
    /// The plugin's `process` returned an error. The block is silenced.
    PluginError,
}

/// Note port summary for inspection.
#[derive(Debug, Clone)]
pub struct NotePortSummary {
    pub name: String,
    pub clap: bool,
    pub midi: bool,
    pub midi_mpe: bool,
    pub midi2: bool,
    pub preferred: Option<&'static str>,
}

/// Voice-info extension result.
#[derive(Debug, Clone, Copy)]
pub struct VoiceInfoSummary {
    pub voice_count: u32,
    pub voice_capacity: u32,
    pub supports_overlapping_notes: bool,
}

/// What a plugin offers for live-mode lanes (used by `clap-scan`).
#[derive(Debug, Clone)]
pub struct InstrumentCapabilities {
    pub note_inputs: Vec<NotePortSummary>,
    /// Channel count per audio input port.
    pub audio_inputs: Vec<u32>,
    /// Channel count per audio output port.
    pub audio_outputs: Vec<u32>,
    /// `None` = no voice-info extension (or `get` failed).
    pub voice_info: Option<VoiceInfoSummary>,
    pub param_count: usize,
    /// Params flagged MODULATABLE_PER_NOTE_ID ("module/name").
    pub per_note_id_modulatable: Vec<String>,
    /// Count of params flagged AUTOMATABLE_PER_NOTE_ID.
    pub per_note_id_automatable: usize,
    pub has_state: bool,
    pub dialect: Option<InstrumentNoteDialect>,
}

// ============================================================================
// Host implementation
// ============================================================================

/// Host handlers for instrument plugins (separate from the effect host so the
/// DJ effect path keeps its exact extension set).
pub struct MeshInstrumentHost;

pub struct InstrumentHostShared {
    plugin_id: String,
    restart_requested: Arc<AtomicBool>,
    callback_requested: Arc<AtomicBool>,
}

pub struct InstrumentHostMainThread<'a> {
    shared: &'a InstrumentHostShared,
}

impl HostHandlers for MeshInstrumentHost {
    type Shared<'a> = InstrumentHostShared;
    type MainThread<'a> = InstrumentHostMainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostParams>()
            .register::<HostState>()
            .register::<HostNotePorts>()
            .register::<HostAudioPorts>()
            .register::<HostVoiceInfo>();
    }
}

impl<'a> SharedHandler<'a> for InstrumentHostShared {
    fn request_restart(&self) {
        log::info!("[CLAP_INST] '{}' requested restart", self.plugin_id);
        self.restart_requested.store(true, Ordering::Release);
    }

    fn request_process(&self) {
        // Always processing
    }

    fn request_callback(&self) {
        self.callback_requested.store(true, Ordering::Release);
    }
}

impl<'a> MainThreadHandler<'a> for InstrumentHostMainThread<'a> {}

impl HostParamsImplShared for InstrumentHostShared {
    fn request_flush(&self) {
        // Params are flushed by the next process() call
    }
}

impl HostParamsImplMainThread for InstrumentHostMainThread<'_> {
    fn rescan(&mut self, _flags: ParamRescanFlags) {}
    fn clear(&mut self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostStateImpl for InstrumentHostMainThread<'_> {
    fn mark_dirty(&mut self) {}
}

impl HostNotePortsImpl for InstrumentHostMainThread<'_> {
    fn supported_dialects(&self) -> NoteDialects {
        NoteDialects::CLAP | NoteDialects::MIDI
    }

    fn rescan(&mut self, _flags: NotePortRescanFlags) {
        self.shared.restart_requested.store(true, Ordering::Release);
    }
}

impl HostAudioPortsImpl for InstrumentHostMainThread<'_> {
    fn is_rescan_flag_supported(&self, _flag: RescanType) -> bool {
        true
    }

    fn rescan(&mut self, _flag: RescanType) {
        // Port layout is re-read on the restart cycle
        self.shared.restart_requested.store(true, Ordering::Release);
    }
}

impl HostVoiceInfoImpl for InstrumentHostMainThread<'_> {
    fn changed(&mut self) {}
}

// ============================================================================
// Event translation (pure, unit-tested without a plugin)
// ============================================================================

/// Stable insertion sort by offset (allocation-free). Equal offsets keep
/// caller order, which is what makes ties (On before Off) work.
fn sort_notes_stable(notes: &mut [InstrumentNote]) {
    for i in 1..notes.len() {
        let mut j = i;
        while j > 0 && notes[j - 1].offset > notes[j].offset {
            notes.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn velocity_to_midi(velocity: f32) -> u8 {
    (velocity.clamp(0.0, 1.0) * 127.0).round() as u8
}

/// MIDI 1.0 bytes for a note, `None` for kinds MIDI cannot express.
fn note_to_midi(note: &InstrumentNote) -> Option<[u8; 3]> {
    let ch = note.channel & 0x0F;
    let key = note.key & 0x7F;
    match note.kind {
        // Velocity 0 would read as note-off in MIDI 1.0
        InstrumentNoteKind::On => Some([0x90 | ch, key, velocity_to_midi(note.velocity).max(1)]),
        InstrumentNoteKind::Off => Some([0x80 | ch, key, velocity_to_midi(note.velocity)]),
        InstrumentNoteKind::Choke => Some([0x80 | ch, key, 0]),
        InstrumentNoteKind::Expression(..) => None,
    }
}

/// Push one note as a CLAP or MIDI event at `time`. Returns `false` if the
/// dialect cannot express it.
fn push_note_event(
    buf: &mut EventBuffer,
    dialect: InstrumentNoteDialect,
    port_index: u16,
    time: u32,
    note: &InstrumentNote,
) -> bool {
    match dialect {
        InstrumentNoteDialect::Clap => {
            let note_id = match note.note_id {
                Some(id) => Match::Specific(id),
                None => Match::All,
            };
            let pckn = Pckn::new(port_index, note.channel as u16, note.key as u16, note_id);
            let velocity = note.velocity.clamp(0.0, 1.0) as f64;
            match note.kind {
                InstrumentNoteKind::On => buf.push(&NoteOnEvent::new(time, pckn, velocity)),
                InstrumentNoteKind::Off => buf.push(&NoteOffEvent::new(time, pckn, velocity)),
                InstrumentNoteKind::Choke => buf.push(&NoteChokeEvent::new(time, pckn)),
                InstrumentNoteKind::Expression(expr, value) => {
                    buf.push(&NoteExpressionEvent::new(time, pckn, expr.to_clap(), value))
                }
            }
            true
        }
        InstrumentNoteDialect::Midi => match note_to_midi(note) {
            Some(data) => {
                buf.push(&MidiEvent::new(time, port_index, data));
                true
            }
            None => false,
        },
    }
}

/// Write the sorted `notes` whose offsets fall in `[start, start + len)` into
/// `buf`, with times relative to `start`. Returns the number of notes that
/// did not fit (capacity) or could not be expressed in the dialect.
fn write_note_events(
    buf: &mut EventBuffer,
    notes: &[InstrumentNote],
    dialect: InstrumentNoteDialect,
    port_index: u16,
    start: u32,
    len: u32,
    capacity: usize,
) -> u32 {
    let end = start + len;
    let mut dropped = 0;
    for note in notes.iter().filter(|n| n.offset >= start && n.offset < end) {
        if buf.len() >= capacity {
            dropped += 1;
            continue;
        }
        if !push_note_event(buf, dialect, port_index, note.offset - start, note) {
            dropped += 1;
        }
    }
    dropped
}

fn pick_dialect(supported: NoteDialects, preferred: Option<NoteDialect>) -> Option<InstrumentNoteDialect> {
    if supported.contains(NoteDialects::CLAP) {
        Some(InstrumentNoteDialect::Clap)
    } else if supported.intersects(NoteDialects::MIDI | NoteDialects::MIDI_MPE)
        || matches!(preferred, Some(NoteDialect::Midi | NoteDialect::MidiMpe))
    {
        Some(InstrumentNoteDialect::Midi)
    } else {
        None
    }
}

fn bytes_to_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string()
}

/// Pending main-thread requests of one instrument (see [`ClapInstrument::idle_signal`]).
#[derive(Clone)]
pub struct IdleSignal {
    restart: Arc<AtomicBool>,
    callback: Arc<AtomicBool>,
}

impl IdleSignal {
    /// True when the plugin asked for a restart or a main-thread callback.
    pub fn pending(&self) -> bool {
        self.restart.load(Ordering::Acquire) || self.callback.load(Ordering::Acquire)
    }
}

// ============================================================================
// ClapInstrument
// ============================================================================

/// A loaded, active CLAP instrument plugin.
pub struct ClapInstrument {
    instance: PluginInstance<MeshInstrumentHost>,
    processor: Option<StartedPluginAudioProcessor<MeshInstrumentHost>>,
    info: DiscoveredClapPlugin,
    dialect: Option<InstrumentNoteDialect>,
    note_port_index: u16,
    sample_rate: f64,
    /// Channel count per input / output port.
    in_layout: Vec<usize>,
    out_layout: Vec<usize>,
    main_out_port: usize,
    /// One slab per port, `channels * INSTRUMENT_MAX_FRAMES` floats.
    in_bufs: Vec<Vec<f32>>,
    out_bufs: Vec<Vec<f32>>,
    input_ports: AudioPorts,
    output_ports: AudioPorts,
    in_events: EventBuffer,
    out_events: EventBuffer,
    note_scratch: Vec<InstrumentNote>,
    params: Vec<InstrumentParamInfo>,
    param_values: Vec<f64>,
    param_index_by_id: HashMap<u32, usize>,
    pending_params: Vec<(u32, f64)>,
    ended: Vec<EndedNote>,
    steady_time: u64,
    dropped_events: u32,
    restart_requested: Arc<AtomicBool>,
    callback_requested: Arc<AtomicBool>,
    /// Keep the bundle alive; must drop after `instance`.
    _bundle: Arc<PluginBundle>,
}

// Safety: same reasoning as `ClapPluginWrapper`: the instance is moved, not
// shared; main-thread and audio-thread calls are serialised by `&mut self`.
unsafe impl Send for ClapInstrument {}

impl ClapInstrument {
    /// Load and activate an instrument. Fails when the plugin has no input
    /// note port speaking CLAP or MIDI, or no audio output.
    pub fn load(
        plugin_info: &DiscoveredClapPlugin,
        bundle: Arc<PluginBundle>,
        sample_rate: u32,
    ) -> ClapResult<Self> {
        let inst = Self::instantiate(plugin_info, bundle, sample_rate)?;
        if inst.dialect.is_none() {
            return Err(ClapError::InstantiationFailed {
                plugin_id: plugin_info.id.clone(),
                reason: "no input note port with CLAP or MIDI dialect".to_string(),
            });
        }
        Ok(inst)
    }

    /// Load an instrument straight from a `.clap` bundle path. `plugin_id`
    /// selects a plugin inside the bundle; `None` takes the first instrument
    /// (or the first plugin if none is tagged as one).
    pub fn load_from_path(bundle_path: &Path, plugin_id: Option<&str>, sample_rate: u32) -> ClapResult<Self> {
        let (info, bundle) = Self::resolve_bundle(bundle_path, plugin_id)?;
        Self::load(&info, bundle, sample_rate)
    }

    /// Like [`load_from_path`](Self::load_from_path) but tolerates a missing
    /// note port, so inspection tools can still report on the plugin.
    pub fn open_for_inspection(bundle_path: &Path, plugin_id: Option<&str>, sample_rate: u32) -> ClapResult<Self> {
        let (info, bundle) = Self::resolve_bundle(bundle_path, plugin_id)?;
        Self::instantiate(&info, bundle, sample_rate)
    }

    fn resolve_bundle(
        bundle_path: &Path,
        plugin_id: Option<&str>,
    ) -> ClapResult<(DiscoveredClapPlugin, Arc<PluginBundle>)> {
        let plugins = ClapDiscovery::with_paths(Vec::new()).scan_bundle(bundle_path)?;
        let info = match plugin_id {
            Some(id) => plugins.into_iter().find(|p| p.id == id),
            None => {
                let first_instrument = plugins.iter().position(|p| p.is_instrument());
                let idx = first_instrument.unwrap_or(0);
                plugins.into_iter().nth(idx)
            }
        }
        .ok_or_else(|| ClapError::PluginNotFound {
            plugin_id: plugin_id.unwrap_or("<first instrument>").to_string(),
            bundle_path: bundle_path.to_path_buf(),
        })?;

        let bundle = unsafe {
            PluginBundle::load(bundle_path).map_err(|e| ClapError::BundleLoadFailed {
                path: PathBuf::from(bundle_path),
                reason: format!("{:?}", e),
            })?
        };
        Ok((info, Arc::new(bundle)))
    }

    fn instantiate(plugin_info: &DiscoveredClapPlugin, bundle: Arc<PluginBundle>, sample_rate: u32) -> ClapResult<Self> {
        let id = plugin_info.id.clone();
        let fail = |reason: String| ClapError::InstantiationFailed { plugin_id: id.clone(), reason };

        let plugin_id = CString::new(plugin_info.id.as_str()).map_err(|_| fail("plugin id contains NUL".into()))?;
        let host_info = HostInfo::new("Mesh Live", "Mesh", "https://github.com/mesh", "0.1.0")
            .map_err(|e| fail(format!("host info: {:?}", e)))?;

        let restart_requested = Arc::new(AtomicBool::new(false));
        let callback_requested = Arc::new(AtomicBool::new(false));
        let (restart_for_host, callback_for_host) = (Arc::clone(&restart_requested), Arc::clone(&callback_requested));
        let host_plugin_id = plugin_info.id.clone();

        let instance = PluginInstance::<MeshInstrumentHost>::new(
            move |_| InstrumentHostShared {
                plugin_id: host_plugin_id,
                restart_requested: restart_for_host,
                callback_requested: callback_for_host,
            },
            |shared| InstrumentHostMainThread { shared },
            &bundle,
            &plugin_id,
            &host_info,
        )
        .map_err(|e| fail(format!("{:?}", e)))?;

        let mut inst = Self {
            instance,
            processor: None,
            info: plugin_info.clone(),
            dialect: None,
            note_port_index: 0,
            sample_rate: sample_rate as f64,
            in_layout: Vec::new(),
            out_layout: Vec::new(),
            main_out_port: 0,
            in_bufs: Vec::new(),
            out_bufs: Vec::new(),
            input_ports: AudioPorts::with_capacity(0, 0),
            output_ports: AudioPorts::with_capacity(0, 0),
            in_events: EventBuffer::with_capacity(MAX_INPUT_EVENTS),
            out_events: EventBuffer::with_capacity(MAX_OUTPUT_EVENTS),
            note_scratch: Vec::with_capacity(MAX_NOTES_PER_BLOCK),
            params: Vec::new(),
            param_values: Vec::new(),
            param_index_by_id: HashMap::new(),
            pending_params: Vec::with_capacity(MAX_PENDING_PARAMS),
            ended: Vec::with_capacity(MAX_ENDED_NOTES),
            steady_time: 0,
            dropped_events: 0,
            restart_requested,
            callback_requested,
            _bundle: bundle,
        };

        inst.configure_ports()?;
        inst.query_params();
        inst.activate()?;
        Ok(inst)
    }

    /// Read note and audio port layouts and size the buffers (main thread, inactive).
    fn configure_ports(&mut self) -> ClapResult<()> {
        let (dialect, port_index) = self.find_note_port();
        self.dialect = dialect;
        self.note_port_index = port_index;

        let mut handle = self.instance.plugin_handle();
        let audio_ports = handle.get_extension::<PluginAudioPorts>().ok_or_else(|| ClapError::InstantiationFailed {
            plugin_id: self.info.id.clone(),
            reason: "plugin has no audio-ports extension".to_string(),
        })?;

        let mut buf = AudioPortInfoBuffer::new();
        let mut layout = |is_input: bool, handle: &mut PluginMainThreadHandle| {
            let mut channels = Vec::new();
            let mut main = None;
            for i in 0..audio_ports.count(handle, is_input) {
                if let Some(port) = audio_ports.get(handle, i, is_input, &mut buf) {
                    if main.is_none() && port.flags.contains(AudioPortFlags::IS_MAIN) {
                        main = Some(channels.len());
                    }
                    channels.push(port.channel_count as usize);
                }
            }
            (channels, main.unwrap_or(0))
        };
        let (in_layout, _) = layout(true, &mut handle);
        let (out_layout, main_out) = layout(false, &mut handle);

        if out_layout.get(main_out).copied().unwrap_or(0) == 0 {
            return Err(ClapError::InstantiationFailed {
                plugin_id: self.info.id.clone(),
                reason: "plugin has no audio output channels".to_string(),
            });
        }

        let slab = |ch: &usize| vec![0.0f32; ch * INSTRUMENT_MAX_FRAMES];
        self.in_bufs = in_layout.iter().map(slab).collect();
        self.out_bufs = out_layout.iter().map(slab).collect();
        self.input_ports = AudioPorts::with_capacity(in_layout.iter().sum(), in_layout.len());
        self.output_ports = AudioPorts::with_capacity(out_layout.iter().sum(), out_layout.len());
        self.in_layout = in_layout;
        self.out_layout = out_layout;
        self.main_out_port = main_out;
        Ok(())
    }

    fn find_note_port(&mut self) -> (Option<InstrumentNoteDialect>, u16) {
        let mut handle = self.instance.plugin_handle();
        let Some(note_ports) = handle.get_extension::<PluginNotePorts>() else {
            return (None, 0);
        };
        let mut buf = NotePortInfoBuffer::new();
        for i in 0..note_ports.count(&mut handle, true) {
            if let Some(port) = note_ports.get(&mut handle, i, true, &mut buf) {
                if let Some(d) = pick_dialect(port.supported_dialects, port.preferred_dialect) {
                    return (Some(d), i as u16);
                }
            }
        }
        (None, 0)
    }

    fn activate(&mut self) -> ClapResult<()> {
        if self.processor.is_some() {
            return Ok(());
        }
        let config = PluginAudioConfiguration {
            sample_rate: self.sample_rate,
            min_frames_count: 1,
            max_frames_count: INSTRUMENT_MAX_FRAMES as u32,
        };
        let stopped = self.instance.activate(|_, _| (), config).map_err(|e| ClapError::ActivationFailed {
            plugin_id: self.info.id.clone(),
            reason: format!("{:?}", e),
        })?;
        let started = stopped.start_processing().map_err(|e| ClapError::ActivationFailed {
            plugin_id: self.info.id.clone(),
            reason: format!("start_processing: {:?}", e),
        })?;
        self.processor = Some(started);
        log::info!(
            "[CLAP_INST] '{}' active at {} Hz, dialect {:?}, audio in {:?} out {:?}",
            self.info.id, self.sample_rate, self.dialect, self.in_layout, self.out_layout
        );
        Ok(())
    }

    fn deactivate(&mut self) {
        if let Some(processor) = self.processor.take() {
            let stopped = processor.stop_processing();
            self.instance.deactivate(stopped);
        }
    }

    // ------------------------------------------------------------------------
    // Info
    // ------------------------------------------------------------------------

    pub fn info(&self) -> &DiscoveredClapPlugin {
        &self.info
    }

    pub fn plugin_id(&self) -> &str {
        &self.info.id
    }

    /// Note dialect in use (`None` only for instances opened for inspection).
    pub fn dialect(&self) -> Option<InstrumentNoteDialect> {
        self.dialect
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn is_active(&self) -> bool {
        self.processor.is_some()
    }

    /// Events dropped since the last [`take_dropped_events`](Self::take_dropped_events)
    /// (capacity overflow or not expressible in the dialect).
    pub fn dropped_events(&self) -> u32 {
        self.dropped_events
    }

    pub fn take_dropped_events(&mut self) -> u32 {
        std::mem::take(&mut self.dropped_events)
    }

    /// NOTE_END events reported during the last `process` call.
    pub fn ended_notes(&self) -> &[EndedNote] {
        &self.ended
    }

    /// Inspect note ports, audio ports, voice info and param flags (main thread).
    pub fn capabilities(&mut self) -> InstrumentCapabilities {
        let mut handle = self.instance.plugin_handle();

        let mut note_inputs = Vec::new();
        if let Some(note_ports) = handle.get_extension::<PluginNotePorts>() {
            let mut buf = NotePortInfoBuffer::new();
            for i in 0..note_ports.count(&mut handle, true) {
                if let Some(port) = note_ports.get(&mut handle, i, true, &mut buf) {
                    let d = port.supported_dialects;
                    note_inputs.push(NotePortSummary {
                        name: bytes_to_string(port.name),
                        clap: d.contains(NoteDialects::CLAP),
                        midi: d.contains(NoteDialects::MIDI),
                        midi_mpe: d.contains(NoteDialects::MIDI_MPE),
                        midi2: d.contains(NoteDialects::MIDI2),
                        preferred: port.preferred_dialect.map(|p| match p {
                            NoteDialect::Clap => "clap",
                            NoteDialect::Midi => "midi",
                            NoteDialect::MidiMpe => "midi-mpe",
                            NoteDialect::Midi2 => "midi2",
                        }),
                    });
                }
            }
        }

        let voice_info = handle.get_extension::<PluginVoiceInfo>().and_then(|ext| ext.get(&mut handle)).map(|v| {
            VoiceInfoSummary {
                voice_count: v.voice_count,
                voice_capacity: v.voice_capacity,
                supports_overlapping_notes: v.flags.contains(VoiceInfoFlags::SUPPORTS_OVERLAPPING_NOTES),
            }
        });
        let has_state = handle.get_extension::<PluginState>().is_some();

        InstrumentCapabilities {
            note_inputs,
            audio_inputs: self.in_layout.iter().map(|&c| c as u32).collect(),
            audio_outputs: self.out_layout.iter().map(|&c| c as u32).collect(),
            voice_info,
            param_count: self.params.len(),
            per_note_id_modulatable: self
                .params
                .iter()
                .filter(|p| p.modulatable_per_note_id)
                .map(|p| match p.module.trim_matches('/') {
                    "" => p.name.clone(),
                    module => format!("{}/{}", module, p.name),
                })
                .collect(),
            per_note_id_automatable: self.params.iter().filter(|p| p.automatable_per_note_id).count(),
            has_state,
            dialect: self.dialect,
        }
    }

    // ------------------------------------------------------------------------
    // Params
    // ------------------------------------------------------------------------

    fn query_params(&mut self) {
        let mut handle = self.instance.plugin_handle();
        let Some(params_ext) = handle.get_extension::<PluginParams>() else {
            self.params.clear();
            return;
        };
        let count = params_ext.count(&mut handle);
        let mut buf = ParamInfoBuffer::new();
        let mut params = Vec::with_capacity(count as usize);
        for i in 0..count {
            if let Some(p) = params_ext.get_info(&mut handle, i, &mut buf) {
                let f = p.flags;
                params.push(InstrumentParamInfo {
                    id: p.id.get(),
                    name: bytes_to_string(p.name),
                    module: bytes_to_string(p.module),
                    min: p.min_value,
                    max: p.max_value,
                    default: p.default_value,
                    automatable: f.contains(ParamInfoFlags::IS_AUTOMATABLE),
                    modulatable: f.contains(ParamInfoFlags::IS_MODULATABLE),
                    modulatable_per_note_id: f.contains(ParamInfoFlags::IS_MODULATABLE_PER_NOTE_ID),
                    automatable_per_note_id: f.contains(ParamInfoFlags::IS_AUTOMATABLE_PER_NOTE_ID),
                    stepped: f.contains(ParamInfoFlags::IS_STEPPED),
                    hidden: f.contains(ParamInfoFlags::IS_HIDDEN),
                    readonly: f.contains(ParamInfoFlags::IS_READONLY),
                });
            }
        }
        self.param_index_by_id = params.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        self.param_values = params.iter().map(|p| p.default).collect();
        self.params = params;
        self.refresh_param_values();
    }

    /// Re-read every param value from the plugin into the cache (main thread).
    pub fn refresh_param_values(&mut self) {
        let mut handle = self.instance.plugin_handle();
        let Some(params_ext) = handle.get_extension::<PluginParams>() else {
            return;
        };
        for (i, p) in self.params.iter().enumerate() {
            if let Some(v) = params_ext.get_value(&mut handle, ClapId::new(p.id)) {
                self.param_values[i] = v;
            }
        }
    }

    /// All parameters, in plugin index order.
    pub fn params(&self) -> &[InstrumentParamInfo] {
        &self.params
    }

    pub fn param_index_for_id(&self, param_id: u32) -> Option<usize> {
        self.param_index_by_id.get(&param_id).copied()
    }

    /// Cached value (native range) of param `index`: last value set by the
    /// host, reported by the plugin, or read by `refresh_param_values`.
    pub fn param_value(&self, index: usize) -> Option<f64> {
        self.param_values.get(index).copied()
    }

    /// Queue a block-rate PARAM_VALUE (native range) for the next `process`.
    /// RT-safe. A second set of the same param before `process` overwrites
    /// the first. Returns `false` for a bad index or a full queue.
    pub fn set_param(&mut self, index: usize, value: f64) -> bool {
        let Some(p) = self.params.get(index) else {
            return false;
        };
        let value = value.clamp(p.min.min(p.max), p.max.max(p.min));
        let id = p.id;
        self.param_values[index] = value;
        if let Some(slot) = self.pending_params.iter_mut().find(|(pid, _)| *pid == id) {
            slot.1 = value;
            return true;
        }
        if self.pending_params.len() >= MAX_PENDING_PARAMS {
            self.dropped_events = self.dropped_events.saturating_add(1);
            return false;
        }
        self.pending_params.push((id, value));
        true
    }

    /// Like [`set_param`](Self::set_param) with a 0..1 value mapped to the param range.
    pub fn set_param_normalized(&mut self, index: usize, normalized: f64) -> bool {
        let Some(p) = self.params.get(index) else {
            return false;
        };
        let value = p.min + normalized.clamp(0.0, 1.0) * (p.max - p.min);
        self.set_param(index, value)
    }

    // ------------------------------------------------------------------------
    // State / presets
    // ------------------------------------------------------------------------

    fn state_error(&self, reason: impl Into<String>) -> ClapError {
        ClapError::StateError { plugin_id: self.info.id.clone(), reason: reason.into() }
    }

    /// Save the plugin state blob (main thread).
    pub fn save_state(&mut self) -> ClapResult<Vec<u8>> {
        let state = self.instance.plugin_handle().get_extension::<PluginState>().ok_or_else(|| self.state_error("no state extension"))?;
        let mut out = Vec::new();
        let mut handle = self.instance.plugin_handle();
        state.save(&mut handle, &mut out).map_err(|e| self.state_error(format!("save failed: {:?}", e)))?;
        Ok(out)
    }

    /// Load a plugin state blob (main thread). Pending param changes are
    /// discarded and the param cache is re-read.
    pub fn load_state(&mut self, bytes: &[u8]) -> ClapResult<()> {
        let state = self.instance.plugin_handle().get_extension::<PluginState>().ok_or_else(|| self.state_error("no state extension"))?;
        let mut reader = bytes;
        let mut handle = self.instance.plugin_handle();
        state.load(&mut handle, &mut reader).map_err(|e| self.state_error(format!("load failed: {:?}", e)))?;
        self.pending_params.clear();
        self.refresh_param_values();
        Ok(())
    }

    /// Save the state blob to `path` (style synth presets reference these files).
    pub fn save_state_to_file(&mut self, path: &Path) -> ClapResult<()> {
        let bytes = self.save_state()?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// Load a state blob from `path`. A VST2 `.fxp` chunk preset (Surge XT's
    /// patch format) loads as its chunk: plugins that save their state as
    /// that chunk read it back without the 60-byte fxp header.
    pub fn load_state_from_file(&mut self, path: &Path) -> ClapResult<()> {
        let bytes = std::fs::read(path)?;
        self.load_state(fxp_chunk(&bytes).unwrap_or(&bytes))
    }

    // ------------------------------------------------------------------------
    // Main-thread housekeeping
    // ------------------------------------------------------------------------

    /// Lock-free view of the plugin's pending main-thread requests. Lets a
    /// housekeeping thread decide whether [`idle`](Self::idle) has work to do
    /// without touching the instrument itself.
    pub fn idle_signal(&self) -> IdleSignal {
        IdleSignal {
            restart: Arc::clone(&self.restart_requested),
            callback: Arc::clone(&self.callback_requested),
        }
    }

    /// Service plugin requests (main thread): runs `on_main_thread` when the
    /// plugin asked for a callback and performs a deactivate → reactivate
    /// cycle on restart / port rescan. Returns `true` if a restart happened.
    pub fn idle(&mut self) -> ClapResult<bool> {
        if self.callback_requested.swap(false, Ordering::AcqRel) {
            self.instance.call_on_main_thread_callback();
        }
        if !self.restart_requested.swap(false, Ordering::AcqRel) {
            return Ok(false);
        }
        self.deactivate();
        self.configure_ports()?;
        self.activate()?;
        Ok(true)
    }

    // ------------------------------------------------------------------------
    // Audio
    // ------------------------------------------------------------------------

    /// Render one block into `out` (overwritten, not mixed).
    ///
    /// `notes` carry offsets relative to the first frame of `out`; they are
    /// stably sorted here. Pending param changes are applied at frame 0.
    /// Blocks longer than [`INSTRUMENT_MAX_FRAMES`] are split, with the
    /// transport advanced per sub-block. RT-safe: no allocation, no locks.
    pub fn process(
        &mut self,
        out: &mut StereoBuffer,
        notes: &[InstrumentNote],
        transport: &TransportInfo,
    ) -> Result<(), InstrumentProcessError> {
        self.ended.clear();
        let frames = out.len();
        if frames == 0 {
            return Ok(());
        }

        // Copy into fixed scratch, clamp offsets, stable sort
        self.note_scratch.clear();
        let last_frame = (frames - 1) as u32;
        for note in notes {
            if self.note_scratch.len() >= MAX_NOTES_PER_BLOCK {
                self.dropped_events = self.dropped_events.saturating_add(1);
                continue;
            }
            let mut n = *note;
            n.offset = n.offset.min(last_frame);
            self.note_scratch.push(n);
        }
        sort_notes_stable(&mut self.note_scratch);

        let out = out.as_mut_slice();
        let mut t = *transport;
        let mut start = 0;
        let mut result = Ok(());
        while start < frames {
            let len = (frames - start).min(INSTRUMENT_MAX_FRAMES);
            if let Err(e) = self.process_chunk(&mut out[start..start + len], start as u32, &t) {
                out[start..start + len].fill(StereoSample::silence());
                result = Err(e);
            }
            t = t.advanced(len, self.sample_rate);
            start += len;
        }
        result
    }

    fn process_chunk(
        &mut self,
        out: &mut [StereoSample],
        start: u32,
        transport: &TransportInfo,
    ) -> Result<(), InstrumentProcessError> {
        let n = out.len();
        let Self {
            processor,
            dialect,
            note_port_index,
            in_layout,
            out_layout,
            main_out_port,
            in_bufs,
            out_bufs,
            input_ports,
            output_ports,
            in_events,
            out_events,
            note_scratch,
            param_values,
            param_index_by_id,
            pending_params,
            ended,
            steady_time,
            dropped_events,
            ..
        } = self;
        let processor = processor.as_mut().ok_or(InstrumentProcessError::NotActive)?;

        // Input events: params at t=0, then notes (already time-sorted)
        in_events.clear();
        for &(id, value) in pending_params.iter() {
            in_events.push(&ParamValueEvent::new(0, ClapId::new(id), Pckn::match_all(), value, Cookie::empty()));
        }
        pending_params.clear();
        if let Some(d) = *dialect {
            let dropped = write_note_events(
                in_events,
                note_scratch,
                d,
                *note_port_index,
                start,
                n as u32,
                MAX_INPUT_EVENTS,
            );
            *dropped_events = dropped_events.saturating_add(dropped);
        }

        for buf in in_bufs.iter_mut() {
            for ch in buf.chunks_exact_mut(INSTRUMENT_MAX_FRAMES) {
                ch[..n].fill(0.0);
            }
        }

        let input_buffers = input_ports.with_input_buffers(in_bufs.iter_mut().zip(in_layout.iter()).map(
            move |(buf, &ch)| AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    buf.chunks_exact_mut(INSTRUMENT_MAX_FRAMES)
                        .take(ch)
                        .map(move |c| InputChannel { buffer: &mut c[..n], is_constant: false }),
                ),
            },
        ));
        let mut output_buffers = output_ports.with_output_buffers(out_bufs.iter_mut().zip(out_layout.iter()).map(
            move |(buf, &ch)| AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only(
                    buf.chunks_exact_mut(INSTRUMENT_MAX_FRAMES).take(ch).map(move |c| &mut c[..n]),
                ),
            },
        ));

        out_events.clear();
        let transport_event = transport.to_clap();
        let status = processor.process(
            &input_buffers,
            &mut output_buffers,
            &in_events.as_input(),
            &mut out_events.as_output(),
            Some(*steady_time),
            Some(&transport_event),
        );
        *steady_time += n as u64;
        if status.is_err() {
            return Err(InstrumentProcessError::PluginError);
        }

        // Main output port → caller buffer
        let main = &out_bufs[*main_out_port];
        let left = &main[..n];
        let right = if out_layout[*main_out_port] >= 2 {
            &main[INSTRUMENT_MAX_FRAMES..INSTRUMENT_MAX_FRAMES + n]
        } else {
            left
        };
        for (i, s) in out.iter_mut().enumerate() {
            s.left = left[i];
            s.right = right[i];
        }

        // Output events: NOTE_END → ended list, PARAM_VALUE → value cache
        for event in out_events.iter() {
            if let Some(end) = event.as_event::<NoteEndEvent>() {
                if ended.len() < MAX_ENDED_NOTES {
                    ended.push(EndedNote {
                        offset: start + end.header().time(),
                        note_id: end.note_id().into_specific(),
                        key: end.key().into_specific().map(|k| k as u8),
                        channel: end.channel().into_specific().map(|c| c as u8),
                    });
                }
            } else if let Some(pv) = event.as_event::<ParamValueEvent>() {
                if let Some(idx) = pv.param_id().and_then(|id| param_index_by_id.get(&id.get())) {
                    param_values[*idx] = pv.value();
                }
            }
        }
        Ok(())
    }
}

impl Drop for ClapInstrument {
    fn drop(&mut self) {
        self.deactivate();
    }
}

/// Size of a VST2 fxp program-chunk header (`CcnK` … `FPCh` … chunk size).
const FXP_CHUNK_HEADER: usize = 60;

/// The chunk of a VST2 `.fxp` program-chunk preset, `None` for anything else.
fn fxp_chunk(bytes: &[u8]) -> Option<&[u8]> {
    (bytes.len() > FXP_CHUNK_HEADER && &bytes[0..4] == b"CcnK" && &bytes[8..12] == b"FPCh")
        .then(|| &bytes[FXP_CHUNK_HEADER..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fxp_chunk_strips_the_header_only_from_chunk_presets() {
        let mut fxp = b"CcnK\0\0\0\0FPCh".to_vec();
        fxp.resize(FXP_CHUNK_HEADER, 0);
        fxp.extend_from_slice(b"sub3data");
        assert_eq!(fxp_chunk(&fxp), Some(&b"sub3data"[..]));
        assert_eq!(fxp_chunk(b"<?xml version=\"1.0\"?>"), None);
        let mut params_preset = fxp.clone();
        params_preset[8..12].copy_from_slice(b"FxCk");
        assert_eq!(fxp_chunk(&params_preset), None, "parameter presets are not chunks");
    }
    use clack_host::events::UnknownEvent;

    fn kinds(buf: &EventBuffer) -> Vec<(u32, u16)> {
        buf.iter().map(|e: &UnknownEvent| (e.header().time(), e.header().type_id())).collect()
    }

    #[test]
    fn sort_is_stable_for_equal_offsets() {
        // Tie: new On (62) must stay before the previous Off (60) at the same offset
        let mut notes = vec![
            InstrumentNote::off(100, 60, 1),
            InstrumentNote::on(10, 60, 0.8, 1),
            InstrumentNote::on(100, 62, 0.8, 2),
        ];
        // Caller wants On(62) before Off(60) at 100: swap to that order first
        notes.swap(0, 2);
        sort_notes_stable(&mut notes);
        assert_eq!(notes[0].offset, 10);
        assert_eq!((notes[1].kind, notes[1].key), (InstrumentNoteKind::On, 62));
        assert_eq!((notes[2].kind, notes[2].key), (InstrumentNoteKind::Off, 60));
    }

    #[test]
    fn clap_dialect_maps_kinds_and_times() {
        let notes = [
            InstrumentNote::on(0, 60, 1.0, 7),
            InstrumentNote::on(64, 62, 0.5, 8),
            InstrumentNote::off(64, 60, 7),
            InstrumentNote::choke(128, 62, None),
            InstrumentNote::expression(130, 62, 8, NoteExpr::Tuning, 2.0),
        ];
        let mut buf = EventBuffer::with_capacity(16);
        let dropped = write_note_events(&mut buf, &notes, InstrumentNoteDialect::Clap, 0, 0, 256, 16);
        assert_eq!(dropped, 0);
        assert_eq!(
            kinds(&buf),
            vec![
                (0, NoteOnEvent::TYPE_ID),
                (64, NoteOnEvent::TYPE_ID),
                (64, NoteOffEvent::TYPE_ID),
                (128, NoteChokeEvent::TYPE_ID),
                (130, NoteExpressionEvent::TYPE_ID),
            ]
        );
        let on = buf.get(1).unwrap().as_event::<NoteOnEvent>().unwrap();
        assert_eq!(on.key(), Match::Specific(62));
        assert_eq!(on.note_id(), Match::Specific(8));
        assert!((on.velocity() - 0.5).abs() < 1e-9);
        let choke = buf.get(3).unwrap().as_event::<NoteChokeEvent>().unwrap();
        assert_eq!(choke.note_id(), Match::All);
    }

    #[test]
    fn midi_dialect_bytes() {
        let mut on = InstrumentNote::on(5, 60, 1.0, 1);
        on.channel = 3;
        assert_eq!(note_to_midi(&on), Some([0x93, 60, 127]));
        // Velocity 0 note-on must not become a MIDI note-off
        let quiet = InstrumentNote::on(0, 61, 0.0, 2);
        assert_eq!(note_to_midi(&quiet), Some([0x90, 61, 1]));
        assert_eq!(note_to_midi(&InstrumentNote::off(0, 60, 1)), Some([0x80, 60, 0]));
        assert_eq!(note_to_midi(&InstrumentNote::choke(0, 60, None)), Some([0x80, 60, 0]));
        let expr = InstrumentNote::expression(0, 60, 1, NoteExpr::Pan, 0.2);
        assert_eq!(note_to_midi(&expr), None);

        let mut buf = EventBuffer::with_capacity(4);
        let dropped = write_note_events(&mut buf, &[on, expr], InstrumentNoteDialect::Midi, 0, 0, 64, 4);
        assert_eq!(dropped, 1);
        let midi = buf.get(0).unwrap().as_event::<MidiEvent>().unwrap();
        assert_eq!(midi.data(), [0x93, 60, 127]);
        assert_eq!(midi.header().time(), 5);
    }

    #[test]
    fn chunk_window_rebases_offsets_and_caps_capacity() {
        let notes = [
            InstrumentNote::on(10, 60, 1.0, 1),
            InstrumentNote::on(300, 61, 1.0, 2),
            InstrumentNote::on(301, 62, 1.0, 3),
            InstrumentNote::on(302, 63, 1.0, 4),
        ];
        let mut buf = EventBuffer::with_capacity(4);
        // Window [256, 512) with room for 2 events
        let dropped = write_note_events(&mut buf, &notes, InstrumentNoteDialect::Clap, 0, 256, 256, 2);
        assert_eq!(dropped, 1);
        assert_eq!(buf.iter().map(|e| e.header().time()).collect::<Vec<_>>(), vec![44, 45]);
    }

    #[test]
    fn dialect_preference() {
        assert_eq!(pick_dialect(NoteDialects::CLAP | NoteDialects::MIDI, None), Some(InstrumentNoteDialect::Clap));
        assert_eq!(pick_dialect(NoteDialects::MIDI, None), Some(InstrumentNoteDialect::Midi));
        assert_eq!(pick_dialect(NoteDialects::MIDI_MPE, None), Some(InstrumentNoteDialect::Midi));
        assert_eq!(pick_dialect(NoteDialects::MIDI2, None), None);
    }

    #[test]
    fn transport_fields() {
        let t = TransportInfo::new(174.0, true, 9.5);
        assert_eq!(t.bar_number(), 2);
        assert_eq!(t.bar_start_beats(), 8.0);
        let ev = t.to_clap();
        assert!(ev.flags.contains(TransportFlags::IS_PLAYING | TransportFlags::HAS_TEMPO));
        assert_eq!(ev.tempo, 174.0);
        assert_eq!(ev.song_pos_beats.to_float(), 9.5);
        assert_eq!(ev.bar_start.to_float(), 8.0);
        assert_eq!(ev.bar_number, 2);

        // 7/8: bar = 3.5 quarter beats
        let t = TransportInfo { time_sig_num: 7, time_sig_den: 8, ..TransportInfo::new(120.0, true, 7.0) };
        assert_eq!(t.bar_number(), 2);

        // 120 bpm at 48 kHz: 24000 frames = 1 beat; stopped transport does not move
        let moved = TransportInfo::new(120.0, true, 0.0).advanced(24000, 48000.0);
        assert!((moved.song_pos_beats - 1.0).abs() < 1e-9);
        let still = TransportInfo::new(120.0, false, 3.0).advanced(24000, 48000.0);
        assert_eq!(still.song_pos_beats, 3.0);
        assert!(!still.to_clap().flags.contains(TransportFlags::IS_PLAYING));
    }

    /// Real-plugin check. Run with
    /// `MESH_TEST_CLAP_INSTRUMENT=/path/to/Synth.clap cargo test -p mesh-core --lib clap::instrument -- --ignored`
    /// (optionally `MESH_TEST_CLAP_INSTRUMENT_ID=<plugin id>`).
    #[test]
    #[ignore = "needs MESH_TEST_CLAP_INSTRUMENT=/path/to/plugin.clap"]
    fn real_instrument_plays_and_releases() {
        let Ok(path) = std::env::var("MESH_TEST_CLAP_INSTRUMENT") else {
            panic!("set MESH_TEST_CLAP_INSTRUMENT to a .clap bundle path");
        };
        let id = std::env::var("MESH_TEST_CLAP_INSTRUMENT_ID").ok();
        let sr = 48_000u32;
        let mut inst = ClapInstrument::load_from_path(Path::new(&path), id.as_deref(), sr).expect("load instrument");
        let block = 256usize;
        let mut buf = StereoBuffer::silence(block);
        let mut t = TransportInfo::new(120.0, true, 0.0);

        let rms = |b: &StereoBuffer| {
            (b.as_slice().iter().map(|s| (s.left * s.left + s.right * s.right) as f64).sum::<f64>()
                / (2 * b.len()) as f64)
                .sqrt()
        };

        // Let the plugin settle, then hold a note for ~0.5 s
        for _ in 0..4 {
            inst.process(&mut buf, &[], &t).unwrap();
            t = t.advanced(block, sr as f64);
        }
        let hold_blocks = (sr as usize / 2) / block;
        let mut held_rms = 0.0f64;
        for i in 0..hold_blocks {
            let notes = if i == 0 { vec![InstrumentNote::on(17, 57, 0.9, 1)] } else { vec![] };
            inst.process(&mut buf, &notes, &t).unwrap();
            if i >= hold_blocks / 2 {
                held_rms = held_rms.max(rms(&buf));
            }
            t = t.advanced(block, sr as f64);
        }
        assert!(held_rms > 1e-3, "held note is silent (rms {held_rms})");

        // Release and let it decay for up to 3 s, collecting NOTE_END
        let mut saw_end = false;
        let mut tail_rms = f64::MAX;
        for i in 0..(3 * sr as usize / block) {
            let notes = if i == 0 { vec![InstrumentNote::off(0, 57, 1)] } else { vec![] };
            inst.process(&mut buf, &notes, &t).unwrap();
            saw_end |= inst.ended_notes().iter().any(|e| e.note_id == Some(1) || e.key == Some(57));
            tail_rms = rms(&buf);
            t = t.advanced(block, sr as f64);
        }
        eprintln!(
            "plugin={} dialect={:?} held_rms={held_rms:.4} tail_rms={tail_rms:.6} note_end={saw_end} params={}",
            inst.plugin_id(),
            inst.dialect(),
            inst.params().len()
        );
        assert!(tail_rms < held_rms * 0.05, "note did not decay (held {held_rms}, tail {tail_rms})");

        // State round trip
        if let Ok(state) = inst.save_state() {
            assert!(!state.is_empty());
            inst.load_state(&state).expect("reload own state");
        }
    }
}
