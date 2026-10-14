//! Live engine: the RT side of live mode and its control handle (design §4).
//!
//! ```text
//!  UI / F1 ─► LiveController ──RenderMsg (mpsc)──► BarRenderer ──BarPacket (rtrb)──┐
//!              (non-RT state)                                                       ▼
//!                   └──────────RtMsg (rtrb)──────────────────────────────► LiveEngine (RT)
//!                                                    clock → step player → lanes → glue → master
//!  UI ◄── LiveShared (atomics, scope rings) ◄────────────────────────────────────────┘
//! ```
//!
//! [`LiveEngine`] implements [`RtEngine`], so the JACK / CPAL backends drive it
//! the same way they drive the DJ engine. Per block it drains [`RtMsg`]s,
//! advances the [`LiveClock`] and processes the block in chunks that end on
//! bar boundaries, so everything bar-quantized (bar swap, lane toggle, kit
//! swap) lands on the bar's first frame.
//!
//! # Lane processing order
//!
//! Lanes are processed one after the other on the audio thread. The DJ
//! engine fans stems out with `rayon` (`deck.rs`), but a `par_iter` entered
//! from a thread outside the pool parks the caller on a mutex and condvar
//! until the workers finish, which breaks this module's "no locks, no
//! blocking" rule, and CLAP instruments would change threads every block.
//! Each [`Lane`] is self-contained, so lanes can move to workers once the
//! board benchmark shows it is needed.
//!
//! # RT safety
//!
//! `process_block` does not allocate, lock or block. Bar packets and stopped
//! recordings go back to the renderer thread over rings; sample data is
//! `basedrop::Shared`.

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use crate::audio::RtEngine;
use crate::clap::TransportInfo;
use crate::music::MusicalKey;
use crate::types::{StereoBuffer, StereoSample};

use super::capture::{Capture, CaptureScope, GlobalCapture, LaneCapture};
use super::clock::{Boundary, ClockSegment, LiveClock};
use super::command::{LiveCommand, MoveKind, RtMsg, RT_MSG_CAPACITY};
use super::glue::{GlueChain, GlueKey, GlueParams, LaneSetup, Role};
use super::harmony::key_intent;
use super::lane::Lane;
use super::loader::{load_style, ControlRoute, LoadError, LoadOptions, LoadedStyle, ResolvedControl};
use super::player::StepPlayer;
use super::render::{BarRenderer, RenderLane, RenderMsg, RendererHandle};
use super::scope::ScopeWriter;
use super::state::LiveShared;
use super::style::{SendBus, Style, TempoSpec, CONTROLS_PER_LANE};
use super::{snap_loop_len, DEFAULT_LOOP_LEN, NUM_LANES, SLOTS_PER_CHAIN, TICKS_PER_BAR};

/// Clock segments handled per pass. A pass is at most `max_block` frames and
/// a step is at least 2205 frames (300 BPM at 44.1 kHz), so this is ample.
const MAX_SEGMENTS: usize = 32;
/// Set recordings that can run at once.
const MAX_RECORDINGS: usize = 4;
/// Bars a tempo change ramps over unless the command says otherwise.
// MUSICAL: long enough to hear the move, short enough to perform with.
pub const DEFAULT_TEMPO_RAMP_BARS: u32 = 2;
/// Seed of every slot of lane `l` at load is `DEFAULT_SEED + l`.
// MUSICAL: equal seeds make a chain of one pattern; the generator's
// phrase position still varies bar 4.
pub const DEFAULT_SEED: u32 = 50;

const BLANK_SEGMENT: ClockSegment = ClockSegment {
    offset: 0,
    frames: 0,
    bar: 0,
    step: 0,
    boundary: Boundary::None,
    bpm: 120.0,
    step_frames: 0.0,
    step_phase: 0.0,
    frame_in_step: 0,
    ends_step: false,
};

/// Accumulated processing time per stage, in nanoseconds (benchmark only).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StageTimes {
    /// Event firing plus instrument and FX rendering, per lane.
    pub lanes: [u64; NUM_LANES],
    pub glue: u64,
    /// Whole `process_block` calls.
    pub total: u64,
    pub blocks: u64,
    /// Longest single `process_block` call.
    pub worst_block: u64,
}

