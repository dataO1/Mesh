//! Native Instruments Traktor Kontrol F1 HID protocol driver
//!
//! The F1 communicates via USB HID with 22-byte input reports and 81-byte output reports.
//!
//! # Input Report (22 bytes)
//!
//! | Bytes | Content |
//! |-------|---------|
//! | 1     | Report ID (0x01) |
//! | 2-5   | Button bitmask (u32 LE, 29 buttons) |
//! | 6     | Encoder position (u8, wraps 0-255) |
//! | 7-14  | 4x knob values (u16 LE each, range 0-4092) |
//! | 15-22 | 4x fader values (u16 LE each, range 0-4092) |
//!
//! # Output Report (81 bytes)
//!
//! | Bytes | Content |
//! |-------|---------|
//! | 1     | Report ID (0x80) |
//! | 2-17  | 7-segment display (16 bytes, 4 digits × 4 segments) |
//! | 18-25 | Function button LEDs (8 bytes, brightness 0-127) |
//! | 26-73 | Grid pad RGB LEDs (48 bytes = 16 pads × 3 BRG) |
//! | 74-81 | Play button LEDs (8 bytes = 4 × 2 sub-LEDs, 0-255) |

use super::{HidDeviceDriver};
use crate::config::HardwareType;
use crate::types::{ControlAddress, ControlDescriptor, ControlEvent, ControlValue, FeedbackCommand};

/// USB Vendor ID for Native Instruments
pub const VID: u16 = 0x17CC;
/// USB Product ID for Kontrol F1
pub const PID: u16 = 0x1120;

/// Input report size in bytes
const INPUT_SIZE: usize = 22;
/// Output report size in bytes (including report ID)
const OUTPUT_SIZE: usize = 81;
/// Output report ID
const OUTPUT_REPORT_ID: u8 = 0x80;

/// Maximum analog value from F1 knobs/faders (12-bit ADC)
const ANALOG_MAX: u16 = 4092;
/// Minimum change in analog value to trigger an event (noise filtering)
const ANALOG_DEADZONE: u16 = 2;

// Button bit positions in the 32-bit bitmask (bytes 1-4, little-endian)
// Determined by empirical testing with the hardware
const BTN_BROWSE: u32     = 1 << 0;
const BTN_SIZE: u32       = 1 << 1;
const BTN_TYPE: u32       = 1 << 2;
const BTN_REVERSE: u32    = 1 << 3;
const BTN_SHIFT: u32      = 1 << 4;
const BTN_CAPTURE: u32    = 1 << 5;
const BTN_QUANT: u32      = 1 << 6;
const BTN_SYNC: u32       = 1 << 7;
const BTN_PLAY_1: u32     = 1 << 8;
const BTN_PLAY_2: u32     = 1 << 9;
const BTN_PLAY_3: u32     = 1 << 10;
const BTN_PLAY_4: u32     = 1 << 11;
const BTN_ENCODER: u32    = 1 << 12;

// Grid pad buttons: bits 16-31 (pad 1 = bit 16, pad 16 = bit 31)
// Layout (looking at the F1 from the front, top-left = pad 13):
//   13 14 15 16   (top row)
//    9 10 11 12
//    5  6  7  8
//    1  2  3  4   (bottom row)
const BTN_PAD_BASE: u32 = 16;

/// Kontrol F1 HID driver
pub struct KontrolF1Driver {
    /// Previous input report for delta detection
    prev_input: [u8; INPUT_SIZE],
    /// Whether we've received at least one report (for initialization)
    has_prev: bool,
    /// Control descriptors (built once)
    descriptors: Vec<ControlDescriptor>,
}

impl KontrolF1Driver {
    pub fn new() -> Self {
        Self {
            prev_input: [0; INPUT_SIZE],
            has_prev: false,
            descriptors: Self::build_descriptors(),
        }
    }

