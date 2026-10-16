//! Native glue chain — fixed-topology mix bus for live mode (design §5.4–§5.6, D20, D27).
//!
//! Every lane passes through role-specific treatment before it reaches the
//! master. The topology is fixed; only the parameters come from the style:
//!
//! ```text
//!  lane[i] ─► trim (LUFS calibration, static dB)
//!          ─► role high-pass        (lead / extra; drums off by default)
//!          ─► mono-below crossover  (bass only)
//!          ─► compressor → saturation (bass / lead / extra)
//!          ─► full-band duck        (bass / lead / extra, keyed by the main kit)
//!          ─► band duck             (bass only, keyed by lead + extra, per band)
//!          ├─► delay send ─┐  (post-lane taps, per-lane levels)
//!          ├─► reverb send ┤
//!          │               │
//!  Drums ──► drum bus: comp 1 → comp 2 → sat ───┐
//!  Bass  ───────────────────────────────────────┴─► low bus ─► shared drive ─┐
//!  Lead / Extra ─────────────────────────────────────────────────────────────┤
//!  delay return / reverb return ─────────────────────────────────────────────┤
//!                                                                             ▼
//!          master compressor ─► saturation ─► MasterLimiter ─► MasterClipper ─► out
//! ```
//!
//! **Shared drive routing**: drums and bass are summed into one "low bus" and
//! saturated *together* (insert on the sum, Switch Angel's glue technique), so
//! the two intermodulate. The stage has a dry/wet `mix`: 1.0 is a pure insert,
//! lower values behave like a send/return with the return at `mix`.
//!
//! All stages are pre-allocated and real-time safe: `process` never
//! allocates, locks or logs. Parameter updates (`set_params`, `set_lane`) are
//! also allocation-free so they can run on the audio thread.
//!
//! [`loudness`] holds the offline BS.1770 meter used by the style tool to
//! compute the per-option trims.

mod bandduck;
mod chain;
mod dynamics;
mod filter;
mod ducker;
pub mod loudness;
mod params;
mod saturation;
mod sends;

#[cfg(all(test, debug_assertions))]
mod rt_tests;

pub use chain::{GlueChain, GlueKey, GlueMeters, LaneSetup};
pub use dynamics::{static_curve_db, Compressor};
pub use ducker::{duck_full_band, DuckEnvelope};
pub use filter::{MonoBelow, RoleHighPass};
pub use loudness::{measure_lufs, trim_db};
pub use bandduck::BandDuck;
pub use params::{
    BandDuckParams, CompressorParams, DelayParams, DrumBusParams, DriveParams, DuckParams, GlueParams,
    HighPassParams, LaneDynamicsParams, MasterParams, MonoBelowParams, ReverbParams,
    RoleDynamicsParams, RoleHighPassParams, SaturationParams, Slope,
};
pub use saturation::Saturator;
pub use sends::SharedSends;

use serde::{Deserialize, Serialize};

/// A lane's mix role. Selects its glue treatment and its LUFS target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Drums,
    Bass,
    Lead,
    #[default]
    Extra,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Drums, Role::Bass, Role::Lead, Role::Extra];

    /// Default integrated-loudness target for calibration (design §5.5).
    /// The style can override it.
    pub fn default_lufs_target(self) -> f64 {
        // MUSICAL: example targets from design §5.5, to be ear-tested in P1.
        match self {
            Role::Drums => -12.0,
            Role::Bass => -14.0,
            Role::Lead => -16.0,
            Role::Extra => -18.0,
        }
    }
}

/// dB → linear amplitude.
#[inline]
pub(crate) fn db_to_lin(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Linear amplitude → dB, floored at −160 dB for silence.
#[inline]
pub(crate) fn lin_to_db(lin: f32) -> f32 {
    20.0 * lin.max(1e-8).log10()
}
