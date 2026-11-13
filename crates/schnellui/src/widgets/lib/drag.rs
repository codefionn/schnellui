use super::*;

const DRAG_THRESHOLD: f32 = 5.0;
const DROP_RING_WIDTH: f32 = 3.0;
const DROP_RING_PRIMS: usize = 4;
const DOCK_PREVIEW_PRIMS: usize = 5;
const DOCK_EDGE_FRACTION: f32 = 0.25;
/// Alpha of the dock preview's wash over the half or whole pane it marks.
const DOCK_WASH_ALPHA: u8 = 40;
/// Share of their alpha, out of 256, the glyphs of a dragged source keep.
const VEIL_KEEP: u16 = 100;

fn handler_ancestor(
    runtime: &Runtime,
    scene: &Scene,
    from: WidgetId,
    predicate: impl Fn(&Handlers) -> bool,
) -> Option<WidgetId> {
    let mut current = Some(from);
    while let Some(id) = current {
        if runtime.with(|rt| rt.borrow().handlers.get(id).is_some_and(&predicate)) {
            return Some(id);
        }
        current = scene.node(id).and_then(|node| node.parent);
    }
    None
}

fn tab_reorder_item_ancestor(
    runtime: &Runtime,
    scene: &Scene,
    from: WidgetId,
) -> Option<(WidgetId, TabReorderItem)> {
    let mut current = Some(from);
    while let Some(id) = current {
        if let Some(item) = runtime.with(|rt| rt.borrow().tab_reorder_items.get(id).copied()) {
            return Some((id, item));
        }
        current = scene.node(id).and_then(|node| node.parent);
    }
    None
}

/// A rich text link that can be activated. A press on one is captured like a
/// possible drag so the click lands on **release**, over the link, and a press
/// dragged off the link ends with nothing (see [`end_drag`]).
fn is_active_rich_link(rt: &WidgetRuntime, id: WidgetId) -> bool {
    rt.rich_links.contains_key(id)
        && rt
            .handlers
            .get(id)
            .is_some_and(|handlers| handlers.click.is_some())
}

fn drag_source_ancestor(runtime: &Runtime, scene: &Scene, from: WidgetId) -> Option<WidgetId> {
    let mut current = Some(from);
    while let Some(id) = current {
        let draggable = runtime.with(|rt| {
            let rt = rt.borrow();
            rt.handlers
                .get(id)
                .is_some_and(|handlers| handlers.drag_start.is_some())
                || rt.tab_reorder_items.contains_key(id)
                || is_active_rich_link(&rt, id)
        });
        if draggable {
            return Some(id);
        }
        current = scene.node(id).and_then(|node| node.parent);
    }
    None
}

pub fn register_tab_reorder(
    runtime: &Runtime,
    bar: WidgetId,
    tabs: SmallVec<[WidgetId; 8]>,
    callback: impl FnMut(usize, usize) + 'static,
) {
    runtime.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        runtime.tab_reorders.insert(bar, Some(Box::new(callback)));
        for (index, tab) in tabs.into_iter().enumerate() {
            runtime
                .tab_reorder_items
                .insert(tab, TabReorderItem { bar, index });
        }
    });
}

/// Runs `paint` on drop-preview paint with hover, press and focus lifted off,
/// then lays them again on top. Those decorations find their strokes by
/// counting from the end of the paint, so preview lines must never land
/// after them, and taking the preview off must not cut into them.
fn under_decorations(runtime: &Runtime, scene: &mut Scene, paint: impl FnOnce(&mut Scene)) {
    let (ring, active, hover) = runtime.with(|rt| {
        let rt = rt.borrow();
        (
            rt.ring.as_ref().map(|d| d.target),
            rt.active.as_ref().map(|d| d.target),
            rt.hover.as_ref().map(|d| d.target),
        )
    });
    if let Some(id) = ring {
        remove_focus_ring(runtime, scene, id);
    }
    strip_active_decoration(runtime, scene);
    strip_hover_decoration(runtime, scene);
    paint(scene);
    if let Some(id) = hover {
        apply_hover_decoration(runtime, scene, id);
    }
    if let Some(id) = active {
        apply_active_decoration(runtime, scene, id);
    }
    if let Some(id) = ring {
        apply_focus_ring(runtime, scene, id);
    }
}

fn strip_drop_preview(runtime: &Runtime, scene: &mut Scene, hover: DropHover) {
    show_drop_tip(runtime, scene, hover.target, false);
    show_zone_label(runtime, scene, hover, false);
    under_decorations(runtime, scene, |scene| strip_preview_lines(scene, hover));
}

/// Shows or hides the chip that names the hovered zone of a dock area, if
/// it has one. A drop on a pane's tab bar joins it whatever the zone says,
/// so none shows there.
fn show_zone_label(runtime: &Runtime, scene: &mut Scene, hover: DropHover, visible: bool) {
    if hover.preview_prims != DOCK_PREVIEW_PRIMS || (visible && hover.over_bar) {
        return;
    }
    let label = runtime.with(|rt| {
        rt.borrow()
            .handlers
            .get(hover.target)
            .and_then(|handlers| handlers.dock_labels[hover.position.index()])
    });
    if let Some(label) = label.filter(|&label| scene.node(label).is_some()) {
        scene.set_visible(label, visible);
        scene.mark_dirty(label, DirtyFlags::PAINT);
    }
}

