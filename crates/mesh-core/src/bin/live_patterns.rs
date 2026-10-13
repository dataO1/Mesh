//! Live-mode pattern viewer: prints generated bars as ASCII step grids, so
//! generator output can be judged without audio (design §6, phase P3).
//!
//! Usage: cargo run -p mesh-core --bin live-patterns -- [OPTIONS]
//!
//!   --gen NAME[,NAME]   lanes to show: drums, bass, lead, arp, stab, pad
//!                       (default drums,bass,lead). Ignored with --style.
//!   --family F          procedural (default) | idiom
//!   --style PATH        style folder or style.yaml: show its four lanes with
//!                       their generator specs, kit layout and archetypes
//!   --seed N            slot seed (default 1)
//!   --bars N            bars to print, from bar 0 of a phrase (default 4)
//!   --fill              render the fill variant of every bar
//!   --half-time         half-time feel
//!   --key KEY           "Am", "F#", "8A" (default Am)
//!   --degrees LIST      chord degree per bar, 1-based, e.g. 1,6,4,5 (default 1)
//!   -p [LANE.]NAME=V    set a generator parameter or option, e.g.
//!                       `-p drums.style=0.5`, `-p density=0.8`, `-p rule_a2=0`.
//!                       Without a lane prefix it applies to every lane.
//!
//! Grid glyphs: `X` loud, `x` medium, `o` quiet, `#` two hits in one step,
//! `-` held note, `T` tied (slid) note. The break row shows the slice index
//! in base 36, upper case when the slice starts part-way in.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mesh_core::live::event::{flags, BarEvents, LiveEvent};
use mesh_core::live::generator::layout::DrumClass;
use mesh_core::live::generator::{build_drums, build_with, BarCtx, DrumLayout, LaneGenerator, MAX_GEN_PARAMS};
use mesh_core::live::glue::Role;
use mesh_core::live::harmony::{serde_key, HarmonyCtx};
use mesh_core::live::style::{GeneratorFamily, GeneratorSpec, StyleSpec, STYLE_FILE};
use mesh_core::live::{STEPS_PER_BAR, TICKS_PER_STEP};
use mesh_core::music::MusicalKey;

const USAGE: &str = "usage: live-patterns [--gen NAMES] [--family procedural|idiom] [--style PATH] \
[--seed N] [--bars N] [--fill] [--half-time] [--key KEY] [--degrees 1,6,4,5] [-p [LANE.]NAME=VALUE ...]";

struct Lane {
    name: String,
    role: Role,
    generator: Box<dyn LaneGenerator>,
}

struct Args {
    gens: Vec<String>,
    family: GeneratorFamily,
    style: Option<PathBuf>,
    seed: u32,
    bars: u8,
    fill: bool,
    half_time: bool,
    key: MusicalKey,
    degrees: Vec<u8>,
    /// (lane prefix or empty, name, value)
    params: Vec<(String, String, f32)>,
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}\n{USAGE}");
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut a = Args {
        gens: vec!["drums".into(), "bass".into(), "lead".into()],
        family: GeneratorFamily::Procedural,
        style: None,
        seed: 1,
        bars: 4,
        fill: false,
        half_time: false,
        key: MusicalKey::new(9, true),
        degrees: vec![1],
        params: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |what: &str| it.next().unwrap_or_else(|| fail(&format!("{what} needs a value")));
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--gen" => a.gens = value("--gen").split(',').map(|s| s.trim().to_string()).collect(),
            "--family" => {
                a.family = match value("--family").as_str() {
                    "procedural" => GeneratorFamily::Procedural,
                    "idiom" => GeneratorFamily::Idiom,
                    other => fail(&format!("unknown family '{other}'")),
                }
            }
            "--style" => a.style = Some(PathBuf::from(value("--style"))),
            "--seed" => a.seed = value("--seed").parse().unwrap_or_else(|_| fail("--seed needs a number")),
            "--bars" => a.bars = value("--bars").parse().unwrap_or_else(|_| fail("--bars needs a number")),
            "--fill" => a.fill = true,
            "--half-time" => a.half_time = true,
            "--key" => {
                let k = value("--key");
                a.key = serde_key::parse(&k).unwrap_or_else(|| fail(&format!("invalid key '{k}'")));
            }
            "--degrees" => {
                a.degrees = value("--degrees")
                    .split(',')
                    .map(|d| match d.trim().parse::<u8>() {
                        Ok(d) if (1..=7).contains(&d) => d,
                        _ => fail("--degrees are 1..=7, comma separated"),
                    })
                    .collect();
            }
            "-p" | "--param" => {
                let v = value("-p");
                let Some((name, val)) = v.split_once('=') else { fail("-p needs NAME=VALUE") };
                let val: f32 = val.parse().unwrap_or_else(|_| fail("-p value must be a number"));
                let (lane, name) = name.split_once('.').unwrap_or(("", name));
                a.params.push((lane.to_string(), name.to_string(), val));
            }
            other => fail(&format!("unknown argument '{other}'")),
        }
    }
    a
}

