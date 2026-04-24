//! Quick diagnostic tool — inspect DB relation row counts and sample data.
//!
//! Usage: cargo run -p mesh-core --bin db-inspect [-- /path/to/collection]

use mesh_core::db::DatabaseService;
use std::path::PathBuf;

fn main() {
    let collection_root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_collection_root);

    eprintln!("Opening: {}/mesh.db", collection_root.display());

    let db = match DatabaseService::new(&collection_root) {
        Ok(db) => db,
        Err(e) => { eprintln!("Failed: {e}"); std::process::exit(1); }
    };

    // --- tracks ---
    let tracks = db.get_all_tracks().unwrap_or_default();
    println!("tracks:              {} rows", tracks.len());

    // --- ml_embeddings ---
    let ml = db.get_all_ml_embeddings().unwrap_or_default();
    println!("ml_embeddings:       {} rows", ml.len());
    if let Some((id, vec)) = ml.first() {
        println!("  sample id={id}  dims={}", vec.len());
    }

    // --- ml_pca_embeddings ---
    let pca_count = tracks.iter()
        .filter_map(|t| t.id)
        .filter(|&id| db.get_pca_embedding_raw(id).ok().flatten().is_some())
        .count();
    println!("ml_pca_embeddings:   {} rows (sampled {} tracks)", pca_count, tracks.len());

    // --- ml_analysis ---
    let ml_analysis_count = tracks.iter()
        .filter_map(|t| t.id)
        .filter(|&id| db.get_ml_analysis(id).ok().flatten().is_some())
        .count();
    println!("ml_analysis:         {} rows", ml_analysis_count);

    // --- stem_energy ---
    let stem_count = tracks.iter()
        .filter_map(|t| t.id)
        .filter(|&id| db.get_stem_energy(id).ok().flatten().is_some())
        .count();
    println!("stem_energy:         {} rows", stem_count);

    // --- played_after ---
    let pa_total: usize = tracks.iter()
        .filter_map(|t| t.id)
        .take(50)
        .map(|id| db.get_played_after_neighbors(id, 100).map(|v| v.len()).unwrap_or(0))
        .sum();
    println!("played_after:        {} edges found (sampled first 50 tracks)", pa_total);

    // --- sample ml_analysis ---
    if let Some(id) = tracks.iter().filter_map(|t| t.id).find(|&id| {
        db.get_ml_analysis(id).ok().flatten().is_some()
    }) {
        if let Ok(Some(ml)) = db.get_ml_analysis(id) {
            println!("\nSample ml_analysis (track_id={id}):");
            println!("  top_genre:    {:?}", ml.top_genre);
            println!("  vocal:        {:?}", ml.vocal_presence);
            println!("  danceability: {:?}", ml.danceability);
            println!("  timbre:       {:?}", ml.timbre);
        }
    }

    // --- aggression calibration ---
    let pair_count = db.get_calibration_pair_count().unwrap_or(0);
    println!("calibration_pairs:   {} rows", pair_count);

    // Debug: check actual relation schema and raw data
    let cozo = db.db().inner();
    println!("  schema: {}", cozo.run_script_str("::columns aggression_calibration_pairs", "", false));
    println!("  raw: {}", cozo.run_script_str(
        "?[id, track_a, track_b, choice] := *aggression_calibration_pairs{id, track_a, track_b, choice} :limit 5", "", false,
    ));

    match db.get_aggression_weights() {
        Ok(Some((weights, correlation))) => {
            println!("aggression_axis:     {} dims, correlation={:.4}", weights.len(), correlation);
            let nonzero = weights.iter().filter(|w| w.abs() > 1e-6).count();
            let max_w = weights.iter().cloned().fold(0.0f32, f32::max);
            let min_w = weights.iter().cloned().fold(0.0f32, f32::min);
            println!("  nonzero_weights={nonzero}  range=[{min_w:.4}, {max_w:.4}]");
        }
        Ok(None) => println!("aggression_axis:     NOT COMPUTED"),
        Err(e) => println!("aggression_axis:     ERROR: {e}"),
    }

    // --- per-track embedding status ---
    println!("\nFirst 5 track IDs and their embedding status:");
    for track in tracks.iter().take(5) {
        if let Some(id) = track.id {
            let has_ml  = db.get_ml_embedding_raw(id).ok().flatten().is_some();
            let has_pca = db.get_pca_embedding_raw(id).ok().flatten().is_some();
            let has_ana = db.get_ml_analysis(id).ok().flatten().is_some();
            println!("  id={:>20}  ml_vec={}  pca_vec={}  ml_analysis={}  title={:?}",
                id, yn(has_ml), yn(has_pca), yn(has_ana), track.title);
        }
    }
}

fn yn(b: bool) -> &'static str { if b { "YES" } else { "no " } }

fn default_collection_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
}
