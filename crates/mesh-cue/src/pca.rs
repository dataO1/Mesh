//! PCA dimension reduction for the HNSW similarity index.
//!
//! Reduces 1280-dim EffNet embeddings to 128-dim PCA projections. This addresses the
//! "concentration of measure" problem: in very high dimensions, cosine distances between
//! all pairs converge to the same value, making HNSW results nearly random. After PCA,
//! the 128-dim space retains the directions of maximum variance and produces much more
//! discriminative distance rankings.
//!
//! # Algorithm
//! 1. Compute column means (center the embedding cloud)
//! 2. Build [N × D] centered matrix via faer
//! 3. Run full SVD — right singular vectors V are the principal component axes
//! 4. Keep first `n_components` columns of V (highest-variance directions)
//! 5. At query time: `project(v) = normalize((v - mean) @ V[:, :k])`

use faer::prelude::*;

/// A trained PCA projection that maps high-dimensional embeddings to a compact space.
pub struct PcaProjection {
    /// Column means of the training data, used to center inputs before projection. Shape: [dim]
    pub mean: Vec<f32>,
    /// Principal component vectors, stored row-major: `[n_components × dim]`.
    /// Row `k` is the k-th principal component (right singular vector of the centered matrix).
    pub components: Vec<f32>,
    pub n_components: usize,
    pub dim: usize,
}

impl PcaProjection {
    /// Project a raw embedding to the PCA space and L2-normalise the result.
    ///
    /// Returns a vector of length `n_components`. Returns a zero-vector if the
    /// input has wrong dimensionality (graceful degradation).
    pub fn project(&self, v: &[f32]) -> Vec<f32> {
        if v.len() != self.dim {
            return vec![0.0f32; self.n_components];
        }
        // Center
        let centered: Vec<f32> = v.iter().zip(&self.mean).map(|(x, m)| x - m).collect();

        // Multiply by components: result[k] = dot(centered, components[k])
        let mut result = vec![0.0f32; self.n_components];
        for k in 0..self.n_components {
            let base = k * self.dim;
            result[k] = centered.iter()
                .zip(&self.components[base..base + self.dim])
                .map(|(c, w)| c * w)
                .sum();
        }

        // L2 normalise (geodesic point on unit hypersphere)
        let norm = result.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-9 {
            result.iter_mut().for_each(|x| *x /= norm);
        }
        result
    }
}

/// Compute a PCA projection from a batch of labelled embeddings.
///
/// # Arguments
/// - `embeddings`: `(track_id, embedding_vec)` pairs; all vecs must have the same length.
/// - `n_components`: number of output dimensions. `None` = auto-detect via 95% explained
///   variance threshold (adapts to the library's intrinsic dimensionality).
///
/// # Returns
/// A [`PcaProjection`] ready for use with [`PcaProjection::project`].
///
/// # Errors
/// Returns an error string if the input is empty, has inconsistent dimensions,
/// or if the SVD fails.
pub fn compute_pca_projection(
    embeddings: &[(i64, Vec<f32>)],
    n_components: Option<usize>,
) -> Result<PcaProjection, String> {
    if embeddings.is_empty() {
        return Err("No embeddings provided".to_string());
    }
    let n = embeddings.len();
    let dim = embeddings[0].1.len();
    if dim == 0 {
        return Err("Zero-length embeddings".to_string());
    }

    // Step 1: Column means
    let mut mean = vec![0.0f32; dim];
    for (_, v) in embeddings {
        if v.len() != dim {
            return Err(format!("Inconsistent embedding dimension: expected {}, got {}", dim, v.len()));
        }
        for (i, &x) in v.iter().enumerate() {
            mean[i] += x;
        }
    }
    mean.iter_mut().for_each(|x| *x /= n as f32);

    // Step 2: Build [N × D] centered matrix (f64 for numerical stability)
    let mat: faer::Mat<f64> = faer::Mat::from_fn(n, dim, |i, j| {
        (embeddings[i].1[j] - mean[j]) as f64
    });

    // Step 3: SVD — the V matrix contains right singular vectors (principal components)
    let svd = mat.svd();
    let v = svd.v(); // [dim × min(N, dim)]
    let s = svd.s_diagonal(); // singular values (descending)

    // Step 4: Determine number of components
    let max_k = v.ncols().min(dim).min(n);
    let k = match n_components {
        Some(fixed) => fixed.min(max_k),
        None => {
            // Auto-detect: keep components until 95% of total variance is explained.
            // Variance per component = singular_value² / (n - 1).
            // We only need ratios, so singular_value² suffices.
            let total_var: f64 = (0..s.nrows()).map(|i| s.read(i).powi(2)).sum();
            if total_var < 1e-10 {
                max_k // degenerate case: keep all
            } else {
                let threshold = 0.95;
                let mut cum_var = 0.0;
                let mut auto_k = max_k;
                for i in 0..max_k {
                    cum_var += s.read(i).powi(2);
                    if cum_var / total_var >= threshold {
                        auto_k = (i + 1).max(20); // floor at 20 dims
                        break;
                    }
                }
                auto_k.min(max_k).min(256) // ceiling at 256
            }
        }
    };

    // Step 5: Extract first k columns of V as f32 principal components
    let mut components = vec![0.0f32; k * dim];
    for ki in 0..k {
        for d in 0..dim {
            components[ki * dim + d] = v.read(d, ki) as f32;
        }
    }

    // Log explained variance breakdown
    let total_var: f64 = (0..s.nrows()).map(|i| s.read(i).powi(2)).sum();
    let kept_var: f64 = (0..k).map(|i| s.read(i).powi(2)).sum();
    let explained_pct = if total_var > 0.0 { kept_var / total_var * 100.0 } else { 0.0 };
    log::info!(
        "[PCA] Projection built: n={}, dim={} → k={} components ({:.1}% variance explained)",
        n, dim, k, explained_pct
    );

    Ok(PcaProjection { mean, components, n_components: k, dim })
}