fn strip_preview_lines(scene: &mut Scene, hover: DropHover) {
    let Some(pd) = scene.paint(hover.preview) else {
        return;
    };
    let len = pd.primitives.len();
    if len < hover.preview_prims {
        return;
    }
    let is_preview = pd.primitives[len - DROP_RING_PRIMS..].iter().all(
        |primitive| matches!(primitive, Primitive::Line { width, .. } if *width == DROP_RING_WIDTH),
    );
    if is_preview {
        scene
            .paint_mut(hover.preview)
            .primitives
            .truncate(len - hover.preview_prims);
        scene.mark_dirty(hover.preview, DirtyFlags::PAINT);
    }
}

pub fn dock_position(rect: Rect, point: Point) -> DockPosition {
    let x = ((point.x - rect.x) / rect.width.max(1.0)).clamp(0.0, 1.0);
    let y = ((point.y - rect.y) / rect.height.max(1.0)).clamp(0.0, 1.0);
    let horizontal_edge = x.min(1.0 - x);
    let vertical_edge = y.min(1.0 - y);
    if horizontal_edge >= DOCK_EDGE_FRACTION && vertical_edge >= DOCK_EDGE_FRACTION {
        DockPosition::Center
    } else if horizontal_edge < vertical_edge {
        if x < 0.5 {
            DockPosition::Left
        } else {
            DockPosition::Right
        }
    } else if y < 0.5 {
        DockPosition::Top
    } else {
        DockPosition::Bottom
    }
}

/// The band a pane's tab bar covers: the bar's rows across the full width of
/// the dock `rect`. Dropping anywhere on it joins the pane, over a label or
/// on the empty strip beside the labels, so the split zones are the pane's
/// content edges below it. `None` when the pane shows no bar. A scroll's
/// content is not searched: its bars belong to nested content.
fn tab_bar_band(scene: &Scene, root: WidgetId, rect: Rect) -> Option<Rect> {
    fn find(scene: &Scene, id: WidgetId) -> Option<WidgetId> {
        let node = scene.node(id)?;
        if node.kind == WidgetKind::TabBar {
            return Some(id);
        }
        if node.kind == WidgetKind::Scroll {
            return None;
        }
        node.children.iter().find_map(|child| find(scene, *child))
    }
    let bar = find(scene, root)?;
    let mut band = scene.layout(bar)?.rect;
    // A bar with a trailing action sits in a wrapper row as tall as the action.
    if let Some(parent) = scene.node(bar).and_then(|node| node.parent) {
        let wrapper = scene.node(parent).is_some_and(|node| {
            node.kind == WidgetKind::Row
                && scene.a11y(parent).map(|a| a.role) == Some(Role::Group.as_u16())
        });
        if wrapper && parent != root {
            if let Some(layout) = scene.layout(parent) {
                band = band.union(&layout.rect);
            }
        }
    }
    (!band.is_empty()).then(|| Rect::new(rect.x, band.y, rect.width, band.height))
}

fn is_drop_target(runtime: &Runtime, scene: &Scene, hit: WidgetId) -> bool {
    handler_ancestor(runtime, scene, hit, |handlers| {
        handlers.drop.is_some() || handlers.dock.is_some()
    })
    .is_some()
}

/// The tab of `bar` at `index`, if the strip has one.
fn tab_at(runtime: &Runtime, bar: WidgetId, index: usize) -> Option<WidgetId> {
    runtime.with(|rt| {
        rt.borrow()
            .tab_reorder_items
            .iter()
            .find(|(_, item)| item.bar == bar && item.index == index)
            .map(|(id, _)| id)
    })
}

/// The tab of `bar` in whose row `point` lies: the one under it, or the last
/// tab when the point is past the end of the row.
fn tab_under_point(
    runtime: &Runtime,
    scene: &Scene,
    bar: WidgetId,
    point: Point,
) -> Option<(WidgetId, TabReorderItem)> {
    runtime.with(|rt| {
        let rt = rt.borrow();
        let mut last: Option<(WidgetId, TabReorderItem)> = None;
        for (id, item) in rt.tab_reorder_items.iter() {
            if item.bar != bar {
                continue;
            }
            let Some(rect) = scene.layout(id).map(|layout| layout.rect) else {
                continue;
            };
            if point.y < rect.y || point.y > rect.bottom() {
                continue;
            }
            if point.x >= rect.x && point.x <= rect.right() {
                return Some((id, *item));
            }
            if point.x > rect.right() && last.is_none_or(|(_, l)| item.index > l.index) {
                last = Some((id, *item));
            }
        }
        last
    })
}

pub(super) fn dock_preview_rect(rect: Rect, position: DockPosition) -> Rect {
    match position {
        // The box the middle zone covers, so where it shows is where a
        // drop swaps.
        DockPosition::Center => Rect::new(
            rect.x + rect.width * DOCK_EDGE_FRACTION,
            rect.y + rect.height * DOCK_EDGE_FRACTION,
            rect.width * (1.0 - 2.0 * DOCK_EDGE_FRACTION),
            rect.height * (1.0 - 2.0 * DOCK_EDGE_FRACTION),
        ),
        DockPosition::Left => Rect::new(rect.x, rect.y, rect.width * 0.5, rect.height),
        DockPosition::Right => Rect::new(
            rect.x + rect.width * 0.5,
            rect.y,
            rect.width * 0.5,
            rect.height,
        ),
        DockPosition::Top => Rect::new(rect.x, rect.y, rect.width, rect.height * 0.5),
        DockPosition::Bottom => Rect::new(
            rect.x,
            rect.y + rect.height * 0.5,
            rect.width,
            rect.height * 0.5,
        ),
    }
}

