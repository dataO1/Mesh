//! Audio engine - Deck, Mixer, latency compensation
//!
//! This module contains the core audio engine components for the DJ player:
//! - [`Deck`]: Individual track player with stems and effect chains
//! - [`Mixer`]: Combines deck outputs with volume/filter controls
//! - [`LatencyCompensator`]: Per-stem latency compensation using delay lines
//! - [`AudioEngine`]: Main engine tying everything together
//!
//! # Multi-Threading Architecture
//!
//! The engine is designed for real-time audio processing with these patterns:
//!
//! ## 1. Pre-Allocation Strategy
//!
//! All buffers are pre-allocated at startup to [`MAX_BUFFER_SIZE`] (8192 samples),
//! eliminating allocations in the audio callback. This includes:
//! - Per-deck stem buffers (4 decks × 4 stems = 16 buffers)
//! - Latency compensation delay lines (16 ring buffers)
//! - Mixer channel buffers (4 channels)
//!
//! ## 2. Parallel Stem Processing
//!
//! Each deck processes its 4 stems in parallel using Rayon:
//! ```text
//! Deck.process() → Rayon par_iter → [Vocals, Drums, Bass, Other] → Sum
//! ```
//! The Rayon thread pool is initialized at startup (`main.rs`) with 4 threads
//! to match `NUM_DECKS`, ensuring worker threads are ready before audio starts.
//!
//! ## 3. Lock-Free UI Reads
//!
//! [`DeckAtomics`] provides lock-free access to frequently-read state:
//! - Playhead position (for waveform display)
//! - Play/pause state (for UI indicators)
//! - Loop state (for loop markers)
//!
//! All atomics use `Ordering::Relaxed` since only visibility is required,
//! not synchronization with other memory operations.
//!
//! ## 4. Latency Compensation
//!
//! Different effect chains have different latencies. To keep stems phase-aligned:
//! ```text
//! Per-stem latency = effect_chain_latency + timestretch_latency
//! Compensation delay = max_latency - stem_latency
//! ```
//! The [`LatencyCompensator`] applies per-stem delays after parallel processing
//! but before summing, ensuring sample-accurate alignment.
//!
//! # Performance Characteristics
//!
//! | Metric | Value | Notes |
//! |--------|-------|-------|
//! | Audio buffer | 256 samples typical | ~5.8ms @ 44.1kHz |
//! | Lock hold time | <10µs | Microseconds only |
//! | Pre-allocated memory | ~1.5MB | Buffers for 4 decks |
//! | Max latency compensation | 4410 samples | 100ms @ 44.1kHz |

mod deck;
mod engine;
mod latency;
mod mixer;

pub use deck::*;
pub use engine::*;
pub use latency::*;
pub use mixer::*;
