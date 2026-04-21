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

use crate::analysis::{analyze_audio, fit_bpm_to_range, generate_beat_grid, AnalysisResult};
use crate::config::{BeatDetectionBackend, BpmConfig, BpmSource, LoudnessConfig};
use crate::export::export_stem_file;
use crate::import::StemImporter;
use crate::ml_analysis::{self, BeatThisAnalyzer, MlAnalysisResult, MlAnalyzer};
use crate::separation::{SeparationConfig, SeparationService};
use anyhow::{Context, Result};
use mesh_core::db::{DatabaseService, MlAnalysisData, Track};
use mesh_core::types::SAMPLE_RATE;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
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
/// # Arguments
/// * `samples` - Full mix audio samples (for key/LUFS/features)
/// * `bpm_samples` - Optional separate audio for BPM analysis (drums-only when BpmSource::Drums)
/// * `bpm_config` - BPM detection configuration
fn analyze_in_subprocess(samples: Vec<f32>, bpm_samples: Option<Vec<f32>>, bpm_config: BpmConfig) -> Result<AnalysisResult> {
    use std::io::{Read, Write};

    // Generate unique temp file paths
    let uid = std::process::id() ^ (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u32);
    let temp_path = std::env::temp_dir().join(format!("mesh_audio_{}.bin", uid));
    let bpm_temp_path = std::env::temp_dir().join(format!("mesh_audio_{}_bpm.bin", uid));

    // RAII guard ensures cleanup on any exit path (early return, panic, or normal)
    let _temp_guard = TempFileGuard::new(temp_path.clone());
    let _bpm_temp_guard = TempFileGuard::new(bpm_temp_path.clone());

    // Helper to write f32 samples to a temp file
    let write_samples = |path: &std::path::Path, data: &[f32]| -> Result<()> {
        let mut file = std::fs::File::create(path)
            .with_context(|| format!("Failed to create temp file: {:?}", path))?;
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                data.as_ptr() as *const u8,
                data.len() * std::mem::size_of::<f32>(),
            )
        };
        file.write_all(bytes)
            .with_context(|| "Failed to write samples to temp file")?;
        Ok(())
    };

    // Write main samples
    write_samples(&temp_path, &samples)?;
    let sample_count = samples.len();

    // Write BPM samples if separate
    let bpm_sample_count = bpm_samples.as_ref().map(|s| s.len());
    if let Some(ref bpm) = bpm_samples {
        write_samples(&bpm_temp_path, bpm)?;
    }

    drop(samples); // Free memory before spawning subprocess
    drop(bpm_samples);

    // Spawn subprocess with temp file paths
    let temp_path_str = temp_path.to_string_lossy().to_string();
    let bpm_temp_path_str = bpm_temp_path.to_string_lossy().to_string();
    let handle = procspawn::spawn(
        (temp_path_str.clone(), sample_count, bpm_temp_path_str.clone(), bpm_sample_count, bpm_config),
        |(path, count, bpm_path, bpm_count, config)| {
            // Helper to read f32 samples from a temp file
            let read_samples = |path: &str, count: usize| -> std::result::Result<Vec<f32>, String> {
                let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                let mut bytes = vec![0u8; count * std::mem::size_of::<f32>()];
                file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                Ok(bytes
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect())
            };

            let samples = read_samples(&path, count)?;
            let bpm_samples = match bpm_count {
                Some(c) => Some(read_samples(&bpm_path, c)?),
                None => None,
            };

            // Run analysis in isolated process
            analyze_audio(&samples, bpm_samples.as_deref(), &config).map_err(|e| e.to_string())
        },
    );

    // Wait for result - temp file cleanup is handled by guards on drop
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
    /// Original source file for embedded tag reading (None for pre-separated stems)
    pub source_path: Option<PathBuf>,
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
            source_path: None,
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
    /// Separating a mixed audio file into stems
    Separating {
        base_name: String,
        progress: f32,
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
    /// Stem separation configuration (for mixed audio files)
    pub separation_config: Option<SeparationConfig>,
}

/// A mixed audio file to be separated into stems
#[derive(Debug, Clone)]
pub struct MixedAudioFile {
    /// Path to the audio file
    pub path: PathBuf,
    /// Base name extracted from filename (without extension)
    pub base_name: String,
}

