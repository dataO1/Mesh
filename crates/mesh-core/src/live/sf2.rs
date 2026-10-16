//! SoundFont 2 (`.sf2`) files into the sample [`Instrument`] model, so a
//! sound pack can carry the file Polyphone saves (SoundFont 2.04 spec).
//!
//! Read: the sample data (`smpl`, 16-bit; stereo pairs joined), the first
//! preset's instruments and their zones: key and velocity ranges, root key
//! (`overridingRootKey` or the sample's), tuning (`coarseTune`, `fineTune`,
//! the sample's pitch correction), loop points and `sampleModes`, start and
//! end offsets, `initialAttenuation`, `pan`, the volume envelope
//! (`attack/hold/decay/sustain/releaseVolEnv`), the filter
//! (`initialFilterFc`, `initialFilterQ`) and the vibrato LFO
//! (`vibLfoToPitch`, `freqVibLFO`, `delayVibLFO`). Instrument global zones
//! apply to their other zones. Preset-level generators are not read (listed
//! in [`Instrument::unknown`]); set the sound on the instrument.
//!
//! Modulators with a MIDI CC source (instrument and preset level, applied
//! instrument-wide) become the CC macros: CC to `initialFilterFc` (cents),
//! `initialAttenuation` (cB, as dB of gain), `vibLfoToPitch` (cents) and
//! `startAddrsOffset` / `startAddrsCoarseOffset` (frames). Other modulators
//! are listed as unknown.

use std::path::Path;

use super::sampler::{CcCurve, CcMap, ZoneMod, ZoneTarget, MAX_ZONE_MODS};
use super::sfz::{Embedded, FilterKind, Instrument, LoopMode, Region, Source};

/// Read and convert `path`.
pub fn load(path: &Path) -> Result<Instrument, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

struct Chunk<'a> {
    id: [u8; 4],
    data: &'a [u8],
}

fn chunks(mut b: &[u8]) -> Vec<Chunk<'_>> {
    let mut out = Vec::new();
    while b.len() >= 8 {
        let id = [b[0], b[1], b[2], b[3]];
        let len = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
        let end = (8 + len).min(b.len());
        out.push(Chunk { id, data: &b[8..end] });
        b = &b[(end + (len & 1)).min(b.len())..];
    }
    out
}

