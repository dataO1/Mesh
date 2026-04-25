//! Shared graph layout and clustering computation for both mesh-cue and mesh-player.
//!
//! - **t-SNE**: Barnes-Hut t-SNE via bhtsne to project high-dim PCA embeddings to 2D
//! - **Consensus clustering**: Multi-scale HDBSCAN for robust community detection

use std::collections::{HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use crate::db::DatabaseService;

/// Apply partial PCA whitening to a set of vectors in-place.
///
/// For each component k, divides by `std_k^alpha` where `std_k` is the standard
/// deviation of component k across all vectors. Then L2-normalizes.
///
/// `alpha=0.0`: no change (identity). `alpha=1.0`: full whitening (all components
/// get equal variance). Intermediate values blend between the two.
///
/// This is computed on-the-fly from the stored vectors — no need to persist
/// singular values separately.
pub fn apply_pca_whitening(vectors: &mut [(i64, Vec<f32>)], alpha: f32) {
    if alpha < 1e-6 || vectors.is_empty() { return; }

    let n = vectors.len();
    let dim = vectors[0].1.len();
    if dim == 0 { return; }

    // Compute per-component mean and std
    let mut means = vec![0.0f32; dim];
    for (_, v) in vectors.iter() {
        for (k, &val) in v.iter().enumerate() {
            means[k] += val;
        }
    }
    for m in &mut means { *m /= n as f32; }

    let mut stds = vec![0.0f32; dim];
    for (_, v) in vectors.iter() {
        for (k, &val) in v.iter().enumerate() {
            stds[k] += (val - means[k]).powi(2);
        }
    }
    for s in &mut stds {
        *s = (*s / n as f32).sqrt().max(1e-10);
    }

    // Apply partial whitening: divide each component by std^alpha, then L2-normalize
    for (_, v) in vectors.iter_mut() {
        for (k, val) in v.iter_mut().enumerate() {
            *val /= stds[k].powf(alpha);
        }
        // L2-normalize
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-10 {
            for val in v.iter_mut() { *val /= norm; }
        }
    }
}

/// Graph layout algorithm selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphAlgorithm {
    #[default]
    Tsne,
    Umap,
}

impl GraphAlgorithm {
    pub fn display_name(&self) -> &'static str {
        match self {
            GraphAlgorithm::Tsne => "t-SNE",
            GraphAlgorithm::Umap => "UMAP",
        }
    }

    pub fn next(self) -> Self {
        match self {
            GraphAlgorithm::Tsne => GraphAlgorithm::Umap,
            GraphAlgorithm::Umap => GraphAlgorithm::Tsne,
        }
    }
}

/// Community detection algorithm selection. HDBSCAN is density-based on the
/// 2D layout; Louvain is modularity-based on a k-NN graph of PCA distances
/// and handles dense continuous regions that HDBSCAN collapses into one
/// mega-cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClusteringAlgorithm {
    Hdbscan,
    #[default]
    Louvain,
}

impl ClusteringAlgorithm {
    pub fn display_name(&self) -> &'static str {
        match self {
            ClusteringAlgorithm::Hdbscan => "HDBSCAN",
            ClusteringAlgorithm::Louvain => "Louvain",
        }
    }

    pub fn next(self) -> Self {
        match self {
            ClusteringAlgorithm::Hdbscan => ClusteringAlgorithm::Louvain,
            ClusteringAlgorithm::Louvain => ClusteringAlgorithm::Hdbscan,
        }
    }
}

/// Collect PCA embeddings from multiple database sources, deduplicated by artist-title.
///
/// Each source is a `(database, source_name)` pair. When the same track appears in
/// multiple sources (matched by lowercase artist + title), the first source wins.
/// Returns `(track_id, pca_vector)` pairs plus a separate metadata map.
pub fn collect_pca_from_sources(
    sources: &[(Arc<DatabaseService>, String)],
) -> (Vec<(i64, Vec<f32>)>, HashMap<i64, TrackMeta>) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut pca_data: Vec<(i64, Vec<f32>)> = Vec::new();
    let mut track_meta: HashMap<i64, TrackMeta> = HashMap::new();

    for (db, source_name) in sources {
        let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
        let source_total = all_pca.len();
        let count_before = pca_data.len();

        for (track, pca_vec) in all_pca {
            let id = match track.id {
                Some(id) => id,
                None => continue,
            };

            // Dedup key: lowercase "artist - title"
            let artist_lower = track.artist.as_deref().unwrap_or("").to_lowercase();
            let title_lower = track.title.to_lowercase();
            let dedup_key = format!("{}\x00{}", artist_lower, title_lower);

            if seen.contains(&dedup_key) {
                continue;
            }
            seen.insert(dedup_key);

            pca_data.push((id, pca_vec));
            track_meta.insert(id, TrackMeta {
                id,
                title: track.title.clone(),
                artist: track.artist.clone(),
                key: track.key.clone(),
                bpm: track.bpm,
            });
        }

        let added = pca_data.len() - count_before;
        log::info!("[GRAPH] Source '{}': {} new tracks ({} duplicates skipped)",
            source_name, added, source_total - added);
    }

    log::info!("[GRAPH] Total: {} unique tracks from {} sources", pca_data.len(), sources.len());
    (pca_data, track_meta)
}

/// Track metadata for graph visualization.
#[derive(Debug, Clone)]
pub struct TrackMeta {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub key: Option<String>,
    pub bpm: Option<f64>,
}

/// Dynamic similarity thresholds derived from actual community structure.
/// Replaces the hardcoded target_distance/bell_width values.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommunityThresholds {
    /// Tight: target distance stays within same community
    pub tight_target: f32,
    pub tight_width: f32,
    /// Medium: same community + nearest neighbors
    pub medium_target: f32,
    pub medium_width: f32,
    /// Open: ~50/50 same community vs others
    pub open_target: f32,
    pub open_width: f32,
}

impl Default for CommunityThresholds {
    fn default() -> Self {
        // Fallback to original hardcoded values if no clustering data
        Self {
            tight_target: 0.25, tight_width: 0.08,
            medium_target: 0.40, medium_width: 0.12,
            open_target: 0.60, open_width: 0.18,
        }
    }
}

/// Result of consensus clustering.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ClusterResult {
    /// Cluster assignments (track_id -> cluster_id, -1 = noise)
    pub clusters: HashMap<i64, i32>,
    /// Per-track confidence [0.0, 1.0] — how consistently this track
    /// clusters with its peers across multiple HDBSCAN scales
    pub confidence: HashMap<i64, f32>,
    /// Cluster colors as [r, g, b] floats (0..1). Derived from the
    /// community's average intensity (warm = high energy, cool = low energy).
    pub colors: HashMap<i32, [f32; 3]>,
    /// Dynamic thresholds derived from community distance statistics
    pub thresholds: CommunityThresholds,
    /// True if this result passed the quality gate (enough communities,
    /// no one community swallowing the library). False if we had to fall
    /// back to best-of-N after exhausting retries. A false value tells
    /// the cache loader "don't keep fighting this — we already tried".
    #[serde(default = "default_true")]
    pub gate_passed: bool,
}

fn default_true() -> bool { true }

