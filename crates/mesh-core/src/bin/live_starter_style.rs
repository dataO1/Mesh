//! Starter live style: assembles a playable breakbeat style folder from
//! permissively licensed sample banks (the Tidal sample flakes), for testing
//! live mode without hand-made sound design. `scripts/live-starter-style.sh`
//! resolves the bank paths from the Nix store and calls this tool.
//!
//! Every chosen file is copied into `<OUT>/assets`, so the folder is
//! self-contained and can be exported to USB as is.
//!
//! Usage:
//!   live-starter-style --out DIR --breaks DIR --dirt DIR --surge DIR
//!                      [--kicks DIR --snares DIR] [--library DIR] [--force]
//!   live-starter-style --inspect FILE...
//!
//!   --out DIR      style folder to create (must not exist unless --force)
//!   --breaks DIR   break loops, one option each (1, 2 or 4 bars, picked by
//!                  the tempo closest to the style's default)
//!   --dirt DIR     github:tidalcycles/Dirt-Samples (hats, percussion, lead
//!                  one-shots, the sub; the kick/snare fallback)
//!   --surge DIR    Surge XT data folder (`share/surge-xt`): reese and pad
//!                  patches for the bass and extra lanes
//!   --kicks DIR    kick one-shots, paired by sorted name with --snares: pair
//!   --snares DIR   n is kick n + snare n (the shorter list repeats)
//!   --library DIR  drum sample library with `Kicks/`, `Snares/`, `Breaks/`,
//!                  `Cymbals/` and `Percussion/` folders (searched
//!                  recursively). Kicks, snares and breaks become the kit
//!                  pairs and breaks unless given explicitly; cymbals and
//!                  percussion are sorted into the hats, ride, shaker,
//!                  percussion and top-loop pools (see "Library pools")
//!   --force        replace an existing style.yaml / assets in DIR
//!   --inspect      print length, rate and pitch estimate of each FILE
//!
//! Drum kit `k` is option `k` of the backbone and break sub-voices, so the
//! kick/snare pairs and the breaks step together when the kit is browsed.
//! The pool sub-voices have one option each; the lane seed picks inside it.
//!
//! # Library pools (Q3)
//!
//! Every file of `Cymbals/` and `Percussion/` is sorted into one pool:
//! - **Loop or one-shot**: "loop" in the name, or 1.5 s or longer without
//!   a hit name (crash, ride, cymbal, hat, shaker …), is a loop.
//!   A loop's bar count is the one of 1, 2, 4, 8 whose tempo is closest to
//!   the style's; loops more than 10 % off are skipped. Loops go to `top`.
//! - **One-shots by name**: open (`open`, `ohh`), closed (`closed`, `chh`,
//!   `hat`, `hh`), ride (`ride`), crash (`crash`, `china`, `splash`,
//!   `cymbal`), shaker (`shaker`, `shake`, `tamb`, `maraca`, `cabasa`).
//!   Other `Percussion/` files are percussion.
//! - **One-shots without a name hint** in `Cymbals/`: by decay, the time the
//!   envelope takes to fall 30 dB below its peak: under 0.25 s closed,
//!   under 0.8 s open, longer crash.
//!
//! `sample-overrides.yaml` in the library folder corrects the result, by
//! file name: `"Perc_118.wav": shaker`, `"Loop 3.wav": { kind: top, bars: 2 }`,
//! `"bad.wav": skip`. Each pool keeps at most its block size, spread evenly
//! over the sorted candidates. The full sorting goes to
//! `<out>/sample-classes.txt`, for writing overrides.
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

const USAGE: &str = "usage: live-starter-style --out DIR --breaks DIR --dirt DIR --surge DIR [--kicks DIR --snares DIR] [--library DIR] [--force]\n       \
live-starter-style --inspect FILE...";

/// The style's default tempo, also used to guess a break's bar count.
const DEFAULT_BPM: f64 = 172.0;

fn fail(msg: impl AsRef<str>) -> ! {
    eprintln!("live-starter-style: {}\n{USAGE}", msg.as_ref());
    std::process::exit(2);
}

// ─── Selection ────────────────────────────────────────────────────────

/// Dirt-Samples files of the fallback kick / snare / ghost and of the hats
/// and percussion layers.
const FALLBACK_BACKBONE: [&str; 3] = ["jungle/jungle4kick1.wav", "jungle/jungle4snare1.wav", "jungle/jungle4hit1.wav"];
const HATS: [&str; 2] = ["jungle/jungle4closedhh.wav", "jungle/jungle4openhh.wav"];
const PERC: [&str; 2] = ["jungle/jungle4perc1.wav", "jungle/jungle4perc2.wav"];

/// A pitched one-shot in Dirt-Samples: option name, path, asset name.
struct Pitched {
    option: &'static str,
    path: &'static str,
    asset: &'static str,
}

const BASS: Pitched = Pitched { option: "round-sub", path: "jungbass/018_synthy_round.wav", asset: "bass/round-sub.wav" };
const LEADS: [Pitched; 2] = [
    Pitched { option: "stab", path: "stab/006_stab15.wav", asset: "lead/stab.wav" },
    Pitched { option: "juno", path: "juno/07_juno_release_mid.wav", asset: "lead/juno.wav" },
];

