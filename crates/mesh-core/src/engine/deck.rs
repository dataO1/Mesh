//! Deck - Individual track player with stems and effect chains

use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use rayon::prelude::*;

use crate::audio_file::LoadedTrack;
use crate::effect::EffectChain;
use crate::types::{
    DeckId, PlayState, Stem, StereoBuffer, StereoSample, TransportPosition,
    NUM_STEMS, SAMPLE_RATE,
};

use super::{LatencyCompensator, LinkedStemAtomics, StemLink, MAX_BUFFER_SIZE};

/// Number of hot cue slots per deck
pub const HOT_CUE_SLOTS: usize = 8;

/// Loop lengths available in beats (1 beat to 64 bars = 256 beats)
pub const LOOP_LENGTHS: [f64; 9] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0];

/// A hot cue point stored in a slot
#[derive(Debug, Clone)]
pub struct HotCue {
    /// Sample position in the track
    pub position: usize,
    /// Label for display
    pub label: String,
    /// Color as hex string (e.g., "#FF5500")
    pub color: Option<String>,
}

/// Pre-computed deck state for fast track application
///
/// This struct holds all the state that needs to be computed when loading a track.
/// By preparing this in a background thread, the actual mutex-holding operation
/// becomes a fast pointer swap instead of expensive string cloning and parsing.
///
/// ## Real-Time Safety
///
/// The expensive operations (string cloning for cue labels, metadata parsing)
/// happen when calling `PreparedTrack::prepare()` in the background thread.
/// The `Deck::apply_prepared_track()` method only does assignments and atomic
/// stores, reducing mutex hold time from 10-50ms to <1ms.
pub struct PreparedTrack {
    /// The loaded track with audio data
    pub track: LoadedTrack,
    /// Pre-computed hot cues (string cloning already done)
    pub hot_cues: [Option<HotCue>; HOT_CUE_SLOTS],
    /// First beat position for initial cue point
    pub first_beat: usize,
}

impl PreparedTrack {
    /// Prepare track state for fast application
    ///
    /// Call this from a background thread. All expensive operations
    /// (string cloning, metadata parsing) happen here, not while
    /// holding the engine mutex.
    pub fn prepare(track: LoadedTrack) -> Self {
        // Import cue points from track metadata (string cloning happens here)
        let hot_cues: [Option<HotCue>; HOT_CUE_SLOTS] = std::array::from_fn(|i| {
            track.metadata.cue_points.get(i).map(|cue| HotCue {
                position: cue.sample_position as usize,
                label: cue.label.clone(),
                color: cue.color.clone(),
            })
        });

        // Extract first beat position
        let first_beat = track
            .metadata
            .beat_grid
            .first_beat_sample
            .map(|b| b as usize)
            .unwrap_or(0);

        Self {
            track,
            hot_cues,
            first_beat,
        }
    }
}

/// Loop state
#[derive(Debug, Clone, Default)]
pub struct LoopState {
    /// Whether the loop is active
    pub active: bool,
    /// Loop start position in samples
    pub start: usize,
    /// Loop end position in samples
    pub end: usize,
    /// Current loop length index into LOOP_LENGTHS
    pub length_index: usize,
}

/// Lock-free playback state for UI access
///
/// This struct contains atomic fields that can be read by the UI thread
/// without acquiring a mutex lock. The audio thread writes to these atomics
/// whenever the corresponding state changes.
///
/// All operations use `Ordering::Relaxed` since we only need visibility,
/// not synchronization with other memory operations.
pub struct DeckAtomics {
    /// Current playhead position in samples
    pub position: AtomicU64,
    /// Playback state: 0=Stopped, 1=Playing, 2=Cueing
    pub state: AtomicU8,
    /// Cue point position in samples
    pub cue_point: AtomicU64,
    /// Whether loop is active
    pub loop_active: AtomicBool,
    /// Loop start position in samples
    pub loop_start: AtomicU64,
    /// Loop end position in samples
    pub loop_end: AtomicU64,
    /// Loop length index (0-6 maps to 0.25, 0.5, 1, 2, 4, 8, 16 beats)
    pub loop_length_index: AtomicU8,
    /// Whether this deck is the master (longest playing, others sync to it)
    pub is_master: AtomicBool,
    /// Whether key matching is enabled for this deck
    pub key_match_enabled: AtomicBool,
    /// Current transpose in semitones (-12 to +12)
    pub current_transpose: AtomicI8,
    /// Whether keys are compatible (no transpose needed even with key match on)
    pub keys_compatible: AtomicBool,
    /// LUFS-based gain compensation (f32 stored as bits)
    /// 1.0 = unity gain, calculated from target_lufs - track_lufs
    pub lufs_gain: AtomicU32,
}

impl DeckAtomics {
    /// Create new atomic state with defaults
    pub fn new() -> Self {
        Self {
            position: AtomicU64::new(0),
            state: AtomicU8::new(0), // Stopped
            cue_point: AtomicU64::new(0),
            loop_active: AtomicBool::new(false),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            loop_length_index: AtomicU8::new(2), // Default to 4 beats (index 2)
            is_master: AtomicBool::new(false),
            key_match_enabled: AtomicBool::new(false),
            current_transpose: AtomicI8::new(0),
            keys_compatible: AtomicBool::new(true),
            lufs_gain: AtomicU32::new(1.0_f32.to_bits()), // Unity gain by default
        }
    }

    /// Get current position (lock-free)
    #[inline]
    pub fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    /// Check if playing (lock-free)
    #[inline]
    pub fn is_playing(&self) -> bool {
        self.state.load(Ordering::Relaxed) == 1
    }

    /// Check if cueing (lock-free)
    #[inline]
    pub fn is_cueing(&self) -> bool {
        self.state.load(Ordering::Relaxed) == 2
    }

    /// Get play state as enum (lock-free)
    #[inline]
    pub fn play_state(&self) -> PlayState {
        match self.state.load(Ordering::Relaxed) {
            1 => PlayState::Playing,
            2 => PlayState::Cueing,
            _ => PlayState::Stopped,
        }
    }

    /// Get cue point position (lock-free)
    #[inline]
    pub fn cue_point(&self) -> u64 {
        self.cue_point.load(Ordering::Relaxed)
    }

    /// Check if loop is active (lock-free)
    #[inline]
    pub fn loop_active(&self) -> bool {
        self.loop_active.load(Ordering::Relaxed)
    }

    /// Get loop start position (lock-free)
    #[inline]
    pub fn loop_start(&self) -> u64 {
        self.loop_start.load(Ordering::Relaxed)
    }

    /// Get loop end position (lock-free)
    #[inline]
    pub fn loop_end(&self) -> u64 {
        self.loop_end.load(Ordering::Relaxed)
    }

    /// Get loop length index (lock-free)
    /// Returns 0-6 mapping to 0.25, 0.5, 1, 2, 4, 8, 16 beats
    #[inline]
    pub fn loop_length_index(&self) -> u8 {
        self.loop_length_index.load(Ordering::Relaxed)
    }

