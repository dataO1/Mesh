//! On-screen keyboard widget for embedded touchscreen and MIDI encoder input.
//!
//! A QWERTY keyboard rendered as a grid of buttons. Supports:
//! - **Touch**: tap any key to type it
//! - **MIDI encoder**: linear traversal through keys (wrapping), press to activate
//!
//! ## Usage
//!
//! ```ignore
//! // In your app state:
//! keyboard: KeyboardState,
//!
//! // In your view (as a modal overlay):
//! keyboard_view(&self.keyboard, "Enter WiFi password")
//!     .map(Message::Keyboard)
//!
//! // In your update:
//! Message::Keyboard(msg) => {
//!     if let Some(event) = keyboard_handle(&mut self.keyboard, msg) {
//!         match event {
//!             KeyboardEvent::Submit(text) => { /* use the text */ }
//!             KeyboardEvent::Cancel => { /* user cancelled */ }
//!         }
//!     }
//! }
//! ```

use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Color, Element, Length};

// ── Key Layout ──

/// Definition of a single key on the keyboard.
#[derive(Debug, Clone, Copy)]
pub struct KeyDef {
    /// Character produced when unshifted
    pub normal: char,
    /// Character produced when shifted
    pub shifted: char,
    /// Relative width multiplier (1.0 = standard key)
    pub width: f32,
    /// Special key behavior (overrides character output)
    pub special: Option<SpecialKey>,
}

/// Special keys that don't produce characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialKey {
    Shift,
    Backspace,
    Space,
    Done,
}

impl KeyDef {
    const fn normal(ch: char) -> Self {
        Self {
            normal: ch,
            shifted: ch.to_ascii_uppercase(),
            width: 1.0,
            special: None,
        }
    }

    const fn symbol(normal: char, shifted: char) -> Self {
        Self { normal, shifted, width: 1.0, special: None }
    }

    const fn special(label: char, kind: SpecialKey, width: f32) -> Self {
        Self { normal: label, shifted: label, width, special: Some(kind) }
    }

    /// Display label for this key given current shift state.
    pub fn label(&self, shifted: bool) -> String {
        match self.special {
            Some(SpecialKey::Shift) => "\u{21E7}".to_string(),     // ⇧
            Some(SpecialKey::Backspace) => "\u{232B}".to_string(), // ⌫
            Some(SpecialKey::Space) => " ".to_string(),
            Some(SpecialKey::Done) => "Done".to_string(),
            None => {
                let ch = if shifted { self.shifted } else { self.normal };
                ch.to_string()
            }
        }
    }

    /// Character this key produces, or None for special keys.
    pub fn character(&self, shifted: bool) -> Option<char> {
        match self.special {
            Some(_) => None,
            None => Some(if shifted { self.shifted } else { self.normal }),
        }
    }
}

/// Row 0: numbers / symbols
pub const ROW_0: &[KeyDef] = &[
    KeyDef::symbol('1', '!'),
    KeyDef::symbol('2', '@'),
    KeyDef::symbol('3', '#'),
    KeyDef::symbol('4', '$'),
    KeyDef::symbol('5', '%'),
    KeyDef::symbol('6', '^'),
    KeyDef::symbol('7', '&'),
    KeyDef::symbol('8', '*'),
    KeyDef::symbol('9', '('),
    KeyDef::symbol('0', ')'),
];

/// Row 1: qwerty top row
pub const ROW_1: &[KeyDef] = &[
    KeyDef::normal('q'), KeyDef::normal('w'), KeyDef::normal('e'),
    KeyDef::normal('r'), KeyDef::normal('t'), KeyDef::normal('y'),
    KeyDef::normal('u'), KeyDef::normal('i'), KeyDef::normal('o'),
    KeyDef::normal('p'),
];

/// Row 2: home row
pub const ROW_2: &[KeyDef] = &[
    KeyDef::normal('a'), KeyDef::normal('s'), KeyDef::normal('d'),
    KeyDef::normal('f'), KeyDef::normal('g'), KeyDef::normal('h'),
    KeyDef::normal('j'), KeyDef::normal('k'), KeyDef::normal('l'),
];

/// Row 3: bottom row with shift and backspace
pub const ROW_3: &[KeyDef] = &[
    KeyDef::special('\u{21E7}', SpecialKey::Shift, 1.5),
    KeyDef::normal('z'), KeyDef::normal('x'), KeyDef::normal('c'),
    KeyDef::normal('v'), KeyDef::normal('b'), KeyDef::normal('n'),
    KeyDef::normal('m'),
    KeyDef::symbol(',', '<'),
    KeyDef::symbol('.', '>'),
    KeyDef::special('\u{232B}', SpecialKey::Backspace, 1.5),
];

