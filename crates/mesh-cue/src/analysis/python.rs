//! Python-based BPM detection algorithms
//!
//! This module provides BPM detection using external Python libraries
//! via subprocess execution. Currently supports:
//!
//! - **Madmom DBN**: Deep neural network beat tracker with Dynamic Bayesian
//!   Network inference, highly accurate for electronic music.
//!
//! ## Architecture
//!
//! Python algorithms use subprocess IPC rather than direct bindings:
//! 1. Audio samples are written to a temporary WAV file (hound crate)
//! 2. Python script is invoked with WAV path and parameters
//! 3. Results are returned as JSON to stdout
//! 4. Temporary file is cleaned up
//!
//! This approach provides isolation (Python GIL doesn't block Rust threads)
//! and simplifies dependency management (Python env handled by Nix).

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::algorithm::BpmAlgorithm;
use super::bpm::{BpmDetector, BpmResult};
use crate::config::BpmConfig;

/// Expected sample rate for audio analysis (Essentia standard)
const ANALYSIS_SAMPLE_RATE: u32 = 44100;

/// JSON output structure from Python beat detection scripts
#[derive(Debug, Deserialize)]
struct PythonBpmOutput {
    /// Beat positions in seconds
    beats: Vec<f64>,
    /// Calculated BPM
    bpm: f64,
    /// Detection confidence (0.0 - 1.0)
    confidence: f64,
    /// Error message if detection failed
    #[serde(default)]
    error: Option<String>,
}

/// Check if Python-based algorithms are available
///
/// Returns true if the MESH_PYTHON environment variable is set
/// and points to a valid Python executable.
pub fn python_algorithms_available() -> bool {
    if let Ok(python_path) = std::env::var("MESH_PYTHON") {
        Path::new(&python_path).exists()
    } else {
        false
    }
}

/// Get the Python executable path from environment
fn get_python_path() -> Result<PathBuf> {
    std::env::var("MESH_PYTHON")
        .map(PathBuf::from)
        .map_err(|_| anyhow!(
            "MESH_PYTHON environment variable not set. \
             Python algorithms require the Nix development shell or installed package."
        ))
}

/// Get the scripts directory path from environment
fn get_scripts_path() -> Result<PathBuf> {
    std::env::var("MESH_SCRIPTS")
        .map(PathBuf::from)
        .map_err(|_| anyhow!(
            "MESH_SCRIPTS environment variable not set. \
             Python algorithms require the Nix development shell or installed package."
        ))
}

/// Write audio samples to a temporary WAV file
///
/// Madmom and other Python libraries expect audio files rather than
/// raw sample arrays. This writes samples to a 16-bit mono WAV file.
fn write_temp_wav(samples: &[f32], sample_rate: u32) -> Result<tempfile::NamedTempFile> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    // Create temp file that persists until dropped
    let temp_file = tempfile::Builder::new()
        .prefix("mesh_bpm_")
        .suffix(".wav")
        .tempfile()
        .context("Failed to create temporary WAV file")?;

    let mut writer = hound::WavWriter::new(
        std::io::BufWriter::new(temp_file.reopen()?),
        spec,
    ).context("Failed to create WAV writer")?;

    // Convert f32 samples to i16
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let i16_sample = (clamped * 32767.0) as i16;
        writer.write_sample(i16_sample)
            .context("Failed to write WAV sample")?;
    }

    writer.finalize().context("Failed to finalize WAV file")?;

    Ok(temp_file)
}

/// Madmom DBN beat detector
///
/// Uses madmom's RNNBeatProcessor + DBNBeatTrackingProcessor pipeline
/// for highly accurate beat detection, especially on electronic music.
pub struct MadmomDetector {
    python_path: PathBuf,
    script_path: PathBuf,
}

impl MadmomDetector {
    /// Create a new Madmom detector
    ///
    /// Requires MESH_PYTHON and MESH_SCRIPTS environment variables to be set.
    pub fn new() -> Result<Self> {
        let python_path = get_python_path()?;
        let scripts_path = get_scripts_path()?;
        let script_path = scripts_path.join("madmom_beats.py");

        if !python_path.exists() {
            return Err(anyhow!(
                "Python executable not found at {:?}",
                python_path
            ));
        }

        if !script_path.exists() {
            return Err(anyhow!(
                "Madmom beat detection script not found at {:?}",
                script_path
            ));
        }

        Ok(Self {
            python_path,
            script_path,
        })
    }

    /// Run Python subprocess and parse JSON output
    fn run_python_script(&self, wav_path: &Path, config: &BpmConfig) -> Result<PythonBpmOutput> {
        log::info!(
            "Running madmom beat detection on {:?} (BPM range: {}-{})",
            wav_path,
            config.min_tempo,
            config.max_tempo
        );

        let output = Command::new(&self.python_path)
            .arg(&self.script_path)
            .arg(wav_path)
            .arg("--min-bpm")
            .arg(config.min_tempo.to_string())
            .arg("--max-bpm")
            .arg(config.max_tempo.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to execute madmom Python script")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "Madmom script failed with exit code {:?}: {}",
                output.status.code(),
                stderr
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        serde_json::from_str(&stdout).with_context(|| {
            format!(
                "Failed to parse madmom JSON output: {}",
                stdout.chars().take(200).collect::<String>()
            )
        })
    }
}

impl BpmDetector for MadmomDetector {
    fn detect(&self, samples: &[f32], config: &BpmConfig) -> Result<BpmResult> {
        log::info!(
            "MadmomDetector: Starting detection on {} samples ({:.1}s at {}Hz)",
            samples.len(),
            samples.len() as f64 / ANALYSIS_SAMPLE_RATE as f64,
            ANALYSIS_SAMPLE_RATE
        );

        // Write samples to temporary WAV file
        let temp_wav = write_temp_wav(samples, ANALYSIS_SAMPLE_RATE)
            .context("Failed to write temporary WAV for madmom")?;

        // Run Python script
        let result = self.run_python_script(temp_wav.path(), config)?;

        // Check for errors in output
        if let Some(error) = result.error {
            return Err(anyhow!("Madmom detection error: {}", error));
        }

        log::info!(
            "MadmomDetector: Detected {:.2} BPM, {} beats, confidence: {:.2}",
            result.bpm,
            result.beats.len(),
            result.confidence
        );

        // Temp file is automatically deleted when dropped
        Ok(BpmResult::new(result.bpm, result.confidence, result.beats))
    }

    fn algorithm(&self) -> BpmAlgorithm {
        BpmAlgorithm::MadmomDbn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_available_check() {
        // This test just verifies the function doesn't panic
        let _available = python_algorithms_available();
    }

    #[test]
    fn test_write_temp_wav() {
        // Generate a simple sine wave
        let sample_rate = 44100;
        let duration_secs = 0.1;
        let num_samples = (sample_rate as f32 * duration_secs) as usize;
        let frequency = 440.0;

        let samples: Vec<f32> = (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (2.0 * std::f32::consts::PI * frequency * t).sin() * 0.5
            })
            .collect();

        let temp_file = write_temp_wav(&samples, sample_rate).unwrap();
        assert!(temp_file.path().exists());

        // Verify the WAV file is readable
        let reader = hound::WavReader::open(temp_file.path()).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, sample_rate);
    }

    #[test]
    fn test_madmom_detector_requires_env() {
        // Without MESH_PYTHON set, creation should fail
        std::env::remove_var("MESH_PYTHON");
        let result = MadmomDetector::new();
        assert!(result.is_err());
    }
}
