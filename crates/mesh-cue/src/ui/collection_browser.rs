//! Collection browser view with hierarchical playlist navigation and graph view

use super::app::{BrowserSide, CollectionState, ImportState, Message};
use super::editor;
use super::state::BrowserTab;
use iced::widget::{button, column, container, row, rule, slider, text, toggler, Canvas, Space};
use iced::{Alignment, Color, Element, Length};
use mesh_core::music::MusicalKey;
use mesh_core::suggestions::scoring::{base_score, classify_transition, transition_type_label};
use mesh_widgets::track_table::TrackRow;
use mesh_widgets::{
    energy_arc, playlist_browser_with_drop_highlight, sz, track_table,
    ArcPoint, ArcTransition, EnergyArcState,
};

/// Render the collection view (editor + dual browsers below)
pub fn view<'a>(
    state: &'a CollectionState,
    _import_state: &'a ImportState,
    stem_link_selection: Option<usize>,
) -> Element<'a, Message> {
    let editor = view_editor(state, stem_link_selection);
    let browser_header = view_browser_header(state);
    let browser_content = match state.active_tab {
        BrowserTab::List => view_browsers(state),
        BrowserTab::Graph => view_graph(state),
    };

    column![
        editor,
        rule::horizontal(2),
        browser_header,
        browser_content,
    ]
    .spacing(5)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Header row with tab buttons + Import/Export
fn view_browser_header(state: &CollectionState) -> Element<'_, Message> {
    let list_btn = {
        let btn = button(text("List").size(sz(13.0))).padding([3, 10]);
        if state.active_tab == BrowserTab::List {
            btn.style(button::primary)
        } else {
            btn.on_press(Message::SetBrowserTab(BrowserTab::List))
                .style(button::secondary)
        }
    };

    let graph_btn = {
        let label = if state.graph_building {
            "Graph..."
        } else {
            "Graph"
        };
        let btn = button(text(label).size(sz(13.0))).padding([3, 10]);
        if state.active_tab == BrowserTab::Graph {
            btn.style(button::primary)
        } else {
            btn.on_press(Message::SetBrowserTab(BrowserTab::Graph))
                .style(button::secondary)
        }
    };

    let import_btn = button(text("Import").size(sz(14.0)))
        .on_press(Message::OpenImport)
        .style(button::secondary)
        .padding([4, 12]);

    let export_btn = button(text("Export").size(sz(14.0)))
        .on_press(Message::OpenExport)
        .style(button::secondary)
        .padding([4, 12]);

    container(
        row![
            list_btn,
            graph_btn,
            Space::new().width(Length::Fill),
            import_btn,
            export_btn,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([0, 8]),
    )
    .width(Length::Fill)
    .into()
}

/// Track editor (top section)
fn view_editor(state: &CollectionState, stem_link_selection: Option<usize>) -> Element<'_, Message> {
    if let Some(ref loaded) = state.loaded_track {
        editor::view(loaded, stem_link_selection, state.stem_colors)
    } else {
        container(
            column![
                text("No track loaded").size(sz(18.0)),
                Space::new().height(20.0),
                text("Select a track from the browser below to load it for editing.").size(sz(14.0)),
            ]
            .spacing(10),
        )
        .padding(15)
        .width(Length::Fill)
        .height(Length::FillPortion(2))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    }
}

