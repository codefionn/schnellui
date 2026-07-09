use super::*;

#[derive(Clone, Copy)]
pub struct ScrollMetrics {
    pub viewport: Rect,
    pub content_height: f32,
    pub max_offset: f32,
    pub offset: f32,
}

pub fn scroll_metrics(scene: &Scene, id: WidgetId) -> Option<ScrollMetrics> {
    if !matches!(scene.node(id), Some(node) if node.kind == WidgetKind::Scroll) {
        return None;
    }
    let viewport = scene.layout(id)?.rect;
    if viewport.is_empty() {
        return None;
    }
    let mut max_bottom = viewport.y;
    for &child in &scene.node(id)?.children {
        if let Some(layout) = scene.layout(child) {
            max_bottom = max_bottom.max(layout.rect.bottom());
        }
    }
    let content_height = (max_bottom - viewport.y).max(0.0);
    let max_offset = (content_height - viewport.height).max(0.0);
    Some(ScrollMetrics {
        viewport,
        content_height,
        max_offset,
        offset: scene.scroll_offset(id).y.clamp(0.0, max_offset),
    })
}

pub fn scrollbar_rects(metrics: ScrollMetrics) -> Option<(Rect, Rect)> {
    if metrics.max_offset <= 0.0 || metrics.viewport.height <= 2.0 * SCROLLBAR_INSET {
        return None;
    }
    let track = Rect::new(
        metrics.viewport.right() - SCROLLBAR_WIDTH - SCROLLBAR_INSET,
        metrics.viewport.y + SCROLLBAR_INSET,
        SCROLLBAR_WIDTH,
        (metrics.viewport.height - 2.0 * SCROLLBAR_INSET).max(0.0),
    );
    let thumb_height = (track.height * metrics.viewport.height / metrics.content_height)
        .clamp(SCROLLBAR_MIN_THUMB.min(track.height), track.height);
    let travel = (track.height - thumb_height).max(0.0);
    let progress = if metrics.max_offset > 0.0 {
        metrics.offset / metrics.max_offset
    } else {
        0.0
    };
    let thumb = Rect::new(
        track.x,
        track.y + travel * progress,
        track.width,
        thumb_height,
    );
    Some((track, thumb))
}

/// Re-emits optional scroll chrome from final layout geometry. Scrollbar paint is
/// attached to the viewport node and composited after its descendants by the WGPU
/// renderer, so it stays fixed while content moves underneath it.
pub fn emit_scrollbar_paint(runtime: &Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    let enabled = runtime.with(|rt| {
        rt.borrow()
            .scrolls
            .get(id)
            .is_some_and(|state| state.scrollbar)
    });
    let rects = enabled
        .then(|| scroll_metrics(scene, id).and_then(scrollbar_rects))
        .flatten();
    let desired = rects.map(|(track, thumb)| {
        let theme = theme_for(runtime, id);
        [
            Primitive::SolidRect {
                rect: track,
                color: theme.surface_muted,
                corner_radius: theme.shape.pill(track.width),
            },
            Primitive::SolidRect {
                rect: thumb,
                color: theme.outline,
                corner_radius: theme.shape.pill(thumb.width),
            },
        ]
    });
    let changed = match (scene.paint(id), desired.as_ref()) {
        (None, None) => false,
        (Some(paint), None) => !paint.primitives.is_empty(),
        (Some(paint), Some(primitives)) => paint.primitives.as_slice() != primitives,
        (None, Some(_)) => true,
    };
    if !changed {
        return true;
    }
    let paint = scene.paint_mut(id);
    paint.primitives.clear();
    if let Some(primitives) = desired {
        paint.primitives.extend(primitives);
    }
    scene.mark_dirty(id, DirtyFlags::PAINT);
    true
}

fn set_scroll_position(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    new: f32,
    notify: bool,
) -> bool {
    set_scroll_position_at(runtime, scene, id, new, notify, Instant::now())
}

