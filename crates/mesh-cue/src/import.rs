//! Stem file import module
//!
//! Imports 4 separate stereo WAV files (Vocals, Drums, Bass, Other)
//! and combines them into a single StemBuffers structure.
//!
//! The importer tracks the source sample rate from the input files,
//! allowing proper resampling during export.

use anyhow::{bail, Context, Result};
use mesh_core::audio_file::StemBuffers;
use mesh_core::types::StereoSample;
use std::path::Path;

/// Result of stem import containing buffers and source sample rate
#[derive(Debug)]
pub struct ImportedStems {
    /// The combined stem audio buffers
    pub buffers: StemBuffers,
    /// Source sample rate from the input WAV files (e.g., 44100 Hz from demucs)
    pub source_sample_rate: u32,
}

/// Stem file importer
#[derive(Debug, Clone)]
pub struct StemImporter {
    /// Path to vocals stem (stereo WAV)
    pub vocals_path: Option<std::path::PathBuf>,
    /// Path to drums stem (stereo WAV)
    pub drums_path: Option<std::path::PathBuf>,
    /// Path to bass stem (stereo WAV)
    pub bass_path: Option<std::path::PathBuf>,
    /// Path to other stem (stereo WAV)
    pub other_path: Option<std::path::PathBuf>,
}

impl Default for StemImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl StemImporter {
    /// Create a new stem importer
    pub fn new() -> Self {
        Self {
            vocals_path: None,
            drums_path: None,
            bass_path: None,
            other_path: None,
        }
    }

    /// Set the vocals stem path
    pub fn set_vocals(&mut self, path: impl AsRef<Path>) {
        self.vocals_path = Some(path.as_ref().to_path_buf());
    }

    /// Set the drums stem path
    pub fn set_drums(&mut self, path: impl AsRef<Path>) {
        self.drums_path = Some(path.as_ref().to_path_buf());
    }

    /// Set the bass stem path
    pub fn set_bass(&mut self, path: impl AsRef<Path>) {
        self.bass_path = Some(path.as_ref().to_path_buf());
    }

    /// Set the other stem path
    pub fn set_other(&mut self, path: impl AsRef<Path>) {
        self.other_path = Some(path.as_ref().to_path_buf());
    }

    /// Check if all stems are loaded
    pub fn is_complete(&self) -> bool {
        self.vocals_path.is_some()
            && self.drums_path.is_some()
            && self.bass_path.is_some()
            && self.other_path.is_some()
    }

    /// Get the number of loaded stems (0-4)
    pub fn loaded_count(&self) -> usize {
        [
            self.vocals_path.is_some(),
            self.drums_path.is_some(),
            self.bass_path.is_some(),
            self.other_path.is_some(),
        ]
        .iter()
        .filter(|&&b| b)
        .count()
    }

    /// Clear all loaded stems
    pub fn clear(&mut self) {
        self.vocals_path = None;
        self.drums_path = None;
        self.bass_path = None;
        self.other_path = None;
    }

