//! One sampler voice: resampled playback, amp envelope, SVF lowpass, pan,
//! choke fade and pitch glide.

use std::f32::consts::{FRAC_PI_4, PI, SQRT_2};

use super::data::{SampleData, PAD};
use super::interp::{Interp, SincTables};
use super::params::{
    exp_coef, LaneSamplerParams, CUTOFF_MAX_HZ, CUTOFF_MIN_HZ, CUTOFF_OFFSET_OCTAVES, EDGE_FADE_SECS, ENV_SILENCE,
};
use crate::types::StereoSample;

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
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
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
    attack_inc: f32,
    decay_coef: f32,
    sustain: f32,
    release_coef: f32,

    /// Choke fade multiplier and its per-sample decrement (0 = not choking).
    fade: f32,
    fade_step: f32,

    gain: f32,
    pan_offset: f32,
    cutoff_offset: f32,
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
            attack_inc: 1.0,
            decay_coef: 0.0,
            sustain: 1.0,
            release_coef: 0.0,
            fade: 1.0,
            fade_step: 0.0,
            gain: 0.0,
            pan_offset: 0.0,
            cutoff_offset: 0.0,
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
            gain: v.gain,
            pan_offset: v.pan_offset,
            cutoff_offset: v.cutoff_offset,
            ..Self::default()
        };
        self.update_block(p, sr);
    }

    /// Recompute block-rate values from the lane parameters.
    pub fn update_block(&mut self, p: &LaneSamplerParams, sr: f32) {
        // Constant-power pan, normalised to unity gain at centre.
        let pan = (p.pan + self.pan_offset).clamp(-1.0, 1.0);
        let theta = (pan + 1.0) * FRAC_PI_4;
        self.gl = theta.cos() * SQRT_2;
        self.gr = theta.sin() * SQRT_2;

        // TPT state-variable lowpass (Simper / Zavalishin).
        let fc = (p.cutoff_hz * (self.cutoff_offset * CUTOFF_OFFSET_OCTAVES).exp2())
            .clamp(CUTOFF_MIN_HZ, CUTOFF_MAX_HZ.min(0.45 * sr));
        let g = (PI * fc / sr).tan();
        let k = SQRT_2 * (1.0 - 0.985 * p.resonance.clamp(0.0, 1.0));
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;

        self.release_coef = exp_coef(p.release, sr);
    }

    /// Enter the release stage.
    pub fn release(&mut self) {
        self.held = false;
        self.stage = Stage::Release;
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

    /// Fastest rate this voice will play at before the next segment, for
    /// picking the sinc kernel band.
    pub fn max_rate(&self) -> f64 {
        self.rate.max(self.target_rate)
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
        let len = self.end - self.start;
        let mut step = self.rate * self.dir;
        let (a1, a2, a3) = (self.a1, self.a2, self.a3);
        let [mut ic1l, mut ic2l, mut ic1r, mut ic2r] = self.ic;

        // Frames whose interpolation taps stay inside the slot; loops read
        // wrapped taps outside this range so the seam is continuous.
        let (safe_lo, safe_hi) = (self.start as usize + 4, (self.end as usize).saturating_sub(6));

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
                    self.level += self.attack_inc;
                    if self.level >= 1.0 {
                        self.level = 1.0;
                        self.stage = Stage::Decay;
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

            let mut g = self.gain * self.level * self.fade;
            if !self.looping {
                let rem = if self.dir > 0.0 { self.end - self.pos } else { self.pos - self.start + 1.0 };
                g *= (rem * inv_edge).min(1.0) as f32;
            }
            o.left += lp_l * g * self.gl;
            o.right += lp_r * g * self.gr;

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
                step = self.rate * self.dir;
                inv_edge = 1.0 / (self.rate.max(1e-6) * edge_len);
            }
            self.pos += step;
            if self.pos >= self.end {
                if self.looping {
                    self.pos -= len * ((self.pos - self.start) / len).floor();
                } else {
                    self.active = false;
                    break;
                }
            } else if self.pos < self.start {
                if self.looping {
                    self.pos += len * ((self.end - self.pos) / len).floor();
                } else {
                    self.active = false;
                    break;
                }
            }
        }
        self.ic = [ic1l, ic2l, ic1r, ic2r];
    }

    /// Interpolate at frame `i` with taps wrapped into the slot (loop seam).
    #[cold]
    fn read_wrapped<I: Interp>(&self, buf: &[StereoSample], i: usize, frac: f32, kernel: &[f32]) -> StereoSample {
        const C: usize = 4;
        let (start, len) = (self.start as i64, (self.end - self.start) as i64);
        let mut taps = [StereoSample::default(); 2 * C + 4];
        for (j, t) in taps.iter_mut().enumerate() {
            let k = i as i64 - C as i64 + j as i64;
            let w = start + (k - start).rem_euclid(len);
            *t = buf[PAD + w as usize];
        }
        I::read(&taps, C, frac, kernel)
    }
}
