//! Runtime tree for the MIDI learn wizard.
//!
//! Built from the static catalog (`learn_catalog`) and a `TopologyConfig`.
//! Manages the collapsible tree structure, encoder-based navigation,
//! mapping capture state, and action log.

use std::collections::VecDeque;
use mesh_midi::learn_catalog;
use mesh_midi::learn_defs::*;
use mesh_midi::{ControlAddress, HardwareType};

// ---------------------------------------------------------------------------
// Mapping data stored on tree nodes
// ---------------------------------------------------------------------------

/// Status of a mapping slot relative to the existing config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingStatus {
    /// No mapping assigned (empty slot)
    Unmapped,
    /// Mapping loaded from existing config, unmodified
    Existing,
    /// Newly captured mapping during this session
    New,
    /// Was in existing config but re-mapped to a different control
    Changed,
}

/// A captured control assignment for a mapping slot.
#[derive(Debug, Clone)]
pub struct MappedControl {
    /// Protocol-agnostic control address (MIDI note/CC or HID name)
    pub address: ControlAddress,
    /// Detected hardware type (Button, Knob, Encoder, etc.)
    pub hardware_type: HardwareType,
    /// Whether this was captured while shift was held
    pub shift_held: bool,
    /// Source device name (for port matching)
    pub source_device: Option<String>,
}

// ---------------------------------------------------------------------------
// Tree nodes
// ---------------------------------------------------------------------------

/// Type tag for flattened node references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatNodeType {
    Section,
    Mapping,
    Reset,
    Done,
}

/// Runtime tree node.
pub enum TreeNode {
    /// Collapsible section header.
    Section {
        section_id: &'static str,
        label: String,
        deck_index: Option<usize>,
        expanded: bool,
        children: Vec<TreeNode>,
    },
    /// Leaf mapping node.
    Mapping {
        def: &'static MappingDef,
        deck_index: Option<usize>,
        mapped: Option<MappedControl>,
        /// Original mapping from existing config (for diff in verification)
        original: Option<MappedControl>,
        status: MappingStatus,
    },
    /// "Reset All Mappings" action node (only shown when existing config loaded).
    Reset,
    /// Terminal "Done" node at the bottom of the tree.
    Done,
}

impl TreeNode {
    /// Whether this is a section node.
    pub fn is_section(&self) -> bool {
        matches!(self, TreeNode::Section { .. })
    }

    /// Whether this section is expanded (false for non-sections).
    pub fn is_expanded(&self) -> bool {
        matches!(self, TreeNode::Section { expanded: true, .. })
    }

    /// Get the mapping def if this is a Mapping node.
    pub fn mapping_def(&self) -> Option<&'static MappingDef> {
        match self {
            TreeNode::Mapping { def, .. } => Some(def),
            _ => None,
        }
    }

    /// Get the section label.
    pub fn label(&self) -> &str {
        match self {
            TreeNode::Section { label, .. } => label,
            TreeNode::Mapping { def, .. } => def.label,
            TreeNode::Reset => "Reset All Mappings",
            TreeNode::Done => "Done",
        }
    }

    /// Get mapped/total counts for a section.
    pub fn section_progress(&self) -> (usize, usize) {
        match self {
            TreeNode::Section { children, .. } => {
                let mut mapped = 0;
                let mut total = 0;
                for child in children {
                    match child {
                        TreeNode::Section { .. } => {
                            let (m, t) = child.section_progress();
                            mapped += m;
                            total += t;
                        }
                        TreeNode::Mapping { status, .. } => {
                            total += 1;
                            if *status != MappingStatus::Unmapped {
                                mapped += 1;
                            }
                        }
                        TreeNode::Reset | TreeNode::Done => {}
                    }
                }
                (mapped, total)
            }
            _ => (0, 0),
        }
    }
}

// ---------------------------------------------------------------------------
// Flattened node reference (for encoder navigation)
// ---------------------------------------------------------------------------

