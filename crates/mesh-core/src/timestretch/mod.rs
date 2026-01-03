//! Time-stretching via signalsmith-stretch
//!
//! Wraps the signalsmith-stretch library to provide BPM-synchronized playback.
//! The stretcher adjusts audio tempo to match a global BPM without pitch change.

use signalsmith_stretch::Stretch;

use crate::types::{StereoBuffer, StereoSample, SAMPLE_RATE};

/// Number of channels (stereo)
const CHANNELS: u32 = 2;

/// Time stretcher for BPM synchronization
///
/// Takes stereo audio at the track's original tempo and outputs audio
/// stretched/compressed to match the global BPM.
pub struct TimeStretcher {
    /// The underlying signalsmith stretcher
    stretcher: Stretch,
    /// Current stretch ratio (output_bpm / input_bpm)
    ratio: f64,
    /// Temporary input buffer (interleaved stereo)
    input_buffer: Vec<f32>,
    /// Temporary output buffer (interleaved stereo)
    output_buffer: Vec<f32>,
}

impl TimeStretcher {
    /// Create a new time stretcher
    pub fn new() -> Self {
        let stretcher = Stretch::preset_default(CHANNELS, SAMPLE_RATE);

        Self {
            stretcher,
            ratio: 1.0,
            input_buffer: Vec::new(),
            output_buffer: Vec::new(),
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
    /// Takes input audio and produces output audio at the stretched tempo.
    /// For a ratio > 1.0 (speedup), output will have fewer samples than input.
    /// For a ratio < 1.0 (slowdown), output will have more samples than input.
    pub fn process(&mut self, input: &StereoBuffer, output: &mut StereoBuffer) {
        if input.is_empty() {
            output.fill_silence();
            return;
        }

        let input_len = input.len();
        let output_len = output.len();

        // Ensure buffers are large enough
        if self.input_buffer.len() < input_len * 2 {
            self.input_buffer.resize(input_len * 2, 0.0);
        }
        if self.output_buffer.len() < output_len * 2 {
            self.output_buffer.resize(output_len * 2, 0.0);
        }

        // Convert input to interleaved
        for (i, sample) in input.iter().enumerate() {
            self.input_buffer[i * 2] = sample.left;
            self.input_buffer[i * 2 + 1] = sample.right;
        }

        // Clear output buffer
        self.output_buffer[..output_len * 2].fill(0.0);

        // Process through signalsmith-stretch
        // The ratio is controlled by providing different input/output sizes:
        // - To speed up (ratio > 1), we need fewer output samples per input sample
        // - To slow down (ratio < 1), we need more output samples per input sample
        //
        // With signalsmith-stretch, the ratio is input_samples / output_samples
        // So we calculate how many input samples to provide for the desired output
        let input_samples = (output_len as f64 / self.ratio) as usize;
        let input_samples = input_samples.min(input_len);

        self.stretcher.process(
            &self.input_buffer[..input_samples * 2],
            &mut self.output_buffer[..output_len * 2],
        );

        // Convert output from interleaved
        for i in 0..output_len {
            output.as_mut_slice()[i] = StereoSample::new(
                self.output_buffer[i * 2],
                self.output_buffer[i * 2 + 1],
            );
        }
    }

    /// Flush any remaining audio from the stretcher
    pub fn flush(&mut self, output: &mut StereoBuffer) {
        let output_len = output.len();
        if self.output_buffer.len() < output_len * 2 {
            self.output_buffer.resize(output_len * 2, 0.0);
        }

        self.output_buffer[..output_len * 2].fill(0.0);
        self.stretcher.flush(&mut self.output_buffer[..output_len * 2]);

        for i in 0..output_len {
            output.as_mut_slice()[i] = StereoSample::new(
                self.output_buffer[i * 2],
                self.output_buffer[i * 2 + 1],
            );
        }
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
