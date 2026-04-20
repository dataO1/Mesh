//! Energy arc ribbon widget — visualizes set energy flow as a flowing ribbon.
//!
//! Three dimensions encoded:
//! - **Center Y position**: perceived energy (intensity + key direction + similarity)
//! - **Ribbon width**: vector dissimilarity (wider = bigger spectral jump)
//! - **Ribbon color**: key transition quality (theme-derived gradient)
//!
//! Uses the application's color theme — no hardcoded colors.

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
    pub similarity_distance: f32,
}

/// State for the energy arc display.
pub struct EnergyArcState {
    pub points: Vec<ArcPoint>,
    pub transitions: Vec<ArcTransition>,
    pub current_index: usize,
    /// Theme stem colors [Vocals, Drums, Bass, Other] for ribbon coloring
    pub stem_colors: [Color; 4],
}

const ARC_HEIGHT: f32 = 55.0;
const V_PAD: f32 = 8.0;
const H_PAD: f32 = 12.0;

/// Create an energy arc element.
pub fn energy_arc<M: 'static>(state: &EnergyArcState) -> Element<'_, M> {
    Canvas::new(state)
        .width(Length::Fill)
        .height(Length::Fixed(ARC_HEIGHT))
        .into()
}

/// Blend two colors by factor t (0 = a, 1 = b).
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

impl<M> canvas::Program<M> for EnergyArcState {
    type State = ();

