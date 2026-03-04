//! OTA system update state, version check, and view for settings UI.
//!
//! Checks GitHub releases for new versions, installs via the
//! `mesh-update` systemd service, and restarts the cage compositor.
//!
//! Only active on NixOS embedded (detected by `/etc/NIXOS` existence).

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Color, Element, Length};
use mesh_widgets::sz;

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

/// Fetch the latest release tag from GitHub.
///
/// When `prerelease` is false, uses `/releases/latest` which only returns
/// stable releases (GitHub excludes prereleases from this endpoint).
/// When `prerelease` is true, fetches all releases and finds the newest
/// version including release candidates and beta versions.
pub fn check_latest_version(prerelease: bool) -> Result<Option<String>, String> {
    let url = if prerelease {
        "https://api.github.com/repos/dataO1/Mesh/releases?per_page=10"
    } else {
        "https://api.github.com/repos/dataO1/Mesh/releases/latest"
    };

    let output = std::process::Command::new("curl")
        .args(["-s", "--max-time", "10", url])
        .output()
        .map_err(|e| format!("Failed to run curl: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl failed: {}", stderr));
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let current = env!("CARGO_PKG_VERSION");

    if prerelease {
        // Parse JSON array: extract all "tag_name" values, find newest
        find_newest_release(&body, current)
    } else {
        // Parse single JSON object: extract "tag_name"
        // GitHub returns: "tag_name": "v0.9.8", — value is at split('"')[1]
        let tag = body.split("\"tag_name\"")
            .nth(1)
            .and_then(|rest| rest.split('"').nth(1))
            .map(|s| s.to_string());

        match tag {
            Some(version) => {
                let remote = version.strip_prefix('v').unwrap_or(&version);
                if is_newer(remote, current) {
                    Ok(Some(version))
                } else {
                    Ok(None)
                }
            }
            None => Err("Could not parse release tag from GitHub API".to_string()),
        }
    }
}

/// Find the newest release from a GitHub `/releases` JSON array response.
///
/// GitHub returns releases newest-first, so the first tag that is newer
/// than our current version wins.
fn find_newest_release(body: &str, current: &str) -> Result<Option<String>, String> {
    // Extract all "tag_name": "vX.Y.Z" values from the JSON array
    // GitHub returns: "tag_name": "v0.9.8", — value is at split('"')[1]
    let tags: Vec<String> = body.split("\"tag_name\"")
        .skip(1) // skip text before first match
        .filter_map(|rest| rest.split('"').nth(1).map(|s| s.to_string()))
        .collect();

    if tags.is_empty() {
        return Err("Could not parse any release tags from GitHub API".to_string());
    }

    // Return the first (newest) tag that is newer than current
    for tag in &tags {
        let remote = tag.strip_prefix('v').unwrap_or(tag);
        if is_newer(remote, current) {
            return Ok(Some(tag.clone()));
        }
    }

    Ok(None) // All releases are older or equal
}

/// Semver comparison with pre-release suffix support.
///
/// Handles versions like "0.9.9", "0.9.9-rc.1", "0.9.9-beta.2".
/// Rules:
/// - Compare base version (MAJOR.MINOR.PATCH) numerically
/// - If base versions are equal: release (no suffix) > pre-release (has suffix)
/// - Among pre-releases with the same base: compare suffix number
fn is_newer(remote: &str, current: &str) -> bool {
    let (r_base, r_pre) = split_version(remote);
    let (c_base, c_pre) = split_version(current);

    let parse_base = |s: &str| -> Vec<u32> {
        s.split('.').filter_map(|p| p.parse().ok()).collect()
    };
    let r = parse_base(r_base);
    let c = parse_base(c_base);

    // Compare base version components
    for i in 0..r.len().max(c.len()) {
        let rv = r.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if rv > cv { return true; }
        if rv < cv { return false; }
    }

    // Base versions are equal — compare pre-release suffixes
    match (r_pre, c_pre) {
        (None, Some(_)) => true,   // "0.9.9" > "0.9.9-rc.1"
        (Some(_), None) => false,  // "0.9.9-rc.1" < "0.9.9"
        (None, None) => false,     // identical
        (Some(r_suffix), Some(c_suffix)) => {
            // Compare suffix numbers: "rc.2" > "rc.1"
            pre_release_num(r_suffix) > pre_release_num(c_suffix)
        }
    }
}

/// Split "0.9.9-rc.1" into ("0.9.9", Some("rc.1"))
fn split_version(v: &str) -> (&str, Option<&str>) {
    match v.split_once('-') {
        Some((base, pre)) => (base, Some(pre)),
        None => (v, None),
    }
}

