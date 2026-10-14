//! Shared helpers for generator tests.

use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::live::event::{BarEvents, LiveEvent};
use crate::live::glue::Role;
use crate::live::harmony::HarmonyCtx;
use crate::live::{MAX_EVENTS_PER_BAR, STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};
use crate::music::MusicalKey;

/// A minor, chord on the root.
pub(crate) fn a_minor() -> HarmonyCtx {
    HarmonyCtx { key: MusicalKey::new(9, true), chord_degree: 0, progression: 0 }
}

/// Context for lane 0 in A minor. Tests edit the fields they vary.
pub(crate) fn ctx<'a>(
    seed: u32,
    bar_in_phrase: u8,
    params: &'a [f32; MAX_GEN_PARAMS],
    drum_mask: &'a [u8; STEPS_PER_BAR],
) -> BarCtx<'a> {
    BarCtx {
        sound_seed: 0,
        class: crate::live::form::SeedClass::Main,
        seed,
        lane: 0,
        role: Role::Drums,
        bar_in_phrase,
        phrase: 0,
        fill: false,
        half_time: false,
        params,
        harmony: a_minor(),
        drum_mask,
    }
}

pub(crate) fn render(g: &dyn LaneGenerator, ctx: &BarCtx) -> BarEvents {
    let mut out = BarEvents::default();
    g.render_bar(ctx, &mut out);
    out
}

/// The invariants every rendered bar must hold.
pub(crate) fn assert_well_formed(bar: &BarEvents) {
    assert!(bar.len() <= MAX_EVENTS_PER_BAR);
    let ev = bar.as_slice();
    for w in ev.windows(2) {
        assert!(w[0].tick <= w[1].tick, "events not sorted: {} > {}", w[0].tick, w[1].tick);
    }
    for e in ev {
        assert!(e.tick < TICKS_PER_BAR, "tick {} outside the bar", e.tick);
        assert!((1..=127).contains(&e.vel), "velocity {}", e.vel);
        assert!(e.tick + e.gate <= TICKS_PER_BAR, "gate crosses the bar end");
        assert!(e.key <= 127);
    }
}

/// ASCII step grid, one row per `(target, label)`: `X` loud, `x` medium,
/// `o` quiet, `.` empty. Used for golden snapshots.
pub(crate) fn grid(events: &[LiveEvent], rows: &[(u8, char)]) -> String {
    let mut out = String::new();
    for &(target, label) in rows {
        out.push(label);
        out.push(' ');
        for s in 0..STEPS_PER_BAR {
            let vel = events
                .iter()
                .filter(|e| e.target == target && (e.tick / TICKS_PER_STEP) as usize == s)
                .map(|e| e.vel)
                .max();
            out.push(match vel {
                None => '.',
                Some(v) if v >= 110 => 'X',
                Some(v) if v >= 70 => 'x',
                Some(_) => 'o',
            });
        }
        out.push('\n');
    }
    out
}

/// Pitched notes as `step:key` pairs, for golden snapshots.
pub(crate) fn notes(events: &[LiveEvent]) -> String {
    events
        .iter()
        .map(|e| format!("{}:{}", e.tick / TICKS_PER_STEP, e.key))
        .collect::<Vec<_>>()
        .join(" ")
}
