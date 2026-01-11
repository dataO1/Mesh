//! Linked Stem Support - Hot-swappable stems from other tracks
//!
//! This module enables DJs to swap individual stems (vocals/drums/bass/other)
//! between tracks while maintaining both in memory for instant toggling.
//!
//! # Architecture
//!
//! Linked stems are injected into the deck processing pipeline BEFORE the slicer:
//! ```text
//! Track.stems → [LINKED STEM INJECTION] → Slicer → EffectChain → TimeStretch → Mixer
//! ```
//!
//! # BPM Synchronization
//!
//! When a stem is linked, it's pre-stretched to match the host track's BPM.
//! This means:
//! - No per-sample stretching during playback (real-time safe)
//! - Both buffers (original and linked) are ready at the same BPM
//! - Toggle is instant with no audible artifacts
//!
//! # Beat Alignment
//!
//! Linked stems use drop markers for structural alignment:
//! - Each track has a drop marker indicating a reference point (e.g., the drop)
//! - Position mapping: host_offset_from_drop = linked_offset_from_drop
//!
//! # Memory Model
//!
//! - `LinkedStemInfo`: Contains the pre-stretched buffer and metadata
//! - `StemLink`: Per-stem link state (one per stem slot in deck)
//! - `LinkedStemAtomics`: Lock-free state for UI access

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::timestretch::TimeStretcher;
use crate::types::{StereoBuffer, StereoSample, NUM_STEMS, SAMPLE_RATE};

/// Information about a linked stem from another track
#[derive(Clone)]
pub struct LinkedStemInfo {
    /// Pre-stretched buffer of the linked stem (at host track's BPM)
    pub buffer: StereoBuffer,

    /// Original BPM of the linked track (before stretching)
    pub original_bpm: f64,

    /// Drop marker position in linked track (samples at host BPM after stretching)
    pub drop_marker: u64,

    /// Track name for UI display
    pub track_name: String,

    /// Track path for prepared mode persistence
    pub track_path: Option<PathBuf>,
}

impl LinkedStemInfo {
    /// Create new linked stem info
    pub fn new(
        buffer: StereoBuffer,
        original_bpm: f64,
        drop_marker: u64,
        track_name: String,
        track_path: Option<PathBuf>,
    ) -> Self {
        Self {
            buffer,
            original_bpm,
            drop_marker,
            track_name,
            track_path,
        }
    }

    /// Get the length of the linked stem buffer in samples
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Per-stem link state for a deck
///
/// Each stem slot in a deck has a StemLink that can optionally hold
/// linked stem data from another track.
pub struct StemLink {
    /// The linked stem data (None if no link)
    pub linked: Option<LinkedStemInfo>,

    /// Whether to use the linked stem or original
    /// Only meaningful when `linked` is Some
    pub use_linked: bool,

    /// Time stretcher for pre-stretching linked stems
    /// Used when linking and when host BPM changes
    stretcher: TimeStretcher,
}

impl StemLink {
    /// Create a new stem link with default state
    pub fn new() -> Self {
        Self::new_with_sample_rate(SAMPLE_RATE)
    }

    /// Create a new stem link with specified sample rate
    pub fn new_with_sample_rate(sample_rate: u32) -> Self {
        Self {
            linked: None,
            use_linked: false,
            stretcher: TimeStretcher::new_with_sample_rate(sample_rate),
        }
    }

    /// Check if a linked stem exists
    pub fn has_linked(&self) -> bool {
        self.linked.is_some()
    }

    /// Check if the linked stem is currently active (being played)
    pub fn is_linked_active(&self) -> bool {
        self.use_linked && self.linked.is_some()
    }

    /// Toggle between original and linked stem
    /// Returns the new state (true = linked, false = original)
    pub fn toggle(&mut self) -> bool {
        if self.linked.is_some() {
            self.use_linked = !self.use_linked;
        }
        self.use_linked
    }

