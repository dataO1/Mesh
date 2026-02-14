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
use mesh_core::audio_file::AudioFileReader;
use mesh_core::db::DatabaseService;
use mesh_core::types::SAMPLE_RATE;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Re-analyze a single track file and update its metadata in the database
///
/// This function:
/// 1. Loads the audio from the existing WAV file
/// 2. Creates a mono mix for analysis
/// 3. Runs the requested analysis in an isolated subprocess
/// 4. Updates the database with new metadata (BPM, key, LUFS)
///
/// For LUFS changes, the waveform preview is regenerated and stored externally.
/// The database is the single source of truth - WAV files contain only audio.
///
/// # Arguments
/// * `path` - Path to the WAV file to re-analyze
/// * `analysis_type` - Which analysis to perform
/// * `bpm_config` - BPM detection configuration
/// * `loudness_config` - Loudness normalization configuration (for waveform regeneration)
/// * `db` - Optional database service for storing analysis results
///
/// # Returns
/// The partial analysis result with the updated fields
pub fn reanalyze_track(
    path: &Path,
    analysis_type: AnalysisType,
    bpm_config: &BpmConfig,
    loudness_config: &LoudnessConfig,
    db: Option<&Arc<DatabaseService>>,
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

    // Update the database with analysis results
    // WAV files are now audio-only containers - all metadata lives in the database
    if let Some(db_service) = db {
        let path_str = path.to_string_lossy();

        // Look up track by path to get track_id
        let track_id = match db_service.get_track_by_path(&path_str) {
            Ok(Some(track)) => track.id.unwrap_or(0),
            Ok(None) => {
                log::warn!("reanalyze_track: Track not found in database: {}", path_str);
                return Ok(result);
            }
            Err(e) => {
                log::error!("reanalyze_track: Failed to look up track: {:?}", e);
                return Ok(result);
            }
        };

        // Update BPM if detected
        if let Some(bpm) = result.bpm {
            if let Err(e) = db_service.update_track_field(track_id, "bpm", &bpm.to_string()) {
                log::error!("Failed to update BPM in database: {:?}", e);
            }
            // Set original_bpm if this is first analysis
            // (handled by upsert logic in import, so we skip here to preserve user edits)
        }

        // Update key if detected
        if let Some(ref key) = result.key {
            if let Err(e) = db_service.update_track_field(track_id, "key", key) {
                log::error!("Failed to update key in database: {:?}", e);
            }
        }

        // Update LUFS if measured
        if let Some(lufs) = result.lufs {
            if let Err(e) = db_service.update_track_field(track_id, "lufs", &lufs.to_string()) {
                log::error!("Failed to update LUFS in database: {:?}", e);
            }
        }

        // TODO: Store first_beat_sample in database when schema is updated
        // if let Some(first_beat) = result.beat_grid.as_ref().and_then(|g| g.first().copied()) {
        //     db_service.update_track_field(track_id, "first_beat_sample", &first_beat.to_string())?;
        // }

        log::info!("reanalyze_track: Updated database for {:?}", path);
    } else {
        log::warn!("reanalyze_track: No database provided, analysis results not persisted");
    }

    // For LUFS changes, regenerate waveform preview (stored externally via waveform_path)
    if analysis_type.requires_waveform_regeneration() {
        // TODO: Generate and store waveform preview to external file
        // For now, waveform will be regenerated on-demand when track is loaded
        log::info!("reanalyze_track: LUFS changed, waveform will be regenerated on next load");
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
        // Convert each stem to mono (average L+R)
        let vocals = (stems.vocals[i].left + stems.vocals[i].right) * 0.5;
        let drums = (stems.drums[i].left + stems.drums[i].right) * 0.5;
        let bass = (stems.bass[i].left + stems.bass[i].right) * 0.5;
        let other = (stems.other[i].left + stems.other[i].right) * 0.5;

        // Sum all stems at full level (no attenuation for accurate LUFS)
        mono.push(vocals + drums + bass + other);
    }

    mono
}