    /// Build control descriptors for all F1 controls
    fn build_descriptors() -> Vec<ControlDescriptor> {
        let mut descs = Vec::with_capacity(49);

        // 16 grid pads (RGB)
        for i in 1..=16 {
            descs.push(ControlDescriptor {
                address: ControlAddress::Hid { name: format!("grid_{}", i) },
                name: format!("Grid Pad {}", i),
                control_type: HardwareType::Button,
                has_led: false,
                has_rgb: true,
            });
        }

        // 4 play buttons (single-color LED)
        for i in 1..=4 {
            descs.push(ControlDescriptor {
                address: ControlAddress::Hid { name: format!("play_{}", i) },
                name: format!("Play {}", i),
                control_type: HardwareType::Button,
                has_led: true,
                has_rgb: false,
            });
        }

        // Function buttons
        for (name, label, has_led) in [
            ("browse", "Browse", true),
            ("size", "Size", true),
            ("type_btn", "Type", true),
            ("reverse", "Reverse", true),
            ("shift", "Shift", true),
            ("capture", "Capture", true),
            ("quant", "Quant", true),
            ("sync", "Sync", true),
            ("encoder_push", "Encoder Push", false),
        ] {
            descs.push(ControlDescriptor {
                address: ControlAddress::Hid { name: name.to_string() },
                name: label.to_string(),
                control_type: HardwareType::Button,
                has_led,
                has_rgb: false,
            });
        }

        // 4 knobs
        for i in 1..=4 {
            descs.push(ControlDescriptor {
                address: ControlAddress::Hid { name: format!("knob_{}", i) },
                name: format!("Knob {}", i),
                control_type: HardwareType::Knob,
                has_led: false,
                has_rgb: false,
            });
        }

        // 4 faders
        for i in 1..=4 {
            descs.push(ControlDescriptor {
                address: ControlAddress::Hid { name: format!("fader_{}", i) },
                name: format!("Fader {}", i),
                control_type: HardwareType::Fader,
                has_led: false,
                has_rgb: false,
            });
        }

        // Encoder
        descs.push(ControlDescriptor {
            address: ControlAddress::Hid { name: "encoder".to_string() },
            name: "Encoder".to_string(),
            control_type: HardwareType::Encoder,
            has_led: false,
            has_rgb: false,
        });

        descs
    }

    /// Parse button bitmask changes into events
    fn parse_buttons(&self, current: u32, previous: u32) -> Vec<ControlEvent> {
        let mut events = Vec::new();
        let changed = current ^ previous;
        if changed == 0 {
            return events;
        }

        // Function buttons
        let function_buttons: &[(&str, u32)] = &[
            ("browse", BTN_BROWSE),
            ("size", BTN_SIZE),
            ("type_btn", BTN_TYPE),
            ("reverse", BTN_REVERSE),
            ("shift", BTN_SHIFT),
            ("capture", BTN_CAPTURE),
            ("quant", BTN_QUANT),
            ("sync", BTN_SYNC),
            ("encoder_push", BTN_ENCODER),
        ];

        for (name, mask) in function_buttons {
            if changed & mask != 0 {
                events.push(ControlEvent {
                    address: ControlAddress::Hid { name: name.to_string() },
                    value: ControlValue::Button(current & mask != 0),
                });
            }
        }

        // Play buttons
        let play_buttons: &[(&str, u32)] = &[
            ("play_1", BTN_PLAY_1),
            ("play_2", BTN_PLAY_2),
            ("play_3", BTN_PLAY_3),
            ("play_4", BTN_PLAY_4),
        ];

        for (name, mask) in play_buttons {
            if changed & mask != 0 {
                events.push(ControlEvent {
                    address: ControlAddress::Hid { name: name.to_string() },
                    value: ControlValue::Button(current & mask != 0),
                });
            }
        }

        // Grid pads (16 pads, bits 16-31)
        for i in 0..16u32 {
            let mask = 1 << (BTN_PAD_BASE + i);
            if changed & mask != 0 {
                events.push(ControlEvent {
                    address: ControlAddress::Hid { name: format!("grid_{}", i + 1) },
                    value: ControlValue::Button(current & mask != 0),
                });
            }
        }

        events
    }

