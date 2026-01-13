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
use std::thread;

use basedrop::Shared;

use crate::timestretch::TimeStretcher;
use crate::types::{StereoBuffer, StereoSample, NUM_STEMS, SAMPLE_RATE};

/// Maximum threads to use for parallel stretching
/// Keep low to avoid starving the audio thread
const MAX_STRETCH_THREADS: usize = 2;

/// Minimum segment size for parallel stretching (in samples)
/// Below this, single-threaded is faster due to overhead
const MIN_PARALLEL_SEGMENT: usize = 2_000_000; // ~40 seconds at 48kHz

/// Information about a linked stem from another track
#[derive(Clone)]
pub struct LinkedStemInfo {
    /// Pre-stretched buffer of the linked stem (at host track's BPM)
    /// Wrapped in Shared for zero-copy access from both audio engine and UI
    pub buffer: Shared<StereoBuffer>,

    /// Original BPM of the linked track (before stretching)
    pub original_bpm: f64,

    /// Drop marker position in linked track (samples at host BPM after stretching)
    pub drop_marker: u64,

    /// Track name for UI display
    pub track_name: String,

    /// Track path for prepared mode persistence
    pub track_path: Option<PathBuf>,

    /// Source track's integrated LUFS (from WAV file bext chunk)
    /// Used to calculate gain correction when mixing with host track
    pub lufs: Option<f32>,
}

