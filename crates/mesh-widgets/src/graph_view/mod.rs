//! Canvas-based graph visualization for the suggestion graph view.
//!
//! Renders track nodes and similarity edges using iced's Canvas widget.
//! Supports pan/zoom navigation, seed selection, and hover tooltips.

pub mod layout;

use std::collections::{HashMap, HashSet};
use iced::widget::canvas::{self, Canvas, Path, Stroke, Text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse, event};

// ════════════════════════════════════════════════════════════════════════════
// Data types
// ════════════════════════════════════════════════════════════════════════════

/// Metadata for a single track node.
#[derive(Debug, Clone)]
pub struct TrackMeta {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub key: Option<String>,
    pub bpm: Option<f64>,
}

/// Messages emitted by the graph view.
#[derive(Debug, Clone)]
pub enum GraphViewMessage {
    SeedSelected(i64),
    NodeHovered(Option<i64>),
    SliderChanged(f32),
    PanZoomChanged { pan: (f32, f32), zoom: f32 },
}

// ════════════════════════════════════════════════════════════════════════════
// State
// ════════════════════════════════════════════════════════════════════════════

/// State for the graph view widget.
pub struct GraphViewState {
    // Layout
    pub positions: HashMap<i64, (f32, f32)>,
    pub pan: (f32, f32),
    pub zoom: f32,
    // Selection
    pub seed_stack: Vec<i64>,
    pub suggestion_ids: HashSet<i64>,
    pub suggestion_scores: HashMap<i64, f32>,
    pub hovered_id: Option<i64>,
    // Data
    /// Dynamic suggestion edges from current seed (seed_id, suggestion_id, composite_score)
    /// These are the ONLY edges shown — driven by the full suggestion algorithm
    pub suggestion_edges: Vec<(i64, i64, f32)>,
    pub track_meta: HashMap<i64, TrackMeta>,
    // Canvas cache
    pub edge_cache: canvas::Cache,
    pub node_cache: canvas::Cache,
    // Energy slider
    pub energy_direction: f32,
}

impl GraphViewState {
    /// Create an empty graph view state.
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            pan: (0.0, 0.0),
            zoom: 10.0,
            seed_stack: Vec::new(),
            suggestion_ids: HashSet::new(),
            suggestion_scores: HashMap::new(),
            hovered_id: None,
            suggestion_edges: Vec::new(),
            track_meta: HashMap::new(),
            edge_cache: canvas::Cache::new(),
            node_cache: canvas::Cache::new(),
            energy_direction: 0.5,
        }
    }

    /// Invalidate canvas caches so everything redraws.
    pub fn clear_caches(&self) {
        self.edge_cache.clear();
        self.node_cache.clear();
    }
}

