//! Tick handler for periodic UI updates
//!
//! The Tick message is sent at 60fps to synchronize UI state with the audio engine.
//! This includes:
//! - Playhead position updates
//! - Loop region overlay
//! - Linked stem state
//! - Slicer visualization
//! - Import/reanalysis progress polling

use iced::Task;
use mesh_widgets::ZoomedViewMode;
use std::sync::atomic::Ordering;

use super::super::app::MeshCueApp;
use super::super::message::Message;

impl MeshCueApp {
    /// Handle Tick message
    ///
    /// Called at 60fps to sync UI with audio engine state via atomics.
    /// This is the "render loop" for UI state that depends on playback.
    pub fn handle_tick(&mut self) -> Task<Message> {
        // Update UI from audio engine state (atomics)
        if let Some(ref mut state) = self.collection.loaded_track {
            if state.duration_samples > 0 {

                // Update loop region overlay (green overlay when loop is active)
                if state.is_loop_active() {
                    let (loop_start, loop_end) = state.loop_bounds();
                    let start_norm = loop_start as f64 / state.duration_samples as f64;
                    let end_norm = loop_end as f64 / state.duration_samples as f64;
                    state
                        .combined_waveform
                        .overview
                        .set_loop_region(Some((start_norm, end_norm)));
                    state
                        .combined_waveform
                        .zoomed
                        .set_loop_region(Some((start_norm, end_norm)));
                } else {
                    state.combined_waveform.overview.set_loop_region(None);
                    state.combined_waveform.zoomed.set_loop_region(None);
                }
            }

            // Sync linked stem state from atomics for waveform display
            let linked_atomics = self.audio.linked_stem_atomics();
            for stem_idx in 0..4 {
                let has_linked = linked_atomics.has_linked[stem_idx].load(Ordering::Relaxed);
                let is_active = linked_atomics.use_linked[stem_idx].load(Ordering::Relaxed);
                state
                    .combined_waveform
                    .set_linked_stem(stem_idx, has_linked, is_active);
            }

            // Sync LUFS gain from engine for waveform scaling (single source of truth)
            let lufs_gain = self.audio.lufs_gain();
            state.combined_waveform.zoomed.set_lufs_gain(lufs_gain);
        }

        if let Some(ref mut state) = self.collection.loaded_track {
            // Sync slicer state from atomics for waveform overlay
            // Check all 4 stems for active slicer
            let slicer_atomics = self.audio.slicer_atomics();
            let duration = state.duration_samples as u64;
            let mut any_active = false;

            for stem_idx in 0..4 {
                let sa = &slicer_atomics[stem_idx];
                let active = sa.active.load(Ordering::Relaxed);
                if active && duration > 0 {
                    let buffer_start = sa.buffer_start.load(Ordering::Relaxed);
                    let buffer_end = sa.buffer_end.load(Ordering::Relaxed);
                    let current_slice = sa.current_slice.load(Ordering::Relaxed);

                    // Convert to normalized positions
                    let start_norm = buffer_start as f64 / duration as f64;
                    let end_norm = buffer_end as f64 / duration as f64;

                    // Set slicer overlay on both waveform views
                    state
                        .combined_waveform
                        .overview
                        .set_slicer_region(Some((start_norm, end_norm)), Some(current_slice));
                    state
                        .combined_waveform
                        .zoomed
                        .set_slicer_region(Some((start_norm, end_norm)), Some(current_slice));

                    // Set fixed buffer view mode (waveform moves, playhead stays centered)
                    state
                        .combined_waveform
                        .zoomed
                        .set_fixed_buffer_bounds(Some((buffer_start as u64, buffer_end as u64)));
                    state
                        .combined_waveform
                        .zoomed
                        .set_view_mode(ZoomedViewMode::FixedBuffer);
                    // Set zoom level based on slicer buffer size
                    let buffer_bars = self.domain.config().slicer.validated_buffer_bars();
                    state
                        .combined_waveform
                        .zoomed
                        .set_fixed_buffer_zoom(buffer_bars);

                    any_active = true;
                    break; // Only show overlay for first active stem
                }
            }

            // Clear slicer overlay and restore scrolling mode if no stems are active
            if !any_active {
                state
                    .combined_waveform
                    .overview
                    .set_slicer_region(None, None);
                state.combined_waveform.zoomed.set_slicer_region(None, None);
                state.combined_waveform.zoomed.set_fixed_buffer_bounds(None);
                state
                    .combined_waveform
                    .zoomed
                    .set_view_mode(ZoomedViewMode::Scrolling);
            }
        }

        // Poll import progress channel from domain - collect first to avoid borrow issues
        let progress_messages: Vec<_> = self
            .domain
            .import_progress_receiver()
            .map(|rx| {
                let mut msgs = Vec::new();
                while let Ok(progress) = rx.try_recv() {
                    msgs.push(progress);
                }
                msgs
            })
            .unwrap_or_default();

        // Process collected messages, accumulating returned Tasks
        let mut tasks: Vec<Task<Message>> = Vec::new();

        for progress in progress_messages {
            let task = self.update(Message::ImportProgressUpdate(progress));
            tasks.push(task);
        }

        // Poll re-analysis progress channel from domain (same pattern as import)
        let reanalysis_messages: Vec<_> = self
            .domain
            .reanalysis_progress_receiver()
            .map(|rx| {
                let mut msgs = Vec::new();
                while let Ok(progress) = rx.try_recv() {
                    msgs.push(progress);
                }
                msgs
            })
            .unwrap_or_default();

        // Process collected re-analysis messages
        for progress in reanalysis_messages {
            let task = self.update(Message::ReanalysisProgress(progress));
            tasks.push(task);
        }

        // Poll PCA build progress channel (collect first to avoid borrow conflict)
        let pca_messages: Vec<_> = self
            .pca_progress_rx
            .as_ref()
            .map(|rx| {
                let mut msgs = Vec::new();
                while let Ok(pair) = rx.try_recv() {
                    msgs.push(pair);
                }
                msgs
            })
            .unwrap_or_default();

        for (done, total) in pca_messages {
            let task = self.update(Message::SimilarityIndexProgress { done, total });
            tasks.push(task);
        }

        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }
}
