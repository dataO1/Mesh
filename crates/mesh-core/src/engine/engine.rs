//! Main audio engine - ties together decks, mixer, and time-stretching

use crate::music::semitones_to_match;
use crate::timestretch::TimeStretcher;
use crate::types::{DeckId, PlayState, Stem, StereoBuffer, NUM_DECKS};

use super::slicer::SlicerPreset;
use super::{Deck, DeckAtomics, EngineCommand, LatencyCompensator, Mixer, PreparedTrack};

/// Global BPM range
pub const MIN_BPM: f64 = 30.0;
pub const MAX_BPM: f64 = 200.0;
pub const DEFAULT_BPM: f64 = 128.0;

/// Audio buffer size for processing
pub const BUFFER_SIZE: usize = 256;

/// Maximum buffer size to pre-allocate for real-time safety
/// Covers all common JACK configurations (64, 128, 256, 512, 1024, 2048, 4096)
/// Pre-allocating to this size eliminates allocations in the audio callback
pub const MAX_BUFFER_SIZE: usize = 8192;

/// The main audio engine
///
/// Manages 4 decks, global BPM synchronization, latency compensation,
/// and mixing to produce master and cue outputs.
pub struct AudioEngine {
    /// The 4 decks
    decks: [Deck; NUM_DECKS],
    /// Global BPM for all decks
    global_bpm: f64,
    /// Mixer for combining deck outputs
    mixer: Mixer,
    /// Global latency compensator
    latency_compensator: LatencyCompensator,
    /// Per-deck time stretchers (applied after stem summing)
    stretchers: [TimeStretcher; NUM_DECKS],
    /// Pre-allocated buffers for deck processing
    deck_buffers: [StereoBuffer; NUM_DECKS],
    /// Pre-allocated buffer for time-stretched output
    stretch_input: StereoBuffer,
    /// Output sample rate (from JACK) - used for sample rate conversion
    output_sample_rate: u32,

    // ─────────────────────────────────────────────────────────────
    // Inter-deck phase synchronization
    // ─────────────────────────────────────────────────────────────
    /// Frame count when each deck started playing (None if stopped)
    /// Used to determine master deck (longest playing = lowest start frame)
    deck_play_start: [Option<u64>; NUM_DECKS],
    /// Global frame counter (incremented each process() call)
    /// Used for tracking relative play start times
    frame_counter: u64,
    /// Whether phase sync is enabled (can be toggled via config)
    phase_sync_enabled: bool,
    /// Slicer presets (8 presets, each with per-stem patterns)
    /// When a preset button is pressed, patterns are loaded to all stems that have
    /// a defined pattern in the preset (others are bypassed).
    slicer_presets: [SlicerPreset; 8],
}