/// Reference into the flattened visible-node list.
///
/// The tree is flattened depth-first, skipping children of collapsed sections.
/// The encoder scrolls through this flat list.
pub struct FlatNode {
    /// Index path into the tree (e.g., [2, 0, 3] = roots[2].children[0].children[3])
    pub tree_path: Vec<usize>,
    /// Nesting depth (0 = root section, 1 = child section, 2 = leaf)
    pub depth: usize,
    /// Node type for quick dispatch
    pub node_type: FlatNodeType,
}

// ---------------------------------------------------------------------------
// Action log (footer)
// ---------------------------------------------------------------------------

/// Status badge for an action log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStatus {
    /// Executed an already-mapped action
    Mapped,
    /// Captured a new mapping for a tree node
    Captured,
}

/// Single entry in the action log.
#[derive(Debug, Clone)]
pub struct ActionLogEntry {
    /// Raw control display (e.g., "CH1 Note 36", "HID grid_1")
    pub control_display: String,
    /// What happened (e.g., "Play Deck 1" or "captured: Cue Deck 1")
    pub action_name: String,
    /// Status badge
    pub status: LogStatus,
}

/// Ring buffer of recent actions, displayed in the footer.
pub struct ActionLog {
    entries: VecDeque<ActionLogEntry>,
    capacity: usize,
}

impl ActionLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, entry: ActionLogEntry) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub fn entries(&self) -> impl Iterator<Item = &ActionLogEntry> {
        self.entries.iter()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for ActionLog {
    fn default() -> Self {
        Self::new(10)
    }
}

// ---------------------------------------------------------------------------
// LearnTree: the complete navigable tree
// ---------------------------------------------------------------------------

/// The complete MIDI learn tree with navigation state.
pub struct LearnTree {
    /// Root-level tree nodes (sections + Done)
    pub roots: Vec<TreeNode>,
    /// Flattened visible nodes for encoder scroll
    pub flat_nodes: Vec<FlatNode>,
    /// Current cursor position in flat_nodes
    pub cursor: usize,
    /// Topology configuration (determines tree shape)
    pub topology: TopologyConfig,
    /// Action log (footer)
    pub action_log: ActionLog,
}

impl LearnTree {
    /// Build a tree from the static catalog and topology config.
    pub fn build(topology: TopologyConfig) -> Self {
        let catalog = learn_catalog::section_catalog();
        let mut roots = Vec::new();

        for section_def in catalog {
            if !topology.is_visible(section_def.visibility) {
                continue;
            }

            let repeat_count = topology.repeat_count(section_def.repeat_mode);

            if repeat_count == 1 {
                // Single instance — create one section with visible mappings
                let children = Self::build_mapping_nodes(section_def, None, &topology);
                if children.is_empty() {
                    continue; // Skip empty sections
                }
                roots.push(TreeNode::Section {
                    section_id: section_def.id,
                    label: section_def.label.to_string(),
                    deck_index: None,
                    expanded: false,
                    children,
                });
            } else {
                // Repeated section — create parent with per-deck sub-sections
                let mut deck_sections = Vec::new();
                for i in 0..repeat_count {
                    let children = Self::build_mapping_nodes(section_def, Some(i), &topology);
                    if children.is_empty() {
                        continue;
                    }
                    deck_sections.push(TreeNode::Section {
                        section_id: section_def.id,
                        label: format!("Deck {}", i + 1),
                        deck_index: Some(i),
                        expanded: false,
                        children,
                    });
                }
                if deck_sections.is_empty() {
                    continue;
                }
                roots.push(TreeNode::Section {
                    section_id: section_def.id,
                    label: section_def.label.to_string(),
                    deck_index: None,
                    expanded: false,
                    children: deck_sections,
                });
            }
        }

        // Add terminal Done node
        roots.push(TreeNode::Done);

        let mut tree = Self {
            roots,
            flat_nodes: Vec::new(),
            cursor: 0,
            topology,
            action_log: ActionLog::default(),
        };
        tree.rebuild_flat_list();
        tree
    }

