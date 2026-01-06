//! Playlist management for mesh-cue
//!
//! This module provides a hierarchical playlist system with:
//! - Abstract storage trait for future DB/file backend flexibility
//! - Filesystem implementation using symlinks for track references
//! - Tree-based navigation with folders and playlists

pub mod filesystem;
pub use filesystem::FilesystemStorage;

use std::path::PathBuf;

/// Unique identifier for a node in the playlist tree.
/// Uses path-like strings (e.g., "tracks/subfolder/track.wav", "playlists/Live Set/Opening")
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeId(pub String);

impl NodeId {
    /// Create the virtual root node ID
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Create the "tracks" (General Collection) node ID
    pub fn tracks() -> Self {
        Self("tracks".to_string())
    }

    /// Create the "playlists" root node ID
    pub fn playlists() -> Self {
        Self("playlists".to_string())
    }

    /// Create a child node ID by appending a path segment
    pub fn child(&self, name: &str) -> Self {
        if self.0.is_empty() {
            Self(name.to_string())
        } else {
            Self(format!("{}/{}", self.0, name))
        }
    }

    /// Get the parent node ID, or None if this is the root
    pub fn parent(&self) -> Option<Self> {
        self.0.rsplit_once('/').map(|(parent, _)| Self(parent.to_string()))
    }

    /// Get the name (last segment) of this node
    pub fn name(&self) -> &str {
        self.0.rsplit_once('/').map(|(_, name)| name).unwrap_or(&self.0)
    }

    /// Check if this is the root node
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Check if this node is under the tracks folder
    pub fn is_in_tracks(&self) -> bool {
        self.0.starts_with("tracks")
    }

    /// Check if this node is under the playlists folder
    pub fn is_in_playlists(&self) -> bool {
        self.0.starts_with("playlists")
    }

    /// Get the path string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            write!(f, "<root>")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

/// Type of node in the playlist tree
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Virtual root of the tree
    Root,
    /// The "General Collection" folder (tracks/)
    Collection,
    /// A subfolder within the collection
    CollectionFolder,
    /// The "Playlists" folder (playlists/)
    PlaylistsRoot,
    /// A user-created playlist folder
    Playlist,
    /// An audio track (real file or symlink)
    Track,
}

impl NodeKind {
    /// Check if this node kind can contain children
    pub fn is_container(&self) -> bool {
        !matches!(self, NodeKind::Track)
    }

    /// Check if this is a playlist that can be modified by the user
    pub fn is_user_editable(&self) -> bool {
        matches!(self, NodeKind::Playlist)
    }
}

/// A node in the playlist tree
#[derive(Debug, Clone)]
pub struct PlaylistNode {
    /// Unique identifier for this node
    pub id: NodeId,
    /// Type of node
    pub kind: NodeKind,
    /// Display name
    pub name: String,
    /// Child node IDs (empty for tracks)
    pub children: Vec<NodeId>,
    /// For tracks: path to actual audio file (resolved from symlink if applicable)
    pub track_path: Option<PathBuf>,
}

impl PlaylistNode {
    /// Check if this node is a container (can have children)
    pub fn is_container(&self) -> bool {
        self.kind.is_container()
    }

    /// Check if this is a track node
    pub fn is_track(&self) -> bool {
        matches!(self.kind, NodeKind::Track)
    }
}

/// Track information for display in the table view
#[derive(Debug, Clone)]
pub struct TrackInfo {
    /// Node ID in the tree
    pub id: NodeId,
    /// Display name (filename without extension)
    pub name: String,
    /// Path to the actual audio file
    pub path: PathBuf,
    /// Artist name if set
    pub artist: Option<String>,
    /// BPM if detected/set
    pub bpm: Option<f64>,
    /// Musical key if detected/set
    pub key: Option<String>,
    /// Duration in seconds
    pub duration: Option<f64>,
}

impl TrackInfo {
    /// Format duration as MM:SS
    pub fn format_duration(&self) -> String {
        self.duration
            .map(|d| {
                let mins = (d / 60.0) as u32;
                let secs = (d % 60.0) as u32;
                format!("{}:{:02}", mins, secs)
            })
            .unwrap_or_else(|| "--:--".to_string())
    }

    /// Format BPM with one decimal place
    pub fn format_bpm(&self) -> String {
        self.bpm
            .map(|b| format!("{:.1}", b))
            .unwrap_or_else(|| "-".to_string())
    }
}

