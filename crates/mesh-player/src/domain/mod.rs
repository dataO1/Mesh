//! Domain layer for mesh-player
//!
//! This module orchestrates all services and manages application state.
//! It provides a clean separation between:
//! - **UI Layer**: Display and user input only
//! - **Domain Layer**: Business logic, service orchestration, state management
//! - **Service Layer**: Low-level services (database, audio engine, USB)
//!
//! ## Key Responsibilities
//!
//! - Manages active storage source (local vs USB)
//! - Loads track metadata from the correct database
//! - Coordinates track loading with the TrackLoader
//! - Forwards commands to the audio engine
//!
//! ## Usage
//!
//! ```ignore
//! // UI calls domain methods instead of accessing services directly
//! domain.load_track_to_deck(0, &path);
//! domain.set_active_storage(StorageSource::Usb { index: 0 });
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mesh_core::audio_file::TrackMetadata;
use mesh_core::db::DatabaseService;

/// Active storage source for track browsing and loading
#[derive(Clone)]
pub enum StorageSource {
    /// Local collection (default)
    Local,
    /// USB storage with its own database
    Usb {
        /// USB device index
        index: usize,
        /// Path to USB collection root
        path: PathBuf,
        /// Database service for this USB
        db: Arc<DatabaseService>,
    },
}

impl std::fmt::Debug for StorageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => write!(f, "Local"),
            Self::Usb { index, path, .. } => {
                f.debug_struct("Usb")
                    .field("index", index)
                    .field("path", path)
                    .finish_non_exhaustive()
            }
        }
    }
}

impl Default for StorageSource {
    fn default() -> Self {
        Self::Local
    }
}

/// Domain layer that orchestrates all services
///
/// This struct manages the interaction between different services
/// and ensures the correct database is used for each operation.
pub struct MeshDomain {
    /// Local database service (always available)
    local_db: Arc<DatabaseService>,

    /// Currently active storage source
    active_storage: StorageSource,

    /// Local collection path
    local_collection_path: PathBuf,
}

impl MeshDomain {
    /// Create a new domain layer
    ///
    /// # Arguments
    /// * `local_db` - Database service for local collection
    /// * `local_collection_path` - Path to local collection root
    pub fn new(local_db: Arc<DatabaseService>, local_collection_path: PathBuf) -> Self {
        Self {
            local_db,
            active_storage: StorageSource::Local,
            local_collection_path,
        }
    }

    // =========================================================================
    // Storage Management
    // =========================================================================

    /// Get the currently active storage source
    pub fn active_storage(&self) -> &StorageSource {
        &self.active_storage
    }

    /// Set the active storage source
    ///
    /// This determines which database is used for metadata lookups.
    pub fn set_active_storage(&mut self, source: StorageSource) {
        log::info!("Domain: Switching storage to {:?}", match &source {
            StorageSource::Local => "Local".to_string(),
            StorageSource::Usb { index, path, .. } => format!("USB {} at {:?}", index, path),
        });
        self.active_storage = source;
    }

    /// Switch to local storage
    pub fn switch_to_local(&mut self) {
        self.set_active_storage(StorageSource::Local);
    }

    /// Switch to USB storage
    ///
    /// Creates a new DatabaseService for the USB if needed.
    pub fn switch_to_usb(&mut self, index: usize, usb_collection_path: &Path) -> Result<(), String> {
        // Create database service for USB
        let db = DatabaseService::new(usb_collection_path)
            .map_err(|e| format!("Failed to open USB database: {}", e))?;

        self.set_active_storage(StorageSource::Usb {
            index,
            path: usb_collection_path.to_path_buf(),
            db,
        });

        Ok(())
    }

    /// Get the active database service
    ///
    /// Returns the database for the currently active storage source.
    pub fn active_db(&self) -> &DatabaseService {
        match &self.active_storage {
            StorageSource::Local => &self.local_db,
            StorageSource::Usb { db, .. } => db,
        }
    }

    /// Get the local database service
    pub fn local_db(&self) -> &DatabaseService {
        &self.local_db
    }

    /// Get the active collection path
    pub fn active_collection_path(&self) -> &Path {
        match &self.active_storage {
            StorageSource::Local => &self.local_collection_path,
            StorageSource::Usb { path, .. } => path,
        }
    }

    // =========================================================================
    // Track Metadata
    // =========================================================================

    /// Load track metadata from the active database
    ///
    /// This is the primary method for getting metadata before loading a track.
    /// It automatically uses the correct database based on active storage.
    pub fn load_track_metadata(&self, path: &str) -> Option<TrackMetadata> {
        let db = self.active_db();
        match db.load_track_metadata_by_path(path) {
            Ok(Some(db_meta)) => Some(db_meta.into()),
            Ok(None) => {
                log::warn!("Track not found in active database: {}", path);
                None
            }
            Err(e) => {
                log::error!("Failed to load track metadata: {}", e);
                None
            }
        }
    }

    /// Load track metadata, falling back to defaults if not found
    pub fn load_track_metadata_or_default(&self, path: &str) -> TrackMetadata {
        self.load_track_metadata(path).unwrap_or_default()
    }

    // =========================================================================
    // Storage Detection
    // =========================================================================

    /// Check if a path belongs to a USB storage
    pub fn is_usb_path(&self, path: &Path) -> bool {
        // USB paths typically start with /run/media or similar
        // This is a heuristic - UsbManager has more precise detection
        if let Some(path_str) = path.to_str() {
            path_str.starts_with("/run/media/")
                || path_str.starts_with("/media/")
                || path_str.starts_with("/mnt/")
        } else {
            false
        }
    }

    /// Check if currently browsing USB storage
    pub fn is_browsing_usb(&self) -> bool {
        matches!(self.active_storage, StorageSource::Usb { .. })
    }
}
