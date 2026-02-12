//! Smart suggestion engine for the collection browser
//!
//! Queries the CozoDB HNSW index to find tracks similar to the currently
//! loaded deck seeds, then re-scores them according to the selected mode.

use std::collections::HashMap;
use mesh_core::db::{DatabaseService, Track};
use mesh_core::music::MusicalKey;

use crate::config::SuggestionMode;

/// A suggested track with its computed score (lower = better match)
#[derive(Debug, Clone)]
pub struct SuggestedTrack {
    pub track: Track,
    pub score: f32,
}

/// Query the database for track suggestions based on loaded deck seeds.
///
/// This runs on a background thread via `Task::perform()`.
///
/// # Algorithm
/// 1. Resolve each seed path to a track ID
/// 2. For each seed, find similar tracks via HNSW index
/// 3. Merge results keeping the best (minimum) distance per candidate
/// 4. Re-score according to the selected mode
/// 5. Sort and return the top results
pub fn query_suggestions(
    db: &DatabaseService,
    seed_paths: Vec<String>,
    mode: SuggestionMode,
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

    // Step 4: Re-score according to mode
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

    let mut suggestions: Vec<SuggestedTrack> = candidates
        .into_values()
        .filter_map(|(track, hnsw_dist)| {
            let score = match mode {
                SuggestionMode::Similar => hnsw_dist,

                SuggestionMode::HarmonicMix => {
                    let key_score = track
                        .key
                        .as_deref()
                        .and_then(|k| {
                            let cand_key = MusicalKey::parse(k)?;
                            let best = seed_keys
                                .iter()
                                .map(|sk| harmonic_score(sk, &cand_key))
                                .fold(0.0f32, f32::max);
                            Some(best)
                        })
                        .unwrap_or(0.0);

                    // Filter out poor harmonic matches
                    if key_score < 0.5 {
                        return None;
                    }
                    // Invert so lower = better: use (1 - key_score) + small hnsw factor
                    (1.0 - key_score) + 0.2 * hnsw_dist
                }

                SuggestionMode::EnergyMatch => {
                    let energy_diff = track
                        .lufs
                        .map(|l| ((l - avg_seed_lufs).abs() / 40.0).min(1.0))
                        .unwrap_or(0.5);
                    0.4 * hnsw_dist + 0.6 * energy_diff
                }

                SuggestionMode::Combined => {
                    let key_penalty = track
                        .key
                        .as_deref()
                        .and_then(|k| {
                            let cand_key = MusicalKey::parse(k)?;
                            let best = seed_keys
                                .iter()
                                .map(|sk| harmonic_score(sk, &cand_key))
                                .fold(0.0f32, f32::max);
                            Some(1.0 - best)
                        })
                        .unwrap_or(0.5);

                    let bpm_penalty = track
                        .bpm
                        .map(|b| {
                            let diff = (b - avg_seed_bpm).abs();
                            // Normalize: 10 BPM diff → 1.0 penalty
                            (diff / 10.0).min(1.0) as f32
                        })
                        .unwrap_or(0.5);

                    0.5 * hnsw_dist + 0.3 * key_penalty + 0.2 * bpm_penalty
                }
            };
            Some(SuggestedTrack { track, score })
        })
        .collect();

    // Step 5: Sort ascending (lower score = better match) and limit
    suggestions.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
    suggestions.truncate(total_limit);

    Ok(suggestions)
}

/// Score harmonic compatibility between two keys using the Camelot wheel.
///
/// Returns a value from 0.0 (incompatible) to 1.0 (perfect match):
/// - Same position + same letter → 1.0
/// - ±1 position, same letter (adjacent on wheel) → 0.9
/// - Same position, different letter (relative major/minor) → 0.85
/// - ±2 positions → 0.6
/// - Otherwise → 0.0
fn harmonic_score(key1: &MusicalKey, key2: &MusicalKey) -> f32 {
    let (pos1, letter1) = key1.camelot();
    let (pos2, letter2) = key2.camelot();

    let same_letter = letter1 == letter2;

    // Circular distance on the 12-position Camelot wheel
    let diff = {
        let d = (pos1 as i8 - pos2 as i8).unsigned_abs();
        d.min(12 - d)
    };

    match (diff, same_letter) {
        (0, true) => 1.0,    // Exact same key
        (0, false) => 0.85,  // Relative major/minor
        (1, true) => 0.9,    // Adjacent on wheel, same mode
        (1, false) => 0.6,   // Adjacent, different mode
        (2, _) => 0.6,       // Two steps away
        _ => 0.0,            // Too far apart
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_harmonic_score_same_key() {
        let am = MusicalKey::parse("Am").unwrap();
        assert_eq!(harmonic_score(&am, &am), 1.0);
    }

    #[test]
    fn test_harmonic_score_relative() {
        // Am and C major are relative (same Camelot position, different letter)
        let am = MusicalKey::parse("Am").unwrap();
        let c = MusicalKey::parse("C").unwrap();
        assert_eq!(harmonic_score(&am, &c), 0.85);
    }

    #[test]
    fn test_harmonic_score_adjacent() {
        // Am (8A) and Em (9A) are adjacent on Camelot wheel
        let am = MusicalKey::parse("Am").unwrap();
        let em = MusicalKey::parse("Em").unwrap();
        assert_eq!(harmonic_score(&am, &em), 0.9);
    }

    #[test]
    fn test_harmonic_score_incompatible() {
        // Am (8A) and F#m (11A) are 3 steps apart
        let am = MusicalKey::parse("Am").unwrap();
        let fsm = MusicalKey::parse("F#m").unwrap();
        assert_eq!(harmonic_score(&am, &fsm), 0.0);
    }
}
