//! Effect picker modal for adding effects to multiband chains
//!
//! Provides a modal dialog for selecting and adding effects.
//! Supports both PD effects and CLAP plugins, grouped by category.

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Element, Length};
use mesh_core::clap::DiscoveredClapPlugin;
use mesh_core::pd::DiscoveredEffect;

/// Effect source type for distinguishing PD from CLAP effects
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectSource {
    /// Pure Data effect
    Pd,
    /// CLAP plugin
    Clap,
}

/// Unified effect item for display in the picker
#[derive(Debug, Clone)]
pub struct EffectListItem {
    /// Unique identifier (folder name for PD, plugin ID for CLAP)
    pub id: String,
    /// Display name
    pub name: String,
    /// Category (e.g., "Distortion", "Reverb")
    pub category: String,
    /// Whether the effect is available (all dependencies met)
    pub available: bool,
    /// Status message (missing deps for PD, error for CLAP)
    pub status_message: Option<String>,
    /// Source type (PD or CLAP)
    pub source: EffectSource,
}

impl EffectListItem {
    /// Create from a PD effect
    pub fn from_pd(effect: &DiscoveredEffect) -> Self {
        let status_message = if !effect.missing_deps.is_empty() {
            Some(format!("Missing: {}", effect.missing_deps.join(", ")))
        } else {
            None
        };

        Self {
            id: effect.id.clone(),
            name: effect.name().to_string(),
            category: effect.category().to_string(),
            available: effect.available,
            status_message,
            source: EffectSource::Pd,
        }
    }

    /// Create from a CLAP plugin
    pub fn from_clap(plugin: &DiscoveredClapPlugin) -> Self {
        let status_message = plugin.error_message.clone();

        Self {
            id: plugin.id.clone(),
            name: plugin.name.clone(),
            category: plugin.category_name().to_string(),
            available: plugin.available,
            status_message,
            source: EffectSource::Clap,
        }
    }
}

/// Messages for the effect picker
#[derive(Debug, Clone)]
pub enum EffectPickerMessage {
    /// Open the picker for a specific target
    Open { target: EffectPickerTarget },
    /// Close the picker without selecting
    Close,
    /// Select a PD effect to add
    SelectPdEffect(String),
    /// Select a CLAP effect to add
    SelectClapEffect(String),
    /// Toggle between showing all effects or filtering by source
    ToggleSourceFilter(Option<EffectSource>),
}

/// Target location for adding an effect
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectPickerTarget {
    /// Add to pre-fx chain
    PreFx,
    /// Add to a specific band's effect chain
    Band(usize),
    /// Add to post-fx chain
    PostFx,
}

/// State for the effect picker modal
#[derive(Debug, Clone)]
pub struct EffectPickerState {
    /// Whether the picker is currently open
    pub is_open: bool,
    /// Target location for the effect
    pub target: EffectPickerTarget,
    /// Currently selected category filter (None = show all)
    pub selected_category: Option<String>,
    /// Filter by source (None = show all, Some = filter)
    pub source_filter: Option<EffectSource>,
}

impl Default for EffectPickerState {
    fn default() -> Self {
        Self {
            is_open: false,
            target: EffectPickerTarget::Band(0),
            selected_category: None,
            source_filter: None,
        }
    }
}

impl EffectPickerState {
    /// Create a new effect picker state
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the picker for pre-fx chain
    pub fn open_pre_fx(&mut self) {
        self.is_open = true;
        self.target = EffectPickerTarget::PreFx;
        self.selected_category = None;
    }

    /// Open the picker for a specific band
    pub fn open_for_band(&mut self, band: usize) {
        self.is_open = true;
        self.target = EffectPickerTarget::Band(band);
        self.selected_category = None;
    }

    /// Open the picker for post-fx chain
    pub fn open_post_fx(&mut self) {
        self.is_open = true;
        self.target = EffectPickerTarget::PostFx;
        self.selected_category = None;
    }

    /// Close the picker
    pub fn close(&mut self) {
        self.is_open = false;
    }

    /// Get target description for display
    fn target_description(&self) -> String {
        match self.target {
            EffectPickerTarget::PreFx => "Pre-FX Chain".to_string(),
            EffectPickerTarget::Band(idx) => format!("Band {}", idx + 1),
            EffectPickerTarget::PostFx => "Post-FX Chain".to_string(),
        }
    }

    /// Render the effect picker modal
    ///
    /// # Arguments
    /// * `pd_effects` - List of discovered PD effects
    /// * `clap_plugins` - List of discovered CLAP plugins
    pub fn view(
        &self,
        pd_effects: &[&DiscoveredEffect],
        clap_plugins: &[&DiscoveredClapPlugin],
    ) -> Element<'static, EffectPickerMessage> {
        if !self.is_open {
            return Space::new().width(0).height(0).into();
        }

        // Build unified effect list
        let mut effects: Vec<EffectListItem> = Vec::new();

        // Add PD effects (filtered by source if needed)
        if self.source_filter.is_none() || self.source_filter == Some(EffectSource::Pd) {
            for effect in pd_effects {
                effects.push(EffectListItem::from_pd(effect));
            }
        }

