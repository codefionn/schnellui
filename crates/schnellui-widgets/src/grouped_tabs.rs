//! Configurable grouped tab lists with an optional tree presentation.
//!
//! [`GroupedTabList`] is a higher-level composition over the ordinary
//! [`Tab`](crate::Tab): groups provide labelled sections and [`TabNode`] adds
//! recursive children. [`GroupedTabMode::Flat`] renders those nodes as a
//! depth-first list; [`GroupedTabMode::Tree`] preserves the hierarchy with a
//! configurable indent and disclosure chevrons. Pair [`TabNode::expanded`] with
//! [`TabNode::on_toggle`] to keep branch state in application data and structurally
//! remount after a fold/unfold request. The resulting leaves are still real tabs,
//! so pointer, keyboard, AccessKit, drag/drop, and selection behavior use the same
//! paths as [`TabBar`](crate::TabBar).
//!
//! ```
//! use schnellui_widgets::{
//!     Button, ButtonAppearance, GroupedTabList, Tab, TabGroup, TabNode,
//! };
//!
//! let navigation = GroupedTabList::new()
//!     .tree()
//!     .indent(14.0)
//!     .min_tab_width(180.0)
//!     .group(
//!         TabGroup::new("Workspace").tab(
//!             TabNode::new("Editor")
//!                 .selected(true)
//!                 .actions([
//!                     Button::new("Refresh").appearance(ButtonAppearance::Ghost),
//!                     Button::new("Close").appearance(ButtonAppearance::Ghost),
//!                 ])
//!                 .child(TabNode::new("Outline"))
//!                 .child(Tab::new("Search")),
//!         ),
//!     )
//!     .group(TabGroup::new("Account").tab(Tab::new("Settings")));
//!
//! assert_eq!(navigation.group_count(), 2);
//! assert_eq!(navigation.tab_count(), 4);
//! ```

use std::borrow::Cow;

use schnellui_a11y::{Role, StateFlags};
use schnellui_layout::{Align, Container, ContainerStyle, EdgeInsets, FlexChild};
use schnellui_scene::{WidgetId, WidgetKind};

use crate::{BuildCtx, Tab, TabAppearance, Text, View};

/// How recursive [`TabNode`] children are presented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupedTabMode {
    /// Ignore visual depth and render every node in depth-first order.
    #[default]
    Flat,
    /// Preserve parent/child relationships as an indented tab tree.
    Tree,
}

/// One recursively nestable tab in a [`TabGroup`].
///
/// All ordinary tab interactions are available directly on this builder. A
/// collapsed node remains visible while its descendants are omitted in tree
/// mode; flat mode deliberately ignores collapse and shows the complete list.
/// Optional trailing actions remain independent of both selection and disclosure.
pub struct TabNode {
    tab: Tab,
    children: Vec<TabNode>,
    actions: Vec<crate::AnyView>,
    expanded: bool,
    on_toggle: Option<Box<dyn FnMut(bool) + 'static>>,
    separator: bool,
    context_menu_trigger: Option<Cow<'static, str>>,
}

impl TabNode {
    /// Creates a leaf node with the given visible and accessible tab label.
    pub fn new(label: impl Into<Cow<'static, str>>) -> TabNode {
        TabNode {
            tab: Tab::new(label),
            children: Vec::new(),
            actions: Vec::new(),
            expanded: true,
            on_toggle: None,
            separator: false,
            context_menu_trigger: None,
        }
    }

    /// Wraps an existing [`Tab`], retaining all of its configured behavior.
    pub fn from_tab(tab: Tab) -> TabNode {
        TabNode {
            tab,
            children: Vec::new(),
            actions: Vec::new(),
            expanded: true,
            on_toggle: None,
            separator: false,
            context_menu_trigger: None,
        }
    }

    /// Appends a child tab node.
    pub fn child(mut self, child: impl Into<TabNode>) -> TabNode {
        self.children.push(child.into());
        self
    }

    /// Appends several child nodes from configuration data.
    pub fn children<I, N>(mut self, children: I) -> TabNode
    where
        I: IntoIterator<Item = N>,
        N: Into<TabNode>,
    {
        self.children.extend(children.into_iter().map(Into::into));
        self
    }

    /// Appends an independent trailing action view to this row. This is commonly
    /// a [`crate::Button`], but can also be a composed icon button or small action
    /// group. Activating it does not select the tab or toggle its branch.
    pub fn action(mut self, action: impl View) -> TabNode {
        self.actions.push(Box::new(action));
        self
    }