    /// Check if this deck is the master (lock-free)
    #[inline]
    pub fn is_master(&self) -> bool {
        self.is_master.load(Ordering::Relaxed)
    }

    /// Get LUFS-based gain compensation (lock-free)
    ///
    /// Returns the linear gain multiplier for loudness normalization.
    /// 1.0 = unity, >1.0 = boost quiet tracks, <1.0 = cut loud tracks
    #[inline]
    pub fn lufs_gain(&self) -> f32 {
        f32::from_bits(self.lufs_gain.load(Ordering::Relaxed))
    }

    /// Set LUFS-based gain compensation (called from audio thread)
    #[inline]
    pub fn set_lufs_gain(&self, gain: f32) {
        self.lufs_gain.store(gain.to_bits(), Ordering::Relaxed);
    }
}

impl Default for DeckAtomics {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopState {
    /// Get the current loop length in beats
    pub fn length_beats(&self) -> f64 {
        LOOP_LENGTHS[self.length_index]
    }

    /// Increase loop length (shift right in LOOP_LENGTHS)
    pub fn increase_length(&mut self) {
        if self.length_index < LOOP_LENGTHS.len() - 1 {
            self.length_index += 1;
        }
    }

    /// Decrease loop length (shift left in LOOP_LENGTHS)
    pub fn decrease_length(&mut self) {
        if self.length_index > 0 {
            self.length_index -= 1;
        }
    }

    /// Get length in samples for the given samples per beat
    pub fn length_samples(&self, samples_per_beat: f64) -> usize {
        (LOOP_LENGTHS[self.length_index] * samples_per_beat) as usize
    }
}

/// Per-stem state including mute/solo and effect chain
pub struct StemState {
    /// Effect chain for this stem
    pub chain: EffectChain,
    /// Whether this stem is muted
    pub muted: bool,
    /// Whether this stem is soloed
    pub soloed: bool,
}

impl Default for StemState {
    fn default() -> Self {
        Self {
            chain: EffectChain::new(),
            muted: false,
            soloed: false,
        }
    }
}

impl StemState {
    /// Create a new stem state
    pub fn new() -> Self {
        Self::default()
    }
}

/// A single deck in the DJ player
///
/// Manages track loading, playback, effect chains, and hot cues.
/// Each deck has 4 stems (Vocals, Drums, Bass, Other), each with its own
/// effect chain and mute/solo controls.
pub struct Deck {
    /// Deck identifier (0-3)
    id: DeckId,
    /// Currently loaded track (None if empty)
    track: Option<LoadedTrack>,
    /// Current playhead position in samples
    position: usize,
    /// Current playback state
    state: PlayState,
    /// Temporary cue point (for CDJ-style cueing)
    cue_point: usize,
    /// Hot cue slots
    hot_cues: [Option<HotCue>; HOT_CUE_SLOTS],
    /// Loop state
    loop_state: LoopState,
    /// Per-stem state (effect chains, mute/solo)
    stems: [StemState; NUM_STEMS],
    /// Scratch/jog adjustment in samples (for pitch bending)
    scratch_offset: f64,
    /// Whether shift is held (for alternate button functions)
    shift_held: bool,
    /// Time stretch ratio (target_bpm / track_bpm)
    /// > 1.0 = speedup (play faster), < 1.0 = slowdown, 1.0 = no stretch
    stretch_ratio: f64,
    /// Position to return to after hot cue preview (None = not previewing)
    hot_cue_preview_return: Option<usize>,
    /// Beat slip mode enabled (loop exit returns to where playhead would have been)
    slip_enabled: bool,
    /// Slip position: where the playhead would be if not looping (updated during loop)
    slip_position: Option<usize>,
    /// Lock-free state for UI access (position, play state, loop state)
    /// The UI can read these atomics without acquiring the engine mutex
    atomics: Arc<DeckAtomics>,
    /// Pre-allocated buffers for parallel stem processing (real-time safe)
    /// One buffer per stem enables parallel processing with Rayon
    /// Capacity is MAX_BUFFER_SIZE to handle any JACK buffer size
    stem_buffers: [StereoBuffer; NUM_STEMS],
    /// Accumulated fractional samples for time stretch accuracy
    ///
    /// When time stretching, the ideal number of samples to read is often
    /// fractional (e.g., 254.54). Rounding each frame loses the remainder,
    /// causing cumulative drift (~1 second over 10 minutes). Instead, we
    /// accumulate the fractional part and let it "catch up" over time.
    fractional_position: f64,
    /// Key match enabled - when true, deck transposes to match master key
    key_match_enabled: bool,
    /// Current transposition in semitones (0 if disabled or compatible)
    current_transpose: i8,
    /// Track's parsed musical key (None if not detected or unavailable)
    track_key: Option<crate::music::MusicalKey>,
    /// Per-stem slicer states (initially only Drums is used, but modular for future)
    slicer_states: [super::slicer::SlicerState; NUM_STEMS],
    /// Per-stem link state for hot-swappable stems from other tracks
    ///
    /// Each stem slot can optionally have a linked stem from another track.
    /// When linked, the stem can be toggled between original and linked.
    stem_links: [StemLink; NUM_STEMS],
    /// Lock-free state for linked stems (for UI access without mutex)
    linked_stem_atomics: Arc<LinkedStemAtomics>,
    /// Drop marker position for this track (for linked stem alignment)
    drop_marker: Option<u64>,
    /// LUFS-based loudness compensation gain (linear multiplier)
    ///
    /// Calculated from track's measured LUFS and configured target LUFS.
    /// 1.0 = unity (no compensation), <1.0 = cut, >1.0 = boost.
    /// Applied after stem summing but before time stretching.
    lufs_gain: f32,

    /// Host track's measured LUFS (from WAV file bext chunk)
    /// Used to calculate gain correction for linked stems
    host_lufs: Option<f32>,
}

impl Deck {
    /// Create a new empty deck
    pub fn new(id: DeckId) -> Self {
        Self {
            id,
            track: None,
            position: 0,
            state: PlayState::Stopped,
            cue_point: 0,
            hot_cues: std::array::from_fn(|_| None),
            loop_state: LoopState::default(),
            stems: std::array::from_fn(|_| StemState::new()),
            scratch_offset: 0.0,
            shift_held: false,
            stretch_ratio: 1.0, // No stretching by default
            hot_cue_preview_return: None,
            slip_enabled: false,
            slip_position: None,
            atomics: Arc::new(DeckAtomics::new()),
            stem_buffers: std::array::from_fn(|_| StereoBuffer::silence(MAX_BUFFER_SIZE)),
            fractional_position: 0.0,
            key_match_enabled: false,
            current_transpose: 0,
            track_key: None,
            slicer_states: std::array::from_fn(|_| super::slicer::SlicerState::new()),
            stem_links: std::array::from_fn(|_| StemLink::new()),
            linked_stem_atomics: Arc::new(LinkedStemAtomics::new()),
            drop_marker: None,
            lufs_gain: 1.0, // Unity gain (no compensation)
            host_lufs: None,
        }
    }

