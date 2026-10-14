//! Keyboard fallback for developing live mode without the F1s.
//!
//! | Key | Action |
//! |---|---|
//! | `1`–`4` | select lane |
//! | `q` `w` `e` `r` `y` `u` `i` `o` | select chain slot (bar) 1–8 (`t` is taken by Quant) |
//! | `Shift` + slot key | loop length of the lane: largest of 1, 2, 4, 8 ≤ the bar |
//! | `Space` | lane on/off (lands on the bar unless Quant is off) |
//! | `↑` / `↓` | seed ±1 of the selected slot (`Shift`: ±10) |
//! | `←` / `→` | browse kit / instrument option, `Enter` loads it at the bar |
//! | `a` `s` `d` `f` (hold) | stutter · fill · double rate · void |
//! | `z`/`x` `c`/`v` `b`/`n` `m`/`,` | control 1–4 down / up (`Shift`: fine) |
//! | `k` / `l` | key intent left / right, `j` commits it for the next phrase |
//! | `-` / `=` | tempo −1 / +1 BPM (ramped) |
//! | `h` | half-time toggle |
//! | `t` | quantized lane toggles on/off |
//! | `g` / `Shift+g` | capture selected lane / whole state |
//! | `Ctrl+r` | start / stop set recording |

use iced::keyboard::{key::Named, Event, Key, Modifiers};

use mesh_core::live::capture::CaptureScope;
use mesh_core::live::command::{LiveCommand, MoveKind};
use mesh_core::live::{snap_loop_len, SLOTS_PER_CHAIN};

/// Step of the key-intent position per key press (range −1..=1).
pub const INTENT_STEP: f32 = 0.2;
/// Control step per key press, coarse and fine.
pub const CONTROL_STEP: (f32, f32) = (0.05, 0.01);

/// What a key does. Commands that need current state carry a delta.
pub enum KeyAction {
    SelectLane(u8),
    Command(LiveCommand),
    ControlDelta { control: u8, delta: f32 },
    IntentStep(f32),
    TempoDelta(f64),
    ToggleQuantize,
    ToggleRecording,
}

const MOVE_KEYS: [(char, MoveKind); 4] =
    [('a', MoveKind::Stutter), ('s', MoveKind::Fill), ('d', MoveKind::Double), ('f', MoveKind::Void)];
const SLOT_KEYS: [char; SLOTS_PER_CHAIN] = ['q', 'w', 'e', 'r', 'y', 'u', 'i', 'o'];
const CONTROL_KEYS: [(char, char); 4] = [('z', 'x'), ('c', 'v'), ('b', 'n'), ('m', ',')];

fn char_of(key: &Key) -> Option<char> {
    match key {
        Key::Character(s) => s.chars().next().map(|c| c.to_ascii_lowercase()),
        Key::Named(Named::Space) => Some(' '),
        _ => None,
    }
}

/// Map a keyboard event for the selected `lane`.
pub fn map_key(event: &Event, lane: u8) -> Option<KeyAction> {
    match event {
        Event::KeyPressed { key, modifiers, repeat, .. } => pressed(key, *modifiers, *repeat, lane),
        Event::KeyReleased { key, .. } => {
            let c = char_of(key)?;
            let (_, kind) = MOVE_KEYS.iter().find(|(k, _)| *k == c)?;
            Some(KeyAction::Command(LiveCommand::MoveRelease { lane, kind: *kind }))
        }
        Event::ModifiersChanged(_) => None,
    }
}

