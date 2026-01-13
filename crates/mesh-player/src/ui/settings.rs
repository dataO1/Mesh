//! Settings modal UI for mesh-player
//!
//! Provides a modal dialog for editing player configuration.

use super::app::Message;
use super::midi_learn::MidiLearnMessage;
use crate::config::{LOOP_LENGTH_OPTIONS, StemColorPalette};
use iced::widget::{button, column, container, row, scrollable, text, toggler, Space};
use iced::{Alignment, Element, Length};

/// Target LUFS presets for loudness normalization
/// Index 0 = loudest (DJ standard), Index 3 = quietest (broadcast safe)
pub const TARGET_LUFS_OPTIONS: [f32; 4] = [-6.0, -9.0, -14.0, -16.0];

/// Get the display name for a LUFS target
fn lufs_preset_name(index: usize) -> &'static str {
    match index {
        0 => "-6 LUFS (Loud)",
        1 => "-9 LUFS (Medium)",
        2 => "-14 LUFS (Streaming)",
        3 => "-16 LUFS (Broadcast)",
        _ => "-6 LUFS (Loud)",
    }
}

/// Find the index of a LUFS value in the presets (or default to 0)
fn lufs_to_index(lufs: f32) -> usize {
    TARGET_LUFS_OPTIONS.iter()
        .position(|&v| (v - lufs).abs() < 0.1)
        .unwrap_or(0)
}

/// Settings state for the modal
#[derive(Debug, Clone)]
pub struct SettingsState {
    /// Whether the settings modal is open
    pub is_open: bool,
    /// Draft loop length index (0-6)
    pub draft_loop_length_index: usize,
    /// Draft zoom bars
    pub draft_zoom_bars: u32,
    /// Draft grid bars
    pub draft_grid_bars: u32,
    /// Draft stem color palette
    pub draft_stem_color_palette: StemColorPalette,
    /// Draft phase sync enabled
    pub draft_phase_sync: bool,
    /// Draft slicer buffer bars (4, 8, or 16)
    pub draft_slicer_buffer_bars: u32,
    /// Draft slicer affected stems [Vocals, Drums, Bass, Other]
    pub draft_slicer_affected_stems: [bool; 4],
    /// Draft auto-gain enabled
    pub draft_auto_gain_enabled: bool,
    /// Draft target LUFS (index into preset values)
    pub draft_target_lufs_index: usize,
    /// Status message (for save feedback)
    pub status: String,
}

impl SettingsState {
    /// Create settings state from current config
    pub fn from_config(config: &crate::config::PlayerConfig) -> Self {
        Self {
            is_open: false,
            draft_loop_length_index: config.display.default_loop_length_index,
            draft_zoom_bars: config.display.default_zoom_bars,
            draft_grid_bars: config.display.grid_bars,
            draft_stem_color_palette: config.display.stem_color_palette,
            draft_phase_sync: config.audio.phase_sync,
            draft_slicer_buffer_bars: config.slicer.default_buffer_bars,
            draft_slicer_affected_stems: config.slicer.affected_stems,
            draft_auto_gain_enabled: config.audio.loudness.auto_gain_enabled,
            draft_target_lufs_index: lufs_to_index(config.audio.loudness.target_lufs),
            status: String::new(),
        }
    }

    /// Create default settings state
    pub fn new() -> Self {
        Self {
            is_open: false,
            draft_loop_length_index: 2, // 4 beats (index 2 in new array)
            draft_zoom_bars: 8,
            draft_grid_bars: 8,
            draft_stem_color_palette: StemColorPalette::default(),
            draft_phase_sync: true, // Enabled by default
            draft_slicer_buffer_bars: 4, // 4 bars = 16 slices
            draft_slicer_affected_stems: [false, true, false, false], // Only Drums by default
            draft_auto_gain_enabled: true, // Auto-gain on by default
            draft_target_lufs_index: 1, // -9 LUFS (balanced)
            status: String::new(),
        }
    }

    /// Get the target LUFS value from the current index
    pub fn target_lufs(&self) -> f32 {
        TARGET_LUFS_OPTIONS.get(self.draft_target_lufs_index)
            .copied()
            .unwrap_or(-9.0)
    }
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}

