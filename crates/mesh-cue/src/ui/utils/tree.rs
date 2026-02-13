//! Tree and playlist helper utilities
//!
//! Functions for building tree nodes and getting tracks from playlist storage.

use crate::domain::DomainTrack;
use mesh_core::playlist::{NodeId, NodeKind, PlaylistNode, PlaylistStorage};
use mesh_widgets::{parse_hex_color, TrackRow, TrackTag, TreeIcon, TreeNode};

/// Build tree nodes from playlist storage
pub fn build_tree_nodes(storage: &dyn PlaylistStorage) -> Vec<TreeNode<NodeId>> {
    let root = storage.root();
    build_node_children(storage, &root)
}

/// Recursively build tree node children
fn build_node_children(storage: &dyn PlaylistStorage, parent: &PlaylistNode) -> Vec<TreeNode<NodeId>> {
    storage
        .get_children(&parent.id)
        .into_iter()
        .filter(|node| node.kind != NodeKind::Track) // Only folders in tree
        .map(|node| {
            let icon = match node.kind {
                NodeKind::Collection => TreeIcon::Collection,
                NodeKind::CollectionFolder => TreeIcon::Folder,
                NodeKind::PlaylistsRoot => TreeIcon::Folder,
                NodeKind::Playlist => TreeIcon::Playlist,
                _ => TreeIcon::Folder,
            };

            // Allow creating children in playlists root and playlist folders
            let allow_create = matches!(node.kind, NodeKind::PlaylistsRoot | NodeKind::Playlist);
            // Allow renaming only playlist folders (not collection, not playlists root)
            let allow_rename = matches!(node.kind, NodeKind::Playlist);

            TreeNode::with_children(
                node.id.clone(),
                node.name.clone(),
                icon,
                build_node_children(storage, &node),
            )
            .with_create_child(allow_create)
            .with_rename(allow_rename)
        })
        .collect()
}

/// Get tracks for a folder as TrackRow items for display
pub fn get_tracks_for_folder(storage: &dyn PlaylistStorage, folder_id: &NodeId) -> Vec<TrackRow<NodeId>> {
    storage
        .get_tracks(folder_id)
        .into_iter()
        .map(|info| {
            let mut row = TrackRow::new(info.id, info.name, info.order);
            if let Some(artist) = info.artist {
                row = row.with_artist(artist);
            }
            if let Some(bpm) = info.bpm {
                row = row.with_bpm(bpm);
            }
            if let Some(key) = info.key {
                row = row.with_key(key);
            }
            if let Some(duration) = info.duration {
                row = row.with_duration(duration);
            }
            if let Some(lufs) = info.lufs {
                row = row.with_lufs(lufs);
            }
            if !info.tags.is_empty() {
                let tags: Vec<TrackTag> = info.tags.iter().map(|(label, color)| {
                    let mut tag = TrackTag::new(label);
                    if let Some(hex) = color {
                        if let Some(c) = parse_hex_color(hex) {
                            tag = tag.with_color(c);
                        }
                    }
                    tag
                }).collect();
                row = row.with_tags(tags);
            }
            row
        })
        .collect()
}

/// Convert domain Tracks to UI TrackRow items
///
/// Used when querying tracks directly from the domain layer (which returns DomainTrack objects)
/// instead of through PlaylistStorage (which returns TrackInfo).
pub fn tracks_to_rows(tracks: &[DomainTrack]) -> Vec<TrackRow<NodeId>> {
    tracks
        .iter()
        .enumerate()
        .map(|(i, track)| {
            // Create NodeId from track path for UI identification
            let node_id = NodeId::tracks().child(&track.path.to_string_lossy());
            let mut row = TrackRow::new(node_id, &track.name, (i + 1) as i32);
            if let Some(ref artist) = track.artist {
                row = row.with_artist(artist);
            }
            if let Some(bpm) = track.bpm {
                row = row.with_bpm(bpm);
            }
            if let Some(ref key) = track.key {
                row = row.with_key(key);
            }
            row = row.with_duration(track.duration_seconds);
            if let Some(lufs) = track.lufs {
                row = row.with_lufs(lufs);
            }
            row
        })
        .collect()
}
