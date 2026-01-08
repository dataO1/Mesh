//! Stem Slicer - Real-time audio remixing by rearranging slice playback order
//!
//! The slicer divides a configurable buffer window (1/4/8/16 bars) into 8 equal slices.
//! Users can rearrange playback order by pressing buttons 1-8, which queue slices
//! in the order pressed. The playhead remains locked to the deck's background playhead,
//! but the slicer remaps which slice content plays at each timing position.
//!
//! # Architecture
//!
//! The slicer operates after track samples are read but before the effect chain:
//! ```text
//! Track → Deck.process() → [SLICER] → EffectChain → TimeStretch → Mixer
//! ```
//!
//! # Queue Behavior
//!
//! The 8-slot queue determines playback order. Initially [0,1,2,3,4,5,6,7] for
//! normal playback. When a user presses button 3, slice index 3 is queued.
//! Two algorithms are supported:
//! - **FIFO Rotate**: Oldest entry removed, new slice added to end
//! - **Replace Current**: New slice replaces the currently playing position
//!
//! The queue persists until explicitly cleared (shift+slicer or mode switch).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use crate::types::{StereoBuffer, StereoSample};

/// Number of slices in the slicer buffer (always 8)
pub const SLICER_NUM_SLICES: usize = 8;

/// Default buffer size in bars
pub const SLICER_DEFAULT_BARS: u32 = 4;

/// Maximum buffer size for pre-allocation (16 bars at 200 BPM, 48kHz)
/// 16 bars * 4 beats/bar * 60/200 sec/beat * 48000 samples/sec ≈ 460800 samples
pub const SLICER_MAX_BUFFER_SAMPLES: usize = 500_000;

/// Queue replacement algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueAlgorithm {
    /// FIFO: oldest entry removed, new slice added to end
    #[default]
    FifoRotate,
    /// Replace: new slice replaces current playback position
    ReplaceCurrent,
}

/// Lock-free slicer state for UI access
///
/// This struct contains atomic fields that can be read by the UI thread
/// without acquiring a mutex lock. The audio thread writes to these atomics
/// whenever the slicer state changes.
pub struct SlicerAtomics {
    /// Whether the slicer is active
    pub active: AtomicBool,
    /// Current buffer start position in samples
    pub buffer_start: AtomicU64,
    /// Current buffer end position in samples
    pub buffer_end: AtomicU64,
    /// Current playback queue packed as 8 u8 values into a u64
    /// Each byte (0-7) holds a slice index (0-7)
    pub queue: AtomicU64,
    /// Current slice index being played (0-7)
    pub current_slice: AtomicU8,
}

impl SlicerAtomics {
    /// Create new atomic state with defaults
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            buffer_start: AtomicU64::new(0),
            buffer_end: AtomicU64::new(0),
            queue: AtomicU64::new(Self::pack_queue(&[0, 1, 2, 3, 4, 5, 6, 7])),
            current_slice: AtomicU8::new(0),
        }
    }

    /// Pack 8 u8 values into a single u64 for atomic storage
    #[inline]
    pub fn pack_queue(queue: &[u8; SLICER_NUM_SLICES]) -> u64 {
        let mut packed = 0u64;
        for (i, &val) in queue.iter().enumerate() {
            packed |= (val as u64) << (i * 8);
        }
        packed
    }

    /// Unpack a u64 into 8 u8 values
    #[inline]
    pub fn unpack_queue(packed: u64) -> [u8; SLICER_NUM_SLICES] {
        let mut queue = [0u8; SLICER_NUM_SLICES];
        for (i, val) in queue.iter_mut().enumerate() {
            *val = ((packed >> (i * 8)) & 0xFF) as u8;
        }
        queue
    }

    /// Check if slicer is active (lock-free)
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Get buffer start position (lock-free)
    #[inline]
    pub fn buffer_start(&self) -> u64 {
        self.buffer_start.load(Ordering::Relaxed)
    }

    /// Get buffer end position (lock-free)
    #[inline]
    pub fn buffer_end(&self) -> u64 {
        self.buffer_end.load(Ordering::Relaxed)
    }

    /// Get current slice index (lock-free)
    #[inline]
    pub fn current_slice(&self) -> u8 {
        self.current_slice.load(Ordering::Relaxed)
    }

    /// Get unpacked queue (lock-free)
    #[inline]
    pub fn queue(&self) -> [u8; SLICER_NUM_SLICES] {
        Self::unpack_queue(self.queue.load(Ordering::Relaxed))
    }
}

