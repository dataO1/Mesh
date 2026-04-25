//! Track editing message handlers
//!
//! Handles: SetBpm, SetKey, AddCuePoint, DeleteCuePoint, SetCueLabel, SaveTrack, SaveComplete,
//! SetCuePoint, ClearCuePoint, JumpToCue, SaveLoop, JumpToSavedLoop, ClearSavedLoop,
//! SetDropMarker, ClearDropMarker

use iced::Task;
use mesh_core::audio_file::{CuePoint, SavedLoop};
use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::utils::{find_nearest_beat_with_index, regenerate_beat_grid, snap_to_nearest_beat, update_waveform_beat_grid};

impl MeshCueApp {
    /// Handle SetBpm message
    ///
    /// When BPM changes, the beat grid is recalculated anchored on the beat
    /// nearest to the current playhead. This preserves nudge adjustments —
    /// the beat you're listening to stays in place while other beats shift.
    pub fn handle_set_bpm(&mut self, bpm: f64) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            let old_bpm = state.bpm;
            state.bpm = bpm.clamp(1.0, 2500.0);

            // Regenerate beat grid anchored on the beat nearest to playhead
            if !state.beat_grid.is_empty() && state.duration_samples > 0 {
                let playhead = state.playhead_position();

                // The beat nearest to the playhead becomes the new anchor downbeat.
                let (_old_idx, anchor_pos) = find_nearest_beat_with_index(&state.beat_grid, playhead);

                log::debug!(
                    "BPM change {:.2} → {:.2}: anchor at {} (playhead {})",
                    old_bpm, bpm, anchor_pos, playhead
                );

                let (beats, anchor_idx) = regenerate_beat_grid(anchor_pos, bpm, state.duration_samples);
                state.beat_grid = beats;
                update_waveform_beat_grid(state, anchor_idx);

                // Propagate to deck so snapping uses updated grid
                self.audio.set_beat_grid(state.beat_grid.clone());
            }

