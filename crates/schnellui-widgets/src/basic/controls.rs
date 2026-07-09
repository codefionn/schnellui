use super::*;

pub struct ProgressBar {
    pub(crate) value: f32,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) name: Option<Cow<'static, str>>,
    pub(crate) size: Size,
}

impl ProgressBar {
    /// A progress bar showing `value` within `[min, max]`.
    pub fn new(value: f32, min: f32, max: f32) -> ProgressBar {
        ProgressBar {
            value,
            min,
            max,
            name: None,
            size: Size {
                width: PROGRESS_WIDTH,
                height: PROGRESS_HEIGHT,
            },
        }
    }
    /// Gives the progress indicator an accessible task name.
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> ProgressBar {
        self.name = Some(name.into());
        self
    }
    /// Overrides the bar's intrinsic size in logical pixels.
    pub fn size(mut self, width: f32, height: f32) -> ProgressBar {
        self.size = Size {
            width: width.max(1.0),
            height: height.max(1.0),
        };
        self
    }
    pub fn role(&self) -> Role {
        Role::ProgressIndicator
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::ProgressBar
    }
}

impl View for ProgressBar {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::ProgressBar, parent);
        let frac = clamp_fraction(this.value, this.min, this.max);
        let pct = frac * 100.0;
        {
            // Semantics declared at definition (SOUL §6.1): role + the percentage as
            // the accessible value. No actions — a progress bar is not interactive.
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::ProgressIndicator.as_u16();
            a.name = this.name.map(Cow::into_owned);
            a.value = Some(format!("{}%", pct.round()));
        }
        let intrinsic = this.size;
        emit_progress_paint(&ctx.runtime, ctx.scene, id, frac, intrinsic);
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        id
    }
}

// ---------------------------------------------------------------------------
// LoadingSpinner (SOUL §8.1 — an indeterminate progress indicator)
// ---------------------------------------------------------------------------

/// An animated indeterminate progress indicator. It exposes
/// `Role::ProgressIndicator` with a default accessible name of "Loading" and no
/// actions. Windowed apps repaint it at the display cadence; headless callers get
/// a deterministic frame and may call [`crate::tick_loading_spinners`] explicitly.
pub struct LoadingSpinner {
    pub(crate) size: f32,
    pub(crate) name: Cow<'static, str>,
    /// Initial loop progress in `[0,1)` of the shared rotation declaration.
    pub(crate) phase: f32,
    pub(crate) animated: bool,
}

impl Default for LoadingSpinner {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadingSpinner {
    pub fn new() -> LoadingSpinner {
        LoadingSpinner {
            size: SPINNER_SIZE,
            name: Cow::Borrowed("Loading"),
            phase: 0.0,
            animated: true,
        }
    }
    /// Sets the square intrinsic size in logical pixels.
    pub fn size(mut self, size: f32) -> LoadingSpinner {
        self.size = size.max(8.0);
        self
    }
    /// Replaces the accessible task name.
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> LoadingSpinner {
        self.name = name.into();
        self
    }
    /// Selects the initial animation phase as a fraction of one full
    /// revolution (`0.0..1.0`, wrapping) — the loop progress of the shared
    /// [`SPINNER_MOTION`](crate::SPINNER_MOTION) declaration.
    pub fn phase(mut self, phase: u8) -> LoadingSpinner {
        self.phase = (phase as f32 / SPINNER_SEGMENTS as f32).fract();
        self
    }
    /// Disables automatic windowed animation while keeping the chosen frame.
    ///
    /// The window host also suppresses automatic animation globally when the
    /// platform's reduced-motion accessibility preference is enabled.
    pub fn animated(mut self, animated: bool) -> LoadingSpinner {
        self.animated = animated;
        self
    }
    pub fn role(&self) -> Role {
        Role::ProgressIndicator
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::LoadingSpinner
    }
}

impl View for LoadingSpinner {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::LoadingSpinner, parent);
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::ProgressIndicator.as_u16();
            a.name = Some(this.name.into_owned());
        }
        emit_spinner_paint(&ctx.runtime, ctx.scene, id, this.size, this.phase);
        let intrinsic = Size {
            width: this.size,
            height: this.size,
        };
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        ctx.runtime.with(|rt| {
            rt.borrow_mut().spinners.insert(
                id,
                SpinnerState {
                    phase: this.phase,
                    size: this.size,
                    animated: this.animated,
                },
            );
        });
        id
    }
}