impl Default for SlicerAtomics {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-stem slicer state
///
/// Manages the slicer buffer window, playback queue, and audio processing
/// for a single stem. The slicer remaps sample positions through the queue
/// to create rearranged playback order.
pub struct SlicerState {
    /// Whether the slicer is enabled
    enabled: bool,
    /// Current buffer window start position in samples (snapped to bar boundary)
    buffer_start: usize,
    /// Current buffer window end position in samples
    buffer_end: usize,
    /// Samples per slice (buffer_length / 8)
    samples_per_slice: usize,
    /// Playback queue - indices 0-7 in each slot determine which slice plays
    queue: [u8; SLICER_NUM_SLICES],
    /// Queue write index for FIFO mode (cycles 0-7)
    queue_write_idx: usize,
    /// Buffer size in bars (4, 8, or 16)
    buffer_bars: u32,
    /// Queue algorithm for button presses
    queue_algorithm: QueueAlgorithm,
    /// Lock-free atomics for UI access
    atomics: Arc<SlicerAtomics>,
    /// Pre-allocated cache for the full slicer buffer window
    /// Enables backward slice jumps when queue reorders earlier slices to play later
    buffer_cache: Vec<StereoSample>,
    /// Whether the buffer cache contains valid data
    buffer_cache_valid: bool,
    /// Last playhead position for detecting buffer boundary crossings
    last_playhead: usize,
    /// First beat sample position from track's beat grid (for grid alignment)
    first_beat_sample: usize,
    /// Whether activation is pending (waiting for next beat boundary)
    pending_enable: bool,
    /// Last beat index for detecting beat boundary crossings
    last_beat_index: usize,
}

impl SlicerState {
    /// Create a new slicer state with default configuration
    pub fn new() -> Self {
        Self {
            enabled: false,
            buffer_start: 0,
            buffer_end: 0,
            samples_per_slice: 0,
            queue: [0, 1, 2, 3, 4, 5, 6, 7],
            queue_write_idx: 0,
            buffer_bars: SLICER_DEFAULT_BARS,
            queue_algorithm: QueueAlgorithm::default(),
            atomics: Arc::new(SlicerAtomics::new()),
            buffer_cache: vec![StereoSample::silence(); SLICER_MAX_BUFFER_SAMPLES],
            buffer_cache_valid: false,
            last_playhead: 0,
            first_beat_sample: 0,
            pending_enable: false,
            last_beat_index: 0,
        }
    }

    /// Get a reference to the atomics for UI access
    #[inline]
    pub fn atomics(&self) -> Arc<SlicerAtomics> {
        Arc::clone(&self.atomics)
    }

    /// Check if the slicer is enabled or pending activation
    ///
    /// Returns true if enabled OR if activation is pending (waiting for beat boundary).
    /// This ensures process() gets called to check for beat boundaries.
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled || self.pending_enable
    }

    /// Check if slicer is actively processing (not just pending)
    #[inline]
    pub fn is_active(&self) -> bool {
        self.enabled
    }

