//! JACK audio playback for mesh-cue
//!
//! Provides audio preview functionality for both the staging area
//! and the collection editor. Reuses patterns from mesh-player.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Audio playback state shared between UI and audio thread
pub struct AudioState {
    /// Current playback position in samples
    pub position: Arc<AtomicU64>,
    /// Whether audio is currently playing
    pub playing: Arc<AtomicBool>,
    /// Total track length in samples
    pub length: u64,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            position: Arc::new(AtomicU64::new(0)),
            playing: Arc::new(AtomicBool::new(false)),
            length: 0,
        }
    }
}

impl AudioState {
    /// Get current playback position
    pub fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    /// Set playback position (seek)
    pub fn seek(&self, position: u64) {
        self.position.store(position.min(self.length), Ordering::Relaxed);
    }

    /// Check if playing
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// Start playback
    pub fn play(&self) {
        self.playing.store(true, Ordering::Relaxed);
    }

    /// Pause playback
    pub fn pause(&self) {
        self.playing.store(false, Ordering::Relaxed);
    }

    /// Toggle play/pause
    pub fn toggle(&self) {
        let current = self.playing.load(Ordering::Relaxed);
        self.playing.store(!current, Ordering::Relaxed);
    }
}

// TODO: Implement JACK client for audio playback
// This will follow the same pattern as mesh-player's audio.rs:
// - Ring buffer for audio data
// - Process callback that reads from buffer
// - Stereo output (all stems summed)
