//! Global key, chord progressions and key intent (design §4.6, D15, D22, D30).
//!
//! Generators emit scale degrees plus an octave. The renderer resolves them to
//! MIDI keys with the key active for that bar, so a key change re-voices every
//! lane without touching patterns. Key and progression changes are armed and
//! land on the first bar of the next phrase.
//!
//! Scales follow Camelot letters: minor = A (natural minor), major = B (major).
//!
//! Key intent maps a bipolar slider onto Mesh's energy-ordered Camelot moves
//! from [`crate::suggestions::scoring`], so every target is compatible by
//! construction.

use crate::music::MusicalKey;
use crate::suggestions::scoring::{classify_transition, transition_energy_direction, TransitionType};

use super::clock::BARS_PER_PHRASE;

/// Natural minor (Aeolian), semitones from the root. Camelot A.
pub const MINOR_SCALE: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];
/// Major (Ionian), semitones from the root. Camelot B.
pub const MAJOR_SCALE: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
/// Degrees per octave.
pub const SCALE_LEN: i32 = 7;

/// Maximum chords in one progression (fixed size keeps it `Copy`).
pub const MAX_PROGRESSION_LEN: usize = 16;

/// Scale table for a key's mode.
pub fn scale_for(key: MusicalKey) -> &'static [u8; 7] {
    if key.minor { &MINOR_SCALE } else { &MAJOR_SCALE }
}

/// Resolve a scale degree to a MIDI key in `key`.
///
/// - `degree`: 0-based scale degree (0 = root). Values outside 0..7 wrap into
///   neighbouring octaves (7 = root an octave up, -1 = 7th below).
/// - `octave`: scientific octave of the key's root (MIDI 60 = C4), so degree 0
///   at octave 4 in A minor is A4 = 69.
/// - `chord_offset`: 0-based degree of the current chord root, added to
///   `degree` (generators write chord-relative degrees).
///
/// Result is clamped to 0..=127. Pure and RT-safe.
pub fn resolve_degree(key: MusicalKey, degree: i32, octave: i32, chord_offset: i32) -> u8 {
    let d = degree + chord_offset;
    let oct_shift = d.div_euclid(SCALE_LEN);
    let idx = d.rem_euclid(SCALE_LEN) as usize;
    let midi = (octave + 1 + oct_shift) * 12 + key.root as i32 + scale_for(key)[idx] as i32;
    midi.clamp(0, 127) as u8
}

/// Build a key from its Camelot position (1..=12) and mode.
pub fn key_from_camelot(number: u8, minor: bool) -> Option<MusicalKey> {
    (0..12u8).map(|r| MusicalKey::new(r, minor)).find(|k| k.camelot().0 == number)
}

/// Camelot label, e.g. "8A".
pub fn camelot_label(key: MusicalKey) -> String {
    let (n, l) = key.camelot();
    format!("{n}{l}")
}

/// A chord progression in scale degrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progression {
    len: u8,
    /// 0-based chord-root degrees.
    degrees: [u8; MAX_PROGRESSION_LEN],
    /// Bars each chord lasts (>= 1).
    bars_per_chord: u8,
}

impl Progression {
    /// Build from 0-based degrees. Returns `None` when empty, too long, a
    /// degree is outside 0..7 or `bars_per_chord` is 0.
    pub fn new(degrees: &[u8], bars_per_chord: u8) -> Option<Self> {
        if degrees.is_empty()
            || degrees.len() > MAX_PROGRESSION_LEN
            || bars_per_chord == 0
            || degrees.iter().any(|&d| d as i32 >= SCALE_LEN)
        {
            return None;
        }
        let mut arr = [0u8; MAX_PROGRESSION_LEN];
        arr[..degrees.len()].copy_from_slice(degrees);
        Some(Self { len: degrees.len() as u8, degrees: arr, bars_per_chord })
    }

    /// Single-chord progression on the root.
    pub fn tonic() -> Self {
        Self { len: 1, degrees: [0; MAX_PROGRESSION_LEN], bars_per_chord: 1 }
    }

    pub fn degrees(&self) -> &[u8] {
        &self.degrees[..self.len as usize]
    }

    pub fn bars_per_chord(&self) -> u8 {
        self.bars_per_chord
    }