/// Compute a cache key for graph positions based on the track set and layout settings.
/// Same tracks + same settings = same key = cached positions reused.
pub fn graph_cache_key(
    pca_data: &[(i64, Vec<f32>)],
    algorithm: GraphAlgorithm,
    clustering: ClusteringAlgorithm,
    normalize: bool,
    whitening_alpha: f32,
) -> String {
    let mut ids: Vec<i64> = pca_data.iter().map(|(id, _)| *id).collect();
    ids.sort();
    let mut hasher = DefaultHasher::new();
    ids.hash(&mut hasher);
    // v18: same algo as v17. Bump forces a one-time recompute for users whose
    // v17 cache held a gate-skipped (degenerate) clustering — see the
    // gate-skipped invalidation in `run_consensus_clustering_cached`.
    // v17: HDBSCAN-L2 min_cluster_size (sqrt(m)*0.8, clamp [15, 50]) +
    // hard cap of 6 sub-communities per macro. Matches typical sub-style
    // counts across DJ genres. Clusters above the cap merge into nearest
    // surviving centroid via the noise-absorb path.
    format!(
        "v18_{:?}_{:?}_norm{}_wh{:.2}_{:016x}",
        algorithm, clustering, normalize as u8, whitening_alpha, hasher.finish()
    )
}

/// Run consensus clustering with DB caching keyed by `cache_key`.
/// Returns cached ClusterResult if available, otherwise computes fresh and stores.
pub fn run_consensus_clustering_cached(
    pca_data: &[(i64, Vec<f32>)],
    positions: &HashMap<i64, (f32, f32)>,
    genre_labels: &HashMap<i64, String>,
    algorithm: ClusteringAlgorithm,
    cache_key: &str,
    db: Option<&DatabaseService>,
) -> ClusterResult {
    if let Some(db) = db {
        match db.get_graph_clusters_json(cache_key) {
            Ok(Some(json)) => {
                match serde_json::from_str::<ClusterResult>(&json) {
                    Ok(result) => {
                        let (num, largest_frac, _) = grade_clustering(&result);
                        if result.gate_passed {
                            log::info!(
                                "[GRAPH] Using cached clusters ({} tracks, {} communities, largest={:.1}%, gate passed)",
                                result.clusters.len(), num, largest_frac * 100.0,
                            );
                            return result;
                        } else {
                            // Gate was skipped — retry loop exhausted at write
                            // time. Don't trust the cached degenerate result:
                            // refight with a fresh seed each launch. The cost
                            // is one cluster recompute per session; the win is
                            // we eventually escape "unlucky seed" lock-in.
                            log::warn!(
                                "[GRAPH] Discarding cached clusters ({} tracks, {} communities, largest={:.1}%, gate had been skipped) — recomputing",
                                result.clusters.len(), num, largest_frac * 100.0,
                            );
                            // Fall through to recompute below.
                        }
                    }
                    Err(e) => log::warn!("[GRAPH] Cached cluster JSON parse failed: {}, recomputing", e),
                }
            }
            Ok(None) => log::info!("[GRAPH] No cached clusters found, computing fresh"),
            Err(e) => log::warn!("[GRAPH] Cluster cache read error: {}, recomputing", e),
        }
    }

    let result = match algorithm {
        ClusteringAlgorithm::Hdbscan => run_consensus_clustering(pca_data, positions),
        ClusteringAlgorithm::Louvain => {
            // Sweep L2 HDBSCAN parameters until the overall clustering passes
            // the quality gate (≥ MIN_COMMUNITIES_GATE communities AND largest
            // ≤ MAX_LARGEST_FRACTION). Smaller sqrt_factor / min_samples =>
            // finer L2 splits => more sub-communities. On libraries with a
            // dominant macro (e.g. 74%-DnB), the default (0.8, 5) often
            // collapses the macro to 2 sub-communities and fails the gate;
            // smaller values rescue that case.
            const ATTEMPTS: &[(f32, usize)] = &[
                (0.8, 5),  // default — what mesh-cue typically used
                (0.6, 5),
                (0.5, 4),
                (0.4, 4),
                (0.3, 3),
                (0.25, 3),
                (0.2, 3),
                (0.15, 2),
                (0.1, 2),
            ];
            let mut best: Option<(ClusterResult, f32)> = None;
            let mut chosen: Option<ClusterResult> = None;
            for (idx, &(factor, samples)) in ATTEMPTS.iter().enumerate() {
                let r = run_louvain_clustering_with_l2(
                    pca_data, positions, genre_labels, factor, samples,
                );
                let (num, largest_frac, score) = grade_clustering(&r);
                log::info!(
                    "[GRAPH/LOUVAIN/RETRY] Attempt {}/{} (l2_factor={:.2}, l2_min_samples={}): {} communities, largest={:.1}%, score={:.2}",
                    idx + 1, ATTEMPTS.len(), factor, samples,
                    num, largest_frac * 100.0, score,
                );
                if num >= MIN_COMMUNITIES_GATE && largest_frac <= MAX_LARGEST_FRACTION {
                    log::info!("[GRAPH/LOUVAIN/RETRY] Gate passed on attempt {}", idx + 1);
                    let mut accepted = r;
                    accepted.gate_passed = true;
                    chosen = Some(accepted);
                    break;
                }
                if best.as_ref().map_or(true, |(_, s)| score > *s) {
                    best = Some((r, score));
                }
            }
            chosen.unwrap_or_else(|| {
                log::warn!(
                    "[GRAPH/LOUVAIN/RETRY] All {} attempts failed gate — using best-scoring result",
                    ATTEMPTS.len(),
                );
                let mut fb = best.map(|(r, _)| r).unwrap_or_else(|| ClusterResult {
                    clusters: HashMap::new(),
                    confidence: HashMap::new(),
                    colors: HashMap::new(),
                    thresholds: CommunityThresholds::default(),
                    gate_passed: false,
                });
                fb.gate_passed = false;
                fb
            })
        }
    };

    if let Some(db) = db {
        match serde_json::to_string(&result) {
            Ok(json) => {
                if let Err(e) = db.store_graph_clusters_json(cache_key, &json) {
                    log::warn!("[GRAPH] Failed to cache clusters: {}", e);
                }
            }
            Err(e) => log::warn!("[GRAPH] Failed to serialize clusters: {}", e),
        }
    }
    result
}

/// Dispatch to the selected layout algorithm, with DB caching.
/// Returns cached positions if available, otherwise computes fresh and stores.
pub fn compute_layout_cached(
    pca_data: &[(i64, Vec<f32>)],
    algorithm: GraphAlgorithm,
    normalize: bool,
    whitening_alpha: f32,
    db: Option<&DatabaseService>,
) -> HashMap<i64, (f32, f32)> {
    // Positions depend on layout settings only — NOT on clustering algorithm.
    // Using the full clustering-aware key here would pointlessly invalidate
    // the layout cache every time the user toggles HDBSCAN ↔ Louvain.
    let mut ids: Vec<i64> = pca_data.iter().map(|(id, _)| *id).collect();
    ids.sort();
    let mut hasher = DefaultHasher::new();
    ids.hash(&mut hasher);
    let cache_key = format!(
        "positions_v6_{:?}_norm{}_wh{:.2}_{:016x}",
        algorithm, normalize as u8, whitening_alpha, hasher.finish()
    );

    // Try cache
    if let Some(db) = db {
        match db.get_graph_positions(&cache_key) {
            Ok(Some(cached)) if cached.len() == pca_data.len() => {
                log::info!(
                    "[GRAPH] Using cached positions ({} tracks, key={})",
                    cached.len(), &cache_key[..cache_key.len().min(40)]
                );
                return cached;
            }
            Ok(Some(cached)) => {
                log::info!(
                    "[GRAPH] Cache stale: {} cached vs {} current tracks",
                    cached.len(), pca_data.len()
                );
            }
            Ok(None) => {
                log::info!("[GRAPH] No cached positions found, computing fresh layout");
            }
            Err(e) => {
                log::warn!("[GRAPH] Cache read error: {}, computing fresh layout", e);
            }
        }
    }

    // Compute fresh
    let positions = compute_layout(pca_data, algorithm, normalize);

    // Store in cache
    if let Some(db) = db {
        if let Err(e) = db.store_graph_positions(&cache_key, &positions) {
            log::warn!("[GRAPH] Failed to cache positions: {}", e);
        } else {
            log::info!("[GRAPH] Cached {} positions (key={})", positions.len(), &cache_key[..cache_key.len().min(40)]);
        }
    }

    positions
}

