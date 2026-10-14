//! Starter live style: assembles a playable breakbeat style folder from
//! permissively licensed sample banks (the Tidal sample flakes), for testing
//! live mode without hand-made sound design. `scripts/live-starter-style.sh`
//! resolves the bank paths from the Nix store and calls this tool.
//!
//! Every chosen file is copied into `<OUT>/assets`, so the folder is
//! self-contained and can be exported to USB as is.
//!
//! Usage:
//!   live-starter-style --out DIR --breaks DIR --dirt DIR --pads DIR [--force]
//!   live-starter-style --inspect FILE...
//!
//!   --out DIR      style folder to create (must not exist unless --force)
//!   --breaks DIR   github:switchangel/breaks (NN_break.wav, two bars each)
//!   --dirt DIR     github:tidalcycles/Dirt-Samples
//!   --pads DIR     github:switchangel/pad
//!   --force        replace an existing style.yaml / assets in DIR
//!   --inspect      print length, rate and pitch estimate of each FILE
//!
//! # Analysis
//!
//! **Root keys** (pitched samples): YIN fundamental estimate (de Cheveigné &
//! Kawahara 2002) on the mono mix, 30 Hz–2 kHz, threshold 0.15, frames of
//! twice the longest period from just after the attack peak for up to 1.5 s.
//! The root is the median f0 of the voiced frames, rounded to the nearest
//! MIDI note. Cents off that note and the voiced share are printed. Under
//! 50 % voiced frames (chords, noisy pads) the spectral fallback
//! ([`spectral_root`]) takes over: the lowest note with a quarter of the
//! strongest semitone's energy, plus the top pitch classes for a sanity check.
//! Both report their method next to the root.
//!
//! **Break slices**: each break is two bars, so it gets an even 32-slice grid
//! (16ths) from its frame count, matching the drum generator's assumption.
//! Each slice is typed from its first 50 ms (attack) against the 50 ms before
//! it, in three bands: low (< 150 Hz), mid (200 Hz–2.5 kHz), high (> 5 kHz).
//! Band energies are relative to that band's strongest slice in the break.
//! - no fresh onset (attack < 1.3x the signal before it, or under 15 % of
//!   the loudest slice) → `other`
//! - loud (≥ 30 %), low band ≥ 25 % and at least doubling, low ≥ 0.36 x mid
//!   → `kick`
//! - mid band ≥ 50 % → `snare`
//! - high band ≥ 30 % with mid < 35 % → `hat`
//! - anything else → `ghost`
//!
//! Thresholds were set by inspecting the switchangel breaks (`STARTER_DEBUG=1`
//! prints the per-slice features); they are a starting point, not a
//! general drum classifier.
//!
//! If a break yields no kick or no snare, it falls back to the amen cycle of
//! the drum layout's standard break, repeated over both bars.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mesh_core::live::sampler::SampleData;
use mesh_core::live::style::{Style, STYLE_FILE};

const USAGE: &str = "usage: live-starter-style --out DIR --breaks DIR --dirt DIR --pads DIR [--force]\n       \
live-starter-style --inspect FILE...";

fn fail(msg: impl AsRef<str>) -> ! {
    eprintln!("live-starter-style: {}\n{USAGE}", msg.as_ref());
    std::process::exit(2);
}

// ─── Selection ────────────────────────────────────────────────────────

