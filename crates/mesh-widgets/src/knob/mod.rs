//! Shader-based knob widget for audio applications
//!
//! A circular knob that displays:
//! - Current value as an arc
//! - Multiple modulation range indicators
//! - Interactive drag control
//!
//! Uses GPU shaders for smooth rendering without canvas conflicts.
//!
//! # Usage
//!
//! ```rust,ignore
//! // Create knob instances once (e.g., in your app state)
//! let mut knob = Knob::new(48.0);
//!
//! // Update value programmatically
//! knob.set_value(0.75);
//!
//! // In your view function
//! knob.view(|event| Message::KnobEvent(event))
//!
//! // In your update function
//! if let Some(new_value) = knob.handle_event(event, DEFAULT_SENSITIVITY) {
//!     // Value changed - do something with it
//! }
//! ```

mod pipeline;

use iced::widget::{mouse_area, shader};
use iced::{Element, Length, Point};
use std::sync::atomic::{AtomicU64, Ordering};

pub use pipeline::ModulationRange;
use pipeline::KnobProgram;

/// Global counter for generating unique knob IDs
static KNOB_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Default knob colors
pub mod colors {
    use iced::Color;

    pub const BACKGROUND: Color = Color::from_rgb(0.15, 0.15, 0.18);
    pub const TRACK: Color = Color::from_rgb(0.25, 0.25, 0.28);
    pub const VALUE: Color = Color::from_rgb(0.3, 0.7, 0.9);
    pub const NOTCH: Color = Color::from_rgb(0.9, 0.9, 0.9);

    // Modulation colors
    pub const MOD_1: Color = Color::from_rgb(0.9, 0.5, 0.2); // Orange
    pub const MOD_2: Color = Color::from_rgb(0.2, 0.9, 0.5); // Green
    pub const MOD_3: Color = Color::from_rgb(0.9, 0.2, 0.5); // Pink
    pub const MOD_4: Color = Color::from_rgb(0.5, 0.2, 0.9); // Purple
}

/// Messages emitted by a knob during interaction
#[derive(Debug, Clone)]
pub enum KnobEvent {
    /// Mouse button pressed on knob
    Pressed,
    /// Mouse button released
    Released,
    /// Mouse moved to position (for drag handling)
    Moved(Point),
}

/// A stateful knob widget with GPU-accelerated rendering
///
/// Each `Knob` instance has a stable ID that persists across frames,
/// allowing efficient GPU resource reuse.
#[derive(Debug, Clone)]
pub struct Knob {
    /// Stable unique ID (never changes after creation)
    id: u64,
    /// Current value (0.0 - 1.0) - the base/stored value
    value: f32,
    /// Display value for the indicator dot - if set, shows modulated position
    /// When None, the indicator shows at `value`
    display_value: Option<f32>,
    /// Whether currently being dragged
    dragging: bool,
    /// Last cursor position during drag
    last_drag_pos: Option<Point>,
    /// Size in pixels
    size: f32,
    /// Bipolar mode (value arc from center instead of min)
    bipolar: bool,
    /// Modulation ranges to display
    modulations: Vec<ModulationRange>,
    /// Background color
    bg_color: iced::Color,
    /// Track color (unfilled arc)
    track_color: iced::Color,
    /// Value color (filled arc)
    value_color: iced::Color,
    /// Notch/indicator color
    notch_color: iced::Color,
}

impl Default for Knob {
    fn default() -> Self {
        Self::new(48.0)
    }
}

impl Knob {
    /// Create a new knob with the specified size
    pub fn new(size: f32) -> Self {
        Self {
            id: KNOB_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            value: 0.5,
            display_value: None,
            dragging: false,
            last_drag_pos: None,
            size,
            bipolar: false,
            modulations: Vec::new(),
            bg_color: colors::BACKGROUND,
            track_color: colors::TRACK,
            value_color: colors::VALUE,
            notch_color: colors::NOTCH,
        }
    }

    /// Get the knob's stable ID
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Get the current value
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Set the current value (clamped to 0.0 - 1.0)
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    /// Check if the knob is being dragged
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Get the size
    pub fn size(&self) -> f32 {
        self.size
    }

    /// Set the size
    pub fn set_size(&mut self, size: f32) {
        self.size = size;
    }

