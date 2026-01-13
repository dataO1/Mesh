//! Stem Slicer - Real-time audio remixing by rearranging slice playback order
//!
//! The slicer divides a configurable buffer window (1/4/8/16 bars) into 16 equal slices.
//! Users can rearrange playback order via manual triggering (slices 0-7) or by loading
//! one of 8 preset patterns. The playhead remains locked to the deck's background playhead,
//! but the slicer remaps which slice content plays at each timing position.
//!
//! # Architecture
//!
//! The slicer operates after track samples are read but before the effect chain:
//! ```text
//! Track → Deck.process() → [SLICER] → EffectChain → TimeStretch → Mixer
//! ```
//!
//! # Modes
//!
//! - **Manual mode**: Buttons 0-7 trigger individual slices, pattern is editable
//! - **Preset mode**: Pattern loaded from config presets, individual triggers disabled
//!
//! # Queue Behavior
//!
//! The 16-slot queue determines playback order. Initially [0,1,2...15] for
//! normal playback. Presets provide pre-defined 16-step patterns.
//! Two algorithms are supported for manual triggering:
//! - **FIFO Rotate**: Oldest entry removed, new slice added to end
//! - **Replace Current**: New slice replaces the currently playing position
//!
//! The queue persists until explicitly cleared (shift+slicer).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use crate::types::{StereoBuffer, StereoSample};

/// Number of slices in the slicer buffer (always 16)
pub const SLICER_NUM_SLICES: usize = 16;

/// Default buffer size in bars
pub const SLICER_DEFAULT_BARS: u32 = 4;

/// Maximum buffer size for pre-allocation (16 bars at 200 BPM, 48kHz)
/// 16 bars * 4 beats/bar * 60/200 sec/beat * 48000 samples/sec ≈ 460800 samples
pub const SLICER_MAX_BUFFER_SAMPLES: usize = 500_000;

/// Maximum number of simultaneous slices per step (polyphonic layers)
pub const MAX_SLICE_LAYERS: usize = 2;

/// Special slice value indicating muted/silent step
pub const MUTED_SLICE: u8 = 255;

/// Number of stems
pub const NUM_STEMS: usize = 4;

// =============================================================================
// Step and Sequence Types
// =============================================================================

/// A single step in the slicer sequence
///
/// Each step can play up to MAX_SLICE_LAYERS slices simultaneously (polyphonic),
/// with independent velocity control per layer. This enables:
/// - Ghost notes (low velocity hits)
/// - Layered sounds (kick + hi-hat on same step)
/// - Muted steps for rhythmic gaps (with release fade)
#[derive(Debug, Clone, Copy)]
pub struct SliceStep {
    /// Slice indices for each layer (MUTED_SLICE = muted/unused)
    /// Layer 0 is primary, additional layers are optional polyphonic overlays
    pub slices: [u8; MAX_SLICE_LAYERS],
    /// Velocity for each layer (0.0-1.0, linear amplitude multiplier)
    /// 1.0 = full velocity, 0.0 = silent
    pub velocities: [f32; MAX_SLICE_LAYERS],
}

impl Default for SliceStep {
    fn default() -> Self {
        Self {
            slices: [MUTED_SLICE; MAX_SLICE_LAYERS],
            velocities: [0.0; MAX_SLICE_LAYERS],
        }
    }
}

impl SliceStep {
    /// Create a simple step with a single slice at full velocity
    pub fn single(slice: u8) -> Self {
        Self {
            slices: [slice, MUTED_SLICE],
            velocities: [1.0, 0.0],
        }
    }

    /// Create a step with a single slice at specified velocity
    pub fn with_velocity(slice: u8, velocity: f32) -> Self {
        Self {
            slices: [slice, MUTED_SLICE],
            velocities: [velocity, 0.0],
        }
    }

    /// Create a muted step (silence)
    pub fn muted() -> Self {
        Self::default()
    }

    /// Check if this step is completely muted (no audible layers)
    #[inline]
    pub fn is_muted(&self) -> bool {
        for i in 0..MAX_SLICE_LAYERS {
            if self.slices[i] != MUTED_SLICE && self.velocities[i] > 0.0 {
                return false;
            }
        }
        true
    }
}

