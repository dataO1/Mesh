//! File browser component
//!
//! Displays a file browser for loading tracks into decks:
//! - Directory listing with folders and audio files
//! - Navigation (back, into folders)
//! - Load to deck buttons
//! - Scrollable list

use std::path::{Path, PathBuf};

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Center, Element, Fill, Length};

/// File entry type
#[derive(Debug, Clone)]
pub enum FileEntryType {
    Directory,
    AudioFile,
}

/// A file or directory entry
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Entry name (filename only)
    pub name: String,
    /// Full path
    pub path: PathBuf,
    /// Entry type
    pub entry_type: FileEntryType,
}

/// State for the file browser
pub struct FileBrowserView {
    /// Current directory
    current_directory: PathBuf,
    /// List of entries in current directory
    entries: Vec<FileEntry>,
    /// Selected entry index
    selected_index: Option<usize>,
    /// Error message if directory scan failed
    error: Option<String>,
}

/// Messages for file browser interaction
#[derive(Debug, Clone)]
pub enum FileBrowserMessage {
    /// Navigate up to parent directory
    NavigateUp,
    /// Navigate into a directory (by index)
    NavigateInto(usize),
    /// Select an entry
    Select(usize),
    /// Load selected file to deck
    LoadToDeck(usize),
    /// Refresh current directory
    Refresh,
}

impl FileBrowserView {
    /// Create a new file browser starting at home directory
    pub fn new() -> Self {
        // Get home directory from environment
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"));

        let mut browser = Self {
            current_directory: home,
            entries: Vec::new(),
            selected_index: None,
            error: None,
        };
        browser.scan_directory();
        browser
    }

    /// Create a new file browser at a specific path
    pub fn at_path<P: AsRef<Path>>(path: P) -> Self {
        let mut browser = Self {
            current_directory: path.as_ref().to_path_buf(),
            entries: Vec::new(),
            selected_index: None,
            error: None,
        };
        browser.scan_directory();
        browser
    }

