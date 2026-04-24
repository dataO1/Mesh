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
    log::info!(
        "[COVERAGE] Input: {} pairs, {} unique calibrated track IDs, {} community assignments",
        calibration_pairs.len(),
        calibrated_ids.len(),
        community_assignments.len(),
    );

    // Group tracks by community
    let mut communities: HashMap<i32, Vec<i64>> = HashMap::new();
    for (&track_id, &cluster_id) in community_assignments {
        if cluster_id >= 0 { // skip noise (cluster_id = -1)
            communities.entry(cluster_id).or_default().push(track_id);
        }
    }
    log::info!("[COVERAGE] {} non-noise communities found", communities.len());

    // Coverage thresholds:
    // - SMALL communities (<30 tracks): need ≥3 calibrated tracks (absolute count)
    // - LARGE communities (≥30 tracks): need ≥10% coverage OR ≥5 calibrated tracks
    // Old 15% threshold was too strict — with 5 reps per community, communities
    // with >33 tracks could never be considered "covered".
    let is_covered = |calibrated_count: usize, total: usize| -> bool {
        if total < 30 {
            calibrated_count >= 3
        } else {
            (calibrated_count as f32 / total as f32) >= 0.10 || calibrated_count >= 5
        }
    };

    // Compute centroids for calibrated communities (for distance check)
    let mut calibrated_centroids: Vec<(i32, Vec<f32>)> = Vec::new();
    for (cluster_id, track_ids) in &communities {
        let calibrated_count = track_ids.iter().filter(|id| calibrated_ids.contains(id)).count();
        if is_covered(calibrated_count, track_ids.len()) {
            if let Some(centroid) = compute_centroid(track_ids, pca_data) {
                calibrated_centroids.push((*cluster_id, centroid));
            }
        }
    }
    log::info!(
        "[COVERAGE] {} communities pass coverage threshold (used as anchor centroids)",
        calibrated_centroids.len(),
    );

    let mut uncovered = Vec::new();
    let mut skipped_too_small = 0;
    for (&cluster_id, track_ids) in &communities {
        if track_ids.len() < 15 {
            skipped_too_small += 1;
            continue;
        }

        let calibrated_count = track_ids.iter().filter(|id| calibrated_ids.contains(id)).count();
        let coverage_pct = calibrated_count as f32 / track_ids.len() as f32;
        let coverage_ok = is_covered(calibrated_count, track_ids.len());

        let mut needs_calibration = !coverage_ok;
        let mut reason = if needs_calibration {
            format!("low coverage: {}/{} = {:.1}%", calibrated_count, track_ids.len(), coverage_pct * 100.0)
        } else {
            String::new()
        };

        // Also flag if centroid is distant from all calibrated centroids
        if !needs_calibration && !calibrated_centroids.is_empty() {
            if let Some(centroid) = compute_centroid(track_ids, pca_data) {
                let max_sim = calibrated_centroids.iter()
                    .map(|(_, c)| cosine_similarity(&centroid, c))
                    .fold(f32::NEG_INFINITY, f32::max);
                if max_sim < 0.3 {
                    needs_calibration = true;
                    reason = format!("centroid too far from any calibrated community (max_sim={:.3} < 0.3)", max_sim);
                }
            }
        }

        log::debug!(
            "[COVERAGE] community {}: {} tracks, {} calibrated ({:.1}%), needs_cal={} ({})",
            cluster_id,
            track_ids.len(),
            calibrated_count,
            coverage_pct * 100.0,
            needs_calibration,
            if reason.is_empty() { "covered".to_string() } else { reason.clone() },
        );

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

    log::info!(
        "[COVERAGE] Result: {} uncovered communities (skipped {} too-small <15 tracks)",
        uncovered.len(),
        skipped_too_small,
    );

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
    let mut already_asked: HashSet<(i64, i64)> = HashSet::new();
    for &(a, b, _) in calibration_pairs {
        calibrated_ids.insert(a);
        calibrated_ids.insert(b);
        // Track in both orders so we can skip duplicates regardless of direction
        already_asked.insert((a, b));
        already_asked.insert((b, a));
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

    // Step 1: Find edge tracks per community via farthest point sampling.
    // Sample more edges (6) than we need for pairs (3) so refinement rounds
    // can pick fresh edges instead of repeating the same pairs.
    let mut community_edges: Vec<(i32, Vec<i64>)> = Vec::new();

    for community in uncovered {
        let tracks_with_pca: Vec<i64> = community.track_ids.iter()
            .filter(|id| pca_data.contains_key(id))
            .copied()
            .collect();

        if tracks_with_pca.is_empty() { continue; }

        let edges = farthest_point_sample(&tracks_with_pca, pca_data, 6);
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
    // Skip pairs already in the user's history so refinement rounds get
    // fresh comparisons instead of repeating the same pairs.
    let mut used_cross_pairs: HashSet<(i64, i64)> = HashSet::new();
    let try_pair = |pair: (i64, i64), used: &HashSet<(i64, i64)>| -> bool {
        !already_asked.contains(&pair)
            && !already_asked.contains(&(pair.1, pair.0))
            && !used.contains(&pair)
            && !used.contains(&(pair.1, pair.0))
    };

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

        // Anchor (cross-community far): try edges in order until we find an unused pair
        if let Some((far_idx, _)) = best_far {
            let far_edges = &community_edges[far_idx].1;
            'outer_far: for &my in edges {
                for &theirs in far_edges {
                    let pair = (my, theirs);
                    if try_pair(pair, &used_cross_pairs) {
                        anchor_pairs.push(pair);
                        used_cross_pairs.insert(pair);
                        break 'outer_far;
                    }
                }
            }
        }
        // Boundary (cross-community near): same logic
        if let Some((near_idx, _)) = best_near {
            let near_edges = &community_edges[near_idx].1;
            'outer_near: for &my in edges {
                for &theirs in near_edges.iter().rev() {
                    let pair = (my, theirs);
                    if try_pair(pair, &used_cross_pairs) {
                        boundary_pairs.push(pair);
                        used_cross_pairs.insert(pair);
                        break 'outer_near;
                    }
                }
            }
        }
    }

    // Also pair edges against anchor reference tracks (if available)
    if !anchor_refs.is_empty() {
        for (_, edges) in &community_edges {
            for &edge in edges.iter().rev() {
                let ref_id = anchor_refs[anchor_pairs.len() % anchor_refs.len()];
                let pair = (edge, ref_id);
                if edge != ref_id && try_pair(pair, &used_cross_pairs) {
                    anchor_pairs.push(pair);
                    used_cross_pairs.insert(pair);
                    break;
                }
            }
        }
    }

    // Step 3: Intra-community pairs from edges. Try multiple edge combinations
    // and skip ones the user has already been asked.
    for (_, edges) in &community_edges {
        let mut added = 0;
        'intra: for i in 0..edges.len() {
            for j in (i + 1)..edges.len() {
                let pair = (edges[i], edges[j]);
                if try_pair(pair, &used_cross_pairs) {
                    intra_pairs.push(pair);
                    used_cross_pairs.insert(pair);
                    added += 1;
                    if added >= 2 { break 'intra; }
                }
            }
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

/// Two-phase calibration plan returned by `build_calibration_plan`.
///
/// **Representatives per community (K=5 skip-chain):**
/// - 4 FPS edge tracks (captures spread across the community)
/// - 1 centroid track (real track closest to PCA centroid, not already an edge)
///
/// **Phase 1** (fixed, deterministic, asked first FIFO): skip-chain bootstrap.
/// For 5 representatives arranged as `[e0, e1, e2, e3, centroid]` we ask:
///   `e0 vs e2`, `e2 vs centroid`, `centroid vs e1`, `e1 vs e3`
/// This creates a connected chain `e0—e2—centroid—e1—e3` across all 5 reps
/// with 4 questions. Transitive closure over consistent answers implies all
/// C(5,2)=10 pair orderings. Skip-chain (non-adjacent jumps) gives stronger
/// per-answer signal than comparing consecutive FPS extremes. Plus a few
/// global anchor pairs to seed the cross-community axis.
///
/// **Phase 2** (dynamic, active learning): cross-community + remaining intra.
/// `next_calibration_pair_v2` picks adaptively using uncertainty + transitive
/// closure + diversity rotation.
pub struct CalibrationPlan {
    /// Bootstrap pairs to ask FIFO before active learning kicks in.
    pub phase_1: Vec<(i64, i64)>,
    /// Pool for active learning (everything not in phase 1).
    pub phase_2: Vec<(i64, i64)>,
    /// Map: track_id → community_id. Used by the diversity heuristic to avoid
    /// asking about the same community in consecutive rounds.
    pub track_community: HashMap<i64, i32>,
}

/// Pick the real track in `track_ids` whose PCA vector is closest to the
/// centroid of the set. Returns the typical/most-representative member.
fn centroid_track(track_ids: &[i64], pca_data: &HashMap<i64, Vec<f32>>) -> Option<i64> {
    let centroid = compute_centroid(track_ids, pca_data)?;
    track_ids.iter()
        .filter_map(|&id| pca_data.get(&id).map(|v| (id, v)))
        .min_by(|a, b| {
            let da = euclidean_dist_sq(a.1, &centroid);
            let db = euclidean_dist_sq(b.1, &centroid);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(id, _)| id)
}

/// Build a hierarchical calibration plan — four-tier Phase 1 that prioritises
/// information gain per answer, Phase 2 remains the active-learning pool.
///
/// Per community, we still compute 5 representatives: 4 FPS edge tracks + 1
/// centroid. But Phase 1 no longer asks the 4-pair skip-chain for every
/// community. Instead it builds a layered question set:
///
///   **Tier 1 — Cross-macro extremes (up to 5 pairs).**
///   Pairs spanning the largest genre-prior aggression gaps across macro
///   genres present in the library (metal × ambient, hardcore × jazz, …).
///   These orient the main aggression axis in the first few clicks.
///
///   **Tier 2 — Per-macro anchor pairs.**
///   For each macro with ≥ 50 tracks, one pair: macro's centroid × the
///   anchor-ref with prior aggression closest to the macro's expected prior.
///   Establishes each macro's position on the axis.
///
///   **Tier 3 — Dominant-macro sub-community skip-chain (2 per sub-community).**
///   Only for the single macro with the most uncovered tracks, AND only if
///   it exceeds `DOMINANT_MACRO_MIN_SIZE`. Asks 2 skip-chain questions per
///   sub-community (`e0 vs e2`, `centroid vs e1`) — transitive closure on a
///   3-track chain implies 3 orderings. Halves the old 4-per-community
///   without losing key structural signal.
///
///   **Tier 4 — Small-macro centroid pairs.**
///   For each macro with 15–49 tracks, one pair: centroid × nearest anchor.
///
/// Phase 2 (active learning pool): all remaining cross-community, cross-rep,
/// and rep×anchor combinations, scored at query time by
/// `next_calibration_pair_v2` using entropy × ||Δ||² × genre-prior gap.
///
/// `_edges_per_community` kept for API compat — unused.
pub fn build_calibration_plan(
    uncovered: &[UncoveredCommunity],
    pca_data: &HashMap<i64, Vec<f32>>,
    genre_labels: &HashMap<i64, String>,
    anchor_refs: &[i64],
    _edges_per_community: usize,
) -> CalibrationPlan {
    /// Only recurse into a macro's sub-communities for Tier 3 if the macro
    /// holds at least this many uncovered tracks. Matches the L2 graph
    /// clustering threshold.
    const DOMINANT_MACRO_MIN_SIZE: usize = 150;

    // Build 5 representatives per uncovered community (same as before).
    let mut community_reps: Vec<(i32, [i64; 5], usize)> = Vec::new(); // (cluster_id, reps, track_count)
    let mut track_community: HashMap<i64, i32> = HashMap::new();

    for community in uncovered {
        let tracks_with_pca: Vec<i64> = community.track_ids.iter()
            .filter(|id| pca_data.contains_key(id))
            .copied()
            .collect();
        if tracks_with_pca.len() < 5 { continue; }

        let edges = farthest_point_sample(&tracks_with_pca, pca_data, 4);
        if edges.len() < 4 { continue; }

        let non_edge: Vec<i64> = tracks_with_pca.iter()
            .filter(|id| !edges.contains(id))
            .copied()
            .collect();
        let cent = match centroid_track(&non_edge, pca_data) {
            Some(c) => c,
            None => continue,
        };

        let reps = [edges[0], edges[1], edges[2], edges[3], cent];
        for &t in &reps {
            track_community.insert(t, community.cluster_id);
        }
        community_reps.push((community.cluster_id, reps, community.track_count));
    }

    let canon = |a: i64, b: i64| if a < b { (a, b) } else { (b, a) };

    // Resolve each community to its macro-genre by voting over its reps'
    // Discogs labels (most-common macro wins). Communities are generally
    // genre-homogeneous after our genre-L1 clustering, but we tolerate the
    // occasional mixed-label edge case by majority-voting.
    let macro_of_community = |reps: &[i64; 5]| -> &'static str {
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for &t in reps {
            if let Some(label) = genre_labels.get(&t) {
                let m = crate::genre_map::macro_genre_for(label);
                *counts.entry(m).or_default() += 1;
            }
        }
        counts.into_iter()
            .max_by_key(|&(_, c)| c)
            .map(|(m, _)| m)
            .unwrap_or("Other")
    };

    // Group community indices by their resolved macro-genre.
    let mut by_macro: HashMap<&'static str, Vec<usize>> = HashMap::new();
    let mut community_macro: Vec<&'static str> = Vec::with_capacity(community_reps.len());
    for (i, (_, reps, _)) in community_reps.iter().enumerate() {
        let m = macro_of_community(reps);
        community_macro.push(m);
        by_macro.entry(m).or_default().push(i);
    }

    // Aggregate uncovered track-count per macro (sum over its communities).
    let macro_sizes: HashMap<&'static str, usize> = by_macro.iter()
        .map(|(&m, idxs)| {
            let total: usize = idxs.iter().map(|&i| community_reps[i].2).sum();
            (m, total)
        })
        .collect();

    // Helper: pick a "representative" track per macro (the rep of its largest
    // community that's closest to that community's centroid — already the
    // 5th rep by construction).
    let macro_reps: HashMap<&'static str, i64> = by_macro.iter()
        .filter_map(|(&m, idxs)| {
            let largest_idx = idxs.iter()
                .max_by_key(|&&i| community_reps[i].2)?;
            Some((m, community_reps[*largest_idx].1[4]))
        })
        .collect();

    // Helper: expected aggression prior for a macro (from its rep's label).
    let macro_prior = |m: &str| -> f32 {
        let rep = match macro_reps.get(m) { Some(r) => *r, None => return 0.35 };
        let label = match genre_labels.get(&rep) { Some(l) => l.as_str(), None => return 0.35 };
        genre_aggression_score(label)
    };

    // Phase 1 construction — order matters for user-visible FIFO flow.
    let mut phase_1: Vec<(i64, i64)> = Vec::new();
    let mut phase_1_set: HashSet<(i64, i64)> = HashSet::new();

    let mut add_if_new = |a: i64, b: i64, phase_1: &mut Vec<(i64, i64)>, set: &mut HashSet<(i64, i64)>| {
        if a == b { return false; }
        let p = canon(a, b);
        if set.insert(p) { phase_1.push(p); true } else { false }
    };

    // ─── Tier 1 ── Cross-macro extremes ────────────────────────────────────
    // Enumerate all macro pairs, score by |prior_gap|, pick top 5. Uses each
    // macro's rep track as the concrete pair member.
    {
        let macros: Vec<&'static str> = macro_reps.keys().copied().collect();
        let mut cross_macro_pairs: Vec<((i64, i64), f32)> = Vec::new();
        for i in 0..macros.len() {
            for j in (i + 1)..macros.len() {
                let rep_a = macro_reps[macros[i]];
                let rep_b = macro_reps[macros[j]];
                let gap = (macro_prior(macros[i]) - macro_prior(macros[j])).abs();
                cross_macro_pairs.push(((rep_a, rep_b), gap));
            }
        }
        cross_macro_pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for ((a, b), _gap) in cross_macro_pairs.into_iter().take(5) {
            add_if_new(a, b, &mut phase_1, &mut phase_1_set);
        }
    }

    // ─── Tier 2 ── Per-macro anchor pairs ──────────────────────────────────
    // For each macro with ≥50 uncovered tracks, pair its rep with the anchor-
    // ref whose prior aggression is farthest from the macro's (so the pair
    // has a clear expected answer).
    if !anchor_refs.is_empty() {
        for (&m, &size) in &macro_sizes {
            if size < 50 { continue; }
            let rep = macro_reps[m];
            let m_prior = macro_prior(m);
            let best_anchor = anchor_refs.iter()
                .copied()
                .filter(|&a| a != rep)
                .max_by(|&a, &b| {
                    let pa = genre_labels.get(&a).map(|l| genre_aggression_score(l)).unwrap_or(0.35);
                    let pb = genre_labels.get(&b).map(|l| genre_aggression_score(l)).unwrap_or(0.35);
                    let da = (pa - m_prior).abs();
                    let db = (pb - m_prior).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                });
            if let Some(anchor) = best_anchor {
                add_if_new(rep, anchor, &mut phase_1, &mut phase_1_set);
            }
        }
    }

    // ─── Tier 3 ── Dominant-macro sub-community skip-chain ─────────────────
    // Identify the single dominant macro (largest total uncovered tracks)
    // if it meets DOMINANT_MACRO_MIN_SIZE. Within it, every sub-community
    // gets a 2-pair skip-chain: e0 vs e2, centroid vs e1. Transitive closure
    // on that 3-track chain implies 3 orderings per sub-community.
    if let Some((&dominant_macro, &dominant_size)) = macro_sizes.iter()
        .max_by_key(|&(_, &s)| s)
    {
        if dominant_size >= DOMINANT_MACRO_MIN_SIZE {
            if let Some(subs) = by_macro.get(dominant_macro) {
                for &sub_idx in subs {
                    let reps = community_reps[sub_idx].1;
                    // 2-pair mini skip-chain across the 5 reps:
                    //   e0 vs e2, centroid vs e1  →  chain e0—e2 and cent—e1
                    //   (each answer implies 1 ordering pair, total 2 pairs)
                    // We also add one "bridge" pair e2 vs centroid so the
                    // chain becomes e0—e2—centroid—e1, closing transitively
                    // to 3 orderings (~half of the old 4-pair chain's 6
                    // closures, at half the user cost).
                    let chain = [
                        (reps[0], reps[2]),
                        (reps[2], reps[4]),
                        (reps[4], reps[1]),
                    ];
                    for (a, b) in chain {
                        add_if_new(a, b, &mut phase_1, &mut phase_1_set);
                    }
                }
            }
        }
    }

    // ─── Tier 4 ── Small-macro centroid pairs ──────────────────────────────
    // For macros that don't qualify for tier 2 or tier 3 but still exist
    // (15–49 tracks), add one rep × closest anchor pair. Skips tiny macros
    // that are already covered by tier 1 cross-macro extremes.
    if !anchor_refs.is_empty() {
        for (&m, &size) in &macro_sizes {
            if size >= 50 { continue; } // covered by tier 2
            if size < 15 { continue; } // too small to matter
            let rep = macro_reps[m];
            let m_prior = macro_prior(m);
            let best_anchor = anchor_refs.iter()
                .copied()
                .filter(|&a| a != rep)
                .max_by(|&a, &b| {
                    let pa = genre_labels.get(&a).map(|l| genre_aggression_score(l)).unwrap_or(0.35);
                    let pb = genre_labels.get(&b).map(|l| genre_aggression_score(l)).unwrap_or(0.35);
                    let da = (pa - m_prior).abs();
                    let db = (pb - m_prior).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                });
            if let Some(anchor) = best_anchor {
                add_if_new(rep, anchor, &mut phase_1, &mut phase_1_set);
            }
        }
    }

    // ─── Phase 2 ── Active-learning pool ────────────────────────────────────
    // Everything remaining: all cross-community rep×rep combinations, all
    // intra-community rep×rep combinations not in phase 1, and all rep×anchor
    // combinations. next_calibration_pair_v2 scores them at query time by
    // entropy × ||Δ||² × genre_prior_gap × diversity.
    let mut phase_2: HashSet<(i64, i64)> = HashSet::new();

    // Cross-community
    for i in 0..community_reps.len() {
        for j in (i + 1)..community_reps.len() {
            for &a in &community_reps[i].1 {
                for &b in &community_reps[j].1 {
                    if a != b {
                        let p = canon(a, b);
                        if !phase_1_set.contains(&p) { phase_2.insert(p); }
                    }
                }
            }
        }
    }
    // Intra-community
    for (_, reps, _) in &community_reps {
        for i in 0..reps.len() {
            for j in (i + 1)..reps.len() {
                let p = canon(reps[i], reps[j]);
                if !phase_1_set.contains(&p) { phase_2.insert(p); }
            }
        }
    }
    // Rep × anchor
    for (_, reps, _) in &community_reps {
        for &t in reps {
            for &anchor in anchor_refs {
                if t != anchor {
                    let p = canon(t, anchor);
                    if !phase_1_set.contains(&p) { phase_2.insert(p); }
                }
            }
        }
    }

    log::info!(
        "[CALIBRATION/PLAN] Phase 1 tiers built: {} pairs across {} macros ({} dominant), phase 2 pool: {} pairs",
        phase_1.len(),
        macro_sizes.len(),
        macro_sizes.values().filter(|&&s| s >= DOMINANT_MACRO_MIN_SIZE).count(),
        phase_2.len(),
    );

    CalibrationPlan {
        phase_1,
        phase_2: phase_2.into_iter().collect(),
        track_community,
    }
}

/// Backwards-compat wrapper: returns just the unioned pool (no phase split).
/// New code should use `build_calibration_plan` instead. Callers without
/// genre labels pass an empty HashMap; the plan degrades gracefully
/// (tier 1 collapses to zero-gap pairs, tier 2/4 still work by anchor-only
/// selection, tier 3 still runs but without the prior-based anchor pick).
pub fn build_candidate_pool(
    uncovered: &[UncoveredCommunity],
    pca_data: &HashMap<i64, Vec<f32>>,
    anchor_refs: &[i64],
    edges_per_community: usize,
) -> Vec<(i64, i64)> {
    let empty_labels: HashMap<i64, String> = HashMap::new();
    let plan = build_calibration_plan(
        uncovered,
        pca_data,
        &empty_labels,
        anchor_refs,
        edges_per_community,
    );
    let mut all = plan.phase_1;
    all.extend(plan.phase_2);
    all
}

/// Select the next most informative pair from a candidate pool.
///
/// Filters:
/// - Skips pairs already asked
/// - Skips pairs whose ordering is transitively implied by prior answers
///   (e.g., if A > B and B > C, skip A vs C — we already know the answer)
///
/// Scoring depends on model state:
/// - **Cold-start** (weight magnitude near zero): rank by PCA delta magnitude
///   only. With zero weights, sigmoid is always 0.5 so uncertainty carries no
///   signal. Largest-delta pairs are the most discriminative.
/// - **Warm**: rank by uncertainty (closeness to sigmoid=0.5), tie-break by
///   delta magnitude.
///
/// Diversity: when `track_community` and `recent_communities` are provided, a
/// small penalty discourages picking pairs that touch communities asked about
/// in the last few rounds — prevents dwelling on one region of the graph.
pub fn next_calibration_pair_v2(
    pool: &[(i64, i64)],
    asked_pairs: &[(i64, i64, i32)],
    pca_data: &HashMap<i64, Vec<f32>>,
    weights: &[f32],
    track_community: &HashMap<i64, i32>,
    recent_communities: &[i32],
    genre_labels: &HashMap<i64, String>,
) -> Option<(i64, i64)> {
    let weights_norm: f32 = weights.iter().map(|w| w * w).sum::<f32>().sqrt();
    let cold_start = weights_norm < 0.01;

    // Build directed graph for transitive closure
    let mut graph: HashMap<i64, HashSet<i64>> = HashMap::new();
    let mut asked_set: HashSet<(i64, i64)> = HashSet::new();
    for &(a, b, choice) in asked_pairs {
        asked_set.insert((a, b));
        asked_set.insert((b, a));
        match choice {
            0 => { graph.entry(a).or_default().insert(b); }
            1 => { graph.entry(b).or_default().insert(a); }
            2 => {
                graph.entry(a).or_default().insert(b);
                graph.entry(b).or_default().insert(a);
            }
            _ => {}
        }
    }

    let reachable = |start: i64, target: i64, graph: &HashMap<i64, HashSet<i64>>| -> bool {
        if start == target { return true; }
        let mut visited: HashSet<i64> = HashSet::new();
        let mut stack: Vec<i64> = vec![start];
        while let Some(node) = stack.pop() {
            if !visited.insert(node) { continue; }
            if let Some(neighbors) = graph.get(&node) {
                for &n in neighbors {
                    if n == target { return true; }
                    stack.push(n);
                }
            }
        }
        false
    };

    let recent_set: HashSet<i32> = recent_communities.iter().copied().collect();

    // Genre-prior gap lookup. Returns 0.0 when labels are missing — the
    // prior_factor becomes 1.0 (neutral), so the scoring gracefully
    // degrades to uncertainty/delta-only ordering.
    let prior_gap = |a: i64, b: i64| -> f32 {
        let la = match genre_labels.get(&a) { Some(s) => s.as_str(), None => return 0.0 };
        let lb = match genre_labels.get(&b) { Some(s) => s.as_str(), None => return 0.0 };
        (genre_aggression_score(la) - genre_aggression_score(lb)).abs()
    };

    let mut best: Option<((i64, i64), f32)> = None;
    for &(a, b) in pool {
        if asked_set.contains(&(a, b)) { continue; }
        if reachable(a, b, &graph) || reachable(b, a, &graph) { continue; }

        let pca_a = match pca_data.get(&a) { Some(v) => v, None => continue };
        let pca_b = match pca_data.get(&b) { Some(v) => v, None => continue };
        if pca_a.len() != pca_b.len() { continue; }

        let delta_sq: f32 = pca_a.iter().zip(pca_b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>();
        let delta_mag = delta_sq.sqrt();

        // Genre-prior gap factor (Change A). In [0.0, 0.75] natively; amplify
        // slightly so a full-spread pair (ambient vs metal) doubles the score,
        // while same-genre pairs (~0) stay at 1.0×.
        let pg = prior_gap(a, b);
        let prior_factor = 1.0 + 1.5 * pg;

        let base_score = if cold_start || weights.len() != pca_a.len() {
            // Cold-start: no learned axis yet. Rank by prior-weighted PCA
            // discriminability — picks "we EXPECT a strong answer here AND
            // it'll move the model" pairs first. First few questions span
            // ambient-vs-metal type extremes, orienting the axis fast.
            prior_factor * delta_mag
        } else {
            // Warm: BALD-style info gain proxy (Change B).
            //   I(pair) ∝ H(σ(w·Δ)) × ||Δ||²
            // Multiplicative form correctly penalises small-Δ pairs — a
            // tiny PCA difference near sigmoid=0.5 gives near-zero info,
            // which the old additive form over-valued.
            let logit: f32 = weights.iter().zip(pca_a.iter()).zip(pca_b.iter())
                .map(|((w, x), y)| w * (x - y))
                .sum();
            let prob = sigmoid(logit).clamp(1e-6, 1.0 - 1e-6);
            let entropy = -(prob * prob.ln() + (1.0 - prob) * (1.0 - prob).ln());
            let delta_sq_capped = delta_sq.min(4.0);
            prior_factor * entropy * delta_sq_capped
        };

        // Diversity penalty: down-weight pairs touching recently-asked
        // communities so the user gets variety instead of dwelling on one
        // region. Each touched-recent community costs 15% of the score.
        let comm_a = track_community.get(&a);
        let comm_b = track_community.get(&b);
        let mut diversity_factor: f32 = 1.0;
        if let Some(&c) = comm_a {
            if recent_set.contains(&c) { diversity_factor *= 0.85; }
        }
        if let Some(&c) = comm_b {
            if recent_set.contains(&c) { diversity_factor *= 0.85; }
        }
        let score = base_score * diversity_factor;

        if best.is_none() || score > best.unwrap().1 {
            best = Some(((a, b), score));
        }
    }
    best.map(|(p, _)| p)
}

/// Backwards-compat wrapper without diversity/cold-start logic.
/// Prefer `next_calibration_pair_v2`.
pub fn next_calibration_pair(
    pool: &[(i64, i64)],
    asked_pairs: &[(i64, i64, i32)],
    pca_data: &HashMap<i64, Vec<f32>>,
    weights: &[f32],
) -> Option<(i64, i64)> {
    // Build directed graph for transitive closure of "more aggressive than"
    // choice 0 = a > b, 1 = b > a, 2 = equal (treated as bidirectional)
    let mut graph: HashMap<i64, HashSet<i64>> = HashMap::new();
    let mut asked_set: HashSet<(i64, i64)> = HashSet::new();
    for &(a, b, choice) in asked_pairs {
        asked_set.insert((a, b));
        asked_set.insert((b, a));
        match choice {
            0 => { graph.entry(a).or_default().insert(b); }
            1 => { graph.entry(b).or_default().insert(a); }
            2 => {
                graph.entry(a).or_default().insert(b);
                graph.entry(b).or_default().insert(a);
            }
            _ => {}
        }
    }

    let reachable = |start: i64, target: i64, graph: &HashMap<i64, HashSet<i64>>| -> bool {
        if start == target { return true; }
        let mut visited: HashSet<i64> = HashSet::new();
        let mut stack: Vec<i64> = vec![start];
        while let Some(node) = stack.pop() {
            if !visited.insert(node) { continue; }
            if let Some(neighbors) = graph.get(&node) {
                for &n in neighbors {
                    if n == target { return true; }
                    stack.push(n);
                }
            }
        }
        false
    };

    let mut best: Option<((i64, i64), f32)> = None;
    for &(a, b) in pool {
        if asked_set.contains(&(a, b)) { continue; }
        // Transitive: if relation is already deducible, skip
        if reachable(a, b, &graph) || reachable(b, a, &graph) { continue; }

        let pca_a = match pca_data.get(&a) { Some(v) => v, None => continue };
        let pca_b = match pca_data.get(&b) { Some(v) => v, None => continue };
        if pca_a.len() != pca_b.len() || pca_a.len() != weights.len() { continue; }

        let logit: f32 = weights.iter().zip(pca_a.iter()).zip(pca_b.iter())
            .map(|((w, x), y)| w * (x - y))
            .sum();
        let prob = sigmoid(logit);
        let uncertainty = 1.0 - 2.0 * (prob - 0.5).abs();

        // Magnitude tie-break: larger deltas have more learning impact
        let delta_mag: f32 = pca_a.iter().zip(pca_b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>()
            .sqrt();
        let score = uncertainty + 0.05 * delta_mag.min(1.0);

        if best.is_none() || score > best.unwrap().1 {
            best = Some(((a, b), score));
        }
    }
    best.map(|(p, _)| p)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a minimal pca_data map with deterministic vectors.
    fn pca(id: i64, v: &[f32]) -> (i64, Vec<f32>) {
        (id, v.to_vec())
    }

    /// Change A+B: cold-start ranking now uses `prior_gap × delta_mag`
    /// instead of `delta_mag` alone. Pairs that span a big expected
    /// aggression gap AND have a big PCA delta should win.
    #[test]
    fn cold_start_prefers_high_prior_gap_pair() {
        let pca_map: HashMap<i64, Vec<f32>> = [
            pca(1, &[1.0, 0.0, 0.0]),  // ambient-ish
            pca(2, &[0.0, 1.0, 0.0]),  // metal-ish
            pca(3, &[0.9, 0.1, 0.0]),  // ambient neighbor
        ].into_iter().collect();

        // Pool has two candidate pairs with similar delta magnitude,
        // but (1,2) has a much larger expected aggression gap
        // (ambient vs metal) than (1,3) (ambient vs ambient).
        let pool = vec![(1, 2), (1, 3)];
        let asked: Vec<(i64, i64, i32)> = vec![];
        let weights: Vec<f32> = vec![0.0; 3]; // cold start
        let track_community: HashMap<i64, i32> = HashMap::new();
        let recent: Vec<i32> = vec![];

        let mut genre_labels: HashMap<i64, String> = HashMap::new();
        genre_labels.insert(1, "Electronic---Ambient".to_string());
        genre_labels.insert(2, "Rock---Heavy Metal".to_string());
        genre_labels.insert(3, "Electronic---Ambient".to_string());

        let picked = next_calibration_pair_v2(
            &pool, &asked, &pca_map, &weights,
            &track_community, &recent, &genre_labels,
        );
        assert_eq!(picked, Some((1, 2)), "cold-start should prefer the ambient-vs-metal pair over ambient-vs-ambient");
    }

    /// Change A: when labels are missing, prior_gap = 0 and scoring falls
    /// back to the delta-magnitude ranking (unchanged behaviour).
    #[test]
    fn cold_start_without_labels_falls_back_to_delta_magnitude() {
        let pca_map: HashMap<i64, Vec<f32>> = [
            pca(1, &[0.0, 0.0, 0.0]),
            pca(2, &[10.0, 0.0, 0.0]),  // big delta vs 1
            pca(3, &[0.1, 0.0, 0.0]),   // small delta vs 1
        ].into_iter().collect();
        let pool = vec![(1, 2), (1, 3)];
        let empty: HashMap<i64, String> = HashMap::new();
        let picked = next_calibration_pair_v2(
            &pool, &[], &pca_map, &vec![0.0; 3],
            &HashMap::new(), &[], &empty,
        );
        assert_eq!(picked, Some((1, 2)), "without labels, largest delta should win");
    }

    /// Change B: warm-start scoring uses entropy × ||Δ||². The additive
    /// `uncertainty + 0.05*delta_mag` previously rewarded a near-50/50
    /// prediction on a tiny Δ almost as much as on a big Δ. The
    /// multiplicative form should strongly prefer the big-Δ pair.
    #[test]
    fn warm_start_multiplicative_scoring_prefers_big_delta_at_equal_uncertainty() {
        // Weight vector such that both pairs have sigmoid(w·Δ) ≈ 0.5
        // (maximum uncertainty), but one pair has tiny Δ, the other big.
        let pca_map: HashMap<i64, Vec<f32>> = [
            pca(1, &[0.0]),
            pca(2, &[0.01]),   // |Δ|=0.01 — tiny
            pca(3, &[0.0]),
            pca(4, &[1.0]),    // |Δ|=1.0 — big
        ].into_iter().collect();
        // weight = 0 → sigmoid = 0.5 for every pair → max entropy; delta
        // decides. But weight=0 triggers cold-start path (norm < 0.01).
        // Use tiny non-zero weight to force warm path.
        let weights = vec![1e-4f32];
        let pool = vec![(1, 2), (3, 4)];
        let empty_labels: HashMap<i64, String> = HashMap::new();
        let picked = next_calibration_pair_v2(
            &pool, &[], &pca_map, &weights,
            &HashMap::new(), &[], &empty_labels,
        );
        assert_eq!(picked, Some((3, 4)), "warm scoring should strongly prefer big-Δ pair at equal uncertainty");
    }

    /// Change D: with 3 macros (one dominant, others small), Phase 1 should
    /// include at least one cross-macro tier-1 pair (spanning the largest
    /// prior gap), and tier-3 sub-community skip-chain only fires for the
    /// dominant macro.
    #[test]
    fn build_calibration_plan_tiers_on_synthetic_library() {
        // Three macros: DnB (dominant, 180 tracks), Rock (small, 40), Jazz (tiny, 18).
        // DnB tracks 1-180, Rock 181-220, Jazz 221-238.
        // Two DnB sub-communities (clusters 1 and 2), one Rock (3), one Jazz (4).
        let mut pca_data: HashMap<i64, Vec<f32>> = HashMap::new();
        let mut genre_labels: HashMap<i64, String> = HashMap::new();

        // DnB sub-community A (100 tracks)
        for i in 1..=100 {
            pca_data.insert(i, vec![i as f32 * 0.01, 0.0, 0.0]);
            genre_labels.insert(i, "Electronic---Drum n Bass".to_string());
        }
        // DnB sub-community B (80 tracks)
        for i in 101..=180 {
            pca_data.insert(i, vec![0.0, i as f32 * 0.01, 0.0]);
            genre_labels.insert(i, "Electronic---Drum n Bass".to_string());
        }
        // Rock (40 tracks)
        for i in 181..=220 {
            pca_data.insert(i, vec![0.0, 0.0, i as f32 * 0.01]);
            genre_labels.insert(i, "Rock---Heavy Metal".to_string());
        }
        // Jazz (18 tracks)
        for i in 221..=238 {
            pca_data.insert(i, vec![(i as f32) * 0.005, 0.0, 0.0]);
            genre_labels.insert(i, "Jazz---Bebop".to_string());
        }

        let uncovered = vec![
            UncoveredCommunity {
                cluster_id: 1,
                track_count: 100,
                coverage_pct: 0.0,
                representative_genre: "Drum n Bass".to_string(),
                track_ids: (1..=100).collect(),
            },
            UncoveredCommunity {
                cluster_id: 2,
                track_count: 80,
                coverage_pct: 0.0,
                representative_genre: "Drum n Bass".to_string(),
                track_ids: (101..=180).collect(),
            },
            UncoveredCommunity {
                cluster_id: 3,
                track_count: 40,
                coverage_pct: 0.0,
                representative_genre: "Heavy Metal".to_string(),
                track_ids: (181..=220).collect(),
            },
            UncoveredCommunity {
                cluster_id: 4,
                track_count: 18,
                coverage_pct: 0.0,
                representative_genre: "Bebop".to_string(),
                track_ids: (221..=238).collect(),
            },
        ];

        let anchor_refs = vec![1i64, 181, 221];
        let plan = build_calibration_plan(&uncovered, &pca_data, &genre_labels, &anchor_refs, 0);

        // Tier 1 should contribute at least one cross-macro pair — find one
        // where both tracks belong to different macros.
        let has_cross_macro = plan.phase_1.iter().any(|&(a, b)| {
            let ma = crate::genre_map::macro_genre_for(genre_labels.get(&a).map(|s| s.as_str()).unwrap_or(""));
            let mb = crate::genre_map::macro_genre_for(genre_labels.get(&b).map(|s| s.as_str()).unwrap_or(""));
            ma != mb
        });
        assert!(has_cross_macro, "tier 1 should introduce at least one cross-macro pair");

        // Phase 1 should be substantially smaller than the old 40+ question flow.
        // With DnB dominant (180 tracks ≥ 150 threshold), tier 3 fires: 2 DnB
        // sub-communities × 3 skip-chain = 6. Tier 1 up to 5. Tier 2 for DnB
        // only (the only ≥ 50-track macro). Tier 4 for Rock + Jazz.
        // Expected total: somewhere in [10, 20].
        assert!(plan.phase_1.len() >= 5, "phase 1 should have at least 5 pairs, got {}", plan.phase_1.len());
        assert!(plan.phase_1.len() <= 25, "phase 1 should not exceed 25 pairs, got {}", plan.phase_1.len());

        // Phase 2 pool should be non-empty (all remaining rep combinations).
        assert!(!plan.phase_2.is_empty(), "phase 2 pool should be populated");
    }

    /// Regression: graceful degradation when genre_labels is empty.
    #[test]
    fn build_calibration_plan_without_genre_labels() {
        let mut pca_data: HashMap<i64, Vec<f32>> = HashMap::new();
        for i in 1..=40 {
            pca_data.insert(i, vec![i as f32 * 0.01, 0.0]);
        }
        let uncovered = vec![UncoveredCommunity {
            cluster_id: 1,
            track_count: 40,
            coverage_pct: 0.0,
            representative_genre: "Unknown".to_string(),
            track_ids: (1..=40).collect(),
        }];
        let empty_labels: HashMap<i64, String> = HashMap::new();
        let plan = build_calibration_plan(&uncovered, &pca_data, &empty_labels, &[], 0);
        // Should still build without panicking. All tracks fall into "Other"
        // macro (40 tracks, below the 50 threshold) so only tier 4 fires —
        // and with no anchor_refs, tier 4 adds nothing either.
        // Phase 1 may be empty; phase 2 pool carries the load.
        assert!(!plan.phase_2.is_empty(), "phase 2 should still contain intra-community pairs");
    }
}
