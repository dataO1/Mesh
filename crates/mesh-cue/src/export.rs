//! 8-channel WAV export with metadata
//!
//! Exports stem buffers to the mesh-player compatible format:
//! - 8 channels (4 stereo stems interleaved)
//! - 48 kHz, 16-bit (matches JACK default)
//! - Metadata in bext chunk
//! - Cue points in cue/adtl chunks
//!
//! If the source audio is at a different sample rate (e.g., 44100 Hz from demucs),
//! it is resampled to SAMPLE_RATE (48000 Hz) before writing.

use anyhow::{Context, Result};
use mesh_core::audio_file::{
    serialize_mslk_chunk, serialize_wvfm_chunk, CuePoint, SavedLoop, StemBuffers,
    StemLinkReference, TrackMetadata,
};
use mesh_core::types::SAMPLE_RATE;
use std::borrow::Cow;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::ui::waveform::{generate_waveform_preview, generate_waveform_preview_with_gain};

/// Export stem buffers to an 8-channel WAV file with metadata
///
/// # Arguments
/// * `path` - Output file path
/// * `buffers` - Source stem buffers
/// * `source_sample_rate` - Sample rate of the source buffers (e.g., 44100 Hz from demucs)
/// * `metadata` - Track metadata (BPM, key, beat grid at TARGET sample rate)
/// * `cue_points` - Cue point markers
/// * `saved_loops` - Saved loop regions
///
/// If `source_sample_rate` differs from SAMPLE_RATE (48000 Hz), the audio is
/// automatically resampled to ensure correct playback speed.
///
/// Note: For gain-scaled waveform previews, use `export_stem_file_with_gain`.
pub fn export_stem_file(
    path: &Path,
    buffers: &StemBuffers,
    source_sample_rate: u32,
    metadata: &TrackMetadata,
    cue_points: &[CuePoint],
    saved_loops: &[SavedLoop],
) -> Result<()> {
    export_stem_file_with_gain(path, buffers, source_sample_rate, metadata, cue_points, saved_loops, 1.0)
}

