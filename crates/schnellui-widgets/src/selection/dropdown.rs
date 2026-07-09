use super::navigation::{CARET_HALF, CARET_RISE, CARET_SPACE, CARET_STROKE};
use super::*;

pub struct Dropdown {
    pub(crate) label: Cow<'static, str>,
    pub(crate) options: Vec<DropdownOption>,
    pub(crate) open: bool,
    pub(crate) on_toggle: Option<ClickHandler>,
}

impl Dropdown {
    /// A dropdown with an accessible label (what the control *is*, e.g. "Example";
    /// the trigger's visible text is the chosen option's label).
    pub fn new(label: impl Into<Cow<'static, str>>) -> Dropdown {
        Dropdown {
            label: label.into(),
            options: Vec::new(),
            open: false,
            on_toggle: None,
        }
    }
    /// Appends one option (SOUL §3.3 `.child(…)` discipline, typed to options).
    pub fn option(mut self, o: DropdownOption) -> Dropdown {
        self.options.push(o);
        self
    }
    /// Whether the option list is showing. Structural — set at build (SOUL §3.3).
    pub fn open(mut self, open: bool) -> Dropdown {
        self.open = open;
        self
    }
    /// Sets the trigger's activation handler (SOUL §6.3 — shared with the a11y
    /// action path). The host flips `open` and remounts.
    pub fn on_toggle(mut self, f: impl FnMut() + 'static) -> Dropdown {
        self.on_toggle = Some(Box::new(f));
        self
    }
    pub fn role(&self) -> Role {
        Role::ComboBox
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Dropdown
    }
    /// Number of options configured (pre-build).
    pub fn option_count(&self) -> usize {
        self.options.len()
    }
}

/// Emits the trigger's paint: optionally the ink frame (the dropdown is
/// input-family under the physical [`Shape`](crate::Shape) tokens — it wears
/// [`frame`](crate::Shape::frame), never the button's float), the surface
/// (washed with the theme's selection tint while open), the caret chevron as
/// two [`Primitive::Line`]s (pointing down closed, up open — same primitive
/// count either way), and the chosen option's label as real glyph quads.
/// `min_width` is the dropdown's shared width (widest option + padding +
/// caret), so the trigger keeps one size no matter which option is chosen and
/// the popup lines up flush beneath it. With the classic shape (frame zero)
/// the primitive list is exactly the legacy surface-caret-glyphs (SOUL §7.3).
/// Returns the label's logical text size for the intrinsic measure.
#[allow(clippy::too_many_arguments)]
fn emit_dropdown_trigger_paint(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    value: &str,
    open: bool,
    scale: f32,
    min_width: f32,
) -> Size {
    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(BUTTON_TEXT_SIZE, scale);
    let shaped = shaper.shape(value, phys, None);
    let ts = Size {
        width: shaped.width * inv,
        height: shaped.height * inv,
    };
    let t = theme_for(runtime, id);
    let (pad_h, pad_v) = (t.shape.pad(PAD_H), t.shape.pad(PAD_V));
    let f = t.shape.frame;
    let intrinsic = Size {
        width: (ts.width + 2.0 * (pad_h + f) + CARET_SPACE).max(min_width),
        height: ts.height + 2.0 * (pad_v + f),
    };
    let rect = node_rect(scene, id, intrinsic);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    // The ink frame behind an inset surface (same layering as the button); the
    // dropdown stays square in every design system — the open popup seams flush
    // against the trigger's bottom edge, so its corners can never round away.
    if f > 0.0 {
        pd.primitives.push(Primitive::SolidRect {
            rect,
            color: t.outline,
            corner_radius: 0.0,
        });
    }
    // [0] the surface — recolors like a selection state (open = the light wash).
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(
            rect.x + f,
            rect.y + f,
            (rect.width - 2.0 * f).max(0.0),
            (rect.height - 2.0 * f).max(0.0),
        ),
        color: if open { t.selection } else { t.surface },
        corner_radius: 0.0,
    });
    // [1][2] the caret chevron at the right edge: ∨ closed, ∧ open.
    let cx = rect.x + rect.width - f - pad_h - CARET_HALF;
    let cy = rect.y + rect.height * 0.5;
    let (tip, base) = if open {
        (cy - CARET_RISE, cy + CARET_RISE)
    } else {
        (cy + CARET_RISE, cy - CARET_RISE)
    };
    for (x0, x1) in [(cx - CARET_HALF, cx), (cx, cx + CARET_HALF)] {
        let (y0, y1) = if x0 < cx { (base, tip) } else { (tip, base) };
        pd.primitives.push(Primitive::Line {
            from: Point { x: x0, y: y0 },
            to: Point { x: x1, y: y1 },
            width: CARET_STROKE,
            color: t.text,
        });
    }
    rasterize_and_push(
        pd,
        shaper,
        atlas,
        &shaped,
        phys as u32,
        t.text,
        scale,
        Point {
            x: rect.x + f + pad_h,
            y: rect.y + f + pad_v,
        },
    );
    ts
}

