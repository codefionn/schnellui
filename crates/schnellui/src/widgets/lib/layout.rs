use super::*;

pub struct DockArea {
    child: Option<AnyView>,
    name: Cow<'static, str>,
    on_dock: Option<Box<dyn FnMut(DockPosition) + 'static>>,
    accepts: Option<Box<dyn Fn(DockPosition) -> bool + 'static>>,
    labels: [Option<Cow<'static, str>>; 5],
    style: ContainerStyle,
}

impl DockArea {
    pub fn new(name: impl Into<Cow<'static, str>>) -> DockArea {
        DockArea {
            child: None,
            name: name.into(),
            on_dock: None,
            accepts: None,
            labels: [None, None, None, None, None],
            style: ContainerStyle::new(Container::Stack),
        }
    }

    /// Says what a drop on `position` does ("Split right", "Swap"): a chip in
    /// the middle of that zone's preview while a drag hovers it. Needs
    /// [`DockArea::size`], which places the chips.
    pub fn zone_label(
        mut self,
        position: DockPosition,
        label: impl Into<Cow<'static, str>>,
    ) -> DockArea {
        self.labels[position.index()] = Some(label.into());
        self
    }

    pub fn child(mut self, child: impl View) -> DockArea {
        self.child = Some(Box::new(child));
        self
    }

    pub fn on_dock(mut self, callback: impl FnMut(DockPosition) + 'static) -> DockArea {
        self.on_dock = Some(Box::new(callback));
        self
    }

    /// Asked while a drag hovers, with the zone under the pointer: false
    /// makes that zone no target for the drag, as for a tab dropped on its
    /// own panes, which would change nothing. Without it every drag is taken.
    pub fn accepts(mut self, accepts: impl Fn(DockPosition) -> bool + 'static) -> DockArea {
        self.accepts = Some(Box::new(accepts));
        self
    }

    /// Fixes the dock target to the same box as its pane surface.
    pub fn size(mut self, width: f32, height: f32) -> DockArea {
        self.style.fixed_size = Some(Size { width, height });
        self
    }

    /// Sets the minimum outer width of the dock target.
    pub fn min_width(mut self, width: f32) -> DockArea {
        self.style.min_width = Some(width.max(0.0));
        self
    }

    /// Sets the minimum outer height of the dock target.
    pub fn min_height(mut self, height: f32) -> DockArea {
        self.style.min_height = Some(height.max(0.0));
        self
    }
}

impl View for DockArea {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Stack, parent);
        let preview_size = this.style.fixed_size;
        {
            let semantics = ctx.scene.a11y_mut(id);
            semantics.role = Role::Group.as_u16();
            semantics.name = Some(this.name.into_owned());
        }
        ctx.layout.set_container(id, this.style);
        if let Some(child) = this.child {
            child.build(ctx, Some(id));
        }
        if let Some(callback) = this.on_dock {
            // A container is transparent to hit-testing, so this last child can
            // paint above pane content without stealing the pointer from tabs.
            let preview = ctx.scene.insert(WidgetKind::Stack, Some(id));
            ctx.scene.a11y_mut(preview).role = Role::Group.as_u16();
            let mut preview_style = ContainerStyle::new(Container::Stack);
            preview_style.fixed_size = preview_size;
            ctx.layout.set_container(preview, preview_style);
            // Its own layer, so the zone's wash lies over the pane's text
            // instead of under its opaque cells, the middle zone as plainly
            // as the edges.
            ctx.scene
                .set_overlay_level(preview, ZONE_PREVIEW_OVERLAY_LEVEL);
            let mut labels = [None; 5];
            if let Some(size) = preview_size {
                let area = Rect::new(0.0, 0.0, size.width, size.height);
                for position in DockPosition::ALL {
                    let Some(text) = this.labels[position.index()].clone() else {
                        continue;
                    };
                    let zone = super::drag::dock_preview_rect(area, position);
                    let chip = Pad::insets(EdgeInsets {
                        top: zone.y,
                        right: 0.0,
                        bottom: 0.0,
                        left: zone.x,
                    })
                    .child(
                        Row::new()
                            .size(zone.width, zone.height)
                            .justify(Justify::Center)
                            .align(Align::Center)
                            .child(Badge::new(text).tone(BadgeTone::Accent).text_size(15.0)),
                    );
                    let chip = Box::new(chip).build(ctx, Some(preview));
                    // Shown only while a drag hovers its zone: hidden, it is
                    // left out of paint, hit tests and the accessibility tree.
                    ctx.scene.set_visible(chip, false);
                    // Its own layer, so the pane's text draws under the chip
                    // rather than through it.
                    ctx.scene.set_overlay_level(chip, ZONE_LABEL_OVERLAY_LEVEL);
                    labels[position.index()] = Some(chip);
                }
            }
            with_handlers(&ctx.runtime, id, |handlers| {
                handlers.dock = Some(callback);
                handlers.dock_preview = Some(preview);
                handlers.dock_accepts = this.accepts;
                handlers.dock_labels = labels;
            });
        }
        id
    }
}

