//! Headless smoke test for the suggestion engine's intensity behaviour.
//!
//! Opens the local collection, picks a seed track (by title substring or,
//! by default, the track closest to median stored intensity), then runs
//! `query_suggestions` at energy slider centre / full peak / full drop and
//! prints the top results with their stored intensity scalars + pooled
//! percentiles. Use after any scoring or intensity-pipeline change to
//! eyeball that the slider actually steers intensity.
//!
//! Usage: cargo run -p mesh-core --bin suggestions-smoke -- \
//!   [/path/to/collection] [--seed <title substring>]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use mesh_core::db::DatabaseService;
use mesh_core::suggestions::config::{
    KeyScoringModel, SuggestionBlendMode, SuggestionKeyFilter, SuggestionTransitionReach,
};
use mesh_core::suggestions::query::{query_suggestions, DbSource, SuggestionConfig};

fn main() {
    let mut collection: Option<PathBuf> = None;
    let mut seed_filter: Option<String> = None;
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--seed" => {
                seed_filter = raw.get(i + 1).cloned();
                i += 2;
            }
            other => {
                collection = Some(PathBuf::from(other));
                i += 1;
            }
        }
    }
    let collection_root = collection.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Music")
            .join("mesh-collection")
    });

    let db = match DatabaseService::new(&collection_root) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Failed to open {:?}: {e}", collection_root);
            std::process::exit(1);
        }
    };

    // Pooled percentile map over stored scalars (mirrors query_suggestions).
    let mut scores = db.get_all_intensity_scores().unwrap_or_default();
    if scores.is_empty() {
        eprintln!("No intensity_score rows — run mesh-cue or reanalyze_ml first.");
        std::process::exit(1);
    }
    scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let n = scores.len() as f32;
    let percentile: HashMap<i64, f32> = scores
        .iter()
        .enumerate()
        .map(|(rank, (id, _, _))| (*id, rank as f32 / (n - 1.0).max(1.0)))
        .collect();
    let scalar: HashMap<i64, f32> = scores.iter().map(|(id, s, _)| (*id, *s)).collect();

    // Seed: --seed substring match, else the median-intensity track with a key.
    let tracks = db.get_all_tracks().unwrap_or_default();
    let seed = match &seed_filter {
        Some(sub) => {
            let sub = sub.to_lowercase();
            tracks
                .iter()
                .find(|t| t.title.to_lowercase().contains(&sub))
                .unwrap_or_else(|| {
                    eprintln!("No track title contains '{sub}'");
                    std::process::exit(1);
                })
        }
        None => {
            let median_id = scores[scores.len() / 2].0;
            tracks
                .iter()
                .find(|t| t.id == Some(median_id) && t.key.is_some())
                .or_else(|| tracks.iter().find(|t| t.key.is_some()))
                .expect("collection has no tracks with a key")
        }
    };
    let seed_id = seed.id.unwrap_or(0);
    println!(
        "seed: '{}' (key={:?}, intensity={:.3}, percentile={:.3})",
        seed.title,
        seed.key.as_deref().unwrap_or("?"),
        scalar.get(&seed_id).copied().unwrap_or(-1.0),
        percentile.get(&seed_id).copied().unwrap_or(-1.0),
    );

    let sources = vec![DbSource {
        db: db.clone(),
        collection_root: collection_root.clone(),
        name: "Local".to_string(),
    }];
    let config = SuggestionConfig::from_display(
        SuggestionBlendMode::Balanced,
        SuggestionKeyFilter::Strict,
        false,
        SuggestionTransitionReach::Medium,
        None,
    );

    for (label, energy_direction) in [("CENTRE", 0.5f32), ("FULL PEAK", 1.0), ("FULL DROP", 0.0)] {
        println!("\n════ {label} (energy_direction={energy_direction:.1}) ════");
        let results = query_suggestions(
            &sources,
            vec![seed.path.to_string_lossy().to_string()],
            energy_direction,
            KeyScoringModel::Krumhansl,
            config,
            10_000,
            10,
            &HashSet::new(),
            None,
            None,
            None,
            true,
        );
        match results {
            Ok(list) => {
                for (rank, s) in list.iter().enumerate() {
                    let id = s.track.id.unwrap_or(0);
                    println!(
                        "  #{:>2} score={:.3} intensity={:>6.3} pct={:>6.3} key={:<4} {}",
                        rank + 1,
                        s.score,
                        scalar.get(&id).copied().unwrap_or(-1.0),
                        percentile.get(&id).copied().unwrap_or(-1.0),
                        s.track.key.as_deref().unwrap_or("?"),
                        s.track.title,
                    );
                }
            }
            Err(e) => eprintln!("query failed: {e}"),
        }
    }
}
