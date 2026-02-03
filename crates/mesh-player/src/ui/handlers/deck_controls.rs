//! Deck controls message handler
//!
//! Handles all per-deck control messages: playback, hot cues, loops, stems, and slicer.

use iced::Task;

use crate::ui::app::MeshApp;
use crate::ui::deck_view::{DeckMessage, ActionButtonMode};
use crate::ui::message::Message;
use mesh_core::types::Stem;

/// Handle deck control messages
pub fn handle(app: &mut MeshApp, deck_idx: usize, deck_msg: DeckMessage) -> Task<Message> {
    if deck_idx >= 4 {
        return Task::none();
    }

    use DeckMessage::*;
    match deck_msg {
        // ─────────────────────────────────────────────────
        // Playback Control
        // ─────────────────────────────────────────────────
        TogglePlayPause => {
            app.domain.toggle_play(deck_idx);
        }
        CuePressed => {
            app.domain.cue_press(deck_idx);
        }
        CueReleased => {
            app.domain.cue_release(deck_idx);
        }
        SetCue => {
            app.domain.set_cue_point(deck_idx);
        }

        // ─────────────────────────────────────────────────
        // Hot Cues
        // ─────────────────────────────────────────────────
        HotCuePressed(slot) => {
            // If slot is empty, engine will set a new hot cue at current position
            // Update UI optimistically by reading current position from atomics
            let slot_was_empty = app.deck_views[deck_idx].hot_cue_position(slot).is_none();
            if slot_was_empty {
                if let Some(ref atomics) = app.deck_atomics {
                    let position = atomics[deck_idx].position();
                    app.deck_views[deck_idx].set_hot_cue_position(slot, Some(position));
                }
            }
            app.domain.hot_cue_press(deck_idx, slot);
        }
        HotCueReleased(_slot) => {
            app.domain.hot_cue_release(deck_idx);
        }
        SetHotCue(_slot) => {
            // Hot cue is set automatically on press if empty
        }
        ClearHotCue(slot) => {
            // Clear the UI state for this hot cue slot
            app.deck_views[deck_idx].set_hot_cue_position(slot, None);
            app.domain.clear_hot_cue(deck_idx, slot);
        }
        Sync => {
            // TODO: Implement sync command
        }

        // ─────────────────────────────────────────────────
        // Loop Control
        // ─────────────────────────────────────────────────
        ToggleLoop => {
            app.domain.toggle_loop(deck_idx);
        }
        ToggleSlip => {
            app.domain.toggle_slip(deck_idx);
        }
        ToggleKeyMatch => {
            // Toggle key matching for this deck
            let current = app.deck_views[deck_idx].key_match_enabled();
            app.domain.set_key_match_enabled(deck_idx, !current);
        }
        SetLoopLength(_beats) => {
            // Loop length is handled via adjust commands
        }
        LoopHalve => {
            app.domain.adjust_loop_length(deck_idx, -1);
        }
        LoopDouble => {
            app.domain.adjust_loop_length(deck_idx, 1);
        }

        // ─────────────────────────────────────────────────
        // Beat Jump
        // ─────────────────────────────────────────────────
        BeatJumpBack => {
            app.domain.beat_jump_backward(deck_idx);
        }
        BeatJumpForward => {
            app.domain.beat_jump_forward(deck_idx);
        }

        // ─────────────────────────────────────────────────
        // Stem Control
        // ─────────────────────────────────────────────────
        ToggleStemMute(stem_idx) => {
            let shift_held = app.deck_views[deck_idx].shift_held();
            log::info!(
                "[STEM_TOGGLE] Stem button pressed: deck={}, stem={}, shift_held={}",
                deck_idx, stem_idx, shift_held
            );

            if shift_held {
                // Shift+Stem: Linked stem operation
                app.handle_shift_stem(deck_idx, stem_idx);
            } else {
                // Normal: Toggle mute
                if let Some(stem) = Stem::from_index(stem_idx) {
                    app.domain.toggle_stem_mute(deck_idx, stem);
                }
                // Toggle mute state in DeckView for UI
                let was_muted = app.deck_views[deck_idx].is_stem_muted(stem_idx);
                let new_muted = !was_muted;
                app.deck_views[deck_idx].set_stem_muted(stem_idx, new_muted);

                // stem_active = NOT muted (when muted, stem is inactive)
                app.player_canvas_state.set_stem_active(deck_idx, stem_idx, !new_muted);
            }
        }
        ToggleStemSolo(stem_idx) => {
            if let Some(stem) = Stem::from_index(stem_idx) {
                app.domain.toggle_stem_solo(deck_idx, stem);
            }
            // Toggle solo state
            let was_soloed = app.deck_views[deck_idx].is_stem_soloed(stem_idx);
            let new_soloed = !was_soloed;

            if new_soloed {
                // Solo: this stem becomes active, all others become inactive
                for i in 0..4 {
                    app.deck_views[deck_idx].set_stem_soloed(i, i == stem_idx);
                    // When soloing, set active state based on solo selection
                    // (ignore mute state - solo overrides)
                    app.player_canvas_state.set_stem_active(deck_idx, i, i == stem_idx);
                }
            } else {
                // Un-solo: all stems become active (unless muted)
                app.deck_views[deck_idx].set_stem_soloed(stem_idx, false);
                for i in 0..4 {
                    let is_muted = app.deck_views[deck_idx].is_stem_muted(i);
                    app.player_canvas_state.set_stem_active(deck_idx, i, !is_muted);
                }
            }
        }
        SelectStem(stem_idx) => {
            // UI-only state, no command needed
            app.deck_views[deck_idx].set_selected_stem(stem_idx);
        }
        SetStemKnob(stem_idx, knob_idx, value) => {
            // Update UI state
            if stem_idx < 4 && knob_idx < 8 {
                app.deck_views[deck_idx].set_stem_knob(stem_idx, knob_idx, value);
            }

            // Send to audio engine via domain
            // Mapping: 8 knobs per stem, first 3 go to effect 0, next 3 to effect 1, etc.
            // This allows up to 2-3 effects with 3-4 params each
            let effect_idx = knob_idx / 3;
            let param_idx = knob_idx % 3;

            if let Some(stem) = Stem::from_index(stem_idx) {
                app.domain.set_effect_param(deck_idx, stem, effect_idx, param_idx, value);
            }
        }
        ToggleEffectBypass(stem_idx, effect_idx) => {
            if let Some(stem) = Stem::from_index(stem_idx) {
                // Get current bypass state from UI and toggle it
                let currently_bypassed = app.deck_views[deck_idx]
                    .is_effect_bypassed(stem_idx, effect_idx);
                let new_bypass = !currently_bypassed;
                app.domain.set_effect_bypass(deck_idx, stem, effect_idx, new_bypass);
                // Update UI state
                app.deck_views[deck_idx].toggle_effect_bypass(stem_idx, effect_idx);
            }
        }

        // ─────────────────────────────────────────────────
        // Slicer Mode Controls
        // ─────────────────────────────────────────────────
        SetActionMode(mode) => {
            // Update UI state
            app.deck_views[deck_idx].set_action_mode(mode);

            // Enable/disable slicer based on mode for stems with patterns
            let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
            let preset = &app.slice_editor.presets[app.slice_editor.selected_preset];

            match mode {
                ActionButtonMode::Slicer => {
                    // Entering slicer mode - enable slicer for stems with patterns
                    for (idx, &stem) in stems.iter().enumerate() {
                        if preset.stems[idx].is_some() {
                            app.domain.set_slicer_enabled(deck_idx, stem, true);
                        }
                    }
                }
                ActionButtonMode::HotCue => {
                    // Leaving slicer mode - disable processing but keep queue arrangement
                    for &stem in &stems {
                        app.domain.set_slicer_enabled(deck_idx, stem, false);
                    }
                }
            }
        }
        SlicerPresetSelect(preset_idx) => {
            // Select a new slicer preset via engine's button action
            // The engine handles enabling slicers and loading patterns
            let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

            // Update selected preset in slice editor state
            app.slice_editor.selected_preset = preset_idx;

            // Send button action to engine for each stem (shift_held=false = load preset)
            let preset = &app.slice_editor.presets[preset_idx];
            for (idx, &stem) in stems.iter().enumerate() {
                if preset.stems[idx].is_some() {
                    app.domain.slicer_button_action(deck_idx, stem, preset_idx, false);
                }
            }
        }
        SlicerTrigger(button_idx) => {
            // Shift+click triggers slice for live queue adjustment
            let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
            let shift_held = app.deck_views[deck_idx].shift_held();
            let preset = &app.slice_editor.presets[app.slice_editor.selected_preset];

            for (idx, &stem) in stems.iter().enumerate() {
                if preset.stems[idx].is_some() {
                    app.domain.slicer_button_action(deck_idx, stem, button_idx, shift_held);
                }
            }
        }
        ResetSlicerPattern => {
            // Reset slicer queue to default [0..15]
            let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
            let preset = &app.slice_editor.presets[app.slice_editor.selected_preset];

            for (idx, &stem) in stems.iter().enumerate() {
                if preset.stems[idx].is_some() {
                    app.domain.slicer_reset_queue(deck_idx, stem);
                }
            }
        }

        // ─────────────────────────────────────────────────
        // Shift State (UI only)
        // ─────────────────────────────────────────────────
        ShiftPressed => {
            app.deck_views[deck_idx].set_shift_held(true);
        }
        ShiftReleased => {
            app.deck_views[deck_idx].set_shift_held(false);
        }

        // ─────────────────────────────────────────────────
        // Effect Chain Control
        // ─────────────────────────────────────────────────
        RemoveEffect(stem_idx, effect_idx) => {
            let stem = match stem_idx {
                0 => Stem::Vocals,
                1 => Stem::Drums,
                2 => Stem::Bass,
                _ => Stem::Other,
            };
            app.domain.remove_effect(deck_idx, stem, effect_idx);
            // Update UI state
            app.deck_views[deck_idx].remove_effect(stem_idx, effect_idx);
        }
        OpenEffectPicker(stem_idx) => {
            // Open effect picker modal for this deck/stem
            return Task::done(Message::EffectPicker(
                crate::ui::effect_picker::EffectPickerMessage::Open {
                    deck: deck_idx,
                    stem: stem_idx,
                }
            ));
        }
        AddEffect(stem_idx, effect_id) => {
            let stem = match stem_idx {
                0 => Stem::Vocals,
                1 => Stem::Drums,
                2 => Stem::Bass,
                _ => Stem::Other,
            };
            // Look up the effect's display name
            let effect_name = app.domain.available_effects()
                .iter()
                .find(|e| e.id == effect_id)
                .map(|e| e.name().to_string())
                .unwrap_or_else(|| effect_id.clone());

            if let Err(e) = app.domain.add_pd_effect(deck_idx, stem, &effect_id) {
                log::error!("Failed to add effect '{}': {}", effect_id, e);
            } else {
                // Update UI state
                app.deck_views[deck_idx].add_effect(stem_idx, effect_name);
            }
        }
    }
    Task::none()
}
