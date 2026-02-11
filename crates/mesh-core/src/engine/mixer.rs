//! Mixer - Combines deck outputs with volume/filter/cue controls
//!
//! Features:
//! - Per-channel trim, 3-band EQ, filter, volume, cue
//! - Master volume and cue/master blend

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rayon::prelude::*;

use super::master_clipper::MasterClipper;
use super::master_limiter::MasterLimiter;
use crate::types::{StereoBuffer, StereoSample, NUM_DECKS, SAMPLE_RATE};

/// Biquad filter state for EQ bands
#[derive(Debug, Clone, Default)]
struct BiquadState {
    x1_l: f32, x2_l: f32, y1_l: f32, y2_l: f32,
    x1_r: f32, x2_r: f32, y1_r: f32, y2_r: f32,
}

impl BiquadState {
    fn process(&mut self, input_l: f32, input_r: f32, coeffs: &BiquadCoeffs) -> (f32, f32) {
        // Left channel
        let out_l = coeffs.b0 * input_l + coeffs.b1 * self.x1_l + coeffs.b2 * self.x2_l
                  - coeffs.a1 * self.y1_l - coeffs.a2 * self.y2_l;
        self.x2_l = self.x1_l;
        self.x1_l = input_l;
        self.y2_l = self.y1_l;
        self.y1_l = out_l;

        // Right channel
        let out_r = coeffs.b0 * input_r + coeffs.b1 * self.x1_r + coeffs.b2 * self.x2_r
                  - coeffs.a1 * self.y1_r - coeffs.a2 * self.y2_r;
        self.x2_r = self.x1_r;
        self.x1_r = input_r;
        self.y2_r = self.y1_r;
        self.y1_r = out_r;

        (out_l, out_r)
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Biquad filter coefficients
#[derive(Debug, Clone)]
struct BiquadCoeffs {
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
}

impl BiquadCoeffs {
    /// Create low shelf filter coefficients
    /// gain_db: boost/cut in dB, freq: shelf frequency
    fn low_shelf(freq: f32, gain_db: f32, sample_rate: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / 2.0 * ((a + 1.0/a) * (1.0/0.9 - 1.0) + 2.0).sqrt();

        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha;
        Self {
            b0: (a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha)) / a0,
            b1: (2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha)) / a0,
            a1: (-2.0 * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha) / a0,
        }
    }

    /// Create peaking EQ filter coefficients
    fn peaking(freq: f32, gain_db: f32, q: f32, sample_rate: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);

        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }

    /// Create high shelf filter coefficients
    fn high_shelf(freq: f32, gain_db: f32, sample_rate: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / 2.0 * ((a + 1.0/a) * (1.0/0.9 - 1.0) + 2.0).sqrt();

        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha;
        Self {
            b0: (a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha)) / a0,
            b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha)) / a0,
            a1: (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha) / a0,
        }
    }

    /// Passthrough (unity gain, no filtering)
    fn passthrough() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }
    }
}

/// EQ frequency centers
const EQ_LO_FREQ: f32 = 100.0;   // Low shelf at 100 Hz
const EQ_MID_FREQ: f32 = 1000.0; // Mid peak at 1 kHz
const EQ_HI_FREQ: f32 = 10000.0; // High shelf at 10 kHz
const EQ_MID_Q: f32 = 0.7;       // Q for mid band

/// Channel strip state for a single deck
#[derive(Debug, Clone)]
pub struct ChannelStrip {
    /// Trim/gain control (-24 to +12 dB, stored as linear multiplier)
    pub trim: f32,
    /// EQ Low band (0.0 = kill, 0.5 = flat, 1.0 = +6dB)
    pub eq_lo: f32,
    /// EQ Mid band (0.0 = kill, 0.5 = flat, 1.0 = +6dB)
    pub eq_mid: f32,
    /// EQ High band (0.0 = kill, 0.5 = flat, 1.0 = +6dB)
    pub eq_hi: f32,
    /// Filter position (-1.0 = full LP, 0.0 = flat, 1.0 = full HP)
    pub filter: f32,
    /// Volume fader (0.0 to 1.0)
    pub volume: f32,
    /// Cue button state (routes to cue bus)
    pub cue_enabled: bool,

    // EQ filter states
    eq_lo_state: BiquadState,
    eq_mid_state: BiquadState,
    eq_hi_state: BiquadState,