    /// Enable or disable the slicer
    ///
    /// When enabling, the slicer enters a pending state and will activate
    /// on the next beat boundary. This ensures slice boundaries align with beats.
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled && !self.enabled && !self.pending_enable {
            // Don't enable immediately - wait for next beat boundary
            self.pending_enable = true;
            log::info!(
                "slicer: PENDING (buffer_bars={}, algorithm={:?}) - will activate on next beat",
                self.buffer_bars,
                self.queue_algorithm
            );
        } else if !enabled {
            self.enabled = false;
            self.pending_enable = false;
            self.buffer_cache_valid = false;
            log::info!("slicer: DISABLED");
        }
        // UI shows pending state as "active" so user gets feedback
        self.atomics.active.store(enabled || self.pending_enable, Ordering::Relaxed);
    }

    /// Set the buffer size in bars (1, 4, 8, or 16)
    pub fn set_buffer_bars(&mut self, bars: u32) {
        self.buffer_bars = bars.clamp(1, 16);
        // Invalidate cache when buffer size changes
        self.buffer_cache_valid = false;
    }

    /// Get the current buffer size in bars
    #[inline]
    pub fn buffer_bars(&self) -> u32 {
        self.buffer_bars
    }

    /// Set the queue algorithm
    pub fn set_queue_algorithm(&mut self, algorithm: QueueAlgorithm) {
        self.queue_algorithm = algorithm;
    }

    /// Set the first beat sample position from track's beat grid
    ///
    /// This is used to align the slicer buffer to the track's beat grid
    /// rather than assuming beats start at sample 0.
    pub fn set_first_beat(&mut self, first_beat: usize) {
        self.first_beat_sample = first_beat;
        // Invalidate cache when grid changes
        self.buffer_cache_valid = false;
    }

    /// Queue a slice for playback (button press)
    ///
    /// Depending on the queue algorithm:
    /// - FIFO: Adds slice to the end, rotating out the oldest
    /// - Replace: Replaces the current slice position
    pub fn queue_slice(&mut self, slice_idx: usize) {
        if slice_idx >= SLICER_NUM_SLICES {
            return;
        }

        match self.queue_algorithm {
            QueueAlgorithm::FifoRotate => {
                // Shift all elements left by one, add new at the end
                for i in 0..SLICER_NUM_SLICES - 1 {
                    self.queue[i] = self.queue[i + 1];
                }
                self.queue[SLICER_NUM_SLICES - 1] = slice_idx as u8;
            }
            QueueAlgorithm::ReplaceCurrent => {
                // Replace at current write position, advance write index
                self.queue[self.queue_write_idx] = slice_idx as u8;
                self.queue_write_idx = (self.queue_write_idx + 1) % SLICER_NUM_SLICES;
            }
        }

        log::debug!(
            "slicer: queued slice {} -> queue=[{},{},{},{},{},{},{},{}]",
            slice_idx,
            self.queue[0], self.queue[1], self.queue[2], self.queue[3],
            self.queue[4], self.queue[5], self.queue[6], self.queue[7]
        );

        self.sync_atomics();
    }

    /// Reset the queue to default order [0, 1, 2, 3, 4, 5, 6, 7]
    pub fn reset_queue(&mut self) {
        log::debug!("slicer: queue reset to [0,1,2,3,4,5,6,7]");
        self.queue = [0, 1, 2, 3, 4, 5, 6, 7];
        self.queue_write_idx = 0;
        self.sync_atomics();
    }

    /// Update the buffer window based on the current playhead position
    ///
    /// Called when the playhead might have crossed a buffer boundary.
    /// The buffer window is aligned to the track's beat grid (using first_beat_sample).
    fn update_buffer_window(&mut self, playhead: usize, samples_per_beat: f64, duration_samples: usize) {
        if samples_per_beat <= 0.0 {
            return;
        }

        let samples_per_bar = (samples_per_beat * 4.0) as usize;
        let buffer_length_bars = self.buffer_bars as usize;
        let buffer_length_samples = samples_per_bar * buffer_length_bars;

        if buffer_length_samples == 0 || samples_per_bar == 0 {
            return;
        }

        // Check if playhead has crossed buffer boundary
        let needs_update = playhead >= self.buffer_end
            || playhead < self.buffer_start
            || self.buffer_end == 0;

        if needs_update {
            // Calculate position relative to beat grid (not absolute sample 0)
            let grid_relative = playhead.saturating_sub(self.first_beat_sample);

            // Find which bar we're in relative to the grid
            let current_bar = grid_relative / samples_per_bar;
            let aligned_bar = (current_bar / buffer_length_bars) * buffer_length_bars;

            // Buffer start is grid-aligned (add back first_beat offset)
            self.buffer_start = self.first_beat_sample + (aligned_bar * samples_per_bar);
            self.buffer_end = (self.buffer_start + buffer_length_samples).min(duration_samples);
            self.samples_per_slice = if self.buffer_end > self.buffer_start {
                (self.buffer_end - self.buffer_start) / SLICER_NUM_SLICES
            } else {
                0
            };

            // Invalidate cache - needs to be refilled
            self.buffer_cache_valid = false;

            log::debug!(
                "slicer: buffer window updated: start={}, end={}, slice_size={} samples ({:.1}ms)",
                self.buffer_start,
                self.buffer_end,
                self.samples_per_slice,
                self.samples_per_slice as f64 / 48.0 // Approximate ms at 48kHz
            );

            // Sync atomics
            self.atomics
                .buffer_start
                .store(self.buffer_start as u64, Ordering::Relaxed);
            self.atomics
                .buffer_end
                .store(self.buffer_end as u64, Ordering::Relaxed);
        }
    }

    /// Fill the buffer cache with samples from the track
    fn fill_buffer_cache(&mut self, track_stem_data: &[StereoSample]) {
        let buffer_length = self.buffer_end.saturating_sub(self.buffer_start);
        if buffer_length == 0 || self.buffer_start >= track_stem_data.len() {
            return;
        }

        let cache_end = (self.buffer_start + buffer_length).min(track_stem_data.len());
        let actual_length = cache_end - self.buffer_start;

        // Copy track data into cache
        for i in 0..actual_length.min(self.buffer_cache.len()) {
            self.buffer_cache[i] = track_stem_data[self.buffer_start + i];
        }

        // Fill remainder with silence if needed
        for i in actual_length..buffer_length.min(self.buffer_cache.len()) {
            self.buffer_cache[i] = StereoSample::silence();
        }

        self.buffer_cache_valid = true;

        log::debug!(
            "slicer: cache filled with {} samples from track (window: {}..{})",
            actual_length,
            self.buffer_start,
            cache_end
        );
    }

    /// Remap a position through the queue
    ///
    /// Given an original position within the buffer, returns the remapped position
    /// based on the queue order.
    #[inline]
    fn remap_position(&self, original_pos: usize) -> usize {
        if original_pos < self.buffer_start || original_pos >= self.buffer_end {
            return original_pos;
        }

        if self.samples_per_slice == 0 {
            return original_pos;
        }

        let relative = original_pos - self.buffer_start;
        let original_slice_idx = (relative / self.samples_per_slice).min(SLICER_NUM_SLICES - 1);
        let pos_within_slice = relative % self.samples_per_slice;

        // Look up remapped slice from queue
        let remapped_slice_idx = self.queue[original_slice_idx] as usize;

        // Calculate remapped position within cache (relative to buffer start)
        remapped_slice_idx * self.samples_per_slice + pos_within_slice
    }

    /// Calculate which slice index corresponds to a position
    #[inline]
    fn slice_for_position(&self, position: usize) -> u8 {
        if position < self.buffer_start || position >= self.buffer_end || self.samples_per_slice == 0 {
            return 0;
        }

        let relative = position - self.buffer_start;
        ((relative / self.samples_per_slice).min(SLICER_NUM_SLICES - 1)) as u8
    }

    /// Process audio through the slicer
    ///
    /// This modifies the input buffer in-place, remapping samples through the queue.
    /// The track_stem_data is used to fill the buffer cache when needed.
    pub fn process(
        &mut self,
        buffer: &mut StereoBuffer,
        playhead: usize,
        samples_per_beat: f64,
        track_stem_data: &[StereoSample],
        duration_samples: usize,
    ) {
        // Handle pending activation - wait for beat boundary
        if self.pending_enable && samples_per_beat > 0.0 {
            let grid_relative = playhead.saturating_sub(self.first_beat_sample);
            let current_beat = (grid_relative as f64 / samples_per_beat) as usize;

            if current_beat != self.last_beat_index {
                // Beat boundary crossed - activate now
                self.enabled = true;
                self.pending_enable = false;
                log::info!("slicer: ACTIVATED at beat {} boundary", current_beat);
            }
            self.last_beat_index = current_beat;
        }

        // Also track beat index when active (for consistent state)
        if self.enabled && samples_per_beat > 0.0 {
            let grid_relative = playhead.saturating_sub(self.first_beat_sample);
            self.last_beat_index = (grid_relative as f64 / samples_per_beat) as usize;
        }

        if !self.enabled {
            return;
        }

        // Update buffer window if playhead crossed boundary
        self.update_buffer_window(playhead, samples_per_beat, duration_samples);

        // Fill cache if invalid
        if !self.buffer_cache_valid {
            self.fill_buffer_cache(track_stem_data);
        }

        // Check if we're within the slicer buffer window
        let buffer_len = buffer.len();
        if buffer_len == 0 || self.samples_per_slice == 0 {
            return;
        }

        // Determine if any part of this buffer is within the slicer window
        let buffer_end_pos = playhead + buffer_len;
        if buffer_end_pos <= self.buffer_start || playhead >= self.buffer_end {
            // Entirely outside slicer window, pass through unchanged
            return;
        }

        // Process each sample through remapping
        let buffer_slice = buffer.as_mut_slice();
        for (i, sample) in buffer_slice.iter_mut().enumerate() {
            let original_pos = playhead + i;

            // Only remap positions within the slicer buffer window
            if original_pos >= self.buffer_start && original_pos < self.buffer_end {
                let cache_pos = self.remap_position(original_pos);
                if cache_pos < self.buffer_cache.len() {
                    *sample = self.buffer_cache[cache_pos];
                }
            }
            // Positions outside the window keep their original samples (already in buffer)
        }

        // Update current slice indicator for UI
        let current_slice = self.slice_for_position(playhead);
        let prev_slice = self.atomics.current_slice.load(Ordering::Relaxed);
        self.atomics
            .current_slice
            .store(current_slice, Ordering::Relaxed);

        // Log when slice changes (remapped slice = queue[current_slice])
        if current_slice != prev_slice {
            let remapped = self.queue[current_slice as usize];
            log::debug!(
                "slicer: slice {} -> playing content from slice {} (pos={})",
                current_slice,
                remapped,
                playhead
            );
        }

        self.last_playhead = playhead;
    }

    /// Sync internal state to atomics
    fn sync_atomics(&self) {
        self.atomics.active.store(self.enabled, Ordering::Relaxed);
        self.atomics
            .buffer_start
            .store(self.buffer_start as u64, Ordering::Relaxed);
        self.atomics
            .buffer_end
            .store(self.buffer_end as u64, Ordering::Relaxed);
        self.atomics
            .queue
            .store(SlicerAtomics::pack_queue(&self.queue), Ordering::Relaxed);
    }
}