/// The live-mode audio engine. Lives on the audio thread.
pub struct LiveEngine {
    clock: LiveClock,
    player: StepPlayer,
    lanes: Vec<Lane>,
    glue: GlueChain,
    glue_params: GlueParams,
    glue_dirty: bool,
    setups: [LaneSetup; NUM_LANES],
    lane_bufs: [StereoBuffer; NUM_LANES],
    mix: StereoBuffer,
    msgs: rtrb::Consumer<RtMsg>,
    trash: rtrb::Producer<rtrb::Producer<StereoSample>>,
    recording: Vec<rtrb::Producer<StereoSample>>,
    shared: Arc<LiveShared>,
    scopes: [ScopeWriter; NUM_LANES],
    segs: [ClockSegment; MAX_SEGMENTS],
    max_block: usize,
    output_latency: Arc<AtomicU64>,
    profiling: bool,
    times: StageTimes,
    /// The audio thread pins itself to `rt::live_cores::AUDIO` on its first block.
    pinned: bool,
}

impl LiveEngine {
    /// Load `style` and start the bar renderer thread. Returns the engine
    /// for the audio backend and the handles for the UI. Non-RT.
    ///
    /// ```ignore
    /// let system = start_audio_system_with(&config, |rate| {
    ///     LiveEngine::start(&style, LoadOptions::new(rate)).expect("style loads")
    /// })?;
    /// ```
    pub fn start(style: &Style, opts: LoadOptions) -> Result<(LiveEngine, LiveHandles), LoadError> {
        let system = build(load_style(style, opts)?);
        let handles = LiveHandles {
            controller: system.controller,
            shared: system.shared,
            renderer: system.renderer.spawn(),
            warnings: system.warnings,
        };
        Ok((system.engine, handles))
    }

    pub fn sample_rate(&self) -> u32 {
        self.clock.sample_rate()
    }

    pub fn shared(&self) -> &Arc<LiveShared> {
        &self.shared
    }

    /// Latency of the master chain in samples (limiter lookahead).
    pub fn latency_samples(&self) -> usize {
        self.glue.latency_samples()
    }

    /// Collect per-stage timings (benchmark). Reads the clock twice per lane
    /// and block, so leave it off in performance.
    pub fn set_profiling(&mut self, on: bool) {
        self.profiling = on;
    }

    /// Timings since the last call.
    pub fn take_stage_times(&mut self) -> StageTimes {
        std::mem::take(&mut self.times)
    }

    fn drain_msgs(&mut self) {
        while let Ok(msg) = self.msgs.pop() {
            match msg {
                RtMsg::LaneOn { lane, on, quantized } => {
                    if let Some(l) = self.lanes.get_mut(lane as usize) {
                        l.set_on(on, quantized);
                    }
                }
                RtMsg::Option { lane, option } => {
                    if let Some(l) = self.lanes.get_mut(lane as usize) {
                        l.queue_option(option as usize);
                    }
                }
                RtMsg::InstrumentParam { lane, control, param, value } => {
                    if let Some(l) = self.lanes.get_mut(lane as usize) {
                        l.set_instrument_param(control as usize, param, value);
                    }
                }
                RtMsg::FxParam { lane, slot, param, value } => {
                    if let Some(l) = self.lanes.get_mut(lane as usize) {
                        l.fx_mut().set_param(slot as usize, param as usize, value);
                    }
                }
                RtMsg::Glue { lane, control, value } => {
                    let role = self.lanes.get(lane as usize).map(|l| l.role()).unwrap_or(Role::Extra);
                    control.apply(&mut self.glue_params, role, value);
                    self.glue_dirty = true;
                }
                RtMsg::Send { lane, bus, level } => {
                    if let Some(setup) = self.setups.get_mut(lane as usize) {
                        match bus {
                            SendBus::Delay => setup.delay_send = level.clamp(0.0, 1.0),
                            SendBus::Reverb => setup.reverb_send = level.clamp(0.0, 1.0),
                        }
                        self.glue.set_lane(lane as usize, *setup);
                    }
                }
                RtMsg::MovePress { lane, kind } => {
                    let lane = lane as usize;
                    if lane < self.lanes.len() {
                        let pos = self.clock.position();
                        self.player.press(lane, kind, pos.bar, pos.tick);
                        if kind == MoveKind::Void {
                            self.lanes[lane].set_void(true);
                        }
                    }
                }
                RtMsg::MoveRelease { lane, kind } => {
                    let lane = lane as usize;
                    if lane < self.lanes.len() {
                        self.player.release(lane, kind);
                        if kind == MoveKind::Void {
                            self.lanes[lane].set_void(false);
                        }
                    }
                }
                RtMsg::RollDivision { ticks } => self.player.set_roll_ticks(ticks),
                RtMsg::Tempo { bpm, bars } => self.clock.request_tempo(bpm, bars),
                RtMsg::StartRecording(producer) => {
                    if self.recording.len() < MAX_RECORDINGS {
                        self.recording.push(producer);
                    } else {
                        let _ = self.trash.push(producer);
                    }
                }
                RtMsg::StopRecording => {
                    // Dropping a producer can free its ring: hand it to the renderer thread.
                    while let Some(producer) = self.recording.pop() {
                        let _ = self.trash.push(producer);
                    }
                }
            }
        }
        if std::mem::take(&mut self.glue_dirty) {
            self.glue.set_params(&self.glue_params);
        }
    }

