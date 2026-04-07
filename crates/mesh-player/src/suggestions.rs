//! Smart suggestion engine for the collection browser
//!
//! Queries the CozoDB HNSW index to find tracks similar to the currently
//! loaded deck seeds, then re-scores them using a unified multi-factor formula
//! with energy-direction-aware harmonic scoring.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use mesh_core::db::{DatabaseService, MlScores, Track};
use mesh_core::music::MusicalKey;
use crate::config::KeyScoringModel;

/// A suggested track with its computed score (lower = better match)
#[derive(Debug, Clone)]
pub struct SuggestedTrack {
    pub track: Track,
    pub score: f32,
    /// Auto-generated reason tags as (label, hex_color)
    pub reason_tags: Vec<(String, Option<String>)>,
    /// Playlist names this track belongs to (populated after query)
    pub playlists: Vec<String>,
}

/// A database source for suggestion queries.
///
/// Each source represents a separate track library (local collection or USB device).
/// Seeds are resolved per-source, and HNSW vector search runs across all sources
/// to find the best matches regardless of which library they're in.
pub struct DbSource {
    pub db: Arc<DatabaseService>,
    pub collection_root: PathBuf,
    /// Human-readable name (e.g., "Local" or USB device label)
    pub name: String,
}

/// Result of a split suggestion query separating playlist-local from global results.
#[derive(Debug)]
///
/// When a playlist is selected in the browser, the top section shows tracks that
/// exist in that playlist (scored identically to global results), and the bottom
/// section shows global cross-collection candidates. When no playlist is selected,
/// all results land in `global_suggestions` and `playlist_suggestions` is empty.
pub struct SplitSuggestions {
    /// Tracks found within the currently selected playlist/folder (shown first, no tint)
    pub playlist_suggestions: Vec<SuggestedTrack>,
    /// Tracks from all collections — global fallback (shown second, visually tinted)
    pub global_suggestions: Vec<SuggestedTrack>,
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

/// Inherent energy direction of a transition type.
///
/// Ranges from -0.80 (strong energy drop) to +0.70 (strong energy lift).
/// Based on research into the emotional impact of key transitions in DJ mixing:
/// - **Semitone up** (+7 Camelot): Visceral pitch lift, strongest energy surge (+0.70)
/// - **Energy boost** (+2): Dramatic whole-step lift, "hands in the air" (+0.50)
/// - **Mood lift** (minor→major): Emotional brightening, "sun coming out" (+0.30)
/// - **Adjacent up** (+1): Gentle forward momentum via dominant modulation (+0.20)
/// - **Diagonal up**: Complex lift combining energy + mood shift (+0.15)
/// - **Same key**: Perfectly neutral — maintains current energy level (0.00)
/// - **Diagonal down**: Complex cooldown with mood shift (-0.15)
/// - **Adjacent down** (-1): Gentle relaxation via subdominant ("plagal") (-0.20)
/// - **Mood darken** (major→minor): Emotional darkening, introspective (-0.30)
/// - **Energy cool** (-2): Strong energy drain, whole-step descent (-0.50)
/// - **Semitone down** (-7): Dramatic settling/sinking sensation (-0.50)
/// - **Tritone** (6 steps): Maximum dissonance, chaotic tension (-0.80)
fn transition_energy_direction(tt: TransitionType) -> f32 {
    match tt {
        TransitionType::SemitoneUp => 0.70,
        TransitionType::EnergyBoost => 0.50,
        TransitionType::MoodLift => 0.30,
        TransitionType::AdjacentUp => 0.20,
        TransitionType::DiagonalUp => 0.15,
        TransitionType::SameKey => 0.0,
        TransitionType::FarStep(s) => s.signum() as f32 * 0.10,
        TransitionType::FarCross(s) => s.signum() as f32 * 0.05,
        TransitionType::DiagonalDown => -0.15,
        TransitionType::AdjacentDown => -0.20,
        TransitionType::MoodDarken => -0.30,
        TransitionType::EnergyCool => -0.50,
        TransitionType::SemitoneDown => -0.50,
        TransitionType::Tritone => -0.80,
    }
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
/// Blends harmonic compatibility with energy-direction alignment based on
/// the fader position. At center (bias=0), returns pure harmonic compatibility.
/// At extremes (|bias|=1), returns pure energy-direction alignment, making
/// energy-appropriate transitions (e.g. SemitoneUp when raising) outscore
/// harmonically safe ones (e.g. SameKey).
///
/// The `model` parameter selects whether harmonic scores come from the hand-tuned
/// Camelot categories or the Krumhansl correlation matrix. Energy direction
/// always uses Camelot-based transition classification.
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

    // Energy-direction score: how well the transition aligns with the fader.
    // 1.0 = perfectly aligned, 0.0 = fully opposing.
    let energy_dir = transition_energy_direction(tt);
    let energy_score = (energy_dir * energy_bias.signum() + 1.0) / 2.0;

    // Blend: linear interpolation from harmonic (center) to energy (extremes).
    let blend = energy_bias.abs();
    (base * (1.0 - blend) + energy_score * blend).clamp(0.0, 1.0)
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

/// Compute a key transition energy direction penalty (0.0 = perfect match, 1.0 = worst).
///
/// Provides an independent signal about whether the transition's emotional
/// energy direction aligns with the fader. At center (bias=0), returns 0.5
/// for all transitions (neutral — no direction preference).
///
/// This is used as its own term in the scoring formula (`w_key_dir`) so the
/// fader can independently steer results toward energy-raising or energy-lowering
/// key transitions.
fn key_direction_penalty(tt: TransitionType, energy_bias: f32) -> f32 {
    let energy_dir = transition_energy_direction(tt);
    // Alignment with fader direction: positive = matching, negative = opposing
    let alignment = energy_dir * energy_bias;
    // Map to 0-1 penalty: good alignment → 0, opposing → 1, neutral → 0.5
    0.5 - 0.5 * alignment.clamp(-1.0, 1.0)
}

// ─── ML Score Penalty Functions ──────────────────────────────────

/// Compute a directional energy penalty for a candidate value vs seed average.
///
/// Used for danceability and approachability. When raising energy (positive bias),
/// candidates with higher values than the seed average get lower penalties.
/// When dropping energy (negative bias), lower values are preferred.
///
/// Returns 0.0 (best alignment) to 1.0 (worst). At center (bias=0), returns 0.5.
/// Tracks without ML data should pass 0.5 as `cand_val` for neutral scoring.
fn direction_penalty(cand_val: f32, seed_avg: f32, energy_bias: f32) -> f32 {
    (0.5 - (cand_val - seed_avg) * energy_bias).clamp(0.0, 1.0)
}

/// Compute a tonal/timbre contrast penalty.
///
/// At energy extremes, DJs often want contrasting characteristics (e.g., follow
/// a dark/atonal track with a bright/tonal one for maximum impact). This penalty
/// rewards candidates whose timbre and tonal scores differ from the seed averages.
///
/// Returns 0.0 (maximum contrast, best) to 1.0 (identical characteristics, worst).
fn contrast_penalty(
    cand_timbre: f32,
    cand_tonal: f32,
    seed_timbre: f32,
    seed_tonal: f32,
) -> f32 {
    let timbre_contrast = (seed_timbre - cand_timbre).abs();
    let tonal_contrast = (seed_tonal - cand_tonal).abs();
    1.0 - (timbre_contrast + tonal_contrast) / 2.0
}

/// Compute acoustic/electronic production character match penalty.
///
/// Prefers candidates with similar production character to the seed average.
/// A fully acoustic seed gets low penalty for acoustic candidates, etc.
///
/// Returns 0.0 (identical character) to 1.0 (opposite character).
fn production_match_penalty(
    cand_acoustic: f32,
    cand_electronic: f32,
    seed_acoustic: f32,
    seed_electronic: f32,
) -> f32 {
    let diff = (cand_acoustic - seed_acoustic).abs()
             + (cand_electronic - seed_electronic).abs();
    (diff / 2.0).min(1.0)
}

/// Per-genre z-score normalization of aggression values.
///
/// Raw `mood_aggressive` scores are genre-biased — DnB tracks score higher
/// than house tracks regardless of relative intensity. This function groups
/// tracks by their primary genre and normalizes within each group, answering
/// "how aggressive is this track *for its genre*?"
///
/// Genres with fewer than 3 tracks use raw values (insufficient sample for stats).
/// The z-score is mapped to 0–1 via linear transform: `(z/3 + 0.5).clamp(0, 1)`.
fn normalize_aggression_by_genre<K: Eq + std::hash::Hash + Copy>(
    ml_scores: &HashMap<K, MlScores>,
) -> HashMap<K, f32> {
    let mut genre_values: HashMap<&str, Vec<(K, f32)>> = HashMap::new();
    for (&tid, scores) in ml_scores {
        let aggression = match scores.aggression {
            Some(a) => a,
            None => continue, // skip tracks without aggression data
        };
        let genre = scores.top_genre.as_deref().unwrap_or("Unknown");
        genre_values.entry(genre).or_default().push((tid, aggression));
    }

    let mut normalized = HashMap::new();
    for (_genre, tracks) in &genre_values {
        if tracks.len() < 3 {
            // Too few tracks in genre — use raw value (no normalization)
            for &(tid, raw) in tracks {
                normalized.insert(tid, raw);
            }
            continue;
        }
        let mean = tracks.iter().map(|t| t.1).sum::<f32>() / tracks.len() as f32;
        let variance = tracks.iter().map(|t| (t.1 - mean).powi(2)).sum::<f32>()
            / tracks.len() as f32;
        let std = variance.sqrt().max(0.01); // avoid division by zero
        for &(tid, raw) in tracks {
            let z = (raw - mean) / std;
            let norm = (z / 3.0 + 0.5).clamp(0.0, 1.0); // ±1.5 std → 0..1
            normalized.insert(tid, norm);
        }
    }
    normalized
}

/// Human-readable label for a transition type
fn transition_type_label(tt: TransitionType) -> &'static str {
    match tt {
        TransitionType::SameKey => "Same Key",
        TransitionType::AdjacentUp | TransitionType::AdjacentDown => "Adjacent",
        TransitionType::DiagonalUp | TransitionType::DiagonalDown => "Diagonal",
        TransitionType::EnergyBoost => "Boost",
        TransitionType::EnergyCool => "Cool",
        TransitionType::MoodLift => "Mood Lift",
        TransitionType::MoodDarken => "Darken",
        TransitionType::SemitoneUp | TransitionType::SemitoneDown => "Semitone",
        TransitionType::FarStep(_) => "Far",
        TransitionType::FarCross(_) => "Cross",
        TransitionType::Tritone => "Tritone",
    }
}

/// Color a tag by penalty quality: low penalty = green (good), high = red (bad).
fn penalty_color(penalty: f32) -> &'static str {
    if penalty <= 0.3 { "#2d8a4e" }       // green = strong match
    else if penalty <= 0.6 { "#c49a2a" }   // amber = moderate
    else { "#a63d40" }                      // red = weak/opposing
}

