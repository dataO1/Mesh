//! 8-channel FLAC export (audio-only, lossless compression)
//!
//! Exports stem buffers to the mesh format:
//! - 8 channels (4 stereo stems interleaved)
//! - 48 kHz, 16-bit (professional audio standard)
//! - FLAC lossless compression (~58% size reduction vs WAV)
//!
//! All metadata (BPM, key, cue points, loops, etc.) is stored in the database,
//! NOT in the audio file.
//!
//! If the source audio is at a different sample rate (e.g., 44100 Hz from demucs),
//! it is resampled to SAMPLE_RATE (48000 Hz) before encoding.

use anyhow::{Context, Result};
use flacenc::component::BitRepr;
use flacenc::error::Verify;
use mesh_core::audio_file::StemBuffers;
use mesh_core::types::SAMPLE_RATE;
use std::borrow::Cow;
use std::path::Path;

/// Export stem buffers to an 8-channel FLAC file (audio only)
///
/// # Arguments
/// * `path` - Output file path
/// * `buffers` - Source stem buffers
/// * `source_sample_rate` - Sample rate of the source buffers (e.g., 44100 Hz from demucs)
///
/// If `source_sample_rate` differs from SAMPLE_RATE (48000 Hz), the audio is
/// automatically resampled to ensure correct playback speed.
///
/// All metadata (BPM, key, cue points, loops) should be stored in the database.
pub fn export_stem_file(
    path: &Path,
    buffers: &StemBuffers,
    source_sample_rate: u32,
) -> Result<()> {
    log::info!("export_stem_file: Starting FLAC export to {:?}", path);
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

    let num_samples = buffers.len();
    let num_channels = 8usize;

    // Interleave f32 stems into i32 buffer (16-bit range)
    // Order: Vocals L, Vocals R, Drums L, Drums R, Bass L, Bass R, Other L, Other R
    log::info!("  Interleaving {} samples x {} channels...", num_samples, num_channels);
    let mut interleaved = vec![0i32; num_samples * num_channels];
    for i in 0..num_samples {
        let base = i * num_channels;
        interleaved[base]     = f32_to_i32_16bit(buffers.vocals[i].left);
        interleaved[base + 1] = f32_to_i32_16bit(buffers.vocals[i].right);
        interleaved[base + 2] = f32_to_i32_16bit(buffers.drums[i].left);
        interleaved[base + 3] = f32_to_i32_16bit(buffers.drums[i].right);
        interleaved[base + 4] = f32_to_i32_16bit(buffers.bass[i].left);
        interleaved[base + 5] = f32_to_i32_16bit(buffers.bass[i].right);
        interleaved[base + 6] = f32_to_i32_16bit(buffers.other[i].left);
        interleaved[base + 7] = f32_to_i32_16bit(buffers.other[i].right);
    }

    // Configure FLAC encoder
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| anyhow::anyhow!("FLAC encoder config error: {:?}", e))?;

    // Pad to block-size boundary to work around flacenc bug:
    // encode_with_fixed_block_size() produces malformed final frames when
    // sample count isn't a multiple of block_size (flacenc-rs#242)
    let block_size = config.block_size;
    let remainder = num_samples % block_size;
    if remainder != 0 {
        let padding = block_size - remainder;
        interleaved.extend(std::iter::repeat(0i32).take(padding * num_channels));
        log::info!("  Padded {} silence samples to align to block_size={}", padding, block_size);
    }

    // Create source from interleaved samples
    let source = flacenc::source::MemSource::from_samples(
        &interleaved, num_channels, 16, SAMPLE_RATE as usize,
    );

    // Encode
    log::info!("  Encoding FLAC (block_size={})...", config.block_size);
    let flac_stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| anyhow::anyhow!("FLAC encoding failed: {:?}", e))?;

    // Write to bytes
    let mut sink = flacenc::bitsink::ByteSink::new();
    flac_stream.write(&mut sink)
        .map_err(|e| anyhow::anyhow!("FLAC stream write failed: {:?}", e))?;

    // Write to file
    std::fs::write(path, sink.as_slice())
        .with_context(|| format!("Failed to write FLAC file: {:?}", path))?;

    let file_size = sink.as_slice().len();
    let raw_size = num_samples * num_channels * 2; // 16-bit = 2 bytes
    log::info!(
        "export_stem_file: FLAC export complete, {} samples, {:.1} MB ({:.1}% of raw)",
        num_samples,
        file_size as f64 / (1024.0 * 1024.0),
        file_size as f64 / raw_size as f64 * 100.0
    );
    Ok(())
}

/// Convert f32 sample [-1.0, 1.0] to i32 in 16-bit range
#[inline]
fn f32_to_i32_16bit(sample: f32) -> i32 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i32
}
