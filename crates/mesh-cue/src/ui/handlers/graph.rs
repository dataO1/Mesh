//! Graph view message handlers
//!
//! Handles graph edge building, layout (radial score-based), seed selection,
//! energy slider changes, and breadcrumb navigation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use iced::Task;
use mesh_core::suggestions::query::GraphEdge;
use mesh_widgets::graph_view::{GraphViewState, TrackMeta};
use mesh_widgets::graph_view::layout;

use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::state::BrowserTab;

/// Number of top suggestions to highlight (matching mesh-player)
const SUGGESTION_HIGHLIGHT_LIMIT: usize = 30;

impl MeshCueApp {
    /// Switch between List and Graph browser tabs.
    pub fn handle_set_browser_tab(&mut self, tab: BrowserTab) -> Task<Message> {
        self.collection.active_tab = tab;
        if tab == BrowserTab::Graph {
            if self.collection.graph_state.is_none() && !self.collection.graph_building {
                return self.handle_build_graph_edges();
            }
        }
        Task::none()
    }

    /// Kick off background: load track metadata and build initial graph state.
    pub fn handle_build_graph_edges(&mut self) -> Task<Message> {
        if self.collection.graph_building {
            return Task::none();
        }
        self.collection.graph_building = true;

        let db = self.domain.db_arc();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    log::info!("[GRAPH] Building graph edges (k=15)...");
                    let edges = db.build_graph_edges(15)
                        .map_err(|e| format!("Graph edge build failed: {e}"))?;
                    log::info!("[GRAPH] Built {} edges", edges.len());
                    Ok::<_, String>(Arc::new(edges))
                })
                .await
                .map_err(|e| format!("Task panicked: {e}"))?
            },
            |result| match result {
                Ok(edges) => Message::GraphEdgesReady(edges),
                Err(e) => {
                    log::error!("[GRAPH] Edge build failed: {}", e);
                    Message::GraphEdgesReady(Arc::new(Vec::new()))
                }
            },
        )
    }

    /// Graph edges built — create GraphViewState with metadata.
    /// No layout yet — nodes are positioned when a seed is selected.
    pub fn handle_graph_edges_ready(&mut self, edges: Arc<Vec<GraphEdge>>) -> Task<Message> {
        self.collection.graph_building = false;
        self.collection.graph_edges = Some(edges);

        let db = self.domain.db_arc();
        let all_tracks = db.get_all_tracks().unwrap_or_default();

        let mut track_meta = HashMap::new();
        let mut node_ids = Vec::new();
        for track in &all_tracks {
            if let Some(id) = track.id {
                node_ids.push(id);
                track_meta.insert(id, TrackMeta {
                    id,
                    title: track.path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    artist: track.artist.clone(),
                    key: track.key.clone(),
                    bpm: track.bpm,
                });
            }
        }

        // Random initial positions (no seed yet — overview)
        let positions: HashMap<i64, (f32, f32)> = {
            use std::f32::consts::PI;
            let golden = PI * (3.0 - 5.0_f32.sqrt());
            node_ids.iter().enumerate().map(|(i, &id)| {
                let r = ((i as f32) / (node_ids.len() as f32)).sqrt() * 0.8;
                let a = i as f32 * golden;
                (id, (r * a.cos(), r * a.sin()))
            }).collect()
        };

        let mut state = GraphViewState::new();
        state.positions = positions;
        state.track_meta = track_meta;

        self.collection.graph_state = Some(state);
        Task::none()
    }

    /// Layout tick — update positions from background task.
    pub fn handle_graph_layout_tick(&mut self, positions: Vec<(i64, f32, f32)>) -> Task<Message> {
        if let Some(ref mut state) = self.collection.graph_state {
            for &(id, x, y) in &positions {
                state.positions.insert(id, (x, y));
            }
            state.clear_caches();
        }
        Task::none()
    }

    /// Select a node as seed — push to breadcrumb stack and query all tracks.
    pub fn handle_graph_seed_selected(&mut self, track_id: i64) -> Task<Message> {
        let state = match self.collection.graph_state.as_mut() {
            Some(s) => s,
            None => return Task::none(),
        };

        if state.seed_stack.last() != Some(&track_id) {
            state.seed_stack.push(track_id);
            if state.seed_stack.len() > 20 {
                state.seed_stack.remove(0);
            }
        }

        self.run_graph_suggestion_query(track_id)
    }

    /// Navigate back in seed history.
    pub fn handle_graph_seed_back(&mut self) -> Task<Message> {
        let state = match self.collection.graph_state.as_mut() {
            Some(s) => s,
            None => return Task::none(),
        };

        if state.seed_stack.len() <= 1 {
            state.seed_stack.clear();
            state.suggestion_ids.clear();
            state.suggestion_scores.clear();
            state.suggestion_edges.clear();
            state.clear_caches();
            return Task::none();
        }

        state.seed_stack.pop();
        let prev = *state.seed_stack.last().unwrap();
        self.run_graph_suggestion_query(prev)
    }

    /// Node hover changed.
    pub fn handle_graph_node_hovered(&mut self, id: Option<i64>) -> Task<Message> {
        if let Some(ref mut state) = self.collection.graph_state {
            state.hovered_id = id;
            state.node_cache.clear();
        }
        Task::none()
    }

    /// Energy direction slider changed — debounced re-query.
    pub fn handle_graph_slider_changed(&mut self, value: f32) -> Task<Message> {
        if let Some(ref mut state) = self.collection.graph_state {
            state.energy_direction = value;
        }

        // Debounce: if a query is already in flight, just update the value.
        // The query result handler checks if the slider moved and re-queries.
        if self.collection.graph_building {
            return Task::none();
        }

        let current_seed = self.collection.graph_state.as_ref()
            .and_then(|s| s.seed_stack.last().copied());

        match current_seed {
            Some(seed_id) => self.run_graph_suggestion_query(seed_id),
            None => Task::none(),
        }
    }

    /// Pan/zoom changed.
    pub fn handle_graph_pan_zoom(&mut self, pan: (f32, f32), zoom: f32) -> Task<Message> {
        if let Some(ref mut state) = self.collection.graph_state {
            state.pan = pan;
            state.zoom = zoom;
            state.clear_caches();
        }
        Task::none()
    }

    /// Internal: run suggestion query for a seed, scoring ALL tracks.
    fn run_graph_suggestion_query(&mut self, seed_id: i64) -> Task<Message> {
        // Mark query in flight for debounce
        self.collection.graph_building = true;

        let db = self.domain.db_arc();
        let state = match self.collection.graph_state.as_ref() {
            Some(s) => s,
            None => {
                self.collection.graph_building = false;
                return Task::none();
            }
        };
        let energy_direction = state.energy_direction;
        let seed_path = db.get_track(seed_id).ok().flatten()
            .map(|t| t.path.to_string_lossy().to_string());

        let seed_path = match seed_path {
            Some(p) => p,
            None => {
                self.collection.graph_building = false;
                return Task::none();
            }
        };

        use mesh_core::suggestions::query::*;
        use mesh_core::suggestions::config::*;

        // Use Off filter to score ALL tracks (no key filtering)
        // The top 30 will be highlighted as suggestions
        let config = SuggestionConfig {
            blend_crossover: SuggestionBlendMode::Balanced.crossover(),
            harmonic_floor: 0.0,
            blended_threshold: 0.0,
            stem_complement: false,
        };

        let sources = vec![DbSource {
            db: db.clone(),
            collection_root: self.collection.collection_path.clone(),
            name: "Local".to_string(),
        }];

        let played = HashSet::new();

        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let suggestions = query_suggestions(
                        &sources,
                        vec![seed_path],
                        energy_direction,
                        KeyScoringModel::Camelot,
                        config,
                        10_000,     // per_seed_limit (unused — brute-force scores all tracks)
                        usize::MAX, // total_limit — don't truncate, we need all scores
                        &played,
                        None,
                        None,
                        true, // emit_components
                    )?;

                    // Fetch PCA embeddings for angular positioning
                    let pca_map: HashMap<i64, Vec<f32>> = sources[0].db
                        .get_all_pca_with_tracks()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|(t, vec)| t.id.map(|id| (id, vec)))
                        .collect();

                    let seed_pca = pca_map.get(&seed_id).cloned().unwrap_or_default();

                    // Build scored tracks with PCA embeddings for layout
                    let scored: Vec<(i64, f32, Vec<f32>)> = suggestions.iter()
                        .filter_map(|s| {
                            let id = s.track.id?;
                            let pca = pca_map.get(&id).cloned().unwrap_or_default();
                            Some((id, s.score, pca))
                        })
                        .collect();

                    let positions = layout::radial_layout(seed_id, &scored, &seed_pca);

                    Ok::<_, String>((suggestions, positions, energy_direction))
                })
                .await
                .map_err(|e| format!("{e}"))?
            },
            move |result| match result {
                Ok((suggestions, positions, queried_energy)) => Message::GraphSuggestionsReady {
                    seed_id,
                    suggestions: Arc::new(suggestions),
                    positions: Arc::new(positions),
                    queried_energy,
                },
                Err(e) => {
                    log::error!("[GRAPH] Suggestion query failed: {}", e);
                    Message::GraphSuggestionsReady {
                        seed_id,
                        suggestions: Arc::new(Vec::new()),
                        positions: Arc::new(Vec::new()),
                        queried_energy: 0.5,
                    }
                }
            },
        )
    }

    /// Suggestions + layout complete — update everything.
    pub fn handle_graph_suggestions_ready(
        &mut self,
        _seed_id: i64,
        suggestions: Arc<Vec<mesh_core::suggestions::query::SuggestedTrack>>,
        positions: Arc<Vec<(i64, f32, f32)>>,
        queried_energy: f32,
    ) -> Task<Message> {
        use mesh_core::playlist::NodeId;
        use mesh_widgets::TrackRow;

        self.collection.graph_building = false;

        // ── Diagnostic: score distribution stats ────────────────────────
        if !suggestions.is_empty() {
            let scores: Vec<f32> = suggestions.iter().map(|s| s.score).collect();
            let n = scores.len();
            let min = scores.iter().cloned().fold(f32::MAX, f32::min);
            let max = scores.iter().cloned().fold(f32::MIN, f32::max);
            let mean = scores.iter().sum::<f32>() / n as f32;

            let mut sorted = scores.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = sorted[n / 2];
            let p10 = sorted[n / 10];
            let p25 = sorted[n / 4];
            let p75 = sorted[(n * 3) / 4];
            let p90 = sorted[(n * 9) / 10];

            let variance = scores.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n as f32;
            let stddev = variance.sqrt();

            // Score histogram (buckets of 0.1)
            let mut buckets = [0u32; 11]; // 0.0-0.1, 0.1-0.2, ..., 0.9-1.0, 1.0+
            for &s in &scores {
                let idx = ((s * 10.0).floor() as usize).min(10);
                buckets[idx] += 1;
            }

            log::info!(
                "[GRAPH STATS] {} tracks scored | score range [{:.3}, {:.3}] | mean={:.3} median={:.3} stddev={:.3}",
                n, min, max, mean, median, stddev
            );
            log::info!(
                "[GRAPH STATS] percentiles: p10={:.3} p25={:.3} p50={:.3} p75={:.3} p90={:.3}",
                p10, p25, median, p75, p90
            );
            log::info!(
                "[GRAPH STATS] histogram (0.0-1.0 in 0.1 buckets): {:?}",
                buckets
            );

            // Top 5 (highest = best) and bottom 5 (lowest = worst)
            log::info!("[GRAPH STATS] best 5 (highest): {:.3} {:.3} {:.3} {:.3} {:.3}",
                sorted[n-1], sorted[(n-2).max(0)], sorted[(n-3).max(0)], sorted[(n-4).max(0)], sorted[(n-5).max(0)]);
            log::info!("[GRAPH STATS] worst 5 (lowest): {:.3} {:.3} {:.3} {:.3} {:.3}",
                sorted[0], sorted[1.min(n-1)], sorted[2.min(n-1)], sorted[3.min(n-1)], sorted[4.min(n-1)]);

            // Gap analysis: how many tracks in the "suggested" inner ring vs outer
            let suggested_count = n.min(SUGGESTION_HIGHLIGHT_LIMIT);
            let threshold = sorted[n.saturating_sub(suggested_count)];
            let total_nodes = self.collection.graph_state.as_ref()
                .map(|s| s.track_meta.len()).unwrap_or(0);
            log::info!(
                "[GRAPH STATS] top {} suggestion score threshold: {:.3} | library nodes: {} | scored tracks: {}",
                SUGGESTION_HIGHLIGHT_LIMIT, threshold, total_nodes, n
            );

            // Radius distribution (what the graph actually shows)
            let radii: Vec<f32> = scores.iter().map(|s| s.sqrt()).collect();
            let r_min = radii.iter().cloned().fold(f32::MAX, f32::min);
            let r_max = radii.iter().cloned().fold(f32::MIN, f32::max);
            let r_mean = radii.iter().sum::<f32>() / n as f32;
            log::info!(
                "[GRAPH STATS] radius range [{:.3}, {:.3}] mean={:.3} (sqrt of score)",
                r_min, r_max, r_mean
            );
        }
        // ── End diagnostic ──────────────────────────────────────────────

        let state = match self.collection.graph_state.as_mut() {
            Some(s) => s,
            None => return Task::none(),
        };

        // Apply radial positions
        for &(id, x, y) in positions.iter() {
            state.positions.insert(id, (x, y));
        }

        // Update highlights: top N are suggestions, rest are unrelated
        state.suggestion_ids.clear();
        state.suggestion_scores.clear();
        state.suggestion_edges.clear();

        let current_seed = state.seed_stack.last().copied();

        // ALL scored tracks get their score stored (for positioning)
        // but only top SUGGESTION_HIGHLIGHT_LIMIT get highlighted
        for (i, suggestion) in suggestions.iter().enumerate() {
            if let Some(id) = suggestion.track.id {
                state.suggestion_scores.insert(id, suggestion.score);
                if i < SUGGESTION_HIGHLIGHT_LIMIT {
                    state.suggestion_ids.insert(id);
                    if let Some(seed_id) = current_seed {
                        state.suggestion_edges.push((seed_id, id, suggestion.score));
                    }
                }
            }
        }

        state.clear_caches();

        // Build left panel rows (top 30 only)
        let mut rows: Vec<TrackRow<NodeId>> = Vec::with_capacity(SUGGESTION_HIGHLIGHT_LIMIT);
        for (i, s) in suggestions.iter().take(SUGGESTION_HIGHLIGHT_LIMIT).enumerate() {
            let title = s.track.path.file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            let node_id = NodeId(format!("graph_{}", s.track.id.unwrap_or(i as i64)));
            let mut row = TrackRow::new(node_id, title, (i + 1) as i32);
            row.artist = s.track.artist.clone();
            row.bpm = s.track.bpm;
            row.key = s.track.key.clone();
            row.final_score = Some(s.score);

            if let Some(ref cs) = s.component_scores {
                row.hnsw_dist = Some(cs.hnsw_distance);
                row.key_score = Some(cs.key_score);
                row.energy_match = Some(cs.intensity_penalty);
                row.coplay_count = Some(cs.coplay_score);
            }

            rows.push(row);
        }
        self.collection.graph_suggestion_rows = rows;

        // Debounce check: if slider moved while query was in flight, re-query
        let current_energy = state.energy_direction;
        if (current_energy - queried_energy).abs() > 0.02 {
            if let Some(seed_id) = current_seed {
                return self.run_graph_suggestion_query(seed_id);
            }
        }

        Task::none()
    }
}
