//! HID I/O thread
//!
//! Dedicated thread per HID device. Reads input reports, parses them via the
//! device driver, and sends resulting ControlEvents to the shared channel.
//! Also drains pending feedback commands and writes output reports.

use super::devices::HidDeviceDriver;
use crate::types::{ControlEvent, FeedbackCommand};
use flume::{Receiver, Sender};
use hidapi::HidDevice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

/// HID I/O thread handle
///
/// Owns the thread join handle and a shutdown flag.
/// When dropped, signals the thread to stop and waits for it.
pub struct HidIoThread {
    /// Shutdown signal
    shutdown: Arc<AtomicBool>,
    /// Thread join handle
    handle: Option<thread::JoinHandle<()>>,
    /// Device name (for logging)
    device_name: String,
    /// Whether the I/O loop is still running (set to false on exit)
    alive: Arc<AtomicBool>,
}

impl HidIoThread {
    /// Spawn a new I/O thread for a HID device
    ///
    /// - `device`: The hidapi device handle (must be opened with non-blocking mode)
    /// - `driver`: Protocol driver for this specific device
    /// - `event_tx`: Channel to send parsed ControlEvents
    /// - `feedback_rx`: Channel to receive FeedbackCommands
    /// - `device_name`: Human-readable name for logging
    pub fn spawn(
        device: HidDevice,
        mut driver: Box<dyn HidDeviceDriver>,
        event_tx: Sender<ControlEvent>,
        feedback_rx: Receiver<FeedbackCommand>,
        device_name: String,
    ) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let alive_clone = alive.clone();
        let name = device_name.clone();

        let handle = thread::Builder::new()
            .name(format!("hid-io-{}", device_name))
            .spawn(move || {
                Self::io_loop(device, &mut *driver, event_tx, feedback_rx, shutdown_clone, &name);
                alive_clone.store(false, Ordering::Relaxed);
            })
            .expect("Failed to spawn HID I/O thread");

        Self {
            shutdown,
            handle: Some(handle),
            device_name,
            alive,
        }
    }

    /// Check if the I/O loop is still running
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Main I/O loop running on the dedicated thread
    fn io_loop(
        device: HidDevice,
        driver: &mut dyn HidDeviceDriver,
        event_tx: Sender<ControlEvent>,
        feedback_rx: Receiver<FeedbackCommand>,
        shutdown: Arc<AtomicBool>,
        name: &str,
    ) {
        log::info!("[HID {}] I/O thread started", name);

        // Input read buffer
        let mut input_buf = vec![0u8; 64]; // Most HID reports are ≤64 bytes

        // Output report buffer (initialized with report ID)
        let output_size = driver.output_report_size();
        let mut output_buf = vec![0u8; output_size];
        output_buf[0] = driver.output_report_id();
        let mut output_dirty = false;

        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }

            // ─── Input: non-blocking read with 1ms timeout ───
            match device.read_timeout(&mut input_buf, 1) {
                Ok(n) if n > 0 => {
                    let events = driver.parse_input(&input_buf[..n]);
                    for event in events {
                        log::debug!("[HID {}] {:?}", name, event);
                        if event_tx.try_send(event).is_err() {
                            log::warn!("[HID {}] Event channel full, dropping event", name);
                        }
                    }
                }
                Ok(_) => {} // Timeout, no data (expected)
                Err(e) => {
                    log::error!("[HID {}] Read error: {}", name, e);
                    break; // Device disconnected
                }
            }

            // ─── Output: drain pending feedback commands ───
            while let Ok(cmd) = feedback_rx.try_recv() {
                driver.apply_feedback(&mut output_buf, cmd);
                output_dirty = true;
            }

            // Write output report if anything changed
            if output_dirty {
                // Hex dump of non-zero output bytes for debugging
                if log::log_enabled!(log::Level::Trace) {
                    let non_zero: Vec<String> = output_buf.iter().enumerate()
                        .filter(|(i, b)| *i > 0 && **b != 0)
                        .map(|(i, b)| format!("[{:2}]={:#04x}", i, b))
                        .collect();
                    if !non_zero.is_empty() {
                        log::trace!("[HID {}] Output: {}", name, non_zero.join(" "));
                    }
                }
                match device.write(&output_buf) {
                    Ok(_) => {
                        output_dirty = false;
                    }
                    Err(e) => {
                        log::error!("[HID {}] Write error: {}", name, e);
                        break; // Device disconnected
                    }
                }
            }
        }

        log::info!("[HID {}] I/O thread stopped", name);
    }
}

impl Drop for HidIoThread {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            log::debug!("[HID {}] Waiting for I/O thread to stop...", self.device_name);
            let _ = handle.join();
        }
    }
}