    /// Set linked stem data
    ///
    /// The provided buffer should already be pre-stretched to the host track's BPM.
    pub fn set_linked(&mut self, info: LinkedStemInfo) {
        self.linked = Some(info);
        // Don't auto-activate - user must toggle
        // self.use_linked = false;
    }

    /// Clear the linked stem
    pub fn clear(&mut self) {
        self.linked = None;
        self.use_linked = false;
    }

    /// Pre-stretch a source buffer to match target BPM
    ///
    /// This is called when:
    /// 1. A new stem link is created
    /// 2. The global/host BPM changes
    ///
    /// Returns the stretched buffer.
    ///
    /// Uses chunked processing (256-sample output chunks) to match how the global
    /// stretcher works during playback. This ensures identical quality between
    /// pre-stretched linked stems and normally-played tracks.
    pub fn pre_stretch(
        &mut self,
        source: &StereoBuffer,
        source_bpm: f64,
        target_bpm: f64,
    ) -> StereoBuffer {
        if source_bpm <= 0.0 || target_bpm <= 0.0 {
            return source.clone();
        }

        let ratio = target_bpm / source_bpm;
        if (ratio - 1.0).abs() < 0.001 {
            // No significant stretch needed
            return source.clone();
        }

        // Process in chunks like the global stretcher does (256-sample output chunks)
        // This matches the streaming behavior in engine.rs and produces identical quality
        const OUTPUT_CHUNK_SIZE: usize = 256;

        let total_output_len = ((source.len() as f64) / ratio).ceil() as usize;
        let mut output = StereoBuffer::silence(total_output_len);

        self.stretcher.set_ratio(ratio);

        let source_slice = source.as_slice();

        // Pre-allocate workspace buffers ONCE to avoid per-chunk allocation overhead.
        // Max input chunk size is OUTPUT_CHUNK_SIZE * ratio, with margin for fractional accumulation.
        let max_input_chunk = ((OUTPUT_CHUNK_SIZE as f64) * ratio * 2.0).ceil() as usize + 1;
        let mut input_workspace = StereoBuffer::with_capacity(max_input_chunk);
        let mut output_workspace = StereoBuffer::with_capacity(OUTPUT_CHUNK_SIZE);

        let mut input_pos = 0usize;
        let mut output_pos = 0usize;
        let mut fractional_input = 0.0f64;

        while output_pos < total_output_len && input_pos < source.len() {
            // Fixed output chunk size (or remainder)
            let output_chunk_len = OUTPUT_CHUNK_SIZE.min(total_output_len - output_pos);

            // Calculate input samples needed (matching deck.rs logic)
            // input_needed = output_chunk_len * ratio
            fractional_input += (output_chunk_len as f64) * ratio;
            let input_chunk_len = fractional_input.floor() as usize;
            fractional_input -= input_chunk_len as f64;

            let input_end = (input_pos + input_chunk_len).min(source.len());
            let actual_input_len = input_end - input_pos;

            if actual_input_len == 0 {
                break;
            }

            // Resize workspace buffers (no allocation - just adjusts working length within capacity)
            input_workspace.resize(actual_input_len);
            output_workspace.resize(output_chunk_len);

            // Copy input data into workspace
            input_workspace
                .as_mut_slice()
                .copy_from_slice(&source_slice[input_pos..input_end]);

            // Process chunk (ratio determined by size difference)
            self.stretcher.process(&input_workspace, &mut output_workspace);

            // Copy output data
            let output_end = (output_pos + output_chunk_len).min(total_output_len);
            output.as_mut_slice()[output_pos..output_end]
                .copy_from_slice(&output_workspace.as_slice()[..output_end - output_pos]);

            input_pos = input_end;
            output_pos = output_end;
        }

        // Flush any remaining samples from the stretcher
        if output_pos < total_output_len {
            let remaining = total_output_len - output_pos;
            let mut flush_buf = StereoBuffer::silence(remaining);
            self.stretcher.flush(&mut flush_buf);
            output.as_mut_slice()[output_pos..]
                .copy_from_slice(&flush_buf.as_slice()[..remaining]);
        }

        output
    }