// ---------------------------------------------------------------------------
// Switch (SOUL §8.1 — an on/off toggle, distinct from a checkbox)
// ---------------------------------------------------------------------------

/// An on/off switch (SOUL §8.1). `Role::Switch`, `StateFlags::CHECKED` when on; its
/// `on_toggle` is the same handler an inbound AccessKit `Click` fires (SOUL §6.3).
pub struct Switch {
    pub(crate) on: bool,
    pub(crate) on_toggle: Option<Box<dyn FnMut(bool) + 'static>>,
}

impl Switch {
    /// A switch in the given on/off state.
    pub fn new(on: bool) -> Switch {
        Switch {
            on,
            on_toggle: None,
        }
    }
    /// Sets the toggle handler, called with the switch's *new* state (SOUL §6.3 —
    /// shared with the a11y action path).
    pub fn on_toggle(mut self, f: impl FnMut(bool) + 'static) -> Switch {
        self.on_toggle = Some(Box::new(f));
        self
    }
    pub fn role(&self) -> Role {
        Role::Switch
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Switch
    }
}

impl View for Switch {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Switch, parent);
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::Switch.as_u16();
            let mut acts = ActionFlags::default();
            acts.insert(ActionFlags::CLICK);
            acts.insert(ActionFlags::FOCUS);
            a.actions = acts.0;
            let mut st = StateFlags::default();
            if this.on {
                st.insert(StateFlags::CHECKED);
            }
            a.state = st.0;
        }
        emit_switch_paint(&ctx.runtime, ctx.scene, id, this.on);
        let intrinsic = switch_intrinsic(&ctx.runtime, id);
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        if let Some(t) = this.on_toggle {
            with_handlers(&ctx.runtime, id, |h| h.toggle = Some(t));
        }
        id
    }
}

// ---------------------------------------------------------------------------
// Radio (SOUL §8.1 — one exclusive option of a group)
// ---------------------------------------------------------------------------

/// One option of a radio group (SOUL §8.1). `Role::Radio`, `StateFlags::CHECKED` when
/// selected. Selecting it clears the sibling radios (group exclusivity, SOUL §6.3) and
/// fires `on_select` — the same handler an inbound AccessKit `Click` fires.
pub struct Radio {
    pub(crate) selected: bool,
    pub(crate) on_select: Option<Box<dyn FnMut() + 'static>>,
}

impl Radio {
    /// A radio in the given selected state.
    pub fn new(selected: bool) -> Radio {
        Radio {
            selected,
            on_select: None,
        }
    }
    /// Sets the selection handler (SOUL §6.3 — shared with the a11y action path).
    pub fn on_select(mut self, f: impl FnMut() + 'static) -> Radio {
        self.on_select = Some(Box::new(f));
        self
    }
    pub fn role(&self) -> Role {
        Role::Radio
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Radio
    }
}

impl View for Radio {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Radio, parent);
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::Radio.as_u16();
            let mut acts = ActionFlags::default();
            acts.insert(ActionFlags::CLICK);
            acts.insert(ActionFlags::FOCUS);
            a.actions = acts.0;
            let mut st = StateFlags::default();
            if this.selected {
                st.insert(StateFlags::CHECKED);
            }
            a.state = st.0;
        }
        emit_radio_paint(&ctx.runtime, ctx.scene, id, this.selected);
        let intrinsic = radio_intrinsic(&ctx.runtime, id);
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        // The selection handler is stored as the node's `click` handler — the same key
        // the ActionRequest router resolves, so pointer + a11y `Click` converge (§6.3).
        if let Some(f) = this.on_select {
            with_handlers(&ctx.runtime, id, |h| h.click = Some(f));
        }
        id
    }
}

