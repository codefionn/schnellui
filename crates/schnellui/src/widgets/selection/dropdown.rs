use super::navigation::{CARET_HALF, CARET_RISE, CARET_SPACE, CARET_STROKE};
use super::*;
use schnellui::layout::EdgeInsets;

pub struct Dropdown {
    pub(crate) label: Cow<'static, str>,
    pub(crate) options: Vec<DropdownOption>,
    pub(crate) open: bool,
    pub(crate) on_toggle: Option<ClickHandler>,
    pub(crate) text_size: Option<f32>,
    pub(crate) min_width: f32,
    pub(crate) placeholder: Option<Cow<'static, str>>,
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
            text_size: None,
            min_width: 0.0,
            placeholder: None,
        }
    }
    /// Shows `text` on the trigger while no option is selected, instead of
    /// the first option, for a menu of actions rather than a choice.
    pub fn placeholder(mut self, text: impl Into<Cow<'static, str>>) -> Dropdown {
        self.placeholder = Some(text.into());
        self
    }
    /// Sets the least logical width of the trigger and its option list, so
    /// the dropdowns of one form can line up. The widest option still wins.
    pub fn min_width(mut self, width: f32) -> Dropdown {
        self.min_width = if width.is_finite() {
            width.max(0.0)
        } else {
            0.0
        };
        self
    }
    /// Sets the logical size of the trigger's and the options' labels. The
    /// default is the button text size.
    pub fn text_size(mut self, size: f32) -> Dropdown {
        self.text_size = (size.is_finite() && size > 0.0).then_some(size);
        self
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
/// input-family under the physical [`Shape`](crate::widgets::Shape) tokens — it wears
/// [`frame`](crate::widgets::Shape::frame), never the button's float), the surface
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
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    value: &str,
    open: bool,
    scale: f32,
    min_width: f32,
    text_size: f32,
) -> Size {
    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(text_size, scale);
    let shaped = shaper.shape(value, phys, None);
    let ts = Size {
        width: shaped.width * inv,
        height: shaped.height * inv,
    };
    let t = theme_for(runtime, id);
    let (pad_h, pad_v) = (t.shape.pad(PAD_H), t.shape.pad(PAD_V));
    let f = t.shape.frame;
    let f = f.max(TRIGGER_BORDER);
    let intrinsic = Size {
        width: (ts.width + 2.0 * (pad_h + f) + CARET_SPACE).max(min_width),
        height: ts.height + 2.0 * (pad_v + f),
    };
    let rect = node_rect(scene, id, intrinsic);
    // A trigger is an outlined field, like a text input, so it reads as a
    // control rather than a label on the page; it wears the accent edge
    // while its list is open. The popup floats a little below it.
    let edge = f.max(TRIGGER_BORDER);
    let radius = t.shape.radius(4.0, rect.height);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: if open { t.focus_color() } else { t.outline },
        corner_radius: radius,
    });
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(
            rect.x + edge,
            rect.y + edge,
            (rect.width - 2.0 * edge).max(0.0),
            (rect.height - 2.0 * edge).max(0.0),
        ),
        color: t.surface,
        corner_radius: (radius - edge).max(0.0),
    });
    // [2][3] the caret chevron at the right edge: ∨ closed, ∧ open.
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

/// How a dropdown trigger was painted at build, kept so choosing an option
/// repaints its label without a remount.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TriggerPaint {
    open: bool,
    scale: f32,
    min_width: f32,
    text_size: f32,
}

/// Repaints the label of every dropdown trigger whose option changed since the
/// last frame, keeping its hover, press and focus decorations.
pub fn relabel_dropdown_triggers(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    let pending = runtime.with(|rt| std::mem::take(&mut rt.borrow_mut().dropdown_relabels));
    for &id in &pending {
        let paint = runtime.with(|rt| rt.borrow().dropdown_triggers.get(id).copied());
        let value = scene.a11y(id).and_then(|a| a.value.clone());
        let (Some(paint), Some(value)) = (paint, value) else {
            continue;
        };
        emit_dropdown_trigger_paint(
            runtime,
            scene,
            shaper,
            atlas,
            id,
            &value,
            paint.open,
            paint.scale,
            paint.min_width,
            paint.text_size,
        );
        scene.mark_dirty(id, DirtyFlags::PAINT);
        // The trigger's paint was cleared and refilled from scratch.
        crate::widgets::redecorate_after_refill(runtime, scene, id);
    }
    // Hand the buffer back so the next selection does not allocate.
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        if rt.dropdown_relabels.is_empty() {
            let mut pending = pending;
            pending.clear();
            rt.dropdown_relabels = pending;
        }
    });
}

