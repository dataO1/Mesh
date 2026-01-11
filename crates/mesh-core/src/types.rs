//! Common types for Mesh
//!
//! This module contains the fundamental audio types used throughout the Mesh
//! DJ software suite, including stereo buffer handling and sample types.

use std::ops::{Index, IndexMut};

/// Default sample rate used throughout Mesh (48kHz - standard professional audio rate)
/// This is the default; actual rate is read from JACK at runtime.
pub const SAMPLE_RATE: u32 = 48000;

/// Number of decks in the DJ player
pub const NUM_DECKS: usize = 4;

/// Number of stems per deck (Vocals, Drums, Bass, Other)
pub const NUM_STEMS: usize = 4;

/// Maximum latency for global compensation (in samples at 44.1kHz)
/// 100ms = 4410 samples
pub const MAX_LATENCY_SAMPLES: usize = 4410;

/// Audio sample type (32-bit float for processing, stored as 16-bit in files)
pub type Sample = f32;

/// Stem identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum Stem {
    Vocals = 0,
    Drums = 1,
    Bass = 2,
    Other = 3,
}

impl Stem {
    /// Get all stems in order
    pub const ALL: [Stem; NUM_STEMS] = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

    /// Convert from index (0-3) to Stem
    pub fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Stem::Vocals),
            1 => Some(Stem::Drums),
            2 => Some(Stem::Bass),
            3 => Some(Stem::Other),
            _ => None,
        }
    }

    /// Get the name of this stem
    pub fn name(&self) -> &'static str {
        match self {
            Stem::Vocals => "Vocals",
            Stem::Drums => "Drums",
            Stem::Bass => "Bass",
            Stem::Other => "Other",
        }
    }
}

/// A single stereo sample (left and right channels)
///
/// Uses `#[repr(C)]` to ensure predictable memory layout: [left, right].
/// This enables zero-copy conversion between `&[StereoSample]` and `&[f32]`
/// (interleaved format) using bytemuck, avoiding per-frame format conversions.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StereoSample {
    pub left: Sample,
    pub right: Sample,
}

impl StereoSample {
    /// Create a new stereo sample
    #[inline]
    pub fn new(left: Sample, right: Sample) -> Self {
        Self { left, right }
    }

    /// Create a silent stereo sample
    #[inline]
    pub fn silence() -> Self {
        Self::default()
    }

    /// Create a mono sample (same value in both channels)
    #[inline]
    pub fn mono(value: Sample) -> Self {
        Self { left: value, right: value }
    }

    /// Scale both channels by a factor
    #[inline]
    pub fn scale(&self, factor: Sample) -> Self {
        Self {
            left: self.left * factor,
            right: self.right * factor,
        }
    }

    /// Add two stereo samples
    #[inline]
    pub fn add(&self, other: &Self) -> Self {
        Self {
            left: self.left + other.left,
            right: self.right + other.right,
        }
    }

    /// Get the peak amplitude (max of abs(left), abs(right))
    #[inline]
    pub fn peak(&self) -> Sample {
        self.left.abs().max(self.right.abs())
    }
}

impl std::ops::Add for StereoSample {
    type Output = Self;

    #[inline]
    fn add(self, other: Self) -> Self {
        Self {
            left: self.left + other.left,
            right: self.right + other.right,
        }
    }
}

impl std::ops::AddAssign for StereoSample {
    #[inline]
    fn add_assign(&mut self, other: Self) {
        self.left += other.left;
        self.right += other.right;
    }
}

impl std::ops::Mul<Sample> for StereoSample {
    type Output = Self;

    #[inline]
    fn mul(self, factor: Sample) -> Self {
        Self {
            left: self.left * factor,
            right: self.right * factor,
        }
    }
}

impl std::ops::MulAssign<Sample> for StereoSample {
    #[inline]
    fn mul_assign(&mut self, factor: Sample) {
        self.left *= factor;
        self.right *= factor;
    }
}

/// A buffer of stereo samples
///
/// This is the primary audio buffer type used throughout Mesh for processing
/// stereo audio. It provides efficient access to interleaved and non-interleaved
/// sample data.
#[derive(Debug, Clone)]
pub struct StereoBuffer {
    samples: Vec<StereoSample>,
}

