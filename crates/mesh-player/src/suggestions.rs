//! Smart suggestion engine for the collection browser
//!
//! Queries the CozoDB HNSW index to find tracks similar to the currently
//! loaded deck seeds, then re-scores them using a unified multi-factor formula
//! with energy-direction-aware harmonic scoring.

use std::collections::HashMap;
use std::sync::LazyLock;
use mesh_core::db::{DatabaseService, Track};
use mesh_core::music::MusicalKey;
use crate::config::KeyScoringModel;

/// A suggested track with its computed score (lower = better match)
#[derive(Debug, Clone)]
pub struct SuggestedTrack {
    pub track: Track,
    pub score: f32,
}

/// Classification of the musical relationship between two keys.
///
/// Every pair of keys on the Camelot wheel maps to exactly one transition type.
/// This drives both the base compatibility score and the energy-direction modifier.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TransitionType {
    /// Same key — 0 steps, same mode (7/7 shared notes)
    SameKey,
    /// Adjacent clockwise — +1 step, same mode (energy lift, 6/7 shared notes)
    AdjacentUp,
    /// Adjacent counter-clockwise — -1 step, same mode (energy cool, 6/7 shared notes)
    AdjacentDown,
    /// Safe diagonal up — cross-mode energy lift: B(n)→A(n+1) or A(n)→B(n-1)
    DiagonalUp,
    /// Safe diagonal down — cross-mode energy cool (reverse of DiagonalUp)
    DiagonalDown,
    /// Energy boost — +2 steps, same mode (5/7 shared notes)
    EnergyBoost,
    /// Energy cool — -2 steps, same mode (5/7 shared notes)
    EnergyCool,
    /// Mood lift — A→B same position (minor→major, 7/7 shared notes)
    MoodLift,
    /// Mood darken — B→A same position (major→minor, 7/7 shared notes)
    MoodDarken,
    /// Semitone up — +7 steps same mode (pop key change, ~3/7 shared notes)
    SemitoneUp,
    /// Semitone down — -5 steps same mode (~3/7 shared notes)
    SemitoneDown,
    /// Far same-mode step (±3, ±4, ±5)
    FarStep(i8),
    /// Far cross-mode step (non-diagonal, ±2+)
    FarCross(i8),
    /// Tritone — ±6 steps (maximum dissonance, 2/7 shared notes)
    Tritone,
}

/// Classify the musical relationship between two keys on the Camelot wheel.
///
/// Uses signed circular distance to distinguish directional movements
/// (e.g., +1 clockwise vs -1 counter-clockwise).
fn classify_transition(seed: &MusicalKey, candidate: &MusicalKey) -> TransitionType {
    let (s_pos, s_letter) = seed.camelot();
    let (c_pos, c_letter) = candidate.camelot();

    let same_mode = s_letter == c_letter;

    // Signed circular step on the 12-position wheel
    let raw = (c_pos as i8) - (s_pos as i8);
    let step = if raw > 6 { raw - 12 } else if raw < -6 { raw + 12 } else { raw };
    let abs_step = step.unsigned_abs();

    if same_mode {
        match step {
            0 => TransitionType::SameKey,
            1 => TransitionType::AdjacentUp,
            -1 => TransitionType::AdjacentDown,
            2 => TransitionType::EnergyBoost,
            -2 => TransitionType::EnergyCool,
            // +7 clockwise = -5 wrapped = one semitone up in pitch
            -5 => TransitionType::SemitoneUp,
            // -7 clockwise = +5 wrapped = one semitone down in pitch
            5 => TransitionType::SemitoneDown,
            6 | -6 => TransitionType::Tritone,
            s if abs_step >= 3 && abs_step <= 4 => TransitionType::FarStep(s),
            _ => TransitionType::FarStep(step),
        }
    } else {
        // Cross-mode transitions
        match step {
            0 => {
                // Same Camelot position, different letter
                if s_letter == 'A' {
                    TransitionType::MoodLift // minor → major
                } else {
                    TransitionType::MoodDarken // major → minor
                }
            }
            // Relative major/minor share the same key signature
            // Am (8A) ↔ C (8B) → relative, but they have the same position
            // Actually: relative = same position different letter = handled above.
            // The "relative" in Camelot terms is same-number A↔B.
            // But we also need the safe diagonals:
            // From B(n), going to A(n+1) is safe diagonal up
            // From A(n), going to B(n-1) is safe diagonal up too (perspective of the mover)
            1 => {
                // +1 step cross-mode
                if s_letter == 'B' {
                    // B(n) → A(n+1): safe diagonal up
                    TransitionType::DiagonalUp
                } else {
                    // A(n) → B(n+1): risky cross
                    TransitionType::FarCross(step)
                }
            }
            -1 => {
                // -1 step cross-mode
                if s_letter == 'A' {
                    // A(n) → B(n-1): safe diagonal down
                    TransitionType::DiagonalDown
                } else {
                    // B(n) → A(n-1): risky cross
                    TransitionType::FarCross(step)
                }
            }
            _ => TransitionType::FarCross(step),
        }
    }
}