    // EQ coefficients (cached, recalculated when EQ changes)
    eq_lo_coeffs: BiquadCoeffs,
    eq_mid_coeffs: BiquadCoeffs,
    eq_hi_coeffs: BiquadCoeffs,
    eq_dirty: bool,

    // Filter state (simple one-pole for now)
    lp_state_l: f32,
    lp_state_r: f32,
    hp_state_l: f32,
    hp_state_r: f32,
}

impl Default for ChannelStrip {
    fn default() -> Self {
        Self {
            trim: 1.0,       // Unity gain
            eq_lo: 0.5,      // Flat
            eq_mid: 0.5,     // Flat
            eq_hi: 0.5,      // Flat
            filter: 0.0,     // Flat
            volume: 1.0,     // Full volume
            cue_enabled: false,
            eq_lo_state: BiquadState::default(),
            eq_mid_state: BiquadState::default(),
            eq_hi_state: BiquadState::default(),
            eq_lo_coeffs: BiquadCoeffs::passthrough(),
            eq_mid_coeffs: BiquadCoeffs::passthrough(),
            eq_hi_coeffs: BiquadCoeffs::passthrough(),
            eq_dirty: true,
            lp_state_l: 0.0,
            lp_state_r: 0.0,
            hp_state_l: 0.0,
            hp_state_r: 0.0,
        }
    }
}

impl ChannelStrip {
    /// Create a new channel strip with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set trim in dB (-24 to +12)
    pub fn set_trim_db(&mut self, db: f32) {
        let db = db.clamp(-24.0, 12.0);
        self.trim = 10.0_f32.powf(db / 20.0);
    }

    /// Get trim in dB
    pub fn trim_db(&self) -> f32 {
        20.0 * self.trim.log10()
    }

    /// Set EQ low band (0.0 = kill, 0.5 = flat, 1.0 = +6dB boost)
    pub fn set_eq_lo(&mut self, value: f32) {
        self.eq_lo = value.clamp(0.0, 1.0);
        self.eq_dirty = true;
    }

    /// Set EQ mid band (0.0 = kill, 0.5 = flat, 1.0 = +6dB boost)
    pub fn set_eq_mid(&mut self, value: f32) {
        self.eq_mid = value.clamp(0.0, 1.0);
        self.eq_dirty = true;
    }

    /// Set EQ high band (0.0 = kill, 0.5 = flat, 1.0 = +6dB boost)
    pub fn set_eq_hi(&mut self, value: f32) {
        self.eq_hi = value.clamp(0.0, 1.0);
        self.eq_dirty = true;
    }

    /// Convert EQ knob position (0-1) to dB gain
    /// 0.0 = -inf (kill), 0.5 = 0dB, 1.0 = +6dB
    fn eq_to_db(value: f32) -> f32 {
        if value < 0.01 {
            -60.0  // Near-kill
        } else if value < 0.5 {
            // 0.01 to 0.5 -> -60dB to 0dB (logarithmic)
            let t = (value - 0.01) / 0.49;
            -60.0 * (1.0 - t)
        } else {
            // 0.5 to 1.0 -> 0dB to +6dB (linear)
            (value - 0.5) * 12.0
        }
    }

    /// Recalculate EQ coefficients if dirty
    fn update_eq_coeffs(&mut self) {
        if !self.eq_dirty {
            return;
        }

        let sr = SAMPLE_RATE as f32;
        let lo_db = Self::eq_to_db(self.eq_lo);
        let mid_db = Self::eq_to_db(self.eq_mid);
        let hi_db = Self::eq_to_db(self.eq_hi);

        // Only update if significantly different from flat
        if lo_db.abs() > 0.1 {
            self.eq_lo_coeffs = BiquadCoeffs::low_shelf(EQ_LO_FREQ, lo_db, sr);
        } else {
            self.eq_lo_coeffs = BiquadCoeffs::passthrough();
        }

        if mid_db.abs() > 0.1 {
            self.eq_mid_coeffs = BiquadCoeffs::peaking(EQ_MID_FREQ, mid_db, EQ_MID_Q, sr);
        } else {
            self.eq_mid_coeffs = BiquadCoeffs::passthrough();
        }

        if hi_db.abs() > 0.1 {
            self.eq_hi_coeffs = BiquadCoeffs::high_shelf(EQ_HI_FREQ, hi_db, sr);
        } else {
            self.eq_hi_coeffs = BiquadCoeffs::passthrough();
        }

        self.eq_dirty = false;
    }

