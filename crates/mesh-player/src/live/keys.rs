//! Keyboard fallback for developing live mode without the F1s.
//!
//! | Key | Action |
//! |---|---|
//! | `1`–`4` | select lane |
//! | `q` `w` `e` `r` `y` `u` `i` `o` | lane pads 1–8 (`t` is taken by Quant): drum voice mutes, or oct− oct+ half dbl form◀ form▶ |
//! | `Shift` + `i` / `o` (drums) | previous / next form |
//! | `Space` | lane on/off (lands on the bar unless Quant is off) |
//! | `↑` / `↓` | master seed ±1 (`Shift`: ±10) |
//! | `Backspace` | back to the previous seed |
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

use super::pads::{self, PadFn, PADS};

/// Step of the key-intent position per key press (range −1..=1).
pub const INTENT_STEP: f32 = 0.2;
/// Control step per key press, coarse and fine.
pub const CONTROL_STEP: (f32, f32) = (0.05, 0.01);

/// What a key does. Commands that need current state carry a delta.
pub enum KeyAction {
    SelectLane(u8),
    Command(LiveCommand),
    /// A lane pad (resolved like the F1 pad).
    Pad(PadFn),
    /// One seed edit of `delta`.
    SeedStep(i32),
    ControlDelta { control: u8, delta: f32 },
    IntentStep(f32),
    TempoDelta(f64),
    ToggleQuantize,
    ToggleRecording,
}

const MOVE_KEYS: [(char, MoveKind); 4] =
    [('a', MoveKind::Stutter), ('s', MoveKind::Fill), ('d', MoveKind::Double), ('f', MoveKind::Void)];
const PAD_KEYS: [char; PADS] = ['q', 'w', 'e', 'r', 'y', 'u', 'i', 'o'];
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
            Named::ArrowUp => KeyAction::SeedStep(seed_step),
            Named::ArrowDown => KeyAction::SeedStep(-seed_step),
            Named::Backspace if !repeat => C(LiveCommand::SwapSeed { lane }),
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
    if let Some(pad) = PAD_KEYS.iter().position(|&k| k == c) {
        return match pads::pad_fn(lane == 0, pad, shift) {
            PadFn::None => None,
            f => (!repeat).then_some(KeyAction::Pad(f)),
        };
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
    fn lanes_pads_and_toggles() {
        use mesh_core::live::generator::layout::DrumVoice;
        use mesh_core::live::state::Rate;
        assert!(matches!(map_key(&press(ch("3"), Modifiers::empty(), false), 0), Some(KeyAction::SelectLane(2))));
        assert!(map_key(&press(ch("5"), Modifiers::empty(), false), 0).is_none());
        assert!(matches!(
            map_key(&press(ch("e"), Modifiers::empty(), false), 0),
            Some(KeyAction::Pad(PadFn::Mute(DrumVoice::Hats)))
        ));
        assert!(matches!(map_key(&press(ch("e"), Modifiers::empty(), false), 1), Some(KeyAction::Pad(PadFn::Rate(Rate::Half)))));
        assert!(matches!(map_key(&press(ch("O"), Modifiers::SHIFT, false), 0), Some(KeyAction::Pad(PadFn::FormNext))));
        assert!(map_key(&press(ch("o"), Modifiers::empty(), false), 2).is_none(), "pad 8 is unused on pitched lanes");
        assert!(map_key(&press(ch("q"), Modifiers::empty(), true), 1).is_none(), "no key repeat on pads");
        assert!(matches!(
            map_key(&press(Key::Named(Named::Space), Modifiers::empty(), false), 2),
            Some(KeyAction::Command(LiveCommand::ToggleLane { lane: 2 }))
        ));
        assert!(map_key(&press(Key::Named(Named::Space), Modifiers::empty(), true), 2).is_none());
        assert!(matches!(map_key(&press(ch("t"), Modifiers::empty(), false), 0), Some(KeyAction::ToggleQuantize)));
    }

    #[test]
    fn seeds_moves_and_modifiers() {
        assert!(matches!(map_key(&press(Key::Named(Named::ArrowUp), Modifiers::SHIFT, false), 0), Some(KeyAction::SeedStep(10))));
        assert!(matches!(
            map_key(&press(Key::Named(Named::Backspace), Modifiers::empty(), false), 3),
            Some(KeyAction::Command(LiveCommand::SwapSeed { lane: 3 }))
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