/// Generate human-readable reason tags from the full scoring breakdown.
///
/// Produces tags for all significant scoring factors, sorted by relevance.
/// The key-relationship tag always leads, followed by other factors ordered
/// by their impact on the final score (weight × deviation from neutral).
///
/// Directional arrows on key/dance/approach tags indicate transition direction:
/// - **▲** = raises energy/tension
/// - **▼** = lowers energy/tension
/// - **━** = neutral/same
#[allow(clippy::too_many_arguments)]
fn generate_reason_tags(
    transition_type: TransitionType,
    key_score: f32,
    hnsw_dist: f32,
    bpm_penalty: f32,
    dance_penalty: f32,
    approach_penalty: f32,
    contrast_pen: f32,
    aggression_pen: f32,
    w_hnsw: f32,
    w_bpm: f32,
    w_dance: f32,
    w_approach: f32,
    w_contrast: f32,
    w_aggression: f32,
) -> Vec<(String, Option<String>)> {
    let mut tags: Vec<(String, Option<String>, f32)> = Vec::with_capacity(8);

    // --- Key tag (always included, always first) ---
    let key_dir = match transition_type {
        TransitionType::SameKey => "\u{2501}",  // ━
        TransitionType::AdjacentUp | TransitionType::EnergyBoost
        | TransitionType::MoodLift | TransitionType::DiagonalUp
        | TransitionType::SemitoneUp => "\u{25B2}",  // ▲
        TransitionType::AdjacentDown | TransitionType::EnergyCool
        | TransitionType::MoodDarken | TransitionType::DiagonalDown
        | TransitionType::SemitoneDown => "\u{25BC}",  // ▼
        TransitionType::Tritone => "\u{25BC}",
        TransitionType::FarStep(s) => if s > 0 { "\u{25B2}" } else { "\u{25BC}" },
        TransitionType::FarCross(s) => if s > 0 { "\u{25B2}" } else { "\u{25BC}" },
    };
    let key_label = transition_type_label(transition_type);
    let key_color = if key_score >= 0.7 { "#2d8a4e" }
                    else if key_score >= 0.4 { "#c49a2a" }
                    else { "#a63d40" };
    // Key gets f32::MAX relevance so it sorts first
    tags.push((format!("{} {}", key_dir, key_label), Some(key_color.to_string()), f32::MAX));

    // --- Other factor tags (included when weight is significant) ---
    // Impact = weight × |penalty - 0.5| — measures how much this factor
    // pushed the score away from a neutral contribution.
    let min_weight = 0.03;

    if w_hnsw >= min_weight {
        let impact = w_hnsw * (hnsw_dist - 0.5).abs();
        tags.push(("Similar".to_string(), Some(penalty_color(hnsw_dist).to_string()), impact));
    }

    if w_bpm >= min_weight {
        let impact = w_bpm * (bpm_penalty - 0.5).abs();
        tags.push(("BPM".to_string(), Some(penalty_color(bpm_penalty).to_string()), impact));
    }

    if w_dance >= min_weight {
        let arrow = if dance_penalty < 0.4 { "▲" } else if dance_penalty > 0.6 { "▼" } else { "━" };
        let impact = w_dance * (dance_penalty - 0.5).abs();
        tags.push((format!("{} Dance", arrow), Some(penalty_color(dance_penalty).to_string()), impact));
    }

    if w_approach >= min_weight {
        let arrow = if approach_penalty < 0.4 { "▲" } else if approach_penalty > 0.6 { "▼" } else { "━" };
        let impact = w_approach * (approach_penalty - 0.5).abs();
        tags.push((format!("{} Reach", arrow), Some(penalty_color(approach_penalty).to_string()), impact));
    }

    if w_contrast >= min_weight {
        let impact = w_contrast * (contrast_pen - 0.5).abs();
        tags.push(("Timbre".to_string(), Some(penalty_color(contrast_pen).to_string()), impact));
    }

    if w_aggression >= min_weight {
        let arrow = if aggression_pen < 0.4 { "▲" } else if aggression_pen > 0.6 { "▼" } else { "━" };
        let impact = w_aggression * (aggression_pen - 0.5).abs();
        tags.push((format!("{} Aggr", arrow), Some(penalty_color(aggression_pen).to_string()), impact));
    }

    // Sort non-key tags by impact descending (key stays first via f32::MAX)
    tags.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    // Strip the impact score from the output
    tags.into_iter().map(|(label, color, _)| (label, color)).collect()
}

