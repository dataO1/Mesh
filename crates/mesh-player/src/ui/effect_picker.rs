//! Effect picker modal for adding effects to stem chains
//!
//! Provides a modal dialog for selecting and adding PD effects to stems.
//! Effects are grouped by category and show availability status.

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Element, Length};
use mesh_core::pd::DiscoveredEffect;
use mesh_core::types::Stem;

/// Messages for the effect picker
#[derive(Debug, Clone)]
pub enum EffectPickerMessage {
    /// Open the picker for a specific deck and stem
    Open { deck: usize, stem: usize },
    /// Close the picker without selecting
    Close,
    /// Select an effect to add
    SelectEffect(String),
}

/// State for the effect picker modal
#[derive(Debug, Clone)]
pub struct EffectPickerState {
    /// Whether the picker is currently open
    pub is_open: bool,
    /// Target deck index (0-3)
    pub target_deck: usize,
    /// Target stem index (0-3: Vocals, Drums, Bass, Other)
    pub target_stem: usize,
    /// Currently selected category filter (None = show all)
    pub selected_category: Option<String>,
}

impl Default for EffectPickerState {
    fn default() -> Self {
        Self {
            is_open: false,
            target_deck: 0,
            target_stem: 0,
            selected_category: None,
        }
    }
}

impl EffectPickerState {
    /// Create a new effect picker state
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the picker for a specific deck and stem
    pub fn open(&mut self, deck: usize, stem: usize) {
        self.is_open = true;
        self.target_deck = deck;
        self.target_stem = stem;
        self.selected_category = None;
    }

    /// Close the picker
    pub fn close(&mut self) {
        self.is_open = false;
    }

    /// Get the target stem as a Stem enum
    pub fn target_stem_enum(&self) -> Stem {
        match self.target_stem {
            0 => Stem::Vocals,
            1 => Stem::Drums,
            2 => Stem::Bass,
            _ => Stem::Other,
        }
    }

    /// Get stem name for display
    fn stem_name(&self) -> &'static str {
        match self.target_stem {
            0 => "Vocals",
            1 => "Drums",
            2 => "Bass",
            _ => "Other",
        }
    }

    /// Render the effect picker modal
    ///
    /// # Arguments
    /// * `effects` - List of discovered effects from PdManager
    pub fn view(&self, effects: &[&DiscoveredEffect]) -> Element<'static, EffectPickerMessage> {
        if !self.is_open {
            return Space::new().width(0).height(0).into();
        }

        // Header
        let header = row![
            text(format!(
                "Add Effect to Deck {} - {}",
                self.target_deck + 1,
                self.stem_name()
            ))
            .size(18),
            Space::new().width(Length::Fill),
            button(text("✕").size(16))
                .on_press(EffectPickerMessage::Close)
                .padding(5),
        ]
        .align_y(Alignment::Center)
        .spacing(10);

        // Group effects by category (clone strings to avoid lifetime issues)
        let mut categories: Vec<String> = effects
            .iter()
            .map(|e| e.category().to_string())
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
                .filter(|e| e.category() == category)
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
                    text("Place PD effects in:").size(12),
                    text("~/Music/mesh-collection/effects/").size(11),
                    Space::new().height(10),
                    text("Each effect needs:").size(12),
                    text("  • metadata.json").size(11),
                    text("  • <effect-name>.pd").size(11),
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
        .height(Length::Fixed(300.0));

        // Footer with cancel button
        let footer = row![
            Space::new().width(Length::Fill),
            button(text("Cancel").size(14))
                .on_press(EffectPickerMessage::Close)
                .padding([8, 16]),
        ];

        // Modal content
        let content = column![header, effect_list, footer]
            .spacing(15)
            .padding(20)
            .width(Length::Fixed(400.0));

        // Wrap in container with background
        container(content)
            .style(container::bordered_box)
            .into()
    }

    /// Render a single effect row
    ///
    /// Note: This clones strings to avoid lifetime issues with the returned Element
    fn view_effect_row(&self, effect: &DiscoveredEffect) -> Element<'static, EffectPickerMessage> {
        let available = effect.available;
        let name = effect.name().to_string();
        let id = effect.id.clone();

        // Effect name and status
        let name_text = if available {
            text(name).size(13)
        } else {
            text(format!("{} (unavailable)", effect.name())).size(13)
        };

        // Missing dependencies hint
        let status = if !effect.missing_deps.is_empty() {
            text(format!("Missing: {}", effect.missing_deps.join(", ")))
                .size(10)
        } else {
            text("").size(10)
        };

        let info_col = column![name_text, status].spacing(2);

        // Add button (disabled if unavailable)
        let add_btn = if available {
            button(text("Add").size(12))
                .on_press(EffectPickerMessage::SelectEffect(id))
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