impl View for Dropdown {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let runtime = &ctx.runtime;
        let this = *self;
        // The wrapper is a plain layout column (Group role, SOUL §8.1): the
        // trigger above the (open) option list. No new container kind needed.
        let wrap = ctx.scene.insert(WidgetKind::Column, parent);
        ctx.layout
            .set_container(wrap, ContainerStyle::new(Container::Column));
        let id = ctx.scene.insert(WidgetKind::Dropdown, Some(wrap));
        let label: String = this.label.into_owned();
        // The trigger shows the chosen option (falling back to the first).
        let value: String = this
            .options
            .iter()
            .find(|o| o.selected)
            .or(this.options.first())
            .map(|o| o.label.clone().into_owned())
            .unwrap_or_default();
        // One shared width for the trigger and every option — the widest label
        // plus the trigger's padding + caret. The trigger never resizes when the
        // choice changes, and the open popup is a single flush-edged opaque panel
        // instead of per-label ragged rows that let content bleed through.
        let inv = 1.0 / norm_scale(ctx.scale);
        let phys = phys_size_px(BUTTON_TEXT_SIZE, ctx.scale);
        let widest = this
            .options
            .iter()
            .map(|o| ctx.text.shape(o.label.as_ref(), phys, None).width * inv)
            .fold(0.0f32, f32::max);
        let sh = theme_for(runtime, id).shape;
        let (pad_h, pad_v) = (sh.pad(PAD_H), sh.pad(PAD_V));
        let f = sh.frame;
        let shared_width = widest + 2.0 * (pad_h + f) + CARET_SPACE;
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::ComboBox.as_u16();
            a.name = Some(label);
            a.value = Some(value.clone());
            let mut acts = ActionFlags::default();
            acts.insert(ActionFlags::CLICK);
            acts.insert(ActionFlags::FOCUS);
            a.actions = acts.0;
            let mut st = StateFlags(a.state);
            st.insert(StateFlags::COLLAPSIBLE);
            if this.open {
                st.insert(StateFlags::EXPANDED);
            }
            a.state = st.0;
        }
        let ts = emit_dropdown_trigger_paint(
            &ctx.runtime,
            ctx.scene,
            ctx.text,
            ctx.atlas,
            id,
            &value,
            this.open,
            ctx.scale,
            shared_width,
        );
        ctx.layout.set_measure(
            id,
            Box::new(move |_avail| Size {
                width: (ts.width + 2.0 * (pad_h + f) + CARET_SPACE).max(shared_width),
                height: ts.height + 2.0 * (pad_v + f),
            }),
        );
        if let Some(cb) = this.on_toggle {
            with_handlers(&ctx.runtime, id, |h| h.click = Some(cb));
        }
        // The option list exists only while open (SOUL §3.3): a closed dropdown's
        // skeleton has no option nodes — opening is a remount, like any structural
        // change. While open it is a **floating popup**, dialog-like: a column
        // anchored just below the trigger (out of flow — siblings never move) and
        // flagged as an overlay layer, so the renderer paints it above the content
        // it covers and hit-testing resolves into it first (SOUL §3.2 z-order).
        if this.open {
            let popup = ctx.scene.insert(WidgetKind::Column, Some(wrap));
            let mut style = ContainerStyle::new(Container::Column);
            style.anchor = Some(Point {
                x: 0.0,
                y: ts.height + 2.0 * (pad_v + f),
            });
            ctx.layout.set_container(popup, style);
            ctx.scene.set_overlay(popup);
            for mut opt in this.options {
                opt.min_width = shared_width;
                Box::new(opt).build(ctx, Some(popup));
            }
        }
        wrap
    }
}

