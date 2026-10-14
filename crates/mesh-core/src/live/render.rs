//! Bar renderer (design §4.2, §4.4).
//!
//! Runs the lane generators off the audio thread. For the bar after the one
//! the RT thread is playing it renders, per lane, the bar and its fill
//! variant, and ships them as one [`BarPacket`]. When a structural input
//! (seed, generator parameter, key, progression, half-time) changes before
//! the boundary, the pending bar is rendered again and the newer packet
//! replaces the older one on the RT side.
//!
//! # Harmony and the bar boundary
//!
//! Key and progression changes are edge-triggered (they land on a phrase
//! start), so the renderer has to know which render the RT thread actually
//! played. Every packet carries an epoch; the RT thread reports the epoch it
//! swapped in ([`LiveShared::played`]). The renderer keeps the harmony state
//! of each shipped epoch and continues from the played one. A re-render that
//! missed the boundary is therefore not counted: its key change stays armed
//! and lands on the next phrase.
//!
//! # Threading
//!
//! [`BarRenderer::service`] is one non-blocking pass. [`BarRenderer::spawn`]
//! runs it on a thread every [`RENDER_POLL`]; offline tools and tests call it
//! between blocks, which makes renders deterministic. The thread also does
//! the live engine's housekeeping: CLAP `idle` and dropping what the RT
//! thread hands back.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::music::MusicalKey;
use crate::types::StereoSample;

use super::clock::BARS_PER_PHRASE;
use super::generator::{BarCtx, DrumLayout, LaneGenerator, MAX_GEN_PARAMS};
use super::glue::Role;
use super::harmony::{HarmonyState, Progression};
use super::lane::ClapCell;
use super::player::{BarPacket, BAR_POOL_SIZE};
use super::state::LiveShared;
use super::loader::DrumKit;
use super::{slot_for_bar, NUM_LANES, SLOTS_PER_CHAIN, STEPS_PER_BAR};

/// How often the renderer thread looks for work. A structural change made
/// later than this before a bar boundary may land one bar late.
pub const RENDER_POLL: Duration = Duration::from_millis(2);
/// Shipped epochs remembered for the pending bar.
const SHIPPED_HISTORY: usize = 2 * BAR_POOL_SIZE;

/// Structural input change, from the controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderMsg {
    Seed { lane: u8, slot: u8, seed: u32 },
    /// Loop the lane's first `len` slots (1, 2, 4 or 8). Lands on the next
    /// bar the renderer has not shipped yet.
    LoopLen { lane: u8, len: u8 },
    GenParam { lane: u8, index: u8, value: f32 },
    /// Arm a key for the next phrase.
    Key(MusicalKey),
    /// Arm a progression for the next phrase.
    Progression(usize),
    HalfTime(bool),
    /// The drum lane switched kit: use that kit's generator and layout.
    Kit { lane: u8, kit: u16 },
}

/// One lane's render inputs.
pub struct RenderLane {
    pub role: Role,
    pub generator: Box<dyn LaneGenerator>,
    pub seeds: [u32; SLOTS_PER_CHAIN],
    /// Bars the lane loops: slot = bar % loop_len.
    pub loop_len: u8,
    pub params: [f32; MAX_GEN_PARAMS],
    /// Drum lane: layout and generator per kit. Empty on other lanes.
    pub kits: Vec<DrumKit>,
    /// Selected kit.
    pub kit: usize,
}

impl RenderLane {
    fn active_generator(&self) -> &dyn LaneGenerator {
        self.kits.get(self.kit).and_then(|k| k.generator.as_deref()).unwrap_or(self.generator.as_ref())
    }

    /// Target layout of the selected kit, for the onset mask.
    fn layout(&self) -> DrumLayout {
        self.kits.get(self.kit).map(|k| k.layout).unwrap_or_else(DrumLayout::standard)
    }
}

/// RT ends of the bar rings.
pub struct BarRings {
    pub rx: rtrb::Consumer<Box<BarPacket>>,
    pub ret: rtrb::Producer<Box<BarPacket>>,
}

