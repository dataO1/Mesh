//! Sample data, slot definitions and the per-lane sample bank.
//!
//! Everything here is built off the RT thread (style loader) and handed to the
//! RT side wrapped in `basedrop::Shared`, the same way deck stems are shared.
//! The RT thread only reads it and clones `Shared` handles (an atomic increment).

use std::path::Path;

use basedrop::Shared;

use crate::audio_file::AudioFileError;
use crate::types::{StereoBuffer, StereoSample};

use super::params::{ZoneMod, MAX_ZONE_MODS};

/// Most zones one note plays at once (layers of a sound pack).
pub const MAX_LAYERS: usize = 4;

/// Zero frames stored before and after the audio so every interpolation tap
/// (at most 4 behind and 5 ahead of the read position) stays in bounds without
/// per-tap clamping.
pub(crate) const PAD: usize = 8;

/// Musical type of a break slice (design §9: stored in the style, used by the
/// generator to swap a slice for one of the same type).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SliceType {
    Kick,
    Snare,
    Ghost,
    Hat,
    Perc,
    #[default]
    Other,
}

/// One slice of a sample, in frames at the sample's own rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slice {
    pub start: usize,
    /// Exclusive.
    pub end: usize,
    pub kind: SliceType,
}

/// Decoded stereo audio plus its tuning and optional slice table.
///
/// Audio is stored with [`PAD`] zero frames on both sides. All positions in the
/// public API are frame indices into the unpadded audio.
pub struct SampleData {
    /// Padded audio: `PAD` zeros, `len` frames, `PAD` zeros.
    frames: StereoBuffer,
    len: usize,
    sample_rate: u32,
    /// MIDI key the sample sounds at when played at rate 1.
    pub root_key: u8,
    /// Tuning of the recording relative to `root_key` (+20 = 20 cents sharp).
    pub root_cents: i16,
    pub slices: Vec<Slice>,
}

impl SampleData {
    /// Build from separate channel buffers. `right` may be empty for mono.
    pub fn from_channels(left: &[f32], right: &[f32], sample_rate: u32, root_key: u8) -> Self {
        let len = left.len();
        let right = if right.len() == len { right } else { left };
        let mut frames = StereoBuffer::silence(len + 2 * PAD);
        let dst = frames.as_mut_slice();
        for i in 0..len {
            dst[PAD + i] = StereoSample::new(left[i], right[i]);
        }
        Self { frames, len, sample_rate, root_key, root_cents: 0, slices: Vec::new() }
    }

    /// Build from interleaved samples with 1 or 2 channels (extra channels ignored).
    pub fn from_interleaved(samples: &[f32], channels: usize, sample_rate: u32, root_key: u8) -> Self {
        let channels = channels.max(1);
        let len = samples.len() / channels;
        let mut frames = StereoBuffer::silence(len + 2 * PAD);
        let dst = frames.as_mut_slice();
        for i in 0..len {
            let l = samples[i * channels];
            let r = if channels > 1 { samples[i * channels + 1] } else { l };
            dst[PAD + i] = StereoSample::new(l, r);
        }
        Self { frames, len, sample_rate, root_key, root_cents: 0, slices: Vec::new() }
    }

    pub fn with_root_cents(mut self, cents: i16) -> Self {
        self.root_cents = cents;
        self
    }

    /// Attach a slice table. Slices are clamped to the sample length.
    pub fn with_slices(mut self, mut slices: Vec<Slice>) -> Self {
        for s in &mut slices {
            s.end = s.end.min(self.len);
            s.start = s.start.min(s.end);
        }
        self.slices = slices;
        self
    }

    /// Number of audio frames (unpadded).
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Frame `i` of the unpadded audio.
    pub fn frame(&self, i: usize) -> StereoSample {
        self.frames.as_slice()[PAD + i]
    }

    /// Padded frames, for the interpolators.
    #[inline]
    pub(crate) fn padded(&self) -> &[StereoSample] {
        self.frames.as_slice()
    }

