//! Browser and USB message handler
//!
//! Handles collection browser navigation, track selection, and USB device events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use iced::Task;

use mesh_core::playlist::NodeId;
use mesh_core::usb::UsbMessage as UsbMsg;
use mesh_widgets::{scroll_to_centered_selection, TrackRow};
use crate::suggestions::{query_suggestions, SuggestedTrack};
use crate::ui::app::MeshApp;
use crate::ui::collection_browser::CollectionBrowserMessage;
use crate::ui::message::Message;

/// Handle collection browser messages
pub fn handle_browser(app: &mut MeshApp, browser_msg: CollectionBrowserMessage) -> Task<Message> {
    // Intercept suggestion messages before delegating to browser state
    match &browser_msg {
        CollectionBrowserMessage::ToggleSuggestions => {
            let was_enabled = app.collection_browser.is_suggestions_enabled();
            app.collection_browser.set_suggestions_enabled(!was_enabled);
            if !was_enabled {
                // Just turned on — trigger query
                app.collection_browser.set_suggestion_loading(true);
                return trigger_suggestion_query(app);
            }
            return Task::none();
        }
        CollectionBrowserMessage::RefreshSuggestions => {
            if app.collection_browser.is_suggestions_enabled() {
                app.collection_browser.set_suggestion_loading(true);
                return trigger_suggestion_query(app);
            }
            return Task::none();
        }
        CollectionBrowserMessage::SetEnergyDirection(value) => {
            let changed = app.collection_browser.set_energy_direction(*value);
            if changed && app.collection_browser.is_suggestions_enabled() {
                app.collection_browser.set_suggestion_loading(true);
                return trigger_suggestion_query(app);
            }
            return Task::none();
        }
        _ => {}
    }

    // Check if this is a scroll message (for auto-scroll after)
    let is_scroll = matches!(browser_msg, CollectionBrowserMessage::ScrollBy(_));

    // Handle collection browser message and check if we need to load a track
    if let Some((deck_idx, path)) = app.collection_browser.handle_message(browser_msg) {
        // Convert to LoadTrack message
        let path_str = path.to_string_lossy().to_string();
        return app.update(Message::LoadTrack(deck_idx, path_str));
    }

    // Sync domain layer with collection browser's active storage
    // This ensures metadata is loaded from the correct database (local or USB)
    if let Some((usb_idx, collection_path)) = app.collection_browser.get_active_usb_info() {
        // Browsing USB - switch domain to USB if not already
        if !app.domain.is_browsing_usb() {
            if let Err(e) = app.domain.switch_to_usb(usb_idx, &collection_path) {
                log::error!("Failed to switch domain to USB: {}", e);
            }
        }
    } else if app.domain.is_browsing_usb() {
        // Browsing local - switch domain back if it was on USB
        app.domain.switch_to_local();
    }

    // If it was a scroll, create a Task to auto-scroll the track list
    if is_scroll {
        if let Some(selected_idx) = app.collection_browser.get_selected_index() {
            let total_tracks = app.collection_browser.track_count();
            // Assumes ~280px visible height (10 rows at 28px each)
            let visible_height = 280.0;
            return scroll_to_centered_selection(selected_idx, total_tracks, visible_height);
        }
    }

    Task::none()
}