/// A container that makes its whole box a pointer drag source, as a pane's
/// title strip is: a press anywhere on its content that is not itself a
/// drag source (a button's click still lands) starts a possible drag, which
/// becomes one past the usual threshold. It takes no focus, so it adds no
/// Tab stop; keyboard users need another way to the same action.
pub struct DragSource {
    child: Option<AnyView>,
    name: Cow<'static, str>,
    on_drag_start: Option<ClickHandler>,
    on_drag_end: Option<Box<dyn FnMut(bool) + 'static>>,
    on_click: Option<ClickHandler>,
    context_menu: Option<crate::widgets::ContextMenu>,
}

impl DragSource {
    pub fn new(name: impl Into<Cow<'static, str>>) -> DragSource {
        DragSource {
            child: None,
            name: name.into(),
            on_drag_start: None,
            on_drag_end: None,
            on_click: None,
            context_menu: None,
        }
    }

    pub fn child(mut self, child: impl View) -> DragSource {
        self.child = Some(Box::new(child));
        self
    }

    /// Runs when the drag starts moving.
    pub fn on_drag_start(mut self, callback: impl FnMut() + 'static) -> DragSource {
        self.on_drag_start = Some(Box::new(callback));
        self
    }

    /// Runs after a drag that moved ends, with whether a target took it.
    pub fn on_drag_end(mut self, callback: impl FnMut(bool) + 'static) -> DragSource {
        self.on_drag_end = Some(Box::new(callback));
        self
    }

    /// Runs when a press on the handle is let go on it without a drag.
    pub fn on_click(mut self, callback: impl FnMut() + 'static) -> DragSource {
        self.on_click = Some(Box::new(callback));
        self
    }

    /// The menu a secondary click on the handle opens.
    pub fn context_menu(mut self, menu: crate::widgets::ContextMenu) -> DragSource {
        self.context_menu = Some(menu);
        self
    }
}

impl View for DragSource {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        // A column, which takes its child's size; a stack would not.
        let id = ctx.scene.insert(WidgetKind::Column, parent);
        {
            let semantics = ctx.scene.a11y_mut(id);
            semantics.role = Role::Group.as_u16();
            semantics.name = Some(this.name.into_owned());
        }
        ctx.layout
            .set_container(id, ContainerStyle::new(Container::Column));
        if let Some(child) = this.child {
            child.build(ctx, Some(id));
        }
        with_handlers(&ctx.runtime, id, |handlers| {
            handlers.drag_start = this.on_drag_start;
            handlers.drag_end = this.on_drag_end;
            if this.on_click.is_some() {
                handlers.click = this.on_click;
            }
        });
        if let Some(menu) = this.context_menu {
            crate::widgets::context_menu::register_context_menu(&ctx.runtime, id, menu);
        }
        id
    }
}

/// Names what a drop on its child does ("New tab", "Move into this tab"):
/// a chip beside the child, shown only while a drag hovers a drop target in
/// it. Pane zones say this with [`DockArea::zone_label`]; this is for a
/// plain drop target, such as a button or a tab. Pointer hover alone never
/// shows it.
pub struct DropTip {
    child: Option<AnyView>,
    label: Cow<'static, str>,
}

impl DropTip {
    pub fn new(label: impl Into<Cow<'static, str>>) -> DropTip {
        DropTip {
            child: None,
            label: label.into(),
        }
    }

    pub fn child(mut self, child: impl View) -> DropTip {
        self.child = Some(Box::new(child));
        self
    }
}

