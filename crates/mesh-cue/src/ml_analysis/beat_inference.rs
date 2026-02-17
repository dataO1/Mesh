//! Beat This! ONNX-based beat and downbeat detection
//!
//! Uses the Beat This! model (CPJKU, ISMIR 2024) for SOTA beat tracking.
//! The small variant (~2M params, ~8 MB) achieves Beat F1 = 88.8 on GTZAN.
//!
//! # Architecture
//!
//! 1. Audio → mel spectrogram (128 bands, 50 fps at 22050 Hz)
//! 2. Chunked inference (1500 frames = 30s per chunk, with cosine overlap blending)
//! 3. Peak picking on activation curves (no DBN, no octave errors)
//! 4. BPM computed from median inter-beat interval
//!
//! Unlike Essentia's RhythmExtractor2013, this approach:
//! - Has no half-tempo problem (no Dynamic Bayesian Network)
//! - Provides built-in downbeat detection
//! - Is thread-safe (ort, unlike Essentia's C++ globals)

use std::path::Path;

use ndarray::Array3;
use ort::session::Session;
use ort::value::Tensor;

use super::models::MlModelType;
use super::preprocessing::BeatThisMelResult;

/// Beat This! model chunk size in frames (30 seconds at 50 fps)
const CHUNK_SIZE: usize = 1500;

/// Overlap between chunks in frames (5 seconds)
const CHUNK_OVERLAP: usize = 250;

/// Minimum activation threshold for peak picking (probability after sigmoid).
/// Matches Beat This! "minimal" postprocessor which uses logit > 0 (= probability > 0.5).
const BEAT_THRESHOLD: f32 = 0.5;

/// Minimum inter-beat distance in frames (corresponds to ~250 BPM at 50 fps)
const MIN_BEAT_DISTANCE: usize = 12;

/// Max-pool kernel size for peak picking (matches official Beat This! postprocessor).
/// 7 frames = ±3 frames = ±60ms at 50 fps.
const PEAK_POOL_KERNEL: usize = 7;

/// Beat This! inference engine
///
/// Holds a pre-loaded ONNX session for beat + downbeat activation prediction.
/// Thread-safe (ort Sessions are Send+Sync), but `run()` requires `&mut self`.
pub struct BeatThisAnalyzer {
    session: Session,
}

// Safety: ort::Session is Send+Sync by design
unsafe impl Send for BeatThisAnalyzer {}
unsafe impl Sync for BeatThisAnalyzer {}

/// Result of Beat This! inference
#[derive(Debug, Clone)]
pub struct BeatThisResult {
    /// Detected beat positions in seconds
    pub beat_times: Vec<f64>,
    /// Detected downbeat positions in seconds
    pub downbeat_times: Vec<f64>,
    /// Computed BPM from median inter-beat interval
    pub bpm: f64,
    /// Mean beat activation strength (confidence proxy, 0.0-1.0)
    pub confidence: f32,
}

impl BeatThisAnalyzer {
    /// Create a new analyzer by loading the Beat This! ONNX model.
    ///
    /// # Arguments
    /// * `model_dir` - Directory containing beat_this_small.onnx
    pub fn new(model_dir: &Path) -> Result<Self, String> {
        let model_path = model_dir.join(MlModelType::BeatThis.filename());

        if !model_path.exists() {
            return Err(format!("Beat This! model not found: {:?}", model_path));
        }

        let session = Session::builder()
            .and_then(|b| b.with_intra_threads(4))
            .and_then(|b| b.commit_from_file(&model_path))
            .map_err(|e| format!("Failed to load Beat This! model: {}", e))?;

        log::info!("Beat This! model loaded from {:?}", model_path);

        Ok(Self { session })
    }

