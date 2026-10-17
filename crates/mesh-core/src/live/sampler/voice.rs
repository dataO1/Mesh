//! One sampler voice: resampled playback, amp envelope, SVF lowpass, pan,
//! choke fade and pitch glide.

use std::f32::consts::{FRAC_PI_4, PI, SQRT_2};

use super::data::{SampleData, PAD};
use super::interp::{Interp, SincTables};
use super::params::{
    exp_coef, LaneSamplerParams, ModEnv, ZoneMod, ZoneTarget, MAX_ZONE_MODS, CUTOFF_MAX_HZ, CUTOFF_MIN_HZ, CUTOFF_OFFSET_OCTAVES, EDGE_FADE_SECS, ENV_SILENCE,
    PITCH_ENV_ATTACK_SECS,
};
use crate::types::StereoSample;

// MUSICAL: stab accent, tune by ear.
/// Gain kick at a note start at full punch (+1 = +6 dB), and its fade time.
/// It drives the saturation, so it shows as a blown-out attack.
const PUNCH_GAIN: f32 = 0.6;
const PUNCH_SECS: f32 = 0.03;
/// Extra mod-envelope pitch depth at full punch (on top of Expression).
const PUNCH_PITCH: f32 = 1.8;
/// Saturation drive at full punch, and the share of it taken back at the
/// output (in dB terms).
const PUNCH_SAT_DB: f32 = 15.0;
const PUNCH_SAT_BACK: f32 = 0.8;
/// The mod envelope runs this much slower at full punch.
const PUNCH_ENV_STRETCH: f32 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Everything a new voice needs, resolved from event + slot + lane params.
pub(crate) struct VoiceInit {
    pub age: u64,
    pub group: u8,
    pub note_id: u16,
    pub held: bool,
    pub gate_ticks: f64,
    pub looping: bool,
    pub reverse: bool,
    /// Slot range in frames.
    pub start: usize,
    pub end: usize,
    /// Start offset as a fraction of the slot (from the end when reversed).
    pub offset: f64,
    pub rate: f64,
    pub gain: f32,
    pub pan_offset: f32,
    pub cutoff_offset: f32,
    /// Cutoff offset the voice starts at, gliding to `cutoff_offset`.
    pub cutoff_start: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    /// Stab accent of this note (0 = none).
    pub punch: f32,
    /// Pitch envelope: rise in semitones and fall time in seconds.
    pub pitch_env: Option<(f32, f32)>,
    /// Loop points (frames, absolute) for a looping voice; None = the slot.
    pub loop_range: Option<(usize, usize)>,
    /// Stop looping on release (SFZ `loop_sustain`).
    pub loop_sustain: bool,
    /// The zone's CC modulators (gain, cutoff).
    pub zone_mods: [ZoneMod; MAX_ZONE_MODS],
    /// Pitch reference for ties: the zone's root key (None = unpitched) and
    /// the playback rate at it.
    pub root_key: Option<u8>,
    pub root_rate: f64,
}

/// Lane modulation for one block (pitch bend, vibrato, CC-driven filter and
/// volume), computed by the sampler and applied by every voice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlockMod {
    /// Pitch bend in cents (smoothed).
    pub bend: f32,
    /// Vibrato value in cents at full depth for this block (LFO × depth).
    pub vibrato: f32,
    /// Vibrato delay and fade-in from each note's start, seconds.
    pub vib_delay: f32,
    pub vib_fade: f32,
    /// Cutoff shift in octaves, gain multiplier.
    pub cutoff_oct: f32,
    pub gain: f32,
    /// CC values in force (zone modulators read them).
    pub cc: [u8; 128],
}

impl Default for BlockMod {
    fn default() -> Self {
        Self { bend: 0.0, vibrato: 0.0, vib_delay: 0.0, vib_fade: 0.0, cutoff_oct: 0.0, gain: 1.0, cc: [0; 128] }
    }
}