    /// Build mapping leaf nodes for a section, filtering by visibility.
    fn build_mapping_nodes(
        section_def: &SectionDef,
        deck_index: Option<usize>,
        topology: &TopologyConfig,
    ) -> Vec<TreeNode> {
        let mut nodes = Vec::new();
        for mapping_def in section_def.mappings {
            if !topology.is_visible(mapping_def.visibility) {
                continue;
            }
            nodes.push(TreeNode::Mapping {
                def: mapping_def,
                deck_index,
                mapped: None,
                original: None,
                status: MappingStatus::Unmapped,
            });
        }
        nodes
    }

    // -----------------------------------------------------------------------
    // Flat list management
    // -----------------------------------------------------------------------

    /// Rebuild the flat node list from current expand/collapse state.
    ///
    /// Called after toggling a section or building the tree.
    pub fn rebuild_flat_list(&mut self) {
        self.flat_nodes.clear();
        for (i, root) in self.roots.iter().enumerate() {
            Self::flatten_node(root, &[i], 0, &mut self.flat_nodes);
        }
        // Clamp cursor
        if !self.flat_nodes.is_empty() && self.cursor >= self.flat_nodes.len() {
            self.cursor = self.flat_nodes.len() - 1;
        }
    }

    fn flatten_node(
        node: &TreeNode,
        path: &[usize],
        depth: usize,
        out: &mut Vec<FlatNode>,
    ) {
        match node {
            TreeNode::Section { expanded, children, .. } => {
                out.push(FlatNode {
                    tree_path: path.to_vec(),
                    depth,
                    node_type: FlatNodeType::Section,
                });
                if *expanded {
                    for (i, child) in children.iter().enumerate() {
                        let mut child_path = path.to_vec();
                        child_path.push(i);
                        Self::flatten_node(child, &child_path, depth + 1, out);
                    }
                }
            }
            TreeNode::Mapping { .. } => {
                out.push(FlatNode {
                    tree_path: path.to_vec(),
                    depth,
                    node_type: FlatNodeType::Mapping,
                });
            }
            TreeNode::Reset => {
                out.push(FlatNode {
                    tree_path: path.to_vec(),
                    depth: 0,
                    node_type: FlatNodeType::Reset,
                });
            }
            TreeNode::Done => {
                out.push(FlatNode {
                    tree_path: path.to_vec(),
                    depth: 0,
                    node_type: FlatNodeType::Done,
                });
            }
        }
    }

    // -----------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------

    /// Move cursor by delta (from encoder scroll). Positive = down, negative = up.
    ///
    /// Manual scrolling does NOT auto-fold/unfold sections — the user controls
    /// section state manually via browse press (toggle). Auto-fold only happens
    /// in `advance_to_next()` after answering questions.
    pub fn scroll(&mut self, delta: i32) {
        if self.flat_nodes.is_empty() {
            return;
        }
        let new = self.cursor as i32 + delta;
        self.cursor = new.clamp(0, self.flat_nodes.len() as i32 - 1) as usize;
    }

    /// Handle select/press on the current node.
    ///
    /// - Section: toggle expand/collapse
    /// - Done: returns true (signals verification)
    /// - Mapping: no-op (mappings are captured via MIDI/HID events, not select)
    pub fn select(&mut self) -> bool {
        if self.flat_nodes.is_empty() {
            return false;
        }
        let flat = &self.flat_nodes[self.cursor];
        match flat.node_type {
            FlatNodeType::Section => {
                // Toggle expand/collapse
                let node = self.node_at_path_mut(&flat.tree_path.clone());
                if let TreeNode::Section { expanded, .. } = node {
                    *expanded = !*expanded;
                }
                self.rebuild_flat_list();
                false
            }
            FlatNodeType::Reset => false, // handled via browse select in tick.rs
            FlatNodeType::Done => true,
            FlatNodeType::Mapping => false,
        }
    }

    /// Get a reference to the node at the current cursor position.
    pub fn current_node(&self) -> Option<&TreeNode> {
        self.flat_nodes.get(self.cursor).map(|f| self.node_at_path(&f.tree_path))
    }

    /// Get a mutable reference to the node at the current cursor position.
    pub fn current_node_mut(&mut self) -> Option<&mut TreeNode> {
        if self.cursor >= self.flat_nodes.len() {
            return None;
        }
        let path = self.flat_nodes[self.cursor].tree_path.clone();
        Some(self.node_at_path_mut(&path))
    }

