//! Analyze which PCA dimensions correlate with perceived aggression.
//!
//! Outputs each track with its first 10 PCA components alongside genre and
//! current intensity composite, so we can check which PCA axis best separates
//! liquid from neuro.
//!
//! Usage: cargo run -p mesh-core --bin pca_aggression [-- /path/to/collection]

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

    let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
    let ids: Vec<i64> = all_pca.iter().filter_map(|(t, _)| t.id).collect();
    let intensity_map = db.batch_get_intensity_components(&ids).unwrap_or_default();

    // Genre info
    let mut genre_map: HashMap<i64, String> = HashMap::new();
    for &id in &ids {
        if let Ok(Some(ml)) = db.get_ml_analysis(id) {
            genre_map.insert(id, ml.top_genre.unwrap_or_default());
        }
    }

    let pca_dim = all_pca.first().map(|(_, v)| v.len()).unwrap_or(0);
    let check_dims = pca_dim.min(113);

    eprintln!("PCA dimensionality: {}", pca_dim);
    eprintln!("Checking first {} dimensions for aggression correlation", check_dims);
    eprintln!("Tracks: {}", all_pca.len());

    // Collect data for correlation analysis
    struct TrackData {
        title: String,
        artist: String,
        genre: String,
        composite: f32,
        pca: Vec<f32>,
    }

    let mut data: Vec<TrackData> = Vec::new();
    for (track, pca_vec) in &all_pca {
        let Some(id) = track.id else { continue };
        let composite = intensity_map.get(&id)
            .map(|ic| mesh_core::suggestions::scoring::composite_intensity_v2(ic))
            .unwrap_or(0.0);
        let genre = genre_map.get(&id).cloned().unwrap_or_default();
        data.push(TrackData {
            title: track.title.clone(),
            artist: track.artist.clone().unwrap_or_default(),
            genre,
            composite,
            pca: pca_vec.clone(),
        });
    }

    // Assign a rough aggression label based on genre keywords
    // 0 = ambient/chill, 1 = liquid/deep, 2 = general DnB, 3 = neuro/techstep, 4 = heavy/crossbreed
    let genre_aggression = |genre: &str, artist: &str| -> f32 {
        let g = genre.to_lowercase();
        let a = artist.to_lowercase();
        // Known heavy artists
        if a.contains("current value") || a.contains("billain") || a.contains("neonlight")
            || a.contains("mefjus") || a.contains("phace") || a.contains("noisia")
            || a.contains("black sun empire") || a.contains("audio") || a.contains("teddy killerz") {
            return 3.5;
        }
        // Known liquid artists
        if a.contains("random movement") || a.contains("calibre") || a.contains("lsb")
            || a.contains("logistics") || a.contains("bcee") || a.contains("etherwood") {
            return 1.0;
        }
        // Genre-based
        if g.contains("ambient") || g.contains("downtempo") || g.contains("chillout") { return 0.0; }
        if g.contains("deep house") || g.contains("minimal") { return 0.5; }
        if g.contains("liquid") { return 1.0; }
        if g.contains("synth") || g.contains("electro") { return 1.5; }
        if g.contains("drum n bass") || g.contains("jungle") { return 2.5; }
        if g.contains("techno") { return 2.0; }
        if g.contains("breakcore") || g.contains("industrial") || g.contains("hardcore") { return 4.0; }
        2.0 // default mid
    };

    // Compute Pearson correlation between each PCA dimension and genre_aggression score
    eprintln!("\n=== PCA DIMENSION CORRELATION WITH GENRE-BASED AGGRESSION ===\n");

    let n = data.len() as f32;
    let aggr_scores: Vec<f32> = data.iter().map(|d| genre_aggression(&d.genre, &d.artist)).collect();
    let aggr_mean = aggr_scores.iter().sum::<f32>() / n;
    let aggr_var: f32 = aggr_scores.iter().map(|a| (a - aggr_mean).powi(2)).sum::<f32>();

    let mut correlations: Vec<(usize, f32)> = Vec::new();

    for dim in 0..check_dims {
        let vals: Vec<f32> = data.iter().map(|d| d.pca[dim]).collect();
        let mean = vals.iter().sum::<f32>() / n;
        let var: f32 = vals.iter().map(|v| (v - mean).powi(2)).sum::<f32>();
        let cov: f32 = vals.iter().zip(aggr_scores.iter())
            .map(|(v, a)| (v - mean) * (a - aggr_mean))
            .sum();
        let denom = (var * aggr_var).sqrt();
        let r = if denom > 1e-10 { cov / denom } else { 0.0 };
        correlations.push((dim, r));
        eprintln!("  PCA[{:>3}]: r = {:+.4}", dim, r);
    }

    // Sort by absolute correlation
    correlations.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));

    eprintln!("\n=== TOP 5 MOST CORRELATED DIMENSIONS ===\n");
    for (dim, r) in correlations.iter().take(5) {
        eprintln!("  PCA[{:>3}]: r = {:+.4}  ({})", dim, r,
            if *r > 0.0 { "higher = more aggressive" } else { "higher = less aggressive" });
    }

    let best_dim = correlations[0].0;
    let best_r = correlations[0].1;
    let sign = if best_r > 0.0 { 1.0 } else { -1.0 };

    eprintln!("\n=== BEST DIMENSION: PCA[{}] (r={:+.4}) ===\n", best_dim, best_r);

    // === MULTI-DIMENSION WEIGHTED SCORE ===
    // Use top 5 correlated dimensions, weighted by their correlation
    let top_dims: Vec<(usize, f32)> = correlations.iter().take(5).map(|&(d, r)| (d, r)).collect();
    eprintln!("=== MULTI-DIMENSION AGGRESSION (top 5 dims, correlation-weighted) ===\n");
    for (dim, r) in &top_dims {
        eprintln!("  dim={}, weight={:+.4}", dim, r);
    }

    // Compute multi-dim score per track: sum(pca[dim] * correlation) for top 5 dims
    let multi_scores: Vec<f32> = data.iter().map(|d| {
        top_dims.iter().map(|&(dim, r)| d.pca[dim] * r).sum::<f32>()
    }).collect();

    // Compute correlation of multi-dim score with aggression proxy
    let multi_mean = multi_scores.iter().sum::<f32>() / n;
    let multi_var: f32 = multi_scores.iter().map(|v| (v - multi_mean).powi(2)).sum();
    let multi_cov: f32 = multi_scores.iter().zip(aggr_scores.iter())
        .map(|(v, a)| (v - multi_mean) * (a - aggr_mean)).sum();
    let multi_r = if (multi_var * aggr_var).sqrt() > 1e-10 {
        multi_cov / (multi_var * aggr_var).sqrt()
    } else { 0.0 };

    eprintln!("\n  Multi-dim (top 5) correlation: r = {:+.4} (vs single-dim r = {:+.4})", multi_r, best_r);

    // === SWEEP N=1..50: find optimal number of dimensions ===
    eprintln!("\n=== CUMULATIVE CORRELATION SWEEP (N=1..{}) ===\n", check_dims);
    let mut best_n = 1usize;
    let mut best_n_r = 0.0f32;
    for n_dims in 1..=check_dims {
        let top_n: Vec<(usize, f32)> = correlations.iter().take(n_dims).map(|&(d, r)| (d, r)).collect();
        let scores_n: Vec<f32> = data.iter().map(|d| {
            top_n.iter().map(|&(dim, r)| d.pca[dim] * r).sum::<f32>()
        }).collect();
        let mean_n = scores_n.iter().sum::<f32>() / n;
        let var_n: f32 = scores_n.iter().map(|v| (v - mean_n).powi(2)).sum();
        let cov_n: f32 = scores_n.iter().zip(aggr_scores.iter())
            .map(|(v, a)| (v - mean_n) * (a - aggr_mean)).sum();
        let r_n = if (var_n * aggr_var).sqrt() > 1e-10 {
            cov_n / (var_n * aggr_var).sqrt()
        } else { 0.0 };
        let marker = if r_n.abs() > best_n_r { best_n = n_dims; best_n_r = r_n.abs(); " <<<" } else { "" };
        eprintln!("  N={:>2}: r={:+.4}  dims=[{}]{}",
            n_dims, r_n,
            top_n.iter().map(|(d, _)| d.to_string()).collect::<Vec<_>>().join(","),
            marker);
    }
    eprintln!("\n  Best N={} with r={:.4}\n", best_n, best_n_r);

    // Sort by multi-dim score and show extremes
    let mut multi_sorted: Vec<(usize, f32)> = multi_scores.iter().enumerate()
        .map(|(i, &s)| (i, s)).collect();
    multi_sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    eprintln!("=== LEAST AGGRESSIVE by multi-dim (bottom 15) ===");
    for &(idx, score) in multi_sorted.iter().take(15) {
        let d = &data[idx];
        eprintln!("  {:.4}\t{:25}\t{}", score, d.artist, d.title);
    }
    eprintln!("\n=== MOST AGGRESSIVE by multi-dim (top 15) ===");
    for &(idx, score) in multi_sorted.iter().rev().take(15) {
        let d = &data[idx];
        eprintln!("  {:.4}\t{:25}\t{}", score, d.artist, d.title);
    }

    // Show known artists
    eprintln!("\n=== KNOWN ARTISTS: multi-dim scores ===");
    for &(idx, score) in &multi_sorted {
        let d = &data[idx];
        if d.artist.contains("Random Movement") || d.artist.contains("Marcus Intalex")
            || d.artist.contains("Black Sun Empire") || d.artist.contains("Billain")
            || d.artist.contains("Current Value") || d.artist.contains("Noisia")
            || d.artist.contains("Neonlight") || d.artist.contains("Phace") {
            eprintln!("  {:.4}\t{:25}\t{}", score, d.artist, d.title);
        }
    }

    // Sort tracks by best single PCA dimension for the TSV output
    let mut sorted: Vec<(usize, f32, f32)> = data.iter().enumerate()
        .map(|(i, d)| (i, d.pca[best_dim] * sign, aggr_scores[i]))
        .collect();
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Also compute real aggression scores using genre+mood and show mood tags
    eprintln!("\n=== SAMPLE TRACKS: genre + mood tags + aggression score ===\n");
    let sample_names = ["Fuzzy Teeth", "Slinkystink", "Airbourne", "Shinde", "Overcome",
                         "Eraser", "Ego VIP", "Bitemark", "Omnivore", "Dooky Boogs",
                         "Paradise", "Witch", "Air ", "Reptile", "Blindside"];
    for (track, _) in &all_pca {
        let Some(id) = track.id else { continue };
        let matches = sample_names.iter().any(|s| track.title.contains(s));
        if !matches { continue; }
        if let Ok(Some(ml)) = db.get_ml_analysis(id) {
            let moods = ml.mood_themes.clone().unwrap_or_default();
            let top_moods: Vec<String> = moods.iter().take(5)
                .map(|(tag, p)| format!("{}={:.2}", tag, p)).collect();
            let genre = ml.top_genre.as_deref().unwrap_or("");
            let aggr = mesh_core::suggestions::aggression::compute_track_aggression(
                genre, ml.mood_themes.as_ref(),
            );
            eprintln!("  aggr={:.3} genre={:20} artist={:25} title={:30} moods=[{}]",
                aggr, genre, track.artist.as_deref().unwrap_or("?"), track.title, top_moods.join(", "));
        }
    }

    // Print header + data as TSV to stdout for inspection
    println!("pca_rank\tpca_val\taggr_label\tcomposite\tartist\ttitle\tgenre");
    for (rank, &(idx, pca_val, aggr)) in sorted.iter().enumerate() {
        let d = &data[idx];
        println!("{}\t{:.4}\t{:.1}\t{:.4}\t{}\t{}\t{}",
            rank + 1, pca_val, aggr, d.composite, d.artist, d.title, d.genre);
    }

    eprintln!("\n{} tracks analyzed", data.len());
}