/// A Surge XT patch option: option name, `.fxp` path in the Surge data folder.
type SurgePatch = (&'static str, &'static str);

// MUSICAL: picked by name, audition and replace by ear.
const SURGE_BASSES: [SurgePatch; 5] = [
    ("reese-primitive", "patches_3rdparty/Vospi/Basses/Primitive Reese.fxp"),
    ("reese-rave", "patches_3rdparty/Giana Brotherz/Basses/Rave Detuner.fxp"),
    ("reese-jungler", "patches_3rdparty/Giana Brotherz/Basses/Deep Jungler.fxp"),
    ("reese-neuro", "patches_3rdparty/Altenberg/Basses/Neuro.fxp"),
    ("hoover", "patches_3rdparty/Kuniklo/Basses/Hoover.fxp"),
];
const SURGE_PADS: [SurgePatch; 4] = [
    ("warm-pad", "patches_factory/Pads/MKS-70 Warm Pad.fxp"),
    ("pad-1", "patches_factory/Pads/Pad 1.fxp"),
    ("ghost-pad", "patches_factory/Pads/Ghost Pad.fxp"),
    ("distant", "patches_factory/Pads/Distant.fxp"),
];
const SURGE_ID: &str = "org.surge-synth-team.surge-xt";
// MUSICAL: first drafts, tune by ear. Multiband up/down compression per lane
// (native `multiband`, crates/mesh-core/src/live/fx.rs). Bands: sub | high
// bass | mids | high mids | highs | air.
/// Bass: a smiley shape for full-range (neuro / reese) basses. Sub lightly
/// held, high bass firmly, mids hard and pushed down, the top lifted.
const MB_BASS: &str = "      - id: multiband
        source: native
        settings:
          split_1: 90
          split_2: 250
          split_3: 2000
          split_4: 5000
          split_5: 10000
          band_1_down_threshold: -16
          band_1_down_ratio: 2
          band_1_attack: 20
          band_1_release: 150
          band_2_down_threshold: -24
          band_2_down_ratio: 4
          band_2_up_threshold: -42
          band_2_up_ratio: 1.5
          band_2_attack: 8
          band_2_release: 100
          band_3_down_threshold: -30
          band_3_down_ratio: 8
          band_3_up_threshold: -50
          band_3_up_ratio: 2
          band_3_attack: 4
          band_3_release: 80
          band_3_gain: -3
          band_4_down_threshold: -26
          band_4_down_ratio: 2
          band_4_up_threshold: -48
          band_4_up_ratio: 3
          band_4_attack: 2
          band_4_release: 60
          band_4_gain: 1
          band_5_down_threshold: -26
          band_5_down_ratio: 1.5
          band_5_up_threshold: -52
          band_5_up_ratio: 3
          band_5_attack: 1
          band_5_release: 50
          band_5_gain: 1.5
          band_6_down_threshold: -26
          band_6_down_ratio: 1.5
          band_6_up_threshold: -52
          band_6_up_ratio: 3
          band_6_attack: 1
          band_6_release: 50
          band_6_gain: 1.5
          up_max: 10
          up_gate: -65
";
/// Lead: lighter, same shape.
const MB_LEAD: &str = "      - id: multiband
        source: native
        settings:
          split_1: 120
          split_2: 400
          split_3: 2500
          split_4: 6000
          split_5: 12000
          band_2_down_threshold: -24
          band_2_down_ratio: 2
          band_3_down_threshold: -26
          band_3_down_ratio: 3
          band_3_attack: 5
          band_3_release: 80
          band_3_gain: -1.5
          band_4_up_threshold: -45
          band_4_up_ratio: 2
          band_5_down_threshold: -20
          band_5_down_ratio: 1.5
          band_5_up_threshold: -50
          band_5_up_ratio: 2
          band_6_down_threshold: -20
          band_6_down_ratio: 1.5
          band_6_up_threshold: -50
          band_6_up_ratio: 2
          up_max: 8
          up_gate: -65
";
/// Pads: gentle, mids a little down, the top a little up.
const MB_PAD: &str = "      - id: multiband
        source: native
        settings:
          band_3_down_threshold: -28
          band_3_down_ratio: 2
          band_3_gain: -1
          band_5_up_threshold: -50
          band_5_up_ratio: 1.5
          band_6_up_threshold: -50
          band_6_up_ratio: 1.5
          up_max: 6
          up_gate: -65
";


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

const SLICES_PER_BAR: usize = 16;

/// Bar count (1, 2 or 4) whose tempo is closest to [`DEFAULT_BPM`].
fn break_bars(frames: usize, sample_rate: u32) -> usize {
    let secs = frames as f64 / sample_rate as f64;
    let off = |bars: usize| (bars as f64 * 4.0 * 60.0 / secs / DEFAULT_BPM).ln().abs();
    [1, 2, 4].into_iter().min_by(|&a, &b| off(a).total_cmp(&off(b))).unwrap_or(2)
}

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
    bars: usize,
    bpm: f64,
    fallback: bool,
}

