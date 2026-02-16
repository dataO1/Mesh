//! Direct MIDI → Engine dispatch
//!
//! Implements the `DirectDispatch` trait from mesh-midi, pushing timing-critical
//! EngineCommands directly into the audio thread's ringbuffer from the MIDI
//! callback thread. This bypasses the ~16ms iced tick loop for actions where
//! latency matters (play, cue, hot cue, beat jump).

use std::sync::Mutex;

use mesh_core::engine::EngineCommand;
use mesh_midi::{DeckAction, DirectDispatch};

/// Routes timing-critical MIDI deck actions directly to the audio engine
/// via a lock-free SPSC ringbuffer, bypassing the iced tick loop.
///
/// The `Mutex` is only needed because `rtrb::Producer` requires `&mut self`
/// for `push()`, but `DirectDispatch::dispatch()` takes `&self`. Contention
/// is negligible — only one MIDI callback thread calls this.
pub struct EngineDirectDispatch {
    producer: Mutex<rtrb::Producer<EngineCommand>>,
}

impl EngineDirectDispatch {
    pub fn new(producer: rtrb::Producer<EngineCommand>) -> Self {
        Self {
            producer: Mutex::new(producer),
        }
    }
}

impl DirectDispatch for EngineDirectDispatch {
    fn dispatch(&self, deck: usize, action: &DeckAction) -> bool {
        // Only dispatch timing-critical actions directly
        let command = match action {
            DeckAction::TogglePlay => Some(EngineCommand::TogglePlay { deck }),
            DeckAction::CuePress => Some(EngineCommand::CuePress { deck }),
            DeckAction::CueRelease => Some(EngineCommand::CueRelease { deck }),
            DeckAction::HotCuePress { slot } => Some(EngineCommand::HotCuePress { deck, slot: *slot }),
            DeckAction::HotCueRelease { .. } => Some(EngineCommand::HotCueRelease { deck }),
            DeckAction::BeatJumpForward => Some(EngineCommand::BeatJumpForward { deck }),
            DeckAction::BeatJumpBackward => Some(EngineCommand::BeatJumpBackward { deck }),
            _ => None,
        };

        if let Some(cmd) = command {
            if let Ok(mut producer) = self.producer.lock() {
                if producer.push(cmd).is_ok() {
                    return true;
                }
                log::warn!("Direct dispatch ringbuffer full — command dropped");
            }
        }
        false
    }
}
