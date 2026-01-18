//! Track table widget for displaying audio tracks
//!
//! A sortable, filterable table for displaying track metadata with search.
//! Follows iced 0.14 patterns with state structs and view functions.
//!
//! ## Usage
//!
//! ```ignore
//! let table = track_table(
//!     &tracks,
//!     &table_state,
//!     |msg| Message::Table(msg),
//! );
//! ```

use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input, Id};
use iced::{Background, Border, Color, Element, Length, Padding, Point, Theme};
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::LazyLock;

/// Row height for track table rows (used for scroll calculations)
pub const TRACK_ROW_HEIGHT: f32 = 28.0;

/// Scrollable ID for the track table (used for programmatic scrolling)
pub static TRACK_TABLE_SCROLLABLE_ID: LazyLock<Id> = LazyLock::new(Id::unique);

/// Modifier keys held during a selection click
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectModifiers {
    /// Shift key held (for range selection)
    pub shift: bool,
    /// Ctrl/Cmd key held (for toggle selection)
    pub ctrl: bool,
}

/// Column types for the track table
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackColumn {
    /// Order in playlist/collection (#)
    Order,
    /// Track name
    Name,
    /// Artist name
    Artist,
    /// BPM (beats per minute)
    Bpm,
    /// Musical key
    Key,
    /// Track duration
    Duration,
    /// LUFS loudness
    Lufs,
}

impl TrackColumn {
    /// Get the display label for this column
    pub fn label(&self) -> &'static str {
        match self {
            Self::Order => "#",
            Self::Name => "Name",
            Self::Artist => "Artist",
            Self::Bpm => "BPM",
            Self::Key => "Key",
            Self::Duration => "Duration",
            Self::Lufs => "LUFS",
        }
    }

    /// Check if this column is editable
    pub fn is_editable(&self) -> bool {
        matches!(self, Self::Artist | Self::Bpm | Self::Key)
    }

    /// Get the width for this column
    pub fn width(&self) -> Length {
        match self {
            Self::Order => Length::Fixed(35.0),
            Self::Name => Length::Fill,
            Self::Artist => Length::Fixed(120.0),
            Self::Bpm => Length::Fixed(60.0),
            Self::Key => Length::Fixed(50.0),
            Self::Duration => Length::Fixed(70.0),
            Self::Lufs => Length::Fixed(55.0),
        }
    }

    /// Get all columns in display order
    pub fn all() -> &'static [TrackColumn] {
        &[
            TrackColumn::Order,
            TrackColumn::Name,
            TrackColumn::Artist,
            TrackColumn::Bpm,
            TrackColumn::Key,
            TrackColumn::Lufs,
            TrackColumn::Duration,
        ]
    }
}

/// A row in the track table
#[derive(Debug, Clone)]
pub struct TrackRow<Id: Clone> {
    /// Unique identifier for this track
    pub id: Id,
    /// Track name (usually filename without extension)
    pub name: String,
    /// Order in playlist/collection (1-based for display as #)
    pub order: i32,
    /// Artist name if known
    pub artist: Option<String>,
    /// BPM if known
    pub bpm: Option<f64>,
    /// Musical key if known
    pub key: Option<String>,
    /// Duration in seconds if known
    pub duration: Option<f64>,
    /// LUFS loudness if known
    pub lufs: Option<f32>,
}

impl<Id: Clone> TrackRow<Id> {
    /// Create a new track row with order
    pub fn new(id: Id, name: impl Into<String>, order: i32) -> Self {
        Self {
            id,
            name: name.into(),
            order,
            artist: None,
            bpm: None,
            key: None,
            duration: None,
            lufs: None,
        }
    }

    /// Set the artist name
    pub fn with_artist(mut self, artist: impl Into<String>) -> Self {
        self.artist = Some(artist.into());
        self
    }

    /// Set the BPM
    pub fn with_bpm(mut self, bpm: f64) -> Self {
        self.bpm = Some(bpm);
        self
    }

    /// Set the musical key
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Set the duration in seconds
    pub fn with_duration(mut self, duration: f64) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Set the LUFS loudness
    pub fn with_lufs(mut self, lufs: f32) -> Self {
        self.lufs = Some(lufs);
        self
    }

    /// Format duration as MM:SS
    pub fn format_duration(&self) -> String {
        self.duration
            .map(|d| {
                let mins = (d / 60.0) as u32;
                let secs = (d % 60.0) as u32;
                format!("{}:{:02}", mins, secs)
            })
            .unwrap_or_else(|| "--:--".to_string())
    }

