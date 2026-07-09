use super::*;

const DRAG_HANDLE_SIZE: Size = Size {
    width: 18.0,
    height: 22.0,
};

pub(crate) fn emit_drag_handle(runtime: &Runtime, scene: &mut Scene, id: WidgetId, visible: bool) {
    let rect = scene
        .layout(id)
        .map(|layout| layout.rect)
        .unwrap_or(Rect::new(
            0.0,
            0.0,
            DRAG_HANDLE_SIZE.width,
            DRAG_HANDLE_SIZE.height,
        ));
    let color = theme_for(runtime, id).text_muted;
    let paint = scene.paint_mut(id);
    paint.primitives.clear();
    if !visible {
        return;
    }
    for row in 0..3 {
        for column in 0..2 {
            paint.primitives.push(Primitive::SolidRect {
                rect: Rect::new(
                    rect.x + 5.0 + column as f32 * 6.0,
                    rect.y + 5.0 + row as f32 * 6.0,
                    3.0,
                    3.0,
                ),
                color,
                corner_radius: 1.5,
            });
        }
    }
}

/// A compact six-dot pointer drag handle that stays visually hidden until the
/// cursor comes within its configured proximity. Its layout and hit area remain
/// stable while hidden, so revealing it never causes a relayout.
pub struct DragHandle {
    name: Cow<'static, str>,
    reveal_distance: f32,
    on_drag_start: Option<ClickHandler>,
    on_drag_end: Option<Box<dyn FnMut(bool) + 'static>>,
}

impl DragHandle {
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            name: name.into(),
            reveal_distance: 20.0,
            on_drag_start: None,
            on_drag_end: None,
        }
    }

    pub fn reveal_distance(mut self, distance: f32) -> Self {
        self.reveal_distance = distance.max(0.0);
        self
    }

    pub fn on_drag_start(mut self, callback: impl FnMut() + 'static) -> Self {
        self.on_drag_start = Some(Box::new(callback));
        self
    }

    pub fn on_drag_end(mut self, callback: impl FnMut(bool) + 'static) -> Self {
        self.on_drag_end = Some(Box::new(callback));
        self
    }
}

impl View for DragHandle {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Icon, parent);
        {
            let semantics = ctx.scene.a11y_mut(id);
            semantics.role = Role::Group.as_u16();
            semantics.name = Some(this.name.into_owned());
        }
        ctx.layout
            .set_measure(id, Box::new(|_available| DRAG_HANDLE_SIZE));
        emit_drag_handle(&ctx.runtime, ctx.scene, id, false);
        ctx.runtime.with(|runtime| {
            let mut runtime = runtime.borrow_mut();
            runtime.proximity_reveals.insert(
                id,
                ProximityRevealState {
                    distance: this.reveal_distance,
                    visible: false,
                },
            );
        });
        with_handlers(&ctx.runtime, id, |handlers| {
            handlers.drag_start = this.on_drag_start;
            handlers.drag_end = this.on_drag_end;
        });
        id
    }
}

/// Visual emphasis for a [`Button`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonAppearance {
    /// The ordinary high-emphasis accent-filled button.
    #[default]
    Solid,
    /// A transparent, text-colored action suitable for toolbars and tab rows.
    Ghost,
}

/// A push button (SOUL §8.1). Carries `Role::Button`; its `on_click` is the same
/// handler an inbound AccessKit `Click` action fires (SOUL §6.3).
pub struct Button {
    pub(crate) label: Cow<'static, str>,
    pub(crate) on_click: Option<ClickHandler>,
    pub(crate) on_drag_start: Option<ClickHandler>,
    pub(crate) on_drag_end: Option<Box<dyn FnMut(bool) + 'static>>,
    pub(crate) on_drop: Option<ClickHandler>,
    pub(crate) disabled: bool,
    pub(crate) width: Option<f32>,
    pub(crate) height: Option<f32>,
    pub(crate) appearance: ButtonAppearance,
    pub(crate) show_label: bool,
    pub(crate) tooltip: Option<Cow<'static, str>>,
    pub(crate) text_size: Option<f32>,
}