/// Voice state. Sample data is held next to it in [`super::Sampler`] so the
/// render loop can borrow both at once.
#[derive(Debug, Clone)]
pub(crate) struct VoiceState {
    pub active: bool,
    pub age: u64,
    pub group: u8,
    pub note_id: u16,
    /// Gate still open (not yet released).
    pub held: bool,
    /// Remaining gate in ticks while held; infinite = wait for NoteOff / end.
    pub gate_ticks_left: f64,
    pub looping: bool,
    /// Loop points while looping (default: the slot), and whether the loop
    /// ends on release.
    loop_lo: f64,
    loop_hi: f64,
    loop_sustain: bool,
    /// Seconds since the note started (vibrato delay and fade).
    t: f32,
    /// Pitch factor of the lane modulation (bend + vibrato), per block.
    mod_pf: f64,
    /// Gain of the lane modulation (CC volume), per block.
    mod_gain: f32,
    /// Stab saturation drive (linear; 1 = off), from the punch.
    sat: f32,
    /// Stab accent of the note (from its `STAB` flag).
    punch: f32,
    /// Output gain taken back after the saturation.
    sat_back: f32,
    /// Mod envelope value 0..1 and the cutoff shift (octaves) of the mod
    /// envelope and LFO, per block.
    menv: f32,
    note_cutoff_oct: f32,
    zone_mods: [ZoneMod; MAX_ZONE_MODS],
    root_key: Option<u8>,
    root_rate: f64,

    pos: f64,
    dir: f64,
    start: f64,
    end: f64,
    rate: f64,
    target_rate: f64,
    glide_mul: f64,
    glide_left: u32,

    stage: Stage,
    level: f32,
    /// Attack progress 0..1; the level follows a quarter sine of it (a
    /// rounded attack that eases into the top).
    attack_phase: f32,
    attack_inc: f32,
    decay_coef: f32,
    sustain: f32,
    release_coef: f32,

    /// Pitch envelope offset in semitones, its peak, rise per sample and
    /// fall coefficient. `pe_rising` until the peak; off at 0.
    pe: f32,
    pe_peak: f32,
    pe_inc: f32,
    pe_coef: f32,
    pe_rising: bool,

    /// Choke fade multiplier and its per-sample decrement (0 = not choking).
    fade: f32,
    fade_step: f32,

    gain: f32,
    pan_offset: f32,
    cutoff_offset: f32,
    cutoff_target: f32,
    /// Slope morph 0 (6 dB/oct) .. 1 (24 dB/oct), `None` = plain 12 dB/oct.
    morph: Option<f32>,
    /// One-pole coefficient and states (L, R), second SVF states.
    op_a: f32,
    op: [f32; 2],
    ic2: [f32; 4],
    gl: f32,
    gr: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    /// SVF integrator states: [ic1 L, ic2 L, ic1 R, ic2 R].
    ic: [f32; 4],
}

impl Default for VoiceState {
    fn default() -> Self {
        Self {
            active: false,
            age: 0,
            group: 0,
            note_id: 0,
            held: false,
            gate_ticks_left: f64::INFINITY,
            looping: false,
            loop_lo: 0.0,
            loop_hi: 0.0,
            loop_sustain: false,
            t: 0.0,
            mod_pf: 1.0,
            mod_gain: 1.0,
            sat: 1.0,
            punch: 0.0,
            sat_back: 1.0,
            menv: 0.0,
            note_cutoff_oct: 0.0,
            zone_mods: [ZoneMod::NONE; MAX_ZONE_MODS],
            root_key: None,
            root_rate: 1.0,
            pos: 0.0,
            dir: 1.0,
            start: 0.0,
            end: 0.0,
            rate: 1.0,
            target_rate: 1.0,
            glide_mul: 1.0,
            glide_left: 0,
            stage: Stage::Attack,
            level: 0.0,
            attack_phase: 0.0,
            attack_inc: 1.0,
            decay_coef: 0.0,
            sustain: 1.0,
            release_coef: 0.0,
            pe: 0.0,
            pe_peak: 0.0,
            pe_inc: 0.0,
            pe_coef: 0.0,
            pe_rising: false,
            fade: 1.0,
            fade_step: 0.0,
            gain: 0.0,
            pan_offset: 0.0,
            cutoff_offset: 0.0,
            cutoff_target: 0.0,
            morph: None,
            op_a: 0.0,
            op: [0.0; 2],
            ic2: [0.0; 4],
            gl: 1.0,
            gr: 1.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            ic: [0.0; 4],
        }
    }
}

