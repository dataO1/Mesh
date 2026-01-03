//! Gain effect - Simple volume control

use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::StereoBuffer;

/// A simple gain (volume) effect
///
/// Parameters:
/// - Gain: Volume multiplier (0.0 = silence, 1.0 = unity, 2.0 = +6dB)
///
/// This effect has zero latency.
pub struct GainEffect {
    base: EffectBase,
}

impl GainEffect {
    /// Create a new gain effect
    pub fn new() -> Self {
        let info = EffectInfo::new("Gain", "Utility")
            .with_param(
                ParamInfo::new("Gain", 0.5) // 0.5 = unity (linear scale mapped to 0-2)
                    .with_range(0.0, 2.0)
                    .with_unit("×"),
            );

        Self {
            base: EffectBase::new(info),
        }
    }

    /// Get the current gain value
    fn gain(&self) -> f32 {
        self.base.param_actual(0)
    }
}

impl Default for GainEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for GainEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }

        let gain = self.gain();
        buffer.scale(gain);
    }

    fn latency_samples(&self) -> u32 {
        0 // Zero latency
    }

    fn info(&self) -> &EffectInfo {
        self.base.info()
    }

    fn get_params(&self) -> &[ParamValue] {
        self.base.get_params()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.base.set_param(index, value);
    }

    fn set_bypass(&mut self, bypass: bool) {
        self.base.set_bypass(bypass);
    }

    fn is_bypassed(&self) -> bool {
        self.base.is_bypassed()
    }

    fn reset(&mut self) {
        // No state to reset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StereoSample;

    #[test]
    fn test_gain_effect() {
        let mut effect = GainEffect::new();

        // Create test buffer
        let mut buffer = StereoBuffer::silence(4);
        buffer.as_mut_slice()[0] = StereoSample::new(1.0, 1.0);
        buffer.as_mut_slice()[1] = StereoSample::new(0.5, 0.5);

        // Default is unity gain (0.5 normalized = 1.0 actual)
        effect.process(&mut buffer);

        // Check output (should be unchanged at unity)
        assert!((buffer[0].left - 1.0).abs() < 0.001);
        assert!((buffer[1].left - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_gain_effect_half() {
        let mut effect = GainEffect::new();
        effect.set_param(0, 0.25); // 0.25 normalized = 0.5 actual (half volume)

        let mut buffer = StereoBuffer::silence(2);
        buffer.as_mut_slice()[0] = StereoSample::new(1.0, 1.0);

        effect.process(&mut buffer);

        assert!((buffer[0].left - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_gain_bypass() {
        let mut effect = GainEffect::new();
        effect.set_param(0, 0.0); // Zero gain
        effect.set_bypass(true);

        let mut buffer = StereoBuffer::silence(2);
        buffer.as_mut_slice()[0] = StereoSample::new(1.0, 1.0);

        effect.process(&mut buffer);

        // Bypassed, so original value should remain
        assert_eq!(buffer[0].left, 1.0);
    }
}
