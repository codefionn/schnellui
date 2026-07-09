use super::dropdown::write_selectable_semantics;
use super::*;

pub struct TabBar {
    pub(crate) children: Vec<AnyView>,
    pub(crate) style: ContainerStyle,
    pub(crate) trailing: Option<AnyView>,
    pub(crate) on_reorder: Option<Box<dyn FnMut(usize, usize) + 'static>>,
}

impl TabBar {
    /// A new empty tab bar.
    pub fn new() -> TabBar {
        TabBar {
            children: Vec::new(),
            style: ContainerStyle::new(Container::Row),
            trailing: None,
            on_reorder: None,
        }
    }
    /// Appends a child (usually a [`Tab`], SOUL §3.3 `.child(…)`).
    pub fn child(mut self, c: impl View) -> TabBar {
        self.children.push(Box::new(c));
        self
    }
    /// Places one independent view after the final tab. This is commonly a
    /// compact [`crate::Button`] for creating a new tab. The view remains outside
    /// the semantic tab list, so activating it never changes tab selection.
    pub fn trailing(mut self, view: impl View) -> TabBar {
        self.trailing = Some(Box::new(view));
        self
    }
    /// Alias for [`Self::trailing`] when the final view is an action control.
    pub fn action(self, view: impl View) -> TabBar {
        self.trailing(view)
    }
    /// Enables pointer reordering for this bar. The callback receives the
    /// dragged tab's old index and its new final index. The tab order remains
    /// controlled by the caller, which should update its model and remount.
    ///
    /// Reordering composes with [`Tab::on_drag_start`], [`Tab::on_drag_end`], and
    /// [`Tab::on_drop`]: a drop on a peer in this bar is claimed as a reorder,
    /// while drops outside the bar continue through the tab's ordinary drag/drop
    /// handlers. This lets dockable tabs reorder locally and dock elsewhere with
    /// the same gesture.
    pub fn on_reorder(mut self, f: impl FnMut(usize, usize) + 'static) -> TabBar {
        self.on_reorder = Some(Box::new(f));
        self
    }
    /// Sets the gap between tabs.
    pub fn gap(mut self, gap: f32) -> TabBar {
        self.style.gap = gap;
        self
    }
    /// Lets overflowing tabs wrap onto additional lines instead of shrinking —
    /// the responsive-flow switch (SOUL §8.1), same as [`Row::wrap`](crate::Row::wrap).
    pub fn wrap(mut self) -> TabBar {
        self.style.wrap = true;
        self
    }
    pub fn role(&self) -> Role {
        Role::TabList
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::TabBar
    }
    /// Number of children configured (pre-build).
    pub fn child_count(&self) -> usize {
        self.children.len()
    }
    /// Whether a trailing view has been configured.
    pub fn has_trailing(&self) -> bool {
        self.trailing.is_some()
    }
}

impl Default for TabBar {
    fn default() -> Self {
        Self::new()
    }
}

impl View for TabBar {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let TabBar {
            children,
            style,
            trailing,
            on_reorder,
        } = *self;
        let compound = trailing.as_ref().map(|_| {
            let wrapper = ctx.scene.insert(WidgetKind::Row, parent);
            ctx.scene.a11y_mut(wrapper).role = Role::Group.as_u16();
            let mut wrapper_style = ContainerStyle::new(Container::Row);
            wrapper_style.align = schnellui_layout::Align::Center;
            wrapper_style.gap = style.gap;
            ctx.layout.set_container(wrapper, wrapper_style);
            wrapper
        });
        let id = ctx.scene.insert(WidgetKind::TabBar, compound.or(parent));
        // A semantic container (like Scroll, SOUL §6.1): geometry from children,
        // but a real TabList role so assistive tech announces the tab group.
        ctx.scene.a11y_mut(id).role = Role::TabList.as_u16();
        ctx.layout.set_container(id, style);
        let mut reorder_tabs = SmallVec::<[WidgetId; 8]>::new();
        for child in children {
            let child_root = child.build(ctx, Some(id));
            if on_reorder.is_some() {
                if let Some(tab) = first_tab_in_subtree(ctx.scene, child_root) {
                    reorder_tabs.push(tab);
                }
            }
        }
        if let Some(callback) = on_reorder {
            crate::register_tab_reorder(&ctx.runtime, id, reorder_tabs, callback);
        }
        if let (Some(wrapper), Some(trailing)) = (compound, trailing) {
            trailing.build(ctx, Some(wrapper));
        }
        compound.unwrap_or(id)
    }
}

