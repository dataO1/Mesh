//! Browser and USB message handler
//!
//! Handles collection browser navigation, track selection, and USB device events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use iced::Task;

use mesh_core::playlist::NodeId;
use mesh_core::usb::UsbMessage as UsbMsg;
use mesh_widgets::{parse_hex_color, scroll_to_centered_selection, TrackRow, TrackTag};
use mesh_core::types::PlayState;
use crate::suggestions::{query_suggestions, DbSource, SuggestedTrack};
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
                // Just turned on — trigger query if seeds available
                if active_seed_paths(app).is_empty() {
                    log::info!("Suggestions enabled but no audible seeds (need playing deck with volume > 0)");
                    app.collection_browser.set_suggestion_loading(false);
                } else {
                    app.collection_browser.set_suggestion_loading(true);
                    return trigger_suggestion_query(app);
                }
            }
            return Task::none();
        }
        CollectionBrowserMessage::RefreshSuggestions => {
            if app.collection_browser.is_suggestions_enabled() {
                if !active_seed_paths(app).is_empty() {
                    app.collection_browser.set_suggestion_loading(true);
                    return trigger_suggestion_query(app);
                }
            }
            return Task::none();
        }
        CollectionBrowserMessage::SetEnergyDirection(value) => {
            // Auto-enable suggestions when energy direction is adjusted
            if !app.collection_browser.is_suggestions_enabled() {
                app.collection_browser.set_suggestions_enabled(true);
            }
            let changed = app.collection_browser.set_energy_direction(*value);
            if changed {
                // Trailing-edge debounce: bump generation and start a timer.
                // Only the last timer (matching current gen) will fire the query.
                app.energy_debounce_gen = app.energy_debounce_gen.wrapping_add(1);
                let gen = app.energy_debounce_gen;
                app.collection_browser.set_suggestion_loading(true);
                return Task::perform(
                    async move { tokio::time::sleep(std::time::Duration::from_millis(300)).await },
                    move |_| Message::CheckEnergyDebounce(gen),
                );
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

                // Convert suggestion reason tags to UI TrackTags
                if !s.reason_tags.is_empty() {
                    let tags: Vec<TrackTag> = s.reason_tags.iter().map(|(label, color)| {
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

/// Collect the current set of active seed paths (playing + volume > 0).
///
/// A deck contributes as a seed only if it has a loaded track, is playing,
/// and its mixer volume is above zero. This prevents silent or paused decks
/// from influencing suggestions.
pub fn active_seed_paths(app: &MeshApp) -> Vec<String> {
    (0..4)
        .filter(|&i| {
            app.deck_views[i].play_state() == PlayState::Playing
                && app.mixer_view.channel_volume(i) > 0.0
        })
        .filter_map(|i| app.deck_views[i].loaded_track_path().map(String::from))
        .collect()
}

/// Build and dispatch a background suggestion query from current deck seeds.
///
/// Only decks that are loaded, playing, and have volume > 0 are used as seeds.
/// This ensures suggestions reflect what the audience is actually hearing.
///
/// Collects all available database sources (local + mounted USBs) so the
/// suggestion engine can search across all libraries simultaneously.
pub fn trigger_suggestion_query(app: &MeshApp) -> Task<Message> {
    let seed_paths = active_seed_paths(app);

    if seed_paths.is_empty() {
        return Task::none();
    }

    // Collect all available database sources: local collection + all mounted USBs
    let mut sources = vec![DbSource {
        db: app.domain.local_db_arc(),
        collection_root: app.domain.local_collection_path().to_path_buf(),
        name: "Local".to_string(),
    }];
    for (_, usb_storage) in &app.collection_browser.usb_storages {
        if let Some(db) = usb_storage.db() {
            sources.push(DbSource {
                db: db.clone(),
                collection_root: usb_storage.collection_root().clone(),
                name: usb_storage.device().label.clone(),
            });
        }
    }

    let energy_direction = app.collection_browser.energy_direction();
    let key_model = app.config.display.key_scoring_model;

    Task::perform(
        async move { query_suggestions(&sources, seed_paths, energy_direction, key_model, 10_000, 30) },
        |result| Message::SuggestionsReady(Arc::new(result)),
    )
}

/// Handle `ScheduleSuggestionRefresh`: start debounce timer if not already pending.
///
/// Called from event handlers (play/pause, volume threshold, track load) when the
/// active seed set may have changed. Only one timer runs at a time — if a refresh
/// is already pending, additional events are coalesced into the same window.
pub fn schedule_suggestion_refresh(app: &mut MeshApp) -> Task<Message> {
    if app.suggestion_refresh_pending || !app.collection_browser.is_suggestions_enabled() {
        return Task::none();
    }
    app.suggestion_refresh_pending = true;
    Task::perform(
        async { tokio::time::sleep(std::time::Duration::from_secs(1)).await },
        |_| Message::CheckSuggestionSeeds,
    )
}

/// Handle `CheckSuggestionSeeds`: debounce timer expired, compute seeds and retrigger.
///
/// Compares the current active seed set against the last query's seeds.
/// Only dispatches a new suggestion query if the set actually changed.
pub fn check_suggestion_seeds(app: &mut MeshApp) -> Task<Message> {
    app.suggestion_refresh_pending = false;

    if !app.collection_browser.is_suggestions_enabled() {
        return Task::none();
    }

    let current_seeds = active_seed_paths(app);
    if app.collection_browser.update_seed_paths(current_seeds) {
        app.collection_browser.set_suggestion_loading(true);
        trigger_suggestion_query(app)
    } else {
        Task::none()
    }
}

/// Handle `CheckEnergyDebounce`: trailing-edge debounce for energy direction slider.
///
/// Each fader movement bumps a generation counter and starts a 300ms timer carrying
/// that generation. When the timer fires, if no newer movement has occurred (gen matches),
/// the fader has stopped — fire the suggestion query.
pub fn check_energy_debounce(app: &mut MeshApp, gen: u64) -> Task<Message> {
    if gen != app.energy_debounce_gen {
        // A newer movement happened — this timer is stale, ignore it
        return Task::none();
    }

    if !app.collection_browser.is_suggestions_enabled() {
        app.collection_browser.set_suggestion_loading(false);
        return Task::none();
    }

    trigger_suggestion_query(app)
}