    /// Re-stretch the existing linked buffer to a new BPM
    ///
    /// Called when the host track's BPM changes. This re-stretches from
    /// the original BPM to the new target BPM.
    pub fn re_stretch_to_bpm(&mut self, _new_target_bpm: f64) {
        if let Some(ref mut _linked) = self.linked {
            // We need to recalculate from original
            // Note: This is expensive - we should ideally keep the original unstretched buffer
            // For now, we'll note this limitation
            log::warn!(
                "re_stretch_to_bpm called but original buffer not preserved. \
                 Linked stem may have quality loss. Consider keeping original buffer."
            );
            // TODO: Store original unstretched buffer for quality re-stretching
        }
    }

    /// Get the stretcher's total latency in samples
    ///
    /// This latency is introduced during pre-stretching and shifts audio content
    /// forward in the output buffer. Must be compensated for in drop marker calculation.
    pub fn stretcher_latency(&self) -> usize {
        self.stretcher.total_latency()
    }
}

impl Default for StemLink {
    fn default() -> Self {
        Self::new()
    }
}

/// Lock-free linked stem state for UI access
///
/// This struct contains atomic fields that can be read by the UI thread
/// without acquiring a mutex lock. The audio thread writes to these atomics
/// whenever the linked stem state changes.
pub struct LinkedStemAtomics {
    /// Whether a linked stem exists [per stem]
    pub has_linked: [AtomicBool; NUM_STEMS],

    /// Whether the linked stem is currently active [per stem]
    pub use_linked: [AtomicBool; NUM_STEMS],

    /// Drop marker position of host track (for UI alignment display)
    pub host_drop_marker: AtomicU64,

