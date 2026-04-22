//! Suggestion algorithm configuration enums.
//!
//! These types control the scoring parameters of the smart suggestion engine.
//! They are serialized to YAML as part of the player's DisplayConfig.

use serde::{Deserialize, Serialize};

/// Key scoring model for harmonic compatibility
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyScoringModel {
    /// Camelot wheel distance with hand-tuned transition scores
    #[default]
    Camelot,
    /// Krumhansl-Kessler probe-tone profile correlations
    Krumhansl,
}

impl KeyScoringModel {
    pub const ALL: [KeyScoringModel; 2] = [
        KeyScoringModel::Camelot,
        KeyScoringModel::Krumhansl,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            KeyScoringModel::Camelot => "Camelot",
            KeyScoringModel::Krumhansl => "Krumhansl",
        }
    }
}

/// Controls when the vector similarity component flips from rewarding similarity
/// (layering mode) to rewarding dissimilarity (transition mode) as the intent
/// slider moves away from center.
///
/// The blend formula is: `similarity * (1 - t) + dissimilarity * t`
/// where `t = (|bias| / crossover).clamp(0, 1)`.
///
/// Lower crossover = flips earlier (small slider movement switches to transition mode).
/// Higher crossover = stays in similarity mode longer (slider must move further).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionBlendMode {
    /// Similarity dominates until slider is nearly at extreme (crossover at 0.9).
    /// Best for sets where most mixing is layering with occasional transitions.
    Layering,
    /// Balanced crossover at 0.6 — moderate slider movement starts favoring transitions.
    #[default]
    Balanced,
    /// Early crossover at 0.3 — even small slider movement favors dissimilar tracks.
    /// Best for sets with frequent energy changes and transitions.
    Transition,
}

impl SuggestionBlendMode {
    pub const ALL: [SuggestionBlendMode; 3] = [
        SuggestionBlendMode::Layering,
        SuggestionBlendMode::Balanced,
        SuggestionBlendMode::Transition,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            SuggestionBlendMode::Layering   => "Layering",
            SuggestionBlendMode::Balanced   => "Balanced",
            SuggestionBlendMode::Transition => "Transition",
        }
    }

    /// Crossover threshold: |bias| at which similarity fully transitions to dissimilarity.
    pub fn crossover(self) -> f32 {
        match self {
            SuggestionBlendMode::Layering   => 0.9,
            SuggestionBlendMode::Balanced   => 0.6,
            SuggestionBlendMode::Transition => 0.3,
        }
    }
}

/// Controls how far from the seed the vector similarity component reaches
/// at extreme slider positions (transitions). Instead of rewarding the MOST
/// dissimilar tracks (which produces jarring genre jumps), a bell curve
/// targets tracks at a specific distance — "adjacent community" territory.
///
/// - Tight: target 0.25 — transitions stay close, same genre but different style
/// - Medium: target 0.40 — adjacent community, slight genre bridge
/// - Open: target 0.60 — cross-genre, bold transitions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionTransitionReach {
    /// Stay close: transitions within the same genre cluster (target distance 0.25)
    Tight,
    /// Adjacent community: bridge to neighboring styles (target distance 0.40)
    #[default]
    Medium,
    /// Cross-genre: bold transitions to different genres (target distance 0.60)
    Open,
}

