//! Audio device enumeration and management
//!
//! Provides functionality to list available audio devices and their capabilities.
//!
//! This module enumerates devices from ALL available audio hosts (JACK, ALSA,
//! PulseAudio, etc.) to give users full control over device selection.
//!
//! On Linux with JACK running, JACK typically shows only one "device" (the JACK
//! server itself) while ALSA shows individual hardware devices. For DJ applications
//! requiring dual outputs (master + headphones), ALSA device selection is often needed.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Host, HostId};

use super::config::DeviceId;
use super::error::{AudioError, AudioResult};

/// Get a human-readable name for a host ID
fn host_name(host_id: HostId) -> String {
    // Use the debug representation which gives us the variant name
    let name = format!("{:?}", host_id);
    // Capitalize common names for better display
    match name.as_str() {
        "Alsa" => "ALSA".to_string(),
        "Jack" => "JACK".to_string(),
        "Wasapi" => "WASAPI".to_string(),
        _ => name,
    }
}

/// Get a host by its name string
fn get_host_by_name(name: &str) -> Option<Host> {
    for host_id in cpal::available_hosts() {
        if host_name(host_id) == name {
            return cpal::host_from_id(host_id).ok();
        }
    }
    None
}

/// Get the default/fallback audio host for the current platform.
fn get_default_host() -> Host {
    cpal::default_host()
}

/// Information about an audio output device
#[derive(Debug, Clone)]
pub struct AudioDevice {
    /// Device identifier for configuration (includes host info)
    pub id: DeviceId,
    /// Human-readable device name
    pub name: String,
    /// Host backend name (e.g., "ALSA", "JACK")
    pub host: String,
    /// Whether this is the system default device for its host
    pub is_default: bool,
    /// Supported sample rates (common ones)
    pub sample_rates: Vec<u32>,
    /// Maximum output channels
    pub max_channels: u16,
}

/// Get all available audio output devices from ALL hosts
///
/// This enumerates devices from every available audio host (JACK, ALSA, etc.)
/// to give users full control over device selection. On Linux, this typically
/// means you'll see both JACK's single device and ALSA's hardware devices.
pub fn get_output_devices() -> AudioResult<Vec<AudioDevice>> {
    let mut all_devices: Vec<AudioDevice> = Vec::new();

    // Enumerate devices from all available hosts
    for host_id in cpal::available_hosts() {
        let host = match cpal::host_from_id(host_id) {
            Ok(h) => h,
            Err(e) => {
                log::debug!("Could not initialize host {:?}: {}", host_id, e);
                continue;
            }
        };

        let host_name_str = host_name(host_id);

        let default_device_name = host
            .default_output_device()
            .and_then(|d: cpal::Device| d.name().ok());

        let devices_iter = match host.output_devices() {
            Ok(d) => d,
            Err(e) => {
                log::debug!("Could not enumerate devices for {:?}: {}", host_id, e);
                continue;
            }
        };

        for device in devices_iter {
            let name = match device.name() {
                Ok(n) => n,
                Err(_) => continue,
            };

            let is_default = default_device_name.as_ref() == Some(&name);

            // Get supported configurations
            let configs: Vec<_> = match device.supported_output_configs() {
                Ok(c) => c.collect(),
                Err(_) => continue,
            };

            if configs.is_empty() {
                continue;
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

            all_devices.push(AudioDevice {
                id: DeviceId::with_host(&name, &host_name_str),
                name: name.clone(),
                host: host_name_str.clone(),
                is_default,
                sample_rates,
                max_channels,
            });
        }
    }

    if all_devices.is_empty() {
        return Err(AudioError::NoDevices);
    }

    // Sort: default devices first, then by host, then by name
    all_devices.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.host.cmp(&b.host))
            .then_with(|| a.name.cmp(&b.name))
    });

    log::info!(
        "Enumerated {} audio devices from {} hosts",
        all_devices.len(),
        cpal::available_hosts().len()
    );

    Ok(all_devices)
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
///
/// Uses the host specified in the DeviceId if available, otherwise
/// searches all available hosts.
pub fn find_device_by_id(id: &DeviceId) -> AudioResult<cpal::Device> {
    // If a host is specified, use that specific host
    if let Some(ref host_name) = id.host {
        if let Some(host) = get_host_by_name(host_name) {
            return host
                .output_devices()
                .map_err(|e| AudioError::ConfigError(e.to_string()))?
                .find(|d: &cpal::Device| d.name().ok().as_ref() == Some(&id.name))
                .ok_or_else(|| AudioError::DeviceNotFound(id.name.clone()));
        }
    }

    // Otherwise, search all hosts for the device by name
    for host_id in cpal::available_hosts() {
        if let Ok(host) = cpal::host_from_id(host_id) {
            if let Ok(devices) = host.output_devices() {
                if let Some(device) = devices
                    .filter(|d: &cpal::Device| d.name().ok().as_ref() == Some(&id.name))
                    .next()
                {
                    return Ok(device);
                }
            }
        }
    }

    Err(AudioError::DeviceNotFound(id.name.clone()))
}

/// Get the CPAL default output device from the default host
pub fn get_cpal_default_device() -> AudioResult<cpal::Device> {
    let host = get_default_host();
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
    /// Device identifier (includes host info)
    pub id: DeviceId,
    /// Human-readable device name
    pub name: String,
    /// Host backend name (e.g., "ALSA", "JACK")
    pub host: String,
    /// Whether this is the system default
    pub is_default: bool,
}

impl From<AudioDevice> for OutputDevice {
    fn from(device: AudioDevice) -> Self {
        Self {
            id: device.id,
            name: device.name,
            host: device.host,
            is_default: device.is_default,
        }
    }
}

impl std::fmt::Display for OutputDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Show host prefix for clarity, e.g., "[ALSA] hw:0,0"
        write!(f, "[{}] {}", self.host, self.name)
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
                        "  - [{}] {} (default: {}, channels: {}, rates: {:?})",
                        device.host,
                        device.name,
                        device.is_default,
                        device.max_channels,
                        device.sample_rates
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