            state.modified = true;
        }
        Task::none()
    }

    /// Handle BPM adjustment (+/- delta)
    pub fn handle_adjust_bpm(&mut self, delta: f64) -> Task<Message> {
        if let Some(ref state) = self.collection.loaded_track {
            let new_bpm = (state.bpm + delta).clamp(1.0, 2500.0);
            return self.handle_set_bpm(new_bpm);
        }
        Task::none()
    }

    /// Handle TapTempo — record a tap and compute BPM from average interval.
    ///
    /// BPM is rounded to the nearest integer. After each tap the beat grid
    /// downbeat is aligned to the current playhead (= tap position) via
    /// `handle_align_beat_grid_to_playhead`, so tapping on the "1" automatically
    /// sets both BPM and phase. Resets history on a gap > 3 seconds.
    /// Requires ≥ 2 taps; uses up to 8 recent taps for averaging.
    pub fn handle_tap_tempo(&mut self) -> Task<Message> {
        if self.collection.loaded_track.is_none() {
            return Task::none();
        }

        let now = std::time::Instant::now();

        // Reset if idle > 3 seconds between taps
        if let Some(&last) = self.tap_tempo_times.last() {
            if now.duration_since(last).as_secs_f64() > 3.0 {
                self.tap_tempo_times.clear();
            }
        }

        self.tap_tempo_times.push(now);

        // Keep only the last 8 taps for a rolling average
        if self.tap_tempo_times.len() > 8 {
            self.tap_tempo_times.drain(..self.tap_tempo_times.len() - 8);
        }

        if self.tap_tempo_times.len() < 2 {
            return Task::none();
        }

        let first = *self.tap_tempo_times.first().unwrap();
        let last = *self.tap_tempo_times.last().unwrap();
        let total_secs = last.duration_since(first).as_secs_f64();
        let avg_interval = total_secs / (self.tap_tempo_times.len() - 1) as f64;

        if avg_interval <= 0.0 {
            return Task::none();
        }

        // Round to nearest integer BPM
        let bpm = (60.0 / avg_interval).round().clamp(20.0, 2500.0);

        // Write BPM to state so handle_align_beat_grid_to_playhead picks it up
        if let Some(ref mut state) = self.collection.loaded_track {
            state.bpm = bpm;
            state.modified = true;
        }

        // Align beat grid downbeat to current tap position (reuses existing logic)
        self.handle_align_beat_grid_to_playhead()
    }

    /// Handle SetKey message
    pub fn handle_set_key(&mut self, key: String) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            state.key = key;
            state.modified = true;
        }
        Task::none()
    }

    /// Handle AddCuePoint message
    pub fn handle_add_cue_point(&mut self, position: u64) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            let index = state.cue_points.len() as u8;
            state.cue_points.push(CuePoint {
                index,
                sample_position: position,
                label: format!("Cue {}", index + 1),
                color: None,
            });
            state.modified = true;
        }
        Task::none()
    }

    /// Handle DeleteCuePoint message
    pub fn handle_delete_cue_point(&mut self, index: usize) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            if index < state.cue_points.len() {
                state.cue_points.remove(index);
                state.modified = true;
            }
        }
        Task::none()
    }

    /// Handle SetCueLabel message
    pub fn handle_set_cue_label(&mut self, index: usize, label: String) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            if let Some(cue) = state.cue_points.get_mut(index) {
                cue.label = label;
                state.modified = true;
            }
        }
        Task::none()
    }

    /// Handle SaveTrack message
    pub fn handle_save_track(&mut self) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            let result = self.domain.save_track_metadata(
                &state.path,
                state.bpm,
                &state.key,
                state.drop_marker,
                state.first_beat_sample,
                &state.cue_points,
                &state.saved_loops,
                &state.stem_links,
            );

            match result {
                Ok(_) => {
                    state.modified = false;
                    log::info!("Track saved to database: {:?}", state.path);
                }
                Err(e) => {
                    log::error!("Failed to save track: {:?}", e);
                }
            }
        }
        Task::none()
    }

    /// Handle SaveComplete message
    pub fn handle_save_complete(&mut self, result: Result<(), String>) -> Task<Message> {
        match result {
            Ok(()) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.modified = false;
                }
                log::info!("Track saved successfully");
            }
            Err(e) => {
                log::error!("Failed to save track: {}", e);
            }
        }
        Task::none()
    }

    /// Handle SetCuePoint message (hot cue)
    ///
    /// Snap to beat using UI's current beat grid
    pub fn handle_set_cue_point(&mut self, index: usize) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            let current_pos = state.playhead_position();
            let snapped_pos = snap_to_nearest_beat(current_pos, &state.beat_grid);

            // Check if a cue already exists near this position (within ~100ms tolerance)
            // This prevents duplicate cues at the same beat position
            const DUPLICATE_TOLERANCE: u64 = 4410; // ~100ms at 44.1kHz
            let duplicate_exists = state.cue_points.iter().any(|c| {
                // Skip checking the slot we're about to overwrite
                if c.index == index as u8 {
                    return false;
                }
                (c.sample_position as i64 - snapped_pos as i64).unsigned_abs()
                    < DUPLICATE_TOLERANCE
            });

            if duplicate_exists {
                log::debug!(
                    "Skipping hot cue {} at position {}: duplicate exists nearby",
                    index + 1,
                    snapped_pos
                );
                return Task::none();
            }

            // Store in cue_points (metadata)
            state.cue_points.retain(|c| c.index != index as u8);
            state.cue_points.push(CuePoint {
                index: index as u8,
                sample_position: snapped_pos,
                label: format!("Cue {}", index + 1),
                color: None,
            });
            state.cue_points.sort_by_key(|c| c.index);

            // Sync to deck so hot cue playback uses updated position immediately
            self.audio.set_hot_cue(index, snapped_pos as usize);

            // Update waveform markers (both overview and zoomed)
            state.combined_waveform.overview.update_cue_markers(&state.cue_points);
            state.combined_waveform.zoomed.update_cue_markers(&state.cue_points);
            state.modified = true;
        }
        Task::none()
    }

    /// Handle ClearCuePoint message
    pub fn handle_clear_cue_point(&mut self, index: usize) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            self.audio.clear_hot_cue(index);
            state.cue_points.retain(|c| c.index != index as u8);
            state.combined_waveform.overview.update_cue_markers(&state.cue_points);
            state.combined_waveform.zoomed.update_cue_markers(&state.cue_points);
            state.modified = true;
        }
        Task::none()
    }

    /// Handle JumpToCue message
    pub fn handle_jump_to_cue(&mut self, index: usize) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            if let Some(cue) = state.cue_points.iter().find(|c| c.index == index as u8) {
                let pos = cue.sample_position;
                self.audio.seek(pos);

                // Update waveform
                if state.duration_samples > 0 {
                    let normalized = pos as f64 / state.duration_samples as f64;
                    state.combined_waveform.overview.set_position(normalized);
                }
                state.update_zoomed_waveform_cache(pos);
            }
        }
        Task::none()
    }

    /// Handle SaveLoop message
    pub fn handle_save_loop(&mut self, index: usize) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            // Only save if loop is active (read from atomics)
            if state.is_loop_active() {
                let (start, end) = state.loop_bounds();
                let saved_loop = SavedLoop {
                    index: index as u8,
                    start_sample: start,
                    end_sample: end,
                    label: String::new(),
                    color: None,
                };
                // Remove any existing loop at this index
                state.saved_loops.retain(|l| l.index != index as u8);
                state.saved_loops.push(saved_loop);
                state.modified = true;
                log::info!("Saved loop {} at {} - {} samples", index, start, end);
            }
        }
        Task::none()
    }

    /// Handle JumpToSavedLoop message
    pub fn handle_jump_to_saved_loop(&mut self, index: usize) -> Task<Message> {
        // Shift+click = clear loop
        if self.shift_held {
            return self.handle_clear_saved_loop(index);
        }

        if let Some(ref mut state) = self.collection.loaded_track {
            let loop_data = state.saved_loops.iter().find(|l| l.index == index as u8).cloned();

            if let Some(saved_loop) = loop_data {
                // Seek to loop start and activate loop via toggle
                self.audio.seek(saved_loop.start_sample);
                self.audio.toggle_loop();

                // Update waveform positions
                if state.duration_samples > 0 {
                    let normalized = saved_loop.start_sample as f64 / state.duration_samples as f64;
                    state.combined_waveform.overview.set_position(normalized);
                }
                state.update_zoomed_waveform_cache(saved_loop.start_sample);

                log::info!("Jumped to saved loop {} at {} - {}", index, saved_loop.start_sample, saved_loop.end_sample);
            }
        }
        Task::none()
    }

    /// Handle ClearSavedLoop message
    pub fn handle_clear_saved_loop(&mut self, index: usize) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            state.saved_loops.retain(|l| l.index != index as u8);
            state.modified = true;
            log::info!("Cleared saved loop {}", index);
        }
        Task::none()
    }

    /// Handle SetDropMarker message
    pub fn handle_set_drop_marker(&mut self) -> Task<Message> {
        // Shift+click = clear drop marker
        if self.shift_held {
            return self.handle_clear_drop_marker();
        }

        if let Some(ref mut state) = self.collection.loaded_track {
            let position = state.playhead_position();
            state.drop_marker = Some(position);
            state.modified = true;
            log::info!("Set drop marker at sample {}", position);

            // Update waveform with new drop marker
            state.combined_waveform.overview.set_drop_marker(Some(position));
            state.combined_waveform.zoomed.set_drop_marker(Some(position));
        }
        Task::none()
    }

    /// Handle ClearDropMarker message
    pub fn handle_clear_drop_marker(&mut self) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            state.drop_marker = None;
            state.modified = true;
            log::info!("Cleared drop marker");

            // Update waveform
            state.combined_waveform.overview.set_drop_marker(None);
            state.combined_waveform.zoomed.set_drop_marker(None);
        }
        Task::none()
    }

    /// Handle SetZoomBars message
    pub fn handle_set_zoom_bars(&mut self, bars: u32) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            state.combined_waveform.zoomed.set_zoom(bars);
        }
        Task::none()
    }

    /// Handle NudgeBeatGridLeft message
    pub fn handle_nudge_beat_grid_left(&mut self) -> Task<Message> {
        use super::super::utils::{nudge_beat_grid, BEAT_GRID_NUDGE_SAMPLES};
        if let Some(ref mut state) = self.collection.loaded_track {
            nudge_beat_grid(state, -(BEAT_GRID_NUDGE_SAMPLES as i64));
            self.audio.set_beat_grid(state.beat_grid.clone());
        }
        Task::none()
    }

    /// Handle NudgeBeatGridRight message
    pub fn handle_nudge_beat_grid_right(&mut self) -> Task<Message> {
        use super::super::utils::{nudge_beat_grid, BEAT_GRID_NUDGE_SAMPLES};
        if let Some(ref mut state) = self.collection.loaded_track {
            nudge_beat_grid(state, BEAT_GRID_NUDGE_SAMPLES as i64);
            self.audio.set_beat_grid(state.beat_grid.clone());
        }
        Task::none()
    }

    /// Handle AlignBeatGridToPlayhead message
    ///
    /// Aligns the beat grid so that a downbeat falls on the current playhead position.
    /// The first beat is placed at the start of the file such that with the current BPM,
    /// a beat aligns exactly with the playhead. This allows DJs to scrub to where they
    /// hear the downbeat and align the grid to it, with beats extending from the beginning.
    pub fn handle_align_beat_grid_to_playhead(&mut self) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            if state.bpm <= 0.0 || state.duration_samples == 0 {
                return Task::none();
            }

            let playhead = state.playhead_position();

            // The playhead itself is the new anchor downbeat. regenerate_beat_grid
            // backfills beats before the anchor so beat-jump and snap reach the
            // start of the track. The shader uses beat_anchor_idx to align red and
            // phrase markers to this anchor regardless of grid_bars.
            log::debug!(
                "Aligning beat grid: anchor = playhead {} (BPM: {:.2})",
                playhead, state.bpm
            );

            let (beats, anchor_idx) = regenerate_beat_grid(playhead, state.bpm, state.duration_samples);
            state.beat_grid = beats;
            update_waveform_beat_grid(state, anchor_idx);

            // Propagate to deck so snapping uses updated grid
            self.audio.set_beat_grid(state.beat_grid.clone());

            state.modified = true;
        }
        Task::none()
    }
}
