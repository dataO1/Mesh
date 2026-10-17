//! Per-lane sampler: fixed voice pool, sample-accurate event handling, RT render.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use basedrop::Shared;

use super::data::{SampleBank, SampleData, SlotDef, MAX_LAYERS};
use super::interp::{sinc_tables, Hermite, Interp, Interpolation, Linear, Sinc8, SincTables};
use super::params::{
    attack_secs, decay_secs, glide_secs, LaneSamplerParams, ModConfig, ZoneMod, ZoneTarget, CHOKE_FADE_SECS, PITCH_ENV_ATTACK_SECS,
};
use super::voice::{BlockMod, VoiceInit, VoiceState};
use crate::live::event::{flags, EventKind, LiveEvent};
use crate::live::MAX_EVENTS_PER_BAR;
use crate::types::{StereoBuffer, StereoSample};

/// Width of the region around the generator's position the Morph knob moves
/// a stream's notes over, as a share of the sample (knob centre = the
/// position, the ends half the region either side, wrapping).
const MORPH_REGION: f64 = 0.4;

/// UI-facing lane meters. Written by the RT thread once per block, read from
/// any thread through the `Arc` returned by [`Sampler::meters`].
#[derive(Debug, Default)]
pub struct SamplerMeters {
    voices: AtomicU32,
    peak_l: AtomicU32,
    peak_r: AtomicU32,
}

impl SamplerMeters {
    /// Voices sounding at the end of the last block.
    pub fn voices_active(&self) -> usize {
        self.voices.load(Ordering::Relaxed) as usize
    }

    /// Highest absolute sample per channel since the last [`Self::take_peak`].
    pub fn peak(&self) -> (f32, f32) {
        (
            f32::from_bits(self.peak_l.load(Ordering::Relaxed)),
            f32::from_bits(self.peak_r.load(Ordering::Relaxed)),
        )
    }

    /// Read and reset the held peak (UI decay is the reader's job).
    pub fn take_peak(&self) -> (f32, f32) {
        (
            f32::from_bits(self.peak_l.swap(0, Ordering::Relaxed)),
            f32::from_bits(self.peak_r.swap(0, Ordering::Relaxed)),
        )
    }

    fn publish(&self, voices: usize, l: f32, r: f32) {
        self.voices.store(voices as u32, Ordering::Relaxed);
        // Non-negative f32 bit patterns order like the integers, so fetch_max
        // on the bits is a float max.
        self.peak_l.fetch_max(l.to_bits(), Ordering::Relaxed);
        self.peak_r.fetch_max(r.to_bits(), Ordering::Relaxed);
    }
}

struct Voice {
    /// Kept alive while the voice plays, so a bank swap mid-note is safe.
    /// Dropped on the RT thread when the voice ends (basedrop defers the free).
    data: Option<Shared<SampleData>>,
    st: VoiceState,
    /// Output: 0 = the main one, n = bus n (see [`Sampler::set_bus_map`]).
    bus: u8,
}

