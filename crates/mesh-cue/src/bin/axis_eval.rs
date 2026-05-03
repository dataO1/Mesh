//! Project every track's MuQ-MuLan embedding onto an intensity-axis variant
//! and emit a ranked table.
//!
//! Reads:
//!   - the variant JSON (path supplied via --variant)
//!   - all rows of `ml_embeddings` from the local mesh DB
//!   - title/artist metadata from `tracks`
//!
//! Writes:
//!   - pretty table to stdout (descending by intensity score)
//!   - CSV to the path supplied via --csv (same ordering)
//!
//! CSV columns: rank, track_id, title, artist, intensity, then one column per
//! sub_axis name in the variant. Sub-axis count is variant-dependent.
//!
//! Usage:
//!   cargo run -p mesh-cue --bin axis_eval -- \
//!     --variant models/aggression-axes/V1_pure_aggression.json \
//!     --csv /tmp/axis-eval/V1.csv \
//!     [--limit 50] \
//!     [--collection ~/Music/mesh-collection]

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use mesh_core::db::DatabaseService;
use mesh_cue::ml_analysis::IntensityAxis;

struct Args {
    variant: PathBuf,
    csv: Option<PathBuf>,
    limit: Option<usize>,
    collection: PathBuf,
}

fn parse_args() -> Args {
    let mut variant: Option<PathBuf> = None;
    let mut csv: Option<PathBuf> = None;
    let mut limit: Option<usize> = None;
    let mut collection: Option<PathBuf> = None;

    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--variant" | "-v" => {
                variant = raw.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--csv" | "-o" => {
                csv = raw.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--limit" | "-l" => {
                limit = raw.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "--collection" | "-c" => {
                collection = raw.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--help" | "-h" => {
                eprintln!("usage: axis_eval --variant <json> [--csv <out>] [--limit N] [--collection <path>]");
                std::process::exit(0);
            }
            other => {
                eprintln!("axis_eval: unknown arg '{}'", other);
                std::process::exit(2);
            }
        }
    }

    let variant = variant.unwrap_or_else(|| {
        eprintln!("axis_eval: --variant <path> is required");
        std::process::exit(2);
    });
    let collection = collection.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Music")
            .join("mesh-collection")
    });
    Args { variant, csv, limit, collection }
}