pub struct BarRenderer {
    rx: mpsc::Receiver<RenderMsg>,
    shared: Arc<LiveShared>,
    bars_tx: rtrb::Producer<Box<BarPacket>>,
    bars_ret: rtrb::Consumer<Box<BarPacket>>,
    pool: Vec<Box<BarPacket>>,
    /// Set recordings the RT thread stopped feeding, dropped here.
    trash: rtrb::Consumer<rtrb::Producer<StereoSample>>,
    lanes: Vec<RenderLane>,
    progressions: Vec<Progression>,
    /// Harmony as the RT thread has played it.
    played: HarmonyState,
    want_key: Option<MusicalKey>,
    want_progression: Option<usize>,
    half_time: bool,
    epoch: u32,
    /// Harmony state after each shipped render of the pending bar.
    shipped: Vec<(u32, HarmonyState)>,
    /// Bar the next shipment is for.
    pending_bar: u64,
    last_played: Option<(u64, u32)>,
    dirty: bool,
    claps: Vec<Arc<ClapCell>>,
}

impl BarRenderer {
    /// Build the renderer and its channels. `trash` receives what the RT
    /// thread wants dropped off-thread.
    pub fn new(
        shared: Arc<LiveShared>,
        lanes: Vec<RenderLane>,
        progressions: Vec<Progression>,
        key: MusicalKey,
        progression: usize,
        claps: Vec<Arc<ClapCell>>,
        trash: rtrb::Consumer<rtrb::Producer<StereoSample>>,
    ) -> (Self, mpsc::Sender<RenderMsg>, BarRings) {
        assert_eq!(lanes.len(), NUM_LANES);
        let (tx, rx) = mpsc::channel();
        let (bars_tx, bars_rx) = rtrb::RingBuffer::new(BAR_POOL_SIZE);
        let (ret_tx, bars_ret) = rtrb::RingBuffer::new(BAR_POOL_SIZE);
        let renderer = Self {
            rx,
            shared,
            bars_tx,
            bars_ret,
            pool: (0..BAR_POOL_SIZE).map(|_| Box::new(BarPacket::default())).collect(),
            trash,
            lanes,
            progressions,
            played: HarmonyState::new(key, progression),
            want_key: None,
            want_progression: None,
            half_time: false,
            epoch: 0,
            shipped: Vec::with_capacity(SHIPPED_HISTORY + 1),
            pending_bar: 0,
            last_played: None,
            dirty: true,
            claps,
        };
        (renderer, tx, BarRings { rx: bars_rx, ret: ret_tx })
    }

    /// Bar the next shipment is for.
    pub fn pending_bar(&self) -> u64 {
        self.pending_bar
    }

    fn apply(&mut self, msg: RenderMsg) {
        match msg {
            RenderMsg::Seed { lane, slot, seed } => {
                if let Some(s) = self.lanes.get_mut(lane as usize).and_then(|l| l.seeds.get_mut(slot as usize)) {
                    *s = seed;
                }
            }
            RenderMsg::LoopLen { lane, len } => {
                if let Some(l) = self.lanes.get_mut(lane as usize) {
                    l.loop_len = len.clamp(1, SLOTS_PER_CHAIN as u8);
                }
            }
            RenderMsg::GenParam { lane, index, value } => {
                if let Some(p) = self.lanes.get_mut(lane as usize).and_then(|l| l.params.get_mut(index as usize)) {
                    *p = value;
                }
            }
            RenderMsg::Key(key) => {
                self.want_key = (key != self.played.key()).then_some(key);
                self.shared.set_pending_key(self.want_key);
            }
            RenderMsg::Progression(index) => {
                self.want_progression =
                    (index != self.played.progression() && index < self.progressions.len()).then_some(index);
                self.shared.set_pending_progression(self.want_progression);
            }
            RenderMsg::HalfTime(on) => self.half_time = on,
            RenderMsg::Kit { lane, kit } => {
                if let Some(l) = self.lanes.get_mut(lane as usize) {
                    l.kit = kit as usize;
                }
            }
        }
        self.dirty = true;
    }

    /// One pass: take input changes, follow the RT thread, render and ship
    /// the pending bar if needed. Never blocks. Returns `true` when a bar
    /// was shipped.
    pub fn service(&mut self) -> bool {
        while let Ok(msg) = self.rx.try_recv() {
            self.apply(msg);
        }
        while let Ok(packet) = self.bars_ret.pop() {
            self.pool.push(packet);
        }
        while self.trash.pop().is_ok() {}
        for clap in &self.claps {
            clap.idle();
        }

        let played = self.shared.played();
        if played != self.last_played {
            self.last_played = played;
            if let Some((bar, epoch)) = played {
                self.commit(bar, epoch);
            }
        }

        if !self.dirty || self.bars_tx.is_full() {
            return false;
        }
        let Some(mut packet) = self.pool.pop() else { return false };
        self.render(&mut packet);
        match self.bars_tx.push(packet) {
            Ok(()) => {
                self.dirty = false;
                true
            }
            Err(rtrb::PushError::Full(packet)) => {
                self.pool.push(packet);
                false
            }
        }
    }

