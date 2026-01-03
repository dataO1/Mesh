//! Staging view UI - Import stems and analyze

use super::app::{Message, StagingState};
use iced::widget::{button, column, container, progress_bar, row, text, text_input, Space};
use iced::{Alignment, Element, Length};

/// Render the staging view
pub fn view(state: &StagingState) -> Element<Message> {
    let stem_selector = view_stem_selectors(state);
    let analysis_section = view_analysis_section(state);
    let export_section = view_export_section(state);

    column![stem_selector, analysis_section, export_section,]
        .spacing(20)
        .width(Length::Fill)
        .into()
}

/// Stem file selectors
fn view_stem_selectors(state: &StagingState) -> Element<Message> {
    let stems = [
        ("Vocals", state.importer.vocals_path.as_ref()),
        ("Drums", state.importer.drums_path.as_ref()),
        ("Bass", state.importer.bass_path.as_ref()),
        ("Other", state.importer.other_path.as_ref()),
    ];

    let selectors: Vec<Element<Message>> = stems
        .iter()
        .enumerate()
        .map(|(i, (name, path))| {
            let status = if path.is_some() { "✓" } else { "○" };
            let path_text = path
                .map(|p| {
                    p.file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Selected".to_string())
                })
                .unwrap_or_else(|| "Select file...".to_string());

            let btn = button(text(format!("{} {}", status, name)))
                .on_press(Message::SelectStemFile(i))
                .width(Length::Fixed(120.0));

            row![btn, text(path_text).size(14)]
                .spacing(10)
                .align_y(Alignment::Center)
                .into()
        })
        .collect();

    let title = text("Import Stems").size(18);
    let status = text(format!(
        "{}/4 stems loaded",
        state.importer.loaded_count()
    ))
    .size(14);

    container(
        column![title, column(selectors).spacing(8), status,].spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Analysis section
fn view_analysis_section(state: &StagingState) -> Element<Message> {
    let title = text("Analysis").size(18);

    // Allow (re-)analysis when all stems are loaded
    let can_analyze = state.importer.is_complete();
    let analyze_btn = button(text(if state.analysis_result.is_some() { "Re-Analyze" } else { "Analyze" }))
        .on_press_maybe(can_analyze.then_some(Message::StartAnalysis));

    let progress: Element<Message> = if let Some(progress) = state.analysis_progress {
        container(progress_bar(0.0..=1.0, progress))
            .width(Length::Fill)
            .height(20.0)
            .into()
    } else {
        Space::new().height(20.0).into()
    };

    let results: Element<Message> = if let Some(ref result) = state.analysis_result {
        column![
            row![text("BPM:"), text(format!("{:.1}", result.bpm))].spacing(10),
            row![text("Key:"), text(&result.key)].spacing(10),
            row![
                text("Grid:"),
                text(format!("{} beats", result.beat_grid.len()))
            ]
            .spacing(10),
        ]
        .spacing(5)
        .into()
    } else {
        text("No analysis yet").size(14).into()
    };

    container(column![title, analyze_btn, progress, results,].spacing(10))
        .padding(15)
        .width(Length::Fill)
        .into()
}

/// Export section
fn view_export_section(state: &StagingState) -> Element<Message> {
    let title = text("Add to Collection").size(18);

    let name_input = text_input("Track name...", &state.track_name)
        .on_input(Message::SetTrackName)
        .width(Length::Fill);

    let can_export = state.analysis_result.is_some() && !state.track_name.is_empty();
    let export_btn = button(text("Add to Collection"))
        .on_press_maybe(can_export.then_some(Message::AddToCollection));

    let status = text(&state.status).size(14);

    container(column![title, name_input, export_btn, status,].spacing(10))
        .padding(15)
        .width(Length::Fill)
        .into()
}
