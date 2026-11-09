//! Checks over the retained interaction records, for the fuzzing
//! harness in `schnellui::testing::fuzz`. They look at runtime state the
//! public API does not expose, so they live next to it.

use super::*;

/// Takes every decoration off and lays it again on the same targets, and
/// reports the first node whose paint differs. Decorations are sized from the
/// paint they sit on, so a difference means one went stale: it kept the size
/// or place of paint that has since moved or grown.
pub(crate) fn decoration_relay_diff(runtime: &Runtime, scene: &mut Scene) -> Option<String> {
    let (ring, active, hover) = runtime.with(|rt| {
        let rt = rt.borrow();
        (
            rt.ring.as_ref().map(|d| d.target),
            rt.active.as_ref().map(|d| d.target),
            rt.hover.as_ref().map(|d| d.target),
        )
    });
    if ring.is_none() && active.is_none() && hover.is_none() {
        return None;
    }
    let mut ids = Vec::new();
    let mut stack: Vec<WidgetId> = scene.root().into_iter().collect();
    while let Some(id) = stack.pop() {
        ids.push(id);
        if let Some(node) = scene.node(id) {
            stack.extend(node.children.iter().copied());
        }
    }
    let before: Vec<_> = ids
        .iter()
        .map(|id| scene.paint(*id).map(|p| p.primitives.clone()))
        .collect();
    if let Some(id) = ring {
        remove_focus_ring(runtime, scene, id);
    }
    strip_active_decoration(runtime, scene);
    strip_hover_decoration(runtime, scene);
    if let Some(id) = hover {
        apply_hover_decoration(runtime, scene, id);
    }
    if let Some(id) = active {
        apply_active_decoration(runtime, scene, id);
    }
    if let Some(id) = ring {
        apply_focus_ring(runtime, scene, id);
    }
    for (id, old) in ids.iter().zip(before) {
        let new = scene.paint(*id).map(|p| p.primitives.clone());
        // Geometry only: a wash is tinted against the fill under it, and a
        // selection that changes that fill while hovered retints on relay.
        let shape = |prims: &Vec<Primitive>| -> Vec<Primitive> {
            prims
                .iter()
                .map(|p| {
                    let mut p = *p;
                    match &mut p {
                        Primitive::SolidRect { color, .. }
                        | Primitive::StrokeRect { color, .. }
                        | Primitive::GlyphQuad { color, .. }
                        | Primitive::Line { color, .. } => *color = Color::TRANSPARENT,
                        Primitive::ImageQuad { tint, .. } => *tint = Color::TRANSPARENT,
                    }
                    p
                })
                .collect()
        };
        let same = match (&old, &new) {
            (Some(a), Some(b)) => shape(a) == shape(b),
            (None, Some(b)) | (Some(b), None) => b.is_empty(),
            (None, None) => true,
        };
        if !same {
            let old = old.unwrap_or_default();
            let new = new.unwrap_or_default();
            let mut lines = String::new();
            for i in 0..old.len().max(new.len()) {
                if old.get(i) != new.get(i) {
                    lines.push_str(&format!(
                        "  [{i}] shown   {:?}\n      relaid  {:?}\n",
                        old.get(i),
                        new.get(i)
                    ));
                }
            }
            return Some(format!(
                "{id:?} {:?} (ring {ring:?} press {active:?} hover {hover:?})\n{lines}",
                scene.node(*id).map(|n| n.kind)
            ));
        }
    }
    None
}

/// Checks that every applied hover, press and focus decoration still finds
/// its primitives: the wash is a solid rect, the stroke sits where its tail
/// says, and every recoloured primitive exists. A repaint that dropped a
/// decoration without forgetting its record fails here.
pub(crate) fn decoration_faults(runtime: &Runtime, scene: &Scene) -> Option<String> {
    runtime.with(|rt| {
        let rt = rt.borrow();
        for (name, decoration) in [("ring", &rt.ring), ("press", &rt.active), ("hover", &rt.hover)] {
            let Some(d) = decoration else { continue };
            if scene.node(d.target).is_none() {
                return Some(format!("{name} decorates dead node {:?}", d.target));
            }
            if let Some((owner, position)) = d.background {
                let ok = scene.paint(owner).is_some_and(|p| {
                    matches!(p.primitives.get(position), Some(Primitive::SolidRect { .. }))
                });
                if !ok {
                    return Some(format!(
                        "{name} wash of {:?} lost: {owner:?}[{position}] is {:?}",
                        d.target,
                        scene.paint(owner).and_then(|p| p.primitives.get(position))
                    ));
                }
            }
            if let Some(owner) = d.border_owner {
                let ok = scene.paint(owner).is_some_and(|p| {
                    p.primitives
                        .len()
                        .checked_sub(d.border_tail + FOCUS_RING_PRIMS)
                        .is_some_and(|start| {
                            matches!(p.primitives[start], Primitive::StrokeRect { width, .. } if width == d.border_width)
                        })
                });
                if !ok {
                    return Some(format!(
                        "{name} stroke of {:?} lost on {owner:?} (tail {})",
                        d.target, d.border_tail
                    ));
                }
            }
            for restore in &d.foreground {
                let ok = scene
                    .paint(restore.owner)
                    .is_some_and(|p| restore.index < p.primitives.len());
                if !ok {
                    return Some(format!(
                        "{name} recolour of {:?} points past {:?}[{}]",
                        d.target, restore.owner, restore.index
                    ));
                }
            }
        }
        None
    })
}

/// Visible tooltips, and floating layers such as dialogs, dropdown popups
/// and context menus, must sit inside the window (`view`) when they fit in it.
pub(crate) fn placement_faults(runtime: &Runtime, scene: &Scene, view: Rect) -> Option<String> {
    let root = scene.root()?;
    // Placement keeps a small margin from the window edge, so a layer only
    // counts as fitting when it fits with that margin on both sides.
    let room = 2.0 * TOOLTIP_VIEWPORT_MARGIN;
    let inside = |r: Rect| {
        (r.width > view.width - room || (r.x >= view.x - 0.5 && r.right() <= view.right() + 0.5))
            && (r.height > view.height - room
                || (r.y >= view.y - 0.5 && r.bottom() <= view.bottom() + 0.5))
    };
    let tooltips: Vec<(WidgetId, HoverTooltipState)> = runtime.with(|rt| {
        let rt = rt.borrow();
        rt.tooltip_ids
            .iter()
            .filter_map(|id| rt.hover_tooltips.get(*id).map(|t| (*id, *t)))
            .collect()
    });
    for (id, t) in tooltips {
        if !t.visible || scene.node(id).is_none() {
            continue;
        }
        if let Some(Primitive::SolidRect { rect, .. }) =
            scene.paint(id).and_then(|p| p.primitives.get(t.background))
        {
            if !inside(*rect) {
                return Some(format!(
                    "tooltip of {id:?} at {rect:?} leaves window {view:?}"
                ));
            }
        }
    }
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(node) = scene.node(id) else { continue };
        stack.extend(node.children.iter().copied());
        if id == root
            || !scene.is_overlay(id)
            || !scene.is_visible(id)
            || scene.overlay_level(id) == TOOLTIP_OVERLAY_LEVEL
        {
            continue;
        }
        let Some(rect) = scene.layout(id).map(|l| l.rect) else {
            continue;
        };
        if rect.width <= 0.0 || rect.height <= 0.0 {
            continue;
        }
        if !inside(rect) {
            return Some(format!(
                "overlay {id:?} {:?} {:?} at {rect:?} leaves window {view:?}",
                node.kind,
                scene.a11y(id).and_then(|a| a.name.clone())
            ));
        }
    }
    None
}