/// Dual playlist browsers (bottom section — List tab)
fn view_browsers(state: &CollectionState) -> Element<'_, Message> {
    let (left_is_drop_target, right_is_drop_target) = match (&state.dragging_track, &state.drag_hover_browser) {
        (Some(drag), Some(hover)) => {
            let left_hovering = *hover == BrowserSide::Left && drag.source_browser != BrowserSide::Left;
            let right_hovering = *hover == BrowserSide::Right && drag.source_browser != BrowserSide::Right;
            (left_hovering, right_hovering)
        }
        _ => (false, false),
    };

    let left_browser = playlist_browser_with_drop_highlight(
        &state.tree_nodes,
        &state.left_tracks,
        &state.browser_left,
        |msg| Message::BrowserLeft(msg),
        left_is_drop_target,
    );

    let right_browser = playlist_browser_with_drop_highlight(
        &state.tree_nodes,
        &state.right_tracks,
        &state.browser_right,
        |msg| Message::BrowserRight(msg),
        right_is_drop_target,
    );

    let browsers = row![
        container(left_browser)
            .width(Length::FillPortion(1))
            .height(Length::Fill),
        rule::vertical(2),
        container(right_browser)
            .width(Length::FillPortion(1))
            .height(Length::Fill),
    ]
    .spacing(0)
    .height(Length::FillPortion(1));

    if let Some(ref arc_state) = state.energy_arc {
        let arc_el: Element<'_, Message> = energy_arc(arc_state);

        column![arc_el, browsers]
            .spacing(0)
            .height(Length::FillPortion(1))
            .into()
    } else {
        browsers.into()
    }
}

// ── Energy arc helpers ──────────────────────────────────────────────

/// Build an `EnergyArcState` from the current track list.
///
/// Uses BPM as the intensity proxy (normalized to roughly [0, 1] in the
/// typical DJ range of 100-200 BPM). Key transitions are computed via
/// mesh-core's Camelot classification and colored by base compatibility
/// score (green >= 0.70, amber >= 0.40, red < 0.40).
///
/// Returns `None` if fewer than 2 tracks have key data.
/// Build an `EnergyArcState` from the current track list.
///
/// `consecutive_similarities`: optional cosine distances between consecutive
/// track PCA embeddings (len = tracks.len() - 1). Pass empty slice if unavailable.
/// Theme-derived colors for arc transitions.
#[derive(Clone)]
pub struct ArcThemeColors {
    pub good: Color,    // compatible transitions (success)
    pub moderate: Color, // moderate transitions (warning)
    pub poor: Color,    // poor transitions (danger)
    pub unknown: Color, // no key data
    pub stems: [Color; 4],
}

pub fn build_energy_arc<Id: Clone>(
    tracks: &[TrackRow<Id>],
    current_index: usize,
    consecutive_similarities: &[f32],
    theme_colors: ArcThemeColors,
) -> Option<EnergyArcState> {
    let has_key_data = tracks.iter().filter(|t| t.key.is_some()).count() >= 2;
    if !has_key_data {
        return None;
    }
    Some(build_energy_arc_inner(tracks, current_index, consecutive_similarities, theme_colors))
}

fn build_energy_arc_inner<Id: Clone>(
    tracks: &[TrackRow<Id>],
    current_index: usize,
    consecutive_similarities: &[f32],
    theme_colors: ArcThemeColors,
) -> EnergyArcState {
    let points: Vec<ArcPoint> = tracks
        .iter()
        .map(|t| {
            // Normalize BPM to [0, 1] in the 80-200 range
            // Use ML intensity only — no BPM fallback (BPM doesn't indicate aggression)
            let intensity = t.intensity.unwrap_or(0.5);
            ArcPoint {
                title: t.title.clone(),
                intensity,
                key: t.key.clone(),
                bpm: t.bpm,
            }
        })
        .collect();

    let transitions: Vec<ArcTransition> = points
        .windows(2)
        .enumerate()
        .map(|(idx, w)| {
            let sim_dist = consecutive_similarities.get(idx).copied().unwrap_or(0.3);
            let key_a = w[0].key.as_deref().and_then(MusicalKey::parse);
            let key_b = w[1].key.as_deref().and_then(MusicalKey::parse);
            match (key_a, key_b) {
                (Some(a), Some(b)) => {
                    let tt = classify_transition(&a, &b);
                    let bs = base_score(tt);
                    let label = transition_type_label(tt);
                    let color = if bs >= 0.70 {
                        theme_colors.good
                    } else if bs >= 0.40 {
                        theme_colors.moderate
                    } else {
                        theme_colors.poor
                    };
                    ArcTransition { label, color, similarity_distance: sim_dist }
                }
                _ => ArcTransition {
                    label: "?",
                    color: theme_colors.unknown,
                    similarity_distance: sim_dist,
                },
            }
        })
        .collect();

    EnergyArcState {
        points,
        transitions,
        current_index,
        stem_colors: theme_colors.stems,
    }
}