impl StereoBuffer {
    /// Create a new buffer with the specified capacity (in stereo samples)
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
        }
    }

    /// Create a buffer filled with silence
    pub fn silence(len: usize) -> Self {
        Self {
            samples: vec![StereoSample::silence(); len],
        }
    }

    /// Create a buffer from interleaved samples [L, R, L, R, ...]
    pub fn from_interleaved(interleaved: &[Sample]) -> Self {
        assert!(interleaved.len() % 2 == 0, "Interleaved buffer must have even length");
        let samples = interleaved
            .chunks_exact(2)
            .map(|chunk| StereoSample::new(chunk[0], chunk[1]))
            .collect();
        Self { samples }
    }

    /// Create a buffer from separate left and right channel slices
    pub fn from_channels(left: &[Sample], right: &[Sample]) -> Self {
        assert_eq!(left.len(), right.len(), "Channel lengths must match");
        let samples = left
            .iter()
            .zip(right.iter())
            .map(|(&l, &r)| StereoSample::new(l, r))
            .collect();
        Self { samples }
    }

    /// Create a buffer from an existing Vec of StereoSamples
    pub fn from_vec(samples: Vec<StereoSample>) -> Self {
        Self { samples }
    }

    /// Get the number of stereo samples in the buffer
    #[inline]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if the buffer is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Resize the buffer, filling with silence if growing
    pub fn resize(&mut self, new_len: usize) {
        self.samples.resize(new_len, StereoSample::silence());
    }

    /// Truncate buffer to length without deallocating (for real-time safety)
    ///
    /// This is safe to call in audio callbacks - it never allocates.
    /// Use this instead of resize() when you know the buffer has enough capacity.
    #[inline]
    pub fn truncate(&mut self, len: usize) {
        self.samples.truncate(len);
    }

    /// Set the working length of a pre-allocated buffer (real-time safe)
    ///
    /// Panics if new_len > capacity. Use for pre-allocated buffers only.
    /// Fills any newly exposed elements with silence.
    #[inline]
    pub fn set_len_from_capacity(&mut self, new_len: usize) {
        let current_len = self.samples.len();
        if new_len > current_len {
            // Growing: fill new elements with silence (capacity already exists)
            debug_assert!(new_len <= self.samples.capacity(), "set_len_from_capacity called with len > capacity");
            self.samples.resize(new_len, StereoSample::silence());
        } else {
            // Shrinking: just truncate (no dealloc)
            self.samples.truncate(new_len);
        }
    }

    /// Fill the buffer with silence
    pub fn fill_silence(&mut self) {
        self.samples.fill(StereoSample::silence());
    }

    /// Get a slice of the samples
    #[inline]
    pub fn as_slice(&self) -> &[StereoSample] {
        &self.samples
    }

    /// Get a mutable slice of the samples
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [StereoSample] {
        &mut self.samples
    }

    /// Get a zero-copy view of samples as interleaved f32 [L, R, L, R, ...]
    ///
    /// This is a zero-cost operation thanks to `#[repr(C)]` on StereoSample.
    /// Use for passing to C libraries that expect interleaved audio.
    #[inline]
    pub fn as_interleaved(&self) -> &[Sample] {
        bytemuck::cast_slice(&self.samples)
    }

    /// Get a zero-copy mutable view of samples as interleaved f32 [L, R, L, R, ...]
    ///
    /// This is a zero-cost operation thanks to `#[repr(C)]` on StereoSample.
    /// Use for receiving audio from C libraries that produce interleaved output.
    #[inline]
    pub fn as_interleaved_mut(&mut self) -> &mut [Sample] {
        bytemuck::cast_slice_mut(&mut self.samples)
    }

    /// Copy samples to an interleaved output buffer [L, R, L, R, ...]
    pub fn to_interleaved(&self, output: &mut [Sample]) {
        assert!(output.len() >= self.samples.len() * 2);
        for (i, sample) in self.samples.iter().enumerate() {
            output[i * 2] = sample.left;
            output[i * 2 + 1] = sample.right;
        }
    }

    /// Write samples to separate left and right channel buffers
    pub fn to_channels(&self, left: &mut [Sample], right: &mut [Sample]) {
        assert!(left.len() >= self.samples.len());
        assert!(right.len() >= self.samples.len());
        for (i, sample) in self.samples.iter().enumerate() {
            left[i] = sample.left;
            right[i] = sample.right;
        }
    }

    /// Add another buffer to this one (summing samples)
    pub fn add_buffer(&mut self, other: &StereoBuffer) {
        assert_eq!(self.len(), other.len(), "Buffer lengths must match");
        for (dst, src) in self.samples.iter_mut().zip(other.samples.iter()) {
            *dst += *src;
        }
    }

    /// Scale all samples by a factor
    pub fn scale(&mut self, factor: Sample) {
        for sample in &mut self.samples {
            *sample *= factor;
        }
    }

    /// Copy from another buffer (real-time safe if pre-allocated)
    ///
    /// For RT safety, ensure `self` has sufficient capacity before calling.
    /// This method will not allocate if `self.capacity() >= other.len()`.
    pub fn copy_from(&mut self, other: &StereoBuffer) {
        let len = other.samples.len();
        debug_assert!(
            len <= self.samples.capacity(),
            "copy_from: insufficient capacity ({} < {})",
            self.samples.capacity(),
            len
        );
        // Set length to match source (truncate never deallocates, resize uses existing capacity)
        if self.samples.len() > len {
            self.samples.truncate(len);
        } else if self.samples.len() < len {
            // Fill new slots with silence (uses existing capacity, no allocation)
            self.samples.resize(len, StereoSample::silence());
        }
        // Copy data
        self.samples[..len].copy_from_slice(&other.samples[..len]);
    }

    /// Push a sample to the buffer
    #[inline]
    pub fn push(&mut self, sample: StereoSample) {
        self.samples.push(sample);
    }

    /// Get an iterator over the samples
    pub fn iter(&self) -> impl Iterator<Item = &StereoSample> {
        self.samples.iter()
    }

    /// Get a mutable iterator over the samples
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut StereoSample> {
        self.samples.iter_mut()
    }

    /// Get the peak amplitude in the buffer
    pub fn peak(&self) -> Sample {
        self.samples.iter().map(|s| s.peak()).fold(0.0, Sample::max)
    }
}