    /// Appends several homogeneous trailing action views.
    pub fn actions<I, V>(mut self, actions: I) -> TabNode
    where
        I: IntoIterator<Item = V>,
        V: View,
    {
        self.actions.extend(
            actions
                .into_iter()
                .map(|action| Box::new(action) as crate::AnyView),
        );
        self
    }

    /// Marks this tab as the initially selected tab.
    pub fn selected(mut self, selected: bool) -> TabNode {
        self.tab = self.tab.selected(selected);
        self
    }

    /// Runs when this tab is selected.
    pub fn on_select(mut self, callback: impl FnMut() + 'static) -> TabNode {
        self.tab = self.tab.on_select(callback);
        self
    }

    /// Makes this tab a pointer drag source while preserving click-to-select.
    pub fn on_drag_start(mut self, callback: impl FnMut() + 'static) -> TabNode {
        self.tab = self.tab.on_drag_start(callback);
        self
    }

    /// Runs when a real drag ends, with whether a target accepted the drop.
    pub fn on_drag_end(mut self, callback: impl FnMut(bool) + 'static) -> TabNode {
        self.tab = self.tab.on_drag_end(callback);
        self
    }

    /// Makes this tab accept a dragged item.
    pub fn on_drop(mut self, callback: impl FnMut() + 'static) -> TabNode {
        self.tab = self.tab.on_drop(callback);
        self
    }

    /// Controls whether descendants are built in tree mode. Pair this controlled
    /// value with [`Self::on_toggle`] and remount after updating application state.
    pub fn expanded(mut self, expanded: bool) -> TabNode {
        self.expanded = expanded;
        self
    }

    /// Runs when an expandable branch is activated, receiving the requested next
    /// state. The tab is selected through its normal `on_select` path first; the
    /// host then updates the controlled [`Self::expanded`] value and remounts.
    pub fn on_toggle(mut self, callback: impl FnMut(bool) + 'static) -> TabNode {
        self.on_toggle = Some(Box::new(callback));
        self
    }

    /// Attaches a context menu to this tree node.
    ///
    /// The menu is registered on the underlying tab and is opened via the
    /// platform's standard gesture (right-click / keyboard ShowContextMenu) or
    /// via an optional trigger button added with [`Self::with_context_menu_button`].
    pub fn context_menu(mut self, menu: crate::ContextMenu) -> TabNode {
        self.tab = self.tab.context_menu(menu);
        self
    }

    /// Appends a single command to this tree node's context menu.
    pub fn context_menu_item(mut self, item: crate::ContextMenuItem) -> TabNode {
        self.tab = self.tab.context_menu_item(item);
        self
    }

    /// Adds a trailing ghost button that opens the tree node's context menu.
    ///
    /// Use this when the menu should also be reachable via an explicit
    /// affordance (e.g. a "⋮" button behind the tree node). The button is
    /// optional: tree nodes without a context menu simply ignore it, and the
    /// standard right-click / keyboard gesture still works even when the button
    /// is absent.
    pub fn with_context_menu_button(mut self) -> TabNode {
        self.context_menu_trigger = Some(Cow::Borrowed("⋮"));
        self
    }

    /// Adds a trailing button with a custom label that opens the tree node's
    /// context menu. See [`Self::with_context_menu_button`].
    pub fn context_menu_button(mut self, label: impl Into<Cow<'static, str>>) -> TabNode {
        self.context_menu_trigger = Some(label.into());
        self
    }

    /// Hides a previously configured context-menu trigger button.
    pub fn without_context_menu_button(mut self) -> TabNode {
        self.context_menu_trigger = None;
        self
    }

    /// Creates a non-interactive separator row. These are skipped for
    /// accessibility and do not count as tabs; they render as a thin
    /// horizontal rule indented to align with the tree depth.
    pub fn separator() -> TabNode {
        TabNode {
            tab: Tab::new(""),
            children: Vec::new(),
            actions: Vec::new(),
            expanded: true,
            on_toggle: None,
            separator: true,
            context_menu_trigger: None,
        }
    }

    /// Whether this node is a visual separator (not a selectable tab).
    pub fn is_separator(&self) -> bool {
        self.separator
    }