    /// Total bars before the progression repeats.
    pub fn cycle_bars(&self) -> u32 {
        self.len as u32 * self.bars_per_chord as u32
    }

    /// Chord degree for the `n`-th bar since the progression started.
    pub fn degree_at(&self, bars_since_start: u64) -> u8 {
        let idx = (bars_since_start / self.bars_per_chord as u64) % self.len as u64;
        self.degrees[idx as usize]
    }
}

impl Default for Progression {
    fn default() -> Self {
        Self::tonic()
    }
}

/// Harmony as seen by a generator for one bar (design §6.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonyCtx {
    pub key: MusicalKey,
    /// 0-based degree of this bar's chord root.
    pub chord_degree: u8,
    /// Index of the active progression in the style.
    pub progression: usize,
}

impl HarmonyCtx {
    pub fn scale(&self) -> &'static [u8; 7] {
        scale_for(self.key)
    }

    /// Resolve a chord-relative degree to a MIDI key.
    pub fn resolve(&self, degree: i32, octave: i32) -> u8 {
        resolve_degree(self.key, degree, octave, self.chord_degree as i32)
    }

    /// Resolve a key-relative degree (ignores the chord).
    pub fn resolve_in_key(&self, degree: i32, octave: i32) -> u8 {
        resolve_degree(self.key, degree, octave, 0)
    }
}

/// What [`HarmonyState::begin_bar`] applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HarmonyChange {
    pub key_changed: bool,
    pub progression_changed: bool,
}

/// Current key and progression, plus changes armed for the next phrase.
///
/// Owned by the bar renderer (non-RT). The renderer calls [`begin_bar`] for
/// each bar it renders, in order, then [`ctx`] for that bar.
///
/// [`begin_bar`]: HarmonyState::begin_bar
/// [`ctx`]: HarmonyState::ctx
#[derive(Debug, Clone, PartialEq)]
pub struct HarmonyState {
    key: MusicalKey,
    progression: usize,
    /// Bar at which the active progression started (its chord 0).
    progression_start_bar: u64,
    pending_key: Option<MusicalKey>,
    pending_progression: Option<usize>,
}

impl HarmonyState {
    pub fn new(key: MusicalKey, progression: usize) -> Self {
        Self {
            key,
            progression,
            progression_start_bar: 0,
            pending_key: None,
            pending_progression: None,
        }
    }

    pub fn key(&self) -> MusicalKey {
        self.key
    }

    pub fn progression(&self) -> usize {
        self.progression
    }

    pub fn pending_key(&self) -> Option<MusicalKey> {
        self.pending_key
    }

    pub fn pending_progression(&self) -> Option<usize> {
        self.pending_progression
    }

    /// Arm a key change for the next phrase. Arming the current key clears it.
    pub fn request_key(&mut self, key: MusicalKey) {
        self.pending_key = if key == self.key { None } else { Some(key) };
    }

    /// Arm a progression change for the next phrase.
    pub fn request_progression(&mut self, index: usize) {
        self.pending_progression = if index == self.progression { None } else { Some(index) };
    }

    pub fn cancel_pending(&mut self) {
        self.pending_key = None;
        self.pending_progression = None;
    }

    /// Enter `bar`. On a phrase start, applies armed changes. A changed
    /// progression restarts at its first chord on that bar.
    pub fn begin_bar(&mut self, bar: u64) -> HarmonyChange {
        let mut change = HarmonyChange::default();
        if bar % BARS_PER_PHRASE != 0 {
            return change;
        }
        if let Some(k) = self.pending_key.take() {
            change.key_changed = k != self.key;
            self.key = k;
        }
        if let Some(p) = self.pending_progression.take() {
            change.progression_changed = p != self.progression;
            self.progression = p;
            self.progression_start_bar = bar;
        }
        change
    }

    /// Harmony context for `bar`. `progressions` is the style's list; an out of
    /// range index falls back to a tonic progression.
    pub fn ctx(&self, bar: u64, progressions: &[Progression]) -> HarmonyCtx {
        let prog = progressions.get(self.progression).copied().unwrap_or_default();
        let since = bar.saturating_sub(self.progression_start_bar);
        HarmonyCtx { key: self.key, chord_degree: prog.degree_at(since), progression: self.progression }
    }
}