/// One option of a [`Dropdown`] (SOUL §8.1). [`Role::ListBoxOption`],
/// `StateFlags::SELECTED` when chosen; selecting it clears the sibling options
/// (single-selection, SOUL §6.3) and fires `on_select` — the same handler an
/// inbound AccessKit `Click` fires.
pub struct DropdownOption {
    pub(crate) label: Cow<'static, str>,
    pub(crate) selected: bool,
    pub(crate) on_select: Option<ClickHandler>,
    /// The popup's shared width, stamped by [`Dropdown::build`] so every option
    /// row spans the same flush-edged panel width (0 = label-intrinsic).
    pub(crate) min_width: f32,
}

impl DropdownOption {
    /// An option with a static label.
    pub fn new(label: impl Into<Cow<'static, str>>) -> DropdownOption {
        DropdownOption {
            label: label.into(),
            selected: false,
            on_select: None,
            min_width: 0.0,
        }
    }
    /// Marks this option as the initially chosen one.
    pub fn selected(mut self, selected: bool) -> DropdownOption {
        self.selected = selected;
        self
    }
    /// Sets the selection handler (SOUL §6.3 — shared with the a11y action path).
    pub fn on_select(mut self, f: impl FnMut() + 'static) -> DropdownOption {
        self.on_select = Some(Box::new(f));
        self
    }
    pub fn role(&self) -> Role {
        Role::ListBoxOption
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::DropdownOption
    }
}

impl View for DropdownOption {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let runtime = &ctx.runtime;
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::DropdownOption, parent);
        let label: String = this.label.into_owned();
        write_selectable_semantics(ctx.scene, id, Role::ListBoxOption, &label, this.selected);
        let min_width = this.min_width;
        let ts = emit_selectable_paint(
            &ctx.runtime,
            ctx.scene,
            ctx.text,
            ctx.atlas,
            id,
            WidgetKind::DropdownOption,
            &label,
            this.selected,
            ctx.scale,
            min_width,
            TabDisclosure::None,
        );
        let sh = theme_for(runtime, id).shape;
        let (pad_h, pad_v) = (sh.pad(PAD_H), sh.pad(PAD_V));
        let f = selectable_frame(&ctx.runtime, id, WidgetKind::DropdownOption);
        ctx.layout.set_measure(
            id,
            Box::new(move |_avail| Size {
                width: (ts.width + 2.0 * (pad_h + f)).max(min_width),
                height: ts.height + 2.0 * pad_v,
            }),
        );
        if let Some(cb) = this.on_select {
            with_handlers(&ctx.runtime, id, |h| h.click = Some(cb));
        }
        id
    }
}

/// Writes a selectable leaf's build-time semantics (SOUL §6.1): role + name +
/// Click/Focus actions + the SELECTED bit.
pub(crate) fn write_selectable_semantics(
    scene: &mut Scene,
    id: WidgetId,
    role: Role,
    label: &str,
    selected: bool,
) {
    let a = scene.a11y_mut(id);
    a.role = role.as_u16();
    a.name = Some(label.to_string());
    let mut acts = ActionFlags::default();
    acts.insert(ActionFlags::CLICK);
    acts.insert(ActionFlags::FOCUS);
    a.actions = acts.0;
    if selected {
        let mut st = StateFlags::default();
        st.insert(StateFlags::SELECTED);
        a.state = st.0;
    }
}