    /// Format BPM with one decimal
    pub fn format_bpm(&self) -> String {
        self.bpm
            .map(|b| format!("{:.1}", b))
            .unwrap_or_else(|| "-".to_string())
    }

    /// Format LUFS with one decimal
    pub fn format_lufs(&self) -> String {
        self.lufs
            .map(|l| format!("{:.1}", l))
            .unwrap_or_else(|| "-".to_string())
    }
}

/// Sort a slice of tracks by a specific column and direction
///
/// This should be called when sort state changes to keep track data in sync.
pub fn sort_tracks<Id: Clone>(tracks: &mut [TrackRow<Id>], column: TrackColumn, ascending: bool) {
    tracks.sort_by(|a, b| {
        let cmp = compare_tracks_by_column(a, b, column);
        if ascending {
            cmp
        } else {
            cmp.reverse()
        }
    });
}

/// Compare two tracks by a specific column
///
/// Used for sorting tracks. None values are sorted to the end.
pub fn compare_tracks_by_column<Id: Clone>(
    a: &TrackRow<Id>,
    b: &TrackRow<Id>,
    column: TrackColumn,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match column {
        TrackColumn::Order => a.order.cmp(&b.order),
        TrackColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        TrackColumn::Artist => {
            let a_artist = a.artist.as_deref().unwrap_or("");
            let b_artist = b.artist.as_deref().unwrap_or("");
            a_artist.to_lowercase().cmp(&b_artist.to_lowercase())
        }
        TrackColumn::Bpm => {
            // Sort None values to the end
            match (a.bpm, b.bpm) {
                (Some(a_bpm), Some(b_bpm)) => a_bpm.partial_cmp(&b_bpm).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        }
        TrackColumn::Key => {
            let a_key = a.key.as_deref().unwrap_or("");
            let b_key = b.key.as_deref().unwrap_or("");
            a_key.cmp(b_key)
        }
        TrackColumn::Duration => {
            match (a.duration, b.duration) {
                (Some(a_dur), Some(b_dur)) => a_dur.partial_cmp(&b_dur).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        }
        TrackColumn::Lufs => {
            match (a.lufs, b.lufs) {
                (Some(a_lufs), Some(b_lufs)) => a_lufs.partial_cmp(&b_lufs).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        }
    }
}

/// State for the track table widget
#[derive(Debug, Clone)]
pub struct TrackTableState<Id: Clone + Eq + Hash> {
    /// Current search query
    pub search_query: String,
    /// Currently selected track IDs (supports multi-selection)
    pub selected: HashSet<Id>,
    /// Anchor point for Shift+click range selection
    pub anchor: Option<Id>,
    /// Most recently selected track (for keyboard navigation)
    pub last_selected: Option<Id>,
    /// Column to sort by
    pub sort_column: TrackColumn,
    /// Sort direction (true = ascending)
    pub sort_ascending: bool,
    /// Currently editing cell (track ID, column)
    pub editing: Option<(Id, TrackColumn)>,
    /// Buffer for the value being edited
    pub edit_buffer: String,
    /// Last known mouse position (for context menu placement)
    pub last_mouse_position: Point,
}

impl<Id: Clone + Eq + Hash> Default for TrackTableState<Id> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Id: Clone + Eq + Hash> TrackTableState<Id> {
    /// Create a new table state with default values
    pub fn new() -> Self {
        Self {
            search_query: String::new(),
            selected: HashSet::new(),
            anchor: None,
            last_selected: None,
            sort_column: TrackColumn::Order,
            sort_ascending: true,
            editing: None,
            edit_buffer: String::new(),
            last_mouse_position: Point::ORIGIN,
        }
    }

    /// Update the last known mouse position
    pub fn set_mouse_position(&mut self, position: Point) {
        self.last_mouse_position = position;
    }

    /// Set the search query
    pub fn set_search(&mut self, query: String) {
        self.search_query = query;
    }

    /// Select a single track (clears other selections)
    /// Used for normal clicks without modifiers
    pub fn select(&mut self, id: Id) {
        self.selected.clear();
        self.selected.insert(id.clone());
        self.anchor = Some(id.clone());
        self.last_selected = Some(id);
    }

    /// Toggle selection of a track (Ctrl+click)
    /// Adds or removes from selection without affecting others
    pub fn toggle_select(&mut self, id: Id) {
        if self.selected.contains(&id) {
            self.selected.remove(&id);
        } else {
            self.selected.insert(id.clone());
        }
        self.last_selected = Some(id);
    }

    /// Select a range of tracks (Shift+click)
    /// Selects all tracks between anchor and target
    pub fn select_range(&mut self, id: Id, all_ids: &[Id]) {
        if let Some(ref anchor) = self.anchor {
            let anchor_idx = all_ids.iter().position(|x| x == anchor);
            let target_idx = all_ids.iter().position(|x| x == &id);

            if let (Some(a), Some(t)) = (anchor_idx, target_idx) {
                let (start, end) = if a <= t { (a, t) } else { (t, a) };
                for i in start..=end {
                    self.selected.insert(all_ids[i].clone());
                }
            }
        } else {
            // No anchor - just select this one
            self.select(id.clone());
        }
        self.last_selected = Some(id);
    }

    /// Handle a selection with modifiers
    /// Dispatches to appropriate selection method based on modifiers
    pub fn handle_select(&mut self, id: Id, modifiers: SelectModifiers, all_ids: &[Id]) {
        if modifiers.shift {
            self.select_range(id, all_ids);
        } else if modifiers.ctrl {
            self.toggle_select(id);
        } else {
            self.select(id);
        }
    }

    /// Clear all selections
    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.last_selected = None;
    }

    /// Check if any tracks are selected
    pub fn has_selection(&self) -> bool {
        !self.selected.is_empty()
    }

    /// Get all selected track IDs
    pub fn selected_ids(&self) -> &HashSet<Id> {
        &self.selected
    }

    /// Get selected IDs as a Vec (useful for iteration with ordering)
    pub fn selected_vec(&self) -> Vec<Id> {
        self.selected.iter().cloned().collect()
    }

    /// Set sort column (toggles direction if same column)
    pub fn set_sort(&mut self, column: TrackColumn) {
        if self.sort_column == column {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_column = column;
            self.sort_ascending = true;
        }
    }

    /// Check if a track is selected
    pub fn is_selected(&self, id: &Id) -> bool {
        self.selected.contains(id)
    }

    /// Start editing a cell
    pub fn start_edit(&mut self, id: Id, column: TrackColumn, current_value: String) {
        self.editing = Some((id, column));
        self.edit_buffer = current_value;
    }

    /// Check if a specific cell is being edited
    pub fn is_editing(&self, id: &Id, column: TrackColumn) -> bool {
        self.editing
            .as_ref()
            .map(|(edit_id, edit_col)| edit_id == id && *edit_col == column)
            .unwrap_or(false)
    }

    /// Cancel editing
    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.edit_buffer.clear();
    }

    /// Commit edit and return the edited data (clears editing state)
    pub fn commit_edit(&mut self) -> Option<(Id, TrackColumn, String)> {
        if let Some((id, column)) = self.editing.take() {
            let value = std::mem::take(&mut self.edit_buffer);
            Some((id, column, value))
        } else {
            None
        }
    }
}

/// Messages emitted by the track table widget
#[derive(Debug, Clone)]
pub enum TrackTableMessage<Id> {
    /// Search query changed
    SearchChanged(String),
    /// Track selected (single click)
    /// Note: Modifier handling (Shift/Ctrl) is done in the app's update() handler
    /// where current keyboard state is available, NOT in the widget's view().
    Select(Id),
    /// Track activated (double click)
    Activate(Id),
    /// Sort by column clicked
    SortBy(TrackColumn),
    /// Start editing a cell (double-click on editable cell)
    StartEdit(Id, TrackColumn, String),
    /// Edit buffer changed
    EditChanged(String),
    /// Commit edit (Enter pressed or focus lost)
    CommitEdit,
    /// Cancel edit (Escape pressed)
    CancelEdit,
    /// Mouse released over track row (for drop detection)
    /// Used to detect when dragged tracks are dropped onto this track/folder
    DropReceived(Id),
    /// Right-click on a track (for context menu)
    /// Contains the track ID and cursor position for menu placement
    RightClick(Id, Point),
    /// Mouse moved over a track row (for tracking cursor position)
    /// Used internally to cache position for right-click context menus
    MouseMoved(Point),
}

/// Build a track table view
///
/// # Arguments
///
/// * `tracks` - Tracks to display
/// * `state` - Current table state (search, selection, sort)
/// * `on_message` - Callback to convert table messages to your message type
///
/// Note: Modifier key handling (Shift/Ctrl for multi-select) is NOT done here.
/// The `Select` message should be handled in the app's `update()` where
/// current keyboard state is available.
pub fn track_table<'a, Id, Message>(
    tracks: &'a [TrackRow<Id>],
    state: &'a TrackTableState<Id>,
    on_message: impl Fn(TrackTableMessage<Id>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    Id: Clone + PartialEq + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    let on_msg = on_message.clone();

    // Search bar
    let search = text_input("Search tracks...", &state.search_query)
        .on_input(move |s| on_msg(TrackTableMessage::SearchChanged(s)))
        .padding(8)
        .size(13);

    // Column headers
    let headers = build_headers(state, on_message.clone());

    // Filter tracks by search query
    let mut filtered: Vec<_> = tracks
        .iter()
        .filter(|t| {
            state.search_query.is_empty()
                || t.name
                    .to_lowercase()
                    .contains(&state.search_query.to_lowercase())
        })
        .collect();

    // Sort tracks by selected column
    filtered.sort_by(|a, b| {
        let cmp = compare_tracks_by_column(a, b, state.sort_column);
        if state.sort_ascending {
            cmp
        } else {
            cmp.reverse()
        }
    });

    // Track rows
    let rows: Vec<Element<'a, Message>> = filtered
        .iter()
        .map(|track| build_track_row(track, state, on_message.clone()))
        .collect();

    let track_list: Element<'a, Message> = if rows.is_empty() {
        let empty_msg = if state.search_query.is_empty() {
            "No tracks in this folder"
        } else {
            "No matching tracks"
        };
        container(
            text(empty_msg)
                .size(12)
                .style(|theme: &Theme| text::Style {
                    color: Some(theme.extended_palette().background.weak.text),
                }),
        )
        .padding(20)
        .center_x(Length::Fill)
        .into()
    } else {
        scrollable(column(rows).spacing(1))
            .id(TRACK_TABLE_SCROLLABLE_ID.clone())
            .height(Length::Fill)
            .into()
    };

    column![
        search,
        container(headers).style(|theme: &Theme| {
            container::Style {
                background: Some(Background::Color(
                    theme.extended_palette().background.weak.color,
                )),
                ..Default::default()
            }
        }),
        track_list,
    ]
    .spacing(2)
    .into()
}