impl Button {
    /// A button with a static label.
    pub fn new(label: impl Into<Cow<'static, str>>) -> Button {
        Button {
            label: label.into(),
            on_click: None,
            on_drag_start: None,
            on_drag_end: None,
            on_drop: None,
            disabled: false,
            width: None,
            height: None,
            appearance: ButtonAppearance::Solid,
            show_label: true,
            tooltip: None,
            text_size: None,
        }
    }

    /// Sets the click handler (SOUL §6.3 — shared with the a11y action path).
    pub fn on_click(mut self, f: impl FnMut() + 'static) -> Button {
        self.on_click = Some(Box::new(f));
        self
    }

    /// Makes this button a pointer drag source.
    pub fn on_drag_start(mut self, f: impl FnMut() + 'static) -> Button {
        self.on_drag_start = Some(Box::new(f));
        self
    }

    /// Runs after a real drag ends, with whether a drop target accepted it.
    pub fn on_drag_end(mut self, f: impl FnMut(bool) + 'static) -> Button {
        self.on_drag_end = Some(Box::new(f));
        self
    }

    /// Makes this button a drop target. It receives a visible preview ring while
    /// an active drag hovers it.
    pub fn on_drop(mut self, f: impl FnMut() + 'static) -> Button {
        self.on_drop = Some(Box::new(f));
        self
    }

    /// Disables the button (reflected as `StateFlags::DISABLED`).
    pub fn disabled(mut self, disabled: bool) -> Button {
        self.disabled = disabled;
        self
    }

    /// Sets a minimum outer width for the button and centers its label within it.
    /// This is useful for keypads and toolbars whose controls form aligned columns.
    pub fn width(mut self, width: f32) -> Button {
        self.width = Some(width);
        self
    }

    /// Sets a minimum outer height for the button.
    pub fn height(mut self, height: f32) -> Button {
        self.height = Some(height);
        self
    }

    /// Selects the visual emphasis without changing button semantics or behavior.
    pub fn appearance(mut self, appearance: ButtonAppearance) -> Button {
        self.appearance = appearance;
        self
    }

    /// Hides the painted label while retaining it as the button's accessible
    /// name. Pair this with a decorative icon and an explicit compact size.
    pub fn icon_only(mut self) -> Button {
        self.show_label = false;
        self
    }

    /// Shows a short visual label while the pointer hovers the button. The
    /// button's ordinary label remains its screen-reader name; callers should use
    /// matching wording so sighted and non-sighted users receive the same name.
    pub fn tooltip(mut self, label: impl Into<Cow<'static, str>>) -> Button {
        self.tooltip = Some(label.into());
        self
    }

    /// Overrides the label font size (logical points before DPI scaling).
    /// Defaults to the widget crate's standard button label size.
    pub fn text_size(mut self, size: f32) -> Button {
        self.text_size = Some(size);
        self
    }

    pub fn role(&self) -> Role {
        Role::Button
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Button
    }
    /// `true` once a handler has been attached.
    pub fn has_handler(&self) -> bool {
        self.on_click.is_some()
    }
}

