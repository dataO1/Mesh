//! RF64/BWF audio file handling
//!
//! This module handles reading 8-channel WAV/RF64 files containing stem-separated
//! audio (Vocals, Drums, Bass, Other as stereo pairs).
//!
//! Supports automatic resampling of legacy 44.1kHz files to 48kHz.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use rubato::{FftFixedInOut, Resampler};

use crate::types::{StereoBuffer, StereoSample, Stem, SAMPLE_RATE};

/// Maximum file size for standard WAV (4GB - 8 bytes for RIFF header)
#[allow(dead_code)]
const WAV_MAX_SIZE: u64 = 0xFFFF_FFFF - 8;

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
    pub fn regenerate(first_beat_sample: u64, bpm: f64, duration_samples: u64) -> Self {
        use crate::types::SAMPLE_RATE;

        if bpm <= 0.0 || duration_samples == 0 {
            return Self::new();
        }

        let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;
        if samples_per_beat == 0 {
            return Self::new();
        }

        let num_beats = ((duration_samples.saturating_sub(first_beat_sample)) / samples_per_beat) as usize;

        let beats: Vec<u64> = (0..=num_beats)
            .map(|i| first_beat_sample + (i as u64 * samples_per_beat))
            .collect();

        Self {
            beats,
            first_beat_sample: Some(first_beat_sample),
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
    /// Pre-computed waveform preview (from wvfm chunk)
    pub waveform_preview: Option<WaveformPreview>,
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

impl From<crate::db::LoadedTrackMetadata> for TrackMetadata {
    fn from(db_meta: crate::db::LoadedTrackMetadata) -> Self {
        let track = &db_meta.track;

        // Regenerate beat grid from first_beat_sample and BPM
        let beat_grid = if let Some(bpm) = track.bpm {
            let duration_samples = (track.duration_seconds * crate::types::SAMPLE_RATE as f64) as u64;
            BeatGrid::regenerate(track.first_beat_sample as u64, bpm, duration_samples)
        } else {
            BeatGrid::default()
        };

        Self {
            artist: track.artist.clone(),
            bpm: track.bpm,
            original_bpm: track.original_bpm,
            key: track.key.clone(),
            lufs: track.lufs,
            duration_seconds: Some(track.duration_seconds),
            beat_grid,
            cue_points: db_meta.cue_points.into_iter().map(Into::into).collect(),
            saved_loops: db_meta.saved_loops.into_iter().map(Into::into).collect(),
            waveform_preview: None, // No longer stored in WAV files
            drop_marker: track.drop_marker.map(|d| d as u64),
            stem_links: Vec::new(), // Stem links handled separately via track IDs
        }
    }
}

/// Quantized waveform peaks for a single stem
///
/// Values are quantized from f32 [-1.0, 1.0] to u8 [0, 255] for compact storage.
/// Use `quantize_peak()` and `dequantize_peak()` for conversion.
#[derive(Debug, Clone, Default)]
pub struct StemPeaks {
    /// Quantized minimum values (0-255, maps to -1.0 to 1.0)
    pub min: Vec<u8>,
    /// Quantized maximum values (0-255, maps to -1.0 to 1.0)
    pub max: Vec<u8>,
}

impl StemPeaks {
    /// Create empty stem peaks
    pub fn new() -> Self {
        Self::default()
    }

    /// Create stem peaks with given capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            min: Vec::with_capacity(capacity),
            max: Vec::with_capacity(capacity),
        }
    }
}

/// Pre-computed waveform preview stored in WAV file
///
/// This stores quantized min/max peaks for each stem at a fixed resolution (e.g., 800 pixels),
/// allowing instant waveform display without recomputing from audio samples.
///
/// # Format
/// Stored in a custom `wvfm` chunk in the WAV file:
/// - Version: 1 byte
/// - Width: 2 bytes (u16 LE)
/// - Stems: 1 byte (always 4)
/// - Reserved: 1 byte
/// - Data: For each stem, width×2 bytes (min array then max array)
#[derive(Debug, Clone)]
pub struct WaveformPreview {
    /// Width in pixels (number of peak pairs per stem)
    pub width: u16,
    /// Peaks for each stem [Vocals, Drums, Bass, Other]
    pub stems: [StemPeaks; 4],
}

impl Default for WaveformPreview {
    fn default() -> Self {
        Self {
            width: 0,
            stems: Default::default(),
        }
    }
}

impl WaveformPreview {
    /// Standard preview width (matches display resolution)
    pub const STANDARD_WIDTH: u16 = 800;

    /// Create an empty waveform preview
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the preview has data
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.stems[0].min.is_empty()
    }

    /// Extract a single stem's peaks, dequantized to f32
    ///
    /// Used for linked stem visualization - extract just the stem we need
    /// from the source track's pre-computed waveform preview.
    ///
    /// Returns a Vec of (min, max) pairs ready for rendering.
    /// Returns empty Vec if stem_idx is out of range.
    pub fn extract_stem_peaks(&self, stem_idx: usize) -> Vec<(f32, f32)> {
        if stem_idx >= 4 {
            return Vec::new();
        }
        let stem = &self.stems[stem_idx];
        stem.min
            .iter()
            .zip(stem.max.iter())
            .map(|(&min, &max)| (dequantize_peak(min), dequantize_peak(max)))
            .collect()
    }
}

/// Quantize a peak value from f32 [-1.0, 1.0] to u8 [0, 255]
pub fn quantize_peak(value: f32) -> u8 {
    ((value.clamp(-1.0, 1.0) + 1.0) * 127.5) as u8
}