impl Index<usize> for StereoBuffer {
    type Output = StereoSample;

    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        &self.samples[index]
    }
}

impl IndexMut<usize> for StereoBuffer {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.samples[index]
    }
}

impl Default for StereoBuffer {
    fn default() -> Self {
        Self { samples: Vec::new() }
    }
}

/// Deck identifier (0-3)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeckId(pub usize);

impl DeckId {
    /// Create a new deck ID (panics if >= NUM_DECKS)
    pub fn new(id: usize) -> Self {
        assert!(id < NUM_DECKS, "Deck ID must be less than {}", NUM_DECKS);
        Self(id)
    }

    /// Get the deck number (1-4 for display)
    pub fn display_number(&self) -> usize {
        self.0 + 1
    }
}

/// Playback state for a deck
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayState {
    #[default]
    Stopped,
    Playing,
    Cueing,
}

/// Transport position in a track
#[derive(Debug, Clone, Copy, Default)]
pub struct TransportPosition {
    /// Current sample position in the track
    pub sample: usize,
    /// Current beat position (0-based, fractional)
    pub beat: f64,
    /// Current bar position (0-based, fractional)
    pub bar: f64,
}

impl TransportPosition {
    /// Create a new transport position
    pub fn new(sample: usize, beat: f64, bar: f64) -> Self {
        Self { sample, beat, bar }
    }

    /// Calculate time in seconds at the given sample rate
    pub fn time_seconds(&self, sample_rate: u32) -> f64 {
        self.sample as f64 / sample_rate as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stereo_sample_operations() {
        let a = StereoSample::new(1.0, 2.0);
        let b = StereoSample::new(0.5, 0.5);

        let sum = a + b;
        assert_eq!(sum.left, 1.5);
        assert_eq!(sum.right, 2.5);

        let scaled = a * 0.5;
        assert_eq!(scaled.left, 0.5);
        assert_eq!(scaled.right, 1.0);
    }

    #[test]
    fn test_stereo_buffer_from_interleaved() {
        let interleaved = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let buffer = StereoBuffer::from_interleaved(&interleaved);

        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer[0].left, 1.0);
        assert_eq!(buffer[0].right, 2.0);
        assert_eq!(buffer[2].left, 5.0);
        assert_eq!(buffer[2].right, 6.0);
    }

    #[test]
    fn test_stereo_buffer_to_interleaved() {
        let buffer = StereoBuffer::from_interleaved(&[1.0, 2.0, 3.0, 4.0]);
        let mut output = [0.0; 4];
        buffer.to_interleaved(&mut output);

        assert_eq!(output, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_stem_enumeration() {
        assert_eq!(Stem::ALL.len(), 4);
        assert_eq!(Stem::Vocals.name(), "Vocals");
        assert_eq!(Stem::Drums as usize, 1);
    }
}