/// Width of a dropdown trigger's outline in a frameless design.
const TRIGGER_BORDER: f32 = 1.0;
/// Space between an open trigger and its popup.
const POPUP_GAP: f32 = 4.0;
/// Padding between a popup's edge and its rows.
const POPUP_PAD: f32 = 4.0;
/// Extra row width so the widest label clears the chosen row's check mark.
const OPTION_CHECK_ROOM: f32 = 8.0;

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
        // The trigger shows the chosen option, falling back to the
        // placeholder, then to the first.
        let chosen = this.options.iter().find(|o| o.selected);
        let value: String = match (chosen, &this.placeholder) {
            (Some(o), _) => o.label.clone().into_owned(),
            (None, Some(text)) => text.clone().into_owned(),
            (None, None) => this
                .options
                .first()
                .map(|o| o.label.clone().into_owned())
                .unwrap_or_default(),
        };
        // One shared width for the trigger and every option — the widest label
        // plus the trigger's padding + caret. The trigger never resizes when the
        // choice changes, and the open popup is a single flush-edged opaque panel
        // instead of per-label ragged rows that let content bleed through.
        let inv = 1.0 / norm_scale(ctx.scale);
        let text_size = this.text_size.unwrap_or(BUTTON_TEXT_SIZE);
        let phys = phys_size_px(text_size, ctx.scale);
        let widest = this
            .options
            .iter()
            .map(|o| o.label.as_ref())
            .chain(this.placeholder.as_deref())
            .map(|text| ctx.text.shape(text, phys, None).width * inv)
            .fold(0.0f32, f32::max);
        let sh = theme_for(runtime, id).shape;
        let (pad_h, pad_v) = (sh.pad(PAD_H), sh.pad(PAD_V));
        let f = sh.frame.max(TRIGGER_BORDER);
        let shared_width = (widest + 2.0 * (pad_h + f) + CARET_SPACE).max(this.min_width);
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
        crate::widgets::note_authored(&ctx.runtime, id, crate::widgets::text_bits(&value));
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
            text_size,
        );
        ctx.runtime.with(|rt| {
            rt.borrow_mut().dropdown_triggers.insert(
                id,
                TriggerPaint {
                    open: this.open,
                    scale: ctx.scale,
                    min_width: shared_width,
                    text_size,
                },
            );
        });
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
            // The popup is a padded, outlined surface with a shadow, a few
            // pixels below the trigger, as wide as the trigger.
            let popup = ctx.scene.insert(WidgetKind::Pad, Some(wrap));
            let frame = sh.frame.max(1.0);
            let inset = POPUP_PAD + frame;
            let mut style = ContainerStyle::new(Container::Pad(EdgeInsets::all(inset)));
            style.anchor = Some(Point {
                x: 0.0,
                y: ts.height + 2.0 * (pad_v + f) + POPUP_GAP,
            });
            style.anchor_above = Some(-POPUP_GAP);
            ctx.layout.set_container(popup, style);
            ctx.scene
                .set_overlay_level(popup, crate::widgets::POPUP_OVERLAY_LEVEL);
            let rows = ctx.scene.insert(WidgetKind::Column, Some(popup));
            ctx.layout
                .set_container(rows, ContainerStyle::new(Container::Column));
            for mut opt in this.options {
                // As wide as the trigger's text box plus room for the check
                // mark that marks the chosen row.
                opt.min_width = shared_width + OPTION_CHECK_ROOM;
                opt.text_size = this.text_size;
                Box::new(opt).build(ctx, Some(rows));
            }
            crate::widgets::panel::register_popup_surface(&ctx.runtime, ctx.scene, popup);
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
    /// The dropdown's label size, stamped like `min_width`.
    pub(crate) text_size: Option<f32>,
}