/// Row 4: space bar and done
pub const ROW_4: &[KeyDef] = &[
    KeyDef::symbol('-', '_'),
    KeyDef::special(' ', SpecialKey::Space, 6.0),
    KeyDef::symbol('/', '?'),
    KeyDef::special('D', SpecialKey::Done, 2.0),
];

/// All rows in order.
pub const ROWS: &[&[KeyDef]] = &[ROW_0, ROW_1, ROW_2, ROW_3, ROW_4];

/// Total number of keys across all rows.
pub fn total_keys() -> usize {
    ROWS.iter().map(|r| r.len()).sum()
}

/// Look up a key by linear index (left→right, top→bottom).
pub fn key_at(index: usize) -> Option<(usize, usize, &'static KeyDef)> {
    let mut remaining = index;
    for (row_idx, row_keys) in ROWS.iter().enumerate() {
        if remaining < row_keys.len() {
            return Some((row_idx, remaining, &row_keys[remaining]));
        }
        remaining -= row_keys.len();
    }
    None
}

// ── State ──

/// On-screen keyboard state. Owns the text buffer and navigation position.
/// The consuming app owns an instance and passes it to view/handle functions.
#[derive(Debug, Clone)]
pub struct KeyboardState {
    /// Whether the keyboard is visible
    pub is_open: bool,
    /// Current text buffer
    pub text: String,
    /// Mask text display with dots (for passwords)
    pub masked: bool,
    /// Focused key index for MIDI encoder navigation (linear index)
    pub focused_key: usize,
    /// Shift key active
    pub shift_active: bool,
    /// Prompt text displayed above the keyboard (set by caller)
    pub prompt: String,
}

impl KeyboardState {
    pub fn new() -> Self {
        Self {
            is_open: false,
            text: String::new(),
            masked: false,
            focused_key: 0,
            shift_active: false,
            prompt: String::new(),
        }
    }

    /// Open the keyboard with a prompt and optional masking.
    pub fn open(&mut self, prompt: impl Into<String>, masked: bool) {
        self.is_open = true;
        self.text.clear();
        self.masked = masked;
        self.focused_key = 0;
        self.shift_active = false;
        self.prompt = prompt.into();
    }

    /// Close the keyboard and clear state.
    pub fn close(&mut self) {
        self.is_open = false;
        self.text.clear();
        self.shift_active = false;
        self.prompt.clear();
    }
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Messages & Events ──

/// Internal messages for keyboard interaction.
/// Use `.map()` to lift these into your app's message type.
#[derive(Debug, Clone)]
pub enum KeyboardMessage {
    /// A key was pressed (touch tap or MIDI select). Value is linear key index.
    KeyPress(usize),
    /// MIDI encoder scrolled — changes focused key.
    MidiScroll(i32),
    /// MIDI encoder pressed — activates the focused key.
    MidiSelect,
    /// Cancel button pressed.
    Cancel,
}

/// Events emitted by the keyboard for the consuming app to act on.
/// Returned from [`keyboard_handle`].
#[derive(Debug, Clone)]
pub enum KeyboardEvent {
    /// User pressed Done. Contains the final text.
    Submit(String),
    /// User pressed Cancel.
    Cancel,
}

// ── View ──

/// Key height in pixels (44px Apple HIG minimum, 48px for comfort).
const KEY_HEIGHT: f32 = 48.0;
/// Base key width for a 1.0-width key.
const KEY_WIDTH: f32 = 44.0;
/// Spacing between keys.
const KEY_SPACING: f32 = 4.0;

/// Render the on-screen keyboard as an `Element<KeyboardMessage>`.
///
/// The caller should wrap this in a modal overlay and `.map()` the message:
/// ```ignore
/// keyboard_view(&state, "Enter password").map(Message::Keyboard)
/// ```
pub fn keyboard_view(state: &KeyboardState) -> Element<'_, KeyboardMessage> {
    // Text display
    let display_text = if state.masked && !state.text.is_empty() {
        "\u{25CF}".repeat(state.text.len()) // ● dots
    } else if state.text.is_empty() {
        "".to_string()
    } else {
        state.text.clone()
    };

    let text_content = if display_text.is_empty() {
        text("Type here...").size(20).color(Color::from_rgba(0.5, 0.5, 0.5, 0.7))
    } else {
        text(display_text).size(20)
    };

    let text_display = container(text_content)
        .padding(12)
        .width(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(Color::from_rgba(0.1, 0.1, 0.12, 1.0).into()),
            border: iced::Border {
                color: Color::from_rgba(0.3, 0.3, 0.35, 1.0),
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        });