    /// Number of direct child nodes configured before build.
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// Number of trailing action views configured for this row.
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }

    /// Number of tabs in this node and all of its descendants.
    pub fn tab_count(&self) -> usize {
        if self.separator {
            return self.children.iter().map(TabNode::tab_count).sum();
        }
        1 + self.children.iter().map(TabNode::tab_count).sum::<usize>()
    }
}

impl From<Tab> for TabNode {
    fn from(tab: Tab) -> Self {
        Self::from_tab(tab)
    }
}

/// A named section in a [`GroupedTabList`].
pub struct TabGroup {
    label: Cow<'static, str>,
    tabs: Vec<TabNode>,
}

impl TabGroup {
    /// Creates an empty group.
    pub fn new(label: impl Into<Cow<'static, str>>) -> TabGroup {
        TabGroup {
            label: label.into(),
            tabs: Vec::new(),
        }
    }

    /// Appends a tab or tab tree.
    pub fn tab(mut self, tab: impl Into<TabNode>) -> TabGroup {
        self.tabs.push(tab.into());
        self
    }

    /// Appends several tabs or tab trees from configuration data.
    pub fn tabs<I, N>(mut self, tabs: I) -> TabGroup
    where
        I: IntoIterator<Item = N>,
        N: Into<TabNode>,
    {
        self.tabs.extend(tabs.into_iter().map(Into::into));
        self
    }

    /// Alias for [`Self::tab`], used by the `view!` macro's container lowering.
    pub fn child(self, tab: impl Into<TabNode>) -> TabGroup {
        self.tab(tab)
    }

    /// Number of top-level tabs configured before build.
    pub fn child_count(&self) -> usize {
        self.tabs.len()
    }

    /// Number of tabs in this group, including nested descendants.
    pub fn tab_count(&self) -> usize {
        self.tabs.iter().map(TabNode::tab_count).sum()
    }
}

/// A vertical, labelled collection of exclusive tabs.
///
/// The default is a compact flat list. Call [`Self::tree`] to preserve nested
/// [`TabNode`] hierarchy. Selection is exclusive across every group in this
/// component, while each leaf remains a normal [`Role::Tab`] and the root remains
/// a single [`Role::TabList`].
pub struct GroupedTabList {
    groups: Vec<TabGroup>,
    mode: GroupedTabMode,
    group_gap: f32,
    tab_gap: f32,
    indent: f32,
    show_group_labels: bool,
    group_label_size: f32,
    min_tab_width: Option<f32>,
    tab_appearance: TabAppearance,
    action_gap: f32,
}

#[derive(Clone, Copy)]
struct GroupedTabStyle {
    tab_gap: f32,
    indent: f32,
    min_tab_width: Option<f32>,
    appearance: TabAppearance,
    action_gap: f32,
}

impl GroupedTabList {
    /// Creates an empty flat grouped tab list.
    pub fn new() -> GroupedTabList {
        GroupedTabList {
            groups: Vec::new(),
            mode: GroupedTabMode::Flat,
            group_gap: 12.0,
            tab_gap: 2.0,
            indent: 16.0,
            show_group_labels: true,
            group_label_size: 12.0,
            min_tab_width: None,
            tab_appearance: TabAppearance::Navigation,
            action_gap: 2.0,
        }
    }

    /// Appends a labelled group.
    pub fn group(mut self, group: TabGroup) -> GroupedTabList {
        self.groups.push(group);
        self
    }

    /// Appends several groups from configuration data.
    pub fn groups(mut self, groups: impl IntoIterator<Item = TabGroup>) -> GroupedTabList {
        self.groups.extend(groups);
        self
    }

    /// Alias for [`Self::group`], used by the `view!` macro's container lowering.
    pub fn child(self, group: TabGroup) -> GroupedTabList {
        self.group(group)
    }

    /// Selects the flat or tree presentation explicitly.
    pub fn mode(mut self, mode: GroupedTabMode) -> GroupedTabList {
        self.mode = mode;
        self
    }

    /// Enables the indented tree presentation.
    pub fn tree(mut self) -> GroupedTabList {
        self.mode = GroupedTabMode::Tree;
        self
    }

    /// Selects the flat depth-first presentation.
    pub fn flat(mut self) -> GroupedTabList {
        self.mode = GroupedTabMode::Flat;
        self
    }

    /// Sets the vertical distance between groups.
    pub fn group_gap(mut self, gap: f32) -> GroupedTabList {
        self.group_gap = gap.max(0.0);
        self
    }

