//! Loudness calibration of a live style (design §5.5, D20).
//!
//! Renders every instrument option through a hand-written reference pattern,
//! measures integrated LUFS (BS.1770) and writes the trim that brings the
//! option to its role's target into `style.yaml` (`lufs_trim_db`).
//!
//! Two passes:
//!
//! 1. **Input trim** (`lufs_trim_db`): the instrument output, no lane FX, no
//!    glue, so every option reaches its lane's processing at the same level
//!    and compressors behave the same for every option. The engine applies
//!    the trim at the same point (see `live::loader`).
//! 2. **Output trim** (`output_trims_db` per lane option / drum kit): the
//!    lane after its whole processing (break bus, lane FX, glue high-pass,
//!    mono-below, compressor and saturation; no duck, no buses), playing the
//!    lane's own generator with its default parameters. The trim brings the
//!    processed lane to its role's target; the glue applies it right after
//!    the dynamics, so it does not change how they work.
//!
//! Drum sub-voice options are measured one at a time, each playing its own
//! targets on 8th notes. Their targets are the drum role's target plus a
//! per-sub-voice offset, because the sub-voices sum on the drum lane.
//!
//! Writing goes through the style schema, so comments and hand formatting in
//! `style.yaml` are lost; the previous file is kept as `style.yaml.bak`.
//! Use `--dry-run` to only print the table.
//!
//! Usage: cargo run --release -p mesh-core --bin style-calibrate -- --style DIR [OPTIONS]
//!
//!   --style DIR     style folder (with style.yaml)
//!   --dry-run       print measurements, do not write
//!   --bars N        bars of the reference pattern to measure (default 4)
//!   --clap          scan and measure CLAP instrument options too
//!   --rate HZ       sample rate (default 48000)

use std::path::PathBuf;

use mesh_core::clap::TransportInfo;
use mesh_core::live::clock::LiveClock;
use mesh_core::live::event::{BarEvents, EventKind, LiveEvent};
use mesh_core::live::generator::fixed::FixedPattern;
use mesh_core::live::generator::layout::{BACKBONE_BASE, BREAK_BASE, HATS_BASE, PERC_BASE};
use mesh_core::live::generator::{BarCtx, DrumLayout, LaneGenerator, MAX_GEN_PARAMS};
use mesh_core::live::glue::{measure_lufs, trim_db, GlueChain, GlueKey, LaneSetup, Role};
use mesh_core::live::harmony::HarmonyCtx;
use mesh_core::live::lane::Lane;
use mesh_core::live::loader::{load_style, ClapMode, LoadOptions};
use mesh_core::live::player::EventSink;
use mesh_core::live::style::{Style, SubVoiceKind, SubVoiceSpec, STYLE_FILE, TRIM_RANGE_DB};
use mesh_core::live::{NUM_LANES, STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};
use mesh_core::types::{StereoBuffer, StereoSample};

const USAGE: &str = "usage: style-calibrate --style DIR [--dry-run] [--bars N] [--clap] [--rate HZ]";
const BLOCK: usize = 256;
/// Bars of the lane's own generator measured for the output trim, and its seed.
const OUTPUT_BARS: usize = 8;
const OUTPUT_SEED: u32 = 1;

// MUSICAL: target of a drum sub-voice relative to the drum role's target.
// Hats and percussion sit under the backbone; to be ear-tested (P1).
fn sub_voice_offset_db(kind: SubVoiceKind) -> f64 {
    match kind {
        SubVoiceKind::Backbone | SubVoiceKind::Break => 0.0,
        SubVoiceKind::Hats => -6.0,
        SubVoiceKind::Perc => -8.0,
    }
}

fn fail(msg: impl AsRef<str>) -> ! {
    eprintln!("style-calibrate: {}\n{USAGE}", msg.as_ref());
    std::process::exit(2);
}

