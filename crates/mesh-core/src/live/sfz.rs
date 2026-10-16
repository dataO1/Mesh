//! Sample instruments for sound packs (docs/plans/2026-10-16-live-sound-packs.md):
//! the [`Instrument`] model, read from an SFZ file here or from a SoundFont 2
//! file ([`super::sf2`]).
//!
//! A subset of SFZ v1/v2 that maps onto the native sampler:
//!
//! - headers `<control> <global> <master> <group> <region>` with SFZ
//!   inheritance (a region sees its group's, master's and global's opcodes),
//!   `default_path`, `#define $NAME value`;
//! - zones: `sample`, `key`, `lokey`, `hikey`, `pitch_keycenter`, `lovel`,
//!   `hivel`; round robin `seq_length` / `seq_position`; `trigger=release`;
//! - playback: `loop_mode`, `loop_start`, `loop_end`, `offset`, `tune`,
//!   `transpose`, `volume`, `pan`;
//! - the instrument's shape: `ampeg_attack/hold/decay/sustain/release`,
//!   `cutoff`, `resonance`, `fil_type`, vibrato (`pitchlfo_depth`,
//!   `pitchlfo_freq`, `pitchlfo_delay`, `pitchlfo_fade`, or v2 `lfoNN_freq` +
//!   `lfoNN_pitch`); the per-note mod envelope (`fileg_*` with `fileg_depth`
//!   on the cutoff, `pitcheg_depth` on the pitch, sharing the times) and mod
//!   LFO (`fillfo_freq/delay/depth`, `amplfo_freq/delay/depth`);
//! - CC macros: `cutoff_onccN` / `cutoff_ccN` (cents), `volume_onccN` (dB),
//!   `pitchlfo_depthccN` / `lfoNN_pitch_onccN` (cents), `offset_onccN`
//!   (frames), `label_ccN`, `set_ccN`.
//!
//! Keys may be MIDI numbers or note names (`c4` = 60, `f#3`, `eb2`).
//! Anything else is kept in [`Instrument::unknown`] so the loader can warn.
//! The sampler holds one envelope, filter and vibrato per lane, so those come
//! from the first region that sets them (usually `<global>` or `<group>`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::sampler::{CcMap, ModEnv, ModLfo, ZoneMod, ZoneTarget};

/// Loop mode of a region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    /// Play once to the end (or until released, with the release envelope).
    #[default]
    NoLoop,
    /// Play to the end and ignore the note-off (drum style).
    OneShot,
    /// Loop between the loop points while the note is held, and after.
    Continuous,
    /// Loop while held, play out to the end after the release.
    Sustain,
}

/// Where a region's audio comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A sample file (SFZ).
    File(PathBuf),
    /// Audio inside the instrument file (SF2): an index into
    /// [`Instrument::embedded`].
    Embedded(usize),
}

/// Audio carried inside an instrument file (SF2 sample data), 16-bit PCM
/// converted to float, one or two channels.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedded {
    pub name: String,
    pub rate: u32,
    pub left: Vec<f32>,
    pub right: Option<Vec<f32>>,
}

/// One playable zone.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub source: Source,
    pub lokey: u8,
    pub hikey: u8,
    pub keycenter: u8,
    pub lovel: u8,
    pub hivel: u8,
    /// Round robin: `seq_position` of `seq_length` (1-based), 1/1 = always.
    pub seq_length: u8,
    pub seq_position: u8,
    /// Plays on the note-off instead of the note-on.
    pub release_trigger: bool,
    pub loop_mode: LoopMode,
    /// Loop points in sample frames (None = the whole sample).
    pub loop_start: Option<u32>,
    pub loop_end: Option<u32>,
    /// Start position in sample frames.
    pub offset: u32,
    /// Fine tune in cents (`tune` + 100 × `transpose`).
    pub tune: i32,
    /// Gain in dB.
    pub volume: f32,
    /// Pan, -100 (left) .. 100 (right).
    pub pan: f32,
    /// The zone's own CC modulators (SF2 zone modulators, SFZ `xfin_*` /
    /// `xfout_*` crossfades). Offsets are in the file's frames.
    pub mods: Vec<ZoneMod>,
}

