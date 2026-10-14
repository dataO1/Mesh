//! Bar renderer (design §4.2, §4.4).
//!
//! Runs the lane generators off the audio thread. For the bar after the one
//! the RT thread is playing it renders, per lane, the bar and its fill
//! variant, and ships them as one [`BarPacket`]. When a structural input
//! (seed, form, rate, generator parameter, key, progression, half-time)
//! changes before the boundary, the pending bar is rendered again and the
//! newer packet replaces the older one on the RT side.
//!
//! # Forms
//!
//! Each lane renders the letter its form places on the bar (see
//! [`super::form`]): the letter seed, its main or filler class (filler bars
//! push some generator parameters up and the drum lane plays its fill in the
//! last filler bar), a primed letter's second half from a derived seed, and
//! the letter's timing feel. Pitched lanes take their chord from the form.
//! A form change lands on the next phrase boundary ([`FORM_CHANGE_BARS`]).
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
use super::event::{flags, BarEvents, EventKind};
use super::form::{
    form_chord, letter_seed, prime_seed, splice_second_half, Feel, Form, SeedClass, FILLER_BOOSTS, FORM_CHANGE_BARS,
};
use super::generator::{BarCtx, DrumLayout, LaneGenerator, MAX_GEN_PARAMS};
use super::glue::Role;
use super::harmony::{HarmonyState, Progression};
use super::lane::ClapCell;
use super::player::{BarPacket, BAR_POOL_SIZE};
use super::state::{FormView, LiveShared, Rate};
use super::loader::DrumKit;
use super::{NUM_LANES, STEPS_PER_BAR, TICKS_PER_BAR};

/// How often the renderer thread looks for work. A structural change made
/// later than this before a bar boundary may land one bar late.
pub const RENDER_POLL: Duration = Duration::from_millis(2);
/// Shipped epochs remembered for the pending bar.
const SHIPPED_HISTORY: usize = 2 * BAR_POOL_SIZE;

// MUSICAL: first draft, tune by ear (Q13).
/// A pitched lane's `gate` parameter at or above this ties every note into
/// the next one, and the next one glides.
pub const LEGATO_FROM: f32 = 0.9;
/// Glide of a legato tie (log scale, as the bass slide).
const LEGATO_GLIDE: u8 = 138;
/// Ticks a tied note overlaps the next one.
const LEGATO_OVERLAP: u16 = 2;

/// Structural input change, from the controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderMsg {
    /// The lane's master seed.
    Seed { lane: u8, seed: u32 },
    /// Switch the lane's form at the next phrase boundary.
    Form { lane: u8, form: u16 },
    /// Note rate of a pitched lane. Lands on the next bar.
    Rate { lane: u8, rate: Rate },
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
    /// Master seed.
    pub seed: u32,
    /// Form index into the renderer's forms.
    pub form: usize,
    pub rate: Rate,
    pub params: [f32; MAX_GEN_PARAMS],
    /// Drum lane: layout and generator per kit. Empty on other lanes.
    pub kits: Vec<DrumKit>,
    /// Selected kit.
    pub kit: usize,
}

