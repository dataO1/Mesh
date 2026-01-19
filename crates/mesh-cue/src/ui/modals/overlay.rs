//! Modal overlay building utilities
//!
//! These helpers eliminate repetitive backdrop and overlay construction code
//! that was duplicated across export, import, delete, and settings modals.

use iced::widget::{center, container, mouse_area, opaque, stack, Space};
use iced::{Color, Element, Length};

use super::super::message::Message;

/// Build a semi-transparent backdrop that closes the modal on click
///
/// Creates a full-screen dark overlay (60% opacity black) that intercepts
/// clicks and sends the specified close message.
///
/// # Arguments
/// * `close_message` - The message to send when the backdrop is clicked
pub fn build_backdrop(close_message: Message) -> Element<'static, Message> {
    mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_theme| container::Style {
                background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.6).into()),
                ..Default::default()
            }),
    )
    .on_press(close_message)
    .into()
}

/// Wrap content in a modal overlay with backdrop
///
/// Combines the base content, a dark backdrop, and the modal content into
/// a stacked view where the modal appears centered above the backdrop.
///
/// # Arguments
/// * `base` - The main application content behind the modal
/// * `modal_content` - The modal dialog content to display
/// * `close_message` - The message to send when clicking outside the modal
///
/// # Example
/// ```ignore
/// with_modal_overlay(
///     base_content,
///     super::import_modal::view(&self.import_state),
///     Message::CloseImport,
/// )
/// ```
pub fn with_modal_overlay<'a>(
    base: Element<'a, Message>,
    modal_content: Element<'a, Message>,
    close_message: Message,
) -> Element<'a, Message> {
    let backdrop = build_backdrop(close_message);

    let modal = center(opaque(modal_content))
        .width(Length::Fill)
        .height(Length::Fill);

    stack![base, backdrop, modal].into()
}