impl View for DropTip {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        // A column takes its child's size, so the chip is placed against it.
        let id = ctx.scene.insert(WidgetKind::Column, parent);
        ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
        ctx.layout
            .set_container(id, ContainerStyle::new(Container::Column));
        if let Some(child) = this.child {
            child.build(ctx, Some(id));
        }
        // An empty mark at the column's corner anchors its paint, which
        // otherwise holds only the chip, when layout moves it.
        let anchor = node_rect(ctx.scene, id, Size::default());
        ctx.scene
            .paint_mut(id)
            .primitives
            .push(Primitive::SolidRect {
                rect: anchor,
                color: Color::TRANSPARENT,
                corner_radius: 0.0,
            });
        register_hover_tooltip(
            &ctx.runtime,
            ctx.scene,
            ctx.text,
            ctx.atlas,
            id,
            &this.label,
            Size::default(),
            ctx.scale,
        );
        elevate_tooltip_host(ctx.scene, id);
        let theme = theme_for(&ctx.runtime, id);
        ctx.runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            // In the accent, as the zone chips are: it names a drop, not a
            // control.
            if let Some(tip) = rt.hover_tooltips.get_mut(id) {
                tip.background_color = theme.accent;
                tip.text_color = theme.on_accent;
            }
            rt.drop_tips.insert(id, ());
        });
        id
    }
}

/// The layer of a dock zone's chip: over the panes, under popups and tooltips.
const ZONE_LABEL_OVERLAY_LEVEL: u8 = 5;
/// The layer of a dock zone's wash: under its chip.
const ZONE_PREVIEW_OVERLAY_LEVEL: u8 = 4;

/// A padded single-child container (SOUL §8.1).
pub struct Pad {
    pub(crate) insets: EdgeInsets,
    pub(crate) child: Option<AnyView>,
}

impl Pad {
    /// Uniform padding.
    pub fn all(v: f32) -> Pad {
        Pad {
            insets: EdgeInsets::all(v),
            child: None,
        }
    }
    /// Explicit insets.
    pub fn insets(insets: EdgeInsets) -> Pad {
        Pad {
            insets,
            child: None,
        }
    }
    /// Sets the single child.
    pub fn child(mut self, c: impl View) -> Pad {
        self.child = Some(Box::new(c));
        self
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Pad
    }
}

impl View for Pad {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Pad, parent);
        ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
        ctx.layout
            .set_container(id, ContainerStyle::new(Container::Pad(this.insets)));
        if let Some(c) = this.child {
            c.build(ctx, Some(id));
        }
        id
    }
}

/// Flexible empty space that grows to fill the main axis (SOUL §8.1).
pub struct Spacer;

impl Spacer {
    pub fn new() -> Spacer {
        Spacer
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Spacer
    }
}

impl Default for Spacer {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Spacer {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let id = ctx.scene.insert(WidgetKind::Spacer, parent);
        ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
        ctx.layout
            .set_container(id, ContainerStyle::new(Container::Spacer));
        id
    }
}

/// Per-child flex factors for one view (SOUL §8.1) — the responsive share of a flex
/// parent's main axis: `grow` claims leftover space, `shrink` yields under
/// overflow, `basis` replaces the intrinsic main size, and the min/max bounds clamp
/// the result.
///
/// `Flex` inserts **no node of its own**: it builds its single child and registers
/// the factors against the child's node ([`LayoutEngine::set_flex`]), so the flex
/// applies directly where CSS would put it. Childless, it degenerates to a
/// **weighted spacer** — a grown empty gap, e.g. `flex(grow = 2.0)` between two
/// labels eats twice the share of a plain `spacer`.
///
/// In `view!`: `flex(grow = 1.0, basis = 120.0) { button { "…" } }`.
pub struct Flex {
    pub(crate) child: Option<AnyView>,
    pub(crate) flex: FlexChild,
}

impl Flex {
    /// A new flex wrapper with no factors set and no child.
    pub fn new() -> Flex {
        Flex {
            child: None,
            flex: FlexChild::default(),
        }
    }
    /// Sets the single wrapped child (a later call replaces it, like `pad`).
    pub fn child(mut self, c: impl View) -> Flex {
        self.child = Some(Box::new(c));
        self
    }
    /// Share of the parent's leftover main-axis space (CSS `flex-grow`).
    pub fn grow(mut self, grow: f32) -> Flex {
        self.flex.grow = Some(grow);
        self
    }
    /// Share of main-axis overflow absorbed (CSS `flex-shrink`).
    pub fn shrink(mut self, shrink: f32) -> Flex {
        self.flex.shrink = Some(shrink);
        self
    }
    /// Starting main size in logical px before grow/shrink (CSS `flex-basis`).
    pub fn basis(mut self, basis: f32) -> Flex {
        self.flex.basis = Some(basis);
        self
    }
    /// Lower width bound the resolved size never shrinks below.
    pub fn min_width(mut self, v: f32) -> Flex {
        self.flex.min_width = Some(v.max(0.0));
        self
    }
    /// Lower height bound.
    pub fn min_height(mut self, v: f32) -> Flex {
        self.flex.min_height = Some(v.max(0.0));
        self
    }
    /// Upper width bound the resolved size never grows past.
    pub fn max_width(mut self, v: f32) -> Flex {
        self.flex.max_width = Some(v);
        self
    }
    /// Upper height bound.
    pub fn max_height(mut self, v: f32) -> Flex {
        self.flex.max_height = Some(v);
        self
    }
}

