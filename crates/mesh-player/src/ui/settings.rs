//! Settings modal UI for mesh-player
//!
//! Provides a modal dialog for editing player configuration.

use super::message::{Message, SettingsMessage};
use super::midi_learn::MidiLearnMessage;
use super::network::NetworkState;
use super::system_update::UpdateState;
use crate::audio::{get_available_stereo_pairs, StereoPair};
use crate::config::{AppFont, FontSize, LOOP_LENGTH_OPTIONS, KeyScoringModel, WaveformAbstraction, WaveformLayout};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, toggler, Id, Space};
use iced::{Alignment, Color, Element, Length};
use mesh_widgets::sz;
use std::sync::LazyLock;

/// Scrollable ID for the settings content area (used for programmatic scrolling via MIDI nav)
pub static SETTINGS_SCROLLABLE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("mesh_settings_scrollable"));

/// Calculate the total number of navigable settings entries.
/// Dynamic because Network and Update sections are optional.
pub fn settings_entry_count(state: &SettingsState) -> usize {
    let mut count = 16; // Base entries from build_settings_entries
    if state.network.is_some() {
        count += 1; // Network section
    }
    if state.update.is_some() {
        count += 1; // System update section
    }
    count += 1; // MIDI Learn (always present, always last)
    count
}

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
    /// Draft theme name (from theme.yaml)
    pub draft_theme: String,
    /// Available theme names (populated when settings open)
    pub available_theme_names: Vec<String>,
    /// Draft phase sync enabled
    pub draft_phase_sync: bool,
    /// Draft slicer buffer bars (1, 4, 8, or 16)
    pub draft_slicer_buffer_bars: u32,
    /// Draft auto-gain enabled
    pub draft_auto_gain_enabled: bool,
    /// Draft target LUFS (index into preset values)
    pub draft_target_lufs_index: usize,
    /// Draft show local collection in browser
    pub draft_show_local_collection: bool,
    /// Draft key scoring model for harmonic compatibility
    pub draft_key_scoring_model: KeyScoringModel,
    /// Draft waveform layout orientation
    pub draft_waveform_layout: WaveformLayout,
    /// Draft waveform abstraction level
    pub draft_waveform_abstraction: WaveformAbstraction,
    /// Draft UI font (requires restart to apply)
    pub draft_font: AppFont,
    /// Draft font size preset
    pub draft_font_size: FontSize,
    /// Draft master device index (for audio routing)
    pub draft_master_device: usize,
    /// Draft cue device index (for audio routing)
    pub draft_cue_device: usize,
    /// Available audio output devices
    pub available_devices: Vec<StereoPair>,
    /// Status message (for save feedback)
    pub status: String,
    /// MIDI navigation state (Some when opened via MIDI, None when opened via mouse)
    pub settings_midi_nav: Option<SettingsMidiNav>,
    /// Network management state (None if nmcli not available)
    pub network: Option<NetworkState>,
    /// System update state (None if not on NixOS)
    pub update: Option<UpdateState>,
    /// Snapshot of values at open time (for dirty detection)
    initial_snapshot: Option<SettingsSnapshot>,
}

impl SettingsState {
    /// Create settings state from current config
    pub fn from_config(config: &crate::config::PlayerConfig) -> Self {
        // Query available audio devices
        let available_devices = get_available_stereo_pairs();
        let num_devices = available_devices.len();

        Self {
            is_open: false,
            draft_loop_length_index: config.display.default_loop_length_index,
            draft_zoom_bars: config.display.default_zoom_bars,
            draft_grid_bars: config.display.grid_bars,
            draft_theme: config.display.theme.clone(),
            available_theme_names: Vec::new(), // Populated by caller
            draft_phase_sync: config.audio.phase_sync,
            draft_slicer_buffer_bars: config.slicer.buffer_bars,
            draft_auto_gain_enabled: config.audio.loudness.auto_gain_enabled,
            draft_target_lufs_index: lufs_to_index(config.audio.loudness.target_lufs),
            draft_show_local_collection: config.display.show_local_collection,
            draft_key_scoring_model: config.display.key_scoring_model,
            draft_waveform_layout: config.display.waveform_layout,
            draft_waveform_abstraction: config.display.waveform_abstraction,
            draft_font: config.display.font,
            draft_font_size: config.display.font_size,
            draft_master_device: config.audio.outputs.master_device.unwrap_or(0),
            draft_cue_device: config.audio.outputs.cue_device.unwrap_or_else(|| {
                if num_devices >= 2 { 1 } else { 0 }
            }),
            available_devices,
            status: String::new(),
            settings_midi_nav: None,
            network: super::handlers::network::init_network_state(),
            update: super::handlers::system_update::init_update_state(),
            initial_snapshot: None,
        }
    }