    /// Drop marker position of each linked stem (for UI alignment display)
    pub linked_drop_marker: [AtomicU64; NUM_STEMS],
}

impl LinkedStemAtomics {
    /// Create new atomic state with defaults
    pub fn new() -> Self {
        Self {
            has_linked: [
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
            ],
            use_linked: [
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
            ],
            host_drop_marker: AtomicU64::new(0),
            linked_drop_marker: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    /// Update atomics from StemLink state
    pub fn sync_from_stem_link(&self, stem_idx: usize, link: &StemLink) {
        if stem_idx >= NUM_STEMS {
            return;
        }

        self.has_linked[stem_idx].store(link.has_linked(), Ordering::Relaxed);
        self.use_linked[stem_idx].store(link.use_linked, Ordering::Relaxed);

        if let Some(ref linked) = link.linked {
            self.linked_drop_marker[stem_idx].store(linked.drop_marker, Ordering::Relaxed);
        }
    }

    /// Set host drop marker
    pub fn set_host_drop_marker(&self, position: u64) {
        self.host_drop_marker.store(position, Ordering::Relaxed);
    }

    /// Check if any stem has a linked stem
    pub fn has_any_linked(&self) -> bool {
        self.has_linked
            .iter()
            .any(|a| a.load(Ordering::Relaxed))
    }

    /// Check if any linked stem is currently active
    pub fn has_any_active(&self) -> bool {
        (0..NUM_STEMS).any(|i| {
            self.has_linked[i].load(Ordering::Relaxed)
                && self.use_linked[i].load(Ordering::Relaxed)
        })
    }
}

impl Default for LinkedStemAtomics {
    fn default() -> Self {
        Self::new()
    }
}

// ────────────────────────────────────────────────────────────────────────────────
// Position Mapping
// ────────────────────────────────────────────────────────────────────────────────

/// Calculate linked stem read position from host position using drop marker alignment
///
/// When the host track's playhead is N samples from its drop marker,
/// the linked stem plays from N samples from its drop marker.
///
/// Returns None if the calculated position is outside the linked buffer bounds.
pub fn map_host_to_linked_position(
    host_position: usize,
    host_drop_marker: u64,
    linked_drop_marker: u64,
    linked_buffer_len: usize,
) -> Option<usize> {
    // Calculate offset from host's drop marker
    let offset_from_drop = host_position as i64 - host_drop_marker as i64;

    // Apply same offset to linked stem's drop marker
    let linked_position = linked_drop_marker as i64 + offset_from_drop;

    // Bounds check - return None if outside linked buffer
    if linked_position < 0 || linked_position as usize >= linked_buffer_len {
        return None;
    }

    Some(linked_position as usize)
}

/// Read samples from a linked stem buffer into an output buffer
///
/// Handles bounds checking and fills with silence when outside buffer range.
pub fn read_from_linked_buffer(
    linked_buffer: &StereoBuffer,
    start_position: usize,
    samples_to_read: usize,
    output: &mut [StereoSample],
) {
    let linked_len = linked_buffer.len();

    for (i, sample) in output.iter_mut().enumerate().take(samples_to_read) {
        let read_pos = start_position + i;
        if read_pos < linked_len {
            *sample = linked_buffer.as_slice()[read_pos];
        } else {
            // Outside buffer - silence
            *sample = StereoSample::silence();
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────────
// Data Transfer Types
// ────────────────────────────────────────────────────────────────────────────────

/// Data for creating a stem link (sent from background loading thread)
///
/// This struct is used to transfer linked stem data from the loader thread
/// to the audio engine via the command queue.
pub struct LinkedStemData {
    /// The stem audio buffer (pre-stretched to host BPM)
    pub buffer: StereoBuffer,

    /// Original BPM of the source track
    pub original_bpm: f64,

    /// Drop marker position in samples (at stretched BPM)
    pub drop_marker: u64,

    /// Track name for display
    pub track_name: String,

    /// Track path for prepared mode persistence
    pub track_path: Option<PathBuf>,
}

impl LinkedStemData {
    /// Convert to LinkedStemInfo
    pub fn into_info(self) -> LinkedStemInfo {
        LinkedStemInfo::new(
            self.buffer,
            self.original_bpm,
            self.drop_marker,
            self.track_name,
            self.track_path,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_mapping_at_drop() {
        // At the drop marker, both should be at their respective drops
        let pos = map_host_to_linked_position(1000, 1000, 500, 2000);
        assert_eq!(pos, Some(500));
    }

    #[test]
    fn test_position_mapping_before_drop() {
        // 200 samples before host drop should be 200 before linked drop
        let pos = map_host_to_linked_position(800, 1000, 500, 2000);
        assert_eq!(pos, Some(300));
    }

    #[test]
    fn test_position_mapping_after_drop() {
        // 300 samples after host drop should be 300 after linked drop
        let pos = map_host_to_linked_position(1300, 1000, 500, 2000);
        assert_eq!(pos, Some(800));
    }

    #[test]
    fn test_position_mapping_before_buffer_start() {
        // Position that maps to negative should return None
        let pos = map_host_to_linked_position(0, 1000, 500, 2000);
        assert_eq!(pos, None); // Would be -500
    }

    #[test]
    fn test_position_mapping_after_buffer_end() {
        // Position that maps past buffer end should return None
        let pos = map_host_to_linked_position(3000, 1000, 500, 2000);
        assert_eq!(pos, None); // Would be 2500, past buffer len 2000
    }

    #[test]
    fn test_stem_link_toggle() {
        let mut link = StemLink::new();

        // No linked stem - toggle does nothing
        assert!(!link.toggle());
        assert!(!link.use_linked);

        // Add linked stem
        let info = LinkedStemInfo::new(
            StereoBuffer::silence(1000),
            128.0,
            500,
            "Test".to_string(),
            None,
        );
        link.set_linked(info);

        // Now toggle works
        assert!(link.toggle()); // true = linked
        assert!(link.use_linked);
        assert!(!link.toggle()); // false = original
        assert!(!link.use_linked);
    }
}
