//! USB Database Cache
//!
//! Provides cached access to USB database instances, avoiding redundant
//! database opens when multiple components need the same USB's database.
//!
//! # Architecture
//!
//! The cache is managed by UsbManager, which registers databases when USB
//! devices are initialized and clears them when devices are disconnected.
//!
//! ```text
//! UsbManager (source of truth)
//!     │
//!     ├─► register_usb_database()   ← called on USB init
//!     ├─► get_usb_database()        ← used by linked stem loader, export, etc.
//!     └─► clear_usb_database()      ← called on USB disconnect
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::db::DatabaseService;

/// Module-level cache for USB DatabaseService instances
///
/// Keyed by collection_root path (e.g., "/run/media/user/USB/mesh-collection").
/// Uses RwLock for concurrent read access with exclusive write access.
static USB_DB_CACHE: RwLock<Option<HashMap<PathBuf, Arc<DatabaseService>>>> = RwLock::new(None);

/// Register a USB database in the cache
///
/// Called by UsbManager when a USB device is initialized. If a database
/// for this path is already cached, it will be replaced.
pub fn register_usb_database(collection_root: PathBuf, db: Arc<DatabaseService>) {
    if let Ok(mut cache) = USB_DB_CACHE.write() {
        let map = cache.get_or_insert_with(HashMap::new);
        log::info!("[USB_CACHE] Registered database for {:?}", collection_root);
        map.insert(collection_root, db);
    }
}

/// Get a cached USB database by collection root path
///
/// Returns None if no database is cached for this path.
pub fn get_usb_database(collection_root: &Path) -> Option<Arc<DatabaseService>> {
    if let Ok(cache) = USB_DB_CACHE.read() {
        if let Some(map) = cache.as_ref() {
            if let Some(db) = map.get(collection_root) {
                log::debug!("[USB_CACHE] Cache hit for {:?}", collection_root);
                return Some(db.clone());
            }
        }
    }
    None
}

/// Get or open a USB database
///
/// First checks the cache, then opens a new database if not cached.
/// Newly opened databases are automatically registered in the cache.
///
/// This is the preferred method for getting a USB database from any code.
pub fn get_or_open_usb_database(collection_root: &Path) -> Option<Arc<DatabaseService>> {
    // Check cache first (fast path)
    if let Some(db) = get_usb_database(collection_root) {
        return Some(db);
    }

    // Cache miss - open and register
    log::info!("[USB_CACHE] Cache miss, opening database at {:?}", collection_root);
    match DatabaseService::new(collection_root) {
        Ok(db) => {
            register_usb_database(collection_root.to_path_buf(), db.clone());
            Some(db)
        }
        Err(e) => {
            log::error!("[USB_CACHE] Failed to open database at {:?}: {}", collection_root, e);
            None
        }
    }
}

/// Clear a specific USB database from the cache
///
/// Called by UsbManager when a USB device is disconnected.
pub fn clear_usb_database(collection_root: &Path) {
    if let Ok(mut cache) = USB_DB_CACHE.write() {
        if let Some(map) = cache.as_mut() {
            if map.remove(collection_root).is_some() {
                log::info!("[USB_CACHE] Cleared database for {:?}", collection_root);
            }
        }
    }
}

/// Clear all USB databases from the cache
///
/// Called on application shutdown or when clearing all USB state.
pub fn clear_all_usb_databases() {
    if let Ok(mut cache) = USB_DB_CACHE.write() {
        if let Some(map) = cache.as_mut() {
            let count = map.len();
            map.clear();
            log::info!("[USB_CACHE] Cleared all {} cached databases", count);
        }
    }
}

/// Find the mesh-collection root for a file path
///
/// Walks up the directory tree looking for a "mesh-collection" directory.
/// Returns None if not found or if path doesn't appear to be on a USB device.
pub fn find_collection_root(path: &Path) -> Option<PathBuf> {
    let mut current = path.parent()?;

    while let Some(parent) = current.parent() {
        if current.file_name()?.to_str()? == "mesh-collection" {
            return Some(current.to_path_buf());
        }

        // Stop at mount point roots
        if parent.to_str() == Some("/run/media")
            || parent.to_str() == Some("/media")
            || parent.to_str() == Some("/mnt")
        {
            return None;
        }

        current = parent;
    }

    None
}

/// Get a USB database for a file path
///
/// Finds the collection root containing the file and returns the cached
/// database (opening it if necessary).
pub fn get_usb_database_for_path(path: &Path) -> Option<Arc<DatabaseService>> {
    let collection_root = find_collection_root(path)?;
    get_or_open_usb_database(&collection_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_collection_root() {
        // Should find mesh-collection in path
        let path = Path::new("/run/media/user/USB/mesh-collection/tracks/song.wav");
        let root = find_collection_root(path);
        assert_eq!(root, Some(PathBuf::from("/run/media/user/USB/mesh-collection")));

        // Should return None for paths without mesh-collection
        let path = Path::new("/home/user/Music/song.wav");
        assert!(find_collection_root(path).is_none());
    }
}
