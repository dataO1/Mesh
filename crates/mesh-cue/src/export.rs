//! 8-channel WAV export (audio-only)
//!
//! Exports stem buffers to the mesh format:
//! - 8 channels (4 stereo stems interleaved)
//! - 48 kHz, 16-bit (matches JACK default)
//! - Waveform preview in wvfm chunk (for fast display)
//!
//! All metadata (BPM, key, cue points, loops, etc.) is stored in the database,
//! NOT in the WAV file. This separation makes metadata operations fast and
//! keeps WAV files as pure audio containers.
//!
//! If the source audio is at a different sample rate (e.g., 44100 Hz from demucs),
//! it is resampled to SAMPLE_RATE (48000 Hz) before writing.

use anyhow::{Context, Result};
use mesh_core::audio_file::{serialize_wvfm_chunk, StemBuffers};
use mesh_core::types::SAMPLE_RATE;
use std::borrow::Cow;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::ui::waveform::generate_waveform_preview_with_gain;

/// Export stem buffers to an 8-channel WAV file (audio only)
///
/// # Arguments
/// * `path` - Output file path
/// * `buffers` - Source stem buffers
/// * `source_sample_rate` - Sample rate of the source buffers (e.g., 44100 Hz from demucs)
///
/// If `source_sample_rate` differs from SAMPLE_RATE (48000 Hz), the audio is
/// automatically resampled to ensure correct playback speed.
///
/// Note: For gain-scaled waveform previews, use `export_stem_file_with_gain`.
/// All metadata (BPM, key, cue points, loops) should be stored in the database.
pub fn export_stem_file(
    path: &Path,
    buffers: &StemBuffers,
    source_sample_rate: u32,
) -> Result<()> {
    export_stem_file_with_gain(path, buffers, source_sample_rate, 1.0)
}

/// Export stem buffers to an 8-channel WAV file with LUFS-compensated waveform preview
///
/// # Arguments
/// * `path` - Output file path
/// * `buffers` - Source stem buffers
/// * `source_sample_rate` - Sample rate of the source buffers (e.g., 44100 Hz from demucs)
/// * `waveform_gain` - Linear gain multiplier for waveform preview (1.0 = unity)
///
/// The waveform_gain should be calculated from:
/// `10^((target_lufs - track_lufs) / 20)`
///
/// All metadata (BPM, key, cue points, loops) should be stored in the database,
/// NOT in the WAV file.
pub fn export_stem_file_with_gain(
    path: &Path,
    buffers: &StemBuffers,
    source_sample_rate: u32,
    waveform_gain: f32,
) -> Result<()> {
    log::info!("export_stem_file: Starting export to {:?}", path);
    log::info!("  Buffer length: {} samples @ {} Hz", buffers.len(), source_sample_rate);
    log::info!("  Target sample rate: {} Hz", SAMPLE_RATE);

    // Resample if source rate differs from target rate
    let buffers: Cow<StemBuffers> = if source_sample_rate != SAMPLE_RATE {
        log::info!("  Resampling from {} Hz to {} Hz...", source_sample_rate, SAMPLE_RATE);
        let resampled = buffers
            .resample(source_sample_rate, SAMPLE_RATE)
            .context("Failed to resample stems")?;
        log::info!("  Resampled: {} samples -> {} samples", buffers.len(), resampled.len());
        Cow::Owned(resampled)
    } else {
        Cow::Borrowed(buffers)
    };

    let file = File::create(path)
        .with_context(|| format!("Failed to create output file: {:?}", path))?;
    log::debug!("  File created successfully");
    let mut writer = BufWriter::new(file);

    // Calculate sizes (using resampled buffer)
    let num_samples = buffers.len();
    let num_channels = 8u16;
    let bits_per_sample = 16u16;
    let bytes_per_sample = bits_per_sample / 8;
    let sample_rate = SAMPLE_RATE;
    let byte_rate = sample_rate * num_channels as u32 * bytes_per_sample as u32;
    let block_align = num_channels * bytes_per_sample;
    let data_size = num_samples as u32 * num_channels as u32 * bytes_per_sample as u32;

    // Generate waveform preview and build wvfm chunk
    // Use gain-scaled version if gain != 1.0 (for LUFS normalization)
    log::info!("  Generating waveform preview (gain={:.3})...", waveform_gain);
    let waveform_preview = generate_waveform_preview_with_gain(&buffers, waveform_gain);
    let wvfm_data = serialize_wvfm_chunk(&waveform_preview);
    // WAV chunks must be word-aligned (2 bytes). Add padding if data length is odd.
    let wvfm_padding = if wvfm_data.len() % 2 != 0 { 1u32 } else { 0u32 };
    let wvfm_chunk_size = 8 + wvfm_data.len() as u32 + wvfm_padding;
    log::info!("  Waveform preview: {} bytes (padding: {})", wvfm_data.len(), wvfm_padding);

    // Calculate total file size
    // WAV now contains only: fmt chunk + wvfm chunk + data chunk
    // All metadata (BPM, key, cues, loops, stem links) is stored in the database
    let chunks_size = 24 // fmt chunk
        + wvfm_chunk_size
        + 8 + data_size; // data chunk header + data

    let file_size = 4 + chunks_size; // "WAVE" + chunks

    // Write RIFF header
    writer.write_all(b"RIFF")?;
    writer.write_all(&file_size.to_le_bytes())?;
    writer.write_all(b"WAVE")?;

    // Write fmt chunk
    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?; // chunk size
    writer.write_all(&1u16.to_le_bytes())?; // audio format (1 = PCM)
    writer.write_all(&num_channels.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&bits_per_sample.to_le_bytes())?;

    // Write wvfm chunk (waveform preview for instant display)
    // Note: All metadata (bext, cue, adtl, mlop, mslk) has been removed - stored in database instead
    writer.write_all(b"wvfm")?;
    writer.write_all(&(wvfm_data.len() as u32).to_le_bytes())?;
    writer.write_all(&wvfm_data)?;
    // Pad to word boundary if chunk data is odd-length
    if wvfm_data.len() % 2 != 0 {
        writer.write_all(&[0u8])?;
    }

    // Write data chunk
    writer.write_all(b"data")?;
    writer.write_all(&data_size.to_le_bytes())?;

    // Write interleaved samples
    // StemBuffers has separate vocals, drums, bass, other StereoBuffer fields
    for i in 0..num_samples {
        // Order: Vocals L, Vocals R, Drums L, Drums R, Bass L, Bass R, Other L, Other R
        write_sample_16bit(&mut writer, buffers.vocals[i].left)?;
        write_sample_16bit(&mut writer, buffers.vocals[i].right)?;
        write_sample_16bit(&mut writer, buffers.drums[i].left)?;
        write_sample_16bit(&mut writer, buffers.drums[i].right)?;
        write_sample_16bit(&mut writer, buffers.bass[i].left)?;
        write_sample_16bit(&mut writer, buffers.bass[i].right)?;
        write_sample_16bit(&mut writer, buffers.other[i].left)?;
        write_sample_16bit(&mut writer, buffers.other[i].right)?;
    }

    writer.flush()?;
    log::info!("export_stem_file: Export complete, wrote {} samples to {:?}", num_samples, path);
    Ok(())
}

/// Write a single f32 sample as 16-bit PCM
fn write_sample_16bit<W: Write>(writer: &mut W, sample: f32) -> Result<()> {
    // Clamp to [-1, 1] and convert to i16
    let clamped = sample.clamp(-1.0, 1.0);
    let value = (clamped * 32767.0) as i16;
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}
