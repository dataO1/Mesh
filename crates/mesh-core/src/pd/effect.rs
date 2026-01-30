//! PdEffect - Pure Data effect implementing the Effect trait
//!
//! Wraps a PD patch as a mesh effect, enabling PD patches to be used
//! in effect chains alongside native Rust effects.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::StereoBuffer;

use super::error::{PdError, PdResult};
use super::instance::{PatchHandle, PdInstance};
use super::metadata::EffectMetadata;

/// A Pure Data effect that implements the Effect trait
///
/// Each PdEffect wraps a single PD patch and manages communication
/// via instance-scoped receives ($0-param0, $0-bypass, etc.).
pub struct PdEffect {
    /// Effect base (info, params, bypass state)
    base: EffectBase,

    /// Reference to the PD instance (shared per deck)
    instance: Arc<Mutex<PdInstance>>,

    /// Handle to the open patch (stored for $0 reference)
    patch_handle: Option<PatchHandle>,

    /// Path to the patch file (for error messages)
    patch_path: PathBuf,

    /// The $0 value for this patch (for instance-scoped receives)
    dollar_zero: i32,

    /// Fixed latency in samples (from metadata, scaled to current sample rate)
    latency: u32,

    /// Temporary input buffer for libpd (interleaved stereo)
    input_buffer: Vec<f32>,

    /// Temporary output buffer for libpd (interleaved stereo)
    output_buffer: Vec<f32>,

    /// Effect ID (folder name)
    effect_id: String,
}

impl PdEffect {
    /// Create a new PD effect from metadata
    ///
    /// # Arguments
    /// * `instance` - Shared reference to the deck's PD instance
    /// * `patch_path` - Path to the .pd patch file
    /// * `metadata` - Effect metadata from metadata.json
    /// * `effect_id` - Effect identifier (folder name)
    pub fn new(
        instance: Arc<Mutex<PdInstance>>,
        patch_path: PathBuf,
        metadata: &EffectMetadata,
        effect_id: String,
    ) -> PdResult<Self> {
        // Build EffectInfo from metadata
        let mut info = EffectInfo::new(&metadata.name, &metadata.category);

        for param_meta in &metadata.params {
            let mut param_info = ParamInfo::new(&param_meta.name, param_meta.default);

            if let (Some(min), Some(max)) = (param_meta.min, param_meta.max) {
                param_info = param_info.with_range(min, max);
            }

            if let Some(ref unit) = param_meta.unit {
                param_info = param_info.with_unit(unit);
            }

            info = info.with_param(param_info);
        }

        let base = EffectBase::new(info);

        // Get sample rate from instance to calculate latency
        let sample_rate = {
            let inst = instance.lock().map_err(|_| {
                PdError::InitializationFailed("Failed to lock PD instance".to_string())
            })?;
            inst.sample_rate() as u32
        };

        let latency = metadata.latency_at_sample_rate(sample_rate);

        // Pre-allocate buffers (will resize as needed during processing)
        let buffer_capacity = 4096 * 2; // Stereo interleaved
        let input_buffer = vec![0.0f32; buffer_capacity];
        let output_buffer = vec![0.0f32; buffer_capacity];

        Ok(Self {
            base,
            instance,
            patch_handle: None,
            patch_path,
            dollar_zero: 0,
            latency,
            input_buffer,
            output_buffer,
            effect_id,
        })
    }

    /// Open the PD patch
    ///
    /// Must be called before processing. Separated from new() to allow
    /// effect chain setup before patch loading.
    pub fn open(&mut self) -> PdResult<()> {
        if self.patch_handle.is_some() {
            return Ok(()); // Already open
        }

        let mut instance = self.instance.lock().map_err(|_| {
            PdError::PatchOpenFailed {
                path: self.patch_path.clone(),
                reason: "Failed to lock PD instance".to_string(),
            }
        })?;

        let handle = instance.open_patch(&self.patch_path)?;
        self.dollar_zero = handle.dollar_zero;
        self.patch_handle = Some(handle);

        // Activate audio processing
        instance.set_audio_active(true)?;

        // Send initial parameter values (release lock first)
        drop(instance);
        self.send_all_params()?;

        log::info!(
            "PdEffect '{}' opened (path={}, $0={})",
            self.effect_id,
            self.patch_path.display(),
            self.dollar_zero
        );

        Ok(())
    }

    /// Close the PD patch
    pub fn close(&mut self) -> PdResult<()> {
        if self.patch_handle.take().is_some() {
            let mut instance = self.instance.lock().map_err(|_| {
                PdError::PatchCloseFailed("Failed to lock PD instance".to_string())
            })?;

            instance.close_patch()?;
            // Note: close_patch() closes the currently open patch

            log::info!("PdEffect '{}' closed", self.effect_id);
        }

        Ok(())
    }

