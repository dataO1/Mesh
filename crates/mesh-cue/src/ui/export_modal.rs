//! USB Export modal UI
//!
//! Provides a modal dialog for exporting playlists to USB devices.
//! Shows detected devices, playlist selection, sync progress, and export controls.

use super::app::Message;
use super::state::export::{ExportPhase, ExportState};
use iced::widget::{
    button, checkbox, column, container, pick_list, progress_bar, row, rule,
    scrollable, text, Space,
};
use iced::{Alignment, Element, Length};
use mesh_core::playlist::NodeId;

/// Render the export modal content
pub fn view(state: &ExportState, playlists: Vec<(NodeId, String)>) -> Element<'static, Message> {
    let title = text("Export to USB").size(24);
    let close_btn = button(text("×").size(20))
        .on_press(Message::CloseExport)
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // Content depends on current phase
    let content: Element<Message> = match &state.phase {
        ExportPhase::SelectDevice => view_device_selection(state, &playlists),
        ExportPhase::Mounting { device_label } => view_mounting(device_label),
        ExportPhase::ScanningUsb => view_scanning_usb(),
        ExportPhase::BuildingSyncPlan {
            files_hashed,
            total_files,
        } => view_building_plan(*files_hashed, *total_files),
        ExportPhase::ReadyToSync { plan } => view_ready_to_sync(state, &playlists, plan),
        ExportPhase::Exporting {
            current_file,
            files_complete,
            bytes_complete,
            total_files,
            total_bytes,
            start_time,
        } => view_exporting(
            current_file,
            *files_complete,
            *bytes_complete,
            *total_files,
            *total_bytes,
            start_time,
        ),
        ExportPhase::Complete {
            duration,
            files_exported,
            failed_files,
        } => view_complete(duration, *files_exported, failed_files),
        ExportPhase::Error(msg) => view_error(msg),
    };

    let body = column![header, content]
        .spacing(20)
        .width(Length::Fixed(600.0));

    container(body)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// Device selection view (initial state)
fn view_device_selection(
    state: &ExportState,
    playlists: &[(NodeId, String)],
) -> Element<'static, Message> {
    // Device dropdown - use display_info() for consistent human-readable formatting
    let device_options: Vec<String> = state
        .devices
        .iter()
        .map(|d| d.display_info())
        .collect();

    let selected_label = state
        .selected_device
        .and_then(|idx| device_options.get(idx))
        .cloned();

    let device_label = text("Device:").size(14);
    let device_options_for_closure = device_options.clone();
    let device_picker = pick_list(device_options, selected_label, move |selected| {
        let idx = device_options_for_closure
            .iter()
            .position(|o| o == &selected)
            .unwrap_or(0);
        Message::SelectExportDevice(idx)
    })
    .width(Length::Fill)
    .placeholder("Select a USB device...");

    let device_row = row![device_label, device_picker]
        .spacing(10)
        .align_y(Alignment::Center);

    let no_devices_hint = if state.devices.is_empty() {
        text("No USB devices detected. Insert a USB drive to continue.")
            .size(12)
            .color(iced::Color::from_rgb(0.7, 0.5, 0.2))
    } else {
        text("").size(1)
    };

    // Playlist checkboxes
    let playlists_title = text("Select Playlists to Export").size(16);

    let playlist_items: Vec<Element<Message>> = playlists
        .iter()
        .filter(|(id, _)| id.0.starts_with("playlists/") && !id.0.ends_with("/playlists"))
        .map(|(id, name)| {
            let id_owned = id.clone();
            let name_owned = name.clone();
            let is_selected = state.is_playlist_selected(id);
            checkbox(is_selected)
                .label(name_owned)
                .on_toggle(move |_| Message::ToggleExportPlaylist(id_owned.clone()))
                .size(16)
                .into()
        })
        .collect();

    let playlists_content: Element<Message> = if playlist_items.is_empty() {
        text("No playlists available. Create playlists first.")
            .size(14)
            .into()
    } else {
        scrollable(column(playlist_items).spacing(8))
            .height(Length::Fixed(200.0))
            .into()
    };

    // Config checkbox
    let config_checkbox = checkbox(state.export_config)
        .label("Include audio/display settings")
        .on_toggle(|_| Message::ToggleExportConfig)
        .size(16);

    // Status
    let status = text(format!(
        "{} playlist(s) selected",
        state.selected_playlists.len()
    ))
    .size(12);

    // Action buttons
    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CloseExport)
        .style(button::secondary);

    let export_btn = if state.can_start_export() {
        button(text("Calculate Changes"))
            .on_press(Message::BuildSyncPlan)
            .style(button::primary)
    } else {
        button(text("Calculate Changes")).style(button::secondary)
    };

    let actions = row![Space::new().width(Length::Fill), cancel_btn, export_btn]
        .spacing(10)
        .width(Length::Fill);

    column![
        device_row,
        no_devices_hint,
        rule::horizontal(1),
        playlists_title,
        playlists_content,
        config_checkbox,
        status,
        actions,
    ]
    .spacing(15)
    .into()
}

