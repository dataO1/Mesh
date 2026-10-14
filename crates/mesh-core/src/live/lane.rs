//! One live lane on the RT thread (design §3, §5).
//!
//! ```text
//! events ─► instrument (Sampler | ClapInstrument) ─► lane FX chain ─► on/off gain ─► lane buffer
//! ```
//!
//! A lane owns everything a kit or instrument swap needs, preloaded: every
//! option's [`SampleBank`] and every CLAP instrument of the lane (D36). A swap
//! on the RT thread is a `Shared` clone or an index change.
//!
//! RT contract: after construction nothing here allocates, locks or blocks.
//! CLAP instruments are shared with the housekeeping thread through
//! [`ClapCell`], a try-only flag: the RT thread never waits for it and plays
//! silence for the one block in which a plugin restart holds the instrument.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use basedrop::Shared;

use crate::clap::{ClapInstrument, IdleSignal, InstrumentNote, InstrumentNoteDialect, NoteExpr, TransportInfo, MAX_NOTES_PER_BLOCK};
use crate::effect::native::{DelayEffect, DjFilterEffect, GainEffect, ReverbEffect};
use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::{StereoBuffer, StereoSample};

use super::clock::ClockSegment;
use super::command::SamplerParam;
use super::event::{flags, EventKind, LiveEvent};
use super::generator::layout;
use super::glue::{Role, SaturationParams, Saturator};
use super::player::EventSink;
use super::sampler::{Interpolation, LaneSamplerParams, SampleBank, Sampler};
use super::state::SpanTag;
use super::style::{SubVoiceKind, CONTROLS_PER_LANE};
use super::TICKS_PER_STEP;

// ─── Drum-lane target layout ───────────────────────────────────────────

// The numbering is the generators' convention, see `generator::layout`:
// backbone 0..8 (0 kick, 1 snare), hats 8..16, perc 16..24, break slices 32..96.
pub const DRUM_TARGET_BACKBONE: u8 = layout::BACKBONE_BASE;
pub const DRUM_TARGET_HATS: u8 = layout::HATS_BASE;
pub const DRUM_TARGET_PERC: u8 = layout::PERC_BASE;
pub const DRUM_TARGET_BREAK: u8 = layout::BREAK_BASE;

/// `(first target, target count)` of a drum sub-voice.
pub fn drum_target_range(kind: SubVoiceKind) -> (u8, u8) {
    match kind {
        SubVoiceKind::Backbone => (DRUM_TARGET_BACKBONE, layout::BLOCK_LEN),
        SubVoiceKind::Hats => (DRUM_TARGET_HATS, layout::BLOCK_LEN),
        SubVoiceKind::Perc => (DRUM_TARGET_PERC, layout::BLOCK_LEN),
        SubVoiceKind::Break => (DRUM_TARGET_BREAK, layout::MAX_BREAK_SLICES as u8),
    }
}

/// Highlight tag of a fired event (see [`SpanTag`]).
pub fn span_tag(role: Role, ev: &LiveEvent) -> SpanTag {
    if role == Role::Drums {
        match ev.target {
            t if t < DRUM_TARGET_HATS => SpanTag::Backbone,
            t if t < DRUM_TARGET_PERC => SpanTag::Hats,
            t if t < DRUM_TARGET_BREAK => SpanTag::Perc,
            _ => SpanTag::Break,
        }
    } else if ev.has(flags::TIE) {
        SpanTag::Tie
    } else if ev.has(flags::ACCENT) {
        SpanTag::Accent
    } else {
        SpanTag::Note
    }
}

// MUSICAL: fade lengths of lane on/off (D41) and void.
/// Fade of a bar-quantized toggle: short enough to read as a cut on the bar.
pub const QUANTIZED_FADE_SECS: f32 = 0.003;
/// Fade of an immediate toggle (Quant off).
pub const IMMEDIATE_FADE_SECS: f32 = 0.030;
/// Fade of the void move.
pub const VOID_FADE_SECS: f32 = 0.005;
/// How long a muted lane (or a swapped-out CLAP instrument) keeps being
/// rendered so its voices can finish releasing.
pub const TAIL_SECS: f32 = 0.5;
/// Gate of a CLAP note started by a gate-less `Trigger`, in ticks.
// MUSICAL: one 16th.
pub const CLAP_TRIGGER_GATE_TICKS: f64 = TICKS_PER_STEP as f64;
/// Velocity boost of an accented CLAP note.
// MUSICAL: matches the sampler's +3 dB accent roughly on velocity-linear patches.
pub const CLAP_ACCENT_GAIN: f32 = 1.25;
/// Notes a CLAP lane can hold at once.
pub const MAX_CLAP_NOTES: usize = 32;
/// Kick or snare onsets reported to the duck per chunk.
pub const MAX_KICK_TRIGGERS: usize = 64;
/// Main-kit targets that key the duck: the backbone's kick and snare.
pub const DRUM_TARGET_KICK: u8 = DRUM_TARGET_BACKBONE;
pub const DRUM_TARGET_SNARE: u8 = DRUM_TARGET_BACKBONE + 1;

