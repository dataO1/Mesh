//! Inspect the learned aggression scale.
//!
//! Reads the calibrated PCA aggression weight vector from the local mesh DB
//! and reports:
//!   - whether weights are stored, with the calibration correlation
//!   - the weight vector itself (per-dimension, sorted by absolute magnitude)
//!   - which dimensions carry meaningful signal vs. which are effectively zero
//!   - calibration pair count
//!   - distribution of projected scores across the library (min / quartiles / max)
//!   - the most and least aggressive tracks under the current scale
//!   - a sanity-check against a few known artists
//!
//! Usage: cargo run -p mesh-core --bin aggression_inspect [-- /path/to/collection]

use mesh_core::db::DatabaseService;
use mesh_core::suggestions::aggression::project_aggression;
use std::path::PathBuf;

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

    // ── 1. Load the learned weights ─────────────────────────────────────────
    let (weights, correlation) = match db.get_aggression_weights() {
        Ok(Some((w, r))) => (w, r),
        Ok(None) => {
            eprintln!("\n  ❌  No aggression scale stored. Run calibration first.");
            std::process::exit(0);
        }
        Err(e) => { eprintln!("DB error: {e}"); std::process::exit(1); }
    };

    println!();
    println!("════════════════════════════════════════════════════════════════════════");
    println!("  LEARNED AGGRESSION SCALE");
    println!("════════════════════════════════════════════════════════════════════════");
    println!("  dimensions:           {}", weights.len());
    println!("  calibration r:        {:+.4}", correlation);
    println!("  pair count:           {}", db.get_calibration_pair_count().unwrap_or(0));

    let l2: f32 = weights.iter().map(|w| w * w).sum::<f32>().sqrt();
    let l1: f32 = weights.iter().map(|w| w.abs()).sum();
    let max_abs = weights.iter().cloned().fold(0.0_f32, |a, b| a.max(b.abs()));
    let nonzero = weights.iter().filter(|w| w.abs() > 1e-6).count();
    let near_zero = weights.iter().filter(|w| w.abs() < 0.01).count();
    println!("  L2 norm:              {:.4}", l2);
    println!("  L1 norm:              {:.4}", l1);
    println!("  max |weight|:         {:.4}", max_abs);
    println!("  nonzero (|w| > 1e-6): {} / {}", nonzero, weights.len());
    println!("  near-zero (|w|<0.01): {} / {}", near_zero, weights.len());

    // ── 2. Per-dimension breakdown sorted by |weight| ──────────────────────
    let mut indexed: Vec<(usize, f32)> = weights.iter().enumerate().map(|(i, &w)| (i, w)).collect();
    indexed.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));

    let cumulative_l1 = indexed.iter().map(|(_, w)| w.abs()).sum::<f32>().max(1e-9);

    println!();
    println!("  ── ALL DIMENSIONS sorted by |weight| ────────────────────────────────");
    println!("  rank  dim    weight     |w|/L1      cum%     direction");
    let mut cum = 0.0_f32;
    for (rank, (dim, w)) in indexed.iter().enumerate() {
        cum += w.abs() / cumulative_l1;
        let dir = if *w > 0.0 { "→ more aggressive" } else { "→ less aggressive" };
        println!(
            "  {:>4}  {:>3}  {:+8.4}  {:>7.2}%  {:>6.2}%  {}",
            rank + 1,
            dim,
            w,
            100.0 * w.abs() / cumulative_l1,
            100.0 * cum,
            dir,
        );
        if cum > 0.95 && rank + 1 < weights.len() {
            let remaining = weights.len() - rank - 1;
            println!("  …  ({} dimensions carry the remaining {:.1}% of L1 weight)",
                remaining, 100.0 * (1.0 - cum));
            break;
        }
    }

    // ── 3. How concentrated is the model? ──────────────────────────────────
    let top1_share = indexed[0].1.abs() / cumulative_l1;
    let top5_share: f32 = indexed.iter().take(5).map(|(_, w)| w.abs()).sum::<f32>() / cumulative_l1;
    let top10_share: f32 = indexed.iter().take(10).map(|(_, w)| w.abs()).sum::<f32>() / cumulative_l1;
    println!();
    println!("  ── CONCENTRATION ────────────────────────────────────────────────────");
    println!("  top-1  dim carries {:.1}% of total |weight|", 100.0 * top1_share);
    println!("  top-5  dims carry  {:.1}%", 100.0 * top5_share);
    println!("  top-10 dims carry  {:.1}%", 100.0 * top10_share);

    // ── 4. Project across the library and report distribution ──────────────
    let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
    let n_tracks = all_pca.len();
    if n_tracks == 0 {
        eprintln!("\n  No tracks with PCA embeddings — cannot report distribution.");
        return;
    }

    let mut scored: Vec<(f32, &mesh_core::db::Track)> = all_pca.iter()
        .filter_map(|(t, vec)| {
            if vec.len() != weights.len() { return None; }
            Some((project_aggression(vec, &weights), t))
        })
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let n = scored.len();
    let pct = |q: f32| -> f32 {
        let i = ((n as f32 - 1.0) * q).round() as usize;
        scored[i].0
    };

    println!();
    println!("════════════════════════════════════════════════════════════════════════");
    println!("  PROJECTED SCORE DISTRIBUTION ACROSS {} TRACKS", n);
    println!("════════════════════════════════════════════════════════════════════════");
    println!("  min:      {:+8.4}", scored[0].0);
    println!("  p10:      {:+8.4}", pct(0.10));
    println!("  p25:      {:+8.4}", pct(0.25));
    println!("  median:   {:+8.4}", pct(0.50));
    println!("  p75:      {:+8.4}", pct(0.75));
    println!("  p90:      {:+8.4}", pct(0.90));
    println!("  max:      {:+8.4}", scored[n - 1].0);
    let mean = scored.iter().map(|(s, _)| s).sum::<f32>() / n as f32;
    let var = scored.iter().map(|(s, _)| (s - mean).powi(2)).sum::<f32>() / n as f32;
    println!("  mean ± σ: {:+8.4} ± {:.4}", mean, var.sqrt());

    // ── 5. Tails — what does the scale think is least and most aggressive? ─
    let head_n = 15.min(n);
    println!();
    println!("  ── LEAST aggressive (bottom {}) ─────────────────────────────────────", head_n);
    for (s, t) in scored.iter().take(head_n) {
        println!("  {:+8.4}  {:25}  {}",
            s,
            t.artist.as_deref().unwrap_or("?").chars().take(25).collect::<String>(),
            t.title);
    }

    println!();
    println!("  ── MOST aggressive (top {}) ─────────────────────────────────────────", head_n);
    for (s, t) in scored.iter().rev().take(head_n) {
        println!("  {:+8.4}  {:25}  {}",
            s,
            t.artist.as_deref().unwrap_or("?").chars().take(25).collect::<String>(),
            t.title);
    }

    // ── 6. Sanity-check against known artists ──────────────────────────────
    let known_aggressive = [
        "Current Value", "Billain", "Neonlight", "Mefjus", "Phace",
        "Noisia", "Black Sun Empire", "Audio", "Teddy Killerz",
    ];
    let known_liquid = [
        "Random Movement", "Calibre", "LSB", "Logistics", "BCee",
        "Etherwood", "Marcus Intalex",
    ];
    let percentile = |idx: usize| 100.0 * idx as f32 / (n as f32 - 1.0);

    println!();
    println!("  ── SANITY CHECK: known artists vs. percentile rank ──────────────────");
    println!("  Aggressive artists should rank HIGH; liquid artists should rank LOW.\n");
    println!("  ── Aggressive ──");
    for (idx, (s, t)) in scored.iter().enumerate() {
        let artist = t.artist.as_deref().unwrap_or("");
        if known_aggressive.iter().any(|a| artist.contains(a)) {
            println!("  pct={:>5.1}%  score={:+8.4}  {:25}  {}",
                percentile(idx), s,
                artist.chars().take(25).collect::<String>(), t.title);
        }
    }
    println!("\n  ── Liquid ──");
    for (idx, (s, t)) in scored.iter().enumerate() {
        let artist = t.artist.as_deref().unwrap_or("");
        if known_liquid.iter().any(|a| artist.contains(a)) {
            println!("  pct={:>5.1}%  score={:+8.4}  {:25}  {}",
                percentile(idx), s,
                artist.chars().take(25).collect::<String>(), t.title);
        }
    }

    println!();
}
