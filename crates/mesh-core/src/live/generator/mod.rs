//! Lane generators (design §6).
//!
//! A generator is a pure function of its [`BarCtx`]: the same context always
//! renders the same bar. Generators run on the bar-renderer thread, never on
//! the audio thread, and write only into the fixed-size [`BarEvents`].

use super::event::BarEvents;
use super::glue::Role;
use super::harmony::HarmonyCtx;
use super::STEPS_PER_BAR;

pub mod fixed;
pub mod registry;

pub use registry::build;

/// Maximum number of generator parameters a lane can expose to controls.
pub const MAX_GEN_PARAMS: usize = 8;

/// Everything a generator may read to render one bar.
#[derive(Debug, Clone, Copy)]
pub struct BarCtx<'a> {
    /// Seed of the chain slot being rendered.
    pub seed: u32,
    /// Lane index, 0 = drums.
    pub lane: u8,
    pub role: Role,
    /// 0..SLOTS_PER_CHAIN.
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
}

/// One lane's pattern generator.
pub trait LaneGenerator: Send {
    /// Render one bar into `out` (cleared by the caller). Must be
    /// deterministic in `ctx` and leave events sorted by tick.
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents);

    /// Parameter names; index = position in `BarCtx::params`.
    fn param_names(&self) -> &'static [&'static str];

    /// Default normalized value for each parameter in `param_names()`.
    fn param_defaults(&self) -> [f32; MAX_GEN_PARAMS] {
        [0.5; MAX_GEN_PARAMS]
    }
}