    /// Render one block. `lane_tap`, when given, receives each lane's
    /// post-glue signal for the block (stems for offline renders and tests);
    /// its buffers must be at least as long as `master`.
    pub fn process(&mut self, master: &mut StereoBuffer, mut lane_tap: Option<&mut [StereoBuffer; NUM_LANES]>) {
        crate::rt::flush_denormals();
        if !self.pinned {
            // One syscall on the first block: the backend creates the audio
            // thread, so this is the earliest point we run on it.
            crate::rt::pin_to_cores(crate::rt::live_cores::AUDIO);
            self.pinned = true;
        }
        let started = self.profiling.then(Instant::now);
        self.drain_msgs();
        self.player.poll();

        let n = master.len();
        let mut done = 0;
        while done < n {
            let want = (n - done).min(self.max_block) as u32;
            let mut count = 0;
            for seg in self.clock.segments(want) {
                self.segs[count] = seg;
                count += 1;
                if count == MAX_SEGMENTS {
                    break;
                }
            }
            if count == 0 {
                break;
            }
            // Chunks end where the next segment starts a bar.
            let mut i = 0;
            while i < count {
                let mut j = i + 1;
                while j < count && self.segs[j].boundary < Boundary::Bar {
                    j += 1;
                }
                let start = done + self.segs[i].offset as usize;
                let end = done + (self.segs[j - 1].offset + self.segs[j - 1].frames) as usize;
                self.process_chunk(i, j, &mut master.as_mut_slice()[start..end]);
                if let Some(tap) = lane_tap.as_deref_mut() {
                    for (t, lane) in tap.iter_mut().zip(&self.lane_bufs) {
                        t.as_mut_slice()[start..end].copy_from_slice(lane.as_slice());
                    }
                }
                i = j;
            }
            let last = self.segs[count - 1];
            done += (last.offset + last.frames) as usize;
        }

        let pos = self.clock.position();
        self.shared.set_clock(pos.bar, pos.step, self.clock.bpm(), self.clock.target_bpm(), self.clock.is_ramping());
        for (li, lane) in self.lanes.iter_mut().enumerate() {
            let shared = self.shared.lane(li);
            shared.set_on(lane.is_on(), lane.on_target());
            shared.set_option(lane.option() as u16);
            shared.set_moves(self.player.held(li));
        }
        if let Some(t) = started {
            let ns = t.elapsed().as_nanos() as u64;
            self.times.total += ns;
            self.times.blocks += 1;
            self.times.worst_block = self.times.worst_block.max(ns);
        }
    }

