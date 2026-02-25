//! Application font configuration
//!
//! Provides a selectable set of monospace fonts bundled at compile time.
//! Both mesh-player and mesh-cue share this enum for consistent font
//! selection across the UI and config persistence.

use std::sync::LazyLock;
use serde::{Deserialize, Serialize};
use iced::Font;
use iced::font::Family;
use iced::widget::image;

/// Logo image handle — created once and reused across all view frames.
/// Without this, `Handle::from_bytes()` inside view functions causes flickering
/// because iced creates a new texture upload per frame.
pub static LOGO_HANDLE: LazyLock<image::Handle> = LazyLock::new(|| {
    image::Handle::from_bytes(
        include_bytes!("../../../assets/grid.png") as &[u8],
    )
});

/// Available application fonts.
///
/// Each variant bundles its .ttf data via `include_bytes!()` and provides
/// the iced `Font` descriptor needed for `default_font()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AppFont {
    /// Hack — clean monospace, optimized for source code (MIT license)
    Hack,
    /// JetBrains Mono — modern monospace with ligatures (OFL 1.1)
    JetBrainsMono,
    /// Press Start 2P — retro 8-bit pixel font (OFL 1.1)
    PressStart2P,
    /// Exo — geometric sans-serif with a futuristic feel (OFL 1.1)
    Exo,
    /// Space Mono — fixed-width typeface designed for editorial use (OFL 1.1)
    #[default]
    SpaceMono,
    /// saxMono — clean monospace between Courier and Letter Gothic (Freeware)
    SaxMono,
}

impl AppFont {
    /// All available fonts in display order.
    pub const ALL: [AppFont; 6] = [
        AppFont::Hack,
        AppFont::JetBrainsMono,
        AppFont::PressStart2P,
        AppFont::Exo,
        AppFont::SpaceMono,
        AppFont::SaxMono,
    ];

    /// Human-readable name for settings UI.
    pub fn display_name(&self) -> &'static str {
        match self {
            AppFont::Hack => "Hack",
            AppFont::JetBrainsMono => "JetBrains Mono",
            AppFont::PressStart2P => "Press Start 2P",
            AppFont::Exo => "Exo",
            AppFont::SpaceMono => "Space Mono",
            AppFont::SaxMono => "Sax Mono",
        }
    }

    /// Font family name as registered in iced's font system.
    pub fn family_name(&self) -> &'static str {
        match self {
            AppFont::Hack => "Hack",
            AppFont::JetBrainsMono => "JetBrains Mono",
            AppFont::PressStart2P => "Press Start 2P",
            AppFont::Exo => "Exo",
            AppFont::SpaceMono => "Space Mono",
            AppFont::SaxMono => "saxMono",
        }
    }

    /// Raw .ttf bytes for registering with iced via `.font()`.
    pub fn font_data(&self) -> &'static [u8] {
        match self {
            AppFont::Hack => include_bytes!("../../../assets/fonts/Hack-Regular.ttf"),
            AppFont::JetBrainsMono => include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"),
            AppFont::PressStart2P => include_bytes!("../../../assets/fonts/PressStart2P-Regular.ttf"),
            AppFont::Exo => include_bytes!("../../../assets/fonts/Exo-Regular.ttf"),
            AppFont::SpaceMono => include_bytes!("../../../assets/fonts/SpaceMono-Regular.ttf"),
            AppFont::SaxMono => include_bytes!("../../../assets/fonts/SaxMono-Regular.ttf"),
        }
    }

    /// iced `Font` descriptor for use with `default_font()`.
    pub fn to_iced_font(&self) -> Font {
        Font {
            family: Family::Name(self.family_name()),
            ..Font::DEFAULT
        }
    }

    /// Global font size scale factor.
    ///
    /// Pixel fonts like Press Start 2P render much larger than vector fonts
    /// at the same nominal size. This multiplier normalizes visual appearance
    /// so that all fonts look approximately the same size in the UI.
    pub fn size_scale(&self) -> f32 {
        match self {
            AppFont::Hack => 1.0,
            AppFont::JetBrainsMono => 1.0,
            AppFont::PressStart2P => 0.42,
            AppFont::Exo => 1.0,
            AppFont::SpaceMono => 1.1,
            AppFont::SaxMono => 1.1,
        }
    }
}
