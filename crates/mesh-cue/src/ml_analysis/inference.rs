//! ONNX-based ML inference for audio analysis (MAEST-only).
//!
//! Runs the MAEST PaSST/AST embedding + 519-class genre head using `ort`
//! (ONNX Runtime). The `MlAnalyzer` holds a pre-loaded session and can be
//! wrapped in `Arc<Mutex<>>` for sharing across rayon workers (`analyze()`
//! requires `&mut self`).
//!
//! # Architecture
//!
//! MAEST is the sole ONNX model on this branch. A single forward pass
//! produces:
//!
//! * 2304-dim pooled embedding from the layer-7 token tensor
//!   (`PartitionedCall/Identity_7`), pooled as `[CLS | DIST | mean(signal)]`.
//! * 519-class sigmoid genre predictions from `PartitionedCall/Identity_13`,
//!   keyed against the Discogs `Genre---Subgenre` taxonomy.
//!
//! The legacy EffNet classification heads (mood, voice, danceability,
//! timbre, ...) were trained against EffNet's 1280-dim embedding and have
//! been removed wholesale. They will be retrained against MAEST embeddings
//! as a follow-up — see `documents/embedding-models-research.md`.

use std::path::Path;
use ndarray::Array3;
use ort::session::Session;
use ort::value::Tensor;
use mesh_core::db::MlAnalysisData;

use super::models::MlModelType;
use super::preprocessing::MelSpectrogramResult;

/// Combined result of a full ML analysis run.
///
/// Bundles the structured `MlAnalysisData` together with the raw 2304-dim
/// MAEST embedding so callers can persist both in a single pass.
/// `embedding` is empty if inference failed.
pub struct MlAnalysisResult {
    pub data: MlAnalysisData,
    /// 2304-dim MAEST embedding (`[CLS | DIST | mean(signal)]` from layer 7,
    /// averaged across windows). Empty if inference failed.
    pub embedding: Vec<f32>,
}

/// Number of mel bands (frequency axis). MAEST consumes 96 bands @ 16 kHz.
const N_BANDS: usize = 96;

/// MAEST 30s window: 1876 mel frames at 16 kHz / hop 256 → ~30.0 seconds.
/// Confirmed via Essentia metadata JSON (input shape `[1, 1876, 96]`).
const MAEST_WINDOW_FRAMES: usize = 1876;

/// MAEST hidden size — the size of each token vector at layer 7.
const MAEST_HIDDEN_DIM: usize = 768;

/// MAEST pooled embedding size: CLS|DIST|mean(rest) → 3 × 768.
const MAEST_EMBEDDING_DIM: usize = MAEST_HIDDEN_DIM * 3;

/// MAEST output tensor index for the layer-7 token embeddings
/// (shape `[1, n_tokens, 768]`).
///
/// The Essentia ONNX export emits 14 outputs as:
///   * 0       — `logits`                       (519 raw logits)
///   * 1..=12  — `layer_00_embeddings` …        (zero-indexed transformer layers)
///                `layer_11_embeddings`         (each `[1, n_tokens, 768]`)
///   * 13      — `activations`                  (519 sigmoid predictions)
///
/// The MAEST paper reports best downstream performance using **layer 7**
/// (1-indexed in the paper). That maps to `layer_06_embeddings` in this
/// 0-indexed export, which is **output index 7** in the iteration order
/// (`logits` is at 0, then `layer_00_embeddings` is at 1, …,
/// `layer_06_embeddings` is at 7).
///
/// We match by index because ORT 2.x rewrites the original names; the
/// emitted order is stable across exports.
const MAEST_EMBEDDING_OUTPUT_INDEX: usize = 7;

/// MAEST output tensor index for the 519-class sigmoid genre predictions
/// (the `activations` output, last in the export).
const MAEST_GENRE_OUTPUT_INDEX: usize = 13;

/// MAEST input tensor name (mel spectrogram, shape `[1, 1876, 96]`).
const MAEST_INPUT_NAME: &str = "melspectrogram";

/// Maximum number of 30-second windows we run MAEST on per track.
///
/// One MAEST forward pass on CPU is ~1.5–4 s. A 4-minute track at 50 %
/// overlap produces 15 windows → ~30–60 s of pure ML CPU per track. We
/// cap at 4 evenly-spaced windows: that covers intro / verse / chorus /
/// outro for a typical electronic track and keeps per-track ML cost to
/// ~6–16 s while preserving the spirit of "average across the song."
const MAEST_MAX_WINDOWS: usize = 4;