/// Dequantize a peak value from u8 [0, 255] to f32 [-1.0, 1.0]
pub fn dequantize_peak(byte: u8) -> f32 {
    (byte as f32 / 127.5) - 1.0
}

/// Parse a wvfm chunk into a WaveformPreview
///
/// # Format
/// - [0]     Version: u8 (1)
/// - [1-2]   Width: u16 LE
/// - [3]     Stems: u8 (4)
/// - [4]     Reserved: u8
/// - [5..]   Data: For each stem, width×2 bytes (min array then max array)
pub fn parse_wvfm_chunk(data: &[u8]) -> Result<WaveformPreview, AudioFileError> {
    const HEADER_SIZE: usize = 5;

    if data.len() < HEADER_SIZE {
        return Err(AudioFileError::Corrupted("wvfm chunk too small".into()));
    }

    let version = data[0];
    if version != 1 {
        return Err(AudioFileError::InvalidFormat(
            format!("Unsupported wvfm version: {}", version)
        ));
    }

    let width = u16::from_le_bytes([data[1], data[2]]);
    let num_stems = data[3];

    if num_stems != 4 {
        return Err(AudioFileError::InvalidFormat(
            format!("Expected 4 stems, found {}", num_stems)
        ));
    }

    // Expected data size: 4 stems × width × 2 (min + max arrays)
    let expected_data_size = HEADER_SIZE + (4 * width as usize * 2);
    if data.len() < expected_data_size {
        return Err(AudioFileError::Corrupted(
            format!("wvfm chunk truncated: expected {} bytes, found {}", expected_data_size, data.len())
        ));
    }

    let mut preview = WaveformPreview {
        width,
        stems: Default::default(),
    };

    // Parse each stem's data
    let mut offset = HEADER_SIZE;
    for stem in &mut preview.stems {
        // Read min array
        stem.min = data[offset..offset + width as usize].to_vec();
        offset += width as usize;

        // Read max array
        stem.max = data[offset..offset + width as usize].to_vec();
        offset += width as usize;
    }

    Ok(preview)
}

/// Serialize a WaveformPreview to bytes for storage in wvfm chunk
///
/// Returns the raw bytes (without chunk ID and size header - caller adds those)
pub fn serialize_wvfm_chunk(preview: &WaveformPreview) -> Vec<u8> {
    const HEADER_SIZE: usize = 5;
    let data_size = 4 * preview.width as usize * 2;
    let mut bytes = Vec::with_capacity(HEADER_SIZE + data_size);

    // Header
    bytes.push(1); // Version
    bytes.extend_from_slice(&preview.width.to_le_bytes()); // Width
    bytes.push(4); // Stems (always 4)
    bytes.push(0); // Reserved

    // Data: for each stem, write min array then max array
    for stem in &preview.stems {
        bytes.extend_from_slice(&stem.min);
        bytes.extend_from_slice(&stem.max);
    }

    bytes
}

/// Read only the waveform preview from a WAV file
///
/// This efficiently scans the file for the `wvfm` chunk without loading
/// audio data, making it suitable for quick waveform display updates.
///
/// Returns `Ok(None)` if the file has no wvfm chunk (not an error).
pub fn read_waveform_preview_from_file<P: AsRef<Path>>(path: P) -> Result<Option<WaveformPreview>, AudioFileError> {
    let file = File::open(path.as_ref())
        .map_err(|e| AudioFileError::IoError(e.to_string()))?;
    let mut reader = BufReader::new(file);

    // Read RIFF/RF64 header
    let mut riff_id = [0u8; 4];
    reader.read_exact(&mut riff_id)
        .map_err(|e| AudioFileError::IoError(e.to_string()))?;

    let is_rf64 = match &riff_id {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return Err(AudioFileError::InvalidFormat("Not a RIFF/RF64 file".into())),
    };

    // Skip file size (4 bytes)
    reader.seek(SeekFrom::Current(4))
        .map_err(|e| AudioFileError::IoError(e.to_string()))?;

    // Read WAVE identifier
    let mut wave_id = [0u8; 4];
    reader.read_exact(&mut wave_id)
        .map_err(|e| AudioFileError::IoError(e.to_string()))?;

    if &wave_id != b"WAVE" {
        return Err(AudioFileError::InvalidFormat("Not a WAVE file".into()));
    }

    // For RF64, skip ds64 chunk if present
    if is_rf64 {
        let mut chunk_id = [0u8; 4];
        if reader.read_exact(&mut chunk_id).is_ok() && &chunk_id == b"ds64" {
            let mut chunk_size = [0u8; 4];
            reader.read_exact(&mut chunk_size)
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;
            let chunk_size = u32::from_le_bytes(chunk_size);
            reader.seek(SeekFrom::Current(chunk_size as i64))
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;
        } else {
            // Seek back if not ds64
            reader.seek(SeekFrom::Current(-4))
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;
        }
    }

    // Scan chunks looking for wvfm
    loop {
        let mut chunk_id = [0u8; 4];
        if reader.read_exact(&mut chunk_id).is_err() {
            break; // End of file
        }

        let mut chunk_size_bytes = [0u8; 4];
        reader.read_exact(&mut chunk_size_bytes)
            .map_err(|e| AudioFileError::IoError(e.to_string()))?;
        let chunk_size = u32::from_le_bytes(chunk_size_bytes);

        if &chunk_id == b"wvfm" {
            // Found wvfm chunk - read and parse it
            let mut wvfm_data = vec![0u8; chunk_size as usize];
            reader.read_exact(&mut wvfm_data)
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;
            return Ok(Some(parse_wvfm_chunk(&wvfm_data)?));
        }

        // Skip this chunk
        reader.seek(SeekFrom::Current(chunk_size as i64))
            .map_err(|e| AudioFileError::IoError(e.to_string()))?;

        // Pad to word boundary
        if chunk_size % 2 != 0 {
            reader.seek(SeekFrom::Current(1))
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;
        }
    }

    // No wvfm chunk found
    Ok(None)
}

