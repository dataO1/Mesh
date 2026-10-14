//! Fixed step-pattern generator: plays a hand-written bar unchanged.
//! Used for sound-engine sign-off (design P1) and tests.
//!
//! A pattern has raw events (drums: absolute targets) and degree notes that
//! are resolved against the bar's harmony at render time, so the same pattern
//! follows key and progression changes. An optional fill variant is returned
//! for the fill bar. With [`FixedPattern::seeded`] the slot seed thins the
//! pattern and sets velocities, which gives tests and bench runs a
//! seed-dependent pattern without any generator logic.

use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::live::event::{flags, BarEvents, EventKind, LiveEvent};
use crate::live::glue::Role;
use crate::live::lane::{DRUM_TARGET_BACKBONE, DRUM_TARGET_HATS};
use crate::live::rng;
use crate::live::TICKS_PER_STEP;

/// A note written as a chord-relative scale degree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DegreeNote {
    pub tick: u16,
    /// 0-based degree relative to the bar's chord root.
    pub degree: i8,
    /// Scientific octave of the key's root (see `harmony::resolve_degree`).
    pub octave: i8,
    /// Gate length in ticks.
    pub gate: u16,
    pub vel: u8,
    /// [`flags`] bits (TIE, ACCENT).
    pub flags: u8,
}

impl DegreeNote {
    pub const fn new(tick: u16, degree: i8, octave: i8, gate: u16, vel: u8) -> Self {
        Self { tick, degree, octave, gate, vel, flags: 0 }
    }

    pub const fn with_flags(mut self, flags: u8) -> Self {
        self.flags = flags;
        self
    }
}

#[derive(Clone, Default)]
struct Bar {
    events: Vec<LiveEvent>,
    notes: Vec<DegreeNote>,
}

/// Plays the same events every bar.
#[derive(Clone, Default)]
pub struct FixedPattern {
    main: Bar,
    fill: Option<Bar>,
    seeded: bool,
}

impl FixedPattern {
    pub fn new(mut events: Vec<LiveEvent>) -> Self {
        events.sort_by_key(|e| e.tick);
        Self { main: Bar { events, notes: Vec::new() }, fill: None, seeded: false }
    }

    /// Pattern of degree notes, resolved against each bar's harmony.
    pub fn from_notes(notes: Vec<DegreeNote>) -> Self {
        Self { main: Bar { events: Vec::new(), notes }, fill: None, seeded: false }
    }

    /// Add degree notes to the main bar.
    pub fn with_notes(mut self, notes: Vec<DegreeNote>) -> Self {
        self.main.notes = notes;
        self
    }

    /// Events and notes returned for the fill bar. Without this the fill
    /// equals the main bar.
    pub fn with_fill(mut self, events: Vec<LiveEvent>, notes: Vec<DegreeNote>) -> Self {
        self.fill = Some(Bar { events, notes });
        self
    }

    /// Let the slot seed thin the pattern and set velocities. Exposes one
    /// parameter, `density` (probability an event is kept, 0.5..1.0).
    pub fn seeded(mut self) -> Self {
        self.seeded = true;
        self
    }

    /// Hand-written reference bar for a role (ear sign-off and calibration).
    // MUSICAL: a plain two-step DnB bar. Sign-off patterns, not generator output.
    pub fn reference(role: Role) -> Self {
        let step = |s: u16| s * TICKS_PER_STEP;
        match role {
            Role::Drums => {
                let hit = |s: u16, target: u8, vel: u8| LiveEvent {
                    tick: step(s),
                    kind: EventKind::Trigger,
                    target,
                    vel,
                    ..Default::default()
                };
                let (kick, snare) = (DRUM_TARGET_BACKBONE, DRUM_TARGET_BACKBONE + 1);
                let (hat, open) = (DRUM_TARGET_HATS, DRUM_TARGET_HATS + 1);
                let mut main = vec![
                    hit(0, kick, 120),
                    hit(10, kick, 110),
                    hit(4, snare, 120),
                    hit(12, snare, 120),
                    // Ghost snares at 35 % (A5 velocity tiers).
                    hit(7, snare, 44),
                    hit(9, snare, 44),
                    hit(14, open, 90),
                ];
                for s in (0..16).step_by(2) {
                    main.push(hit(s, hat, if s % 4 == 0 { 100 } else { 70 }));
                }
                // Fill: the same bar with a 32nd snare roll over the last beat.
                let mut fill: Vec<LiveEvent> = main.iter().copied().filter(|e| e.tick < step(12)).collect();
                for i in 0..8u16 {
                    let mut e = hit(12, snare, 60 + (i as u8) * 8);
                    e.tick = step(12) + i * TICKS_PER_STEP / 2;
                    fill.push(e);
                }
                main.sort_by_key(|e| e.tick);
                fill.sort_by_key(|e| e.tick);
                Self::new(main).with_fill(fill, Vec::new())
            }
            Role::Bass => Self::from_notes(vec![
                DegreeNote::new(step(0), 0, 1, 60, 115).with_flags(flags::ACCENT),
                DegreeNote::new(step(3), 0, 1, 20, 90),
                DegreeNote::new(step(6), -1, 1, 40, 100),
                DegreeNote::new(step(10), 0, 1, 60, 110),
                DegreeNote::new(step(14), 4, 1, 40, 100).with_flags(flags::TIE),
            ]),
            Role::Lead => Self::from_notes(vec![
                DegreeNote::new(step(2), 0, 4, 20, 100),
                DegreeNote::new(step(5), 2, 4, 20, 90),
                DegreeNote::new(step(11), 4, 4, 30, 100),
            ]),
            Role::Extra => Self::from_notes(vec![
                DegreeNote::new(0, 0, 3, 380, 90),
                DegreeNote::new(0, 4, 3, 380, 80),
            ]),
        }
    }
}