    /// Get a reference to the lock-free atomic state
    ///
    /// The UI can clone this Arc and read position/state without acquiring
    /// the engine mutex, eliminating lock contention during playback.
    pub fn atomics(&self) -> Arc<DeckAtomics> {
        Arc::clone(&self.atomics)
    }

    /// Get a reference to the linked stem atomic state
    ///
    /// The UI can clone this Arc and read linked stem state without
    /// acquiring the engine mutex.
    pub fn linked_stem_atomics(&self) -> Arc<LinkedStemAtomics> {
        Arc::clone(&self.linked_stem_atomics)
    }

    /// Write play state to atomics (internal helper)
    #[inline]
    fn sync_state_atomic(&self) {
        let state_val = match self.state {
            PlayState::Stopped => 0,
            PlayState::Playing => 1,
            PlayState::Cueing => 2,
        };
        self.atomics.state.store(state_val, Ordering::Relaxed);
    }

    /// Write position to atomics (internal helper)
    #[inline]
    fn sync_position_atomic(&self) {
        self.atomics.position.store(self.position as u64, Ordering::Relaxed);
    }

    /// Write loop state to atomics (internal helper)
    #[inline]
    fn sync_loop_atomic(&self) {
        self.atomics.loop_active.store(self.loop_state.active, Ordering::Relaxed);
        self.atomics.loop_start.store(self.loop_state.start as u64, Ordering::Relaxed);
        self.atomics.loop_end.store(self.loop_state.end as u64, Ordering::Relaxed);
        self.atomics.loop_length_index.store(self.loop_state.length_index as u8, Ordering::Relaxed);
    }

    /// Set the default loop length index
    ///
    /// Call this after creating a new Deck to apply the user's preferred
    /// default loop length from config.
    ///
    /// # Arguments
    /// * `index` - Index into LOOP_LENGTHS (0-6, corresponding to 0.25, 0.5, 1, 2, 4, 8, 16 beats)
    pub fn set_loop_length_index(&mut self, index: usize) {
        self.loop_state.length_index = index.min(LOOP_LENGTHS.len() - 1);
        self.sync_loop_atomic(); // Sync for UI access
    }

    /// Get the current loop length index
    pub fn loop_length_index(&self) -> usize {
        self.loop_state.length_index
    }

    /// Write cue point to atomics (internal helper)
    #[inline]
    fn sync_cue_atomic(&self) {
        self.atomics.cue_point.store(self.cue_point as u64, Ordering::Relaxed);
    }

    /// Write key match state to atomics (internal helper)
    #[inline]
    fn sync_key_match_atomic(&self) {
        self.atomics.key_match_enabled.store(self.key_match_enabled, Ordering::Relaxed);
        self.atomics.current_transpose.store(self.current_transpose, Ordering::Relaxed);
        // Keys are compatible if transpose is 0
        self.atomics.keys_compatible.store(self.current_transpose == 0, Ordering::Relaxed);
    }

    /// Get the deck ID
    pub fn id(&self) -> DeckId {
        self.id
    }

    /// Apply a pre-prepared track for minimal mutex hold time
    ///
    /// This method only performs pointer moves and atomic stores - no allocations
    /// or string cloning. Call `PreparedTrack::prepare()` in a background thread
    /// first, then use this method while holding the engine mutex.
    ///
    /// ## Real-Time Safety
    ///
    /// Mutex hold time: <1ms (vs 10-50ms for `load_track()`)
    /// Operations: Only assignments and atomic stores
    /// Allocations: None
    ///
    /// Note: Effect chain reset is deferred - it will happen on the next audio
    /// frame. This is inaudible (<0.1ms of stale effects).
    pub fn apply_prepared_track(&mut self, prepared: PreparedTrack) {
        // Preserve the user's loop length preference
        let length_index = self.loop_state.length_index;

        // All these are moves/copies, no allocations
        self.track = Some(prepared.track);
        self.position = prepared.first_beat;
        self.state = PlayState::Stopped;
        self.cue_point = prepared.first_beat;
        self.hot_cues = prepared.hot_cues;
        self.loop_state = LoopState {
            length_index,
            ..LoopState::default()
        };
        self.scratch_offset = 0.0;
        self.hot_cue_preview_return = None;
        self.fractional_position = 0.0; // Reset stretch accumulator

        // Update slicer grid alignment and reset queue for new track
        let first_beat = prepared.first_beat;
        for slicer in &mut self.slicer_states {
            slicer.set_first_beat(first_beat);
            slicer.reset_queue();
        }

        // Load drop marker from track metadata (for linked stem alignment)
        self.drop_marker = self.track.as_ref().and_then(|t| t.metadata.drop_marker);

        // Clear any existing linked stems when loading a new track
        for (i, link) in self.stem_links.iter_mut().enumerate() {
            link.clear();
            self.linked_stem_atomics.sync_from_stem_link(i, link);
        }
        if let Some(dm) = self.drop_marker {
            self.linked_stem_atomics.set_host_drop_marker(dm);
        }

        // Reset transient playback state (mutes, solos, slicers, key match)
        // Preserves slip mode and loop length
        self.reset_playback_state();

        // Sync atomics for lock-free UI reads (fast atomic stores)
        self.sync_position_atomic();
        self.sync_state_atomic();
        self.sync_cue_atomic();
        self.sync_loop_atomic();

        // Note: Effect chain reset is NOT done here to minimize mutex hold time.
        // The chains will be reset on next frame or can be done after releasing mutex.
    }

    /// Reset all effect chains (call after releasing mutex if needed)
    pub fn reset_effect_chains(&mut self) {
        for stem in &mut self.stems {
            stem.chain.reset();
        }
    }

    /// Reset playback state for a new track
    ///
    /// Resets transient playback state that shouldn't persist between tracks:
    /// - Stem mute/solo states (all unmuted, none soloed)
    /// - Slicer enabled states (all disabled)
    /// - Key matching (disabled, transpose reset)
    ///
    /// Preserves user preferences:
    /// - Slip mode (on/off)
    /// - Loop length index
    fn reset_playback_state(&mut self) {
        // Reset stem mute/solo states
        for stem in &mut self.stems {
            stem.muted = false;
            stem.soloed = false;
        }

        // Disable all slicers
        for slicer in &mut self.slicer_states {
            slicer.set_enabled(false);
        }

        // Reset key matching
        self.key_match_enabled = false;
        self.current_transpose = 0;
        self.track_key = None;
        self.atomics.key_match_enabled.store(false, Ordering::Relaxed);
        self.atomics.current_transpose.store(0, Ordering::Relaxed);

        // Clear slip position (but preserve slip_enabled)
        self.slip_position = None;
    }

