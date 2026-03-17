//! Settings modal UI for mesh-player
//!
//! Provides a modal dialog for editing player configuration.
//!
//! All navigable settings are defined in a single registry (`build_settings_items()`).
//! Both the view rendering and MIDI navigation derive from this list — no hardcoded
//! indices, no manual counts. Adding a setting = adding one item to the vec.

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
        .position(|&v| (v - lufs).abs() < 0.01)
        .unwrap_or(0)
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
/// Grid density options in beats
pub const GRID_SIZES: [u32; 4] = [8, 16, 32, 64];
/// Slicer buffer bar options
pub const BUFFER_SIZES: [u32; 4] = [1, 4, 8, 16];

// ── Data-Driven Settings Registry ─────────────────────────────────────────────

/// Button width hint for ButtonGroup rendering
pub enum ButtonWidth {
    Fixed(f32),
    Shrink,
}

/// Extra non-navigable widget rendered after an item
pub enum SectionExtra {
    /// "Refresh Devices" button in Audio Output section
    RefreshDevicesButton,
}

/// What MIDI does with this settings item.
/// Determines rendering, scroll behavior, and press behavior.
pub enum SettingsBehavior {
    /// On/off toggle. Scroll flips value, press toggles edit mode.
    Toggle {
        value: bool,
        on_toggle: fn(bool) -> SettingsMessage,
    },
    /// Row of option buttons. Scroll cycles selection, press toggles edit mode.
    ButtonGroup {
        options: Vec<String>,
        selected: usize,
        on_select: fn(usize) -> SettingsMessage,
    },
    /// pick_list normally, inline button column when MIDI editing.
    DeviceSelect {
        devices: Vec<String>,
        selected: usize,
        on_select: fn(usize) -> SettingsMessage,
    },
    /// Press opens a sub-panel (no editing mode).
    SubPanel(SubPanelType),
    /// Press fires a message directly (no editing mode).
    Action(Message),
}

/// A navigable settings entry — single source of truth for view + MIDI.
///
/// Build these with `SettingsItem::new(label, behavior)` and chain setters.
/// The position in `build_settings_items()` IS the nav index.
pub struct SettingsItem {
    /// New section header to render before this item (sz 18)
    pub section: Option<&'static str>,
    /// Hint text below section header (sz 12)
    pub section_hint: Option<&'static str>,
    /// Sub-header within current section (sz 14), rendered above navigable area
    pub subsection: Option<&'static str>,
    /// Hint below subsection label (sz 12)
    pub subsection_hint: Option<&'static str>,
    /// Label for Toggle/DeviceSelect rows (rendered inside navigable area)
    pub label: &'static str,
    /// Hint below label for Toggle items (rendered inside navigable area)
    pub hint: Option<&'static str>,
    /// Prefix label before ButtonGroup buttons ("Beats:", "LUFS:", etc.)
    pub prefix: Option<&'static str>,
    /// Button width for ButtonGroup rendering
    pub button_width: ButtonWidth,
    /// Whether ButtonGroup row should wrap (for Font with many options)
    pub wrap: bool,
    /// Extra text rendered below the navigable area
    pub below_text: Option<String>,
    /// Extra non-navigable widget after this item
    pub section_extra: Option<SectionExtra>,
    /// Render Action button with danger (red) style instead of primary (blue)
    pub danger: bool,
    /// What MIDI does with this item
    pub behavior: SettingsBehavior,
}

impl SettingsItem {
    /// Create a new settings item with the given label and behavior.
    /// All optional fields default to None/false.
    pub fn new(label: &'static str, behavior: SettingsBehavior) -> Self {
        Self {
            section: None,
            section_hint: None,
            subsection: None,
            subsection_hint: None,
            label,
            hint: None,
            prefix: None,
            button_width: ButtonWidth::Fixed(70.0),
            wrap: false,
            below_text: None,
            section_extra: None,
            danger: false,
            behavior,
        }
    }

