//! Cross-platform audio backend for Mesh
//!
//! Provides a unified audio system using CPAL that works on Linux (ALSA/PipeWire),
//! Windows (WASAPI), and macOS (CoreAudio).
//!
//! # Architecture
//!
//! The audio system follows a lock-free design for real-time safety:
//!
//! - **UI Thread**: Sends commands via lock-free ringbuffer
//! - **Audio Thread**: Owns the AudioEngine exclusively, processes commands
//! - **Atomics**: UI reads playback state via relaxed atomics (no locks)
//!
//! # Output Modes
//!
//! - **MasterOnly**: Single stereo output (used by mesh-cue)
//! - **MasterAndCue**: Dual stereo outputs (used by mesh-player for DJ mixing)
//!
//! # Example Usage
//!
//! ```ignore
//! use mesh_core::audio::{AudioConfig, start_audio_system};
//!
//! // For mesh-cue (single output)
//! let config = AudioConfig::master_only();
//! let result = start_audio_system(&config, db_service)?;
//!
//! // For mesh-player (dual outputs)
//! let config = AudioConfig::master_and_cue();
//! let result = start_audio_system(&config, db_service)?;
//!
//! // Send commands from UI
//! result.command_sender.send(EngineCommand::Play { deck: 0 })?;
//!
//! // Read state via atomics (no locks)
//! let position = result.deck_atomics[0].position();
//! ```

mod config;
mod cpal_backend;
mod device;
mod error;

// Re-export public API
pub use config::{
    AudioConfig, BufferSize, DeviceId, OutputMode, DEFAULT_BUFFER_SIZE, MAX_BUFFER_SIZE,
};
pub use cpal_backend::{start_audio_system, AudioHandle, AudioSystemResult, CommandSender};
pub use device::{
    get_available_output_devices, get_available_stereo_pairs, get_default_device,
    get_output_devices, AudioDevice, OutputDevice, StereoPair,
};
pub use error::{AudioError, AudioResult};