/// Filter type (`fil_type`), as far as the sampler's lowpass goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterKind {
    /// No filter opcodes: the lane's own filter settings stay.
    #[default]
    None,
    Lowpass1,
    Lowpass2,
    Lowpass4,
    /// A type the sampler cannot play (high-pass, band-pass, ...): ignored.
    Unsupported,
}

/// The instrument-wide shape: envelope, filter, vibrato and CC macros.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shape {
    /// Seconds. None = not set by the SFZ.
    pub attack: Option<f32>,
    pub hold: Option<f32>,
    pub decay: Option<f32>,
    /// 0..1.
    pub sustain: Option<f32>,
    pub release: Option<f32>,
    pub filter: FilterKind,
    /// Cutoff in Hz.
    pub cutoff: Option<f32>,
    /// Resonance in dB.
    pub resonance: Option<f32>,
    /// Vibrato: depth in cents, rate in Hz, delay and fade-in in seconds.
    pub vibrato_depth: f32,
    pub vibrato_rate: f32,
    pub vibrato_delay: f32,
    pub vibrato_fade: f32,
    /// CC number -> modulation at CC = 127.
    pub cutoff_cc: Vec<CcMap>,
    pub volume_cc: Vec<CcMap>,
    pub vibrato_cc: Vec<CcMap>,
    /// Amounts in the file's frames.
    pub offset_cc: Vec<CcMap>,
    /// CC labels (`label_ccN`).
    pub labels: BTreeMap<u8, String>,
    /// Initial CC values (`set_ccN`), 0..127.
    pub cc_init: BTreeMap<u8, u8>,
    /// Per-note mod envelope and LFO.
    pub mod_env: ModEnv,
    pub mod_lfo: ModLfo,
}

/// A sample instrument: zones, its shape and CC macros.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Instrument {
    pub regions: Vec<Region>,
    /// Audio inside the file (SF2); SFZ regions point at files instead.
    pub embedded: Vec<Embedded>,
    pub shape: Shape,
    /// Opcodes outside the subset, with a count, for a load warning.
    pub unknown: BTreeMap<String, usize>,
    /// Problems that dropped a region or a value.
    pub warnings: Vec<String>,
}

impl Instrument {
    /// Read and parse `path`; sample paths resolve against its folder.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        Ok(parse(&text, dir))
    }

    /// Read an `.sfz` or `.sf2` file, by extension.
    pub fn load_any(path: &Path) -> Result<Self, String> {
        match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("sf2") => super::sf2::load(path),
            _ => Self::load(path),
        }
    }

    /// Distinct sample files, in first-use order.
    pub fn sample_files(&self) -> Vec<&Path> {
        let mut out: Vec<&Path> = Vec::new();
        for r in &self.regions {
            if let Source::File(f) = &r.source {
                if !out.contains(&f.as_path()) {
                    out.push(f);
                }
            }
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Control,
    Global,
    Master,
    Group,
    Region,
}

/// Parse SFZ text. `dir` is the folder sample paths are relative to.
pub fn parse(text: &str, dir: &Path) -> Instrument {
    let text = strip_comments(text);
    let mut defines: Vec<(String, String)> = Vec::new();
    let mut lines = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("#define") {
            let mut it = rest.split_whitespace();
            if let (Some(name), Some(value)) = (it.next(), it.next()) {
                defines.push((name.to_string(), value.to_string()));
            }
            continue;
        }
        if t.starts_with('#') {
            continue;
        }
        let mut l = line.to_string();
        // Longest names first, so $A does not eat $AB.
        let mut d = defines.clone();
        d.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
        for (name, value) in &d {
            l = l.replace(name.as_str(), value);
        }
        lines.push(l);
    }
    let joined = lines.join("\n");

    let mut sfz = Instrument::default();
    // Opcodes per level; a region sees all levels above it.
    let mut stack: [Vec<(String, String)>; 5] = Default::default();
    let mut level = Level::Global;
    let mut default_path = String::new();
    let mut flush = |stack: &[Vec<(String, String)>; 5], sfz: &mut Instrument, default_path: &str| {
        let mut ops: Vec<(String, String)> = Vec::new();
        for l in [Level::Control, Level::Global, Level::Master, Level::Group, Level::Region] {
            ops.extend(stack[l as usize].iter().cloned());
        }
        region_from(&ops, dir, default_path, sfz);
    };
    for tok in tokenize(&joined) {
        match tok {
            Token::Header(h) => {
                if level == Level::Region {
                    flush(&stack, &mut sfz, &default_path);
                }
                level = match h.as_str() {
                    "control" => Level::Control,
                    "global" => Level::Global,
                    "master" => Level::Master,
                    "group" => Level::Group,
                    "region" => Level::Region,
                    other => {
                        *sfz.unknown.entry(format!("<{other}>")).or_default() += 1;
                        level
                    }
                };
                // A new header clears its own level and every level below it.
                for l in (level as usize)..5 {
                    stack[l].clear();
                }
            }
            Token::Op(k, v) => {
                if level == Level::Control && k == "default_path" {
                    default_path = v.replace('\\', "/");
                    continue;
                }
                stack[level as usize].push((k, v));
            }
        }
    }
    if level == Level::Region {
        flush(&stack, &mut sfz, &default_path);
    }
    sfz
}

