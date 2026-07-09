use super::*;

/// A quiet fixed-size surface used behind pane content. It stays semantically
/// transparent; the content layered above it owns the meaning.
pub(crate) struct Surface {
    size: Size,
    fill: Color,
    outline: Color,
    radius: f32,
}

impl Surface {
    fn new(width: f32, height: f32, fill: Color) -> Self {
        Self {
            size: Size { width, height },
            fill,
            outline: Color::rgb(0xbc, 0xb3, 0xa5),
            radius: 7.0,
        }
    }
}

impl View for Surface {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Image, parent);
        ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
        let rect = node_rect(ctx.scene, id, this.size);
        let pd = ctx.scene.paint_mut(id);
        pd.primitives.push(Primitive::SolidRect {
            rect,
            color: this.outline,
            corner_radius: this.radius,
        });
        pd.primitives.push(Primitive::SolidRect {
            rect: Rect::new(
                rect.x + 1.0,
                rect.y + 1.0,
                rect.width - 2.0,
                rect.height - 2.0,
            ),
            color: this.fill,
            corner_radius: this.radius - 1.0,
        });
        ctx.layout
            .set_measure(id, Box::new(move |_available| this.size));
        id
    }
}

pub(crate) fn framed(width: f32, height: f32, fill: Color, content: impl View) -> impl View {
    Stack::new()
        .size(width, height)
        .child(Surface::new(width, height, fill))
        .child(Pad::all(18.0).child(content))
}

pub(crate) fn library_button(
    runtime: WorkspaceRuntime,
    label: &'static str,
    kind: WidgetType,
    width: f32,
) -> Button {
    Button::new(label)
        .width(width)
        .on_click(move || add_widget(&runtime, kind))
}

pub(crate) struct NamedSwitch {
    name: &'static str,
    switch: Switch,
}

impl NamedSwitch {
    fn new(name: &'static str, switch: Switch) -> Self {
        Self { name, switch }
    }
}

impl View for NamedSwitch {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = Box::new(this.switch).build(ctx, parent);
        ctx.scene.a11y_mut(id).name = Some(this.name.into());
        id
    }
}

pub(crate) fn canvas_setting(
    label: &'static str,
    value: bool,
    width: f32,
    on_toggle: impl FnMut(bool) + 'static,
) -> impl View {
    Row::new()
        .width(width)
        .justify(Justify::SpaceBetween)
        .align(Align::Center)
        .child(Text::new(label).size(13.0))
        .child(NamedSwitch::new(
            label,
            Switch::new(value).on_toggle(on_toggle),
        ))
}

pub(crate) fn sidebar(
    runtime: WorkspaceRuntime,
    state: &WorkspaceState,
    width: f32,
    height: f32,
) -> impl View {
    let compact = state.compact;
    let hide_single_tab_bars = state.hide_single_tab_bars;
    let inner_width = width - 36.0;
    framed(
        width,
        height,
        PANEL_ALT,
        Column::new()
            .width(inner_width)
            .gap(13.0)
            .child(Text::new("WIDGET LIBRARY").size(12.0))
            .child(
                Text::new("Build your own rhythm.")
                    .size(24.0)
                    .wrap(WrapMode::Word),
            )
            .child(
                Text::new("Add, arrange, and dock every view where it works for you.")
                    .size(13.0)
                    .wrap(WrapMode::Word),
            )
            .child(Divider::new())
            .child(library_button(
                runtime.clone(),
                "+  PULSE",
                WidgetType::Pulse,
                inner_width - 4.0,
            ))
            .child(library_button(
                runtime.clone(),
                "+  FOCUS TIMER",
                WidgetType::Focus,
                inner_width - 4.0,
            ))
            .child(library_button(
                runtime.clone(),
                "+  NOTES",
                WidgetType::Notes,
                inner_width - 4.0,
            ))
            .child(library_button(
                runtime.clone(),
                "+  WEATHER",
                WidgetType::Weather,
                inner_width - 4.0,
            ))
            .child(Divider::new())
            .child(Text::new("CANVAS").size(11.0))
            .child(canvas_setting("Compact panes", compact, inner_width, {
                let runtime = runtime.clone();
                move |value| {
                    runtime.update(|store| store.compact = value);
                    runtime.request_remount();
                }
            }))
            .child(canvas_setting(
                "Hide single tab bars",
                hide_single_tab_bars,
                inner_width,
                {
                    let runtime = runtime.clone();
                    move |value| {
                        runtime.update(|store| store.hide_single_tab_bars = value);
                        runtime.request_remount();
                    }
                },
            ))
            .child(Spacer::new())
            .child(Badge::new(format!("{} LIVE WIDGETS", state.tabs.len())))
            .child(Text::new("TIP  Grab any tab to move it.").size(11.0)),
    )
}

