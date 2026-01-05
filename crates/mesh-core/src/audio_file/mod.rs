//! RF64/BWF audio file handling
//!
//! This module handles reading 8-channel WAV/RF64 files containing stem-separated
//! audio (Vocals, Drums, Bass, Other as stereo pairs).

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

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
    /// Sample rate in Hz (should be 44100)
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
    pub fn is_compatible(&self) -> Result<(), AudioFileError> {
        if self.channels != STEM_CHANNEL_COUNT {
            return Err(AudioFileError::WrongChannelCount {
                expected: STEM_CHANNEL_COUNT,
                found: self.channels,
            });
        }
        if self.sample_rate != SAMPLE_RATE {
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
    /// BPM of the track
    pub bpm: Option<f64>,
    /// Original BPM (before any adjustments)
    pub original_bpm: Option<f64>,
    /// Musical key (e.g., "Am", "C#m")
    pub key: Option<String>,
    /// Beat grid
    pub beat_grid: BeatGrid,
    /// Cue points (up to 8)
    pub cue_points: Vec<CuePoint>,
    /// Pre-computed waveform preview (from wvfm chunk)
    pub waveform_preview: Option<WaveformPreview>,
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

impl TrackMetadata {
    /// Parse metadata from bext description string
    ///
    /// Format: `BPM:128.00|KEY:Am|FIRST_BEAT:14335|ORIGINAL_BPM:125.00`
    /// Legacy: `BPM:128.00|KEY:Am|GRID:0,22050,44100,...|ORIGINAL_BPM:125.00`
    ///
    /// Note: FIRST_BEAT is preferred over GRID as it stores only the first beat position,
    /// allowing the full beat grid to be regenerated from BPM when the track is loaded.
    pub fn parse_bext_description(description: &str) -> Self {
        let mut metadata = Self::default();
        let mut first_beat: Option<u64> = None;

        for part in description.split('|') {
            if let Some((key, value)) = part.split_once(':') {
                match key.trim() {
                    "BPM" => metadata.bpm = value.trim().parse().ok(),
                    "ORIGINAL_BPM" => metadata.original_bpm = value.trim().parse().ok(),
                    "KEY" => metadata.key = Some(value.trim().to_string()),
                    "FIRST_BEAT" => first_beat = value.trim().parse().ok(),
                    "GRID" => metadata.beat_grid = BeatGrid::from_csv(value),
                    _ => {}
                }
            }
        }

        // If we have FIRST_BEAT but no/empty GRID, store it for later regeneration
        if let Some(fb) = first_beat {
            metadata.beat_grid.first_beat_sample = Some(fb);
        }

        metadata
    }

    /// Parse metadata from bext description and regenerate beat grid from duration
    ///
    /// This is the preferred method when loading a track - it regenerates the full
    /// beat grid from BPM and first beat position using the known duration.
    pub fn parse_bext_description_with_duration(description: &str, duration_samples: u64) -> Self {
        let mut metadata = Self::parse_bext_description(description);

        // If we have first_beat_sample and BPM but empty/truncated beats, regenerate
        if let (Some(first_beat), Some(bpm)) = (metadata.beat_grid.first_beat_sample, metadata.bpm) {
            // Only regenerate if we have very few beats (likely truncated) or none
            if metadata.beat_grid.beats.len() < 50 {
                log::debug!(
                    "Regenerating beat grid: first_beat={}, bpm={:.1}, duration={}",
                    first_beat, bpm, duration_samples
                );
                metadata.beat_grid = BeatGrid::regenerate(first_beat, bpm, duration_samples);
            }
        }

        metadata
    }

    /// Serialize metadata to bext description string
    ///
    /// Format: `BPM:128.00|KEY:Am|FIRST_BEAT:14335|ORIGINAL_BPM:125.00`
    ///
    /// Uses FIRST_BEAT instead of full GRID to fit in 256-byte bext description limit.
    /// The full beat grid is regenerated from BPM + FIRST_BEAT when loading.
    pub fn to_bext_description(&self) -> String {
        let mut parts = Vec::new();

        if let Some(bpm) = self.bpm {
            parts.push(format!("BPM:{:.2}", bpm));
        }
        if let Some(ref key) = self.key {
            parts.push(format!("KEY:{}", key));
        }
        // Store only FIRST_BEAT (fits in 256 bytes, grid regenerated on load)
        let first_beat = self.beat_grid.first_beat_sample
            .or_else(|| self.beat_grid.beats.first().copied());
        if let Some(fb) = first_beat {
            parts.push(format!("FIRST_BEAT:{}", fb));
        }
        if let Some(original) = self.original_bpm {
            parts.push(format!("ORIGINAL_BPM:{:.2}", original));
        }

        parts.join("|")
    }
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
    pub fn read_all_stems(&mut self) -> Result<StemBuffers, AudioFileError> {
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

/// Read metadata from a WAV/RF64 file without loading audio data
///
/// Also calculates duration from the file header and regenerates the beat grid
/// from BPM + FIRST_BEAT if available.
pub fn read_metadata<P: AsRef<Path>>(path: P) -> Result<TrackMetadata, AudioFileError> {
    use std::time::Instant;
    let start = Instant::now();

    let file = File::open(path.as_ref())
        .map_err(|e| AudioFileError::IoError(e.to_string()))?;
    let mut reader = BufReader::new(file);

    // Read and validate header
    let mut header = [0u8; 12];
    reader.read_exact(&mut header)
        .map_err(|e| AudioFileError::IoError(e.to_string()))?;

    let is_rf64 = &header[0..4] == b"RF64";
    if &header[0..4] != b"RIFF" && !is_rf64 {
        return Err(AudioFileError::InvalidFormat("Not a RIFF/RF64 file".into()));
    }
    if &header[8..12] != b"WAVE" {
        return Err(AudioFileError::InvalidFormat("Not a WAVE file".into()));
    }

    let mut metadata = TrackMetadata::default();
    let mut cue_points: Vec<(u32, u64)> = Vec::new(); // id, position
    let mut bext_description: Option<String> = None;

    // Track format info for duration calculation
    let mut channels: u16 = 8;
    let mut bits_per_sample: u16 = 16;
    let mut data_size: u64 = 0;
    let mut waveform_preview: Option<WaveformPreview> = None;

    // Parse chunks
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
                // Format chunk - extract channel count and bit depth for duration calculation
                let mut fmt_data = vec![0u8; chunk_size as usize];
                reader.read_exact(&mut fmt_data)
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;

                if fmt_data.len() >= 16 {
                    channels = u16::from_le_bytes([fmt_data[2], fmt_data[3]]);
                    bits_per_sample = u16::from_le_bytes([fmt_data[14], fmt_data[15]]);
                }
            }
            b"data" => {
                // Data chunk - capture size for duration calculation
                data_size = chunk_size as u64;
                // Skip the audio data
                reader.seek(SeekFrom::Current(chunk_size as i64))
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;
            }
            b"bext" => {
                // Broadcast Extension chunk
                let mut bext_data = vec![0u8; chunk_size as usize];
                reader.read_exact(&mut bext_data)
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;

                // Description is first 256 bytes (null-terminated string)
                // Store it for later parsing with duration info
                if bext_data.len() >= 256 {
                    let description_end = bext_data[..256].iter()
                        .position(|&b| b == 0)
                        .unwrap_or(256);
                    if let Ok(description) = std::str::from_utf8(&bext_data[..description_end]) {
                        bext_description = Some(description.to_string());
                    }
                }
            }
            b"cue " => {
                // Cue points chunk
                let mut cue_data = vec![0u8; chunk_size as usize];
                reader.read_exact(&mut cue_data)
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;

                if cue_data.len() >= 4 {
                    let num_cues = u32::from_le_bytes([cue_data[0], cue_data[1], cue_data[2], cue_data[3]]);

                    // Each cue point is 24 bytes
                    for i in 0..num_cues as usize {
                        let offset = 4 + i * 24;
                        if offset + 24 <= cue_data.len() {
                            let cue_id = u32::from_le_bytes([
                                cue_data[offset], cue_data[offset + 1],
                                cue_data[offset + 2], cue_data[offset + 3]
                            ]);
                            let sample_pos = u32::from_le_bytes([
                                cue_data[offset + 20], cue_data[offset + 21],
                                cue_data[offset + 22], cue_data[offset + 23]
                            ]);
                            cue_points.push((cue_id, sample_pos as u64));
                        }
                    }
                }
            }
            b"LIST" => {
                // LIST chunk (may contain adtl with cue labels)
                let mut list_data = vec![0u8; chunk_size as usize];
                reader.read_exact(&mut list_data)
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;

                if list_data.len() >= 4 && &list_data[0..4] == b"adtl" {
                    // Parse adtl sub-chunks for labels
                    let mut pos = 4;
                    while pos + 8 <= list_data.len() {
                        let sub_id = &list_data[pos..pos + 4];
                        let sub_size = u32::from_le_bytes([
                            list_data[pos + 4], list_data[pos + 5],
                            list_data[pos + 6], list_data[pos + 7]
                        ]) as usize;

                        if sub_id == b"labl" && pos + 8 + sub_size <= list_data.len() {
                            // Label sub-chunk
                            let cue_id = u32::from_le_bytes([
                                list_data[pos + 8], list_data[pos + 9],
                                list_data[pos + 10], list_data[pos + 11]
                            ]);

                            // Find the cue point and add label
                            let label_end = list_data[pos + 12..pos + 8 + sub_size]
                                .iter()
                                .position(|&b| b == 0)
                                .unwrap_or(sub_size - 4);

                            if let Ok(label_str) = std::str::from_utf8(&list_data[pos + 12..pos + 12 + label_end]) {
                                // Parse label format: "Drop|color:#FF5500"
                                let (label, color) = if let Some((l, c)) = label_str.split_once("|color:") {
                                    (l.to_string(), Some(c.to_string()))
                                } else {
                                    (label_str.to_string(), None)
                                };

                                // Find matching cue point
                                if let Some((_, pos)) = cue_points.iter().find(|(id, _)| *id == cue_id) {
                                    metadata.cue_points.push(CuePoint {
                                        index: metadata.cue_points.len() as u8,
                                        sample_position: *pos,
                                        label,
                                        color,
                                    });
                                }
                            }
                        }

                        pos += 8 + sub_size;
                        if sub_size % 2 != 0 {
                            pos += 1;
                        }
                    }
                }
            }
            b"wvfm" => {
                // Waveform preview chunk
                let mut wvfm_data = vec![0u8; chunk_size as usize];
                reader.read_exact(&mut wvfm_data)
                    .map_err(|e| AudioFileError::IoError(e.to_string()))?;

                match parse_wvfm_chunk(&wvfm_data) {
                    Ok(preview) => {
                        log::debug!("Loaded waveform preview: {}px width, 4 stems", preview.width);
                        waveform_preview = Some(preview);
                    }
                    Err(e) => {
                        log::warn!("Failed to parse wvfm chunk: {}", e);
                    }
                }
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
    log::debug!("    [PERF] Chunk parsing complete: {:?}", start.elapsed());

    // Calculate duration in samples from format info
    let bytes_per_sample = (bits_per_sample / 8) as u64;
    let bytes_per_frame = channels as u64 * bytes_per_sample;
    let duration_samples = if bytes_per_frame > 0 {
        data_size / bytes_per_frame
    } else {
        0
    };

    // Parse bext description with duration to regenerate beat grid
    if let Some(description) = bext_description {
        metadata = TrackMetadata::parse_bext_description_with_duration(&description, duration_samples);
    }
    log::debug!("    [PERF] Beat grid regenerated: {:?}", start.elapsed());

    // Add any cue points without labels
    for (id, pos) in cue_points {
        if !metadata.cue_points.iter().any(|c| c.sample_position == pos) {
            metadata.cue_points.push(CuePoint {
                index: metadata.cue_points.len() as u8,
                sample_position: pos,
                label: format!("Cue {}", id),
                color: None,
            });
        }
    }

    // Sort cue points by position
    metadata.cue_points.sort_by_key(|c| c.sample_position);

    // Re-index after sorting
    for (i, cue) in metadata.cue_points.iter_mut().enumerate() {
        cue.index = i as u8;
    }

    // Attach waveform preview if present
    metadata.waveform_preview = waveform_preview;

    Ok(metadata)
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
    /// Load a track from a file path
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, AudioFileError> {
        use std::time::Instant;

        let path_ref = path.as_ref();
        let total_start = Instant::now();
        log::info!("[PERF] Loading track: {:?}", path_ref);

        // Read metadata first (fast, doesn't load audio)
        let meta_start = Instant::now();
        let metadata = read_metadata(path_ref)?;
        log::info!("  [PERF] Metadata loaded in {:?}", meta_start.elapsed());

        // Then read the audio data
        let open_start = Instant::now();
        let mut reader = AudioFileReader::open(path_ref)?;
        log::info!("  [PERF] File opened in {:?}", open_start.elapsed());

        let stems_start = Instant::now();
        let stems = reader.read_all_stems()?;
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

    /// Load only metadata from a file (fast, no audio data)
    ///
    /// Use this for quick UI display while audio loads in background.
    pub fn load_metadata_only<P: AsRef<Path>>(path: P) -> Result<TrackMetadata, AudioFileError> {
        read_metadata(path.as_ref())
    }

    /// Load only audio stems from a file (slow, loads all audio data)
    ///
    /// Use this after metadata is loaded to get the actual audio.
    pub fn load_stems<P: AsRef<Path>>(path: P) -> Result<StemBuffers, AudioFileError> {
        let mut reader = AudioFileReader::open(path.as_ref())?;
        reader.read_all_stems()
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
        let valid_format = AudioFormat {
            format_tag: 1,
            channels: 8,
            sample_rate: 44100,
            bits_per_sample: 16,
            block_align: 16,
        };
        assert!(valid_format.is_compatible().is_ok());

        let wrong_channels = AudioFormat {
            channels: 2,
            ..valid_format
        };
        assert!(matches!(
            wrong_channels.is_compatible(),
            Err(AudioFileError::WrongChannelCount { .. })
        ));

        let wrong_rate = AudioFormat {
            sample_rate: 48000,
            ..valid_format
        };
        assert!(matches!(
            wrong_rate.is_compatible(),
            Err(AudioFileError::WrongSampleRate { .. })
        ));
    }

    #[test]
    fn test_metadata_parsing() {
        let description = "BPM:128.00|KEY:Am|GRID:0,22050,44100|ORIGINAL_BPM:125.00";
        let metadata = TrackMetadata::parse_bext_description(description);

        assert_eq!(metadata.bpm, Some(128.0));
        assert_eq!(metadata.original_bpm, Some(125.0));
        assert_eq!(metadata.key, Some("Am".to_string()));
        assert_eq!(metadata.beat_grid.beats, vec![0, 22050, 44100]);
    }

    #[test]
    fn test_metadata_roundtrip() {
        // Create metadata with first_beat_sample set
        let original = TrackMetadata {
            bpm: Some(174.5),
            original_bpm: Some(172.0),
            key: Some("Dm".to_string()),
            beat_grid: BeatGrid::from_csv("0,11025,22050"),
            cue_points: Vec::new(),
            waveform_preview: None,
        };

        // Serialize to bext description (now uses FIRST_BEAT format)
        let description = original.to_bext_description();

        // Should contain FIRST_BEAT instead of full GRID
        assert!(description.contains("FIRST_BEAT:0"));
        assert!(!description.contains("GRID:"));

        // Parse back - without duration, beats won't be regenerated
        let parsed = TrackMetadata::parse_bext_description(&description);

        // Verify basic roundtrip
        assert_eq!(parsed.bpm, original.bpm);
        assert_eq!(parsed.original_bpm, original.original_bpm);
        assert_eq!(parsed.key, original.key);
        // first_beat_sample is preserved
        assert_eq!(parsed.beat_grid.first_beat_sample, Some(0));
    }

    #[test]
    fn test_metadata_roundtrip_with_duration() {
        // Create metadata
        let original = TrackMetadata {
            bpm: Some(120.0), // 120 BPM = 22050 samples per beat at 44100
            original_bpm: Some(120.0),
            key: Some("Am".to_string()),
            beat_grid: BeatGrid::from_csv("0,22050,44100,66150"),
            cue_points: Vec::new(),
            waveform_preview: None,
        };

        // Serialize to bext description
        let description = original.to_bext_description();

        // Parse with duration to regenerate beat grid
        let duration_samples = 100000; // About 2.3 seconds
        let parsed = TrackMetadata::parse_bext_description_with_duration(&description, duration_samples);

        // Beat grid should be regenerated
        assert_eq!(parsed.bpm, Some(120.0));
        assert_eq!(parsed.beat_grid.first_beat_sample, Some(0));
        assert!(!parsed.beat_grid.beats.is_empty());
        // Should have approximately 4-5 beats for 100000 samples at 120 BPM
        assert!(parsed.beat_grid.beats.len() >= 4);
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
