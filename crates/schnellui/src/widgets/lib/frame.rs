use super::*;

pub fn measure_text(
    runtime: &Runtime,
    id: WidgetId,
    avail: Size,
    shaper: &mut TextShaper,
) -> Option<Size> {
    // Rich text views flow through their own width-aware measure (SOUL §8.1),
    // sharing this single hook so the umbrella threads exactly one DynMeasure.
    if let Some(sz) = rich::measure_rich(runtime, id, avail, shaper) {
        return Some(sz);
    }
    if let Some(sz) = text_area::measure_wrapped_area(runtime, id, avail, shaper) {
        return Some(sz);
    }
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let tl = rt.text_layouts.get_mut(id)?;
        let w = avail.width;
        if let Some(sz) = tl.cache.get(w) {
            return Some(sz);
        }
        let inv = 1.0 / norm_scale(tl.scale);
        let shaped = tl.shape(shaper, w);
        let sz = Size {
            width: shaped.width * inv,
            height: shaped.height * inv,
        };
        // Bounded ring: distinct widths (a resize sweep) overwrite the oldest
        // rather than growing without limit; steady-state same-width probes hit.
        tl.cache.insert(w, sz);
        Some(sz)
    })
}

/// The post-layout paint pass for wrapping text leaves (SOUL §8.1). Runs right after
/// [`LayoutEngine::compute`](schnellui::layout::LayoutEngine::compute) each layout-dirty
/// frame: for every registered [`TextLayout`] whose laid-out box changed (or whose
/// text changed), it re-shapes at the box width and re-emits the multi-line glyph
/// quads at their **absolute** aligned positions via [`rasterize_lines_and_push`],
/// then flags the node paint-dirty.
///
/// **Idempotent + proportional (Directive #3):** a node whose box and text are both
/// unchanged is skipped (no re-shape, no heap touch). Because the whole layout block
/// is itself skipped on a clean frame (nothing layout-dirty), a steady-state re-render
/// with wrapping text present but unchanged does **zero** work here (SOUL §1). The
/// glyphs are emitted at their final positions, so [`reposition_node`] deliberately
/// leaves these nodes untouched — sliding them would collapse the alignment offset.
pub fn emit_wrapped_paint(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    // Snapshot ids into pooled scratch: no heap once it has grown, however many
    // wrapped sites the document holds (§4.4).
    let mut ids = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let mut ids = std::mem::take(&mut rt.layout_scratch);
        ids.extend(rt.text_layouts.keys());
        ids
    });
    for &id in &ids {
        if scene.node(id).is_none() {
            continue;
        }
        let rect = match scene.layout(id) {
            Some(b) if !b.rect.is_empty() => b.rect,
            _ => continue,
        };
        // Resolve the scoped theme before mutably borrowing the widget runtime
        // below; `theme_for` reads the same registry.
        let text_color = theme_for(runtime, id).text;
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let Some(tl) = rt.text_layouts.get_mut(id) else {
                return;
            };
            if !tl.dirty && tl.last_emit == Some(rect) {
                return; // box and text unchanged ⇒ nothing to re-emit
            }
            let phys = phys_size_px(tl.size_px, tl.scale) as u32;
            let scale = tl.scale;
            // Shape at the laid-out box width (the wrap width the measure pass used).
            // The measure pass usually left this width's shape in the layout.
            let shaped = tl.shape(shaper, rect.width);
            let pd = scene.paint_mut(id);
            pd.primitives.clear();
            rasterize_lines_and_push(
                pd,
                shaper,
                atlas,
                shaped,
                phys,
                text_color,
                scale,
                Point {
                    x: rect.x,
                    y: rect.y,
                },
            );
            tl.last_emit = Some(rect);
            tl.dirty = false;
            scene.mark_dirty(id, DirtyFlags::PAINT);
        });
    }
    ids.clear();
    runtime.with(|rt| rt.borrow_mut().layout_scratch = ids);
    // Rich text views defer their paint the same way (measure during layout,
    // emit after), and a text area whose box was resized by an edit re-anchors
    // its paint into the new rect — one post-layout pass covers both (SOUL §8.1).
    rich::emit_rich_paint(runtime, scene, shaper, atlas);
    text_area::reemit_moved_areas(runtime, scene, shaper, atlas);
}