// ---------------------------------------------------------------------------
// click / activation dispatch (SOUL §6.3 — one inbound path for pointer + a11y)
// ---------------------------------------------------------------------------

/// The click/activation dispatch hook for this module's widget kinds (SOUL §6.3),
/// called by [`dispatch_click`](crate::dispatch_click) for `Tab` and `ListItem`.
/// Selects exclusively: tabs clear every selected tab in their nearest semantic
/// tab list (including grouped/tree descendants), while list items and dropdown
/// options retain sibling exclusivity. The selected peers are recolored in place
/// and marked dirty, then the clicked node's stored `on_select` fires. An
/// already-selected tab/item still fires its handler but does not re-dirty its own
/// state. Returns `true` if a handler ran or the clicked node's state changed. The
/// disabled check already happened in the caller.
pub(crate) fn dispatch_click_selection(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    kind: WidgetKind,
) -> bool {
    // A plain TabBar and a GroupedTabList share Role::TabList. Searching by
    // semantics keeps the ordinary direct-child case fast while allowing grouped
    // and recursively nested tabs to remain one exclusive selection scope.
    let scope = if kind == WidgetKind::Tab {
        let mut current = scene.node(id).and_then(|node| node.parent);
        let mut tab_list = None;
        while let Some(candidate) = current {
            if scene
                .a11y(candidate)
                .is_some_and(|a| Role::from_u16(a.role) == Role::TabList)
            {
                tab_list = Some(candidate);
                break;
            }
            current = scene.node(candidate).and_then(|node| node.parent);
        }
        tab_list.or_else(|| scene.node(id).and_then(|node| node.parent))
    } else {
        scene.node(id).and_then(|node| node.parent)
    };

    // Snapshot only the selected peers before mutating the scene. Thirty-two tabs
    // stay inline; an unusually large navigation list may spill on interaction,
    // never on steady-state rendering.
    let mut selected_peers = SmallVec::<[WidgetId; 32]>::new();
    if let Some(scope) = scope {
        if kind == WidgetKind::Tab {
            collect_selected_descendants(scene, scope, id, kind, &mut selected_peers);
        } else if let Some(node) = scene.node(scope) {
            selected_peers.extend(node.children.iter().copied().filter(|&peer| {
                peer != id
                    && scene.node(peer).is_some_and(|node| node.kind == kind)
                    && is_selected(scene, peer)
            }));
        }
    }
    for peer in selected_peers {
        clear_selected(scene, peer);
        recolor_selection(runtime, scene, peer, kind, false);
        scene.mark_dirty(peer, DirtyFlags::PAINT);
        scene.mark_dirty(peer, DirtyFlags::A11Y);
    }
    // Select the clicked node if it was not already (no re-dirty otherwise).
    let was_selected = is_selected(scene, id);
    if !was_selected {
        set_selected(scene, id);
        recolor_selection(runtime, scene, id, kind, true);
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
    fired || !was_selected
}

fn collect_selected_descendants(
    scene: &Scene,
    parent: WidgetId,
    selected_id: WidgetId,
    kind: WidgetKind,
    out: &mut SmallVec<[WidgetId; 32]>,
) {
    let Some(node) = scene.node(parent) else {
        return;
    };
    for &child in &node.children {
        if child != selected_id
            && scene.node(child).is_some_and(|node| node.kind == kind)
            && is_selected(scene, child)
        {
            out.push(child);
        }
        collect_selected_descendants(scene, child, selected_id, kind, out);
    }
}

/// The click/activation dispatch hook for the dropdown kinds (SOUL §6.3), called
/// by [`dispatch_click`](crate::dispatch_click) for `Dropdown` and
/// `DropdownOption`.
///
/// The **trigger** only fires its stored toggle handler: showing/hiding the
/// option list is structural, so the host remounts with `open` flipped
/// (SOUL §3.3) — the same in-window remount an example switcher uses. An
/// **option** selects with sibling exclusivity exactly like a tab/list item,
/// then mirrors its label into the sibling trigger's accessible value
/// (SOUL §6.1), so the combo box announces the new choice without a remount.
pub(crate) fn dispatch_click_dropdown(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    kind: WidgetKind,
) -> bool {
    match kind {
        WidgetKind::Dropdown | WidgetKind::TextInput
            if scene
                .a11y(id)
                .is_some_and(|a| Role::from_u16(a.role) == Role::ComboBox) =>
        {
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
        WidgetKind::DropdownOption => {
            let acted = dispatch_click_selection(runtime, scene, id, WidgetKind::DropdownOption);
            if acted {
                // option → popup column → dropdown wrapper, whose children hold
                // the trigger leaf beside the popup.
                let trigger = scene
                    .node(id)
                    .and_then(|n| n.parent)
                    .and_then(|popup| scene.node(popup))
                    .and_then(|pn| pn.parent)
                    .and_then(|wrap| scene.node(wrap))
                    .and_then(|wn| {
                        wn.children.iter().copied().find(|&c| {
                            scene.node(c).is_some_and(|n| {
                                n.kind == WidgetKind::Dropdown
                                    || (n.kind == WidgetKind::TextInput
                                        && scene.a11y(c).is_some_and(|a| {
                                            Role::from_u16(a.role) == Role::ComboBox
                                        }))
                            })
                        })
                    });
                let name = scene.a11y(id).and_then(|a| a.name.clone());
                if let (Some(t), Some(name)) = (trigger, name) {
                    scene.set_a11y_value(t, Some(name));
                }
            }
            acted
        }
        _ => false,
    }
}

/// Dismisses every expanded dropdown that does not contain `interaction_target`.
///
/// Dropdown visibility is structural, so dismissal fires the same `on_toggle`
/// handler as the trigger and lets the host remount with `open = false`. Keeping
/// the target's dropdown open is important for interactions with its trigger and
/// options; a target outside every dropdown (including `None` for blank space)
/// dismisses all of them. Returns `true` when at least one toggle handler ran.
pub fn dismiss_open_dropdowns(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    interaction_target: Option<WidgetId>,
) -> bool {
    fn collect_expanded(scene: &Scene, id: WidgetId, out: &mut SmallVec<[WidgetId; 4]>) {
        let Some(node) = scene.node(id) else { return };
        if scene.a11y(id).is_some_and(|a| {
            Role::from_u16(a.role) == Role::ComboBox
                && StateFlags(a.state).contains(StateFlags::EXPANDED)
        }) {
            out.push(id);
        }
        for &child in &node.children {
            collect_expanded(scene, child, out);
        }
    }

    let Some(root) = scene.root() else {
        return false;
    };
    let mut expanded = SmallVec::<[WidgetId; 4]>::new();
    collect_expanded(scene, root, &mut expanded);

    let mut dismissed = false;
    for trigger in expanded {
        let Some(wrapper) = scene.node(trigger).and_then(|node| node.parent) else {
            continue;
        };
        if interaction_target.is_some_and(|target| crate::is_in_subtree(scene, target, wrapper)) {
            continue;
        }

        let callback = runtime.with(|runtime| {
            runtime
                .borrow_mut()
                .handlers
                .get_mut(trigger)
                .and_then(|handlers| handlers.click.take())
        });
        if let Some(mut callback) = callback {
            callback();
            runtime.with(|runtime| {
                if let Some(handlers) = runtime.borrow_mut().handlers.get_mut(trigger) {
                    handlers.click = Some(callback);
                }
            });
            dismissed = true;
        }
    }
    dismissed
}