fn slice_break(data: &SampleData) -> Break {
    let bars = break_bars(data.len(), data.sample_rate());
    let slices_n = bars * SLICES_PER_BAR;
    let x = mono(data);
    let sr = data.sample_rate() as f32;
    let n = x.len();
    let low = Biquad::new(sr, 150.0, false).run2(&x);
    let mid = Biquad::new(sr, 2500.0, false).run2(&Biquad::new(sr, 200.0, true).run2(&x));
    let high = Biquad::new(sr, 5000.0, true).run2(&x);
    let starts: Vec<usize> = (0..slices_n).map(|i| (i as f64 * n as f64 / slices_n as f64).round() as usize).collect();
    let win = ((0.05 * sr) as usize).min(n / slices_n);
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
    let bpm = bars as f64 * 4.0 * 60.0 * data.sample_rate() as f64 / n as f64;
    Break { slices, bpm, bars, fallback }
}

// ─── Style assembly ───────────────────────────────────────────────────

// ─── Library pools ────────────────────────────────────────────────────

/// A drum pool a library file is sorted into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PoolKind {
    Closed,
    Open,
    Ride,
    Crash,
    Shaker,
    Perc,
    Top,
    Skip,
}

impl PoolKind {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "closed" => Self::Closed,
            "open" => Self::Open,
            "ride" => Self::Ride,
            "crash" => Self::Crash,
            "shaker" => Self::Shaker,
            "perc" => Self::Perc,
            "top" => Self::Top,
            "skip" => Self::Skip,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::Ride => "ride",
            Self::Crash => "crash",
            Self::Shaker => "shaker",
            Self::Perc => "perc",
            Self::Top => "top",
            Self::Skip => "skip",
        }
    }
}

// MUSICAL: first draft, check the report and override by ear.
/// One-shots at least this long, or named "loop", are loops.
const LOOP_MIN_SECS: f64 = 1.5;
/// Loops whose tempo is further than this from the style's are skipped.
const LOOP_TEMPO_TOLERANCE: f64 = 0.10;
/// Decay (−30 dB) of an unnamed cymbal one-shot: closed below, open below
/// the second value, crash above.
const DECAY_CLOSED_SECS: f64 = 0.25;
const DECAY_OPEN_SECS: f64 = 0.8;
/// Most samples per pool (the drum layout's block sizes: hats 32 shared by
/// closed and open, ride 16 shared by ride and crash).
const POOL_CAPS: [(PoolKind, usize); 7] = [
    (PoolKind::Closed, 24),
    (PoolKind::Open, 8),
    (PoolKind::Ride, 10),
    (PoolKind::Crash, 6),
    (PoolKind::Shaker, 16),
    (PoolKind::Perc, 32),
    (PoolKind::Top, 32),
];

/// One sorted library file.
struct Classified {
    path: PathBuf,
    kind: PoolKind,
    /// Top loops: bars at the style tempo.
    bars: Option<u8>,
    why: String,
}

/// Name tokens: lower case, split on anything but letters and digits.
fn has_word(name: &str, words: &[&str]) -> bool {
    let lower = name.to_ascii_lowercase();
    words.iter().any(|w| lower.contains(w))
}

/// Seconds the envelope takes to fall 30 dB below its peak.
fn decay_secs(data: &SampleData) -> f64 {
    let x = mono(data);
    let peak_at = x.iter().enumerate().fold((0, 0.0f32), |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m });
    let floor = peak_at.1 * 10f32.powf(-30.0 / 20.0);
    // 10 ms windows after the peak; the decay ends at the first window under the floor.
    let win = (data.sample_rate() as usize / 100).max(1);
    let mut end = x.len();
    for start in (peak_at.0..x.len()).step_by(win) {
        let w = &x[start..(start + win).min(x.len())];
        if w.iter().fold(0.0f32, |m, v| m.max(v.abs())) < floor {
            end = start;
            break;
        }
    }
    (end - peak_at.0) as f64 / data.sample_rate() as f64
}

/// Bars (1, 2, 4 or 8) of a loop at the style tempo and its tempo, or `None`
/// when every bar count is more than [`LOOP_TEMPO_TOLERANCE`] off.
fn loop_bars(secs: f64) -> (Option<u8>, f64) {
    let (bars, bpm) = [1u8, 2, 4, 8]
        .iter()
        .map(|&b| (b, b as f64 * 240.0 / secs))
        .min_by(|a, b| (a.1 - DEFAULT_BPM).abs().total_cmp(&(b.1 - DEFAULT_BPM).abs()))
        .unwrap_or((1, 0.0));
    ((bpm - DEFAULT_BPM).abs() / DEFAULT_BPM <= LOOP_TEMPO_TOLERANCE).then_some(bars).map_or((None, bpm), |b| (Some(b), bpm))
}

