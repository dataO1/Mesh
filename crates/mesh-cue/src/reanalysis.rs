//! Re-analysis system for existing tracks
//!
//! Provides selective re-analysis of tracks without full re-import.
//! Supports updating only specific metadata (LUFS, BPM, Key) while
//! preserving existing cue points, loops, and other data.

use crate::analysis::{
    analyze_partial_in_subprocess, AnalysisType, PartialAnalysisResult, ReanalysisProgress,
};
use crate::config::{BpmConfig, LoudnessConfig};
use anyhow::{Context, Result};
use mesh_core::audio_file::{
    read_metadata, update_metadata_bulk, AudioFileReader, PartialMetadataUpdate,
};
use mesh_core::types::SAMPLE_RATE;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// Re-analyze a single track file and update its metadata
///
/// This function:
/// 1. Loads the audio from the existing WAV file
/// 2. Creates a mono mix for analysis
/// 3. Runs the requested analysis in an isolated subprocess
/// 4. Updates the file's bext chunk with new metadata
///
/// For LUFS changes, a full re-export with new waveform is required.
/// For BPM/Key only, the bext chunk is updated in-place (fast path).
///
/// # Arguments
/// * `path` - Path to the WAV file to re-analyze
/// * `analysis_type` - Which analysis to perform
/// * `bpm_config` - BPM detection configuration
/// * `loudness_config` - Loudness normalization configuration (for waveform regeneration)
///
/// # Returns
/// The partial analysis result with the updated fields
pub fn reanalyze_track(
    path: &Path,
    analysis_type: AnalysisType,
    bpm_config: &BpmConfig,
    loudness_config: &LoudnessConfig,
) -> Result<PartialAnalysisResult> {
    log::info!(
        "reanalyze_track: {} analysis on {:?}",
        analysis_type.display_name(),
        path
    );

    // Load audio from existing file
    let mut reader = AudioFileReader::open(path)
        .with_context(|| format!("Failed to open file: {:?}", path))?;

    let stems = reader
        .read_all_stems()
        .with_context(|| format!("Failed to read stems from: {:?}", path))?;

    // Create mono mix for analysis (sum all stems)
    let mono_samples = create_mono_mix(&stems);

    log::info!(
        "reanalyze_track: Created mono mix with {} samples ({:.1}s)",
        mono_samples.len(),
        mono_samples.len() as f64 / SAMPLE_RATE as f64
    );

    // Run analysis in subprocess
    let result = analyze_partial_in_subprocess(mono_samples, analysis_type, bpm_config.clone())
        .with_context(|| format!("Analysis failed for: {:?}", path))?;

    // Update the file based on what was analyzed
    if analysis_type.requires_waveform_regeneration() {
        // For LUFS changes, we need to regenerate the waveform preview
        // This requires a full re-export of the file
        reexport_with_new_waveform(path, &result, loudness_config)
            .with_context(|| format!("Re-export failed for: {:?}", path))?;
    } else {
        // Fast path: just update bext chunk in-place
        let updates = PartialMetadataUpdate {
            bpm: result.bpm,
            first_beat: result.beat_grid.as_ref().and_then(|g| g.first().copied()),
            key: result.key.clone(),
            lufs: result.lufs,
        };

        update_metadata_bulk(path, &updates)
            .with_context(|| format!("Metadata update failed for: {:?}", path))?;
    }

    log::info!(
        "reanalyze_track: Completed for {:?} - {:?}",
        path,
        result
    );

    Ok(result)
}

/// Create a mono mix from stem buffers for analysis
fn create_mono_mix(stems: &mesh_core::audio_file::StemBuffers) -> Vec<f32> {
    let len = stems.len();
    let mut mono = Vec::with_capacity(len);

    for i in 0..len {
        // Sum all stems and average (use .left and .right fields)
        let vocals = (stems.vocals[i].left + stems.vocals[i].right) / 2.0;
        let drums = (stems.drums[i].left + stems.drums[i].right) / 2.0;
        let bass = (stems.bass[i].left + stems.bass[i].right) / 2.0;
        let other = (stems.other[i].left + stems.other[i].right) / 2.0;

        mono.push((vocals + drums + bass + other) / 4.0);
    }

    mono
}

