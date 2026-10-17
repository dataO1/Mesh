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
//! # Live parameter moves
//!
//! A generator parameter move (density, mutate, …) also re-renders the bar
//! that is playing, in the harmony it is playing in, and ships it as a
//! replacement ([`BarPacket::replace`]). The RT player swaps it in on its next
//! block and reads on from where it is, so a fader acts within a block
//! instead of on the next bar.
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
use super::event::{flags, BarEvents, EventKind, LiveEvent};
use super::form::{
    form_chord, letter_offset, letter_seed, prime_seed, splice_second_half, Feel, Form, FormPos, Letter, Play, SeedClass,
    FILLER_BOOSTS, FORM_CHANGE_BARS, LETTER_OFFSETS, NOTE_JITTER_SHARE,
};
use super::rng;

/// RNG stream of the form's sound swaps.
const S_SWAP: u32 = 0xF50;
use super::generator::{BarCtx, DrumLayout, LaneGenerator, MAX_GEN_PARAMS};
use super::glue::Role;
use super::harmony::{HarmonyCtx, HarmonyState, Progression};
use super::lane::{ClapCell, OptionFamily};
use super::player::{BarPacket, BAR_POOL_SIZE};
use super::state::{FormView, LiveShared, Rate};
use super::loader::DrumKit;
use super::{NUM_LANES, STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// How often the renderer thread looks for work. A structural change made
/// later than this before a bar boundary may land one bar late.
pub const RENDER_POLL: Duration = Duration::from_millis(2);
/// Shipped epochs remembered for the pending bar.
const SHIPPED_HISTORY: usize = 2 * BAR_POOL_SIZE;


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
    /// Arm a harmony seed (the mode) for the next phrase.
    ModeSeed(u32),
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
    /// The pads' rate, on top of `base_rate`.
    pub rate: Rate,
    /// The lane's centre speed.
    pub base_rate: i8,
    pub params: [f32; MAX_GEN_PARAMS],
    /// Drum lane: layout and generator per kit. Pack lanes: the generator
    /// per option (each pack plays its phrases). Empty on other lanes.
    pub kits: Vec<DrumKit>,
    /// Selected option (the drum lane: its kit).
    pub kit: usize,
    /// Options that loaded, for the form's sound swaps (`*`).
    pub available: Vec<bool>,
    /// Kind of sound per option (missing = sampler); a swap stays inside one kind.
    pub families: Vec<OptionFamily>,
}

/// A form change waiting for its phrase boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FormSwitch {
    form: usize,
    /// First bar that plays the new form.
    at: u64,
}

impl RenderLane {
    fn generator_for(&self, kit: usize) -> &dyn LaneGenerator {
        self.kits.get(kit).and_then(|k| k.generator.as_deref()).unwrap_or(self.generator.as_ref())
    }

    /// Target layout of a kit, for the onset mask.
    fn layout_for(&self, kit: usize) -> DrumLayout {
        self.kits.get(kit).map(|k| k.layout).unwrap_or_else(DrumLayout::standard)
    }