fn classify(path: &Path, percussion_folder: bool) -> Classified {
    let data = match SampleData::decode(path, 60) {
        Ok(d) => d,
        Err(e) => return Classified { path: path.to_path_buf(), kind: PoolKind::Skip, bars: None, why: format!("cannot decode: {e}") },
    };
    let secs = data.len() as f64 / data.sample_rate() as f64;
    let name = file_name(path);
    let done = |kind, bars, why: String| Classified { path: path.to_path_buf(), kind, bars, why };
    // A long hit that names itself (a crash or ride ringing out) is a one-shot.
    let named_hit = has_word(&name, &["crash", "ride", "cymbal", "china", "splash", "hat", "shaker", "tamb"])
        && !has_word(&name, &["loop", "top"]);
    if has_word(&name, &["loop"]) || (secs >= LOOP_MIN_SECS && !named_hit) {
        return match loop_bars(secs) {
            (Some(bars), bpm) => done(PoolKind::Top, Some(bars), format!("loop {secs:.2} s = {bars} bar(s) at {bpm:.1} BPM")),
            (None, bpm) => done(PoolKind::Skip, None, format!("loop {secs:.2} s, nearest tempo {bpm:.1} BPM is off")),
        };
    }
    let by_name = [
        (PoolKind::Open, &["open", "ohh"][..]),
        (PoolKind::Ride, &["ride"][..]),
        (PoolKind::Crash, &["crash", "china", "splash", "cymbal"][..]),
        (PoolKind::Shaker, &["shaker", "shake", "tamb", "maraca", "cabasa"][..]),
        (PoolKind::Closed, &["closed", "chh", "hat", "hh"][..]),
    ];
    if let Some((kind, words)) = by_name.iter().find(|(_, words)| has_word(&name, words)) {
        let word = words.iter().find(|w| name.to_ascii_lowercase().contains(**w)).unwrap_or(&"");
        return done(*kind, None, format!("one-shot {secs:.2} s, name has '{word}'"));
    }
    if percussion_folder || has_word(&name, &["perc", "tom", "bongo", "conga", "rim", "clap", "block", "cowbell"]) {
        return done(PoolKind::Perc, None, format!("one-shot {secs:.2} s, percussion"));
    }
    let decay = decay_secs(&data);
    let kind = if decay < DECAY_CLOSED_SECS {
        PoolKind::Closed
    } else if decay < DECAY_OPEN_SECS {
        PoolKind::Open
    } else {
        PoolKind::Crash
    };
    done(kind, None, format!("one-shot {secs:.2} s, decay {decay:.2} s"))
}

/// `sample-overrides.yaml`: file name → kind, or `{ kind, bars }`.
fn load_overrides(dir: &Path) -> Vec<(String, PoolKind, Option<u8>)> {
    let path = dir.join("sample-overrides.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let map: serde_yaml::Mapping =
        serde_yaml::from_str(&text).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    map.iter()
        .map(|(k, v)| {
            let file = k.as_str().unwrap_or_else(|| fail(format!("{}: keys are file names", path.display()))).to_string();
            let (kind, bars) = match v {
                serde_yaml::Value::String(kind) => (kind.clone(), None),
                serde_yaml::Value::Mapping(m) => (
                    m.get("kind").and_then(|k| k.as_str()).unwrap_or("").to_string(),
                    m.get("bars").and_then(|b| b.as_u64()).map(|b| b as u8),
                ),
                _ => (String::new(), None),
            };
            let kind = PoolKind::parse(&kind)
                .unwrap_or_else(|| fail(format!("{}: '{file}' has an unknown kind '{kind}'", path.display())));
            (file, kind, bars)
        })
        .collect()
}

/// Sort the library's cymbals and percussion into pools, applying overrides.
fn classify_library(library: &Path, report: &mut String, out: &Path) -> Vec<Classified> {
    let overrides = load_overrides(library);
    let mut all = Vec::new();
    for (folder, percussion) in [("Cymbals", false), ("Percussion", true)] {
        let dir = library.join(folder);
        if !dir.is_dir() {
            let _ = writeln!(report, "Library: no {folder}/ folder");
            continue;
        }
        for path in audio_files(&dir) {
            let mut c = classify(&path, percussion);
            let name = file_name(&path);
            if let Some((_, kind, bars)) = overrides.iter().find(|(f, _, _)| *f == name) {
                c.kind = *kind;
                c.bars = if *kind == PoolKind::Top { bars.or(c.bars).or(Some(1)) } else { None };
                c.why = format!("override ({})", c.why);
            }
            all.push(c);
        }
    }
    let mut listing = String::new();
    for c in &all {
        let _ = writeln!(listing, "{:<7} {:<60} {}", c.kind.name(), c.path.display(), c.why);
    }
    let _ = std::fs::write(out.join("sample-classes.txt"), listing);
    let _ = write!(report, "Library pools ({} files, {} overrides; full list in sample-classes.txt):", all.len(), overrides.len());
    for kind in [PoolKind::Closed, PoolKind::Open, PoolKind::Ride, PoolKind::Crash, PoolKind::Shaker, PoolKind::Perc, PoolKind::Top, PoolKind::Skip] {
        let _ = write!(report, " {} {}", kind.name(), all.iter().filter(|c| c.kind == kind).count());
    }
    let _ = writeln!(report);
    all
}

/// At most `cap` of `items`, spread evenly over the list.
fn spread<T>(items: Vec<T>, cap: usize) -> Vec<T> {
    let n = items.len();
    if n <= cap {
        return items;
    }
    let keep: Vec<usize> = (0..cap).map(|i| i * n / cap).collect();
    items.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, x)| x).collect()
}

