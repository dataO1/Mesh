//! Energy arc widget -- shows intensity progression and key transitions
//! across a playlist as a small line graph rendered on a Canvas.

use iced::widget::canvas::{self, Path, Stroke, Text};
use iced::widget::Canvas;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme, mouse};

/// Data for one track in the energy arc.
#[derive(Debug, Clone)]
pub struct ArcPoint {
    /// Track title (used for tooltips / labels)
    pub title: String,
    /// Composite intensity value in [0, 1]
    pub intensity: f32,
    /// Musical key string (e.g. "Am", "5B")
    pub key: Option<String>,
    /// BPM if known
    pub bpm: Option<f64>,
}

/// Key transition annotation between two consecutive tracks.
#[derive(Debug, Clone)]
pub struct ArcTransition {
    /// Short label: "Same Key", "Adjacent", "Boost", etc.
    pub label: &'static str,
    /// Color indicating transition quality (green/amber/red)
    pub color: Color,
}

/// State for the energy arc display.
pub struct EnergyArcState {
    /// All track points in playlist order.
    pub points: Vec<ArcPoint>,
    /// Transitions between consecutive tracks (len = points.len() - 1).
    pub transitions: Vec<ArcTransition>,
    /// Index of the current / focused track (centered in the visible window).
    pub current_index: usize,
}

// ---- Constants ----

/// How many tracks before/after the current index to show.
const WINDOW_RADIUS: usize = 3;

/// Height of the widget (fixed).
const ARC_HEIGHT: f32 = 40.0;

/// Radius for normal track dots.
const DOT_RADIUS: f32 = 3.5;

/// Radius for the current (focused) track dot.
const CURRENT_DOT_RADIUS: f32 = 5.5;

/// Line width for the intensity polyline.
const LINE_WIDTH: f32 = 1.5;

/// Font size for transition labels.
const LABEL_SIZE: f32 = 8.5;

/// Vertical padding so the line does not touch top/bottom edges.
const V_PAD: f32 = 8.0;

/// Horizontal padding so dots are not clipped at edges.
const H_PAD: f32 = 16.0;

/// Subtle background color (nearly transparent dark).
const BG_COLOR: Color = Color {
    r: 0.08,
    g: 0.08,
    b: 0.10,
    a: 0.55,
};

/// Line color (light gray).
const LINE_COLOR: Color = Color {
    r: 0.65,
    g: 0.65,
    b: 0.70,
    a: 0.85,
};

/// Dot color for non-current points.
const DOT_COLOR: Color = Color {
    r: 0.75,
    g: 0.75,
    b: 0.80,
    a: 0.95,
};

/// Accent ring color for the current point.
const CURRENT_RING_COLOR: Color = Color {
    r: 0.40,
    g: 0.70,
    b: 0.95,
    a: 1.0,
};

// ---- View function ----

/// Create an energy arc element from the given state.
///
/// Returns a 40px tall, full-width Canvas.
pub fn energy_arc<M: 'static>(state: &EnergyArcState) -> Element<'_, M> {
    Canvas::new(state)
        .width(Length::Fill)
        .height(Length::Fixed(ARC_HEIGHT))
        .into()
}

// ---- Canvas Program impl ----

impl<M> canvas::Program<M> for EnergyArcState {
    type State = ();

    fn draw(
        &self,
        _interaction: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // Background
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), BG_COLOR);

        if self.points.is_empty() {
            return vec![frame.into_geometry()];
        }

        // Determine the visible window
        let n = self.points.len();
        let center = self.current_index.min(n.saturating_sub(1));
        let win_start = center.saturating_sub(WINDOW_RADIUS);
        let win_end = (center + WINDOW_RADIUS + 1).min(n);
        let win_len = win_end - win_start;

        if win_len == 0 {
            return vec![frame.into_geometry()];
        }

        let usable_w = (bounds.width - 2.0 * H_PAD).max(1.0);
        let usable_h = (bounds.height - 2.0 * V_PAD).max(1.0);

        // Compute screen positions for each visible point
        let positions: Vec<Point> = (0..win_len)
            .map(|i| {
                let x = if win_len == 1 {
                    bounds.width / 2.0
                } else {
                    H_PAD + (i as f32 / (win_len - 1) as f32) * usable_w
                };
                let intensity = self.points[win_start + i].intensity.clamp(0.0, 1.0);
                // Y: 0 at bottom, 1 at top
                let y = V_PAD + (1.0 - intensity) * usable_h;
                Point::new(x, y)
            })
            .collect();

        // Edge fade: points at the border of the window are dimmer
        let alpha_for = |local_i: usize| -> f32 {
            let dist_from_center = (local_i as isize - (center - win_start) as isize).unsigned_abs();
            if dist_from_center == 0 {
                1.0
            } else {
                (1.0 - (dist_from_center as f32 / (WINDOW_RADIUS as f32 + 1.0))).max(0.25)
            }
        };

        // Draw the polyline
        if positions.len() >= 2 {
            for i in 0..positions.len() - 1 {
                let a = alpha_for(i).min(alpha_for(i + 1));
                let seg_color = Color { a: a * LINE_COLOR.a, ..LINE_COLOR };
                let path = Path::line(positions[i], positions[i + 1]);
                frame.stroke(&path, Stroke::default().with_color(seg_color).with_width(LINE_WIDTH));
            }
        }

        // Draw transition labels between consecutive dots
        for i in 0..positions.len().saturating_sub(1) {
            let trans_idx = win_start + i; // index into self.transitions
            if trans_idx >= self.transitions.len() {
                break;
            }
            let tr = &self.transitions[trans_idx];
            let mid_x = (positions[i].x + positions[i + 1].x) / 2.0;
            let mid_y = (positions[i].y + positions[i + 1].y) / 2.0;
            let a = alpha_for(i).min(alpha_for(i + 1));

            let label = Text {
                content: tr.label.to_string(),
                position: Point::new(mid_x, mid_y - 9.0),
                color: Color { a: a * tr.color.a, ..tr.color },
                size: LABEL_SIZE.into(),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Bottom.into(),
                ..Text::default()
            };
            frame.fill_text(label);
        }

        // Draw dots
        for (i, pos) in positions.iter().enumerate() {
            let is_current = (win_start + i) == center;
            let a = alpha_for(i);

            if is_current {
                // Outer accent ring
                let ring = Path::circle(*pos, CURRENT_DOT_RADIUS + 1.5);
                frame.fill(&ring, Color { a: 0.35, ..CURRENT_RING_COLOR });
                // Inner dot
                let dot = Path::circle(*pos, CURRENT_DOT_RADIUS);
                frame.fill(&dot, CURRENT_RING_COLOR);
            } else {
                let dot = Path::circle(*pos, DOT_RADIUS);
                frame.fill(&dot, Color { a: a * DOT_COLOR.a, ..DOT_COLOR });
            }
        }

        vec![frame.into_geometry()]
    }
}