pub(crate) fn widget_body(tab: &WorkspaceTab, width: f32, compact: bool) -> Column {
    let chart_width = width - 54.0;
    let content_height = if compact { 108.0 } else { 144.0 };
    let narrow = width < 320.0;
    let mut header = Row::new()
        .width(width - 36.0)
        .justify(Justify::SpaceBetween)
        .align(Align::Center)
        .child(
            Column::new()
                .gap(2.0)
                .child(Text::new(format!("{}  {}", tab.widget.marker(), tab.title)).size(22.0))
                .child(Text::new("LIVE / PERSONAL VIEW").size(10.0)),
        );
    if !narrow {
        header = header.child(Badge::new("SYNCED"));
    }

    let body = match tab.widget {
        WidgetType::Pulse => Column::new()
            .gap(10.0)
            .child(
                Row::new()
                    .gap(22.0)
                    .wrap()
                    .child(
                        Column::new()
                            .child(Text::new("72").size(34.0))
                            .child(Text::new("ENERGY").size(10.0)),
                    )
                    .child(
                        Column::new()
                            .child(Text::new("6.4h").size(34.0))
                            .child(Text::new("DEEP WORK").size(10.0)),
                    ),
            )
            .child(
                LineChart::new([28.0, 41.0, 37.0, 58.0, 52.0, 71.0, 66.0, 82.0])
                    .title("Weekly energy")
                    .size(chart_width, content_height)
                    .color(ORANGE)
                    .stroke_width(3.0)
                    .markers(true),
            ),
        WidgetType::Focus => Column::new()
            .gap(13.0)
            .child(Text::new("42:18").size(48.0))
            .child(
                Text::new("Strategy sprint · no notifications")
                    .size(13.0)
                    .wrap(WrapMode::Word),
            )
            .child(ProgressBar::new(68.0, 0.0, 100.0))
            .child(
                Row::new()
                    .gap(8.0)
                    .child(Badge::new("68%"))
                    .child(Text::new("18 minutes left").size(12.0)),
            ),
        WidgetType::Notes => Column::new()
            .gap(12.0)
            .child(Text::new("TODAY'S THREAD").size(11.0))
            .child(
                Text::new("→ Prototype the docking flow")
                    .size(if narrow { 13.0 } else { 16.0 })
                    .wrap(WrapMode::Word),
            )
            .child(
                Text::new("→ Keep the canvas calm")
                    .size(if narrow { 13.0 } else { 16.0 })
                    .wrap(WrapMode::Word),
            )
            .child(
                Text::new("→ Send the Friday field note")
                    .size(if narrow { 13.0 } else { 16.0 })
                    .wrap(WrapMode::Word),
            )
            .child(Divider::new())
            .child(Text::new("3 notes · edited 8m ago").size(12.0)),
        WidgetType::Weather => Column::new()
            .gap(12.0)
            .child(
                Row::new()
                    .gap(18.0)
                    .align(Align::Center)
                    .child(Text::new("18°").size(52.0))
                    .child(
                        Column::new()
                            .gap(2.0)
                            .child(Text::new("BERLIN").size(12.0))
                            .child(Text::new("Clear, soft wind").size(15.0)),
                    ),
            )
            .child(
                BarChart::new([12.0, 14.0, 17.0, 18.0, 16.0, 13.0])
                    .title("Today's temperature")
                    .size(chart_width, content_height)
                    .color(CYAN)
                    .baseline(false),
            ),
        WidgetType::Agent => Column::new()
            .gap(12.0)
            .child(
                Row::new()
                    .gap(8.0)
                    .align(Align::Center)
                    .child(Badge::new("READY"))
                    .child(Text::new("LOCAL WORKSPACE AGENT").size(11.0)),
            )
            .child(
                Text::new("What should we build next?")
                    .size(if narrow { 22.0 } else { 30.0 })
                    .wrap(WrapMode::Word),
            )
            .child(
                Text::new(
                    "Describe a change, ask for a codebase tour, or hand off a failing test.",
                )
                .size(13.0)
                .wrap(WrapMode::Word),
            )
            .child(Divider::new())
            .child(Text::new("No task running · context is ready").size(12.0)),
        WidgetType::Terminal => Column::new()
            .gap(10.0)
            .child(Text::new("$ cargo test --workspace").size(15.0))
            .child(Text::new("   Finished test profile").size(13.0))
            .child(Text::new("   195 passed · 0 failed").size(13.0))
            .child(Divider::new())
            .child(Text::new("$ _").size(15.0)),
    };

    Column::new()
        .width(width - 36.0)
        .gap(15.0)
        .child(header)
        .child(Divider::new())
        .child(body)
}

