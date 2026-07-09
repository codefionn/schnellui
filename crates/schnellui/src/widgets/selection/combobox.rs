use super::navigation::{CARET_HALF, CARET_RISE, CARET_STROKE};
use super::*;
use std::rc::Rc;

pub struct ComboBox {
    value: String,
    label: Cow<'static, str>,
    options: Vec<DropdownOption>,
    open: bool,
    allow_free_text: bool,
    width: Option<f32>,
    max_popup_height: Option<f32>,
    on_input: Option<InputHandler>,
    on_toggle: Option<ClickHandler>,
    on_accept_free_text: Option<InputHandler>,
}

/// A searchable combo-box suggestion. It shares the same selectable popup row
/// implementation as [`DropdownOption`].
pub type ComboBoxOption = DropdownOption;

/// Retained suggestion rows for one mounted combo box. The complete option set
/// is built once; ordinary search edits only change row visibility.
#[derive(Clone)]
pub(crate) struct ComboBoxState {
    /// Shared so the per-keystroke snapshot of this state is a refcount bump.
    options: Rc<[ComboBoxRow]>,
    custom: Option<WidgetId>,
    scale: f32,
    min_width: f32,
}

/// One retained suggestion row: its label and the lowercased label the search
/// compares against, computed once at mount instead of on every keystroke.
pub(crate) struct ComboBoxRow {
    id: WidgetId,
    label: String,
    lowercase: String,
}

impl ComboBox {
    /// Creates a controlled combo box with its current field value.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: Cow::Borrowed(""),
            options: Vec::new(),
            open: false,
            allow_free_text: false,
            width: None,
            max_popup_height: None,
            on_input: None,
            on_toggle: None,
            on_accept_free_text: None,
        }
    }

    /// Sets the persistent floating and accessible label.
    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label = label.into();
        self
    }

    /// Appends a selectable suggestion.
    pub fn option(mut self, option: DropdownOption) -> Self {
        self.options.push(option);
        self
    }

    /// Controls whether the suggestion popup is mounted.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Allows a value outside the suggestion list to be accepted.
    pub fn allow_free_text(mut self, allow: bool) -> Self {
        self.allow_free_text = allow;
        self
    }

    /// Sets a minimum field and popup width.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width.is_finite().then_some(width.max(0.0));
        self
    }

    /// Caps the suggestion popup's height. Taller suggestion lists scroll
    /// inside the popup instead of running past the window.
    pub fn max_popup_height(mut self, height: f32) -> Self {
        self.max_popup_height = height.is_finite().then_some(height.max(0.0));
        self
    }

    /// Fires whenever normal text editing changes the search text. Suggestion
    /// filtering is already handled in place, so this callback should only mirror
    /// state needed by the host and must not remount merely to refresh the popup.
    pub fn on_input(mut self, callback: impl FnMut(&str) + 'static) -> Self {
        self.on_input = Some(Box::new(callback));
        self
    }

    /// Fires when the collapsed field is activated or an expanded combo is
    /// dismissed. The host flips `open` and remounts.
    pub fn on_toggle(mut self, callback: impl FnMut() + 'static) -> Self {
        self.on_toggle = Some(Box::new(callback));
        self
    }

    /// Fires when the generated custom-value row is selected.
    pub fn on_accept_free_text(mut self, callback: impl FnMut(&str) + 'static) -> Self {
        self.on_accept_free_text = Some(Box::new(callback));
        self
    }

    pub fn role(&self) -> Role {
        Role::ComboBox
    }

    /// The editable leaf intentionally shares the text-input scene kind; its
    /// ComboBox role supplies the richer semantics while retaining the complete
    /// native editing path.
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::TextInput
    }

    pub fn option_count(&self) -> usize {
        self.options.len()
    }
}

impl View for ComboBox {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let runtime = &ctx.runtime;
        let mut this = *self;
        let wrapper = ctx.scene.insert(WidgetKind::Column, parent);
        ctx.layout
            .set_container(wrapper, ContainerStyle::new(Container::Column));