fn set_scroll_position_at(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    new: f32,
    notify: bool,
    now: Instant,
) -> bool {
    let current = scene.scroll_offset(id).y;
    if new == current {
        return false;
    }
    scene.set_scroll_offset(id, Point { x: 0.0, y: new });
    // Sub-pixel position changes can round to the already published semantic
    // value. Keep that no-op off the a11y dirty channel as well.
    if new.round() as i64 != current.round() as i64 {
        scene.set_a11y_value_i64(id, new.round() as i64);
    }
    emit_scrollbar_paint(runtime, scene, id);
    if notify {
        let callback = runtime.with(|rt| {
            rt.borrow_mut()
                .handlers
                .get_mut(id)
                .and_then(|handlers| handlers.scroll.take())
        });
        if let Some(mut callback) = callback {
            callback(new);
            runtime.with(|rt| {
                if let Some(handlers) = rt.borrow_mut().handlers.get_mut(id) {
                    handlers.scroll = Some(callback);
                }
            });
        }
        schedule_debounced_scroll(runtime, id, new, now);
    }
    true
}

fn schedule_debounced_scroll(runtime: &Runtime, id: WidgetId, offset: f32, now: Instant) {
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let Some(debounced) = rt
            .scrolls
            .get_mut(id)
            .and_then(|state| state.debounced.as_mut())
        else {
            return;
        };
        let burst_start = *debounced.burst_start.get_or_insert(now);
        let trailing = now.checked_add(debounced.delay).unwrap_or(now);
        let maximum = burst_start.checked_add(debounced.max_wait).unwrap_or(now);
        debounced.deadline = Some(trailing.min(maximum));
        debounced.latest_offset = offset;
    });
}

/// Returns the earliest pending trailing scroll callback. Native hosts combine
/// this with their other wake deadlines and use `ControlFlow::WaitUntil`.
pub fn next_scroll_callback_deadline(runtime: &Runtime) -> Option<Instant> {
    runtime.with(|rt| {
        rt.borrow()
            .scrolls
            .values()
            .filter_map(|state| {
                state
                    .debounced
                    .as_ref()
                    .and_then(|callback| callback.deadline)
            })
            .min()
    })
}

/// Runs every trailing scroll callback due at `now`, without retaining a runtime
/// borrow across application code. Returns whether any callback was fired.
///
/// A callback's schedule is cleared before invocation, beginning a fresh burst
/// for the next scroll mutation. Reinstallation is conditional on the viewport
/// still being retained, so purged scroll nodes cannot revive a pending callback.
pub fn fire_due_scroll_callbacks(runtime: &Runtime, now: Instant) -> bool {
    let ids: SmallVec<[WidgetId; 4]> = runtime.with(|rt| {
        rt.borrow()
            .scrolls
            .iter()
            .filter_map(|(id, state)| {
                state.debounced.as_ref().and_then(|callback| {
                    callback
                        .deadline
                        .is_some_and(|deadline| deadline <= now)
                        .then_some(id)
                })
            })
            .collect()
    });
    let mut fired = false;
    for id in ids {
        let callback = runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let callback = rt
                .scrolls
                .get_mut(id)
                .and_then(|state| state.debounced.as_mut())?;
            if !callback.deadline.is_some_and(|deadline| deadline <= now) {
                return None;
            }
            callback.deadline = None;
            callback.burst_start = None;
            Some((callback.callback.take()?, callback.latest_offset))
        });
        let Some((mut callback, offset)) = callback else {
            continue;
        };
        callback(offset);
        runtime.with(|rt| {
            if let Some(callback_state) = rt
                .borrow_mut()
                .scrolls
                .get_mut(id)
                .and_then(|state| state.debounced.as_mut())
            {
                callback_state.callback = Some(callback);
            }
        });
        fired = true;
    }
    fired
}

/// Returns the stable remount identity configured for a scroll viewport.
pub fn scroll_restoration_key(runtime: &Runtime, id: WidgetId) -> Option<String> {
    runtime.with(|rt| {
        rt.borrow()
            .scrolls
            .get(id)
            .and_then(|state| state.restoration_key.as_deref())
            .map(str::to_owned)
    })
}

/// Whether this viewport requests conditional end-following across remounts.
pub fn scroll_follows_end(runtime: &Runtime, id: WidgetId) -> bool {
    runtime.with(|rt| {
        rt.borrow()
            .scrolls
            .get(id)
            .is_some_and(|state| state.follow_end)
    })
}

/// Whether the viewport currently rests at its final scroll position.
pub fn scroll_is_at_end(scene: &Scene, id: WidgetId) -> bool {
    scroll_metrics(scene, id).is_some_and(|metrics| metrics.max_offset - metrics.offset <= 1.0)
}

