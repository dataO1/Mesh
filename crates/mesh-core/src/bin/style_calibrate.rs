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
//! **Analysis** (`--analyse`): measures every sample's loudness once and
//! stores it in the style (`lufs` on the sample), so the loader does not
//! measure the samples again at every load (calibration loads the style
//! twice). Samples that have a value are skipped unless `--remeasure`.
//!
//! Usage: cargo run --release -p mesh-core --bin style-calibrate -- --style DIR [OPTIONS]
//!
//!   --style DIR     style folder (with style.yaml)
//!   --analyse       only measure sample loudness and store it (no calibration)
//!   --remeasure     with --analyse: measure samples that already have a value too
//!   --lanes A,B     only these lanes (by name); the other lanes stay untouched
//!   --dry-run       print measurements, do not write
//!   --bars N        bars of the reference pattern to measure (default 4)
//!   --clap          scan and measure CLAP instrument options too
//!   --rate HZ       sample rate (default 48000)

use std::path::PathBuf;

use mesh_core::clap::TransportInfo;
use mesh_core::live::clock::LiveClock;
use mesh_core::live::event::{BarEvents, EventKind, LiveEvent};
use mesh_core::live::generator::fixed::FixedPattern;
use mesh_core::live::generator::layout::{
    BACKBONE_BASE, BREAK_BASE, HATS_BASE, PERC_BASE, RIDE_BASE, SHAKER_BASE, TOP_BASE,
};
use mesh_core::live::generator::{BarCtx, DrumLayout, LaneGenerator, MAX_GEN_PARAMS};
use mesh_core::live::glue::{measure_lufs, trim_db, GlueChain, GlueKey, LaneSetup, Role};
use mesh_core::live::harmony::HarmonyCtx;
use mesh_core::live::lane::Lane;
use mesh_core::live::loader::{analyse_sample_file, load_style, ClapMode, LoadOptions};
use mesh_core::live::player::EventSink;
use mesh_core::live::style::{SampleSpec, Style, SubVoiceKind, SubVoiceSpec, STYLE_FILE, TRIM_RANGE_DB};
use mesh_core::live::{NUM_LANES, STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};
use mesh_core::types::{StereoBuffer, StereoSample};

const USAGE: &str = "usage: style-calibrate --style DIR [--analyse [--remeasure]] [--lanes A,B] [--dry-run] [--bars N] [--clap] [--rate HZ]";
const BLOCK: usize = 256;
/// Bars of the lane's own generator measured for the output trim, and its seed.
const OUTPUT_BARS: usize = 8;
/// Lanes with at least this many options (drum kits) are measured on
/// worker threads, up to `MAX_WORKERS`.
const PARALLEL_MIN_OPTIONS: usize = 16;
const MAX_WORKERS: usize = 8;
const OUTPUT_SEED: u32 = 1;