/// `spec` with the command-line parameters for lane `name` applied.
fn with_overrides(mut spec: GeneratorSpec, name: &str, args: &Args) -> GeneratorSpec {
    for (lane, param, value) in &args.params {
        if lane.is_empty() || lane == name {
            spec.params.insert(param.clone(), *value);
        }
    }
    spec
}

/// The lanes to show and the drum layout used for glyphs and the onset mask.
fn build_lanes(args: &Args) -> (Vec<Lane>, DrumLayout) {
    if let Some(path) = &args.style {
        let file = if path.is_dir() { path.join(STYLE_FILE) } else { path.clone() };
        let text = std::fs::read_to_string(&file).unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", file.display())));
        let style = StyleSpec::from_yaml(&text).unwrap_or_else(|e| fail(&format!("cannot parse {}: {e}", file.display())));
        let layout = style
            .lanes
            .iter()
            .find(|l| l.role == Role::Drums)
            .map(DrumLayout::from_lane)
            .unwrap_or_else(DrumLayout::standard);
        let lanes = style
            .lanes
            .iter()
            .map(|l| Lane {
                name: l.name.clone(),
                role: l.role,
                generator: build_with(&with_overrides(l.generator.clone(), &l.name, args), l.role, &layout, &l.archetypes),
            })
            .collect();
        return (lanes, layout);
    }
    let layout = DrumLayout::standard();
    let lanes = args
        .gens
        .iter()
        .map(|name| {
            let role = match name.as_str() {
                "drums" => Role::Drums,
                "bass" => Role::Bass,
                "pad" => Role::Extra,
                _ => Role::Lead,
            };
            let spec = GeneratorSpec { family: args.family, name: name.clone(), params: BTreeMap::new(), idiom: None };
            let spec = with_overrides(spec, name, args);
            let generator: Box<dyn LaneGenerator> = if role == Role::Drums {
                Box::new(build_drums(&spec, &layout, &[]))
            } else {
                build_with(&spec, role, &layout, &[])
            };
            Lane { name: name.clone(), role, generator }
        })
        .collect();
    (lanes, layout)
}

fn glyph(vel: u8) -> char {
    match vel {
        110.. => 'X',
        70..=109 => 'x',
        _ => 'o',
    }
}

fn step_of(ev: &LiveEvent) -> usize {
    ((ev.tick / TICKS_PER_STEP) as usize).min(STEPS_PER_BAR - 1)
}

fn print_row(label: &str, row: &[char; STEPS_PER_BAR]) {
    if row.iter().any(|&c| c != '.') {
        println!("  {label:<6} {}", row.iter().collect::<String>());
    }
}