    /// Sets the vertical distance between tabs and tree branches.
    pub fn tab_gap(mut self, gap: f32) -> GroupedTabList {
        self.tab_gap = gap.max(0.0);
        self
    }

    /// Sets the logical-pixel indentation added at each tree depth.
    pub fn indent(mut self, indent: f32) -> GroupedTabList {
        self.indent = indent.max(0.0);
        self
    }

    /// Shows or hides visible group headings. Group names remain available in
    /// the accessibility tree either way.
    pub fn show_group_labels(mut self, show: bool) -> GroupedTabList {
        self.show_group_labels = show;
        self
    }

    /// Hides visible group headings.
    pub fn hide_group_labels(mut self) -> GroupedTabList {
        self.show_group_labels = false;
        self
    }

    /// Sets the group-heading font size.
    pub fn group_label_size(mut self, size: f32) -> GroupedTabList {
        self.group_label_size = size.max(1.0);
        self
    }

    /// Gives top-level tabs this minimum width. Nested tree tabs subtract their
    /// accumulated indentation so every row ends at approximately the same edge.
    pub fn min_tab_width(mut self, width: f32) -> GroupedTabList {
        self.min_tab_width = Some(width.max(0.0));
        self
    }

    /// Sets the visual treatment for every tab in the grouped list. The default
    /// [`TabAppearance::Navigation`] is optimized for vertical flat/tree lists;
    /// choose [`TabAppearance::Classic`] for the traditional filled tab surface.
    pub fn tab_appearance(mut self, appearance: TabAppearance) -> GroupedTabList {
        self.tab_appearance = appearance;
        self
    }

    /// Sets the horizontal distance between a tab and its trailing action buttons.
    pub fn action_gap(mut self, gap: f32) -> GroupedTabList {
        self.action_gap = gap.max(0.0);
        self
    }

    /// The semantic role of the complete list.
    pub fn role(&self) -> Role {
        Role::TabList
    }

    /// The retained dispatch kind used for the semantic list container.
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::TabBar
    }

    /// Number of groups configured before build.
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Number of tabs in every group, including nested descendants.
    pub fn tab_count(&self) -> usize {
        self.groups.iter().map(TabGroup::tab_count).sum()
    }
}

impl Default for GroupedTabList {
    fn default() -> Self {
        Self::new()
    }
}

impl View for GroupedTabList {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let root = ctx.scene.insert(WidgetKind::TabBar, parent);
        ctx.scene.a11y_mut(root).role = Role::TabList.as_u16();
        let mut root_style = ContainerStyle::new(Container::Column);
        root_style.gap = this.group_gap;
        ctx.layout.set_container(root, root_style);

        let tab_style = GroupedTabStyle {
            tab_gap: this.tab_gap,
            indent: this.indent,
            min_tab_width: this.min_tab_width,
            appearance: this.tab_appearance,
            action_gap: this.action_gap,
        };
        for group in this.groups {
            build_group(
                ctx,
                root,
                group,
                this.mode,
                this.show_group_labels,
                this.group_label_size,
                tab_style,
            );
        }
        root
    }
}

fn build_group(
    ctx: &mut BuildCtx,
    parent: WidgetId,
    group: TabGroup,
    mode: GroupedTabMode,
    show_label: bool,
    label_size: f32,
    tab_style: GroupedTabStyle,
) {
    let group_id = ctx.scene.insert(WidgetKind::Column, Some(parent));
    {
        let a11y = ctx.scene.a11y_mut(group_id);
        a11y.role = Role::Group.as_u16();
        a11y.name = Some(group.label.to_string());
    }
    let mut group_style = ContainerStyle::new(Container::Column);
    group_style.gap = tab_style.tab_gap;
    ctx.layout.set_container(group_id, group_style);

    if show_label && !group.label.is_empty() {
        Box::new(Text::new(group.label).size(label_size)).build(ctx, Some(group_id));
    }

    let tabs_id = ctx.scene.insert(WidgetKind::Column, Some(group_id));
    ctx.scene.a11y_mut(tabs_id).role = Role::Group.as_u16();
    let mut tabs_style = ContainerStyle::new(Container::Column);
    tabs_style.gap = tab_style.tab_gap;
    ctx.layout.set_container(tabs_id, tabs_style);

    match mode {
        GroupedTabMode::Flat => {
            for node in group.tabs {
                build_flat_node(ctx, tabs_id, node, tab_style);
            }
        }
        GroupedTabMode::Tree => {
            for node in group.tabs {
                build_tree_node(ctx, tabs_id, node, tab_style, 0);
            }
        }
    }
}