    /// Process segments `first..end` (one chunk, inside one bar) into `out`.
    fn process_chunk(&mut self, first: usize, end: usize, out: &mut [StereoSample]) {
        let head = self.segs[first];
        let base = head.offset;
        let frames = out.len();

        if head.boundary >= Boundary::Bar {
            let epoch = match self.player.begin_bar(head.bar) {
                Some(packet) => {
                    self.shared.set_harmony(packet.key, packet.chord_degree, packet.progression, packet.half_time);
                    self.clock.set_half_time(packet.half_time);
                    packet.epoch
                }
                None => 0,
            };
            self.shared.set_played(head.bar, epoch);
            for lane in &mut self.lanes {
                lane.begin_bar(0);
            }
        }

        let samples_per_tick = head.frames_per_tick();
        let beats = head.bar as f64 * 4.0 + head.tick_pos() * 4.0 / TICKS_PER_BAR as f64;
        let transport = TransportInfo::new(head.bpm, true, beats);
        for li in 0..self.lanes.len() {
            let t0 = self.profiling.then(Instant::now);
            let lane = &mut self.lanes[li];
            lane.begin_chunk();
            let mut step = head.step;
            for seg in &self.segs[first..end] {
                let mut seg = *seg;
                seg.offset -= base;
                lane.begin_segment(&seg);
                step = self.player.fire_lane(li, &seg, lane);
                lane.end_segment();
            }
            let buf = &mut self.lane_bufs[li];
            buf.set_len_from_capacity(frames);
            lane.render(buf, samples_per_tick, &transport);

            let shared = self.shared.lane(li);
            shared.set_active_step(step);
            if let (Some((tag, fill)), _) = lane.take_fired() {
                shared.note_fired(tag, fill);
            }
            if let Some(t) = t0 {
                self.times.lanes[li] += t.elapsed().as_nanos() as u64;
            }
        }

        let t0 = self.profiling.then(Instant::now);
        self.mix.set_len_from_capacity(frames);
        // The drum lane's kick onsets key the duck.
        let key = GlueKey { sidechain: None, triggers: self.lanes[0].kick_triggers() };
        self.glue.process(&mut self.lane_bufs, key, head.bpm, &mut self.mix);
        out.copy_from_slice(self.mix.as_slice());
        if let Some(t) = t0 {
            self.times.glue += t.elapsed().as_nanos() as u64;
        }

        for (li, buf) in self.lane_bufs.iter().enumerate() {
            let peak = buf.as_slice().iter().fold(0.0f32, |p, s| p.max(s.left.abs()).max(s.right.abs()));
            let shared = self.shared.lane(li);
            shared.set_meters(self.lanes[li].voices() as u32, peak);
            self.scopes[li].write(shared.scope(), buf.as_slice());
        }
    }
}

impl RtEngine for LiveEngine {
    fn process_block(&mut self, master: &mut StereoBuffer, cue: &mut StereoBuffer) {
        self.process(master, None);
        // Cue output is a copy of the master for now.
        let n = master.len().min(cue.len());
        cue.as_mut_slice()[..n].copy_from_slice(&master.as_slice()[..n]);
    }

    fn recording_producers(&mut self) -> &mut Vec<rtrb::Producer<StereoSample>> {
        &mut self.recording
    }

    fn output_latency_samples(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.output_latency)
    }
}

// ─── Control side ──────────────────────────────────────────────────────

/// Non-RT state of one lane, owned by the [`LiveController`].
#[derive(Debug, Clone)]
pub struct LaneControl {
    pub name: String,
    pub role: Role,
    pub controls: [ResolvedControl; CONTROLS_PER_LANE],
    /// Control positions, `0..=1`.
    pub positions: [f32; CONTROLS_PER_LANE],
    pub seeds: [u32; SLOTS_PER_CHAIN],
    /// Bars the lane loops (1, 2, 4 or 8).
    pub loop_len: u8,
    pub selected_slot: usize,
    pub option_names: Vec<String>,
    pub option_available: Vec<bool>,
    /// Option loaded or queued for the next bar.
    pub option: usize,
    /// Option under the encoder while the lane button is held.
    pub browse: Option<usize>,
    /// Requested on/off state.
    pub on: bool,
}

/// Control handle of the live engine. Owns the non-RT state and routes
/// [`LiveCommand`]s to the bar renderer and the audio thread. Single owner
/// (the UI thread, which also drains the controller surface).
pub struct LiveController {
    shared: Arc<LiveShared>,
    rt: rtrb::Producer<RtMsg>,
    render: mpsc::Sender<RenderMsg>,
    style: String,
    tempo: TempoSpec,
    lanes: Vec<LaneControl>,
    progressions: usize,
    quantize: bool,
    half_time: bool,
    intent: Option<MusicalKey>,
    captures: Vec<Capture>,
    capture_collection: Option<PathBuf>,
    /// Results of the off-thread capture saves, for the UI's confirmation.
    saved_tx: mpsc::Sender<Result<PathBuf, String>>,
    saved_rx: mpsc::Receiver<Result<PathBuf, String>>,
    dropped: u32,
}

impl LiveController {
    /// State the UI reads every tick.
    pub fn shared(&self) -> &Arc<LiveShared> {
        &self.shared
    }

    pub fn style_name(&self) -> &str {
        &self.style
    }

    pub fn tempo_range(&self) -> TempoSpec {
        self.tempo
    }

    pub fn lane(&self, lane: usize) -> &LaneControl {
        &self.lanes[lane]
    }

    pub fn progression_count(&self) -> usize {
        self.progressions
    }

    /// Captures taken or recalled this session, oldest first.
    pub fn captures(&self) -> &[Capture] {
        &self.captures
    }