pub(crate) fn pane_view(
    runtime: WorkspaceRuntime,
    state: &WorkspaceState,
    pane: &PaneState,
    width: f32,
    height: f32,
) -> impl View {
    let reorder_pane = pane.id;
    let reorder_runtime = runtime.clone();
    let mut tabs = TabBar::new().gap(4.0).on_reorder(move |from, to| {
        if reorder_pane_tabs(&reorder_runtime, reorder_pane, from, to) {
            reorder_runtime.request_remount();
        }
    });
    for tab_id in &pane.tabs {
        let tab = tab_by_id(state, *tab_id);
        let (id, pane_id, before_id) = (tab.id, pane.id, tab.id);
        let label = format!("{} {}", tab.widget.marker(), tab.title);
        let select_runtime = runtime.clone();
        let drag_start_runtime = runtime.clone();
        let drag_end_runtime = runtime.clone();
        let drop_runtime = runtime.clone();
        tabs = tabs.child(
            Tab::new(label)
                .selected(pane.active == tab.id)
                .on_select(move || {
                    select_runtime.update(|store| {
                        if let Some(pane) = store.panes.iter_mut().find(|pane| pane.id == pane_id) {
                            pane.active = id;
                        }
                    });
                    select_runtime.request_remount();
                })
                .on_drag_start(move || {
                    drag_start_runtime.update(|store| store.dragging = Some(id));
                })
                .on_drag_end(move |_| {
                    drag_end_runtime.update(|store| store.dragging = None);
                })
                .on_drop(move || {
                    if move_dragged_to_pane(&drop_runtime, pane_id, Some(before_id)) {
                        drop_runtime.request_remount();
                    }
                }),
        );
    }
    let agent_pane = pane.id;
    let terminal_pane = pane.id;
    let agent_runtime = runtime.clone();
    let terminal_runtime = runtime.clone();
    tabs = tabs.trailing(
        Row::new()
            .gap(2.0)
            .align(Align::Center)
            .child(
                Button::new("+ AGENT")
                    .appearance(ButtonAppearance::Ghost)
                    .on_click(move || {
                        add_widget_to_pane(&agent_runtime, WidgetType::Agent, Some(agent_pane));
                    }),
            )
            .child(
                Button::new("+ TERMINAL")
                    .appearance(ButtonAppearance::Ghost)
                    .on_click(move || {
                        add_widget_to_pane(
                            &terminal_runtime,
                            WidgetType::Terminal,
                            Some(terminal_pane),
                        );
                    }),
            ),
    );
    if width < 360.0 {
        tabs = tabs.wrap();
    }

    let active = tab_by_id(state, pane.active);
    let compact = state.compact || height < 390.0;
    let show_tab_bar = !state.hide_single_tab_bars || pane.tabs.len() != 1;
    let tab_row = Row::new()
        .width(width - 36.0)
        .gap(8.0)
        .align(Align::Center)
        .child(tabs);
    let mut pane_content = Column::new().width(width - 36.0).gap(14.0);
    if show_tab_bar {
        pane_content = pane_content.child(tab_row);
    }
    pane_content = pane_content.child(
        Scroll::new()
            .size(
                width - 36.0,
                (height - if show_tab_bar { 92.0 } else { 54.0 }).max(100.0),
            )
            .child(widget_body(&active, width, compact)),
    );
    let mut pane_frame =
        Stack::new()
            .size(width, height)
            .child(framed(width, height, PANEL, pane_content));
    if !show_tab_bar {
        let (drag_tab, drag_name) = (active.id, format!("Move {} pane", active.title));
        let drag_start_runtime = runtime.clone();
        let drag_end_runtime = runtime.clone();
        pane_frame = pane_frame.child(
            Pad::all(6.0).child(
                DragHandle::new(drag_name)
                    .reveal_distance(22.0)
                    .on_drag_start(move || {
                        drag_start_runtime.update(|store| store.dragging = Some(drag_tab));
                    })
                    .on_drag_end(move |_| {
                        drag_end_runtime.update(|store| store.dragging = None);
                    }),
            ),
        );
    }
    let dock_pane_id = pane.id;
    let dock_runtime = runtime;
    DockArea::new(format!("Dock {}", active.title))
        .size(width, height)
        .on_dock(move |position| {
            if dock_dragged(&dock_runtime, dock_pane_id, position) {
                dock_runtime.request_remount();
            }
        })
        .child(pane_frame)
}

pub(crate) struct LayoutTreeView {
    runtime: WorkspaceRuntime,
    state: WorkspaceState,
    node: LayoutNode,
    width: f32,
    height: f32,
}

