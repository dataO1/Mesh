//! Slicer editor modal state

/// State for the slicer editor modal
///
/// Minimal wrapper — just tracks whether the modal is open.
/// The actual `SliceEditorState` data lives on `LoadedTrackState`.
#[derive(Debug, Clone)]
pub struct SlicerEditorState {
    /// Whether the slicer editor modal is open
    pub is_open: bool,
}

impl Default for SlicerEditorState {
    fn default() -> Self {
        Self::new()
    }
}

impl SlicerEditorState {
    pub fn new() -> Self {
        Self { is_open: false }
    }

    pub fn open(&mut self) {
        self.is_open = true;
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }
}