    /// Resample to `target_rate` (off-RT; FFT resampler shared with track
    /// loading). Slice positions are scaled along. No-op if the rate matches.
    pub fn resampled_to(self, target_rate: u32) -> Result<Self, AudioFileError> {
        if self.sample_rate == target_rate || self.len == 0 {
            return Ok(self);
        }
        let src = &self.frames.as_slice()[PAD..PAD + self.len];
        let left: Vec<f32> = src.iter().map(|s| s.left).collect();
        let right: Vec<f32> = src.iter().map(|s| s.right).collect();
        let left = resample_aligned(&left, self.sample_rate, target_rate)?;
        let right = resample_aligned(&right, self.sample_rate, target_rate)?;
        let ratio = target_rate as f64 / self.sample_rate as f64;
        let mut out = Self::from_channels(&left, &right, target_rate, self.root_key)
            .with_root_cents(self.root_cents);
        let slices = self
            .slices
            .iter()
            .map(|s| Slice {
                start: (s.start as f64 * ratio).round() as usize,
                end: (s.end as f64 * ratio).round() as usize,
                kind: s.kind,
            })
            .collect();
        out = out.with_slices(slices);
        Ok(out)
    }

    /// Decode a WAV or FLAC file at its own sample rate. Off-RT only. Used
    /// when slice markers (given in file frames) are attached before
    /// [`Self::resampled_to`] scales them.
    pub fn decode(path: &Path, root_key: u8) -> Result<Self, AudioFileError> {
        let (samples, channels, rate) = decode_file(path)?;
        Ok(Self::from_interleaved(&samples, channels, rate, root_key))
    }

    /// Decode a WAV or FLAC file and resample it to `target_rate`. Off-RT only.
    pub fn load_file(path: &Path, root_key: u8, target_rate: u32) -> Result<Self, AudioFileError> {
        let (samples, channels, rate) = decode_file(path)?;
        Self::from_interleaved(&samples, channels, rate, root_key).resampled_to(target_rate)
    }
}

/// Resample one channel with the FFT resampler used for track loading, with
/// the resampler's delay removed, so frame 0 of the input is frame 0 of the
/// output and the tail is kept. (`audio_file::resample_mono_audio` leaves the
/// delay in, which shifts a sample's onset late by several milliseconds.)
fn resample_aligned(input: &[f32], source_rate: u32, target_rate: u32) -> Result<Vec<f32>, AudioFileError> {
    use rubato::{FftFixedInOut, Resampler};

    let mut resampler = FftFixedInOut::<f32>::new(source_rate as usize, target_rate as usize, 1024, 1)
        .map_err(|e| AudioFileError::IoError(format!("sample resampler init failed: {e}")))?;
    let chunk = resampler.input_frames_max();
    let delay = resampler.output_delay();
    let wanted = (input.len() as f64 * target_rate as f64 / source_rate as f64).ceil() as usize;

    let mut out: Vec<f32> = Vec::with_capacity(wanted + delay + resampler.output_frames_max());
    let mut buf = vec![vec![0.0f32; chunk]];
    let mut pos = 0;
    // Feed zeros past the end until the delayed tail has come out.
    while out.len() < delay + wanted {
        let n = input.len().saturating_sub(pos).min(chunk);
        buf[0][..n].copy_from_slice(&input[pos..pos + n]);
        buf[0][n..].fill(0.0);
        let chunk_out = resampler
            .process(&buf, None)
            .map_err(|e| AudioFileError::IoError(format!("sample resampling failed: {e}")))?;
        out.extend_from_slice(&chunk_out[0]);
        pos += n;
    }
    out.drain(..delay);
    out.truncate(wanted);
    Ok(out)
}

/// Decode any WAV (hound) or other symphonia-supported file to interleaved f32.
fn decode_file(path: &Path) -> Result<(Vec<f32>, usize, u32), AudioFileError> {
    let is_wav = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    if is_wav {
        let reader = hound::WavReader::open(path).map_err(|e| AudioFileError::IoError(e.to_string()))?;
        let spec = reader.spec();
        let samples: Result<Vec<f32>, _> = match spec.sample_format {
            hound::SampleFormat::Float => reader.into_samples::<f32>().collect(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader.into_samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect()
            }
        };
        let samples = samples.map_err(|e| AudioFileError::Corrupted(e.to_string()))?;
        return Ok((samples, spec.channels as usize, spec.sample_rate));
    }

    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| AudioFileError::IoError(e.to_string()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| AudioFileError::InvalidFormat(e.to_string()))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| AudioFileError::InvalidFormat("no audio track".into()))?;
    let track_id = track.id;
    let rate = track.codec_params.sample_rate.unwrap_or(crate::types::SAMPLE_RATE);
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(2);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| AudioFileError::InvalidFormat(e.to_string()))?;

    let mut out = Vec::new();
    let mut buf: Option<SampleBuffer<f32>> = None;
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(AudioFileError::Corrupted(e.to_string())),
        };
        let b = buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, *decoded.spec()));
        if (b.capacity()) < decoded.capacity() * channels {
            *b = SampleBuffer::new(decoded.capacity() as u64, *decoded.spec());
        }
        b.copy_interleaved_ref(decoded);
        out.extend_from_slice(b.samples());
    }
    Ok((out, channels, rate))
}

