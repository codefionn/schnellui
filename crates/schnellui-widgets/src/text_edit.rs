//! Interactive text editing for [`TextInput`](crate::TextInput) (SOUL §6.3, §8.1):
//! focus, caret, selection, and the keyboard/pointer edit dispatch.
//!
//! This module owns the retained **edit state** of every text input — the current
//! value plus a caret/anchor byte-index pair (`caret == anchor` ⇒ no selection) —
//! stored in the app-owned widget runtime beside the input's handlers, exactly
//! like the dynamic-text slots (SOUL §3.3: it holds a `String` and feeds `!Send`
//! user handlers, so it cannot live in the render-ready scene columns).
//!
//! **One inbound path** (SOUL §6.3): the windowed keyboard/pointer events and an
//! inbound AccessKit `Focus`/`SetValue` `ActionRequest` all converge on the same
//! dispatch functions here, which mutate the edit state, re-emit the input's paint
//! in place, update the accessible value, and fire the widget's `on_input` handler.
//!
//! **Budget** (SOUL §4.1): an edit is user-initiated, `text_edit`-class work — it
//! may re-shape the line and clone the value for a11y. It never runs on the
//! steady-state re-render path (a clean frame does zero work here), and paint is
//! cleared-and-refilled in place (grow-only `Vec`, §4.4). Caret math is plain
//! byte-index and advance arithmetic; **LTR is assumed** for caret↔x mapping
//! (v0 — clusters are carried by the shaper for the bidi refinement).

use schnellui_a11y::{ActionFlags, StateFlags};
use schnellui_scene::{DirtyFlags, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind};
use schnellui_text::{GlyphAtlas, ShapedText, TextShaper};
use smallvec::SmallVec;
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    node_rect, norm_scale, phys_size_px, rasterize_and_push, EMPTY_LINE_RATIO, PAD_H, PAD_V,
};

mod pointer;
pub(crate) use pointer::*;
pub use pointer::{EditKey, TextPointerAction};
mod paint;
pub(crate) use paint::*;
mod dispatch;
pub use dispatch::*;

// ---------------------------------------------------------------------------
// visual + metric constants (deterministic for shots, SOUL §7.3)
// ---------------------------------------------------------------------------

/// Minimum content width a text field reserves (SOUL §8.1).
pub(crate) const MIN_FIELD_W: f32 = 120.0;
/// Border thickness — the inner surface is inset by this much. Colours come
/// from the ambient theme ([`outline`](crate::Theme::outline) at rest,
/// [`accent`](crate::Theme::accent) focused), shared with
/// [`text_area`](crate::text_area) so both editables read as one family.
pub(crate) const INPUT_BORDER_W: f32 = 1.0;
/// Caret stroke width, logical px.
pub(crate) const CARET_W: f32 = 1.0;
/// A floated Material-style label is one typography step below the input value.
const FLOAT_LABEL_SIZE_RATIO: f32 = 0.75;
/// Progress per display frame. At a typical 60 Hz this yields a ~150 ms transition.
const FLOAT_LABEL_ANIMATION_STEP: f32 = 0.12;

/// The field's effective content padding under the ambient shape tokens
/// (SOUL §8.1): the classic button pads scaled by the density axis. Shared by
/// paint, the intrinsic measure, and the caret hit-test so they never disagree.
pub(crate) fn input_pads(runtime: &crate::Runtime, id: WidgetId) -> (f32, f32) {
    let sh = crate::theme_for(runtime, id).shape;
    (sh.pad(PAD_H), sh.pad(PAD_V))
}

/// The field's effective border width: the ink [`frame`](crate::Shape::frame)
/// when the design system sets one, else the classic hairline.
pub(crate) fn input_border_w(runtime: &crate::Runtime, id: WidgetId) -> f32 {
    let f = crate::theme_for(runtime, id).shape.frame;
    if f > 0.0 {
        f
    } else {
        INPUT_BORDER_W
    }
}

// ---------------------------------------------------------------------------
// the retained edit state (SOUL §3.3 registry — see module docs)
// ---------------------------------------------------------------------------