    /// Collection root captures are saved under (`<root>/captures/<style>/`).
    /// Without it captures stay in memory.
    pub fn set_capture_collection(&mut self, collection: Option<PathBuf>) {
        self.capture_collection = collection;
    }

    /// Next finished capture save: the file written, or why it failed.
    pub fn take_saved_capture(&mut self) -> Option<Result<PathBuf, String>> {
        self.saved_rx.try_recv().ok()
    }

    /// Messages lost because the RT ring was full.
    pub fn dropped_messages(&self) -> u32 {
        self.dropped
    }

    fn rt(&mut self, msg: RtMsg) {
        if self.rt.push(msg).is_err() {
            self.dropped += 1;
            log::warn!("[LIVE] RT message ring full, message dropped");
        }
    }

    fn render(&mut self, msg: RenderMsg) {
        // Fails only when the renderer is gone (shutdown).
        let _ = self.render.send(msg);
    }

    /// Apply one command.
    pub fn send(&mut self, cmd: LiveCommand) {
        match cmd {
            LiveCommand::SelectSlot { lane, slot } => {
                let (l, slot) = (lane as usize, slot as usize);
                if l < self.lanes.len() && slot < SLOTS_PER_CHAIN {
                    self.lanes[l].selected_slot = slot;
                    self.shared.lane(l).set_selected_slot(slot as u8);
                }
            }
            LiveCommand::SetSlotSeed { lane, slot, seed } => self.set_seed(lane as usize, slot as usize, seed),
            LiveCommand::SeedDelta { lane, delta } => {
                if let Some(l) = self.lanes.get(lane as usize) {
                    let slot = l.selected_slot;
                    self.set_seed(lane as usize, slot, l.seeds[slot].wrapping_add_signed(delta));
                }
            }
            LiveCommand::LaneSeedDelta { lane, delta } => {
                if let Some(l) = self.lanes.get(lane as usize) {
                    let seeds = l.seeds;
                    for (slot, seed) in seeds.iter().enumerate() {
                        self.set_seed(lane as usize, slot, seed.wrapping_add_signed(delta));
                    }
                }
            }
            LiveCommand::SetLoopLen { lane, len } => self.set_loop_len(lane as usize, len),
            LiveCommand::SetControl { lane, control, value } => {
                self.set_control(lane as usize, control as usize, value)
            }
            LiveCommand::SetLaneOn { lane, on } => self.set_lane_on(lane as usize, on),
            LiveCommand::ToggleLane { lane } => {
                if let Some(on) = self.lanes.get(lane as usize).map(|l| !l.on) {
                    self.set_lane_on(lane as usize, on);
                }
            }
            LiveCommand::SetQuantize(on) => {
                self.quantize = on;
                self.shared.set_quantize(on);
            }
            LiveCommand::BrowseOption { lane, delta } => self.browse_option(lane as usize, delta),
            LiveCommand::CommitOption { lane } => {
                if let Some(option) = self.lanes.get_mut(lane as usize).and_then(|l| l.browse.take()) {
                    self.set_option(lane as usize, option);
                }
            }
            LiveCommand::SetOption { lane, option } => self.set_option(lane as usize, option as usize),
            LiveCommand::MovePress { lane, kind } => {
                if (lane as usize) < self.lanes.len() {
                    self.rt(RtMsg::MovePress { lane, kind });
                }
            }
            LiveCommand::MoveRelease { lane, kind } => {
                if (lane as usize) < self.lanes.len() {
                    self.rt(RtMsg::MoveRelease { lane, kind });
                }
            }
            LiveCommand::SetRollDivision { ticks } => self.rt(RtMsg::RollDivision { ticks }),
            LiveCommand::SetKeyIntent(position) => {
                let target = key_intent(self.shared.key(), position);
                self.intent = target.transition.map(|_| target.key);
                self.shared.set_intent_key(self.intent);
            }
            LiveCommand::CommitKey => {
                if let Some(key) = self.intent {
                    self.render(RenderMsg::Key(key));
                }
            }
            LiveCommand::SetKey(key) => self.render(RenderMsg::Key(key)),
            LiveCommand::SelectProgression(index) => {
                if index < self.progressions {
                    self.render(RenderMsg::Progression(index));
                }
            }
            LiveCommand::ProgressionDelta(delta) => {
                if self.progressions > 0 {
                    let base = self.shared.pending_progression().unwrap_or_else(|| self.shared.progression());
                    let next = (base as i64 + delta as i64).rem_euclid(self.progressions as i64) as usize;
                    self.render(RenderMsg::Progression(next));
                }
            }
            LiveCommand::RequestTempo { bpm, bars } => {
                if bpm.is_finite() {
                    let bpm = bpm.clamp(self.tempo.min, self.tempo.max);
                    self.rt(RtMsg::Tempo { bpm, bars });
                }
            }
            LiveCommand::SetHalfTime(on) => self.set_half_time(on),
            LiveCommand::ToggleHalfTime => self.set_half_time(!self.half_time),
            LiveCommand::Capture(scope) => {
                self.capture(scope);
            }
            LiveCommand::RecallCapture(capture) => self.recall(&capture),
            LiveCommand::StartRecording(producer) => self.rt(RtMsg::StartRecording(producer)),
            LiveCommand::StopRecording => self.rt(RtMsg::StopRecording),
        }
    }