/// Graph view (Graph tab) — left panel analysis + right panel graph canvas
fn view_graph<'a>(state: &'a CollectionState) -> Element<'a, Message> {
    match state.graph_state.as_ref() {
        None => {
            // Graph not yet built — show loading
            container(
                text(if state.graph_building {
                    "Building suggestion graph..."
                } else {
                    "Click Graph tab to build the suggestion graph."
                })
                .size(sz(16.0)),
            )
            .width(Length::Fill)
            .height(Length::FillPortion(1))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
        }
        Some(graph_state) => {
            // Breadcrumb trail
            let breadcrumbs = view_breadcrumbs(graph_state);

            // Controls row: energy slider + normalize toggle
            let controls = container(
                row![
                    text("Drop").size(sz(11.0)),
                    slider(0.0..=1.0, graph_state.energy_direction, Message::GraphSliderChanged)
                        .step(0.01)
                        .width(Length::Fill),
                    text("Peak").size(sz(11.0)),
                    Space::new().width(12.0),
                    text("Reach").size(sz(10.0)),
                    {
                        let idx = graph_state.transition_reach_index;
                        let labels = ["Tight", "Med", "Open"];
                        let label = labels[idx.min(2)];
                        button(text(label).size(sz(10.0)))
                            .on_press(Message::GraphTransitionReach((idx + 1) % 3))
                            .style(button::secondary)
                            .padding([2, 6])
                    },
                    Space::new().width(8.0),
                    text("Norm").size(sz(10.0)),
                    toggler(graph_state.normalize_vectors)
                        .on_toggle(Message::GraphToggleNormalize)
                        .size(sz(14.0)),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([2, 8]),
            )
            .width(Length::Fill);

            // Legend with PCA dims info
            let dims_info = if graph_state.pca_dims > 0 {
                format!("PCA: {}d", graph_state.pca_dims)
            } else {
                "PCA: ?".to_string()
            };
            let tracks_info = format!("{} tracks", graph_state.track_meta.len());
            let clusters_info = {
                let n_clusters = graph_state.cluster_colors.len();
                if n_clusters > 0 { format!("{} clusters", n_clusters) } else { String::new() }
            };
            let status_or_help = if let Some(ref msg) = graph_state.status_message {
                text(msg.clone()).size(sz(10.0))
            } else {
                text("Drop \u{2190} Intent \u{2192} Peak | Reach: transition distance | Norm: t-SNE mode").size(sz(10.0))
            };
            let legend = container(
                row![
                    text(dims_info).size(sz(10.0)),
                    text(" | ").size(sz(10.0)),
                    text(tracks_info).size(sz(10.0)),
                    text(if clusters_info.is_empty() { "" } else { " | " }).size(sz(10.0)),
                    text(clusters_info).size(sz(10.0)),
                    text(" | ").size(sz(10.0)),
                    status_or_help,
                ]
                .spacing(0)
                .align_y(Alignment::Center)
                .padding([2, 8]),
            )
            .width(Length::Fill);

            // Graph canvas — map GraphViewMessage to app Message
            let graph_canvas: Element<'a, mesh_widgets::GraphViewMessage> = Canvas::new(graph_state)
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
            let graph_canvas: Element<'a, Message> = graph_canvas.map(|gvm| match gvm {
                mesh_widgets::GraphViewMessage::SeedSelected(id) => Message::GraphSeedSelected(id),
                mesh_widgets::GraphViewMessage::NodeHovered(id) => Message::GraphNodeHovered(id),
                mesh_widgets::GraphViewMessage::SliderChanged(v) => Message::GraphSliderChanged(v),
                mesh_widgets::GraphViewMessage::PanZoomChanged { pan, zoom } => Message::GraphPanZoom { pan, zoom },
            });

            let right_panel = column![
                controls,
                legend,
                graph_canvas,
            ]
            .spacing(2)
            .width(Length::FillPortion(1))
            .height(Length::Fill);

            // Left panel: breadcrumbs + suggestion list
            let suggestion_content: Element<'a, Message> = if state.graph_suggestion_rows.is_empty() {
                container(
                    text("Select a node to see suggestions").size(sz(12.0))
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
            } else {
                // Use the standard track_table widget for consistent look
                track_table(
                    &state.graph_suggestion_rows,
                    &state.graph_table_state,
                    |msg| Message::GraphTable(msg),
                )
            };

            let suggestion_count = state.graph_suggestion_rows.len();
            let header_text = if suggestion_count > 0 {
                format!("Suggestions ({})", suggestion_count)
            } else {
                "Suggestions".to_string()
            };

            let left_panel = column![
                breadcrumbs,
                text(header_text).size(sz(13.0)),
                suggestion_content,
            ]
            .spacing(4)
            .padding([0, 4])
            .width(Length::FillPortion(1))
            .height(Length::Fill);

            row![
                left_panel,
                rule::vertical(2),
                right_panel,
            ]
            .spacing(0)
            .height(Length::FillPortion(1))
            .into()
        }
    }
}

