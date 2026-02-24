//! Audio file handling (FLAC lossless)
//!
//! This module handles reading 8-channel FLAC files containing stem-separated
//! audio (Vocals, Drums, Bass, Other as stereo pairs).
//!
//! Supports automatic resampling of legacy 44.1kHz files to 48kHz.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use rubato::{FftFixedInOut, Resampler};

use crate::types::{StereoBuffer, StereoSample, Stem, SAMPLE_RATE};

/// Expected channel count for stem files (4 stereo stems)
pub const STEM_CHANNEL_COUNT: u16 = 8;

/// Audio file errors
#[derive(Debug, Clone)]
pub enum AudioFileError {
    /// File not found or couldn't be opened
    IoError(String),
    /// Invalid or unsupported file format
    InvalidFormat(String),
    /// Wrong channel count for stem file
    WrongChannelCount { expected: u16, found: u16 },
    /// Wrong sample rate
    WrongSampleRate { expected: u32, found: u32 },
    /// Unsupported bit depth
    UnsupportedBitDepth(u16),
    /// Missing required chunk
    MissingChunk(&'static str),
    /// File is corrupted or truncated
    Corrupted(String),
}

impl std::fmt::Display for AudioFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioFileError::IoError(msg) => write!(f, "IO error: {}", msg),
            AudioFileError::InvalidFormat(msg) => write!(f, "Invalid format: {}", msg),
            AudioFileError::WrongChannelCount { expected, found } => {
                write!(f, "Wrong channel count: expected {}, found {}", expected, found)
            }
            AudioFileError::WrongSampleRate { expected, found } => {
                write!(f, "Wrong sample rate: expected {}, found {}", expected, found)
            }
            AudioFileError::UnsupportedBitDepth(depth) => {
                write!(f, "Unsupported bit depth: {}", depth)
            }
            AudioFileError::MissingChunk(name) => write!(f, "Missing required chunk: {}", name),
            AudioFileError::Corrupted(msg) => write!(f, "File corrupted: {}", msg),
        }
    }
}

impl std::error::Error for AudioFileError {}

/// Audio format information from fmt chunk
#[derive(Debug, Clone)]
pub struct AudioFormat {
    /// Number of channels (should be 8 for stem files)
    pub channels: u16,
    /// Sample rate in Hz (48kHz default, 44.1kHz supported with resampling)
    pub sample_rate: u32,
    /// Bits per sample (16, 24, or 32)
    pub bits_per_sample: u16,
    /// Bytes per sample frame (channels * bits_per_sample / 8)
    pub block_align: u16,
    /// Audio format tag (1 = PCM, 3 = IEEE float)
    pub format_tag: u16,
}

impl AudioFormat {
    /// Check if this format is compatible with Mesh requirements
    ///
    /// Note: Sample rate is validated but different rates are allowed -
    /// files will be resampled to match the system rate (48kHz) on load.
    pub fn is_compatible(&self) -> Result<(), AudioFileError> {
        if self.channels != STEM_CHANNEL_COUNT {
            return Err(AudioFileError::WrongChannelCount {
                expected: STEM_CHANNEL_COUNT,
                found: self.channels,
            });
        }
        // Allow common sample rates - resampling will handle the conversion
        // Supported: 44100, 48000, 88200, 96000
        if self.sample_rate != 44100 && self.sample_rate != 48000
            && self.sample_rate != 88200 && self.sample_rate != 96000 {
            return Err(AudioFileError::WrongSampleRate {
                expected: SAMPLE_RATE,
                found: self.sample_rate,
            });
        }
        if self.bits_per_sample != 16 && self.bits_per_sample != 24 && self.bits_per_sample != 32 {
            return Err(AudioFileError::UnsupportedBitDepth(self.bits_per_sample));
        }
        Ok(())
    }

    /// Check if resampling is needed to match the target sample rate
    pub fn needs_resampling(&self) -> bool {
        self.sample_rate != SAMPLE_RATE
    }
}

/// A cue point in the audio file
#[derive(Debug, Clone)]
pub struct CuePoint {
    /// Cue point index (0-7 for 8 action buttons)
    pub index: u8,
    /// Sample position in the file
    pub sample_position: u64,
    /// Label (from adtl chunk)
    pub label: String,
    /// Color as hex string (from adtl chunk)
    pub color: Option<String>,
}

/// A saved loop in the audio file
///
/// Loops are stored separately from hot cues, allowing up to 8 loop slots.
/// Each loop has a start and end position, with optional label and color.
#[derive(Debug, Clone)]
pub struct SavedLoop {
    /// Loop slot index (0-7 for 8 loop buttons)
    pub index: u8,
    /// Loop start sample position
    pub start_sample: u64,
    /// Loop end sample position
    pub end_sample: u64,
    /// Label for the loop (e.g., "Verse", "Chorus")
    pub label: String,
    /// Color as hex string
    pub color: Option<String>,
}

/// Beat grid information
#[derive(Debug, Clone)]
pub struct BeatGrid {
    /// Sample positions of beats
    pub beats: Vec<u64>,
    /// First beat sample position (for regeneration from BPM)
    pub first_beat_sample: Option<u64>,
}

impl BeatGrid {
    /// Create an empty beat grid
    pub fn new() -> Self {
        Self {
            beats: Vec::new(),
            first_beat_sample: None,
        }
    }

    /// Create a beat grid from a comma-separated list of sample positions
    pub fn from_csv(csv: &str) -> Self {
        let beats: Vec<u64> = csv
            .split(',')
            .filter_map(|s| s.trim().parse::<u64>().ok())
            .collect();
        let first_beat_sample = beats.first().copied();
        Self {
            beats,
            first_beat_sample,
        }
    }

    /// Regenerate beat grid from BPM, first beat position, and duration
    ///
    /// This creates a uniform beat grid based on tempo and first beat,
    /// which is more efficient than storing all beat positions.
    ///
    /// Uses the default SAMPLE_RATE (48kHz). For other sample rates,
    /// use `regenerate_with_rate()` instead.
    pub fn regenerate(first_beat_sample: u64, bpm: f64, duration_samples: u64) -> Self {
        use crate::types::SAMPLE_RATE;
        Self::regenerate_with_rate(first_beat_sample, bpm, duration_samples, SAMPLE_RATE)
    }

    /// Regenerate beat grid with a specific sample rate
    ///
    /// Use this when the audio has been resampled to a different rate than 48kHz.
    /// The sample positions will be calculated using the provided sample rate.
    pub fn regenerate_with_rate(first_beat_sample: u64, bpm: f64, duration_samples: u64, sample_rate: u32) -> Self {
        if bpm <= 0.0 || duration_samples == 0 {
            return Self::new();
        }

        let samples_per_beat = sample_rate as f64 * 60.0 / bpm;
        if samples_per_beat <= 0.0 {
            return Self::new();
        }

        let num_beats = ((duration_samples.saturating_sub(first_beat_sample)) as f64 / samples_per_beat) as usize;

        // Use f64 accumulation to prevent truncation drift.
        // Integer cast of samples_per_beat loses the fractional part (e.g. 16551.724 → 16551
        // at 174 BPM), which accumulates to ~7.5ms over 500 beats. Computing each position
        // from the f64 formula limits error to ±0.5 samples regardless of beat count.
        let beats: Vec<u64> = (0..=num_beats)
            .map(|i| (first_beat_sample as f64 + i as f64 * samples_per_beat).round() as u64)
            .collect();

        Self {
            beats,
            first_beat_sample: Some(first_beat_sample),
        }
    }