// ─── Seeded lane chords ────────────────────────────────────────────────

// MUSICAL: first draft, tune by ear. Chord-root weights per 0-based scale
// degree. The diminished chord (ii° in minor, vii° in major) never comes up.
const MINOR_CHORD_WEIGHTS: [f32; 7] = [3.0, 0.0, 2.0, 3.0, 2.0, 3.0, 3.0];
const MAJOR_CHORD_WEIGHTS: [f32; 7] = [3.0, 2.0, 1.0, 3.0, 3.0, 3.0, 0.0];
/// Extra weight of the tonic on a loop's first slot, so loops mostly start home.
const FIRST_SLOT_TONIC_BOOST: f32 = 12.0;
/// RNG stream of the chord draw (separate from every generator stream).
const S_CHORD: u32 = 0x50;

/// Chord root (0-based scale degree) of one chain slot of a pitched lane.
///
/// Every slot draws its chord from its own seed, so a lane's loop is its own
/// progression: reseeding a slot changes that bar's chord, reseeding the
/// lane changes them all. Stays diatonic in the active mode.
pub fn seeded_chord(seed: u32, lane: u8, slot: u8, minor: bool) -> u8 {
    let mut weights = if minor { MINOR_CHORD_WEIGHTS } else { MAJOR_CHORD_WEIGHTS };
    if slot == 0 {
        weights[0] += FIRST_SLOT_TONIC_BOOST;
    }
    let h = crate::live::rng::hash(seed, lane, slot, 0, S_CHORD);
    crate::live::rng::weighted_pick(h, &weights).unwrap_or(0) as u8
}

// ─── Key intent ────────────────────────────────────────────────────────

/// Up moves, closest to centre first (design §4.6).
pub const INTENT_UP: [TransitionType; 5] = [
    TransitionType::AdjacentUp,
    TransitionType::DiagonalUp,
    TransitionType::MoodLift,
    TransitionType::EnergyBoost,
    TransitionType::SemitoneUp,
];

/// Down moves, mirroring [`INTENT_UP`].
pub const INTENT_DOWN: [TransitionType; 5] = [
    TransitionType::AdjacentDown,
    TransitionType::DiagonalDown,
    TransitionType::MoodDarken,
    TransitionType::EnergyCool,
    TransitionType::SemitoneDown,
];

/// Slider zones per side, as |position| lower edges. `|x| < INTENT_ZONES[0]`
/// is the centre dead zone (stay). Zone `i` (from `INTENT_ZONES[i]` up to the
/// next edge) selects the `i`-th reachable move. From any key exactly 4 of the
/// 5 listed moves are reachable per side (diagonals and mood moves only exist
/// from one mode), so 4 zones.
// MUSICAL: tune on hardware (zone widths decide how easy each move is to hit).
pub const INTENT_ZONES: [f32; 4] = [0.15, 0.35, 0.55, 0.78];

/// One reachable key-intent move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntentMove {
    pub transition: TransitionType,
    pub key: MusicalKey,
    /// `transition_energy_direction` of the move.
    pub energy: f32,
}

/// The moves reachable from one key on one side, ordered by energy (gentlest
/// first). Fixed size, no allocation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntentMoves {
    len: usize,
    moves: [IntentMove; 5],
}

impl IntentMoves {
    pub fn as_slice(&self) -> &[IntentMove] {
        &self.moves[..self.len]
    }
}

/// Target picked by the key-intent slider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntentTarget {
    /// `None` = centre (stay).
    pub transition: Option<TransitionType>,
    pub key: MusicalKey,
}

/// The key that `transition` reaches from `from`, if any.
pub fn target_for(from: MusicalKey, transition: TransitionType) -> Option<MusicalKey> {
    (0..24u8)
        .map(|i| MusicalKey::new(i % 12, i >= 12))
        .find(|k| classify_transition(&from, k) == transition)
}