/// One drum kit: an option of every drum sub-voice. Paths are relative to
/// Dirt-Samples. Kit `k` of the lane is option `k` of each sub-voice.
struct Kit {
    name: &'static str,
    /// kick, snare, ghost/rim
    backbone: [&'static str; 3],
    /// closed, open
    hats: [&'static str; 2],
    perc: [&'static str; 2],
    /// Break file in the switchangel bank.
    break_file: &'static str,
}

const KITS: [Kit; 3] = [
    Kit {
        name: "909",
        backbone: ["bd/BT0A0D0.wav", "sn/ST0T0S7.wav", "rm/RIM0.wav"],
        hats: ["hc/HHCD0.wav", "ho/HHOD0.wav"],
        perc: ["perc/000_perc0.wav", "cp/HANDCLP0.wav"],
        break_file: "10_break.wav",
    },
    Kit {
        name: "808",
        backbone: ["808bd/BD0050.WAV", "808sd/SD0050.WAV", "808/RS.WAV"],
        hats: ["808hc/HC00.WAV", "808oh/OH00.WAV"],
        perc: ["808/CB.WAV", "808/CP.WAV"],
        break_file: "12_break.wav",
    },
    Kit {
        name: "jungle",
        backbone: ["jungle/jungle4kick1.wav", "jungle/jungle4snare1.wav", "jungle/jungle4hit1.wav"],
        hats: ["jungle/jungle4closedhh.wav", "jungle/jungle4openhh.wav"],
        perc: ["jungle/jungle4perc1.wav", "jungle/jungle4perc2.wav"],
        break_file: "14_break.wav",
    },
];

/// A pitched one-shot: option name, bank, path in the bank, asset name.
struct Pitched {
    option: &'static str,
    bank: Bank,
    path: &'static str,
    asset: &'static str,
}

#[derive(Clone, Copy)]
enum Bank {
    Dirt,
    Pads,
}

const BASS: Pitched =
    Pitched { option: "round-sub", bank: Bank::Dirt, path: "jungbass/018_synthy_round.wav", asset: "bass/round-sub.wav" };
const LEADS: [Pitched; 2] = [
    Pitched { option: "stab", bank: Bank::Dirt, path: "stab/006_stab15.wav", asset: "lead/stab.wav" },
    Pitched { option: "juno", bank: Bank::Dirt, path: "juno/07_juno_release_mid.wav", asset: "lead/juno.wav" },
];
const PAD: Pitched =
    Pitched { option: "sw-pad", bank: Bank::Pads, path: "14_switch_angel_pad.wav", asset: "pad/sw-pad.wav" };

/// License files copied next to the assets (bank, file in bank, asset name).
const LICENSES: [(Bank, &str, &str); 1] = [(Bank::Pads, "LICENSE", "LICENSE-switchangel.txt")];

// ─── DSP helpers ──────────────────────────────────────────────────────

fn mono(data: &SampleData) -> Vec<f32> {
    (0..data.len()).map(|i| {
        let f = data.frame(i);
        0.5 * (f.left + f.right)
    }).collect()
}

/// RBJ biquad, direct form I.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
}

impl Biquad {
    fn new(sr: f32, hz: f32, high: bool) -> Self {
        let w = std::f32::consts::TAU * hz / sr;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = if high {
            [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0]
        } else {
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0]
        };
        Self { b: [b[0] / a0, b[1] / a0, b[2] / a0], a: [-2.0 * c / a0, (1.0 - alpha) / a0] }
    }

    fn run(&self, x: &[f32]) -> Vec<f32> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        x.iter()
            .map(|&x0| {
                let y0 = self.b[0] * x0 + self.b[1] * x1 + self.b[2] * x2 - self.a[0] * y1 - self.a[1] * y2;
                (x2, x1, y2, y1) = (x1, x0, y1, y0);
                y0
            })
            .collect()
    }