/// A form change waiting for its phrase boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FormSwitch {
    form: usize,
    /// First bar that plays the new form.
    at: u64,
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
    /// Pending form change per lane.
    switches: [Option<FormSwitch>; NUM_LANES],
    forms: Vec<Form>,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        shared: Arc<LiveShared>,
        lanes: Vec<RenderLane>,
        forms: Vec<Form>,
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
            switches: [None; NUM_LANES],
            forms: if forms.is_empty() { vec![Form::single()] } else { forms },
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
            RenderMsg::Seed { lane, seed } => {
                if let Some(l) = self.lanes.get_mut(lane as usize) {
                    l.seed = seed;
                }
            }
            RenderMsg::Form { lane, form } => {
                let li = lane as usize;
                if let Some(l) = self.lanes.get(li) {
                    let form = (form as usize).min(self.forms.len() - 1);
                    self.switches[li] = (form != l.form).then(|| FormSwitch {
                        form,
                        at: self.pending_bar.div_ceil(FORM_CHANGE_BARS) * FORM_CHANGE_BARS,
                    });
                    self.shared.lane(li).set_form_pending(self.switches[li].map(|s| s.form as u16));
                }
            }
            RenderMsg::Rate { lane, rate } => {
                if let Some(l) = self.lanes.get_mut(lane as usize) {
                    l.rate = rate;
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
        // Form switches whose phrase has started are now the lane's form.
        for (li, switch) in self.switches.iter_mut().enumerate() {
            if let Some(sw) = *switch {
                if bar >= sw.at {
                    self.lanes[li].form = sw.form;
                    *switch = None;
                    self.shared.lane(li).set_form_pending(None);
                }
            }
        }
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
        let mut scratch = BarEvents::default();
        // Lane 0 (drums) first: its onsets feed the other lanes.
        for (li, lane) in self.lanes.iter().enumerate() {
            let lane8 = li as u8;
            let form_index = match self.switches[li] {
                Some(sw) if bar >= sw.at => sw.form,
                _ => lane.form,
            };
            let form = &self.forms[form_index.min(self.forms.len() - 1)];
            let pos = form.pos(bar);
            let seed = letter_seed(lane.seed, lane8, pos.letter);
            let class = pos.letter.class();
            let generator = lane.active_generator();

            // Pitched lanes play the chord the form gives them, in the shared key.
            let mut lane_harmony = ctx;
            if lane.role != Role::Drums {
                lane_harmony.chord_degree = form_chord(lane.seed, lane8, &pos, seed, ctx.key.minor);
            }
            let mut params = lane.params;
            if class == SeedClass::Filler {
                boost_filler(generator.param_names(), &mut params);
            }
            let feel = Feel::of(seed, lane8, class);
            // The drum lane fills the last bar of a filler segment.
            let filler_fill = lane.role == Role::Drums && class == SeedClass::Filler && pos.last_of_segment;

            for fill in [false, true] {
                let out = if fill { &mut packet.lanes[li].fill } else { &mut packet.lanes[li].main };
                out.clear();
                let mut bar_ctx = BarCtx {
                    seed,
                    sound_seed: if class == SeedClass::Main { lane.seed } else { seed },
                    class,
                    lane: lane8,
                    role: lane.role,
                    bar_in_phrase,
                    phrase,
                    fill: fill || filler_fill,
                    half_time: self.half_time,
                    params: &params,
                    harmony: lane_harmony,
                    drum_mask: if li == 0 { &no_mask } else { &drum_mask },
                };
                generator.render_bar(&bar_ctx, out);
                out.sort();
                if pos.prime {
                    // A': same seed, a different second half.
                    bar_ctx.seed = prime_seed(seed, lane8);
                    scratch.clear();
                    generator.render_bar(&bar_ctx, &mut scratch);
                    scratch.sort();
                    splice_second_half(out, &scratch);
                }
                feel.apply(out, seed, lane8);
                if lane.role != Role::Drums {
                    apply_rate(out, lane.rate, bar, &mut scratch);
                    if param_named(generator.param_names(), &params, "gate").is_some_and(|g| g >= LEGATO_FROM) {
                        legato(out);
                    }
                }
            }
            packet.lanes[li].pos = FormView {
                segment: pos.segment,
                letter: pos.letter.index(),
                prime: pos.prime,
                chord: lane_harmony.chord_degree,
            };
            packet.lanes[li].form = form_index as u16;
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

/// Value of the generator parameter `name`, if the generator has it.
fn param_named(names: &[&str], params: &[f32; MAX_GEN_PARAMS], name: &str) -> Option<f32> {
    names.iter().position(|n| *n == name).and_then(|i| params.get(i).copied())
}

/// Push a filler bar's parameters up ([`FILLER_BOOSTS`]).
fn boost_filler(names: &[&str], params: &mut [f32; MAX_GEN_PARAMS]) {
    for (name, boost) in FILLER_BOOSTS {
        if let Some(i) = names.iter().position(|n| *n == name) {
            if let Some(p) = params.get_mut(i) {
                *p = (*p + boost).clamp(0.0, 1.0);
            }
        }
    }
}

/// Half rate: the bar's first half (even bars) or second half (odd bars) at
/// half speed. Double rate: the bar compressed into each half.
fn apply_rate(out: &mut BarEvents, rate: Rate, bar: u64, scratch: &mut BarEvents) {
    let half = TICKS_PER_BAR / 2;
    match rate {
        Rate::Normal => return,
        Rate::Half => {
            let from = if bar % 2 == 0 { 0 } else { half };
            scratch.clear();
            for e in out.as_slice().iter().filter(|e| e.tick >= from && e.tick < from + half) {
                let mut e = *e;
                e.tick = (e.tick - from) * 2;
                e.gate = e.gate.saturating_mul(2).min(TICKS_PER_BAR - e.tick);
                scratch.push(e);
            }
        }
        Rate::Double => {
            scratch.clear();
            for offset in [0, half] {
                for e in out.as_slice() {
                    let mut e = *e;
                    e.tick = e.tick / 2 + offset;
                    e.gate = if e.gate > 0 { (e.gate / 2).max(1) } else { 0 };
                    if offset > 0 {
                        // Distinct voices for the repeat (NoteOff matching).
                        e.note_id = e.note_id.wrapping_add(0x4000);
                    }
                    scratch.push(e);
                }
            }
            scratch.sort();
        }
    }
    std::mem::swap(out, scratch);
}

/// Tie every note into the next one, which glides (note length at the top
/// of its range).
fn legato(out: &mut BarEvents) {
    let ev = out.as_mut_slice();
    let n = ev.len();
    // Note-on groups by tick: (first index, end index).
    let group_end = |start: usize, ev: &[crate::live::event::LiveEvent]| {
        let mut j = start;
        while j < n && ev[j].tick == ev[start].tick {
            j += 1;
        }
        j
    };
    let mut prev: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < n {
        if ev[i].kind != EventKind::NoteOn {
            i += 1;
            continue;
        }
        let end = group_end(i, ev);
        let ons = |a: usize, b: usize, ev: &[crate::live::event::LiveEvent]| {
            (a..b).filter(|&k| ev[k].kind == EventKind::NoteOn).count()
        };
        if let Some((ps, pe)) = prev {
            let (from, to) = (ev[ps].tick, ev[i].tick);
            for k in ps..pe {
                if ev[k].kind == EventKind::NoteOn {
                    ev[k].gate = (to - from + LEGATO_OVERLAP).min(TICKS_PER_BAR - from);
                }
            }
            // Single notes slide into each other; chords just hold.
            if ons(ps, pe, ev) == 1 && ons(i, end, ev) == 1 {
                let (pk, k) = ((ps..pe).find(|&k| ev[k].kind == EventKind::NoteOn), (i..end).find(|&k| ev[k].kind == EventKind::NoteOn));
                if let (Some(pk), Some(k)) = (pk, k) {
                    if ev[pk].key != ev[k].key {
                        ev[k].flags |= flags::TIE;
                        ev[k].glide = LEGATO_GLIDE;
                    }
                }
            }
        }
        prev = Some((i, end));
        i = end;
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
    use crate::live::event::LiveEvent;
    use crate::live::form::Letter;
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
            &["density"]
        }
    }

    /// Key the probe plays on `lane` in bar `bar` of `form` (its chord root, octave 3).
    fn lane_key(key: MusicalKey, master: u32, lane: u8, form: &Form, bar: u64) -> u8 {
        let pos = form.pos(bar);
        let chord = form_chord(master, lane, &pos, letter_seed(master, lane, pos.letter), key.minor);
        crate::live::harmony::resolve_degree(key, 0, 3, chord as i32)
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
            seed: 0,
            form: 0,
            rate: Rate::Normal,
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
                seed: 10 * role as u32,
                form: 0,
                rate: Rate::Normal,
                params: [0.5; MAX_GEN_PARAMS],
                kits: Vec::new(),
                kit: 0,
            });
        }
        let progressions = vec![Progression::new(&[0, 5, 3, 4], 1).unwrap(), Progression::new(&[0, 6], 2).unwrap()];
        let (_trash_tx, trash) = rtrb::RingBuffer::new(4);
        let forms = vec![Form::single(), Form::parse("A3 C1").unwrap(), Form::parse("A1 A'1").unwrap()];
        let (renderer, tx, rings) =
            BarRenderer::new(Arc::clone(&shared), lanes, forms, progressions, key, 0, Vec::new(), trash);
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
        let am = MusicalKey::new(9, true);
        assert!(r.renderer.service(), "bar 0 is rendered before the engine starts");
        assert!(!r.renderer.service(), "nothing changed");
        let p0 = r.rt_enter(0).unwrap();
        assert_eq!(p0.bar, 0);
        let bass = p0.lanes[1].main.as_slice()[0];
        assert_eq!(bass.vel, 11, "master seed 10 (letter A)");
        assert_eq!(bass.note_id, 2, "drum mask has two onsets");
        assert_eq!(bass.key, lane_key(am, 10, 1, &Form::single(), 0), "A3 on the lane's form chord");
        assert_eq!(p0.lanes[1].fill.as_slice()[0].flags & 1, 1, "fill variant rendered with fill = true");
        assert_eq!(p0.lanes[0].main.len(), 2);
        assert_eq!((p0.lanes[1].pos.letter, p0.lanes[1].form), (0, 0));

        assert!(r.renderer.service(), "bar 1 follows as soon as bar 0 plays");
        assert_eq!(r.renderer.pending_bar(), 1);
        let p1 = r.rt_enter(1).unwrap();
        let bass = p1.lanes[1].main.as_slice()[0];
        assert_eq!((bass.target, bass.vel), (1, 11), "bar_in_phrase, same seed");
        assert_eq!(p1.lanes[2].main.as_slice()[0].key, lane_key(am, 20, 2, &Form::single(), 1), "each lane draws its own chords");
        let pos = Form::single().pos(1);
        let chord = form_chord(10, 1, &pos, letter_seed(10, 1, pos.letter), true);
        assert_eq!(p1.lanes[1].pos.chord, chord, "the packet carries the lane's chord");
    }

    #[test]
    fn structural_change_rerenders_the_pending_bar() {
        let mut r = rig();
        r.renderer.service();
        r.rt_enter(0);
        r.renderer.service();
        let first = r.renderer.epoch;
        r.tx.send(RenderMsg::Seed { lane: 1, seed: 55 }).unwrap();
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
        assert_eq!(
            p4.lanes[1].main.as_slice()[0].key,
            lane_key(MusicalKey::new(0, true), 10, 1, &Form::single(), 4),
            "C minor, the lane keeps its chords"
        );
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
            r.tx.send(RenderMsg::Seed { lane: 2, seed: i }).unwrap();
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

    /// `(vel, gate)` of lane 1's probe event for bars `0..bars`.
    fn played(r: &mut Rig, bars: u64) -> Vec<(u8, u16)> {
        let mut out = Vec::new();
        r.renderer.service();
        for bar in 0..bars {
            let p = r.rt_enter(bar).unwrap();
            let e = p.lanes[1].main.as_slice()[0];
            out.push((e.vel, e.gate));
            r.renderer.service();
        }
        out
    }

    #[test]
    fn form_letters_pick_seeds_and_filler_boosts() {
        let mut r = rig();
        r.tx.send(RenderMsg::Form { lane: 1, form: 1 }).unwrap();
        let got = played(&mut r, 8);
        let c = 1 + (letter_seed(10, 1, Letter::C) % 100) as u8;
        let vels: Vec<u8> = got.iter().map(|g| g.0).collect();
        assert_eq!(vels, [11, 11, 11, c, 11, 11, 11, c], "A3 C1");
        let gates: Vec<u16> = got.iter().map(|g| g.1).collect();
        assert_eq!(gates[0], 50, "main bars keep the control value");
        assert!(gates[3] >= 65, "filler bars push density up: {}", gates[3]);
    }

    #[test]
    fn form_change_lands_on_the_phrase() {
        let mut r = rig();
        r.renderer.service();
        for bar in 0..2 {
            r.rt_enter(bar);
            r.renderer.service();
        }
        r.tx.send(RenderMsg::Form { lane: 1, form: 1 }).unwrap();
        r.renderer.service();
        assert_eq!(r.shared.lane(1).view().form_pending, Some(1));
        for bar in 2..4 {
            let p = r.rt_enter(bar).unwrap();
            assert_eq!(p.lanes[1].form, 0, "bar {bar} keeps the old form");
            r.renderer.service();
        }
        let p4 = r.rt_enter(4).unwrap();
        assert_eq!(p4.lanes[1].form, 1);
        r.renderer.service();
        assert_eq!(r.shared.lane(1).view().form_pending, None, "cleared once the phrase started");
        // Bar 7 is the C of the new form.
        for bar in 5..7 {
            r.rt_enter(bar);
            r.renderer.service();
        }
        assert_eq!(r.rt_enter(7).unwrap().lanes[1].pos.letter, Letter::C.index());
    }

    #[test]
    fn primed_letter_takes_its_second_half_elsewhere() {
        let gen_bar = |seed: u32| {
            let mut out = BarEvents::default();
            for t in [0u16, 96, 192, 288] {
                out.push(LiveEvent { tick: t, vel: 1 + (seed % 100) as u8, ..Default::default() });
            }
            out
        };
        // Same logic as the renderer: first half from A, second from A'.
        let mut a = gen_bar(10);
        let b = gen_bar(prime_seed(10, 1));
        splice_second_half(&mut a, &b);
        let v: Vec<u8> = a.as_slice().iter().map(|e| e.vel).collect();
        assert_eq!(v[..2], [11, 11]);
        assert_eq!(v[2], b.as_slice()[2].vel);

        let mut r = rig();
        r.tx.send(RenderMsg::Form { lane: 1, form: 2 }).unwrap();
        let got = played(&mut r, 2);
        assert_eq!(got[0].0, got[1].0, "the probe's one event sits in the first half: A' starts like A");
    }

    #[test]
    fn rates_reshape_pitched_bars() {
        let mut bar = BarEvents::default();
        for t in [0u16, 96, 192, 288] {
            bar.push(LiveEvent { tick: t, kind: EventKind::NoteOn, gate: 48, note_id: t, ..Default::default() });
        }
        let mut scratch = BarEvents::default();
        let mut half = bar.clone();
        apply_rate(&mut half, Rate::Half, 1, &mut scratch);
        let ticks: Vec<u16> = half.as_slice().iter().map(|e| e.tick).collect();
        assert_eq!(ticks, [0, 192], "odd bars stretch the second half");
        assert_eq!(half.as_slice()[0].gate, 96);
        let mut double = bar.clone();
        apply_rate(&mut double, Rate::Double, 0, &mut scratch);
        let ticks: Vec<u16> = double.as_slice().iter().map(|e| e.tick).collect();
        assert_eq!(ticks, [0, 48, 96, 144, 192, 240, 288, 336]);
        assert_eq!(double.as_slice()[1].gate, 24);
        let ids: std::collections::HashSet<u16> = double.as_slice().iter().map(|e| e.note_id).collect();
        assert_eq!(ids.len(), 8, "repeats get their own note ids");
    }

    #[test]
    fn legato_ties_single_notes_and_holds_chords() {
        let mut bar = BarEvents::default();
        let note = |tick: u16, key: u8| LiveEvent { tick, kind: EventKind::NoteOn, key, gate: 10, ..Default::default() };
        for e in [note(0, 60), note(96, 62), note(192, 64), note(192, 67), note(288, 69)] {
            bar.push(e);
        }
        legato(&mut bar);
        let ev = bar.as_slice();
        assert_eq!(ev[0].gate, 96 + LEGATO_OVERLAP);
        assert!(ev[1].has(flags::TIE) && ev[1].glide == LEGATO_GLIDE);
        assert!(!ev[2].has(flags::TIE) && !ev[3].has(flags::TIE), "a chord does not slide");
        assert_eq!((ev[2].gate, ev[3].gate), (96 + LEGATO_OVERLAP, 96 + LEGATO_OVERLAP));
        assert!(!ev[4].has(flags::TIE), "out of a chord: no slide");
        assert_eq!(ev[4].gate, 10, "the last note keeps its gate");
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
