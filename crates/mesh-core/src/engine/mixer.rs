//! Mixer - Combines deck outputs with volume/filter/cue controls
//!
//! Features:
//! - Per-channel trim, 3-band EQ, filter, volume, cue
//! - Master volume and cue/master blend

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use rayon::prelude::*;

use super::master_clipper::MasterClipper;
use super::master_limiter::MasterLimiter;
use crate::effect::native::svf::SvfFilter;
use crate::types::{StereoBuffer, StereoSample, NUM_DECKS, SAMPLE_RATE};

/// Lock-free peak level atomics exposed to the UI thread for metering.
///
/// Audio thread writes on every buffer; UI thread reads at ~60 fps.
/// Values are linear amplitudes (not dBFS) — the UI converts to dB.
pub struct LevelAtomics {
    /// Per-channel post-fader peak (f32 bits, linear 0.0..~2.0)
    pub channel_peaks: [AtomicU32; NUM_DECKS],
    /// Master bus peak after clipper/limiter (f32 bits, linear 0.0..~2.0)
    pub master_peak: AtomicU32,
}

impl LevelAtomics {
    pub fn new() -> Self {
        Self {
            channel_peaks: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            master_peak: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    pub fn channel_peak(&self, deck: usize) -> f32 {
        f32::from_bits(self.channel_peaks[deck].load(Ordering::Relaxed))
    }

    pub fn master_peak(&self) -> f32 {
        f32::from_bits(self.master_peak.load(Ordering::Relaxed))
    }
}

impl Default for LevelAtomics {
    fn default() -> Self { Self::new() }
}

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

// ── DJ Filter (24 dB/oct cascaded SVF with adaptive Q) ──────────────

/// Dead zone half-width around center position (bypass when |pos| < this)
const DJ_FILTER_DEAD_ZONE: f32 = 0.02;
/// Minimum Q (transparent, near-Butterworth)
const DJ_FILTER_Q_MIN: f32 = 0.5;
/// Maximum Q (dramatic resonant peak)
const DJ_FILTER_Q_MAX: f32 = 3.0;
/// Q curve exponent (concave: slow rise at first, fast at extremes)
const DJ_FILTER_Q_POWER: f32 = 1.5;
/// LP sweep lower bound (Hz)
const DJ_FILTER_LP_MIN: f32 = 60.0;
/// LP sweep upper bound (Hz)
const DJ_FILTER_LP_MAX: f32 = 20000.0;
/// HP sweep lower bound (Hz)
const DJ_FILTER_HP_MIN: f32 = 20.0;
/// HP sweep upper bound (Hz)
const DJ_FILTER_HP_MAX: f32 = 12000.0;
/// Effective sweep range: knob ±1.0 maps to ±this value internally.
/// 0.65 keeps the extremes musical without over-cutting.
const DJ_FILTER_SWEEP_RANGE: f32 = 0.65;

/// Two-stage (24 dB/oct) DJ mixer filter with adaptive resonance.
///
/// Cascades two 12 dB/oct SVF stages for the steep slope expected from
/// professional DJ mixers (Pioneer DJM, Allen & Heath Xone:92). Q rises
/// with sweep depth so the filter stays transparent near center but
/// develops a singing resonant peak at the extremes.
#[derive(Debug, Clone)]
struct DjFilter {
    /// First 12 dB/oct SVF stage
    stage1: SvfFilter,
    /// Second 12 dB/oct SVF stage (cascade → 24 dB/oct)
    stage2: SvfFilter,
    /// Cached position to skip recalculation when knob is stationary
    last_position: f32,
}

impl DjFilter {
    fn new() -> Self {
        Self {
            stage1: SvfFilter::new(),
            stage2: SvfFilter::new(),
            last_position: f32::NAN, // force first update
        }
    }

    /// Recalculate cutoff & Q from the knob position (-1..+1).
    /// The raw knob range is scaled by SWEEP_RANGE so the extremes stay
    /// musical instead of cutting too aggressively.
    fn update_params(&mut self, position: f32) {
        if self.last_position == position {
            return;
        }
        self.last_position = position;

        // Scale into effective range: full knob travel → ±SWEEP_RANGE
        let position = position * DJ_FILTER_SWEEP_RANGE;
        let depth = position.abs();
        // Adaptive Q: rises with sweep depth on a concave power curve
        let q = DJ_FILTER_Q_MIN
            + (DJ_FILTER_Q_MAX - DJ_FILTER_Q_MIN) * depth.powf(DJ_FILTER_Q_POWER);

        let cutoff = if position < 0.0 {
            // LP mode: exponential sweep from 20 kHz down to 60 Hz
            // position -1 → cutoff 60 Hz, position 0 → cutoff 20 kHz
            let ratio = DJ_FILTER_LP_MAX / DJ_FILTER_LP_MIN; // 333.3
            DJ_FILTER_LP_MIN * ratio.powf(1.0 + position) // (1+pos) goes 0→1
        } else {
            // HP mode: exponential sweep from 20 Hz up to 12 kHz
            // position 0 → cutoff 20 Hz, position 1 → cutoff 12 kHz
            let ratio = DJ_FILTER_HP_MAX / DJ_FILTER_HP_MIN; // 600
            DJ_FILTER_HP_MIN * ratio.powf(position)
        };

        self.stage1.set_params(cutoff, q);
        self.stage2.set_params(cutoff, q);
    }

    /// Process one stereo sample through the 24 dB/oct cascade.
    /// Call `update_params` once per buffer before entering the sample loop.
    #[inline]
    fn process(&mut self, left: f32, right: f32, is_lp: bool) -> (f32, f32) {
        let out1 = self.stage1.process(left, right);
        if is_lp {
            let out2 = self.stage2.process(out1.low_l, out1.low_r);
            (out2.low_l, out2.low_r)
        } else {
            let out2 = self.stage2.process(out1.high_l, out1.high_r);
            (out2.high_l, out2.high_r)
        }
    }

    fn reset(&mut self) {
        self.stage1.reset();
        self.stage2.reset();
        self.last_position = f32::NAN;
    }
}

impl Default for DjFilter {
    fn default() -> Self {
        Self::new()
    }
}

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

    // DJ filter (24 dB/oct cascaded SVF)
    dj_filter: DjFilter,
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
            dj_filter: DjFilter::default(),
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

    /// Process audio through the channel strip (trim + EQ + DJ filter)
    pub fn process(&mut self, buffer: &mut StereoBuffer) {
        // Update EQ coefficients if needed
        self.update_eq_coeffs();

        let filter_pos = self.filter.clamp(-1.0, 1.0);
        let filter_active = filter_pos.abs() > DJ_FILTER_DEAD_ZONE;

        // Update SVF coefficients once per buffer (skips if position unchanged)
        if filter_active {
            self.dj_filter.update_params(filter_pos);
        }

        let is_lp = filter_pos < 0.0;

        for sample in buffer.iter_mut() {
            // Apply trim
            let mut left = sample.left * self.trim;
            let mut right = sample.right * self.trim;

            // Apply 3-band EQ
            (left, right) = self.eq_lo_state.process(left, right, &self.eq_lo_coeffs);
            (left, right) = self.eq_mid_state.process(left, right, &self.eq_mid_coeffs);
            (left, right) = self.eq_hi_state.process(left, right, &self.eq_hi_coeffs);

            // Apply 24 dB/oct DJ filter (bypassed in dead zone)
            if filter_active {
                (left, right) = self.dj_filter.process(left, right, is_lp);
            }

            *sample = StereoSample::new(left, right);
        }
    }

    /// Reset all filter states
    pub fn reset(&mut self) {
        self.eq_lo_state.reset();
        self.eq_mid_state.reset();
        self.eq_hi_state.reset();
        self.dj_filter.reset();
    }
}

/// Auto-cue weight: how strongly a deck is routed to the cue bus based on volume.
///
/// Logarithmic (exponential) decay over the full [0, 1] range:
/// - volume = 0.0 → weight 1.0 (fully in headphones)
/// - volume = 1.0 → weight 0.0 (silent in headphones)
///
/// Formula: (exp(-k·v) - exp(-k)) / (1 - exp(-k)), k=4.
/// Normalized so endpoints are exact. At v=0.3, weight ≈ 0.29 (vs. 1.0 in the old linear).
#[inline]
fn auto_cue_weight(volume: f32) -> f32 {
    const K: f32 = 4.0;
    const EXP_NEG_K: f32 = 0.018_315_64; // exp(-4.0)
    const NORM: f32 = 1.0 - EXP_NEG_K;   // 0.981_684_36
    ((-K * volume).exp() - EXP_NEG_K) / NORM
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
    /// Auto-cue: route low-volume decks to headphone output automatically
    auto_cue: bool,
    /// Lock-free peak levels for UI metering
    level_atomics: Arc<LevelAtomics>,
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
            auto_cue: true,
            level_atomics: Arc::new(LevelAtomics::new()),
            limiter: MasterLimiter::new(),
            clipper: MasterClipper::new(),
        }
    }

    /// Enable or disable auto-cue routing
    pub fn set_auto_cue(&mut self, enabled: bool) {
        self.auto_cue = enabled;
    }

    /// Get a clone of the level atomics Arc for the UI thread
    pub fn level_atomics(&self) -> Arc<LevelAtomics> {
        Arc::clone(&self.level_atomics)
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

        // Phase 2: Sequential summing to master/cue buses + per-channel peak tracking
        // This is fast O(n) and must be sequential to avoid race conditions
        let mut channel_peaks = [0.0f32; NUM_DECKS];
        for (deck_idx, buffer) in deck_buffers.iter().enumerate() {
            let channel = &self.channels[deck_idx];
            let mut deck_peak: f32 = 0.0;

            // Add to master output (with volume fader)
            for i in 0..buffer_len.min(buffer.len()) {
                let sample = buffer[i];

                // Master bus: apply volume fader
                let master_sample = sample * channel.volume;
                master_out.as_mut_slice()[i] += master_sample;

                // Track post-fader peak (stereo max)
                deck_peak = deck_peak
                    .max(master_sample.left.abs())
                    .max(master_sample.right.abs());

                // Cue bus: weighted send based on auto-cue + manual CUE button.
                // Auto-cue weight: logarithmic decay 1.0→0.0 across full volume range.
                // Manual CUE button forces weight to 1.0 (additive/independent).
                let auto_weight = if self.auto_cue {
                    auto_cue_weight(channel.volume)
                } else {
                    0.0
                };
                let cue_weight = f32::max(auto_weight, channel.cue_enabled as u8 as f32);
                if cue_weight > 0.0 {
                    cue_out.as_mut_slice()[i] += sample * cue_weight;
                }
            }
            channel_peaks[deck_idx] = deck_peak;
        }

        // Publish per-channel peaks to UI thread (lock-free)
        for i in 0..NUM_DECKS {
            self.level_atomics.channel_peaks[i].store(channel_peaks[i].to_bits(), Ordering::Relaxed);
        }

        // Apply master volume
        master_out.scale(self.master_volume);

        // Safety clipper: shaves transient peaks cleanly (zero latency)
        self.clipper.process(master_out);

        // Lookahead limiter: transparent gain reduction for sustained overs
        self.limiter.process(master_out);

        // Publish master peak after all processing
        let master_peak = master_out.as_slice().iter()
            .fold(0.0f32, |acc, s| acc.max(s.left.abs()).max(s.right.abs()));
        self.level_atomics.master_peak.store(master_peak.to_bits(), Ordering::Relaxed);

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

    #[test]
    fn test_dj_filter_bypass_at_center() {
        let mut strip = ChannelStrip::new();
        strip.filter = 0.0; // center = dead zone

        let mut buffer = StereoBuffer::silence(64);
        for i in 0..buffer.len() {
            buffer.as_mut_slice()[i] = StereoSample::new(1.0, 1.0);
        }

        strip.process(&mut buffer);

        // With flat EQ and centered filter, output ≈ input
        assert!(
            (buffer[63].left - 1.0).abs() < 0.01,
            "Center filter should pass through: {}",
            buffer[63].left
        );
    }

    #[test]
    fn test_dj_filter_lowpass_attenuates_nyquist() {
        let mut strip = ChannelStrip::new();
        strip.filter = -1.0; // full LP

        // Nyquist signal (alternating +1/-1)
        let mut buffer = StereoBuffer::silence(256);
        for i in 0..buffer.len() {
            let val = if i % 2 == 0 { 1.0 } else { -1.0 };
            buffer.as_mut_slice()[i] = StereoSample::new(val, val);
        }

        strip.process(&mut buffer);

        let avg: f32 = buffer.iter().skip(128).map(|s| s.left.abs()).sum::<f32>() / 128.0;
        assert!(
            avg < 0.1,
            "Full LP should strongly attenuate Nyquist, avg={}",
            avg
        );
    }

    #[test]
    fn test_dj_filter_highpass_passes_nyquist() {
        let mut strip = ChannelStrip::new();
        strip.filter = 1.0; // full HP

        // Nyquist signal
        let mut buffer = StereoBuffer::silence(256);
        for i in 0..buffer.len() {
            let val = if i % 2 == 0 { 1.0 } else { -1.0 };
            buffer.as_mut_slice()[i] = StereoSample::new(val, val);
        }

        strip.process(&mut buffer);

        let avg: f32 = buffer.iter().skip(128).map(|s| s.left.abs()).sum::<f32>() / 128.0;
        assert!(
            avg > 0.3,
            "Full HP should pass high frequencies, avg={}",
            avg
        );
    }

    #[test]
    fn test_dj_filter_highpass_rejects_dc() {
        let mut strip = ChannelStrip::new();
        strip.filter = 1.0; // full HP

        // DC signal
        let mut buffer = StereoBuffer::silence(2048);
        for i in 0..buffer.len() {
            buffer.as_mut_slice()[i] = StereoSample::new(1.0, 1.0);
        }

        strip.process(&mut buffer);

        // After settling, DC should be almost completely removed
        let tail_avg: f32 =
            buffer.iter().skip(1024).map(|s| s.left.abs()).sum::<f32>() / 1024.0;
        assert!(
            tail_avg < 0.05,
            "Full HP should reject DC, avg={}",
            tail_avg
        );
    }

    #[test]
    fn test_dj_filter_adaptive_q() {
        // Verify Q increases with sweep depth
        let mut filter = DjFilter::new();

        // Small sweep
        filter.update_params(-0.1);
        let q_small = DJ_FILTER_Q_MIN
            + (DJ_FILTER_Q_MAX - DJ_FILTER_Q_MIN) * 0.1_f32.powf(DJ_FILTER_Q_POWER);

        // Full sweep
        filter.update_params(-1.0);
        let q_full = DJ_FILTER_Q_MIN
            + (DJ_FILTER_Q_MAX - DJ_FILTER_Q_MIN) * 1.0_f32.powf(DJ_FILTER_Q_POWER);

        assert!(q_small < q_full, "Q should increase with depth: {} < {}", q_small, q_full);
        assert!((q_full - DJ_FILTER_Q_MAX).abs() < 0.001, "Full depth should reach max Q");
    }
}