/// Pool sub-voices (hats, ride, shaker, perc, top) as style YAML, assets copied.
fn pool_voices(paths: &Paths, classified: Vec<Classified>, report: &mut String) -> String {
    let mut pools: Vec<(PoolKind, Vec<Classified>)> = POOL_CAPS.iter().map(|&(k, _)| (k, Vec::new())).collect();
    for c in classified {
        if let Some((_, v)) = pools.iter_mut().find(|(k, _)| *k == c.kind) {
            v.push(c);
        }
    }
    for (kind, items) in pools.iter_mut() {
        let cap = POOL_CAPS.iter().find(|(k, _)| k == kind).map_or(0, |&(_, c)| c);
        *items = spread(std::mem::take(items), cap);
    }
    let take = |kind: PoolKind, pools: &mut Vec<(PoolKind, Vec<Classified>)>| {
        pools.iter_mut().find(|(k, _)| *k == kind).map(|(_, v)| std::mem::take(v)).unwrap_or_default()
    };
    let entries = |items: Vec<Classified>, dir: &str, tag: Option<&str>| -> (usize, String) {
        let mut y = String::new();
        for (i, c) in items.iter().enumerate() {
            let rel = copy_asset(paths, &c.path, &format!("drums/{dir}/{i:03}-{}", file_name(&c.path)));
            let mut fields = format!("file: {}", q(&rel));
            if let Some(t) = tag {
                let _ = write!(fields, ", tag: {t}");
            }
            if let Some(b) = c.bars {
                let _ = write!(fields, ", bars: {b}");
            }
            let _ = writeln!(y, "                - {{ {fields} }}");
        }
        (items.len(), y)
    };
    let mut y = String::new();
    let voice = |y: &mut String, voice: &str, choke: u8, body: &str, n: usize| {
        if n == 0 {
            return;
        }
        let choke = if choke > 0 { format!("        choke: {choke}
") } else { String::new() };
        let _ = write!(y, "      - voice: {voice}
{choke}        options:
          - name: library
            sampler:
              samples:
{body}");
    };
    let (nc, closed) = entries(take(PoolKind::Closed, &mut pools), "hats", Some("closed"));
    let (no, open) = entries(take(PoolKind::Open, &mut pools), "hats", Some("open"));
    voice(&mut y, "hats", 2, &format!("{closed}{open}"), nc + no);
    let (nr, rides) = entries(take(PoolKind::Ride, &mut pools), "ride", Some("ride"));
    let (ncr, crashes) = entries(take(PoolKind::Crash, &mut pools), "ride", Some("crash"));
    voice(&mut y, "ride", 0, &format!("{rides}{crashes}"), nr + ncr);
    let (ns, shakers) = entries(take(PoolKind::Shaker, &mut pools), "shaker", None);
    voice(&mut y, "shaker", 0, &shakers, ns);
    let (np, perc) = entries(take(PoolKind::Perc, &mut pools), "perc", None);
    voice(&mut y, "perc", 0, &perc, np);
    let (nt, tops) = entries(take(PoolKind::Top, &mut pools), "top", None);
    voice(&mut y, "top", 3, &tops, nt);
    let _ = writeln!(
        report,
        "Pools used: closed {nc} open {no} ride {nr} crash {ncr} shaker {ns} perc {np} top {nt}"
    );
    y
}

struct Paths {
    out: PathBuf,
    breaks: PathBuf,
    dirt: PathBuf,
    surge: PathBuf,
    /// Kick and snare folders (both or neither).
    pairs: Option<(PathBuf, PathBuf)>,
    /// Drum library with Cymbals/ and Percussion/ (pools).
    library: Option<PathBuf>,
}

/// Audio files in `dir` and its subfolders, sorted by path.
fn audio_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| fail(format!("{}: {e}", dir.display())));
        for p in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            if p.is_dir() {
                walk(&p, out);
            } else if p
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "wav" | "flac" | "aif" | "aiff"))
            {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    if files.is_empty() {
        fail(format!("no audio files in {}", dir.display()));
    }
    files
}

/// Single-quoted YAML scalar.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Surge patch options for a lane; the `.fxp` files are copied into the style.
fn surge_options(paths: &Paths, patches: &[SurgePatch], dir: &str) -> String {
    let mut y = String::new();
    for (name, rel) in patches {
        let preset = copy_asset(paths, &paths.surge.join(rel), &format!("{dir}/{name}.fxp"));
        let _ = write!(
            y,
            "      - name: {name}   # Surge XT {rel}\n        clap:\n          plugin_id: {SURGE_ID}\n          preset: {}\n",
            q(&preset)
        );
    }
    y
}

