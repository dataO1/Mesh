//! Network management state, backend, and view for settings UI.
//!
//! Provides WiFi and LAN status display with connection management.
//! On Linux, uses `nmrs` (D-Bus bindings for NetworkManager) for all operations.
//! On other platforms, the network section is hidden gracefully.

use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Color, Element, Length};

use super::message::Message;

// ── State Types (unconditional — no platform gating) ──

/// WiFi connection status
#[derive(Debug, Clone)]
pub enum WifiStatus {
    Disconnected,
    Connecting { ssid: String },
    Connected { ssid: String, signal: u8 },
}

/// LAN (ethernet) connection status
#[derive(Debug, Clone)]
pub enum LanStatus {
    Disconnected,
    Connected { interface: String },
}

/// A discovered WiFi network from scan
#[derive(Debug, Clone)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u8,
    pub secured: bool,
    pub in_use: bool,
}

/// Network management state. Lives on SettingsState as `Option<NetworkState>`.
/// None when not on Linux or NetworkManager is not available.
#[derive(Debug, Clone)]
pub struct NetworkState {
    /// False when no WiFi adapter is present (section greyed out)
    pub has_wifi_adapter: bool,
    /// Current WiFi connection status
    pub wifi_status: WifiStatus,
    /// Current LAN connection status
    pub lan_status: LanStatus,
    /// Scanned WiFi networks (populated by Scan)
    pub networks: Vec<WifiNetwork>,
    /// MIDI-navigated highlight index in network list
    pub selected_network: Option<usize>,
    /// Scan in progress
    pub scanning: bool,
    /// Connection attempt in progress
    pub connecting: bool,
    /// Error message from last operation
    pub error_message: String,
}

impl NetworkState {
    pub fn new() -> Self {
        Self {
            has_wifi_adapter: false,
            wifi_status: WifiStatus::Disconnected,
            lan_status: LanStatus::Disconnected,
            networks: Vec::new(),
            selected_network: None,
            scanning: false,
            connecting: false,
            error_message: String::new(),
        }
    }
}

/// Messages for network management
#[derive(Debug, Clone)]
pub enum NetworkMessage {
    /// Trigger a WiFi scan
    Scan,
    /// Scan completed with results
    ScanComplete(Result<Vec<WifiNetwork>, String>),
    /// Check current network status (WiFi + LAN)
    CheckStatus,
    /// Status check completed: (wifi_status, lan_status, has_wifi_adapter)
    StatusComplete(Result<(WifiStatus, LanStatus, bool), String>),
    /// Select a network from the list (index into networks vec)
    SelectNetwork(usize),
    /// Connect to an open (unsecured) network
    ConnectOpen(String),
    /// Connect to a secured network with password
    ConnectSecured { ssid: String, password: String },
    /// Connection attempt completed
    ConnectComplete(Result<(), String>),
    /// Disconnect from current WiFi
    Disconnect,
    /// Disconnect completed
    DisconnectComplete(Result<(), String>),
    /// MIDI encoder scroll through network list
    ScrollNetworks(i32),
}

// ── Backend: Linux (nmrs via D-Bus) ──
//
// All functions are synchronous/blocking. Each spawns a dedicated thread with its
// own single-threaded tokio runtime to execute nmrs async calls. Two constraints
// force this design:
//   1. nmrs (zbus) futures are !Send — can't pass them to iced's Task::perform
//   2. iced's Task::perform runs on tokio's thread pool — calling block_on() there
//      panics with "Cannot start a runtime from within a runtime"
// The dedicated thread has no pre-existing runtime, so block_on() works safely.
// Thread spawn cost (~100μs) is negligible vs D-Bus I/O (~10-100ms).

#[cfg(target_os = "linux")]
pub mod backend {
    use super::*;