/// Compute the base compatibility score for a transition type (energy_bias = 0).
///
/// Returns 0.0 (worst) to 1.0 (best). Nothing returns exactly 0.0 — even the
/// tritone has a small score, allowing the adaptive filter to unlock it at extremes.
fn base_score(tt: TransitionType) -> f32 {
    match tt {
        TransitionType::SameKey => 1.00,
        TransitionType::AdjacentUp => 0.85,
        TransitionType::AdjacentDown => 0.85,
        TransitionType::DiagonalUp => 0.75,
        TransitionType::DiagonalDown => 0.75,
        TransitionType::MoodLift => 0.70,
        TransitionType::MoodDarken => 0.70,
        TransitionType::EnergyBoost => 0.50,
        TransitionType::EnergyCool => 0.50,
        TransitionType::SemitoneUp => 0.20,
        TransitionType::SemitoneDown => 0.20,
        TransitionType::FarStep(s) => {
            match s.unsigned_abs() {
                3 => 0.25,
                4 => 0.15,
                5 => 0.08,
                _ => 0.05,
            }
        }
        TransitionType::FarCross(_) => 0.10,
        TransitionType::Tritone => 0.03,
    }
}

/// Compute the energy-dependent modifier for a transition type.
///
/// The modifier scales linearly with `|energy_bias|` and is 0.0 at center.
/// Positive modifier = bonus (raises score), negative = penalty (lowers score).
fn energy_modifier(tt: TransitionType, energy_bias: f32) -> f32 {
    if energy_bias.abs() < 0.05 {
        return 0.0;
    }

    let abs_bias = energy_bias.abs();

    let raw = if energy_bias > 0.0 {
        // Raising energy (fader right of center)
        match tt {
            TransitionType::SemitoneUp => 0.35,
            TransitionType::EnergyBoost => 0.30,
            TransitionType::MoodLift => 0.20,
            TransitionType::DiagonalUp => 0.15,
            TransitionType::AdjacentUp => 0.10,
            TransitionType::FarStep(s) if s > 0 => 0.05,
            TransitionType::AdjacentDown => -0.15,
            TransitionType::MoodDarken => -0.15,
            TransitionType::EnergyCool => -0.20,
            TransitionType::DiagonalDown => -0.10,
            _ => 0.0,
        }
    } else {
        // Dropping energy (fader left of center)
        match tt {
            TransitionType::EnergyCool => 0.25,
            TransitionType::SemitoneDown => 0.20,
            TransitionType::MoodDarken => 0.20,
            TransitionType::Tritone => 0.15,
            TransitionType::DiagonalDown => 0.15,
            TransitionType::AdjacentDown => 0.10,
            TransitionType::FarStep(s) => {
                match s.unsigned_abs() {
                    3 => 0.10,
                    4 | 5 => 0.08,
                    _ => 0.0,
                }
            }
            TransitionType::AdjacentUp => -0.15,
            TransitionType::MoodLift => -0.15,
            TransitionType::EnergyBoost => -0.20,
            TransitionType::DiagonalUp => -0.10,
            _ => 0.0,
        }
    };

    raw * abs_bias
}

// ─── Krumhansl-Kessler Perceptual Key Distance ──────────────────────

/// Krumhansl-Kessler probe-tone profiles (Krumhansl & Kessler, 1982).
///
/// Each array represents how well each of the 12 chromatic pitch classes
/// "fits" in the given key context, as rated by listeners.
const MAJOR_PROFILE: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