/// The sub-chunks of a `LIST` chunk of type `kind`.
fn list<'a>(top: &[Chunk<'a>], kind: &[u8; 4]) -> Vec<Chunk<'a>> {
    top.iter()
        .find(|c| &c.id == b"LIST" && c.data.len() >= 4 && &c.data[..4] == kind)
        .map(|c| chunks(&c.data[4..]))
        .unwrap_or_default()
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn i16le(b: &[u8], i: usize) -> i16 {
    i16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

#[derive(Clone, Copy)]
struct SampleHeader {
    start: u32,
    end: u32,
    loop_start: u32,
    loop_end: u32,
    rate: u32,
    pitch: u8,
    correction: i8,
    link: u16,
    kind: u16,
}

/// Generator operators read (SoundFont 2.04 section 8.1.2).
mod gen {
    pub const START_OFFSET: u16 = 0;
    pub const END_OFFSET: u16 = 1;
    pub const LOOP_START_OFFSET: u16 = 2;
    pub const LOOP_END_OFFSET: u16 = 3;
    pub const START_COARSE: u16 = 4;
    pub const MOD_LFO_TO_PITCH: u16 = 5;
    pub const VIB_LFO_TO_PITCH: u16 = 6;
    pub const MOD_ENV_TO_PITCH: u16 = 7;
    pub const FILTER_FC: u16 = 8;
    pub const FILTER_Q: u16 = 9;
    pub const MOD_LFO_TO_FILTER_FC: u16 = 10;
    pub const MOD_ENV_TO_FILTER_FC: u16 = 11;
    pub const MOD_LFO_TO_VOLUME: u16 = 13;
    pub const DELAY_MOD_LFO: u16 = 21;
    pub const FREQ_MOD_LFO: u16 = 22;
    pub const DELAY_MOD_ENV: u16 = 25;
    pub const ATTACK_MOD_ENV: u16 = 26;
    pub const HOLD_MOD_ENV: u16 = 27;
    pub const DECAY_MOD_ENV: u16 = 28;
    pub const SUSTAIN_MOD_ENV: u16 = 29;
    pub const RELEASE_MOD_ENV: u16 = 30;
    pub const END_COARSE: u16 = 12;
    pub const PAN: u16 = 17;
    pub const DELAY_VIB_LFO: u16 = 23;
    pub const FREQ_VIB_LFO: u16 = 24;
    pub const ATTACK_VOL: u16 = 34;
    pub const HOLD_VOL: u16 = 35;
    pub const DECAY_VOL: u16 = 36;
    pub const SUSTAIN_VOL: u16 = 37;
    pub const RELEASE_VOL: u16 = 38;
    pub const INSTRUMENT: u16 = 41;
    pub const KEY_RANGE: u16 = 43;
    pub const VEL_RANGE: u16 = 44;
    pub const LOOP_START_COARSE: u16 = 45;
    pub const ATTENUATION: u16 = 48;
    pub const LOOP_END_COARSE: u16 = 50;
    pub const COARSE_TUNE: u16 = 51;
    pub const FINE_TUNE: u16 = 52;
    pub const SAMPLE_ID: u16 = 53;
    pub const SAMPLE_MODES: u16 = 54;
    pub const ROOT_KEY: u16 = 58;
    /// Generators this reader understands (others are counted as unknown).
    pub const KNOWN: &[u16] = &[
        START_OFFSET, END_OFFSET, LOOP_START_OFFSET, LOOP_END_OFFSET, START_COARSE, VIB_LFO_TO_PITCH, FILTER_FC,
        FILTER_Q, MOD_LFO_TO_PITCH, MOD_ENV_TO_PITCH, MOD_LFO_TO_FILTER_FC, MOD_ENV_TO_FILTER_FC, MOD_LFO_TO_VOLUME,
        DELAY_MOD_LFO, FREQ_MOD_LFO, DELAY_MOD_ENV, ATTACK_MOD_ENV, HOLD_MOD_ENV, DECAY_MOD_ENV, SUSTAIN_MOD_ENV,
        RELEASE_MOD_ENV, END_COARSE, PAN, DELAY_VIB_LFO, FREQ_VIB_LFO, ATTACK_VOL, HOLD_VOL, DECAY_VOL, SUSTAIN_VOL,
        RELEASE_VOL, INSTRUMENT, KEY_RANGE, VEL_RANGE, LOOP_START_COARSE, ATTENUATION, LOOP_END_COARSE, COARSE_TUNE,
        FINE_TUNE, SAMPLE_ID, SAMPLE_MODES, ROOT_KEY,
    ];
}

/// One zone's generators: operator -> raw 16-bit amount.
type Gens = Vec<(u16, [u8; 2])>;

fn find(g: &Gens, op: u16) -> Option<[u8; 2]> {
    g.iter().rev().find(|(o, _)| *o == op).map(|(_, a)| *a)
}
fn short(g: &Gens, op: u16) -> Option<i16> {
    find(g, op).map(i16::from_le_bytes)
}
fn range(g: &Gens, op: u16) -> Option<(u8, u8)> {
    find(g, op).map(|a| (a[0], a[1]))
}

/// Zones (generator lists) of a `*bag` / `*gen` pair for items `first..last`.
fn zones(bag: &[u8], gens: &[u8], first: usize, last: usize) -> Vec<Gens> {
    let mut out = Vec::new();
    for z in first..last {
        if (z + 1) * 4 + 2 > bag.len() {
            break;
        }
        let (g0, g1) = (u16le(bag, z * 4) as usize, u16le(bag, (z + 1) * 4) as usize);
        let mut list = Vec::new();
        for gi in g0..g1 {
            if gi * 4 + 4 > gens.len() {
                break;
            }
            list.push((u16le(gens, gi * 4), [gens[gi * 4 + 2], gens[gi * 4 + 3]]));
        }
        out.push(list);
    }
    out
}

/// Seconds from SF2 timecents (-12000 ≈ 1 ms is the spec's "instant").
fn timecents(tc: i16) -> f32 {
    if tc <= -12000 { 0.0 } else { 2f32.powf(tc as f32 / 1200.0) }
}

/// A modulator record as a CC mapping, if this reader can play it: a MIDI CC
/// source (unipolar, forward or reversed, any curve), no amount source.
fn cc_modulator(b: &[u8]) -> Result<(CcMap, u16), String> {
    let (src, dest, amount, amt_src) = (u16le(b, 0), u16le(b, 2), i16le(b, 4), u16le(b, 6));
    let cc = (src & 0x7f) as u8;
    if src & 0x80 == 0 || src & 0x0200 != 0 || amt_src != 0 || amount == 0 {
        return Err(format!("modulator {src:#06x} -> {dest}"));
    }
    let curve = match src >> 10 {
        0 => CcCurve::Linear,
        1 => CcCurve::Concave,
        2 => CcCurve::Convex,
        _ => CcCurve::Switch,
    };
    let map = CcMap { cc, amount: amount as f32, reversed: src & 0x0100 != 0, curve };
    Ok((map, dest))
}

/// The records of a modulator list (the last is the terminal one).
fn mod_records(list: &[u8]) -> impl Iterator<Item = &[u8]> {
    let n = (list.len() / 10).saturating_sub(1);
    (0..n).map(move |i| &list[i * 10..i * 10 + 10])
}

/// Instrument-wide CC modulators (preset level and the instrument's global
/// zone) into the shape's CC maps.
fn modulators<'a>(records: impl Iterator<Item = &'a [u8]>, out: &mut Instrument) {
    for b in records {
        let (map, dest) = match cc_modulator(b) {
            Ok(m) => m,
            Err(what) => {
                *out.unknown.entry(what).or_default() += 1;
                continue;
            }
        };
        let sh = &mut out.shape;
        match dest {
            gen::FILTER_FC => sh.cutoff_cc.push(map),
            gen::ATTENUATION => sh.volume_cc.push(CcMap { amount: -map.amount * 0.1, ..map }),
            gen::VIB_LFO_TO_PITCH => sh.vibrato_cc.push(map),
            gen::START_OFFSET if map.amount > 0.0 => sh.offset_cc.push(map),
            // Coarse offset: 32768 frames per unit (reaches into a long mudpie).
            gen::START_COARSE if map.amount > 0.0 => sh.offset_cc.push(CcMap { amount: map.amount * 32768.0, ..map }),
            _ => *out.unknown.entry(format!("modulator CC{} -> {dest}", map.cc)).or_default() += 1,
        }
    }
}

