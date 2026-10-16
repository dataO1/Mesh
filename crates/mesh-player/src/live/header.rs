//! Header strip texts (design §8): key, tempo, half-time, lane chords,
//! phrase position, recording. Pure functions so the formats are tested.
//!
//! Numbers use fixed widths so the strip does not shift with a monospace font.

use std::time::Duration;

use mesh_core::live::harmony::camelot_label;
use mesh_core::live::state::ClockView;
use mesh_core::music::MusicalKey;

/// "8A Am".
pub fn key_label(key: MusicalKey) -> String {
    format!("{:>3} {}", camelot_label(key), key)
}

/// Current key and mode, plus what is armed for the next phrase.
pub fn key_text(current: MusicalKey, pending: Option<MusicalKey>, mode: &str, mode_pending: bool) -> String {
    let new_mode = if mode_pending { " → new mode" } else { "" };
    match pending {
        Some(p) if p != current => {
            format!("KEY {} {mode} → {}{new_mode} (next phrase)", key_label(current), key_label(p))
        }
        _ if mode_pending => format!("KEY {} {mode}{new_mode} (next phrase)", key_label(current)),
        _ => format!("KEY {} {mode}", key_label(current)),
    }
}

/// Key the intent slider points at, when it would change something.
pub fn intent_text(current: MusicalKey, pending: Option<MusicalKey>, intent: Option<MusicalKey>) -> Option<String> {
    let intent = intent?;
    (intent != pending.unwrap_or(current)).then(|| format!("intent {}", key_label(intent)))
}

/// "172.0 BPM", or "170.0 → 174.0 BPM" while ramping.
pub fn bpm_text(clock: &ClockView) -> String {
    if clock.ramping || (clock.target_bpm - clock.bpm).abs() >= 0.05 {
        format!("{:5.1} → {:5.1} BPM", clock.bpm, clock.target_bpm)
    } else {
        format!("{:5.1} BPM", clock.bpm)
    }
}

/// "½T off", "½T on", or "½T off → on" while a change waits for the bar.
pub fn half_time_text(current: bool, target: bool) -> String {
    let word = |on: bool| if on { "on" } else { "off" };
    if current == target {
        format!("½T {}", word(current))
    } else {
        format!("½T {} → {}", word(current), word(target))
    }
}

/// Upper-case roman numeral of a 1-based scale degree (1..=7).
pub fn roman(degree: u8) -> &'static str {
    match degree {
        1 => "I",
        2 => "II",
        3 => "III",
        4 => "IV",
        5 => "V",
        6 => "VI",
        7 => "VII",
        _ => "?",
    }
}

/// "chords VI · IV · I": the chord each pitched lane plays, in lane order.
/// Degrees are 0-based.
pub fn chords_text(chord_degrees: &[u8]) -> String {
    let romans: Vec<&str> = chord_degrees.iter().map(|&d| roman(d + 1)).collect();
    format!("chords {}", romans.join(" · "))
}

/// Which of the phrase's 4 bar cells are lit (bars played so far, inclusive).
pub fn phrase_cells(bar_in_phrase: u8) -> [bool; 4] {
    std::array::from_fn(|i| i as u8 <= bar_in_phrase)
}

/// "● REC 03:12" (minutes may exceed 59).
pub fn recording_text(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    format!("● REC {:02}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AM: MusicalKey = MusicalKey::new(9, true);
    const EM: MusicalKey = MusicalKey::new(4, true);
    const BM: MusicalKey = MusicalKey::new(11, true);

    fn clock(bpm: f64, target: f64, ramping: bool) -> ClockView {
        ClockView { bar: 0, step: 0, phrase: 0, bar_in_phrase: 0, bpm, target_bpm: target, ramping }
    }

    #[test]
    fn key_labels_and_pending_key() {
        assert_eq!(key_label(AM), " 8A Am");
        assert_eq!(key_label(MusicalKey::new(4, false)), "12B E");
        assert_eq!(key_text(AM, None, "aeolian", false), "KEY  8A Am aeolian");
        assert_eq!(key_text(AM, Some(AM), "aeolian", false), "KEY  8A Am aeolian");
        assert_eq!(key_text(AM, Some(EM), "dorian", false), "KEY  8A Am dorian →  9A Em (next phrase)");
        assert_eq!(key_text(AM, None, "dorian", true), "KEY  8A Am dorian → new mode (next phrase)");
    }

    #[test]
    fn intent_only_shows_a_change() {
        assert_eq!(intent_text(AM, None, None), None);
        assert_eq!(intent_text(AM, None, Some(AM)), None);
        assert_eq!(intent_text(AM, Some(EM), Some(EM)), None);
        assert_eq!(intent_text(AM, Some(EM), Some(BM)).as_deref(), Some("intent 10A Bm"));
    }

    #[test]
    fn bpm_shows_ramp_target() {
        assert_eq!(bpm_text(&clock(172.0, 172.0, false)), "172.0 BPM");
        assert_eq!(bpm_text(&clock(170.04, 174.0, true)), "170.0 → 174.0 BPM");
        assert_eq!(bpm_text(&clock(99.5, 99.5, false)), " 99.5 BPM");
    }

    #[test]
    fn half_time_chords_phrase_recording() {
        assert_eq!(half_time_text(false, false), "½T off");
        assert_eq!(half_time_text(false, true), "½T off → on");
        assert_eq!(chords_text(&[5, 3, 0]), "chords VI · IV · I");
        assert_eq!(phrase_cells(0), [true, false, false, false]);
        assert_eq!(phrase_cells(3), [true; 4]);
        assert_eq!(recording_text(Duration::from_secs(3725)), "● REC 62:05");
    }
}
