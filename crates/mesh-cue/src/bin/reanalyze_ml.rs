//! Headless ML re-embedding for the entire local collection.
//!
//! Iterates every track in the local mesh DB, decodes audio to native rate,
//! computes mel + runs MuQ-MuLan with the current MUQ_MULAN_MAX_CLIPS setting,
//! stores the new 512-d embedding back into ml_embeddings.
//!
//! Skips track-metadata / loudness / key / drop-marker / etc. — only ML.
//! Used for fast turnaround when MUQ_MULAN_MAX_CLIPS changes.
//!
//! Usage:
//!   cargo run -p mesh-cue --release --bin reanalyze_ml -- \
//!     [--collection ~/Music/mesh-collection] \
//!     [--limit N] \
//!     [--only-missing]
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Context;
use rayon::prelude::*;

use mesh_core::audio_file::AudioFileReader;
use mesh_core::db::DatabaseService;
use mesh_cue::ml_analysis;

struct Args {
    collection: PathBuf,
    limit: Option<usize>,
    only_missing: bool,
    only_ids: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut collection: Option<PathBuf> = None;
    let mut limit: Option<usize> = None;
    let mut only_missing = false;
    let mut only_ids: Option<PathBuf> = None;
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--collection" | "-c" => { collection = raw.get(i+1).map(PathBuf::from); i += 2; }
            "--limit" | "-l" => { limit = raw.get(i+1).and_then(|s| s.parse().ok()); i += 2; }
            "--only-missing" => { only_missing = true; i += 1; }
            "--only-ids" => { only_ids = raw.get(i+1).map(PathBuf::from); i += 2; }
            "--help" | "-h" => {
                eprintln!("usage: reanalyze_ml [--collection <path>] [--limit N] [--only-missing] [--only-ids <file>]");
                eprintln!("  --only-ids: file with one track_id per line (or first column of CSV with header)");
                std::process::exit(0);
            }
            other => { eprintln!("reanalyze_ml: unknown arg '{}'", other); std::process::exit(2); }
        }
    }
    Args {
        collection: collection.unwrap_or_else(|| {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
                .join("Music").join("mesh-collection")
        }),
        limit,
        only_missing,
        only_ids,
    }
}

fn create_mono_mix(stems: &mesh_core::audio_file::StemBuffers) -> Vec<f32> {
    let len = stems.len();
    let mut mono = Vec::with_capacity(len);
    for i in 0..len {
        let v = (stems.vocals[i].left + stems.vocals[i].right) * 0.5;
        let d = (stems.drums[i].left + stems.drums[i].right) * 0.5;
        let b = (stems.bass[i].left + stems.bass[i].right) * 0.5;
        let o = (stems.other[i].left + stems.other[i].right) * 0.5;
        mono.push(v + d + b + o);
    }
    mono
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = parse_args();

    eprintln!("[reanalyze_ml] DB at {}/mesh.db", args.collection.display());
    let db = DatabaseService::new(&args.collection)
        .map_err(|e| anyhow::anyhow!("DB open failed: {}", e))?;

    let model_dir = ml_analysis::ensure_ml_model_dir(|_, _, _| {})
        .ok_or_else(|| anyhow::anyhow!("MuQ-MuLan model dir not found"))?;
    eprintln!("[reanalyze_ml] model_dir: {}", model_dir.display());

    let tracks = db.get_all_tracks()
        .map_err(|e| anyhow::anyhow!("get_all_tracks failed: {}", e))?;
    eprintln!("[reanalyze_ml] {} tracks total", tracks.len());

    let already_embedded: HashSet<i64> = if args.only_missing {
        let raw = db.get_all_ml_embeddings()
            .map_err(|e| anyhow::anyhow!("get_all_ml_embeddings: {}", e))?;
        raw.iter().map(|(id, _)| *id).collect()
    } else { HashSet::new() };

    let only_ids: Option<HashSet<i64>> = args.only_ids.as_ref().map(|p| {
        let raw = std::fs::read_to_string(p).expect("read --only-ids file");
        raw.lines()
            .filter_map(|l| l.split(',').next())
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .collect()
    });

    let mut work: Vec<_> = tracks.into_iter()
        .filter_map(|t| t.id.map(|id| (id, t.path)))
        .filter(|(id, _)| !args.only_missing || !already_embedded.contains(id))
        .filter(|(id, _)| only_ids.as_ref().map_or(true, |s| s.contains(id)))
        .collect();
    if let Some(n) = args.limit { work.truncate(n); }
    let total = work.len();
    eprintln!("[reanalyze_ml] {} tracks to (re-)embed", total);

    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let start = std::time::Instant::now();

    work.par_iter().for_each(|(track_id, path)| {
        let path = std::path::Path::new(path);
        let result: anyhow::Result<()> = (|| {
            let reader = AudioFileReader::open(path)
                .with_context(|| format!("open {}", path.display()))?;
            let stems = reader.read_all_stems()
                .with_context(|| format!("read_all_stems {}", path.display()))?;
            let mono = create_mono_mix(&stems);
            let mel = ml_analysis::preprocessing::compute_mel_spectrogram(
                &mono, mesh_core::types::SAMPLE_RATE as f32,
            ).map_err(|e| anyhow::anyhow!("mel: {}", e))?;
            let ml = ml_analysis::with_thread_local_analyzer(&model_dir, |a| a.analyze(&mel))
                .map_err(|e| anyhow::anyhow!("analyzer: {}", e))?
                .map_err(|e| anyhow::anyhow!("analyze: {}", e))?;
            if ml.embedding.len() != ml_analysis::MUQ_MULAN_EMBEDDING_DIM {
                anyhow::bail!("wrong embedding dim {}", ml.embedding.len());
            }
            db.store_ml_embedding(*track_id, &ml.embedding)
                .map_err(|e| anyhow::anyhow!("store: {}", e))?;
            Ok(())
        })();
        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
        match result {
            Ok(()) => {
                if n % 10 == 0 || n == total {
                    let elapsed = start.elapsed().as_secs_f32();
                    let rate = n as f32 / elapsed.max(0.001);
                    let eta = (total - n) as f32 / rate.max(0.001);
                    eprintln!(
                        "[reanalyze_ml] {}/{} done ({:.1}/s, eta {:.0}s, failed {})",
                        n, total, rate, eta, failed.load(Ordering::Relaxed),
                    );
                }
            }
            Err(e) => {
                failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("[reanalyze_ml] FAILED {}: {}", path.display(), e);
            }
        }
    });

    let elapsed = start.elapsed().as_secs_f32();
    eprintln!(
        "[reanalyze_ml] complete: {} succeeded, {} failed, {:.0}s total ({:.2}/s)",
        total - failed.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
        elapsed,
        total as f32 / elapsed.max(0.001),
    );
    Ok(())
}
