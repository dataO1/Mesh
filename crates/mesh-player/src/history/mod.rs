//! DJ session history manager
//!
//! Tracks all DJ actions during a session and persists them to all active databases.
//! Maintains in-memory state for live features (suggestion filtering, browser dimming).
//!
//! # Architecture
//!
//! The HistoryManager owns per-deck play state and writes to all active `DatabaseService`
//! instances (local + connected USB sticks). All database writes are fire-and-forget:
//! a single target failure never blocks the others.
//!
//! # Usage
//!
//! ```ignore
//! let mut history = HistoryManager::new(local_db);
//! history.on_track_loaded(0, "/path/to/track.flac", "Artist - Title", Some(42), LoadSource::Browser, None);
//! history.on_play_started(0, 88200, &[0.8, 0.0, 0.0, 0.0]); // deck 0 at sample 88200
//! history.played_paths(); // -> {"Artist - Title"}
//! history.end_session();
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mesh_core::db::{DatabaseService, TrackPlayRecord, TrackPlayUpdate};

// ============================================================================
// Public Types
// ============================================================================

/// Where a track was loaded from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadSource {
    Browser,
    Suggestions,
}

impl LoadSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Suggestions => "suggestions",
        }
    }
}

/// Suggestion metadata captured at track load time
#[derive(Debug, Clone)]
pub struct SuggestionContext {
    pub score: f32,
    pub reason_tags: Vec<(String, Option<String>)>,
    pub energy_direction: f32,
}

// ============================================================================
// Internal State
// ============================================================================

/// Per-deck in-flight play state (created on load, finalized on replacement or session end)
struct DeckPlayState {
    track_path: String,
    track_name: String,
    track_id: Option<i64>,
    loaded_at: i64,
    load_source: LoadSource,
    suggestion_score: Option<f32>,
    suggestion_tags_json: Option<String>,
    suggestion_energy_dir: Option<f32>,
    play_started_at: Option<i64>,
    play_start_sample: Option<i64>,
    hot_cues_used: Vec<u8>,
    loop_was_active: bool,
    played_with: Vec<String>,
}

/// A write target: a database + its collection root (for identification on removal)
struct WriteTarget {
    db: Arc<DatabaseService>,
    root: PathBuf,
}

// ============================================================================
// HistoryManager
// ============================================================================

/// Manages DJ session history and persists actions to all active databases.
///
/// Keeps per-deck play state in memory and writes to all connected databases
/// (local + USB sticks) on each state transition.
pub struct HistoryManager {
    session_id: i64,
    deck_state: [Option<DeckPlayState>; 4],
    played_this_session: HashSet<String>,
    write_targets: Vec<WriteTarget>,
}

impl HistoryManager {
    /// Get the session ID for this history session
    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    /// Create a new session and write the session record to the local database.
    pub fn new(local_db: Arc<DatabaseService>, local_root: PathBuf) -> Self {
        let session_id = now_millis();

        let target = WriteTarget { db: local_db, root: local_root };
        let write_targets = vec![target];

        let manager = Self {
            session_id,
            deck_state: [const { None }; 4],
            played_this_session: HashSet::new(),
            write_targets,
        };
        manager.write_to_all_bg(move |db| {
            if let Err(e) = db.create_session(session_id) {
                log::warn!("[HISTORY] Failed to create session: {e}");
            }
        });
        log::info!("[HISTORY] Session started (id={})", session_id);
        manager
    }

    // ========================================================================
    // Write Target Management
    // ========================================================================

    /// Add a USB database as a write target. Writes the current session record to it.
    pub fn add_write_target(&mut self, db: Arc<DatabaseService>, root: PathBuf) {
        // Avoid duplicates
        if self.write_targets.iter().any(|t| t.root == root) {
            return;
        }
        // Write current session to the new target so it has the full context
        if let Err(e) = db.create_session(self.session_id) {
            log::warn!("[HISTORY] Failed to create session on new target {}: {e}", root.display());
        }
        log::info!("[HISTORY] Added write target: {}", root.display());
        self.write_targets.push(WriteTarget { db, root });
    }

    /// Remove a write target by its collection root path (e.g., on USB disconnect).
    pub fn remove_write_target(&mut self, root: &Path) {
        if let Some(pos) = self.write_targets.iter().position(|t| t.root == root) {
            log::info!("[HISTORY] Removed write target: {}", root.display());
            self.write_targets.remove(pos);
        }
    }

    // ========================================================================
    // Event Handlers
    // ========================================================================

