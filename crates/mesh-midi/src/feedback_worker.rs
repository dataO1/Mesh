//! Background thread for HID LED feedback evaluation
//!
//! Moves feedback evaluation (evaluate_feedback + apply_feedback) off the UI thread.
//! The UI thread just sends a lightweight FeedbackState (~120 bytes) via a bounded channel.
//! The worker evaluates all feedback mappings and sends HID commands to the device I/O threads.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use flume::{Receiver, Sender};

use crate::config::FeedbackMapping;
use crate::feedback::{evaluate_feedback, FeedbackChangeTracker, FeedbackState};
use crate::shared_state::SharedState;
use crate::types::{ControlAddress, FeedbackCommand};

/// Per-HID-device feedback data owned by the worker thread
struct HidDeviceFeedback {
    /// Feedback mappings from device profile
    mappings: Vec<FeedbackMapping>,
    /// Channel to the device's I/O thread
    output_tx: Sender<FeedbackCommand>,
    /// Tracks last-sent values to avoid redundant sends
    change_tracker: FeedbackChangeTracker,
    /// Shared state for physical→virtual deck resolution
    shared_state: Arc<SharedState>,
    /// Device ID for filtering results
    device_id: String,
    /// Last result count for stale address detection
    last_result_count: usize,
}

impl HidDeviceFeedback {
    /// Evaluate feedback and send changed values to the device
    fn update(&mut self, state: &FeedbackState) {
        let deck_target = self.shared_state.deck_target.read()
            .map(|dt| dt.clone())
            .unwrap_or_default();

        let results = evaluate_feedback(&self.mappings, state, &deck_target);

        let mut device_result_count = 0usize;

        for result in &results {
            if let ControlAddress::Hid { device_id, name } = &result.address {
                if device_id != &self.device_id {
                    continue;
                }
                device_result_count += 1;
                if self.change_tracker.update(&result.address, result.value, result.color) {
                    let cmd = if let Some([r, g, b]) = result.color {
                        FeedbackCommand::SetRgb {
                            control: name.clone(),
                            r, g, b,
                        }
                    } else {
                        FeedbackCommand::SetLed {
                            control: name.clone(),
                            brightness: result.value,
                        }
                    };
                    let _ = self.output_tx.try_send(cmd);
                }
            }
        }

        // Stale address clearing on mode switch (result count changed)
        if device_result_count != self.last_result_count {
            self.last_result_count = device_result_count;

            let current_names: std::collections::HashSet<&str> = results.iter()
                .filter_map(|r| {
                    if let ControlAddress::Hid { device_id, name } = &r.address {
                        if device_id == &self.device_id { Some(name.as_str()) } else { None }
                    } else {
                        None
                    }
                })
                .collect();

            let stale: Vec<(ControlAddress, String)> = self.change_tracker.tracked_addresses()
                .filter_map(|addr| {
                    if let ControlAddress::Hid { device_id, name } = addr {
                        if device_id == &self.device_id && !current_names.contains(name.as_str()) {
                            return Some((addr.clone(), name.clone()));
                        }
                    }
                    None
                })
                .collect();

            for (addr, name) in &stale {
                if self.change_tracker.update(addr, 0, Some([0, 0, 0])) {
                    let cmd = FeedbackCommand::SetRgb {
                        control: name.clone(),
                        r: 0, g: 0, b: 0,
                    };
                    let _ = self.output_tx.try_send(cmd);
                }
            }
        }
    }
}

/// Registration info for a HID device to be added to the feedback worker
pub struct HidFeedbackRegistration {
    pub mappings: Vec<FeedbackMapping>,
    pub output_tx: Sender<FeedbackCommand>,
    pub shared_state: Arc<SharedState>,
    pub device_id: String,
}

/// Background feedback evaluation worker
///
/// Receives FeedbackState from the UI thread and evaluates HID LED feedback
/// on a dedicated thread, keeping the UI thread responsive.
pub struct FeedbackWorker {
    state_tx: Sender<FeedbackState>,
    _thread: JoinHandle<()>,
}

impl FeedbackWorker {
    /// Create a new feedback worker with the given HID devices
    pub fn new(devices: Vec<HidFeedbackRegistration>) -> Self {
        let (tx, rx) = flume::bounded::<FeedbackState>(2);

        let thread = std::thread::Builder::new()
            .name("feedback-worker".into())
            .spawn(move || {
                Self::run(rx, devices);
            })
            .expect("Failed to spawn feedback worker thread");

        Self {
            state_tx: tx,
            _thread: thread,
        }
    }

    fn run(rx: Receiver<FeedbackState>, registrations: Vec<HidFeedbackRegistration>) {
        let mut devices: Vec<HidDeviceFeedback> = registrations.into_iter()
            .map(|reg| HidDeviceFeedback {
                mappings: reg.mappings,
                output_tx: reg.output_tx,
                change_tracker: FeedbackChangeTracker::new(),
                shared_state: reg.shared_state,
                device_id: reg.device_id,
                last_result_count: 0,
            })
            .collect();

        log::info!("Feedback worker started with {} HID device(s)", devices.len());

        loop {
            // Block waiting for state
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(state) => {
                    // Drain any queued states, use only the latest
                    let mut latest = state;
                    while let Ok(newer) = rx.try_recv() {
                        latest = newer;
                    }

                    // Evaluate and send for each HID device
                    for dev in &mut devices {
                        dev.update(&latest);
                    }
                }
                Err(flume::RecvTimeoutError::Disconnected) => {
                    log::info!("Feedback worker: channel disconnected, shutting down");
                    break;
                }
                Err(flume::RecvTimeoutError::Timeout) => {
                    // No state updates — just loop
                    continue;
                }
            }
        }
    }

    /// Send feedback state to the worker (non-blocking, drops if full)
    pub fn send(&self, state: &FeedbackState) {
        // try_send with bounded(2): drops if worker is behind, ensuring
        // we always process the latest state rather than queuing stale ones
        let _ = self.state_tx.try_send(state.clone());
    }
}
