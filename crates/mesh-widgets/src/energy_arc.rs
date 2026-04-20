//! Energy arc widget — shows intensity progression and key transitions
//! across a playlist as a line graph rendered on a Canvas.
//!
//! Shows ALL tracks in the playlist. Intensity is min-max normalized so the
//! lowest track maps to the bottom and the highest to the top (full vertical
//! range). The current/selected track is highlighted with a larger dot;
//! distant tracks fade slightly. Key transitions are shown as colored line
//! segments (green = compatible, amber = moderate, red = poor).

use iced::widget::canvas::{self, Path, Stroke};
use iced::widget::Canvas;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme, mouse};

/// Data for one track in the energy arc.
#[derive(Debug, Clone)]
pub struct ArcPoint {
    pub title: String,
    pub intensity: f32,
    pub key: Option<String>,
    pub bpm: Option<f64>,
}

/// Key transition annotation between two consecutive tracks.
#[derive(Debug, Clone)]
pub struct ArcTransition {
    pub label: &'static str,
    pub color: Color,
    /// Cosine distance between consecutive PCA embeddings [0, ~1.5].
    /// 0 = identical sound, higher = more different.
    pub similarity_distance: f32,
}

/// State for the energy arc display.
pub struct EnergyArcState {
    pub points: Vec<ArcPoint>,
    pub transitions: Vec<ArcTransition>,
    pub current_index: usize,
}

const ARC_HEIGHT: f32 = 50.0;
const V_PAD: f32 = 10.0;
const H_PAD: f32 = 12.0;

const LINE_COLOR: Color = Color { r: 0.55, g: 0.55, b: 0.60, a: 0.6 };
const DOT_COLOR: Color = Color { r: 0.70, g: 0.70, b: 0.75, a: 0.9 };
const CURRENT_COLOR: Color = Color { r: 0.90, g: 0.90, b: 0.95, a: 1.0 };
const BG_COLOR: Color = Color { r: 0.08, g: 0.08, b: 0.10, a: 0.55 };

/// Create an energy arc element.
pub fn energy_arc<M: 'static>(state: &EnergyArcState) -> Element<'_, M> {
    Canvas::new(state)
        .width(Length::Fill)
        .height(Length::Fixed(ARC_HEIGHT))
        .into()
}

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
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), BG_COLOR);

        let n = self.points.len();
        if n < 2 {
            return vec![frame.into_geometry()];
        }

        let usable_w = (bounds.width - 2.0 * H_PAD).max(1.0);
        let usable_h = (bounds.height - 2.0 * V_PAD).max(1.0);
        let center = self.current_index.min(n - 1);

        // Compute perceived energy: cumulative signal combining intensity,
        // key transition direction, and vector dissimilarity.
        let mut perceived = vec![0.0f32; n];
        perceived[0] = self.points[0].intensity;
        for i in 1..n {
            let intensity_delta = self.points[i].intensity - self.points[i - 1].intensity;

            if i - 1 < self.transitions.len() {
                let tr = &self.transitions[i - 1];
                // Key direction from transition color heuristic:
                // green (high base_score) = compatible = small direction
                // We use the label to infer direction
                let key_dir = match tr.label {
                    "Same Key" => 0.0,
                    "Adjacent" => 0.15, // could be up or down, approximate
                    "Diagonal" => 0.10,
                    "Boost" => 0.35,
                    "Cool" => -0.35,
                    "Mood Lift" => 0.25,
                    "Darken" => -0.25,
                    "Semitone" => 0.20,
                    "Far" | "Cross" => 0.0,
                    "Tritone" => 0.0,
                    _ => 0.0,
                };

                // Dissimilarity amplifies the change
                let dissim = tr.similarity_distance;
                let amplifier = 0.5 + dissim;

                let direction = intensity_delta * 0.5 + key_dir * 0.3;
                // Exponential decay to prevent drift: blend toward raw intensity
                perceived[i] = perceived[i - 1] * 0.8
                    + self.points[i].intensity * 0.2
                    + direction * amplifier * 0.25;
            } else {
                perceived[i] = self.points[i].intensity;
            }
        }

        // Min-max normalize perceived energy for full vertical range
        let min_p = perceived.iter().fold(f32::MAX, |a, &b| a.min(b));
        let max_p = perceived.iter().fold(f32::MIN, |a, &b| a.max(b));
        let range = (max_p - min_p).max(0.001);

        // Compute screen positions
        let positions: Vec<Point> = (0..n)
            .map(|i| {
                let x = H_PAD + (i as f32 / (n - 1) as f32) * usable_w;
                let norm = (perceived[i] - min_p) / range;
                let y = V_PAD + (1.0 - norm) * usable_h;
                Point::new(x, y)
            })
            .collect();

        // Alpha based on distance from current (fade far tracks)
        let alpha_for = |i: usize| -> f32 {
            let dist = (i as isize - center as isize).unsigned_abs() as f32;
            (1.0 - dist * 0.04).clamp(0.15, 1.0)
        };

        // Layer 1: Transition-colored line segments
        for i in 0..n - 1 {
            let a = alpha_for(i).min(alpha_for(i + 1));

            // Color the segment by transition quality (if available)
            let seg_color = if i < self.transitions.len() {
                let tc = self.transitions[i].color;
                Color { a: a * 0.8, ..tc }
            } else {
                Color { a: a * LINE_COLOR.a, ..LINE_COLOR }
            };

            let width = if i == center || i + 1 == center { 2.5 } else { 1.5 };
            let path = Path::line(positions[i], positions[i + 1]);
            frame.stroke(&path, Stroke::default().with_color(seg_color).with_width(width));
        }

        // Layer 2: Dots (all tracks)
        for (i, pos) in positions.iter().enumerate() {
            let is_current = i == center;
            let a = alpha_for(i);

            if is_current {
                // Current: large white dot
                let dot = Path::circle(*pos, 5.0);
                frame.fill(&dot, CURRENT_COLOR);
            } else {
                let r = 2.5;
                let dot = Path::circle(*pos, r);
                frame.fill(&dot, Color { a, ..DOT_COLOR });
            }
        }

        // Layer 3: Transition labels near current position only (±2)
        for i in center.saturating_sub(2)..=(center + 1).min(n.saturating_sub(2)) {
            if i >= self.transitions.len() { break; }
            let tr = &self.transitions[i];
            let mid_x = (positions[i].x + positions[i + 1].x) / 2.0;
            // Place label above or below the line depending on direction
            let going_up = positions[i + 1].y < positions[i].y;
            let label_y = if going_up {
                (positions[i].y.min(positions[i + 1].y)) - 3.0
            } else {
                (positions[i].y.max(positions[i + 1].y)) + 10.0
            };

            let label = canvas::Text {
                content: tr.label.to_string(),
                position: Point::new(mid_x, label_y),
                color: tr.color,
                size: 9.0.into(),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Center.into(),
                ..canvas::Text::default()
            };
            frame.fill_text(label);
        }

        vec![frame.into_geometry()]
    }
}
