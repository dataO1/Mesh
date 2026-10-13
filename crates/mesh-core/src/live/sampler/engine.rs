//! Per-lane sampler: fixed voice pool, sample-accurate event handling, RT render.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use basedrop::Shared;

use super::data::{SampleBank, SampleData};
use super::interp::{sinc_tables, Hermite, Interp, Interpolation, Linear, Sinc8, SincTables};
use super::params::{decay_secs, glide_secs, LaneSamplerParams, CHOKE_FADE_SECS};
use super::voice::{VoiceInit, VoiceState};
use crate::live::event::{flags, EventKind, LiveEvent};
use crate::live::MAX_EVENTS_PER_BAR;
use crate::types::{StereoBuffer, StereoSample};

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
/// - **NoteOff** releases held voices with the matching `note_id`.
/// - **Voice stealing**: with no free voice, the oldest voice in the lane is
///   cut and reused.
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
}

impl Sampler {
    /// New lane sampler with `max_voices` pre-allocated voices. Off-RT.
    pub fn new(sample_rate: u32, max_voices: usize) -> Self {
        let voices = (0..max_voices.max(1))
            .map(|_| Voice { data: None, st: VoiceState::default() })
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
        }
    }

    /// Swap the lane's sample bank. Playing voices keep their own sample
    /// handles; the old bank is freed by the basedrop collector.
    pub fn set_bank(&mut self, bank: Shared<SampleBank>) {
        self.bank = Some(bank);
    }

    pub fn set_params(&mut self, params: LaneSamplerParams) {
        self.params = params;
    }

    pub fn params(&self) -> &LaneSamplerParams {
        &self.params
    }

    pub fn sample_rate(&self) -> u32 {
        self.sr as u32
    }

    pub fn max_voices(&self) -> usize {
        self.voices.len()
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
        out.fill(StereoSample::silence());
        let n = out.len();
        let spt = samples_per_tick.max(1e-9);

        let p = self.params;
        for v in self.voices.iter_mut().filter(|v| v.st.active) {
            v.st.update_block(&p, self.sr);
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
            self.render(&mut out[cursor..next], spt);
            cursor = next;
        }
        self.pending_len = 0;

        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for o in out.iter_mut() {
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

    fn render(&mut self, out: &mut [StereoSample], spt: f64) {
        if out.is_empty() {
            return;
        }
        match self.params.interpolation {
            Interpolation::Linear => self.render_with::<Linear>(out, spt),
            Interpolation::Hermite => self.render_with::<Hermite>(out, spt),
            Interpolation::Sinc8 => self.render_with::<Sinc8>(out, spt),
        }
    }

    fn render_with<I: Interp>(&mut self, out: &mut [StereoSample], spt: f64) {
        for v in self.voices.iter_mut() {
            if !v.st.active {
                continue;
            }
            match v.data.as_deref() {
                Some(data) => v.st.render::<I>(data, out, spt, self.tables, self.sr),
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
        let Some(slot) = bank.resolve(ev.target, ev.key) else { return };
        if slot.is_empty() {
            return;
        }
        let p = &self.params;
        let group = if ev.choke != 0 { ev.choke } else { slot.choke };
        let semis = if slot.pitched && ev.key != 0 { ev.key as f64 - slot.root_key as f64 } else { 0.0 };
        let cents = ev.cents as f64 - slot.root_cents as f64;
        let rate = (semis / 12.0 + cents / 1200.0).exp2() * (slot.sample.sample_rate() as f64 / self.sr as f64);
        let mono = bank.is_mono(group);
        let gate_ticks = if ev.gate > 0 { ev.gate as f64 } else { f64::INFINITY };

        if ev.kind == EventKind::NoteOn && (ev.has(flags::TIE) || mono) {
            let held = self
                .voices
                .iter_mut()
                .filter(|v| v.st.active && v.st.held && !v.st.is_choking() && (!mono || v.st.group == group))
                .max_by_key(|v| v.st.age);
            if let Some(v) = held {
                v.st.glide_to(rate, (glide_secs(ev.glide) * self.sr) as f64);
                v.st.note_id = ev.note_id;
                v.st.gate_ticks_left = gate_ticks;
                return;
            }
        }

        let vel = (ev.vel.min(127) as f32 / 127.0).powf(p.velocity_curve);
        let accent = if ev.has(flags::ACCENT) { p.accent_gain } else { 1.0 };
        let held = ev.gate > 0 || ev.kind == EventKind::NoteOn;
        let (decay, sustain) = if ev.decay != 0 { (decay_secs(ev.decay), 0.0) } else { (p.decay, p.sustain) };
        self.age += 1;
        let init = VoiceInit {
            age: self.age,
            group,
            note_id: ev.note_id,
            held,
            gate_ticks,
            looping: held && ev.has(flags::LOOP),
            reverse: ev.has(flags::REVERSE),
            start: slot.start,
            end: slot.end,
            offset: ev.start as f64 / u16::MAX as f64,
            rate,
            gain: slot.gain * vel * accent,
            pan_offset: ev.pan as f32 / 127.0,
            cutoff_offset: ev.cutoff as f32 / 127.0,
            attack: p.attack,
            decay,
            sustain,
        };
        let data = slot.sample.clone();

        if group != 0 {
            self.choke_group(group);
        }
        let idx = match self.voices.iter().position(|v| !v.st.active) {
            Some(i) => i,
            None => self
                .voices
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| v.st.age)
                .map(|(i, _)| i)
                .unwrap_or(0),
        };
        let v = &mut self.voices[idx];
        v.data = Some(data);
        v.st.start(&init, &self.params, self.sr);
    }

    /// Current playback rate of the most recently started active voice (tests).
    #[cfg(test)]
    pub(crate) fn newest_voice_rate(&self) -> Option<f64> {
        self.voices.iter().filter(|v| v.st.active).max_by_key(|v| v.st.age).map(|v| v.st.rate())
    }
}
