//! Audio device enumeration and management
//!
//! Provides functionality to list available audio devices and their capabilities.
//!
//! On Linux, this module prefers JACK over ALSA for better latency and routing.
//! JACK provides descriptive port names like "system:playback_1" whereas ALSA
//! uses hardware IDs like "hw:0,0".

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::Host;

use super::config::DeviceId;
use super::error::{AudioError, AudioResult};

/// Get the preferred audio host for the current platform.
///
/// On Linux, prefers JACK if available (for pro-audio routing and better names),
/// falls back to ALSA. On other platforms, uses the system default.
fn get_preferred_host() -> Host {
    #[cfg(target_os = "linux")]
    {
        // Try JACK first for better latency and descriptive device names
        if let Some(jack_host) = cpal::available_hosts()
            .into_iter()
            .find(|h| *h == cpal::HostId::Jack)
        {
            if let Ok(host) = cpal::host_from_id(jack_host) {
                log::info!("Using JACK audio host");
                return host;
            }
        }
        log::info!("JACK not available, using default host (ALSA)");
    }

    get_preferred_host()
}

/// Information about an audio output device
#[derive(Debug, Clone)]
pub struct AudioDevice {
    /// Device identifier for configuration
    pub id: DeviceId,
    /// Human-readable device name
    pub name: String,
    /// Whether this is the system default device
    pub is_default: bool,
    /// Supported sample rates (common ones)
    pub sample_rates: Vec<u32>,
    /// Maximum output channels
    pub max_channels: u16,
}

/// Get all available audio output devices
pub fn get_output_devices() -> AudioResult<Vec<AudioDevice>> {
    let host = get_preferred_host();

    let default_device_name = host
        .default_output_device()
        .and_then(|d| d.name().ok());

    let devices: Vec<AudioDevice> = host
        .output_devices()
        .map_err(|e| AudioError::ConfigError(e.to_string()))?
        .filter_map(|device| {
            let name = device.name().ok()?;
            let is_default = default_device_name.as_ref() == Some(&name);

            // Get supported configurations
            let configs: Vec<_> = device.supported_output_configs().ok()?.collect();
            if configs.is_empty() {
                return None;
            }

            // Extract sample rates and channels from supported configs
            let mut sample_rates: Vec<u32> = Vec::new();
            let mut max_channels: u16 = 0;

            for config in &configs {
                max_channels = max_channels.max(config.channels());

                // Add common sample rates that fall within the supported range
                for rate in [44100, 48000, 88200, 96000, 176400, 192000] {
                    if rate >= config.min_sample_rate().0
                        && rate <= config.max_sample_rate().0
                        && !sample_rates.contains(&rate)
                    {
                        sample_rates.push(rate);
                    }
                }
            }

            sample_rates.sort();

            Some(AudioDevice {
                id: DeviceId::new(&name),
                name,
                is_default,
                sample_rates,
                max_channels,
            })
        })
        .collect();

    if devices.is_empty() {
        return Err(AudioError::NoDevices);
    }

    Ok(devices)
}

/// Get the default audio output device
pub fn get_default_device() -> AudioResult<AudioDevice> {
    let devices = get_output_devices()?;
    devices
        .into_iter()
        .find(|d| d.is_default)
        .or_else(|| {
            // If no default found, try to get first device
            get_output_devices().ok().and_then(|d| d.into_iter().next())
        })
        .ok_or_else(|| AudioError::NoDefaultDevice("No output devices available".to_string()))
}

/// Find a device by its ID
pub fn find_device_by_id(id: &DeviceId) -> AudioResult<cpal::Device> {
    let host = get_preferred_host();

    host.output_devices()
        .map_err(|e| AudioError::ConfigError(e.to_string()))?
        .find(|d| d.name().ok().as_ref() == Some(&id.name))
        .ok_or_else(|| AudioError::DeviceNotFound(id.name.clone()))
}

/// Get the CPAL default output device
pub fn get_cpal_default_device() -> AudioResult<cpal::Device> {
    let host = get_preferred_host();
    host.default_output_device()
        .ok_or_else(|| AudioError::NoDefaultDevice("No default output device".to_string()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Simplified Device Types for UI (Settings Dropdowns)
// ═══════════════════════════════════════════════════════════════════════════

/// Simplified output device for UI display
///
/// This is a lightweight version of `AudioDevice` for use in settings UI.
#[derive(Debug, Clone)]
pub struct OutputDevice {
    /// Device identifier
    pub id: DeviceId,
    /// Human-readable name
    pub name: String,
    /// Whether this is the system default
    pub is_default: bool,
}

impl From<AudioDevice> for OutputDevice {
    fn from(device: AudioDevice) -> Self {
        Self {
            id: device.id,
            name: device.name,
            is_default: device.is_default,
        }
    }
}

impl std::fmt::Display for OutputDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Stereo output pair for settings UI compatibility
///
/// In CPAL mode, this represents a single audio device.
/// The "pair" terminology is kept for settings UI consistency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StereoPair {
    /// Human-readable label
    pub label: String,
    /// Device ID
    pub device_id: DeviceId,
}

impl std::fmt::Display for StereoPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label)
    }
}

/// Get available audio devices as simplified OutputDevice structs
pub fn get_available_output_devices() -> Vec<OutputDevice> {
    match get_output_devices() {
        Ok(devices) => devices.into_iter().map(OutputDevice::from).collect(),
        Err(e) => {
            log::warn!("Failed to enumerate audio devices: {}", e);
            Vec::new()
        }
    }
}

/// Get available devices as StereoPair (for settings UI dropdowns)
///
/// This provides a simple list of devices for UI pick_list widgets.
pub fn get_available_stereo_pairs() -> Vec<StereoPair> {
    get_available_output_devices()
        .into_iter()
        .map(|d| StereoPair {
            label: d.name.clone(),
            device_id: d.id,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_enumeration() {
        // This test may fail if no audio devices are available
        match get_output_devices() {
            Ok(devices) => {
                println!("Found {} audio devices:", devices.len());
                for device in &devices {
                    println!(
                        "  - {} (default: {}, channels: {}, rates: {:?})",
                        device.name, device.is_default, device.max_channels, device.sample_rates
                    );
                }
            }
            Err(AudioError::NoDevices) => {
                println!("No audio devices available (expected in CI)");
            }
            Err(e) => {
                println!("Error enumerating devices: {}", e);
            }
        }
    }
}