fn decode(path: &Path) -> SampleData {
    SampleData::decode(path, 60).unwrap_or_else(|e| fail(format!("cannot decode {}: {e}", path.display())))
}

/// `files` that decode; the others are reported and left out (a broken file
/// would stop the style from loading).
fn decodable(files: Vec<PathBuf>, report: &mut String) -> Vec<PathBuf> {
    files
        .into_iter()
        .filter(|f| match SampleData::decode(f, 60) {
            Ok(_) => true,
            Err(e) => {
                let _ = writeln!(report, "  skipped {}: cannot decode: {e}", f.display());
                false
            }
        })
        .collect()
}

/// Copy `src` to `<out>/assets/<asset>`; returns the style-relative path.
fn copy_asset(paths: &Paths, src: &Path, asset: &str) -> String {
    let rel = format!("assets/{asset}");
    let dst = paths.out.join(&rel);
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap_or_else(|e| fail(format!("{}: {e}", dst.display())));
    // A repeated pair partner is already there (and may be read-only).
    if dst.exists() {
        return rel;
    }
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
    let src = paths.dirt.join(p.path);
    let data = decode(&src);
    let root = estimate_root(&data).unwrap_or_else(|| fail(format!("no pitch found in {}", src.display())));
    let _ = writeln!(report, "  {:<10} root {:<4} {}  ({})", p.option, note_name(root.midi), root.detail, p.path);
    (copy_asset(paths, &src, p.asset), root.midi)
}

