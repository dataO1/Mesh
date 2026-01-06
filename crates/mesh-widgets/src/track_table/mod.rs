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

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Background, Border, Color, Element, Length, Padding, Theme};

/// Column types for the track table
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackColumn {
    /// Track name
    Name,
    /// BPM (beats per minute)
    Bpm,
    /// Musical key
    Key,
    /// Track duration
    Duration,
}

impl TrackColumn {
    /// Get the display label for this column
    pub fn label(&self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Bpm => "BPM",
            Self::Key => "Key",
            Self::Duration => "Duration",
        }
    }

    /// Get the width for this column
    pub fn width(&self) -> Length {
        match self {
            Self::Name => Length::Fill,
            Self::Bpm => Length::Fixed(60.0),
            Self::Key => Length::Fixed(50.0),
            Self::Duration => Length::Fixed(70.0),
        }
    }

    /// Get all columns in display order
    pub fn all() -> &'static [TrackColumn] {
        &[
            TrackColumn::Name,
            TrackColumn::Bpm,
            TrackColumn::Key,
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
    /// BPM if known
    pub bpm: Option<f64>,
    /// Musical key if known
    pub key: Option<String>,
    /// Duration in seconds if known
    pub duration: Option<f64>,
}

impl<Id: Clone> TrackRow<Id> {
    /// Create a new track row
    pub fn new(id: Id, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            bpm: None,
            key: None,
            duration: None,
        }
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
}

/// State for the track table widget
#[derive(Debug, Clone)]
pub struct TrackTableState<Id: Clone> {
    /// Current search query
    pub search_query: String,
    /// Currently selected track ID
    pub selected: Option<Id>,
    /// Column to sort by
    pub sort_column: TrackColumn,
    /// Sort direction (true = ascending)
    pub sort_ascending: bool,
}

impl<Id: Clone> Default for TrackTableState<Id> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Id: Clone> TrackTableState<Id> {
    /// Create a new table state with default values
    pub fn new() -> Self {
        Self {
            search_query: String::new(),
            selected: None,
            sort_column: TrackColumn::Name,
            sort_ascending: true,
        }
    }

    /// Set the search query
    pub fn set_search(&mut self, query: String) {
        self.search_query = query;
    }

    /// Select a track
    pub fn select(&mut self, id: Id) {
        self.selected = Some(id);
    }

    /// Clear selection
    pub fn clear_selection(&mut self) {
        self.selected = None;
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
    pub fn is_selected(&self, id: &Id) -> bool
    where
        Id: PartialEq,
    {
        self.selected.as_ref() == Some(id)
    }
}

/// Messages emitted by the track table widget
#[derive(Debug, Clone)]
pub enum TrackTableMessage<Id> {
    /// Search query changed
    SearchChanged(String),
    /// Track selected (single click)
    Select(Id),
    /// Track activated (double click)
    Activate(Id),
    /// Sort by column clicked
    SortBy(TrackColumn),
}

/// Build a track table view
///
/// # Arguments
///
/// * `tracks` - Tracks to display
/// * `state` - Current table state (search, selection, sort)
/// * `on_message` - Callback to convert table messages to your message type
pub fn track_table<'a, Id, Message>(
    tracks: &'a [TrackRow<Id>],
    state: &'a TrackTableState<Id>,
    on_message: impl Fn(TrackTableMessage<Id>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    Id: Clone + PartialEq + 'a,
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
    let filtered: Vec<_> = tracks
        .iter()
        .filter(|t| {
            state.search_query.is_empty()
                || t.name
                    .to_lowercase()
                    .contains(&state.search_query.to_lowercase())
        })
        .collect();

    // Track rows
    let rows: Vec<Element<'a, Message>> = filtered
        .iter()
        .map(|track| build_track_row(track, state, on_message.clone()))
        .collect();

    let track_list = if rows.is_empty() {
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
    Id: Clone + 'a,
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
    Id: Clone + 'a,
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

/// Build a single track row
fn build_track_row<'a, Id, Message>(
    track: &'a TrackRow<Id>,
    state: &'a TrackTableState<Id>,
    on_message: impl Fn(TrackTableMessage<Id>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    Id: Clone + PartialEq + 'a,
    Message: Clone + 'a,
{
    let is_selected = state.is_selected(&track.id);
    let id = track.id.clone();
    let on_msg = on_message.clone();

    let row_content = row![
        text(&track.name)
            .size(12)
            .width(TrackColumn::Name.width()),
        text(track.format_bpm())
            .size(12)
            .width(TrackColumn::Bpm.width()),
        text(track.key.as_deref().unwrap_or("-"))
            .size(12)
            .width(TrackColumn::Key.width()),
        text(track.format_duration())
            .size(12)
            .width(TrackColumn::Duration.width()),
    ]
    .spacing(1)
    .padding(Padding::from([4, 8]));

    button(row_content)
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
        })
        .on_press(on_msg(TrackTableMessage::Select(id)))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_row_formatting() {
        let row = TrackRow::new("1", "Test Track")
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
        assert_eq!(state.sort_column, TrackColumn::Name);
        assert!(state.sort_ascending);

        state.set_sort(TrackColumn::Name);
        assert!(!state.sort_ascending); // Toggled

        state.set_sort(TrackColumn::Bpm);
        assert_eq!(state.sort_column, TrackColumn::Bpm);
        assert!(state.sort_ascending); // Reset for new column
    }
}