/// Dispatch to the selected layout algorithm (uncached).
pub fn compute_layout(
    pca_data: &[(i64, Vec<f32>)],
    algorithm: GraphAlgorithm,
    normalize: bool,
) -> HashMap<i64, (f32, f32)> {
    match algorithm {
        GraphAlgorithm::Tsne => compute_tsne_layout(pca_data, normalize),
        GraphAlgorithm::Umap => compute_umap_layout(pca_data, normalize),
    }
}

/// Run Barnes-Hut t-SNE on PCA embeddings to produce 2D positions.
///
/// Input: slice of (track_id, pca_vector). Returns track_id -> (x, y).
/// Perplexity scales with dataset size: sqrt(n)/2 clamped to [5, 50].
/// 750 epochs, theta=0.5, Euclidean distance.
///
/// For reproducibility: input is sorted by track ID (deterministic processing
/// order) and the output is PCA-aligned to a canonical orientation so the
/// layout doesn't rotate/flip between runs.
pub fn compute_tsne_layout(
    pca_data: &[(i64, Vec<f32>)],
    normalize: bool,
) -> HashMap<i64, (f32, f32)> {
    if pca_data.len() < 10 {
        log::warn!("[GRAPH] Not enough PCA embeddings for t-SNE ({})", pca_data.len());
        return HashMap::new();
    }

    // Sort by track ID for deterministic processing order
    let mut sorted: Vec<(i64, &Vec<f32>)> = pca_data.iter().map(|(id, v)| (*id, v)).collect();
    sorted.sort_by_key(|(id, _)| *id);

    let n = sorted.len();
    log::info!("[GRAPH] Running Barnes-Hut t-SNE on {} tracks → 2D...", n);

    let ids: Vec<i64> = sorted.iter().map(|(id, _)| *id).collect();

    let owned_vecs: Vec<Vec<f32>> = if normalize {
        sorted.iter().map(|(_, pca)| {
            let mut v = (*pca).clone();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 1e-10 { for x in v.iter_mut() { *x /= norm; } }
            v
        }).collect()
    } else {
        sorted.iter().map(|(_, pca)| (*pca).clone()).collect()
    };
    let samples: Vec<&[f32]> = owned_vecs.iter().map(|v| v.as_slice()).collect();

    let perplexity = ((n as f32).sqrt() / 2.0).clamp(5.0, 50.0);

    let mut tsne = bhtsne::tSNE::new(&samples);
    tsne.embedding_dim(2)
        .perplexity(perplexity)
        .epochs(750)
        .barnes_hut(0.5, |a: &&[f32], b: &&[f32]| {
            a.iter().zip(b.iter())
                .map(|(x, y)| (x - y).powi(2))
                .sum::<f32>()
                .sqrt()
        });

    let embedding = tsne.embedding();
    let mut raw_positions: Vec<(f32, f32)> = (0..n)
        .map(|i| (embedding[i * 2], embedding[i * 2 + 1]))
        .collect();

    // PCA-align the 2D output for canonical orientation.
    // This removes random rotation/reflection between runs.
    pca_align_2d(&mut raw_positions);

    let mut positions = HashMap::with_capacity(n);
    for (i, &id) in ids.iter().enumerate() {
        positions.insert(id, raw_positions[i]);
    }

    log::info!("[GRAPH] t-SNE complete — {} 2D positions", positions.len());
    positions
}

/// Run UMAP on PCA embeddings to produce 2D positions.
///
/// UMAP preserves both local AND global structure better than t-SNE.
/// Tracks that are similar across communities appear close in the graph.
///
/// Dynamic defaults:
/// - n_neighbors: sqrt(n) clamped to [5, 50] (same scaling as t-SNE perplexity)
/// - min_dist: 0.1 (standard, controls cluster packing)
/// - n_epochs: scales with dataset size (200 for small, 500 for large)
pub fn compute_umap_layout(
    pca_data: &[(i64, Vec<f32>)],
    normalize: bool,
) -> HashMap<i64, (f32, f32)> {
    use ndarray::Array2;
    use umap_rs::{Umap, UmapConfig};
    use umap_rs::config::{GraphParams, ManifoldParams, OptimizationParams};

    if pca_data.len() < 10 {
        log::warn!("[GRAPH] Not enough PCA embeddings for UMAP ({})", pca_data.len());
        return HashMap::new();
    }

    let mut sorted: Vec<(i64, &Vec<f32>)> = pca_data.iter().map(|(id, v)| (*id, v)).collect();
    sorted.sort_by_key(|(id, _)| *id);

    let n = sorted.len();
    let dims = sorted[0].1.len();
    log::info!("[GRAPH] Running UMAP on {} tracks ({}d → 2D)...", n, dims);

    let ids: Vec<i64> = sorted.iter().map(|(id, _)| *id).collect();

    // Build ndarray matrix from PCA vectors (optional normalization)
    let mut data = Array2::<f32>::zeros((n, dims));
    for (i, (_, pca)) in sorted.iter().enumerate() {
        let mut row = data.row_mut(i);
        for (j, &val) in pca.iter().enumerate() {
            row[j] = val;
        }
        if normalize {
            let norm: f32 = row.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 1e-10 {
                row.iter_mut().for_each(|x| *x /= norm);
            }
        }
    }

    // Dynamic parameters
    let n_neighbors = ((n as f32).sqrt() as usize).clamp(5, 50);
    let n_epochs = if n < 500 { 200 } else { 500 };

    // Brute-force KNN using cosine distance (matches the suggestion algorithm)
    let k = n_neighbors;
    let mut knn_indices = Array2::<u32>::zeros((n, k));
    let mut knn_dists = Array2::<f32>::zeros((n, k));

    for i in 0..n {
        let mut dists: Vec<(usize, f32)> = (0..n)
            .filter(|&j| j != i)
            .map(|j| {
                // Cosine distance: 1 - (dot / (norm_a * norm_b))
                let mut dot = 0.0f32;
                let mut na = 0.0f32;
                let mut nb = 0.0f32;
                for d in 0..dims {
                    let a = data[[i, d]];
                    let b = data[[j, d]];
                    dot += a * b;
                    na += a * a;
                    nb += b * b;
                }
                let denom = (na * nb).sqrt();
                let cos_dist = if denom > 1e-10 { (1.0 - dot / denom).max(0.0) } else { 1.0 };
                (j, cos_dist)
            })
            .collect();
        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        for (ki, &(j, d)) in dists.iter().take(k).enumerate() {
            knn_indices[[i, ki]] = j as u32;
            knn_dists[[i, ki]] = d;
        }
    }

    // Deterministic initialization from first 2 PCA components at natural scale
    let mut init = Array2::<f32>::zeros((n, 2));
    // Scale to reasonable range for UMAP (std ~1.0)
    let std0: f32;
    let std1: f32;
    if dims >= 2 {
        let mean0: f32 = (0..n).map(|i| data[[i, 0]]).sum::<f32>() / n as f32;
        let mean1: f32 = (0..n).map(|i| data[[i, 1]]).sum::<f32>() / n as f32;
        std0 = ((0..n).map(|i| (data[[i, 0]] - mean0).powi(2)).sum::<f32>() / n as f32).sqrt().max(1e-6);
        std1 = ((0..n).map(|i| (data[[i, 1]] - mean1).powi(2)).sum::<f32>() / n as f32).sqrt().max(1e-6);
        for i in 0..n {
            init[[i, 0]] = (data[[i, 0]] - mean0) / std0 * 0.1;
            init[[i, 1]] = (data[[i, 1]] - mean1) / std1 * 0.1;
        }
    }

    let config = UmapConfig {
        manifold: ManifoldParams {
            min_dist: 0.1,
            ..Default::default()
        },
        graph: GraphParams {
            n_neighbors: k,
            ..Default::default()
        },
        optimization: OptimizationParams {
            n_epochs: Some(n_epochs),
            ..Default::default()
        },
        ..Default::default()
    };

    let umap = Umap::new(config);
    let fitted = umap.fit(
        data.view(),
        knn_indices.view(),
        knn_dists.view(),
        init.view(),
    );

    let embedding = fitted.embedding();
    let mut raw_positions: Vec<(f32, f32)> = (0..n)
        .map(|i| (embedding[[i, 0]], embedding[[i, 1]]))
        .collect();

    pca_align_2d(&mut raw_positions);

    let mut positions = HashMap::with_capacity(n);
    for (i, &id) in ids.iter().enumerate() {
        positions.insert(id, raw_positions[i]);
    }

    log::info!("[GRAPH] UMAP complete — {} 2D positions (n_neighbors={}, n_epochs={})", n, k, n_epochs);
    positions
}

