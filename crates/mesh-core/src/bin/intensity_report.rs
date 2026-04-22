//! Dump all tracks with intensity components, genre, and stem energy for analysis.
//!
//! Usage: cargo run -p mesh-core --bin intensity-report [-- /path/to/collection]
//! Output: TSV to stdout (pipe to file for spreadsheet analysis)

use mesh_core::db::DatabaseService;
use std::path::PathBuf;
use std::collections::HashMap;

fn main() {
    let collection_root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Music")
                .join("mesh-collection")
        });

    eprintln!("Opening: {}/mesh.db", collection_root.display());

    let db = match DatabaseService::new(&collection_root) {
        Ok(db) => db,
        Err(e) => { eprintln!("Failed: {e}"); std::process::exit(1); }
    };

    let tracks = db.get_all_tracks().unwrap_or_default();
    let ids: Vec<i64> = tracks.iter().filter_map(|t| t.id).collect();

    // Batch fetch all data
    let intensity_map = db.batch_get_intensity_components(&ids).unwrap_or_default();
    let ml_scores = db.get_ml_scores_batch(&ids).unwrap_or_default();
    let stem_map = db.batch_get_stem_energy(&ids).unwrap_or_default();

    // Collect all ML analysis for genre info
    let mut genre_map: HashMap<i64, (String, f32, Option<f32>, Option<f32>)> = HashMap::new();
    for &id in &ids {
        if let Ok(Some(ml)) = db.get_ml_analysis(id) {
            genre_map.insert(id, (
                ml.top_genre.unwrap_or_default(),
                ml.vocal_presence,
                ml.danceability,
                ml.timbre,
            ));
        }
    }

    // Compute composite for sorting
    let mut rows: Vec<(i64, String, Option<String>, f32)> = Vec::new();
    for track in &tracks {
        let Some(id) = track.id else { continue };
        let Some(ic) = intensity_map.get(&id) else { continue };
        let composite = mesh_core::suggestions::scoring::composite_intensity_v2(ic);
        rows.push((id, track.title.clone(), track.artist.clone(), composite));
    }
    rows.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));

    // Header
    println!("rank\tcomposite\tartist\ttitle\tgenre\tflux\tflatness\tcentroid\tdissonance\tcrest\tenergy_var\tharm_complex\trolloff\tcent_var\tflux_var\tvocal\tdanceability\ttimbre\tdrum_energy\tbass_energy");

    for (rank, (id, title, artist, composite)) in rows.iter().enumerate() {
        let ic = intensity_map.get(id).unwrap();
        let genre_info = genre_map.get(id);
        let genre = genre_info.map(|(g, _, _, _)| g.as_str()).unwrap_or("");
        let vocal = genre_info.map(|(_, v, _, _)| *v).unwrap_or(0.0);
        let dance = genre_info.and_then(|(_, _, d, _)| *d).unwrap_or(0.0);
        let timbre = genre_info.and_then(|(_, _, _, t)| *t).unwrap_or(0.0);
        let (_, drum, bass, _) = stem_map.get(id).copied().unwrap_or((0.0, 0.0, 0.0, 0.0));

        println!("{}\t{:.4}\t{}\t{}\t{}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.2}\t{:.2}\t{:.2}\t{:.4}\t{:.4}",
            rank + 1,
            composite,
            artist.as_deref().unwrap_or(""),
            title,
            genre,
            ic.spectral_flux,
            ic.flatness,
            ic.spectral_centroid,
            ic.dissonance,
            ic.crest_factor,
            ic.energy_variance,
            ic.harmonic_complexity,
            ic.spectral_rolloff,
            ic.centroid_variance,
            ic.flux_variance,
            vocal,
            dance,
            timbre,
            drum,
            bass,
        );
    }

    eprintln!("\n{} tracks with intensity data", rows.len());
}