/// View while mounting device
fn view_mounting(device_label: &str) -> Element<'static, Message> {
    column![
        text(format!("Mounting {}...", device_label)).size(16),
        container(progress_bar(0.0..=1.0, 0.3)).width(Length::Fill),
    ]
    .spacing(10)
    .into()
}

/// View while scanning USB playlists
fn view_scanning_usb() -> Element<'static, Message> {
    column![
        text("Scanning USB playlists...").size(16),
        container(progress_bar(0.0..=1.0, 0.5)).width(Length::Fill),
    ]
    .spacing(10)
    .into()
}

/// View while building sync plan
fn view_building_plan(files_hashed: usize, total_files: usize) -> Element<'static, Message> {
    let progress = if total_files > 0 {
        files_hashed as f32 / total_files as f32
    } else {
        0.0
    };

    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CancelExport)
        .style(button::danger);

    column![
        text(format!(
            "Calculating changes: {}/{} files hashed",
            files_hashed, total_files
        ))
        .size(16),
        container(progress_bar(0.0..=1.0, progress)).width(Length::Fill),
        row![Space::new().width(Length::Fill), cancel_btn],
    ]
    .spacing(10)
    .into()
}

/// View when sync plan is ready
fn view_ready_to_sync(
    state: &ExportState,
    _playlists: &[(NodeId, String)],
    plan: &mesh_core::usb::SyncPlan,
) -> Element<'static, Message> {
    let summary_title = text("Sync Summary").size(18);

    // Summary stats
    let copy_count = plan.to_copy.len();
    let delete_count = plan.to_delete.len();
    let unchanged_count = plan.unchanged.len();
    let total_mb = plan.total_bytes as f64 / 1_000_000.0;

    let copy_text = text(format!("{} files to copy ({:.1} MB)", copy_count, total_mb))
        .size(14)
        .color(if copy_count > 0 {
            iced::Color::from_rgb(0.2, 0.7, 0.2)
        } else {
            iced::Color::from_rgb(0.5, 0.5, 0.5)
        });

    let delete_text = text(format!("{} files to delete", delete_count))
        .size(14)
        .color(if delete_count > 0 {
            iced::Color::from_rgb(0.9, 0.4, 0.2)
        } else {
            iced::Color::from_rgb(0.5, 0.5, 0.5)
        });

    let unchanged_text = text(format!("{} files unchanged (skipped)", unchanged_count))
        .size(14)
        .color(iced::Color::from_rgb(0.5, 0.5, 0.5));

    // Device space check
    let device_info: Element<Message> = if let Some(device) = state.selected_device() {
        let available_mb = device.available_bytes as f64 / 1_000_000.0;
        let has_space = plan.total_bytes <= device.available_bytes;

        let space_color = if has_space {
            iced::Color::from_rgb(0.2, 0.7, 0.2)
        } else {
            iced::Color::from_rgb(0.9, 0.3, 0.2)
        };

        text(format!("{:.1} MB available on {}", available_mb, device.label))
            .size(12)
            .color(space_color)
            .into()
    } else {
        text("").size(1).into()
    };

    // Config export reminder
    let config_note = if state.export_config {
        text("Audio/display settings will be included")
            .size(12)
            .color(iced::Color::from_rgb(0.5, 0.5, 0.5))
    } else {
        text("").size(1)
    };

    // Show a few files that will be copied
    let files_preview: Element<Message> = if !plan.to_copy.is_empty() {
        let items: Vec<Element<Message>> = plan
            .to_copy
            .iter()
            .take(5)
            .map(|f| {
                let name = f
                    .destination
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| f.destination.display().to_string());
                let size_kb = f.size as f64 / 1000.0;
                text(format!("  {} ({:.1} KB)", name, size_kb))
                    .size(12)
                    .into()
            })
            .collect();

        let mut preview_col = column(items).spacing(2);
        if plan.to_copy.len() > 5 {
            preview_col = preview_col.push(
                text(format!("  ... and {} more", plan.to_copy.len() - 5))
                    .size(12)
                    .color(iced::Color::from_rgb(0.5, 0.5, 0.5)),
            );
        }

        column![text("Files to copy:").size(14), preview_col]
            .spacing(4)
            .into()
    } else {
        text("All files are up to date!")
            .size(14)
            .color(iced::Color::from_rgb(0.2, 0.7, 0.2))
            .into()
    };

    // Check if we have space
    let can_export = state
        .selected_device()
        .map(|d| plan.total_bytes <= d.available_bytes && !plan.is_empty())
        .unwrap_or(false);

    // Action buttons
    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CloseExport)
        .style(button::secondary);

    let export_btn = if can_export {
        button(text("Start Export"))
            .on_press(Message::StartExport)
            .style(button::primary)
    } else if plan.is_empty() {
        button(text("Nothing to Export")).style(button::secondary)
    } else {
        button(text("Insufficient Space")).style(button::danger)
    };

    let actions = row![Space::new().width(Length::Fill), cancel_btn, export_btn]
        .spacing(10)
        .width(Length::Fill);

    column![
        summary_title,
        copy_text,
        delete_text,
        unchanged_text,
        device_info,
        config_note,
        rule::horizontal(1),
        files_preview,
        actions,
    ]
    .spacing(10)
    .into()
}