/// Build column headers row
fn build_headers<'a, Id, Message>(
    state: &'a TrackTableState<Id>,
    on_message: impl Fn(TrackTableMessage<Id>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    Id: Clone + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    let headers: Vec<Element<'a, Message>> = TrackColumn::all()
        .iter()
        .map(|&col| build_header_cell(col, state, on_message.clone()))
        .collect();

    row(headers)
        .spacing(1)
        .padding(Padding::from([6, 8]))
        .into()
}

/// Build a single column header cell
fn build_header_cell<'a, Id, Message>(
    column: TrackColumn,
    state: &'a TrackTableState<Id>,
    on_message: impl Fn(TrackTableMessage<Id>) -> Message + 'a,
) -> Element<'a, Message>
where
    Id: Clone + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    let is_sorted = state.sort_column == column;
    let arrow = if is_sorted {
        if state.sort_ascending {
            " \u{25B2}" // ▲
        } else {
            " \u{25BC}" // ▼
        }
    } else {
        ""
    };

    let label = format!("{}{}", column.label(), arrow);

    button(text(label).size(11))
        .padding(Padding::from([2, 4]))
        .width(column.width())
        .style(|theme: &Theme, _status| {
            let palette = theme.extended_palette();
            button::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                text_color: palette.background.base.text,
                border: Border::default(),
                ..Default::default()
            }
        })
        .on_press(on_message(TrackTableMessage::SortBy(column)))
        .into()
}

