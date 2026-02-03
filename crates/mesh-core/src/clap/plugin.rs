//! Low-level CLAP plugin wrapper using clack-host
//!
//! This module provides the core plugin hosting functionality, wrapping
//! clack-host's API into a mesh-friendly interface.

use std::ffi::CString;
use std::sync::Arc;

use clack_host::prelude::*;
use clack_host::bundle::PluginBundle;
use clack_host::process::StartedPluginAudioProcessor;

use super::error::{ClapError, ClapResult};
use super::discovery::DiscoveredClapPlugin;

/// Sample rate used for CLAP plugins
pub const CLAP_SAMPLE_RATE: u32 = 48000;

/// Default buffer size for processing
pub const CLAP_BUFFER_SIZE: u32 = 256;

/// Maximum buffer size we'll allocate for
pub const CLAP_MAX_BUFFER_SIZE: u32 = 4096;

// ============================================================================
// Host Implementation
// ============================================================================

/// Mesh's CLAP host implementation
pub struct MeshClapHost;

impl HostHandlers for MeshClapHost {
    type Shared<'a> = MeshClapHostShared;
    type MainThread<'a> = MeshClapHostMainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        // We could register extensions here like HostLog, HostParams, etc.
        // For now, keep it minimal for initial implementation
        let _ = builder;
    }
}

/// Shared host data accessible from any thread
pub struct MeshClapHostShared {
    /// Plugin ID for logging
    plugin_id: String,
}

impl MeshClapHostShared {
    fn new(plugin_id: String) -> Self {
        Self { plugin_id }
    }
}

impl<'a> SharedHandler<'a> for MeshClapHostShared {
    fn initializing(&self, _instance: InitializingPluginHandle<'a>) {
        log::debug!("CLAP plugin '{}' initializing", self.plugin_id);
    }

    fn request_restart(&self) {
        log::debug!("CLAP plugin '{}' requested restart (ignored)", self.plugin_id);
    }

    fn request_process(&self) {
        // We're always processing, so this is a no-op
    }

    fn request_callback(&self) {
        // We don't support main-thread callbacks in the audio thread context
        log::trace!("CLAP plugin '{}' requested callback (ignored)", self.plugin_id);
    }
}

/// Main thread host data
pub struct MeshClapHostMainThread<'a> {
    _shared: &'a MeshClapHostShared,
    #[allow(dead_code)]
    plugin: Option<InitializedPluginHandle<'a>>,
}

impl<'a> MeshClapHostMainThread<'a> {
    fn new(shared: &'a MeshClapHostShared) -> Self {
        Self {
            _shared: shared,
            plugin: None,
        }
    }
}

impl<'a> MainThreadHandler<'a> for MeshClapHostMainThread<'a> {
    fn initialized(&mut self, instance: InitializedPluginHandle<'a>) {
        self.plugin = Some(instance);
    }
}

// ============================================================================
// Plugin Wrapper
// ============================================================================

/// Information about a CLAP plugin's parameter
#[derive(Debug, Clone)]
pub struct ClapParamInfo {
    /// CLAP parameter ID
    pub id: u32,
    /// Parameter name
    pub name: String,
    /// Minimum value
    pub min: f64,
    /// Maximum value
    pub max: f64,
    /// Default value
    pub default: f64,
}

/// Wrapper around a loaded and activated CLAP plugin
///
/// This handles the plugin lifecycle and provides a simplified interface
/// for audio processing.
pub struct ClapPluginWrapper {
    /// The plugin instance
    instance: Option<PluginInstance<MeshClapHost>>,
    /// The audio processor (when activated)
    processor: Option<StartedPluginAudioProcessor<MeshClapHost>>,
    /// Plugin metadata
    info: DiscoveredClapPlugin,
    /// Audio ports for input
    input_ports: AudioPorts,
    /// Audio ports for output
    output_ports: AudioPorts,
    /// Input buffer (non-interleaved: [L, L, L, ..., R, R, R, ...])
    input_buffer: Vec<f32>,
    /// Output buffer (non-interleaved: [L, L, L, ..., R, R, R, ...])
    output_buffer: Vec<f32>,
    /// Current buffer size
    buffer_size: usize,
    /// Sample rate
    sample_rate: u32,
    /// Whether the plugin is activated
    activated: bool,
    /// Cached latency
    latency_samples: u32,
    /// Keep the bundle alive
    _bundle: Arc<PluginBundle>,
}