/// View while exporting
fn view_exporting(
    current_file: &str,
    files_complete: usize,
    bytes_complete: u64,
    total_files: usize,
    total_bytes: u64,
    start_time: &std::time::Instant,
) -> Element<'static, Message> {
    let progress = if total_bytes > 0 {
        bytes_complete as f32 / total_bytes as f32
    } else {
        0.0
    };

    // Calculate ETA
    let elapsed = start_time.elapsed();
    let eta_text = if bytes_complete > 0 {
        let rate = bytes_complete as f64 / elapsed.as_secs_f64();
        let remaining_bytes = total_bytes.saturating_sub(bytes_complete);
        let eta_secs = remaining_bytes as f64 / rate;
        if eta_secs > 60.0 {
            format!(
                "ETA: {}m {}s",
                (eta_secs / 60.0) as u32,
                (eta_secs % 60.0) as u32
            )
        } else {
            format!("ETA: {:.0}s", eta_secs)
        }
    } else {
        String::from("Calculating...")
    };

    // Truncate filename if too long
    let display_name = if current_file.len() > 50 {
        format!("...{}", &current_file[current_file.len() - 47..])
    } else {
        current_file.to_string()
    };

    let status = text(format!(
        "Exporting: {} ({}/{})",
        display_name, files_complete, total_files
    ))
    .size(14);

    let bytes_text = text(format!(
        "{:.1} MB / {:.1} MB",
        bytes_complete as f64 / 1_000_000.0,
        total_bytes as f64 / 1_000_000.0
    ))
    .size(12);

    let eta = text(eta_text).size(12);

    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CancelExport)
        .style(button::danger);

    column![
        status,
        container(progress_bar(0.0..=1.0, progress)).width(Length::Fill),
        row![bytes_text, Space::new().width(Length::Fill), eta],
        row![Space::new().width(Length::Fill), cancel_btn],
    ]
    .spacing(10)
    .into()
}