    /// Scale all sample positions by a ratio (for sample rate conversion)
    ///
    /// When audio is resampled from one rate to another, all sample positions
    /// need to be scaled by (target_rate / source_rate) to remain accurate.
    pub fn scale_positions(&mut self, ratio: f64) {
        // Scale all beat positions
        for beat in &mut self.beats {
            *beat = ((*beat as f64) * ratio).round() as u64;
        }
        // Scale first beat sample
        if let Some(ref mut fbs) = self.first_beat_sample {
            *fbs = ((*fbs as f64) * ratio).round() as u64;
        }
    }

    /// Get the beat index for a sample position (which beat are we on/past)
    pub fn beat_at_sample(&self, sample: u64) -> Option<usize> {
        // Find the last beat that is <= sample (i.e., which beat are we on)
        self.beats.iter().rposition(|&b| b <= sample)
    }
}

impl Default for BeatGrid {
    fn default() -> Self {
        Self::new()
    }
}

/// Metadata extracted from bext and cue chunks
#[derive(Debug, Clone, Default)]
pub struct TrackMetadata {
    /// Track title (parsed from filename or embedded tags during import)
    pub title: Option<String>,
    /// Artist name
    pub artist: Option<String>,
    /// BPM of the track
    pub bpm: Option<f64>,
    /// Original BPM (before any adjustments)
    pub original_bpm: Option<f64>,
    /// Musical key (e.g., "Am", "C#m")
    pub key: Option<String>,
    /// Integrated LUFS loudness (EBU R128)
    ///
    /// Measured once during import. Used for automatic gain compensation
    /// to reach the configured target loudness level.
    pub lufs: Option<f32>,
    /// Duration in seconds (calculated from file header)
    pub duration_seconds: Option<f64>,
    /// Beat grid
    pub beat_grid: BeatGrid,
    /// Cue points (up to 8)
    pub cue_points: Vec<CuePoint>,
    /// Saved loops (up to 8)
    pub saved_loops: Vec<SavedLoop>,
    /// Drop marker for structural alignment (sample position)
    ///
    /// Used for linked stems: when swapping stems between tracks,
    /// the drop markers are aligned so that structural elements
    /// (e.g., the drop) play at the same time.
    pub drop_marker: Option<u64>,
    /// Stem link references (prepared links stored in mslk chunk)
    ///
    /// These are pre-configured links to stems from other tracks.
    /// When the track loads, these linked stems can be loaded in the background.
    pub stem_links: Vec<StemLinkReference>,
}

impl TrackMetadata {
    /// Scale all sample-based positions for sample rate conversion
    ///
    /// When audio is resampled from `source_rate` to `target_rate`, all sample
    /// positions (cue points, loops, beat grid, drop marker) must be scaled
    /// by the ratio `target_rate / source_rate` to remain accurate.
    ///
    /// # Arguments
    /// * `source_rate` - Original sample rate of the audio file
    /// * `target_rate` - Target sample rate after resampling
    ///
    /// # Example
    /// ```ignore
    /// // Audio resampled from 48kHz to 44.1kHz
    /// metadata.scale_sample_positions(48000, 44100);
    /// ```
    pub fn scale_sample_positions(&mut self, source_rate: u32, target_rate: u32) {
        if source_rate == target_rate || source_rate == 0 {
            return; // No scaling needed
        }

        let ratio = target_rate as f64 / source_rate as f64;
        log::info!(
            "[METADATA] Scaling sample positions: {}Hz -> {}Hz (ratio={:.6})",
            source_rate, target_rate, ratio
        );

        // Scale cue points
        for cue in &mut self.cue_points {
            let old_pos = cue.sample_position;
            cue.sample_position = ((cue.sample_position as f64) * ratio).round() as u64;
            log::debug!(
                "[METADATA] Cue {} scaled: {} -> {}",
                cue.index, old_pos, cue.sample_position
            );
        }

        // Scale saved loops
        for saved_loop in &mut self.saved_loops {
            saved_loop.start_sample = ((saved_loop.start_sample as f64) * ratio).round() as u64;
            saved_loop.end_sample = ((saved_loop.end_sample as f64) * ratio).round() as u64;
        }

        // Scale beat grid
        self.beat_grid.scale_positions(ratio);

        // Scale drop marker
        if let Some(ref mut dm) = self.drop_marker {
            let old_dm = *dm;
            *dm = ((*dm as f64) * ratio).round() as u64;
            log::debug!("[METADATA] Drop marker scaled: {} -> {}", old_dm, *dm);
        }

        // Scale stem link references (source drop markers)
        for stem_link in &mut self.stem_links {
            stem_link.source_drop_marker =
                ((stem_link.source_drop_marker as f64) * ratio).round() as u64;
        }
    }

    /// Regenerate the beat grid for a specific sample rate
    ///
    /// This is useful after resampling when you want to recalculate beat positions
    /// using the correct samples-per-beat for the new sample rate.
    pub fn regenerate_beat_grid(&mut self, duration_samples: u64, sample_rate: u32) {
        if let Some(bpm) = self.bpm {
            if let Some(first_beat) = self.beat_grid.first_beat_sample {
                self.beat_grid = BeatGrid::regenerate_with_rate(
                    first_beat,
                    bpm,
                    duration_samples,
                    sample_rate,
                );
            }
        }
    }
}

/// Reference to a linked stem stored in a WAV file
///
/// This is stored in the `mslk` (Mesh Stem Links) chunk and allows
/// tracks to specify which stems should be linked from other tracks.
#[derive(Debug, Clone)]
pub struct StemLinkReference {
    /// Which stem slot this link applies to (0=Vocals, 1=Drums, 2=Bass, 3=Other)
    pub stem_index: u8,
    /// Path to the track containing the linked stem (relative or absolute)
    pub source_path: std::path::PathBuf,
    /// Which stem to extract from the source track
    pub source_stem: u8,
    /// Drop marker position in the source track (for alignment)
    pub source_drop_marker: u64,
}

impl StemLinkReference {
    /// Create a new stem link reference
    pub fn new(stem_index: u8, source_path: std::path::PathBuf, source_stem: u8, source_drop_marker: u64) -> Self {
        Self {
            stem_index,
            source_path,
            source_stem,
            source_drop_marker,
        }
    }
}

// ============================================================================
// Database type conversions (From/Into implementations)
// ============================================================================

impl From<crate::db::CuePoint> for CuePoint {
    fn from(db_cue: crate::db::CuePoint) -> Self {
        Self {
            index: db_cue.index,
            sample_position: db_cue.sample_position as u64,
            label: db_cue.label.unwrap_or_default(),
            color: db_cue.color,
        }
    }
}

impl From<crate::db::SavedLoop> for SavedLoop {
    fn from(db_loop: crate::db::SavedLoop) -> Self {
        Self {
            index: db_loop.index,
            start_sample: db_loop.start_sample as u64,
            end_sample: db_loop.end_sample as u64,
            label: db_loop.label.unwrap_or_default(),
            color: db_loop.color,
        }
    }
}

impl From<crate::db::Track> for TrackMetadata {
    fn from(track: crate::db::Track) -> Self {
        // Regenerate beat grid from first_beat_sample and BPM
        let beat_grid = if let Some(bpm) = track.bpm {
            let duration_samples = (track.duration_seconds * crate::types::SAMPLE_RATE as f64) as u64;
            BeatGrid::regenerate(track.first_beat_sample as u64, bpm, duration_samples)
        } else {
            BeatGrid::default()
        };

        Self {
            title: Some(track.title.clone()),
            artist: track.artist.clone(),
            bpm: track.bpm,
            original_bpm: track.original_bpm,
            key: track.key.clone(),
            lufs: track.lufs,
            duration_seconds: Some(track.duration_seconds),
            beat_grid,
            cue_points: track.cue_points.into_iter().map(Into::into).collect(),
            saved_loops: track.saved_loops.into_iter().map(Into::into).collect(),
            drop_marker: track.drop_marker.map(|d| d as u64),
            // Stem links are NOT included here because StemLink has track_id references
            // that require database lookups. The engine handles stem link loading separately.
            stem_links: Vec::new(),
        }
    }
}