    /// Run an async closure on a dedicated thread with its own tokio runtime.
    /// The closure creates !Send futures (nmrs/zbus) that stay on the new thread.
    /// The result is sent back via sync_channel.
    fn run_on_thread<T, F, Fut>(label: &str, f: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T>,
    {
        let label_owned = label.to_string();
        log::debug!("[nmrs] run_on_thread: spawning thread for '{}'", label);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            log::debug!("[nmrs] thread '{}': building tokio runtime", label_owned);
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("[nmrs] thread '{}': failed to build runtime: {}", label_owned, e);
                    return;
                }
            };
            log::debug!("[nmrs] thread '{}': calling block_on", label_owned);
            let result = rt.block_on(f());
            log::debug!("[nmrs] thread '{}': block_on completed, sending result", label_owned);
            let _ = tx.send(result);
        });
        log::debug!("[nmrs] run_on_thread: waiting for '{}' result", label);
        let result = rx.recv().map_err(|e| format!("nmrs thread failed: {}", e));
        log::debug!("[nmrs] run_on_thread: '{}' result received (ok={})", label, result.is_ok());
        result
    }

    /// Check if NetworkManager is available by attempting a D-Bus connection.
    pub fn is_available() -> bool {
        run_on_thread("is_available", || async {
            let result = nmrs::NetworkManager::new().await;
            log::debug!("[nmrs] is_available: NetworkManager::new() = {:?}", result.is_ok());
            result.is_ok()
        }).unwrap_or(false)
    }

    /// Detect whether a WiFi adapter is present.
    pub fn detect_wifi_adapter() -> bool {
        run_on_thread("detect_wifi_adapter", || async {
            let nm = match nmrs::NetworkManager::new().await {
                Ok(nm) => nm,
                Err(e) => {
                    log::debug!("[nmrs] detect_wifi_adapter: NM connect failed: {}", e);
                    return false;
                }
            };
            let devices = match nm.list_devices().await {
                Ok(d) => {
                    log::debug!("[nmrs] detect_wifi_adapter: {} devices found", d.len());
                    for dev in &d {
                        log::debug!("[nmrs]   device: {} type={:?} state={:?}",
                            dev.interface, dev.device_type, dev.state);
                    }
                    d
                }
                Err(e) => {
                    log::debug!("[nmrs] detect_wifi_adapter: list_devices failed: {}", e);
                    return false;
                }
            };
            let has_wifi = devices.iter().any(|d| d.device_type == nmrs::DeviceType::Wifi);
            log::debug!("[nmrs] detect_wifi_adapter: has_wifi={}", has_wifi);
            has_wifi
        }).unwrap_or(false)
    }

    /// Get current WiFi status (connected SSID + signal, or disconnected).
    pub fn get_wifi_status() -> WifiStatus {
        run_on_thread("get_wifi_status", || async {
            let nm = match nmrs::NetworkManager::new().await {
                Ok(nm) => nm,
                Err(e) => {
                    log::debug!("[nmrs] get_wifi_status: NM connect failed: {}", e);
                    return WifiStatus::Disconnected;
                }
            };

            let ssid = match nm.current_ssid().await {
                Some(s) => {
                    log::debug!("[nmrs] get_wifi_status: current_ssid = {:?}", s);
                    s
                }
                None => {
                    log::debug!("[nmrs] get_wifi_status: no current SSID");
                    return WifiStatus::Disconnected;
                }
            };

            let signal = match nm.current_network().await {
                Ok(Some(net)) => {
                    log::debug!("[nmrs] get_wifi_status: current_network strength={:?}", net.strength);
                    net.strength.unwrap_or(0)
                }
                Ok(None) => {
                    log::debug!("[nmrs] get_wifi_status: current_network = None");
                    0
                }
                Err(e) => {
                    log::debug!("[nmrs] get_wifi_status: current_network error: {}", e);
                    0
                }
            };

            WifiStatus::Connected { ssid, signal }
        }).unwrap_or(WifiStatus::Disconnected)
    }

    /// Get current LAN (ethernet) status.
    pub fn get_lan_status() -> LanStatus {
        run_on_thread("get_lan_status", || async {
            let nm = match nmrs::NetworkManager::new().await {
                Ok(nm) => nm,
                Err(e) => {
                    log::debug!("[nmrs] get_lan_status: NM connect failed: {}", e);
                    return LanStatus::Disconnected;
                }
            };
            let devices = match nm.list_devices().await {
                Ok(d) => d,
                Err(e) => {
                    log::debug!("[nmrs] get_lan_status: list_devices failed: {}", e);
                    return LanStatus::Disconnected;
                }
            };

            for device in &devices {
                if device.device_type == nmrs::DeviceType::Ethernet
                    && device.state == nmrs::DeviceState::Activated
                {
                    log::debug!("[nmrs] get_lan_status: found active ethernet: {}", device.interface);
                    return LanStatus::Connected {
                        interface: device.interface.clone(),
                    };
                }
            }

            log::debug!("[nmrs] get_lan_status: no active ethernet");
            LanStatus::Disconnected
        }).unwrap_or(LanStatus::Disconnected)
    }

    /// Scan for available WiFi networks.
    pub fn scan_wifi() -> Result<Vec<WifiNetwork>, String> {
        run_on_thread("scan_wifi", || async {
            let nm = nmrs::NetworkManager::new()
                .await
                .map_err(|e| format!("NetworkManager connection failed: {}", e))?;

            // Trigger a fresh scan
            nm.scan_networks()
                .await
                .map_err(|e| format!("WiFi scan failed: {}", e))?;

            // List discovered networks
            let raw_networks = nm
                .list_networks()
                .await
                .map_err(|e| format!("Failed to list networks: {}", e))?;

            let mut networks = Vec::new();
            let mut seen_ssids = std::collections::HashSet::new();

            for net in raw_networks {
                if net.ssid.is_empty() || !seen_ssids.insert(net.ssid.clone()) {
                    continue;
                }
                networks.push(WifiNetwork {
                    ssid: net.ssid,
                    signal: net.strength.unwrap_or(0),
                    secured: net.secured,
                    in_use: false,
                });
            }

            networks.sort_by(|a, b| b.signal.cmp(&a.signal));

            // Mark the currently connected network
            if let Some(current_ssid) = nm.current_ssid().await {
                for net in &mut networks {
                    if net.ssid == current_ssid {
                        net.in_use = true;
                        break;
                    }
                }
            }

            Ok(networks)
        })?
    }

    /// Connect to a WiFi network (open or with password).
    pub fn connect_wifi(ssid: &str, password: Option<&str>) -> Result<(), String> {
        let ssid = ssid.to_string();
        let password = password.map(|p| p.to_string());
        run_on_thread("connect_wifi", move || async move {
            let nm = nmrs::NetworkManager::new()
                .await
                .map_err(|e| format!("NetworkManager connection failed: {}", e))?;

            let security = match password {
                Some(psk) => nmrs::WifiSecurity::WpaPsk { psk },
                None => nmrs::WifiSecurity::Open,
            };

            nm.connect(&ssid, security)
                .await
                .map_err(|e| format!("Connection failed: {}", e))
        })?
    }

    /// Disconnect from the current WiFi network.
    pub fn disconnect_wifi() -> Result<(), String> {
        run_on_thread("disconnect_wifi", || async {
            let nm = nmrs::NetworkManager::new()
                .await
                .map_err(|e| format!("NetworkManager connection failed: {}", e))?;

            nm.disconnect()
                .await
                .map_err(|e| format!("Disconnect failed: {}", e))
        })?
    }
}