impl LaneGenerator for FixedPattern {
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let bar = match (&self.fill, ctx.fill) {
            (Some(fill), true) => fill,
            _ => &self.main,
        };
        let key = rng::RngKey::new(ctx.seed, ctx.lane, ctx.bar_in_phrase);
        let keep = 0.5 + 0.5 * ctx.params[0].clamp(0.0, 1.0);
        // Index doubles as the RNG coordinate so two events on one tick differ.
        let mut index = 0u8;
        let mut emit = |mut ev: LiveEvent, out: &mut BarEvents| {
            index = index.wrapping_add(1);
            if self.seeded && matches!(ev.kind, EventKind::Trigger | EventKind::NoteOn) {
                if !rng::chance(key.at(index, 1), keep) {
                    return;
                }
                ev.vel = rng::range_i32(key.at(index, 0), 48, 127) as u8;
            }
            out.push(ev);
        };
        for ev in &bar.events {
            emit(*ev, out);
        }
        for (i, n) in bar.notes.iter().enumerate() {
            emit(
                LiveEvent {
                    tick: n.tick,
                    kind: EventKind::NoteOn,
                    flags: n.flags,
                    key: ctx.harmony.resolve(n.degree as i32, n.octave as i32),
                    vel: n.vel,
                    gate: n.gate,
                    note_id: i as u16 + 1,
                    ..Default::default()
                },
                out,
            );
        }
        out.sort();
    }

    fn param_names(&self) -> &'static [&'static str] {
        if self.seeded { &["density"] } else { &[] }
    }

    fn param_defaults(&self) -> [f32; MAX_GEN_PARAMS] {
        [0.5; MAX_GEN_PARAMS]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::harmony::HarmonyCtx;
    use crate::live::STEPS_PER_BAR;
    use crate::music::MusicalKey;

    fn render(pattern: &FixedPattern, seed: u32, key: MusicalKey, chord: u8, fill: bool) -> Vec<LiveEvent> {
        let params = pattern.param_defaults();
        let ctx = BarCtx {
            sound_seed: 0,
            class: crate::live::form::SeedClass::Main,
            seed,
            lane: 1,
            role: Role::Bass,
            bar_in_phrase: 0,
            phrase: 0,
            fill,
            half_time: false,
            params: &params,
            harmony: HarmonyCtx { key, chord_degree: chord, progression: 0 },
            drum_mask: &[0; STEPS_PER_BAR],
        };
        let mut out = BarEvents::default();
        pattern.render_bar(&ctx, &mut out);
        out.as_slice().to_vec()
    }

    #[test]
    fn degree_notes_follow_key_and_chord() {
        let p = FixedPattern::reference(Role::Bass);
        let am = render(&p, 1, MusicalKey::new(9, true), 0, false);
        let cm = render(&p, 1, MusicalKey::new(0, true), 0, false);
        assert_eq!(am[0].key, 33, "A1");
        assert_eq!(cm[0].key, 24, "C1");
        // Chord on the 4th degree lifts the root by a fourth.
        let am_iv = render(&p, 1, MusicalKey::new(9, true), 3, false);
        assert_eq!(am_iv[0].key, 38, "D2");
        assert!(am.windows(2).all(|w| w[0].tick <= w[1].tick));
        assert!(am.iter().all(|e| e.gate > 0 && e.note_id > 0));
    }

    #[test]
    fn fill_variant_differs_only_when_asked() {
        let p = FixedPattern::reference(Role::Drums);
        let key = MusicalKey::new(9, true);
        let main = render(&p, 1, key, 0, false);
        let fill = render(&p, 1, key, 0, true);
        assert_ne!(main, fill);
        assert_eq!(render(&p, 99, key, 0, false), main, "unseeded pattern ignores the seed");
        let no_fill = FixedPattern::reference(Role::Lead);
        assert_eq!(render(&no_fill, 1, key, 0, true), render(&no_fill, 1, key, 0, false));
    }

    #[test]
    fn seeded_pattern_is_deterministic_per_seed() {
        let p = FixedPattern::reference(Role::Drums).seeded();
        let key = MusicalKey::new(9, true);
        let a = render(&p, 7, key, 0, false);
        assert_eq!(a, render(&p, 7, key, 0, false));
        assert_ne!(a, render(&p, 8, key, 0, false));
        assert_eq!(p.param_names(), &["density"]);
    }
}