fn region_from(ops: &[(String, String)], dir: &Path, default_path: &str, sfz: &mut Instrument) {
    let mut r = Region {
        source: Source::Embedded(0),
        lokey: 0,
        hikey: 127,
        keycenter: 60,
        lovel: 1,
        hivel: 127,
        seq_length: 1,
        seq_position: 1,
        release_trigger: false,
        loop_mode: LoopMode::NoLoop,
        loop_start: None,
        loop_end: None,
        offset: 0,
        tune: 0,
        volume: 0.0,
        pan: 0.0,
        mods: Vec::new(),
    };
    let mut keycenter_set = false;
    let mut transpose = 0i32;
    let shape = &mut sfz.shape;
    let mut sample = None;
    for (k, v) in ops {
        let num = || v.parse::<f32>().ok();
        let key = || note(v);
        let cc = |prefix: &str| k.strip_prefix(prefix).and_then(|n| n.parse::<u8>().ok()).filter(|&n| n < 128);
        match k.as_str() {
            "sample" => sample = Some(v.replace('\\', "/")),
            "key" => {
                if let Some(n) = key() {
                    r.lokey = n;
                    r.hikey = n;
                    if !keycenter_set {
                        r.keycenter = n;
                    }
                }
            }
            "lokey" => r.lokey = key().unwrap_or(r.lokey),
            "hikey" => r.hikey = key().unwrap_or(r.hikey),
            "pitch_keycenter" => {
                if let Some(n) = key() {
                    r.keycenter = n;
                    keycenter_set = true;
                }
            }
            "lovel" => r.lovel = num().map_or(r.lovel, |x| x.clamp(0.0, 127.0) as u8),
            "hivel" => r.hivel = num().map_or(r.hivel, |x| x.clamp(0.0, 127.0) as u8),
            "seq_length" => r.seq_length = num().map_or(1, |x| x.clamp(1.0, 64.0) as u8),
            "seq_position" => r.seq_position = num().map_or(1, |x| x.clamp(1.0, 64.0) as u8),
            "trigger" => r.release_trigger = v == "release",
            "loop_mode" | "loopmode" => {
                r.loop_mode = match v.as_str() {
                    "one_shot" => LoopMode::OneShot,
                    "loop_continuous" => LoopMode::Continuous,
                    "loop_sustain" => LoopMode::Sustain,
                    _ => LoopMode::NoLoop,
                }
            }
            "loop_start" | "loopstart" => r.loop_start = num().map(|x| x.max(0.0) as u32),
            "loop_end" | "loopend" => r.loop_end = num().map(|x| x.max(0.0) as u32),
            "offset" => r.offset = num().map_or(0, |x| x.max(0.0) as u32),
            "tune" => r.tune = num().map_or(0, |x| x as i32),
            "transpose" => transpose = num().map_or(0, |x| x as i32),
            "volume" => r.volume = num().unwrap_or(0.0),
            "pan" => r.pan = num().map_or(0.0, |x| x.clamp(-100.0, 100.0)),
            "ampeg_attack" => shape.attack = shape.attack.or(num()),
            "ampeg_hold" => shape.hold = shape.hold.or(num()),
            "ampeg_decay" => shape.decay = shape.decay.or(num()),
            "ampeg_sustain" => shape.sustain = shape.sustain.or(num().map(|x| (x / 100.0).clamp(0.0, 1.0))),
            "ampeg_release" => shape.release = shape.release.or(num()),
            "cutoff" => shape.cutoff = shape.cutoff.or(num()),
            "resonance" => shape.resonance = shape.resonance.or(num()),
            "fil_type" | "filtype" => {
                if shape.filter == FilterKind::None {
                    shape.filter = match v.as_str() {
                        "lpf_1p" => FilterKind::Lowpass1,
                        "lpf_2p" | "lpf_2p_sv" => FilterKind::Lowpass2,
                        "lpf_4p" | "lpf_6p" => FilterKind::Lowpass4,
                        _ => FilterKind::Unsupported,
                    }
                }
            }
            "pitchlfo_depth" | "lfo01_pitch" => {
                if shape.vibrato_depth == 0.0 {
                    shape.vibrato_depth = num().unwrap_or(0.0)
                }
            }
            "pitchlfo_freq" | "lfo01_freq" => {
                if shape.vibrato_rate == 0.0 {
                    shape.vibrato_rate = num().unwrap_or(0.0)
                }
            }
            "pitchlfo_delay" | "lfo01_delay" => shape.vibrato_delay = num().unwrap_or(0.0),
            "fileg_delay" | "pitcheg_delay" => shape.mod_env.delay = num().unwrap_or(0.0).max(0.0),
            "fileg_attack" | "pitcheg_attack" => shape.mod_env.attack = num().unwrap_or(0.0).max(0.0),
            "fileg_hold" | "pitcheg_hold" => shape.mod_env.hold = num().unwrap_or(0.0).max(0.0),
            "fileg_decay" | "pitcheg_decay" => shape.mod_env.decay = num().unwrap_or(0.0).max(0.0),
            "fileg_sustain" | "pitcheg_sustain" => shape.mod_env.sustain = num().map_or(1.0, |x| (x / 100.0).clamp(0.0, 1.0)),
            "fileg_release" | "pitcheg_release" => shape.mod_env.release = num().unwrap_or(0.0).max(0.0),
            "fileg_depth" => shape.mod_env.to_cutoff = num().unwrap_or(0.0),
            "pitcheg_depth" => shape.mod_env.to_pitch = num().unwrap_or(0.0),
            "fillfo_freq" | "amplfo_freq" => shape.mod_lfo.rate = num().unwrap_or(0.0).max(0.0),
            "fillfo_delay" | "amplfo_delay" => shape.mod_lfo.delay = num().unwrap_or(0.0).max(0.0),
            "fillfo_depth" => shape.mod_lfo.to_cutoff = num().unwrap_or(0.0),
            "amplfo_depth" => shape.mod_lfo.to_volume_db = num().unwrap_or(0.0),
            "pitchlfo_fade" | "lfo01_fade" => shape.vibrato_fade = num().unwrap_or(0.0),
            _ => {
                // The first mapping of a CC wins (a region sees its parents' opcodes too).
                let add = |map: &mut Vec<CcMap>, n: u8, amount: f32| {
                    if !map.iter().any(|m| m.cc == n) {
                        map.push(CcMap::new(n, amount));
                    }
                };
                // Crossfades: silent at lo .. full at hi (in), full at lo .. silent at hi (out).
                let fade = |r: &mut Region, n: u8, out: bool, set_lo: bool, v: u8| {
                    let i = match r.mods.iter().position(|m| m.src.cc == n && matches!(m.target, ZoneTarget::Fade { out: o, .. } if o == out)) {
                        Some(i) => i,
                        None => {
                            let (lo, hi) = if out { (127, 127) } else { (0, 0) };
                            r.mods.push(ZoneMod { src: CcMap::new(n, 1.0), target: ZoneTarget::Fade { lo, hi, out } });
                            r.mods.len() - 1
                        }
                    };
                    if let ZoneTarget::Fade { lo, hi, .. } = &mut r.mods[i].target {
                        if set_lo { *lo = v } else { *hi = v }
                    }
                };
                let byte = || num().map_or(0, |x| x.clamp(0.0, 127.0) as u8);
                if let Some(n) = cc("cutoff_oncc").or_else(|| cc("cutoff_cc")) {
                    add(&mut shape.cutoff_cc, n, num().unwrap_or(0.0));
                } else if let Some(n) = cc("volume_oncc").or_else(|| cc("gain_oncc")) {
                    add(&mut shape.volume_cc, n, num().unwrap_or(0.0));
                } else if let Some(n) = cc("pitchlfo_depthcc").or_else(|| cc("pitchlfo_depth_oncc")).or_else(|| cc("lfo01_pitch_oncc")) {
                    add(&mut shape.vibrato_cc, n, num().unwrap_or(0.0));
                } else if let Some(n) = cc("offset_oncc").or_else(|| cc("offset_cc")) {
                    add(&mut shape.offset_cc, n, num().unwrap_or(0.0).max(0.0));
                } else if let Some(n) = cc("xfin_locc") {
                    fade(&mut r, n, false, true, byte());
                } else if let Some(n) = cc("xfin_hicc") {
                    fade(&mut r, n, false, false, byte());
                } else if let Some(n) = cc("xfout_locc") {
                    fade(&mut r, n, true, true, byte());
                } else if let Some(n) = cc("xfout_hicc") {
                    fade(&mut r, n, true, false, byte());
                } else if let Some(n) = cc("label_cc") {
                    shape.labels.entry(n).or_insert_with(|| v.clone());
                } else if let Some(n) = cc("set_cc") {
                    shape.cc_init.entry(n).or_insert(num().map_or(0, |x| x.clamp(0.0, 127.0) as u8));
                } else {
                    *sfz.unknown.entry(k.clone()).or_default() += 1;
                }
            }
        }
    }
    r.tune += 100 * transpose;
    if !keycenter_set && r.lokey == r.hikey {
        r.keycenter = r.lokey;
    }
    let Some(sample) = sample else {
        sfz.warnings.push("region without a sample".to_string());
        return;
    };
    if r.lokey > r.hikey || r.lovel > r.hivel {
        sfz.warnings.push(format!("region {sample}: empty key or velocity range"));
        return;
    }
    r.source = Source::File(dir.join(default_path).join(sample));
    sfz.regions.push(r);
}