/// Re-emits text areas inside a Scroll whose emitted row band no longer covers
/// the rows the viewport shows. A scroll moves only the offset, so this runs on
/// every settled frame; areas still inside their band do no emission.
pub fn emit_scrolled_text_areas(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    text_area::reemit_scrolled_areas(runtime, scene, shaper, atlas);
}

/// Re-emits terminal grids whose retained model changed.
///
/// Terminal output is paint-only unless its row or column count changes, so this
/// pass must run independently of the layout-only deferred paint above.
pub fn emit_terminal_grid_paint(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    terminal_grid::emit_terminal_grids(runtime, scene, shaper, atlas);
}

/// Delivers ready dynamic Text/RichText/TerminalGrid bindings and mutates their
/// retained nodes in place. Text changes flag **paint** + **a11y** (name/value is
/// semantic, §6.2); a measured-width change flags **layout** as well. This is the
/// widgets-side "pull" the paint pass drives; work is proportional to affected,
/// rather than registered, dynamic sites.
///
/// The producer is taken out of the registry before it runs, so no registry borrow
/// is held across user code (§3.1 discipline). Re-shaping runs through the pooled
/// context (amortized zero once warm — the budgeted `text_edit` path, §4.1).
pub fn run_dynamic_slots(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    // Signal-core has already identified precisely which tracked producers may
    // need work. Drain its per-runtime queue instead of scanning every dynamic
    // text/rich/grid site after every unrelated signal write.
    let ids = runtime.take_ready_dynamic_ids();
    for raw_id in &ids {
        let id = WidgetId::from(slotmap::KeyData::from_ffi(*raw_id));
        if scene.node(id).is_none() {
            continue;
        }
        let taken = runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            rt.slots.get_mut(id).and_then(|s| {
                s.f.take().map(|f| {
                    (
                        f,
                        std::mem::take(&mut s.buf),
                        s.shared.clone(),
                        s.size_px,
                        s.role,
                        s.scale,
                        s.wrapped,
                    )
                })
            })
        });
        let Some((mut f, mut cur, shared, size_px, role, scale, wrapped)) = taken else {
            if rich::poll_dynamic_source(runtime, scene, id) {
                continue;
            }
            let _ = terminal_grid::poll_dynamic_source(runtime, scene, id);
            continue;
        };
        // Run and re-track with no registry borrow held (§3.1).
        // The producer appends into the slot's reused buffer; the comparison
        // with the last value is by borrow, so an unchanged slot allocates nothing.
        cur.clear();
        runtime.track_dynamic(id, || f(&mut cur));
        let changed = runtime.with(|rt| rt.borrow().slots.get(id).is_none_or(|s| s.last != cur));
        if changed && wrapped {
            // A wrapping/aligned text slot (SOUL §8.1): its paint is owned by
            // `emit_wrapped_paint` and its height depends on the wrap width, so we
            // don't shape here. Update the retained text + a11y and flag the channels;
            // the frame's layout pass re-measures (line count / height may change) and
            // `emit_wrapped_paint` re-emits the glyphs at the laid-out width. Invalidate
            // the measure cache so the next measure re-shapes the new text.
            {
                // Reuse the node's existing a11y string capacity.
                let a = scene.a11y_text_mut(id);
                let slot = if role == Role::Status {
                    &mut a.value
                } else {
                    &mut a.name
                };
                let dst = slot.get_or_insert_with(String::new);
                dst.clear();
                dst.push_str(&cur);
            }
            runtime.with(|rt| {
                let mut rt = rt.borrow_mut();
                if let Some(tl) = rt.text_layouts.get_mut(id) {
                    tl.text.clear();
                    tl.text.push_str(&cur);
                    tl.text_changed();
                }
            });
            scene.mark_dirty(id, DirtyFlags::PAINT);
            scene.mark_dirty(id, DirtyFlags::A11Y);
            // Wrapped text height depends on content, so always re-measure (LAYOUT).
            scene.mark_dirty(id, DirtyFlags::LAYOUT);
        } else if changed {
            {
                // Reuse the node's existing a11y string capacity.
                let a = scene.a11y_text_mut(id);
                let slot = if role == Role::Status {
                    &mut a.value
                } else {
                    &mut a.name
                };
                let dst = slot.get_or_insert_with(String::new);
                dst.clear();
                dst.push_str(&cur);
            }
            // Re-shape + re-emit real glyph quads for the new text (SOUL §8.1). The
            // re-emit places glyphs at a provisional local origin, so anchor THIS one
            // node onto its laid-out origin immediately — the frame's `reposition_paint`
            // pass only runs when the frame is layout-dirty, and a value-only change of
            // the *same shaped width* is not (no relayout). Without this the fresh
            // glyphs stay stranded at the origin and paint over the title at the
            // top-left (the reported windowed corruption). Proportional to this one
            // changed slot (Directive #3), zero-alloc (slides rects in place). If the
            // width *did* change, LAYOUT is flagged below and the frame's relayout +
            // `reposition_paint` re-anchor from here — idempotent, never double-applied.
            let new_sz = emit_text_paint(
                scene,
                shaper,
                atlas,
                id,
                &cur,
                size_px,
                theme_for(runtime, id).text,
                scale,
            );
            reposition_node(runtime, scene, id);
            let old_sz = *shared.borrow();
            *shared.borrow_mut() = new_sz;
            scene.mark_dirty(id, DirtyFlags::PAINT);
            scene.mark_dirty(id, DirtyFlags::A11Y);
            // Only a *measured-width* change touches the layout channel (§8.1).
            if (old_sz.width - new_sz.width).abs() > 0.001 {
                scene.mark_dirty(id, DirtyFlags::LAYOUT);
            }
        }
        // return the producer, advance last on change
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            if let Some(s) = rt.slots.get_mut(id) {
                s.f = Some(f);
                if changed {
                    // Swap rather than clone: the old `last` becomes next run's buffer.
                    std::mem::swap(&mut s.last, &mut cur);
                }
                s.buf = cur;
            }
        });
    }
    runtime.return_ready_dynamic_ids(ids);
    // Versioned terminal grids are externally clocked (for example by a PTY),
    // so they retain their independent revision polling path.
    terminal_grid::poll_dynamic_sources(runtime, scene);
}

