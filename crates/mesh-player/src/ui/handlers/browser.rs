//! Browser and USB message handler
//!
//! Handles collection browser navigation, track selection, and USB device events.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use iced::Task;

use mesh_core::playlist::NodeId;
use mesh_core::usb::UsbMessage as UsbMsg;
use mesh_widgets::{parse_hex_color, scroll_to_centered_selection, TrackRow, TrackTag};
use mesh_core::types::PlayState;
use crate::history::SuggestionContext;
use crate::suggestions::{query_suggestions, DbSource, SplitSuggestions, SuggestedTrack};
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
        CollectionBrowserMessage::OpenSearch => {
            let current = app.collection_browser.browser.table_state.search_query.clone();
            app.keyboard.open("Search tracks...", false);
            app.keyboard.text = current; // pre-fill with existing query
            app.keyboard_for_search = true;
            return Task::none();
        }
        _ => {}
    }

    // Check if this is a scroll message (for auto-scroll after)
    let is_scroll = matches!(browser_msg, CollectionBrowserMessage::ScrollBy(_));

    // Handle collection browser message and check if we need to load a track
    if let Some((deck_idx, path)) = app.collection_browser.handle_message(browser_msg) {
        let path_str = path.to_string_lossy().to_string();
        // If track came from the suggestion panel, capture suggestion metadata
        let suggestion_ctx = app.collection_browser.get_suggestion_context(&path_str);
        return app.update(Message::LoadTrack(deck_idx, path_str, suggestion_ctx));
    }

    // Sync domain layer with collection browser's active storage.
    // Note: load_track_metadata() resolves the correct database from the track
    // path itself, so this sync is not required for metadata correctness. But it
    // keeps active_storage in sync for other consumers (e.g. active_collection_path).
    if let Some((usb_idx, collection_path)) = app.collection_browser.get_active_usb_info() {
        // Always call switch_to_usb — it handles same-stick no-ops internally,
        // and we need it to fire for USB→USB switches (different sticks).
        if let Err(e) = app.domain.switch_to_usb(usb_idx, &collection_path) {
            log::error!("Failed to switch domain to USB: {}", e);
        }
    } else if app.domain.is_browsing_usb() {
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
            let has_usb = devices.iter().any(|d| d.mount_point.is_some() && d.has_mesh_collection);
            app.collection_browser.update_usb_devices(devices.clone());
            // Initialize storages for mounted devices and trigger metadata preload
            for device in &devices {
                if device.mount_point.is_some() && device.has_mesh_collection {
                    app.collection_browser.init_usb_storage(device);
                    // Register USB database as history write target
                    register_usb_history_target(app, &device.device_path);
                    // Trigger background metadata preload for instant browsing
                    let _ = app.domain.send_usb_command(
                        mesh_core::usb::UsbCommand::PreloadMetadata {
                            device_path: device.device_path.clone(),
                        }
                    );
                }
            }
            // Rebuild graph to include USB sources if any were found
            if has_usb {
                return Task::done(Message::RebuildGraph);
            }
        }
        UsbMsg::DeviceConnected(device) => {
            app.collection_browser.add_usb_device(device.clone());
            if device.mount_point.is_some() && device.has_mesh_collection {
                app.collection_browser.init_usb_storage(&device);
                // Register USB database as history write target
                register_usb_history_target(app, &device.device_path);
                // Trigger background metadata preload for instant browsing
                let _ = app.domain.send_usb_command(
                    mesh_core::usb::UsbCommand::PreloadMetadata {
                        device_path: device.device_path.clone(),
                    }
                );
            }
            app.status = format!("USB: {} connected", device.label);
            // Rebuild graph to include the new USB source
            return Task::done(Message::RebuildGraph);
        }
        UsbMsg::DeviceDisconnected { device_path } => {
            // Clear cached database for this USB device
            let collection_root = device_path.join("mesh-collection");
            mesh_core::usb::cache::clear_usb_database(&collection_root);

            // Remove USB database from history write targets
            app.history.remove_write_target(&collection_root);

            app.collection_browser.remove_usb_device(&device_path);
            app.status = "USB: Device disconnected".to_string();
            // Rebuild graph without the removed source
            return Task::done(Message::RebuildGraph);
        }
        UsbMsg::MountComplete { result } => {
            match result {
                Ok(device) => {
                    // Update device in browser and init storage if it has mesh collection
                    app.collection_browser.add_usb_device(device.clone());
                    if device.has_mesh_collection {
                        app.collection_browser.init_usb_storage(&device);
                        // Register USB database as history write target
                        register_usb_history_target(app, &device.device_path);
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
/// Converts `SplitSuggestions` into `Vec<TrackRow<NodeId>>` with:
/// - Playlist suggestions first (blue playlist-name tag, no row tint)
/// - Global suggestions second (tinted background row, no playlist tag)
pub fn handle_suggestions_ready(
    app: &mut MeshApp,
    result: Arc<Result<SplitSuggestions, String>>,
) -> Task<Message> {
    match result.as_ref() {
        Ok(split) => {
            let capacity = split.playlist_suggestions.len() + split.global_suggestions.len();
            let mut tracks = Vec::with_capacity(capacity);
            let mut paths = HashMap::new();
            let mut contexts = HashMap::new();
            let energy_direction = app.collection_browser.energy_direction();

            // Playlist suggestions — prepend per-track playlist pills, no row tint
            for (i, s) in split.playlist_suggestions.iter().enumerate() {
                let reason_tags = prepend_playlist_pills(s);
                let row = suggestion_to_row(s, i, reason_tags, false, energy_direction, &mut paths, &mut contexts);
                tracks.push(row);
            }

            // Global suggestions — same per-track playlist pills, visually tinted row
            let has_playlist_section = !split.playlist_suggestions.is_empty();
            let offset = split.playlist_suggestions.len();
            for (i, s) in split.global_suggestions.iter().enumerate() {
                let reason_tags = prepend_playlist_pills(s);
                let row = suggestion_to_row(s, offset + i, reason_tags, has_playlist_section, energy_direction, &mut paths, &mut contexts);
                tracks.push(row);
            }

            log::info!("Suggestions ready: {} playlist + {} global",
                split.playlist_suggestions.len(), split.global_suggestions.len());
            app.collection_browser.apply_suggestion_results(tracks, paths, contexts);

            // Update graph highlighting with suggestion data
            // Resolve seed track IDs via DB path lookup (before mutable graph borrow)
            let seed_paths_for_graph = active_seed_paths(app);
            let db = app.collection_browser.db_service_arc();
            let seed_ids: Vec<i64> = seed_paths_for_graph.iter()
                .filter_map(|p| {
                    db.get_track_by_path(p).ok().flatten().and_then(|t| t.id)
                })
                .collect();

            if let Some(ref mut graph) = app.collection_browser.canvas_state.graph {
                graph.suggestion_ids.clear();
                graph.suggestion_scores.clear();
                graph.suggestion_edges.clear();

                // Collect top-30 suggestion IDs and scores
                let all_suggestions: Vec<&SuggestedTrack> = split.playlist_suggestions.iter()
                    .chain(split.global_suggestions.iter())
                    .collect();

                for s in all_suggestions.iter().take(30) {
                    if let Some(id) = s.track.id {
                        graph.suggestion_ids.insert(id);
                        graph.suggestion_scores.insert(id, s.score);
                        // Build edges from each seed to this suggestion
                        for &seed_id in &seed_ids {
                            graph.suggestion_edges.push((seed_id, id, s.score));
                        }
                    }
                }

                // Update seed stack — include all active seeds (multiple playing decks)
                if !seed_ids.is_empty() {
                    graph.seed_stack = seed_ids.clone();
                    graph.seed_position = 0;
                }

                graph.clear_caches();
            }
        }
        Err(e) => {
            log::warn!("Suggestion query failed: {}", e);
            app.collection_browser.set_suggestion_loading(false);
            app.status = format!("Suggestions: {}", e);
        }
    }
    Task::none()
}

/// Build reason tags for a suggestion, prepending a blue pill for each playlist the track belongs to.
fn prepend_playlist_pills(s: &SuggestedTrack) -> Vec<(String, Option<String>)> {
    let mut tags = s.reason_tags.clone();
    // Insert playlist pills at the front (in order)
    for (i, name) in s.playlists.iter().enumerate() {
        tags.insert(i, (name.clone(), Some(mesh_core::suggestions::scoring::TAG_COLOR_SOURCE.to_string())));
    }
    tags
}

/// Convert a `SuggestedTrack` into a `TrackRow<NodeId>`, populating the path and context maps.
fn suggestion_to_row(
    s: &SuggestedTrack,
    idx: usize,
    reason_tags: Vec<(String, Option<String>)>,
    is_global: bool,
    energy_direction: f32,
    paths: &mut HashMap<NodeId, PathBuf>,
    contexts: &mut HashMap<String, SuggestionContext>,
) -> TrackRow<NodeId> {
    let track = &s.track;
    let node_id = NodeId(format!("suggestion:{}", track.id.unwrap_or(idx as i64)));
    let mut row = TrackRow::new(node_id.clone(), track.title.clone(), idx as i32);
    if let Some(ref artist) = track.artist { row = row.with_artist(artist.clone()); }
    if let Some(bpm) = track.bpm { row = row.with_bpm(bpm); }
    if let Some(ref key) = track.key { row = row.with_key(key.clone()); }
    row = row.with_duration(track.duration_seconds);
    if let Some(lufs) = track.lufs { row = row.with_lufs(lufs); }

    if !reason_tags.is_empty() {
        let tags: Vec<TrackTag> = reason_tags.iter().map(|(label, color)| {
            let mut tag = TrackTag::new(label);
            if let Some(hex) = color {
                if let Some(c) = parse_hex_color(hex) { tag = tag.with_color(c); }
            }
            tag
        }).collect();
        row = row.with_tags(tags);
    }

    if is_global { row = row.with_global_suggestion(true); }
    if s.is_proven_followup { row = row.with_proven_followup(true); }

    let track_path_str = track.path.to_string_lossy().to_string();
    row.track_path = Some(track_path_str.clone());
    contexts.insert(track_path_str, SuggestionContext {
        score: s.score,
        reason_tags: s.reason_tags.clone(),
        energy_direction,
    });
    paths.insert(node_id, track.path.clone());
    row
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

/// When bias is high (|bias| > 0.6) and multiple decks are playing, seed suggestions from the
/// deck most likely to stay (highest mixer volume). The next track needs to work with what's
/// staying, not what's being faded out.
fn staying_seed_path(app: &MeshApp, seed_paths: &[String]) -> Vec<String> {
    if seed_paths.len() <= 1 { return seed_paths.to_vec(); }
    let best = (0..4)
        .filter(|&i| {
            app.deck_views[i].loaded_track_path()
                .map(|p| seed_paths.contains(&p.to_string()))
                .unwrap_or(false)
        })
        .max_by(|&a, &b| {
            app.mixer_view.channel_volume(a)
                .partial_cmp(&app.mixer_view.channel_volume(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    best.and_then(|i| app.deck_views[i].loaded_track_path().map(|p| vec![p.to_string()]))
        .unwrap_or_else(|| seed_paths.to_vec())
}

/// When the slider is near center (|bias| < 0.3) and 2+ decks are playing, compute an
/// averaged embedding so the HNSW query finds tracks that bridge both sonic spaces.
/// Prefers PCA-128 vectors; falls back to ML-1280. Returns None if fewer than 2 available.
fn compute_blend_vec(app: &MeshApp, seed_paths: &[String], bias_abs: f32) -> Option<Vec<f64>> {
    if seed_paths.len() < 2 || bias_abs > 0.6 { return None; }

    let local_db = app.domain.local_db_arc();

    let vecs: Vec<Vec<f64>> = seed_paths.iter()
        .filter_map(|path| {
            local_db.get_track_by_path(path).ok().flatten()
                .and_then(|t| t.id)
                .and_then(|id| {
                    local_db.get_pca_embedding_raw(id).ok().flatten()
                        .or_else(|| local_db.get_ml_embedding_raw(id).ok().flatten())
                        .map(|v| v.iter().map(|&x| x as f64).collect::<Vec<f64>>())
                })
        })
        .collect();

    if vecs.len() < 2 { return None; }

    let dim = vecs[0].len();
    let avg: Vec<f64> = (0..dim)
        .map(|i| vecs.iter().map(|v| v[i]).sum::<f64>() / vecs.len() as f64)
        .collect();

    // L2-normalise: geodesic midpoint on the unit hypersphere
    let norm = avg.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm < 1e-9 { return None; }
    let blend: Vec<f64> = avg.iter().map(|x| x / norm).collect();

    log::debug!(
        "[SUGGESTIONS] dual-deck blend: {} vecs averaged (dim={}, bias_abs={:.2})",
        vecs.len(), dim, bias_abs
    );
    Some(blend)
}

/// Build and dispatch a background suggestion query from current deck seeds.
///
/// When no decks are playing, delegates to opener mode which scores candidates
/// on-the-fly by intro quality. When 2+ decks are playing:
/// - Slider near center (|bias| < 0.3): blend mode — averaged embedding query
/// - Slider at edge (|bias| > 0.6): staying mode — seed from highest-volume deck
pub fn trigger_suggestion_query(app: &mut MeshApp) -> Task<Message> {
    let seed_paths = active_seed_paths(app);
    let energy_direction = app.collection_browser.energy_direction();
    let energy_bias = (energy_direction - 0.5) * 2.0;
    let bias_abs = energy_bias.abs();

    // Compute blend vec before any seed mutation (needs all seed paths)
    let blend_query_vec = compute_blend_vec(app, &seed_paths, bias_abs);

    // Biased with multiple decks: reduce to the staying deck
    let seed_paths = if bias_abs > 0.6 && seed_paths.len() > 1 {
        let staying = staying_seed_path(app, &seed_paths);
        log::debug!("[SUGGESTIONS] biased mode: reduced to staying deck {:?}", staying);
        staying
    } else {
        seed_paths
    };

    // Opener mode (no playing decks) is handled inside query_suggestions — don't bail here

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

    let key_model = app.config.display.key_scoring_model;
    let played = app.history.played_paths().clone();

    // Build suggestion algorithm config from display settings
    let suggestion_config = crate::suggestions::SuggestionConfig::from_display(
        app.config.display.suggestion_blend_mode,
        app.config.display.suggestion_key_filter,
        app.config.display.suggestion_stem_complement,
        app.config.display.suggestion_transition_reach,
    );

    // Snapshot current playlist context for the split logic
    let playlist_paths = app.collection_browser.playlist_track_paths().map(|(p, _)| p);
    let playlist_split = app.config.display.suggestion_playlist_split;

    Task::perform(
        async move {
            // Fetch more candidates than we need so both halves have enough after the split
            // No pre-split truncation — carry all scored candidates into split_suggestions
            // so each bucket independently picks its best 15 from the full pool.
            // Playlist tracks get a lenient key threshold via preferred_paths.
            let mut all = query_suggestions(&sources, seed_paths, energy_direction, key_model, suggestion_config, 10_000, usize::MAX, &played, playlist_paths.as_ref(), blend_query_vec, false)?;

            // Attach per-track playlist memberships via a single reverse-lookup query per source
            for src in &sources {
                match src.db.get_all_playlist_memberships() {
                    Ok(memberships) => {
                        for s in &mut all {
                            if let Some(id) = s.track.id {
                                if s.track.path.starts_with(&src.collection_root) {
                                    if let Some(names) = memberships.get(&id) {
                                        s.playlists = names.clone();
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => log::warn!("Failed to load playlist memberships: {}", e),
                }
            }

            let (playlist_suggestions, global_suggestions) =
                split_suggestions(all, if playlist_split { playlist_paths.as_ref() } else { None });
            Ok(SplitSuggestions { playlist_suggestions, global_suggestions })
        },
        |result| Message::SuggestionsReady(Arc::new(result)),
    )
}

/// Partition a flat suggestion list into (playlist-local, global) buckets.
///
/// Tracks whose absolute path is in `playlist_paths` go into the first bucket
/// (capped at 15); the rest fill the second bucket up to a combined total of 30.
/// If no playlist filter is provided, all results are returned as global (no split).
fn split_suggestions(
    all: Vec<SuggestedTrack>,
    playlist_paths: Option<&HashSet<String>>,
) -> (Vec<SuggestedTrack>, Vec<SuggestedTrack>) {
    const PLAYLIST_CAP: usize = 15;
    const TOTAL: usize = 30;

    let Some(paths) = playlist_paths else {
        let mut global = all;
        global.truncate(TOTAL);
        return (vec![], global);
    };

    let (mut playlist, mut global): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|s| {
            let p = s.track.path.to_string_lossy();
            paths.contains(p.as_ref())
        });

    playlist.truncate(PLAYLIST_CAP);
    let global_cap = TOTAL.saturating_sub(playlist.len());
    global.truncate(global_cap);
    (playlist, global)
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

/// Register a USB device's database as a history write target.
///
/// Looks up the USB storage by device path, and if it has a DB, adds it
/// as a write target so session history is persisted to the USB stick.
fn register_usb_history_target(app: &mut MeshApp, device_path: &PathBuf) {
    if let Some((_, usb_storage)) = app.collection_browser.usb_storages
        .iter()
        .find(|(path, _)| path == device_path)
    {
        if let Some(db) = usb_storage.db() {
            app.history.add_write_target(
                db.clone(),
                usb_storage.collection_root().clone(),
            );
        }
    }
}
