//! Dump (track_id, path, drop_marker, frame_count, title, artist) to a CSV
//! so external Python tools (track-grading captioner) can iterate the library
//! without a Cozo Python driver.
//!
//! Usage:
//!   cargo run -p mesh-cue --release --bin dump_track_list -- \
//!     [--collection ~/Music/mesh-collection] \
//!     [--out /tmp/track-list.csv]
use std::path::PathBuf;
use std::io::Write;
use mesh_core::db::DatabaseService;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut collection = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
        .join("Music").join("mesh-collection");
    let mut out = PathBuf::from("/tmp/track-list.csv");
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--collection" | "-c" => { collection = PathBuf::from(&args[i+1]); i += 2; }
            "--out" | "-o" => { out = PathBuf::from(&args[i+1]); i += 2; }
            "--help" | "-h" => {
                eprintln!("usage: dump_track_list [--collection <path>] [--out <csv>]");
                std::process::exit(0);
            }
            other => { eprintln!("unknown arg: {other}"); std::process::exit(2); }
        }
    }

    let db = DatabaseService::new(&collection)
        .map_err(|e| anyhow::anyhow!("DB open: {e}"))?;
    let tracks = db.get_all_tracks().map_err(|e| anyhow::anyhow!("{e}"))?;
    eprintln!("[dump] {} tracks", tracks.len());

    if let Some(parent) = out.parent() { std::fs::create_dir_all(parent).ok(); }
    let mut f = std::fs::File::create(&out)?;
    writeln!(f, "track_id,path,drop_marker,frame_count,title,artist")?;
    let mut written = 0usize;
    for t in &tracks {
        let Some(id) = t.id else { continue };
        let drop_marker = t.drop_marker.map(|d| d.to_string()).unwrap_or_default();
        let title = csv_escape(&t.title);
        let artist = t.artist.as_deref().map(csv_escape).unwrap_or_default();
        let path_str = t.path.to_string_lossy();
        writeln!(f, "{},{},{},,{},{}", id, csv_escape(&path_str), drop_marker, title, artist)?;
        written += 1;
    }
    eprintln!("[dump] wrote {} rows → {}", written, out.display());
    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