fn resolve_drop_hover(
    runtime: &Runtime,
    scene: &Scene,
    point: Point,
    source: WidgetId,
) -> Option<DropHover> {
    let hit = hit_test(runtime, scene, point)?;
    let source_item = tab_reorder_item_ancestor(runtime, scene, source);
    let target_item = match (source_item, tab_reorder_item_ancestor(runtime, scene, hit)) {
        (_, Some(found)) => Some(found),
        // Over a tab's close button, which sits on the tab without being
        // part of it, or on the empty strip after the last tab.
        (Some((_, item)), None) if !is_drop_target(runtime, scene, hit) => {
            tab_under_point(runtime, scene, item.bar, point)
        }
        _ => None,
    };
    if let (Some((_, source_item)), Some((target, target_item))) = (source_item, target_item) {
        if source_item.bar == target_item.bar {
            let after_target = scene
                .layout(target)
                .is_some_and(|layout| point.x >= layout.rect.x + layout.rect.width * 0.5);
            let insertion = target_item.index + usize::from(after_target);
            let to = if insertion > source_item.index {
                insertion - 1
            } else {
                insertion
            };
            // A spot that leaves the order as it is marks nothing.
            if to == source_item.index {
                return None;
            }
            // The marker goes on the near edge of the tab that will follow
            // the dragged one, so after one tab and before the next mark
            // the same spot. Only past the last tab is it on a right edge.
            let next = after_target
                .then(|| tab_at(runtime, target_item.bar, target_item.index + 1))
                .flatten();
            let (preview, after) = match next {
                Some(next) => (next, false),
                None => (target, after_target),
            };
            return Some(DropHover {
                target,
                preview,
                position: DockPosition::Center,
                over_bar: false,
                reorder: Some(TabReorderHover {
                    bar: source_item.bar,
                    from: source_item.index,
                    to,
                    after,
                }),
                preview_prims: DROP_RING_PRIMS,
            });
        }
    }
    let target = handler_ancestor(runtime, scene, hit, |handlers| {
        handlers.drop.is_some() || handlers.dock.is_some()
    })?;
    let is_dock = runtime.with(|rt| {
        rt.borrow()
            .handlers
            .get(target)
            .is_some_and(|handlers| handlers.dock.is_some())
    });
    let mut over_bar = false;
    let position = if is_dock {
        scene
            .layout(target)
            .map(|layout| {
                over_bar = tab_bar_band(scene, target, layout.rect)
                    .is_some_and(|band| band.contains(point));
                if over_bar {
                    DockPosition::Center
                } else {
                    dock_position(layout.rect, point)
                }
            })
            .unwrap_or(DockPosition::Center)
    } else {
        DockPosition::Center
    };
    let accepted = runtime.with(|rt| {
        rt.borrow()
            .handlers
            .get(target)
            .and_then(|handlers| handlers.dock_accepts.as_ref())
            .is_none_or(|accepts| accepts(position))
    });
    if !accepted {
        return None;
    }
    Some(DropHover {
        target,
        preview: if is_dock {
            runtime.with(|rt| {
                rt.borrow()
                    .handlers
                    .get(target)
                    .and_then(|handlers| handlers.dock_preview)
                    .unwrap_or(target)
            })
        } else {
            target
        },
        position,
        over_bar,
        reorder: None,
        preview_prims: if is_dock {
            DOCK_PREVIEW_PRIMS
        } else {
            DROP_RING_PRIMS
        },
    })
}

fn apply_drop_preview(runtime: &Runtime, scene: &mut Scene, hover: DropHover) {
    under_decorations(runtime, scene, |scene| push_preview(runtime, scene, hover));
    show_zone_label(runtime, scene, hover, true);
    if hover.reorder.is_none() {
        show_drop_tip(runtime, scene, hover.target, true);
    }
}

/// Shows or hides the [`DropTip`] over a drop target: the target itself or
/// one of its nearest ancestors.
fn show_drop_tip(runtime: &Runtime, scene: &mut Scene, target: WidgetId, visible: bool) {
    let mut current = Some(target);
    for _ in 0..4 {
        let Some(id) = current else {
            return;
        };
        let changed = runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            if !rt.drop_tips.contains_key(id) {
                return None;
            }
            let tip = rt.hover_tooltips.get_mut(id)?;
            Some(super::interaction::show_tooltip(scene, id, tip, visible))
        });
        if let Some(changed) = changed {
            if changed {
                scene.mark_dirty(id, DirtyFlags::PAINT);
            }
            return;
        }
        current = scene.node(id).and_then(|node| node.parent);
    }
}