impl Default for GraphViewState {
    fn default() -> Self {
        Self::new()
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Coordinate transforms
// ════════════════════════════════════════════════════════════════════════════

fn to_screen(pos: (f32, f32), pan: (f32, f32), zoom: f32, bounds: Rectangle) -> Point {
    Point {
        x: bounds.width * 0.5 + (pos.0 + pan.0) * zoom,
        y: bounds.height * 0.5 + (pos.1 + pan.1) * zoom,
    }
}

fn from_screen(screen: Point, pan: (f32, f32), zoom: f32, bounds: Rectangle) -> (f32, f32) {
    (
        (screen.x - bounds.width * 0.5) / zoom - pan.0,
        (screen.y - bounds.height * 0.5) / zoom - pan.1,
    )
}

// ════════════════════════════════════════════════════════════════════════════
// Colors
// ════════════════════════════════════════════════════════════════════════════

// Edge colors removed — edges now use score_color() from the suggestion algorithm
const COLOR_NODE_DIM: Color = Color::from_rgb(0.227, 0.227, 0.227);          // #3a3a3a
const COLOR_SEED_ACCENT: Color = Color::from_rgb(0.290, 0.498, 0.647);       // #4a7fa5
const COLOR_SCORE_BEST: Color = Color::from_rgb(0.176, 0.541, 0.306);        // #2d8a4e
const COLOR_SCORE_WORST: Color = Color::from_rgb(0.651, 0.239, 0.251);       // #a63d40
const COLOR_BACKGROUND: Color = Color::from_rgb(0.08, 0.08, 0.08);

/// Linearly interpolate between best (green) and worst (red) based on score.
/// Score is 0..1 where HIGHER = better match (reward-based scoring).
fn score_color(score: f32) -> Color {
    let t = (1.0 - score).clamp(0.0, 1.0); // invert: high score → low t → green
    Color::from_rgb(
        COLOR_SCORE_BEST.r + (COLOR_SCORE_WORST.r - COLOR_SCORE_BEST.r) * t,
        COLOR_SCORE_BEST.g + (COLOR_SCORE_WORST.g - COLOR_SCORE_BEST.g) * t,
        COLOR_SCORE_BEST.b + (COLOR_SCORE_WORST.b - COLOR_SCORE_BEST.b) * t,
    )
}

// ════════════════════════════════════════════════════════════════════════════
// Canvas Program
// ════════════════════════════════════════════════════════════════════════════

/// Internal interaction state managed by the Canvas widget tree.
#[derive(Default)]
pub struct GraphInteraction {
    drag_start: Option<Point>,
    pan_at_drag_start: (f32, f32),
}

impl canvas::Program<GraphViewMessage> for GraphViewState {
    type State = GraphInteraction;

    fn update(
        &self,
        state: &mut GraphInteraction,
        event: &event::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<GraphViewMessage>> {
        let cursor_pos = cursor.position_in(bounds)?;

        match event {
            // ── Scroll to zoom ──────────────────────────────────────────
            event::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 60.0,
                };
                let factor = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
                let new_zoom = (self.zoom * factor).clamp(0.5, 500.0);

                // Zoom toward cursor position
                let world_before = from_screen(cursor_pos, self.pan, self.zoom, bounds);
                let world_after = from_screen(cursor_pos, self.pan, new_zoom, bounds);
                let new_pan = (
                    self.pan.0 + (world_after.0 - world_before.0),
                    self.pan.1 + (world_after.1 - world_before.1),
                );

                Some(
                    canvas::Action::publish(GraphViewMessage::PanZoomChanged {
                        pan: new_pan,
                        zoom: new_zoom,
                    })
                    .and_capture(),
                )
            }

            // ── Mouse press: start drag or select node ──────────────────
            event::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // Hit test: check if cursor is over any node
                let world = from_screen(cursor_pos, self.pan, self.zoom, bounds);
                let hit_radius = 7.0 / self.zoom; // screen-space radius in world coords

                let hit_id = self.positions.iter().find_map(|(&id, &(x, y))| {
                    let dx = world.0 - x;
                    let dy = world.1 - y;
                    if (dx * dx + dy * dy).sqrt() < hit_radius {
                        Some(id)
                    } else {
                        None
                    }
                });

                if let Some(id) = hit_id {
                    Some(canvas::Action::publish(GraphViewMessage::SeedSelected(id)).and_capture())
                } else {
                    // Start panning
                    state.drag_start = Some(cursor_pos);
                    state.pan_at_drag_start = self.pan;
                    Some(canvas::Action::capture())
                }
            }

            // ── Mouse move: drag pan or hover detection ─────────────────
            event::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(start) = state.drag_start {
                    let dx = (cursor_pos.x - start.x) / self.zoom;
                    let dy = (cursor_pos.y - start.y) / self.zoom;
                    let new_pan = (
                        state.pan_at_drag_start.0 + dx,
                        state.pan_at_drag_start.1 + dy,
                    );
                    Some(
                        canvas::Action::publish(GraphViewMessage::PanZoomChanged {
                            pan: new_pan,
                            zoom: self.zoom,
                        })
                        .and_capture(),
                    )
                } else {
                    // Hover detection
                    let world = from_screen(cursor_pos, self.pan, self.zoom, bounds);
                    let hit_radius = 7.0 / self.zoom;

                    let hover_id = self.positions.iter().find_map(|(&id, &(x, y))| {
                        let dx = world.0 - x;
                        let dy = world.1 - y;
                        if (dx * dx + dy * dy).sqrt() < hit_radius {
                            Some(id)
                        } else {
                            None
                        }
                    });

                    if hover_id != self.hovered_id {
                        Some(canvas::Action::publish(GraphViewMessage::NodeHovered(hover_id)))
                    } else {
                        None
                    }
                }
            }

            // ── Mouse release: end drag ─────────────────────────────────
            event::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.drag_start.is_some() {
                    state.drag_start = None;
                    Some(canvas::Action::capture())
                } else {
                    None
                }
            }

            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &GraphInteraction,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // Background
        frame.fill_rectangle(
            Point::ORIGIN,
            bounds.size(),
            COLOR_BACKGROUND,
        );

