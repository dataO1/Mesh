//! Settings modal UI
//!
//! Provides a modal dialog for editing application configuration.

use super::app::{Message, SettingsState};
use crate::config::{BackendType, BpmSource, ModelType, SeparationConfig};
use mesh_widgets::{sz, AppFont, FontSize};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input, Space};
use mesh_core::engine::InterpolationMethod;
use iced::{Alignment, Element, Length};

/// Render the settings modal content
pub fn view(state: &SettingsState) -> Element<'_, Message> {
    let title = text("Settings").size(sz(24.0));
    let close_btn = button(text("×").size(sz(20.0)))
        .on_press(Message::CloseSettings)
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // Audio output section (at top)
    let audio_section = view_audio_output_section(state);

    // BPM Range section
    let bpm_section = view_bpm_section(state);

    // Display settings section
    let display_section = view_display_section(state);

    // Track name format section
    let format_section = view_track_name_format_section(state);

    // Status message (for save feedback)
    let status: Element<Message> = if !state.status.is_empty() {
        text(&state.status).size(sz(14.0)).into()
    } else {
        Space::new().height(20).into()
    };

    // Action buttons
    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CloseSettings)
        .style(button::secondary);

    let save_btn = button(text("Save"))
        .on_press(Message::SaveSettings)
        .style(button::primary);

    let actions = row![Space::new().width(Length::Fill), cancel_btn, save_btn]
        .spacing(10)
        .width(Length::Fill);

    // Scrollable content area
    let scrollable_content = scrollable(
        column![audio_section, bpm_section, display_section, format_section]
            .spacing(15)
    )
    .height(Length::Fixed(400.0));

    let content = column![header, scrollable_content, status, actions]
        .spacing(15)
        .width(Length::Fixed(450.0));

    container(content)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// Audio output settings section