/// Resolve track metadata from the correct database for a given file path.
///
/// This is the **single source of truth** for metadata resolution. All code
/// that needs track metadata from a database should use this function instead
/// of calling `db.get_track_by_path()` directly.
///
/// # Path resolution strategy
///
/// 1. Try the local database with the full absolute path (fast hit for local
///    tracks, no-op miss for USB tracks since they're not in the local DB).
/// 2. If not found, check whether the path is under a USB `mesh-collection/`
///    directory. If so, open (or cache) that USB's database and query with the
///    collection-root prefix stripped (USB databases store relative paths).
/// 3. If no record is found in any database, returns `TrackMetadata::default()`.
///
/// This order avoids opening duplicate connections to the local mesh.db
/// (whose path also contains `mesh-collection`).
pub fn resolve_track_metadata(path: &Path, local_db: &crate::db::DatabaseService) -> TrackMetadata {
    use crate::usb::{find_collection_root, get_or_open_usb_database};

    // 1. Try local database with the full path.
    //    For local tracks this is the fast path. For USB tracks this will
    //    return None (the track simply doesn't exist in the local DB).
    let path_str = path.to_string_lossy().into_owned();
    match local_db.get_track_metadata(&path_str) {
        Ok(Some(metadata)) => return metadata,
        Ok(None) => {}
        Err(e) => {
            log::warn!("[RESOLVE] Local DB error for {}: {}", path_str, e);
        }
    }

    // 2. Try USB database: find the collection root, strip it from the path,
    //    and query with the relative path that USB databases expect.
    if let Some(collection_root) = find_collection_root(path) {
        if let Some(usb_db) = get_or_open_usb_database(&collection_root) {
            let lookup_path = path.strip_prefix(&collection_root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path_str.clone());

            match usb_db.get_track_metadata(&lookup_path) {
                Ok(Some(metadata)) => {
                    log::info!(
                        "[RESOLVE] Found metadata in USB DB ({}) for: {}",
                        collection_root.display(), lookup_path
                    );
                    return metadata;
                }
                Ok(None) => {
                    log::warn!(
                        "[RESOLVE] Track not in USB DB ({}): {}",
                        collection_root.display(), lookup_path
                    );
                }
                Err(e) => {
                    log::error!(
                        "[RESOLVE] USB DB error ({}): {}",
                        collection_root.display(), e
                    );
                }
            }
        }
    }

    log::warn!("[RESOLVE] Track not found in any database: {}, using defaults", path_str);
    TrackMetadata::default()
}

/// Stem audio buffers extracted from a file
#[derive(Debug, Clone)]
pub struct StemBuffers {
    /// Vocals stem (stereo)
    pub vocals: StereoBuffer,
    /// Drums stem (stereo)
    pub drums: StereoBuffer,
    /// Bass stem (stereo)
    pub bass: StereoBuffer,
    /// Other stem (stereo)
    pub other: StereoBuffer,
}

impl StemBuffers {
    /// Create new stem buffers with the given length
    ///
    /// Allocates stems SEQUENTIALLY with yields between each allocation.
    /// This prevents page fault storms from blocking the audio RT thread.
    ///
    /// ## Why Sequential?
    ///
    /// The previous parallel allocation (via Rayon) triggered ~452,000 page faults
    /// simultaneously across 4 threads. This overwhelmed the kernel's page fault
    /// handler, causing scheduling delays that blocked the audio RT thread.
    ///
    /// Sequential allocation with yields:
    /// - 113K faults → yield → 113K faults → yield → ...
    /// - Each yield gives the audio RT thread a chance to run
    pub fn with_length(len: usize) -> Self {
        use std::time::Instant;

        let start = Instant::now();

        // Allocate sequentially with yields between each stem
        // This spreads page faults over time, preventing RT thread starvation
        let vocals = StereoBuffer::silence(len);
        log::debug!("    [PERF] Allocated vocals in {:?}", start.elapsed());
        std::thread::yield_now();

        let drums_start = Instant::now();
        let drums = StereoBuffer::silence(len);
        log::debug!("    [PERF] Allocated drums in {:?}", drums_start.elapsed());
        std::thread::yield_now();

        let bass_start = Instant::now();
        let bass = StereoBuffer::silence(len);
        log::debug!("    [PERF] Allocated bass in {:?}", bass_start.elapsed());
        std::thread::yield_now();

        let other_start = Instant::now();
        let other = StereoBuffer::silence(len);
        log::debug!("    [PERF] Allocated other in {:?}", other_start.elapsed());

        Self { vocals, drums, bass, other }
    }

    /// Get the number of samples (all stems have same length)
    pub fn len(&self) -> usize {
        self.vocals.len()
    }

    /// Check if buffers are empty
    pub fn is_empty(&self) -> bool {
        self.vocals.is_empty()
    }

    /// Get a buffer by stem type
    pub fn get(&self, stem: Stem) -> &StereoBuffer {
        match stem {
            Stem::Vocals => &self.vocals,
            Stem::Drums => &self.drums,
            Stem::Bass => &self.bass,
            Stem::Other => &self.other,
        }
    }

    /// Get a mutable buffer by stem type
    pub fn get_mut(&mut self, stem: Stem) -> &mut StereoBuffer {
        match stem {
            Stem::Vocals => &mut self.vocals,
            Stem::Drums => &mut self.drums,
            Stem::Bass => &mut self.bass,
            Stem::Other => &mut self.other,
        }
    }

    /// Copy `count` frames from `src` (starting at offset 0) into `self` at `dst_offset`.
    ///
    /// Used after parallel decoding to merge per-region results into the main buffer.
    /// Panics if `dst_offset + count > self.len()` or `count > src.len()`.
    pub fn copy_region_from(&mut self, src: &StemBuffers, dst_offset: usize, count: usize) {
        self.vocals.as_mut_slice()[dst_offset..dst_offset + count]
            .copy_from_slice(&src.vocals.as_slice()[..count]);
        self.drums.as_mut_slice()[dst_offset..dst_offset + count]
            .copy_from_slice(&src.drums.as_slice()[..count]);
        self.bass.as_mut_slice()[dst_offset..dst_offset + count]
            .copy_from_slice(&src.bass.as_slice()[..count]);
        self.other.as_mut_slice()[dst_offset..dst_offset + count]
            .copy_from_slice(&src.other.as_slice()[..count]);
    }

    /// Get duration in seconds at the standard sample rate
    pub fn duration_seconds(&self) -> f64 {
        self.vocals.len() as f64 / SAMPLE_RATE as f64
    }

    /// Resample all stems from source rate to target rate
    ///
    /// Uses high-quality FFT-based resampling via rubato.
    /// This is called automatically when loading legacy 44.1kHz files.
    pub fn resample(&self, source_rate: u32, target_rate: u32) -> Result<Self, AudioFileError> {
        if source_rate == target_rate {
            return Ok(self.clone());
        }

        use std::time::Instant;
        let start = Instant::now();

        log::info!(
            "    [PERF] Resampling {} frames from {}Hz to {}Hz",
            self.len(),
            source_rate,
            target_rate
        );

        // Calculate output length
        let ratio = target_rate as f64 / source_rate as f64;
        let output_len = (self.len() as f64 * ratio).ceil() as usize;

        // Create output buffers
        let mut output = StemBuffers::with_length(output_len);

        // Resample each stem (stereo = 2 channels)
        for stem in Stem::ALL {
            resample_stereo_buffer(
                self.get(stem),
                output.get_mut(stem),
                source_rate,
                target_rate,
            )?;
        }

        log::info!(
            "    [PERF] Resampling complete: {:?} ({} -> {} frames)",
            start.elapsed(),
            self.len(),
            output_len
        );

        Ok(output)
    }
}