impl Default for SlicerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_packing() {
        let queue = [0, 1, 2, 3, 4, 5, 6, 7];
        let packed = SlicerAtomics::pack_queue(&queue);
        let unpacked = SlicerAtomics::unpack_queue(packed);
        assert_eq!(queue, unpacked);

        let queue2 = [7, 6, 5, 4, 3, 2, 1, 0];
        let packed2 = SlicerAtomics::pack_queue(&queue2);
        let unpacked2 = SlicerAtomics::unpack_queue(packed2);
        assert_eq!(queue2, unpacked2);
    }

    #[test]
    fn test_queue_slice_fifo() {
        let mut slicer = SlicerState::new();
        slicer.set_queue_algorithm(QueueAlgorithm::FifoRotate);

        // Initial queue: [0, 1, 2, 3, 4, 5, 6, 7]
        assert_eq!(slicer.queue, [0, 1, 2, 3, 4, 5, 6, 7]);

        // Queue slice 3
        slicer.queue_slice(3);
        assert_eq!(slicer.queue, [1, 2, 3, 4, 5, 6, 7, 3]);

        // Queue slice 5
        slicer.queue_slice(5);
        assert_eq!(slicer.queue, [2, 3, 4, 5, 6, 7, 3, 5]);
    }

    #[test]
    fn test_queue_slice_replace() {
        let mut slicer = SlicerState::new();
        slicer.set_queue_algorithm(QueueAlgorithm::ReplaceCurrent);

        // Initial queue: [0, 1, 2, 3, 4, 5, 6, 7]
        assert_eq!(slicer.queue, [0, 1, 2, 3, 4, 5, 6, 7]);

        // Queue slice 3 at position 0
        slicer.queue_slice(3);
        assert_eq!(slicer.queue, [3, 1, 2, 3, 4, 5, 6, 7]);

        // Queue slice 5 at position 1
        slicer.queue_slice(5);
        assert_eq!(slicer.queue, [3, 5, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_reset_queue() {
        let mut slicer = SlicerState::new();
        slicer.queue_slice(3);
        slicer.queue_slice(5);
        slicer.queue_slice(1);

        slicer.reset_queue();
        assert_eq!(slicer.queue, [0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_remap_position() {
        let mut slicer = SlicerState::new();
        slicer.buffer_start = 0;
        slicer.buffer_end = 8000; // 8000 samples = 8 slices of 1000 samples each
        slicer.samples_per_slice = 1000;
        slicer.enabled = true;

        // Default queue [0,1,2,3,4,5,6,7] - no remapping
        assert_eq!(slicer.remap_position(0), 0);
        assert_eq!(slicer.remap_position(500), 500);
        assert_eq!(slicer.remap_position(1000), 1000);

        // Swap queue: slice 0 plays slice 1's content, slice 1 plays slice 0's content
        slicer.queue = [1, 0, 2, 3, 4, 5, 6, 7];

        // Position 0-999 (originally slice 0) should now play from slice 1 (1000-1999)
        assert_eq!(slicer.remap_position(0), 1000);
        assert_eq!(slicer.remap_position(500), 1500);
        assert_eq!(slicer.remap_position(999), 1999);

        // Position 1000-1999 (originally slice 1) should now play from slice 0 (0-999)
        assert_eq!(slicer.remap_position(1000), 0);
        assert_eq!(slicer.remap_position(1500), 500);
    }

    #[test]
    fn test_slice_for_position() {
        let mut slicer = SlicerState::new();
        slicer.buffer_start = 0;
        slicer.buffer_end = 8000;
        slicer.samples_per_slice = 1000;
        slicer.enabled = true;

        assert_eq!(slicer.slice_for_position(0), 0);
        assert_eq!(slicer.slice_for_position(999), 0);
        assert_eq!(slicer.slice_for_position(1000), 1);
        assert_eq!(slicer.slice_for_position(7000), 7);
        assert_eq!(slicer.slice_for_position(7999), 7);
    }
}