/// One text input's retained editing state. `caret`/`anchor` are **byte** indices
/// into `value`, always on `char` boundaries; `caret == anchor` means no selection.
pub(crate) struct EditState {
    pub(crate) value: String,
    /// Whether paint and accessibility surfaces must obscure `value`.
    pub(crate) password: bool,
    pub(crate) placeholder: String,
    pub(crate) caret: usize,
    pub(crate) anchor: usize,
    pub(crate) size_px: f32,
    /// Optional minimum painted width requested by the view.
    pub(crate) width: Option<f32>,
    /// logical→physical scale captured at build (SOUL §7.1 `--scale`).
    pub(crate) scale: f32,
    /// Selection unit captured by the current pointer gesture. This preserves
    /// the initially selected word/line when a double/triple-click becomes a
    /// drag in either direction.
    pointer_origin: Option<PointerOrigin>,
    /// Additional VS Code-style selections/carets. The public caret/anchor pair
    /// remains the primary selection for compatibility with the existing edit
    /// and accessibility paths.
    secondary: SmallVec<[TextSelection; 4]>,
    /// `0` is the empty, resting label; `1` is the compact floated label.
    label_progress: f32,
    /// Focus or a non-empty value asks the label to float. Kept separately so
    /// display frames can interpolate without rebuilding the widget.
    label_target: f32,
}

/// What a key application changed — selects the re-emit + notify work. Shared
/// with [`text_area`](crate::text_area)'s multi-line edit state (SOUL §6.3).
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum Change {
    None,
    /// caret/anchor moved; paint-only (recomposite the caret/selection visuals).
    Caret,
    /// the value changed; paint + a11y value + `on_input`.
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextSelection {
    pub(crate) caret: usize,
    pub(crate) anchor: usize,
}

/// Carries the user-controlled selection of a single-line editor into its
/// counterpart in a freshly mounted runtime. The replacement view remains
/// authoritative for the value: a changed controlled value deliberately keeps
/// the replacement editor's default selection.
pub(crate) fn inherit_edit_selection(
    runtime: &crate::Runtime,
    id: WidgetId,
    previous_runtime: &crate::Runtime,
    previous_id: WidgetId,
) -> bool {
    let previous = previous_runtime.with(|rt| {
        rt.borrow().edits.get(previous_id).map(|state| {
            (
                state.value.clone(),
                state.password,
                state.placeholder.clone(),
                state.caret,
                state.anchor,
                state.secondary.clone(),
                state.label_progress,
            )
        })
    });
    let Some((value, password, placeholder, caret, anchor, secondary, label_progress)) = previous
    else {
        return false;
    };
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let Some(state) = rt.edits.get_mut(id) else {
            return false;
        };
        // Preserve only the matching label's visual position. The replacement
        // value and subsequent focus restoration remain authoritative for its
        // target, so a modal that steals focus can still animate the label down.
        if state.placeholder == placeholder {
            state.label_progress = label_progress;
        }
        if state.value != value || state.password != password {
            return false;
        }
        state.caret = caret;
        state.anchor = anchor;
        state.secondary = secondary;
        state.pointer_origin = None;
        true
    })
}

impl TextSelection {
    pub(crate) fn collapsed(at: usize) -> Self {
        Self {
            caret: at,
            anchor: at,
        }
    }