/// Run batch re-analysis on multiple tracks
///
/// Processes tracks in parallel using rayon thread pool.
/// Sends progress updates through the channel for UI display.
/// Analysis results are stored directly in the database.
///
/// # Arguments
/// * `tracks` - List of track file paths to re-analyze
/// * `analysis_type` - Which analysis to perform
/// * `bpm_config` - BPM detection configuration
/// * `loudness_config` - Loudness configuration (unused, kept for API compatibility)
/// * `parallel_processes` - Number of parallel workers (1-16)
/// * `progress_tx` - Channel to send progress updates
/// * `cancel_flag` - Atomic flag to check for cancellation
/// * `db` - Optional database service for storing results
pub fn run_batch_reanalysis(
    tracks: Vec<PathBuf>,
    analysis_type: AnalysisType,
    bpm_config: BpmConfig,
    loudness_config: LoudnessConfig,
    parallel_processes: u8,
    progress_tx: Sender<ReanalysisProgress>,
    cancel_flag: std::sync::Arc<AtomicBool>,
    db: Option<Arc<DatabaseService>>,
) {
    // Note: loudness_config is kept for API compatibility but no longer used
    // since WAV files no longer store metadata
    let _ = &loudness_config;
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
                let success = match reanalyze_track(path, analysis_type, &bpm_config, &loudness_config, db.as_ref()) {
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

// ============================================================================
// ML / Similarity Reanalysis
// ============================================================================

/// Extract vocal mono from stem buffers (for vocal presence detection)
fn create_vocals_mono(stems: &mesh_core::audio_file::StemBuffers) -> Vec<f32> {
    let len = stems.len();
    let mut mono = Vec::with_capacity(len);
    for i in 0..len {
        mono.push((stems.vocals[i].left + stems.vocals[i].right) * 0.5);
    }
    mono
}

/// Re-analyze a single track's ML/similarity features
///
/// This function:
/// 1. Loads the audio from the existing WAV file
/// 2. Extracts vocal mono for vocal presence detection (RMS energy)
/// 3. Creates a full mono mix for mel spectrogram computation
/// 4. Computes mel spectrogram (pure Rust DSP, 96-band, 16kHz)
/// 5. Runs EffNet → genre predictions + embedding → mood head → arousal/valence
/// 6. Clears old ML-generated tags, then auto-tags with new results
/// 7. Stores ML analysis data in the database
///
/// Unlike BPM/key/LUFS reanalysis, this does NOT use a subprocess — ort is thread-safe.
fn reanalyze_ml_track(
    path: &Path,
    ml_analyzer: &Arc<Mutex<crate::ml_analysis::MlAnalyzer>>,
    db: &Arc<DatabaseService>,
) -> Result<()> {
    log::info!("reanalyze_ml_track: {:?}", path);

    // Load audio from existing file
    let mut reader = AudioFileReader::open(path)
        .with_context(|| format!("Failed to open file: {:?}", path))?;

    let stems = reader
        .read_all_stems()
        .with_context(|| format!("Failed to read stems from: {:?}", path))?;

    // Look up track_id by path
    let path_str = path.to_string_lossy();
    let track_id = match db.get_track_by_path(&path_str) {
        Ok(Some(track)) => match track.id {
            Some(id) => id,
            None => {
                anyhow::bail!("Track has no ID: {}", path_str);
            }
        },
        Ok(None) => {
            anyhow::bail!("Track not found in database: {}", path_str);
        }
        Err(e) => {
            anyhow::bail!("Failed to look up track: {:?}", e);
        }
    };

    // Step 1: Vocal presence (pure Rust RMS on vocal stem)
    let vocals_mono = create_vocals_mono(&stems);
    let vocal_presence = crate::ml_analysis::compute_vocal_presence(&vocals_mono, SAMPLE_RATE);
    log::info!(
        "reanalyze_ml_track: vocal_presence={:.2} for {:?}",
        vocal_presence,
        path
    );

    // Step 2: Mono mix → mel spectrogram (pure Rust DSP, input to EffNet)
    let mono_mix = create_mono_mix(&stems);
    let mel = crate::ml_analysis::preprocessing::compute_mel_spectrogram(
        &mono_mix,
        SAMPLE_RATE as f32,
    )
    .map_err(|e| anyhow::anyhow!("Mel spectrogram failed: {}", e))?;

    log::info!(
        "reanalyze_ml_track: mel spectrogram {} frames × {} bands",
        mel.frames.len(),
        mel.n_bands
    );

    // Step 3: Run EffNet + classification heads (genre, mood, arousal/valence)
    let ml_result = {
        let mut analyzer = ml_analyzer
            .lock()
            .map_err(|e| anyhow::anyhow!("MlAnalyzer lock poisoned: {}", e))?;
        analyzer
            .analyze(&mel, vocal_presence)
            .map_err(|e| anyhow::anyhow!("ML inference failed: {}", e))?
    };

    log::info!(
        "reanalyze_ml_track: genre={:?}, arousal={:?}, valence={:?}",
        ml_result.top_genre,
        ml_result.arousal,
        ml_result.valence
    );

    // Step 4: Store ML analysis data in database
    if let Err(e) = db.store_ml_analysis(track_id, &ml_result) {
        log::error!("reanalyze_ml_track: Failed to store ML analysis: {:?}", e);
    }

    // Step 5: Clear old ML-generated tags, then auto-tag with new results
    crate::batch_import::clear_ml_tags(track_id, db);
    crate::batch_import::auto_tag_from_ml(track_id, &ml_result, db);

    log::info!("reanalyze_ml_track: Complete for {:?}", path);
    Ok(())
}

/// Run batch ML/similarity reanalysis on multiple tracks
///
/// Initializes the MlAnalyzer (EffNet + optional mood head), then processes
/// tracks in parallel using rayon. Each track gets: vocal presence detection,
/// mel spectrogram computation, EffNet inference, and auto-tagging.
///
/// # Arguments
/// * `tracks` - List of track file paths to re-analyze
/// * `experimental` - If true, also run Jamendo mood model (enables arousal/valence)
/// * `parallel_processes` - Number of parallel workers (1-16)
/// * `progress_tx` - Channel to send progress updates
/// * `cancel_flag` - Atomic flag to check for cancellation
/// * `db` - Database service for storing results
pub fn run_batch_ml_reanalysis(
    tracks: Vec<PathBuf>,
    experimental: bool,
    parallel_processes: u8,
    progress_tx: Sender<ReanalysisProgress>,
    cancel_flag: Arc<AtomicBool>,
    db: Arc<DatabaseService>,
) {
    use crate::ml_analysis;
    use rayon::prelude::*;

    let start_time = Instant::now();
    let total = tracks.len();

    log::info!(
        "run_batch_ml_reanalysis: Starting similarity analysis for {} tracks (experimental={})",
        total,
        experimental
    );

    // Send start notification
    let _ = progress_tx.send(ReanalysisProgress::Started {
        total_tracks: total,
        analysis_type: AnalysisType::Similarity,
    });

    // Check for early cancellation
    if cancel_flag.load(Ordering::Relaxed) {
        log::info!("run_batch_ml_reanalysis: Cancelled before processing");
        let _ = progress_tx.send(ReanalysisProgress::AllComplete {
            succeeded: 0,
            failed: 0,
            duration: Duration::ZERO,
        });
        return;
    }

    // Initialize ML analyzer (downloads models if needed)
    let ml_analyzer: Arc<Mutex<ml_analysis::MlAnalyzer>> = {
        let mgr = match ml_analysis::MlModelManager::new() {
            Ok(mgr) => mgr,
            Err(e) => {
                log::error!("run_batch_ml_reanalysis: Cannot determine model cache dir: {}", e);
                let _ = progress_tx.send(ReanalysisProgress::AllComplete {
                    succeeded: 0,
                    failed: total,
                    duration: start_time.elapsed(),
                });
                return;
            }
        };

        if let Err(e) = mgr.ensure_all_models(experimental) {
            log::warn!("run_batch_ml_reanalysis: Failed to download ML models: {}", e);
        }

        let model_dir = mgr
            .model_path(ml_analysis::MlModelType::EffNetEmbedding)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();

        match ml_analysis::MlAnalyzer::new(&model_dir, experimental) {
            Ok(analyzer) => {
                log::info!(
                    "run_batch_ml_reanalysis: ML analyzer initialized (experimental={})",
                    experimental
                );
                Arc::new(Mutex::new(analyzer))
            }
            Err(e) => {
                log::error!("run_batch_ml_reanalysis: ML models not available: {}", e);
                let _ = progress_tx.send(ReanalysisProgress::AllComplete {
                    succeeded: 0,
                    failed: total,
                    duration: start_time.elapsed(),
                });
                return;
            }
        }
    };

    // Configure rayon thread pool
    let num_workers = parallel_processes.clamp(1, 16) as usize;
    log::info!("run_batch_ml_reanalysis: Using {} parallel workers", num_workers);
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
                if cancel_flag.load(Ordering::Relaxed) {
                    return false;
                }

                let track_name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown")
                    .to_string();

                let _ = progress_tx.send(ReanalysisProgress::TrackStarted {
                    track_name: track_name.clone(),
                    index,
                    total,
                });

                match reanalyze_ml_track(path, &ml_analyzer, &db) {
                    Ok(()) => {
                        let _ = progress_tx.send(ReanalysisProgress::TrackCompleted {
                            track_name,
                            success: true,
                            error: None,
                        });
                        true
                    }
                    Err(e) => {
                        log::error!("run_batch_ml_reanalysis: Failed for {:?}: {}", path, e);
                        let _ = progress_tx.send(ReanalysisProgress::TrackCompleted {
                            track_name,
                            success: false,
                            error: Some(e.to_string()),
                        });
                        false
                    }
                }
            })
            .collect()
    });

    let duration = start_time.elapsed();
    let succeeded = results.iter().filter(|&&s| s).count();
    let failed = results.len() - succeeded;

    log::info!(
        "run_batch_ml_reanalysis: Complete in {:.1}s - {} succeeded, {} failed",
        duration.as_secs_f64(),
        succeeded,
        failed
    );

    let _ = progress_tx.send(ReanalysisProgress::AllComplete {
        succeeded,
        failed,
        duration,
    });
}