impl View for Button {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Button, parent);
        let label: String = this.label.into_owned();
        {
            // Semantics declared at definition (SOUL §6.1): role + name + supported
            // actions + disabled state.
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::Button.as_u16();
            a.name = Some(label.clone());
            let mut acts = ActionFlags::default();
            acts.insert(ActionFlags::CLICK);
            acts.insert(ActionFlags::FOCUS);
            a.actions = acts.0;
            if this.disabled {
                let mut st = StateFlags::default();
                st.insert(StateFlags::DISABLED);
                a.state = st.0;
            }
        }
        // Shape the label once → background SolidRect + real glyph quads, and the
        // glyph-exact label size that drives the intrinsic measure (SOUL §8.1).
        let ts = emit_button_paint(
            &ctx.runtime,
            ctx.scene,
            ctx.text,
            ctx.atlas,
            id,
            &label,
            this.show_label,
            this.disabled,
            this.width,
            this.height,
            this.appearance,
            this.text_size,
            ctx.scale,
        );
        // Shape tokens are baked here (theme reads are build-time, SOUL §8.1);
        // a theme change remounts, so the closure never goes stale.
        let intrinsic = sized_button_intrinsic(
            &ctx.runtime,
            id,
            ts,
            this.width,
            this.height,
            this.appearance,
        );
        if let Some(tooltip) = this.tooltip {
            register_hover_tooltip(
                &ctx.runtime,
                ctx.scene,
                ctx.text,
                ctx.atlas,
                id,
                &tooltip,
                intrinsic,
                ctx.scale,
            );
        }
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        // Store the handler under this node's id — the same key the ActionRequest
        // router resolves, so pointer and a11y `Click` converge (SOUL §6.3).
        if let Some(cb) = this.on_click {
            with_handlers(&ctx.runtime, id, |h| h.click = Some(cb));
        }
        if this.on_drag_start.is_some() || this.on_drag_end.is_some() || this.on_drop.is_some() {
            with_handlers(&ctx.runtime, id, |handlers| {
                handlers.drag_start = this.on_drag_start;
                handlers.drag_end = this.on_drag_end;
                handlers.drop = this.on_drop;
            });
        }
        id
    }
}

/// A toggle checkbox (SOUL §8.1). `Role::CheckBox`, `StateFlags::CHECKED`.
pub struct Checkbox {
    pub(crate) checked: bool,
    pub(crate) name: Option<Cow<'static, str>>,
    pub(crate) on_toggle: Option<Box<dyn FnMut(bool) + 'static>>,
}

impl Checkbox {
    pub fn new(checked: bool) -> Checkbox {
        Checkbox {
            checked,
            name: None,
            on_toggle: None,
        }
    }
    /// Gives the checkbox an accessible name.
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> Checkbox {
        self.name = Some(name.into());
        self
    }
    pub fn on_toggle(mut self, f: impl FnMut(bool) + 'static) -> Checkbox {
        self.on_toggle = Some(Box::new(f));
        self
    }
    pub fn role(&self) -> Role {
        Role::CheckBox
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Checkbox
    }
}

impl View for Checkbox {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Checkbox, parent);
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::CheckBox.as_u16();
            a.name = this.name.map(Cow::into_owned);
            let mut acts = ActionFlags::default();
            acts.insert(ActionFlags::CLICK);
            acts.insert(ActionFlags::FOCUS);
            a.actions = acts.0;
            let mut st = StateFlags::default();
            if this.checked {
                st.insert(StateFlags::CHECKED);
            }
            a.state = st.0;
        }
        emit_checkbox_paint(&ctx.runtime, ctx.scene, id, this.checked);
        let intrinsic = checkbox_intrinsic(&ctx.runtime, id);
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        if let Some(t) = this.on_toggle {
            with_handlers(&ctx.runtime, id, |h| h.toggle = Some(t));
        }
        id
    }
}

/// A value slider (SOUL §8.1). `Role::Slider` with min/max/now + Increment/Decrement.
pub struct Slider {
    pub(crate) value: f32,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) step: Option<f32>,
    pub(crate) name: Option<Cow<'static, str>>,
    pub(crate) disabled: bool,
    pub(crate) on_change: Option<Box<dyn FnMut(f32) + 'static>>,
}