// ---------------------------------------------------------------------------
// Divider (SOUL §8.1 — a decorative separator)
// ---------------------------------------------------------------------------

/// A decorative separator hairline (SOUL §8.1). Transparent `Role::Group` — no name,
/// value, or actions (it is not interactive and announces nothing). Spans the full
/// available width; its thickness defaults to 1px.
pub struct Divider {
    pub(crate) thickness: f32,
}

impl Divider {
    /// A 1px separator.
    pub fn new() -> Divider {
        Divider { thickness: 1.0 }
    }
    /// Overrides the hairline thickness in logical pixels.
    pub fn thickness(mut self, thickness: f32) -> Divider {
        self.thickness = thickness;
        self
    }
    pub fn role(&self) -> Role {
        Role::Group
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Divider
    }
}

impl Default for Divider {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Divider {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Divider, parent);
        // Decorative: a transparent Group with no name / value / actions (SOUL §6.1).
        ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
        let thickness = this.thickness;
        emit_divider_paint(&ctx.runtime, ctx.scene, id, thickness);
        // A separator claims the full available width and its own thickness (SOUL §8.1).
        ctx.layout.set_measure(
            id,
            Box::new(move |avail: Size| Size {
                width: if avail.width.is_finite() {
                    avail.width
                } else {
                    0.0
                },
                height: thickness,
            }),
        );
        id
    }
}

// ---------------------------------------------------------------------------
// Link (SOUL §8.1 — an inline navigation action)
// ---------------------------------------------------------------------------

/// An inline navigation link (SOUL §8.1). `Role::Link` with the label as its
/// accessible name; its `on_click` is the same handler an inbound AccessKit `Click`
/// action fires (SOUL §6.3). Painted as blue underlined text — the underline is a
/// real hairline `SolidRect` riding the same node, so [`crate::reposition_paint`]
/// slides text and underline together.
pub struct Link {
    pub(crate) label: Cow<'static, str>,
    pub(crate) on_click: Option<ClickHandler>,
    pub(crate) disabled: bool,
}

impl Link {
    /// A link with a static label.
    pub fn new(label: impl Into<Cow<'static, str>>) -> Link {
        Link {
            label: label.into(),
            on_click: None,
            disabled: false,
        }
    }
    /// Sets the activation handler (SOUL §6.3 — shared with the a11y action path).
    pub fn on_click(mut self, f: impl FnMut() + 'static) -> Link {
        self.on_click = Some(Box::new(f));
        self
    }
    /// Disables the link (reflected as `StateFlags::DISABLED`; a disabled link is
    /// inert to dispatch, like a disabled button).
    pub fn disabled(mut self, disabled: bool) -> Link {
        self.disabled = disabled;
        self
    }
    pub fn role(&self) -> Role {
        Role::Link
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Link
    }
    /// `true` once a handler has been attached.
    pub fn has_handler(&self) -> bool {
        self.on_click.is_some()
    }
}

impl View for Link {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Link, parent);
        let label: String = this.label.into_owned();
        {
            // Semantics declared at definition (SOUL §6.1): role + name + Click/Focus.
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::Link.as_u16();
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
        // Shape once → glyph quads in link blue + an underline hairline spanning the
        // shaped run (SOUL §8.1). Glyphs sit at a local (0,0) origin; the frame's
        // reposition pass anchors the whole set onto the laid-out box.
        let inv = 1.0 / norm_scale(ctx.scale);
        let phys = phys_size_px(LINK_TEXT_SIZE, ctx.scale);
        let shaped = ctx.text.shape(&label, phys, None);
        let sz = Size {
            width: shaped.width * inv,
            height: shaped.height * inv,
        };
        let pd = ctx.scene.paint_mut(id);
        pd.primitives.clear();
        pd.primitives.push(Primitive::SolidRect {
            rect: Rect::new(
                0.0,
                sz.height + LINK_UNDERLINE_GAP,
                sz.width,
                LINK_UNDERLINE,
            ),
            color: theme_for(&ctx.runtime, id).accent,
            corner_radius: 0.0,
        });
        rasterize_and_push(
            pd,
            ctx.text,
            ctx.atlas,
            &shaped,
            phys as u32,
            theme_for(&ctx.runtime, id).accent,
            ctx.scale,
            Point { x: 0.0, y: 0.0 },
        );
        // Intrinsic size covers the text run plus its underline (SOUL §8.1).
        let intrinsic = Size {
            width: sz.width,
            height: sz.height + LINK_UNDERLINE_GAP + LINK_UNDERLINE,
        };
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        if let Some(cb) = this.on_click {
            with_handlers(&ctx.runtime, id, |h| h.click = Some(cb));
        }
        id
    }
}

