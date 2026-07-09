use super::*;

pub const FOCUS_RING_W: f32 = 3.0;
/// One rectangular outline is four [`Primitive::Line`]s.
pub const FOCUS_RING_PRIMS: usize = 4;
/// Hover border width, chosen not to alias either focus-ring stroke.
pub const HOVER_BORDER_W: f32 = 1.5;
/// Pressed-state border, between hover feedback and keyboard focus emphasis.
pub const ACTIVE_BORDER_W: f32 = 2.0;
/// One rectangular hover border is four retained lines.
pub const HOVER_BORDER_PRIMS: usize = 4;

pub(crate) fn ordinary_primitive_end(runtime: &Runtime, id: WidgetId, pd: &PaintData) -> usize {
    runtime.with(|runtime| {
        let runtime = runtime.borrow();
        if let Some(tooltip) = runtime.hover_tooltips.get(id) {
            return tooltip.base_primitive_end.min(pd.primitives.len());
        }

        let mut end = pd.primitives.len();
        if runtime
            .ring
            .as_ref()
            .is_some_and(|ring| ring.border_owner == Some(id))
            && end >= FOCUS_RING_PRIMS
            && pd.primitives[end - FOCUS_RING_PRIMS..]
                .iter()
                .all(|primitive| matches!(primitive, Primitive::Line { width, .. } if *width == FOCUS_RING_W))
        {
            end -= FOCUS_RING_PRIMS;
        }
        if runtime
            .active
            .as_ref()
            .is_some_and(|active| active.border_owner == Some(id))
            && end >= FOCUS_RING_PRIMS
            && pd.primitives[end - FOCUS_RING_PRIMS..end]
                .iter()
                .all(|primitive| matches!(primitive, Primitive::Line { width, .. } if *width == ACTIVE_BORDER_W))
        {
            end -= FOCUS_RING_PRIMS;
        }
        if runtime
            .hover
            .as_ref()
            .is_some_and(|hover| hover.border_owner == Some(id))
            && end >= HOVER_BORDER_PRIMS
            && pd.primitives[end - HOVER_BORDER_PRIMS..end]
                .iter()
                .all(|primitive| matches!(primitive, Primitive::Line { width, .. } if *width == HOVER_BORDER_W))
        {
            end -= HOVER_BORDER_PRIMS;
        }
        end
    })
}

/// The bounding box of a node's current paint primitives — the rect the ring
/// hugs. Primitive bounds (not the layout rect) because a stretched layout box
/// can be wider than the pixels the widget actually painted.
pub fn paint_bounds(runtime: &Runtime, scene: &Scene, id: WidgetId) -> Option<Rect> {
    let pd = scene.paint(id)?;
    let primitive_end = ordinary_primitive_end(runtime, id, pd);
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for prim in &pd.primitives[..primitive_end] {
        let (x0, y0, x1, y1) = match *prim {
            Primitive::SolidRect { rect, .. }
            | Primitive::GlyphQuad { rect, .. }
            | Primitive::ImageQuad { rect, .. } => {
                (rect.x, rect.y, rect.x + rect.width, rect.y + rect.height)
            }
            Primitive::Line {
                from, to, width, ..
            } => (
                from.x.min(to.x) - width * 0.5,
                from.y.min(to.y) - width * 0.5,
                from.x.max(to.x) + width * 0.5,
                from.y.max(to.y) + width * 0.5,
            ),
        };
        min_x = min_x.min(x0);
        min_y = min_y.min(y0);
        max_x = max_x.max(x1);
        max_y = max_y.max(y1);
    }
    if min_x.is_finite() && max_x > min_x && max_y > min_y {
        Some(Rect::new(min_x, min_y, max_x - min_x, max_y - min_y))
    } else {
        None
    }
}

