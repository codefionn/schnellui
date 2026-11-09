use super::*;

pub const FOCUS_RING_W: f32 = 2.0;
/// Gap between a free-standing control's edge and its outset focus ring, so
/// an accent-filled control and an accent ring never merge into one shape.
pub const FOCUS_RING_GAP: f32 = 2.0;
/// How far a toggle's hover halo reaches past the control.
pub const TOGGLE_HALO: f32 = 4.0;
/// One focus ring is a single [`Primitive::StrokeRect`].
pub const FOCUS_RING_PRIMS: usize = 1;
/// Hover border width, chosen not to alias either focus-ring stroke.
pub const HOVER_BORDER_W: f32 = 1.5;
/// Pressed-state border, wider than hover and never equal to the focus ring.
pub const ACTIVE_BORDER_W: f32 = 2.5;
/// A link has no surface of its own to take a corner radius from.
const LINK_RING_RADIUS: f32 = 3.0;
/// One hover border is a single retained stroke.
pub const HOVER_BORDER_PRIMS: usize = 1;

/// Where `decoration`'s stroke starts in `pd`, if it is still there: one
/// stroked rect of its width, `border_tail` primitives before the end. A repaint that rewrote
/// the node's primitives fails this, and the stale record is then ignored
/// instead of eating unrelated paint.
fn stroke_start(pd: &PaintData, decoration: &AppliedInteraction) -> Option<usize> {
    let start = pd
        .primitives
        .len()
        .checked_sub(decoration.border_tail + FOCUS_RING_PRIMS)?;
    pd.primitives[start..start + FOCUS_RING_PRIMS]
        .iter()
        .all(|primitive| {
            matches!(primitive, Primitive::StrokeRect { width, .. } if *width == decoration.border_width)
        })
        .then_some(start)
}

pub(crate) fn ordinary_primitive_end(runtime: &Runtime, id: WidgetId, pd: &PaintData) -> usize {
    runtime.with(|runtime| {
        let runtime = runtime.borrow();
        if let Some(tooltip) = runtime.hover_tooltips.get(id) {
            return tooltip.base_primitive_end.min(pd.primitives.len());
        }

        // Decorations stroke the end of their owner's primitives; the ordinary
        // paint stops where the earliest intact stroke starts.
        let mut end = pd.primitives.len();
        for decoration in [&runtime.ring, &runtime.active, &runtime.hover]
            .into_iter()
            .flatten()
        {
            if decoration.border_owner == Some(id) {
                if let Some(start) = stroke_start(pd, decoration) {
                    end = end.min(start);
                }
            }
        }
        end
    })
}

/// Indices of the hover/press/focus background washes currently inserted in
/// `id`'s paint (`usize::MAX` for none). They are decorations: bounds and
/// anchoring skip them, since a toggle's halo reaches past the control.
pub(crate) fn decoration_washes(runtime: &Runtime, id: WidgetId) -> [usize; 3] {
    runtime.with(|rt| {
        let rt = rt.borrow();
        let mut washes = [usize::MAX; 3];
        for (slot, decoration) in [&rt.ring, &rt.active, &rt.hover].into_iter().enumerate() {
            if let Some((owner, position)) = decoration.as_ref().and_then(|d| d.background) {
                if owner == id {
                    washes[slot] = position;
                }
            }
        }
        washes
    })
}