    fn set_seed(&mut self, lane: usize, slot: usize, seed: u32) {
        if lane < self.lanes.len() && slot < SLOTS_PER_CHAIN {
            self.lanes[lane].seeds[slot] = seed;
            self.shared.lane(lane).set_seed(slot, seed);
            self.render(RenderMsg::Seed { lane: lane as u8, slot: slot as u8, seed });
        }
    }

    fn set_loop_len(&mut self, lane: usize, len: u8) {
        if lane < self.lanes.len() {
            let len = snap_loop_len(len as usize);
            self.lanes[lane].loop_len = len;
            self.shared.lane(lane).set_loop_len(len);
            self.render(RenderMsg::LoopLen { lane: lane as u8, len });
        }
    }

    fn set_control(&mut self, lane: usize, control: usize, value: f32) {
        if lane >= self.lanes.len() || control >= CONTROLS_PER_LANE || !value.is_finite() {
            return;
        }
        let position = value.clamp(0.0, 1.0);
        self.lanes[lane].positions[control] = position;
        self.shared.lane(lane).set_control(control, position);
        let c = &self.lanes[lane].controls[control];
        let mapped = c.spec.map(position);
        let (lane8, control8) = (lane as u8, control as u8);
        match c.route {
            ControlRoute::Generator(index) => {
                self.render(RenderMsg::GenParam { lane: lane8, index: index as u8, value: mapped })
            }
            ControlRoute::Instrument(param) => {
                self.rt(RtMsg::InstrumentParam { lane: lane8, control: control8, param, value: mapped })
            }
            ControlRoute::Fx { slot, param } => self.rt(RtMsg::FxParam { lane: lane8, slot, param, value: mapped }),
            ControlRoute::Glue(glue) => self.rt(RtMsg::Glue { lane: lane8, control: glue, value: mapped }),
            ControlRoute::Send(bus) => self.rt(RtMsg::Send { lane: lane8, bus, level: mapped }),
            ControlRoute::Unresolved => {}
        }
    }

    fn set_lane_on(&mut self, lane: usize, on: bool) {
        if lane < self.lanes.len() {
            self.lanes[lane].on = on;
            self.rt(RtMsg::LaneOn { lane: lane as u8, on, quantized: self.quantize });
        }
    }

    fn browse_option(&mut self, lane: usize, delta: i32) {
        let Some(l) = self.lanes.get_mut(lane) else { return };
        let n = l.option_available.len() as i64;
        if n == 0 || delta == 0 {
            return;
        }
        // Step to the next option that loaded, in the direction of travel.
        let mut at = l.browse.unwrap_or(l.option) as i64;
        let dir = delta.signum() as i64;
        let mut left = delta.unsigned_abs();
        let mut guard = 0;
        while left > 0 && guard < 2 * n {
            at = (at + dir).rem_euclid(n);
            guard += 1;
            if l.option_available[at as usize] {
                left -= 1;
            }
        }
        if l.option_available[at as usize] {
            l.browse = Some(at as usize);
            self.shared.lane(lane).set_option_pending(Some(at as u16));
        }
    }

    fn set_option(&mut self, lane: usize, option: usize) {
        let Some(l) = self.lanes.get_mut(lane) else { return };
        if !l.option_available.get(option).copied().unwrap_or(false) {
            return;
        }
        l.option = option;
        l.browse = None;
        self.shared.lane(lane).set_option_pending(Some(option as u16));
        self.rt(RtMsg::Option { lane: lane as u8, option: option as u16 });
        if self.lanes[lane].role == Role::Drums {
            // The drum generator follows the kit (sample and slice counts differ).
            self.render(RenderMsg::Kit { lane: lane as u8, kit: option as u16 });
        }
    }