    /// Unload the current track
    pub fn unload_track(&mut self) {
        self.track = None;
        self.position = 0;
        self.state = PlayState::Stopped;
        self.cue_point = 0;
        self.hot_cues = std::array::from_fn(|_| None);
        self.loop_state = LoopState::default();
        self.hot_cue_preview_return = None;

        // Clear drop marker and linked stems
        self.drop_marker = None;
        for (i, link) in self.stem_links.iter_mut().enumerate() {
            link.clear();
            self.linked_stem_atomics.sync_from_stem_link(i, link);
        }
        self.linked_stem_atomics.set_host_drop_marker(0);

        // Sync atomics for lock-free UI reads
        self.sync_position_atomic();
        self.sync_state_atomic();
        self.sync_cue_atomic();
        self.sync_loop_atomic();
    }

    /// Check if a track is loaded
    pub fn has_track(&self) -> bool {
        self.track.is_some()
    }

    /// Get a reference to the loaded track
    pub fn track(&self) -> Option<&LoadedTrack> {
        self.track.as_ref()
    }

    /// Get the current playback state
    pub fn state(&self) -> PlayState {
        self.state
    }

    /// Get the current position in samples
    pub fn position(&self) -> u64 {
        self.position as u64
    }

    /// Get the current transport position with beat/bar info
    pub fn transport_position(&self) -> TransportPosition {
        if let Some(track) = &self.track {
            let beat = track
                .beat_at_sample(self.position as u64)
                .map(|b| b as f64)
                .unwrap_or(0.0);
            let bar = beat / 4.0; // Assuming 4/4 time
            TransportPosition::new(self.position, beat, bar)
        } else {
            TransportPosition::default()
        }
    }

    /// Get the samples per beat for the loaded track
    pub fn samples_per_beat(&self) -> f64 {
        self.track
            .as_ref()
            .map(|t| t.samples_per_beat())
            .unwrap_or(SAMPLE_RATE as f64 * 60.0 / 120.0) // Default 120 BPM
    }

    /// Snap a sample position to the nearest beat in the grid
    pub fn snap_to_beat(&self, position: usize) -> usize {
        if let Some(track) = &self.track {
            let beats = &track.metadata.beat_grid.beats;
            beats
                .iter()
                .min_by_key(|&&b| (b as i64 - position as i64).unsigned_abs())
                .map(|&b| b as usize)
                .unwrap_or(position)
        } else {
            position
        }
    }

    /// Update the beat grid (call after UI nudge operations)
    ///
    /// This syncs the deck's internal beat grid with UI changes,
    /// ensuring beat jump operations use the updated grid.
    pub fn set_beat_grid(&mut self, beats: Vec<u64>) {
        if let Some(ref mut track) = self.track {
            track.metadata.beat_grid.first_beat_sample = beats.first().copied();
            track.metadata.beat_grid.beats = beats;
        }
    }

    // --- Playback controls ---

    /// Start/resume playback
    pub fn play(&mut self) {
        if self.track.is_some() {
            self.state = PlayState::Playing;
            self.sync_state_atomic();
        }
    }

    /// Pause playback
    pub fn pause(&mut self) {
        self.state = PlayState::Stopped;
        self.sync_state_atomic();
    }

    /// Toggle play/pause
    pub fn toggle_play(&mut self) {
        match self.state {
            PlayState::Playing => self.pause(),
            PlayState::Stopped | PlayState::Cueing => {
                // Clear preview return when committing to play - this ensures
                // releasing a hot cue button won't stop playback once play is pressed
                self.clear_preview_return();
                self.play();
            }
        }
    }

    /// CDJ-style cue button press
    ///
    /// If stopped: set cue point to current position and start previewing
    /// If playing: jump to cue point and stop
    /// If cueing: do nothing (release will handle it)
    pub fn cue_press(&mut self) {
        if self.track.is_none() {
            return;
        }

        match self.state {
            PlayState::Playing => {
                // Jump to cue point and stop
                self.position = self.cue_point;
                self.state = PlayState::Stopped;
                self.sync_position_atomic();
                self.sync_state_atomic();
            }
            PlayState::Stopped => {
                // Set cue to current position and start previewing
                self.cue_point = self.position;
                self.state = PlayState::Cueing;
                self.sync_cue_atomic();
                self.sync_state_atomic();
            }
            PlayState::Cueing => {
                // Already cueing, do nothing (release will stop)
            }
        }
    }

    /// CDJ-style cue button release
    pub fn cue_release(&mut self) {
        if self.state == PlayState::Cueing {
            // Return to cue point and stop
            self.position = self.cue_point;
            self.state = PlayState::Stopped;
            self.sync_position_atomic();
            self.sync_state_atomic();
        }
    }

    /// Set the cue point at the current position (snapped to nearest beat)
    pub fn set_cue_point(&mut self) {
        self.cue_point = self.snap_to_beat(self.position);
        self.sync_cue_atomic();
    }

    /// Set cue point at specific position without snapping
    ///
    /// Used when the caller has already snapped to a beat grid
    /// (e.g., when the UI has a more up-to-date beat grid than the Deck)
    pub fn set_cue_point_position(&mut self, position: usize) {
        self.cue_point = position;
        self.sync_cue_atomic();
    }

    /// Get the current cue point position
    pub fn cue_point(&self) -> usize {
        self.cue_point
    }

    /// Jump to a specific sample position
    pub fn seek(&mut self, position: usize) {
        if let Some(track) = &self.track {
            self.position = position.min(track.duration_samples.saturating_sub(1));
            self.fractional_position = 0.0; // Reset stretch accumulator on seek
            self.sync_position_atomic();
        }
    }

    /// Get the current beat jump size in beats (equals loop length)
    ///
    /// Beat jump is now unified with loop length - they always match.
    /// Minimum of 1 beat for fractional loop lengths.
    pub fn beat_jump_size(&self) -> i32 {
        // Use loop length as beat jump size, minimum 1 beat
        self.loop_state.length_beats().max(1.0) as i32
    }

    /// Get the current time stretch ratio
    pub fn stretch_ratio(&self) -> f64 {
        self.stretch_ratio
    }

    /// Set the time stretch ratio
    ///
    /// - ratio > 1.0: speedup (play faster to match higher target BPM)
    /// - ratio < 1.0: slowdown (play slower to match lower target BPM)
    /// - ratio = 1.0: no stretching (play at native tempo)
    ///
    /// Clamped to 0.5..2.0 range (half speed to double speed)
    pub fn set_stretch_ratio(&mut self, ratio: f64) {
        self.stretch_ratio = ratio.clamp(0.5, 2.0);
    }

    /// Check if key matching is enabled for this deck
    pub fn key_match_enabled(&self) -> bool {
        self.key_match_enabled
    }

    /// Enable or disable key matching for this deck
    pub fn set_key_match_enabled(&mut self, enabled: bool) {
        self.key_match_enabled = enabled;
        self.sync_key_match_atomic();
    }

    /// Get the current transposition in semitones
    pub fn current_transpose(&self) -> i8 {
        self.current_transpose
    }

    /// Set the current transposition (called by engine during key match calculation)
    pub fn set_current_transpose(&mut self, semitones: i8) {
        self.current_transpose = semitones;
        self.sync_key_match_atomic();
    }

    /// Get the track's musical key
    pub fn track_key(&self) -> Option<crate::music::MusicalKey> {
        self.track_key
    }

