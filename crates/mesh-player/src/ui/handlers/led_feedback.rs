//! LED feedback handler — 30Hz timer-driven
//!
//! Evaluates controller LED/display state based on current deck, mixer, and slicer state.
//! Runs on a 30Hz timer subscription, separate from the frame-synced tick handler.
//!
//! LED brightness changes are imperceptible above ~25Hz; 30Hz gives smooth
//! beat-synced pulsing (~10 cosine samples/beat at 174 BPM) while keeping
//! feedback evaluation off the critical rendering path.

use iced::Task;

use crate::ui::app::MeshApp;
use crate::ui::message::Message;

/// Handle the LED feedback update (called ~30Hz via timer subscription).
pub fn handle(app: &mut MeshApp) -> Task<Message> {
    let Some(ref mut controller) = app.controller else {
        return Task::none();
    };

    let mut feedback = mesh_midi::FeedbackState::default();

    // Compute beat phase from master deck's playhead + beatgrid
    let global_bpm = app.domain.global_bpm();
    if let Some(ref atomics) = app.deck_atomics {
        if global_bpm > 0.0 {
            // Find master deck, or fall back to deck 0
            let master_idx = (0..4).find(|&i| atomics[i].is_master()).unwrap_or(0);
            let position = atomics[master_idx].position() as f64;
            let first_beat = app.deck_views[master_idx].first_beat_sample() as f64;
            let samples_per_beat = 48000.0 * 60.0 / global_bpm;
            // Beat phase: how far through the current beat (0.0-1.0)
            // Halve the rate for fast tempos (>150 BPM) to keep the pulse comfortable
            let effective_spb = if global_bpm > 150.0 { samples_per_beat * 2.0 } else { samples_per_beat };
            let offset = (position - first_beat).rem_euclid(effective_spb);
            feedback.beat_phase = (offset / effective_spb) as f32;
        }
    }

    // Compute slicer preset assignment bitmap once (doesn't vary per deck)
    let slicer_presets_assigned: u8 = app.slice_editor.presets
        .iter()
        .enumerate()
        .fold(0u8, |acc, (i, p)| {
            if p.stems.iter().any(|s| s.is_some()) { acc | (1 << i) } else { acc }
        });

    for deck_idx in 0..4 {
        // Get play state and loop active from atomics
        if let Some(ref atomics) = app.deck_atomics {
            feedback.decks[deck_idx].is_playing = atomics[deck_idx].is_playing();
            feedback.decks[deck_idx].is_cueing = atomics[deck_idx].is_cueing();
            feedback.decks[deck_idx].loop_active = atomics[deck_idx].loop_active();
            feedback.decks[deck_idx].key_match_enabled =
                atomics[deck_idx].key_match_enabled.load(std::sync::atomic::Ordering::Relaxed);
        }

        // Get slicer state
        if let Some(ref slicer_atomics) = app.slicer_atomics {
            feedback.decks[deck_idx].slicer_active =
                slicer_atomics[deck_idx].active.load(std::sync::atomic::Ordering::Relaxed);
            feedback.decks[deck_idx].slicer_current_slice =
                slicer_atomics[deck_idx].current_slice.load(std::sync::atomic::Ordering::Relaxed);
        }

        // Get deck view state (hot cues, slip, stem mutes, action mode)
        feedback.decks[deck_idx].hot_cues_set = app.deck_views[deck_idx].hot_cues_bitmap();
        feedback.decks[deck_idx].slip_active = app.deck_views[deck_idx].slip_enabled();
        feedback.decks[deck_idx].stems_muted = app.deck_views[deck_idx].stems_muted_bitmap();

        // Linked stem state for LED color toggling + subtle pulse
        if let Some(ref linked_atomics) = app.linked_stem_atomics {
            let mut has_linked: u8 = 0;
            let mut use_linked: u8 = 0;
            for stem in 0..4 {
                if linked_atomics[deck_idx].has_linked[stem].load(std::sync::atomic::Ordering::Relaxed) {
                    has_linked |= 1 << stem;
                }
                if linked_atomics[deck_idx].use_linked[stem].load(std::sync::atomic::Ordering::Relaxed) {
                    use_linked |= 1 << stem;
                }
            }
            feedback.decks[deck_idx].has_linked = has_linked;
            feedback.decks[deck_idx].use_linked = use_linked;
        }

        // Set action mode for LED feedback
        use crate::ui::deck_view::ActionButtonMode;
        feedback.decks[deck_idx].action_mode = match app.deck_views[deck_idx].action_mode() {
            ActionButtonMode::Performance => mesh_midi::ActionMode::Performance,
            ActionButtonMode::HotCue => mesh_midi::ActionMode::HotCue,
            ActionButtonMode::Slicer => mesh_midi::ActionMode::Slicer,
        };

        // Slicer preset assignment bitmap (computed once above) and selected preset
        feedback.decks[deck_idx].slicer_presets_assigned = slicer_presets_assigned;
        feedback.decks[deck_idx].slicer_selected_preset = app.deck_views[deck_idx].slicer_selected_preset() as u8;

        // Loop length for 7-segment display
        feedback.decks[deck_idx].loop_length_beats = app.deck_views[deck_idx].loop_length_beats();

        // Get mixer cue (PFL) state
        feedback.mixer[deck_idx].cue_enabled = app.mixer_view.cue_enabled(deck_idx);
    }

    // Browse mode per-side
    feedback.browse_active = app.browse_mode_active;

    controller.update_feedback(&feedback);

    Task::none()
}