impl Slider {
    pub fn new(value: f32, min: f32, max: f32) -> Slider {
        Slider {
            value,
            min,
            max,
            step: None,
            name: None,
            disabled: false,
            on_change: None,
        }
    }
    /// Sets the value increment used by arrows, assistive actions, and pointer
    /// snapping. Invalid/non-positive values fall back to 1% of the range.
    pub fn step(mut self, step: f32) -> Slider {
        self.step = Some(step);
        self
    }
    /// Gives the range input an accessible name.
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> Slider {
        self.name = Some(name.into());
        self
    }
    /// Makes the slider inert and exposes the disabled semantic state.
    pub fn disabled(mut self, disabled: bool) -> Slider {
        self.disabled = disabled;
        self
    }
    pub fn on_change(mut self, f: impl FnMut(f32) + 'static) -> Slider {
        self.on_change = Some(Box::new(f));
        self
    }
    pub fn role(&self) -> Role {
        Role::Slider
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Slider
    }
}

impl View for Slider {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Slider, parent);
        let min = if this.min.is_finite() { this.min } else { 0.0 };
        let max = if this.max.is_finite() && this.max > min {
            this.max
        } else {
            min
        };
        let default_step = ((max - min) * SLIDER_STEP_FRACTION).max(f32::EPSILON);
        let step = this
            .step
            .filter(|step| step.is_finite() && *step > 0.0)
            .unwrap_or(default_step);
        let value = if this.value.is_finite() {
            this.value
        } else {
            min
        };
        let now = quantize_slider(value, min, max, step);
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::Slider.as_u16();
            a.name = this.name.map(Cow::into_owned);
            a.value = Some(format_slider_value(now));
            if this.disabled {
                let mut state = StateFlags::default();
                state.insert(StateFlags::DISABLED);
                a.state = state.0;
            } else {
                let mut acts = ActionFlags::default();
                acts.insert(ActionFlags::SET_VALUE);
                acts.insert(ActionFlags::INCREMENT);
                acts.insert(ActionFlags::DECREMENT);
                acts.insert(ActionFlags::FOCUS);
                a.actions = acts.0;
            }
        }
        // Retained range state — what a keyboard arrow / AccessKit Increment
        // adjusts through [`dispatch_adjust`] (SOUL §6.3).
        ctx.runtime.with(|rt| {
            rt.borrow_mut().sliders.insert(
                id,
                SliderState {
                    value: now,
                    min,
                    max,
                    step,
                },
            );
        });
        // Track surface + filled portion up to `now` (SOUL §8.1).
        let frac = if max > min {
            ((now - min) / (max - min)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        emit_slider_paint(&ctx.runtime, ctx.scene, id, frac, this.disabled);
        let intrinsic = slider_intrinsic(&ctx.runtime, id);
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        if let Some(c) = this.on_change {
            with_handlers(&ctx.runtime, id, |h| h.change = Some(c));
        }
        id
    }
}

/// A single-line text input (SOUL §8.1). `Role::TextInput`; IME composition surfaces
/// into the retained text node (SOUL §6.3). A label rests in the empty field like a
/// placeholder, then floats upward when the field is focused or contains a value.
pub struct TextInput {
    pub(crate) value: String,
    pub(crate) placeholder: Cow<'static, str>,
    pub(crate) on_input: Option<InputHandler>,
    pub(crate) context_menu: Option<ContextMenu>,
    pub(crate) width: Option<f32>,
    pub(crate) password: bool,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> TextInput {
        TextInput {
            value: value.into(),
            placeholder: Cow::Borrowed(""),
            on_input: None,
            context_menu: None,
            width: None,
            password: false,
        }
    }
    /// Sets the Material-style floating label.
    ///
    /// Retained as the original `placeholder` spelling for API compatibility:
    /// unlike a disposable hint, this text remains visible after entry begins.
    pub fn placeholder(mut self, p: impl Into<Cow<'static, str>>) -> TextInput {
        self.placeholder = p.into();
        self
    }
    /// Sets the Material-style floating label.
    pub fn label(self, label: impl Into<Cow<'static, str>>) -> TextInput {
        self.placeholder(label)
    }
    pub fn on_input(mut self, f: impl FnMut(&str) + 'static) -> TextInput {
        self.on_input = Some(Box::new(f));
        self
    }
    /// Sets a minimum painted width for the field.
    pub fn width(mut self, width: f32) -> TextInput {
        self.width = width.is_finite().then_some(width.max(0.0));
        self
    }
    /// Replaces the standard Cut/Copy/Paste/Select All context menu.
    pub fn context_menu(mut self, menu: ContextMenu) -> TextInput {
        self.context_menu = Some(menu);
        self
    }
    /// Appends one command to the standard text-editing context menu.
    pub fn context_menu_item(mut self, item: ContextMenuItem) -> TextInput {
        self.context_menu
            .get_or_insert_with(ContextMenu::default_text)
            .push(item);
        self
    }
    pub fn role(&self) -> Role {
        Role::TextInput
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::TextInput
    }
}

