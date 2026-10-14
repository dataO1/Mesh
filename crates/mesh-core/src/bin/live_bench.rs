//! Offline CPU benchmark of the live engine (design P1 board benchmark, D32).
//!
//! Processes N seconds of audio as fast as possible on the calling thread and
//! prints the realtime factor, the worst block time and a per-stage
//! breakdown. Run it on the Orange Pi pinned to an audio core, e.g.
//! `taskset -c 1 live-bench --voices 32 --interp sinc8`.
//!
//! Without `--style` it writes a small synthetic style to a temp folder and
//! plays a stress pattern that holds `--voices` looping voices on every lane,
//! which measures the sampler's cost per voice. With `--style` it plays that
//! style with its own generators (or `--fixed` reference patterns).
//!
//! Usage: cargo run --release -p mesh-core --bin live-bench -- [OPTIONS]
//!
//!   --seconds S          audio to process (default 30)
//!   --block N            block size in frames (default 256)
//!   --rate HZ            sample rate (default 48000)
//!   --voices N[,N,N,N]   voice pool per lane; the stress pattern fills it (default 16)
//!   --interp MODE        linear | hermite | sinc8 (default sinc8)
//!   --lanes N            lanes switched on, 1..4 (default 4)
//!   --style DIR          benchmark a real style instead of the synthetic one
//!   --fixed              with --style: reference patterns instead of generators
//!   --clap ID            synthetic style: CLAP instrument plugin id for the CLAP lanes
//!   --clap-lanes N       synthetic style: lanes (from lane 1) using --clap, 0..2 (default 1 with --clap)
//!   --clap-path DIR      extra CLAP search path (repeatable)

use std::path::{Path, PathBuf};
use std::time::Instant;

use mesh_core::live::command::LiveCommand;
use mesh_core::live::engine::build;
use mesh_core::live::event::{flags, EventKind, LiveEvent};
use mesh_core::live::generator::fixed::FixedPattern;
use mesh_core::live::generator::LaneGenerator;
use mesh_core::live::loader::{load_style, ClapMode, LoadOptions};
use mesh_core::live::sampler::Interpolation;
use mesh_core::live::style::Style;
use mesh_core::live::NUM_LANES;
use mesh_core::types::StereoBuffer;

const USAGE: &str = "usage: live-bench [--seconds S] [--block N] [--rate HZ] [--voices N[,N,N,N]] \
[--interp linear|hermite|sinc8] [--lanes N] [--style DIR] [--fixed] [--clap ID] [--clap-lanes N] [--clap-path DIR]";

fn fail(msg: impl AsRef<str>) -> ! {
    eprintln!("live-bench: {}\n{USAGE}", msg.as_ref());
    std::process::exit(2);
}

fn parse<T: std::str::FromStr>(what: &str, s: &str) -> T {
    s.trim().parse().unwrap_or_else(|_| fail(format!("bad {what} '{s}'")))
}

/// Write a one-second looping saw tone as the only sample of every lane.
fn write_synthetic_style(dir: &Path, clap: Option<&str>, clap_lanes: usize) {
    let assets = dir.join("assets");
    std::fs::create_dir_all(&assets).unwrap_or_else(|e| fail(format!("cannot create {}: {e}", assets.display())));
    let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(assets.join("tone.wav"), spec).unwrap_or_else(|e| fail(e.to_string()));
    for i in 0..48_000u32 {
        // 220 Hz saw, slightly different per channel so the stereo path is exercised.
        let l = (i as f32 * 220.0 / 48_000.0).fract() * 2.0 - 1.0;
        let r = (i as f32 * 221.0 / 48_000.0).fract() * 2.0 - 1.0;
        w.write_sample((l * 8000.0) as i16).and_then(|_| w.write_sample((r * 8000.0) as i16)).unwrap_or_else(|e| fail(e.to_string()));
    }
    w.finalize().unwrap_or_else(|e| fail(e.to_string()));

    let controls = "    controls:\n\
        \x20     - { target: { type: instrument, param: 0 }, default: 1.0 }\n\
        \x20     - { target: { type: instrument, param: 1 }, default: 0.0 }\n\
        \x20     - { target: { type: send, bus: delay }, default: 0.2 }\n\
        \x20     - { target: { type: send, bus: reverb }, default: 0.2 }\n\
        \x20   generator: { family: procedural, name: fixed }\n";
    let sampler = "sampler: { samples: [ { file: assets/tone.wav, root_key: A3 } ] }";
    let mut yaml = String::from(
        "name: bench\nversion: 1\ntempo: { default: 172, min: 160, max: 180 }\ndefault_key: Am\n\
         progressions:\n  - { name: i, degrees: [1] }\nlanes:\n",
    );
    yaml.push_str(&format!(
        "  - name: drums\n    role: drums\n    archetypes: [two_step]\n    sub_voices:\n\
         \x20     - voice: backbone\n        options:\n          - name: tone\n            {sampler}\n{controls}"
    ));
    for (i, (name, role)) in [("bass", "bass"), ("lead", "lead"), ("extra", "extra")].into_iter().enumerate() {
        let option = match clap {
            Some(id) if i < clap_lanes => format!("clap: {{ plugin_id: {id} }}"),
            _ => sampler.to_string(),
        };
        yaml.push_str(&format!("  - name: {name}\n    role: {role}\n    options:\n      - name: tone\n        {option}\n{controls}"));
    }
    std::fs::write(dir.join("style.yaml"), yaml).unwrap_or_else(|e| fail(e.to_string()));
}

