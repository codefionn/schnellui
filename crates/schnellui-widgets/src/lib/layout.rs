use super::*;

pub struct DockArea {
    child: Option<AnyView>,
    name: Cow<'static, str>,
    on_dock: Option<Box<dyn FnMut(DockPosition) + 'static>>,
    style: ContainerStyle,
}

impl DockArea {
    pub fn new(name: impl Into<Cow<'static, str>>) -> DockArea {
        DockArea {
            child: None,
            name: name.into(),
            on_dock: None,
            style: ContainerStyle::new(Container::Stack),
        }
    }

    pub fn child(mut self, child: impl View) -> DockArea {
        self.child = Some(Box::new(child));
        self
    }

    pub fn on_dock(mut self, callback: impl FnMut(DockPosition) + 'static) -> DockArea {
        self.on_dock = Some(Box::new(callback));
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
            with_handlers(&ctx.runtime, id, |handlers| {
                handlers.dock = Some(callback);
                handlers.dock_preview = Some(preview);
            });
        }
        id
    }
}

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
pub struct Scroll {
    pub(crate) child: Option<AnyView>,
    pub(crate) style: ContainerStyle,
    pub(crate) on_scroll: Option<Box<dyn FnMut(f32) + 'static>>,
    pub(crate) on_scroll_debounced: Option<(Duration, Duration, Box<dyn FnMut(f32) + 'static>)>,
    pub(crate) name: Option<Cow<'static, str>>,
    pub(crate) scrollbar: bool,
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
        ctx.layout.set_container(id, this.style);
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