// ─── Lane FX chain ─────────────────────────────────────────────────────

/// Native saturation effect (`id: drive`), wrapping the glue [`Saturator`].
pub struct DriveEffect {
    base: EffectBase,
    sat: Saturator,
}

impl DriveEffect {
    pub fn new() -> Self {
        let info = EffectInfo::new("Drive", "Distortion")
            .with_param(ParamInfo::new("Drive", 0.25).with_range(0.0, 24.0).with_unit("dB"))
            .with_param(ParamInfo::new("Asymmetry", 0.0).with_range(0.0, 0.5))
            .with_param(ParamInfo::new("Output", 0.5).with_range(-12.0, 12.0).with_unit("dB"));
        let mut fx = Self { base: EffectBase::new(info), sat: Saturator::new() };
        fx.configure();
        fx
    }

    fn configure(&mut self) {
        self.sat.configure(&SaturationParams {
            drive_db: self.base.param_actual(0),
            asymmetry: self.base.param_actual(1),
            mix: 1.0,
            output_db: self.base.param_actual(2),
        });
    }
}

impl Default for DriveEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for DriveEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if !self.base.is_bypassed() {
            self.sat.process(buffer.as_mut_slice());
        }
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn info(&self) -> &EffectInfo {
        self.base.info()
    }

    fn get_params(&self) -> &[ParamValue] {
        self.base.get_params()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.base.set_param(index, value);
        self.configure();
    }

    fn set_bypass(&mut self, bypass: bool) {
        self.base.set_bypass(bypass);
    }

    fn is_bypassed(&self) -> bool {
        self.base.is_bypassed()
    }

    fn reset(&mut self) {
        self.sat.reset();
    }
}

/// Native effect ids accepted by `FxSource::Native`.
pub const NATIVE_EFFECT_IDS: [&str; 5] = ["filter", "delay", "reverb", "gain", "drive"];

/// Build a native effect by id.
pub fn native_effect(id: &str) -> Option<Box<dyn Effect>> {
    Some(match id {
        "filter" => Box::new(DjFilterEffect::new()),
        "delay" => Box::new(DelayEffect::new()),
        "reverb" => Box::new(ReverbEffect::new()),
        "gain" => Box::new(GainEffect::new()),
        "drive" => Box::new(DriveEffect::new()),
        _ => return None,
    })
}

/// One slot of a lane FX chain. `effect` is `None` when the style names an
/// effect that could not be built; the slot stays so later slot indices in
/// control mappings keep their meaning.
pub struct FxSlot {
    pub effect: Option<Box<dyn Effect>>,
    pub dry_wet: f32,
}

/// Single-band lane FX chain (design §5.3).
pub struct LaneFx {
    slots: Vec<FxSlot>,
    dry: StereoBuffer,
}

impl LaneFx {
    pub fn new(slots: Vec<FxSlot>, max_block: usize) -> Self {
        Self { slots, dry: StereoBuffer::silence(max_block) }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Process `buf` in place. RT-safe for blocks up to `max_block`.
    pub fn process(&mut self, buf: &mut StereoBuffer) {
        let n = buf.len();
        for slot in &mut self.slots {
            let Some(fx) = slot.effect.as_mut() else { continue };
            if slot.dry_wet <= 0.0 || fx.is_bypassed() {
                continue;
            }
            if slot.dry_wet >= 1.0 {
                fx.process(buf);
                continue;
            }
            self.dry.set_len_from_capacity(n);
            self.dry.as_mut_slice().copy_from_slice(buf.as_slice());
            fx.process(buf);
            let (wet, dry) = (slot.dry_wet, 1.0 - slot.dry_wet);
            for (o, d) in buf.as_mut_slice().iter_mut().zip(self.dry.as_slice()) {
                o.left = o.left * wet + d.left * dry;
                o.right = o.right * wet + d.right * dry;
            }
        }
    }

    /// Set a normalized effect parameter. Unknown slots are ignored.
    pub fn set_param(&mut self, slot: usize, param: usize, value: f32) {
        if let Some(fx) = self.slots.get_mut(slot).and_then(|s| s.effect.as_mut()) {
            fx.set_param(param, value.clamp(0.0, 1.0));
        }
    }

    pub fn reset(&mut self) {
        for fx in self.slots.iter_mut().filter_map(|s| s.effect.as_mut()) {
            fx.reset();
        }
    }
}

// ─── CLAP instrument sharing ───────────────────────────────────────────

/// A CLAP instrument shared between the RT thread (process) and the
/// housekeeping thread (`idle`, which may restart the plugin).
///
/// Access is exclusive through a try-only flag. The RT side calls
/// [`ClapCell::try_lock`] and skips the block when it fails; it never waits.
/// The housekeeping side takes the flag only when [`IdleSignal::pending`]
/// says the plugin asked for something, so contention is limited to plugin
/// restarts.
pub struct ClapCell {
    busy: AtomicBool,
    inst: UnsafeCell<ClapInstrument>,
    signal: IdleSignal,
}

// Safety: `inst` is only reached through `ClapGuard`, which holds `busy`.
unsafe impl Sync for ClapCell {}
unsafe impl Send for ClapCell {}

impl ClapCell {
    pub fn new(inst: ClapInstrument) -> Arc<Self> {
        let signal = inst.idle_signal();
        Arc::new(Self { busy: AtomicBool::new(false), inst: UnsafeCell::new(inst), signal })
    }

