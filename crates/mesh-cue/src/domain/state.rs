//! Domain-level state for loaded tracks
//!
//! This module contains the metadata and editing state for tracks,
//! separated from UI concerns like waveform rendering.

use std::path::PathBuf;
use mesh_core::audio_file::{CuePoint, SavedLoop, StemLinkReference};
use mesh_core::db::Track;
use mesh_core::types::SAMPLE_RATE;

/// Domain-level state for a loaded track
///
/// This struct contains only the metadata and editing state for a track.
/// UI-specific state (waveforms, rendering) is kept in the UI layer.
#[derive(Debug, Clone)]
pub struct LoadedTrackState {
    // ═══════════════════════════════════════════════════════════════════════
    // Identity
    // ═══════════════════════════════════════════════════════════════════════

    /// Path to the track file
    pub path: PathBuf,

    /// Database track ID (if loaded from database)
    pub track_id: Option<i64>,

    /// Track display name
    pub name: String,

    /// Artist name
    pub artist: Option<String>,

    // ═══════════════════════════════════════════════════════════════════════
    // Editable Metadata
    // ═══════════════════════════════════════════════════════════════════════

    /// Current BPM (may be modified by user)
    pub bpm: f64,

    /// Original detected BPM (preserved for reference)
    pub original_bpm: f64,

    /// Musical key (e.g., "8A", "11B")
    pub key: String,

    /// Drop marker sample position (for linked stem alignment)
    pub drop_marker: Option<u64>,

    /// First beat sample position (for beat grid regeneration)
    pub first_beat_sample: u64,

    /// Integrated LUFS loudness
    pub lufs: Option<f32>,

    /// Duration in samples
    pub duration_samples: u64,

    // ═══════════════════════════════════════════════════════════════════════
    // Associated Data
    // ═══════════════════════════════════════════════════════════════════════

    /// Hot cue points (up to 8)
    pub cue_points: Vec<CuePoint>,

    /// Saved loops (up to 8)
    pub saved_loops: Vec<SavedLoop>,

    /// Stem links for prepared mode
    pub stem_links: Vec<StemLinkReference>,

    /// Beat grid (sample positions)
    pub beat_grid: Vec<u64>,
}

impl LoadedTrackState {
    /// Create from a database Track
    pub fn from_db_track(track: Track) -> Self {
        let bpm = track.bpm.unwrap_or(120.0);
        let first_beat_sample = track.first_beat_sample as u64;
        let duration_samples = (track.duration_seconds * SAMPLE_RATE as f64) as u64;

        // Generate beat grid from BPM and first beat
        let beat_grid = generate_beat_grid(bpm, first_beat_sample, duration_samples);

        // Convert database cue points to runtime format
        let cue_points: Vec<CuePoint> = track.cue_points.iter().map(|c| {
            CuePoint {
                index: c.index,
                sample_position: c.sample_position as u64,
                label: c.label.clone().unwrap_or_default(),
                color: c.color.clone(),
            }
        }).collect();

        // Convert database saved loops to runtime format
        let saved_loops: Vec<SavedLoop> = track.saved_loops.iter().map(|l| {
            SavedLoop {
                index: l.index,
                start_sample: l.start_sample as u64,
                end_sample: l.end_sample as u64,
                label: l.label.clone().unwrap_or_default(),
                color: l.color.clone(),
            }
        }).collect();

        // Note: stem_links are NOT converted here because converting from
        // database format (ID-based) to runtime format (path-based) requires
        // database lookups. Use MeshCueDomain::load_track_state() instead,
        // which handles the conversion properly.
        let stem_links = Vec::new();

        Self {
            path: track.path.clone(),
            track_id: track.id,
            name: track.name.clone(),
            artist: track.artist.clone(),
            bpm,
            original_bpm: track.original_bpm.unwrap_or(bpm),
            key: track.key.clone().unwrap_or_default(),
            drop_marker: track.drop_marker.map(|d| d as u64),
            first_beat_sample,
            lufs: track.lufs,
            duration_samples,
            cue_points,
            saved_loops,
            stem_links,
            beat_grid,
        }
    }