/// Clamps every retained scroll offset against freshly computed content geometry.
/// This is run after layout so an offset restored across a content remount remains
/// stable when possible and lands at the new end when the replacement is shorter.
pub fn clamp_scroll_offsets(runtime: &Runtime, scene: &mut Scene) -> bool {
    let ids: SmallVec<[WidgetId; 8]> = runtime.with(|rt| {
        rt.borrow()
            .scrolls
            .keys()
            .filter(|id| scene.node(*id).is_some())
            .collect()
    });
    let mut changed = false;
    for id in ids {
        if let Some(metrics) = scroll_metrics(scene, id) {
            changed |= set_scroll_position(runtime, scene, id, metrics.offset, false);
            emit_scrollbar_paint(runtime, scene, id);
        }
    }
    changed
}

/// One wheel-notch / scroll-action's worth of vertical scroll on a `Scroll` viewport
/// — the single inbound path shared by mouse-wheel input and an inbound AccessKit
/// `ScrollUp`/`ScrollDown` `ActionRequest` (SOUL §6.3). A **positive** `delta_y`
/// scrolls the content *up* (the offset grows toward the end); a negative one scrolls
/// back toward the start.
///
/// Acts only on a [`WidgetKind::Scroll`] node that has a laid-out viewport rect;
/// anything else returns `false`. The scrollable content height is the furthest bottom
/// edge of the node's *direct children* (their rects are absolute window-space)
/// measured from the viewport's top, and `max_offset = (content_height −
/// viewport_height).max(0)`. The new offset is `(current + delta_y).clamp(0,
/// max_offset)`; if the clamp leaves it unchanged (already at an end, or an empty
/// delta) nothing is touched and this returns `false`.
///
/// On a real move it writes the scene scroll-offset column
/// ([`Scene::set_scroll_offset`] — **paint-dirty only**, the renderer recomposites the
/// viewport's children, no relayout, SOUL §3.2) and updates the accessible value to
/// the rounded offset string. That value update is the one **budgeted** allocation on
/// this path (a fresh `String`, the `text_edit`-class cost of SOUL §4.1); everything
/// else here is alloc-free — the clamp is scalar math and the offset column mutates in
/// place. Finally it fires the widget's stored `on_scroll` handler with the new offset,
/// taken out of the registry *before* it runs so no registry borrow is held across
/// user code (§3.1, the same discipline as [`dispatch_click`]).
pub fn dispatch_scroll(runtime: &Runtime, scene: &mut Scene, id: WidgetId, delta_y: f32) -> bool {
    dispatch_scroll_at(runtime, scene, id, delta_y, Instant::now())
}

/// Routes a wheel delta through the nested scroll chain under `point`.
///
/// The deepest viewport gets first refusal. When it is already at the requested
/// edge, the same delta bubbles to its nearest scroll ancestor, matching native
/// nested scroll behavior. A moved viewport consumes the wheel event.
pub fn dispatch_wheel_at(runtime: &Runtime, scene: &mut Scene, point: Point, delta_y: f32) -> bool {
    let mut target = hit_test_scroll_in(runtime, scene, point);
    while let Some(id) = target {
        if dispatch_scroll(runtime, scene, id, delta_y) {
            return true;
        }
        target = scroll_ancestor(scene, id);
    }
    false
}

fn scroll_ancestor(scene: &Scene, id: WidgetId) -> Option<WidgetId> {
    let mut parent = scene.node(id)?.parent;
    while let Some(candidate) = parent {
        let node = scene.node(candidate)?;
        if node.kind == WidgetKind::Scroll {
            return Some(candidate);
        }
        parent = node.parent;
    }
    None
}

pub(crate) fn dispatch_scroll_at(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    delta_y: f32,
    now: Instant,
) -> bool {
    let Some(metrics) = scroll_metrics(scene, id) else {
        return false;
    };
    let new = (metrics.offset + delta_y).clamp(0.0, metrics.max_offset);
    set_scroll_position_at(runtime, scene, id, new, true, now)
}

fn configured_scroll_at(
    runtime: &Runtime,
    scene: &Scene,
    point: Point,
    predicate: impl Fn(&ScrollState) -> bool,
) -> Option<WidgetId> {
    let mut candidate = hit_test_scroll_in(runtime, scene, point);
    while let Some(id) = candidate {
        let configured = runtime.with(|rt| rt.borrow().scrolls.get(id).is_some_and(&predicate));
        if configured {
            return Some(id);
        }
        candidate = scene
            .node(id)
            .and_then(|node| node.parent)
            .and_then(|mut id| loop {
                let node = scene.node(id)?;
                if node.kind == WidgetKind::Scroll {
                    return Some(id);
                }
                id = node.parent?;
            });
    }
    None
}

