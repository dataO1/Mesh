//! HID device backend
//!
//! Provides USB HID device discovery, connection, and I/O thread management.
//! Each connected HID device gets a dedicated I/O thread that reads input reports
//! and writes output reports (for LED/display feedback).

pub mod devices;
pub mod thread;

use crate::types::{ControlEvent, FeedbackCommand};
use flume::Sender;
use hidapi::HidApi;
use thread::HidIoThread;

/// Information about a discovered HID device
#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    /// USB Vendor ID
    pub vendor_id: u16,
    /// USB Product ID
    pub product_id: u16,
    /// Device serial number (if available)
    pub serial: Option<String>,
    /// Device filesystem path
    pub path: String,
    /// Product name (from USB descriptor or driver registry)
    pub product_name: String,
}

/// A connected HID device with its I/O thread
pub struct HidConnection {
    /// I/O thread handle (owns the thread lifetime)
    _io_thread: HidIoThread,
    /// Sender for feedback commands (written by output handler)
    feedback_tx: Sender<FeedbackCommand>,
    /// Device info
    pub info: HidDeviceInfo,
}

impl HidConnection {
    /// Get the feedback command sender (for the output handler)
    pub fn feedback_sender(&self) -> Sender<FeedbackCommand> {
        self.feedback_tx.clone()
    }
}

/// Enumerate all known HID devices currently connected
///
/// Only returns devices with matching VID/PID in the driver registry.
pub fn enumerate_devices() -> Vec<HidDeviceInfo> {
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            log::warn!("HID: Failed to initialize hidapi: {}", e);
            return Vec::new();
        }
    };

    let mut found = Vec::new();

    for device_info in api.device_list() {
        let vid = device_info.vendor_id();
        let pid = device_info.product_id();

        if devices::is_known_device(vid, pid) {
            let name = devices::device_name(vid, pid)
                .unwrap_or("Unknown HID Device")
                .to_string();

            let serial = device_info
                .serial_number()
                .map(|s| s.to_string());

            let path = device_info.path().to_string_lossy().to_string();

            log::info!(
                "HID: Found '{}' (VID={:#06x} PID={:#06x}) at {}",
                name, vid, pid, path
            );

            found.push(HidDeviceInfo {
                vendor_id: vid,
                product_id: pid,
                serial,
                path,
                product_name: name,
            });
        }
    }

    found
}

/// Connect to a HID device and spawn its I/O thread
///
/// - `info`: Device to connect to (from enumerate_devices())
/// - `event_tx`: Channel to send parsed ControlEvents (shared with MIDI events)
///
/// Returns the connection handle and control descriptors from the driver.
/// Descriptors are collected before the driver is moved into the I/O thread.
pub fn connect_device(
    info: &HidDeviceInfo,
    event_tx: Sender<ControlEvent>,
) -> Result<(HidConnection, Vec<crate::types::ControlDescriptor>), String> {
    // Create driver for this device
    let driver = devices::create_driver(info.vendor_id, info.product_id)
        .ok_or_else(|| format!("No driver for VID={:#06x} PID={:#06x}", info.vendor_id, info.product_id))?;

    // Collect control descriptors before moving driver into I/O thread
    let descriptors = driver.controls().to_vec();

    // Open the HID device
    let api = HidApi::new().map_err(|e| format!("Failed to init hidapi: {}", e))?;
    let device = api
        .open_path(std::ffi::CString::new(info.path.clone()).unwrap().as_ref())
        .map_err(|e| format!("Failed to open HID device at {}: {}", info.path, e))?;

    // Set non-blocking mode (the I/O thread uses read_timeout instead)
    device
        .set_blocking_mode(false)
        .map_err(|e| format!("Failed to set non-blocking mode: {}", e))?;

    // Create feedback channel
    let (feedback_tx, feedback_rx) = flume::bounded::<FeedbackCommand>(64);

    // Spawn I/O thread
    let io_thread = HidIoThread::spawn(
        device,
        driver,
        event_tx,
        feedback_rx,
        info.product_name.clone(),
    );

    log::info!("HID: Connected to '{}' at {} ({} controls)", info.product_name, info.path, descriptors.len());

    Ok((HidConnection {
        _io_thread: io_thread,
        feedback_tx,
        info: info.clone(),
    }, descriptors))
}

/// Output handler for HID feedback
///
/// Translates FeedbackResults into FeedbackCommands and sends them to the
/// device's I/O thread via channel.
pub struct HidOutputHandler {
    feedback_tx: Sender<FeedbackCommand>,
    change_tracker: crate::feedback::FeedbackChangeTracker,
}

impl HidOutputHandler {
    /// Create a new output handler from a connected device
    pub fn new(feedback_tx: Sender<FeedbackCommand>) -> Self {
        Self {
            feedback_tx,
            change_tracker: crate::feedback::FeedbackChangeTracker::new(),
        }
    }

    /// Apply evaluated feedback results for HID controls
    pub fn apply_feedback(&mut self, results: &[crate::feedback::FeedbackResult]) {
        for result in results {
            // Only handle HID addresses
            if let crate::types::ControlAddress::Hid { name } = &result.address {
                if let Some(value) = self.change_tracker.update(&result.address, result.value) {
                    // Use RGB color if available, otherwise fall back to brightness
                    let cmd = if let Some([r, g, b]) = result.color {
                        FeedbackCommand::SetRgb {
                            control: name.clone(),
                            r, g, b,
                        }
                    } else {
                        FeedbackCommand::SetLed {
                            control: name.clone(),
                            brightness: value,
                        }
                    };
                    if self.feedback_tx.try_send(cmd).is_err() {
                        log::warn!("HID: Feedback channel full");
                    }
                }
            }
        }
    }
}