fn push_preview(runtime: &Runtime, scene: &mut Scene, hover: DropHover) {
    let rect = if hover.preview_prims == DOCK_PREVIEW_PRIMS {
        let Some(layout) = scene.layout(hover.target) else {
            return;
        };
        match hover
            .over_bar
            .then(|| tab_bar_band(scene, hover.target, layout.rect))
            .flatten()
        {
            Some(band) => band,
            None => dock_preview_rect(layout.rect, hover.position),
        }
    } else {
        let Some(rect) = paint_bounds(runtime, scene, hover.preview) else {
            return;
        };
        rect
    };
    if hover.preview_prims == DOCK_PREVIEW_PRIMS {
        scene
            .paint_mut(hover.preview)
            .primitives
            .push(Primitive::SolidRect {
                rect,
                // A light wash: the pane's content stays readable under it,
                // and the outline marks the edge.
                // Weighted for the linear-light blend, so a light scheme's
                // wash reads as strongly as a dark one's.
                color: {
                    let theme = theme_for(runtime, hover.preview);
                    let accent = theme.accent;
                    crate::widgets::theme::wash_over(
                        Color::rgba(accent.r, accent.g, accent.b, DOCK_WASH_ALPHA),
                        theme.surface,
                    )
                },
                corner_radius: 5.0,
            });
    }
    if rect.is_empty() {
        return;
    }
    // A reorder marks where the tab lands: a bar on the hovered tab's near
    // edge instead of a ring around it, which reads as focus.
    if let Some(reorder) = hover.reorder {
        // Inside the tab, so nothing clips the bar's 5 px.
        let x = if reorder.after {
            rect.right() - 2.5
        } else {
            rect.x + 2.5
        };
        let color = theme_for(runtime, hover.preview).accent;
        let pd = scene.paint_mut(hover.preview);
        // Four lines, as many as a ring, so leaving removes them alike.
        for dx in [-1.0, -0.5, 0.5, 1.0] {
            pd.primitives.push(Primitive::Line {
                from: Point {
                    x: x + dx,
                    y: rect.y + 2.0,
                },
                to: Point {
                    x: x + dx,
                    y: rect.bottom() - 2.0,
                },
                width: DROP_RING_WIDTH,
                color,
            });
        }
        scene.mark_dirty(hover.preview, DirtyFlags::PAINT);
        return;
    }
    let half = DROP_RING_WIDTH * 0.5;
    let x0 = rect.x + half;
    let y0 = rect.y + half;
    let x1 = rect.right() - half;
    let y1 = rect.bottom() - half;
    // The accent, as focus and selection use: a drop target is a place to
    // go, not a warning.
    let color = theme_for(runtime, hover.preview).accent;
    let pd = scene.paint_mut(hover.preview);
    for (from, to) in [
        (Point { x: x0, y: y0 }, Point { x: x1, y: y0 }),
        (Point { x: x0, y: y1 }, Point { x: x1, y: y1 }),
        (Point { x: x0, y: y0 }, Point { x: x0, y: y1 }),
        (Point { x: x1, y: y0 }, Point { x: x1, y: y1 }),
    ] {
        pd.primitives.push(Primitive::Line {
            from,
            to,
            width: DROP_RING_WIDTH,
            color,
        });
    }
    scene.mark_dirty(hover.preview, DirtyFlags::PAINT);
}

/// Fades the labels and icons of what a drag that just started moving
/// carries, so it reads as picked up: a pane's title strip, a tab.
/// Glyphs draw above every fill, so a veil laid over them would not show;
/// their own colors change instead. Splitters and links keep theirs.
fn lay_veil(runtime: &Runtime, scene: &mut Scene, source: WidgetId) {
    if splitter_offset(runtime, source, Point { x: 0.0, y: 0.0 }).is_some()
        || runtime.with(|rt| is_active_rich_link(&rt.borrow(), source))
    {
        return;
    }
    let mut saved = Vec::new();
    let mut nodes = scene.subtree_nodes(source);
    // A tab's close button sits beside it in their shared row; it fades
    // with the tab, so the whole tab reads as lifted.
    let row = scene
        .node(source)
        .filter(|node| node.kind == WidgetKind::Tab)
        .and_then(|node| node.parent);
    if let Some(row) = row {
        let closes: Vec<WidgetId> = scene
            .node(row)
            .map(|node| node.children.iter().copied().collect())
            .unwrap_or_default();
        nodes.extend(closes.into_iter().filter(|id| {
            scene
                .node(*id)
                .is_some_and(|node| node.kind == WidgetKind::Button)
        }));
    }
    for id in nodes {
        let Some(paint) = scene.paint(id) else {
            continue;
        };
        if !paint.primitives.iter().any(|primitive| {
            matches!(
                primitive,
                Primitive::GlyphQuad { .. } | Primitive::Line { .. }
            )
        }) {
            continue;
        }
        for (index, primitive) in scene.paint_mut(id).primitives.iter_mut().enumerate() {
            if let Primitive::GlyphQuad { color, .. } | Primitive::Line { color, .. } = primitive {
                saved.push((id, index, *color));
                color.a = (u16::from(color.a) * VEIL_KEEP / 256) as u8;
            }
        }
        scene.mark_dirty(id, DirtyFlags::PAINT);
    }
    runtime.with(|rt| rt.borrow_mut().drag_veil = saved);
}

