//! Native Rust effects
//!
//! These effects are implemented directly in Rust for minimal latency
//! and maximum performance.

mod gain;
mod filter;

pub use gain::GainEffect;
pub use filter::DjFilterEffect;
