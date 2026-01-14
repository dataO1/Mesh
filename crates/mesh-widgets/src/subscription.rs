//! Subscription helpers for bridging sync channels to iced subscriptions
//!
//! This module provides utilities for converting `std::sync::mpsc` channels
//! to iced `Subscription`s, enabling message-driven architecture in UI apps.
//!
//! # Usage
//!
//! ```ignore
//! use mesh_widgets::mpsc_subscription;
//!
//! fn subscription(&self) -> Subscription<Message> {
//!     Subscription::batch([
//!         mpsc_subscription(
//!             self.track_loader.result_receiver(),
//!         ).map(Message::TrackLoaded),
//!         mpsc_subscription(
//!             self.linked_stem_loader.result_receiver(),
//!         ).map(Message::LinkedStemLoaded),
//!         // ... other subscriptions
//!     ])
//! }
//! ```

use std::any::TypeId;
use std::hash::Hash;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use iced::advanced::subscription::{self, EventStream, Hasher, Recipe};
use iced::futures::stream::BoxStream;
use iced::Subscription;

/// Recipe for polling an mpsc receiver as an iced subscription.
///
/// This implements iced's `Recipe` trait to create a proper subscription
/// that polls a sync channel receiver with minimal CPU overhead.
struct MpscRecipe<T> {
    /// Unique ID for subscription identity (typically TypeId or pointer)
    id: u64,
    /// The receiver to poll
    receiver: Arc<Mutex<Receiver<T>>>,
}

impl<T: Send + 'static> Recipe for MpscRecipe<T> {
    type Output = T;

    fn hash(&self, state: &mut Hasher) {
        // Use TypeId + our unique ID for subscription identity
        TypeId::of::<Self>().hash(state);
        self.id.hash(state);
    }

    fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<'static, Self::Output> {
        let receiver = self.receiver;

        Box::pin(iced::futures::stream::unfold(receiver, |rx| async move {
            loop {
                // Try to receive without blocking
                if let Some(item) = rx.lock().ok().and_then(|r| r.try_recv().ok()) {
                    return Some((item, rx));
                }

                // Small sleep to avoid busy-spinning while remaining responsive
                // 1ms is fast enough for UI updates while being CPU-friendly
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        }))
    }
}

/// Create an iced subscription from a sync mpsc channel receiver.
///
/// This bridges synchronous `std::sync::mpsc::Receiver` to async iced subscriptions
/// by polling the receiver with a small sleep interval (1ms).
///
/// # Arguments
///
/// * `receiver` - Arc-wrapped mutex-protected receiver to poll
///
/// # Returns
///
/// A `Subscription<T>` that yields items from the receiver. Use `.map()` to
/// convert to your message type.
///
/// # Example
///
/// ```ignore
/// mpsc_subscription(self.loader.result_receiver())
///     .map(Message::TrackLoaded)
/// ```
pub fn mpsc_subscription<T>(receiver: Arc<Mutex<Receiver<T>>>) -> Subscription<T>
where
    T: Send + 'static,
{
    // Use pointer address as unique ID for this receiver
    let id = Arc::as_ptr(&receiver) as u64;

    subscription::from_recipe(MpscRecipe { id, receiver })
}

/// Variant of `mpsc_subscription` that takes ownership of receiver directly.
///
/// Useful when the receiver is not wrapped in Arc<Mutex<>>.
///
/// # Example
///
/// ```ignore
/// let (tx, rx) = std::sync::mpsc::channel();
/// mpsc_subscription_owned(rx).map(Message::DataReceived)
/// ```
pub fn mpsc_subscription_owned<T>(receiver: Receiver<T>) -> Subscription<T>
where
    T: Send + 'static,
{
    let receiver = Arc::new(Mutex::new(receiver));
    mpsc_subscription(receiver)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Testing subscriptions requires an iced runtime, so these are
    // integration tests rather than unit tests. The subscription helpers
    // are tested indirectly through the app integration tests.

    #[test]
    fn test_types_compile() {
        // Just verify the function signatures compile correctly
        fn _check_mpsc<T>(_: Subscription<T>) {}
    }
}