        let field = ctx.scene.insert(WidgetKind::TextInput, Some(wrapper));
        let label = this.label.into_owned();
        let scroll_label = this
            .max_popup_height
            .map(|_| format!("{label} suggestions"));
        let context_menu = ContextMenu::default_text();
        {
            let a = ctx.scene.a11y_mut(field);
            a.role = Role::ComboBox.as_u16();
            a.name = (!label.is_empty()).then(|| label.clone());
            a.value = Some(this.value.clone());
            let mut actions = ActionFlags::default();
            actions.insert(ActionFlags::FOCUS);
            actions.insert(ActionFlags::SET_VALUE);
            actions.insert(ActionFlags::CLICK);
            if !context_menu.is_empty() {
                actions.insert(ActionFlags::SHOW_CONTEXT_MENU);
            }
            a.actions = actions.0;
            let mut state = StateFlags::default();
            state.insert(StateFlags::COLLAPSIBLE);
            if this.open {
                state.insert(StateFlags::EXPANDED);
            }
            a.state = state.0;
        }
        crate::widgets::text_edit::register_edit_state(
            &ctx.runtime,
            field,
            this.value.clone(),
            label,
            BUTTON_TEXT_SIZE,
            ctx.scale,
            this.width,
            false,
        );
        crate::widgets::context_menu::register_context_menu(&ctx.runtime, field, context_menu);
        let intrinsic = crate::widgets::text_edit::emit_text_input_paint(
            &ctx.runtime,
            ctx.scene,
            ctx.text,
            ctx.atlas,
            field,
        );
        ctx.layout
            .set_measure(field, Box::new(move |_available| intrinsic));
        if let Some(callback) = this.on_input.take() {
            with_handlers(&ctx.runtime, field, |handlers| {
                handlers.input = Some(callback)
            });
        }
        if let Some(callback) = this.on_toggle.take() {
            with_handlers(&ctx.runtime, field, |handlers| {
                handlers.click = Some(callback)
            });
        }

        if this.open {
            let popup = ctx.scene.insert(WidgetKind::Column, Some(wrapper));
            let mut style = ContainerStyle::new(Container::Column);
            style.anchor = Some(Point {
                x: 0.0,
                y: intrinsic.height,
            });
            ctx.layout.set_container(popup, style);
            ctx.scene.set_overlay(popup);

            let normalized = this.value.trim().to_lowercase();
            let exact = this
                .options
                .iter()
                .any(|option| option.label.eq_ignore_ascii_case(this.value.trim()));
            let widest = this
                .options
                .iter()
                .map(|option| {
                    ctx.text
                        .shape(
                            option.label.as_ref(),
                            phys_size_px(BUTTON_TEXT_SIZE, ctx.scale),
                            None,
                        )
                        .width
                        / norm_scale(ctx.scale)
                })
                .fold(0.0f32, f32::max);
            let shape = theme_for(runtime, field).shape;
            let shared_width = this
                .width
                .unwrap_or_else(|| widest + 2.0 * (shape.pad(PAD_H) + shape.frame));
            // Rows go into a capped scroll viewport when a height limit is set.
            let rows = match (this.max_popup_height, scroll_label) {
                (Some(height), Some(name)) => {
                    let scroll =
                        Box::new(crate::widgets::Scroll::new().label(name)).build(ctx, Some(popup));
                    ctx.layout.set_flex(
                        scroll,
                        FlexChild {
                            max_height: Some(height),
                            ..FlexChild::default()
                        },
                    );
                    scroll
                }
                _ => popup,
            };
            let mut option_rows = Vec::with_capacity(this.options.len());
            for mut option in this.options {
                let label = option.label.as_ref().to_owned();
                let matches =
                    normalized.is_empty() || exact || label.to_lowercase().contains(&normalized);
                option.min_width = shared_width;
                let option_id = Box::new(option).build(ctx, Some(rows));
                ctx.layout.set_visible(ctx.scene, option_id, matches);
                let lowercase = label.to_lowercase();
                option_rows.push(ComboBoxRow {
                    id: option_id,
                    label,
                    lowercase,
                });
            }

            let custom = if this.allow_free_text {
                let mut custom = DropdownOption::new(format!("Use “{}”", this.value));
                custom.min_width = shared_width;
                if let Some(mut callback) = this.on_accept_free_text.take() {
                    let runtime = ctx.runtime.clone();
                    custom = custom.on_select(move || {
                        let value = crate::widgets::text_edit::edit_value(&runtime, field)
                            .unwrap_or_default();
                        callback(&value);
                    });
                }
                let custom_id = Box::new(custom).build(ctx, Some(rows));
                ctx.layout.set_visible(
                    ctx.scene,
                    custom_id,
                    !this.value.trim().is_empty() && !exact,
                );
                Some(custom_id)
            } else {
                None
            };
            ctx.runtime.with(|runtime| {
                runtime.borrow_mut().comboboxes.insert(
                    field,
                    ComboBoxState {
                        options: option_rows.into(),
                        custom,
                        scale: ctx.scale,
                        min_width: shared_width,
                    },
                );
            });
        }
        wrapper
    }
}

