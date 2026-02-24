//! Priority region planning for streaming track loading.
//!
//! Computes which sample ranges to load first (around hot cues, drop markers,
//! and the track start) so the DJ can play from entry points while the
//! rest of the audio loads in the background.
//!
//! Copied from mesh-player's loader/regions.rs — these are pure functions
//! that depend only on TrackMetadata.

use mesh_core::audio_file::TrackMetadata;

/// A contiguous sample range to load.
pub struct LoadRegion {
    /// Start sample (inclusive)
    pub start: usize,
    /// End sample (exclusive)
    pub end: usize,
}

impl LoadRegion {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

/// Compute priority load regions from track metadata.
///
/// Collects first_beat + all hot cue positions + drop marker, creates
/// `[point - 64*spb, point + 64*spb]` regions, merges overlapping, and
/// sorts by file position for sequential I/O.
pub fn compute_priority_regions(
    metadata: &TrackMetadata,
    duration_samples: usize,
    sample_rate: u32,
) -> Vec<LoadRegion> {
    let bpm = metadata.bpm.unwrap_or(120.0);
    let spb = (sample_rate as f64 * 60.0 / bpm) as usize;
    let margin = 64 * spb;

    let mut points: Vec<usize> = Vec::new();

    // Always include the start of the track (first beat or position 0)
    if let Some(fb) = metadata.beat_grid.first_beat_sample {
        points.push(fb as usize);
    } else {
        points.push(0);
    }

    // Include all hot cue positions
    for cue in &metadata.cue_points {
        points.push(cue.sample_position as usize);
    }

    // Include drop marker if set
    if let Some(dm) = metadata.drop_marker {
        points.push(dm as usize);
    }

    // Create regions around each point, clamp to track bounds
    let mut regions: Vec<LoadRegion> = points
        .iter()
        .map(|&p| LoadRegion {
            start: p.saturating_sub(margin),
            end: (p + margin).min(duration_samples),
        })
        .collect();

    // Sort by start position for sequential I/O
    regions.sort_by_key(|r| r.start);

    // Merge overlapping regions
    let mut merged: Vec<LoadRegion> = Vec::new();
    for r in regions {
        if let Some(last) = merged.last_mut() {
            if r.start <= last.end {
                last.end = last.end.max(r.end);
                continue;
            }
        }
        merged.push(r);
    }
    merged
}

/// Compute gap regions (everything NOT covered by priority regions).
///
/// These are loaded after priority regions are sent to the engine.
pub fn compute_gaps(priority: &[LoadRegion], duration_samples: usize) -> Vec<LoadRegion> {
    let mut gaps = Vec::new();
    let mut pos = 0;
    for r in priority {
        if pos < r.start {
            gaps.push(LoadRegion {
                start: pos,
                end: r.start,
            });
        }
        pos = r.end;
    }
    if pos < duration_samples {
        gaps.push(LoadRegion {
            start: pos,
            end: duration_samples,
        });
    }
    gaps
}
