//! Deck - Individual track player with stems and effect chains

use crate::audio_file::LoadedTrack;
use crate::effect::EffectChain;
use crate::types::{
    DeckId, PlayState, Stem, StereoBuffer, StereoSample, TransportPosition,
    NUM_STEMS, SAMPLE_RATE,
};

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
        }
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
        }
    }

    /// Pause playback
    pub fn pause(&mut self) {
        self.state = PlayState::Stopped;
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
            }
            PlayState::Stopped => {
                // Set cue to current position and start previewing
                self.cue_point = self.position;
                self.state = PlayState::Cueing;
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
        }
    }

    /// Set the cue point at the current position (snapped to nearest beat)
    pub fn set_cue_point(&mut self) {
        self.cue_point = self.snap_to_beat(self.position);
    }

    /// Get the current cue point position
    pub fn cue_point(&self) -> usize {
        self.cue_point
    }

    /// Jump to a specific sample position
    pub fn seek(&mut self, position: usize) {
        if let Some(track) = &self.track {
            self.position = position.min(track.duration_samples.saturating_sub(1));
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
                }
                PlayState::Stopped | PlayState::Cueing => {
                    // Preview mode - set main cue point to hot cue, play from cue
                    // On release, returns to the hot cue position (not the original position)
                    self.cue_point = pos;
                    self.hot_cue_preview_return = Some(pos);
                    self.position = pos;
                    self.state = PlayState::Cueing;
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
        }
    }

    /// Loop out - end loop at current position and activate
    pub fn loop_out(&mut self) {
        if self.track.is_some() {
            self.loop_state.end = self.position;
            self.loop_state.active = true;
        }
    }

    /// Turn loop off
    pub fn loop_off(&mut self) {
        self.loop_state.active = false;
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
    pub fn process(&mut self, output: &mut StereoBuffer) {
        let Some(track) = &self.track else {
            output.fill_silence();
            return;
        };

        // If stopped, output silence (prevents repeating buffer buzz)
        if self.state == PlayState::Stopped {
            output.fill_silence();
            return;
        }

        // Fill output with processed stems
        output.fill_silence();

        let any_soloed = self.any_stem_soloed();
        let buffer_len = output.len();

        // Temporary buffer for each stem
        let mut stem_buffer = StereoBuffer::silence(buffer_len);

        for stem in Stem::ALL {
            let stem_state = &mut self.stems[stem as usize];

            // Skip if muted, or if others are soloed and this isn't
            if stem_state.muted || (any_soloed && !stem_state.soloed) {
                continue;
            }

            // Copy samples from track to stem buffer
            let stem_data = track.stems.get(stem);
            for i in 0..buffer_len {
                let read_pos = self.position + i;
                if read_pos < track.duration_samples {
                    stem_buffer.as_mut_slice()[i] = stem_data[read_pos];
                } else {
                    stem_buffer.as_mut_slice()[i] = StereoSample::silence();
                }
            }

            // Process through effect chain (pass any_soloed for solo logic)
            stem_state.chain.process(&mut stem_buffer, any_soloed);

            // Add to output
            output.add_buffer(&stem_buffer);
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
            }
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