/// Parse a mlop (mesh loops) chunk into a Vec<SavedLoop>
///
/// Format:
/// - num_loops (4 bytes, u32 LE)
/// - For each loop:
///   - index (1 byte)
///   - start_sample (8 bytes, u64 LE)
///   - end_sample (8 bytes, u64 LE)
///   - label_len (2 bytes, u16 LE)
///   - label (label_len bytes, UTF-8)
///   - color_len (2 bytes, u16 LE)
///   - color (color_len bytes, UTF-8, or empty if 0)
pub fn parse_mlop_chunk(data: &[u8]) -> Result<Vec<SavedLoop>, AudioFileError> {
    if data.len() < 4 {
        return Err(AudioFileError::Corrupted("mlop chunk too small".into()));
    }

    let num_loops = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let mut loops = Vec::with_capacity(num_loops);
    let mut pos = 4;

    for _ in 0..num_loops {
        // index (1 byte)
        if pos >= data.len() {
            break;
        }
        let index = data[pos];
        pos += 1;

        // start_sample (8 bytes)
        if pos + 8 > data.len() {
            break;
        }
        let start_sample = u64::from_le_bytes([
            data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
        ]);
        pos += 8;

        // end_sample (8 bytes)
        if pos + 8 > data.len() {
            break;
        }
        let end_sample = u64::from_le_bytes([
            data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
        ]);
        pos += 8;

        // label_len (2 bytes)
        if pos + 2 > data.len() {
            break;
        }
        let label_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;

        // label
        if pos + label_len > data.len() {
            break;
        }
        let label = String::from_utf8_lossy(&data[pos..pos + label_len]).to_string();
        pos += label_len;

        // color_len (2 bytes)
        if pos + 2 > data.len() {
            break;
        }
        let color_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;

        // color
        let color = if color_len > 0 && pos + color_len <= data.len() {
            let c = String::from_utf8_lossy(&data[pos..pos + color_len]).to_string();
            pos += color_len;
            Some(c)
        } else {
            None
        };

        loops.push(SavedLoop {
            index,
            start_sample,
            end_sample,
            label,
            color,
        });
    }

    Ok(loops)
}

/// Parse a mslk (mesh stem links) chunk into a Vec<StemLinkReference>
///
/// Format:
/// - version (1 byte, currently 1)
/// - num_links (1 byte)
/// - For each link:
///   - stem_index (1 byte, 0=Vocals, 1=Drums, 2=Bass, 3=Other)
///   - source_stem (1 byte)
///   - source_drop_marker (8 bytes, u64 LE)
///   - path_len (2 bytes, u16 LE)
///   - path (path_len bytes, UTF-8)
pub fn parse_mslk_chunk(data: &[u8]) -> Result<Vec<StemLinkReference>, AudioFileError> {
    if data.len() < 2 {
        return Err(AudioFileError::Corrupted("mslk chunk too small".into()));
    }

    let version = data[0];
    if version != 1 {
        return Err(AudioFileError::Corrupted(format!("Unknown mslk version: {}", version)));
    }

    let num_links = data[1] as usize;
    let mut links = Vec::with_capacity(num_links);
    let mut pos = 2;

    for _ in 0..num_links {
        // stem_index (1 byte)
        if pos >= data.len() {
            break;
        }
        let stem_index = data[pos];
        pos += 1;

        // source_stem (1 byte)
        if pos >= data.len() {
            break;
        }
        let source_stem = data[pos];
        pos += 1;

        // source_drop_marker (8 bytes)
        if pos + 8 > data.len() {
            break;
        }
        let source_drop_marker = u64::from_le_bytes([
            data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
        ]);
        pos += 8;

        // path_len (2 bytes)
        if pos + 2 > data.len() {
            break;
        }
        let path_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;

        // path
        if pos + path_len > data.len() {
            break;
        }
        let path_str = String::from_utf8_lossy(&data[pos..pos + path_len]).to_string();
        let source_path = std::path::PathBuf::from(path_str);
        pos += path_len;

        links.push(StemLinkReference {
            stem_index,
            source_path,
            source_stem,
            source_drop_marker,
        });
    }

    Ok(links)
}

