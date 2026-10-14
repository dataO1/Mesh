//! Offline live-mode render: loads a style, runs the live engine for N bars
//! and writes a WAV. Used for ear sign-off (design P1/P2) and regression
//! renders. The bar renderer is serviced between blocks, so the same
//! arguments always give the same file.
//!
//! Usage: cargo run --release -p mesh-core --bin live-render -- --style DIR --out FILE.wav [OPTIONS]
//!
//!   --style DIR          style folder (with style.yaml)
//!   --out FILE.wav       master output, 32-bit float stereo
//!   --bars N             bars to render (default 16)
//!   --fixed              hand-written reference patterns instead of the style's generators
//!   --seed N             seed of every slot of lane L is N + L (default: engine default)
//!   --seeds L=A,B,C,D    the four slot seeds of lane L (repeatable)
//!   --control L.C=V      performance control C (0..3) of lane L to V (0..1) (repeatable)
//!   --option L=N         instrument option / kit of lane L (repeatable)
//!   --mute L[,L]         lanes switched off
//!   --tempo BPM          tempo from bar 0 (clamped to the style's range)
//!   --key KEY            start key, e.g. "Am", "8A" (applies from bar 0)
//!   --key-at BAR=KEY     arm a key change during bar BAR (lands on the next phrase)
//!   --half-time          half-time feel
//!   --stems              also write FILE.lane<L>.wav per lane (post-glue lane signal)
//!   --clap               scan and load CLAP instruments / effects
//!   --rate HZ            sample rate (default 48000)
//!   --block N            block size (default 256)
//!   --interp MODE        linear | hermite | sinc8 (default sinc8)

use std::path::{Path, PathBuf};

use mesh_core::live::command::LiveCommand;
use mesh_core::live::engine::build;
use mesh_core::live::generator::fixed::FixedPattern;
use mesh_core::live::generator::LaneGenerator;
use mesh_core::live::glue::measure_lufs;
use mesh_core::live::harmony::serde_key;
use mesh_core::live::loader::{load_style, ClapMode, LoadOptions};
use mesh_core::live::sampler::Interpolation;
use mesh_core::live::style::Style;
use mesh_core::live::NUM_LANES;
use mesh_core::types::{StereoBuffer, StereoSample};

const USAGE: &str = "usage: live-render --style DIR --out FILE.wav [--bars N] [--fixed] [--seed N] \
[--seeds L=A,B,C,D] [--control L.C=V] [--option L=N] [--mute L,L] [--tempo BPM] [--key KEY] \
[--key-at BAR=KEY] [--half-time] [--stems] [--clap] [--rate HZ] [--block N] [--interp MODE]";

fn fail(msg: impl AsRef<str>) -> ! {
    eprintln!("live-render: {}\n{USAGE}", msg.as_ref());
    std::process::exit(2);
}

fn parse<T: std::str::FromStr>(what: &str, s: &str) -> T {
    s.trim().parse().unwrap_or_else(|_| fail(format!("bad {what} '{s}'")))
}

fn split<'a>(what: &str, s: &'a str, sep: char) -> (&'a str, &'a str) {
    s.split_once(sep).unwrap_or_else(|| fail(format!("{what} needs the form A{sep}B, got '{s}'")))
}

fn write_wav(path: &Path, rate: u32, samples: &[StereoSample]) {
    let spec = hound::WavSpec { channels: 2, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec).unwrap_or_else(|e| fail(format!("cannot create {}: {e}", path.display())));
    for s in samples {
        w.write_sample(s.left).and_then(|_| w.write_sample(s.right)).unwrap_or_else(|e| fail(format!("write failed: {e}")));
    }
    w.finalize().unwrap_or_else(|e| fail(format!("write failed: {e}")));
}

