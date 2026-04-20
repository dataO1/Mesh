//! Combined canvas widget for the mesh-player collection browser.
//!
//! Draws both the energy arc ribbon (top strip) and the suggestion graph view
//! (below) in a single iced Canvas — required due to iced's canvas bug that
//! prevents multiple Canvas widgets.

use iced::widget::canvas::{self, Canvas};
use iced::widget::canvas::Path;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};

use crate::energy_arc::{EnergyArcState, ARC_HEIGHT, V_PAD, H_PAD};
use crate::graph_view::{GraphViewState, draw_graph_readonly};

/// State for the combined browser canvas.
pub struct BrowserCanvasState {
    pub energy_arc: Option<EnergyArcState>,
    pub graph: Option<GraphViewState>,
}

impl<M: 'static> canvas::Program<M> for BrowserCanvasState {
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

        let arc_h = if self.energy_arc.is_some() { ARC_HEIGHT } else { 0.0 };

        // ── Top strip: Energy arc ribbon ──────────────────────────────────
        if let Some(arc) = &self.energy_arc {
            let arc_bounds = Rectangle {
                x: 0.0, y: 0.0,
                width: bounds.width, height: arc_h,
            };
            draw_energy_arc_into(arc, &mut frame, arc_bounds, theme);
        }

        // ── Below: Graph view ─────────────────────────────────────────────
        let palette = theme.extended_palette();
        let app_bg = palette.background.base.color;
        let graph_bounds = Rectangle {
            x: 0.0, y: arc_h,
            width: bounds.width,
            height: (bounds.height - arc_h).max(0.0),
        };
        if let Some(graph) = &self.graph {
            draw_graph_readonly(graph, &mut frame, graph_bounds, Some(app_bg));
        } else {
            frame.fill_rectangle(
                Point::new(0.0, arc_h),
                Size::new(bounds.width, (bounds.height - arc_h).max(0.0)),
                app_bg,
            );
        }

        vec![frame.into_geometry()]
    }
}

/// Create the combined browser canvas element.
pub fn browser_canvas<M: 'static>(state: &BrowserCanvasState) -> Element<'_, M> {
    Canvas::new(state)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

// ── Energy arc drawing (extracted from energy_arc.rs) ─────────────────────

fn draw_energy_arc_into(
    arc: &EnergyArcState,
    frame: &mut canvas::Frame,
    bounds: Rectangle,
    theme: &Theme,
) {
    let palette = theme.extended_palette();
    let bg = palette.background.base.color;
    let accent = palette.primary.base.color;

    frame.fill_rectangle(Point::new(bounds.x, bounds.y), bounds.size(), bg);

    let n = arc.points.len();
    if n < 2 { return; }

    let usable_w = (bounds.width - 2.0 * H_PAD).max(1.0);
    let usable_h = (bounds.height - 2.0 * V_PAD).max(1.0);
    let center = arc.current_index.min(n - 1);

    // Perceived energy
    let mut perceived = vec![0.0f32; n];
    perceived[0] = arc.points[0].intensity;
    for i in 1..n {
        let base = arc.points[i].intensity;
        if i - 1 < arc.transitions.len() {
            let tr = &arc.transitions[i - 1];
            let key_dir = match tr.label {
                "Same Key" => 0.0,
                "Adjacent" => 0.12, "Diagonal" => 0.08,
                "Boost" => 0.25, "Cool" => -0.25,
                "Mood Lift" => 0.18, "Darken" => -0.18,
                "Semitone" => 0.15,
                _ => 0.0,
            };
            let amplifier = 0.5 + tr.similarity_distance;
            perceived[i] = base + key_dir * amplifier * 0.3;
        } else {
            perceived[i] = base;
        }
    }

    let min_p = perceived.iter().fold(f32::MAX, |a, &b| a.min(b));
    let max_p = perceived.iter().fold(f32::MIN, |a, &b| a.max(b));
    let range = (max_p - min_p).max(0.001);
    let norm: Vec<f32> = perceived.iter().map(|&p| (p - min_p) / range).collect();

    let x_for = |i: usize| -> f32 { bounds.x + H_PAD + (i as f32 / (n - 1) as f32) * usable_w };
    let y_for = |n_val: f32| -> f32 { bounds.y + V_PAD + (1.0 - n_val) * usable_h };

    // Ribbon half-widths
    let raw_widths: Vec<f32> = (0..n).map(|i| {
        let left = if i > 0 && i - 1 < arc.transitions.len() { arc.transitions[i - 1].similarity_distance } else { 0.0 };
        let right = if i < arc.transitions.len() { arc.transitions[i].similarity_distance } else { 0.0 };
        (left + right) / 2.0
    }).collect();
    let w_min = raw_widths.iter().fold(f32::MAX, |a, &b| a.min(b));
    let w_max = raw_widths.iter().fold(f32::MIN, |a, &b| a.max(b));
    let w_range = (w_max - w_min).max(0.001);
    let half_widths: Vec<f32> = raw_widths.iter()
        .map(|&w| 2.0 + ((w - w_min) / w_range) * 14.0)
        .collect();

    // Focus + Gaussian color blend
    let gray = Color::from_rgb(0.35, 0.35, 0.38);
    let focus = center as isize - 1;
    let color_mix = |i: usize| -> f32 {
        let signed_dist = i as isize - focus;
        let dist = signed_dist.unsigned_abs() as f32;
        let sigma2 = if signed_dist < 0 { 0.7 } else { 1.5 };
        (-dist * dist / (2.0 * sigma2)).exp()
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

    // Ribbon fill
    for i in 0..n - 1 {
        let x0 = x_for(i); let x1 = x_for(i + 1);
        let y0 = y_for(norm[i]); let y1 = y_for(norm[i + 1]);
        let hw0 = half_widths[i]; let hw1 = half_widths[i + 1];
        let seg_color = if i < arc.transitions.len() {
            let tc = mix_color(arc.transitions[i].color, i);
            Color { a: 0.35, ..tc }
        } else {
            Color { a: 0.2, ..gray }
        };

        let mut path = canvas::path::Builder::new();
        path.move_to(Point::new(x0, y0 - hw0));
        path.line_to(Point::new(x1, y1 - hw1));
        path.line_to(Point::new(x1, y1 + hw1));
        path.line_to(Point::new(x0, y0 + hw0));
        path.close();
        frame.fill(&path.build(), seg_color);
    }

    // Vertical marker at selected track
    let x = x_for(center);
    let marker_color = Color { a: 0.7, ..accent };
    frame.stroke(
        &Path::line(Point::new(x, bounds.y), Point::new(x, bounds.y + bounds.height)),
        canvas::Stroke::default().with_color(marker_color).with_width(2.0),
    );
}