        let seed_set: HashSet<i64> = self.seed_stack.iter().copied().collect();
        let current_seed = self.seed_stack.last().copied();
        let has_seed = current_seed.is_some();

        // ── Layer 1: Suggestion edges (seed → suggestions, composite score) ──
        // Only shown when a seed is selected — these ARE the graph's edges
        for &(from, to, score) in &self.suggestion_edges {
            let from_pos = match self.positions.get(&from) {
                Some(p) => *p,
                None => continue,
            };
            let to_pos = match self.positions.get(&to) {
                Some(p) => *p,
                None => continue,
            };

            let p1 = to_screen(from_pos, self.pan, self.zoom, bounds);
            let p2 = to_screen(to_pos, self.pan, self.zoom, bounds);

            // Color by composite score: higher = better = greener/thicker
            let edge_color = score_color(score);
            let opacity = score.clamp(0.2, 0.9);
            let width = 0.5 + score * 2.0;
            let line_color = Color { a: opacity, ..edge_color };

            let path = Path::line(p1, p2);
            frame.stroke(&path, Stroke::default().with_color(line_color).with_width(width));
        }

        // ── Layer 2: Seed history trail (red line between consecutive seeds) ──
        if self.seed_stack.len() >= 2 {
            let trail_color = Color::from_rgb(0.85, 0.2, 0.2); // red
            for window in self.seed_stack.windows(2) {
                let (a, b) = (window[0], window[1]);
                let a_pos = match self.positions.get(&a) {
                    Some(p) => *p,
                    None => continue,
                };
                let b_pos = match self.positions.get(&b) {
                    Some(p) => *p,
                    None => continue,
                };
                let p1 = to_screen(a_pos, self.pan, self.zoom, bounds);
                let p2 = to_screen(b_pos, self.pan, self.zoom, bounds);
                let path = Path::line(p1, p2);
                frame.stroke(
                    &path,
                    Stroke::default().with_color(trail_color).with_width(2.0),
                );
            }
        }

        // ── Layer 3: Unrelated nodes (dimmed, no edges) ─────────────────
        // Only draw if no seed is selected (overview) or as faint dots with seed
        let dim_alpha = if has_seed { 0.15 } else { 0.5 };
        for (&id, &pos) in &self.positions {
            if seed_set.contains(&id) || self.suggestion_ids.contains(&id) {
                continue;
            }

            let screen = to_screen(pos, self.pan, self.zoom, bounds);
            if screen.x < -10.0 || screen.y < -10.0
                || screen.x > bounds.width + 10.0
                || screen.y > bounds.height + 10.0
            {
                continue;
            }

            let circle = Path::circle(screen, 3.0);
            frame.fill(&circle, Color { a: dim_alpha, ..COLOR_NODE_DIM });
        }

        // ── Layer 4: Suggestion nodes (colored by score) ────────────────
        for &id in &self.suggestion_ids {
            if seed_set.contains(&id) {
                continue;
            }
            let pos = match self.positions.get(&id) {
                Some(p) => *p,
                None => continue,
            };
            let screen = to_screen(pos, self.pan, self.zoom, bounds);
            let score = self.suggestion_scores.get(&id).copied().unwrap_or(1.0);
            let color = score_color(score);
            let circle = Path::circle(screen, 5.0);
            frame.fill(&circle, color);
        }