    /// Parse encoder change into event (wrap-around aware)
    fn parse_encoder(&self, current: u8, previous: u8) -> Option<ControlEvent> {
        if current == previous {
            return None;
        }

        // Handle wrap-around: the encoder position is 0-255 and wraps
        let delta = (current as i16 - previous as i16 + 128 + 256) % 256 - 128;
        if delta == 0 {
            return None;
        }

        Some(ControlEvent {
            address: ControlAddress::Hid { name: "encoder".to_string() },
            value: ControlValue::Relative(delta as i32),
        })
    }

    /// Parse analog value (knob or fader) change into event
    fn parse_analog(
        &self,
        name: &str,
        current_bytes: &[u8],
        previous_bytes: &[u8],
    ) -> Option<ControlEvent> {
        if current_bytes.len() < 2 || previous_bytes.len() < 2 {
            return None;
        }

        let current = u16::from_le_bytes([current_bytes[0], current_bytes[1]]);
        let previous = u16::from_le_bytes([previous_bytes[0], previous_bytes[1]]);

        // Apply deadzone to filter noise
        if current.abs_diff(previous) < ANALOG_DEADZONE {
            return None;
        }

        let normalized = (current as f64) / (ANALOG_MAX as f64);
        let normalized = normalized.clamp(0.0, 1.0);

        Some(ControlEvent {
            address: ControlAddress::Hid { name: name.to_string() },
            value: ControlValue::Absolute(normalized),
        })
    }

    /// Read a u32 from 4 bytes (little-endian) in the input report
    fn read_buttons(data: &[u8]) -> u32 {
        if data.len() < 5 {
            return 0;
        }
        u32::from_le_bytes([data[1], data[2], data[3], data[4]])
    }
}

impl HidDeviceDriver for KontrolF1Driver {
    fn parse_input(&mut self, data: &[u8]) -> Vec<ControlEvent> {
        if data.len() < INPUT_SIZE {
            return Vec::new();
        }

        // First report: store as baseline, emit no events
        if !self.has_prev {
            self.prev_input[..INPUT_SIZE].copy_from_slice(&data[..INPUT_SIZE]);
            self.has_prev = true;
            return Vec::new();
        }

        let mut events = Vec::new();

        // Buttons (bytes 1-4)
        let current_buttons = Self::read_buttons(data);
        let previous_buttons = Self::read_buttons(&self.prev_input);
        events.extend(self.parse_buttons(current_buttons, previous_buttons));

        // Encoder (byte 5)
        if let Some(event) = self.parse_encoder(data[5], self.prev_input[5]) {
            events.push(event);
        }

        // Knobs (bytes 6-13, four u16 LE values)
        for i in 0..4 {
            let offset = 6 + i * 2;
            if let Some(event) = self.parse_analog(
                &format!("knob_{}", i + 1),
                &data[offset..offset + 2],
                &self.prev_input[offset..offset + 2],
            ) {
                events.push(event);
            }
        }

        // Faders (bytes 14-21, four u16 LE values)
        for i in 0..4 {
            let offset = 14 + i * 2;
            if let Some(event) = self.parse_analog(
                &format!("fader_{}", i + 1),
                &data[offset..offset + 2],
                &self.prev_input[offset..offset + 2],
            ) {
                events.push(event);
            }
        }

        // Store current as previous for next delta
        self.prev_input[..INPUT_SIZE].copy_from_slice(&data[..INPUT_SIZE]);

        events
    }

