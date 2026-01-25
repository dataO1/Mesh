//! Keyboard input message handlers
//!
//! Handles: KeyPressed, KeyReleased, ModifiersChanged, GlobalMouseMoved

use iced::{keyboard, Task};
use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::state::{BrowserSide, View};
use crate::keybindings;

impl MeshCueApp {
    /// Handle KeyPressed message
    pub fn handle_key_pressed(
        &mut self,
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        repeat: bool,
    ) -> Task<Message> {
        // Track modifier key states for selection actions
        self.shift_held = modifiers.shift();
        self.ctrl_held = modifiers.control();

        // Only handle keybindings in Collection view
        if self.current_view != View::Collection {
            return Task::none();
        }

        // Enter key loads selected track (works even without a loaded track)
        // With multi-selection, loads the most recently selected track
        if !repeat {
            if let keyboard::Key::Named(keyboard::key::Named::Enter) = &key {
                log::info!("Enter pressed - checking for selected track");
                // Check left browser's most recent selection first
                if let Some(ref track_id) =
                    self.collection.browser_left.table_state.last_selected
                {
                    log::info!("  Found selection in left browser: {:?}", track_id);
                    if let Some(node) = self.domain.get_node(track_id) {
                        log::info!(
                            "  Node found: kind={:?}, track_path={:?}",
                            node.kind,
                            node.track_path
                        );
                        if let Some(path) = node.track_path {
                            return self.update(Message::LoadTrackByPath(path));
                        }
                    }
                }
                // Then check right browser
                if let Some(ref track_id) =
                    self.collection.browser_right.table_state.last_selected
                {
                    log::info!("  Found selection in right browser: {:?}", track_id);
                    if let Some(node) = self.domain.get_node(track_id) {
                        log::info!(
                            "  Node found: kind={:?}, track_path={:?}",
                            node.kind,
                            node.track_path
                        );
                        if let Some(path) = node.track_path {
                            return self.update(Message::LoadTrackByPath(path));
                        }
                    }
                }
            }

            // Delete key opens delete confirmation modal
            if let keyboard::Key::Named(keyboard::key::Named::Delete) = &key {
                log::info!("Delete pressed - checking for selected tracks");
                // Check which browser has selection (prefer left)
                if self.collection.browser_left.table_state.has_selection() {
                    return self.update(Message::RequestDelete(BrowserSide::Left));
                } else if self.collection.browser_right.table_state.has_selection() {
                    return self.update(Message::RequestDelete(BrowserSide::Right));
                }
            }
        }

        // Remaining keybindings require a loaded track
        if self.collection.loaded_track.is_none() {
            return Task::none();
        }

        // Convert key + modifiers to string for matching
        let key_str = keybindings::key_to_string(&key, &modifiers);
        if key_str.is_empty() {
            return Task::none();
        }

        let bindings = &self.keybindings.editing;

        // Play/Pause (ignore repeat)
        if !repeat && bindings.play_pause.iter().any(|b| b == &key_str) {
            let is_playing = self.collection.loaded_track.as_ref()
                .map(|s| s.is_playing()).unwrap_or(false);
            return self.update(if is_playing { Message::Pause } else { Message::Play });
        }

        // Beat jump forward/backward (allow repeat for continuous jumping)
        if bindings.beat_jump_forward.iter().any(|b| b == &key_str) {
            let jump_size = self.collection.loaded_track.as_ref()
                .map(|s| s.beat_jump_size()).unwrap_or(4);
            return self.update(Message::BeatJump(jump_size));
        }
        if bindings.beat_jump_backward.iter().any(|b| b == &key_str) {
            let jump_size = self.collection.loaded_track.as_ref()
                .map(|s| s.beat_jump_size()).unwrap_or(4);
            return self.update(Message::BeatJump(-jump_size));
        }

        // Beat grid nudge (allow repeat)
        if bindings.grid_nudge_forward.iter().any(|b| b == &key_str) {
            return self.update(Message::NudgeBeatGridRight);
        }
        if bindings.grid_nudge_backward.iter().any(|b| b == &key_str) {
            return self.update(Message::NudgeBeatGridLeft);
        }

        // Beat grid align to playhead (ignore repeat)
        if !repeat && bindings.align_beat_grid.iter().any(|b| b == &key_str) {
            return self.update(Message::AlignBeatGridToPlayhead);
        }

        // Increase/decrease loop length (also affects beat jump size, ignore repeat)
        if !repeat && bindings.increase_jump_size.iter().any(|b| b == &key_str) {
            if self.collection.loaded_track.is_some() {
                self.audio.adjust_loop_length(1); // Double
            }
            return Task::none();
        }
        if !repeat && bindings.decrease_jump_size.iter().any(|b| b == &key_str) {
            if self.collection.loaded_track.is_some() {
                self.audio.adjust_loop_length(-1); // Halve
            }
            return Task::none();
        }

        // Delete hot cues (ignore repeat)
        if !repeat {
            if let Some(index) = bindings.match_delete_hot_cue(&key_str) {
                return self.update(Message::ClearCuePoint(index));
            }
        }

        // Main cue button (filter repeat - only trigger on first press)
        if bindings.match_cue_button(&key_str) {
            if !repeat && !self.pressed_cue_key {
                self.pressed_cue_key = true;
                return self.update(Message::Cue);
            }
            return Task::none();
        }

        // Hot cue trigger/set (filter repeat - only trigger on first press)
        if let Some(index) = bindings.match_hot_cue(&key_str) {
            // Skip if repeat and key already pressed
            if repeat && self.pressed_hot_cue_keys.contains(&index) {
                return Task::none();
            }

            // Track this key as pressed
            self.pressed_hot_cue_keys.insert(index);

            // If cue exists, trigger it; otherwise set it
            let cue_exists = self.collection.loaded_track.as_ref()
                .map(|s| s.cue_points.iter().any(|c| c.index == index as u8))
                .unwrap_or(false);
            if cue_exists {
                return self.update(Message::HotCuePressed(index));
            } else {
                return self.update(Message::SetCuePoint(index));
            }
        }

        Task::none()
    }

