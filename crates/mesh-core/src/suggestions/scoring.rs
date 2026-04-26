//! Pure scoring functions for the smart suggestion engine.
//!
//! All functions in this module are stateless — they take musical properties
//! as input and return scores. No database or IO access.

use std::sync::LazyLock;
use crate::music::MusicalKey;
use super::config::KeyScoringModel;

// ─── Transition Classification ───────────���──────────────────────────

/// Classification of the musical relationship between two keys.
///
/// Every pair of keys on the Camelot wheel maps to exactly one transition type.
/// This drives both the base compatibility score and the energy-direction modifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransitionType {
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
pub fn classify_transition(seed: &MusicalKey, candidate: &MusicalKey) -> TransitionType {
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

// ─── Base Score ─────────���───────────────────────────────────────────

/// Compute the base compatibility score for a transition type (energy_bias = 0).
///
/// Returns 0.0 (worst) to 1.0 (best). Nothing returns exactly 0.0 — even the
/// tritone has a small score, allowing the adaptive filter to unlock it at extremes.
pub fn base_score(tt: TransitionType) -> f32 {
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

// ─── Energy Direction ───────���───────────────────────────────────────

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
pub fn transition_energy_direction(tt: TransitionType) -> f32 {
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
        // Tritone: maximum dissonance, but NOT directional — it's chaotic tension,
        // not an energy-lowering move. Neutral direction prevents it from being
        // rewarded at extreme slider positions.
        TransitionType::Tritone => 0.0,
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
pub fn krumhansl_base_score(seed: &MusicalKey, candidate: &MusicalKey) -> f32 {
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
pub fn key_transition_score(
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

    // Blend: interpolation from harmonic (center) to energy (extremes).
    // Harmonic floor of 25% ensures good harmonic quality always matters —
    // a tritone never outranks a compatible transition regardless of slider.
    let blend = energy_bias.abs();
    let harmonic_weight = (1.0 - blend).max(0.25);
    let energy_weight = 1.0 - harmonic_weight;
    (base * harmonic_weight + energy_score * energy_weight).clamp(0.0, 1.0)
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
pub fn key_direction_penalty(tt: TransitionType, energy_bias: f32) -> f32 {
    let energy_dir = transition_energy_direction(tt);
    // Alignment with fader direction: positive = matching, negative = opposing
    let alignment = energy_dir * energy_bias;
    // Map to 0-1 penalty: good alignment → 0, opposing → 1, neutral → 0.5
    0.5 - 0.5 * alignment.clamp(-1.0, 1.0)
}

// ─── Composite Intensity + Penalty Functions ─────��───────────────

/// Compute composite intensity from individual components (v2).
///
/// All components are raw [0, 1] values stored per-track in DB.
/// The composite is computed at query time so weights can be tuned without reanalysis.
pub fn composite_intensity_v2(ic: &crate::db::IntensityComponents) -> f32 {
    // Weights tuned against human-ranked DnB aggression judgments.
    // Grit/compression dominate (0.65); texture/dynamics are secondary.
    // Variance features (evar, cvar, fvar) set to 0.0 — they measure temporal
    // dynamics (jazzy variation), which inversely correlates with perceived
    // aggression in electronic music. Kept at 0.0 rather than removed so
    // they remain documented and can be re-enabled if rhythm features are added.
    (0.15 * ic.spectral_flux             // timbral chop — moderate aggression signal
    + 0.25 * ic.flatness                 // distortion/noise — primary aggression marker
    + 0.10 * ic.spectral_centroid        // brightness/harshness
    + 0.20 * ic.dissonance               // spectral roughness — strong aggression marker
    + 0.15 * ic.crest_factor             // compression = wall-of-sound = aggressive
    + 0.00 * ic.energy_variance          // DISABLED: dynamic range ≠ aggression
    + 0.05 * (1.0 - ic.harmonic_complexity)  // atonal noise content
    + 0.04 * ic.spectral_rolloff         // high-frequency energy
    + 0.04 * ic.centroid_variance        // filter sweeps — minor texture signal
    + 0.02 * ic.flux_variance)           // chop inconsistency — minor texture signal
    .clamp(0.0, 1.0)
}

/// Intensity reward [0, 1] that matches energy level at center and steers direction at extremes.
///
/// - **Center** (`bias=0`): rewards candidates at similar intensity to the seed.
///   Same intensity → 1.0, opposite intensity → 0.0.
/// - **High extreme** (`bias=+1`): rewards candidates MORE aggressive than the seed.
///   Much more aggressive → 1.0, much less → 0.0.
/// - **Low extreme** (`bias=-1`): rewards candidates LESS aggressive than the seed.
///   Much less aggressive → 1.0, much more → 0.0.
/// - **Intermediate**: smooth linear blend between match and direction behaviours.
/// `blend_crossover`: from `SuggestionBlendMode` — controls how far the slider
/// must move before switching from relative (layering) to absolute (transition).
/// Coupled with the vector similarity crossover for consistent behavior.
pub fn intensity_reward(cand_intensity: f32, seed_intensity: f32, energy_bias: f32, blend_crossover: f32) -> f32 {
    let blend_t = (energy_bias.abs() / blend_crossover).clamp(0.0, 1.0);
    // Match component (center): 1.0 when same intensity as seed, 0.0 when opposite.
    // Used for layering — you want tracks at the same energy level.
    let match_reward = 1.0 - (cand_intensity - seed_intensity).abs();
    // Absolute component (extremes): rewards ABSOLUTE intensity level, ignoring seed.
    //   Peak (bias>0): high intensity = high reward (aggressive tracks)
    //   Drop (bias<0): low intensity = high reward (calm tracks)
    // This avoids the "already at max intensity, nowhere to go up" problem.
    let abs_reward = if energy_bias >= 0.0 {
        cand_intensity       // peak: more intense = better
    } else {
        1.0 - cand_intensity // drop: less intense = better
    };
    match_reward * (1.0 - blend_t) + abs_reward * blend_t
}

/// Intensity penalty (legacy wrapper, used by generate_reason_tags).
pub fn intensity_penalty(cand_intensity: f32, seed_intensity: f32, energy_bias: f32) -> f32 {
    1.0 - intensity_reward(cand_intensity, seed_intensity, energy_bias, 0.6)
}

/// PCA aggression reward — directional scoring using the library's aggression axis.
///
/// Uses one-sided linear falloff: wrong direction from seed gets hard penalty,
/// right direction gets linear reward toward the target.
///
/// - `cand_aggr`: candidate's percentile-ranked aggression score [0, 1]
/// - `seed_aggr`: seed's percentile-ranked aggression score [0, 1]
/// - `energy_bias`: slider position, -1.0 (drop) to +1.0 (peak), 0.0 = center
/// - `intensity_reach`: acceptance width (Tight=0.15, Medium=0.30, Open=0.50)
///
/// At center (bias=0): returns 1.0 for all tracks (no aggression influence).
/// The aggression weight is linearly introduced by the caller based on |bias|.
pub fn aggression_reward(cand_aggr: f32, seed_aggr: f32, energy_bias: f32, intensity_reach: f32) -> f32 {
    const WIDTH: f32 = 0.20;   // constant tent half-width (ring thickness)
    const FLOOR: f32 = 0.25;   // soft floor for off-target tracks

    let bias_abs = energy_bias.abs().min(1.0);
    let reach = intensity_reach.max(0.05);

    // Ring radius: slider shifts target away from seed, capped by reach.
    // Center: target = seed (radius 0). Full extreme: target = seed ± reach.
    let target = (seed_aggr + energy_bias.signum() * reach * bias_abs).clamp(0.0, 1.0);

    let tent = (1.0 - (cand_aggr - target).abs() / WIDTH).max(0.0);
    FLOOR + (1.0 - FLOOR) * tent
}

/// PCA similarity reward — ring in percentile-rank distance space around the seed.
///
/// At center, the ring sits at a small fixed radius from the seed (similar but
/// not identical). As the slider moves toward peak or drop, the ring's radius
/// slides linearly toward `reach` (the configured transition target distance),
/// letting the user dial controlled dissimilarity. Width and floor are constant
/// so the shape doesn't change with the slider — only the centre slides.
///
/// - `hnsw_dist`: percentile-rank normalised PCA distance to seed [0, 1]
/// - `energy_bias`: slider position [-1, 1]; only `|bias|` matters here (distance is unsigned)
/// - `reach`: target ring radius at full slider (Tight=0.15, Medium=0.25, Open=0.40)
pub fn similarity_reward(hnsw_dist: f32, energy_bias: f32, reach: f32) -> f32 {
    const CENTER_RADIUS: f32 = 0.05; // small fixed radius at center
    const WIDTH: f32 = 0.15;         // constant ring thickness
    const FLOOR: f32 = 0.20;         // soft floor for off-target tracks

    let bias_abs = energy_bias.abs().min(1.0);
    let target = CENTER_RADIUS + (reach - CENTER_RADIUS) * bias_abs;
    let tent = (1.0 - (hnsw_dist - target).abs() / WIDTH).max(0.0);
    FLOOR + (1.0 - FLOOR) * tent
}

/// Per-component intensity reward: weighted Euclidean distance between ranked component vectors.
///
/// Rewards tracks that are similar on EVERY axis individually, not just on the blended composite.
/// A gritty+smooth track won't match a clean+choppy seed even if their composites are equal.
///
/// At center: small distance = high reward (similar character).
/// At peak: reward candidates whose ranked components are each HIGHER than seed.
/// At drop: reward candidates whose ranked components are each LOWER than seed.
/// Component weights for per-component intensity matching.
/// Same weights as composite_intensity_v2.
fn intensity_component_pairs(
    cand_ic: &crate::db::IntensityComponents,
    seed_ic: &crate::db::IntensityComponents,
) -> [(f32, f32, f32); 10] {
    [
        (0.15, cand_ic.spectral_flux,        seed_ic.spectral_flux),
        (0.25, cand_ic.flatness,             seed_ic.flatness),
        (0.10, cand_ic.spectral_centroid,    seed_ic.spectral_centroid),
        (0.20, cand_ic.dissonance,           seed_ic.dissonance),
        (0.15, cand_ic.crest_factor,         seed_ic.crest_factor),
        (0.00, cand_ic.energy_variance,      seed_ic.energy_variance),
        (0.05, 1.0 - cand_ic.harmonic_complexity, 1.0 - seed_ic.harmonic_complexity),
        (0.04, cand_ic.spectral_rolloff,     seed_ic.spectral_rolloff),
        (0.04, cand_ic.centroid_variance,    seed_ic.centroid_variance),
        (0.02, cand_ic.flux_variance,        seed_ic.flux_variance),
    ]
}

/// Per-component intensity reward with directional one-sided linear falloff.
///
/// **Center**: weighted Euclidean distance between ranked component vectors (match character).
///
/// **Extremes**: one-sided linear reward per component.
///   - Target = interpolation from seed toward library extreme (0.0 for drop, 1.0 for peak).
///     Reach controls how far: Tight=50%, Medium=75%, Open=100% of the way.
///   - Wrong direction (above seed at drop / below at peak) = sharp linear penalty to 0.
///   - Right direction = gentle linear falloff from target (1.0) to seed (moderate) to extreme (still OK).
///   - The entire "less aggressive" zone scores well, with target zone preferred.
pub fn intensity_reward_per_component(
    cand_ic: &crate::db::IntensityComponents,
    seed_ic: &crate::db::IntensityComponents,
    energy_bias: f32,
    blend_crossover: f32,
    intensity_reach: f32,
) -> f32 {
    let weights = intensity_component_pairs(cand_ic, seed_ic);
    let blend_t = (energy_bias.abs() / blend_crossover).clamp(0.0, 1.0);

    // Center: weighted Euclidean distance → reward (similar character)
    let dist_sq: f32 = weights.iter()
        .map(|(w, c, s)| w * (c - s).powi(2))
        .sum();
    let match_reward = 1.0 - dist_sq.sqrt().min(1.0);

    // Extremes: one-sided linear reward per component.
    //
    // Target: slider position interpolates from seed (center) to library extreme
    // (full peak → 1.0, full drop → 0.0). The slider itself controls how far.
    //
    // Reach controls acceptance width around the target:
    //   Tight (0.15) = narrow zone, only very close to target scores well
    //   Open (0.50) = wide zone, broad acceptance in the right direction
    //
    // One-sided: wrong direction from seed gets sharp penalty to 0.
    let is_peak = energy_bias >= 0.0;

    let directional_reward: f32 = weights.iter()
        .map(|(w, c, s)| {
            if *w < 1e-6 { return 0.0; }

            // Target: blend_t interpolates from seed toward extreme
            let target = if is_peak {
                s + (1.0 - s) * blend_t   // center: seed, full peak: 1.0
            } else {
                s * (1.0 - blend_t)        // center: seed, full drop: 0.0
            };

            // One-sided linear falloff:
            let delta = if is_peak { c - s } else { s - c }; // positive = right direction

            if delta < 0.0 {
                // Wrong direction: sharp linear penalty
                w * (1.0 + delta).max(0.0)
            } else {
                // Right direction: reward based on distance from target
                // Width of acceptance zone scales with intensity_reach
                let target_delta = (c - target).abs();
                let width = intensity_reach.max(0.10); // minimum width to avoid division issues
                let reward = (1.0 - target_delta / width).max(0.0);
                w * reward
            }
        })
        .sum();

    match_reward * (1.0 - blend_t) + directional_reward * blend_t
}

/// Hybrid intensity reward: per-component distance at center, composite direction at extremes.
pub fn intensity_reward_hybrid(
    cand_ic: &crate::db::IntensityComponents,
    seed_ic: &crate::db::IntensityComponents,
    cand_composite: f32,
    seed_composite: f32,
    energy_bias: f32,
    blend_crossover: f32,
    intensity_reach: f32,
) -> f32 {
    let blend_t = (energy_bias.abs() / blend_crossover).clamp(0.0, 1.0);

    // Center: per-component distance (match character)
    let per_comp = intensity_reward_per_component(cand_ic, seed_ic, 0.0, blend_crossover, intensity_reach);

    // Extreme: composite with one-sided linear falloff
    let is_peak = energy_bias >= 0.0;
    let target = if is_peak {
        seed_composite + (1.0 - seed_composite) * blend_t
    } else {
        seed_composite * (1.0 - blend_t)
    };

    let delta = if is_peak { cand_composite - seed_composite } else { seed_composite - cand_composite };
    let width = intensity_reach.max(0.10);
    let composite_reward = if delta < 0.0 {
        (1.0 + delta).max(0.0)
    } else {
        let target_delta = (cand_composite - target).abs();
        (1.0 - target_delta / width).max(0.0)
    };

    per_comp * (1.0 - blend_t) + composite_reward * blend_t
}

/// Stem complement component — bipolar [0, 1].
///
/// - seed=1, cand=0 (or vice versa): → 1.0 (fully complementary, max boost)
/// - seed=1, cand=1 (both high):     → 0.0 (fully clashing, max penalty)
/// - seed=0, cand=0 (both silent):   → 0.5 (neutral)
///
/// Formula: `(|seed - cand| - min(seed, cand) + 1) / 2`
pub fn stem_complement_component(seed: f32, cand: f32) -> f32 {
    ((seed - cand).abs() - seed.min(cand) + 1.0) / 2.0
}

// ─── Reason Tag Generation ──────────────────────────────────────────

/// Human-readable label for a transition type
pub fn transition_type_label(tt: TransitionType) -> &'static str {
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

// Suggestion tag color markers — sentinel hex values remapped to theme stem
// colors by resolve_tag_color() on the UI side.
// Good → Vocals stem, Moderate → Bass stem, Poor → theme danger (passthrough).
pub const TAG_COLOR_GOOD: &str = "#00AA01";
pub const TAG_COLOR_MODERATE: &str = "#00AA02";
pub const TAG_COLOR_SOURCE: &str = "#00AA03";
pub const TAG_COLOR_POOR: &str = "#a63d40";
/// Color a tag by reward quality: high reward = good, low = poor.
pub fn reward_color(reward: f32) -> &'static str {
    if reward >= 0.7 { TAG_COLOR_GOOD }
    else if reward >= 0.4 { TAG_COLOR_MODERATE }
    else { TAG_COLOR_POOR }
}

/// Color a key transition tag by its inherent harmonic quality (base_score).
/// Uses the same thresholds as the energy arc ribbon for visual consistency.
pub fn transition_color(tt: TransitionType) -> &'static str {
    let bs = base_score(tt);
    if bs >= 0.80 { TAG_COLOR_GOOD }
    else if bs >= 0.40 { TAG_COLOR_MODERATE }
    else { TAG_COLOR_POOR }
}

/// Color a tag by penalty quality (legacy, for reason tags that still use penalty framing).
pub fn penalty_color(penalty: f32) -> &'static str {
    reward_color(1.0 - penalty)
}

/// Generate human-readable reason tags from the full scoring breakdown.
///
/// All component values are rewards [0, 1] where higher = better match.
/// Tags are colored green (high reward) → amber → red (low reward).
///
/// Directional arrows indicate transition direction:
/// - **▲** = raises energy/tension
/// - **▼** = lowers energy/tension
/// - **━** = neutral/same
#[allow(clippy::too_many_arguments)]
pub fn generate_reason_tags(
    transition_type: TransitionType,
    // Raw cosine similarity [0, 1] — higher = more similar to seed
    raw_similarity: f32,
    // Energy delta: candidate_intensity - seed_intensity, positive = more energy
    energy_delta: f32,
    // Stem complement scores (0=clashing, 0.5=neutral, 1=complementary)
    vocal_comp: f32,
    other_comp: f32,
    w_vocal_compl: f32,
    w_other_compl: f32,
) -> Vec<(String, Option<String>)> {
    let mut tags: Vec<(String, Option<String>, f32)> = Vec::with_capacity(8);

    // --- Key tag (always first): transition type name, colored by harmonic quality ---
    let key_label = transition_type_label(transition_type);
    let key_color = transition_color(transition_type);
    tags.push((key_label.to_string(), Some(key_color.to_string()), f32::MAX));

    let min_weight = 0.03;

    // --- Similarity tag: raw cosine similarity relative to seed ---
    // Green = very similar, amber = moderate, red = dissimilar
    let sim_color = reward_color(raw_similarity);
    tags.push(("Similarity".to_string(), Some(sim_color.to_string()), raw_similarity.abs()));

    // --- Energy tag: relative energy shift from seed ---
    // Good = more energy, moderate = roughly same, poor = less energy
    let energy_color = if energy_delta > 0.15 { TAG_COLOR_GOOD }
        else if energy_delta > -0.15 { TAG_COLOR_MODERATE }
        else { TAG_COLOR_POOR };
    tags.push(("Energy".to_string(), Some(energy_color.to_string()), energy_delta.abs()));

    // --- Stem complement tags: complementary → good, clashing → poor ---
    if w_vocal_compl >= min_weight {
        if vocal_comp > 0.65 || vocal_comp < 0.35 {
            let color = if vocal_comp > 0.65 { TAG_COLOR_GOOD } else { TAG_COLOR_POOR };
            let impact = w_vocal_compl * (vocal_comp - 0.5).abs();
            tags.push(("Vocals".to_string(), Some(color.to_string()), impact));
        }
    }
    if w_other_compl >= min_weight {
        if other_comp > 0.65 || other_comp < 0.35 {
            let color = if other_comp > 0.65 { TAG_COLOR_GOOD } else { TAG_COLOR_POOR };
            let impact = w_other_compl * (other_comp - 0.5).abs();
            tags.push(("Lead".to_string(), Some(color.to_string()), impact));
        }
    }

    // Sort non-key tags by impact descending (key stays first via f32::MAX)
    tags.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    // Strip the impact score from the output
    tags.into_iter().map(|(label, color, _)| (label, color)).collect()
}

// ─── Tests ────────────��─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Transition Classification ───��──────────────────────────────

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
        let bbm = MusicalKey::parse("Bbm").unwrap(); // 3A
        assert_eq!(classify_transition(&am, &bbm), TransitionType::SemitoneUp);
    }

    #[test]
    fn test_classify_tritone() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let ebm = MusicalKey::parse("Ebm").unwrap(); // 2A
        assert_eq!(classify_transition(&am, &ebm), TransitionType::Tritone);
    }

    #[test]
    fn test_classify_far_step() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
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
        assert!(peak_score >= 0.65, "Energy boost at peak should be competitive: {}", peak_score);
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

        let same = key_transition_score(&am, &am, 1.0, CAM);
        let adj_up = key_transition_score(&am, &em, 1.0, CAM);
        let boost = key_transition_score(&am, &bm, 1.0, CAM);
        let semi_up = key_transition_score(&am, &bbm, 1.0, CAM);

        assert!(semi_up > same, "SemitoneUp should outscore SameKey at +1.0: {} vs {}", semi_up, same);
        assert!(boost > same, "EnergyBoost should outscore SameKey at +1.0: {} vs {}", boost, same);
        assert!(adj_up > same, "AdjacentUp should outscore SameKey at +1.0: {} vs {}", adj_up, same);
        assert!(semi_up > boost, "SemitoneUp > EnergyBoost: {} vs {}", semi_up, boost);
        assert!(boost > adj_up, "EnergyBoost > AdjacentUp: {} vs {}", boost, adj_up);
    }

    #[test]
    fn test_key_score_extreme_drop_prefers_energy_lowering() {
        let am = MusicalKey::parse("Am").unwrap();
        let gm = MusicalKey::parse("Gm").unwrap();  // EnergyCool (-2)
        let dm = MusicalKey::parse("Dm").unwrap();  // AdjacentDown (-1)

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
        let raise = key_transition_score(&am, &c, 0.5, CAM);
        let drop = key_transition_score(&am, &c, -0.5, CAM);
        assert!(raise > drop, "Mood lift should score better when raising than dropping: {} vs {}", raise, drop);
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

    // ─── Dual Harmonic Filter ───────────────────────────────────────

    #[test]
    fn test_harmonic_floor_blocks_dissonant_transitions() {
        const STRICT_FLOOR: f32 = 0.45;
        assert!(base_score(TransitionType::SemitoneUp)   < STRICT_FLOOR);
        assert!(base_score(TransitionType::SemitoneDown) < STRICT_FLOOR);
        assert!(base_score(TransitionType::FarStep(3))   < STRICT_FLOOR);
        assert!(base_score(TransitionType::FarCross(1))  < STRICT_FLOOR);
        assert!(base_score(TransitionType::Tritone)      < STRICT_FLOOR);
        assert!(base_score(TransitionType::EnergyBoost) >= STRICT_FLOOR);
        assert!(base_score(TransitionType::EnergyCool)  >= STRICT_FLOOR);
    }

    #[test]
    fn test_blended_threshold_flow_and_break_zones() {
        let am = MusicalKey::parse("Am").unwrap(); // 8A
        let bm = MusicalKey::parse("Bm").unwrap(); // 10A = EnergyBoost from Am (+2 steps)

        const STRICT_BLENDED: f32 = 0.65;

        let score_center = key_transition_score(&am, &bm, 0.0, CAM);
        assert!(score_center < STRICT_BLENDED,
            "EnergyBoost blocked at center: {:.2} < {:.2}", score_center, STRICT_BLENDED);

        let score_extreme = key_transition_score(&am, &bm, 1.0, CAM);
        assert!(score_extreme >= STRICT_BLENDED,
            "EnergyBoost unlocks at extreme: {:.2} >= {:.2}", score_extreme, STRICT_BLENDED);

        let score_same = key_transition_score(&am, &am, 1.0, CAM);
        assert!(score_same < STRICT_BLENDED,
            "SameKey blocked at extreme: {:.2} < {:.2}", score_same, STRICT_BLENDED);
    }

    // ─── Key Direction Penalty ──────────────────────────────────────

    #[test]
    fn test_key_dir_center_is_neutral() {
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

        assert!(semitone_up < 0.5, "Semitone up should be below neutral when raising: {}", semitone_up);
        assert!(energy_boost < 0.5, "Energy boost should be below neutral when raising: {}", energy_boost);
        assert_eq!(same_key, 0.5);
        assert!(energy_cool > 0.5, "Energy cool should be above neutral when raising: {}", energy_cool);
        assert!(semitone_down > 0.5, "Semitone down should be above neutral when raising: {}", semitone_down);
        assert!(semitone_up < energy_boost, "Semitone up should be preferred over boost: {} vs {}", semitone_up, energy_boost);
    }

    #[test]
    fn test_key_dir_drop_prefers_energy_lowering_transitions() {
        let energy_cool = key_direction_penalty(TransitionType::EnergyCool, -1.0);
        let mood_darken = key_direction_penalty(TransitionType::MoodDarken, -1.0);
        let same_key = key_direction_penalty(TransitionType::SameKey, -1.0);
        let mood_lift = key_direction_penalty(TransitionType::MoodLift, -1.0);
        let energy_boost = key_direction_penalty(TransitionType::EnergyBoost, -1.0);

        assert!(energy_cool < 0.5, "Energy cool should be below neutral when dropping: {}", energy_cool);
        assert!(mood_darken < 0.5, "Mood darken should be below neutral when dropping: {}", mood_darken);
        assert!(mood_lift > 0.5, "Mood lift should be above neutral when dropping: {}", mood_lift);
        assert!(energy_boost > 0.5, "Energy boost should be above neutral when dropping: {}", energy_boost);
        assert_eq!(same_key, 0.5);
    }

    #[test]
    fn test_key_dir_scales_with_fader() {
        let full = key_direction_penalty(TransitionType::SemitoneUp, 1.0);
        let half = key_direction_penalty(TransitionType::SemitoneUp, 0.5);
        assert!(half < 0.5, "Should still be below neutral at half fader");
        assert!(half > full, "Effect should be stronger at full fader: half={} full={}", half, full);
    }

    // ─── Krumhansl Matrix Validation ─────���──────────────────────────

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
        let m = &*KRUMHANSL_MATRIX;
        let am_c = m[21][0]; // Am→C
        assert!(am_c > 0.5, "Relative keys should be close: {am_c}");
    }

    #[test]
    fn test_krumhansl_parallel_keys_moderate() {
        let m = &*KRUMHANSL_MATRIX;
        let c_cm = m[0][12];
        assert!(c_cm > 0.3, "Parallel keys should be moderately close: {c_cm}");
    }

    #[test]
    fn test_krumhansl_tritone_distant() {
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
        assert!(camelot > 0.5, "Camelot Am→C should be high: {camelot}");
        assert!(krumhansl > 0.5, "Krumhansl Am→C should be high: {krumhansl}");
    }

    #[test]
    fn test_similarity_reward_center_targets_small_radius() {
        // At center the ring sits at CENTER_RADIUS = 0.05, width 0.15.
        // A track at distance 0.05 should peak; far-off tracks land on the floor.
        let peak = similarity_reward(0.05, 0.0, 0.40);
        let floor = similarity_reward(0.50, 0.0, 0.40);
        assert!(peak > 0.95, "center peak should be near 1.0: {peak}");
        assert!(floor < 0.25, "far track should be near floor 0.20: {floor}");
        assert!(floor >= 0.20, "soft floor should hold at 0.20: {floor}");
    }

    #[test]
    fn test_similarity_reward_extreme_shifts_to_reach() {
        // At full slider with reach=0.40, the ring centre slides to 0.40.
        let peak_extreme = similarity_reward(0.40, 1.0, 0.40);
        let center_extreme = similarity_reward(0.05, 1.0, 0.40);
        assert!(peak_extreme > 0.95, "extreme should peak at reach: {peak_extreme}");
        assert!(center_extreme < peak_extreme, "near-seed should score below ring at extreme: {center_extreme} vs {peak_extreme}");
    }

    #[test]
    fn test_similarity_reward_symmetric_in_bias_sign() {
        // Distance is unsigned, so peak (+0.7) and drop (-0.7) score identically.
        let plus = similarity_reward(0.30, 0.7, 0.40);
        let minus = similarity_reward(0.30, -0.7, 0.40);
        assert!((plus - minus).abs() < 1e-6, "symmetric in sign: {plus} vs {minus}");
    }

    #[test]
    fn test_similarity_reward_tight_reach_still_moves() {
        // Tight reach=0.15 must still produce visible movement from centre to extreme.
        let center = similarity_reward(0.05, 0.0, 0.15); // peak at 0.05
        let extreme = similarity_reward(0.05, 1.0, 0.15); // ring shifted to 0.15
        assert!(center > extreme, "centre should outrank near-seed track at extreme: {center} vs {extreme}");
        let extreme_at_target = similarity_reward(0.15, 1.0, 0.15);
        assert!(extreme_at_target > extreme, "ring shift should reward target distance at extreme: {extreme_at_target} vs {extreme}");
    }

}
