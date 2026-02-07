//! Low-level CLAP plugin wrapper using clack-host
//!
//! This module provides the core plugin hosting functionality, wrapping
//! clack-host's API into a mesh-friendly interface.

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clack_host::prelude::*;
use clack_host::bundle::PluginBundle;
use clack_host::events::event_types::ParamValueEvent;
use clack_host::events::io::EventBuffer;
use clack_host::process::StartedPluginAudioProcessor;
use clack_host::utils::{ClapId, Cookie};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamInfoBuffer,
    ParamRescanFlags, PluginParams,
};
use clack_extensions::gui::{GuiSize, HostGui, HostGuiImpl, PluginGui, GuiApiType, GuiConfiguration, Window};
use clack_extensions::latency::{HostLatency, HostLatencyImpl, PluginLatency};
use crossbeam::channel::{self, Sender, Receiver};

use super::error::{ClapError, ClapResult};
use super::discovery::DiscoveredClapPlugin;

/// Sample rate used for CLAP plugins
pub const CLAP_SAMPLE_RATE: u32 = 48000;

/// Default buffer size for processing
pub const CLAP_BUFFER_SIZE: u32 = 256;

/// Maximum buffer size we'll allocate for
pub const CLAP_MAX_BUFFER_SIZE: u32 = 4096;

/// Capacity for the parameter change notification channel
const PARAM_CHANGE_CHANNEL_CAPACITY: usize = 64;

// ============================================================================
// Parameter Change Notifications
// ============================================================================

/// A parameter change detected from plugin output events
///
/// This is sent when the plugin's GUI (or automation) changes a parameter,
/// allowing the host to detect which parameter was modified for "learn" mode.
#[derive(Debug, Clone)]
pub struct ParamChangeEvent {
    /// The CLAP parameter ID that changed
    pub param_id: u32,
    /// The new value (in plugin's native range)
    pub value: f64,
}

/// Receiver for parameter change events from a plugin
pub type ParamChangeReceiver = Receiver<ParamChangeEvent>;

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
        builder.register::<HostParams>();
        builder.register::<HostGui>();
        builder.register::<HostLatency>();
    }
}

impl HostParamsImplShared for MeshClapHostShared {
    fn request_flush(&self) {
        // Plugin GUI thread is requesting a param flush
        // Set flag so wrapper can call flush() on next opportunity
        self.flush_requested.store(true, Ordering::Release);
        log::info!("[CLAP_LEARN] Plugin '{}' GUI called request_flush()", self.plugin_id);
    }
}

impl HostGuiImpl for MeshClapHostShared {
    fn resize_hints_changed(&self) {
        // We don't dynamically handle resize hints yet
        log::trace!("CLAP plugin '{}' resize hints changed", self.plugin_id);
    }

    fn request_resize(&self, new_size: GuiSize) -> Result<(), clack_host::host::HostError> {
        // For floating windows, we accept all resize requests
        log::debug!(
            "CLAP plugin '{}' requested resize to {}x{}",
            self.plugin_id,
            new_size.width,
            new_size.height
        );
        Ok(())
    }

    fn request_show(&self) -> Result<(), clack_host::host::HostError> {
        log::debug!("CLAP plugin '{}' requested show", self.plugin_id);
        Ok(())
    }

    fn request_hide(&self) -> Result<(), clack_host::host::HostError> {
        log::debug!("CLAP plugin '{}' requested hide", self.plugin_id);
        Ok(())
    }

    fn closed(&self, was_destroyed: bool) {
        log::debug!(
            "CLAP plugin '{}' GUI closed (destroyed: {})",
            self.plugin_id,
            was_destroyed
        );
    }
}

impl HostParamsImplMainThread for MeshClapHostMainThread<'_> {
    fn rescan(&mut self, _flags: ParamRescanFlags) {
        // We don't track param changes dynamically yet
    }

    fn clear(&mut self, _param_id: ClapId, _flags: ParamClearFlags) {
        // No-op
    }
}

/// Shared host data accessible from any thread
pub struct MeshClapHostShared {
    /// Plugin ID for logging
    plugin_id: String,
    /// Flag set when plugin requests a parameter flush (from GUI thread)
    flush_requested: Arc<AtomicBool>,
}