    /// Set bipolar mode
    pub fn set_bipolar(&mut self, bipolar: bool) {
        self.bipolar = bipolar;
    }

    /// Set the value color
    pub fn set_value_color(&mut self, color: iced::Color) {
        self.value_color = color;
    }

    /// Set the background color
    pub fn set_bg_color(&mut self, color: iced::Color) {
        self.bg_color = color;
    }

    /// Set modulation ranges
    pub fn set_modulations(&mut self, mods: Vec<ModulationRange>) {
        self.modulations = mods.into_iter().take(4).collect();
    }

    /// Add a modulation range (max 4)
    pub fn add_modulation(&mut self, range: ModulationRange) {
        if self.modulations.len() < 4 {
            self.modulations.push(range);
        }
    }

    /// Clear all modulation ranges
    pub fn clear_modulations(&mut self) {
        self.modulations.clear();
    }

    /// Set the display value (where the indicator dot appears)
    /// Use this for showing modulated position while keeping base value separate
    pub fn set_display_value(&mut self, value: Option<f32>) {
        self.display_value = value.map(|v| v.clamp(0.0, 1.0));
    }

    /// Get the effective display value (display_value if set, otherwise value)
    pub fn effective_display_value(&self) -> f32 {
        self.display_value.unwrap_or(self.value)
    }

    /// Handle a knob event and return the new value if it changed
    ///
    /// Call this from your update function when you receive a `KnobEvent`.
    /// Returns `Some(new_value)` if the value changed, `None` otherwise.
    pub fn handle_event(&mut self, event: KnobEvent, sensitivity: f32) -> Option<f32> {
        match event {
            KnobEvent::Pressed => {
                self.dragging = true;
                self.last_drag_pos = None;
                None
            }
            KnobEvent::Released => {
                self.dragging = false;
                self.last_drag_pos = None;
                None
            }
            KnobEvent::Moved(position) => {
                if self.dragging {
                    if let Some(last_pos) = self.last_drag_pos {
                        // Calculate delta (up or right increases value)
                        let delta_y = last_pos.y - position.y; // Inverted: up = positive
                        let delta_x = position.x - last_pos.x;

                        // Use the larger delta
                        let delta = if delta_y.abs() > delta_x.abs() {
                            delta_y
                        } else {
                            delta_x
                        };

                        let old_value = self.value;
                        self.value = (self.value + delta * sensitivity).clamp(0.0, 1.0);
                        self.last_drag_pos = Some(position);

                        if (self.value - old_value).abs() > f32::EPSILON {
                            return Some(self.value);
                        }
                    } else {
                        self.last_drag_pos = Some(position);
                    }
                }
                None
            }
        }
    }

    /// Create the view Element for this knob
    ///
    /// The `on_event` callback receives `KnobEvent`s that should be passed
    /// to `handle_event` in your update function.
    pub fn view<'a, Message: Clone + 'a>(
        &self,
        on_event: impl Fn(KnobEvent) -> Message + 'a,
    ) -> Element<'a, Message> {
        let program = KnobProgram {
            id: self.id,
            value: self.value,
            display_value: self.effective_display_value(),
            dragging: self.dragging,
            bipolar: self.bipolar,
            modulations: self.modulations.clone(),
            bg_color: self.bg_color,
            track_color: self.track_color,
            value_color: self.value_color,
            notch_color: self.notch_color,
        };

        let shader_widget = shader(program)
            .width(Length::Fixed(self.size))
            .height(Length::Fixed(self.size));

        // Map shader messages (never emitted) to our Message type
        let on_press = on_event(KnobEvent::Pressed);
        let on_release = on_event(KnobEvent::Released);
        let fallback = on_press.clone();

        let shader_element: Element<'a, ()> = shader_widget.into();
        let mapped_element: Element<'a, Message> = shader_element.map(move |()| fallback.clone());

        // Wrap in mouse_area for interaction
        mouse_area(mapped_element)
            .on_press(on_press)
            .on_release(on_release)
            .on_move(move |pos| on_event(KnobEvent::Moved(pos)))
            .into()
    }
}

/// Default sensitivity for knob dragging (value change per pixel)
pub const DEFAULT_SENSITIVITY: f32 = 0.005;