/// ML analysis engine with a pre-loaded MAEST ONNX session.
pub struct MlAnalyzer {
    maest: Session,
    /// 519 Discogs `Genre---Subgenre` style labels in MAEST output order.
    genre_labels: Vec<String>,
}

// Safety: ort::Session is Send+Sync by design
unsafe impl Send for MlAnalyzer {}
unsafe impl Sync for MlAnalyzer {}

impl MlAnalyzer {
    /// Create a new analyzer with a pre-loaded MAEST model.
    ///
    /// # Arguments
    /// * `model_dir` - Directory containing the MAEST ONNX file
    ///   (`MlModelType::MaestEmbedding519l.filename()`).
    pub fn new(model_dir: &Path) -> Result<Self, String> {
        let maest_path = model_dir.join(MlModelType::MaestEmbedding519l.filename());

        if !maest_path.exists() {
            return Err(format!("MAEST model not found: {:?}", maest_path));
        }

        let maest = Session::builder()
            .and_then(|b| b.with_intra_threads(1))
            .and_then(|b| b.commit_from_file(&maest_path))
            .map_err(|e| format!("Failed to load MAEST: {}", e))?;

        log::info!("Loaded MAEST (519-style Embedding + Genre) model");
        // One-shot dump of output names so we can confirm the index mapping
        // (ORT rewrites `PartitionedCall/Identity_7` → `PartitionedCall_Identity_7`
        // etc., which is why we match by index, not name).
        for (i, out) in maest.outputs().iter().enumerate() {
            log::debug!("MAEST output[{}]: name={:?}", i, out.name());
        }

        Ok(Self {
            maest,
            genre_labels: discogs519_labels(),
        })
    }

    /// Run MAEST inference on a mel spectrogram.
    ///
    /// 1. Slide a 1876-frame window across the spectrogram with 50% overlap
    ///    (pad with zeros if the input is shorter).
    /// 2. For each window: run MAEST → pool layer-7 tokens to 2304-dim,
    ///    apply sigmoid to the genre logits.
    /// 3. Average the pooled embedding and the 519-d sigmoid scores across
    ///    windows.
    /// 4. Decode the top labels (threshold > 0.05, top 10, sorted desc).
    pub fn analyze(
        &mut self,
        mel: &MelSpectrogramResult,
    ) -> Result<MlAnalysisResult, String> {
        let patches = extract_patches(&mel.frames, MAEST_WINDOW_FRAMES);
        if patches.is_empty() {
            return Err("Audio too short for MAEST analysis".to_string());
        }
        log::debug!(
            "ML: running MAEST on {} windows ({} mel frames)",
            patches.len(),
            mel.frames.len()
        );

        let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(patches.len());
        let mut all_genre_preds: Vec<Vec<f32>> = Vec::with_capacity(patches.len());

        for patch in &patches {
            let (embedding, genre_preds) = self.run_maest(patch)?;
            all_embeddings.push(embedding);
            all_genre_preds.push(genre_preds);
        }

        let avg_embedding = average_embeddings(&all_embeddings);
        let avg_genre_preds = average_embeddings(&all_genre_preds);

        let (top_genre, genre_scores) = self.decode_genre_predictions(&avg_genre_preds);

        Ok(MlAnalysisResult {
            data: MlAnalysisData {
                top_genre,
                genre_scores,
            },
            embedding: avg_embedding,
        })
    }

