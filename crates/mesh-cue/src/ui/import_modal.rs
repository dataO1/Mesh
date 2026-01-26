//! Import modal UI
//!
//! Provides a modal dialog for batch importing audio files from the import folder.
//! Supports two modes:
//! - **Stems mode**: Import pre-separated stem files (Artist - Track_(Vocals).wav, etc.)
//! - **Mixed audio mode**: Import regular audio files and auto-separate into stems

use super::app::{ImportPhase, ImportState, Message};
use super::state::ImportMode;
use crate::batch_import::{MixedAudioFile, StemGroup};
use iced::widget::{button, column, container, progress_bar, row, scrollable, text, Space};
use iced::{Alignment, Element, Length};

/// Render the import modal content
pub fn view(state: &ImportState) -> Element<'_, Message> {
    let title_text = match state.import_mode {
        ImportMode::Stems => "Import Stems",
        ImportMode::MixedAudio => "Import Audio (Auto-Separate)",
    };
    let title = text(title_text).size(24);
    let close_btn = button(text("×").size(20))
        .on_press(Message::CloseImport)
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // Mode toggle buttons
    let stems_btn_base = button(text("Pre-separated Stems"));
    let stems_btn: Element<Message> = if state.import_mode == ImportMode::Stems {
        stems_btn_base.style(button::primary).into()
    } else {
        stems_btn_base
            .on_press(Message::SetImportMode(ImportMode::Stems))
            .style(button::secondary)
            .into()
    };
    let mixed_btn_base = button(text("Mixed Audio"));
    let mixed_btn: Element<Message> = if state.import_mode == ImportMode::MixedAudio {
        mixed_btn_base.style(button::primary).into()
    } else {
        mixed_btn_base
            .on_press(Message::SetImportMode(ImportMode::MixedAudio))
            .style(button::secondary)
            .into()
    };
    let mode_toggle = row![stems_btn, mixed_btn].spacing(8);

    // Import folder display
    let folder_label = text("Import Folder:").size(14);
    let folder_path = text(state.import_folder.display().to_string())
        .size(12);
    let folder_section = column![folder_label, folder_path].spacing(5);

    // Content depends on current phase
    let content: Element<Message> = match &state.phase {
        None => view_scan_results(state),
        Some(ImportPhase::Scanning) => view_scanning(),
        Some(ImportPhase::Processing { current_track, completed, total, start_time }) => {
            view_processing(current_track, *completed, *total, start_time)
        }
        Some(ImportPhase::Complete { duration }) => view_complete(duration, &state.results),
    };

    let body = column![header, mode_toggle, folder_section, content]
        .spacing(20)
        .width(Length::Fixed(550.0));

    container(body)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// View when not importing - show scan results and action buttons
fn view_scan_results(state: &ImportState) -> Element<'_, Message> {
    match state.import_mode {
        ImportMode::Stems => view_scan_results_stems(state),
        ImportMode::MixedAudio => view_scan_results_mixed(state),
    }
}

/// View scan results for stem files
fn view_scan_results_stems(state: &ImportState) -> Element<'_, Message> {
    let groups = &state.detected_groups;

    // Groups list
    let groups_title = text("Detected Tracks").size(18);

    let groups_list: Element<Message> = if groups.is_empty() {
        column![
            text("No stem files found in import folder.").size(14),
            text("Place stems with pattern: Artist - Track_(Vocals).wav").size(12),
        ]
        .spacing(5)
        .into()
    } else {
        let items: Vec<Element<Message>> = groups
            .iter()
            .map(|group| view_stem_group(group))
            .collect();

        scrollable(column(items).spacing(8))
            .height(Length::Fixed(300.0))
            .into()
    };

    // Count complete vs incomplete groups
    let complete_count = groups.iter().filter(|g| g.is_complete()).count();
    let incomplete_count = groups.len() - complete_count;

    let status = if groups.is_empty() {
        text("").size(12)
    } else if incomplete_count > 0 {
        text(format!(
            "{} ready to import, {} incomplete (need all 4 stems)",
            complete_count, incomplete_count
        ))
        .size(12)
    } else {
        text(format!("{} tracks ready to import", complete_count)).size(12)
    };

    // Action buttons
    let refresh_btn = button(text("Refresh"))
        .on_press(Message::ScanImportFolder)
        .style(button::secondary);

    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CloseImport)
        .style(button::secondary);

    let start_btn = if complete_count > 0 {
        button(text("Start Import"))
            .on_press(Message::StartBatchImport)
            .style(button::primary)
    } else {
        button(text("Start Import")).style(button::secondary)
    };

    let actions = row![refresh_btn, Space::new().width(Length::Fill), cancel_btn, start_btn]
        .spacing(10)
        .width(Length::Fill);

    column![groups_title, groups_list, status, actions]
        .spacing(15)
        .into()
}

