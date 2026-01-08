//! Music theory utilities for key matching
//!
//! Provides key parsing, semitone calculations, and relative key detection
//! for automatic harmonic mixing.

/// Musical key with root note and scale
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MusicalKey {
    /// Root note as semitone offset from C (0=C, 1=C#, 2=D, ..., 11=B)
    pub root: u8,
    /// true = minor, false = major
    pub minor: bool,
}

impl MusicalKey {
    /// Create a new musical key
    pub const fn new(root: u8, minor: bool) -> Self {
        Self {
            root: root % 12,
            minor,
        }
    }

    /// Parse key string like "Am", "C#m", "F", "Bb"
    ///
    /// Supported formats:
    /// - Single letter: C, D, E, F, G, A, B
    /// - With sharp: C#, D#, F#, G#, A#
    /// - With flat: Db, Eb, Gb, Ab, Bb
    /// - Minor suffix: Am, C#m, Bbm
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        let mut chars = s.chars().peekable();

        // Parse root note
        let root_char = chars.next()?.to_ascii_uppercase();
        let base_root = match root_char {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => return None,
        };

        // Check for sharp or flat
        let root = match chars.peek() {
            Some('#') => {
                chars.next();
                (base_root + 1) % 12
            }
            Some('b') => {
                chars.next();
                (base_root + 11) % 12 // +11 is same as -1 mod 12
            }
            _ => base_root,
        };

        // Check for minor suffix
        let remaining: String = chars.collect();
        let minor = remaining.to_lowercase().starts_with('m')
            || remaining.to_lowercase().contains("min");

        Some(Self { root, minor })
    }

    /// Get the relative major/minor key
    ///
    /// For minor keys: relative major is 3 semitones up
    /// For major keys: relative minor is 3 semitones down
    pub fn relative(&self) -> Self {
        if self.minor {
            // Minor -> Major: go up 3 semitones
            Self {
                root: (self.root + 3) % 12,
                minor: false,
            }
        } else {
            // Major -> Minor: go down 3 semitones
            Self {
                root: (self.root + 9) % 12, // +9 is same as -3 mod 12
                minor: true,
            }
        }
    }

    /// Get the Camelot wheel position (1-12, A/B)
    ///
    /// A = minor keys, B = major keys
    /// The number represents position on the circle of fifths
    pub fn camelot(&self) -> (u8, char) {
        // Camelot wheel mapping
        // Major keys (B): C=8, G=9, D=10, A=11, E=12, B=1, F#=2, Db=3, Ab=4, Eb=5, Bb=6, F=7
        // Minor keys (A): Am=8, Em=9, Bm=10, F#m=11, C#m=12, G#m=1, D#m=2, Bbm=3, Fm=4, Cm=5, Gm=6, Dm=7
        let camelot_major = [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1]; // Index by root (0=C, 1=C#, etc.)
        let camelot_minor = [5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10];

        let position = if self.minor {
            camelot_minor[self.root as usize]
        } else {
            camelot_major[self.root as usize]
        };

        let letter = if self.minor { 'A' } else { 'B' };
        (position, letter)
    }

    /// Convert to string representation
    pub fn to_string(&self) -> String {
        let note_names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        let note = note_names[self.root as usize];
        if self.minor {
            format!("{}m", note)
        } else {
            note.to_string()
        }
    }
}

impl std::fmt::Display for MusicalKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

/// Check if two keys are compatible (same key or relative major/minor)
///
/// Compatible keys share the same notes and don't need transposition
pub fn are_compatible(key1: &MusicalKey, key2: &MusicalKey) -> bool {
    // Same key
    if key1 == key2 {
        return true;
    }

    // Check if one is the relative of the other
    key1.relative() == *key2
}