    /// Run MAEST on a single 1876-frame window → (embedding[2304], genre_preds[519]).
    fn run_maest(&mut self, patch: &[Vec<f32>]) -> Result<(Vec<f32>, Vec<f32>), String> {
        let n_frames = patch.len();
        let n_bands = if n_frames > 0 { patch[0].len() } else { N_BANDS };

        let mut flat = Vec::with_capacity(n_frames * n_bands);
        for frame in patch {
            flat.extend_from_slice(frame);
        }

        // MAEST input: [batch=1, time_frames, mel_bands]
        let input = Array3::from_shape_vec((1, n_frames, n_bands), flat)
            .map_err(|e| format!("MAEST input shape error: {}", e))?;

        let input_tensor = Tensor::from_array(input)
            .map_err(|e| format!("MAEST tensor creation error: {}", e))?;

        let outputs = self.maest.run(
            ort::inputs![MAEST_INPUT_NAME => input_tensor]
        ).map_err(|e| format!("MAEST inference error: {}", e))?;

        // MAEST emits 14 outputs in a fixed order — pick the two we need by
        // index. ORT 2.x rewrites the original `PartitionedCall/Identity_*`
        // names (slashes → underscores), so name-based matching is brittle.
        let collected: Vec<_> = outputs.iter().collect();

        let (_, emb_value) = collected.get(MAEST_EMBEDDING_OUTPUT_INDEX).ok_or_else(|| {
            format!(
                "MAEST embedding output index {} missing (got {} outputs)",
                MAEST_EMBEDDING_OUTPUT_INDEX, collected.len()
            )
        })?;
        let (emb_shape, emb_data) = emb_value.try_extract_tensor::<f32>()
            .map_err(|e| format!("MAEST embedding extraction error: {}", e))?;
        let embedding = pool_layer7_tokens(
            &emb_data.iter().copied().collect::<Vec<_>>(),
            &emb_shape,
        );

        let (_, genre_value) = collected.get(MAEST_GENRE_OUTPUT_INDEX).ok_or_else(|| {
            format!(
                "MAEST genre output index {} missing (got {} outputs)",
                MAEST_GENRE_OUTPUT_INDEX, collected.len()
            )
        })?;
        let (_genre_shape, genre_data) = genre_value.try_extract_tensor::<f32>()
            .map_err(|e| format!("MAEST genre extraction error: {}", e))?;
        // Apply sigmoid — the export emits raw logits in this output.
        let genre_preds: Vec<f32> = genre_data.iter().map(|&l| sigmoid(l)).collect();

        Ok((embedding, genre_preds))
    }

    /// Decode the 519-d sigmoid output → (top_genre, top-10 scores above 0.05).
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
}

// ============================================================================
// Pooling, Patch Extraction, Embedding Averaging
// ============================================================================

/// Pool MAEST layer-7 token tensor `[1, n_tokens, 768]` into a flat 2304-dim
/// vector composed of `[CLS | DIST | mean(signal)]`.
///
/// The first two tokens are the CLS and DIST class tokens; the remaining
/// tokens are the patch ("signal") tokens that get mean-pooled.
fn pool_layer7_tokens(data: &[f32], shape: &[i64]) -> Vec<f32> {
    // Expect either [1, n_tokens, 768] or [n_tokens, 768] — handle both.
    let (n_tokens, hidden) = match shape {
        [_, n, h] => (*n as usize, *h as usize),
        [n, h] => (*n as usize, *h as usize),
        _ => {
            log::warn!("MAEST embedding has unexpected shape {:?}; returning zeros", shape);
            return vec![0.0; MAEST_EMBEDDING_DIM];
        }
    };

    if hidden != MAEST_HIDDEN_DIM {
        log::warn!(
            "MAEST embedding hidden dim {} != expected {}; returning zeros",
            hidden, MAEST_HIDDEN_DIM
        );
        return vec![0.0; MAEST_EMBEDDING_DIM];
    }
    if n_tokens < 3 {
        log::warn!(
            "MAEST embedding has only {} tokens (need ≥3 for CLS|DIST|signal); returning zeros",
            n_tokens
        );
        return vec![0.0; MAEST_EMBEDDING_DIM];
    }

    let cls = &data[0..hidden];
    let dist = &data[hidden..2 * hidden];

    // Mean over signal tokens (indices 2..n_tokens)
    let n_signal = (n_tokens - 2) as f32;
    let mut mean_signal = vec![0.0f32; hidden];
    for tok in 2..n_tokens {
        let off = tok * hidden;
        let row = &data[off..off + hidden];
        for (i, &v) in row.iter().enumerate() {
            mean_signal[i] += v;
        }
    }
    for v in &mut mean_signal {
        *v /= n_signal;
    }

    let mut pooled = Vec::with_capacity(MAEST_EMBEDDING_DIM);
    pooled.extend_from_slice(cls);
    pooled.extend_from_slice(dist);
    pooled.extend_from_slice(&mean_signal);
    pooled
}