        // ── Layer 5: Breadcrumb seeds (previous in history) ─────────────
        for &id in &self.seed_stack {
            if Some(id) == current_seed {
                continue;
            }
            if let Some(&pos) = self.positions.get(&id) {
                let screen = to_screen(pos, self.pan, self.zoom, bounds);
                let ring = Path::circle(screen, 7.0);
                frame.stroke(
                    &ring,
                    Stroke::default()
                        .with_color(Color { a: 0.6, ..COLOR_SEED_ACCENT })
                        .with_width(1.5),
                );
                let dot = Path::circle(screen, 5.0);
                frame.fill(&dot, Color { a: 0.6, ..COLOR_SEED_ACCENT });
            }
        }

        // ── Layer 6: Current seed node ──────────────────────────────────
        if let Some(seed_id) = current_seed {
            if let Some(&pos) = self.positions.get(&seed_id) {
                let screen = to_screen(pos, self.pan, self.zoom, bounds);
                let ring = Path::circle(screen, 9.0);
                frame.stroke(
                    &ring,
                    Stroke::default().with_color(COLOR_SEED_ACCENT).with_width(2.0),
                );
                let dot = Path::circle(screen, 7.0);
                frame.fill(&dot, COLOR_SEED_ACCENT);
            }
        }

        // ── Tooltip overlay for hovered node ────────────────────────────
        if let Some(hovered_id) = self.hovered_id {
            if let (Some(&pos), Some(meta)) =
                (self.positions.get(&hovered_id), self.track_meta.get(&hovered_id))
            {
                let screen = to_screen(pos, self.pan, self.zoom, bounds);

                // Build label text
                let label = if let Some(ref artist) = meta.artist {
                    format!("{} - {}", artist, meta.title)
                } else {
                    meta.title.clone()
                };

                let detail = match (&meta.key, meta.bpm) {
                    (Some(k), Some(b)) => format!("{}  {:.0} BPM", k, b),
                    (Some(k), None) => k.clone(),
                    (None, Some(b)) => format!("{:.0} BPM", b),
                    _ => String::new(),
                };

                // Draw tooltip background
                let tooltip_x = screen.x + 12.0;
                let tooltip_y = screen.y - 28.0;
                let tooltip_w = (label.len().max(detail.len()) as f32 * 7.0).min(300.0) + 12.0;
                let tooltip_h = if detail.is_empty() { 22.0 } else { 36.0 };
                frame.fill_rectangle(
                    Point::new(tooltip_x - 4.0, tooltip_y - 2.0),
                    Size::new(tooltip_w, tooltip_h),
                    Color::from_rgba(0.0, 0.0, 0.0, 0.85),
                );

                frame.fill_text(Text {
                    content: label,
                    position: Point::new(tooltip_x, tooltip_y),
                    color: Color::WHITE,
                    size: 12.0.into(),
                    ..Text::default()
                });

                if !detail.is_empty() {
                    frame.fill_text(Text {
                        content: detail,
                        position: Point::new(tooltip_x, tooltip_y + 14.0),
                        color: Color::from_rgb(0.7, 0.7, 0.7),
                        size: 10.0.into(),
                        ..Text::default()
                    });
                }
            }
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &GraphInteraction,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.drag_start.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if let Some(pos) = cursor.position_in(bounds) {
            let world = from_screen(pos, self.pan, self.zoom, bounds);
            let hit_radius = 7.0 / self.zoom;
            let is_over_node = self.positions.values().any(|&(x, y)| {
                let dx = world.0 - x;
                let dy = world.1 - y;
                (dx * dx + dy * dy).sqrt() < hit_radius
            });
            if is_over_node {
                return mouse::Interaction::Pointer;
            }
            return mouse::Interaction::Grab;
        }
        mouse::Interaction::default()
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Public view function
// ════════════════════════════════════════════════════════════════════════════

/// Create the graph view canvas element.
///
/// The `on_message` closure maps internal `GraphViewMessage` values to the
/// caller's application message type.
pub fn graph_view<'a, Message: 'a>(
    state: &'a GraphViewState,
    on_message: impl Fn(GraphViewMessage) -> Message + 'a,
) -> Element<'a, Message> {
    let canvas: Element<'a, GraphViewMessage> = Canvas::new(state)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    canvas.map(on_message)
}
