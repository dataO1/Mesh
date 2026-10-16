//! Live-mode sampler (design §5.1, D31).
//!
//! One engine serves every sample-based lane: one-shots, drum kits, break
//! slices, multisampled instruments and stem phrases.
//!
//! - [`SampleData`]: decoded stereo audio with root key, tuning and an optional
//!   typed slice table. Built off-RT and shared as `basedrop::Shared`.
//! - [`SampleBank`]: maps an event `target` to a [`SlotDef`] (whole sample,
//!   slice, or key zones), each with its own default choke group, so a drum lane
//!   can hold several sub-voices.
//! - [`Sampler`]: one per lane. Fixed voice pool, events queued with a sample
//!   offset and applied sample-accurately inside [`Sampler::process`].
//!   See its docs for the exact event semantics.
//! - [`LaneSamplerParams`]: block-rate lane parameters.
//! - [`Interpolation`]: resampling quality, selectable per lane for the board
//!   benchmark (design R2).

mod data;
mod engine;
mod interp;
mod params;
mod voice;

#[cfg(test)]
mod tests;

pub use data::{SampleBank, SampleData, Slice, SliceType, SlotDef, MAX_LAYERS};
pub use engine::{Sampler, SamplerMeters};
pub use interp::{sinc_tables, Interpolation, SincTables};
pub use params::{
    attack_byte, attack_secs, decay_secs, glide_secs, LaneSamplerParams, ModConfig, CHOKE_FADE_SECS, CUTOFF_MAX_HZ,
    CUTOFF_MIN_HZ, CUTOFF_OFFSET_OCTAVES, EDGE_FADE_SECS, MAX_CC_MAPS, MAX_ZONE_MODS, CcCurve, CcMap, ModEnv, ModLfo, NO_CC, SoundShape, ZoneMod, ZoneTarget,
};
