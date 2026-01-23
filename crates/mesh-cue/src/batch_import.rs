//! Batch import system for stem files
//!
//! Scans an import folder for stem files, groups them by track name,
//! and processes them in parallel using a worker pool.
//!
//! # File Naming Convention
//!
//! Stems should follow the pattern: `BaseName_(StemType).wav`
//! - `Artist - Track_(Vocals).wav`
//! - `Artist - Track_(Drums).wav`
//! - `Artist - Track_(Bass).wav`
//! - `Artist - Track_(Other).wav`
//!
//! # Usage
//!
//! ```ignore
//! let (progress_tx, progress_rx) = std::sync::mpsc::channel();
//! let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
//!
//! let config = ImportConfig {
//!     import_folder: PathBuf::from("~/Music/mesh-collection/import"),
//!     collection_path: PathBuf::from("~/Music/mesh-collection"),
//!     bpm_config: BpmConfig::default(),
//! };
//!
//! std::thread::spawn(move || {
//!     run_batch_import(config, progress_tx, cancel_rx);
//! });
//!
//! // Poll progress_rx for updates
//! ```

use crate::analysis::{analyze_audio, AnalysisResult};
use crate::config::{BpmConfig, BpmSource, LoudnessConfig};
use crate::export::export_stem_file_with_gain;
use crate::import::StemImporter;
use anyhow::{Context, Result};
use mesh_core::db::{DatabaseService, Track};
use mesh_core::types::SAMPLE_RATE;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;

/// RAII guard for temp file cleanup - deletes file on drop unless disarmed.
///
/// This ensures temp files are cleaned up even on early returns or panics.
/// Use `disarm()` if you want to keep the file (e.g., after successful move).
struct TempFileGuard {
    path: PathBuf,
    disarmed: bool,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, disarmed: false }
    }

    /// Prevent cleanup on drop (call after successful move/rename)
    #[allow(dead_code)]
    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if !self.disarmed {
            if let Err(e) = std::fs::remove_file(&self.path) {
                // Only warn if file exists - it's OK if it was never created
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("Failed to cleanup temp file {:?}: {}", self.path, e);
                }
            }
        }
    }
}

/// Run audio analysis in an isolated subprocess.
///
/// Essentia's C++ library is NOT thread-safe - it has global state for logging,
/// FFT plan caches, and algorithm registries. Running multiple threads causes
/// segfaults and garbled output.
///
/// By spawning each analysis in a separate process, we get true isolation:
/// each process has its own copy of Essentia's globals.
///
/// NOTE: We use temp files instead of serializing samples over IPC.
/// Serializing 14M+ f32 samples (56MB+) through procspawn's bincode/IPC
/// causes failures due to buffer limits and memory pressure.
///
/// See: <https://github.com/MTG/essentia/issues/87>
fn analyze_in_subprocess(samples: Vec<f32>, bpm_config: BpmConfig) -> Result<AnalysisResult> {
    use std::io::{Read, Write};

    // Generate unique temp file path
    let temp_path = std::env::temp_dir().join(format!(
        "mesh_audio_{}.bin",
        std::process::id() ^ (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u32)
    ));

    // RAII guard ensures cleanup on any exit path (early return, panic, or normal)
    let _temp_guard = TempFileGuard::new(temp_path.clone());

    // Write samples to temp file (raw f32 bytes)
    {
        let mut file = std::fs::File::create(&temp_path)
            .with_context(|| format!("Failed to create temp file: {:?}", temp_path))?;
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                samples.as_ptr() as *const u8,
                samples.len() * std::mem::size_of::<f32>(),
            )
        };
        file.write_all(bytes)
            .with_context(|| "Failed to write samples to temp file")?;
    }
    let sample_count = samples.len();
    drop(samples); // Free memory before spawning subprocess

    // Spawn subprocess with only the temp file path (small data)
    let temp_path_str = temp_path.to_string_lossy().to_string();
    let handle = procspawn::spawn((temp_path_str.clone(), sample_count, bpm_config), |(path, count, config)| {
        // Read samples from temp file in subprocess
        let samples = (|| -> std::result::Result<Vec<f32>, String> {
            let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let mut bytes = vec![0u8; count * std::mem::size_of::<f32>()];
            file.read_exact(&mut bytes).map_err(|e| e.to_string())?;

            // Convert bytes back to f32
            let samples: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();
            Ok(samples)
        })()?;

        // Run analysis in isolated process
        analyze_audio(&samples, &config).map_err(|e| e.to_string())
    });

    // Wait for result - temp file cleanup is handled by _temp_guard on drop
    handle
        .join()
        .map_err(|e| anyhow::anyhow!("Analysis subprocess failed: {:?}", e))?
        .map_err(|e| anyhow::anyhow!("Analysis error: {}", e))
}