/// Bounds all paint in a semantic target's subtree and returns the last painted
/// descendant as the decoration owner. Containers such as clickable table rows
/// paint through children, so attaching their feedback to that final descendant
/// keeps it above every cell in the wgpu painter's tree order.
fn decoration_geometry(runtime: &Runtime, scene: &Scene, id: WidgetId) -> Option<(Rect, WidgetId)> {
    fn visit(
        runtime: &Runtime,
        scene: &Scene,
        id: WidgetId,
        bounds: &mut Option<Rect>,
        owner: &mut Option<WidgetId>,
    ) {
        if let Some(rect) = paint_bounds(runtime, scene, id) {
            *bounds = Some(match *bounds {
                Some(current) => current.union(&rect),
                None => rect,
            });
            *owner = Some(id);
        }
        if let Some(node) = scene.node(id) {
            for child in &node.children {
                visit(runtime, scene, *child, bounds, owner);
            }
        }
    }

    let mut bounds = None;
    let mut owner = None;
    visit(runtime, scene, id, &mut bounds, &mut owner);
    bounds.zip(owner)
}

/// Appends one rectangular stroke of 4 [`Primitive::Line`]s along `rect`'s edges,
/// each endpoint inset by `inset` plus half the stroke width — so the stroke's
/// extent stays **inside** `rect` and the node's min-origin (what
/// [`reposition_paint`] anchors by) is unchanged by wearing a ring.
fn push_ring_stroke(pd: &mut PaintData, rect: Rect, inset: f32, width: f32, color: Color) {
    let half = width * 0.5;
    let (x0, y0) = (rect.x + inset + half, rect.y + inset + half);
    let (x1, y1) = (
        rect.x + rect.width - inset - half,
        rect.y + rect.height - inset - half,
    );
    for (from, to) in [
        (Point { x: x0, y: y0 }, Point { x: x1, y: y0 }), // top
        (Point { x: x0, y: y1 }, Point { x: x1, y: y1 }), // bottom
        (Point { x: x0, y: y0 }, Point { x: x0, y: y1 }), // left
        (Point { x: x1, y: y0 }, Point { x: x1, y: y1 }), // right
    ] {
        pd.primitives.push(Primitive::Line {
            from,
            to,
            width,
            color,
        });
    }
}

/// Strips a previously-appended focus ring — the trailing [`FOCUS_RING_PRIMS`]
/// primitives, verified against the exact outer/inner pattern so nothing else
/// (a link's underline, a caret) is ever eaten. Returns `true` if a ring was
/// removed; `false` when none was present (e.g. a state re-emit already cleared
/// the whole primitive set).
pub fn strip_focus_decoration(runtime: &Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    let decoration = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        if rt.ring.as_ref().is_some_and(|ring| ring.target == id) {
            rt.ring.take()
        } else {
            None
        }
    });
    decoration.is_some_and(|decoration| {
        strip_applied_interaction(runtime, scene, decoration, FOCUS_RING_W)
    })
}

/// Draws the keyboard **focus outline** on `id` (SOUL §6.3): the same custom
/// 3px inset box used by the native HTML renderer. Its color is derived by
/// [`Theme::focus_color`] to retain at least 3:1 contrast against the theme's
/// common adjacent surfaces. Idempotent: an existing outline is stripped before
/// the new one is appended.
pub(crate) fn apply_focus_ring(runtime: &Runtime, scene: &mut Scene, id: WidgetId) {
    if scene.node(id).is_none() {
        return;
    }
    strip_focus_decoration(runtime, scene, id);
    let Some(component) = interaction_component(scene, id) else {
        return;
    };
    let theme = theme_for(runtime, id);
    let mut style = theme.interaction_style(component, InteractionState::Focus);
    if style == InteractionStyle::NONE {
        style.border = Some(theme.focus_color());
    }
    if let Some(decoration) = apply_interaction_style(runtime, scene, id, style, FOCUS_RING_W) {
        runtime.with(|rt| rt.borrow_mut().ring = Some(decoration));
    }
}

/// Removes the focus ring from `id` on blur (SOUL §6.3). Safe when no ring is
/// present (for example, an intervening re-emit cleared the primitives).
pub(crate) fn remove_focus_ring(runtime: &Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    let removed = strip_focus_decoration(runtime, scene, id);
    if removed {
        scene.mark_dirty(id, DirtyFlags::PAINT);
    }
    removed
}

