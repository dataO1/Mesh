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
        log::info!("scan: Scanning collection at {:?}", self.path);
        self.tracks.clear();

        // Ensure directory exists
        if !self.path.exists() {
            log::info!("scan: Directory doesn't exist, creating it");
            fs::create_dir_all(&self.path)
                .with_context(|| format!("Failed to create collection directory: {:?}", self.path))?;
            return Ok(());
        }

        // Scan the tracks/ subdirectory for WAV files
        let tracks_dir = self.path.join("tracks");
        if !tracks_dir.exists() {
            log::info!("scan: Tracks directory doesn't exist, creating it");
            fs::create_dir_all(&tracks_dir)?;
            return Ok(());
        }

        let entries = fs::read_dir(&tracks_dir)
            .with_context(|| format!("Failed to read tracks directory: {:?}", tracks_dir))?;

        let mut file_count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            log::debug!("scan: Found entry: {:?}", path);

            // Check if it's a WAV file
            if path.extension().map_or(false, |ext| ext.eq_ignore_ascii_case("wav")) {
                file_count += 1;
                log::info!("scan: Loading WAV file: {:?}", path);
                match self.load_track_info(&path) {
                    Ok(track) => {
                        log::info!("scan: Loaded track '{}' (BPM={:.1}, Key={})", track.name, track.bpm, track.key);
                        self.tracks.push(track);
                    }
                    Err(e) => {
                        log::warn!("Failed to load track info for {:?}: {}", path, e);
                    }
                }
            }
        }

        // Sort by name
        self.tracks.sort_by(|a, b| a.name.cmp(&b.name));
        log::info!("scan: Complete, found {} WAV files, loaded {} tracks", file_count, self.tracks.len());

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
        log::info!("add_track: Adding '{}' to collection", name);
        log::info!("  Source: {:?}", source_path);
        log::info!("  Collection dir: {:?}", self.path);

        // Verify source exists
        match std::fs::metadata(source_path) {
            Ok(meta) => log::info!("  Source file exists: {} bytes", meta.len()),
            Err(e) => {
                log::error!("  Source file doesn't exist: {}", e);
                anyhow::bail!("Source file doesn't exist: {:?}", source_path);
            }
        }

        // Ensure collection directory and tracks subdirectory exist
        log::info!("  Creating collection directory if needed...");
        let tracks_dir = self.path.join("tracks");
        fs::create_dir_all(&tracks_dir)?;
        log::info!("  Tracks directory ready: {:?}", tracks_dir);

        // Create destination path in tracks/ subdirectory
        let dest_name = format!("{}.wav", sanitize_filename(name));
        let dest_path = tracks_dir.join(&dest_name);
        log::info!("  Destination: {:?}", dest_path);

        // Copy file
        log::info!("  Copying file...");
        let bytes_copied = fs::copy(source_path, &dest_path)
            .with_context(|| format!("Failed to copy track to collection: {:?}", dest_path))?;
        log::info!("  Copied {} bytes", bytes_copied);

        // Verify destination exists
        match std::fs::metadata(&dest_path) {
            Ok(meta) => log::info!("  Destination file verified: {} bytes", meta.len()),
            Err(e) => log::error!("  Destination file check failed: {}", e),
        }

        // Refresh the collection
        log::info!("  Refreshing collection...");
        self.scan()?;
        log::info!("add_track: Complete, {} tracks in collection", self.tracks.len());

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