/// Stem types supported by the import system
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StemType {
    Vocals,
    Drums,
    Bass,
    Other,
}

impl StemType {
    /// Parse stem type from filename suffix (case-insensitive)
    pub fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix.to_lowercase().as_str() {
            "vocals" => Some(StemType::Vocals),
            "drums" => Some(StemType::Drums),
            "bass" => Some(StemType::Bass),
            "other" | "instrumental" => Some(StemType::Other),
            _ => None,
        }
    }
}

/// A group of stems forming a complete track
#[derive(Debug, Clone)]
pub struct StemGroup {
    /// Base name of the track (e.g., "Artist - Track")
    pub base_name: String,
    /// Path to vocals stem
    pub vocals: Option<PathBuf>,
    /// Path to drums stem
    pub drums: Option<PathBuf>,
    /// Path to bass stem
    pub bass: Option<PathBuf>,
    /// Path to other/instrumental stem
    pub other: Option<PathBuf>,
}

impl StemGroup {
    /// Create a new empty stem group
    pub fn new(base_name: String) -> Self {
        Self {
            base_name,
            vocals: None,
            drums: None,
            bass: None,
            other: None,
        }
    }

    /// Check if all 4 stems are present
    pub fn is_complete(&self) -> bool {
        self.vocals.is_some()
            && self.drums.is_some()
            && self.bass.is_some()
            && self.other.is_some()
    }

    /// Get count of loaded stems (0-4)
    pub fn stem_count(&self) -> usize {
        [
            self.vocals.is_some(),
            self.drums.is_some(),
            self.bass.is_some(),
            self.other.is_some(),
        ]
        .iter()
        .filter(|&&b| b)
        .count()
    }

    /// Set a stem path by type
    pub fn set_stem(&mut self, stem_type: StemType, path: PathBuf) {
        match stem_type {
            StemType::Vocals => self.vocals = Some(path),
            StemType::Drums => self.drums = Some(path),
            StemType::Bass => self.bass = Some(path),
            StemType::Other => self.other = Some(path),
        }
    }

    /// Get all source stem paths (for deletion after import)
    pub fn all_paths(&self) -> Vec<&Path> {
        [&self.vocals, &self.drums, &self.bass, &self.other]
            .iter()
            .filter_map(|opt| opt.as_deref())
            .collect()
    }
}

/// Result from processing a single track
#[derive(Debug, Clone)]
pub struct TrackImportResult {
    /// Base name of the track
    pub base_name: String,
    /// Whether the import succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Output path in collection (if successful)
    pub output_path: Option<PathBuf>,
}

/// Progress updates sent from import thread to UI
#[derive(Debug, Clone)]
pub enum ImportProgress {
    /// Import started, groups detected
    Started {
        total: usize,
    },
    /// Starting to process a track
    TrackStarted {
        base_name: String,
        index: usize,
        total: usize,
    },
    /// Finished processing a track
    TrackCompleted(TrackImportResult),
    /// All imports complete
    AllComplete {
        results: Vec<TrackImportResult>,
    },
}

/// Configuration for batch import
#[derive(Clone)]
pub struct ImportConfig {
    /// Folder to scan for stems
    pub import_folder: PathBuf,
    /// Collection folder to export to
    pub collection_path: PathBuf,
    /// Shared database service for thread-safe track insertion
    pub db_service: Arc<DatabaseService>,
    /// BPM detection configuration
    pub bpm_config: BpmConfig,
    /// Loudness normalization configuration
    pub loudness_config: LoudnessConfig,
    /// Number of parallel analysis processes (1-16)
    pub parallel_processes: u8,
}