// MUSICAL: target of a drum sub-voice relative to the drum role's target.
// Hats and percussion sit under the backbone; to be ear-tested (P1).
fn sub_voice_offset_db(kind: SubVoiceKind) -> f64 {
    match kind {
        // User: hats, break and perc 2 dB down (2026-10-15).
        SubVoiceKind::Backbone => 0.0,
        SubVoiceKind::Break => -2.0,
        SubVoiceKind::Hats => -8.0,
        SubVoiceKind::Ride | SubVoiceKind::Shaker => -6.0,
        SubVoiceKind::Perc => -10.0,
        SubVoiceKind::Top => -4.0,
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
        SubVoiceKind::Hats => (
            (0..32u8).filter(|i| (layout.hats_closed | layout.hats_open) & 1 << i != 0).map(|i| HATS_BASE + i).collect(),
            layout.hats_choke,
        ),
        SubVoiceKind::Ride => (
            (0..16u8).filter(|i| (layout.rides | layout.crashes) & 1 << i != 0).map(|i| RIDE_BASE + i).collect(),
            0,
        ),
        SubVoiceKind::Shaker => ((0..layout.shaker_count).map(|i| SHAKER_BASE + i).collect(), 0),
        SubVoiceKind::Perc => ((0..layout.perc_count).map(|i| PERC_BASE + i).collect(), 0),
        SubVoiceKind::Top => ((0..layout.top_count).map(|i| TOP_BASE + i).collect(), layout.top_choke),
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
    let mut analyse = false;
    let mut remeasure = false;
    let mut lane_filter: Option<Vec<String>> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().unwrap_or_else(|| fail(format!("{name} needs a value")));
        match arg.as_str() {
            "--style" => style_dir = Some(PathBuf::from(value("--style"))),
            "--dry-run" => dry_run = true,
            "--bars" => bars = value("--bars").parse().unwrap_or_else(|_| fail("bad bar count")),
            "--clap" => clap = true,
            "--analyse" => analyse = true,
            "--remeasure" => remeasure = true,
            "--lanes" => lane_filter = Some(value("--lanes").split(',').map(str::to_string).collect()),
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
    if let Some(names) = &lane_filter {
        for n in names {
            if !style.spec.lanes.iter().any(|l| &l.name == n) {
                let known: Vec<&str> = style.spec.lanes.iter().map(|l| l.name.as_str()).collect();
                fail(format!("no lane '{n}' (lanes: {})", known.join(", ")));
            }
        }
    }
    let selected = |name: &str| lane_filter.as_ref().is_none_or(|f| f.iter().any(|n| n == name));

    if analyse {
        analyse_samples(&mut style, &selected, rate, remeasure);
        save(&style, &style_dir, dry_run);
        return;
    }
    let unmeasured = sampler_specs(&mut style, &selected).filter(|s| s.lufs.is_none()).count();
    if unmeasured > 0 {
        println!("note: {unmeasured} samples have no stored loudness and are measured at every load (run --analyse)");
    }

    let mut opts = LoadOptions::new(rate);
    opts.max_block = BLOCK;
    opts.calibration = true;
    if clap {
        opts.clap = ClapMode::Scan { extra_paths: Vec::new() };
    }
    let t = std::time::Instant::now();
    let mut loaded = load_style(&style, opts).unwrap_or_else(|e| fail(e.to_string()));
    println!("timing: load 1 {:.1} s", t.elapsed().as_secs_f32());
    let t_pass1 = std::time::Instant::now();
    for w in &loaded.warnings {
        println!("warning: {w}");
    }
    let bpm = loaded.tempo.default;
    let harmony = HarmonyCtx { key: loaded.key, chord_degree: 0, progression: 0, mode: 0 };
    let targets = style.spec.loudness;
    let output_targets = style.spec.output_loudness.unwrap_or(targets);
    let mut rows: Vec<Row> = Vec::new();

    for li in 0..loaded.lanes.len() {
        let role = loaded.meta[li].role;
        let lane_name = loaded.meta[li].name.clone();
        if !selected(&lane_name) {
            continue;
        }
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
                    sound_seed: 0,
                    class: mesh_core::live::form::SeedClass::Main,
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
                    jitter: [0.0; mesh_core::live::generator::MAX_GEN_PARAMS],
                    bass_onsets: 0,
                    bass_phrase: [0; 4],
                    drum_fill: false,
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

    println!("timing: pass 1 {:.1} s", t_pass1.elapsed().as_secs_f32());
    // Pass 2: the processed lane, with the new input trims and no output trims.
    for lane in &mut style.spec.lanes {
        if selected(&lane.name) {
            lane.output_trims_db.clear();
        }
    }
    let mut opts = LoadOptions::new(rate);
    opts.max_block = BLOCK;
    if clap {
        opts.clap = ClapMode::Scan { extra_paths: Vec::new() };
    }
    let t = std::time::Instant::now();
    let mut loaded = load_style(&style, opts).unwrap_or_else(|e| fail(e.to_string()));
    println!("timing: load 2 {:.1} s", t.elapsed().as_secs_f32());
    let t_pass2 = std::time::Instant::now();
    let mut glue_params = loaded.glue;
    glue_params.duck.enabled = false;
    let ctx = OutputCtx { glue: glue_params, rate, bpm, harmony };
    let mut out_rows: Vec<Row> = Vec::new();
    for li in 0..loaded.lanes.len() {
        let (name, role, n_options) = {
            let meta = &loaded.meta[li];
            (meta.name.clone(), meta.role, meta.option_names.len())
        };
        if !selected(&name) {
            continue;
        }
        let target = output_targets.target(role) as f64;
        let t_lane = std::time::Instant::now();
        // Lanes with many options (drum kits) without CLAP instruments are
        // measured on worker threads, each with its own copy of the style.
        let parallel = n_options >= PARALLEL_MIN_OPTIONS && !style.spec.lanes[li].options.iter().any(|o| o.clap.is_some());
        let measured: Vec<Option<f64>> = if parallel {
            let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(MAX_WORKERS);
            let mut results = vec![None; n_options];
            std::thread::scope(|scope| {
                let handles: Vec<_> = (0..workers)
                    .map(|w| {
                        let style = &style;
                        let ctx = &ctx;
                        scope.spawn(move || {
                            let mut opts = LoadOptions::new(ctx.rate);
                            opts.max_block = BLOCK;
                            let mut mine = load_style(style, opts).unwrap_or_else(|e| fail(e.to_string()));
                            (w..n_options).step_by(workers).map(|oi| (oi, output_lufs(&mut mine, li, oi, ctx))).collect::<Vec<_>>()
                        })
                    })
                    .collect();
                for h in handles {
                    for (oi, m) in h.join().unwrap_or_else(|_| fail("a calibration worker panicked")) {
                        results[oi] = m;
                    }
                }
            });
            results
        } else {
            (0..n_options).map(|oi| output_lufs(&mut loaded, li, oi, &ctx)).collect()
        };
        let meta = &loaded.meta[li];
        let mut trims = Vec::with_capacity(n_options);
        for (oi, m) in measured.into_iter().enumerate() {
            let Some(measured) = m else {
                trims.push(0.0);
                continue;
            };
            let new = measured.is_finite().then(|| (trim_db(measured, target) as f32).clamp(TRIM_RANGE_DB.0, TRIM_RANGE_DB.1));
            out_rows.push(Row { lane: meta.name.clone(), option: meta.option_names[oi].clone(), measured, target, old: 0.0, new });
            trims.push(new.unwrap_or(0.0));
        }
        style.spec.lanes[li].output_trims_db = trims;
        println!("lane-time: {} {:.1} s ({} options{})", name, t_lane.elapsed().as_secs_f32(), n_options, if parallel { ", parallel" } else { "" });
    }
    println!("timing: pass 2 {:.1} s", t_pass2.elapsed().as_secs_f32());
    println!("\nprocessed lane output (lane FX + glue dynamics):");
    println!("{:<18} {:<24} {:>10} {:>8} {:>12}", "lane", "option / kit", "measured", "target", "output trim");
    for r in &out_rows {
        let new = r.new.map(|t| format!("{t:+.1}")).unwrap_or_else(|| "silent".into());
        println!("{:<18} {:<24} {:>8.1} L {:>6.1} L {:>12}", r.lane, r.option, r.measured, r.target, new);
    }

    save(&style, &style_dir, dry_run);
}

/// Write `style.yaml` (previous file kept as `style.yaml.bak`), or only say so on a dry run.
/// What the processed-output measurement of an option needs.
struct OutputCtx {
    glue: mesh_core::live::glue::GlueParams,
    rate: u32,
    bpm: f64,
    harmony: HarmonyCtx,
}

/// Loudness of option `oi` of lane `li` through the lane FX and its glue
/// stages, playing the lane's own generator for [`OUTPUT_BARS`] bars.
/// `None` when the option did not load.
fn output_lufs(loaded: &mut mesh_core::live::loader::LoadedStyle, li: usize, oi: usize, ctx: &OutputCtx) -> Option<f64> {
    let meta = &loaded.meta[li];
    if !meta.option_available[oi] {
        return None;
    }
    let role = meta.role;
    let generator: &dyn LaneGenerator =
        meta.drum_kits.get(oi).and_then(|k| k.generator.as_deref()).unwrap_or(meta.generator.as_ref());
    let bar_events: Vec<BarEvents> = (0..OUTPUT_BARS)
        .map(|b| {
            let mut bar = BarEvents::default();
            generator.render_bar(
                &BarCtx {
                    sound_seed: OUTPUT_SEED,
                    class: mesh_core::live::form::SeedClass::Main,
                    seed: OUTPUT_SEED,
                    lane: li as u8,
                    role,
                    bar_in_phrase: (b % 4) as u8,
                    phrase: 0,
                    fill: false,
                    half_time: false,
                    params: &meta.gen_params,
                    harmony: ctx.harmony,
                    drum_mask: &[0; STEPS_PER_BAR],
                    jitter: [0.0; mesh_core::live::generator::MAX_GEN_PARAMS],
                    bass_onsets: 0,
                    bass_phrase: [0; 4],
                    drum_fill: false,
                },
                &mut bar,
            );
            bar.sort();
            bar
        })
        .collect();
    let lane = &mut loaded.lanes[li];
    // From silence, so the option measured before leaves no tail.
    lane.reset_sound();
    lane.queue_option(oi);
    lane.begin_bar(0);
    render(lane, &[BarEvents::default()], ctx.rate, ctx.bpm);
    let raw = render(lane, &bar_events, ctx.rate, ctx.bpm);

    // The lane's glue stages, this lane alone.
    let mut glue = GlueChain::new(BLOCK);
    glue.set_params(&ctx.glue);
    glue.set_lane(li, LaneSetup { role, trim_db: 0.0, delay_send: 0.0, reverb_send: 0.0 });
    let mut processed = Vec::with_capacity(raw.len());
    let mut out = StereoBuffer::silence(BLOCK);
    for chunk in raw.chunks(BLOCK) {
        let mut lanes: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| StereoBuffer::silence(chunk.len()));
        lanes[li] = StereoBuffer::from_vec(chunk.to_vec());
        out.set_len_from_capacity(chunk.len());
        glue.process(&mut lanes, GlueKey::default(), ctx.bpm, &mut out);
        processed.extend_from_slice(lanes[li].as_slice());
    }
    Some(measure_lufs(&processed, ctx.rate as f64))
}

fn save(style: &Style, style_dir: &std::path::Path, dry_run: bool) {
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

/// Every sampler sample of the selected lanes (options and drum sub-voice options).
fn sampler_specs<'a>(style: &'a mut Style, selected: &'a dyn Fn(&str) -> bool) -> impl Iterator<Item = &'a mut SampleSpec> {
    style.spec.lanes.iter_mut().filter(move |l| selected(&l.name)).flat_map(|lane| {
        let sub = lane.sub_voices.iter_mut().flat_map(|sv| sv.options.iter_mut());
        lane.options.iter_mut().chain(sub)
    })
    .filter_map(|o| o.sampler.as_mut())
    .flat_map(|s| s.samples.iter_mut())
}

/// Measure each distinct sample file once (in parallel) and store the value
/// on every sample that uses the file. Silent or undecodable files get none.
fn analyse_samples(style: &mut Style, selected: &dyn Fn(&str) -> bool, rate: u32, remeasure: bool) {
    use rayon::prelude::*;
    let dir = style.dir.clone();
    let mut files: Vec<PathBuf> = Vec::new();
    for s in sampler_specs(style, selected) {
        if (remeasure || s.lufs.is_none()) && !files.contains(&s.file) {
            files.push(s.file.clone());
        }
    }
    let start = std::time::Instant::now();
    let results: Vec<(PathBuf, Option<f32>)> = files
        .par_iter()
        .map(|f| match analyse_sample_file(&dir.join(f), rate) {
            // 0.01 dB is far below what the gain can resolve; keeps style.yaml tidy.
            Ok(l) if l.is_finite() => (f.clone(), Some((l * 100.0).round() as f32 / 100.0)),
            Ok(_) => (f.clone(), None),
            Err(e) => {
                println!("warning: {}: {e}", f.display());
                (f.clone(), None)
            }
        })
        .collect();
    let mut stored = 0;
    for s in sampler_specs(style, selected) {
        if let Some((_, Some(l))) = results.iter().find(|(f, _)| *f == s.file) {
            s.lufs = Some(*l);
            stored += 1;
        }
    }
    println!("analysed {} files ({stored} samples) in {:.1} s", files.len(), start.elapsed().as_secs_f32());
}