    /// Get the current cursor's flat node info.
    pub fn current_flat(&self) -> Option<&FlatNode> {
        self.flat_nodes.get(self.cursor)
    }

    // -----------------------------------------------------------------------
    // Mapping operations
    // -----------------------------------------------------------------------

    /// Record a mapping on the current node (must be a Mapping node).
    ///
    /// Returns true if the mapping was recorded, false if cursor is not on a Mapping.
    pub fn record_mapping(&mut self, entry: MappedControl) -> bool {
        let path = match self.flat_nodes.get(self.cursor) {
            Some(f) if f.node_type == FlatNodeType::Mapping => f.tree_path.clone(),
            _ => return false,
        };
        let node = self.node_at_path_mut(&path);
        if let TreeNode::Mapping { mapped, status, original, .. } = node {
            let was_existing = original.is_some();
            *mapped = Some(entry);
            *status = if was_existing {
                MappingStatus::Changed
            } else {
                MappingStatus::New
            };
            true
        } else {
            false
        }
    }

    /// After recording a mapping, advance to the next unmapped leaf.
    ///
    /// 1. First searches the visible flat list for the next unmapped node.
    /// 2. If not found, searches collapsed sections — collapses current root,
    ///    expands the next root with unmapped nodes, and jumps there.
    pub fn advance_to_next(&mut self) {
        let start = self.cursor;
        let current_root = self.root_index_for_cursor(start);
        let current_sub = self.sub_index_for_cursor(start);

        // 1. Search visible flat list after cursor
        for i in (start + 1)..self.flat_nodes.len() {
            if self.flat_nodes[i].node_type == FlatNodeType::Mapping {
                let node = self.node_at_path(&self.flat_nodes[i].tree_path);
                if let TreeNode::Mapping { status, .. } = node {
                    if *status == MappingStatus::Unmapped {
                        self.cursor = i;
                        return;
                    }
                }
            }
        }

        // 2. Search for next unmapped SUBSECTION within the same root
        //    (e.g., Transport Deck 1 → Transport Deck 2)
        if let Some(root_idx) = current_root {
            if let Some(target_sub) = self.find_next_unmapped_sub(root_idx, current_sub) {
                self.collapse_sub(root_idx, current_sub);
                self.expand_sub(root_idx, target_sub);
                self.rebuild_flat_list();
                // Find first unmapped in target subsection
                let section_start = self.flat_nodes.iter().position(|f|
                    f.tree_path.len() >= 2 && f.tree_path[0] == root_idx && f.tree_path[1] == target_sub
                ).unwrap_or(0);
                // Park cursor at section header (rebuild_flat_list may have clamped it to end)
                self.cursor = section_start;
                for i in section_start..self.flat_nodes.len() {
                    if self.flat_nodes[i].node_type == FlatNodeType::Mapping {
                        let node = self.node_at_path(&self.flat_nodes[i].tree_path);
                        if let TreeNode::Mapping { status, .. } = node {
                            if *status == MappingStatus::Unmapped {
                                self.cursor = i;
                                return;
                            }
                        }
                    }
                }
                return;
            }
        }

        // 3. No unmapped subsections → search next ROOT section
        if let Some(target) = self.find_next_unmapped_root(current_root) {
            if let Some(cur) = current_root {
                self.collapse_root(cur);
            }
            self.expand_to_first_unmapped(target);
            self.rebuild_flat_list();
            let section_start = self.flat_nodes.iter().position(|f|
                f.tree_path.first() == Some(&target)
            ).unwrap_or(0);
            // Park cursor at section header (rebuild_flat_list may have clamped it to end)
            self.cursor = section_start;
            for i in section_start..self.flat_nodes.len() {
                if self.flat_nodes[i].node_type == FlatNodeType::Mapping {
                    let node = self.node_at_path(&self.flat_nodes[i].tree_path);
                    if let TreeNode::Mapping { status, .. } = node {
                        if *status == MappingStatus::Unmapped {
                            self.cursor = i;
                            return;
                        }
                    }
                }
            }
            return;
        }

        // 4. No more unmapped anywhere → move to next visible item
        let new = (start + 1).min(self.flat_nodes.len().saturating_sub(1));
        self.cursor = new;
    }