    /// Start a new section with this header (sz 18)
    pub fn section(mut self, s: &'static str) -> Self { self.section = Some(s); self }
    /// Hint text below section header (sz 12)
    pub fn section_hint(mut self, s: &'static str) -> Self { self.section_hint = Some(s); self }
    /// Sub-header within section (sz 14)
    pub fn subsection(mut self, s: &'static str) -> Self { self.subsection = Some(s); self }
    /// Hint below subsection (sz 12)
    pub fn subsection_hint(mut self, s: &'static str) -> Self { self.subsection_hint = Some(s); self }
    /// Hint below label for Toggle items
    pub fn hint(mut self, s: &'static str) -> Self { self.hint = Some(s); self }
    /// Prefix label before ButtonGroup buttons
    pub fn prefix(mut self, s: &'static str) -> Self { self.prefix = Some(s); self }
    /// Button width for ButtonGroup
    pub fn button_width(mut self, w: ButtonWidth) -> Self { self.button_width = w; self }
    /// Enable wrapping for ButtonGroup row
    pub fn wrap_buttons(mut self) -> Self { self.wrap = true; self }
    /// Extra text below the navigable area
    pub fn below_text(mut self, s: String) -> Self { self.below_text = Some(s); self }
    /// Extra non-navigable widget after item
    pub fn section_extra(mut self, e: SectionExtra) -> Self { self.section_extra = Some(e); self }
    /// Use danger (red) style for Action buttons
    pub fn danger(mut self) -> Self { self.danger = true; self }
}

/// Build the ordered list of ALL navigable settings from current state.
///
/// This is the SINGLE source of truth for which settings exist, their order,
/// their options, and their behavior. Both the view and MIDI navigation
/// derive from this list. Vec position = nav index.
pub fn build_settings_items(state: &SettingsState) -> Vec<SettingsItem> {
    let mut items = Vec::new();

    // ── Set Recording (always first for quick access) ──
    let rec_label = if state.recording_active { "Stop Recording" } else { "Record Set" };
    let mut rec_item = SettingsItem::new(rec_label, SettingsBehavior::Action(
        Message::Settings(SettingsMessage::RecordingConfirm),
    ))
        .section("Recording")
        .hint("Record master output to WAV on all connected USB sticks");
    if state.recording_active {
        rec_item = rec_item.danger();
    }
    items.push(rec_item);

    // ── Power Off (embedded only, first item for quick access) ──
    #[cfg(feature = "embedded-rt")]
    items.push(
        SettingsItem::new("Power Off", SettingsBehavior::Action(
            Message::Settings(SettingsMessage::PowerOffConfirm),
        ))
            .section("System")
            .hint("Safely shut down the device")
            .danger()
    );

    items.extend([
        // ── Audio Output ──
        SettingsItem::new("Master (Speakers):", SettingsBehavior::DeviceSelect {
            devices: state.available_devices.iter().map(|d| d.to_string()).collect(),
            selected: state.draft_master_device,
            on_select: |idx| SettingsMessage::UpdateMasterPair(idx),
        })
            .section("Audio Output")
            .section_hint("Route master and cue to different audio devices"),

        SettingsItem::new("Cue (Headphones):", SettingsBehavior::DeviceSelect {
            devices: state.available_devices.iter().map(|d| d.to_string()).collect(),
            selected: state.draft_cue_device,
            on_select: |idx| SettingsMessage::UpdateCuePair(idx),
        })
            .section_extra(SectionExtra::RefreshDevicesButton),

        // ── Playback ──
        SettingsItem::new("Automatic Beat Sync", SettingsBehavior::Toggle {
            value: state.draft_phase_sync,
            on_toggle: |v| SettingsMessage::UpdatePhaseSync(v),
        })
            .section("Playback")
            .hint("Automatically align beats when starting playback or hot cues"),

        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: LOOP_LENGTH_OPTIONS.iter().map(|&b| format_beats(b)).collect(),
            selected: state.draft_loop_length_index,
            on_select: |idx| SettingsMessage::UpdateLoopLength(idx),
        })
            .subsection("Default Loop/Beat Jump Length")
            .subsection_hint("Loop length also controls beat jump distance")
            .prefix("Beats:")
            .button_width(ButtonWidth::Fixed(36.0)),

        // ── Display ──
        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: WaveformLayout::ALL.iter().map(|l| l.display_name().to_string()).collect(),
            selected: WaveformLayout::ALL.iter().position(|&l| l == state.draft_waveform_layout).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateWaveformLayout(WaveformLayout::ALL[idx.min(WaveformLayout::ALL.len() - 1)]),
        })
            .section("Display")
            .subsection("Waveform Layout")
            .subsection_hint("Orientation of waveform display")
            .button_width(ButtonWidth::Fixed(100.0)),

        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: WaveformAbstraction::ALL.iter().map(|a| a.display_name().to_string()).collect(),
            selected: WaveformAbstraction::ALL.iter().position(|&a| a == state.draft_waveform_abstraction).unwrap_or(1),
            on_select: |idx| SettingsMessage::UpdateWaveformAbstraction(WaveformAbstraction::ALL[idx.min(WaveformAbstraction::ALL.len() - 1)]),
        })
            .subsection("Waveform Abstraction")
            .subsection_hint("Grid-aligned subsampling intensity (Low = detailed, High = smooth)")
            .button_width(ButtonWidth::Fixed(70.0)),

        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: ZOOM_SIZES.iter().map(|s| format!("{} bars", s)).collect(),
            selected: ZOOM_SIZES.iter().position(|&s| s == state.draft_zoom_bars).unwrap_or(2),
            on_select: |idx| SettingsMessage::UpdateZoomBars(ZOOM_SIZES[idx.min(ZOOM_SIZES.len() - 1)]),
        })
            .subsection("Default Zoomed Waveform Level")
            .subsection_hint("Number of bars visible in zoomed waveform view")
            .prefix("Bars:")
            .button_width(ButtonWidth::Fixed(36.0)),

        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: GRID_SIZES.iter().map(|s| format!("{} beats", s)).collect(),
            selected: GRID_SIZES.iter().position(|&s| s == state.draft_grid_bars).unwrap_or(1),
            on_select: |idx| SettingsMessage::UpdateGridBars(GRID_SIZES[idx.min(GRID_SIZES.len() - 1)]),
        })
            .subsection("Overview Grid Density")
            .subsection_hint("Beat grid line spacing on the overview waveform")
            .prefix("Beats:")
            .button_width(ButtonWidth::Fixed(36.0)),

        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: state.available_theme_names.clone(),
            selected: state.available_theme_names.iter().position(|n| *n == state.draft_theme).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateThemeIndex(idx),
        })
            .subsection("Theme")
            .subsection_hint("Color scheme for UI and waveform visualization")
            .button_width(ButtonWidth::Fixed(80.0)),

        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: AppFont::ALL.iter().map(|f| f.display_name().to_string()).collect(),
            selected: AppFont::ALL.iter().position(|&f| f == state.draft_font).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateFont(AppFont::ALL[idx.min(AppFont::ALL.len() - 1)]),
        })
            .subsection("Font")
            .subsection_hint("UI typeface (restart required to apply)")
            .button_width(ButtonWidth::Shrink)
            .wrap_buttons(),

        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: FontSize::ALL.iter().map(|f| f.display_name().to_string()).collect(),
            selected: FontSize::ALL.iter().position(|&f| f == state.draft_font_size).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateFontSize(FontSize::ALL[idx.min(FontSize::ALL.len() - 1)]),
        })
            .subsection("Font Size")
            .subsection_hint("Text size preset (restart required to apply)")
            .button_width(ButtonWidth::Fixed(70.0)),

        SettingsItem::new("Show Local Collection", SettingsBehavior::Toggle {
            value: state.draft_show_local_collection,
            on_toggle: |v| SettingsMessage::UpdateShowLocalCollection(v),
        })
            .subsection("Browser")
            .hint("Display local music library alongside USB devices"),

        SettingsItem::new("Persistent Browse", SettingsBehavior::Toggle {
            value: state.draft_persistent_browse,
            on_toggle: |v| SettingsMessage::UpdatePersistentBrowse(v),
        })
            .hint("Keep browser visible while browse mode is active (disable idle timeout)"),

        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: KeyScoringModel::ALL.iter().map(|m| m.display_name().to_string()).collect(),
            selected: KeyScoringModel::ALL.iter().position(|&m| m == state.draft_key_scoring_model).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateKeyScoringModel(KeyScoringModel::ALL[idx.min(KeyScoringModel::ALL.len() - 1)]),
        })
            .subsection("Key Matching")
            .subsection_hint("Algorithm for harmonic compatibility scoring")
            .button_width(ButtonWidth::Fixed(85.0)),

        // ── Loudness ──
        SettingsItem::new("Auto-Gain Normalization", SettingsBehavior::Toggle {
            value: state.draft_auto_gain_enabled,
            on_toggle: |v| SettingsMessage::UpdateAutoGainEnabled(v),
        })
            .section("Loudness")
            .hint("Automatically adjust track volume to match target loudness"),

        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: TARGET_LUFS_OPTIONS.iter().map(|&l| format!("{:.0}", l)).collect(),
            selected: state.draft_target_lufs_index,
            on_select: |idx| SettingsMessage::UpdateTargetLufs(idx),
        })
            .subsection("Target Loudness")
            .subsection_hint("Tracks will be gain-compensated to reach this level")
            .prefix("LUFS:")
            .button_width(ButtonWidth::Fixed(50.0))
            .below_text(lufs_preset_name(state.draft_target_lufs_index).to_string()),

        // ── Slicer ──
        SettingsItem::new("", SettingsBehavior::ButtonGroup {
            options: BUFFER_SIZES.iter().map(|s| format!("{} bars", s)).collect(),
            selected: BUFFER_SIZES.iter().position(|&s| s == state.draft_slicer_buffer_bars).unwrap_or(0),
            on_select: |idx| SettingsMessage::UpdateSlicerBufferBars(BUFFER_SIZES[idx.min(BUFFER_SIZES.len() - 1)]),
        })
            .section("Slicer")
            .subsection("Buffer Size")
            .subsection_hint("Size of the slicer buffer window (16 slices)")
            .prefix("Bars:")
            .button_width(ButtonWidth::Fixed(44.0))
            .below_text("Edit slicer presets and per-stem patterns in mesh-cue".into()),
    ]);

    // ── Conditional sections ──
    if state.network.is_some() {
        items.push(SettingsItem::new("Network", SettingsBehavior::SubPanel(SubPanelType::Network)));
    }
    if state.update.is_some() {
        items.push(
            SettingsItem::new("Pre-release Updates", SettingsBehavior::Toggle {
                value: state.draft_prerelease_channel,
                on_toggle: |v| SettingsMessage::UpdatePrereleaseChannel(v),
            })
                .section("System Update")
                .hint("Include release candidates and beta versions in OTA checks")
        );
        items.push(SettingsItem::new("System Update", SettingsBehavior::SubPanel(SubPanelType::SystemUpdate)));
    }

    // ── MIDI Learn ──
    items.push(
        SettingsItem::new("Start MIDI Learn", SettingsBehavior::Action(
            Message::MidiLearn(MidiLearnMessage::Start),
        ))
            .section("MIDI Controller")
            .section_hint("Create a custom mapping for your MIDI controller")
    );

    items
}