/// Makes the generic focus indicator match the current input modality. Returns
/// whether paint changed. Editables are intentionally unaffected: their focused
/// border and caret are part of their own paint rather than the generic ring.
pub fn set_focus_ring_visible(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    visible: bool,
) -> bool {
    if scene.node(id).is_none() {
        return false;
    }
    let wearing = runtime.with(|rt| {
        rt.borrow()
            .ring
            .as_ref()
            .is_some_and(|decoration| decoration.target == id)
    });
    if visible {
        if wearing {
            false
        } else {
            apply_focus_ring(runtime, scene, id);
            runtime.with(|rt| {
                rt.borrow()
                    .ring
                    .as_ref()
                    .is_some_and(|decoration| decoration.target == id)
            })
        }
    } else {
        remove_focus_ring(runtime, scene, id)
    }
}

/// Re-applies the ring after a state dispatch **cleared-and-refilled** a focused
/// widget's paint (checkbox / switch / radio toggle, slider adjust) — otherwise
/// activating a focused control would silently drop its ring (SOUL §6.3). A no-op
/// unless `id` is the current ring wearer.
pub(crate) fn reapply_focus_ring(runtime: &Runtime, scene: &mut Scene, id: WidgetId) {
    let (wearing, hovered, active) = runtime.with(|rt| {
        let rt = rt.borrow();
        (
            rt.ring
                .as_ref()
                .is_some_and(|decoration| decoration.target == id),
            rt.hover
                .as_ref()
                .is_some_and(|decoration| decoration.target == id),
            rt.active
                .as_ref()
                .is_some_and(|decoration| decoration.target == id),
        )
    });
    if wearing {
        remove_focus_ring(runtime, scene, id);
    }
    if active {
        strip_active_decoration(runtime, scene);
    }
    if hovered {
        apply_hover_decoration(runtime, scene, id);
    }
    if active {
        apply_active_decoration(runtime, scene, id);
    }
    if wearing {
        apply_focus_ring(runtime, scene, id);
    }
}

fn interaction_component(scene: &Scene, id: WidgetId) -> Option<InteractionComponent> {
    match scene.node(id)?.kind {
        WidgetKind::Button => Some(InteractionComponent::Button),
        WidgetKind::Tab
        | WidgetKind::ListItem
        | WidgetKind::TableRow
        | WidgetKind::Link
        | WidgetKind::DropdownOption => Some(InteractionComponent::Navigation),
        WidgetKind::Checkbox | WidgetKind::Slider | WidgetKind::Switch | WidgetKind::Radio => {
            Some(InteractionComponent::Toggle)
        }
        WidgetKind::TextInput | WidgetKind::TextArea | WidgetKind::Dropdown => {
            Some(InteractionComponent::Editable)
        }
        WidgetKind::Scroll
        | WidgetKind::TerminalGrid
        | WidgetKind::Image
        | WidgetKind::RichText => Some(InteractionComponent::RawSurface),
        _ => None,
    }
}

fn subtree_ids(scene: &Scene, root: WidgetId) -> Vec<WidgetId> {
    fn collect(scene: &Scene, id: WidgetId, ids: &mut Vec<WidgetId>) {
        ids.push(id);
        if let Some(node) = scene.node(id) {
            for child in &node.children {
                collect(scene, *child, ids);
            }
        }
    }
    let mut ids = Vec::new();
    collect(scene, root, &mut ids);
    ids
}

