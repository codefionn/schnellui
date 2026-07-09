use super::*;

pub fn has_floating_label_animations(runtime: &crate::Runtime) -> bool {
    runtime.with(|rt| {
        rt.borrow().edits.values().any(|edit| {
            !edit.placeholder.is_empty()
                && (edit.label_progress - edit.label_target).abs() > f32::EPSILON
        })
    })
}

fn advance_floating_labels(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    animated: bool,
) -> bool {
    let changed: SmallVec<[WidgetId; 8]> = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let mut changed = SmallVec::new();
        for (id, edit) in &mut rt.edits {
            if edit.placeholder.is_empty() || scene.node(id).is_none() {
                continue;
            }
            let delta = edit.label_target - edit.label_progress;
            if delta.abs() <= f32::EPSILON {
                continue;
            }
            edit.label_progress = if animated && delta.abs() > FLOAT_LABEL_ANIMATION_STEP {
                edit.label_progress + FLOAT_LABEL_ANIMATION_STEP.copysign(delta)
            } else {
                edit.label_target
            };
            changed.push(id);
        }
        changed
    });
    for &id in &changed {
        emit_text_input_paint(runtime, scene, shaper, atlas, id);
        scene.mark_dirty(id, DirtyFlags::PAINT);
    }
    !changed.is_empty()
}

/// Advances active floating-label transitions by one display frame.
pub fn tick_floating_labels(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) -> bool {
    advance_floating_labels(runtime, scene, shaper, atlas, true)
}

/// Snaps active floating labels to their target state for reduced-motion hosts.
pub fn finish_floating_label_animations(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) -> bool {
    advance_floating_labels(runtime, scene, shaper, atlas, false)
}

// ---------------------------------------------------------------------------
// dispatch (the single inbound path — pointer, keyboard, AccessKit; SOUL §6.3)
// ---------------------------------------------------------------------------

fn is_text_input(scene: &Scene, id: WidgetId) -> bool {
    matches!(scene.node(id), Some(n) if n.kind == WidgetKind::TextInput)
}

/// Any editable text kind — the dispatchers below fan out on this (SOUL §6.3:
/// one inbound path; `TextInput` edits here, `TextArea` in [`crate::text_area`]).
fn editable_kind(scene: &Scene, id: WidgetId) -> Option<WidgetKind> {
    match scene.node(id) {
        Some(n) if n.kind == WidgetKind::TextInput || n.kind == WidgetKind::TextArea => {
            Some(n.kind)
        }
        _ => None,
    }
}

fn is_disabled(scene: &Scene, id: WidgetId) -> bool {
    scene
        .a11y(id)
        .map(|a| StateFlags(a.state).contains(StateFlags::DISABLED))
        .unwrap_or(false)
}

/// Fires the widget's `on_input` with the new value, taken out of the registry
/// before it runs so user code never executes under the registry borrow (§3.1).
/// Shared with [`text_area`](crate::text_area)'s commit path (SOUL §6.3).
pub(crate) fn fire_on_input(runtime: &crate::Runtime, id: WidgetId, value: &str) {
    let cb = runtime.with(|rt| {
        rt.borrow_mut()
            .handlers
            .get_mut(id)
            .and_then(|h| h.input.take())
    });
    if let Some(mut cb) = cb {
        cb(value);
        runtime.with(|rt| {
            if let Some(h) = rt.borrow_mut().handlers.get_mut(id) {
                h.input = Some(cb);
            }
        });
    }
}

/// Clones the current retained value of a single-line editor.
///
/// Selection widgets use this after an edit callback returns so filtering is
/// based on the locally painted value even when the controlled host deliberately
/// postpones a structural rebuild.
pub(crate) fn edit_value(runtime: &crate::Runtime, id: WidgetId) -> Option<String> {
    runtime.with(|rt| rt.borrow().edits.get(id).map(|state| state.value.clone()))
}