    /// Called when a track is loaded to a deck.
    ///
    /// Finalizes any previous track on this deck, then records the new track load.
    /// Eagerly adds the track to the played set so the browser dims it immediately.
    pub fn on_track_loaded(
        &mut self,
        deck: usize,
        track_path: &str,
        track_name: &str,
        track_id: Option<i64>,
        source: LoadSource,
        suggestion: Option<&SuggestionContext>,
    ) {
        if deck >= 4 { return; }

        // Finalize the previous track on this deck (if any)
        self.finalize_deck(deck);

        let loaded_at = now_millis();

        let suggestion_tags_json = suggestion.map(|s| {
            serde_json::to_string(&s.reason_tags).unwrap_or_default()
        });
        let suggestion_tags_json_copy = suggestion_tags_json.clone();

        let record = TrackPlayRecord {
            session_id: self.session_id,
            loaded_at,
            track_path: track_path.to_string(),
            track_name: track_name.to_string(),
            track_id,
            deck_index: deck as u8,
            load_source: source.as_str().to_string(),
            suggestion_score: suggestion.map(|s| s.score),
            suggestion_tags_json,
            suggestion_energy_dir: suggestion.map(|s| s.energy_direction),
        };

        self.write_to_all_bg(move |db| {
            if let Err(e) = db.insert_track_play(&record) {
                log::warn!("[HISTORY] Failed to insert track play: {e}");
            }
        });

        // Eagerly add to played set so browser dims immediately on load
        self.played_this_session.insert(track_path.to_string());

        self.deck_state[deck] = Some(DeckPlayState {
            track_path: track_path.to_string(),
            track_name: track_name.to_string(),
            track_id,
            loaded_at,
            load_source: source,
            suggestion_score: suggestion.map(|s| s.score),
            suggestion_tags_json: suggestion_tags_json_copy,
            suggestion_energy_dir: suggestion.map(|s| s.energy_direction),
            play_started_at: None,
            play_start_sample: None,
            hot_cues_used: Vec::new(),
            loop_was_active: false,
            played_with: Vec::new(),
        });

        log::info!(
            "[HISTORY] Track loaded: deck={}, name=\"{}\", source={:?}",
            deck, track_name, source
        );
    }

    /// Called when the DJ presses play on a deck (first play after load).
    ///
    /// Records play start time, sample position, and bidirectionally updates
    /// `played_with` for all co-playing decks.
    pub fn on_play_started(&mut self, deck: usize, position_samples: u64, channel_volumes: &[f32; 4]) {
        if deck >= 4 { return; }

        // Only record the first play after load
        if self.deck_state[deck].as_ref().map_or(true, |s| s.play_started_at.is_some()) {
            return;
        }

        let now = now_millis();

        // Collect co-playing tracks: other decks with play_started, volume > 0
        let co_playing: Vec<(usize, String)> = (0..4)
            .filter(|&d| d != deck && channel_volumes[d] > 0.0)
            .filter_map(|d| {
                self.deck_state[d].as_ref()
                    .filter(|s| s.play_started_at.is_some())
                    .map(|s| (d, s.track_name.clone()))
            })
            .collect();

        let played_with_names: Vec<String> = co_playing.iter().map(|(_, n)| n.clone()).collect();
        let played_with_json = if played_with_names.is_empty() {
            None
        } else {
            serde_json::to_string(&played_with_names).ok()
        };

        // Update this deck's state
        let state = self.deck_state[deck].as_mut().unwrap();
        state.play_started_at = Some(now);
        state.play_start_sample = Some(position_samples as i64);
        state.played_with = played_with_names;

        let loaded_at = state.loaded_at;
        let session_id = self.session_id;

        // Write play_started to all DBs (background — never block UI)
        let position_i64 = position_samples as i64;
        self.write_to_all_bg(move |db| {
            if let Err(e) = db.update_play_started(
                session_id, loaded_at, now, position_i64, played_with_json.clone(),
            ) {
                log::warn!("[HISTORY] Failed to update play_started: {e}");
            }
        });

        // Bidirectional: add this track to each co-player's played_with
        let new_track_name = self.deck_state[deck].as_ref().unwrap().track_name.clone();
        for (co_deck, _) in &co_playing {
            if let Some(co_state) = self.deck_state[*co_deck].as_mut() {
                if !co_state.played_with.contains(&new_track_name) {
                    co_state.played_with.push(new_track_name.clone());
                    let co_loaded_at = co_state.loaded_at;
                    let updated_json = serde_json::to_string(&co_state.played_with).ok();
                    self.write_to_all_bg(move |db| {
                        if let Err(e) = db.update_played_with(session_id, co_loaded_at, updated_json.clone()) {
                            log::warn!("[HISTORY] Failed to update co-player played_with: {e}");
                        }
                    });
                }
            }
        }

        log::info!(
            "[HISTORY] Play started: deck={}, sample={}, played_with={:?}",
            deck, position_samples, self.deck_state[deck].as_ref().unwrap().played_with
        );
    }