/// Refreshes versioned image sources directly in their retained atlas regions.
/// This is intentionally independent of the signal-version gate: embedders such
/// as Servo publish frames from their own event loop.
pub fn poll_dynamic_images(runtime: &Runtime, scene: &mut Scene) {
    let mut ids = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let mut ids = std::mem::take(&mut rt.dynamic_image_scratch);
        ids.extend_from_slice(&rt.dynamic_image_ids);
        ids
    });
    for &id in &ids {
        if scene.node(id).is_none() {
            continue;
        }
        let source = runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let state = rt.dynamic_images.get_mut(id)?;
            let current_revision = (state.revision)();
            if current_revision == state.observed_revision {
                return None;
            }
            Some((
                current_revision,
                state.frame.take()?,
                state.display,
                state.texels,
            ))
        });
        let Some((current_revision, mut frame_source, display, previous_texels)) = source else {
            continue;
        };
        let frame = frame_source();
        let mut next_texels = previous_texels;
        let updated = frame.as_ref().is_some_and(|frame| {
            if frame.width == 0
                || frame.height == 0
                || frame.pixels.len() < frame.width as usize * frame.height as usize * 4
            {
                return false;
            }
            let fits = |allocation: TexelRect| {
                frame.width <= allocation.width && frame.height <= allocation.height
            };
            let allocation = match previous_texels {
                // Steady state: same-size (or smaller) frames overwrite the
                // existing region. Keep the largest allocation as capacity, while
                // sampling only the current frame, so shrink/grow oscillation
                // never leaks atlas shelves.
                Some(allocation) if fits(allocation) => allocation,
                // A larger frame needs a new region; the old one goes back to the
                // atlas below. Grow by at least a quarter in each direction, so a
                // panel dragged wider re-allocates a handful of times instead of
                // once per pixel. If the atlas is exhausted (or the frame exceeds
                // `ImageAtlas::MAX_DIM`), fall back to the largest region we can
                // get and show the frame's top-left crop rather than freezing.
                previous => {
                    let max = schnellui::scene::ImageAtlas::MAX_DIM;
                    let (want_w, want_h) = match previous {
                        Some(old) => (
                            frame.width.max(old.width + old.width / 4),
                            frame.height.max(old.height + old.height / 4),
                        ),
                        None => (frame.width, frame.height),
                    };
                    let exact = (frame.width.min(max), frame.height.min(max));
                    let images = scene.images_mut();
                    match images
                        .reserve(want_w.min(max), want_h.min(max))
                        .or_else(|| images.reserve(exact.0, exact.1))
                        .or(previous)
                    {
                        Some(allocation) => allocation,
                        None => return false,
                    }
                }
            };
            let used = TexelRect {
                x: allocation.x,
                y: allocation.y,
                width: frame.width.min(allocation.width),
                height: frame.height.min(allocation.height),
            };
            if !scene
                .images_mut()
                .write_rect_strided(used, &frame.pixels, frame.width)
            {
                return false;
            }
            replace_dynamic_image_quad(runtime, scene, id, used, display);
            if let Some(previous) = previous_texels.filter(|previous| *previous != allocation) {
                scene.images_mut().free(previous);
            }
            next_texels = Some(allocation);
            true
        });
        runtime.with(|rt| {
            if let Some(state) = rt.borrow_mut().dynamic_images.get_mut(id) {
                state.frame = Some(frame_source);
                if updated {
                    state.observed_revision = current_revision;
                    state.texels = next_texels;
                }
            }
        });
        if updated {
            scene.mark_dirty(id, DirtyFlags::PAINT);
        }
    }
    ids.clear();
    runtime.with(|rt| rt.borrow_mut().dynamic_image_scratch = ids);
}