    /// Process audio through the channel strip (trim + EQ + filter)
    pub fn process(&mut self, buffer: &mut StereoBuffer) {
        // Update EQ coefficients if needed
        self.update_eq_coeffs();

        // Filter coefficient based on position
        let filter_pos = self.filter.clamp(-1.0, 1.0);

        // Cutoff frequencies (in Hz)
        let lp_cutoff = if filter_pos < 0.0 {
            // LP active: sweep from 20kHz down to 100Hz
            20000.0 * (1.0 + filter_pos).max(0.005)
        } else {
            20000.0 // LP disabled
        };

        let hp_cutoff = if filter_pos > 0.0 {
            // HP active: sweep from 20Hz up to 5kHz
            20.0 + filter_pos * 4980.0
        } else {
            20.0 // HP disabled
        };

        // Convert to filter coefficients (simple one-pole)
        let lp_coeff = Self::cutoff_to_coeff(lp_cutoff);
        let hp_coeff = Self::cutoff_to_coeff(hp_cutoff);

        for sample in buffer.iter_mut() {
            // Apply trim
            let mut left = sample.left * self.trim;
            let mut right = sample.right * self.trim;

            // Apply 3-band EQ
            (left, right) = self.eq_lo_state.process(left, right, &self.eq_lo_coeffs);
            (left, right) = self.eq_mid_state.process(left, right, &self.eq_mid_coeffs);
            (left, right) = self.eq_hi_state.process(left, right, &self.eq_hi_coeffs);

            // Apply LP filter
            self.lp_state_l += lp_coeff * (left - self.lp_state_l);
            self.lp_state_r += lp_coeff * (right - self.lp_state_r);
            left = self.lp_state_l;
            right = self.lp_state_r;

            // Apply HP filter (subtract LP from original)
            self.hp_state_l += hp_coeff * (left - self.hp_state_l);
            self.hp_state_r += hp_coeff * (right - self.hp_state_r);
            left = left - self.hp_state_l;
            right = right - self.hp_state_r;

            *sample = StereoSample::new(left, right);
        }
    }

    /// Convert cutoff frequency to one-pole filter coefficient
    fn cutoff_to_coeff(cutoff: f32) -> f32 {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        let dt = 1.0 / SAMPLE_RATE as f32;
        dt / (rc + dt)
    }

    /// Reset all filter states
    pub fn reset(&mut self) {
        self.eq_lo_state.reset();
        self.eq_mid_state.reset();
        self.eq_hi_state.reset();
        self.lp_state_l = 0.0;
        self.lp_state_r = 0.0;
        self.hp_state_l = 0.0;
        self.hp_state_r = 0.0;
    }
}

/// Main mixer combining all deck outputs
pub struct Mixer {
    /// Per-deck channel strips
    channels: [ChannelStrip; NUM_DECKS],
    /// Master volume (0.0 to 1.0)
    master_volume: f32,
    /// Cue/master blend for headphones (0.0 = cue only, 1.0 = master only)
    cue_mix: f32,
    /// Cue/headphone output volume (0.0 to 1.0)
    cue_volume: f32,
    /// Master bus lookahead limiter (transparent, before clipper)
    limiter: MasterLimiter,
    /// Master bus safety clipper (ClipOnly2-style, after limiter)
    clipper: MasterClipper,
}

impl Mixer {
    /// Create a new mixer
    pub fn new() -> Self {
        Self {
            channels: std::array::from_fn(|_| ChannelStrip::new()),
            master_volume: 1.0,
            cue_mix: 0.0,
            cue_volume: 0.8,
            limiter: MasterLimiter::new(),
            clipper: MasterClipper::new(),
        }
    }

    /// Get a reference to a channel strip
    pub fn channel(&self, deck: usize) -> Option<&ChannelStrip> {
        self.channels.get(deck)
    }

    /// Get a mutable reference to a channel strip
    pub fn channel_mut(&mut self, deck: usize) -> Option<&mut ChannelStrip> {
        self.channels.get_mut(deck)
    }