/// Scan import folder for mixed audio files (MP3, FLAC, WAV without stem suffix)
///
/// These files will be separated into stems before import.
pub fn scan_mixed_audio_files(import_folder: &Path) -> Result<Vec<MixedAudioFile>> {
    log::info!("scan_mixed_audio_files: Scanning {:?}", import_folder);

    if !import_folder.exists() {
        fs::create_dir_all(import_folder)
            .with_context(|| format!("Failed to create import folder: {:?}", import_folder))?;
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    let supported_extensions = ["mp3", "flac", "wav", "ogg", "m4a", "aac"];

    let entries = fs::read_dir(import_folder)
        .with_context(|| format!("Failed to read import folder: {:?}", import_folder))?;

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip directories
        if path.is_dir() {
            continue;
        }

        // Check extension
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());

        if !ext.as_ref().map_or(false, |e| supported_extensions.contains(&e.as_str())) {
            continue;
        }

        // Get filename
        let filename = match path.file_stem().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        // Skip files that look like stems (have _(Vocals), _(Drums), etc.)
        if filename.contains("_(") && filename.contains(")") {
            // Check if it's a stem file pattern
            let lower = filename.to_lowercase();
            if lower.contains("_(vocals)")
                || lower.contains("_(drums)")
                || lower.contains("_(bass)")
                || lower.contains("_(other)")
                || lower.contains("_(instrumental)")
            {
                continue;
            }
        }

        files.push(MixedAudioFile {
            path: path.clone(),
            base_name: filename.to_string(),
        });
    }

    files.sort_by(|a, b| a.base_name.cmp(&b.base_name));

    log::info!(
        "scan_mixed_audio_files: Found {} mixed audio files",
        files.len()
    );

    Ok(files)
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

/// Compute per-stem RMS energy densities as fractions of total RMS.
///
/// Returns `(vocal, drums, bass, other)` where each value is in [0, 1] and
/// the four values sum to 1.0. Captures the stem energy balance of a track
/// for complement scoring in suggestions.
pub fn compute_stem_energy_ratios(
    buffers: &mesh_core::audio_file::StemBuffers,
) -> (f32, f32, f32, f32) {
    fn rms(buf: &mesh_core::types::StereoBuffer) -> f32 {
        let s = buf.as_slice();
        if s.is_empty() {
            return 0.0;
        }
        let sum: f32 = s.iter().map(|s| s.left * s.left + s.right * s.right).sum();
        (sum / (2.0 * s.len() as f32)).sqrt()
    }
    let v = rms(&buffers.vocals);
    let d = rms(&buffers.drums);
    let b = rms(&buffers.bass);
    let o = rms(&buffers.other);
    let total = v + d + b + o;
    if total < 1e-9 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (v / total, d / total, b / total, o / total)
}

