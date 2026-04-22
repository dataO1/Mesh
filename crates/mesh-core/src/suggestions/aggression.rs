//! PCA aggression axis computation and scoring.
//!
//! Computes a per-track aggression estimate from genre labels + mood tags,
//! then finds the PCA dimension most correlated with this estimate.

use std::collections::HashMap;

/// Compute a genre-based aggression score from a Discogs genre label.
/// Returns a value in [0.0, 0.75] representing coarse genre aggression.
pub fn genre_aggression_score(genre: &str) -> f32 {
    // Strip "SuperGenre---SubGenre" to get the sub-genre
    let sub = genre.split("---").last().unwrap_or(genre);
    let sub_lower = sub.to_lowercase();
    let full_lower = genre.to_lowercase();

    // Check super-genre categories first for broad classification
    if full_lower.starts_with("children") || full_lower.starts_with("non-music") { return 0.00; }

    if full_lower.starts_with("classical") {
        return if sub_lower.contains("baroque") || sub_lower.contains("choral")
            || sub_lower.contains("renaissance") || sub_lower.contains("medieval")
            || sub_lower.contains("romantic") || sub_lower.contains("opera") { 0.05 } else { 0.10 };
    }

    if full_lower.starts_with("folk") || full_lower.starts_with("latin") { return 0.10; }

    if full_lower.starts_with("jazz") {
        return if sub_lower.contains("smooth") || sub_lower.contains("easy listening")
            || sub_lower.contains("bossa") || sub_lower.contains("cool jazz")
            || sub_lower.contains("swing") || sub_lower.contains("dixieland") { 0.15 }
        else { 0.25 };
    }

    if full_lower.starts_with("stage") { return 0.20; }

    if full_lower.starts_with("pop") {
        return if sub_lower.contains("ballad") || sub_lower.contains("light")
            || sub_lower.contains("chanson") || sub_lower.contains("vocal") { 0.15 }
        else { 0.28 };
    }

    if full_lower.starts_with("blues") { return 0.30; }

    if full_lower.starts_with("funk") || full_lower.starts_with("soul") {
        return if sub_lower.contains("soul") || sub_lower.contains("neo soul")
            || sub_lower.contains("boogie") || sub_lower.contains("disco")
            || sub_lower.contains("r&b") { 0.28 }
        else { 0.35 };
    }

    if full_lower.starts_with("reggae") {
        return if sub_lower.contains("lovers") || sub_lower.contains("pop")
            || sub_lower.contains("rocksteady") || sub_lower.contains("dub") { 0.20 }
        else { 0.35 };
    }

    // Hip Hop block (under rock)
    if full_lower.starts_with("hip hop") {
        return if sub_lower.contains("jazzy") || sub_lower.contains("instrumental")
            || sub_lower.contains("trip hop") || sub_lower.contains("cloud")
            || sub_lower.contains("conscious") || sub_lower.contains("pop rap") { 0.30 }
        else if sub_lower.contains("horrorcore") || sub_lower.contains("britcore") { 0.44 }
        else { 0.42 }; // boom bap, gangsta, crunk, trap, hardcore hip-hop
    }

    // Rock block
    if full_lower.starts_with("rock") {
        // Soft rock
        if sub_lower.contains("acoustic") || sub_lower.contains("soft rock")
            || sub_lower.contains("folk rock") || sub_lower.contains("dream pop")
            || sub_lower.contains("shoegaze") || sub_lower.contains("lo-fi")
            || sub_lower.contains("post rock") { return 0.30; }
        // Indie/classic
        if sub_lower.contains("indie") || sub_lower.contains("brit pop")
            || sub_lower.contains("pop rock") || sub_lower.contains("aor")
            || sub_lower.contains("classic rock") || sub_lower.contains("prog rock") { return 0.30; }
        // Mid rock
        if sub_lower.contains("blues rock") || sub_lower.contains("country rock")
            || sub_lower.contains("southern") || sub_lower.contains("psychedelic")
            || sub_lower.contains("garage") || sub_lower.contains("grunge")
            || sub_lower.contains("punk") || sub_lower.contains("emo")
            || sub_lower.contains("mod") { return 0.40; }
        // Hard rock
        if sub_lower.contains("hard rock") || sub_lower.contains("arena")
            || sub_lower.contains("stoner") { return 0.43; }
        // Heavy metal
        if sub_lower.contains("heavy metal") || sub_lower.contains("power metal")
            || sub_lower.contains("thrash") || sub_lower.contains("speed metal")
            || sub_lower.contains("nu metal") || sub_lower.contains("sludge")
            || sub_lower.contains("doom metal") { return 0.62; }
        // Extreme metal
        if sub_lower.contains("metalcore") || sub_lower.contains("deathcore")
            || sub_lower.contains("post-hardcore") || sub_lower.contains("melodic hardcore")
            || sub_lower.contains("hardcore") { return 0.65; }
        if sub_lower.contains("black metal") || sub_lower.contains("death metal")
            || sub_lower.contains("technical death") || sub_lower.contains("melodic death") { return 0.70; }
        if sub_lower.contains("grindcore") || sub_lower.contains("goregrind")
            || sub_lower.contains("pornogrind") || sub_lower.contains("noisecore")
            || sub_lower.contains("power violence") || sub_lower.contains("crust") { return 0.70; }
        if sub_lower.contains("atmospheric") || sub_lower.contains("depressive")
            || sub_lower.contains("funeral") || sub_lower.contains("viking") { return 0.72; }
        // Default rock
        return 0.40;
    }

    // Electronic block
    if full_lower.starts_with("electronic") || full_lower.contains("electro") {
        // Ambient/chill
        if sub_lower.contains("ambient") || sub_lower.contains("new age")
            || sub_lower.contains("downtempo") || sub_lower.contains("chillwave") { return 0.20; }
        if sub_lower.contains("dark ambient") || sub_lower.contains("drone")
            || sub_lower.contains("berlin-school") || sub_lower.contains("dungeon") { return 0.25; }
        // Deep/mellow electronic
        if sub_lower.contains("trip hop") || sub_lower.contains("deep house")
            || sub_lower.contains("dub techno") || sub_lower.contains("leftfield")
            || sub_lower.contains("broken beat") { return 0.28; }
        // Synth/experimental
        if sub_lower.contains("synth-pop") || sub_lower.contains("synthwave")
            || sub_lower.contains("vaporwave") || sub_lower.contains("electroclash")
            || sub_lower.contains("dance-pop") { return 0.32; }
        if sub_lower.contains("idm") || sub_lower.contains("glitch")
            || sub_lower.contains("experimental") || sub_lower.contains("abstract")
            || sub_lower.contains("musique") { return 0.32; }
        // House/disco
        if sub_lower.contains("house") || sub_lower.contains("disco")
            || sub_lower.contains("nu-disco") { return 0.35; }
        // Electro/EBM
        if sub_lower.contains("electro") || sub_lower.contains("ebm") { return 0.37; }
        // Techno/trance/dubstep
        if sub_lower.contains("techno") || sub_lower.contains("minimal")
            || sub_lower.contains("tech house") { return 0.47; }
        if sub_lower.contains("trance") || sub_lower.contains("goa") { return 0.47; }
        if sub_lower.contains("dubstep") || sub_lower.contains("bassline")
            || sub_lower.contains("grime") || sub_lower.contains("uk garage")
            || sub_lower.contains("speed garage") { return 0.47; }
        // Hard electronic
        if sub_lower.contains("hard house") || sub_lower.contains("hard trance")
            || sub_lower.contains("hard techno") || sub_lower.contains("schranz") { return 0.50; }
        // Breakbeat (just under DnB)
        if sub_lower.contains("breakbeat") || sub_lower.contains("breaks")
            || sub_lower.contains("progressive breaks") || sub_lower.contains("big beat") { return 0.53; }
        // DnB — highest standard electronic
        if sub_lower.contains("drum n bass") || sub_lower.contains("jungle")
            || sub_lower.contains("halftime") { return 0.55; }
        // Hardcore electronic
        if sub_lower.contains("hardcore") || sub_lower.contains("hardstyle")
            || sub_lower.contains("jumpstyle") || sub_lower.contains("donk")
            || sub_lower.contains("hands up") { return 0.58; }
        if sub_lower.contains("gabber") || sub_lower.contains("makina")
            || sub_lower.contains("hi nrg") { return 0.60; }
        // Extreme electronic
        if sub_lower.contains("breakcore") || sub_lower.contains("speedcore")
            || sub_lower.contains("rhythmic noise") || sub_lower.contains("power electronics")
            || sub_lower.contains("industrial") { return 0.68; }
        if sub_lower.contains("noise") { return 0.75; }
        // Default electronic
        return 0.40;
    }

    // Fallback for unknown genres
    0.35
}