/// Play `bars[i]` in bar `i` on `lane`, plus a one-bar tail, and return the audio.
fn render(lane: &mut Lane, bars: &[BarEvents], rate: u32, bpm: f64) -> Vec<StereoSample> {
    let n = bars.len() as u64;
    let mut clock = LiveClock::new(rate, bpm);
    let mut out = Vec::new();
    let mut buf = StereoBuffer::silence(BLOCK);
    while clock.position().bar <= n {
        let pos = clock.position();
        let transport = TransportInfo::new(bpm, true, pos.bar as f64 * 4.0 + pos.tick * 4.0 / TICKS_PER_BAR as f64);
        lane.begin_chunk();
        let mut samples_per_tick = clock.ticks_to_frames(1.0);
        for seg in clock.segments(BLOCK as u32) {
            samples_per_tick = seg.frames_per_tick();
            lane.begin_segment(&seg);
            if seg.bar < n {
                for ev in bars[seg.bar as usize].as_slice() {
                    if let Some(offset) = seg.offset_for_tick(ev.tick) {
                        lane.fire(ev, offset, false);
                    }
                }
            }
            lane.end_segment();
        }
        lane.render(&mut buf, samples_per_tick, &transport);
        out.extend_from_slice(buf.as_slice());
    }
    out
}

/// 8th-note hits cycling through `targets`.
fn drum_bar(targets: &[u8], choke: u8) -> BarEvents {
    let mut bar = BarEvents::default();
    if targets.is_empty() {
        return bar;
    }
    for (i, step) in (0..STEPS_PER_BAR as u16).step_by(2).enumerate() {
        bar.push(LiveEvent {
            tick: step * TICKS_PER_STEP,
            kind: EventKind::Trigger,
            target: targets[i % targets.len()],
            choke,
            vel: 110,
            ..Default::default()
        });
    }
    bar
}

/// Targets of one sub-voice in a kit layout.
fn sub_voice_targets(kind: SubVoiceKind, layout: &DrumLayout) -> (Vec<u8>, u8) {
    match kind {
        SubVoiceKind::Backbone => {
            ([layout.kick, layout.snare, layout.ghost].into_iter().flatten().map(|t| t.max(BACKBONE_BASE)).collect(), 0)
        }
        SubVoiceKind::Hats => {
            ([layout.hat_closed, layout.hat_open].into_iter().flatten().map(|t| t.max(HATS_BASE)).collect(), layout.hats_choke)
        }
        SubVoiceKind::Perc => ((0..layout.perc_count).map(|i| PERC_BASE + i).collect(), 0),
        SubVoiceKind::Break => {
            ((0..layout.slices().len()).map(|i| layout.slice_target(i).max(BREAK_BASE)).collect(), layout.break_choke)
        }
    }
}

struct Row {
    lane: String,
    option: String,
    measured: f64,
    target: f64,
    old: f32,
    new: Option<f32>,
}

