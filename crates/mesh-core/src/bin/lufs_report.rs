//! LUFS comparison report: drop loudness vs integrated loudness.
//!
//! Reads all tracks from the CozoDB at ~/Music/mesh-collection/mesh.db
//! and prints a table comparing the two LUFS measurements side-by-side.
//!
//! Usage: cargo run -p mesh-core --bin lufs-report

use mesh_core::db::DatabaseService;
use std::path::PathBuf;

fn main() {
    let collection_root = default_collection_root();
    eprintln!("Opening database at: {}/mesh.db", collection_root.display());

    let db = match DatabaseService::new(&collection_root) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Failed to open database: {}", e);
            std::process::exit(1);
        }
    };

    let tracks = match db.get_all_tracks() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Failed to query tracks: {:?}", e);
            std::process::exit(1);
        }
    };

    // Filter to tracks that have at least one LUFS value
    let mut rows: Vec<_> = tracks
        .iter()
        .filter(|t| t.lufs.is_some() || t.integrated_lufs.is_some())
        .collect();

    // Sort by delta (largest difference first) for easy spotting of problem tracks
    rows.sort_by(|a, b| {
        let delta_a = match (a.lufs, a.integrated_lufs) {
            (Some(d), Some(i)) => (d - i).abs(),
            _ => 0.0,
        };
        let delta_b = match (b.lufs, b.integrated_lufs) {
            (Some(d), Some(i)) => (d - i).abs(),
            _ => 0.0,
        };
        delta_b.partial_cmp(&delta_a).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Print header
    println!(
        "{:<50} {:>10} {:>10} {:>8}",
        "Track", "Drop LUFS", "Integ LUFS", "Delta"
    );
    println!("{}", "-".repeat(82));

    let mut count = 0;
    for t in &rows {
        let name = if t.name.len() > 48 {
            format!("{}…", &t.name[..47])
        } else {
            t.name.clone()
        };

        let drop_str = t.lufs.map(|v| format!("{:.1}", v)).unwrap_or_else(|| "—".into());
        let integ_str = t.integrated_lufs.map(|v| format!("{:.1}", v)).unwrap_or_else(|| "—".into());
        let delta_str = match (t.lufs, t.integrated_lufs) {
            (Some(d), Some(i)) => format!("{:+.1}", d - i),
            _ => "—".into(),
        };

        println!("{:<50} {:>10} {:>10} {:>8}", name, drop_str, integ_str, delta_str);
        count += 1;
    }

    println!("{}", "-".repeat(82));
    eprintln!("{} tracks with LUFS data ({} total in DB)", count, tracks.len());
}

fn default_collection_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
}
