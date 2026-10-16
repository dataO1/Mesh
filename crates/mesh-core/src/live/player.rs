//! RT step player (design §4.4).
//!
//! Holds the bar that is playing and the bar rendered for the next boundary,
//! and turns clock segments into sample-accurate event firings per lane.
//! Momentary moves are applied here, to the already-rendered bar, so they
//! respond within one block:
//!
//! - **fill**: read the pre-rendered fill variant instead of the main bar,
//! - **double**: play the bar at 2× rate (the whole bar twice per bar),
//! - **stutter**: loop a window of the bar at the roll division,
//! - **void**: fire nothing (the lane also mutes its output).
//!
//! Rendered bars arrive from the bar renderer as pooled [`BarPacket`] boxes
//! over an `rtrb` ring and go back over a second ring when they are replaced,
//! so the RT thread never allocates or frees one. If the packet for a bar has
//! not arrived at the boundary, the playing bar repeats.

use crate::music::MusicalKey;

use super::clock::ClockSegment;
use super::command::MoveKind;
use super::event::{BarEvents, EventKind, LiveEvent};
use super::state::{move_bits, FormView};
use super::{NUM_LANES, STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// Bar packets in circulation. The RT side holds at most two (playing and
/// next); the rest covers packets in flight and re-renders.
pub const BAR_POOL_SIZE: usize = 8;
/// Events a stutter window can hold.
pub const STUTTER_EVENTS: usize = 32;
/// Default stutter / roll division in ticks.
// MUSICAL: 16th-note roll. A 32nd (12) is the classic DnB snare roll.
pub const DEFAULT_ROLL_TICKS: u16 = TICKS_PER_STEP;
/// Rounding tolerance, same as the clock's (`clock::FRAME_EPS`).
const FRAME_EPS: f64 = 1e-6;

/// One lane's rendered bar and its fill variant.
#[derive(Clone, Default)]
pub struct LaneBar {
    pub main: BarEvents,
    pub fill: BarEvents,
    /// Form position and chord this bar was rendered at.
    pub pos: FormView,
    /// Form index the bar was rendered with.
    pub form: u16,
    /// Drum lane: voices the bar's arrangement turns on.
    pub voices: u8,
    /// A form swap (`*`) in this bar: option and its ticks `from..to`.
    pub swap: Option<(u16, u16, u16)>,
    /// Synth lanes in breaks and rests: the lane's least delay and reverb
    /// send for this bar (tails ring out). None: the lane's own sends.
    pub break_sends: Option<(f32, f32)>,
}

/// One rendered bar for all lanes, with the harmony it was rendered in.
#[derive(Clone)]
pub struct BarPacket {
    /// Absolute bar index this packet is for.
    pub bar: u64,
    /// Render generation, > 0. Reported back when the packet is swapped in.
    pub epoch: u32,
    pub key: MusicalKey,
    pub chord_degree: u8,
    pub progression: usize,
    /// Mode in the key's family.
    pub mode: u8,
    pub half_time: bool,
    /// A re-render of the bar that is playing: replaces it mid-bar.
    pub replace: bool,
    pub lanes: [LaneBar; NUM_LANES],
}

impl Default for BarPacket {
    fn default() -> Self {
        Self {
            bar: 0,
            epoch: 0,
            key: MusicalKey::new(9, true),
            chord_degree: 0,
            progression: 0,
            mode: 0,
            half_time: false,
            replace: false,
            lanes: std::array::from_fn(|_| LaneBar::default()),
        }
    }
}

/// Receives the events a lane fires in one segment.
pub trait EventSink {
    /// `offset` is in frames from the start of the segment's block. `fill`
    /// is true when the event came from the fill bar.
    fn fire(&mut self, ev: &LiveEvent, offset: u32, fill: bool);
}

/// Frame offset at which real-valued sub-tick `sub` (0..24 within the
/// segment's step) fires, or `None` when another segment of the step owns it.
/// Same ownership rule as [`ClockSegment::offset_for_tick`], extended to
/// fractional ticks (double rate lands events on half ticks).
pub fn seg_offset(seg: &ClockSegment, sub: f64) -> Option<u32> {
    let t = sub * seg.frames_per_tick() - seg.step_phase;
    let j = (t - FRAME_EPS).ceil() as i64 - seg.frame_in_step as i64;
    if j < 0 {
        return None;
    }
    let j = if j >= seg.frames as i64 {
        if seg.ends_step {
            seg.frames as i64 - 1
        } else {
            return None;
        }
    } else {
        j
    };
    Some(seg.offset + j as u32)
}

/// A captured stutter window.
#[derive(Clone, Copy)]
struct Stutter {
    /// Absolute tick the loop grid is anchored to.
    anchor: i64,
    /// Loop length in ticks.
    div: u16,
    /// Bar tick the window was taken from (for the UI's active step).
    source_tick: u16,
    fill: bool,
    len: usize,
    /// Window events with `tick` relative to the window start.
    events: [LiveEvent; STUTTER_EVENTS],
}

#[derive(Clone, Copy)]
struct LaneMoves {
    held: u8,
    stutter: Option<Stutter>,
}

/// Bar storage and move state of the RT thread.
pub struct StepPlayer {
    current: Option<Box<BarPacket>>,
    next: Option<Box<BarPacket>>,
    /// Bar the next swap is for.
    expected: u64,
    bars_rx: rtrb::Consumer<Box<BarPacket>>,
    bars_ret: rtrb::Producer<Box<BarPacket>>,
    lanes: [LaneMoves; NUM_LANES],
    roll_ticks: u16,
}

impl StepPlayer {
    /// `bars_rx` delivers rendered packets, `bars_ret` returns used ones. The
    /// return ring must hold [`BAR_POOL_SIZE`] packets.
    pub fn new(bars_rx: rtrb::Consumer<Box<BarPacket>>, bars_ret: rtrb::Producer<Box<BarPacket>>) -> Self {
        Self {
            current: None,
            next: None,
            expected: 0,
            bars_rx,
            bars_ret,
            lanes: [LaneMoves { held: 0, stutter: None }; NUM_LANES],
            roll_ticks: DEFAULT_ROLL_TICKS,
        }
    }

    fn give_back(&mut self, packet: Box<BarPacket>) {
        // The return ring is as large as the pool, so this cannot fail.
        let _ = self.bars_ret.push(packet);
    }

    /// Take newly rendered packets. The newest one for the expected bar
    /// replaces any earlier render of it; a replacement of the playing bar
    /// swaps in at once (returns `true`); anything else is stale.
    pub fn poll(&mut self) -> bool {
        let mut replaced = false;
        while let Ok(packet) = self.bars_rx.pop() {
            if packet.replace {
                if self.current.as_ref().is_some_and(|c| c.bar == packet.bar) {
                    if let Some(old) = self.current.replace(packet) {
                        self.give_back(old);
                    }
                    replaced = true;
                } else {
                    self.give_back(packet);
                }
            } else if packet.bar == self.expected {
                if let Some(old) = self.next.replace(packet) {
                    self.give_back(old);
                }
            } else {
                self.give_back(packet);
            }
        }
        replaced
    }

    /// Enter `bar`. Swaps in its packet if it arrived and returns it;
    /// returns `None` when the playing bar has to repeat.
    pub fn begin_bar(&mut self, bar: u64) -> Option<&BarPacket> {
        self.expected = bar + 1;
        match self.next.take() {
            Some(next) if next.bar == bar => {
                if let Some(old) = self.current.replace(next) {
                    self.give_back(old);
                }
                self.current.as_deref()
            }
            stale => {
                if let Some(p) = stale {
                    self.give_back(p);
                }
                None
            }
        }
    }

    /// Packet that is playing.
    pub fn current(&self) -> Option<&BarPacket> {
        self.current.as_deref()
    }

    pub fn set_roll_ticks(&mut self, ticks: u16) {
        self.roll_ticks = ticks.clamp(1, TICKS_PER_BAR);
    }

    /// [`move_bits`] held on `lane`.
    pub fn held(&self, lane: usize) -> u8 {
        self.lanes[lane].held
    }

    pub fn is_void(&self, lane: usize) -> bool {
        self.lanes[lane].held & move_bits::VOID != 0
    }

    /// Press a move. `bar` and `tick` are the clock position of the next frame.
    pub fn press(&mut self, lane: usize, kind: MoveKind, bar: u64, tick: f64) {
        self.lanes[lane].held |= kind.bit();
        if kind == MoveKind::Stutter {
            self.lanes[lane].stutter = Some(self.capture_stutter(lane, bar, tick));
        }
    }

    pub fn release(&mut self, lane: usize, kind: MoveKind) {
        self.lanes[lane].held &= !kind.bit();
        if kind == MoveKind::Stutter {
            self.lanes[lane].stutter = None;
        }
    }

    /// Copy the window to loop out of the playing bar.
    ///
    /// The window starts on the division slot of the lane's most recent onset
    /// (so a roll always has something to repeat), or on the current slot when
    /// the bar has had no onset yet. The loop grid is anchored to the current
    /// slot, which keeps the repeats on the beat grid.
    fn capture_stutter(&self, lane: usize, bar: u64, tick: f64) -> Stutter {
        let div = self.roll_ticks;
        let now = tick.floor().clamp(0.0, (TICKS_PER_BAR - 1) as f64) as u16;
        let slot = now / div * div;
        let fill = self.lanes[lane].held & move_bits::FILL != 0;
        let mut st = Stutter {
            anchor: bar as i64 * TICKS_PER_BAR as i64 + slot as i64,
            div,
            source_tick: slot,
            fill,
            len: 0,
            events: [LiveEvent::default(); STUTTER_EVENTS],
        };
        let Some(packet) = self.current.as_deref() else { return st };
        let events = if fill { &packet.lanes[lane].fill } else { &packet.lanes[lane].main };
        let events = events.as_slice();
        let last_onset = events
            .iter()
            .rev()
            .find(|e| e.tick <= now && matches!(e.kind, EventKind::Trigger | EventKind::NoteOn));
        if let Some(e) = last_onset {
            st.source_tick = e.tick / div * div;
        }
        let (lo, hi) = (st.source_tick, st.source_tick.saturating_add(div));
        for e in events.iter().filter(|e| e.tick >= lo && e.tick < hi) {
            // Gates carry their own release; a NoteOff outside the window would never come.
            if e.kind == EventKind::NoteOff || st.len == STUTTER_EVENTS {
                continue;
            }
            let mut e = *e;
            e.tick -= lo;
            // Each repeat must end before the next one starts.
            if e.gate > div || (e.gate == 0 && e.kind == EventKind::NoteOn) {
                e.gate = div;
            }
            st.events[st.len] = e;
            st.len += 1;
        }
        st
    }

    /// Fire `lane`'s events owned by `seg` into `sink`. Returns the bar step
    /// the lane is reading (for the UI highlight).
    pub fn fire_lane(&self, lane: usize, seg: &ClockSegment, sink: &mut impl EventSink) -> u8 {
        let moves = &self.lanes[lane];
        let step = seg.step as u16;
        if moves.held & move_bits::VOID != 0 {
            return seg.step;
        }

        if let Some(st) = moves.stutter.as_ref() {
            let step_start = seg.bar as i64 * TICKS_PER_BAR as i64 + (step * TICKS_PER_STEP) as i64;
            let div = st.div as i64;
            for e in &st.events[..st.len] {
                // Sub-ticks u in this step with (step_start + u - anchor) ≡ e.tick (mod div).
                let mut u = (e.tick as i64 - (step_start - st.anchor)).rem_euclid(div);
                while u < TICKS_PER_STEP as i64 {
                    if let Some(offset) = seg_offset(seg, u as f64) {
                        sink.fire(e, offset, st.fill);
                    }
                    u += div;
                }
            }
            return (st.source_tick / TICKS_PER_STEP) as u8;
        }

        let Some(packet) = self.current.as_deref() else { return seg.step };
        let fill = moves.held & move_bits::FILL != 0;
        let events = if fill { &packet.lanes[lane].fill } else { &packet.lanes[lane].main };
        let events = events.as_slice();

        if moves.held & move_bits::DOUBLE != 0 {
            // Real step s reads source ticks [48s, 48s + 48) of the bar, wrapped.
            let base = (2 * step * TICKS_PER_STEP) % TICKS_PER_BAR;
            let first = events.partition_point(|e| e.tick < base);
            for e in events[first..].iter().take_while(|e| e.tick < base + 2 * TICKS_PER_STEP) {
                if let Some(offset) = seg_offset(seg, (e.tick - base) as f64 * 0.5) {
                    let mut e = *e;
                    if e.gate > 0 {
                        e.gate = (e.gate / 2).max(1);
                    }
                    sink.fire(&e, offset, fill);
                }
            }
            return ((2 * step) % STEPS_PER_BAR as u16) as u8;
        }

        let base = step * TICKS_PER_STEP;
        let first = events.partition_point(|e| e.tick < base);
        for e in events[first..].iter().take_while(|e| e.tick < base + TICKS_PER_STEP) {
            if let Some(offset) = seg_offset(seg, (e.tick - base) as f64) {
                sink.fire(e, offset, fill);
            }
        }
        seg.step
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::live::clock::{Boundary, LiveClock};

    pub(crate) struct Rig {
        pub player: StepPlayer,
        pub to_rt: rtrb::Producer<Box<BarPacket>>,
        pub from_rt: rtrb::Consumer<Box<BarPacket>>,
        pub clock: LiveClock,
        /// (absolute frame, bar, event, from fill)
        pub fired: Vec<(u64, u64, LiveEvent, bool)>,
        pub frame: u64,
    }

    struct Collect<'a> {
        out: &'a mut Vec<(u64, u64, LiveEvent, bool)>,
        base: u64,
        bar: u64,
    }

    impl EventSink for Collect<'_> {
        fn fire(&mut self, ev: &LiveEvent, offset: u32, fill: bool) {
            self.out.push((self.base + offset as u64, self.bar, *ev, fill));
        }
    }

    pub(crate) fn trigger(tick: u16, target: u8) -> LiveEvent {
        LiveEvent { tick, kind: EventKind::Trigger, target, vel: 100, ..Default::default() }
    }

    pub(crate) fn packet(bar: u64, epoch: u32, main: &[LiveEvent], fill: &[LiveEvent]) -> Box<BarPacket> {
        let mut p = Box::new(BarPacket { bar, epoch, ..Default::default() });
        for e in main {
            p.lanes[0].main.push(*e);
        }
        for e in fill {
            p.lanes[0].fill.push(*e);
        }
        p
    }

    impl Rig {
        pub fn new(bpm: f64) -> Self {
            let (to_rt, bars_rx) = rtrb::RingBuffer::new(BAR_POOL_SIZE);
            let (bars_ret, from_rt) = rtrb::RingBuffer::new(BAR_POOL_SIZE);
            Self {
                player: StepPlayer::new(bars_rx, bars_ret),
                to_rt,
                from_rt,
                clock: LiveClock::new(48_000, bpm),
                fired: Vec::new(),
                frame: 0,
            }
        }

        /// Process one block of lane 0 the way the engine does.
        pub fn block(&mut self, frames: u32) {
            self.player.poll();
            for seg in self.clock.segments(frames) {
                if seg.boundary >= Boundary::Bar {
                    self.player.begin_bar(seg.bar);
                }
                let mut sink = Collect { out: &mut self.fired, base: self.frame, bar: seg.bar };
                self.player.fire_lane(0, &seg, &mut sink);
            }
            self.frame += frames as u64;
            self.player.poll();
        }

        pub fn run(&mut self, frames: u64, block: u32) {
            let mut left = frames;
            while left > 0 {
                let n = left.min(block as u64) as u32;
                self.block(n);
                left -= n as u64;
            }
        }
    }

    fn bar_frames(bpm: f64) -> f64 {
        48_000.0 * 60.0 / bpm * 4.0
    }

    #[test]
    fn seg_offset_matches_clock_for_integer_ticks() {
        let mut clock = LiveClock::new(48_000, 171.3);
        for _ in 0..200 {
            for seg in clock.segments(97) {
                for sub in 0..TICKS_PER_STEP {
                    let tick = seg.step as u16 * TICKS_PER_STEP + sub;
                    assert_eq!(seg_offset(&seg, sub as f64), seg.offset_for_tick(tick));
                }
            }
        }
    }

    #[test]
    fn onsets_are_sample_accurate_at_any_block_size() {
        // 16ths, a 32nd pair and an off-grid triplet tick.
        let pattern: Vec<LiveEvent> =
            [0u16, 24, 36, 96, 104, 191, 192, 300, 383].iter().map(|&t| trigger(t, 0)).collect();
        for bpm in [87.5, 133.33, 170.0, 174.0, 200.0] {
            let fpt = bar_frames(bpm) / TICKS_PER_BAR as f64;
            let total = (bar_frames(bpm) * 3.0).ceil() as u64;
            let mut reference: Option<Vec<u64>> = None;
            for block in [32u32, 64, 127, 256, 1024, 4096] {
                let mut rig = Rig::new(bpm);
                // Feed one bar ahead, like the renderer.
                rig.to_rt.push(packet(0, 1, &pattern, &[])).ok().unwrap();
                let mut fed = 1u64;
                let mut left = total;
                while left > 0 {
                    let n = left.min(block as u64) as u32;
                    rig.block(n);
                    left -= n as u64;
                    let playing = rig.clock.position().bar;
                    while fed <= playing + 1 {
                        rig.to_rt.push(packet(fed, 1, &pattern, &[])).ok().unwrap();
                        fed += 1;
                    }
                    while rig.from_rt.pop().is_ok() {}
                }
                let frames: Vec<u64> = rig.fired.iter().map(|f| f.0).collect();
                assert_eq!(frames.len(), pattern.len() * 3, "bpm {bpm} block {block}");
                for (i, &f) in frames.iter().enumerate() {
                    let tick = (i / pattern.len()) as f64 * TICKS_PER_BAR as f64 + pattern[i % pattern.len()].tick as f64;
                    let exact = tick * fpt;
                    // First frame at or after the event's real time (float carry may move it by one).
                    assert!(
                        (f as f64 - exact.ceil()).abs() <= 1.0 && f as f64 > exact - 1.0,
                        "bpm {bpm} block {block} event {i}: frame {f}, exact {exact}"
                    );
                }
                match &reference {
                    None => reference = Some(frames),
                    Some(r) => assert_eq!(&frames, r, "bpm {bpm}: block {block} moved an onset"),
                }
            }
        }
    }

    #[test]
    fn swap_lands_exactly_on_the_bar_and_newest_render_wins() {
        let bpm = 170.0;
        let mut rig = Rig::new(bpm);
        rig.to_rt.push(packet(0, 1, &[trigger(0, 1), trigger(192, 1)], &[])).ok().unwrap();
        rig.run(1000, 256);
        // Two renders of bar 1: the later one must play.
        rig.to_rt.push(packet(1, 2, &[trigger(0, 2)], &[])).ok().unwrap();
        rig.run(1000, 256);
        rig.to_rt.push(packet(1, 3, &[trigger(0, 3), trigger(96, 3)], &[])).ok().unwrap();
        rig.run((bar_frames(bpm) * 2.0) as u64 - 2000, 256);

        let bar1 = bar_frames(bpm).ceil() as u64;
        let targets: Vec<(u64, u8)> = rig.fired.iter().map(|f| (f.0, f.2.target)).collect();
        assert_eq!(targets[0], (0, 1));
        assert_eq!(targets[2], (bar1, 3), "{targets:?}");
        assert!(targets.iter().all(|t| t.1 != 2), "superseded render played");
        assert!(targets.iter().all(|&(f, t)| (t == 1) == (f < bar1)), "{targets:?}");
        // The superseded packet and bar 0's packet came back for reuse.
        let mut returned = Vec::new();
        while let Ok(p) = rig.from_rt.pop() {
            returned.push(p.epoch);
        }
        assert_eq!(returned, vec![2, 1]);
    }

    #[test]
    fn starvation_repeats_the_playing_bar() {
        let bpm = 170.0;
        let mut rig = Rig::new(bpm);
        rig.to_rt.push(packet(0, 1, &[trigger(0, 7), trigger(96, 7)], &[])).ok().unwrap();
        rig.run((bar_frames(bpm) * 3.0) as u64, 512);
        assert_eq!(rig.fired.len(), 6, "bar 0 played three times");
        assert_eq!(rig.fired[2].1, 1);
        assert_eq!(rig.fired[4].1, 2);
        // A packet for a bar that already passed is returned unused.
        rig.to_rt.push(packet(1, 2, &[], &[])).ok().unwrap();
        rig.block(64);
        assert_eq!(rig.from_rt.pop().unwrap().epoch, 2);
        assert_eq!(rig.player.current().unwrap().epoch, 1);
    }

    #[test]
    fn fill_switches_source_while_held() {
        let mut rig = Rig::new(170.0);
        let main = [trigger(0, 1), trigger(96, 1), trigger(192, 1), trigger(288, 1)];
        let fill = [trigger(0, 9), trigger(96, 9), trigger(192, 9), trigger(288, 9)];
        rig.to_rt.push(packet(0, 1, &main, &fill)).ok().unwrap();
        let beat = (bar_frames(170.0) / 4.0) as u64;
        rig.run(beat + 100, 256);
        rig.player.press(0, MoveKind::Fill, 0, 100.0);
        rig.run(beat * 2, 256);
        rig.player.release(0, MoveKind::Fill);
        rig.run(beat - 200, 256);
        let seq: Vec<(u8, bool)> = rig.fired.iter().map(|f| (f.2.target, f.3)).collect();
        assert_eq!(seq, vec![(1, false), (1, false), (9, true), (9, true)]);
    }

    #[test]
    fn double_plays_the_bar_twice() {
        let bpm = 170.0;
        let mut rig = Rig::new(bpm);
        let main = [trigger(0, 1), trigger(96, 2), trigger(193, 3)];
        let mut gated = main;
        gated[1].gate = 48;
        rig.to_rt.push(packet(0, 1, &gated, &[])).ok().unwrap();
        rig.player.press(0, MoveKind::Double, 0, 0.0);
        rig.run(bar_frames(bpm) as u64, 256);
        let fpt = bar_frames(bpm) / TICKS_PER_BAR as f64;
        let got: Vec<(u64, u8)> = rig.fired.iter().map(|f| (f.0, f.2.target)).collect();
        let expect: Vec<(f64, u8)> =
            vec![(0.0, 1), (48.0, 2), (96.5, 3), (192.0, 1), (240.0, 2), (288.5, 3)];
        assert_eq!(got.len(), expect.len(), "{got:?}");
        for ((frame, target), (tick, want)) in got.iter().zip(&expect) {
            assert_eq!(target, want);
            assert!((*frame as f64 - (tick * fpt).ceil()).abs() <= 1.0, "{frame} vs tick {tick}");
        }
        assert_eq!(rig.fired[1].2.gate, 24, "gates are halved");
    }

    #[test]
    fn stutter_repeats_the_last_onset_on_the_grid() {
        let bpm = 170.0;
        let mut rig = Rig::new(bpm);
        let main = [trigger(0, 1), trigger(96, 2), trigger(192, 3)];
        rig.to_rt.push(packet(0, 1, &main, &[])).ok().unwrap();
        let fpt = bar_frames(bpm) / TICKS_PER_BAR as f64;
        // Press at tick 130: the last onset is the snare at tick 96.
        rig.run((130.0 * fpt) as u64, 256);
        rig.fired.clear();
        let pos = rig.clock.position();
        rig.player.press(0, MoveKind::Stutter, pos.bar, pos.tick);
        rig.run((48.0 * fpt) as u64, 256);
        rig.player.release(0, MoveKind::Stutter);
        let got: Vec<(u64, u8)> = rig.fired.iter().map(|f| (f.0, f.2.target)).collect();
        // Repeats on the 16th grid after the press: ticks 144 and 168.
        assert_eq!(got.len(), 2, "{got:?}");
        for ((frame, target), tick) in got.iter().zip([144.0, 168.0]) {
            assert_eq!(*target, 2);
            assert!((*frame as f64 - (tick * fpt).ceil()).abs() <= 1.0, "{frame} vs tick {tick}");
        }
        // After release the bar continues where the clock is.
        rig.fired.clear();
        rig.run((30.0 * fpt) as u64, 256);
        assert_eq!(rig.fired.iter().map(|f| f.2.target).collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn stutter_at_a_32nd_rolls_twice_per_step() {
        let bpm = 170.0;
        let mut rig = Rig::new(bpm);
        rig.to_rt.push(packet(0, 1, &[trigger(0, 5)], &[])).ok().unwrap();
        rig.player.set_roll_ticks(12);
        rig.block(64);
        rig.fired.clear();
        let pos = rig.clock.position();
        rig.player.press(0, MoveKind::Stutter, pos.bar, pos.tick);
        let fpt = bar_frames(bpm) / TICKS_PER_BAR as f64;
        rig.run((96.0 * fpt) as u64 - 64, 128);
        // Ticks 12, 24, ..., 84 (tick 0 passed before the press).
        assert_eq!(rig.fired.len(), 7, "{:?}", rig.fired.iter().map(|f| f.0).collect::<Vec<_>>());
    }

    #[test]
    fn void_fires_nothing() {
        let mut rig = Rig::new(170.0);
        rig.to_rt.push(packet(0, 1, &[trigger(0, 1), trigger(96, 1)], &[])).ok().unwrap();
        rig.player.press(0, MoveKind::Void, 0, 0.0);
        rig.run(bar_frames(170.0) as u64 / 2, 256);
        assert!(rig.fired.is_empty());
        assert!(rig.player.is_void(0));
    }
}
