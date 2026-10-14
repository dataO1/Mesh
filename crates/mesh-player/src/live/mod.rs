//! Live mode application (design `docs/plans/2026-10-13-live-mode-design.md`).
//!
//! Selected at startup with `mode: live` in `player-config.yaml` (D35):
//!
//! ```yaml
//! mode: live
//! live:
//!   style: breaks          # folder under <collection>/styles/
//!   f1_left_serial: null   # optional, USB serial of the left Kontrol F1
//! ```
//!
//! - [`boot`]: startup (style, audio backend, iced application).
//! - [`app`]: `LiveApp`, the screen of design §8.
//! - [`controls`]: the two Kontrol F1s mapped onto live commands (§7).
//! - [`input`]: controller polling and LED feedback thread.
//! - [`keys`]: keyboard fallback for development without F1s.
//! - [`readout`], [`header`]: pure text formatting of the screen.
//! - [`startup`]: style resolution and its error report.

pub mod app;
pub mod boot;
pub mod controls;
pub mod header;
pub mod input;
pub mod keys;
pub mod readout;
pub mod startup;

pub use boot::run;