/// Align 2D positions to principal axes so the layout is orientation-stable.
/// Computes the 2x2 covariance matrix, finds the dominant eigenvector,
/// rotates all points so the dominant axis is horizontal, then ensures
/// the majority of mass is in the top-right quadrant (fixes reflection).
fn pca_align_2d(positions: &mut [(f32, f32)]) {
    let n = positions.len() as f32;
    if n < 2.0 { return; }

    // Center
    let (cx, cy) = positions.iter().fold((0.0f32, 0.0f32), |(sx, sy), &(x, y)| (sx + x, sy + y));
    let (cx, cy) = (cx / n, cy / n);
    for p in positions.iter_mut() {
        p.0 -= cx;
        p.1 -= cy;
    }

    // 2x2 covariance matrix
    let (mut cxx, mut cxy, mut cyy) = (0.0f32, 0.0f32, 0.0f32);
    for &(x, y) in positions.iter() {
        cxx += x * x;
        cxy += x * y;
        cyy += y * y;
    }

    // Dominant eigenvector of [[cxx, cxy], [cxy, cyy]] via analytic formula
    let trace = cxx + cyy;
    let det = cxx * cyy - cxy * cxy;
    let discriminant = (trace * trace / 4.0 - det).max(0.0);
    let lambda1 = trace / 2.0 + discriminant.sqrt();

    // Eigenvector for lambda1
    let (ex, ey) = if cxy.abs() > 1e-10 {
        let ey = lambda1 - cxx;
        let len = (cxy * cxy + ey * ey).sqrt();
        (cxy / len, ey / len)
    } else if cxx >= cyy {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };

    // Rotate so dominant axis aligns with X
    // Rotation: x' = x*ex + y*ey, y' = -x*ey + y*ex
    for p in positions.iter_mut() {
        let (x, y) = *p;
        p.0 = x * ex + y * ey;
        p.1 = -x * ey + y * ex;
    }

    // Fix reflection: ensure more mass is in positive X and positive Y
    let (sum_x, sum_y) = positions.iter()
        .fold((0.0f32, 0.0f32), |(sx, sy), &(x, y)| {
            (sx + x.signum(), sy + y.signum())
        });
    if sum_x < 0.0 {
        for p in positions.iter_mut() { p.0 = -p.0; }
    }
    if sum_y < 0.0 {
        for p in positions.iter_mut() { p.1 = -p.1; }
    }
}

/// Minimum number of communities a clustering roll must produce to be
/// accepted. Below this, we retry. Tuned for "rough subgenres at a glance"
/// — typical good rolls produce 12–15 communities.
const MIN_COMMUNITIES_GATE: usize = 5;

/// Maximum share of assigned tracks that may land in the single largest
/// community. If one community swallows more than this, the roll is rejected.
const MAX_LARGEST_FRACTION: f32 = 0.5;

/// Per-attempt parameter sets. The `hdbscan` crate has NO randomness (no
/// rand dep, no seed) — so retrying with identical params gives identical
/// output. Each attempt varies (scales, min_cluster_size) so successive
/// attempts actually explore different granularities.
///
/// `min_samples=1` is deliberately excluded from all sets: in a dense
/// t-SNE layout it chain-merges everything reachable into one mega-cluster.
///
/// `min_cluster_size` lives in the 7–12 range: setting it higher (e.g. 15)
/// forces small subgenres to be absorbed into whatever mega-blob is nearby
/// in the 2D layout, which collapses a 900-track library into 2–3 clusters
/// where 12–15 would be natural.
struct AttemptParams {
    scales: &'static [usize],
    min_cluster_size: usize,
}

const ATTEMPT_PARAMS: [AttemptParams; 5] = [
    AttemptParams { scales: &[2, 4, 6, 9, 13, 18], min_cluster_size: 9 },   // original-style good default
    AttemptParams { scales: &[3, 7, 12, 18, 25],   min_cluster_size: 8 },   // slightly looser
    AttemptParams { scales: &[5, 10, 15, 20, 25],  min_cluster_size: 10 },  // moderate
    AttemptParams { scales: &[2, 5, 10, 15, 20],   min_cluster_size: 7 },   // more, smaller clusters
    AttemptParams { scales: &[8, 12, 16, 20, 25],  min_cluster_size: 12 },  // fewer, larger clusters
];

const MAX_CLUSTERING_ATTEMPTS: usize = ATTEMPT_PARAMS.len();