    /// Two cascaded passes (24 dB/oct).
    fn run2(&self, x: &[f32]) -> Vec<f32> {
        self.run(&self.run(x))
    }
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

/// Pitch estimate of a one-shot.
struct Pitch {
    hz: f32,
    midi: u8,
    cents: f32,
    voiced: f32,
}

fn yin(x: &[f32], sr: f32) -> Option<Pitch> {
    const FMIN: f32 = 30.0;
    const FMAX: f32 = 2000.0;
    const THRESHOLD: f32 = 0.15;
    let tau_max = (sr / FMIN) as usize;
    let tau_min = (sr / FMAX).max(2.0) as usize;
    let w = tau_max * 2;
    let peak = x.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?.0;
    let start = peak + (0.03 * sr) as usize;
    let end = x.len().min(start + (1.5 * sr) as usize);
    if end < start + w + tau_max {
        return None;
    }
    let mut f0s = Vec::new();
    let mut frames = 0;
    let mut d = vec![0.0f32; tau_max + 1];
    let mut pos = start;
    while pos + w + tau_max <= end {
        frames += 1;
        let frame = &x[pos..pos + w + tau_max];
        if rms(&frame[..w]) < 1e-4 {
            pos += w / 2;
            continue;
        }
        for (tau, dt) in d.iter_mut().enumerate().skip(1) {
            *dt = (0..w).map(|j| {
                let diff = frame[j] - frame[j + tau];
                diff * diff
            }).sum();
        }
        // Cumulative mean normalized difference.
        let mut sum = 0.0;
        let mut cmnd = vec![1.0f32; tau_max + 1];
        for tau in 1..=tau_max {
            sum += d[tau];
            cmnd[tau] = if sum > 0.0 { d[tau] * tau as f32 / sum } else { 1.0 };
        }
        let mut best = None;
        let mut tau = tau_min;
        while tau < tau_max {
            if cmnd[tau] < THRESHOLD {
                while tau + 1 < tau_max && cmnd[tau + 1] < cmnd[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }
        if let Some(t) = best {
            // Parabolic interpolation around the dip.
            let (a, b, c) = (cmnd[t - 1], cmnd[t], cmnd[t + 1]);
            let den = a - 2.0 * b + c;
            let shift = if den.abs() > 1e-9 { 0.5 * (a - c) / den } else { 0.0 };
            f0s.push(sr / (t as f32 + shift));
        }
        pos += w / 2;
    }
    if f0s.is_empty() {
        return None;
    }
    f0s.sort_by(f32::total_cmp);
    let hz = f0s[f0s.len() / 2];
    let note = 69.0 + 12.0 * (hz / 440.0).log2();
    let midi = note.round().clamp(0.0, 127.0);
    Some(Pitch { hz, midi: midi as u8, cents: (note - midi) * 100.0, voiced: f0s.len() as f32 / frames as f32 })
}


/// Spectral fallback for chords and noisy material, where YIN finds no
/// stable period: semitone energies (Hann-windowed DFT at every MIDI note
/// 28..=96, 0.5 s frames over the first 4 s after the attack), the root is
/// the lowest note with at least 25 % of the strongest note's energy. Also
/// returns the three strongest pitch classes.
fn spectral_root(x: &[f32], sr: f32) -> Option<(u8, [u8; 3])> {
    const LO: u8 = 28;
    const HI: u8 = 96;
    let win = (0.5 * sr) as usize;
    let peak = x.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?.0;
    let start = peak + (0.05 * sr) as usize;
    let end = x.len().min(start + (4.0 * sr) as usize);
    if end < start + win {
        return None;
    }
    let hann: Vec<f32> =
        (0..win).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / win as f32).cos()).collect();
    let mut energy = [0.0f64; 128];
    let mut pos = start;
    while pos + win <= end {
        let frame = &x[pos..pos + win];
        for m in LO..=HI {
            let hz = 440.0 * 2f64.powf((m as f64 - 69.0) / 12.0);
            let w = std::f64::consts::TAU * hz / sr as f64;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, (&v, &h)) in frame.iter().zip(&hann).enumerate() {
                let (s, c) = (w * i as f64).sin_cos();
                let v = (v * h) as f64;
                re += v * c;
                im += v * s;
            }
            energy[m as usize] += re * re + im * im;
        }
        pos += win / 2;
    }
    let max = energy.iter().copied().fold(0.0, f64::max);
    if max <= 0.0 {
        return None;
    }
    let root = (LO..=HI).find(|&m| energy[m as usize] >= 0.25 * max)?;
    let mut chroma = [0.0f64; 12];
    for m in LO..=HI {
        chroma[m as usize % 12] += energy[m as usize];
    }
    let mut order: Vec<u8> = (0..12).collect();
    order.sort_by(|a, b| chroma[*b as usize].total_cmp(&chroma[*a as usize]));
    Some((root, [order[0], order[1], order[2]]))
}
fn note_name(midi: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[midi as usize % 12], midi as i32 / 12 - 1)
}