    fn set_half_time(&mut self, on: bool) {
        self.half_time = on;
        self.shared.set_half_time_target(on);
        self.render(RenderMsg::HalfTime(on));
    }

    /// Snapshot the current state (D16). The capture is added to the bank
    /// and, when a collection is set, saved on a short-lived thread.
    pub fn capture(&mut self, scope: CaptureScope) -> Capture {
        let global = GlobalCapture {
            key: self.shared.pending_key().unwrap_or_else(|| self.shared.key()),
            progression: self.shared.pending_progression().unwrap_or_else(|| self.shared.progression()),
            tempo: self.shared.clock().target_bpm,
            half_time: self.half_time,
        };
        let lanes: [LaneCapture; NUM_LANES] = std::array::from_fn(|i| {
            let l = &self.lanes[i];
            LaneCapture { seeds: l.seeds, loop_len: l.loop_len, controls: l.positions, option: l.option, on: l.on }
        });
        let capture = Capture::new(self.style.clone(), scope, global, lanes);
        self.captures.push(capture.clone());
        if let Some(collection) = self.capture_collection.clone() {
            let to_save = capture.clone();
            let saved = self.saved_tx.clone();
            let spawned = std::thread::Builder::new().name("live-capture-save".into()).spawn(move || {
                let result = match to_save.save(&collection) {
                    Ok(path) => {
                        log::info!("[LIVE] capture saved to {}", path.display());
                        Ok(path)
                    }
                    Err(e) => {
                        log::warn!("[LIVE] capture not saved: {e}");
                        Err(e.to_string())
                    }
                };
                let _ = saved.send(result);
            });
            if let Err(e) = spawned {
                log::warn!("[LIVE] capture not saved: {e}");
                let _ = self.saved_tx.send(Err(e.to_string()));
            }
        }
        capture
    }

    /// Restore a capture. A lane-scoped capture restores only its lane;
    /// a full capture also restores key, progression, tempo and half-time.
    /// Changes land like the commands they are made of (next bar / phrase).
    pub fn recall(&mut self, capture: &Capture) {
        let lanes = match capture.scope {
            CaptureScope::Lane { lane } => lane as usize..lane as usize + 1,
            CaptureScope::All => 0..NUM_LANES,
        };
        for li in lanes {
            let Some(src) = capture.lanes.get(li) else { continue };
            if li >= self.lanes.len() {
                continue;
            }
            for (slot, &seed) in src.seeds.iter().enumerate() {
                self.set_seed(li, slot, seed);
            }
            self.set_loop_len(li, src.loop_len);
            for (control, &value) in src.controls.iter().enumerate() {
                self.set_control(li, control, value);
            }
            if src.option != self.lanes[li].option {
                self.set_option(li, src.option);
            }
            self.set_lane_on(li, src.on);
        }
        if capture.scope == CaptureScope::All {
            self.render(RenderMsg::Key(capture.global.key));
            if capture.global.progression < self.progressions {
                self.render(RenderMsg::Progression(capture.global.progression));
            }
            let bpm = capture.global.tempo.clamp(self.tempo.min, self.tempo.max);
            self.rt(RtMsg::Tempo { bpm, bars: DEFAULT_TEMPO_RAMP_BARS });
            self.set_half_time(capture.global.half_time);
        }
    }

    /// Push the style's defaults (send levels, control positions) to the
    /// engine and the renderer.
    fn apply_defaults(&mut self, sends: &[super::style::SendLevels]) {
        for (li, s) in sends.iter().enumerate() {
            self.rt(RtMsg::Send { lane: li as u8, bus: SendBus::Delay, level: s.delay });
            self.rt(RtMsg::Send { lane: li as u8, bus: SendBus::Reverb, level: s.reverb });
        }
        for li in 0..self.lanes.len() {
            for slot in 0..SLOTS_PER_CHAIN {
                self.shared.lane(li).set_seed(slot, self.lanes[li].seeds[slot]);
            }
            for control in 0..CONTROLS_PER_LANE {
                let position = self.lanes[li].positions[control];
                self.set_control(li, control, position);
            }
            self.shared.lane(li).set_option_pending(Some(self.lanes[li].option as u16));
        }
    }
}

/// UI-side handles of a running live engine.
pub struct LiveHandles {
    pub controller: LiveController,
    pub shared: Arc<LiveShared>,
    /// Keeps the bar renderer thread alive; dropping it stops the thread.
    pub renderer: RendererHandle,
    /// Style parts that could not be loaded.
    pub warnings: Vec<String>,
}

