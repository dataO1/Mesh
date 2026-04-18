//! Force-directed layout engine (ForceAtlas2-inspired).
//!
//! Implements attraction, repulsion and gravity forces for graph spatialization.
//! No external crate dependency (forceatlas2 requires nightly Rust).

use std::collections::HashMap;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

// ════════════════════════════════════════════════════════════════════════════
// Settings
// ════════════════════════════════════════════════════════════════════════════

/// Configuration for the force-directed layout.
#[derive(Debug, Clone)]
pub struct LayoutSettings {
    /// Attraction coefficient (edges pull connected nodes together)
    pub ka: f32,
    /// Repulsion coefficient (all nodes push each other apart)
    pub kr: f32,
    /// Gravity coefficient (pulls nodes toward center)
    pub kg: f32,
    /// Whether gravity is distance-independent (more compact)
    pub strong_gravity: bool,
    /// Speed damping factor (lower = more stable, higher = faster convergence)
    pub speed: f32,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            ka: 1.0,
            kr: 1.0,
            kg: 1.0,
            strong_gravity: false,
            speed: 1.0,
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Graph layout state
// ════════════════════════════════════════════════════════════════════════════

/// Graph layout state -- positions for all nodes.
pub struct GraphLayout {
    /// (track_id, x, y) for each node
    pub positions: Vec<(i64, f32, f32)>,
    pub iteration: usize,
    pub converged: bool,
    /// Map from track_id to index in positions vec (used by callers via `position_map()`)
    #[allow(dead_code)]
    id_to_index: HashMap<i64, usize>,
    /// Edges as (from_index, to_index, weight)
    indexed_edges: Vec<(usize, usize, f32)>,
    /// Settings
    settings: LayoutSettings,
}

impl GraphLayout {
    /// Access positions as a map for convenient lookup.
    pub fn position_map(&self) -> HashMap<i64, (f32, f32)> {
        self.positions.iter().map(|&(id, x, y)| (id, (x, y))).collect()
    }