/// 16-step pattern for a single stem
///
/// Contains all step information including layers and velocities.
/// Used both for runtime state and preset storage.
#[derive(Debug, Clone)]
pub struct StepSequence {
    /// The 16 steps in this sequence
    pub steps: [SliceStep; SLICER_NUM_SLICES],
}

impl Default for StepSequence {
    fn default() -> Self {
        Self::sequential()
    }
}

impl StepSequence {
    /// Create a sequential sequence [0,1,2,...,15] at full velocity
    pub fn sequential() -> Self {
        let mut steps = [SliceStep::default(); SLICER_NUM_SLICES];
        for i in 0..SLICER_NUM_SLICES {
            steps[i] = SliceStep::single(i as u8);
        }
        Self { steps }
    }

    /// Create from a simple slice array (legacy format, full velocity)
    pub fn from_slice_array(slices: &[u8; SLICER_NUM_SLICES]) -> Self {
        let mut steps = [SliceStep::default(); SLICER_NUM_SLICES];
        for i in 0..SLICER_NUM_SLICES {
            steps[i] = SliceStep::single(slices[i]);
        }
        Self { steps }
    }

    /// Get slice indices as simple array (for atomics packing)
    pub fn to_slice_array(&self) -> [u8; SLICER_NUM_SLICES] {
        let mut result = [0u8; SLICER_NUM_SLICES];
        for i in 0..SLICER_NUM_SLICES {
            result[i] = self.steps[i].slices[0];
        }
        result
    }
}

/// A slicer preset that defines patterns for all stems
///
/// When loaded via preset button, each stem gets its own pattern (or bypass).
/// This enables coherent drum+bass combos in a single preset button.
///
/// Example: Preset 1 might have:
/// - Drums: half-time pattern with ghost notes
/// - Bass: complementary bass line
/// - Vocals: None (bypass - plays original)
/// - Other: None (bypass)
#[derive(Debug, Clone)]
pub struct SlicerPreset {
    /// Per-stem patterns (None = bypass slicer for this stem)
    /// Index order: [Vocals, Drums, Bass, Other]
    pub stems: [Option<StepSequence>; NUM_STEMS],
}

impl Default for SlicerPreset {
    fn default() -> Self {
        Self {
            stems: [None, None, None, None],
        }
    }
}

impl SlicerPreset {
    /// Create a preset that applies the same pattern to specified stems
    pub fn for_stems(sequence: StepSequence, stem_mask: [bool; NUM_STEMS]) -> Self {
        Self {
            stems: [
                if stem_mask[0] { Some(sequence.clone()) } else { None },
                if stem_mask[1] { Some(sequence.clone()) } else { None },
                if stem_mask[2] { Some(sequence.clone()) } else { None },
                if stem_mask[3] { Some(sequence) } else { None },
            ],
        }
    }

    /// Create from legacy format: simple slice array applied to drums only
    pub fn drums_only(slices: &[u8; SLICER_NUM_SLICES]) -> Self {
        Self {
            stems: [
                None,
                Some(StepSequence::from_slice_array(slices)),
                None,
                None,
            ],
        }
    }
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
    /// Current playback queue packed as 16 u8 values into two u64s
    /// queue_low holds steps 0-7, queue_high holds steps 8-15
    pub queue_low: AtomicU64,
    pub queue_high: AtomicU64,
    /// Current slice index being played (0-15)
    pub current_slice: AtomicU8,
}