/// A MIDI note from a number or a name (`c4` = 60, `c#4`, `db4`, `c-1` = 0).
pub fn note(v: &str) -> Option<u8> {
    if let Ok(n) = v.trim().parse::<i32>() {
        return (0..128).contains(&n).then_some(n as u8);
    }
    let s = v.trim().to_ascii_lowercase();
    let mut c = s.chars();
    let base = match c.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest: String = c.collect();
    let (acc, oct) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b') {
        (-1, r)
    } else {
        (0, rest.as_str())
    };
    let octave: i32 = oct.parse().ok()?;
    let n = (octave + 1) * 12 + base + acc;
    (0..128).contains(&n).then_some(n as u8)
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'/') {
            for c in chars.by_ref() {
                if c == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut prev = ' ';
            for c in chars.by_ref() {
                if prev == '*' && c == '/' {
                    break;
                }
                if c == '\n' {
                    out.push('\n');
                }
                prev = c;
            }
        } else {
            out.push(c);
        }
    }
    out
}

enum Token {
    Header(String),
    Op(String, String),
}

/// Headers and `opcode=value` pairs. A value runs to the next opcode, header
/// or line end, so sample paths may hold spaces.
fn tokenize(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim();
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix('<') {
                if let Some(end) = r.find('>') {
                    out.push(Token::Header(r[..end].trim().to_ascii_lowercase()));
                    rest = r[end + 1..].trim_start();
                    continue;
                }
                break;
            }
            let Some(eq) = rest.find('=') else { break };
            let key = rest[..eq].trim().to_string();
            let after = &rest[eq + 1..];
            // The value ends where the next `name=` or `<header>` begins.
            let end = next_boundary(after);
            let value = after[..end].trim().to_string();
            if !key.is_empty() {
                out.push(Token::Op(key, value));
            }
            rest = after[end..].trim_start();
        }
    }
    out
}