    fn draw(
        &self,
        _interaction: &(),
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let palette = theme.extended_palette();
        let bg = palette.background.base.color;
        let text_color = palette.background.base.text;
        let accent = palette.primary.base.color;

        // Slightly lighter background than app bg
        let ribbon_bg = Color { r: bg.r + 0.03, g: bg.g + 0.03, b: bg.b + 0.03, a: 0.7 };
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), ribbon_bg);

        let n = self.points.len();
        if n < 2 {
            return vec![frame.into_geometry()];
        }

        let usable_w = (bounds.width - 2.0 * H_PAD).max(1.0);
        let usable_h = (bounds.height - 2.0 * V_PAD).max(1.0);
        let center = self.current_index.min(n - 1);

        // ── Compute perceived energy ──────────────────────────────────
        // Base = raw intensity (aggression). Key direction and similarity
        // modify the perceived level additively — no decay/smoothing.
        let mut perceived = vec![0.0f32; n];
        perceived[0] = self.points[0].intensity;
        for i in 1..n {
            let base = self.points[i].intensity;

            if i - 1 < self.transitions.len() {
                let tr = &self.transitions[i - 1];
                let key_dir = match tr.label {
                    "Same Key" => 0.0,
                    "Adjacent" => 0.12,
                    "Diagonal" => 0.08,
                    "Boost" => 0.25,
                    "Cool" => -0.25,
                    "Mood Lift" => 0.18,
                    "Darken" => -0.18,
                    "Semitone" => 0.15,
                    _ => 0.0,
                };
                // Dissimilarity amplifies the key direction effect
                let dissim = tr.similarity_distance;
                let amplifier = 0.5 + dissim;
                perceived[i] = base + key_dir * amplifier * 0.3;
            } else {
                perceived[i] = base;
            }
        }

        // Min-max normalize
        let min_p = perceived.iter().fold(f32::MAX, |a, &b| a.min(b));
        let max_p = perceived.iter().fold(f32::MIN, |a, &b| a.max(b));
        let range = (max_p - min_p).max(0.001);
        let norm: Vec<f32> = perceived.iter().map(|&p| (p - min_p) / range).collect();

        // X positions for each track
        let x_for = |i: usize| -> f32 {
            H_PAD + (i as f32 / (n - 1) as f32) * usable_w
        };
        // Y from normalized energy (0 at bottom, 1 at top)
        let y_for = |n_val: f32| -> f32 {
            V_PAD + (1.0 - n_val) * usable_h
        };

        // ── Ribbon half-widths from similarity ────────────────────────
        let half_widths: Vec<f32> = (0..n).map(|i| {
            // Width from average of adjacent transition distances
            let left = if i > 0 && i - 1 < self.transitions.len() {
                self.transitions[i - 1].similarity_distance
            } else { 0.2 };
            let right = if i < self.transitions.len() {
                self.transitions[i].similarity_distance
            } else { 0.2 };
            let avg_dissim = (left + right) / 2.0;
            // Map to visual width: min 2px, max 12px
            2.0 + avg_dissim.clamp(0.0, 1.0) * 10.0
        }).collect();

        // ── Distance-based alpha (fade distant tracks) ────────────────
        let alpha_for = |i: usize| -> f32 {
            let dist = (i as isize - center as isize).unsigned_abs() as f32;
            (1.0 - dist * 0.03).clamp(0.15, 1.0)
        };

        // ── Layer 1: Ribbon fill (segment by segment) ─────────────────
        // Each segment is a filled quadrilateral between two track positions
        for i in 0..n - 1 {
            let x0 = x_for(i);
            let x1 = x_for(i + 1);
            let y0 = y_for(norm[i]);
            let y1 = y_for(norm[i + 1]);
            let hw0 = half_widths[i];
            let hw1 = half_widths[i + 1];
            let a = alpha_for(i).min(alpha_for(i + 1));

            // Color from transition quality (use stem colors as gradient)
            let seg_color = if i < self.transitions.len() {
                let tc = self.transitions[i].color;
                Color { a: a * 0.35, ..tc }
            } else {
                Color { a: a * 0.2, ..self.stem_colors[3] }
            };

            // Draw filled quad: top-left, top-right, bottom-right, bottom-left
            let mut path = canvas::path::Builder::new();
            path.move_to(Point::new(x0, y0 - hw0));
            path.line_to(Point::new(x1, y1 - hw1));
            path.line_to(Point::new(x1, y1 + hw1));
            path.line_to(Point::new(x0, y0 + hw0));
            path.close();
            frame.fill(&path.build(), seg_color);
        }

        // ── Layer 2: Center line (crisp, on top of ribbon fill) ───────
        for i in 0..n - 1 {
            let x0 = x_for(i);
            let x1 = x_for(i + 1);
            let y0 = y_for(norm[i]);
            let y1 = y_for(norm[i + 1]);
            let a = alpha_for(i).min(alpha_for(i + 1));

            let line_color = if i < self.transitions.len() {
                let tc = self.transitions[i].color;
                Color { a: a * 0.9, ..tc }
            } else {
                Color { a: a * 0.5, ..text_color }
            };

            let width = if i == center || i + 1 == center { 2.0 } else { 1.2 };
            let path = Path::line(Point::new(x0, y0), Point::new(x1, y1));
            frame.stroke(&path, Stroke::default().with_color(line_color).with_width(width));
        }

        // ── Layer 3: Track dots ───────────────────────────────────────
        for i in 0..n {
            let x = x_for(i);
            let y = y_for(norm[i]);
            let a = alpha_for(i);

            if i == center {
                // Current track: accent dot with glow
                let glow = Path::circle(Point::new(x, y), 6.0);
                frame.fill(&glow, Color { a: 0.25, ..accent });
                let dot = Path::circle(Point::new(x, y), 4.0);
                frame.fill(&dot, accent);
            } else {
                let dot_color = Color { a: a * 0.7, ..text_color };
                let dot = Path::circle(Point::new(x, y), 2.0);
                frame.fill(&dot, dot_color);
            }
        }

        // ── Layer 4: Transition labels near current (±1 only) ─────────
        for i in center.saturating_sub(1)..=(center).min(n.saturating_sub(2)) {
            if i >= self.transitions.len() { break; }
            let tr = &self.transitions[i];
            let mid_x = (x_for(i) + x_for(i + 1)) / 2.0;
            let y0 = y_for(norm[i]);
            let y1 = y_for(norm[i + 1]);
            let label_y = y0.min(y1) - half_widths[i].max(half_widths[i + 1]) - 2.0;

            let label = canvas::Text {
                content: tr.label.to_string(),
                position: Point::new(mid_x, label_y),
                color: Color { a: 0.85, ..tr.color },
                size: 8.5.into(),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Bottom.into(),
                ..canvas::Text::default()
            };
            frame.fill_text(label);
        }

        vec![frame.into_geometry()]
    }
}