/// Render the settings modal content
pub fn view(state: &SettingsState) -> Element<'_, Message> {
    let title = text("Settings").size(24);
    let close_btn = button(text("×").size(20))
        .on_press(Message::CloseSettings)
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // Loop length section
    let loop_section = view_loop_section(state);

    // Display settings section
    let display_section = view_display_section(state);

    // Loudness normalization section
    let loudness_section = view_loudness_section(state);

    // Slicer settings section
    let slicer_section = view_slicer_section(state);

    // MIDI settings section
    let midi_section = view_midi_section();

    // Scrollable content area for all sections
    let scrollable_content = scrollable(
        column![loop_section, display_section, loudness_section, slicer_section, midi_section]
            .spacing(15)
            .width(Length::Fill)
    )
    .height(Length::Fill);

    // Status message (for save feedback)
    let status: Element<Message> = if !state.status.is_empty() {
        text(&state.status).size(14).into()
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

    // Layout: fixed header, scrollable middle, fixed footer
    let content = column![header, scrollable_content, status, actions]
        .spacing(15)
        .width(Length::Fixed(450.0))
        .height(Length::Fixed(600.0)); // Max height for modal

    container(content)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// Playback settings (loop length, phase sync)
fn view_loop_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Playback").size(18);

    // Phase sync toggle
    let phase_sync_label = text("Automatic Beat Sync").size(14);
    let phase_sync_hint = text("Automatically align beats when starting playback or hot cues")
        .size(12);
    let phase_sync_toggle = toggler(state.draft_phase_sync)
        .on_toggle(Message::UpdateSettingsPhaseSync);
    let phase_sync_row = row![
        column![phase_sync_label, phase_sync_hint].spacing(4),
        Space::new().width(Length::Fill),
        phase_sync_toggle,
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Loop length section
    let subsection_title = text("Default Loop/Beat Jump Length").size(14);
    let hint = text("Loop length also controls beat jump distance")
        .size(12);

    // Loop length buttons (0.25, 0.5, 1, 2, 4, 8, 16 beats)
    let loop_buttons: Vec<Element<Message>> = LOOP_LENGTH_OPTIONS
        .iter()
        .enumerate()
        .map(|(idx, &beats)| {
            let is_selected = state.draft_loop_length_index == idx;
            let label = if beats < 1.0 {
                format!("{:.2}", beats)
            } else {
                format!("{:.0}", beats)
            };
            let btn = button(text(label).size(11))
                .on_press(Message::UpdateSettingsLoopLength(idx))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(36.0));
            btn.into()
        })
        .collect();

    let loop_label = text("Beats:").size(14);
    let loop_row = row![
        loop_label,
        row(loop_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    container(
        column![
            section_title,
            phase_sync_row,
            Space::new().height(10),
            subsection_title,
            hint,
            loop_row
        ]
        .spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Display settings (waveform zoom and grid)
fn view_display_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Display").size(18);

    // Zoom level section
    let zoom_subsection = text("Default Zoomed Waveform Level").size(14);
    let zoom_hint = text("Number of bars visible in zoomed waveform view")
        .size(12);

    let zoom_sizes: [u32; 6] = [2, 4, 8, 16, 32, 64];
    let zoom_buttons: Vec<Element<Message>> = zoom_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_zoom_bars == size;
            let btn = button(text(format!("{}", size)).size(11))
                .on_press(Message::UpdateSettingsZoomBars(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(36.0));
            btn.into()
        })
        .collect();

    let zoom_label = text("Bars:").size(14);
    let zoom_row = row![
        zoom_label,
        row(zoom_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Grid density section
    let grid_subsection = text("Overview Grid Density").size(14);
    let grid_hint = text("Beat grid line spacing on the overview waveform")
        .size(12);

    let grid_sizes: [u32; 4] = [4, 8, 16, 32];
    let grid_buttons: Vec<Element<Message>> = grid_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_grid_bars == size;
            let btn = button(text(format!("{}", size)).size(11))
                .on_press(Message::UpdateSettingsGridBars(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(36.0));
            btn.into()
        })
        .collect();

    let grid_label = text("Bars:").size(14);
    let grid_row = row![
        grid_label,
        row(grid_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Stem color palette section
    let palette_subsection = text("Stem Color Palette").size(14);
    let palette_hint = text("Color scheme for waveform stem visualization")
        .size(12);

    let palette_buttons: Vec<Element<Message>> = StemColorPalette::ALL
        .iter()
        .map(|&palette| {
            let is_selected = state.draft_stem_color_palette == palette;
            let btn = button(text(palette.display_name()).size(11))
                .on_press(Message::UpdateSettingsStemColorPalette(palette))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(75.0));
            btn.into()
        })
        .collect();

    let palette_row = row(palette_buttons).spacing(4).align_y(Alignment::Center);

    container(
        column![
            section_title,
            zoom_subsection,
            zoom_hint,
            zoom_row,
            Space::new().height(10),
            grid_subsection,
            grid_hint,
            grid_row,
            Space::new().height(10),
            palette_subsection,
            palette_hint,
            palette_row,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Loudness normalization settings (auto-gain, target LUFS)
fn view_loudness_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Loudness").size(18);

    // Auto-gain toggle
    let auto_gain_label = text("Auto-Gain Normalization").size(14);
    let auto_gain_hint = text("Automatically adjust track volume to match target loudness")
        .size(12);
    let auto_gain_toggle = toggler(state.draft_auto_gain_enabled)
        .on_toggle(Message::UpdateSettingsAutoGainEnabled);
    let auto_gain_row = row![
        column![auto_gain_label, auto_gain_hint].spacing(4),
        Space::new().width(Length::Fill),
        auto_gain_toggle,
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Target LUFS section
    let target_subsection = text("Target Loudness").size(14);
    let target_hint = text("Tracks will be gain-compensated to reach this level")
        .size(12);

    let target_buttons: Vec<Element<Message>> = TARGET_LUFS_OPTIONS
        .iter()
        .enumerate()
        .map(|(idx, &lufs)| {
            let is_selected = state.draft_target_lufs_index == idx;
            let label = format!("{:.0}", lufs);
            let btn = button(text(label).size(11))
                .on_press(Message::UpdateSettingsTargetLufs(idx))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(50.0));
            btn.into()
        })
        .collect();

    let target_label = text("LUFS:").size(14);
    let target_row = row![
        target_label,
        row(target_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Current preset description
    let preset_desc = text(lufs_preset_name(state.draft_target_lufs_index)).size(12);

    container(
        column![
            section_title,
            auto_gain_row,
            Space::new().height(10),
            target_subsection,
            target_hint,
            target_row,
            preset_desc,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Slicer settings (buffer size, queue algorithm)
fn view_slicer_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Slicer").size(18);

    // Buffer bars section
    let buffer_subsection = text("Buffer Size").size(14);
    let buffer_hint = text("Size of the slicer buffer window (always 8 slices)")
        .size(12);

    let buffer_sizes: [u32; 4] = [1, 4, 8, 16];
    let buffer_buttons: Vec<Element<Message>> = buffer_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_slicer_buffer_bars == size;
            let btn = button(text(format!("{}", size)).size(11))
                .on_press(Message::UpdateSettingsSlicerBufferBars(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(44.0));
            btn.into()
        })
        .collect();

    let buffer_label = text("Bars:").size(14);
    let buffer_row = row![
        buffer_label,
        row(buffer_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Affected stems section
    let stems_subsection = text("Affected Stems").size(14);
    let stems_hint = text("Which stems are processed by the slicer")
        .size(12);

    let stem_names = ["Vocals", "Drums", "Bass", "Other"];
    let stems_buttons: Vec<Element<Message>> = stem_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            let is_selected = state.draft_slicer_affected_stems[idx];
            let btn = button(text(*name).size(11))
                .on_press(Message::UpdateSettingsSlicerAffectedStem(idx, !is_selected))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(60.0));
            btn.into()
        })
        .collect();

    let stems_label = text("Stems:").size(14);
    let stems_row = row![
        stems_label,
        row(stems_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    container(
        column![
            section_title,
            buffer_subsection,
            buffer_hint,
            buffer_row,
            Space::new().height(10),
            stems_subsection,
            stems_hint,
            stems_row,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// MIDI settings section (learn button)
fn view_midi_section() -> Element<'static, Message> {
    let section_title = text("MIDI Controller").size(18);

    let hint = text("Create a custom mapping for your MIDI controller")
        .size(12);

    let learn_btn = button(text("Start MIDI Learn").size(14))
        .on_press(Message::MidiLearn(MidiLearnMessage::Start))
        .style(button::primary);

    container(
        column![
            section_title,
            hint,
            Space::new().height(5),
            learn_btn,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}
