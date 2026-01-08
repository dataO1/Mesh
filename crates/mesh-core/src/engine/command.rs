//! Lock-free command queue for real-time audio engine control
//!
//! This module implements the **Command Pattern** for audio engines:
//! the UI thread sends commands via a lock-free queue, and the audio
//! thread processes them at frame boundaries.
//!
//! # Why Lock-Free?
//!
//! Traditional mutex-based sharing causes audio dropouts:
//! - UI holds mutex for 1ms to load a track
//! - Audio callback (every 5.8ms) calls `try_lock()` and fails
//! - Failed lock = silence output = audible dropout
//!
//! With a lock-free queue:
//! - UI pushes command in ~50ns (never blocks)
//! - Audio pops commands in ~50ns (never blocks)
//! - No mutex = no contention = no dropouts
//!
//! # Real-Time Safety
//!
//! The `rtrb` ringbuffer is specifically designed for audio:
//! - **No allocations**: Fixed-size ringbuffer allocated at startup
//! - **Wait-free**: Both push and pop are O(1) and never block
//! - **Single-producer single-consumer**: Perfect for UI→Audio pattern
//!
//! # Usage
//!
//! ```ignore
//! // At startup
//! let (tx, rx) = command_channel(64);
//!
//! // UI thread: send commands (non-blocking)
//! tx.push(EngineCommand::Play { deck: 0 });
//!
//! // Audio thread: process pending commands
//! engine.process_commands(&mut rx);
//! ```

use super::slicer::QueueAlgorithm;
use super::PreparedTrack;
use crate::types::Stem;

/// Commands sent from UI thread to audio thread
///
/// Each variant represents an atomic operation on the engine.
/// Commands are processed at the start of each audio frame,
/// ensuring deterministic timing and no mid-frame state changes.
pub enum EngineCommand {
    // ─────────────────────────────────────────────────────────────
    // Track Management
    // ─────────────────────────────────────────────────────────────
    /// Load a prepared track onto a deck
    ///
    /// The `PreparedTrack` is boxed because it's large (~107MB of audio data).
    /// Boxing ensures the command enum itself stays small (pointer-sized).
    LoadTrack {
        deck: usize,
        track: Box<PreparedTrack>,
    },
    /// Unload track from a deck
    UnloadTrack { deck: usize },

    // ─────────────────────────────────────────────────────────────
    // Playback Control
    // ─────────────────────────────────────────────────────────────
    /// Start playback on a deck
    Play { deck: usize },
    /// Pause playback on a deck
    Pause { deck: usize },
    /// Toggle play/pause on a deck
    TogglePlay { deck: usize },
    /// Seek to a specific sample position
    Seek { deck: usize, position: usize },

    // ─────────────────────────────────────────────────────────────
    // CDJ-Style Cueing
    // ─────────────────────────────────────────────────────────────
    /// CDJ-style cue button press (sets cue point or returns to it)
    CuePress { deck: usize },
    /// CDJ-style cue button release (stops preview playback)
    CueRelease { deck: usize },
    /// Set cue point at current position (snapped to beat)
    SetCuePoint { deck: usize },

    // ─────────────────────────────────────────────────────────────
    // Hot Cues
    // ─────────────────────────────────────────────────────────────
    /// Hot cue button press (set/jump/preview depending on state)
    HotCuePress { deck: usize, slot: usize },
    /// Hot cue button release (ends preview if active)
    HotCueRelease { deck: usize },
    /// Clear a hot cue slot
    ClearHotCue { deck: usize, slot: usize },
    /// Set shift state (for alternate button functions)
    SetShift { deck: usize, held: bool },

    // ─────────────────────────────────────────────────────────────
    // Loop Control
    // ─────────────────────────────────────────────────────────────
    /// Toggle loop on/off at current position
    ToggleLoop { deck: usize },
    /// Set loop in point at current position
    LoopIn { deck: usize },
    /// Set loop out point and activate loop
    LoopOut { deck: usize },
    /// Turn off active loop
    LoopOff { deck: usize },
    /// Adjust loop length (positive = longer, negative = shorter)
    AdjustLoopLength { deck: usize, direction: i32 },
    /// Set loop length index directly (0-6 maps to 0.25, 0.5, 1, 2, 4, 8, 16 beats)
    SetLoopLengthIndex { deck: usize, index: usize },
    /// Toggle slip mode (loop exit returns to where playhead would have been)
    ToggleSlip { deck: usize },

    // ─────────────────────────────────────────────────────────────
    // Beat Jump
    // ─────────────────────────────────────────────────────────────
    /// Jump forward by beat_jump_size beats (equals loop length)
    BeatJumpForward { deck: usize },
    /// Jump backward by beat_jump_size beats (equals loop length)
    BeatJumpBackward { deck: usize },

