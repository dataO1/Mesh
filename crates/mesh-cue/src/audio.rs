//! Audio backend for mesh-cue
//!
//! Uses the shared AudioEngine from mesh-core with a single deck (deck 0)
//! for preview playback. This gives mesh-cue full access to:
//! - Slicer with presets and per-stem patterns
//! - Hot cue preview
//! - Loop preview
//! - Effect chains
//! - Time stretching
//!
//! # Lock-Free Architecture
//!
//! Same architecture as mesh-player:
//! - Commands sent via lock-free ringbuffer (UI → Audio)
//! - State read via atomics (Audio → UI)
//! - Audio thread owns the engine exclusively

use std::sync::Arc;

use mesh_core::audio::{self, AudioConfig, AudioResult};
use mesh_core::audio_file::LoadedTrack;
use mesh_core::db::DatabaseService;
use mesh_core::engine::{DeckAtomics, EngineCommand, LinkedStemAtomics, PreparedTrack, SlicerAtomics};
use mesh_core::loader::LinkedStemResultReceiver;

// Re-export for convenience
pub use mesh_core::audio::{
    get_available_stereo_pairs, reconnect_ports, AudioError, CommandSender, OutputDevice,
    StereoPair,
};
pub use mesh_core::engine::{SlicerPreset, StepSequence};

/// The deck index used for preview (always deck 0)
pub const PREVIEW_DECK: usize = 0;

/// Handle to the active audio system
pub struct AudioHandle {
    _handle: mesh_core::audio::AudioHandle,
}

/// Audio state for UI interaction
///
/// Provides high-level API for preview playback using deck 0.
/// All operations are lock-free via command queue and atomics.
pub struct AudioState {
    /// Command sender (None if audio unavailable)
    command_sender: Option<CommandSender>,
    /// Deck atomics for reading playback state
    deck_atomics: Arc<DeckAtomics>,
    /// Slicer atomics for reading slicer state (one per stem: VOC, DRM, BAS, OTH)
    slicer_atomics: [Arc<SlicerAtomics>; 4],
    /// Linked stem atomics
    linked_stem_atomics: Arc<LinkedStemAtomics>,
    /// Sample rate
    sample_rate: u32,
    /// Linked stem result receiver (engine owns the loader)
    linked_stem_receiver: Option<LinkedStemResultReceiver>,
}

impl AudioState {
    /// Create audio state from startup results
    fn new(
        command_sender: CommandSender,
        deck_atomics: Arc<DeckAtomics>,
        slicer_atomics: [Arc<SlicerAtomics>; 4],
        linked_stem_atomics: Arc<LinkedStemAtomics>,
        linked_stem_receiver: LinkedStemResultReceiver,
        sample_rate: u32,
    ) -> Self {
        Self {
            command_sender: Some(command_sender),
            deck_atomics,
            slicer_atomics,
            linked_stem_atomics,
            sample_rate,
            linked_stem_receiver: Some(linked_stem_receiver),
        }
    }

    /// Create a disconnected audio state (when audio is unavailable)
    pub fn disconnected() -> Self {
        Self {
            command_sender: None,
            deck_atomics: Arc::new(DeckAtomics::new()),
            slicer_atomics: [
                Arc::new(SlicerAtomics::new()),
                Arc::new(SlicerAtomics::new()),
                Arc::new(SlicerAtomics::new()),
                Arc::new(SlicerAtomics::new()),
            ],
            linked_stem_atomics: Arc::new(LinkedStemAtomics::new()),
            sample_rate: 44100,
            linked_stem_receiver: None,
        }
    }

    /// Send a command to the audio engine
    fn send(&mut self, cmd: EngineCommand) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(cmd);
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Playback state (read via atomics)
    // ─────────────────────────────────────────────────────────────────────────

    /// Get current playback position in samples
    pub fn position(&self) -> u64 {
        self.deck_atomics.position()
    }

    /// Check if playing
    pub fn is_playing(&self) -> bool {
        self.deck_atomics.is_playing()
    }

    /// Get sample rate
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get deck atomics for direct access
    pub fn deck_atomics(&self) -> &Arc<DeckAtomics> {
        &self.deck_atomics
    }

