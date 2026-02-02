//! PdInstance - wrapper around libpd-rs for per-deck PD processing
//!
//! This wrapper provides a simplified API over libpd-rs tailored for mesh's needs.
//!
//! # Important: Single Global Pd Instance
//!
//! libpd is fundamentally single-threaded and does NOT support multiple calls to
//! `Pd::init_and_configure()`. We track initialization state to ensure libpd is
//! initialized exactly once, then reuse that instance.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use crossbeam::queue::SegQueue;
use libpd_rs::functions::receive::on_print;
use libpd_rs::functions::verbose_print_state;
use libpd_rs::{Pd, PdAudioContext};

use super::error::{PdError, PdResult};

/// One-time initialization for libpd (both the library AND print hook)
static LIBPD_INIT: Once = Once::new();

/// Tracks whether libpd has been initialized
static LIBPD_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Lock-free queue for PD console messages (RT-safe)
/// Messages are pushed from the audio thread and drained from non-RT contexts
static PD_MESSAGE_QUEUE: SegQueue<PdMessage> = SegQueue::new();

/// Flag to track if we have pending messages (avoids unnecessary queue checks)
static HAS_PENDING_MESSAGES: AtomicBool = AtomicBool::new(false);

/// A message from PD's console output
#[derive(Debug, Clone)]
pub struct PdMessage {
    pub text: String,
    pub level: PdMessageLevel,
}

/// Severity level for PD messages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdMessageLevel {
    Info,
    Warning,
    Error,
}

/// Initialize the global print hook for capturing PD console output
///
/// This uses a lock-free queue to safely capture messages from the audio thread.
/// Call `drain_pd_messages()` periodically from a non-RT context to log them.
///
/// NOTE: This is called during libpd initialization via LIBPD_INIT.
fn init_print_hook() {
    // Enable verbose printing to see external loading info
    verbose_print_state(true);

    // Register global print hook - called from DSP thread
    // CRITICAL: This callback runs on the audio thread - NO BLOCKING OPERATIONS!
    on_print(|msg: &str| {
        let msg = msg.trim();
        if msg.is_empty() {
            return;
        }

        // Determine message level based on content
        let msg_lower = msg.to_lowercase();
        let level = if msg_lower.contains("error")
            || msg_lower.contains("can't")
            || msg_lower.contains("couldn't")
            || msg_lower.contains("failed")
        {
            PdMessageLevel::Error
        } else if msg_lower.contains("warning") || msg_lower.contains("deprecated") {
            PdMessageLevel::Warning
        } else {
            PdMessageLevel::Info
        };

        // Push to lock-free queue (RT-safe, non-blocking)
        PD_MESSAGE_QUEUE.push(PdMessage {
            text: msg.to_string(),
            level,
        });
        HAS_PENDING_MESSAGES.store(true, Ordering::Release);
    });

    log::debug!("PD print hook initialized with lock-free message queue");
}

/// Drain all pending PD messages and log them
///
/// Call this from a non-RT context (e.g., after opening a patch, in UI update loop)
/// to process any messages that were queued from the audio thread.
pub fn drain_pd_messages() {
    if !HAS_PENDING_MESSAGES.load(Ordering::Acquire) {
        return;
    }

    while let Some(msg) = PD_MESSAGE_QUEUE.pop() {
        match msg.level {
            PdMessageLevel::Error => log::error!("[PD] {}", msg.text),
            PdMessageLevel::Warning => log::warn!("[PD] {}", msg.text),
            PdMessageLevel::Info => log::info!("[PD] {}", msg.text),
        }
    }

    HAS_PENDING_MESSAGES.store(false, Ordering::Release);
}

/// Check if there are pending PD messages without draining them
pub fn has_pending_pd_messages() -> bool {
    HAS_PENDING_MESSAGES.load(Ordering::Acquire)
}

/// Handle to an open PD patch
#[derive(Debug)]
pub struct PatchHandle {
    /// The $0 value for this patch instance
    /// Used for instance-scoped receives (e.g., $0-param0)
    pub dollar_zero: i32,
}

impl PatchHandle {
    /// Get the $0 value for instance-scoped receives
    pub fn instance_id(&self) -> i32 {
        self.dollar_zero
    }
}

/// Wrapper around libpd-rs for audio effect processing
///
/// IMPORTANT: libpd can only be initialized ONCE per process. There should be
/// exactly ONE PdInstance for the entire application. All effects (from all decks)
/// share this single instance and use $0 prefix for patch isolation.
pub struct PdInstance {
    /// The underlying libpd-rs Pd instance
    pd: Pd,

    /// Audio context for real-time processing
    ctx: PdAudioContext,

    /// Whether audio processing is active
    audio_active: bool,

    /// Sample rate configured for this instance
    sample_rate: i32,

    /// Number of open patches (for tracking)
    open_patches: usize,
}