// ─── Break slicing ────────────────────────────────────────────────────

const SLICES: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Kick,
    Snare,
    Ghost,
    Hat,
    Other,
}

impl Kind {
    fn yaml(self) -> &'static str {
        match self {
            Kind::Kick => "kick",
            Kind::Snare => "snare",
            Kind::Ghost => "ghost",
            Kind::Hat => "hat",
            Kind::Other => "other",
        }
    }
}

/// Amen cycle of `layout::STANDARD_BREAK` (16 steps), used as the fallback.
const AMEN: [Kind; 16] = {
    use Kind::*;
    [Kick, Hat, Kick, Hat, Snare, Hat, Hat, Ghost, Hat, Ghost, Kick, Kick, Snare, Hat, Hat, Ghost]
};

struct SliceInfo {
    start: u64,
    kind: Kind,
    /// Attack RMS relative to the loudest slice.
    rel: f32,
    /// Attack RMS over the RMS just before the slice.
    rise: f32,
    /// Band attack energies relative to the band's strongest slice.
    low: f32,
    mid: f32,
    high: f32,
    /// Low-band attack over the low band just before the slice.
    low_rise: f32,
}

struct Break {
    slices: Vec<SliceInfo>,
    bpm: f64,
    fallback: bool,
}

fn slice_break(data: &SampleData) -> Break {
    let x = mono(data);
    let sr = data.sample_rate() as f32;
    let n = x.len();
    let low = Biquad::new(sr, 150.0, false).run2(&x);
    let mid = Biquad::new(sr, 2500.0, false).run2(&Biquad::new(sr, 200.0, true).run2(&x));
    let high = Biquad::new(sr, 5000.0, true).run2(&x);
    let starts: Vec<usize> = (0..SLICES).map(|i| (i as f64 * n as f64 / SLICES as f64).round() as usize).collect();
    let win = ((0.05 * sr) as usize).min(n / SLICES);
    let attack = |sig: &[f32], s: usize| rms(&sig[s..(s + win).min(n)]);
    let before = |sig: &[f32], s: usize| rms(&sig[s.saturating_sub(win)..s]);
    let rise = |sig: &[f32], s: usize| if s == 0 { 10.0 } else { attack(sig, s) / before(sig, s).max(1e-6) };
    let max_of = |sig: &[f32]| starts.iter().map(|&s| attack(sig, s)).fold(1e-9f32, f32::max);
    let (full_max, low_max, mid_max, high_max) = (max_of(&x), max_of(&low), max_of(&mid), max_of(&high));
    let mut slices: Vec<SliceInfo> = starts
        .iter()
        .map(|&s| {
            let info = SliceInfo {
                start: s as u64,
                kind: Kind::Other,
                rel: attack(&x, s) / full_max,
                rise: rise(&x, s),
                low: attack(&low, s) / low_max,
                mid: attack(&mid, s) / mid_max,
                high: attack(&high, s) / high_max,
                low_rise: rise(&low, s),
            };
            let kind = if info.rise < 1.3 || info.rel < 0.15 {
                Kind::Other
            } else if info.rel >= 0.3 && info.low >= 0.25 && info.low_rise >= 2.0 && info.low >= 0.36 * info.mid {
                Kind::Kick
            } else if info.mid >= 0.5 {
                Kind::Snare
            } else if info.high >= 0.3 && info.mid < 0.35 {
                Kind::Hat
            } else {
                Kind::Ghost
            };
            SliceInfo { kind, ..info }
        })
        .collect();
    let has = |k| slices.iter().any(|s| s.kind == k);
    let fallback = !(has(Kind::Kick) && has(Kind::Snare));
    if fallback {
        for (i, s) in slices.iter_mut().enumerate() {
            s.kind = AMEN[i % 16];
        }
    }
    let bpm = 8.0 * 60.0 * data.sample_rate() as f64 / n as f64;
    Break { slices, bpm, fallback }
}

