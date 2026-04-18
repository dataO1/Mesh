//! Collection browser view with hierarchical playlist navigation and graph view

use super::app::{BrowserSide, CollectionState, ImportState, Message};
use super::editor;
use super::state::BrowserTab;
use iced::widget::{button, column, container, row, rule, slider, text, toggler, Canvas, Space};
use iced::{Alignment, Element, Length};
use mesh_widgets::{playlist_browser_with_drop_highlight, sz, track_table};

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

    row![
        container(left_browser)
            .width(Length::FillPortion(1))
            .height(Length::Fill),
        rule::vertical(2),
        container(right_browser)
            .width(Length::FillPortion(1))
            .height(Length::Fill),
    ]
    .spacing(0)
    .height(Length::FillPortion(1))
    .into()
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
                    Space::new().width(16.0),
                    text("Normalize").size(sz(10.0)),
                    toggler(graph_state.normalize_vectors)
                        .on_toggle(Message::GraphToggleNormalize)
                        .size(sz(14.0)),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([2, 8]),
            )
            .width(Length::Fill);

            // Legend
            let legend = container(
                row![
                    text("Nodes = tracks").size(sz(10.0)),
                    text(" | ").size(sz(10.0)),
                    text("Edges = composite suggestion score (key + HNSW + energy + co-play)").size(sz(10.0)),
                    text(" | ").size(sz(10.0)),
                    text("Green/thick = best match").size(sz(10.0)),
                    text(" | ").size(sz(10.0)),
                    text("Red/thin = weakest").size(sz(10.0)),
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

/// Breadcrumb trail showing seed navigation history
fn view_breadcrumbs(graph_state: &mesh_widgets::GraphViewState) -> Element<'_, Message> {
    if graph_state.seed_stack.is_empty() {
        return container(text("No seed selected").size(sz(12.0)))
            .padding([4, 8])
            .into();
    }

    let mut crumbs = row![].spacing(4).align_y(Alignment::Center);

    // Back button
    if graph_state.seed_stack.len() > 1 {
        crumbs = crumbs.push(
            button(text("<").size(sz(12.0)))
                .on_press(Message::GraphSeedBack)
                .style(button::secondary)
                .padding([2, 6]),
        );
    }

    // Show breadcrumb labels
    for (i, &seed_id) in graph_state.seed_stack.iter().enumerate() {
        let label: String = graph_state.track_meta.get(&seed_id)
            .map(|m| {
                let display = if let Some(ref artist) = m.artist {
                    format!("{} - {}", artist, m.title)
                } else {
                    m.title.clone()
                };
                display.chars().take(25).collect::<String>()
            })
            .unwrap_or_else(|| format!("#{}", seed_id));

        if i > 0 {
            crumbs = crumbs.push(text(" > ").size(sz(11.0)));
        }

        let is_current = i == graph_state.seed_stack.len() - 1;
        if is_current {
            crumbs = crumbs.push(
                text(label).size(sz(12.0)),
            );
        } else {
            crumbs = crumbs.push(
                button(text(label).size(sz(11.0)))
                    .on_press(Message::GraphSeedSelected(seed_id))
                    .style(button::text)
                    .padding([1, 4]),
            );
        }
    }

    container(crumbs).padding([4, 8]).into()
}