impl MeshClapHostShared {
    fn new(plugin_id: String, flush_requested: Arc<AtomicBool>) -> Self {
        Self { plugin_id, flush_requested }
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
    /// Plugin params extension (if supported)
    pub params_ext: Option<PluginParams>,
    /// Plugin latency extension (if supported)
    pub latency_ext: Option<PluginLatency>,
}

impl<'a> MeshClapHostMainThread<'a> {
    fn new(shared: &'a MeshClapHostShared) -> Self {
        Self {
            _shared: shared,
            plugin: None,
            params_ext: None,
            latency_ext: None,
        }
    }
}

impl<'a> MainThreadHandler<'a> for MeshClapHostMainThread<'a> {
    fn initialized(&mut self, instance: InitializedPluginHandle<'a>) {
        self.params_ext = instance.get_extension();
        self.latency_ext = instance.get_extension();
        self.plugin = Some(instance);
    }
}

impl HostLatencyImpl for MeshClapHostMainThread<'_> {
    fn changed(&mut self) {
        // Plugin notified us that its latency changed
        // We'll query it on the next process cycle
        log::debug!("CLAP plugin latency changed notification received");
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
    /// Output event buffer for capturing plugin parameter changes
    output_event_buffer: EventBuffer,
    /// Channel sender for parameter change notifications
    param_change_sender: Sender<ParamChangeEvent>,
    /// Flag set by host when plugin GUI requests a param flush
    flush_requested: Arc<AtomicBool>,
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
    /// Cached parameter values for change detection during learning mode
    /// Maps param_id -> (previous_value, has_been_sampled)
    cached_param_values: HashMap<u32, f64>,
    /// Whether we're actively polling for param changes (learning mode)
    learning_mode_active: bool,
}

