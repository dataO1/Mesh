//! Linkwitz-Riley Crossover Filter
//!
//! Splits audio into multiple frequency bands using Linkwitz-Riley 24dB/oct
//! crossover filters. LR24 crossovers sum to unity gain with no phase issues
//! at the crossover frequency.
//!
//! ## How it works
//!
//! A Linkwitz-Riley filter is created by cascading two Butterworth filters.
//! For LR24 (24dB/octave slope), we cascade two 12dB/oct (2-pole) Butterworth
//! filters with Q=0.707 (1/√2).
//!
//! For N bands, we need N-1 crossover frequencies. Each crossover point
//! creates a lowpass and highpass split.

use crate::types::{StereoBuffer, StereoSample, SAMPLE_RATE};

/// Maximum number of bands supported
pub const MAX_BANDS: usize = 8;

/// Two-pole (12dB/octave) state-variable filter
///
/// This is the building block for our LR24 crossover. We use SVF topology
/// because it's numerically stable and provides LP, HP, BP simultaneously.
#[derive(Clone)]
struct SvfFilter {
    // State per channel (left/right)
    ic1eq_l: f32,
    ic2eq_l: f32,
    ic1eq_r: f32,
    ic2eq_r: f32,
    // Coefficients
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfFilter {
    fn new() -> Self {
        let mut f = Self {
            ic1eq_l: 0.0,
            ic2eq_l: 0.0,
            ic1eq_r: 0.0,
            ic2eq_r: 0.0,
            g: 0.0,
            k: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
        };
        // Default: 1kHz, Butterworth Q for LR cascade
        f.set_frequency(1000.0);
        f
    }

    /// Set cutoff frequency with Butterworth Q (0.707)
    fn set_frequency(&mut self, cutoff: f32) {
        let cutoff = cutoff.clamp(20.0, 20000.0);
        // Q = 0.707 (1/sqrt(2)) for Butterworth, which cascades to LR24
        let q = std::f32::consts::FRAC_1_SQRT_2;

        self.g = (std::f32::consts::PI * cutoff / SAMPLE_RATE as f32).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }

    /// Process stereo sample, returns (lowpass, highpass)
    #[inline]
    fn process(&mut self, input: StereoSample) -> (StereoSample, StereoSample) {
        // Left channel
        let v3_l = input.left - self.ic2eq_l;
        let v1_l = self.a1 * self.ic1eq_l + self.a2 * v3_l;
        let v2_l = self.ic2eq_l + self.a2 * self.ic1eq_l + self.a3 * v3_l;
        self.ic1eq_l = 2.0 * v1_l - self.ic1eq_l;
        self.ic2eq_l = 2.0 * v2_l - self.ic2eq_l;

        let low_l = v2_l;
        let band_l = v1_l;
        let high_l = input.left - self.k * band_l - low_l;

        // Right channel
        let v3_r = input.right - self.ic2eq_r;
        let v1_r = self.a1 * self.ic1eq_r + self.a2 * v3_r;
        let v2_r = self.ic2eq_r + self.a2 * self.ic1eq_r + self.a3 * v3_r;
        self.ic1eq_r = 2.0 * v1_r - self.ic1eq_r;
        self.ic2eq_r = 2.0 * v2_r - self.ic2eq_r;

        let low_r = v2_r;
        let band_r = v1_r;
        let high_r = input.right - self.k * band_r - low_r;

        (
            StereoSample::new(low_l, low_r),
            StereoSample::new(high_l, high_r),
        )
    }

    fn reset(&mut self) {
        self.ic1eq_l = 0.0;
        self.ic2eq_l = 0.0;
        self.ic1eq_r = 0.0;
        self.ic2eq_r = 0.0;
    }
}

/// A single LR24 crossover point (splits into low and high)
///
/// Uses two cascaded 12dB Butterworth filters to achieve 24dB/oct slopes.
#[derive(Clone)]
struct CrossoverPoint {
    /// First stage lowpass
    lp1: SvfFilter,
    /// Second stage lowpass (cascade)
    lp2: SvfFilter,
    /// First stage highpass
    hp1: SvfFilter,
    /// Second stage highpass (cascade)
    hp2: SvfFilter,
    /// Crossover frequency in Hz
    frequency: f32,
}

impl CrossoverPoint {
    fn new(frequency: f32) -> Self {
        let mut point = Self {
            lp1: SvfFilter::new(),
            lp2: SvfFilter::new(),
            hp1: SvfFilter::new(),
            hp2: SvfFilter::new(),
            frequency,
        };
        point.set_frequency(frequency);
        point
    }