/// Resample a stereo buffer using FFT-based resampling
fn resample_stereo_buffer(
    input: &StereoBuffer,
    output: &mut StereoBuffer,
    source_rate: u32,
    target_rate: u32,
) -> Result<(), AudioFileError> {
    const CHANNELS: usize = 2;

    // Create resampler
    let mut resampler = FftFixedInOut::<f32>::new(
        source_rate as usize,
        target_rate as usize,
        1024, // chunk size
        CHANNELS,
    ).map_err(|e| AudioFileError::IoError(format!("Resampler init failed: {}", e)))?;

    // Convert StereoBuffer to separate channel vectors (rubato expects Vec<Vec<f32>>)
    let input_len = input.len();
    let mut left_in: Vec<f32> = Vec::with_capacity(input_len);
    let mut right_in: Vec<f32> = Vec::with_capacity(input_len);

    for sample in input.as_slice() {
        left_in.push(sample.left);
        right_in.push(sample.right);
    }

    // Process in chunks
    let chunk_size = resampler.input_frames_max();
    let output_chunk_size = resampler.output_frames_max();

    let mut left_out: Vec<f32> = Vec::with_capacity(output.len());
    let mut right_out: Vec<f32> = Vec::with_capacity(output.len());

    let mut pos = 0;
    while pos < input_len {
        let end = (pos + chunk_size).min(input_len);
        let frames_in = end - pos;

        // Prepare input chunk (pad with zeros if needed)
        let mut chunk_in = vec![vec![0.0f32; chunk_size]; CHANNELS];
        chunk_in[0][..frames_in].copy_from_slice(&left_in[pos..end]);
        chunk_in[1][..frames_in].copy_from_slice(&right_in[pos..end]);

        // Process
        let chunk_out = resampler.process(&chunk_in, None)
            .map_err(|e| AudioFileError::IoError(format!("Resampling failed: {}", e)))?;

        // Calculate how many output frames correspond to input frames
        let frames_out = if frames_in == chunk_size {
            output_chunk_size
        } else {
            // Last partial chunk - scale proportionally
            ((frames_in as f64 / chunk_size as f64) * output_chunk_size as f64).ceil() as usize
        };

        left_out.extend_from_slice(&chunk_out[0][..frames_out.min(chunk_out[0].len())]);
        right_out.extend_from_slice(&chunk_out[1][..frames_out.min(chunk_out[1].len())]);

        pos += chunk_size;
    }

    // Copy to output buffer
    let out_len = output.len().min(left_out.len());
    for i in 0..out_len {
        output.as_mut_slice()[i] = StereoSample::new(left_out[i], right_out[i]);
    }

    Ok(())
}

/// Resample mono audio using high-quality FFT-based resampling via rubato.
///
/// This is the mono equivalent of `resample_stereo_buffer`, used for
/// resampling analysis audio to Essentia's expected 44100 Hz rate.
///
/// Returns `Ok(input.to_vec())` if source_rate == target_rate (no-op).
pub fn resample_mono_audio(
    input: &[f32],
    source_rate: u32,
    target_rate: u32,
) -> Result<Vec<f32>, AudioFileError> {
    if source_rate == target_rate {
        return Ok(input.to_vec());
    }

    let input_len = input.len();
    if input_len == 0 {
        return Ok(Vec::new());
    }

    const CHANNELS: usize = 1;

    let mut resampler = FftFixedInOut::<f32>::new(
        source_rate as usize,
        target_rate as usize,
        1024,
        CHANNELS,
    )
    .map_err(|e| AudioFileError::IoError(format!("Mono resampler init failed: {}", e)))?;

    let chunk_size = resampler.input_frames_max();
    let output_chunk_size = resampler.output_frames_max();

    let ratio = target_rate as f64 / source_rate as f64;
    let expected_output_len = (input_len as f64 * ratio).ceil() as usize;
    let mut output = Vec::with_capacity(expected_output_len);

    let mut pos = 0;
    while pos < input_len {
        let end = (pos + chunk_size).min(input_len);
        let frames_in = end - pos;

        // Prepare input chunk (pad with zeros if needed for last chunk)
        let mut chunk_in = vec![vec![0.0f32; chunk_size]; CHANNELS];
        chunk_in[0][..frames_in].copy_from_slice(&input[pos..end]);

        let chunk_out = resampler
            .process(&chunk_in, None)
            .map_err(|e| AudioFileError::IoError(format!("Mono resampling failed: {}", e)))?;

        let frames_out = if frames_in == chunk_size {
            output_chunk_size
        } else {
            ((frames_in as f64 / chunk_size as f64) * output_chunk_size as f64).ceil() as usize
        };

        output.extend_from_slice(&chunk_out[0][..frames_out.min(chunk_out[0].len())]);
        pos += chunk_size;
    }

    // Trim to expected length (rubato may produce slightly more)
    output.truncate(expected_output_len);

    Ok(output)
}

/// FLAC audio file reader
///
/// Reads the entire file into memory on open, then uses symphonia for
/// seeking and decoding. This allows USB I/O to happen once upfront,
/// with all subsequent region reads being pure CPU decode.
pub struct AudioFileReader {
    format: AudioFormat,
    /// Entire file contents in memory (shared via Arc for region reads)
    data: Arc<[u8]>,
    /// Total number of sample frames
    total_frames: u64,
}

impl AudioFileReader {
    /// Open an audio file for reading
    ///
    /// Reads the entire file into memory and probes with symphonia to extract
    /// format information. All I/O happens here — subsequent reads are CPU-only.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, AudioFileError> {
        use std::time::Instant;
        use symphonia::core::codecs::CODEC_TYPE_NULL;
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let path_ref = path.as_ref();
        let read_start = Instant::now();

        // Read entire file into memory (single sequential I/O)
        let file_bytes = std::fs::read(path_ref)
            .map_err(|e| AudioFileError::IoError(e.to_string()))?;
        let file_size = file_bytes.len();
        let data: Arc<[u8]> = file_bytes.into();

        log::info!(
            "    [PERF] File read into memory: {:?} ({:.1} MB)",
            read_start.elapsed(),
            file_size as f64 / 1_000_000.0
        );

        // Probe with symphonia to get format info
        let cursor = Cursor::new(Arc::clone(&data));
        let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path_ref.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(|e| AudioFileError::InvalidFormat(format!("Probe failed: {}", e)))?;

