//! Audio backend for Mesh DJ Player
//!
//! Provides the interface to the cross-platform audio system.
//! Uses CPAL via mesh-core for audio output with support for
//! separate master and cue/headphone outputs.
//!
//! # Architecture
//!
//! The audio system uses a lock-free design:
//! - UI Thread: Sends commands via lock-free ringbuffer
//! - Audio Thread: Owns the AudioEngine exclusively
//! - Atomics: UI reads playback state without locks

use std::sync::Arc;

use mesh_core::audio::{self, AudioConfig, AudioHandle, AudioResult, DeviceId};
use mesh_core::db::DatabaseService;
use mesh_core::engine::{DeckAtomics, LinkedStemAtomics, SlicerAtomics};
use mesh_core::loader::LinkedStemResultReceiver;
use mesh_core::types::NUM_DECKS;

// Re-export types from mesh-core for compatibility
pub use mesh_core::audio::{
    get_available_stereo_pairs, CommandSender, OutputDevice, StereoPair,
};

/// Result type for start_audio_system
pub type AudioSystemResult = (
    AudioHandle,
    CommandSender,
    [Arc<DeckAtomics>; NUM_DECKS],
    [Arc<SlicerAtomics>; NUM_DECKS],
    [Arc<LinkedStemAtomics>; NUM_DECKS],
    LinkedStemResultReceiver,
    u32, // sample_rate
);

/// Start the audio system for mesh-player (master + cue outputs)
///
/// This sets up dual stereo outputs: one for master/speakers, one for cue/headphones.
/// Both outputs can optionally use different devices.
///
/// # Arguments
/// * `_client_name` - Client name (kept for API compatibility, not used with CPAL)
/// * `db_service` - Database service for the audio engine
///
/// # Returns
/// Tuple of (handle, command_sender, deck_atomics, slicer_atomics, linked_stem_atomics, linked_stem_receiver, sample_rate)
pub fn start_audio_system(
    _client_name: &str,
    db_service: Arc<DatabaseService>,
) -> AudioResult<AudioSystemResult> {
    // Use master+cue mode for DJ player
    let config = AudioConfig::master_and_cue();

    let result = audio::start_audio_system(&config, db_service)?;

    Ok((
        result.handle,
        result.command_sender,
        result.deck_atomics,
        result.slicer_atomics,
        result.linked_stem_atomics,
        result.linked_stem_receiver,
        result.sample_rate,
    ))
}

/// Start the audio system with specific device configuration
///
/// Allows selecting specific devices for master and cue outputs.
#[allow(dead_code)]
pub fn start_audio_system_with_devices(
    db_service: Arc<DatabaseService>,
    master_device: Option<DeviceId>,
    cue_device: Option<DeviceId>,
) -> AudioResult<AudioSystemResult> {
    let mut config = AudioConfig::master_and_cue();
    config.master_device = master_device;
    config.cue_device = cue_device;

    let result = audio::start_audio_system(&config, db_service)?;

    Ok((
        result.handle,
        result.command_sender,
        result.deck_atomics,
        result.slicer_atomics,
        result.linked_stem_atomics,
        result.linked_stem_receiver,
        result.sample_rate,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_enumeration() {
        let devices = get_available_stereo_pairs();
        println!("Found {} audio devices", devices.len());
        for device in &devices {
            println!("  - {}", device.label);
        }
    }
}