impl SlicerAtomics {
    /// Create new atomic state with defaults
    pub fn new() -> Self {
        let (low, high) = Self::pack_queue(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        Self {
            active: AtomicBool::new(false),
            buffer_start: AtomicU64::new(0),
            buffer_end: AtomicU64::new(0),
            queue_low: AtomicU64::new(low),
            queue_high: AtomicU64::new(high),
            current_slice: AtomicU8::new(0),
        }
    }

    /// Pack 16 u8 values into two u64s for atomic storage
    /// Returns (low, high) where low contains elements 0-7 and high contains 8-15
    #[inline]
    pub fn pack_queue(queue: &[u8; SLICER_NUM_SLICES]) -> (u64, u64) {
        let mut low = 0u64;
        let mut high = 0u64;
        for i in 0..8 {
            low |= (queue[i] as u64) << (i * 8);
            high |= (queue[i + 8] as u64) << (i * 8);
        }
        (low, high)
    }

    /// Unpack two u64s into 16 u8 values
    #[inline]
    pub fn unpack_queue(low: u64, high: u64) -> [u8; SLICER_NUM_SLICES] {
        let mut queue = [0u8; SLICER_NUM_SLICES];
        for i in 0..8 {
            queue[i] = ((low >> (i * 8)) & 0xFF) as u8;
            queue[i + 8] = ((high >> (i * 8)) & 0xFF) as u8;
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
        let low = self.queue_low.load(Ordering::Relaxed);
        let high = self.queue_high.load(Ordering::Relaxed);
        Self::unpack_queue(low, high)
    }
}

impl Default for SlicerAtomics {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-stem slicer state
///
/// Manages the slicer buffer window, step sequence, and audio processing
/// for a single stem. The slicer remaps sample positions through the sequence
/// to create rearranged playback with velocity and layer support.
pub struct SlicerState {
    /// Whether the slicer is enabled
    enabled: bool,
    /// Current buffer window start position in samples (snapped to bar boundary)
    buffer_start: usize,
    /// Current buffer window end position in samples
    buffer_end: usize,
    /// Samples per slice (buffer_length / 16)
    samples_per_slice: usize,
    /// Step sequence with layers and velocities
    sequence: StepSequence,
    /// Buffer size in bars (1, 4, 8, or 16)
    buffer_bars: u32,
    /// Lock-free atomics for UI access
    atomics: Arc<SlicerAtomics>,
    /// Pre-allocated cache for the full slicer buffer window
    /// Enables backward slice jumps when sequence reorders earlier slices to play later
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
    /// One-shot override: play this slice content instead of sequence lookup
    one_shot_content: Option<u8>,
    /// The timing slot when one-shot was triggered
    one_shot_slot: usize,
    /// Position offset when triggered (to start from slice beginning)
    one_shot_start_offset: usize,
}

impl SlicerState {
    /// Create a new slicer state with default configuration
    pub fn new() -> Self {
        Self {
            enabled: false,
            buffer_start: 0,
            buffer_end: 0,
            samples_per_slice: 0,
            sequence: StepSequence::sequential(),
            buffer_bars: SLICER_DEFAULT_BARS,
            atomics: Arc::new(SlicerAtomics::new()),
            buffer_cache: vec![StereoSample::silence(); SLICER_MAX_BUFFER_SAMPLES],
            buffer_cache_valid: false,
            last_playhead: 0,
            first_beat_sample: 0,
            pending_enable: false,
            last_beat_index: 0,
            one_shot_content: None,
            one_shot_slot: 0,
            one_shot_start_offset: 0,
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

    /// Get the buffer start position (in samples)
    #[inline]
    pub fn buffer_start(&self) -> usize {
        self.buffer_start
    }

    /// Get the number of samples per slice
    #[inline]
    pub fn samples_per_slice(&self) -> usize {
        self.samples_per_slice
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
                "slicer: PENDING (buffer_bars={}) - will activate on next beat",
                self.buffer_bars
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

    /// Set the first beat sample position from track's beat grid
    ///
    /// This is used to align the slicer buffer to the track's beat grid
    /// rather than assuming beats start at sample 0.
    pub fn set_first_beat(&mut self, first_beat: usize) {
        self.first_beat_sample = first_beat;
        // Invalidate cache when grid changes
        self.buffer_cache_valid = false;
    }

    /// Reset the sequence to default order [0, 1, 2, ..., 15] at full velocity
    pub fn reset_queue(&mut self) {
        log::debug!("slicer: sequence reset to sequential [0..15]");
        self.sequence = StepSequence::sequential();
        self.sync_atomics();
    }

    /// Load a step sequence
    ///
    /// Replaces the entire sequence with the provided pattern.
    pub fn load_sequence(&mut self, sequence: StepSequence) {
        log::debug!("slicer: loading sequence");
        self.sequence = sequence;
        self.sync_atomics();
    }

    /// Load from a simple slice array (legacy format, full velocity)
    ///
    /// Replaces the entire sequence with the provided 16-step pattern at full velocity.
    pub fn load_preset(&mut self, preset: [u8; 16]) {
        log::debug!("slicer: loading preset {:?}", &preset[..]);
        self.sequence = StepSequence::from_slice_array(&preset);
        self.sync_atomics();
    }

    /// Handle a button action from the UI
    ///
    /// This is the unified API for slicer button presses. The UI just reports
    /// which button was pressed and whether shift was held - all behavior
    /// logic lives here.
    ///
    /// - **Normal press (no shift)**: Load preset pattern at button_idx
    /// - **Shift+press**: Assign context-aware slice to current timing slot + preview
    ///
    /// Context-aware slice mapping:
    /// - If playhead is in first half of buffer (slots 0-7): button N → slice N
    /// - If playhead is in second half (slots 8-15): button N → slice N+8
    pub fn handle_button_action(
        &mut self,
        button_idx: usize,
        shift_held: bool,
        current_pos: usize,
        presets: &[[u8; 16]; 8],
    ) {
        if shift_held {
            // Shift+button: Assign slice to current timing slot + one-shot preview
            let current_slot = self.slice_for_position(current_pos) as usize;

            // Context-aware slice mapping: first half uses 0-7, second half uses 8-15
            let slice_idx = if current_slot < 8 {
                button_idx.min(7)  // Clamp to valid slice index
            } else {
                (button_idx + 8).min(15)
            };

            log::debug!(
                "slicer: shift+button {} -> assign slice {} to slot {} + preview",
                button_idx, slice_idx, current_slot
            );

            // Trigger slice (assigns to current slot + one-shot preview)
            self.trigger_slice(current_pos, slice_idx);
        } else {
            // Normal button press: Load preset pattern
            if button_idx < 8 {
                log::debug!("slicer: button {} -> load preset", button_idx);
                self.load_preset(presets[button_idx]);
            }
        }
    }

    /// Set a specific slot in the sequence to a slice index (layer 0, full velocity)
    ///
    /// Directly sets a specific slot to a slice index.
    /// Used by handle_button_action for shift+button slice assignment.
    pub fn set_slot(&mut self, slot: usize, slice_idx: usize) {
        if slot < SLICER_NUM_SLICES && slice_idx < SLICER_NUM_SLICES {
            self.sequence.steps[slot] = SliceStep::single(slice_idx as u8);
            log::debug!(
                "slicer: set slot {} = slice {} -> queue={:?}",
                slot, slice_idx,
                self.sequence.to_slice_array()
            );
            self.sync_atomics();
        }
    }

    /// Set a specific step with full control over layers and velocities
    pub fn set_step(&mut self, slot: usize, step: SliceStep) {
        if slot < SLICER_NUM_SLICES {
            self.sequence.steps[slot] = step;
            self.sync_atomics();
        }
    }

    /// Set velocity for a specific layer of a step
    pub fn set_step_velocity(&mut self, slot: usize, layer: usize, velocity: f32) {
        if slot < SLICER_NUM_SLICES && layer < MAX_SLICE_LAYERS {
            self.sequence.steps[slot].velocities[layer] = velocity.clamp(0.0, 1.0);
            self.sync_atomics();
        }
    }

    /// Set slice index for a specific layer of a step
    pub fn set_step_slice(&mut self, slot: usize, layer: usize, slice_idx: u8) {
        if slot < SLICER_NUM_SLICES && layer < MAX_SLICE_LAYERS {
            self.sequence.steps[slot].slices[layer] = slice_idx;
            self.sync_atomics();
        }
    }

    /// Set a step as muted
    pub fn set_step_muted(&mut self, slot: usize, muted: bool) {
        if slot < SLICER_NUM_SLICES {
            if muted {
                self.sequence.steps[slot] = SliceStep::muted();
            } else {
                // Restore to sequential default if unmuting
                self.sequence.steps[slot] = SliceStep::single(slot as u8);
            }
            self.sync_atomics();
        }
    }

    /// Trigger a slice with one-shot playback from slice beginning
    ///
    /// Sets up a one-shot override that plays the triggered slice's content
    /// from its beginning, without seeking the deck. The deck continues its
    /// natural progression, and at the next slot boundary, normal queue
    /// playback resumes (grid-locked).
    ///
    /// The queue is also updated so future loops play the triggered content
    /// at this timing slot.
    ///
    /// Returns None (no seek needed - one-shot handles playback via remap).
    pub fn trigger_slice(&mut self, current_pos: usize, slice_idx: usize) -> Option<usize> {
        if slice_idx >= SLICER_NUM_SLICES || self.samples_per_slice == 0 {
            return None;
        }

        // Calculate current timing slot and position within slot
        let relative = current_pos.saturating_sub(self.buffer_start);
        let current_slot = (relative / self.samples_per_slice).min(SLICER_NUM_SLICES - 1);
        let pos_within_slot = relative % self.samples_per_slice;

        // Set one-shot override for immediate playback from slice start
        self.one_shot_content = Some(slice_idx as u8);
        self.one_shot_slot = current_slot;
        self.one_shot_start_offset = pos_within_slot;

        // Also update queue for future loops
        self.set_slot(current_slot, slice_idx);

        log::debug!(
            "slicer trigger: one-shot slice {} at slot {}, offset {} (start from beginning)",
            slice_idx, current_slot, pos_within_slot
        );

        // Return None - don't seek the deck, one-shot handles it
        None
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

    /// Get the mixed sample at a position with layered playback, velocity, and release fade
    ///
    /// Mixes all active layers at the current timing slot, applying velocity to each.
    /// Supports one-shot override for immediate slice triggering.
    ///
    /// **Release fade**: When the current step is audible and the next step is muted,
    /// applies a linear fade out over the last 1/4 of the slice to avoid clicks.
    ///
    /// Returns None if the position is outside the slicer buffer window.
    #[inline]
    fn get_sample_at_position(&self, original_pos: usize) -> Option<StereoSample> {
        if original_pos < self.buffer_start || original_pos >= self.buffer_end {
            return None;
        }

        if self.samples_per_slice == 0 {
            return None;
        }

        let relative = original_pos - self.buffer_start;
        let timing_slot = (relative / self.samples_per_slice).min(SLICER_NUM_SLICES - 1);
        let pos_within_slot = relative % self.samples_per_slice;

        // Check for one-shot override
        if let Some(content) = self.one_shot_content {
            if timing_slot == self.one_shot_slot {
                // One-shot active: play triggered content from beginning (full velocity, single layer)
                let adjusted_pos = pos_within_slot.saturating_sub(self.one_shot_start_offset);
                let cache_pos = (content as usize) * self.samples_per_slice + adjusted_pos;
                return self.buffer_cache.get(cache_pos).copied();
            }
        }

        // Normal sequence lookup - mix all layers with velocities
        let step = &self.sequence.steps[timing_slot];

        // Fast path: if step is completely muted, return silence
        if step.is_muted() {
            return Some(StereoSample::silence());
        }

        let mut result = StereoSample::silence();

        for layer in 0..MAX_SLICE_LAYERS {
            let slice_idx = step.slices[layer];
            let velocity = step.velocities[layer];

            // Skip muted or zero-velocity layers
            if slice_idx == MUTED_SLICE || velocity <= 0.0 {
                continue;
            }

            // Skip invalid slice indices
            if slice_idx as usize >= SLICER_NUM_SLICES {
                continue;
            }

            // Calculate cache position for this slice
            let cache_pos = (slice_idx as usize) * self.samples_per_slice + pos_within_slot;

            if let Some(sample) = self.buffer_cache.get(cache_pos) {
                // Mix with velocity
                result.left += sample.left * velocity;
                result.right += sample.right * velocity;
            }
        }

        // Apply release fade if next step is muted (to avoid clicks)
        // Fade starts at 3/4 into the slice, ends at slice boundary
        let fade_length = self.samples_per_slice / 4;
        let fade_start = self.samples_per_slice - fade_length;

        if pos_within_slot >= fade_start {
            // Check if next step is muted
            let next_slot = (timing_slot + 1) % SLICER_NUM_SLICES;
            let next_step = &self.sequence.steps[next_slot];

            if next_step.is_muted() {
                // Apply linear fade out
                let fade_pos = pos_within_slot - fade_start;
                let fade = 1.0 - (fade_pos as f32 / fade_length as f32);
                result.left *= fade;
                result.right *= fade;
            }
        }

        Some(result)
    }

    /// Legacy position remap (for UI indication and tests - returns layer 0 slice index)
    #[allow(dead_code)]
    #[inline]
    fn remap_position(&self, original_pos: usize) -> usize {
        if original_pos < self.buffer_start || original_pos >= self.buffer_end {
            return original_pos;
        }

        if self.samples_per_slice == 0 {
            return original_pos;
        }

        let relative = original_pos - self.buffer_start;
        let timing_slot = (relative / self.samples_per_slice).min(SLICER_NUM_SLICES - 1);
        let pos_within_slot = relative % self.samples_per_slice;

        // Check for one-shot override
        if let Some(content) = self.one_shot_content {
            if timing_slot == self.one_shot_slot {
                let adjusted_pos = pos_within_slot.saturating_sub(self.one_shot_start_offset);
                return (content as usize) * self.samples_per_slice + adjusted_pos;
            }
        }

        // Return layer 0 position (for backwards compat with tests)
        let step = &self.sequence.steps[timing_slot];
        let remapped_slice_idx = step.slices[0] as usize;

        if remapped_slice_idx >= SLICER_NUM_SLICES {
            return timing_slot * self.samples_per_slice + pos_within_slot;
        }

        remapped_slice_idx * self.samples_per_slice + pos_within_slot
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

        // Process each sample through layered mixing with velocities
        // No crossfade - slicer creates intentional rhythmic chops (like Serato/Traktor)
        let buffer_slice = buffer.as_mut_slice();
        for (i, sample) in buffer_slice.iter_mut().enumerate() {
            let original_pos = playhead + i;

            // Get mixed sample (layers + velocities) for positions within slicer window
            if let Some(mixed) = self.get_sample_at_position(original_pos) {
                *sample = mixed;
            }
            // Positions outside the window keep their original samples (already in buffer)
        }

        // Update current slice indicator for UI - store which slice CONTENT is playing
        let timing_slice = self.slice_for_position(playhead);

        // Clear one-shot if we've moved to a different slot
        if self.one_shot_content.is_some() {
            let timing_slot = timing_slice as usize;
            if timing_slot != self.one_shot_slot {
                log::debug!(
                    "slicer: one-shot cleared (slot {} -> {})",
                    self.one_shot_slot, timing_slot
                );
                self.one_shot_content = None;
            }
        }

        // Determine content slice - use one-shot if active, otherwise sequence (layer 0)
        let content_slice = if let Some(content) = self.one_shot_content {
            content
        } else {
            self.sequence.steps[timing_slice as usize].slices[0]
        };

        let prev_content = self.atomics.current_slice.load(Ordering::Relaxed);
        self.atomics
            .current_slice
            .store(content_slice, Ordering::Relaxed);

        // Log when content slice changes
        if content_slice != prev_content {
            log::debug!(
                "slicer: position {} -> playing slice {} content (pos={})",
                timing_slice,
                content_slice,
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
        // Pack layer 0 slice indices for atomics (UI compatibility)
        let slice_array = self.sequence.to_slice_array();
        let (low, high) = SlicerAtomics::pack_queue(&slice_array);
        self.atomics.queue_low.store(low, Ordering::Relaxed);
        self.atomics.queue_high.store(high, Ordering::Relaxed);
    }

    /// Get the current sequence
    pub fn sequence(&self) -> &StepSequence {
        &self.sequence
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
        let queue = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let (low, high) = SlicerAtomics::pack_queue(&queue);
        let unpacked = SlicerAtomics::unpack_queue(low, high);
        assert_eq!(queue, unpacked);

        let queue2 = [15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0];
        let (low2, high2) = SlicerAtomics::pack_queue(&queue2);
        let unpacked2 = SlicerAtomics::unpack_queue(low2, high2);
        assert_eq!(queue2, unpacked2);
    }

    #[test]
    fn test_load_preset() {
        let mut slicer = SlicerState::new();

        // Initial sequence: [0..15] at full velocity
        let slice_array = slicer.sequence().to_slice_array();
        assert_eq!(slice_array, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);

        // Load a preset pattern
        let preset = [0, 0, 2, 2, 4, 4, 6, 6, 8, 8, 10, 10, 12, 12, 14, 14];
        slicer.load_preset(preset);
        assert_eq!(slicer.sequence().to_slice_array(), preset);
    }

    #[test]
    fn test_set_slot() {
        let mut slicer = SlicerState::new();

        // Set specific slots
        slicer.set_slot(0, 5);
        slicer.set_slot(3, 7);
        assert_eq!(slicer.sequence().steps[0].slices[0], 5);
        assert_eq!(slicer.sequence().steps[3].slices[0], 7);
        // Other slots unchanged
        assert_eq!(slicer.sequence().steps[1].slices[0], 1);
    }

    #[test]
    fn test_reset_queue() {
        let mut slicer = SlicerState::new();

        // Modify the sequence
        slicer.load_preset([15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);

        slicer.reset_queue();
        assert_eq!(
            slicer.sequence().to_slice_array(),
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
    }

    #[test]
    fn test_remap_position() {
        let mut slicer = SlicerState::new();
        slicer.buffer_start = 0;
        slicer.buffer_end = 16000; // 16000 samples = 16 slices of 1000 samples each
        slicer.samples_per_slice = 1000;
        slicer.enabled = true;

        // Default sequence [0..15] - no remapping
        assert_eq!(slicer.remap_position(0), 0);
        assert_eq!(slicer.remap_position(500), 500);
        assert_eq!(slicer.remap_position(1000), 1000);

        // Swap sequence: slot 0 plays slice 1's content, slot 1 plays slice 0's content
        slicer.load_preset([1, 0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);

        // Position 0-999 (timing slot 0) should now play from slice 1 (1000-1999)
        assert_eq!(slicer.remap_position(0), 1000);
        assert_eq!(slicer.remap_position(500), 1500);
        assert_eq!(slicer.remap_position(999), 1999);

        // Position 1000-1999 (timing slot 1) should now play from slice 0 (0-999)
        assert_eq!(slicer.remap_position(1000), 0);
        assert_eq!(slicer.remap_position(1500), 500);
    }

    #[test]
    fn test_slice_for_position() {
        let mut slicer = SlicerState::new();
        slicer.buffer_start = 0;
        slicer.buffer_end = 16000;
        slicer.samples_per_slice = 1000;
        slicer.enabled = true;

        assert_eq!(slicer.slice_for_position(0), 0);
        assert_eq!(slicer.slice_for_position(999), 0);
        assert_eq!(slicer.slice_for_position(1000), 1);
        assert_eq!(slicer.slice_for_position(7000), 7);
        assert_eq!(slicer.slice_for_position(15000), 15);
        assert_eq!(slicer.slice_for_position(15999), 15);
    }

    #[test]
    fn test_slice_step_muted() {
        let muted = SliceStep::muted();
        assert!(muted.is_muted());

        let audible = SliceStep::single(5);
        assert!(!audible.is_muted());

        let ghost = SliceStep::with_velocity(3, 0.3);
        assert!(!ghost.is_muted());

        // Zero velocity is effectively muted
        let zero_vel = SliceStep::with_velocity(3, 0.0);
        assert!(zero_vel.is_muted());
    }

    #[test]
    fn test_step_sequence_from_slice_array() {
        let slices = [0, 0, 2, 2, 4, 4, 6, 6, 8, 8, 10, 10, 12, 12, 14, 14];
        let seq = StepSequence::from_slice_array(&slices);

        // Check that slices are preserved in layer 0
        assert_eq!(seq.to_slice_array(), slices);

        // Check that velocities are 1.0 (full)
        for step in &seq.steps {
            assert_eq!(step.velocities[0], 1.0);
        }
    }

    #[test]
    fn test_layered_playback_mixing() {
        let mut slicer = SlicerState::new();
        slicer.buffer_start = 0;
        slicer.buffer_end = 16000;
        slicer.samples_per_slice = 1000;
        slicer.enabled = true;

        // Fill cache with test data: each slice has a distinct value
        for i in 0..16 {
            let value = (i + 1) as f32 * 0.1; // 0.1, 0.2, 0.3, etc.
            for j in 0..1000 {
                let cache_idx = i * 1000 + j;
                if cache_idx < slicer.buffer_cache.len() {
                    slicer.buffer_cache[cache_idx] = StereoSample { left: value, right: value };
                }
            }
        }
        slicer.buffer_cache_valid = true;

        // Test single layer at full velocity
        slicer.sequence.steps[0] = SliceStep::single(0); // Slice 0 = value 0.1
        let sample = slicer.get_sample_at_position(0).unwrap();
        assert!((sample.left - 0.1).abs() < 0.001);

        // Test single layer at half velocity
        slicer.sequence.steps[0] = SliceStep::with_velocity(0, 0.5); // Slice 0 at 50%
        let sample = slicer.get_sample_at_position(0).unwrap();
        assert!((sample.left - 0.05).abs() < 0.001); // 0.1 * 0.5 = 0.05

        // Test two layers mixed
        slicer.sequence.steps[0] = SliceStep {
            slices: [0, 1],      // Slice 0 (0.1) + Slice 1 (0.2)
            velocities: [1.0, 1.0],
        };
        let sample = slicer.get_sample_at_position(0).unwrap();
        assert!((sample.left - 0.3).abs() < 0.001); // 0.1 + 0.2 = 0.3

        // Test two layers with different velocities
        slicer.sequence.steps[0] = SliceStep {
            slices: [2, 3],      // Slice 2 (0.3) + Slice 3 (0.4)
            velocities: [0.5, 0.25],
        };
        let sample = slicer.get_sample_at_position(0).unwrap();
        // 0.3 * 0.5 + 0.4 * 0.25 = 0.15 + 0.1 = 0.25
        assert!((sample.left - 0.25).abs() < 0.001);

        // Test muted step returns silence
        slicer.sequence.steps[1] = SliceStep::muted();
        let sample = slicer.get_sample_at_position(1000).unwrap(); // Position in slot 1
        assert_eq!(sample.left, 0.0);
        assert_eq!(sample.right, 0.0);
    }

    #[test]
    fn test_release_fade_before_muted() {
        let mut slicer = SlicerState::new();
        slicer.buffer_start = 0;
        slicer.buffer_end = 16000;
        slicer.samples_per_slice = 1000;
        slicer.enabled = true;

        // Fill cache: slice 0 has constant value 1.0
        for i in 0..1000 {
            slicer.buffer_cache[i] = StereoSample { left: 1.0, right: 1.0 };
        }
        slicer.buffer_cache_valid = true;

        // Set slot 0 = audible (slice 0), slot 1 = muted
        slicer.sequence.steps[0] = SliceStep::single(0);
        slicer.sequence.steps[1] = SliceStep::muted();

        // Fade region: last 1/4 of slice (samples 750-999)
        // Fade length = 1000 / 4 = 250 samples
        // Fade start = 1000 - 250 = 750

        // Before fade region (sample 500) - full amplitude
        let sample = slicer.get_sample_at_position(500).unwrap();
        assert!((sample.left - 1.0).abs() < 0.001);

        // At fade start (sample 750) - still full (fade = 1.0 - 0/250 = 1.0)
        let sample = slicer.get_sample_at_position(750).unwrap();
        assert!((sample.left - 1.0).abs() < 0.01);

        // Middle of fade (sample 875) - half amplitude (fade = 1.0 - 125/250 = 0.5)
        let sample = slicer.get_sample_at_position(875).unwrap();
        assert!((sample.left - 0.5).abs() < 0.01);

        // End of fade (sample 999) - near zero (fade = 1.0 - 249/250 ≈ 0.004)
        let sample = slicer.get_sample_at_position(999).unwrap();
        assert!(sample.left < 0.01);

        // No fade when next slot is NOT muted
        slicer.sequence.steps[1] = SliceStep::single(1);
        let sample = slicer.get_sample_at_position(875).unwrap();
        assert!((sample.left - 1.0).abs() < 0.001); // Full amplitude, no fade
    }
}