/// Errors that can occur during playlist operations
#[derive(Debug)]
pub enum PlaylistError {
    /// A playlist with this name already exists
    AlreadyExists(String),
    /// The requested playlist or node was not found
    NotFound(String),
    /// Cannot modify collection folders (they're read-only)
    CannotModifyCollection,
    /// IO error during file operations
    Io(std::io::Error),
    /// Invalid operation attempted
    InvalidOperation(String),
}

impl std::fmt::Display for PlaylistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaylistError::AlreadyExists(name) => write!(f, "Playlist already exists: {}", name),
            PlaylistError::NotFound(name) => write!(f, "Playlist not found: {}", name),
            PlaylistError::CannotModifyCollection => write!(f, "Cannot modify collection folders"),
            PlaylistError::Io(e) => write!(f, "IO error: {}", e),
            PlaylistError::InvalidOperation(msg) => write!(f, "Invalid operation: {}", msg),
        }
    }
}

impl std::error::Error for PlaylistError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PlaylistError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for PlaylistError {
    fn from(e: std::io::Error) -> Self {
        PlaylistError::Io(e)
    }
}

/// Abstract storage backend for playlists.
///
/// This trait abstracts the storage mechanism, allowing for different
/// implementations (filesystem with symlinks, SQLite database, etc.)
pub trait PlaylistStorage: Send + Sync {
    /// Get the root node of the tree
    fn root(&self) -> PlaylistNode;

    /// Get a node by its ID
    fn get_node(&self, id: &NodeId) -> Option<PlaylistNode>;

    /// Get all children of a node
    fn get_children(&self, id: &NodeId) -> Vec<PlaylistNode>;

    /// Get all tracks in a folder (for table display)
    fn get_tracks(&self, folder_id: &NodeId) -> Vec<TrackInfo>;

    /// Create a new playlist folder
    fn create_playlist(&mut self, parent: &NodeId, name: &str) -> Result<NodeId, PlaylistError>;

    /// Rename a playlist
    fn rename_playlist(&mut self, id: &NodeId, new_name: &str) -> Result<(), PlaylistError>;

    /// Delete a playlist and all its contents
    fn delete_playlist(&mut self, id: &NodeId) -> Result<(), PlaylistError>;

    /// Add a track to a playlist (creates a symlink in filesystem impl)
    fn add_track_to_playlist(
        &mut self,
        track_path: &PathBuf,
        playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError>;

    /// Remove a track from a playlist (removes symlink, not original file)
    fn remove_track_from_playlist(&mut self, track_id: &NodeId) -> Result<(), PlaylistError>;

    /// Move a track between playlists
    fn move_track(
        &mut self,
        track_id: &NodeId,
        target_playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError>;

    /// Refresh the tree from storage (re-scan filesystem, etc.)
    fn refresh(&mut self) -> Result<(), PlaylistError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id_child() {
        let root = NodeId::root();
        let tracks = root.child("tracks");
        assert_eq!(tracks.as_str(), "tracks");

        let subfolder = tracks.child("subfolder");
        assert_eq!(subfolder.as_str(), "tracks/subfolder");

        let track = subfolder.child("song.wav");
        assert_eq!(track.as_str(), "tracks/subfolder/song.wav");
    }

    #[test]
    fn test_node_id_parent() {
        let node = NodeId("tracks/subfolder/song.wav".to_string());
        let parent = node.parent().unwrap();
        assert_eq!(parent.as_str(), "tracks/subfolder");

        let grandparent = parent.parent().unwrap();
        assert_eq!(grandparent.as_str(), "tracks");

        assert!(NodeId::root().parent().is_none());
    }

    #[test]
    fn test_node_id_name() {
        let node = NodeId("tracks/subfolder/song.wav".to_string());
        assert_eq!(node.name(), "song.wav");

        let tracks = NodeId::tracks();
        assert_eq!(tracks.name(), "tracks");
    }

    #[test]
    fn test_node_id_location() {
        let track = NodeId("tracks/subfolder/song.wav".to_string());
        assert!(track.is_in_tracks());
        assert!(!track.is_in_playlists());

        let playlist = NodeId("playlists/Live Set/Opening".to_string());
        assert!(!playlist.is_in_tracks());
        assert!(playlist.is_in_playlists());
    }
}