impl LayoutTreeView {
    pub(crate) fn new(
        runtime: WorkspaceRuntime,
        state: WorkspaceState,
        node: LayoutNode,
        width: f32,
        height: f32,
    ) -> Self {
        Self {
            runtime,
            state,
            node,
            width,
            height,
        }
    }
}

pub(crate) fn split_group_name(node: &LayoutNode) -> String {
    fn collect_panes(node: &LayoutNode, panes: &mut Vec<String>) {
        match node {
            LayoutNode::Pane(id) => panes.push(id.to_string()),
            LayoutNode::Split { first, second, .. } => {
                collect_panes(first, panes);
                collect_panes(second, panes);
            }
        }
    }

    let mut panes = Vec::new();
    collect_panes(node, &mut panes);
    format!("Dock between panes {}", panes.join(", "))
}

pub(crate) fn axis_span(node: &LayoutNode, axis: SplitAxis) -> usize {
    match node {
        LayoutNode::Split {
            axis: node_axis,
            first,
            second,
        } if *node_axis == axis => axis_span(first, axis) + axis_span(second, axis),
        _ => 1,
    }
}

pub(crate) fn weighted_split_extent(
    total: f32,
    gap: f32,
    first_span: usize,
    second_span: usize,
    minimum: f32,
) -> (f32, f32) {
    let total_span = first_span + second_span;
    let pane_extent =
        ((total - gap * (total_span.saturating_sub(1)) as f32) / total_span as f32).max(minimum);
    let subtree_extent =
        |span: usize| pane_extent * span as f32 + gap * span.saturating_sub(1) as f32;
    (subtree_extent(first_span), subtree_extent(second_span))
}

impl View for LayoutTreeView {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        match this.node {
            LayoutNode::Pane(pane_id) => {
                let pane = this
                    .state
                    .panes
                    .iter()
                    .find(|pane| pane.id == pane_id)
                    .cloned()
                    .expect("layout pane must exist");
                Box::new(pane_view(
                    this.runtime,
                    &this.state,
                    &pane,
                    this.width,
                    this.height,
                ))
                .build(ctx, parent)
            }
            LayoutNode::Split {
                axis,
                first,
                second,
            } => {
                let gap = 14.0;
                let split_node = LayoutNode::Split {
                    axis,
                    first: first.clone(),
                    second: second.clone(),
                };
                let split_name = split_group_name(&split_node);
                match axis {
                    SplitAxis::Horizontal => {
                        let (first_width, second_width) = weighted_split_extent(
                            this.width,
                            gap,
                            axis_span(&first, axis),
                            axis_span(&second, axis),
                            180.0,
                        );
                        let dock_target = split_node.clone();
                        let dock_runtime = this.runtime.clone();
                        Box::new(
                            DockArea::new(split_name)
                                .size(this.width, this.height)
                                .on_dock(move |position| {
                                    if dock_dragged_to_group(
                                        &dock_runtime,
                                        &dock_target,
                                        axis,
                                        position,
                                    ) {
                                        dock_runtime.request_remount();
                                    }
                                })
                                .child(
                                    Row::new()
                                        .size(this.width, this.height)
                                        .gap(gap)
                                        .child(LayoutTreeView::new(
                                            this.runtime.clone(),
                                            this.state.clone(),
                                            *first,
                                            first_width,
                                            this.height,
                                        ))
                                        .child(LayoutTreeView::new(
                                            this.runtime,
                                            this.state,
                                            *second,
                                            second_width,
                                            this.height,
                                        )),
                                ),
                        )
                        .build(ctx, parent)
                    }
                    SplitAxis::Vertical => {
                        let (first_height, second_height) = weighted_split_extent(
                            this.height,
                            gap,
                            axis_span(&first, axis),
                            axis_span(&second, axis),
                            150.0,
                        );
                        let dock_target = split_node;
                        let dock_runtime = this.runtime.clone();
                        Box::new(
                            DockArea::new(split_name)
                                .size(this.width, this.height)
                                .on_dock(move |position| {
                                    if dock_dragged_to_group(
                                        &dock_runtime,
                                        &dock_target,
                                        axis,
                                        position,
                                    ) {
                                        dock_runtime.request_remount();
                                    }
                                })
                                .child(
                                    Column::new()
                                        .size(this.width, this.height)
                                        .gap(gap)
                                        .child(LayoutTreeView::new(
                                            this.runtime.clone(),
                                            this.state.clone(),
                                            *first,
                                            this.width,
                                            first_height,
                                        ))
                                        .child(LayoutTreeView::new(
                                            this.runtime,
                                            this.state,
                                            *second,
                                            this.width,
                                            second_height,
                                        )),
                                ),
                        )
                        .build(ctx, parent)
                    }
                }
            }
        }
    }
}