/// The sampler engine for one lane.
///
/// RT contract: [`Self::handle_event`], [`Self::process`], [`Self::set_params`]
/// and [`Self::set_bank`] never allocate, lock or block. Construction and bank
/// building happen off the RT thread.
///
/// Event semantics (all fields of [`LiveEvent`], design §5.1):
/// - `Trigger` / `NoteOn` start a voice on the zone `bank.resolve(target, key)`.
///   `vel == 0` is ignored. Unmapped targets are ignored.
/// - **Pitch**: `key == 0` means "no pitch": the slot plays at its recorded
///   pitch. Otherwise the shift is `key − slot.root_key` semitones. Unpitched
///   slots (drum hits) ignore `key`. `cents − slot.root_cents` is always added.
/// - **Start / reverse**: `start` is a fraction of the slot. Forward playback
///   starts that far from the slot start. With `REVERSE` it is measured from the
///   slot end, so `start = 0` plays the whole slot backwards.
/// - **Gate**: `gate > 0` holds the voice for that many ticks, then releases.
///   `gate == 0` on a `Trigger` is a one-shot that plays to the slot end;
///   on a `NoteOn` it holds until the matching `NoteOff` (or the slot end).
///   Ticks are converted with the `samples_per_tick` passed to each `process`
///   call, so tempo changes between blocks shorten or lengthen the rest of the
///   gate correctly.
/// - **Loop**: `LOOP` on a held voice loops the slot. The loop keeps running
///   through the release tail so the release never hits the slot edge.
/// - **Envelope**: lane attack / decay / sustain / release. A non-zero event
///   `decay` replaces the decay time and sets sustain to 0, so the hit dies
///   away over that time (drum-machine decay).
/// - **Choke**: the effective group is the event `choke`, or the slot's default
///   when the event has 0. A new hit in a group fades the group's other voices
///   out over 10 ms. A `Choke` event fades its group; group 0 fades every voice.
/// - **Mono groups** ([`SampleBank::set_mono`]): a `NoteOn` while a voice of the
///   group is still held is played legato (as if `TIE` were set) instead of
///   retriggering. Triggers always retrigger.
/// - **Tie**: `TIE` on a `NoteOn` retargets the most recently started held
///   voice to the new pitch with an exponential rate ramp over `glide`
///   (0 = instant), and takes over its `note_id` and gate. Without a held voice
///   the note starts normally.
/// - **Pitch envelope**: a pitched `NoteOn` with a short gate (stab) starts
///   with a slight pitch rise and fall ([`pitch_env`]). The shorter the note,
///   the larger the rise and the faster the fall. Long notes play straight.
/// - **NoteOff** releases held voices with the matching `note_id`.
/// - **Voice stealing**: past `max_voices` sounding voices, the oldest one
///   fades out over [`STEAL_FADE_SECS`] while the new one starts in a spare
///   slot, so a steal never clicks. Only with every spare slot busy too is
///   the oldest voice cut.
pub struct Sampler {
    sr: f32,
    params: LaneSamplerParams,
    bank: Option<Shared<SampleBank>>,
    voices: Box<[Voice]>,
    pending: Box<[(u32, LiveEvent)]>,
    pending_len: usize,
    dropped_events: u32,
    age: u64,
    active: usize,
    tables: &'static SincTables,
    meters: Arc<SamplerMeters>,
    /// Targets `lo..hi` render into the second output of [`Sampler::process_split`].
    /// Bus of each target (0 = main output).
    bus_map: Box<[u8; 256]>,
    /// Buses in use (the largest index in `bus_map`).
    buses: usize,
    /// Samples per tick of the block being processed (note lengths).
    spt: f64,
    /// Voices that may sound at once; the slots past it are spares for
    /// stolen voices fading out.
    max_active: usize,
    /// Lane modulation state: CC values, pitch bend (target and smoothed, in
    /// cents), vibrato LFO phase (cycles).
    cc: [u8; 128],
    /// CCs the events (a phrase's curves) have set since the pack loaded.
    cc_set: u128,
    bend_target: f32,
    bend_now: f32,
    vib_phase: f32,
    /// Round-robin counter per target.
    rr: Box<[u32; 256]>,
}

/// Pitch bend smoothing time constant (no zipper on stepped bend curves).
const BEND_SMOOTH_SECS: f32 = 0.004;

// MUSICAL: short enough to free the voice fast, long enough not to click.
/// Fade of a stolen voice.
pub const STEAL_FADE_SECS: f32 = 0.005;