/// Parse a stem filename to extract base name and stem type
///
/// Supported patterns:
/// - `BaseName_(Vocals).wav` → Some(("BaseName", Vocals))
/// - `BaseName_(Drums).wav` → Some(("BaseName", Drums))
/// - `BaseName_(Bass).wav` → Some(("BaseName", Bass))
/// - `BaseName_(Other).wav` → Some(("BaseName", Other))
/// - `BaseName_(Instrumental).wav` → Some(("BaseName", Other))
///
/// Returns None if the filename doesn't match the expected pattern.
pub fn parse_stem_filename(filename: &str) -> Option<(String, StemType)> {
    // Remove .wav extension (case-insensitive)
    let name = filename.strip_suffix(".wav").or_else(|| filename.strip_suffix(".WAV"))?;

    // Find the stem type suffix: _(Type)
    let suffix_start = name.rfind("_(")?;
    let suffix_end = name.rfind(')')?;

    if suffix_end <= suffix_start + 2 {
        return None;
    }

    // Extract parts
    let base_name = &name[..suffix_start];
    let stem_suffix = &name[suffix_start + 2..suffix_end];

    // Parse stem type
    let stem_type = StemType::from_suffix(stem_suffix)?;

    Some((base_name.to_string(), stem_type))
}

/// Scan import folder and group stems by track name
///
/// Returns a list of stem groups, each containing 0-4 stems.
/// Only complete groups (all 4 stems) can be imported.
pub fn scan_and_group_stems(import_folder: &Path) -> Result<Vec<StemGroup>> {
    log::info!("scan_and_group_stems: Scanning {:?}", import_folder);

    // Ensure directory exists
    if !import_folder.exists() {
        fs::create_dir_all(import_folder)
            .with_context(|| format!("Failed to create import folder: {:?}", import_folder))?;
        return Ok(Vec::new());
    }

    // Build a map of base_name -> StemGroup
    let mut groups: HashMap<String, StemGroup> = HashMap::new();

    let entries = fs::read_dir(import_folder)
        .with_context(|| format!("Failed to read import folder: {:?}", import_folder))?;

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip non-WAV files
        if !path
            .extension()
            .map_or(false, |ext| ext.eq_ignore_ascii_case("wav"))
        {
            continue;
        }

        // Get filename
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        // Parse the filename
        if let Some((base_name, stem_type)) = parse_stem_filename(filename) {
            log::debug!(
                "scan_and_group_stems: Found {:?} stem for '{}'",
                stem_type,
                base_name
            );

            // Get or create group
            let group = groups
                .entry(base_name.clone())
                .or_insert_with(|| StemGroup::new(base_name));

            // Add this stem
            group.set_stem(stem_type, path);
        } else {
            log::warn!(
                "scan_and_group_stems: Couldn't parse filename: {}",
                filename
            );
        }
    }

    // Convert to sorted vec
    let mut result: Vec<StemGroup> = groups.into_values().collect();
    result.sort_by(|a, b| a.base_name.cmp(&b.base_name));

    log::info!(
        "scan_and_group_stems: Found {} track groups ({} complete)",
        result.len(),
        result.iter().filter(|g| g.is_complete()).count()
    );

    Ok(result)
}

