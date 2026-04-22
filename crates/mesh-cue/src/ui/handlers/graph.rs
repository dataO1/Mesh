//! Graph view message handlers
//!
//! Handles graph edge building, t-SNE layout with fisheye distortion,
//! HDBSCAN cluster overlays, seed selection, energy slider changes,
//! and breadcrumb navigation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use iced::Color;
use iced::Task;
use mesh_widgets::graph_view::{GraphViewState, TrackMeta};

/// Result of the combined background graph build (t-SNE + clustering + metadata).
#[derive(Debug)]
pub struct GraphBuildResult {
    pub positions: HashMap<i64, (f32, f32)>,
    pub track_meta: HashMap<i64, TrackMeta>,
    pub pca_dims: usize,
    pub cluster_result: mesh_core::graph_compute::ClusterResult,
    pub normalize: bool,
    pub stem_colors: [Color; 4],
}

impl GraphBuildResult {
    pub fn empty() -> Self {
        Self {
            positions: HashMap::new(),
            track_meta: HashMap::new(),
            pca_dims: 0,
            cluster_result: mesh_core::graph_compute::ClusterResult {
                clusters: HashMap::new(),
                confidence: HashMap::new(),
                colors: HashMap::new(),
                thresholds: mesh_core::graph_compute::CommunityThresholds::default(),
            },
            normalize: false,
            stem_colors: [Color::WHITE; 4],
        }
    }
}

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

    /// Kick off background graph build: t-SNE layout + metadata in one task.
    /// Skips the old edge build step (edges were unused — suggestions use brute-force).
    pub fn handle_build_graph_edges(&mut self) -> Task<Message> {
        if self.collection.graph_building {
            return Task::none();
        }
        self.collection.graph_building = true;

        let db = self.domain.db_arc();
        let normalize = self.collection.graph_normalize_vectors;
        let algorithm = self.collection.graph_algorithm;
        let stem_colors = self.collection.stem_colors;
        let whiten_alpha = self.collection.pca_whitening_alpha;

        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    mesh_core::rt::pin_to_big_cores();

                    // Load PCA embeddings + track metadata in one pass
                    let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
                    let mut pca_data: Vec<(i64, Vec<f32>)> = all_pca.iter()
                        .filter_map(|(t, v)| Some((t.id?, v.clone())))
                        .collect();

                    // Apply partial PCA whitening before layout
                    mesh_core::graph_compute::apply_pca_whitening(&mut pca_data, whiten_alpha);

                    // Build track metadata
                    let track_meta: HashMap<i64, TrackMeta> = all_pca.iter()
                        .filter_map(|(t, _)| {
                            let id = t.id?;
                            Some((id, TrackMeta {
                                id,
                                title: t.title.clone(),
                                artist: t.artist.clone(),
                                key: t.key.clone(),
                                bpm: t.bpm,
                            }))
                        })
                        .collect();

                    // Detect PCA dimensionality
                    let pca_dims = pca_data.first().map(|(_, v)| v.len()).unwrap_or(0);

                    // Run layout algorithm
                    let positions = mesh_core::graph_compute::compute_layout(&pca_data, algorithm, normalize);

                    // Run consensus clustering + compute community thresholds
                    let mut cluster_result = mesh_core::graph_compute::run_consensus_clustering(&positions);
                    cluster_result.thresholds = mesh_core::graph_compute::compute_community_thresholds(&pca_data, &cluster_result.clusters);

                    (positions, track_meta, pca_dims, cluster_result, normalize, stem_colors)
                })
                .await
                .ok()
            },
            |result| match result {
                Some((positions, track_meta, pca_dims, cluster_result, normalize, stem_colors)) => {
                    Message::GraphEdgesReady(Arc::new(GraphBuildResult {
                        positions, track_meta, pca_dims, cluster_result, normalize, stem_colors,
                    }))
                }
                None => {
                    log::error!("[GRAPH] Background graph build panicked");
                    Message::GraphEdgesReady(Arc::new(GraphBuildResult::empty()))
                }
            },
        )
    }

    /// Graph build complete — create GraphViewState with final t-SNE positions + clusters.
    pub fn handle_graph_edges_ready(&mut self, data: Arc<GraphBuildResult>) -> Task<Message> {
        self.collection.graph_building = false;

        if data.positions.is_empty() {
            log::warn!("[GRAPH] No positions — PCA embeddings may not be built yet");
            return Task::none();
        }

        let mut state = GraphViewState::new();
        state.positions = data.positions.clone();
        state.tsne_positions = data.positions.clone();
        state.track_meta = data.track_meta.clone();
        state.normalize_vectors = data.normalize;
        state.pca_dims = data.pca_dims;
        state.stem_colors = Some(data.stem_colors);
        state.accent_color = Some(data.stem_colors[0]);
        state.clusters = data.cluster_result.clusters.clone();
        state.cluster_confidence = data.cluster_result.confidence.clone();
        state.cluster_colors = data.cluster_result.colors.iter()
            .map(|(&id, &[r, g, b])| (id, Color::from_rgb(r, g, b)))
            .collect();

        self.collection.graph_state = Some(state);
        self.collection.community_thresholds = Some(data.cluster_result.thresholds.clone());
        log::info!("[GRAPH] Graph ready — {} nodes, {} clusters",
            data.positions.len(),
            data.cluster_result.colors.len());
        Task::none()
    }

    /// Change transition reach and re-query suggestions.
    pub fn handle_graph_transition_reach(&mut self, idx: usize) -> Task<Message> {
        if let Some(ref mut state) = self.collection.graph_state {
            state.transition_reach_index = idx.min(2);
        }
        // Re-query with new transition reach
        let current_seed = self.collection.graph_state.as_ref()
            .and_then(|s| s.seed_stack.get(s.seed_position).copied());
        match current_seed {
            Some(seed_id) => self.run_graph_suggestion_query(seed_id),
            None => Task::none(),
        }
    }

    /// Select a node as seed — push to breadcrumb stack and query all tracks.
    pub fn handle_graph_seed_selected(&mut self, track_id: i64) -> Task<Message> {
        let state = match self.collection.graph_state.as_mut() {
            Some(s) => s,
            None => return Task::none(),
        };

        // Clear any status message on new navigation
        state.status_message = None;

        // Browser-like: truncate forward history, push new seed
        let current = state.seed_stack.get(state.seed_position).copied();
        if current != Some(track_id) {
            // Truncate everything after current position (discard forward history)
            state.seed_stack.truncate(state.seed_position + if current.is_some() { 1 } else { 0 });
            state.seed_stack.push(track_id);
            state.seed_position = state.seed_stack.len() - 1;
        }

        self.run_graph_suggestion_query(track_id)
    }

    /// Toggle L2-normalization — logs comparison stats then rebuilds.
    pub fn handle_graph_toggle_normalize(&mut self, enabled: bool) -> Task<Message> {
        log::info!("[GRAPH] Vector normalization toggled: {} → {}", !enabled, enabled);

        // Log distance distribution comparison for both modes
        let db = self.domain.db_arc();
        if let Ok(all_pca) = db.get_all_pca_with_tracks() {
            if let Some(first) = all_pca.first() {
                let seed = &first.1;
                let mut raw_dists = Vec::new();
                let mut norm_dists = Vec::new();

                for (_, pca) in all_pca.iter().skip(1).take(100) {
                    // Raw cosine distance
                    raw_dists.push(mesh_core::suggestions::query::cosine_distance_pub(seed, pca));

                    // Normalized cosine distance
                    let mut s = seed.clone();
                    let mut c = pca.clone();
                    let sn = s.iter().map(|x| x*x).sum::<f32>().sqrt();
                    let cn = c.iter().map(|x| x*x).sum::<f32>().sqrt();
                    if sn > 1e-10 { s.iter_mut().for_each(|x| *x /= sn); }
                    if cn > 1e-10 { c.iter_mut().for_each(|x| *x /= cn); }
                    norm_dists.push(mesh_core::suggestions::query::cosine_distance_pub(&s, &c));
                }

                if !raw_dists.is_empty() {
                    raw_dists.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    norm_dists.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let n = raw_dists.len();
                    let raw_mean = raw_dists.iter().sum::<f32>() / n as f32;
                    let norm_mean = norm_dists.iter().sum::<f32>() / n as f32;
                    let raw_std = (raw_dists.iter().map(|d| (d - raw_mean).powi(2)).sum::<f32>() / n as f32).sqrt();
                    let norm_std = (norm_dists.iter().map(|d| (d - norm_mean).powi(2)).sum::<f32>() / n as f32).sqrt();
                    log::info!("[GRAPH NORM] Raw distances:  mean={:.4} std={:.4} range=[{:.4}, {:.4}]",
                        raw_mean, raw_std, raw_dists[0], raw_dists[n-1]);
                    log::info!("[GRAPH NORM] Normalized:     mean={:.4} std={:.4} range=[{:.4}, {:.4}]",
                        norm_mean, norm_std, norm_dists[0], norm_dists[n-1]);
                    log::info!("[GRAPH NORM] Spread ratio (std): {:.2}x (>1 = normalization helps, <1 = hurts)",
                        norm_std / raw_std.max(0.0001));
                }
            }
        }

        self.collection.graph_state = None;
        self.collection.graph_edges = None;
        self.collection.graph_suggestion_rows.clear();
        self.collection.graph_normalize_vectors = enabled;
        self.handle_build_graph_edges()
    }

    /// Navigate back in seed history.
    /// Navigate back in seed history (keep forward history intact).
    pub fn handle_graph_seed_back(&mut self) -> Task<Message> {
        let state = match self.collection.graph_state.as_mut() {
            Some(s) => s,
            None => return Task::none(),
        };

        if state.seed_position == 0 {
            // Already at the start — clear selection
            state.suggestion_ids.clear();
            state.suggestion_scores.clear();
            state.suggestion_edges.clear();
            state.clear_caches();
            return Task::none();
        }

        state.seed_position -= 1;
        let prev = state.seed_stack[state.seed_position];
        self.run_graph_suggestion_query(prev)
    }

    /// Navigate forward in seed history.
    pub fn handle_graph_seed_forward(&mut self) -> Task<Message> {
        let state = match self.collection.graph_state.as_mut() {
            Some(s) => s,
            None => return Task::none(),
        };

        if state.seed_position + 1 >= state.seed_stack.len() {
            return Task::none(); // no forward history
        }

        state.seed_position += 1;
        let next = state.seed_stack[state.seed_position];
        self.run_graph_suggestion_query(next)
    }

    /// Export the seed history as a playlist in the DB.
    pub fn handle_graph_export_playlist(&mut self) -> Task<Message> {
        let state = match self.collection.graph_state.as_ref() {
            Some(s) => s,
            None => return Task::none(),
        };

        if state.seed_stack.is_empty() {
            return Task::none();
        }

        // Collect track IDs in seed order
        let seed_ids: Vec<i64> = state.seed_stack.clone();

        // Create playlist
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let name = format!("Set Plan {}", timestamp);

        match self.domain.create_playlist(&name, None) {
            Ok(playlist_id) => {
                let mut added = 0;
                for &track_id in &seed_ids {
                    if self.domain.add_track_to_playlist(playlist_id, track_id).is_ok() {
                        added += 1;
                    }
                }
                log::info!("[GRAPH] Exported {} tracks as playlist '{}' (id={})", added, name, playlist_id);
                if let Some(ref mut state) = self.collection.graph_state {
                    state.status_message = Some(format!("Exported {} tracks as \"{}\"", added, name));
                }
                self.domain.refresh_tree();
                self.collection.tree_nodes = self.domain.tree_nodes().to_vec();
                Task::perform(async {}, |_| Message::RefreshPlaylists)
            }
            Err(e) => {
                log::error!("[GRAPH] Failed to create playlist: {}", e);
                if let Some(ref mut state) = self.collection.graph_state {
                    state.status_message = Some(format!("Export failed: {}", e));
                }
                Task::none()
            }
        }
    }

    /// Node hover changed.
    pub fn handle_graph_node_hovered(&mut self, id: Option<i64>) -> Task<Message> {
        if let Some(ref mut state) = self.collection.graph_state {
            state.hovered_id = id;
            state.node_cache.clear();
        }
        // Highlight corresponding row in suggestion table
        if let Some(track_id) = id {
            let node_id = mesh_core::playlist::NodeId(format!("graph_{}", track_id));
            self.collection.graph_table_state.selected.clear();
            self.collection.graph_table_state.selected.insert(node_id);
        } else {
            self.collection.graph_table_state.selected.clear();
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
            .and_then(|s| s.seed_stack.get(s.seed_position).copied());

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
        let reach_idx = state.transition_reach_index;
        let reach = SuggestionTransitionReach::ALL[reach_idx.min(2)];
        let key_filter = SuggestionKeyFilter::ALL[self.collection.graph_key_filter_index.min(2)];
        let (harmonic_floor, blended_threshold) = key_filter.thresholds();
        let config = SuggestionConfig {
            blend_crossover: SuggestionBlendMode::Balanced.crossover(),
            harmonic_floor,
            blended_threshold,
            stem_complement: false,
            transition_target: reach.target_distance(self.collection.community_thresholds.as_ref()),
            transition_width: reach.bell_width(self.collection.community_thresholds.as_ref()),
            custom_weights: {
                let w = self.collection.suggestion_weights;
                if (w[0] - 0.55).abs() > 0.01 || (w[1] - 0.25).abs() > 0.01 || (w[2] - 0.20).abs() > 0.01 {
                    Some(w)
                } else {
                    None
                }
            },
            intensity_reach: reach.intensity_reach(),
            pca_whitening_alpha: self.collection.pca_whitening_alpha,
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

                    Ok::<_, String>((suggestions, energy_direction))
                })
                .await
                .map_err(|e| format!("{e}"))?
            },
            move |result| match result {
                Ok((suggestions, queried_energy)) => Message::GraphSuggestionsReady {
                    seed_id,
                    suggestions: Arc::new(suggestions),
                    queried_energy,
                },
                Err(e) => {
                    log::error!("[GRAPH] Suggestion query failed: {}", e);
                    Message::GraphSuggestionsReady {
                        seed_id,
                        suggestions: Arc::new(Vec::new()),
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
        }
        // ── End diagnostic ──────────────────────────────────────────────

        let state = match self.collection.graph_state.as_mut() {
            Some(s) => s,
            None => return Task::none(),
        };

        let current_seed = state.seed_stack.last().copied();

        // Restore t-SNE positions — the 2D projection always stays as the base
        // layout. Selecting a seed only changes highlighting and auto-zoom,
        // not node positions. This preserves cluster structure visually.
        for (&id, &(x, y)) in &state.tsne_positions {
            state.positions.insert(id, (x, y));
        }

        // Update highlights: top N are suggestions, filtering out tracks already in seed history
        state.suggestion_ids.clear();
        state.suggestion_scores.clear();
        state.suggestion_edges.clear();

        let seed_set: HashSet<i64> = state.seed_stack.iter().copied().collect();
        let mut highlighted = 0usize;

        for suggestion in suggestions.iter() {
            if let Some(id) = suggestion.track.id {
                state.suggestion_scores.insert(id, suggestion.score);
                // Skip tracks already in seed history for highlights + edges
                if !seed_set.contains(&id) && highlighted < SUGGESTION_HIGHLIGHT_LIMIT {
                    state.suggestion_ids.insert(id);
                    if let Some(seed_id) = current_seed {
                        state.suggestion_edges.push((seed_id, id, suggestion.score));
                    }
                    highlighted += 1;
                }
            }
        }

        state.clear_caches();

        // Build left panel rows (top 30, excluding seed history)
        let mut rows: Vec<TrackRow<NodeId>> = Vec::with_capacity(SUGGESTION_HIGHLIGHT_LIMIT);
        let mut row_count = 0usize;
        for s in suggestions.iter() {
            if row_count >= SUGGESTION_HIGHLIGHT_LIMIT { break; }
            let _id = match s.track.id {
                Some(id) if !seed_set.contains(&id) => id,
                _ => continue,
            };
            let i = row_count;
            let node_id = NodeId(format!("graph_{}", s.track.id.unwrap_or(i as i64)));
            let mut row = TrackRow::new(node_id, &s.track.title, (i + 1) as i32);
            row.artist = s.track.artist.clone();
            row.bpm = s.track.bpm;
            row.key = s.track.key.clone();
            row.final_score = Some(s.score);
            row.track_path = Some(s.track.path.to_string_lossy().to_string());

            // Reason tags as colored pills
            row.tags = s.reason_tags.iter().map(|(label, color)| {
                let mut tag = mesh_widgets::TrackTag::new(label);
                if let Some(hex) = color {
                    tag.color = mesh_widgets::parse_hex_color(hex);
                }
                tag
            }).collect();

            // Score component breakdown
            if let Some(ref cs) = s.component_scores {
                row.hnsw_dist = Some(cs.hnsw_distance);
                row.key_score = Some(cs.key_score);
                row.energy_match = Some(cs.intensity_penalty); // actually reward now
                row.coplay_count = Some(cs.coplay_score);
            }

            rows.push(row);
            row_count += 1;
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
