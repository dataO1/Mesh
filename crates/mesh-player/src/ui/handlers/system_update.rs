//! System update message handler
//!
//! Handles version checking, update installation, and journal polling.
//! All blocking commands run inside Task::perform async blocks.

use iced::Task;

use crate::ui::app::MeshApp;
use crate::ui::message::Message;
use crate::ui::system_update::{
    self, SystemUpdateMessage, UpdateCheckStatus, UpdateInstallStatus,
};

/// Handle system update messages
pub fn handle(app: &mut MeshApp, msg: SystemUpdateMessage) -> Task<Message> {
    // Bail if no update state (not on NixOS)
    let state = match app.settings.update.as_mut() {
        Some(s) => s,
        None => return Task::none(),
    };

    match msg {
        SystemUpdateMessage::CheckForUpdate => {
            state.check_status = UpdateCheckStatus::Checking;
            let prerelease = app.config.updates.prerelease_channel;
            Task::perform(
                async move { system_update::check_latest_version(prerelease) },
                |result| Message::SystemUpdate(SystemUpdateMessage::CheckComplete(result)),
            )
        }

        SystemUpdateMessage::CheckComplete(result) => {
            let state = app.settings.update.as_mut().unwrap();
            match result {
                Ok(Some(version)) => {
                    state.available_version = Some(version.clone());
                    state.check_status = UpdateCheckStatus::Available(version);
                }
                Ok(None) => {
                    state.check_status = UpdateCheckStatus::UpToDate;
                }
                Err(e) => {
                    state.check_status = UpdateCheckStatus::Error(e);
                }
            }
            Task::none()
        }

        SystemUpdateMessage::InstallUpdate => {
            let state = app.settings.update.as_mut().unwrap();
            if let Some(version) = state.available_version.clone() {
                state.install_status = UpdateInstallStatus::Starting;
                state.journal_lines.clear();
                Task::perform(
                    async move { system_update::start_update(&version) },
                    |result| Message::SystemUpdate(SystemUpdateMessage::InstallStarted(result)),
                )
            } else {
                Task::none()
            }
        }

        SystemUpdateMessage::InstallStarted(result) => {
            let state = app.settings.update.as_mut().unwrap();
            match result {
                Ok(()) => {
                    state.install_status = UpdateInstallStatus::Installing;
                    // Anchor journal queries to *now* so we capture all build output
                    // regardless of how long the update takes.
                    state.install_started_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs());
                    state.install_tick = 0;
                    // First journal poll will happen via subscription timer
                }
                Err(e) => {
                    state.install_status = UpdateInstallStatus::Error(e);
                }
            }
            Task::none()
        }

        SystemUpdateMessage::PollJournal => {
            let since = app.settings.update.as_ref()
                .and_then(|s| s.install_started_at);
            Task::perform(
                async move { system_update::poll_journal(since) },
                |result| Message::SystemUpdate(SystemUpdateMessage::JournalUpdate(result)),
            )
        }

        SystemUpdateMessage::JournalUpdate(result) => {
            let state = app.settings.update.as_mut().unwrap();
            match result {
                Ok((lines, still_active)) => {
                    if !lines.is_empty() {
                        state.journal_lines = lines;
                    }
                    state.install_tick = state.install_tick.wrapping_add(1);
                    if !still_active && state.install_status == UpdateInstallStatus::Installing {
                        // Service finished — check result
                        return Task::perform(
                            async { system_update::check_update_result() },
                            |result| Message::SystemUpdate(
                                SystemUpdateMessage::InstallComplete(result)
                            ),
                        );
                    }
                }
                Err(e) => {
                    log::warn!("Journal poll error: {}", e);
                }
            }
            Task::none()
        }

        SystemUpdateMessage::InstallComplete(result) => {
            let state = app.settings.update.as_mut().unwrap();
            match result {
                Ok(()) => {
                    state.install_status = UpdateInstallStatus::Complete;
                }
                Err(e) => {
                    state.install_status = UpdateInstallStatus::Error(e);
                }
            }
            Task::none()
        }

        SystemUpdateMessage::RestartCage => {
            // This will kill our own process — cage restarts with new binary
            Task::perform(
                async { system_update::restart_cage() },
                |_| Message::SystemUpdate(SystemUpdateMessage::RestartInitiated),
            )
        }

        SystemUpdateMessage::RestartInitiated => {
            // We likely won't reach here — cage restart kills us
            Task::none()
        }
    }
}

/// Initialize update state if on NixOS
pub fn init_update_state() -> Option<system_update::UpdateState> {
    if system_update::is_nixos() {
        Some(system_update::UpdateState::new())
    } else {
        log::info!("Not on NixOS — system update disabled");
        None
    }
}
