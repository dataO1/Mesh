//! ONNX-based ML inference for audio analysis
//!
//! Runs EffNet embedding + optional mood classification head using ort (ONNX Runtime).
//! The `MlAnalyzer` holds pre-loaded sessions and can be wrapped in
//! `Arc<Mutex<>>` for sharing across rayon workers (`run()` requires `&mut self`).
//!
//! # Architecture
//!
//! The EffNet model produces BOTH genre predictions (400-class) AND 1280-dim
//! embeddings in a single forward pass — no separate genre head is needed.
//! The Essentia hub only has the genre head as TensorFlow `.pb`, not ONNX.
//!
//! Arousal/valence is derived from Jamendo mood predictions (no EffNet-compatible
//! A/V regression model exists — DEAM/emoMusic heads require MusiCNN 200-dim).

use std::path::Path;
use ndarray::{Array2, Array3};
use ort::session::Session;
use ort::value::Tensor;
use mesh_core::db::MlAnalysisData;

use super::models::MlModelType;
use super::preprocessing::MelSpectrogramResult;

/// Number of mel spectrogram frames expected by EffNet.
/// EffNet input shape: [batch, 128 frames, 96 mel bands] at 16kHz.
const PATCH_SIZE: usize = 128;

/// Number of mel bands (frequency axis)
const N_BANDS: usize = 96;

/// ML analysis engine with pre-loaded ONNX sessions
///
/// EffNet produces genre predictions + embeddings in one pass (no separate genre head).
/// Jamendo mood head is optional (experimental only), enables arousal derivation.
pub struct MlAnalyzer {
    effnet: Session,
    mood: Option<Session>,
    genre_labels: Vec<String>,
    mood_labels: Vec<String>,
}

// Safety: ort::Session is Send+Sync by design
unsafe impl Send for MlAnalyzer {}
unsafe impl Sync for MlAnalyzer {}

impl MlAnalyzer {
    /// Create a new analyzer with pre-loaded ONNX models.
    ///
    /// # Arguments
    /// * `model_dir` - Directory containing the ONNX model files
    /// * `experimental` - If true, also load Jamendo mood model (enables arousal derivation)
    pub fn new(model_dir: &Path, experimental: bool) -> Result<Self, String> {
        let effnet_path = model_dir.join(MlModelType::EffNetEmbedding.filename());

        if !effnet_path.exists() {
            return Err(format!("EffNet model not found: {:?}", effnet_path));
        }

        let effnet = Session::builder()
            .and_then(|b| b.with_intra_threads(1))
            .and_then(|b| b.commit_from_file(&effnet_path))
            .map_err(|e| format!("Failed to load EffNet: {}", e))?;

        let mood = if experimental {
            let mood_path = model_dir.join(MlModelType::JamendoMood.filename());
            if mood_path.exists() {
                Some(
                    Session::builder()
                        .and_then(|b| b.with_intra_threads(1))
                        .and_then(|b| b.commit_from_file(&mood_path))
                        .map_err(|e| format!("Failed to load mood model: {}", e))?
                )
            } else {
                log::warn!("Mood model not found, skipping");
                None
            }
        } else {
            None
        };

        Ok(Self {
            effnet,
            mood,
            genre_labels: discogs400_labels(),
            mood_labels: jamendo_mood_labels(),
        })
    }