impl Sampler {
    /// New lane sampler with `max_voices` pre-allocated voices. Off-RT.
    pub fn new(sample_rate: u32, max_voices: usize) -> Self {
        let max_voices = max_voices.max(1);
        let spare = max_voices.div_ceil(2).max(2);
        let voices = (0..max_voices + spare)
            .map(|_| Voice { data: None, st: VoiceState::default(), bus: 0 })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            sr: sample_rate as f32,
            params: LaneSamplerParams::default(),
            bank: None,
            voices,
            pending: vec![(0, LiveEvent::default()); MAX_EVENTS_PER_BAR].into_boxed_slice(),
            pending_len: 0,
            dropped_events: 0,
            age: 0,
            active: 0,
            tables: sinc_tables(),
            meters: Arc::new(SamplerMeters::default()),
            bus_map: Box::new([0; 256]),
            buses: 0,
            spt: 0.0,
            max_active: max_voices,
            cc: [0; 128],
            cc_set: 0,
            bend_target: 0.0,
            bend_now: 0.0,
            vib_phase: 0.0,
            rr: Box::new([0; 256]),
        }
    }

    /// Route voices started on targets `lo..hi` to the second output of
    /// [`Self::process_split`] (the drum lane's break bus). Off-RT.
    pub fn set_bus_range(&mut self, range: Option<(u8, u8)>) {
        let mut map = [0u8; 256];
        if let Some((lo, hi)) = range {
            for t in lo..hi {
                map[t as usize] = 1;
            }
        }
        self.set_bus_map(map);
    }

    /// Route each target to an output: 0 = main, n = bus n of
    /// [`Self::process_split`]. Off-RT.
    pub fn set_bus_map(&mut self, map: [u8; 256]) {
        self.buses = map.iter().copied().max().unwrap_or(0) as usize;
        *self.bus_map = map;
    }

    /// Buses in use.
    pub fn buses(&self) -> usize {
        self.buses
    }

    /// Stop every voice at once and drop queued events (off-RT: calibration
    /// and tests measure each sound from silence).
    pub fn reset_voices(&mut self) {
        for v in self.voices.iter_mut() {
            v.st.active = false;
            v.data = None;
        }
        self.pending_len = 0;
        self.active = 0;
    }

    /// Swap the lane's sample bank. Playing voices keep their own sample
    /// handles; the old bank is freed by the basedrop collector.
    pub fn set_bank(&mut self, bank: Shared<SampleBank>) {
        self.bank = Some(bank);
    }

    pub fn set_params(&mut self, params: LaneSamplerParams) {
        // A new modulation map (another sound pack) starts from its CC values.
        if params.modulation != self.params.modulation {
            self.cc = [0; 128];
            self.cc_set = 0;
            for &(n, v) in params.modulation.cc_init.iter().filter(|(n, _)| *n < 128) {
                self.cc[n as usize] = v.min(127);
            }
            self.bend_target = 0.0;
        }
        self.params = params;
    }

    /// The CC values in force. The Tone and Morph knobs set their CCs; when
    /// the phrase draws such a CC too, the knob shifts the phrase's curve
    /// instead (centre = as written, the ends reach 0 and 127).
    pub(crate) fn cc_now(&self) -> [u8; 128] {
        let (p, mut cc) = (&self.params, self.cc);
        for (n, knob) in [(p.modulation.tone_cc as usize, p.tone), (p.modulation.morph_cc as usize, p.morph)] {
            if knob >= 0.0 && n < 128 {
                let k = knob.clamp(0.0, 1.0);
                cc[n] = if self.cc_set & (1u128 << n) != 0 {
                    (cc[n] as f32 + (k - 0.5) * 254.0).round().clamp(0.0, 127.0) as u8
                } else {
                    (k * 127.0).round() as u8
                };
            }
        }
        cc
    }

    /// The lane modulation of the next block of `frames`: bend smoothing,
    /// vibrato LFO, and the CC maps (the Tone knob overrides its CC).
    fn block_mod(&mut self, frames: usize) -> BlockMod {
        let cc = self.cc_now();
        let p = &self.params;
        let m = &p.modulation;
        let expr = p.expression.max(0.0);
        let coef = (-(frames as f32) / (BEND_SMOOTH_SECS * self.sr)).exp();
        self.bend_now = self.bend_target + (self.bend_now - self.bend_target) * coef;
        let depth = (m.vib_depth + ModConfig::sum(&m.vibrato_cc, &cc)) * expr;
        let vibrato = if depth != 0.0 {
            self.vib_phase = (self.vib_phase + m.vib_rate * frames as f32 / self.sr).fract();
            depth * (self.vib_phase * std::f32::consts::TAU).sin()
        } else {
            0.0
        };
        BlockMod {
            bend: self.bend_now * expr,
            vibrato,
            vib_delay: m.vib_delay,
            vib_fade: m.vib_fade,
            cutoff_oct: ModConfig::sum(&m.cutoff_cc, &cc) / 1200.0,
            gain: 10f32.powf(ModConfig::sum(&m.volume_cc, &cc) / 20.0),
            cc,
        }
    }

    pub fn params(&self) -> &LaneSamplerParams {
        &self.params
    }

    pub fn sample_rate(&self) -> u32 {
        self.sr as u32
    }

    pub fn max_voices(&self) -> usize {
        self.max_active
    }

    /// Voices sounding after the last `process` call.
    pub fn voices_active(&self) -> usize {
        self.active
    }

    /// Shared meter handle for the UI. Clone it once, off the RT thread.
    pub fn meters(&self) -> Arc<SamplerMeters> {
        self.meters.clone()
    }

    /// Events dropped because more than `MAX_EVENTS_PER_BAR` were queued for
    /// one block.
    pub fn dropped_events(&self) -> u32 {
        self.dropped_events
    }

    /// Queue an event to fire `offset` frames into the next [`Self::process`]
    /// block. Offsets past the block end are clamped to its last frame.
    pub fn handle_event(&mut self, ev: &LiveEvent, offset: u32) {
        if self.pending_len == self.pending.len() {
            self.dropped_events = self.dropped_events.saturating_add(1);
            return;
        }
        self.pending[self.pending_len] = (offset, *ev);
        self.pending_len += 1;
    }

    /// Render one block into `out` (overwritten). `samples_per_tick` is the
    /// clock's current tick length, used for gates.
    pub fn process(&mut self, out: &mut [StereoSample], samples_per_tick: f64) {
        self.process_split(out, None, samples_per_tick);
    }

    /// [`Self::process`] with the bus voices rendered into `bus`: bus n in
    /// frames `(n - 1) * len .. n * len` of it (`len` = `out.len()`,
    /// overwritten). Without `bus` they go to `out`.
    pub fn process_split(&mut self, out: &mut [StereoSample], mut bus: Option<&mut [StereoSample]>, samples_per_tick: f64) {
        out.fill(StereoSample::silence());
        if let Some(b) = bus.as_deref_mut() {
            b.fill(StereoSample::silence());
        }
        let n = out.len();
        let spt = samples_per_tick.max(1e-9);
        self.spt = spt;

        let p = self.params;
        let m = self.block_mod(n);
        for v in self.voices.iter_mut().filter(|v| v.st.active) {
            v.st.update_block(&p, self.sr, n, &m);
        }

        // Stable sort pending by offset (insertion sort, usually sorted).
        let pend = &mut self.pending[..self.pending_len];
        for i in 1..pend.len() {
            let mut j = i;
            while j > 0 && pend[j - 1].0 > pend[j].0 {
                pend.swap(j - 1, j);
                j -= 1;
            }
        }

        let last = n.saturating_sub(1);
        let mut k = 0;
        let mut cursor = 0;
        loop {
            while k < self.pending_len && (self.pending[k].0 as usize).min(last) <= cursor {
                let ev = self.pending[k].1;
                self.apply(&ev);
                k += 1;
            }
            if cursor >= n {
                break;
            }
            let next = if k < self.pending_len { (self.pending[k].0 as usize).min(last) } else { n };
            self.render(&mut out[cursor..next], bus.as_deref_mut(), (n, cursor), spt);
            cursor = next;
        }
        self.pending_len = 0;

        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for o in out.iter_mut().chain(bus.into_iter().flatten()) {
            *o *= p.gain;
            pl = pl.max(o.left.abs());
            pr = pr.max(o.right.abs());
        }
        self.active = self.voices.iter().filter(|v| v.st.active).count();
        self.meters.publish(self.active, pl, pr);
    }

    /// [`Self::process`] into the first `frames` frames of a [`StereoBuffer`].
    pub fn process_buffer(&mut self, out: &mut StereoBuffer, frames: usize, samples_per_tick: f64) {
        let frames = frames.min(out.len());
        self.process(&mut out.as_mut_slice()[..frames], samples_per_tick);
    }

    /// Render into `out` (frames `at.1..` of a block of `at.0`) and the
    /// same frames of each bus.
    fn render(&mut self, out: &mut [StereoSample], bus: Option<&mut [StereoSample]>, at: (usize, usize), spt: f64) {
        if out.is_empty() {
            return;
        }
        match self.params.interpolation {
            Interpolation::Linear => self.render_with::<Linear>(out, bus, at, spt),
            Interpolation::Hermite => self.render_with::<Hermite>(out, bus, at, spt),
            Interpolation::Sinc8 => self.render_with::<Sinc8>(out, bus, at, spt),
        }
    }

    fn render_with<I: Interp>(
        &mut self,
        out: &mut [StereoSample],
        mut bus: Option<&mut [StereoSample]>,
        (block, from): (usize, usize),
        spt: f64,
    ) {
        let len = out.len();
        for v in self.voices.iter_mut() {
            if !v.st.active {
                continue;
            }
            let at = (v.bus as usize).saturating_sub(1) * block + from;
            let dst: &mut [StereoSample] = match bus.as_deref_mut() {
                Some(b) if v.bus > 0 && at + len <= b.len() => &mut b[at..at + len],
                _ => &mut *out,
            };
            match v.data.as_deref() {
                Some(data) => v.st.render::<I>(data, dst, spt, self.tables, self.sr),
                None => v.st.active = false,
            }
            if !v.st.active {
                v.data = None;
            }
        }
    }

    fn apply(&mut self, ev: &LiveEvent) {
        match ev.kind {
            EventKind::Trigger | EventKind::NoteOn => self.note_on(ev),
            EventKind::NoteOff => {
                for v in self.voices.iter_mut() {
                    if v.st.active && v.st.held && v.st.note_id == ev.note_id {
                        v.st.release();
                    }
                }
            }
            EventKind::Choke => self.choke_group(ev.choke),
            EventKind::Bend => self.bend_target = ev.cents as f32,
            EventKind::Cc => {
                if ev.target < 128 {
                    self.cc[ev.target as usize] = ev.vel.min(127);
                    self.cc_set |= 1u128 << ev.target;
                }
            }
        }
    }

    fn choke_group(&mut self, group: u8) {
        let fade = CHOKE_FADE_SECS * self.sr;
        for v in self.voices.iter_mut() {
            if v.st.active && (group == 0 || v.st.group == group) {
                v.st.choke(fade);
            }
        }
    }

    fn note_on(&mut self, ev: &LiveEvent) {
        if ev.vel == 0 {
            return;
        }
        let Some(bank) = self.bank.as_ref() else { return };
        let rr = self.rr[ev.target as usize];
        // Every zone the note plays (layers); clones only bump the sample's refcount.
        let (found, in_rr) = bank.resolve_layers(ev.target, ev.key, ev.vel, rr);
        let mono = found[0].is_some_and(|s| bank.is_mono(if ev.choke != 0 { ev.choke } else { s.choke }));
        let mut layers: [Option<SlotDef>; MAX_LAYERS] = Default::default();
        for (dst, src) in layers.iter_mut().zip(found) {
            *dst = src.filter(|s| !s.is_empty()).cloned();
        }
        if layers[0].is_none() {
            return;
        }
        if in_rr {
            self.rr[ev.target as usize] = rr.wrapping_add(1);
        }
        let group = layers[0].as_ref().map_or(0, |s| if ev.choke != 0 { ev.choke } else { s.choke });
        let gate_ticks = if ev.gate > 0 { ev.gate as f64 } else { f64::INFINITY };

        // Tie (or a mono group): the held voices of the newest note glide to
        // the new pitch, each layer from its own zone root.
        // A stab always strikes (its envelope and accent), unless tied.
        if ev.kind == EventKind::NoteOn && (ev.has(flags::TIE) || (mono && !ev.has(flags::STAB))) {
            let newest = self
                .voices
                .iter()
                .filter(|v| v.st.active && v.st.held && !v.st.is_choking() && (!mono || v.st.group == group))
                .max_by_key(|v| v.st.age)
                .map(|v| v.st.note_id);
            if let Some(id) = newest {
                let glide = (glide_secs(ev.glide) * self.params.expression.max(0.0).min(2.0) * self.sr) as f64;
                for v in self.voices.iter_mut().filter(|v| v.st.active && v.st.held && v.st.note_id == id) {
                    let rate = v.st.rate_for(ev.key, ev.cents as f64);
                    v.st.glide_to(rate, glide);
                    v.st.note_id = ev.note_id;
                    v.st.gate_ticks_left = gate_ticks;
                }
                return;
            }
        }

        if group != 0 {
            self.choke_group(group);
        }
        for slot in layers.iter().flatten() {
            self.start_voice(ev, slot, group, gate_ticks);
        }
    }

    /// Start one voice of a note on `slot`.
    fn start_voice(&mut self, ev: &LiveEvent, slot: &SlotDef, group: u8, gate_ticks: f64) {
        let p = &self.params;
        let pitched = slot.pitched && ev.key != 0;
        let semis = if pitched { ev.key as f64 - slot.root_key as f64 } else { 0.0 };
        let root_rate = (-(slot.root_cents as f64) / 1200.0).exp2() * (slot.sample.sample_rate() as f64 / self.sr as f64);
        let rate = root_rate * (semis / 12.0 + ev.cents as f64 / 1200.0).exp2();

        let vel = (ev.vel.min(127) as f32 / 127.0).powf(p.velocity_curve);
        let accent = if ev.has(flags::ACCENT) { p.accent_gain } else { 1.0 };
        let held = (ev.gate > 0 || ev.kind == EventKind::NoteOn) && !slot.one_shot;
        let stab = ev.has(flags::STAB);
        let (decay, sustain) = if ev.decay != 0 {
            (decay_secs(ev.decay), 0.0)
        } else if stab {
            (p.decay, 0.0)
        } else {
            (p.decay, p.sustain)
        };
        self.age += 1;
        let init = VoiceInit {
            age: self.age,
            group,
            note_id: ev.note_id,
            held,
            gate_ticks,
            looping: held && (ev.has(flags::LOOP) || slot.loop_range.is_some()),
            reverse: ev.has(flags::REVERSE),
            start: slot.start,
            end: slot.end,
            // Notes start at the slot start, except on streams (mudpies),
            // where the generator picks the position.
            // Pack zones may set their own start (SFZ `offset`, plus its CC).
            offset: if ev.kind == EventKind::NoteOn && !slot.stream {
                let cc = self.cc_now();
                let cc_frames = (ModConfig::sum(&p.modulation.offset_cc, &cc)
                    + ZoneMod::sum(&slot.zone_mods, &cc, ZoneTarget::Offset)) as f64;
                ((slot.start_offset as f64 + cc_frames) / slot.len().max(1) as f64).clamp(0.0, 0.999)
            } else {
                // A stream: the generator's seeded position across the whole
                // sample, moved within a region around it by the Morph knob.
                let morph = if p.morph >= 0.0 { (p.morph as f64 - 0.5) * MORPH_REGION } else { 0.0 };
                (ev.start as f64 / u16::MAX as f64 + morph).rem_euclid(1.0).min(0.999)
            },
            rate,
            gain: slot.gain * vel * accent,
            pan_offset: ev.pan as f32 / 127.0 + slot.pan,
            cutoff_offset: ev.cutoff as f32 / 127.0,
            // A gliding cutoff starts where the newest voice's is now.
            cutoff_start: if p.cutoff_glide > 0.0 {
                self.voices
                    .iter()
                    .filter(|v| v.st.active)
                    .max_by_key(|v| v.st.age)
                    .map_or(ev.cutoff as f32 / 127.0, |v| v.st.cutoff_now())
            } else {
                ev.cutoff as f32 / 127.0
            },
            attack: if ev.attack != 0 { attack_secs(ev.attack) } else { p.attack },
            pitch_env: if slot.pitched && ev.kind == EventKind::NoteOn {
                pitch_env(gate_ticks * self.spt / self.sr as f64)
            } else {
                None
            },
            decay,
            sustain,
            punch: if stab { p.punch } else { 0.0 },
            loop_range: slot.loop_range,
            loop_sustain: slot.loop_sustain,
            zone_mods: slot.zone_mods,
            root_key: pitched.then_some(slot.root_key),
            root_rate,
        };
        let data = slot.sample.clone();

        // A free slot, else (all spares busy) cut the oldest voice, preferring
        // one that is already fading.
        let idx = match self.voices.iter().position(|v| !v.st.active) {
            Some(i) => i,
            None => self
                .voices
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| (!v.st.is_choking(), v.st.age))
                .map(|(i, _)| i)
                .unwrap_or(0),
        };
        let v = &mut self.voices[idx];
        v.data = Some(data);
        v.bus = self.bus_map[ev.target as usize];
        v.st.start(&init, &self.params, self.sr);
        // Over the limit: the oldest sounding voice fades out.
        let sounding = self.voices.iter().filter(|v| v.st.active && !v.st.is_choking()).count();
        if sounding > self.max_active {
            let fade = STEAL_FADE_SECS * self.sr;
            if let Some(old) = self
                .voices
                .iter_mut()
                .filter(|v| v.st.active && !v.st.is_choking())
                .min_by_key(|v| v.st.age)
            {
                old.st.choke(fade);
            }
        }
    }

    /// Cutoff offset of the newest active voice (tests).
    #[cfg(test)]
    pub(crate) fn newest_voice_cutoff(&self) -> Option<f32> {
        self.voices.iter().filter(|v| v.st.active).max_by_key(|v| v.st.age).map(|v| v.st.cutoff_now())
    }

    /// Playback rate of the newest active voice including its pitch envelope (tests).
    #[cfg(test)]
    pub(crate) fn newest_voice_pitch(&self) -> Option<f64> {
        self.voices.iter().filter(|v| v.st.active).max_by_key(|v| v.st.age).map(|v| v.st.pitch_rate())
    }

    /// Current playback rate of the most recently started active voice (tests).
    #[cfg(test)]
    pub(crate) fn newest_voice_rate(&self) -> Option<f64> {
        self.voices.iter().filter(|v| v.st.active).max_by_key(|v| v.st.age).map(|v| v.st.rate())
    }
}

