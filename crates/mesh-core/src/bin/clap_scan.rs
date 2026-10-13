//! CLAP instrument scan for live mode (design §5.2, risk R3).
//!
//! Lists installed CLAP instrument plugins and reports, per plugin:
//! note-port dialects, audio ports, voice-info (voice count, overlapping
//! notes), params flagged MODULATABLE_PER_NOTE_ID, and state support.
//!
//! CLAP has no capability flag for note expressions, so `--probe` measures it:
//! it renders the same note twice, once with a +12 semitone TUNING expression,
//! and compares zero-crossing rates (≈2× means the expression was applied).
//! `--probe` also confirms the plugin makes sound and reports NOTE_END.
//!
//! Usage: cargo run -p mesh-core --bin clap-scan -- [--probe] [-v] [PATH ...]
//!
//! PATH may be a directory of `.clap` bundles or a single bundle. Without
//! paths, the standard CLAP directories are scanned (~/.clap, /usr/lib/clap, ...).

use std::path::{Path, PathBuf};

use mesh_core::clap::{
    ClapDiscovery, ClapInstrument, DiscoveredClapPlugin, InstrumentNote, NoteExpr, TransportInfo,
};
use mesh_core::types::StereoBuffer;

const SAMPLE_RATE: u32 = 48_000;
const BLOCK: usize = 256;

fn main() {
    let mut probe = false;
    let mut verbose = false;
    let mut paths = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--probe" => probe = true,
            "-v" | "--verbose" => verbose = true,
            "-h" | "--help" => {
                eprintln!("usage: clap-scan [--probe] [-v] [PATH ...]");
                return;
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }

    let plugins = discover(&paths);
    let instruments: Vec<_> = plugins.iter().filter(|p| p.available && p.is_instrument()).collect();
    let unavailable: Vec<_> = plugins.iter().filter(|p| !p.available).collect();

    println!("CLAP instruments: {} (of {} plugins scanned)", instruments.len(), plugins.len());
    for p in &unavailable {
        println!("  unavailable: {:?}: {}", p.bundle_path, p.error_message.as_deref().unwrap_or("?"));
    }

    for p in instruments {
        println!();
        println!("{} ({}) v{} by {}", p.name, p.id, p.version, p.vendor);
        println!("  bundle:   {}", p.bundle_path.display());
        println!("  features: {}", p.features.join(", "));
        report(p, probe, verbose);
    }
}

fn discover(paths: &[PathBuf]) -> Vec<DiscoveredClapPlugin> {
    if paths.is_empty() {
        let mut d = ClapDiscovery::new();
        return d.scan().to_vec();
    }
    let mut out = Vec::new();
    let (dirs, bundles): (Vec<_>, Vec<_>) = paths.iter().cloned().partition(|p| p.is_dir() && !is_bundle(p));
    if !dirs.is_empty() {
        out.extend(ClapDiscovery::with_paths(dirs).scan().iter().cloned());
    }
    let single = ClapDiscovery::with_paths(Vec::new());
    for b in bundles {
        match single.scan_bundle(&b) {
            Ok(found) => out.extend(found),
            Err(e) => out.push(DiscoveredClapPlugin::unavailable(b, e.to_string())),
        }
    }
    out
}

fn is_bundle(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "clap")
}