    /// Run beat detection on a mel spectrogram.
    ///
    /// Chunks the spectrogram into 30-second segments with overlap,
    /// runs inference on each, blends with cosine weighting, then
    /// applies peak picking to extract beat and downbeat positions.
    pub fn detect_beats(&mut self, mel: &BeatThisMelResult) -> Result<BeatThisResult, String> {
        let n_frames = mel.frames.len();
        if n_frames == 0 {
            return Err("Empty mel spectrogram".to_string());
        }

        log::info!(
            "Beat This! inference: {} frames ({:.1}s at {} fps)",
            n_frames,
            n_frames as f32 / mel.fps,
            mel.fps
        );

        // Run chunked inference with overlap blending
        let (beat_activations, downbeat_activations) = self.run_chunked(mel)?;

        // Peak picking
        let beat_frames = pick_peaks(&beat_activations, BEAT_THRESHOLD, MIN_BEAT_DISTANCE);
        let downbeat_frames = pick_peaks(&downbeat_activations, BEAT_THRESHOLD, MIN_BEAT_DISTANCE);

        // Convert frame indices to seconds
        let frame_to_sec = |frame: usize| frame as f64 / mel.fps as f64;
        let beat_times: Vec<f64> = beat_frames.iter().map(|&f| frame_to_sec(f)).collect();
        let downbeat_times: Vec<f64> = downbeat_frames.iter().map(|&f| frame_to_sec(f)).collect();

        // Compute BPM from median inter-beat interval
        let bpm = compute_bpm_from_beats(&beat_times);

        // Confidence from mean peak activation
        let confidence = if !beat_frames.is_empty() {
            let sum: f32 = beat_frames.iter().map(|&f| beat_activations[f]).sum();
            sum / beat_frames.len() as f32
        } else {
            0.0
        };

        log::info!(
            "Beat This! result: {} beats, {} downbeats, BPM={:.1}, confidence={:.2}",
            beat_times.len(),
            downbeat_times.len(),
            bpm,
            confidence
        );

        Ok(BeatThisResult {
            beat_times,
            downbeat_times,
            bpm,
            confidence,
        })
    }

    /// Run chunked inference with cosine-weighted overlap blending.
    ///
    /// Splits the mel spectrogram into CHUNK_SIZE-frame segments with CHUNK_OVERLAP
    /// overlap. Each chunk is run through the model independently, then overlapping
    /// regions are blended with cosine weighting for smooth transitions.
    fn run_chunked(
        &mut self,
        mel: &BeatThisMelResult,
    ) -> Result<(Vec<f32>, Vec<f32>), String> {
        let n_frames = mel.frames.len();
        let n_bands = mel.n_bands;

        // For short audio that fits in one chunk, run directly
        if n_frames <= CHUNK_SIZE {
            return self.run_single_chunk(&mel.frames);
        }

        // Allocate output buffers
        let mut beat_sum = vec![0.0f32; n_frames];
        let mut downbeat_sum = vec![0.0f32; n_frames];
        let mut weight_sum = vec![0.0f32; n_frames];

        // Process chunks with overlap
        let step = CHUNK_SIZE - CHUNK_OVERLAP;
        let mut start = 0;

        while start < n_frames {
            let end = (start + CHUNK_SIZE).min(n_frames);
            let chunk_len = end - start;

            // Extract chunk frames (pad with zeros if needed)
            let chunk: Vec<Vec<f32>> = if chunk_len < CHUNK_SIZE {
                let mut padded = mel.frames[start..end].to_vec();
                while padded.len() < CHUNK_SIZE {
                    padded.push(vec![0.0; n_bands]);
                }
                padded
            } else {
                mel.frames[start..end].to_vec()
            };

            // Run inference on chunk
            let (chunk_beats, chunk_downbeats) = self.run_single_chunk(&chunk)?;

            // Cosine blending weights
            for i in 0..chunk_len {
                let weight = cosine_blend_weight(i, chunk_len, CHUNK_OVERLAP);
                let global_idx = start + i;

                beat_sum[global_idx] += chunk_beats[i] * weight;
                downbeat_sum[global_idx] += chunk_downbeats[i] * weight;
                weight_sum[global_idx] += weight;
            }

            start += step;
        }

        // Normalize by weight sum
        for i in 0..n_frames {
            if weight_sum[i] > 0.0 {
                beat_sum[i] /= weight_sum[i];
                downbeat_sum[i] /= weight_sum[i];
            }
        }

        Ok((beat_sum, downbeat_sum))
    }

