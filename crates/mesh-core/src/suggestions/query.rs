//! Query orchestration for the smart suggestion engine.
//!
//! Contains the main `query_suggestions()` function that coordinates HNSW search,
//! multi-factor scoring, harmonic filtering, and result ranking.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use crate::db::{DatabaseService, MlScores, Track};
use crate::music::MusicalKey;
use super::config::KeyScoringModel;
use super::scoring::*;

/// Scoring configuration passed into `query_suggestions`.
/// Groups the user-adjustable algorithm parameters so the function signature stays clean.
#[derive(Debug, Clone, Copy)]
pub struct SuggestionConfig {
    /// Crossover threshold [0, 1]: how far the intent slider must move from center
    /// before the vector component flips from similarity to dissimilarity.
    /// Lower = switches to transition mode earlier. See `SuggestionBlendMode`.
    pub blend_crossover: f32,
    /// Minimum `base_score(TransitionType)` required to enter scoring (key filter layer 1).
    pub harmonic_floor: f32,
    /// Minimum energy-blended key score required (key filter layer 2).
    pub blended_threshold: f32,
    /// Enable stem complement penalty (clashing vocals/lead reduce score).
    pub stem_complement: bool,
}

impl SuggestionConfig {
    /// Build a SuggestionConfig from the suggestion config enum values.
    pub fn from_display(
        blend_mode: super::config::SuggestionBlendMode,
        key_filter: super::config::SuggestionKeyFilter,
        stem_complement: bool,
    ) -> Self {
        let (harmonic_floor, blended_threshold) = key_filter.thresholds();
        Self {
            blend_crossover: blend_mode.crossover(),
            harmonic_floor,
            blended_threshold,
            stem_complement,
        }
    }
}

/// Per-component score breakdown for a suggested track.
///
/// Populated when `emit_components: true` (graph view / inspection).
/// `None` on the hot path (player) to avoid allocation overhead.
#[derive(Debug, Clone)]
pub struct ComponentScores {
    /// Raw PCA-128 cosine distance [0, max_pool]
    pub hnsw_distance: f32,
    /// Goldilocks-transformed value fed into the composite score
    pub hnsw_component: f32,
    /// Best energy-blended key score across seeds [0, 1] (1 = perfect)
    pub key_score: f32,
    /// Key direction penalty [0, 1] (0 = aligned with fader)
    pub key_direction: f32,
    /// Intensity penalty [0, 1]
    pub intensity_penalty: f32,
    /// Time-decayed co-play weight [0, 1]
    pub coplay_score: f32,
    /// Vocal stem complement [0, 1] (1 = complementary)
    pub vocal_complement: f32,
    /// Other stem complement [0, 1]
    pub other_complement: f32,
    /// Classified transition type for this candidate
    pub transition_type: TransitionType,
}

