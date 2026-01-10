//! MIDI hardware type detection
//!
//! Analyzes MIDI message patterns during learn mode to automatically detect
//! what type of physical control is being used (button, knob, fader, encoder, etc.)

use crate::config::HardwareType;
use std::time::{Duration, Instant};

/// Default sampling duration for hardware detection
const DETECTION_DURATION: Duration = Duration::from_millis(1000);

/// Minimum samples needed for reliable detection
const MIN_SAMPLES_FOR_DETECTION: usize = 3;

/// Time window for detecting 14-bit CC pairs (MSB + LSB)
const PAIR_DETECTION_WINDOW: Duration = Duration::from_millis(5);

/// Message rate threshold for jog wheel vs encoder (messages per second)
const JOG_WHEEL_RATE_THRESHOLD: f32 = 15.0;

/// Range threshold for relative encoder detection (values stay near center)
const RELATIVE_RANGE_THRESHOLD: u8 = 30;

/// Proximity to center (64) for relative encoder detection
const CENTER_PROXIMITY_THRESHOLD: f32 = 15.0;

/// Range threshold for absolute controls (knob/fader)
const ABSOLUTE_RANGE_THRESHOLD: u8 = 50;

/// Monotonicity ratio for fader vs knob distinction
const FADER_MONOTONICITY_THRESHOLD: f32 = 0.8;

/// Time-stamped MIDI sample for detection analysis
#[derive(Debug, Clone)]
pub struct MidiSample {
    /// Milliseconds since detection started
    pub timestamp_ms: u64,
    /// MIDI value (0-127)
    pub value: u8,
    /// Whether this was a Note message (vs CC)
    pub is_note: bool,
    /// For notes: true if NoteOn with velocity > 0
    pub is_note_on: bool,
    /// CC number (for 14-bit pair detection)
    pub cc_number: Option<u8>,
}

/// Buffer for collecting MIDI samples during hardware detection
#[derive(Debug, Clone)]
pub struct MidiSampleBuffer {
    /// When sampling started
    start_time: Instant,
    /// Collected samples
    samples: Vec<MidiSample>,
    /// Maximum sampling duration
    max_duration: Duration,
    /// MIDI channel (for consistency check)
    channel: u8,
    /// Note/CC number (primary control being sampled)
    number: u8,
    /// Whether we're sampling notes or CC
    is_note: bool,
    /// Detected paired CC number (for 14-bit detection)
    /// If we see CC N and CC N+32 within 5ms, store the pair
    paired_cc: Option<u8>,
    /// Timestamp of last CC for pair detection
    last_cc_time: Option<Instant>,
    /// Last CC number seen (for pair detection)
    last_cc_number: Option<u8>,
}

impl MidiSampleBuffer {
    /// Create a new sample buffer for hardware detection
    ///
    /// # Arguments
    /// * `channel` - MIDI channel of the control
    /// * `number` - Note or CC number
    /// * `is_note` - true if Note message, false if CC
    pub fn new(channel: u8, number: u8, is_note: bool) -> Self {
        Self {
            start_time: Instant::now(),
            samples: Vec::with_capacity(64),
            max_duration: DETECTION_DURATION,
            channel,
            number,
            is_note,
            paired_cc: None,
            last_cc_time: None,
            last_cc_number: None,
        }
    }

    /// Check if this event matches the control being sampled
    pub fn matches(&self, channel: u8, number: u8, is_note: bool) -> bool {
        self.channel == channel && self.is_note == is_note && {
            if is_note {
                // For notes, must match exactly
                self.number == number
            } else {
                // For CC, allow paired CC (N+32 or N-32) for 14-bit detection
                self.number == number
                    || number == self.number.wrapping_add(32)
                    || number == self.number.wrapping_sub(32)
            }
        }
    }

