//! Rotary knob widget for mesh DJ applications
//!
//! A canvas-based circular knob control that can be dragged up/down to adjust value.
//! Used for effect parameters in stem chains.

use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Cache, Canvas, Event, Frame, Geometry, Path, Program, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Size, Theme};

/// State for a rotary knob (holds drag state and cache)
#[derive(Debug, Default)]
pub struct RotaryKnobState {
    cache: Cache,
}

impl RotaryKnobState {
    /// Create a new rotary knob state
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the rendering cache (call when value changes externally)
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

/// Canvas interaction state for drag tracking
#[derive(Debug, Clone, Copy, Default)]
pub struct RotaryKnobInteraction {
    /// Whether currently dragging
    pub is_dragging: bool,
    /// Y position when drag started
    pub drag_start_y: f32,
    /// Value when drag started
    pub drag_start_value: f32,
}

/// View struct for the rotary knob canvas program
pub struct RotaryKnobCanvas<'a, Message, F>
where
    F: Fn(f32) -> Message,
{
    state: &'a RotaryKnobState,
    value: f32,
    label: Option<&'a str>,
    on_change: F,
}

impl<'a, Message, F> Program<Message> for RotaryKnobCanvas<'a, Message, F>
where
    Message: Clone,
    F: Fn(f32) -> Message,
{
    type State = RotaryKnobInteraction;

    fn update(
        &self,
        interaction: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(pos) = cursor.position_in(bounds) {
                    interaction.is_dragging = true;
                    interaction.drag_start_y = pos.y;
                    interaction.drag_start_value = self.value;
                    return Some(canvas::Action::request_redraw());
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if interaction.is_dragging {
                    interaction.is_dragging = false;
                    return Some(canvas::Action::request_redraw());
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if interaction.is_dragging {
                    if let Some(pos) = cursor.position_in(bounds) {
                        // Drag up = increase, drag down = decrease
                        // Sensitivity: 100px of drag = full range
                        let delta_y = interaction.drag_start_y - pos.y;
                        let delta_value = delta_y / 100.0;
                        let new_value = (interaction.drag_start_value + delta_value).clamp(0.0, 1.0);

                        if (new_value - self.value).abs() > 0.001 {
                            return Some(canvas::Action::publish((self.on_change)(new_value)));
                        }
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn draw(
        &self,
        _interaction: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let geometry = self.state.cache.draw(renderer, bounds.size(), |frame| {
            self.draw_knob(frame, bounds.size());
        });
        vec![geometry]
    }
}

impl<'a, Message, F> RotaryKnobCanvas<'a, Message, F>
where
    F: Fn(f32) -> Message,
{
    /// Draw the rotary knob
    fn draw_knob(&self, frame: &mut Frame, size: Size) {
        let knob_size = size.height.min(size.width);
        let center = Point::new(knob_size / 2.0, knob_size / 2.0);
        let radius = knob_size / 2.0 - 2.0;

        // Background circle
        let background = Path::circle(center, radius);
        frame.fill(&background, Color::from_rgb(0.15, 0.15, 0.15));

        // Border
        frame.stroke(
            &background,
            Stroke::default()
                .with_color(Color::from_rgb(0.3, 0.3, 0.3))
                .with_width(1.5),
        );

        // Arc indicator
        // Start angle: 220° (bottom-left), End angle: -40° (bottom-right, 320°)
        // Full range is ~280°
        let start_angle = 220.0_f32.to_radians();
        let end_angle = -40.0_f32.to_radians();
        let total_arc = start_angle - end_angle;

        // Draw background arc (full range, dim)
        let arc_radius = radius * 0.7;
        let bg_arc = create_arc_path(center, arc_radius, start_angle, end_angle);
        frame.stroke(
            &bg_arc,
            Stroke::default()
                .with_color(Color::from_rgb(0.25, 0.25, 0.25))
                .with_width(3.0),
        );

        // Draw value arc (from start to current value)
        if self.value > 0.001 {
            let value_angle = start_angle - (self.value * total_arc);
            let value_arc = create_arc_path(center, arc_radius, start_angle, value_angle);

            // Color: blue to orange gradient based on value
            let color = if self.value < 0.5 {
                Color::from_rgb(0.3, 0.6, 0.9)
            } else {
                Color::from_rgb(0.9, 0.6, 0.3)
            };

            frame.stroke(&value_arc, Stroke::default().with_color(color).with_width(3.0));
        }

        // Draw indicator dot at current position
        let indicator_angle = start_angle - (self.value * total_arc);
        let dot_radius = arc_radius;
        let dot_center = Point::new(
            center.x + dot_radius * indicator_angle.cos(),
            center.y - dot_radius * indicator_angle.sin(),
        );
        let dot = Path::circle(dot_center, 3.0);
        frame.fill(&dot, Color::WHITE);

        // Draw label if present
        if let Some(label) = self.label {
            use iced::widget::canvas::Text;
            frame.fill_text(Text {
                content: label.to_string(),
                position: Point::new(knob_size / 2.0, knob_size + 2.0),
                color: Color::from_rgb(0.7, 0.7, 0.7),
                size: 10.0.into(),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Top.into(),
                ..Text::default()
            });
        }
    }
}

/// Create an arc path from start_angle to end_angle
fn create_arc_path(center: Point, radius: f32, start_angle: f32, end_angle: f32) -> Path {
    Path::new(|builder| {
        let steps = 32;
        let angle_range = start_angle - end_angle;

        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let angle = start_angle - (t * angle_range);
            let x = center.x + radius * angle.cos();
            let y = center.y - radius * angle.sin();

            if i == 0 {
                builder.move_to(Point::new(x, y));
            } else {
                builder.line_to(Point::new(x, y));
            }
        }
    })
}

/// Create a rotary knob canvas element
///
/// # Arguments
/// * `state` - Reference to knob state (for rendering cache)
/// * `value` - Current value (0.0-1.0)
/// * `size` - Size of the knob in pixels
/// * `label` - Optional label to display below the knob
/// * `on_change` - Callback when value changes
///
/// # Returns
/// An Element that produces messages via the on_change callback
pub fn rotary_knob<'a, Message: Clone + 'a>(
    state: &'a RotaryKnobState,
    value: f32,
    size: f32,
    label: Option<&'a str>,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let height = if label.is_some() { size + 14.0 } else { size };

    Canvas::new(RotaryKnobCanvas {
        state,
        value: value.clamp(0.0, 1.0),
        label,
        on_change,
    })
    .width(Length::Fixed(size))
    .height(Length::Fixed(height))
    .into()
}