fn build_tab(
    ctx: &mut BuildCtx,
    parent: WidgetId,
    mut tab: Tab,
    min_tab_width: Option<f32>,
    appearance: TabAppearance,
) -> (WidgetId, WidgetId) {
    tab = tab.appearance(appearance);
    if let Some(width) = min_tab_width {
        tab = tab.width(width);
    }
    let root = Box::new(tab).build(ctx, Some(parent));
    let id = if ctx
        .scene
        .node(root)
        .is_some_and(|node| node.kind == WidgetKind::Tab)
    {
        root
    } else {
        ctx.scene
            .node(root)
            .and_then(|node| {
                node.children.iter().copied().find(|child| {
                    ctx.scene
                        .node(*child)
                        .is_some_and(|node| node.kind == WidgetKind::Tab)
                })
            })
            .expect("Tab::build must return a tab or a wrapper containing one")
    };
    if let Some(width) = min_tab_width {
        ctx.layout.set_flex(
            root,
            FlexChild {
                min_width: Some(width),
                ..FlexChild::default()
            },
        );
    }
    (root, id)
}

fn build_tab_row(
    ctx: &mut BuildCtx,
    parent: WidgetId,
    tab: Tab,
    actions: Vec<crate::AnyView>,
    min_tab_width: Option<f32>,
    style: GroupedTabStyle,
) -> WidgetId {
    build_tab_row_with_trigger(ctx, parent, tab, actions, min_tab_width, style, None)
}

fn build_tab_row_with_trigger(
    ctx: &mut BuildCtx,
    parent: WidgetId,
    tab: Tab,
    actions: Vec<crate::AnyView>,
    min_tab_width: Option<f32>,
    style: GroupedTabStyle,
    trigger_label: Option<Cow<'static, str>>,
) -> WidgetId {
    let has_trigger = trigger_label.is_some();
    let mut trigger_label = trigger_label;
    // Fast path: no actions and no trigger -> direct tab
    if actions.is_empty() && !has_trigger {
        return build_tab(ctx, parent, tab, min_tab_width, style.appearance).1;
    }

    let row_id = ctx.scene.insert(WidgetKind::Row, Some(parent));
    ctx.scene.a11y_mut(row_id).role = Role::Group.as_u16();
    let mut row_style = ContainerStyle::new(Container::Row);
    row_style.align = Align::Center;
    row_style.gap = style.action_gap;
    row_style.width = min_tab_width;
    ctx.layout.set_container(row_id, row_style);

    let (tab_root, tab_id) = build_tab(ctx, row_id, tab, None, style.appearance);
    ctx.layout.set_flex(
        tab_root,
        FlexChild {
            grow: Some(1.0),
            shrink: Some(1.0),
            min_width: Some(0.0),
            ..FlexChild::default()
        },
    );
    for action in actions {
        action.build(ctx, Some(row_id));
    }
    if let Some(label) = trigger_label.take() {
        let label_owned: String = label.into_owned();
        let button = crate::Button::new(label_owned).appearance(crate::ButtonAppearance::Ghost);
        let trigger_id = Box::new(button).build(ctx, Some(row_id));
        let leaf = ctx
            .scene
            .node(trigger_id)
            .and_then(|node| {
                if node.kind == crate::WidgetKind::Button {
                    Some(trigger_id)
                } else {
                    node.children.iter().copied().find(|child| {
                        ctx.scene
                            .node(*child)
                            .is_some_and(|n| n.kind == crate::WidgetKind::Button)
                    })
                }
            })
            .unwrap_or(trigger_id);
        crate::context_menu::register_context_menu_trigger(&ctx.runtime, leaf, tab_id);
    }
    tab_id
}

fn build_flat_node(ctx: &mut BuildCtx, parent: WidgetId, node: TabNode, style: GroupedTabStyle) {
    if node.separator {
        Box::new(crate::Divider::new()).build(ctx, Some(parent));
        for child in node.children {
            build_flat_node(ctx, parent, child, style);
        }
        return;
    }
    let TabNode {
        tab,
        children,
        actions,
        context_menu_trigger,
        ..
    } = node;
    build_tab_row_with_trigger(
        ctx,
        parent,
        tab,
        actions,
        style.min_tab_width,
        style,
        context_menu_trigger,
    );
    for child in children {
        build_flat_node(ctx, parent, child, style);
    }
}

