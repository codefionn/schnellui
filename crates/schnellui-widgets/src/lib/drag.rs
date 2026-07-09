use super::*;

const DRAG_THRESHOLD: f32 = 5.0;
const DROP_RING_WIDTH: f32 = 3.0;
const DROP_RING_PRIMS: usize = 4;
const DOCK_PREVIEW_PRIMS: usize = 5;
const DOCK_EDGE_FRACTION: f32 = 0.28;

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

fn drag_source_ancestor(runtime: &Runtime, scene: &Scene, from: WidgetId) -> Option<WidgetId> {
    let mut current = Some(from);
    while let Some(id) = current {
        let draggable = runtime.with(|rt| {
            let rt = rt.borrow();
            rt.handlers
                .get(id)
                .is_some_and(|handlers| handlers.drag_start.is_some())
                || rt.tab_reorder_items.contains_key(id)
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

fn strip_drop_preview(scene: &mut Scene, hover: DropHover) {
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

fn dock_preview_rect(rect: Rect, position: DockPosition) -> Rect {
    match position {
        DockPosition::Center => Rect::new(
            rect.x + rect.width * 0.08,
            rect.y + rect.height * 0.08,
            rect.width * 0.84,
            rect.height * 0.84,
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
    if let (Some((_, source_item)), Some((target, target_item))) = (
        tab_reorder_item_ancestor(runtime, scene, source),
        tab_reorder_item_ancestor(runtime, scene, hit),
    ) {
        if source_item.bar == target_item.bar && source != target {
            let after_target = scene
                .layout(target)
                .is_some_and(|layout| point.x >= layout.rect.x + layout.rect.width * 0.5);
            let insertion = target_item.index + usize::from(after_target);
            let to = if insertion > source_item.index {
                insertion - 1
            } else {
                insertion
            };
            return Some(DropHover {
                target,
                preview: target,
                position: DockPosition::Center,
                reorder: Some(TabReorderHover {
                    bar: source_item.bar,
                    from: source_item.index,
                    to,
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
    let position = if is_dock {
        scene
            .layout(target)
            .map(|layout| dock_position(layout.rect, point))
            .unwrap_or(DockPosition::Center)
    } else {
        DockPosition::Center
    };
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
        reorder: None,
        preview_prims: if is_dock {
            DOCK_PREVIEW_PRIMS
        } else {
            DROP_RING_PRIMS
        },
    })
}

fn apply_drop_preview(runtime: &Runtime, scene: &mut Scene, hover: DropHover) {
    let rect = if hover.preview_prims == DOCK_PREVIEW_PRIMS {
        let Some(layout) = scene.layout(hover.target) else {
            return;
        };
        dock_preview_rect(layout.rect, hover.position)
    } else {
        let Some(rect) = paint_bounds(runtime, scene, hover.target) else {
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
                color: Color::rgba(
                    theme_for(runtime, hover.preview).attention.r,
                    theme_for(runtime, hover.preview).attention.g,
                    theme_for(runtime, hover.preview).attention.b,
                    58,
                ),
                corner_radius: 5.0,
            });
    }
    if rect.is_empty() {
        return;
    }
    let half = DROP_RING_WIDTH * 0.5;
    let x0 = rect.x + half;
    let y0 = rect.y + half;
    let x1 = rect.right() - half;
    let y1 = rect.bottom() - half;
    let color = theme_for(runtime, hover.preview).attention;
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
            active: false,
            hovered: None,
        });
    });
    true
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
        changed = true;
    }

    let hovered = resolve_drop_hover(runtime, scene, point, capture.source)
        .filter(|hover| hover.target != capture.source);
    if hovered != capture.hovered {
        if let Some(previous) = capture.hovered {
            strip_drop_preview(scene, previous);
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

/// Releases a possible/active content drag. A sub-threshold gesture becomes a
/// click; a real drag fires the hovered target's drop handler followed by the
/// source's `on_drag_end(accepted)` callback.
pub fn end_drag(runtime: &Runtime, scene: &mut Scene, _point: Point) -> DragRelease {
    let Some(capture) = runtime.with(|rt| rt.borrow_mut().drag_pointer.take()) else {
        return DragRelease::None;
    };
    if !capture.active {
        return DragRelease::Click(capture.source);
    }
    if let Some(hover) = capture.hovered {
        strip_drop_preview(scene, hover);
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
        if clickable {
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
        if !scene.is_visible(id) {
            return None;
        }
        let node = scene.node(id)?;
        // A scroll viewport clips every descendant. Rejecting an outside point
        // before descending is both correct (off-viewport children cannot receive
        // a wheel event) and avoids walking long virtualized/offscreen branches.
        if node.kind == WidgetKind::Scroll {
            let viewport = scene.layout(id)?.rect;
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

        // Layout rects are uncomposited; descendants move by every ancestor
        // scroll offset. Walk root→target so each viewport is tested in the same
        // transformed coordinate space the renderer uses.
        let mut ancestor_offset_y = 0.0;
        for &node_id in chain.iter().rev() {
            let node = scene.node(node_id)?;
            if !scene.is_visible(node_id) {
                return None;
            }
            if node.kind == WidgetKind::Scroll {
                let viewport = scene.layout(node_id)?.rect;
                let transformed_point = Point {
                    x: p.x,
                    y: p.y + ancestor_offset_y,
                };
                if viewport.is_empty() || !viewport.contains(transformed_point) {
                    return None;
                }
                ancestor_offset_y += scene.scroll_offset(node_id).y;
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
// containers (delegate geometry to schnellui-layout §8.1)
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
                        WidgetKind::Row => schnellui_layout::Container::Row,
                        WidgetKind::Column => schnellui_layout::Container::Column,
                        WidgetKind::Stack => schnellui_layout::Container::Stack,
                        _ => schnellui_layout::Container::Column,
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
    "A Z-overlay container (SOUL §8.1)."
);
