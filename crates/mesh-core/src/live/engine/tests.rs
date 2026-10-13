//! Engine integration tests: the fixture style rendered offline, with the
//! bar renderer serviced between blocks (deterministic).

use super::*;
use crate::live::capture::CaptureScope;
use crate::live::event::{EventKind, LiveEvent};
use crate::live::generator::fixed::{DegreeNote, FixedPattern};
use crate::live::generator::LaneGenerator;
use crate::live::lane::{DRUM_TARGET_BACKBONE, IMMEDIATE_FADE_SECS, QUANTIZED_FADE_SECS, VOID_FADE_SECS};
use crate::live::loader::fixture_dir;
use crate::live::state::{move_bits, SpanTag};
use crate::live::{STEPS_PER_BAR, TICKS_PER_STEP};

const SR: u32 = 48_000;
const BPM: f64 = 172.0;
/// Level below which a lane counts as silent.
const QUIET: f32 = 1e-4;

type Gens = [Option<Box<dyn LaneGenerator>>; NUM_LANES];

struct Rig {
    sys: LiveSystem,
    block: usize,
    master: Vec<StereoSample>,
    lanes: [Vec<StereoSample>; NUM_LANES],
    /// Service the renderer after each block.
    render: bool,
}

fn reference() -> Gens {
    [Role::Drums, Role::Bass, Role::Lead, Role::Extra].map(|r| Some(Box::new(FixedPattern::reference(r)) as Box<dyn LaneGenerator>))
}

fn silent() -> Gens {
    std::array::from_fn(|_| Some(Box::new(FixedPattern::default()) as Box<dyn LaneGenerator>))
}

fn kicks(ticks: &[u16]) -> Box<dyn LaneGenerator> {
    Box::new(FixedPattern::new(
        ticks
            .iter()
            .map(|&tick| LiveEvent { tick, kind: EventKind::Trigger, target: DRUM_TARGET_BACKBONE, vel: 120, ..Default::default() })
            .collect(),
    ))
}

/// A stab on every 16th: close to a continuous signal.
fn dense_lead() -> Box<dyn LaneGenerator> {
    Box::new(FixedPattern::from_notes(
        (0..STEPS_PER_BAR as u16).map(|s| DegreeNote::new(s * TICKS_PER_STEP, 0, 4, 20, 110)).collect(),
    ))
}

fn rig(gens: Gens, block: usize) -> Rig {
    let style = Style::load(&fixture_dir()).unwrap_or_else(|e| panic!("{e}"));
    let mut opts = LoadOptions::new(SR);
    opts.max_block = 1024;
    opts.generators = gens;
    let sys = build(load_style(&style, opts).unwrap_or_else(|e| panic!("{e}")));
    Rig { sys, block, master: Vec::new(), lanes: std::array::from_fn(|_| Vec::new()), render: true }
}

fn bar_frames(bpm: f64) -> f64 {
    SR as f64 * 240.0 / bpm
}

impl Rig {
    fn run(&mut self, frames: usize) {
        let mut left = frames;
        while left > 0 {
            let n = left.min(self.block);
            let mut master = StereoBuffer::silence(n);
            let mut tap: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| StereoBuffer::silence(n));
            self.sys.engine.process(&mut master, Some(&mut tap));
            self.master.extend_from_slice(master.as_slice());
            for (dst, src) in self.lanes.iter_mut().zip(&tap) {
                dst.extend_from_slice(src.as_slice());
            }
            if self.render {
                self.sys.renderer.service();
            }
            left -= n;
        }
    }

    /// Run until `bars` bars (at the default tempo) have been rendered in total.
    fn run_to(&mut self, bars: f64) {
        let target = (bars * bar_frames(BPM)).round() as usize;
        // Whole blocks only, so two rigs fed the same calls stay block-aligned.
        let frames = target.saturating_sub(self.master.len()).div_ceil(self.block) * self.block;
        self.run(frames);
    }

    fn send(&mut self, cmd: LiveCommand) {
        self.sys.controller.send(cmd);
        if self.render {
            self.sys.renderer.service();
        }
    }

    fn peak(&self, lane: usize, from: usize, to: usize) -> f32 {
        self.lanes[lane][from..to.min(self.lanes[lane].len())].iter().fold(0.0, |p, s| p.max(s.left.abs()).max(s.right.abs()))
    }
}

