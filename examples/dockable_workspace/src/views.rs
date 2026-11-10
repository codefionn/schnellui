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

pub(crate) fn framed(
    width: f32,
    height: f32,
    fill: Color,
    padding: f32,
    content: impl View,
) -> impl View {
    Stack::new()
        .size(width, height)
        .child(Surface::new(width, height, fill))
        .child(Pad::all(padding).child(content))
}

/// Secondary text in the muted ink: captions, units and metadata.
pub(crate) fn caption(text: impl Into<std::borrow::Cow<'static, str>>, size: f32) -> impl View {
    ThemeProvider::new(MUTED_THEME, Text::new(text).size(size))
}

pub(crate) fn library_button(
    runtime: WorkspaceRuntime,
    label: &'static str,
    kind: WidgetType,
    width: f32,
) -> Button {
    Button::new(label)
        .appearance(ButtonAppearance::Outline)
        .width(width)
        .height(40.0)
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
    let inner_height = height - 36.0;
    // A short window can't fit the whole library, so it scrolls inside the
    // card. The viewport reaches 14 px into the right padding for the
    // scrollbar, which only shows when the content overflows.
    let library = Column::new()
        .width(inner_width)
        .min_height(inner_height)
        .gap(13.0)
        .child(caption("WIDGET LIBRARY", 12.0))
        .child(Text::new("Add a widget").size(22.0).bold())
        .child(
            Text::new("It opens as a tab in the first pane. Drag its tab to dock it anywhere.")
                .size(13.0)
                .wrap(WrapMode::Word),
        )
        .child(Divider::new())
        .child(library_button(
            runtime.clone(),
            "Add Pulse",
            WidgetType::Pulse,
            inner_width - 4.0,
        ))
        .child(library_button(
            runtime.clone(),
            "Add Focus timer",
            WidgetType::Focus,
            inner_width - 4.0,
        ))
        .child(library_button(
            runtime.clone(),
            "Add Notes",
            WidgetType::Notes,
            inner_width - 4.0,
        ))
        .child(library_button(
            runtime.clone(),
            "Add Weather",
            WidgetType::Weather,
            inner_width - 4.0,
        ))
        .child(Divider::new())
        .child(caption("LAYOUT", 12.0))
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
        .child(caption("Tip: drag any tab to move it.", 12.0));
    Stack::new()
        .size(width, height)
        .child(Surface::new(width, height, PANEL_ALT))
        .child(
            Pad::insets(EdgeInsets {
                top: 18.0,
                right: 4.0,
                bottom: 18.0,
                left: 18.0,
            })
            .child(
                Scroll::new()
                    .label("Widget library")
                    .scrollbar(true)
                    .size(inner_width + 14.0, inner_height)
                    .child(library),
            ),
        )
}

/// The heading and the one-line status shown at the top of a widget. The tab
/// already names the widget, so the heading says what the pane is showing.
fn widget_heading(widget: WidgetType) -> (&'static str, &'static str) {
    match widget {
        WidgetType::Pulse => ("Energy this week", "Updated 5 min ago"),
        WidgetType::Focus => ("Strategy sprint", "Notifications paused until 11:00"),
        WidgetType::Notes => ("Today's thread", "5 notes · edited 8 min ago"),
        WidgetType::Weather => ("Berlin", "Clear, soft wind"),
        WidgetType::Agent => ("Workspace agent", "Ready · no task running"),
        WidgetType::Terminal => ("Shell", "~/workspace · zsh"),
    }
}

/// One line of the focus timer's agenda: a muted time and what happens then.
fn agenda_row(time: &'static str, what: &'static str, width: f32) -> Row {
    Row::new()
        .width(width)
        .gap(14.0)
        .align(Align::Center)
        .child(caption(time, 14.0))
        .child(Text::new(what).size(14.0))
}

