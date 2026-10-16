//! Lane generators (design §6).
//!
//! A generator is a pure function of its [`BarCtx`]: the same context always
//! renders the same bar. Generators run on the bar-renderer thread, never on
//! the audio thread, and write only into the fixed-size [`BarEvents`].

use super::event::BarEvents;
use super::form::SeedClass;
use super::glue::Role;
use super::harmony::HarmonyCtx;
use super::STEPS_PER_BAR;

pub mod bass;
pub mod blocks;
pub mod drums;
pub mod fixed;
pub mod idiom;
pub mod layout;
pub mod motif;
pub mod phrase;
pub mod lead;
pub mod recipe;
pub mod registry;
#[cfg(test)]
pub(crate) mod test_util;

pub use layout::DrumLayout;
pub use registry::{build, build_drums, build_lane, build_lane_motifs, build_with, build_with_motifs};

/// Maximum number of generator parameters a lane can expose to controls.
pub const MAX_GEN_PARAMS: usize = 12;

/// Everything a generator may read to render one bar.
#[derive(Debug, Clone, Copy)]
pub struct BarCtx<'a> {
    /// Seed of the letter being rendered (see [`super::form`]).
    pub seed: u32,
    /// Seed that picks sounds (drum pools): the master seed in main bars, the
    /// letter seed in filler bars.
    pub sound_seed: u32,
    /// Main or filler material.
    pub class: SeedClass,
    /// Lane index, 0 = drums.
    pub lane: u8,
    pub role: Role,
    /// Global bar % 4.
    pub bar_in_phrase: u8,
    pub phrase: u32,
    /// Render the fill variant of this bar.
    pub fill: bool,
    /// Half-time feel (the clock is unchanged).
    pub half_time: bool,
    /// Normalized generator parameters, indexed like `param_names()`.
    pub params: &'a [f32; MAX_GEN_PARAMS],
    pub harmony: HarmonyCtx,
    /// The drum lane's onsets for this bar (velocity per step, 0 = none).
    /// All zero while the drum lane itself is rendered.
    pub drum_mask: &'a [u8; STEPS_PER_BAR],
    /// Per-note variation per parameter: each note reads its parameter up to
    /// this far from the value, seeded per note ([`blocks::note_param`]).
    pub jitter: [f32; MAX_GEN_PARAMS],
    /// The bass lane's onsets this bar, one bit per step (lanes after the
    /// bass follow it). 0 for the drum and bass lanes.
    pub bass_onsets: u16,
    /// The bass lane's motif onsets over the phrase, one step mask per bar
    /// (all 0 without a bass clip). Lead clips are picked against them.
    pub bass_phrase: [u16; 4],
    /// The drum lane fills this bar (a filler segment's last bar). Motif
    /// leads rest.
    pub drum_fill: bool,
}

/// One lane's pattern generator.
pub trait LaneGenerator: Send {
    /// Render one bar into `out` (cleared by the caller). Must be
    /// deterministic in `ctx` and leave events sorted by tick.
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents);

    /// Parameter names; index = position in `BarCtx::params`.
    fn param_names(&self) -> &'static [&'static str];

    /// Drum lane: voices on by default in this bar (bit per
    /// [`DrumVoice`](layout::DrumVoice)); the pads flip them. Other lanes: all.
    fn voice_mask(&self, _ctx: &BarCtx) -> u8 {
        0xFF
    }

    /// Motif mode: the onsets the clip this seed picks has in each bar of the
    /// phrase, so the lead lane can interlock with it. `None` without clips.
    fn motif_phrase(&self, _seed: u32, _lane: u8) -> Option<[u16; 4]> {
        None
    }

    /// Default normalized value for each parameter in `param_names()`.
    fn param_defaults(&self) -> [f32; MAX_GEN_PARAMS] {
        [0.5; MAX_GEN_PARAMS]
    }
}
