//! Deck - Individual track player with stems and effect chains

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use rayon::prelude::*;

use crate::audio_file::LoadedTrack;
use crate::effect::EffectChain;
use crate::types::{
    DeckId, PlayState, Stem, StereoBuffer, StereoSample, TransportPosition,
    NUM_STEMS, SAMPLE_RATE,
};

use super::{LatencyCompensator, MAX_BUFFER_SIZE};

/// Number of hot cue slots per deck
pub const HOT_CUE_SLOTS: usize = 8;

/// Number of loop lengths available (0.25, 0.5, 1, 2, 4, 8, 16 beats)
pub const LOOP_LENGTHS: [f64; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

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
    /// Beat jump size in beats (1, 4, 8, 16, 32)
    beat_jump_size: i32,
    /// Position to return to after hot cue preview (None = not previewing)
    hot_cue_preview_return: Option<usize>,
    /// Lock-free state for UI access (position, play state, loop state)
    /// The UI can read these atomics without acquiring the engine mutex
    atomics: Arc<DeckAtomics>,
    /// Pre-allocated buffers for parallel stem processing (real-time safe)
    /// One buffer per stem enables parallel processing with Rayon
    /// Capacity is MAX_BUFFER_SIZE to handle any JACK buffer size
    stem_buffers: [StereoBuffer; NUM_STEMS],
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
            beat_jump_size: 4, // Default 4 beats
            hot_cue_preview_return: None,
            atomics: Arc::new(DeckAtomics::new()),
            stem_buffers: std::array::from_fn(|_| StereoBuffer::silence(MAX_BUFFER_SIZE)),
        }
    }

    /// Get a reference to the lock-free atomic state
    ///
    /// The UI can clone this Arc and read position/state without acquiring
    /// the engine mutex, eliminating lock contention during playback.
    pub fn atomics(&self) -> Arc<DeckAtomics> {
        Arc::clone(&self.atomics)
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
    }

    /// Write cue point to atomics (internal helper)
    #[inline]
    fn sync_cue_atomic(&self) {
        self.atomics.cue_point.store(self.cue_point as u64, Ordering::Relaxed);
    }

    /// Get the deck ID
    pub fn id(&self) -> DeckId {
        self.id
    }

    /// Load a track into this deck
    pub fn load_track(&mut self, track: LoadedTrack) {
        // Import cue points from track metadata
        let hot_cues: [Option<HotCue>; HOT_CUE_SLOTS] = std::array::from_fn(|i| {
            track.metadata.cue_points.get(i).map(|cue| HotCue {
                position: cue.sample_position as usize,
                label: cue.label.clone(),
                color: cue.color.clone(),
            })
        });

        // Set cue point to first beat if available, otherwise start of track
        let first_beat = track
            .metadata
            .beat_grid
            .first_beat_sample
            .map(|b| b as usize)
            .unwrap_or(0);

        self.track = Some(track);
        self.position = first_beat;
        self.state = PlayState::Stopped;
        self.cue_point = first_beat;
        self.hot_cues = hot_cues;
        self.loop_state = LoopState::default();
        self.scratch_offset = 0.0;

        // Sync atomics for lock-free UI reads
        self.sync_position_atomic();
        self.sync_state_atomic();
        self.sync_cue_atomic();
        self.sync_loop_atomic();

        // Reset all effect chains
        for stem in &mut self.stems {
            stem.chain.reset();
        }
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
            PlayState::Stopped | PlayState::Cueing => self.play(),
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

    /// Get the current cue point position
    pub fn cue_point(&self) -> usize {
        self.cue_point
    }

    /// Jump to a specific sample position
    pub fn seek(&mut self, position: usize) {
        if let Some(track) = &self.track {
            self.position = position.min(track.duration_samples.saturating_sub(1));
            self.sync_position_atomic();
        }
    }

    /// Set the beat jump size in beats
    pub fn set_beat_jump_size(&mut self, beats: i32) {
        self.beat_jump_size = beats.clamp(1, 32);
    }

    /// Get the current beat jump size in beats
    pub fn beat_jump_size(&self) -> i32 {
        self.beat_jump_size
    }

    /// Beat jump forward by beat_jump_size beats
    pub fn beat_jump_forward(&mut self) {
        if let Some(track) = &self.track {
            let beats = &track.metadata.beat_grid.beats;
            let current_idx = beats
                .iter()
                .position(|&b| b as usize >= self.position)
                .unwrap_or(0);
            let target_idx =
                (current_idx + self.beat_jump_size as usize).min(beats.len().saturating_sub(1));
            if let Some(&target_pos) = beats.get(target_idx) {
                self.position = target_pos as usize;
                self.sync_position_atomic();
            }
        }
    }

    /// Beat jump backward by beat_jump_size beats
    pub fn beat_jump_backward(&mut self) {
        if let Some(track) = &self.track {
            let beats = &track.metadata.beat_grid.beats;
            let current_idx = beats
                .iter()
                .position(|&b| b as usize >= self.position)
                .unwrap_or(0);
            let target_idx = current_idx.saturating_sub(self.beat_jump_size as usize);
            if let Some(&target_pos) = beats.get(target_idx) {
                self.position = target_pos as usize;
                self.sync_position_atomic();
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
                    // Already playing - just jump
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

    /// Get a hot cue by slot index
    pub fn hot_cue(&self, slot: usize) -> Option<&HotCue> {
        self.hot_cues.get(slot).and_then(|c| c.as_ref())
    }

    /// Set shift state (for alternate button functions)
    pub fn set_shift(&mut self, held: bool) {
        self.shift_held = held;
    }

    // --- Loop controls ---

    /// Toggle loop on/off
    pub fn toggle_loop(&mut self) {
        if self.track.is_none() {
            return;
        }

        if self.loop_state.active {
            self.loop_state.active = false;
        } else {
            // Set loop start at current position
            let length = self.loop_state.length_samples(self.samples_per_beat());
            self.loop_state.start = self.position;
            self.loop_state.end = self.position + length;
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

        // Update loop end if loop is active
        if self.loop_state.active {
            let length = self.loop_state.length_samples(self.samples_per_beat());
            self.loop_state.end = self.loop_state.start + length;
            self.sync_loop_atomic();
        }
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

    /// Toggle solo for a stem
    pub fn toggle_stem_solo(&mut self, stem: Stem) {
        let state = &mut self.stems[stem as usize];
        state.soloed = !state.soloed;
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

    /// Check if any stem effect chain is soloed
    fn any_stem_soloed(&self) -> bool {
        self.stems.iter().any(|s| s.chain.is_soloed())
    }

    // --- Audio processing ---

    /// Advance the playhead and fill the output buffer with processed audio
    ///
    /// This is called from the audio thread to generate samples.
    /// Returns the summed stereo output after stem processing.
    ///
    /// Uses Rayon for parallel stem processing - each stem is processed on a
    /// separate thread, then results are summed. This provides ~3-4x speedup
    /// on multi-core CPUs for effect-heavy workloads.
    ///
    /// ## Latency Compensation
    ///
    /// If `compensator` is provided, per-stem latency compensation is applied
    /// after effect processing but before summing. This ensures all stems are
    /// sample-aligned regardless of different effect chain latencies.
    pub fn process(
        &mut self,
        output: &mut StereoBuffer,
        compensator: Option<&mut LatencyCompensator>,
        deck_id: usize,
    ) {
        let Some(track) = &self.track else {
            output.fill_silence();
            return;
        };

        // If stopped, output silence (prevents repeating buffer buzz)
        if self.state == PlayState::Stopped {
            output.fill_silence();
            return;
        }

        // Fill output with silence (will add processed stems)
        output.fill_silence();

        // Extract values needed for parallel processing (avoids borrow conflicts)
        let any_soloed = self.any_stem_soloed();
        let buffer_len = output.len();
        let position = self.position;
        let duration_samples = track.duration_samples;

        // Set working length of all pre-allocated stem buffers (real-time safe: no allocation)
        // Capacity remains at MAX_BUFFER_SIZE, only the length field changes
        for buf in &mut self.stem_buffers {
            buf.set_len_from_capacity(buffer_len);
        }

        // Parallel stem processing with Rayon
        // Each stem has its own buffer, enabling true parallelism without contention
        // The closure captures immutable references to track data, while each iteration
        // gets exclusive mutable access to its own (stem_state, stem_buffer) pair
        self.stems
            .par_iter_mut()
            .zip(self.stem_buffers.par_iter_mut())
            .enumerate()
            .for_each(|(stem_idx, (stem_state, stem_buffer))| {
                let stem = Stem::ALL[stem_idx];

                // Skip if muted, or if others are soloed and this isn't
                if stem_state.muted || (any_soloed && !stem_state.soloed) {
                    stem_buffer.fill_silence();
                    return;
                }

                // Copy samples from track to stem buffer
                let stem_data = track.stems.get(stem);
                let buf_slice = stem_buffer.as_mut_slice();
                for i in 0..buffer_len {
                    let read_pos = position + i;
                    if read_pos < duration_samples {
                        buf_slice[i] = stem_data[read_pos];
                    } else {
                        buf_slice[i] = StereoSample::silence();
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

        // Sum all stem buffers to output (sequential - fast O(n) operation)
        // This happens after parallel processing and compensation
        for stem_buffer in &self.stem_buffers {
            output.add_buffer(stem_buffer);
        }

        // Advance playhead (only if playing)
        if self.state == PlayState::Playing || self.state == PlayState::Cueing {
            self.position += buffer_len;

            // Handle looping
            if self.loop_state.active && self.position >= self.loop_state.end {
                self.position = self.loop_state.start;
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
        assert_eq!(loop_state.length_beats(), 0.25); // First element

        loop_state.increase_length();
        assert_eq!(loop_state.length_beats(), 0.5);

        loop_state.increase_length();
        assert_eq!(loop_state.length_beats(), 1.0);

        loop_state.decrease_length();
        assert_eq!(loop_state.length_beats(), 0.5);
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