    /// Set the track's musical key (parsed from metadata)
    pub fn set_track_key(&mut self, key: Option<crate::music::MusicalKey>) {
        self.track_key = key;
    }

    // --- Loudness Compensation ---

    /// Get the current LUFS gain compensation (linear multiplier)
    pub fn lufs_gain(&self) -> f32 {
        self.lufs_gain
    }

    /// Get the track's measured LUFS (from metadata)
    ///
    /// Returns the track's integrated loudness in LUFS. Used to calculate
    /// gain compensation and for recalculating when target LUFS changes.
    pub fn track_lufs(&self) -> Option<f32> {
        self.host_lufs
    }

    /// Set the LUFS gain compensation (linear multiplier)
    ///
    /// This is calculated from: `10^((target_lufs - track_lufs) / 20)`
    /// - 1.0 = unity (no compensation)
    /// - >1.0 = boost (track quieter than target)
    /// - <1.0 = cut (track louder than target)
    ///
    /// Also stores the host LUFS and recalculates all linked stem gains.
    pub fn set_lufs_gain(&mut self, gain: f32, host_lufs: Option<f32>) {
        self.lufs_gain = gain;
        self.host_lufs = host_lufs;

        // Update atomics for UI access
        self.atomics.set_lufs_gain(gain);

        log::debug!("Deck {}: LUFS gain set to {:.3} ({:+.1} dB), host_lufs={:?}",
            self.id.0, gain, 20.0 * gain.log10(), host_lufs);

        // Recalculate all linked stem gains based on new host LUFS
        for link in &mut self.stem_links {
            link.update_gain(host_lufs);
        }
    }

    // --- Slicer ---

    /// Get the slicer atomics for a specific stem (for UI access)
    pub fn slicer_atomics(&self, stem: Stem) -> std::sync::Arc<super::slicer::SlicerAtomics> {
        self.slicer_states[stem as usize].atomics()
    }

    /// Check if slicer is enabled for a stem
    pub fn slicer_enabled(&self, stem: Stem) -> bool {
        self.slicer_states[stem as usize].is_enabled()
    }

    /// Enable or disable slicer for a stem
    pub fn set_slicer_enabled(&mut self, stem: Stem, enabled: bool) {
        self.slicer_states[stem as usize].set_enabled(enabled);
    }

    /// Handle shift+button for slice assignment on a single stem
    ///
    /// Assigns a slice to the current timing slot and triggers one-shot preview.
    pub fn slicer_handle_shift_button(
        &mut self,
        stem: Stem,
        button_idx: usize,
        current_pos: usize,
    ) {
        // Use existing trigger_slice for shift+button behavior
        self.slicer_states[stem as usize].trigger_slice(current_pos, button_idx);
    }

    /// Load a step sequence onto a stem's slicer
    ///
    /// Replaces the entire sequence for this stem. Called by engine when
    /// loading per-stem presets.
    pub fn slicer_load_sequence(&mut self, stem_idx: usize, sequence: super::slicer::StepSequence) {
        if stem_idx < self.slicer_states.len() {
            self.slicer_states[stem_idx].load_sequence(sequence);
        }
    }

    /// Reset the slicer queue to default order
    pub fn slicer_reset_queue(&mut self, stem: Stem) {
        self.slicer_states[stem as usize].reset_queue();
    }

    /// Set the slicer buffer size in bars
    pub fn set_slicer_buffer_bars(&mut self, stem: Stem, bars: u32) {
        self.slicer_states[stem as usize].set_buffer_bars(bars);
    }

    /// Beat jump forward by beat_jump_size beats (equals loop length)
    pub fn beat_jump_forward(&mut self) {
        if let Some(track) = &self.track {
            let beats = &track.metadata.beat_grid.beats;
            let jump_size = self.beat_jump_size() as usize;
            let current_idx = beats
                .iter()
                .position(|&b| b as usize >= self.position)
                .unwrap_or(0);
            let target_idx = (current_idx + jump_size).min(beats.len().saturating_sub(1));
            if let Some(&target_pos) = beats.get(target_idx) {
                let old_position = self.position;
                self.position = target_pos as usize;
                self.sync_position_atomic();

                // If loop is active, move the loop by the same distance and snap to grid
                if self.loop_state.active {
                    let jump_distance = self.position.saturating_sub(old_position);
                    let new_start = self.loop_state.start.saturating_add(jump_distance);
                    let new_end = self.loop_state.end.saturating_add(jump_distance);
                    // Snap loop boundaries to beat grid
                    self.loop_state.start = self.snap_to_beat(new_start);
                    self.loop_state.end = self.snap_to_beat(new_end);
                    self.sync_loop_atomic();
                }
            }
        }
    }

    /// Beat jump backward by beat_jump_size beats (equals loop length)
    pub fn beat_jump_backward(&mut self) {
        if let Some(track) = &self.track {
            let beats = &track.metadata.beat_grid.beats;
            let jump_size = self.beat_jump_size() as usize;
            let current_idx = beats
                .iter()
                .position(|&b| b as usize >= self.position)
                .unwrap_or(0);
            let target_idx = current_idx.saturating_sub(jump_size);
            if let Some(&target_pos) = beats.get(target_idx) {
                let old_position = self.position;
                self.position = target_pos as usize;
                self.sync_position_atomic();

                // If loop is active, move the loop by the same distance (backward) and snap to grid
                if self.loop_state.active {
                    let jump_distance = old_position.saturating_sub(self.position);
                    let new_start = self.loop_state.start.saturating_sub(jump_distance);
                    let new_end = self.loop_state.end.saturating_sub(jump_distance);
                    // Snap loop boundaries to beat grid
                    self.loop_state.start = self.snap_to_beat(new_start);
                    self.loop_state.end = self.snap_to_beat(new_end);
                    self.sync_loop_atomic();
                }
            }
        }
    }

    // --- Hot cues ---

    /// Handle hot cue button press (CDJ-style with preview)
    ///
    /// - Empty slot: set hot cue at current position (snapped to beat)
    /// - With shift: delete hot cue
    /// - When playing: jump to hot cue and keep playing
    /// - When stopped: preview mode (play from cue, release returns to original position)
    pub fn hot_cue_press(&mut self, slot: usize) {
        if slot >= HOT_CUE_SLOTS || self.track.is_none() {
            return;
        }

        if self.shift_held {
            // Delete hot cue
            self.hot_cues[slot] = None;
            return;
        }

        if let Some(cue) = &self.hot_cues[slot] {
            let pos = cue.position;
            match self.state {
                PlayState::Playing => {
                    // Already playing - jump to hot cue
                    // Phase sync is handled at the engine level
                    self.position = pos;
                    self.sync_position_atomic();
                }
                PlayState::Stopped | PlayState::Cueing => {
                    // Preview mode - set main cue point to hot cue, play from cue
                    // On release, returns to the hot cue position (not the original position)
                    self.cue_point = pos;
                    self.hot_cue_preview_return = Some(pos);
                    self.position = pos;
                    self.state = PlayState::Cueing;
                    self.sync_position_atomic();
                    self.sync_cue_atomic();
                    self.sync_state_atomic();
                }
            }
        } else {
            // Empty slot - set cue (snapped to beat)
            self.set_hot_cue(slot);
        }
    }