/// The bounding box of a node's current paint primitives — the rect the ring
/// hugs. Primitive bounds (not the layout rect) because a stretched layout box
/// can be wider than the pixels the widget actually painted.
pub fn paint_bounds(runtime: &Runtime, scene: &Scene, id: WidgetId) -> Option<Rect> {
    let pd = scene.paint(id)?;
    let primitive_end = ordinary_primitive_end(runtime, id, pd);
    // A hover or pressed wash is a decoration, not the widget: a toggle's
    // halo reaches past the control and must not widen the focus ring.
    let washes = decoration_washes(runtime, id);
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (index, prim) in pd.primitives[..primitive_end].iter().enumerate() {
        if washes.contains(&index) {
            continue;
        }
        let (x0, y0, x1, y1) = match *prim {
            Primitive::SolidRect { rect, .. }
            | Primitive::StrokeRect { rect, .. }
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

/// Appends one [`Primitive::StrokeRect`] band of `width` just inside `rect`
/// grown by `-inset` (a negative inset puts the ring outside `rect`). `radius`
/// is the corner radius of the shape being outlined; an outset ring adds the
/// outset to it, so the ring stays concentric with the control.
fn push_ring_stroke(
    pd: &mut PaintData,
    rect: Rect,
    inset: f32,
    radius: f32,
    width: f32,
    color: Color,
) {
    let rect = Rect::new(
        rect.x + inset,
        rect.y + inset,
        (rect.width - 2.0 * inset).max(0.0),
        (rect.height - 2.0 * inset).max(0.0),
    );
    pd.primitives.push(Primitive::StrokeRect {
        rect,
        color,
        corner_radius: (radius - inset).max(0.0),
        width,
    });
}

/// Corner radius of the surface a control's ring should follow: the
/// largest solid rect in `owner`'s own paint, skipping hover and press washes
/// (a toggle's halo is bigger than the control). A control with no surface
/// (text, a row of cells) gets `0`.
/// Base corner radius of a focus ring on a square tab or row.
const NAV_RING_RADIUS: f32 = 3.0;

fn surface_radius(runtime: &Runtime, scene: &Scene, owner: WidgetId) -> f32 {
    let Some(paint) = scene.paint(owner) else {
        return 0.0;
    };
    let end = ordinary_primitive_end(runtime, owner, paint);
    let washes = decoration_washes(runtime, owner);
    let mut best = (0.0_f32, 0.0_f32);
    for (index, primitive) in paint.primitives[..end].iter().enumerate() {
        if washes.contains(&index) {
            continue;
        }
        if let Primitive::SolidRect {
            rect,
            corner_radius,
            ..
        } = *primitive
        {
            let area = rect.width * rect.height;
            if area > best.0 {
                best = (area, corner_radius);
            }
        }
    }
    best.1
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
        strip_applied_interaction(runtime, scene, decoration, Layer::Ring)
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
    // The keyboard cursor in an open option list is the row hover wash, so
    // keys and pointer mark the row Enter picks the same way; the chosen
    // row keeps its tint and check mark.
    if scene.node(id).map(|node| node.kind) == Some(WidgetKind::DropdownOption) {
        style = theme.interaction_style(component, InteractionState::Hover);
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
    let Some(kind) = scene.node(id).map(|node| node.kind) else {
        return false;
    };
    // A field shows its ring however it got focus, as CSS :focus-visible
    // does for text inputs: typing goes there next, whatever moved focus.
    let visible = visible || matches!(kind, WidgetKind::TextInput | WidgetKind::TextArea);
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

/// Re-applies hover, press and focus after `id`'s paint was **cleared and
/// refilled** from scratch (checkbox / switch / radio toggle, slider adjust).
///
/// The refill already dropped every primitive the decorations had added to
/// `id`, so their records for `id` point at the new paint. Stripping them the
/// usual way would delete whatever now sits at those indices: a hovered,
/// pressed slider lost its rail and fill on the first scrub step. This forgets
/// those records without touching the paint, then lays the decorations again.
pub(crate) fn redecorate_after_refill(runtime: &Runtime, scene: &mut Scene, id: WidgetId) {
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let rt = &mut *rt;
        for decoration in [&mut rt.ring, &mut rt.active, &mut rt.hover]
            .into_iter()
            .flatten()
        {
            decoration.foreground.retain(|restore| restore.owner != id);
            if decoration.border_owner == Some(id) {
                decoration.border_owner = None;
                decoration.border_tail = 0;
            }
            if decoration.background.is_some_and(|(owner, _)| owner == id) {
                decoration.background = None;
            }
        }
    });
    reapply_focus_ring(runtime, scene, id);
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

/// A background was inserted (`delta` 1) or removed (-1) in `owner` at `at`:
/// moves the recorded colour restores and washes of the decorations still
/// applied. Strokes are tracked from the end of the list and are unaffected.
///
/// Decorations do not always come off in the order they went on: a key can
/// take hover away from under a held press. The press wash then sits one
/// primitive lower, and its record has to follow it.
fn shift_restores(runtime: &Runtime, owner: WidgetId, at: usize, delta: isize) {
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let rt = &mut *rt;
        for decoration in [&mut rt.ring, &mut rt.active, &mut rt.hover]
            .into_iter()
            .flatten()
        {
            for restore in &mut decoration.foreground {
                if restore.owner == owner && restore.index >= at {
                    restore.index = restore.index.saturating_add_signed(delta);
                }
            }
            if let Some((wash_owner, position)) = &mut decoration.background {
                if *wash_owner == owner && *position >= at {
                    *position = position.saturating_add_signed(delta);
                }
            }
        }
    });
}

/// Adjusts the stroke tails of the decorations still applied to `owner` when a
/// stroke is stacked on the end of it (`removed_tail` is `None`: every existing
/// stroke gains four primitives after it) or taken out of the middle (strokes
/// that were below it, with a longer tail, lose four).
fn retail_strokes(runtime: &Runtime, owner: WidgetId, removed_tail: Option<usize>) {
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let rt = &mut *rt;
        for decoration in [&mut rt.ring, &mut rt.active, &mut rt.hover]
            .into_iter()
            .flatten()
        {
            if decoration.border_owner != Some(owner) {
                continue;
            }
            match removed_tail {
                None => decoration.border_tail += FOCUS_RING_PRIMS,
                Some(tail) if decoration.border_tail > tail => {
                    decoration.border_tail -= FOCUS_RING_PRIMS;
                }
                Some(_) => {}
            }
        }
    });
}