/// Replaces a dynamic image's ordinary paint while retaining any tooltip and
/// interaction decorations appended after it. Dynamic images can begin as a
/// placeholder and receive their first atlas allocation after layout, so using
/// `push_image_quad` here would clear that retained tail and leave the tooltip's
/// primitive indices dangling.
fn replace_dynamic_image_quad(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    texels: TexelRect,
    display: Size,
) {
    let rect = node_rect(scene, id, display);
    let image = Primitive::ImageQuad {
        rect,
        atlas_uv: Rect::new(
            texels.x as f32,
            texels.y as f32,
            texels.width as f32,
            texels.height as f32,
        ),
        tint: Color::WHITE,
    };
    let tooltip = runtime.with(|rt| rt.borrow().hover_tooltips.get(id).copied());
    let mut adjusted_tooltip = None;
    let mut invalid_tooltip = false;
    {
        let paint = scene.paint_mut(id);
        if let Some(mut tooltip) = tooltip {
            let valid = tooltip.base_primitive_end <= paint.primitives.len()
                && tooltip.background >= tooltip.base_primitive_end
                && tooltip.background < paint.primitives.len()
                && tooltip.glyph_start > tooltip.background
                && tooltip.glyph_start <= tooltip.glyph_end
                && tooltip.glyph_end <= paint.primitives.len();
            if valid {
                let old_base_end = tooltip.base_primitive_end;
                paint
                    .primitives
                    .splice(..old_base_end, std::iter::once(image));
                let remap = |index: usize| index - old_base_end + 1;
                tooltip.base_primitive_end = 1;
                tooltip.background = remap(tooltip.background);
                tooltip.glyph_start = remap(tooltip.glyph_start);
                tooltip.glyph_end = remap(tooltip.glyph_end);
                adjusted_tooltip = Some(tooltip);
            } else {
                paint.primitives.clear();
                paint.primitives.push(image);
                invalid_tooltip = true;
            }
        } else {
            paint.primitives.clear();
            paint.primitives.push(image);
        }
    }
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        if let Some(tooltip) = adjusted_tooltip {
            rt.hover_tooltips.insert(id, tooltip);
        } else if invalid_tooltip {
            rt.hover_tooltips.remove(id);
            rt.tooltip_ids.retain(|tooltip| *tooltip != id);
        }
    });
}