    /// Handle hot cue button release
    ///
    /// If previewing, return to the original position and stop.
    pub fn hot_cue_release(&mut self) {
        if let Some(return_pos) = self.hot_cue_preview_return.take() {
            self.position = return_pos;
            self.state = PlayState::Stopped;
            self.sync_position_atomic();
            self.sync_state_atomic();
        }
    }

    /// Clear the hot cue preview return position
    ///
    /// Call this when transitioning to Play during a preview to prevent
    /// the release handler from jumping back to the cue position.
    pub fn clear_preview_return(&mut self) {
        self.hot_cue_preview_return = None;
    }

    /// Get a hot cue by slot index
    pub fn hot_cue(&self, slot: usize) -> Option<&HotCue> {
        self.hot_cues.get(slot).and_then(|c| c.as_ref())
    }

    /// Set shift state (for alternate button functions)
    pub fn set_shift(&mut self, held: bool) {
        self.shift_held = held;
    }

    // --- Slip mode controls ---

    /// Toggle slip mode on/off
    pub fn toggle_slip(&mut self) {
        self.slip_enabled = !self.slip_enabled;
        // Clear slip position when disabling
        if !self.slip_enabled {
            self.slip_position = None;
        }
    }

    /// Set slip mode enabled state
    pub fn set_slip_enabled(&mut self, enabled: bool) {
        self.slip_enabled = enabled;
        if !enabled {
            self.slip_position = None;
        }
    }

    /// Check if slip mode is enabled
    pub fn slip_enabled(&self) -> bool {
        self.slip_enabled
    }

    // --- Loop controls ---

    /// Toggle loop on/off
    ///
    /// When slip mode is enabled:
    /// - Entering loop: captures current position as slip position
    /// - Exiting loop: jumps to where playhead would have been (slip position)
    pub fn toggle_loop(&mut self) {
        if self.track.is_none() {
            return;
        }

        if self.loop_state.active {
            // Exiting loop
            self.loop_state.active = false;
            // If slip mode is enabled, return to slip position
            if self.slip_enabled {
                if let Some(slip_pos) = self.slip_position.take() {
                    self.position = slip_pos;
                    self.sync_position_atomic();
                }
            }
        } else {
            // Entering loop
            // If slip mode is enabled, capture current position
            if self.slip_enabled {
                self.slip_position = Some(self.position);
            }
            // Snap loop start to nearest beat
            let start = self.snap_to_beat(self.position);
            // Calculate raw end and snap to nearest beat
            let length = self.loop_state.length_samples(self.samples_per_beat());
            let raw_end = start + length;
            let end = self.snap_to_beat(raw_end);

            self.loop_state.start = start;
            self.loop_state.end = end;
            self.loop_state.active = true;
        }
        self.sync_loop_atomic();
    }

    /// Get the loop state
    pub fn loop_state(&self) -> &LoopState {
        &self.loop_state
    }

    /// Adjust loop length (positive = longer, negative = shorter)
    pub fn adjust_loop_length(&mut self, direction: i32) {
        if direction > 0 {
            self.loop_state.increase_length();
        } else if direction < 0 {
            self.loop_state.decrease_length();
        }

        // Update loop end if loop is active, snapping to beat grid
        if self.loop_state.active {
            let length = self.loop_state.length_samples(self.samples_per_beat());
            let raw_end = self.loop_state.start + length;
            let end = self.snap_to_beat(raw_end);
            self.loop_state.end = end;
        }

        // Always sync loop length index for UI (even when loop not active)
        self.sync_loop_atomic();
    }

    // --- Stem controls ---

    /// Get a reference to a stem's state
    pub fn stem(&self, stem: Stem) -> &StemState {
        &self.stems[stem as usize]
    }

    /// Get a mutable reference to a stem's state
    pub fn stem_mut(&mut self, stem: Stem) -> &mut StemState {
        &mut self.stems[stem as usize]
    }

    /// Toggle mute for a stem
    pub fn toggle_stem_mute(&mut self, stem: Stem) {
        let state = &mut self.stems[stem as usize];
        state.muted = !state.muted;
    }

    /// Set mute state for a stem (explicit set, not toggle)
    pub fn set_stem_mute(&mut self, stem: Stem, muted: bool) {
        let state = &mut self.stems[stem as usize];
        state.muted = muted;
    }

    /// Toggle solo for a stem
    pub fn toggle_stem_solo(&mut self, stem: Stem) {
        let state = &mut self.stems[stem as usize];
        state.soloed = !state.soloed;
    }

    /// Set solo state for a stem (explicit set, not toggle)
    pub fn set_stem_solo(&mut self, stem: Stem, soloed: bool) {
        let state = &mut self.stems[stem as usize];
        state.soloed = soloed;
    }

    /// Get a reference to a stem's effect chain by index
    pub fn stem_chain(&self, index: usize) -> Option<&EffectChain> {
        self.stems.get(index).map(|s| &s.chain)
    }

    /// Get a mutable reference to a stem's effect chain by index
    pub fn stem_chain_mut(&mut self, index: usize) -> Option<&mut EffectChain> {
        self.stems.get_mut(index).map(|s| &mut s.chain)
    }

    /// Trigger hot cue (for UI compatibility)
    pub fn trigger_hot_cue(&mut self, slot: usize) {
        self.hot_cue_press(slot);
    }

    /// Set hot cue at current position (snapped to nearest beat)
    pub fn set_hot_cue(&mut self, slot: usize) {
        if slot < HOT_CUE_SLOTS && self.track.is_some() {
            let snapped_pos = self.snap_to_beat(self.position);
            self.hot_cues[slot] = Some(HotCue {
                position: snapped_pos,
                label: format!("Cue {}", slot + 1),
                color: None,
            });
        }
    }

    /// Set hot cue at a specific position (no snapping)
    ///
    /// Used when the caller has already snapped to a beat grid
    /// (e.g., when the UI has a more up-to-date beat grid than the Deck)
    pub fn set_hot_cue_position(&mut self, slot: usize, position: usize) {
        if slot < HOT_CUE_SLOTS {
            self.hot_cues[slot] = Some(HotCue {
                position,
                label: format!("Cue {}", slot + 1),
                color: None,
            });
        }
    }

    /// Clear hot cue (for UI compatibility)
    pub fn clear_hot_cue(&mut self, slot: usize) {
        if slot < HOT_CUE_SLOTS {
            self.hot_cues[slot] = None;
        }
    }

    /// Loop in - start loop at current position
    pub fn loop_in(&mut self) {
        if self.track.is_some() {
            self.loop_state.start = self.position;
            self.sync_loop_atomic();
        }
    }

    /// Loop out - end loop at current position and activate
    pub fn loop_out(&mut self) {
        if self.track.is_some() {
            self.loop_state.end = self.position;
            self.loop_state.active = true;
            self.sync_loop_atomic();
        }
    }