    /// Import all stems and combine into StemBuffers
    ///
    /// This loads all 4 stem files, validates they are compatible
    /// (same length, same sample rate), and interleaves them into
    /// the 8-channel format used by mesh-player.
    ///
    /// Returns `ImportedStems` containing the buffers and the source sample rate,
    /// which is needed for proper resampling during export.
    pub fn import(&self) -> Result<ImportedStems> {
        log::info!("import: Starting stem import");

        if !self.is_complete() {
            log::error!("import: Not all stems are loaded");
            bail!("Not all stems are loaded");
        }

        let vocals_path = self.vocals_path.as_ref().unwrap();
        let drums_path = self.drums_path.as_ref().unwrap();
        let bass_path = self.bass_path.as_ref().unwrap();
        let other_path = self.other_path.as_ref().unwrap();

        log::info!("import: Loading vocals from {:?}", vocals_path);
        let (vocals, vocals_rate) = load_stereo_wav(vocals_path)
            .with_context(|| format!("Failed to load vocals: {:?}", vocals_path))?;
        log::info!("import: Vocals loaded: {} samples @ {} Hz", vocals.len(), vocals_rate);

        log::info!("import: Loading drums from {:?}", drums_path);
        let (drums, drums_rate) = load_stereo_wav(drums_path)
            .with_context(|| format!("Failed to load drums: {:?}", drums_path))?;
        log::info!("import: Drums loaded: {} samples @ {} Hz", drums.len(), drums_rate);

        log::info!("import: Loading bass from {:?}", bass_path);
        let (bass, bass_rate) = load_stereo_wav(bass_path)
            .with_context(|| format!("Failed to load bass: {:?}", bass_path))?;
        log::info!("import: Bass loaded: {} samples @ {} Hz", bass.len(), bass_rate);

        log::info!("import: Loading other from {:?}", other_path);
        let (other, other_rate) = load_stereo_wav(other_path)
            .with_context(|| format!("Failed to load other: {:?}", other_path))?;
        log::info!("import: Other loaded: {} samples @ {} Hz", other.len(), other_rate);

        // Validate all stems have the same sample rate
        let source_sample_rate = vocals_rate;
        if drums_rate != source_sample_rate || bass_rate != source_sample_rate || other_rate != source_sample_rate {
            log::error!(
                "import: Stem files have different sample rates: vocals={}, drums={}, bass={}, other={}",
                vocals_rate, drums_rate, bass_rate, other_rate
            );
            bail!(
                "Stem files have different sample rates: vocals={}, drums={}, bass={}, other={}",
                vocals_rate, drums_rate, bass_rate, other_rate
            );
        }

        // Validate all stems have the same length
        let len = vocals.len();
        if drums.len() != len || bass.len() != len || other.len() != len {
            log::error!(
                "import: Stem files have different lengths: vocals={}, drums={}, bass={}, other={}",
                vocals.len(),
                drums.len(),
                bass.len(),
                other.len()
            );
            bail!(
                "Stem files have different lengths: vocals={}, drums={}, bass={}, other={}",
                vocals.len(),
                drums.len(),
                bass.len(),
                other.len()
            );
        }

        log::info!("import: All stems validated, {} samples each @ {} Hz", len, source_sample_rate);

        // Combine into StemBuffers
        log::info!("import: Combining into StemBuffers...");
        let mut buffers = StemBuffers::with_length(len);

        for i in 0..len {
            buffers.vocals.as_mut_slice()[i] = vocals[i];
            buffers.drums.as_mut_slice()[i] = drums[i];
            buffers.bass.as_mut_slice()[i] = bass[i];
            buffers.other.as_mut_slice()[i] = other[i];
        }

        log::info!("import: Complete, created StemBuffers with {} samples @ {} Hz", buffers.len(), source_sample_rate);
        Ok(ImportedStems {
            buffers,
            source_sample_rate,
        })
    }

    /// Get mono-summed audio for analysis
    ///
    /// Combines all stems into a single mono channel for BPM/key analysis.
    pub fn get_mono_sum(&self) -> Result<Vec<f32>> {
        log::info!("get_mono_sum: Creating mono mix for analysis");
        let imported = self.import()?;
        let buffers = &imported.buffers;
        let len = buffers.len();

        log::info!("get_mono_sum: Summing {} samples to mono", len);
        let mono: Vec<f32> = (0..len)
            .map(|i| {
                // Sum all stems and convert to mono
                let vocals_mono = (buffers.vocals[i].left + buffers.vocals[i].right) * 0.5;
                let drums_mono = (buffers.drums[i].left + buffers.drums[i].right) * 0.5;
                let bass_mono = (buffers.bass[i].left + buffers.bass[i].right) * 0.5;
                let other_mono = (buffers.other[i].left + buffers.other[i].right) * 0.5;

                // Mix all stems (with some headroom)
                (vocals_mono + drums_mono + bass_mono + other_mono) * 0.25
            })
            .collect();

        log::info!("get_mono_sum: Complete, {} mono samples", mono.len());
        Ok(mono)
    }

    /// Get drums-only mono audio for BPM analysis
    ///
    /// Drums typically have the clearest beat for tempo detection,
    /// providing more accurate BPM results than analyzing the full mix.
    pub fn get_drums_mono(&self) -> Result<Vec<f32>> {
        log::info!("get_drums_mono: Loading drums stem for BPM analysis");
        let imported = self.import()?;
        let buffers = &imported.buffers;
        let len = buffers.len();

        log::info!("get_drums_mono: Converting {} stereo samples to mono", len);
        let mono: Vec<f32> = (0..len)
            .map(|i| {
                // Convert drums stereo to mono
                (buffers.drums[i].left + buffers.drums[i].right) * 0.5
            })
            .collect();

        log::info!("get_drums_mono: Complete, {} mono samples", mono.len());
        Ok(mono)
    }
}