/// Handle USB device messages
pub fn handle_usb(app: &mut MeshApp, usb_msg: UsbMsg) -> Task<Message> {
    match usb_msg {
        UsbMsg::DevicesRefreshed(devices) => {
            log::info!("USB: {} devices detected", devices.len());
            app.collection_browser.update_usb_devices(devices.clone());
            // Initialize storages for mounted devices and trigger metadata preload
            for device in &devices {
                if device.mount_point.is_some() && device.has_mesh_collection {
                    app.collection_browser.init_usb_storage(device);
                    // Trigger background metadata preload for instant browsing
                    let _ = app.domain.send_usb_command(
                        mesh_core::usb::UsbCommand::PreloadMetadata {
                            device_path: device.device_path.clone(),
                        }
                    );
                }
            }
        }
        UsbMsg::DeviceConnected(device) => {
            app.collection_browser.add_usb_device(device.clone());
            if device.mount_point.is_some() && device.has_mesh_collection {
                app.collection_browser.init_usb_storage(&device);
                // Trigger background metadata preload for instant browsing
                let _ = app.domain.send_usb_command(
                    mesh_core::usb::UsbCommand::PreloadMetadata {
                        device_path: device.device_path.clone(),
                    }
                );
            }
            app.status = format!("USB: {} connected", device.label);
        }
        UsbMsg::DeviceDisconnected { device_path } => {
            app.collection_browser.remove_usb_device(&device_path);
            app.status = "USB: Device disconnected".to_string();
        }
        UsbMsg::MountComplete { result } => {
            match result {
                Ok(device) => {
                    // Update device in browser and init storage if it has mesh collection
                    app.collection_browser.add_usb_device(device.clone());
                    if device.has_mesh_collection {
                        app.collection_browser.init_usb_storage(&device);
                        // Trigger background metadata preload for instant browsing
                        let _ = app.domain.send_usb_command(
                            mesh_core::usb::UsbCommand::PreloadMetadata {
                                device_path: device.device_path.clone(),
                            }
                        );
                    }
                }
                Err(e) => {
                    log::warn!("USB mount failed: {}", e);
                }
            }
        }
        UsbMsg::MetadataPreloaded { device_path, metadata } => {
            // Metadata now comes from USB's mesh.db, no caching needed
            log::debug!("USB: Metadata preload complete for {} ({} tracks)",
                device_path.display(), metadata.len());
        }
        UsbMsg::MetadataPreloadProgress { device_path, loaded, total } => {
            log::debug!("USB: Preloading metadata {}/{} from {}",
                loaded, total, device_path.display());
        }
        _ => {
            // Ignore export-related messages in mesh-player (read-only)
        }
    }
    Task::none()
}

/// Handle the result of a background suggestion query.
///
/// Converts `Vec<SuggestedTrack>` into `Vec<TrackRow<NodeId>>` with
/// "suggestion:" prefixed IDs, and builds a path lookup map.
pub fn handle_suggestions_ready(
    app: &mut MeshApp,
    result: Arc<Result<Vec<SuggestedTrack>, String>>,
) -> Task<Message> {
    match result.as_ref() {
        Ok(suggested) => {
            let mut tracks = Vec::with_capacity(suggested.len());
            let mut paths = HashMap::new();

            for (i, s) in suggested.iter().enumerate() {
                let track = &s.track;
                let node_id = NodeId(format!("suggestion:{}", track.id.unwrap_or(i as i64)));

                let mut row = TrackRow::new(
                    node_id.clone(),
                    track.name.clone(),
                    i as i32,
                );
                if let Some(ref artist) = track.artist {
                    row = row.with_artist(artist.clone());
                }
                if let Some(bpm) = track.bpm {
                    row = row.with_bpm(bpm);
                }
                if let Some(ref key) = track.key {
                    row = row.with_key(key.clone());
                }
                row = row.with_duration(track.duration_seconds);
                if let Some(lufs) = track.lufs {
                    row = row.with_lufs(lufs);
                }

                paths.insert(node_id, track.path.clone());
                tracks.push(row);
            }

            log::info!("Suggestions ready: {} tracks", tracks.len());
            app.collection_browser.apply_suggestion_results(tracks, paths);
        }
        Err(e) => {
            log::warn!("Suggestion query failed: {}", e);
            app.collection_browser.set_suggestion_loading(false);
            app.status = format!("Suggestions: {}", e);
        }
    }
    Task::none()
}

/// Build and dispatch a background suggestion query from current deck seeds.
///
/// Collects loaded track paths from all decks, then runs the similarity
/// query on a background thread via `Task::perform()`.
pub fn trigger_suggestion_query(app: &MeshApp) -> Task<Message> {
    let seed_paths: Vec<String> = (0..4)
        .filter_map(|i| app.deck_views[i].loaded_track_path().map(String::from))
        .collect();

    if seed_paths.is_empty() {
        return Task::none();
    }

    let db = app.domain.active_db_arc();
    let energy_direction = app.collection_browser.energy_direction();
    let key_model = app.config.display.key_scoring_model;

    Task::perform(
        async move { query_suggestions(&db, seed_paths, energy_direction, key_model, 30, 50) },
        |result| Message::SuggestionsReady(Arc::new(result)),
    )
}