    /// Regenerate beat grid after BPM change
    pub fn regenerate_beat_grid(&mut self) {
        self.beat_grid = generate_beat_grid(self.bpm, self.first_beat_sample, self.duration_samples);
    }

    /// Get BPM as formatted string
    pub fn bpm_display(&self) -> String {
        format!("{:.1}", self.bpm)
    }

    /// Find the nearest beat to a sample position
    pub fn nearest_beat(&self, sample: u64) -> Option<u64> {
        if self.beat_grid.is_empty() {
            return None;
        }

        // Binary search for nearest beat
        match self.beat_grid.binary_search(&sample) {
            Ok(idx) => Some(self.beat_grid[idx]),
            Err(idx) => {
                if idx == 0 {
                    Some(self.beat_grid[0])
                } else if idx >= self.beat_grid.len() {
                    Some(self.beat_grid[self.beat_grid.len() - 1])
                } else {
                    // Compare distances to adjacent beats
                    let before = self.beat_grid[idx - 1];
                    let after = self.beat_grid[idx];
                    if sample - before < after - sample {
                        Some(before)
                    } else {
                        Some(after)
                    }
                }
            }
        }
    }

    /// Get beat index for a sample position
    pub fn beat_index(&self, sample: u64) -> Option<usize> {
        if self.beat_grid.is_empty() {
            return None;
        }

        match self.beat_grid.binary_search(&sample) {
            Ok(idx) => Some(idx),
            Err(idx) => {
                if idx > 0 { Some(idx - 1) } else { Some(0) }
            }
        }
    }

    /// Get samples per beat at current BPM
    pub fn samples_per_beat(&self) -> f64 {
        (SAMPLE_RATE as f64 * 60.0) / self.bpm
    }

    /// Convert sample position to beat number
    pub fn sample_to_beat(&self, sample: u64) -> f64 {
        if self.first_beat_sample > sample {
            return 0.0;
        }
        let samples_from_first_beat = sample - self.first_beat_sample;
        samples_from_first_beat as f64 / self.samples_per_beat()
    }

    /// Convert beat number to sample position
    pub fn beat_to_sample(&self, beat: f64) -> u64 {
        let samples_offset = (beat * self.samples_per_beat()) as u64;
        self.first_beat_sample + samples_offset
    }
}

/// Generate beat grid from BPM and first beat position
fn generate_beat_grid(bpm: f64, first_beat_sample: u64, duration_samples: u64) -> Vec<u64> {
    if bpm <= 0.0 {
        return Vec::new();
    }

    let samples_per_beat = (SAMPLE_RATE as f64 * 60.0) / bpm;
    let estimated_beats = (duration_samples as f64 / samples_per_beat) as usize + 10;

    let mut grid = Vec::with_capacity(estimated_beats);

    // Generate beats before first beat (for tracks that start mid-beat)
    let mut sample = first_beat_sample;
    while sample > samples_per_beat as u64 {
        sample -= samples_per_beat as u64;
    }

    // Generate all beats
    while sample < duration_samples {
        grid.push(sample);
        sample += samples_per_beat as u64;
    }

    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_beat_grid() {
        // 120 BPM = 24000 samples per beat at 48kHz
        let grid = generate_beat_grid(120.0, 0, 480000);
        assert!(!grid.is_empty());
        assert_eq!(grid[0], 0);
        // Should have ~20 beats in 10 seconds
        assert!(grid.len() >= 20);
    }

    #[test]
    fn test_nearest_beat() {
        let state = LoadedTrackState {
            path: PathBuf::from("/test.wav"),
            track_id: Some(1),
            name: "Test".to_string(),
            artist: None,
            bpm: 120.0,
            original_bpm: 120.0,
            key: "Am".to_string(),
            drop_marker: None,
            first_beat_sample: 0,
            lufs: None,
            duration_samples: 480000,
            cue_points: vec![],
            saved_loops: vec![],
            stem_links: vec![],
            beat_grid: vec![0, 24000, 48000, 72000],
        };

        // Exact match
        assert_eq!(state.nearest_beat(24000), Some(24000));

        // Closer to previous beat
        assert_eq!(state.nearest_beat(25000), Some(24000));

        // Closer to next beat
        assert_eq!(state.nearest_beat(45000), Some(48000));
    }
}