/// `voices` looping notes started on the bar and held for the whole bar.
fn stress_pattern(voices: usize) -> Box<dyn LaneGenerator> {
    let events = (0..voices.min(120))
        .map(|i| LiveEvent {
            tick: 0,
            kind: EventKind::NoteOn,
            flags: flags::LOOP,
            // Spread over two octaves so voices resample at different rates.
            key: 45 + (i % 24) as u8,
            vel: 60,
            gate: 383,
            note_id: i as u16 + 1,
            ..Default::default()
        })
        .collect();
    Box::new(FixedPattern::new(events))
}

fn main() {
    let mut seconds = 30.0f64;
    let mut block = 256usize;
    let mut rate = 48_000u32;
    let mut voices = [16usize; NUM_LANES];
    let mut interp = Interpolation::default();
    let mut lanes_on = NUM_LANES;
    let mut style_dir: Option<PathBuf> = None;
    let mut fixed = false;
    let mut clap: Option<String> = None;
    let mut clap_lanes: Option<usize> = None;
    let mut clap_paths: Vec<PathBuf> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().unwrap_or_else(|| fail(format!("{name} needs a value")));
        match arg.as_str() {
            "--seconds" => seconds = parse("seconds", &value("--seconds")),
            "--block" => block = parse::<usize>("block size", &value("--block")).max(1),
            "--rate" => rate = parse("sample rate", &value("--rate")),
            "--voices" => {
                let v = value("--voices");
                let list: Vec<usize> = v.split(',').map(|s| parse("voice count", s)).collect();
                voices = std::array::from_fn(|i| *list.get(i).or(list.last()).unwrap_or(&16));
            }
            "--interp" => {
                interp = match value("--interp").as_str() {
                    "linear" => Interpolation::Linear,
                    "hermite" => Interpolation::Hermite,
                    "sinc8" => Interpolation::Sinc8,
                    other => fail(format!("unknown interpolation '{other}'")),
                }
            }
            "--lanes" => lanes_on = parse::<usize>("lane count", &value("--lanes")).clamp(1, NUM_LANES),
            "--style" => style_dir = Some(PathBuf::from(value("--style"))),
            "--fixed" => fixed = true,
            "--clap" => clap = Some(value("--clap")),
            "--clap-lanes" => clap_lanes = Some(parse::<usize>("CLAP lane count", &value("--clap-lanes")).min(2)),
            "--clap-path" => clap_paths.push(PathBuf::from(value("--clap-path"))),
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => fail(format!("unknown argument '{other}'")),
        }
    }
    let clap_lanes = clap_lanes.unwrap_or(usize::from(clap.is_some()));

    let synthetic = style_dir.is_none();
    let temp = tempfile::tempdir().unwrap_or_else(|e| fail(e.to_string()));
    let dir = style_dir.unwrap_or_else(|| {
        write_synthetic_style(temp.path(), clap.as_deref(), clap_lanes);
        temp.path().to_path_buf()
    });
    let style = Style::load(&dir).unwrap_or_else(|e| fail(e.to_string()));

    let mut opts = LoadOptions::new(rate);
    opts.max_block = block.max(64);
    opts.interpolation = interp;
    opts.voices = voices;
    if clap.is_some() || !synthetic {
        opts.clap = ClapMode::Scan { extra_paths: clap_paths };
    }
    for (i, lane) in style.spec.lanes.iter().enumerate().take(NUM_LANES) {
        if synthetic {
            opts.generators[i] = Some(stress_pattern(voices[i]));
        } else if fixed {
            opts.generators[i] = Some(Box::new(FixedPattern::reference(lane.role)));
        }
    }
    let loaded = load_style(&style, opts).unwrap_or_else(|e| fail(e.to_string()));
    let clap_count = loaded.claps.len();
    let mut sys = build(loaded);
    for w in &sys.warnings {
        println!("warning: {w}");
    }
    for lane in lanes_on..NUM_LANES {
        sys.controller.send(LiveCommand::SetLaneOn { lane: lane as u8, on: false });
    }
    sys.renderer.service();
    sys.engine.set_profiling(true);

    let total_blocks = (seconds * rate as f64 / block as f64).ceil() as u64;
    let mut buf = StereoBuffer::silence(block);
    let mut render_ns = 0u128;
    let mut voice_sum = 0u64;
    let mut voice_peak = 0u32;
    let wall = Instant::now();
    for _ in 0..total_blocks {
        sys.engine.process(&mut buf, None);
        let active: u32 = (0..NUM_LANES).map(|l| sys.shared.lane(l).view().voices).sum();
        voice_sum += active as u64;
        voice_peak = voice_peak.max(active);
        // The bar renderer runs on another core in the player; time it apart.
        let t = Instant::now();
        sys.renderer.service();
        render_ns += t.elapsed().as_nanos();
    }
    let wall = wall.elapsed();
    let times = sys.engine.take_stage_times();

    let audio_s = total_blocks as f64 * block as f64 / rate as f64;
    let engine_s = times.total as f64 / 1e9;
    let block_ms = block as f64 / rate as f64 * 1e3;
    let pct = |ns: u64| 100.0 * ns as f64 / times.total.max(1) as f64;
    println!(
        "live-bench: {audio_s:.1} s audio, {rate} Hz, block {block} ({block_ms:.2} ms), interp {interp:?}, \
         voices {voices:?}, {lanes_on} lane(s) on, {clap_count} CLAP instrument(s), {} style",
        if synthetic { "synthetic stress" } else { "real" }
    );
    println!("  realtime factor      {:.1}x  (engine {:.3} s for {audio_s:.1} s audio, {:.2} % of one core)", audio_s / engine_s, engine_s, 100.0 * engine_s / audio_s);
    println!("  mean block           {:.3} ms ({:.1} % of the block period)", engine_s * 1e3 / total_blocks as f64, 100.0 * engine_s * 1e3 / total_blocks as f64 / block_ms);
    println!("  worst block          {:.3} ms ({:.1} % of the block period)", times.worst_block as f64 / 1e6, 100.0 * times.worst_block as f64 / 1e6 / block_ms);
    println!("  voices               mean {:.1}, peak {voice_peak}", voice_sum as f64 / total_blocks as f64);
    for (i, ns) in times.lanes.iter().enumerate() {
        println!("  lane {i} ({:<5})       {:5.1} %  ({:.3} ms/block)", sys.controller.lane(i).name, pct(*ns), *ns as f64 / 1e6 / total_blocks as f64);
    }
    println!("  glue + master        {:5.1} %  ({:.3} ms/block)", pct(times.glue), times.glue as f64 / 1e6 / total_blocks as f64);
    let other = times.total.saturating_sub(times.lanes.iter().sum::<u64>() + times.glue);
    println!("  clock, commands, UI  {:5.1} %", pct(other));
    println!("  bar renderer         {:.3} ms per bar-render pass total {:.1} ms (off the audio thread)", render_ns as f64 / 1e6 / total_blocks as f64, render_ns as f64 / 1e6);
    println!("  starved bars         {}", sys.shared.starved_bars());
    println!("  wall clock           {:.2} s", wall.as_secs_f64());
}
