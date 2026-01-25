//! Tree view widget for hierarchical navigation
//!
//! A collapsible tree widget for displaying folder/playlist hierarchies.
//! Follows iced 0.14 patterns with state structs and view functions.
//!
//! ## Usage
//!
//! ```ignore
//! let tree = tree_view(
//!     &tree_nodes,
//!     &tree_state,
//!     |msg| Message::Tree(msg),
//! );
//! ```

use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Point, Theme};
use std::collections::HashSet;
use std::hash::Hash;

/// Icon type for tree nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeIcon {
    /// Generic folder
    Folder,
    /// Open folder
    FolderOpen,
    /// Playlist
    Playlist,
    /// Open playlist
    PlaylistOpen,
    /// Audio track
    Track,
    /// Collection (special folder)
    Collection,
}

impl TreeIcon {
    /// Get the emoji character for this icon
    pub fn as_char(&self) -> &'static str {
        match self {
            TreeIcon::Folder | TreeIcon::FolderOpen => "\u{1F4C1}",  // 📁
            TreeIcon::Playlist | TreeIcon::PlaylistOpen => "\u{1F4CB}",  // 📋
            TreeIcon::Track => "\u{1F3B5}",  // 🎵
            TreeIcon::Collection => "\u{1F4BF}",  // 💿
        }
    }
}

/// A node in the tree display
#[derive(Debug, Clone)]
pub struct TreeNode<Id: Clone> {
    /// Unique identifier for this node
    pub id: Id,
    /// Display label
    pub label: String,
    /// Icon to show
    pub icon: TreeIcon,
    /// Child nodes (empty for leaf nodes)
    pub children: Vec<TreeNode<Id>>,
    /// Whether this node allows creating children (shows "+" button)
    pub allow_create_child: bool,
    /// Whether this node can be renamed
    pub allow_rename: bool,
}

impl<Id: Clone> TreeNode<Id> {
    /// Create a new tree node
    pub fn new(id: Id, label: impl Into<String>, icon: TreeIcon) -> Self {
        Self {
            id,
            label: label.into(),
            icon,
            children: Vec::new(),
            allow_create_child: false,
            allow_rename: false,
        }
    }

    /// Create a new tree node with children
    pub fn with_children(
        id: Id,
        label: impl Into<String>,
        icon: TreeIcon,
        children: Vec<TreeNode<Id>>,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            icon,
            children,
            allow_create_child: false,
            allow_rename: false,
        }
    }

    /// Set whether this node allows creating children
    pub fn with_create_child(mut self, allow: bool) -> Self {
        self.allow_create_child = allow;
        self
    }

    /// Set whether this node can be renamed
    pub fn with_rename(mut self, allow: bool) -> Self {
        self.allow_rename = allow;
        self
    }

    /// Check if this node has children
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }
}

/// State for the tree widget
#[derive(Debug, Clone)]
pub struct TreeState<Id: Clone + Eq + Hash> {
    /// Set of expanded node IDs
    pub expanded: HashSet<Id>,
    /// Currently selected node ID
    pub selected: Option<Id>,
    /// Node currently being edited (inline rename)
    pub editing: Option<Id>,
    /// Buffer for the edited name
    pub edit_buffer: String,
    /// Last known mouse position (for context menu placement)
    pub last_mouse_position: Point,
}

impl<Id: Clone + Eq + Hash> Default for TreeState<Id> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Id: Clone + Eq + Hash> TreeState<Id> {
    /// Create a new empty tree state
    pub fn new() -> Self {
        Self {
            expanded: HashSet::new(),
            selected: None,
            editing: None,
            edit_buffer: String::new(),
            last_mouse_position: Point::ORIGIN,
        }
    }

    /// Update the last known mouse position
    pub fn set_mouse_position(&mut self, position: Point) {
        self.last_mouse_position = position;
    }

    /// Toggle the expanded state of a node
    pub fn toggle_expanded(&mut self, id: &Id) {
        if self.expanded.contains(id) {
            self.expanded.remove(id);
        } else {
            self.expanded.insert(id.clone());
        }
    }

    /// Expand a node
    pub fn expand(&mut self, id: Id) {
        self.expanded.insert(id);
    }

    /// Collapse a node
    pub fn collapse(&mut self, id: &Id) {
        self.expanded.remove(id);
    }

    /// Select a node
    pub fn select(&mut self, id: Id) {
        self.selected = Some(id);
    }