    /// The RT thread entered `bar` playing the packet with `epoch` (0 = it
    /// repeated the previous bar).
    fn commit(&mut self, bar: u64, epoch: u32) {
        if let Some((_, state)) = self.shipped.iter().find(|(e, _)| *e == epoch) {
            self.played = state.clone();
        }
        if self.want_key == Some(self.played.key()) {
            self.want_key = None;
        }
        if self.want_progression == Some(self.played.progression()) {
            self.want_progression = None;
        }
        self.shared.set_pending_key(self.want_key);
        self.shared.set_pending_progression(self.want_progression);
        self.shipped.clear();
        self.pending_bar = bar + 1;
        self.dirty = true;
    }

    fn render(&mut self, packet: &mut BarPacket) {
        let bar = self.pending_bar;
        let mut harmony = self.played.clone();
        if let Some(key) = self.want_key {
            harmony.request_key(key);
        }
        if let Some(p) = self.want_progression {
            harmony.request_progression(p);
        }
        harmony.begin_bar(bar);
        let ctx = harmony.ctx(bar, &self.progressions);

        self.epoch = if self.epoch >= u32::MAX - 1 { 1 } else { self.epoch + 1 };
        packet.bar = bar;
        packet.epoch = self.epoch;
        packet.key = ctx.key;
        packet.chord_degree = ctx.chord_degree;
        packet.progression = ctx.progression;
        packet.half_time = self.half_time;

        let bar_in_phrase = (bar % BARS_PER_PHRASE) as u8;
        let phrase = (bar / BARS_PER_PHRASE) as u32;
        let no_mask = [0u8; STEPS_PER_BAR];
        let mut drum_mask = [0u8; STEPS_PER_BAR];
        // Lane 0 (drums) first: its onsets feed the other lanes.
        for (li, lane) in self.lanes.iter().enumerate() {
            for fill in [false, true] {
                let out = if fill { &mut packet.lanes[li].fill } else { &mut packet.lanes[li].main };
                out.clear();
                // The slot follows the lane's loop; phrase position stays global.
                let bar_ctx = BarCtx {
                    seed: lane.seeds[slot_for_bar(bar, lane.loop_len)],
                    lane: li as u8,
                    role: lane.role,
                    bar_in_phrase,
                    phrase,
                    fill,
                    half_time: self.half_time,
                    params: &lane.params,
                    harmony: ctx,
                    drum_mask: if li == 0 { &no_mask } else { &drum_mask },
                };
                lane.active_generator().render_bar(&bar_ctx, out);
                // The player relies on tick order.
                out.sort();
            }
            if li == 0 {
                drum_mask = lane.layout().onset_mask(packet.lanes[0].main.as_slice());
            }
        }

        if self.shipped.len() == SHIPPED_HISTORY {
            self.shipped.remove(0);
        }
        self.shipped.push((self.epoch, harmony));
    }

    /// Run the renderer on its own thread until the handle is dropped.
    pub fn spawn(mut self) -> RendererHandle {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("live-bar-renderer".into())
            .spawn(move || {
                // Light background work: A55 cores, away from the live
                // audio thread on the A76 cores (`rt::live_cores`).
                crate::rt::pin_to_cores(crate::rt::live_cores::BACKGROUND);
                while !stop_thread.load(Ordering::Relaxed) {
                    match self.rx.recv_timeout(RENDER_POLL) {
                        Ok(msg) => self.apply(msg),
                        Err(RecvTimeoutError::Timeout) => {}
                        // The controller is gone; the engine keeps playing.
                        Err(RecvTimeoutError::Disconnected) => std::thread::sleep(RENDER_POLL),
                    }
                    self.service();
                }
            })
            .expect("spawn live bar renderer");
        RendererHandle { stop, thread: Some(thread) }
    }
}