    /// Add a sample to the buffer
    ///
    /// Returns true if sampling should continue, false if complete
    pub fn add_sample(&mut self, value: u8, is_note_on: bool, cc_number: Option<u8>) -> bool {
        let elapsed = self.start_time.elapsed();
        let timestamp_ms = elapsed.as_millis() as u64;

        // Check for 14-bit CC pair
        if !self.is_note {
            if let Some(cc) = cc_number {
                let now = Instant::now();

                // Check if this forms a pair with the previous CC
                if let (Some(last_time), Some(last_cc)) = (self.last_cc_time, self.last_cc_number) {
                    if now.duration_since(last_time) < PAIR_DETECTION_WINDOW {
                        // Check if it's a valid MSB/LSB pair
                        let diff = (cc as i16 - last_cc as i16).abs();
                        if diff == 32 {
                            // Found a 14-bit pair!
                            self.paired_cc = Some(cc.max(last_cc));
                            log::debug!(
                                "14-bit pair detected: CC {} + CC {} (diff: {})",
                                last_cc,
                                cc,
                                diff
                            );
                        }
                    }
                }

                self.last_cc_time = Some(now);
                self.last_cc_number = Some(cc);
            }
        }

        self.samples.push(MidiSample {
            timestamp_ms,
            value,
            is_note: self.is_note,
            is_note_on,
            cc_number,
        });

        // Continue sampling if within duration and not too many samples
        elapsed < self.max_duration && self.samples.len() < 256
    }

    /// Check if sampling is complete
    ///
    /// Sampling is complete when:
    /// - Duration has elapsed, or
    /// - We have enough samples for reliable detection
    pub fn is_complete(&self) -> bool {
        let elapsed = self.start_time.elapsed();

        // Always complete after max duration
        if elapsed >= self.max_duration {
            return true;
        }

        // For notes, we just need one press (instant detection)
        if self.is_note && !self.samples.is_empty() {
            return true;
        }

        // For CC, wait for either duration or sufficient samples with variation
        if self.samples.len() >= MIN_SAMPLES_FOR_DETECTION {
            let values: Vec<u8> = self.samples.iter().map(|s| s.value).collect();
            let min = *values.iter().min().unwrap_or(&64);
            let max = *values.iter().max().unwrap_or(&64);
            let range = max - min;

            // If we have clear variation, we can classify early
            if range > ABSOLUTE_RANGE_THRESHOLD {
                return true;
            }
        }

        false
    }

    /// Get the number of collected samples
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// Get elapsed sampling time as a ratio (0.0 - 1.0)
    pub fn elapsed_ratio(&self) -> f32 {
        let elapsed = self.start_time.elapsed();
        (elapsed.as_secs_f32() / self.max_duration.as_secs_f32()).min(1.0)
    }

    /// Analyze collected samples and determine hardware type
    pub fn analyze(&self) -> HardwareType {
        // Note messages are always buttons
        if self.is_note {
            return HardwareType::Button;
        }

        // Need samples for CC analysis
        if self.samples.is_empty() {
            return HardwareType::Unknown;
        }

        // Check for 14-bit pair first
        if self.paired_cc.is_some() {
            return HardwareType::Fader14Bit;
        }

        let values: Vec<u8> = self.samples.iter().map(|s| s.value).collect();

        if values.len() < 2 {
            // Single sample - can't determine type reliably
            // Use value position as hint
            let value = values[0];
            if (60..=68).contains(&value) {
                // Near center - likely encoder
                return HardwareType::Encoder;
            }
            return HardwareType::Unknown;
        }

        // Calculate statistics
        let min = *values.iter().min().unwrap();
        let max = *values.iter().max().unwrap();
        let range = max - min;
        let mean = values.iter().map(|&v| v as f32).sum::<f32>() / values.len() as f32;

        // Message rate (samples per second)
        let duration_secs = self.start_time.elapsed().as_secs_f32().max(0.001);
        let rate = values.len() as f32 / duration_secs;

        // Center proximity (how close mean is to 64)
        let center_proximity = (mean - 64.0).abs();
        let is_centered = center_proximity < CENTER_PROXIMITY_THRESHOLD;

        log::debug!(
            "Detection stats: range={}, mean={:.1}, rate={:.1}/s, centered={}",
            range,
            mean,
            rate,
            is_centered
        );

        // Classification logic
        if is_centered && range < RELATIVE_RANGE_THRESHOLD {
            // Values clustered around 64 with small range -> Relative encoder
            if rate > JOG_WHEEL_RATE_THRESHOLD {
                // High message rate -> Jog wheel
                log::debug!("Detected: JogWheel (centered, small range, high rate)");
                HardwareType::JogWheel
            } else {
                // Normal rate -> Browser encoder
                log::debug!("Detected: Encoder (centered, small range, normal rate)");
                HardwareType::Encoder
            }
        } else if range > ABSOLUTE_RANGE_THRESHOLD {
            // Wide range -> Absolute control
            let monotonicity = self.calculate_monotonicity(&values);
            log::debug!("Monotonicity: {:.2}", monotonicity);

            if monotonicity > FADER_MONOTONICITY_THRESHOLD {
                // Mostly moving in one direction -> Fader
                log::debug!("Detected: Fader (wide range, monotonic)");
                HardwareType::Fader
            } else {
                // Variable movement -> Knob
                log::debug!("Detected: Knob (wide range, variable)");
                HardwareType::Knob
            }
        } else if range > 0 {
            // Medium range - likely absolute knob/fader
            log::debug!("Detected: Knob (medium range)");
            HardwareType::Knob
        } else {
            // No variation - could be stuck at a value
            log::debug!("Detected: Unknown (no variation)");
            HardwareType::Unknown
        }
    }