/// Clears what a drag leaves on the parts of the tree that stay, when its
/// source or target is about to go with a replaced branch: the drop preview
/// on a target that stays, and the faded glyphs.
pub(super) fn abandon_drag(
    runtime: &Runtime,
    scene: &mut Scene,
    removed: impl Fn(WidgetId) -> bool,
) {
    let Some(capture) = runtime.with(|rt| rt.borrow().drag_pointer) else {
        return;
    };
    let ends = removed(capture.source)
        || capture
            .hovered
            .is_some_and(|hover| removed(hover.target) || removed(hover.preview));
    if !ends {
        return;
    }
    if let Some(hover) = capture
        .hovered
        .filter(|hover| !removed(hover.target) && !removed(hover.preview))
    {
        strip_drop_preview(runtime, scene, hover);
    }
    lift_veil(runtime, scene);
}

/// Gives back the colors [`lay_veil`] faded, on the nodes still there.
fn lift_veil(runtime: &Runtime, scene: &mut Scene) {
    let saved = runtime.with(|rt| std::mem::take(&mut rt.borrow_mut().drag_veil));
    for (id, index, original) in saved {
        if scene.node(id).is_none() {
            continue;
        }
        if let Some(Primitive::GlyphQuad { color, .. } | Primitive::Line { color, .. }) =
            scene.paint_mut(id).primitives.get_mut(index)
        {
            *color = original;
            scene.mark_dirty(id, DirtyFlags::PAINT);
        }
    }
}

/// Captures a press that starts on a widget configured with
/// `on_drag_start`. The drag does not become active until pointer movement
/// crosses a small threshold, preserving normal click behavior.
pub fn begin_drag(runtime: &Runtime, scene: &Scene, point: Point) -> bool {
    let Some(hit) = hit_test(runtime, scene, point) else {
        return false;
    };
    let Some(source) = drag_source_ancestor(runtime, scene, hit) else {
        return false;
    };
    let disabled = scene
        .a11y(source)
        .map(|a| StateFlags(a.state).contains(StateFlags::DISABLED))
        .unwrap_or(false);
    if disabled {
        return false;
    }
    runtime.with(|rt| {
        rt.borrow_mut().drag_pointer = Some(DragPointerCapture {
            source,
            origin: point,
            last: point,
            active: false,
            hovered: None,
        });
    });
    true
}

/// The source of a content drag in progress, pressed or already moving.
pub(crate) fn content_drag_source(runtime: &Runtime) -> Option<WidgetId> {
    runtime.with(|rt| rt.borrow().drag_pointer.map(|capture| capture.source))
}

/// Hands a content drag in progress to `source`, its source's counterpart in
/// a rebuilt tree. The old tree's drop preview went with it, so the drag
/// starts over no target. Returns where the pointer last was when the drag
/// had started moving, for [`update_drag`] to find the target under it once
/// the new tree is laid out.
pub(crate) fn transfer_content_drag(
    replacement: &Runtime,
    source: WidgetId,
    previous: &Runtime,
) -> Option<Point> {
    let capture = previous.with(|rt| rt.borrow().drag_pointer)?;
    replacement.with(|rt| {
        rt.borrow_mut().drag_pointer = Some(DragPointerCapture {
            source,
            hovered: None,
            ..capture
        });
    });
    capture.active.then_some(capture.last)
}

/// Returns the widget runtime's current pointer-capture state for diagnostics.
///
/// The snapshot is observational: it does not dispatch, mutate, or retain a
/// borrow of the runtime.
pub fn interaction_debug_state(runtime: &Runtime) -> InteractionDebugState {
    runtime.with(|rt| {
        let rt = rt.borrow();
        InteractionDebugState {
            content_drag_source: rt.drag_pointer.map(|capture| capture.source),
            content_drag_active: rt.drag_pointer.is_some_and(|capture| capture.active),
            dialog_pointer_capture: rt.dialog_pointer.is_some(),
        }
    })
}

/// Advances the captured content drag and updates the highlighted drop-preview
/// target. Returns `true` when the screen or cursor needs refreshing.
pub fn update_drag(runtime: &Runtime, scene: &mut Scene, point: Point) -> bool {
    let Some(mut capture) = runtime.with(|rt| rt.borrow().drag_pointer) else {
        return false;
    };
    let mut changed = false;
    capture.last = point;
    runtime.with(|rt| rt.borrow_mut().drag_pointer = Some(capture));
    if !capture.active {
        let dx = point.x - capture.origin.x;
        let dy = point.y - capture.origin.y;
        if dx * dx + dy * dy < DRAG_THRESHOLD * DRAG_THRESHOLD {
            return false;
        }
        capture.active = true;
        runtime.with(|rt| rt.borrow_mut().drag_pointer = Some(capture));
        let callback = runtime.with(|rt| {
            rt.borrow_mut()
                .handlers
                .get_mut(capture.source)
                .and_then(|handlers| handlers.drag_start.take())
        });
        if let Some(mut callback) = callback {
            callback();
            runtime.with(|rt| {
                if let Some(handlers) = rt.borrow_mut().handlers.get_mut(capture.source) {
                    handlers.drag_start = Some(callback);
                }
            });
        }
        // Hover lifts first: taking it off later would paint the source's
        // labels back in their resting colors over the veil.
        super::interaction::update_control_hover(runtime, scene, None);
        lay_veil(runtime, scene, capture.source);
        changed = true;
    }

    // A splitter's drag moves its guide; it has no drop target.
    let delta = Point {
        x: point.x - capture.origin.x,
        y: point.y - capture.origin.y,
    };
    if let Some(offset) = splitter_offset(runtime, capture.source, delta) {
        emit_splitter(runtime, scene, capture.source, Some(offset));
        return true;
    }

    let hovered = resolve_drop_hover(runtime, scene, point, capture.source)
        .filter(|hover| hover.target != capture.source);
    if hovered != capture.hovered {
        if let Some(previous) = capture.hovered {
            strip_drop_preview(runtime, scene, previous);
        }
        if let Some(target) = hovered {
            apply_drop_preview(runtime, scene, target);
        }
        capture.hovered = hovered;
        runtime.with(|rt| rt.borrow_mut().drag_pointer = Some(capture));
        changed = true;
    }
    changed
}