fn main() {
    let mut style_dir: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut bars = 16u64;
    let mut fixed = false;
    let mut stems = false;
    let mut rate = 48_000u32;
    let mut block = 256usize;
    let mut clap = false;
    let mut interp = Interpolation::default();
    let mut commands: Vec<LiveCommand> = Vec::new();
    let mut key_changes: Vec<(u64, mesh_core::music::MusicalKey)> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().unwrap_or_else(|| fail(format!("{name} needs a value")));
        match arg.as_str() {
            "--style" => style_dir = Some(PathBuf::from(value("--style"))),
            "--out" => out = Some(PathBuf::from(value("--out"))),
            "--bars" => bars = parse("bar count", &value("--bars")),
            "--fixed" => fixed = true,
            "--stems" => stems = true,
            "--clap" => clap = true,
            "--rate" => rate = parse("sample rate", &value("--rate")),
            "--block" => block = parse::<usize>("block size", &value("--block")).max(1),
            "--interp" => {
                interp = match value("--interp").as_str() {
                    "linear" => Interpolation::Linear,
                    "hermite" => Interpolation::Hermite,
                    "sinc8" => Interpolation::Sinc8,
                    other => fail(format!("unknown interpolation '{other}'")),
                }
            }
            "--seed" => {
                let base: u32 = parse("seed", &value("--seed"));
                for lane in 0..NUM_LANES as u8 {
                    for slot in 0..4 {
                        commands.push(LiveCommand::SetSlotSeed { lane, slot, seed: base.wrapping_add(lane as u32) });
                    }
                }
            }
            "--seeds" => {
                let v = value("--seeds");
                let (lane, seeds) = split("--seeds", &v, '=');
                let lane: u8 = parse("lane", lane);
                for (slot, seed) in seeds.split(',').enumerate().take(4) {
                    commands.push(LiveCommand::SetSlotSeed { lane, slot: slot as u8, seed: parse("seed", seed) });
                }
            }
            "--control" => {
                let v = value("--control");
                let (target, val) = split("--control", &v, '=');
                let (lane, control) = split("--control", target, '.');
                commands.push(LiveCommand::SetControl {
                    lane: parse("lane", lane),
                    control: parse("control", control),
                    value: parse("control value", val),
                });
            }
            "--option" => {
                let v = value("--option");
                let (lane, option) = split("--option", &v, '=');
                commands.push(LiveCommand::SetOption { lane: parse("lane", lane), option: parse("option", option) });
            }
            "--mute" => {
                for lane in value("--mute").split(',') {
                    commands.push(LiveCommand::SetLaneOn { lane: parse("lane", lane), on: false });
                }
            }
            "--tempo" => commands.push(LiveCommand::RequestTempo { bpm: parse("tempo", &value("--tempo")), bars: 0 }),
            "--key" => {
                let v = value("--key");
                commands.push(LiveCommand::SetKey(serde_key::parse(&v).unwrap_or_else(|| fail(format!("bad key '{v}'")))));
            }
            "--key-at" => {
                let v = value("--key-at");
                let (bar, key) = split("--key-at", &v, '=');
                key_changes.push((parse("bar", bar), serde_key::parse(key).unwrap_or_else(|| fail(format!("bad key '{key}'")))));
            }
            "--half-time" => commands.push(LiveCommand::SetHalfTime(true)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => fail(format!("unknown argument '{other}'")),
        }
    }
    let style_dir = style_dir.unwrap_or_else(|| fail("--style is required"));
    let out = out.unwrap_or_else(|| fail("--out is required"));

    let style = Style::load(&style_dir).unwrap_or_else(|e| fail(e.to_string()));
    let mut opts = LoadOptions::new(rate);
    opts.max_block = block.max(64);
    opts.interpolation = interp;
    if clap {
        opts.clap = ClapMode::Scan { extra_paths: Vec::new() };
    }
    if fixed {
        for (i, lane) in style.spec.lanes.iter().enumerate().take(NUM_LANES) {
            opts.generators[i] = Some(Box::new(FixedPattern::reference(lane.role)) as Box<dyn LaneGenerator>);
        }
    }
    let loaded = load_style(&style, opts).unwrap_or_else(|e| fail(e.to_string()));
    println!(
        "style '{}': {:.1} MB of samples, {} CLAP instrument(s), key {}, {} BPM",
        loaded.name,
        loaded.sample_bytes as f64 / 1e6,
        loaded.claps.len(),
        loaded.key,
        loaded.tempo.default
    );
    let mut sys = build(loaded);
    for w in &sys.warnings {
        println!("warning: {w}");
    }
    // Everything given on the command line applies from bar 0.
    for cmd in commands {
        sys.controller.send(cmd);
    }
    sys.renderer.service();

    let mut master: Vec<StereoSample> = Vec::new();
    let mut lanes: [Vec<StereoSample>; NUM_LANES] = std::array::from_fn(|_| Vec::new());
    let mut buf = StereoBuffer::silence(block);
    let mut tap: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| StereoBuffer::silence(block));
    key_changes.sort_by_key(|k| k.0);
    let mut next_key = 0;
    while sys.shared.clock().bar < bars {
        let bar = sys.shared.clock().bar;
        while next_key < key_changes.len() && key_changes[next_key].0 <= bar {
            sys.controller.send(LiveCommand::SetKey(key_changes[next_key].1));
            next_key += 1;
        }
        sys.engine.process(&mut buf, Some(&mut tap));
        master.extend_from_slice(buf.as_slice());
        if stems {
            for (dst, src) in lanes.iter_mut().zip(&tap) {
                dst.extend_from_slice(src.as_slice());
            }
        }
        sys.renderer.service();
    }

    let master_gr = sys.engine.take_master_peak_gr_db();
    write_wav(&out, rate, &master);
    let peak = master.iter().fold(0.0f32, |p, s| p.max(s.left.abs()).max(s.right.abs()));
    println!(
        "wrote {} ({:.1} s, peak {:.2} dBFS, {:.1} LUFS, key {} at the end, {} starved bar(s), master comp peak GR {:.1} dB)",
        out.display(),
        master.len() as f64 / rate as f64,
        20.0 * peak.max(1e-9).log10(),
        measure_lufs(&master, rate as f64),
        sys.shared.key(),
        sys.shared.starved_bars(),
        master_gr
    );
    if stems {
        for (i, lane) in lanes.iter().enumerate() {
            let path = out.with_extension(format!("lane{i}.wav"));
            write_wav(&path, rate, lane);
            println!("wrote {} ({}, {:.1} LUFS)", path.display(), sys.controller.lane(i).name, measure_lufs(lane, rate as f64));
        }
    }
}