/// A zone's own CC modulators (a crossfade layer: attenuation; filter;
/// start).
fn zone_modulators<'a>(records: impl Iterator<Item = &'a [u8]>, out: &mut Instrument) -> Vec<ZoneMod> {
    let mut mods = Vec::new();
    for b in records {
        let (map, dest) = match cc_modulator(b) {
            Ok(m) => m,
            Err(what) => {
                *out.unknown.entry(what).or_default() += 1;
                continue;
            }
        };
        let m = match dest {
            gen::ATTENUATION => ZoneMod { src: CcMap { amount: -map.amount * 0.1, ..map }, target: ZoneTarget::Volume },
            gen::FILTER_FC => ZoneMod { src: map, target: ZoneTarget::Cutoff },
            gen::START_OFFSET if map.amount > 0.0 => ZoneMod { src: map, target: ZoneTarget::Offset },
            gen::START_COARSE if map.amount > 0.0 => {
                ZoneMod { src: CcMap { amount: map.amount * 32768.0, ..map }, target: ZoneTarget::Offset }
            }
            _ => {
                *out.unknown.entry(format!("zone modulator CC{} -> {dest}", map.cc)).or_default() += 1;
                continue;
            }
        };
        if mods.len() < MAX_ZONE_MODS {
            mods.push(m);
        } else {
            out.warnings.push(format!("more than {MAX_ZONE_MODS} modulators on a zone; the rest are skipped"));
        }
    }
    mods
}