/// Reachable moves from `from`, upward (`up = true`) or downward, sorted by
/// absolute energy direction (gentlest first, design order breaks ties).
///
/// Note: `transition_energy_direction` rates DiagonalUp (+0.15) below
/// AdjacentUp (+0.20), so from a major key the first two up moves swap
/// relative to the design table. Energy order wins so the slider is monotonic.
pub fn intent_moves(from: MusicalKey, up: bool) -> IntentMoves {
    let list = if up { &INTENT_UP } else { &INTENT_DOWN };
    let placeholder = IntentMove { transition: TransitionType::SameKey, key: from, energy: 0.0 };
    let mut moves = [placeholder; 5];
    let mut len = 0;
    for &tt in list {
        if let Some(key) = target_for(from, tt) {
            moves[len] = IntentMove { transition: tt, key, energy: transition_energy_direction(tt) };
            len += 1;
        }
    }
    // Stable insertion sort by |energy|.
    for i in 1..len {
        let mut j = i;
        while j > 0 && moves[j - 1].energy.abs() > moves[j].energy.abs() {
            moves.swap(j - 1, j);
            j -= 1;
        }
    }
    IntentMoves { len, moves }
}

/// Map a slider position in `[-1, 1]` to a target key from `from`.
pub fn key_intent(from: MusicalKey, position: f32) -> IntentTarget {
    let stay = IntentTarget { transition: None, key: from };
    if !position.is_finite() {
        return stay;
    }
    let mag = position.abs().min(1.0);
    let zone = match INTENT_ZONES.iter().rposition(|&edge| mag >= edge) {
        Some(z) => z,
        None => return stay,
    };
    let moves = intent_moves(from, position > 0.0);
    let slice = moves.as_slice();
    match slice.get(zone.min(slice.len().saturating_sub(1))) {
        Some(m) => IntentTarget { transition: Some(m.transition), key: m.key },
        None => stay,
    }
}