// MUSICAL: first draft, tune by ear (user: a slight pitch envelope on stabs,
// fast attack, about 100 ms fall depending on the note length).
/// Pitch rise of the shortest notes in semitones.
const PITCH_ENV_SEMIS: f32 = 1.5;
/// Notes up to this long get the full rise. It fades out by `PITCH_ENV_LONG_SECS`.
const PITCH_ENV_SHORT_SECS: f64 = 0.12;
const PITCH_ENV_LONG_SECS: f64 = 0.6;
/// Fall time: this share of the note, between the bounds.
const PITCH_ENV_FALL_SHARE: f64 = 0.7;
const PITCH_ENV_FALL_SECS: (f64, f64) = (0.03, 0.1);

/// Pitch envelope (rise in semitones, fall in seconds) of a note lasting
/// `secs`. `None` for long or held notes.
pub(crate) fn pitch_env(secs: f64) -> Option<(f32, f32)> {
    if !secs.is_finite() || secs >= PITCH_ENV_LONG_SECS {
        return None;
    }
    let t = ((secs - PITCH_ENV_SHORT_SECS) / (PITCH_ENV_LONG_SECS - PITCH_ENV_SHORT_SECS)).clamp(0.0, 1.0);
    // Smoothstep down from full to none.
    let shortness = 1.0 - t * t * (3.0 - 2.0 * t);
    let semis = PITCH_ENV_SEMIS * shortness as f32;
    let fall = (secs * PITCH_ENV_FALL_SHARE).clamp(PITCH_ENV_FALL_SECS.0, PITCH_ENV_FALL_SECS.1);
    (semis > 0.01 && fall > PITCH_ENV_ATTACK_SECS as f64).then_some((semis, fall as f32))
}