fn build_tree_node(
    ctx: &mut BuildCtx,
    parent: WidgetId,
    node: TabNode,
    style: GroupedTabStyle,
    depth: usize,
) {
    if node.separator {
        // Simple full-width divider; indent is handled by the parent's
        // tree layout, and a full-width hairline is the expected visual
        // break between worktree groups.
        Box::new(crate::Divider::new()).build(ctx, Some(parent));
        for child in node.children {
            build_tree_node(ctx, parent, child, style, depth);
        }
        return;
    }
    let TabNode {
        mut tab,
        children,
        actions,
        expanded,
        mut on_toggle,
        context_menu_trigger,
        ..
    } = node;
    let node_id = ctx.scene.insert(WidgetKind::Column, Some(parent));
    ctx.scene.a11y_mut(node_id).role = Role::Group.as_u16();
    let mut node_style = ContainerStyle::new(Container::Column);
    node_style.gap = style.tab_gap;
    ctx.layout.set_container(node_id, node_style);

    let accumulated_indent = style.indent * depth as f32;
    let node_min_width = style
        .min_tab_width
        .map(|width| (width - accumulated_indent).max(0.0));
    let has_children = !children.is_empty();
    tab = tab.tree_disclosure(has_children.then_some(expanded));
    if has_children {
        if let Some(mut toggle) = on_toggle.take() {
            let mut select = tab.on_select.take();
            tab.on_select = Some(Box::new(move || {
                if let Some(select) = select.as_mut() {
                    select();
                }
                toggle(!expanded);
            }));
        }
    }
    let tab_id = build_tab_row_with_trigger(
        ctx,
        node_id,
        tab,
        actions,
        node_min_width,
        style,
        context_menu_trigger,
    );
    if has_children {
        let a11y = ctx.scene.a11y_mut(tab_id);
        let mut state = StateFlags(a11y.state);
        state.insert(StateFlags::COLLAPSIBLE);
        if expanded {
            state.insert(StateFlags::EXPANDED);
        }
        a11y.state = state.0;
    }

    if expanded && has_children {
        let inset_id = ctx.scene.insert(WidgetKind::Pad, Some(node_id));
        ctx.scene.a11y_mut(inset_id).role = Role::Group.as_u16();
        ctx.layout.set_container(
            inset_id,
            ContainerStyle::new(Container::Pad(EdgeInsets {
                left: style.indent,
                ..EdgeInsets::default()
            })),
        );

        let children_id = ctx.scene.insert(WidgetKind::Column, Some(inset_id));
        ctx.scene.a11y_mut(children_id).role = Role::Group.as_u16();
        let mut children_style = ContainerStyle::new(Container::Column);
        children_style.gap = style.tab_gap;
        ctx.layout.set_container(children_id, children_style);
        for child in children {
            build_tree_node(ctx, children_id, child, style, depth + 1);
        }
    }
}

/// Short alias for callers that prefer the component name without “List”.
pub type GroupedTabs = GroupedTabList;

/// Alias matching the existing [`crate::TabBar`] naming convention.
pub type GroupedTabBar = GroupedTabList;