    /// Calculate monotonicity ratio (0.0 = random, 1.0 = perfectly monotonic)
    fn calculate_monotonicity(&self, values: &[u8]) -> f32 {
        if values.len() < 3 {
            return 0.0;
        }

        let mut increasing = 0;
        let mut decreasing = 0;

        for i in 1..values.len() {
            if values[i] > values[i - 1] {
                increasing += 1;
            } else if values[i] < values[i - 1] {
                decreasing += 1;
            }
        }

        let total = increasing + decreasing;
        if total == 0 {
            return 0.0;
        }

        // Ratio of dominant direction
        (increasing.max(decreasing) as f32) / (total as f32)
    }

    /// Get the detected paired CC number (for 14-bit controls)
    pub fn get_paired_cc(&self) -> Option<u8> {
        self.paired_cc
    }

    /// Get the primary CC/note number
    pub fn get_number(&self) -> u8 {
        self.number
    }

    /// Get the MIDI channel
    pub fn get_channel(&self) -> u8 {
        self.channel
    }

    /// Check if this is a Note control
    pub fn is_note(&self) -> bool {
        self.is_note
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_button_detection() {
        let mut buffer = MidiSampleBuffer::new(0, 60, true); // Note message
        buffer.add_sample(127, true, None); // Note On

        assert!(buffer.is_complete());
        assert_eq!(buffer.analyze(), HardwareType::Button);
    }

    #[test]
    fn test_encoder_detection() {
        let mut buffer = MidiSampleBuffer::new(0, 35, false); // CC message

        // Simulate encoder values around center
        for &value in &[64, 65, 63, 64, 66, 62, 65, 63] {
            buffer.add_sample(value, false, Some(35));
        }

        let result = buffer.analyze();
        assert!(
            result == HardwareType::Encoder || result == HardwareType::JogWheel,
            "Expected Encoder or JogWheel, got {:?}",
            result
        );
    }

    #[test]
    fn test_knob_detection() {
        let mut buffer = MidiSampleBuffer::new(0, 10, false); // CC message

        // Simulate knob being turned back and forth
        for &value in &[0, 30, 60, 40, 80, 50, 100, 70, 127] {
            buffer.add_sample(value, false, Some(10));
        }

        assert_eq!(buffer.analyze(), HardwareType::Knob);
    }

    #[test]
    fn test_fader_detection() {
        let mut buffer = MidiSampleBuffer::new(0, 7, false); // CC message

        // Simulate fader being pushed up monotonically
        for &value in &[0, 10, 25, 40, 55, 70, 85, 100, 115, 127] {
            buffer.add_sample(value, false, Some(7));
        }

        assert_eq!(buffer.analyze(), HardwareType::Fader);
    }

    #[test]
    fn test_matches() {
        let buffer = MidiSampleBuffer::new(1, 10, false);

        // Same control
        assert!(buffer.matches(1, 10, false));

        // Different channel
        assert!(!buffer.matches(2, 10, false));

        // Different type
        assert!(!buffer.matches(1, 10, true));

        // Paired CC (10 + 32 = 42)
        assert!(buffer.matches(1, 42, false));
    }
}
