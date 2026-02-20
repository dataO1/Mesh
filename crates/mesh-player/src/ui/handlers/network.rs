//! Network message handler
//!
//! Handles WiFi scanning, connection, and status checking via nmcli.
//! All blocking nmcli calls run inside Task::perform async blocks.

use iced::Task;

use crate::ui::app::MeshApp;
use crate::ui::message::Message;
use crate::ui::network::{
    self, NetworkMessage, NetworkState, WifiStatus,
};

/// Handle network messages
pub fn handle(app: &mut MeshApp, msg: NetworkMessage) -> Task<Message> {
    // Bail if no network state (nmcli not available)
    let state = match app.settings.network.as_mut() {
        Some(s) => s,
        None => return Task::none(),
    };

    match msg {
        NetworkMessage::Scan => {
            state.scanning = true;
            state.error_message.clear();
            Task::perform(
                async { network::scan_wifi() },
                |result| Message::Network(NetworkMessage::ScanComplete(result)),
            )
        }

        NetworkMessage::ScanComplete(result) => {
            let state = app.settings.network.as_mut().unwrap();
            state.scanning = false;
            match result {
                Ok(networks) => {
                    state.networks = networks;
                    state.selected_network = None;
                }
                Err(e) => {
                    state.error_message = e;
                }
            }
            Task::none()
        }

        NetworkMessage::CheckStatus => {
            Task::perform(
                async {
                    let has_wifi = network::detect_wifi_adapter();
                    let wifi = network::get_wifi_status();
                    let lan = network::get_lan_status();
                    Ok((wifi, lan, has_wifi))
                },
                |result| Message::Network(NetworkMessage::StatusComplete(result)),
            )
        }

        NetworkMessage::StatusComplete(result) => {
            let state = app.settings.network.as_mut().unwrap();
            match result {
                Ok((wifi, lan, has_wifi)) => {
                    state.wifi_status = wifi;
                    state.lan_status = lan;
                    state.has_wifi_adapter = has_wifi;
                }
                Err(e) => {
                    state.error_message = e;
                }
            }
            Task::none()
        }

        NetworkMessage::SelectNetwork(idx) => {
            let state = app.settings.network.as_mut().unwrap();
            if let Some(network) = state.networks.get(idx).cloned() {
                if network.secured {
                    // Open on-screen keyboard for password entry
                    app.keyboard.open(
                        format!("WiFi password for \"{}\"", network.ssid),
                        true, // masked
                    );
                    // Store SSID for when keyboard submits
                    // We use the keyboard prompt to carry the SSID context
                    state.selected_network = Some(idx);
                } else {
                    // Connect directly to open network
                    state.connecting = true;
                    state.error_message.clear();
                    let ssid = network.ssid.clone();
                    return Task::perform(
                        async move { network::connect_wifi(&ssid, None) },
                        |result| Message::Network(NetworkMessage::ConnectComplete(result)),
                    );
                }
            }
            Task::none()
        }

        NetworkMessage::ConnectOpen(ssid) => {
            let state = app.settings.network.as_mut().unwrap();
            state.connecting = true;
            state.wifi_status = WifiStatus::Connecting { ssid: ssid.clone() };
            state.error_message.clear();
            Task::perform(
                async move { network::connect_wifi(&ssid, None) },
                |result| Message::Network(NetworkMessage::ConnectComplete(result)),
            )
        }

        NetworkMessage::ConnectSecured { ssid, password } => {
            let state = app.settings.network.as_mut().unwrap();
            state.connecting = true;
            state.wifi_status = WifiStatus::Connecting { ssid: ssid.clone() };
            state.error_message.clear();
            Task::perform(
                async move { network::connect_wifi(&ssid, Some(&password)) },
                |result| Message::Network(NetworkMessage::ConnectComplete(result)),
            )
        }

        NetworkMessage::ConnectComplete(result) => {
            let state = app.settings.network.as_mut().unwrap();
            state.connecting = false;
            match result {
                Ok(()) => {
                    state.error_message.clear();
                    // Refresh status to get IP and signal
                    return Task::perform(
                        async {
                            let has_wifi = network::detect_wifi_adapter();
                            let wifi = network::get_wifi_status();
                            let lan = network::get_lan_status();
                            Ok((wifi, lan, has_wifi))
                        },
                        |result| Message::Network(NetworkMessage::StatusComplete(result)),
                    );
                }
                Err(e) => {
                    state.wifi_status = WifiStatus::Disconnected;
                    state.error_message = e;
                }
            }
            Task::none()
        }

        NetworkMessage::Disconnect => {
            let state = app.settings.network.as_mut().unwrap();
            state.error_message.clear();
            Task::perform(
                async { network::disconnect_wifi() },
                |result| Message::Network(NetworkMessage::DisconnectComplete(result)),
            )
        }

        NetworkMessage::DisconnectComplete(result) => {
            let state = app.settings.network.as_mut().unwrap();
            match result {
                Ok(()) => {
                    state.wifi_status = WifiStatus::Disconnected;
                    state.error_message.clear();
                }
                Err(e) => {
                    state.error_message = e;
                }
            }
            Task::none()
        }

        NetworkMessage::ScrollNetworks(delta) => {
            let state = app.settings.network.as_mut().unwrap();
            let count = state.networks.len();
            if count > 0 {
                let current = state.selected_network.unwrap_or(0);
                state.selected_network = Some(if delta > 0 {
                    (current + 1) % count
                } else {
                    (current + count - 1) % count
                });
            }
            Task::none()
        }
    }
}

/// Initialize network state if nmcli is available
pub fn init_network_state() -> Option<NetworkState> {
    if network::is_nmcli_available() {
        Some(NetworkState::new())
    } else {
        log::info!("nmcli not found — network settings disabled");
        None
    }
}
