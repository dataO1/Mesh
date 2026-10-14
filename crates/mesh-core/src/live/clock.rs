//! Live mode master clock (design §4.3).
//!
//! `LiveClock` owns tempo and the sample-accurate musical position. The JACK
//! callback drives it once per block:
//!
//! ```ignore
//! for seg in clock.segments(n_frames) {
//!     if seg.boundary >= Boundary::Bar { /* swap in the next rendered bar */ }
//!     for ev in bar.as_slice() {
//!         if let Some(offset) = seg.offset_for_tick(ev.tick) { fire(ev, offset) }
//!     }
//! }
//! ```
//!
//! # Timing model
//!
//! Time advances one 16th step at a time. Tempo is constant within a step and
//! may only change at step boundaries, so a tempo ramp is a staircase with one
//! tread per step (at 170 BPM a step is ~88 ms and a 2-BPM ramp over 4 bars
//! moves 0.03 BPM per step, far below audibility). Because tempo is constant
//! inside a step, tick → sample conversion within a step is exact.
//!
//! A step starts at a real-valued sample time. Frame `F` (integer) belongs to
//! the step whose real start is `<= F`. An event at tick `t` fires on the first
//! frame at or after its real time. Each segment covers one step (or the part
//! of one step inside the current block), so every event is owned by exactly
//! one segment.
//!
//! RT-safety: no allocation, no locks, no panics on the advance path.

use super::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// Lowest tempo the clock accepts.
pub const MIN_BPM: f64 = 40.0;
/// Highest tempo the clock accepts.
pub const MAX_BPM: f64 = 300.0;
/// Bars per phrase. Harmony and phrase shaping use 4-bar phrases whatever
/// each lane's loop length.
pub const BARS_PER_PHRASE: u64 = 4;

const TICKS_PER_BEAT: f64 = TICKS_PER_BAR as f64 / 4.0;

/// Tolerance when rounding real-valued times to frames. A time within this
/// distance after a frame counts as that frame, so float noise on exact
/// boundaries never pushes an event or a step start one frame late.
const FRAME_EPS: f64 = 1e-6;

/// Musical boundary crossed at the start of a segment. Ordered: a phrase
/// boundary is also a bar boundary, a bar boundary is also a step boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Boundary {
    /// Segment continues a step that started in an earlier block.
    None,
    /// Segment starts on a 16th step.
    Step,
    /// Segment starts on a bar.
    Bar,
    /// Segment starts on a phrase (every [`BARS_PER_PHRASE`] bars).
    Phrase,
}

/// A contiguous run of frames inside one step, at one tempo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockSegment {
    /// Offset of the first frame within the block.
    pub offset: u32,
    /// Number of frames in this segment (> 0).
    pub frames: u32,
    /// Absolute bar index.
    pub bar: u64,
    /// Step within the bar, 0..STEPS_PER_BAR.
    pub step: u8,
    /// Boundary that falls on this segment's first frame.
    pub boundary: Boundary,
    /// Tempo during this step.
    pub bpm: f64,
    /// Length of this step in (real-valued) frames.
    pub step_frames: f64,
    /// Real distance in frames from the step's start to the step's first
    /// frame, in `[0, 1)`. Constant for all segments of one step.
    pub step_phase: f64,
    /// Whole frames of this step processed before this segment.
    pub frame_in_step: u32,
    /// True when this segment runs to the end of its step.
    pub ends_step: bool,
}

impl ClockSegment {
    /// Phrase index of this segment's bar.
    pub fn phrase(&self) -> u64 {
        self.bar / BARS_PER_PHRASE
    }

    /// Bar within the phrase, 0..BARS_PER_PHRASE.
    pub fn bar_in_phrase(&self) -> u8 {
        (self.bar % BARS_PER_PHRASE) as u8
    }

    /// Frames per tick at this segment's tempo.
    pub fn frames_per_tick(&self) -> f64 {
        self.step_frames / TICKS_PER_STEP as f64
    }

    /// First tick (integer) owned by this segment, inclusive.
    pub fn tick_start(&self) -> u16 {
        let base = self.step as u16 * TICKS_PER_STEP;
        // Smallest sub-tick s with ceil(s*fpt - x) >= 0, i.e. s*fpt > x - 1.
        let mut sub = 0u16;
        while sub < TICKS_PER_STEP && self.sub_offset(sub) < 0 {
            sub += 1;
        }
        base + sub
    }