fn view_audio_output_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Audio Output").size(sz(18.0));
    let hint = text("Select the audio output for playback preview")
        .size(sz(12.0));

    // Output device dropdown
    let output_label = text("Output:").size(sz(14.0));
    let output_dropdown: Element<'_, Message> = if state.available_stereo_pairs.is_empty() {
        text("No audio outputs available").size(sz(12.0)).into()
    } else {
        pick_list(
            state.available_stereo_pairs.clone(),
            state.available_stereo_pairs.get(state.selected_output_pair).cloned(),
            |pair| {
                // Find index of selected pair
                let idx = state.available_stereo_pairs.iter()
                    .position(|p| p == &pair)
                    .unwrap_or(0);
                Message::UpdateSettingsOutputPair(idx)
            },
        )
        .width(Length::Fixed(200.0))
        .into()
    };

    let output_row = row![output_label, Space::new().width(Length::Fill), output_dropdown]
        .spacing(10)
        .align_y(Alignment::Center);

    // Refresh button
    let refresh_btn = button(text("Refresh Ports").size(sz(11.0)))
        .on_press(Message::RefreshAudioDevices)
        .style(button::secondary);

    // Scratch interpolation subsection
    let scratch_title = text("Scratch Interpolation").size(sz(14.0));
    let scratch_hint = text("Audio quality when scrubbing waveform (Linear = fast, Cubic = smooth)")
        .size(sz(12.0));

    let scratch_options = [InterpolationMethod::Linear, InterpolationMethod::Cubic, InterpolationMethod::Sinc];
    let scratch_buttons: Vec<Element<Message>> = scratch_options
        .iter()
        .map(|&method| {
            let is_selected = state.draft_scratch_interpolation == method;
            let label = match method {
                InterpolationMethod::Linear => "Linear",
                InterpolationMethod::Cubic => "Cubic",
                InterpolationMethod::Sinc => "Sinc",
            };
            let btn = button(text(label).size(sz(12.0)))
                .on_press(Message::UpdateSettingsScratchInterpolation(method))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(70.0));
            btn.into()
        })
        .collect();

    let scratch_label = text("Method:").size(sz(14.0));
    let scratch_row = row![
        scratch_label,
        row(scratch_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    container(
        column![
            section_title,
            hint,
            Space::new().height(5),
            output_row,
            Space::new().height(5),
            refresh_btn,
            Space::new().height(10),
            scratch_title,
            scratch_hint,
            scratch_row,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// BPM detection range settings
fn view_bpm_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Analysis").size(sz(18.0));

    let subsection_title = text("BPM Detection Range").size(sz(14.0));
    let hint = text("Set the expected BPM range for your music genre (e.g., DnB: 160-190)")
        .size(sz(12.0));

    let min_label = text("Min Tempo:").size(sz(14.0));
    let min_input = text_input("40", &state.draft_min_tempo)
        .on_input(Message::UpdateSettingsMinTempo)
        .width(Length::Fixed(80.0));
    let min_range = text("(40-180)").size(sz(12.0));

    let min_row = row![min_label, min_input, min_range]
        .spacing(10)
        .align_y(Alignment::Center);

    let max_label = text("Max Tempo:").size(sz(14.0));
    let max_input = text_input("208", &state.draft_max_tempo)
        .on_input(Message::UpdateSettingsMaxTempo)
        .width(Length::Fixed(80.0));
    let max_range = text("(60-250)").size(sz(12.0));

    let max_row = row![max_label, max_input, max_range]
        .spacing(10)
        .align_y(Alignment::Center);

    // BPM Source subsection
    let source_title = text("BPM Analysis Source").size(sz(14.0));
    let source_hint = text("Which audio to analyze for BPM detection (drums recommended)")
        .size(sz(12.0));

    // Source selection buttons
    let source_options = [BpmSource::Drums, BpmSource::FullMix];
    let source_buttons: Vec<Element<Message>> = source_options
        .iter()
        .map(|&source| {
            let is_selected = state.draft_bpm_source == source;
            let btn = button(text(source.to_string()).size(sz(12.0)))
                .on_press(Message::UpdateSettingsBpmSource(source))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(90.0));
            btn.into()
        })
        .collect();

    let source_label = text("Source:").size(sz(14.0));
    let source_row = row![
        source_label,
        row(source_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);


    // Parallel processes subsection
    let parallel_title = text("Parallel Analysis").size(sz(14.0));
    let parallel_hint = text("Number of tracks to analyze simultaneously during batch import")
        .size(sz(12.0));

    let parallel_label = text("Processes:").size(sz(14.0));
    let parallel_input = text_input("4", &state.draft_parallel_processes)
        .on_input(Message::UpdateSettingsParallelProcesses)
        .width(Length::Fixed(80.0));
    let parallel_range = text("(1-16)").size(sz(12.0));

    let parallel_row = row![parallel_label, parallel_input, parallel_range]
        .spacing(10)
        .align_y(Alignment::Center);

    container(
        column![
            section_title,
            subsection_title,
            hint,
            min_row,
            max_row,
            Space::new().height(10),
            source_title,
            source_hint,
            source_row,
            Space::new().height(10),
            parallel_title,
            parallel_hint,
            parallel_row,
        ]
        .spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Display settings (waveform grid density)
fn view_display_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Display").size(sz(18.0));

    let subsection_title = text("Overview Grid Density").size(sz(14.0));
    let hint = text("Beat grid line spacing on the overview waveform")
        .size(sz(12.0));

    // Grid density buttons (8, 16, 32, 64 beats)
    let grid_sizes: [u32; 4] = [8, 16, 32, 64];
    let grid_buttons: Vec<Element<Message>> = grid_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_grid_bars == size;
            let btn = button(text(format!("{}", size)).size(sz(12.0)))
                .on_press(Message::UpdateSettingsGridBars(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(40.0));
            btn.into()
        })
        .collect();

    let grid_label = text("Beats:").size(sz(14.0));
    let grid_row = row![
        grid_label,
        row(grid_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Slicer buffer size section
    let slicer_title = text("Slicer Buffer Size").size(sz(14.0));
    let slicer_hint = text("How many beats the 16 slices span (1 bar = 4 beats)")
        .size(sz(12.0));

    // Slicer buffer buttons (1, 4, 8, 16 bars)
    let slicer_sizes: [u32; 4] = [1, 4, 8, 16];
    let slicer_labels = ["4 beats", "16 beats", "32 beats", "64 beats"];
    let slicer_buttons: Vec<Element<Message>> = slicer_sizes
        .iter()
        .zip(slicer_labels.iter())
        .map(|(&size, &label)| {
            let is_selected = state.draft_slicer_buffer_bars == size;
            let btn = button(text(label).size(sz(12.0)))
                .on_press(Message::UpdateSettingsSlicerBufferBars(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(70.0));
            btn.into()
        })
        .collect();

    let slicer_label = text("Buffer:").size(sz(14.0));
    let slicer_row = row![
        slicer_label,
        row(slicer_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Theme selection section
    let theme_title = text("Theme").size(sz(14.0));
    let theme_hint = text("Color scheme for UI and waveform visualization")
        .size(sz(12.0));

    let theme_buttons: Vec<Element<Message>> = state.available_theme_names
        .iter()
        .map(|name| {
            let is_selected = state.draft_theme == *name;
            let btn = button(text(name.as_str()).size(sz(11.0)))
                .on_press(Message::UpdateSettingsTheme(name.clone()))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(80.0));
            btn.into()
        })
        .collect();

    let theme_row = row(theme_buttons).spacing(4).align_y(Alignment::Center);

    // Font section
    let font_title = text("Font").size(sz(14.0));
    let font_hint = text("UI typeface (restart required to apply)")
        .size(sz(12.0));

    let font_buttons: Vec<Element<Message>> = AppFont::ALL
        .iter()
        .map(|&font| {
            let is_selected = state.draft_font == font;
            let btn = button(text(font.display_name()).size(sz(11.0)))
                .on_press(Message::UpdateSettingsFont(font))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Shrink);
            btn.into()
        })
        .collect();

    let font_row = row(font_buttons).spacing(4).align_y(Alignment::Center).wrap();

    // Font size section
    let fsize_title = text("Font Size").size(sz(14.0));
    let fsize_hint = text("Text size preset (restart required to apply)")
        .size(sz(12.0));

    let fsize_buttons: Vec<Element<Message>> = FontSize::ALL
        .iter()
        .map(|&fs| {
            let is_selected = state.draft_font_size == fs;
            let btn = button(text(fs.display_name()).size(sz(11.0)))
                .on_press(Message::UpdateSettingsFontSize(fs))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(70.0));
            btn.into()
        })
        .collect();

    let fsize_row = row(fsize_buttons).spacing(4).align_y(Alignment::Center);

    container(
        column![
            section_title,
            subsection_title,
            hint,
            grid_row,
            Space::new().height(10),
            slicer_title,
            slicer_hint,
            slicer_row,
            Space::new().height(10),
            theme_title,
            theme_hint,
            theme_row,
            Space::new().height(10),
            font_title,
            font_hint,
            font_row,
            Space::new().height(10),
            fsize_title,
            fsize_hint,
            fsize_row,
        ]
        .spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Track name format template settings
fn view_track_name_format_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Import").size(sz(18.0));

    let subsection_title = text("Track Name Format").size(sz(14.0));
    let hint = text("Template for auto-filling track names from stem filenames")
        .size(sz(12.0));

    let format_label = text("Format:").size(sz(14.0));
    let format_input = text_input("{artist} - {name}", &state.draft_track_name_format)
        .on_input(Message::UpdateSettingsTrackNameFormat)
        .width(Length::Fixed(200.0));

    let format_row = row![format_label, format_input]
        .spacing(10)
        .align_y(Alignment::Center);

    let tags_hint = text("Tags: {artist}, {name}")
        .size(sz(12.0));

    // Stem separation backend subsection
    let backend_title = text("Separation Backend").size(sz(14.0));
    let backend_hint = text("Engine used for stem separation (Charon = pure Rust, ORT = ONNX Runtime)")
        .size(sz(12.0));

    // Backend selection buttons - only show available backends
    let backend_options = BackendType::available();
    let backend_buttons: Vec<Element<Message>> = backend_options
        .iter()
        .map(|&backend| {
            let is_selected = state.draft_separation_backend == backend;
            let btn = button(text(backend.display_name()).size(sz(11.0)))
                .on_press(Message::UpdateSettingsSeparationBackend(backend))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(130.0));
            btn.into()
        })
        .collect();

    let backend_label = text("Backend:").size(sz(14.0));
    let backend_row = row![
        backend_label,
        row(backend_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let backend_description = text(state.draft_separation_backend.description())
        .size(sz(12.0));

    // Stem separation model subsection
    let separation_title = text("Stem Separation Model").size(sz(14.0));
    let separation_hint = text("Model used for automatic stem separation during import")
        .size(sz(12.0));

    // Model selection buttons
    let model_options = ModelType::all();
    let model_buttons: Vec<Element<Message>> = model_options
        .iter()
        .map(|&model| {
            let is_selected = state.draft_separation_model == model;
            let btn = button(text(model.display_name()).size(sz(11.0)))
                .on_press(Message::UpdateSettingsSeparationModel(model))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(130.0));
            btn.into()
        })
        .collect();

    let model_label = text("Model:").size(sz(14.0));
    let model_row = row![
        model_label,
        row(model_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let model_description = text(state.draft_separation_model.description())
        .size(sz(12.0));

    // Shift augmentation subsection
    let shifts_title = text("Shift Augmentation").size(sz(14.0));
    let shifts_hint = text("Run model multiple times with random shifts for better quality (slower)")
        .size(sz(12.0));

    // Shifts selection buttons (1-5)
    let shifts_options: [u8; 5] = [1, 2, 3, 4, 5];
    let shifts_buttons: Vec<Element<Message>> = shifts_options
        .iter()
        .map(|&shifts| {
            let is_selected = state.draft_separation_shifts == shifts;
            let label = SeparationConfig::shifts_display_name(shifts);
            let btn = button(text(label).size(sz(11.0)))
                .on_press(Message::UpdateSettingsSeparationShifts(shifts))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(85.0));
            btn.into()
        })
        .collect();

    let shifts_label = text("Quality:").size(sz(14.0));
    let shifts_row = row![
        shifts_label,
        row(shifts_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let shifts_description = text(SeparationConfig::shifts_description(state.draft_separation_shifts))
        .size(sz(12.0));

    container(
        column![
            section_title,
            subsection_title,
            hint,
            format_row,
            tags_hint,
            Space::new().height(10),
            backend_title,
            backend_hint,
            backend_row,
            backend_description,
            Space::new().height(10),
            separation_title,
            separation_hint,
            model_row,
            model_description,
            Space::new().height(10),
            shifts_title,
            shifts_hint,
            shifts_row,
            shifts_description,
        ]
        .spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}