    /// Send all current parameter values to the patch
    fn send_all_params(&self) -> PdResult<()> {
        let instance = self.instance.lock().map_err(|_| {
            PdError::SendFailed {
                msg_type: "params".to_string(),
                receiver: "all".to_string(),
                reason: "Failed to lock PD instance".to_string(),
            }
        })?;

        for (i, param) in self.base.get_params().iter().enumerate() {
            let receiver = format!("{}-param{}", self.dollar_zero, i);
            instance.send_float(&receiver, param.normalized)?;
        }

        // Send initial bypass state
        let bypass_receiver = format!("{}-bypass", self.dollar_zero);
        let bypass_value = if self.base.is_bypassed() { 1.0 } else { 0.0 };
        instance.send_float(&bypass_receiver, bypass_value)?;

        Ok(())
    }

    /// Send a single parameter value to the patch
    fn send_param(&self, index: usize, value: f32) -> PdResult<()> {
        if self.patch_handle.is_none() {
            return Ok(()); // Not open yet, will send on open
        }

        let instance = self.instance.lock().map_err(|_| {
            PdError::SendFailed {
                msg_type: "float".to_string(),
                receiver: format!("param{}", index),
                reason: "Failed to lock PD instance".to_string(),
            }
        })?;

        let receiver = format!("{}-param{}", self.dollar_zero, index);
        instance.send_float(&receiver, value)
    }

    /// Send bypass state to the patch
    fn send_bypass(&self, bypass: bool) -> PdResult<()> {
        if self.patch_handle.is_none() {
            return Ok(()); // Not open yet, will send on open
        }

        let instance = self.instance.lock().map_err(|_| {
            PdError::SendFailed {
                msg_type: "float".to_string(),
                receiver: "bypass".to_string(),
                reason: "Failed to lock PD instance".to_string(),
            }
        })?;

        let receiver = format!("{}-bypass", self.dollar_zero);
        instance.send_float(&receiver, if bypass { 1.0 } else { 0.0 })
    }

    /// Get the effect ID
    pub fn effect_id(&self) -> &str {
        &self.effect_id
    }

    /// Check if the patch is open
    pub fn is_open(&self) -> bool {
        self.patch_handle.is_some()
    }
}

impl Effect for PdEffect {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        // Skip processing if bypassed (mesh handles bypass at chain level,
        // but we also tell PD for patches that implement their own bypass)
        if self.base.is_bypassed() {
            return;
        }

        // Skip if patch not open
        if self.patch_handle.is_none() {
            return;
        }

        let sample_count = buffer.len();
        if sample_count == 0 {
            return;
        }

        // Ensure buffers are large enough
        let interleaved_size = sample_count * 2;
        if self.input_buffer.len() < interleaved_size {
            self.input_buffer.resize(interleaved_size, 0.0);
            self.output_buffer.resize(interleaved_size, 0.0);
        }

        // Copy input to our buffer (StereoBuffer's as_interleaved is zero-copy view)
        let input_slice = buffer.as_interleaved();
        self.input_buffer[..interleaved_size].copy_from_slice(&input_slice[..interleaved_size]);

        // Process through libpd
        let instance = match self.instance.lock() {
            Ok(inst) => inst,
            Err(_) => {
                log::warn!("Failed to lock PD instance during processing");
                return;
            }
        };

        let _ = instance.process(
            &self.input_buffer[..interleaved_size],
            &mut self.output_buffer[..interleaved_size],
        );

        // Release lock before copying back
        drop(instance);

        // Copy output back to StereoBuffer using the mutable interleaved view
        let output_slice = buffer.as_interleaved_mut();
        output_slice[..interleaved_size].copy_from_slice(&self.output_buffer[..interleaved_size]);
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

        // Send to PD (ignore errors during param updates)
        if let Err(e) = self.send_param(index, value) {
            log::warn!("Failed to send param {} to PD: {}", index, e);
        }
    }

    fn set_bypass(&mut self, bypass: bool) {
        self.base.set_bypass(bypass);

        // Send to PD (ignore errors during bypass updates)
        if let Err(e) = self.send_bypass(bypass) {
            log::warn!("Failed to send bypass to PD: {}", e);
        }
    }

    fn is_bypassed(&self) -> bool {
        self.base.is_bypassed()
    }

    fn reset(&mut self) {
        // Re-send all parameters to reset PD state
        if let Err(e) = self.send_all_params() {
            log::warn!("Failed to reset PD effect params: {}", e);
        }
    }
}

impl Drop for PdEffect {
    fn drop(&mut self) {
        if let Err(e) = self.close() {
            log::warn!("Error closing PD effect: {}", e);
        }
    }
}

// PdEffect is Send because:
// - EffectBase is Send
// - Arc<Mutex<PdInstance>> is Send
// - Other fields are owned data
unsafe impl Send for PdEffect {}

#[cfg(test)]
mod tests {
    #[test]
    fn test_receiver_format() {
        let dollar_zero = 1001;
        let receiver = format!("{}-param{}", dollar_zero, 0);
        assert_eq!(receiver, "1001-param0");

        let bypass_receiver = format!("{}-bypass", dollar_zero);
        assert_eq!(bypass_receiver, "1001-bypass");
    }
}