/// Byte index in `s` where the next opcode (a word followed by `=`) or header starts.
fn next_boundary(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            return i;
        }
        if b[i].is_ascii_whitespace() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            let start = j;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j > start && j < b.len() && b[j] == b'=' {
                return i;
            }
            if j < b.len() && b[j] == b'<' {
                return i;
            }
        }
        i += 1;
    }
    b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: &str = r#"
// a lead pack
<control> default_path=samples/ label_cc74=Tone
#define $CENTER 41
<global> ampeg_attack=0.002 ampeg_release=0.25 ampeg_sustain=80
         cutoff=2000 cutoff_oncc74=2400 fil_type=lpf_2p resonance=6
         pitchlfo_depth=0 pitchlfo_depthcc1=40 pitchlfo_freq=5.5 pitchlfo_delay=0.3
<group>  seq_length=2 lokey=36 hikey=46 pitch_keycenter=$CENTER
<region> sample=lead F2 a.wav seq_position=1
<region> sample=lead F2 b.wav seq_position=2 volume=-1.5
<group>
<region> sample=lead_F3.wav lokey=f#2 hikey=c4 pitch_keycenter=f3 lovel=64 loop_mode=loop_continuous loop_start=100 loop_end=48000
<region> sample=mudpie.wav key=60 offset=1000 offset_oncc20=2000000 /* stream */ tune=-30 transpose=-12
<region> sample=layer.wav key=72 xfin_locc74=0 xfin_hicc74=100
<region> sample=rel.wav trigger=release amplfo_depth=3 fileg_depth=1200 fileg_decay=0.4 fileg_sustain=25 egfoo=1
"#;

    #[test]
    fn parses_the_subset_with_inheritance() {
        let s = parse(DEMO, Path::new("/pack"));
        assert_eq!(s.regions.len(), 6, "{:?}", s.warnings);
        assert_eq!(s.regions[4].mods, vec![ZoneMod { src: CcMap::new(74, 1.0), target: ZoneTarget::Fade { lo: 0, hi: 100, out: false } }]);
        let a = &s.regions[0];
        assert_eq!(a.source, Source::File(PathBuf::from("/pack/samples/lead F2 a.wav")), "spaces in sample names");
        assert_eq!((a.lokey, a.hikey, a.keycenter, a.seq_length, a.seq_position), (36, 46, 41, 2, 1));
        assert_eq!((s.regions[1].seq_position, s.regions[1].volume), (2, -1.5));
        let c = &s.regions[2];
        assert_eq!((c.lokey, c.hikey, c.keycenter, c.lovel), (42, 60, 53, 64), "note names, group cleared");
        assert_eq!((c.loop_mode, c.loop_start, c.loop_end), (LoopMode::Continuous, Some(100), Some(48000)));
        let m = &s.regions[3];
        assert_eq!((m.lokey, m.hikey, m.keycenter, m.offset, m.tune), (60, 60, 60, 1000, -1230));
        assert!(s.regions[5].release_trigger);
        let sh = &s.shape;
        assert_eq!((sh.attack, sh.release, sh.sustain), (Some(0.002), Some(0.25), Some(0.8)));
        assert_eq!((sh.filter, sh.cutoff, sh.resonance), (FilterKind::Lowpass2, Some(2000.0), Some(6.0)));
        assert_eq!(sh.cutoff_cc, vec![CcMap::new(74, 2400.0)]);
        assert_eq!(sh.vibrato_cc, vec![CcMap::new(1, 40.0)]);
        assert_eq!((sh.vibrato_rate, sh.vibrato_delay), (5.5, 0.3));
        assert_eq!(sh.offset_cc, vec![CcMap::new(20, 2_000_000.0)]);
        assert_eq!(sh.labels.get(&74).map(String::as_str), Some("Tone"));
        assert_eq!(sh.mod_lfo.to_volume_db, 3.0);
        assert_eq!((sh.mod_env.to_cutoff, sh.mod_env.decay, sh.mod_env.sustain), (1200.0, 0.4, 0.25));
        assert_eq!(s.unknown.get("egfoo"), Some(&1));
    }

    #[test]
    fn note_names() {
        assert_eq!(note("c4"), Some(60));
        assert_eq!(note("C#4"), Some(61));
        assert_eq!(note("eb2"), Some(39));
        assert_eq!(note("c-1"), Some(0));
        assert_eq!(note("41"), Some(41));
        assert_eq!(note("h3"), None);
    }

    #[test]
    fn region_without_sample_warns() {
        let s = parse("<region> key=60", Path::new("."));
        assert!(s.regions.is_empty());
        assert_eq!(s.warnings.len(), 1);
    }
}