impl SuggestionTransitionReach {
    pub const ALL: [SuggestionTransitionReach; 3] = [
        SuggestionTransitionReach::Tight,
        SuggestionTransitionReach::Medium,
        SuggestionTransitionReach::Open,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            SuggestionTransitionReach::Tight  => "Tight",
            SuggestionTransitionReach::Medium => "Medium",
            SuggestionTransitionReach::Open   => "Open",
        }
    }

    /// Target normalized distance for the transition bell curve.
    /// Uses dynamic thresholds if available, otherwise falls back to hardcoded defaults.
    pub fn target_distance(self, dynamic: Option<&crate::graph_compute::CommunityThresholds>) -> f32 {
        match dynamic {
            Some(t) => match self {
                SuggestionTransitionReach::Tight  => t.tight_target,
                SuggestionTransitionReach::Medium => t.medium_target,
                SuggestionTransitionReach::Open   => t.open_target,
            },
            None => match self {
                SuggestionTransitionReach::Tight  => 0.25,
                SuggestionTransitionReach::Medium => 0.40,
                SuggestionTransitionReach::Open   => 0.60,
            },
        }
    }

    /// Target intensity shift at full peak/drop slider.
    /// Expressed as percentile-rank delta from seed (e.g., 0.15 = shift 15% of the library range).
    pub fn intensity_reach(self) -> f32 {
        match self {
            SuggestionTransitionReach::Tight  => 0.15,
            SuggestionTransitionReach::Medium => 0.30,
            SuggestionTransitionReach::Open   => 0.50,
        }
    }

    /// Width (2σ²) of the bell curve around the target distance.
    pub fn bell_width(self, dynamic: Option<&crate::graph_compute::CommunityThresholds>) -> f32 {
        match dynamic {
            Some(t) => match self {
                SuggestionTransitionReach::Tight  => t.tight_width,
                SuggestionTransitionReach::Medium => t.medium_width,
                SuggestionTransitionReach::Open   => t.open_width,
            },
            None => match self {
                SuggestionTransitionReach::Tight  => 0.08,
                SuggestionTransitionReach::Medium => 0.12,
                SuggestionTransitionReach::Open   => 0.18,
            },
        }
    }
}

/// Intensity matching mode for smart suggestions.
///
/// Controls how the intensity component compares seed vs candidate tracks.
/// Both modes use weighted Euclidean distance between the 10-dim ranked
/// component vectors — never a naive 1D composite sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntensityMatchMode {
    /// Match: per-component Euclidean distance at all slider positions.
    /// Center = similar character, extremes = each component shifted toward
    /// more/less aggressive. Pure 10D distance, no composite collapse.
    Match,
    /// Auto (default): per-component distance at center (match character),
    /// composite direction at extremes (match energy level).
    /// Blends between the two using the intent slider position.
    #[default]
    Auto,
}

impl IntensityMatchMode {
    pub const ALL: [IntensityMatchMode; 2] = [
        IntensityMatchMode::Match,
        IntensityMatchMode::Auto,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            IntensityMatchMode::Match => "Match",
            IntensityMatchMode::Auto  => "Auto",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::Match => Self::Auto,
            Self::Auto  => Self::Match,
        }
    }
}

/// Harmonic filter strictness for smart suggestions.
///
/// Controls which key relationships are allowed to appear at all.
/// Strict mode mirrors the original behaviour; Relaxed and Off progressively
/// open up atonal/experimental transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKeyFilter {
    /// Only compatible keys (same, adjacent, diagonal, mood shifts).
    /// Blocks semitone, far-step, and tritone moves. (default)
    #[default]
    Strict,
    /// Also allows semitone and cross-key moves for atonal/mashup mixing.
    Relaxed,
    /// No harmonic filter — all keys scored, nothing blocked outright.
    Off,
}

impl SuggestionKeyFilter {
    pub const ALL: [SuggestionKeyFilter; 3] = [
        SuggestionKeyFilter::Strict,
        SuggestionKeyFilter::Relaxed,
        SuggestionKeyFilter::Off,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            SuggestionKeyFilter::Strict  => "Strict",
            SuggestionKeyFilter::Relaxed => "Relaxed",
            SuggestionKeyFilter::Off     => "Off",
        }
    }

    /// Returns `(harmonic_floor, blended_threshold)`.
    ///
    /// `harmonic_floor`: minimum `base_score(TransitionType)` required to enter scoring.
    /// `blended_threshold`: minimum energy-direction-blended key score.
    pub fn thresholds(self) -> (f32, f32) {
        match self {
            SuggestionKeyFilter::Strict  => (0.45, 0.65),
            SuggestionKeyFilter::Relaxed => (0.20, 0.45),
            SuggestionKeyFilter::Off     => (0.00, 0.00),
        }
    }
}