    fn set_frequency(&mut self, freq: f32) {
        self.frequency = freq.clamp(20.0, 20000.0);
        self.lp1.set_frequency(self.frequency);
        self.lp2.set_frequency(self.frequency);
        self.hp1.set_frequency(self.frequency);
        self.hp2.set_frequency(self.frequency);
    }

    /// Process and split into (low_band, high_band)
    #[inline]
    fn process(&mut self, input: StereoSample) -> (StereoSample, StereoSample) {
        // Lowpass path: input → LP1 → LP2 (24dB/oct total)
        let (lp1_out, _) = self.lp1.process(input);
        let (low, _) = self.lp2.process(lp1_out);

        // Highpass path: input → HP1 → HP2 (24dB/oct total)
        let (_, hp1_out) = self.hp1.process(input);
        let (_, high) = self.hp2.process(hp1_out);

        (low, high)
    }

    fn reset(&mut self) {
        self.lp1.reset();
        self.lp2.reset();
        self.hp1.reset();
        self.hp2.reset();
    }
}

/// Linkwitz-Riley 24dB/oct multiband crossover
///
/// Splits audio into 2-8 bands using LR24 crossover filters.
/// The bands sum back to the original signal (unity gain, phase coherent).
///
/// ## Usage
///
/// ```ignore
/// let mut crossover = LinkwitzRileyCrossover::new();
/// crossover.set_band_count(3);
/// crossover.set_frequency(0, 200.0);  // Low/Mid split at 200Hz
/// crossover.set_frequency(1, 2000.0); // Mid/High split at 2kHz
///
/// // Process audio
/// let bands = crossover.process(input_sample);
/// // bands[0] = 20-200Hz, bands[1] = 200-2000Hz, bands[2] = 2000-20kHz
/// ```
pub struct LinkwitzRileyCrossover {
    /// Crossover points (N-1 for N bands)
    crossovers: [CrossoverPoint; MAX_BANDS - 1],
    /// Number of active bands (2-8)
    band_count: usize,
    /// Whether the crossover is enabled
    enabled: bool,
}

impl LinkwitzRileyCrossover {
    /// Create a new crossover with default frequencies
    pub fn new() -> Self {
        // Default crossover frequencies for up to 8 bands
        // These are typical multiband mastering frequencies
        let default_freqs = [100.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

        let crossovers = std::array::from_fn(|i| {
            CrossoverPoint::new(default_freqs.get(i).copied().unwrap_or(1000.0))
        });

        Self {
            crossovers,
            band_count: 1, // Single band = passthrough (no splitting)
            enabled: false,
        }
    }

    /// Set the number of bands (1-8)
    ///
    /// - 1 band = no splitting (passthrough)
    /// - 2 bands = 1 crossover point
    /// - N bands = N-1 crossover points
    pub fn set_band_count(&mut self, count: usize) {
        self.band_count = count.clamp(1, MAX_BANDS);
        self.enabled = self.band_count > 1;
    }

    /// Get the number of active bands
    pub fn band_count(&self) -> usize {
        self.band_count
    }

    /// Set crossover frequency for a specific crossover point
    ///
    /// Index 0 is between band 0 and band 1, etc.
    pub fn set_frequency(&mut self, crossover_index: usize, frequency: f32) {
        if crossover_index < self.band_count.saturating_sub(1) {
            self.crossovers[crossover_index].set_frequency(frequency);
        }
    }

    /// Get crossover frequency for a specific point
    pub fn frequency(&self, crossover_index: usize) -> f32 {
        self.crossovers
            .get(crossover_index)
            .map(|c| c.frequency)
            .unwrap_or(1000.0)
    }

    /// Check if crossover is enabled (more than 1 band)
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Process a single stereo sample and return band outputs
    ///
    /// Returns an array where only the first `band_count` elements are valid.
    /// If band_count is 1, returns the input unchanged in band 0.
    #[inline]
    pub fn process(&mut self, input: StereoSample) -> [StereoSample; MAX_BANDS] {
        let mut bands = [StereoSample::default(); MAX_BANDS];

        if !self.enabled || self.band_count <= 1 {
            // Passthrough mode
            bands[0] = input;
            return bands;
        }

        // For N bands, we process through N-1 crossover points
        // Each crossover splits into low and high
        //
        // Example for 3 bands with crossovers at 200Hz and 2kHz:
        // 1. Split input at 200Hz → low0, high0
        // 2. Split high0 at 2kHz → low1, high1
        // 3. Band 0 = low0, Band 1 = low1, Band 2 = high1

        let mut current = input;

        for i in 0..(self.band_count - 1) {
            let (low, high) = self.crossovers[i].process(current);
            bands[i] = low;
            current = high;
        }

        // Last band gets the remaining high frequencies
        bands[self.band_count - 1] = current;

        bands
    }

    /// Process an entire buffer, writing results to band buffers
    ///
    /// `band_buffers` should have at least `band_count` elements,
    /// each with the same length as `input`.
    pub fn process_buffer(
        &mut self,
        input: &StereoBuffer,
        band_buffers: &mut [StereoBuffer],
    ) {
        let band_count = self.band_count.min(band_buffers.len());

        for (i, sample) in input.iter().enumerate() {
            let bands = self.process(*sample);

            for (b, band_buf) in band_buffers.iter_mut().enumerate().take(band_count) {
                if i < band_buf.len() {
                    band_buf.as_mut_slice()[i] = bands[b];
                }
            }
        }
    }

    /// Reset all filter states (call when starting new audio or after silence)
    pub fn reset(&mut self) {
        for crossover in &mut self.crossovers {
            crossover.reset();
        }
    }
}

impl Default for LinkwitzRileyCrossover {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for LinkwitzRileyCrossover {
    fn clone(&self) -> Self {
        Self {
            crossovers: self.crossovers.clone(),
            band_count: self.band_count,
            enabled: self.enabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crossover_creation() {
        let crossover = LinkwitzRileyCrossover::new();
        assert_eq!(crossover.band_count(), 1);
        assert!(!crossover.is_enabled());
    }

    #[test]
    fn test_crossover_band_count() {
        let mut crossover = LinkwitzRileyCrossover::new();

        crossover.set_band_count(3);
        assert_eq!(crossover.band_count(), 3);
        assert!(crossover.is_enabled());

        crossover.set_band_count(1);
        assert_eq!(crossover.band_count(), 1);
        assert!(!crossover.is_enabled());

        // Clamp to max
        crossover.set_band_count(100);
        assert_eq!(crossover.band_count(), MAX_BANDS);
    }

    #[test]
    fn test_passthrough_mode() {
        let mut crossover = LinkwitzRileyCrossover::new();
        crossover.set_band_count(1);

        let input = StereoSample::new(0.5, -0.5);
        let bands = crossover.process(input);

        assert_eq!(bands[0].left, input.left);
        assert_eq!(bands[0].right, input.right);
    }

    #[test]
    fn test_two_band_split() {
        let mut crossover = LinkwitzRileyCrossover::new();
        crossover.set_band_count(2);
        crossover.set_frequency(0, 1000.0);

        // Process samples to let the filter settle (need many samples for LR24)
        let mut last_sum = 0.0;
        for _ in 0..10000 {
            let input = StereoSample::new(1.0, 1.0);
            let bands = crossover.process(input);
            last_sum = bands[0].left + bands[1].left;
        }

        // After settling, sum should be close to 1.0 (unity gain)
        // LR24 crossovers sum to unity for steady-state DC input
        assert!(
            (last_sum - 1.0).abs() < 0.01,
            "LR24 should sum to unity after settling, got {}",
            last_sum
        );
    }

    #[test]
    fn test_frequency_response() {
        let mut crossover = LinkwitzRileyCrossover::new();
        crossover.set_band_count(2);
        crossover.set_frequency(0, 1000.0); // 1kHz crossover

        // Test with DC (0 Hz) - should go entirely to low band
        crossover.reset();
        let mut low_energy = 0.0;
        let mut high_energy = 0.0;

        for _ in 0..10000 {
            let input = StereoSample::new(1.0, 1.0);
            let bands = crossover.process(input);
            low_energy += bands[0].left.abs();
            high_energy += bands[1].left.abs();
        }

        // DC should be mostly in the low band
        assert!(
            low_energy > high_energy * 10.0,
            "DC should be in low band: low={}, high={}",
            low_energy,
            high_energy
        );
    }

    #[test]
    fn test_three_band_frequencies() {
        let mut crossover = LinkwitzRileyCrossover::new();
        crossover.set_band_count(3);
        crossover.set_frequency(0, 200.0);
        crossover.set_frequency(1, 2000.0);

        assert_eq!(crossover.frequency(0), 200.0);
        assert_eq!(crossover.frequency(1), 2000.0);
    }
}