// ── Settings State ────────────────────────────────────────────────────────────

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
    /// Draft persistent browse (keep browser overlay visible while browse mode active)
    pub draft_persistent_browse: bool,
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
    /// Draft prerelease channel toggle (include RC/beta in OTA checks)
    pub draft_prerelease_channel: bool,
    /// Whether set recording is currently active
    pub recording_active: bool,
    /// Whether the set recording confirmation dialog is showing
    pub recording_confirm: bool,
    /// Whether the power off confirmation dialog is showing
    pub power_off_confirm: bool,
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
            available_theme_names: Vec::new(),
            draft_phase_sync: config.audio.phase_sync,
            draft_slicer_buffer_bars: config.slicer.validated_buffer_bars(),
            draft_auto_gain_enabled: config.audio.loudness.auto_gain_enabled,
            draft_target_lufs_index: lufs_to_index(config.audio.loudness.target_lufs),
            draft_show_local_collection: config.display.show_local_collection,
            draft_persistent_browse: config.display.persistent_browse,
            draft_key_scoring_model: config.display.key_scoring_model,
            draft_waveform_layout: config.display.waveform_layout,
            draft_waveform_abstraction: config.display.waveform_abstraction,
            draft_font: config.display.font,
            draft_font_size: config.display.font_size,
            draft_master_device: config.audio.outputs.master_device.unwrap_or(0).min(num_devices.saturating_sub(1)),
            draft_cue_device: config.audio.outputs.cue_device.unwrap_or(if num_devices > 1 { 1 } else { 0 }).min(num_devices.saturating_sub(1)),
            available_devices,
            draft_prerelease_channel: config.updates.prerelease_channel,
            recording_active: false,
            recording_confirm: false,
            power_off_confirm: false,
            status: String::new(),
            settings_midi_nav: None,
            network: super::handlers::network::init_network_state(),
            update: super::handlers::system_update::init_update_state(),
            initial_snapshot: None,
        }
    }

    /// Take a snapshot of current draft values (for dirty detection)
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
            persistent_browse: self.draft_persistent_browse,
            key_scoring_model: self.draft_key_scoring_model,
            waveform_layout: self.draft_waveform_layout,
            waveform_abstraction: self.draft_waveform_abstraction,
            font: self.draft_font,
            font_size: self.draft_font_size,
            master_device: self.draft_master_device,
            cue_device: self.draft_cue_device,
            prerelease_channel: self.draft_prerelease_channel,
        });
    }

    /// Check if any setting has changed since the snapshot
    pub fn has_changes(&self) -> bool {
        if let Some(ref snap) = self.initial_snapshot {
            self.draft_loop_length_index != snap.loop_length_index
            || self.draft_zoom_bars != snap.zoom_bars
            || self.draft_grid_bars != snap.grid_bars
            || self.draft_theme != snap.theme
            || self.draft_phase_sync != snap.phase_sync
            || self.draft_slicer_buffer_bars != snap.slicer_buffer_bars
            || self.draft_auto_gain_enabled != snap.auto_gain_enabled
            || self.draft_target_lufs_index != snap.target_lufs_index
            || self.draft_show_local_collection != snap.show_local_collection
            || self.draft_persistent_browse != snap.persistent_browse
            || self.draft_key_scoring_model != snap.key_scoring_model
            || self.draft_waveform_layout != snap.waveform_layout
            || self.draft_waveform_abstraction != snap.waveform_abstraction
            || self.draft_font != snap.font
            || self.draft_font_size != snap.font_size
            || self.draft_master_device != snap.master_device
            || self.draft_cue_device != snap.cue_device
            || self.draft_prerelease_channel != snap.prerelease_channel
        } else {
            false
        }
    }

    /// Refresh the available audio devices list
    pub fn refresh_audio_devices(&mut self) {
        self.available_devices = get_available_stereo_pairs();
    }
}

