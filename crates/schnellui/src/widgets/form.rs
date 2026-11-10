//! # Forms
//!
//! A [`Form`] is a column of fields with an `on_submit` handler. A submit button
//! ([`Button::submit`]) or Enter in a single-line input with no handler of its own
//! asks the nearest enclosing form to submit. Pointer, keyboard and AccessKit
//! `Click` all reach the same [`dispatch_click`] (SOUL §6.3), which only records
//! the request: the check needs the text shaper to repaint fields and the app to
//! move focus, so [`crate::App`] runs it right after the input, or at the start of
//! the next frame for a host that dispatches through the widget API directly.
//!
//! A submit walks the form's required fields in tree order, reading the live
//! values (the retained edit state and checked bit, not what the app authored).
//! If any is missing each gets [`StateFlags::INVALID`] and the first one takes
//! keyboard focus with the ring, the path Tab takes. If none is, `INVALID` is
//! cleared and `on_submit` runs once. Fields in a nested form belong to that
//! form. Disabled and hidden fields are skipped, as a browser skips them.
//!
//! The check runs on submit only; nothing here is touched per frame.

use super::*;

/// A column of fields that can be submitted (SOUL §8.1). The container is
/// laid out like a [`Column`] and carries [`Role::Form`].
///
/// ```ignore
/// Form::new()
///     .label("Contact")
///     .on_submit(move || send())
///     .child(TextInput::new("").label("Name").required(true))
///     .child(Button::new("Send").submit())
/// ```
pub struct Form {
    column: Column,
    name: Option<Cow<'static, str>>,
    on_submit: Option<ClickHandler>,
}

impl Form {
    pub fn new() -> Form {
        Form {
            column: Column::new(),
            name: None,
            on_submit: None,
        }
    }

    /// Gives the form an accessible name.
    pub fn label(mut self, name: impl Into<Cow<'static, str>>) -> Form {
        self.name = Some(name.into());
        self
    }

    /// Runs `f` once when the form is submitted with every required field filled.
    pub fn on_submit(mut self, f: impl FnMut() + 'static) -> Form {
        self.on_submit = Some(Box::new(f));
        self
    }

    /// Appends a child (SOUL §3.3 `.child(…)`).
    pub fn child(mut self, c: impl View) -> Form {
        self.column = self.column.child(c);
        self
    }

    /// Sets the gap between children, as [`Column::gap`] does.
    pub fn gap(mut self, gap: f32) -> Form {
        self.column = self.column.gap(gap);
        self
    }

    /// Sets main-axis distribution, as [`Column::justify`] does.
    pub fn justify(mut self, justify: Justify) -> Form {
        self.column = self.column.justify(justify);
        self
    }

    /// Sets cross-axis alignment, as [`Column::align`] does.
    pub fn align(mut self, align: Align) -> Form {
        self.column = self.column.align(align);
        self
    }

    /// Sizes the form to its parent's content box, as [`Column::fill`] does.
    pub fn fill(mut self) -> Form {
        self.column = self.column.fill();
        self
    }

    /// Fixes the outer box, as [`Column::size`] does.
    pub fn size(mut self, width: f32, height: f32) -> Form {
        self.column = self.column.size(width, height);
        self
    }

    /// Fixes the width only, as [`Column::width`] does.
    pub fn width(mut self, width: f32) -> Form {
        self.column = self.column.width(width);
        self
    }

    /// Fixes the height only, as [`Column::height`] does.
    pub fn height(mut self, height: f32) -> Form {
        self.column = self.column.height(height);
        self
    }

    /// Sets the minimum outer width, as [`Column::min_width`] does.
    pub fn min_width(mut self, width: f32) -> Form {
        self.column = self.column.min_width(width);
        self
    }

    /// Sets the minimum outer height, as [`Column::min_height`] does.
    pub fn min_height(mut self, height: f32) -> Form {
        self.column = self.column.min_height(height);
        self
    }

    /// Overrides the container style.
    pub fn style(mut self, style: ContainerStyle) -> Form {
        self.column = self.column.style(style);
        self
    }

    pub fn role(&self) -> Role {
        Role::Form
    }

    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Column
    }
}

impl Default for Form {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Form {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = Box::new(this.column).build(ctx, parent);
        {
            let semantics = ctx.scene.a11y_mut(id);
            semantics.role = Role::Form.as_u16();
            semantics.name = this.name.map(Cow::into_owned);
        }
        if let Some(on_submit) = this.on_submit {
            // The form's own `submit` slot is the one a text input's Enter uses, so
            // `dispatch_submit` runs it with the same disabled guard.
            with_handlers(&ctx.runtime, id, |h| h.submit = Some(on_submit));
        }
        id
    }
}

fn is_form(scene: &Scene, id: WidgetId) -> bool {
    scene
        .a11y(id)
        .is_some_and(|a| Role::from_u16(a.role) == Role::Form)
}

/// The nearest [`Form`] strictly above `id`.
pub(crate) fn enclosing_form(scene: &Scene, id: WidgetId) -> Option<WidgetId> {
    let mut current = scene.node(id)?.parent;
    while let Some(ancestor) = current {
        if is_form(scene, ancestor) {
            return Some(ancestor);
        }
        current = scene.node(ancestor)?.parent;
    }
    None
}