/// Filters a mounted combo box's retained suggestions after its editable value
/// changes. Returns `true` when layout must run again.
///
/// This is deliberately separate from `on_input`: the text editor paints the
/// keystroke first, the host callback may cheaply mirror the controlled value,
/// and then SchnellUI updates only this popup instead of remounting the host tree.
pub fn refresh_combobox_filter(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    layout: &mut LayoutEngine,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    field: WidgetId,
) -> bool {
    let Some(query) = crate::widgets::text_edit::edit_value(runtime, field) else {
        return false;
    };
    let Some(state) = runtime.with(|runtime| runtime.borrow().comboboxes.get(field).cloned())
    else {
        return false;
    };

    let trimmed = query.trim();
    let normalized = trimmed.to_lowercase();
    let exact = state
        .options
        .iter()
        .any(|row| row.label.eq_ignore_ascii_case(trimmed));
    let mut changed = false;
    for row in state.options.iter() {
        let visible = normalized.is_empty() || exact || row.lowercase.contains(normalized.as_str());
        changed |= layout.set_visible(scene, row.id, visible);
    }

    if let Some(custom) = state.custom {
        let visible = !trimmed.is_empty() && !exact;
        changed |= layout.set_visible(scene, custom, visible);
        let label = format!("Use “{query}”");
        let label_changed =
            scene.a11y(custom).and_then(|a11y| a11y.name.as_deref()) != Some(label.as_str());
        if label_changed {
            scene.a11y_mut(custom).name = Some(label.clone());
            let text_size = emit_selectable_paint(
                runtime,
                scene,
                shaper,
                atlas,
                custom,
                WidgetKind::DropdownOption,
                &label,
                false,
                state.scale,
                state.min_width,
                TabDisclosure::None,
            );
            let shape = theme_for(runtime, custom).shape;
            let (pad_h, pad_v) = (shape.pad(PAD_H), shape.pad(PAD_V));
            let frame = selectable_frame(runtime, custom, WidgetKind::DropdownOption);
            let min_width = state.min_width;
            layout.set_measure(
                custom,
                Box::new(move |_available| Size {
                    width: (text_size.width + 2.0 * (pad_h + frame)).max(min_width),
                    height: text_size.height + 2.0 * pad_v,
                }),
            );
            scene.mark_dirty(custom, DirtyFlags::PAINT);
            scene.mark_dirty(custom, DirtyFlags::A11Y);
            changed = true;
        }
    }
    changed
}

/// Appends the disclosure chevron after the normal text-input painter has
/// rebuilt an editable combo field.
/// The logical width the combobox arrow takes from the right of its text box, so
/// a scrolled value stops before the arrow instead of running under it.
pub(crate) fn combobox_caret_reserve(scene: &Scene, id: WidgetId) -> f32 {
    match scene.a11y(id) {
        Some(a11y) if Role::from_u16(a11y.role) == Role::ComboBox => 3.0 * CARET_HALF,
        _ => 0.0,
    }
}

pub(crate) fn append_combobox_caret(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    id: WidgetId,
) {
    let Some(a11y) = scene.a11y(id) else { return };
    if Role::from_u16(a11y.role) != Role::ComboBox {
        return;
    }
    let open = StateFlags(a11y.state).contains(StateFlags::EXPANDED);
    let Some(rect) = scene.layout(id).map(|layout| layout.rect).or_else(|| {
        scene.paint(id).and_then(|paint| {
            paint
                .primitives
                .iter()
                .find_map(|primitive| match primitive {
                    Primitive::SolidRect { rect, .. } => Some(*rect),
                    _ => None,
                })
        })
    }) else {
        return;
    };
    let theme = theme_for(runtime, id);
    let cx = rect.x + rect.width - theme.shape.pad(PAD_H) - CARET_HALF;
    let cy = rect.y + rect.height * 0.5;
    let (tip, base) = if open {
        (cy - CARET_RISE, cy + CARET_RISE)
    } else {
        (cy + CARET_RISE, cy - CARET_RISE)
    };
    let paint = scene.paint_mut(id);
    for (x0, x1) in [(cx - CARET_HALF, cx), (cx, cx + CARET_HALF)] {
        let (y0, y1) = if x0 < cx { (base, tip) } else { (tip, base) };
        paint.primitives.push(Primitive::Line {
            from: Point { x: x0, y: y0 },
            to: Point { x: x1, y: y1 },
            width: CARET_STROKE,
            color: theme.text,
        });
    }
}