    /// Create default settings state
    pub fn new() -> Self {
        let available_devices = get_available_stereo_pairs();
        let num_devices = available_devices.len();

        Self {
            is_open: false,
            draft_loop_length_index: 2, // 4 beats (index 2 in new array)
            draft_zoom_bars: 8,
            draft_grid_bars: 32,
            draft_theme: "Mesh".to_string(),
            available_theme_names: Vec::new(),
            draft_phase_sync: true, // Enabled by default
            draft_slicer_buffer_bars: 1, // 1 bar = 4 beats (default)
            draft_auto_gain_enabled: true, // Auto-gain on by default
            draft_target_lufs_index: 1, // -9 LUFS (balanced)
            draft_show_local_collection: false, // USB-only by default
            draft_key_scoring_model: KeyScoringModel::default(),
            draft_waveform_layout: WaveformLayout::default(),
            draft_waveform_abstraction: WaveformAbstraction::default(),
            draft_font: AppFont::default(),
            draft_font_size: FontSize::default(),
            draft_master_device: 0, // First device
            draft_cue_device: if num_devices >= 2 { 1 } else { 0 }, // Second device or fallback
            available_devices,
            status: String::new(),
            settings_midi_nav: None,
            network: super::handlers::network::init_network_state(),
            update: super::handlers::system_update::init_update_state(),
            initial_snapshot: None,
        }
    }

    /// Refresh available audio devices
    pub fn refresh_audio_devices(&mut self) {
        self.available_devices = get_available_stereo_pairs();
        // Clamp selections to valid range
        let max_idx = self.available_devices.len().saturating_sub(1);
        self.draft_master_device = self.draft_master_device.min(max_idx);
        self.draft_cue_device = self.draft_cue_device.min(max_idx);
    }

    /// Get the target LUFS value from the current index
    pub fn target_lufs(&self) -> f32 {
        TARGET_LUFS_OPTIONS.get(self.draft_target_lufs_index)
            .copied()
            .unwrap_or(-9.0)
    }

    /// Take a snapshot of current draft values for dirty detection
    pub fn take_snapshot(&mut self) {
        self.initial_snapshot = Some(SettingsSnapshot {
            loop_length_index: self.draft_loop_length_index,
            zoom_bars: self.draft_zoom_bars,
            grid_bars: self.draft_grid_bars,
            theme: self.draft_theme.clone(),
            phase_sync: self.draft_phase_sync,
            slicer_buffer_bars: self.draft_slicer_buffer_bars,
            auto_gain_enabled: self.draft_auto_gain_enabled,
            target_lufs_index: self.draft_target_lufs_index,
            show_local_collection: self.draft_show_local_collection,
            key_scoring_model: self.draft_key_scoring_model,
            waveform_layout: self.draft_waveform_layout,
            waveform_abstraction: self.draft_waveform_abstraction,
            font: self.draft_font,
            font_size: self.draft_font_size,
            master_device: self.draft_master_device,
            cue_device: self.draft_cue_device,
        });
    }