/// Query the database for track suggestions based on loaded deck seeds.
///
/// This runs on a background thread via `Task::perform()`.
///
/// # Algorithm
/// 1. Resolve each seed path to a track ID
/// 2. For each seed, find similar tracks via HNSW index
/// 3. Merge results keeping the best (minimum) distance per candidate
/// 4. Score using unified formula: hnsw + key_transition + key_dir + bpm
/// 5. Filter by adaptive harmonic threshold
/// 6. Sort and return the top results
pub fn query_suggestions(
    sources: &[DbSource],
    seed_paths: Vec<String>,
    energy_direction: f32,
    key_scoring_model: KeyScoringModel,
    per_seed_limit: usize,
    total_limit: usize,
    played_paths: &HashSet<String>,
    // Tracks whose absolute path is in this set are treated as "preferred" —
    // they receive a 50% more lenient key filter threshold so that user-curated
    // playlist tracks appear even when their key relationship is less ideal.
    preferred_paths: Option<&HashSet<String>>,
) -> Result<Vec<SuggestedTrack>, String> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }

    // Diagnostic: log audio features count per source
    for (idx, source) in sources.iter().enumerate() {
        let features = source.db.count_audio_features().unwrap_or(0);
        log::debug!("[SUGGESTIONS] Source {} ({}): audio_features={}", idx, source.name, features);
    }

    // Step 1: Resolve seed paths to tracks across all database sources.
    // For each seed path, try each source — first with the absolute path
    // (local DB stores full paths), then with the path relative to collection_root
    // (USB DBs store portable relative paths).
    let mut seed_tracks: Vec<(usize, Track)> = Vec::new(); // (source_index, track)
    for path in &seed_paths {
        let path_buf = PathBuf::from(path);
        let mut found = false;
        for (src_idx, source) in sources.iter().enumerate() {
            // Try absolute path (local DB stores full paths)
            if let Ok(Some(track)) = source.db.get_track_by_path(path) {
                if track.id.is_some() {
                    seed_tracks.push((src_idx, track));
                    found = true;
                    break;
                }
            }
            // Try relative path (USB DB stores paths relative to collection_root)
            if let Ok(rel) = path_buf.strip_prefix(&source.collection_root) {
                let rel_str = rel.to_string_lossy();
                if let Ok(Some(track)) = source.db.get_track_by_path(&rel_str) {
                    if track.id.is_some() {
                        seed_tracks.push((src_idx, track));
                        found = true;
                        break;
                    }
                }
            }
        }
        if !found {
            log::debug!("Suggestion seed not found in any database: {}", path);
        }
    }

    log::debug!(
        "[SUGGESTIONS] Resolved {}/{} seeds across {} sources",
        seed_tracks.len(), seed_paths.len(), sources.len()
    );
    for (src_idx, track) in &seed_tracks {
        log::debug!(
            "[SUGGESTIONS]   seed: src={} id={:?} path={:?}",
            sources[*src_idx].name, track.id, track.path
        );
    }

    if seed_tracks.is_empty() {
        log::debug!("[SUGGESTIONS] No seeds resolved — returning empty");
        return Ok(Vec::new());
    }

    // Seed filenames for cross-DB deduplication (same audio file may exist in multiple DBs)
    let seed_filenames: HashSet<String> = seed_tracks
        .iter()
        .filter_map(|(_, t)| t.path.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();

    // Step 2 & 3: Cross-database HNSW search.
    // For each seed, extract its feature vector and search ALL databases.
    // In the seed's own DB we use the efficient by-ID lookup; in other DBs
    // we pass the raw vector to their HNSW index.
    let mut candidates: HashMap<(usize, i64), (Track, f32)> = HashMap::new();

    for &(seed_src_idx, ref seed_track) in &seed_tracks {
        let seed_id = match seed_track.id {
            Some(id) => id,
            None => continue,
        };

        // Get the seed's feature vector for cross-DB search
        let seed_features = sources[seed_src_idx].db.get_audio_features(seed_id);
        let seed_vector = seed_features.ok().flatten().map(|f| f.to_vector());
        log::debug!(
            "[SUGGESTIONS] Seed {} (src={}) has_features={}",
            seed_id, sources[seed_src_idx].name, seed_vector.is_some()
        );

        for (target_idx, target_source) in sources.iter().enumerate() {
            let results = if target_idx == seed_src_idx {
                // Same DB — efficient by-ID lookup
                target_source.db.find_similar_tracks(seed_id, per_seed_limit)
            } else if let Some(ref vec) = seed_vector {
                // Different DB — cross-DB vector search
                target_source.db.find_similar_by_vector(vec, per_seed_limit)
            } else {
                continue; // No features available for this seed
            };

            match results {
                Ok(results) => {
                    log::debug!(
                        "[SUGGESTIONS] HNSW search: seed={} target={} returned {} results",
                        seed_id, target_source.name, results.len()
                    );
                    for (mut track, distance) in results {
                        if let Some(track_id) = track.id {
                            // Skip if this is a seed track in another DB (same filename)
                            if let Some(name) = track.path.file_name() {
                                if seed_filenames.contains(&*name.to_string_lossy()) {
                                    continue;
                                }
                            }
                            let key = (target_idx, track_id);
                            // Resolve relative paths to absolute for loading
                            if !track.path.is_absolute() {
                                track.path = target_source.collection_root.join(&track.path);
                            }
                            // Skip tracks already played this session
                            if played_paths.contains(&*track.path.to_string_lossy()) {
                                continue;
                            }
                            // Keep minimum distance per candidate
                            candidates
                                .entry(key)
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
                    log::warn!("Similarity query failed for seed {} in source {}: {}", seed_id, target_idx, e);
                }
            }
        }
    }

    // Cross-source dedup: same track may exist in both Local and USB DBs.
    // Group by filename, keep the entry with the lowest HNSW distance.
    if sources.len() > 1 {
        let mut best_by_filename: HashMap<String, (usize, i64, f32)> = HashMap::new();
        for (&(src_idx, track_id), (track, dist)) in &candidates {
            if let Some(name) = track.path.file_name() {
                let fname = name.to_string_lossy().to_string();
                best_by_filename
                    .entry(fname)
                    .and_modify(|existing| {
                        if *dist < existing.2 {
                            *existing = (src_idx, track_id, *dist);
                        }
                    })
                    .or_insert((src_idx, track_id, *dist));
            }
        }
        let keep_keys: HashSet<(usize, i64)> = best_by_filename
            .values()
            .map(|&(src, id, _)| (src, id))
            .collect();
        let before = candidates.len();
        candidates.retain(|key, _| keep_keys.contains(key));
        let deduped = before - candidates.len();
        if deduped > 0 {
            log::debug!("[SUGGESTIONS] Deduped {} cross-source duplicates", deduped);
        }
    }

    log::debug!("[SUGGESTIONS] Total candidates after HNSW: {}", candidates.len());

    if candidates.is_empty() {
        log::debug!("[SUGGESTIONS] No candidates — returning empty");
        return Ok(Vec::new());
    }

    // Step 4: Compute seed averages for scoring

    let avg_seed_bpm = {
        let bpm_values: Vec<f64> = seed_tracks.iter().filter_map(|(_, t)| t.bpm).collect();
        if bpm_values.is_empty() {
            128.0
        } else {
            bpm_values.iter().sum::<f64>() / bpm_values.len() as f64
        }
    };

    // Collect seed keys for harmonic scoring
    let seed_keys: Vec<MusicalKey> = seed_tracks
        .iter()
        .filter_map(|(_, t)| t.key.as_deref().and_then(MusicalKey::parse))
        .collect();

    // Energy direction bias: -1.0 (drop) through 0.0 (maintain) to +1.0 (peak)
    let energy_bias = (energy_direction - 0.5) * 2.0;
    let filter_threshold = adaptive_filter_threshold(energy_bias);

    // Step 4b: Batch-fetch ML scores from each source DB, merged under composite keys
    let ml_scores: HashMap<(usize, i64), MlScores> = {
        // Group IDs by source index
        let mut ids_by_source: HashMap<usize, Vec<i64>> = HashMap::new();
        for &(src_idx, track_id) in candidates.keys() {
            ids_by_source.entry(src_idx).or_default().push(track_id);
        }
        for &(src_idx, ref track) in &seed_tracks {
            if let Some(id) = track.id {
                ids_by_source.entry(src_idx).or_default().push(id);
            }
        }
        // Fetch from each source and merge under (source_index, track_id) keys
        let mut merged = HashMap::new();
        for (src_idx, ids) in &ids_by_source {
            if let Ok(scores) = sources[*src_idx].db.get_ml_scores_batch(ids) {
                for (id, score) in scores {
                    merged.insert((*src_idx, id), score);
                }
            }
        }
        merged
    };

    // Compute seed ML averages (fallback 0.5 when no data)
    let seed_ml: Vec<&MlScores> = seed_tracks
        .iter()
        .filter_map(|(idx, t)| t.id.map(|id| (*idx, id)))
        .filter_map(|key| ml_scores.get(&key))
        .collect();
    let avg_ml = |f: fn(&MlScores) -> Option<f32>| -> f32 {
        let vals: Vec<f32> = seed_ml.iter().filter_map(|s| f(s)).collect();
        if vals.is_empty() { 0.5 } else { vals.iter().sum::<f32>() / vals.len() as f32 }
    };
    let avg_seed_dance = avg_ml(|s| s.danceability);
    let avg_seed_approach = avg_ml(|s| s.approachability);
    let avg_seed_timbre = avg_ml(|s| s.timbre);
    let avg_seed_tonal = avg_ml(|s| s.tonal);
    let avg_seed_acoustic = avg_ml(|s| s.mood_acoustic);
    let avg_seed_electronic = avg_ml(|s| s.mood_electronic);

    // Step 4c: Genre-normalize aggression across the candidate pool
    let norm_aggression = normalize_aggression_by_genre(&ml_scores);
    let avg_seed_aggression = {
        let vals: Vec<f32> = seed_tracks
            .iter()
            .filter_map(|(idx, t)| t.id.map(|id| (*idx, id)))
            .filter_map(|key| norm_aggression.get(&key).copied())
            .collect();
        if vals.is_empty() { 0.5 } else { vals.iter().sum::<f32>() / vals.len() as f32 }
    };

    // Step 5: Unified scoring — single formula for all candidates
    //
    // Dynamic weights: at center, scoring emphasizes similarity and harmony.
    // As the fader moves to extremes, HNSW drops to zero and energy-direction
    // signals (aggression, danceability, key direction) emerge as dominant.
    //
    // Center (bias=0): 0.42 hnsw + 0.25 key + 0.15 key_dir + 0.15 bpm + 0.03 prod = 1.00
    // Extreme (|bias|=1): 0.15 key + 0.22 key_dir + 0.10 bpm + 0.03 prod
    //                    + 0.10 dance + 0.06 approach + 0.04 contrast + 0.30 aggr = 1.00
    let bias_abs = energy_bias.abs();
    let w_hnsw       = 0.42 - 0.42 * bias_abs;  // 0.42 → 0.00
    let w_key        = 0.25 - 0.10 * bias_abs;  // 0.25 → 0.15
    let w_key_dir    = 0.15 + 0.07 * bias_abs;  // 0.15 → 0.22
    let w_bpm        = 0.15 - 0.05 * bias_abs;  // 0.15 → 0.10
    let w_production = 0.03;                      // constant — subtle tiebreaker
    let w_dance      = 0.10 * bias_abs;          // 0.00 → 0.10
    let w_approach   = 0.06 * bias_abs;          // 0.00 → 0.06
    let w_contrast   = 0.04 * bias_abs;          // 0.00 → 0.04
    let w_aggression = 0.30 * bias_abs;          // 0.00 → 0.30

    // Collect source names per candidate for source-tag generation later
    let source_names: HashMap<usize, &str> = sources.iter().enumerate()
        .map(|(i, s)| (i, s.name.as_str()))
        .collect();
    let multi_source = {
        let active_sources: HashSet<usize> = candidates.keys().map(|(idx, _)| *idx).collect();
        active_sources.len() > 1
    };

    let mut suggestions: Vec<SuggestedTrack> = candidates
        .into_iter()
        .filter_map(|((src_idx, _track_id), (track, hnsw_dist))| {
            // Key transition score: best match across all seeds
            // Also capture transition type for reason tag generation
            let (best_key_score, best_tt) = track
                .key
                .as_deref()
                .and_then(|k| MusicalKey::parse(k))
                .map(|ck| {
                    seed_keys
                        .iter()
                        .map(|sk| {
                            let tt = classify_transition(sk, &ck);
                            let score = key_transition_score(sk, &ck, energy_bias, key_scoring_model);
                            (score, tt)
                        })
                        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                        .unwrap_or((0.3, TransitionType::FarStep(6)))
                })
                .unwrap_or((0.3, TransitionType::FarStep(6))); // No key = moderate penalty

            // Apply adaptive filter threshold.
            // Tracks in the user's preferred set (e.g. currently browsed playlist)
            // use a 50% more lenient threshold — personal curation implies trust.
            let is_preferred = preferred_paths.map_or(false, |pp| {
                let p = track.path.to_string_lossy();
                pp.contains(p.as_ref())
            });
            let effective_threshold = if is_preferred {
                filter_threshold * 0.5
            } else {
                filter_threshold
            };
            if best_key_score < effective_threshold {
                return None;
            }

            let key_penalty = 1.0 - best_key_score;

            // Key energy direction penalty: does the transition's emotional
            // energy direction match the fader? 0.0 = perfect alignment, 1.0 = opposing.
            // At center (bias=0) all transitions get 0.5 (neutral).
            let key_dir_penalty = key_direction_penalty(best_tt, energy_bias);

            // BPM penalty: normalized distance (10 BPM diff → 1.0 penalty)
            let bpm_penalty = track
                .bpm
                .map(|b| {
                    let diff = (b - avg_seed_bpm).abs();
                    (diff / 10.0).min(1.0) as f32
                })
                .unwrap_or(0.5);

            // ML score penalties (fallback 0.5 = neutral when no data)
            let ml_key = track.id.map(|id| (src_idx, id));
            let cand_ml = ml_key.and_then(|k| ml_scores.get(&k));
            let cand_dance = cand_ml.and_then(|s| s.danceability).unwrap_or(0.5);
            let cand_approach = cand_ml.and_then(|s| s.approachability).unwrap_or(0.5);
            let cand_timbre = cand_ml.and_then(|s| s.timbre).unwrap_or(0.5);
            let cand_tonal = cand_ml.and_then(|s| s.tonal).unwrap_or(0.5);
            let cand_acoustic = cand_ml.and_then(|s| s.mood_acoustic).unwrap_or(0.5);
            let cand_electronic = cand_ml.and_then(|s| s.mood_electronic).unwrap_or(0.5);

            let dance_penalty = direction_penalty(cand_dance, avg_seed_dance, energy_bias);
            let approach_penalty = direction_penalty(cand_approach, avg_seed_approach, energy_bias);
            let contrast_pen = contrast_penalty(cand_timbre, cand_tonal, avg_seed_timbre, avg_seed_tonal);
            let production_pen = production_match_penalty(
                cand_acoustic, cand_electronic, avg_seed_acoustic, avg_seed_electronic,
            );

            // Genre-normalized aggression (fallback 0.5 when no data)
            let cand_norm_aggr = ml_key
                .and_then(|k| norm_aggression.get(&k).copied())
                .unwrap_or(0.5);
            let aggression_pen = direction_penalty(cand_norm_aggr, avg_seed_aggression, energy_bias);

            let score = w_hnsw       * hnsw_dist
                + w_key        * key_penalty
                + w_key_dir    * key_dir_penalty
                + w_bpm        * bpm_penalty
                + w_production * production_pen
                + w_dance      * dance_penalty
                + w_approach   * approach_penalty
                + w_contrast   * contrast_pen
                + w_aggression * aggression_pen;

            let mut reason_tags = generate_reason_tags(
                best_tt, best_key_score,
                hnsw_dist, bpm_penalty,
                dance_penalty, approach_penalty, contrast_pen,
                aggression_pen,
                w_hnsw, w_bpm, w_dance, w_approach, w_contrast,
                w_aggression,
            );

            // Prepend source library tag when results span multiple databases
            if multi_source {
                let source_name = source_names.get(&src_idx).copied().unwrap_or("?");
                reason_tags.insert(0, (source_name.to_string(), Some("#808080".to_string())));
            }

            Some(SuggestedTrack { track, score, reason_tags, playlists: Vec::new() })
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
        assert!(drop_score >= 0.10, "Tritone at drop should be viable: {}", drop_score);
    }

    #[test]
    fn test_key_score_extreme_prefers_energy_over_harmony() {
        let am = MusicalKey::parse("Am").unwrap();
        let em = MusicalKey::parse("Em").unwrap();  // AdjacentUp (+1)
        let bm = MusicalKey::parse("Bm").unwrap();  // EnergyBoost (+2)
        let bbm = MusicalKey::parse("Bbm").unwrap(); // SemitoneUp (+7)

        // At full raise, energy-aligned transitions should outscore SameKey
        let same = key_transition_score(&am, &am, 1.0, CAM);
        let adj_up = key_transition_score(&am, &em, 1.0, CAM);
        let boost = key_transition_score(&am, &bm, 1.0, CAM);
        let semi_up = key_transition_score(&am, &bbm, 1.0, CAM);

        assert!(semi_up > same, "SemitoneUp should outscore SameKey at +1.0: {} vs {}", semi_up, same);
        assert!(boost > same, "EnergyBoost should outscore SameKey at +1.0: {} vs {}", boost, same);
        assert!(adj_up > same, "AdjacentUp should outscore SameKey at +1.0: {} vs {}", adj_up, same);
        // Ordering: strongest energy lift should score highest
        assert!(semi_up > boost, "SemitoneUp > EnergyBoost: {} vs {}", semi_up, boost);
        assert!(boost > adj_up, "EnergyBoost > AdjacentUp: {} vs {}", boost, adj_up);
    }

    #[test]
    fn test_key_score_extreme_drop_prefers_energy_lowering() {
        let am = MusicalKey::parse("Am").unwrap();
        let gm = MusicalKey::parse("Gm").unwrap();  // EnergyCool (-2)
        let dm = MusicalKey::parse("Dm").unwrap();  // AdjacentDown (-1)

        // At full drop, energy-lowering transitions should outscore SameKey
        let same = key_transition_score(&am, &am, -1.0, CAM);
        let cool = key_transition_score(&am, &gm, -1.0, CAM);
        let adj_down = key_transition_score(&am, &dm, -1.0, CAM);

        assert!(cool > same, "EnergyCool should outscore SameKey at -1.0: {} vs {}", cool, same);
        assert!(adj_down > same, "AdjacentDown should outscore SameKey at -1.0: {} vs {}", adj_down, same);
    }

    #[test]
    fn test_key_score_mood_lift_direction_matters() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let c = MusicalKey::parse("C").unwrap();   // 8B
        // MoodLift is a gentle energy lift — at moderate fader, raising should
        // score higher than dropping (direction matters)
        let raise = key_transition_score(&am, &c, 0.5, CAM);
        let drop = key_transition_score(&am, &c, -0.5, CAM);
        assert!(raise > drop, "Mood lift should score better when raising than dropping: {} vs {}", raise, drop);
        // At extreme, more dramatic transitions dominate, but mood lift still viable
        let extreme = key_transition_score(&am, &c, 1.0, CAM);
        assert!(extreme > 0.50, "Mood lift should still be viable at extreme: {}", extreme);
    }

    #[test]
    fn test_key_score_mood_darken_direction_matters() {
        let c = MusicalKey::parse("C").unwrap();   // 8B
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let drop = key_transition_score(&c, &am, -0.5, CAM);
        let raise = key_transition_score(&c, &am, 0.5, CAM);
        assert!(drop > raise, "Mood darken should score better when dropping than raising: {} vs {}", drop, raise);
        let extreme = key_transition_score(&c, &am, -1.0, CAM);
        assert!(extreme > 0.50, "Mood darken should still be viable at extreme: {}", extreme);
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

    // ─── Key Direction Penalty ──────────────────────────────────────

    #[test]
    fn test_key_dir_center_is_neutral() {
        // At center, all transitions get 0.5 regardless of direction
        assert_eq!(key_direction_penalty(TransitionType::SemitoneUp, 0.0), 0.5);
        assert_eq!(key_direction_penalty(TransitionType::EnergyCool, 0.0), 0.5);
        assert_eq!(key_direction_penalty(TransitionType::SameKey, 0.0), 0.5);
    }

    #[test]
    fn test_key_dir_raise_prefers_energy_raising_transitions() {
        let semitone_up = key_direction_penalty(TransitionType::SemitoneUp, 1.0);
        let energy_boost = key_direction_penalty(TransitionType::EnergyBoost, 1.0);
        let same_key = key_direction_penalty(TransitionType::SameKey, 1.0);
        let energy_cool = key_direction_penalty(TransitionType::EnergyCool, 1.0);
        let semitone_down = key_direction_penalty(TransitionType::SemitoneDown, 1.0);

        // Energy-raising transitions should have low penalty when raising
        assert!(semitone_up < 0.5, "Semitone up should be below neutral when raising: {}", semitone_up);
        assert!(energy_boost < 0.5, "Energy boost should be below neutral when raising: {}", energy_boost);
        // Same key is neutral
        assert_eq!(same_key, 0.5);
        // Energy-lowering transitions should have high penalty when raising
        assert!(energy_cool > 0.5, "Energy cool should be above neutral when raising: {}", energy_cool);
        assert!(semitone_down > 0.5, "Semitone down should be above neutral when raising: {}", semitone_down);
        // Ordering: semitone up best, then energy boost
        assert!(semitone_up < energy_boost, "Semitone up should be preferred over boost: {} vs {}", semitone_up, energy_boost);
    }

    #[test]
    fn test_key_dir_drop_prefers_energy_lowering_transitions() {
        let energy_cool = key_direction_penalty(TransitionType::EnergyCool, -1.0);
        let mood_darken = key_direction_penalty(TransitionType::MoodDarken, -1.0);
        let same_key = key_direction_penalty(TransitionType::SameKey, -1.0);
        let mood_lift = key_direction_penalty(TransitionType::MoodLift, -1.0);
        let energy_boost = key_direction_penalty(TransitionType::EnergyBoost, -1.0);

        // Energy-lowering transitions should have low penalty when dropping
        assert!(energy_cool < 0.5, "Energy cool should be below neutral when dropping: {}", energy_cool);
        assert!(mood_darken < 0.5, "Mood darken should be below neutral when dropping: {}", mood_darken);
        // Energy-raising transitions should have high penalty when dropping
        assert!(mood_lift > 0.5, "Mood lift should be above neutral when dropping: {}", mood_lift);
        assert!(energy_boost > 0.5, "Energy boost should be above neutral when dropping: {}", energy_boost);
        assert_eq!(same_key, 0.5);
    }

    #[test]
    fn test_key_dir_scales_with_fader() {
        // At half fader, penalty should be between center (0.5) and extreme
        let full = key_direction_penalty(TransitionType::SemitoneUp, 1.0);
        let half = key_direction_penalty(TransitionType::SemitoneUp, 0.5);
        assert!(half < 0.5, "Should still be below neutral at half fader");
        assert!(half > full, "Effect should be stronger at full fader: half={} full={}", half, full);
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

    // ─── Direction Penalty ─────────────────────────────────────────

    #[test]
    fn test_direction_penalty_center_is_neutral() {
        // At center (bias=0), penalty is always 0.5 regardless of values
        assert_eq!(direction_penalty(0.8, 0.5, 0.0), 0.5);
        assert_eq!(direction_penalty(0.2, 0.5, 0.0), 0.5);
        assert_eq!(direction_penalty(0.5, 0.5, 0.0), 0.5);
    }

    #[test]
    fn test_direction_penalty_raise_prefers_higher() {
        // When raising energy (bias=1.0), higher candidate → lower penalty
        let high = direction_penalty(0.8, 0.5, 1.0);
        let same = direction_penalty(0.5, 0.5, 1.0);
        let low = direction_penalty(0.2, 0.5, 1.0);
        assert!(high < same, "Higher value should have lower penalty when raising: {} vs {}", high, same);
        assert!(same < low, "Same value should beat lower when raising: {} vs {}", same, low);
    }

    #[test]
    fn test_direction_penalty_drop_prefers_lower() {
        // When dropping energy (bias=-1.0), lower candidate → lower penalty
        let high = direction_penalty(0.8, 0.5, -1.0);
        let same = direction_penalty(0.5, 0.5, -1.0);
        let low = direction_penalty(0.2, 0.5, -1.0);
        assert!(low < same, "Lower value should have lower penalty when dropping: {} vs {}", low, same);
        assert!(same < high, "Same value should beat higher when dropping: {} vs {}", same, high);
    }

    #[test]
    fn test_direction_penalty_clamped() {
        // Extreme differences should clamp to 0.0 and 1.0
        let best = direction_penalty(1.0, 0.0, 1.0);
        let worst = direction_penalty(0.0, 1.0, 1.0);
        assert!(best <= 0.01, "Maximum alignment should be near 0: {}", best);
        assert!(worst >= 0.99, "Maximum opposition should be near 1: {}", worst);
    }

    #[test]
    fn test_direction_penalty_scales_with_bias() {
        // At half bias, penalty should be between center (0.5) and extreme
        let full = direction_penalty(0.8, 0.5, 1.0);
        let half = direction_penalty(0.8, 0.5, 0.5);
        let center = direction_penalty(0.8, 0.5, 0.0);
        assert!(full < half, "Full bias should give stronger signal: {} vs {}", full, half);
        assert!(half < center, "Half bias should be between center and full: {} vs {}", half, center);
    }

    // ─── Contrast Penalty ──────────────────────────────────────────

    #[test]
    fn test_contrast_penalty_identical_is_worst() {
        // Identical characteristics → 1.0 (worst, no contrast)
        assert_eq!(contrast_penalty(0.8, 0.7, 0.8, 0.7), 1.0);
    }

    #[test]
    fn test_contrast_penalty_opposite_is_best() {
        // Maximum contrast: dark+atonal seed vs bright+tonal candidate
        let p = contrast_penalty(1.0, 1.0, 0.0, 0.0);
        assert!(p < 0.01, "Maximum contrast should be near 0: {}", p);
    }

    #[test]
    fn test_contrast_penalty_partial() {
        // Only timbre differs, tonal same → partial contrast
        let p = contrast_penalty(1.0, 0.5, 0.0, 0.5);
        assert!(p > 0.0 && p < 1.0, "Partial contrast should be between 0 and 1: {}", p);
        // timbre contrast = 1.0, tonal contrast = 0.0, avg = 0.5 → penalty = 0.5
        assert!((p - 0.5).abs() < 0.01, "Should be 0.5: {}", p);
    }

    #[test]
    fn test_contrast_penalty_symmetric() {
        // Swapping candidate and seed should give same result
        let a = contrast_penalty(0.8, 0.3, 0.2, 0.7);
        let b = contrast_penalty(0.2, 0.7, 0.8, 0.3);
        assert!((a - b).abs() < 0.001, "Should be symmetric: {} vs {}", a, b);
    }

    // ─── Production Match Penalty ────────────────────────────────────

    #[test]
    fn test_production_match_identical() {
        let p = production_match_penalty(0.8, 0.9, 0.8, 0.9);
        assert!(p < 0.01, "Identical production should be near 0: {}", p);
    }

    #[test]
    fn test_production_match_opposite() {
        // Fully acoustic seed vs fully electronic candidate
        let p = production_match_penalty(0.0, 1.0, 1.0, 0.0);
        assert!((p - 1.0).abs() < 0.01, "Opposite production should be 1.0: {}", p);
    }

    #[test]
    fn test_production_match_partial() {
        let p = production_match_penalty(0.6, 0.4, 0.4, 0.6);
        // diff = |0.6-0.4| + |0.4-0.6| = 0.2 + 0.2 = 0.4, /2 = 0.2
        assert!((p - 0.2).abs() < 0.01, "Partial diff should be 0.2: {}", p);
    }

    // ─── Genre-Normalized Aggression ─────────────────────────────────

    #[test]
    fn test_normalize_aggression_single_genre() {
        let mut ml = HashMap::new();
        // 5 tracks in "House" with different raw aggression
        for (i, aggr) in [0.1, 0.15, 0.2, 0.25, 0.3].iter().enumerate() {
            ml.insert(i as i64, MlScores {
                aggression: Some(*aggr),
                top_genre: Some("House".to_string()),
                ..Default::default()
            });
        }
        let norm = normalize_aggression_by_genre(&ml);
        // The highest raw (0.3) should get the highest normalized value
        assert!(norm[&4] > norm[&2], "Highest raw should normalize highest: {} vs {}", norm[&4], norm[&2]);
        assert!(norm[&2] > norm[&0], "Middle should be between: {} vs {}", norm[&2], norm[&0]);
    }

    #[test]
    fn test_normalize_aggression_cross_genre_equity() {
        let mut ml = HashMap::new();
        // House tracks: low raw aggression (0.1 to 0.2)
        for (i, aggr) in [0.10, 0.12, 0.14, 0.16, 0.20].iter().enumerate() {
            ml.insert(i as i64, MlScores {
                aggression: Some(*aggr),
                top_genre: Some("House".to_string()),
                ..Default::default()
            });
        }
        // DnB tracks: high raw aggression (0.5 to 0.7)
        for (i, aggr) in [0.50, 0.55, 0.60, 0.65, 0.70].iter().enumerate() {
            ml.insert((i + 5) as i64, MlScores {
                aggression: Some(*aggr),
                top_genre: Some("Drum and Bass".to_string()),
                ..Default::default()
            });
        }
        let norm = normalize_aggression_by_genre(&ml);
        // The most aggressive house track (0.20) should have a similar normalized
        // score to the most aggressive DnB track (0.70), since both are at the
        // top of their genre
        let house_top = norm[&4]; // raw 0.20, top of House
        let dnb_top = norm[&9];   // raw 0.70, top of DnB
        assert!(
            (house_top - dnb_top).abs() < 0.15,
            "Genre-top tracks should have similar normalized scores: House={} DnB={}",
            house_top, dnb_top
        );
    }

    #[test]
    fn test_normalize_aggression_small_genre_uses_raw() {
        let mut ml = HashMap::new();
        // Only 2 tracks in a genre — should use raw values
        ml.insert(1, MlScores {
            aggression: Some(0.3),
            top_genre: Some("Ambient".to_string()),
            ..Default::default()
        });
        ml.insert(2, MlScores {
            aggression: Some(0.7),
            top_genre: Some("Ambient".to_string()),
            ..Default::default()
        });
        let norm = normalize_aggression_by_genre(&ml);
        assert!((norm[&1] - 0.3).abs() < 0.001, "Small genre should use raw: {}", norm[&1]);
        assert!((norm[&2] - 0.7).abs() < 0.001, "Small genre should use raw: {}", norm[&2]);
    }

    #[test]
    fn test_normalize_aggression_no_data_excluded() {
        let mut ml = HashMap::new();
        ml.insert(1, MlScores {
            aggression: None, // no aggression data
            top_genre: Some("House".to_string()),
            ..Default::default()
        });
        let norm = normalize_aggression_by_genre(&ml);
        assert!(norm.is_empty(), "Tracks without aggression should be excluded");
    }

    // ─── Weight Sum Verification ─────────────────────────────────────

    #[test]
    fn test_weights_sum_to_one_at_center() {
        let bias_abs: f32 = 0.0;
        let sum = (0.42 - 0.42 * bias_abs)  // hnsw
                + (0.25 - 0.10 * bias_abs)   // key
                + (0.15 + 0.07 * bias_abs)   // key_dir
                + (0.15 - 0.05 * bias_abs)   // bpm
                + 0.03                         // production
                + 0.10 * bias_abs            // dance
                + 0.06 * bias_abs            // approach
                + 0.04 * bias_abs            // contrast
                + 0.30 * bias_abs;           // aggression
        assert!((sum - 1.0).abs() < 0.001, "Center weights should sum to 1.0: {sum}");
    }

    #[test]
    fn test_weights_sum_to_one_at_extreme() {
        let bias_abs: f32 = 1.0;
        let sum = (0.42 - 0.42 * bias_abs)  // hnsw
                + (0.25 - 0.10 * bias_abs)   // key
                + (0.15 + 0.07 * bias_abs)   // key_dir
                + (0.15 - 0.05 * bias_abs)   // bpm
                + 0.03                         // production
                + 0.10 * bias_abs            // dance
                + 0.06 * bias_abs            // approach
                + 0.04 * bias_abs            // contrast
                + 0.30 * bias_abs;           // aggression
        assert!((sum - 1.0).abs() < 0.001, "Extreme weights should sum to 1.0: {sum}");
    }

    #[test]
    fn test_weights_sum_to_one_at_half() {
        let bias_abs: f32 = 0.5;
        let sum = (0.42 - 0.42 * bias_abs)
                + (0.25 - 0.10 * bias_abs)
                + (0.15 + 0.07 * bias_abs)
                + (0.15 - 0.05 * bias_abs)
                + 0.03
                + 0.10 * bias_abs
                + 0.06 * bias_abs
                + 0.04 * bias_abs
                + 0.30 * bias_abs;
        assert!((sum - 1.0).abs() < 0.001, "Half weights should sum to 1.0: {sum}");
    }
}