/// Multi-scale consensus clustering on the 2D t-SNE/UMAP projection.
///
/// We cluster in the 2D space, not in the high-dimensional PCA space. t-SNE
/// and UMAP are explicitly designed to pull similar tracks close and push
/// dissimilar ones apart, so the 2D layout already contains the separation
/// structure we want communities to follow. Clustering in 128-D directly
/// hits the curse of dimensionality — Euclidean distances become nearly
/// uniform across pairs, HDBSCAN finds only the very densest pockets, and
/// most of the library gets labeled as noise.
///
/// `pca_data` is accepted for API symmetry with `compute_layout_cached` but
/// not used for clustering; it's kept so future enhancements (e.g.,
/// clustering on a low-D PCA slice) don't require a signature change.
///
/// Runs HDBSCAN at 7 different min_samples values and builds a co-occurrence
/// matrix. Tracks that consistently cluster together (>= 70% of runs) are
/// connected via union-find into robust communities.
///
/// HDBSCAN has internal non-determinism; occasional rolls still collapse
/// the library into 1–3 mega-communities even in 2D. We run up to
/// MAX_CLUSTERING_ATTEMPTS rolls, accept the first that passes the quality
/// gate, and fall back to the best-scoring roll if none pass.
///
/// Cluster colors are derived from each community's average 2D position —
/// mapped to a perceptual hue wheel. This produces deterministic colors
/// that correlate with the visual layout.
pub fn run_consensus_clustering(
    _pca_data: &[(i64, Vec<f32>)],
    positions: &HashMap<i64, (f32, f32)>,
) -> ClusterResult {
    let n = positions.len();
    if n < 10 {
        return ClusterResult {
            clusters: HashMap::new(),
            confidence: HashMap::new(),
            colors: HashMap::new(),
            thresholds: CommunityThresholds::default(),
            gate_passed: true,
        };
    }

    let mut ids: Vec<i64> = positions.keys().copied().collect();
    ids.sort();

    let data: Vec<Vec<f64>> = ids.iter()
        .filter_map(|id| positions.get(id))
        .map(|&(x, y)| vec![x as f64, y as f64])
        .collect();

    // Retry loop with parameter variation: each attempt uses a different
    // (scales, min_cluster_size) pair so results differ. Keep the
    // best-scoring attempt; return the first that passes the gate.
    // Best-of-N is the fallback.
    let mut best: Option<(ClusterResult, f32)> = None;
    for (attempt_idx, params) in ATTEMPT_PARAMS.iter().enumerate() {
        let attempt = attempt_idx + 1;
        let mut result = run_consensus_clustering_once(
            &data, positions, &ids, params.scales, params.min_cluster_size,
        );
        let (num_communities, largest_frac, score) = grade_clustering(&result);
        log::info!(
            "[GRAPH] Clustering attempt {}/{} (scales={:?}, min_size={}): {} communities, largest={:.1}%, score={:.2}",
            attempt, MAX_CLUSTERING_ATTEMPTS, params.scales, params.min_cluster_size,
            num_communities, largest_frac * 100.0, score,
        );

        if num_communities >= MIN_COMMUNITIES_GATE && largest_frac <= MAX_LARGEST_FRACTION {
            log::info!("[GRAPH] Gate passed on attempt {}", attempt);
            result.gate_passed = true;
            return result;
        }

        if best.as_ref().map_or(true, |(_, s)| score > *s) {
            best = Some((result, score));
        }
    }

    log::warn!(
        "[GRAPH] All {} attempts failed gate — using best-scoring roll (data may genuinely be homogeneous)",
        MAX_CLUSTERING_ATTEMPTS,
    );
    let mut fallback = best.map(|(r, _)| r).unwrap_or_else(|| ClusterResult {
        clusters: HashMap::new(),
        confidence: HashMap::new(),
        colors: HashMap::new(),
        thresholds: CommunityThresholds::default(),
        gate_passed: false,
    });
    fallback.gate_passed = false;
    fallback
}

/// Grade a clustering result. Returns (num_communities, largest_fraction, score).
/// Score rewards both community count and balance — higher is better.
fn grade_clustering(result: &ClusterResult) -> (usize, f32, f32) {
    let num = result.colors.len();
    if num == 0 { return (0, 0.0, 0.0); }

    let mut sizes: HashMap<i32, usize> = HashMap::new();
    for &cid in result.clusters.values() {
        if cid >= 0 { *sizes.entry(cid).or_default() += 1; }
    }
    let total_assigned: usize = sizes.values().sum();
    if total_assigned == 0 { return (num, 0.0, 0.0); }
    let largest = sizes.values().copied().max().unwrap_or(0);
    let largest_frac = largest as f32 / total_assigned as f32;
    // Reward more communities AND balanced sizes (small largest_frac).
    let score = num as f32 * (1.0 - largest_frac);
    (num, largest_frac, score)
}

/// Single consensus clustering roll — extracted from `run_consensus_clustering`
/// so the retry loop can call it with different (scales, min_cluster_size)
/// per attempt.
fn run_consensus_clustering_once(
    data: &[Vec<f64>],
    positions: &HashMap<i64, (f32, f32)>,
    ids: &[i64],
    scales: &[usize],
    min_cluster_size: usize,
) -> ClusterResult {
    let n = data.len();

    // Run HDBSCAN at the requested min_samples values
    let mut all_labels: Vec<Vec<i32>> = Vec::new();

    for &min_samples in scales {
        let hp = hdbscan::HdbscanHyperParams::builder()
            .min_cluster_size(min_cluster_size)
            .min_samples(min_samples)
            .build();
        let clusterer = hdbscan::Hdbscan::new(data, hp);
        match clusterer.cluster() {
            Ok(labels) => all_labels.push(labels),
            Err(_) => continue,
        }
    }

    if all_labels.is_empty() {
        return ClusterResult {
            clusters: ids.iter().map(|&id| (id, -1)).collect(),
            confidence: ids.iter().map(|&id| (id, 0.0)).collect(),
            colors: HashMap::new(),
            thresholds: CommunityThresholds::default(),
            gate_passed: false,
        };
    }
    let num_runs = all_labels.len();

    // Build co-occurrence matrix
    let mut cooccurrence = vec![0u8; n * n];
    for labels in &all_labels {
        let mut cluster_members: HashMap<i32, Vec<usize>> = HashMap::new();
        for (i, &label) in labels.iter().enumerate() {
            if label >= 0 {
                cluster_members.entry(label).or_default().push(i);
            }
        }
        for members in cluster_members.values() {
            for &a in members {
                for &b in members {
                    if a < b {
                        cooccurrence[a * n + b] += 1;
                    }
                }
            }
        }
    }

    // Threshold at 70% and find connected components (union-find)
    let threshold = (num_runs as f32 * 0.7).ceil() as u8;
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut Vec<usize>, x: usize) -> usize {
        let mut r = x;
        while parent[r] != r { r = parent[r]; }
        let mut c = x;
        while parent[c] != r { let next = parent[c]; parent[c] = r; c = next; }
        r
    }

    for a in 0..n {
        for b in (a + 1)..n {
            if cooccurrence[a * n + b] >= threshold {
                let ra = find(&mut parent, a);
                let rb = find(&mut parent, b);
                if ra != rb { parent[ra] = rb; }
            }
        }
    }

    // Assign cluster IDs from connected components (skip singletons)
    let mut component_sizes: HashMap<usize, usize> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        *component_sizes.entry(root).or_default() += 1;
    }

    let mut cluster_id_map: HashMap<usize, i32> = HashMap::new();
    let mut next_id = 0i32;
    let mut roots_by_size: Vec<(usize, usize)> = component_sizes.iter()
        .filter(|(_, &size)| size >= min_cluster_size)
        .map(|(&root, &size)| (root, size))
        .collect();
    roots_by_size.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (root, _) in roots_by_size {
        cluster_id_map.insert(root, next_id);
        next_id += 1;
    }

    // Compute per-track confidence
    let mut clusters = HashMap::with_capacity(n);
    let mut confidence = HashMap::with_capacity(n);
    for i in 0..n {
        let root = find(&mut parent, i);
        let cluster_id = cluster_id_map.get(&root).copied().unwrap_or(-1);
        clusters.insert(ids[i], cluster_id);

        if cluster_id >= 0 {
            let mut total_cooc = 0u32;
            let mut count = 0u32;
            for j in 0..n {
                if i == j { continue; }
                if find(&mut parent, j) == root {
                    let cooc = if i < j { cooccurrence[i * n + j] } else { cooccurrence[j * n + i] };
                    total_cooc += cooc as u32;
                    count += 1;
                }
            }
            let conf = if count > 0 {
                total_cooc as f32 / (count as f32 * num_runs as f32)
            } else {
                0.0
            };
            confidence.insert(ids[i], conf);
        } else {
            confidence.insert(ids[i], 0.0);
        }
    }

    // Derive cluster colors from spatial position (angle around center → hue)
    // This produces deterministic colors that match the visual layout.
    let mut colors = HashMap::new();
    let (global_cx, global_cy) = {
        let total_positions = positions.len() as f32;
        let (sx, sy) = positions.values().fold((0.0f32, 0.0f32), |(sx, sy), &(x, y)| (sx + x, sy + y));
        if total_positions > 0.0 { (sx / total_positions, sy / total_positions) } else { (0.0, 0.0) }
    };

    for (&_root, &cid) in &cluster_id_map {
        // Compute cluster centroid
        let mut cx = 0.0f32;
        let mut cy = 0.0f32;
        let mut count = 0u32;
        for i in 0..n {
            if clusters.get(&ids[i]).copied() == Some(cid) {
                let (x, y) = positions[&ids[i]];
                cx += x;
                cy += y;
                count += 1;
            }
        }
        if count > 0 {
            cx /= count as f32;
            cy /= count as f32;
        }

        // Angle from global center → hue (0..360)
        let angle = (cy - global_cy).atan2(cx - global_cx); // -π..π
        let hue = (angle.to_degrees() + 180.0) % 360.0; // 0..360

        // HSL to RGB (saturation=0.55, lightness=0.55 for muted, readable colors)
        let rgb = hsl_to_rgb(hue, 0.55, 0.55);
        colors.insert(cid, rgb);
    }

    let num_clusters = cluster_id_map.len();
    log::info!(
        "[GRAPH] Consensus clustering: {} communities from {} runs (min_cluster_size={}, threshold={}%, {} tracks)",
        num_clusters, num_runs, min_cluster_size, 70, n
    );

    // gate_passed is a placeholder here — the outer retry loop overwrites it
    // based on whether this particular roll cleared the quality gate.
    ClusterResult { clusters, confidence, colors, thresholds: CommunityThresholds::default(), gate_passed: false }
}

