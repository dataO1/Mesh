//! 8-channel WAV export with metadata
//!
//! Exports stem buffers to the mesh-player compatible format:
//! - 8 channels (4 stereo stems interleaved)
//! - 44.1 kHz, 16-bit
//! - Metadata in bext chunk
//! - Cue points in cue/adtl chunks

use anyhow::{Context, Result};
use mesh_core::audio_file::{CuePoint, StemBuffers, TrackMetadata};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Export stem buffers to an 8-channel WAV file with metadata
pub fn export_stem_file(
    path: &Path,
    buffers: &StemBuffers,
    metadata: &TrackMetadata,
    cue_points: &[CuePoint],
) -> Result<()> {
    log::info!("export_stem_file: Starting export to {:?}", path);
    log::info!("  Buffer length: {} samples", buffers.len());
    log::info!("  Metadata: BPM={:?}, Key={:?}", metadata.bpm, metadata.key);
    log::info!("  Cue points: {}", cue_points.len());

    let file = File::create(path)
        .with_context(|| format!("Failed to create output file: {:?}", path))?;
    log::debug!("  File created successfully");
    let mut writer = BufWriter::new(file);

    // Calculate sizes
    let num_samples = buffers.len();
    let num_channels = 8u16;
    let bits_per_sample = 16u16;
    let bytes_per_sample = bits_per_sample / 8;
    let sample_rate = 44100u32;
    let byte_rate = sample_rate * num_channels as u32 * bytes_per_sample as u32;
    let block_align = num_channels * bytes_per_sample;
    let data_size = num_samples as u32 * num_channels as u32 * bytes_per_sample as u32;

    // Build metadata string for bext chunk
    let metadata_str = format_metadata_string(metadata);
    let bext_size = calculate_bext_size(&metadata_str);

    // Build cue chunk data
    let cue_chunk_data = build_cue_chunk(cue_points);
    let adtl_chunk_data = build_adtl_chunk(cue_points);

    // Calculate total file size
    // RIFF header (12) + fmt chunk (24) + bext chunk + cue chunk + adtl chunk + data chunk (8 + data)
    let chunks_size = 24 // fmt chunk
        + bext_size
        + cue_chunk_data.len() as u32
        + adtl_chunk_data.len() as u32
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

/// Format metadata string for bext chunk description
fn format_metadata_string(metadata: &TrackMetadata) -> String {
    let bpm = metadata.bpm.unwrap_or(120.0);
    let original_bpm = metadata.original_bpm.unwrap_or(bpm);
    let key = metadata.key.as_deref().unwrap_or("?");

    let grid_str: String = metadata
        .beat_grid
        .beats
        .iter()
        .take(100) // Limit to first 100 beats to avoid huge strings
        .map(|&pos| pos.to_string())
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "BPM:{:.2}|KEY:{}|GRID:{}|ORIGINAL_BPM:{:.2}",
        bpm, key, grid_str, original_bpm
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_core::audio_file::BeatGrid;

    #[test]
    fn test_format_metadata() {
        let metadata = TrackMetadata {
            bpm: Some(128.0),
            original_bpm: Some(125.5),
            key: Some(String::from("Am")),
            beat_grid: BeatGrid::from_csv("0,22050,44100"),
            ..Default::default()
        };

        let result = format_metadata_string(&metadata);
        assert!(result.contains("BPM:128.00"));
        assert!(result.contains("KEY:Am"));
        assert!(result.contains("GRID:0,22050,44100"));
        assert!(result.contains("ORIGINAL_BPM:125.50"));
    }

    #[test]
    fn test_format_color() {
        let color = Some("#FF5500".to_string());
        assert_eq!(format_color(&color), "#FF5500");
        assert_eq!(format_color(&None), "#FF5500");
    }
}