/// Build a cell for a track row (handles editing state)
fn build_cell<'a, Id, Message>(
    track: &'a TrackRow<Id>,
    column: TrackColumn,
    state: &'a TrackTableState<Id>,
    on_message: impl Fn(TrackTableMessage<Id>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    Id: Clone + PartialEq + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    let is_editing = state.is_editing(&track.id, column);

    // Get the display value for this cell
    let display_value = match column {
        TrackColumn::Order => track.order.to_string(),
        TrackColumn::Name => track.name.clone(),
        TrackColumn::Artist => track.artist.clone().unwrap_or_else(|| "-".to_string()),
        TrackColumn::Bpm => track.format_bpm(),
        TrackColumn::Key => track.key.clone().unwrap_or_else(|| "-".to_string()),
        TrackColumn::Duration => track.format_duration(),
        TrackColumn::Lufs => track.format_lufs(),
    };

    if is_editing {
        // Show text input for editing
        let on_msg_change = on_message.clone();
        let on_msg_submit = on_message.clone();

        text_input("", &state.edit_buffer)
            .on_input(move |s| on_msg_change(TrackTableMessage::EditChanged(s)))
            .on_submit(on_msg_submit(TrackTableMessage::CommitEdit))
            .size(12)
            .padding(2)
            .width(column.width())
            .into()
    } else if column.is_editable() {
        // Editable cell - wrap in mouse_area for double-click to edit
        let id = track.id.clone();
        let on_msg = on_message.clone();
        let current_value = match column {
            TrackColumn::Artist => track.artist.clone().unwrap_or_default(),
            TrackColumn::Bpm => track.bpm.map(|b| format!("{:.1}", b)).unwrap_or_default(),
            TrackColumn::Key => track.key.clone().unwrap_or_default(),
            _ => String::new(),
        };

        mouse_area(
            text(display_value)  // Move ownership to text widget
                .size(12)
                .width(column.width()),
        )
        .on_double_click(on_msg(TrackTableMessage::StartEdit(id, column, current_value)))
        .into()
    } else {
        // Non-editable cell - just display text
        text(display_value)
            .size(12)
            .width(column.width())
            .into()
    }
}