impl ClapPluginWrapper {
    /// Create a new plugin wrapper from a discovered plugin
    pub fn new(plugin_info: &DiscoveredClapPlugin, bundle: Arc<PluginBundle>) -> ClapResult<Self> {
        let plugin_id = CString::new(plugin_info.id.as_str()).map_err(|_| {
            ClapError::InstantiationFailed {
                plugin_id: plugin_info.id.clone(),
                reason: "Invalid plugin ID (contains null byte)".to_string(),
            }
        })?;

        let host_info = HostInfo::new("Mesh DJ", "Mesh", "https://github.com/mesh", "0.1.0")
            .map_err(|e| ClapError::InstantiationFailed {
                plugin_id: plugin_info.id.clone(),
                reason: format!("Failed to create host info: {:?}", e),
            })?;

        let cloned_id = plugin_info.id.clone();
        let instance = PluginInstance::<MeshClapHost>::new(
            |_| MeshClapHostShared::new(cloned_id.clone()),
            |shared| MeshClapHostMainThread::new(shared),
            &bundle,
            &plugin_id,
            &host_info,
        )
        .map_err(|e| ClapError::InstantiationFailed {
            plugin_id: plugin_info.id.clone(),
            reason: format!("{:?}", e),
        })?;

        let buffer_size = CLAP_BUFFER_SIZE as usize;
        let stereo_buffer_size = buffer_size * 2; // L and R channels

        Ok(Self {
            instance: Some(instance),
            processor: None,
            info: plugin_info.clone(),
            input_ports: AudioPorts::with_capacity(2, 1), // 2 channels, 1 port
            output_ports: AudioPorts::with_capacity(2, 1),
            input_buffer: vec![0.0; stereo_buffer_size],
            output_buffer: vec![0.0; stereo_buffer_size],
            buffer_size,
            sample_rate: CLAP_SAMPLE_RATE,
            activated: false,
            latency_samples: 0,
            _bundle: bundle,
        })
    }

    /// Activate the plugin for audio processing
    pub fn activate(&mut self, sample_rate: u32, buffer_size: u32) -> ClapResult<()> {
        if self.activated {
            return Ok(());
        }

        let mut instance = self.instance.take().ok_or_else(|| ClapError::NotActivated {
            plugin_id: self.info.id.clone(),
        })?;

        self.sample_rate = sample_rate;
        self.buffer_size = buffer_size as usize;

        // Resize buffers
        let stereo_buffer_size = self.buffer_size * 2;
        self.input_buffer.resize(stereo_buffer_size, 0.0);
        self.output_buffer.resize(stereo_buffer_size, 0.0);

        // Create audio configuration
        let audio_config = PluginAudioConfiguration {
            sample_rate: sample_rate as f64,
            min_frames_count: buffer_size,
            max_frames_count: CLAP_MAX_BUFFER_SIZE,
        };

        // Activate the plugin - returns a StoppedPluginAudioProcessor
        let stopped_processor = instance
            .activate(|_, _| (), audio_config)
            .map_err(|e| ClapError::ActivationFailed {
                plugin_id: self.info.id.clone(),
                reason: format!("{:?}", e),
            })?;

        // Start processing - consumes Stopped, returns Started
        let processor = stopped_processor.start_processing().map_err(|e| ClapError::ActivationFailed {
            plugin_id: self.info.id.clone(),
            reason: format!("Failed to start processing: {:?}", e),
        })?;

        // Query latency (if available)
        // For now, use 0 - we'd need the latency extension to query this
        self.latency_samples = 0;

        self.instance = Some(instance);
        self.processor = Some(processor);
        self.activated = true;

        log::info!(
            "CLAP plugin '{}' activated at {}Hz, buffer size {}",
            self.info.id,
            sample_rate,
            buffer_size
        );

        Ok(())
    }

    /// Deactivate the plugin
    pub fn deactivate(&mut self) {
        if let Some(processor) = self.processor.take() {
            // Stop processing - returns StoppedPluginAudioProcessor
            let stopped = processor.stop_processing();

            // Deactivate via the instance (takes the stopped processor)
            if let Some(ref mut instance) = self.instance {
                instance.deactivate(stopped);
            }

            self.activated = false;
            log::info!("CLAP plugin '{}' deactivated", self.info.id);
        }
    }

    /// Check if the plugin is activated
    pub fn is_activated(&self) -> bool {
        self.activated
    }

    /// Get the plugin's latency in samples
    pub fn latency(&self) -> u32 {
        self.latency_samples
    }

    /// Get plugin info
    pub fn info(&self) -> &DiscoveredClapPlugin {
        &self.info
    }

