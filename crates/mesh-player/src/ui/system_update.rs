//! OTA system update state, version check, and view for settings UI.
//!
//! Checks GitHub releases for new versions, installs via the
//! `mesh-update` systemd service, and restarts the cage compositor.
//!
//! Only active on NixOS embedded (detected by `/etc/NIXOS` existence).

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Color, Element, Length};

use super::message::Message;

// ── State Types ──

/// Status of version check
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateCheckStatus {
    Idle,
    Checking,
    UpToDate,
    Available(String),
    Error(String),
}

/// Status of update installation
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateInstallStatus {
    Idle,
    Starting,
    Installing,
    Complete,
    Error(String),
}

/// System update state. Lives on SettingsState as `Option<UpdateState>`.
/// None when not on NixOS.
#[derive(Debug, Clone)]
pub struct UpdateState {
    /// Current running version (from Cargo.toml)
    pub current_version: String,
    /// Latest available version (from GitHub)
    pub available_version: Option<String>,
    /// Version check status
    pub check_status: UpdateCheckStatus,
    /// Installation status
    pub install_status: UpdateInstallStatus,
    /// Journal output lines from the update service
    pub journal_lines: Vec<String>,
}

impl UpdateState {
    pub fn new() -> Self {
        Self {
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            available_version: None,
            check_status: UpdateCheckStatus::Idle,
            install_status: UpdateInstallStatus::Idle,
            journal_lines: Vec::new(),
        }
    }

    /// Whether the update service is currently running (need journal polling)
    pub fn is_installing(&self) -> bool {
        self.install_status == UpdateInstallStatus::Installing
    }

    /// Whether installation completed successfully (ready for restart)
    pub fn is_install_complete(&self) -> bool {
        self.install_status == UpdateInstallStatus::Complete
    }

    /// Whether a newer version is available for install
    pub fn has_available_update(&self) -> bool {
        matches!(self.check_status, UpdateCheckStatus::Available(_))
    }
}

/// Messages for system update management
#[derive(Debug, Clone)]
pub enum SystemUpdateMessage {
    /// Check GitHub for latest release
    CheckForUpdate,
    /// Version check completed
    CheckComplete(Result<Option<String>, String>),
    /// Start the update installation
    InstallUpdate,
    /// Update service started (or failed to start)
    InstallStarted(Result<(), String>),
    /// Poll journal for progress
    PollJournal,
    /// Journal poll result: (lines, still_active)
    JournalUpdate(Result<(Vec<String>, bool), String>),
    /// Update service completed
    InstallComplete(Result<(), String>),
    /// Restart the cage compositor to run new version
    RestartCage,
    /// Cage restart initiated
    RestartInitiated,
}

// ── Shell Command Wrappers ──

/// Check if this is a NixOS system
pub fn is_nixos() -> bool {
    std::path::Path::new("/etc/NIXOS").exists()
}

/// Fetch the latest release tag from GitHub
pub fn check_latest_version() -> Result<Option<String>, String> {
    let output = std::process::Command::new("curl")
        .args(["-s", "--max-time", "10",
               "https://api.github.com/repos/dataO1/Mesh/releases/latest"])
        .output()
        .map_err(|e| format!("Failed to run curl: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl failed: {}", stderr));
    }

    let body = String::from_utf8_lossy(&output.stdout);

    // Simple JSON parsing for "tag_name": "v0.9.2"
    // Avoids serde_json dependency for a single field
    let tag = body.split("\"tag_name\"")
        .nth(1)
        .and_then(|rest| rest.split('"').nth(2))
        .map(|s| s.to_string());

    match tag {
        Some(version) => {
            let current = env!("CARGO_PKG_VERSION");
            let remote = version.strip_prefix('v').unwrap_or(&version);
            if is_newer(remote, current) {
                Ok(Some(version))
            } else {
                Ok(None) // Up to date
            }
        }
        None => Err("Could not parse release tag from GitHub API".to_string()),
    }
}