/// Numerically-safe sigmoid.
#[inline]
fn sigmoid(x: f32) -> f32 {
    if x >= 0.0 {
        let z = (-x).exp();
        1.0 / (1.0 + z)
    } else {
        let z = x.exp();
        z / (1.0 + z)
    }
}

/// Extract up to `MAEST_MAX_WINDOWS` evenly-spaced `patch_size`-frame
/// windows from the mel spectrogram. Pads short inputs with zeros.
///
/// Even spacing (vs sliding overlap) covers the full track structure with
/// far fewer MAEST passes — a 4-minute track drops from ~15 windows to 4
/// while still sampling intro / verse / chorus / outro.
fn extract_patches(frames: &[Vec<f32>], patch_size: usize) -> Vec<Vec<Vec<f32>>> {
    if frames.is_empty() {
        return Vec::new();
    }
    if frames.len() < patch_size {
        let n_bands = frames[0].len();
        let mut padded = frames.to_vec();
        while padded.len() < patch_size {
            padded.push(vec![0.0; n_bands]);
        }
        return vec![padded];
    }

    let max_starts = frames.len() - patch_size;
    let n_windows = MAEST_MAX_WINDOWS.max(1);
    let mut patches = Vec::with_capacity(n_windows);

    if n_windows == 1 || max_starts == 0 {
        // Single centred window for very short tracks.
        let start = max_starts / 2;
        patches.push(frames[start..start + patch_size].to_vec());
        return patches;
    }

    // Evenly space `n_windows` window starts across [0, max_starts].
    for i in 0..n_windows {
        let start = (i * max_starts) / (n_windows - 1);
        patches.push(frames[start..start + patch_size].to_vec());
    }
    patches
}

/// Average multiple equal-length vectors into a single vector.
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
// Label list (from MAEST metadata JSON: discogs-maest-30s-pw-519l-2.json)
// ============================================================================