/// Each zone's modulator records (`ibag` modulator indices into `imod`).
fn zone_mod_lists<'a>(bag: &[u8], mods: &'a [u8], first: usize, last: usize) -> Vec<Vec<&'a [u8]>> {
    (first..last)
        .map(|z| {
            if (z + 1) * 4 + 4 > bag.len() {
                return Vec::new();
            }
            let (m0, m1) = (u16le(bag, z * 4 + 2) as usize, u16le(bag, (z + 1) * 4 + 2) as usize);
            (m0..m1).filter(|i| i * 10 + 10 <= mods.len()).map(|i| &mods[i * 10..i * 10 + 10]).collect()
        })
        .collect()
}

/// Hz from SF2 absolute cents.
fn abs_cents_hz(c: i16) -> f32 {
    8.176 * 2f32.powf(c as f32 / 1200.0)
}

/// Parse a SoundFont 2 file's bytes.
pub fn parse(bytes: &[u8]) -> Result<Instrument, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"sfbk" {
        return Err("not a SoundFont 2 file".to_string());
    }
    let top = chunks(&bytes[12..]);
    let sdta = list(&top, b"sdta");
    let pdta = list(&top, b"pdta");
    let get = |id: &[u8; 4]| pdta.iter().find(|c| &c.id == id).map(|c| c.data).unwrap_or(&[]);
    let smpl = sdta.iter().find(|c| &c.id == b"smpl").map(|c| c.data).ok_or("no sample data (smpl)")?;
    let (phdr, pbag, pgen, inst, ibag, igen, shdr) =
        (get(b"phdr"), get(b"pbag"), get(b"pgen"), get(b"inst"), get(b"ibag"), get(b"igen"), get(b"shdr"));
    if phdr.len() < 76 || inst.len() < 44 || shdr.len() < 92 {
        return Err("no preset, instrument or sample headers".to_string());
    }

    let headers: Vec<SampleHeader> = (0..shdr.len() / 46 - 1)
        .map(|i| {
            let b = &shdr[i * 46..];
            SampleHeader {
                start: u32le(b, 20),
                end: u32le(b, 24),
                loop_start: u32le(b, 28),
                loop_end: u32le(b, 32),
                rate: u32le(b, 36),
                pitch: b[40],
                correction: b[41] as i8,
                link: u16le(b, 42),
                kind: u16le(b, 44),
            }
        })
        .collect();
    let pcm = |h: &SampleHeader| -> Vec<f32> {
        let (s, e) = (h.start as usize, (h.end as usize).min(smpl.len() / 2));
        (s..e.max(s)).map(|i| i16le(smpl, i * 2) as f32 / 32768.0).collect()
    };

    let mut out = Instrument::default();
    // The first preset's instruments.
    let p_first = u16le(phdr, 24) as usize;
    let p_last = u16le(phdr, 38 + 24) as usize;
    let mut instruments = Vec::new();
    for z in zones(pbag, pgen, p_first, p_last) {
        if let Some(a) = find(&z, gen::INSTRUMENT) {
            instruments.push(u16::from_le_bytes(a) as usize);
        }
        for (op, _) in &z {
            if *op != gen::INSTRUMENT && *op != gen::KEY_RANGE && *op != gen::VEL_RANGE {
                *out.unknown.entry(format!("preset generator {op}")).or_default() += 1;
            }
        }
    }
    // Preset-level modulators apply to the whole instrument.
    modulators(mod_records(get(b"pmod")), &mut out);
    let imod = get(b"imod");

    // Embedded audio per sample header, stereo pairs joined on the left one.
    let mut embedded_of: Vec<Option<usize>> = vec![None; headers.len()];
    let n_inst = inst.len() / 22 - 1;
    for &ii in &instruments {
        if ii >= n_inst {
            continue;
        }
        let (z0, z1) = (u16le(inst, ii * 22 + 20) as usize, u16le(inst, (ii + 1) * 22 + 20) as usize);
        let izones = zones(ibag, igen, z0, z1);
        let zmods = zone_mod_lists(ibag, imod, z0, z1);
        let has_global = izones.first().is_some_and(|z| find(z, gen::SAMPLE_ID).is_none());
        let global: Gens = if has_global { izones[0].clone() } else { Vec::new() };
        // The global zone's modulators shape the whole instrument; a sample
        // zone's own modulators stay with that zone (layers, crossfades).
        if has_global {
            modulators(zmods[0].iter().copied(), &mut out);
        }
        for (zi, z) in izones.iter().enumerate().filter(|(_, z)| find(z, gen::SAMPLE_ID).is_some()) {
            let mut g = global.clone();
            g.extend(z.iter().cloned());
            for (op, _) in &g {
                if !gen::KNOWN.contains(op) {
                    *out.unknown.entry(format!("generator {op}")).or_default() += 1;
                }
            }
            let sid = u16::from_le_bytes(find(&g, gen::SAMPLE_ID).unwrap_or([0, 0])) as usize;
            let Some(h) = headers.get(sid).copied() else { continue };
            // A right channel linked to a left one plays through the left zone.
            let right = h.kind & 2 != 0;
            if right && headers.get(h.link as usize).is_some_and(|l| l.kind & 4 != 0) {
                continue;
            }
            let idx = match embedded_of[sid] {
                Some(i) => i,
                None => {
                    let left = pcm(&h);
                    let r = (h.kind & 4 != 0).then(|| headers.get(h.link as usize)).flatten().map(&pcm);
                    out.embedded.push(Embedded { name: format!("sample {sid}"), rate: h.rate, left, right: r });
                    embedded_of[sid] = Some(out.embedded.len() - 1);
                    out.embedded.len() - 1
                }
            };
            let len = h.end.saturating_sub(h.start);
            let off = |fine: u16, coarse: u16| short(&g, fine).unwrap_or(0) as i64 + 32768 * short(&g, coarse).unwrap_or(0) as i64;
            let start_off = off(gen::START_OFFSET, gen::START_COARSE).max(0) as u32;
            let _end_off = off(gen::END_OFFSET, gen::END_COARSE);
            let ls = (h.loop_start as i64 - h.start as i64 + off(gen::LOOP_START_OFFSET, gen::LOOP_START_COARSE)).clamp(0, len as i64) as u32;
            let le = (h.loop_end as i64 - h.start as i64 + off(gen::LOOP_END_OFFSET, gen::LOOP_END_COARSE)).clamp(0, len as i64) as u32;
            let modes = short(&g, gen::SAMPLE_MODES).unwrap_or(0) & 3;
            let (lokey, hikey) = range(&g, gen::KEY_RANGE).unwrap_or((0, 127));
            let (lovel, hivel) = range(&g, gen::VEL_RANGE).unwrap_or((0, 127));
            let root = short(&g, gen::ROOT_KEY).filter(|k| (0..128).contains(k)).map_or(h.pitch.min(127), |k| k as u8);
            let tune = 100 * short(&g, gen::COARSE_TUNE).unwrap_or(0) as i32 + short(&g, gen::FINE_TUNE).unwrap_or(0) as i32
                + h.correction as i32;
            let mods = zone_modulators(zmods.get(zi).into_iter().flatten().copied(), &mut out);
            out.regions.push(Region {
                source: Source::Embedded(idx),
                lokey,
                hikey,
                keycenter: root,
                lovel,
                hivel,
                seq_length: 1,
                seq_position: 1,
                release_trigger: false,
                loop_mode: match modes {
                    1 => LoopMode::Continuous,
                    3 => LoopMode::Sustain,
                    _ => LoopMode::NoLoop,
                },
                loop_start: (modes & 1 == 1 && le > ls).then_some(ls),
                loop_end: (modes & 1 == 1 && le > ls).then_some(le),
                offset: start_off,
                tune,
                // Attenuation in centibels; SF2 players commonly apply 0.4 of it.
                volume: -(short(&g, gen::ATTENUATION).unwrap_or(0).max(0) as f32) * 0.04,
                // A joined stereo pair is already stereo: the zones' hard
                // left / right pans only placed the two mono halves.
                pan: if h.kind & 4 != 0 { 0.0 } else { short(&g, gen::PAN).unwrap_or(0).clamp(-500, 500) as f32 / 5.0 },
                mods,
            });
            // The instrument shape: the first zone that sets it.
            let sh = &mut out.shape;
            let t = |op| short(&g, op).map(timecents);
            sh.attack = sh.attack.or(t(gen::ATTACK_VOL));
            sh.hold = sh.hold.or(t(gen::HOLD_VOL));
            sh.decay = sh.decay.or(t(gen::DECAY_VOL));
            sh.release = sh.release.or(t(gen::RELEASE_VOL));
            if sh.sustain.is_none() {
                sh.sustain = short(&g, gen::SUSTAIN_VOL).map(|cb| 10f32.powf(-(cb.max(0) as f32) / 200.0));
            }
            if sh.cutoff.is_none() {
                if let Some(fc) = short(&g, gen::FILTER_FC).filter(|&c| c < 13500) {
                    sh.cutoff = Some(abs_cents_hz(fc));
                    sh.filter = FilterKind::Lowpass2;
                    sh.resonance = short(&g, gen::FILTER_Q).map(|q| q.max(0) as f32 / 10.0);
                }
            }
            // Mod envelope and LFO: the first zone that gives them a target.
            let (env_pitch, env_fc) = (short(&g, gen::MOD_ENV_TO_PITCH), short(&g, gen::MOD_ENV_TO_FILTER_FC));
            if !sh.mod_env.is_on() && (env_pitch.is_some() || env_fc.is_some()) {
                let e = &mut sh.mod_env;
                e.to_pitch = env_pitch.unwrap_or(0) as f32;
                e.to_cutoff = env_fc.unwrap_or(0) as f32;
                let tc = |op| short(&g, op).map_or(0.0, timecents);
                e.delay = tc(gen::DELAY_MOD_ENV);
                e.attack = tc(gen::ATTACK_MOD_ENV);
                e.hold = tc(gen::HOLD_MOD_ENV);
                e.decay = tc(gen::DECAY_MOD_ENV);
                e.release = tc(gen::RELEASE_MOD_ENV);
                // Sustain is a decrease in 0.1 % units.
                e.sustain = 1.0 - short(&g, gen::SUSTAIN_MOD_ENV).unwrap_or(0).clamp(0, 1000) as f32 / 1000.0;
            }
            let (lfo_pitch, lfo_fc, lfo_vol) =
                (short(&g, gen::MOD_LFO_TO_PITCH), short(&g, gen::MOD_LFO_TO_FILTER_FC), short(&g, gen::MOD_LFO_TO_VOLUME));
            if !sh.mod_lfo.is_on() && (lfo_pitch.is_some() || lfo_fc.is_some() || lfo_vol.is_some()) {
                let l = &mut sh.mod_lfo;
                l.to_pitch = lfo_pitch.unwrap_or(0) as f32;
                l.to_cutoff = lfo_fc.unwrap_or(0) as f32;
                l.to_volume_db = lfo_vol.unwrap_or(0) as f32 / 10.0;
                l.rate = short(&g, gen::FREQ_MOD_LFO).map_or(8.176, abs_cents_hz);
                l.delay = short(&g, gen::DELAY_MOD_LFO).map_or(0.0, timecents);
            }
            if sh.vibrato_depth == 0.0 {
                if let Some(d) = short(&g, gen::VIB_LFO_TO_PITCH).filter(|&d| d != 0) {
                    sh.vibrato_depth = d as f32;
                    sh.vibrato_rate = short(&g, gen::FREQ_VIB_LFO).map_or(8.176, abs_cents_hz);
                    sh.vibrato_delay = short(&g, gen::DELAY_VIB_LFO).map_or(0.0, timecents);
                }
            }
        }
    }
    if out.regions.is_empty() {
        return Err("the first preset has no playable zones".to_string());
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal SoundFont: one 100-frame sample, one instrument with a global
    /// zone (release 1 s) and one zone (keys 36-60, root 48, loop on), one preset.
    pub(crate) fn tiny_sf2() -> Vec<u8> {
        fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut v = id.to_vec();
            v.extend((data.len() as u32).to_le_bytes());
            v.extend(data);
            if data.len() % 2 == 1 {
                v.push(0);
            }
            v
        }
        fn list(kind: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
            let mut d = kind.to_vec();
            for p in parts {
                d.extend(p);
            }
            chunk(b"LIST", &d)
        }
        let mut smpl = Vec::new();
        for i in 0..100i16 {
            smpl.extend((i * 100).to_le_bytes());
        }
        smpl.extend([0u8; 92]); // 46 zero points after the sample (spec)
        let name20 = |s: &str| {
            let mut n = [0u8; 20];
            n[..s.len()].copy_from_slice(s.as_bytes());
            n.to_vec()
        };
        let mut phdr = name20("lead");
        phdr.extend([0u8; 4]); // preset, bank
        phdr.extend(0u16.to_le_bytes()); // bag index
        phdr.extend([0u8; 12]);
        phdr.extend(name20("EOP"));
        phdr.extend([0u8; 4]);
        phdr.extend(1u16.to_le_bytes());
        phdr.extend([0u8; 12]);
        let pbag = [0u16.to_le_bytes(), 0u16.to_le_bytes(), 1u16.to_le_bytes(), 0u16.to_le_bytes()].concat();
        let pgen = [41u16.to_le_bytes(), 0u16.to_le_bytes(), [0, 0], [0, 0]].concat();
        let mut inst = name20("inst");
        inst.extend(0u16.to_le_bytes());
        inst.extend(name20("EOI"));
        inst.extend(2u16.to_le_bytes());
        // Zone 0: global (release 0 timecents = 1 s); zone 1: keys, root, loop, sample.
        let ibag = [0u16, 0, 1, 0, 5, 0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
        let g = |op: u16, a: [u8; 2]| [op.to_le_bytes(), a].concat();
        let igen = [
            g(38, 0i16.to_le_bytes()),
            g(43, [36, 60]),
            g(58, 48i16.to_le_bytes()),
            g(54, 1i16.to_le_bytes()),
            g(53, 0u16.to_le_bytes()),
            g(0, [0, 0]),
        ]
        .concat();
        let mut shdr = name20("s");
        for v in [0u32, 100, 10, 90, 48000] {
            shdr.extend(v.to_le_bytes());
        }
        shdr.extend([60u8, 0]);
        shdr.extend(0u16.to_le_bytes());
        shdr.extend(1u16.to_le_bytes());
        shdr.extend(name20("EOS"));
        shdr.extend([0u8; 26]);
        let sfbk = [
            b"sfbk".to_vec(),
            list(b"INFO", &[chunk(b"ifil", &[2, 0, 4, 0])]),
            list(b"sdta", &[chunk(b"smpl", &smpl)]),
            list(
                b"pdta",
                &[
                    chunk(b"phdr", &phdr),
                    chunk(b"pbag", &pbag),
                    chunk(b"pmod", &[0u8; 10]),
                    chunk(b"pgen", &pgen),
                    chunk(b"inst", &inst),
                    chunk(b"ibag", &ibag),
                    chunk(b"imod", &[0u8; 10]),
                    chunk(b"igen", &igen),
                    chunk(b"shdr", &shdr),
                ],
            ),
        ]
        .concat();
        chunk(b"RIFF", &sfbk)
    }

    #[test]
    fn reads_zones_loops_and_the_envelope() {
        let i = parse(&tiny_sf2()).unwrap();
        assert_eq!(i.regions.len(), 1);
        let r = &i.regions[0];
        assert_eq!((r.lokey, r.hikey, r.keycenter), (36, 60, 48));
        assert_eq!((r.loop_mode, r.loop_start, r.loop_end), (LoopMode::Continuous, Some(10), Some(90)));
        assert_eq!(r.source, Source::Embedded(0));
        assert_eq!(i.embedded[0].left.len(), 100);
        assert_eq!(i.embedded[0].rate, 48000);
        assert!((i.embedded[0].left[1] - 100.0 / 32768.0).abs() < 1e-6);
        assert_eq!(i.shape.release, Some(1.0), "the global zone's release applies");
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse(b"RIFF\0\0\0\0WAVE").is_err());
    }

    #[test]
    fn cc_modulators_become_macros() {
        let rec = |src: u16, dest: u16, amount: i16, amt_src: u16| {
            let mut v = src.to_le_bytes().to_vec();
            v.extend(dest.to_le_bytes());
            v.extend(amount.to_le_bytes());
            v.extend(amt_src.to_le_bytes());
            v.extend(0u16.to_le_bytes());
            v
        };
        let mut list = rec(0x80 | 74, super::gen::FILTER_FC, 2400, 0);
        list.extend(rec(0x80 | 7, super::gen::ATTENUATION, 960, 0));
        list.extend(rec(0x80 | 1, super::gen::VIB_LFO_TO_PITCH, 50, 0));
        list.extend(rec(0x02, super::gen::FILTER_FC, -2400, 0)); // velocity source: not a CC
        // Reversed, concave CC74 -> attenuation (a layer that fades in).
        list.extend(rec(0x80 | 0x0100 | 0x0400 | 74, super::gen::ATTENUATION, 960, 0));
        list.extend([0u8; 10]);
        let mut out = Instrument::default();
        super::modulators(super::mod_records(&list), &mut out);
        assert_eq!(out.shape.cutoff_cc, vec![CcMap::new(74, 2400.0)]);
        assert_eq!(out.shape.volume_cc[0], CcMap::new(7, -96.0));
        assert_eq!(out.shape.vibrato_cc, vec![CcMap::new(1, 50.0)]);
        let fade = out.shape.volume_cc[1];
        assert!(fade.reversed && fade.curve == CcCurve::Concave && fade.amount == -96.0, "{fade:?}");
        assert_eq!(out.unknown.len(), 1);
        // As a zone modulator it stays with the zone.
        let mut z = Instrument::default();
        let mods = super::zone_modulators(super::mod_records(&list), &mut z);
        assert_eq!(mods.len(), 3, "attenuation x2 and the filter; vibrato is instrument-wide only");
        assert!(matches!(mods[2].target, ZoneTarget::Volume) && mods[2].src.reversed);
    }

    /// `MESH_SF2=path cargo test -p mesh-core --lib inspect_sf2 -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn inspect_sf2() {
        let path = std::env::var("MESH_SF2").expect("MESH_SF2");
        let mut i = load(std::path::Path::new(&path)).unwrap();
        for e in &mut i.embedded {
            println!("sample {} rate {} frames {} stereo {}", e.name, e.rate, e.left.len(), e.right.is_some());
            e.left.clear();
            e.right = None;
        }
        println!("{:#?}", i);
    }
}