    /// Check if any draft values differ from the snapshot taken at open time
    pub fn has_changes(&self) -> bool {
        match &self.initial_snapshot {
            None => false,
            Some(snap) => {
                snap.loop_length_index != self.draft_loop_length_index
                    || snap.zoom_bars != self.draft_zoom_bars
                    || snap.grid_bars != self.draft_grid_bars
                    || snap.theme != self.draft_theme
                    || snap.phase_sync != self.draft_phase_sync
                    || snap.slicer_buffer_bars != self.draft_slicer_buffer_bars
                    || snap.auto_gain_enabled != self.draft_auto_gain_enabled
                    || snap.target_lufs_index != self.draft_target_lufs_index
                    || snap.show_local_collection != self.draft_show_local_collection
                    || snap.key_scoring_model != self.draft_key_scoring_model
                    || snap.waveform_layout != self.draft_waveform_layout
                    || snap.waveform_abstraction != self.draft_waveform_abstraction
                    || snap.font != self.draft_font
                    || snap.font_size != self.draft_font_size
                    || snap.master_device != self.draft_master_device
                    || snap.cue_device != self.draft_cue_device
            }
        }
    }
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Snapshot for dirty detection ──

/// Captures all draft values at open time so we can detect changes
#[derive(Debug, Clone, PartialEq)]
struct SettingsSnapshot {
    loop_length_index: usize,
    zoom_bars: u32,
    grid_bars: u32,
    theme: String,
    phase_sync: bool,
    slicer_buffer_bars: u32,
    auto_gain_enabled: bool,
    target_lufs_index: usize,
    show_local_collection: bool,
    key_scoring_model: KeyScoringModel,
    waveform_layout: WaveformLayout,
    waveform_abstraction: WaveformAbstraction,
    font: AppFont,
    font_size: FontSize,
    master_device: usize,
    cue_device: usize,
}

// ── MIDI Navigation State ──

/// Sub-panel focus state for domain-specific MIDI navigation within settings.
/// When active, encoder scroll/select operate on the sub-panel instead of the settings list.
#[derive(Debug, Clone)]
pub enum SubPanelFocus {
    /// Navigating the WiFi network list (encoder cycles networks, press connects)
    WifiNetworkList { selected: usize },
    /// Navigating update actions (encoder cycles actions, press activates)
    UpdateActions { selected: usize },
}

/// State for MIDI-driven settings navigation
#[derive(Debug, Clone)]
pub struct SettingsMidiNav {
    /// Currently focused setting index in the flat list
    pub focused_index: usize,
    /// Whether we're editing the focused setting's value (vs browsing the list)
    pub editing: bool,
    /// Browse mode state for each side before settings opened (to restore on close)
    pub saved_browse_state: [bool; 2],
    /// Active sub-panel (overrides normal navigation when Some)
    pub sub_panel: Option<SubPanelFocus>,
}

impl SettingsMidiNav {
    pub fn new(saved_browse_state: [bool; 2]) -> Self {
        Self {
            focused_index: 0,
            editing: false,
            saved_browse_state,
            sub_panel: None,
        }
    }
}

// ── Data-Driven Settings Registry ──

/// A navigable setting entry — pure data describing one setting and its options.
/// The navigation system only sees Vec<SettingsEntry> and indices.
pub struct SettingsEntry {
    /// Display label for this setting
    pub label: &'static str,
    /// Display labels for each selectable option
    pub options: Vec<String>,
    /// Currently selected option index
    pub selected: usize,
    /// Message factory: given an option index, returns the SettingsMessage to apply it
    pub on_select: fn(usize) -> SettingsMessage,
}

/// Format a beat count for display (fraction notation for sub-beat values)
fn format_beats(beats: f64) -> String {
    if (beats - 0.125).abs() < 0.001 { "1/8".into() }
    else if (beats - 0.25).abs() < 0.001 { "1/4".into() }
    else if (beats - 0.5).abs() < 0.001 { "1/2".into() }
    else { format!("{:.0}", beats) }
}

/// Zoom bar options used in both the registry and view
pub const ZOOM_SIZES: [u32; 6] = [2, 4, 8, 16, 32, 64];
/// Grid density options in beats — used in both the registry and view
pub const GRID_SIZES: [u32; 4] = [8, 16, 32, 64];
/// Slicer buffer bar options used in both the registry and view
pub const BUFFER_SIZES: [u32; 4] = [1, 4, 8, 16];

/// Build the flat, ordered list of all navigable settings from current state.
///
/// This is the SINGLE source of truth for which settings exist, their options,
/// and current values. When restructuring the settings UI, update this function
/// and the view layout — navigation logic stays untouched.
pub fn build_settings_entries(state: &SettingsState) -> Vec<SettingsEntry> {
    vec![
        SettingsEntry {
            label: "Master Device",
            options: state.available_devices.iter().map(|d| d.to_string()).collect(),
            selected: state.draft_master_device,
            on_select: |idx| SettingsMessage::UpdateMasterPair(idx),
        },
        SettingsEntry {
            label: "Cue Device",
            options: state.available_devices.iter().map(|d| d.to_string()).collect(),
            selected: state.draft_cue_device,
            on_select: |idx| SettingsMessage::UpdateCuePair(idx),
        },
        SettingsEntry {
            label: "Automatic Beat Sync",
            options: vec!["On".into(), "Off".into()],
            selected: if state.draft_phase_sync { 0 } else { 1 },
            on_select: |idx| SettingsMessage::UpdatePhaseSync(idx == 0),
        },
        SettingsEntry {
            label: "Loop/Beat Jump Length",
            options: LOOP_LENGTH_OPTIONS.iter().map(|&b| format_beats(b)).collect(),
            selected: state.draft_loop_length_index,
            on_select: |idx| SettingsMessage::UpdateLoopLength(idx),
        },
        SettingsEntry {
            label: "Waveform Layout",
            options: WaveformLayout::ALL.iter().map(|l| l.display_name().to_string()).collect(),
            selected: WaveformLayout::ALL.iter().position(|&l| l == state.draft_waveform_layout).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateWaveformLayout(WaveformLayout::ALL[idx.min(WaveformLayout::ALL.len() - 1)]),
        },
        SettingsEntry {
            label: "Waveform Abstraction",
            options: WaveformAbstraction::ALL.iter().map(|a| a.display_name().to_string()).collect(),
            selected: WaveformAbstraction::ALL.iter().position(|&a| a == state.draft_waveform_abstraction).unwrap_or(1),
            on_select: |idx| SettingsMessage::UpdateWaveformAbstraction(WaveformAbstraction::ALL[idx.min(WaveformAbstraction::ALL.len() - 1)]),
        },
        SettingsEntry {
            label: "Zoomed Waveform Level",
            options: ZOOM_SIZES.iter().map(|s| format!("{} bars", s)).collect(),
            selected: ZOOM_SIZES.iter().position(|&s| s == state.draft_zoom_bars).unwrap_or(2),
            on_select: |idx| SettingsMessage::UpdateZoomBars(ZOOM_SIZES[idx.min(ZOOM_SIZES.len() - 1)]),
        },
        SettingsEntry {
            label: "Overview Grid Density",
            options: GRID_SIZES.iter().map(|s| format!("{} beats", s)).collect(),
            selected: GRID_SIZES.iter().position(|&s| s == state.draft_grid_bars).unwrap_or(1),
            on_select: |idx| SettingsMessage::UpdateGridBars(GRID_SIZES[idx.min(GRID_SIZES.len() - 1)]),
        },
        SettingsEntry {
            label: "Theme",
            options: state.available_theme_names.clone(),
            selected: state.available_theme_names.iter().position(|n| *n == state.draft_theme).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateThemeIndex(idx),
        },
        SettingsEntry {
            label: "Font",
            options: AppFont::ALL.iter().map(|f| f.display_name().to_string()).collect(),
            selected: AppFont::ALL.iter().position(|&f| f == state.draft_font).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateFont(AppFont::ALL[idx.min(AppFont::ALL.len() - 1)]),
        },
        SettingsEntry {
            label: "Font Size",
            options: FontSize::ALL.iter().map(|f| f.display_name().to_string()).collect(),
            selected: FontSize::ALL.iter().position(|&f| f == state.draft_font_size).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateFontSize(FontSize::ALL[idx.min(FontSize::ALL.len() - 1)]),
        },
        SettingsEntry {
            label: "Show Local Collection",
            options: vec!["On".into(), "Off".into()],
            selected: if state.draft_show_local_collection { 0 } else { 1 },
            on_select: |idx| SettingsMessage::UpdateShowLocalCollection(idx == 0),
        },
        SettingsEntry {
            label: "Key Matching",
            options: KeyScoringModel::ALL.iter().map(|m| m.display_name().to_string()).collect(),
            selected: KeyScoringModel::ALL.iter().position(|&m| m == state.draft_key_scoring_model).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateKeyScoringModel(KeyScoringModel::ALL[idx.min(KeyScoringModel::ALL.len() - 1)]),
        },
        SettingsEntry {
            label: "Auto-Gain",
            options: vec!["On".into(), "Off".into()],
            selected: if state.draft_auto_gain_enabled { 0 } else { 1 },
            on_select: |idx| SettingsMessage::UpdateAutoGainEnabled(idx == 0),
        },
        SettingsEntry {
            label: "Target Loudness",
            options: TARGET_LUFS_OPTIONS.iter().map(|&l| format!("{:.0} LUFS", l)).collect(),
            selected: state.draft_target_lufs_index,
            on_select: |idx| SettingsMessage::UpdateTargetLufs(idx),
        },
        SettingsEntry {
            label: "Slicer Buffer",
            options: BUFFER_SIZES.iter().map(|s| format!("{} bars", s)).collect(),
            selected: BUFFER_SIZES.iter().position(|&s| s == state.draft_slicer_buffer_bars).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateSlicerBufferBars(BUFFER_SIZES[idx.min(BUFFER_SIZES.len() - 1)]),
        },
    ]
}

// ── Visual Highlighting ──

/// Wrap a setting row with visual highlighting when focused via MIDI navigation.
/// Always applies the same padding so layout doesn't shift when focus changes.
pub fn wrap_navigable<'a>(
    content: Element<'a, Message>,
    setting_index: usize,
    nav: Option<&SettingsMidiNav>,
) -> Element<'a, Message> {
    let is_focused = nav.is_some_and(|n| n.focused_index == setting_index);
    let is_editing = nav.is_some_and(|n| n.focused_index == setting_index && n.editing);

    let bg_color = if is_editing {
        Color::from_rgba(0.3, 0.6, 1.0, 0.2)
    } else if is_focused {
        Color::from_rgba(0.2, 0.4, 0.8, 0.12)
    } else {
        Color::TRANSPARENT
    };

    container(content)
        .style(move |_theme| container::Style {
            background: Some(bg_color.into()),
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .padding(4)
        .width(Length::Fill)
        .into()
}

/// Render the settings modal content
pub fn view(state: &SettingsState) -> Element<'_, Message> {
    let title = text("Settings").size(sz(24.0));
    let close_btn = button(text("×").size(sz(20.0)))
        .on_press(Message::Settings(SettingsMessage::Close))
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let nav = state.settings_midi_nav.as_ref();

    // Audio output section (at top - important for DJ workflow)
    let audio_output_section = view_audio_output_section(state, nav);

    // Loop length section
    let loop_section = view_loop_section(state, nav);

    // Display settings section
    let display_section = view_display_section(state, nav);

    // Loudness normalization section
    let loudness_section = view_loudness_section(state, nav);

    // Slicer settings section
    let slicer_section = view_slicer_section(state, nav);

    // Dynamic entry indices for network/update (base entries = indices 0..15)
    let mut next_idx = 16usize;

    // Network settings section (only when nmcli available)
    let network_section: Option<Element<'_, Message>> = state.network.as_ref().map(|ns| {
        let idx = next_idx;
        next_idx += 1;
        wrap_navigable(super::network::view_network_section(ns), idx, nav)
    });

    // System update section (only on NixOS)
    // Extract focused action from sub-panel for highlighting
    let update_focused_action = nav.and_then(|n| {
        if let Some(SubPanelFocus::UpdateActions { selected }) = &n.sub_panel {
            Some(*selected)
        } else {
            None
        }
    });
    let update_section: Option<Element<'_, Message>> = state.update.as_ref().map(|us| {
        let idx = next_idx;
        next_idx += 1;
        wrap_navigable(super::system_update::view_update_section(us, update_focused_action), idx, nav)
    });

    // MIDI settings section (always last navigable entry)
    let midi_section = wrap_navigable(view_midi_section(), next_idx, nav);

    // Scrollable content area for all sections (audio output first)
    let mut sections: Vec<Element<'_, Message>> = vec![
        audio_output_section, loop_section, display_section,
        loudness_section, slicer_section,
    ];
    if let Some(ns) = network_section {
        sections.push(ns);
    }
    if let Some(us) = update_section {
        sections.push(us);
    }
    sections.push(midi_section);

    let scrollable_content = scrollable(
        column(sections)
            .spacing(15)
            .width(Length::Fill)
    )
    .id(SETTINGS_SCROLLABLE_ID.clone())
    .height(Length::Fill);

    // Status message (for save feedback)
    let status: Element<Message> = if !state.status.is_empty() {
        text(&state.status).size(sz(14.0)).into()
    } else {
        Space::new().height(20).into()
    };

    // Action buttons
    let cancel_btn = button(text("Cancel"))
        .on_press(Message::Settings(SettingsMessage::Close))
        .style(button::secondary);

    let save_btn = button(text("Save"))
        .on_press(Message::Settings(SettingsMessage::Save))
        .style(button::primary);

    let actions = row![Space::new().width(Length::Fill), cancel_btn, save_btn]
        .spacing(10)
        .width(Length::Fill);

    // Layout: fixed header, scrollable middle, fixed footer
    let content = column![header, scrollable_content, status, actions]
        .spacing(15)
        .width(Length::Fixed(550.0))  // Wider to fit long device names
        .height(Length::Fixed(600.0)); // Max height for modal

    container(content)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// Playback settings (loop length, phase sync)
