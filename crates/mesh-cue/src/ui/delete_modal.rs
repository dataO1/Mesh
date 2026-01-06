//! Delete confirmation modal
//!
//! Provides a confirmation dialog for deleting tracks or playlists.
//! Differentiates between removing from playlist (safe) vs deleting from collection (permanent).

use super::app::Message;
use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Element, Length};
use mesh_core::playlist::NodeId;

/// What kind of deletion is being confirmed
#[derive(Debug, Clone)]
pub enum DeleteTarget {
    /// Tracks being removed from a playlist (just removes references, files stay)
    PlaylistTracks {
        playlist_name: String,
        track_names: Vec<String>,
        track_ids: Vec<NodeId>,
    },
    /// Tracks being permanently deleted from the collection (DELETES FILES!)
    CollectionTracks {
        track_names: Vec<String>,
        track_ids: Vec<NodeId>,
    },
    /// A playlist being deleted (tracks stay in collection)
    Playlist {
        playlist_name: String,
        playlist_id: NodeId,
    },
}

impl DeleteTarget {
    /// Returns true if this deletion is permanent (deletes files from disk)
    pub fn is_permanent(&self) -> bool {
        matches!(self, DeleteTarget::CollectionTracks { .. })
    }

    /// Get the IDs to delete
    pub fn ids(&self) -> Vec<NodeId> {
        match self {
            DeleteTarget::PlaylistTracks { track_ids, .. } => track_ids.clone(),
            DeleteTarget::CollectionTracks { track_ids, .. } => track_ids.clone(),
            DeleteTarget::Playlist { playlist_id, .. } => vec![playlist_id.clone()],
        }
    }
}

/// State for the delete confirmation modal
#[derive(Debug, Clone, Default)]
pub struct DeleteState {
    /// Whether the delete modal is open
    pub is_open: bool,
    /// What we're about to delete
    pub target: Option<DeleteTarget>,
}

impl DeleteState {
    /// Open the modal for a specific delete target
    pub fn show(&mut self, target: DeleteTarget) {
        self.target = Some(target);
        self.is_open = true;
    }

    /// Close the modal without deleting
    pub fn cancel(&mut self) {
        self.is_open = false;
        self.target = None;
    }

    /// Close the modal after deletion completes
    pub fn complete(&mut self) {
        self.is_open = false;
        self.target = None;
    }
}

/// Render the delete confirmation modal
pub fn view(state: &DeleteState) -> Element<'_, Message> {
    let Some(ref target) = state.target else {
        return Space::new().into();
    };

    let title = text("Confirm Delete").size(24);
    let close_btn = button(text("×").size(20))
        .on_press(Message::CancelDelete)
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let (description, items, warning) = match target {
        DeleteTarget::PlaylistTracks { playlist_name, track_names, .. } => {
            let desc = text(format!(
                "Remove {} track{} from \"{}\"?",
                track_names.len(),
                if track_names.len() == 1 { "" } else { "s" },
                playlist_name
            ))
            .size(16);
            let items = track_names.clone();
            let warn = text("Tracks will remain in your collection.")
                .size(12)
                .color(iced::Color::from_rgb(0.5, 0.5, 0.5));
            (desc, items, warn)
        }
        DeleteTarget::CollectionTracks { track_names, .. } => {
            let desc = text(format!(
                "Permanently delete {} track{}?",
                track_names.len(),
                if track_names.len() == 1 { "" } else { "s" }
            ))
            .size(16);
            let items = track_names.clone();
            let warn = text("⚠ WARNING: This will PERMANENTLY delete files from disk!")
                .size(14)
                .color(iced::Color::from_rgb(0.9, 0.2, 0.2));
            (desc, items, warn)
        }
        DeleteTarget::Playlist { playlist_name, .. } => {
            let desc = text(format!("Delete playlist \"{}\"?", playlist_name)).size(16);
            let warn = text("Tracks will remain in your collection.")
                .size(12)
                .color(iced::Color::from_rgb(0.5, 0.5, 0.5));
            (desc, Vec::new(), warn)
        }
    };

    // List of items being deleted (for tracks)
    let items_list: Element<Message> = if items.is_empty() {
        Space::new().height(0).into()
    } else if items.len() <= 5 {
        let item_elements: Vec<Element<Message>> = items
            .iter()
            .map(|name| text(format!("• {}", name)).size(12).into())
            .collect();
        column(item_elements).spacing(4).into()
    } else {
        // Show first 5 + "and X more"
        let mut item_elements: Vec<Element<Message>> = items
            .iter()
            .take(5)
            .map(|name| text(format!("• {}", name)).size(12).into())
            .collect();
        item_elements.push(
            text(format!("... and {} more", items.len() - 5))
                .size(12)
                .color(iced::Color::from_rgb(0.5, 0.5, 0.5))
                .into(),
        );
        scrollable(column(item_elements).spacing(4))
            .height(Length::Fixed(120.0))
            .into()
    };

    // Action buttons
    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CancelDelete)
        .style(button::secondary);

    let delete_btn = if target.is_permanent() {
        button(text("Delete Permanently"))
            .on_press(Message::ConfirmDelete)
            .style(button::danger)
    } else {
        button(text("Delete"))
            .on_press(Message::ConfirmDelete)
            .style(button::primary)
    };

    let actions = row![Space::new().width(Length::Fill), cancel_btn, delete_btn]
        .spacing(10)
        .width(Length::Fill);

    let body = column![header, description, items_list, warning, actions]
        .spacing(15)
        .width(Length::Fixed(450.0));

    container(body)
        .padding(30)
        .style(container::rounded_box)
        .into()
}