// ─── Style assembly ───────────────────────────────────────────────────

struct Paths {
    out: PathBuf,
    breaks: PathBuf,
    dirt: PathBuf,
    pads: PathBuf,
}

impl Paths {
    fn bank(&self, bank: Bank) -> &Path {
        match bank {
            Bank::Dirt => &self.dirt,
            Bank::Pads => &self.pads,
        }
    }
}

fn decode(path: &Path) -> SampleData {
    SampleData::decode(path, 60).unwrap_or_else(|e| fail(format!("cannot decode {}: {e}", path.display())))
}

/// Copy `src` to `<out>/assets/<asset>`; returns the style-relative path.
fn copy_asset(paths: &Paths, src: &Path, asset: &str) -> String {
    let rel = format!("assets/{asset}");
    let dst = paths.out.join(&rel);
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap_or_else(|e| fail(format!("{}: {e}", dst.display())));
    std::fs::copy(src, &dst).unwrap_or_else(|e| fail(format!("copy {} -> {}: {e}", src.display(), dst.display())));
    rel
}

/// Root key of a pitched sample and how it was found.
struct Root {
    midi: u8,
    detail: String,
}

/// YIN when at least half the frames are voiced, else the spectral fallback.
fn estimate_root(data: &SampleData) -> Option<Root> {
    let x = mono(data);
    let sr = data.sample_rate() as f32;
    let yin_root = yin(&x, sr);
    if let Some(p) = yin_root.as_ref().filter(|p| p.voiced >= 0.5) {
        return Some(Root {
            midi: p.midi,
            detail: format!("YIN {:.2} Hz, {:+.0} ct, voiced {:.0} %", p.hz, p.cents, p.voiced * 100.0),
        });
    }
    let (midi, top) = spectral_root(&x, sr)?;
    const PC: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let yin_note = yin_root.map_or("none".to_string(), |p| format!("{} at {:.0} % voiced", note_name(p.midi), p.voiced * 100.0));
    Some(Root {
        midi,
        detail: format!(
            "spectral: lowest strong note, top pitch classes {} {} {} (YIN {yin_note})",
            PC[top[0] as usize], PC[top[1] as usize], PC[top[2] as usize]
        ),
    })
}

/// Copy a pitched sample and measure its root. Returns (path, MIDI root).
fn pitched(paths: &Paths, p: &Pitched, report: &mut String) -> (String, u8) {
    let src = paths.bank(p.bank).join(p.path);
    let data = decode(&src);
    let root = estimate_root(&data).unwrap_or_else(|| fail(format!("no pitch found in {}", src.display())));
    let _ = writeln!(report, "  {:<10} root {:<4} {}  ({})", p.option, note_name(root.midi), root.detail, p.path);
    (copy_asset(paths, &src, p.asset), root.midi)
}