/// Simple semver comparison: is `remote` newer than `current`?
fn is_newer(remote: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.').filter_map(|p| p.parse().ok()).collect()
    };
    let r = parse(remote);
    let c = parse(current);

    for i in 0..r.len().max(c.len()) {
        let rv = r.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if rv > cv { return true; }
        if rv < cv { return false; }
    }
    false
}

/// Write update target version and start the mesh-update service
pub fn start_update(version: &str) -> Result<(), String> {
    // Write version to target file
    std::fs::create_dir_all("/var/lib/mesh")
        .map_err(|e| format!("Failed to create /var/lib/mesh: {}", e))?;
    std::fs::write("/var/lib/mesh/update-target", version)
        .map_err(|e| format!("Failed to write update target: {}", e))?;

    // Start the update service via systemctl (polkit allows this for mesh user)
    let output = std::process::Command::new("systemctl")
        .args(["start", "mesh-update.service"])
        .output()
        .map_err(|e| format!("Failed to start update service: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("Failed to start update: {}", stderr.trim()))
    }
}

/// Poll journal for update service progress
/// Returns (lines, still_active)
pub fn poll_journal() -> Result<(Vec<String>, bool), String> {
    // Get recent journal entries
    let output = std::process::Command::new("journalctl")
        .args(["-u", "mesh-update", "--since", "5 min ago",
               "-n", "20", "--no-pager"])
        .output()
        .map_err(|e| format!("Failed to read journal: {}", e))?;

    let lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();

    // Check if service is still active
    let status = std::process::Command::new("systemctl")
        .args(["is-active", "mesh-update.service"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let still_active = status == "activating" || status == "active";
    Ok((lines, still_active))
}

/// Check the final result of the update service
pub fn check_update_result() -> Result<(), String> {
    let output = std::process::Command::new("systemctl")
        .args(["show", "mesh-update.service", "--property=Result"])
        .output()
        .map_err(|e| format!("Failed to check result: {}", e))?;

    let result = String::from_utf8_lossy(&output.stdout);
    if result.contains("success") {
        Ok(())
    } else {
        Err(format!("Update failed: {}", result.trim()))
    }
}

/// Restart the cage compositor to pick up the new binary
pub fn restart_cage() -> Result<(), String> {
    let output = std::process::Command::new("systemctl")
        .args(["restart", "cage-tty1.service"])
        .output()
        .map_err(|e| format!("Failed to restart cage: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("Restart failed: {}", stderr.trim()))
    }
}

// ── View ──

/// Wrap an element with a highlight background when it's the focused sub-panel action.
fn wrap_focus<'a>(
    elem: Element<'a, Message>,
    action_idx: usize,
    focused: Option<usize>,
) -> Element<'a, Message> {
    let bg = if focused == Some(action_idx) {
        Color::from_rgba(0.3, 0.5, 1.0, 0.3)
    } else {
        Color::TRANSPARENT
    };
    container(elem)
        .style(move |_theme| container::Style {
            background: Some(bg.into()),
            border: iced::Border { radius: 4.0.into(), ..Default::default() },
            ..Default::default()
        })
        .padding(2)
        .width(Length::Fill)
        .into()
}

/// Render the system update settings section.
/// `focused_action` is Some(idx) when in sub-panel (0=Check, 1=Install/Restart).
pub fn view_update_section(state: &UpdateState, focused_action: Option<usize>) -> Element<'_, Message> {
    let section_title = text("System Update").size(18);

    let version_label = text(format!("Current version: v{}", state.current_version))
        .size(12)
        .color(Color::from_rgb(0.5, 0.5, 0.5));

    let mut content_items: Vec<Element<'_, Message>> = vec![
        section_title.into(),
        version_label.into(),
    ];

    // Status and action based on current state
    match &state.check_status {
        UpdateCheckStatus::Idle => {
            let check_btn = button(text("Check for Updates").size(11))
                .on_press(Message::SystemUpdate(SystemUpdateMessage::CheckForUpdate))
                .style(button::secondary);
            content_items.push(wrap_focus(check_btn.into(), 0, focused_action));
        }
        UpdateCheckStatus::Checking => {
            let label = text("Checking for updates...")
                .size(12)
                .color(Color::from_rgb(0.7, 0.7, 0.3));
            content_items.push(label.into());
        }
        UpdateCheckStatus::UpToDate => {
            let label = text("Up to date")
                .size(12)
                .color(Color::from_rgb(0.4, 0.8, 0.4));
            let check_btn = button(text("Check Again").size(11))
                .on_press(Message::SystemUpdate(SystemUpdateMessage::CheckForUpdate))
                .style(button::secondary);
            content_items.push(wrap_focus(
                row![label, Space::new().width(Length::Fill), check_btn]
                    .align_y(Alignment::Center)
                    .into(),
                0,
                focused_action,
            ));
        }
        UpdateCheckStatus::Available(version) => {
            let label = text(format!("{} available", version))
                .size(12)
                .color(Color::from_rgb(0.3, 0.7, 1.0));

            match &state.install_status {
                UpdateInstallStatus::Idle => {
                    let install_btn = button(text("Install").size(11))
                        .on_press(Message::SystemUpdate(SystemUpdateMessage::InstallUpdate))
                        .style(button::primary);
                    content_items.push(wrap_focus(
                        row![label, Space::new().width(Length::Fill), install_btn]
                            .align_y(Alignment::Center)
                            .into(),
                        1,
                        focused_action,
                    ));
                }
                UpdateInstallStatus::Starting => {
                    let status = text("Starting update...")
                        .size(12)
                        .color(Color::from_rgb(0.7, 0.7, 0.3));
                    content_items.push(row![label, Space::new().width(8), status].into());
                }
                UpdateInstallStatus::Installing => {
                    let status = text("Installing...")
                        .size(12)
                        .color(Color::from_rgb(0.7, 0.7, 0.3));
                    content_items.push(row![label, Space::new().width(8), status].into());
                }
                UpdateInstallStatus::Complete => {
                    let status = text("Update complete!")
                        .size(12)
                        .color(Color::from_rgb(0.4, 0.8, 0.4));
                    let restart_btn = button(text("Restart Now").size(11))
                        .on_press(Message::SystemUpdate(SystemUpdateMessage::RestartCage))
                        .style(button::primary);
                    content_items.push(wrap_focus(
                        row![status, Space::new().width(Length::Fill), restart_btn]
                            .align_y(Alignment::Center)
                            .into(),
                        1,
                        focused_action,
                    ));
                }
                UpdateInstallStatus::Error(e) => {
                    let err = text(e)
                        .size(11)
                        .color(Color::from_rgb(1.0, 0.4, 0.4));
                    content_items.push(err.into());
                }
            }
        }
        UpdateCheckStatus::Error(e) => {
            let err = text(e)
                .size(11)
                .color(Color::from_rgb(1.0, 0.4, 0.4));
            let retry_btn = button(text("Retry").size(11))
                .on_press(Message::SystemUpdate(SystemUpdateMessage::CheckForUpdate))
                .style(button::secondary);
            content_items.push(wrap_focus(
                row![err, Space::new().width(Length::Fill), retry_btn]
                    .align_y(Alignment::Center)
                    .into(),
                0,
                focused_action,
            ));
        }
    }

    // Journal output (during installation)
    if !state.journal_lines.is_empty() {
        let journal_text = state.journal_lines.join("\n");
        let journal_view = scrollable(
            text(journal_text)
                .size(9)
                .color(Color::from_rgb(0.6, 0.6, 0.6))
        )
        .height(Length::Fixed(120.0));

        let journal_container = container(journal_view)
            .padding(8)
            .width(Length::Fill)
            .style(|_theme| container::Style {
                background: Some(Color::from_rgba(0.08, 0.08, 0.1, 1.0).into()),
                border: iced::Border {
                    color: Color::from_rgba(0.25, 0.25, 0.3, 1.0),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            });

        content_items.push(journal_container.into());
    }

    container(column(content_items).spacing(8))
        .padding(15)
        .width(Length::Fill)
        .into()
}
