//! JACK audio client for mesh-cue
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

use jack::{AudioOut, Client, ClientOptions, Control, Port, ProcessScope};
use mesh_core::audio_file::LoadedTrack;
use mesh_core::engine::{
    command_channel, AudioEngine, DeckAtomics, EngineCommand, LinkedStemAtomics, PreparedTrack,
    SlicerAtomics,
};
use mesh_core::loader::LinkedStemResultReceiver;
use mesh_core::types::StereoBuffer;

// Re-export for convenience
pub use mesh_core::engine::{SlicerPreset, StepSequence};

/// Maximum buffer size to pre-allocate
const MAX_BUFFER_SIZE: usize = 8192;

/// The deck index used for preview (always deck 0)
pub const PREVIEW_DECK: usize = 0;

/// Handle to the active JACK client
pub struct JackHandle {
    _async_client: jack::AsyncClient<JackNotifications, JackProcessor>,
}

/// Command sender for UI thread
///
/// Wraps the lock-free producer for sending EngineCommand to the audio thread.
pub struct CommandSender {
    producer: rtrb::Producer<EngineCommand>,
}

impl CommandSender {
    /// Send a command to the audio engine (non-blocking)
    pub fn send(&mut self, cmd: EngineCommand) -> Result<(), EngineCommand> {
        self.producer.push(cmd).map_err(|e| match e {
            rtrb::PushError::Full(value) => value,
        })
    }
}

/// JACK process handler - owns the AudioEngine exclusively
struct JackProcessor {
    /// Output ports (stereo only - no separate cue for editor)
    left: Port<AudioOut>,
    right: Port<AudioOut>,
    /// The audio engine (OWNED, not shared)
    engine: AudioEngine,
    /// Command receiver from UI
    command_rx: rtrb::Consumer<EngineCommand>,
    /// Pre-allocated output buffer
    master_buffer: StereoBuffer,
    /// Cue buffer (required by engine, but we don't output it)
    cue_buffer: StereoBuffer,
}

impl jack::ProcessHandler for JackProcessor {
    fn process(&mut self, _client: &Client, ps: &ProcessScope) -> Control {
        let n_frames = ps.n_frames() as usize;

        // Set working buffer length (RT-safe: no allocation)
        self.master_buffer.set_len_from_capacity(n_frames);
        self.cue_buffer.set_len_from_capacity(n_frames);

        // Process commands from UI (lock-free)
        self.engine.process_commands(&mut self.command_rx);

        // Process audio through the engine
        self.engine.process(&mut self.master_buffer, &mut self.cue_buffer);

        // Copy master output to JACK ports
        let out_left = self.left.as_mut_slice(ps);
        let out_right = self.right.as_mut_slice(ps);

        for i in 0..n_frames {
            let sample = self.master_buffer[i];
            out_left[i] = sample.left;
            out_right[i] = sample.right;
        }

        Control::Continue
    }
}

/// JACK notification handler
struct JackNotifications;

impl jack::NotificationHandler for JackNotifications {
    unsafe fn shutdown(&mut self, _status: jack::ClientStatus, reason: &str) {
        log::warn!("JACK server shut down: {}", reason);
    }

    fn sample_rate(&mut self, _client: &Client, srate: jack::Frames) -> Control {
        log::info!("JACK sample rate: {}", srate);
        Control::Continue
    }

    fn xrun(&mut self, _client: &Client) -> Control {
        log::warn!("JACK xrun (audio dropout)");
        Control::Continue
    }
}

/// Error type for JACK operations
#[derive(Debug)]
pub enum JackError {
    ClientCreation(String),
    PortRegistration(String),
    Activation(String),
}

impl std::fmt::Display for JackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JackError::ClientCreation(msg) => write!(f, "Failed to create JACK client: {}", msg),
            JackError::PortRegistration(msg) => write!(f, "Failed to register port: {}", msg),
            JackError::Activation(msg) => write!(f, "Failed to activate client: {}", msg),
        }
    }
}

impl std::error::Error for JackError {}

