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
    /// Original t-SNE positions (preserved for fisheye distortion on seed select)
    pub tsne_positions: HashMap<i64, (f32, f32)>,
    pub pan: (f32, f32),
    pub zoom: f32,
    // Selection (browser-like back/forward navigation)
    /// Full seed history (back + current + forward)
    pub seed_stack: Vec<i64>,
    /// Current position in seed_stack (0-based). Seeds before this are "back", after are "forward".
    pub seed_position: usize,
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
    /// Whether to L2-normalize PCA vectors before distance computation.
    /// On = equal component weight (flatter clusters). Off = natural variance (sharper clusters).
    pub normalize_vectors: bool,
    /// Number of PCA dimensions used (0 = not yet known)
    pub pca_dims: usize,
    /// Transition reach index (0=Tight, 1=Medium, 2=Open) for the graph view
    pub transition_reach_index: usize,
    /// Status message (e.g., "Exported 12 tracks as Set Plan")
    pub status_message: Option<String>,
    // Cluster overlays (multi-scale consensus)
    /// Cluster assignments (track_id → cluster_id, -1 = noise)
    pub clusters: HashMap<i64, i32>,
    /// Per-track cluster confidence [0.0, 1.0] — how consistently this track
    /// clusters with its peers across multiple HDBSCAN scales
    pub cluster_confidence: HashMap<i64, f32>,
    /// Cluster colors (cluster_id → color)
    pub cluster_colors: HashMap<i32, Color>,
    /// Theme-derived stem colors [Vocals, Drums, Bass, Other] for score coloring.
    /// If set, score_best uses stems[0] (Vocals) and score_worst uses arc danger.
    pub stem_colors: Option<[Color; 4]>,
    /// Theme-derived accent color for seed highlighting
    pub accent_color: Option<Color>,
}

impl GraphViewState {
    /// Create an empty graph view state.
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            tsne_positions: HashMap::new(),
            pan: (0.0, 0.0),
            zoom: 10.0,
            seed_stack: Vec::new(),
            seed_position: 0,
            suggestion_ids: HashSet::new(),
            suggestion_scores: HashMap::new(),
            hovered_id: None,
            suggestion_edges: Vec::new(),
            track_meta: HashMap::new(),
            edge_cache: canvas::Cache::new(),
            node_cache: canvas::Cache::new(),
            energy_direction: 0.5,
            normalize_vectors: false, // off by default — preserve natural PCA variance
            pca_dims: 0,
            cluster_confidence: HashMap::new(),
            transition_reach_index: 1, // default: Medium
            status_message: None,
            clusters: HashMap::new(),
            cluster_colors: HashMap::new(),
            stem_colors: None,
            accent_color: None,
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

pub fn to_screen(pos: (f32, f32), pan: (f32, f32), zoom: f32, bounds: Rectangle) -> Point {
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
pub const COLOR_NODE_DIM: Color = Color::from_rgb(0.227, 0.227, 0.227);          // #3a3a3a
pub const COLOR_SEED_ACCENT: Color = Color::from_rgb(0.290, 0.498, 0.647);       // #4a7fa5
pub const COLOR_SCORE_BEST: Color = Color::from_rgb(0.176, 0.541, 0.306);        // #2d8a4e
pub const COLOR_SCORE_WORST: Color = Color::from_rgb(0.651, 0.239, 0.251);       // #a63d40
pub const COLOR_BACKGROUND: Color = Color::from_rgb(0.08, 0.08, 0.08);

/// Linearly interpolate between best (green) and worst (red) based on score.
/// Score is 0..1 where HIGHER = better match (reward-based scoring).
pub fn score_color(score: f32) -> Color {
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
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // Background from theme
        let bg = theme.extended_palette().background.base.color;
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), bg);

