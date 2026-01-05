//! Main audio engine - ties together decks, mixer, and time-stretching

use crate::audio_file::LoadedTrack;
use crate::timestretch::TimeStretcher;
use crate::types::{DeckId, Stem, StereoBuffer, NUM_DECKS};

use super::{Deck, DeckAtomics, LatencyCompensator, Mixer};

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

    /// Load a track into a deck
    pub fn load_track(&mut self, deck: usize, track: LoadedTrack) {
        if let Some(d) = self.decks.get_mut(deck) {
            // Update time stretcher for this deck's BPM
            let track_bpm = track.bpm();
            d.load_track(track);
            self.stretchers[deck].set_bpm(track_bpm, self.global_bpm);

            // Clear latency compensation buffers for this deck
            self.latency_compensator.clear_deck(deck);

            // Update stem latencies for this deck
            self.update_deck_latencies(deck);
        }
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

        // Update all stretchers
        for (i, deck) in self.decks.iter().enumerate() {
            if let Some(track) = deck.track() {
                self.stretchers[i].set_bpm(track.bpm(), self.global_bpm);
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

    /// Process one buffer of audio
    ///
    /// Returns master and cue outputs.
    pub fn process(&mut self, master_out: &mut StereoBuffer, cue_out: &mut StereoBuffer) {
        let buffer_len = master_out.len();

        // Set working length of pre-allocated buffers (real-time safe: no allocation)
        // Capacity remains at MAX_BUFFER_SIZE, only the length field changes
        for buf in &mut self.deck_buffers {
            buf.set_len_from_capacity(buffer_len);
        }
        self.stretch_input.set_len_from_capacity(buffer_len);

        // Process each deck with per-stem latency compensation
        // We need to split the mutable borrow to access both decks and latency_compensator
        for deck_idx in 0..NUM_DECKS {
            // Get deck output with per-stem latency compensation
            // The compensator ensures all stems are sample-aligned before summing
            self.decks[deck_idx].process(
                &mut self.deck_buffers[deck_idx],
                Some(&mut self.latency_compensator),
                deck_idx,
            );

            // Time-stretch to global BPM
            if self.decks[deck_idx].has_track() {
                self.stretch_input.copy_from(&self.deck_buffers[deck_idx]);
                self.stretchers[deck_idx].process(&self.stretch_input, &mut self.deck_buffers[deck_idx]);
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
