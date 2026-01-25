//! Playback control message handlers
//!
//! Handles: Play, Pause, Stop, Seek, ToggleLoop, AdjustLoopLength, Cue, CueReleased, BeatJump

use iced::Task;
use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::utils::snap_to_nearest_beat;

impl MeshCueApp {
    /// Handle Play message
    pub fn handle_play(&mut self) -> Task<Message> {
        if self.collection.loaded_track.is_some() {
            self.audio.play();
        }
        // Clear pressed hot cue keys to prevent spurious release events
        self.pressed_hot_cue_keys.clear();
        Task::none()
    }

    /// Handle Pause message
    pub fn handle_pause(&mut self) -> Task<Message> {
        if self.collection.loaded_track.is_some() {
            self.audio.pause();
        }
        Task::none()
    }

    /// Handle Stop message
    pub fn handle_stop(&mut self) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            self.audio.pause();
            self.audio.seek(0);
            state.update_zoomed_waveform_cache(0);
        }
        Task::none()
    }

    /// Handle Seek message
    pub fn handle_seek(&mut self, position: f64) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            let seek_pos = (position * state.duration_samples as f64) as u64;
            self.audio.seek(seek_pos);
            state.combined_waveform.overview.set_position(position);
            state.update_zoomed_waveform_cache(seek_pos);
        }
        Task::none()
    }

    /// Handle ScratchStart message (enter vinyl-style scrubbing)
    pub fn handle_scratch_start(&mut self) -> Task<Message> {
        if self.collection.loaded_track.is_some() {
            self.audio.scratch_start();
        }
        Task::none()
    }

    /// Handle ScratchMove message (update scratch position)
    pub fn handle_scratch_move(&mut self, position: f64) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            let scratch_pos = (position * state.duration_samples as f64) as u64;
            self.audio.scratch_move(scratch_pos);
            state.combined_waveform.overview.set_position(position);
            state.update_zoomed_waveform_cache(scratch_pos);
        }
        Task::none()
    }

    /// Handle ScratchEnd message (exit vinyl-style scrubbing)
    pub fn handle_scratch_end(&mut self) -> Task<Message> {
        if self.collection.loaded_track.is_some() {
            self.audio.scratch_end();
        }
        Task::none()
    }

    /// Handle ToggleLoop message
    pub fn handle_toggle_loop(&mut self) -> Task<Message> {
        if self.collection.loaded_track.is_some() {
            self.audio.toggle_loop();
        }
        Task::none()
    }

    /// Handle AdjustLoopLength message
    pub fn handle_adjust_loop_length(&mut self, delta: i32) -> Task<Message> {
        if self.collection.loaded_track.is_some() {
            self.audio.adjust_loop_length(delta);
        }
        Task::none()
    }

    /// Handle Cue message (CDJ-style cue)
    ///
    /// Only works when stopped:
    /// - Set cue point at current position (snapped to beat)
    /// - Start preview playback
    pub fn handle_cue(&mut self) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            // Only act when stopped (not playing)
            if state.is_playing() {
                return Task::none();
            }

            // Snap to nearest beat using UI's current beat grid
            let current_pos = state.playhead_position();
            let snapped_pos = snap_to_nearest_beat(current_pos, &state.beat_grid);

            // Seek to snapped position, set cue point, and start preview
            self.audio.seek(snapped_pos);
            self.audio.set_cue_point();
            self.audio.play();

            // Update waveform and cue marker
            if state.duration_samples > 0 {
                let normalized = snapped_pos as f64 / state.duration_samples as f64;
                state.combined_waveform.overview.set_position(normalized);
                state.combined_waveform.overview.set_cue_position(Some(normalized));
            }
            state.update_zoomed_waveform_cache(snapped_pos);
        }
        Task::none()
    }

    /// Handle CueReleased message (CDJ-style cue release)
    ///
    /// Stop preview, return to cue point
    pub fn handle_cue_released(&mut self) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            let cue_pos = state.cue_point();
            self.audio.pause();
            self.audio.seek(cue_pos);

            // Update waveform
            if state.duration_samples > 0 {
                let normalized = cue_pos as f64 / state.duration_samples as f64;
                state.combined_waveform.overview.set_position(normalized);
            }
            state.update_zoomed_waveform_cache(cue_pos);
        }
        Task::none()
    }

    /// Handle BeatJump message
    pub fn handle_beat_jump(&mut self, beats: i32) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            if beats > 0 {
                self.audio.beat_jump_forward();
            } else {
                self.audio.beat_jump_backward();
            }
            // Position will be updated on next tick via atomics
            // Trigger waveform update
            let pos = state.playhead_position();
            if state.duration_samples > 0 {
                let normalized = pos as f64 / state.duration_samples as f64;
                state.combined_waveform.overview.set_position(normalized);
            }
            state.update_zoomed_waveform_cache(pos);
        }
        Task::none()
    }

    /// Handle SetOverviewGridBars message
    pub fn handle_set_overview_grid_bars(&mut self, bars: u32) -> Task<Message> {
        if let Some(ref mut state) = self.collection.loaded_track {
            state.combined_waveform.overview.set_grid_bars(bars);
        }
        Task::none()
    }
}