/// Serde helper: store a [`MusicalKey`] as its name ("Am", "C#").
pub mod serde_key {
    use super::MusicalKey;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(key: &MusicalKey, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&key.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<MusicalKey, D::Error> {
        let s = String::deserialize(d)?;
        parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid key '{s}'")))
    }

    /// Parse a key name ("Am", "Bb") or Camelot label ("8A", "3B").
    pub fn parse(s: &str) -> Option<MusicalKey> {
        let t = s.trim();
        if let Some(letter) = t.chars().last() {
            let num = &t[..t.len() - letter.len_utf8()];
            if let Ok(n) = num.parse::<u8>() {
                return match letter.to_ascii_uppercase() {
                    'A' => super::key_from_camelot(n, true),
                    'B' => super::key_from_camelot(n, false),
                    _ => None,
                };
            }
        }
        MusicalKey::parse(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_keys() -> impl Iterator<Item = MusicalKey> {
        (0..24u8).map(|i| MusicalKey::new(i % 12, i >= 12))
    }

    #[test]
    fn resolve_all_24_keys() {
        for key in all_keys() {
            let scale = scale_for(key);
            for octave in 1..7 {
                for deg in 0..7 {
                    let midi = resolve_degree(key, deg, octave, 0);
                    let expect = (octave + 1) * 12 + key.root as i32 + scale[deg as usize] as i32;
                    assert_eq!(midi as i32, expect, "{key} deg {deg} oct {octave}");
                    // Pitch class always belongs to the key.
                    let pc = (midi as i32 - key.root as i32).rem_euclid(12) as u8;
                    assert!(scale.contains(&pc));
                }
                // Octave wrap: degree 7 = root one octave up, -7 = one down.
                let root = resolve_degree(key, 0, octave, 0);
                assert_eq!(resolve_degree(key, 7, octave, 0), root + 12);
                assert_eq!(resolve_degree(key, -7, octave, 0), root - 12);
            }
        }
    }

    #[test]
    fn seeded_chords_are_diatonic_and_varied() {
        for minor in [true, false] {
            let dim = if minor { 1 } else { 6 };
            let mut seen = [0u32; 7];
            let mut first_tonic = 0;
            for seed in 0..2000u32 {
                for slot in 0..8u8 {
                    let c = seeded_chord(seed, 1, slot, minor);
                    assert!(c < 7 && c != dim, "degree {c}");
                    assert_eq!(c, seeded_chord(seed, 1, slot, minor), "deterministic");
                    seen[c as usize] += 1;
                    if slot == 0 && c == 0 {
                        first_tonic += 1;
                    }
                }
            }
            assert_eq!(seen.iter().filter(|&&n| n > 0).count(), 6, "{seen:?}");
            assert!(first_tonic > 1000, "first slot mostly tonic: {first_tonic}");
        }
        // Lanes sharing a seed get different progressions.
        let differ = (0..100u32).filter(|&s| seeded_chord(s, 1, 2, true) != seeded_chord(s, 2, 2, true)).count();
        assert!(differ > 50);
    }

    #[test]
    fn resolve_known_notes() {
        let am = MusicalKey::new(9, true);
        assert_eq!(resolve_degree(am, 0, 4, 0), 69); // A4
        assert_eq!(resolve_degree(am, 2, 4, 0), 72); // C5
        assert_eq!(resolve_degree(am, -1, 4, 0), 67); // G4
        let c = MusicalKey::new(0, false);
        assert_eq!(resolve_degree(c, 0, 4, 0), 60);
        assert_eq!(resolve_degree(c, 2, 4, 0), 64);
        // Chord offset: degree 0 over chord VI (5) in A minor = F5.
        assert_eq!(resolve_degree(am, 0, 4, 5), 77);
        // Chord-relative fifth (degree 4) over iv (3) in A minor: D + fifth = A5.
        assert_eq!(resolve_degree(am, 4, 4, 3), 81);
    }

    #[test]
    fn resolve_clamps() {
        let b = MusicalKey::new(11, false);
        assert_eq!(resolve_degree(b, 100, 9, 0), 127);
        assert_eq!(resolve_degree(b, -100, -1, 0), 0);
    }

    #[test]
    fn camelot_round_trip() {
        for key in all_keys() {
            let (n, l) = key.camelot();
            assert_eq!(key_from_camelot(n, l == 'A'), Some(key));
            assert_eq!(serde_key::parse(&camelot_label(key)), Some(key));
            assert_eq!(serde_key::parse(&key.to_string()), Some(key));
        }
        assert_eq!(key_from_camelot(13, true), None);
        assert_eq!(serde_key::parse("8A"), Some(MusicalKey::new(9, true)));
        assert_eq!(serde_key::parse("8C"), None);
        assert_eq!(serde_key::parse("H"), None);
    }

    #[test]
    fn progression_degrees() {
        let p = Progression::new(&[0, 5, 3, 4], 1).unwrap();
        let seq: Vec<u8> = (0..8).map(|b| p.degree_at(b)).collect();
        assert_eq!(seq, [0, 5, 3, 4, 0, 5, 3, 4]);
        let p2 = Progression::new(&[0, 5], 2).unwrap();
        let seq: Vec<u8> = (0..6).map(|b| p2.degree_at(b)).collect();
        assert_eq!(seq, [0, 0, 5, 5, 0, 0]);
        assert_eq!(p2.cycle_bars(), 4);
        assert!(Progression::new(&[], 1).is_none());
        assert!(Progression::new(&[7], 1).is_none());
        assert!(Progression::new(&[0], 0).is_none());
        assert!(Progression::new(&[0; 17], 1).is_none());
    }

    #[test]
    fn pending_changes_wait_for_phrase() {
        let am = MusicalKey::new(9, true);
        let em = MusicalKey::new(4, true);
        let progs = [Progression::tonic(), Progression::new(&[0, 5, 3, 4], 1).unwrap()];
        let mut h = HarmonyState::new(am, 0);
        assert_eq!(h.begin_bar(0), HarmonyChange::default());
        h.begin_bar(1);
        h.request_key(em);
        h.request_progression(1);
        for bar in 2..4 {
            assert_eq!(h.begin_bar(bar), HarmonyChange::default());
            assert_eq!(h.ctx(bar, &progs).key, am);
            assert_eq!(h.ctx(bar, &progs).chord_degree, 0);
        }
        let c = h.begin_bar(4);
        assert!(c.key_changed && c.progression_changed);
        assert_eq!(h.key(), em);
        assert_eq!(h.pending_key(), None);
        // Progression restarts at the phrase.
        let degs: Vec<u8> = (4..9).map(|b| h.ctx(b, &progs).chord_degree).collect();
        assert_eq!(degs, [0, 5, 3, 4, 0]);
    }

    #[test]
    fn rerequesting_current_clears_pending() {
        let am = MusicalKey::new(9, true);
        let mut h = HarmonyState::new(am, 0);
        h.request_key(MusicalKey::new(4, true));
        h.request_key(am);
        assert_eq!(h.pending_key(), None);
        h.request_progression(2);
        h.cancel_pending();
        assert_eq!(h.pending_progression(), None);
    }

    #[test]
    fn out_of_range_progression_falls_back_to_tonic() {
        let h = HarmonyState::new(MusicalKey::new(0, false), 9);
        assert_eq!(h.ctx(3, &[]).chord_degree, 0);
    }

    #[test]
    fn harmony_ctx_resolves_against_chord() {
        let ctx = HarmonyCtx { key: MusicalKey::new(9, true), chord_degree: 5, progression: 0 };
        assert_eq!(ctx.resolve(0, 2), resolve_degree(ctx.key, 5, 2, 0));
        assert_eq!(ctx.resolve_in_key(0, 2), 45);
    }

    #[test]
    fn four_moves_per_side_from_every_key() {
        for key in all_keys() {
            for up in [true, false] {
                let m = intent_moves(key, up);
                assert_eq!(m.as_slice().len(), INTENT_ZONES.len(), "{key} up={up}");
                for mv in m.as_slice() {
                    assert_eq!(classify_transition(&key, &mv.key), mv.transition);
                    assert_ne!(mv.key, key);
                }
            }
        }
    }

    #[test]
    fn intent_is_monotonic_in_energy() {
        for key in all_keys() {
            let mut prev = f32::NEG_INFINITY;
            let mut x = -1.0f32;
            while x <= 1.0 {
                let t = key_intent(key, x);
                let e = t.transition.map(transition_energy_direction).unwrap_or(0.0);
                assert!(e >= prev, "{key} at {x}: {e} < {prev}");
                assert_eq!(t.transition.is_none(), x.abs() < INTENT_ZONES[0]);
                prev = e;
                x += 0.01;
            }
        }
    }

    #[test]
    fn intent_signs_and_extremes() {
        let am = MusicalKey::new(9, true); // 8A
        assert_eq!(key_intent(am, 0.0).key, am);
        assert_eq!(key_intent(am, f32::NAN).transition, None);
        let gentle_up = key_intent(am, 0.2);
        assert_eq!(gentle_up.transition, Some(TransitionType::AdjacentUp));
        assert_eq!(camelot_label(gentle_up.key), "9A");
        assert_eq!(key_intent(am, 1.0).transition, Some(TransitionType::SemitoneUp));
        assert_eq!(key_intent(am, 5.0).transition, Some(TransitionType::SemitoneUp));
        assert_eq!(key_intent(am, -1.0).transition, Some(TransitionType::SemitoneDown));
        // DiagonalDown (-0.15) is gentler than AdjacentDown (-0.20).
        assert_eq!(key_intent(am, -0.2).transition, Some(TransitionType::DiagonalDown));
        assert_eq!(key_intent(am, -0.4).transition, Some(TransitionType::AdjacentDown));
        // From minor: MoodLift is reachable upward.
        assert_eq!(key_intent(am, 0.4).transition, Some(TransitionType::MoodLift));
        assert_eq!(camelot_label(key_intent(am, 0.4).key), "8B");
        // From major: the gentlest up move is DiagonalUp (energy order).
        let c = MusicalKey::new(0, false);
        assert_eq!(key_intent(c, 0.2).transition, Some(TransitionType::DiagonalUp));
        assert_eq!(key_intent(c, -0.4).transition, Some(TransitionType::MoodDarken));
    }

    #[test]
    fn serde_key_in_yaml() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct W {
            #[serde(with = "serde_key")]
            k: MusicalKey,
        }
        let w = W { k: MusicalKey::new(1, true) };
        let y = serde_yaml::to_string(&w).unwrap();
        assert_eq!(serde_yaml::from_str::<W>(&y).unwrap(), w);
        assert!(serde_yaml::from_str::<W>("k: X#").is_err());
    }
}