    /// Exclusive access, or `None` when the other thread holds it.
    pub fn try_lock(&self) -> Option<ClapGuard<'_>> {
        self.busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| ClapGuard { cell: self })
    }

    /// Housekeeping: service plugin main-thread requests if any are pending.
    /// Non-RT. Returns `true` when the plugin was restarted.
    pub fn idle(&self) -> bool {
        if !self.signal.pending() {
            return false;
        }
        // The RT thread holds the flag for one process call at most.
        let mut guard = loop {
            match self.try_lock() {
                Some(g) => break g,
                None => std::thread::yield_now(),
            }
        };
        match guard.idle() {
            Ok(restarted) => restarted,
            Err(e) => {
                log::warn!("[LIVE] CLAP instrument '{}' idle failed: {e}", guard.plugin_id());
                false
            }
        }
    }
}

pub struct ClapGuard<'a> {
    cell: &'a ClapCell,
}

impl std::ops::Deref for ClapGuard<'_> {
    type Target = ClapInstrument;

    fn deref(&self) -> &ClapInstrument {
        // Safety: the guard holds `busy`.
        unsafe { &*self.cell.inst.get() }
    }
}

impl std::ops::DerefMut for ClapGuard<'_> {
    fn deref_mut(&mut self) -> &mut ClapInstrument {
        // Safety: the guard holds `busy`.
        unsafe { &mut *self.cell.inst.get() }
    }
}