    // -----------------------------------------------------------------------
    // Section auto-fold/unfold helpers
    // -----------------------------------------------------------------------

    /// Find which root section index the current cursor belongs to.
    fn root_index_for_cursor(&self, cursor: usize) -> Option<usize> {
        self.flat_nodes.get(cursor)
            .and_then(|f| f.tree_path.first().copied())
    }

    /// Find which subsection index the current cursor belongs to (tree_path[1]).
    fn sub_index_for_cursor(&self, cursor: usize) -> Option<usize> {
        self.flat_nodes.get(cursor)
            .and_then(|f| if f.tree_path.len() >= 2 { Some(f.tree_path[1]) } else { None })
    }

    /// Find next unmapped subsection within a root, starting after `after_sub`.
    fn find_next_unmapped_sub(&self, root_idx: usize, after_sub: Option<usize>) -> Option<usize> {
        if let TreeNode::Section { children, .. } = &self.roots[root_idx] {
            let start = after_sub.map(|s| s + 1).unwrap_or(0);
            for i in start..children.len() {
                if matches!(&children[i], TreeNode::Section { .. }) && Self::node_has_unmapped(&children[i]) {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Collapse a subsection within a root.
    fn collapse_sub(&mut self, root_idx: usize, sub: Option<usize>) {
        if let Some(sub_idx) = sub {
            if let TreeNode::Section { children, .. } = &mut self.roots[root_idx] {
                if let Some(TreeNode::Section { expanded, .. }) = children.get_mut(sub_idx) {
                    *expanded = false;
                }
            }
        }
    }

    /// Expand a specific subsection within a root (keeps root expanded).
    fn expand_sub(&mut self, root_idx: usize, sub_idx: usize) {
        if let TreeNode::Section { expanded, children, .. } = &mut self.roots[root_idx] {
            *expanded = true;
            if let Some(TreeNode::Section { expanded: child_exp, .. }) = children.get_mut(sub_idx) {
                *child_exp = true;
            }
        }
    }

    /// Search root sections starting after `after_root` for one with unmapped nodes.
    fn find_next_unmapped_root(&self, after_root: Option<usize>) -> Option<usize> {
        let start = after_root.map(|r| r + 1).unwrap_or(0);
        for i in start..self.roots.len() {
            if Self::node_has_unmapped(&self.roots[i]) {
                return Some(i);
            }
        }
        None
    }

    /// Recursively check if a node has any unmapped mapping descendants.
    fn node_has_unmapped(node: &TreeNode) -> bool {
        match node {
            TreeNode::Section { children, .. } => {
                children.iter().any(|c| Self::node_has_unmapped(c))
            }
            TreeNode::Mapping { status, .. } => *status == MappingStatus::Unmapped,
            TreeNode::Reset | TreeNode::Done => false,
        }
    }

    /// Collapse a root section and all its sub-sections.
    fn collapse_root(&mut self, root_idx: usize) {
        if root_idx >= self.roots.len() { return; }
        if let TreeNode::Section { expanded, children, .. } = &mut self.roots[root_idx] {
            *expanded = false;
            for child in children.iter_mut() {
                if let TreeNode::Section { expanded, .. } = child {
                    *expanded = false;
                }
            }
        }
    }

    /// Expand a root section and its first sub-section that has unmapped nodes.
    fn expand_to_first_unmapped(&mut self, root_idx: usize) {
        if root_idx >= self.roots.len() { return; }
        if let TreeNode::Section { expanded, children, .. } = &mut self.roots[root_idx] {
            *expanded = true;
            // If children are sub-sections, expand first one with unmapped nodes
            // Find index first (immutable), then mutate — avoids borrow conflict
            let target_idx = children.iter().position(|child| {
                matches!(child, TreeNode::Section { .. }) && Self::node_has_unmapped(child)
            });
            if let Some(idx) = target_idx {
                if let TreeNode::Section { expanded: child_exp, .. } = &mut children[idx] {
                    *child_exp = true;
                }
            }
        }
    }

    /// Clear the mapping on the current node (revert to unmapped).
    pub fn clear_current_mapping(&mut self) {
        let path = match self.flat_nodes.get(self.cursor) {
            Some(f) if f.node_type == FlatNodeType::Mapping => f.tree_path.clone(),
            _ => return,
        };
        let node = self.node_at_path_mut(&path);
        if let TreeNode::Mapping { mapped, status, original, .. } = node {
            *mapped = None;
            *status = if original.is_some() {
                // Was existing but now cleared — treat as changed
                MappingStatus::Changed
            } else {
                MappingStatus::Unmapped
            };
        }
    }

    /// Clear ALL mappings in the tree (reset to unmapped state).
    pub fn clear_all_mappings(&mut self) {
        for root in &mut self.roots {
            Self::clear_all_recursive(root);
        }
        self.rebuild_flat_list();
        self.cursor = 0;
    }

    fn clear_all_recursive(node: &mut TreeNode) {
        match node {
            TreeNode::Section { children, .. } => {
                for child in children {
                    Self::clear_all_recursive(child);
                }
            }
            TreeNode::Mapping { mapped, status, original, .. } => {
                *mapped = None;
                *original = None;
                *status = MappingStatus::Unmapped;
            }
            TreeNode::Reset | TreeNode::Done => {}
        }
    }

    // -----------------------------------------------------------------------
    // Query: all mapped nodes (for config generation)
    // -----------------------------------------------------------------------

    /// Collect all mapped nodes for config generation.
    ///
    /// Returns (MappingDef, deck_index, MappedControl) for each node with a mapping.
    pub fn all_mapped_nodes(&self) -> Vec<(&'static MappingDef, Option<usize>, &MappedControl)> {
        let mut result = Vec::new();
        for root in &self.roots {
            Self::collect_mapped(root, &mut result);
        }
        result
    }

    fn collect_mapped<'a>(
        node: &'a TreeNode,
        out: &mut Vec<(&'static MappingDef, Option<usize>, &'a MappedControl)>,
    ) {
        match node {
            TreeNode::Section { children, .. } => {
                for child in children {
                    Self::collect_mapped(child, out);
                }
            }
            TreeNode::Mapping { def, deck_index, mapped: Some(ctrl), .. } => {
                out.push((def, *deck_index, ctrl));
            }
            _ => {}
        }
    }

    /// Collect only changed nodes (for verification window diff).
    ///
    /// Returns nodes where status is New or Changed.
    pub fn changed_nodes(&self) -> Vec<(&'static MappingDef, Option<usize>, &MappedControl, MappingStatus)> {
        let mut result = Vec::new();
        for root in &self.roots {
            Self::collect_changed(root, &mut result);
        }
        result
    }

    fn collect_changed<'a>(
        node: &'a TreeNode,
        out: &mut Vec<(&'static MappingDef, Option<usize>, &'a MappedControl, MappingStatus)>,
    ) {
        match node {
            TreeNode::Section { children, .. } => {
                for child in children {
                    Self::collect_changed(child, out);
                }
            }
            TreeNode::Mapping { def, deck_index, mapped: Some(ctrl), status, .. }
                if *status == MappingStatus::New || *status == MappingStatus::Changed =>
            {
                out.push((def, *deck_index, ctrl, *status));
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Find node by action (for loading existing config)
    // -----------------------------------------------------------------------

    /// Find a mapping node by action string, deck index, and param value.
    ///
    /// Used when loading existing config to match `ControlMapping` entries
    /// back to tree nodes.
    pub fn find_mapping_node_mut(
        &mut self,
        action: &str,
        deck_idx: Option<usize>,
        param_key: Option<&str>,
        param_value: Option<usize>,
    ) -> Option<&mut TreeNode> {
        for root in &mut self.roots {
            if let Some(node) = Self::find_in_node_mut(root, action, deck_idx, param_key, param_value) {
                return Some(node);
            }
        }
        None
    }

    fn find_in_node_mut<'a>(
        node: &'a mut TreeNode,
        action: &str,
        deck_idx: Option<usize>,
        param_key: Option<&str>,
        param_value: Option<usize>,
    ) -> Option<&'a mut TreeNode> {
        match node {
            TreeNode::Section { children, .. } => {
                for child in children {
                    if let Some(found) = Self::find_in_node_mut(child, action, deck_idx, param_key, param_value) {
                        return Some(found);
                    }
                }
                None
            }
            TreeNode::Mapping { def, deck_index, .. } => {
                if def.action == action
                    && *deck_index == deck_idx
                    && def.param_key == param_key
                    && def.param_value == param_value
                {
                    Some(node)
                } else {
                    None
                }
            }
            TreeNode::Reset | TreeNode::Done => None,
        }
    }

    /// Find a mapping node by its def.id (unique identifier).
    pub fn find_by_id_mut(&mut self, id: &str) -> Option<&mut TreeNode> {
        for root in &mut self.roots {
            if let Some(node) = Self::find_by_id_in_node_mut(root, id) {
                return Some(node);
            }
        }
        None
    }

    fn find_by_id_in_node_mut<'a>(
        node: &'a mut TreeNode,
        id: &str,
    ) -> Option<&'a mut TreeNode> {
        match node {
            TreeNode::Section { children, .. } => {
                for child in children {
                    if let Some(found) = Self::find_by_id_in_node_mut(child, id) {
                        return Some(found);
                    }
                }
                None
            }
            TreeNode::Mapping { def, .. } if def.id == id => Some(node),
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Auto-expand navigation section
    // -----------------------------------------------------------------------

    /// Expand the Navigation section and set cursor to its first child.
    pub fn expand_navigation(&mut self) {
        // Find and expand the navigation section (may not be first if Reset is present)
        for root in &mut self.roots {
            if let TreeNode::Section { section_id, expanded, .. } = root {
                if *section_id == "navigation" {
                    *expanded = true;
                    break;
                }
            }
        }
        self.rebuild_flat_list();
        // Set cursor to first unmapped mapping (shift buttons)
        for (i, flat) in self.flat_nodes.iter().enumerate() {
            if flat.node_type == FlatNodeType::Mapping {
                let node = self.node_at_path(&flat.tree_path);
                if let TreeNode::Mapping { status, .. } = node {
                    if *status == MappingStatus::Unmapped {
                        self.cursor = i;
                        return;
                    }
                }
            }
        }
        // Fallback: first mapping node
        if let Some(i) = self.flat_nodes.iter().position(|f| f.node_type == FlatNodeType::Mapping) {
            self.cursor = i;
        }
    }

    /// Total count of all mapped nodes (for progress display).
    pub fn total_progress(&self) -> (usize, usize) {
        let mut mapped = 0;
        let mut total = 0;
        for root in &self.roots {
            Self::count_progress(root, &mut mapped, &mut total);
        }
        (mapped, total)
    }

    fn count_progress(node: &TreeNode, mapped: &mut usize, total: &mut usize) {
        match node {
            TreeNode::Section { children, .. } => {
                for child in children {
                    Self::count_progress(child, mapped, total);
                }
            }
            TreeNode::Mapping { status, .. } => {
                *total += 1;
                if *status != MappingStatus::Unmapped {
                    *mapped += 1;
                }
            }
            TreeNode::Reset | TreeNode::Done => {}
        }
    }

    // -----------------------------------------------------------------------
    // Path traversal helpers
    // -----------------------------------------------------------------------

    pub fn node_at_path(&self, path: &[usize]) -> &TreeNode {
        let mut current: &TreeNode = &self.roots[path[0]];
        for &idx in &path[1..] {
            match current {
                TreeNode::Section { children, .. } => {
                    current = &children[idx];
                }
                _ => panic!("Invalid tree path: expected Section at depth"),
            }
        }
        current
    }

    pub fn node_at_path_mut(&mut self, path: &[usize]) -> &mut TreeNode {
        let mut current: &mut TreeNode = &mut self.roots[path[0]];
        for &idx in &path[1..] {
            match current {
                TreeNode::Section { children, .. } => {
                    current = &mut children[idx];
                }
                _ => panic!("Invalid tree path: expected Section at depth"),
            }
        }
        current
    }
}