/// Process a single track group: load stems, analyze, export
///
/// This is run by worker threads. When `ml_analyzer` is provided,
/// also runs ML analysis (genre, arousal/valence, mood) and auto-tagging.
/// When `beat_this` is provided and backend is Advanced, uses Beat This! for BPM/beats.
fn process_single_track(
    group: &StemGroup,
    config: &ImportConfig,
    ml_analyzer: Option<&Arc<Mutex<MlAnalyzer>>>,
    beat_this: Option<&Arc<Mutex<BeatThisAnalyzer>>>,
    known_artists: &std::collections::HashSet<String>,
) -> TrackImportResult {
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

    // Check if this track already exists in the collection (skip duplicates)
    let sanitized_for_check = sanitize_filename(&base_name);
    let final_check_path = config.collection_path.join("tracks").join(format!("{}.flac", sanitized_for_check));
    if final_check_path.exists() {
        log::info!(
            "process_single_track: '{}' already exists in collection, skipping duplicate import",
            base_name
        );
        return TrackImportResult {
            base_name,
            success: false,
            error: Some("Track already exists in collection".to_string()),
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

    // Compute stem energy ratios while buffers are in scope (before any move/drop)
    let (vocal_density, drums_density, bass_density, other_density) = compute_stem_energy_ratios(&buffers);

    // Create full mix for subprocess analysis (key/LUFS always need all audio content)
    let mono_samples = match importer.get_mono_sum() {
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

    // Create BPM-specific mono based on configured source (drums-only or full mix)
    let bpm_mono = match config.bpm_config.source {
        BpmSource::Drums => {
            log::info!("process_single_track: Using drums-only for BPM analysis");
            match importer.get_drums_mono() {
                Ok(s) => Some(s),
                Err(e) => {
                    log::warn!("process_single_track: Failed to get drums mono, falling back to full mix: {}", e);
                    None
                }
            }
        }
        BpmSource::FullMix => None, // Will use mono_samples directly
    };

    // Resample mono audio to 44100 Hz if source is at a different rate.
    // Essentia's algorithms (BPM, key, onset) internally assume 44100 Hz input.
    let (mono_samples, bpm_mono) = if source_sample_rate != 44100 {
        log::info!(
            "process_single_track: Resampling mono analysis audio {} Hz → 44100 Hz ({} samples)",
            source_sample_rate,
            mono_samples.len()
        );
        let resampled = match mesh_core::audio_file::resample_mono_audio(&mono_samples, source_sample_rate, 44100) {
            Ok(r) => r,
            Err(e) => {
                return TrackImportResult {
                    base_name,
                    success: false,
                    error: Some(format!("Failed to resample for analysis: {}", e)),
                    output_path: None,
                };
            }
        };
        let bpm_resampled = match bpm_mono {
            Some(bpm) => match mesh_core::audio_file::resample_mono_audio(&bpm, source_sample_rate, 44100) {
                Ok(r) => Some(r),
                Err(e) => {
                    log::warn!("process_single_track: Failed to resample BPM mono, using full mix: {}", e);
                    None
                }
            },
            None => None,
        };
        log::info!(
            "process_single_track: Resampled to {} samples at 44100 Hz",
            resampled.len()
        );
        (resampled, bpm_resampled)
    } else {
        (mono_samples, bpm_mono)
    };

    // Select BPM audio: drums-only if available, otherwise full mix
    let bpm_audio = bpm_mono.as_deref().unwrap_or(&mono_samples);

    // Run Beat This! BEFORE subprocess if using Advanced backend
    // (ort is thread-safe, mel spectrogram borrows bpm_audio, then mono_samples is moved to subprocess)
    let beat_this_result = if config.bpm_config.backend == BeatDetectionBackend::Advanced {
        if let Some(bt_arc) = beat_this {
            log::info!("process_single_track: Computing Beat This! mel spectrogram for '{}'", base_name);
            match ml_analysis::preprocessing::compute_mel_spectrogram_beat_this(bpm_audio, 44100.0) {
                Ok(mel) => {
                    match bt_arc.lock() {
                        Ok(mut analyzer) => {
                            match analyzer.detect_beats(&mel) {
                                Ok(result) => {
                                    log::info!(
                                        "process_single_track: Beat This! detected {} beats, BPM={:.1}",
                                        result.beat_times.len(),
                                        result.bpm
                                    );
                                    Some(result)
                                }
                                Err(e) => {
                                    log::warn!("process_single_track: Beat This! inference failed, falling back to Essentia: {}", e);
                                    None
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("process_single_track: BeatThisAnalyzer lock poisoned: {}", e);
                            None
                        }
                    }
                }
                Err(e) => {
                    log::warn!("process_single_track: Beat This! mel spectrogram failed: {}", e);
                    None
                }
            }
        } else {
            log::warn!("process_single_track: Advanced backend selected but Beat This! model not available");
            None
        }
    } else {
        None
    };

    // Analyze audio in isolated subprocess (Essentia for key, LUFS, features;
    // BPM only if Simple backend or if Beat This! failed above)
    let mut analysis = match analyze_in_subprocess(mono_samples, bpm_mono, config.bpm_config.clone()) {
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

    // Override BPM/beat_grid with Beat This! results when available
    if let Some(ref bt_result) = beat_this_result {
        // Round BPM for display/engine use; keep raw value as original for reference
        analysis.bpm = fit_bpm_to_range(bt_result.bpm, config.bpm_config.min_tempo, config.bpm_config.max_tempo);
        analysis.original_bpm = bt_result.bpm;
        analysis.confidence = bt_result.confidence;

        // Convert Beat This! beat times (seconds) to a fixed beat grid at system sample rate
        // Use the first detected beat as the phase anchor, BPM from median IBI
        let duration_samples = (analysis.beat_grid.last().copied().unwrap_or(0) as f64 * 1.1) as u64;
        let duration_from_source = if !bt_result.beat_times.is_empty() {
            // Duration in system samples: last beat time * sample_rate + some padding
            let last_beat = bt_result.beat_times.last().unwrap_or(&0.0);
            ((last_beat + 2.0) * SAMPLE_RATE as f64) as u64
        } else {
            duration_samples
        };

        // Build fixed grid from Beat This! detected beats
        // Convert beat times to tick positions for generate_beat_grid (expects f64 seconds)
        analysis.beat_grid = generate_beat_grid(
            bt_result.bpm,
            &bt_result.beat_times,
            &[], // No raw audio needed — we have direct beat positions
            duration_from_source,
            None, // No ODF — Beat This! provides better phase than onset search
        );

        log::info!(
            "process_single_track: Overrode with Beat This! — BPM={:.1}, {} grid beats",
            analysis.bpm,
            analysis.beat_grid.len()
        );
    }

    log::info!(
        "process_single_track: '{}' analyzed: BPM={:.1}, Key={}",
        base_name,
        analysis.bpm,
        analysis.key
    );

    // Extract artist/title from embedded tags and filename patterns
    let resolved = crate::metadata::resolve_metadata(
        group.source_path.as_deref(),
        &base_name,
        known_artists,
    );

    // Calculate resampling ratio: target samples will differ from source if rates mismatch
    // E.g., 44100 Hz input → 48000 Hz output means samples scale by 48000/44100 = 1.088
    let resample_ratio = SAMPLE_RATE as f64 / source_sample_rate as f64;

    // Get duration in samples for beat grid generation (at TARGET sample rate after resampling)
    let source_duration_samples = buffers.len() as u64;
    let duration_samples = (source_duration_samples as f64 * resample_ratio) as u64;

    // Get first beat position from analysis — already at system sample rate (48kHz)
    // because generate_beat_grid() outputs positions at SAMPLE_RATE, and we now pass
    // correctly scaled duration to it. No resample_ratio needed here.
    let first_beat = analysis.beat_grid.first().copied().unwrap_or(0);

    log::info!(
        "process_single_track: Resampling {} Hz → {} Hz (ratio: {:.4}), duration: {} → {} samples, first_beat: {}",
        source_sample_rate, SAMPLE_RATE, resample_ratio, source_duration_samples, duration_samples, first_beat
    );

    // Export to temp file first (export handles resampling from source_sample_rate to SAMPLE_RATE)
    let temp_dir = std::env::temp_dir();
    let sanitized_name = sanitize_filename(&base_name);
    let temp_path = temp_dir.join(format!("{}.flac", sanitized_name));

    // RAII guard ensures temp file cleanup on any exit path (early return, panic, or normal)
    let _temp_guard = TempFileGuard::new(temp_path.clone());

    // Export audio only - all metadata (BPM, key, cues, loops) is stored in the database
    if let Err(e) = export_stem_file(&temp_path, &buffers, source_sample_rate) {
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

    let final_path = tracks_dir.join(format!("{}.flac", sanitized_name));

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

    // ── ML Analysis (mel spectrogram → genre/arousal/mood/voice) ──
    let ml_result: Option<MlAnalysisResult> = if ml_analyzer.is_some() {
        // Compute mel spectrogram from full mix mono (pure Rust DSP)
        let mono_for_mel = match importer.get_mono_sum() {
            Ok(m) => m,
            Err(_) => Vec::new(),
        };
        let mel = if !mono_for_mel.is_empty() {
            match ml_analysis::preprocessing::compute_mel_spectrogram(&mono_for_mel, SAMPLE_RATE as f32) {
                Ok(mel) => {
                    log::info!(
                        "process_single_track: '{}' mel spectrogram: {} frames × {} bands",
                        base_name, mel.frames.len(), mel.n_bands
                    );
                    Some(mel)
                }
                Err(e) => {
                    log::warn!("process_single_track: '{}' mel spectrogram failed: {}", base_name, e);
                    None
                }
            }
        } else {
            None
        };

        // Run EffNet + classification heads (genre, mood, voice/instrumental)
        if let (Some(analyzer_arc), Some(mel)) = (ml_analyzer, mel) {
            match analyzer_arc.lock() {
                Ok(mut analyzer) => {
                    match analyzer.analyze(&mel) {
                        Ok(result) => {
                            log::info!(
                                "process_single_track: '{}' ML analysis complete — genre={:?}, arousal={:?}, vocal={:.3}",
                                base_name, result.data.top_genre, result.data.arousal, result.data.vocal_presence
                            );
                            Some(result)
                        }
                        Err(e) => {
                            log::warn!("process_single_track: '{}' ML inference failed: {}", base_name, e);
                            Some(MlAnalysisResult {
                                data: MlAnalysisData {
                                    vocal_presence: 0.0,
                                    arousal: None,
                                    valence: None,
                                    top_genre: None,
                                    genre_scores: Vec::new(),
                                    mood_themes: None,
                                    binary_moods: None,
                                    danceability: None,
                                    approachability: None,
                                    reverb: None,
                                    timbre: None,
                                    tonal: None,
                                    mood_acoustic: None,
                                    mood_electronic: None,
                                },
                                embedding: Vec::new(),
                            })
                        }
                    }
                }
                Err(e) => {
                    log::error!("process_single_track: MlAnalyzer lock poisoned: {}", e);
                    None
                }
            }
        } else {
            // No mel spectrogram available
            Some(MlAnalysisResult {
                data: MlAnalysisData {
                    vocal_presence: 0.0,
                    arousal: None,
                    valence: None,
                    top_genre: None,
                    genre_scores: Vec::new(),
                    mood_themes: None,
                    binary_moods: None,
                    danceability: None,
                    approachability: None,
                    reverb: None,
                    timbre: None,
                    tonal: None,
                    mood_acoustic: None,
                    mood_electronic: None,
                },
                embedding: Vec::new(),
            })
        }
    } else {
        None
    };

    // Insert track into the shared database service using new Track API
    let mut track = Track::new(final_path.clone(), resolved.title.clone());
    track.original_name = base_name.clone();
    track.artist = resolved.artist;
    track.bpm = Some(analysis.bpm);
    track.original_bpm = Some(analysis.original_bpm);
    track.key = Some(analysis.key.clone());
    track.duration_seconds = (duration_samples as f64) / (SAMPLE_RATE as f64);
    track.lufs = analysis.lufs;
    track.integrated_lufs = analysis.integrated_lufs;
    track.first_beat_sample = first_beat as i64;

    match config.db_service.save_track(&track) {
        Ok(track_id) => {
            log::info!(
                "process_single_track: '{}' inserted into database (id={})",
                base_name, track_id
            );

            // Compute and store intensity components (multi-frame, pure Rust — no subprocess needed)
            // Use the analysis audio features for spectral_centroid and energy_variance (full-track)
            if let Some(ref features) = analysis.audio_features {
                // We need the mono samples again — reload from the exported FLAC
                // Actually, the features already contain spectral_centroid and energy_variance
                // For the new multi-frame features (flux, flatness, etc.), we store what we can
                // from the existing features and defer the rest to reanalysis
                let mut ic = mesh_core::db::IntensityComponents {
                    spectral_centroid: features.spectral_centroid,
                    energy_variance: features.energy_variance,
                    flatness: features.mfcc_flatness, // single-frame for now, improved during reanalysis
                    dissonance: features.dissonance.unwrap_or(0.0),
                    harmonic_complexity: features.harmonic_complexity,
                    spectral_rolloff: features.spectral_rolloff,
                    spectral_flux: 0.0,  // requires multi-frame — computed during reanalysis
                    crest_factor: 0.0,   // requires full track audio — computed during reanalysis
                };
                if let Err(e) = config.db_service.store_intensity_components(track_id, &ic) {
                    log::warn!("process_single_track: Failed to store intensity components for '{}': {}", base_name, e);
                }
            }

            // Store ML analysis results and auto-tag
            if let Some(ref ml) = ml_result {
                if let Err(e) = config.db_service.store_ml_analysis(track_id, &ml.data) {
                    log::warn!(
                        "process_single_track: Failed to store ML analysis for '{}': {}",
                        base_name, e
                    );
                } else {
                    log::info!(
                        "process_single_track: '{}' ML analysis stored",
                        base_name
                    );
                    // Auto-tag from ML results
                    auto_tag_from_ml(track_id, &ml.data, &config.db_service);
                }

                // Persist EffNet embedding for 1280-dim HNSW similarity search
                if ml.embedding.len() == 1280 {
                    if let Err(e) = config.db_service.store_ml_embedding(track_id, &ml.embedding) {
                        log::warn!("process_single_track: Failed to store ML embedding for '{}': {}", base_name, e);
                    }
                }

                // Persist stem energy densities for complement scoring in suggestions
                if let Err(e) = config.db_service.store_stem_energy(track_id, vocal_density, drums_density, bass_density, other_density) {
                    log::warn!("process_single_track: Failed to store stem energy for '{}': {}", base_name, e);
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

/// ML-generated tag colors (used for clearing before re-tagging)
pub const ML_TAG_COLORS: &[&str] = &[
    "#2563eb", // genre super (dark blue)
    "#60a5fa", // genre sub (light blue)
    "#3b82f6", // genre plain (blue)
    "#8b5cf6", // Jamendo mood (purple)
    "#b8bb26", // vocal (gruvbox green — matches vocal stem color)
    "#2d8a4e", // vocal legacy (green — kept for clearing old tags)
    "#ec4899", // binary mood (pink)
    "#0d9488", // audio characteristics (teal)
];

/// Remove all ML-generated tags from a track before re-tagging.
///
/// Identifies ML tags by their color codes (genre, mood, vocal/instrumental).
pub fn clear_ml_tags(track_id: i64, db: &DatabaseService) {
    match db.get_tags(track_id) {
        Ok(tags) => {
            for (label, color) in tags {
                if let Some(ref c) = color {
                    if ML_TAG_COLORS.contains(&c.as_str()) {
                        let _ = db.remove_tag(track_id, &label);
                    }
                }
            }
        }
        Err(e) => {
            log::warn!("clear_ml_tags: Failed to get tags for track {}: {}", track_id, e);
        }
    }
}

/// Auto-generate tags from ML analysis results
///
/// Creates colored tags for genre, mood, and vocal/instrumental classification.
/// Tags are stored via the database tag system.
pub fn auto_tag_from_ml(track_id: i64, ml: &MlAnalysisData, db: &DatabaseService) {
    // Genre tags — split Discogs "SuperGenre---SubGenre" into separate tags
    // Super-genres (dark blue), sub-genres (blue) — deduplicate super-genres
    let mut seen_super = std::collections::HashSet::new();
    for (label, score) in ml.genre_scores.iter().take(3) {
        if *score < 0.15 {
            continue;
        }
        if let Some((super_genre, sub_genre)) = label.split_once("---") {
            // Add super-genre once (darker blue)
            if seen_super.insert(super_genre.to_string()) {
                if let Err(e) = db.add_tag(track_id, super_genre, Some("#2563eb")) {
                    log::warn!("auto_tag_from_ml: Failed to add genre tag '{}': {}", super_genre, e);
                }
            }
            // Consolidate DnB-family sub-genres into a single "DnB" tag,
            // and skip "Instrumental" (redundant with ML voice detection)
            let sub_tag = match sub_genre {
                "Drum n Bass" | "Breakcore" | "Jungle" => "DnB",
                "Instrumental" => continue,
                other => other,
            };
            // Add sub-genre (lighter blue)
            if let Err(e) = db.add_tag(track_id, sub_tag, Some("#60a5fa")) {
                log::warn!("auto_tag_from_ml: Failed to add genre tag '{}': {}", sub_tag, e);
            }
        } else {
            // No separator — use as-is
            if let Err(e) = db.add_tag(track_id, label, Some("#3b82f6")) {
                log::warn!("auto_tag_from_ml: Failed to add genre tag '{}': {}", label, e);
            }
        }
    }

    // Jamendo mood/theme tags (purple) — top 3 above 0.2 confidence
    if let Some(ref moods) = ml.mood_themes {
        for (label, score) in moods.iter().take(3) {
            if *score >= 0.2 {
                if let Err(e) = db.add_tag(track_id, label, Some("#8b5cf6")) {
                    log::warn!("auto_tag_from_ml: Failed to add mood tag '{}': {}", label, e);
                }
            }
        }
    }

    // Binary mood tags (pink) — above 0.5 threshold
    if let Some(ref binary_moods) = ml.binary_moods {
        for (label, prob) in binary_moods {
            if *prob >= 0.5 {
                if let Err(e) = db.add_tag(track_id, label, Some("#ec4899")) {
                    log::warn!("auto_tag_from_ml: Failed to add binary mood tag '{}': {}", label, e);
                }
            }
        }
    }

    // Vocal tag (gruvbox green — matches vocal stem color) — from ML voice/instrumental classifier
    if ml.vocal_presence >= 0.5 {
        let _ = db.add_tag(track_id, "Vocal", Some("#b8bb26"));
    }

    // Audio characteristic tags (teal) — from binary classifiers
    // Timbre: Bright or Dark (mutually exclusive)
    if let Some(bright_prob) = ml.timbre {
        if bright_prob >= 0.5 {
            let _ = db.add_tag(track_id, "Bright", Some("#0d9488"));
        } else {
            let _ = db.add_tag(track_id, "Dark", Some("#0d9488"));
        }
    }

    // Tonal/Atonal (mutually exclusive)
    if let Some(tonal_prob) = ml.tonal {
        if tonal_prob >= 0.5 {
            let _ = db.add_tag(track_id, "Tonal", Some("#0d9488"));
        } else {
            let _ = db.add_tag(track_id, "Atonal", Some("#0d9488"));
        }
    }

    // Acoustic (positive class only — no tag for non-acoustic)
    if let Some(acoustic_prob) = ml.mood_acoustic {
        if acoustic_prob >= 0.5 {
            let _ = db.add_tag(track_id, "Acoustic", Some("#0d9488"));
        }
    }

    // Electronic (positive class only — no tag for non-electronic)
    if let Some(electronic_prob) = ml.mood_electronic {
        if electronic_prob >= 0.5 {
            let _ = db.add_tag(track_id, "Electronic", Some("#0d9488"));
        }
    }
}

/// Process a mixed audio file: separate into stems, then import
///
/// This function:
/// 1. Runs stem separation on the mixed audio file
/// 2. Writes stems to temp WAV files
/// 3. Uses the existing import pipeline via process_single_track
/// 4. Cleans up temp stem files
fn process_mixed_track(
    file: &MixedAudioFile,
    config: &ImportConfig,
    progress_tx: &Sender<ImportProgress>,
    ml_analyzer: Option<&Arc<Mutex<MlAnalyzer>>>,
    beat_this: Option<&Arc<Mutex<BeatThisAnalyzer>>>,
    known_artists: &std::collections::HashSet<String>,
) -> TrackImportResult {
    let base_name = file.base_name.clone();
    log::info!("process_mixed_track: Separating '{}'", base_name);

    // Check if this track already exists in the collection (skip duplicates)
    let sanitized_check = sanitize_filename(&base_name);
    let final_check_path = config.collection_path.join("tracks").join(format!("{}.flac", sanitized_check));
    if final_check_path.exists() {
        log::info!(
            "process_mixed_track: '{}' already exists in collection, skipping duplicate import",
            base_name
        );
        return TrackImportResult {
            base_name,
            success: false,
            error: Some("Track already exists in collection".to_string()),
            output_path: None,
        };
    }

    // Get separation config
    let sep_config = match &config.separation_config {
        Some(cfg) => cfg.clone(),
        None => {
            return TrackImportResult {
                base_name,
                success: false,
                error: Some("Separation not configured".to_string()),
                output_path: None,
            };
        }
    };

    // Create separation service
    let service = match SeparationService::with_config(sep_config) {
        Ok(s) => s,
        Err(e) => {
            return TrackImportResult {
                base_name,
                success: false,
                error: Some(format!("Failed to create separation service: {}", e)),
                output_path: None,
            };
        }
    };

    // Create progress callback that sends updates
    let base_name_clone = base_name.clone();
    let progress_tx_clone = progress_tx.clone();
    let progress_cb = std::sync::Arc::new(move |progress: crate::separation::SeparationProgress| {
        let _ = progress_tx_clone.send(ImportProgress::Separating {
            base_name: base_name_clone.clone(),
            progress: progress.progress,
        });
    });

    // Run separation
    let stems = match service.separate(&file.path, Some(progress_cb)) {
        Ok(s) => s,
        Err(e) => {
            return TrackImportResult {
                base_name,
                success: false,
                error: Some(format!("Separation failed: {}", e)),
                output_path: None,
            };
        }
    };

    log::info!(
        "process_mixed_track: '{}' separated, {} samples per stem",
        base_name,
        stems.samples_per_channel()
    );

    // Write stems to temp directory
    let temp_dir = std::env::temp_dir().join("mesh-separation");
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        return TrackImportResult {
            base_name,
            success: false,
            error: Some(format!("Failed to create temp directory: {}", e)),
            output_path: None,
        };
    }

    let sanitized = sanitize_filename(&base_name);
    let (vocals_path, drums_path, bass_path, other_path) =
        match stems.write_to_wav_files(&temp_dir, &sanitized) {
            Ok(paths) => paths,
            Err(e) => {
                return TrackImportResult {
                    base_name,
                    success: false,
                    error: Some(format!("Failed to write temp stems: {}", e)),
                    output_path: None,
                };
            }
        };

    // Create RAII guards for cleanup
    let _vocals_guard = TempFileGuard::new(vocals_path.clone());
    let _drums_guard = TempFileGuard::new(drums_path.clone());
    let _bass_guard = TempFileGuard::new(bass_path.clone());
    let _other_guard = TempFileGuard::new(other_path.clone());

    // Create a StemGroup from the temp files
    let mut group = StemGroup::new(base_name.clone());
    group.source_path = Some(file.path.clone());
    group.vocals = Some(vocals_path);
    group.drums = Some(drums_path);
    group.bass = Some(bass_path);
    group.other = Some(other_path);

    // Process using existing pipeline
    // Note: We don't delete source files from group.all_paths() since they're temp files
    // that will be cleaned up by the guards
    process_single_track(&group, config, ml_analyzer, beat_this, known_artists)
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

    // Initialize ML analyzer if models are available
    let (ml_analyzer, beat_this_analyzer): (Option<Arc<Mutex<MlAnalyzer>>>, Option<Arc<Mutex<BeatThisAnalyzer>>>) = {
        match ml_analysis::models::MlModelManager::new() {
            Ok(mgr) => {
                // Ensure models are downloaded before starting import
                if let Err(e) = mgr.ensure_all_models() {
                    log::warn!("run_batch_import: Failed to download ML models: {}", e);
                }
                let model_dir = mgr.model_path(ml_analysis::MlModelType::EffNetEmbedding)
                    .parent().unwrap_or(std::path::Path::new(".")).to_path_buf();

                let ml = match MlAnalyzer::new(&model_dir) {
                    Ok(analyzer) => {
                        log::info!("run_batch_import: ML analyzer initialized");
                        Some(Arc::new(Mutex::new(analyzer)))
                    }
                    Err(e) => {
                        log::warn!("run_batch_import: ML models not available, skipping ML analysis: {}", e);
                        None
                    }
                };

                // Initialize Beat This! analyzer if using Advanced backend
                let bt = if config.bpm_config.backend == BeatDetectionBackend::Advanced {
                    if let Err(e) = mgr.ensure_beat_detection_models() {
                        log::warn!("run_batch_import: Failed to download Beat This! model: {}", e);
                    }
                    match BeatThisAnalyzer::new(&model_dir) {
                        Ok(analyzer) => {
                            log::info!("run_batch_import: Beat This! analyzer initialized");
                            Some(Arc::new(Mutex::new(analyzer)))
                        }
                        Err(e) => {
                            log::warn!("run_batch_import: Beat This! model not available, falling back to Essentia: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };

                (ml, bt)
            }
            Err(e) => {
                log::warn!("run_batch_import: Cannot determine model cache dir: {}", e);
                (None, None)
            }
        }
    };

    // Load known artists once for filename disambiguation across all tracks
    let known_artists = crate::metadata::get_known_artists(&config.db_service);

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
                let result = process_single_track(group, &config, ml_analyzer.as_ref(), beat_this_analyzer.as_ref(), &known_artists);

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

/// Run batch import for mixed audio files (with automatic stem separation)
///
/// Similar to `run_batch_import`, but processes mixed audio files by:
/// 1. Separating each file into stems using the configured backend
/// 2. Importing the separated stems using the standard pipeline
///
/// Requires `config.separation_config` to be set.
///
/// # Arguments
///
/// * `files` - Mixed audio files to import
/// * `config` - Import configuration (must include separation_config)
/// * `progress_tx` - Channel to send progress updates
/// * `cancel_flag` - Atomic flag to signal cancellation
pub fn run_batch_import_mixed(
    files: Vec<MixedAudioFile>,
    config: ImportConfig,
    progress_tx: Sender<ImportProgress>,
    cancel_flag: Arc<AtomicBool>,
) {
    let start_time = Instant::now();
    let total = files.len();

    log::info!(
        "run_batch_import_mixed: Starting import of {} mixed audio files",
        total
    );

    // Send start notification
    let _ = progress_tx.send(ImportProgress::Started { total });

    // Check for early cancellation
    if cancel_flag.load(Ordering::Relaxed) {
        log::info!("run_batch_import_mixed: Cancelled before processing");
        let _ = progress_tx.send(ImportProgress::AllComplete {
            results: Vec::new(),
        });
        return;
    }

    // Verify separation config is present
    if config.separation_config.is_none() {
        log::error!("run_batch_import_mixed: No separation config provided");
        let results: Vec<TrackImportResult> = files
            .iter()
            .map(|f| TrackImportResult {
                base_name: f.base_name.clone(),
                success: false,
                error: Some("Separation not configured".to_string()),
                output_path: None,
            })
            .collect();
        let _ = progress_tx.send(ImportProgress::AllComplete { results });
        return;
    }

    // Initialize ML analyzer and Beat This! if models are available
    let (ml_analyzer, beat_this_analyzer): (Option<Arc<Mutex<MlAnalyzer>>>, Option<Arc<Mutex<BeatThisAnalyzer>>>) = {
        match ml_analysis::models::MlModelManager::new() {
            Ok(mgr) => {
                if let Err(e) = mgr.ensure_all_models() {
                    log::warn!("run_batch_import_mixed: Failed to download ML models: {}", e);
                }
                let model_dir = mgr.model_path(ml_analysis::MlModelType::EffNetEmbedding)
                    .parent().unwrap_or(std::path::Path::new(".")).to_path_buf();

                let ml = match MlAnalyzer::new(&model_dir) {
                    Ok(analyzer) => {
                        log::info!("run_batch_import_mixed: ML analyzer initialized");
                        Some(Arc::new(Mutex::new(analyzer)))
                    }
                    Err(e) => {
                        log::warn!("run_batch_import_mixed: ML models not available, skipping ML analysis: {}", e);
                        None
                    }
                };

                let bt = if config.bpm_config.backend == BeatDetectionBackend::Advanced {
                    if let Err(e) = mgr.ensure_beat_detection_models() {
                        log::warn!("run_batch_import_mixed: Failed to download Beat This! model: {}", e);
                    }
                    match BeatThisAnalyzer::new(&model_dir) {
                        Ok(analyzer) => {
                            log::info!("run_batch_import_mixed: Beat This! analyzer initialized");
                            Some(Arc::new(Mutex::new(analyzer)))
                        }
                        Err(e) => {
                            log::warn!("run_batch_import_mixed: Beat This! not available: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };

                (ml, bt)
            }
            Err(e) => {
                log::warn!("run_batch_import_mixed: Cannot determine model cache dir: {}", e);
                (None, None)
            }
        }
    };

    // Load known artists once for filename disambiguation across all tracks
    let known_artists = crate::metadata::get_known_artists(&config.db_service);

    // Process files sequentially (separation is memory-intensive)
    // TODO: Consider parallel processing with memory limits
    let mut results = Vec::with_capacity(total);

    for (index, file) in files.iter().enumerate() {
        // Check for cancellation
        if cancel_flag.load(Ordering::Relaxed) {
            results.push(TrackImportResult {
                base_name: file.base_name.clone(),
                success: false,
                error: Some("Cancelled".to_string()),
                output_path: None,
            });
            break;
        }

        // Send track started notification
        let _ = progress_tx.send(ImportProgress::TrackStarted {
            base_name: file.base_name.clone(),
            index,
            total,
        });

        // Process the track (separation + import)
        let result = process_mixed_track(file, &config, &progress_tx, ml_analyzer.as_ref(), beat_this_analyzer.as_ref(), &known_artists);

        // Delete source file on success
        if result.success {
            if let Err(e) = fs::remove_file(&file.path) {
                log::warn!(
                    "run_batch_import_mixed: Failed to delete source file {:?}: {}",
                    file.path,
                    e
                );
            } else {
                log::debug!(
                    "run_batch_import_mixed: Deleted source file {:?}",
                    file.path
                );
            }
        }

        // Send track completed notification
        let _ = progress_tx.send(ImportProgress::TrackCompleted(result.clone()));

        results.push(result);
    }

    let duration = start_time.elapsed();
    let success_count = results.iter().filter(|r| r.success).count();
    let fail_count = results.len() - success_count;

    log::info!(
        "run_batch_import_mixed: Complete in {:.1}s - {} succeeded, {} failed",
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
