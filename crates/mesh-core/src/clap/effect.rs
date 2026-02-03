//! CLAP Effect implementation
//!
//! Wraps a CLAP plugin as a mesh Effect, enabling integration with
//! the effect chain system.

use std::sync::{Arc, Mutex};

use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::StereoBuffer;

use super::error::ClapResult;
use super::plugin::{ClapPluginWrapper, CLAP_SAMPLE_RATE, CLAP_BUFFER_SIZE};
use super::discovery::DiscoveredClapPlugin;

/// Maximum parameters exposed to mesh (maps to 8 hardware knobs)
pub const MAX_CLAP_PARAMS: usize = 8;

/// A CLAP plugin wrapped as a mesh Effect
///
/// This provides the bridge between CLAP plugins and mesh's effect system,
/// handling:
/// - Effect trait implementation
/// - Parameter mapping (CLAP params to mesh's 8-knob system)
/// - Thread-safe access via Arc<Mutex<>>
/// - Latency reporting for compensation
pub struct ClapEffect {
    /// Effect base (info, params, bypass state)
    base: EffectBase,
    /// The wrapped CLAP plugin
    wrapper: Arc<Mutex<ClapPluginWrapper>>,
    /// Plugin ID for error messages
    plugin_id: String,
    /// Cached latency
    latency: u32,
    /// Interleaved audio buffer for processing
    process_buffer: Vec<f32>,
}

impl ClapEffect {
    /// Create a new CLAP effect from a plugin wrapper
    pub fn new(wrapper: ClapPluginWrapper) -> ClapResult<Self> {
        let plugin_info = wrapper.info().clone();
        let latency = wrapper.latency();

        // Build EffectInfo from plugin metadata
        let mut info = EffectInfo::new(&plugin_info.name, plugin_info.category_name());

        // For now, we expose up to 8 generic parameters
        // In a full implementation, we'd query the plugin's actual parameters
        // and map the first 8 (or allow user-configurable mapping)
        for i in 0..MAX_CLAP_PARAMS.min(8) {
            info = info.with_param(
                ParamInfo::new(format!("Param {}", i + 1), 0.5)
                    .with_range(0.0, 1.0)
            );
        }

        let base = EffectBase::new(info);
        let process_buffer = vec![0.0; CLAP_BUFFER_SIZE as usize * 2];

        Ok(Self {
            base,
            wrapper: Arc::new(Mutex::new(wrapper)),
            plugin_id: plugin_info.id,
            latency,
            process_buffer,
        })
    }

    /// Create and activate a CLAP effect from a discovered plugin
    pub fn from_plugin(
        plugin_info: &DiscoveredClapPlugin,
        bundle: Arc<clack_host::bundle::PluginBundle>,
    ) -> ClapResult<Self> {
        let mut wrapper = ClapPluginWrapper::new(plugin_info, bundle)?;
        wrapper.activate(CLAP_SAMPLE_RATE, CLAP_BUFFER_SIZE)?;
        Self::new(wrapper)
    }

    /// Get the plugin ID
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Get access to the underlying wrapper (for advanced operations)
    pub fn wrapper(&self) -> &Arc<Mutex<ClapPluginWrapper>> {
        &self.wrapper
    }
}

impl Effect for ClapEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }

        // Try to acquire lock - if we can't, skip this frame (RT-safe)
        let mut wrapper = match self.wrapper.try_lock() {
            Ok(w) => w,
            Err(_) => {
                log::trace!("CLAP effect '{}': lock contention, skipping frame", self.plugin_id);
                return;
            }
        };

        // Ensure our process buffer is large enough
        let sample_count = buffer.len();
        let interleaved_size = sample_count * 2;
        if self.process_buffer.len() < interleaved_size {
            self.process_buffer.resize(interleaved_size, 0.0);
        }

        // Get interleaved view of the buffer
        let interleaved = buffer.as_interleaved_mut();

        // Copy input to our buffer
        self.process_buffer[..interleaved_size].copy_from_slice(&interleaved[..interleaved_size]);

        // Process through CLAP plugin
        if let Err(e) = wrapper.process(&self.process_buffer[..interleaved_size], interleaved) {
            log::warn!("CLAP effect '{}' processing error: {}", self.plugin_id, e);
            // On error, buffer is left with input data (passthrough)
        }
    }

    fn latency_samples(&self) -> u32 {
        self.latency
    }

    fn info(&self) -> &EffectInfo {
        self.base.info()
    }

    fn get_params(&self) -> &[ParamValue] {
        self.base.get_params()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.base.set_param(index, value);
        // TODO: Send parameter to CLAP plugin via parameter events
        // This requires implementing the params extension
    }

    fn set_bypass(&mut self, bypass: bool) {
        self.base.set_bypass(bypass);
    }

    fn is_bypassed(&self) -> bool {
        self.base.is_bypassed()
    }

    fn reset(&mut self) {
        // CLAP plugins don't have a standard reset mechanism
        // We could deactivate and reactivate, but that's heavy
        // For now, do nothing - state is maintained
    }
}

// Safety: ClapEffect is Send because:
// - EffectBase is Send
// - Arc<Mutex<ClapPluginWrapper>> is Send
// - Other fields are owned data
unsafe impl Send for ClapEffect {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_params_constant() {
        assert_eq!(MAX_CLAP_PARAMS, 8);
    }
}