/// Anchors every node's paint fragments to its computed [`LayoutBox`] origin
/// (SOUL §8.1 pass order — paint reads geometry *after* layout runs). Widgets emit
/// paint during `build`, before the first layout pass, so their primitives sit at a
/// provisional origin; once Taffy has written absolute rects this pass slides each
/// node's whole primitive set by the delta between its current min-origin and its
/// laid-out origin, preserving intra-node offsets (a button's label keeps its pad).
///
/// Driven by [`Scene::geometry_changed`], which the pass drains.
///
/// **Idempotent and zero-alloc in steady state:** after the first slide a node's
/// min-origin already equals its layout origin, so the delta is zero and nothing is
/// written — a clean re-frame touches no memory (Directive #1). Only nodes whose
/// layout rect actually moved get repositioned, and only the primitive `rect`
/// fields are mutated in place (no `Vec` growth).
pub fn reposition_paint(runtime: &Runtime, scene: &mut Scene) {
    // Only nodes whose geometry changed need anchoring, so the pass costs what moved
    // rather than the tree size. `reposition_node` may add entries (a re-emit that
    // writes geometry), so walk by index and re-read the length.
    let mut scroll_parents: SmallVec<[WidgetId; 8]> = SmallVec::new();
    let mut i = 0;
    while i < scene.geometry_changed().len() {
        let id = scene.geometry_changed()[i];
        i += 1;
        reposition_node(runtime, scene, id);
        // A scroll viewport's scrollbar is derived from its children's extent, so a
        // moved or resized child re-anchors the viewport once per pass.
        let Some(parent) = scene.node(id).and_then(|node| node.parent) else {
            continue;
        };
        if scene
            .node(parent)
            .is_some_and(|node| node.kind == WidgetKind::Scroll)
            && !scroll_parents.contains(&parent)
        {
            if scroll_parents.len() < scroll_parents.inline_size() {
                scroll_parents.push(parent);
            }
            reposition_node(runtime, scene, parent);
        }
    }
    scene.clear_geometry_changed();
}