/// Re-export a track with regenerated waveform preview
///
/// This is needed when LUFS changes, as the waveform preview is scaled
/// by the loudness compensation gain.
fn reexport_with_new_waveform(
    path: &Path,
    analysis_result: &PartialAnalysisResult,
    loudness_config: &LoudnessConfig,
) -> Result<()> {
    use crate::export::export_stem_file_with_gain;
    use mesh_core::audio_file::LoadedTrack;
    use std::fs;

    log::info!("reexport_with_new_waveform: Re-exporting {:?}", path);

    // Load the full track (including metadata, cues, loops)
    let track = LoadedTrack::load(path)
        .map_err(|e| anyhow::anyhow!("Failed to load track: {}", e))?;

    // Update metadata with new analysis results
    let mut metadata = track.metadata.clone();
    if let Some(bpm) = analysis_result.bpm {
        metadata.bpm = Some(bpm);
        if metadata.original_bpm.is_none() {
            metadata.original_bpm = Some(bpm);
        }
    }
    if let Some(ref beat_grid) = analysis_result.beat_grid {
        metadata.beat_grid.first_beat_sample = beat_grid.first().copied();
        metadata.beat_grid.beats = beat_grid.clone();
    }
    if let Some(ref key) = analysis_result.key {
        metadata.key = Some(key.clone());
    }
    if let Some(lufs) = analysis_result.lufs {
        metadata.lufs = Some(lufs);
    }

    // Calculate new waveform gain from LUFS
    let waveform_gain = metadata
        .lufs
        .map(|lufs| loudness_config.calculate_gain_linear(lufs))
        .unwrap_or(1.0);

    log::info!(
        "reexport_with_new_waveform: LUFS={:?}, waveform_gain={:.3}",
        metadata.lufs,
        waveform_gain
    );

    // Export to temp file first
    let temp_path = path.with_extension("wav.tmp");

    // Clone stems from Shared for export
    let stems = (*track.stems).clone();

    export_stem_file_with_gain(
        &temp_path,
        &stems,
        SAMPLE_RATE, // Already at target rate
        &metadata,
        &metadata.cue_points,
        &metadata.saved_loops,
        waveform_gain,
    )
    .with_context(|| format!("Export failed for temp file: {:?}", temp_path))?;

    // Atomic rename to replace original
    fs::rename(&temp_path, path)
        .with_context(|| format!("Failed to replace {:?} with {:?}", path, temp_path))?;

    log::info!(
        "reexport_with_new_waveform: Successfully updated {:?}",
        path
    );

    Ok(())
}

/// Run batch re-analysis on multiple tracks
///
/// Processes tracks in parallel using rayon thread pool.
/// Sends progress updates through the channel for UI display.
///
/// # Arguments
/// * `tracks` - List of track file paths to re-analyze
/// * `analysis_type` - Which analysis to perform
/// * `bpm_config` - BPM detection configuration
/// * `loudness_config` - Loudness configuration (for waveform regeneration)
/// * `parallel_processes` - Number of parallel workers (1-16)
/// * `progress_tx` - Channel to send progress updates
/// * `cancel_flag` - Atomic flag to check for cancellation
pub fn run_batch_reanalysis(
    tracks: Vec<PathBuf>,
    analysis_type: AnalysisType,
    bpm_config: BpmConfig,
    loudness_config: LoudnessConfig,
    parallel_processes: u8,
    progress_tx: Sender<ReanalysisProgress>,
    cancel_flag: std::sync::Arc<AtomicBool>,
) {
    use rayon::prelude::*;

    let start_time = Instant::now();
    let total = tracks.len();

    log::info!(
        "run_batch_reanalysis: Starting {} analysis for {} tracks",
        analysis_type.display_name(),
        total
    );

    // Send start notification
    let _ = progress_tx.send(ReanalysisProgress::Started {
        total_tracks: total,
        analysis_type,
    });

    // Check for early cancellation
    if cancel_flag.load(Ordering::Relaxed) {
        log::info!("run_batch_reanalysis: Cancelled before processing");
        let _ = progress_tx.send(ReanalysisProgress::AllComplete {
            succeeded: 0,
            failed: 0,
            duration: Duration::ZERO,
        });
        return;
    }

    // Configure rayon thread pool with user-specified parallelism (same as batch_import)
    let num_workers = parallel_processes.clamp(1, 16) as usize;
    log::info!("run_batch_reanalysis: Using {} parallel workers", num_workers);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_workers)
        .build()
        .expect("Failed to create thread pool");

    // Process tracks in parallel
    let results: Vec<bool> = pool.install(|| {
        tracks
            .par_iter()
            .enumerate()
            .map(|(index, path)| {
                // Check for cancellation
                if cancel_flag.load(Ordering::Relaxed) {
                    return false;
                }

                let track_name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown")
                    .to_string();

                // Send track started notification
                let _ = progress_tx.send(ReanalysisProgress::TrackStarted {
                    track_name: track_name.clone(),
                    index,
                    total,
                });

                // Re-analyze the track
                let success = match reanalyze_track(path, analysis_type, &bpm_config, &loudness_config) {
                    Ok(_) => {
                        let _ = progress_tx.send(ReanalysisProgress::TrackCompleted {
                            track_name,
                            success: true,
                            error: None,
                        });
                        true
                    }
                    Err(e) => {
                        log::error!("run_batch_reanalysis: Failed for {:?}: {}", path, e);
                        let _ = progress_tx.send(ReanalysisProgress::TrackCompleted {
                            track_name,
                            success: false,
                            error: Some(e.to_string()),
                        });
                        false
                    }
                };

                success
            })
            .collect()
    });

    let duration = start_time.elapsed();
    let succeeded = results.iter().filter(|&&s| s).count();
    let failed = results.len() - succeeded;

    log::info!(
        "run_batch_reanalysis: Complete in {:.1}s - {} succeeded, {} failed",
        duration.as_secs_f64(),
        succeeded,
        failed
    );

    // Send completion notification
    let _ = progress_tx.send(ReanalysisProgress::AllComplete {
        succeeded,
        failed,
        duration,
    });
}