fn strip_applied_interaction(
    runtime: &Runtime,
    scene: &mut Scene,
    decoration: AppliedInteraction,
    border_width: f32,
) -> bool {
    for restore in &decoration.foreground {
        if let Some(primitive) = scene
            .paint_mut(restore.owner)
            .primitives
            .get_mut(restore.index)
        {
            match primitive {
                Primitive::GlyphQuad { color, .. } | Primitive::Line { color, .. } => {
                    *color = restore.color;
                }
                Primitive::ImageQuad { tint, .. } => *tint = restore.color,
                Primitive::SolidRect { .. } => {}
            }
            scene.mark_dirty(restore.owner, DirtyFlags::PAINT);
        }
    }
    if let Some(owner) = decoration.border_owner {
        let position = scene.paint(owner).and_then(|pd| {
            pd.primitives
                .windows(FOCUS_RING_PRIMS)
                .rposition(|window| {
                    window.iter().all(
                        |primitive| matches!(primitive, Primitive::Line { width, .. } if *width == border_width),
                    )
                })
        });
        if let Some(position) = position {
            scene
                .paint_mut(owner)
                .primitives
                .drain(position..position + FOCUS_RING_PRIMS);
            scene.mark_dirty(owner, DirtyFlags::PAINT);
        }
    }
    if let Some((owner, position)) = decoration.background {
        if scene.paint(owner).is_some_and(|paint| {
            matches!(
                paint.primitives.get(position),
                Some(Primitive::SolidRect { .. })
            )
        }) {
            scene.paint_mut(owner).primitives.remove(position);
            runtime.with(|rt| {
                if let Some(tooltip) = rt.borrow_mut().hover_tooltips.get_mut(owner) {
                    if position < tooltip.background {
                        tooltip.background = tooltip.background.saturating_sub(1);
                        tooltip.glyph_start = tooltip.glyph_start.saturating_sub(1);
                        tooltip.glyph_end = tooltip.glyph_end.saturating_sub(1);
                    }
                    tooltip.base_primitive_end = tooltip.base_primitive_end.saturating_sub(1);
                }
            });
            scene.mark_dirty(owner, DirtyFlags::PAINT);
        }
    }
    true
}

fn apply_interaction_style(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    style: InteractionStyle,
    border_width: f32,
) -> Option<AppliedInteraction> {
    let Some((rect, owner)) = decoration_geometry(runtime, scene, id) else {
        return None;
    };
    let theme = theme_for(runtime, id);
    if style == InteractionStyle::NONE {
        return None;
    }

    let background = style.background.map(|color| {
        let end = scene
            .paint(id)
            .map_or(0, |paint| ordinary_primitive_end(runtime, id, paint));
        let position = scene.paint(id).map_or(0, |paint| {
            paint.primitives[..end]
                .iter()
                .position(|primitive| {
                    matches!(
                        primitive,
                        Primitive::GlyphQuad { .. } | Primitive::ImageQuad { .. }
                    )
                })
                .unwrap_or(end)
        });
        scene.paint_mut(id).primitives.insert(
            position,
            Primitive::SolidRect {
                rect,
                color,
                corner_radius: theme.shape.radius(3.0, rect.height),
            },
        );
        runtime.with(|rt| {
            if let Some(tooltip) = rt.borrow_mut().hover_tooltips.get_mut(id) {
                tooltip.base_primitive_end += 1;
                if position <= tooltip.background {
                    tooltip.background += 1;
                    tooltip.glyph_start += 1;
                    tooltip.glyph_end += 1;
                }
            }
        });
        scene.mark_dirty(id, DirtyFlags::PAINT);
        (id, position)
    });

    let mut foreground = Vec::new();
    if let Some(next) = style.foreground {
        for child in subtree_ids(scene, id) {
            let end = scene
                .paint(child)
                .map_or(0, |paint| ordinary_primitive_end(runtime, child, paint));
            let paint = scene.paint_mut(child);
            for (index, primitive) in paint.primitives[..end].iter_mut().enumerate() {
                let color = match primitive {
                    Primitive::GlyphQuad { color, .. } | Primitive::Line { color, .. } => color,
                    Primitive::ImageQuad { tint, .. } => tint,
                    Primitive::SolidRect { .. } => continue,
                };
                foreground.push(PrimitiveColorRestore {
                    owner: child,
                    index,
                    color: *color,
                });
                *color = next;
            }
            if foreground.iter().any(|restore| restore.owner == child) {
                scene.mark_dirty(child, DirtyFlags::PAINT);
            }
        }
    }

    let border_owner = style.border.map(|border| {
        push_ring_stroke(scene.paint_mut(owner), rect, 0.0, border_width, border);
        scene.mark_dirty(owner, DirtyFlags::PAINT);
        owner
    });
    Some(AppliedInteraction {
        target: id,
        border_owner,
        foreground,
        background,
    })
}

pub fn strip_hover_decoration(runtime: &Runtime, scene: &mut Scene) -> bool {
    runtime
        .with(|rt| rt.borrow_mut().hover.take())
        .is_some_and(|decoration| {
            strip_applied_interaction(runtime, scene, decoration, HOVER_BORDER_W)
        })
}