    /// Turn loop off
    pub fn loop_off(&mut self) {
        self.loop_state.active = false;
        self.sync_loop_atomic();
    }

    /// Check if loop is currently active
    pub fn is_loop_active(&self) -> bool {
        self.loop_state.active
    }

    /// Get loop bounds (start, end) as sample positions
    pub fn loop_bounds(&self) -> (usize, usize) {
        (self.loop_state.start, self.loop_state.end)
    }

    /// Set loop bounds and activate the loop
    ///
    /// Used for recalling saved loops - sets exact positions without snapping.
    pub fn set_loop(&mut self, start: usize, end: usize) {
        self.loop_state.start = start;
        self.loop_state.end = end;
        self.loop_state.active = true;
        self.sync_loop_atomic();
    }

    /// Check if any stem is soloed
    fn any_stem_soloed(&self) -> bool {
        self.stems.iter().any(|s| s.soloed)
    }

    // --- Audio processing ---

    /// Advance the playhead and fill the stretch_input buffer with processed audio
    ///
    /// This is called from the audio thread to generate samples for time stretching.
    /// The deck reads `output_len * stretch_ratio` samples, processes them through
    /// effect chains, and writes to `stretch_input`. The engine then passes
    /// stretch_input through the time stretcher to produce exactly `output_len` samples.
    ///
    /// Uses Rayon for parallel stem processing - each stem is processed on a
    /// separate thread, then results are summed. This provides ~3-4x speedup
    /// on multi-core CPUs for effect-heavy workloads.
    ///
    /// ## Parameters
    ///
    /// - `stretch_input`: Buffer to fill with processed audio (will be resized to samples_to_read)
    /// - `output_len`: Target output length (JACK buffer size) - used with stretch_ratio
    /// - `compensator`: Optional per-stem latency compensation
    /// - `deck_id`: Deck index for latency compensator
    ///
    /// ## Time Stretch Behavior
    ///
    /// - `stretch_ratio > 1.0` (speedup): reads MORE samples, stretcher compresses to output_len
    /// - `stretch_ratio < 1.0` (slowdown): reads FEWER samples, stretcher expands to output_len
    /// - `stretch_ratio = 1.0`: reads output_len samples (no stretching)
    pub fn process(
        &mut self,
        stretch_input: &mut StereoBuffer,
        output_len: usize,
        compensator: Option<&mut LatencyCompensator>,
        deck_id: usize,
    ) {
        let Some(track) = &self.track else {
            stretch_input.set_len_from_capacity(output_len);
            stretch_input.fill_silence();
            return;
        };

        // If stopped, output silence (prevents repeating buffer buzz)
        if self.state == PlayState::Stopped {
            stretch_input.set_len_from_capacity(output_len);
            stretch_input.fill_silence();
            return;
        }

        // Calculate how many samples to read based on stretch ratio
        // For speedup (ratio > 1): read MORE samples, stretcher will compress
        // For slowdown (ratio < 1): read FEWER samples, stretcher will expand
        //
        // IMPORTANT: We accumulate fractional samples to prevent drift.
        // Without this, rounding each frame loses ~0.5 samples, causing
        // ~1 second of drift per 10 minutes of playback.
        self.fractional_position += (output_len as f64) * self.stretch_ratio;
        let samples_to_read = self.fractional_position.floor() as usize;
        self.fractional_position -= samples_to_read as f64;
        let samples_to_read = samples_to_read.clamp(1, MAX_BUFFER_SIZE);

        // Set stretch_input length to samples we'll read
        stretch_input.set_len_from_capacity(samples_to_read);
        stretch_input.fill_silence();

        // Extract values needed for parallel processing (avoids borrow conflicts)
        let any_soloed = self.any_stem_soloed();
        let position = self.position;
        let duration_samples = track.duration_samples;
        let samples_per_beat = track.samples_per_beat();

        // Extract linked stem buffer references and gains before parallel section
        // This avoids borrow checker issues with stem_links in the parallel closure
        // Note: Linked buffers are pre-aligned to host timeline, so no drop marker offset needed
        let linked_stems: [Option<&StereoBuffer>; NUM_STEMS] = std::array::from_fn(|i| {
            if self.stem_links[i].is_linked_active() {
                self.stem_links[i]
                    .linked
                    .as_ref()
                    .map(|info| &*info.buffer)  // Dereference Shared<StereoBuffer> to &StereoBuffer
            } else {
                None
            }
        });

        // Extract linked stem gains for LUFS-based level matching
        // Each gain brings the linked stem to the host track's level
        let linked_gains: [f32; NUM_STEMS] = std::array::from_fn(|i| self.stem_links[i].gain);

        // Set working length of all pre-allocated stem buffers (real-time safe: no allocation)
        // Capacity remains at MAX_BUFFER_SIZE, only the length field changes
        for buf in &mut self.stem_buffers {
            buf.set_len_from_capacity(samples_to_read);
        }

        // Parallel stem processing with Rayon
        // Each stem has its own buffer, enabling true parallelism without contention
        // The closure captures immutable references to track data, while each iteration
        // gets exclusive mutable access to its own (stem_state, stem_buffer, slicer_state) tuple
        self.stems
            .par_iter_mut()
            .zip(self.stem_buffers.par_iter_mut())
            .zip(self.slicer_states.par_iter_mut())
            .enumerate()
            .for_each(|(stem_idx, ((stem_state, stem_buffer), slicer_state))| {
                let stem = Stem::ALL[stem_idx];

                // Skip if muted, or if others are soloed and this isn't
                if stem_state.muted || (any_soloed && !stem_state.soloed) {
                    stem_buffer.fill_silence();
                    return;
                }

                // Get the original stem data from track (always needed for slicer fallback)
                let stem_data = track.stems.get(stem);
                let buf_slice = stem_buffer.as_mut_slice();

                // Check for linked stem - read from linked buffer if active
                // Linked buffers are pre-aligned to host timeline (same length as host track)
                // so we can read directly at host position without offset calculation
                if let Some(linked_buffer) = linked_stems[stem_idx] {
                    // LINKED STEM PATH: Read from pre-aligned linked buffer
                    // Note: Gain is applied AFTER slicer processing to avoid double-gain
                    let linked_len = linked_buffer.len();
                    let linked_slice = linked_buffer.as_slice();

                    for i in 0..samples_to_read {
                        let read_pos = position + i;
                        if read_pos < linked_len {
                            buf_slice[i] = linked_slice[read_pos];
                        } else {
                            // Outside linked buffer bounds - output silence
                            buf_slice[i] = StereoSample::silence();
                        }
                    }
                } else {
                    // ORIGINAL STEM PATH: Read from track's stem buffer
                    for i in 0..samples_to_read {
                        let read_pos = position + i;
                        if read_pos < duration_samples {
                            buf_slice[i] = stem_data[read_pos];
                        } else {
                            buf_slice[i] = StereoSample::silence();
                        }
                    }
                }

                // Apply slicer if enabled (works on whichever stem is active - original or linked)
                // The slicer remaps sample positions through the playback queue
                // Uses the appropriate buffer (linked or original) for slice lookup
                if slicer_state.is_enabled() {
                    if let Some(linked_buffer) = linked_stems[stem_idx] {
                        // Slicer uses linked buffer when linked stem is active
                        slicer_state.process(
                            stem_buffer,
                            position,
                            samples_per_beat,
                            linked_buffer.as_slice(),
                            linked_buffer.len(),
                        );
                    } else {
                        // Slicer uses original stem buffer
                        slicer_state.process(
                            stem_buffer,
                            position,
                            samples_per_beat,
                            stem_data.as_slice(),
                            duration_samples,
                        );
                    }
                }

                // Apply LUFS-based gain correction for linked stems
                // This is done AFTER slicer processing to avoid double-gain issues
                // (slicer reads from raw buffer, so we apply gain once at the end)
                if linked_stems[stem_idx].is_some() {
                    let gain = linked_gains[stem_idx];
                    if gain != 1.0 {
                        for sample in stem_buffer.as_mut_slice() {
                            sample.left *= gain;
                            sample.right *= gain;
                        }
                    }
                }

                // Process through effect chain (pass any_soloed for solo logic)
                stem_state.chain.process(stem_buffer, any_soloed);
            });

        // Apply per-stem latency compensation (sequential - must happen after parallel)
        // Each stem is delayed by (max_latency - stem_latency) samples to align all stems
        if let Some(comp) = compensator {
            for (stem_idx, stem_buffer) in self.stem_buffers.iter_mut().enumerate() {
                comp.process(deck_id, stem_idx, stem_buffer);
            }
        }

        // Sum all stem buffers to stretch_input (sequential - fast O(n) operation)
        // This happens after parallel processing and compensation
        for stem_buffer in &self.stem_buffers {
            stretch_input.add_buffer(stem_buffer);
        }

        // Apply LUFS-based gain compensation (if not unity)
        // This normalizes tracks to the configured target loudness
        if self.lufs_gain != 1.0 {
            stretch_input.scale(self.lufs_gain);
        }

        // Advance playhead by samples actually read (not output_len!)
        // This ensures playback speed matches the stretch ratio
        if self.state == PlayState::Playing || self.state == PlayState::Cueing {
            self.position += samples_to_read;

            // Update slip position if slip mode is enabled and we're looping
            // This tracks where the playhead WOULD be if we weren't looping
            if self.slip_enabled && self.loop_state.active {
                if let Some(ref mut slip_pos) = self.slip_position {
                    *slip_pos += samples_to_read;
                }
            }

            // Handle looping - preserve overshoot to prevent drift
            // Without this, we lose samples each loop iteration, causing
            // cumulative drift (~33ms over 10 minutes of looping)
            if self.loop_state.active && self.position >= self.loop_state.end {
                let loop_length = self.loop_state.end.saturating_sub(self.loop_state.start);
                if loop_length > 0 {
                    let overshoot = self.position - self.loop_state.end;
                    // Use modulo in case overshoot exceeds loop length (very short loops)
                    self.position = self.loop_state.start + (overshoot % loop_length);
                }
            }

            // Handle end of track
            if self.position >= track.duration_samples {
                self.position = track.duration_samples.saturating_sub(1);
                self.state = PlayState::Stopped;
                self.sync_state_atomic();
            }

            // Sync position to atomics for lock-free UI reads
            self.sync_position_atomic();
        }
    }