/// Re-emits `id`'s paint and flags the channels a `Change` touched. For a `Text`
/// change also updates the accessible value and fires `on_input` (SOUL §6.2/§6.3).
fn commit_change(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    change: Change,
) -> bool {
    match change {
        Change::None => false,
        Change::Caret => {
            emit_text_input_paint(runtime, scene, shaper, atlas, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
            true
        }
        Change::Text => {
            let value = runtime.with(|rt| {
                rt.borrow()
                    .edits
                    .get(id)
                    .map(|e| (e.value.clone(), e.password))
            });
            let Some((value, password)) = value else {
                return false;
            };
            scene.set_a11y_value(id, Some(display_value(&value, password)));
            emit_text_input_paint(runtime, scene, shaper, atlas, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
            fire_on_input(runtime, id, &value);
            true
        }
    }
}

/// Moves keyboard focus to `target` (or clears it with `None`), enforcing
/// **exclusivity** via the a11y [`StateFlags::FOCUSED`] bit — the same bit
/// [`focused`](schnellui_a11y::focused) and the AccessKit tree read (SOUL §6.2).
/// Only a live, non-disabled node advertising the `Focus` action can take focus;
/// anything else clears it. A focused/blurred text input is repainted in place
/// (focus ring + caret). Returns `true` if focus actually moved.
pub fn dispatch_focus(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    target: Option<WidgetId>,
) -> bool {
    dispatch_focus_with_indicator(runtime, scene, shaper, atlas, target, true)
}

/// Pointer-flavoured focus, mirroring the browser's `:focus-visible` heuristic:
/// focus still moves semantically, but ordinary controls do not receive the
/// keyboard focus ring. Editables do receive the outline and continue to paint
/// their focused border and caret because pointer placement immediately starts
/// an editing interaction.
pub fn dispatch_pointer_focus(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    target: Option<WidgetId>,
) -> bool {
    dispatch_focus_with_indicator(runtime, scene, shaper, atlas, target, false)
}

fn dispatch_focus_with_indicator(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    target: Option<WidgetId>,
    show_indicator: bool,
) -> bool {
    let old = schnellui_a11y::focused(scene);
    let guarded_target = if let Some(panel) = crate::dialog::active_modal_panel(scene) {
        match target {
            Some(target) if crate::dialog::is_in_subtree(scene, target, panel) => Some(target),
            // Pointer presses on the modal scrim and programmatic attempts to
            // focus the obscured application keep focus inside the dialog.
            _ => old
                .filter(|old| crate::dialog::is_in_subtree(scene, *old, panel))
                .or_else(|| {
                    schnellui_a11y::tab_order(scene)
                        .into_iter()
                        .find(|id| crate::dialog::is_in_subtree(scene, *id, panel))
                }),
        }
    } else {
        target
    };
    let new = guarded_target.filter(|&t| {
        scene.node(t).is_some()
            && !is_disabled(scene, t)
            && scene
                .a11y(t)
                .map(|a| ActionFlags(a.actions).contains(ActionFlags::FOCUS))
                .unwrap_or(false)
    });
    let indicator_visible = show_indicator
        || new.is_some_and(|id| {
            matches!(
                scene.node(id).map(|node| node.kind),
                Some(WidgetKind::TextInput | WidgetKind::TextArea)
            )
        });
    if old == new {
        return new
            .map(|id| crate::set_focus_ring_visible(runtime, scene, id, indicator_visible))
            .unwrap_or(false);
    }
    if let Some(o) = old {
        let state = scene.a11y_mut(o).state & !StateFlags::FOCUSED.0;
        scene.set_a11y_state(o, state);
        repaint_editable(runtime, scene, shaper, atlas, o);
        crate::remove_focus_ring(runtime, scene, o);
    }
    if let Some(n) = new {
        let state = scene.a11y_mut(n).state | StateFlags::FOCUSED.0;
        scene.set_a11y_state(n, state);
        repaint_editable(runtime, scene, shaper, atlas, n);
        // The generic ring follows browser `:focus-visible`: keyboard,
        // assistive, and programmatic focus show it. Pointer focus shows it only
        // for editables, which browsers treat as keyboard-input controls.
        crate::set_focus_ring_visible(runtime, scene, n, indicator_visible);
    }
    true
}

/// Re-emits a focused/blurred editable's paint in place (focus ring + caret),
/// fanning out on the editable kind (SOUL §6.3). Non-editables are untouched.
fn repaint_editable(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
) {
    match editable_kind(scene, id) {
        Some(WidgetKind::TextInput) => {
            emit_text_input_paint(runtime, scene, shaper, atlas, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
        }
        Some(WidgetKind::TextArea) => {
            crate::text_area::emit_text_area_paint(runtime, scene, shaper, atlas, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
        }
        _ => {}
    }
}

/// Routes one editing key to a text input — the identical mutation an inbound
/// AccessKit `SetValue` ultimately shares (SOUL §6.3). Mutates the edit state,
/// re-emits paint in place, updates the accessible value, and fires `on_input`
/// on a value change. Returns `true` if anything changed (⇒ redraw).
pub fn dispatch_edit_key(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    key: EditKey,
) -> bool {
    // A multi-line area routes to its own edit state (same key set, SOUL §6.3).
    if matches!(editable_kind(scene, id), Some(WidgetKind::TextArea)) {
        return crate::text_area::dispatch_area_key(runtime, scene, shaper, atlas, id, key);
    }
    if !is_text_input(scene, id) || is_disabled(scene, id) {
        return false;
    }
    let change = runtime.with(|rt| {
        rt.borrow_mut()
            .edits
            .get_mut(id)
            .map(|e| e.apply(&key))
            .unwrap_or(Change::None)
    });
    commit_change(runtime, scene, shaper, atlas, id, change)
}

/// Returns the currently selected text for an editable, if it has a non-empty
/// selection. Additional selections are returned in document order separated
/// by newlines, matching common desktop editor clipboard behavior.
pub fn selected_text(runtime: &crate::Runtime, scene: &Scene, id: WidgetId) -> Option<String> {
    match editable_kind(scene, id) {
        Some(WidgetKind::TextArea) if !is_disabled(scene, id) => {
            crate::text_area::selected_area_text(runtime, id)
        }
        Some(WidgetKind::TextInput) if !is_disabled(scene, id) => runtime.with(|rt| {
            let rt = rt.borrow();
            let state = rt.edits.get(id)?;
            selection_text(&state.value, state.caret, state.anchor, &state.secondary)
        }),
        _ => None,
    }
}

/// Deletes only the active selection(s). A collapsed caret is a no-op.
pub fn delete_text_selection(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
) -> bool {
    dispatch_edit_key(runtime, scene, shaper, atlas, id, EditKey::Insert(""))
}

/// Inserts clipboard text into an editable. Single-line fields strip line
/// breaks; text areas normalize all platform line endings to `\n`.
pub fn dispatch_paste(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    text: &str,
) -> bool {
    match editable_kind(scene, id) {
        Some(WidgetKind::TextInput) => {
            let normalized;
            let text = if text.contains(['\r', '\n']) {
                normalized = text
                    .chars()
                    .filter(|character| !matches!(character, '\r' | '\n'))
                    .collect::<String>();
                normalized.as_str()
            } else {
                text
            };
            dispatch_edit_key(runtime, scene, shaper, atlas, id, EditKey::Insert(text))
        }
        Some(WidgetKind::TextArea) => {
            let normalized;
            let text = if text.contains('\r') {
                normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                normalized.as_str()
            } else {
                text
            };
            dispatch_edit_key(runtime, scene, shaper, atlas, id, EditKey::Insert(text))
        }
        _ => false,
    }
}

/// Places the caret from a pointer position on a text input (press), or extends
/// the selection to it (`extend` — a drag, or a shift-click). `p` is the pointer
/// in **logical** window coordinates, the same space [`hit_test`](crate::hit_test)
/// uses. Returns `true` if the caret/selection moved (⇒ redraw).
pub fn dispatch_text_pointer(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    p: Point,
    extend: bool,
) -> bool {
    dispatch_text_pointer_action(
        runtime,
        scene,
        shaper,
        atlas,
        id,
        p,
        TextPointerAction::Place { extend },
    )
}

/// Applies a semantic pointer-selection action to a `TextInput` or `TextArea`.
/// This is the headless/testable path used by window hosts for single, double,
/// and triple presses plus their drag continuation.
pub fn dispatch_text_pointer_action(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    p: Point,
    action: TextPointerAction,
) -> bool {
    // A multi-line area maps the pointer to (row, column) itself (SOUL §6.3).
    if matches!(editable_kind(scene, id), Some(WidgetKind::TextArea)) {
        return crate::text_area::dispatch_area_pointer(
            runtime, scene, shaper, atlas, id, p, action,
        );
    }
    if !is_text_input(scene, id) || is_disabled(scene, id) {
        return false;
    }
    let Some((value, size_px, scale, password)) = runtime.with(|rt| {
        let rt = rt.borrow();
        rt.edits
            .get(id)
            .map(|e| (e.value.clone(), e.size_px, e.scale, e.password))
    }) else {
        return false;
    };
    let Some(rect) = scene.layout(id).map(|b| b.rect).filter(|r| !r.is_empty()) else {
        return false;
    };
    let shown_value = display_value(&value, password);
    let shaped = shaper.shape(&shown_value, phys_size_px(size_px, scale), None);
    // pointer → inline offset (logical → physical), clamped left of the text box
    let x_phys = (p.x - rect.x - input_pads(runtime, id).0).max(0.0) * norm_scale(scale);
    let caret_idx = value_byte_for_display_byte(
        &value,
        byte_at_x(&shaped, shown_value.len(), x_phys),
        password,
    );
    let hit_idx = value_byte_for_display_byte(
        &value,
        byte_under_x(&shaped, shown_value.len(), x_phys),
        password,
    );
    let change = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let Some(e) = rt.edits.get_mut(id) else {
            return Change::None;
        };
        if action == TextPointerAction::AddCaret {
            e.pointer_origin = None;
            return toggle_additional_caret(
                &mut e.caret,
                &mut e.anchor,
                &mut e.secondary,
                caret_idx,
            );
        }
        let before = (e.caret, e.anchor);
        let cleared_secondary = action != TextPointerAction::Drag && !e.secondary.is_empty();
        if action != TextPointerAction::Drag {
            e.secondary.clear();
        }
        let idx = pointer_selection_index(action, e.pointer_origin, caret_idx, hit_idx);
        (e.caret, e.anchor) = apply_pointer_selection(
            &e.value,
            e.anchor,
            &mut e.pointer_origin,
            idx,
            action,
            false,
        );
        if (e.caret, e.anchor) == before && !cleared_secondary {
            Change::None
        } else {
            Change::Caret
        }
    });
    commit_change(runtime, scene, shaper, atlas, id, change)
}

/// Replaces a text input's whole value — the inbound AccessKit `SetValue` path
/// (SOUL §6.3): same paint re-emit, same a11y value update, same `on_input` as
/// typing. The caret parks at the end. Returns `true` if the value changed.
pub fn set_text_value(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    value: &str,
) -> bool {
    // A multi-line area replaces its value through its own state (SOUL §6.3).
    if matches!(editable_kind(scene, id), Some(WidgetKind::TextArea)) {
        return crate::text_area::dispatch_area_set_value(runtime, scene, shaper, atlas, id, value);
    }
    if !is_text_input(scene, id) || is_disabled(scene, id) {
        return false;
    }
    let change = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let Some(e) = rt.edits.get_mut(id) else {
            return Change::None;
        };
        if e.value == value {
            return Change::None;
        }
        e.value.clear();
        e.value.push_str(value);
        e.caret = e.value.len();
        e.anchor = e.caret;
        e.secondary.clear();
        Change::Text
    });
    commit_change(runtime, scene, shaper, atlas, id, change)
}
