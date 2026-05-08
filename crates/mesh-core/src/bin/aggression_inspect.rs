//! Inspect the live intensity-axis projection across the user's library.
//!
//! Uses whichever axis the runtime would pick (per-collection override or
//! binary-embedded default — see `IntensityProvider::load_for_collection`),
//! so the numbers shown here match exactly what mesh-cue / mesh-player do
//! at runtime. Works for V15 linear, V18 linear, and V18.1 MLP.
//!
//! Reports:
//!   - which axis is active (variant_id, model_kind)
//!   - distribution of projected scores across the library
//!     (min / quartiles / max + mean ± σ)
//!   - the most and least aggressive tracks under the current scale
//!   - sanity-check against well-known DnB artists (aggressive vs liquid)
//!
//! Usage: cargo run -p mesh-core --bin aggression_inspect [-- /path/to/collection]

use mesh_core::db::DatabaseService;
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

    // ── 1. Active axis ─────────────────────────────────────────────────────
    let provider = match db.intensity_provider() {
        Some(p) => p,
        None => {
            eprintln!("\n  ❌ No intensity axis loaded — DB returned None.");
            std::process::exit(1);
        }
    };
    let axis = &provider.axis;
    println!();
    println!("════════════════════════════════════════════════════════════════════════");
    println!("  ACTIVE INTENSITY AXIS");
    println!("════════════════════════════════════════════════════════════════════════");
    println!("  variant_id:    {}", axis.variant_id);
    println!("  formula:       {}", axis.intensity_formula);
    println!("  embedding_dim: {}", axis.embedding_dim);
    let kind = match &axis.model_kind {
        mesh_core::intensity_axis::ModelKind::Linear { vec, bias } =>
            format!("linear (Vec<f32> len={}, bias={:+.4})", vec.len(), bias),
        mesh_core::intensity_axis::ModelKind::Mlp { w1, b1: _, w2, b2 } =>
            format!("mlp (W1: {}×{}, hidden={}, W2 len={}, bias={:+.4})",
                    w1.len(), w1.first().map(|r| r.len()).unwrap_or(0),
                    w1.len(), w2.len(), b2),
    };
    println!("  model_kind:    {}", kind);

    // ── 2. Project across the library ──────────────────────────────────────
    let all_tracks = db.get_all_tracks().unwrap_or_default();
    let n_total = all_tracks.len();
    println!();
    println!("  total tracks in DB: {}", n_total);

    let mut scored: Vec<(f32, &mesh_core::db::Track)> = all_tracks.iter()
        .filter_map(|t| {
            let tid = t.id?;
            let emb = db.get_ml_embedding_raw(tid).ok().flatten()?;
            if emb.len() != mesh_core::intensity_axis::EMBEDDING_DIM { return None; }
            Some((provider.project(&emb), t))
        })
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let n = scored.len();
    if n == 0 {
        eprintln!("\n  ❌  No tracks with 512-d MuQ-MuLan embeddings — re-run ML analysis.");
        return;
    }
    let pct = |q: f32| -> f32 {
        let i = ((n as f32 - 1.0) * q).round() as usize;
        scored[i].0
    };

    println!();
    println!("════════════════════════════════════════════════════════════════════════");
    println!("  PROJECTED SCORE DISTRIBUTION ACROSS {} TRACKS", n);
    println!("  (raw axis output — for V18.1 mlp this is roughly in [0, 1])");
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

    // ── 3. Tails — what does the scale think is least and most aggressive? ─
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

    // ── 4. Sanity-check against known DnB artists ─────────────────────────
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
    println!("  ── SANITY CHECK: known DnB artists vs. percentile rank ─────────────");
    println!("  Aggressive artists should rank HIGH; liquid artists should rank LOW.\n");
    println!("  ── Aggressive ──");
    let mut n_agg = 0usize;
    let mut sum_agg_pct = 0.0_f32;
    for (idx, (s, t)) in scored.iter().enumerate() {
        let artist = t.artist.as_deref().unwrap_or("");
        if known_aggressive.iter().any(|a| artist.contains(a)) {
            println!("  pct={:>5.1}%  score={:+8.4}  {:25}  {}",
                percentile(idx), s,
                artist.chars().take(25).collect::<String>(), t.title);
            n_agg += 1;
            sum_agg_pct += percentile(idx);
        }
    }
    if n_agg > 0 {
        println!("  ── aggressive mean percentile: {:.1}% (over {} tracks)",
                 sum_agg_pct / n_agg as f32, n_agg);
    }

    println!("\n  ── Liquid ──");
    let mut n_liq = 0usize;
    let mut sum_liq_pct = 0.0_f32;
    for (idx, (s, t)) in scored.iter().enumerate() {
        let artist = t.artist.as_deref().unwrap_or("");
        if known_liquid.iter().any(|a| artist.contains(a)) {
            println!("  pct={:>5.1}%  score={:+8.4}  {:25}  {}",
                percentile(idx), s,
                artist.chars().take(25).collect::<String>(), t.title);
            n_liq += 1;
            sum_liq_pct += percentile(idx);
        }
    }
    if n_liq > 0 {
        println!("  ── liquid mean percentile:     {:.1}% (over {} tracks)",
                 sum_liq_pct / n_liq as f32, n_liq);
    }

    if n_agg > 0 && n_liq > 0 {
        let separation = (sum_agg_pct / n_agg as f32) - (sum_liq_pct / n_liq as f32);
        println!();
        println!("  ── separation: aggressive_mean − liquid_mean = {:+.1} pp", separation);
        println!("     (positive = correct direction; >40pp = strong; >60pp = excellent)");
    }

    println!();
}
