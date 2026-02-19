//! Embedded audio tag reading via lofty
//!
//! Reads ID3v2, Vorbis Comments, MP4 (iTunes), FLAC, and WAV RIFF tags
//! in a format-agnostic way. Safe to call from rayon threads.

use std::path::Path;

use lofty::file::TaggedFileExt;
use lofty::tag::Accessor;

/// Metadata extracted from embedded audio tags
pub struct EmbeddedTags {
    pub title: Option<String>,
    pub artist: Option<String>,
}

/// Read embedded tags from an audio file.
///
/// Returns `None` if the file can't be read or contains no useful metadata.
/// "Useful" means at least one of title/artist is a non-empty string.
pub fn read_embedded_tags(path: &Path) -> Option<EmbeddedTags> {
    let tagged_file = lofty::read_from_path(path).ok()?;

    // Try primary tag first (e.g., ID3v2 for MP3), fall back to any available tag
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag())?;

    let title = tag.title().and_then(|s| non_empty(s.to_string()));
    let artist = tag.artist().and_then(|s| non_empty(s.to_string()));

    // Only return if at least one field is populated
    if title.is_none() && artist.is_none() {
        return None;
    }

    Some(EmbeddedTags { title, artist })
}

/// Filter empty/whitespace-only strings to None
fn non_empty(s: String) -> Option<String> {
    let trimmed = s.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty_filters_whitespace() {
        assert!(non_empty("".to_string()).is_none());
        assert!(non_empty("   ".to_string()).is_none());
        assert_eq!(non_empty("hello".to_string()), Some("hello".to_string()));
        assert_eq!(non_empty("  hello  ".to_string()), Some("hello".to_string()));
    }

    #[test]
    fn read_nonexistent_file_returns_none() {
        assert!(read_embedded_tags(Path::new("/nonexistent/file.mp3")).is_none());
    }
}