/// Cancels a content drag that has started moving: the drop preview goes,
/// the source's `on_drag_end(false)` runs, and the release that follows does
/// nothing. A press that has not crossed the drag threshold is left alone,
/// so it can still become a click. Returns whether a drag was cancelled.
pub fn cancel_drag(runtime: &Runtime, scene: &mut Scene) -> bool {
    let Some(capture) = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.drag_pointer
            .filter(|capture| capture.active)
            .and_then(|_| rt.drag_pointer.take())
    }) else {
        return false;
    };
    lift_veil(runtime, scene);
    if let Some(hover) = capture.hovered {
        strip_drop_preview(runtime, scene, hover);
    }
    if splitter_offset(runtime, capture.source, Point { x: 0.0, y: 0.0 }).is_some() {
        emit_splitter(runtime, scene, capture.source, None);
    }
    let callback = runtime.with(|rt| {
        rt.borrow_mut()
            .handlers
            .get_mut(capture.source)
            .and_then(|handlers| handlers.drag_end.take())
    });
    if let Some(mut callback) = callback {
        callback(false);
        runtime.with(|rt| {
            if let Some(handlers) = rt.borrow_mut().handlers.get_mut(capture.source) {
                handlers.drag_end = Some(callback);
            }
        });
    }
    true
}

/// Releases a possible/active content drag. A sub-threshold gesture becomes a
/// click; a real drag fires the hovered target's drop handler followed by the
/// source's `on_drag_end(accepted)` callback.
pub fn end_drag(runtime: &Runtime, scene: &mut Scene, point: Point) -> DragRelease {
    let Some(capture) = runtime.with(|rt| rt.borrow_mut().drag_pointer.take()) else {
        return DragRelease::None;
    };
    // A rich text link is clicked by pressing and releasing on it. Released
    // anywhere else, the press ends with no click, however far it moved.
    if runtime.with(|rt| is_active_rich_link(&rt.borrow(), capture.source))
        && hit_test(runtime, scene, point) != Some(capture.source)
    {
        if let Some(hover) = capture.hovered {
            strip_drop_preview(runtime, scene, hover);
        }
        return DragRelease::None;
    }
    if !capture.active {
        return DragRelease::Click(capture.source);
    }
    let delta = Point {
        x: point.x - capture.origin.x,
        y: point.y - capture.origin.y,
    };
    if let Some(offset) = splitter_offset(runtime, capture.source, delta) {
        release_splitter(runtime, scene, capture.source, offset);
        return DragRelease::Drop { accepted: true };
    }
    lift_veil(runtime, scene);
    if let Some(hover) = capture.hovered {
        strip_drop_preview(runtime, scene, hover);
    }
    let accepted = capture.hovered.is_some();
    if let Some(hover) = capture.hovered {
        let reorder_callback = hover.reorder.and_then(|reorder| {
            runtime.with(|rt| {
                rt.borrow_mut()
                    .tab_reorders
                    .get_mut(reorder.bar)
                    .and_then(Option::take)
                    .map(|callback| (reorder, callback))
            })
        });
        if let Some((reorder, mut callback)) = reorder_callback {
            callback(reorder.from, reorder.to);
            runtime.with(|rt| {
                if let Some(slot) = rt.borrow_mut().tab_reorders.get_mut(reorder.bar) {
                    *slot = Some(callback);
                }
            });
        } else {
            let dock_callback = runtime.with(|rt| {
                rt.borrow_mut()
                    .handlers
                    .get_mut(hover.target)
                    .and_then(|handlers| handlers.dock.take())
            });
            if let Some(mut callback) = dock_callback {
                callback(hover.position);
                runtime.with(|rt| {
                    if let Some(handlers) = rt.borrow_mut().handlers.get_mut(hover.target) {
                        handlers.dock = Some(callback);
                    }
                });
            } else {
                let callback = runtime.with(|rt| {
                    rt.borrow_mut()
                        .handlers
                        .get_mut(hover.target)
                        .and_then(|handlers| handlers.drop.take())
                });
                if let Some(mut callback) = callback {
                    callback();
                    runtime.with(|rt| {
                        if let Some(handlers) = rt.borrow_mut().handlers.get_mut(hover.target) {
                            handlers.drop = Some(callback);
                        }
                    });
                }
            }
        }
    }
    let callback = runtime.with(|rt| {
        rt.borrow_mut()
            .handlers
            .get_mut(capture.source)
            .and_then(|handlers| handlers.drag_end.take())
    });
    if let Some(mut callback) = callback {
        callback(accepted);
        runtime.with(|rt| {
            if let Some(handlers) = rt.borrow_mut().handlers.get_mut(capture.source) {
                handlers.drag_end = Some(callback);
            }
        });
    }
    if accepted {
        DragRelease::Drop { accepted: true }
    } else {
        let clickable = runtime.with(|rt| {
            rt.borrow()
                .handlers
                .get(capture.source)
                .is_some_and(|handlers| handlers.click.is_some())
        });
        // A tab let go off every target is chosen, as a click on it. Any
        // other control clicks only when the pointer came back onto it: a
        // press slid off a close button closes nothing.
        let tab = scene
            .node(capture.source)
            .is_some_and(|node| node.kind == WidgetKind::Tab);
        let back_on_it = hit_test(runtime, scene, point).is_some_and(|hit| {
            std::iter::successors(Some(hit), |id| scene.node(*id).and_then(|node| node.parent))
                .any(|id| id == capture.source)
        });
        if clickable && (tab || back_on_it) {
            DragRelease::Click(capture.source)
        } else {
            DragRelease::Drop { accepted: false }
        }
    }
}