    /// The option a form swap (`*`) plays: the lane's one swap partner, picked
    /// by its master seed among the other loaded options of the same kind
    /// (sampler, CLAP synth or stream sampler; any kit on the drum lane), so
    /// the swap always fits the lane. `None` without such an option.
    fn swap_option(&self, lane: u8) -> Option<usize> {
        let family = |i: usize| self.families.get(i).copied().unwrap_or_default();
        let fits = |i: &usize| *i != self.kit && self.available[*i] && family(*i) == family(self.kit);
        let count = (0..self.available.len()).filter(fits).count() as u32;
        if count == 0 {
            return None;
        }
        let pick = rng::below(rng::hash(self.seed, lane, 0, 0, S_SWAP), count) as usize;
        (0..self.available.len()).filter(fits).nth(pick)
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
    want_mode_seed: Option<u32>,
    want_progression: Option<usize>,
    half_time: bool,
    epoch: u32,
    /// Harmony state after each shipped render of the pending bar.
    shipped: Vec<(u32, HarmonyState)>,
    /// Bar the next shipment is for.
    pending_bar: u64,
    last_played: Option<(u64, u32)>,
    dirty: bool,
    /// A parameter moved: re-render the playing bar too.
    dirty_current: bool,
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
        (key, mode_seed): (MusicalKey, u32),
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
            played: HarmonyState::with_mode_seed(key, progression, mode_seed),
            want_key: None,
            want_mode_seed: None,
            want_progression: None,
            half_time: false,
            epoch: 0,
            shipped: Vec::with_capacity(SHIPPED_HISTORY + 1),
            pending_bar: 0,
            last_played: None,
            dirty: true,
            dirty_current: false,
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
                self.dirty_current = true;
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
            RenderMsg::ModeSeed(seed) => {
                self.want_mode_seed = (seed != self.played.mode_seed()).then_some(seed);
                self.shared.set_mode_pending(self.want_mode_seed.is_some());
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

        let mut shipped = false;
        // The playing bar first, so a fader move is heard at once.
        if self.dirty_current {
            match self.last_played {
                Some((bar, _)) => {
                    if self.ship(bar, true) {
                        self.dirty_current = false;
                        shipped = true;
                    }
                }
                None => self.dirty_current = false,
            }
        }
        if self.dirty && self.ship(self.pending_bar, false) {
            self.dirty = false;
            shipped = true;
        }
        shipped
    }

    /// Render `bar` into a pooled packet and push it to the RT side.
    fn ship(&mut self, bar: u64, current: bool) -> bool {
        if self.bars_tx.is_full() {
            return false;
        }
        let Some(mut packet) = self.pool.pop() else { return false };
        self.render(&mut packet, bar, current);
        match self.bars_tx.push(packet) {
            Ok(()) => true,
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
        if self.want_mode_seed == Some(self.played.mode_seed()) {
            self.want_mode_seed = None;
        }
        self.shared.set_pending_key(self.want_key);
        self.shared.set_pending_progression(self.want_progression);
        self.shared.set_mode_pending(self.want_mode_seed.is_some());
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

    /// Render `bar`: the pending bar, or with `current` the bar the RT thread
    /// is playing (its harmony and half-time as played; a replacement).
    fn render(&mut self, packet: &mut BarPacket, bar: u64, current: bool) {
        let mut harmony = self.played.clone();
        let half_time = if current { self.shared.half_time() } else { self.half_time };
        if !current {
            if let Some(key) = self.want_key {
                harmony.request_key(key);
            }
            if let Some(p) = self.want_progression {
                harmony.request_progression(p);
            }
            if let Some(seed) = self.want_mode_seed {
                harmony.request_mode_seed(seed);
            }
            harmony.begin_bar(bar);
        }
        let ctx = harmony.ctx(bar, &self.progressions);

        if !current {
            self.epoch = if self.epoch >= u32::MAX - 1 { 1 } else { self.epoch + 1 };
        }
        packet.bar = bar;
        packet.epoch = if current { 0 } else { self.epoch };
        packet.replace = current;
        packet.key = ctx.key;
        packet.chord_degree = ctx.chord_degree;
        packet.progression = ctx.progression;
        packet.mode = ctx.mode;
        packet.half_time = half_time;

        let bar_in_phrase = (bar % BARS_PER_PHRASE) as u8;
        let phrase = (bar / BARS_PER_PHRASE) as u32;
        let no_mask = [0u8; STEPS_PER_BAR];
        let mut drum_mask = [0u8; STEPS_PER_BAR];
        let mut bass_onsets = 0u16;
        let mut bass_phrase = [0u16; 4];
        let mut drum_fill = false;
        let mut drum_break = false;
        let mut scratch = BarEvents::default();
        // Lane 0 (drums) first: its onsets feed the other lanes.
        let mut part_buf = BarEvents::default();
        for (li, lane) in self.lanes.iter().enumerate() {
            let lane8 = li as u8;
            let form_index = match self.switches[li] {
                Some(sw) if bar >= sw.at => sw.form,
                _ => lane.form,
            };
            let synth = matches!(lane.role, Role::Lead | Role::Extra);
            let form = self.forms[form_index.min(self.forms.len() - 1)].for_lane(synth);
            // A bar may hold several segments (beat resolution): the first
            // part renders the bar, later parts are spliced over their ticks.
            let (parts, n_parts) = form.parts(bar, lane.seed, lane8);
            let pos = parts[0].pos;
            let seed = letter_seed(lane.seed, lane8, pos.letter);

            // Pitched lanes play the chord the form gives them, in the shared key.
            let mut lane_harmony = ctx;
            if lane.role != Role::Drums {
                lane_harmony.chord_degree = form_chord(lane.seed, lane8, &pos, seed, ctx.key.minor);
            }
            let part_ctx = PartCtx {
                lane: lane8,
                bar,
                bar_in_phrase,
                phrase,
                half_time,
                harmony: lane_harmony,
                drum_mask: if li == 0 { &no_mask } else { &drum_mask },
                bass_onsets: if lane.role == Role::Bass { 0 } else { bass_onsets },
                bass_phrase: if lane.role == Role::Bass { [0; 4] } else { bass_phrase },
                drum_fill: lane.role != Role::Drums && drum_fill,
                drum_break: lane.role != Role::Drums && drum_break,
            };
            let mut swap: Option<(u16, u16, u16)> = None;
            let mut kit = lane.kit;
            for fill in [false, true] {
                let out = if fill { &mut packet.lanes[li].fill } else { &mut packet.lanes[li].main };
                out.clear();
                let (voices, first_kit) = render_part(lane, &part_ctx, &pos, fill, out, &mut scratch);
                if !fill {
                    packet.lanes[li].voices = voices;
                    kit = first_kit.unwrap_or(lane.kit);
                    swap = first_kit.map(|o| (o as u16, parts[0].from, parts[0].to));
                }
                for p in &parts[1..n_parts] {
                    part_buf.clear();
                    let (_, part_kit) = render_part(lane, &part_ctx, &p.pos, fill, &mut part_buf, &mut scratch);
                    splice_range(out, &part_buf, p.from, p.to, &mut scratch);
                    if !fill && swap.is_none() {
                        swap = part_kit.map(|o| (o as u16, p.from, p.to));
                    }
                }
            }
            packet.lanes[li].pos = FormView {
                segment: pos.source,
                letter: pos.letter.index(),
                prime: pos.prime,
                chord: lane_harmony.chord_degree,
            };
            packet.lanes[li].form = form_index as u16;
            packet.lanes[li].swap = swap;
            // Synth lanes open their sends where the drums break or the lane
            // rests, so the last notes ring out; drops stay dry (user).
            packet.lanes[li].break_sends = (synth && (drum_break || pos.play == Play::Rest)).then_some(BREAK_SENDS);
            if li == 0 {
                // A filler segment's last bar is a drum fill: motif leads rest.
                drum_fill = pos.letter.class() == SeedClass::Filler && pos.last_of_segment;
                drum_break = pos.letter.class() == SeedClass::Filler;
                drum_mask = lane.layout_for(kit).onset_mask(packet.lanes[0].main.as_slice());
            }
            if lane.role == Role::Bass {
                bass_phrase = lane.generator.motif_phrase(seed, lane8).unwrap_or([0; 4]);
                bass_onsets = onset_bits(packet.lanes[li].main.as_slice());
            }
        }

        if current {
            return;
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

// MUSICAL: first draft (user: reverb and delay usually high in breaks).
/// Least delay and reverb send of a synth lane in a break or a rest.
pub const BREAK_SENDS: (f32, f32) = (0.35, 0.5);

/// What every part of a lane's bar renders against.
struct PartCtx<'a> {
    lane: u8,
    bar: u64,
    bar_in_phrase: u8,
    phrase: u32,
    half_time: bool,
    harmony: HarmonyCtx,
    drum_mask: &'a [u8; STEPS_PER_BAR],
    bass_onsets: u16,
    bass_phrase: [u16; 4],
    drum_fill: bool,
    /// The drums play a break letter (C, D) this bar.
    drum_break: bool,
}

/// Render a whole bar of `lane` as form position `pos` plays it: the
/// letter's seed and settings, the prime's second half, the feel and the
/// lane speed. Returns the drum voice mask and the option a sound swap (`*`)
/// plays, if any.
fn render_part(
    lane: &RenderLane,
    c: &PartCtx,
    pos: &FormPos,
    fill: bool,
    out: &mut BarEvents,
    scratch: &mut BarEvents,
) -> (u8, Option<usize>) {
    // Rests, and break-only segments outside the drums' breaks, play nothing.
    let silent = match pos.play {
        Play::All => false,
        Play::Rest => true,
        Play::Breaks => !c.drum_break,
    };
    if silent {
        out.clear();
        return (0, None);
    }
    let seed = letter_seed(lane.seed, c.lane, pos.letter);
    let class = pos.letter.class();
    // A form swap (`*`) plays another of the lane's sounds.
    let swap = if pos.swap { lane.swap_option(c.lane) } else { None };
    let generator = lane.generator_for(swap.unwrap_or(lane.kit));
    let mut params = lane.params;
    vary_letter(generator.param_names(), seed, c.lane, pos.letter, pos.second, &mut params);
    if class == SeedClass::Filler {
        boost_filler(generator.param_names(), &mut params);
    }
    // The drum lane fills the last bar of a filler segment.
    let filler_fill = lane.role == Role::Drums && class == SeedClass::Filler && pos.last_of_segment;
    let mut bar_ctx = BarCtx {
        seed,
        sound_seed: if class == SeedClass::Main { lane.seed } else { seed },
        class,
        lane: c.lane,
        role: lane.role,
        bar_in_phrase: c.bar_in_phrase,
        phrase: c.phrase,
        fill: fill || filler_fill,
        half_time: c.half_time,
        params: &params,
        harmony: c.harmony,
        drum_mask: c.drum_mask,
        jitter: note_jitter(generator.param_names()),
        bass_onsets: c.bass_onsets,
        bass_phrase: c.bass_phrase,
        drum_fill: c.drum_fill,
    };
    generator.render_bar(&bar_ctx, out);
    out.sort();
    let voices = generator.voice_mask(&bar_ctx);
    if pos.prime {
        // A': same seed, a different second half.
        bar_ctx.seed = prime_seed(seed, c.lane);
        scratch.clear();
        generator.render_bar(&bar_ctx, scratch);
        scratch.sort();
        splice_second_half(out, scratch);
    }
    Feel::of(seed, c.lane, class).apply(out, seed, c.lane);
    if lane.role != Role::Drums {
        // The bass block's own speed, then the centre and the pads on top.
        let block = if lane.role == Role::Bass { block_rate(seed, c.lane, class) } else { 0 };
        apply_speed(out, lane.base_rate + lane.rate.exponent() + block, c.bar, scratch);
    }
    (voices, swap)
}

// MUSICAL: tune by ear (user: the seed and the form set the bass speed per
// block; the rate pads move it further). Speed steps (exponent on the
// centre speed) and their weights: main letters stay home more, fillers
// speed up more.
const BLOCK_RATE_MAIN: [(i8, f32); 3] = [(0, 0.65), (1, 0.25), (-1, 0.1)];
const BLOCK_RATE_FILLER: [(i8, f32); 3] = [(0, 0.35), (1, 0.45), (-1, 0.2)];
const S_BLOCK_RATE: u32 = 0xF51;

/// Speed step of a bass block, seeded by its letter seed.
fn block_rate(seed: u32, lane: u8, class: SeedClass) -> i8 {
    let table = if class == SeedClass::Main { &BLOCK_RATE_MAIN } else { &BLOCK_RATE_FILLER };
    let weights = table.map(|t| t.1);
    table[rng::weighted_pick(rng::hash(seed, lane, 0, 0, S_BLOCK_RATE), &weights).unwrap_or(0)].0
}

/// Replace the events of `out` in ticks `from..to` with those of `part`.
fn splice_range(out: &mut BarEvents, part: &BarEvents, from: u16, to: u16, scratch: &mut BarEvents) {
    scratch.clear();
    for e in out.as_slice().iter().filter(|e| e.tick < from || e.tick >= to) {
        scratch.push(*e);
    }
    for e in part.as_slice().iter().filter(|e| e.tick >= from && e.tick < to) {
        scratch.push(*e);
    }
    scratch.sort();
    out.clear();
    for e in scratch.as_slice() {
        out.push(*e);
    }
}

/// Lowest value a letter moves a non-zero control to.
const LETTER_FLOOR: f32 = 0.05;

/// Move a letter's parameters around the control values ([`LETTER_OFFSETS`]).
fn vary_letter(names: &[&str], seed: u32, lane: u8, letter: Letter, second: bool, params: &mut [f32; MAX_GEN_PARAMS]) {
    for (k, (name, _)) in LETTER_OFFSETS.iter().enumerate() {
        if let Some(i) = names.iter().position(|n| n == name) {
            if let Some(p) = params.get_mut(i) {
                // A control above 0 stays above it, so a block never falls silent.
                let floor = if *p > 0.0 { LETTER_FLOOR.min(*p) } else { 0.0 };
                *p = (*p + letter_offset(seed, lane, letter, k, second)).clamp(floor, 1.0);
            }
        }
    }
}

/// Per-note jitter amplitude per parameter ([`NOTE_JITTER_SHARE`] of the
/// letter offsets).
fn note_jitter(names: &[&str]) -> [f32; MAX_GEN_PARAMS] {
    let mut out = [0.0; MAX_GEN_PARAMS];
    for (name, max) in LETTER_OFFSETS {
        if let Some(i) = names.iter().position(|n| *n == name) {
            if let Some(j) = out.get_mut(i) {
                *j = NOTE_JITTER_SHARE * max;
            }
        }
    }
    out
}

/// Steps of a bar with a note-on, one bit per step.
fn onset_bits(events: &[LiveEvent]) -> u16 {
    let mut bits = 0u16;
    for e in events.iter().filter(|e| e.kind == EventKind::NoteOn) {
        let step = ((e.tick + TICKS_PER_STEP / 2) / TICKS_PER_STEP) as usize;
        if step < STEPS_PER_BAR {
            bits |= 1 << step;
        }
    }
    bits
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

/// Play the bar at `2^exponent` speed. Below 1: each bar plays the next
/// part of the rendered bar (half: first or second half, quarter: one of
/// the four quarters) stretched over the whole bar. A note held over the start of
/// the part comes back on its first tick (held across the barline). 2: the
/// bar compressed into each half. Exponents are clamped to −2..=1.
fn apply_speed(out: &mut BarEvents, exponent: i8, bar: u64, scratch: &mut BarEvents) {
    let half = TICKS_PER_BAR / 2;
    match exponent.clamp(-2, 1) {
        0 => return,
        e if e < 0 => {
            let k = 1u16 << (-e) as u16;
            let part = TICKS_PER_BAR / k;
            let from = (bar % k as u64) as u16 * part;
            scratch.clear();
            // Held across the part start: continue it from the downbeat.
            for e in out.as_slice().iter().filter(|e| {
                e.kind == EventKind::NoteOn && e.gate > 0 && e.tick < from && e.tick + e.gate > from
            }) {
                let mut e = *e;
                e.gate = (e.tick + e.gate - from).min(part) * k;
                e.tick = 0;
                e.flags &= !(flags::TIE | flags::ACCENT);
                e.note_id = e.note_id.wrapping_add(0x2000);
                scratch.push(e);
            }
            for e in out.as_slice().iter().filter(|e| e.tick >= from && e.tick < from + part) {
                let mut e = *e;
                e.tick = (e.tick - from) * k;
                e.gate = e.gate.saturating_mul(k).min(TICKS_PER_BAR - e.tick);
                scratch.push(e);
            }
        }
        _ => {
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
    use crate::live::event::{EventKind, LiveEvent};
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
            base_rate: 0,
            params: [0.5; MAX_GEN_PARAMS],
            kits: vec![
                DrumKit { layout: DrumLayout::standard(), generator: None },
                DrumKit {
                    layout: DrumLayout::empty(),
                    generator: Some(Box::new(FixedPattern::new(vec![LiveEvent { tick: 96, target: 1, vel: 90, ..Default::default() }]))),
                },
            ],
            kit: 0,
            available: vec![true, true],
            families: Vec::new(),
        }];
        for role in [Role::Bass, Role::Lead, Role::Extra] {
            lanes.push(RenderLane {
                role,
                generator: Box::new(Probe),
                seed: 10 * role as u32,
                form: 0,
                rate: Rate::Normal,
                base_rate: 0,
                params: [0.5; MAX_GEN_PARAMS],
                kits: Vec::new(),
                kit: 0,
                available: vec![true],
                families: Vec::new(),
            });
        }
        let progressions = vec![Progression::new(&[0, 5, 3, 4], 1).unwrap(), Progression::new(&[0, 6], 2).unwrap()];
        let (_trash_tx, trash) = rtrb::RingBuffer::new(4);
        let forms = vec![
            Form::single(),
            Form::parse("A3 C1").unwrap(),
            Form::parse("A1 A'1").unwrap(),
            Form::parse("A1 A*1").unwrap(),
            Form::parse("A.5 A*.5").unwrap(),
        ];
        let (renderer, tx, rings) =
            BarRenderer::new(Arc::clone(&shared), lanes, forms, progressions, (key, 0), 0, Vec::new(), trash);
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
    fn bass_blocks_take_seeded_speeds() {
        let speeds = |class| (0..300u32).map(|s| block_rate(s, 1, class)).collect::<Vec<i8>>();
        let (main, filler) = (speeds(SeedClass::Main), speeds(SeedClass::Filler));
        let share = |v: &[i8], x: i8| v.iter().filter(|&&s| s == x).count() as f32 / v.len() as f32;
        assert!(share(&main, 0) > 0.5, "main blocks mostly keep the centre speed");
        assert!(share(&filler, 1) > share(&main, 1), "fillers speed up more");
        assert!(main.contains(&-1) && filler.contains(&-1));
    }

    #[test]
    fn form_letters_pick_seeds_and_filler_boosts() {
        let mut r = rig();
        r.tx.send(RenderMsg::Form { lane: 1, form: 1 }).unwrap();
        let got = played(&mut r, 8);
        let c = 1 + (letter_seed(10, 1, Letter::C) % 100) as u8;
        let vels: Vec<u8> = got.iter().map(|g| g.0).collect();
        assert_eq!(vels[..4], [11, 11, 11, c], "A3 C1");
        // The second pass plays the seeded variation, on the same letter seeds.
        let form = Form::parse("A3 C1").unwrap();
        for bar in 4..8 {
            let letter = form.pos_varied(bar, 10, 1).letter;
            assert_eq!(vels[bar as usize], 1 + (letter_seed(10, 1, letter) % 100) as u8, "bar {bar}");
        }
        let gates: Vec<u16> = got.iter().map(|g| g.1).collect();
        assert_eq!(gates[0], 50, "main bars keep the control value");
        // The bass block's own speed scales the gate; undo it.
        let speed = block_rate(letter_seed(10, 1, Letter::C), 1, SeedClass::Filler);
        let gate = gates[3] as f32 * 2f32.powi(speed as i32);
        assert!(gate >= 65.0, "filler bars push density up: {gate}");
    }

    #[test]
    fn form_swaps_play_another_kit() {
        let mut r = rig();
        r.tx.send(RenderMsg::Form { lane: 0, form: 3 }).unwrap();
        r.renderer.service();
        // The form lands on the next phrase (bar 4): A, then A* on bar 5.
        for bar in 0..4 {
            r.rt_enter(bar);
            r.renderer.service();
        }
        let a = r.rt_enter(4).unwrap();
        assert_eq!(a.lanes[0].swap, None);
        assert_eq!(a.lanes[0].main.len(), 2, "kit 0 pattern");
        r.renderer.service();
        let swapped = r.rt_enter(5).unwrap();
        assert_eq!(swapped.lanes[0].swap, Some((1, 0, crate::live::TICKS_PER_BAR)), "the only other kit, the whole bar");
        assert_eq!(swapped.lanes[0].main.as_slice()[0].target, 1, "kit 1's generator plays");
    }

    #[test]
    fn swap_partner_is_one_fixed_option_of_the_same_family() {
        use crate::live::lane::OptionFamily::{Sampler, Stream, Synth};
        let lane = |seed: u32, kit: usize, families: Vec<OptionFamily>, available: Vec<bool>| RenderLane {
            role: Role::Bass,
            generator: Box::new(Probe),
            seed,
            form: 0,
            rate: Rate::Normal,
            base_rate: 0,
            params: [0.5; MAX_GEN_PARAMS],
            kits: Vec::new(),
            kit,
            available,
            families,
        };
        let families = vec![Sampler, Synth, Sampler, Stream, Sampler, Synth, Stream];
        let mut partners = std::collections::BTreeSet::new();
        for seed in 0..60 {
            let l = lane(seed, 0, families.clone(), vec![true; 7]);
            let partner = l.swap_option(1).expect("two other samplers");
            assert!([2, 4].contains(&partner), "seed {seed}: option {partner} is no sampler");
            assert_eq!(l.swap_option(1), Some(partner), "one fixed partner per lane");
            partners.insert(partner);
            // A synth swaps to the other synth, a stream to the other stream.
            assert_eq!(lane(seed, 1, families.clone(), vec![true; 7]).swap_option(1), Some(5));
            assert_eq!(lane(seed, 3, families.clone(), vec![true; 7]).swap_option(1), Some(6));
        }
        assert_eq!(partners.len(), 2, "the seed picks among them");
        // Unloaded options never partner; nothing of the same kind = no swap.
        let l = lane(3, 0, families.clone(), vec![true, true, false, true, false, true, true]);
        assert_eq!(l.swap_option(1), None);
        // Drum kits (all samplers): any other kit.
        let kits = lane(3, 1, Vec::new(), vec![true; 3]);
        assert!([0, 2].contains(&kits.swap_option(0).unwrap()));
    }

    #[test]
    fn half_bar_swaps_splice_another_kit() {
        let mut r = rig();
        r.tx.send(RenderMsg::Form { lane: 0, form: 4 }).unwrap();
        r.renderer.service();
        for bar in 0..4 {
            r.rt_enter(bar);
            r.renderer.service();
        }
        let p = r.rt_enter(4).unwrap();
        assert_eq!(p.lanes[0].swap, Some((1, 192, crate::live::TICKS_PER_BAR)), "the second half swaps");
        // Kit 0 plays its hits at 0 and 96; kit 1 its hit at 96, outside the half.
        let ticks: Vec<u16> = p.lanes[0].main.as_slice().iter().map(|e| e.tick).collect();
        assert_eq!(ticks, [0, 96], "the first half keeps kit 0's hits");
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
        // Bar 7 is the filler of the new form (C, or D in the second pass).
        for bar in 5..7 {
            r.rt_enter(bar);
            r.renderer.service();
        }
        let want = Form::parse("A3 C1").unwrap().pos_varied(7, 10, 1).letter;
        assert_eq!(want.class(), SeedClass::Filler);
        assert_eq!(r.rt_enter(7).unwrap().lanes[1].pos.letter, want.index());
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
        apply_speed(&mut half, Rate::Half.exponent(), 1, &mut scratch);
        let ticks: Vec<u16> = half.as_slice().iter().map(|e| e.tick).collect();
        assert_eq!(ticks, [0, 192], "odd bars stretch the second half");
        assert_eq!(half.as_slice()[0].gate, 96);
        // A note held over the middle continues from the odd bar's downbeat.
        let mut held = BarEvents::default();
        held.push(LiveEvent { tick: 144, kind: EventKind::NoteOn, gate: 96, note_id: 1, ..Default::default() });
        held.push(LiveEvent { tick: 240, kind: EventKind::NoteOn, gate: 24, note_id: 2, ..Default::default() });
        apply_speed(&mut held, Rate::Half.exponent(), 1, &mut scratch);
        let got: Vec<(u16, u16)> = held.as_slice().iter().map(|e| (e.tick, e.gate)).collect();
        assert_eq!(got, [(0, 96), (96, 48)]);
        let mut double = bar.clone();
        apply_speed(&mut double, Rate::Double.exponent(), 0, &mut scratch);
        // Base half plus the half pad: quarter speed, a quarter of the bar each bar.
        let mut quarter = bar.clone();
        apply_speed(&mut quarter, -2, 2, &mut scratch);
        let ticks: Vec<u16> = quarter.as_slice().iter().map(|e| e.tick).collect();
        assert_eq!(ticks, [0], "bar 2 plays the third quarter (tick 192) at quarter speed");
        // Base half plus the double pad: unchanged.
        let mut same = bar.clone();
        apply_speed(&mut same, Rate::Half.exponent() + Rate::Double.exponent(), 0, &mut scratch);
        assert_eq!(same.as_slice(), bar.as_slice());
        let ticks: Vec<u16> = double.as_slice().iter().map(|e| e.tick).collect();
        assert_eq!(ticks, [0, 48, 96, 144, 192, 240, 288, 336]);
        assert_eq!(double.as_slice()[1].gate, 24);
        let ids: std::collections::HashSet<u16> = double.as_slice().iter().map(|e| e.note_id).collect();
        assert_eq!(ids.len(), 8, "repeats get their own note ids");
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
