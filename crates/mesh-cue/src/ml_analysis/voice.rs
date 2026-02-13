//! Vocal presence detection via RMS energy analysis
//!
//! Computes a 0.0–1.0 vocal presence score from the separated vocal stem.
//! No ML model needed — pure Rust signal processing.

/// Compute vocal presence from separated vocal stem audio.
///
/// Divides the audio into 1-second windows and counts the fraction of windows
/// where the RMS energy exceeds a threshold. Returns a ratio from 0.0
/// (instrumental) to 1.0 (vocal throughout).
///
/// # Arguments
/// * `vocal_mono` - Mono vocal stem samples (any sample rate)
/// * `sample_rate` - Sample rate of the audio (e.g., 44100)
pub fn compute_vocal_presence(vocal_mono: &[f32], sample_rate: u32) -> f32 {
    if vocal_mono.is_empty() || sample_rate == 0 {
        return 0.0;
    }

    let segment_size = sample_rate as usize; // 1-second windows
    let threshold = 0.01_f32; // RMS threshold for "vocal present"

    let mut vocal_segments = 0u32;
    let mut total_segments = 0u32;

    for chunk in vocal_mono.chunks(segment_size) {
        total_segments += 1;

        // Compute RMS for this segment
        let sum_sq: f32 = chunk.iter().map(|&s| s * s).sum();
        let rms = (sum_sq / chunk.len() as f32).sqrt();

        if rms > threshold {
            vocal_segments += 1;
        }
    }

    if total_segments == 0 {
        return 0.0;
    }

    vocal_segments as f32 / total_segments as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silent_audio_is_instrumental() {
        let silence = vec![0.0f32; 44100 * 10]; // 10 seconds of silence
        let result = compute_vocal_presence(&silence, 44100);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_loud_audio_is_vocal() {
        // Loud sine-like signal
        let loud: Vec<f32> = (0..44100 * 10)
            .map(|i| (i as f32 * 0.01).sin() * 0.5)
            .collect();
        let result = compute_vocal_presence(&loud, 44100);
        assert!(result > 0.9, "Loud audio should be detected as vocal: {}", result);
    }

    #[test]
    fn test_mixed_audio() {
        let mut audio = vec![0.0f32; 44100 * 10]; // 10 seconds
        // Make first 5 seconds loud (vocal)
        for i in 0..(44100 * 5) {
            audio[i] = (i as f32 * 0.01).sin() * 0.5;
        }
        let result = compute_vocal_presence(&audio, 44100);
        assert!((result - 0.5).abs() < 0.15, "Half vocal should be ~0.5: {}", result);
    }

    #[test]
    fn test_empty_audio() {
        assert_eq!(compute_vocal_presence(&[], 44100), 0.0);
    }

    #[test]
    fn test_zero_sample_rate() {
        assert_eq!(compute_vocal_presence(&[0.5], 0), 0.0);
    }
}