    /// Fractional tick position of the segment's first frame within the bar.
    pub fn tick_pos(&self) -> f64 {
        self.step as f64 * TICKS_PER_STEP as f64 + self.frames_into_step() / self.frames_per_tick()
    }

    /// Real distance in frames from the step start to this segment's first frame.
    pub fn frames_into_step(&self) -> f64 {
        self.step_phase + self.frame_in_step as f64
    }

    /// Frame offset within the block at which an event at `tick` fires, or
    /// `None` when the event is not owned by this segment.
    pub fn offset_for_tick(&self, tick: u16) -> Option<u32> {
        if tick >= TICKS_PER_BAR || tick / TICKS_PER_STEP != self.step as u16 {
            return None;
        }
        let j = self.sub_offset(tick % TICKS_PER_STEP);
        if j < 0 {
            return None;
        }
        let j = if j >= self.frames as i64 {
            // An event less than one frame before the next step lands on the
            // next step's first frame; keep it in its own step instead.
            if self.ends_step { self.frames as i64 - 1 } else { return None }
        } else {
            j
        };
        Some(self.offset + j as u32)
    }

    /// Offset in frames (relative to this segment's first frame) of sub-tick
    /// `sub` within the step. Negative when it fell before this segment.
    ///
    /// The frame index within the step is computed from the step's fixed phase
    /// only, then shifted by an integer, so every segment of a step agrees on
    /// it exactly and each event is owned by one segment.
    fn sub_offset(&self, sub: u16) -> i64 {
        let t = sub as f64 * self.frames_per_tick() - self.step_phase;
        // Guard against float noise right on a frame (e.g. 2.0000000001).
        let frame_in_step = (t - FRAME_EPS).ceil() as i64;
        frame_in_step - self.frame_in_step as i64
    }
}