/// Stops and joins the renderer thread on drop.
pub struct RendererHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for RendererHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::event::{BarEvents, EventKind, LiveEvent};
    use crate::live::generator::fixed::FixedPattern;
    use crate::live::player::StepPlayer;

    /// Records the context it was rendered with: one event whose fields
    /// carry seed, params and mask.
    struct Probe;

    impl LaneGenerator for Probe {
        fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
            out.push(LiveEvent {
                tick: 0,
                kind: EventKind::Trigger,
                target: ctx.bar_in_phrase,
                key: ctx.harmony.resolve(0, 3),
                vel: 1 + (ctx.seed % 100) as u8,
                gate: (ctx.params[0] * 100.0) as u16,
                note_id: ctx.drum_mask.iter().filter(|&&v| v > 0).count() as u16,
                flags: (ctx.fill as u8) | (ctx.half_time as u8) << 1,
                ..Default::default()
            });
        }

        fn param_names(&self) -> &'static [&'static str] {
            &["p"]
        }
    }

    struct Rig {
        renderer: BarRenderer,
        tx: mpsc::Sender<RenderMsg>,
        player: StepPlayer,
        shared: Arc<LiveShared>,
    }

    fn rig() -> Rig {
        let key = MusicalKey::new(9, true);
        let shared = Arc::new(LiveShared::new(170.0, key, 0));
        let drums = FixedPattern::new(vec![
            LiveEvent { tick: 0, vel: 100, ..Default::default() },
            LiveEvent { tick: 96, vel: 80, ..Default::default() },
        ]);
        let mut lanes = vec![RenderLane {
            role: Role::Drums,
            generator: Box::new(drums),
            seeds: [0; SLOTS_PER_CHAIN],
            loop_len: 4,
            params: [0.5; MAX_GEN_PARAMS],
            kits: vec![
                DrumKit { layout: DrumLayout::standard(), generator: None },
                DrumKit {
                    layout: DrumLayout::empty(),
                    generator: Some(Box::new(FixedPattern::new(vec![LiveEvent { tick: 96, target: 1, vel: 90, ..Default::default() }]))),
                },
            ],
            kit: 0,
        }];
        for role in [Role::Bass, Role::Lead, Role::Extra] {
            lanes.push(RenderLane {
                role,
                generator: Box::new(Probe),
                seeds: [10, 11, 12, 13, 14, 15, 16, 17],
                loop_len: 4,
                params: [0.5; MAX_GEN_PARAMS],
                kits: Vec::new(),
                kit: 0,
            });
        }
        let progressions = vec![Progression::new(&[0, 5, 3, 4], 1).unwrap(), Progression::new(&[0, 6], 2).unwrap()];
        let (_trash_tx, trash) = rtrb::RingBuffer::new(4);
        let (renderer, tx, rings) = BarRenderer::new(Arc::clone(&shared), lanes, progressions, key, 0, Vec::new(), trash);
        Rig { renderer, tx, player: StepPlayer::new(rings.rx, rings.ret), shared }
    }

    impl Rig {
        /// The RT thread reaches the start of `bar`.
        fn rt_enter(&mut self, bar: u64) -> Option<BarPacket> {
            self.player.poll();
            let packet = self.player.begin_bar(bar).cloned();
            self.shared.set_played(bar, packet.as_ref().map(|p| p.epoch).unwrap_or(0));
            packet
        }
    }

    #[test]
    fn renders_one_bar_ahead_with_context() {
        let mut r = rig();
        assert!(r.renderer.service(), "bar 0 is rendered before the engine starts");
        assert!(!r.renderer.service(), "nothing changed");
        let p0 = r.rt_enter(0).unwrap();
        assert_eq!(p0.bar, 0);
        let bass = p0.lanes[1].main.as_slice()[0];
        assert_eq!(bass.vel, 11, "slot 0 seed");
        assert_eq!(bass.note_id, 2, "drum mask has two onsets");
        assert_eq!(bass.key, 57, "A3, chord i");
        assert_eq!(p0.lanes[1].fill.as_slice()[0].flags & 1, 1, "fill variant rendered with fill = true");
        assert_eq!(p0.lanes[0].main.len(), 2);

        assert!(r.renderer.service(), "bar 1 follows as soon as bar 0 plays");
        assert_eq!(r.renderer.pending_bar(), 1);
        let p1 = r.rt_enter(1).unwrap();
        let bass = p1.lanes[1].main.as_slice()[0];
        assert_eq!((bass.target, bass.vel), (1, 12), "bar_in_phrase and slot 1 seed");
        assert_eq!(bass.key, 65, "chord VI of A minor: F4");
    }

    #[test]
    fn structural_change_rerenders_the_pending_bar() {
        let mut r = rig();
        r.renderer.service();
        r.rt_enter(0);
        r.renderer.service();
        let first = r.renderer.epoch;
        r.tx.send(RenderMsg::Seed { lane: 1, slot: 1, seed: 55 }).unwrap();
        r.tx.send(RenderMsg::GenParam { lane: 1, index: 0, value: 0.25 }).unwrap();
        r.tx.send(RenderMsg::HalfTime(true)).unwrap();
        assert!(r.renderer.service());
        assert!(r.renderer.epoch > first);
        let p1 = r.rt_enter(1).unwrap();
        let bass = p1.lanes[1].main.as_slice()[0];
        assert_eq!((bass.vel, bass.gate, bass.flags & 2), (56, 25, 2));
        assert!(p1.half_time);
        assert_eq!(p1.epoch, r.renderer.epoch, "the newer render plays");
    }

    #[test]
    fn key_and_progression_land_on_the_phrase() {
        let mut r = rig();
        r.renderer.service();
        r.rt_enter(0);
        r.tx.send(RenderMsg::Key(MusicalKey::new(0, true))).unwrap();
        r.tx.send(RenderMsg::Progression(1)).unwrap();
        r.renderer.service();
        assert_eq!(r.shared.pending_key(), Some(MusicalKey::new(0, true)));
        assert_eq!(r.shared.pending_progression(), Some(1));
        for bar in 1..4 {
            let p = r.rt_enter(bar).unwrap();
            assert_eq!(p.key, MusicalKey::new(9, true), "bar {bar} stays in A minor");
            assert_eq!(p.progression, 0);
            r.renderer.service();
        }
        let p4 = r.rt_enter(4).unwrap();
        assert_eq!(p4.key, MusicalKey::new(0, true));
        assert_eq!(p4.progression, 1);
        assert_eq!(p4.lanes[1].main.as_slice()[0].key, 48, "C3, new progression starts on its first chord");
        r.renderer.service();
        assert_eq!(r.shared.pending_key(), None, "cleared once the bar played");
        assert_eq!(r.shared.pending_progression(), None);
        // The new progression holds each chord for two bars.
        let p5 = r.rt_enter(5).unwrap();
        assert_eq!(p5.chord_degree, 0);
        r.renderer.service();
        r.renderer.service();
        assert_eq!(r.rt_enter(6).unwrap().chord_degree, 6);
    }

    #[test]
    fn key_change_that_misses_the_boundary_waits_for_the_next_phrase() {
        let mut r = rig();
        r.renderer.service();
        for bar in 0..3 {
            r.rt_enter(bar);
            r.renderer.service();
        }
        // Bar 4 (a phrase start) is already shipped. The RT thread takes it,
        // then the key change arrives: its re-render of bar 4 is too late.
        r.player.poll();
        r.rt_enter(3);
        r.renderer.service();
        r.player.poll();
        let p4 = r.player.begin_bar(4).cloned().unwrap();
        r.tx.send(RenderMsg::Key(MusicalKey::new(0, true))).unwrap();
        assert!(r.renderer.service(), "re-rendered bar 4 in C minor");
        r.shared.set_played(4, p4.epoch);
        assert_eq!(p4.key, MusicalKey::new(9, true));
        r.renderer.service();
        assert_eq!(r.shared.pending_key(), Some(MusicalKey::new(0, true)), "still armed");
        for bar in 5..8 {
            assert_eq!(r.rt_enter(bar).unwrap().key, MusicalKey::new(9, true));
            r.renderer.service();
        }
        assert_eq!(r.rt_enter(8).unwrap().key, MusicalKey::new(0, true));
    }

    #[test]
    fn starved_rt_keeps_harmony_and_gets_the_next_bar() {
        let mut r = rig();
        r.renderer.service();
        r.rt_enter(0);
        // The renderer does not run during bar 0: bar 1 never arrives.
        assert!(r.rt_enter(1).is_none());
        assert_eq!(r.shared.starved_bars(), 1);
        r.renderer.service();
        assert_eq!(r.renderer.pending_bar(), 2);
        assert_eq!(r.rt_enter(2).unwrap().bar, 2);
    }

    #[test]
    fn pool_never_runs_dry_under_constant_rerenders() {
        let mut r = rig();
        r.renderer.service();
        r.rt_enter(0);
        let mut shipped = 0;
        for i in 0..200u32 {
            r.tx.send(RenderMsg::Seed { lane: 2, slot: 1, seed: i }).unwrap();
            if r.renderer.service() {
                shipped += 1;
            }
            // The RT thread polls every few renders.
            if i % 3 == 0 {
                r.player.poll();
            }
        }
        assert!(shipped > 60, "{shipped}");
        r.player.poll();
        r.renderer.service();
        let p1 = r.rt_enter(1).unwrap();
        assert_eq!(p1.lanes[2].main.as_slice()[0].vel, 1 + 99, "latest seed plays");
        // Every packet is accounted for: pool + RT side + rings.
        r.renderer.service();
        assert_eq!(r.renderer.pool.len() + 1 + r.renderer.bars_tx.slots().abs_diff(BAR_POOL_SIZE), BAR_POOL_SIZE);
    }

    /// Seed slot (vel − 11 on the probe lanes) and bar_in_phrase of lane 1 for bars `0..bars`.
    fn played_slots(r: &mut Rig, bars: u64) -> Vec<(u8, u8)> {
        let mut out = Vec::new();
        r.renderer.service();
        for bar in 0..bars {
            let p = r.rt_enter(bar).unwrap();
            let e = p.lanes[1].main.as_slice()[0];
            out.push((e.vel - 11, e.target));
            r.renderer.service();
        }
        out
    }

    #[test]
    fn loop_length_picks_the_slot_and_keeps_the_phrase_position() {
        for (len, want) in [
            (1u8, [0u8, 0, 0, 0, 0, 0, 0, 0, 0]),
            (2, [0, 1, 0, 1, 0, 1, 0, 1, 0]),
            (4, [0, 1, 2, 3, 0, 1, 2, 3, 0]),
            (8, [0, 1, 2, 3, 4, 5, 6, 7, 0]),
        ] {
            let mut r = rig();
            r.tx.send(RenderMsg::LoopLen { lane: 1, len }).unwrap();
            let got = played_slots(&mut r, 9);
            let slots: Vec<u8> = got.iter().map(|g| g.0).collect();
            assert_eq!(slots, want, "loop {len}");
            let phrase: Vec<u8> = got.iter().map(|g| g.1).collect();
            assert_eq!(phrase, [0, 1, 2, 3, 0, 1, 2, 3, 0], "bar_in_phrase stays global (loop {len})");
        }
    }

    #[test]
    fn loop_length_change_lands_on_the_next_bar() {
        let mut r = rig();
        r.tx.send(RenderMsg::LoopLen { lane: 1, len: 8 }).unwrap();
        r.renderer.service();
        for bar in 0..5 {
            r.rt_enter(bar);
            r.renderer.service();
        }
        // Bar 5 is pending (slot 5 at length 8). Shorten to 2 while bar 4 plays.
        r.tx.send(RenderMsg::LoopLen { lane: 1, len: 2 }).unwrap();
        r.renderer.service();
        let vel = |p: BarPacket| p.lanes[1].main.as_slice()[0].vel - 11;
        assert_eq!(vel(r.rt_enter(5).unwrap()), 1, "bar 5 at length 2 plays slot 1");
        r.renderer.service();
        assert_eq!(vel(r.rt_enter(6).unwrap()), 0);
        // Other lanes keep their own length.
        r.renderer.service();
        assert_eq!(r.rt_enter(7).unwrap().lanes[2].main.as_slice()[0].vel - 11, 3, "lane 2 still loops 4");
    }

    #[test]
    fn kit_change_switches_generator_and_mask_layout() {
        let mut r = rig();
        r.renderer.service();
        r.rt_enter(0);
        r.tx.send(RenderMsg::Kit { lane: 0, kit: 1 }).unwrap();
        r.renderer.service();
        let p1 = r.rt_enter(1).unwrap();
        // Kit 1's generator plays a snare only; its layout has no kick, so
        // the mask the bass sees is empty.
        assert_eq!(p1.lanes[0].main.len(), 1);
        assert_eq!(p1.lanes[1].main.as_slice()[0].note_id, 0);
    }
}