impl Drop for ClapGuard<'_> {
    fn drop(&mut self) {
        self.cell.busy.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
struct ActiveNote {
    key: u8,
    /// Id sent to the plugin (unique per sounding note).
    clap_id: u32,
    /// `note_id` of the event, for `NoteOff` matching.
    event_id: u16,
    /// Ticks until the gate ends, measured from the current segment's start.
    ticks_left: f64,
    age: u64,
}

/// Turns lane events into CLAP note events with gate tracking (design §5.2).
///
/// CLAP has no gate, so every started note is tracked and its NOTE_OFF sent
/// when the gate runs out. Order at one sample offset:
/// - a gate ending where the next note starts: OFF before ON (retrigger),
/// - a `TIE` note: ON before the previous note's OFF, so a mono / legato
///   patch glides.
struct NoteTracker {
    notes: [ActiveNote; MAX_CLAP_NOTES],
    len: usize,
    next_id: u32,
    age: u64,
    /// Current segment.
    seg_offset: u32,
    seg_frames: u32,
    fpt: f64,
    out: Box<[InstrumentNote]>,
    out_len: usize,
    dropped: u32,
    /// Send note expressions (CLAP dialect only).
    expressions: bool,
}

impl NoteTracker {
    fn new() -> Self {
        let blank = ActiveNote { key: 0, clap_id: 0, event_id: 0, ticks_left: 0.0, age: 0 };
        Self {
            notes: [blank; MAX_CLAP_NOTES],
            len: 0,
            next_id: 1,
            age: 0,
            seg_offset: 0,
            seg_frames: 0,
            fpt: 1.0,
            out: vec![InstrumentNote::off(0, 0, 0); MAX_NOTES_PER_BLOCK].into_boxed_slice(),
            out_len: 0,
            dropped: 0,
            expressions: false,
        }
    }

    fn push(&mut self, note: InstrumentNote) {
        if self.out_len < self.out.len() {
            self.out[self.out_len] = note;
            self.out_len += 1;
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    fn remove(&mut self, index: usize) -> ActiveNote {
        let note = self.notes[index];
        self.len -= 1;
        self.notes[index] = self.notes[self.len];
        note
    }

    fn begin_segment(&mut self, seg: &ClockSegment) {
        self.seg_offset = seg.offset;
        self.seg_frames = seg.frames;
        self.fpt = seg.frames_per_tick();
    }

    /// Frame offset (within the block) at which a note's gate ends.
    fn end_offset(&self, note: &ActiveNote) -> u64 {
        if note.ticks_left.is_finite() {
            self.seg_offset as u64 + (note.ticks_left * self.fpt - 1e-6).max(0.0).ceil() as u64
        } else {
            u64::MAX
        }
    }

    /// Send NOTE_OFF for every gate ending at or before `limit`, except
    /// `skip`. The NOTE_OFF is placed no later than `cap`.
    fn expire(&mut self, limit: u64, cap: u64, skip_id: Option<u32>) {
        let mut i = 0;
        while i < self.len {
            let note = self.notes[i];
            let end = self.end_offset(&note);
            if end <= limit && Some(note.clap_id) != skip_id {
                self.remove(i);
                self.push(InstrumentNote::off(end.min(cap) as u32, note.key, note.clap_id));
            } else {
                i += 1;
            }
        }
    }

    fn end_segment(&mut self) {
        if self.seg_frames > 0 {
            // Gates ending on the segment's last frame or earlier; one ending
            // exactly on the next segment's first frame is ordered there.
            let last = self.seg_offset as u64 + self.seg_frames as u64 - 1;
            self.expire(last, last, None);
        }
        let advanced = self.seg_frames as f64 / self.fpt;
        for note in &mut self.notes[..self.len] {
            note.ticks_left -= advanced;
        }
        self.seg_frames = 0;
    }

    fn fire(&mut self, ev: &LiveEvent, offset: u32) {
        match ev.kind {
            EventKind::Trigger | EventKind::NoteOn => {
                if ev.vel == 0 {
                    return;
                }
                let tie = ev.kind == EventKind::NoteOn && ev.has(flags::TIE);
                let tied = if tie {
                    self.notes[..self.len].iter().max_by_key(|n| n.age).map(|n| n.clap_id)
                } else {
                    None
                };
                // A gate is measured from its note's first frame, so it can end
                // one frame after the next note's real start. That still abuts.
                self.expire(offset as u64 + 1, offset as u64, tied);
                if self.len == MAX_CLAP_NOTES {
                    // Steal the oldest note.
                    if let Some(i) = (0..self.len).min_by_key(|&i| self.notes[i].age) {
                        let old = self.remove(i);
                        self.push(InstrumentNote::off(offset, old.key, old.clap_id));
                    }
                }
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1).max(1);
                let accent = if ev.has(flags::ACCENT) { CLAP_ACCENT_GAIN } else { 1.0 };
                let velocity = (ev.vel.min(127) as f32 / 127.0 * accent).min(1.0);
                self.push(InstrumentNote::on(offset, ev.key, velocity, id));
                if self.expressions {
                    if ev.cents != 0 {
                        self.push(InstrumentNote::expression(offset, ev.key, id, NoteExpr::Tuning, ev.cents as f64 / 100.0));
                    }
                    if ev.pan != 0 {
                        self.push(InstrumentNote::expression(offset, ev.key, id, NoteExpr::Pan, 0.5 + ev.pan as f64 / 254.0));
                    }
                }
                if let Some(prev) = tied {
                    if let Some(i) = (0..self.len).find(|&i| self.notes[i].clap_id == prev) {
                        let old = self.remove(i);
                        self.push(InstrumentNote::off(offset, old.key, old.clap_id));
                    }
                }
                let gate = if ev.gate > 0 {
                    ev.gate as f64
                } else if ev.kind == EventKind::Trigger {
                    CLAP_TRIGGER_GATE_TICKS
                } else {
                    f64::INFINITY
                };
                self.age += 1;
                self.notes[self.len] = ActiveNote {
                    key: ev.key,
                    clap_id: id,
                    event_id: ev.note_id,
                    ticks_left: (offset - self.seg_offset) as f64 / self.fpt + gate,
                    age: self.age,
                };
                self.len += 1;
            }
            EventKind::NoteOff => {
                self.expire(offset as u64, offset as u64, None);
                let mut i = 0;
                while i < self.len {
                    if self.notes[i].event_id == ev.note_id {
                        let old = self.remove(i);
                        self.push(InstrumentNote::off(offset, old.key, old.clap_id));
                    } else {
                        i += 1;
                    }
                }
            }
            EventKind::Choke => {
                while self.len > 0 {
                    let old = self.remove(0);
                    self.push(InstrumentNote::choke(offset, old.key, Some(old.clap_id)));
                }
            }
        }
    }

    /// Release every held note at `offset`.
    fn all_notes_off(&mut self, offset: u32) {
        while self.len > 0 {
            let old = self.remove(0);
            self.push(InstrumentNote::off(offset, old.key, old.clap_id));
        }
    }
}

// ─── Lane ──────────────────────────────────────────────────────────────

/// One preloaded instrument option of a lane.
pub enum LaneOption {
    /// Sample bank and the option's LUFS trim as a linear gain. Drum kits
    /// carry their trims per slot, so their `gain` is 1.
    Sampler { bank: Shared<SampleBank>, gain: f32 },
    /// Index into the lane's CLAP instruments, plus the trim gain.
    Clap { slot: usize, gain: f32 },
    /// Could not be loaded; selecting it keeps the current instrument.
    Unavailable,
}

/// Construction parameters of a [`Lane`].
pub struct LaneInit {
    pub role: Role,
    pub sample_rate: u32,
    pub max_block: usize,
    pub max_voices: usize,
    pub interpolation: Interpolation,
    pub options: Vec<LaneOption>,
    pub claps: Vec<Arc<ClapCell>>,
    pub fx: LaneFx,
    pub default_option: usize,
}

/// One lane on the RT thread.
pub struct Lane {
    role: Role,
    sr: f32,
    sampler: Sampler,
    sampler_params: LaneSamplerParams,
    options: Vec<LaneOption>,
    claps: Vec<Arc<ClapCell>>,
    option: usize,
    pending_option: Option<usize>,
    active_clap: Option<usize>,
    clap_gain: f32,
    /// Swapped-out CLAP instrument still rendering its release, frames left.
    releasing_clap: Option<(usize, u32)>,
    /// Instrument controls need re-sending to the CLAP instrument.
    clap_params_dirty: bool,
    tracker: NoteTracker,
    fx: LaneFx,
    scratch: StereoBuffer,
    /// Events fire while on.
    on: bool,
    pending_on: Option<bool>,
    void: bool,
    gain: f32,
    fade_step: f32,
    /// Frames a silent lane keeps rendering.
    tail_left: u32,
    /// `(param, value)` last set by each performance control.
    inst_controls: [Option<(u32, f32)>; CONTROLS_PER_LANE],
    kick_triggers: [u32; MAX_KICK_TRIGGERS],
    kick_len: usize,
    snare_triggers: [u32; MAX_KICK_TRIGGERS],
    snare_len: usize,
    /// Tag of the last fired event this chunk, `None` when nothing fired.
    last_fired: Option<(SpanTag, bool)>,
    fired_count: u32,
}

impl Lane {
    /// Build a lane. Off-RT.
    pub fn new(init: LaneInit) -> Self {
        let mut sampler = Sampler::new(init.sample_rate, init.max_voices);
        let sampler_params = LaneSamplerParams { interpolation: init.interpolation, ..Default::default() };
        sampler.set_params(sampler_params);
        let mut lane = Self {
            role: init.role,
            sr: init.sample_rate as f32,
            sampler,
            sampler_params,
            options: init.options,
            claps: init.claps,
            option: init.default_option,
            pending_option: None,
            active_clap: None,
            clap_gain: 1.0,
            releasing_clap: None,
            clap_params_dirty: false,
            tracker: NoteTracker::new(),
            fx: init.fx,
            scratch: StereoBuffer::silence(init.max_block),
            on: true,
            pending_on: None,
            void: false,
            gain: 1.0,
            fade_step: 1.0,
            tail_left: 0,
            inst_controls: [None; CONTROLS_PER_LANE],
            kick_triggers: [0; MAX_KICK_TRIGGERS],
            kick_len: 0,
            snare_triggers: [0; MAX_KICK_TRIGGERS],
            snare_len: 0,
            last_fired: None,
            fired_count: 0,
        };
        lane.apply_option(init.default_option, 0);
        lane.tracker.out_len = 0;
        lane
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// State the lane will be in after a queued toggle.
    pub fn on_target(&self) -> bool {
        self.pending_on.unwrap_or(self.on)
    }

    pub fn option(&self) -> usize {
        self.option
    }

    pub fn option_count(&self) -> usize {
        self.options.len()
    }

    pub fn fx_mut(&mut self) -> &mut LaneFx {
        &mut self.fx
    }

    /// Sounding voices (sampler voices plus held CLAP notes).
    pub fn voices(&self) -> usize {
        self.sampler.voices_active() + self.tracker.len
    }

    /// Main-kit kick onsets of the last chunk, ascending frame offsets.
    pub fn kick_triggers(&self) -> &[u32] {
        &self.kick_triggers[..self.kick_len]
    }

    /// Main-kit snare onsets of the last chunk, ascending frame offsets.
    pub fn snare_triggers(&self) -> &[u32] {
        &self.snare_triggers[..self.snare_len]
    }

    /// Tag of the last event fired in the last chunk and how many fired.
    pub fn take_fired(&mut self) -> (Option<(SpanTag, bool)>, u32) {
        (self.last_fired.take(), std::mem::take(&mut self.fired_count))
    }

    fn set_fade(&mut self, secs: f32) {
        self.fade_step = 1.0 / (secs * self.sr).max(1.0);
    }

    /// Switch the lane on or off. `quantized` waits for [`Self::begin_bar`].
    pub fn set_on(&mut self, on: bool, quantized: bool) {
        if quantized {
            self.pending_on = (on != self.on).then_some(on);
        } else {
            self.pending_on = None;
            self.switch(on, IMMEDIATE_FADE_SECS, 0);
        }
    }

    fn switch(&mut self, on: bool, fade_secs: f32, offset: u32) {
        if on == self.on {
            return;
        }
        self.on = on;
        self.set_fade(fade_secs);
        if !on {
            self.silence_notes(offset);
        }
    }

    /// Release held notes: CLAP notes get NOTE_OFF, sampler voices are choked.
    fn silence_notes(&mut self, offset: u32) {
        self.tracker.all_notes_off(offset);
        let choke = LiveEvent { kind: EventKind::Choke, choke: 0, ..Default::default() };
        self.sampler.handle_event(&choke, offset);
    }

    /// Hold or release the void move.
    pub fn set_void(&mut self, void: bool) {
        if void != self.void {
            self.void = void;
            self.set_fade(VOID_FADE_SECS);
            if void {
                self.silence_notes(0);
            }
        }
    }

    /// Queue an option swap for the next bar.
    pub fn queue_option(&mut self, option: usize) {
        if option < self.options.len() {
            self.pending_option = Some(option);
        }
    }

    /// Bar boundary at frame `offset` of the chunk: apply queued changes.
    pub fn begin_bar(&mut self, offset: u32) {
        if let Some(option) = self.pending_option.take() {
            self.apply_option(option, offset);
        }
        if let Some(on) = self.pending_on.take() {
            self.switch(on, QUANTIZED_FADE_SECS, offset);
        }
    }

    fn apply_option(&mut self, option: usize, offset: u32) {
        let (new_clap, gain) = match self.options.get(option) {
            Some(LaneOption::Sampler { bank, gain }) => {
                // Playing voices keep their own sample handles and ring out.
                self.sampler.set_bank(bank.clone());
                self.sampler_params.gain = *gain;
                self.sampler.set_params(self.sampler_params);
                (None, 1.0)
            }
            Some(LaneOption::Clap { slot, gain }) if *slot < self.claps.len() => (Some(*slot), *gain),
            _ => return,
        };
        self.option = option;
        if new_clap != self.active_clap {
            self.tracker.all_notes_off(offset);
            if let Some(old) = self.active_clap {
                self.releasing_clap = Some((old, (TAIL_SECS * self.sr) as u32));
            }
            if new_clap.is_some() && self.releasing_clap.map(|(i, _)| Some(i)) == Some(new_clap) {
                self.releasing_clap = None;
            }
            self.active_clap = new_clap;
        }
        self.clap_gain = gain;
        self.clap_params_dirty = new_clap.is_some();
        // Sampler controls follow the lane, not the option.
        for c in self.inst_controls.into_iter().flatten() {
            self.apply_sampler_control(c.0, c.1);
        }
    }

    fn apply_sampler_control(&mut self, param: u32, value: f32) {
        if self.active_clap.is_none() {
            if let Some(p) = SamplerParam::from_index(param) {
                p.apply(&mut self.sampler_params, value);
                self.sampler.set_params(self.sampler_params);
            }
        }
    }

    /// Set the instrument parameter driven by performance control `control`.
    pub fn set_instrument_param(&mut self, control: usize, param: u32, value: f32) {
        if let Some(slot) = self.inst_controls.get_mut(control) {
            *slot = Some((param, value));
        }
        if self.active_clap.is_some() {
            self.clap_params_dirty = true;
        } else {
            self.apply_sampler_control(param, value);
        }
    }

    /// Start a chunk: clear the per-chunk note and trigger lists.
    pub fn begin_chunk(&mut self) {
        // Notes queued before the chunk (all-notes-off from a command) stay.
        self.kick_len = 0;
        self.snare_len = 0;
    }

    pub fn begin_segment(&mut self, seg: &ClockSegment) {
        self.tracker.begin_segment(seg);
    }

    pub fn end_segment(&mut self) {
        self.tracker.end_segment();
    }

    /// Render the chunk into `out` (its length is the chunk length).
    pub fn render(&mut self, out: &mut StereoBuffer, samples_per_tick: f64, transport: &TransportInfo) {
        let n = out.len();
        let target = if self.on && !self.void { 1.0 } else { 0.0 };

        if target == 0.0 && self.gain == 0.0 {
            if self.tail_left == 0 {
                // Fully off: nothing to render. Queued note-offs are dropped
                // with the frozen instrument state.
                self.tracker.out_len = 0;
                out.fill_silence();
                self.sort_kicks();
                return;
            }
            self.tail_left = self.tail_left.saturating_sub(n as u32);
        } else {
            self.tail_left = (TAIL_SECS * self.sr) as u32;
        }

        self.sampler.process(out.as_mut_slice(), samples_per_tick);
        self.render_claps(out, transport);
        self.fx.process(out);

        // On/off gain ramp.
        if self.gain != target || target != 1.0 {
            let step = self.fade_step;
            let mut g = self.gain;
            for s in out.as_mut_slice() {
                g = if g < target { (g + step).min(target) } else { (g - step).max(target) };
                *s *= g;
            }
            self.gain = g;
        }
        self.sort_kicks();
    }

    fn render_claps(&mut self, out: &mut StereoBuffer, transport: &TransportInfo) {
        let n = out.len();
        let notes_len = std::mem::take(&mut self.tracker.out_len);
        if let Some(slot) = self.active_clap {
            if let Some(mut inst) = self.claps[slot].try_lock() {
                self.tracker.expressions = inst.dialect() == Some(InstrumentNoteDialect::Clap);
                if self.clap_params_dirty {
                    for (param, value) in self.inst_controls.into_iter().flatten() {
                        inst.set_param_normalized(param as usize, value as f64);
                    }
                    self.clap_params_dirty = false;
                }
                self.scratch.set_len_from_capacity(n);
                if inst.process(&mut self.scratch, &self.tracker.out[..notes_len], transport).is_ok() {
                    mix_in(out, &self.scratch, self.clap_gain);
                }
            }
        }
        if let Some((slot, left)) = self.releasing_clap {
            if let Some(mut inst) = self.claps[slot].try_lock() {
                self.scratch.set_len_from_capacity(n);
                if inst.process(&mut self.scratch, &[], transport).is_ok() {
                    mix_in(out, &self.scratch, self.clap_gain);
                }
            }
            self.releasing_clap = (left as usize > n).then(|| (slot, left - n as u32));
        }
    }

    fn sort_kicks(&mut self) {
        sort_offsets(&mut self.kick_triggers[..self.kick_len]);
        sort_offsets(&mut self.snare_triggers[..self.snare_len]);
    }
}

/// Insertion sort (short, mostly sorted, allocation-free).
fn sort_offsets(t: &mut [u32]) {
    for i in 1..t.len() {
        let mut j = i;
        while j > 0 && t[j - 1] > t[j] {
            t.swap(j - 1, j);
            j -= 1;
        }
    }
}

fn mix_in(out: &mut StereoBuffer, src: &StereoBuffer, gain: f32) {
    for (o, s) in out.as_mut_slice().iter_mut().zip(src.as_slice()) {
        *o += StereoSample::new(s.left * gain, s.right * gain);
    }
}

impl EventSink for Lane {
    fn fire(&mut self, ev: &LiveEvent, offset: u32, fill: bool) {
        if !self.on || self.void {
            return;
        }
        let onset = matches!(ev.kind, EventKind::Trigger | EventKind::NoteOn);
        if onset {
            self.last_fired = Some((span_tag(self.role, ev), fill));
            self.fired_count += 1;
            // Only the main kit keys the duck, never break slices.
            if self.role == Role::Drums && ev.vel > 0 && ev.kind == EventKind::Trigger {
                if ev.target == DRUM_TARGET_KICK && self.kick_len < MAX_KICK_TRIGGERS {
                    self.kick_triggers[self.kick_len] = offset;
                    self.kick_len += 1;
                } else if ev.target == DRUM_TARGET_SNARE && self.snare_len < MAX_KICK_TRIGGERS {
                    self.snare_triggers[self.snare_len] = offset;
                    self.snare_len += 1;
                }
            }
        }
        if self.active_clap.is_some() {
            self.tracker.fire(ev, offset);
        } else {
            self.sampler.handle_event(ev, offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clap::InstrumentNoteKind;
    use crate::live::clock::LiveClock;

    fn note_on(tick: u16, key: u8, gate: u16, id: u16) -> LiveEvent {
        LiveEvent { tick, kind: EventKind::NoteOn, key, vel: 100, gate, note_id: id, ..Default::default() }
    }

    /// Run `events` through a tracker over one bar and return
    /// `(absolute frame, is_on, key)` in the order the plugin would see them.
    fn track(events: &[LiveEvent], block: u32) -> Vec<(u64, bool, u8)> {
        let mut clock = LiveClock::new(48_000, 170.0);
        let mut tracker = NoteTracker::new();
        let mut out = Vec::new();
        let mut base = 0u64;
        while clock.position().bar < 1 {
            for seg in clock.segments(block) {
                tracker.begin_segment(&seg);
                if seg.bar == 0 {
                    for ev in events {
                        if let Some(offset) = seg.offset_for_tick(ev.tick) {
                            tracker.fire(ev, offset);
                        }
                    }
                }
                tracker.end_segment();
            }
            let mut notes = tracker.out[..tracker.out_len].to_vec();
            tracker.out_len = 0;
            // Same stable sort the instrument host applies.
            notes.sort_by_key(|n| n.offset);
            for n in notes {
                match n.kind {
                    InstrumentNoteKind::On => out.push((base + n.offset as u64, true, n.key)),
                    InstrumentNoteKind::Off => out.push((base + n.offset as u64, false, n.key)),
                    _ => {}
                }
            }
            base += block as u64;
        }
        out
    }

    #[test]
    fn gate_end_sends_note_off_at_the_right_frame() {
        let fpt = 48_000.0 * 60.0 / 170.0 * 4.0 / 384.0;
        for block in [64, 256, 1024] {
            let got = track(&[note_on(24, 60, 48, 1)], block);
            assert_eq!(got.len(), 2, "{got:?}");
            assert_eq!((got[0].1, got[0].2), (true, 60));
            assert_eq!((got[1].1, got[1].2), (false, 60));
            let len = (got[1].0 - got[0].0) as f64;
            assert!((len - 48.0 * fpt).abs() <= 1.5, "block {block}: gate {len} frames");
        }
    }

    #[test]
    fn abutting_notes_retrigger_but_ties_overlap() {
        // Second note starts exactly where the first gate ends.
        let plain = track(&[note_on(0, 60, 24, 1), note_on(24, 62, 24, 2)], 256);
        let kinds: Vec<(bool, u8)> = plain.iter().map(|n| (n.1, n.2)).collect();
        assert_eq!(kinds, vec![(true, 60), (false, 60), (true, 62), (false, 62)]);
        assert_eq!(plain[1].0, plain[2].0, "off and on share the frame");

        let mut tied = note_on(24, 62, 24, 2);
        tied.flags = flags::TIE;
        let legato = track(&[note_on(0, 60, 24, 1), tied], 256);
        let kinds: Vec<(bool, u8)> = legato.iter().map(|n| (n.1, n.2)).collect();
        assert_eq!(kinds, vec![(true, 60), (true, 62), (false, 60), (false, 62)], "tie: on before off");
        assert_eq!(legato[1].0, legato[2].0);
    }

    #[test]
    fn note_off_event_and_choke_release_held_notes() {
        let off = LiveEvent { tick: 96, kind: EventKind::NoteOff, note_id: 5, ..Default::default() };
        let got = track(&[note_on(0, 50, 0, 5), off], 128);
        assert_eq!(got.iter().map(|n| n.1).collect::<Vec<_>>(), vec![true, false]);

        let mut tracker = NoteTracker::new();
        let mut clock = LiveClock::new(48_000, 170.0);
        let seg = clock.segments(64).next().unwrap();
        tracker.begin_segment(&seg);
        tracker.fire(&note_on(0, 40, 0, 1), 0);
        tracker.fire(&note_on(0, 47, 0, 2), 0);
        assert_eq!(tracker.len, 2);
        tracker.all_notes_off(10);
        assert_eq!(tracker.len, 0);
        assert_eq!(tracker.out_len, 4);
    }

    #[test]
    fn drive_effect_distorts_and_span_tags_follow_targets() {
        let mut fx = DriveEffect::new();
        fx.set_param(0, 1.0);
        let mut buf = StereoBuffer::from_vec(vec![StereoSample::mono(0.9); 64]);
        fx.process(&mut buf);
        assert!(buf.iter().all(|s| s.left.is_finite() && s.left != 0.9));

        let hit = |target| LiveEvent { target, ..Default::default() };
        assert_eq!(span_tag(Role::Drums, &hit(1)), SpanTag::Backbone);
        assert_eq!(span_tag(Role::Drums, &hit(40)), SpanTag::Break);
        assert_eq!(span_tag(Role::Drums, &hit(9)), SpanTag::Hats);
        assert_eq!(span_tag(Role::Drums, &hit(17)), SpanTag::Perc);
        let mut tie = hit(0);
        tie.flags = flags::TIE;
        assert_eq!(span_tag(Role::Bass, &tie), SpanTag::Tie);
        for id in NATIVE_EFFECT_IDS {
            assert!(native_effect(id).is_some(), "{id}");
        }
        assert!(native_effect("nope").is_none());
    }
}