/// One playable region of a sample, addressed by an event `target`.
#[derive(Clone)]
pub struct SlotDef {
    pub sample: Shared<SampleData>,
    /// Frame range within the sample (`end` exclusive).
    pub start: usize,
    pub end: usize,
    pub kind: SliceType,
    /// Root key / tuning for this slot (defaults to the sample's).
    pub root_key: u8,
    pub root_cents: i16,
    /// Lowest and highest event key this zone answers (multisample key split).
    pub key_lo: u8,
    pub key_hi: u8,
    /// Default lane-local choke group (0 = none). An event's non-zero
    /// `choke` overrides it.
    pub choke: u8,
    /// Static gain (style LUFS trim per sub-voice).
    pub gain: f32,
    /// `false` ignores the event key (drum hits): only `cents` retunes.
    pub pitched: bool,
    /// A long stream of audio (a mudpie): notes start at the event's
    /// `start` position. Other pitched slots start notes at the slot start.
    pub stream: bool,
    /// Velocity range this zone answers (SFZ `lovel` / `hivel`).
    pub vel_lo: u8,
    pub vel_hi: u8,
    /// Round robin: this zone plays as position `seq_pos` (1-based) of
    /// `seq_len` (SFZ `seq_length` / `seq_position`); 1 / 1 = always.
    pub seq_len: u8,
    pub seq_pos: u8,
    /// Loop points in frames of the sample (absolute), looped while the
    /// note is held; `loop_sustain` stops looping on release.
    pub loop_range: Option<(usize, usize)>,
    pub loop_sustain: bool,
    /// Plays to the end, ignoring the note-off (SFZ `one_shot`).
    pub one_shot: bool,
    /// Where notes start, in frames after `start` (SFZ `offset`).
    pub start_offset: usize,
    /// Pan of the zone, -1..1.
    pub pan: f32,
    /// The zone's own CC modulators (crossfades, per-layer volume, cutoff
    /// and start); unused entries are [`ZoneMod::NONE`].
    pub zone_mods: [ZoneMod; MAX_ZONE_MODS],
}

impl SlotDef {
    /// Whole-sample slot.
    pub fn whole(sample: Shared<SampleData>) -> Self {
        let end = sample.len();
        Self::region(sample, 0, end, SliceType::Other)
    }

    /// Slot over the sample's `index`-th slice. `None` if out of range.
    pub fn slice(sample: Shared<SampleData>, index: usize) -> Option<Self> {
        let s = *sample.slices.get(index)?;
        Some(Self::region(sample, s.start, s.end, s.kind))
    }

    fn region(sample: Shared<SampleData>, start: usize, end: usize, kind: SliceType) -> Self {
        let (root_key, root_cents) = (sample.root_key, sample.root_cents);
        let end = end.min(sample.len());
        Self {
            sample,
            start: start.min(end),
            end,
            kind,
            root_key,
            root_cents,
            key_lo: 0,
            key_hi: 127,
            choke: 0,
            gain: 1.0,
            pitched: true,
            stream: false,
            vel_lo: 0,
            vel_hi: 127,
            seq_len: 1,
            seq_pos: 1,
            loop_range: None,
            loop_sustain: false,
            one_shot: false,
            start_offset: 0,
            pan: 0.0,
            zone_mods: [ZoneMod::NONE; MAX_ZONE_MODS],
        }
    }

    /// Mark the slot as a stream (see [`Self::stream`]).
    pub fn streamed(mut self) -> Self {
        self.stream = true;
        self
    }

    pub fn with_choke(mut self, group: u8) -> Self {
        self.choke = group;
        self
    }

    pub fn with_gain(mut self, gain: f32) -> Self {
        self.gain = gain;
        self
    }

    pub fn with_keys(mut self, lo: u8, hi: u8) -> Self {
        self.key_lo = lo;
        self.key_hi = hi;
        self
    }

