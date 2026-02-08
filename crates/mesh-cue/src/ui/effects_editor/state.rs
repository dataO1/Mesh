//! Effects editor state

use mesh_core::types::Stem;
use mesh_widgets::MultibandEditorState;

/// State for the effects editor modal
///
/// Wraps the MultibandEditorState from mesh-widgets and adds
/// mesh-cue specific functionality like preset management UI.
#[derive(Debug, Clone)]
pub struct EffectsEditorState {
    /// Whether the effects editor modal is open
    pub is_open: bool,

    /// Core multiband editor state (from mesh-widgets)
    pub editor: MultibandEditorState,

    /// Preset currently being edited (None = new/unsaved)
    pub editing_preset: Option<String>,

    /// Whether save dialog is showing
    pub save_dialog_open: bool,

    /// Text input for preset name (for save dialog)
    pub preset_name_input: String,

    /// Status message to display
    pub status: String,

    /// Which stem to use for audio preview (default: Other for full mix context)
    pub preview_stem: Stem,

    /// Whether audio preview is enabled (applies editor changes in real-time)
    pub audio_preview_enabled: bool,
}

impl Default for EffectsEditorState {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectsEditorState {
    /// Create a new effects editor state
    pub fn new() -> Self {
        Self {
            is_open: false,
            editor: MultibandEditorState::new(),
            editing_preset: None,
            save_dialog_open: false,
            preset_name_input: String::new(),
            status: String::new(),
            preview_stem: Stem::Other, // Default to "Other" stem for full mix preview
            audio_preview_enabled: false, // Disabled by default - user can enable
        }
    }

    /// Open the effects editor
    pub fn open(&mut self) {
        self.is_open = true;
        self.status.clear();
        // Set up editor state for a generic "preview" context
        // In mesh-cue, we're editing presets, not per-deck/stem effects
        self.editor.open(0, 0, "Preview");
    }

    /// Close the effects editor
    pub fn close(&mut self) {
        self.is_open = false;
        self.save_dialog_open = false;
        self.editor.close();
    }

    /// Start editing a new preset
    pub fn new_preset(&mut self) {
        self.editing_preset = None;
        self.preset_name_input = "New Preset".to_string();
        self.editor = MultibandEditorState::new();
        self.editor.open(0, 0, "Preview");
        // Preserve preview settings when starting a new preset
    }

    /// Set the preview stem
    pub fn set_preview_stem(&mut self, stem: Stem) {
        self.preview_stem = stem;
    }

    /// Toggle audio preview
    pub fn toggle_audio_preview(&mut self) {
        self.audio_preview_enabled = !self.audio_preview_enabled;
    }

    /// Load a preset for editing
    pub fn load_preset(&mut self, name: String) {
        self.editing_preset = Some(name.clone());
        self.preset_name_input = name;
    }

    /// Open save dialog
    pub fn open_save_dialog(&mut self) {
        if self.editing_preset.is_some() {
            // Use existing name
            self.preset_name_input = self.editing_preset.clone().unwrap_or_default();
        }
        self.save_dialog_open = true;
    }

    /// Close save dialog
    pub fn close_save_dialog(&mut self) {
        self.save_dialog_open = false;
    }

    /// Set status message
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }
}
