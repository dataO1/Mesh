//! Priority region planning for streaming track loading.
//!
//! Computes which sample ranges to load first (around hot cues) so the DJ
//! can play from entry points while the rest of the audio loads progressively.
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

/// Maximum frames per gap sub-chunk. Large gaps are split into chunks of
/// this size so the work-stealing pool produces a steady stream of UI
/// updates instead of a few large bursts.
const GAP_CHUNK_FRAMES: usize = 480_000; // ~10 seconds at 48 kHz

/// Compute priority load regions from track metadata.
///
/// Only hot cue positions are prioritised. Creates `[cue - 32*spb, cue + 32*spb]`
/// regions (64 beats total), merges overlapping, and sorts by file position.
pub fn compute_priority_regions(
    metadata: &TrackMetadata,
    duration_samples: usize,
    sample_rate: u32,
) -> Vec<LoadRegion> {
    let bpm = metadata.bpm.unwrap_or(120.0);
    let spb = (sample_rate as f64 * 60.0 / bpm) as usize;
    let margin = 32 * spb; // 32 beats each side = 64 beats total

    // Only hot cue positions — track start and drop marker load with gaps
    let points: Vec<usize> = metadata
        .cue_points
        .iter()
        .map(|cue| cue.sample_position as usize)
        .collect();

    if points.is_empty() {
        return Vec::new();
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

/// Compute gap regions (everything NOT covered by priority regions),
/// split into sub-chunks of at most `GAP_CHUNK_FRAMES` for granular
/// progressive loading.
pub fn compute_gaps(priority: &[LoadRegion], duration_samples: usize) -> Vec<LoadRegion> {
    let mut gaps = Vec::new();
    let mut pos = 0;
    for r in priority {
        if pos < r.start {
            split_into_chunks(pos, r.start, &mut gaps);
        }
        pos = r.end;
    }
    if pos < duration_samples {
        split_into_chunks(pos, duration_samples, &mut gaps);
    }
    gaps
}

/// Split a range [start, end) into sub-chunks of at most GAP_CHUNK_FRAMES.
fn split_into_chunks(start: usize, end: usize, out: &mut Vec<LoadRegion>) {
    let mut pos = start;
    while pos < end {
        let chunk_end = (pos + GAP_CHUNK_FRAMES).min(end);
        out.push(LoadRegion {
            start: pos,
            end: chunk_end,
        });
        pos = chunk_end;
    }
}