impl VoiceState {
    pub fn start(&mut self, v: &VoiceInit, p: &LaneSamplerParams, sr: f32) {
        let len = (v.end - v.start) as f64;
        let (start, end) = (v.start as f64, v.end as f64);
        let pos = if v.reverse {
            (end - 1.0 - v.offset * len).max(start)
        } else {
            (start + v.offset * len).min(end - 1.0)
        };
        let attack_n = v.attack * sr;
        *self = Self {
            active: true,
            age: v.age,
            group: v.group,
            note_id: v.note_id,
            held: v.held,
            gate_ticks_left: v.gate_ticks,
            looping: v.looping,
            loop_lo: v.loop_range.map_or(start, |(a, _)| (a as f64).clamp(start, end - 1.0)),
            loop_hi: v.loop_range.map_or(end, |(_, b)| (b as f64).clamp(start + 1.0, end)),
            loop_sustain: v.loop_sustain,
            zone_mods: v.zone_mods,
            root_key: v.root_key,
            root_rate: v.root_rate,
            pos,
            dir: if v.reverse { -1.0 } else { 1.0 },
            start,
            end,
            rate: v.rate,
            target_rate: v.rate,
            stage: if attack_n < 1.0 { Stage::Decay } else { Stage::Attack },
            level: if attack_n < 1.0 { 1.0 } else { 0.0 },
            attack_inc: 1.0 / attack_n.max(1.0),
            decay_coef: exp_coef(v.decay, sr),
            sustain: v.sustain,
            punch: v.punch,
            gain: v.gain,
            pan_offset: v.pan_offset,
            cutoff_offset: v.cutoff_start,
            cutoff_target: v.cutoff_offset,
            ..Self::default()
        };
        if let Some((semis, fall)) = v.pitch_env {
            self.pe_peak = semis;
            self.pe_inc = semis / (PITCH_ENV_ATTACK_SECS * sr).max(1.0);
            self.pe_coef = exp_coef(fall, sr);
            self.pe_rising = true;
        }
        self.update_block(p, sr, 0, &BlockMod::default());
    }