fn pressed(key: &Key, mods: Modifiers, repeat: bool, lane: u8) -> Option<KeyAction> {
    use KeyAction::Command as C;
    let shift = mods.shift();
    if let Key::Named(named) = key {
        let seed_step = if shift { 10 } else { 1 };
        return Some(match named {
            Named::ArrowUp => C(LiveCommand::SeedDelta { lane, delta: seed_step }),
            Named::ArrowDown => C(LiveCommand::SeedDelta { lane, delta: -seed_step }),
            Named::ArrowLeft => C(LiveCommand::BrowseOption { lane, delta: -1 }),
            Named::ArrowRight => C(LiveCommand::BrowseOption { lane, delta: 1 }),
            Named::Enter => C(LiveCommand::CommitOption { lane }),
            Named::Space if !repeat => C(LiveCommand::ToggleLane { lane }),
            _ => return None,
        });
    }
    let c = char_of(key)?;
    if mods.control() {
        return (c == 'r' && !repeat).then_some(KeyAction::ToggleRecording);
    }
    if let Some(d) = c.to_digit(10).filter(|d| (1..=4).contains(d)) {
        return Some(KeyAction::SelectLane(d as u8 - 1));
    }
    if let Some(slot) = SLOT_KEYS.iter().position(|&k| k == c) {
        if shift {
            return (!repeat).then_some(C(LiveCommand::SetLoopLen { lane, len: snap_loop_len(slot + 1) }));
        }
        return Some(C(LiveCommand::SelectSlot { lane, slot: slot as u8 }));
    }
    if let Some((_, kind)) = MOVE_KEYS.iter().find(|(k, _)| *k == c) {
        return (!repeat).then_some(C(LiveCommand::MovePress { lane, kind: *kind }));
    }
    let step = if shift { CONTROL_STEP.1 } else { CONTROL_STEP.0 };
    for (i, (down, up)) in CONTROL_KEYS.iter().enumerate() {
        if c == *down || c == *up {
            let delta = if c == *up { step } else { -step };
            return Some(KeyAction::ControlDelta { control: i as u8, delta });
        }
    }
    if repeat && !matches!(c, '-' | '=' | 'k' | 'l') {
        return None;
    }
    Some(match c {
        'k' => KeyAction::IntentStep(-INTENT_STEP),
        'l' => KeyAction::IntentStep(INTENT_STEP),
        'j' => C(LiveCommand::CommitKey),
        '-' => KeyAction::TempoDelta(-1.0),
        '=' => KeyAction::TempoDelta(1.0),
        'h' => C(LiveCommand::ToggleHalfTime),
        't' => KeyAction::ToggleQuantize,
        'g' => C(LiveCommand::Capture(if shift { CaptureScope::All } else { CaptureScope::Lane { lane } })),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{key::Physical, Location};

    fn press(key: Key, modifiers: Modifiers, repeat: bool) -> Event {
        Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers,
            text: None,
            repeat,
        }
    }

    fn ch(c: &str) -> Key {
        Key::Character(c.into())
    }

    #[test]
    fn lanes_slots_and_toggles() {
        assert!(matches!(map_key(&press(ch("3"), Modifiers::empty(), false), 0), Some(KeyAction::SelectLane(2))));
        assert!(map_key(&press(ch("5"), Modifiers::empty(), false), 0).is_none());
        assert!(matches!(
            map_key(&press(ch("e"), Modifiers::empty(), false), 1),
            Some(KeyAction::Command(LiveCommand::SelectSlot { lane: 1, slot: 2 }))
        ));
        assert!(matches!(
            map_key(&press(Key::Named(Named::Space), Modifiers::empty(), false), 2),
            Some(KeyAction::Command(LiveCommand::ToggleLane { lane: 2 }))
        ));
        assert!(map_key(&press(Key::Named(Named::Space), Modifiers::empty(), true), 2).is_none());
    }

    #[test]
    fn eight_slot_keys_and_shift_sets_loop_length() {
        for (i, k) in ["q", "w", "e", "r", "y", "u", "i", "o"].iter().enumerate() {
            assert!(
                matches!(map_key(&press(ch(k), Modifiers::empty(), false), 2),
                    Some(KeyAction::Command(LiveCommand::SelectSlot { lane: 2, slot })) if slot as usize == i),
                "{k}"
            );
        }
        for (k, want) in [("Q", 1), ("W", 2), ("E", 2), ("R", 4), ("Y", 4), ("I", 4), ("O", 8)] {
            assert!(
                matches!(map_key(&press(ch(k), Modifiers::SHIFT, false), 1),
                    Some(KeyAction::Command(LiveCommand::SetLoopLen { lane: 1, len })) if len == want),
                "{k}"
            );
        }
        assert!(matches!(map_key(&press(ch("t"), Modifiers::empty(), false), 0), Some(KeyAction::ToggleQuantize)));
    }

    #[test]
    fn seeds_moves_and_modifiers() {
        assert!(matches!(
            map_key(&press(Key::Named(Named::ArrowUp), Modifiers::SHIFT, false), 0),
            Some(KeyAction::Command(LiveCommand::SeedDelta { lane: 0, delta: 10 }))
        ));
        assert!(matches!(
            map_key(&press(ch("S"), Modifiers::SHIFT, false), 3),
            Some(KeyAction::Command(LiveCommand::MovePress { lane: 3, kind: MoveKind::Fill }))
        ));
        let release = Event::KeyReleased {
            key: ch("a"),
            modified_key: ch("a"),
            physical_key: Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
        };
        assert!(matches!(
            map_key(&release, 1),
            Some(KeyAction::Command(LiveCommand::MoveRelease { lane: 1, kind: MoveKind::Stutter }))
        ));
        assert!(matches!(map_key(&press(ch("r"), Modifiers::CTRL, false), 0), Some(KeyAction::ToggleRecording)));
        assert!(matches!(
            map_key(&press(ch("G"), Modifiers::SHIFT, false), 0),
            Some(KeyAction::Command(LiveCommand::Capture(CaptureScope::All)))
        ));
        assert!(matches!(
            map_key(&press(ch(","), Modifiers::empty(), false), 0),
            Some(KeyAction::ControlDelta { control: 3, delta }) if delta == CONTROL_STEP.0
        ));
    }
}