impl LinkedStemInfo {
    /// Create new linked stem info
    pub fn new(
        buffer: Shared<StereoBuffer>,
        original_bpm: f64,
        drop_marker: u64,
        track_name: String,
        track_path: Option<PathBuf>,
        lufs: Option<f32>,
    ) -> Self {
        Self {
            buffer,
            original_bpm,
            drop_marker,
            track_name,
            track_path,
            lufs,
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

    /// Pre-computed gain correction for linked stem (linear multiplier)
    /// Calculated from: 10^((host_lufs - linked_lufs) / 20)
    /// Brings the linked stem to the host track's level before deck LUFS compensation
    pub gain: f32,

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
    ///
    /// Uses the cheaper time stretcher preset since linked stems are
    /// pre-stretched in the background where speed matters more than
    /// maximum quality.
    pub fn new_with_sample_rate(sample_rate: u32) -> Self {
        Self {
            linked: None,
            use_linked: false,
            gain: 1.0, // Unity gain (no correction)
            stretcher: TimeStretcher::new_cheaper(sample_rate),
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
        self.gain = 1.0; // Reset to unity gain
    }

    /// Calculate and update gain correction based on host and linked LUFS
    ///
    /// The gain brings the linked stem to the host track's level:
    /// - If linked is louder than host: gain < 1.0 (attenuate)
    /// - If linked is quieter than host: gain > 1.0 (boost)
    /// - If either LUFS is missing: gain = 1.0 (no correction)
    ///
    /// After this correction, the deck's overall `lufs_gain` will bring
    /// everything to the target LUFS level.
    pub fn update_gain(&mut self, host_lufs: Option<f32>) {
        self.gain = match (host_lufs, self.linked.as_ref().and_then(|l| l.lufs)) {
            (Some(host), Some(linked)) => {
                let gain = 10.0_f32.powf((host - linked) / 20.0);
                log::debug!(
                    "Linked stem gain: host={:.1} LUFS, linked={:.1} LUFS → gain={:.3} ({:+.1} dB)",
                    host, linked, gain, host - linked
                );
                gain
            }
            _ => 1.0, // No correction if either LUFS is missing
        };
    }

    /// Pre-stretch a source buffer to match target BPM
    ///
    /// This is called when:
    /// 1. A new stem link is created
    /// 2. The global/host BPM changes
    ///
    /// Returns the stretched buffer.
    ///
    /// Uses parallel processing - splits the buffer into segments and processes
    /// them concurrently across multiple CPU cores for maximum throughput.
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

        // Use parallel stretching for large buffers, but limit threads to avoid
        // starving the audio thread. Use std::thread instead of rayon to avoid
        // contention with rayon's global pool (which may be used for other work).
        if source.len() >= MIN_PARALLEL_SEGMENT && MAX_STRETCH_THREADS > 1 {
            return Self::pre_stretch_parallel(source, ratio);
        }

        // For smaller buffers, use single-threaded chunked processing
        Self::pre_stretch_chunked(&mut self.stretcher, source, ratio)
    }

    /// Parallel pre-stretching using dedicated threads (not rayon)
    ///
    /// Uses std::thread to avoid starving rayon's global pool and the audio thread.
    /// Limits parallelism to MAX_STRETCH_THREADS to leave CPU headroom.
    fn pre_stretch_parallel(source: &StereoBuffer, ratio: f64) -> StereoBuffer {
        let source_len = source.len();
        let total_output_len = ((source_len as f64) / ratio).ceil() as usize;

        // Use at most MAX_STRETCH_THREADS segments
        let num_segments = MAX_STRETCH_THREADS.min(source_len / MIN_PARALLEL_SEGMENT).max(1);

        if num_segments == 1 {
            // Fall back to single-threaded
            let mut stretcher = TimeStretcher::new_cheaper(SAMPLE_RATE);
            return Self::pre_stretch_chunked(&mut stretcher, source, ratio);
        }

        let segment_input_size = source_len / num_segments;

        // Clone source data for thread safety (each thread gets its own segment)
        let source_data: Vec<StereoSample> = source.as_slice().to_vec();

        // Spawn threads for each segment
        let handles: Vec<_> = (0..num_segments)
            .map(|i| {
                let start = i * segment_input_size;
                let end = if i == num_segments - 1 {
                    source_len
                } else {
                    (i + 1) * segment_input_size
                };

                // Clone segment data for this thread
                let segment_data: Vec<StereoSample> = source_data[start..end].to_vec();
                let segment_ratio = ratio;

                thread::spawn(move || {
                    let mut stretcher = TimeStretcher::new_cheaper(SAMPLE_RATE);
                    let segment = StereoBuffer::from_vec(segment_data);
                    Self::pre_stretch_chunked(&mut stretcher, &segment, segment_ratio)
                })
            })
            .collect();

        // Wait for all threads and collect results
        let stretched_segments: Vec<StereoBuffer> = handles
            .into_iter()
            .map(|h| h.join().expect("Stretch thread panicked"))
            .collect();

        // Combine segments (simple concatenation - no overlap needed since each
        // segment processes independently from clean boundaries)
        let mut output = StereoBuffer::silence(total_output_len);
        let output_slice = output.as_mut_slice();

        let mut output_pos = 0usize;
        for stretched in stretched_segments.iter() {
            let stretched_slice = stretched.as_slice();
            let copy_len = stretched_slice.len().min(total_output_len - output_pos);
            if copy_len > 0 {
                output_slice[output_pos..output_pos + copy_len]
                    .copy_from_slice(&stretched_slice[..copy_len]);
                output_pos += copy_len;
            }
        }

        output
    }

    /// Single-threaded chunked pre-stretching
    fn pre_stretch_chunked(
        stretcher: &mut TimeStretcher,
        source: &StereoBuffer,
        ratio: f64,
    ) -> StereoBuffer {
        const OUTPUT_CHUNK_SIZE: usize = 4096;

        let total_output_len = ((source.len() as f64) / ratio).ceil() as usize;
        let mut output = StereoBuffer::silence(total_output_len);

        stretcher.set_ratio(ratio);
        let source_slice = source.as_slice();

        // Pre-allocate workspace buffers
        let max_input_chunk = ((OUTPUT_CHUNK_SIZE as f64) * ratio * 2.0).ceil() as usize + 1;
        let mut input_workspace = StereoBuffer::with_capacity(max_input_chunk);
        let mut output_workspace = StereoBuffer::with_capacity(OUTPUT_CHUNK_SIZE);

        let mut input_pos = 0usize;
        let mut output_pos = 0usize;
        let mut fractional_input = 0.0f64;

        while output_pos < total_output_len && input_pos < source.len() {
            let output_chunk_len = OUTPUT_CHUNK_SIZE.min(total_output_len - output_pos);

            fractional_input += (output_chunk_len as f64) * ratio;
            let input_chunk_len = fractional_input.floor() as usize;
            fractional_input -= input_chunk_len as f64;

            let input_end = (input_pos + input_chunk_len).min(source.len());
            let actual_input_len = input_end - input_pos;

            if actual_input_len == 0 {
                break;
            }

            input_workspace.resize(actual_input_len);
            output_workspace.resize(output_chunk_len);

            input_workspace
                .as_mut_slice()
                .copy_from_slice(&source_slice[input_pos..input_end]);

            stretcher.process(&input_workspace, &mut output_workspace);

            let output_end = (output_pos + output_chunk_len).min(total_output_len);
            output.as_mut_slice()[output_pos..output_end]
                .copy_from_slice(&output_workspace.as_slice()[..output_end - output_pos]);

            input_pos = input_end;
            output_pos = output_end;
        }

        // Flush remaining samples
        if output_pos < total_output_len {
            let remaining = total_output_len - output_pos;
            let mut flush_buf = StereoBuffer::silence(remaining);
            stretcher.flush(&mut flush_buf);
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
    /// Wrapped in Shared for zero-copy access from both audio engine and UI
    pub buffer: Shared<StereoBuffer>,

    /// Original BPM of the source track
    pub original_bpm: f64,

    /// Drop marker position in samples (at stretched BPM)
    pub drop_marker: u64,

    /// Track name for display
    pub track_name: String,

    /// Track path for prepared mode persistence
    pub track_path: Option<PathBuf>,

    /// Source track's integrated LUFS (from WAV file bext chunk)
    /// Used to calculate gain correction when mixing with host track
    pub lufs: Option<f32>,
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
            self.lufs,
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
