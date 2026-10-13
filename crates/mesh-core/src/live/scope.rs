//! Per-lane scope ring (design §8).
//!
//! The RT thread writes one point per [`SCOPE_DECIMATION`] frames; the UI
//! copies the ring out without locking. Points are plain atomics, so a read
//! racing a write sees a mix of old and new points, which a scope tolerates.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::types::StereoSample;

/// Points kept per lane.
pub const SCOPE_POINTS: usize = 512;
/// Frames folded into one scope point. 512 points × 16 frames is about
/// 170 ms at 48 kHz: one beat at 170 BPM is 353 ms, so the scope shows a
/// single hit's shape.
// MUSICAL: scope time window, tune on the device screen.
pub const SCOPE_DECIMATION: usize = 16;

/// Lock-free ring of decimated mono samples.
pub struct ScopeRing {
    points: [AtomicU32; SCOPE_POINTS],
    /// Total points written (the next write goes to `written % SCOPE_POINTS`).
    written: AtomicUsize,
}

impl Default for ScopeRing {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeRing {
    pub fn new() -> Self {
        Self { points: std::array::from_fn(|_| AtomicU32::new(0)), written: AtomicUsize::new(0) }
    }

    /// Total points written since start.
    pub fn written(&self) -> usize {
        self.written.load(Ordering::Relaxed)
    }

    /// Copy the ring into `out`, oldest point first. Lock-free.
    pub fn snapshot(&self, out: &mut [f32; SCOPE_POINTS]) {
        let head = self.written.load(Ordering::Relaxed);
        for (i, o) in out.iter_mut().enumerate() {
            *o = f32::from_bits(self.points[(head + i) % SCOPE_POINTS].load(Ordering::Relaxed));
        }
    }

    fn push(&self, v: f32) {
        let w = self.written.load(Ordering::Relaxed);
        self.points[w % SCOPE_POINTS].store(v.to_bits(), Ordering::Relaxed);
        self.written.store(w.wrapping_add(1), Ordering::Relaxed);
    }
}

/// RT-side writer state: folds frames into points. One per lane, owned by
/// the engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScopeWriter {
    /// Sample with the largest magnitude in the current window (signed, so
    /// the scope keeps the waveform's polarity and its transients).
    held: f32,
    count: usize,
}

impl ScopeWriter {
    /// Feed a block (mono sum of both channels). RT-safe.
    pub fn write(&mut self, ring: &ScopeRing, block: &[StereoSample]) {
        for s in block {
            let m = 0.5 * (s.left + s.right);
            if m.abs() > self.held.abs() {
                self.held = m;
            }
            self.count += 1;
            if self.count >= SCOPE_DECIMATION {
                ring.push(self.held);
                self.held = 0.0;
                self.count = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimates_and_orders_oldest_first() {
        let ring = ScopeRing::new();
        let mut w = ScopeWriter::default();
        // Point k holds the value k + 1 (peak of its window).
        let total = SCOPE_POINTS + 10;
        let block: Vec<StereoSample> = (0..total * SCOPE_DECIMATION)
            .map(|i| {
                let peak = i % SCOPE_DECIMATION == 3;
                StereoSample::mono(if peak { (i / SCOPE_DECIMATION + 1) as f32 } else { 0.0 })
            })
            .collect();
        // Uneven block sizes must not change the result.
        for chunk in block.chunks(37) {
            w.write(&ring, chunk);
        }
        assert_eq!(ring.written(), total);
        let mut out = [0.0; SCOPE_POINTS];
        ring.snapshot(&mut out);
        assert_eq!(out[0], 11.0);
        assert_eq!(out[SCOPE_POINTS - 1], total as f32);
        assert!(out.windows(2).all(|p| p[1] == p[0] + 1.0));
    }

    #[test]
    fn keeps_polarity_of_the_peak() {
        let ring = ScopeRing::new();
        let mut w = ScopeWriter::default();
        let mut block = vec![StereoSample::mono(0.1); SCOPE_DECIMATION];
        block[5] = StereoSample::mono(-0.8);
        w.write(&ring, &block);
        let mut out = [0.0; SCOPE_POINTS];
        ring.snapshot(&mut out);
        assert_eq!(out[SCOPE_POINTS - 1], -0.8);
    }
}