fn apply_hover_decoration(runtime: &Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    strip_hover_decoration(runtime, scene);
    let Some(component) = interaction_component(scene, id) else {
        return false;
    };
    let style = theme_for(runtime, id).interaction_style(component, InteractionState::Hover);
    let Some(decoration) = apply_interaction_style(runtime, scene, id, style, HOVER_BORDER_W)
    else {
        return false;
    };
    runtime.with(|rt| rt.borrow_mut().hover = Some(decoration));
    true
}

pub fn strip_active_decoration(runtime: &Runtime, scene: &mut Scene) -> bool {
    runtime
        .with(|rt| rt.borrow_mut().active.take())
        .is_some_and(|decoration| {
            strip_applied_interaction(runtime, scene, decoration, ACTIVE_BORDER_W)
        })
}

fn apply_active_decoration(runtime: &Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    strip_active_decoration(runtime, scene);
    let Some(component) = interaction_component(scene, id) else {
        return false;
    };
    let style = theme_for(runtime, id).interaction_style(component, InteractionState::Active);
    let Some(decoration) = apply_interaction_style(runtime, scene, id, style, ACTIVE_BORDER_W)
    else {
        return false;
    };
    runtime.with(|rt| rt.borrow_mut().active = Some(decoration));
    true
}

/// Applies or clears the retained active/pressed interaction state. This mutates
/// only primitive colors and a bounded set of decoration primitives; GPU shader
/// and pipeline state are unaffected.
pub fn set_active_interaction(
    runtime: &Runtime,
    scene: &mut Scene,
    target: Option<WidgetId>,
) -> bool {
    let mut candidate = target;
    let mut resolved = None;
    while let Some(id) = candidate {
        let enabled = scene.a11y(id).is_some_and(|a| {
            !StateFlags(a.state).contains(StateFlags::DISABLED)
                && ActionFlags(a.actions).contains(ActionFlags::FOCUS)
        });
        if enabled
            && interaction_component(scene, id).is_some_and(|component| {
                theme_for(runtime, id).interaction_style(component, InteractionState::Active)
                    != InteractionStyle::NONE
            })
        {
            resolved = Some(id);
            break;
        }
        candidate = scene.node(id).and_then(|node| node.parent);
    }
    let target = resolved;
    let current = runtime.with(|rt| rt.borrow().active.as_ref().map(|active| active.target));
    if current == target {
        return false;
    }
    let focused = runtime.with(|rt| rt.borrow().ring.as_ref().map(|ring| ring.target));
    if let Some(id) = focused {
        remove_focus_ring(runtime, scene, id);
    }
    let mut changed = strip_active_decoration(runtime, scene);
    if let Some(id) = target {
        changed |= apply_active_decoration(runtime, scene, id);
    }
    if let Some(id) = focused {
        apply_focus_ring(runtime, scene, id);
    }
    changed
}