impl AudioEngine {
    /// Create a new audio engine with the specified output sample rate
    ///
    /// The sample rate should come from JACK (or other audio backend).
    /// Audio files will be resampled to match this rate on load.
    pub fn new_with_sample_rate(output_sample_rate: u32) -> Self {
        log::info!("AudioEngine created with sample rate: {} Hz", output_sample_rate);
        Self {
            decks: std::array::from_fn(|i| Deck::new(DeckId::new(i))),
            global_bpm: DEFAULT_BPM,
            mixer: Mixer::new(),
            latency_compensator: LatencyCompensator::new(),
            stretchers: std::array::from_fn(|_| TimeStretcher::new_with_sample_rate(output_sample_rate)),
            deck_buffers: std::array::from_fn(|_| StereoBuffer::silence(MAX_BUFFER_SIZE)),
            stretch_input: StereoBuffer::silence(MAX_BUFFER_SIZE),
            output_sample_rate,
            // Phase sync tracking
            deck_play_start: [None; NUM_DECKS],
            frame_counter: 0,
            phase_sync_enabled: true, // Enabled by default
            // Default slicer presets (can be overwritten via SetSlicerPresets command)
            // These defaults apply patterns to drums only (for backward compatibility)
            slicer_presets: [
                SlicerPreset::drums_only(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]), // Sequential
                SlicerPreset::drums_only(&[0, 0, 2, 2, 4, 4, 6, 6, 8, 8, 10, 10, 12, 12, 14, 14]), // Half-time
                SlicerPreset::drums_only(&[0, 1, 0, 3, 4, 5, 4, 7, 8, 9, 8, 11, 12, 13, 12, 15]),  // Kick emphasis
                SlicerPreset::drums_only(&[0, 1, 2, 2, 4, 5, 6, 6, 8, 9, 6, 6, 12, 6, 6, 6]),      // Snare roll
                SlicerPreset::drums_only(&[0, 1, 2, 3, 4, 4, 6, 7, 8, 9, 10, 11, 12, 12, 14, 15]), // Shuffle
                SlicerPreset::drums_only(&[15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0]), // Full reverse
                SlicerPreset::drums_only(&[0, 0, 2, 2, 4, 4, 6, 6, 0, 0, 2, 2, 4, 4, 6, 6]),       // Stutter
                SlicerPreset::drums_only(&[0, 2, 4, 6, 0, 2, 4, 6, 0, 2, 4, 6, 0, 2, 4, 6]),       // Rapid fire
            ],
        }
    }

    /// Create a new audio engine with default sample rate (48000 Hz)
    pub fn new() -> Self {
        Self::new_with_sample_rate(crate::types::SAMPLE_RATE)
    }

    /// Get the output sample rate
    pub fn sample_rate(&self) -> u32 {
        self.output_sample_rate
    }

    /// Get a reference to a deck
    pub fn deck(&self, id: usize) -> Option<&Deck> {
        self.decks.get(id)
    }

    /// Get a mutable reference to a deck
    pub fn deck_mut(&mut self, id: usize) -> Option<&mut Deck> {
        self.decks.get_mut(id)
    }

    /// Get lock-free atomics for all decks
    ///
    /// Returns Arc references to each deck's atomic state. The UI can clone
    /// these and read position/state without acquiring the engine mutex.
    /// Call this once during initialization and store the Arcs.
    pub fn deck_atomics(&self) -> [std::sync::Arc<DeckAtomics>; NUM_DECKS] {
        std::array::from_fn(|i| self.decks[i].atomics())
    }

    /// Get slicer atomics for the drums stem on all decks
    ///
    /// Returns Arc references to each deck's drums slicer atomics.
    /// The UI can read slicer state (active, queue, current slice) without blocking.
    pub fn slicer_atomics(&self) -> [std::sync::Arc<super::slicer::SlicerAtomics>; NUM_DECKS] {
        use crate::types::Stem;
        std::array::from_fn(|i| self.decks[i].slicer_atomics(Stem::Drums))
    }

    /// Get slicer atomics for all 4 stems on a specific deck
    ///
    /// Returns Arc references to each stem's slicer atomics for the specified deck.
    /// Used by mesh-cue which has a single deck but may enable slicer on any stem.
    pub fn slicer_atomics_for_deck(&self, deck: usize) -> [std::sync::Arc<super::slicer::SlicerAtomics>; 4] {
        use crate::types::Stem;
        [
            self.decks[deck].slicer_atomics(Stem::Vocals),
            self.decks[deck].slicer_atomics(Stem::Drums),
            self.decks[deck].slicer_atomics(Stem::Bass),
            self.decks[deck].slicer_atomics(Stem::Other),
        ]
    }

    /// Get linked stem atomics for all decks
    ///
    /// Returns Arc references to each deck's linked stem atomics.
    /// The UI can read linked stem state (has_linked, use_linked) without blocking.
    pub fn linked_stem_atomics(&self) -> [std::sync::Arc<super::LinkedStemAtomics>; NUM_DECKS] {
        std::array::from_fn(|i| self.decks[i].linked_stem_atomics())
    }

    /// Get a reference to the mixer
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Get a mutable reference to the mixer
    pub fn mixer_mut(&mut self) -> &mut Mixer {
        &mut self.mixer
    }

    // ─────────────────────────────────────────────────────────────
    // Inter-deck phase synchronization
    // ─────────────────────────────────────────────────────────────

    /// Find the master deck (longest playing)
    ///
    /// The master is the deck that has been playing the longest (lowest start frame).
    /// Other decks synchronize their phase to the master when starting or jumping.
    ///
    /// Returns None if no deck is currently playing.
    fn master_deck_id(&self) -> Option<usize> {
        self.deck_play_start
            .iter()
            .enumerate()
            .filter_map(|(id, start)| start.map(|s| (id, s)))
            .min_by_key(|(_, start)| *start)
            .map(|(id, _)| id)
    }

    /// Calculate phase-locked position for a deck syncing to master
    ///
    /// When a deck starts playing or jumps while another deck is playing,
    /// this calculates the position that aligns beats across both decks.
    ///
    /// # Algorithm
    /// 1. Find master deck's phase offset from its beat grid
    /// 2. Find nearest beat to target position on slave deck
    /// 3. Apply master's phase offset to land at same relative position
    ///
    /// Returns the original position if phase sync is disabled, no master,
    /// or beat grid is unavailable.
    fn phase_locked_position(&self, deck_id: usize, target_position: usize) -> usize {
        // Skip if phase sync is disabled
        if !self.phase_sync_enabled {
            return target_position;
        }

        // Find master deck
        let Some(master_id) = self.master_deck_id() else {
            return target_position; // No master, no adjustment
        };

        // Don't sync to self
        if master_id == deck_id {
            return target_position;
        }

        let master = &self.decks[master_id];
        let slave = &self.decks[deck_id];

        // Both need loaded tracks with beat grids
        let Some(master_track) = master.track() else {
            return target_position;
        };
        let Some(slave_track) = slave.track() else {
            return target_position;
        };

        // Check master has a beat grid
        if master_track.metadata.beat_grid.beats.is_empty() {
            return target_position;
        }

        // Calculate master's phase offset from its beat grid
        let master_pos = master.position() as usize;
        let master_nearest_beat = master.snap_to_beat(master_pos);
        let phase_offset = master_pos as i64 - master_nearest_beat as i64;

        // Find nearest beat to slave's target position
        let slave_nearest_beat = slave.snap_to_beat(target_position);

        // Apply master's phase offset to slave
        let result = (slave_nearest_beat as i64 + phase_offset).max(0) as usize;

        // Clamp to track bounds
        result.min(slave_track.duration_samples.saturating_sub(1))
    }

    /// Calculate phase-locked position for a deck jumping while it's the master
    ///
    /// When the master deck jumps (e.g., hot cue), it preserves its own phase
    /// so that slave decks remain in sync. Without this, the master's phase
    /// would change and slaves would drift out of alignment.
    ///
    /// Returns the original position if phase sync is disabled, no track,
    /// or beat grid is unavailable.
    fn self_phase_locked_position(&self, deck_id: usize, target_position: usize) -> usize {
        // Skip if phase sync is disabled
        if !self.phase_sync_enabled {
            return target_position;
        }

        let deck = &self.decks[deck_id];

        let Some(track) = deck.track() else {
            return target_position;
        };

        // Check we have a beat grid
        if track.metadata.beat_grid.beats.is_empty() {
            return target_position;
        }

        // Get current phase offset
        let current_pos = deck.position() as usize;
        let current_nearest_beat = deck.snap_to_beat(current_pos);
        let phase_offset = current_pos as i64 - current_nearest_beat as i64;

        // Apply to target
        let target_nearest_beat = deck.snap_to_beat(target_position);
        let result = (target_nearest_beat as i64 + phase_offset).max(0) as usize;

        result.min(track.duration_samples.saturating_sub(1))
    }

    /// Apply phase sync correction after a position jump
    ///
    /// Call this after any operation that moves a playing deck's position
    /// (beat jump, hot cue, etc.). Handles master/slave distinction and
    /// edge cases (no master, sync disabled, no beat grid).
    ///
    /// Returns the phase-corrected position, or None if no correction needed.
    fn apply_post_jump_phase_sync(&self, deck_id: usize) -> Option<usize> {
        // Only apply if deck is playing
        if self.decks[deck_id].state() != PlayState::Playing {
            return None;
        }

        let master_id = self.master_deck_id();
        let is_master = master_id == Some(deck_id);
        let current_pos = self.decks[deck_id].position() as usize;

        let synced_pos = if is_master {
            // Master: preserve own phase for slaves to follow
            self.self_phase_locked_position(deck_id, current_pos)
        } else if master_id.is_some() {
            // Slave: sync to master
            self.phase_locked_position(deck_id, current_pos)
        } else {
            // No master (first/only deck playing)
            return None;
        };

        // Only return if position actually changed
        if synced_pos != current_pos {
            Some(synced_pos)
        } else {
            None
        }
    }

    /// Load a pre-prepared track with minimal mutex hold time
    ///
    /// This method uses `Deck::apply_prepared_track()` which only performs
    /// pointer moves and atomic stores - no allocations or string cloning.
    ///
    /// ## Real-Time Safety
    ///
    /// Mutex hold time: <1ms (vs 10-50ms for `load_track()`)
    ///
    /// The expensive work (string cloning for cue labels) is done in
    /// `PreparedTrack::prepare()` which should be called from a background thread.
    ///
    /// Note: Effect chain reset is deferred to minimize mutex hold time.
    /// Effects will reset on the next audio frame (inaudible).
    pub fn load_track_fast(&mut self, deck: usize, prepared: PreparedTrack) {
        if deck >= NUM_DECKS {
            return;
        }

        // Extract BPM before consuming prepared track
        let track_bpm = prepared.track.bpm();

        // Fast track application - only assignments and atomic stores
        if let Some(d) = self.decks.get_mut(deck) {
            d.apply_prepared_track(prepared);

            // Set stretch ratio for this deck based on track BPM vs global BPM
            let ratio = self.global_bpm / track_bpm;
            log::debug!(
                "Track loaded on deck {}: track_bpm={:.2}, global_bpm={:.2}, ratio={:.4}",
                deck, track_bpm, self.global_bpm, ratio
            );
            d.set_stretch_ratio(ratio);
        }

        // Update time stretcher for this deck's BPM
        self.stretchers[deck].set_bpm(track_bpm, self.global_bpm);

        // Clear latency compensation buffers for this deck
        self.latency_compensator.clear_deck(deck);

        // Update stem latencies for this deck
        self.update_deck_latencies(deck);
    }

    /// Unload a track from a deck
    pub fn unload_track(&mut self, deck: usize) {
        if let Some(d) = self.decks.get_mut(deck) {
            d.unload_track();
            self.stretchers[deck].reset();
            self.latency_compensator.clear_deck(deck);
        }
    }

    /// Set the global BPM
    pub fn set_global_bpm(&mut self, bpm: f64) {
        self.global_bpm = bpm.clamp(MIN_BPM, MAX_BPM);

        // Update all deck stretch ratios and stretchers
        for (i, deck) in self.decks.iter_mut().enumerate() {
            if let Some(track) = deck.track() {
                let track_bpm = track.bpm();
                let ratio = self.global_bpm / track_bpm;
                log::debug!(
                    "BPM changed - deck {}: track_bpm={:.2}, global_bpm={:.2}, ratio={:.4}",
                    i, track_bpm, self.global_bpm, ratio
                );
                deck.set_stretch_ratio(ratio);
                self.stretchers[i].set_bpm(track_bpm, self.global_bpm);
            }
        }
    }

    /// Get the global BPM
    pub fn global_bpm(&self) -> f64 {
        self.global_bpm
    }

    /// Adjust global BPM by delta
    pub fn adjust_bpm(&mut self, delta: f64) {
        self.set_global_bpm(self.global_bpm + delta);
    }

    /// Get the current global latency in samples
    pub fn global_latency(&self) -> u32 {
        self.latency_compensator.global_latency()
    }

    /// Set whether phase sync is enabled
    ///
    /// When enabled, starting playback or hitting hot cues will automatically
    /// align to the master deck's beat phase. When disabled, decks play
    /// from their exact cued position without adjustment.
    pub fn set_phase_sync_enabled(&mut self, enabled: bool) {
        self.phase_sync_enabled = enabled;
    }

    /// Check if phase sync is enabled
    pub fn phase_sync_enabled(&self) -> bool {
        self.phase_sync_enabled
    }

    /// Update the is_master atomic on all decks
    ///
    /// Call this after any change to deck_play_start (play/pause/stop).
    /// The master deck's atomic will be set to true, all others to false.
    fn sync_master_atomics(&self) {
        let master_id = self.master_deck_id();
        for (i, deck) in self.decks.iter().enumerate() {
            deck.atomics()
                .is_master
                .store(Some(i) == master_id, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Update latency compensation for a deck's stems
    ///
    /// Calculates total latency for each stem including:
    /// - Effect chain latency (per-stem, varies by effects)
    /// - Timestretch latency (per-deck, same for all stems)
    fn update_deck_latencies(&mut self, deck: usize) {
        if let Some(d) = self.decks.get(deck) {
            // Get timestretch latency for this deck (applies to all stems)
            let stretch_latency = self.stretchers[deck].total_latency() as u32;

            for (stem_idx, stem) in Stem::ALL.iter().enumerate() {
                let effect_latency = d.stem(*stem).chain.total_latency();
                // Total latency = effect chain + timestretch
                let total_latency = effect_latency + stretch_latency;
                self.latency_compensator.set_stem_latency(deck, stem_idx, total_latency);
            }
        }
    }

    /// Notify the engine that a stem's effect chain has changed
    ///
    /// Call this after adding/removing/bypassing effects to update latency compensation.
    pub fn on_effect_chain_changed(&mut self, deck: usize, _stem: Stem) {
        self.update_deck_latencies(deck);
    }

    /// Process all pending commands from the lock-free queue
    ///
    /// Call this at the start of each audio frame, before `process()`.
    /// Commands are processed in order, ensuring deterministic behavior.
    ///
    /// ## Real-Time Safety
    ///
    /// - Uses `rtrb::Consumer::pop()` which is wait-free (O(1), no syscalls)
    /// - Each command dispatch is a direct method call (no allocations)
    /// - Empty queue returns immediately (no spinning or blocking)
    pub fn process_commands(&mut self, consumer: &mut rtrb::Consumer<EngineCommand>) {
        while let Ok(cmd) = consumer.pop() {
            match cmd {
                // Track Management
                EngineCommand::LoadTrack { deck, track } => {
                    // NOTE: This log call is NOT RT-safe, but LoadTrack is a rare event
                    // and we need timing data to diagnose dropouts. For production,
                    // consider a lock-free log queue.
                    log::debug!("[PERF] Audio: Processing LoadTrack for deck {}", deck);
                    let start = std::time::Instant::now();
                    self.load_track_fast(deck, *track);
                    log::debug!("[PERF] Audio: load_track_fast() took {:?}", start.elapsed());
                }
                EngineCommand::UnloadTrack { deck } => {
                    self.unload_track(deck);
                }

                // Playback Control (with inter-deck phase sync)
                EngineCommand::Play { deck } => {
                    if deck < NUM_DECKS {
                        // Skip sync if coming from preview mode (Cueing) - already synced there
                        let was_cueing = self.decks[deck].state() == PlayState::Cueing;

                        // Only sync if NOT coming from preview mode
                        if !was_cueing {
                            let master_id = self.master_deck_id();
                            let should_sync = master_id.is_some() && master_id != Some(deck);
                            if should_sync {
                                let current_pos = self.decks[deck].position() as usize;
                                let synced_pos = self.phase_locked_position(deck, current_pos);
                                self.decks[deck].seek(synced_pos);
                            }
                        }

                        self.decks[deck].play();

                        // Track when this deck started playing
                        if self.deck_play_start[deck].is_none() {
                            self.deck_play_start[deck] = Some(self.frame_counter);
                        }

                        // Update master atomics for UI
                        self.sync_master_atomics();
                    }
                }
                EngineCommand::Pause { deck } => {
                    if deck < NUM_DECKS {
                        self.decks[deck].pause();
                        // Clear play tracking (deck is no longer playing)
                        self.deck_play_start[deck] = None;
                        // Update master atomics for UI
                        self.sync_master_atomics();
                    }
                }
                EngineCommand::TogglePlay { deck } => {
                    if deck < NUM_DECKS {
                        let state = self.decks[deck].state();
                        let was_playing = state == PlayState::Playing;
                        let was_cueing = state == PlayState::Cueing;

                        // Only sync if transitioning from Stopped (not Cueing - already synced there)
                        if !was_playing && !was_cueing {
                            let master_id = self.master_deck_id();
                            if master_id.is_some() && master_id != Some(deck) {
                                let current_pos = self.decks[deck].position() as usize;
                                let synced_pos = self.phase_locked_position(deck, current_pos);
                                self.decks[deck].seek(synced_pos);
                            }
                        }

                        self.decks[deck].toggle_play();

                        // Update play tracking
                        if was_playing {
                            self.deck_play_start[deck] = None;
                        } else if self.deck_play_start[deck].is_none() {
                            self.deck_play_start[deck] = Some(self.frame_counter);
                        }

                        // Update master atomics for UI
                        self.sync_master_atomics();
                    }
                }
                EngineCommand::Seek { deck, position } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.seek(position);
                    }
                }

                // CDJ-Style Cueing (with inter-deck phase sync on preview)
                EngineCommand::CuePress { deck } => {
                    if deck < NUM_DECKS {
                        let was_stopped = self.decks[deck].state() == PlayState::Stopped;

                        self.decks[deck].cue_press();

                        // If we just entered preview mode from stopped, sync the position
                        // so the preview sounds correctly aligned with master
                        if was_stopped && self.decks[deck].state() == PlayState::Cueing {
                            if let Some(master_id) = self.master_deck_id() {
                                if master_id != deck {
                                    let current_pos = self.decks[deck].position() as usize;
                                    let synced_pos = self.phase_locked_position(deck, current_pos);
                                    if synced_pos != current_pos {
                                        self.decks[deck].seek(synced_pos);
                                    }
                                }
                            }
                        }
                    }
                }
                EngineCommand::CueRelease { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.cue_release();
                    }
                }
                EngineCommand::SetCuePoint { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_cue_point();
                    }
                }

                // Hot Cues (with inter-deck phase sync)
                EngineCommand::HotCuePress { deck, slot } => {
                    if deck < NUM_DECKS {
                        // Save state before hot cue (pressing when stopped enters preview mode)
                        let was_stopped = self.decks[deck].state() == PlayState::Stopped;

                        // Execute the hot cue press (jump/preview/set)
                        self.decks[deck].hot_cue_press(slot);

                        // Apply phase sync correction:
                        // - If was playing: helper handles it (deck is still Playing)
                        // - If was stopped (now Cueing): sync preview position so it sounds correct
                        if let Some(synced_pos) = self.apply_post_jump_phase_sync(deck) {
                            self.decks[deck].seek(synced_pos);
                        } else if was_stopped {
                            // Helper returns None for Cueing state, but we still need to sync preview
                            if let Some(master_id) = self.master_deck_id() {
                                if master_id != deck {
                                    let current_pos = self.decks[deck].position() as usize;
                                    let synced_pos = self.phase_locked_position(deck, current_pos);
                                    if synced_pos != current_pos {
                                        self.decks[deck].seek(synced_pos);
                                    }
                                }
                            }
                        }
                    }
                }
                EngineCommand::HotCueRelease { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.hot_cue_release();
                    }
                }
                EngineCommand::ClearHotCue { deck, slot } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.clear_hot_cue(slot);
                    }
                }
                EngineCommand::SetShift { deck, held } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_shift(held);
                    }
                }

                // Loop Control
                EngineCommand::ToggleLoop { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.toggle_loop();
                    }
                }
                EngineCommand::LoopIn { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.loop_in();
                    }
                }
                EngineCommand::LoopOut { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.loop_out();
                    }
                }
                EngineCommand::LoopOff { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.loop_off();
                    }
                }
                EngineCommand::AdjustLoopLength { deck, direction } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.adjust_loop_length(direction);
                    }
                }
                EngineCommand::SetLoopLengthIndex { deck, index } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_loop_length_index(index);
                    }
                }
                EngineCommand::ToggleSlip { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.toggle_slip();
                    }
                }

                // Beat Jump (with inter-deck phase sync)
                EngineCommand::BeatJumpForward { deck } => {
                    if deck < NUM_DECKS {
                        // Execute beat jump (handles position + loop movement)
                        self.decks[deck].beat_jump_forward();

                        // Apply phase sync correction if playing
                        if let Some(synced_pos) = self.apply_post_jump_phase_sync(deck) {
                            self.decks[deck].seek(synced_pos);
                        }
                    }
                }
                EngineCommand::BeatJumpBackward { deck } => {
                    if deck < NUM_DECKS {
                        // Execute beat jump (handles position + loop movement)
                        self.decks[deck].beat_jump_backward();

                        // Apply phase sync correction if playing
                        if let Some(synced_pos) = self.apply_post_jump_phase_sync(deck) {
                            self.decks[deck].seek(synced_pos);
                        }
                    }
                }
                // Stem Control
                EngineCommand::ToggleStemMute { deck, stem } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.toggle_stem_mute(stem);
                    }
                }
                EngineCommand::ToggleStemSolo { deck, stem } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.toggle_stem_solo(stem);
                    }
                }
                EngineCommand::SetStemMute { deck, stem, muted } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_stem_mute(stem, muted);
                    }
                }
                EngineCommand::SetStemSolo { deck, stem, soloed } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_stem_solo(stem, soloed);
                    }
                }

                // Key Matching
                EngineCommand::SetKeyMatchEnabled { deck, enabled } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_key_match_enabled(enabled);
                    }
                }
                EngineCommand::SetTrackKey { deck, key } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        let parsed_key = key.as_ref().and_then(|k| crate::music::MusicalKey::parse(k));
                        d.set_track_key(parsed_key);
                    }
                }

                // Slicer Control
                EngineCommand::SetSlicerEnabled { deck, stem, enabled } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_slicer_enabled(stem, enabled);
                    }
                }
                EngineCommand::SlicerButtonAction { deck, stem, button_idx, shift_held } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        if shift_held {
                            // Shift+button: slice assignment on single stem (existing behavior)
                            let current_pos = d.position() as usize;
                            d.slicer_handle_shift_button(stem, button_idx, current_pos);
                        } else {
                            // Normal button: load preset for ALL stems (per-stem patterns)
                            if button_idx < 8 {
                                let preset = &self.slicer_presets[button_idx];
                                for (stem_idx, stem) in Stem::ALL.iter().enumerate() {
                                    if let Some(seq) = &preset.stems[stem_idx] {
                                        // Load sequence and enable slicer for this stem
                                        d.slicer_load_sequence(stem_idx, seq.clone());
                                        d.set_slicer_enabled(*stem, true);
                                    } else {
                                        // No pattern defined - disable slicer for this stem
                                        d.set_slicer_enabled(*stem, false);
                                    }
                                }
                                log::debug!("slicer: loaded preset {} for all stems on deck {}", button_idx, deck);
                            }
                        }
                    }
                }
                EngineCommand::SlicerResetQueue { deck, stem } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.slicer_reset_queue(stem);
                    }
                }
                EngineCommand::SetSlicerBufferBars { deck, stem, bars } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_slicer_buffer_bars(stem, bars);
                    }
                }
                EngineCommand::SetSlicerPresets { presets } => {
                    self.slicer_presets = *presets;
                }
                EngineCommand::SlicerLoadSequence { deck, stem, sequence } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.slicer_load_sequence(stem as usize, *sequence);
                    }
                }

                // Linked Stems
                EngineCommand::LinkStem { deck, stem, linked_stem } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        let stem_idx = stem as usize;
                        let info = linked_stem.into_info();
                        d.set_linked_stem(stem_idx, info);
                        log::info!(
                            "Linked stem {} on deck {} from external track",
                            stem_idx, deck
                        );
                    }
                }
                EngineCommand::ToggleLinkedStem { deck, stem } => {
                    let stem_idx = stem as usize;
                    log::info!(
                        "[STEM_TOGGLE] Engine received ToggleLinkedStem: deck={}, stem={}",
                        deck, stem_idx
                    );
                    if let Some(d) = self.decks.get_mut(deck) {
                        let has_linked = d.stem_link(stem_idx).map_or(false, |l| l.has_linked());
                        log::info!(
                            "[STEM_TOGGLE] Before toggle: has_linked={}",
                            has_linked
                        );
                        let is_linked = d.toggle_linked_stem(stem_idx);
                        log::info!(
                            "[STEM_TOGGLE] Toggled linked stem {} on deck {}: now {}",
                            stem_idx, deck,
                            if is_linked { "LINKED" } else { "ORIGINAL" }
                        );
                    } else {
                        log::warn!("[STEM_TOGGLE] Deck {} not found!", deck);
                    }
                }

                // Mixer Control
                EngineCommand::SetVolume { deck, volume } => {
                    if let Some(ch) = self.mixer.channel_mut(deck) {
                        ch.volume = volume;
                    }
                }
                EngineCommand::SetCrossfader { position: _ } => {
                    // TODO: Crossfader not yet implemented in mixer
                }
                EngineCommand::SetCueListen { deck, enabled } => {
                    if let Some(ch) = self.mixer.channel_mut(deck) {
                        ch.cue_enabled = enabled;
                    }
                }
                EngineCommand::SetEqHi { deck, value } => {
                    if let Some(ch) = self.mixer.channel_mut(deck) {
                        ch.set_eq_hi(value);
                    }
                }
                EngineCommand::SetEqMid { deck, value } => {
                    if let Some(ch) = self.mixer.channel_mut(deck) {
                        ch.set_eq_mid(value);
                    }
                }
                EngineCommand::SetEqLo { deck, value } => {
                    if let Some(ch) = self.mixer.channel_mut(deck) {
                        ch.set_eq_lo(value);
                    }
                }
                EngineCommand::SetFilter { deck, value } => {
                    if let Some(ch) = self.mixer.channel_mut(deck) {
                        ch.filter = value;
                    }
                }

                // Loudness Compensation
                EngineCommand::SetLufsGain { deck, gain, host_lufs } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_lufs_gain(gain, host_lufs);
                    }
                }

                // Global
                EngineCommand::SetGlobalBpm(bpm) => {
                    self.set_global_bpm(bpm);
                }
                EngineCommand::AdjustBpm(delta) => {
                    self.adjust_bpm(delta);
                }
                EngineCommand::SetPhaseSync(enabled) => {
                    self.set_phase_sync_enabled(enabled);
                }
            }
        }
    }

    /// Process one buffer of audio
    ///
    /// Returns master and cue outputs.
    ///
    /// ## Time Stretching Flow
    ///
    /// 1. Deck reads `output_len * stretch_ratio` samples into stretch_input
    /// 2. Time stretcher compresses/expands stretch_input to exactly output_len samples
    /// 3. Result goes to mixer
    ///
    /// This enables each deck to play at its native BPM while outputting audio
    /// synchronized to the global target BPM.
    pub fn process(&mut self, master_out: &mut StereoBuffer, cue_out: &mut StereoBuffer) {
        // Increment frame counter for phase sync tracking
        self.frame_counter = self.frame_counter.wrapping_add(1);

        // Update key matching transposition for each deck
        // The master deck is the one that has been playing longest
        let master_id = self.master_deck_id();
        let master_key = master_id.and_then(|id| self.decks[id].track_key());

        for (i, deck) in self.decks.iter_mut().enumerate() {
            if deck.key_match_enabled() && Some(i) != master_id {
                // Slave deck with key matching enabled: transpose to match master
                if let (Some(deck_key), Some(master_key)) = (deck.track_key(), master_key) {
                    let semitones = semitones_to_match(&deck_key, &master_key);
                    deck.set_current_transpose(semitones);
                    self.stretchers[i].set_pitch_semitones(semitones as f64);
                } else {
                    // Missing key info: reset to no transpose
                    deck.set_current_transpose(0);
                    self.stretchers[i].set_pitch_semitones(0.0);
                }
            } else {
                // Master deck or key matching disabled: no transpose
                deck.set_current_transpose(0);
                self.stretchers[i].set_pitch_semitones(0.0);
            }
        }

        let output_len = master_out.len();

        // Set working length of deck output buffers (real-time safe: no allocation)
        // Capacity remains at MAX_BUFFER_SIZE, only the length field changes
        for buf in &mut self.deck_buffers {
            buf.set_len_from_capacity(output_len);
        }

        // Process each deck with per-stem latency compensation and time stretching
        for deck_idx in 0..NUM_DECKS {
            // Deck fills stretch_input with variable samples based on stretch_ratio
            // The deck reads output_len * stretch_ratio samples from the track
            self.decks[deck_idx].process(
                &mut self.stretch_input,
                output_len,
                Some(&mut self.latency_compensator),
                deck_idx,
            );

            // Time-stretch: convert variable input to fixed output_len
            // stretch_input may be larger (speedup) or smaller (slowdown) than output_len
            if self.decks[deck_idx].has_track() {
                self.stretchers[deck_idx].process(&self.stretch_input, &mut self.deck_buffers[deck_idx]);
            } else {
                // No track loaded - copy silence from stretch_input to deck_buffer
                self.deck_buffers[deck_idx].copy_from(&self.stretch_input);
            }
        }

        // Detect decks that stopped naturally (end of track) and update master tracking
        // This ensures a deck reaching end of playback is properly unset as master
        let mut master_changed = false;
        for deck_idx in 0..NUM_DECKS {
            if self.deck_play_start[deck_idx].is_some() && self.decks[deck_idx].state() == PlayState::Stopped {
                self.deck_play_start[deck_idx] = None;
                master_changed = true;
                log::debug!("deck {}: stopped naturally (end of track), clearing master tracking", deck_idx);
            }
        }
        if master_changed {
            self.sync_master_atomics();
        }

        // Mix deck outputs to master and cue
        self.mixer.process(&mut self.deck_buffers, master_out, cue_out);
    }

    /// Reset all decks and the mixer
    pub fn reset(&mut self) {
        for deck in &mut self.decks {
            if deck.has_track() {
                for stem in Stem::ALL {
                    deck.stem_mut(stem).chain.reset();
                }
            }
        }
        for stretcher in &mut self.stretchers {
            stretcher.reset();
        }
        self.latency_compensator.clear();
        self.mixer.reset();
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = AudioEngine::new();
        assert_eq!(engine.global_bpm(), DEFAULT_BPM);
        assert_eq!(engine.global_latency(), 0);
    }

    #[test]
    fn test_bpm_adjustment() {
        let mut engine = AudioEngine::new();

        engine.set_global_bpm(120.0);
        assert_eq!(engine.global_bpm(), 120.0);

        engine.adjust_bpm(5.0);
        assert_eq!(engine.global_bpm(), 125.0);

        // Test clamping
        engine.set_global_bpm(10.0);
        assert_eq!(engine.global_bpm(), MIN_BPM);

        engine.set_global_bpm(300.0);
        assert_eq!(engine.global_bpm(), MAX_BPM);
    }

    #[test]
    fn test_process_empty_engine() {
        let mut engine = AudioEngine::new();
        let mut master = StereoBuffer::silence(256);
        let mut cue = StereoBuffer::silence(256);

        // Should not panic with no tracks loaded
        engine.process(&mut master, &mut cue);

        assert_eq!(master.len(), 256);
        assert_eq!(cue.len(), 256);
    }
}
