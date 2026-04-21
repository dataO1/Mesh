//! Shared graph layout and clustering computation for both mesh-cue and mesh-player.
//!
//! - **t-SNE**: Barnes-Hut t-SNE via bhtsne to project high-dim PCA embeddings to 2D
//! - **Consensus clustering**: Multi-scale HDBSCAN for robust community detection

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::db::DatabaseService;

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
#[derive(Debug, Clone)]
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
#[derive(Debug)]
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
}

/// Dispatch to the selected layout algorithm.
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

/// Multi-scale consensus clustering on 2D positions.
///
/// Runs HDBSCAN at 7 different min_samples values and builds a co-occurrence
/// matrix. Tracks that consistently cluster together (>= 70% of runs) are
/// connected via union-find into robust communities.
///
/// Cluster colors are derived from each community's average position in the
/// 2D space — mapped to a perceptual hue wheel. This produces deterministic
/// colors that correlate with the spatial layout.
pub fn run_consensus_clustering(
    positions: &HashMap<i64, (f32, f32)>,
) -> ClusterResult {
    let n = positions.len();
    if n < 10 {
        return ClusterResult {
            clusters: HashMap::new(),
            confidence: HashMap::new(),
            colors: HashMap::new(),
            thresholds: CommunityThresholds::default(),
        };
    }

    let ids: Vec<i64> = positions.keys().copied().collect();
    let data: Vec<Vec<f64>> = ids.iter()
        .filter_map(|id| positions.get(id))
        .map(|&(x, y)| vec![x as f64, y as f64])
        .collect();

    // Run HDBSCAN at multiple scales
    let scales = [1usize, 2, 4, 6, 9, 13, 18];
    let mut all_labels: Vec<Vec<i32>> = Vec::new();
    let min_cluster_size = (n / 100).max(5).min(20);

    for &min_samples in &scales {
        let hp = hdbscan::HdbscanHyperParams::builder()
            .min_cluster_size(min_cluster_size)
            .min_samples(min_samples)
            .build();
        let clusterer = hdbscan::Hdbscan::new(&data, hp);
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
    for (&root, &size) in &component_sizes {
        if size >= min_cluster_size {
            cluster_id_map.insert(root, next_id);
            next_id += 1;
        }
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
        let (sx, sy) = positions.values().fold((0.0f32, 0.0f32), |(sx, sy), &(x, y)| (sx + x, sy + y));
        (sx / n as f32, sy / n as f32)
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

    ClusterResult { clusters, confidence, colors, thresholds: CommunityThresholds::default() }
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

    // Bell widths in percentile-rank space
    let tight_width = ((tight_target * 0.4).max(0.03)).min(0.15);
    let medium_width = (((open_target - tight_target) * 0.3).max(0.05)).min(0.20);
    let open_width = ((open_target * 0.3).max(0.06)).min(0.25);

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