/// Hit-tests a pointer position to the **deepest** [`WidgetKind::Scroll`] node whose
/// laid-out viewport rect contains it (SOUL §8.1). Unlike [`hit_test`] — which targets
/// content *leaves* and treats containers as transparent — here the scroll container
/// *is* the target: this is what routes a mouse-wheel event to the innermost scroll
/// viewport under the cursor, so nested scroll areas each capture their own wheel
/// input (SOUL §3.2). Returns `None` before layout has run or when no scroll viewport
/// is hit; the resolved id feeds [`dispatch_scroll`].
pub fn hit_test_scroll(scene: &Scene, p: Point) -> Option<WidgetId> {
    fn rec(scene: &Scene, id: WidgetId, p: Point) -> Option<WidgetId> {
        if !scene.is_visible(id) || !scene.composite_context(id).1.contains(p) {
            return None;
        }
        let node = scene.node(id)?;
        // A scroll viewport clips every descendant. Rejecting an outside point
        // before descending is both correct (off-viewport children cannot receive
        // a wheel event) and avoids walking long virtualized/offscreen branches.
        if node.kind == WidgetKind::Scroll {
            let viewport = scene.composited_rect(id);
            if viewport.is_empty() || !viewport.contains(p) {
                return None;
            }
        }
        // Recurse children first so a nested (deeper) scroll wins over its ancestor.
        for &c in node.children.iter().rev() {
            if let Some(hit) = rec(scene, c, p) {
                return Some(hit);
            }
        }
        if node.kind == WidgetKind::Scroll {
            return Some(id);
        }
        None
    }
    let root = dialog::active_modal_panel(scene).or_else(|| scene.root())?;
    rec(scene, root, p)
}

/// Runtime-indexed wheel hit-test. Unlike [`hit_test_scroll`]'s retained-tree
/// fallback, this inspects only configured `Scroll` viewports, so a wheel over a
/// 1,000-row text document does not visit every leaf. It retains the same modal,
/// responsive-visibility, nested-viewport clipping, and deepest-target rules.
pub fn hit_test_scroll_in(runtime: &Runtime, scene: &Scene, p: Point) -> Option<WidgetId> {
    let root = dialog::active_modal_panel_in(runtime, scene).or_else(|| scene.root())?;

    fn depth_if_hit(scene: &Scene, root: WidgetId, id: WidgetId, p: Point) -> Option<usize> {
        let mut chain = SmallVec::<[WidgetId; 8]>::new();
        let mut current = Some(id);
        while let Some(node_id) = current {
            let node = scene.node(node_id)?;
            chain.push(node_id);
            if node_id == root {
                break;
            }
            current = node.parent;
        }
        if chain.last().copied() != Some(root) {
            return None;
        }

        if !scene.composite_context(id).1.contains(p) {
            return None;
        }
        // Test every containing viewport after inherited translations, scrolls
        // and property clips have been resolved in the renderer's coordinates.
        for &node_id in chain.iter().rev() {
            let node = scene.node(node_id)?;
            if !scene.is_visible(node_id) {
                return None;
            }
            if node.kind == WidgetKind::Scroll {
                let viewport = scene.composited_rect(node_id);
                if viewport.is_empty() || !viewport.contains(p) {
                    return None;
                }
            }
        }
        Some(chain.len() - 1)
    }

    runtime.with(|rt| {
        rt.borrow()
            .scroll_ids
            .iter()
            .copied()
            .filter_map(|id| {
                depth_if_hit(scene, root, id, p).map(|depth| {
                    // Depth preserves the nested-scroll rule. Overlay level/order
                    // make peers agree with the renderer's visual stacking order.
                    (depth, scene.overlay_level(id), scene.overlay_order(id), id)
                })
            })
            .max_by_key(|(depth, level, order, _)| (*depth, *level, *order))
            .map(|(_, _, _, id)| id)
    })
}

