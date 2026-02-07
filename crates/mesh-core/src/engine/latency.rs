//! Global latency compensation across all stems
//!
//! All 16 stems (4 decks × 4 stems) must be sample-aligned for proper
//! beat sync. This module provides delay buffers for latency compensation.

use crate::types::{StereoBuffer, StereoSample, MAX_LATENCY_SAMPLES, NUM_DECKS, NUM_STEMS};

/// Ring buffer for delay line
struct DelayLine {
    buffer: Vec<StereoSample>,
    write_pos: usize,
    delay_samples: usize,
}

impl DelayLine {
    /// Create a new delay line with the given maximum size
    fn new(max_samples: usize) -> Self {
        Self {
            buffer: vec![StereoSample::silence(); max_samples],
            write_pos: 0,
            delay_samples: 0,
        }
    }

    /// Set the delay amount in samples
    fn set_delay(&mut self, samples: usize) {
        if samples >= self.buffer.len() {
            log::warn!(
                "Latency compensation {} samples exceeds buffer size {}, clamping! Audio may be misaligned.",
                samples,
                self.buffer.len()
            );
        }
        self.delay_samples = samples.min(self.buffer.len() - 1);
    }

    /// Process a single sample through the delay line
    #[inline]
    fn process(&mut self, input: StereoSample) -> StereoSample {
        // Write input to buffer
        self.buffer[self.write_pos] = input;

        // Calculate read position (behind write position by delay_samples)
        let read_pos = if self.write_pos >= self.delay_samples {
            self.write_pos - self.delay_samples
        } else {
            self.buffer.len() - (self.delay_samples - self.write_pos)
        };

        // Read delayed output
        let output = self.buffer[read_pos];

        // Advance write position
        self.write_pos = (self.write_pos + 1) % self.buffer.len();

        output
    }

    /// Clear the delay line (fill with silence)
    fn clear(&mut self) {
        self.buffer.fill(StereoSample::silence());
        self.write_pos = 0;
    }
}

/// Global latency compensator for all stems across all decks
///
/// Maintains 16 delay lines (4 decks × 4 stems) to ensure all audio
/// paths have equal latency for proper beat sync.
pub struct LatencyCompensator {
    /// Delay lines for each stem of each deck [deck][stem]
    delay_lines: [[DelayLine; NUM_STEMS]; NUM_DECKS],
    /// Maximum latency across all stems (the target latency)
    global_max_latency: u32,
    /// Per-stem latencies (for calculating compensation)
    stem_latencies: [[u32; NUM_STEMS]; NUM_DECKS],
}

impl LatencyCompensator {
    /// Create a new latency compensator
    pub fn new() -> Self {
        Self {
            delay_lines: std::array::from_fn(|_| {
                std::array::from_fn(|_| DelayLine::new(MAX_LATENCY_SAMPLES))
            }),
            global_max_latency: 0,
            stem_latencies: [[0; NUM_STEMS]; NUM_DECKS],
        }
    }

    /// Update the latency for a specific stem
    ///
    /// Call this whenever an effect chain changes (add/remove/bypass effect).
    /// This will recalculate compensation delays for all stems.
    pub fn set_stem_latency(&mut self, deck: usize, stem: usize, latency: u32) {
        if deck < NUM_DECKS && stem < NUM_STEMS {
            self.stem_latencies[deck][stem] = latency;
            self.recalculate_delays();
        }
    }

    /// Get the current global maximum latency
    pub fn global_latency(&self) -> u32 {
        self.global_max_latency
    }

    /// Recalculate all compensation delays based on current stem latencies
    fn recalculate_delays(&mut self) {
        let old_max = self.global_max_latency;

        // Find global maximum
        self.global_max_latency = self
            .stem_latencies
            .iter()
            .flat_map(|deck| deck.iter())
            .copied()
            .max()
            .unwrap_or(0);

        // Log when global max changes
        if self.global_max_latency != old_max {
            log::info!(
                "[LATENCY] Global max changed: {} -> {} samples ({:.2}ms @ 48kHz)",
                old_max,
                self.global_max_latency,
                self.global_max_latency as f32 / 48.0
            );
            // Log per-deck latencies for debugging
            for deck in 0..NUM_DECKS {
                let stems = &self.stem_latencies[deck];
                if stems.iter().any(|&s| s > 0) {
                    log::debug!(
                        "[LATENCY] Deck {} stems: [{}, {}, {}, {}]",
                        deck, stems[0], stems[1], stems[2], stems[3]
                    );
                }
            }
        }

        // Set compensation delay for each stem
        for deck in 0..NUM_DECKS {
            for stem in 0..NUM_STEMS {
                let compensation = self.global_max_latency - self.stem_latencies[deck][stem];
                self.delay_lines[deck][stem].set_delay(compensation as usize);
            }
        }
    }

    /// Process a buffer through the compensation delay line for a specific stem
    pub fn process(&mut self, deck: usize, stem: usize, buffer: &mut StereoBuffer) {
        if deck >= NUM_DECKS || stem >= NUM_STEMS {
            return;
        }

        let delay_line = &mut self.delay_lines[deck][stem];
        for sample in buffer.iter_mut() {
            *sample = delay_line.process(*sample);
        }
    }

    /// Process a single sample through the compensation delay line
    #[inline]
    pub fn process_sample(
        &mut self,
        deck: usize,
        stem: usize,
        sample: StereoSample,
    ) -> StereoSample {
        if deck >= NUM_DECKS || stem >= NUM_STEMS {
            return sample;
        }
        self.delay_lines[deck][stem].process(sample)
    }

    /// Clear all delay lines (call on track load, etc.)
    pub fn clear(&mut self) {
        for deck in &mut self.delay_lines {
            for delay_line in deck {
                delay_line.clear();
            }
        }
    }

    /// Clear delay lines for a specific deck
    pub fn clear_deck(&mut self, deck: usize) {
        if deck < NUM_DECKS {
            for delay_line in &mut self.delay_lines[deck] {
                delay_line.clear();
            }
        }
    }
}

impl Default for LatencyCompensator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delay_line() {
        let mut delay = DelayLine::new(10);
        delay.set_delay(3);

        // First 3 samples should output silence (delay filling)
        let s1 = StereoSample::new(1.0, 1.0);
        let s2 = StereoSample::new(2.0, 2.0);
        let s3 = StereoSample::new(3.0, 3.0);
        let s4 = StereoSample::new(4.0, 4.0);

        assert_eq!(delay.process(s1), StereoSample::silence());
        assert_eq!(delay.process(s2), StereoSample::silence());
        assert_eq!(delay.process(s3), StereoSample::silence());

        // After delay, we should get delayed samples
        let out = delay.process(s4);
        assert_eq!(out.left, 1.0);
        assert_eq!(out.right, 1.0);
    }

    #[test]
    fn test_latency_compensator() {
        let mut comp = LatencyCompensator::new();

        // Set some latencies
        comp.set_stem_latency(0, 0, 100); // Deck 0, stem 0: 100 samples
        comp.set_stem_latency(0, 1, 200); // Deck 0, stem 1: 200 samples
        comp.set_stem_latency(1, 0, 150); // Deck 1, stem 0: 150 samples

        // Global max should be 200
        assert_eq!(comp.global_latency(), 200);
    }
}