/// Snapshot of draft values for dirty detection
#[derive(Debug, Clone)]
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
    persistent_browse: bool,
    key_scoring_model: KeyScoringModel,
    waveform_layout: WaveformLayout,
    waveform_abstraction: WaveformAbstraction,
    font: AppFont,
    font_size: FontSize,
    master_device: usize,
    cue_device: usize,
    prerelease_channel: bool,
}

// ── MIDI Navigation State ─────────────────────────────────────────────────────

/// Sub-panel focus for settings entries that open inline panels
#[derive(Debug, Clone)]
pub enum SubPanelFocus {
    /// Cycling through WiFi networks
    WifiNetworkList { selected: usize },
    /// Cycling through update actions (Check/Install or Install/Restart)
    UpdateActions { selected: usize },
    /// Cycling through set recording confirmation (0=Cancel, 1=Start/Stop)
    RecordingConfirm { selected: usize },
    /// Cycling through power off confirmation (0=Cancel, 1=Power Off)
    PowerOffConfirm { selected: usize },
}

/// Which kind of sub-panel a settings item opens
pub enum SubPanelType {
    Network,
    SystemUpdate,
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

// ── View Rendering ────────────────────────────────────────────────────────────

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

/// Wrap a dialog button with highlight when it's the focused action via MIDI.
fn wrap_dialog_focus<'a>(
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
        .into()
}