fn print_drums(events: &[LiveEvent], layout: &DrumLayout) {
    const ROWS: [&str; 7] = ["kick", "snare", "ghost", "hat", "open", "perc", "break"];
    let mut rows = [['.'; STEPS_PER_BAR]; 7];
    for ev in events {
        let s = step_of(ev);
        let (row, mark) = match layout.class(ev.target) {
            DrumClass::Kick => (0, glyph(ev.vel)),
            DrumClass::Snare => (1, glyph(ev.vel)),
            DrumClass::Ghost => (2, glyph(ev.vel)),
            DrumClass::HatClosed => (3, glyph(ev.vel)),
            DrumClass::HatOpen => (4, glyph(ev.vel)),
            DrumClass::Perc(_) => (5, glyph(ev.vel)),
            DrumClass::Slice(i, _) => {
                let c = char::from_digit(i as u32 % 36, 36).unwrap_or('?');
                (6, if ev.start > 0 { c.to_ascii_uppercase() } else { c })
            }
            DrumClass::Unknown => continue,
        };
        rows[row][s] = if rows[row][s] == '.' { mark } else { '#' };
    }
    for (label, row) in ROWS.iter().zip(&rows) {
        print_row(label, row);
    }
}

fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[key as usize % 12], key as i32 / 12 - 1)
}

fn print_notes(events: &[LiveEvent]) {
    let mut keys: Vec<u8> = events.iter().map(|e| e.key).collect();
    keys.sort_unstable_by(|a, b| b.cmp(a));
    keys.dedup();
    for key in keys {
        let mut row = ['.'; STEPS_PER_BAR];
        for ev in events.iter().filter(|e| e.key == key) {
            let s = step_of(ev);
            let end = ((ev.tick + ev.gate) / TICKS_PER_STEP) as usize;
            for held in row.iter_mut().take(end.min(STEPS_PER_BAR)).skip(s + 1) {
                if *held == '.' {
                    *held = '-';
                }
            }
            row[s] = if ev.flags & flags::TIE != 0 { 'T' } else { glyph(ev.vel) };
        }
        print_row(&note_name(key), &row);
    }
}

fn main() {
    let args = parse_args();
    let (lanes, layout) = build_lanes(&args);
    for lane in &lanes {
        let defaults = lane.generator.param_defaults();
        let params: Vec<String> = lane
            .generator
            .param_names()
            .iter()
            .enumerate()
            .map(|(i, n)| format!("{n}={:.2}", defaults[i]))
            .collect();
        println!("{} ({:?}): {}", lane.name, lane.role, params.join(" "));
    }
    let empty_mask = [0u8; STEPS_PER_BAR];
    for bar in 0..args.bars {
        let bar_in_phrase = bar % 4;
        let degree = args.degrees[bar as usize % args.degrees.len()];
        let harmony = HarmonyCtx { key: args.key, chord_degree: degree - 1, progression: 0 };
        println!(
            "\n── bar {bar} (phrase bar {}) seed {} key {} degree {degree}{}{} ──",
            bar_in_phrase + 1,
            args.seed,
            args.key,
            if args.fill { " FILL" } else { "" },
            if args.half_time { " half-time" } else { "" },
        );
        println!("  {:<6} 1...2...3...4...", "");
        let render = |lane_index: usize, lane: &Lane, mask: &[u8; STEPS_PER_BAR]| {
            let params: [f32; MAX_GEN_PARAMS] = lane.generator.param_defaults();
            let ctx = BarCtx {
                seed: args.seed,
                lane: lane_index as u8,
                role: lane.role,
                bar_in_phrase,
                phrase: (bar / 4) as u32,
                fill: args.fill,
                half_time: args.half_time,
                params: &params,
                harmony,
                drum_mask: mask,
            };
            let mut out = BarEvents::default();
            lane.generator.render_bar(&ctx, &mut out);
            out
        };
        // Drums first: their onsets feed the other lanes.
        let mut mask = empty_mask;
        for (i, lane) in lanes.iter().enumerate().filter(|(_, l)| l.role == Role::Drums) {
            let out = render(i, lane, &empty_mask);
            mask = layout.onset_mask(out.as_slice());
            println!(" {} [{} events]", lane.name, out.len());
            print_drums(out.as_slice(), &layout);
        }
        for (i, lane) in lanes.iter().enumerate().filter(|(_, l)| l.role != Role::Drums) {
            let out = render(i, lane, &mask);
            println!(" {} [{} events]", lane.name, out.len());
            print_notes(out.as_slice());
        }
    }
}
