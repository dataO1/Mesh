//! One-time migration: absolute-path track IDs → relative-path track IDs.
//!
//! Run with:
//!   cargo run -p mesh-core --bin migrate_track_ids -- [collection_root]
//!
//! If collection_root is omitted, uses ~/Music/mesh-collection.
//!
//! ALWAYS run this against a BACKUP first:
//!   cp mesh.db mesh.db.bak-YYYYMMDD

use std::path::PathBuf;

fn default_collection_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let collection_root = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        default_collection_root()
    };

    println!("mesh-core track ID migration");
    println!("Collection: {:?}", collection_root);
    println!();

    let db_path = collection_root.join("mesh.db");
    if !db_path.exists() {
        eprintln!("ERROR: mesh.db not found at {:?}", db_path);
        eprintln!("Pass the collection root as the first argument.");
        std::process::exit(1);
    }

    match mesh_core::migration::run_migration(&collection_root) {
        Ok((changed, skipped)) => {
            println!();
            println!("Done.");
            println!("  {:>6} tracks migrated to relative-path IDs", changed);
            println!("  {:>6} tracks already had stable IDs (unchanged)", skipped);
        }
        Err(e) => {
            eprintln!("Migration failed: {}", e);
            eprintln!();
            eprintln!("Restore from backup: cp mesh.db.bak-<timestamp> mesh.db");
            std::process::exit(1);
        }
    }
}