/// Calculate semitones to transpose from_key to match to_key
///
/// Returns 0 if keys are compatible (same or relative)
/// Returns smallest interval (-6 to +6 semitones)
pub fn semitones_to_match(from_key: &MusicalKey, to_key: &MusicalKey) -> i8 {
    // If compatible, no transposition needed
    if are_compatible(from_key, to_key) {
        return 0;
    }

    // Calculate the root difference
    // We want to shift from_key's root to match to_key's root
    // But we also need to consider that minor and major have different "feels"

    // For matching, we transpose to the same root note
    // If from_key is minor and to_key is major (or vice versa), we match to the relative
    let target_root = if from_key.minor == to_key.minor {
        // Same scale type: match roots directly
        to_key.root
    } else {
        // Different scale types: match to relative
        // This makes Am match to C (relative major) rather than A major
        to_key.relative().root
    };

    // Calculate semitone difference (positive = up, negative = down)
    let diff = (target_root as i8) - (from_key.root as i8);

    // Normalize to -6..+6 range (prefer smaller intervals)
    if diff > 6 {
        diff - 12
    } else if diff < -6 {
        diff + 12
    } else {
        diff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_major_keys() {
        assert_eq!(MusicalKey::parse("C"), Some(MusicalKey::new(0, false)));
        assert_eq!(MusicalKey::parse("G"), Some(MusicalKey::new(7, false)));
        assert_eq!(MusicalKey::parse("F#"), Some(MusicalKey::new(6, false)));
        assert_eq!(MusicalKey::parse("Bb"), Some(MusicalKey::new(10, false)));
        assert_eq!(MusicalKey::parse("Db"), Some(MusicalKey::new(1, false)));
    }

    #[test]
    fn test_parse_minor_keys() {
        assert_eq!(MusicalKey::parse("Am"), Some(MusicalKey::new(9, true)));
        assert_eq!(MusicalKey::parse("Em"), Some(MusicalKey::new(4, true)));
        assert_eq!(MusicalKey::parse("C#m"), Some(MusicalKey::new(1, true)));
        assert_eq!(MusicalKey::parse("Bbm"), Some(MusicalKey::new(10, true)));
        assert_eq!(MusicalKey::parse("F#m"), Some(MusicalKey::new(6, true)));
    }

    #[test]
    fn test_relative_keys() {
        // Am <-> C
        let am = MusicalKey::parse("Am").unwrap();
        let c = MusicalKey::parse("C").unwrap();
        assert_eq!(am.relative(), c);
        assert_eq!(c.relative(), am);

        // Em <-> G
        let em = MusicalKey::parse("Em").unwrap();
        let g = MusicalKey::parse("G").unwrap();
        assert_eq!(em.relative(), g);
        assert_eq!(g.relative(), em);
    }

    #[test]
    fn test_compatibility() {
        let am = MusicalKey::parse("Am").unwrap();
        let c = MusicalKey::parse("C").unwrap();
        let em = MusicalKey::parse("Em").unwrap();
        let g = MusicalKey::parse("G").unwrap();

        // Same key is compatible
        assert!(are_compatible(&am, &am));
        assert!(are_compatible(&c, &c));

        // Relative keys are compatible
        assert!(are_compatible(&am, &c));
        assert!(are_compatible(&c, &am));
        assert!(are_compatible(&em, &g));

        // Non-relative keys are not compatible
        assert!(!are_compatible(&am, &em));
        assert!(!are_compatible(&c, &g));
    }

    #[test]
    fn test_semitones_to_match() {
        let am = MusicalKey::parse("Am").unwrap();
        let c = MusicalKey::parse("C").unwrap();
        let em = MusicalKey::parse("Em").unwrap();
        let bm = MusicalKey::parse("Bm").unwrap();

        // Compatible keys need no transposition
        assert_eq!(semitones_to_match(&am, &c), 0);
        assert_eq!(semitones_to_match(&c, &am), 0);

        // Am to Em: A(9) to E(4) = -5 or +7, prefer -5
        assert_eq!(semitones_to_match(&am, &em), -5);

        // Em to Am: E(4) to A(9) = +5
        assert_eq!(semitones_to_match(&em, &am), 5);

        // Am to Bm: A(9) to B(11) = +2
        assert_eq!(semitones_to_match(&am, &bm), 2);
    }

    #[test]
    fn test_camelot() {
        let am = MusicalKey::parse("Am").unwrap();
        let c = MusicalKey::parse("C").unwrap();

        // Am and C are both Camelot 8 (A and B respectively)
        assert_eq!(am.camelot(), (8, 'A'));
        assert_eq!(c.camelot(), (8, 'B'));
    }

    #[test]
    fn test_to_string() {
        assert_eq!(MusicalKey::parse("Am").unwrap().to_string(), "Am");
        assert_eq!(MusicalKey::parse("C").unwrap().to_string(), "C");
        assert_eq!(MusicalKey::parse("F#m").unwrap().to_string(), "F#m");
        assert_eq!(MusicalKey::parse("Bb").unwrap().to_string(), "A#"); // Normalized to sharps
    }
}
