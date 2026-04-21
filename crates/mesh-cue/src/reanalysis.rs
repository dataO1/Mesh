//! Re-analysis system for existing tracks
//!
//! Provides selective re-analysis of tracks without full re-import.
//! Supports updating only specific metadata (LUFS, BPM, Key) while
//! preserving existing cue points, loops, and other data.

use crate::analysis::{
    analyze_partial_in_subprocess, AnalysisType, MetadataOptions,
    PartialAnalysisResult, ReanalysisProgress, SubprocessTask,
};
use crate::config::{BpmConfig, BpmSource};
use anyhow::{Context, Result};
use mesh_core::audio_file::AudioFileReader;
use mesh_core::db::DatabaseService;
use mesh_core::types::SAMPLE_RATE;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Re-analyze a single track's BPM and beat grid
///
/// This function:
/// 1. Loads the audio from the existing WAV file
/// 2. Creates a mono mix for analysis
/// 3. Runs BPM detection in an isolated Essentia subprocess
/// 4. Updates the database with new BPM and beat grid
///
/// # Arguments
/// * `path` - Path to the WAV file to re-analyze
/// * `bpm_config` - BPM detection configuration
/// * `db` - Optional database service for storing analysis results
///
/// # Returns
/// The partial analysis result with BPM and beat grid
pub fn reanalyze_track(
    path: &Path,
    bpm_config: &BpmConfig,
    db: Option<&Arc<DatabaseService>>,
) -> Result<PartialAnalysisResult> {
    log::info!("reanalyze_track: Beats analysis on {:?}", path);

    // Load audio from existing file
    let reader = AudioFileReader::open(path)
        .with_context(|| format!("Failed to open file: {:?}", path))?;

    let file_sample_rate = reader.format().sample_rate;

    // Read stems at 44100 Hz for Essentia analysis (Essentia expects 44100 Hz input).
    // Collection files are typically at 48000 Hz — reading at 44100 triggers rubato
    // resampling via read_all_stems_to(), giving Essentia correctly-rated audio.
    const ESSENTIA_RATE: u32 = 44100;
    let stems = reader
        .read_all_stems_to(ESSENTIA_RATE)
        .with_context(|| format!("Failed to read stems from: {:?}", path))?;

    // Create full mix for subprocess (key/LUFS always need all audio content)
    let mono_samples = create_mono_mix(&stems);

    // Create separate BPM mono when drums-only is configured.
    // When FullMix, bpm_mono is None — subprocess will use mono_samples.
    let bpm_mono: Option<Vec<f32>> = match bpm_config.source {
        BpmSource::Drums => {
            log::info!("reanalyze_track: Using drums-only for BPM analysis");
            Some(create_drums_mono(&stems))
        }
        BpmSource::FullMix => None,
    };

    log::info!(
        "reanalyze_track: {} samples ({:.1}s at {} Hz, file was {} Hz), BPM source: {}",
        mono_samples.len(),
        mono_samples.len() as f64 / ESSENTIA_RATE as f64,
        ESSENTIA_RATE,
        file_sample_rate,
        bpm_config.source,
    );

    // Run BPM analysis in subprocess (Essentia for BPM detection and beat grid)
    // Full mix goes as main samples; drums-only bpm_mono is passed separately
    let result = analyze_partial_in_subprocess(mono_samples, bpm_mono, SubprocessTask::Beats(bpm_config.clone()))
        .with_context(|| format!("Beats analysis failed for: {:?}", path))?;

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
        }

        // Update first_beat_sample from beat grid
        if let Some(first_beat) = result.beat_grid.as_ref().and_then(|g| g.first().copied()) {
            if let Err(e) = db_service.update_track_field(track_id, "first_beat_sample", &first_beat.to_string()) {
                log::error!("Failed to update first_beat_sample in database: {:?}", e);
            }
        }

        log::info!("reanalyze_track: Updated database for {:?}", path);
    } else {
        log::warn!("reanalyze_track: No database provided, analysis results not persisted");
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

/// Create a drums-only mono signal from stem buffers for BPM analysis
fn create_drums_mono(stems: &mesh_core::audio_file::StemBuffers) -> Vec<f32> {
    let len = stems.len();
    let mut mono = Vec::with_capacity(len);

    for i in 0..len {
        mono.push((stems.drums[i].left + stems.drums[i].right) * 0.5);
    }

    mono
}

/// Run batch beats re-analysis on multiple tracks
///
/// Processes tracks in parallel using rayon thread pool.
/// Sends progress updates through the channel for UI display.
/// Analysis results are stored directly in the database.
///
/// # Arguments
/// * `tracks` - List of track file paths to re-analyze
/// * `bpm_config` - BPM detection configuration
/// * `parallel_processes` - Number of parallel workers (1-16)
/// * `progress_tx` - Channel to send progress updates
/// * `cancel_flag` - Atomic flag to check for cancellation
/// * `db` - Optional database service for storing results
pub fn run_batch_reanalysis(
    tracks: Vec<PathBuf>,
    bpm_config: BpmConfig,
    parallel_processes: u8,
    progress_tx: Sender<ReanalysisProgress>,
    cancel_flag: std::sync::Arc<AtomicBool>,
    db: Option<Arc<DatabaseService>>,
) {
    use rayon::prelude::*;

    let start_time = Instant::now();
    let total = tracks.len();

    log::info!(
        "run_batch_reanalysis: Starting Beats analysis for {} tracks",
        total
    );

    // Send start notification
    let _ = progress_tx.send(ReanalysisProgress::Started {
        total_tracks: total,
        analysis_type: AnalysisType::Beats,
        metadata_options: None,
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
                let success = match reanalyze_track(path, &bpm_config, db.as_ref()) {
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
// Metadata Reanalysis (Name/Artist, Loudness, Key, ML Tags)
// ============================================================================

/// Re-analyze a single track's metadata based on selected options
///
/// Steps (only runs what's ticked):
/// 1. Name/Artist: look up original_name from DB, re-parse with metadata module
/// 2. Essentia subprocess (LUFS and/or Key): read audio at 44100Hz, run analysis
/// 3. ML features (Tags): compute mel spectrogram, run EffNet, auto-tag
fn reanalyze_metadata_track(
    path: &Path,
    options: &MetadataOptions,
    db: &Arc<DatabaseService>,
    known_artists: &HashSet<String>,
    ml_analyzer: Option<&Arc<Mutex<crate::ml_analysis::MlAnalyzer>>>,
) -> Result<()> {
    log::info!("reanalyze_metadata_track: {:?} (name={}, loudness={}, key={}, tags={})",
        path, options.name_artist, options.loudness, options.key, options.tags);

    // Look up track by path to get track_id and original_name
    let path_str = path.to_string_lossy();
    let track = db.get_track_by_path(&path_str)
        .with_context(|| format!("Failed to look up track: {}", path_str))?
        .ok_or_else(|| anyhow::anyhow!("Track not found in database: {}", path_str))?;
    let track_id = track.id.ok_or_else(|| anyhow::anyhow!("Track has no ID: {}", path_str))?;

    // Step 1: Name/Artist re-parsing
    if options.name_artist {
        let original = &track.original_name;
        if !original.is_empty() {
            let resolved = crate::metadata::resolve_metadata(None, original, known_artists);
            if let Err(e) = db.update_track_field(track_id, "title", &resolved.title) {
                log::error!("reanalyze_metadata_track: Failed to update title: {:?}", e);
            }
            if let Some(ref artist) = resolved.artist {
                if let Err(e) = db.update_track_field(track_id, "artist", artist) {
                    log::error!("reanalyze_metadata_track: Failed to update artist: {:?}", e);
                }
            }
            log::info!("reanalyze_metadata_track: Re-parsed title='{}', artist={:?}",
                resolved.title, resolved.artist);
        } else {
            log::info!("reanalyze_metadata_track: No original_name stored, skipping name re-parse");
        }
    }

    // Step 2: Essentia subprocess for LUFS and/or Key
    if options.needs_essentia() {
        const ESSENTIA_RATE: u32 = 44100;
        let reader = AudioFileReader::open(path)
            .with_context(|| format!("Failed to open file: {:?}", path))?;
        let stems = reader
            .read_all_stems_to(ESSENTIA_RATE)
            .with_context(|| format!("Failed to read stems from: {:?}", path))?;
        let mono_samples = create_mono_mix(&stems);

        let essentia_opts = MetadataOptions {
            name_artist: false,
            loudness: options.loudness,
            key: options.key,
            tags: false,
        };

        let result = analyze_partial_in_subprocess(
            mono_samples, None, SubprocessTask::Metadata(essentia_opts),
        ).with_context(|| format!("Essentia analysis failed for: {:?}", path))?;

        // Update database with Essentia results
        if let Some(lufs) = result.lufs {
            if let Err(e) = db.update_track_field(track_id, "lufs", &lufs.to_string()) {
                log::error!("reanalyze_metadata_track: Failed to update LUFS: {:?}", e);
            }
        }
        if let Some(integrated) = result.integrated_lufs {
            if let Err(e) = db.update_track_field(track_id, "integrated_lufs", &integrated.to_string()) {
                log::error!("reanalyze_metadata_track: Failed to update integrated LUFS: {:?}", e);
            }
        }
        if let Some(ref key) = result.key {
            if let Err(e) = db.update_track_field(track_id, "key", key) {
                log::error!("reanalyze_metadata_track: Failed to update key: {:?}", e);
            }
        }
    }

    // Step 3: ML features (Tags) — ort is thread-safe, no subprocess needed
    if options.tags {
        if let Some(ml_arc) = ml_analyzer {
            // Load audio ONCE at native rate — used for ML mel spectrogram + stem energy RMS
            let reader = AudioFileReader::open(path)
                .with_context(|| format!("Failed to open file: {:?}", path))?;
            let stems = reader
                .read_all_stems()
                .with_context(|| format!("Failed to read stems from: {:?}", path))?;
            let mono_mix = create_mono_mix(&stems);

            // ML analysis on native-rate mono mix
            let mel = crate::ml_analysis::preprocessing::compute_mel_spectrogram(
                &mono_mix, SAMPLE_RATE as f32,
            ).map_err(|e| anyhow::anyhow!("Mel spectrogram failed: {}", e))?;

            let ml_result = {
                let mut analyzer = ml_arc.lock()
                    .map_err(|e| anyhow::anyhow!("MlAnalyzer lock poisoned: {}", e))?;
                analyzer.analyze(&mel)
                    .map_err(|e| anyhow::anyhow!("ML inference failed: {}", e))?
            };

            log::info!(
                "reanalyze_metadata_track: ML genre={:?}, vocal={:.3}",
                ml_result.data.top_genre, ml_result.data.vocal_presence
            );

            if let Err(e) = db.store_ml_analysis(track_id, &ml_result.data) {
                log::error!("reanalyze_metadata_track: Failed to store ML analysis: {:?}", e);
            }
            crate::batch_import::clear_ml_tags(track_id, db);
            crate::batch_import::auto_tag_from_ml(track_id, &ml_result.data, db);

            // Persist EffNet embedding
            if ml_result.embedding.len() == 1280 {
                if let Err(e) = db.store_ml_embedding(track_id, &ml_result.embedding) {
                    log::warn!("reanalyze_metadata_track: Failed to store ML embedding: {:?}", e);
                }
            }

            // Stem energy densities — reuse already-loaded stems (no extra file open)
            let (vocal, drums, bass, other) = crate::batch_import::compute_stem_energy_ratios(&stems);
            if let Err(e) = db.store_stem_energy(track_id, vocal, drums, bass, other) {
                log::warn!("reanalyze_metadata_track: Failed to store stem energy: {:?}", e);
            }

            // Intensity components — load ONCE at 44100 Hz for Essentia features
            const ESSENTIA_RATE_FEAT: u32 = 44100;
            match AudioFileReader::open(path).and_then(|r| r.read_all_stems_to(ESSENTIA_RATE_FEAT)) {
                Ok(feat_stems) => {
                    let mono_44 = create_mono_mix(&feat_stems);
                    // Compute multi-frame intensity components (pure Rust, no subprocess)
                    let intensity_components = crate::features::compute_intensity_components(&mono_44, ESSENTIA_RATE_FEAT as f32);

                    match crate::features::extract_audio_features_in_subprocess(mono_44) {
                        Ok(features) => {
                            // Merge: full-track centroid + energy_variance from Essentia,
                            // multi-frame values for flux, flatness, dissonance, etc. from pure Rust
                            let mut ic = intensity_components;
                            ic.spectral_centroid = features.spectral_centroid;
                            ic.energy_variance = features.energy_variance;
                            if let Err(e) = db.store_intensity_components(track_id, &ic) {
                                log::warn!("reanalyze_metadata_track: Failed to store intensity components: {:?}", e);
                            }
                        }
                        Err(e) => {
                            log::warn!("reanalyze_metadata_track: Feature extraction failed: {:?}", e);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("reanalyze_metadata_track: Could not load stems for feature extraction: {:?}", e);
                }
            }
        } else {
            log::warn!("reanalyze_metadata_track: Tags requested but ML analyzer not available");
        }
    }

    log::info!("reanalyze_metadata_track: Complete for {:?}", path);
    Ok(())
}

/// Run batch metadata reanalysis on multiple tracks
///
/// Processes tracks in parallel using rayon thread pool.
/// Initializes shared resources once (known_artists, MlAnalyzer) before the batch.
///
/// # Arguments
/// * `tracks` - List of track file paths to re-analyze
/// * `options` - Which metadata sub-analyses to run
/// * `parallel_processes` - Number of parallel workers (1-16)
/// * `progress_tx` - Channel to send progress updates
/// * `cancel_flag` - Atomic flag to check for cancellation
/// * `db` - Database service for storing results
pub fn run_batch_metadata_reanalysis(
    tracks: Vec<PathBuf>,
    options: MetadataOptions,
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
        "run_batch_metadata_reanalysis: Starting for {} tracks (name={}, loudness={}, key={}, tags={})",
        total, options.name_artist, options.loudness, options.key, options.tags
    );

    // Send start notification
    let _ = progress_tx.send(ReanalysisProgress::Started {
        total_tracks: total,
        analysis_type: AnalysisType::Metadata,
        metadata_options: Some(options),
    });

    // Check for early cancellation
    if cancel_flag.load(Ordering::Relaxed) {
        log::info!("run_batch_metadata_reanalysis: Cancelled before processing");
        let _ = progress_tx.send(ReanalysisProgress::AllComplete {
            succeeded: 0,
            failed: 0,
            duration: Duration::ZERO,
        });
        return;
    }

    // Load known artists once for name re-parsing (if needed)
    let known_artists: HashSet<String> = if options.name_artist {
        crate::metadata::get_known_artists(&db)
    } else {
        HashSet::new()
    };

    // Initialize ML analyzer once for the entire batch (if Tags is ticked)
    let ml_analyzer: Option<Arc<Mutex<ml_analysis::MlAnalyzer>>> = if options.tags {
        match ml_analysis::MlModelManager::new() {
            Ok(mgr) => {
                if let Err(e) = mgr.ensure_all_models() {
                    log::warn!("run_batch_metadata_reanalysis: Failed to download ML models: {}", e);
                }
                let model_dir = mgr
                    .model_path(ml_analysis::MlModelType::EffNetEmbedding)
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .to_path_buf();
                match ml_analysis::MlAnalyzer::new(&model_dir) {
                    Ok(analyzer) => {
                        log::info!("run_batch_metadata_reanalysis: ML analyzer initialized");
                        Some(Arc::new(Mutex::new(analyzer)))
                    }
                    Err(e) => {
                        log::warn!("run_batch_metadata_reanalysis: ML models not available, skipping tags: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                log::error!("run_batch_metadata_reanalysis: Cannot determine model cache dir: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Configure rayon thread pool
    let num_workers = parallel_processes.clamp(1, 16) as usize;
    log::info!("run_batch_metadata_reanalysis: Using {} parallel workers", num_workers);
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

                match reanalyze_metadata_track(path, &options, &db, &known_artists, ml_analyzer.as_ref()) {
                    Ok(()) => {
                        let _ = progress_tx.send(ReanalysisProgress::TrackCompleted {
                            track_name,
                            success: true,
                            error: None,
                        });
                        true
                    }
                    Err(e) => {
                        log::error!("run_batch_metadata_reanalysis: Failed for {:?}: {}", path, e);
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
        "run_batch_metadata_reanalysis: Complete in {:.1}s - {} succeeded, {} failed",
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