/// Load a stereo WAV file and return sample pairs with the source sample rate
fn load_stereo_wav(path: &Path) -> Result<(Vec<StereoSample>, u32)> {
    use std::io::{Read, Seek, SeekFrom};

    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);

    // Read RIFF header (12 bytes)
    let mut header = [0u8; 12];
    reader
        .read_exact(&mut header)
        .context("Failed to read WAV header")?;

    // Validate RIFF/WAVE header
    let is_riff = &header[0..4] == b"RIFF" || &header[0..4] == b"RF64";
    if !is_riff {
        bail!("Not a RIFF/RF64 file");
    }
    if &header[8..12] != b"WAVE" {
        bail!("Not a WAVE file");
    }

    // Parse chunks to find fmt and data
    let mut format: Option<WavFormat> = None;
    let mut data_offset: Option<u64> = None;
    let mut data_size: Option<u64> = None;

    loop {
        // Read chunk header (8 bytes)
        let mut chunk_header = [0u8; 8];
        if reader.read_exact(&mut chunk_header).is_err() {
            break; // End of file
        }

        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes([
            chunk_header[4],
            chunk_header[5],
            chunk_header[6],
            chunk_header[7],
        ]);

        match chunk_id {
            b"fmt " => {
                format = Some(read_fmt_chunk(&mut reader, chunk_size)?);
            }
            b"data" => {
                data_offset = Some(reader.stream_position()?);
                data_size = Some(chunk_size as u64);
                // Don't skip data chunk - we'll read it after finding format
                break;
            }
            _ => {
                // Skip unknown chunks
                reader.seek(SeekFrom::Current(chunk_size as i64))?;
            }
        }

        // Word-align (chunks are padded to even boundaries)
        if chunk_size % 2 != 0 {
            reader.seek(SeekFrom::Current(1))?;
        }
    }

    // Validate we found the required chunks
    let format = format.context("Missing fmt chunk")?;
    let data_offset = data_offset.context("Missing data chunk")?;
    let data_size = data_size.context("Missing data chunk")?;

    // Validate format: must be stereo
    if format.channels != 2 {
        bail!(
            "Expected stereo (2 channels), found {} channels",
            format.channels
        );
    }

    // We accept any sample rate - Essentia will handle analysis at native rate
    // and we only need relative analysis, not absolute timing
    log::debug!(
        "Loading WAV: {} channels, {} Hz, {} bits",
        format.channels,
        format.sample_rate,
        format.bits_per_sample
    );

    // Seek to data start
    reader.seek(SeekFrom::Start(data_offset))?;

    // Calculate frame count
    let bytes_per_frame = format.channels as u64 * (format.bits_per_sample as u64 / 8);
    let frame_count = data_size / bytes_per_frame;

    // Read samples based on bit depth
    let samples = match (format.format_tag, format.bits_per_sample) {
        (1, 16) => read_16bit_stereo(&mut reader, frame_count as usize)?,
        (1, 24) => read_24bit_stereo(&mut reader, frame_count as usize)?,
        (1, 32) => read_32bit_int_stereo(&mut reader, frame_count as usize)?,
        (3, 32) => read_32bit_float_stereo(&mut reader, frame_count as usize)?,
        _ => bail!(
            "Unsupported format: tag={}, bits={}",
            format.format_tag,
            format.bits_per_sample
        ),
    };

    // Return samples with their source sample rate for proper resampling during export
    Ok((samples, format.sample_rate))
}

/// WAV format information
#[derive(Debug)]
struct WavFormat {
    format_tag: u16,      // 1 = PCM, 3 = IEEE float
    channels: u16,        // Should be 2 for stereo
    sample_rate: u32,     // e.g., 44100
    bits_per_sample: u16, // 16, 24, or 32
}