fn build(paths: &Paths) -> String {
    let mut report = String::new();
    let mut y = String::new();

    // Drum kits.
    let mut backbone = String::new();
    let mut breaks = String::new();
    let mut hats = String::new();
    let mut perc = String::new();
    let _ = writeln!(report, "Break slices (32 = two bars of 16ths; K kick, S snare, g ghost, h hat, . other):");
    for kit in &KITS {
        let dir = format!("drums/{}", kit.name);
        let one = |file: &str, name: &str| {
            let ext = Path::new(file).extension().and_then(|e| e.to_str()).unwrap_or("wav").to_ascii_lowercase();
            copy_asset(paths, &paths.dirt.join(file), &format!("{dir}/{name}.{ext}"))
        };
        let [k, s, g] = kit.backbone;
        let _ = write!(
            backbone,
            "          - name: {n}\n            sampler:\n              samples:\n                - {{ file: {} }}\n                - {{ file: {} }}\n                - {{ file: {} }}\n",
            one(k, "kick"),
            one(s, "snare"),
            one(g, "ghost"),
            n = kit.name
        );
        let [c, o] = kit.hats;
        let _ = write!(
            hats,
            "          - name: {n}\n            sampler:\n              samples:\n                - {{ file: {}, choke: 2 }}\n                - {{ file: {}, choke: 2 }}\n",
            one(c, "hat-closed"),
            one(o, "hat-open"),
            n = kit.name
        );
        let [p1, p2] = kit.perc;
        let _ = write!(
            perc,
            "          - name: {n}\n            sampler:\n              samples:\n                - {{ file: {} }}\n                - {{ file: {} }}\n",
            one(p1, "perc1"),
            one(p2, "perc2"),
            n = kit.name
        );

        let src = paths.breaks.join(kit.break_file);
        let data = decode(&src);
        let b = slice_break(&data);
        let stem = kit.break_file.trim_end_matches(".wav");
        let rel = copy_asset(paths, &src, &format!("breaks/sw-{stem}.wav"));
        let grid: String = b
            .slices
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let c = match s.kind {
                    Kind::Kick => 'K',
                    Kind::Snare => 'S',
                    Kind::Ghost => 'g',
                    Kind::Hat => 'h',
                    Kind::Other => '.',
                };
                if i % 4 == 3 && i != SLICES - 1 { format!("{c} ") } else { c.to_string() }
            })
            .collect();
        let _ = writeln!(
            report,
            "  sw-{stem:<9} {grid}  ({:.1} BPM at 2 bars, {} frames @ {} Hz{})",
            b.bpm,
            data.len(),
            data.sample_rate(),
            if b.fallback { ", AMEN FALLBACK" } else { "" }
        );
        if std::env::var_os("STARTER_DEBUG").is_some() {
            for (i, s) in b.slices.iter().enumerate() {
                let _ = writeln!(
                    report,
                    "      {i:2} {:<5} rel {:.2} rise {:5.1} low {:.2} (rise {:5.1}) mid {:.2} high {:.2}",
                    s.kind.yaml(),
                    s.rel,
                    s.rise,
                    s.low,
                    s.low_rise,
                    s.mid,
                    s.high
                );
            }
        }
        let _ = write!(
            breaks,
            "          - name: sw-{stem}\n            sampler:\n              samples:\n                - file: {rel}\n                  choke: 1\n                  slices:\n"
        );
        for s in &b.slices {
            let _ = writeln!(breaks, "                    - {{ start: {}, type: {} }}", s.start, s.kind.yaml());
        }
    }

    let _ = writeln!(report, "Root keys:");
    let (bass_file, bass_root) = pitched(paths, &BASS, &mut report);
    let leads: Vec<(String, u8)> = LEADS.iter().map(|p| pitched(paths, p, &mut report)).collect();
    let (pad_file, pad_root) = pitched(paths, &PAD, &mut report);

    for (bank, file, name) in LICENSES {
        copy_asset(paths, &paths.bank(bank).join(file), name);
    }
    let _ = std::fs::copy(paths.breaks.join("LICENSE"), paths.out.join("assets/breaks/LICENSE-switchangel-breaks.txt"));

    let _ = write!(
        y,
        r#"# Starter breakbeat style, generated by scripts/live-starter-style.sh
# (crates/mesh-core/src/bin/live_starter_style.rs). Samples come from
# switchangel/breaks + switchangel/pad (Unlicense) and tidalcycles/Dirt-Samples
# (mixed sources: keep this folder private). Values are starting points to
# tune by ear; style-calibrate writes the lufs_trim_db fields.
name: starter-breaks
version: 1
tempo: {{ default: 172, min: 160, max: 178 }}
default_key: Am

progressions:
  - name: i-VI-iv-v
    degrees: [1, 6, 4, 5]
  - name: i-VII
    degrees: [1, 7]
    bars_per_chord: 2
default_progression: 0

loudness: {{ drums: -12, bass: -14, lead: -16, extra: -18 }}

glue:
  role_hpf: {{ lead: 500, extra: 300 }}
  bass_mono_hz: 120
  duck: {{ depth_db: -6, low_hz: 50, high_hz: 120, release_beats: 0.5 }}
  drum_bus:
    comp: {{ threshold_db: -12, ratio: 4, attack_ms: 10, release_ms: 120 }}
    drive: 0.2
  shared_drive: {{ enabled: false }}
  master: {{ threshold_db: -8, ratio: 4, attack_ms: 25, release_ms: 10 }}

send_fx:
  delay: {{ id: delay, source: native, params: {{ 0: 0.375, 1: 0.4 }} }}
  reverb: {{ id: reverb, source: native }}

lanes:
  # Kit k = option k of every sub-voice (909 + sw-10, 808 + sw-12, jungle + sw-14).
  - name: drums
    role: drums
    archetypes: [half_time, two_step, rolling, jungle, hardcore]
    sub_voices:
      - voice: backbone
        options:
{backbone}      - voice: break
        choke: 1
        options:
{breaks}      - voice: hats
        choke: 2
        options:
{hats}      - voice: perc
        options:
{perc}    fx:
      - {{ id: filter, source: native, params: {{ 0: 0.5 }} }}
    controls:
      - {{ label: style, target: {{ type: generator, param: style }}, default: 0.4 }}
      - {{ label: dens, target: {{ type: generator, param: density }}, default: 0.5 }}
      - {{ label: mutate, target: {{ type: generator, param: mutate }}, default: 0.0 }}
      - {{ label: verb, target: {{ type: send, bus: reverb }}, min: 0.0, max: 0.6, default: 0.0 }}
    generator:
      family: procedural
      name: drums
      params: {{ style: 0.4, density: 0.5, swing: 0.0 }}

  - name: bass
    role: bass
    default_option: 0
    options:
      - name: {bass_opt}
        sampler:
          samples:
            - {{ file: {bass_file}, root_key: {bass_root}, choke: 1 }}   # {bass_note}, measured
      # Surge XT init patch. Needs the CLAP bundle discoverable (~/.clap).
      - name: surge-init
        clap:
          plugin_id: org.surge-synth-team.surge-xt
    fx:
      - {{ id: drive, source: native, params: {{ 0: 0.3 }}, dry_wet: 0.8 }}
    controls:
      - {{ label: cx, target: {{ type: generator, param: complexity }}, default: 0.4 }}
      - {{ label: dens, target: {{ type: generator, param: density }}, default: 0.5 }}
      - {{ label: spread, target: {{ type: generator, param: spread }}, default: 0.3 }}
      - {{ label: duck, target: {{ type: glue, param: duck_depth }}, min: 0.0, max: 1.0 }}
    generator:
      family: procedural
      name: bass
      params: {{ complexity: 0.4, density: 0.5 }}

  - name: lead
    role: lead
    options:
      - name: {l0_opt}
        sampler:
          samples:
            - {{ file: {l0_file}, root_key: {l0_root} }}   # {l0_note}, measured
      - name: {l1_opt}
        sampler:
          samples:
            - {{ file: {l1_file}, root_key: {l1_root} }}   # {l1_note}, measured
    controls:
      - {{ label: spread, target: {{ type: generator, param: spread }}, default: 0.4 }}
      - {{ label: dens, target: {{ type: generator, param: density }}, default: 0.3 }}
      - {{ label: cutoff, target: {{ type: instrument, param: 0 }}, default: 1.0 }}
      - {{ label: delay, target: {{ type: send, bus: delay }}, min: 0.0, max: 0.7, curve: s }}
    generator:
      family: procedural
      name: lead
      params: {{ complexity: 0.2, density: 0.3 }}
    sends: {{ delay: 0.2, reverb: 0.1 }}

  - name: extra
    role: extra
    options:
      - name: {pad_opt}
        sampler:
          samples:
            - {{ file: {pad_file}, root_key: {pad_root} }}   # {pad_note}, measured
    controls:
      - {{ label: spread, target: {{ type: generator, param: spread }}, default: 0.2 }}
      - {{ label: dens, target: {{ type: generator, param: density }}, default: 0.3 }}
      - {{ label: reso, target: {{ type: instrument, param: 1 }}, curve: log, default: 0.0 }}
      - {{ label: verb, target: {{ type: send, bus: reverb }}, min: 0.0, max: 0.8 }}
    generator:
      family: idiom
      name: pad
      params: {{ complexity: 0.1, density: 0.3 }}
      idiom:
        level_maps:
          - name: long-notes
            voice: pad
            levels: [255, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 0, 0, 64, 0]
    sends: {{ reverb: 0.4 }}
"#,
        bass_opt = BASS.option,
        bass_note = note_name(bass_root),
        l0_opt = LEADS[0].option,
        l0_file = leads[0].0,
        l0_root = leads[0].1,
        l0_note = note_name(leads[0].1),
        l1_opt = LEADS[1].option,
        l1_file = leads[1].0,
        l1_root = leads[1].1,
        l1_note = note_name(leads[1].1),
        pad_opt = PAD.option,
        pad_note = note_name(pad_root),
    );
    std::fs::write(paths.out.join(STYLE_FILE), y).unwrap_or_else(|e| fail(format!("write style.yaml: {e}")));
    report
}