/// First frames where the signal rises above [`QUIET`] after `gap` quiet frames.
fn onsets(signal: &[StereoSample], gap: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut quiet = gap;
    for (i, s) in signal.iter().enumerate() {
        if s.left.abs().max(s.right.abs()) > QUIET {
            if quiet >= gap {
                out.push(i);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    out
}

fn first_difference(a: &[StereoSample], b: &[StereoSample]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

#[test]
fn renders_fixture_style_to_finite_non_silent_audio() {
    let mut r = rig(reference(), 256);
    r.run_to(4.0);
    assert!(r.sys.warnings.iter().any(|w| w.contains("surge")), "{:?}", r.sys.warnings);
    assert!(r.master.iter().all(|s| s.left.is_finite() && s.right.is_finite()));
    let peak = r.master.iter().fold(0.0f32, |p, s| p.max(s.left.abs()).max(s.right.abs()));
    assert!(peak > 0.05 && peak <= 1.0, "master peak {peak}");
    for lane in 0..NUM_LANES {
        assert!(r.peak(lane, 0, usize::MAX) > 0.01, "lane {lane} is silent");
    }
    // UI state follows.
    let shared = &r.sys.shared;
    let clock = shared.clock();
    assert!(clock.bar >= 3 && clock.bpm == BPM && !clock.ramping, "{clock:?}");
    assert_eq!(shared.key(), MusicalKey::new(9, true));
    assert_eq!(shared.starved_bars(), 0);
    let drums = shared.lane(0).view();
    assert!(drums.on && drums.fired.count > 30, "{drums:?}");
    assert!(matches!(drums.fired.tag, SpanTag::Backbone | SpanTag::Hats));
    assert!(matches!(shared.lane(1).view().fired.tag, SpanTag::Tie | SpanTag::Accent | SpanTag::Note));
    assert_eq!(drums.seeds, [DEFAULT_SEED; SLOTS_PER_CHAIN]);
    assert!(shared.lane(0).take_peak() > 0.01);
    let mut scope = [0.0; crate::live::scope::SCOPE_POINTS];
    shared.lane(0).scope().snapshot(&mut scope);
    assert!(shared.lane(0).scope().written() > 1000 && scope.iter().any(|v| v.abs() > 0.01));
}

#[test]
fn drum_onsets_are_sample_accurate_across_tempos_and_block_sizes() {
    let ticks = [0u16, 96, 192, 288];
    for bpm in [160.0, 172.0, 177.7] {
        let fpt = bar_frames(bpm) / TICKS_PER_BAR as f64;
        let mut reference: Option<Vec<usize>> = None;
        for block in [64usize, 256, 1000] {
            let mut gens = silent();
            gens[0] = Some(kicks(&ticks));
            let mut r = rig(gens, block);
            // A zero-bar request made before the first block applies on bar 0.
            r.send(LiveCommand::RequestTempo { bpm, bars: 0 });
            r.run((bar_frames(bpm) * 3.0) as usize);
            assert_eq!(r.sys.shared.clock().bpm, bpm);
            let found = onsets(&r.lanes[0], 2000);
            assert_eq!(found.len(), 12, "bpm {bpm} block {block}: {found:?}");
            for (i, &frame) in found.iter().enumerate() {
                let tick = (i / 4 * TICKS_PER_BAR as usize) as f64 + ticks[i % 4] as f64;
                // The voice starts on the event's frame; its first sample is at
                // the start of a 0.5 ms attack, so the threshold is crossed a few frames in.
                let late = frame as f64 - (tick * fpt).ceil();
                assert!((0.0..=4.0).contains(&late), "bpm {bpm} block {block} hit {i}: {late} frames late");
            }
            match &reference {
                None => reference = Some(found),
                Some(first) => assert_eq!(&found, first, "bpm {bpm}: block size {block} moved an onset"),
            }
        }
    }
}

#[test]
fn seed_and_generator_param_changes_land_exactly_on_the_bar() {
    let seeded = || {
        let mut gens = silent();
        gens[0] = Some(Box::new(FixedPattern::reference(Role::Drums).seeded()) as Box<dyn LaneGenerator>);
        gens
    };
    let bar2 = (2.0 * bar_frames(BPM)).ceil() as usize;

    let mut plain = rig(seeded(), 256);
    plain.run_to(4.0);

    // Seed of the slot that plays bar 2, changed halfway through bar 1.
    let mut changed = rig(seeded(), 256);
    changed.run_to(1.5);
    changed.send(LiveCommand::SetSlotSeed { lane: 0, slot: 2, seed: 9001 });
    changed.run_to(4.0);
    let diff = first_difference(&plain.lanes[0], &changed.lanes[0]).expect("a new seed changes the bar");
    assert!(diff >= bar2 && diff < bar2 + bar_frames(BPM) as usize, "first difference at {diff}, bar 2 starts at {bar2}");
    let bar3 = (3.0 * bar_frames(BPM)).ceil() as usize + 4800;
    assert_eq!(plain.lanes[0][bar3..], changed.lanes[0][bar3..], "bar 3 uses its own slot's seed");
    assert_eq!(changed.sys.shared.lane(0).view().seeds[2], 9001);

    // The drum lane's second control is mapped to the generator's density.
    let mut thinned = rig(seeded(), 256);
    thinned.run_to(1.5);
    thinned.send(LiveCommand::SetControl { lane: 0, control: 1, value: 0.0 });
    thinned.run_to(4.0);
    let diff = first_difference(&plain.lanes[0], &thinned.lanes[0]).expect("density changes the bar");
    assert!(diff >= bar2, "generator parameter changed the playing bar (frame {diff})");
    assert_eq!(thinned.sys.shared.lane(0).view().controls[1], 0.0);
}

#[test]
fn key_commit_lands_on_the_phrase_and_changes_pitches() {
    let bass_only = || {
        let mut gens = silent();
        gens[1] = Some(Box::new(FixedPattern::reference(Role::Bass)) as Box<dyn LaneGenerator>);
        gens
    };
    let mut plain = rig(bass_only(), 256);
    plain.run_to(6.0);

    let mut moved = rig(bass_only(), 256);
    moved.run_to(1.25);
    // Slider up: the gentlest upward move from A minor.
    moved.send(LiveCommand::SetKeyIntent(0.2));
    let target = moved.sys.shared.intent_key().expect("the slider left the dead zone");
    assert_ne!(target, MusicalKey::new(9, true));
    assert_eq!(moved.sys.shared.pending_key(), None, "intent alone arms nothing");
    moved.send(LiveCommand::CommitKey);
    assert_eq!(moved.sys.shared.pending_key(), Some(target));

    let phrase = (4.0 * bar_frames(BPM)).ceil() as usize;
    moved.run_to(3.9);
    assert_eq!(moved.sys.shared.key(), MusicalKey::new(9, true), "old key until the phrase ends");
    moved.run_to(6.0);
    assert_eq!(moved.sys.shared.key(), target);
    assert_eq!(moved.sys.shared.pending_key(), None);

    let diff = first_difference(&plain.lanes[1], &moved.lanes[1]).expect("a new key changes the bass pitches");
    assert!(diff >= phrase && diff < phrase + 64, "first difference at {diff}, phrase starts at {phrase}");

    // Progressions land the same way.
    moved.send(LiveCommand::ProgressionDelta(1));
    assert_eq!(moved.sys.shared.pending_progression(), Some(1));
    moved.run_to(7.9);
    assert_eq!(moved.sys.shared.progression(), 0);
    moved.run_to(8.1);
    assert_eq!(moved.sys.shared.progression(), 1);
    assert_eq!(moved.sys.shared.chord_degree(), 0, "a new progression starts on its first chord");
}

#[test]
fn lane_toggle_is_quantized_by_default_and_immediate_without_quant() {
    let lead = || {
        let mut gens = silent();
        gens[2] = Some(dense_lead());
        gens
    };
    let ms = |ms: f32| (ms / 1000.0 * SR as f32) as usize;
    let bar2 = (2.0 * bar_frames(BPM)).ceil() as usize;

    // Quantized: sounds until the bar, then cuts.
    let mut r = rig(lead(), 128);
    r.run_to(1.4);
    let pressed = r.master.len();
    r.send(LiveCommand::ToggleLane { lane: 2 });
    r.run_to(3.0);
    assert!(r.peak(2, pressed + ms(50.0), bar2) > 0.01, "lane kept playing until the bar");
    assert!(r.peak(2, bar2 - ms(5.0), bar2) > QUIET);
    let cut = bar2 + ms(QUANTIZED_FADE_SECS * 1000.0) + 64;
    assert!(r.peak(2, cut, usize::MAX) < QUIET, "lane silent after the bar");
    let view = r.sys.shared.lane(2).view();
    assert!(!view.on && !view.on_target);

    // Back on: starts with the next bar's first event.
    let on_at = r.master.len();
    r.send(LiveCommand::SetLaneOn { lane: 2, on: true });
    r.run_to(4.5);
    let bar4 = (4.0 * bar_frames(BPM)).ceil() as usize;
    let first = onsets(&r.lanes[2][on_at..], 1000)[0] + on_at;
    assert!(first >= bar4 && first <= bar4 + 4, "re-entry at {first}, bar 4 starts at {bar4}");

    // Immediate: fades right away.
    let mut r = rig(lead(), 128);
    r.send(LiveCommand::SetQuantize(false));
    assert!(!r.sys.shared.quantize());
    r.run_to(1.4);
    let pressed = r.master.len();
    r.send(LiveCommand::ToggleLane { lane: 2 });
    r.run_to(1.9);
    let faded = pressed + ms(IMMEDIATE_FADE_SECS * 1000.0) + 64;
    assert!(r.peak(2, pressed, pressed + ms(5.0)) > QUIET, "fade, not a hard cut");
    assert!(r.peak(2, faded, usize::MAX) < QUIET, "silent well before the bar");
}

#[test]
fn momentary_moves_act_while_held() {
    let ms = |ms: f32| (ms / 1000.0 * SR as f32) as usize;
    let mut plain = rig(reference(), 128);
    plain.run_to(4.0);

    // Fill: held over the last beat of bar 1, where the reference fill rolls the snare.
    let mut r = rig(reference(), 128);
    r.run_to(1.7);
    let pressed = r.master.len();
    r.send(LiveCommand::MovePress { lane: 0, kind: MoveKind::Fill });
    r.run_to(2.0);
    assert_eq!(r.sys.shared.lane(0).view().moves, move_bits::FILL);
    assert!(r.sys.shared.lane(0).view().fired.fill);
    r.send(LiveCommand::MoveRelease { lane: 0, kind: MoveKind::Fill });
    r.run_to(4.0);
    assert_eq!(r.sys.shared.lane(0).view().moves, 0);
    let diff = first_difference(&plain.lanes[0], &r.lanes[0]).expect("the fill bar differs");
    assert!(diff >= pressed, "fill changed audio before the press");
    assert_eq!(plain.lanes[1], r.lanes[1], "other lanes are untouched");
    let bar3 = (3.0 * bar_frames(BPM)).ceil() as usize;
    assert_eq!(plain.lanes[0][bar3..], r.lanes[0][bar3..], "main bar is back after release");

    // Void: the lane goes silent while held and resumes after.
    let mut r = rig(reference(), 128);
    r.run_to(1.2);
    let pressed = r.master.len();
    r.send(LiveCommand::MovePress { lane: 0, kind: MoveKind::Void });
    r.run_to(1.7);
    let released = r.master.len();
    assert!(r.peak(0, pressed + ms(VOID_FADE_SECS * 1000.0) + 64, released) < QUIET);
    assert!(r.peak(1, pressed, released) > 0.01, "void is per lane");
    r.send(LiveCommand::MoveRelease { lane: 0, kind: MoveKind::Void });
    r.run_to(2.5);
    assert!(r.peak(0, released, usize::MAX) > 0.01);

    // Stutter and double keep the lane playing with more hits than the plain bar.
    for kind in [MoveKind::Stutter, MoveKind::Double] {
        let mut gens = silent();
        gens[0] = Some(kicks(&[0, 192]));
        let mut r = rig(gens, 128);
        r.send(LiveCommand::SetRollDivision { ticks: 96 });
        r.run_to(1.02);
        r.send(LiveCommand::MovePress { lane: 0, kind });
        r.run_to(2.0);
        r.send(LiveCommand::MoveRelease { lane: 0, kind });
        r.run_to(2.9);
        let bar1 = bar_frames(BPM) as usize;
        let bar2 = (2.0 * bar_frames(BPM)) as usize;
        // Skip the bar-start kick, which is still ringing.
        let hits = onsets(&r.lanes[0][bar1 + 6000..bar2], 2000).len();
        // Stutter: the bar-start kick on every beat (3 more beats). Double: the bar twice (ticks 96, 192, 288).
        assert_eq!(hits, 3, "{kind:?}");
        assert_eq!(onsets(&r.lanes[0][bar2..], 2000).len(), 2, "{kind:?}: plain bar after release");
    }
}

#[test]
fn bar_starvation_repeats_the_playing_bar() {
    let mut gens = silent();
    gens[0] = Some(kicks(&[0, 96, 192, 288]));
    let mut r = rig(gens, 256);
    r.run_to(0.5);
    // The renderer stops: bars 1 and 2 never arrive.
    r.render = false;
    r.run_to(2.9);
    assert_eq!(r.sys.shared.starved_bars(), 1, "bar 1 was rendered during bar 0, bar 2 was not");
    let fpt = bar_frames(BPM) / TICKS_PER_BAR as f64;
    let found = onsets(&r.lanes[0], 2000);
    assert_eq!(found.len(), 12, "{found:?}");
    for (i, &frame) in found.iter().enumerate() {
        let late = frame as f64 - (i as f64 * 96.0 * fpt).ceil();
        assert!((0.0..=4.0).contains(&late), "hit {i}: {late}");
    }
    // The renderer comes back and the engine picks up the next bar.
    r.render = true;
    r.run_to(4.9);
    assert_eq!(r.sys.shared.starved_bars(), 1, "no further bar was missed");
    assert_eq!(onsets(&r.lanes[0], 2000).len(), 20);
}

#[test]
fn same_seeds_and_commands_give_identical_audio() {
    let seeded = || -> Gens {
        [Role::Drums, Role::Bass, Role::Lead, Role::Extra]
            .map(|r| Some(Box::new(FixedPattern::reference(r).seeded()) as Box<dyn LaneGenerator>))
    };
    let play = |seed: u32| {
        let mut r = rig(seeded(), 200);
        for lane in 0..NUM_LANES as u8 {
            for slot in 0..SLOTS_PER_CHAIN as u8 {
                r.send(LiveCommand::SetSlotSeed { lane, slot, seed: seed + lane as u32 * 7 + slot as u32 });
            }
        }
        r.run_to(1.3);
        r.send(LiveCommand::SetControl { lane: 0, control: 2, value: 0.6 });
        r.send(LiveCommand::RequestTempo { bpm: 165.0, bars: 1 });
        r.send(LiveCommand::MovePress { lane: 2, kind: MoveKind::Double });
        r.run_to(2.6);
        r.send(LiveCommand::MoveRelease { lane: 2, kind: MoveKind::Double });
        r.send(LiveCommand::ToggleHalfTime);
        r.run_to(5.0);
        r.master
    };
    let a = play(1234);
    assert!(a.iter().any(|s| s.left.abs() > 0.01));
    assert!(a == play(1234), "same inputs, different audio");
    assert!(a != play(4321), "different seeds, same audio");
}

#[cfg(debug_assertions)]
#[test]
fn rt_path_does_not_allocate() {
    use crate::audio::RtEngine;
    use crate::live::test_support::count_allocs;

    let mut r = rig(reference(), 256);
    r.run_to(1.0);
    let (rec_tx, _rec_rx) = rtrb::RingBuffer::<StereoSample>::new(1 << 16);
    let mut master = StereoBuffer::silence(256);
    let mut cue = StereoBuffer::silence(256);
    let mut total = 0;
    let blocks = (bar_frames(BPM) * 6.0) as usize / 256;
    let mut recorder = Some(rec_tx);
    for b in 0..blocks {
        // Commands are queued off the audio thread, like the UI does.
        match b {
            10 => r.sys.controller.send(LiveCommand::SetControl { lane: 0, control: 2, value: 0.4 }),
            20 => r.sys.controller.send(LiveCommand::SetControl { lane: 1, control: 3, value: 0.9 }),
            30 => r.sys.controller.send(LiveCommand::SetOption { lane: 2, option: 1 }),
            40 => r.sys.controller.send(LiveCommand::ToggleLane { lane: 3 }),
            50 => r.sys.controller.send(LiveCommand::MovePress { lane: 0, kind: MoveKind::Stutter }),
            60 => r.sys.controller.send(LiveCommand::MoveRelease { lane: 0, kind: MoveKind::Stutter }),
            70 => r.sys.controller.send(LiveCommand::MovePress { lane: 1, kind: MoveKind::Void }),
            80 => r.sys.controller.send(LiveCommand::MoveRelease { lane: 1, kind: MoveKind::Void }),
            90 => r.sys.controller.send(LiveCommand::RequestTempo { bpm: 176.0, bars: 1 }),
            100 => r.sys.controller.send(LiveCommand::StartRecording(recorder.take().unwrap())),
            110 => r.sys.controller.send(LiveCommand::SetKey(MusicalKey::new(0, true))),
            120 => r.sys.controller.send(LiveCommand::ToggleLane { lane: 3 }),
            200 => r.sys.controller.send(LiveCommand::StopRecording),
            _ => {}
        }
        let (_, allocs) = count_allocs(|| r.sys.engine.process_block(&mut master, &mut cue));
        total += allocs;
        if b == 150 {
            assert_eq!(r.sys.engine.recording_producers().len(), 1);
        }
        r.sys.renderer.service();
    }
    assert_eq!(total, 0, "the audio path allocated");
    assert!(r.sys.engine.recording_producers().is_empty());
    assert_eq!(cue.as_slice(), master.as_slice(), "cue is a copy of the master");
    assert_eq!(r.sys.shared.lane(2).view().option, 1);
    assert_eq!(r.sys.controller.dropped_messages(), 0);
}

#[test]
fn option_swap_lands_on_the_bar() {
    let lead = || {
        let mut gens = silent();
        gens[2] = Some(dense_lead());
        gens
    };
    let mut plain = rig(lead(), 256);
    plain.run_to(3.0);

    let mut r = rig(lead(), 256);
    r.run_to(1.5);
    // Browse while "holding" the lane button, load on release.
    r.send(LiveCommand::BrowseOption { lane: 2, delta: 1 });
    assert_eq!(r.sys.controller.lane(2).browse, Some(1));
    assert_eq!(r.sys.shared.lane(2).view().option_pending, Some(1));
    r.run_to(1.6);
    r.send(LiveCommand::CommitOption { lane: 2 });
    r.run_to(1.9);
    assert_eq!(r.sys.shared.lane(2).view().option, 0, "still the old instrument before the bar");
    r.run_to(3.0);
    let view = r.sys.shared.lane(2).view();
    assert_eq!((view.option, view.option_pending), (1, None));
    let bar2 = (2.0 * bar_frames(BPM)).ceil() as usize;
    let diff = first_difference(&plain.lanes[2], &r.lanes[2]).expect("the other option sounds different");
    assert!(diff >= bar2 && diff < bar2 + 64, "first difference at {diff}, bar 2 starts at {bar2}");

    // Options that failed to load (the CLAP bass) are skipped.
    r.send(LiveCommand::BrowseOption { lane: 1, delta: 1 });
    assert_eq!(r.sys.controller.lane(1).browse, Some(0), "wraps past the unavailable option");
    r.send(LiveCommand::SetOption { lane: 1, option: 1 });
    assert_eq!(r.sys.controller.lane(1).option, 0);
}

#[test]
fn controls_route_to_fx_sends_instrument_and_glue() {
    let mut plain = rig(reference(), 256);
    plain.run_to(3.0);
    let from = (1.5 * bar_frames(BPM)) as usize + 4096;

    // FX: the drum cutoff control closes the lane's filter.
    let mut r = rig(reference(), 256);
    r.run_to(1.5);
    r.send(LiveCommand::SetControl { lane: 0, control: 2, value: 0.0 });
    r.run_to(3.0);
    assert!(plain.lanes[0][..from - 4096] == r.lanes[0][..from - 4096]);
    assert!(r.peak(0, from, usize::MAX) < plain.peak(0, from, usize::MAX) * 0.9, "low-pass removes the hats' level");

    // Send: more reverb on the drums changes the master, not the lane.
    let mut r = rig(reference(), 256);
    r.run_to(1.5);
    r.send(LiveCommand::SetControl { lane: 0, control: 3, value: 1.0 });
    r.run_to(3.0);
    assert!(plain.lanes[0] == r.lanes[0]);
    assert!(plain.master != r.master);

    // Instrument: the lead cutoff control darkens the sampler.
    let mut r = rig(reference(), 256);
    r.run_to(1.5);
    r.send(LiveCommand::SetControl { lane: 2, control: 2, value: 0.1 });
    r.run_to(3.0);
    assert!(r.peak(2, from, usize::MAX) < plain.peak(2, from, usize::MAX) * 0.5);

    // Glue: the bass duck depth control changes the bass lane only.
    let mut r = rig(reference(), 256);
    r.run_to(1.5);
    r.send(LiveCommand::SetControl { lane: 1, control: 3, value: 1.0 });
    r.run_to(3.0);
    assert!(plain.lanes[1] != r.lanes[1]);
    assert!(plain.lanes[0] == r.lanes[0]);
    assert_eq!(r.sys.shared.lane(1).view().controls[3], 1.0);
}

#[test]
fn tempo_ramp_and_half_time_land_on_the_bar() {
    let mut r = rig(reference(), 256);
    r.run_to(0.5);
    r.send(LiveCommand::RequestTempo { bpm: 500.0, bars: 1 });
    r.send(LiveCommand::ToggleHalfTime);
    assert!(r.sys.shared.half_time_target());
    r.run_to(0.6);
    let clock = r.sys.shared.clock();
    assert_eq!((clock.bpm, clock.target_bpm, clock.ramping), (BPM, 178.0, true), "clamped to the style's range");
    assert!(!r.sys.shared.half_time());
    r.run_to(0.9);
    assert_eq!(r.sys.shared.clock().bpm, BPM, "ramp waits for the bar");
    r.run_to(1.5);
    let mid = r.sys.shared.clock();
    assert!(mid.bpm > BPM && mid.bpm < 178.0 && mid.ramping, "{mid:?}");
    assert!(r.sys.shared.half_time(), "half-time flag arrives with bar 1");
    r.run_to(2.5);
    let end = r.sys.shared.clock();
    assert_eq!((end.bpm, end.ramping), (178.0, false));
}

#[test]
fn capture_round_trip_restores_state() {
    let tmp = tempfile::tempdir().unwrap();
    let mut r = rig(reference(), 256);
    r.sys.controller.set_capture_collection(Some(tmp.path().to_path_buf()));

    // A state worth capturing.
    r.send(LiveCommand::SelectSlot { lane: 1, slot: 2 });
    r.send(LiveCommand::SeedDelta { lane: 1, delta: 5 });
    r.send(LiveCommand::SetSlotSeed { lane: 0, slot: 3, seed: 777 });
    r.send(LiveCommand::SetControl { lane: 2, control: 3, value: 0.8 });
    r.send(LiveCommand::SetOption { lane: 2, option: 1 });
    r.send(LiveCommand::SetLaneOn { lane: 3, on: false });
    r.send(LiveCommand::SetKey(MusicalKey::new(2, true)));
    r.send(LiveCommand::SelectProgression(1));
    r.send(LiveCommand::RequestTempo { bpm: 166.0, bars: 0 });
    r.send(LiveCommand::SetHalfTime(true));
    r.run_to(4.5);
    assert_eq!(r.sys.shared.key(), MusicalKey::new(2, true));

    let captured = r.sys.controller.capture(CaptureScope::All);
    assert_eq!(captured.style, "fixture-breaks");
    assert_eq!(captured.global.key, MusicalKey::new(2, true));
    assert_eq!((captured.global.progression, captured.global.tempo, captured.global.half_time), (1, 166.0, true));
    assert_eq!(captured.lanes[1].seeds, [DEFAULT_SEED + 1, DEFAULT_SEED + 1, DEFAULT_SEED + 6, DEFAULT_SEED + 1]);
    assert_eq!(captured.lanes[0].seeds[3], 777);
    assert_eq!((captured.lanes[2].controls[3], captured.lanes[2].option), (0.8, 1));
    assert!(!captured.lanes[3].on && captured.lanes[0].on);

    // Saved off-thread under <collection>/captures/<style>/.
    let saved = (0..200).find_map(|_| {
        std::thread::sleep(std::time::Duration::from_millis(10));
        crate::live::capture::list_captures(tmp.path(), "fixture-breaks").ok()?.pop()
    });
    let loaded = Capture::load(&saved.expect("capture file written")).unwrap();
    assert_eq!(loaded, captured);

    // Wreck the state, then recall.
    for lane in 0..NUM_LANES as u8 {
        for slot in 0..SLOTS_PER_CHAIN as u8 {
            r.send(LiveCommand::SetSlotSeed { lane, slot, seed: 1 });
        }
        for control in 0..CONTROLS_PER_LANE as u8 {
            r.send(LiveCommand::SetControl { lane, control, value: 0.0 });
        }
        r.send(LiveCommand::SetLaneOn { lane, on: lane == 3 });
    }
    r.send(LiveCommand::SetOption { lane: 2, option: 0 });
    r.send(LiveCommand::SetKey(MusicalKey::new(9, true)));
    r.send(LiveCommand::SelectProgression(0));
    r.send(LiveCommand::RequestTempo { bpm: 176.0, bars: 0 });
    r.send(LiveCommand::SetHalfTime(false));
    r.run_to(9.0);
    assert_eq!(r.sys.shared.key(), MusicalKey::new(9, true));
    assert_eq!(r.sys.shared.lane(2).view().option, 0);

    r.send(LiveCommand::RecallCapture(Box::new(loaded)));
    r.run_to(18.0);
    let again = r.sys.controller.capture(CaptureScope::All);
    assert_eq!(again.global, captured.global);
    assert_eq!(again.lanes, captured.lanes);
    // The engine followed, not just the controller.
    let shared = &r.sys.shared;
    assert_eq!(shared.key(), MusicalKey::new(2, true));
    assert_eq!(shared.progression(), 1);
    assert_eq!(shared.clock().bpm, 166.0);
    assert!(shared.half_time());
    assert_eq!(shared.lane(2).view().option, 1);
    assert!(!shared.lane(3).view().on && shared.lane(0).view().on);
    assert_eq!(shared.lane(0).view().seeds[3], 777);
    assert_eq!(shared.lane(2).view().controls[3], 0.8);

    // A lane capture restores only its lane.
    let lane_capture = r.sys.controller.capture(CaptureScope::Lane { lane: 1 });
    r.send(LiveCommand::SetSlotSeed { lane: 1, slot: 0, seed: 4 });
    r.send(LiveCommand::SetSlotSeed { lane: 0, slot: 0, seed: 4 });
    r.sys.controller.recall(&lane_capture);
    assert_eq!(r.sys.controller.lane(1).seeds[0], DEFAULT_SEED + 1);
    assert_eq!(r.sys.controller.lane(0).seeds[0], 4);
    assert_eq!(r.sys.controller.captures().len(), 3);
}

#[test]
fn threaded_start_plays_without_starving() {
    let style = Style::load(&fixture_dir()).unwrap();
    let mut opts = LoadOptions::new(SR);
    opts.generators = reference();
    let (mut engine, mut handles) = LiveEngine::start(&style, opts).unwrap();
    handles.controller.send(LiveCommand::SetSlotSeed { lane: 0, slot: 1, seed: 3 });
    let mut master = StereoBuffer::silence(256);
    let mut peak = 0.0f32;
    // Three bars at roughly real time, so the renderer thread keeps up.
    for _ in 0..(bar_frames(BPM) * 3.0) as usize / 256 {
        engine.process(&mut master, None);
        peak = peak.max(master.peak());
        std::thread::sleep(std::time::Duration::from_micros(2000));
    }
    assert!(peak > 0.05);
    assert_eq!(handles.shared.starved_bars(), 0);
    assert!(handles.shared.clock().bar >= 2);
}