/// Serialize stem links to bytes for storage in mslk chunk
///
/// Returns the raw bytes (without chunk ID and size header - caller adds those)
pub fn serialize_mslk_chunk(links: &[StemLinkReference]) -> Vec<u8> {
    let mut bytes = Vec::new();

    // Version
    bytes.push(1);

    // Number of links (max 255)
    bytes.push(links.len().min(255) as u8);

    // Each link
    for link in links.iter().take(255) {
        bytes.push(link.stem_index);
        bytes.push(link.source_stem);
        bytes.extend_from_slice(&link.source_drop_marker.to_le_bytes());

        let path_str = link.source_path.to_string_lossy();
        let path_bytes = path_str.as_bytes();
        let path_len = path_bytes.len().min(65535) as u16;
        bytes.extend_from_slice(&path_len.to_le_bytes());
        bytes.extend_from_slice(&path_bytes[..path_len as usize]);
    }

    bytes
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
    /// This prevents page fault storms from blocking the JACK RT thread.
    ///
    /// ## Why Sequential?
    ///
    /// The previous parallel allocation (via Rayon) triggered ~452,000 page faults
    /// simultaneously across 4 threads. This overwhelmed the kernel's page fault
    /// handler, causing scheduling delays that blocked the JACK RT thread.
    ///
    /// Sequential allocation with yields:
    /// - 113K faults → yield → 113K faults → yield → ...
    /// - Each yield gives the JACK RT thread a chance to run
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

/// WAV/RF64 file reader
pub struct AudioFileReader {
    reader: BufReader<File>,
    format: AudioFormat,
    data_offset: u64,
    data_size: u64,
    /// True if file is RF64 (supports >4GB files)
    #[allow(dead_code)]
    is_rf64: bool,
}

impl AudioFileReader {
    /// Open an audio file for reading
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, AudioFileError> {
        let file = File::open(path.as_ref())
            .map_err(|e| AudioFileError::IoError(e.to_string()))?;
        let mut reader = BufReader::new(file);

        // Read RIFF/RF64 header
        let mut riff_id = [0u8; 4];
        reader.read_exact(&mut riff_id)
            .map_err(|e| AudioFileError::IoError(e.to_string()))?;

        let is_rf64 = match &riff_id {
            b"RIFF" => false,
            b"RF64" => true,
            _ => return Err(AudioFileError::InvalidFormat("Not a RIFF/RF64 file".into())),
        };

        // Read file size (placeholder for RF64)
        let mut size_bytes = [0u8; 4];
        reader.read_exact(&mut size_bytes)
            .map_err(|e| AudioFileError::IoError(e.to_string()))?;

        // Read WAVE identifier
        let mut wave_id = [0u8; 4];
        reader.read_exact(&mut wave_id)
            .map_err(|e| AudioFileError::IoError(e.to_string()))?;

        if &wave_id != b"WAVE" {
            return Err(AudioFileError::InvalidFormat("Not a WAVE file".into()));
        }

        // For RF64, read the ds64 chunk first to get actual sizes
        let mut actual_data_size: Option<u64> = None;
        if is_rf64 {
            // ds64 chunk should be first after WAVE
            let mut chunk_id = [0u8; 4];
            reader.read_exact(&mut chunk_id)
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;

            if &chunk_id == b"ds64" {
                let mut chunk_size = [0u8; 4];
                reader.read_exact(&mut chunk_size)
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;
                let chunk_size = u32::from_le_bytes(chunk_size);

                // Read ds64 content
                let mut ds64_data = vec![0u8; chunk_size as usize];
                reader.read_exact(&mut ds64_data)
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;

                if ds64_data.len() >= 16 {
                    // Skip riff_size (8 bytes), read data_size (8 bytes)
                    let data_size_bytes: [u8; 8] = ds64_data[8..16].try_into().unwrap();
                    actual_data_size = Some(u64::from_le_bytes(data_size_bytes));
                }
            } else {
                // Seek back if not ds64
                reader.seek(SeekFrom::Current(-4))
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;
            }
        }

        // Find fmt and data chunks
        let mut format: Option<AudioFormat> = None;
        let mut data_offset: Option<u64> = None;
        let mut data_size: Option<u64> = actual_data_size;

        loop {
            let mut chunk_id = [0u8; 4];
            if reader.read_exact(&mut chunk_id).is_err() {
                break;
            }

            let mut chunk_size_bytes = [0u8; 4];
            reader.read_exact(&mut chunk_size_bytes)
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;
            let chunk_size = u32::from_le_bytes(chunk_size_bytes);

            match &chunk_id {
                b"fmt " => {
                    format = Some(Self::read_fmt_chunk(&mut reader, chunk_size)?);
                }
                b"data" => {
                    data_offset = Some(reader.stream_position()
                        .map_err(|e| AudioFileError::IoError(e.to_string()))?);

                    // For standard WAV, use chunk size; for RF64, we already have it
                    if data_size.is_none() {
                        data_size = Some(chunk_size as u64);
                    }

                    // Skip past data to continue parsing (for metadata chunks)
                    let skip_size = if is_rf64 && actual_data_size.is_some() {
                        actual_data_size.unwrap()
                    } else {
                        chunk_size as u64
                    };
                    reader.seek(SeekFrom::Current(skip_size as i64))
                        .map_err(|e| AudioFileError::IoError(e.to_string()))?;
                }
                _ => {
                    // Skip unknown chunks
                    reader.seek(SeekFrom::Current(chunk_size as i64))
                        .map_err(|e| AudioFileError::IoError(e.to_string()))?;
                }
            }

            // Pad to word boundary
            if chunk_size % 2 != 0 {
                reader.seek(SeekFrom::Current(1))
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;
            }
        }

        let format = format.ok_or(AudioFileError::MissingChunk("fmt"))?;
        let data_offset = data_offset.ok_or(AudioFileError::MissingChunk("data"))?;
        let data_size = data_size.ok_or(AudioFileError::MissingChunk("data"))?;

        // Validate format
        format.is_compatible()?;

        Ok(Self {
            reader,
            format,
            data_offset,
            data_size,
            is_rf64,
        })
    }

    /// Read the fmt chunk
    fn read_fmt_chunk(reader: &mut BufReader<File>, size: u32) -> Result<AudioFormat, AudioFileError> {
        if size < 16 {
            return Err(AudioFileError::Corrupted("fmt chunk too small".into()));
        }

        let mut fmt_data = vec![0u8; size as usize];
        reader.read_exact(&mut fmt_data)
            .map_err(|e| AudioFileError::IoError(e.to_string()))?;

        let format_tag = u16::from_le_bytes([fmt_data[0], fmt_data[1]]);
        let channels = u16::from_le_bytes([fmt_data[2], fmt_data[3]]);
        let sample_rate = u32::from_le_bytes([fmt_data[4], fmt_data[5], fmt_data[6], fmt_data[7]]);
        let _byte_rate = u32::from_le_bytes([fmt_data[8], fmt_data[9], fmt_data[10], fmt_data[11]]);
        let block_align = u16::from_le_bytes([fmt_data[12], fmt_data[13]]);
        let bits_per_sample = u16::from_le_bytes([fmt_data[14], fmt_data[15]]);

        Ok(AudioFormat {
            format_tag,
            channels,
            sample_rate,
            bits_per_sample,
            block_align,
        })
    }

    /// Get the audio format
    pub fn format(&self) -> &AudioFormat {
        &self.format
    }

    /// Get the number of sample frames in the file
    pub fn frame_count(&self) -> u64 {
        self.data_size / self.format.block_align as u64
    }

    /// Get the duration in seconds
    pub fn duration_seconds(&self) -> f64 {
        self.frame_count() as f64 / self.format.sample_rate as f64
    }

    /// Read all audio data into stem buffers
    ///
    /// This uses the default target sample rate (SAMPLE_RATE constant, 48kHz).
    /// For JACK-aware loading, use `read_all_stems_to(target_rate)` instead.
    pub fn read_all_stems(&mut self) -> Result<StemBuffers, AudioFileError> {
        self.read_all_stems_to(SAMPLE_RATE)
    }

    /// Read all audio data into stem buffers, resampling to target rate
    ///
    /// # Arguments
    /// * `target_sample_rate` - The target sample rate (typically JACK's sample rate)
    ///
    /// This allows loading tracks to match whatever sample rate JACK is running at.
    /// If the file's sample rate differs from target, audio is automatically resampled.
    pub fn read_all_stems_to(&mut self, target_sample_rate: u32) -> Result<StemBuffers, AudioFileError> {
        use std::time::Instant;

        let frame_count = self.frame_count() as usize;

        // Allocation timing
        let alloc_start = Instant::now();
        let mut stems = StemBuffers::with_length(frame_count);
        log::debug!(
            "    [PERF] Buffer allocation: {:?} ({} frames)",
            alloc_start.elapsed(),
            frame_count
        );

        // Seek timing
        let seek_start = Instant::now();
        self.reader.seek(SeekFrom::Start(self.data_offset))
            .map_err(|e| AudioFileError::IoError(e.to_string()))?;
        log::debug!("    [PERF] Seek to data: {:?}", seek_start.elapsed());

        // Read timing
        let read_start = Instant::now();
        match self.format.bits_per_sample {
            16 => self.read_16bit_samples(&mut stems, frame_count)?,
            24 => self.read_24bit_samples(&mut stems, frame_count)?,
            32 => {
                if self.format.format_tag == 3 {
                    self.read_32bit_float_samples(&mut stems, frame_count)?;
                } else {
                    self.read_32bit_int_samples(&mut stems, frame_count)?;
                }
            }
            _ => return Err(AudioFileError::UnsupportedBitDepth(self.format.bits_per_sample)),
        }
        let read_elapsed = read_start.elapsed();
        let bytes_read = frame_count * 8 * (self.format.bits_per_sample as usize / 8);
        let throughput_mb_s = if read_elapsed.as_secs_f64() > 0.0 {
            (bytes_read as f64 / 1_000_000.0) / read_elapsed.as_secs_f64()
        } else {
            0.0
        };
        log::info!(
            "    [PERF] Audio read: {:?} ({:.1} MB, {:.1} MB/s)",
            read_elapsed,
            bytes_read as f64 / 1_000_000.0,
            throughput_mb_s
        );

        // Resample if file sample rate differs from target rate (e.g., 48kHz -> 44.1kHz for JACK)
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

    /// Read 16-bit samples using chunked I/O for better performance
    ///
    /// Reads data in 1MB chunks instead of per-frame to reduce syscall overhead.
    fn read_16bit_samples(&mut self, stems: &mut StemBuffers, frame_count: usize) -> Result<(), AudioFileError> {
        const BYTES_PER_FRAME: usize = 16; // 8 channels * 2 bytes
        const CHUNK_FRAMES: usize = 65536; // 64K frames = 1MB chunks
        const SCALE: f32 = 1.0 / 32768.0;

        let mut chunk_buffer = vec![0u8; CHUNK_FRAMES * BYTES_PER_FRAME];

        let mut frames_read = 0;
        while frames_read < frame_count {
            let frames_this_chunk = CHUNK_FRAMES.min(frame_count - frames_read);
            let bytes_to_read = frames_this_chunk * BYTES_PER_FRAME;

            self.reader.read_exact(&mut chunk_buffer[..bytes_to_read])
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;

            // Convert chunk with good cache locality
            for j in 0..frames_this_chunk {
                let offset = j * BYTES_PER_FRAME;
                let i = frames_read + j;

                // Channel order: Vocals L/R, Drums L/R, Bass L/R, Other L/R
                let vocals_l = i16::from_le_bytes([chunk_buffer[offset], chunk_buffer[offset + 1]]) as f32 * SCALE;
                let vocals_r = i16::from_le_bytes([chunk_buffer[offset + 2], chunk_buffer[offset + 3]]) as f32 * SCALE;
                let drums_l = i16::from_le_bytes([chunk_buffer[offset + 4], chunk_buffer[offset + 5]]) as f32 * SCALE;
                let drums_r = i16::from_le_bytes([chunk_buffer[offset + 6], chunk_buffer[offset + 7]]) as f32 * SCALE;
                let bass_l = i16::from_le_bytes([chunk_buffer[offset + 8], chunk_buffer[offset + 9]]) as f32 * SCALE;
                let bass_r = i16::from_le_bytes([chunk_buffer[offset + 10], chunk_buffer[offset + 11]]) as f32 * SCALE;
                let other_l = i16::from_le_bytes([chunk_buffer[offset + 12], chunk_buffer[offset + 13]]) as f32 * SCALE;
                let other_r = i16::from_le_bytes([chunk_buffer[offset + 14], chunk_buffer[offset + 15]]) as f32 * SCALE;

                stems.vocals.as_mut_slice()[i] = StereoSample::new(vocals_l, vocals_r);
                stems.drums.as_mut_slice()[i] = StereoSample::new(drums_l, drums_r);
                stems.bass.as_mut_slice()[i] = StereoSample::new(bass_l, bass_r);
                stems.other.as_mut_slice()[i] = StereoSample::new(other_l, other_r);
            }

            frames_read += frames_this_chunk;
        }

        Ok(())
    }

    /// Read 24-bit samples using chunked I/O for better performance
    ///
    /// Reads data in ~1MB chunks instead of per-frame to reduce syscall overhead.
    fn read_24bit_samples(&mut self, stems: &mut StemBuffers, frame_count: usize) -> Result<(), AudioFileError> {
        const BYTES_PER_FRAME: usize = 24; // 8 channels * 3 bytes
        const CHUNK_FRAMES: usize = 43008; // ~1MB chunks (43008 * 24 = 1,032,192 bytes)
        const SCALE: f32 = 1.0 / 8388608.0; // 2^23

        let mut chunk_buffer = vec![0u8; CHUNK_FRAMES * BYTES_PER_FRAME];

        // Convert 24-bit to i32 (sign-extend)
        let to_i32 = |b: &[u8]| -> i32 {
            let val = (b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16);
            if val & 0x800000 != 0 {
                val | !0xFFFFFF // Sign extend
            } else {
                val
            }
        };

        let mut frames_read = 0;
        while frames_read < frame_count {
            let frames_this_chunk = CHUNK_FRAMES.min(frame_count - frames_read);
            let bytes_to_read = frames_this_chunk * BYTES_PER_FRAME;

            self.reader.read_exact(&mut chunk_buffer[..bytes_to_read])
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;

            // Convert chunk with good cache locality
            for j in 0..frames_this_chunk {
                let offset = j * BYTES_PER_FRAME;
                let i = frames_read + j;

                let vocals_l = to_i32(&chunk_buffer[offset..offset + 3]) as f32 * SCALE;
                let vocals_r = to_i32(&chunk_buffer[offset + 3..offset + 6]) as f32 * SCALE;
                let drums_l = to_i32(&chunk_buffer[offset + 6..offset + 9]) as f32 * SCALE;
                let drums_r = to_i32(&chunk_buffer[offset + 9..offset + 12]) as f32 * SCALE;
                let bass_l = to_i32(&chunk_buffer[offset + 12..offset + 15]) as f32 * SCALE;
                let bass_r = to_i32(&chunk_buffer[offset + 15..offset + 18]) as f32 * SCALE;
                let other_l = to_i32(&chunk_buffer[offset + 18..offset + 21]) as f32 * SCALE;
                let other_r = to_i32(&chunk_buffer[offset + 21..offset + 24]) as f32 * SCALE;

                stems.vocals.as_mut_slice()[i] = StereoSample::new(vocals_l, vocals_r);
                stems.drums.as_mut_slice()[i] = StereoSample::new(drums_l, drums_r);
                stems.bass.as_mut_slice()[i] = StereoSample::new(bass_l, bass_r);
                stems.other.as_mut_slice()[i] = StereoSample::new(other_l, other_r);
            }

            frames_read += frames_this_chunk;
        }

        Ok(())
    }

    /// Read 32-bit float samples using chunked I/O for better performance
    ///
    /// Reads data in 1MB chunks instead of per-frame to reduce syscall overhead.
    fn read_32bit_float_samples(&mut self, stems: &mut StemBuffers, frame_count: usize) -> Result<(), AudioFileError> {
        const BYTES_PER_FRAME: usize = 32; // 8 channels * 4 bytes
        const CHUNK_FRAMES: usize = 32768; // 1MB chunks (32768 * 32 = 1,048,576 bytes)

        let mut chunk_buffer = vec![0u8; CHUNK_FRAMES * BYTES_PER_FRAME];

        let mut frames_read = 0;
        while frames_read < frame_count {
            let frames_this_chunk = CHUNK_FRAMES.min(frame_count - frames_read);
            let bytes_to_read = frames_this_chunk * BYTES_PER_FRAME;

            self.reader.read_exact(&mut chunk_buffer[..bytes_to_read])
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;

            // Convert chunk with good cache locality
            for j in 0..frames_this_chunk {
                let offset = j * BYTES_PER_FRAME;
                let i = frames_read + j;

                let vocals_l = f32::from_le_bytes([chunk_buffer[offset], chunk_buffer[offset + 1], chunk_buffer[offset + 2], chunk_buffer[offset + 3]]);
                let vocals_r = f32::from_le_bytes([chunk_buffer[offset + 4], chunk_buffer[offset + 5], chunk_buffer[offset + 6], chunk_buffer[offset + 7]]);
                let drums_l = f32::from_le_bytes([chunk_buffer[offset + 8], chunk_buffer[offset + 9], chunk_buffer[offset + 10], chunk_buffer[offset + 11]]);
                let drums_r = f32::from_le_bytes([chunk_buffer[offset + 12], chunk_buffer[offset + 13], chunk_buffer[offset + 14], chunk_buffer[offset + 15]]);
                let bass_l = f32::from_le_bytes([chunk_buffer[offset + 16], chunk_buffer[offset + 17], chunk_buffer[offset + 18], chunk_buffer[offset + 19]]);
                let bass_r = f32::from_le_bytes([chunk_buffer[offset + 20], chunk_buffer[offset + 21], chunk_buffer[offset + 22], chunk_buffer[offset + 23]]);
                let other_l = f32::from_le_bytes([chunk_buffer[offset + 24], chunk_buffer[offset + 25], chunk_buffer[offset + 26], chunk_buffer[offset + 27]]);
                let other_r = f32::from_le_bytes([chunk_buffer[offset + 28], chunk_buffer[offset + 29], chunk_buffer[offset + 30], chunk_buffer[offset + 31]]);

                stems.vocals.as_mut_slice()[i] = StereoSample::new(vocals_l, vocals_r);
                stems.drums.as_mut_slice()[i] = StereoSample::new(drums_l, drums_r);
                stems.bass.as_mut_slice()[i] = StereoSample::new(bass_l, bass_r);
                stems.other.as_mut_slice()[i] = StereoSample::new(other_l, other_r);
            }

            frames_read += frames_this_chunk;
        }

        Ok(())
    }

    /// Read 32-bit integer samples using chunked I/O for better performance
    ///
    /// Reads data in 1MB chunks instead of per-frame to reduce syscall overhead.
    fn read_32bit_int_samples(&mut self, stems: &mut StemBuffers, frame_count: usize) -> Result<(), AudioFileError> {
        const BYTES_PER_FRAME: usize = 32; // 8 channels * 4 bytes
        const CHUNK_FRAMES: usize = 32768; // 1MB chunks (32768 * 32 = 1,048,576 bytes)
        const SCALE: f32 = 1.0 / 2147483648.0; // 2^31

        let mut chunk_buffer = vec![0u8; CHUNK_FRAMES * BYTES_PER_FRAME];

        let mut frames_read = 0;
        while frames_read < frame_count {
            let frames_this_chunk = CHUNK_FRAMES.min(frame_count - frames_read);
            let bytes_to_read = frames_this_chunk * BYTES_PER_FRAME;

            self.reader.read_exact(&mut chunk_buffer[..bytes_to_read])
                .map_err(|e| AudioFileError::IoError(e.to_string()))?;

            // Convert chunk with good cache locality
            for j in 0..frames_this_chunk {
                let offset = j * BYTES_PER_FRAME;
                let i = frames_read + j;

                let vocals_l = i32::from_le_bytes([chunk_buffer[offset], chunk_buffer[offset + 1], chunk_buffer[offset + 2], chunk_buffer[offset + 3]]) as f32 * SCALE;
                let vocals_r = i32::from_le_bytes([chunk_buffer[offset + 4], chunk_buffer[offset + 5], chunk_buffer[offset + 6], chunk_buffer[offset + 7]]) as f32 * SCALE;
                let drums_l = i32::from_le_bytes([chunk_buffer[offset + 8], chunk_buffer[offset + 9], chunk_buffer[offset + 10], chunk_buffer[offset + 11]]) as f32 * SCALE;
                let drums_r = i32::from_le_bytes([chunk_buffer[offset + 12], chunk_buffer[offset + 13], chunk_buffer[offset + 14], chunk_buffer[offset + 15]]) as f32 * SCALE;
                let bass_l = i32::from_le_bytes([chunk_buffer[offset + 16], chunk_buffer[offset + 17], chunk_buffer[offset + 18], chunk_buffer[offset + 19]]) as f32 * SCALE;
                let bass_r = i32::from_le_bytes([chunk_buffer[offset + 20], chunk_buffer[offset + 21], chunk_buffer[offset + 22], chunk_buffer[offset + 23]]) as f32 * SCALE;
                let other_l = i32::from_le_bytes([chunk_buffer[offset + 24], chunk_buffer[offset + 25], chunk_buffer[offset + 26], chunk_buffer[offset + 27]]) as f32 * SCALE;
                let other_r = i32::from_le_bytes([chunk_buffer[offset + 28], chunk_buffer[offset + 29], chunk_buffer[offset + 30], chunk_buffer[offset + 31]]) as f32 * SCALE;

                stems.vocals.as_mut_slice()[i] = StereoSample::new(vocals_l, vocals_r);
                stems.drums.as_mut_slice()[i] = StereoSample::new(drums_l, drums_r);
                stems.bass.as_mut_slice()[i] = StereoSample::new(bass_l, bass_r);
                stems.other.as_mut_slice()[i] = StereoSample::new(other_l, other_r);
            }

            frames_read += frames_this_chunk;
        }

        Ok(())
    }
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
/// JACK xruns when replacing tracks.
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
    /// For JACK-aware loading, use `load_to(path, db, target_rate)` instead.
    pub fn load<P: AsRef<Path>>(path: P, db: &crate::db::DatabaseService) -> Result<Self, AudioFileError> {
        Self::load_to(path, db, SAMPLE_RATE)
    }

    /// Load a track from a file path, resampling to target sample rate
    ///
    /// # Arguments
    /// * `path` - Path to the audio file
    /// * `db` - Database service for loading metadata
    /// * `target_sample_rate` - Target sample rate (typically JACK's sample rate)
    ///
    /// This allows loading tracks to match whatever sample rate JACK is running at.
    /// Metadata (BPM, key, cue points, etc.) is loaded from the database.
    /// Waveform preview is loaded from the WAV file's wvfm chunk.
    pub fn load_to<P: AsRef<Path>>(path: P, db: &crate::db::DatabaseService, target_sample_rate: u32) -> Result<Self, AudioFileError> {
        use std::time::Instant;

        let path_ref = path.as_ref();
        let meta_start = Instant::now();
        let path_str = path_ref.to_string_lossy().to_string();

        // Load metadata from database
        let metadata: TrackMetadata = match db.load_track_metadata_by_path(&path_str) {
            Ok(Some(db_meta)) => db_meta.into(),
            Ok(None) => {
                log::warn!("Track not found in database: {}, using default metadata", path_str);
                TrackMetadata::default()
            }
            Err(e) => {
                log::warn!("Failed to load metadata from database: {}, using default", e);
                TrackMetadata::default()
            }
        };
        log::info!("  [PERF] Metadata loaded from DB in {:?}", meta_start.elapsed());

        // Delegate to load_with_metadata
        Self::load_with_metadata(path, metadata, target_sample_rate)
    }

    /// Load a track with pre-loaded metadata (DB-agnostic)
    ///
    /// # Arguments
    /// * `path` - Path to the audio file
    /// * `metadata` - Pre-loaded track metadata (from any source: local DB, USB DB, etc.)
    /// * `target_sample_rate` - Target sample rate (typically JACK's sample rate)
    ///
    /// This is the core loading function that doesn't depend on any database.
    /// The caller is responsible for loading metadata from the appropriate source.
    /// Waveform preview is loaded from the WAV file's wvfm chunk.
    pub fn load_with_metadata<P: AsRef<Path>>(
        path: P,
        mut metadata: TrackMetadata,
        target_sample_rate: u32,
    ) -> Result<Self, AudioFileError> {
        use std::time::Instant;

        let path_ref = path.as_ref();
        let total_start = Instant::now();
        log::info!("[PERF] Loading track: {:?} (target rate: {} Hz)", path_ref, target_sample_rate);

        // Load waveform preview from WAV file (stored in wvfm chunk)
        let wvfm_start = Instant::now();
        match read_waveform_preview_from_file(path_ref) {
            Ok(Some(waveform)) => {
                metadata.waveform_preview = Some(waveform);
                log::info!("  [PERF] Waveform preview loaded from WAV in {:?}", wvfm_start.elapsed());
            }
            Ok(None) => {
                log::debug!("  No wvfm chunk found in {:?}", path_ref);
            }
            Err(e) => {
                log::warn!("  Failed to read waveform preview: {}", e);
            }
        }

        // Load the audio data
        let open_start = Instant::now();
        let mut reader = AudioFileReader::open(path_ref)?;
        log::info!("  [PERF] File opened in {:?}", open_start.elapsed());

        let stems_start = Instant::now();
        let stems = reader.read_all_stems_to(target_sample_rate)?;
        log::info!(
            "  [PERF] Audio data read in {:?} ({} frames, {:.1} MB)",
            stems_start.elapsed(),
            stems.len(),
            (stems.len() * 32) as f64 / 1_000_000.0 // 8 channels × 4 bytes per f32
        );

        let duration_samples = stems.len();
        let duration_seconds = stems.duration_seconds();

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
    /// For JACK-aware loading, use `load_stems_to(path, target_rate)` instead.
    pub fn load_stems<P: AsRef<Path>>(path: P) -> Result<StemBuffers, AudioFileError> {
        Self::load_stems_to(path, SAMPLE_RATE)
    }

    /// Load only audio stems from a file, resampling to target sample rate
    ///
    /// # Arguments
    /// * `path` - Path to the audio file
    /// * `target_sample_rate` - Target sample rate (typically JACK's sample rate)
    ///
    /// This allows loading tracks to match whatever sample rate JACK is running at.
    pub fn load_stems_to<P: AsRef<Path>>(path: P, target_sample_rate: u32) -> Result<StemBuffers, AudioFileError> {
        let mut reader = AudioFileReader::open(path.as_ref())?;
        reader.read_all_stems_to(target_sample_rate)
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

    #[test]
    fn test_mslk_chunk_roundtrip() {
        use std::path::PathBuf;

        // Create stem links
        let original = vec![
            StemLinkReference {
                stem_index: 0, // Vocals
                source_path: PathBuf::from("/music/linked_track.wav"),
                source_stem: 1, // Drums from source
                source_drop_marker: 1234567,
            },
            StemLinkReference {
                stem_index: 2, // Bass
                source_path: PathBuf::from("/music/another_track.wav"),
                source_stem: 2, // Bass from source
                source_drop_marker: 9876543,
            },
        ];

        // Serialize to bytes
        let bytes = serialize_mslk_chunk(&original);

        // Parse back
        let parsed = parse_mslk_chunk(&bytes).expect("Failed to parse mslk chunk");

        // Verify roundtrip
        assert_eq!(parsed.len(), 2);

        assert_eq!(parsed[0].stem_index, 0);
        assert_eq!(parsed[0].source_path, PathBuf::from("/music/linked_track.wav"));
        assert_eq!(parsed[0].source_stem, 1);
        assert_eq!(parsed[0].source_drop_marker, 1234567);

        assert_eq!(parsed[1].stem_index, 2);
        assert_eq!(parsed[1].source_path, PathBuf::from("/music/another_track.wav"));
        assert_eq!(parsed[1].source_stem, 2);
        assert_eq!(parsed[1].source_drop_marker, 9876543);
    }

    #[test]
    fn test_mslk_chunk_empty() {
        let empty: Vec<StemLinkReference> = Vec::new();
        let bytes = serialize_mslk_chunk(&empty);
        let parsed = parse_mslk_chunk(&bytes).expect("Failed to parse empty mslk chunk");
        assert!(parsed.is_empty());
    }
}
