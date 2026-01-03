//! Stem file import module
//!
//! Imports 4 separate stereo WAV files (Vocals, Drums, Bass, Other)
//! and combines them into a single StemBuffers structure.

use anyhow::{bail, Context, Result};
use mesh_core::audio_file::StemBuffers;
use mesh_core::types::{StereoBuffer, StereoSample};
use std::path::Path;

/// Stem file importer
#[derive(Debug)]
pub struct StemImporter {
    /// Path to vocals stem (stereo WAV)
    pub vocals_path: Option<std::path::PathBuf>,
    /// Path to drums stem (stereo WAV)
    pub drums_path: Option<std::path::PathBuf>,
    /// Path to bass stem (stereo WAV)
    pub bass_path: Option<std::path::PathBuf>,
    /// Path to other stem (stereo WAV)
    pub other_path: Option<std::path::PathBuf>,
}

impl Default for StemImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl StemImporter {
    /// Create a new stem importer
    pub fn new() -> Self {
        Self {
            vocals_path: None,
            drums_path: None,
            bass_path: None,
            other_path: None,
        }
    }

    /// Set the vocals stem path
    pub fn set_vocals(&mut self, path: impl AsRef<Path>) {
        self.vocals_path = Some(path.as_ref().to_path_buf());
    }

    /// Set the drums stem path
    pub fn set_drums(&mut self, path: impl AsRef<Path>) {
        self.drums_path = Some(path.as_ref().to_path_buf());
    }

    /// Set the bass stem path
    pub fn set_bass(&mut self, path: impl AsRef<Path>) {
        self.bass_path = Some(path.as_ref().to_path_buf());
    }

    /// Set the other stem path
    pub fn set_other(&mut self, path: impl AsRef<Path>) {
        self.other_path = Some(path.as_ref().to_path_buf());
    }

    /// Check if all stems are loaded
    pub fn is_complete(&self) -> bool {
        self.vocals_path.is_some()
            && self.drums_path.is_some()
            && self.bass_path.is_some()
            && self.other_path.is_some()
    }

    /// Get the number of loaded stems (0-4)
    pub fn loaded_count(&self) -> usize {
        [
            self.vocals_path.is_some(),
            self.drums_path.is_some(),
            self.bass_path.is_some(),
            self.other_path.is_some(),
        ]
        .iter()
        .filter(|&&b| b)
        .count()
    }

    /// Clear all loaded stems
    pub fn clear(&mut self) {
        self.vocals_path = None;
        self.drums_path = None;
        self.bass_path = None;
        self.other_path = None;
    }

    /// Import all stems and combine into StemBuffers
    ///
    /// This loads all 4 stem files, validates they are compatible
    /// (same length, same sample rate), and interleaves them into
    /// the 8-channel format used by mesh-player.
    pub fn import(&self) -> Result<StemBuffers> {
        if !self.is_complete() {
            bail!("Not all stems are loaded");
        }

        let vocals_path = self.vocals_path.as_ref().unwrap();
        let drums_path = self.drums_path.as_ref().unwrap();
        let bass_path = self.bass_path.as_ref().unwrap();
        let other_path = self.other_path.as_ref().unwrap();

        // Load each stem file
        let vocals = load_stereo_wav(vocals_path)
            .with_context(|| format!("Failed to load vocals: {:?}", vocals_path))?;
        let drums = load_stereo_wav(drums_path)
            .with_context(|| format!("Failed to load drums: {:?}", drums_path))?;
        let bass = load_stereo_wav(bass_path)
            .with_context(|| format!("Failed to load bass: {:?}", bass_path))?;
        let other = load_stereo_wav(other_path)
            .with_context(|| format!("Failed to load other: {:?}", other_path))?;

        // Validate all stems have the same length
        let len = vocals.len();
        if drums.len() != len || bass.len() != len || other.len() != len {
            bail!(
                "Stem files have different lengths: vocals={}, drums={}, bass={}, other={}",
                vocals.len(),
                drums.len(),
                bass.len(),
                other.len()
            );
        }

        // Combine into StemBuffers
        let mut buffers = StemBuffers::with_length(len);

        for i in 0..len {
            buffers.vocals.as_mut_slice()[i] = vocals[i];
            buffers.drums.as_mut_slice()[i] = drums[i];
            buffers.bass.as_mut_slice()[i] = bass[i];
            buffers.other.as_mut_slice()[i] = other[i];
        }

        Ok(buffers)
    }

    /// Get mono-summed audio for analysis
    ///
    /// Combines all stems into a single mono channel for BPM/key analysis.
    pub fn get_mono_sum(&self) -> Result<Vec<f32>> {
        let buffers = self.import()?;
        let len = buffers.len();

        let mono: Vec<f32> = (0..len)
            .map(|i| {
                // Sum all stems and convert to mono
                let vocals_mono = (buffers.vocals[i].left + buffers.vocals[i].right) * 0.5;
                let drums_mono = (buffers.drums[i].left + buffers.drums[i].right) * 0.5;
                let bass_mono = (buffers.bass[i].left + buffers.bass[i].right) * 0.5;
                let other_mono = (buffers.other[i].left + buffers.other[i].right) * 0.5;

                // Mix all stems (with some headroom)
                (vocals_mono + drums_mono + bass_mono + other_mono) * 0.25
            })
            .collect();

        Ok(mono)
    }
}

/// Load a stereo WAV file and return sample pairs
fn load_stereo_wav(path: &Path) -> Result<Vec<StereoSample>> {
    // Use riff crate to parse WAV file
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);

    // TODO: Implement proper WAV loading using riff crate
    // For now, return a placeholder
    //
    // The implementation should:
    // 1. Parse RIFF header
    // 2. Find 'fmt ' chunk and validate format (PCM, stereo, 44.1kHz or convert)
    // 3. Find 'data' chunk and read samples
    // 4. Convert to f32 StereoSample pairs

    bail!("WAV loading not yet implemented - this is a placeholder")
}