    pub(crate) fn range(self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ReplaceMode {
    Selection,
    Backspace,
    Delete,
}

pub(crate) fn selection_list(
    caret: usize,
    anchor: usize,
    secondary: &[TextSelection],
) -> SmallVec<[TextSelection; 5]> {
    let mut selections = SmallVec::with_capacity(secondary.len() + 1);
    selections.push(TextSelection { caret, anchor });
    selections.extend_from_slice(secondary);
    selections
}

/// Returns the selected fragments in document order, separated like the
/// multi-selection clipboard convention used by desktop editors.
pub(crate) fn selection_text(
    value: &str,
    caret: usize,
    anchor: usize,
    secondary: &[TextSelection],
) -> Option<String> {
    let mut ranges: SmallVec<[(usize, usize); 5]> = selection_list(caret, anchor, secondary)
        .into_iter()
        .map(TextSelection::range)
        .filter(|(start, end)| start != end)
        .collect();
    if ranges.is_empty() {
        return None;
    }
    ranges.sort_unstable();
    ranges.dedup();

    let capacity = ranges.iter().map(|(start, end)| end - start).sum::<usize>()
        + ranges.len().saturating_sub(1);
    let mut selected = String::with_capacity(capacity);
    for (index, (start, end)) in ranges.into_iter().enumerate() {
        if index != 0 {
            selected.push('\n');
        }
        selected.push_str(&value[start..end]);
    }
    Some(selected)
}

pub(crate) fn map_selections(
    caret: &mut usize,
    anchor: &mut usize,
    secondary: &mut SmallVec<[TextSelection; 4]>,
    mut f: impl FnMut(TextSelection) -> TextSelection,
) -> Change {
    let before = selection_list(*caret, *anchor, secondary);
    let mut after: SmallVec<[TextSelection; 5]> = before.iter().copied().map(&mut f).collect();
    let primary = after.remove(0);
    after.retain(|selection| selection.range() != primary.range());
    let mut deduped = SmallVec::<[TextSelection; 4]>::new();
    for selection in after {
        if !deduped
            .iter()
            .any(|existing| existing.range() == selection.range())
        {
            deduped.push(selection);
        }
    }
    *caret = primary.caret;
    *anchor = primary.anchor;
    *secondary = deduped;
    if selection_list(*caret, *anchor, secondary) == before {
        Change::None
    } else {
        Change::Caret
    }
}

/// Applies one replacement at every retained selection as a single logical
/// edit. Ranges are normalized and processed right-to-left so byte offsets stay
/// valid; resulting carets are adjusted for edits that occurred to their left.
pub(crate) fn replace_selections(
    value: &mut String,
    caret: &mut usize,
    anchor: &mut usize,
    secondary: &mut SmallVec<[TextSelection; 4]>,
    replacement: &str,
    mode: ReplaceMode,
) -> Change {
    #[derive(Clone, Copy)]
    struct Edit {
        start: usize,
        end: usize,
        primary: bool,
    }

    let mut edits: Vec<Edit> = selection_list(*caret, *anchor, secondary)
        .into_iter()
        .enumerate()
        .map(|(index, selection)| {
            let (mut start, mut end) = selection.range();
            if start == end {
                match mode {
                    ReplaceMode::Selection => {}
                    ReplaceMode::Backspace if start > 0 => {
                        start = prev_boundary(value, start);
                    }
                    ReplaceMode::Delete if end < value.len() => {
                        end = next_boundary(value, end);
                    }
                    ReplaceMode::Backspace | ReplaceMode::Delete => {}
                }
            }
            Edit {
                start,
                end,
                primary: index == 0,
            }
        })
        .collect();
    edits.sort_by_key(|edit| (edit.start, edit.end));

    let mut normalized: Vec<Edit> = Vec::with_capacity(edits.len());
    for edit in edits {
        if let Some(last) = normalized.last_mut() {
            let duplicates = last.start == edit.start && last.end == edit.end;
            let overlaps = edit.start < last.end && edit.end > last.start;
            let point_inside = (last.start == last.end
                && edit.start <= last.start
                && last.start <= edit.end)
                || (edit.start == edit.end && last.start <= edit.start && edit.start <= last.end);
            if duplicates || overlaps || point_inside {
                last.start = last.start.min(edit.start);
                last.end = last.end.max(edit.end);
                last.primary |= edit.primary;
                continue;
            }
        }
        normalized.push(edit);
    }

    let any_mutation = normalized
        .iter()
        .any(|edit| edit.start != edit.end || !replacement.is_empty());
    if !any_mutation {
        return Change::None;
    }

    let mut delta = 0isize;
    let mut results: Vec<(usize, bool)> = Vec::with_capacity(normalized.len());
    for edit in &normalized {
        let position = (edit.start as isize + delta + replacement.len() as isize) as usize;
        results.push((position, edit.primary));
        delta += replacement.len() as isize - (edit.end - edit.start) as isize;
    }
    for edit in normalized.iter().rev() {
        value.replace_range(edit.start..edit.end, replacement);
    }

    let primary_index = results
        .iter()
        .position(|(_, primary)| *primary)
        .unwrap_or(0);
    let primary = results.remove(primary_index).0;
    *caret = primary;
    *anchor = primary;
    secondary.clear();
    secondary.extend(
        results
            .into_iter()
            .map(|(position, _)| TextSelection::collapsed(position)),
    );
    Change::Text
}

pub(crate) fn toggle_additional_caret(
    caret: &mut usize,
    anchor: &mut usize,
    secondary: &mut SmallVec<[TextSelection; 4]>,
    at: usize,
) -> Change {
    if let Some(index) = secondary
        .iter()
        .position(|selection| *selection == TextSelection::collapsed(at))
    {
        secondary.remove(index);
        return Change::Caret;
    }
    if *caret == at && *anchor == at {
        if let Some(promoted) = secondary.pop() {
            *caret = promoted.caret;
            *anchor = promoted.anchor;
            return Change::Caret;
        }
        return Change::None;
    }
    secondary.push(TextSelection::collapsed(at));
    Change::Caret
}

impl EditState {
    /// Replaces the selection (or splices at the caret) with `t`.
    fn insert(&mut self, t: &str) -> Change {
        replace_selections(
            &mut self.value,
            &mut self.caret,
            &mut self.anchor,
            &mut self.secondary,
            t,
            ReplaceMode::Selection,
        )
    }

    fn apply(&mut self, key: &EditKey) -> Change {
        match key {
            EditKey::Insert(t) => self.insert(t),
            EditKey::Backspace => replace_selections(
                &mut self.value,
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                "",
                ReplaceMode::Backspace,
            ),
            EditKey::Delete => replace_selections(
                &mut self.value,
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                "",
                ReplaceMode::Delete,
            ),
            EditKey::Left { select, word } => {
                // A plain Left over a selection collapses to its start (standard
                // editor behavior); otherwise step a char (or word) boundary.
                map_selections(
                    &mut self.caret,
                    &mut self.anchor,
                    &mut self.secondary,
                    |selection| {
                        let range = selection.range();
                        let target = if !*select && !*word && selection.caret != selection.anchor {
                            range.0
                        } else if *word {
                            prev_word(&self.value, selection.caret)
                        } else {
                            prev_boundary(&self.value, selection.caret)
                        };
                        TextSelection {
                            caret: target,
                            anchor: if *select { selection.anchor } else { target },
                        }
                    },
                )
            }
            EditKey::Right { select, word } => map_selections(
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                |selection| {
                    let range = selection.range();
                    let target = if !*select && !*word && selection.caret != selection.anchor {
                        range.1
                    } else if *word {
                        next_word(&self.value, selection.caret)
                    } else {
                        next_boundary(&self.value, selection.caret)
                    };
                    TextSelection {
                        caret: target,
                        anchor: if *select { selection.anchor } else { target },
                    }
                },
            ),
            EditKey::Home { select } => map_selections(
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                |selection| TextSelection {
                    caret: 0,
                    anchor: if *select { selection.anchor } else { 0 },
                },
            ),
            EditKey::End { select } => {
                let end = self.value.len();
                map_selections(
                    &mut self.caret,
                    &mut self.anchor,
                    &mut self.secondary,
                    |selection| TextSelection {
                        caret: end,
                        anchor: if *select { selection.anchor } else { end },
                    },
                )
            }
            // Single-line: no line to move to, no newline to insert (SOUL §8.1;
            // the multi-line semantics live in `text_area`).
            EditKey::Up { .. } | EditKey::Down { .. } | EditKey::Enter => Change::None,
            EditKey::SelectAll => {
                let before = (self.caret, self.anchor);
                let had_secondary = !self.secondary.is_empty();
                self.secondary.clear();
                self.anchor = 0;
                self.caret = self.value.len();
                if (self.caret, self.anchor) == before && !had_secondary {
                    Change::None
                } else {
                    Change::Caret
                }
            }
        }
    }
}

/// Registers a text input's edit state at build (caret parked at the end).
pub(crate) fn register_edit_state(
    runtime: &crate::Runtime,
    id: WidgetId,
    value: String,
    placeholder: String,
    size_px: f32,
    scale: f32,
    width: Option<f32>,
    password: bool,
) {
    let end = value.len();
    let label_progress = if placeholder.is_empty() || value.is_empty() {
        0.0
    } else {
        1.0
    };
    runtime.with(|rt| {
        rt.borrow_mut().edits.insert(
            id,
            EditState {
                value,
                password,
                placeholder,
                caret: end,
                anchor: end,
                size_px,
                scale,
                width,
                pointer_origin: None,
                secondary: SmallVec::new(),
                label_progress,
                label_target: label_progress,
            },
        );
    });
}

// ---------------------------------------------------------------------------
// the framework-owned edit event (windowing-toolkit-agnostic, SOUL §6.3)
// ---------------------------------------------------------------------------

/// A text-editing key, already resolved from platform modifiers by the caller
/// (the windowed loop or a test). `Insert` carries the typed text (one keystroke's
/// worth — possibly multi-byte, never control characters).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildCtx, TextInput, View};
    use schnellui_layout::LayoutEngine;
    use schnellui_scene::LayoutBox;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn build_input(
        runtime: &crate::Runtime,
        view: TextInput,
    ) -> (Scene, LayoutEngine, TextShaper, GlyphAtlas, WidgetId) {
        crate::reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let id = {
            let mut ctx = BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            (Box::new(view) as Box<dyn View>).build(&mut ctx, None)
        };
        scene.set_root(id);
        (scene, layout, text, atlas, id)
    }

    fn edit_value(runtime: &crate::Runtime, id: WidgetId) -> String {
        runtime.with(|rt| rt.borrow().edits.get(id).map(|e| e.value.clone()).unwrap())
    }

    fn caret_anchor(runtime: &crate::Runtime, id: WidgetId) -> (usize, usize) {
        runtime.with(|rt| {
            let rt = rt.borrow();
            let e = rt.edits.get(id).unwrap();
            (e.caret, e.anchor)
        })
    }

    fn label_motion(runtime: &crate::Runtime, id: WidgetId) -> (f32, f32) {
        runtime.with(|rt| {
            let rt = rt.borrow();
            let e = rt.edits.get(id).unwrap();
            (e.label_progress, e.label_target)
        })
    }

    fn all_selections(runtime: &crate::Runtime, id: WidgetId) -> Vec<(usize, usize)> {
        runtime.with(|rt| {
            let rt = rt.borrow();
            let e = rt.edits.get(id).unwrap();
            selection_list(e.caret, e.anchor, &e.secondary)
                .into_iter()
                .map(|selection| (selection.caret, selection.anchor))
                .collect()
        })
    }

    fn has_caret_line(scene: &Scene, id: WidgetId) -> bool {
        scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::Line { .. }))
    }

    #[test]
    fn build_registers_edit_state_and_paints_field_surface() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) =
            build_input(runtime, TextInput::new("hi").placeholder("name"));
        assert_eq!(edit_value(runtime, id), "hi");
        assert_eq!(caret_anchor(runtime, id), (2, 2)); // caret parked at the end
        let prims = &scene.paint(id).unwrap().primitives;
        // border + background surfaces, then the value's glyph quads
        assert!(matches!(prims[0], Primitive::SolidRect { .. }));
        assert!(matches!(prims[1], Primitive::SolidRect { .. }));
        assert!(prims
            .iter()
            .any(|p| matches!(p, Primitive::GlyphQuad { .. })));
        // unfocused ⇒ no caret
        assert!(!has_caret_line(&scene, id));
    }

    #[test]
    fn password_display_mapping_tracks_graphemes_without_exposing_text() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let value = "a\u{301}🙂";
        assert_eq!(obscured_value(value), "••");
        assert_eq!(display_byte_for_value_byte(value, 0, true), 0);
        assert_eq!(
            display_byte_for_value_byte(value, "a\u{301}".len(), true),
            PASSWORD_GLYPH.len()
        );
        assert_eq!(
            value_byte_for_display_byte(value, PASSWORD_GLYPH.len(), true),
            "a\u{301}".len()
        );
        assert_eq!(
            value_byte_for_display_byte(value, 2 * PASSWORD_GLYPH.len(), true),
            value.len()
        );
    }

    #[test]
    fn focus_is_exclusive_and_draws_caret() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        crate::reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let (a, b) = {
            let mut ctx = BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            let a = (Box::new(TextInput::new("aa")) as Box<dyn View>).build(&mut ctx, Some(root));
            let b = (Box::new(TextInput::new("bb")) as Box<dyn View>).build(&mut ctx, Some(root));
            (a, b)
        };
        assert!(dispatch_focus(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            Some(a)
        ));
        assert_eq!(schnellui_a11y::focused(&scene), Some(a));
        assert!(has_caret_line(&scene, a));
        // moving focus clears the old holder (exclusivity) and its caret
        assert!(dispatch_focus(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            Some(b)
        ));
        assert_eq!(schnellui_a11y::focused(&scene), Some(b));
        assert!(!has_caret_line(&scene, a));
        assert!(has_caret_line(&scene, b));
        // blur
        assert!(dispatch_focus(
            runtime, &mut scene, &mut text, &mut atlas, None
        ));
        assert_eq!(schnellui_a11y::focused(&scene), None);
        // no-op blur reports no change
        assert!(!dispatch_focus(
            runtime, &mut scene, &mut text, &mut atlas, None
        ));
    }

    #[test]
    fn placeholder_floats_on_focus_and_returns_when_empty_and_blurred() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_input(runtime, TextInput::new("").label("Project name"));
        assert_eq!(label_motion(runtime, id), (0.0, 0.0));
        assert!(!has_floating_label_animations(runtime,));

        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        assert_eq!(label_motion(runtime, id), (0.0, 1.0));
        assert!(has_floating_label_animations(runtime,));
        assert!(tick_floating_labels(
            runtime, &mut scene, &mut text, &mut atlas
        ));
        let (progress, target) = label_motion(runtime, id);
        assert!(progress > 0.0 && progress < 1.0);
        assert_eq!(target, 1.0);

        for _ in 0..16 {
            tick_floating_labels(runtime, &mut scene, &mut text, &mut atlas);
        }
        assert_eq!(label_motion(runtime, id), (1.0, 1.0));
        assert!(!has_floating_label_animations(runtime,));

        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, None);
        assert_eq!(label_motion(runtime, id), (1.0, 0.0));
        assert!(finish_floating_label_animations(
            runtime, &mut scene, &mut text, &mut atlas
        ));
        assert_eq!(label_motion(runtime, id), (0.0, 0.0));
    }

    #[test]
    fn populated_input_builds_with_its_label_already_floated() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) =
            build_input(runtime, TextInput::new("Schnell").label("Project name"));
        assert_eq!(label_motion(runtime, id), (1.0, 1.0));
        assert_eq!(
            scene.a11y(id).unwrap().name.as_deref(),
            Some("Project name")
        );
        assert!(!has_floating_label_animations(runtime,));
    }

    #[test]
    fn remount_inherits_label_progress_but_keeps_the_replacement_target() {
        let previous_runtime = crate::Runtime::new();
        let (mut previous_scene, _l, mut previous_text, mut previous_atlas, previous_id) =
            build_input(&previous_runtime, TextInput::new("").label("Project name"));
        dispatch_focus(
            &previous_runtime,
            &mut previous_scene,
            &mut previous_text,
            &mut previous_atlas,
            Some(previous_id),
        );
        finish_floating_label_animations(
            &previous_runtime,
            &mut previous_scene,
            &mut previous_text,
            &mut previous_atlas,
        );
        assert_eq!(label_motion(&previous_runtime, previous_id), (1.0, 1.0));

        let replacement_runtime = crate::Runtime::new();
        let (_scene, _l, _text, _atlas, replacement_id) = build_input(
            &replacement_runtime,
            TextInput::new("").label("Project name"),
        );
        assert!(inherit_edit_selection(
            &replacement_runtime,
            replacement_id,
            &previous_runtime,
            previous_id,
        ));
        assert_eq!(
            label_motion(&replacement_runtime, replacement_id),
            (1.0, 0.0),
            "the replacement target must still reflect its currently unfocused state"
        );
    }

    #[test]
    fn non_focusable_target_cannot_take_focus() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) = build_input(runtime, TextInput::new("x"));
        let label = scene.insert(WidgetKind::Text, Some(id));
        assert!(!dispatch_focus(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            Some(label)
        ));
        assert_eq!(schnellui_a11y::focused(&scene), None);
    }

    #[test]
    fn typing_inserts_at_caret_updates_a11y_and_fires_on_input() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = seen.clone();
        let (mut scene, _l, mut text, mut atlas, id) = build_input(
            runtime,
            TextInput::new("ab").on_input(move |v| s2.borrow_mut().push(v.into())),
        );
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("c")
        ));
        assert_eq!(edit_value(runtime, id), "abc");
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("abc"));
        // caret sits after the insertion; insert mid-string honors it
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Left {
                select: false,
                word: false
            }
        ));
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("X")
        ));
        assert_eq!(edit_value(runtime, id), "abXc");
        assert_eq!(*seen.borrow(), vec!["abc".to_string(), "abXc".to_string()]);
    }

    #[test]
    fn selection_replace_and_backspace() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_input(runtime, TextInput::new("hello"));
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        // select-all + type replaces the whole value
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::SelectAll
        ));
        // a focused range selection paints a selection wash (3rd SolidRect)
        let washes = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter(|p| matches!(p, Primitive::SolidRect { .. }))
            .count();
        assert_eq!(washes, 3, "border + bg + selection wash");
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("x")
        ));
        assert_eq!(edit_value(runtime, id), "x");
        // backspace deletes the last char; at the start it is a no-op
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Backspace
        ));
        assert_eq!(edit_value(runtime, id), "");
        assert!(!dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Backspace
        ));
    }

    #[test]
    fn clipboard_text_uses_document_order_and_unicode_boundaries() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let value = "café tea";
        let secondary = [TextSelection {
            caret: 0,
            anchor: 5,
        }];
        assert_eq!(
            selection_text(value, value.len(), 6, &secondary).as_deref(),
            Some("café\ntea")
        );
        assert_eq!(selection_text(value, 0, 0, &[]), None);
    }

    #[test]
    fn caret_motion_respects_char_boundaries() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        // 'é' is 2 bytes — arrows must land on char boundaries, never split one
        let (mut scene, _l, mut text, mut atlas, id) = build_input(runtime, TextInput::new("aé"));
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        assert_eq!(caret_anchor(runtime, id), (3, 3));
        let left = EditKey::Left {
            select: false,
            word: false,
        };
        dispatch_edit_key(runtime, &mut scene, &mut text, &mut atlas, id, left);
        assert_eq!(caret_anchor(runtime, id), (1, 1));
        dispatch_edit_key(runtime, &mut scene, &mut text, &mut atlas, id, left);
        assert_eq!(caret_anchor(runtime, id), (0, 0));
        // at the start Left is a no-op
        assert!(!dispatch_edit_key(
            runtime, &mut scene, &mut text, &mut atlas, id, left
        ));
        // shift+right extends a selection (anchor stays)
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Right {
                select: true,
                word: false,
            },
        );
        assert_eq!(caret_anchor(runtime, id), (1, 0));
        // deleting the selection leaves the 'é'
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Backspace,
        );
        assert_eq!(edit_value(runtime, id), "é");
    }

    #[test]
    fn word_motion_and_home_end() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_input(runtime, TextInput::new("one two"));
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Home { select: false },
        );
        assert_eq!(caret_anchor(runtime, id), (0, 0));
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Right {
                select: false,
                word: true,
            },
        );
        assert_eq!(caret_anchor(runtime, id).0, 4, "past 'one' and the space");
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Left {
                select: false,
                word: true,
            },
        );
        assert_eq!(caret_anchor(runtime, id).0, 0);
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::End { select: true },
        );
        assert_eq!(
            caret_anchor(runtime, id),
            (7, 0),
            "shift+End selects to the end"
        );
    }

    #[test]
    fn pointer_places_caret_and_drag_selects() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) = build_input(runtime, TextInput::new("abc"));
        // give the node a laid-out rect so pointer → inline offset resolves
        let rect = Rect::new(10.0, 10.0, 160.0, 28.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        // press at the far left of the text box ⇒ caret 0
        assert!(dispatch_text_pointer(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            Point {
                x: rect.x + PAD_H,
                y: 20.0
            },
            false,
        ));
        assert_eq!(caret_anchor(runtime, id), (0, 0));
        // drag far right ⇒ selection 0..len
        assert!(dispatch_text_pointer(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            Point {
                x: rect.x + rect.width,
                y: 20.0
            },
            true,
        ));
        assert_eq!(caret_anchor(runtime, id), (3, 0));
    }

    #[test]
    fn unicode_word_ranges_keep_words_punctuation_and_space_distinct() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let value = "naïve, café";
        assert_eq!(word_range_at(value, 2), (0, 6));
        assert_eq!(word_range_at(value, 6), (6, 7));
        assert_eq!(word_range_at(value, 7), (7, 8));
        assert_eq!(word_range_at(value, 10), (8, 13));
        assert_eq!(word_range_at(value, value.len()), (13, 13));
    }

    #[test]
    fn double_click_selects_word_and_word_drag_preserves_the_origin() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let value = "one two three";
        let (mut scene, _l, mut text, mut atlas, id) = build_input(runtime, TextInput::new(value));
        let rect = Rect::new(10.0, 10.0, 220.0, 28.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));

        let shaped = text.shape(value, phys_size_px(crate::BUTTON_TEXT_SIZE, 1.0), None);
        let final_word_glyph = shaped.glyphs.iter().find(|g| g.cluster == 6).unwrap();
        let in_two = Point {
            // The right half of the final glyph is nearest to the caret after
            // the word, but it is still geometrically over the word.
            x: rect.x
                + input_pads(runtime, id).0
                + advance_before(&shaped, 6)
                + final_word_glyph.x_advance * 0.75,
            y: rect.y + rect.height * 0.5,
        };
        assert!(dispatch_text_pointer_action(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            in_two,
            TextPointerAction::SelectWord,
        ));
        assert_eq!(caret_anchor(runtime, id), (7, 4));

        assert!(dispatch_text_pointer_action(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            Point {
                x: rect.x,
                y: in_two.y,
            },
            TextPointerAction::Drag,
        ));
        assert_eq!(caret_anchor(runtime, id), (0, 7));

        assert!(dispatch_text_pointer_action(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            Point {
                x: rect.x + rect.width,
                y: in_two.y,
            },
            TextPointerAction::Drag,
        ));
        assert_eq!(caret_anchor(runtime, id), (value.len(), 4));
    }

    #[test]
    fn additional_carets_edit_navigate_paint_and_clear_together() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let value = "abc";
        let (mut scene, _l, mut text, mut atlas, id) = build_input(runtime, TextInput::new(value));
        let rect = Rect::new(10.0, 10.0, 180.0, 28.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));

        let left = Point {
            x: rect.x + input_pads(runtime, id).0,
            y: rect.y + rect.height * 0.5,
        };
        assert!(dispatch_text_pointer_action(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            left,
            TextPointerAction::AddCaret,
        ));
        assert_eq!(all_selections(runtime, id), vec![(3, 3), (0, 0)]);
        let carets = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter(
                |primitive| matches!(primitive, Primitive::Line { width, .. } if *width == CARET_W),
            )
            .count();
        assert_eq!(carets, 2);

        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("X"),
        ));
        assert_eq!(edit_value(runtime, id), "XabcX");
        assert_eq!(all_selections(runtime, id), vec![(5, 5), (1, 1)]);

        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Left {
                select: false,
                word: false,
            },
        ));
        assert_eq!(all_selections(runtime, id), vec![(4, 4), (0, 0)]);

        // A regular click exits multi-cursor mode.
        assert!(dispatch_text_pointer(
            runtime, &mut scene, &mut text, &mut atlas, id, left, false,
        ));
        assert_eq!(all_selections(runtime, id), vec![(0, 0)]);
    }

    #[test]
    fn backspace_applies_to_every_caret_without_offset_drift() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let mut value = "abc".to_string();
        let mut caret = 3;
        let mut anchor = 3;
        let mut secondary = SmallVec::from_slice(&[TextSelection::collapsed(1)]);
        assert_eq!(
            replace_selections(
                &mut value,
                &mut caret,
                &mut anchor,
                &mut secondary,
                "",
                ReplaceMode::Backspace,
            ),
            Change::Text,
        );
        assert_eq!(value, "b");
        assert_eq!((caret, anchor), (1, 1));
        assert_eq!(secondary.as_slice(), &[TextSelection::collapsed(0)]);
    }

    #[test]
    fn set_text_value_replaces_and_notifies() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = seen.clone();
        let (mut scene, _l, mut text, mut atlas, id) = build_input(
            runtime,
            TextInput::new("old").on_input(move |v| s2.borrow_mut().push(v.into())),
        );
        assert!(set_text_value(
            runtime, &mut scene, &mut text, &mut atlas, id, "new"
        ));
        assert_eq!(edit_value(runtime, id), "new");
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("new"));
        assert_eq!(*seen.borrow(), vec!["new".to_string()]);
        // unchanged value is a no-op (no duplicate on_input)
        assert!(!set_text_value(
            runtime, &mut scene, &mut text, &mut atlas, id, "new"
        ));
        assert_eq!(seen.borrow().len(), 1);
    }
}