fn build(paths: &Paths) -> String {
    let mut report = String::new();
    let mut y = String::new();

    // Drum kits: kick/snare pairs (or the fallback kit), breaks, one hats and
    // one percussion option.
    let mut backbone = String::new();
    let mut breaks = String::new();
    let one = |file: &str, name: &str| {
        let ext = Path::new(file).extension().and_then(|e| e.to_str()).unwrap_or("wav").to_ascii_lowercase();
        copy_asset(paths, &paths.dirt.join(file), &format!("drums/jungle/{name}.{ext}"))
    };
    match &paths.pairs {
        Some((kick_dir, snare_dir)) => {
            let (kicks, snares) = (decodable(audio_files(kick_dir), &mut report), decodable(audio_files(snare_dir), &mut report));
            let _ = writeln!(report, "Kick/snare pairs ({} kicks, {} snares):", kicks.len(), snares.len());
            for i in 0..kicks.len().max(snares.len()) {
                let (k, s) = (&kicks[i % kicks.len()], &snares[i % snares.len()]);
                let kf = copy_asset(paths, k, &format!("drums/kicks/{}", file_name(k)));
                let sf = copy_asset(paths, s, &format!("drums/snares/{}", file_name(s)));
                let name = format!("pair-{:02}", i + 1);
                let _ = writeln!(report, "  {name}  {} + {}", file_name(k), file_name(s));
                let _ = write!(
                    backbone,
                    "          - name: {name}   # {} + {}\n            sampler:\n              samples:\n                - {{ file: {} }}\n                - {{ file: {} }}\n",
                    stem(k),
                    stem(s),
                    q(&kf),
                    q(&sf)
                );
            }
        }
        None => {
            let [k, s, g] = FALLBACK_BACKBONE;
            let _ = write!(
                backbone,
                "          - name: jungle\n            sampler:\n              samples:\n                - {{ file: {} }}\n                - {{ file: {} }}\n                - {{ file: {} }}\n",
                one(k, "kick"),
                one(s, "snare"),
                one(g, "ghost")
            );
        }
    }
    // Pools: the library's cymbals and percussion, else two Dirt hats and percussion hits.
    let pools = match &paths.library {
        Some(library) => {
            let classified = classify_library(library, &mut report, &paths.out);
            pool_voices(paths, classified, &mut report)
        }
        None => format!(
            "      - voice: hats\n        choke: 2\n        options:\n          - name: jungle\n            sampler:\n              samples:\n                - {{ file: {}, choke: 2 }}\n                - {{ file: {}, choke: 2 }}\n      - voice: perc\n        options:\n          - name: jungle\n            sampler:\n              samples:\n                - {{ file: {} }}\n                - {{ file: {} }}\n",
            one(HATS[0], "hat-closed"),
            one(HATS[1], "hat-open"),
            one(PERC[0], "perc1"),
            one(PERC[1], "perc2")
        ),
    };

    let _ = writeln!(report, "Break slices (16 per bar; K kick, S snare, g ghost, h hat, . other):");
    for src in decodable(audio_files(&paths.breaks), &mut report) {
        let data = decode(&src);
        let b = slice_break(&data);
        let name = stem(&src);
        let rel = copy_asset(paths, &src, &format!("breaks/{}", file_name(&src)));
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
                if i % 4 == 3 && i != b.slices.len() - 1 { format!("{c} ") } else { c.to_string() }
            })
            .collect();
        let _ = writeln!(
            report,
            "  {name:<12} {grid}  ({:.1} BPM at {} bar(s), {} frames @ {} Hz{})",
            b.bpm,
            b.bars,
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
            "          - name: {}\n            sampler:\n              samples:\n                - file: {}\n                  choke: 1\n                  slices:\n",
            q(&name),
            q(&rel)
        );
        for s in &b.slices {
            let _ = writeln!(breaks, "                    - {{ start: {}, type: {} }}", s.start, s.kind.yaml());
        }
    }

    let _ = writeln!(report, "Root keys:");
    let (bass_file, bass_root) = pitched(paths, &BASS, &mut report);
    let leads: Vec<(String, u8)> = LEADS.iter().map(|p| pitched(paths, p, &mut report)).collect();
    let reeses = surge_options(paths, &SURGE_BASSES, "synth/bass");
    let pads = surge_options(paths, &SURGE_PADS, "synth/pad");
    let _ = writeln!(report, "Surge XT patches: {} bass, {} pad", SURGE_BASSES.len(), SURGE_PADS.len());
    let _ = std::fs::copy(paths.breaks.join("LICENSE"), paths.out.join("assets/breaks/LICENSE-breaks.txt"));
    let _ = writeln!(report, "Forms: built-in list ({} forms)", mesh_core::live::form::DEFAULT_FORMS.len());

    let _ = write!(
        y,
        r#"# Starter breakbeat style, generated by scripts/live-starter-style.sh
# (crates/mesh-core/src/bin/live_starter_style.rs). Samples come from
# the break folder, tidalcycles/Dirt-Samples and Surge XT patches (mixed
# sources: keep this folder private). Values are starting points to
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

# Input targets: the level every option feeds into its lane processing.
loudness: {{ drums: -12, bass: -14, lead: -16, extra: -18 }}
# Output targets: the processed lanes (mix balance). MUSICAL: the first
# balance by ear, then drums +1 dB and bass -1.5 dB (by ear).
output_loudness: {{ drums: -5, bass: -9, lead: -14, extra: -24.5 }}

glue:
  role_hpf: {{ lead: 500, extra: 300 }}
  bass_mono_hz: 120
  drum_bus:
    # MUSICAL: threshold for about 3.5 dB peak reduction on this kit (live-render).
    comp: {{ threshold_db: -4 }}
    comp2: {{ threshold_db: -11 }}
    drive: 0.2
  shared_drive: {{ enabled: false }}
  master: {{ threshold_db: -8, ratio: 4, attack_ms: 25, release_ms: 10 }}
  # Drum bus inserts, after its compressors and saturation: Airwindows Pop2
  # (punchy, 76-style character) into ButterComp2 (smooth, 2A-style).
  drum_bus_fx:
    - {{ id: Pop2, source: airwindows }}
    - {{ id: ButterComp2, source: airwindows, settings: {{ compress: 0.3 }} }}

send_fx:
  delay: {{ id: delay, source: native, params: {{ 0: 0.375, 1: 0.4 }} }}
  reverb: {{ id: reverb, source: native }}

lanes:
  # Kit k = option k of the backbone and the break: kick/snare pair k with
  # break k. The pools (hats, ride, shaker, perc, top) keep one option; the
  # lane seed picks samples from them.
  - name: drums
    role: drums
    archetypes: [half_time, two_step, rolling, jungle, hardcore]
    sub_voices:
      - voice: backbone
        options:
{backbone}      - voice: break
        choke: 1
        options:
{breaks}{pools}    fx:
      - {{ id: filter, source: native, settings: {{ filter: 0.0 }} }}
      # Whole kit: low cut 40 Hz, +1.5 dB at 100 Hz, -2 dB around 200-500 Hz.
      - {{ id: eq, source: native, settings: {{ low_cut: 40, bell_1_freq: 100, bell_1_gain: 1.5, bell_1_q: 1.0, bell_2_freq: 320, bell_2_gain: -2, bell_2_q: 0.7 }} }}
    # Break bus, before the lane FX (effects: crates/mesh-core/src/live/fx.rs).
    # MUSICAL: user spec, tune by ear.
    break_fx:
      # Low cut 120 Hz (12 dB/oct), -3 dB at 200 Hz (Q 1), +4.5 dB broad around 5 kHz (Q 0.5).
      - {{ id: eq, source: native, settings: {{ low_cut: 120, bell_1_freq: 200, bell_1_gain: -3, bell_1_q: 1.0, bell_2_freq: 5000, bell_2_gain: 4.5, bell_2_q: 0.5 }} }}
      # 4:1 hard knee, 50 ms attack / 50 ms release, threshold -18 dB:
      # lets the attack through and pulls the body down.
      - {{ id: compressor, source: native, settings: {{ threshold: -18, ratio: 4, attack: 50, release: 50, knee: 0, makeup: 0 }} }}
      # Transient shaper: +4 dB attack, sustain unchanged.
      - {{ id: transient, source: native, settings: {{ attack: 4, sustain: 0 }} }}
    controls:
      - {{ label: style, target: {{ type: generator, param: style }}, default: 0.4 }}
      - {{ label: dens, target: {{ type: generator, param: density }}, default: 0.5 }}
      - {{ label: mutate, target: {{ type: generator, param: mutate }}, default: 0.0 }}
      - {{ label: verb, target: {{ type: send, bus: reverb }}, min: 0.0, max: 0.6, default: 0.0 }}
    generator:
      family: procedural
      name: drums
      # One density brings the voices in (Q5): hats, ghosts and percussion
      # from the bottom, the shaker from 0.35, the ride from 0.5, the top
      # loop from 0.6. MUSICAL: amounts and entry points to tune by ear.
      params:
        {{ style: 0.4, density: 0.5, swing: 0.0, overlay_ride: 0.6, overlay_shaker: 0.5, overlay_top: 0.8,
          enter_shaker: 0.35, enter_ride: 0.5, enter_top: 0.6 }}

  - name: bass
    role: bass
    default_option: 0
    options:
      - name: {bass_opt}
        sampler:
          samples:
            - {{ file: {bass_file}, root_key: {bass_root}, choke: 1 }}   # {bass_note}, measured
      # Surge XT reese patches. Need the CLAP bundle discoverable (~/.clap).
{reeses}    # Multiband first, then the drive (the glue adds compression and saturation).
    fx:
{mb_bass}      - {{ id: drive, source: native, settings: {{ drive: 7 }}, dry_wet: 0.8 }}
    controls:
      - {{ label: cx, target: {{ type: generator, param: complexity }}, default: 0.4 }}
      - {{ label: dens, target: {{ type: generator, param: density }}, default: 0.5 }}
      - {{ label: spread, target: {{ type: generator, param: spread }}, default: 0.3 }}
      # Fader B: note length (legato and glide at the top, Q13).
      - {{ label: len, target: {{ type: generator, param: gate }}, default: 0.6 }}
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
    fx:
{mb_lead}    controls:
      - {{ label: spread, target: {{ type: generator, param: spread }}, default: 0.4 }}
      - {{ label: dens, target: {{ type: generator, param: density }}, default: 0.3 }}
      - {{ label: cutoff, target: {{ type: instrument, param: 0 }}, default: 1.0 }}
      # Fader B: note length (legato and glide at the top, Q13).
      - {{ label: len, target: {{ type: generator, param: gate }}, default: 0.5 }}
    # Shift + fader B: the delay send that fader B held before.
    shift_controls:
      - null
      - null
      - null
      - {{ label: delay, target: {{ type: send, bus: delay }}, min: 0.0, max: 0.7, curve: s, default: 0.29 }}
    generator:
      family: procedural
      name: lead
      params: {{ complexity: 0.2, density: 0.3 }}
    sends: {{ delay: 0.2, reverb: 0.1 }}

  - name: extra
    role: extra
    options:
      # Surge XT pad patches.
{pads}    fx:
{mb_pad}    controls:
      - {{ label: spread, target: {{ type: generator, param: spread }}, default: 0.2 }}
      - {{ label: dens, target: {{ type: generator, param: density }}, default: 0.3 }}
      # Knob B: reverb send. Fader B: note length (Q13).
      - {{ label: verb, target: {{ type: send, bus: reverb }}, min: 0.0, max: 0.8 }}
      - {{ label: len, target: {{ type: generator, param: gate }}, default: 1.0 }}
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
        mb_bass = MB_BASS,
        mb_lead = MB_LEAD,
        mb_pad = MB_PAD,
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
    let (mut out, mut breaks, mut dirt, mut surge, mut force) = (None, None, None, None, false);
    let (mut kicks, mut snares, mut library) = (None, None, None);
    let mut inspect_files = None::<Vec<PathBuf>>;
    while let Some(a) = args.next() {
        let mut val = || args.next().map(PathBuf::from).unwrap_or_else(|| fail(format!("{a} needs a value")));
        match a.as_str() {
            "--out" => out = Some(val()),
            "--breaks" => breaks = Some(val()),
            "--dirt" => dirt = Some(val()),
            "--surge" => surge = Some(val()),
            "--kicks" => kicks = Some(val()),
            "--snares" => snares = Some(val()),
            "--library" => library = Some(val()),
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
    // The library fills in kicks, snares and breaks that were not given.
    if let Some(lib) = &library {
        let sub = |name: &str| Some(lib.join(name)).filter(|p| p.is_dir());
        if kicks.is_none() && snares.is_none() {
            (kicks, snares) = (sub("Kicks"), sub("Snares"));
        }
        if breaks.is_none() {
            breaks = sub("Breaks");
        }
    }
    let need = |p: Option<PathBuf>, what: &str| p.unwrap_or_else(|| fail(format!("missing {what}")));
    let paths = Paths {
        library,
        out: need(out, "--out"),
        breaks: need(breaks, "--breaks"),
        dirt: need(dirt, "--dirt"),
        surge: need(surge, "--surge"),
        pairs: match (kicks, snares) {
            (Some(k), Some(s)) => Some((k, s)),
            (None, None) => None,
            _ => fail("--kicks and --snares go together"),
        },
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
