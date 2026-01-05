//! Main audio engine - ties together decks, mixer, and time-stretching

use crate::timestretch::TimeStretcher;
use crate::types::{DeckId, Stem, StereoBuffer, NUM_DECKS};

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
}

impl AudioEngine {
    /// Create a new audio engine
    pub fn new() -> Self {
        Self {
            decks: std::array::from_fn(|i| Deck::new(DeckId::new(i))),
            global_bpm: DEFAULT_BPM,
            mixer: Mixer::new(),
            latency_compensator: LatencyCompensator::new(),
            stretchers: std::array::from_fn(|_| TimeStretcher::new()),
            deck_buffers: std::array::from_fn(|_| StereoBuffer::silence(MAX_BUFFER_SIZE)),
            stretch_input: StereoBuffer::silence(MAX_BUFFER_SIZE),
        }
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

    /// Get a reference to the mixer
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Get a mutable reference to the mixer
    pub fn mixer_mut(&mut self) -> &mut Mixer {
        &mut self.mixer
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

                // Playback Control
                EngineCommand::Play { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.play();
                    }
                }
                EngineCommand::Pause { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.pause();
                    }
                }
                EngineCommand::TogglePlay { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.toggle_play();
                    }
                }
                EngineCommand::Seek { deck, position } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.seek(position);
                    }
                }

                // CDJ-Style Cueing
                EngineCommand::CuePress { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.cue_press();
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

                // Hot Cues
                EngineCommand::HotCuePress { deck, slot } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.hot_cue_press(slot);
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

                // Beat Jump
                EngineCommand::BeatJumpForward { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.beat_jump_forward();
                    }
                }
                EngineCommand::BeatJumpBackward { deck } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.beat_jump_backward();
                    }
                }
                EngineCommand::SetBeatJumpSize { deck, beats } => {
                    if let Some(d) = self.decks.get_mut(deck) {
                        d.set_beat_jump_size(beats);
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

                // Global
                EngineCommand::SetGlobalBpm(bpm) => {
                    self.set_global_bpm(bpm);
                }
                EngineCommand::AdjustBpm(delta) => {
                    self.adjust_bpm(delta);
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
