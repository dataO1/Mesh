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
    /// Whether to show the weight tuner triangle (visual display)
    pub show_weight_tuner: bool,
    /// Current weights [similarity, key, intensity] — sum to 1.0
    pub weights: [f32; 3],
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
        if self.show_weight_tuner {
            // Split: left = graph, right = triangle
            let tri_w = 80.0_f32.min(bounds.width * 0.3);
            let graph_w = bounds.width - tri_w;

            let graph_sub = Rectangle { x: 0.0, y: arc_h, width: graph_w, height: graph_bounds.height };
            if let Some(graph) = &self.graph {
                draw_graph_readonly(graph, &mut frame, graph_sub, Some(app_bg));
            } else {
                frame.fill_rectangle(Point::new(0.0, arc_h), Size::new(graph_w, graph_bounds.height), app_bg);
            }

            let tri_bounds = Rectangle { x: graph_w, y: arc_h, width: tri_w, height: graph_bounds.height };
            draw_weight_triangle(&mut frame, tri_bounds, self.weights, app_bg);
        } else {
            if let Some(graph) = &self.graph {
                draw_graph_readonly(graph, &mut frame, graph_bounds, Some(app_bg));
            } else {
                frame.fill_rectangle(
                    Point::new(0.0, arc_h),
                    Size::new(bounds.width, graph_bounds.height),
                    app_bg,
                );
            }
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

// ── Weight triangle (barycentric coordinate picker) ──────────────────────

fn draw_weight_triangle(
    frame: &mut canvas::Frame,
    bounds: Rectangle,
    weights: [f32; 3], // [similarity, key, intensity]
    bg: Color,
) {
    use iced::widget::canvas::{Stroke, Text};

    frame.fill_rectangle(Point::new(bounds.x, bounds.y), bounds.size(), bg);

    let margin = 12.0;
    let usable = (bounds.width - margin * 2.0).min(bounds.height - margin * 2.0 - 20.0).max(20.0);
    let cx = bounds.x + bounds.width / 2.0;
    let top_y = bounds.y + margin + 12.0; // leave room for top label

    // Equilateral triangle vertices (top = similarity, bottom-left = key, bottom-right = intensity)
    let h = usable * 0.866; // sqrt(3)/2
    let top = Point::new(cx, top_y);
    let bl = Point::new(cx - usable / 2.0, top_y + h);
    let br = Point::new(cx + usable / 2.0, top_y + h);

    // Draw triangle outline
    let tri_color = Color::from_rgb(0.4, 0.4, 0.42);
    let mut path = canvas::path::Builder::new();
    path.move_to(top);
    path.line_to(bl);
    path.line_to(br);
    path.close();
    frame.stroke(&path.build(), Stroke::default().with_color(tri_color).with_width(1.0));

    // Labels
    let label_color = Color::from_rgb(0.6, 0.6, 0.62);
    let sz = 9.0;
    frame.fill_text(Text {
        content: "Sim".to_string(),
        position: Point::new(top.x - 8.0, top.y - 12.0),
        color: label_color, size: sz.into(), ..Text::default()
    });
    frame.fill_text(Text {
        content: "Key".to_string(),
        position: Point::new(bl.x - 4.0, bl.y + 3.0),
        color: label_color, size: sz.into(), ..Text::default()
    });
    frame.fill_text(Text {
        content: "Int".to_string(),
        position: Point::new(br.x - 8.0, br.y + 3.0),
        color: label_color, size: sz.into(), ..Text::default()
    });

    // Dot at current weight position (barycentric → cartesian)
    let [w_sim, w_key, w_int] = weights;
    let dot_x = w_sim * top.x + w_key * bl.x + w_int * br.x;
    let dot_y = w_sim * top.y + w_key * bl.y + w_int * br.y;

    let dot_color = Color::from_rgb(0.9, 0.75, 0.2);
    frame.fill(&Path::circle(Point::new(dot_x, dot_y), 5.0), dot_color);
    frame.stroke(
        &Path::circle(Point::new(dot_x, dot_y), 5.0),
        Stroke::default().with_color(Color::WHITE).with_width(1.5),
    );

    // Weight values text
    let vals = format!("S:{:.0} K:{:.0} I:{:.0}", w_sim * 100.0, w_key * 100.0, w_int * 100.0);
    frame.fill_text(Text {
        content: vals,
        position: Point::new(bounds.x + 4.0, bounds.y + bounds.height - 14.0),
        color: label_color, size: sz.into(), ..Text::default()
    });
}
