//! Direct dispatch trait for bypassing UI tick latency
//!
//! Timing-critical MIDI commands (hot cue, play, beat jump) can be
//! dispatched directly to the audio engine from the MIDI callback thread,
//! bypassing the ~16ms iced tick loop. The UI still receives the event
//! (with `engine_dispatched: true`) for visual updates.

use crate::messages::DeckAction;

/// Trait for dispatching timing-critical commands directly to the audio engine
///
/// Implementations push commands onto a lock-free ringbuffer that the audio
/// thread drains alongside the normal command queue.
pub trait DirectDispatch: Send + Sync {
    /// Attempt to dispatch a deck action directly to the audio engine.
    ///
    /// Returns `true` if the action was dispatched (the UI should skip
    /// sending the duplicate engine command). Returns `false` if the action
    /// is not timing-critical or the ringbuffer is full.
    fn dispatch(&self, deck: usize, action: &DeckAction) -> bool;
}