/// Extract the numeric suffix from a pre-release tag.
/// "rc.1" → 1, "beta.2" → 2, "alpha.3" → 3, "rc" → 0
fn pre_release_num(pre: &str) -> u32 {
    pre.rsplit('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
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
    let section_title = text("System Update").size(sz(18.0));

    let version_label = text(format!("Current version: v{}", state.current_version))
        .size(sz(12.0))
        .color(Color::from_rgb(0.5, 0.5, 0.5));

    let mut content_items: Vec<Element<'_, Message>> = vec![
        section_title.into(),
        version_label.into(),
    ];

    // Status and action based on current state
    match &state.check_status {
        UpdateCheckStatus::Idle => {
            let check_btn = button(text("Check for Updates").size(sz(11.0)))
                .on_press(Message::SystemUpdate(SystemUpdateMessage::CheckForUpdate))
                .style(button::secondary);
            content_items.push(wrap_focus(check_btn.into(), 0, focused_action));
        }
        UpdateCheckStatus::Checking => {
            let label = text("Checking for updates...")
                .size(sz(12.0))
                .color(Color::from_rgb(0.7, 0.7, 0.3));
            content_items.push(label.into());
        }
        UpdateCheckStatus::UpToDate => {
            let label = text("Up to date")
                .size(sz(12.0))
                .color(Color::from_rgb(0.4, 0.8, 0.4));
            let check_btn = button(text("Check Again").size(sz(11.0)))
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
                .size(sz(12.0))
                .color(Color::from_rgb(0.3, 0.7, 1.0));

            match &state.install_status {
                UpdateInstallStatus::Idle => {
                    let install_btn = button(text("Install").size(sz(11.0)))
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
                        .size(sz(12.0))
                        .color(Color::from_rgb(0.7, 0.7, 0.3));
                    content_items.push(row![label, Space::new().width(8), status].into());
                }
                UpdateInstallStatus::Installing => {
                    let status = text("Installing...")
                        .size(sz(12.0))
                        .color(Color::from_rgb(0.7, 0.7, 0.3));
                    content_items.push(row![label, Space::new().width(8), status].into());
                }
                UpdateInstallStatus::Complete => {
                    let status = text("Update complete!")
                        .size(sz(12.0))
                        .color(Color::from_rgb(0.4, 0.8, 0.4));
                    let restart_btn = button(text("Restart Now").size(sz(11.0)))
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
                        .size(sz(11.0))
                        .color(Color::from_rgb(1.0, 0.4, 0.4));
                    content_items.push(err.into());
                }
            }
        }
        UpdateCheckStatus::Error(e) => {
            let err = text(e)
                .size(sz(11.0))
                .color(Color::from_rgb(1.0, 0.4, 0.4));
            let retry_btn = button(text("Retry").size(sz(11.0)))
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
                .size(sz(9.0))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_basic() {
        assert!(is_newer("0.9.9", "0.9.8"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.9.8", "0.9.9"));
        assert!(!is_newer("0.9.9", "0.9.9"));
    }

    #[test]
    fn test_is_newer_prerelease() {
        // RC is newer than previous stable
        assert!(is_newer("0.9.9-rc.1", "0.9.8"));
        // Stable is newer than its own RC
        assert!(is_newer("0.9.9", "0.9.9-rc.1"));
        // RC is not newer than its own stable
        assert!(!is_newer("0.9.9-rc.1", "0.9.9"));
        // Higher RC is newer than lower RC
        assert!(is_newer("0.9.9-rc.2", "0.9.9-rc.1"));
        assert!(!is_newer("0.9.9-rc.1", "0.9.9-rc.2"));
    }

    #[test]
    fn test_is_newer_beta_alpha() {
        assert!(is_newer("1.0.0-beta.1", "0.9.9"));
        assert!(is_newer("1.0.0", "1.0.0-beta.2"));
        assert!(is_newer("1.0.0-beta.2", "1.0.0-beta.1"));
    }

    #[test]
    fn test_split_version() {
        assert_eq!(split_version("0.9.9"), ("0.9.9", None));
        assert_eq!(split_version("0.9.9-rc.1"), ("0.9.9", Some("rc.1")));
        assert_eq!(split_version("1.0.0-beta.2"), ("1.0.0", Some("beta.2")));
    }

    #[test]
    fn test_find_newest_release() {
        // Simulated GitHub API response (newest first)
        let body = r#"[
            {"tag_name": "v0.9.9-rc.2", "prerelease": true},
            {"tag_name": "v0.9.9-rc.1", "prerelease": true},
            {"tag_name": "v0.9.8", "prerelease": false}
        ]"#;

        // Current is 0.9.8 — should find v0.9.9-rc.2 (first newer)
        let result = find_newest_release(body, "0.9.8").unwrap();
        assert_eq!(result, Some("v0.9.9-rc.2".to_string()));

        // Current is 0.9.9-rc.2 — nothing newer
        let result = find_newest_release(body, "0.9.9-rc.2").unwrap();
        assert_eq!(result, None);

        // Current is 0.9.9 — RCs are not newer than stable
        let result = find_newest_release(body, "0.9.9").unwrap();
        assert_eq!(result, None);
    }
}
