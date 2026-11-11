//! Values the user changed on controls the app did not bind to state.
//!
//! A rebuild (theme switch, structural remount) constructs every control from
//! its authored value. A checkbox the user ticked, a slider they moved or a
//! field they typed in would snap back, because nothing in the app knows the
//! live value. Each such control records what it was authored with at build,
//! as one `u64` (a flag, the bits of a float, or a hash of a text). When a
//! counterpart in the replacement was authored with the same value, the app
//! has not spoken and the previous tree's live value carries over. A different
//! authored value means the app wrote something new, and it wins. A control
//! with a change handler is never carried: the app heard every edit, and a
//! rebuild that authors the first value again (a draft cleared after submit)
//! is the app speaking, not silence.

use super::*;
use std::hash::{Hash, Hasher};

/// The authored value of a text, small enough to keep per control.
pub(crate) fn text_bits(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Remembers what the app authored for `id`. Call once, at build.
pub(crate) fn note_authored(runtime: &Runtime, id: WidgetId, bits: u64) {
    runtime.with(|rt| {
        rt.borrow_mut().authored.insert(id, bits);
    });
}

fn flag(on: bool) -> u64 {
    u64::from(on)
}

fn has(scene: &Scene, id: WidgetId, bit: StateFlags) -> bool {
    scene
        .a11y(id)
        .is_some_and(|a| StateFlags(a.state).contains(bit))
}

/// The control's current value in the encoding [`note_authored`] uses.
fn live_bits(runtime: &Runtime, scene: &Scene, id: WidgetId) -> Option<u64> {
    Some(match scene.node(id)?.kind {
        WidgetKind::Checkbox | WidgetKind::Switch | WidgetKind::Radio => {
            flag(has(scene, id, StateFlags::CHECKED))
        }
        WidgetKind::Tab
        | WidgetKind::ListItem
        | WidgetKind::DropdownOption
        | WidgetKind::TableRow => flag(has(scene, id, StateFlags::SELECTED)),
        WidgetKind::Slider => u64::from(
            runtime
                .with(|rt| rt.borrow().sliders.get(id).map(|s| s.value))?
                .to_bits(),
        ),
        WidgetKind::TextInput => {
            runtime.with(|rt| rt.borrow().edits.get(id).map(|e| text_bits(&e.value)))?
        }
        WidgetKind::TextArea => runtime.with(|rt| {
            rt.borrow()
                .areas
                .get(id)
                .map(|a| text_bits(&a.real_value()))
        })?,
        WidgetKind::Dropdown => text_bits(scene.a11y(id)?.value.as_deref()?),
        _ => return None,
    })
}

/// Whether `id` has the handler that reports its value to the app.
fn has_change_handler(runtime: &Runtime, id: WidgetId, kind: WidgetKind) -> bool {
    runtime.with(|rt| {
        rt.borrow().handlers.get(id).is_some_and(|h| match kind {
            WidgetKind::Checkbox | WidgetKind::Switch => h.toggle.is_some(),
            WidgetKind::Slider => h.change.is_some(),
            WidgetKind::TextInput | WidgetKind::TextArea => h.input.is_some(),
            _ => h.click.is_some(),
        })
    })
}

/// The container a selectable belongs to: its tab bar, list, table or
/// dropdown. A pick moves between siblings, so the whole group is either the
/// app's or the user's.
fn selection_group(scene: &Scene, id: WidgetId) -> Option<WidgetId> {
    let mut at = scene.node(id)?.parent;
    while let Some(node_id) = at {
        let node = scene.node(node_id)?;
        let is_group = matches!(
            node.kind,
            WidgetKind::TabBar | WidgetKind::List | WidgetKind::Table
        ) || node.children.iter().any(|&c| {
            scene
                .node(c)
                .is_some_and(|n| n.kind == WidgetKind::Dropdown)
        });
        if is_group {
            return Some(node_id);
        }
        at = node.parent;
    }
    None
}

/// Whether the app hears about changes to `id`. A control with a change
/// handler (on_toggle, on_change, on_input, on_select) is the app's: the app
/// was told every edit and decides what the next build shows, even when that
/// is the value it first authored (a draft cleared after submit). A
/// selectable counts as the app's when any pick in its group is reported.
fn reported_to_app(runtime: &Runtime, scene: &Scene, id: WidgetId) -> bool {
    let Some(kind) = scene.node(id).map(|n| n.kind) else {
        return false;
    };
    let item_kind = match kind {
        WidgetKind::Tab | WidgetKind::ListItem | WidgetKind::TableRow => kind,
        WidgetKind::DropdownOption | WidgetKind::Dropdown => WidgetKind::DropdownOption,
        _ => return has_change_handler(runtime, id, kind),
    };
    let Some(group) = selection_group(scene, id) else {
        return has_change_handler(runtime, id, kind);
    };
    scene.subtree_nodes(group).into_iter().any(|item| {
        scene.node(item).is_some_and(|n| n.kind == item_kind)
            && has_change_handler(runtime, item, item_kind)
    })
}

/// Whether the user changed `id` away from what the app authored, on a
/// control whose value the app does not hear about.
pub fn control_changed_by_user(runtime: &Runtime, scene: &Scene, id: WidgetId) -> bool {
    if reported_to_app(runtime, scene, id) {
        return false;
    }
    let authored = runtime.with(|rt| rt.borrow().authored.get(id).copied());
    match (authored, live_bits(runtime, scene, id)) {
        (Some(authored), Some(live)) => authored != live,
        _ => false,
    }
}

/// Carries the live value of one control the user changed into its
/// counterpart in a replacement tree, when the replacement was authored with
/// the same value. Runtime state, accessibility and paint follow, and no
/// handler fires: the app already saw (or chose to ignore) the user's edit.
/// Returns whether a value was carried.
#[allow(clippy::too_many_arguments)]
pub fn inherit_control_value(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    previous_runtime: &Runtime,
    previous_scene: &Scene,
    previous_id: WidgetId,
) -> bool {
    let Some(kind) = scene.node(id).map(|n| n.kind) else {
        return false;
    };
    if previous_scene.node(previous_id).map(|n| n.kind) != Some(kind)
        || !control_changed_by_user(previous_runtime, previous_scene, previous_id)
    {
        return false;
    }
    let previous_authored =
        previous_runtime.with(|rt| rt.borrow().authored.get(previous_id).copied());
    let authored = runtime.with(|rt| rt.borrow().authored.get(id).copied());
    if authored.is_none() || authored != previous_authored {
        return false;
    }
    let Some(live) = live_bits(previous_runtime, previous_scene, previous_id) else {
        return false;
    };
    match kind {
        WidgetKind::Checkbox | WidgetKind::Switch | WidgetKind::Radio => {
            let on = live == 1;
            let mut state = StateFlags(scene.a11y(id).map_or(0, |a| a.state));
            if on {
                state.insert(StateFlags::CHECKED);
            } else {
                state.0 &= !StateFlags::CHECKED.0;
            }
            scene.set_a11y_state(id, state.0);
            match kind {
                WidgetKind::Checkbox => emit_checkbox_paint(runtime, scene, id, on),
                WidgetKind::Switch => basic::emit_switch_paint(runtime, scene, id, on),
                _ => basic::emit_radio_paint(runtime, scene, id, on),
            }
            redecorate_after_refill(runtime, scene, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
        }
        WidgetKind::Tab | WidgetKind::ListItem | WidgetKind::DropdownOption => {
            let on = live == 1;
            let mut state = StateFlags(scene.a11y(id).map_or(0, |a| a.state));
            if on {
                state.insert(StateFlags::SELECTED);
            } else {
                state.0 &= !StateFlags::SELECTED.0;
            }
            scene.set_a11y_state(id, state.0);
            selection::recolor_selection(runtime, scene, id, kind, on);
            scene.mark_dirty(id, DirtyFlags::PAINT);
        }
        WidgetKind::TableRow => {
            let on = live == 1;
            let mut state = StateFlags(scene.a11y(id).map_or(0, |a| a.state));
            if on {
                state.insert(StateFlags::SELECTED);
            } else {
                state.0 &= !StateFlags::SELECTED.0;
            }
            scene.set_a11y_state(id, state.0);
            table::recolor_row(runtime, scene, id, on);
            scene.mark_dirty(id, DirtyFlags::PAINT);
        }
        WidgetKind::Slider => {
            let value = f32::from_bits(live as u32);
            let Some((min, max, step)) =
                runtime.with(|rt| rt.borrow().sliders.get(id).map(|s| (s.min, s.max, s.step)))
            else {
                return false;
            };
            let now = quantize_slider(value, min, max, step);
            runtime.with(|rt| {
                if let Some(s) = rt.borrow_mut().sliders.get_mut(id) {
                    s.value = now;
                }
            });
            let frac = if max > min {
                ((now - min) / (max - min)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let disabled = has(scene, id, StateFlags::DISABLED);
            emit_slider_paint(runtime, scene, id, frac, disabled);
            redecorate_after_refill(runtime, scene, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
            scene.set_a11y_value(id, Some(format_slider_value(now)));
        }
        WidgetKind::TextInput | WidgetKind::TextArea => {
            let value = previous_runtime.with(|rt| {
                let rt = rt.borrow();
                if kind == WidgetKind::TextArea {
                    rt.areas
                        .get(previous_id)
                        .map(|a| a.real_value().into_owned())
                } else {
                    rt.edits.get(previous_id).map(|e| e.value.clone())
                }
            });
            let Some(value) = value else { return false };
            // The edit already reached the app when it was typed.
            let handler = runtime.with(|rt| {
                rt.borrow_mut()
                    .handlers
                    .get_mut(id)
                    .and_then(|h| h.input.take())
            });
            let changed = set_text_value(runtime, scene, shaper, atlas, id, &value);
            runtime.with(|rt| {
                if let Some(h) = rt.borrow_mut().handlers.get_mut(id) {
                    h.input = handler;
                }
            });
            return changed;
        }
        WidgetKind::Dropdown => {
            let Some(value) = previous_scene
                .a11y(previous_id)
                .and_then(|a| a.value.clone())
            else {
                return false;
            };
            scene.set_a11y_value(id, Some(value));
            // The painted label follows on the next frame.
            runtime.with(|rt| {
                let mut rt = rt.borrow_mut();
                if rt.dropdown_triggers.contains_key(id) && !rt.dropdown_relabels.contains(&id) {
                    rt.dropdown_relabels.push(id);
                }
            });
        }
        _ => return false,
    }
    true
}