/// Hit-tests a pointer position to the top-most **content leaf** whose composited rect
/// contains it (SOUL §8.1). Scroll offsets and viewport clips follow the renderer's
/// accumulated transform, while containers remain transparent to hit-testing (they
/// carry no role and take no content input). Returns `None` before layout has run
/// (rects are empty) or on a miss. The resolved id feeds [`dispatch_click`] — the
/// pointer path that converges with the action path (§6.3).
///
/// **Overlay subtrees hit-test first** (SOUL §3.2 z-order): the renderer draws a
/// [`Scene::is_overlay`] subtree above everything after it in tree order, so the
/// pointer must resolve into it before the base content it covers — otherwise the
/// content under a dropdown's floating option list would steal its clicks.
pub fn hit_test(runtime: &Runtime, scene: &Scene, p: Point) -> Option<WidgetId> {
    fn translated_rect(rect: Rect, offset: Point) -> Rect {
        Rect::new(
            rect.x + offset.x,
            rect.y + offset.y,
            rect.width,
            rect.height,
        )
    }

    fn child_composite_state(
        scene: &Scene,
        id: WidgetId,
        offset: Point,
        clip: Option<Rect>,
    ) -> (Point, Option<Rect>) {
        if scene.node(id).map(|node| node.kind) != Some(WidgetKind::Scroll) {
            return (offset, clip);
        }
        let scroll = scene.scroll_offset(id);
        let child_offset = Point {
            x: offset.x - scroll.x,
            y: offset.y - scroll.y,
        };
        let viewport = scene
            .layout(id)
            .map(|layout| translated_rect(layout.rect, offset))
            .filter(|rect| !rect.is_empty());
        let child_clip = match (clip, viewport) {
            (Some(parent), Some(viewport)) => Some(parent.intersect(&viewport)),
            (None, Some(viewport)) => Some(viewport),
            (parent, None) => parent,
        };
        (child_offset, child_clip)
    }

    fn contains(scene: &Scene, id: WidgetId, p: Point, offset: Point) -> bool {
        scene
            .layout(id)
            .is_some_and(|layout| translated_rect(layout.rect, offset).contains(p))
    }

    // The base-content leaf hit: children last-drawn-first (top-most wins). With
    // `skip_overlays`, overlay subtrees are invisible (they belong to the layer
    // above); without it, the same walk resolves *inside* one overlay subtree.
    fn leaf_hit(
        runtime: &Runtime,
        scene: &Scene,
        id: WidgetId,
        p: Point,
        skip_overlays: bool,
        offset: Point,
        clip: Option<Rect>,
    ) -> Option<WidgetId> {
        if !scene.is_visible(id) || clip.is_some_and(|clip| !clip.contains(p)) {
            return None;
        }
        if skip_overlays && scene.is_overlay(id) {
            return None;
        }
        let node = scene.node(id)?;
        let (child_offset, child_clip) = child_composite_state(scene, id, offset, clip);
        for &c in node.children.iter().rev() {
            if let Some(hit) = leaf_hit(
                runtime,
                scene,
                c,
                p,
                skip_overlays,
                child_offset,
                child_clip,
            ) {
                return Some(hit);
            }
        }
        // A DockArea is intentionally an implicit full-surface target. Its
        // ordinary descendants still win first (so tabs remain clickable and
        // directly droppable), but otherwise its empty pane surface accepts the
        // pointer instead of requiring a visible target control.
        if runtime.with(|rt| {
            rt.borrow()
                .handlers
                .get(id)
                .is_some_and(|handlers| handlers.dock.is_some())
        }) && contains(scene, id, p, offset)
        {
            return Some(id);
        }
        // A dialog surface is a semantic container, but its empty padded areas
        // still form an opaque pointer surface. Returning it prevents a click
        // inside the panel from being mistaken for a backdrop click.
        if node.kind == WidgetKind::Dialog && contains(scene, id, p, offset) {
            return Some(id);
        }
        if !node.kind.is_container() && contains(scene, id, p, offset) {
            return Some(id);
        }
        None
    }
    // The overlay layer: resolve every top-level overlay at the point and keep
    // the same highest `(level, within-level order)` the renderer paints last.
    fn overlay_hit(
        runtime: &Runtime,
        scene: &Scene,
        id: WidgetId,
        p: Point,
        offset: Point,
        clip: Option<Rect>,
        best: &mut Option<((u8, u64), WidgetId)>,
    ) {
        if !scene.is_visible(id) || clip.is_some_and(|clip| !clip.contains(p)) {
            return;
        }
        let Some(node) = scene.node(id) else { return };
        if scene.is_overlay(id) {
            let mut hit = leaf_hit(runtime, scene, id, p, false, offset, clip);
            // A modal dialog's layer is the backdrop target: it captures the
            // whole layer after every child has had first chance to hit.
            if hit.is_none()
                && node.kind == WidgetKind::DialogLayer
                && dialog::layer_is_modal(runtime, id)
                && contains(scene, id, p, offset)
            {
                hit = Some(id);
            }
            if let Some(hit) = hit {
                let key = (scene.overlay_level(id), scene.overlay_order(id));
                if best.as_ref().is_none_or(|(current, _)| key > *current) {
                    *best = Some((key, hit));
                }
            }
            // Nested overlays are composited as part of this deferred subtree.
            return;
        }
        let (child_offset, child_clip) = child_composite_state(scene, id, offset, clip);
        for &child in &node.children {
            overlay_hit(runtime, scene, child, p, child_offset, child_clip, best);
        }
    }
    // A focus-grabbing modal owns the entire input plane even when a modeless
    // dialog was declared later. Search only its layer; a miss inside the layer
    // is the modal backdrop target. Every sibling and lower dialog is inert.
    if let Some(layer) = dialog::active_modal_layer(runtime, scene) {
        if let Some(hit) = leaf_hit(runtime, scene, layer, p, false, Point::default(), None) {
            return Some(hit);
        }
        return scene
            .layout(layer)
            .is_some_and(|layout| layout.rect.contains(p))
            .then_some(layer);
    }

    let root = scene.root()?;
    let mut best = None;
    overlay_hit(runtime, scene, root, p, Point::default(), None, &mut best);
    best.map(|(_, hit)| hit)
        .or_else(|| leaf_hit(runtime, scene, root, p, true, Point::default(), None))
}