/// 24×24 Pearson correlation matrix between all key profiles.
///
/// Index convention: 0-11 = C major through B major, 12-23 = C minor through B minor.
/// Computed once at first access via `LazyLock`.
static KRUMHANSL_MATRIX: LazyLock<[[f32; 24]; 24]> = LazyLock::new(compute_krumhansl_matrix);

fn compute_krumhansl_matrix() -> [[f32; 24]; 24] {
    // Build 24 profiles by rotating the major/minor templates
    let mut profiles = [[0.0f32; 12]; 24];
    for root in 0..12 {
        for pitch in 0..12 {
            profiles[root][pitch] = MAJOR_PROFILE[(pitch + 12 - root) % 12];
            profiles[root + 12][pitch] = MINOR_PROFILE[(pitch + 12 - root) % 12];
        }
    }
    // Pearson correlation between all pairs
    let mut matrix = [[0.0f32; 24]; 24];
    for i in 0..24 {
        for j in 0..24 {
            matrix[i][j] = pearson_correlation(&profiles[i], &profiles[j]);
        }
    }
    matrix
}

fn pearson_correlation(x: &[f32; 12], y: &[f32; 12]) -> f32 {
    let n = 12.0;
    let mean_x: f32 = x.iter().sum::<f32>() / n;
    let mean_y: f32 = y.iter().sum::<f32>() / n;
    let mut cov = 0.0f32;
    let mut var_x = 0.0f32;
    let mut var_y = 0.0f32;
    for i in 0..12 {
        let dx = x[i] - mean_x;
        let dy = y[i] - mean_y;
        cov += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }
    let denom = (var_x * var_y).sqrt();
    if denom < 1e-10 { 0.0 } else { cov / denom }
}

/// Look up Krumhansl perceptual similarity as a base score (0.0–1.0).
///
/// The correlation ranges from ~-0.1 to 1.0. We clamp to a small positive
/// floor so that even the worst transitions are never completely zero
/// (matching the Camelot model's behavior).
fn krumhansl_base_score(seed: &MusicalKey, candidate: &MusicalKey) -> f32 {
    let s_idx = seed.root as usize + if seed.minor { 12 } else { 0 };
    let c_idx = candidate.root as usize + if candidate.minor { 12 } else { 0 };
    let r = KRUMHANSL_MATRIX[s_idx][c_idx];
    r.max(0.02)
}

// ─── Unified Key Transition Score ───────────────────────────────────

/// Compute the energy-direction-aware key transition score.
///
/// Combines a base compatibility score with an energy-dependent modifier to produce
/// a single score from 0.0 (worst) to ~1.0 (best).
///
/// At `energy_bias = 0.0` (fader center), this returns the base compatibility score,
/// producing behavior identical to v1 for safe transitions.
///
/// The `model` parameter selects whether base scores come from the hand-tuned
/// Camelot categories or the Krumhansl correlation matrix. Energy modifiers
/// always use Camelot-based transition classification for directionality.
fn key_transition_score(
    seed_key: &MusicalKey,
    cand_key: &MusicalKey,
    energy_bias: f32,
    model: KeyScoringModel,
) -> f32 {
    let tt = classify_transition(seed_key, cand_key);
    let base = match model {
        KeyScoringModel::Camelot => base_score(tt),
        KeyScoringModel::Krumhansl => krumhansl_base_score(seed_key, cand_key),
    };
    let modifier = energy_modifier(tt, energy_bias);
    (base + modifier).clamp(0.0, 1.0)
}

/// Compute the adaptive filter threshold based on energy bias.
///
/// At fader center, the threshold is strict (only safe transitions pass).
/// As the fader moves toward extremes, the threshold relaxes to allow
/// dramatic key changes.
fn adaptive_filter_threshold(energy_bias: f32) -> f32 {
    let abs_bias = energy_bias.abs();
    if abs_bias < 0.1 {
        0.50 // Strict: same-key, adjacent, relative only
    } else if abs_bias < 0.4 {
        0.35 // Moderate: +2 energy boosts start passing
    } else if abs_bias < 0.7 {
        0.20 // Strong: semitone lifts, ±3 pass
    } else {
        0.10 // Extreme: nearly everything except tritone
    }
}