/// Slides a **single** node's paint fragments so their min-origin lands on that node's
/// laid-out [`LayoutBox`] origin (the per-node core of [`reposition_paint`], SOUL
/// §8.1). A no-op — and never a heap touch — when the node has no laid-out box yet
/// (mount, before the first layout pass), has no primitives, or is already anchored
/// (idempotent, the steady-state zero-alloc case, Directive #1). Only the primitive
/// `rect` fields are mutated in place. Exposed to [`run_dynamic_slots`] so a value-only
/// re-emit (which does not flag the layout channel, hence skips the frame's full
/// `reposition_paint`) can still anchor exactly the one node it touched.
pub fn reposition_node(runtime: &Runtime, scene: &mut Scene, id: WidgetId) {
    // Scrollbar geometry depends on both the final viewport and content extent.
    if scene
        .node(id)
        .is_some_and(|node| node.kind == WidgetKind::Scroll)
        && emit_scrollbar_paint(runtime, scene, id)
    {
        return;
    }
    // Content-sized panels derive their paint from their final child-driven box.
    if panel::reposition(runtime, scene, id) {
        return;
    }
    // Dialog scrims and surfaces derive both origin and size from their final
    // layout boxes, so they own a post-layout re-emit rather than a translation.
    if dialog::reposition(runtime, scene, id) {
        return;
    }
    // Wrapping/aligned text owns its own absolute positioning (SOUL §8.1):
    // `emit_wrapped_paint` emits its glyphs at final aligned coordinates, so a
    // min-origin slide here would collapse the per-line alignment offset. Skip it.
    if runtime.with(|rt| {
        let rt = rt.borrow();
        // Rich text views likewise emit at absolute laid-out positions
        // (post-layout, SOUL §8.1) — a min-origin slide would double-shift.
        rt.text_layouts.contains_key(id)
            || rt.rich.contains_key(id)
            || terminal_grid::contains(runtime, id)
    }) {
        return;
    }
    let (target, lay_rect) = match scene.layout(id) {
        Some(b) if !b.rect.is_empty() => (
            Point {
                x: b.rect.x,
                y: b.rect.y,
            },
            b.rect,
        ),
        _ => return,
    };
    // A Divider is a *width-spanning* hairline (SOUL §8.1): its build-time paint
    // carries a provisional zero width (a leaf cannot know its span before layout),
    // so the generic translate-only path below would keep it invisible. Anchor it by
    // overwriting its rect with the laid-out box instead, keeping the emitted
    // thickness as the height — idempotent, in-place, no heap touch (Directive #1).
    // v0 special case until paint can subscribe to geometry (§3.2 property tree).
    if scene
        .node(id)
        .is_some_and(|n| n.kind == WidgetKind::Divider)
    {
        // Read first: `paint_mut` marks the node paint-dirty, so take it only when
        // the hairline actually needs to change.
        let new = match scene.paint(id).and_then(|pd| pd.primitives.first()) {
            Some(Primitive::SolidRect { rect, .. }) => {
                let new = Rect::new(lay_rect.x, lay_rect.y, lay_rect.width, rect.height);
                (*rect != new).then_some(new)
            }
            _ => None,
        };
        if let Some(new) = new {
            if let Some(Primitive::SolidRect { rect, .. }) =
                scene.paint_mut(id).primitives.first_mut()
            {
                *rect = new;
            }
        }
        return;
    }
    let Some(pd) = scene.paint(id) else { return };
    if pd.primitives.is_empty() {
        return;
    }
    // Current min-origin of this node's ordinary primitives. A tooltip may sit
    // above/left of its target; exclude that overflow so revealing it never
    // changes the anchor used to position the widget itself.
    let primitive_end = ordinary_primitive_end(runtime, id, pd);
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    for prim in &pd.primitives[..primitive_end] {
        let (px, py) = match *prim {
            Primitive::SolidRect { rect, .. } => (rect.x, rect.y),
            Primitive::GlyphQuad { rect, .. } => (rect.x, rect.y),
            Primitive::ImageQuad { rect, .. } => (rect.x, rect.y),
            // A line's top-left bound is min(from,to) inset by half its stroke width,
            // so the stroke's extent is included when anchoring (SOUL §3.2).
            Primitive::Line {
                from, to, width, ..
            } => (
                from.x.min(to.x) - width * 0.5,
                from.y.min(to.y) - width * 0.5,
            ),
        };
        min_x = min_x.min(px);
        min_y = min_y.min(py);
    }
    let dx = target.x - min_x;
    let dy = target.y - min_y;
    if dx.abs() >= 0.001 || dy.abs() >= 0.001 {
        let pd = scene.paint_mut(id);
        for prim in &mut pd.primitives {
            match prim {
                Primitive::SolidRect { rect, .. } => {
                    rect.x += dx;
                    rect.y += dy;
                }
                Primitive::GlyphQuad { rect, .. } => {
                    rect.x += dx;
                    rect.y += dy;
                }
                Primitive::ImageQuad { rect, .. } => {
                    rect.x += dx;
                    rect.y += dy;
                }
                // Slide both endpoints so the whole segment moves with the node (§3.2).
                Primitive::Line { from, to, .. } => {
                    from.x += dx;
                    from.y += dy;
                    to.x += dx;
                    to.y += dy;
                }
            }
        }
    }
    position_hover_tooltip(runtime, scene, id);
    selection::resize_tab_surface(runtime, scene, id);
}