impl Default for Flex {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Flex {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = match this.child {
            // With a child: build it directly under `parent` and hang the factors
            // on ITS node — no wrapper node, so the flex applies where CSS would.
            Some(c) => c.build(ctx, parent),
            // Childless: a weighted spacer (grows per the factors, draws nothing).
            None => {
                let id = ctx.scene.insert(WidgetKind::Spacer, parent);
                ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
                ctx.layout
                    .set_container(id, ContainerStyle::new(Container::Spacer));
                id
            }
        };
        ctx.layout.set_flex(id, this.flex);
        id
    }
}

/// A node-transparent responsive visibility wrapper.
///
/// Prefer [`View::show_when`] for fluent composition. `Responsive::new(query)`
/// is useful when constructing wrappers explicitly or through generated code.
pub struct Responsive {
    child: Option<AnyView>,
    query: ResponsiveQuery,
}

impl Responsive {
    pub fn new(query: ResponsiveQuery) -> Self {
        Self { child: None, query }
    }

    pub fn child(mut self, child: impl View) -> Self {
        self.child = Some(Box::new(child));
        self
    }
}

impl View for Responsive {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = this
            .child
            .expect("Responsive requires one child")
            .build(ctx, parent);
        ctx.layout.set_responsive(id, this.query);
        id
    }
}

/// A node-transparent component-reference wrapper.
///
/// Usually constructed via [`View::with_ref`]. The same [`ComponentRef`] can then
/// be resolved from the mounted scene or used by [`ResponsiveQuery::component`].
pub struct Referenced {
    child: Option<AnyView>,
    reference: ComponentRef,
}

impl Referenced {
    pub fn new(reference: ComponentRef) -> Self {
        Self {
            child: None,
            reference,
        }
    }

    pub fn child(mut self, child: impl View) -> Self {
        self.child = Some(Box::new(child));
        self
    }
}

impl View for Referenced {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = this
            .child
            .expect("Referenced requires one child")
            .build(ctx, parent);
        ctx.scene.set_component_ref(id, this.reference);
        id
    }
}