/// Compute LUFS-based direction bonus.
///
/// Rewards candidates that are louder when raising energy, quieter when dropping.
/// Returns ±0.1 max, negative = better match.
fn lufs_direction_bonus(candidate_lufs: Option<f32>, avg_seed_lufs: f32, energy_bias: f32) -> f32 {
    if energy_bias.abs() < 0.05 {
        return 0.0;
    }
    candidate_lufs
        .map(|l| {
            let diff = l - avg_seed_lufs; // positive = louder
            let alignment = diff * energy_bias; // positive when matching desired direction
            -(alignment.clamp(-6.0, 6.0) / 60.0) // ±0.1 max, negative = better
        })
        .unwrap_or(0.0)
}

/// Query the database for track suggestions based on loaded deck seeds.
///
/// This runs on a background thread via `Task::perform()`.
///
/// # Algorithm
/// 1. Resolve each seed path to a track ID
/// 2. For each seed, find similar tracks via HNSW index
/// 3. Merge results keeping the best (minimum) distance per candidate
/// 4. Score using unified formula: hnsw + key_transition + lufs + bpm
/// 5. Filter by adaptive harmonic threshold
/// 6. Sort and return the top results
pub fn query_suggestions(
    db: &DatabaseService,
    seed_paths: Vec<String>,
    energy_direction: f32,
    key_scoring_model: KeyScoringModel,
    per_seed_limit: usize,
    total_limit: usize,
) -> Result<Vec<SuggestedTrack>, String> {
    // Step 1: Resolve seed paths to track IDs
    let mut seed_tracks: Vec<Track> = Vec::new();
    for path in &seed_paths {
        match db.get_track_by_path(path) {
            Ok(Some(track)) if track.id.is_some() => {
                seed_tracks.push(track);
            }
            Ok(_) => {
                log::debug!("Suggestion seed not in database: {}", path);
            }
            Err(e) => {
                log::warn!("Failed to look up seed track {}: {}", path, e);
            }
        }
    }

    if seed_tracks.is_empty() {
        return Ok(Vec::new());
    }

    let seed_ids: Vec<i64> = seed_tracks.iter().filter_map(|t| t.id).collect();

    // Step 2 & 3: Query similar tracks for each seed and merge
    let mut candidates: HashMap<i64, (Track, f32)> = HashMap::new();

    for &seed_id in &seed_ids {
        match db.find_similar_tracks(seed_id, per_seed_limit) {
            Ok(results) => {
                for (track, distance) in results {
                    if let Some(track_id) = track.id {
                        // Skip seed tracks themselves
                        if seed_ids.contains(&track_id) {
                            continue;
                        }
                        // Keep minimum distance per candidate
                        candidates
                            .entry(track_id)
                            .and_modify(|(_, existing_dist)| {
                                if distance < *existing_dist {
                                    *existing_dist = distance;
                                }
                            })
                            .or_insert((track, distance));
                    }
                }
            }
            Err(e) => {
                log::warn!("Similarity query failed for seed {}: {}", seed_id, e);
            }
        }
    }

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Step 4: Compute seed averages for scoring
    let avg_seed_lufs = {
        let lufs_values: Vec<f32> = seed_tracks.iter().filter_map(|t| t.lufs).collect();
        if lufs_values.is_empty() {
            -9.0 // fallback
        } else {
            lufs_values.iter().sum::<f32>() / lufs_values.len() as f32
        }
    };

    let avg_seed_bpm = {
        let bpm_values: Vec<f64> = seed_tracks.iter().filter_map(|t| t.bpm).collect();
        if bpm_values.is_empty() {
            128.0
        } else {
            bpm_values.iter().sum::<f64>() / bpm_values.len() as f64
        }
    };

    // Collect seed keys for harmonic scoring
    let seed_keys: Vec<MusicalKey> = seed_tracks
        .iter()
        .filter_map(|t| t.key.as_deref().and_then(MusicalKey::parse))
        .collect();

    // Energy direction bias: -1.0 (drop) through 0.0 (maintain) to +1.0 (peak)
    let energy_bias = (energy_direction - 0.5) * 2.0;
    let filter_threshold = adaptive_filter_threshold(energy_bias);

    // Step 5: Unified scoring — single formula for all candidates
    //
    // score = 0.40 * hnsw_dist           (audio similarity / genre / style)
    //       + 0.30 * (1.0 - key_score)   (harmonic compatibility, energy-aware)
    //       + 0.15 * lufs_bonus           (loudness alignment with energy intent)
    //       + 0.15 * bpm_penalty          (tempo compatibility)
    let mut suggestions: Vec<SuggestedTrack> = candidates
        .into_values()
        .filter_map(|(track, hnsw_dist)| {
            // Key transition score: best match across all seeds
            let best_key_score = track
                .key
                .as_deref()
                .and_then(|k| MusicalKey::parse(k))
                .map(|ck| {
                    seed_keys
                        .iter()
                        .map(|sk| key_transition_score(sk, &ck, energy_bias, key_scoring_model))
                        .fold(0.0f32, f32::max)
                })
                .unwrap_or(0.3); // No key = moderate penalty

            // Apply adaptive filter threshold
            if best_key_score < filter_threshold {
                return None;
            }

            let key_penalty = 1.0 - best_key_score;

            // LUFS direction bonus (negative = better when aligned)
            let lufs_bonus = lufs_direction_bonus(track.lufs, avg_seed_lufs, energy_bias);

            // BPM penalty: normalized distance (10 BPM diff → 1.0 penalty)
            let bpm_penalty = track
                .bpm
                .map(|b| {
                    let diff = (b - avg_seed_bpm).abs();
                    (diff / 10.0).min(1.0) as f32
                })
                .unwrap_or(0.5);

            let score = 0.40 * hnsw_dist
                + 0.30 * key_penalty
                + 0.15 * lufs_bonus
                + 0.15 * bpm_penalty;

            Some(SuggestedTrack { track, score })
        })
        .collect();

    // Step 6: Sort ascending (lower score = better match) and limit
    suggestions.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
    suggestions.truncate(total_limit);

    Ok(suggestions)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Transition Classification ──────────────────────────────────

    #[test]
    fn test_classify_same_key() {
        let am = MusicalKey::parse("Am").unwrap();
        assert_eq!(classify_transition(&am, &am), TransitionType::SameKey);
    }

    #[test]
    fn test_classify_relative() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let c = MusicalKey::parse("C").unwrap();   // 8B
        // Am→C = minor→major = MoodLift (same position, A→B)
        assert_eq!(classify_transition(&am, &c), TransitionType::MoodLift);
        // C→Am = major→minor = MoodDarken
        assert_eq!(classify_transition(&c, &am), TransitionType::MoodDarken);
    }

    #[test]
    fn test_classify_adjacent() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let em = MusicalKey::parse("Em").unwrap(); // 9A
        let dm = MusicalKey::parse("Dm").unwrap(); // 7A
        assert_eq!(classify_transition(&am, &em), TransitionType::AdjacentUp);
        assert_eq!(classify_transition(&am, &dm), TransitionType::AdjacentDown);
    }

    #[test]
    fn test_classify_energy_boost() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let bm = MusicalKey::parse("Bm").unwrap(); // 10A
        assert_eq!(classify_transition(&am, &bm), TransitionType::EnergyBoost);
    }

    #[test]
    fn test_classify_energy_cool() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let gm = MusicalKey::parse("Gm").unwrap(); // 6A
        assert_eq!(classify_transition(&am, &gm), TransitionType::EnergyCool);
    }

    #[test]
    fn test_classify_mood_lift_darken() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let c = MusicalKey::parse("C").unwrap();   // 8B
        assert_eq!(classify_transition(&am, &c), TransitionType::MoodLift);
        assert_eq!(classify_transition(&c, &am), TransitionType::MoodDarken);
    }

    #[test]
    fn test_classify_diagonal() {
        // B(n)→A(n+1) = safe diagonal up
        let c = MusicalKey::parse("C").unwrap();   // 8B
        let em = MusicalKey::parse("Em").unwrap(); // 9A
        assert_eq!(classify_transition(&c, &em), TransitionType::DiagonalUp);

        // A(n)→B(n-1) = safe diagonal down: 8A → 7B = F major
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let f = MusicalKey::parse("F").unwrap();   // 7B
        assert_eq!(classify_transition(&am, &f), TransitionType::DiagonalDown);
    }

    #[test]
    fn test_classify_semitone() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        // +7 same mode = semitone up
        // 8A + 7 = 15 → 15-12 = 3A = Bbm/A#m? Let's check: Camelot 3A = Bbm
        let bbm = MusicalKey::parse("Bbm").unwrap(); // 3A
        assert_eq!(classify_transition(&am, &bbm), TransitionType::SemitoneUp);
    }

    #[test]
    fn test_classify_tritone() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        // ±6 same mode = tritone. 8A + 6 = 14 → 2A = Ebm
        let ebm = MusicalKey::parse("Ebm").unwrap(); // 2A
        assert_eq!(classify_transition(&am, &ebm), TransitionType::Tritone);
    }

    #[test]
    fn test_classify_far_step() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        // +3 same mode = FarStep(3). 8A + 3 = 11A = F#m
        let fsm = MusicalKey::parse("F#m").unwrap(); // 11A
        assert_eq!(classify_transition(&am, &fsm), TransitionType::FarStep(3));
    }

    // ─── Key Transition Score (Camelot model) ─────────────────────────

    const CAM: KeyScoringModel = KeyScoringModel::Camelot;

    #[test]
    fn test_key_score_center_same_key() {
        let am = MusicalKey::parse("Am").unwrap();
        assert_eq!(key_transition_score(&am, &am, 0.0, CAM), 1.0);
    }

    #[test]
    fn test_key_score_center_adjacent() {
        let am = MusicalKey::parse("Am").unwrap();
        let em = MusicalKey::parse("Em").unwrap();
        assert_eq!(key_transition_score(&am, &em, 0.0, CAM), 0.85);
    }

    #[test]
    fn test_key_score_center_is_symmetric_for_adjacent() {
        let am = MusicalKey::parse("Am").unwrap();
        let em = MusicalKey::parse("Em").unwrap();
        let dm = MusicalKey::parse("Dm").unwrap();
        // At center, +1 and -1 should have same base score
        assert_eq!(
            key_transition_score(&am, &em, 0.0, CAM),
            key_transition_score(&am, &dm, 0.0, CAM)
        );
    }

    #[test]
    fn test_key_score_raise_prefers_up() {
        let am = MusicalKey::parse("Am").unwrap();
        let em = MusicalKey::parse("Em").unwrap(); // +1 up
        let dm = MusicalKey::parse("Dm").unwrap(); // -1 down
        let up_score = key_transition_score(&am, &em, 1.0, CAM);
        let down_score = key_transition_score(&am, &dm, 1.0, CAM);
        assert!(up_score > down_score, "Raising energy should prefer +1 over -1");
    }

    #[test]
    fn test_key_score_raise_unlocks_energy_boost() {
        let am = MusicalKey::parse("Am").unwrap();
        let bm = MusicalKey::parse("Bm").unwrap(); // +2 energy boost
        let center_score = key_transition_score(&am, &bm, 0.0, CAM);
        let peak_score = key_transition_score(&am, &bm, 1.0, CAM);
        assert!(peak_score > center_score, "Energy boost should improve at peak");
        assert!(peak_score >= 0.75, "Energy boost at peak should be competitive: {}", peak_score);
    }

    #[test]
    fn test_key_score_raise_unlocks_semitone_up() {
        let am = MusicalKey::parse("Am").unwrap();
        let bbm = MusicalKey::parse("Bbm").unwrap(); // +7 semitone up
        let center_score = key_transition_score(&am, &bbm, 0.0, CAM);
        let peak_score = key_transition_score(&am, &bbm, 1.0, CAM);
        assert!(peak_score > center_score, "Semitone up should improve at peak");
        assert!(peak_score >= 0.50, "Semitone up at peak should be viable: {}", peak_score);
    }

    #[test]
    fn test_key_score_drop_unlocks_tritone() {
        let am = MusicalKey::parse("Am").unwrap();
        let ebm = MusicalKey::parse("Ebm").unwrap(); // tritone
        let center_score = key_transition_score(&am, &ebm, 0.0, CAM);
        let drop_score = key_transition_score(&am, &ebm, -1.0, CAM);
        assert!(drop_score > center_score, "Tritone should improve at drop");
        assert!(drop_score > 0.10, "Tritone at drop should be viable: {}", drop_score);
    }

    #[test]
    fn test_key_score_mood_lift_boosted_when_raising() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let c = MusicalKey::parse("C").unwrap();   // 8B
        let center_score = key_transition_score(&am, &c, 0.0, CAM);
        let peak_score = key_transition_score(&am, &c, 1.0, CAM);
        assert!(peak_score > center_score, "Mood lift should improve when raising energy");
    }

    #[test]
    fn test_key_score_mood_darken_boosted_when_dropping() {
        let c = MusicalKey::parse("C").unwrap();   // 8B
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let center_score = key_transition_score(&c, &am, 0.0, CAM);
        let drop_score = key_transition_score(&c, &am, -1.0, CAM);
        assert!(drop_score > center_score, "Mood darken should improve when dropping energy");
    }

    // ─── Adaptive Filter Threshold ──────────────────────────────────

    #[test]
    fn test_filter_threshold_strictest_at_center() {
        assert_eq!(adaptive_filter_threshold(0.0), 0.50);
    }

    #[test]
    fn test_filter_threshold_relaxes_at_extremes() {
        let center = adaptive_filter_threshold(0.0);
        let extreme = adaptive_filter_threshold(1.0);
        assert!(extreme < center, "Threshold should be lower at extremes");
        assert_eq!(extreme, 0.10);
    }

    // ─── LUFS Direction Bonus ───────────────────────────────────────

    #[test]
    fn test_lufs_direction_center_is_zero() {
        assert_eq!(lufs_direction_bonus(Some(-8.0), -10.0, 0.0), 0.0);
    }

    #[test]
    fn test_lufs_direction_raise_prefers_louder() {
        let louder = lufs_direction_bonus(Some(-6.0), -10.0, 1.0); // +4 dB
        let quieter = lufs_direction_bonus(Some(-14.0), -10.0, 1.0); // -4 dB
        assert!(louder < quieter, "Lower score = better, louder should score lower when raising");
    }

    #[test]
    fn test_lufs_direction_drop_prefers_quieter() {
        let louder = lufs_direction_bonus(Some(-6.0), -10.0, -1.0);
        let quieter = lufs_direction_bonus(Some(-14.0), -10.0, -1.0);
        assert!(quieter < louder, "Quieter should score lower when dropping");
    }

    // ─── Krumhansl Matrix Validation ────────────────────────────────

    #[test]
    fn test_krumhansl_same_key_is_one() {
        let m = &*KRUMHANSL_MATRIX;
        for i in 0..24 {
            assert!((m[i][i] - 1.0).abs() < 0.001, "Same key {i} should correlate 1.0");
        }
    }

    #[test]
    fn test_krumhansl_symmetric() {
        let m = &*KRUMHANSL_MATRIX;
        for i in 0..24 {
            for j in 0..24 {
                assert!((m[i][j] - m[j][i]).abs() < 0.001,
                    "Matrix should be symmetric: [{i}][{j}]={} vs [{j}][{i}]={}", m[i][j], m[j][i]);
            }
        }
    }

    #[test]
    fn test_krumhansl_relative_keys_close() {
        // Am (root=9, minor=true, idx=21) ↔ C (root=0, major=false, idx=0)
        let m = &*KRUMHANSL_MATRIX;
        let am_c = m[21][0]; // Am→C
        assert!(am_c > 0.5, "Relative keys should be close: {am_c}");
    }

    #[test]
    fn test_krumhansl_parallel_keys_moderate() {
        // C major (idx=0) ↔ C minor (idx=12)
        let m = &*KRUMHANSL_MATRIX;
        let c_cm = m[0][12];
        assert!(c_cm > 0.3, "Parallel keys should be moderately close: {c_cm}");
    }

    #[test]
    fn test_krumhansl_tritone_distant() {
        // C (idx=0) ↔ F# (idx=6)
        let m = &*KRUMHANSL_MATRIX;
        let c_fs = m[0][6];
        assert!(c_fs < 0.2, "Tritone should be distant: {c_fs}");
    }

    #[test]
    fn test_key_score_model_switching() {
        let am = MusicalKey::parse("Am").unwrap();
        let c = MusicalKey::parse("C").unwrap();
        let camelot = key_transition_score(&am, &c, 0.0, KeyScoringModel::Camelot);
        let krumhansl = key_transition_score(&am, &c, 0.0, KeyScoringModel::Krumhansl);
        // Both should rate Am→C highly, but values will differ
        assert!(camelot > 0.5, "Camelot Am→C should be high: {camelot}");
        assert!(krumhansl > 0.5, "Krumhansl Am→C should be high: {krumhansl}");
    }
}
