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
        let _text_color = palette.background.base.text;
        let accent = palette.primary.base.color;

        // Match the app background exactly so faded tracks are still visible against it
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), bg);

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

        // ── Ribbon half-widths from similarity (min-max normalized) ───
        let raw_widths: Vec<f32> = (0..n).map(|i| {
            let left = if i > 0 && i - 1 < self.transitions.len() {
                self.transitions[i - 1].similarity_distance
            } else { 0.0 };
            let right = if i < self.transitions.len() {
                self.transitions[i].similarity_distance
            } else { 0.0 };
            (left + right) / 2.0
        }).collect();
        let w_min = raw_widths.iter().fold(f32::MAX, |a, &b| a.min(b));
        let w_max = raw_widths.iter().fold(f32::MIN, |a, &b| a.max(b));
        let w_range = (w_max - w_min).max(0.001);
        let half_widths: Vec<f32> = raw_widths.iter()
            .map(|&w| {
                let norm_w = (w - w_min) / w_range; // [0, 1]
                2.0 + norm_w * 14.0 // 2px min, 16px max
            })
            .collect();

        // ── Focus centered on INCOMING transition ──────────────────────
        // The selected track is what you're mixing IN. The "hot" segment is
        // cc-1 (prev→current) — the transition you need to execute.
        // Segment i connects track[i] → track[i+1].
        // Focus point = segment cc-1 (incoming to selected track).
        let gray = Color::from_rgb(0.35, 0.35, 0.38);
        let focus = center as isize - 1; // incoming segment index

        // Color: smooth Gaussian falloff from focus — no hard edges
        let color_mix = |i: usize| -> f32 {
            let dist = (i as isize - focus).unsigned_abs() as f32;
            (-dist * dist * 0.3).exp() // Gaussian: σ² ≈ 1.7 segments
        };
        let mix_color = |base: Color, i: usize| -> Color {
            let t = color_mix(i);
            Color {
                r: gray.r + (base.r - gray.r) * t,
                g: gray.g + (base.g - gray.g) * t,
                b: gray.b + (base.b - gray.b) * t,
                a: base.a,
            }
        };

        // ── Layer 1: Ribbon fill (segment by segment) ─────────────────
        for i in 0..n - 1 {
            let x0 = x_for(i);
            let x1 = x_for(i + 1);
            let y0 = y_for(norm[i]);
            let y1 = y_for(norm[i + 1]);
            let hw0 = half_widths[i];
            let hw1 = half_widths[i + 1];
            let seg_color = if i < self.transitions.len() {
                let tc = mix_color(self.transitions[i].color, i);
                Color { a: 0.35, ..tc }
            } else {
                Color { a: 0.2, ..gray }
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

        // ── Layer 2: Vertical marker at selected track ────────────────
        {
            let x = x_for(center);
            let marker_color = Color { a: 0.5, ..accent };
            // Dotted vertical line spanning full height
            let dot_spacing = 4.0;
            let mut y_pos = 0.0f32;
            while y_pos < bounds.height {
                let seg_end = (y_pos + 2.0).min(bounds.height);
                let path = Path::line(
                    Point::new(x, y_pos),
                    Point::new(x, seg_end),
                );
                frame.stroke(&path, Stroke::default().with_color(marker_color).with_width(1.0));
                y_pos += dot_spacing;
            }
        }

        vec![frame.into_geometry()]
    }
}
