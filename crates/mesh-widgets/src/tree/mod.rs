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

use iced::widget::{button, column, row, scrollable, text, horizontal_space};
use iced::{Background, Border, Color, Element, Length, Padding, Theme};
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
}

impl<Id: Clone> TreeNode<Id> {
    /// Create a new tree node
    pub fn new(id: Id, label: impl Into<String>, icon: TreeIcon) -> Self {
        Self {
            id,
            label: label.into(),
            icon,
            children: Vec::new(),
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
        }
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
        }
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
}

/// Messages emitted by the tree widget
#[derive(Debug, Clone)]
pub enum TreeMessage<Id> {
    /// Toggle expand/collapse state of a node
    Toggle(Id),
    /// Select a node
    Select(Id),
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
    Message: 'a,
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
    Message: 'a,
{
    let mut elements = Vec::new();

    for node in nodes {
        let indent = horizontal_space().width(Length::Fixed((depth * 16) as f32));
        let is_expanded = state.is_expanded(&node.id);
        let is_selected = state.is_selected(&node.id);

        // Expand/collapse arrow button
        let arrow: Element<'a, Message> = if node.has_children() {
            let arrow_text = if is_expanded { "\u{25BC}" } else { "\u{25B6}" }; // ▼ or ▶
            let id_clone = node.id.clone();
            let on_msg = on_message.clone();

            button(text(arrow_text).size(10))
                .padding(Padding::from([2, 4]))
                .style(|theme: &Theme, status| {
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
            horizontal_space().width(Length::Fixed(22.0)).into()
        };

        // Icon
        let icon = node.icon.as_char();

        // Label button (clickable for selection)
        let id_clone = node.id.clone();
        let on_msg = on_message.clone();

        let label_content = row![text(icon).size(14), text(&node.label).size(12),].spacing(6);

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

        let row_content = row![indent, arrow, label_btn].spacing(2).align_y(iced::Alignment::Center);

        elements.push(row_content.into());

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