/// Process a single track group: load stems, analyze, export
///
/// This is run by worker threads.
fn process_single_track(group: &StemGroup, config: &ImportConfig) -> TrackImportResult {
    let base_name = group.base_name.clone();
    log::info!("process_single_track: Processing '{}'", base_name);

    // Verify group is complete
    if !group.is_complete() {
        return TrackImportResult {
            base_name,
            success: false,
            error: Some(format!(
                "Incomplete stem group: only {}/4 stems found",
                group.stem_count()
            )),
            output_path: None,
        };
    }

    // Set up the importer
    let mut importer = StemImporter::new();
    importer.set_vocals(group.vocals.as_ref().unwrap());
    importer.set_drums(group.drums.as_ref().unwrap());
    importer.set_bass(group.bass.as_ref().unwrap());
    importer.set_other(group.other.as_ref().unwrap());

    // Load and combine stems
    let imported = match importer.import() {
        Ok(b) => b,
        Err(e) => {
            return TrackImportResult {
                base_name,
                success: false,
                error: Some(format!("Failed to load stems: {}", e)),
                output_path: None,
            };
        }
    };
    let source_sample_rate = imported.source_sample_rate;
    let buffers = imported.buffers;

    // Get mono audio for BPM analysis based on configured source
    let mono_samples = match config.bpm_config.source {
        BpmSource::Drums => {
            log::info!("process_single_track: Using drums-only for BPM analysis");
            importer.get_drums_mono()
        }
        BpmSource::FullMix => {
            log::info!("process_single_track: Using full mix for BPM analysis");
            importer.get_mono_sum()
        }
    };
    let mono_samples = match mono_samples {
        Ok(s) => s,
        Err(e) => {
            return TrackImportResult {
                base_name,
                success: false,
                error: Some(format!("Failed to create mono audio for analysis: {}", e)),
                output_path: None,
            };
        }
    };

    // Analyze audio in isolated subprocess (Essentia is not thread-safe)
    let analysis = match analyze_in_subprocess(mono_samples, config.bpm_config.clone()) {
        Ok(a) => a,
        Err(e) => {
            return TrackImportResult {
                base_name,
                success: false,
                error: Some(format!("Analysis failed: {}", e)),
                output_path: None,
            };
        }
    };

    log::info!(
        "process_single_track: '{}' analyzed: BPM={:.1}, Key={}",
        base_name,
        analysis.bpm,
        analysis.key
    );

    // Extract artist from base_name if in "Artist - Track" format
    let artist = base_name
        .split(" - ")
        .next()
        .map(|s| s.to_string());

    // Calculate resampling ratio: target samples will differ from source if rates mismatch
    // E.g., 44100 Hz input → 48000 Hz output means samples scale by 48000/44100 = 1.088
    let resample_ratio = SAMPLE_RATE as f64 / source_sample_rate as f64;

    // Get duration in samples for beat grid generation (at TARGET sample rate after resampling)
    let source_duration_samples = buffers.len() as u64;
    let duration_samples = (source_duration_samples as f64 * resample_ratio) as u64;

    // Get first beat position from analysis and scale to target sample rate
    let source_first_beat = analysis.beat_grid.first().copied().unwrap_or(0);
    let first_beat = (source_first_beat as f64 * resample_ratio) as u64;

    log::info!(
        "process_single_track: Resampling {} Hz → {} Hz (ratio: {:.4}), duration: {} → {} samples, first_beat: {} → {}",
        source_sample_rate, SAMPLE_RATE, resample_ratio, source_duration_samples, duration_samples, source_first_beat, first_beat
    );

    // Export to temp file first (export handles resampling from source_sample_rate to SAMPLE_RATE)
    let temp_dir = std::env::temp_dir();
    let sanitized_name = sanitize_filename(&base_name);
    let temp_path = temp_dir.join(format!("{}.wav", sanitized_name));

    // RAII guard ensures temp file cleanup on any exit path (early return, panic, or normal)
    let _temp_guard = TempFileGuard::new(temp_path.clone());

    // Calculate waveform gain from LUFS for loudness-normalized preview
    // The new LoudnessConfig API handles Option<f32> directly and returns 1.0 if None
    let waveform_gain = config.loudness_config.calculate_gain_linear(analysis.lufs);

    // Export audio only - all metadata (BPM, key, cues, loops) is stored in the database
    if let Err(e) = export_stem_file_with_gain(&temp_path, &buffers, source_sample_rate, waveform_gain) {
        return TrackImportResult {
            base_name,
            success: false,
            error: Some(format!("Export failed: {}", e)),
            output_path: None,
        };
    }

    // Move to collection
    let tracks_dir = config.collection_path.join("tracks");
    if let Err(e) = fs::create_dir_all(&tracks_dir) {
        return TrackImportResult {
            base_name,
            success: false,
            error: Some(format!("Failed to create tracks directory: {}", e)),
            output_path: None,
        };
    }

    let final_path = tracks_dir.join(format!("{}.wav", sanitized_name));

    // Copy from temp to collection (fs::rename might fail across filesystems)
    // Temp file cleanup is handled by _temp_guard on drop
    if let Err(e) = fs::copy(&temp_path, &final_path) {
        return TrackImportResult {
            base_name,
            success: false,
            error: Some(format!("Failed to copy to collection: {}", e)),
            output_path: None,
        };
    }

    log::info!(
        "process_single_track: '{}' exported to {:?}",
        base_name,
        final_path
    );

    // Insert track into the shared database service using new Track API
    let mut track = Track::new(final_path.clone(), base_name.clone());
    track.artist = artist;
    track.bpm = Some(analysis.bpm);
    track.original_bpm = Some(analysis.original_bpm);
    track.key = Some(analysis.key.clone());
    track.duration_seconds = (duration_samples as f64) / (SAMPLE_RATE as f64);
    track.lufs = analysis.lufs;
    track.first_beat_sample = first_beat as i64;

    match config.db_service.save_track(&track) {
        Ok(track_id) => {
            log::info!(
                "process_single_track: '{}' inserted into database (id={})",
                base_name, track_id
            );

            // Store audio features for similarity search
            if let Some(ref features) = analysis.audio_features {
                if let Err(e) = config.db_service.store_audio_features(track_id, features) {
                    log::warn!(
                        "process_single_track: Failed to store audio features for '{}': {}",
                        base_name, e
                    );
                } else {
                    log::info!(
                        "process_single_track: '{}' audio features stored for similarity search",
                        base_name
                    );
                }
            }
        }
        Err(e) => {
            log::warn!(
                "process_single_track: Failed to insert '{}' into database: {}",
                base_name, e
            );
        }
    }

    TrackImportResult {
        base_name,
        success: true,
        error: None,
        output_path: Some(final_path),
    }
}