        let track = probed.format.tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| AudioFileError::InvalidFormat("No audio track found".into()))?;

        let codec_params = &track.codec_params;

        let channels = codec_params.channels
            .map(|c| c.count() as u16)
            .unwrap_or(8);

        let sample_rate = codec_params.sample_rate
            .ok_or_else(|| AudioFileError::InvalidFormat("Unknown sample rate".into()))?;

        let bits_per_sample = codec_params.bits_per_sample.unwrap_or(16) as u16;

        let total_frames = codec_params.n_frames
            .ok_or_else(|| AudioFileError::InvalidFormat("Unknown frame count".into()))?;

        let block_align = channels * (bits_per_sample / 8);

        let format = AudioFormat {
            channels,
            sample_rate,
            bits_per_sample,
            block_align,
            format_tag: 1, // PCM equivalent
        };

        // Validate channel count
        if channels != STEM_CHANNEL_COUNT {
            return Err(AudioFileError::WrongChannelCount {
                expected: STEM_CHANNEL_COUNT,
                found: channels,
            });
        }

        log::info!(
            "    [PERF] Format: {}ch {}Hz {}bit, {} frames ({:.1}s)",
            channels, sample_rate, bits_per_sample, total_frames,
            total_frames as f64 / sample_rate as f64
        );

        Ok(Self { format, data, total_frames })
    }

    /// Create a new symphonia decoder from the in-memory data
    ///
    /// Each call creates a fresh decoder with its own Cursor — allowing
    /// independent seek positions for parallel or sequential region reads.
    fn create_decoder(&self) -> Result<(
        Box<dyn symphonia::core::formats::FormatReader>,
        Box<dyn symphonia::core::codecs::Decoder>,
        u32, // track_id
    ), AudioFileError> {
        use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let cursor = Cursor::new(Arc::clone(&self.data));
        let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

        let mut hint = Hint::new();
        hint.with_extension("flac");

        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(|e| AudioFileError::InvalidFormat(format!("Probe failed: {}", e)))?;

        let track = probed.format.tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| AudioFileError::InvalidFormat("No audio track found".into()))?;

        let track_id = track.id;

        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| AudioFileError::InvalidFormat(format!("Decoder init failed: {}", e)))?;

        Ok((probed.format, decoder, track_id))
    }

    /// Get the audio format
    pub fn format(&self) -> &AudioFormat {
        &self.format
    }

    /// Get the number of sample frames in the file
    pub fn frame_count(&self) -> u64 {
        self.total_frames
    }

    /// Get the duration in seconds
    pub fn duration_seconds(&self) -> f64 {
        self.total_frames as f64 / self.format.sample_rate as f64
    }

    /// Read all audio data into stem buffers
    ///
    /// This uses the default target sample rate (SAMPLE_RATE constant, 48kHz).
    /// For sample-rate-aware loading, use `read_all_stems_to(target_rate)` instead.
    pub fn read_all_stems(&self) -> Result<StemBuffers, AudioFileError> {
        self.read_all_stems_to(SAMPLE_RATE)
    }

    /// Read all audio data into stem buffers, resampling to target rate
    ///
    /// Decodes the full FLAC stream sequentially, deinterleaving 8 channels
    /// into 4 stereo stem buffers. Resamples if the file rate differs from target.
    pub fn read_all_stems_to(&self, target_sample_rate: u32) -> Result<StemBuffers, AudioFileError> {
        use std::time::Instant;
        use symphonia::core::audio::SampleBuffer;

        let frame_count = self.total_frames as usize;

        // Allocation timing
        let alloc_start = Instant::now();
        let mut stems = StemBuffers::with_length(frame_count);
        log::debug!(
            "    [PERF] Buffer allocation: {:?} ({} frames)",
            alloc_start.elapsed(),
            frame_count
        );

        // Decode timing
        let decode_start = Instant::now();
        let (mut format_reader, mut decoder, track_id) = self.create_decoder()?;

        let mut write_pos = 0usize;
        let mut sample_buf: Option<SampleBuffer<f32>> = None;

        loop {
            let packet = match format_reader.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(AudioFileError::IoError(format!("Decode error: {}", e))),
            };

            if packet.track_id() != track_id {
                continue;
            }

            let decoded = decoder.decode(&packet)
                .map_err(|e| AudioFileError::IoError(format!("Packet decode error: {}", e)))?;

            // Initialize sample buffer on first decode
            if sample_buf.is_none() {
                let spec = *decoded.spec();
                let duration = decoded.capacity() as u64;
                sample_buf = Some(SampleBuffer::new(duration, spec));
            }

            if let Some(ref mut buf) = sample_buf {
                buf.copy_interleaved_ref(decoded);
                let samples = buf.samples();
                let ch = 8;
                let packet_frames = samples.len() / ch;

                let frames_to_write = packet_frames.min(frame_count.saturating_sub(write_pos));
                for j in 0..frames_to_write {
                    let base = j * ch;
                    let i = write_pos + j;
                    stems.vocals.as_mut_slice()[i] = StereoSample::new(samples[base], samples[base + 1]);
                    stems.drums.as_mut_slice()[i] = StereoSample::new(samples[base + 2], samples[base + 3]);
                    stems.bass.as_mut_slice()[i] = StereoSample::new(samples[base + 4], samples[base + 5]);
                    stems.other.as_mut_slice()[i] = StereoSample::new(samples[base + 6], samples[base + 7]);
                }
                write_pos += frames_to_write;
            }
        }

        let decode_elapsed = decode_start.elapsed();
        let raw_bytes = frame_count * 8 * 2; // 8 channels × 2 bytes (16-bit)
        log::info!(
            "    [PERF] FLAC decode: {:?} ({:.1} MB decoded, {:.1} MB/s effective)",
            decode_elapsed,
            raw_bytes as f64 / 1_000_000.0,
            if decode_elapsed.as_secs_f64() > 0.0 {
                (raw_bytes as f64 / 1_000_000.0) / decode_elapsed.as_secs_f64()
            } else { 0.0 }
        );

        // Resample if file sample rate differs from target rate
        if self.format.sample_rate != target_sample_rate {
            log::info!(
                "    File sample rate ({} Hz) differs from target rate ({} Hz), resampling...",
                self.format.sample_rate,
                target_sample_rate
            );
            stems = stems.resample(self.format.sample_rate, target_sample_rate)?;
        }

        Ok(stems)
    }

    /// Read a single stem from the FLAC file, discarding all other channels.
    ///
    /// This allocates only 1 `StereoBuffer` (~150 MB for a 5-min track) instead
    /// of 4 (~600 MB), and only writes the 2 channels belonging to the requested
    /// stem during decode. The other 6 channels are decoded (FLAC interleaves
    /// them) but immediately discarded.
    ///
    /// If the file needs resampling, only the single stem is resampled.
    pub fn read_single_stem_to(
        &self,
        stem: crate::types::Stem,
        target_sample_rate: u32,
    ) -> Result<StereoBuffer, AudioFileError> {
        use std::time::Instant;
        use symphonia::core::audio::SampleBuffer;

        let frame_count = self.total_frames as usize;
        let ch_offset = stem as usize * 2; // Vocals=0, Drums=2, Bass=4, Other=6

        let alloc_start = Instant::now();
        let mut buffer = StereoBuffer::silence(frame_count);
        log::debug!(
            "    [PERF] Allocated single stem {:?} in {:?} ({} frames)",
            stem, alloc_start.elapsed(), frame_count
        );

        let decode_start = Instant::now();
        let (mut format_reader, mut decoder, track_id) = self.create_decoder()?;

        let mut write_pos = 0usize;
        let mut sample_buf: Option<SampleBuffer<f32>> = None;

        loop {
            let packet = match format_reader.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(AudioFileError::IoError(format!("Decode error: {}", e))),
            };

            if packet.track_id() != track_id {
                continue;
            }

            let decoded = decoder.decode(&packet)
                .map_err(|e| AudioFileError::IoError(format!("Packet decode error: {}", e)))?;

            if sample_buf.is_none() {
                let spec = *decoded.spec();
                let duration = decoded.capacity() as u64;
                sample_buf = Some(SampleBuffer::new(duration, spec));
            }

            if let Some(ref mut buf) = sample_buf {
                buf.copy_interleaved_ref(decoded);
                let samples = buf.samples();
                let ch = 8;
                let packet_frames = samples.len() / ch;

                let frames_to_write = packet_frames.min(frame_count.saturating_sub(write_pos));
                let slice = buffer.as_mut_slice();
                for j in 0..frames_to_write {
                    let base = j * ch + ch_offset;
                    slice[write_pos + j] = StereoSample::new(samples[base], samples[base + 1]);
                }
                write_pos += frames_to_write;
            }
        }

        let decode_elapsed = decode_start.elapsed();
        let raw_bytes = frame_count * 8 * 2;
        log::info!(
            "    [PERF] Single-stem FLAC decode ({:?}): {:?} ({:.1} MB decoded, {:.1} MB/s effective)",
            stem, decode_elapsed,
            raw_bytes as f64 / 1_000_000.0,
            if decode_elapsed.as_secs_f64() > 0.0 {
                (raw_bytes as f64 / 1_000_000.0) / decode_elapsed.as_secs_f64()
            } else { 0.0 }
        );

        // Resample single stem if needed
        if self.format.sample_rate != target_sample_rate {
            log::info!(
                "    Resampling single stem {:?} from {} Hz to {} Hz",
                stem, self.format.sample_rate, target_sample_rate
            );
            let ratio = target_sample_rate as f64 / self.format.sample_rate as f64;
            let output_len = (frame_count as f64 * ratio).ceil() as usize;
            let mut resampled = StereoBuffer::silence(output_len);
            resample_stereo_buffer(
                &buffer, &mut resampled, self.format.sample_rate, target_sample_rate,
            )?;
            return Ok(resampled);
        }

        Ok(buffer)
    }

    /// Check whether this file needs resampling to match the target rate.
    pub fn needs_resampling(&self, target_rate: u32) -> bool {
        self.format.sample_rate != target_rate
    }

    /// Read a region of samples from the FLAC file into pre-allocated stems.
    ///
    /// Creates a new decoder, seeks to `file_start` frame, and decodes `count`
    /// frames into `stems` at offset `buffer_start`. All I/O is against in-memory
    /// data (no disk reads).
    ///
    /// Precondition: `buffer_start + count <= stems.len()`
    pub fn read_region_into(
        &self,
        stems: &mut StemBuffers,
        file_start: usize,
        buffer_start: usize,
        count: usize,
    ) -> Result<(), AudioFileError> {
        use symphonia::core::audio::SampleBuffer;
        use symphonia::core::formats::SeekTo;

        let (mut format_reader, mut decoder, track_id) = self.create_decoder()?;

        // Seek to the start frame (if not at beginning)
        if file_start > 0 {
            format_reader.seek(
                symphonia::core::formats::SeekMode::Accurate,
                SeekTo::TimeStamp {
                    ts: file_start as u64,
                    track_id,
                },
            ).map_err(|e| AudioFileError::IoError(format!("Seek failed: {}", e)))?;
        }

        // Decode `count` frames
        let mut frames_written = 0usize;
        let mut sample_buf: Option<SampleBuffer<f32>> = None;

        while frames_written < count {
            let packet = match format_reader.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(AudioFileError::IoError(format!("Decode error: {}", e))),
            };

            if packet.track_id() != track_id {
                continue;
            }

            let decoded = decoder.decode(&packet)
                .map_err(|e| AudioFileError::IoError(format!("Packet decode error: {}", e)))?;

            if sample_buf.is_none() {
                let spec = *decoded.spec();
                let duration = decoded.capacity() as u64;
                sample_buf = Some(SampleBuffer::new(duration, spec));
            }

            if let Some(ref mut buf) = sample_buf {
                buf.copy_interleaved_ref(decoded);
                let samples = buf.samples();
                let ch = 8;
                let packet_frames = samples.len() / ch;
                let frames_to_write = packet_frames.min(count - frames_written);

                for j in 0..frames_to_write {
                    let base = j * ch;
                    let i = buffer_start + frames_written + j;
                    stems.vocals.as_mut_slice()[i] = StereoSample::new(samples[base], samples[base + 1]);
                    stems.drums.as_mut_slice()[i] = StereoSample::new(samples[base + 2], samples[base + 3]);
                    stems.bass.as_mut_slice()[i] = StereoSample::new(samples[base + 4], samples[base + 5]);
                    stems.other.as_mut_slice()[i] = StereoSample::new(samples[base + 6], samples[base + 7]);
                }

                frames_written += frames_to_write;
            }
        }

        Ok(())
    }

    /// Decode a region of the FLAC file into a new owned StemBuffers.
    ///
    /// Creates a fresh decoder, seeks to `file_start`, and decodes `count` frames
    /// into a newly allocated StemBuffers of length `count`. Returns owned data
    /// suitable for parallel decoding — each thread gets its own buffer with no
    /// shared mutable state.
    ///
    /// This is the parallel-friendly counterpart to `read_region_into()`.
    pub fn decode_region(
        &self,
        file_start: usize,
        count: usize,
    ) -> Result<StemBuffers, AudioFileError> {
        use symphonia::core::audio::SampleBuffer;
        use symphonia::core::formats::SeekTo;

        let (mut format_reader, mut decoder, track_id) = self.create_decoder()?;

        // Seek to the start frame.
        // Symphonia seeks to the nearest sync point (FLAC frame boundary), which
        // may be before our target. We must skip the leading frames ourselves —
        // the decoder does NOT trim automatically.
        let mut skip_frames = 0usize;
        if file_start > 0 {
            let seeked = format_reader.seek(
                symphonia::core::formats::SeekMode::Accurate,
                SeekTo::TimeStamp {
                    ts: file_start as u64,
                    track_id,
                },
            ).map_err(|e| AudioFileError::IoError(format!("Seek failed: {}", e)))?;

            if seeked.actual_ts < seeked.required_ts {
                skip_frames = (seeked.required_ts - seeked.actual_ts) as usize;
                log::debug!(
                    "decode_region: skipping {} leading frames (seeked to {} for target {})",
                    skip_frames, seeked.actual_ts, seeked.required_ts
                );
            } else if seeked.actual_ts > seeked.required_ts {
                log::warn!(
                    "decode_region: seek overshot forward actual={} required={}",
                    seeked.actual_ts, seeked.required_ts
                );
            }
        }

        let mut stems = StemBuffers::with_length(count);
        let mut frames_written = 0usize;
        let mut sample_buf: Option<SampleBuffer<f32>> = None;

        while frames_written < count {
            let packet = match format_reader.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(AudioFileError::IoError(format!("Decode error: {}", e))),
            };

            if packet.track_id() != track_id {
                continue;
            }

            let decoded = decoder.decode(&packet)
                .map_err(|e| AudioFileError::IoError(format!("Packet decode error: {}", e)))?;

            if sample_buf.is_none() {
                let spec = *decoded.spec();
                let duration = decoded.capacity() as u64;
                sample_buf = Some(SampleBuffer::new(duration, spec));
            }

            if let Some(ref mut buf) = sample_buf {
                buf.copy_interleaved_ref(decoded);
                let samples = buf.samples();
                let ch = 8;
                let packet_frames = samples.len() / ch;

                // Skip leading frames from seek overshoot (sync point before target)
                let offset = if skip_frames > 0 {
                    let to_skip = skip_frames.min(packet_frames);
                    skip_frames -= to_skip;
                    to_skip
                } else {
                    0
                };

                let available = packet_frames - offset;
                let frames_to_write = available.min(count - frames_written);

                for j in 0..frames_to_write {
                    let base = (offset + j) * ch;
                    let i = frames_written + j;
                    stems.vocals.as_mut_slice()[i] = StereoSample::new(samples[base], samples[base + 1]);
                    stems.drums.as_mut_slice()[i] = StereoSample::new(samples[base + 2], samples[base + 3]);
                    stems.bass.as_mut_slice()[i] = StereoSample::new(samples[base + 4], samples[base + 5]);
                    stems.other.as_mut_slice()[i] = StereoSample::new(samples[base + 6], samples[base + 7]);
                }

                frames_written += frames_to_write;
            }
        }

        Ok(stems)
    }
}