/// Export stem buffers to an 8-channel WAV file with LUFS-compensated waveform preview
///
/// Same as `export_stem_file`, but with an additional `waveform_gain` parameter
/// that scales the waveform preview for loudness-normalized display.
///
/// # Arguments
/// * `path` - Output file path
/// * `buffers` - Source stem buffers
/// * `source_sample_rate` - Sample rate of the source buffers (e.g., 44100 Hz from demucs)
/// * `metadata` - Track metadata (BPM, key, beat grid at TARGET sample rate)
/// * `cue_points` - Cue point markers
/// * `saved_loops` - Saved loop regions
/// * `waveform_gain` - Linear gain multiplier for waveform preview (1.0 = unity)
///
/// The waveform_gain should be calculated from:
/// `10^((target_lufs - track_lufs) / 20)`
pub fn export_stem_file_with_gain(
    path: &Path,
    buffers: &StemBuffers,
    source_sample_rate: u32,
    metadata: &TrackMetadata,
    cue_points: &[CuePoint],
    saved_loops: &[SavedLoop],
    waveform_gain: f32,
) -> Result<()> {
    log::info!("export_stem_file: Starting export to {:?}", path);
    log::info!("  Buffer length: {} samples @ {} Hz", buffers.len(), source_sample_rate);
    log::info!("  Target sample rate: {} Hz", SAMPLE_RATE);
    log::info!("  Metadata: BPM={:?}, Key={:?}", metadata.bpm, metadata.key);
    log::info!("  Cue points: {}", cue_points.len());
    log::info!("  Saved loops: {}", saved_loops.len());

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

    // Build metadata string for bext chunk (includes DROP marker if set)
    let metadata_str = metadata.to_bext_description();
    let bext_size = calculate_bext_size(&metadata_str);

    // Build cue chunk data
    let cue_chunk_data = build_cue_chunk(cue_points);
    let adtl_chunk_data = build_adtl_chunk(cue_points);

    // Build saved loops chunk (custom "mlop" chunk)
    let mlop_chunk_data = build_mlop_chunk(saved_loops);

    // Build stem links chunk (custom "mslk" chunk for prepared mode)
    let mslk_chunk_data = build_mslk_chunk(&metadata.stem_links);
    if !mslk_chunk_data.is_empty() {
        log::info!("  Stem links: {} links", metadata.stem_links.len());
    }

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
    // RIFF header (12) + fmt chunk (24) + bext chunk + cue chunk + adtl chunk + mlop chunk + mslk chunk + wvfm chunk + data chunk (8 + data)
    let chunks_size = 24 // fmt chunk
        + bext_size
        + cue_chunk_data.len() as u32
        + adtl_chunk_data.len() as u32
        + mlop_chunk_data.len() as u32
        + mslk_chunk_data.len() as u32
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

    // Write bext chunk
    write_bext_chunk(&mut writer, &metadata_str)?;

    // Write cue chunk (if there are cue points)
    if !cue_chunk_data.is_empty() {
        writer.write_all(&cue_chunk_data)?;
    }

    // Write adtl LIST chunk (if there are cue points)
    if !adtl_chunk_data.is_empty() {
        writer.write_all(&adtl_chunk_data)?;
    }

    // Write mlop chunk (saved loops - custom mesh chunk)
    if !mlop_chunk_data.is_empty() {
        writer.write_all(&mlop_chunk_data)?;
    }

    // Write mslk chunk (stem links - custom mesh chunk for prepared mode)
    if !mslk_chunk_data.is_empty() {
        writer.write_all(&mslk_chunk_data)?;
    }

    // Write wvfm chunk (waveform preview for instant display)
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

/// Calculate bext chunk size (padded to even bytes)
fn calculate_bext_size(_metadata_str: &str) -> u32 {
    // bext chunk: "bext" (4) + size (4) + description (256) + ...
    // We use a simplified version with just the description field
    let description_size = 256;
    let chunk_size = description_size;
    8 + chunk_size // chunk header + data
}

/// Write bext chunk to writer
fn write_bext_chunk<W: Write>(writer: &mut W, metadata_str: &str) -> Result<()> {
    writer.write_all(b"bext")?;

    let description_size = 256u32;
    writer.write_all(&description_size.to_le_bytes())?;

    // Write description (256 bytes, null-padded)
    let mut description = [0u8; 256];
    let bytes = metadata_str.as_bytes();
    let copy_len = bytes.len().min(255);
    description[..copy_len].copy_from_slice(&bytes[..copy_len]);
    writer.write_all(&description)?;

    Ok(())
}

/// Build cue chunk data
fn build_cue_chunk(cue_points: &[CuePoint]) -> Vec<u8> {
    if cue_points.is_empty() {
        return Vec::new();
    }

    let mut data = Vec::new();

    // "cue " chunk header
    data.extend_from_slice(b"cue ");

    // Chunk size (4 + 24 * num_cues)
    let num_cues = cue_points.len() as u32;
    let chunk_size = 4 + 24 * num_cues;
    data.extend_from_slice(&chunk_size.to_le_bytes());

    // Number of cue points
    data.extend_from_slice(&num_cues.to_le_bytes());

    // Write each cue point
    for (i, cue) in cue_points.iter().enumerate() {
        // Cue point ID
        data.extend_from_slice(&(i as u32 + 1).to_le_bytes());
        // Position (in samples, not used - we use sample offset)
        data.extend_from_slice(&0u32.to_le_bytes());
        // Data chunk ID ("data")
        data.extend_from_slice(b"data");
        // Chunk start
        data.extend_from_slice(&0u32.to_le_bytes());
        // Block start
        data.extend_from_slice(&0u32.to_le_bytes());
        // Sample offset (actual position)
        data.extend_from_slice(&(cue.sample_position as u32).to_le_bytes());
    }

    data
}

/// Build adtl LIST chunk for cue point labels and colors
fn build_adtl_chunk(cue_points: &[CuePoint]) -> Vec<u8> {
    if cue_points.is_empty() {
        return Vec::new();
    }

    let mut list_data = Vec::new();

    // Build labl sub-chunks
    for (i, cue) in cue_points.iter().enumerate() {
        let label = format!("{}:{}|color:{}", i + 1, cue.label, format_color(&cue.color));
        let label_bytes = label.as_bytes();

        // "labl" sub-chunk
        list_data.extend_from_slice(b"labl");

        // Chunk size (4 for cue ID + label length + null terminator)
        let label_size = 4 + label_bytes.len() as u32 + 1;
        // Pad to even
        let padded_size = (label_size + 1) & !1;
        list_data.extend_from_slice(&padded_size.to_le_bytes());

        // Cue point ID
        list_data.extend_from_slice(&(i as u32 + 1).to_le_bytes());

        // Label string (null-terminated)
        list_data.extend_from_slice(label_bytes);
        list_data.push(0); // null terminator

        // Pad to even
        if label_size % 2 == 1 {
            list_data.push(0);
        }
    }

    // Wrap in LIST chunk
    let mut data = Vec::new();
    data.extend_from_slice(b"LIST");
    data.extend_from_slice(&((4 + list_data.len()) as u32).to_le_bytes());
    data.extend_from_slice(b"adtl");
    data.extend_from_slice(&list_data);

    data
}

/// Build "mlop" (mesh loops) custom chunk for saved loops
///
/// Format:
/// - "mlop" (4 bytes) - chunk ID
/// - size (4 bytes) - chunk data size
/// - num_loops (4 bytes) - number of loops
/// - For each loop:
///   - index (1 byte)
///   - start_sample (8 bytes, u64 LE)
///   - end_sample (8 bytes, u64 LE)
///   - label_len (2 bytes, u16 LE)
///   - label (label_len bytes, UTF-8)
///   - color_len (2 bytes, u16 LE)
///   - color (color_len bytes, UTF-8, or 0 if none)
fn build_mlop_chunk(saved_loops: &[SavedLoop]) -> Vec<u8> {
    if saved_loops.is_empty() {
        return Vec::new();
    }

    let mut data = Vec::new();

    // Build loop data first to calculate size
    let mut loop_data = Vec::new();

    // Number of loops
    loop_data.extend_from_slice(&(saved_loops.len() as u32).to_le_bytes());

    for loop_slot in saved_loops {
        // Index
        loop_data.push(loop_slot.index);
        // Start/end samples
        loop_data.extend_from_slice(&loop_slot.start_sample.to_le_bytes());
        loop_data.extend_from_slice(&loop_slot.end_sample.to_le_bytes());
        // Label
        let label_bytes = loop_slot.label.as_bytes();
        loop_data.extend_from_slice(&(label_bytes.len() as u16).to_le_bytes());
        loop_data.extend_from_slice(label_bytes);
        // Color
        if let Some(ref color) = loop_slot.color {
            let color_bytes = color.as_bytes();
            loop_data.extend_from_slice(&(color_bytes.len() as u16).to_le_bytes());
            loop_data.extend_from_slice(color_bytes);
        } else {
            loop_data.extend_from_slice(&0u16.to_le_bytes());
        }
    }

    // Pad to word boundary if needed
    if loop_data.len() % 2 != 0 {
        loop_data.push(0);
    }

    // Write chunk header + data
    data.extend_from_slice(b"mlop");
    data.extend_from_slice(&(loop_data.len() as u32).to_le_bytes());
    data.extend_from_slice(&loop_data);

    data
}

/// Build "mslk" (mesh stem links) custom chunk for prepared mode
///
/// Format:
/// - "mslk" (4 bytes) - chunk ID
/// - size (4 bytes) - chunk data size
/// - data (variable) - serialized stem links from mesh_core::audio_file::serialize_mslk_chunk
fn build_mslk_chunk(stem_links: &[StemLinkReference]) -> Vec<u8> {
    if stem_links.is_empty() {
        return Vec::new();
    }

    let mslk_data = serialize_mslk_chunk(stem_links);

    // Pad to word boundary if needed
    let padding = if mslk_data.len() % 2 != 0 { 1 } else { 0 };

    let mut data = Vec::new();
    data.extend_from_slice(b"mslk");
    data.extend_from_slice(&(mslk_data.len() as u32).to_le_bytes());
    data.extend_from_slice(&mslk_data);
    if padding > 0 {
        data.push(0);
    }

    data
}

/// Format color for output (use existing color or default)
fn format_color(color: &Option<String>) -> &str {
    color.as_deref().unwrap_or("#FF5500")
}

/// Write a single f32 sample as 16-bit PCM
fn write_sample_16bit<W: Write>(writer: &mut W, sample: f32) -> Result<()> {
    // Clamp to [-1, 1] and convert to i16
    let clamped = sample.clamp(-1.0, 1.0);
    let value = (clamped * 32767.0) as i16;
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

/// Save updated metadata to an existing track file
///
/// Re-exports the file with new metadata while preserving audio.
/// This is used by the track editor to save user modifications.
///
/// Note: Existing tracks in the collection are already at SAMPLE_RATE (48kHz),
/// so no resampling is performed.
pub fn save_track_metadata(
    path: &Path,
    stems: &StemBuffers,
    metadata: &TrackMetadata,
    cue_points: &[CuePoint],
    saved_loops: &[SavedLoop],
) -> Result<()> {
    log::info!("save_track_metadata: Saving to {:?}", path);
    // Reuse the export function - existing tracks are already at SAMPLE_RATE, so no resampling
    export_stem_file(path, stems, SAMPLE_RATE, metadata, cue_points, saved_loops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_core::audio_file::BeatGrid;

    #[test]
    fn test_metadata_to_bext_description() {
        // Test basic metadata
        let metadata = TrackMetadata {
            bpm: Some(128.0),
            original_bpm: Some(125.5),
            key: Some(String::from("Am")),
            beat_grid: BeatGrid::from_csv("0,22050,44100"),
            ..Default::default()
        };

        let result = metadata.to_bext_description();
        assert!(result.contains("BPM:128.00"));
        assert!(result.contains("KEY:Am"));
        assert!(result.contains("FIRST_BEAT:0"));
        assert!(result.contains("ORIGINAL_BPM:125.50"));
    }

    #[test]
    fn test_metadata_with_drop_marker() {
        // Test that DROP marker is included in bext description
        let metadata = TrackMetadata {
            bpm: Some(128.0),
            key: Some(String::from("Am")),
            drop_marker: Some(1234567),
            ..Default::default()
        };

        let result = metadata.to_bext_description();
        assert!(result.contains("DROP:1234567"), "DROP marker should be in bext description: {}", result);
    }

    #[test]
    fn test_format_color() {
        let color = Some("#FF5500".to_string());
        assert_eq!(format_color(&color), "#FF5500");
        assert_eq!(format_color(&None), "#FF5500");
    }
}
