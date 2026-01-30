//! PdInstance - wrapper around libpd-rs for per-deck PD processing
//!
//! Each deck gets its own PdInstance for thread isolation and parallel processing.
//! This wrapper provides a simplified API over libpd-rs tailored for mesh's needs.

use std::path::Path;
use std::sync::Once;

use libpd_rs::{Pd, PdAudioContext};
use libpd_rs::functions::receive::on_print;
use libpd_rs::functions::verbose_print_state;

use super::error::{PdError, PdResult};

/// Global flag to ensure print hook is only registered once
static PRINT_HOOK_INIT: Once = Once::new();

/// Initialize the global PD print hook (called once across all instances)
fn init_print_hook() {
    PRINT_HOOK_INIT.call_once(|| {
        // Enable verbose printing from PD
        verbose_print_state(true);

        // Register print hook to capture all PD console output
        on_print(|msg: &str| {
            // Trim whitespace and skip empty messages
            let msg = msg.trim();
            if msg.is_empty() {
                return;
            }

            // Route to appropriate log level based on content
            if msg.contains("error") || msg.contains("can't") || msg.contains("couldn't") {
                log::error!("[PD] {}", msg);
            } else if msg.contains("warning") || msg.contains("deprecated") {
                log::warn!("[PD] {}", msg);
            } else {
                log::info!("[PD] {}", msg);
            }
        });

        log::debug!("PD print hook initialized");
    });
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

/// Wrapper around libpd-rs for a single deck
///
/// Provides thread-safe access to libpd operations. Each deck should
/// have its own PdInstance to enable parallel processing.
pub struct PdInstance {
    /// The underlying libpd-rs Pd instance
    pd: Pd,

    /// Audio context for real-time processing
    ctx: PdAudioContext,

    /// Deck index this instance belongs to
    deck_index: usize,

    /// Whether audio processing is active
    audio_active: bool,

    /// Sample rate configured for this instance
    sample_rate: i32,

    /// Number of open patches (for tracking)
    open_patches: usize,
}

impl PdInstance {
    /// Create a new PD instance for a deck
    ///
    /// # Arguments
    /// * `deck_index` - The deck this instance belongs to (0-3)
    /// * `sample_rate` - Audio sample rate (typically 48000)
    pub fn new(deck_index: usize, sample_rate: i32) -> PdResult<Self> {
        // Initialize print hook before any PD operations (only happens once)
        init_print_hook();

        // Initialize libpd with stereo I/O (2 in, 2 out)
        let pd = Pd::init_and_configure(2, 2, sample_rate).map_err(|e| {
            PdError::InitializationFailed(format!("libpd init failed: {}", e))
        })?;

        let ctx = pd.audio_context();

        log::info!(
            "PdInstance created for deck {} @ {}Hz",
            deck_index,
            sample_rate
        );

        Ok(Self {
            pd,
            ctx,
            deck_index,
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

        log::debug!(
            "Deck {}: Added PD search path: {}",
            self.deck_index,
            path.display()
        );

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

        log::info!(
            "Deck {}: Opened PD patch: {} ($0={})",
            self.deck_index,
            path.display(),
            dollar_zero
        );

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

        log::debug!("Deck {}: Closed PD patch", self.deck_index);

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

    /// Get the deck index this instance belongs to
    pub fn deck_index(&self) -> usize {
        self.deck_index
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
        log::debug!("PdInstance for deck {} dropped", self.deck_index);
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