/// Run the batch import process
///
/// This is meant to be called from a delegation thread.
/// It uses rayon's thread pool for parallel processing.
///
/// # Arguments
///
/// * `groups` - Stem groups to import (only complete groups will be processed)
/// * `config` - Import configuration
/// * `progress_tx` - Channel to send progress updates
/// * `cancel_flag` - Atomic flag to signal cancellation
pub fn run_batch_import(
    groups: Vec<StemGroup>,
    config: ImportConfig,
    progress_tx: Sender<ImportProgress>,
    cancel_flag: Arc<AtomicBool>,
) {
    let start_time = Instant::now();

    // Filter to complete groups only
    let complete_groups: Vec<_> = groups.into_iter().filter(|g| g.is_complete()).collect();
    let total = complete_groups.len();

    log::info!(
        "run_batch_import: Starting import of {} tracks",
        total
    );

    // Send start notification
    let _ = progress_tx.send(ImportProgress::Started { total });

    // Check for early cancellation
    if cancel_flag.load(Ordering::Relaxed) {
        log::info!("run_batch_import: Cancelled before processing");
        let _ = progress_tx.send(ImportProgress::AllComplete {
            results: Vec::new(),
        });
        return;
    }

    // Configure rayon thread pool with user-specified parallelism
    let num_workers = config.parallel_processes.clamp(1, 16) as usize;
    log::info!("run_batch_import: Using {} parallel workers", num_workers);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_workers)
        .build()
        .expect("Failed to create thread pool");

    // Process groups in parallel, collecting results
    let results: Vec<TrackImportResult> = pool.install(|| {
        complete_groups
            .par_iter()
            .enumerate()
            .map(|(index, group)| {
                // Check for cancellation
                if cancel_flag.load(Ordering::Relaxed) {
                    return TrackImportResult {
                        base_name: group.base_name.clone(),
                        success: false,
                        error: Some("Cancelled".to_string()),
                        output_path: None,
                    };
                }

                // Send track started notification
                let _ = progress_tx.send(ImportProgress::TrackStarted {
                    base_name: group.base_name.clone(),
                    index,
                    total,
                });

                // Process the track
                let result = process_single_track(group, &config);

                // Delete source files on success
                if result.success {
                    for path in group.all_paths() {
                        if let Err(e) = fs::remove_file(path) {
                            log::warn!(
                                "run_batch_import: Failed to delete source file {:?}: {}",
                                path,
                                e
                            );
                        } else {
                            log::debug!("run_batch_import: Deleted source file {:?}", path);
                        }
                    }
                }

                // Send track completed notification
                let _ = progress_tx.send(ImportProgress::TrackCompleted(result.clone()));

                result
            })
            .collect()
    });

    let duration = start_time.elapsed();
    let success_count = results.iter().filter(|r| r.success).count();
    let fail_count = results.len() - success_count;

    log::info!(
        "run_batch_import: Complete in {:.1}s - {} succeeded, {} failed",
        duration.as_secs_f64(),
        success_count,
        fail_count
    );

    // Send completion notification
    let _ = progress_tx.send(ImportProgress::AllComplete { results });
}