impl View for TextInput {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::TextInput, parent);
        let placeholder: String = this.placeholder.into_owned();
        let context_menu = this.context_menu.unwrap_or_else(ContextMenu::default_text);
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = if this.password {
                Role::PasswordInput.as_u16()
            } else {
                Role::TextInput.as_u16()
            };
            a.value = Some(if this.password {
                text_edit::obscured_value(&this.value)
            } else {
                this.value.clone()
            });
            // The persistent floating label is the accessible name (SOUL §6.1).
            if !placeholder.is_empty() {
                a.name = Some(placeholder.clone());
            }
            let mut acts = ActionFlags::default();
            acts.insert(ActionFlags::FOCUS);
            acts.insert(ActionFlags::SET_VALUE);
            if !context_menu.is_empty() {
                acts.insert(ActionFlags::SHOW_CONTEXT_MENU);
            }
            a.actions = acts.0;
        }
        // Retained edit state (value + caret/anchor) — the paint emission below and
        // the focus/keyboard/pointer dispatches all read it (SOUL §6.3).
        text_edit::register_edit_state(
            &ctx.runtime,
            id,
            this.value.clone(),
            placeholder,
            BUTTON_TEXT_SIZE,
            ctx.scale,
            this.width,
            this.password,
        );
        context_menu::register_context_menu(&ctx.runtime, id, context_menu);
        let intrinsic =
            text_edit::emit_text_input_paint(&ctx.runtime, ctx.scene, ctx.text, ctx.atlas, id);
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        if let Some(i) = this.on_input {
            with_handlers(&ctx.runtime, id, |h| h.input = Some(i));
        }
        id
    }
}

/// A protected single-line input. It shares [`TextInput`]'s editing behavior,
/// while rendering bullets and exposing `Role::PasswordInput` to assistive
/// technology. Editing and [`PasswordInput::on_input`] retain the real value.
pub struct PasswordInput(TextInput);

impl PasswordInput {
    pub fn new(value: impl Into<String>) -> PasswordInput {
        let mut input = TextInput::new(value);
        input.password = true;
        PasswordInput(input)
    }
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> PasswordInput {
        self.0.placeholder = placeholder.into();
        self
    }
    pub fn label(self, label: impl Into<Cow<'static, str>>) -> PasswordInput {
        self.placeholder(label)
    }
    pub fn on_input(mut self, f: impl FnMut(&str) + 'static) -> PasswordInput {
        self.0.on_input = Some(Box::new(f));
        self
    }
    pub fn width(mut self, width: f32) -> PasswordInput {
        self.0.width = width.is_finite().then_some(width.max(0.0));
        self
    }
    pub fn context_menu(mut self, menu: ContextMenu) -> PasswordInput {
        self.0.context_menu = Some(menu);
        self
    }
    pub fn context_menu_item(mut self, item: ContextMenuItem) -> PasswordInput {
        self.0
            .context_menu
            .get_or_insert_with(ContextMenu::default_text)
            .push(item);
        self
    }
    pub fn role(&self) -> Role {
        Role::PasswordInput
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::TextInput
    }
}

impl View for PasswordInput {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        Box::new(self.0).build(ctx, parent)
    }
}