/// A suggested track with its computed score (lower = better match)
#[derive(Debug, Clone)]
pub struct SuggestedTrack {
    pub track: Track,
    pub score: f32,
    /// Auto-generated reason tags as (label, hex_color)
    pub reason_tags: Vec<(String, Option<String>)>,
    /// Playlist names this track belongs to (populated after query)
    pub playlists: Vec<String>,
    /// True when this track has been historically played after a current seed (co-play count ≥ threshold)
    pub is_proven_followup: bool,
    /// Per-component score breakdown (only populated when emit_components=true)
    pub component_scores: Option<ComponentScores>,
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

/// Query the database for track suggestions based on loaded deck seeds.
///
/// This runs on a background thread via `Task::perform()`.
///
/// # Algorithm
/// 1. Resolve each seed path to a track ID
/// 2. For each seed, find similar tracks via HNSW index
/// 3. Merge results keeping the best (minimum) distance per candidate
/// 4. Score using unified formula: hnsw_blend + key + key_dir + bpm + energy signals
///    The HNSW component is normalised within the pool and flips direction with the fader:
///    centre → rewards similarity, extreme → rewards spectral diversity.
/// 5. Filter by fixed harmonic threshold (0.50) — the energy-blended key score is
///    naturally energy-direction-aware so no adaptive relaxation is needed
/// 6. Sort and return the top results
///
/// When `emit_components` is true, each result includes a full `ComponentScores`
/// breakdown for inspection/debugging (graph view). This adds minor allocation
/// overhead so the player path passes `false`.
pub fn query_suggestions(
    sources: &[DbSource],
    seed_paths: Vec<String>,
    energy_direction: f32,
    key_scoring_model: KeyScoringModel,
    suggestion_config: SuggestionConfig,
    _per_seed_limit: usize,
    total_limit: usize,
    played_paths: &HashSet<String>,
    // Tracks whose absolute path is in this set are treated as "preferred" —
    // they receive a 50% more lenient key filter threshold so that user-curated
    // playlist tracks appear even when their key relationship is less ideal.
    preferred_paths: Option<&HashSet<String>>,
    // Pre-computed blend vector for dual-deck mode (average of both seeds' PCA/ML embeddings,
    // L2-normalised). When `Some`, the per-seed HNSW routing is skipped and a single
    // vector query is issued per source instead.
    blend_query_vec: Option<Vec<f64>>,
    emit_components: bool,
) -> Result<Vec<SuggestedTrack>, String> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }

    // Opener mode: no seeds playing → score candidates on-the-fly from intro quality
    if seed_paths.is_empty() {
        return query_opener_suggestions(sources, energy_direction, total_limit, played_paths);
    }

    // Diagnostic: log audio features count per source
    for (idx, source) in sources.iter().enumerate() {
        let features = source.db.count_audio_features().unwrap_or(0);
        log::debug!("[SUGGESTIONS] Source {} ({}): audio_features={}", idx, source.name, features);
    }

    // Step 1: Resolve seed paths to tracks across all database sources.
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
        log::debug!("[SUGGESTIONS] No seeds resolved in any DB — returning empty");
        return Ok(Vec::new());
    }

    // Seed filenames for cross-DB deduplication (same audio file may exist in multiple DBs)
    let seed_filenames: HashSet<String> = seed_tracks
        .iter()
        .filter_map(|(_, t)| t.path.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();

    // Step 2: Brute-force candidate selection via PCA cosine distance.
    // Loads ALL tracks with PCA embeddings and computes exact distances.
    // This replaces the HNSW approximate search — exact and scores every track.
    let mut candidates: HashMap<(usize, i64), (Track, f32)> = HashMap::new();

    // Build seed PCA vectors (for cosine distance computation)
    let seed_pca_vecs: Vec<(usize, Vec<f32>)> = seed_tracks
        .iter()
        .filter_map(|(src_idx, track)| {
            let id = track.id?;
            let vec = sources[*src_idx].db.get_pca_embedding_raw(id).ok().flatten()?;
            Some((*src_idx, vec))
        })
        .collect();

    // If blend vector provided, use that instead of per-seed vectors
    let blend_pca: Option<Vec<f32>> = blend_query_vec.as_ref().map(|v| {
        v.iter().map(|&x| x as f32).collect()
    });

    for (src_idx, source) in sources.iter().enumerate() {
        // Load all tracks + PCA embeddings from this source
        let all_pca = match source.db.get_all_pca_with_tracks() {
            Ok(v) => v,
            Err(e) => {
                log::warn!("[SUGGESTIONS] Failed to load PCA embeddings from source {}: {}", source.name, e);
                continue;
            }
        };

        log::debug!(
            "[SUGGESTIONS] Brute-force: source '{}' has {} tracks with PCA embeddings",
            source.name, all_pca.len()
        );

        for (mut track, pca_vec) in all_pca {
            let track_id = match track.id {
                Some(id) => id,
                None => continue,
            };

            // Skip seed tracks
            if let Some(name) = track.path.file_name() {
                if seed_filenames.contains(&*name.to_string_lossy()) { continue; }
            }

            // Resolve relative paths
            if !track.path.is_absolute() {
                track.path = source.collection_root.join(&track.path);
            }

            // Skip already-played tracks
            if played_paths.contains(&*track.path.to_string_lossy()) { continue; }

            // Compute cosine distance to seed(s)
            let dist = if let Some(ref blend) = blend_pca {
                cosine_distance(blend, &pca_vec)
            } else {
                // Best (minimum) distance across all seed PCA vectors
                seed_pca_vecs.iter()
                    .map(|(_, sv)| cosine_distance(sv, &pca_vec))
                    .fold(f32::MAX, f32::min)
            };

            candidates
                .entry((src_idx, track_id))
                .and_modify(|(_, existing_dist)| {
                    if dist < *existing_dist { *existing_dist = dist; }
                })
                .or_insert((track, dist));
        }
    }

    // Cross-source dedup: same track may exist in both Local and USB DBs.
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

    // Co-play boost: fetch historically proven follow-up tracks for all seeds.
    let coplay_boost: HashMap<(usize, i64), f32> = {
        let mut boost: HashMap<(usize, i64), f32> = HashMap::new();
        for &(seed_src_idx, ref seed_track) in &seed_tracks {
            let Some(seed_id) = seed_track.id else { continue };
            match sources[seed_src_idx].db.get_played_after_neighbors(seed_id, 100) {
                Ok(neighbors) => {
                    for (cand_id, w) in neighbors {
                        boost.entry((seed_src_idx, cand_id))
                            .and_modify(|v| *v = v.max(w))
                            .or_insert(w);
                    }
                }
                Err(e) => log::debug!("[SUGGESTIONS] co-play fetch failed for seed {}: {}", seed_id, e),
            }
        }
        log::debug!("[SUGGESTIONS] co-play boost map: {} tracks have proven history", boost.len());
        boost
    };

    // Step 4: Compute seed averages for scoring

    let seed_keys: Vec<MusicalKey> = seed_tracks
        .iter()
        .filter_map(|(_, t)| t.key.as_deref().and_then(MusicalKey::parse))
        .collect();

    // Energy direction bias: -1.0 (drop) through 0.0 (maintain) to +1.0 (peak)
    let energy_bias = (energy_direction - 0.5) * 2.0;

    // Step 4b: Batch-fetch ML scores from each source DB
    let ml_scores: HashMap<(usize, i64), MlScores> = {
        let mut ids_by_source: HashMap<usize, Vec<i64>> = HashMap::new();
        for &(src_idx, track_id) in candidates.keys() {
            ids_by_source.entry(src_idx).or_default().push(track_id);
        }
        for &(src_idx, ref track) in &seed_tracks {
            if let Some(id) = track.id {
                ids_by_source.entry(src_idx).or_default().push(id);
            }
        }
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

    // Step 4c: Stem energy — seed densities + batch candidate prefetch
    let seed_stem: (f32, f32, f32, f32) = seed_tracks
        .iter()
        .find_map(|(idx, t)| {
            t.id.and_then(|id| {
                sources[*idx].db.get_stem_energy(id).ok().flatten()
            })
        })
        .unwrap_or((0.5, 0.4, 0.1, 0.2)); // neutral defaults

    let stem_map: HashMap<(usize, i64), (f32, f32, f32, f32)> = {
        let mut ids_by_source: HashMap<usize, Vec<i64>> = HashMap::new();
        for &(src_idx, track_id) in candidates.keys() {
            ids_by_source.entry(src_idx).or_default().push(track_id);
        }
        let mut merged = HashMap::new();
        for (src_idx, ids) in &ids_by_source {
            if let Ok(m) = sources[*src_idx].db.batch_get_stem_energy(ids) {
                for (id, densities) in m {
                    merged.insert((*src_idx, id), densities);
                }
            }
        }
        merged
    };

    // Step 4d: Batch-fetch flatness and dissonance for all candidates
    let flatness_map: HashMap<(usize, i64), f32> = {
        let mut ids_by_source: HashMap<usize, Vec<i64>> = HashMap::new();
        for &(src_idx, track_id) in candidates.keys() {
            ids_by_source.entry(src_idx).or_default().push(track_id);
        }
        for (src_idx, t) in &seed_tracks {
            if let Some(id) = t.id {
                ids_by_source.entry(*src_idx).or_default().push(id);
            }
        }
        let mut merged = HashMap::new();
        for (src_idx, ids) in &ids_by_source {
            if let Ok(m) = sources[*src_idx].db.batch_get_flatness(ids) {
                for (id, f) in m { merged.insert((*src_idx, id), f); }
            }
        }
        merged
    };

    let dissonance_map: HashMap<(usize, i64), f32> = {
        let mut ids_by_source: HashMap<usize, Vec<i64>> = HashMap::new();
        for &(src_idx, track_id) in candidates.keys() {
            ids_by_source.entry(src_idx).or_default().push(track_id);
        }
        for (src_idx, t) in &seed_tracks {
            if let Some(id) = t.id {
                ids_by_source.entry(*src_idx).or_default().push(id);
            }
        }
        let mut merged = HashMap::new();
        for (src_idx, ids) in &ids_by_source {
            if let Ok(m) = sources[*src_idx].db.batch_get_dissonance(ids) {
                for (id, d) in m { merged.insert((*src_idx, id), d); }
            }
        }
        merged
    };

    // Step 4e: Genre-normalize composite intensity across the candidate pool.
    let norm_intensity = normalize_intensity_by_genre(&ml_scores, &flatness_map, &dissonance_map);
    let avg_seed_intensity = {
        let vals: Vec<f32> = seed_tracks
            .iter()
            .filter_map(|(idx, t)| t.id.map(|id| (*idx, id)))
            .filter_map(|key| norm_intensity.get(&key).copied())
            .collect();
        if vals.is_empty() { 0.5 } else { vals.iter().sum::<f32>() / vals.len() as f32 }
    };

    // ════════════════════════════════════════════════════════════════════════
    // Step 5: Reward-based scoring (higher = better match)
    // ════════════════════════════════════════════════════════════════════════
    //
    // Each component contributes a REWARD in [0, weight] to the total score.
    // A perfect match on all dimensions yields score ≈ 1.0.
    // A terrible match yields score ≈ 0.0.
    //
    // ┌─────────────────────────────────────────────────────────────────────┐
    // │ Component     │ Weight │ Center (layering)  │ Extremes (transition)│
    // ├───────────────┼────────┼────────────────────┼──────────────────────┤
    // │ Intensity     │ 0.30   │ Same level → high  │ Directional → high  │
    // │ Key compat.   │ 0.30   │ Compatible → high  │ Energy-aligned→high │
    // │ Vector sim.   │ 0.25   │ Similar → high     │ Dissimilar → high   │
    // │ Co-play hist. │ 0.07   │ Proven → high      │ Fades to 0          │
    // ├───────────────┼────────┼────────────────────┴──────────────────────┤
    // │ Stem penalty  │ -0.13  │ Clashing vocals/lead SUBTRACT from score │
    // │ (only center) │        │ Complementary = no penalty               │
    // └─────────────────────────────────────────────────────────────────────┘
    //
    // Note: key_transition_score() already blends harmonic compatibility with
    // energy direction based on the slider position, so no separate key_dir
    // component is needed.
    //
    // Slider semantics:
    //   Center (0.5): Find tracks for LAYERING — similar sound, compatible key,
    //                 similar energy, complementary stems. Good for mashups.
    //   High (→1.0):  Find tracks for TRANSITION UP — dissimilar sound, energy-
    //                 raising key transitions, more aggressive. Build energy.
    //   Low (→0.0):   Find tracks for TRANSITION DOWN — dissimilar sound, energy-
    //                 lowering key transitions, less aggressive. Wind down.
    //
    // Sort: DESCENDING (higher score = better match).

    let bias_abs = energy_bias.abs();
    let w_intensity   = 0.30;
    let w_key         = 0.30;
    let w_vector      = 0.25;
    let w_coplay      = 0.07 * (1.0 - bias_abs);           // 0.07 center → 0.00 extreme
    // Stem penalty weights (only at center, subtracted from score)
    let w_vocal_pen = if suggestion_config.stem_complement { 0.08 * (1.0 - bias_abs) } else { 0.0 };
    let w_other_pen = if suggestion_config.stem_complement { 0.05 * (1.0 - bias_abs) } else { 0.0 };

    let max_hnsw_dist = candidates.values()
        .map(|(_, d)| *d)
        .fold(0.0_f32, f32::max)
        .max(1e-6);

    let source_names: HashMap<usize, &str> = sources.iter().enumerate()
        .map(|(i, s)| (i, s.name.as_str()))
        .collect();
    let multi_source = {
        let active_sources: HashSet<usize> = candidates.keys().map(|(idx, _)| *idx).collect();
        active_sources.len() > 1
    };

    let mut suggestions: Vec<SuggestedTrack> = candidates
        .into_iter()
        .filter_map(|((src_idx, track_id), (track, hnsw_dist))| {
            // Key transition score: best match across all seeds
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
                .unwrap_or((0.3, TransitionType::FarStep(6)));

            // Dual-layer harmonic filter
            let harmonic_base = base_score(best_tt);
            if harmonic_base < suggestion_config.harmonic_floor {
                return None;
            }
            let is_preferred = preferred_paths.map_or(false, |pp| {
                let p = track.path.to_string_lossy();
                pp.contains(p.as_ref())
            });
            let effective_threshold = if is_preferred {
                suggestion_config.blended_threshold * 0.5
            } else {
                suggestion_config.blended_threshold
            };
            if best_key_score < effective_threshold {
                return None;
            }

            // ── Key reward ──
            // key_transition_score already returns [0, 1] reward that blends harmonic
            // compatibility with energy direction based on slider position.
            // No separate key_dir component needed — it's already incorporated.
            let key_reward = best_key_score;

            // ── Intensity reward ──
            let ml_key = track.id.map(|id| (src_idx, id));
            let cand_norm_intensity = ml_key
                .and_then(|k| norm_intensity.get(&k).copied())
                .unwrap_or(0.5);
            let int_reward = intensity_reward(cand_norm_intensity, avg_seed_intensity, energy_bias);

            // ── Vector similarity reward ──
            // Center: similarity → high reward (1 - normalized_distance).
            // Extremes: dissimilarity → high reward (normalized_distance).
            // The blend_crossover controls how far the slider must move before
            // similarity fully transitions to dissimilarity:
            //   t = (|bias| / crossover).clamp(0, 1)
            //   vec_reward = similarity * (1-t) + dissimilarity * t
            let norm_dist = hnsw_dist / max_hnsw_dist;
            let similarity = 1.0 - norm_dist;
            let dissimilarity = norm_dist;
            let blend_t = (bias_abs / suggestion_config.blend_crossover).clamp(0.0, 1.0);
            let vec_reward = similarity * (1.0 - blend_t) + dissimilarity * blend_t;

            // ── Co-play reward ──
            // Proven follow-ups get a boost. No history = 0 reward (not a penalty).
            let coplay = coplay_boost.get(&(src_idx, track_id)).copied().unwrap_or(0.0);
            let coplay_reward = coplay;

            // ── Stem complement penalty ──
            // Clashing stems (both loud) SUBTRACT from score — bad for layering.
            // Complementary (one loud, one quiet) = no penalty.
            let cand_stem = track.id
                .and_then(|id| stem_map.get(&(src_idx, id)).copied())
                .unwrap_or((0.5, 0.4, 0.1, 0.2));
            let vocal_comp = stem_complement_component(seed_stem.0, cand_stem.0);
            let other_comp = stem_complement_component(seed_stem.3, cand_stem.3);
            // Penalty = how much to subtract for clashing (0 = complementary, 1 = clashing)
            let vocal_clash = 1.0 - vocal_comp;
            let other_clash = 1.0 - other_comp;

            // ── Final score: sum of rewards minus stem penalties ──
            let score = (w_vector    * vec_reward
                + w_key       * key_reward
                + w_intensity * int_reward
                + w_coplay    * coplay_reward
                - w_vocal_pen * vocal_clash
                - w_other_pen * other_clash)
                .clamp(0.0, 1.0);

            let is_proven_followup = coplay >= 0.3;

            let mut reason_tags = generate_reason_tags(
                best_tt, key_reward,
                vec_reward, bias_abs,
                int_reward, w_intensity,
                vocal_comp, other_comp,
                w_vocal_pen, w_other_pen,
            );

            if multi_source {
                let source_name = source_names.get(&src_idx).copied().unwrap_or("?");
                reason_tags.insert(0, (source_name.to_string(), Some("#808080".to_string())));
            }

            let component_scores = if emit_components {
                Some(ComponentScores {
                    hnsw_distance: vec_reward, // store reward, not raw distance
                    hnsw_component: vec_reward,
                    key_score: key_reward,
                    key_direction: key_reward, // same value, key already includes direction
                    intensity_penalty: int_reward, // now a reward despite the field name
                    coplay_score: coplay_reward,
                    vocal_complement: vocal_comp,
                    other_complement: other_comp,
                    transition_type: best_tt,
                })
            } else {
                None
            };

            Some(SuggestedTrack { track, score, reason_tags, playlists: Vec::new(), is_proven_followup, component_scores })
        })
        .collect();

    // Step 6: Sort DESCENDING (higher score = better match) and limit
    suggestions.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    suggestions.truncate(total_limit);

    Ok(suggestions)
}

/// Score candidates for opener mode: no decks playing, DJ is choosing the first track.
///
/// Scoring is on-the-fly from existing DB data — no re-import needed.
/// Eligible tracks must have a `drop_marker` and at least 8 intro bars.
fn query_opener_suggestions(
    sources: &[DbSource],
    energy_direction: f32,
    total_limit: usize,
    played_paths: &HashSet<String>,
) -> Result<Vec<SuggestedTrack>, String> {
    let intent_intensity = energy_direction;
    let mut results: Vec<SuggestedTrack> = Vec::new();

    #[inline]
    fn gauss(x: f32, mu: f32, sigma: f32) -> f32 {
        (-(x - mu).powi(2) / (2.0 * sigma * sigma)).exp()
    }

    for source in sources {
        let tracks = source.db.get_tracks_with_drop_marker()
            .map_err(|e| e.to_string())?;

        let ids: Vec<i64> = tracks.iter().filter_map(|t| t.id).collect();
        let stem_map     = source.db.batch_get_stem_energy(&ids).unwrap_or_default();
        let ml_map       = source.db.get_ml_scores_batch(&ids).unwrap_or_default();
        let flatness_map = source.db.batch_get_flatness(&ids).unwrap_or_default();

        for track in &tracks {
            if played_paths.contains(&*track.path.to_string_lossy()) { continue; }
            let Some(id) = track.id else { continue };
            let Some(drop_sample) = track.drop_marker else { continue };
            let Some(bpm) = track.bpm else { continue };
            if bpm < 1.0 { continue; }

            let intro_secs = drop_sample as f32 / 44100.0;
            let bars_per_sec = bpm as f32 / 240.0;
            let intro_bars = intro_secs * bars_per_sec;
            if intro_bars < 8.0 { continue; }

            let (vocal, drums, _bass, other) = stem_map.get(&id)
                .copied()
                .unwrap_or((0.2, 0.3, 0.25, 0.25));

            let ml       = ml_map.get(&id);
            let flatness = flatness_map.get(&id).copied();
            let relaxed  = ml.and_then(|m| m.relaxed);
            let intensity = composite_intensity(
                ml.and_then(|m| m.aggression), flatness, relaxed, None,
            ).unwrap_or(0.5);

            let long_intro     = (intro_bars / 64.0).min(1.0);
            let other_interest = gauss(other, 0.25, 0.12);
            let vocal_interest = gauss(vocal, 0.15, 0.10);
            let drums_ok       = 1.0 - drums.clamp(0.0, 1.0);
            let intensity_match = 1.0 - (intensity - intent_intensity).abs().clamp(0.0, 1.0);

            let raw = 0.35 * long_intro + 0.20 * other_interest
                    + 0.15 * vocal_interest + 0.15 * drums_ok + 0.15 * intensity_match;
            let score = 1.0 - raw;

            let reason_tags = vec![
                (format!("{:.0}b intro", intro_bars), Some("#3b82f6".to_string())),
            ];
            let mut track_abs = track.clone();
            if !track_abs.path.is_absolute() {
                track_abs.path = source.collection_root.join(&track_abs.path);
            }
            results.push(SuggestedTrack {
                track: track_abs,
                score,
                reason_tags,
                playlists: Vec::new(),
                is_proven_followup: false,
                component_scores: None,
            });
        }
    }

    log::debug!(
        "[SUGGESTIONS] opener mode: {} candidates scored (intent_intensity={:.2})",
        results.len(), intent_intensity
    );

    results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
    results.dedup_by(|a, b| a.track.path == b.track.path);
    results.truncate(total_limit);
    Ok(results)
}

// ════════════════════════════════════════════════════════════════════════════
// Graph edge types for the suggestion graph view
// ════════════════════════════════════════════════════════════════════════════

/// Edge in the precomputed suggestion graph (in-memory only, not a DB relation).
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub from_id: i64,
    pub to_id: i64,
    /// Raw HNSW cosine distance
    pub hnsw_distance: f32,
    /// True if a played_after edge also exists between these tracks
    pub is_played_after: bool,
    /// Time-decayed co-play weight (0 if no co-play history)
    pub played_after_weight: f32,
}

/// Cosine distance between two vectors: 1 - cosine_similarity.
/// Returns 0.0 for identical vectors, 2.0 for opposite vectors.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len().min(b.len()) {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = (norm_a * norm_b).sqrt();
    if denom < 1e-10 { 1.0 } else { 1.0 - dot / denom }
}