fn inspect(files: &[PathBuf]) {
    for f in files {
        let data = decode(f);
        let secs = data.len() as f64 / data.sample_rate() as f64;
        let root = estimate_root(&data)
            .map_or("no pitch".to_string(), |r| format!("{:<4} {}", note_name(r.midi), r.detail));
        println!("{secs:>6.2} s {:>6} Hz  {root}  {}", data.sample_rate(), f.display());
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut out, mut breaks, mut dirt, mut pads, mut force) = (None, None, None, None, false);
    let mut inspect_files = None::<Vec<PathBuf>>;
    while let Some(a) = args.next() {
        let mut val = || args.next().map(PathBuf::from).unwrap_or_else(|| fail(format!("{a} needs a value")));
        match a.as_str() {
            "--out" => out = Some(val()),
            "--breaks" => breaks = Some(val()),
            "--dirt" => dirt = Some(val()),
            "--pads" => pads = Some(val()),
            "--force" => force = true,
            "--inspect" => inspect_files = Some(args.by_ref().map(PathBuf::from).collect()),
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => fail(format!("unknown argument {other}")),
        }
    }
    if let Some(files) = inspect_files {
        inspect(&files);
        return;
    }
    let need = |p: Option<PathBuf>, what: &str| p.unwrap_or_else(|| fail(format!("missing {what}")));
    let paths = Paths {
        out: need(out, "--out"),
        breaks: need(breaks, "--breaks"),
        dirt: need(dirt, "--dirt"),
        pads: need(pads, "--pads"),
    };
    if paths.out.join(STYLE_FILE).exists() && !force {
        fail(format!("{} already holds a style (use --force)", paths.out.display()));
    }
    if force {
        let _ = std::fs::remove_dir_all(paths.out.join("assets"));
    }
    std::fs::create_dir_all(&paths.out).unwrap_or_else(|e| fail(format!("{}: {e}", paths.out.display())));

    let report = build(&paths);
    print!("{report}");
    match Style::load(&paths.out) {
        Ok(style) => println!("Style '{}' written to {} and validated.", style.spec.name, paths.out.display()),
        Err(e) => {
            eprintln!("live-starter-style: generated style does not validate:\n{e}");
            std::process::exit(1);
        }
    }
}