    /// Run full ML analysis pipeline on a mel spectrogram.
    ///
    /// 1. Extract patches from mel spectrogram (128 frames x 96 bands each)
    /// 2. Run EffNet -> genre predictions + 1280-dim embeddings per patch
    /// 3. Average genre predictions and embeddings across patches
    /// 4. Decode genre labels from averaged predictions
    /// 5. Run mood head on averaged embedding (if experimental)
    /// 6. Derive arousal/valence from mood predictions (if available)
    pub fn analyze(
        &mut self,
        mel: &MelSpectrogramResult,
        vocal_presence: f32,
    ) -> Result<MlAnalysisData, String> {
        let patches = extract_patches(&mel.frames, PATCH_SIZE);
        if patches.is_empty() {
            return Err("Audio too short for ML analysis".to_string());
        }
        log::debug!("ML: running EffNet on {} patches ({} mel frames)", patches.len(), mel.frames.len());

        // Run EffNet on each patch — get both genre predictions and embeddings
        let mut all_genre_preds: Vec<Vec<f32>> = Vec::new();
        let mut all_embeddings: Vec<Vec<f32>> = Vec::new();
        for patch in &patches {
            let (genre_preds, embedding) = self.run_effnet(patch)?;
            all_genre_preds.push(genre_preds);
            all_embeddings.push(embedding);
        }

        // Average across patches
        let avg_genre_preds = average_embeddings(&all_genre_preds);
        let avg_embedding = average_embeddings(&all_embeddings);

        // Decode genre labels from EffNet's built-in genre output
        let (top_genre, genre_scores) = self.decode_genre_predictions(&avg_genre_preds);

        // Run mood classification head on averaged embedding
        let mood_themes = if self.mood.is_some() {
            Some(self.run_mood(&avg_embedding)?)
        } else {
            None
        };

        // Derive arousal/valence from mood predictions
        let (arousal, valence) = if let Some(ref moods) = mood_themes {
            derive_arousal_valence_from_mood(moods)
        } else {
            (None, None)
        };

        Ok(MlAnalysisData {
            vocal_presence,
            arousal,
            valence,
            top_genre,
            genre_scores,
            mood_themes,
        })
    }

    /// Run EffNet on a single patch -> (genre_preds [400], embedding [1280])
    ///
    /// EffNet input: [batch=1, 128 frames, 96 bands] (3D tensor)
    /// EffNet outputs: [0] genre predictions [1,400], [1] embedding [1,1280]
    fn run_effnet(&mut self, patch: &[Vec<f32>]) -> Result<(Vec<f32>, Vec<f32>), String> {
        let n_frames = patch.len();
        let n_bands = if n_frames > 0 { patch[0].len() } else { N_BANDS };

        let mut flat = Vec::with_capacity(n_frames * n_bands);
        for frame in patch {
            flat.extend_from_slice(frame);
        }

        // EffNet expects 3D: [batch, time_frames, mel_bands]
        let input = Array3::from_shape_vec((1, n_frames, n_bands), flat)
            .map_err(|e| format!("EffNet input shape error: {}", e))?;

        let input_name = "melspectrogram";

        let input_tensor = Tensor::from_array(input)
            .map_err(|e| format!("EffNet tensor creation error: {}", e))?;

        let outputs = self.effnet.run(
            ort::inputs![input_name => input_tensor]
        ).map_err(|e| format!("EffNet inference error: {}", e))?;

        // EffNet has 2 outputs: [0]=genre_preds [n,400], [1]=embedding [n,1280]
        let mut output_iter = outputs.iter();
        let (_, genre_value) = output_iter.next()
            .ok_or("EffNet produced no output")?;

        let (_shape, genre_data) = genre_value.try_extract_tensor::<f32>()
            .map_err(|e| format!("EffNet genre extraction error: {}", e))?;
        let genre_preds = genre_data.to_vec();

        // Second output is the embedding (fall back to genre if only one output)
        let embedding = if let Some((_, emb_value)) = output_iter.next() {
            let (_shape, emb_data) = emb_value.try_extract_tensor::<f32>()
                .map_err(|e| format!("EffNet embedding extraction error: {}", e))?;
            emb_data.to_vec()
        } else {
            log::warn!("EffNet has only one output, using it as embedding");
            genre_preds.clone()
        };

        Ok((genre_preds, embedding))
    }