/// Read and parse the fmt chunk
fn read_fmt_chunk<R: std::io::Read>(reader: &mut R, size: u32) -> Result<WavFormat> {
    if size < 16 {
        bail!("fmt chunk too small");
    }

    let mut fmt_data = vec![0u8; size as usize];
    reader.read_exact(&mut fmt_data)?;

    Ok(WavFormat {
        format_tag: u16::from_le_bytes([fmt_data[0], fmt_data[1]]),
        channels: u16::from_le_bytes([fmt_data[2], fmt_data[3]]),
        sample_rate: u32::from_le_bytes([fmt_data[4], fmt_data[5], fmt_data[6], fmt_data[7]]),
        bits_per_sample: u16::from_le_bytes([fmt_data[14], fmt_data[15]]),
    })
}

/// Read 16-bit PCM stereo samples
fn read_16bit_stereo<R: std::io::Read>(reader: &mut R, frame_count: usize) -> Result<Vec<StereoSample>> {
    const SCALE: f32 = 1.0 / 32768.0;
    let mut samples = Vec::with_capacity(frame_count);
    let mut frame_buffer = [0u8; 4]; // 2 channels * 2 bytes

    for _ in 0..frame_count {
        reader.read_exact(&mut frame_buffer)?;
        let left = i16::from_le_bytes([frame_buffer[0], frame_buffer[1]]) as f32 * SCALE;
        let right = i16::from_le_bytes([frame_buffer[2], frame_buffer[3]]) as f32 * SCALE;
        samples.push(StereoSample::new(left, right));
    }

    Ok(samples)
}

/// Read 24-bit PCM stereo samples
fn read_24bit_stereo<R: std::io::Read>(reader: &mut R, frame_count: usize) -> Result<Vec<StereoSample>> {
    const SCALE: f32 = 1.0 / 8388608.0; // 2^23
    let mut samples = Vec::with_capacity(frame_count);
    let mut frame_buffer = [0u8; 6]; // 2 channels * 3 bytes

    // Convert 24-bit to i32 with sign extension
    let to_i32 = |b: &[u8]| -> i32 {
        let val = (b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16);
        if val & 0x800000 != 0 {
            val | !0xFFFFFF // Sign extend
        } else {
            val
        }
    };

    for _ in 0..frame_count {
        reader.read_exact(&mut frame_buffer)?;
        let left = to_i32(&frame_buffer[0..3]) as f32 * SCALE;
        let right = to_i32(&frame_buffer[3..6]) as f32 * SCALE;
        samples.push(StereoSample::new(left, right));
    }

    Ok(samples)
}

/// Read 32-bit integer stereo samples
fn read_32bit_int_stereo<R: std::io::Read>(reader: &mut R, frame_count: usize) -> Result<Vec<StereoSample>> {
    const SCALE: f32 = 1.0 / 2147483648.0; // 2^31
    let mut samples = Vec::with_capacity(frame_count);
    let mut frame_buffer = [0u8; 8]; // 2 channels * 4 bytes

    for _ in 0..frame_count {
        reader.read_exact(&mut frame_buffer)?;
        let left = i32::from_le_bytes([
            frame_buffer[0],
            frame_buffer[1],
            frame_buffer[2],
            frame_buffer[3],
        ]) as f32
            * SCALE;
        let right = i32::from_le_bytes([
            frame_buffer[4],
            frame_buffer[5],
            frame_buffer[6],
            frame_buffer[7],
        ]) as f32
            * SCALE;
        samples.push(StereoSample::new(left, right));
    }

    Ok(samples)
}

/// Read 32-bit float stereo samples
fn read_32bit_float_stereo<R: std::io::Read>(reader: &mut R, frame_count: usize) -> Result<Vec<StereoSample>> {
    let mut samples = Vec::with_capacity(frame_count);
    let mut frame_buffer = [0u8; 8]; // 2 channels * 4 bytes

    for _ in 0..frame_count {
        reader.read_exact(&mut frame_buffer)?;
        let left = f32::from_le_bytes([
            frame_buffer[0],
            frame_buffer[1],
            frame_buffer[2],
            frame_buffer[3],
        ]);
        let right = f32::from_le_bytes([
            frame_buffer[4],
            frame_buffer[5],
            frame_buffer[6],
            frame_buffer[7],
        ]);
        samples.push(StereoSample::new(left, right));
    }

    Ok(samples)
}