/// Result of loading a single stem from a track file.
///
/// Much lighter than [`LoadedTrack`] — only allocates 1 stem buffer (~150 MB)
/// instead of 4 (~600 MB). Used by the linked stem loader which only needs
/// one stem from the source track.
pub struct SingleStemLoad {
    /// The decoded stem audio (stereo)
    pub buffer: StereoBuffer,
    /// Track metadata (BPM, key, cue points, drop marker, etc.)
    pub metadata: TrackMetadata,
    /// Duration in samples (capped at metadata duration to exclude FLAC padding)
    pub duration_samples: usize,
}

/// A fully loaded track ready for playback
///
/// Contains all audio data in memory plus metadata for DJ functionality.
/// Entire tracks are loaded into RAM for instant beat jumping.
///
/// ## Memory Sharing
///
/// The `stems` field uses `basedrop::Shared<StemBuffers>` to allow zero-copy sharing between:
/// - The audio engine (for playback)
/// - The UI (for waveform display)
///
/// This eliminates a 452MB clone per track load, reducing page faults by ~50%.
///
/// ## RT-Safe Deallocation
///
/// Unlike `Arc`, when a `Shared` is dropped on the audio thread, it doesn't
/// immediately free memory. Instead, it enqueues the pointer for collection
/// by a background GC thread. This prevents 100+ms deallocations from causing
/// audio underruns when replacing tracks.
pub struct LoadedTrack {
    /// Path to the source file
    pub path: std::path::PathBuf,
    /// Audio data for each stem (Shared for RT-safe deallocation)
    pub stems: basedrop::Shared<StemBuffers>,
    /// Track metadata (BPM, key, beat grid, cue points)
    pub metadata: TrackMetadata,
    /// Duration in samples
    pub duration_samples: usize,
    /// Duration in seconds
    pub duration_seconds: f64,
}