    fn apply_feedback(&mut self, output: &mut [u8], cmd: FeedbackCommand) {
        if output.len() < OUTPUT_SIZE {
            return;
        }

        match cmd {
            FeedbackCommand::SetLed { ref control, brightness } => {
                // Function button LEDs: bytes 17-24 (brightness 0-127)
                if let Some(offset) = function_button_led_offset(control) {
                    output[offset] = brightness.min(127);
                }
                // Play button LEDs: bytes 73-80 (2 sub-LEDs per button, 0-255)
                else if let Some(offset) = play_button_led_offset(control) {
                    // Both sub-LEDs to same brightness (scaled 0-127 → 0-255)
                    let scaled = (brightness as u16 * 255 / 127).min(255) as u8;
                    output[offset] = scaled;
                    output[offset + 1] = scaled;
                }
            }
            FeedbackCommand::SetRgb { ref control, r, g, b } => {
                // Grid pad RGB: bytes 25-72 (16 pads × 3 bytes BRG order)
                if let Some(offset) = grid_pad_rgb_offset(control) {
                    output[offset] = b.min(127);     // Blue
                    output[offset + 1] = r.min(127); // Red
                    output[offset + 2] = g.min(127); // Green
                }
            }
            FeedbackCommand::SetDisplay { ref text } => {
                // 7-segment display: bytes 1-16 (4 digits × 4 segment bytes each)
                encode_7segment(text, &mut output[1..17]);
            }
        }
    }

    fn output_report_size(&self) -> usize {
        OUTPUT_SIZE
    }

    fn output_report_id(&self) -> u8 {
        OUTPUT_REPORT_ID
    }