/// Captures an optional native scrollbar thumb or pages its track. Returns `true`
/// whenever the press belongs to scrollbar chrome, even if the viewport was already
/// at the requested end, so content beneath the chrome never receives the click.
pub fn begin_scrollbar_pointer(runtime: &Runtime, scene: &mut Scene, point: Point) -> bool {
    let Some(id) = configured_scroll_at(runtime, scene, point, |state| state.scrollbar) else {
        return false;
    };
    let Some(metrics) = scroll_metrics(scene, id) else {
        return false;
    };
    let Some((track, thumb)) = scrollbar_rects(metrics) else {
        return false;
    };
    if !track.contains(point) {
        return false;
    }
    if thumb.contains(point) {
        runtime.with(|rt| {
            rt.borrow_mut().scrollbar_pointer = Some(ScrollbarPointerCapture {
                id,
                grab_offset: point.y - thumb.y,
            });
        });
    } else {
        let direction = if point.y < thumb.y { -1.0 } else { 1.0 };
        let page = (metrics.viewport.height - EDGE_AUTO_SCROLL_ZONE).max(1.0);
        let _ = dispatch_scroll(runtime, scene, id, direction * page);
    }
    true
}

/// Moves a captured scrollbar thumb, mapping track travel directly onto the full
/// content offset. Returns `true` only when the offset changed.
pub fn update_scrollbar_pointer(runtime: &Runtime, scene: &mut Scene, point: Point) -> bool {
    let Some(capture) = runtime.with(|rt| rt.borrow().scrollbar_pointer) else {
        return false;
    };
    let Some(metrics) = scroll_metrics(scene, capture.id) else {
        return false;
    };
    let Some((track, thumb)) = scrollbar_rects(metrics) else {
        return false;
    };
    let travel = track.height - thumb.height;
    if travel <= 0.0 {
        return false;
    }
    let thumb_y = (point.y - capture.grab_offset).clamp(track.y, track.bottom() - thumb.height);
    let offset = (thumb_y - track.y) / travel * metrics.max_offset;
    set_scroll_position(runtime, scene, capture.id, offset, true)
}

/// Releases any captured native scrollbar thumb.
pub fn end_scrollbar_pointer(runtime: &Runtime) -> bool {
    runtime.with(|rt| rt.borrow_mut().scrollbar_pointer.take().is_some())
}

/// Whether a scrollbar thumb currently owns the pointer stream.
pub fn scrollbar_pointer_active(runtime: &Runtime) -> bool {
    runtime.with(|rt| rt.borrow().scrollbar_pointer.is_some())
}

/// Updates edge-auto-scroll activation for a held pointer. Passing `held = false`
/// clears it immediately; custom hosts can call this from their pointer stream.
pub fn update_edge_auto_scroll(runtime: &Runtime, scene: &Scene, point: Point, held: bool) -> bool {
    let next = held
        .then(|| {
            configured_scroll_at(runtime, scene, point, |state| state.edge_auto_scroll).and_then(
                |id| {
                    let viewport = scene.layout(id)?.rect;
                    let direction = if point.y <= viewport.y + EDGE_AUTO_SCROLL_ZONE {
                        -1.0
                    } else if point.y >= viewport.bottom() - EDGE_AUTO_SCROLL_ZONE {
                        1.0
                    } else {
                        return None;
                    };
                    Some(EdgeAutoScrollState { id, direction })
                },
            )
        })
        .flatten();
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let changed = rt.edge_auto_scroll.map(|state| (state.id, state.direction))
            != next.map(|state| (state.id, state.direction));
        rt.edge_auto_scroll = next;
        changed
    })
}

/// Advances one active pointer-edge scroll step.
pub fn tick_edge_auto_scroll(runtime: &Runtime, scene: &mut Scene) -> bool {
    let Some(state) = runtime.with(|rt| rt.borrow().edge_auto_scroll) else {
        return false;
    };
    dispatch_scroll(
        runtime,
        scene,
        state.id,
        state.direction * EDGE_AUTO_SCROLL_STEP,
    )
}

