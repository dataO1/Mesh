//! Controller input thread of live mode.
//!
//! iced rebuilds and redraws the view after every message, so polling the F1s
//! from a fast UI tick would also redraw at that rate. Instead this thread
//! owns the [`ControllerManager`] (built with an empty config, so HID events
//! arrive raw through `drain_hid_events`), polls it every
//! [`POLL_INTERVAL`] and hands events to the [`LiveControlSurface`]. LED and
//! display feedback runs here at [`FEEDBACK_INTERVAL`] (~30 Hz). The UI ticks
//! at 30 Hz on its own and only reads.
//!
//! The controller and the surface sit behind one mutex shared with the UI
//! (keyboard fallback, recording, readout). Neither side holds it for more
//! than a handful of commands, and the audio thread never touches it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mesh_core::live::engine::LiveController;
use mesh_core::live::state::LiveShared;
use mesh_midi::{ControllerManager, FeedbackCommand, MidiConfig};

use super::controls::LiveControlSurface;

/// Controller polling period. Bounds the input latency added on top of the
/// audio block.
pub const POLL_INTERVAL: Duration = Duration::from_millis(2);
/// LED / 7-segment refresh period.
pub const FEEDBACK_INTERVAL: Duration = Duration::from_millis(33);
/// Sent LED states are forgotten this often so a replugged unit relights.
const FEEDBACK_REFRESH: Duration = Duration::from_secs(2);

/// The live control state shared by the input thread and the UI.
pub struct LiveCtl {
    pub controller: LiveController,
    pub surface: LiveControlSurface,
}

pub type SharedCtl = Arc<Mutex<LiveCtl>>;

/// Lock the shared control state. A panic while holding it leaves the data
/// usable (every field is updated atomically by its own methods).
pub fn lock(ctl: &SharedCtl) -> MutexGuard<'_, LiveCtl> {
    ctl.lock().unwrap_or_else(|e| e.into_inner())
}

/// Handle of the running input thread. Dropping it stops the thread.
pub struct InputThread {
    stop: Arc<AtomicBool>,
    hid_devices: Arc<AtomicUsize>,
    handle: Option<JoinHandle<()>>,
}

impl InputThread {
    pub fn spawn(ctl: SharedCtl, shared: Arc<LiveShared>) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let hid_devices = Arc::new(AtomicUsize::new(0));
        let (stop2, hid2) = (Arc::clone(&stop), Arc::clone(&hid_devices));
        let handle = std::thread::Builder::new()
            .name("live-input".into())
            .spawn(move || run(ctl, shared, stop2, hid2))?;
        Ok(Self { stop, hid_devices, handle: Some(handle) })
    }

    /// Connected HID controllers.
    pub fn hid_devices(&self) -> usize {
        self.hid_devices.load(Ordering::Relaxed)
    }
}

impl Drop for InputThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// LED state key for de-duplication: brightness or packed RGB.
fn led_value(cmd: &FeedbackCommand) -> Option<(&str, u32)> {
    match cmd {
        FeedbackCommand::SetLed { control, brightness } => Some((control, *brightness as u32)),
        FeedbackCommand::SetRgb { control, r, g, b } => {
            Some((control, 1 << 24 | (*r as u32) << 16 | (*g as u32) << 8 | *b as u32))
        }
        FeedbackCommand::SetDisplay { .. } => None,
    }
}

fn run(ctl: SharedCtl, shared: Arc<LiveShared>, stop: Arc<AtomicBool>, hid_devices: Arc<AtomicUsize>) {
    let mut manager = match ControllerManager::new_with_config(MidiConfig::default()) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("[LIVE] controllers unavailable: {e}");
            return;
        }
    };
    let connected = manager.hid_device_ids().len();
    hid_devices.store(connected, Ordering::Relaxed);
    log::info!("[LIVE] input thread started, {connected} HID device(s)");

    let mut leds = Vec::new();
    let mut displays = Vec::new();
    let mut sent: HashMap<(String, String), u32> = HashMap::new();
    let mut next_feedback = Instant::now();
    let mut next_refresh = Instant::now() + FEEDBACK_REFRESH;

    while !stop.load(Ordering::Relaxed) {
        // Hot-plug and health checks; MIDI messages are not used in live mode.
        let _ = manager.drain();
        let events = manager.drain_hid_events();
        if !events.is_empty() {
            let mut guard = lock(&ctl);
            let LiveCtl { controller, surface } = &mut *guard;
            for ev in &events {
                surface.handle(ev, controller);
            }
        }

        let now = Instant::now();
        if now >= next_feedback {
            next_feedback = now + FEEDBACK_INTERVAL;
            {
                let mut guard = lock(&ctl);
                let LiveCtl { controller, surface } = &mut *guard;
                surface.feedback(&shared, controller, &mut leds, &mut displays);
            }
            if now >= next_refresh {
                next_refresh = now + FEEDBACK_REFRESH;
                sent.clear();
                hid_devices.store(manager.hid_device_ids().len(), Ordering::Relaxed);
            }
            for (device, cmd) in leds.drain(..) {
                if let Some((control, value)) = led_value(&cmd) {
                    let key = (device.clone(), control.to_string());
                    if sent.get(&key) == Some(&value) {
                        continue;
                    }
                    if manager.send_hid_feedback(&device, cmd) {
                        sent.insert(key, value);
                    }
                } else {
                    manager.send_hid_feedback(&device, cmd);
                }
            }
            for (device, text) in displays.drain(..) {
                // De-duplicated by the output handler.
                manager.send_hid_display(&device, &text);
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