    /// Get slicer atomics for all 4 stems (VOC, DRM, BAS, OTH)
    ///
    /// Used for waveform slicer overlay visualization.
    pub fn slicer_atomics(&self) -> &[Arc<SlicerAtomics>; 4] {
        &self.slicer_atomics
    }

    /// Get linked stem atomics
    pub fn linked_stem_atomics(&self) -> &Arc<LinkedStemAtomics> {
        &self.linked_stem_atomics
    }

    /// Get linked stem result receiver (for subscription)
    pub fn linked_stem_receiver(&self) -> Option<LinkedStemResultReceiver> {
        self.linked_stem_receiver.clone()
    }

    /// Get LUFS gain from atomics (single source of truth from engine)
    pub fn lufs_gain(&self) -> f32 {
        self.deck_atomics.lufs_gain()
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Configuration commands (engine calculates internally)
    // ─────────────────────────────────────────────────────────────────────────

    /// Set loudness configuration (engine calculates LUFS gain for loaded tracks)
    pub fn set_loudness_config(&mut self, config: mesh_core::config::LoudnessConfig) {
        self.send(EngineCommand::SetLoudnessConfig(config));
    }

    /// Request linked stem load (engine owns the loader)
    pub fn load_linked_stem(
        &mut self,
        stem_idx: usize,
        path: std::path::PathBuf,
        host_bpm: f64,
        host_drop_marker: u64,
        host_duration: u64,
    ) {
        self.send(EngineCommand::LoadLinkedStem(Box::new(
            mesh_core::engine::LoadLinkedStemRequest {
                deck: PREVIEW_DECK,
                stem_idx,
                path,
                host_bpm,
                host_drop_marker,
                host_duration,
            },
        )));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Playback control
    // ─────────────────────────────────────────────────────────────────────────

    /// Start playback
    pub fn play(&mut self) {
        self.send(EngineCommand::Play { deck: PREVIEW_DECK });
    }

    /// Pause playback
    pub fn pause(&mut self) {
        self.send(EngineCommand::Pause { deck: PREVIEW_DECK });
    }

    /// Toggle play/pause
    pub fn toggle(&mut self) {
        self.send(EngineCommand::TogglePlay { deck: PREVIEW_DECK });
    }

    /// Seek to position in samples
    pub fn seek(&mut self, position: u64) {
        self.send(EngineCommand::Seek {
            deck: PREVIEW_DECK,
            position: position as usize,
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Scratch mode (vinyl-style scrubbing)
    // ─────────────────────────────────────────────────────────────────────────

    /// Enter scratch mode - like touching a vinyl record
    ///
    /// Audio plays at the current position but the playhead doesn't advance.
    /// Position is controlled via scratch_move() calls.
    pub fn scratch_start(&mut self) {
        self.send(EngineCommand::ScratchStart { deck: PREVIEW_DECK });
    }

    /// Update scratch position - like moving a vinyl record
    ///
    /// Moves the playhead to the new position. Audio output will reflect
    /// this position, creating a vinyl scratch sound effect.
    pub fn scratch_move(&mut self, position: u64) {
        self.send(EngineCommand::ScratchMove {
            deck: PREVIEW_DECK,
            position: position as usize,
        });
    }

    /// Exit scratch mode - like releasing a vinyl record
    ///
    /// Restores the play state from before scratch started.
    pub fn scratch_end(&mut self) {
        self.send(EngineCommand::ScratchEnd { deck: PREVIEW_DECK });
    }

    /// Set scratch interpolation method
    ///
    /// Linear = fast, acceptable quality; Cubic = better quality, more CPU
    pub fn set_scratch_interpolation(&mut self, method: mesh_core::engine::InterpolationMethod) {
        self.send(EngineCommand::SetScratchInterpolation {
            deck: PREVIEW_DECK,
            method,
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Track loading
    // ─────────────────────────────────────────────────────────────────────────

    /// Load a track for preview
    ///
    /// Creates a PreparedTrack from the LoadedTrack (pre-computes hot cues).
    pub fn load_track(&mut self, track: LoadedTrack) {
        let prepared = PreparedTrack::prepare(track);
        self.send(EngineCommand::LoadTrack {
            deck: PREVIEW_DECK,
            track: Box::new(prepared),
        });
    }

    /// Unload current track
    pub fn unload_track(&mut self) {
        self.send(EngineCommand::UnloadTrack { deck: PREVIEW_DECK });
    }

    /// Set global BPM (affects time-stretching ratio for all decks)
    ///
    /// For mesh-cue, we set this to the track's analyzed BPM so playback
    /// is at original speed (no time-stretching).
    pub fn set_global_bpm(&mut self, bpm: f64) {
        self.send(EngineCommand::SetGlobalBpm(bpm));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // CDJ-Style Cueing
    // ─────────────────────────────────────────────────────────────────────────

    /// CDJ-style cue button press
    pub fn cue_press(&mut self) {
        self.send(EngineCommand::CuePress { deck: PREVIEW_DECK });
    }

    /// CDJ-style cue button release
    pub fn cue_release(&mut self) {
        self.send(EngineCommand::CueRelease { deck: PREVIEW_DECK });
    }

    /// Set cue point at current position
    pub fn set_cue_point(&mut self) {
        self.send(EngineCommand::SetCuePoint { deck: PREVIEW_DECK });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Hot Cues
    // ─────────────────────────────────────────────────────────────────────────

    /// Hot cue button press
    pub fn hot_cue_press(&mut self, slot: usize) {
        self.send(EngineCommand::HotCuePress {
            deck: PREVIEW_DECK,
            slot,
        });
    }

    /// Hot cue button release
    pub fn hot_cue_release(&mut self) {
        self.send(EngineCommand::HotCueRelease { deck: PREVIEW_DECK });
    }

    /// Clear a hot cue slot
    pub fn clear_hot_cue(&mut self, slot: usize) {
        self.send(EngineCommand::ClearHotCue {
            deck: PREVIEW_DECK,
            slot,
        });
    }

    /// Set a hot cue at a specific position (for editor metadata sync)
    ///
    /// This propagates cue point changes to the deck so hot cue playback
    /// uses the updated positions immediately without requiring a track reload.
    pub fn set_hot_cue(&mut self, slot: usize, position: usize) {
        self.send(EngineCommand::SetHotCue {
            deck: PREVIEW_DECK,
            slot,
            position,
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Loop Control
    // ─────────────────────────────────────────────────────────────────────────

    /// Toggle loop on/off
    pub fn toggle_loop(&mut self) {
        self.send(EngineCommand::ToggleLoop { deck: PREVIEW_DECK });
    }

    /// Set loop in point
    pub fn loop_in(&mut self) {
        self.send(EngineCommand::LoopIn { deck: PREVIEW_DECK });
    }

    /// Set loop out point and activate
    pub fn loop_out(&mut self) {
        self.send(EngineCommand::LoopOut { deck: PREVIEW_DECK });
    }

    /// Turn off loop
    pub fn loop_off(&mut self) {
        self.send(EngineCommand::LoopOff { deck: PREVIEW_DECK });
    }

    /// Adjust loop length
    pub fn adjust_loop_length(&mut self, direction: i32) {
        self.send(EngineCommand::AdjustLoopLength {
            deck: PREVIEW_DECK,
            direction,
        });
    }

    /// Set loop length index
    pub fn set_loop_length_index(&mut self, index: usize) {
        self.send(EngineCommand::SetLoopLengthIndex {
            deck: PREVIEW_DECK,
            index,
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Beat Jump
    // ─────────────────────────────────────────────────────────────────────────

    /// Jump forward by loop length beats
    pub fn beat_jump_forward(&mut self) {
        self.send(EngineCommand::BeatJumpForward { deck: PREVIEW_DECK });
    }

    /// Jump backward by loop length beats
    pub fn beat_jump_backward(&mut self) {
        self.send(EngineCommand::BeatJumpBackward { deck: PREVIEW_DECK });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Slicer control
    // ─────────────────────────────────────────────────────────────────────────

    /// Enable/disable slicer for a stem
    pub fn set_slicer_enabled(&mut self, stem: mesh_core::types::Stem, enabled: bool) {
        self.send(EngineCommand::SetSlicerEnabled {
            deck: PREVIEW_DECK,
            stem,
            enabled,
        });
    }

    /// Set slicer buffer bars (1, 4, 8, or 16)
    pub fn set_slicer_buffer_bars(&mut self, stem: mesh_core::types::Stem, bars: u32) {
        self.send(EngineCommand::SetSlicerBufferBars {
            deck: PREVIEW_DECK,
            stem,
            bars,
        });
    }

    /// Load slicer presets (8 presets with per-stem patterns)
    pub fn set_slicer_presets(&mut self, presets: [mesh_core::engine::SlicerPreset; 8]) {
        self.send(EngineCommand::SetSlicerPresets {
            presets: Box::new(presets),
        });
    }

    /// Trigger slicer button action (for manual slice triggering)
    pub fn slicer_button_action(
        &mut self,
        stem: mesh_core::types::Stem,
        button_idx: u8,
        shift_held: bool,
    ) {
        self.send(EngineCommand::SlicerButtonAction {
            deck: PREVIEW_DECK,
            stem,
            button_idx: button_idx as usize,
            shift_held,
        });
    }

    /// Reset slicer queue to default pattern
    pub fn slicer_reset_queue(&mut self, stem: mesh_core::types::Stem) {
        self.send(EngineCommand::SlicerResetQueue {
            deck: PREVIEW_DECK,
            stem,
        });
    }

    /// Load a specific sequence into a stem's slicer
    pub fn slicer_load_sequence(
        &mut self,
        stem: mesh_core::types::Stem,
        sequence: mesh_core::engine::StepSequence,
    ) {
        self.send(EngineCommand::SlicerLoadSequence {
            deck: PREVIEW_DECK,
            stem,
            sequence: Box::new(sequence),
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Linked Stem Control
    // ─────────────────────────────────────────────────────────────────────────

    /// Toggle between original and linked stem for playback
    ///
    /// Only has effect if a linked stem exists for this stem slot.
    pub fn toggle_linked_stem(&mut self, stem: mesh_core::types::Stem) {
        self.send(EngineCommand::ToggleLinkedStem {
            deck: PREVIEW_DECK,
            stem,
        });
    }

    /// Link a stem from another track
    ///
    /// The linked stem should be pre-stretched to match the host track's BPM.
    /// `host_lufs` is passed explicitly to avoid race conditions when the linked
    /// stem loads asynchronously after the host track.
    pub fn link_stem(
        &mut self,
        stem: mesh_core::types::Stem,
        linked_data: mesh_core::engine::LinkedStemData,
        host_lufs: Option<f32>,
    ) {
        self.send(EngineCommand::LinkStem {
            deck: PREVIEW_DECK,
            stem,
            linked_stem: Box::new(linked_data),
            host_lufs,
        });
    }

    /// Update beat grid on the preview deck (for live beatgrid nudging)
    ///
    /// This propagates beatgrid changes to the engine so snapping operations
    /// use the updated grid immediately without requiring a track reload.
    pub fn set_beat_grid(&mut self, beats: Vec<u64>) {
        self.send(EngineCommand::SetBeatGrid {
            deck: PREVIEW_DECK,
            beats,
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Volume control (for preview)
    // ─────────────────────────────────────────────────────────────────────────

    /// Set deck volume (0.0 - 1.0)
    pub fn set_volume(&mut self, volume: f32) {
        self.send(EngineCommand::SetVolume {
            deck: PREVIEW_DECK,
            volume,
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Multiband Effects Control (for effects editor preview)
    // ─────────────────────────────────────────────────────────────────────────

    /// Set a crossover frequency for the preview stem's multiband container
    pub fn set_multiband_crossover(&mut self, stem: mesh_core::types::Stem, crossover_index: usize, freq: f32) {
        self.send(EngineCommand::SetMultibandCrossover {
            deck: PREVIEW_DECK,
            stem,
            crossover_index,
            freq,
        });
    }

    /// Add a band to the preview stem's multiband container
    pub fn add_multiband_band(&mut self, stem: mesh_core::types::Stem) {
        self.send(EngineCommand::AddMultibandBand {
            deck: PREVIEW_DECK,
            stem,
        });
    }

    /// Remove a band from the preview stem's multiband container
    pub fn remove_multiband_band(&mut self, stem: mesh_core::types::Stem, band_index: usize) {
        self.send(EngineCommand::RemoveMultibandBand {
            deck: PREVIEW_DECK,
            stem,
            band_index,
        });
    }

    /// Set mute state for a band
    pub fn set_multiband_band_mute(&mut self, stem: mesh_core::types::Stem, band_index: usize, muted: bool) {
        self.send(EngineCommand::SetMultibandBandMute {
            deck: PREVIEW_DECK,
            stem,
            band_index,
            muted,
        });
    }

    /// Set solo state for a band
    pub fn set_multiband_band_solo(&mut self, stem: mesh_core::types::Stem, band_index: usize, soloed: bool) {
        self.send(EngineCommand::SetMultibandBandSolo {
            deck: PREVIEW_DECK,
            stem,
            band_index,
            soloed,
        });
    }

    /// Set gain for a band (linear, 0.0-2.0)
    pub fn set_multiband_band_gain(&mut self, stem: mesh_core::types::Stem, band_index: usize, gain: f32) {
        self.send(EngineCommand::SetMultibandBandGain {
            deck: PREVIEW_DECK,
            stem,
            band_index,
            gain,
        });
    }

    /// Add an effect to a band's chain
    pub fn add_multiband_band_effect(&mut self, stem: mesh_core::types::Stem, band_index: usize, effect: Box<dyn mesh_core::effect::Effect>) {
        self.send(EngineCommand::AddMultibandBandEffect {
            deck: PREVIEW_DECK,
            stem,
            band_index,
            effect,
        });
    }

    /// Remove an effect from a band's chain
    pub fn remove_multiband_band_effect(&mut self, stem: mesh_core::types::Stem, band_index: usize, effect_index: usize) {
        self.send(EngineCommand::RemoveMultibandBandEffect {
            deck: PREVIEW_DECK,
            stem,
            band_index,
            effect_index,
        });
    }

    /// Set bypass state for an effect within a band
    pub fn set_multiband_effect_bypass(&mut self, stem: mesh_core::types::Stem, band_index: usize, effect_index: usize, bypass: bool) {
        self.send(EngineCommand::SetMultibandEffectBypass {
            deck: PREVIEW_DECK,
            stem,
            band_index,
            effect_index,
            bypass,
        });
    }

    /// Set a parameter value on an effect within a band
    pub fn set_multiband_effect_param(&mut self, stem: mesh_core::types::Stem, band_index: usize, effect_index: usize, param_index: usize, value: f32) {
        self.send(EngineCommand::SetMultibandEffectParam {
            deck: PREVIEW_DECK,
            stem,
            band_index,
            effect_index,
            param_index,
            value,
        });
    }

    /// Set a macro value for the stem's multiband container (0-7)
    pub fn set_multiband_macro(&mut self, stem: mesh_core::types::Stem, macro_index: usize, value: f32) {
        self.send(EngineCommand::SetMultibandMacro {
            deck: PREVIEW_DECK,
            stem,
            macro_index,
            value,
        });
    }

    /// Add an effect to the pre-fx chain (before multiband split)
    pub fn add_multiband_pre_fx(&mut self, stem: mesh_core::types::Stem, effect: Box<dyn mesh_core::effect::Effect>) {
        self.send(EngineCommand::AddMultibandPreFx {
            deck: PREVIEW_DECK,
            stem,
            effect,
        });
    }

    /// Remove an effect from the pre-fx chain
    pub fn remove_multiband_pre_fx(&mut self, stem: mesh_core::types::Stem, effect_index: usize) {
        self.send(EngineCommand::RemoveMultibandPreFx {
            deck: PREVIEW_DECK,
            stem,
            effect_index,
        });
    }

    /// Set bypass state for a pre-fx effect
    pub fn set_multiband_pre_fx_bypass(&mut self, stem: mesh_core::types::Stem, effect_index: usize, bypass: bool) {
        self.send(EngineCommand::SetMultibandPreFxBypass {
            deck: PREVIEW_DECK,
            stem,
            effect_index,
            bypass,
        });
    }

    /// Set a parameter on a pre-fx effect
    pub fn set_multiband_pre_fx_param(&mut self, stem: mesh_core::types::Stem, effect_index: usize, param_index: usize, value: f32) {
        self.send(EngineCommand::SetMultibandPreFxParam {
            deck: PREVIEW_DECK,
            stem,
            effect_index,
            param_index,
            value,
        });
    }

    /// Add an effect to the post-fx chain (after band summation)
    pub fn add_multiband_post_fx(&mut self, stem: mesh_core::types::Stem, effect: Box<dyn mesh_core::effect::Effect>) {
        self.send(EngineCommand::AddMultibandPostFx {
            deck: PREVIEW_DECK,
            stem,
            effect,
        });
    }

    /// Remove an effect from the post-fx chain
    pub fn remove_multiband_post_fx(&mut self, stem: mesh_core::types::Stem, effect_index: usize) {
        self.send(EngineCommand::RemoveMultibandPostFx {
            deck: PREVIEW_DECK,
            stem,
            effect_index,
        });
    }

    /// Set bypass state for a post-fx effect
    pub fn set_multiband_post_fx_bypass(&mut self, stem: mesh_core::types::Stem, effect_index: usize, bypass: bool) {
        self.send(EngineCommand::SetMultibandPostFxBypass {
            deck: PREVIEW_DECK,
            stem,
            effect_index,
            bypass,
        });
    }

    /// Set a parameter on a post-fx effect
    pub fn set_multiband_post_fx_param(&mut self, stem: mesh_core::types::Stem, effect_index: usize, param_index: usize, value: f32) {
        self.send(EngineCommand::SetMultibandPostFxParam {
            deck: PREVIEW_DECK,
            stem,
            effect_index,
            param_index,
            value,
        });
    }

    /// Reset a stem's multiband host to default state (single band, no effects)
    ///
    /// Used when disabling audio preview in the effects editor to return the stem
    /// to a clean processing state.
    pub fn reset_multiband(&mut self, stem: mesh_core::types::Stem) {
        self.send(EngineCommand::ResetMultiband {
            deck: PREVIEW_DECK,
            stem,
        });
    }
}

/// Start the audio system for mesh-cue
///
/// Returns AudioState for UI interaction and AudioHandle to keep audio alive.
///
/// # Arguments
/// * `db_service` - Database service for loading track metadata in background loaders
pub fn start_audio_system(
    db_service: Arc<DatabaseService>,
) -> AudioResult<(AudioState, AudioHandle)> {
    // Use master-only mode for mesh-cue (single stereo output for preview)
    let config = AudioConfig::master_only();

    let result = audio::start_audio_system(&config, db_service)?;

    log::info!(
        "Audio system started (sample rate: {}Hz)",
        result.sample_rate
    );

    // Get slicer atomics for preview deck (deck 0)
    let slicer_atomics = [
        result.slicer_atomics[PREVIEW_DECK].clone(),
        result.slicer_atomics[PREVIEW_DECK].clone(),
        result.slicer_atomics[PREVIEW_DECK].clone(),
        result.slicer_atomics[PREVIEW_DECK].clone(),
    ];

    // Set deck 0 volume to 1.0 (master) for preview
    let mut audio_state = AudioState::new(
        result.command_sender,
        result.deck_atomics[PREVIEW_DECK].clone(),
        slicer_atomics,
        result.linked_stem_atomics[PREVIEW_DECK].clone(),
        result.linked_stem_receiver,
        result.sample_rate,
    );
    audio_state.set_volume(1.0);

    Ok((
        audio_state,
        AudioHandle {
            _handle: result.handle,
        },
    ))
}

// Device types (OutputDevice, StereoPair, get_available_stereo_pairs) are
// re-exported from mesh_core::audio at the top of this file.