    fn controls(&self) -> &[ControlDescriptor] {
        &self.descriptors
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Output report helpers
// ═══════════════════════════════════════════════════════════════════════

/// Map function button name to its LED byte offset in the output report.
/// Function button LEDs occupy bytes 17-24 (0-indexed from report start).
fn function_button_led_offset(name: &str) -> Option<usize> {
    match name {
        "browse"  => Some(17),
        "size"    => Some(18),
        "type_btn" => Some(19),
        "reverse" => Some(20),
        "shift"   => Some(21),
        "capture" => Some(22),
        "quant"   => Some(23),
        "sync"    => Some(24),
        _ => None,
    }
}

/// Map play button name to its LED byte offset (first of 2 sub-LED bytes).
/// Play button LEDs occupy bytes 73-80: play_1 = [73,74], play_2 = [75,76], etc.
fn play_button_led_offset(name: &str) -> Option<usize> {
    match name {
        "play_1" => Some(73),
        "play_2" => Some(75),
        "play_3" => Some(77),
        "play_4" => Some(79),
        _ => None,
    }
}

/// Map grid pad name to its RGB byte offset (first of 3 BRG bytes).
/// Grid pad RGB LEDs occupy bytes 25-72: grid_1 = [25,26,27], grid_2 = [28,29,30], etc.
fn grid_pad_rgb_offset(name: &str) -> Option<usize> {
    // Parse "grid_N" where N is 1-16
    let n: usize = name.strip_prefix("grid_")?.parse().ok()?;
    if n >= 1 && n <= 16 {
        Some(25 + (n - 1) * 3)
    } else {
        None
    }
}

/// 7-segment digit encoding lookup table.
/// Each digit is encoded as which of the 7 segments (a-g) are lit.
/// The F1 uses 4 bytes per digit position (segment groups).
///
/// Segment layout:
///  _a_
/// |   |
/// f   b
/// |_g_|
/// |   |
/// e   c
/// |_d_| .dp
///
/// The F1's 16-byte display area encodes 4 digits, each using 4 bytes that
/// control individual segment groups. The exact bit-to-segment mapping is
/// empirical. For simplicity, we use a nibble-based encoding.
const SEVEN_SEG: [u8; 16] = [
    0x3F, // 0: a b c d e f
    0x06, // 1: b c
    0x5B, // 2: a b d e g
    0x4F, // 3: a b c d g
    0x66, // 4: b c f g
    0x6D, // 5: a c d f g
    0x7D, // 6: a c d e f g
    0x07, // 7: a b c
    0x7F, // 8: a b c d e f g
    0x6F, // 9: a b c d f g
    0x77, // A: a b c e f g
    0x7C, // b: c d e f g
    0x39, // C: a d e f
    0x5E, // d: b c d e g
    0x79, // E: a d e f g
    0x71, // F: a e f g
];

/// Blank segment (all off)
const SEG_BLANK: u8 = 0x00;
/// Dash segment (g only)
const SEG_DASH: u8 = 0x40;

/// Encode a string into 7-segment display bytes (16 bytes for 4 digit positions).
///
/// The F1's display encoding uses 4 bytes per digit position where each byte
/// controls a pair of segments. We map the standard 7-segment encoding to
/// the F1's byte layout.
fn encode_7segment(text: &str, display: &mut [u8]) {
    if display.len() < 16 {
        return;
    }

    // Clear display
    display.iter_mut().for_each(|b| *b = 0);

    // Right-justify: fill from the right (digit 3 = rightmost)
    let chars: Vec<char> = text.chars().take(4).collect();
    let start = 4usize.saturating_sub(chars.len());

    for (i, ch) in chars.iter().enumerate() {
        let pos = start + i;
        let seg = match ch {
            '0'..='9' => SEVEN_SEG[(*ch as u8 - b'0') as usize],
            'a'..='f' => SEVEN_SEG[10 + (*ch as u8 - b'a') as usize],
            'A'..='F' => SEVEN_SEG[10 + (*ch as u8 - b'A') as usize],
            '-' => SEG_DASH,
            ' ' => SEG_BLANK,
            _ => SEG_DASH, // Unknown char = dash
        };

        // Map 7-segment bits to F1's 4-byte-per-digit layout.
        // Each digit position uses 4 consecutive bytes (4 × 4 = 16 total).
        // The F1 uses a direct segment-to-byte mapping: each byte in the
        // 4-byte group controls specific segments of that digit position.
        let base = pos * 4;
        // Byte 0: segments a,b (bits 0,1 of seg value)
        display[base] = ((seg & 0x01) << 0) | ((seg & 0x02) << 0);
        // Byte 1: segments c,g (bits 2,6)
        display[base + 1] = ((seg >> 2) & 0x01) | (((seg >> 6) & 0x01) << 1);
        // Byte 2: segments d,e (bits 3,4)
        display[base + 2] = ((seg >> 3) & 0x01) | (((seg >> 4) & 0x01) << 1);
        // Byte 3: segment f + dp (bit 5)
        display[base + 3] = (seg >> 5) & 0x01;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a minimal input report with all zeros
    fn make_report() -> [u8; INPUT_SIZE] {
        [0; INPUT_SIZE]
    }

    #[test]
    fn test_first_report_no_events() {
        let mut driver = KontrolF1Driver::new();
        let report = make_report();
        let events = driver.parse_input(&report);
        assert!(events.is_empty(), "First report should produce no events");
    }

    #[test]
    fn test_button_press() {
        let mut driver = KontrolF1Driver::new();

        // First report: baseline (all buttons released)
        let report1 = make_report();
        driver.parse_input(&report1);

        // Second report: browse button pressed (bit 0 of button bitmask)
        let mut report2 = make_report();
        report2[1] = 0x01; // Bit 0 = browse button
        let events = driver.parse_input(&report2);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0].address, ControlAddress::Hid { name } if name == "browse"));
        assert!(matches!(&events[0].value, ControlValue::Button(true)));
    }