impl DropdownOption {
    /// An option with a static label.
    pub fn new(label: impl Into<Cow<'static, str>>) -> DropdownOption {
        DropdownOption {
            label: label.into(),
            selected: false,
            on_select: None,
            min_width: 0.0,
            text_size: None,
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
        crate::widgets::note_authored(&ctx.runtime, id, u64::from(this.selected));
        if this.text_size.is_some() {
            let style = TabStyle {
                text_size: this.text_size,
                ..TabStyle::default()
            };
            runtime.with(|runtime| runtime.borrow_mut().tab_styles.insert(id, style));
        }
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
/// called by [`dispatch_click`](crate::widgets::dispatch_click) for `Tab` and `ListItem`.
/// Selects exclusively: tabs clear every selected tab in their nearest semantic
/// tab list (including grouped/tree descendants), while list items and dropdown
/// options retain sibling exclusivity. The selected peers are recolored in place
/// and marked dirty, then the clicked node's stored `on_select` fires. An
/// already-selected tab/item still fires its handler but does not re-dirty its own
/// state. Returns `true` if a handler ran or the clicked node's state changed. The
/// disabled check already happened in the caller.
pub(crate) fn dispatch_click_selection(
    runtime: &crate::widgets::Runtime,
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
    if kind == WidgetKind::Tab {
        crate::widgets::activate_branch(runtime, scene, id, was_selected);
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
/// by [`dispatch_click`](crate::widgets::dispatch_click) for `Dropdown` and
/// `DropdownOption`.
///
/// The **trigger** only fires its stored toggle handler: showing/hiding the
/// option list is structural, so the host remounts with `open` flipped
/// (SOUL §3.3) — the same in-window remount an example switcher uses. An
/// **option** selects with sibling exclusivity exactly like a tab/list item,
/// then mirrors its label into the sibling trigger's accessible value
/// (SOUL §6.1), so the combo box announces the new choice without a remount.
pub(crate) fn dispatch_click_dropdown(
    runtime: &crate::widgets::Runtime,
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
                // option → rows → popup surface → dropdown wrapper, whose
                // children hold the trigger leaf beside the popup. Walk up a
                // few ancestors so either popup shape finds it.
                let is_trigger = |c: WidgetId| {
                    scene.node(c).is_some_and(|n| {
                        n.kind == WidgetKind::Dropdown
                            || (n.kind == WidgetKind::TextInput
                                && scene
                                    .a11y(c)
                                    .is_some_and(|a| Role::from_u16(a.role) == Role::ComboBox))
                    })
                };
                let mut trigger = None;
                let mut ancestor = scene.node(id).and_then(|n| n.parent);
                for _ in 0..4 {
                    let Some(node) = ancestor.and_then(|a| scene.node(a)) else {
                        break;
                    };
                    if let Some(found) = node.children.iter().copied().find(|&c| is_trigger(c)) {
                        trigger = Some(found);
                        break;
                    }
                    ancestor = node.parent;
                }
                let name = scene.a11y(id).and_then(|a| a.name.clone());
                if let (Some(t), Some(name)) = (trigger, name) {
                    scene.set_a11y_value(t, Some(name));
                    // The painted label follows on the next frame.
                    runtime.with(|rt| {
                        let mut rt = rt.borrow_mut();
                        if rt.dropdown_triggers.contains_key(t)
                            && !rt.dropdown_relabels.contains(&t)
                        {
                            rt.dropdown_relabels.push(t);
                        }
                    });
                }
            }
            acted
        }
        _ => false,
    }
}

/// One open dropdown or combo list: its floating popup and the control that
/// opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenList {
    pub popup: WidgetId,
    pub trigger: WidgetId,
}

/// The open dropdown and combo lists, found through the overlay roots, so the
/// cost is the number of overlays and not the size of the scene. A list is an
/// overlay at the popup level whose wrapper holds an expanded combo trigger.
pub fn open_lists(scene: &Scene) -> SmallVec<[OpenList; 2]> {
    let mut lists = SmallVec::new();
    for popup in scene.overlays() {
        if scene.overlay_level(popup) != crate::widgets::POPUP_OVERLAY_LEVEL {
            continue;
        }
        let Some(wrapper) = scene.node(popup).and_then(|node| node.parent) else {
            continue;
        };
        let trigger = scene.node(wrapper).and_then(|node| {
            node.children.iter().copied().find(|&child| {
                scene.a11y(child).is_some_and(|a| {
                    Role::from_u16(a.role) == Role::ComboBox
                        && StateFlags(a.state).contains(StateFlags::EXPANDED)
                })
            })
        });
        if let Some(trigger) = trigger {
            lists.push(OpenList { popup, trigger });
        }
    }
    lists
}

/// Dismisses every expanded dropdown that does not contain `interaction_target`.
///
/// Dropdown visibility is structural, so dismissal fires the same `on_toggle`
/// handler as the trigger and lets the host remount with `open = false`. Keeping
/// the target's dropdown open is important for interactions with its trigger and
/// options; a target outside every dropdown (including `None` for blank space)
/// dismisses all of them. Returns `true` when at least one toggle handler ran.
pub fn dismiss_open_dropdowns(
    runtime: &crate::widgets::Runtime,
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
        if interaction_target
            .is_some_and(|target| crate::widgets::is_in_subtree(scene, target, wrapper))
        {
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
