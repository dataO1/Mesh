//! HID device driver registry
//!
//! Maps USB VID/PID pairs to device-specific protocol drivers.
//! Each driver knows how to parse its device's input reports and
//! build output reports for LED/display feedback.

pub mod kontrol_f1;

use crate::types::{ControlDescriptor, ControlEvent, FeedbackCommand};

/// Trait for HID device protocol drivers
///
/// Each supported HID device implements this trait. The driver translates
/// between raw HID reports and abstract control events/feedback commands.
pub trait HidDeviceDriver: Send {
    /// Parse a raw HID input report into control events
    ///
    /// Uses delta detection: only emits events for controls that changed
    /// since the last input report. Returns an empty vec if nothing changed.
    fn parse_input(&mut self, data: &[u8]) -> Vec<ControlEvent>;

    /// Apply a feedback command to the output report buffer
    ///
    /// Writes the command's data into the appropriate bytes of the output buffer.
    /// The caller is responsible for sending the buffer to the device.
    fn apply_feedback(&mut self, output: &mut [u8], cmd: FeedbackCommand);

    /// Size of the output report (including report ID byte)
    fn output_report_size(&self) -> usize;

    /// Report ID for the output report
    fn output_report_id(&self) -> u8;

    /// Get descriptors for all controls on this device
    ///
    /// Used by learn mode to show human-readable names and skip hardware
    /// type detection (the driver already knows the control type).
    fn controls(&self) -> &[ControlDescriptor];
}

/// Known HID device entry
struct KnownDevice {
    vendor_id: u16,
    product_id: u16,
    name: &'static str,
    create: fn(String) -> Box<dyn HidDeviceDriver>,
}

/// Registry of known HID devices
static KNOWN_DEVICES: &[KnownDevice] = &[
    KnownDevice {
        vendor_id: kontrol_f1::VID,
        product_id: kontrol_f1::PID,
        name: "Traktor Kontrol F1",
        create: |id| Box::new(kontrol_f1::KontrolF1Driver::new(id)),
    },
];

/// Create a driver for a known HID device, or None if unrecognized
///
/// `device_id` uniquely identifies this physical device instance (typically the USB serial number).
pub fn create_driver(vendor_id: u16, product_id: u16, device_id: String) -> Option<Box<dyn HidDeviceDriver>> {
    KNOWN_DEVICES
        .iter()
        .find(|d| d.vendor_id == vendor_id && d.product_id == product_id)
        .map(|d| (d.create)(device_id))
}

/// Check if a VID/PID pair is a known supported device
pub fn is_known_device(vendor_id: u16, product_id: u16) -> bool {
    KNOWN_DEVICES
        .iter()
        .any(|d| d.vendor_id == vendor_id && d.product_id == product_id)
}

/// Get the name for a known device
pub fn device_name(vendor_id: u16, product_id: u16) -> Option<&'static str> {
    KNOWN_DEVICES
        .iter()
        .find(|d| d.vendor_id == vendor_id && d.product_id == product_id)
        .map(|d| d.name)
}
