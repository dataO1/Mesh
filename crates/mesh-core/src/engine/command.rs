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

use super::{LinkedStemData, PreparedTrack};
use super::scratch::InterpolationMethod;
use super::slicer::{SlicerPreset, StepSequence};
use crate::types::Stem;

/// Request data for loading a linked stem from a file path
///
/// Separated into a struct so it can be boxed in the command enum,
/// keeping the enum size small for cache-efficient lock-free queueing.
pub struct LoadLinkedStemRequest {
    pub deck: usize,
    pub stem_idx: usize,
    pub path: std::path::PathBuf,
    pub host_bpm: f64,
    pub host_drop_marker: u64,
    pub host_duration: u64,
}

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
    // Scratch Mode (Vinyl-style scrubbing)
    // ─────────────────────────────────────────────────────────────
    /// Enter scratch mode - saves current play state, outputs audio at position
    /// without natural playhead progression (like touching a vinyl record)
    ScratchStart { deck: usize },
    /// Update scratch position - moves playhead and outputs audio
    /// The audio output will sound like vinyl scratching
    ScratchMove { deck: usize, position: usize },
    /// Exit scratch mode - restores previous play state
    /// (resumes playing if was playing, stays paused if was paused)
    ScratchEnd { deck: usize },
    /// Set scratch interpolation method for a deck
    /// Linear = fast, acceptable quality; Cubic = better quality, more CPU
    SetScratchInterpolation { deck: usize, method: InterpolationMethod },

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
    /// Set a hot cue at a specific position (for editor metadata sync)
    ///
    /// Unlike HotCuePress which sets at current playhead, this sets at an
    /// explicit position. Used when UI sets cue points in track metadata.
    SetHotCue { deck: usize, slot: usize, position: usize },
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
    /// Update beat grid on a deck (for live beatgrid nudging in editors)
    ///
    /// This allows the beat grid to be updated without reloading the track,
    /// so snapping operations use the updated grid immediately.
    SetBeatGrid { deck: usize, beats: Vec<u64> },

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
    /// Unified slicer button action from UI (UI doesn't know about behavior)
    /// Engine decides what to do based on shift_held state
    SlicerButtonAction {
        deck: usize,
        stem: Stem,
        button_idx: usize,
        shift_held: bool,
    },
    /// Reset slicer queue to default order [0,1,2,...,15]
    SlicerResetQueue { deck: usize, stem: Stem },
    /// Set slicer buffer size in bars (1, 4, 8, or 16)
    SetSlicerBufferBars { deck: usize, stem: Stem, bars: u32 },
    /// Set slicer preset patterns (loaded from config)
    /// Each preset defines per-stem patterns for coordinated multi-stem slicing.
    /// Boxed because the 8 presets with 4 stems each is large.
    SetSlicerPresets { presets: Box<[SlicerPreset; 8]> },
    /// Load a step sequence directly onto a stem's slicer
    SlicerLoadSequence {
        deck: usize,
        stem: Stem,
        sequence: Box<StepSequence>,
    },

    // ─────────────────────────────────────────────────────────────
    // Linked Stems (Hot-Swappable)
    // ─────────────────────────────────────────────────────────────
    /// Link a stem from another track to a deck's stem slot
    ///
    /// The linked stem should be pre-stretched to match the host deck's BPM.
    /// Drop markers are used for structural alignment during playback.
    ///
    /// `host_lufs` is passed explicitly to avoid race conditions - the deck's stored
    /// host_lufs might be stale if another track loaded between the host load and
    /// the linked stem load completing.
    ///
    /// Boxed because LinkedStemData contains large pre-stretched buffer.
    LinkStem {
        deck: usize,
        stem: Stem,
        linked_stem: Box<LinkedStemData>,
        /// Host track's LUFS at the time the linked stem was requested
        /// Used to calculate gain matching between host and linked stem
        host_lufs: Option<f32>,
    },
    /// Toggle between original and linked stem
    ///
    /// Only has effect if a linked stem exists for this slot.
    /// Returns immediately without blocking.
    ToggleLinkedStem { deck: usize, stem: Stem },
    /// Request loading a linked stem from a file path
    ///
    /// This is the command version of manual stem linking from the UI.
    /// The engine owns the LinkedStemLoader and handles all stem loading
    /// (both automatic from metadata and manual from this command).
    ///
    /// Results are delivered via the LinkedStemResultReceiver.
    ///
    /// Boxed because LoadLinkedStemRequest contains PathBuf (24 bytes) plus
    /// multiple fields, totaling 64 bytes - too large for cache-efficient queueing.
    LoadLinkedStem(Box<LoadLinkedStemRequest>),

    // ─────────────────────────────────────────────────────────────
    // Multiband Container Control
    // ─────────────────────────────────────────────────────────────
    // Each stem has exactly one MultibandHost (always present).
    // Effects are added INTO the multiband's bands, not alongside it.
    // ─────────────────────────────────────────────────────────────
    /// Set a crossover frequency for a stem's multiband container
    ///
    /// The crossover_index is 0-based (0 = first crossover between bands 0 and 1).
    /// Frequency is in Hz and will be clamped to valid range.
    SetMultibandCrossover {
        deck: usize,
        stem: Stem,
        crossover_index: usize,
        freq: f32,
    },
    /// Add a band to a stem's multiband container (splits the last band)
    ///
    /// Maximum 8 bands supported. If already at max, this is a no-op.
    AddMultibandBand { deck: usize, stem: Stem },
    /// Remove a band from a stem's multiband container by index
    ///
    /// Minimum 1 band required. If only 1 band exists, this is a no-op.
    RemoveMultibandBand {
        deck: usize,
        stem: Stem,
        band_index: usize,
    },
    /// Set mute state for a band in a stem's multiband container
    SetMultibandBandMute {
        deck: usize,
        stem: Stem,
        band_index: usize,
        muted: bool,
    },
    /// Set solo state for a band in a stem's multiband container
    SetMultibandBandSolo {
        deck: usize,
        stem: Stem,
        band_index: usize,
        soloed: bool,
    },
    /// Set gain for a band in a stem's multiband container (linear, 0.0-2.0)
    SetMultibandBandGain {
        deck: usize,
        stem: Stem,
        band_index: usize,
        gain: f32,
    },
    /// Add an effect to a band's chain in a stem's multiband container
    ///
    /// Maximum 8 effects per band. Boxed because Box<dyn Effect> is a trait object.
    AddMultibandBandEffect {
        deck: usize,
        stem: Stem,
        band_index: usize,
        effect: Box<dyn crate::effect::Effect>,
    },
    /// Remove an effect from a band's chain in a stem's multiband container
    RemoveMultibandBandEffect {
        deck: usize,
        stem: Stem,
        band_index: usize,
        effect_index: usize,
    },
    /// Set bypass state for an effect within a multiband band
    SetMultibandEffectBypass {
        deck: usize,
        stem: Stem,
        band_index: usize,
        effect_index: usize,
        bypass: bool,
    },
    /// Set a parameter value on an effect within a multiband band
    SetMultibandEffectParam {
        deck: usize,
        stem: Stem,
        band_index: usize,
        effect_index: usize,
        param_index: usize,
        value: f32,
    },
    /// Set a macro value in a stem's multiband container (0-3)
    ///
    /// The macro system allows one knob to control multiple parameters.
    SetMultibandMacro {
        deck: usize,
        stem: Stem,
        macro_index: usize,
        value: f32,
    },
    /// Add a macro mapping to route a macro to an effect parameter
    ///
    /// When a macro value changes, the mapped parameter will be modulated.
    AddMultibandMacroMapping {
        deck: usize,
        stem: Stem,
        macro_index: usize,
        location: crate::effect::multiband::EffectLocation,
        effect_index: usize,
        param_index: usize,
        min_value: f32,
        max_value: f32,
    },
    /// Clear all macro mappings for a macro
    ClearMultibandMacroMappings {
        deck: usize,
        stem: Stem,
        macro_index: usize,
    },
    /// Add an effect to the pre-fx chain (before multiband split)
    AddMultibandPreFx {
        deck: usize,
        stem: Stem,
        effect: Box<dyn crate::effect::Effect>,
    },
    /// Remove an effect from the pre-fx chain
    RemoveMultibandPreFx {
        deck: usize,
        stem: Stem,
        effect_index: usize,
    },
    /// Set bypass state for a pre-fx effect
    SetMultibandPreFxBypass {
        deck: usize,
        stem: Stem,
        effect_index: usize,
        bypass: bool,
    },
    /// Set a parameter value on a pre-fx effect
    SetMultibandPreFxParam {
        deck: usize,
        stem: Stem,
        effect_index: usize,
        param_index: usize,
        value: f32,
    },
    /// Add an effect to the post-fx chain (after band summation)
    AddMultibandPostFx {
        deck: usize,
        stem: Stem,
        effect: Box<dyn crate::effect::Effect>,
    },
    /// Remove an effect from the post-fx chain
    RemoveMultibandPostFx {
        deck: usize,
        stem: Stem,
        effect_index: usize,
    },
    /// Set bypass state for a post-fx effect
    SetMultibandPostFxBypass {
        deck: usize,
        stem: Stem,
        effect_index: usize,
        bypass: bool,
    },
    /// Set a parameter value on a post-fx effect
    SetMultibandPostFxParam {
        deck: usize,
        stem: Stem,
        effect_index: usize,
        param_index: usize,
        value: f32,
    },
    /// Reset a stem's multiband host to default state (single band, no effects)
    ///
    /// Used when disabling audio preview to return the stem to a clean state.
    ResetMultiband { deck: usize, stem: Stem },

    /// Atomically swap the entire MultibandHost on a stem
    ///
    /// Replaces 300-1000+ individual commands with a single pointer-sized
    /// command containing a fully-built MultibandHost from the preset loader.
    /// The Box is pointer-sized so it fits easily in the ring buffer queue.
    SwapMultiband {
        deck: usize,
        stem: Stem,
        multiband: Box<crate::effect::MultibandHost>,
    },

    // ─────────────────────────────────────────────────────────────
    // Multiband Dry/Wet Mix Control
    // ─────────────────────────────────────────────────────────────
    /// Set per-effect dry/wet mix for a pre-fx effect
    ///
    /// mix: 0.0 = fully dry (effect bypassed), 1.0 = fully wet (normal processing)
    SetMultibandPreFxEffectDryWet {
        deck: usize,
        stem: Stem,
        effect_index: usize,
        mix: f32,
    },
    /// Set per-effect dry/wet mix for a band effect
    ///
    /// mix: 0.0 = fully dry (effect bypassed), 1.0 = fully wet (normal processing)
    SetMultibandBandEffectDryWet {
        deck: usize,
        stem: Stem,
        band_index: usize,
        effect_index: usize,
        mix: f32,
    },
    /// Set per-effect dry/wet mix for a post-fx effect
    ///
    /// mix: 0.0 = fully dry (effect bypassed), 1.0 = fully wet (normal processing)
    SetMultibandPostFxEffectDryWet {
        deck: usize,
        stem: Stem,
        effect_index: usize,
        mix: f32,
    },
    /// Set chain dry/wet mix for the entire pre-fx chain
    ///
    /// mix: 0.0 = fully dry (chain bypassed), 1.0 = fully wet (normal processing)
    SetMultibandPreFxChainDryWet {
        deck: usize,
        stem: Stem,
        mix: f32,
    },
    /// Set chain dry/wet mix for a band's entire effect chain
    ///
    /// mix: 0.0 = fully dry (chain bypassed), 1.0 = fully wet (normal processing)
    SetMultibandBandChainDryWet {
        deck: usize,
        stem: Stem,
        band_index: usize,
        mix: f32,
    },
    /// Set chain dry/wet mix for the entire post-fx chain
    ///
    /// mix: 0.0 = fully dry (chain bypassed), 1.0 = fully wet (normal processing)
    SetMultibandPostFxChainDryWet {
        deck: usize,
        stem: Stem,
        mix: f32,
    },
    /// Set global dry/wet mix for the entire multiband effect rack
    ///
    /// mix: 0.0 = fully dry (rack bypassed), 1.0 = fully wet (normal processing)
    SetMultibandGlobalDryWet {
        deck: usize,
        stem: Stem,
        mix: f32,
    },

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
    /// Set master output volume (0.0 - 1.0)
    SetMasterVolume { volume: f32 },
    /// Set cue/master mix for headphone output (0.0 = cue only, 1.0 = master only)
    SetCueMix { mix: f32 },
    /// Set cue/headphone output volume (0.0 - 1.0)
    SetCueVolume { volume: f32 },

    // ─────────────────────────────────────────────────────────────
    // Loudness Compensation
    // ─────────────────────────────────────────────────────────────
    /// Set LUFS-based gain compensation for a deck
    ///
    /// The gain is a linear multiplier calculated from:
    /// `gain = 10^((target_lufs - track_lufs) / 20)`
    ///
    /// The `host_lufs` is used to calculate gain correction for linked stems,
    /// so they match the host track's level before deck-wide compensation.
    ///
    /// This is sent when:
    /// - A track is loaded (calculated from track's measured LUFS)
    /// - Target LUFS setting changes (recalculated for all loaded tracks)
    SetLufsGain { deck: usize, gain: f32, host_lufs: Option<f32> },
    /// Update the loudness configuration
    ///
    /// The engine uses this to calculate LUFS gain automatically when tracks load.
    /// When the config changes, all loaded decks have their LUFS gain recalculated.
    ///
    /// Sent when:
    /// - App starts (initial config from saved settings)
    /// - User changes auto-gain enabled or target LUFS in settings
    SetLoudnessConfig(crate::config::LoudnessConfig),

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
/// Bulk operations like preset loading can send 300-500+ commands in a burst
/// (effects, parameters, dry/wet values, macro mappings per stem). 1024
/// provides headroom while keeping memory usage modest (~200 KB per channel).
pub const COMMAND_QUEUE_CAPACITY: usize = 1024;

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
        // Ensure EngineCommand stays small for cache efficiency in the ringbuffer.
        // Largest variants are SetBeatGrid/SetTrackKey (deck + Vec/String = 32 bytes).
        // Large data like LoadLinkedStemRequest (64 bytes) must be boxed.
        // This still fits comfortably within a 64-byte cache line.
        let size = std::mem::size_of::<EngineCommand>();
        assert!(size <= 40, "EngineCommand is {} bytes, expected <= 40", size);
    }
}