/// Seed history header with back/forward navigation and export button.
/// Shows ±2 tracks around current position in a scrollable row.
fn view_breadcrumbs(graph_state: &mesh_widgets::GraphViewState) -> Element<'_, Message> {
    if graph_state.seed_stack.is_empty() {
        return container(text("No seed selected — click a node").size(sz(12.0)))
            .padding([4, 8])
            .into();
    }

    let pos = graph_state.seed_position;
    let len = graph_state.seed_stack.len();
    let mut header = row![].spacing(4).align_y(Alignment::Center);

    // Back button
    let back_btn = button(text("\u{25C0}").size(sz(11.0))).padding([2, 4]);
    if pos > 0 {
        header = header.push(back_btn.on_press(Message::GraphSeedBack).style(button::secondary));
    } else {
        header = header.push(back_btn.style(button::secondary));
    }

    // Show window of tracks: ±2 around current position
    let window_start = pos.saturating_sub(2);
    let window_end = (pos + 3).min(len); // exclusive

    if window_start > 0 {
        header = header.push(text("...").size(sz(10.0)));
    }

    for i in window_start..window_end {
        let seed_id = graph_state.seed_stack[i];
        let label: String = graph_state.track_meta.get(&seed_id)
            .map(|m| {
                let display = if let Some(ref artist) = m.artist {
                    format!("{} - {}", artist, m.title)
                } else {
                    m.title.clone()
                };
                display.chars().take(20).collect::<String>()
            })
            .unwrap_or_else(|| format!("#{}", seed_id));

        if i > window_start {
            header = header.push(text("\u{2192}").size(sz(10.0))); // →
        }

        if i == pos {
            // Current — bold/highlighted
            header = header.push(
                container(text(label).size(sz(11.0)))
                    .padding([2, 4])
                    .style(|_: &iced::Theme| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgb(0.25, 0.35, 0.50))),
                        border: iced::Border { radius: 3.0.into(), ..Default::default() },
                        ..Default::default()
                    })
            );
        } else {
            // Clickable
            header = header.push(
                button(text(label).size(sz(10.0)))
                    .on_press(Message::GraphSeedSelected(seed_id))
                    .style(button::text)
                    .padding([1, 4]),
            );
        }
    }

    if window_end < len {
        header = header.push(text("...").size(sz(10.0)));
    }

    // Forward button
    let fwd_btn = button(text("\u{25B6}").size(sz(11.0))).padding([2, 4]);
    if pos + 1 < len {
        header = header.push(fwd_btn.on_press(Message::GraphSeedForward).style(button::secondary));
    } else {
        header = header.push(fwd_btn.style(button::secondary));
    }

    // Export button
    header = header.push(Space::new().width(Length::Fill));
    header = header.push(
        button(text(format!("Export ({})", len)).size(sz(10.0)))
            .on_press(Message::GraphExportPlaylist)
            .style(button::secondary)
            .padding([2, 8]),
    );

    // Track count
    header = header.push(text(format!("{}/{}", pos + 1, len)).size(sz(10.0)));

    container(header).padding([4, 8]).into()
}
