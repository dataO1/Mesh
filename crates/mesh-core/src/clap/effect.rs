//! CLAP Effect implementation
//!
//! Wraps a CLAP plugin as a mesh Effect, enabling integration with
//! the effect chain system.

use std::sync::{Arc, Mutex};

use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::StereoBuffer;

use super::discovery::DiscoveredClapPlugin;
use super::error::ClapResult;
use super::plugin::{ClapPluginWrapper, ParamChangeReceiver, CLAP_BUFFER_SIZE, CLAP_SAMPLE_RATE};

/// A CLAP plugin wrapped as a mesh Effect
///
/// This provides the bridge between CLAP plugins and mesh's effect system,
/// handling:
/// - Effect trait implementation
/// - Parameter querying (all CLAP params exposed via Effect::info().params)
/// - Thread-safe access via Arc<Mutex<>>
/// - Latency reporting for compensation
/// - Parameter change notifications for learning mode
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
    /// CLAP parameter IDs (maps param index to CLAP param ID)
    clap_param_ids: Vec<u32>,
    /// Pending parameter changes to send to plugin (param_id, value)
    pending_param_changes: Vec<(u32, f64)>,
    /// Receiver for parameter change notifications from plugin GUI
    param_change_receiver: ParamChangeReceiver,
}

impl ClapEffect {
    /// Create a new CLAP effect from a plugin wrapper and param change receiver
    pub fn new(
        mut wrapper: ClapPluginWrapper,
        param_change_receiver: ParamChangeReceiver,
    ) -> ClapResult<Self> {
        let plugin_info = wrapper.info().clone();
        let latency = wrapper.latency();

        // Query all parameters from the plugin
        let clap_params = wrapper.query_params();

        // Build EffectInfo from actual plugin parameters
        let mut info = EffectInfo::new(&plugin_info.name, plugin_info.category_name());
        let mut clap_param_ids = Vec::with_capacity(clap_params.len());

        if clap_params.is_empty() {
            // Plugin doesn't expose params or doesn't support extension
            // Create 8 generic params as fallback
            log::info!(
                "CLAP plugin '{}' has no params, creating 8 generic placeholders",
                plugin_info.id
            );
            for i in 0..8 {
                info = info.with_param(ParamInfo::new(format!("Param {}", i + 1), 0.5).with_range(0.0, 1.0));
                clap_param_ids.push(i as u32);
            }
        } else {
            log::info!(
                "CLAP plugin '{}' exposes {} parameters",
                plugin_info.id,
                clap_params.len()
            );
            for param in &clap_params {
                // Normalize default value to 0.0-1.0 range
                let range = param.max - param.min;
                let default_normalized = if range > 0.0 {
                    ((param.default - param.min) / range) as f32
                } else {
                    0.5
                };

                info = info.with_param(
                    ParamInfo::new(&param.name, default_normalized)
                        .with_range(param.min as f32, param.max as f32),
                );
                clap_param_ids.push(param.id);
            }
        }

        let base = EffectBase::new(info);
        let process_buffer = vec![0.0; CLAP_BUFFER_SIZE as usize * 2];

        Ok(Self {
            base,
            wrapper: Arc::new(Mutex::new(wrapper)),
            plugin_id: plugin_info.id,
            latency,
            process_buffer,
            clap_param_ids,
            pending_param_changes: Vec::new(),
            param_change_receiver,
        })
    }

    /// Create and activate a CLAP effect from a discovered plugin
    pub fn from_plugin(
        plugin_info: &DiscoveredClapPlugin,
        bundle: Arc<clack_host::bundle::PluginBundle>,
    ) -> ClapResult<Self> {
        let (mut wrapper, receiver) = ClapPluginWrapper::new(plugin_info, bundle)?;
        wrapper.activate(CLAP_SAMPLE_RATE, CLAP_BUFFER_SIZE)?;
        Self::new(wrapper, receiver)
    }

    /// Create and activate a CLAP effect, returning both effect and receiver separately
    ///
    /// This variant is used when the param change receiver needs to go to a GUI handle
    /// instead of staying in the effect. The effect gets a dummy receiver.
    pub fn from_plugin_with_separate_receiver(
        plugin_info: &DiscoveredClapPlugin,
        bundle: Arc<clack_host::bundle::PluginBundle>,
    ) -> ClapResult<(Self, ParamChangeReceiver)> {
        let (mut wrapper, receiver) = ClapPluginWrapper::new(plugin_info, bundle)?;
        wrapper.activate(CLAP_SAMPLE_RATE, CLAP_BUFFER_SIZE)?;

        // Create a dummy channel for the effect - it won't receive anything
        // since the real receiver is being returned for the GUI handle
        let (_dummy_sender, dummy_receiver) = crossbeam::channel::bounded(1);

        let effect = Self::new(wrapper, dummy_receiver)?;
        Ok((effect, receiver))
    }

    /// Get the plugin ID
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Get access to the underlying wrapper (for advanced operations)
    pub fn wrapper(&self) -> &Arc<Mutex<ClapPluginWrapper>> {
        &self.wrapper
    }

    /// Get the CLAP parameter info for all parameters
    pub fn clap_param_ids(&self) -> &[u32] {
        &self.clap_param_ids
    }

    /// Poll for parameter changes from the plugin GUI
    ///
    /// Returns all pending parameter changes. Call this periodically from the UI
    /// thread to detect when the plugin's GUI modifies parameters (for learning mode).
    ///
    /// Each change contains the CLAP param_id and the new value in the plugin's
    /// native range.
    pub fn poll_param_changes(&self) -> Vec<super::plugin::ParamChangeEvent> {
        let mut changes = Vec::new();
        while let Ok(change) = self.param_change_receiver.try_recv() {
            changes.push(change);
        }
        changes
    }

    /// Get the parameter name for a CLAP param ID
    ///
    /// Returns None if the param ID is not found.
    pub fn param_name_for_id(&self, param_id: u32) -> Option<&str> {
        self.clap_param_ids
            .iter()
            .position(|&id| id == param_id)
            .and_then(|idx| self.base.info().params.get(idx))
            .map(|p| p.name.as_str())
    }

    /// Get the parameter index for a CLAP param ID
    ///
    /// Returns None if the param ID is not found.
    pub fn param_index_for_id(&self, param_id: u32) -> Option<usize> {
        self.clap_param_ids.iter().position(|&id| id == param_id)
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
                log::trace!(
                    "CLAP effect '{}': lock contention, skipping frame",
                    self.plugin_id
                );
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

        // Drain pending param changes and process through CLAP plugin
        let param_changes: Vec<(u32, f64)> = self.pending_param_changes.drain(..).collect();
        let result = if param_changes.is_empty() {
            wrapper.process(&self.process_buffer[..interleaved_size], interleaved)
        } else {
            wrapper.process_with_params(
                &self.process_buffer[..interleaved_size],
                interleaved,
                &param_changes,
            )
        };

        if let Err(e) = result {
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
        // Update our local state
        self.base.set_param(index, value);

        // Queue parameter change for the plugin
        if let Some(&param_id) = self.clap_param_ids.get(index) {
            // Get the actual (denormalized) value
            let actual_value = self.base.get_params().get(index).map(|p| p.actual).unwrap_or(value);
            self.pending_param_changes.push((param_id, actual_value as f64));
        }
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
    #[test]
    fn test_clap_param_id_storage() {
        // Basic test that the types are correct
        let ids: Vec<u32> = vec![1, 2, 3];
        assert_eq!(ids.len(), 3);
    }
}