/// View scan results for mixed audio files
fn view_scan_results_mixed(state: &ImportState) -> Element<'_, Message> {
    let files = &state.detected_mixed_files;

    // Files list
    let files_title = text("Detected Audio Files").size(18);

    let files_list: Element<Message> = if files.is_empty() {
        column![
            text("No audio files found in import folder.").size(14),
            text("Supported formats: MP3, FLAC, WAV, OGG, M4A").size(12),
            text("Files with _(Vocals), _(Drums), etc. are skipped (use Stems mode).").size(11),
        ]
        .spacing(5)
        .into()
    } else {
        let items: Vec<Element<Message>> = files
            .iter()
            .map(|file| view_mixed_audio_file(file))
            .collect();

        scrollable(column(items).spacing(8))
            .height(Length::Fixed(300.0))
            .into()
    };

    let file_count = files.len();
    let status = if files.is_empty() {
        text("").size(12)
    } else {
        text(format!(
            "{} audio {} ready for stem separation",
            file_count,
            if file_count == 1 { "file" } else { "files" }
        ))
        .size(12)
    };

    // Note about stem separation
    let note = text("⚠ Stem separation requires ~4GB RAM per track and may take several minutes.")
        .size(11)
        .color(iced::Color::from_rgb(0.8, 0.6, 0.2));

    // Action buttons
    let refresh_btn = button(text("Refresh"))
        .on_press(Message::ScanImportFolder)
        .style(button::secondary);

    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CloseImport)
        .style(button::secondary);

    let start_btn = if file_count > 0 {
        button(text("Start Import"))
            .on_press(Message::StartMixedAudioImport)
            .style(button::primary)
    } else {
        button(text("Start Import")).style(button::secondary)
    };

    let actions = row![refresh_btn, Space::new().width(Length::Fill), cancel_btn, start_btn]
        .spacing(10)
        .width(Length::Fill);

    column![files_title, files_list, status, note, actions]
        .spacing(15)
        .into()
}

/// View a single stem group
fn view_stem_group(group: &StemGroup) -> Element<'_, Message> {
    let name = text(&group.base_name).size(14);
    let status_icon = if group.is_complete() {
        text("✓").size(14).color(iced::Color::from_rgb(0.2, 0.8, 0.2))
    } else {
        text(format!("{}/4", group.stem_count()))
            .size(12)
            .color(iced::Color::from_rgb(0.8, 0.6, 0.2))
    };

    // Show which stems are present/missing
    let stems_detail = format!(
        "{}{}{}{}",
        if group.vocals.is_some() { "V" } else { "·" },
        if group.drums.is_some() { "D" } else { "·" },
        if group.bass.is_some() { "B" } else { "·" },
        if group.other.is_some() { "O" } else { "·" },
    );
    let stems_text = text(stems_detail)
        .size(12)
        .color(iced::Color::from_rgb(0.5, 0.5, 0.5));

    container(
        row![name, Space::new().width(Length::Fill), stems_text, status_icon]
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .padding(8)
    .width(Length::Fill)
    .style(|theme: &iced::Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(iced::Background::Color(palette.background.weak.color)),
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
    .into()
}

/// View a single mixed audio file
fn view_mixed_audio_file(file: &MixedAudioFile) -> Element<'_, Message> {
    let name = text(&file.base_name).size(14);

    // Get file extension
    let ext = file.path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("?")
        .to_uppercase();
    let ext_text = text(ext)
        .size(11)
        .color(iced::Color::from_rgb(0.5, 0.5, 0.5));

    let status_icon = text("♪")
        .size(14)
        .color(iced::Color::from_rgb(0.4, 0.6, 0.9));

    container(
        row![name, Space::new().width(Length::Fill), ext_text, status_icon]
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .padding(8)
    .width(Length::Fill)
    .style(|theme: &iced::Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(iced::Background::Color(palette.background.weak.color)),
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
    .into()
}

/// View while scanning
fn view_scanning() -> Element<'static, Message> {
    column![
        text("Scanning import folder...").size(16),
        container(progress_bar(0.0..=1.0, 0.5)).width(Length::Fill),
    ]
    .spacing(10)
    .into()
}

/// View while processing tracks
fn view_processing<'a>(
    current_track: &'a str,
    completed: usize,
    total: usize,
    start_time: &'a std::time::Instant,
) -> Element<'a, Message> {
    let progress = if total > 0 {
        completed as f32 / total as f32
    } else {
        0.0
    };

    // Calculate ETA
    let elapsed = start_time.elapsed();
    let eta_text = if completed > 0 {
        let avg_time_per_track = elapsed.as_secs_f64() / completed as f64;
        let remaining = total - completed;
        let eta_secs = (avg_time_per_track * remaining as f64) as u64;
        if eta_secs > 60 {
            format!("ETA: {}m {}s", eta_secs / 60, eta_secs % 60)
        } else {
            format!("ETA: {}s", eta_secs)
        }
    } else {
        String::from("Calculating...")
    };

    let status = text(format!(
        "Importing: {} ({}/{})",
        current_track, completed, total
    ))
    .size(14);

    let eta = text(eta_text).size(12);

    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CancelImport)
        .style(button::danger);

    column![
        status,
        container(progress_bar(0.0..=1.0, progress)).width(Length::Fill),
        row![eta, Space::new().width(Length::Fill), cancel_btn]
            .align_y(Alignment::Center),
    ]
    .spacing(10)
    .into()
}