    /// Clear selection
    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    /// Check if a node is expanded
    pub fn is_expanded(&self, id: &Id) -> bool {
        self.expanded.contains(id)
    }

    /// Check if a node is selected
    pub fn is_selected(&self, id: &Id) -> bool {
        self.selected.as_ref() == Some(id)
    }

    /// Check if a node is being edited
    pub fn is_editing(&self, id: &Id) -> bool {
        self.editing.as_ref() == Some(id)
    }

    /// Start editing a node
    pub fn start_edit(&mut self, id: Id, current_name: String) {
        self.editing = Some(id);
        self.edit_buffer = current_name;
    }

    /// Cancel editing
    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.edit_buffer.clear();
    }

    /// Commit edit and return the new name (clears editing state)
    pub fn commit_edit(&mut self) -> Option<(Id, String)> {
        if let Some(id) = self.editing.take() {
            let name = std::mem::take(&mut self.edit_buffer);
            Some((id, name))
        } else {
            None
        }
    }
}

/// Messages emitted by the tree widget
#[derive(Debug, Clone)]
pub enum TreeMessage<Id> {
    /// Toggle expand/collapse state of a node
    Toggle(Id),
    /// Select a node
    Select(Id),
    /// Create a child node in this folder
    CreateChild(Id),
    /// Start inline editing of a node's name
    StartEdit(Id),
    /// Update the edit buffer
    EditChanged(String),
    /// Commit the edit (save)
    CommitEdit,
    /// Cancel the edit
    CancelEdit,
    /// Mouse released over node (for drop detection)
    DropReceived(Id),
    /// Right-click on a node (for context menu)
    /// Contains the node ID and cursor position for menu placement
    RightClick(Id, Point),
    /// Mouse moved over a node (for tracking cursor position)
    MouseMoved(Point),
}

/// Build a tree view from nodes
///
/// # Arguments
///
/// * `nodes` - Root-level tree nodes to display
/// * `state` - Current tree state (expanded nodes, selection)
/// * `on_message` - Callback to convert tree messages to your message type
///
/// # Example
///
/// ```ignore
/// tree_view(
///     &nodes,
///     &state,
///     |msg| match msg {
///         TreeMessage::Toggle(id) => Message::TreeToggle(id),
///         TreeMessage::Select(id) => Message::TreeSelect(id),
///     },
/// )
/// ```
pub fn tree_view<'a, Id, Message>(
    nodes: &'a [TreeNode<Id>],
    state: &'a TreeState<Id>,
    on_message: impl Fn(TreeMessage<Id>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    Id: Clone + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    let content = build_tree_rows(nodes, state, &on_message, 0);

    scrollable(column(content).spacing(1).padding(Padding::from([4, 8])))
        .height(Length::Fill)
        .into()
}