    /// Set master volume (0.0 to 1.0)
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 1.0);
    }

    /// Get master volume
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    /// Set cue/master mix (0.0 = cue only, 1.0 = master only)
    pub fn set_cue_mix(&mut self, mix: f32) {
        self.cue_mix = mix.clamp(0.0, 1.0);
    }

    /// Get cue mix
    pub fn cue_mix(&self) -> f32 {
        self.cue_mix
    }

    /// Get the master clipper's clip indicator atomic (for UI)
    pub fn clip_indicator(&self) -> Arc<AtomicBool> {
        self.clipper.clip_indicator()
    }

    /// Set cue/headphone volume (0.0 to 1.0)
    pub fn set_cue_volume(&mut self, volume: f32) {
        self.cue_volume = volume.clamp(0.0, 1.0);
    }

    /// Get cue volume
    pub fn cue_volume(&self) -> f32 {
        self.cue_volume
    }

    /// Process deck outputs and produce master + cue outputs
    ///
    /// deck_buffers: Array of processed deck outputs
    /// master_out: Output buffer for master mix
    /// cue_out: Output buffer for cue/headphone mix
    ///
    /// Uses Rayon for parallel channel strip processing - each deck's EQ/filter
    /// chain runs on a separate thread, then results are summed to master/cue.
    pub fn process(
        &mut self,
        deck_buffers: &mut [StereoBuffer; NUM_DECKS],
        master_out: &mut StereoBuffer,
        cue_out: &mut StereoBuffer,
    ) {
        let buffer_len = master_out.len();
        master_out.fill_silence();
        cue_out.fill_silence();

        // Phase 1: Parallel channel strip processing (EQ, filters)
        // Each channel processes its deck buffer independently
        self.channels
            .par_iter_mut()
            .zip(deck_buffers.par_iter_mut())
            .for_each(|(channel, buffer)| {
                channel.process(buffer);
            });

        // Phase 2: Sequential summing to master/cue buses
        // This is fast O(n) and must be sequential to avoid race conditions
        for (deck_idx, buffer) in deck_buffers.iter().enumerate() {
            let channel = &self.channels[deck_idx];

            // Add to master output (with volume fader)
            for i in 0..buffer_len.min(buffer.len()) {
                let sample = buffer[i];

                // Master bus: apply volume fader
                let master_sample = sample * channel.volume;
                master_out.as_mut_slice()[i] += master_sample;

                // Cue bus: full volume (bypass fader) if cue enabled
                if channel.cue_enabled {
                    cue_out.as_mut_slice()[i] += sample;
                }
            }
        }

        // Apply master volume
        master_out.scale(self.master_volume);

        // Safety clipper: shaves transient peaks cleanly (zero latency)
        self.clipper.process(master_out);

        // Lookahead limiter: transparent gain reduction for sustained overs
        self.limiter.process(master_out);

        // Mix cue/master for headphones (cue_out becomes the headphone output)
        for i in 0..buffer_len {
            let master = master_out[i];
            let cue = cue_out[i];

            // Crossfade between cue and master, then apply cue volume
            cue_out.as_mut_slice()[i] = StereoSample::new(
                (cue.left * (1.0 - self.cue_mix) + master.left * self.cue_mix) * self.cue_volume,
                (cue.right * (1.0 - self.cue_mix) + master.right * self.cue_mix) * self.cue_volume,
            );
        }
    }

    /// Reset all channel strip filter states
    pub fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.reset();
        }
    }
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_strip_defaults() {
        let strip = ChannelStrip::new();
        assert_eq!(strip.trim, 1.0);
        assert_eq!(strip.filter, 0.0);
        assert_eq!(strip.volume, 1.0);
        assert!(!strip.cue_enabled);
    }

    #[test]
    fn test_trim_db_conversion() {
        let mut strip = ChannelStrip::new();

        strip.set_trim_db(0.0);
        assert!((strip.trim - 1.0).abs() < 0.001);

        strip.set_trim_db(6.0);
        assert!((strip.trim - 2.0).abs() < 0.01);

        strip.set_trim_db(-6.0);
        assert!((strip.trim - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_mixer_creation() {
        let mixer = Mixer::new();
        assert_eq!(mixer.master_volume(), 1.0);
        assert_eq!(mixer.cue_mix(), 0.0);
        assert_eq!(mixer.cue_volume(), 0.8);
    }
}