/// Descriptive alias for a node used specifically in tree mode.
pub type TabTreeNode = TabNode;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dispatch_click, reset, Button};
    use schnellui_layout::LayoutEngine;
    use schnellui_scene::{Scene, Size};
    use schnellui_signal::create_signal;
    use schnellui_text::{GlyphAtlas, TextShaper};

    fn build_one(
        runtime: &crate::Runtime,
        view: impl View,
    ) -> (Scene, LayoutEngine, TextShaper, GlyphAtlas, WidgetId) {
        reset(runtime);
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
            Box::new(view).build(&mut ctx, None)
        };
        scene.set_root(id);
        (scene, layout, text, atlas, id)
    }

    fn collect_role(scene: &Scene, id: WidgetId, role: Role, out: &mut Vec<WidgetId>) {
        if scene
            .a11y(id)
            .is_some_and(|a11y| Role::from_u16(a11y.role) == role)
        {
            out.push(id);
        }
        if let Some(node) = scene.node(id) {
            for &child in &node.children {
                collect_role(scene, child, role, out);
            }
        }
    }

    fn tab_named(scene: &Scene, root: WidgetId, name: &str) -> WidgetId {
        let mut tabs = Vec::new();
        collect_role(scene, root, Role::Tab, &mut tabs);
        tabs.into_iter()
            .find(|&id| scene.a11y(id).and_then(|a11y| a11y.name.as_deref()) == Some(name))
            .unwrap_or_else(|| panic!("missing tab {name:?}"))
    }

    #[test]
    fn builder_reports_groups_and_recursive_tab_count() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let node = TabNode::new("Actions").actions([Button::new("Refresh"), Button::new("Close")]);
        assert_eq!(node.action_count(), 2);

        let list = GroupedTabList::new()
            .tree()
            .group(
                TabGroup::new("Workspace").tab(
                    TabNode::new("Editor")
                        .child(TabNode::new("Outline"))
                        .child(Tab::new("Search")),
                ),
            )
            .group(TabGroup::new("Account").tab(TabNode::new("Settings")));

        assert_eq!(list.role(), Role::TabList);
        assert_eq!(list.kind(), WidgetKind::TabBar);
        assert_eq!(list.mode, GroupedTabMode::Tree);
        assert_eq!(list.group_count(), 2);
        assert_eq!(list.tab_count(), 4);
    }

    #[test]
    fn flat_mode_flattens_trees_and_keeps_group_semantics() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _layout, _text, _atlas, root) = build_one(
            runtime,
            GroupedTabList::new()
                .group(
                    TabGroup::new("Workspace").tab(
                        TabNode::new("Editor")
                            .expanded(false)
                            .child(TabNode::new("Outline")),
                    ),
                )
                .group(TabGroup::new("Account").tab(TabNode::new("Settings"))),
        );

        assert_eq!(
            Role::from_u16(scene.a11y(root).unwrap().role),
            Role::TabList
        );
        let groups = scene.node(root).unwrap().children.clone();
        assert_eq!(groups.len(), 2);
        assert_eq!(
            scene.a11y(groups[0]).unwrap().name.as_deref(),
            Some("Workspace")
        );
        assert_eq!(
            scene.a11y(groups[1]).unwrap().name.as_deref(),
            Some("Account")
        );

        let mut tabs = Vec::new();
        collect_role(&scene, root, Role::Tab, &mut tabs);
        let names: Vec<_> = tabs
            .iter()
            .map(|&id| scene.a11y(id).unwrap().name.as_deref().unwrap())
            .collect();
        assert_eq!(names, ["Editor", "Outline", "Settings"]);
    }

    #[test]
    fn tree_mode_indents_children_and_omits_collapsed_descendants() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _text, _atlas, root) = build_one(
            runtime,
            GroupedTabList::new()
                .tree()
                .indent(20.0)
                .min_tab_width(180.0)
                .group(
                    TabGroup::new("Workspace")
                        .tab(TabNode::new("Editor").child(TabNode::new("Outline")))
                        .tab(
                            TabNode::new("Collapsed")
                                .expanded(false)
                                .child(TabNode::new("Hidden")),
                        ),
                ),
        );
        layout.sync_tree(&scene, root);
        layout.compute(
            &mut scene,
            root,
            Size {
                width: 400.0,
                height: 400.0,
            },
        );
        crate::reposition_paint(runtime, &mut scene);

        let editor = tab_named(&scene, root, "Editor");
        let outline = tab_named(&scene, root, "Outline");
        let collapsed = tab_named(&scene, root, "Collapsed");
        let editor_rect = scene.layout(editor).unwrap().rect;
        let outline_rect = scene.layout(outline).unwrap().rect;
        assert_eq!(outline_rect.x, editor_rect.x + 20.0);
        assert_eq!(outline_rect.right(), editor_rect.right());
        let schnellui_scene::Primitive::SolidRect {
            rect: editor_surface,
            ..
        } = scene.paint(editor).unwrap().primitives[0]
        else {
            panic!("tree tab surface must be a solid rectangle");
        };
        let schnellui_scene::Primitive::SolidRect {
            rect: outline_surface,
            color: outline_color,
            ..
        } = scene.paint(outline).unwrap().primitives[0]
        else {
            panic!("nested tree tab surface must be a solid rectangle");
        };
        assert_eq!(editor_surface, editor_rect);
        assert_eq!(outline_surface, outline_rect);
        assert_eq!(outline_color, schnellui_scene::Color::TRANSPARENT);
        let editor_state = StateFlags(scene.a11y(editor).unwrap().state);
        let collapsed_state = StateFlags(scene.a11y(collapsed).unwrap().state);
        assert!(editor_state.contains(StateFlags::COLLAPSIBLE));
        assert!(editor_state.contains(StateFlags::EXPANDED));
        assert!(collapsed_state.contains(StateFlags::COLLAPSIBLE));
        assert!(!collapsed_state.contains(StateFlags::EXPANDED));
        assert_eq!(
            scene
                .paint(editor)
                .unwrap()
                .primitives
                .iter()
                .filter(|primitive| matches!(primitive, schnellui_scene::Primitive::Line { .. }))
                .count(),
            2,
            "an expandable row paints one two-segment disclosure chevron"
        );

        let mut tabs = Vec::new();
        collect_role(&scene, root, Role::Tab, &mut tabs);
        assert!(!tabs
            .iter()
            .any(|&id| scene.a11y(id).unwrap().name.as_deref() == Some("Hidden")));
    }

    #[test]
    fn selection_is_exclusive_across_groups_and_tree_depths() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let chosen = create_signal(String::new());
        let (mut scene, _layout, _text, _atlas, root) = build_one(
            runtime,
            GroupedTabList::new()
                .tree()
                .group(
                    TabGroup::new("Workspace").tab(
                        TabNode::new("Editor")
                            .selected(true)
                            .child(TabNode::new("Outline")),
                    ),
                )
                .group(TabGroup::new("Account").tab(
                    TabNode::new("Settings").on_select(move || chosen.set("Settings".to_string())),
                )),
        );
        let editor = tab_named(&scene, root, "Editor");
        let settings = tab_named(&scene, root, "Settings");

        assert!(dispatch_click(runtime, &mut scene, settings));
        assert_eq!(chosen.get(), "Settings");
        assert!(!StateFlags(scene.a11y(editor).unwrap().state).contains(StateFlags::SELECTED));
        assert!(StateFlags(scene.a11y(settings).unwrap().state).contains(StateFlags::SELECTED));
    }

    #[test]
    fn branch_activation_selects_and_requests_the_next_expansion_state() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let requested = create_signal(None::<bool>);
        let (mut scene, _layout, _text, _atlas, root) = build_one(
            runtime,
            GroupedTabList::new().tree().group(
                TabGroup::new("Workspace").tab(
                    TabNode::new("Editor")
                        .expanded(true)
                        .on_toggle(move |next| requested.set(Some(next)))
                        .child(TabNode::new("Outline")),
                ),
            ),
        );
        let editor = tab_named(&scene, root, "Editor");

        assert!(dispatch_click(runtime, &mut scene, editor));
        assert_eq!(requested.get(), Some(false));
        assert!(StateFlags(scene.a11y(editor).unwrap().state).contains(StateFlags::SELECTED));
    }

    #[test]
    fn trailing_action_is_independent_and_shares_the_configured_row_width() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let invoked = create_signal(0usize);
        let (mut scene, mut layout, _text, _atlas, root) = build_one(
            runtime,
            GroupedTabList::new().min_tab_width(180.0).group(
                TabGroup::new("Workspace").tab(
                    TabNode::new("Editor").action(
                        Button::new("Refresh")
                            .appearance(crate::ButtonAppearance::Ghost)
                            .on_click(move || invoked.update(|count| *count += 1)),
                    ),
                ),
            ),
        );
        layout.sync_tree(&scene, root);
        layout.compute(
            &mut scene,
            root,
            Size {
                width: 400.0,
                height: 200.0,
            },
        );
        crate::reposition_paint(runtime, &mut scene);

        let editor = tab_named(&scene, root, "Editor");
        let mut buttons = Vec::new();
        collect_role(&scene, root, Role::Button, &mut buttons);
        let refresh = buttons[0];
        let editor_rect = scene.layout(editor).unwrap().rect;
        let refresh_rect = scene.layout(refresh).unwrap().rect;
        let row = scene.node(editor).unwrap().parent.unwrap();
        let row_rect = scene.layout(row).unwrap().rect;
        assert_eq!(row_rect.width, 180.0);
        assert!(editor_rect.right() <= refresh_rect.x);
        assert_eq!(refresh_rect.right(), row_rect.right());

        assert!(dispatch_click(runtime, &mut scene, refresh));
        assert_eq!(invoked.get(), 1);
        assert!(!StateFlags(scene.a11y(editor).unwrap().state).contains(StateFlags::SELECTED));
    }
}
