# Essentia-rs API Reference

This document provides a comprehensive reference for using the `essentia-rs` Rust bindings to the Essentia audio analysis library. This is specific to the Rust crate and includes important differences from the C++ API.

## Table of Contents

1. [Crate Structure](#crate-structure)
2. [Core Concepts](#core-concepts)
3. [API Pattern](#api-pattern)
4. [DJ-Relevant Algorithms](#dj-relevant-algorithms)
5. [Effects & Audio Processing Algorithms](#effects--audio-processing-algorithms)
6. [Code Examples](#code-examples)
7. [Important Differences from C++ API](#important-differences-from-c-api)
8. [Troubleshooting](#troubleshooting)

---

## Crate Structure

The `essentia` crate (v0.1.5) is organized as follows:

```
essentia/
├── algorithm/           # All algorithm implementations (auto-generated at build time)
│   ├── rhythm/          # BPM, beat tracking, tempo
│   ├── tonal/           # Key detection, chords, tuning
│   ├── spectral/        # Spectral analysis
│   ├── extractors/      # High-level feature extractors
│   ├── filters/         # Audio filters
│   ├── transformations/ # FFT, DCT, etc.
│   └── ...
├── data/                # DataContainer types and traits
├── essentia/            # Main Essentia instance
├── pool/                # Pool for storing results
└── phantom/             # Phantom types for type-safe data containers
```

### Key Imports

```rust
// Main Essentia instance
use essentia::essentia::Essentia;

// Algorithms (examples)
use essentia::algorithm::rhythm::rhythm_extractor_2013::RhythmExtractor2013;
use essentia::algorithm::tonal::key_extractor::KeyExtractor;
use essentia::algorithm::tonal::key::Key;

// Data extraction trait (REQUIRED to call .get() on results)
use essentia::data::GetFromDataContainer;

// Algorithm states
use essentia::Configured;
use essentia::Initialized;
```

---

## Core Concepts

### Typed State Machine Pattern

Unlike the C++ API which uses runtime checks, essentia-rs uses Rust's type system to enforce correct algorithm usage at **compile time**.

Each algorithm has two states:
- **`Initialized`**: Can set parameters, then call `.configure()`
- **`Configured`**: Can call `.compute()` with input data

```rust
// State transitions:
Algorithm<Initialized>  --[.configure()]--> Algorithm<Configured> --[.compute()]--> Result
```

### DataContainer

Results are wrapped in `DataContainer<T>` which provides type-safe access to Essentia's internal data. Use the `GetFromDataContainer` trait to extract values:

```rust
use essentia::data::GetFromDataContainer;

let bpm_container = result.bpm()?;  // DataContainer<Float>
let bpm: f32 = bpm_container.get(); // Extract the actual value
```

### Phantom Types

The crate uses phantom types for type safety:
- `Float` → `f32`
- `VectorFloat` → `Vec<f32>`
- `String` → `String`
- `VectorString` → `Vec<String>`
- `Int` → `i32`
- `Bool` → `bool`

---

## API Pattern

### General Usage Pattern

```rust
use essentia::essentia::Essentia;
use essentia::algorithm::some_category::some_algorithm::SomeAlgorithm;
use essentia::data::GetFromDataContainer;

// 1. Create Essentia instance (no global init needed, unlike C++)
let essentia = Essentia::new();

// 2. Create algorithm from Essentia instance
let algo = essentia.create::<SomeAlgorithm>();

// 3. Set parameters using named methods (NOT generic .parameter())
let algo = algo
    .some_param(value)?       // Each parameter has its own method
    .another_param(value)?
    .configure()?;            // Transition to Configured state

// 4. Compute with input
let result = algo.compute(&input_data)?;

// 5. Extract outputs using named methods + .get()
let output_value = result.output_name()?.get();
```

### Parameter Methods

**IMPORTANT**: Unlike C++ which uses `configure({{"param", value}})`, essentia-rs generates a dedicated method for each parameter:

| C++ | Rust |
|-----|------|
| `algo->configure({{"minTempo", 40}})` | `algo.min_tempo(40)?` |
| `algo->configure({{"method", "multifeature"}})` | `algo.method("multifeature")?` |
| `algo->configure({{"profileType", "edma"}})` | `algo.profile_type("edma")?` |

Parameter names are converted from camelCase to snake_case.

### Output Methods

Similarly, outputs are accessed via named methods:

| C++ | Rust |
|-----|------|
| `algo->output("bpm").get()` | `result.bpm()?.get()` |
| `algo->output("ticks").get()` | `result.ticks()?.get()` |
| `algo->output("key").get()` | `result.key()?.get()` |

---

## DJ-Relevant Algorithms

### RhythmExtractor2013 (BPM + Beat Detection)

**Location**: `essentia::algorithm::rhythm::rhythm_extractor_2013::RhythmExtractor2013`

The most accurate algorithm for BPM and beat position detection, optimized for electronic/dance music.

**Input**: Mono audio samples at 44100 Hz

**Parameters**:
| Method | Type | Default | Description |
|--------|------|---------|-------------|
| `min_tempo(v)` | i32 | 40 | Minimum detectable BPM |
| `max_tempo(v)` | i32 | 208 | Maximum detectable BPM |
| `method(v)` | &str | "multifeature" | Detection method: "multifeature" or "degara" |

**Outputs** (via `RhythmExtractor2013Result`):
| Method | Type | Description |
|--------|------|-------------|
| `bpm()` | f32 | Detected tempo in BPM |
| `ticks()` | Vec<f32> | Beat positions in seconds |
| `confidence()` | f32 | Detection confidence (only with "multifeature") |
| `estimates()` | Vec<f32> | BPM candidates distribution |
| `bpm_intervals()` | Vec<f32> | Inter-beat intervals in seconds |

**Example**:
```rust
use essentia::essentia::Essentia;
use essentia::algorithm::rhythm::rhythm_extractor_2013::RhythmExtractor2013;
use essentia::data::GetFromDataContainer;

let essentia = Essentia::new();
let rhythm = essentia.create::<RhythmExtractor2013>()
    .min_tempo(40)?
    .max_tempo(208)?
    .method("multifeature")?
    .configure()?;

let result = rhythm.compute(&mono_samples)?;

let bpm: f32 = result.bpm()?.get();
let beats: Vec<f32> = result.ticks()?.get();
let confidence: f32 = result.confidence()?.get();
```

---

### KeyExtractor (Musical Key Detection)

**Location**: `essentia::algorithm::tonal::key_extractor::KeyExtractor`

All-in-one key detection - internally computes spectrum, spectral peaks, HPCP, then key.

**Input**: Mono audio samples at 44100 Hz

**Key Parameters**:
| Method | Type | Default | Description |
|--------|------|---------|-------------|
| `profile_type(v)` | &str | "bgate" | Key profile (see below) |
| `frame_size(v)` | i32 | 4096 | Analysis frame size |
| `hop_size(v)` | i32 | 4096 | Hop between frames |
| `sample_rate(v)` | f32 | 44100.0 | Input sample rate |

**Profile Types** (for `profile_type`):
- `"bgate"` - Default, good general purpose
- `"edma"` - **Recommended for EDM/DJ use** (Electronic Dance Music Analysis)
- `"edmm"` - Electronic Dance Music Minor
- `"krumhansl"` - Krumhansl-Schmuckler
- `"temperley"` - Temperley-Kostka-Payne
- `"weichai"` - Wei Chai
- `"tonictriad"` - Simple tonic triad
- `"thpcp"` - Tonal Harmony PCP
- `"shaath"` - Shaath
- `"gomez"` - Gomez
- `"noland"` - Noland
- `"faraldo"` - Faraldo
- `"pentatonic"` - Pentatonic
- `"braw"` - Basic Raw

**Outputs** (via `KeyExtractorResult`):
| Method | Type | Description |
|--------|------|-------------|
| `key()` | String | Musical key note: "A", "Bb", "C#", etc. |
| `scale()` | String | Scale type: "major" or "minor" |
| `strength()` | f32 | Detection confidence |

**Example**:
```rust
use essentia::essentia::Essentia;
use essentia::algorithm::tonal::key_extractor::KeyExtractor;
use essentia::data::GetFromDataContainer;

let essentia = Essentia::new();
let key_algo = essentia.create::<KeyExtractor>()
    .profile_type("edma")?  // Best for electronic music
    .configure()?;

let result = key_algo.compute(&mono_samples)?;

let key: String = result.key()?.get();     // e.g., "A"
let scale: String = result.scale()?.get(); // "major" or "minor"
let strength: f32 = result.strength()?.get();

// Format as "Am", "C", "F#m", etc.
let key_string = if scale == "minor" {
    format!("{}m", key)
} else {
    key
};
```

---

### BeatTrackerMultiFeature (Detailed Beat Tracking)

**Location**: `essentia::algorithm::rhythm::beat_tracker_multi_feature::BeatTrackerMultiFeature`

More detailed beat tracking with confidence scores per beat.

**Input**: Mono audio at 44100 Hz

**Outputs**:
- `ticks()` - Beat positions in seconds
- `confidence()` - Confidence score [0.0, 5.32] (higher = more confident)

---

### BeatTrackerDegara (Alternative Beat Tracker)

**Location**: `essentia::algorithm::rhythm::beat_tracker_degara::BeatTrackerDegara`

Alternative beat tracking algorithm.

---

### Danceability

**Location**: `essentia::algorithm::rhythm::danceability::Danceability`

Estimates how "danceable" a track is (0.0 to ~3.0).

---

### OnsetDetection / OnsetRate

**Location**: `essentia::algorithm::rhythm::onset_detection::OnsetDetection`

Detects note/beat onsets - useful for transition point detection.

---

## Effects & Audio Processing Algorithms

These algorithms could be useful for future audio effects in mesh-player.

### Filters

| Algorithm | Location | Description |
|-----------|----------|-------------|
| `HighPass` | `algorithm::filters::high_pass` | High-pass filter |
| `LowPass` | `algorithm::filters::low_pass` | Low-pass filter |
| `BandPass` | `algorithm::filters::band_pass` | Band-pass filter |
| `BandReject` | `algorithm::filters::band_reject` | Notch filter |
| `EqualLoudness` | `algorithm::filters::equal_loudness` | Equal loudness contour |
| `DCRemoval` | `algorithm::filters::dc_removal` | DC offset removal |
| `AllPass` | `algorithm::filters::all_pass` | All-pass filter (phase shift) |
| `MovingAverage` | `algorithm::filters::moving_average` | Smoothing filter |

### Spectral Processing

| Algorithm | Location | Description |
|-----------|----------|-------------|
| `FFT` | `algorithm::transformations::fft` | Fast Fourier Transform |
| `IFFT` | `algorithm::transformations::ifft` | Inverse FFT |
| `Spectrum` | `algorithm::spectral::spectrum` | Compute magnitude spectrum |
| `SpectralPeaks` | `algorithm::spectral::spectral_peaks` | Find spectral peaks |
| `SpectralContrast` | `algorithm::spectral::spectral_contrast` | Spectral contrast |
| `SpectralCentroid` | `algorithm::spectral::spectral_centroid_time` | Brightness measure |

### Dynamics

| Algorithm | Location | Description |
|-----------|----------|-------------|
| `Loudness` | `algorithm::loudness_dynamics::loudness` | Loudness measurement |
| `DynamicComplexity` | `algorithm::loudness_dynamics::dynamic_complexity` | Dynamic range analysis |
| `Clipper` | `algorithm::standard::clipper` | Hard clipper |
| `Limiter` | `algorithm::loudness_dynamics::loudness_ebur128` | Integrated loudness (for limiting reference) |

### Pitch & Tonal

| Algorithm | Location | Description |
|-----------|----------|-------------|
| `PitchYin` | `algorithm::pitch::pitch_yin` | Pitch detection (YIN) |
| `PitchMelodia` | `algorithm::pitch::pitch_melodia` | Melodic pitch tracking |
| `Vibrato` | `algorithm::pitch::vibrato` | Vibrato detection |
| `Dissonance` | `algorithm::tonal::dissonance` | Dissonance measure |
| `TuningFrequency` | `algorithm::tonal::tuning_frequency` | Detect tuning (A=440?) |
| `HPCP` | `algorithm::tonal::hpcp` | Harmonic Pitch Class Profile |
| `Key` | `algorithm::tonal::key` | Key from HPCP input |
| `ChordsDetection` | `algorithm::tonal::chords_detection` | Chord detection |

### Envelope & Temporal

| Algorithm | Location | Description |
|-----------|----------|-------------|
| `Envelope` | `algorithm::envelope_sfx::envelope` | Amplitude envelope |
| `LogAttackTime` | `algorithm::envelope_sfx::log_attack_time` | Attack time |
| `EffectiveDuration` | `algorithm::duration_silence::effective_duration` | Effective duration |
| `SilenceRate` | `algorithm::duration_silence::silence_rate` | Silence detection |
| `StartStopSilence` | `algorithm::duration_silence::start_stop_silence` | Trim silence |

### Music Information Retrieval

| Algorithm | Location | Description |
|-----------|----------|-------------|
| `MusicExtractor` | `algorithm::extractors::music_extractor` | Full track analysis |
| `FreesoundExtractor` | `algorithm::extractors::freesound_extractor` | Freesound-style features |
| `LowLevelSpectralExtractor` | `algorithm::extractors::low_level_spectral_extractor` | Low-level features |

---

## Code Examples

### Complete BPM Detection Example

```rust
use anyhow::{Context, Result};
use essentia::algorithm::rhythm::rhythm_extractor_2013::RhythmExtractor2013;
use essentia::data::GetFromDataContainer;
use essentia::essentia::Essentia;

pub fn detect_bpm(samples: &[f32]) -> Result<(f64, Vec<f64>)> {
    // Create Essentia instance
    let essentia = Essentia::new();

    // Create and configure algorithm
    let rhythm = essentia
        .create::<RhythmExtractor2013>()
        .min_tempo(40)?
        .max_tempo(208)?
        .method("multifeature")?
        .configure()?;

    // Run computation
    let result = rhythm.compute(samples)?;

    // Extract results
    let bpm: f32 = result.bpm()?.get();
    let ticks: Vec<f32> = result.ticks()?.get();

    // Convert to f64
    let bpm = bpm as f64;
    let beats: Vec<f64> = ticks.iter().map(|&t| t as f64).collect();

    Ok((bpm, beats))
}
```

### Complete Key Detection Example

```rust
use anyhow::{Context, Result};
use essentia::algorithm::tonal::key_extractor::KeyExtractor;
use essentia::data::GetFromDataContainer;
use essentia::essentia::Essentia;

pub fn detect_key(samples: &[f32]) -> Result<String> {
    let essentia = Essentia::new();

    let key_algo = essentia
        .create::<KeyExtractor>()
        .profile_type("edma")?  // Electronic Dance Music profile
        .configure()?;

    let result = key_algo.compute(samples)?;

    let key: String = result.key()?.get();
    let scale: String = result.scale()?.get();

    // Format as standard DJ notation: "Am", "C", "F#m"
    let key_string = if scale == "minor" {
        format!("{}m", key)
    } else {
        key
    };

    Ok(key_string)
}
```

### Using Low-Level Key Algorithm (with HPCP)

If you need more control, you can use the HPCP → Key pipeline:

```rust
use essentia::algorithm::tonal::hpcp::Hpcp;
use essentia::algorithm::tonal::key::Key;
use essentia::algorithm::spectral::spectrum::Spectrum;
use essentia::algorithm::spectral::spectral_peaks::SpectralPeaks;
// ... requires more setup, KeyExtractor is recommended
```

---

## Important Differences from C++ API

| Aspect | C++ | Rust (essentia-rs) |
|--------|-----|-------------------|
| Initialization | `essentia::init()` required | Not needed, just `Essentia::new()` |
| Algorithm creation | `AlgorithmFactory::create("Name")` | `essentia.create::<AlgorithmName>()` |
| Parameter setting | `algo->configure({{"param", value}})` | `algo.param_name(value)?` |
| Input setting | `algo->input("name").set(data)` | Part of `.compute(data)` |
| Running | `algo->compute()` | `algo.compute(&data)?` |
| Output access | `algo->output("name").get()` | `result.name()?.get()` |
| Error handling | Exceptions | `Result<T, Error>` |
| Memory | Manual management | Automatic (Rust ownership) |
| Thread safety | Manual locks | Compile-time (!Send, !Sync on algorithms) |
| Algorithm naming | `"RhythmExtractor2013"` string | `RhythmExtractor2013` type |
| Module path | Flat namespace | `algorithm::category::algo_name::AlgoName` |

### Algorithm Name Mapping

C++ algorithm names map to Rust as follows:
- CamelCase preserved for struct names
- snake_case for module names

| C++ Name | Rust Module | Rust Struct |
|----------|-------------|-------------|
| `RhythmExtractor2013` | `rhythm::rhythm_extractor_2013` | `RhythmExtractor2013` |
| `KeyExtractor` | `tonal::key_extractor` | `KeyExtractor` |
| `BeatTrackerMultiFeature` | `rhythm::beat_tracker_multi_feature` | `BeatTrackerMultiFeature` |
| `SpectralPeaks` | `spectral::spectral_peaks` | `SpectralPeaks` |

---

## Troubleshooting

### Common Errors

**"unresolved import"**: Algorithm types are deeply nested:
```rust
// Wrong
use essentia::RhythmExtractor2013;

// Correct
use essentia::algorithm::rhythm::rhythm_extractor_2013::RhythmExtractor2013;
```

**"no method named `parameter`"**: Use named parameter methods:
```rust
// Wrong (C++ style)
algo.parameter("minTempo", 40)?

// Correct (Rust style)
algo.min_tempo(40)?
```

**"attempted to take value of method"**: Results require `.get()`:
```rust
// Wrong
let bpm = result.bpm;

// Correct
let bpm: f32 = result.bpm()?.get();
```

**"trait bound `GetFromDataContainer` not satisfied"**: Import the trait:
```rust
use essentia::data::GetFromDataContainer;
```

### Sample Rate Requirements

Most algorithms expect **44100 Hz** sample rate. If your audio is different:
1. Resample before analysis, OR
2. Set `sample_rate` parameter if the algorithm supports it

### Memory Considerations

- Algorithms are **!Send** and **!Sync** - cannot be shared across threads
- Create new instances per thread if needed
- Results borrow from algorithms - extract data with `.get()` to own it

---

## Nix Build Notes

The `essentia-rs` crate requires the Essentia C++ library. In the mesh project:

1. Essentia is built from source in `flake.nix`
2. Environment variables are set in `shellHook`:
   - `PKG_CONFIG_PATH` - for pkg-config to find essentia
   - `LD_LIBRARY_PATH` - for runtime linking
   - `USE_TENSORFLOW=0` - disable TensorFlow (not needed for basic analysis)

---

## References

- [Essentia Official Documentation](https://essentia.upf.edu/documentation.html)
- [Essentia Algorithm Reference](https://essentia.upf.edu/algorithms_reference.html)
- [essentia-rs GitHub](https://github.com/lagmoellertim/essentia-rs)
- [RhythmExtractor2013 Reference](https://essentia.upf.edu/reference/std_RhythmExtractor2013.html)
- [KeyExtractor Reference](https://essentia.upf.edu/reference/std_KeyExtractor.html)