/// Snapshot of the clock position, for UI and the bar renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockPosition {
    pub bar: u64,
    pub step: u8,
    /// Fractional tick within the bar, `0.0..TICKS_PER_BAR`.
    pub tick: f64,
    pub phrase: u64,
    pub bar_in_phrase: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TempoRamp {
    from: f64,
    to: f64,
    /// Total steps in the ramp (>= 1).
    steps: u32,
    /// Steps already started.
    done: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PendingRamp {
    target: f64,
    bars: u32,
}

/// Sample-accurate master clock.
#[derive(Debug, Clone)]
pub struct LiveClock {
    sample_rate: f64,
    /// Tempo of the current step (or of the next step when not yet started).
    bpm: f64,
    bar: u64,
    step: u8,
    /// Real frames from the current step's start to its first frame, `[0, 1)`.
    step_phase: f64,
    /// Whole frames of the current step already processed.
    frame_in_step: u32,
    /// Length of the current step in frames; valid when `step_started`.
    step_frames: f64,
    /// False until the first frame of the current step is processed. Tempo
    /// decisions for a step are taken lazily at that point, so requests made
    /// right at a boundary still land on it.
    step_started: bool,
    pending_ramp: Option<PendingRamp>,
    ramp: Option<TempoRamp>,
    pending_bpm: Option<f64>,
    half_time: bool,
}

impl LiveClock {
    pub fn new(sample_rate: u32, bpm: f64) -> Self {
        Self {
            sample_rate: sample_rate.max(1) as f64,
            bpm: clamp_bpm(bpm),
            bar: 0,
            step: 0,
            step_phase: 0.0,
            frame_in_step: 0,
            step_frames: 0.0,
            step_started: false,
            pending_ramp: None,
            ramp: None,
            pending_bpm: None,
            half_time: false,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate as u32
    }

    /// Change the sample rate (JACK restart). Keeps the musical position.
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        let new = sample_rate.max(1) as f64;
        if self.step_started {
            let ratio = new / self.sample_rate;
            // Keep the fractional position; restart the step's frame grid.
            let pos = (self.step_phase + self.frame_in_step as f64) * ratio;
            self.step_frames *= ratio;
            self.frame_in_step = pos.floor() as u32;
            self.step_phase = pos - pos.floor();
        }
        self.sample_rate = new;
    }

    /// Current tempo (of the step being played).
    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Tempo the clock is heading to: ramp target, pending target or current.
    pub fn target_bpm(&self) -> f64 {
        if let Some(p) = self.pending_ramp {
            p.target
        } else if let Some(b) = self.pending_bpm {
            b
        } else if let Some(r) = self.ramp {
            r.to
        } else {
            self.bpm
        }
    }

    /// True while a ramp is pending or running.
    pub fn is_ramping(&self) -> bool {
        self.pending_ramp.is_some() || self.ramp.is_some()
    }

    /// Request a bar-quantized tempo ramp: it starts at the next bar boundary
    /// and reaches `target_bpm` linearly over `bars` bars (`0` = jump at the
    /// bar). A new request replaces any pending one; a running ramp keeps
    /// running until the new one starts, which then ramps from wherever the
    /// tempo is.
    pub fn request_tempo(&mut self, target_bpm: f64, bars: u32) {
        self.pending_bpm = None;
        self.pending_ramp = Some(PendingRamp { target: clamp_bpm(target_bpm), bars });
    }

    /// Set the tempo from the next step on, cancelling ramps (startup, reset).
    pub fn set_tempo_immediate(&mut self, bpm: f64) {
        self.pending_ramp = None;
        self.ramp = None;
        if self.step_started {
            self.pending_bpm = Some(clamp_bpm(bpm));
        } else {
            self.bpm = clamp_bpm(bpm);
        }
    }

    /// Half-time is a generator context flag, not a clock change (design §4.3).
    pub fn half_time(&self) -> bool {
        self.half_time
    }

    pub fn set_half_time(&mut self, on: bool) {
        self.half_time = on;
    }

    /// Jump to the start of `bar` (transport reset). Keeps tempo state.
    pub fn seek_to_bar(&mut self, bar: u64) {
        self.bar = bar;
        self.step = 0;
        self.step_phase = 0.0;
        self.frame_in_step = 0;
        self.step_started = false;
    }

    /// Position of the next frame to be processed.
    pub fn position(&self) -> ClockPosition {
        let frac = if self.step_started && self.step_frames > 0.0 {
            ((self.step_phase + self.frame_in_step as f64) / self.step_frames).min(1.0)
        } else {
            0.0
        };
        ClockPosition {
            bar: self.bar,
            step: self.step,
            tick: (self.step as f64 + frac) * TICKS_PER_STEP as f64,
            phrase: self.bar / BARS_PER_PHRASE,
            bar_in_phrase: (self.bar % BARS_PER_PHRASE) as u8,
        }
    }

    /// Frames in one step at `bpm`.
    pub fn step_frames_at(&self, bpm: f64) -> f64 {
        self.sample_rate * 60.0 / bpm / (STEPS_PER_BAR as f64 / 4.0)
    }

    /// Convert a tick count to frames at the current tempo (gate lengths).
    pub fn ticks_to_frames(&self, ticks: f64) -> f64 {
        ticks * self.sample_rate * 60.0 / (self.bpm * TICKS_PER_BEAT)
    }

    /// Iterate this block's segments. Each segment lies within one step at one
    /// tempo. The clock advances as the iterator is consumed; dropping the
    /// iterator early leaves the remaining frames unprocessed.
    pub fn segments(&mut self, frames: u32) -> Segments<'_> {
        Segments { clock: self, offset: 0, remaining: frames }
    }

    /// Advance by `frames` without inspecting segments.
    pub fn advance(&mut self, frames: u32) {
        for _ in self.segments(frames) {}
    }

    /// Take the tempo decisions for the step about to start.
    fn begin_step(&mut self) {
        if self.step == 0 {
            if let Some(p) = self.pending_ramp.take() {
                self.pending_bpm = None;
                if p.bars == 0 {
                    self.ramp = None;
                    self.bpm = p.target;
                } else {
                    self.ramp = Some(TempoRamp {
                        from: self.bpm,
                        to: p.target,
                        steps: p.bars.saturating_mul(STEPS_PER_BAR as u32),
                        done: 0,
                    });
                }
            }
        }
        if let Some(b) = self.pending_bpm.take() {
            self.bpm = b;
        }
        if let Some(r) = self.ramp.as_mut() {
            r.done += 1;
            // MUSICAL: linear in BPM, so step `steps` (the last) plays at target.
            let t = r.done as f64 / r.steps as f64;
            self.bpm = r.from + (r.to - r.from) * t;
            if r.done >= r.steps {
                self.bpm = r.to;
                self.ramp = None;
            }
        }
        self.step_frames = self.step_frames_at(self.bpm);
        self.step_started = true;
    }

    fn next_segment(&mut self, offset: u32, remaining: u32) -> Option<ClockSegment> {
        if remaining == 0 {
            return None;
        }
        let boundary = if self.step_started {
            Boundary::None
        } else {
            self.begin_step();
            if self.step != 0 {
                Boundary::Step
            } else if self.bar % BARS_PER_PHRASE == 0 {
                Boundary::Phrase
            } else {
                Boundary::Bar
            }
        };
        // The step owns frames k >= 0 with phase + k < step_frames.
        let total = (self.step_frames - self.step_phase - FRAME_EPS).ceil().max(1.0);
        let total = if total >= u32::MAX as f64 { u32::MAX } else { total as u32 };
        let left = total.saturating_sub(self.frame_in_step).max(1);
        let frames = remaining.min(left);
        let seg = ClockSegment {
            offset,
            frames,
            bar: self.bar,
            step: self.step,
            boundary,
            bpm: self.bpm,
            step_frames: self.step_frames,
            step_phase: self.step_phase,
            frame_in_step: self.frame_in_step,
            ends_step: frames == left,
        };
        self.frame_in_step += frames;
        if frames == left {
            // Carry the sub-frame remainder into the next step's phase.
            let end = self.step_phase + self.frame_in_step as f64;
            self.step_phase = (end - self.step_frames).clamp(0.0, 1.0 - FRAME_EPS);
            self.frame_in_step = 0;
            self.step_started = false;
            self.step += 1;
            if self.step as usize >= STEPS_PER_BAR {
                self.step = 0;
                self.bar += 1;
            }
        }
        Some(seg)
    }
}

