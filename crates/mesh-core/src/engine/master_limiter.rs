//! Master lookahead limiter — transparent feed-forward true-peak limiting
//!
//! Placed before the safety clipper in the master signal chain:
//!   master volume → **limiter** → clipper → output
//!
//! Uses a 1.5 ms lookahead to anticipate peaks and smoothly reduce gain
//! *before* they arrive at the output. The result is transparent limiting
//! with no audible pumping or harmonic distortion — only gain changes.
//!
//! # Algorithm
//!
//! 1. Each input sample is written to a ring-buffer delay line.
//! 2. The stereo peak is compared against the threshold; a per-sample
//!    "target gain" is stored in a parallel ring buffer.
//! 3. A sliding-window minimum over the lookahead window finds the lowest
//!    target gain in the upcoming audio — this is the gain we must reach
//!    by the time that peak arrives at the output.
//! 4. An exponential envelope follower smooths the gain:
//!    - **Attack**: converges 99 % within the lookahead period so gain
//!      reduction is fully applied before the peak exits the delay.
//!    - **Release**: 100 ms time-constant for smooth, pump-free recovery.
//! 5. The delayed audio is scaled by the smoothed gain and output.
//!
//! # Performance
//!
//! The per-sample cost is dominated by the sliding-window min scan
//! (72 iterations at 48 kHz). At ~3.5 M comparisons/s this is negligible.
//! No heap allocation occurs during processing.

use crate::types::{StereoBuffer, SAMPLE_RATE};

// ═══════════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════════

/// Maximum ring-buffer size (supports up to ~5 ms at 192 kHz).
const MAX_DELAY: usize = 1024;

/// Lookahead time in seconds (1.5 ms — 72 samples at 48 kHz).
const LOOKAHEAD_SECS: f32 = 0.0015;

/// Release time-constant in seconds.
/// 100 ms gives smooth recovery without audible pumping.
const RELEASE_SECS: f32 = 0.1;

// ═══════════════════════════════════════════════════════════════════════════════
// Limiter
// ═══════════════════════════════════════════════════════════════════════════════

/// Transparent feed-forward lookahead limiter.
///
/// The limiter only ever *reduces* gain — it never boosts. When the input
/// is below the threshold the output is bit-identical to the (delayed) input.
pub struct MasterLimiter {
    /// Threshold in linear amplitude
    threshold: f32,
    /// Lookahead in samples (derived from LOOKAHEAD_SECS × SAMPLE_RATE)
    lookahead: usize,

    // — Ring buffers (fixed-size, no heap allocation) ————————————————————————

    /// Stereo audio delay line: `delay[channel][position]`
    delay: [[f32; MAX_DELAY]; 2],
    /// Per-sample target gain (threshold / peak, or 1.0 when below threshold)
    target_gains: [f32; MAX_DELAY],
    /// Shared write cursor for all ring buffers
    write_pos: usize,

    // — Envelope follower ————————————————————————————————————————————————————

    /// Current smoothed gain applied to the output (1.0 = unity)
    gain: f32,
    /// Attack coefficient: `coeff^lookahead ≈ 0.01` (99 % convergence)
    attack_coeff: f32,
    /// Release coefficient: exponential decay with `RELEASE_SECS` τ
    release_coeff: f32,
}

impl MasterLimiter {
    /// Create a limiter with the default threshold of −0.3 dBFS
    /// (matches the clipper ceiling so the limiter does all the heavy
    /// lifting and the clipper only catches edge-case residuals).
    pub fn new() -> Self {
        Self::with_threshold_db(-0.3)
    }

    /// Create a limiter with a custom threshold in dBFS.
    pub fn with_threshold_db(db: f32) -> Self {
        let threshold = 10.0_f32.powf(db / 20.0);

        let lookahead = (LOOKAHEAD_SECS * SAMPLE_RATE as f32).round() as usize;
        let lookahead = lookahead.clamp(1, MAX_DELAY);

        // Attack: 99 % convergence in `lookahead` samples.
        //   coeff^N = 0.01  →  coeff = exp(ln 0.01 / N)
        let attack_coeff = (-4.605_17 / lookahead as f32).exp();

        // Release: first-order exponential with RELEASE_SECS time-constant.
        //   coeff = exp(-1 / (τ × fs))
        let release_coeff = (-1.0 / (RELEASE_SECS * SAMPLE_RATE as f32)).exp();

        Self {
            threshold,
            lookahead,
            delay: [[0.0; MAX_DELAY]; 2],
            target_gains: [1.0; MAX_DELAY],
            write_pos: 0,
            gain: 1.0,
            attack_coeff,
            release_coeff,
        }
    }

    /// Latency in samples introduced by this limiter.
    pub fn latency_samples(&self) -> usize {
        self.lookahead
    }