    /// Record a hot cue press (accumulates unique slot indices).
    pub fn on_hot_cue_pressed(&mut self, deck: usize, slot: u8) {
        if deck >= 4 { return; }
        if let Some(state) = self.deck_state[deck].as_mut() {
            if !state.hot_cues_used.contains(&slot) {
                state.hot_cues_used.push(slot);
            }
        }
    }

    /// Mark that a loop was active at some point during this track's play.
    pub fn on_loop_observed(&mut self, deck: usize) {
        if deck >= 4 { return; }
        if let Some(state) = self.deck_state[deck].as_mut() {
            state.loop_was_active = true;
        }
    }

    /// Get the set of track paths played this session (for suggestion filtering and browser dimming).
    pub fn played_paths(&self) -> &HashSet<String> {
        &self.played_this_session
    }

    /// End the current session: finalize all active decks and write session end timestamp.
    ///
    /// Uses synchronous writes since this is called from Drop — data must be
    /// flushed before the process exits.
    pub fn end_session(&mut self) {
        for deck in 0..4 {
            self.finalize_deck_sync(deck);
        }
        let ended_at = now_millis();
        let session_id = self.session_id;
        self.write_to_all_sync(|db| {
            if let Err(e) = db.end_session(session_id, ended_at) {
                log::warn!("[HISTORY] Failed to end session: {e}");
            }
        });
        log::info!(
            "[HISTORY] Session ended (id={}, tracks_played={})",
            self.session_id, self.played_this_session.len()
        );
    }

    // ========================================================================
    // Internal Helpers
    // ========================================================================

    /// Finalize the current track on a deck: compute play duration and write to all DBs.
    /// Uses background thread for writes (called during normal operation).
    fn finalize_deck(&mut self, deck: usize) {
        self.finalize_deck_inner(deck, false);
    }

    /// Finalize synchronously (called during shutdown to ensure data is flushed).
    fn finalize_deck_sync(&mut self, deck: usize) {
        self.finalize_deck_inner(deck, true);
    }

    fn finalize_deck_inner(&mut self, deck: usize, sync: bool) {
        let state = match self.deck_state[deck].take() {
            Some(s) => s,
            None => return,
        };

        let now = now_millis();
        let seconds_played = state.play_started_at.map(|started| {
            (now - started) as f32 / 1000.0
        });

        let hot_cues_json = if state.hot_cues_used.is_empty() {
            None
        } else {
            serde_json::to_string(&state.hot_cues_used).ok()
        };

        let update = TrackPlayUpdate {
            play_started_at: state.play_started_at,
            play_start_sample: state.play_start_sample,
            play_ended_at: Some(now),
            seconds_played,
            hot_cues_used_json: hot_cues_json,
            loop_was_active: state.loop_was_active,
            played_with_json: if state.played_with.is_empty() {
                None
            } else {
                serde_json::to_string(&state.played_with).ok()
            },
        };

        let session_id = self.session_id;
        let loaded_at = state.loaded_at;
        if sync {
            self.write_to_all_sync(|db| {
                if let Err(e) = db.finalize_track_play(session_id, loaded_at, &update) {
                    log::warn!("[HISTORY] Failed to finalize track play: {e}");
                }
            });
        } else {
            self.write_to_all_bg(move |db| {
                if let Err(e) = db.finalize_track_play(session_id, loaded_at, &update) {
                    log::warn!("[HISTORY] Failed to finalize track play: {e}");
                }
            });
        }

        log::debug!(
            "[HISTORY] Finalized deck {}: \"{}\" ({:.1?}s played)",
            deck, state.track_name, seconds_played
        );
    }

    /// Write an operation to all active databases on a background thread.
    ///
    /// Fire-and-forget: spawns a thread per call so the UI thread is never blocked
    /// by CozoDB write locks or USB I/O. The `Arc<DatabaseService>` write lock
    /// serializes concurrent writes per-database automatically.
    fn write_to_all_bg(&self, f: impl Fn(&DatabaseService) + Send + 'static) {
        let targets: Vec<Arc<DatabaseService>> = self.write_targets
            .iter()
            .map(|t| t.db.clone())
            .collect();
        std::thread::spawn(move || {
            mesh_core::rt::pin_to_big_cores();
            for db in &targets {
                f(db);
            }
        });
    }

    /// Write an operation to all active databases synchronously.
    ///
    /// Used only during shutdown (Drop) to ensure data is flushed before exit.
    fn write_to_all_sync(&self, f: impl Fn(&DatabaseService)) {
        for target in &self.write_targets {
            f(&target.db);
        }
    }
}

/// Current time in milliseconds since Unix epoch.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