fn view_loop_section<'a>(state: &'a SettingsState, nav: Option<&SettingsMidiNav>) -> Element<'a, Message> {
    let section_title = text("Playback").size(sz(18.0));

    // Phase sync toggle
    let phase_sync_label = text("Automatic Beat Sync").size(sz(14.0));
    let phase_sync_hint = text("Automatically align beats when starting playback or hot cues")
        .size(sz(12.0));
    let phase_sync_toggle = toggler(state.draft_phase_sync)
        .on_toggle(|v| Message::Settings(SettingsMessage::UpdatePhaseSync(v)));
    let phase_sync_row = row![
        column![phase_sync_label, phase_sync_hint].spacing(4),
        Space::new().width(Length::Fill),
        phase_sync_toggle,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let phase_sync_row = wrap_navigable(phase_sync_row.into(), 2, nav);

    // Loop length section
    let subsection_title = text("Default Loop/Beat Jump Length").size(sz(14.0));
    let hint = text("Loop length also controls beat jump distance")
        .size(sz(12.0));

    // Loop length buttons (1/8 beat to 256 beats)
    let loop_buttons: Vec<Element<Message>> = LOOP_LENGTH_OPTIONS
        .iter()
        .enumerate()
        .map(|(idx, &beats)| {
            let is_selected = state.draft_loop_length_index == idx;
            let label = format_beats(beats);
            let btn = button(text(label).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateLoopLength(idx)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(36.0));
            btn.into()
        })
        .collect();

    let loop_label = text("Beats:").size(sz(14.0));
    let loop_row = row![
        loop_label,
        row(loop_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let loop_row = wrap_navigable(loop_row.into(), 3, nav);

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
fn view_display_section<'a>(state: &'a SettingsState, nav: Option<&SettingsMidiNav>) -> Element<'a, Message> {
    let section_title = text("Display").size(sz(18.0));

    // Waveform layout section
    let layout_subsection = text("Waveform Layout").size(sz(14.0));
    let layout_hint = text("Orientation of waveform display")
        .size(sz(12.0));

    let layout_buttons: Vec<Element<Message>> = WaveformLayout::ALL
        .iter()
        .map(|&layout| {
            let is_selected = state.draft_waveform_layout == layout;
            let btn = button(text(layout.display_name()).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateWaveformLayout(layout)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(100.0));
            btn.into()
        })
        .collect();

    let layout_row = row(layout_buttons).spacing(4).align_y(Alignment::Center);
    let layout_row = wrap_navigable(layout_row.into(), 4, nav);

    // Waveform abstraction level section
    let abstraction_subsection = text("Waveform Abstraction").size(sz(14.0));
    let abstraction_hint = text("Grid-aligned subsampling intensity (Low = detailed, High = smooth)")
        .size(sz(12.0));

    let abstraction_buttons: Vec<Element<Message>> = WaveformAbstraction::ALL
        .iter()
        .map(|&level| {
            let is_selected = state.draft_waveform_abstraction == level;
            let btn = button(text(level.display_name()).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateWaveformAbstraction(level)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(70.0));
            btn.into()
        })
        .collect();

    let abstraction_row = row(abstraction_buttons).spacing(4).align_y(Alignment::Center);
    let abstraction_row = wrap_navigable(abstraction_row.into(), 5, nav);

    // Zoom level section
    let zoom_subsection = text("Default Zoomed Waveform Level").size(sz(14.0));
    let zoom_hint = text("Number of bars visible in zoomed waveform view")
        .size(sz(12.0));

    let zoom_sizes: [u32; 6] = [2, 4, 8, 16, 32, 64];
    let zoom_buttons: Vec<Element<Message>> = zoom_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_zoom_bars == size;
            let btn = button(text(format!("{}", size)).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateZoomBars(size)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(36.0));
            btn.into()
        })
        .collect();

    let zoom_label = text("Bars:").size(sz(14.0));
    let zoom_row = row![
        zoom_label,
        row(zoom_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let zoom_row = wrap_navigable(zoom_row.into(), 6, nav);

    // Grid density section
    let grid_subsection = text("Overview Grid Density").size(sz(14.0));
    let grid_hint = text("Beat grid line spacing on the overview waveform")
        .size(sz(12.0));

    let grid_sizes: [u32; 4] = [8, 16, 32, 64];
    let grid_buttons: Vec<Element<Message>> = grid_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_grid_bars == size;
            let btn = button(text(format!("{}", size)).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateGridBars(size)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(36.0));
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
    let grid_row = wrap_navigable(grid_row.into(), 7, nav);

    // Theme section
    let palette_subsection = text("Theme").size(sz(14.0));
    let palette_hint = text("Color scheme for UI and waveform visualization")
        .size(sz(12.0));

    let palette_buttons: Vec<Element<Message>> = state.available_theme_names
        .iter()
        .map(|name| {
            let is_selected = state.draft_theme == *name;
            let btn = button(text(name.as_str()).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateTheme(name.clone())))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(80.0));
            btn.into()
        })
        .collect();

    let palette_row = row(palette_buttons).spacing(4).align_y(Alignment::Center);
    let palette_row = wrap_navigable(palette_row.into(), 8, nav);

    // Font section (right after Theme)
    let font_subsection = text("Font").size(sz(14.0));
    let font_hint = text("UI typeface (restart required to apply)")
        .size(sz(12.0));

    let font_buttons: Vec<Element<Message>> = AppFont::ALL
        .iter()
        .map(|&font| {
            let is_selected = state.draft_font == font;
            let btn = button(text(font.display_name()).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateFont(font)))
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
    let font_row = wrap_navigable(font_row.into(), 9, nav);

    // Font size section
    let size_subsection = text("Font Size").size(sz(14.0));
    let size_hint = text("Text size preset (restart required to apply)")
        .size(sz(12.0));

    let size_buttons: Vec<Element<Message>> = FontSize::ALL
        .iter()
        .map(|&fs| {
            let is_selected = state.draft_font_size == fs;
            let btn = button(text(fs.display_name()).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateFontSize(fs)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(70.0));
            btn.into()
        })
        .collect();

    let size_row = row(size_buttons).spacing(4).align_y(Alignment::Center);
    let size_row = wrap_navigable(size_row.into(), 10, nav);

    // Browser settings
    let browser_subsection = text("Browser").size(sz(14.0));
    let local_collection_label = text("Show Local Collection").size(sz(14.0));
    let local_collection_hint = text("Display local music library alongside USB devices")
        .size(sz(12.0));
    let local_collection_toggle = toggler(state.draft_show_local_collection)
        .on_toggle(|v| Message::Settings(SettingsMessage::UpdateShowLocalCollection(v)));
    let local_collection_row = row![
        column![local_collection_label, local_collection_hint].spacing(4),
        Space::new().width(Length::Fill),
        local_collection_toggle,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let local_collection_row = wrap_navigable(local_collection_row.into(), 11, nav);

    // Key scoring model section
    let key_model_subsection = text("Key Matching").size(sz(14.0));
    let key_model_hint = text("Algorithm for harmonic compatibility scoring")
        .size(sz(12.0));

    let model_buttons: Vec<Element<Message>> = KeyScoringModel::ALL
        .iter()
        .map(|&model| {
            let is_selected = state.draft_key_scoring_model == model;
            let btn = button(text(model.display_name()).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateKeyScoringModel(model)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(85.0));
            btn.into()
        })
        .collect();

    let model_row = row(model_buttons).spacing(4).align_y(Alignment::Center);
    let model_row = wrap_navigable(model_row.into(), 12, nav);

    container(
        column![
            section_title,
            layout_subsection,
            layout_hint,
            layout_row,
            Space::new().height(10),
            abstraction_subsection,
            abstraction_hint,
            abstraction_row,
            Space::new().height(10),
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
            Space::new().height(10),
            font_subsection,
            font_hint,
            font_row,
            Space::new().height(10),
            size_subsection,
            size_hint,
            size_row,
            Space::new().height(10),
            browser_subsection,
            local_collection_row,
            Space::new().height(10),
            key_model_subsection,
            key_model_hint,
            model_row,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Loudness normalization settings (auto-gain, target LUFS)
fn view_loudness_section<'a>(state: &'a SettingsState, nav: Option<&SettingsMidiNav>) -> Element<'a, Message> {
    let section_title = text("Loudness").size(sz(18.0));

    // Auto-gain toggle
    let auto_gain_label = text("Auto-Gain Normalization").size(sz(14.0));
    let auto_gain_hint = text("Automatically adjust track volume to match target loudness")
        .size(sz(12.0));
    let auto_gain_toggle = toggler(state.draft_auto_gain_enabled)
        .on_toggle(|v| Message::Settings(SettingsMessage::UpdateAutoGainEnabled(v)));
    let auto_gain_row = row![
        column![auto_gain_label, auto_gain_hint].spacing(4),
        Space::new().width(Length::Fill),
        auto_gain_toggle,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let auto_gain_row = wrap_navigable(auto_gain_row.into(), 13, nav);

    // Target LUFS section
    let target_subsection = text("Target Loudness").size(sz(14.0));
    let target_hint = text("Tracks will be gain-compensated to reach this level")
        .size(sz(12.0));

    let target_buttons: Vec<Element<Message>> = TARGET_LUFS_OPTIONS
        .iter()
        .enumerate()
        .map(|(idx, &lufs)| {
            let is_selected = state.draft_target_lufs_index == idx;
            let label = format!("{:.0}", lufs);
            let btn = button(text(label).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateTargetLufs(idx)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(50.0));
            btn.into()
        })
        .collect();

    let target_label = text("LUFS:").size(sz(14.0));
    let target_row = row![
        target_label,
        row(target_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let target_row = wrap_navigable(target_row.into(), 14, nav);

    // Current preset description
    let preset_desc = text(lufs_preset_name(state.draft_target_lufs_index)).size(sz(12.0));

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

/// Slicer settings (buffer size)
fn view_slicer_section<'a>(state: &'a SettingsState, nav: Option<&SettingsMidiNav>) -> Element<'a, Message> {
    let section_title = text("Slicer").size(sz(18.0));

    // Buffer bars section
    let buffer_subsection = text("Buffer Size").size(sz(14.0));
    let buffer_hint = text("Size of the slicer buffer window (16 slices)")
        .size(sz(12.0));

    let buffer_sizes: [u32; 4] = [1, 4, 8, 16];
    let buffer_buttons: Vec<Element<Message>> = buffer_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_slicer_buffer_bars == size;
            let btn = button(text(format!("{}", size)).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateSlicerBufferBars(size)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(44.0));
            btn.into()
        })
        .collect();

    let buffer_label = text("Bars:").size(sz(14.0));
    let buffer_row = row![
        buffer_label,
        row(buffer_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let buffer_row = wrap_navigable(buffer_row.into(), 15, nav);

    // Note about preset editing
    let preset_hint = text("Edit slicer presets and per-stem patterns in mesh-cue")
        .size(sz(12.0));

    container(
        column![
            section_title,
            buffer_subsection,
            buffer_hint,
            buffer_row,
            Space::new().height(5),
            preset_hint,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// MIDI settings section (learn button)
fn view_midi_section() -> Element<'static, Message> {
    let section_title = text("MIDI Controller").size(sz(18.0));

    let hint = text("Create a custom mapping for your MIDI controller")
        .size(sz(12.0));

    let learn_btn = button(text("Start MIDI Learn").size(sz(14.0)))
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

/// Audio output settings section (device routing)
fn view_audio_output_section<'a>(state: &'a SettingsState, nav: Option<&SettingsMidiNav>) -> Element<'a, Message> {
    let section_title = text("Audio Output").size(sz(18.0));

    let hint = text("Route master and cue to different audio devices")
        .size(sz(12.0));

    // Master output — show button group when MIDI-editing, pick_list otherwise
    let master_label = text("Master (Speakers):").size(sz(14.0));
    let master_is_editing = nav.is_some_and(|n| n.focused_index == 0 && n.editing);
    let master_control: Element<'_, Message> = if state.available_devices.is_empty() {
        text("No audio devices available").size(sz(12.0)).into()
    } else if master_is_editing {
        // Inline button group so user can see all options while cycling via encoder
        let btns: Vec<Element<Message>> = state.available_devices.iter().enumerate().map(|(idx, dev)| {
            let is_selected = state.draft_master_device == idx;
            button(text(dev.to_string()).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateMasterPair(idx)))
                .style(if is_selected { button::primary } else { button::secondary })
                .into()
        }).collect();
        column(btns).spacing(4).width(Length::Fixed(280.0)).into()
    } else {
        pick_list(
            state.available_devices.clone(),
            state.available_devices.get(state.draft_master_device).cloned(),
            |pair| {
                let idx = state.available_devices.iter()
                    .position(|p| p == &pair)
                    .unwrap_or(0);
                Message::Settings(SettingsMessage::UpdateMasterPair(idx))
            },
        )
        .width(Length::Fixed(280.0))
        .into()
    };
    let master_row = row![master_label, Space::new().width(Length::Fill), master_control]
        .spacing(10)
        .align_y(if master_is_editing { Alignment::Start } else { Alignment::Center });
    let master_row = wrap_navigable(master_row.into(), 0, nav);

    // Cue output — show button group when MIDI-editing, pick_list otherwise
    let cue_label = text("Cue (Headphones):").size(sz(14.0));
    let cue_is_editing = nav.is_some_and(|n| n.focused_index == 1 && n.editing);
    let cue_control: Element<'_, Message> = if state.available_devices.is_empty() {
        text("No audio devices available").size(sz(12.0)).into()
    } else if cue_is_editing {
        let btns: Vec<Element<Message>> = state.available_devices.iter().enumerate().map(|(idx, dev)| {
            let is_selected = state.draft_cue_device == idx;
            button(text(dev.to_string()).size(sz(11.0)))
                .on_press(Message::Settings(SettingsMessage::UpdateCuePair(idx)))
                .style(if is_selected { button::primary } else { button::secondary })
                .into()
        }).collect();
        column(btns).spacing(4).width(Length::Fixed(280.0)).into()
    } else {
        pick_list(
            state.available_devices.clone(),
            state.available_devices.get(state.draft_cue_device).cloned(),
            |pair| {
                let idx = state.available_devices.iter()
                    .position(|p| p == &pair)
                    .unwrap_or(0);
                Message::Settings(SettingsMessage::UpdateCuePair(idx))
            },
        )
        .width(Length::Fixed(280.0))
        .into()
    };
    let cue_row = row![cue_label, Space::new().width(Length::Fill), cue_control]
        .spacing(10)
        .align_y(if cue_is_editing { Alignment::Start } else { Alignment::Center });
    let cue_row = wrap_navigable(cue_row.into(), 1, nav);

    // Refresh button
    let refresh_btn = button(text("Refresh Devices").size(sz(11.0)))
        .on_press(Message::Settings(SettingsMessage::RefreshAudioDevices))
        .style(button::secondary);

    container(
        column![
            section_title,
            hint,
            Space::new().height(5),
            master_row,
            cue_row,
            Space::new().height(5),
            refresh_btn,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}