/// Whether edge scrolling is active and can still move in its requested direction.
pub fn has_active_edge_auto_scroll(runtime: &Runtime, scene: &Scene) -> bool {
    let Some(state) = runtime.with(|rt| rt.borrow().edge_auto_scroll) else {
        return false;
    };
    scroll_metrics(scene, state.id).is_some_and(|metrics| {
        (state.direction < 0.0 && metrics.offset > 0.0)
            || (state.direction > 0.0 && metrics.offset < metrics.max_offset)
    })
}

/// A keyboard/AccessKit adjustment of a range widget's value (SOUL §6.3) — the
/// standard-browser slider keys: arrows step, PageUp/PageDown jump, Home/End pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjust {
    /// Move by `n` steps; one step is 1% of the range (the browser
    /// `<input type=range>` default). Arrows pass ±1, PageUp/PageDown ±10.
    Steps(i32),
    /// Pin to the range minimum (Home).
    ToMin,
    /// Pin to the range maximum (End).
    ToMax,
}

/// One slider step as a fraction of the range — 1%, used when the builder does not
/// provide an explicit [`Slider::step`].
pub const SLIDER_STEP_FRACTION: f32 = 0.01;

pub fn format_slider_value(value: f32) -> String {
    let mut text = format!("{value:.4}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" {
        text.clear();
        text.push('0');
    }
    text
}

pub fn quantize_slider(value: f32, min: f32, max: f32, step: f32) -> f32 {
    if max <= min {
        return min;
    }
    let clamped = value.clamp(min, max);
    if !step.is_finite() || step <= 0.0 {
        return clamped;
    }
    let snapped = min + ((clamped - min) / step).round() * step;
    snapped.clamp(min, max)
}

/// Sets a slider to an absolute value through the same retained mutation path used
/// by pointer scrubbing and AccessKit `SetValue`.
pub fn dispatch_set_slider_value(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    value: f32,
) -> bool {
    if !matches!(scene.node(id), Some(n) if n.kind == WidgetKind::Slider) {
        return false;
    }
    let disabled = scene
        .a11y(id)
        .map(|a| StateFlags(a.state).contains(StateFlags::DISABLED))
        .unwrap_or(false);
    if disabled || !value.is_finite() {
        return false;
    }
    let Some((old, min, max, step)) = runtime.with(|rt| {
        rt.borrow()
            .sliders
            .get(id)
            .map(|s| (s.value, s.min, s.max, s.step))
    }) else {
        return false;
    };
    let new = quantize_slider(value, min, max, step);
    if (new - old).abs() <= f32::EPSILON {
        return false;
    }
    runtime.with(|rt| {
        if let Some(s) = rt.borrow_mut().sliders.get_mut(id) {
            s.value = new;
        }
    });
    let frac = if max > min {
        ((new - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    emit_slider_paint(runtime, scene, id, frac, false);
    reapply_focus_ring(runtime, scene, id);
    scene.mark_dirty(id, DirtyFlags::PAINT);
    scene.set_a11y_value(id, Some(format_slider_value(new)));
    let cb = runtime.with(|rt| {
        rt.borrow_mut()
            .handlers
            .get_mut(id)
            .and_then(|h| h.change.take())
    });
    if let Some(mut cb) = cb {
        cb(new);
        runtime.with(|rt| {
            if let Some(h) = rt.borrow_mut().handlers.get_mut(id) {
                h.change = Some(cb);
            }
        });
    }
    true
}

/// Scrubs a slider from a logical pointer position. Positions outside the rail
/// clamp to the nearest endpoint, matching native range controls.
pub fn dispatch_slider_pointer(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    point: Point,
) -> bool {
    if !matches!(scene.node(id), Some(n) if n.kind == WidgetKind::Slider) {
        return false;
    }
    let Some(rect) = scene.layout(id).map(|layout| layout.rect) else {
        return false;
    };
    let Some((min, max)) = runtime.with(|rt| rt.borrow().sliders.get(id).map(|s| (s.min, s.max)))
    else {
        return false;
    };
    if rect.width <= 0.0 || max <= min {
        return false;
    }
    let fraction = ((point.x - rect.x) / rect.width).clamp(0.0, 1.0);
    dispatch_set_slider_value(runtime, scene, id, min + (max - min) * fraction)
}

/// Adjusts a slider's value — the single inbound path shared by the keyboard
/// (arrows / PageUp / PageDown / Home / End on the focused slider) and an inbound
/// AccessKit `Increment`/`Decrement` `ActionRequest` (SOUL §6.3). Clamps to the
/// slider's `[min, max]`, re-emits the track + fill paint in place, updates the
/// accessible value, and fires the stored `on_change` handler with the new value
/// (taken out of the registry before it runs — §3.1). Returns `true` if the value
/// actually moved; a slider already at the targeted end is a no-op. A disabled or
/// non-slider target returns `false`.
pub fn dispatch_adjust(runtime: &Runtime, scene: &mut Scene, id: WidgetId, adjust: Adjust) -> bool {
    if !matches!(scene.node(id), Some(n) if n.kind == WidgetKind::Slider) {
        return false;
    }
    let disabled = scene
        .a11y(id)
        .map(|a| StateFlags(a.state).contains(StateFlags::DISABLED))
        .unwrap_or(false);
    if disabled {
        return false;
    }
    let Some((value, min, max, step)) = runtime.with(|rt| {
        rt.borrow()
            .sliders
            .get(id)
            .map(|s| (s.value, s.min, s.max, s.step))
    }) else {
        return false;
    };
    let target = match adjust {
        Adjust::Steps(n) => value + step * n as f32,
        Adjust::ToMin => min,
        Adjust::ToMax => max,
    };
    dispatch_set_slider_value(runtime, scene, id, target)
}

/// A keyboard activation key, resolved by the caller (SOUL §6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivateKey {
    Enter,
    Space,
}

/// Activates a widget from the keyboard with the standard browser matrix
/// (SOUL §6.3): a button takes Enter *and* Space; a link takes Enter only; a
/// checkbox / switch / radio takes Space only; tabs, list items, dropdowns and
/// selectable table rows take both. Returns `None` when `key` does not activate
/// this widget kind (the caller falls through to page scrolling — a Space on a
/// focused link scrolls, exactly like a browser), and `Some(changed)` when the
/// key was consumed — `changed` is [`dispatch_click`]'s result, so a consumed
/// activation with no handler is still consumed (`Some(false)`).
pub fn dispatch_key_activate(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    key: ActivateKey,
) -> Option<bool> {
    let kind = scene.node(id)?.kind;
    let activates = match kind {
        WidgetKind::Button => true,
        WidgetKind::Link => key == ActivateKey::Enter,
        WidgetKind::Checkbox | WidgetKind::Switch | WidgetKind::Radio => key == ActivateKey::Space,
        WidgetKind::Tab
        | WidgetKind::ListItem
        | WidgetKind::Dropdown
        | WidgetKind::DropdownOption
        | WidgetKind::TableRow => true,
        _ => false,
    };
    if !activates {
        return None;
    }
    Some(dispatch_click(runtime, scene, id))
}

/// The nearest [`WidgetKind::Scroll`] ancestor of `from` (SOUL §6.3) — the
/// viewport a keyboard scroll targets when focus sits inside one, exactly like a
/// browser scrolling the container around the focused element.
pub fn enclosing_scroll(scene: &Scene, from: WidgetId) -> Option<WidgetId> {
    let mut cur = scene.node(from)?.parent;
    while let Some(id) = cur {
        let node = scene.node(id)?;
        if node.kind == WidgetKind::Scroll {
            return Some(id);
        }
        cur = node.parent;
    }
    None
}

/// The first [`WidgetKind::Scroll`] viewport in tree pre-order (SOUL §6.3) — the
/// "document" a keyboard scroll falls back to when nothing focused sits inside
/// one, the way PageDown scrolls a browser page with no focus.
pub fn first_scroll(scene: &Scene) -> Option<WidgetId> {
    first_scroll_in(scene, scene.root()?)
}

/// The first scroll viewport in `root`'s subtree. Dialog keyboard routing uses
/// this to keep page keys inside an active modal.
pub fn first_scroll_in(scene: &Scene, root: WidgetId) -> Option<WidgetId> {
    fn rec(scene: &Scene, id: WidgetId) -> Option<WidgetId> {
        let node = scene.node(id)?;
        if node.kind == WidgetKind::Scroll {
            return Some(id);
        }
        for &c in &node.children {
            if let Some(found) = rec(scene, c) {
                return Some(found);
            }
        }
        None
    }
    rec(scene, root)
}

// ---------------------------------------------------------------------------
// the keyboard focus ring (SOUL §6.3 — focus must be *visible* to be usable)
// ---------------------------------------------------------------------------