/// Full Discogs 519-style taxonomy in MAEST output order.
///
/// Generated from the Essentia metadata JSON's `classes` array; do not
/// edit by hand. Source URL:
/// `https://essentia.upf.edu/models/feature-extractors/maest/discogs-maest-30s-pw-519l-2.json`
fn discogs519_labels() -> Vec<String> {
    [
        "Blues---Boogie Woogie", "Blues---Chicago Blues", "Blues---Country Blues",
        "Blues---Delta Blues", "Blues---East Coast Blues", "Blues---Electric Blues",
        "Blues---Harmonica Blues", "Blues---Jump Blues", "Blues---Louisiana Blues",
        "Blues---Memphis Blues", "Blues---Modern Electric Blues", "Blues---Piano Blues",
        "Blues---Piedmont Blues", "Blues---Rhythm & Blues", "Blues---Texas Blues",
        "Brass & Military---Brass Band", "Brass & Military---Marches", "Brass & Military---Military",
        "Brass & Military---Pipe & Drum", "Children's---Educational", "Children's---Nursery Rhymes",
        "Children's---Story", "Classical---Baroque", "Classical---Choral",
        "Classical---Classical", "Classical---Contemporary", "Classical---Early",
        "Classical---Impressionist", "Classical---Medieval", "Classical---Modern",
        "Classical---Neo-Classical", "Classical---Neo-Romantic", "Classical---Opera",
        "Classical---Operetta", "Classical---Oratorio", "Classical---Post-Modern",
        "Classical---Renaissance", "Classical---Romantic", "Classical---Twelve-tone",
        "Electronic---Abstract", "Electronic---Acid", "Electronic---Acid House",
        "Electronic---Acid Jazz", "Electronic---Ambient", "Electronic---Baltimore Club",
        "Electronic---Bassline", "Electronic---Beatdown", "Electronic---Berlin-School",
        "Electronic---Big Beat", "Electronic---Bleep", "Electronic---Breakbeat",
        "Electronic---Breakcore", "Electronic---Breaks", "Electronic---Broken Beat",
        "Electronic---Chillwave", "Electronic---Chiptune", "Electronic---Dance-pop",
        "Electronic---Dark Ambient", "Electronic---Darkwave", "Electronic---Deep House",
        "Electronic---Deep Techno", "Electronic---Disco", "Electronic---Disco Polo",
        "Electronic---Donk", "Electronic---Doomcore", "Electronic---Downtempo",
        "Electronic---Drone", "Electronic---Drum n Bass", "Electronic---Dub",
        "Electronic---Dub Techno", "Electronic---Dubstep", "Electronic---Dungeon Synth",
        "Electronic---EBM", "Electronic---Electro", "Electronic---Electro House",
        "Electronic---Electroacoustic", "Electronic---Electroclash", "Electronic---Euro House",
        "Electronic---Euro-Disco", "Electronic---Eurobeat", "Electronic---Eurodance",
        "Electronic---Experimental", "Electronic---Footwork", "Electronic---Freestyle",
        "Electronic---Future Jazz", "Electronic---Gabber", "Electronic---Garage House",
        "Electronic---Ghetto", "Electronic---Ghetto House", "Electronic---Ghettotech",
        "Electronic---Glitch", "Electronic---Glitch Hop", "Electronic---Goa Trance",
        "Electronic---Grime", "Electronic---Halftime", "Electronic---Hands Up",
        "Electronic---Happy Hardcore", "Electronic---Hard Beat", "Electronic---Hard House",
        "Electronic---Hard Techno", "Electronic---Hard Trance", "Electronic---Hardcore",
        "Electronic---Hardstyle", "Electronic---Harsh Noise Wall", "Electronic---Hi NRG",
        "Electronic---Hip Hop", "Electronic---Hip-House", "Electronic---House",
        "Electronic---IDM", "Electronic---Illbient", "Electronic---Industrial",
        "Electronic---Italo House", "Electronic---Italo-Disco", "Electronic---Italodance",
        "Electronic---J-Core", "Electronic---Jazzdance", "Electronic---Juke",
        "Electronic---Jumpstyle", "Electronic---Jungle", "Electronic---Latin",
        "Electronic---Leftfield", "Electronic---Lento Violento", "Electronic---Makina",
        "Electronic---Minimal", "Electronic---Minimal Techno", "Electronic---Modern Classical",
        "Electronic---Musique Concrète", "Electronic---Neo Trance", "Electronic---Neofolk",
        "Electronic---New Age", "Electronic---New Beat", "Electronic---New Wave",
        "Electronic---Noise", "Electronic---Nu-Disco", "Electronic---Power Electronics",
        "Electronic---Progressive Breaks", "Electronic---Progressive House", "Electronic---Progressive Trance",
        "Electronic---Psy-Trance", "Electronic---Rhythmic Noise", "Electronic---Schranz",
        "Electronic---Sound Collage", "Electronic---Speed Garage", "Electronic---Speedcore",
        "Electronic---Synth-pop", "Electronic---Synthwave", "Electronic---Tech House",
        "Electronic---Tech Trance", "Electronic---Techno", "Electronic---Trance",
        "Electronic---Tribal", "Electronic---Tribal House", "Electronic---Trip Hop",
        "Electronic---Tropical House", "Electronic---UK Funky", "Electronic---UK Garage",
        "Electronic---Vaporwave", "Electronic---Witch House", "Folk, World, & Country---Aboriginal",
        "Folk, World, & Country---African", "Folk, World, & Country---Andalusian Classical", "Folk, World, & Country---Andean Music",
        "Folk, World, & Country---Appalachian Music", "Folk, World, & Country---Basque Music", "Folk, World, & Country---Bhangra",
        "Folk, World, & Country---Bluegrass", "Folk, World, & Country---Cajun", "Folk, World, & Country---Canzone Napoletana",
        "Folk, World, & Country---Carnatic", "Folk, World, & Country---Catalan Music", "Folk, World, & Country---Celtic",
        "Folk, World, & Country---Chacarera", "Folk, World, & Country---Chinese Classical", "Folk, World, & Country---Chutney",
        "Folk, World, & Country---Copla", "Folk, World, & Country---Country", "Folk, World, & Country---Cretan",
        "Folk, World, & Country---Dangdut", "Folk, World, & Country---Fado", "Folk, World, & Country---Flamenco",
        "Folk, World, & Country---Folk", "Folk, World, & Country---Funaná", "Folk, World, & Country---Gamelan",
        "Folk, World, & Country---Ghazal", "Folk, World, & Country---Gospel", "Folk, World, & Country---Griot",
        "Folk, World, & Country---Hawaiian", "Folk, World, & Country---Highlife", "Folk, World, & Country---Hillbilly",
        "Folk, World, & Country---Hindustani", "Folk, World, & Country---Honky Tonk", "Folk, World, & Country---Indian Classical",
        "Folk, World, & Country---Kaseko", "Folk, World, & Country---Klezmer", "Folk, World, & Country---Laïkó",
        "Folk, World, & Country---Luk Thung", "Folk, World, & Country---Maloya", "Folk, World, & Country---Mbalax",
        "Folk, World, & Country---Min'yō", "Folk, World, & Country---Mizrahi", "Folk, World, & Country---Nhạc Vàng",
        "Folk, World, & Country---Nordic", "Folk, World, & Country---Népzene", "Folk, World, & Country---Ottoman Classical",
        "Folk, World, & Country---Overtone Singing", "Folk, World, & Country---Pacific", "Folk, World, & Country---Pasodoble",
        "Folk, World, & Country---Persian Classical", "Folk, World, & Country---Phleng Phuea Chiwit", "Folk, World, & Country---Polka",
        "Folk, World, & Country---Qawwali", "Folk, World, & Country---Raï", "Folk, World, & Country---Rebetiko",
        "Folk, World, & Country---Romani", "Folk, World, & Country---Salegy", "Folk, World, & Country---Sea Shanties",
        "Folk, World, & Country---Soukous", "Folk, World, & Country---Séga", "Folk, World, & Country---Volksmusik",
        "Folk, World, & Country---Western Swing", "Folk, World, & Country---Zouk", "Folk, World, & Country---Zydeco",
        "Folk, World, & Country---Éntekhno", "Funk / Soul---Afrobeat", "Funk / Soul---Bayou Funk",
        "Funk / Soul---Boogie", "Funk / Soul---Contemporary R&B", "Funk / Soul---Disco",
        "Funk / Soul---Free Funk", "Funk / Soul---Funk", "Funk / Soul---Gogo",
        "Funk / Soul---Gospel", "Funk / Soul---Minneapolis Sound", "Funk / Soul---Neo Soul",
        "Funk / Soul---New Jack Swing", "Funk / Soul---P.Funk", "Funk / Soul---Psychedelic",
        "Funk / Soul---Rhythm & Blues", "Funk / Soul---Soul", "Funk / Soul---Swingbeat",
        "Funk / Soul---UK Street Soul", "Hip Hop---Bass Music", "Hip Hop---Beatbox",
        "Hip Hop---Boom Bap", "Hip Hop---Bounce", "Hip Hop---Britcore",
        "Hip Hop---Cloud Rap", "Hip Hop---Conscious", "Hip Hop---Crunk",
        "Hip Hop---Cut-up/DJ", "Hip Hop---DJ Battle Tool", "Hip Hop---Electro",
        "Hip Hop---Favela Funk", "Hip Hop---G-Funk", "Hip Hop---Gangsta",
        "Hip Hop---Go-Go", "Hip Hop---Grime", "Hip Hop---Hardcore Hip-Hop",
        "Hip Hop---Hiplife", "Hip Hop---Horrorcore", "Hip Hop---Hyphy",
        "Hip Hop---Instrumental", "Hip Hop---Jazzy Hip-Hop", "Hip Hop---Kwaito",
        "Hip Hop---Miami Bass", "Hip Hop---Pop Rap", "Hip Hop---Ragga HipHop",
        "Hip Hop---RnB/Swing", "Hip Hop---Screw", "Hip Hop---Thug Rap",
        "Hip Hop---Trap", "Hip Hop---Trip Hop", "Hip Hop---Turntablism",
        "Jazz---Afro-Cuban Jazz", "Jazz---Afrobeat", "Jazz---Avant-garde Jazz",
        "Jazz---Big Band", "Jazz---Bop", "Jazz---Bossa Nova",
        "Jazz---Cape Jazz", "Jazz---Contemporary Jazz", "Jazz---Cool Jazz",
        "Jazz---Dixieland", "Jazz---Easy Listening", "Jazz---Free Improvisation",
        "Jazz---Free Jazz", "Jazz---Fusion", "Jazz---Gypsy Jazz",
        "Jazz---Hard Bop", "Jazz---Jazz-Funk", "Jazz---Jazz-Rock",
        "Jazz---Latin Jazz", "Jazz---Modal", "Jazz---Post Bop",
        "Jazz---Ragtime", "Jazz---Smooth Jazz", "Jazz---Soul-Jazz",
        "Jazz---Space-Age", "Jazz---Swing", "Latin---Afro-Cuban",
        "Latin---Axé", "Latin---Bachata", "Latin---Baião",
        "Latin---Batucada", "Latin---Beguine", "Latin---Bolero",
        "Latin---Boogaloo", "Latin---Bossanova", "Latin---Carimbó",
        "Latin---Cha-Cha", "Latin---Charanga", "Latin---Choro",
        "Latin---Compas", "Latin---Conjunto", "Latin---Corrido",
        "Latin---Cubano", "Latin---Cumbia", "Latin---Danzon",
        "Latin---Descarga", "Latin---Forró", "Latin---Gaita",
        "Latin---Guaguancó", "Latin---Guajira", "Latin---Guaracha",
        "Latin---Jibaro", "Latin---Lambada", "Latin---MPB",
        "Latin---Mambo", "Latin---Mariachi", "Latin---Marimba",
        "Latin---Merengue", "Latin---Música Criolla", "Latin---Norteño",
        "Latin---Nueva Cancion", "Latin---Nueva Trova", "Latin---Pachanga",
        "Latin---Plena", "Latin---Porro", "Latin---Quechua",
        "Latin---Ranchera", "Latin---Reggaeton", "Latin---Rumba",
        "Latin---Salsa", "Latin---Samba", "Latin---Samba-Canção",
        "Latin---Son", "Latin---Son Montuno", "Latin---Sonero",
        "Latin---Tango", "Latin---Tejano", "Latin---Timba",
        "Latin---Trova", "Latin---Vallenato", "Non-Music---Audiobook",
        "Non-Music---Comedy", "Non-Music---Dialogue", "Non-Music---Education",
        "Non-Music---Erotic", "Non-Music---Field Recording", "Non-Music---Health-Fitness",
        "Non-Music---Interview", "Non-Music---Monolog", "Non-Music---Movie Effects",
        "Non-Music---Poetry", "Non-Music---Political", "Non-Music---Promotional",
        "Non-Music---Public Broadcast", "Non-Music---Radioplay", "Non-Music---Religious",
        "Non-Music---Sermon", "Non-Music---Sound Art", "Non-Music---Sound Poetry",
        "Non-Music---Special Effects", "Non-Music---Speech", "Non-Music---Spoken Word",
        "Non-Music---Technical", "Non-Music---Therapy", "Pop---Ballad",
        "Pop---Barbershop", "Pop---Bollywood", "Pop---Break-In",
        "Pop---Bubblegum", "Pop---Chanson", "Pop---City Pop",
        "Pop---Enka", "Pop---Ethno-pop", "Pop---Europop",
        "Pop---Indie Pop", "Pop---J-pop", "Pop---K-pop",
        "Pop---Karaoke", "Pop---Kayōkyoku", "Pop---Levenslied",
        "Pop---Light Music", "Pop---Music Hall", "Pop---Novelty",
        "Pop---Parody", "Pop---Schlager", "Pop---Vocal",
        "Reggae---Calypso", "Reggae---Dancehall", "Reggae---Dub",
        "Reggae---Dub Poetry", "Reggae---Lovers Rock", "Reggae---Mento",
        "Reggae---Ragga", "Reggae---Reggae", "Reggae---Reggae Gospel",
        "Reggae---Reggae-Pop", "Reggae---Rocksteady", "Reggae---Roots Reggae",
        "Reggae---Ska", "Reggae---Soca", "Reggae---Steel Band",
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
        "Rock---Groove Metal", "Rock---Grunge", "Rock---Hard Rock",
        "Rock---Hardcore", "Rock---Heavy Metal", "Rock---Horror Rock",
        "Rock---Indie Rock", "Rock---Industrial", "Rock---Industrial Metal",
        "Rock---J-Rock", "Rock---Jangle Pop", "Rock---K-Rock",
        "Rock---Krautrock", "Rock---Lo-Fi", "Rock---Lounge",
        "Rock---Math Rock", "Rock---Melodic Death Metal", "Rock---Melodic Hardcore",
        "Rock---Metalcore", "Rock---Mod", "Rock---NDW",
        "Rock---Neofolk", "Rock---New Wave", "Rock---No Wave",
        "Rock---Noise", "Rock---Noisecore", "Rock---Nu Metal",
        "Rock---Oi", "Rock---Parody", "Rock---Pop Punk",
        "Rock---Pop Rock", "Rock---Pornogrind", "Rock---Post Rock",
        "Rock---Post-Hardcore", "Rock---Post-Metal", "Rock---Post-Punk",
        "Rock---Power Metal", "Rock---Power Pop", "Rock---Power Violence",
        "Rock---Prog Rock", "Rock---Progressive Metal", "Rock---Psychedelic Rock",
        "Rock---Psychobilly", "Rock---Pub Rock", "Rock---Punk",
        "Rock---Rock & Roll", "Rock---Rock Opera", "Rock---Rockabilly",
        "Rock---Shoegaze", "Rock---Ska", "Rock---Skiffle",
        "Rock---Sludge Metal", "Rock---Soft Rock", "Rock---Southern Rock",
        "Rock---Space Rock", "Rock---Speed Metal", "Rock---Stoner Rock",
        "Rock---Surf", "Rock---Swamp Pop", "Rock---Symphonic Rock",
        "Rock---Technical Death Metal", "Rock---Thrash", "Rock---Twist",
        "Rock---Viking Metal", "Rock---Yé-Yé", "Stage & Screen---Musical",
        "Stage & Screen---Score", "Stage & Screen---Soundtrack", "Stage & Screen---Theme",
    ].iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_patches_short() {
        let frames: Vec<Vec<f32>> = (0..50).map(|i| vec![i as f32; N_BANDS]).collect();
        let patches = extract_patches(&frames, MAEST_WINDOW_FRAMES);
        assert_eq!(patches.len(), 1, "Short audio should produce 1 padded patch");
        assert_eq!(patches[0].len(), MAEST_WINDOW_FRAMES);
    }

    #[test]
    fn test_extract_patches_normal() {
        // Two windows with 50%-overlap stride: enough frames to yield ≥2 windows.
        let frames: Vec<Vec<f32>> = (0..MAEST_WINDOW_FRAMES * 3).map(|i| vec![i as f32; N_BANDS]).collect();
        let patches = extract_patches(&frames, MAEST_WINDOW_FRAMES);
        assert!(patches.len() >= 2, "Long audio should produce multiple windows");
        for patch in &patches {
            assert_eq!(patch.len(), MAEST_WINDOW_FRAMES);
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
    fn test_discogs519_label_count() {
        assert_eq!(discogs519_labels().len(), 519);
    }

    #[test]
    fn test_maest_window_frames_constant() {
        assert_eq!(MAEST_WINDOW_FRAMES, 1876, "MAEST input frame count is 1876");
    }

    #[test]
    fn test_maest_embedding_dim_constant() {
        assert_eq!(MAEST_EMBEDDING_DIM, 2304, "MAEST pooled embedding is 3 × 768 = 2304-d");
    }

    #[test]
    fn test_pool_layer7_tokens_shape() {
        // Build a fake [1, 4, 768] tensor: tokens 0..4 with all-i values.
        let mut data = Vec::new();
        for tok in 0..4 {
            for _ in 0..MAEST_HIDDEN_DIM {
                data.push(tok as f32);
            }
        }
        let pooled = pool_layer7_tokens(&data, &[1, 4, MAEST_HIDDEN_DIM as i64]);
        assert_eq!(pooled.len(), MAEST_EMBEDDING_DIM);
        // CLS = token 0 → 0.0 throughout
        assert!(pooled[..MAEST_HIDDEN_DIM].iter().all(|&v| v == 0.0));
        // DIST = token 1 → 1.0 throughout
        assert!(pooled[MAEST_HIDDEN_DIM..2 * MAEST_HIDDEN_DIM].iter().all(|&v| v == 1.0));
        // signal mean = (2 + 3) / 2 = 2.5
        assert!(pooled[2 * MAEST_HIDDEN_DIM..].iter().all(|&v| (v - 2.5).abs() < 1e-6));
    }
}