    /// Recompute block-rate values from the lane parameters. `frames`: the
    /// block length, for the cutoff glide.
    pub fn update_block(&mut self, p: &LaneSamplerParams, sr: f32, frames: usize, m: &BlockMod) {
        // Lane modulation: bend and vibrato (faded in per note), CC filter and volume.
        let vib = if m.vibrato != 0.0 && self.t >= m.vib_delay {
            let ramp = if m.vib_fade > 0.0 { ((self.t - m.vib_delay) / m.vib_fade).min(1.0) } else { 1.0 };
            m.vibrato * ramp
        } else {
            0.0
        };
        // Per-note mod envelope and LFO (a sound pack's shape), their depths
        // scaled by the Expression knob like bends and vibrato.
        let (env, lfo) = (&p.modulation.env, &p.modulation.lfo);
        let expr = p.expression.clamp(0.0, 2.0);
        let mut cents = m.bend + vib;
        // The zone's own modulators (a crossfade layer, per-layer filter).
        let mut cut_cents = ZoneMod::sum(&self.zone_mods, &m.cc, ZoneTarget::Cutoff);
        let mut gain = m.gain * ZoneMod::gain(&self.zone_mods, &m.cc);
        // Stab accent: a gain kick fading over the first tens of ms.
        let punch = self.punch;
        if punch > 0.0 {
            gain *= 1.0 + PUNCH_GAIN * punch * (-self.t / PUNCH_SECS).exp();
        }
        if env.is_on() {
            // Stabs: the mod envelope runs slower, so the filter falls less
            // quickly (rounder stabs).
            let stretch = 1.0 + PUNCH_ENV_STRETCH * punch;
            self.menv = mod_env_value(env, self.menv, self.t / stretch, frames as f32 / sr / stretch, self.stage == Stage::Release);
            cents += self.menv * env.to_pitch * expr * (1.0 + PUNCH_PITCH * punch);
            cut_cents += self.menv * env.to_cutoff * expr;
        }
        if lfo.is_on() && self.t >= lfo.delay {
            let ph = (self.t - lfo.delay) * lfo.rate + 0.25;
            let tri = 4.0 * (ph - ph.floor() - 0.5).abs() - 1.0;
            cents += tri * lfo.to_pitch * expr;
            cut_cents += tri * lfo.to_cutoff * expr;
            gain *= 10f32.powf(tri * lfo.to_volume_db * expr / 20.0);
        }
        self.note_cutoff_oct = cut_cents / 1200.0;
        self.mod_pf = ((cents / 1200.0) as f64).exp2();
        self.mod_gain = gain;
        self.sat = if punch > 0.0 { 10f32.powf(PUNCH_SAT_DB * punch / 20.0) } else { 1.0 };
        self.sat_back = self.sat.powf(-PUNCH_SAT_BACK);
        self.t += frames as f32 / sr;
        if p.cutoff_glide > 0.0 && frames > 0 {
            let coef = (-(frames as f32) / (p.cutoff_glide * sr)).exp();
            self.cutoff_offset = self.cutoff_target + (self.cutoff_offset - self.cutoff_target) * coef;
        } else if p.cutoff_glide <= 0.0 {
            self.cutoff_offset = self.cutoff_target;
        }
        // Constant-power pan, normalised to unity gain at centre.
        let pan = (p.pan + self.pan_offset).clamp(-1.0, 1.0);
        let theta = (pan + 1.0) * FRAC_PI_4;
        self.gl = theta.cos() * SQRT_2;
        self.gr = theta.sin() * SQRT_2;

        // TPT state-variable lowpass (Simper / Zavalishin).
        let fc = (p.cutoff_hz * (self.cutoff_offset * CUTOFF_OFFSET_OCTAVES + m.cutoff_oct + self.note_cutoff_oct).exp2())
            .clamp(CUTOFF_MIN_HZ, CUTOFF_MAX_HZ.min(0.45 * sr));
        let g = (PI * fc / sr).tan();
        let k = SQRT_2 * (1.0 - 0.985 * p.resonance.clamp(0.0, 1.0));
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
        self.morph = p.filter_morph.map(|(lo, hi)| ((fc / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0));
        self.op_a = 1.0 - (-2.0 * PI * fc / sr).exp();

        self.release_coef = exp_coef(p.release, sr);
    }

    /// Enter the release stage.
    pub fn release(&mut self) {
        self.held = false;
        self.stage = Stage::Release;
        if self.loop_sustain {
            self.looping = false;
        }
    }

    /// Start (or keep) the choke fade over `fade_samples`.
    pub fn choke(&mut self, fade_samples: f32) {
        self.held = false;
        let step = 1.0 / fade_samples.max(1.0);
        if self.fade_step == 0.0 || step > self.fade_step {
            self.fade_step = step;
        }
    }

    pub fn is_choking(&self) -> bool {
        self.fade_step > 0.0
    }

    /// The playback rate of this voice's zone for `key` and `cents`.
    pub fn rate_for(&self, key: u8, cents: f64) -> f64 {
        let semis = match self.root_key {
            Some(root) if key != 0 => key as f64 - root as f64,
            _ => 0.0,
        };
        self.root_rate * (semis / 12.0 + cents / 1200.0).exp2()
    }

    /// Tie: retarget pitch with an exponential rate ramp over `glide_samples`.
    pub fn glide_to(&mut self, rate: f64, glide_samples: f64) {
        self.target_rate = rate;
        if glide_samples < 1.0 {
            self.rate = rate;
            self.glide_left = 0;
        } else {
            self.glide_left = glide_samples as u32;
            self.glide_mul = (rate / self.rate).powf(1.0 / self.glide_left as f64);
        }
    }

    /// Current playback rate (tests).
    #[cfg(test)]
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Playback rate including the pitch envelope (tests).
    #[cfg(test)]
    pub fn pitch_rate(&self) -> f64 {
        self.rate * pitch_factor(self.pe)
    }

    /// Fastest rate this voice will play at before the next segment, for
    /// picking the sinc kernel band.
    pub fn max_rate(&self) -> f64 {
        self.rate.max(self.target_rate) * pitch_factor(self.pe_peak.max(self.pe)) * self.mod_pf.max(1.0)
    }

    /// Render up to `out.len()` frames, adding into `out`. Splits at the gate
    /// expiry so release starts on the exact sample. Returns with
    /// `active == false` once the voice has finished.
    pub fn render<I: Interp>(
        &mut self,
        data: &SampleData,
        out: &mut [StereoSample],
        spt: f64,
        tables: &SincTables,
        sr: f32,
    ) {
        let mut done = 0;
        while done < out.len() && self.active {
            let mut seg = out.len() - done;
            let gated = self.held && self.gate_ticks_left.is_finite();
            if gated {
                let left = self.gate_ticks_left * spt;
                if left <= 1e-6 {
                    self.release();
                } else {
                    seg = seg.min((left - 1e-6).ceil() as usize);
                }
            }
            self.render_seg::<I>(data, &mut out[done..done + seg], tables, sr);
            if gated && self.held {
                self.gate_ticks_left -= seg as f64 / spt;
            }
            done += seg;
        }
    }

    #[inline(always)]
    fn render_seg<I: Interp>(&mut self, data: &SampleData, out: &mut [StereoSample], tables: &SincTables, sr: f32) {
        let buf = data.padded();
        let kernel = tables.band(self.max_rate());
        let edge_len = (EDGE_FADE_SECS * sr) as f64;
        let mut inv_edge = 1.0 / (self.rate.max(1e-6) * edge_len);
        let (lo, hi) = (self.loop_lo, self.loop_hi);
        let len = hi - lo;
        let mpf = self.mod_pf;
        let mut step = self.rate * self.dir * pitch_factor(self.pe) * mpf;
        let (a1, a2, a3) = (self.a1, self.a2, self.a3);
        let [mut ic1l, mut ic2l, mut ic1r, mut ic2r] = self.ic;

        // Frames whose interpolation taps stay inside the slot; loops read
        // wrapped taps outside this range so the seam is continuous.
        let (safe_lo, safe_hi) = (lo as usize + 4, (hi as usize).saturating_sub(6));

        for o in out.iter_mut() {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            let s = if self.looping && (i < safe_lo || i > safe_hi) {
                self.read_wrapped::<I>(buf, i, frac, kernel)
            } else {
                I::read(buf, i + PAD, frac, kernel)
            };

            // Amp envelope.
            match self.stage {
                Stage::Attack => {
                    self.attack_phase += self.attack_inc;
                    if self.attack_phase >= 1.0 {
                        self.level = 1.0;
                        self.stage = Stage::Decay;
                    } else {
                        self.level = (self.attack_phase * std::f32::consts::FRAC_PI_2).sin();
                    }
                }
                Stage::Decay => {
                    self.level = self.sustain + (self.level - self.sustain) * self.decay_coef;
                    if self.level - self.sustain < ENV_SILENCE {
                        self.level = self.sustain;
                        self.stage = Stage::Sustain;
                    }
                }
                Stage::Sustain => {}
                Stage::Release => self.level *= self.release_coef,
            }

            // Lowpass, both channels.
            let v3 = s.left - ic2l;
            let v1 = a1 * ic1l + a2 * v3;
            let lp_l = ic2l + a2 * ic1l + a3 * v3;
            ic1l = 2.0 * v1 - ic1l;
            ic2l = 2.0 * lp_l - ic2l;
            let v3 = s.right - ic2r;
            let v1 = a1 * ic1r + a2 * v3;
            let lp_r = ic2r + a2 * ic1r + a3 * v3;
            ic1r = 2.0 * v1 - ic1r;
            ic2r = 2.0 * lp_r - ic2r;
            let (lp_l, lp_r) = match self.morph {
                Some(m) => self.morph_filter(s.left, s.right, lp_l, lp_r, m),
                None => (lp_l, lp_r),
            };

            let mut g = self.gain * self.level * self.fade * self.mod_gain;
            if !self.looping {
                let rem = if self.dir > 0.0 { self.end - self.pos } else { self.pos - self.start + 1.0 };
                g *= (rem * inv_edge).min(1.0) as f32;
            }
            let (mut l, mut r) = (lp_l * g, lp_r * g);
            if self.sat > 1.0 {
                // Stab saturation: soft clip, the gain taken back by most of
                // the drive, so the attack blows out instead of spiking.
                l = (l * self.sat).tanh() * self.sat_back;
                r = (r * self.sat).tanh() * self.sat_back;
            }
            o.left += l * self.gl;
            o.right += r * self.gr;

            if self.fade_step > 0.0 {
                self.fade -= self.fade_step;
                if self.fade <= 0.0 {
                    self.active = false;
                    break;
                }
            }
            if (self.stage == Stage::Release || (self.stage == Stage::Sustain && self.sustain < ENV_SILENCE))
                && self.level < ENV_SILENCE
            {
                self.active = false;
                break;
            }

            // Advance.
            if self.glide_left > 0 {
                self.glide_left -= 1;
                self.rate = if self.glide_left == 0 { self.target_rate } else { self.rate * self.glide_mul };
                step = self.rate * self.dir * mpf;
                inv_edge = 1.0 / (self.rate.max(1e-6) * edge_len);
            }
            if self.pe_rising || self.pe > 0.0 {
                if self.pe_rising {
                    self.pe += self.pe_inc;
                    if self.pe >= self.pe_peak {
                        self.pe = self.pe_peak;
                        self.pe_rising = false;
                    }
                } else {
                    self.pe *= self.pe_coef;
                    if self.pe < PITCH_ENV_OFF {
                        self.pe = 0.0;
                        self.pe_peak = 0.0;
                    }
                }
                step = self.rate * self.dir * pitch_factor(self.pe) * mpf;
            }
            self.pos += step;
            if self.looping && self.pos >= hi {
                self.pos -= len * ((self.pos - lo) / len).floor();
            } else if self.looping && self.dir < 0.0 && self.pos < lo {
                self.pos += len * ((hi - self.pos) / len).floor();
            } else if self.pos >= self.end {
                self.active = false;
                break;
            } else if self.pos < self.start {
                self.active = false;
                break;
            }
        }
        self.ic = [ic1l, ic2l, ic1r, ic2r];
    }

    /// Slope morph: a one-pole (6 dB/oct) under the SVF (12), a second SVF
    /// stage over it (24), crossfaded by `m`.
    #[inline(always)]
    fn morph_filter(&mut self, xl: f32, xr: f32, l12: f32, r12: f32, m: f32) -> (f32, f32) {
        let a = self.op_a;
        self.op[0] += a * (xl - self.op[0]);
        self.op[1] += a * (xr - self.op[1]);
        let (a1, a2, a3) = (self.a1, self.a2, self.a3);
        let [mut c1l, mut c2l, mut c1r, mut c2r] = self.ic2;
        let v3 = l12 - c2l;
        let v1 = a1 * c1l + a2 * v3;
        let l24 = c2l + a2 * c1l + a3 * v3;
        c1l = 2.0 * v1 - c1l;
        c2l = 2.0 * l24 - c2l;
        let v3 = r12 - c2r;
        let v1 = a1 * c1r + a2 * v3;
        let r24 = c2r + a2 * c1r + a3 * v3;
        c1r = 2.0 * v1 - c1r;
        c2r = 2.0 * r24 - c2r;
        self.ic2 = [c1l, c2l, c1r, c2r];
        if m < 0.5 {
            let t = m * 2.0;
            (self.op[0] + (l12 - self.op[0]) * t, self.op[1] + (r12 - self.op[1]) * t)
        } else {
            let t = m * 2.0 - 1.0;
            (l12 + (l24 - l12) * t, r12 + (r24 - r12) * t)
        }
    }

    /// Current cutoff offset (gliding).
    pub fn cutoff_now(&self) -> f32 {
        self.cutoff_offset
    }

    /// Interpolate at frame `i` with taps wrapped into the slot (loop seam).
    #[cold]
    fn read_wrapped<I: Interp>(&self, buf: &[StereoSample], i: usize, frac: f32, kernel: &[f32]) -> StereoSample {
        const C: usize = 4;
        let (start, len) = (self.loop_lo as i64, (self.loop_hi - self.loop_lo) as i64);
        let mut taps = [StereoSample::default(); 2 * C + 4];
        for (j, t) in taps.iter_mut().enumerate() {
            let k = i as i64 - C as i64 + j as i64;
            let w = start + (k - start).rem_euclid(len);
            *t = buf[PAD + w as usize];
        }
        I::read(&taps, C, frac, kernel)
    }
}

/// Pitch envelope offset (semitones) below which it stops.
const PITCH_ENV_OFF: f32 = 1e-3;

/// Rate factor of a pitch offset in semitones.
#[inline(always)]
/// The mod envelope at note time `t` (seconds), stepping `dt`; `prev` is its
/// last value (the release falls from it).
fn mod_env_value(e: &ModEnv, prev: f32, t: f32, dt: f32, released: bool) -> f32 {
    const LN_1000: f32 = 6.907_755;
    if released {
        return if e.release > 0.0 { prev * (-LN_1000 * dt / e.release).exp() } else { 0.0 };
    }
    let t = t - e.delay;
    if t < 0.0 {
        0.0
    } else if t < e.attack {
        t / e.attack
    } else if t < e.attack + e.hold {
        1.0
    } else if e.decay > 0.0 {
        e.sustain + (1.0 - e.sustain) * (-LN_1000 * (t - e.attack - e.hold) / e.decay).exp()
    } else {
        e.sustain
    }
}

fn pitch_factor(semis: f32) -> f64 {
    if semis == 0.0 { 1.0 } else { ((semis / 12.0) as f64).exp2() }
}

#[cfg(test)]
mod mod_env_tests {
    use super::*;

    #[test]
    fn mod_envelope_shape() {
        let e = ModEnv { delay: 0.1, attack: 0.1, hold: 0.1, decay: 0.2, sustain: 0.25, release: 0.5, to_pitch: 0.0, to_cutoff: 1200.0 };
        assert_eq!(mod_env_value(&e, 0.0, 0.05, 0.01, false), 0.0, "delay");
        assert!((mod_env_value(&e, 0.0, 0.15, 0.01, false) - 0.5).abs() < 1e-5, "half way up the attack");
        assert_eq!(mod_env_value(&e, 0.0, 0.25, 0.01, false), 1.0, "hold");
        assert!((mod_env_value(&e, 0.0, 2.0, 0.01, false) - 0.25).abs() < 1e-3, "sustain");
        let r = mod_env_value(&e, 0.25, 2.0, 0.5, true);
        assert!((r - 0.25 / 1000.0).abs() < 1e-5, "released over its release time: {r}");
    }
}
