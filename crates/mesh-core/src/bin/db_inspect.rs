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
    // Two independent counts so we can spot wrapper-vs-raw mismatches:
    //   (a) `get_all_ml_embeddings` — what the PCA build actually consumes
    //   (b) raw Cozo scan — ground truth in the relation
    // If these disagree, the wrapper is filtering rows (was the case with
    // a stale `vec.len() != 1280` guard during the EffNet → MAEST swap).
    let ml = db.get_all_ml_embeddings().unwrap_or_default();
    println!("ml_embeddings:       {} rows (via get_all_ml_embeddings)", ml.len());
    if let Some((id, vec)) = ml.first() {
        println!("  sample id={id}  dims={}", vec.len());
    }

    // Raw scan + per-dim histogram via Cozo. Aggregations in CozoScript live
    // in the rule HEAD; `count(track_id)` and `length(vec)` here are bound
    // to head positions automatically.
    let cozo = db.db().inner();
    let raw_count = cozo.run_script_str(
        "?[count(track_id)] := *ml_embeddings{track_id}",
        "", false,
    );
    println!("  raw_count:        {}", short_json(&raw_count));
    // Round-7.7: also report 1024-d intensity-probe table population.
    let int_count = cozo.run_script_str(
        "?[count(track_id)] := *ml_intensity_embeddings{track_id}",
        "", false,
    );
    println!("  intensity_count:  {}", short_json(&int_count));
    let dim_hist = cozo.run_script_str(
        "?[dim, count(track_id)] := *ml_embeddings{track_id, vec}, dim = length(vec)",
        "", false,
    );
    println!("  dim_histogram:    {}", short_json(&dim_hist));
    let schema_cols = cozo.run_script_str("::columns ml_embeddings", "", false);
    println!("  declared_schema:  {}", short_json(&schema_cols));

    // Tracks that have an `ml_analysis` row (so reanalysis attempted them)
    // but no `ml_embeddings` row — these are the silent-skip casualties
    // worth re-running ML on. Limited to 20 paths to keep the output sane.
    let missing = cozo.run_script_str(
        r#"
        ?[track_id, title, duration_seconds] :=
            *tracks{id: track_id, title, duration_seconds},
            *ml_analysis{track_id},
            not *ml_embeddings{track_id}
        :order duration_seconds
        :limit 20
        "#,
        "", false,
    );
    let missing_count = cozo.run_script_str(
        r#"
        ?[count(track_id)] :=
            *tracks{id: track_id},
            *ml_analysis{track_id},
            not *ml_embeddings{track_id}
        "#,
        "", false,
    );
    println!("  missing_embed:    {} (sample below)", short_json(&missing_count));
    println!("                    {}", short_json(&missing));

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
            println!("  genre_scores: {} entries", ml.genre_scores.len());
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

/// Strip the verbose top-level JSON wrapper so each diagnostic line is one
/// readable row, e.g. `rows=[[2304, 854]]` instead of the full reply object.
fn short_json(reply: &str) -> String {
    use serde_json::Value;
    let v: Value = match serde_json::from_str(reply) {
        Ok(v) => v,
        Err(_) => return reply.trim().to_string(),
    };
    if v.get("ok") == Some(&Value::Bool(false)) {
        return format!("ERR: {}", v.get("message").and_then(|m| m.as_str()).unwrap_or(reply));
    }
    let headers = v.get("headers").and_then(|h| h.as_array()).cloned().unwrap_or_default();
    let rows = v.get("rows").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    let cols: Vec<String> = headers.iter().filter_map(|h| h.as_str().map(String::from)).collect();
    format!("cols={:?} rows={}", cols, Value::Array(rows))
}

fn default_collection_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
}
