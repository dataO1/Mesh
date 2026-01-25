//! Stem link message handlers
//!
//! Handles: StartStemLinkSelection, ConfirmStemLink, ClearStemLink, ToggleStemLinkActive

use iced::Task;
use mesh_core::audio_file::StemLinkReference;
use mesh_core::types::Stem;
use super::super::app::MeshCueApp;
use super::super::message::Message;

impl MeshCueApp {
    /// Handle StartStemLinkSelection message
    pub fn handle_start_stem_link_selection(&mut self, stem_idx: usize) -> Task<Message> {
        // Shift+click = clear stem link
        if self.shift_held {
            return self.handle_clear_stem_link(stem_idx);
        }

        // Enter stem link selection mode
        self.stem_link_selection = Some(stem_idx);
        log::info!("Started stem link selection for stem {}", stem_idx);
        // Focus the browser for track selection
        // (browser will highlight when stem_link_selection is Some)
        Task::none()
    }

    /// Handle ConfirmStemLink message
    ///
    /// Called when user confirms track selection in stem link mode
    pub fn handle_confirm_stem_link(&mut self, stem_idx: usize) -> Task<Message> {
        // Get the source track path from browser selection
        let mut source_path: Option<std::path::PathBuf> = None;

        // Check right browser for selection
        if let Some(ref track_id) = self.collection.browser_right.table_state.last_selected {
            if let Some(node) = self.domain.get_node(track_id) {
                source_path = node.track_path.clone();
            }
        }

        // If no selection in right browser, check left browser
        if source_path.is_none() {
            if let Some(ref track_id) = self.collection.browser_left.table_state.last_selected {
                if let Some(node) = self.domain.get_node(track_id) {
                    source_path = node.track_path.clone();
                }
            }
        }

        // Create the stem link if we have a valid source path
        if let Some(path) = source_path {
            if let Some(ref mut state) = self.collection.loaded_track {
                let link = StemLinkReference {
                    stem_index: stem_idx as u8,
                    source_path: path.clone(),
                    source_stem: stem_idx as u8, // Same stem from source
                    source_drop_marker: 0, // Will be filled when source is analyzed
                };

                // Remove any existing link for this stem
                state.stem_links.retain(|l| l.stem_index != stem_idx as u8);
                // Add new link
                state.stem_links.push(link);
                state.modified = true;

                log::info!("Linked stem {} to track {:?}", stem_idx, path);
            }
        } else {
            log::warn!("ConfirmStemLink: No track selected in browser");
        }

        // Exit selection mode
        self.stem_link_selection = None;
        Task::none()
    }

    /// Handle ClearStemLink message
    pub fn handle_clear_stem_link(&mut self, stem_idx: usize) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            state.stem_links.retain(|l| l.stem_index != stem_idx as u8);
            state.modified = true;
            log::info!("Cleared stem link for stem {}", stem_idx);
        }
        // Also exit selection mode if we were in it
        self.stem_link_selection = None;
        Task::none()
    }

    /// Handle ToggleStemLinkActive message
    ///
    /// Toggle between original and linked stem for playback.
    /// Shift+click clears the stem link entirely.
    pub fn handle_toggle_stem_link_active(&mut self, stem_idx: usize) -> Task<Message> {
        // Shift+click = clear stem link (delete the link)
        if self.shift_held {
            return self.handle_clear_stem_link(stem_idx);
        }

        // Normal click = toggle between original and linked stem
        if let Some(stem) = Stem::from_index(stem_idx) {
            self.audio.toggle_linked_stem(stem);
            log::info!("Toggled linked stem active for stem {}", stem_idx);
        }
        Task::none()
    }
}