impl std::fmt::Debug for LoadedTrack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedTrack")
            .field("path", &self.path)
            .field("stems", &format!("<Shared<StemBuffers> {} frames>", self.stems.len()))
            .field("metadata", &self.metadata)
            .field("duration_samples", &self.duration_samples)
            .field("duration_seconds", &self.duration_seconds)
            .finish()
    }
}

impl LoadedTrack {
    /// Load a track from a file path with metadata from the database
    ///
    /// Uses the default system sample rate (48kHz).
    /// For sample-rate-aware loading, use `load_to(path, db, target_rate)` instead.
    pub fn load<P: AsRef<Path>>(path: P, db: &crate::db::DatabaseService) -> Result<Self, AudioFileError> {
        Self::load_to(path, db, SAMPLE_RATE)
    }

    /// Load a track from a file path, resampling to target sample rate
    ///
    /// # Arguments
    /// * `path` - Path to the audio file
    /// * `local_db` - Local database service (fallback when path isn't on USB)
    /// * `target_sample_rate` - Target sample rate (from the audio backend)
    ///
    /// Metadata is resolved automatically via [`resolve_track_metadata`], which
    /// handles both local and USB paths transparently. Callers never need to
    /// worry about which database to query or how to normalize paths.
    pub fn load_to<P: AsRef<Path>>(path: P, local_db: &crate::db::DatabaseService, target_sample_rate: u32) -> Result<Self, AudioFileError> {
        use std::time::Instant;

        let path_ref = path.as_ref();
        let meta_start = Instant::now();

        let metadata = resolve_track_metadata(path_ref, local_db);
        log::info!("  [PERF] Metadata loaded from DB in {:?}", meta_start.elapsed());

        // Delegate to load_with_metadata
        Self::load_with_metadata(path, metadata, target_sample_rate)
    }

    /// Load a track with pre-loaded metadata (DB-agnostic)
    ///
    /// # Arguments
    /// * `path` - Path to the audio file
    /// * `metadata` - Pre-loaded track metadata (from any source: local DB, USB DB, etc.)
    /// * `target_sample_rate` - Target sample rate (from the audio backend)
    ///
    /// This is the core loading function that doesn't depend on any database.
    /// The caller is responsible for loading metadata from the appropriate source.
    pub fn load_with_metadata<P: AsRef<Path>>(
        path: P,
        mut metadata: TrackMetadata,
        target_sample_rate: u32,
    ) -> Result<Self, AudioFileError> {
        use std::time::Instant;

        let path_ref = path.as_ref();
        let total_start = Instant::now();
        log::info!("[PERF] Loading track: {:?} (target rate: {} Hz)", path_ref, target_sample_rate);

        // Load the audio data (reads entire file into memory, then decodes)
        let open_start = Instant::now();
        let reader = AudioFileReader::open(path_ref)?;
        log::info!("  [PERF] File opened in {:?}", open_start.elapsed());

        // Get the source sample rate before reading (for metadata conversion)
        let source_sample_rate = reader.format().sample_rate;

        let stems_start = Instant::now();
        let stems = reader.read_all_stems_to(target_sample_rate)?;
        log::info!(
            "  [PERF] Audio data read in {:?} ({} frames, {:.1} MB)",
            stems_start.elapsed(),
            stems.len(),
            (stems.len() * 32) as f64 / 1_000_000.0 // 8 channels × 4 bytes per f32
        );

        // Cap at metadata-derived duration — FLAC header may include block-size padding
        let metadata_frames = metadata.duration_seconds
            .map(|d| (d * target_sample_rate as f64).round() as usize);
        let duration_samples = if let Some(mf) = metadata_frames {
            stems.len().min(mf)
        } else {
            stems.len()
        };
        let duration_seconds = duration_samples as f64 / target_sample_rate as f64;

        // If audio was resampled, scale all metadata sample positions to match
        // This ensures cue points, beat grid, loops, and drop markers remain accurate
        if source_sample_rate != target_sample_rate {
            metadata.scale_sample_positions(source_sample_rate, target_sample_rate);
            // Regenerate beat grid with correct samples-per-beat for target rate
            metadata.regenerate_beat_grid(duration_samples as u64, target_sample_rate);
        }

        // Wrap in Shared for RT-safe deallocation (defers drop to GC thread)
        let stems = basedrop::Shared::new(&crate::engine::gc::gc_handle(), stems);

        log::info!("[PERF] Total track load: {:?}", total_start.elapsed());

        Ok(Self {
            path: path_ref.to_path_buf(),
            stems,
            metadata,
            duration_samples,
            duration_seconds,
        })
    }

    /// Load only audio stems from a file (slow, loads all audio data)
    ///
    /// Uses the default system sample rate (48kHz).
    /// For sample-rate-aware loading, use `load_stems_to(path, target_rate)` instead.
    pub fn load_stems<P: AsRef<Path>>(path: P) -> Result<StemBuffers, AudioFileError> {
        Self::load_stems_to(path, SAMPLE_RATE)
    }

    /// Load only audio stems from a file, resampling to target sample rate
    ///
    /// # Arguments
    /// * `path` - Path to the audio file
    /// * `target_sample_rate` - Target sample rate (from the audio backend)
    ///
    /// This allows loading tracks to match whatever sample rate the audio system is running at.
    pub fn load_stems_to<P: AsRef<Path>>(path: P, target_sample_rate: u32) -> Result<StemBuffers, AudioFileError> {
        let reader = AudioFileReader::open(path.as_ref())?;
        reader.read_all_stems_to(target_sample_rate)
    }

    /// Load stems with parallel region decoding (up to 4 threads).
    ///
    /// Splits the file into equal regions and decodes each in parallel using
    /// `AudioFileReader::decode_region()`. Falls back to sequential decode if
    /// the file needs resampling or is very short (< 2 seconds).
    ///
    /// Uses the default system sample rate (48kHz).
    pub fn load_stems_parallel<P: AsRef<Path>>(path: P) -> Result<StemBuffers, AudioFileError> {
        Self::load_stems_parallel_to(path, SAMPLE_RATE)
    }