/// Asks `form` to submit. Recorded rather than run, see the module docs.
pub(crate) fn request_submit(runtime: &Runtime, form: WidgetId) {
    runtime.with(|rt| rt.borrow_mut().pending_submit = Some(form));
}

/// Takes the submit [`request_submit`] recorded, if any.
pub fn take_pending_submit(runtime: &Runtime) -> Option<WidgetId> {
    runtime.with(|rt| rt.borrow_mut().pending_submit.take())
}

/// Whether Enter in the text input `id` submits an enclosing form: it has no
/// `on_submit` of its own and sits inside a form. Returns that form.
pub(crate) fn implicit_submit_form(
    runtime: &Runtime,
    scene: &Scene,
    id: WidgetId,
) -> Option<WidgetId> {
    let own = runtime.with(|rt| {
        rt.borrow()
            .handlers
            .get(id)
            .is_some_and(|h| h.submit.is_some())
    });
    if own {
        return None;
    }
    enclosing_form(scene, id)
}

/// How a submit ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// Every required field had a value; the form's `on_submit` ran.
    Submitted,
    /// These fields were missing: `first` (the first in tree order) should take
    /// focus, and every missing field is now [`StateFlags::INVALID`].
    Blocked { first: WidgetId },
}

/// The required fields of `form` in tree order, skipping nested forms' fields.
fn required_fields(scene: &Scene, form: WidgetId) -> SmallVec<[WidgetId; 8]> {
    fn walk(scene: &Scene, id: WidgetId, out: &mut SmallVec<[WidgetId; 8]>) {
        let Some(node) = scene.node(id) else { return };
        for &child in &node.children {
            if !scene.is_visible(child) || is_form(scene, child) {
                continue;
            }
            if let Some(a) = scene.a11y(child) {
                let state = StateFlags(a.state);
                if state.contains(StateFlags::REQUIRED) && !state.contains(StateFlags::DISABLED) {
                    out.push(child);
                }
            }
            walk(scene, child, out);
        }
    }
    let mut out = SmallVec::new();
    walk(scene, form, &mut out);
    out
}

/// Whether a required field has no value, from its live runtime state.
fn is_missing(runtime: &Runtime, scene: &Scene, id: WidgetId) -> bool {
    match scene.node(id).map(|n| n.kind) {
        Some(WidgetKind::TextInput) => runtime.with(|rt| {
            rt.borrow()
                .edits
                .get(id)
                .is_some_and(|edit| edit.value.is_empty())
        }),
        Some(WidgetKind::TextArea) => runtime.with(|rt| {
            rt.borrow()
                .areas
                .get(id)
                .is_some_and(|area| area.value.is_empty())
        }),
        Some(WidgetKind::Checkbox) => !scene
            .a11y(id)
            .is_some_and(|a| StateFlags(a.state).contains(StateFlags::CHECKED)),
        _ => false,
    }
}

/// Sets or clears [`StateFlags::INVALID`] and repaints the field's outline.
/// Returns whether anything changed.
fn set_invalid(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    invalid: bool,
) -> bool {
    let mut state = StateFlags(scene.a11y(id).map_or(0, |a| a.state));
    if state.contains(StateFlags::INVALID) == invalid {
        return false;
    }
    if invalid {
        state.insert(StateFlags::INVALID);
    } else {
        state.0 &= !StateFlags::INVALID.0;
    }
    scene.set_a11y_state(id, state.0);
    match scene.node(id).map(|n| n.kind) {
        Some(WidgetKind::TextInput) => {
            emit_text_input_paint(runtime, scene, shaper, atlas, id);
        }
        Some(WidgetKind::TextArea) => {
            emit_text_area_paint(runtime, scene, shaper, atlas, id);
            // A splice keeps the ring the area wears, so recolor it.
            reapply_focus_ring(runtime, scene, id);
        }
        Some(WidgetKind::Checkbox) => {
            emit_checkbox_paint(runtime, scene, id, state.contains(StateFlags::CHECKED));
            redecorate_after_refill(runtime, scene, id);
        }
        _ => {}
    }
    scene.mark_dirty(id, DirtyFlags::PAINT);
    true
}

/// Clears `INVALID` on a field that just received a value, so the error outline
/// does not outlive the mistake. The caller repaints right after; this only
/// flips the bit, so the repaint it already does shows the clean outline.
/// Returns whether it was set.
pub(crate) fn clear_invalid_on_value(scene: &mut Scene, id: WidgetId) -> bool {
    let state = scene.a11y(id).map_or(0, |a| a.state);
    let was = state & StateFlags::INVALID.0 != 0;
    if was {
        scene.set_a11y_state(id, state & !StateFlags::INVALID.0);
    }
    was
}

/// Validates and submits `form` (see the module docs). On success `on_submit`
/// runs once, after every `INVALID` is cleared.
pub fn submit_form(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    form: WidgetId,
) -> SubmitOutcome {
    let fields = required_fields(scene, form);
    let mut first = None;
    for &field in &fields {
        let missing = is_missing(runtime, scene, field);
        set_invalid(runtime, scene, shaper, atlas, field, missing);
        if missing && first.is_none() {
            first = Some(field);
        }
    }
    if let Some(first) = first {
        return SubmitOutcome::Blocked { first };
    }
    let _ = dispatch_submit(runtime, scene, form);
    SubmitOutcome::Submitted
}