fn first_tab_in_subtree(scene: &Scene, id: WidgetId) -> Option<WidgetId> {
    let node = scene.node(id)?;
    if node.kind == WidgetKind::Tab {
        return Some(id);
    }
    node.children
        .iter()
        .find_map(|child| first_tab_in_subtree(scene, *child))
}

// ---------------------------------------------------------------------------
// Tab (SOUL §8.1 — one exclusively-selectable tab)
// ---------------------------------------------------------------------------

/// One tab of a [`TabBar`] (SOUL §8.1). `Role::Tab`, `StateFlags::SELECTED` when
/// active; selecting it clears the sibling tabs (group exclusivity, SOUL §6.3) and
/// fires `on_select` — the same handler an inbound AccessKit `Click` fires.
pub struct Tab {
    pub(crate) label: Cow<'static, str>,
    pub(crate) selected: bool,
    pub(crate) width: Option<f32>,
    pub(crate) appearance: TabAppearance,
    pub(crate) disclosure: TabDisclosure,
    pub(crate) on_select: Option<ClickHandler>,
    pub(crate) on_drag_start: Option<ClickHandler>,
    pub(crate) on_drag_end: Option<Box<dyn FnMut(bool) + 'static>>,
    pub(crate) on_drop: Option<ClickHandler>,
    pub(crate) on_close: Option<ClickHandler>,
    pub(crate) context_menu: Option<ContextMenu>,
}

impl Tab {
    /// A tab with a static label.
    pub fn new(label: impl Into<Cow<'static, str>>) -> Tab {
        Tab {
            label: label.into(),
            selected: false,
            width: None,
            appearance: TabAppearance::Classic,
            disclosure: TabDisclosure::None,
            on_select: None,
            on_drag_start: None,
            on_drag_end: None,
            on_drop: None,
            on_close: None,
            context_menu: None,
        }
    }
    /// Marks this tab as the initially selected one.
    pub fn selected(mut self, selected: bool) -> Tab {
        self.selected = selected;
        self
    }
    /// Sets a minimum visual width for the painted tab row.
    pub fn width(mut self, width: f32) -> Tab {
        self.width = Some(width.max(0.0));
        self
    }
    /// Selects the tab's visual treatment.
    pub fn appearance(mut self, appearance: TabAppearance) -> Tab {
        self.appearance = appearance;
        self
    }
    /// Reserves the tree disclosure gutter and optionally paints a branch chevron.
    pub(crate) fn tree_disclosure(mut self, expanded: Option<bool>) -> Tab {
        self.disclosure = expanded
            .map(TabDisclosure::Branch)
            .unwrap_or(TabDisclosure::Placeholder);
        self
    }
    /// Sets the selection handler (SOUL §6.3 — shared with the a11y action path).
    pub fn on_select(mut self, f: impl FnMut() + 'static) -> Tab {
        self.on_select = Some(Box::new(f));
        self
    }
    /// Makes this tab a pointer drag source while preserving click-to-select.
    pub fn on_drag_start(mut self, f: impl FnMut() + 'static) -> Tab {
        self.on_drag_start = Some(Box::new(f));
        self
    }
    /// Runs when a real drag ends, with whether a drop target accepted it.
    pub fn on_drag_end(mut self, f: impl FnMut(bool) + 'static) -> Tab {
        self.on_drag_end = Some(Box::new(f));
        self
    }
    /// Accepts a dragged item and shows a preview ring while hovered.
    pub fn on_drop(mut self, f: impl FnMut() + 'static) -> Tab {
        self.on_drop = Some(Box::new(f));
        self
    }
    /// Adds a trailing close button and runs `f` when it is activated. The close
    /// target is independent from selection and drag gestures on the tab itself.
    pub fn on_close(mut self, f: impl FnMut() + 'static) -> Tab {
        self.on_close = Some(Box::new(f));
        self
    }
    /// Attaches a custom context menu to this tab.
    pub fn context_menu(mut self, menu: ContextMenu) -> Tab {
        self.context_menu = Some(menu);
        self
    }
    /// Appends one custom command to this tab's context menu.
    pub fn context_menu_item(mut self, item: ContextMenuItem) -> Tab {
        self.context_menu
            .get_or_insert_with(ContextMenu::new)
            .push(item);
        self
    }
    pub fn role(&self) -> Role {
        Role::Tab
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Tab
    }
}

