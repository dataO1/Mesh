//! Filename pattern parser for artist/title extraction
//!
//! Handles real-world naming conventions: UVR5 numeric prefixes, en/em dashes,
//! Bandcamp multi-segment names, track numbers, and artist connector normalization.

use std::collections::HashSet;

use regex::Regex;
use std::sync::LazyLock;

/// Confidence level of the filename parse result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseConfidence {
    /// Clean single-dash split or known-artist match
    High,
    /// Multi-dash with first-segment fallback
    Medium,
    /// No dash found — entire string is title
    Low,
}

/// Result of parsing a filename into artist/title components
#[derive(Debug, Clone)]
pub struct ParsedFilename {
    pub artist: Option<String>,
    pub title: String,
    pub confidence: ParseConfidence,
}

// ─── Compiled Regexes (lazily initialized, shared across threads) ────────────

/// UVR5 numeric prefix: `56_` at start of string
static RE_UVR5_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+_").unwrap());

/// Track number prefix: `01 - `, `A1 - `, `03.`, `1)`, etc.
static RE_TRACK_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z]?\d{1,2}\s*[-.)]\s*").unwrap());

/// Artist connectors: ` & `, ` x `, ` X `, ` feat. `, ` ft. `, ` vs. `, ` and `
/// Captures them as split points to normalize to comma-separated lists.
/// Uses word boundaries to avoid splitting inside words like "Alix" or "Maximal".
static RE_ARTIST_CONNECTOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(?:&|feat\.|ft\.|vs\.)\s+|\s+(?-i:x|X)\s+|\s+(?-i:and)\s+").unwrap()
});

/// `(Original Mix)` — carries no information, strip entirely
static RE_ORIGINAL_MIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\(Original Mix\)").unwrap());

/// Square brackets to convert to parentheses: `[anything]`
static RE_SQUARE_BRACKETS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]").unwrap());

/// Parse a filename into artist and title components.
///
/// Uses `known_artists` (lowercase-normalized) for multi-dash disambiguation.
pub fn parse_filename(filename: &str, known_artists: &HashSet<String>) -> ParsedFilename {
    let mut s = filename.to_string();

    // Step 1: Strip UVR5 prefix (e.g., `56_Artist - Title` → `Artist - Title`)
    s = RE_UVR5_PREFIX.replace(&s, "").to_string();

    // Step 2: Strip track number prefix (e.g., `01 - `, `A1 - `, `03.`)
    s = RE_TRACK_NUMBER.replace(&s, "").to_string();

    // Step 3: Normalize separators → standard ` - `
    s = s.replace(" – ", " - ");  // en dash
    s = s.replace(" — ", " - ");  // em dash
    s = s.replace("_-_", " - ");  // underscore separator

    // Step 4: Split on ` - `
    let segments: Vec<&str> = s.split(" - ").collect();

    let (raw_artist, raw_title, confidence) = match segments.len() {
        0 | 1 => {
            // No dash — entire string is title
            (None, s.trim().to_string(), ParseConfidence::Low)
        }
        2 => {
            // Clean single-dash split
            (
                Some(segments[0].trim().to_string()),
                segments[1].trim().to_string(),
                ParseConfidence::High,
            )
        }
        _ => {
            // Multi-dash: use known-artist disambiguation
            disambiguate_multi_dash(&segments, known_artists)
        }
    };

    // Step 5: Normalize artist connectors
    let artist = raw_artist.map(|a| normalize_artist(&a));

    // Step 6: Normalize title
    let title = normalize_title(&raw_title);

    ParsedFilename {
        artist,
        title,
        confidence,
    }
}

/// Disambiguate multi-dash filenames using known artists.
///
/// Tries progressively longer prefixes (joined with ` - `) against the known
/// artist set. If a match is found, the matched portion is the artist and the
/// remainder is the title. Otherwise, falls back to first segment = artist.
fn disambiguate_multi_dash(
    segments: &[&str],
    known_artists: &HashSet<String>,
) -> (Option<String>, String, ParseConfidence) {
    // Try 1 segment, then 2, etc. (up to n-1 so there's always a title)
    for prefix_len in 1..segments.len() {
        let candidate: String = segments[..prefix_len]
            .iter()
            .map(|s| s.trim())
            .collect::<Vec<_>>()
            .join(" - ");

        if known_artists.contains(&candidate.to_lowercase()) {
            let title = segments[prefix_len..]
                .iter()
                .map(|s| s.trim())
                .collect::<Vec<_>>()
                .join(" - ");
            return (Some(candidate), title, ParseConfidence::High);
        }
    }

    // No known artist match — first segment = artist, rest = title
    let artist = segments[0].trim().to_string();
    let title = segments[1..]
        .iter()
        .map(|s| s.trim())
        .collect::<Vec<_>>()
        .join(" - ");
    (Some(artist), title, ParseConfidence::Medium)
}

/// Normalize artist connector tokens to comma-separated list (public entry point).
///
/// Used by `mod.rs` to normalize tag-sourced artists too.
pub fn normalize_artist_public(artist: &str) -> String {
    normalize_artist(artist)
}

/// Normalize title (public entry point for tag-sourced titles).
pub fn normalize_title_public(title: &str) -> String {
    normalize_title(title)
}