/// A live engine with its renderer not yet on a thread. Offline tools and
/// tests keep the renderer and call [`BarRenderer::service`] between blocks;
/// [`LiveEngine::start`] spawns it instead.
pub struct LiveSystem {
    pub engine: LiveEngine,
    pub controller: LiveController,
    pub renderer: BarRenderer,
    pub shared: Arc<LiveShared>,
    pub warnings: Vec<String>,
}

/// Wire a loaded style into an engine, a controller and a bar renderer.
/// The first bar is rendered before this returns. Non-RT.
pub fn build(loaded: LoadedStyle) -> LiveSystem {
    let LoadedStyle {
        name,
        sample_rate,
        max_block,
        tempo,
        key,
        progressions,
        default_progression,
        glue: glue_params,
        lanes,
        meta,
        claps,
        warnings,
        ..
    } = loaded;

    let shared = Arc::new(LiveShared::new(tempo.default, key, default_progression));
    let (rt_tx, rt_rx) = rtrb::RingBuffer::new(RT_MSG_CAPACITY);
    let (trash_tx, trash_rx) = rtrb::RingBuffer::new(2 * MAX_RECORDINGS);

    let mut render_lanes = Vec::with_capacity(NUM_LANES);
    let mut controls = Vec::with_capacity(NUM_LANES);
    let mut sends = Vec::with_capacity(NUM_LANES);
    let mut setups = [LaneSetup::default(); NUM_LANES];
    for (li, m) in meta.into_iter().enumerate() {
        // MUSICAL: see DEFAULT_SEED.
        let seeds = [DEFAULT_SEED + li as u32; SLOTS_PER_CHAIN];
        // The option trim is applied at the instrument (see `loader`), so the glue trim stays 0.
        setups[li] = LaneSetup { role: m.role, trim_db: 0.0, delay_send: m.sends.delay, reverb_send: m.sends.reverb };
        sends.push(m.sends);
        controls.push(LaneControl {
            name: m.name,
            role: m.role,
            positions: std::array::from_fn(|c| m.controls[c].spec.default),
            controls: m.controls,
            seeds,
            selected_slot: 0,
            loop_len: DEFAULT_LOOP_LEN,
            option_names: m.option_names,
            option_available: m.option_available,
            option: m.default_option,
            browse: None,
            on: true,
        });
        render_lanes.push(RenderLane {
            role: m.role,
            generator: m.generator,
            seeds,
            loop_len: DEFAULT_LOOP_LEN,
            params: m.gen_params,
            kits: m.drum_kits,
            kit: 0,
        });
    }

    let n_progressions = progressions.len();
    let (mut renderer, render_tx, rings) =
        BarRenderer::new(Arc::clone(&shared), render_lanes, progressions, key, default_progression, claps, trash_rx);

    let mut glue = GlueChain::new(max_block);
    glue.set_params(&glue_params);
    for (li, setup) in setups.iter().enumerate() {
        glue.set_lane(li, *setup);
    }

    let engine = LiveEngine {
        clock: LiveClock::new(sample_rate, tempo.default),
        player: StepPlayer::new(rings.rx, rings.ret),
        lanes,
        glue,
        glue_params,
        glue_dirty: false,
        setups,
        lane_bufs: std::array::from_fn(|_| StereoBuffer::silence(max_block)),
        mix: StereoBuffer::silence(max_block),
        msgs: rt_rx,
        trash: trash_tx,
        recording: Vec::with_capacity(MAX_RECORDINGS),
        shared: Arc::clone(&shared),
        scopes: [ScopeWriter::default(); NUM_LANES],
        segs: [BLANK_SEGMENT; MAX_SEGMENTS],
        max_block,
        output_latency: Arc::new(AtomicU64::new(0)),
        profiling: false,
        pinned: false,
        times: StageTimes::default(),
    };

    let (saved_tx, saved_rx) = mpsc::channel();
    let mut controller = LiveController {
        shared: Arc::clone(&shared),
        rt: rt_tx,
        render: render_tx,
        style: name,
        tempo,
        lanes: controls,
        progressions: n_progressions,
        quantize: false,
        half_time: false,
        intent: None,
        captures: Vec::new(),
        capture_collection: None,
        saved_tx,
        saved_rx,
        dropped: 0,
    };
    controller.apply_defaults(&sends);
    // Bar 0 has to be waiting when the audio thread starts.
    renderer.service();

    LiveSystem { engine, controller, renderer, shared, warnings }
}

#[cfg(test)]
mod tests;
