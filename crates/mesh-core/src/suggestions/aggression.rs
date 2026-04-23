//! PCA aggression axis computation and scoring.
//!
//! Computes a per-track aggression estimate from genre labels + mood tags,
//! then finds the PCA dimension most correlated with this estimate.
//!
//! Also provides user-feedback calibration: pairwise comparisons learn a weight
//! vector via logistic regression on PCA difference vectors.

use std::collections::HashMap;
use std::collections::HashSet;

/// Compute a genre-based aggression score from a Discogs genre label.
/// Returns a value in [0.0, 0.75] representing coarse genre aggression.
pub fn genre_aggression_score(genre: &str) -> f32 {
    // Strip "SuperGenre---SubGenre" to get the sub-genre
    let sub = genre.split("---").last().unwrap_or(genre);
    let sub_lower = sub.to_lowercase();
    let full_lower = genre.to_lowercase();

    // Check super-genre categories first for broad classification
    if full_lower.starts_with("children") || full_lower.starts_with("non-music") { return 0.00; }

    if full_lower.starts_with("classical") {
        return if sub_lower.contains("baroque") || sub_lower.contains("choral")
            || sub_lower.contains("renaissance") || sub_lower.contains("medieval")
            || sub_lower.contains("romantic") || sub_lower.contains("opera") { 0.05 } else { 0.10 };
    }

    if full_lower.starts_with("folk") || full_lower.starts_with("latin") { return 0.10; }

    if full_lower.starts_with("jazz") {
        return if sub_lower.contains("smooth") || sub_lower.contains("easy listening")
            || sub_lower.contains("bossa") || sub_lower.contains("cool jazz")
            || sub_lower.contains("swing") || sub_lower.contains("dixieland") { 0.15 }
        else { 0.25 };
    }

    if full_lower.starts_with("stage") { return 0.20; }

    if full_lower.starts_with("pop") {
        return if sub_lower.contains("ballad") || sub_lower.contains("light")
            || sub_lower.contains("chanson") || sub_lower.contains("vocal") { 0.15 }
        else { 0.28 };
    }

    if full_lower.starts_with("blues") { return 0.30; }

    if full_lower.starts_with("funk") || full_lower.starts_with("soul") {
        return if sub_lower.contains("soul") || sub_lower.contains("neo soul")
            || sub_lower.contains("boogie") || sub_lower.contains("disco")
            || sub_lower.contains("r&b") { 0.28 }
        else { 0.35 };
    }

    if full_lower.starts_with("reggae") {
        return if sub_lower.contains("lovers") || sub_lower.contains("pop")
            || sub_lower.contains("rocksteady") || sub_lower.contains("dub") { 0.20 }
        else { 0.35 };
    }

    // Hip Hop block (under rock)
    if full_lower.starts_with("hip hop") {
        return if sub_lower.contains("jazzy") || sub_lower.contains("instrumental")
            || sub_lower.contains("trip hop") || sub_lower.contains("cloud")
            || sub_lower.contains("conscious") || sub_lower.contains("pop rap") { 0.30 }
        else if sub_lower.contains("horrorcore") || sub_lower.contains("britcore") { 0.44 }
        else { 0.42 }; // boom bap, gangsta, crunk, trap, hardcore hip-hop
    }

    // Rock block
    if full_lower.starts_with("rock") {
        // Soft rock
        if sub_lower.contains("acoustic") || sub_lower.contains("soft rock")
            || sub_lower.contains("folk rock") || sub_lower.contains("dream pop")
            || sub_lower.contains("shoegaze") || sub_lower.contains("lo-fi")
            || sub_lower.contains("post rock") { return 0.30; }
        // Indie/classic
        if sub_lower.contains("indie") || sub_lower.contains("brit pop")
            || sub_lower.contains("pop rock") || sub_lower.contains("aor")
            || sub_lower.contains("classic rock") || sub_lower.contains("prog rock") { return 0.30; }
        // Mid rock
        if sub_lower.contains("blues rock") || sub_lower.contains("country rock")
            || sub_lower.contains("southern") || sub_lower.contains("psychedelic")
            || sub_lower.contains("garage") || sub_lower.contains("grunge")
            || sub_lower.contains("punk") || sub_lower.contains("emo")
            || sub_lower.contains("mod") { return 0.40; }
        // Hard rock
        if sub_lower.contains("hard rock") || sub_lower.contains("arena")
            || sub_lower.contains("stoner") { return 0.43; }
        // Heavy metal
        if sub_lower.contains("heavy metal") || sub_lower.contains("power metal")
            || sub_lower.contains("thrash") || sub_lower.contains("speed metal")
            || sub_lower.contains("nu metal") || sub_lower.contains("sludge")
            || sub_lower.contains("doom metal") { return 0.62; }
        // Extreme metal
        if sub_lower.contains("metalcore") || sub_lower.contains("deathcore")
            || sub_lower.contains("post-hardcore") || sub_lower.contains("melodic hardcore")
            || sub_lower.contains("hardcore") { return 0.65; }
        if sub_lower.contains("black metal") || sub_lower.contains("death metal")
            || sub_lower.contains("technical death") || sub_lower.contains("melodic death") { return 0.70; }
        if sub_lower.contains("grindcore") || sub_lower.contains("goregrind")
            || sub_lower.contains("pornogrind") || sub_lower.contains("noisecore")
            || sub_lower.contains("power violence") || sub_lower.contains("crust") { return 0.70; }
        if sub_lower.contains("atmospheric") || sub_lower.contains("depressive")
            || sub_lower.contains("funeral") || sub_lower.contains("viking") { return 0.72; }
        // Default rock
        return 0.40;
    }

    // Electronic block
    if full_lower.starts_with("electronic") || full_lower.contains("electro") {
        // Ambient/chill
        if sub_lower.contains("ambient") || sub_lower.contains("new age")
            || sub_lower.contains("downtempo") || sub_lower.contains("chillwave") { return 0.20; }
        if sub_lower.contains("dark ambient") || sub_lower.contains("drone")
            || sub_lower.contains("berlin-school") || sub_lower.contains("dungeon") { return 0.25; }
        // Deep/mellow electronic
        if sub_lower.contains("trip hop") || sub_lower.contains("deep house")
            || sub_lower.contains("dub techno") || sub_lower.contains("leftfield")
            || sub_lower.contains("broken beat") { return 0.28; }
        // Synth/experimental
        if sub_lower.contains("synth-pop") || sub_lower.contains("synthwave")
            || sub_lower.contains("vaporwave") || sub_lower.contains("electroclash")
            || sub_lower.contains("dance-pop") { return 0.32; }
        if sub_lower.contains("idm") || sub_lower.contains("glitch")
            || sub_lower.contains("experimental") || sub_lower.contains("abstract")
            || sub_lower.contains("musique") { return 0.32; }
        // House/disco
        if sub_lower.contains("house") || sub_lower.contains("disco")
            || sub_lower.contains("nu-disco") { return 0.35; }
        // Electro/EBM
        if sub_lower.contains("electro") || sub_lower.contains("ebm") { return 0.37; }
        // Techno/trance/dubstep
        if sub_lower.contains("techno") || sub_lower.contains("minimal")
            || sub_lower.contains("tech house") { return 0.47; }
        if sub_lower.contains("trance") || sub_lower.contains("goa") { return 0.47; }
        if sub_lower.contains("dubstep") || sub_lower.contains("bassline")
            || sub_lower.contains("grime") || sub_lower.contains("uk garage")
            || sub_lower.contains("speed garage") { return 0.47; }
        // Hard electronic
        if sub_lower.contains("hard house") || sub_lower.contains("hard trance")
            || sub_lower.contains("hard techno") || sub_lower.contains("schranz") { return 0.50; }
        // Breakbeat (just under DnB)
        if sub_lower.contains("breakbeat") || sub_lower.contains("breaks")
            || sub_lower.contains("progressive breaks") || sub_lower.contains("big beat") { return 0.53; }
        // DnB — highest standard electronic
        if sub_lower.contains("drum n bass") || sub_lower.contains("jungle")
            || sub_lower.contains("halftime") { return 0.55; }
        // Hardcore electronic
        if sub_lower.contains("hardcore") || sub_lower.contains("hardstyle")
            || sub_lower.contains("jumpstyle") || sub_lower.contains("donk")
            || sub_lower.contains("hands up") { return 0.58; }
        if sub_lower.contains("gabber") || sub_lower.contains("makina")
            || sub_lower.contains("hi nrg") { return 0.60; }
        // Extreme electronic
        if sub_lower.contains("breakcore") || sub_lower.contains("speedcore")
            || sub_lower.contains("rhythmic noise") || sub_lower.contains("power electronics")
            || sub_lower.contains("industrial") { return 0.68; }
        if sub_lower.contains("noise") { return 0.75; }
        // Default electronic
        return 0.40;
    }

    // Fallback for unknown genres
    0.35
}