/// View when import is complete
fn view_complete(
    duration: &std::time::Duration,
    results: &[crate::batch_import::TrackImportResult],
) -> Element<'static, Message> {
    let success_count = results.iter().filter(|r| r.success).count();
    let fail_count = results.len() - success_count;

    let duration_text = if duration.as_secs() > 60 {
        format!(
            "Completed in {}m {:.1}s",
            duration.as_secs() / 60,
            duration.as_secs_f64() % 60.0
        )
    } else {
        format!("Completed in {:.1}s", duration.as_secs_f64())
    };

    let summary = text(format!(
        "{} tracks imported successfully, {} failed",
        success_count, fail_count
    ))
    .size(16);

    let duration_label = text(duration_text).size(12);

    // Show failed tracks if any
    let failures: Element<Message> = if fail_count > 0 {
        let failed_items: Vec<Element<Message>> = results
            .iter()
            .filter(|r| !r.success)
            .map(|r| {
                let error = r.error.as_deref().unwrap_or("Unknown error");
                text(format!("• {}: {}", r.base_name, error))
                    .size(12)
                    .color(iced::Color::from_rgb(0.9, 0.3, 0.3))
                    .into()
            })
            .collect();

        column![
            text("Failed imports:").size(14),
            scrollable(column(failed_items).spacing(4)).height(Length::Fixed(100.0)),
        ]
        .spacing(5)
        .into()
    } else {
        Space::new().height(0).into()
    };

    let ok_btn = button(text("OK"))
        .on_press(Message::DismissImportResults)
        .style(button::primary);

    let actions = row![Space::new().width(Length::Fill), ok_btn]
        .width(Length::Fill);

    column![summary, duration_label, failures, actions]
        .spacing(15)
        .into()
}

/// Render a compact progress bar for the collection browser bottom
///
/// This is displayed at the bottom of the collection view while import is running.
pub fn view_progress_bar(state: &ImportState) -> Option<Element<'static, Message>> {
    match &state.phase {
        Some(ImportPhase::Processing {
            current_track,
            completed,
            total,
            start_time,
        }) => {
            let progress = if *total > 0 {
                *completed as f32 / *total as f32
            } else {
                0.0
            };

            // Calculate ETA
            let elapsed = start_time.elapsed();
            let eta_text = if *completed > 0 {
                let avg_time_per_track = elapsed.as_secs_f64() / *completed as f64;
                let remaining = total - completed;
                let eta_secs = (avg_time_per_track * remaining as f64) as u64;
                format!("{}/{}  ETA: {}s", completed, total, eta_secs)
            } else {
                format!("{}/{}", completed, total)
            };

            // Truncate track name if too long
            let display_name = if current_track.len() > 40 {
                format!("{}...", &current_track[..37])
            } else {
                current_track.clone()
            };

            Some(build_status_bar(
                format!("Importing: {}", display_name),
                eta_text,
                progress,
                Message::CancelImport,
            ))
        }
        _ => None,
    }
}

/// Build a generic status bar with progress indicator
///
/// Reusable for import, re-analysis, and other long-running operations.
pub fn build_status_bar<M: Clone + 'static>(
    label: String,
    progress_text: String,
    progress: f32,
    cancel_message: M,
) -> Element<'static, M> {
    let cancel_btn = button(text("×").size(14))
        .on_press(cancel_message)
        .style(button::secondary)
        .padding([2, 6]);

    let bar = container(
        row![
            text(label).size(12),
            Space::new().width(Length::Fill),
            text(progress_text).size(12),
            cancel_btn,
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .padding([4, 8]),
    )
    .width(Length::Fill);

    let progress_row = column![bar, container(progress_bar(0.0..=1.0, progress)).width(Length::Fill)]
        .spacing(2);

    container(progress_row)
        .width(Length::Fill)
        .padding(8)
        .style(|theme: &iced::Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(iced::Background::Color(
                    palette.background.strong.color,
                )),
                ..Default::default()
            }
        })
        .into()
}