// ── Backend: Non-Linux (stubs) ──

#[cfg(not(target_os = "linux"))]
pub mod backend {
    use super::*;

    pub fn is_available() -> bool {
        false
    }

    pub fn detect_wifi_adapter() -> bool {
        false
    }

    pub fn get_wifi_status() -> WifiStatus {
        WifiStatus::Disconnected
    }

    pub fn get_lan_status() -> LanStatus {
        LanStatus::Disconnected
    }

    pub fn scan_wifi() -> Result<Vec<WifiNetwork>, String> {
        Err("WiFi management not available on this platform".to_string())
    }

    pub fn connect_wifi(_ssid: &str, _password: Option<&str>) -> Result<(), String> {
        Err("WiFi management not available on this platform".to_string())
    }

    pub fn disconnect_wifi() -> Result<(), String> {
        Err("WiFi management not available on this platform".to_string())
    }
}

// ── View ──

/// Signal strength to bar characters
fn signal_bars(signal: u8) -> &'static str {
    match signal {
        0..=25 => "▂",
        26..=50 => "▂▅",
        51..=75 => "▂▅█",
        _ => "▂▅██",
    }
}

/// Render the network settings section
pub fn view_network_section(state: &NetworkState) -> Element<'_, Message> {
    let section_title = text("Network").size(18);

    let mut content_items: Vec<Element<'_, Message>> = vec![section_title.into()];

    // LAN status (always shown if connected)
    match &state.lan_status {
        LanStatus::Connected { interface } => {
            let lan_label = text(format!("LAN: Connected ({})", interface))
                .size(12)
                .color(Color::from_rgb(0.4, 0.8, 0.4));
            content_items.push(lan_label.into());
        }
        LanStatus::Disconnected => {
            // Only show "LAN: Not connected" if WiFi is also disconnected
            if matches!(state.wifi_status, WifiStatus::Disconnected) {
                let lan_label = text("LAN: Not connected")
                    .size(12)
                    .color(Color::from_rgb(0.5, 0.5, 0.5));
                content_items.push(lan_label.into());
            }
        }
    }

    // WiFi status line
    let wifi_status_elem: Element<'_, Message> = match &state.wifi_status {
        WifiStatus::Connected { ssid, signal } => {
            text(format!(
                "WiFi: Connected to {} ({})",
                ssid, signal_bars(*signal),
            ))
            .size(12)
            .color(Color::from_rgb(0.4, 0.8, 0.4))
            .into()
        }
        WifiStatus::Connecting { ssid } => {
            text(format!("WiFi: Connecting to {}...", ssid))
                .size(12)
                .color(Color::from_rgb(0.9, 0.7, 0.2))
                .into()
        }
        WifiStatus::Disconnected => {
            text("WiFi: Not connected")
                .size(12)
                .color(Color::from_rgb(0.5, 0.5, 0.5))
                .into()
        }
    };
    content_items.push(wifi_status_elem);

    // No WiFi adapter — greyed-out section
    if !state.has_wifi_adapter {
        let no_adapter = text("No WiFi adapter detected")
            .size(12)
            .color(Color::from_rgb(0.5, 0.5, 0.5));
        content_items.push(no_adapter.into());

        return container(column(content_items).spacing(8))
            .padding(15)
            .width(Length::Fill)
            .into();
    }

    // Action buttons row
    let scan_btn = if state.scanning {
        button(text("Scanning...").size(11)).style(button::secondary)
    } else {
        button(text("Scan").size(11))
            .on_press(Message::Network(NetworkMessage::Scan))
            .style(button::secondary)
    };

    let disconnect_btn_elem: Element<'_, Message> = if matches!(state.wifi_status, WifiStatus::Connected { .. }) {
        button(text("Disconnect").size(11))
            .on_press(Message::Network(NetworkMessage::Disconnect))
            .style(button::secondary)
            .into()
    } else {
        Space::new().into()
    };

    let actions = row![scan_btn, disconnect_btn_elem]
        .spacing(8)
        .align_y(Alignment::Center);
    content_items.push(actions.into());

    // Error message
    if !state.error_message.is_empty() {
        let err = text(&state.error_message)
            .size(11)
            .color(Color::from_rgb(1.0, 0.4, 0.4));
        content_items.push(err.into());
    }

    // Network list
    if state.networks.is_empty() && !state.scanning {
        let empty_label = text("No networks found — press Scan to search")
            .size(12)
            .color(Color::from_rgb(0.5, 0.5, 0.5));
        content_items.push(empty_label.into());
    } else {
        for (i, network) in state.networks.iter().enumerate() {
            let is_selected = state.selected_network == Some(i);
            let lock_icon = if network.secured { " 🔒" } else { "" };
            let in_use_marker = if network.in_use { " ●" } else { "" };
            let label = format!(
                "{}{} {} {}",
                network.ssid,
                in_use_marker,
                signal_bars(network.signal),
                lock_icon,
            );

            let bg = if is_selected {
                Color::from_rgba(0.3, 0.5, 1.0, 0.3)
            } else if network.in_use {
                Color::from_rgba(0.2, 0.6, 0.2, 0.15)
            } else {
                Color::TRANSPARENT
            };

            let network_row = button(text(label).size(11))
                .on_press(Message::Network(NetworkMessage::SelectNetwork(i)))
                .style(move |_theme, _status| button::Style {
                    background: Some(bg.into()),
                    text_color: Color::WHITE,
                    border: iced::Border {
                        radius: 3.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .width(Length::Fill)
                .padding([3, 8]);

            content_items.push(network_row.into());
        }
    }

    container(column(content_items).spacing(6))
        .padding(15)
        .width(Length::Fill)
        .into()
}