fn report(p: &DiscoveredClapPlugin, probe: bool, verbose: bool) {
    let mut inst = match ClapInstrument::open_for_inspection(&p.bundle_path, Some(&p.id), SAMPLE_RATE) {
        Ok(i) => i,
        Err(e) => {
            println!("  LOAD FAILED: {e}");
            return;
        }
    };
    let caps = inst.capabilities();

    if caps.note_inputs.is_empty() {
        println!("  note ports:   none");
    }
    for (i, n) in caps.note_inputs.iter().enumerate() {
        let mut d = Vec::new();
        for (on, name) in [(n.clap, "clap"), (n.midi, "midi"), (n.midi_mpe, "midi-mpe"), (n.midi2, "midi2")] {
            if on {
                d.push(name);
            }
        }
        println!(
            "  note port {i}:  \"{}\" dialects [{}] preferred {}",
            n.name,
            d.join(", "),
            n.preferred.unwrap_or("-")
        );
    }
    println!("  host dialect: {}", caps.dialect.map(|d| format!("{d:?}")).unwrap_or_else(|| "UNUSABLE".into()));
    println!("  audio ports:  in {:?} out {:?}", caps.audio_inputs, caps.audio_outputs);
    match caps.voice_info {
        Some(v) => println!(
            "  voice-info:   {} voices (capacity {}), overlapping notes: {}",
            v.voice_count, v.voice_capacity, v.supports_overlapping_notes
        ),
        None => println!("  voice-info:   not supported"),
    }
    println!(
        "  params:       {} total, {} MODULATABLE_PER_NOTE_ID, {} AUTOMATABLE_PER_NOTE_ID",
        caps.param_count,
        caps.per_note_id_modulatable.len(),
        caps.per_note_id_automatable
    );
    if verbose {
        for name in &caps.per_note_id_modulatable {
            println!("    per-note mod: {name}");
        }
    } else if !caps.per_note_id_modulatable.is_empty() {
        let shown: Vec<_> = caps.per_note_id_modulatable.iter().take(5).cloned().collect();
        println!("    e.g. {} (-v for all)", shown.join(", "));
    }
    println!("  state:        {}", if caps.has_state { "yes" } else { "no" });
    println!("  note expr:    {}", if probe { "probing..." } else { "not declared by CLAP (use --probe)" });

    if probe && caps.dialect.is_some() {
        run_probe(&mut inst);
    }
}

/// Render `blocks` blocks, firing `first` in block 0. Returns the left
/// channel of the last `keep` blocks and whether NOTE_END was seen.
fn render(inst: &mut ClapInstrument, first: &[InstrumentNote], blocks: usize, keep: usize) -> (Vec<f32>, bool) {
    let mut buf = StereoBuffer::silence(BLOCK);
    let t = TransportInfo::new(120.0, true, 0.0);
    let mut out = Vec::new();
    let mut ended = false;
    for i in 0..blocks {
        let notes: &[InstrumentNote] = if i == 0 { first } else { &[] };
        if inst.process(&mut buf, notes, &t).is_err() {
            break;
        }
        ended |= !inst.ended_notes().is_empty();
        if i + keep >= blocks {
            out.extend(buf.as_slice().iter().map(|s| s.left));
        }
    }
    (out, ended)
}

fn zero_crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count()
}

fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

fn run_probe(inst: &mut ClapInstrument) {
    let key = 57; // A3, 220 Hz
    let hold = SAMPLE_RATE as usize / 2 / BLOCK;
    let tail = 2 * SAMPLE_RATE as usize / BLOCK;

    // Plain note, then release and wait for NOTE_END
    let (plain, _) = render(inst, &[InstrumentNote::on(0, key, 0.9, 1)], hold, hold / 2);
    let (_, ended) = render(inst, &[InstrumentNote::off(0, key, 1)], tail, 1);

    // Same note with +12 semitones TUNING expression right after note-on
    let with_expr = [
        InstrumentNote::on(0, key, 0.9, 2),
        InstrumentNote::expression(0, key, 2, NoteExpr::Tuning, 12.0),
    ];
    let (tuned, _) = render(inst, &with_expr, hold, hold / 2);
    render(inst, &[InstrumentNote::off(0, key, 2)], tail, 1);

    let level = rms(&plain);
    println!("  probe sound:  rms {level:.4} ({})", if level > 1e-3 { "audible" } else { "SILENT" });
    println!("  probe end:    NOTE_END {}", if ended { "reported" } else { "not reported" });
    if level <= 1e-3 {
        return;
    }
    let ratio = zero_crossings(&tuned) as f64 / zero_crossings(&plain).max(1) as f64;
    let verdict = if ratio > 1.6 {
        "TUNING applied"
    } else if ratio < 1.15 {
        "TUNING ignored"
    } else {
        "inconclusive"
    };
    println!("  probe expr:   zero-crossing ratio {ratio:.2} with +12 st TUNING ({verdict})");
}