fn main() {
    let args = parse_args();

    eprintln!("axis_eval: loading variant {:?}", args.variant);
    let axis = match IntensityAxis::load(&args.variant) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("axis_eval: failed to load variant: {}", e);
            std::process::exit(1);
        }
    };
    eprintln!(
        "axis_eval: variant '{}' ({}) — {} sub-axes, formula: {}",
        axis.variant_id, axis.name, axis.sub_axes.len(), axis.intensity_formula,
    );

    eprintln!("axis_eval: opening mesh DB at {:?}/mesh.db", args.collection);
    let db = match DatabaseService::new(&args.collection) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("axis_eval: DB open failed: {}", e);
            std::process::exit(1);
        }
    };

    eprintln!("axis_eval: loading ml_embeddings...");
    let embeddings = match db.get_all_ml_embeddings() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("axis_eval: get_all_ml_embeddings failed: {}", e);
            std::process::exit(1);
        }
    };
    eprintln!("axis_eval: {} embeddings loaded", embeddings.len());

    if embeddings.is_empty() {
        eprintln!("axis_eval: no embeddings — run reanalysis first");
        std::process::exit(0);
    }

    eprintln!("axis_eval: loading track metadata...");
    let tracks = match db.get_all_tracks() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("axis_eval: get_all_tracks failed: {}", e);
            std::process::exit(1);
        }
    };
    let track_meta: HashMap<i64, (String, String)> = tracks
        .into_iter()
        .filter_map(|t| {
            t.id.map(|id| {
                let artist = t.artist.unwrap_or_default();
                (id, (t.title, artist))
            })
        })
        .collect();

    // Project every track onto intensity + each sub-axis.
    eprintln!("axis_eval: projecting {} tracks...", embeddings.len());
    let mut rows: Vec<Row> = embeddings
        .iter()
        .map(|(id, emb)| {
            let intensity = axis.project(emb);
            let sub_scores: Vec<f32> = axis.sub_axes
                .iter()
                .map(|sub| {
                    emb.iter().zip(sub.axis_vec.iter()).map(|(a, b)| a * b).sum::<f32>()
                })
                .collect();
            let (title, artist) = track_meta
                .get(id)
                .cloned()
                .unwrap_or_else(|| ("<unknown>".to_string(), String::new()));
            Row { track_id: *id, title, artist, intensity, sub_scores }
        })
        .collect();

    rows.sort_by(|a, b| b.intensity.partial_cmp(&a.intensity).unwrap_or(std::cmp::Ordering::Equal));

    // Distribution stats.
    if !rows.is_empty() {
        let scores: Vec<f32> = rows.iter().map(|r| r.intensity).collect();
        let min = scores.iter().copied().fold(f32::INFINITY, f32::min);
        let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mean = scores.iter().sum::<f32>() / scores.len() as f32;
        let mut sorted = scores.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted[sorted.len() / 2];
        let p25 = sorted[sorted.len() / 4];
        let p75 = sorted[(sorted.len() * 3) / 4];
        eprintln!(
            "axis_eval: intensity distribution — min {:+.3} | p25 {:+.3} | med {:+.3} | mean {:+.3} | p75 {:+.3} | max {:+.3}",
            min, p25, median, mean, p75, max,
        );
    }

    // CSV.
    if let Some(csv_path) = &args.csv {
        if let Some(parent) = csv_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match File::create(csv_path) {
            Ok(f) => {
                let mut w = BufWriter::new(f);
                let mut header = String::from("rank,track_id,title,artist,intensity");
                for sub in &axis.sub_axes {
                    header.push(',');
                    header.push_str(&sub.name);
                }
                writeln!(w, "{}", header).ok();
                for (rank, row) in rows.iter().enumerate() {
                    let mut line = format!(
                        "{},{},{},{},{:.6}",
                        rank + 1,
                        row.track_id,
                        csv_escape(&row.title),
                        csv_escape(&row.artist),
                        row.intensity,
                    );
                    for s in &row.sub_scores {
                        line.push(',');
                        line.push_str(&format!("{:.6}", s));
                    }
                    writeln!(w, "{}", line).ok();
                }
                eprintln!("axis_eval: wrote {} rows → {:?}", rows.len(), csv_path);
            }
            Err(e) => eprintln!("axis_eval: CSV write failed: {}", e),
        }
    }

    // Pretty stdout table (head + tail to keep it readable).
    let limit = args.limit.unwrap_or(20);
    println!();
    println!("════════════════════════════════════════════════════════════════════════");
    println!("  variant: {} — {}", axis.variant_id, axis.name);
    println!("  formula: {}", axis.intensity_formula);
    println!("════════════════════════════════════════════════════════════════════════");

    let mut head_label = format!("rank | id    | intensity | {:<40} | {:<24}", "title", "artist");
    for sub in &axis.sub_axes {
        head_label.push_str(&format!(" | {:<10}", sub.name));
    }
    println!("{}", head_label);
    println!("{}", "-".repeat(head_label.chars().count().min(220)));

    let print_row = |r: &Row, rank: usize| {
        let mut line = format!(
            "{:>4} | {:>5} | {:+.4}  | {:<40} | {:<24}",
            rank,
            r.track_id,
            r.intensity,
            truncate(&r.title, 40),
            truncate(&r.artist, 24),
        );
        for s in &r.sub_scores {
            line.push_str(&format!(" | {:+.4}", s));
        }
        println!("{}", line);
    };

    println!("  TOP {} most-intense:", limit);
    for (rank, row) in rows.iter().take(limit).enumerate() {
        print_row(row, rank + 1);
    }
    if rows.len() > 2 * limit {
        println!();
        println!("  BOTTOM {} least-intense:", limit);
        let total = rows.len();
        for (i, row) in rows.iter().rev().take(limit).enumerate() {
            let rank = total - i;
            print_row(row, rank);
        }
    }
    println!();
}

struct Row {
    track_id: i64,
    title: String,
    artist: String,
    intensity: f32,
    sub_scores: Vec<f32>,
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        s.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