impl ClapPluginWrapper {
    /// Create a new plugin wrapper from a discovered plugin
    ///
    /// Returns the wrapper and a receiver for parameter change notifications.
    /// The receiver can be used to detect when the plugin's GUI changes parameters.
    pub fn new(
        plugin_info: &DiscoveredClapPlugin,
        bundle: Arc<PluginBundle>,
    ) -> ClapResult<(Self, ParamChangeReceiver)> {
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

        // Create shared flush flag for host<->wrapper communication
        let flush_requested = Arc::new(AtomicBool::new(false));
        let flush_flag_for_host = Arc::clone(&flush_requested);

        let cloned_id = plugin_info.id.clone();
        let instance = PluginInstance::<MeshClapHost>::new(
            |_| MeshClapHostShared::new(cloned_id.clone(), flush_flag_for_host),
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

        // Create channel for parameter change notifications
        let (sender, receiver) = channel::bounded(PARAM_CHANGE_CHANNEL_CAPACITY);

        Ok((
            Self {
                instance: Some(instance),
                processor: None,
                info: plugin_info.clone(),
                input_ports: AudioPorts::with_capacity(2, 1), // 2 channels, 1 port
                output_ports: AudioPorts::with_capacity(2, 1),
                input_buffer: vec![0.0; stereo_buffer_size],
                output_buffer: vec![0.0; stereo_buffer_size],
                output_event_buffer: EventBuffer::with_capacity(32),
                param_change_sender: sender,
                flush_requested,
                buffer_size,
                sample_rate: CLAP_SAMPLE_RATE,
                activated: false,
                latency_samples: 0,
                _bundle: bundle,
                cached_param_values: HashMap::new(),
                learning_mode_active: false,
            },
            receiver,
        ))
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

        // Query latency using the latency extension
        self.latency_samples = instance
            .access_handler(|h| h.latency_ext)
            .map(|ext| {
                let mut handle = instance.plugin_handle();
                ext.get(&mut handle)
            })
            .unwrap_or(0);

        self.instance = Some(instance);
        self.processor = Some(processor);
        self.activated = true;

        log::info!(
            "CLAP plugin '{}' activated at {}Hz, buffer size {}, latency {} samples",
            self.info.id,
            sample_rate,
            buffer_size,
            self.latency_samples
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

    /// Query all available parameters from the plugin
    ///
    /// Uses the CLAP params extension to enumerate all plugin parameters.
    /// Returns empty Vec if the plugin doesn't support the params extension.
    pub fn query_params(&mut self) -> Vec<ClapParamInfo> {
        let instance = match self.instance.as_mut() {
            Some(i) => i,
            None => return Vec::new(),
        };

        // Get params extension from main thread handler
        let params_ext = match instance.access_handler(|h| h.params_ext) {
            Some(ext) => ext,
            None => {
                log::debug!("Plugin '{}' doesn't support params extension", self.info.id);
                return Vec::new();
            }
        };

        let mut plugin_handle = instance.plugin_handle();
        let count = params_ext.count(&mut plugin_handle);
        log::debug!("Plugin '{}' has {} parameters", self.info.id, count);

        let mut params = Vec::with_capacity(count as usize);
        let mut info_buffer = ParamInfoBuffer::new();

        for i in 0..count {
            if let Some(info) = params_ext.get_info(&mut plugin_handle, i, &mut info_buffer) {
                // Convert name bytes to string (trimming null bytes)
                let name = String::from_utf8_lossy(info.name)
                    .trim_end_matches('\0')
                    .to_string();

                params.push(ClapParamInfo {
                    id: info.id.get(),
                    name,
                    min: info.min_value,
                    max: info.max_value,
                    default: info.default_value,
                });
            }
        }

        params
    }

    /// Get the current value of a parameter by its CLAP param ID
    ///
    /// Returns the value in the plugin's native range (not normalized).
    pub fn get_param_value(&mut self, param_id: u32) -> Option<f64> {
        let instance = match self.instance.as_mut() {
            Some(i) => i,
            None => return None,
        };

        let params_ext = match instance.access_handler(|h| h.params_ext) {
            Some(ext) => ext,
            None => return None,
        };

        let mut plugin_handle = instance.plugin_handle();
        let clap_id = ClapId::new(param_id);
        params_ext.get_value(&mut plugin_handle, clap_id)
    }

    /// Poll for parameter changes from the plugin GUI
    ///
    /// This method supports two mechanisms for detecting GUI parameter changes:
    ///
    /// 1. **request_flush() based**: Some plugins call host->params->request_flush() when
    ///    their GUI modifies a parameter. This sets the `flush_requested` flag.
    ///
    /// 2. **Unconditional flush**: Many plugins (like LSP) don't call request_flush().
    ///    They send parameter changes through the audio thread's output events instead.
    ///    When audio is not playing, we must call flush_active() unconditionally to poll
    ///    for any pending GUI changes.
    ///
    /// Call this periodically from the UI thread during learning mode to detect changes.
    pub fn poll_gui_param_changes(&mut self) {
        // Check if flush was explicitly requested (some plugins do this)
        let was_requested = self.flush_requested.swap(false, Ordering::AcqRel);
        if was_requested {
            log::info!("[CLAP_LEARN] Processing flush request for plugin '{}'", self.info.id);
        }

        // Need the processor (plugin must be activated and started)
        let processor = match self.processor.as_mut() {
            Some(p) => p,
            None => {
                log::warn!("[CLAP_LEARN] No audio processor for '{}', cannot flush params", self.info.id);
                return;
            }
        };

        let instance = match self.instance.as_ref() {
            Some(i) => i,
            None => {
                log::warn!("[CLAP_LEARN] No instance for '{}'", self.info.id);
                return;
            }
        };

        // Get params extension
        let params_ext = match instance.access_handler(|h| h.params_ext) {
            Some(ext) => ext,
            None => {
                log::warn!("[CLAP_LEARN] Plugin '{}' has no params extension", self.info.id);
                return;
            }
        };

        // Clear output event buffer before processing
        self.output_event_buffer.clear();

        // When learning mode is active, call process() with a silent buffer.
        // Many plugins (like LSP) only output ParamValueEvent during process(),
        // not during flush_active(). This ensures we capture GUI parameter changes
        // even when audio is not actively playing.
        if self.learning_mode_active {
            log::trace!("[CLAP_LEARN] Processing silent buffer to capture GUI param changes for '{}'", self.info.id);

            // Use a small buffer size for minimal overhead
            const SILENT_BUFFER_SIZE: usize = 64;

            // Ensure our buffers are large enough for silent processing
            let stereo_buffer_size = SILENT_BUFFER_SIZE * 2;
            if self.input_buffer.len() < stereo_buffer_size {
                self.input_buffer.resize(stereo_buffer_size, 0.0);
            }
            if self.output_buffer.len() < stereo_buffer_size {
                self.output_buffer.resize(stereo_buffer_size, 0.0);
            }

            // Fill input with silence
            self.input_buffer[..stereo_buffer_size].fill(0.0);
            self.output_buffer[..stereo_buffer_size].fill(0.0);

            // Split buffers to get non-overlapping mutable references for L/R channels
            let (input_left, input_right) = self.input_buffer[..stereo_buffer_size].split_at_mut(SILENT_BUFFER_SIZE);
            let (output_left, output_right) = self.output_buffer[..stereo_buffer_size].split_at_mut(SILENT_BUFFER_SIZE);

            // Prepare input buffers using the same pattern as process_with_params
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

            // Empty input events
            let input_events = EventBuffer::new();
            let input_events_ref = input_events.as_input();
            let mut output_events = self.output_event_buffer.as_output();

            // Process the silent buffer - this triggers the plugin to output any pending param changes
            let result = processor.process(
                &input_buffers,
                &mut output_buffers,
                &input_events_ref,
                &mut output_events,
                None, // steady time
                None, // transport
            );

            drop(output_events);

            if let Err(e) = result {
                log::warn!("[CLAP_LEARN] Silent process failed for '{}': {:?}", self.info.id, e);
            }
        } else {
            // When not in learning mode, just call flush_active
            let mut plugin_handle = processor.plugin_handle();
            let input_events = EventBuffer::new();

            log::trace!("[CLAP_LEARN] Calling flush_active on plugin '{}'", self.info.id);

            params_ext.flush_active(
                &mut plugin_handle,
                &input_events.as_input(),
                &mut self.output_event_buffer.as_output(),
            );
        }

        // Check how many output events we got
        let event_count = self.output_event_buffer.as_input().iter().count();
        log::trace!("[CLAP_LEARN] Got {} output events from plugin", event_count);

        // Process any output events (sends to param_change_sender channel)
        self.process_output_events();

        // If process/flush didn't produce any events, try direct value polling as fallback.
        // Some plugins might still not output events but do update their internal values.
        log::trace!(
            "[CLAP_LEARN] poll_gui_param_changes: event_count={}, learning_mode_active={}",
            event_count, self.learning_mode_active
        );
        if event_count == 0 && self.learning_mode_active {
            log::trace!("[CLAP_LEARN] Calling poll_param_value_changes() for '{}'", self.info.id);
            self.poll_param_value_changes();
        }
    }

    /// Start learning mode - snapshot all current parameter values
    ///
    /// Call this when entering learning mode. All parameter values are cached
    /// so that subsequent calls to `poll_gui_param_changes()` can detect changes
    /// by comparing current values to the snapshot.
    pub fn start_learning_mode(&mut self) {
        log::info!("[CLAP_LEARN] Starting learning mode for '{}'", self.info.id);

        let instance = match self.instance.as_mut() {
            Some(i) => i,
            None => {
                log::warn!("[CLAP_LEARN] No instance for '{}', cannot start learning", self.info.id);
                return;
            }
        };

        // Get params extension
        let params_ext = match instance.access_handler(|h| h.params_ext) {
            Some(ext) => ext,
            None => {
                log::warn!("[CLAP_LEARN] Plugin '{}' has no params extension", self.info.id);
                return;
            }
        };

        // Clear old cache
        self.cached_param_values.clear();

        // Get the plugin handle for main thread operations
        let mut plugin_handle = instance.plugin_handle();
        let param_count = params_ext.count(&mut plugin_handle);

        // Cache current values of all parameters
        let mut info_buffer = ParamInfoBuffer::new();
        for i in 0..param_count {
            if let Some(info) = params_ext.get_info(&mut plugin_handle, i, &mut info_buffer) {
                let param_id = ClapId::new(info.id.get());
                if let Some(value) = params_ext.get_value(&mut plugin_handle, param_id) {
                    self.cached_param_values.insert(info.id.get(), value);
                }
            }
        }

        self.learning_mode_active = true;
        log::info!(
            "[CLAP_LEARN] Cached {} parameter values for '{}'",
            self.cached_param_values.len(),
            self.info.id
        );
    }

    /// Stop learning mode and clear the parameter cache
    pub fn stop_learning_mode(&mut self) {
        log::info!("[CLAP_LEARN] Stopping learning mode for '{}'", self.info.id);
        self.learning_mode_active = false;
        self.cached_param_values.clear();
    }

    /// Poll for parameter value changes by comparing current values to cached values
    ///
    /// This is a fallback for plugins that don't properly implement `flush_active()`.
    /// It directly queries parameter values and compares them to the cached snapshot.
    fn poll_param_value_changes(&mut self) {
        if self.cached_param_values.is_empty() {
            log::warn!("[CLAP_LEARN] No cached param values - start_learning_mode() may not have run");
            return;
        }

        let instance = match self.instance.as_mut() {
            Some(i) => i,
            None => return,
        };

        let params_ext = match instance.access_handler(|h| h.params_ext) {
            Some(ext) => ext,
            None => return,
        };

        let mut plugin_handle = instance.plugin_handle();

        // Collect changes first (can't mutate cache while iterating)
        let mut changed: Option<(u32, f64)> = None;

        // Sample first 3 parameters to see if values are changing at all
        let mut sample_count = 0;
        for (&param_id, &cached_value) in self.cached_param_values.iter() {
            let clap_id = ClapId::new(param_id);
            if let Some(current_value) = params_ext.get_value(&mut plugin_handle, clap_id) {
                // Log first 3 params to see what values we're getting
                if sample_count < 3 {
                    log::debug!(
                        "[CLAP_LEARN] Sample param {}: id={}, cached={:.6}, current={:.6}, diff={:.6}",
                        sample_count,
                        param_id,
                        cached_value,
                        current_value,
                        (current_value - cached_value).abs()
                    );
                    sample_count += 1;
                }

                // Check if value changed (with small epsilon for floating point comparison)
                const EPSILON: f64 = 0.0001;
                if (current_value - cached_value).abs() > EPSILON {
                    log::info!(
                        "[CLAP_LEARN] Value change detected: plugin='{}', param_id={}, old={:.4}, new={:.4}",
                        self.info.id,
                        param_id,
                        cached_value,
                        current_value
                    );
                    changed = Some((param_id, current_value));
                    // Only report the first changed parameter (for learning mode)
                    break;
                }
            }
        }

        // Process the change after iteration is complete
        if let Some((param_id, new_value)) = changed {
            // Update cache with new value (so we don't report it again)
            self.cached_param_values.insert(param_id, new_value);

            // Send the change notification
            match self.param_change_sender.try_send(ParamChangeEvent {
                param_id,
                value: new_value,
            }) {
                Ok(_) => log::info!("[CLAP_LEARN] Sent value-based param change to channel"),
                Err(e) => log::warn!("[CLAP_LEARN] Failed to send param change: {:?}", e),
            }
        }
    }

    // ========================================================================
    // GUI Methods
    // ========================================================================

    /// Check if the plugin supports GUI
    pub fn supports_gui(&mut self) -> bool {
        let instance = match self.instance.as_mut() {
            Some(i) => i,
            None => return false,
        };

        instance
            .plugin_handle()
            .get_extension::<PluginGui>()
            .is_some()
    }

    /// Get the GUI extension from the plugin
    fn get_gui_extension(&mut self) -> ClapResult<PluginGui> {
        let instance = self.instance.as_mut().ok_or_else(|| ClapError::NotActivated {
            plugin_id: self.info.id.clone(),
        })?;

        instance
            .plugin_handle()
            .get_extension::<PluginGui>()
            .ok_or_else(|| ClapError::GuiNotSupported {
                plugin_id: self.info.id.clone(),
            })
    }

    /// Check if a specific GUI API is supported
    pub fn is_gui_api_supported(&mut self, is_floating: bool) -> bool {
        let instance = match self.instance.as_mut() {
            Some(i) => i,
            None => return false,
        };

        let gui_ext = match instance.plugin_handle().get_extension::<PluginGui>() {
            Some(ext) => ext,
            None => return false,
        };

        let api_type = Self::current_platform_api();
        let config = GuiConfiguration {
            api_type,
            is_floating,
        };

        gui_ext.is_api_supported(&mut instance.plugin_handle(), config)
    }

    /// Get the current platform's GUI API type
    fn current_platform_api() -> GuiApiType<'static> {
        #[cfg(target_os = "windows")]
        {
            GuiApiType::WIN32
        }
        #[cfg(target_os = "macos")]
        {
            GuiApiType::COCOA
        }
        #[cfg(target_os = "linux")]
        {
            GuiApiType::X11
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            GuiApiType::X11 // Fallback
        }
    }

    /// Create the plugin's GUI
    ///
    /// Must be called before show_gui().
    pub fn create_gui(&mut self, is_floating: bool) -> ClapResult<()> {
        let instance = self.instance.as_mut().ok_or_else(|| ClapError::NotActivated {
            plugin_id: self.info.id.clone(),
        })?;

        let gui_ext = instance
            .plugin_handle()
            .get_extension::<PluginGui>()
            .ok_or_else(|| ClapError::GuiNotSupported {
                plugin_id: self.info.id.clone(),
            })?;

        let api_type = Self::current_platform_api();
        let config = GuiConfiguration {
            api_type,
            is_floating,
        };

        gui_ext
            .create(&mut instance.plugin_handle(), config)
            .map_err(|e| ClapError::GuiCreationFailed {
                plugin_id: self.info.id.clone(),
                reason: format!("{:?}", e),
            })?;

        log::info!(
            "Created GUI for plugin '{}' (floating: {})",
            self.info.id,
            is_floating
        );
        Ok(())
    }

    /// Get the preferred GUI size
    pub fn get_gui_size(&mut self) -> ClapResult<(u32, u32)> {
        let instance = self.instance.as_mut().ok_or_else(|| ClapError::NotActivated {
            plugin_id: self.info.id.clone(),
        })?;

        let gui_ext = instance
            .plugin_handle()
            .get_extension::<PluginGui>()
            .ok_or_else(|| ClapError::GuiNotSupported {
                plugin_id: self.info.id.clone(),
            })?;

        gui_ext
            .get_size(&mut instance.plugin_handle())
            .map(|size| (size.width, size.height))
            .ok_or_else(|| ClapError::GuiCreationFailed {
                plugin_id: self.info.id.clone(),
                reason: "Failed to get GUI size".to_string(),
            })
    }

    /// Set the parent window for the GUI (for embedded mode)
    ///
    /// # Safety
    /// The caller must ensure the window handle remains valid until destroy_gui is called.
    pub unsafe fn set_gui_parent(&mut self, window: Window) -> ClapResult<()> {
        let instance = self.instance.as_mut().ok_or_else(|| ClapError::NotActivated {
            plugin_id: self.info.id.clone(),
        })?;

        let gui_ext = instance
            .plugin_handle()
            .get_extension::<PluginGui>()
            .ok_or_else(|| ClapError::GuiNotSupported {
                plugin_id: self.info.id.clone(),
            })?;

        gui_ext
            .set_parent(&mut instance.plugin_handle(), window)
            .map_err(|_| ClapError::GuiParentFailed {
                plugin_id: self.info.id.clone(),
            })?;

        log::debug!("Set GUI parent for plugin '{}'", self.info.id);
        Ok(())
    }

    /// Show the plugin's GUI
    pub fn show_gui(&mut self) -> ClapResult<()> {
        let instance = self.instance.as_mut().ok_or_else(|| ClapError::NotActivated {
            plugin_id: self.info.id.clone(),
        })?;

        let gui_ext = instance
            .plugin_handle()
            .get_extension::<PluginGui>()
            .ok_or_else(|| ClapError::GuiNotSupported {
                plugin_id: self.info.id.clone(),
            })?;

        gui_ext
            .show(&mut instance.plugin_handle())
            .map_err(|_| ClapError::GuiShowFailed {
                plugin_id: self.info.id.clone(),
            })?;

        log::info!("Showed GUI for plugin '{}'", self.info.id);
        Ok(())
    }

    /// Hide the plugin's GUI
    pub fn hide_gui(&mut self) -> ClapResult<()> {
        let instance = self.instance.as_mut().ok_or_else(|| ClapError::NotActivated {
            plugin_id: self.info.id.clone(),
        })?;

        let gui_ext = instance
            .plugin_handle()
            .get_extension::<PluginGui>()
            .ok_or_else(|| ClapError::GuiNotSupported {
                plugin_id: self.info.id.clone(),
            })?;

        gui_ext
            .hide(&mut instance.plugin_handle())
            .map_err(|_| ClapError::GuiHideFailed {
                plugin_id: self.info.id.clone(),
            })?;

        log::debug!("Hid GUI for plugin '{}'", self.info.id);
        Ok(())
    }

    /// Destroy the plugin's GUI and free resources
    pub fn destroy_gui(&mut self) {
        let instance = match self.instance.as_mut() {
            Some(i) => i,
            None => return,
        };

        let gui_ext = match instance.plugin_handle().get_extension::<PluginGui>() {
            Some(ext) => ext,
            None => return,
        };

        gui_ext.destroy(&mut instance.plugin_handle());
        log::debug!("Destroyed GUI for plugin '{}'", self.info.id);
    }

    /// Process audio through the plugin with parameter changes
    ///
    /// Takes interleaved stereo input, applies parameter changes, and produces
    /// interleaved stereo output.
    ///
    /// Parameter changes are provided as (param_id, value) pairs where:
    /// - param_id is the CLAP parameter ID
    /// - value is in the plugin's native range (NOT normalized 0-1)
    pub fn process_with_params(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        param_changes: &[(u32, f64)],
    ) -> ClapResult<()> {
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

        // Build input events from parameter changes
        let mut event_buffer = EventBuffer::with_capacity(param_changes.len());
        for &(param_id, value) in param_changes {
            // Skip u32::MAX which is invalid in CLAP
            if param_id == u32::MAX {
                continue;
            }
            // Create param value event at time 0 (apply immediately)
            let event = ParamValueEvent::new(
                0,                      // time
                ClapId::new(param_id), // param_id (panics if u32::MAX, but we checked above)
                Pckn::match_all(),     // pckn - match all notes
                value,                 // value in plugin's native range
                Cookie::empty(),       // no cookie
            );
            event_buffer.push(&event);
        }
        let input_events = event_buffer.as_input();

        // Clear output event buffer and prepare for capturing plugin output
        self.output_event_buffer.clear();
        let mut output_events = self.output_event_buffer.as_output();

        // Process
        processor
            .process(
                &input_buffers,
                &mut output_buffers,
                &input_events,
                &mut output_events,
                None, // steady time
                None, // transport
            )
            .map_err(|e| ClapError::ProcessingError {
                plugin_id: self.info.id.clone(),
                reason: format!("{:?}", e),
            })?;

        // Drop output_events to release borrow on output_event_buffer
        drop(output_events);

        // Process output events to detect parameter changes from plugin GUI
        self.process_output_events();

        // Interleave output: [L, L, L, ..., R, R, R, ...] -> [L, R, L, R, ...]
        for i in 0..frame_count {
            output[i * 2] = self.output_buffer[i];           // Left channel
            output[i * 2 + 1] = self.output_buffer[frame_count + i]; // Right channel
        }

        Ok(())
    }

    /// Process output events from the plugin to detect parameter changes
    ///
    /// This is called after each process() call to check if the plugin's GUI
    /// or internal automation changed any parameters.
    fn process_output_events(&self) {
        let mut event_idx = 0;
        for event in self.output_event_buffer.as_input().iter() {
            log::debug!("[CLAP_LEARN] Processing output event {}", event_idx);
            event_idx += 1;

            // Check if this is a parameter value event
            if let Some(param_event) = event.as_event::<ParamValueEvent>() {
                // param_id() returns Option<ClapId> - None means "all parameters"
                let param_id = match param_event.param_id() {
                    Some(id) => id.get(),
                    None => {
                        log::debug!("[CLAP_LEARN] Skipping 'all parameters' event");
                        continue;
                    }
                };
                let value = param_event.value();

                log::info!(
                    "[CLAP_LEARN] Got param change event: plugin='{}', param_id={}, value={}",
                    self.info.id,
                    param_id,
                    value
                );

                // Try to send the change notification (non-blocking)
                // If the channel is full, we just drop the event (UI can poll less frequently)
                match self.param_change_sender.try_send(ParamChangeEvent {
                    param_id,
                    value,
                }) {
                    Ok(_) => log::info!("[CLAP_LEARN] Sent param change to channel"),
                    Err(e) => log::warn!("[CLAP_LEARN] Failed to send param change: {:?}", e),
                }
            }
        }
    }

    /// Process audio through the plugin (without parameter changes)
    ///
    /// Takes interleaved stereo input and produces interleaved stereo output.
    /// The input buffer is copied, processed, and the result is written to output.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> ClapResult<()> {
        self.process_with_params(input, output, &[])
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