    /// Scan the current directory for entries
    fn scan_directory(&mut self) {
        self.entries.clear();
        self.selected_index = None;
        self.error = None;

        let dir = match std::fs::read_dir(&self.current_directory) {
            Ok(dir) => dir,
            Err(e) => {
                self.error = Some(format!("Error reading directory: {}", e));
                return;
            }
        };

        let mut entries: Vec<FileEntry> = dir
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip hidden files
                if name.starts_with('.') {
                    return None;
                }

                if path.is_dir() {
                    Some(FileEntry {
                        name,
                        path,
                        entry_type: FileEntryType::Directory,
                    })
                } else if is_audio_file(&path) {
                    Some(FileEntry {
                        name,
                        path,
                        entry_type: FileEntryType::AudioFile,
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort: directories first, then files, alphabetically
        entries.sort_by(|a, b| {
            match (&a.entry_type, &b.entry_type) {
                (FileEntryType::Directory, FileEntryType::AudioFile) => std::cmp::Ordering::Less,
                (FileEntryType::AudioFile, FileEntryType::Directory) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        self.entries = entries;
    }

    /// Handle a file browser message
    /// Returns Some(path) if a file should be loaded to a deck
    pub fn handle_message(&mut self, msg: FileBrowserMessage) -> Option<(usize, PathBuf)> {
        match msg {
            FileBrowserMessage::NavigateUp => {
                if let Some(parent) = self.current_directory.parent() {
                    self.current_directory = parent.to_path_buf();
                    self.scan_directory();
                }
                None
            }
            FileBrowserMessage::NavigateInto(idx) => {
                if let Some(entry) = self.entries.get(idx) {
                    if matches!(entry.entry_type, FileEntryType::Directory) {
                        self.current_directory = entry.path.clone();
                        self.scan_directory();
                    }
                }
                None
            }
            FileBrowserMessage::Select(idx) => {
                if idx < self.entries.len() {
                    self.selected_index = Some(idx);
                }
                None
            }
            FileBrowserMessage::LoadToDeck(deck_idx) => {
                if let Some(selected) = self.selected_index {
                    if let Some(entry) = self.entries.get(selected) {
                        if matches!(entry.entry_type, FileEntryType::AudioFile) {
                            return Some((deck_idx, entry.path.clone()));
                        }
                    }
                }
                None
            }
            FileBrowserMessage::Refresh => {
                self.scan_directory();
                None
            }
        }
    }

    /// Build the file browser view
    pub fn view(&self) -> Element<FileBrowserMessage> {
        // Header with current path and back button
        let path_display = self.current_directory.to_string_lossy();
        let path_text = text(format!("{}", path_display))
            .size(11);

        let back_btn = button(text("←").size(14))
            .on_press(FileBrowserMessage::NavigateUp)
            .padding(5);

        let refresh_btn = button(text("⟳").size(14))
            .on_press(FileBrowserMessage::Refresh)
            .padding(5);

        let header = row![
            back_btn,
            path_text,
            Space::new().width(Fill),
            refresh_btn,
        ]
        .spacing(5)
        .align_y(Center);

        // File list
        let file_list = if let Some(error) = &self.error {
            column![text(error).size(12)]
        } else if self.entries.is_empty() {
            column![text("Empty directory").size(12)]
        } else {
            let entries: Vec<Element<FileBrowserMessage>> = self.entries
                .iter()
                .enumerate()
                .map(|(idx, entry)| self.view_entry(idx, entry))
                .collect();

            column(entries).spacing(2)
        };

        let scrollable_list = scrollable(file_list)
            .height(Length::Fill);

        // Load to deck buttons
        let has_audio_selected = self.selected_index
            .and_then(|idx| self.entries.get(idx))
            .map(|e| matches!(e.entry_type, FileEntryType::AudioFile))
            .unwrap_or(false);

        let load_buttons = if has_audio_selected {
            row![
                button(text("→ Deck 1").size(10))
                    .on_press(FileBrowserMessage::LoadToDeck(0))
                    .padding(4),
                button(text("→ Deck 2").size(10))
                    .on_press(FileBrowserMessage::LoadToDeck(1))
                    .padding(4),
                button(text("→ Deck 3").size(10))
                    .on_press(FileBrowserMessage::LoadToDeck(2))
                    .padding(4),
                button(text("→ Deck 4").size(10))
                    .on_press(FileBrowserMessage::LoadToDeck(3))
                    .padding(4),
            ]
            .spacing(5)
        } else {
            row![text("Select a file to load").size(10)]
        };

        let content = column![
            text("FILES").size(12),
            header,
            scrollable_list,
            load_buttons,
        ]
        .spacing(5)
        .padding(10);

        container(content)
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// View for a single file entry
    fn view_entry(&self, idx: usize, entry: &FileEntry) -> Element<FileBrowserMessage> {
        let is_selected = self.selected_index == Some(idx);

        let icon = match entry.entry_type {
            FileEntryType::Directory => "📁",
            FileEntryType::AudioFile => "🎵",
        };

        let label = text(format!("{} {}", icon, entry.name))
            .size(12);

        let btn = button(label)
            .width(Fill)
            .padding(4)
            .style(if is_selected {
                button::primary
            } else {
                button::secondary
            });

        // Single click selects, double-click navigates into directories
        let btn = match entry.entry_type {
            FileEntryType::Directory => {
                if is_selected {
                    btn.on_press(FileBrowserMessage::NavigateInto(idx))
                } else {
                    btn.on_press(FileBrowserMessage::Select(idx))
                }
            }
            FileEntryType::AudioFile => {
                btn.on_press(FileBrowserMessage::Select(idx))
            }
        };

        btn.into()
    }
}

impl Default for FileBrowserView {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a path is a supported audio file
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext_lower = ext.to_lowercase();
            ext_lower == "wav" || ext_lower == "rf64"
        })
        .unwrap_or(false)
}