    /// Process audio through the plugin
    ///
    /// Takes interleaved stereo input and produces interleaved stereo output.
    /// The input buffer is copied, processed, and the result is written to output.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> ClapResult<()> {
        let processor = self.processor.as_mut().ok_or_else(|| ClapError::NotActivated {
            plugin_id: self.info.id.clone(),
        })?;

        let frame_count = input.len() / 2;
        if frame_count == 0 {
            return Ok(());
        }

        // Ensure our buffers are large enough
        let stereo_buffer_size = frame_count * 2;
        if self.input_buffer.len() < stereo_buffer_size {
            self.input_buffer.resize(stereo_buffer_size, 0.0);
            self.output_buffer.resize(stereo_buffer_size, 0.0);
        }

        // Deinterleave input: [L, R, L, R, ...] -> [L, L, L, ..., R, R, R, ...]
        for i in 0..frame_count {
            self.input_buffer[i] = input[i * 2];           // Left channel
            self.input_buffer[frame_count + i] = input[i * 2 + 1]; // Right channel
        }

        // Clear output buffer
        self.output_buffer[..stereo_buffer_size].fill(0.0);

        // Split buffers to get non-overlapping mutable references for L/R channels
        // This avoids the "cannot borrow as mutable more than once" error
        let (input_left, input_right) = self.input_buffer[..stereo_buffer_size].split_at_mut(frame_count);
        let (output_left, output_right) = self.output_buffer[..stereo_buffer_size].split_at_mut(frame_count);

        // Prepare input buffers
        let input_buffers = self.input_ports.with_input_buffers(std::iter::once(AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                [
                    InputChannel {
                        buffer: input_left,
                        is_constant: false,
                    },
                    InputChannel {
                        buffer: input_right,
                        is_constant: false,
                    },
                ]
                .into_iter(),
            ),
        }));

        // Prepare output buffers
        let mut output_buffers = self.output_ports.with_output_buffers(std::iter::once(AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                [
                    output_left,
                    output_right,
                ]
                .into_iter(),
            ),
        }));

        // Process
        processor
            .process(
                &input_buffers,
                &mut output_buffers,
                &InputEvents::empty(),
                &mut OutputEvents::void(),
                None, // steady time
                None, // transport
            )
            .map_err(|e| ClapError::ProcessingError {
                plugin_id: self.info.id.clone(),
                reason: format!("{:?}", e),
            })?;

        // Interleave output: [L, L, L, ..., R, R, R, ...] -> [L, R, L, R, ...]
        for i in 0..frame_count {
            output[i * 2] = self.output_buffer[i];           // Left channel
            output[i * 2 + 1] = self.output_buffer[frame_count + i]; // Right channel
        }

        Ok(())
    }

    /// Process audio in-place (reads from buffer, writes result back)
    pub fn process_inplace(&mut self, buffer: &mut [f32]) -> ClapResult<()> {
        // We need a temporary buffer for the output
        let len = buffer.len();
        let mut temp_output = vec![0.0; len];
        self.process(buffer, &mut temp_output)?;
        buffer.copy_from_slice(&temp_output);
        Ok(())
    }
}

impl Drop for ClapPluginWrapper {
    fn drop(&mut self) {
        self.deactivate();
    }
}

// Safety: ClapPluginWrapper is Send because:
// - All fields are owned or use thread-safe synchronization
// - clack-host's PluginInstance and StartedPluginAudioProcessor are designed
//   to be moved between threads (though processing must happen on audio thread)
unsafe impl Send for ClapPluginWrapper {}

#[cfg(test)]
mod tests {
    #[test]
    fn test_deinterleave_interleave() {
        // Test the deinterleave/interleave logic independently
        let input = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // [L1, R1, L2, R2, L3, R3]
        let frame_count = input.len() / 2;

        // Deinterleave
        let mut deinterleaved = vec![0.0; input.len()];
        for i in 0..frame_count {
            deinterleaved[i] = input[i * 2];           // Left
            deinterleaved[frame_count + i] = input[i * 2 + 1]; // Right
        }
        assert_eq!(deinterleaved, vec![1.0, 3.0, 5.0, 2.0, 4.0, 6.0]);

        // Interleave
        let mut reinterleaved = vec![0.0; input.len()];
        for i in 0..frame_count {
            reinterleaved[i * 2] = deinterleaved[i];           // Left
            reinterleaved[i * 2 + 1] = deinterleaved[frame_count + i]; // Right
        }
        assert_eq!(reinterleaved, input.to_vec());
    }
}