/// Where a decoration sits in the stack its colours compose in: hover at the
/// bottom, press above it, the focus ring on top. Every path that lays a
/// decoration first lifts the ones above it, so this order always holds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Layer {
    Hover,
    Press,
    Ring,
}

/// Hands `restore`'s colour to the lowest decoration above `layer` that
/// recoloured the same primitive. That decoration recorded `layer`'s colour as
/// the one to put back, which is wrong once `layer` is gone. Returns whether a
/// decoration above took it, in which case the paint keeps the colour shown.
fn pass_restore_up(runtime: &Runtime, layer: Layer, restore: &PrimitiveColorRestore) -> bool {
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let rt = &mut *rt;
        for (above, decoration) in [(Layer::Press, &mut rt.active), (Layer::Ring, &mut rt.ring)] {
            if above <= layer {
                continue;
            }
            let Some(decoration) = decoration else {
                continue;
            };
            if let Some(upper) = decoration
                .foreground
                .iter_mut()
                .find(|upper| upper.owner == restore.owner && upper.index == restore.index)
            {
                upper.color = restore.color;
                return true;
            }
        }
        false
    })
}

fn strip_applied_interaction(
    runtime: &Runtime,
    scene: &mut Scene,
    mut decoration: AppliedInteraction,
    layer: Layer,
) -> bool {
    for restore in &decoration.foreground {
        // A key can take hover away from under a held press. The press
        // recoloured on top of hover, so it keeps showing; it must put back
        // the colour from before hover when it comes off.
        if pass_restore_up(runtime, layer, restore) {
            continue;
        }
        if let Some(primitive) = scene
            .paint_mut(restore.owner)
            .primitives
            .get_mut(restore.index)
        {
            match primitive {
                Primitive::GlyphQuad { color, .. }
                | Primitive::Line { color, .. }
                | Primitive::StrokeRect { color, .. } => {
                    *color = restore.color;
                }
                Primitive::ImageQuad { tint, .. } => *tint = restore.color,
                // Only a link's underline is recorded as a rect.
                Primitive::SolidRect { color, .. } => *color = restore.color,
            }
            scene.mark_dirty(restore.owner, DirtyFlags::PAINT);
        }
    }
    if decoration.foreground.capacity() > 0 {
        decoration.foreground.clear();
        let buffer = std::mem::take(&mut decoration.foreground);
        runtime.with(|rt| rt.borrow_mut().restore_pool.push(buffer));
    }
    if let Some(owner) = decoration.border_owner {
        if let Some(start) = scene
            .paint(owner)
            .and_then(|pd| stroke_start(pd, &decoration))
        {
            scene
                .paint_mut(owner)
                .primitives
                .drain(start..start + FOCUS_RING_PRIMS);
            retail_strokes(runtime, owner, Some(decoration.border_tail));
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
            shift_restores(runtime, owner, position, -1);
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

/// Recolours the foreground primitives of `id` and its descendants to `next`,
/// recording each original colour so the decoration can be undone.
fn recolor_foreground(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    next: Color,
    restores: &mut Vec<PrimitiveColorRestore>,
) {
    let end = scene
        .paint(id)
        .map_or(0, |paint| ordinary_primitive_end(runtime, id, paint));
    let before = restores.len();
    // A link's underline is a rect drawn in its ink, so it follows the text.
    let link = scene
        .node(id)
        .is_some_and(|node| node.kind == WidgetKind::Link);
    for (index, primitive) in scene.paint_mut(id).primitives[..end].iter_mut().enumerate() {
        let color = match primitive {
            Primitive::GlyphQuad { color, .. } | Primitive::Line { color, .. } => color,
            Primitive::ImageQuad { tint, .. } => tint,
            Primitive::SolidRect { color, .. } if link => color,
            Primitive::SolidRect { .. } | Primitive::StrokeRect { .. } => continue,
        };
        restores.push(PrimitiveColorRestore {
            owner: id,
            index,
            color: *color,
        });
        *color = next;
    }
    if restores.len() > before {
        scene.mark_dirty(id, DirtyFlags::PAINT);
    }
    // Children are copied out one at a time by index: the borrow of the scene
    // ends before each recursive call, and no list of the subtree is built.
    let mut child = 0;
    while let Some(next_child) = scene
        .node(id)
        .and_then(|node| node.children.get(child).copied())
    {
        recolor_foreground(runtime, scene, next_child, next, restores);
        child += 1;
    }
}

fn apply_interaction_style(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    style: InteractionStyle,
    border_width: f32,
) -> Option<AppliedInteraction> {
    if style == InteractionStyle::NONE {
        return None;
    }
    let (rect, owner) = decoration_geometry(runtime, scene, id)?;
    let component = interaction_component(scene, id);
    let kind = scene.node(id).map(|node| node.kind);

    let background = style.background.map(|color| {
        let shape = theme_for(runtime, id).shape;
        // A toggle is too small to tint in place: its wash is a soft halo
        // behind the control, round for the round ones.
        let halo = component == Some(InteractionComponent::Toggle);
        let (wash, corner_radius) = if halo {
            let wash = Rect::new(
                rect.x - TOGGLE_HALO,
                rect.y - TOGGLE_HALO,
                rect.width + 2.0 * TOGGLE_HALO,
                rect.height + 2.0 * TOGGLE_HALO,
            );
            let radius = match kind {
                Some(WidgetKind::Checkbox) => shape.radius(6.0, wash.height),
                _ => shape.pill(wash.height),
            };
            (wash, radius)
        } else {
            (rect, shape.radius(3.0, rect.height))
        };
        // A row that paints nothing itself (a table row: its cells paint) is
        // washed on top of its last painted cell, since a wash under the
        // cells' opaque fills would not show.
        let own_end = scene
            .paint(id)
            .map_or(0, |paint| ordinary_primitive_end(runtime, id, paint));
        let on_top = !halo && own_end == 0;
        let id = if on_top { owner } else { id };
        let end = scene
            .paint(id)
            .map_or(0, |paint| ordinary_primitive_end(runtime, id, paint));
        let position = if halo {
            0
        } else if on_top {
            end
        } else {
            scene.paint(id).map_or(0, |paint| {
                paint.primitives[..end]
                    .iter()
                    .position(|primitive| {
                        matches!(
                            primitive,
                            Primitive::GlyphQuad { .. } | Primitive::ImageQuad { .. }
                        )
                    })
                    .unwrap_or(end)
            })
        };
        // Weigh the wash against what it covers: the widget's own opaque
        // fill under it, else the page.
        let backdrop = scene
            .paint(id)
            .and_then(|paint| {
                let (cx, cy) = (wash.x + wash.width * 0.5, wash.y + wash.height * 0.5);
                paint.primitives[..position]
                    .iter()
                    .rev()
                    .find_map(|p| match *p {
                        Primitive::SolidRect { rect, color, .. }
                            if color.a == 255
                                && rect.x <= cx
                                && cx <= rect.x + rect.width
                                && rect.y <= cy
                                && cy <= rect.y + rect.height =>
                        {
                            Some(color)
                        }
                        _ => None,
                    })
            })
            .unwrap_or_else(|| theme_for(runtime, id).page);
        let color = crate::widgets::theme::wash_over(color, backdrop);
        let rect = wash;
        // The new rect shifts the later primitives of this node, including the
        // colour restores of decorations already applied.
        shift_restores(runtime, id, position, 1);
        scene.paint_mut(id).primitives.insert(
            position,
            Primitive::SolidRect {
                rect,
                color,
                corner_radius,
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
        foreground = runtime
            .with(|rt| rt.borrow_mut().restore_pool.pop())
            .unwrap_or_default();
        recolor_foreground(runtime, scene, id, next, &mut foreground);
    }

    let border_owner = style.border.map(|border| {
        // Strokes already on this owner now have the new one after them.
        retail_strokes(runtime, owner, None);
        // A free-standing control or a link wears its focus ring just outside
        // its edge. Rows, tabs and options sit flush against their
        // neighbours, which would paint over an outset ring, so theirs stays
        // inside. A field's ring replaces its border, so it is not doubled.
        let outset = border_width == FOCUS_RING_W
            && (matches!(
                component,
                Some(InteractionComponent::Button | InteractionComponent::Toggle)
            ) || kind == Some(WidgetKind::Link));
        let inset = if outset {
            -(FOCUS_RING_W + FOCUS_RING_GAP)
        } else {
            0.0
        };
        // A slider rings its thumb, the part the arrows move, rather than a
        // box around the whole rail.
        let thumb = if kind == Some(WidgetKind::Slider) {
            scene.paint(owner).and_then(|paint| {
                let end = ordinary_primitive_end(runtime, owner, paint);
                paint.primitives[..end].iter().rev().find_map(|p| match *p {
                    Primitive::SolidRect {
                        rect,
                        corner_radius,
                        ..
                    } if rect.width > 0.0 && (rect.width - rect.height).abs() < 0.5 => {
                        Some((rect, corner_radius))
                    }
                    _ => None,
                })
            })
        } else {
            None
        };
        let (rect, radius) = match thumb {
            Some(thumb) => thumb,
            None if kind == Some(WidgetKind::Link) => (rect, LINK_RING_RADIUS),
            None => (rect, surface_radius(runtime, scene, owner)),
        };
        // A square tab or row still takes the design's small rounding on
        // its ring, so focus has one shape language across controls.
        let radius = if radius == 0.0 && component == Some(InteractionComponent::Navigation) {
            theme_for(runtime, id)
                .shape
                .radius(NAV_RING_RADIUS, rect.height)
        } else {
            radius
        };
        push_ring_stroke(
            scene.paint_mut(owner),
            rect,
            inset,
            radius,
            border_width,
            border,
        );
        scene.mark_dirty(owner, DirtyFlags::PAINT);
        owner
    });
    Some(AppliedInteraction {
        target: id,
        border_owner,
        border_tail: 0,
        border_width,
        foreground,
        background,
    })
}

pub fn strip_hover_decoration(runtime: &Runtime, scene: &mut Scene) -> bool {
    runtime
        .with(|rt| rt.borrow_mut().hover.take())
        .is_some_and(|decoration| {
            strip_applied_interaction(runtime, scene, decoration, Layer::Hover)
        })
}

fn apply_hover_decoration(runtime: &Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    strip_hover_decoration(runtime, scene);
    let Some(component) = interaction_component(scene, id) else {
        return false;
    };
    let mut style = interaction_style_for(runtime, id, component, InteractionState::Hover);
    // A link is text: a wash or box around one word reads as focus. It
    // answers hover by deepening its ink toward the body text instead.
    if scene
        .node(id)
        .is_some_and(|node| node.kind == WidgetKind::Link)
        && style.background.is_some()
    {
        let theme = theme_for(runtime, id);
        style = InteractionStyle::foreground(crate::widgets::theme::mix_color(
            theme.link_color(),
            theme.text,
            0.6,
        ));
    }
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
            strip_applied_interaction(runtime, scene, decoration, Layer::Press)
        })
}

fn apply_active_decoration(runtime: &Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    strip_active_decoration(runtime, scene);
    let Some(component) = interaction_component(scene, id) else {
        return false;
    };
    let style = interaction_style_for(runtime, id, component, InteractionState::Active);
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
                interaction_style_for(runtime, id, component, InteractionState::Active)
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
    // The ring is the outermost decoration, so it comes off and goes back on
    // only when it shares nodes with the press target being swapped.
    let focused = runtime
        .with(|rt| rt.borrow().ring.as_ref().map(|ring| ring.target))
        .filter(|&ring| {
            current.is_some_and(|old| shares_nodes(scene, ring, old))
                || target.is_some_and(|new| shares_nodes(scene, ring, new))
        });
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

    fn property_composite_state(
        scene: &Scene,
        id: WidgetId,
        mut offset: Point,
        clip: Option<Rect>,
    ) -> (Point, Option<Rect>) {
        let properties = scene.composite_properties(id);
        offset.x += properties.translation.x;
        offset.y += properties.translation.y;
        let clip = match (clip, properties.clip) {
            (Some(parent), Some(local)) => Some(parent.intersect(&translated_rect(local, offset))),
            (None, Some(local)) => Some(translated_rect(local, offset)),
            (parent, None) => parent,
        };
        (offset, clip)
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
        let (offset, clip) = property_composite_state(scene, id, offset, clip);
        if !scene.is_visible(id) || clip.is_some_and(|clip| !clip.contains(p)) {
            return None;
        }
        if skip_overlays && scene.is_overlay(id) {
            return None;
        }
        // Nothing in this subtree reaches the point: skip it whole.
        if !translated_rect(scene.subtree_bounds(id), offset).contains(p) {
            return None;
        }
        let node = scene.node(id)?;
        let (child_offset, child_clip) = child_composite_state(scene, id, offset, clip);
        // A wide parent narrows its children through the scene's spatial index
        // (top-most first) instead of testing each one.
        let mut candidates = SmallVec::<[WidgetId; 8]>::new();
        let local = Point {
            x: p.x - child_offset.x,
            y: p.y - child_offset.y,
        };
        if scene.child_hit_candidates(id, local, &mut candidates) {
            for c in candidates {
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
        } else {
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
        }
        // A DockArea is intentionally an implicit full-surface target. Its
        // ordinary descendants still win first (so tabs remain clickable and
        // directly droppable), but otherwise its empty pane surface accepts the
        // pointer instead of requiring a visible target control.
        if contains(scene, id, p, offset)
            && runtime.with(|rt| {
                rt.borrow()
                    .handlers
                    .get(id)
                    .is_some_and(|handlers| handlers.dock.is_some())
            })
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
    // Overlays are a sparse set, so this costs nothing in an overlay-less scene.
    // Each overlay's composite offset and clip come from its ancestor chain.
    fn overlay_hit(runtime: &Runtime, scene: &Scene, root: WidgetId, p: Point) -> Option<WidgetId> {
        let mut best: Option<((u8, u64), WidgetId)> = None;
        for id in scene.overlays() {
            let key = (scene.overlay_level(id), scene.overlay_order(id));
            if best.as_ref().is_some_and(|(current, _)| key <= *current) {
                continue;
            }
            // Ancestors, nearest first. An overlay nested inside another is
            // composited with it and is not a top-level layer; one detached from
            // the root is not drawn at all.
            let mut chain = SmallVec::<[WidgetId; 16]>::new();
            let mut current = scene.node(id).and_then(|node| node.parent);
            let mut nested = false;
            let mut attached = id == root;
            while let Some(ancestor) = current {
                if scene.is_overlay(ancestor) {
                    nested = true;
                    break;
                }
                chain.push(ancestor);
                if ancestor == root {
                    attached = true;
                    break;
                }
                current = scene.node(ancestor).and_then(|node| node.parent);
            }
            if nested || !attached || !scene.is_effectively_visible(id) {
                continue;
            }
            let mut offset = Point::default();
            let mut clip = None;
            for &ancestor in chain.iter().rev() {
                (offset, clip) = property_composite_state(scene, ancestor, offset, clip);
                (offset, clip) = child_composite_state(scene, ancestor, offset, clip);
            }
            if clip.is_some_and(|clip| !clip.contains(p)) {
                continue;
            }
            let Some(node) = scene.node(id) else { continue };
            let mut hit = nested_hit(runtime, scene, root, id, p)
                .or_else(|| leaf_hit(runtime, scene, id, p, false, offset, clip));
            // A modal dialog's layer is the backdrop target: it captures the
            // whole layer after every child has had first chance to hit.
            if hit.is_none()
                && node.kind == WidgetKind::DialogLayer
                && dialog::layer_is_modal(runtime, id)
                && scene.composited_rect(id).contains(p)
            {
                hit = Some(id);
            }
            if let Some(hit) = hit {
                best = Some((key, hit));
            }
        }
        best.map(|(_, hit)| hit)
    }
    // Overlays whose nearest overlay ancestor is `parent`, such as a dropdown
    // popup inside a dialog. The renderer draws each right after `parent`'s
    // layer, above the rest of `parent`, so they get the first chance to hit.
    // The highest `(level, order)` wins, and their own nested overlays first.
    fn nested_hit(
        runtime: &Runtime,
        scene: &Scene,
        root: WidgetId,
        parent: WidgetId,
        p: Point,
    ) -> Option<WidgetId> {
        let mut best: Option<((u8, u64), WidgetId)> = None;
        for id in scene.overlays() {
            if id == parent {
                continue;
            }
            let key = (scene.overlay_level(id), scene.overlay_order(id));
            if best.as_ref().is_some_and(|(current, _)| key <= *current) {
                continue;
            }
            // Every ancestor up to the root, nearest first.
            let mut chain = SmallVec::<[WidgetId; 16]>::new();
            let mut current = scene.node(id).and_then(|node| node.parent);
            let mut nearest = None;
            let mut attached = false;
            while let Some(ancestor) = current {
                if nearest.is_none() && scene.is_overlay(ancestor) {
                    nearest = Some(ancestor);
                }
                chain.push(ancestor);
                if ancestor == root {
                    attached = true;
                    break;
                }
                current = scene.node(ancestor).and_then(|node| node.parent);
            }
            if nearest != Some(parent) || !attached || !scene.is_effectively_visible(id) {
                continue;
            }
            let mut offset = Point::default();
            let mut clip = None;
            for &ancestor in chain.iter().rev() {
                (offset, clip) = property_composite_state(scene, ancestor, offset, clip);
                (offset, clip) = child_composite_state(scene, ancestor, offset, clip);
            }
            let hit = nested_hit(runtime, scene, root, id, p)
                .or_else(|| leaf_hit(runtime, scene, id, p, false, offset, clip));
            if let Some(hit) = hit {
                best = Some((key, hit));
            }
        }
        best.map(|(_, hit)| hit)
    }
    // A focus-grabbing modal owns the entire input plane even when a modeless
    // dialog was declared later. Search only its layer; a miss inside the layer
    // is the modal backdrop target. Every sibling and lower dialog is inert.
    if let Some(layer) = dialog::active_modal_layer(runtime, scene) {
        let root = scene.root()?;
        let hit = nested_hit(runtime, scene, root, layer, p)
            .or_else(|| leaf_hit(runtime, scene, layer, p, false, Point::default(), None));
        if let Some(hit) = hit {
            return Some(hit);
        }
        return scene.composited_rect(layer).contains(p).then_some(layer);
    }

    let root = scene.root()?;
    overlay_hit(runtime, scene, root, p)
        .or_else(|| leaf_hit(runtime, scene, root, p, true, Point::default(), None))
}

/// Resolves the native-style pointer cursor at a logical scene position.
///
/// Dialog chrome has priority over its child label nodes, and an active dialog
/// capture keeps its move/resize cursor even if the pointer leaves the chrome.
/// For ordinary content, semantic actions are followed through ancestors so a
/// table cell correctly inherits its selectable row's pointer cursor.
pub fn cursor_at(runtime: &Runtime, scene: &Scene, p: Point) -> CursorIcon {
    cursor_at_with(runtime, scene, p, || hit_test(runtime, scene, p))
}

/// [`cursor_at`] with the hit supplied by the caller, which may have it from the
/// same pointer event already. `hit` runs only if the cursor depends on it and
/// must return the result of [`hit_test`] at `p`.
pub fn cursor_at_with(
    runtime: &Runtime,
    scene: &Scene,
    p: Point,
    hit: impl FnOnce() -> Option<WidgetId>,
) -> CursorIcon {
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
    let Some(hit) = hit() else {
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

fn hover_target_at(runtime: &Runtime, scene: &Scene, hit: Option<WidgetId>) -> Option<WidgetId> {
    let mut current = hit;
    while let Some(id) = current {
        let semantics = scene.a11y(id);
        let disabled = semantics
            .map(|a| StateFlags(a.state).contains(StateFlags::DISABLED))
            .unwrap_or(false);
        if disabled {
            return None;
        }
        let paints_hover = interaction_component(scene, id).is_some_and(|component| {
            interaction_style_for(runtime, id, component, InteractionState::Hover)
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

/// Whether two decoration targets can touch the same nodes: a decoration stays
/// inside its target's subtree, so they can only if one target contains the other.
fn shares_nodes(scene: &Scene, a: WidgetId, b: WidgetId) -> bool {
    fn contains(scene: &Scene, ancestor: WidgetId, mut node: WidgetId) -> bool {
        loop {
            if node == ancestor {
                return true;
            }
            match scene.node(node).and_then(|node| node.parent) {
                Some(parent) => node = parent,
                None => return false,
            }
        }
    }
    contains(scene, a, b) || contains(scene, b, a)
}

fn update_control_hover(runtime: &Runtime, scene: &mut Scene, hit: Option<WidgetId>) -> bool {
    let next = hover_target_at(runtime, scene, hit);
    let current = runtime.with(|rt| rt.borrow().hover.as_ref().map(|hover| hover.target));
    if current == next {
        return false;
    }

    // Only the hover decoration changes. The press and focus decorations are
    // touched just when they share nodes with the old or new hover target, where
    // they have to come off and go back on above the new one (stack order:
    // hover, press, focus ring).
    let overlaps = |target: WidgetId| {
        current.is_some_and(|old| shares_nodes(scene, target, old))
            || next.is_some_and(|new| shares_nodes(scene, target, new))
    };
    let focused_ring = runtime
        .with(|rt| rt.borrow().ring.as_ref().map(|ring| ring.target))
        .filter(|&target| overlaps(target));
    let active = runtime
        .with(|rt| rt.borrow().active.as_ref().map(|active| active.target))
        .filter(|&target| overlaps(target));
    if let Some(id) = focused_ring {
        remove_focus_ring(runtime, scene, id);
    }
    if active.is_some() {
        strip_active_decoration(runtime, scene);
    }
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
    let hit = hit_test(runtime, scene, point);
    update_pointer_proximity_hit(runtime, scene, point, hit)
}

/// [`update_pointer_proximity`] for a caller that already resolved [`hit_test`] at
/// `point`. Hover decoration is paint-only, so the hit stays valid for the cursor
/// lookup that follows.
pub fn update_pointer_proximity_hit(
    runtime: &Runtime,
    scene: &mut Scene,
    point: Point,
    hit: Option<WidgetId>,
) -> bool {
    update_pointer_proximity_held(runtime, scene, point, hit, false)
}

/// [`update_pointer_proximity_hit`] for a pointer the keyboard has taken
/// hover from: with `held`, tooltips stay hidden under the resting pointer
/// too, so a label left by the mouse does not cover what the keys move to.
pub fn update_pointer_proximity_held(
    runtime: &Runtime,
    scene: &mut Scene,
    point: Point,
    hit: Option<WidgetId>,
    held: bool,
) -> bool {
    let control_changed = update_control_hover(runtime, scene, hit);
    let proximity_changed = runtime.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = &mut *registry;
        let captured_source = registry.drag_pointer.map(|capture| capture.source);
        let mut changed = false;
        for &id in &registry.proximity_ids {
            let Some(reveal) = registry.proximity_reveals.get_mut(id) else {
                continue;
            };
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
        for &id in &registry.tooltip_ids {
            let Some(tooltip) = registry.hover_tooltips.get_mut(id) else {
                continue;
            };
            let Some(rect) = scene.layout(id).map(|layout| layout.rect) else {
                continue;
            };
            changed |= show_tooltip(scene, id, tooltip, !held && rect.contains(point));
        }
        changed
    });
    control_changed | proximity_changed
}

/// Hides every shown hover tooltip. Returns whether paint changed.
pub fn hide_hover_tooltips(runtime: &Runtime, scene: &mut Scene) -> bool {
    runtime.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = &mut *registry;
        let mut changed = false;
        for &id in &registry.tooltip_ids {
            if let Some(tooltip) = registry.hover_tooltips.get_mut(id) {
                changed |= show_tooltip(scene, id, tooltip, false);
            }
        }
        changed
    })
}

/// Shows or hides one tooltip by recolouring its retained primitives.
fn show_tooltip(
    scene: &mut Scene,
    id: WidgetId,
    tooltip: &mut crate::widgets::HoverTooltipState,
    visible: bool,
) -> bool {
    if visible == tooltip.visible {
        return false;
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
    // Only the tooltip's host is repainted: it is an overlay whose
    // retained instances already cover the label past its layout box.
    scene.mark_dirty(id, DirtyFlags::PAINT);
    true
}