/// Registers a content-input handler slot for a node, creating it if absent.
pub(crate) fn with_handlers<R>(
    runtime: &Runtime,
    id: WidgetId,
    f: impl FnOnce(&mut Handlers) -> R,
) -> R {
    runtime.with(|rt| f(rt.borrow_mut().handlers.entry(id).unwrap().or_default()))
}

// ---------------------------------------------------------------------------
// containers (delegate geometry to schnellui::layout §8.1)
// ---------------------------------------------------------------------------

macro_rules! flex_container {
    ($name:ident, $kind:expr, $doc:literal) => {
        #[doc = $doc]
        pub struct $name {
            pub(crate) children: Vec<AnyView>,
            pub(crate) style: ContainerStyle,
        }

        impl $name {
            /// A new empty container.
            pub fn new() -> $name {
                $name {
                    children: Vec::new(),
                    style: ContainerStyle::new(match $kind {
                        WidgetKind::Row => schnellui::layout::Container::Row,
                        WidgetKind::Column => schnellui::layout::Container::Column,
                        WidgetKind::Stack => schnellui::layout::Container::Stack,
                        _ => schnellui::layout::Container::Column,
                    }),
                }
            }

            /// Appends a child (SOUL §3.3 `.child(…)`).
            pub fn child(mut self, c: impl View) -> $name {
                self.children.push(Box::new(c));
                self
            }

            /// Sets the gap between children on the main axis.
            pub fn gap(mut self, gap: f32) -> $name {
                self.style.gap = gap;
                self
            }

            /// Sets main-axis distribution (SOUL §8.1) — where leftover space goes.
            pub fn justify(mut self, justify: Justify) -> $name {
                self.style.justify = justify;
                self
            }

            /// Sets cross-axis alignment (SOUL §8.1).
            pub fn align(mut self, align: Align) -> $name {
                self.style.align = align;
                self
            }

            /// Lets overflowing children wrap onto additional lines instead of
            /// shrinking — the responsive-flow switch (SOUL §8.1). In `view!` this
            /// is the valueless `wrap` flag: `row(wrap, gap = 8.0) { … }`.
            pub fn wrap(mut self) -> $name {
                self.style.wrap = true;
                self
            }

            /// Sizes the container to **100% of its parent's content box** — and,
            /// at the layout root, to the viewport itself, which windowed mode
            /// re-derives from the window on every resize (SOUL §8.1). This is how
            /// a layout *tracks* the real window instead of baking a pixel size.
            /// In `view!` it is the valueless `fill` flag: `column(fill) { … }`.
            pub fn fill(mut self) -> $name {
                self.style.fill = true;
                self
            }

            /// Fixes the container's outer box (sets [`ContainerStyle::fixed_size`]).
            pub fn size(mut self, width: f32, height: f32) -> $name {
                self.style.fixed_size = Some(Size { width, height });
                self
            }

            /// Fixes the width only; the height stays content-sized (SOUL §8.1).
            /// This is what gives a wrapping row a definite line width to flow
            /// against while its height tracks the number of lines.
            pub fn width(mut self, width: f32) -> $name {
                self.style.width = Some(width);
                self
            }

            /// Fixes the height only; the width stays content-sized (SOUL §8.1).
            pub fn height(mut self, height: f32) -> $name {
                self.style.height = Some(height);
                self
            }

            /// Never grows past the parent's content box, whatever a definite
            /// `width` or `size` asks for: a 460px column in a 320px window
            /// shrinks to 320px and its text reflows, instead of running off
            /// the edge. In `view!` it is the valueless `fit_parent` flag.
            pub fn fit_parent(mut self) -> $name {
                self.style.fit_parent = true;
                self
            }

            /// Sets the minimum outer width; content may make it wider.
            pub fn min_width(mut self, width: f32) -> $name {
                self.style.min_width = Some(width.max(0.0));
                self
            }

            /// Sets the minimum outer height; content may make it taller.
            pub fn min_height(mut self, height: f32) -> $name {
                self.style.min_height = Some(height.max(0.0));
                self
            }

            /// Overrides the container style.
            pub fn style(mut self, style: ContainerStyle) -> $name {
                self.style = style;
                self
            }

            /// The scene dispatch tag for this container.
            pub fn kind(&self) -> WidgetKind {
                $kind
            }

            /// Number of children configured (pre-build).
            pub fn child_count(&self) -> usize {
                self.children.len()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl View for $name {
            fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
                let this = *self;
                let id = ctx.scene.insert($kind, parent);
                // Containers carry the transparent `Group` role — no pixels, no
                // content input of their own (SOUL §8.1).
                ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
                ctx.layout.set_container(id, this.style);
                // Build children under this node (scene.insert re-parents them).
                for child in this.children {
                    child.build(ctx, Some(id));
                }
                id
            }
        }
    };
}

flex_container!(
    Row,
    WidgetKind::Row,
    "A horizontal flex container (SOUL §8.1)."
);
flex_container!(
    Column,
    WidgetKind::Column,
    "A vertical flex container (SOUL §8.1)."
);
flex_container!(
    Stack,
    WidgetKind::Stack,
    "A Z-overlay container (SOUL §8.1). Children fill its box, except on an axis\n\
     whose `justify` (horizontal) or `align` (vertical) is `Center` or `End`: there\n\
     each child keeps its own size and is placed by that alignment, which is how an\n\
     icon is centered under a same-sized button."
);