    /// Replace the edge set and reset convergence, keeping existing positions.
    ///
    /// Used when the seed/slider changes: the suggestion algorithm produces new
    /// edges (seed → suggestions weighted by composite score) and the layout
    /// re-converges to show better matches closer to the seed.
    pub fn update_edges(&mut self, edges: &[(i64, i64, f32)]) {
        self.indexed_edges = edges
            .iter()
            .filter_map(|&(from, to, dist)| {
                let from_idx = self.id_to_index.get(&from)?;
                let to_idx = self.id_to_index.get(&to)?;
                let weight = 1.0 / (1.0 + dist);
                Some((*from_idx, *to_idx, weight))
            })
            .collect();
        self.converged = false;
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Layout construction
// ════════════════════════════════════════════════════════════════════════════

/// Build initial layout from nodes and edges.
///
/// Assigns random positions in [-1, 1] square, then runs 50 warm-up iterations.
pub fn build_initial_layout(
    node_ids: &[i64],
    edges: &[(i64, i64, f32)], // (from, to, weight)
) -> GraphLayout {
    build_initial_layout_with_settings(node_ids, edges, LayoutSettings::default())
}

/// Build initial layout with custom settings.
pub fn build_initial_layout_with_settings(
    node_ids: &[i64],
    edges: &[(i64, i64, f32)],
    settings: LayoutSettings,
) -> GraphLayout {
    if node_ids.is_empty() {
        return GraphLayout {
            positions: Vec::new(),
            iteration: 0,
            converged: true,
            id_to_index: HashMap::new(),
            indexed_edges: Vec::new(),
            settings,
        };
    }

    // Map track IDs to 0..N indices
    let id_to_index: HashMap<i64, usize> = node_ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();

    // Build edge list as index triples.
    // Convert distance to attraction weight: closer tracks attract more.
    let indexed_edges: Vec<(usize, usize, f32)> = edges
        .iter()
        .filter_map(|&(from, to, dist)| {
            let from_idx = id_to_index.get(&from)?;
            let to_idx = id_to_index.get(&to)?;
            let weight = 1.0 / (1.0 + dist);
            Some((*from_idx, *to_idx, weight))
        })
        .collect();

    // Random initial positions in [-1, 1]
    let mut rng = SmallRng::seed_from_u64(42);
    let positions: Vec<(i64, f32, f32)> = node_ids
        .iter()
        .map(|&id| {
            let x: f32 = rng.gen_range(-1.0..1.0);
            let y: f32 = rng.gen_range(-1.0..1.0);
            (id, x, y)
        })
        .collect();

    let mut layout = GraphLayout {
        positions,
        iteration: 0,
        converged: false,
        id_to_index,
        indexed_edges,
        settings,
    };

    // Run warm-up iterations
    iterate_layout(&mut layout, 50);

    layout
}

// ════════════════════════════════════════════════════════════════════════════
// Layout iteration
// ════════════════════════════════════════════════════════════════════════════

/// Run N iterations of the force-directed layout algorithm.
///
/// Mutates positions in place. Sets `converged = true` when position changes
/// are below a threshold.
pub fn iterate_layout(
    layout: &mut GraphLayout,
    steps: usize,
) {
    if layout.positions.is_empty() || layout.converged {
        return;
    }

    let n = layout.positions.len();
    let settings = &layout.settings;
    let ka = settings.ka;
    let kr = settings.kr;
    let kg = settings.kg;
    let strong_gravity = settings.strong_gravity;
    let speed = settings.speed;

    // Precompute degree (number of edges per node) for attraction scaling
    let mut degree = vec![0u32; n];
    for &(a, b, _) in &layout.indexed_edges {
        degree[a] += 1;
        degree[b] += 1;
    }

    let mut max_delta: f32 = 0.0;

    for _ in 0..steps {
        // Accumulate forces
        let mut forces = vec![(0.0f32, 0.0f32); n];

        // Repulsion: all pairs (O(n^2) for small graphs, sufficient for <10k nodes)
        // ForceAtlas2 repulsion: F = kr * (deg(i)+1) * (deg(j)+1) / dist
        for i in 0..n {
            let (_, xi, yi) = layout.positions[i];
            for j in (i + 1)..n {
                let (_, xj, yj) = layout.positions[j];
                let dx = xi - xj;
                let dy = yi - yj;
                let dist_sq = dx * dx + dy * dy;
                let dist = dist_sq.sqrt().max(0.001);

                let mass_i = (degree[i] + 1) as f32;
                let mass_j = (degree[j] + 1) as f32;
                let force = kr * mass_i * mass_j / dist;

                let fx = force * dx / dist;
                let fy = force * dy / dist;

                forces[i].0 += fx;
                forces[i].1 += fy;
                forces[j].0 -= fx;
                forces[j].1 -= fy;
            }
        }

        // Attraction: along edges
        // ForceAtlas2 attraction: F = ka * weight * dist
        for &(a, b, weight) in &layout.indexed_edges {
            let (_, xa, ya) = layout.positions[a];
            let (_, xb, yb) = layout.positions[b];
            let dx = xb - xa;
            let dy = yb - ya;
            let dist = (dx * dx + dy * dy).sqrt().max(0.001);

            let force = ka * weight * dist;
            let fx = force * dx / dist;
            let fy = force * dy / dist;

            forces[a].0 += fx;
            forces[a].1 += fy;
            forces[b].0 -= fx;
            forces[b].1 -= fy;
        }

        // Gravity: pull toward center
        for i in 0..n {
            let (_, x, y) = layout.positions[i];
            let dist = (x * x + y * y).sqrt().max(0.001);
            let mass = (degree[i] + 1) as f32;

            if strong_gravity {
                // Distance-independent gravity
                forces[i].0 -= kg * mass * x / dist;
                forces[i].1 -= kg * mass * y / dist;
            } else {
                // Standard gravity (weaker with distance)
                forces[i].0 -= kg * mass * x;
                forces[i].1 -= kg * mass * y;
            }
        }

        // Apply forces with adaptive speed (simplified ForceAtlas2 speed control)
        let mut step_max_delta: f32 = 0.0;
        for i in 0..n {
            let mass = (degree[i] + 1) as f32;
            // Speed is inversely proportional to node mass for stability
            let node_speed = speed / mass;

            let fx = forces[i].0;
            let fy = forces[i].1;
            let force_mag = (fx * fx + fy * fy).sqrt().max(0.001);

            // Limit displacement to avoid instability
            let max_disp = 10.0;
            let disp = (node_speed * force_mag).min(max_disp);

            let dx = disp * fx / force_mag;
            let dy = disp * fy / force_mag;

            layout.positions[i].1 += dx;
            layout.positions[i].2 += dy;

            let delta = (dx * dx + dy * dy).sqrt();
            if delta > step_max_delta {
                step_max_delta = delta;
            }
        }

        max_delta = step_max_delta;
        layout.iteration += 1;
    }

    // Converged when the largest position change is negligible
    if max_delta < 0.001 {
        layout.converged = true;
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Radial score-based layout
// ════════════════════════════════════════════════════════════════════════════

/// Build a radial layout where distance from center = suggestion score,
/// and angle is derived from PCA embedding components for natural clustering.
///
/// Seed is placed at origin. Each other node's position is:
/// - **Radius** = suggestion score (lower penalty = closer to seed)
/// - **Angle** = atan2(pca[1], pca[0]) of the track's PCA embedding relative to
///   the seed's embedding. Tracks with similar spectral profiles naturally cluster
///   at similar angles.
///
/// `seed_id`: the seed track (placed at origin).
/// `scored_tracks`: `(track_id, score, pca_vec)` sorted ascending by score.
/// `seed_pca`: seed's PCA embedding (used to compute relative angles).
///
/// Returns `(track_id, x, y)` for every scored node + seed.
pub fn radial_layout(
    seed_id: i64,
    scored_tracks: &[(i64, f32, Vec<f32>)],
    seed_pca: &[f32],
) -> Vec<(i64, f32, f32)> {
    let mut positions = Vec::with_capacity(scored_tracks.len() + 1);

    // Seed at center
    positions.push((seed_id, 0.0, 0.0));

    // Min-max normalize scores to [0, 1] for visual spread.
    // Reward scoring: higher = better match = closer to seed.
    // After normalization: best → radius 0.03, worst → radius 1.0.
    let min_score = scored_tracks.iter()
        .filter(|(id, _, _)| *id != seed_id)
        .map(|(_, s, _)| *s)
        .fold(f32::MAX, f32::min);
    let max_score = scored_tracks.iter()
        .filter(|(id, _, _)| *id != seed_id)
        .map(|(_, s, _)| *s)
        .fold(f32::MIN, f32::max);
    let score_range = (max_score - min_score).max(0.001);

    for &(id, score, ref pca) in scored_tracks {
        if id == seed_id {
            continue;
        }

        // Invert: high score (good) → small radius (close to seed)
        let norm = 1.0 - (score - min_score) / score_range; // 1.0 = worst, 0.0 = best
        let radius = 0.03 + norm.sqrt() * 0.97;

        // Angle from PCA: compute difference vector (track - seed) in PCA space,
        // then use atan2 of the first two components. Tracks with similar spectral
        // profiles relative to the seed get similar angles → natural clustering.
        let angle = if pca.len() >= 2 && seed_pca.len() >= 2 {
            let dx = pca[0] - seed_pca[0];
            let dy = pca[1] - seed_pca[1];
            dy.atan2(dx)
        } else {
            // Fallback: deterministic spread from track ID
            (id as f32 * 2.399_963) % std::f32::consts::TAU
        };

        let x = radius * angle.cos();
        let y = radius * angle.sin();
        positions.push((id, x, y));
    }

    positions
}