    /// Decode genre labels from raw EffNet genre prediction probabilities
    fn decode_genre_predictions(&self, probs: &[f32]) -> (Option<String>, Vec<(String, f32)>) {
        let mut scored: Vec<(String, f32)> = probs
            .iter()
            .enumerate()
            .filter(|(_, &p)| p > 0.05)
            .map(|(i, &p)| {
                let label = self.genre_labels.get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("genre_{}", i));
                (label, p)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(10);

        // Store clean sub-genre as top_genre (e.g., "Breakcore" not "Electronic---Breakcore")
        let top_genre = scored.first().map(|(label, _)| {
            label.split_once("---").map_or(label.clone(), |(_, sub)| sub.to_string())
        });
        (top_genre, scored)
    }

    /// Run mood/theme classification on embedding
    fn run_mood(&mut self, embedding: &[f32]) -> Result<Vec<(String, f32)>, String> {
        let session = self.mood.as_mut().ok_or("No mood model")?;

        let input = Array2::from_shape_vec((1, embedding.len()), embedding.to_vec())
            .map_err(|e| format!("Mood input shape error: {}", e))?;

        // Input tensor name from ONNX model (converted from TF classification head)
        let input_name = "embeddings";

        let input_tensor = Tensor::from_array(input)
            .map_err(|e| format!("Mood tensor creation error: {}", e))?;

        let outputs = session.run(
            ort::inputs![input_name => input_tensor]
        ).map_err(|e| format!("Mood inference error: {}", e))?;

        let (_, first_value) = outputs.iter().next()
            .ok_or("Mood model produced no output")?;

        let (_shape, probs_data) = first_value.try_extract_tensor::<f32>()
            .map_err(|e| format!("Mood output extraction error: {}", e))?;

        let mut scored: Vec<(String, f32)> = probs_data
            .iter()
            .enumerate()
            .filter(|(_, &p)| p > 0.1)
            .map(|(i, &p)| {
                let label = self.mood_labels.get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("mood_{}", i));
                (label, p)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(5);

        Ok(scored)
    }
}

// ============================================================================
// Arousal/Valence Derivation from Mood Tags
// ============================================================================

/// Derive arousal and valence from Jamendo mood/theme predictions.
///
/// No EffNet-compatible arousal/valence regression model exists (DEAM/emoMusic
/// heads use MusiCNN 200-dim embeddings, not EffNet 1280-dim). Instead, we
/// approximate arousal and valence from the 56 Jamendo mood tags using
/// psychoacoustic mappings.
///
/// Arousal (energy/activation): high = energetic/fast/powerful, low = calm/slow/soft
/// Valence (positive/negative): high = happy/upbeat/fun, low = sad/dark/melancholic
fn derive_arousal_valence_from_mood(moods: &[(String, f32)]) -> (Option<f32>, Option<f32>) {
    if moods.is_empty() {
        return (None, None);
    }

    // Arousal weights: positive = high energy, negative = low energy
    const AROUSAL_WEIGHTS: &[(&str, f32)] = &[
        // High arousal
        ("energetic", 1.0), ("fast", 0.9), ("heavy", 0.8), ("powerful", 0.9),
        ("party", 0.8), ("action", 0.9), ("epic", 0.7), ("sport", 0.7),
        ("dramatic", 0.6), ("upbeat", 0.7), ("fun", 0.5), ("groovy", 0.4),
        // Low arousal
        ("calm", -1.0), ("relaxing", -0.9), ("meditative", -0.9), ("slow", -0.8),
        ("soft", -0.8), ("nature", -0.6), ("dream", -0.5), ("background", -0.5),
        ("soundscape", -0.4),
    ];

    // Valence weights: positive = happy/pleasant, negative = sad/dark
    const VALENCE_WEIGHTS: &[(&str, f32)] = &[
        // Positive valence
        ("happy", 1.0), ("upbeat", 0.8), ("uplifting", 0.9), ("fun", 0.8),
        ("positive", 0.9), ("hopeful", 0.7), ("inspiring", 0.7), ("summer", 0.6),
        ("romantic", 0.5), ("love", 0.5), ("cool", 0.3), ("funny", 0.6),
        // Negative valence
        ("sad", -1.0), ("dark", -0.8), ("melancholic", -0.9), ("dramatic", -0.4),
        ("heavy", -0.3), ("deep", -0.2), ("emotional", -0.3),
    ];

    let mut arousal_sum = 0.0_f32;
    let mut arousal_weight_sum = 0.0_f32;
    let mut valence_sum = 0.0_f32;
    let mut valence_weight_sum = 0.0_f32;

    for (label, prob) in moods {
        let label_lower = label.to_lowercase();
        for &(tag, weight) in AROUSAL_WEIGHTS {
            if label_lower == tag {
                arousal_sum += weight * prob;
                arousal_weight_sum += prob;
            }
        }
        for &(tag, weight) in VALENCE_WEIGHTS {
            if label_lower == tag {
                valence_sum += weight * prob;
                valence_weight_sum += prob;
            }
        }
    }

    // Normalize: weighted sum is in [-1, +1], map to [0, 1]
    let arousal = if arousal_weight_sum > 0.01 {
        Some(((arousal_sum / arousal_weight_sum) * 0.5 + 0.5).clamp(0.0, 1.0))
    } else {
        None
    };

    let valence = if valence_weight_sum > 0.01 {
        Some(((valence_sum / valence_weight_sum) * 0.5 + 0.5).clamp(0.0, 1.0))
    } else {
        None
    };

    (arousal, valence)
}

// ============================================================================
// Patch Extraction and Embedding Utilities
// ============================================================================

/// Extract fixed-size patches from mel spectrogram frames
fn extract_patches(frames: &[Vec<f32>], patch_size: usize) -> Vec<Vec<Vec<f32>>> {
    if frames.len() < patch_size {
        // If too short, pad with zeros
        if frames.is_empty() {
            return Vec::new();
        }
        let n_bands = frames[0].len();
        let mut padded = frames.to_vec();
        while padded.len() < patch_size {
            padded.push(vec![0.0; n_bands]);
        }
        return vec![padded];
    }

    // Extract overlapping patches with 50% overlap
    let hop = patch_size / 2;
    let mut patches = Vec::new();
    let mut start = 0;

    while start + patch_size <= frames.len() {
        patches.push(frames[start..start + patch_size].to_vec());
        start += hop;
    }

    // Always include the last patch
    if patches.is_empty() || start < frames.len() {
        let last_start = frames.len().saturating_sub(patch_size);
        patches.push(frames[last_start..].to_vec());
    }

    patches
}

/// Average multiple embeddings into a single vector
fn average_embeddings(embeddings: &[Vec<f32>]) -> Vec<f32> {
    if embeddings.is_empty() {
        return Vec::new();
    }
    let dim = embeddings[0].len();
    let n = embeddings.len() as f32;
    let mut avg = vec![0.0f32; dim];
    for emb in embeddings {
        for (i, &v) in emb.iter().enumerate() {
            if i < dim {
                avg[i] += v;
            }
        }
    }
    for v in &mut avg {
        *v /= n;
    }
    avg
}

// ============================================================================
// Label Lists (from Essentia model hub metadata)
// ============================================================================

/// Full Discogs400 genre taxonomy (400 classes, exact model output order)
fn discogs400_labels() -> Vec<String> {
    [
        // Blues (12)
        "Blues---Boogie Woogie", "Blues---Chicago Blues", "Blues---Country Blues",
        "Blues---Delta Blues", "Blues---Electric Blues", "Blues---Harmonica Blues",
        "Blues---Jump Blues", "Blues---Louisiana Blues", "Blues---Modern Electric Blues",
        "Blues---Piano Blues", "Blues---Rhythm & Blues", "Blues---Texas Blues",
        // Brass & Military (3)
        "Brass & Military---Brass Band", "Brass & Military---Marches",
        "Brass & Military---Military",
        // Children's (3)
        "Children's---Educational", "Children's---Nursery Rhymes", "Children's---Story",
        // Classical (13)
        "Classical---Baroque", "Classical---Choral", "Classical---Classical",
        "Classical---Contemporary", "Classical---Impressionist", "Classical---Medieval",
        "Classical---Modern", "Classical---Neo-Classical", "Classical---Neo-Romantic",
        "Classical---Opera", "Classical---Post-Modern", "Classical---Renaissance",
        "Classical---Romantic",
        // Electronic (106)
        "Electronic---Abstract", "Electronic---Acid", "Electronic---Acid House",
        "Electronic---Acid Jazz", "Electronic---Ambient", "Electronic---Bassline",
        "Electronic---Beatdown", "Electronic---Berlin-School", "Electronic---Big Beat",
        "Electronic---Bleep", "Electronic---Breakbeat", "Electronic---Breakcore",
        "Electronic---Breaks", "Electronic---Broken Beat", "Electronic---Chillwave",
        "Electronic---Chiptune", "Electronic---Dance-pop", "Electronic---Dark Ambient",
        "Electronic---Darkwave", "Electronic---Deep House", "Electronic---Deep Techno",
        "Electronic---Disco", "Electronic---Disco Polo", "Electronic---Donk",
        "Electronic---Downtempo", "Electronic---Drone", "Electronic---Drum n Bass",
        "Electronic---Dub", "Electronic---Dub Techno", "Electronic---Dubstep",
        "Electronic---Dungeon Synth", "Electronic---EBM", "Electronic---Electro",
        "Electronic---Electro House", "Electronic---Electroclash", "Electronic---Euro House",
        "Electronic---Euro-Disco", "Electronic---Eurobeat", "Electronic---Eurodance",
        "Electronic---Experimental", "Electronic---Freestyle", "Electronic---Future Jazz",
        "Electronic---Gabber", "Electronic---Garage House", "Electronic---Ghetto",
        "Electronic---Ghetto House", "Electronic---Glitch", "Electronic---Goa Trance",
        "Electronic---Grime", "Electronic---Halftime", "Electronic---Hands Up",
        "Electronic---Happy Hardcore", "Electronic---Hard House", "Electronic---Hard Techno",
        "Electronic---Hard Trance", "Electronic---Hardcore", "Electronic---Hardstyle",
        "Electronic---Hi NRG", "Electronic---Hip Hop", "Electronic---Hip-House",
        "Electronic---House", "Electronic---IDM", "Electronic---Illbient",
        "Electronic---Industrial", "Electronic---Italo House", "Electronic---Italo-Disco",
        "Electronic---Italodance", "Electronic---Jazzdance", "Electronic---Juke",
        "Electronic---Jumpstyle", "Electronic---Jungle", "Electronic---Latin",
        "Electronic---Leftfield", "Electronic---Makina", "Electronic---Minimal",
        "Electronic---Minimal Techno", "Electronic---Modern Classical",
        "Electronic---Musique Concrète", "Electronic---Neofolk", "Electronic---New Age",
        "Electronic---New Beat", "Electronic---New Wave", "Electronic---Noise",
        "Electronic---Nu-Disco", "Electronic---Power Electronics",
        "Electronic---Progressive Breaks", "Electronic---Progressive House",
        "Electronic---Progressive Trance", "Electronic---Psy-Trance",
        "Electronic---Rhythmic Noise", "Electronic---Schranz", "Electronic---Sound Collage",
        "Electronic---Speed Garage", "Electronic---Speedcore", "Electronic---Synth-pop",
        "Electronic---Synthwave", "Electronic---Tech House", "Electronic---Tech Trance",
        "Electronic---Techno", "Electronic---Trance", "Electronic---Tribal",
        "Electronic---Tribal House", "Electronic---Trip Hop", "Electronic---Tropical House",
        "Electronic---UK Garage", "Electronic---Vaporwave",
        // Folk, World, & Country (27)
        "Folk, World, & Country---African", "Folk, World, & Country---Bluegrass",
        "Folk, World, & Country---Cajun", "Folk, World, & Country---Canzone Napoletana",
        "Folk, World, & Country---Catalan Music", "Folk, World, & Country---Celtic",
        "Folk, World, & Country---Country", "Folk, World, & Country---Fado",
        "Folk, World, & Country---Flamenco", "Folk, World, & Country---Folk",
        "Folk, World, & Country---Gospel", "Folk, World, & Country---Highlife",
        "Folk, World, & Country---Hillbilly", "Folk, World, & Country---Hindustani",
        "Folk, World, & Country---Honky Tonk", "Folk, World, & Country---Indian Classical",
        "Folk, World, & Country---Laïkó", "Folk, World, & Country---Nordic",
        "Folk, World, & Country---Pacific", "Folk, World, & Country---Polka",
        "Folk, World, & Country---Raï", "Folk, World, & Country---Romani",
        "Folk, World, & Country---Soukous", "Folk, World, & Country---Séga",
        "Folk, World, & Country---Volksmusik", "Folk, World, & Country---Zouk",
        "Folk, World, & Country---Éntekhno",
        // Funk / Soul (15)
        "Funk / Soul---Afrobeat", "Funk / Soul---Boogie",
        "Funk / Soul---Contemporary R&B", "Funk / Soul---Disco",
        "Funk / Soul---Free Funk", "Funk / Soul---Funk", "Funk / Soul---Gospel",
        "Funk / Soul---Neo Soul", "Funk / Soul---New Jack Swing",
        "Funk / Soul---P.Funk", "Funk / Soul---Psychedelic",
        "Funk / Soul---Rhythm & Blues", "Funk / Soul---Soul",
        "Funk / Soul---Swingbeat", "Funk / Soul---UK Street Soul",
        // Hip Hop (26)
        "Hip Hop---Bass Music", "Hip Hop---Boom Bap", "Hip Hop---Bounce",
        "Hip Hop---Britcore", "Hip Hop---Cloud Rap", "Hip Hop---Conscious",
        "Hip Hop---Crunk", "Hip Hop---Cut-up/DJ", "Hip Hop---DJ Battle Tool",
        "Hip Hop---Electro", "Hip Hop---G-Funk", "Hip Hop---Gangsta",
        "Hip Hop---Grime", "Hip Hop---Hardcore Hip-Hop", "Hip Hop---Horrorcore",
        "Hip Hop---Instrumental", "Hip Hop---Jazzy Hip-Hop", "Hip Hop---Miami Bass",
        "Hip Hop---Pop Rap", "Hip Hop---Ragga HipHop", "Hip Hop---RnB/Swing",
        "Hip Hop---Screw", "Hip Hop---Thug Rap", "Hip Hop---Trap",
        "Hip Hop---Trip Hop", "Hip Hop---Turntablism",
        // Jazz (25)
        "Jazz---Afro-Cuban Jazz", "Jazz---Afrobeat", "Jazz---Avant-garde Jazz",
        "Jazz---Big Band", "Jazz---Bop", "Jazz---Bossa Nova",
        "Jazz---Contemporary Jazz", "Jazz---Cool Jazz", "Jazz---Dixieland",
        "Jazz---Easy Listening", "Jazz---Free Improvisation", "Jazz---Free Jazz",
        "Jazz---Fusion", "Jazz---Gypsy Jazz", "Jazz---Hard Bop",
        "Jazz---Jazz-Funk", "Jazz---Jazz-Rock", "Jazz---Latin Jazz",
        "Jazz---Modal", "Jazz---Post Bop", "Jazz---Ragtime",
        "Jazz---Smooth Jazz", "Jazz---Soul-Jazz", "Jazz---Space-Age", "Jazz---Swing",
        // Latin (35)
        "Latin---Afro-Cuban", "Latin---Baião", "Latin---Batucada",
        "Latin---Beguine", "Latin---Bolero", "Latin---Boogaloo",
        "Latin---Bossanova", "Latin---Cha-Cha", "Latin---Charanga",
        "Latin---Compas", "Latin---Cubano", "Latin---Cumbia",
        "Latin---Descarga", "Latin---Forró", "Latin---Guaguancó",
        "Latin---Guajira", "Latin---Guaracha", "Latin---MPB",
        "Latin---Mambo", "Latin---Mariachi", "Latin---Merengue",
        "Latin---Norteño", "Latin---Nueva Cancion", "Latin---Pachanga",
        "Latin---Porro", "Latin---Ranchera", "Latin---Reggaeton",
        "Latin---Rumba", "Latin---Salsa", "Latin---Samba",
        "Latin---Son", "Latin---Son Montuno", "Latin---Tango",
        "Latin---Tejano", "Latin---Vallenato",
        // Non-Music (13)
        "Non-Music---Audiobook", "Non-Music---Comedy", "Non-Music---Dialogue",
        "Non-Music---Education", "Non-Music---Field Recording", "Non-Music---Interview",
        "Non-Music---Monolog", "Non-Music---Poetry", "Non-Music---Political",
        "Non-Music---Promotional", "Non-Music---Radioplay", "Non-Music---Religious",
        "Non-Music---Spoken Word",
        // Pop (16)
        "Pop---Ballad", "Pop---Bollywood", "Pop---Bubblegum", "Pop---Chanson",
        "Pop---City Pop", "Pop---Europop", "Pop---Indie Pop", "Pop---J-pop",
        "Pop---K-pop", "Pop---Kayōkyoku", "Pop---Light Music", "Pop---Music Hall",
        "Pop---Novelty", "Pop---Parody", "Pop---Schlager", "Pop---Vocal",
        // Reggae (11)
        "Reggae---Calypso", "Reggae---Dancehall", "Reggae---Dub",
        "Reggae---Lovers Rock", "Reggae---Ragga", "Reggae---Reggae",
        "Reggae---Reggae-Pop", "Reggae---Rocksteady", "Reggae---Roots Reggae",
        "Reggae---Ska", "Reggae---Soca",
        // Rock (91)
        "Rock---AOR", "Rock---Acid Rock", "Rock---Acoustic",
        "Rock---Alternative Rock", "Rock---Arena Rock", "Rock---Art Rock",
        "Rock---Atmospheric Black Metal", "Rock---Avantgarde", "Rock---Beat",
        "Rock---Black Metal", "Rock---Blues Rock", "Rock---Brit Pop",
        "Rock---Classic Rock", "Rock---Coldwave", "Rock---Country Rock",
        "Rock---Crust", "Rock---Death Metal", "Rock---Deathcore",
        "Rock---Deathrock", "Rock---Depressive Black Metal", "Rock---Doo Wop",
        "Rock---Doom Metal", "Rock---Dream Pop", "Rock---Emo",
        "Rock---Ethereal", "Rock---Experimental", "Rock---Folk Metal",
        "Rock---Folk Rock", "Rock---Funeral Doom Metal", "Rock---Funk Metal",
        "Rock---Garage Rock", "Rock---Glam", "Rock---Goregrind",
        "Rock---Goth Rock", "Rock---Gothic Metal", "Rock---Grindcore",
        "Rock---Grunge", "Rock---Hard Rock", "Rock---Hardcore",
        "Rock---Heavy Metal", "Rock---Indie Rock", "Rock---Industrial",
        "Rock---Krautrock", "Rock---Lo-Fi", "Rock---Lounge",
        "Rock---Math Rock", "Rock---Melodic Death Metal", "Rock---Melodic Hardcore",
        "Rock---Metalcore", "Rock---Mod", "Rock---Neofolk",
        "Rock---New Wave", "Rock---No Wave", "Rock---Noise",
        "Rock---Noisecore", "Rock---Nu Metal", "Rock---Oi",
        "Rock---Parody", "Rock---Pop Punk", "Rock---Pop Rock",
        "Rock---Pornogrind", "Rock---Post Rock", "Rock---Post-Hardcore",
        "Rock---Post-Metal", "Rock---Post-Punk", "Rock---Power Metal",
        "Rock---Power Pop", "Rock---Power Violence", "Rock---Prog Rock",
        "Rock---Progressive Metal", "Rock---Psychedelic Rock", "Rock---Psychobilly",
        "Rock---Pub Rock", "Rock---Punk", "Rock---Rock & Roll",
        "Rock---Rockabilly", "Rock---Shoegaze", "Rock---Ska",
        "Rock---Sludge Metal", "Rock---Soft Rock", "Rock---Southern Rock",
        "Rock---Space Rock", "Rock---Speed Metal", "Rock---Stoner Rock",
        "Rock---Surf", "Rock---Symphonic Rock", "Rock---Technical Death Metal",
        "Rock---Thrash", "Rock---Twist", "Rock---Viking Metal", "Rock---Yé-Yé",
        // Stage & Screen (4)
        "Stage & Screen---Musical", "Stage & Screen---Score",
        "Stage & Screen---Soundtrack", "Stage & Screen---Theme",
    ].iter().map(|s| s.to_string()).collect()
}

/// Jamendo mood/theme labels (56 classes, exact model output order)
fn jamendo_mood_labels() -> Vec<String> {
    [
        "action", "adventure", "advertising", "background", "ballad",
        "calm", "children", "christmas", "commercial", "cool",
        "corporate", "dark", "deep", "documentary", "drama",
        "dramatic", "dream", "emotional", "energetic", "epic",
        "fast", "film", "fun", "funny", "game",
        "groovy", "happy", "heavy", "holiday", "hopeful",
        "inspiring", "love", "meditative", "melancholic", "melodic",
        "motivational", "movie", "nature", "party", "positive",
        "powerful", "relaxing", "retro", "romantic", "sad",
        "sexy", "slow", "soft", "soundscape", "space",
        "sport", "summer", "trailer", "travel", "upbeat",
        "uplifting",
    ].iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_patches_short() {
        let frames: Vec<Vec<f32>> = (0..50).map(|i| vec![i as f32; 96]).collect();
        let patches = extract_patches(&frames, PATCH_SIZE);
        assert_eq!(patches.len(), 1, "Short audio should produce 1 padded patch");
        assert_eq!(patches[0].len(), PATCH_SIZE);
    }

    #[test]
    fn test_extract_patches_normal() {
        let frames: Vec<Vec<f32>> = (0..500).map(|i| vec![i as f32; 96]).collect();
        let patches = extract_patches(&frames, PATCH_SIZE);
        assert!(patches.len() >= 2, "500 frames should produce multiple patches");
        for patch in &patches {
            assert_eq!(patch.len(), PATCH_SIZE);
        }
    }

    #[test]
    fn test_average_embeddings() {
        let emb1 = vec![1.0, 2.0, 3.0];
        let emb2 = vec![3.0, 4.0, 5.0];
        let avg = average_embeddings(&[emb1, emb2]);
        assert_eq!(avg, vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_average_embeddings_empty() {
        assert!(average_embeddings(&[]).is_empty());
    }

    #[test]
    fn test_discogs400_label_count() {
        assert_eq!(discogs400_labels().len(), 400);
    }

    #[test]
    fn test_jamendo_mood_label_count() {
        assert_eq!(jamendo_mood_labels().len(), 56);
    }

    #[test]
    fn test_derive_arousal_energetic() {
        let moods = vec![
            ("energetic".to_string(), 0.9),
            ("fast".to_string(), 0.7),
            ("party".to_string(), 0.6),
        ];
        let (arousal, _) = derive_arousal_valence_from_mood(&moods);
        assert!(arousal.unwrap() > 0.7, "Energetic tracks should have high arousal");
    }

    #[test]
    fn test_derive_arousal_calm() {
        let moods = vec![
            ("calm".to_string(), 0.9),
            ("relaxing".to_string(), 0.8),
            ("soft".to_string(), 0.6),
        ];
        let (arousal, _) = derive_arousal_valence_from_mood(&moods);
        assert!(arousal.unwrap() < 0.3, "Calm tracks should have low arousal");
    }

    #[test]
    fn test_derive_valence_happy() {
        let moods = vec![
            ("happy".to_string(), 0.9),
            ("uplifting".to_string(), 0.7),
            ("fun".to_string(), 0.6),
        ];
        let (_, valence) = derive_arousal_valence_from_mood(&moods);
        assert!(valence.unwrap() > 0.7, "Happy tracks should have high valence");
    }

    #[test]
    fn test_derive_valence_sad() {
        let moods = vec![
            ("sad".to_string(), 0.8),
            ("melancholic".to_string(), 0.7),
            ("dark".to_string(), 0.5),
        ];
        let (_, valence) = derive_arousal_valence_from_mood(&moods);
        assert!(valence.unwrap() < 0.3, "Sad tracks should have low valence");
    }

    #[test]
    fn test_derive_empty_moods() {
        let (arousal, valence) = derive_arousal_valence_from_mood(&[]);
        assert!(arousal.is_none());
        assert!(valence.is_none());
    }

    #[test]
    fn test_patch_size_constant() {
        assert_eq!(PATCH_SIZE, 128, "EffNet expects 128-frame patches");
    }
}
