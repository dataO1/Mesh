//! Shared delay and reverb sends (design §5.4).
//!
//! Reuses the native [`DelayEffect`] and [`ReverbEffect`] unchanged through
//! the [`Effect`] trait, run 100 % wet on a send bus. Lanes add into the
//! buses with their own send levels; the returns are added to the mix.

use crate::effect::native::{beats_to_ms, DelayEffect, ReverbEffect};
use crate::effect::Effect;
use crate::types::{StereoBuffer, StereoSample};

use super::db_to_lin;
use super::params::{DelayParams, ReverbParams};

// Parameter indices / ranges of the reused native effects.
const DELAY_TIME: usize = 0;
const DELAY_FEEDBACK: usize = 1;
const DELAY_MIX: usize = 2;
const DELAY_PING_PONG: usize = 3;
const DELAY_MIN_MS: f32 = 10.0;
const DELAY_MAX_MS: f32 = 2000.0;
const DELAY_MAX_FEEDBACK: f32 = 0.95;
const REVERB_ROOM: usize = 0;
const REVERB_DAMPING: usize = 1;
const REVERB_WIDTH: usize = 2;
const REVERB_MIX: usize = 3;

pub struct SharedSends {
    delay: DelayEffect,
    reverb: ReverbEffect,
    delay_bus: StereoBuffer,
    reverb_bus: StereoBuffer,
    delay_params: DelayParams,
    delay_return: f32,
    reverb_return: f32,
    bpm: f64,
}

impl SharedSends {
    /// Pre-allocates both send buses for blocks up to `max_block` samples.
    pub fn new(max_block: usize) -> Self {
        let mut delay = DelayEffect::new();
        delay.set_param(DELAY_MIX, 1.0);
        let mut reverb = ReverbEffect::new();
        reverb.set_param(REVERB_MIX, 1.0);
        let mut s = Self {
            delay,
            reverb,
            delay_bus: StereoBuffer::silence(max_block),
            reverb_bus: StereoBuffer::silence(max_block),
            delay_params: DelayParams::default(),
            delay_return: 1.0,
            reverb_return: 1.0,
            bpm: 120.0,
        };
        s.configure(&DelayParams::default(), &ReverbParams::default());
        s
    }

    pub fn configure(&mut self, d: &DelayParams, r: &ReverbParams) {
        self.delay_params = *d;
        self.delay.set_param(DELAY_FEEDBACK, (d.feedback / DELAY_MAX_FEEDBACK).clamp(0.0, 1.0));
        self.delay.set_param(DELAY_PING_PONG, if d.ping_pong { 1.0 } else { 0.0 });
        self.delay_return = db_to_lin(d.return_db);
        self.reverb.set_param(REVERB_ROOM, r.room_size);
        self.reverb.set_param(REVERB_DAMPING, r.damping);
        self.reverb.set_param(REVERB_WIDTH, r.width);
        self.reverb_return = db_to_lin(r.return_db);
        self.update_delay_time();
    }

    pub fn set_bpm(&mut self, bpm: f64) {
        if bpm != self.bpm && bpm > 0.0 {
            self.bpm = bpm;
            self.update_delay_time();
        }
    }

    /// Delay time in ms at the current tempo, clamped to the delay line.
    pub fn delay_ms(&self) -> f32 {
        beats_to_ms(self.delay_params.beats, self.bpm).clamp(DELAY_MIN_MS, DELAY_MAX_MS)
    }

    fn update_delay_time(&mut self) {
        let norm = (self.delay_ms() - DELAY_MIN_MS) / (DELAY_MAX_MS - DELAY_MIN_MS);
        self.delay.set_param(DELAY_TIME, norm);
    }

    /// Start a block: size and clear both buses.
    pub fn begin(&mut self, len: usize) {
        for bus in [&mut self.delay_bus, &mut self.reverb_bus] {
            bus.set_len_from_capacity(len);
            bus.fill_silence();
        }
    }

    /// Add one lane into the buses at its send levels (linear).
    pub fn accumulate(&mut self, lane: &[StereoSample], delay_send: f32, reverb_send: f32) {
        if delay_send > 0.0 {
            for (d, s) in self.delay_bus.as_mut_slice().iter_mut().zip(lane) {
                *d += *s * delay_send;
            }
        }
        if reverb_send > 0.0 {
            for (d, s) in self.reverb_bus.as_mut_slice().iter_mut().zip(lane) {
                *d += *s * reverb_send;
            }
        }
    }

    /// Run both effects on their buses and add the returns into `out`.
    /// The effects always run so tails keep ringing after sends close.
    pub fn process_add(&mut self, out: &mut [StereoSample]) {
        self.delay.process(&mut self.delay_bus);
        self.reverb.process(&mut self.reverb_bus);
        let (dr, rr) = (self.delay_return, self.reverb_return);
        for ((o, d), r) in out.iter_mut().zip(self.delay_bus.iter()).zip(self.reverb_bus.iter()) {
            o.left += d.left * dr + r.left * rr;
            o.right += d.right * dr + r.right * rr;
        }
    }

    pub fn reset(&mut self) {
        self.delay.reset();
        self.reverb.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_time_matches_tempo() {
        for (bpm, beats) in [(174.0, 0.75), (120.0, 1.0), (85.0, 0.5)] {
            let block = 256;
            let mut s = SharedSends::new(block);
            s.configure(
                &DelayParams { beats, feedback: 0.0, ping_pong: false, return_db: 0.0 },
                &ReverbParams::default(),
            );
            s.set_bpm(bpm);
            let expected = (beats as f64 * 60.0 / bpm * 48_000.0) as usize;
            let mut first = None;
            let mut lane = vec![StereoSample::default(); block];
            for b in 0..(expected / block + 2) {
                lane.fill(StereoSample::default());
                if b == 0 {
                    lane[0] = StereoSample::new(1.0, 1.0);
                }
                s.begin(block);
                s.accumulate(&lane, 1.0, 0.0);
                let mut out = vec![StereoSample::default(); block];
                s.process_add(&mut out);
                if first.is_none() {
                    if let Some(i) = out.iter().position(|x| x.left.abs() > 0.5) {
                        first = Some(b * block + i);
                    }
                }
            }
            let got = first.expect("echo must arrive");
            assert!(
                (got as i64 - expected as i64).abs() <= 1,
                "{bpm} BPM × {beats} beats: echo at {got}, expected {expected}"
            );
        }
    }

    #[test]
    fn closed_sends_return_silence() {
        let mut s = SharedSends::new(128);
        s.begin(128);
        s.accumulate(&vec![StereoSample::new(0.5, 0.5); 128], 0.0, 0.0);
        let mut out = vec![StereoSample::default(); 128];
        s.process_add(&mut out);
        assert!(out.iter().all(|x| x.left == 0.0 && x.right == 0.0));
    }
}