    /// Run the model on a single chunk of mel spectrogram frames.
    ///
    /// Input: [1, n_frames, 128] → Output: beat_activation [1, n_frames], downbeat_activation [1, n_frames]
    /// The model outputs logits; we apply sigmoid to get probabilities.
    fn run_single_chunk(
        &mut self,
        frames: &[Vec<f32>],
    ) -> Result<(Vec<f32>, Vec<f32>), String> {
        let n_frames = frames.len();
        let n_bands = if n_frames > 0 { frames[0].len() } else { 128 };

        // Flatten frames into [1, n_frames, n_bands] tensor
        let mut flat = Vec::with_capacity(n_frames * n_bands);
        for frame in frames {
            flat.extend_from_slice(frame);
        }

        // Beat This! expects 3D: [batch=1, time, mel_bands=128]
        let input = Array3::from_shape_vec((1, n_frames, n_bands), flat)
            .map_err(|e| format!("Beat This! input shape error: {}", e))?;

        let input_tensor = Tensor::from_array(input)
            .map_err(|e| format!("Beat This! tensor creation error: {}", e))?;

        let outputs = self.session.run(
            ort::inputs!["mel_spectrogram" => input_tensor]
        ).map_err(|e| format!("Beat This! inference error: {}", e))?;

        // Extract outputs: [0]=beat_activation [1,n], [1]=downbeat_activation [1,n]
        // Model outputs are logits — apply sigmoid to convert to probabilities
        let mut output_iter = outputs.iter();

        let (_, beat_value) = output_iter.next()
            .ok_or("Beat This! produced no output")?;
        let (_shape, beat_data) = beat_value.try_extract_tensor::<f32>()
            .map_err(|e| format!("Beat activation extraction error: {}", e))?;
        let beat_activations: Vec<f32> = beat_data.iter()
            .map(|&logit| sigmoid(logit))
            .collect();

        let downbeat_activations = if let Some((_, db_value)) = output_iter.next() {
            let (_shape, db_data) = db_value.try_extract_tensor::<f32>()
                .map_err(|e| format!("Downbeat activation extraction error: {}", e))?;
            db_data.iter().map(|&logit| sigmoid(logit)).collect()
        } else {
            log::warn!("Beat This! model has only one output, no downbeat detection");
            vec![0.0; beat_activations.len()]
        };

        Ok((beat_activations, downbeat_activations))
    }
}

/// Sigmoid function: converts logits to probabilities.
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Cosine blending weight for overlap regions.
///
/// Returns 1.0 in the middle of a chunk, ramping down with cosine weighting
/// at the overlap boundaries for smooth transitions.
fn cosine_blend_weight(frame_in_chunk: usize, chunk_len: usize, overlap: usize) -> f32 {
    if overlap == 0 || chunk_len <= overlap {
        return 1.0;
    }

    let f = frame_in_chunk as f32;
    let half_overlap = overlap as f32 / 2.0;

    // Ramp up at start
    if f < half_overlap {
        return 0.5 * (1.0 - (std::f32::consts::PI * f / half_overlap).cos());
    }

    // Ramp down at end
    let frames_from_end = (chunk_len - 1 - frame_in_chunk) as f32;
    if frames_from_end < half_overlap {
        return 0.5 * (1.0 - (std::f32::consts::PI * frames_from_end / half_overlap).cos());
    }

    1.0
}