/// A clipped, scrollable viewport (SOUL §8.1). A vertical scroll **re-composites**:
/// its content offset is a scene column ([`Scene::set_scroll_offset`]) the wgpu
/// renderer applies when it gathers the node's descendants (offsetting them by
/// −offset and clipping them to the viewport rect), so a scroll marks **paint-dirty
/// only** — never a relayout, never a re-raster of the content itself (SOUL §3.2).
/// This is the v0 stand-in for the SOUL §3.2 property/transform tree.
///
/// Wheel and action input arrive through [`dispatch_scroll`]; the accessible viewport
/// carries [`Role::ScrollView`] with the `ScrollUp`/`ScrollDown` actions and an
/// accessible value tracking the vertical offset (SOUL §6.1, §6.3).
///
/// Careful: the scrollbar is **off by default** ([`Scroll::scrollbar`]). A viewport
/// built without `.scrollbar(true)` scrolls by wheel and keys but shows no sign that
/// it has more content, so users miss everything below the fold. Apps have shipped
/// lists like that several times. Turn the bar on for every viewport whose content
/// can overflow. While the content overflows, layout reserves the bar's strip
/// (`SCROLLBAR_WIDTH + SCROLLBAR_INSET`, 12 px) at the right edge and lays the content
/// out in the rest, so the bar never covers it. Content that fits keeps the full width.
/// Debounce delay, max wait, and the handler behind [`Scroll::on_scroll_debounced`].
type DebouncedScrollHandler = (Duration, Duration, Box<dyn FnMut(f32) + 'static>);

pub struct Scroll {
    pub(crate) child: Option<AnyView>,
    pub(crate) style: ContainerStyle,
    pub(crate) on_scroll: Option<Box<dyn FnMut(f32) + 'static>>,
    pub(crate) on_scroll_debounced: Option<DebouncedScrollHandler>,
    pub(crate) name: Option<Cow<'static, str>>,
    pub(crate) scrollbar: bool,
    pub(crate) stable_gutter: bool,
    pub(crate) edge_fade: Option<(Color, f32)>,
    pub(crate) edge_auto_scroll: bool,
    pub(crate) follow_end: bool,
    pub(crate) initial_offset: Option<f32>,
    pub(crate) restoration_key: Option<Cow<'static, str>>,
}

impl Scroll {
    pub fn new() -> Scroll {
        Scroll {
            child: None,
            style: ContainerStyle::new(Container::Scroll),
            on_scroll: None,
            on_scroll_debounced: None,
            name: None,
            scrollbar: false,
            stable_gutter: false,
            edge_fade: None,
            edge_auto_scroll: false,
            follow_end: false,
            initial_offset: None,
            restoration_key: None,
        }
    }
    /// Gives the viewport an accessible name. This is also useful for targeting
    /// a specific focused scroll surface with application-level key handling.
    pub fn label(mut self, name: impl Into<Cow<'static, str>>) -> Scroll {
        self.name = Some(name.into());
        self
    }
    /// With a scrollbar: keeps the bar's strip reserved even while the content
    /// fits, so the content has one width and does not shift when it starts to
    /// overflow.
    pub fn stable_gutter(mut self, stable: bool) -> Scroll {
        self.stable_gutter = stable;
        self
    }
    /// Fades the content into `ground` over `height` pixels at an edge that has
    /// more content beyond it: the top once the view is scrolled down, the bottom
    /// while more follows. An edge at rest shows nothing, and the fade grows with
    /// the distance scrolled, so it never pops in.
    pub fn edge_fade(mut self, ground: Color, height: f32) -> Scroll {
        self.edge_fade = (height > 0.0).then_some((ground, height));
        self
    }
    /// Sets the single scrollable child (SOUL §3.3 `.child(…)`).
    pub fn child(mut self, c: impl View) -> Scroll {
        self.child = Some(Box::new(c));
        self
    }
    /// Fixes the viewport box to `width × height` logical px — the visible window onto
    /// the content (sets [`ContainerStyle::fixed_size`]). Content taller than the box
    /// scrolls; content that fits can't scroll (`max_offset` clamps to 0).
    pub fn size(mut self, width: f32, height: f32) -> Scroll {
        self.style.fixed_size = Some(Size { width, height });
        self
    }
    /// Makes the viewport fill its parent's box, so a page-level scroll
    /// follows the window size (the template `scroll().fill()`).
    pub fn fill(mut self) -> Scroll {
        self.style.fill = true;
        self
    }
    /// Hands the content the viewport's width instead of its own max-content
    /// width. By default a Scroll sizes its child to its natural width, so a
    /// wrapping row or a text paragraph inside never sees a width to wrap at and
    /// runs past a narrow window with no way to scroll to it (the viewport scrolls
    /// only down). With this on, wrapping rows wrap, and text under a column that
    /// stretches reflows, at the viewport width. A child with a definite width
    /// keeps it.
    pub fn fit_width(mut self) -> Scroll {
        self.style.align = Align::Stretch;
        self
    }
    /// Sets the minimum viewport width; content or a definite size may make it wider.
    pub fn min_width(mut self, width: f32) -> Scroll {
        self.style.min_width = Some(width.max(0.0));
        self
    }
    /// Sets the minimum viewport height; content or a definite size may make it taller.
    pub fn min_height(mut self, height: f32) -> Scroll {
        self.style.min_height = Some(height.max(0.0));
        self
    }
    /// Registers a callback fired with the **new** vertical offset after each scroll
    /// (SOUL §6.3 — the same handler mouse-wheel input and an inbound
    /// `ScrollDown`/`ScrollUp` action both reach through [`dispatch_scroll`]).
    pub fn on_scroll(mut self, f: impl FnMut(f32) + 'static) -> Scroll {
        self.on_scroll = Some(Box::new(f));
        self
    }
    /// Registers a trailing-edge scroll callback. Each real offset change resets
    /// the trailing delay, while `max_wait` bounds a continuous gesture so the
    /// callback still runs periodically. The callback receives the latest
    /// clamped vertical offset and runs from the window host's wake cycle.
    pub fn on_scroll_debounced(
        mut self,
        delay: Duration,
        max_wait: Duration,
        f: impl FnMut(f32) + 'static,
    ) -> Scroll {
        self.on_scroll_debounced = Some((delay, max_wait.max(delay), Box::new(f)));
        self
    }
    /// Shows or hides an interactive vertical scrollbar. It is hidden by default;
    /// when shown, its thumb can be dragged and its track pages the viewport.
    /// Forgetting this call leaves overflowing content invisible to the user; see
    /// the note on [`Scroll`].
    pub fn scrollbar(mut self, visible: bool) -> Scroll {
        self.scrollbar = visible;
        self
    }
    /// Enables or disables scrolling while a held pointer is within the viewport's
    /// top or bottom 24 logical pixels. Disabled by default.
    pub fn edge_auto_scroll(mut self, enabled: bool) -> Scroll {
        self.edge_auto_scroll = enabled;
        self
    }
    /// Keeps the viewport pinned to its end across content remounts only while it
    /// was already at the end. A newly mounted viewport starts at the end; once a
    /// user scrolls upward, later content preserves that reading position.
    pub fn follow_end(mut self, enabled: bool) -> Scroll {
        self.follow_end = enabled;
        self
    }
    /// Sets the initial vertical position for a newly mounted viewport.
    /// A restored position with the same restoration key takes precedence on remount.
    pub fn initial_offset(mut self, offset: f32) -> Scroll {
        self.initial_offset = offset.is_finite().then(|| offset.max(0.0));
        self
    }
    /// Gives this viewport a stable, non-visual identity for restoring its offset
    /// across structural remounts. This prevents equally labelled viewports for
    /// different documents or conversations from sharing a scroll position.
    pub fn restoration_key(mut self, key: impl Into<Cow<'static, str>>) -> Scroll {
        self.restoration_key = Some(key.into());
        self
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Scroll
    }
}

impl Default for Scroll {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Scroll {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Scroll, parent);
        // A scroll viewport is semantic (SOUL §6.1): Role::ScrollView, the up/down
        // scroll actions + Focus, and an accessible value = the vertical offset in
        // logical px (rounded), starting at "0". [`dispatch_scroll`] keeps that value in
        // sync on every scroll (§6.2). Scrolling itself works through the scene
        // scroll-offset column + the renderer's gather-pass recomposite (see the type
        // docs) — the v0 stand-in for the SOUL §3.2 property tree.
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::ScrollView.as_u16();
            let mut acts = ActionFlags::default();
            acts.insert(ActionFlags::SCROLL_UP);
            acts.insert(ActionFlags::SCROLL_DOWN);
            acts.insert(ActionFlags::FOCUS);
            a.actions = acts.0;
            a.name = this.name.map(Cow::into_owned);
            // Wheel events rewrite this retained buffer in place. Twenty bytes
            // holds every signed i64 scroll offset, including its sign.
            let mut offset_value = String::with_capacity(20);
            offset_value.push('0');
            a.value = Some(offset_value);
        }
        let mut style = this.style;
        // The bar gets its own strip while the content overflows (SOUL §8.1: layout
        // owns geometry), so content never runs under it.
        style.scrollbar_gutter = this.scrollbar;
        style.scrollbar_gutter_stable = this.stable_gutter;
        ctx.layout.set_container(id, style);
        ctx.runtime.with(|runtime| {
            let debounced =
                this.on_scroll_debounced
                    .map(|(delay, max_wait, callback)| DebouncedScroll {
                        delay,
                        max_wait,
                        callback: Some(callback),
                        burst_start: None,
                        deadline: None,
                        latest_offset: 0.0,
                    });
            runtime.borrow_mut().scrolls.insert(
                id,
                ScrollState {
                    scrollbar: this.scrollbar,
                    edge_fade: this.edge_fade,
                    edge_auto_scroll: this.edge_auto_scroll,
                    follow_end: this.follow_end,
                    restoration_key: this.restoration_key,
                    debounced,
                },
            );
            runtime.borrow_mut().scroll_ids.push(id);
        });
        if this.follow_end {
            // Layout computes the real end later in the frame; clamping this
            // sentinel then lands exactly on that final content extent.
            ctx.scene.set_scroll_offset(
                id,
                Point {
                    x: 0.0,
                    y: f32::MAX,
                },
            );
        } else if let Some(offset) = this.initial_offset {
            ctx.scene.set_scroll_offset(id, Point { x: 0.0, y: offset });
            ctx.scene.set_a11y_value_i64(id, offset.round() as i64);
        }
        if let Some(f) = this.on_scroll {
            with_handlers(&ctx.runtime, id, |h| h.scroll = Some(f));
        }
        if let Some(c) = this.child {
            c.build(ctx, Some(id));
        }
        id
    }
}