/// Resolves the native-style pointer cursor at a logical scene position.
///
/// Dialog chrome has priority over its child label nodes, and an active dialog
/// capture keeps its move/resize cursor even if the pointer leaves the chrome.
/// For ordinary content, semantic actions are followed through ancestors so a
/// table cell correctly inherits its selectable row's pointer cursor.
pub fn cursor_at(runtime: &Runtime, scene: &Scene, p: Point) -> CursorIcon {
    if let Some(cursor) = dialog::captured_cursor(runtime) {
        return cursor;
    }
    if let Some(capture) = runtime.with(|rt| rt.borrow().drag_pointer) {
        if capture.active {
            let clickable = runtime.with(|rt| {
                rt.borrow()
                    .handlers
                    .get(capture.source)
                    .is_some_and(|handlers| handlers.click.is_some())
            });
            return if clickable {
                CursorIcon::Pointer
            } else {
                CursorIcon::Grabbing
            };
        }
    }
    let Some(hit) = hit_test(runtime, scene, p) else {
        return CursorIcon::Default;
    };
    if let Some(cursor) = dialog::cursor_for_hit(runtime, scene, hit, p) {
        return cursor;
    }

    let mut current = Some(hit);
    while let Some(id) = current {
        let Some(node) = scene.node(id) else {
            break;
        };
        let semantics = scene.a11y(id);
        let disabled = semantics
            .map(|a| StateFlags(a.state).contains(StateFlags::DISABLED))
            .unwrap_or(false);
        if disabled {
            return CursorIcon::Default;
        }
        let (clickable, draggable) = runtime.with(|rt| {
            let rt = rt.borrow();
            let handlers = rt.handlers.get(id);
            (
                handlers.is_some_and(|handlers| handlers.click.is_some()),
                handlers.is_some_and(|handlers| handlers.drag_start.is_some())
                    || rt.tab_reorder_items.contains_key(id),
            )
        });
        // A control that is both clickable and draggable (most notably a tab)
        // keeps the pointer cursor until its drag has actually started. Switching
        // between pointer and grab at an adjacent close button moves the native
        // cursor hotspot enough to bounce hit-testing across their shared edge.
        if clickable {
            return CursorIcon::Pointer;
        }
        if draggable {
            return CursorIcon::Grab;
        }
        match node.kind {
            WidgetKind::TextInput | WidgetKind::TextArea => return CursorIcon::Text,
            WidgetKind::Slider => return CursorIcon::EwResize,
            _ => {}
        }
        if semantics
            .map(|a| ActionFlags(a.actions).contains(ActionFlags::CLICK))
            .unwrap_or(false)
        {
            return CursorIcon::Pointer;
        }
        current = node.parent;
    }
    CursorIcon::Default
}

fn hover_target_at(runtime: &Runtime, scene: &Scene, point: Point) -> Option<WidgetId> {
    let mut current = hit_test(runtime, scene, point);
    while let Some(id) = current {
        let semantics = scene.a11y(id);
        let disabled = semantics
            .map(|a| StateFlags(a.state).contains(StateFlags::DISABLED))
            .unwrap_or(false);
        if disabled {
            return None;
        }
        let paints_hover = interaction_component(scene, id).is_some_and(|component| {
            theme_for(runtime, id).interaction_style(component, InteractionState::Hover)
                != InteractionStyle::NONE
        });
        let hoverable = paints_hover
            && semantics
                .map(|a| {
                    let actions = ActionFlags(a.actions);
                    actions.contains(ActionFlags::FOCUS)
                        || (Role::from_u16(a.role) == Role::MenuItem
                            && actions.contains(ActionFlags::CLICK))
                })
                .unwrap_or(false);
        if hoverable {
            return Some(id);
        }
        current = scene.node(id).and_then(|node| node.parent);
    }
    None
}