    #[test]
    fn test_button_release() {
        let mut driver = KontrolF1Driver::new();

        // Baseline with browse pressed
        let mut report1 = make_report();
        report1[1] = 0x01;
        driver.parse_input(&report1);

        // Release browse
        let report2 = make_report();
        let events = driver.parse_input(&report2);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0].address, ControlAddress::Hid { name } if name == "browse"));
        assert!(matches!(&events[0].value, ControlValue::Button(false)));
    }

    #[test]
    fn test_grid_pad() {
        let mut driver = KontrolF1Driver::new();
        driver.parse_input(&make_report()); // Baseline

        // Press grid pad 1 (bit 16)
        let mut report = make_report();
        report[3] = 0x01; // Byte 3, bit 0 = overall bit 16 = grid_1
        let events = driver.parse_input(&report);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0].address, ControlAddress::Hid { name } if name == "grid_1"));
        assert!(matches!(&events[0].value, ControlValue::Button(true)));
    }

    #[test]
    fn test_encoder_clockwise() {
        let mut driver = KontrolF1Driver::new();

        // Baseline: encoder at position 100
        let mut report1 = make_report();
        report1[5] = 100;
        driver.parse_input(&report1);

        // Move clockwise to 103
        let mut report2 = make_report();
        report2[5] = 103;
        let events = driver.parse_input(&report2);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0].address, ControlAddress::Hid { name } if name == "encoder"));
        assert!(matches!(&events[0].value, ControlValue::Relative(3)));
    }

    #[test]
    fn test_encoder_wrap_around() {
        let mut driver = KontrolF1Driver::new();

        // Baseline: encoder at position 254
        let mut report1 = make_report();
        report1[5] = 254;
        driver.parse_input(&report1);

        // Wrap around clockwise to 2
        let mut report2 = make_report();
        report2[5] = 2;
        let events = driver.parse_input(&report2);

        assert_eq!(events.len(), 1);
        if let ControlValue::Relative(delta) = &events[0].value {
            assert_eq!(*delta, 4); // 254 → 2 = +4 (wrapped CW)
        } else {
            panic!("Expected Relative value");
        }
    }

    #[test]
    fn test_fader_movement() {
        let mut driver = KontrolF1Driver::new();
        driver.parse_input(&make_report()); // Baseline

        // Move fader 1 (bytes 14-15) to ~50%
        let mut report = make_report();
        let value: u16 = 2046; // ~50% of 4092
        let bytes = value.to_le_bytes();
        report[14] = bytes[0];
        report[15] = bytes[1];
        let events = driver.parse_input(&report);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0].address, ControlAddress::Hid { name } if name == "fader_1"));
        if let ControlValue::Absolute(v) = &events[0].value {
            assert!((*v - 0.5).abs() < 0.01, "Expected ~0.5, got {}", v);
        } else {
            panic!("Expected Absolute value");
        }
    }

    #[test]
    fn test_knob_deadzone() {
        let mut driver = KontrolF1Driver::new();

        // Baseline: knob 1 at value 100
        let mut report1 = make_report();
        let val: u16 = 100;
        let bytes = val.to_le_bytes();
        report1[6] = bytes[0];
        report1[7] = bytes[1];
        driver.parse_input(&report1);

        // Move knob 1 by 1 (within deadzone of 2)
        let mut report2 = make_report();
        let val2: u16 = 101;
        let bytes2 = val2.to_le_bytes();
        report2[6] = bytes2[0];
        report2[7] = bytes2[1];
        let events = driver.parse_input(&report2);

        assert!(events.is_empty(), "Movement within deadzone should be filtered");
    }

    #[test]
    fn test_multiple_simultaneous_changes() {
        let mut driver = KontrolF1Driver::new();
        driver.parse_input(&make_report()); // Baseline

        // Simultaneously: browse pressed + fader 1 moved
        let mut report = make_report();
        report[1] = 0x01; // browse button
        let value: u16 = 4092; // fader 1 max
        let bytes = value.to_le_bytes();
        report[14] = bytes[0];
        report[15] = bytes[1];
        let events = driver.parse_input(&report);

        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_descriptor_count() {
        let driver = KontrolF1Driver::new();
        let descs = driver.controls();
        // 16 pads + 4 play + 9 function + 4 knobs + 4 faders + 1 encoder = 38
        assert_eq!(descs.len(), 38);
    }

    #[test]
    fn test_no_change_no_events() {
        let mut driver = KontrolF1Driver::new();

        let report = make_report();
        driver.parse_input(&report); // Baseline

        // Same report again
        let events = driver.parse_input(&report);
        assert!(events.is_empty(), "Identical report should produce no events");
    }

    // ─── Output report tests ──────────────────────────────────────────

    fn make_output() -> [u8; OUTPUT_SIZE] {
        let mut buf = [0u8; OUTPUT_SIZE];
        buf[0] = OUTPUT_REPORT_ID;
        buf
    }

    #[test]
    fn test_function_button_led() {
        let mut driver = KontrolF1Driver::new();
        let mut output = make_output();

        driver.apply_feedback(&mut output, FeedbackCommand::SetLed {
            control: "browse".to_string(),
            brightness: 100,
        });
        assert_eq!(output[17], 100, "Browse LED at byte 17");

        driver.apply_feedback(&mut output, FeedbackCommand::SetLed {
            control: "sync".to_string(),
            brightness: 127,
        });
        assert_eq!(output[24], 127, "Sync LED at byte 24");
    }

    #[test]
    fn test_play_button_led() {
        let mut driver = KontrolF1Driver::new();
        let mut output = make_output();

        driver.apply_feedback(&mut output, FeedbackCommand::SetLed {
            control: "play_1".to_string(),
            brightness: 127,
        });
        // 127 scaled to 255
        assert_eq!(output[73], 255, "Play 1 sub-LED A at byte 73");
        assert_eq!(output[74], 255, "Play 1 sub-LED B at byte 74");

        driver.apply_feedback(&mut output, FeedbackCommand::SetLed {
            control: "play_3".to_string(),
            brightness: 64,
        });
        // 64 * 255 / 127 ≈ 128
        let expected = (64u16 * 255 / 127).min(255) as u8;
        assert_eq!(output[77], expected);
    }

    #[test]
    fn test_grid_pad_rgb() {
        let mut driver = KontrolF1Driver::new();
        let mut output = make_output();

        // Grid pad 1 = bytes 25,26,27 in BRG order
        driver.apply_feedback(&mut output, FeedbackCommand::SetRgb {
            control: "grid_1".to_string(),
            r: 100,
            g: 50,
            b: 75,
        });
        assert_eq!(output[25], 75, "Blue");
        assert_eq!(output[26], 100, "Red");
        assert_eq!(output[27], 50, "Green");

        // Grid pad 16 = bytes 25 + 15*3 = 70,71,72
        driver.apply_feedback(&mut output, FeedbackCommand::SetRgb {
            control: "grid_16".to_string(),
            r: 127,
            g: 127,
            b: 127,
        });
        assert_eq!(output[70], 127, "Blue (pad 16)");
        assert_eq!(output[71], 127, "Red (pad 16)");
        assert_eq!(output[72], 127, "Green (pad 16)");
    }

    #[test]
    fn test_grid_pad_rgb_offset_bounds() {
        // Verify grid_1 through grid_16 all resolve to valid offsets
        for i in 1..=16 {
            let name = format!("grid_{}", i);
            let offset = grid_pad_rgb_offset(&name);
            assert!(offset.is_some(), "grid_{} should have a valid offset", i);
            let off = offset.unwrap();
            assert!(off + 2 < OUTPUT_SIZE, "grid_{} BRG bytes must fit in output", i);
        }
        // grid_0 and grid_17 should not resolve
        assert!(grid_pad_rgb_offset("grid_0").is_none());
        assert!(grid_pad_rgb_offset("grid_17").is_none());
    }

    #[test]
    fn test_unknown_control_ignored() {
        let mut driver = KontrolF1Driver::new();
        let mut output = make_output();
        let original = output.clone();

        // Unknown control should not modify output
        driver.apply_feedback(&mut output, FeedbackCommand::SetLed {
            control: "nonexistent".to_string(),
            brightness: 127,
        });
        assert_eq!(output, original, "Unknown control should not modify output");
    }

    #[test]
    fn test_7segment_display() {
        let mut driver = KontrolF1Driver::new();
        let mut output = make_output();

        driver.apply_feedback(&mut output, FeedbackCommand::SetDisplay {
            text: "42".to_string(),
        });
        // Display bytes 1-16 should have non-zero content for digits
        // (exact values depend on encoding, just verify not all zeros)
        let display = &output[1..17];
        assert!(display.iter().any(|&b| b != 0), "Display should have content for '42'");
    }
}