/// Sanitize a filename by removing invalid characters
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect()
}

/// Get the default import folder path
/// Uses dirs::home_dir() for cross-platform compatibility (Windows, macOS, Linux)
pub fn default_import_folder() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
        .join("import")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stem_filename_vocals() {
        let result = parse_stem_filename("Artist - Track_(Vocals).wav");
        assert_eq!(result, Some(("Artist - Track".to_string(), StemType::Vocals)));
    }

    #[test]
    fn test_parse_stem_filename_drums() {
        let result = parse_stem_filename("1_01 Black Sun Empire - Feed the Machine_(Drums).wav");
        assert_eq!(
            result,
            Some((
                "1_01 Black Sun Empire - Feed the Machine".to_string(),
                StemType::Drums
            ))
        );
    }

    #[test]
    fn test_parse_stem_filename_bass() {
        let result = parse_stem_filename("Test_(Bass).wav");
        assert_eq!(result, Some(("Test".to_string(), StemType::Bass)));
    }

    #[test]
    fn test_parse_stem_filename_other() {
        let result = parse_stem_filename("Test_(Other).wav");
        assert_eq!(result, Some(("Test".to_string(), StemType::Other)));
    }

    #[test]
    fn test_parse_stem_filename_instrumental() {
        let result = parse_stem_filename("Test_(Instrumental).wav");
        assert_eq!(result, Some(("Test".to_string(), StemType::Other)));
    }

    #[test]
    fn test_parse_stem_filename_case_insensitive() {
        let result = parse_stem_filename("Test_(VOCALS).wav");
        assert_eq!(result, Some(("Test".to_string(), StemType::Vocals)));

        let result = parse_stem_filename("Test_(vocals).WAV");
        assert_eq!(result, Some(("Test".to_string(), StemType::Vocals)));
    }

    #[test]
    fn test_parse_stem_filename_invalid() {
        assert_eq!(parse_stem_filename("Test.wav"), None);
        assert_eq!(parse_stem_filename("Test_(Unknown).wav"), None);
        assert_eq!(parse_stem_filename("Test_Vocals.wav"), None);
        assert_eq!(parse_stem_filename("Test.mp3"), None);
    }

    #[test]
    fn test_stem_group_complete() {
        let mut group = StemGroup::new("Test".to_string());
        assert!(!group.is_complete());
        assert_eq!(group.stem_count(), 0);

        group.set_stem(StemType::Vocals, PathBuf::from("v.wav"));
        assert_eq!(group.stem_count(), 1);

        group.set_stem(StemType::Drums, PathBuf::from("d.wav"));
        group.set_stem(StemType::Bass, PathBuf::from("b.wav"));
        assert_eq!(group.stem_count(), 3);
        assert!(!group.is_complete());

        group.set_stem(StemType::Other, PathBuf::from("o.wav"));
        assert_eq!(group.stem_count(), 4);
        assert!(group.is_complete());
    }
}