// ============================================================================
// Louvain community detection
// ============================================================================
//
// Modularity-based community detection on a k-NN graph of PCA distances.
// Unlike HDBSCAN (density-based, clusters on the 2D t-SNE layout), Louvain
// works directly on graph structure — so a densely-connected region that
// HDBSCAN would collapse into one mega-community can still be partitioned
// if modularity optimization finds meaningful sub-communities.
//
// Reference: Blondel et al. 2008, "Fast unfolding of communities in large
// networks", https://doi.org/10.1088/1742-5468/2008/10/P10008

/// Minimum tracks a macro-genre must contain to get level-2 clustering.
/// Smaller macros stay as a single community (leaf). Raised from 50 to 150
/// because the prior threshold over-fragmented small macros — e.g. Trance
/// with 73 tracks split into 6 sub-communities of ~9-18 tracks each, several
/// of which fell below the post-merge min_size and became orphan fragments.
/// 150 ensures recursion only fires for genuinely dominant macros where the
/// sub-structure is real (e.g. a 680-track DnB mass splitting into 5-8
/// liquid/neuro/techstep-shaped sub-communities).
const MACRO_RECURSE_MIN_SIZE: usize = 150;

/// Entry point: 2-level hierarchical clustering.
///
///   Level 1 (genre-driven): each track is mapped to a macro-genre bucket via
///     its Discogs EffNet label. Deterministic, semantically grounded. Only
///     macros that actually exist in the library become level-1 communities.
///
///   Level 2 (Louvain within each macro, if size >= 50): re-whitens the PCA
///     vectors of that macro's tracks only, builds a k-NN graph, and runs
///     multi-level Louvain to surface sub-genre structure. Macros below the
///     size threshold stay as a single leaf community.
///
/// Tracks missing ML analysis or with unmapped labels fall into the "Other"
/// macro. Leaf IDs are renumbered contiguously by descending size at the end.
pub fn run_louvain_clustering(
    pca_data: &[(i64, Vec<f32>)],
    positions: &HashMap<i64, (f32, f32)>,
    genre_labels: &HashMap<i64, String>,
) -> ClusterResult {
    run_louvain_clustering_with_l2(pca_data, positions, genre_labels, 0.8, 5)
}