    pub fn unpitched(mut self) -> Self {
        self.pitched = false;
        self
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// Maps event `target` (0..=255) to one or more key zones, plus per-group
/// mono flags. Built off-RT, shared to the lane as `Shared<SampleBank>`.
///
/// A target is one of:
/// - a one-shot or stem phrase (one whole-sample zone),
/// - a break slice (one slice zone),
/// - a drum sub-voice (any of the above with its own default choke group),
/// - a multisampled instrument (several zones split by key).
pub struct SampleBank {
    targets: Vec<Vec<SlotDef>>,
    mono: [bool; 256],
}

impl Default for SampleBank {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleBank {
    pub fn new() -> Self {
        Self { targets: vec![Vec::new(); 256], mono: [false; 256] }
    }

    /// Replace `target` with a single zone.
    pub fn set(&mut self, target: u8, slot: SlotDef) {
        self.targets[target as usize] = vec![slot];
    }

    /// Add a key zone to `target` (multisampled instrument).
    pub fn add_zone(&mut self, target: u8, slot: SlotDef) {
        self.targets[target as usize].push(slot);
    }

    /// Map every slice of `sample` to consecutive targets from `first`, all in
    /// choke group `choke`. Returns the number of slices mapped.
    pub fn map_slices(&mut self, first: u8, sample: &Shared<SampleData>, choke: u8) -> usize {
        let mut n = 0;
        for i in 0..sample.slices.len() {
            let t = first as usize + i;
            if t > 255 {
                break;
            }
            if let Some(slot) = SlotDef::slice(sample.clone(), i) {
                self.set(t as u8, slot.with_choke(choke));
                n += 1;
            }
        }
        n
    }

    /// Mark a choke group as mono (see [`super::Sampler`] docs).
    pub fn set_mono(&mut self, group: u8, mono: bool) {
        self.mono[group as usize] = mono;
    }

    #[inline]
    pub fn is_mono(&self, group: u8) -> bool {
        group != 0 && self.mono[group as usize]
    }

    /// Zone for `target` at `key`, velocity `vel` and round-robin counter
    /// `rr`: among the zones whose key and velocity ranges hold the note, a
    /// round-robin group plays position `rr % seq_len + 1`. Without a match
    /// it falls back to [`Self::resolve`]. Returns the zone and whether it is
    /// part of a round robin (the caller then advances its counter).
    #[inline]
    pub fn resolve_note(&self, target: u8, key: u8, vel: u8, rr: u32) -> Option<(&SlotDef, bool)> {
        let zones = &self.targets[target as usize];
        let fits = |z: &&SlotDef| (z.key_lo..=z.key_hi).contains(&key) && (z.vel_lo..=z.vel_hi).contains(&vel);
        let mut seq = None;
        let mut plain = None;
        for z in zones.iter().filter(fits) {
            if z.seq_len > 1 {
                if seq.is_none() && (rr % z.seq_len as u32) + 1 == z.seq_pos as u32 {
                    seq = Some(z);
                }
            } else if plain.is_none() {
                plain = Some(z);
            }
        }
        match (seq, plain) {
            (Some(z), _) => Some((z, true)),
            (None, Some(z)) => Some((z, false)),
            (None, None) => self.resolve(target, key).map(|z| (z, false)),
        }
    }

    /// Every zone a note plays (layers): the one [`Self::resolve_note`] picks,
    /// plus the other zones (outside round robins) whose key and velocity
    /// ranges hold the note, up to [`MAX_LAYERS`].
    pub fn resolve_layers(&self, target: u8, key: u8, vel: u8, rr: u32) -> ([Option<&SlotDef>; MAX_LAYERS], bool) {
        let mut out = [None; MAX_LAYERS];
        let Some((first, in_rr)) = self.resolve_note(target, key, vel, rr) else { return (out, false) };
        out[0] = Some(first);
        let mut n = 1;
        for z in &self.targets[target as usize] {
            if n >= MAX_LAYERS {
                break;
            }
            let holds = (z.key_lo..=z.key_hi).contains(&key) && (z.vel_lo..=z.vel_hi).contains(&vel);
            if holds && z.seq_len <= 1 && !std::ptr::eq(z, first) {
                out[n] = Some(z);
                n += 1;
            }
        }
        (out, in_rr)
    }

    /// Zone for `target` at `key`. Key 0 (no pitch) or a key outside every
    /// zone falls back to the zone whose root is nearest the key.
    #[inline]
    pub fn resolve(&self, target: u8, key: u8) -> Option<&SlotDef> {
        let zones = &self.targets[target as usize];
        if zones.len() <= 1 || key == 0 {
            return zones.first();
        }
        zones
            .iter()
            .find(|z| (z.key_lo..=z.key_hi).contains(&key))
            .or_else(|| zones.iter().min_by_key(|z| (z.root_key as i16 - key as i16).abs()))
    }
}