fn update_control_hover(runtime: &Runtime, scene: &mut Scene, point: Point) -> bool {
    let next = hover_target_at(runtime, scene, point);
    let current = runtime.with(|rt| rt.borrow().hover.as_ref().map(|hover| hover.target));
    if current == next {
        return false;
    }

    let focused_ring = runtime.with(|rt| rt.borrow().ring.as_ref().map(|ring| ring.target));
    let active = runtime.with(|rt| rt.borrow().active.as_ref().map(|active| active.target));
    if let Some(id) = focused_ring {
        remove_focus_ring(runtime, scene, id);
    }
    strip_active_decoration(runtime, scene);
    let mut changed = strip_hover_decoration(runtime, scene);
    if let Some(id) = next {
        changed |= apply_hover_decoration(runtime, scene, id);
    }
    if let Some(id) = active {
        changed |= apply_active_decoration(runtime, scene, id);
    }
    // Keep the stronger keyboard-focus indicator above the hover wash when both
    // states apply to the same control.
    if let Some(id) = focused_ring {
        apply_focus_ring(runtime, scene, id);
    }
    changed
}

/// Reveals or hides pointer feedback, proximity UI such as [`DragHandle`]s, and
/// hover tooltips. Returns `true` only when paint changed.
pub fn update_pointer_proximity(runtime: &Runtime, scene: &mut Scene, point: Point) -> bool {
    let control_changed = update_control_hover(runtime, scene, point);
    let proximity_changed = runtime.with(|registry| {
        let mut registry = registry.borrow_mut();
        let captured_source = registry.drag_pointer.map(|capture| capture.source);
        let mut changed = false;
        let mut tooltip_changed = false;
        for (id, reveal) in &mut registry.proximity_reveals {
            let Some(rect) = scene.layout(id).map(|layout| layout.rect) else {
                continue;
            };
            let distance = reveal.distance;
            let near = point.x >= rect.x - distance
                && point.x <= rect.right() + distance
                && point.y >= rect.y - distance
                && point.y <= rect.bottom() + distance;
            let visible = near || captured_source == Some(id);
            if visible != reveal.visible {
                reveal.visible = visible;
                emit_drag_handle(runtime, scene, id, visible);
                scene.mark_dirty(id, DirtyFlags::PAINT);
                changed = true;
            }
        }
        for (id, tooltip) in &mut registry.hover_tooltips {
            let Some(rect) = scene.layout(id).map(|layout| layout.rect) else {
                continue;
            };
            let visible = rect.contains(point);
            if visible == tooltip.visible {
                continue;
            }
            tooltip.visible = visible;
            if let Some(Primitive::SolidRect { color, .. }) =
                scene.paint_mut(id).primitives.get_mut(tooltip.background)
            {
                *color = if visible {
                    tooltip.background_color
                } else {
                    Color::TRANSPARENT
                };
            }
            let pd = scene.paint_mut(id);
            for primitive in &mut pd.primitives[tooltip.glyph_start..tooltip.glyph_end] {
                if let Primitive::GlyphQuad { color, .. } = primitive {
                    *color = if visible {
                        tooltip.text_color
                    } else {
                        Color::TRANSPARENT
                    };
                }
            }
            scene.mark_dirty(id, DirtyFlags::PAINT);
            changed = true;
            tooltip_changed = true;
        }
        // Tooltip paint extends beyond its compact button's layout box. Expand
        // hover damage to the retained root so both reveal and dismissal redraw
        // the complete label instead of only the icon hit target.
        if tooltip_changed {
            if let Some(root) = scene.root() {
                scene.mark_dirty(root, DirtyFlags::PAINT);
            }
        }
        changed
    });
    control_changed | proximity_changed
}