fn main() {
    let mut style_dir: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut bars = 4u64;
    let mut clap = false;
    let mut rate = 48_000u32;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().unwrap_or_else(|| fail(format!("{name} needs a value")));
        match arg.as_str() {
            "--style" => style_dir = Some(PathBuf::from(value("--style"))),
            "--dry-run" => dry_run = true,
            "--bars" => bars = value("--bars").parse().unwrap_or_else(|_| fail("bad bar count")),
            "--clap" => clap = true,
            "--rate" => rate = value("--rate").parse().unwrap_or_else(|_| fail("bad sample rate")),
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => fail(format!("unknown argument '{other}'")),
        }
    }
    let style_dir = style_dir.unwrap_or_else(|| fail("--style is required"));
    let mut style = Style::load(&style_dir).unwrap_or_else(|e| fail(e.to_string()));

    let mut opts = LoadOptions::new(rate);
    opts.max_block = BLOCK;
    opts.calibration = true;
    if clap {
        opts.clap = ClapMode::Scan { extra_paths: Vec::new() };
    }
    let mut loaded = load_style(&style, opts).unwrap_or_else(|e| fail(e.to_string()));
    for w in &loaded.warnings {
        println!("warning: {w}");
    }
    let bpm = loaded.tempo.default;
    let harmony = HarmonyCtx { key: loaded.key, chord_degree: 0, progression: 0 };
    let targets = style.spec.loudness;
    let output_targets = style.spec.output_loudness.unwrap_or(targets);
    let mut rows: Vec<Row> = Vec::new();

    for li in 0..loaded.lanes.len() {
        let role = loaded.meta[li].role;
        let lane_name = loaded.meta[li].name.clone();
        let lane = &mut loaded.lanes[li];
        let measure = |lane: &mut Lane, option: usize, bar: &BarEvents| -> f64 {
            lane.queue_option(option);
            lane.begin_bar(0);
            // Let the previous option's voices die before measuring.
            render(lane, &[BarEvents::default()], rate, bpm);
            measure_lufs(&render(lane, &vec![bar.clone(); bars as usize], rate, bpm), rate as f64)
        };

        if role == Role::Drums {
            let subs: Vec<SubVoiceSpec> = style.spec.lanes[li].sub_voices.clone();
            for (si, sv) in subs.iter().enumerate() {
                // The loader maps only the first sub-voice of each kind.
                if subs[..si].iter().any(|s| s.voice == sv.voice) || sv.options.is_empty() {
                    continue;
                }
                let n = sv.options.len();
                for oi in 0..n {
                    // Kit k puts this sub-voice on option (default + k) % n.
                    let kit = (oi + n - sv.default_option % n) % n;
                    let layout = DrumLayout::from_sub_voices(&subs, |s| (s.default_option + kit) % s.options.len().max(1));
                    let (hits, choke) = sub_voice_targets(sv.voice, &layout);
                    let measured = measure(lane, kit, &drum_bar(&hits, choke));
                    let target = targets.target(role) as f64 + sub_voice_offset_db(sv.voice);
                    let opt = &mut style.spec.lanes[li].sub_voices[si].options[oi];
                    let new = measured.is_finite().then(|| (trim_db(measured, target) as f32).clamp(TRIM_RANGE_DB.0, TRIM_RANGE_DB.1));
                    rows.push(Row { lane: format!("{lane_name}/{:?}", sv.voice).to_lowercase(), option: opt.name.clone(), measured, target, old: opt.lufs_trim_db, new });
                    if let Some(t) = new {
                        opt.lufs_trim_db = t;
                    }
                }
            }
        } else {
            let pattern = FixedPattern::reference(role);
            let mut bar = BarEvents::default();
            let params = [0.5f32; MAX_GEN_PARAMS];
            pattern.render_bar(
                &BarCtx {
                    seed: 0,
                    lane: li as u8,
                    role,
                    bar_in_phrase: 0,
                    phrase: 0,
                    fill: false,
                    half_time: false,
                    params: &params,
                    harmony,
                    drum_mask: &[0; STEPS_PER_BAR],
                },
                &mut bar,
            );
            for oi in 0..style.spec.lanes[li].options.len() {
                let opt = &mut style.spec.lanes[li].options[oi];
                if !loaded.meta[li].option_available[oi] {
                    println!("skipped: {lane_name}/{} did not load", opt.name);
                    continue;
                }
                let measured = measure(lane, oi, &bar);
                let target = targets.target(role) as f64;
                let new = measured.is_finite().then(|| (trim_db(measured, target) as f32).clamp(TRIM_RANGE_DB.0, TRIM_RANGE_DB.1));
                rows.push(Row { lane: lane_name.clone(), option: opt.name.clone(), measured, target, old: opt.lufs_trim_db, new });
                if let Some(t) = new {
                    opt.lufs_trim_db = t;
                }
            }
        }
    }

    println!("{:<18} {:<24} {:>10} {:>8} {:>9} {:>9}", "lane", "option", "measured", "target", "old trim", "new trim");
    for r in &rows {
        let new = r.new.map(|t| format!("{t:+.1}")).unwrap_or_else(|| "silent".into());
        println!("{:<18} {:<24} {:>8.1} L {:>6.1} L {:>+8.1} {:>9}", r.lane, r.option, r.measured, r.target, r.old, new);
    }

    // Pass 2: the processed lane, with the new input trims and no output trims.
    for lane in &mut style.spec.lanes {
        lane.output_trims_db.clear();
    }
    let mut opts = LoadOptions::new(rate);
    opts.max_block = BLOCK;
    if clap {
        opts.clap = ClapMode::Scan { extra_paths: Vec::new() };
    }
    let mut loaded = load_style(&style, opts).unwrap_or_else(|e| fail(e.to_string()));
    let mut glue_params = loaded.glue;
    glue_params.duck.enabled = false;
    let mut out_rows: Vec<Row> = Vec::new();
    for li in 0..loaded.lanes.len() {
        let meta = &loaded.meta[li];
        let role = meta.role;
        let target = output_targets.target(role) as f64;
        let mut trims = Vec::with_capacity(meta.option_names.len());
        for oi in 0..meta.option_names.len() {
            if !meta.option_available[oi] {
                trims.push(0.0);
                continue;
            }
            let generator: &dyn LaneGenerator = meta
                .drum_kits
                .get(oi)
                .and_then(|k| k.generator.as_deref())
                .unwrap_or(meta.generator.as_ref());
            let bar_events: Vec<BarEvents> = (0..OUTPUT_BARS)
                .map(|b| {
                    let mut bar = BarEvents::default();
                    generator.render_bar(
                        &BarCtx {
                            seed: OUTPUT_SEED,
                            lane: li as u8,
                            role,
                            bar_in_phrase: (b % 4) as u8,
                            phrase: 0,
                            fill: false,
                            half_time: false,
                            params: &meta.gen_params,
                            harmony,
                            drum_mask: &[0; STEPS_PER_BAR],
                        },
                        &mut bar,
                    );
                    bar.sort();
                    bar
                })
                .collect();
            let lane = &mut loaded.lanes[li];
            lane.queue_option(oi);
            lane.begin_bar(0);
            render(lane, &[BarEvents::default()], rate, bpm);
            let raw = render(lane, &bar_events, rate, bpm);

            // The lane's glue stages, this lane alone.
            let mut glue = GlueChain::new(BLOCK);
            glue.set_params(&glue_params);
            glue.set_lane(li, LaneSetup { role, trim_db: 0.0, delay_send: 0.0, reverb_send: 0.0 });
            let mut processed = Vec::with_capacity(raw.len());
            let mut out = StereoBuffer::silence(BLOCK);
            for chunk in raw.chunks(BLOCK) {
                let mut lanes: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| StereoBuffer::silence(chunk.len()));
                lanes[li] = StereoBuffer::from_vec(chunk.to_vec());
                out.set_len_from_capacity(chunk.len());
                glue.process(&mut lanes, GlueKey::default(), bpm, &mut out);
                processed.extend_from_slice(lanes[li].as_slice());
            }
            let measured = measure_lufs(&processed, rate as f64);
            let new = measured.is_finite().then(|| (trim_db(measured, target) as f32).clamp(TRIM_RANGE_DB.0, TRIM_RANGE_DB.1));
            out_rows.push(Row { lane: meta.name.clone(), option: meta.option_names[oi].clone(), measured, target, old: 0.0, new });
            trims.push(new.unwrap_or(0.0));
        }
        style.spec.lanes[li].output_trims_db = trims;
    }
    println!("\nprocessed lane output (lane FX + glue dynamics):");
    println!("{:<18} {:<24} {:>10} {:>8} {:>12}", "lane", "option / kit", "measured", "target", "output trim");
    for r in &out_rows {
        let new = r.new.map(|t| format!("{t:+.1}")).unwrap_or_else(|| "silent".into());
        println!("{:<18} {:<24} {:>8.1} L {:>6.1} L {:>12}", r.lane, r.option, r.measured, r.target, new);
    }

    if dry_run {
        println!("dry run: {} not changed", style_dir.join(STYLE_FILE).display());
        return;
    }
    let path = style_dir.join(STYLE_FILE);
    let yaml = style.spec.to_yaml().unwrap_or_else(|e| fail(format!("cannot serialize the style: {e}")));
    let backup = style_dir.join(format!("{STYLE_FILE}.bak"));
    std::fs::copy(&path, &backup).unwrap_or_else(|e| fail(format!("cannot back up {}: {e}", path.display())));
    std::fs::write(&path, yaml).unwrap_or_else(|e| fail(format!("cannot write {}: {e}", path.display())));
    println!("wrote {} (previous file kept as {})", path.display(), backup.display());
}