/// Variant of `run_louvain_clustering` accepting L2 HDBSCAN parameter overrides.
pub fn run_louvain_clustering_with_l2(
    pca_data: &[(i64, Vec<f32>)],
    positions: &HashMap<i64, (f32, f32)>,
    genre_labels: &HashMap<i64, String>,
    l2_sqrt_factor: f32,
    l2_min_samples: usize,
) -> ClusterResult {
    let n = pca_data.len();
    if n < 10 {
        return ClusterResult {
            clusters: HashMap::new(),
            confidence: HashMap::new(),
            colors: HashMap::new(),
            thresholds: CommunityThresholds::default(),
            gate_passed: true,
        };
    }

    // Normalize + sort by track id for determinism
    let mut sorted: Vec<(i64, Vec<f32>)> = pca_data.iter()
        .map(|(id, v)| {
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-10);
            let normed: Vec<f32> = v.iter().map(|x| x / norm).collect();
            (*id, normed)
        })
        .collect();
    sorted.sort_by_key(|(id, _)| *id);
    let ids: Vec<i64> = sorted.iter().map(|(id, _)| *id).collect();

    // -------- Level 1: map each track to a macro-genre --------
    let mut macro_of_track: Vec<&'static str> = Vec::with_capacity(n);
    for id in &ids {
        let macro_name = match genre_labels.get(id) {
            Some(label) => crate::genre_map::macro_genre_for(label),
            None => "Other",
        };
        macro_of_track.push(macro_name);
    }

    // Count tracks per macro (log summary)
    let mut macro_counts: HashMap<&'static str, usize> = HashMap::new();
    for m in &macro_of_track { *macro_counts.entry(*m).or_default() += 1; }
    let mut macro_summary: Vec<(&&'static str, &usize)> = macro_counts.iter().collect();
    macro_summary.sort_by(|a, b| b.1.cmp(a.1));
    log::info!(
        "[GRAPH/LOUVAIN/L1] Macro-genre distribution ({} tracks): {}",
        n,
        macro_summary.iter()
            .map(|(m, c)| format!("{}={}", m, c))
            .collect::<Vec<_>>()
            .join(", "),
    );

    // -------- Level 2: within-macro Louvain for macros with enough tracks --------
    //
    // Each track is assigned a globally unique leaf id. Tracks in macros below
    // the recurse threshold share one leaf id per macro (the whole macro is
    // one community). Tracks in recursed macros get leaf ids that reflect the
    // sub-community the level-2 Louvain assigned them to.
    let mut leaf_of_track: Vec<usize> = vec![0; n];
    let mut next_leaf_id: usize = 0;

    // Iterate macros in stable order (by descending size, then name) for
    // deterministic leaf-id assignment across runs
    let mut ordered_macros: Vec<(&&'static str, &usize)> = macro_counts.iter().collect();
    ordered_macros.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

    for (macro_name, &size) in ordered_macros {
        // Collect indices of this macro
        let macro_indices: Vec<usize> = (0..n)
            .filter(|&i| &macro_of_track[i] == macro_name)
            .collect();
        if macro_indices.is_empty() { continue; }

        if size < MACRO_RECURSE_MIN_SIZE {
            // Single-leaf macro: all tracks in this macro get one leaf id
            let leaf_id = next_leaf_id;
            next_leaf_id += 1;
            for &idx in &macro_indices {
                leaf_of_track[idx] = leaf_id;
            }
            log::info!(
                "[GRAPH/LOUVAIN/L2] Macro '{}' ({} tracks): kept as single community (below threshold)",
                macro_name, size,
            );
            continue;
        }

        // Run HDBSCAN on the macro's 2D positions (cluster in 2D space
        // where t-SNE has already done variance-preserving separation).
        // This gives visually coherent sub-communities — every sub-community
        // is a contiguous 2D blob by construction. Louvain on k-NN PCA would
        // find modularity-optimal sub-communities that can be spatially
        // scattered.
        let subset_ids: Vec<i64> = macro_indices.iter().map(|&i| ids[i]).collect();
        let sub_labels = cluster_subset_hdbscan_with_params(
            &subset_ids, positions, l2_sqrt_factor, l2_min_samples,
        );

        // Collect unique sub-community ids (could be 1 if Louvain merged everything)
        let unique_subs: HashSet<usize> = sub_labels.iter().copied().collect();
        let mut sub_to_leaf: HashMap<usize, usize> = HashMap::new();
        for &sub in &unique_subs {
            sub_to_leaf.insert(sub, next_leaf_id);
            next_leaf_id += 1;
        }

        for (subset_pos, &outer_idx) in macro_indices.iter().enumerate() {
            let sub = sub_labels[subset_pos];
            leaf_of_track[outer_idx] = sub_to_leaf[&sub];
        }

        log::info!(
            "[GRAPH/LOUVAIN/L2] Macro '{}' ({} tracks) → {} sub-community{}",
            macro_name, size, unique_subs.len(),
            if unique_subs.len() == 1 { "" } else { "ies" },
        );
    }

    // -------- Renumber leaf ids contiguously by descending size --------
    let mut leaf_counts: HashMap<usize, usize> = HashMap::new();
    for &c in &leaf_of_track { *leaf_counts.entry(c).or_default() += 1; }
    let mut by_size: Vec<(usize, usize)> = leaf_counts.into_iter().collect();
    by_size.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut new_idx: HashMap<usize, i32> = HashMap::new();
    for (new_id, (old_id, _)) in by_size.iter().enumerate() {
        new_idx.insert(*old_id, new_id as i32);
    }

    let mut clusters: HashMap<i64, i32> = HashMap::with_capacity(n);
    let mut confidence: HashMap<i64, f32> = HashMap::with_capacity(n);
    for i in 0..n {
        let cid = new_idx.get(&leaf_of_track[i]).copied().unwrap_or(-1);
        clusters.insert(ids[i], cid);
        confidence.insert(ids[i], if cid >= 0 { 1.0 } else { 0.0 });
    }

    let colors = derive_cluster_colors(&clusters, positions);

    let num_clusters = new_idx.len();
    log::info!(
        "[GRAPH/LOUVAIN] {} leaf communities total (genre-driven L1 + Louvain L2 per macro, {} tracks)",
        num_clusters, n,
    );

    // Gate: the genre-based level 1 is deterministic, so this is now more a
    // sanity check than a retry signal. Still useful for downstream logging.
    let (num, largest_frac) = {
        let mut sizes: HashMap<i32, usize> = HashMap::new();
        for &cid in clusters.values() {
            if cid >= 0 { *sizes.entry(cid).or_default() += 1; }
        }
        let total_assigned: usize = sizes.values().sum();
        let largest = sizes.values().copied().max().unwrap_or(0);
        let frac = if total_assigned > 0 {
            largest as f32 / total_assigned as f32
        } else { 0.0 };
        (sizes.len(), frac)
    };
    let gate_passed = num >= MIN_COMMUNITIES_GATE && largest_frac <= MAX_LARGEST_FRACTION;

    ClusterResult {
        clusters,
        confidence,
        colors,
        thresholds: CommunityThresholds::default(),
        gate_passed,
    }
}

/// Run HDBSCAN on a subset's 2D positions. Returns sub-community labels
/// aligned with `subset_ids`. Noise points (label -1 from HDBSCAN) are
/// absorbed into their nearest non-noise community by centroid distance —
/// every track ends up assigned.
///
/// Clustering in 2D space gives visually coherent sub-communities: every
/// sub-community is a contiguous 2D blob by construction. The 2D layout
/// (t-SNE) has already done the dimensionality reduction work of preserving
/// meaningful spatial structure; HDBSCAN just finds density peaks.
fn cluster_subset_hdbscan(
    subset_ids: &[i64],
    positions: &HashMap<i64, (f32, f32)>,
) -> Vec<usize> {
    cluster_subset_hdbscan_with_params(subset_ids, positions, 0.8, 5)
}

/// Variant accepting tunable L2 HDBSCAN parameters. Used by the retry loop in
/// `run_consensus_clustering_cached` to vary the granularity until the overall
/// clustering passes the quality gate.
fn cluster_subset_hdbscan_with_params(
    subset_ids: &[i64],
    positions: &HashMap<i64, (f32, f32)>,
    sqrt_factor: f32,
    min_samples: usize,
) -> Vec<usize> {
    let m = subset_ids.len();
    if m < 20 { return vec![0; m]; } // trivially one community

    // Extract 2D positions in subset order. Tracks without positions get
    // placeholder (0,0) and will fall through to noise-absorb.
    let coords: Vec<Vec<f64>> = subset_ids.iter()
        .map(|id| {
            let (x, y) = positions.get(id).copied().unwrap_or((0.0, 0.0));
            vec![x as f64, y as f64]
        })
        .collect();

    // Scale HDBSCAN min_cluster_size to subset size. Default `sqrt_factor=0.8`
    // targets ~4-5 sub-communities per dominant macro. Smaller factors find
    // finer structure (more, smaller sub-communities) — used by retry loop.
    //
    //   m=680, factor=0.8 → 21 (default)
    //   m=680, factor=0.5 → 13 → clamped to 15 floor
    //   m=680, factor=0.3 → 7  → clamped to 5 floor (retry)
    //
    // A hard ceiling of MAX_SUB_COMMUNITIES is applied after clustering:
    // more than 6 sub-communities per macro doesn't match how DJs think
    // about sub-styles regardless of library size.
    let min_cluster_size = ((m as f32).sqrt() * sqrt_factor) as usize;
    let min_cluster_size = min_cluster_size.clamp(5, 50);
    const MAX_SUB_COMMUNITIES: usize = 6;

    log::debug!(
        "[GRAPH/HDBSCAN/L2] subset n={}, min_cluster_size={}, min_samples={}",
        m, min_cluster_size, min_samples,
    );

    let hp = hdbscan::HdbscanHyperParams::builder()
        .min_cluster_size(min_cluster_size)
        .min_samples(min_samples)
        .build();
    let clusterer = hdbscan::Hdbscan::new(&coords, hp);
    let labels: Vec<i32> = match clusterer.cluster() {
        Ok(l) => l,
        Err(e) => {
            log::warn!("[GRAPH/HDBSCAN/L2] HDBSCAN failed: {} — treating subset as one community", e);
            return vec![0; m];
        }
    };

    // Build centroid map for non-noise communities, tracking size
    let mut sums: HashMap<i32, (f32, f32, u32)> = HashMap::new();
    for (i, &c) in labels.iter().enumerate() {
        if c < 0 { continue; }
        if let Some(&(x, y)) = positions.get(&subset_ids[i]) {
            let e = sums.entry(c).or_insert((0.0, 0.0, 0));
            e.0 += x; e.1 += y; e.2 += 1;
        }
    }

    // Hard cap at MAX_SUB_COMMUNITIES: if HDBSCAN returned more clusters
    // than the cap, keep only the top-K largest. Tracks in the discarded
    // smaller clusters fall through to the noise-absorb path and get
    // reassigned to their nearest surviving centroid. Matches DJ intuition
    // that no macro-genre has more than ~6 useful sub-styles.
    let total_before = sums.len();
    let kept_labels: HashSet<i32> = if sums.len() > MAX_SUB_COMMUNITIES {
        let mut by_size: Vec<(i32, u32)> = sums.iter()
            .map(|(&c, &(_, _, n))| (c, n))
            .collect();
        by_size.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        by_size.truncate(MAX_SUB_COMMUNITIES);
        by_size.into_iter().map(|(c, _)| c).collect()
    } else {
        sums.keys().copied().collect()
    };
    if total_before > MAX_SUB_COMMUNITIES {
        log::info!(
            "[GRAPH/HDBSCAN/L2] Capping {} HDBSCAN clusters → top {} (others merge via nearest-centroid)",
            total_before, MAX_SUB_COMMUNITIES,
        );
    }

    let centroids: HashMap<i32, (f32, f32)> = sums.into_iter()
        .filter(|(c, _)| kept_labels.contains(c))
        .filter_map(|(c, (sx, sy, n))| if n > 0 { Some((c, (sx / n as f32, sy / n as f32))) } else { None })
        .collect();

    if centroids.is_empty() {
        // HDBSCAN found no clusters (all noise) — treat subset as one community
        return vec![0; m];
    }

    // Assign each track:
    //   - noise (-1): nearest surviving centroid
    //   - kept cluster: its own label
    //   - discarded cluster (exceeded cap): nearest surviving centroid
    let mut noise_absorbed = 0usize;
    let mut capped_reassigned = 0usize;
    let result: Vec<usize> = (0..m).map(|i| {
        let c = labels[i];
        if c >= 0 && kept_labels.contains(&c) { return c as usize; }
        if c < 0 { noise_absorbed += 1; } else { capped_reassigned += 1; }
        let pos = positions.get(&subset_ids[i]).copied().unwrap_or((0.0, 0.0));
        let nearest = centroids.iter()
            .min_by(|a, b| {
                let da = (pos.0 - a.1.0).powi(2) + (pos.1 - a.1.1).powi(2);
                let db = (pos.0 - b.1.0).powi(2) + (pos.1 - b.1.1).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(c, _)| *c as usize)
            .unwrap_or(0);
        nearest
    }).collect();

    if noise_absorbed > 0 || capped_reassigned > 0 {
        log::debug!(
            "[GRAPH/HDBSCAN/L2] absorbed {} noise + {} cap-truncated tracks into nearest surviving community",
            noise_absorbed, capped_reassigned,
        );
    }

    result
}

/// Shared color derivation (used by both HDBSCAN and Louvain paths).
fn derive_cluster_colors(
    clusters: &HashMap<i64, i32>,
    positions: &HashMap<i64, (f32, f32)>,
) -> HashMap<i32, [f32; 3]> {
    let total_positions = positions.len() as f32;
    let (global_cx, global_cy) = {
        let (sx, sy) = positions.values()
            .fold((0.0f32, 0.0f32), |(sx, sy), &(x, y)| (sx + x, sy + y));
        if total_positions > 0.0 { (sx / total_positions, sy / total_positions) } else { (0.0, 0.0) }
    };

    // Gather per-cluster centroids
    let mut sums: HashMap<i32, (f32, f32, u32)> = HashMap::new();
    for (&id, &cid) in clusters {
        if cid < 0 { continue; }
        if let Some(&(x, y)) = positions.get(&id) {
            let e = sums.entry(cid).or_insert((0.0, 0.0, 0));
            e.0 += x; e.1 += y; e.2 += 1;
        }
    }

    let mut colors = HashMap::new();
    for (cid, (sx, sy, count)) in sums {
        if count == 0 { continue; }
        let cx = sx / count as f32;
        let cy = sy / count as f32;
        let angle = (cy - global_cy).atan2(cx - global_cx);
        let hue = (angle.to_degrees() + 180.0) % 360.0;
        colors.insert(cid, hsl_to_rgb(hue, 0.55, 0.55));
    }
    colors
}

/// Compute dynamic similarity thresholds as percentile ranks.
///
/// Since the suggestion algorithm uses percentile-rank normalization (rank / N),
/// the thresholds must also be on the percentile-rank scale [0, 1].
///
/// Approach: sample pairwise distances, sort them ALL into one pool, then find
/// what percentile rank corresponds to the intra/inter community boundaries:
/// - Tight target: percentile rank where 75% of intra-community pairs fall below
/// - Open target: percentile rank of the median inter-community distance
/// - Medium: midpoint
pub fn compute_community_thresholds(
    pca_data: &[(i64, Vec<f32>)],
    clusters: &HashMap<i64, i32>,
) -> CommunityThresholds {
    use crate::suggestions::query::cosine_distance_pub;

    if pca_data.len() < 20 {
        return CommunityThresholds::default();
    }

    let pca_map: HashMap<i64, &Vec<f32>> = pca_data.iter().map(|(id, v)| (*id, v)).collect();

    // Sample pairwise distances, tracking whether each pair is intra or inter community
    let ids: Vec<i64> = pca_data.iter().map(|(id, _)| *id).collect();
    let n = ids.len();
    let step = (n * n / 10000).max(1);
    let mut pair_count = 0usize;

    // All sampled distances (for percentile ranking)
    let mut all_distances: Vec<f32> = Vec::new();
    // Intra-community distances (subset of all)
    let mut intra_distances: Vec<f32> = Vec::new();
    // Inter-community distances (subset of all)
    let mut inter_distances: Vec<f32> = Vec::new();

    for i in 0..n {
        for j in (i + 1)..n {
            pair_count += 1;
            if pair_count % step != 0 { continue; }

            let (va, vb) = match (pca_map.get(&ids[i]), pca_map.get(&ids[j])) {
                (Some(a), Some(b)) => (a, b),
                _ => continue,
            };
            let dist = cosine_distance_pub(va, vb);
            all_distances.push(dist);

            let ca = clusters.get(&ids[i]).copied().unwrap_or(-1);
            let cb = clusters.get(&ids[j]).copied().unwrap_or(-1);

            if ca >= 0 && ca == cb {
                intra_distances.push(dist);
            } else if ca >= 0 && cb >= 0 {
                inter_distances.push(dist);
            }
        }
    }

    if intra_distances.is_empty() || inter_distances.is_empty() || all_distances.is_empty() {
        return CommunityThresholds::default();
    }

    all_distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    intra_distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    inter_distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let total = all_distances.len();

    // Helper: find the percentile rank of a raw distance in the full pool
    let to_percentile = |raw_dist: f32| -> f32 {
        let pos = all_distances.partition_point(|&d| d < raw_dist);
        pos as f32 / (total as f32 - 1.0).max(1.0)
    };

    // Tight: 75th percentile of intra-community raw distances → convert to percentile rank
    let intra_p75 = intra_distances[(intra_distances.len() * 3 / 4).min(intra_distances.len() - 1)];
    let tight_target = to_percentile(intra_p75);

    // Open: median of inter-community raw distances → convert to percentile rank
    let inter_median = inter_distances[inter_distances.len() / 2];
    let open_target = to_percentile(inter_median);

    // Medium: midpoint in percentile space
    let medium_target = (tight_target + open_target) / 2.0;

    // Bell widths proportional to the gap between tight and open targets.
    // The full useful range is (open - tight). Each mode gets a fraction:
    // tight = narrow (15% of range), medium = moderate (25%), open = wide (40%).
    let range = (open_target - tight_target).max(0.05);
    let tight_width = (range * 0.15).clamp(0.008, 0.05);
    let medium_width = (range * 0.25).clamp(0.015, 0.08);
    let open_width = (range * 0.40).clamp(0.025, 0.12);

    let thresholds = CommunityThresholds {
        tight_target, tight_width,
        medium_target, medium_width,
        open_target, open_width,
    };

    eprintln!("[GRAPH] Community thresholds (percentile-rank): tight={:.3} (w={:.3}), medium={:.3} (w={:.3}), open={:.3} (w={:.3})",
        tight_target, tight_width, medium_target, medium_width, open_target, open_width);
    eprintln!("[GRAPH] Raw distance boundaries: intra_p75={:.4}, inter_median={:.4}",
        intra_p75, inter_median);
    eprintln!("[GRAPH] Sampled {} total, {} intra, {} inter pairs",
        all_distances.len(), intra_distances.len(), inter_distances.len());

    thresholds
}

/// Convert HSL to RGB. h in [0, 360), s and l in [0, 1].
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h_prime as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r1 + m, g1 + m, b1 + m]
}