/// Mood tag weights for aggression estimation.
/// Positive = boosts aggression, negative = reduces aggression.
pub fn mood_tag_weight(tag: &str) -> f32 {
    match tag {
        // Boost aggression
        "heavy" | "powerful" | "energetic" => 0.30,
        "dark" | "dramatic" | "epic" | "action" => 0.25,
        "fast" | "sport" => 0.20,
        "deep" | "trailer" | "adventure" => 0.15,
        "party" | "groovy" | "cool" => 0.10,
        // Reduce aggression
        "calm" | "relaxing" | "soft" | "meditative" => -0.30,
        "romantic" | "love" | "sad" | "melancholic" => -0.25,
        "slow" | "dream" | "nature" | "soundscape" | "space" => -0.20,
        "happy" | "hopeful" | "uplifting" | "children" => -0.15,
        "melodic" | "emotional" | "inspiring" | "positive" => -0.10,
        "funny" | "fun" | "christmas" | "holiday" | "summer" => -0.05,
        // Neutral
        _ => 0.0,
    }
}

/// Compute per-track aggression estimate from genre label + mood tags.
/// Returns a value in [0.0, 1.0].
pub fn compute_track_aggression(
    genre: &str,
    mood_themes: Option<&Vec<(String, f32)>>,
) -> f32 {
    let genre_score = genre_aggression_score(genre); // 0.0–0.75

    let mood_score = mood_themes
        .map(|tags| {
            tags.iter()
                .map(|(tag, prob)| prob * mood_tag_weight(tag))
                .sum::<f32>()
        })
        .unwrap_or(0.0); // typically -0.5 to +0.5

    // Combine: 60% genre (reliable coarse signal) + 40% mood (noisy but fine-grained)
    (0.6 * genre_score + 0.4 * (mood_score + 0.5).clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// Find the PCA dimension most correlated with aggression estimates.
///
/// Returns (dimension_index, sign, correlation).
/// sign: multiply PCA value by this to get "higher = more aggressive".
pub fn find_aggression_axis(
    pca_data: &[(i64, Vec<f32>)],
    aggression_estimates: &HashMap<i64, f32>,
    max_dims_to_check: usize,
) -> Option<(usize, f32, f32)> {
    if pca_data.is_empty() { return None; }
    let pca_dim = pca_data[0].1.len();
    let check_dims = pca_dim.min(max_dims_to_check);

    // Build paired vectors (pca_value, aggression) for tracks that have both
    let paired: Vec<(i64, f32)> = pca_data.iter()
        .filter_map(|(id, _)| aggression_estimates.get(id).map(|a| (*id, *a)))
        .collect();

    if paired.len() < 10 { return None; }

    let n = paired.len() as f32;
    let aggr_vals: Vec<f32> = paired.iter().map(|(id, _)| aggression_estimates[id]).collect();
    let aggr_mean = aggr_vals.iter().sum::<f32>() / n;
    let aggr_var: f32 = aggr_vals.iter().map(|a| (a - aggr_mean).powi(2)).sum();

    if aggr_var < 1e-10 { return None; } // no variance in aggression estimates

    let mut best = (0usize, 0.0f32, 0.0f32); // (dim, sign, |r|)

    // Build a map for fast PCA lookup
    let pca_map: HashMap<i64, &Vec<f32>> = pca_data.iter().map(|(id, v)| (*id, v)).collect();

    for dim in 0..check_dims {
        let vals: Vec<f32> = paired.iter()
            .filter_map(|(id, _)| pca_map.get(id).map(|v| v[dim]))
            .collect();
        if vals.len() != paired.len() { continue; }

        let mean = vals.iter().sum::<f32>() / n;
        let var: f32 = vals.iter().map(|v| (v - mean).powi(2)).sum();
        if var < 1e-10 { continue; }

        let cov: f32 = vals.iter().zip(aggr_vals.iter())
            .map(|(v, a)| (v - mean) * (a - aggr_mean))
            .sum();
        let r = cov / (var * aggr_var).sqrt();

        if r.abs() > best.2 {
            best = (dim, if r > 0.0 { 1.0 } else { -1.0 }, r.abs());
        }
    }

    if best.2 < 0.01 { return None; } // no meaningful correlation found

    let sign = if best.1 > 0.0 { 1.0 } else { -1.0 };
    let r_signed = best.2 * sign;
    Some((best.0, sign, r_signed))
}