/// Iterator over one block's [`ClockSegment`]s. See [`LiveClock::segments`].
pub struct Segments<'a> {
    clock: &'a mut LiveClock,
    offset: u32,
    remaining: u32,
}

impl Iterator for Segments<'_> {
    type Item = ClockSegment;

    fn next(&mut self) -> Option<ClockSegment> {
        let seg = self.clock.next_segment(self.offset, self.remaining)?;
        self.offset += seg.frames;
        self.remaining -= seg.frames;
        Some(seg)
    }
}

fn clamp_bpm(bpm: f64) -> f64 {
    if bpm.is_finite() { bpm.clamp(MIN_BPM, MAX_BPM) } else { MIN_BPM }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn bar_frames(bpm: f64) -> f64 {
        SR as f64 * 60.0 / bpm * 4.0
    }

    /// Run `blocks` blocks of `block` frames, collecting segments with their
    /// absolute frame position.
    fn run(clock: &mut LiveClock, block: u32, blocks: usize) -> Vec<(u64, ClockSegment)> {
        let mut out = Vec::new();
        let mut abs = 0u64;
        for _ in 0..blocks {
            for seg in clock.segments(block) {
                out.push((abs + seg.offset as u64, seg));
            }
            abs += block as u64;
        }
        out
    }

    #[test]
    fn first_segment_is_phrase_boundary() {
        let mut c = LiveClock::new(SR, 172.0);
        let seg = c.segments(256).next().unwrap();
        assert_eq!(seg.boundary, Boundary::Phrase);
        assert_eq!((seg.bar, seg.step, seg.offset), (0, 0, 0));
    }

    #[test]
    fn segments_cover_block_exactly() {
        let mut c = LiveClock::new(SR, 173.3);
        for block in [1u32, 64, 255, 1024, 4096] {
            let mut sum = 0;
            let mut next_off = 0;
            for seg in c.segments(block) {
                assert_eq!(seg.offset, next_off);
                assert!(seg.frames > 0);
                next_off += seg.frames;
                sum += seg.frames;
            }
            assert_eq!(sum, block);
        }
    }

    #[test]
    fn bar_boundaries_land_on_expected_frames() {
        let bpm = 170.0;
        let mut c = LiveClock::new(SR, bpm);
        let segs = run(&mut c, 512, 2000);
        let bars: Vec<(u64, u64)> = segs
            .iter()
            .filter(|(_, s)| s.boundary >= Boundary::Bar)
            .map(|(abs, s)| (*abs, s.bar))
            .collect();
        assert!(bars.len() > 10);
        for (abs, bar) in bars {
            let ideal = (bar as f64 * bar_frames(bpm)).ceil() as u64;
            assert!(abs.abs_diff(ideal) <= 1, "bar {bar} at {abs}, ideal {ideal}");
        }
    }

    #[test]
    fn phrase_flags_every_fourth_bar() {
        let mut c = LiveClock::new(SR, 174.0);
        for (_, s) in run(&mut c, 1024, 1500) {
            match s.boundary {
                Boundary::Phrase => assert_eq!((s.bar % 4, s.step), (0, 0)),
                Boundary::Bar => assert!(s.bar % 4 != 0 && s.step == 0),
                Boundary::Step => assert!(s.step != 0),
                Boundary::None => {}
            }
        }
    }

    #[test]
    fn block_size_does_not_change_timing() {
        // Event onsets must be identical whatever the block size.
        let ticks: Vec<u16> = (0..TICKS_PER_BAR).step_by(7).collect();
        let onsets = |block: u32| {
            let mut c = LiveClock::new(SR, 171.5);
            let mut out = Vec::new();
            let mut abs = 0u64;
            while abs < bar_frames(171.5) as u64 * 3 {
                for seg in c.segments(block) {
                    for &t in &ticks {
                        if let Some(o) = seg.offset_for_tick(t) {
                            out.push((seg.bar, t, abs + o as u64));
                        }
                    }
                }
                abs += block as u64;
            }
            out
        };
        let a = onsets(64);
        let b = onsets(1000);
        let n = a.len().min(b.len());
        assert!(n > 100);
        assert_eq!(a[..n], b[..n]);
    }

    #[test]
    fn every_tick_fires_exactly_once_at_ideal_frame() {
        let bpm = 165.0;
        let mut c = LiveClock::new(SR, bpm);
        let mut fired = vec![0u32; TICKS_PER_BAR as usize * 2];
        let mut abs = 0u64;
        while abs < (bar_frames(bpm) * 2.0) as u64 + 333 {
            for seg in c.segments(333) {
                if seg.bar >= 2 {
                    continue;
                }
                for t in 0..TICKS_PER_BAR {
                    if let Some(o) = seg.offset_for_tick(t) {
                        let idx = seg.bar as usize * TICKS_PER_BAR as usize + t as usize;
                        fired[idx] += 1;
                        let ideal = ((seg.bar as f64 * TICKS_PER_BAR as f64 + t as f64)
                            * bar_frames(bpm)
                            / TICKS_PER_BAR as f64)
                            .ceil();
                        let got = (abs + o as u64) as f64;
                        assert!((got - ideal).abs() <= 1.0, "tick {t}: {got} vs {ideal}");
                    }
                }
            }
            abs += 333;
        }
        assert!(fired.iter().all(|&n| n == 1), "{fired:?}");
    }

    #[test]
    fn exactly_once_sweep_with_ramps() {
        for &bpm in &[87.3, 140.0, 165.0, 170.0, 172.5, 174.0, 299.0] {
            for &block in &[32u32, 64, 128, 333, 1024] {
                for &sr in &[44_100u32, 48_000] {
                    let mut c = LiveClock::new(sr, bpm);
                    c.request_tempo(bpm * 1.03, 2);
                    let bars = 6u64;
                    let mut fired = vec![0u8; (bars * TICKS_PER_BAR as u64) as usize];
                    while c.position().bar < bars + 1 {
                        for seg in c.segments(block) {
                            if seg.bar >= bars {
                                continue;
                            }
                            for t in seg.tick_start()..TICKS_PER_BAR {
                                if let Some(o) = seg.offset_for_tick(t) {
                                    assert!(o >= seg.offset && o < seg.offset + seg.frames);
                                    fired[(seg.bar * TICKS_PER_BAR as u64 + t as u64) as usize] += 1;
                                }
                            }
                        }
                    }
                    assert!(
                        fired.iter().all(|&n| n == 1),
                        "bpm {bpm} block {block} sr {sr}: {:?}",
                        fired.iter().position(|&n| n != 1)
                    );
                }
            }
        }
    }

    #[test]
    fn foreign_ticks_are_rejected() {
        let mut c = LiveClock::new(SR, 170.0);
        let seg = c.segments(64).next().unwrap();
        assert_eq!(seg.offset_for_tick(TICKS_PER_STEP), None);
        assert_eq!(seg.offset_for_tick(TICKS_PER_BAR), None);
        assert_eq!(seg.offset_for_tick(0), Some(0));
        assert_eq!(seg.tick_start(), 0);
    }

    #[test]
    fn tempo_request_waits_for_next_bar() {
        let mut c = LiveClock::new(SR, 170.0);
        c.advance(1000); // mid bar 0
        c.request_tempo(180.0, 0);
        let segs = run(&mut c, 256, 2000);
        for (_, s) in &segs {
            if s.bar == 0 {
                assert_eq!(s.bpm, 170.0);
            } else {
                assert_eq!(s.bpm, 180.0);
            }
        }
        assert_eq!(c.target_bpm(), 180.0);
        assert!(!c.is_ramping());
    }

    #[test]
    fn request_at_exact_bar_start_applies_to_that_bar() {
        let mut c = LiveClock::new(SR, 170.0);
        c.request_tempo(150.0, 0);
        let seg = c.segments(10).next().unwrap();
        assert_eq!(seg.bpm, 150.0);
    }

    #[test]
    fn ramp_is_linear_monotonic_and_bar_quantized() {
        let mut c = LiveClock::new(SR, 170.0);
        c.advance(5000);
        c.request_tempo(174.0, 2);
        assert!(c.is_ramping());
        let segs = run(&mut c, 512, 1200);
        let mut per_step: Vec<(u64, u8, f64)> = Vec::new();
        for (_, s) in &segs {
            if s.boundary != Boundary::None {
                per_step.push((s.bar, s.step, s.bpm));
            }
        }
        // Bar 0: unchanged. Bars 1-2: ramp. Bar 3+: target.
        let mut prev = 170.0;
        for &(bar, step, bpm) in &per_step {
            match bar {
                0 => assert_eq!(bpm, 170.0),
                1 | 2 => {
                    let k = (bar - 1) * 16 + step as u64 + 1;
                    let expect = 170.0 + 4.0 * k as f64 / 32.0;
                    assert!((bpm - expect).abs() < 1e-9, "bar {bar} step {step}: {bpm}");
                    assert!(bpm > prev);
                }
                _ => assert_eq!(bpm, 174.0),
            }
            prev = bpm;
        }
        assert!(!c.is_ramping());
        assert_eq!(c.bpm(), 174.0);
    }

    #[test]
    fn ramp_bar_lengths_match_integrated_tempo() {
        // Frames between bar boundaries = sum of per-step lengths at step tempo.
        let mut c = LiveClock::new(SR, 160.0);
        c.request_tempo(176.0, 4);
        let segs = run(&mut c, 300, 3000);
        let mut starts: Vec<u64> = Vec::new();
        let mut step_lens: Vec<Vec<f64>> = vec![Vec::new(); 8];
        for (abs, s) in &segs {
            if s.boundary >= Boundary::Bar {
                starts.push(*abs);
            }
            if s.boundary != Boundary::None && (s.bar as usize) < 8 {
                step_lens[s.bar as usize].push(s.step_frames);
            }
        }
        let mut acc = 0.0f64;
        for bar in 0..6usize {
            acc += step_lens[bar].iter().sum::<f64>();
            let got = starts[bar + 1] as f64;
            assert!((got - acc.ceil()).abs() <= 1.0, "bar {}: {got} vs {}", bar + 1, acc);
        }
    }

    #[test]
    fn tick_to_offset_follows_tempo_changes() {
        // Same tick, two tempos: offset from the bar start scales with 1/bpm.
        let measure = |bpm: f64| {
            let mut c = LiveClock::new(SR, bpm);
            let mut abs = 0u64;
            let mut bar_start = None;
            let mut onset = None;
            while onset.is_none() {
                for seg in c.segments(128) {
                    if seg.bar == 1 && seg.boundary == Boundary::Bar {
                        bar_start = Some(abs + seg.offset as u64);
                    }
                    if seg.bar == 1 {
                        if let Some(o) = seg.offset_for_tick(200) {
                            onset = Some(abs + o as u64);
                        }
                    }
                }
                abs += 128;
            }
            (onset.unwrap() - bar_start.unwrap()) as f64
        };
        let a = measure(170.0);
        let b = measure(85.0);
        assert!((b / a - 2.0).abs() < 0.001, "{a} {b}");
        let ideal = 200.0 / TICKS_PER_BAR as f64 * bar_frames(170.0);
        assert!((a - ideal).abs() <= 1.5);
    }

    #[test]
    fn new_request_replaces_pending() {
        let mut c = LiveClock::new(SR, 170.0);
        c.advance(100);
        c.request_tempo(180.0, 4);
        c.request_tempo(160.0, 0);
        assert_eq!(c.target_bpm(), 160.0);
        c.advance(bar_frames(170.0) as u32 + 10);
        assert_eq!(c.bpm(), 160.0);
    }

    #[test]
    fn immediate_tempo_applies_next_step() {
        let mut c = LiveClock::new(SR, 170.0);
        c.advance(100);
        c.request_tempo(100.0, 8);
        c.set_tempo_immediate(120.0);
        assert!(!c.is_ramping());
        let segs: Vec<_> = c.segments(10_000).collect();
        assert_eq!(segs[0].bpm, 170.0);
        assert!(segs[1..].iter().all(|s| s.bpm == 120.0));
    }

    #[test]
    fn bpm_is_clamped() {
        let mut c = LiveClock::new(SR, 10.0);
        assert_eq!(c.bpm(), MIN_BPM);
        c.request_tempo(1000.0, 0);
        assert_eq!(c.target_bpm(), MAX_BPM);
        c.request_tempo(f64::NAN, 0);
        assert_eq!(c.target_bpm(), MIN_BPM);
    }

    #[test]
    fn half_time_is_only_a_flag() {
        let mut a = LiveClock::new(SR, 172.0);
        let mut b = LiveClock::new(SR, 172.0);
        b.set_half_time(true);
        assert!(b.half_time());
        let sa: Vec<_> = a.segments(20_000).collect();
        let sb: Vec<_> = b.segments(20_000).collect();
        assert_eq!(sa, sb);
    }

    #[test]
    fn position_and_seek() {
        let mut c = LiveClock::new(SR, 120.0);
        // 120 BPM at 48k: 1 bar = 96000 frames, 1 step = 6000.
        c.advance(96_000 * 5 + 6000 * 3 + 3000);
        let p = c.position();
        assert_eq!((p.bar, p.step, p.phrase, p.bar_in_phrase), (5, 3, 1, 1));
        assert!((p.tick - (3.5 * TICKS_PER_STEP as f64)).abs() < 1e-9);
        c.seek_to_bar(8);
        let seg = c.segments(16).next().unwrap();
        assert_eq!((seg.bar, seg.boundary), (8, Boundary::Phrase));
        assert_eq!(seg.phrase(), 2);
    }

    #[test]
    fn sample_rate_change_keeps_position() {
        let mut c = LiveClock::new(48_000, 120.0);
        c.advance(3000);
        let before = c.position();
        c.set_sample_rate(96_000);
        let after = c.position();
        assert!((before.tick - after.tick).abs() < 1e-9);
        assert_eq!(c.step_frames_at(120.0), 12_000.0);
    }

    #[test]
    fn ticks_to_frames_matches_step_length() {
        let c = LiveClock::new(SR, 170.0);
        let step = c.step_frames_at(170.0);
        assert!((c.ticks_to_frames(TICKS_PER_STEP as f64) - step).abs() < 1e-9);
    }

    #[test]
    fn segment_tick_helpers() {
        let mut c = LiveClock::new(SR, 120.0);
        c.advance(6000 * 2 + 1500); // step 2, a quarter in
        let seg = c.segments(64).next().unwrap();
        assert_eq!(seg.boundary, Boundary::None);
        assert!((seg.tick_pos() - (2.25 * TICKS_PER_STEP as f64)).abs() < 1e-9);
        // 1500 frames in = 6 ticks at 250 frames/tick: tick 48+6 fires at 0.
        assert_eq!(seg.tick_start(), 2 * TICKS_PER_STEP + 6);
        assert_eq!(seg.offset_for_tick(2 * TICKS_PER_STEP + 6), Some(0));
        assert_eq!(seg.offset_for_tick(2 * TICKS_PER_STEP + 5), None);
    }
}
