//! Network management state, nmcli commands, and view for settings UI.
//!
//! Provides WiFi and LAN status display with connection management.
//! All network operations use `nmcli` via `std::process::Command`.
//! On systems without `nmcli` (desktop dev), the network section is hidden.

use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Color, Element, Length};

use super::message::Message;

// ── State Types ──

/// WiFi connection status
#[derive(Debug, Clone)]
pub enum WifiStatus {
    Disconnected,
    Connecting { ssid: String },
    Connected { ssid: String, signal: u8, ip: String },
}

/// LAN (ethernet) connection status
#[derive(Debug, Clone)]
pub enum LanStatus {
    Disconnected,
    Connected { interface: String, ip: String },
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
/// None when nmcli is not available (desktop dev environments).
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

// ── nmcli Command Wrappers ──
// These are pure functions that run synchronous commands.
// They are called from Task::perform async blocks.

/// Check if nmcli is available on PATH
pub fn is_nmcli_available() -> bool {
    std::process::Command::new("which")
        .arg("nmcli")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detect if a WiFi adapter is present via nmcli
pub fn detect_wifi_adapter() -> bool {
    let output = std::process::Command::new("nmcli")
        .args(["-t", "-f", "TYPE,STATE", "device"])
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.lines().any(|line| line.starts_with("wifi:"))
        }
        Err(_) => false,
    }
}

/// Get current WiFi status (SSID, signal, IP)
pub fn get_wifi_status() -> WifiStatus {
    // Find active wifi connection
    let output = std::process::Command::new("nmcli")
        .args(["-t", "-f", "TYPE,NAME,DEVICE", "connection", "show", "--active"])
        .output();

    let (ssid, device) = match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let mut found = (String::new(), String::new());
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split(':').collect();
                // format: TYPE:NAME:DEVICE (802-11-wireless:MyNetwork:wlan0)
                if parts.len() >= 3 && parts[0].contains("wireless") {
                    found = (parts[1].to_string(), parts[2].to_string());
                    break;
                }
            }
            found
        }
        Err(_) => return WifiStatus::Disconnected,
    };

    if ssid.is_empty() {
        return WifiStatus::Disconnected;
    }

    // Get IP and signal from device
    let ip = get_device_ip(&device);
    let signal = get_wifi_signal(&device);

    WifiStatus::Connected { ssid, signal, ip }
}

/// Get current LAN status (interface, IP)
pub fn get_lan_status() -> LanStatus {
    let output = std::process::Command::new("nmcli")
        .args(["-t", "-f", "TYPE,DEVICE,STATE", "device"])
        .output();

    let device = match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let mut found = String::new();
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 3 && parts[0] == "ethernet" && parts[2] == "connected" {
                    found = parts[1].to_string();
                    break;
                }
            }
            found
        }
        Err(_) => return LanStatus::Disconnected,
    };

    if device.is_empty() {
        return LanStatus::Disconnected;
    }

    let ip = get_device_ip(&device);
    LanStatus::Connected { interface: device, ip }
}

/// Get IP address for a network device
fn get_device_ip(device: &str) -> String {
    let output = std::process::Command::new("nmcli")
        .args(["-t", "-f", "IP4.ADDRESS", "device", "show", device])
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            for line in stdout.lines() {
                // Format: IP4.ADDRESS[1]:192.168.1.100/24
                if let Some(addr) = line.strip_prefix("IP4.ADDRESS") {
                    if let Some(ip) = addr.split(':').nth(1) {
                        // Strip CIDR suffix
                        return ip.split('/').next().unwrap_or(ip).to_string();
                    }
                }
            }
            String::new()
        }
        Err(_) => String::new(),
    }
}

/// Get WiFi signal strength for a device
fn get_wifi_signal(device: &str) -> u8 {
    let output = std::process::Command::new("nmcli")
        .args(["-t", "-f", "IN-USE,SIGNAL", "device", "wifi", "list", "ifname", device])
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 2 && parts[0] == "*" {
                    return parts[1].parse().unwrap_or(0);
                }
            }
            0
        }
        Err(_) => 0,
    }
}

/// Scan for available WiFi networks
pub fn scan_wifi() -> Result<Vec<WifiNetwork>, String> {
    let output = std::process::Command::new("nmcli")
        .args(["-t", "-f", "SSID,SIGNAL,SECURITY,IN-USE", "device", "wifi", "list", "--rescan", "yes"])
        .output()
        .map_err(|e| format!("Failed to run nmcli: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("nmcli scan failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut networks = Vec::new();
    let mut seen_ssids = std::collections::HashSet::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 4 {
            let ssid = parts[0].to_string();
            if ssid.is_empty() || !seen_ssids.insert(ssid.clone()) {
                continue; // Skip empty SSIDs and duplicates
            }
            let signal = parts[1].parse().unwrap_or(0);
            let secured = !parts[2].is_empty() && parts[2] != "--";
            let in_use = parts[3] == "*";
            networks.push(WifiNetwork { ssid, signal, secured, in_use });
        }
    }

    // Sort by signal strength descending
    networks.sort_by(|a, b| b.signal.cmp(&a.signal));
    Ok(networks)
}

/// Connect to a WiFi network (open or secured)
pub fn connect_wifi(ssid: &str, password: Option<&str>) -> Result<(), String> {
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["device", "wifi", "connect", ssid]);
    if let Some(pw) = password {
        cmd.args(["password", pw]);
    }

    let output = cmd.output().map_err(|e| format!("Failed to run nmcli: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("Connection failed: {}", stderr.trim()))
    }
}

/// Disconnect from WiFi
pub fn disconnect_wifi() -> Result<(), String> {
    // Find the wifi device name first
    let output = std::process::Command::new("nmcli")
        .args(["-t", "-f", "TYPE,DEVICE", "device"])
        .output()
        .map_err(|e| format!("Failed to run nmcli: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let wifi_device = stdout.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 2 && parts[0] == "wifi" {
                Some(parts[1].to_string())
            } else {
                None
            }
        })
        .next();

    match wifi_device {
        Some(device) => {
            let output = std::process::Command::new("nmcli")
                .args(["device", "disconnect", &device])
                .output()
                .map_err(|e| format!("Failed to run nmcli: {}", e))?;

            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(format!("Disconnect failed: {}", stderr.trim()))
            }
        }
        None => Err("No WiFi device found".to_string()),
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
        LanStatus::Connected { interface, ip } => {
            let lan_label = text(format!("LAN: Connected ({}) — {}", interface, ip))
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
        WifiStatus::Connected { ssid, signal, ip } => {
            text(format!(
                "WiFi: Connected to {} ({}) — {}",
                ssid, signal_bars(*signal), ip
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