// ---------------------------------------------------------------------------
// Badge (SOUL §8.1 — a short live-status pill, e.g. a notification count)
// ---------------------------------------------------------------------------

/// A short status pill (SOUL §8.1) — a notification count or "NEW" marker.
/// `Role::Status` with the text as its accessible **value** (a live region a screen
/// reader announces on change, §6.2). Not interactive — it advertises no actions.
pub struct Badge {
    pub(crate) text: Cow<'static, str>,
}

impl Badge {
    /// A badge showing the given short text.
    pub fn new(text: impl Into<Cow<'static, str>>) -> Badge {
        Badge { text: text.into() }
    }
    pub fn role(&self) -> Role {
        Role::Status
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Badge
    }
}

impl View for Badge {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Badge, parent);
        let text: String = this.text.into_owned();
        // Status semantics: the text is the accessible *value* (SOUL §6.1, §6.2).
        write_text_semantics(ctx.scene, id, Role::Status, &text);
        // Shape once → pill background + label glyphs inset by the badge padding.
        let inv = 1.0 / norm_scale(ctx.scale);
        let phys = phys_size_px(BADGE_TEXT_SIZE, ctx.scale);
        let shaped = ctx.text.shape(&text, phys, None);
        let ts = Size {
            width: shaped.width * inv,
            height: shaped.height * inv,
        };
        let t = theme_for(&ctx.runtime, id);
        let sh = t.shape;
        let (pad_h, pad_v) = (sh.pad(BADGE_PAD_H), sh.pad(BADGE_PAD_V));
        let intrinsic = Size {
            width: ts.width + 2.0 * (pad_h + sh.frame),
            height: ts.height + 2.0 * (pad_v + sh.frame),
        };
        let rect = node_rect(ctx.scene, id, intrinsic);
        // The pill: full-height corner radius rounds the ends (SOUL §8.1) — and
        // squares out into a stamp under a zero-roundness design ([`Shape::pill`]).
        let radius = sh.pill(intrinsic.height);
        let pd = ctx.scene.paint_mut(id);
        pd.primitives.clear();
        if sh.frame > 0.0 {
            pd.primitives.push(Primitive::SolidRect {
                rect,
                color: t.outline,
                corner_radius: radius,
            });
        }
        pd.primitives.push(Primitive::SolidRect {
            rect: Rect::new(
                rect.x + sh.frame,
                rect.y + sh.frame,
                (rect.width - 2.0 * sh.frame).max(0.0),
                (rect.height - 2.0 * sh.frame).max(0.0),
            ),
            color: t.attention,
            corner_radius: (radius - sh.frame).max(0.0),
        });
        rasterize_and_push(
            pd,
            ctx.text,
            ctx.atlas,
            &shaped,
            phys as u32,
            t.on_accent,
            ctx.scale,
            Point {
                x: rect.x + sh.frame + pad_h,
                y: rect.y + sh.frame + pad_v,
            },
        );
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        id
    }
}

// ---------------------------------------------------------------------------
// click / activation dispatch (SOUL §6.3 — one inbound path for pointer + a11y)
// ---------------------------------------------------------------------------