/// Build a single track row
fn build_track_row<'a, Id, Message>(
    track: &'a TrackRow<Id>,
    state: &'a TrackTableState<Id>,
    on_message: impl Fn(TrackTableMessage<Id>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    Id: Clone + PartialEq + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    let is_selected = state.is_selected(&track.id);
    let id = track.id.clone();
    let id_activate = track.id.clone();
    let on_msg = on_message.clone();
    let on_msg_activate = on_message.clone();

    // Check if any cell in this row is being edited
    let is_row_editing = state.editing.as_ref().map(|(id, _)| id == &track.id).unwrap_or(false);

    // Build cells for each column
    let cells: Vec<Element<'a, Message>> = TrackColumn::all()
        .iter()
        .map(|&col| build_cell(track, col, state, on_message.clone()))
        .collect();

    let row_content = row(cells)
        .spacing(1)
        .padding(Padding::from([4, 8]));

    // Button is used for visual styling only - no .on_press()
    // All mouse event handling goes through mouse_area to avoid event consumption
    let row_button = button(row_content)
        .padding(0)
        .width(Length::Fill)
        .style(move |theme: &Theme, status| {
            let palette = theme.extended_palette();
            let bg = if is_selected {
                palette.primary.weak.color
            } else {
                match status {
                    button::Status::Hovered => palette.background.weak.color,
                    _ => Color::TRANSPARENT,
                }
            };
            let text_color = if is_selected {
                palette.primary.weak.text
            } else {
                palette.background.base.text
            };

            button::Style {
                background: Some(Background::Color(bg)),
                text_color,
                border: Border::default(),
                ..Default::default()
            }
        });

    // If we're editing this row, don't add row-level mouse handlers
    // (they would interfere with the text input)
    if is_row_editing {
        row_button.into()
    } else {
        // Clone id for drop detection and right-click
        let id_drop = track.id.clone();
        let id_right_click = track.id.clone();
        let on_msg_drop = on_message.clone();
        let on_msg_right = on_message.clone();
        let on_msg_move = on_message.clone();

        // Use cached mouse position for right-click menu placement
        let cached_position = state.last_mouse_position;

        // mouse_area handles all mouse events:
        // - on_press: Select track (modifier handling is in app's update())
        // - on_double_click: Activate/load track
        // - on_release: Drop detection (when drag ends over this row)
        // - on_move: Track cursor position for context menu placement
        // - on_right_press: Show context menu
        mouse_area(row_button)
            .on_press(on_msg(TrackTableMessage::Select(id)))
            .on_double_click(on_msg_activate(TrackTableMessage::Activate(id_activate)))
            .on_release(on_msg_drop(TrackTableMessage::DropReceived(id_drop)))
            .on_move(move |point| on_msg_move(TrackTableMessage::MouseMoved(point)))
            .on_right_press(on_msg_right(TrackTableMessage::RightClick(id_right_click, cached_position)))
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_row_formatting() {
        let row = TrackRow::new("1", "Test Track", 1)
            .with_bpm(128.5)
            .with_duration(185.0);

        assert_eq!(row.format_bpm(), "128.5");
        assert_eq!(row.format_duration(), "3:05");
    }

    #[test]
    fn test_table_state() {
        let mut state: TrackTableState<String> = TrackTableState::new();

        // Test search
        state.set_search("test".to_string());
        assert_eq!(state.search_query, "test");

        // Test selection
        state.select("track1".to_string());
        assert!(state.is_selected(&"track1".to_string()));
        assert!(!state.is_selected(&"track2".to_string()));

        // Test sort toggle
        assert_eq!(state.sort_column, TrackColumn::Order);
        assert!(state.sort_ascending);

        state.set_sort(TrackColumn::Order);
        assert!(!state.sort_ascending); // Toggled

        state.set_sort(TrackColumn::Bpm);
        assert_eq!(state.sort_column, TrackColumn::Bpm);
        assert!(state.sort_ascending); // Reset for new column
    }
}
