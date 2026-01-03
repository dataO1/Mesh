//! Collection management for mesh-cue
//!
//! Manages a folder of 8-channel WAV files that represent the user's
//! prepared track collection.

use anyhow::{Context, Result};
use mesh_core::audio_file::{read_metadata, AudioFileReader};
use std::fs;
use std::path::{Path, PathBuf};

/// A track in the collection
#[derive(Debug, Clone)]
pub struct CollectionTrack {
    /// Full path to the WAV file
    pub path: PathBuf,
    /// Track display name (filename without extension)
    pub name: String,
    /// BPM from metadata
    pub bpm: f64,
    /// Musical key from metadata
    pub key: String,
    /// Duration in seconds
    pub duration: f64,
}

/// Collection manager
#[derive(Debug)]
pub struct Collection {
    /// Path to the collection folder
    path: PathBuf,
    /// Cached list of tracks
    tracks: Vec<CollectionTrack>,
}

impl Collection {
    /// Create a new collection at the given path
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            tracks: Vec::new(),
        }
    }

    /// Get the collection folder path
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get all tracks in the collection
    pub fn tracks(&self) -> &[CollectionTrack] {
        &self.tracks
    }

    /// Scan the collection folder for WAV files
    pub fn scan(&mut self) -> Result<()> {
        self.tracks.clear();

        // Ensure directory exists
        if !self.path.exists() {
            fs::create_dir_all(&self.path)
                .with_context(|| format!("Failed to create collection directory: {:?}", self.path))?;
            return Ok(());
        }

        // Scan for WAV files
        let entries = fs::read_dir(&self.path)
            .with_context(|| format!("Failed to read collection directory: {:?}", self.path))?;

        for entry in entries.flatten() {
            let path = entry.path();

            // Check if it's a WAV file
            if path.extension().map_or(false, |ext| ext.eq_ignore_ascii_case("wav")) {
                match self.load_track_info(&path) {
                    Ok(track) => self.tracks.push(track),
                    Err(e) => {
                        log::warn!("Failed to load track info for {:?}: {}", path, e);
                    }
                }
            }
        }

        // Sort by name
        self.tracks.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(())
    }

    /// Load track information from a WAV file
    fn load_track_info(&self, path: &Path) -> Result<CollectionTrack> {
        // Get filename as track name
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| String::from("Unknown"));

        // Try to load metadata and duration from the file
        let (bpm, key, duration) = match read_metadata(path) {
            Ok(metadata) => {
                // Get duration from audio reader
                let duration = match AudioFileReader::open(path) {
                    Ok(reader) => reader.duration_seconds(),
                    Err(_) => 0.0,
                };
                (
                    metadata.bpm.unwrap_or(120.0),
                    metadata.key.unwrap_or_else(|| String::from("?")),
                    duration,
                )
            }
            Err(_) => {
                // Fallback: just get basic info
                (120.0, String::from("?"), 0.0)
            }
        };

        Ok(CollectionTrack {
            path: path.to_path_buf(),
            name,
            bpm,
            key,
            duration,
        })
    }

    /// Add a track to the collection (copy file to collection folder)
    pub fn add_track(&mut self, source_path: &Path, name: &str) -> Result<PathBuf> {
        // Ensure collection directory exists
        fs::create_dir_all(&self.path)?;

        // Create destination path
        let dest_name = format!("{}.wav", sanitize_filename(name));
        let dest_path = self.path.join(&dest_name);

        // Copy file
        fs::copy(source_path, &dest_path)
            .with_context(|| format!("Failed to copy track to collection: {:?}", dest_path))?;

        // Refresh the collection
        self.scan()?;

        Ok(dest_path)
    }
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

impl Default for Collection {
    fn default() -> Self {
        // Default to ~/Music/mesh-collection
        let default_path = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("Music")
            .join("mesh-collection");
        Self::new(default_path)
    }
}
