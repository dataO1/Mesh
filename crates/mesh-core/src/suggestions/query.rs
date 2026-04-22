//! Query orchestration for the smart suggestion engine.
//!
//! Contains the main `query_suggestions()` function that coordinates HNSW search,
//! multi-factor scoring, harmonic filtering, and result ranking.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use crate::db::{DatabaseService, Track};
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
    /// Target distance for the transition bell curve at extreme slider.
    pub transition_target: f32,
    /// Width (2σ²) of the transition bell curve.
    pub transition_width: f32,
    /// Custom weights [similarity, key, intensity]. If None, uses defaults (0.30, 0.30, 0.33).
    pub custom_weights: Option<[f32; 3]>,
    /// Intensity matching mode (per-component / hybrid).
    pub intensity_match_mode: super::config::IntensityMatchMode,
    /// Target intensity shift at full peak/drop (percentile-rank delta from seed).
    /// Derived from SuggestionTransitionReach: Tight=0.15, Medium=0.30, Open=0.50.
    pub intensity_reach: f32,
}

impl SuggestionConfig {
    /// Build a SuggestionConfig from the suggestion config enum values.
    pub fn from_display(
        blend_mode: super::config::SuggestionBlendMode,
        key_filter: super::config::SuggestionKeyFilter,
        stem_complement: bool,
        transition_reach: super::config::SuggestionTransitionReach,
        community_thresholds: Option<&crate::graph_compute::CommunityThresholds>,
    ) -> Self {
        let (harmonic_floor, blended_threshold) = key_filter.thresholds();
        Self {
            blend_crossover: blend_mode.crossover(),
            harmonic_floor,
            blended_threshold,
            stem_complement,
            transition_target: transition_reach.target_distance(community_thresholds),
            transition_width: transition_reach.bell_width(community_thresholds),
            custom_weights: None,
            intensity_match_mode: Default::default(),
            intensity_reach: transition_reach.intensity_reach(),
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

    // Diagnostic: log track count per source
    for (idx, source) in sources.iter().enumerate() {
        log::debug!("[SUGGESTIONS] Source {} ({})", idx, source.name);
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

    // Seed artist-titles for cross-DB deduplication (same track may exist in multiple DBs)
    let seed_keys: HashSet<String> = seed_tracks
        .iter()
        .map(|(_, t)| {
            let artist = t.artist.as_deref().unwrap_or("").to_lowercase();
            let title = t.title.to_lowercase();
            format!("{}\x00{}", artist, title)
        })
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

    // If blend vector provided, use that instead of per-seed vectors.
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

            // Skip seed tracks (by artist-title match)
            let cand_artist = track.artist.as_deref().unwrap_or("").to_lowercase();
            let cand_title = track.title.to_lowercase();
            let cand_key = format!("{}\x00{}", cand_artist, cand_title);
            if seed_keys.contains(&cand_key) { continue; }

            // Resolve relative paths
            if !track.path.is_absolute() {
                track.path = source.collection_root.join(&track.path);
            }

            // Skip already-played tracks
            if played_paths.contains(&*track.path.to_string_lossy()) { continue; }

            // No L2-normalization needed — cosine distance already normalizes internally

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
    // Dedup by artist-title combination (case-insensitive), keeping the best distance.
    if sources.len() > 1 {
        let mut best_by_track: HashMap<String, (usize, i64, f32)> = HashMap::new();
        for (&(src_idx, track_id), (track, dist)) in &candidates {
            let artist = track.artist.as_deref().unwrap_or("").to_lowercase();
            let title = track.title.to_lowercase();
            let dedup_key = format!("{}\x00{}", artist, title);
            best_by_track
                .entry(dedup_key)
                .and_modify(|existing| {
                    if *dist < existing.2 {
                        *existing = (src_idx, track_id, *dist);
                    }
                })
                .or_insert((src_idx, track_id, *dist));
        }
        let keep_keys: HashSet<(usize, i64)> = best_by_track
            .values()
            .map(|&(src, id, _)| (src, id))
            .collect();
        let before = candidates.len();
        candidates.retain(|key, _| keep_keys.contains(key));
        let deduped = before - candidates.len();
        if deduped > 0 {
            log::debug!("[SUGGESTIONS] Deduped {} cross-source duplicates (by artist-title)", deduped);
        }
    }

    log::debug!("[SUGGESTIONS] Total candidates after HNSW: {}", candidates.len());

    if candidates.is_empty() {
        log::debug!("[SUGGESTIONS] No candidates — returning empty");
        return Ok(Vec::new());
    }

    // Percentile-rank normalize vector distances: each track's distance is replaced
    // by its rank position / N, giving uniform [0, 1] spread regardless of the raw
    // distance distribution. This replaces the old genre z-score + pool-max pipeline
    // which compressed the discriminative range.
    {
        let mut dist_ranking: Vec<((usize, i64), f32)> = candidates.iter()
            .map(|(&key, (_, dist))| (key, *dist))
            .collect();
        dist_ranking.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let n = dist_ranking.len().max(1) as f32;
        for (rank, &(key, _raw)) in dist_ranking.iter().enumerate() {
            if let Some((_, dist)) = candidates.get_mut(&key) {
                *dist = rank as f32 / (n - 1.0).max(1.0); // [0, 1] percentile rank
            }
        }
        // Log raw distance distribution for debugging
        if !dist_ranking.is_empty() {
            let raw_dists: Vec<f32> = dist_ranking.iter().map(|(_, d)| *d).collect();
            let n_raw = raw_dists.len();
            let p25 = raw_dists[n_raw / 4];
            let median = raw_dists[n_raw / 2];
            let p75 = raw_dists[n_raw * 3 / 4];
            eprintln!("[SUGGESTIONS] Raw distance distribution: min={:.4} p25={:.4} median={:.4} p75={:.4} max={:.4} (n={})",
                raw_dists[0], p25, median, p75, raw_dists[n_raw - 1], n_raw);
        }
        log::debug!("[SUGGESTIONS] Percentile-rank normalized {} candidate distances", candidates.len());
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

    // Step 4b: Stem energy — seed densities + batch candidate prefetch
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

    // Step 4d: Fetch intensity components and compute composite at query time.
    // Keep raw components for per-group tag generation alongside composite scores.
    let mut intensity_components_map: HashMap<(usize, i64), crate::db::IntensityComponents> = HashMap::new();
    let mut intensity_map: HashMap<(usize, i64), f32> = HashMap::new();
    for (src_idx, source) in sources.iter().enumerate() {
        let all_ids: Vec<i64> = candidates.keys()
            .filter(|(si, _)| *si == src_idx)
            .map(|(_, id)| *id)
            .collect();
        if all_ids.is_empty() { continue; }

        if let Ok(components) = source.db.batch_get_intensity_components(&all_ids) {
            for (id, ic) in components {
                intensity_map.insert((src_idx, id), composite_intensity_v2(&ic));
                intensity_components_map.insert((src_idx, id), ic);
            }
        }
    }
    // Also fetch seed components for intensity tags
    for &(src_idx, ref track) in &seed_tracks {
        if let Some(id) = track.id {
            if !intensity_components_map.contains_key(&(src_idx, id)) {
                if let Ok(components) = sources[src_idx].db.batch_get_intensity_components(&[id]) {
                    for (sid, ic) in components {
                        intensity_map.entry((src_idx, sid)).or_insert_with(|| composite_intensity_v2(&ic));
                        intensity_components_map.insert((src_idx, sid), ic);
                    }
                }
            }
        }
    }

    // Percentile-rank normalize each intensity component across all candidates + seeds.
    // Ensures every component contributes equally regardless of absolute scale.
    {
        let component_keys: Vec<(usize, i64)> = intensity_components_map.keys().copied().collect();
        if component_keys.len() >= 5 {
            macro_rules! rank_field {
                ($field:ident) => {{
                    let mut vals: Vec<((usize, i64), f32)> = component_keys.iter()
                        .filter_map(|k| intensity_components_map.get(k).map(|ic| (*k, ic.$field)))
                        .collect();
                    vals.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                    let n = vals.len().max(1) as f32;
                    let ranks: HashMap<(usize, i64), f32> = vals.iter().enumerate()
                        .map(|(rank, &(key, _))| (key, rank as f32 / (n - 1.0).max(1.0)))
                        .collect();
                    ranks
                }};
            }

            let r_flux = rank_field!(spectral_flux);
            let r_flat = rank_field!(flatness);
            let r_cent = rank_field!(spectral_centroid);
            let r_diss = rank_field!(dissonance);
            let r_crest = rank_field!(crest_factor);
            let r_evar = rank_field!(energy_variance);
            let r_harm = rank_field!(harmonic_complexity);
            let r_roll = rank_field!(spectral_rolloff);
            let r_cvar = rank_field!(centroid_variance);
            let r_fvar = rank_field!(flux_variance);

            for key in &component_keys {
                if let Some(ic) = intensity_components_map.get_mut(key) {
                    if let Some(&r) = r_flux.get(key) { ic.spectral_flux = r; }
                    if let Some(&r) = r_flat.get(key) { ic.flatness = r; }
                    if let Some(&r) = r_cent.get(key) { ic.spectral_centroid = r; }
                    if let Some(&r) = r_diss.get(key) { ic.dissonance = r; }
                    if let Some(&r) = r_crest.get(key) { ic.crest_factor = r; }
                    if let Some(&r) = r_evar.get(key) { ic.energy_variance = r; }
                    if let Some(&r) = r_harm.get(key) { ic.harmonic_complexity = r; }
                    if let Some(&r) = r_roll.get(key) { ic.spectral_rolloff = r; }
                    if let Some(&r) = r_cvar.get(key) { ic.centroid_variance = r; }
                    if let Some(&r) = r_fvar.get(key) { ic.flux_variance = r; }
                }
            }

            // Recompute composites from percentile-ranked values
            for key in &component_keys {
                if let Some(ic) = intensity_components_map.get(key) {
                    intensity_map.insert(*key, composite_intensity_v2(ic));
                }
            }
        }
    }

    let avg_seed_intensity = {
        let vals: Vec<f32> = seed_tracks
            .iter()
            .filter_map(|(idx, t)| t.id.map(|id| (*idx, id)))
            .filter_map(|key| intensity_map.get(&key).copied())
            .collect();
        if vals.is_empty() { 0.5 } else { vals.iter().sum::<f32>() / vals.len() as f32 }
    };

    // Diagnostic: intensity data coverage + distribution
    {
        let total_candidates = candidates.len();
        let with_intensity = candidates.keys()
            .filter(|k| intensity_map.contains_key(k))
            .count();
        let with_components = candidates.keys()
            .filter(|k| intensity_components_map.contains_key(k))
            .count();
        eprintln!("[INTENSITY] candidates: {} total, {} with composite, {} with raw components",
            total_candidates, with_intensity, with_components);
        eprintln!("[INTENSITY] seed avg_intensity={:.3}", avg_seed_intensity);

        // Log seed raw components
        for (idx, t) in &seed_tracks {
            if let Some(id) = t.id {
                if let Some(ic) = intensity_components_map.get(&(*idx, id)) {
                    eprintln!("[INTENSITY] seed '{}': flux={:.3} flat={:.3} cent={:.3} diss={:.3} crest={:.3} evar={:.3} harm={:.3} roll={:.3} cvar={:.3} fvar={:.3} → composite={:.3}",
                        t.title, ic.spectral_flux, ic.flatness, ic.spectral_centroid,
                        ic.dissonance, ic.crest_factor, ic.energy_variance,
                        ic.harmonic_complexity, ic.spectral_rolloff,
                        ic.centroid_variance, ic.flux_variance,
                        intensity_map.get(&(*idx, id)).copied().unwrap_or(-1.0));
                } else {
                    eprintln!("[INTENSITY] seed '{}': NO COMPONENTS", t.title);
                }
            }
        }

        // Distribution of composite values
        if with_intensity > 0 {
            let mut vals: Vec<f32> = intensity_map.values().copied().collect();
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n = vals.len();
            eprintln!("[INTENSITY] composite distribution: min={:.3} p25={:.3} median={:.3} p75={:.3} max={:.3}",
                vals[0], vals[n/4], vals[n/2], vals[n*3/4], vals[n-1]);
        }
    }

    // Average seed IntensityComponents for per-group tag deltas
    let avg_seed_ic: crate::db::IntensityComponents = {
        let seed_ics: Vec<&crate::db::IntensityComponents> = seed_tracks.iter()
            .filter_map(|(idx, t)| t.id.map(|id| (*idx, id)))
            .filter_map(|key| intensity_components_map.get(&key))
            .collect();
        if seed_ics.is_empty() {
            crate::db::IntensityComponents::default()
        } else {
            let n = seed_ics.len() as f32;
            crate::db::IntensityComponents {
                spectral_flux: seed_ics.iter().map(|ic| ic.spectral_flux).sum::<f32>() / n,
                flatness: seed_ics.iter().map(|ic| ic.flatness).sum::<f32>() / n,
                spectral_centroid: seed_ics.iter().map(|ic| ic.spectral_centroid).sum::<f32>() / n,
                dissonance: seed_ics.iter().map(|ic| ic.dissonance).sum::<f32>() / n,
                crest_factor: seed_ics.iter().map(|ic| ic.crest_factor).sum::<f32>() / n,
                energy_variance: seed_ics.iter().map(|ic| ic.energy_variance).sum::<f32>() / n,
                harmonic_complexity: seed_ics.iter().map(|ic| ic.harmonic_complexity).sum::<f32>() / n,
                spectral_rolloff: seed_ics.iter().map(|ic| ic.spectral_rolloff).sum::<f32>() / n,
                centroid_variance: seed_ics.iter().map(|ic| ic.centroid_variance).sum::<f32>() / n,
                flux_variance: seed_ics.iter().map(|ic| ic.flux_variance).sum::<f32>() / n,
            }
        }
    };

    // Percentile thresholds for intensity tag group deltas across candidates
    let intensity_group_percentiles: [(f32, f32); 4] = {
        let cand_ics: Vec<&crate::db::IntensityComponents> = candidates.keys()
            .filter_map(|key| intensity_components_map.get(key))
            .collect();
        if cand_ics.len() < 5 {
            [(f32::MIN, f32::MAX); 4] // too few → no tags
        } else {
            let mut result = [(0.0f32, 1.0f32); 4];
            for (i, group) in IntensityTagGroup::ALL.iter().enumerate() {
                let seed_val = group.value(&avg_seed_ic);
                let mut deltas: Vec<f32> = cand_ics.iter()
                    .map(|ic| group.value(ic) - seed_val)
                    .collect();
                deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let n = deltas.len();
                result[i] = (deltas[n / 5], deltas[n * 4 / 5]);
            }
            result
        }
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
    let (w_vector, w_key, w_intensity) = match suggestion_config.custom_weights {
        Some([ws, wk, wi]) => (ws, wk, wi),
        None => (0.25, 0.30, 0.30),
    };
    let w_coplay      = 0.07 * (1.0 - bias_abs);           // 0.07 center → 0.00 extreme
    // Stem penalty weights (only at center, subtracted from score)
    let w_vocal_pen = if suggestion_config.stem_complement { 0.08 * (1.0 - bias_abs) } else { 0.0 };
    let w_other_pen = if suggestion_config.stem_complement { 0.05 * (1.0 - bias_abs) } else { 0.0 };

    // Distances are already percentile-rank normalized to [0, 1]

    // Precompute transition reward as percentile-rank of deviation from target.
    // This replaces the Gaussian bell curve which had poor resolution near the peak.
    // Each track's |percentile_rank - target| is ranked, giving uniform [0, 1] spread.
    let target = suggestion_config.transition_target;
    let transition_rewards: HashMap<(usize, i64), f32> = {
        let mut deviations: Vec<((usize, i64), f32)> = candidates.iter()
            .map(|(&key, (_, dist))| (key, (*dist - target).abs()))
            .collect();
        deviations.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let n_cand = deviations.len().max(1) as f32;
        deviations.iter().enumerate()
            .map(|(rank, &(key, _))| (key, 1.0 - rank as f32 / (n_cand - 1.0).max(1.0)))
            .collect()
    };

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
                .and_then(|k| intensity_map.get(&k).copied())
                .unwrap_or(0.5);
            let int_reward = match suggestion_config.intensity_match_mode {
                super::config::IntensityMatchMode::Match => {
                    ml_key
                        .and_then(|k| intensity_components_map.get(&k))
                        .map(|cand_ic| intensity_reward_per_component(cand_ic, &avg_seed_ic, energy_bias, suggestion_config.blend_crossover, suggestion_config.intensity_reach))
                        .unwrap_or(0.5)
                }
                super::config::IntensityMatchMode::Auto => {
                    ml_key
                        .and_then(|k| intensity_components_map.get(&k))
                        .map(|cand_ic| intensity_reward_hybrid(cand_ic, &avg_seed_ic, cand_norm_intensity, avg_seed_intensity, energy_bias, suggestion_config.blend_crossover, suggestion_config.intensity_reach))
                        .unwrap_or(0.5)
                }
            };

            // ── Vector similarity reward ──
            // hnsw_dist is already percentile-rank normalized [0, 1]
            // Center: similarity → high reward (1 - percentile_rank).
            // Extremes: transition reward from deviation-rank (closest to target = best).
            let norm_dist = hnsw_dist; // already [0, 1] from percentile rank
            let similarity = 1.0 - norm_dist;
            let transition_reward = transition_rewards.get(&(src_idx, track_id)).copied().unwrap_or(0.5);
            let blend_t = (bias_abs / suggestion_config.blend_crossover).clamp(0.0, 1.0);
            let vec_reward = similarity * (1.0 - blend_t) + transition_reward * blend_t;

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

            let energy_delta = cand_norm_intensity - avg_seed_intensity;
            let mut reason_tags = generate_reason_tags(
                best_tt,
                similarity,
                energy_delta,
                vocal_comp, other_comp,
                w_vocal_pen, w_other_pen,
            );

            // Intensity component tags (outlier deltas from seed)
            if let Some(cand_ic) = intensity_components_map.get(&(src_idx, track_id)) {
                let int_tags = generate_intensity_tags(cand_ic, &avg_seed_ic, &intensity_group_percentiles);
                reason_tags.extend(int_tags);
            }

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

    // Diagnostic: top 10 suggestion breakdown with full component detail
    {
        let w = match suggestion_config.custom_weights {
            Some([ws, wk, wi]) => format!("S={:.2} K={:.2} I={:.2}", ws, wk, wi),
            None => "default".to_string(),
        };
        eprintln!("[SUGGESTIONS] Top results (weights: {}, energy_bias={:.2}):", w, energy_bias);
        for (i, s) in suggestions.iter().take(10).enumerate() {
            let id_key = s.track.id.map(|id| (0usize, id));
            let composite = id_key.and_then(|k| intensity_map.get(&k).copied());
            let ic = id_key.and_then(|k| intensity_components_map.get(&k));
            let ic_str = if let Some(ic) = ic {
                format!("flux={:.3} flat={:.3} cent={:.3} diss={:.3} crest={:.3} evar={:.3} harm={:.3} roll={:.3} cvar={:.3} fvar={:.3}",
                    ic.spectral_flux, ic.flatness, ic.spectral_centroid,
                    ic.dissonance, ic.crest_factor, ic.energy_variance,
                    ic.harmonic_complexity, ic.spectral_rolloff,
                    ic.centroid_variance, ic.flux_variance)
            } else {
                "NO DATA".to_string()
            };
            // Show per-component score breakdown
            let breakdown = if let Some(cs) = &s.component_scores {
                format!("vec={:.3} key={:.3} int={:.3} cop={:.3}",
                    cs.hnsw_component, cs.key_score, cs.intensity_penalty, cs.coplay_score)
            } else {
                // Recompute the intensity reward using the active mode for logging
                let int_rwd = id_key.and_then(|k| intensity_components_map.get(&k))
                    .map(|cand_ic| match suggestion_config.intensity_match_mode {
                        super::config::IntensityMatchMode::Match =>
                            intensity_reward_per_component(cand_ic, &avg_seed_ic, energy_bias, suggestion_config.blend_crossover, suggestion_config.intensity_reach),
                        super::config::IntensityMatchMode::Auto =>
                            intensity_reward_hybrid(cand_ic, &avg_seed_ic, composite.unwrap_or(0.5), avg_seed_intensity, energy_bias, suggestion_config.blend_crossover, suggestion_config.intensity_reach),
                    });
                format!("int_reward={:.3} mode={}", int_rwd.unwrap_or(-1.0), suggestion_config.intensity_match_mode.display_name())
            };
            eprintln!("[SUGGESTIONS] #{:>2} score={:.3} comp={:.3} {} | {} | ranked: {}",
                i + 1, s.score, composite.unwrap_or(-1.0), breakdown, s.track.title, ic_str);
        }
    }

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
        let intensity_map = source.db.batch_get_intensity_components(&ids).unwrap_or_default();

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

            let intensity = intensity_map.get(&id)
                .map(composite_intensity_v2)
                .unwrap_or(0.5);

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
    results.dedup_by(|a, b| {
        a.track.artist.as_deref().unwrap_or("").eq_ignore_ascii_case(
            b.track.artist.as_deref().unwrap_or("")
        ) && a.track.title.eq_ignore_ascii_case(&b.track.title)
    });
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

/// L2-normalize a vector in-place. No-op for near-zero vectors.
fn l2_normalize(v: &mut Vec<f32>) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-10 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine distance between two vectors: 1 - cosine_similarity.
/// Returns 0.0 for identical vectors, 2.0 for opposite vectors.
pub fn cosine_distance_pub(a: &[f32], b: &[f32]) -> f32 {
    cosine_distance(a, b)
}

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
