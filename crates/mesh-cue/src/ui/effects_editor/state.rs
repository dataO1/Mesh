//! Effects editor state

use mesh_core::types::Stem;
use mesh_widgets::multiband::StemEffectData;
use mesh_widgets::MultibandEditorState;

/// State for the effects editor modal
///
/// Wraps the MultibandEditorState from mesh-widgets and adds
/// mesh-cue specific functionality like preset management UI.
///
/// Note: Fields like `save_dialog_open` and `preset_name_input` live in the
/// inner `editor` state since the view reads from there directly.
#[derive(Debug, Clone)]
pub struct EffectsEditorState {
    /// Whether the effects editor modal is open
    pub is_open: bool,

    /// Core multiband editor state (from mesh-widgets)
    /// Shows the currently active stem's effects
    pub editor: MultibandEditorState,

    /// Saved per-stem effect data (for stem switching without data loss)
    /// When switching stems, the current stem's effects are snapshotted here
    /// and the new stem's effects are restored from here.
    pub stem_data: [Option<StemEffectData>; 4],

    /// Per-stem loaded preset names (from deck preset references)
    pub stem_preset_names: [Option<String>; 4],

    /// Which stem is currently being edited (0-3)
    pub active_stem: usize,

    /// Loaded deck preset name
    pub deck_preset_name: Option<String>,

    /// Preset currently being edited (None = new/unsaved)
    pub editing_preset: Option<String>,

    /// Status message to display
    pub status: String,

    /// Whether audio preview is enabled (applies editor changes in real-time)
    /// When enabled, all stems with data get their effects synced to audio.
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
            stem_data: [None, None, None, None],
            stem_preset_names: [None, None, None, None],
            active_stem: 0,
            deck_preset_name: None,
            editing_preset: None,
            status: String::new(),
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
        self.editor.save_dialog_open = false;
        self.editor.close();
    }

    /// Start editing a new preset
    pub fn new_preset(&mut self) {
        self.editing_preset = None;
        self.editor = MultibandEditorState::new();
        self.editor.preset_name_input = "New Preset".to_string();
        self.editor.open(0, 0, "Preview");
        // Reset stem data for new preset
        self.stem_data = [None, None, None, None];
        self.stem_preset_names = [None, None, None, None];
        self.active_stem = 0;
        self.deck_preset_name = None;
    }

    /// Get the active stem as a Stem type
    pub fn active_stem_type(&self) -> Stem {
        Stem::from_index(self.active_stem).unwrap_or(Stem::Other)
    }

    /// Toggle audio preview
    pub fn toggle_audio_preview(&mut self) {
        self.audio_preview_enabled = !self.audio_preview_enabled;
    }

    /// Load a preset for editing
    pub fn load_preset(&mut self, name: String) {
        self.editing_preset = Some(name.clone());
        self.editor.preset_name_input = name;
    }

    /// Open save dialog
    pub fn open_save_dialog(&mut self) {
        if let Some(ref name) = self.editing_preset {
            // Use existing name
            self.editor.preset_name_input = name.clone();
        }
        self.editor.save_dialog_open = true;
    }

    /// Close save dialog
    pub fn close_save_dialog(&mut self) {
        self.editor.save_dialog_open = false;
    }

    /// Set status message
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }
}