impl View for Tab {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let runtime = &ctx.runtime;
        let this = *self;
        let compound = this.on_close.is_some().then(|| {
            let wrapper = ctx.scene.insert(WidgetKind::Row, parent);
            ctx.scene.a11y_mut(wrapper).role = Role::Group.as_u16();
            let mut style = ContainerStyle::new(Container::Row);
            style.gap = 0.0;
            style.align = schnellui_layout::Align::Center;
            ctx.layout.set_container(wrapper, style);
            wrapper
        });
        let id = ctx.scene.insert(WidgetKind::Tab, compound.or(parent));
        let label: String = this.label.into_owned();
        write_selectable_semantics(ctx.scene, id, Role::Tab, &label, this.selected);
        if this
            .context_menu
            .as_ref()
            .is_some_and(|menu| !menu.is_empty())
        {
            let a11y = ctx.scene.a11y_mut(id);
            let mut actions = ActionFlags(a11y.actions);
            actions.insert(ActionFlags::SHOW_CONTEXT_MENU);
            a11y.actions = actions.0;
        }
        ctx.runtime.with(|runtime| {
            runtime
                .borrow_mut()
                .tab_appearances
                .insert(id, this.appearance);
        });
        let ts = emit_selectable_paint(
            &ctx.runtime,
            ctx.scene,
            ctx.text,
            ctx.atlas,
            id,
            WidgetKind::Tab,
            &label,
            this.selected,
            ctx.scale,
            this.width.unwrap_or(0.0),
            this.disclosure,
        );
        let sh = theme_for(runtime, id).shape;
        let (pad_h, pad_v) = (sh.pad(PAD_H), sh.pad(PAD_V));
        ctx.layout.set_measure(
            id,
            Box::new(move |_avail| Size {
                width: (ts.width
                    + 2.0 * pad_h
                    + if this.disclosure == TabDisclosure::None {
                        0.0
                    } else {
                        TAB_DISCLOSURE_SPACE
                    })
                .max(this.width.unwrap_or(0.0)),
                height: ts.height + 2.0 * pad_v,
            }),
        );
        if let Some(f) = this.on_select {
            with_handlers(&ctx.runtime, id, |h| h.click = Some(f));
        }
        if this.on_drag_start.is_some() || this.on_drag_end.is_some() || this.on_drop.is_some() {
            with_handlers(&ctx.runtime, id, |handlers| {
                handlers.drag_start = this.on_drag_start;
                handlers.drag_end = this.on_drag_end;
                handlers.drop = this.on_drop;
            });
        }
        if let Some(menu) = this.context_menu {
            crate::context_menu::register_context_menu(&ctx.runtime, id, menu);
        }
        if let (Some(wrapper), Some(on_close)) = (compound, this.on_close) {
            let close = ctx.scene.insert(WidgetKind::Button, Some(wrapper));
            {
                let semantics = ctx.scene.a11y_mut(close);
                semantics.role = Role::Button.as_u16();
                semantics.name = Some(format!("Close {label}"));
                let mut actions = ActionFlags::default();
                actions.insert(ActionFlags::CLICK);
                actions.insert(ActionFlags::FOCUS);
                semantics.actions = actions.0;
            }
            // Keep density-sensitive padding around the fixed-size glyph. Scaling
            // the target's entire width made compact themes squeeze the × into
            // the label's trailing characters.
            let close_width = 2.0 * (TAB_CLOSE_HALF + theme_for(runtime, close).shape.pad(PAD_H));
            let close_height = ts.height + 2.0 * pad_v;
            let close_rect = Rect::new(0.0, 0.0, close_width, close_height);
            let center = Point {
                x: close_rect.width * 0.5,
                y: close_rect.height * 0.5,
            };
            let color = theme_for(runtime, close).text_muted;
            let paint = ctx.scene.paint_mut(close);
            paint.primitives.push(Primitive::SolidRect {
                rect: close_rect,
                color: Color::TRANSPARENT,
                corner_radius: 0.0,
            });
            paint.primitives.push(Primitive::Line {
                from: Point {
                    x: center.x - TAB_CLOSE_HALF,
                    y: center.y - TAB_CLOSE_HALF,
                },
                to: Point {
                    x: center.x + TAB_CLOSE_HALF,
                    y: center.y + TAB_CLOSE_HALF,
                },
                width: TAB_CLOSE_STROKE,
                color,
            });
            paint.primitives.push(Primitive::Line {
                from: Point {
                    x: center.x + TAB_CLOSE_HALF,
                    y: center.y - TAB_CLOSE_HALF,
                },
                to: Point {
                    x: center.x - TAB_CLOSE_HALF,
                    y: center.y + TAB_CLOSE_HALF,
                },
                width: TAB_CLOSE_STROKE,
                color,
            });
            ctx.layout.set_measure(
                close,
                Box::new(move |_avail| Size {
                    width: close_width,
                    height: close_height,
                }),
            );
            with_handlers(&ctx.runtime, close, |handlers| {
                handlers.click = Some(on_close)
            });
            ctx.runtime.with(|runtime| {
                runtime.borrow_mut().tab_close_buttons.insert(id, close);
            });
        }
        compound.unwrap_or(id)
    }
}