impl PdInstance {
    /// Create the global PD instance
    ///
    /// IMPORTANT: This should only be called ONCE for the entire application.
    /// libpd does not support multiple initializations.
    ///
    /// # Arguments
    /// * `sample_rate` - Audio sample rate (typically 48000)
    pub fn new(sample_rate: i32) -> PdResult<Self> {
        log::info!("[PD-DEBUG] PdInstance::new() called with sample_rate={}", sample_rate);

        // Check if libpd was already initialized (programmer error to call twice)
        if LIBPD_INITIALIZED.swap(true, Ordering::SeqCst) {
            return Err(PdError::InitializationFailed(
                "libpd already initialized - only one PdInstance allowed per process".to_string(),
            ));
        }

        log::info!("[PD-DEBUG] First initialization, proceeding...");

        // Initialize print hook (but NOT before Pd::init_and_configure)
        // Note: We skip the print hook for now to isolate the crash
        log::info!("[PD-DEBUG] About to call Pd::init_and_configure(2, 2, {})", sample_rate);

        // Initialize libpd with stereo I/O (2 in, 2 out)
        let pd = Pd::init_and_configure(2, 2, sample_rate).map_err(|e| {
            // Reset the flag on failure so user can retry
            LIBPD_INITIALIZED.store(false, Ordering::SeqCst);
            PdError::InitializationFailed(format!("libpd init failed: {}", e))
        })?;

        log::info!("[PD-DEBUG] Pd::init_and_configure succeeded!");

        // Now initialize print hook AFTER libpd is set up
        LIBPD_INIT.call_once(|| {
            log::info!("[PD-DEBUG] Initializing print hook...");
            init_print_hook();
            log::info!("[PD-DEBUG] Print hook initialized");
        });

        log::info!("[PD-DEBUG] Getting audio context...");
        let ctx = pd.audio_context();

        log::info!("[PD-DEBUG] PdInstance created @ {}Hz (global singleton)", sample_rate);

        Ok(Self {
            pd,
            ctx,
            audio_active: false,
            sample_rate,
            open_patches: 0,
        })
    }

    /// Add a search path for externals and abstractions
    pub fn add_search_path(&mut self, path: &Path) -> PdResult<()> {
        self.pd.add_path_to_search_paths(path).map_err(|e| {
            PdError::InitializationFailed(format!("Failed to add search path: {}", e))
        })?;

        log::debug!("[PD] Added search path: {}", path.display());

        Ok(())
    }

    /// Open a PD patch file
    ///
    /// Returns a handle with the patch's $0 value for instance-scoped communication.
    pub fn open_patch(&mut self, path: &Path) -> PdResult<PatchHandle> {
        if !path.exists() {
            return Err(PdError::PatchNotFound(path.to_path_buf()));
        }

        self.pd.open_patch(path).map_err(|e| PdError::PatchOpenFailed {
            path: path.to_path_buf(),
            reason: format!("{}", e),
        })?;

        // Get the $0 value for this patch
        let dollar_zero = self.pd.dollar_zero().map_err(|e| PdError::PatchOpenFailed {
            path: path.to_path_buf(),
            reason: format!("Failed to get $0: {}", e),
        })?;

        self.open_patches += 1;

        // Drain any messages that were queued during patch loading
        // (e.g., external loading errors, warnings about missing objects)
        drain_pd_messages();

        log::info!("[PD] Opened patch: {} ($0={})", path.display(), dollar_zero);

        Ok(PatchHandle { dollar_zero })
    }

    /// Close the current PD patch
    pub fn close_patch(&mut self) -> PdResult<()> {
        self.pd.close_patch().map_err(|e| {
            PdError::PatchCloseFailed(format!("{}", e))
        })?;

        if self.open_patches > 0 {
            self.open_patches -= 1;
        }

        log::debug!("[PD] Closed patch");

        Ok(())
    }

    /// Activate or deactivate audio processing
    pub fn set_audio_active(&mut self, active: bool) -> PdResult<()> {
        self.pd.activate_audio(active).map_err(|e| {
            PdError::InitializationFailed(format!("Failed to set audio active: {}", e))
        })?;

        self.audio_active = active;

        Ok(())
    }

    /// Send a float value to a receiver
    ///
    /// # Arguments
    /// * `receiver` - The receive name (e.g., "123-param0" for $0-param0 where $0=123)
    /// * `value` - The float value to send
    pub fn send_float(&self, receiver: &str, value: f32) -> PdResult<()> {
        // Set this instance as current before sending
        self.pd.set_as_current();

        libpd_rs::functions::send::send_float_to(receiver, value).map_err(|e| {
            PdError::SendFailed {
                msg_type: "float".to_string(),
                receiver: receiver.to_string(),
                reason: format!("{}", e),
            }
        })
    }

    /// Send a bang to a receiver
    pub fn send_bang(&self, receiver: &str) -> PdResult<()> {
        // Set this instance as current before sending
        self.pd.set_as_current();

        libpd_rs::functions::send::send_bang_to(receiver).map_err(|e| {
            PdError::SendFailed {
                msg_type: "bang".to_string(),
                receiver: receiver.to_string(),
                reason: format!("{}", e),
            }
        })
    }

    /// Process audio through libpd
    ///
    /// # Arguments
    /// * `input` - Interleaved stereo input (L, R, L, R, ...)
    /// * `output` - Interleaved stereo output buffer (must be same length as input)
    ///
    /// # Returns
    /// Number of samples processed per channel
    pub fn process(&self, input: &[f32], output: &mut [f32]) -> usize {
        debug_assert_eq!(
            input.len(),
            output.len(),
            "Input/output buffer size mismatch"
        );

        // Calculate ticks: libpd processes in blocks of 64 samples
        // For stereo (2 channels), we need: ticks = (buffer_len / channels) / 64
        let ticks = libpd_rs::functions::util::calculate_ticks(2, output.len() as i32);

        // Process audio through the PD context
        self.ctx.process_float(ticks, input, output);

        // Return samples per channel
        output.len() / 2
    }

    /// Get the configured sample rate
    pub fn sample_rate(&self) -> i32 {
        self.sample_rate
    }

    /// Check if audio is active
    pub fn is_audio_active(&self) -> bool {
        self.audio_active
    }

    /// Get the number of open patches
    pub fn open_patch_count(&self) -> usize {
        self.open_patches
    }
}

impl Drop for PdInstance {
    fn drop(&mut self) {
        if self.audio_active {
            let _ = self.set_audio_active(false);
        }
        log::debug!("[PD] Global PdInstance dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_handle_instance_id() {
        let handle = PatchHandle { dollar_zero: 1001 };
        assert_eq!(handle.instance_id(), 1001);
    }
}