/// View when export is complete
fn view_complete(
    duration: &std::time::Duration,
    files_exported: usize,
    failed_files: &[(std::path::PathBuf, String)],
) -> Element<'static, Message> {
    let duration_text = if duration.as_secs() > 60 {
        format!(
            "Completed in {}m {:.1}s",
            duration.as_secs() / 60,
            duration.as_secs_f64() % 60.0
        )
    } else {
        format!("Completed in {:.1}s", duration.as_secs_f64())
    };

    let success_icon = if failed_files.is_empty() {
        text("✓")
            .size(48)
            .color(iced::Color::from_rgb(0.2, 0.8, 0.2))
    } else {
        text("!")
            .size(48)
            .color(iced::Color::from_rgb(0.9, 0.6, 0.2))
    };

    let summary = if failed_files.is_empty() {
        text(format!("{} files exported successfully", files_exported)).size(16)
    } else {
        text(format!(
            "{} files exported, {} failed",
            files_exported,
            failed_files.len()
        ))
        .size(16)
    };

    let duration_label = text(duration_text).size(12);

    // Show failed files if any
    let failures: Element<Message> = if !failed_files.is_empty() {
        let failed_items: Vec<Element<Message>> = failed_files
            .iter()
            .take(10)
            .map(|(path, error)| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                text(format!("• {}: {}", name, error))
                    .size(12)
                    .color(iced::Color::from_rgb(0.9, 0.3, 0.3))
                    .into()
            })
            .collect();

        let mut failures_col = column(failed_items).spacing(4);
        if failed_files.len() > 10 {
            failures_col = failures_col.push(
                text(format!("  ... and {} more", failed_files.len() - 10))
                    .size(12)
                    .color(iced::Color::from_rgb(0.5, 0.5, 0.5)),
            );
        }

        column![
            text("Failed exports:").size(14),
            scrollable(failures_col).height(Length::Fixed(100.0)),
        ]
        .spacing(5)
        .into()
    } else {
        Space::new().height(0).into()
    };

    let ok_btn = button(text("OK"))
        .on_press(Message::DismissExportResults)
        .style(button::primary);

    let actions = row![Space::new().width(Length::Fill), ok_btn].width(Length::Fill);

    column![
        row![
            success_icon,
            column![summary, duration_label].spacing(5)
        ]
        .spacing(15)
        .align_y(Alignment::Center),
        failures,
        actions,
    ]
    .spacing(15)
    .into()
}

/// View for error state
fn view_error(message: &str) -> Element<'static, Message> {
    let error_icon = text("✗")
        .size(48)
        .color(iced::Color::from_rgb(0.9, 0.3, 0.2));

    let error_text = text(format!("Error: {}", message)).size(14);

    let ok_btn = button(text("OK"))
        .on_press(Message::CloseExport)
        .style(button::secondary);

    let actions = row![Space::new().width(Length::Fill), ok_btn].width(Length::Fill);

    column![
        row![error_icon, error_text]
            .spacing(15)
            .align_y(Alignment::Center),
        actions,
    ]
    .spacing(20)
    .into()
}

/// Render a compact progress bar for the collection browser bottom
///
/// This is displayed at the bottom of the collection view while export is running.
pub fn view_progress_bar(state: &ExportState) -> Option<Element<'static, Message>> {
    match &state.phase {
        ExportPhase::Exporting {
            current_file,
            files_complete,
            bytes_complete,
            total_files,
            total_bytes,
            start_time,
        } => {
            let progress = if *total_bytes > 0 {
                *bytes_complete as f32 / *total_bytes as f32
            } else {
                0.0
            };

            // Calculate ETA
            let elapsed = start_time.elapsed();
            let eta_text = if *bytes_complete > 0 {
                let rate = *bytes_complete as f64 / elapsed.as_secs_f64();
                let remaining = total_bytes.saturating_sub(*bytes_complete);
                let eta_secs = remaining as f64 / rate;
                format!("{}/{}  ETA: {:.0}s", files_complete, total_files, eta_secs)
            } else {
                format!("{}/{}", files_complete, total_files)
            };

            // Truncate filename if too long
            let display_name = if current_file.len() > 40 {
                format!("{}...", &current_file[..37])
            } else {
                current_file.clone()
            };

            Some(super::import_modal::build_status_bar(
                format!("Exporting: {}", display_name),
                eta_text,
                progress,
                Message::CancelExport,
            ))
        }
        ExportPhase::BuildingSyncPlan {
            files_hashed,
            total_files,
        } => {
            let progress = if *total_files > 0 {
                *files_hashed as f32 / *total_files as f32
            } else {
                0.0
            };

            Some(super::import_modal::build_status_bar(
                "Calculating changes...".to_string(),
                format!("{}/{} files", files_hashed, total_files),
                progress,
                Message::CancelExport,
            ))
        }
        _ => None,
    }
}