// ---------------------------------------------------------------------------
// List (SOUL §8.1 — the semantic column of items)
// ---------------------------------------------------------------------------

/// A selectable list (SOUL §8.1): a semantic **column** container carrying
/// [`Role::List`]. Its [`ListItem`] children select exclusively (single-selection,
/// SOUL §6.3), like tabs.
pub struct List {
    pub(crate) children: Vec<AnyView>,
    pub(crate) style: ContainerStyle,
}

impl List {
    /// A new empty list.
    pub fn new() -> List {
        List {
            children: Vec::new(),
            style: ContainerStyle::new(Container::Column),
        }
    }
    /// Appends a child (usually a [`ListItem`], SOUL §3.3 `.child(…)`).
    pub fn child(mut self, c: impl View) -> List {
        self.children.push(Box::new(c));
        self
    }
    /// Sets the gap between items.
    pub fn gap(mut self, gap: f32) -> List {
        self.style.gap = gap;
        self
    }
    pub fn role(&self) -> Role {
        Role::List
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::List
    }
    /// Number of children configured (pre-build).
    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}

impl Default for List {
    fn default() -> Self {
        Self::new()
    }
}

impl View for List {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::List, parent);
        ctx.scene.a11y_mut(id).role = Role::List.as_u16();
        ctx.layout.set_container(id, this.style);
        for child in this.children {
            child.build(ctx, Some(id));
        }
        id
    }
}

// ---------------------------------------------------------------------------
// ListItem (SOUL §8.1 — one exclusively-selectable list entry)
// ---------------------------------------------------------------------------

/// One entry of a [`List`] (SOUL §8.1). `Role::ListItem`, `StateFlags::SELECTED`
/// when chosen; selecting it clears the sibling items (single-selection, SOUL §6.3)
/// and fires `on_select` — the same handler an inbound AccessKit `Click` fires.
pub struct ListItem {
    pub(crate) label: Cow<'static, str>,
    pub(crate) selected: bool,
    pub(crate) on_select: Option<ClickHandler>,
}

impl ListItem {
    /// A list item with a static label.
    pub fn new(label: impl Into<Cow<'static, str>>) -> ListItem {
        ListItem {
            label: label.into(),
            selected: false,
            on_select: None,
        }
    }
    /// Marks this item as the initially selected one.
    pub fn selected(mut self, selected: bool) -> ListItem {
        self.selected = selected;
        self
    }
    /// Sets the selection handler (SOUL §6.3 — shared with the a11y action path).
    pub fn on_select(mut self, f: impl FnMut() + 'static) -> ListItem {
        self.on_select = Some(Box::new(f));
        self
    }
    pub fn role(&self) -> Role {
        Role::ListItem
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::ListItem
    }
}

impl View for ListItem {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let runtime = &ctx.runtime;
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::ListItem, parent);
        let label: String = this.label.into_owned();
        write_selectable_semantics(ctx.scene, id, Role::ListItem, &label, this.selected);
        let ts = emit_selectable_paint(
            &ctx.runtime,
            ctx.scene,
            ctx.text,
            ctx.atlas,
            id,
            WidgetKind::ListItem,
            &label,
            this.selected,
            ctx.scale,
            0.0,
            TabDisclosure::None,
        );
        let sh = theme_for(runtime, id).shape;
        let (pad_h, pad_v) = (sh.pad(PAD_H), sh.pad(PAD_V));
        ctx.layout.set_measure(
            id,
            Box::new(move |_avail| Size {
                width: ts.width + 2.0 * pad_h,
                height: ts.height + 2.0 * pad_v,
            }),
        );
        if let Some(f) = this.on_select {
            with_handlers(&ctx.runtime, id, |h| h.click = Some(f));
        }
        id
    }
}

// ---------------------------------------------------------------------------
// Dropdown (SOUL §8.1 — the collapsed trigger + its exclusive option list)
// ---------------------------------------------------------------------------

/// Space reserved at the trigger's right edge for the caret chevron.
pub(crate) const CARET_SPACE: f32 = 18.0;
/// Half-width of the caret chevron.
pub(crate) const CARET_HALF: f32 = 4.0;
/// Half-height of the caret chevron (tip-to-base from the vertical center).
pub(crate) const CARET_RISE: f32 = 2.0;
/// Stroke width of the caret chevron's two line segments.
pub(crate) const CARET_STROKE: f32 = 1.6;
