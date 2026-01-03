//! Mixer - Combines deck outputs with volume/filter/cue controls

use crate::types::{StereoBuffer, StereoSample, NUM_DECKS, SAMPLE_RATE};

/// Channel strip state for a single deck
#[derive(Debug, Clone)]
pub struct ChannelStrip {
    /// Trim/gain control (-24 to +12 dB, stored as linear multiplier)
    pub trim: f32,
    /// Filter position (-1.0 = full LP, 0.0 = flat, 1.0 = full HP)
    pub filter: f32,
    /// Volume fader (0.0 to 1.0)
    pub volume: f32,
    /// Cue button state (routes to cue bus)
    pub cue_enabled: bool,

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
            filter: 0.0,     // Flat
            volume: 1.0,     // Full volume
            cue_enabled: false,
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

    /// Process audio through the channel strip (trim + filter)
    /// Returns the post-fader audio
    pub fn process(&mut self, buffer: &mut StereoBuffer) {
        // Filter coefficient based on position
        // Simple one-pole filter crossfade
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

    /// Reset filter state
    pub fn reset(&mut self) {
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
}

impl Mixer {
    /// Create a new mixer
    pub fn new() -> Self {
        Self {
            channels: std::array::from_fn(|_| ChannelStrip::new()),
            master_volume: 1.0,
            cue_mix: 0.5,
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

    /// Process deck outputs and produce master + cue outputs
    ///
    /// deck_buffers: Array of processed deck outputs
    /// master_out: Output buffer for master mix
    /// cue_out: Output buffer for cue/headphone mix
    pub fn process(
        &mut self,
        deck_buffers: &mut [StereoBuffer; NUM_DECKS],
        master_out: &mut StereoBuffer,
        cue_out: &mut StereoBuffer,
    ) {
        let buffer_len = master_out.len();
        master_out.fill_silence();
        cue_out.fill_silence();

        for (deck_idx, buffer) in deck_buffers.iter_mut().enumerate() {
            let channel = &mut self.channels[deck_idx];

            // Process through channel strip (trim + filter)
            channel.process(buffer);

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

        // Mix cue/master for headphones (cue_out becomes the headphone output)
        for i in 0..buffer_len {
            let master = master_out[i];
            let cue = cue_out[i];

            // Crossfade between cue and master
            cue_out.as_mut_slice()[i] = StereoSample::new(
                cue.left * (1.0 - self.cue_mix) + master.left * self.cue_mix,
                cue.right * (1.0 - self.cue_mix) + master.right * self.cue_mix,
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
        assert_eq!(mixer.cue_mix(), 0.5);
    }
}
