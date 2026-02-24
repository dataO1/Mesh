//! Export track analysis data from the mesh database to JSON.
//!
//! Reads all tracks from the CozoDB at ~/Music/mesh-collection/mesh.db
//! and writes a JSON file with BPM, key, beat grid stats, etc.
//!
//! Usage: cargo run -p mesh-core --bin export-analysis [-- OUTPUT_PATH]

use mesh_core::db::DatabaseService;
use std::path::PathBuf;

fn main() {
    // Parse output path from args (default: analysis-export.json)
    let output_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "analysis-export.json".to_string());

    // Open the database
    let collection_root = default_collection_root();
    eprintln!("Opening database at: {}/mesh.db", collection_root.display());

    let db = match DatabaseService::new(&collection_root) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Failed to open database: {}", e);
            std::process::exit(1);
        }
    };

    // Query all tracks
    let tracks = match db.get_all_tracks() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Failed to query tracks: {:?}", e);
            std::process::exit(1);
        }
    };

    eprintln!("Found {} tracks", tracks.len());

    // Build JSON array
    let entries: Vec<serde_json::Value> = tracks
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.title,
                "artist": t.artist,
                "path": t.path.to_string_lossy(),
                "bpm": t.bpm,
                "original_bpm": t.original_bpm,
                "key": t.key,
                "duration_seconds": t.duration_seconds,
                "lufs": t.lufs,
                "first_beat_sample": t.first_beat_sample,
            })
        })
        .collect();

    let output = serde_json::json!({
        "export_date": chrono_now(),
        "track_count": entries.len(),
        "tracks": entries,
    });

    // Write to file
    let json_str = serde_json::to_string_pretty(&output).expect("JSON serialization failed");
    std::fs::write(&output_path, &json_str).unwrap_or_else(|e| {
        eprintln!("Failed to write {}: {}", output_path, e);
        std::process::exit(1);
    });

    eprintln!("Exported {} tracks to {}", entries.len(), output_path);
}

fn default_collection_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
}

fn chrono_now() -> String {
    // Simple ISO-8601-ish timestamp without pulling in chrono
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("unix:{}", duration.as_secs())
}