    /// Get the maximum latency across all stem effect chains
    pub fn max_stem_latency(&self) -> u32 {
        self.stems
            .iter()
            .map(|s| s.chain.total_latency())
            .max()
            .unwrap_or(0)
    }

    // ────────────────────────────────────────────────────────────────────────────────
    // Linked Stems
    // ────────────────────────────────────────────────────────────────────────────────

    /// Get the drop marker position for this track
    ///
    /// Returns None if no drop marker is set.
    pub fn drop_marker(&self) -> Option<u64> {
        self.drop_marker
    }

    /// Set a linked stem for a stem slot
    ///
    /// The linked stem info should already be pre-stretched to match
    /// this deck's BPM when this is called.
    pub fn set_linked_stem(&mut self, stem_idx: usize, info: super::LinkedStemInfo) {
        if stem_idx >= NUM_STEMS {
            return;
        }
        self.stem_links[stem_idx].set_linked(info);
        // Calculate gain correction based on host and linked LUFS
        self.stem_links[stem_idx].update_gain(self.host_lufs);
        self.linked_stem_atomics.sync_from_stem_link(stem_idx, &self.stem_links[stem_idx]);
    }

    /// Toggle a linked stem between original and linked
    ///
    /// Returns the new state (true = linked is active, false = original is active).
    /// Returns false if no linked stem exists for this slot.
    pub fn toggle_linked_stem(&mut self, stem_idx: usize) -> bool {
        if stem_idx >= NUM_STEMS {
            return false;
        }
        let result = self.stem_links[stem_idx].toggle();
        self.linked_stem_atomics.sync_from_stem_link(stem_idx, &self.stem_links[stem_idx]);
        result
    }

    /// Get a reference to a stem link
    pub fn stem_link(&self, stem_idx: usize) -> Option<&StemLink> {
        self.stem_links.get(stem_idx)
    }

    /// Get a mutable reference to a stem link
    pub fn stem_link_mut(&mut self, stem_idx: usize) -> Option<&mut StemLink> {
        self.stem_links.get_mut(stem_idx)
    }

    /// Check if any stem has a linked stem
    pub fn has_any_linked_stem(&self) -> bool {
        self.stem_links.iter().any(|link| link.has_linked())
    }

    /// Check if any linked stem is currently active
    pub fn has_any_active_linked_stem(&self) -> bool {
        self.stem_links.iter().any(|link| link.is_linked_active())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deck_creation() {
        let deck = Deck::new(DeckId::new(0));
        assert!(!deck.has_track());
        assert_eq!(deck.state(), PlayState::Stopped);
        assert_eq!(deck.position(), 0);
    }

    #[test]
    fn test_loop_state() {
        let mut loop_state = LoopState::default();
        assert_eq!(loop_state.length_beats(), 1.0); // First element (1 beat minimum)

        loop_state.increase_length();
        assert_eq!(loop_state.length_beats(), 2.0);

        loop_state.increase_length();
        assert_eq!(loop_state.length_beats(), 4.0);

        loop_state.decrease_length();
        assert_eq!(loop_state.length_beats(), 2.0);
    }

    #[test]
    fn test_hot_cue_operations() {
        let mut deck = Deck::new(DeckId::new(0));

        // Without a track, hot cue operations should be no-ops
        deck.hot_cue_press(0);
        assert!(deck.hot_cue(0).is_none());

        // Test with shift to delete (should be no-op on empty slot)
        deck.set_shift(true);
        deck.hot_cue_press(0);
        assert!(deck.hot_cue(0).is_none());
    }
}