    /// Handle KeyReleased message
    pub fn handle_key_released(
        &mut self,
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
    ) -> Task<Message> {
        // ALWAYS update modifier state, regardless of view
        // This fixes Shift+Click not working after Shift is released
        self.shift_held = modifiers.shift();
        self.ctrl_held = modifiers.control();

        // Only handle keybindings in Collection view with a loaded track
        if self.current_view != View::Collection {
            return Task::none();
        }
        if self.collection.loaded_track.is_none() {
            return Task::none();
        }

        // Convert key to string for matching
        let key_str = keybindings::key_to_string(&key, &modifiers);
        if key_str.is_empty() {
            return Task::none();
        }

        let bindings = &self.keybindings.editing;

        // Main cue button release - stop preview, return to cue point
        if bindings.match_cue_button(&key_str) && self.pressed_cue_key {
            self.pressed_cue_key = false;
            return self.update(Message::CueReleased);
        }

        // Hot cue release - dispatch HotCueReleased to stop preview
        if let Some(index) = bindings.match_hot_cue(&key_str) {
            // Only release if this key was tracked as pressed
            if self.pressed_hot_cue_keys.remove(&index) {
                // Only send release if cue exists (preview was started)
                let cue_exists = self.collection.loaded_track.as_ref()
                    .map(|s| s.cue_points.iter().any(|c| c.index == index as u8))
                    .unwrap_or(false);
                if cue_exists {
                    return self.update(Message::HotCueReleased(index));
                }
            }
        }

        Task::none()
    }

    /// Handle ModifiersChanged message
    pub fn handle_modifiers_changed(&mut self, modifiers: keyboard::Modifiers) -> Task<Message> {
        // Track modifier key states for Shift+Click and Ctrl+Click selection
        // This fires when modifiers change without another key being pressed
        self.shift_held = modifiers.shift();
        self.ctrl_held = modifiers.control();
        log::debug!(
            "[MODIFIERS] shift={}, ctrl={}",
            self.shift_held,
            self.ctrl_held
        );
        Task::none()
    }

    /// Handle GlobalMouseMoved message
    pub fn handle_global_mouse_moved(&mut self, position: iced::Point) -> Task<Message> {
        self.global_mouse_position = position;
        Task::none()
    }
}
