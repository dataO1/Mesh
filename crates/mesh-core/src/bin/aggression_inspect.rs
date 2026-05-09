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
    // Two positional-ish args, picked by literal flag form for simplicity:
    //   <collection_root>            — defaults to ~/Music/mesh-collection
    //   --export-md <path>           — also dump full per-track ranking to file
    let mut export_md: Option<PathBuf> = None;
    let mut outliers_md: Option<PathBuf> = None;
    let mut outlier_k: usize = 15;
    let mut outlier_top_n: usize = 30;
    let mut baseline_scores: Option<PathBuf> = None;
    let mut baseline_label: String = "baseline".into();
    let mut active_label: String = "active".into();
    let mut positional: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--export-md" {
            if let Some(p) = args.next() {
                export_md = Some(PathBuf::from(p));
            } else {
                eprintln!("--export-md requires a path argument");
                std::process::exit(2);
            }
        } else if a == "--outliers" {
            if let Some(p) = args.next() {
                outliers_md = Some(PathBuf::from(p));
            } else {
                eprintln!("--outliers requires a path argument");
                std::process::exit(2);
            }
        } else if a == "--outlier-k" {
            if let Some(v) = args.next().and_then(|s| s.parse().ok()) {
                outlier_k = v;
            } else {
                eprintln!("--outlier-k requires an integer");
                std::process::exit(2);
            }
        } else if a == "--outlier-top-n" {
            if let Some(v) = args.next().and_then(|s| s.parse().ok()) {
                outlier_top_n = v;
            } else {
                eprintln!("--outlier-top-n requires an integer");
                std::process::exit(2);
            }
        } else if a == "--baseline-scores" {
            if let Some(p) = args.next() {
                baseline_scores = Some(PathBuf::from(p));
            } else {
                eprintln!("--baseline-scores requires a path");
                std::process::exit(2);
            }
        } else if a == "--baseline-label" {
            if let Some(v) = args.next() { baseline_label = v; }
            else { eprintln!("--baseline-label requires a string"); std::process::exit(2); }
        } else if a == "--active-label" {
            if let Some(v) = args.next() { active_label = v; }
            else { eprintln!("--active-label requires a string"); std::process::exit(2); }
        } else {
            positional.push(a);
        }
    }
    let collection_root = positional.into_iter()
        .next()
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

    // Round-7.7: read the 1024-d intensity-probe column. Falls back to None
    // for tracks analysed before round-7.7 (those need re-analysis to land in
    // ml_intensity_embeddings — surfaces as the migration prompt at startup).
    let mut scored: Vec<(f32, &mesh_core::db::Track)> = all_tracks.iter()
        .filter_map(|t| {
            let tid = t.id?;
            let emb = db.get_ml_intensity_embedding_raw(tid).ok().flatten()?;
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

    // ── 5. Optional full-ranking export (--export-md) ─────────────────────
    if let Some(out_path) = export_md.as_ref() {
        use std::io::Write;
        let n = scored.len();
        // Sort descending so the table reads "most → least intense" — most
        // useful for at-a-glance review and easy joining later (rank 1 =
        // top of library).
        let mut by_score = scored.clone();
        by_score.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let kind_str = match &axis.model_kind {
            mesh_core::intensity_axis::ModelKind::Linear { vec, bias } =>
                format!("linear (Vec<f32> len={}, bias={:+.4})", vec.len(), bias),
            mesh_core::intensity_axis::ModelKind::Mlp { w1, b2, .. } =>
                format!("mlp (input_dim={}, hidden={}, bias={:+.4})",
                        w1.first().map(|r| r.len()).unwrap_or(0), w1.len(), b2),
        };

        let mut buf = String::new();
        buf.push_str("---\n");
        buf.push_str("tags: [knowledge-base, mesh, intensity-axis, baseline-export]\n");
        buf.push_str(&format!("created: {}\n", chrono::Utc::now().format("%Y-%m-%d")));
        buf.push_str("status: archival baseline\n");
        buf.push_str(&format!("axis_variant: {}\n", axis.variant_id));
        buf.push_str(&format!("axis_kind: {}\n", kind_str));
        buf.push_str(&format!("library: {}\n", collection_root.display()));
        buf.push_str(&format!("n_tracks: {}\n", n));
        buf.push_str("---\n\n");
        buf.push_str(&format!("# Library intensity ranking — {}\n\n", axis.variant_id));
        buf.push_str(&format!(
            "Per-track V18.x intensity projection on the deployed library, captured \
            for before/after comparison across round-7.7 substrate changes.\n\n\
            **Axis:** `{}` — {}\n\n\
            **Library:** `{}` ({} tracks)\n\n\
            **Distribution:** min `{:+.4}` · p25 `{:+.4}` · median `{:+.4}` · p75 `{:+.4}` · max `{:+.4}` · mean ± σ `{:+.4} ± {:.4}`\n\n",
            axis.variant_id, kind_str,
            collection_root.display(), n,
            scored[0].0, pct(0.25), pct(0.50), pct(0.75), scored[n-1].0,
            mean, var.sqrt(),
        ));

        if n_agg > 0 && n_liq > 0 {
            let separation = (sum_agg_pct / n_agg as f32) - (sum_liq_pct / n_liq as f32);
            buf.push_str(&format!(
                "**Sanity-check separation:** aggressive_mean = {:.1}% · \
                liquid_mean = {:.1}% · **separation = {:+.1} pp**\n\n",
                sum_agg_pct / n_agg as f32, sum_liq_pct / n_liq as f32, separation,
            ));
        }

        buf.push_str("## Full per-track ranking (sorted by intensity, descending)\n\n");
        buf.push_str("| rank | percentile | score | track_id | artist | title | bpm | key | duration_s |\n");
        buf.push_str("|---:|---:|---:|---:|---|---|---:|---|---:|\n");
        for (rank_idx, (score, t)) in by_score.iter().enumerate() {
            let rank = rank_idx + 1;
            let pct = 100.0 * (n - rank) as f32 / (n as f32 - 1.0).max(1.0);
            let id_str = t.id.map(|i| i.to_string()).unwrap_or_else(|| "—".into());
            // Pipe-escape to keep markdown table happy
            let artist = t.artist.as_deref().unwrap_or("?").replace('|', "\\|");
            let title  = t.title.replace('|', "\\|");
            let bpm = t.bpm.map(|v| format!("{:.1}", v)).unwrap_or_default();
            let key = t.key.as_deref().unwrap_or("");
            let dur = format!("{:.0}", t.duration_seconds);
            buf.push_str(&format!(
                "| {} | {:.1}% | {:+.4} | {} | {} | {} | {} | {} | {} |\n",
                rank, pct, score, id_str, artist, title, bpm, key, dur,
            ));
        }

        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::File::create(out_path).and_then(|mut f| f.write_all(buf.as_bytes())) {
            Ok(()) => {
                println!();
                println!("  ── EXPORT: wrote {} tracks to {}", n, out_path.display());
            }
            Err(e) => {
                eprintln!("\n  ❌  failed to write export to {}: {}", out_path.display(), e);
                std::process::exit(1);
            }
        }
    }

    // ── 6. Optional kNN-residual outlier detection (--outliers) ───────────
    // For each track, find its k nearest sonic neighbours by cosine
    // similarity in 1024-d intensity-probe embedding space. Compare its own
    // intensity score to the mean intensity of those neighbours. A track
    // whose score is far above its neighbours' mean is "over-ranked": the
    // model thinks it's more intense than its sonic siblings. Far below =
    // under-ranked. Z-scored against the per-track neighbour σ.
    //
    // When `--baseline-scores <path>` is also supplied, the same kNN graph is
    // re-scored against a second source of per-track scores (parsed from a
    // baseline.md exported by --export-md). This isolates the *scoring*
    // change between two model versions while holding the sonic-neighbourhood
    // definition fixed — so V18.1 vs V18.X outliers are directly comparable.
    if let Some(out_path) = outliers_md.as_ref() {
        use std::io::Write;
        eprintln!("\n  Running kNN-residual outlier detection (k={}) …", outlier_k);

        // Gather (track_id, embedding, score_active, &Track).
        let triples: Vec<(i64, Vec<f32>, f32, &mesh_core::db::Track)> = all_tracks.iter()
            .filter_map(|t| {
                let tid = t.id?;
                let emb = db.get_ml_intensity_embedding_raw(tid).ok().flatten()?;
                if emb.len() != mesh_core::intensity_axis::EMBEDDING_DIM { return None; }
                let score = provider.project(&emb);
                Some((tid, emb, score, t))
            })
            .collect();

        // Optional baseline-scores join: HashMap<track_id, score>. Tracks
        // missing from the baseline are flagged in the report as "no baseline".
        let baseline_scores_map: Option<std::collections::HashMap<i64, f32>> = baseline_scores.as_ref().map(|p| {
            let txt = std::fs::read_to_string(p).unwrap_or_else(|e| {
                eprintln!("  ❌  failed to read --baseline-scores {}: {}", p.display(), e);
                std::process::exit(1);
            });
            // Baseline tables look like:
            //   | rank | percentile | score | track_id | artist | title | bpm | key | duration_s |
            // We just want columns 3 (score) and 4 (track_id). Robust to header lines etc.
            let mut map = std::collections::HashMap::new();
            for line in txt.lines() {
                if !line.starts_with('|') { continue; }
                let cells: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if cells.len() < 6 { continue; }
                let score: Option<f32> = cells[3].parse().ok();
                let tid: Option<i64> = cells[4].parse().ok();
                if let (Some(s), Some(t)) = (score, tid) {
                    map.insert(t, s);
                }
            }
            eprintln!("  Loaded {} baseline scores from {}", map.len(), p.display());
            map
        });

        // Pre-normalize embeddings → cosine similarity becomes a dot product.
        let norms: Vec<Vec<f32>> = triples.iter().map(|(_, e, _, _)| {
            let mag: f32 = e.iter().map(|x| x*x).sum::<f32>().sqrt().max(1e-12);
            e.iter().map(|x| x / mag).collect()
        }).collect();

        let m = triples.len();
        let k = outlier_k.min(m.saturating_sub(1));

        // Per-track scoring sources. Active = live model. Baseline = parsed file.
        // For tracks missing from the baseline, baseline_score = NaN and we
        // exclude them from baseline aggregates.
        let scores_active: Vec<f32> = triples.iter().map(|(_, _, s, _)| *s).collect();
        let scores_baseline: Vec<f32> = triples.iter().map(|(tid, _, _, _)| {
            baseline_scores_map.as_ref()
                .and_then(|m| m.get(tid).copied())
                .unwrap_or(f32::NAN)
        }).collect();

        // For each track, find top-k neighbours and compute residual under
        // both scoring sources. Same neighbour set for both → directly
        // comparable z-scores.
        #[derive(Clone)]
        struct Outlier {
            idx: usize,
            score_active: f32,
            score_baseline: f32, // NaN if missing
            nb_mean_active: f32,
            nb_mean_baseline: f32, // NaN if any neighbour missing baseline
            residual_active: f32,
            residual_baseline: f32, // NaN if missing
            z_active: f32,
            z_baseline: f32, // NaN if missing
            top_neighbours: Vec<(usize, f32)>,
        }
        let mut outliers: Vec<Outlier> = Vec::with_capacity(m);
        for i in 0..m {
            let qi = &norms[i];
            let mut sims: Vec<(f32, usize)> = (0..m)
                .filter(|&j| j != i)
                .map(|j| {
                    let qj = &norms[j];
                    let dot: f32 = qi.iter().zip(qj.iter()).map(|(a, b)| a * b).sum();
                    (dot, j)
                })
                .collect();
            sims.select_nth_unstable_by(k, |a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            let neighbours = &sims[..k];

            // Active scoring
            let nb_scores_a: Vec<f32> = neighbours.iter().map(|(_, j)| scores_active[*j]).collect();
            let mean_a = nb_scores_a.iter().sum::<f32>() / k as f32;
            let var_a = nb_scores_a.iter().map(|s| (s - mean_a).powi(2)).sum::<f32>() / k as f32;
            let std_a = var_a.sqrt().max(1e-3);
            let res_a = scores_active[i] - mean_a;
            let z_a = res_a / std_a;

            // Baseline scoring (skip if track or any neighbour missing)
            let nb_scores_b: Vec<f32> = neighbours.iter().map(|(_, j)| scores_baseline[*j]).collect();
            let any_missing = scores_baseline[i].is_nan() || nb_scores_b.iter().any(|s| s.is_nan());
            let (mean_b, res_b, z_b) = if any_missing {
                (f32::NAN, f32::NAN, f32::NAN)
            } else {
                let mean = nb_scores_b.iter().sum::<f32>() / k as f32;
                let var = nb_scores_b.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / k as f32;
                let std = var.sqrt().max(1e-3);
                let r = scores_baseline[i] - mean;
                (mean, r, r / std)
            };

            let mut top3 = neighbours.to_vec();
            top3.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            let top_neighbours = top3.iter().take(3).map(|(s, j)| (*j, *s)).collect();

            outliers.push(Outlier {
                idx: i,
                score_active: scores_active[i], score_baseline: scores_baseline[i],
                nb_mean_active: mean_a, nb_mean_baseline: mean_b,
                residual_active: res_a, residual_baseline: res_b,
                z_active: z_a, z_baseline: z_b,
                top_neighbours,
            });
        }

        let render_neighbours = |o: &Outlier| -> String {
            o.top_neighbours.iter().map(|(j, sim)| {
                let t = triples[*j].3;
                let nb_score_a = scores_active[*j];
                format!("{} — {} (sim {:.3}, {} {:+.3})",
                    t.artist.as_deref().unwrap_or("?").replace('|', "\\|"),
                    t.title.replace('|', "\\|"),
                    sim, active_label, nb_score_a)
            }).collect::<Vec<_>>().join("<br>")
        };

        let mut buf = String::new();
        buf.push_str("---\n");
        buf.push_str("tags: [knowledge-base, mesh, intensity-axis, outlier-detection]\n");
        buf.push_str(&format!("created: {}\n", chrono::Utc::now().format("%Y-%m-%d")));
        buf.push_str("status: kNN-residual outlier snapshot\n");
        buf.push_str(&format!("axis_variant: {}\n", axis.variant_id));
        buf.push_str(&format!("library: {}\n", collection_root.display()));
        buf.push_str(&format!("n_tracks: {}\n", m));
        buf.push_str(&format!("knn_k: {}\n", k));
        buf.push_str(&format!("active_label: {}\n", active_label));
        if baseline_scores.is_some() {
            buf.push_str(&format!("baseline_label: {}\n", baseline_label));
        }
        buf.push_str("---\n\n");
        buf.push_str(&format!("# Intensity outliers — {} vs {} (kNN-residual)\n\n",
            active_label, baseline_label));

        if baseline_scores.is_some() {
            buf.push_str(&format!(
                "For each track, this finds its **{} nearest sonic neighbours** by cosine \
                similarity in the 1024-d MuQ-MuLan intensity-probe embedding (V18.X's substrate). \
                The same neighbour set is then re-scored under both `{}` (current model, live \
                projection) and `{}` (parsed from baseline file). This holds *what counts as a \
                similar track* fixed and isolates how the **scoring decisions** differ.\n\n\
                * **z = (track_score − neighbour_mean) / neighbour_std**\n\
                * **|z| > 2σ** = noteworthy disagreement with sonic siblings\n\
                * **|z| > 3σ** = strong outlier\n\n\
                The summary table below is the headline answer: *which model produces more \
                outliers, or stronger outliers?*\n\n",
                k, active_label, baseline_label));
        } else {
            buf.push_str(&format!(
                "For each track, this finds its **{} nearest sonic neighbours** by cosine \
                similarity in the 1024-d MuQ-MuLan intensity-probe embedding. \
                Then compares the track's own intensity score to those neighbours' mean.\n\n\
                * **z ≫ 0** → over-ranked vs sonic siblings\n\
                * **z ≪ 0** → under-ranked vs sonic siblings\n\n", k));
        }

        // Aggregate outlier stats per scoring source
        let aggregate = |zs: &[f32]| -> (f32, f32, usize, usize, usize) {
            let valid: Vec<f32> = zs.iter().copied().filter(|z| !z.is_nan()).collect();
            if valid.is_empty() { return (f32::NAN, f32::NAN, 0, 0, 0); }
            let mean_abs_z = valid.iter().map(|z| z.abs()).sum::<f32>() / valid.len() as f32;
            let max_abs_z = valid.iter().map(|z| z.abs()).fold(0.0_f32, f32::max);
            let n_2sigma = valid.iter().filter(|z| z.abs() > 2.0).count();
            let n_3sigma = valid.iter().filter(|z| z.abs() > 3.0).count();
            (mean_abs_z, max_abs_z, n_2sigma, n_3sigma, valid.len())
        };
        let zs_a: Vec<f32> = outliers.iter().map(|o| o.z_active).collect();
        let zs_b: Vec<f32> = outliers.iter().map(|o| o.z_baseline).collect();
        let (mean_a, max_a, n2_a, n3_a, n_valid_a) = aggregate(&zs_a);
        let (mean_b, max_b, n2_b, n3_b, n_valid_b) = aggregate(&zs_b);

        if baseline_scores.is_some() {
            buf.push_str("## Aggregate outlier metrics\n\n");
            buf.push_str("Lower mean |z| and lower outlier counts = better internal consistency \
                (the model agrees with sonic neighbours more often).\n\n");
            buf.push_str(&format!("| metric | {} | {} | winner |\n", active_label, baseline_label));
            buf.push_str("|---|---:|---:|:---|\n");
            let pick = |a: f32, b: f32| -> &str {
                if a.is_nan() || b.is_nan() { "—" }
                else if a < b { active_label.as_str() } else if b < a { baseline_label.as_str() } else { "tie" }
            };
            let pick_int = |a: usize, b: usize| -> &str {
                if a < b { active_label.as_str() } else if b < a { baseline_label.as_str() } else { "tie" }
            };
            buf.push_str(&format!("| Tracks scored | {} | {} | — |\n", n_valid_a, n_valid_b));
            buf.push_str(&format!("| **Mean \\|z\\|** (overall outlier intensity) | `{:.3}σ` | `{:.3}σ` | **{}** |\n",
                mean_a, mean_b, pick(mean_a, mean_b)));
            buf.push_str(&format!("| **Max \\|z\\|** (worst outlier) | `{:.3}σ` | `{:.3}σ` | **{}** |\n",
                max_a, max_b, pick(max_a, max_b)));
            buf.push_str(&format!("| Tracks with \\|z\\| > 2σ | {} ({:.1} %) | {} ({:.1} %) | **{}** |\n",
                n2_a, 100.0 * n2_a as f32 / n_valid_a.max(1) as f32,
                n2_b, 100.0 * n2_b as f32 / n_valid_b.max(1) as f32,
                pick_int(n2_a, n2_b)));
            buf.push_str(&format!("| Tracks with \\|z\\| > 3σ | {} ({:.1} %) | {} ({:.1} %) | **{}** |\n",
                n3_a, 100.0 * n3_a as f32 / n_valid_a.max(1) as f32,
                n3_b, 100.0 * n3_b as f32 / n_valid_b.max(1) as f32,
                pick_int(n3_a, n3_b)));
            buf.push_str("\n");
        }

        // Sort by max(|z_active|, |z_baseline|) to surface "biggest outlier in either model".
        let mut over: Vec<&Outlier> = outliers.iter().collect();
        let mut under: Vec<&Outlier> = outliers.iter().collect();
        if baseline_scores.is_some() {
            // Comparison mode: sort by max signed z across both models so we
            // see tracks one model thinks are wildly out of place even if the
            // other agrees with the cluster.
            over.sort_by(|a, b| {
                let za = a.z_active.max(if a.z_baseline.is_nan() { f32::NEG_INFINITY } else { a.z_baseline });
                let zb = b.z_active.max(if b.z_baseline.is_nan() { f32::NEG_INFINITY } else { b.z_baseline });
                zb.partial_cmp(&za).unwrap_or(std::cmp::Ordering::Equal)
            });
            under.sort_by(|a, b| {
                let za = a.z_active.min(if a.z_baseline.is_nan() { f32::INFINITY } else { a.z_baseline });
                let zb = b.z_active.min(if b.z_baseline.is_nan() { f32::INFINITY } else { b.z_baseline });
                za.partial_cmp(&zb).unwrap_or(std::cmp::Ordering::Equal)
            });
        } else {
            over.sort_by(|a, b| b.z_active.partial_cmp(&a.z_active).unwrap_or(std::cmp::Ordering::Equal));
            under.sort_by(|a, b| a.z_active.partial_cmp(&b.z_active).unwrap_or(std::cmp::Ordering::Equal));
        }

        let nan_str = |v: f32, fmt_str: &str| if v.is_nan() { "—".to_string() } else {
            // Tiny formatter to handle a few precisions inline.
            match fmt_str {
                "z"  => format!("{:+.2}σ", v),
                "s"  => format!("{:+.4}", v),
                _ => format!("{}", v),
            }
        };

        let render_row = |o: &Outlier| -> String {
            let t = triples[o.idx].3;
            let id_str = t.id.map(|i| i.to_string()).unwrap_or_else(|| "—".into());
            let artist = t.artist.as_deref().unwrap_or("?").replace('|', "\\|");
            let title  = t.title.replace('|', "\\|");
            if baseline_scores.is_some() {
                format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                    nan_str(o.z_active, "z"),
                    nan_str(o.z_baseline, "z"),
                    nan_str(o.score_active, "s"),
                    nan_str(o.score_baseline, "s"),
                    nan_str(o.nb_mean_active, "s"),
                    nan_str(o.nb_mean_baseline, "s"),
                    nan_str(o.residual_active, "s"),
                    nan_str(o.residual_baseline, "s"),
                    id_str, artist, title,
                )
            } else {
                format!("| {} | {} | {} | {} | {} | {} | {} | {} |",
                    nan_str(o.z_active, "z"),
                    nan_str(o.score_active, "s"),
                    nan_str(o.nb_mean_active, "s"),
                    nan_str(o.residual_active, "s"),
                    id_str, artist, title, render_neighbours(o),
                )
            }
        };

        let header = if baseline_scores.is_some() {
            format!("| z {a} | z {b} | score {a} | score {b} | nb mean {a} | nb mean {b} | residual {a} | residual {b} | track_id | artist | title |\n\
                     |---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|",
                a = active_label, b = baseline_label)
        } else {
            "| z-score | track score | neighbour mean | residual | track_id | artist | title | top-3 nearest neighbours |\n\
             |---:|---:|---:|---:|---:|---|---|---|".to_string()
        };

        buf.push_str(&format!("## Top {} most over-ranked tracks\n\n", outlier_top_n));
        buf.push_str(&format!("{}\n\n", if baseline_scores.is_some() {
            format!("Sorted by **max(z {}, z {})** — surfaces the biggest over-rank in either model. \
                If z is much larger in one column than the other, that model is the one mis-scoring \
                this track. If both columns agree (both ≈ 0 or both ≈ +3σ), both models are doing \
                the same thing.", active_label, baseline_label)
        } else {
            "Model rates these much *higher* than their sonic siblings.".to_string()
        }));
        buf.push_str(&format!("{}\n", header));
        for o in over.iter().take(outlier_top_n) {
            buf.push_str(&render_row(o));
            buf.push('\n');
        }
        buf.push('\n');

        buf.push_str(&format!("## Top {} most under-ranked tracks\n\n", outlier_top_n));
        buf.push_str(&format!("{}\n\n", if baseline_scores.is_some() {
            format!("Sorted by **min(z {}, z {})** — surfaces the biggest under-rank in either model.",
                active_label, baseline_label)
        } else {
            "Model rates these much *lower* than their sonic siblings.".to_string()
        }));
        buf.push_str(&format!("{}\n", header));
        for o in under.iter().take(outlier_top_n) {
            buf.push_str(&render_row(o));
            buf.push('\n');
        }
        buf.push('\n');

        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::File::create(out_path).and_then(|mut f| f.write_all(buf.as_bytes())) {
            Ok(()) => {
                println!();
                println!("  ── OUTLIERS: wrote top-{} over/under-ranked to {}",
                    outlier_top_n, out_path.display());
                if baseline_scores.is_some() {
                    println!("     {} mean |z|={:.3}σ  max={:.3}σ  |z|>2σ: {}  |z|>3σ: {}",
                        active_label, mean_a, max_a, n2_a, n3_a);
                    println!("     {} mean |z|={:.3}σ  max={:.3}σ  |z|>2σ: {}  |z|>3σ: {}",
                        baseline_label, mean_b, max_b, n2_b, n3_b);
                } else {
                    println!("     {} z-range observed: [{:+.2}σ, {:+.2}σ]",
                        active_label, under[0].z_active, over[0].z_active);
                }
            }
            Err(e) => {
                eprintln!("\n  ❌  failed to write outliers to {}: {}", out_path.display(), e);
                std::process::exit(1);
            }
        }
    }

    println!();
}