/// Pick peaks from an activation curve using max-pool, matching Beat This!
/// "minimal" postprocessor.
///
/// Algorithm (from `beat_this/model/postprocessor.py`):
/// 1. 1D max-pool with kernel=7 (±3 frames = ±60ms) to find local maxima
/// 2. Keep frames that equal their max-pooled value AND exceed threshold
/// 3. Deduplicate adjacent peaks by keeping the highest
/// 4. Enforce minimum inter-peak distance
fn pick_peaks(activations: &[f32], threshold: f32, min_distance: usize) -> Vec<usize> {
    let n = activations.len();
    if n < 3 {
        return Vec::new();
    }

    let half_k = PEAK_POOL_KERNEL / 2; // 3 for kernel=7

    // Step 1: Max-pool with kernel=PEAK_POOL_KERNEL, stride=1, padding=half_k
    // A frame is a peak if it equals the max in its ±half_k neighborhood
    let mut peaks = Vec::new();
    for i in 0..n {
        if activations[i] < threshold {
            continue;
        }

        // Check if this frame is the max within ±half_k
        let start = i.saturating_sub(half_k);
        let end = (i + half_k + 1).min(n);
        let mut is_max = true;
        for j in start..end {
            if activations[j] > activations[i] {
                is_max = false;
                break;
            }
        }

        if is_max {
            peaks.push(i);
        }
    }

    // Step 2: Deduplicate adjacent peaks (keep highest in each cluster)
    // Adjacent = within 1 frame of each other
    if peaks.len() < 2 {
        return peaks;
    }

    let mut deduped = Vec::with_capacity(peaks.len());
    let mut cluster_start = 0;
    for i in 1..=peaks.len() {
        let end_cluster = i == peaks.len() || peaks[i] - peaks[i - 1] > 1;
        if end_cluster {
            // Find the highest peak in this cluster
            let best = (cluster_start..i)
                .max_by(|&a, &b| {
                    activations[peaks[a]]
                        .partial_cmp(&activations[peaks[b]])
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();
            deduped.push(peaks[best]);
            cluster_start = i;
        }
    }

    // Step 3: Enforce minimum inter-peak distance
    let mut final_peaks = Vec::with_capacity(deduped.len());
    for &peak in &deduped {
        if let Some(&last) = final_peaks.last() {
            if peak - last < min_distance {
                // Keep the higher peak
                if activations[peak] > activations[last] {
                    final_peaks.pop();
                    final_peaks.push(peak);
                }
                continue;
            }
        }
        final_peaks.push(peak);
    }

    final_peaks
}

/// Compute BPM from detected beat positions using trimmed mean of inter-beat intervals.
///
/// At 50 fps, beat positions are quantized to 20ms frames. For ~174 BPM,
/// the true IBI of 17.24 frames means consecutive beats alternate between
/// 17 and 18 frames apart. The median would snap to exactly 17 (= 176.47 BPM)
/// or 18 (= 166.67 BPM), never the true value. The trimmed mean averages out
/// this quantization error over hundreds of beats for sub-frame precision.
fn compute_bpm_from_beats(beat_times: &[f64]) -> f64 {
    if beat_times.len() < 2 {
        return 120.0; // Default fallback
    }

    // Compute all consecutive inter-beat intervals
    let mut ibis: Vec<f64> = beat_times.windows(2)
        .map(|w| w[1] - w[0])
        .filter(|&ibi| ibi > 0.1 && ibi < 3.0) // Filter unreasonable intervals (20-600 BPM range)
        .collect();

    if ibis.is_empty() {
        return 120.0;
    }

    // Sort for trimmed mean (robust to outliers from breakdowns/intros)
    ibis.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // Trim 10% from each end to remove outliers, then take the mean.
    // The mean of the remaining IBIs averages out the ±1 frame quantization
    // that would make the median snap to exactly 17 or 18 frames.
    let trim = ibis.len() / 10;
    let trimmed = if trim > 0 && ibis.len() > 2 * trim + 1 {
        &ibis[trim..ibis.len() - trim]
    } else {
        &ibis[..]
    };

    let mean_ibi = trimmed.iter().sum::<f64>() / trimmed.len() as f64;

    // Convert inter-beat interval to BPM
    let bpm = 60.0 / mean_ibi;

    // Round to reasonable precision (0.01 BPM)
    (bpm * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_peaks_basic() {
        // Peaks well-separated (>PEAK_POOL_KERNEL apart)
        let mut activations = vec![0.0; 20];
        activations[3] = 0.8;
        activations[14] = 0.9;
        let peaks = pick_peaks(&activations, 0.3, 1);
        assert_eq!(peaks, vec![3, 14]);
    }

    #[test]
    fn test_pick_peaks_max_pool_suppression() {
        // Two peaks within the max-pool window (kernel=7, ±3 frames)
        // Only the higher one should survive
        let mut activations = vec![0.0; 15];
        activations[5] = 0.6;
        activations[7] = 0.8; // within 3 frames of index 5
        let peaks = pick_peaks(&activations, 0.3, 1);
        assert_eq!(peaks, vec![7]); // Only the higher peak survives max-pool
    }

    #[test]
    fn test_pick_peaks_min_distance() {
        // Two peaks far enough from max-pool but within min_distance
        let mut activations = vec![0.0; 30];
        activations[5] = 0.7;
        activations[15] = 0.9; // >7 frames apart (passes max-pool) but within min_distance=12
        let peaks = pick_peaks(&activations, 0.3, 12);
        assert_eq!(peaks, vec![15]); // Keep the higher peak
    }

    #[test]
    fn test_pick_peaks_below_threshold() {
        let activations = vec![0.0, 0.1, 0.2, 0.1, 0.0];
        let peaks = pick_peaks(&activations, 0.3, 1);
        assert!(peaks.is_empty());
    }

    #[test]
    fn test_compute_bpm_174() {
        // Simulate 174 BPM beats over 10 seconds
        let ibi = 60.0 / 174.0; // ~0.3448 seconds
        let beat_times: Vec<f64> = (0..30).map(|i| i as f64 * ibi).collect();
        let bpm = compute_bpm_from_beats(&beat_times);
        assert!((bpm - 174.0).abs() < 0.1, "Expected ~174 BPM, got {}", bpm);
    }

    #[test]
    fn test_compute_bpm_120() {
        let ibi = 0.5; // 120 BPM
        let beat_times: Vec<f64> = (0..20).map(|i| i as f64 * ibi).collect();
        let bpm = compute_bpm_from_beats(&beat_times);
        assert!((bpm - 120.0).abs() < 0.1, "Expected ~120 BPM, got {}", bpm);
    }

    #[test]
    fn test_compute_bpm_empty() {
        assert_eq!(compute_bpm_from_beats(&[]), 120.0);
        assert_eq!(compute_bpm_from_beats(&[1.0]), 120.0);
    }

    #[test]
    fn test_compute_bpm_frame_quantized() {
        // Simulate 174 BPM beats quantized to 50fps frames (the real-world case).
        // True IBI = 17.24 frames → beats alternate between 17 and 18 frames apart.
        // The old median approach would give 176.47 BPM (17 frames); trimmed mean
        // should recover ~174 BPM.
        let fps = 50.0;
        let true_bpm = 174.0;
        let true_ibi_frames = 60.0 * fps / true_bpm; // 17.24 frames
        let mut beat_times = Vec::new();
        let mut pos: f64 = 0.0;
        for _ in 0..500 {
            // Quantize to integer frames, then convert to seconds
            let frame = pos.round() as usize;
            beat_times.push(frame as f64 / fps);
            pos += true_ibi_frames;
        }
        let bpm = compute_bpm_from_beats(&beat_times);
        assert!(
            (bpm - true_bpm).abs() < 1.0,
            "Frame-quantized 174 BPM should recover ~174, got {}",
            bpm
        );
        // Must be much closer than the old median (176.47)
        assert!(
            (bpm - true_bpm).abs() < (176.47 - true_bpm).abs(),
            "Trimmed mean ({}) should be closer to true BPM than median (176.47)",
            bpm
        );
    }

    #[test]
    fn test_cosine_blend_middle() {
        assert!((cosine_blend_weight(500, 1500, 250) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_blend_edges() {
        // At very start, weight should be near 0
        let w = cosine_blend_weight(0, 1500, 250);
        assert!(w < 0.1, "Edge weight should be near 0, got {}", w);
    }
}