/// Audio state for UI interaction
///
/// Provides high-level API for preview playback using deck 0.
/// All operations are lock-free via command queue and atomics.
pub struct AudioState {
    /// Command sender (None if JACK unavailable)
    command_sender: Option<CommandSender>,
    /// Deck atomics for reading playback state
    deck_atomics: Arc<DeckAtomics>,
    /// Slicer atomics for reading slicer state (one per stem: VOC, DRM, BAS, OTH)
    slicer_atomics: [Arc<SlicerAtomics>; 4],
    /// Linked stem atomics
    linked_stem_atomics: Arc<LinkedStemAtomics>,
    /// JACK sample rate
    sample_rate: u32,
    /// Linked stem result receiver (engine owns the loader)
    linked_stem_receiver: Option<LinkedStemResultReceiver>,
}

impl AudioState {
    /// Create audio state from JACK startup results
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

    /// Create a disconnected audio state (when JACK is unavailable)
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
        self.send(EngineCommand::LoadLinkedStem {
            deck: PREVIEW_DECK,
            stem_idx,
            path,
            host_bpm,
            host_drop_marker,
            host_duration,
        });
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
}

/// Start the JACK audio client for mesh-cue
///
/// Returns AudioState for UI interaction and JackHandle to keep client alive.
pub fn start_jack_client() -> Result<(AudioState, JackHandle), JackError> {
    let (client, _status) = Client::new("mesh-cue", ClientOptions::NO_START_SERVER)
        .map_err(|e| JackError::ClientCreation(e.to_string()))?;

    let sample_rate = client.sample_rate() as u32;

    log::info!(
        "JACK client 'mesh-cue' created (sample rate: {}, buffer size: {})",
        sample_rate,
        client.buffer_size()
    );

    // Register stereo output ports
    let left = client
        .register_port("out_left", AudioOut::default())
        .map_err(|e| JackError::PortRegistration(e.to_string()))?;

    let right = client
        .register_port("out_right", AudioOut::default())
        .map_err(|e| JackError::PortRegistration(e.to_string()))?;

    // Create engine and extract atomics before moving to processor
    let engine = AudioEngine::new_with_sample_rate(sample_rate);
    let deck_atomics = engine.deck_atomics()[PREVIEW_DECK].clone();
    // Get slicer atomics for all 4 stems on preview deck
    let slicer_atomics = engine.slicer_atomics_for_deck(PREVIEW_DECK);
    let linked_stem_atomics = engine.linked_stem_atomics()[PREVIEW_DECK].clone();
    // Get linked stem result receiver before engine is moved to processor
    let linked_stem_receiver = engine.linked_stem_result_receiver();

    // Create lock-free command channel
    let (command_tx, command_rx) = command_channel();

    // Create processor with engine (OWNED, not shared)
    let processor = JackProcessor {
        left,
        right,
        engine,
        command_rx,
        master_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
        cue_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
    };

    // Activate client
    let async_client = client
        .activate_async(JackNotifications, processor)
        .map_err(|e| JackError::Activation(e.to_string()))?;

    log::info!("JACK client activated - full AudioEngine ready");

    // Auto-connect to system playback
    if let Err(e) = auto_connect_ports() {
        log::warn!("Could not auto-connect to system playback: {}", e);
    }

    // Set deck 0 volume to 1.0 (master) for preview
    let mut audio_state = AudioState::new(
        CommandSender { producer: command_tx },
        deck_atomics,
        slicer_atomics,
        linked_stem_atomics,
        linked_stem_receiver,
        sample_rate,
    );
    audio_state.set_volume(1.0);

    Ok((
        audio_state,
        JackHandle {
            _async_client: async_client,
        },
    ))
}

/// Auto-connect mesh-cue outputs to system playback
fn auto_connect_ports() -> Result<(), JackError> {
    let (client, _) = Client::new("mesh-cue_connect", ClientOptions::NO_START_SERVER)
        .map_err(|e| JackError::ClientCreation(e.to_string()))?;

    let playback_ports = client.ports(
        Some("system:playback_.*"),
        None,
        jack::PortFlags::IS_INPUT,
    );

    if playback_ports.len() >= 2 {
        if let Err(e) = client.connect_ports_by_name("mesh-cue:out_left", &playback_ports[0]) {
            log::warn!("Could not connect left output: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name("mesh-cue:out_right", &playback_ports[1]) {
            log::warn!("Could not connect right output: {}", e);
        }
        log::info!(
            "Connected outputs to {} and {}",
            playback_ports[0],
            playback_ports[1]
        );
    }

    Ok(())
}