        let seed_set: HashSet<i64> = self.seed_stack.iter().copied().collect();
        let current_seed = self.seed_stack.get(self.seed_position).copied();
        let has_seed = current_seed.is_some();
        let seed_accent = self.accent_color.unwrap_or(COLOR_SEED_ACCENT);
        let stems = self.stem_colors;

        // ── Layer 1: Suggestion edges (gray, visible) ──
        let edge_gray = Color::from_rgb(0.5, 0.5, 0.5);
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

            let opacity = score.clamp(0.3, 0.8);
            let width = 0.5 + score * 1.5;

            let path = Path::line(p1, p2);
            frame.stroke(&path, Stroke::default().with_color(Color { a: opacity, ..edge_gray }).with_width(width));
        }

        // ── Layer 2: Seed history trail (fading red line) ──
        // Lines near the current position are vivid, distant ones fade out
        if self.seed_stack.len() >= 2 {
            let trail_base = Color::from_rgb(0.85, 0.2, 0.2);
            let cur = self.seed_position;
            for (seg_idx, window) in self.seed_stack.windows(2).enumerate() {
                let (a, b) = (window[0], window[1]);
                let a_pos = match self.positions.get(&a) {
                    Some(p) => *p,
                    None => continue,
                };
                let b_pos = match self.positions.get(&b) {
                    Some(p) => *p,
                    None => continue,
                };

                // Distance from current position (segment covers seg_idx..seg_idx+1)
                let dist = if seg_idx < cur {
                    (cur - seg_idx - 1) as f32  // segments before current
                } else {
                    (seg_idx - cur) as f32      // segments after current (forward)
                };
                // Fade: 0 = full, 1 = slightly faded, 3+ = very faded
                let alpha = (1.0 - dist * 0.3).clamp(0.08, 0.9);
                let width = if dist < 1.5 { 2.0 } else { 1.0 };

                let p1 = to_screen(a_pos, self.pan, self.zoom, bounds);
                let p2 = to_screen(b_pos, self.pan, self.zoom, bounds);
                let path = Path::line(p1, p2);
                frame.stroke(
                    &path,
                    Stroke::default()
                        .with_color(Color { a: alpha, ..trail_base })
                        .with_width(width),
                );
            }
        }

        // ── Layer 3: Unrelated nodes (cluster-colored with confidence alpha) ──
        let base_alpha = if has_seed { 0.40 } else { 1.0 };
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

            // Color by cluster, alpha by confidence (strong members = vivid, weak = faded)
            let base_color = self.clusters.get(&id)
                .and_then(|&cid| if cid >= 0 { self.cluster_colors.get(&cid) } else { None })
                .copied()
                .unwrap_or(COLOR_NODE_DIM);
            let confidence = self.cluster_confidence.get(&id).copied().unwrap_or(0.2);
            let alpha = (confidence * 0.7 + 0.15) * base_alpha; // range ~0.15 to ~0.85

            let circle = Path::circle(screen, 3.0);
            frame.fill(&circle, Color { a: alpha, ..base_color });
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
            let color = themed_score_color(score, stems);
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
                        .with_color(Color { a: 0.6, ..seed_accent })
                        .with_width(1.5),
                );
                let dot = Path::circle(screen, 5.0);
                frame.fill(&dot, Color { a: 0.6, ..seed_accent });
            }
        }

        // ── Layer 6: Current seed node ──────────────────────────────────
        if let Some(seed_id) = current_seed {
            if let Some(&pos) = self.positions.get(&seed_id) {
                let screen = to_screen(pos, self.pan, self.zoom, bounds);
                let ring = Path::circle(screen, 9.0);
                frame.stroke(
                    &ring,
                    Stroke::default().with_color(seed_accent).with_width(2.0),
                );
                let dot = Path::circle(screen, 7.0);
                frame.fill(&dot, seed_accent);
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

/// Draw the graph into an existing frame (read-only, no interaction).
///
/// Used by the combined browser canvas in mesh-player where the graph shares
/// a single Canvas widget with the energy arc ribbon.
/// `bounds` is the sub-region within the frame where the graph should render.
/// Auto-fits zoom to show all nodes within the bounds.
/// Score color using theme stem colors when available.
fn themed_score_color(score: f32, stem_colors: Option<[Color; 4]>) -> Color {
    match stem_colors {
        Some(stems) => {
            // Vocals stem = good, Bass stem = moderate, hardcoded red = poor
            let t = (1.0 - score).clamp(0.0, 1.0);
            if t < 0.5 {
                // Good → moderate (vocals → bass stem)
                let s = t * 2.0;
                Color::from_rgb(
                    stems[0].r + (stems[2].r - stems[0].r) * s,
                    stems[0].g + (stems[2].g - stems[0].g) * s,
                    stems[0].b + (stems[2].b - stems[0].b) * s,
                )
            } else {
                // Moderate → poor (bass stem → red)
                let s = (t - 0.5) * 2.0;
                Color::from_rgb(
                    stems[2].r + (COLOR_SCORE_WORST.r - stems[2].r) * s,
                    stems[2].g + (COLOR_SCORE_WORST.g - stems[2].g) * s,
                    stems[2].b + (COLOR_SCORE_WORST.b - stems[2].b) * s,
                )
            }
        }
        None => score_color(score),
    }
}

pub fn draw_graph_readonly(state: &GraphViewState, frame: &mut canvas::Frame, bounds: Rectangle, bg_color: Option<Color>) {
    // Background from theme or fallback
    frame.fill_rectangle(
        Point::new(bounds.x, bounds.y),
        bounds.size(),
        bg_color.unwrap_or(COLOR_BACKGROUND),
    );

    if state.positions.is_empty() {
        return;
    }

    // Auto-fit: use 2nd/98th percentile bounds to trim t-SNE outliers
    let mut xs: Vec<f32> = state.positions.values().map(|p| p.0).collect();
    let mut ys: Vec<f32> = state.positions.values().map(|p| p.1).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = (xs.len() as f32 * 0.02) as usize;
    let hi = ((xs.len() as f32 * 0.98) as usize).max(lo + 1).min(xs.len() - 1);
    let min_x = xs[lo]; let max_x = xs[hi];
    let min_y = ys[lo]; let max_y = ys[hi];
    let data_w = (max_x - min_x).max(0.001);
    let data_h = (max_y - min_y).max(0.001);
    let margin = 6.0;
    let usable_w = (bounds.width - margin * 2.0).max(1.0);
    let usable_h = (bounds.height - margin * 2.0).max(1.0);
    let zoom = (usable_w / data_w).min(usable_h / data_h);
    let center_x = (min_x + max_x) / 2.0;
    let center_y = (min_y + max_y) / 2.0;
    let pan = (-center_x, -center_y);

    // Offset bounds for to_screen: graph draws relative to its sub-region
    let graph_bounds = Rectangle {
        x: 0.0, y: 0.0,
        width: bounds.width, height: bounds.height,
    };

    let seed_set: HashSet<i64> = state.seed_stack.iter().copied().collect();
    let current_seed = state.seed_stack.get(state.seed_position).copied();
    let has_seed = current_seed.is_some();
    let seed_accent = state.accent_color.unwrap_or(COLOR_SEED_ACCENT);
    let stems = state.stem_colors;

    // Offset all points by bounds origin (no frame.translate — avoids coordinate bugs)
    let ox = bounds.x;
    let oy = bounds.y;
    // Snap to whole pixels to avoid blurry anti-aliased sub-pixel circles
    let pt = |pos: (f32, f32)| -> Point {
        let s = to_screen(pos, pan, zoom, graph_bounds);
        Point::new((s.x + ox).round(), (s.y + oy).round())
    };

    // ── Layer 1: Edges only for the selected (hovered) track ──
    if let Some(hovered_id) = state.hovered_id {
        let edge_gray = Color::from_rgb(0.55, 0.55, 0.55);
        for &(from, to, score) in &state.suggestion_edges {
            if to != hovered_id { continue; }
            let from_pos = match state.positions.get(&from) { Some(p) => *p, None => continue };
            let to_pos = match state.positions.get(&to) { Some(p) => *p, None => continue };
            let opacity = score.clamp(0.5, 0.95);
            let width = 1.5 + score * 1.5;
            frame.stroke(&Path::line(pt(from_pos), pt(to_pos)), Stroke::default().with_color(Color { a: opacity, ..edge_gray }).with_width(width));
        }
    }

    // ── Layer 2: Seed history trail ──
    if state.seed_stack.len() >= 2 {
        let trail_base = Color::from_rgb(0.85, 0.2, 0.2);
        let cur = state.seed_position;
        for (seg_idx, window) in state.seed_stack.windows(2).enumerate() {
            let (a, b) = (window[0], window[1]);
            let a_pos = match state.positions.get(&a) { Some(p) => *p, None => continue };
            let b_pos = match state.positions.get(&b) { Some(p) => *p, None => continue };
            let dist = if seg_idx < cur { (cur - seg_idx - 1) as f32 } else { (seg_idx - cur) as f32 };
            let alpha = (1.0 - dist * 0.3).clamp(0.08, 0.9);
            let width = if dist < 1.5 { 2.0 } else { 1.0 };
            frame.stroke(&Path::line(pt(a_pos), pt(b_pos)), Stroke::default().with_color(Color { a: alpha, ..trail_base }).with_width(width));
        }
    }

    // ── Layer 3: Base nodes (cluster-colored, pixel-snapped) ──
    let base_alpha = if has_seed { 0.45 } else { 1.0 };
    for (&id, &pos) in &state.positions {
        if seed_set.contains(&id) { continue; }
        let screen = pt(pos);
        if screen.x < ox - 5.0 || screen.y < oy - 5.0 || screen.x > ox + bounds.width + 5.0 || screen.y > oy + bounds.height + 5.0 { continue; }

        // Suggestion nodes get a slightly brighter treatment
        let is_suggestion = state.suggestion_ids.contains(&id);
        let base_color = state.clusters.get(&id)
            .and_then(|&cid| if cid >= 0 { state.cluster_colors.get(&cid) } else { None })
            .copied()
            .unwrap_or(COLOR_NODE_DIM);
        let confidence = state.cluster_confidence.get(&id).copied().unwrap_or(0.2);
        let alpha = if is_suggestion {
            (confidence * 0.5 + 0.4).min(0.9)
        } else {
            (confidence * 0.7 + 0.15) * base_alpha
        };
        let radius = if is_suggestion { 4.0 } else { 2.0 };
        frame.fill(&Path::circle(screen, radius), Color { a: alpha, ..base_color });
    }

    // ── Layer 4: Selected track highlight (ring + dot) ──
    if let Some(hovered_id) = state.hovered_id {
        if let Some(&pos) = state.positions.get(&hovered_id) {
            let screen = pt(pos);
            let color = state.suggestion_scores.get(&hovered_id)
                .map(|&s| themed_score_color(s, stems))
                .unwrap_or(seed_accent);
            frame.stroke(&Path::circle(screen, 7.0), Stroke::default().with_color(Color { a: 0.9, ..color }).with_width(2.0));
            frame.fill(&Path::circle(screen, 5.0), Color { a: 0.9, ..color });
        }
    }

    // ── Layer 5+6: All seed nodes (playing decks) ──
    for &id in &state.seed_stack {
        if let Some(&pos) = state.positions.get(&id) {
            let screen = pt(pos);
            frame.stroke(&Path::circle(screen, 7.0), Stroke::default().with_color(seed_accent).with_width(2.0));
            frame.fill(&Path::circle(screen, 5.0), seed_accent);
        }
    }
}
