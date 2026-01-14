//! Path utilities for mesh configuration files
//!
//! Provides standard paths for the mesh collection and configuration files.

use std::path::PathBuf;

/// Get the default collection path
///
/// Returns: `~/Music/mesh-collection`
///
/// This is the standard location for the mesh audio collection,
/// shared between mesh-player and mesh-cue.
pub fn default_collection_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
}

/// Get the default config file path for a given app
///
/// # Arguments
/// * `filename` - Config file name (e.g., "config.yaml", "player-config.yaml")
///
/// Returns: `~/Music/mesh-collection/{filename}`
pub fn default_config_path(filename: &str) -> PathBuf {
    default_collection_path().join(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collection_path_ends_with_mesh_collection() {
        let path = default_collection_path();
        assert!(path.ends_with("mesh-collection"));
    }

    #[test]
    fn test_config_path_includes_filename() {
        let path = default_config_path("test.yaml");
        assert!(path.ends_with("test.yaml"));
    }
}