/// Routes a click/activation to a widget's stored handler — the single inbound path
/// shared by pointer input and an AccessKit `Click` `ActionRequest` (SOUL §6.3).
/// Toggles a checkbox's own state (marking paint + a11y dirty) before firing its
/// `on_toggle`. Returns `true` if a target handler ran. A disabled widget is inert.
///
/// The handler is taken out of the registry before it runs, so user code (which may
/// re-enter to dispatch another widget) never executes under the registry borrow
/// (§3.1 discipline).
pub fn dispatch_click(runtime: &Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    let Some(node) = scene.node(id) else {
        return false;
    };
    let kind = node.kind;
    let disabled = scene
        .a11y(id)
        .map(|a| StateFlags(a.state).contains(StateFlags::DISABLED))
        .unwrap_or(false);
    if disabled {
        return false;
    }
    match kind {
        // A link activates exactly like a button: fire the stored click handler
        // (SOUL §6.3 — no state of its own to toggle).
        WidgetKind::Button | WidgetKind::Link => {
            let cb = runtime.with(|rt| {
                rt.borrow_mut()
                    .handlers
                    .get_mut(id)
                    .and_then(|h| h.click.take())
            });
            let Some(mut cb) = cb else { return false };
            cb();
            runtime.with(|rt| {
                if let Some(h) = rt.borrow_mut().handlers.get_mut(id) {
                    h.click = Some(cb);
                }
            });
            true
        }
        WidgetKind::Checkbox => {
            let new = {
                let a = scene.a11y_mut(id);
                let mut s = StateFlags(a.state);
                let now = !s.contains(StateFlags::CHECKED);
                if now {
                    s.insert(StateFlags::CHECKED);
                } else {
                    s.0 &= !StateFlags::CHECKED.0;
                }
                a.state = s.0;
                now
            };
            emit_checkbox_paint(runtime, scene, id, new);
            // The re-emit cleared the primitives; a focused checkbox keeps its ring.
            reapply_focus_ring(runtime, scene, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
            scene.mark_dirty(id, DirtyFlags::A11Y);
            let tog = runtime.with(|rt| {
                rt.borrow_mut()
                    .handlers
                    .get_mut(id)
                    .and_then(|h| h.toggle.take())
            });
            if let Some(mut t) = tog {
                t(new);
                runtime.with(|rt| {
                    if let Some(h) = rt.borrow_mut().handlers.get_mut(id) {
                        h.toggle = Some(t);
                    }
                });
            }
            true
        }
        // Basic-widget kinds (SOUL §8.1) dispatch through the hook in [`basic`] so
        // that module owns its widgets' toggle/select semantics end-to-end (§6.3).
        WidgetKind::Switch | WidgetKind::Radio => {
            basic::dispatch_click_basic(runtime, scene, id, kind)
        }
        // Selection kinds (tabs / list items) dispatch through the hook in
        // [`selection`] — sibling-exclusive selection, recolored in place (§6.3).
        WidgetKind::Tab | WidgetKind::ListItem => {
            selection::dispatch_click_selection(runtime, scene, id, kind)
        }
        // Dropdown kinds dispatch through the hook in [`selection`]: the trigger
        // fires its toggle handler (the host remounts with `open` flipped); an
        // option selects exclusively and mirrors its label into the trigger's
        // accessible value (§6.3).
        WidgetKind::Dropdown | WidgetKind::DropdownOption => {
            selection::dispatch_click_dropdown(runtime, scene, id, kind)
        }
        WidgetKind::TextInput
            if scene
                .a11y(id)
                .is_some_and(|a| Role::from_u16(a.role) == Role::ComboBox) =>
        {
            selection::dispatch_click_dropdown(runtime, scene, id, kind)
        }
        // Table kinds dispatch through the hook in [`table`]: a cell click bubbles
        // to its row; an AccessKit Click targets the row directly (§6.3).
        WidgetKind::TableRow | WidgetKind::TableCell => {
            table::dispatch_click_table(runtime, scene, id, kind)
        }
        // The dialog layer is returned by modal backdrop hit-testing. A
        // persistent layer still captures the pointer; it simply has no action.
        WidgetKind::DialogLayer => dialog::dispatch_backdrop(runtime, scene, id),
        _ => false,
    }
}