/// Render a single settings item's widget based on its behavior variant.
///
/// String content from SettingsItem is cloned into the widget tree so the returned
/// Element does not borrow from the item (which is a local in view()).
/// SubPanel items delegate to external view functions that may borrow from state.
fn render_item<'a>(
    item: &SettingsItem,
    index: usize,
    nav: Option<&SettingsMidiNav>,
    state: &'a SettingsState,
) -> Element<'a, Message> {
    let is_editing = nav.is_some_and(|n| n.focused_index == index && n.editing);

    match &item.behavior {
        SettingsBehavior::Toggle { value, on_toggle } => {
            let on_toggle = *on_toggle;
            let toggle = toggler(*value)
                .on_toggle(move |v| Message::Settings(on_toggle(v)));
            let mut label_col: Vec<Element<'static, Message>> = vec![
                text(item.label.to_string()).size(sz(14.0)).into(),
            ];
            if let Some(hint) = item.hint {
                label_col.push(text(hint.to_string()).size(sz(12.0)).into());
            }
            row![
                column(label_col).spacing(4),
                Space::new().width(Length::Fill),
                toggle,
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        }

        SettingsBehavior::ButtonGroup { options, selected, on_select } => {
            let on_select = *on_select;
            let buttons: Vec<Element<'static, Message>> = options.iter().enumerate().map(|(idx, opt)| {
                let is_sel = *selected == idx;
                let btn = button(text(opt.clone()).size(sz(11.0)))
                    .on_press(Message::Settings(on_select(idx)))
                    .style(if is_sel { button::primary } else { button::secondary });
                let btn = match &item.button_width {
                    ButtonWidth::Fixed(w) => btn.width(Length::Fixed(*w)),
                    ButtonWidth::Shrink => btn.width(Length::Shrink),
                };
                btn.into()
            }).collect();

            if item.wrap {
                let btn_row = row(buttons).spacing(4).align_y(Alignment::Center).wrap();
                if let Some(prefix) = item.prefix {
                    row![text(prefix.to_string()).size(sz(14.0)), btn_row]
                        .spacing(10).align_y(Alignment::Center).into()
                } else {
                    btn_row.into()
                }
            } else if let Some(prefix) = item.prefix {
                let btn_row = row(buttons).spacing(4).align_y(Alignment::Center);
                row![text(prefix.to_string()).size(sz(14.0)), btn_row]
                    .spacing(10).align_y(Alignment::Center).into()
            } else {
                row(buttons).spacing(4).align_y(Alignment::Center).into()
            }
        }

        SettingsBehavior::DeviceSelect { devices, selected, on_select } => {
            let on_select = *on_select;
            let label_text = text(item.label.to_string()).size(sz(14.0));

            if devices.is_empty() {
                row![label_text, Space::new().width(Length::Fill),
                     text("No audio devices available").size(sz(12.0))]
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .into()
            } else if is_editing {
                let btns: Vec<Element<'static, Message>> = devices.iter().enumerate().map(|(idx, dev)| {
                    let is_sel = *selected == idx;
                    button(text(dev.clone()).size(sz(11.0)))
                        .on_press(Message::Settings(on_select(idx)))
                        .style(if is_sel { button::primary } else { button::secondary })
                        .into()
                }).collect();
                row![label_text, Space::new().width(Length::Fill),
                     column(btns).spacing(4).width(Length::Fixed(280.0))]
                    .spacing(10)
                    .align_y(Alignment::Start)
                    .into()
            } else {
                let devices_for_picklist = state.available_devices.clone();
                let control: Element<'static, Message> = pick_list(
                    devices_for_picklist.clone(),
                    devices_for_picklist.get(*selected).cloned(),
                    move |pair: StereoPair| {
                        let idx = devices_for_picklist.iter()
                            .position(|p| p == &pair)
                            .unwrap_or(0);
                        Message::Settings(on_select(idx))
                    },
                )
                .width(Length::Fixed(280.0))
                .into();

                row![label_text, Space::new().width(Length::Fill), control]
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .into()
            }
        }

        SettingsBehavior::SubPanel(SubPanelType::Network) => {
            if let Some(ref ns) = state.network {
                super::network::view_network_section(ns)
            } else {
                Space::new().into()
            }
        }

        SettingsBehavior::SubPanel(SubPanelType::SystemUpdate) => {
            let focused_action = nav.and_then(|n| {
                if let Some(SubPanelFocus::UpdateActions { selected }) = &n.sub_panel {
                    Some(*selected)
                } else {
                    None
                }
            });
            if let Some(ref us) = state.update {
                super::system_update::view_update_section(us, focused_action)
            } else {
                Space::new().into()
            }
        }

        SettingsBehavior::Action(msg) => {
            let style = if item.danger { button::danger } else { button::primary };
            button(text(item.label.to_string()).size(sz(14.0)))
                .on_press(msg.clone())
                .style(style)
                .into()
        }
    }
}