    // ─────────────────────────────────────────────────────────────
    // Stem Control
    // ─────────────────────────────────────────────────────────────
    /// Toggle mute for a stem
    ToggleStemMute { deck: usize, stem: Stem },
    /// Set mute state for a stem (explicit, not toggle)
    SetStemMute { deck: usize, stem: Stem, muted: bool },
    /// Toggle solo for a stem
    ToggleStemSolo { deck: usize, stem: Stem },
    /// Set solo state for a stem (explicit, not toggle)
    SetStemSolo { deck: usize, stem: Stem, soloed: bool },

    // ─────────────────────────────────────────────────────────────
    // Key Matching
    // ─────────────────────────────────────────────────────────────
    /// Enable/disable automatic key matching for a deck
    /// When enabled, the deck will transpose to match the master deck's key
    SetKeyMatchEnabled { deck: usize, enabled: bool },
    /// Set the track's musical key (parsed from metadata)
    SetTrackKey { deck: usize, key: Option<String> },

    // ─────────────────────────────────────────────────────────────
    // Slicer Control
    // ─────────────────────────────────────────────────────────────
    /// Enable/disable slicer for a stem on a deck
    SetSlicerEnabled { deck: usize, stem: Stem, enabled: bool },
    /// Queue a slice for playback (button press in slicer mode, 0-7)
    SlicerQueueSlice { deck: usize, stem: Stem, slice_idx: usize },
    /// Reset slicer queue to default order [0,1,2,3,4,5,6,7]
    SlicerResetQueue { deck: usize, stem: Stem },
    /// Set slicer buffer size in bars (4, 8, or 16)
    SetSlicerBufferBars { deck: usize, stem: Stem, bars: u32 },
    /// Set slicer queue algorithm (FIFO rotate or Replace current)
    SetSlicerQueueAlgorithm { deck: usize, stem: Stem, algorithm: QueueAlgorithm },

    // ─────────────────────────────────────────────────────────────
    // Mixer Control
    // ─────────────────────────────────────────────────────────────
    /// Set channel volume (0.0 - 1.0)
    SetVolume { deck: usize, volume: f32 },
    /// Set crossfader position (-1.0 = A, 0.0 = center, 1.0 = B)
    SetCrossfader { position: f32 },
    /// Set channel to cue (pre-fader listen)
    SetCueListen { deck: usize, enabled: bool },
    /// Set channel EQ high (0.0 = kill, 0.5 = flat, 1.0 = boost)
    SetEqHi { deck: usize, value: f32 },
    /// Set channel EQ mid (0.0 = kill, 0.5 = flat, 1.0 = boost)
    SetEqMid { deck: usize, value: f32 },
    /// Set channel EQ low (0.0 = kill, 0.5 = flat, 1.0 = boost)
    SetEqLo { deck: usize, value: f32 },
    /// Set channel filter (-1.0 = full LP, 0.0 = flat, 1.0 = full HP)
    SetFilter { deck: usize, value: f32 },

    // ─────────────────────────────────────────────────────────────
    // Global
    // ─────────────────────────────────────────────────────────────
    /// Set global BPM (affects time-stretching on all decks)
    SetGlobalBpm(f64),
    /// Adjust global BPM by delta
    AdjustBpm(f64),
    /// Enable or disable inter-deck phase synchronization
    ///
    /// When enabled, starting playback or triggering hot cues will
    /// automatically align to the master deck's beat phase.
    SetPhaseSync(bool),
}

/// Capacity of the command queue
///
/// 64 commands is enough for ~1 second of rapid button presses at 60fps.
/// If the queue fills up, new commands are dropped (better than blocking).
pub const COMMAND_QUEUE_CAPACITY: usize = 64;

/// Create a new command channel (producer/consumer pair)
///
/// Returns `(Producer, Consumer)` where:
/// - Producer: Send side, owned by UI thread
/// - Consumer: Receive side, owned by audio thread
///
/// The channel is bounded with capacity for [`COMMAND_QUEUE_CAPACITY`] commands.
pub fn command_channel() -> (rtrb::Producer<EngineCommand>, rtrb::Consumer<EngineCommand>) {
    rtrb::RingBuffer::new(COMMAND_QUEUE_CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_channel_creation() {
        let (mut tx, mut rx) = command_channel();

        // Send a command
        tx.push(EngineCommand::Play { deck: 0 }).unwrap();

        // Receive it
        let cmd = rx.pop().unwrap();
        assert!(matches!(cmd, EngineCommand::Play { deck: 0 }));
    }

    #[test]
    fn test_command_channel_empty() {
        let (_tx, mut rx) = command_channel();

        // Empty queue should return error
        assert!(rx.pop().is_err());
    }

    #[test]
    fn test_command_size() {
        // Ensure EngineCommand stays small for cache efficiency in the ringbuffer
        // SetTrackKey with Option<String> is the largest variant at 32 bytes
        // This still fits 2 commands per 64-byte cache line
        let size = std::mem::size_of::<EngineCommand>();
        assert!(size <= 32, "EngineCommand is {} bytes, expected <= 32", size);
    }
}