/// Normalize artist connector tokens to comma-separated list.
///
/// `"Artist1 & Artist2"` → `"Artist1, Artist2"`
/// `"Artist1 feat. Artist2"` → `"Artist1, Artist2"`
fn normalize_artist(artist: &str) -> String {
    let parts: Vec<&str> = RE_ARTIST_CONNECTOR.split(artist).collect();
    if parts.len() <= 1 {
        return artist.trim().to_string();
    }
    parts
        .iter()
        .map(|s| s.trim())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Normalize title: brackets → parens, strip Original Mix, etc.
fn normalize_title(title: &str) -> String {
    let mut t = title.to_string();

    // Square brackets → parentheses
    t = RE_SQUARE_BRACKETS.replace_all(&t, "($1)").to_string();

    // Strip (Original Mix) — it's the default and carries no information
    t = RE_ORIGINAL_MIX.replace_all(&t, "").to_string();

    t.trim().to_string()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_known() -> HashSet<String> {
        HashSet::new()
    }

    fn known_with(artists: &[&str]) -> HashSet<String> {
        artists.iter().map(|s| s.to_lowercase()).collect()
    }

    // ── Basic artist - title ─────────────────────────────────────────────

    #[test]
    fn simple_artist_title() {
        let r = parse_filename("Noisia - Diplodocus", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Noisia"));
        assert_eq!(r.title, "Diplodocus");
        assert_eq!(r.confidence, ParseConfidence::High);
    }

    #[test]
    fn artist_title_with_remix() {
        let r = parse_filename("Noisia - Diplodocus (Phace Remix)", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Noisia"));
        assert_eq!(r.title, "Diplodocus (Phace Remix)");
    }

    // ── UVR5 prefix ─────────────────────────────────────────────────────

    #[test]
    fn uvr5_prefix_stripped() {
        let r = parse_filename("56_Noisia - Diplodocus", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Noisia"));
        assert_eq!(r.title, "Diplodocus");
    }

    // ── Track number prefix ─────────────────────────────────────────────

    #[test]
    fn track_number_01_dash() {
        let r = parse_filename("01 - Noisia - Diplodocus", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Noisia"));
        assert_eq!(r.title, "Diplodocus");
    }

    #[test]
    fn track_number_a1_dash() {
        let r = parse_filename("A1 - Artist - Title", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Artist"));
        assert_eq!(r.title, "Title");
    }

    // ── Combined UVR5 + track number ────────────────────────────────────

    #[test]
    fn uvr5_and_track_number() {
        let r = parse_filename("56_01 - Artist - Title", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Artist"));
        assert_eq!(r.title, "Title");
    }

    // ── Separator normalization ─────────────────────────────────────────

    #[test]
    fn en_dash_normalized() {
        let r = parse_filename("Artist \u{2013} Title", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Artist"));
        assert_eq!(r.title, "Title");
    }

    #[test]
    fn underscore_separator_normalized() {
        let r = parse_filename("Artist_-_Title", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Artist"));
        assert_eq!(r.title, "Title");
    }

    // ── Artist connectors ───────────────────────────────────────────────

    #[test]
    fn ampersand_connector() {
        let r = parse_filename("Artist1 & Artist2 - Title", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Artist1, Artist2"));
        assert_eq!(r.title, "Title");
    }

    #[test]
    fn x_connector() {
        let r = parse_filename("Artist1 x Artist2 - Title", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Artist1, Artist2"));
        assert_eq!(r.title, "Title");
    }

    #[test]
    fn feat_connector() {
        let r = parse_filename("Artist1 feat. Artist2 - Title", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Artist1, Artist2"));
        assert_eq!(r.title, "Title");
    }

    #[test]
    fn ft_connector() {
        let r = parse_filename("Artist1 ft. Artist2 - Title", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Artist1, Artist2"));
        assert_eq!(r.title, "Title");
    }

    // ── No dash (title only) ────────────────────────────────────────────

    #[test]
    fn no_dash_title_only() {
        let r = parse_filename("Just A Title", &empty_known());
        assert!(r.artist.is_none());
        assert_eq!(r.title, "Just A Title");
        assert_eq!(r.confidence, ParseConfidence::Low);
    }

    // ── Title normalization ─────────────────────────────────────────────

    #[test]
    fn original_mix_stripped() {
        let r = parse_filename("Artist - Title (Original Mix)", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Artist"));
        assert_eq!(r.title, "Title");
    }

    #[test]
    fn extended_mix_preserved() {
        let r = parse_filename("Artist - Title (Extended Mix)", &empty_known());
        assert_eq!(r.title, "Title (Extended Mix)");
    }

    #[test]
    fn square_brackets_to_parens() {
        let r = parse_filename("Artist - Title [DJ Edit]", &empty_known());
        assert_eq!(r.title, "Title (DJ Edit)");
    }

    #[test]
    fn square_brackets_remix_to_parens() {
        let r = parse_filename("Artist - Title [Phace Remix]", &empty_known());
        assert_eq!(r.title, "Title (Phace Remix)");
    }

    // ── Known-artist disambiguation ─────────────────────────────────────

    #[test]
    fn known_artist_multi_word() {
        let known = known_with(&["Black Sun Empire"]);
        let r = parse_filename("Black Sun Empire - Feed the Machine", &known);
        assert_eq!(r.artist.as_deref(), Some("Black Sun Empire"));
        assert_eq!(r.title, "Feed the Machine");
        assert_eq!(r.confidence, ParseConfidence::High);
    }

    #[test]
    fn known_artist_three_dashes() {
        let known = known_with(&["Black Sun Empire"]);
        let r = parse_filename("Black Sun Empire - Arrakis - Remix", &known);
        assert_eq!(r.artist.as_deref(), Some("Black Sun Empire"));
        assert_eq!(r.title, "Arrakis - Remix");
        assert_eq!(r.confidence, ParseConfidence::High);
    }

    #[test]
    fn unknown_artist_three_dashes_fallback() {
        let r = parse_filename("Unknown - Arrakis - Remix", &empty_known());
        assert_eq!(r.artist.as_deref(), Some("Unknown"));
        assert_eq!(r.title, "Arrakis - Remix");
        assert_eq!(r.confidence, ParseConfidence::Medium);
    }
}