/// Wrap a list of section elements into a section container with header.
fn flush_section<'a>(
    header: Option<&str>,
    header_hint: Option<&str>,
    items: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut col_items: Vec<Element<'a, Message>> = Vec::new();
    if let Some(h) = header {
        col_items.push(text(h.to_string()).size(sz(18.0)).into());
    }
    if let Some(h) = header_hint {
        col_items.push(text(h.to_string()).size(sz(12.0)).into());
        col_items.push(Space::new().height(5).into());
    }
    col_items.extend(items);
    container(column(col_items).spacing(8))
        .padding(15)
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

    let items = build_settings_items(state);
    let nav = state.settings_midi_nav.as_ref();

    let mut sections: Vec<Element<'_, Message>> = Vec::new();
    let mut current_section_items: Vec<Element<'_, Message>> = Vec::new();
    let mut current_section_header: Option<String> = None;
    let mut current_section_hint: Option<String> = None;

    for (index, item) in items.iter().enumerate() {
        // SubPanel items are standalone — they include their own section container
        if matches!(&item.behavior, SettingsBehavior::SubPanel(_)) {
            // Flush any pending section first
            if !current_section_items.is_empty() || current_section_header.is_some() {
                sections.push(flush_section(
                    current_section_header.take().as_deref(), current_section_hint.take().as_deref(),
                    std::mem::take(&mut current_section_items),
                ));
            }
            // Render the sub-panel view, wrapped in navigable highlight
            let rendered = render_item(item, index, nav, state);
            sections.push(wrap_navigable(rendered, index, nav));
            continue;
        }

        // Start a new section if this item declares one
        if item.section.is_some() {
            // Flush previous section
            if !current_section_items.is_empty() || current_section_header.is_some() {
                sections.push(flush_section(
                    current_section_header.take().as_deref(), current_section_hint.take().as_deref(),
                    std::mem::take(&mut current_section_items),
                ));
            }
            current_section_header = item.section.map(|s| s.to_string());
            current_section_hint = item.section_hint.map(|s| s.to_string());
        }

        // Render subsection label + hint above the navigable area
        if let Some(subsection) = item.subsection {
            current_section_items.push(Space::new().height(10).into());
            current_section_items.push(text(subsection.to_string()).size(sz(14.0)).into());
            if let Some(hint) = item.subsection_hint {
                current_section_items.push(text(hint.to_string()).size(sz(12.0)).into());
            }
        }

        // Render the item widget, wrapped in navigable highlight
        let rendered = render_item(item, index, nav, state);
        current_section_items.push(wrap_navigable(rendered, index, nav));

        // Render below_text if present
        if let Some(ref below) = item.below_text {
            current_section_items.push(text(below.clone()).size(sz(12.0)).into());
        }

        // Render section_extra if present
        if let Some(ref extra) = item.section_extra {
            match extra {
                SectionExtra::RefreshDevicesButton => {
                    current_section_items.push(Space::new().height(5).into());
                    current_section_items.push(
                        button(text("Refresh Devices").size(sz(11.0)))
                            .on_press(Message::Settings(SettingsMessage::RefreshAudioDevices))
                            .style(button::secondary)
                            .into()
                    );
                }
            }
        }
    }

    // Flush the last section
    if !current_section_items.is_empty() || current_section_header.is_some() {
        sections.push(flush_section(
            current_section_header.take().as_deref(), current_section_hint.take().as_deref(),
            std::mem::take(&mut current_section_items),
        ));
    }

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
        .width(Length::Fixed(550.0))
        .height(Length::Fixed(600.0));

    let settings_view: Element<Message> = container(content)
        .padding(30)
        .style(container::rounded_box)
        .into();

    // Set recording confirmation overlay
    if state.recording_confirm {
        use iced::widget::{center, stack};

        let focused_action = state.settings_midi_nav.as_ref().and_then(|n| {
            if let Some(SubPanelFocus::RecordingConfirm { selected }) = &n.sub_panel {
                Some(*selected)
            } else {
                None
            }
        });

        let backdrop: Element<Message> = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_theme: &iced::Theme| container::Style {
                background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.5).into()),
                ..Default::default()
            })
            .into();

        let cancel_btn: Element<Message> = button(text("Cancel").size(sz(16.0)))
            .on_press(Message::Settings(SettingsMessage::RecordingCancel))
            .style(button::secondary)
            .padding([8, 24])
            .into();

        let (title, description, confirm_label, confirm_style) = if state.recording_active {
            ("Stop Recording?", "The current recording will be finalized and saved.", "Stop", button::danger as fn(&iced::Theme, button::Status) -> button::Style)
        } else {
            ("Start Recording?", "Master output will be recorded to WAV on all connected USB sticks.", "Record", button::primary as fn(&iced::Theme, button::Status) -> button::Style)
        };

        let confirm_btn: Element<Message> = button(text(confirm_label).size(sz(16.0)))
            .on_press(Message::Settings(SettingsMessage::RecordingExecute))
            .style(confirm_style)
            .padding([8, 24])
            .into();

        let cancel_btn = wrap_dialog_focus(cancel_btn, 0, focused_action);
        let confirm_btn = wrap_dialog_focus(confirm_btn, 1, focused_action);

        let dialog = container(
            column![
                text(title).size(sz(22.0)),
                text(description).size(sz(14.0)),
                Space::new().height(10),
                row![Space::new().width(Length::Fill), cancel_btn, confirm_btn]
                    .spacing(10)
                    .width(Length::Fill),
            ]
            .spacing(10)
            .width(Length::Fixed(400.0))
        )
        .padding(25)
        .style(container::rounded_box);

        return stack![settings_view, backdrop, center(dialog)].into();
    }

    // Power off confirmation overlay (embedded only)
    if state.power_off_confirm {
        use iced::widget::{center, stack};

        // Extract MIDI focus for the confirmation dialog buttons
        let focused_action = state.settings_midi_nav.as_ref().and_then(|n| {
            if let Some(SubPanelFocus::PowerOffConfirm { selected }) = &n.sub_panel {
                Some(*selected)
            } else {
                None
            }
        });

        let backdrop: Element<Message> = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_theme: &iced::Theme| container::Style {
                background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.5).into()),
                ..Default::default()
            })
            .into();

        let cancel_btn: Element<Message> = button(text("Cancel").size(sz(16.0)))
            .on_press(Message::Settings(SettingsMessage::PowerOffCancel))
            .style(button::secondary)
            .padding([8, 24])
            .into();

        let confirm_btn: Element<Message> = button(text("Power Off").size(sz(16.0)))
            .on_press(Message::Settings(SettingsMessage::PowerOffExecute))
            .style(button::danger)
            .padding([8, 24])
            .into();

        // Wrap buttons with MIDI focus highlight
        let cancel_btn = wrap_dialog_focus(cancel_btn, 0, focused_action);
        let confirm_btn = wrap_dialog_focus(confirm_btn, 1, focused_action);

        let dialog = container(
            column![
                text("Power Off?").size(sz(22.0)),
                text("The device will shut down. Are you sure?").size(sz(14.0)),
                Space::new().height(10),
                row![Space::new().width(Length::Fill), cancel_btn, confirm_btn]
                    .spacing(10)
                    .width(Length::Fill),
            ]
            .spacing(10)
            .width(Length::Fixed(350.0))
        )
        .padding(25)
        .style(container::rounded_box);

        stack![settings_view, backdrop, center(dialog)].into()
    } else {
        settings_view
    }
}
