//! USB Export Service
//!
//! Provides a thread pool-based export service for USB operations.
//! Each track export is atomic: WAV copy + DB sync + progress callback.
//!
//! # Architecture
//!
//! ```text
//! Domain Layer
//!     │
//!     │ export_tracks()
//!     ▼
//! ExportService (rayon ThreadPool, 4 threads)
//!     │
//!     │ par_iter().for_each()
//!     ▼
//! Per-Track Worker:
//!   1. Copy WAV with verification
//!   2. sync_track_atomic() to USB DB
//!   3. Send ExportProgress::TrackComplete
//!     │
//!     │ ExportProgress (mpsc)
//!     ▼
//! UI Layer (subscription)
//! ```

mod message;
mod service;

pub use message::ExportProgress;
pub use service::ExportService;