        // Add CLAP plugins (filtered by source if needed)
        if self.source_filter.is_none() || self.source_filter == Some(EffectSource::Clap) {
            for plugin in clap_plugins {
                effects.push(EffectListItem::from_clap(plugin));
            }
        }

        // Header
        let header = row![
            text(format!("Add Effect to {}", self.target_description()))
                .size(18),
            Space::new().width(Length::Fill),
            button(text("✕").size(16))
                .on_press(EffectPickerMessage::Close)
                .padding(5),
        ]
        .align_y(Alignment::Center)
        .spacing(10);

        // Source filter buttons
        let filter_row = row![
            button(text("All").size(12))
                .on_press(EffectPickerMessage::ToggleSourceFilter(None))
                .padding([4, 10])
                .style(if self.source_filter.is_none() {
                    button::primary
                } else {
                    button::secondary
                }),
            button(text("PD").size(12))
                .on_press(EffectPickerMessage::ToggleSourceFilter(Some(EffectSource::Pd)))
                .padding([4, 10])
                .style(if self.source_filter == Some(EffectSource::Pd) {
                    button::primary
                } else {
                    button::secondary
                }),
            button(text("CLAP").size(12))
                .on_press(EffectPickerMessage::ToggleSourceFilter(Some(EffectSource::Clap)))
                .padding([4, 10])
                .style(if self.source_filter == Some(EffectSource::Clap) {
                    button::primary
                } else {
                    button::secondary
                }),
            Space::new().width(Length::Fill),
            text(format!(
                "{} PD, {} CLAP",
                pd_effects.len(),
                clap_plugins.len()
            ))
            .size(11),
        ]
        .spacing(5)
        .align_y(Alignment::Center);

        // Group effects by category
        let mut categories: Vec<String> = effects
            .iter()
            .map(|e| e.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        categories.sort();

        // Build effect list
        let mut effect_rows: Vec<Element<'static, EffectPickerMessage>> = Vec::new();

        for category in &categories {
            // Category header
            effect_rows.push(
                text(category.clone())
                    .size(14)
                    .style(iced::widget::text::primary)
                    .into(),
            );

            // Effects in this category
            let category_effects: Vec<_> = effects
                .iter()
                .filter(|e| &e.category == category)
                .collect();

            for effect in category_effects {
                let effect_row = self.view_effect_row(effect);
                effect_rows.push(effect_row);
            }

            // Spacing between categories
            effect_rows.push(Space::new().height(10).into());
        }

        // If no effects found
        if effects.is_empty() {
            effect_rows.push(
                column![
                    text("No effects found").size(14),
                    Space::new().height(10),
                    text("Effects locations (in mesh-collection/effects/):").size(12),
                    text("  PD:   effects/pd/<effect-name>/").size(11),
                    text("  CLAP: effects/clap/*.clap").size(11),
                ]
                .spacing(2)
                .into(),
            );
        }

        let effect_list = scrollable(
            column(effect_rows)
                .spacing(4)
                .padding(10)
                .width(Length::Fill),
        )
        .height(Length::Fixed(350.0));

        // Footer with cancel button
        let footer = row![
            Space::new().width(Length::Fill),
            button(text("Cancel").size(14))
                .on_press(EffectPickerMessage::Close)
                .padding([8, 16]),
        ];

        // Modal content
        let content = column![header, filter_row, effect_list, footer]
            .spacing(15)
            .padding(20)
            .width(Length::Fixed(450.0));

        // Wrap in container with background
        container(content)
            .style(container::bordered_box)
            .into()
    }

    /// Render a single effect row
    fn view_effect_row(&self, effect: &EffectListItem) -> Element<'static, EffectPickerMessage> {
        let available = effect.available;
        let name = effect.name.clone();
        let id = effect.id.clone();
        let source = effect.source.clone();

        // Source badge
        let source_badge = match &effect.source {
            EffectSource::Pd => text("PD").size(9),
            EffectSource::Clap => text("CLAP").size(9),
        };

        // Effect name and status
        let name_text = if available {
            text(name).size(13)
        } else {
            text(format!("{} (unavailable)", effect.name)).size(13)
        };

        // Status message (missing deps or error)
        let status = if let Some(ref msg) = effect.status_message {
            text(msg.clone()).size(10)
        } else {
            text("").size(10)
        };

        let info_col = column![
            row![source_badge, Space::new().width(5), name_text]
                .align_y(Alignment::Center),
            status
        ]
        .spacing(2);

        // Add button (disabled if unavailable)
        let add_btn = if available {
            let msg = match source {
                EffectSource::Pd => EffectPickerMessage::SelectPdEffect(id),
                EffectSource::Clap => EffectPickerMessage::SelectClapEffect(id),
            };
            button(text("Add").size(12))
                .on_press(msg)
                .padding([4, 12])
        } else {
            button(text("Add").size(12)).padding([4, 12])
        };

        row![info_col, Space::new().width(Length::Fill), add_btn]
            .align_y(Alignment::Center)
            .spacing(10)
            .padding([4, 8])
            .into()
    }
}
