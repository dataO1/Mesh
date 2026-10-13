//! Live mode — generative breakbeat performance engine.
//!
//! Design: `docs/plans/2026-10-13-live-mode-design.md`.
//! Research: `docs/plans/2026-10-13-live-mode-research.md`.
//!
//! Live mode is a startup-selected alternative to the DJ engine. It runs one
//! multi-layer drum lane plus three instrument lanes, locked to one master
//! clock ([`clock`]) and one global key ([`harmony`]). Generators render the
//! next bar of [`event::LiveEvent`]s on a non-RT thread; the RT step player
//! fires them into sampler voices / CLAP instruments, lane FX and the glue chain.

pub mod event;
pub mod clock;
pub mod rng;
pub mod harmony;
pub mod style;
pub mod capture;
pub mod sampler;
pub mod glue;
pub mod generator;
pub mod scope;
pub mod state;
pub mod command;
pub mod lane;
pub mod loader;
pub mod render;
pub mod player;
pub mod engine;

/// Number of lanes: drums + 3 instrument lanes.
pub const NUM_LANES: usize = 4;
/// Steps (16ths) per bar.
pub const STEPS_PER_BAR: usize = 16;
/// Event time resolution: ticks per bar (96 per beat).
pub const TICKS_PER_BAR: u16 = 384;
/// Ticks per 16th step.
pub const TICKS_PER_STEP: u16 = TICKS_PER_BAR / STEPS_PER_BAR as u16;
/// Bars per chain / phrase.
pub const SLOTS_PER_CHAIN: usize = 4;
/// Fixed event capacity per lane per bar (covers 32nd rolls across sub-voices).
pub const MAX_EVENTS_PER_BAR: usize = 256;

/// Shared RT-safety test support. A crate may have only one global allocator,
/// and `assert_no_alloc`'s violation counter is global, so every live-mode
/// allocation test goes through [`test_support::count_allocs`].
#[cfg(all(test, debug_assertions))]
pub(crate) mod test_support {
    use std::sync::Mutex;

    #[global_allocator]
    static ALLOC_CHECK: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

    static SERIAL: Mutex<()> = Mutex::new(());

    /// Run `f` with allocation checking on and return how many allocator
    /// calls it made. Serialized, because the violation counter is global.
    pub(crate) fn count_allocs<R>(f: impl FnOnce() -> R) -> (R, u32) {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        assert_no_alloc::reset_violation_count();
        let r = assert_no_alloc::assert_no_alloc(f);
        let n = assert_no_alloc::violation_count();
        assert_no_alloc::reset_violation_count();
        (r, n)
    }
}