/// Mood tag weights for aggression estimation.
/// Positive = boosts aggression, negative = reduces aggression.
pub fn mood_tag_weight(tag: &str) -> f32 {
    match tag {
        // Boost aggression
        "heavy" | "powerful" | "energetic" => 0.30,
        "dark" | "dramatic" | "epic" | "action" => 0.25,
        "fast" | "sport" => 0.20,
        "deep" | "trailer" | "adventure" => 0.15,
        "party" | "groovy" | "cool" => 0.10,
        // Reduce aggression
        "calm" | "relaxing" | "soft" | "meditative" => -0.30,
        "romantic" | "love" | "sad" | "melancholic" => -0.25,
        "slow" | "dream" | "nature" | "soundscape" | "space" => -0.20,
        "happy" | "hopeful" | "uplifting" | "children" => -0.15,
        "melodic" | "emotional" | "inspiring" | "positive" => -0.10,
        "funny" | "fun" | "christmas" | "holiday" | "summer" => -0.05,
        // Neutral
        _ => 0.0,
    }
}

/// Compute per-track aggression estimate from genre label + mood tags.
/// Returns a value in [0.0, 1.0].
pub fn compute_track_aggression(
    genre: &str,
    mood_themes: Option<&Vec<(String, f32)>>,
) -> f32 {
    let genre_score = genre_aggression_score(genre); // 0.0–0.75

    let mood_score = mood_themes
        .map(|tags| {
            tags.iter()
                .map(|(tag, prob)| prob * mood_tag_weight(tag))
                .sum::<f32>()
        })
        .unwrap_or(0.0); // typically -0.5 to +0.5

    // Combine: 60% genre (reliable coarse signal) + 40% mood (noisy but fine-grained)
    (0.6 * genre_score + 0.4 * (mood_score + 0.5).clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// Compute per-dimension aggression weights from PCA data and aggression estimates.
///
/// Returns a weight vector (one weight per PCA dimension) where each weight is
/// the Pearson correlation between that dimension and the aggression proxy.
/// The weighted sum `sum(pca[i] * weight[i])` projects each track onto the
/// optimal aggression axis in the full PCA space.
///
/// Also returns the combined correlation r of the full weighted projection.
pub fn compute_aggression_weights(
    pca_data: &[(i64, Vec<f32>)],
    aggression_estimates: &HashMap<i64, f32>,
) -> Option<(Vec<f32>, f32)> {
    if pca_data.is_empty() { return None; }
    let pca_dim = pca_data[0].1.len();

    // Build paired data: tracks that have both PCA and aggression estimate
    let pca_map: HashMap<i64, &Vec<f32>> = pca_data.iter().map(|(id, v)| (*id, v)).collect();
    let paired: Vec<i64> = pca_data.iter()
        .filter_map(|(id, _)| aggression_estimates.get(id).map(|_| *id))
        .collect();

    if paired.len() < 10 { return None; }

    let n = paired.len() as f32;
    let aggr_vals: Vec<f32> = paired.iter().map(|id| aggression_estimates[id]).collect();
    let aggr_mean = aggr_vals.iter().sum::<f32>() / n;
    let aggr_var: f32 = aggr_vals.iter().map(|a| (a - aggr_mean).powi(2)).sum();

    if aggr_var < 1e-10 { return None; }

    // Compute correlation weight for each PCA dimension
    let mut weights = vec![0.0f32; pca_dim];
    for dim in 0..pca_dim {
        let vals: Vec<f32> = paired.iter()
            .filter_map(|id| pca_map.get(id).map(|v| v[dim]))
            .collect();
        if vals.len() != paired.len() { continue; }

        let mean = vals.iter().sum::<f32>() / n;
        let var: f32 = vals.iter().map(|v| (v - mean).powi(2)).sum();
        if var < 1e-10 { continue; }

        let cov: f32 = vals.iter().zip(aggr_vals.iter())
            .map(|(v, a)| (v - mean) * (a - aggr_mean))
            .sum();
        weights[dim] = cov / (var * aggr_var).sqrt();
    }

    // Compute combined correlation of the weighted projection
    let proj_scores: Vec<f32> = paired.iter()
        .filter_map(|id| pca_map.get(id).map(|v| {
            v.iter().zip(weights.iter()).map(|(p, w)| p * w).sum::<f32>()
        }))
        .collect();
    let proj_mean = proj_scores.iter().sum::<f32>() / n;
    let proj_var: f32 = proj_scores.iter().map(|v| (v - proj_mean).powi(2)).sum();
    let proj_cov: f32 = proj_scores.iter().zip(aggr_vals.iter())
        .map(|(v, a)| (v - proj_mean) * (a - aggr_mean)).sum();
    let combined_r = if (proj_var * aggr_var).sqrt() > 1e-10 {
        proj_cov / (proj_var * aggr_var).sqrt()
    } else { 0.0 };

    Some((weights, combined_r))
}

/// Compute aggression score for a single track using the weight vector.
/// Returns a raw score (not percentile-ranked — caller should rank across library).
pub fn project_aggression(pca_vec: &[f32], weights: &[f32]) -> f32 {
    pca_vec.iter().zip(weights.iter()).map(|(p, w)| p * w).sum()
}

// ════════════════════════════════════════════════════════════════════════════
// User-feedback calibration: pairwise logistic regression
// ════════════════════════════════════════════════════════════════════════════

/// A community flagged as needing calibration data.
#[derive(Debug, Clone)]
pub struct UncoveredCommunity {
    pub cluster_id: i32,
    pub track_count: usize,
    pub coverage_pct: f32,
    pub representative_genre: String,
    pub track_ids: Vec<i64>,
}

/// Detect communities that lack calibration coverage.
///
/// Returns communities with >15 tracks and <15% of tracks appearing in any
/// calibration pair. Also flags communities whose centroid is far from all
/// calibrated community centroids (cosine distance > 0.7).
pub fn detect_uncovered_communities(
    community_assignments: &HashMap<i64, i32>,
    calibration_pairs: &[(i64, i64, i32)],
    pca_data: &HashMap<i64, Vec<f32>>,
    genre_labels: &HashMap<i64, String>,
) -> Vec<UncoveredCommunity> {
    // Build set of track IDs that appear in any calibration pair
    let mut calibrated_ids: HashSet<i64> = HashSet::new();
    for &(a, b, _) in calibration_pairs {
        calibrated_ids.insert(a);
        calibrated_ids.insert(b);
    }

    // Group tracks by community
    let mut communities: HashMap<i32, Vec<i64>> = HashMap::new();
    for (&track_id, &cluster_id) in community_assignments {
        if cluster_id >= 0 { // skip noise (cluster_id = -1)
            communities.entry(cluster_id).or_default().push(track_id);
        }
    }

    // Compute centroids for calibrated communities (for distance check)
    let mut calibrated_centroids: Vec<Vec<f32>> = Vec::new();
    for (_, track_ids) in &communities {
        let calibrated_count = track_ids.iter().filter(|id| calibrated_ids.contains(id)).count();
        let coverage = calibrated_count as f32 / track_ids.len().max(1) as f32;
        if coverage >= 0.15 {
            if let Some(centroid) = compute_centroid(track_ids, pca_data) {
                calibrated_centroids.push(centroid);
            }
        }
    }

    let mut uncovered = Vec::new();
    for (&cluster_id, track_ids) in &communities {
        if track_ids.len() < 15 { continue; }

        let calibrated_count = track_ids.iter().filter(|id| calibrated_ids.contains(id)).count();
        let coverage_pct = calibrated_count as f32 / track_ids.len() as f32;

        let mut needs_calibration = coverage_pct < 0.15;

        // Also flag if centroid is distant from all calibrated centroids
        if !needs_calibration && !calibrated_centroids.is_empty() {
            if let Some(centroid) = compute_centroid(track_ids, pca_data) {
                let min_sim = calibrated_centroids.iter()
                    .map(|c| cosine_similarity(&centroid, c))
                    .fold(f32::NEG_INFINITY, f32::max);
                if min_sim < 0.3 {
                    needs_calibration = true;
                }
            }
        }

        if needs_calibration {
            // Find most common genre label for this community
            let representative_genre = most_common_genre(track_ids, genre_labels);
            uncovered.push(UncoveredCommunity {
                cluster_id,
                track_count: track_ids.len(),
                coverage_pct,
                representative_genre,
                track_ids: track_ids.clone(),
            });
        }
    }

    // Sort by track count descending (largest uncovered communities first)
    uncovered.sort_by(|a, b| b.track_count.cmp(&a.track_count));
    uncovered
}

/// Plan calibration pairs for uncovered communities.
///
/// Returns (track_a, track_b) pairs organized in three phases:
/// 1. Anchor: uncovered track vs well-covered track (40%)
/// 2. Intra-community: pairs within uncovered communities (40%)
/// 3. Boundary: uncovered vs nearest-neighbor community (20%)
pub fn plan_calibration_pairs(
    uncovered: &[UncoveredCommunity],
    community_assignments: &HashMap<i64, i32>,
    pca_data: &HashMap<i64, Vec<f32>>,
    current_weights: &[f32],
    calibration_pairs: &[(i64, i64, i32)],
) -> (Vec<(i64, i64)>, Vec<(i64, i64)>, Vec<(i64, i64)>) {
    let mut calibrated_ids: HashSet<i64> = HashSet::new();
    for &(a, b, _) in calibration_pairs {
        calibrated_ids.insert(a);
        calibrated_ids.insert(b);
    }

    // Collect well-covered tracks with aggression scores for anchoring
    let mut covered_tracks_scored: Vec<(i64, f32)> = Vec::new();
    for (&track_id, _) in community_assignments {
        if calibrated_ids.contains(&track_id) {
            if let Some(pca) = pca_data.get(&track_id) {
                let score = project_aggression(pca, current_weights);
                covered_tracks_scored.push((track_id, score));
            }
        }
    }
    covered_tracks_scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Pick anchor tracks at 25th, 50th, 75th percentile of aggression.
    // For first-time calibration (no existing pairs), use tracks from the
    // largest non-uncovered communities spread across the PCA projection.
    let anchor_refs: Vec<i64> = if covered_tracks_scored.len() >= 3 {
        let n = covered_tracks_scored.len();
        vec![
            covered_tracks_scored[n / 4].0,
            covered_tracks_scored[n / 2].0,
            covered_tracks_scored[n * 3 / 4].0,
        ]
    } else if !covered_tracks_scored.is_empty() {
        covered_tracks_scored.iter().map(|(id, _)| *id).collect()
    } else {
        // First-time calibration: pick reference tracks from non-uncovered communities
        // sorted by proxy aggression score to span the range
        let uncovered_set: HashSet<i64> = uncovered.iter()
            .flat_map(|c| c.track_ids.iter().copied())
            .collect();
        let mut all_scored: Vec<(i64, f32)> = community_assignments.keys()
            .filter(|id| !uncovered_set.contains(id))
            .filter_map(|&id| pca_data.get(&id).map(|pca| (id, project_aggression(pca, current_weights))))
            .collect();
        all_scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        if all_scored.len() >= 3 {
            let n = all_scored.len();
            vec![all_scored[n / 4].0, all_scored[n / 2].0, all_scored[n * 3 / 4].0]
        } else {
            all_scored.iter().map(|(id, _)| *id).collect()
        }
    };

    let mut anchor_pairs = Vec::new();
    let mut intra_pairs = Vec::new();
    let mut boundary_pairs = Vec::new();

    // Step 1: Find 3 edge tracks per community via farthest point sampling.
    // Edges capture the extremes of variation within each cluster.
    let mut community_edges: Vec<(i32, Vec<i64>)> = Vec::new();

    for community in uncovered {
        let tracks_with_pca: Vec<i64> = community.track_ids.iter()
            .filter(|id| pca_data.contains_key(id))
            .copied()
            .collect();

        if tracks_with_pca.is_empty() { continue; }

        let edges = farthest_point_sample(&tracks_with_pca, pca_data, 3);
        community_edges.push((community.cluster_id, edges));
    }

    // Step 2: Anchor pairs — cross-community edge comparisons.
    // Pair edges from different communities, prioritizing communities that are
    // far apart in PCA space. This creates diverse comparisons like
    // "metalcore edge vs ambient edge" and "hard techno vs soft DnB."
    let mut community_centroids: Vec<(usize, Vec<f32>)> = Vec::new();
    for (idx, (_, edges)) in community_edges.iter().enumerate() {
        if let Some(centroid) = compute_centroid(edges, pca_data) {
            community_centroids.push((idx, centroid));
        }
    }

    // Create cross-community pairs: for each community, pair one edge against
    // an edge from the most distant community and one from the nearest.
    let mut used_cross_pairs: HashSet<(i64, i64)> = HashSet::new();
    for (idx, (_, edges)) in community_edges.iter().enumerate() {
        if edges.is_empty() { continue; }
        let this_centroid = community_centroids.iter()
            .find(|(i, _)| *i == idx).map(|(_, c)| c);
        let this_centroid = match this_centroid {
            Some(c) => c,
            None => continue,
        };

        // Find most distant and nearest other community
        let mut best_far: Option<(usize, f32)> = None;
        let mut best_near: Option<(usize, f32)> = None;
        for &(other_idx, ref other_c) in &community_centroids {
            if other_idx == idx { continue; }
            let sim = cosine_similarity(this_centroid, other_c);
            if best_far.is_none() || sim < best_far.unwrap().1 {
                best_far = Some((other_idx, sim));
            }
            if best_near.is_none() || sim > best_near.unwrap().1 {
                best_near = Some((other_idx, sim));
            }
        }

        // Pair first edge against farthest community's first edge
        if let Some((far_idx, _)) = best_far {
            let far_edges = &community_edges[far_idx].1;
            if !far_edges.is_empty() {
                let pair = (edges[0], far_edges[0]);
                if !used_cross_pairs.contains(&pair) && !used_cross_pairs.contains(&(pair.1, pair.0)) {
                    anchor_pairs.push(pair);
                    used_cross_pairs.insert(pair);
                }
            }
        }
        // Pair second edge against nearest community's edge (boundary calibration)
        if let Some((near_idx, _)) = best_near {
            let near_edges = &community_edges[near_idx].1;
            if edges.len() >= 2 && !near_edges.is_empty() {
                let near_edge = near_edges.last().copied().unwrap_or(near_edges[0]);
                let pair = (edges[1], near_edge);
                if !used_cross_pairs.contains(&pair) && !used_cross_pairs.contains(&(pair.1, pair.0)) {
                    boundary_pairs.push(pair);
                    used_cross_pairs.insert(pair);
                }
            }
        }
    }

    // Also pair edges against anchor reference tracks (if available)
    if !anchor_refs.is_empty() {
        for (_, edges) in &community_edges {
            if let Some(&edge) = edges.last() {
                let ref_id = anchor_refs[anchor_pairs.len() % anchor_refs.len()];
                if edge != ref_id {
                    anchor_pairs.push((edge, ref_id));
                }
            }
        }
    }

    // Step 3: Intra-community pairs from edges.
    // Pair the edges against each other within each community — these are
    // the tracks with maximum internal spread.
    for (_, edges) in &community_edges {
        if edges.len() >= 2 {
            intra_pairs.push((edges[0], edges[1]));
        }
        if edges.len() >= 3 {
            intra_pairs.push((edges[0], edges[2]));
        }
    }

    // Cap total pairs at 60
    let total = anchor_pairs.len() + intra_pairs.len() + boundary_pairs.len();
    if total > 60 {
        let scale = 60.0 / total as f32;
        anchor_pairs.truncate((anchor_pairs.len() as f32 * scale).ceil() as usize);
        intra_pairs.truncate((intra_pairs.len() as f32 * scale).ceil() as usize);
        boundary_pairs.truncate((boundary_pairs.len() as f32 * scale).ceil() as usize);
    }

    (anchor_pairs, intra_pairs, boundary_pairs)
}

/// Select the next most informative pair from remaining candidates using
/// active learning (uncertainty sampling).
pub fn select_next_pair(
    candidates: &[(i64, i64)],
    pca_data: &HashMap<i64, Vec<f32>>,
    weights: &[f32],
) -> Option<(i64, i64)> {
    candidates.iter()
        .filter_map(|&(a, b)| {
            let pa = pca_data.get(&a)?;
            let pb = pca_data.get(&b)?;
            let delta: Vec<f32> = pa.iter().zip(pb.iter()).map(|(x, y)| x - y).collect();
            let score = project_aggression(&delta, weights).abs();
            Some(((a, b), score))
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(pair, _)| pair)
}

/// Learn aggression weights from stored pairwise comparisons via logistic regression.
///
/// `pairs`: (track_a, track_b, choice) where choice 0=A more aggressive, 1=B, 2=equal
/// `pca_data`: track_id → PCA vector
/// `init_weights`: initial weight vector (warm start from proxy or previous calibration)
///
/// Returns (learned_weights, accuracy) where accuracy is leave-one-out on the training pairs.
pub fn learn_weights_from_pairs(
    pairs: &[(i64, i64, i32)],
    pca_data: &HashMap<i64, Vec<f32>>,
    init_weights: &[f32],
) -> Option<(Vec<f32>, f32)> {
    if pairs.is_empty() { return None; }

    let pca_dim = init_weights.len();
    let lr = 0.01f32;
    let lambda = 0.001f32;
    let epochs = 50;

    // Build training data: delta vectors + labels
    let mut deltas: Vec<Vec<f32>> = Vec::new();
    let mut labels: Vec<f32> = Vec::new(); // 1.0 = A more aggressive, 0.0 = B more aggressive
    let mut equal_deltas: Vec<Vec<f32>> = Vec::new();

    for &(a, b, choice) in pairs {
        let pa = pca_data.get(&a);
        let pb = pca_data.get(&b);
        if let (Some(va), Some(vb)) = (pa, pb) {
            if va.len() != pca_dim || vb.len() != pca_dim { continue; }
            let delta: Vec<f32> = va.iter().zip(vb.iter()).map(|(x, y)| x - y).collect();
            match choice {
                0 => { deltas.push(delta); labels.push(1.0); }
                1 => { deltas.push(delta); labels.push(0.0); }
                2 => { equal_deltas.push(delta); }
                _ => {}
            }
        }
    }

    if deltas.is_empty() { return None; }

    let mut weights = init_weights.to_vec();

    // Batch gradient descent with L2 regularization
    for _ in 0..epochs {
        let mut grad = vec![0.0f32; pca_dim];

        // Standard logistic loss gradient
        for (delta, &label) in deltas.iter().zip(labels.iter()) {
            let dot: f32 = delta.iter().zip(weights.iter()).map(|(d, w)| d * w).sum();
            let pred = sigmoid(dot);
            let error = pred - label;
            for (g, d) in grad.iter_mut().zip(delta.iter()) {
                *g += error * d;
            }
        }

        // Equal pairs: penalize |w · delta| (push toward 0)
        for delta in &equal_deltas {
            let dot: f32 = delta.iter().zip(weights.iter()).map(|(d, w)| d * w).sum();
            // Gradient of dot^2 / 2 w.r.t. w is dot * delta
            let equal_weight = 0.5; // softer than comparison pairs
            for (g, d) in grad.iter_mut().zip(delta.iter()) {
                *g += equal_weight * dot * d;
            }
        }

        let n = (deltas.len() + equal_deltas.len()).max(1) as f32;
        for (w, g) in weights.iter_mut().zip(grad.iter()) {
            *w -= lr * (g / n + lambda * *w);
        }
    }

    // Compute leave-one-out accuracy
    let accuracy = if deltas.len() >= 2 {
        let mut correct = 0;
        for i in 0..deltas.len() {
            // Train without pair i
            let mut loo_weights = init_weights.to_vec();
            for _ in 0..epochs {
                let mut grad = vec![0.0f32; pca_dim];
                for (j, (delta, &label)) in deltas.iter().zip(labels.iter()).enumerate() {
                    if j == i { continue; }
                    let dot: f32 = delta.iter().zip(loo_weights.iter()).map(|(d, w)| d * w).sum();
                    let pred = sigmoid(dot);
                    let error = pred - label;
                    for (g, d) in grad.iter_mut().zip(delta.iter()) {
                        *g += error * d;
                    }
                }
                let n = (deltas.len() - 1).max(1) as f32;
                for (w, g) in loo_weights.iter_mut().zip(grad.iter()) {
                    *w -= lr * (g / n + lambda * *w);
                }
            }
            // Predict on held-out pair
            let dot: f32 = deltas[i].iter().zip(loo_weights.iter()).map(|(d, w)| d * w).sum();
            let pred = sigmoid(dot);
            let predicted_label = if pred >= 0.5 { 1.0 } else { 0.0 };
            if (predicted_label - labels[i]).abs() < 0.5 {
                correct += 1;
            }
        }
        correct as f32 / deltas.len() as f32
    } else {
        0.0
    };

    Some((weights, accuracy))
}

/// Perform a single online SGD step after one user comparison.
/// Mutates `weights` in place. Returns the prediction before update.
pub fn sgd_step(
    weights: &mut [f32],
    pca_a: &[f32],
    pca_b: &[f32],
    choice: i32, // 0=A, 1=B, 2=equal
) -> f32 {
    let lr = 0.01f32;
    let lambda = 0.001f32;

    let delta: Vec<f32> = pca_a.iter().zip(pca_b.iter()).map(|(a, b)| a - b).collect();
    let dot: f32 = delta.iter().zip(weights.iter()).map(|(d, w)| d * w).sum();
    let pred = sigmoid(dot);

    match choice {
        0 | 1 => {
            let label = if choice == 0 { 1.0f32 } else { 0.0 };
            let error = pred - label;
            for (w, d) in weights.iter_mut().zip(delta.iter()) {
                *w -= lr * (error * d + lambda * *w);
            }
        }
        2 => {
            // Equal: push projection difference toward 0
            for (w, d) in weights.iter_mut().zip(delta.iter()) {
                *w -= lr * (0.5 * dot * d + lambda * *w);
            }
        }
        _ => {}
    }

    pred
}

/// Estimate the drop sample position by finding the steepest sustained RMS transition.
///
/// Scans the audio for the point where RMS rises from below-median to consistently
/// above-median, selecting the crossing with the steepest 4-window slope.
/// Falls back to 33% of total length if no clear transition found.
pub fn estimate_drop_sample(pcm: &[f32], sample_rate: u32, channels: u32) -> u64 {
    let window_samples = (sample_rate as f32 * 0.5) as usize * channels as usize;
    if pcm.len() < window_samples * 8 {
        return (pcm.len() as f64 * 0.33 / channels as f64) as u64;
    }

    // Compute RMS per window
    let n_windows = pcm.len() / window_samples;
    let mut rms: Vec<f32> = Vec::with_capacity(n_windows);
    for i in 0..n_windows {
        let start = i * window_samples;
        let end = (start + window_samples).min(pcm.len());
        let sum_sq: f32 = pcm[start..end].iter().map(|s| s * s).sum();
        rms.push((sum_sq / (end - start) as f32).sqrt());
    }

    // Compute median RMS
    let mut sorted_rms = rms.clone();
    sorted_rms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted_rms[sorted_rms.len() / 2];

    if median < 1e-8 {
        return (pcm.len() as f64 * 0.33 / channels as f64) as u64;
    }

    // Find steepest sustained transition: below-median → 4 consecutive above-median windows
    let sustain_count = 4usize;
    let mut best_idx: Option<usize> = None;
    let mut best_slope = 0.0f32;

    for i in 1..n_windows.saturating_sub(sustain_count) {
        // Check: window i-1 is below median, windows i..i+sustain are all above
        if rms[i - 1] >= median { continue; }
        let all_above = (0..sustain_count).all(|j| rms[i + j] > median);
        if !all_above { continue; }

        // Slope: average RMS of sustain windows minus the pre-transition window
        let sustain_avg: f32 = (0..sustain_count).map(|j| rms[i + j]).sum::<f32>() / sustain_count as f32;
        let slope = sustain_avg - rms[i - 1];
        if slope > best_slope {
            best_slope = slope;
            best_idx = Some(i);
        }
    }

    match best_idx {
        Some(idx) => {
            // Convert window index to sample position (mono samples)
            let samples_per_window = window_samples / channels as usize;
            (idx * samples_per_window) as u64
        }
        None => (pcm.len() as f64 * 0.33 / channels as f64) as u64,
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-10 || norm_b < 1e-10 { return 0.0; }
    dot / (norm_a * norm_b)
}

fn compute_centroid(track_ids: &[i64], pca_data: &HashMap<i64, Vec<f32>>) -> Option<Vec<f32>> {
    let vecs: Vec<&Vec<f32>> = track_ids.iter()
        .filter_map(|id| pca_data.get(id))
        .collect();
    if vecs.is_empty() { return None; }
    let dim = vecs[0].len();
    let n = vecs.len() as f32;
    let mut centroid = vec![0.0f32; dim];
    for v in &vecs {
        for (c, val) in centroid.iter_mut().zip(v.iter()) {
            *c += val;
        }
    }
    for c in &mut centroid {
        *c /= n;
    }
    Some(centroid)
}

/// Farthest point sampling: select k tracks that are maximally spread in PCA space.
///
/// 1. First point: farthest from centroid
/// 2. Second point: farthest from first
/// 3. Third+ point: farthest from the nearest already-selected point (greedy k-center)
fn farthest_point_sample(
    track_ids: &[i64],
    pca_data: &HashMap<i64, Vec<f32>>,
    k: usize,
) -> Vec<i64> {
    if track_ids.len() <= k {
        return track_ids.to_vec();
    }

    let vecs: Vec<(i64, &Vec<f32>)> = track_ids.iter()
        .filter_map(|&id| pca_data.get(&id).map(|v| (id, v)))
        .collect();
    if vecs.is_empty() { return Vec::new(); }

    // First point: farthest from centroid
    let centroid = compute_centroid(track_ids, pca_data).unwrap_or_default();
    let mut selected: Vec<usize> = Vec::with_capacity(k);
    let first = vecs.iter().enumerate()
        .max_by(|(_, (_, a)), (_, (_, b))| {
            euclidean_dist_sq(a, &centroid)
                .partial_cmp(&euclidean_dist_sq(b, &centroid))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);
    selected.push(first);

    // Subsequent points: maximize min distance to any selected point
    let mut min_dists: Vec<f32> = vecs.iter()
        .map(|(_, v)| euclidean_dist_sq(v, vecs[first].1))
        .collect();

    for _ in 1..k {
        // Pick the point with largest min-distance to selected set
        let next = min_dists.iter().enumerate()
            .filter(|(i, _)| !selected.contains(i))
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);
        selected.push(next);

        // Update min distances
        for (i, (_, v)) in vecs.iter().enumerate() {
            let d = euclidean_dist_sq(v, vecs[next].1);
            if d < min_dists[i] {
                min_dists[i] = d;
            }
        }
    }

    selected.iter().map(|&i| vecs[i].0).collect()
}

fn euclidean_dist_sq(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum()
}

fn most_common_genre(track_ids: &[i64], genre_labels: &HashMap<i64, String>) -> String {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for id in track_ids {
        if let Some(genre) = genre_labels.get(id) {
            // Use super-genre (before "---") for readability
            let super_genre = genre.split("---").next().unwrap_or(genre);
            *counts.entry(super_genre).or_default() += 1;
        }
    }
    counts.into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(genre, _)| genre.to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}