/// The body of one widget, laid out for a pane `width` wide whose scroll area
/// is `height` tall. Charts grow to use the height they are given.
pub(crate) fn widget_body(tab: &WorkspaceTab, width: f32, height: f32, compact: bool) -> Column {
    let gap = if compact { 8.0 } else { 14.0 };
    let narrow = width < 320.0;
    let (title, status) = widget_heading(tab.widget);
    let header = Column::new()
        .gap(2.0)
        .child(
            Text::new(title)
                .size(if compact { 16.0 } else { 19.0 })
                .bold(),
        )
        .child(caption(status, 12.0));
    let header_height = if compact { 40.0 } else { 46.0 };
    let chart_height =
        |above: f32| (height - header_height - above - gap * 3.0 - 8.0).clamp(80.0, 440.0);
    let figure = if compact { 28.0 } else { 34.0 };

    let body = match tab.widget {
        WidgetType::Pulse => Column::new()
            .gap(gap)
            .child(
                Row::new()
                    .gap(28.0)
                    .wrap()
                    .child(
                        Column::new()
                            .child(Text::new("72").size(figure))
                            .child(caption("Energy score", 12.0)),
                    )
                    .child(
                        Column::new()
                            .child(Text::new("6.4h").size(figure))
                            .child(caption("Deep work", 12.0)),
                    ),
            )
            .child(
                LineChart::new([28.0, 41.0, 37.0, 58.0, 52.0, 71.0, 66.0, 82.0])
                    .title("Energy score over the last eight days")
                    .axes(true)
                    .labels(["Tue", "Wed", "Thu", "Fri", "Sat", "Sun", "Mon", "Today"])
                    .tooltips(true)
                    .size(width, chart_height(figure + 16.0 + gap))
                    .color(ORANGE)
                    .stroke_width(3.0)
                    .markers(true),
            ),
        WidgetType::Focus => Column::new()
            .gap(gap)
            .child(Text::new("42:18").size(if compact { 38.0 } else { 52.0 }))
            .child(
                ProgressBar::new(68.0, 0.0, 100.0)
                    .name("Sprint progress")
                    .size(width, 8.0),
            )
            .child(caption("68% done · 18 minutes left", 13.0))
            .child(Divider::new())
            .child(caption("UP NEXT", 12.0))
            .child(agenda_row("11:00", "Design review with Mara", width))
            .child(agenda_row("13:30", "Lunch walk", width))
            .child(agenda_row("15:00", "Write the docking notes", width)),
        WidgetType::Notes => {
            let size = if narrow || compact { 14.0 } else { 16.0 };
            Column::new()
                .gap(gap)
                .child(
                    Text::new("→  Prototype the docking flow")
                        .size(size)
                        .wrap(WrapMode::Word),
                )
                .child(
                    Text::new("→  Keep the canvas calm")
                        .size(size)
                        .wrap(WrapMode::Word),
                )
                .child(
                    Text::new("→  Send the Friday field note")
                        .size(size)
                        .wrap(WrapMode::Word),
                )
                .child(
                    Text::new("→  Book the train for Thursday")
                        .size(size)
                        .wrap(WrapMode::Word),
                )
                .child(Divider::new())
                .child(caption("Pinned", 12.0))
                .child(
                    Text::new(
                        "Docking should feel like moving paper on a desk: no surprises, no modes.",
                    )
                    .size(size)
                    .wrap(WrapMode::Word),
                )
        }
        WidgetType::Weather => Column::new()
            .gap(gap)
            .child(
                Row::new()
                    .gap(18.0)
                    .align(Align::Center)
                    .child(Text::new("18°").size(if compact { 40.0 } else { 52.0 }))
                    .child(caption("High 18° · Low 12°", 13.0)),
            )
            .child(
                BarChart::new([12.0, 14.0, 17.0, 18.0, 16.0, 13.0])
                    .title("Temperature through the day")
                    .axes(true)
                    .labels(["6h", "9h", "12h", "15h", "18h", "21h"])
                    .unit("°C")
                    .tooltips(true)
                    .size(width, chart_height(if compact { 48.0 } else { 62.0 }))
                    .color(CYAN)
                    .baseline(false),
            ),
        WidgetType::Agent => Column::new()
            .gap(gap)
            .child(
                Text::new("What should we build next?")
                    .size(if narrow || compact { 20.0 } else { 26.0 })
                    .wrap(WrapMode::Word),
            )
            .child(caption(
                "Describe a change, ask for a codebase tour, or hand off a failing test.",
                13.0,
            )),
        WidgetType::Terminal => Column::new()
            .gap(if compact { 4.0 } else { 8.0 })
            .child(Text::new("$ cargo test --workspace").size(14.0))
            .child(caption("   Finished test profile", 13.0))
            .child(caption("   195 passed · 0 failed", 13.0))
            .child(Text::new("$ _").size(14.0)),
    };

    Column::new()
        .width(width)
        .gap(gap)
        .child(header)
        .child(Divider::new())
        .child(body)
}

/// The pane's "+" button.
const ADD_BUTTON_W: f32 = 36.0;
/// The "more tabs" button: a down triangle and a count.
const MORE_BUTTON_W: f32 = 44.0;