    // Prompt + cancel header
    let prompt_label = text(&state.prompt).size(13);
    let cancel_btn = button(text("Cancel").size(13))
        .on_press(KeyboardMessage::Cancel)
        .style(button::secondary);

    let header = row![
        prompt_label,
        Space::new().width(Length::Fill),
        cancel_btn,
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill);

    // Build keyboard rows
    let total = total_keys();
    let mut row_elements: Vec<Element<'_, KeyboardMessage>> = Vec::new();
    let mut linear_idx = 0;

    for row_keys in ROWS {
        let mut btns: Vec<Element<'_, KeyboardMessage>> = Vec::new();
        for key_def in *row_keys {
            let idx = linear_idx;
            let is_focused = state.focused_key == idx && idx < total;
            let label = key_def.label(state.shift_active);
            let is_shift = key_def.special == Some(SpecialKey::Shift);
            let shift_engaged = is_shift && state.shift_active;

            let btn_style = move |_theme: &iced::Theme, status: button::Status| {
                let base_bg = if shift_engaged {
                    Color::from_rgba(0.3, 0.5, 1.0, 0.4)
                } else {
                    match status {
                        button::Status::Hovered | button::Status::Pressed => {
                            Color::from_rgba(0.35, 0.35, 0.4, 1.0)
                        }
                        _ => Color::from_rgba(0.22, 0.22, 0.26, 1.0),
                    }
                };

                let (bg, border_color, border_width) = if is_focused {
                    (
                        Color::from_rgba(0.3, 0.5, 1.0, 0.5),
                        Color::from_rgba(0.4, 0.6, 1.0, 0.8),
                        2.0,
                    )
                } else {
                    (base_bg, Color::from_rgba(0.35, 0.35, 0.4, 0.6), 1.0)
                };

                button::Style {
                    background: Some(bg.into()),
                    text_color: Color::WHITE,
                    border: iced::Border {
                        color: border_color,
                        width: border_width,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                }
            };

            let width = KEY_WIDTH * key_def.width + KEY_SPACING * (key_def.width - 1.0);

            let btn = button(
                container(text(label).size(16))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .on_press(KeyboardMessage::KeyPress(idx))
            .style(btn_style)
            .width(Length::Fixed(width))
            .height(Length::Fixed(KEY_HEIGHT));

            btns.push(btn.into());
            linear_idx += 1;
        }

        row_elements.push(
            row(btns)
                .spacing(KEY_SPACING)
                .align_y(Alignment::Center)
                .into(),
        );
    }

    let keyboard_grid = column(row_elements)
        .spacing(KEY_SPACING)
        .align_x(Alignment::Center);

    let content = column![header, text_display, Space::new().height(8), keyboard_grid]
        .spacing(8)
        .width(Length::Shrink)
        .align_x(Alignment::Center);

    container(content)
        .padding(20)
        .style(container::rounded_box)
        .into()
}

// ── Handle ──

/// Process a keyboard message. Mutates state and optionally returns an event
/// for the consuming app to act on (submit or cancel).
pub fn keyboard_handle(
    state: &mut KeyboardState,
    msg: KeyboardMessage,
) -> Option<KeyboardEvent> {
    match msg {
        KeyboardMessage::KeyPress(idx) => {
            if let Some((_row, _col, key_def)) = key_at(idx) {
                match key_def.special {
                    Some(SpecialKey::Shift) => {
                        state.shift_active = !state.shift_active;
                    }
                    Some(SpecialKey::Backspace) => {
                        state.text.pop();
                    }
                    Some(SpecialKey::Space) => {
                        state.text.push(' ');
                    }
                    Some(SpecialKey::Done) => {
                        let result = state.text.clone();
                        state.close();
                        return Some(KeyboardEvent::Submit(result));
                    }
                    None => {
                        if let Some(ch) = key_def.character(state.shift_active) {
                            state.text.push(ch);
                            // Auto-disable shift after typing a character
                            state.shift_active = false;
                        }
                    }
                }
            }
            None
        }
        KeyboardMessage::MidiScroll(delta) => {
            let total = total_keys();
            if total > 0 {
                let current = state.focused_key;
                state.focused_key = if delta > 0 {
                    (current + 1) % total
                } else {
                    (current + total - 1) % total
                };
            }
            None
        }
        KeyboardMessage::MidiSelect => {
            // Activate the focused key
            let idx = state.focused_key;
            keyboard_handle(state, KeyboardMessage::KeyPress(idx))
        }
        KeyboardMessage::Cancel => {
            state.close();
            Some(KeyboardEvent::Cancel)
        }
    }
}