    /// Process a stereo buffer in-place.
    ///
    /// Each sample is delayed by `lookahead` samples and scaled by the
    /// smoothed gain envelope. The gain is always ≤ 1.0.
    pub fn process(&mut self, buffer: &mut StereoBuffer) {
        for sample in buffer.iter_mut() {
            // ── 1. Peak detection on the *input* (before delay) ──────────
            let peak = sample.left.abs().max(sample.right.abs());

            let target = if peak > self.threshold {
                self.threshold / peak
            } else {
                1.0
            };

            // ── 2. Store target gain in the ring buffer ──────────────────
            self.target_gains[self.write_pos] = target;

            // ── 3. Sliding-window minimum over the lookahead window ──────
            //    This tells us the worst-case gain we'll need before the
            //    corresponding audio sample exits the delay line.
            let min_gain = self.window_min_gain();

            // ── 4. Smooth the gain envelope ──────────────────────────────
            if min_gain < self.gain {
                // Attack: fast convergence toward the target
                self.gain = self.gain * self.attack_coeff
                    + min_gain * (1.0 - self.attack_coeff);
            } else {
                // Release: slow return toward unity
                self.gain = self.gain * self.release_coeff
                    + min_gain * (1.0 - self.release_coeff);
            }

            // ── 5. Read delayed audio ────────────────────────────────────
            let read_pos =
                (self.write_pos + MAX_DELAY - self.lookahead) % MAX_DELAY;
            let out_left = self.delay[0][read_pos] * self.gain;
            let out_right = self.delay[1][read_pos] * self.gain;

            // ── 6. Write current input into the delay line ───────────────
            self.delay[0][self.write_pos] = sample.left;
            self.delay[1][self.write_pos] = sample.right;

            // ── 7. Output ────────────────────────────────────────────────
            sample.left = out_left;
            sample.right = out_right;

            // Advance ring-buffer cursor
            self.write_pos = (self.write_pos + 1) % MAX_DELAY;
        }
    }

    /// Find the minimum target gain across the current lookahead window.
    ///
    /// Scans `lookahead` entries (72 at 48 kHz). The cost is ~3.5 M
    /// comparisons/s which is negligible on any modern CPU.
    #[inline]
    fn window_min_gain(&self) -> f32 {
        let mut min = 1.0_f32;
        // Scan from write_pos backward through the lookahead window
        for i in 0..self.lookahead {
            let pos = (self.write_pos + MAX_DELAY - i) % MAX_DELAY;
            let g = self.target_gains[pos];
            if g < min {
                min = g;
            }
        }
        min
    }
}

impl Default for MasterLimiter {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StereoSample;

    fn make_buffer(samples: &[(f32, f32)]) -> StereoBuffer {
        let mut buf = StereoBuffer::with_capacity(samples.len());
        buf.resize(samples.len());
        for (i, &(l, r)) in samples.iter().enumerate() {
            buf.as_mut_slice()[i] = StereoSample::new(l, r);
        }
        buf
    }

    #[test]
    fn test_below_threshold_is_transparent() {
        let mut limiter = MasterLimiter::new();
        let threshold = limiter.threshold;

        // Feed silence to fill the delay line
        let mut warmup = make_buffer(&[(0.0, 0.0); 128]);
        limiter.process(&mut warmup);

        // Signal at half threshold should pass through unchanged
        let level = threshold * 0.5;
        let mut buf = make_buffer(&[(level, -level); 128]);
        limiter.process(&mut buf);

        // After the lookahead delay, output should match input
        for i in limiter.lookahead..128 {
            let s = buf.as_slice()[i];
            assert!(
                (s.left - level).abs() < 1e-5,
                "left[{}] = {}, expected {}",
                i, s.left, level
            );
            assert!(
                (s.right - (-level)).abs() < 1e-5,
                "right[{}] = {}, expected {}",
                i, s.right, -level
            );
        }
    }

    #[test]
    fn test_hot_signal_is_reduced() {
        let mut limiter = MasterLimiter::new();
        let threshold = limiter.threshold;

        // Warmup
        let mut warmup = make_buffer(&[(0.0, 0.0); 128]);
        limiter.process(&mut warmup);

        // Signal 6 dB above threshold
        let hot = threshold * 2.0;
        let mut buf = make_buffer(&[(hot, hot); 256]);
        limiter.process(&mut buf);

        // After convergence the output should be near the threshold.
        // Allow generous tolerance because the exponential attack
        // doesn't reach the exact target instantly.
        for i in 128..256 {
            let s = buf.as_slice()[i];
            assert!(
                s.left <= threshold * 1.05,
                "left[{}] = {} exceeds threshold {} by more than 5 %",
                i, s.left, threshold
            );
        }
    }

    #[test]
    fn test_gain_recovers_after_transient() {
        let mut limiter = MasterLimiter::new();
        let threshold = limiter.threshold;

        // Warmup with silence
        let mut warmup = make_buffer(&[(0.0, 0.0); 128]);
        limiter.process(&mut warmup);

        // Short transient burst (hot)
        let hot = threshold * 2.0;
        let mut burst = make_buffer(&[(hot, hot); 32]);
        limiter.process(&mut burst);

        // Followed by quiet signal for 300 ms (~3 release time-constants).
        // With τ = 100 ms, after 300 ms: residual ≈ exp(−3) ≈ 5 %
        let quiet = threshold * 0.3;
        let mut tail = make_buffer(&[(quiet, quiet); 14400]); // 300 ms at 48 kHz
        limiter.process(&mut tail);

        // Gain should have recovered to > 90 % of unity
        let last = tail.as_slice()[14399];
        assert!(
            last.left > quiet * 0.9,
            "gain didn't recover: output {} vs expected > {}",
            last.left, quiet * 0.9
        );
    }

    #[test]
    fn test_latency() {
        let limiter = MasterLimiter::new();
        // At 48 kHz, 1.5 ms → 72 samples
        assert_eq!(limiter.latency_samples(), 72);
    }
}