/// A tab's width from its label: about 7.5px a character at the tab text size,
/// plus the tab's padding and the bar's gap. An estimate on the safe side.
fn estimated_tabs_width(state: &WorkspaceState, tabs: &[u64]) -> f32 {
    tabs.iter()
        .map(|id| tab_by_id(state, *id).title.chars().count() as f32 * 7.5 + 28.0)
        .sum()
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
    // A pane too narrow for every tab shows only its active tab, and the rest
    // move into a "more" menu, so tabs never spill over the next pane, where a
    // click would land on the wrong one.
    let padding = if state.compact { 10.0 } else { 18.0 };
    let room = width - padding * 2.0 - ADD_BUTTON_W - 8.0;
    let all_fit = estimated_tabs_width(state, &pane.tabs) <= room;
    let shown: Vec<_> = if all_fit {
        pane.tabs.clone()
    } else if estimated_tabs_width(state, &[pane.active]) <= room - MORE_BUTTON_W {
        vec![pane.active]
    } else {
        Vec::new()
    };
    for tab_id in &shown {
        let tab = tab_by_id(state, *tab_id);
        let (id, pane_id, before_id) = (tab.id, pane.id, tab.id);
        let label = tab.title.clone();
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
    let active = tab_by_id(state, pane.active);
    let mut add_menu = ContextMenu::new();
    for widget in [
        WidgetType::Pulse,
        WidgetType::Focus,
        WidgetType::Notes,
        WidgetType::Weather,
        WidgetType::Agent,
        WidgetType::Terminal,
    ] {
        let (add_runtime, pane_id) = (runtime.clone(), pane.id);
        add_menu = add_menu.item(ContextMenuItem::new(widget.title()).on_select(move || {
            add_widget_to_pane(&add_runtime, widget, Some(pane_id));
        }));
    }
    let hidden: Vec<_> = pane
        .tabs
        .iter()
        .copied()
        .filter(|id| !shown.contains(id))
        .collect();
    let mut actions = Row::new().gap(2.0).align(Align::Center);
    if !hidden.is_empty() {
        let mut more_menu = ContextMenu::new();
        for tab_id in &hidden {
            let tab = tab_by_id(state, *tab_id);
            let (id, pane_id, select_runtime) = (tab.id, pane.id, runtime.clone());
            more_menu =
                more_menu.item(ContextMenuItem::new(tab.title.clone()).on_select(move || {
                    select_runtime.update(|store| {
                        if let Some(pane) = store.panes.iter_mut().find(|pane| pane.id == pane_id) {
                            pane.active = id;
                        }
                    });
                    select_runtime.request_remount();
                }));
        }
        actions = actions.child(
            Button::new(format!(
                "{} more tabs in the {} pane",
                hidden.len(),
                active.title
            ))
            .appearance(ButtonAppearance::Ghost)
            .display(format!("\u{25be}{}", hidden.len()))
            .width(MORE_BUTTON_W)
            .height(36.0)
            .tooltip("Show the other tabs")
            .menu(more_menu),
        );
    }
    actions = actions.child(
        Button::new(format!("Add a widget to the {} pane", active.title))
            .appearance(ButtonAppearance::Ghost)
            .display("+")
            .text_size(20.0)
            .width(ADD_BUTTON_W)
            .height(36.0)
            .tooltip("Add a widget to this pane")
            .menu(add_menu),
    );
    tabs = tabs.trailing(actions);

    let compact = state.compact;
    let show_tab_bar = !state.hide_single_tab_bars || pane.tabs.len() != 1;
    let inner_width = width - padding * 2.0;
    let tab_row = Row::new()
        .width(inner_width)
        .gap(8.0)
        .align(Align::Center)
        .child(tabs);
    let gap = if compact { 6.0 } else { 14.0 };
    let mut pane_content = Column::new().width(inner_width).gap(gap);
    if show_tab_bar {
        pane_content = pane_content.child(tab_row);
    }
    let scroll_height =
        (height - padding * 2.0 - if show_tab_bar { 40.0 + gap } else { 0.0 }).max(100.0);
    pane_content = pane_content.child(
        Scroll::new()
            .label(format!("{} pane", active.title))
            .scrollbar(true)
            .size(inner_width, scroll_height)
            .child(widget_body(
                &active,
                inner_width - 14.0,
                scroll_height,
                compact,
            )),
    );
    let mut pane_frame =
        Stack::new()
            .size(width, height)
            .child(framed(width, height, PANEL, padding, pane_content));
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