    /// Load stems with parallel region decoding, resampling to target sample rate.
    ///
    /// For native-rate files, splits into N regions and decodes in parallel.
    /// For files needing resampling, falls back to sequential (rubato needs
    /// contiguous blocks).
    pub fn load_stems_parallel_to<P: AsRef<Path>>(
        path: P,
        target_sample_rate: u32,
    ) -> Result<StemBuffers, AudioFileError> {
        let reader = AudioFileReader::open(path.as_ref())?;

        if reader.needs_resampling(target_sample_rate) {
            return reader.read_all_stems_to(target_sample_rate);
        }

        let frame_count = reader.frame_count() as usize;
        // Need at least 1s per region to be worthwhile
        let num_threads = 4usize.min(frame_count / 48000);
        if num_threads <= 1 {
            return reader.read_all_stems_to(target_sample_rate);
        }

        log::info!(
            "[PERF] Parallel stem load: {} frames across {} threads",
            frame_count, num_threads
        );
        let par_start = std::time::Instant::now();

        let region_size = frame_count / num_threads;
        let mut regions: Vec<StemBuffers> = Vec::with_capacity(num_threads);

        std::thread::scope(|s| {
            let handles: Vec<_> = (0..num_threads)
                .map(|i| {
                    let start = i * region_size;
                    let count = if i == num_threads - 1 {
                        frame_count - start
                    } else {
                        region_size
                    };
                    let r = &reader;
                    s.spawn(move || r.decode_region(start, count))
                })
                .collect();

            for handle in handles {
                regions.push(handle.join().expect("decode thread panicked")?);
            }
            Ok::<(), AudioFileError>(())
        })?;

        // Merge decoded regions into a single contiguous buffer
        let mut stems = StemBuffers::with_length(frame_count);
        let mut offset = 0;
        for region in &regions {
            let len = region.len();
            stems.copy_region_from(region, offset, len);
            offset += len;
        }

        log::info!(
            "[PERF] Parallel stem load complete: {:?} ({} threads)",
            par_start.elapsed(), num_threads
        );

        Ok(stems)
    }

    /// Load a single stem from a track file with automatic metadata resolution.
    ///
    /// This is the preferred entry point for linked stem loading. It only
    /// allocates and decodes the requested stem, using ~75% less memory than
    /// [`load_to`] which decodes all 4 stems.
    ///
    /// Metadata is resolved automatically via [`resolve_track_metadata`].
    pub fn load_single_stem_to<P: AsRef<Path>>(
        path: P,
        stem: crate::types::Stem,
        local_db: &crate::db::DatabaseService,
        target_sample_rate: u32,
    ) -> Result<SingleStemLoad, AudioFileError> {
        use std::time::Instant;

        let path_ref = path.as_ref();
        let total_start = Instant::now();

        let meta_start = Instant::now();
        let mut metadata = resolve_track_metadata(path_ref, local_db);
        log::info!("  [PERF] Metadata loaded from DB in {:?}", meta_start.elapsed());

        log::info!(
            "[PERF] Loading single stem {:?} from {:?} (target rate: {} Hz)",
            stem, path_ref, target_sample_rate
        );

        let open_start = Instant::now();
        let reader = AudioFileReader::open(path_ref)?;
        log::info!("  [PERF] File opened in {:?}", open_start.elapsed());

        let source_sample_rate = reader.format().sample_rate;

        let stem_start = Instant::now();
        let buffer = reader.read_single_stem_to(stem, target_sample_rate)?;
        log::info!(
            "  [PERF] Single stem decoded in {:?} ({} frames, {:.1} MB)",
            stem_start.elapsed(),
            buffer.len(),
            (buffer.len() * 8) as f64 / 1_000_000.0 // 2 channels × 4 bytes
        );

        // Cap at metadata-derived duration (FLAC padding)
        let metadata_frames = metadata.duration_seconds
            .map(|d| (d * target_sample_rate as f64).round() as usize);
        let duration_samples = if let Some(mf) = metadata_frames {
            buffer.len().min(mf)
        } else {
            buffer.len()
        };

        // Scale metadata sample positions if resampled
        if source_sample_rate != target_sample_rate {
            metadata.scale_sample_positions(source_sample_rate, target_sample_rate);
            metadata.regenerate_beat_grid(duration_samples as u64, target_sample_rate);
        }

        log::info!("[PERF] Total single-stem load: {:?}", total_start.elapsed());

        Ok(SingleStemLoad {
            buffer,
            metadata,
            duration_samples,
        })
    }

    /// Get the BPM of the track (or a default if not set)
    pub fn bpm(&self) -> f64 {
        self.metadata.bpm.unwrap_or(120.0)
    }

    /// Get the musical key (or "?" if unknown)
    pub fn key(&self) -> &str {
        self.metadata.key.as_deref().unwrap_or("?")
    }

    /// Get the filename without path
    pub fn filename(&self) -> &str {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
    }

    /// Get a formatted display name using parsed metadata
    ///
    /// Returns "{Artist} - {Title}" when both are available, "{Title}" when
    /// only title is set, or falls back to the filename without extension.
    pub fn display_name(&self) -> String {
        let title = self.metadata.title.as_deref().unwrap_or_else(|| {
            self.path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
        });
        match self.metadata.artist.as_deref() {
            Some(artist) if !artist.is_empty() => format!("{} - {}", artist, title),
            _ => title.to_string(),
        }
    }

    /// Get a cue point by index
    pub fn get_cue(&self, index: usize) -> Option<&CuePoint> {
        self.metadata.cue_points.get(index)
    }

    /// Get the number of cue points
    pub fn cue_count(&self) -> usize {
        self.metadata.cue_points.len()
    }

    /// Get the beat index at a sample position
    pub fn beat_at_sample(&self, sample: u64) -> Option<usize> {
        self.metadata.beat_grid.beat_at_sample(sample)
    }

    /// Get the sample position of a beat
    pub fn sample_at_beat(&self, beat: usize) -> Option<u64> {
        self.metadata.beat_grid.beats.get(beat).copied()
    }

    /// Get number of beats in the track
    pub fn beat_count(&self) -> usize {
        self.metadata.beat_grid.beats.len()
    }

    /// Calculate samples per beat at the track's BPM
    pub fn samples_per_beat(&self) -> f64 {
        let bpm = self.bpm();
        SAMPLE_RATE as f64 * 60.0 / bpm
    }

    /// Get estimated memory usage in bytes
    pub fn memory_usage(&self) -> usize {
        // 4 stems * 2 channels * 4 bytes per sample * num samples
        self.duration_samples * 4 * 2 * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_format_compatibility() {
        // 48kHz (default) should be compatible
        let format_48k = AudioFormat {
            format_tag: 1,
            channels: 8,
            sample_rate: 48000,
            bits_per_sample: 16,
            block_align: 16,
        };
        assert!(format_48k.is_compatible().is_ok());
        assert!(!format_48k.needs_resampling());

        // 44.1kHz should be compatible (will be resampled)
        let format_44k = AudioFormat {
            sample_rate: 44100,
            ..format_48k
        };
        assert!(format_44k.is_compatible().is_ok());
        assert!(format_44k.needs_resampling());

        // Wrong channels should fail
        let wrong_channels = AudioFormat {
            channels: 2,
            ..format_48k
        };
        assert!(matches!(
            wrong_channels.is_compatible(),
            Err(AudioFileError::WrongChannelCount { .. })
        ));

        // Unsupported sample rate (e.g., 22050) should fail
        let wrong_rate = AudioFormat {
            sample_rate: 22050,
            ..format_48k
        };
        assert!(matches!(
            wrong_rate.is_compatible(),
            Err(AudioFileError::WrongSampleRate { .. })
        ));
    }

    #[test]
    fn test_beat_grid() {
        let grid = BeatGrid::from_csv("0,22050,44100,66150");
        assert_eq!(grid.beats.len(), 4);
        assert_eq!(grid.beat_at_sample(10000), Some(0));
        assert_eq!(grid.beat_at_sample(30000), Some(1));
    }

    #[test]
    fn test_stem_buffers() {
        let mut stems = StemBuffers::with_length(100);
        assert_eq!(stems.len(), 100);
        assert_eq!(stems.get(Stem::Vocals).len(), 100);

        stems.get_mut(Stem::Drums).scale(0.5);
        assert_eq!(stems.drums.len(), 100);
    }

}