/// Recursively build tree rows for display
fn build_tree_rows<'a, Id, Message>(
    nodes: &'a [TreeNode<Id>],
    state: &'a TreeState<Id>,
    on_message: &(impl Fn(TreeMessage<Id>) -> Message + 'a + Clone),
    depth: usize,
) -> Vec<Element<'a, Message>>
where
    Id: Clone + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    let mut elements = Vec::new();

    for node in nodes {
        let indent = Space::new().width(Length::Fixed((depth * 16) as f32));
        let is_expanded = state.is_expanded(&node.id);
        let is_selected = state.is_selected(&node.id);
        let is_editing = state.is_editing(&node.id);

        // Expand/collapse arrow button
        let arrow: Element<'a, Message> = if node.has_children() {
            let arrow_text = if is_expanded { "\u{25BC}" } else { "\u{25B6}" }; // ▼ or ▶
            let id_clone = node.id.clone();
            let on_msg = on_message.clone();

            button(text(arrow_text).size(10))
                .padding(Padding::from([2, 4]))
                .style(|theme: &Theme, _status| {
                    let palette = theme.extended_palette();
                    button::Style {
                        background: Some(Background::Color(Color::TRANSPARENT)),
                        text_color: palette.background.base.text,
                        border: Border::default(),
                        ..Default::default()
                    }
                })
                .on_press(on_msg(TreeMessage::Toggle(id_clone)))
                .into()
        } else {
            Space::new().width(Length::Fixed(22.0)).into()
        };

        // Icon
        let icon = node.icon.as_char();

        // Label: either text input (editing) or button (normal)
        let label_widget: Element<'a, Message> = if is_editing {
            // Editing mode: show text input
            let on_msg_input = on_message.clone();
            let on_msg_submit = on_message.clone();

            row![
                text(icon).size(14),
                text_input("", &state.edit_buffer)
                    .on_input(move |s| on_msg_input(TreeMessage::EditChanged(s)))
                    .on_submit(on_msg_submit(TreeMessage::CommitEdit))
                    .size(12)
                    .width(Length::Fixed(120.0))
                    .padding(2),
            ]
            .spacing(6)
            .into()
        } else {
            // Normal mode: clickable label button
            let id_clone = node.id.clone();
            let on_msg = on_message.clone();

            // Use Wrapping::None + clip to truncate long labels without overlap
            let label_text = text(&node.label)
                .size(12)
                .wrapping(iced::widget::text::Wrapping::None);

            let label_content = row![
                text(icon).size(14),
                container(label_text).clip(true),
            ]
            .spacing(6);

            let label_btn = button(label_content)
                .padding(Padding::from([3, 6]))
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
                        border: Border {
                            radius: 4.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .on_press(on_msg(TreeMessage::Select(id_clone)));

            // Wrap in mouse_area for double-click to edit (if allowed)
            if node.allow_rename {
                let id_edit = node.id.clone();
                let on_msg_edit = on_message.clone();
                mouse_area(label_btn)
                    .on_double_click(on_msg_edit(TreeMessage::StartEdit(id_edit)))
                    .into()
            } else {
                label_btn.into()
            }
        };

        // Create child button ("+") if allowed
        let create_btn: Option<Element<'a, Message>> = if node.allow_create_child {
            let id_create = node.id.clone();
            let on_msg_create = on_message.clone();

            Some(
                button(text("+").size(12))
                    .padding(Padding::from([2, 6]))
                    .style(|theme: &Theme, status| {
                        let palette = theme.extended_palette();
                        let bg = match status {
                            button::Status::Hovered => palette.primary.weak.color,
                            _ => Color::TRANSPARENT,
                        };
                        button::Style {
                            background: Some(Background::Color(bg)),
                            text_color: palette.background.base.text,
                            border: Border {
                                radius: 4.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(on_msg_create(TreeMessage::CreateChild(id_create)))
                    .into(),
            )
        } else {
            None
        };

        // Build row with optional create button
        let row_content = if let Some(create_btn) = create_btn {
            row![indent, arrow, label_widget, create_btn]
                .spacing(2)
                .align_y(iced::Alignment::Center)
        } else {
            row![indent, arrow, label_widget]
                .spacing(2)
                .align_y(iced::Alignment::Center)
        };

        // Wrap row in mouse_area for various mouse events
        let id_drop = node.id.clone();
        let id_right_click = node.id.clone();
        let on_msg_drop = on_message.clone();
        let on_msg_move = on_message.clone();
        let on_msg_right = on_message.clone();

        // Use cached mouse position for right-click menu placement
        let cached_position = state.last_mouse_position;

        // mouse_area handles:
        // - on_release: Drop detection (when drag ends over this node)
        // - on_move: Track cursor position for context menu placement
        // - on_right_press: Show context menu
        let row_with_events = mouse_area(row_content)
            .on_release(on_msg_drop(TreeMessage::DropReceived(id_drop)))
            .on_move(move |point| on_msg_move(TreeMessage::MouseMoved(point)))
            .on_right_press(on_msg_right(TreeMessage::RightClick(id_right_click, cached_position)));

        elements.push(row_with_events.into());

        // Recursively add children if expanded
        if is_expanded && node.has_children() {
            let child_elements = build_tree_rows(&node.children, state, on_message, depth + 1);
            elements.extend(child_elements);
        }
    }

    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_state() {
        let mut state: TreeState<String> = TreeState::new();

        // Test expand/collapse
        state.expand("folder1".to_string());
        assert!(state.is_expanded(&"folder1".to_string()));

        state.toggle_expanded(&"folder1".to_string());
        assert!(!state.is_expanded(&"folder1".to_string()));

        // Test selection
        state.select("item1".to_string());
        assert!(state.is_selected(&"item1".to_string()));
        assert!(!state.is_selected(&"item2".to_string()));

        state.clear_selection();
        assert!(!state.is_selected(&"item1".to_string()));
    }

    #[test]
    fn test_tree_node() {
        let leaf = TreeNode::new("leaf", "Leaf", TreeIcon::Track);
        assert!(!leaf.has_children());

        let folder = TreeNode::with_children(
            "folder",
            "Folder",
            TreeIcon::Folder,
            vec![leaf.clone()],
        );
        assert!(folder.has_children());
    }
}