/// The click/activation dispatch hook for this module's widget kinds (SOUL §6.3),
/// called by [`dispatch_click`](crate::dispatch_click) for `Switch` and `Radio`.
/// Returns `true` if a target handler ran or state changed. The disabled check already
/// happened in the caller, so this never sees a disabled widget.
pub(crate) fn dispatch_click_basic(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    kind: WidgetKind,
) -> bool {
    match kind {
        WidgetKind::Switch => {
            // Flip the CHECKED bit, re-emit the paint for the new state, flag the two
            // channels it touched (SOUL §8.1), then fire `on_toggle` with the new bool.
            let now = toggle_checked(scene, id);
            emit_switch_paint(runtime, scene, id, now);
            // The re-emit cleared the primitives; a focused switch keeps its ring.
            crate::reapply_focus_ring(runtime, scene, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
            scene.mark_dirty(id, DirtyFlags::A11Y);
            // Take the handler out of the registry before running it — user code may
            // re-enter the runtime, and no registry borrow may be held across it (§3.1).
            let tog = runtime.with(|rt| {
                rt.borrow_mut()
                    .handlers
                    .get_mut(id)
                    .and_then(|h| h.toggle.take())
            });
            if let Some(mut t) = tog {
                t(now);
                runtime.with(|rt| {
                    if let Some(h) = rt.borrow_mut().handlers.get_mut(id) {
                        h.toggle = Some(t);
                    }
                });
            }
            true
        }
        WidgetKind::Radio => dispatch_radio(runtime, scene, id),
        _ => false,
    }
}

/// Selects one radio with group exclusivity (SOUL §6.3): clears every currently-checked
/// sibling radio (each re-painted + marked dirty), checks the clicked one if it was not
/// already, then fires its stored `on_select`. An already-selected radio still fires its
/// handler but does not re-dirty its own state. Returns `true` if a handler ran or the
/// clicked radio's state changed.
fn dispatch_radio(runtime: &crate::Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    // Radio-group exclusivity via the tree: clear any checked sibling radio (§6.3).
    if let Some(parent) = scene.node(id).and_then(|n| n.parent) {
        // Snapshot sibling ids first so we can mutate the scene while iterating (≤8
        // group members ⇒ zero heap, §4.4).
        let siblings: SmallVec<[WidgetId; 8]> = scene
            .node(parent)
            .map(|n| n.children.iter().copied().collect())
            .unwrap_or_default();
        for sib in siblings {
            if sib == id {
                continue;
            }
            let checked_radio = scene.node(sib).map(|n| n.kind) == Some(WidgetKind::Radio)
                && is_checked(scene, sib);
            if checked_radio {
                clear_checked(scene, sib);
                emit_radio_paint(runtime, scene, sib, false);
                scene.mark_dirty(sib, DirtyFlags::PAINT);
                scene.mark_dirty(sib, DirtyFlags::A11Y);
            }
        }
    }
    // Check the clicked radio if it was not already selected (no re-dirty otherwise).
    let was_checked = is_checked(scene, id);
    if !was_checked {
        set_checked(scene, id);
        emit_radio_paint(runtime, scene, id, true);
        // The re-emit cleared the primitives; a focused radio keeps its ring.
        crate::reapply_focus_ring(runtime, scene, id);
        scene.mark_dirty(id, DirtyFlags::PAINT);
        scene.mark_dirty(id, DirtyFlags::A11Y);
    }
    // Fire `on_select` (stored as the `click` handler), taken out of the registry
    // before it runs so no borrow is held across user code (§3.1).
    let cb = runtime.with(|rt| {
        rt.borrow_mut()
            .handlers
            .get_mut(id)
            .and_then(|h| h.click.take())
    });
    let fired = cb.is_some();
    if let Some(mut cb) = cb {
        cb();
        runtime.with(|rt| {
            if let Some(h) = rt.borrow_mut().handlers.get_mut(id) {
                h.click = Some(cb);
            }
        });
    }
    fired || !was_checked
}
