//! Shared graph layout and clustering computation for both mesh-cue and mesh-player.
//!
//! - **t-SNE**: Barnes-Hut t-SNE via bhtsne to project high-dim PCA embeddings to 2D
//! - **Consensus clustering**: Multi-scale HDBSCAN for robust community detection

use std::collections::HashMap;

/// Result of consensus clustering.
pub struct ClusterResult {
    /// Cluster assignments (track_id -> cluster_id, -1 = noise)
    pub clusters: HashMap<i64, i32>,
    /// Per-track confidence [0.0, 1.0] — how consistently this track
    /// clusters with its peers across multiple HDBSCAN scales
    pub confidence: HashMap<i64, f32>,
    /// Cluster colors as [r, g, b] floats (0..1). Converted to iced::Color on the UI side.
    pub colors: HashMap<i32, [f32; 3]>,
}

// 12-color palette for cluster visualization
const CLUSTER_PALETTE: [[f32; 3]; 12] = [
    [0.27, 0.53, 0.80],
    [0.80, 0.40, 0.27],
    [0.33, 0.70, 0.40],
    [0.73, 0.33, 0.73],
    [0.80, 0.73, 0.27],
    [0.27, 0.73, 0.73],
    [0.87, 0.47, 0.53],
    [0.53, 0.53, 0.80],
    [0.60, 0.40, 0.30],
    [0.40, 0.75, 0.55],
    [0.85, 0.55, 0.25],
    [0.50, 0.30, 0.70],
];

/// Run Barnes-Hut t-SNE on PCA embeddings to produce 2D positions.
///
/// Input: slice of (track_id, pca_vector). Returns track_id -> (x, y).
/// Perplexity scales with dataset size: sqrt(n)/2 clamped to [5, 50].
/// 750 epochs, theta=0.5, Euclidean distance.
pub fn compute_tsne_layout(
    pca_data: &[(i64, Vec<f32>)],
    normalize: bool,
) -> HashMap<i64, (f32, f32)> {
    if pca_data.len() < 10 {
        log::warn!("[GRAPH] Not enough PCA embeddings for t-SNE ({})", pca_data.len());
        return HashMap::new();
    }

    let n = pca_data.len();
    log::info!("[GRAPH] Running Barnes-Hut t-SNE on {} tracks → 2D...", n);

    let ids: Vec<i64> = pca_data.iter().map(|(id, _)| *id).collect();

    let owned_vecs: Vec<Vec<f32>> = if normalize {
        pca_data.iter().map(|(_, pca)| {
            let mut v = pca.clone();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 1e-10 { for x in v.iter_mut() { *x /= norm; } }
            v
        }).collect()
    } else {
        pca_data.iter().map(|(_, pca)| pca.clone()).collect()
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
    let mut positions = HashMap::with_capacity(n);
    for (i, &id) in ids.iter().enumerate() {
        positions.insert(id, (embedding[i * 2], embedding[i * 2 + 1]));
    }

    log::info!("[GRAPH] t-SNE complete — {} 2D positions", positions.len());
    positions
}

/// Multi-scale consensus clustering on 2D positions.
///
/// Runs HDBSCAN at 7 different min_samples values and builds a co-occurrence
/// matrix. Tracks that consistently cluster together (>= 70% of runs) are
/// connected via union-find into robust communities.
pub fn run_consensus_clustering(
    positions: &HashMap<i64, (f32, f32)>,
) -> ClusterResult {
    let n = positions.len();
    if n < 10 {
        return ClusterResult {
            clusters: HashMap::new(),
            confidence: HashMap::new(),
            colors: HashMap::new(),
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

    // Assign colors from palette
    let mut colors = HashMap::new();
    for (&_root, &cid) in &cluster_id_map {
        colors.insert(cid, CLUSTER_PALETTE[cid as usize % CLUSTER_PALETTE.len()]);
    }

    let num_clusters = cluster_id_map.len();
    log::info!(
        "[GRAPH] Consensus clustering: {} communities from {} runs (min_cluster_size={}, threshold={}%, {} tracks)",
        num_clusters, num_runs, min_cluster_size, 70, n
    );

    ClusterResult { clusters, confidence, colors }
}
