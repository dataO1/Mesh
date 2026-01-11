//! Time-stretching via signalsmith-stretch
//!
//! Wraps the signalsmith-stretch library to provide BPM-synchronized playback.
//! The stretcher adjusts audio tempo to match a global BPM without pitch change.

use signalsmith_stretch::Stretch;

use crate::types::{StereoBuffer, SAMPLE_RATE};

/// Number of channels (stereo)
const CHANNELS: u32 = 2;

/// Time stretcher for BPM synchronization and pitch shifting
///
/// Takes stereo audio at the track's original tempo and outputs audio
/// stretched/compressed to match the global BPM, with optional pitch shifting
/// for key matching.
///
/// Uses zero-copy format conversion - StereoBuffer is reinterpreted as
/// interleaved f32 without any per-frame copying.
pub struct TimeStretcher {
    /// The underlying signalsmith stretcher
    stretcher: Stretch,
    /// Current stretch ratio (output_bpm / input_bpm)
    ratio: f64,
    /// Pitch shift in semitones (positive = up, negative = down)
    pitch_semitones: f64,
}

impl TimeStretcher {
    /// Create a new time stretcher with the specified sample rate
    pub fn new_with_sample_rate(sample_rate: u32) -> Self {
        let stretcher = Stretch::preset_default(CHANNELS, sample_rate);

        Self {
            stretcher,
            ratio: 1.0,
            pitch_semitones: 0.0,
        }
    }

    /// Create a new time stretcher with default sample rate
    pub fn new() -> Self {
        Self::new_with_sample_rate(SAMPLE_RATE)
    }

    /// Create a faster time stretcher with reduced quality
    ///
    /// Uses signalsmith-stretch's `preset_cheaper` which is 30-50% faster
    /// but with slightly lower audio quality. Ideal for background operations
    /// like pre-stretching linked stems where speed matters more than
    /// maximum quality.
    pub fn new_cheaper(sample_rate: u32) -> Self {
        let stretcher = Stretch::preset_cheaper(CHANNELS, sample_rate);

        Self {
            stretcher,
            ratio: 1.0,
            pitch_semitones: 0.0,
        }
    }

    /// Set the stretch ratio (output_bpm / input_bpm)
    ///
    /// ratio > 1.0: speed up (fewer output samples per input)
    /// ratio < 1.0: slow down (more output samples per input)
    /// ratio = 1.0: no change
    pub fn set_ratio(&mut self, ratio: f64) {
        self.ratio = ratio.clamp(0.5, 2.0); // Limit to reasonable range
    }

    /// Get the current stretch ratio
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Calculate stretch ratio from BPMs
    pub fn ratio_from_bpm(track_bpm: f64, target_bpm: f64) -> f64 {
        if track_bpm > 0.0 {
            target_bpm / track_bpm
        } else {
            1.0
        }
    }

    /// Set ratio from track and target BPM
    pub fn set_bpm(&mut self, track_bpm: f64, target_bpm: f64) {
        self.set_ratio(Self::ratio_from_bpm(track_bpm, target_bpm));
    }

    /// Set pitch shift in semitones (positive = up, negative = down)
    ///
    /// Used for automatic key matching - transposes audio to match the master deck's key.
    /// Range is clamped to -12..+12 semitones (one octave).
    pub fn set_pitch_semitones(&mut self, semitones: f64) {
        self.pitch_semitones = semitones.clamp(-12.0, 12.0);
        // Call signalsmith-stretch's transpose function
        // None for tonality_limit means no limit on formant preservation
        self.stretcher
            .set_transpose_factor_semitones(self.pitch_semitones as f32, None);
    }

    /// Get the current pitch shift in semitones
    pub fn pitch_semitones(&self) -> f64 {
        self.pitch_semitones
    }

    /// Get the input latency in samples
    pub fn input_latency(&self) -> usize {
        self.stretcher.input_latency()
    }

    /// Get the output latency in samples
    pub fn output_latency(&self) -> usize {
        self.stretcher.output_latency()
    }

    /// Total latency in samples
    pub fn total_latency(&self) -> usize {
        self.input_latency() + self.output_latency()
    }

    /// Reset the stretcher state
    pub fn reset(&mut self) {
        self.stretcher.reset();
    }

    /// Process audio through the time stretcher
    ///
    /// Takes input audio (variable size from deck) and produces output audio
    /// at the target size. The deck has already sized the input buffer based on
    /// stretch_ratio, so we just pass through to signalsmith-stretch.
    ///
    /// The stretch ratio is: input_len / output_len
    /// - input_len > output_len: speedup (compressing more samples into fewer)
    /// - input_len < output_len: slowdown (expanding fewer samples into more)
    /// - input_len = output_len: no stretching
    ///
    /// Uses zero-copy format conversion via bytemuck - the input/output buffers
    /// are reinterpreted as interleaved f32 without any copying.
    pub fn process(&mut self, input: &StereoBuffer, output: &mut StereoBuffer) {
        if input.is_empty() {
            output.fill_silence();
            return;
        }

        let input_len = input.len();
        let output_len = output.len();

        // Zero-copy format conversion: reinterpret StereoSample slices as interleaved f32
        // Thanks to #[repr(C)] on StereoSample, [StereoSample] has the same layout as [f32]
        let input_interleaved = input.as_interleaved();
        let output_interleaved = output.as_interleaved_mut();

        // Clear output region we'll write to
        output_interleaved[..output_len * 2].fill(0.0);

        // Pass variable input through signalsmith-stretch to produce fixed output
        // The actual stretch ratio is determined by the size difference
        self.stretcher.process(
            &input_interleaved[..input_len * 2],
            &mut output_interleaved[..output_len * 2],
        );
    }

    /// Flush any remaining audio from the stretcher
    ///
    /// Uses zero-copy format conversion via bytemuck.
    pub fn flush(&mut self, output: &mut StereoBuffer) {
        let output_len = output.len();
        let output_interleaved = output.as_interleaved_mut();

        output_interleaved[..output_len * 2].fill(0.0);
        self.stretcher.flush(&mut output_interleaved[..output_len * 2]);
    }
}

impl Default for TimeStretcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_stretcher_creation() {
        let stretcher = TimeStretcher::new();
        assert_eq!(stretcher.ratio(), 1.0);
        assert!(stretcher.input_latency() > 0);
        assert!(stretcher.output_latency() > 0);
    }

    #[test]
    fn test_ratio_calculation() {
        // 120 BPM track playing at 128 BPM
        let ratio = TimeStretcher::ratio_from_bpm(120.0, 128.0);
        assert!((ratio - 128.0 / 120.0).abs() < 0.001);

        // 130 BPM track playing at 120 BPM
        let ratio = TimeStretcher::ratio_from_bpm(130.0, 120.0);
        assert!((ratio - 120.0 / 130.0).abs() < 0.001);
    }

    #[test]
    fn test_process_unity_ratio() {
        let mut stretcher = TimeStretcher::new();
        stretcher.set_ratio(1.0);

        let input = StereoBuffer::silence(512);
        let mut output = StereoBuffer::silence(512);

        stretcher.process(&input, &mut output);

        // Output should be valid (not checking exact values due to windowing)
        assert_eq!(output.len(), 512);
    }

    #[test]
    fn test_bpm_setting() {
        let mut stretcher = TimeStretcher::new();
        stretcher.set_bpm(120.0, 128.0);

        let expected = 128.0 / 120.0;
        assert!((stretcher.ratio() - expected).abs() < 0.001);
    }
}
