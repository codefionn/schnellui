use crate::types::*;
use schnellui_scene::{LayoutBox, Point, Rect, Scene, Size, WidgetId, WidgetKind};
use slotmap::SecondaryMap;
use smallvec::SmallVec;
use taffy::style_helpers::{length, percent};

pub struct LayoutEngine {
    taffy: taffy::TaffyTree<WidgetId>,
    map: SecondaryMap<WidgetId, taffy::NodeId>,
    styles: SecondaryMap<WidgetId, ContainerStyle>,
    /// Per-child flex factors (SOUL §8.1) — folded into the node's Taffy style
    /// after its container/leaf base, so only explicitly-set fields override.
    flexes: SecondaryMap<WidgetId, FlexChild>,
    measures: SecondaryMap<WidgetId, MeasureFn>,
    /// Content leaves whose width should fill the parent's content box (SOUL §8.1) —
    /// set for *wrapping* text so Taffy hands the leaf a **definite** available width
    /// and it wraps to the line box instead of sizing to its unwrapped max-content.
    /// The leaf's height still comes from its width-aware measurement.
    fills: SecondaryMap<WidgetId, ()>,
    /// Responsive rules registered by node-transparent `show_when` wrappers.
    responsive: SecondaryMap<WidgetId, ResponsiveQuery>,
    /// Last resolved direct visibility for each responsive root. Absent means
    /// visible; descendants inherit hiddenness during downstream tree walks.
    responsive_visible: SecondaryMap<WidgetId, bool>,
    /// Grow-only traversal stack, cleared-and-refilled per `compute` (SOUL §4.4) so
    /// writing the layout column allocates nothing in steady state.
    scratch: Vec<(WidgetId, Point)>,
    /// Grow-only query-resolution scratch. Responsive resize work may grow it once;
    /// subsequent relayouts clear-and-refill without allocating.
    responsive_scratch: Vec<(WidgetId, bool)>,
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutEngine {
    /// A fresh engine with an empty Taffy tree.
    ///
    /// Rounding is **disabled** so measured sizes pass through geometry exactly —
    /// pixel snapping is the renderer/compositor's job, not layout's, and exact
    /// geometry keeps the screenshot goldens deterministic (SOUL §7.3).
    pub fn new() -> LayoutEngine {
        let mut taffy = taffy::TaffyTree::new();
        taffy.disable_rounding();
        LayoutEngine {
            taffy,
            map: SecondaryMap::new(),
            styles: SecondaryMap::new(),
            flexes: SecondaryMap::new(),
            measures: SecondaryMap::new(),
            fills: SecondaryMap::new(),
            responsive: SecondaryMap::new(),
            responsive_visible: SecondaryMap::new(),
            scratch: Vec::new(),
            responsive_scratch: Vec::new(),
        }
    }

    /// Registers a responsive visibility rule against a node's own Taffy style.
    /// The wrapper is node-transparent: when the query matches this is the exact
    /// same tree; when it does not, Taffy's `display: none` removes the whole
    /// subtree from layout.
    pub fn set_responsive(&mut self, id: WidgetId, query: ResponsiveQuery) {
        self.responsive.insert(id, query);
        self.responsive_visible.insert(id, true);
        if let Some(&t) = self.map.get(id) {
            let _ = self.taffy.mark_dirty(t);
        }
    }

    /// Shows or hides a retained subtree without reconstructing it.
    ///
    /// Hidden roots use Taffy's `display: none` and the scene's matching
    /// visibility bit, so layout, paint, hit-testing, and accessibility all agree.
    /// Interactive widgets use this for local structural-looking changes whose
    /// complete set of children is already known at mount time.
    pub fn set_visible(&mut self, scene: &mut Scene, id: WidgetId, visible: bool) -> bool {
        self.apply_responsive_visibility(scene, id, visible)
    }

    /// Marks a content leaf as **width-filling** (SOUL §8.1): its Taffy width becomes
    /// `100%` of the parent content box, so the measure pass is handed a definite
    /// available width and a *wrapping* text leaf wraps to the line box (its height
    /// still comes from the width-aware [`DynMeasure`]). Idempotent; invalidates the
    /// node's cache so the next [`Self::compute`] picks up the new style.
    pub fn set_fill_width(&mut self, id: WidgetId) {
        self.fills.insert(id, ());
        if let Some(&t) = self.map.get(id) {
            let _ = self.taffy.mark_dirty(t);
        }
    }

    /// Registers/overwrites a container node's style (SOUL §8.1).
    ///
    /// The style is applied to the Taffy node on the next [`Self::sync_tree`]; if
    /// the node is already synced its cache is invalidated so the next
    /// [`Self::compute`] relayouts it and its ancestors.
    pub fn set_container(&mut self, id: WidgetId, style: ContainerStyle) {
        self.styles.insert(id, style);
        if let Some(&t) = self.map.get(id) {
            let _ = self.taffy.mark_dirty(t);
        }
    }

    /// Returns the retained style registered for a container. Interactive
    /// components use this to update one geometric property (for example a
    /// dialog's absolute position or size) without reconstructing its other
    /// padding/alignment settings.
    pub fn container_style(&self, id: WidgetId) -> Option<ContainerStyle> {
        self.styles.get(id).copied()
    }

    /// Registers/overwrites a node's per-child flex factors (SOUL §8.1) — its
    /// responsive share of the flex parent's main axis. Applies to containers and
    /// content leaves alike; only the [`FlexChild`] fields that are `Some` override
    /// the node's base style.
    ///
    /// Like [`Self::set_container`], the factors are applied to the Taffy node on
    /// the next [`Self::sync_tree`]; an already-synced node's cache is invalidated
    /// so the next sync + [`Self::compute`] relayouts it and its ancestors.
    pub fn set_flex(&mut self, id: WidgetId, flex: FlexChild) {
        self.flexes.insert(id, flex);
        if let Some(&t) = self.map.get(id) {
            let _ = self.taffy.mark_dirty(t);
        }
    }

    /// Registers a content leaf's intrinsic-size measurement (SOUL §8.1).
    ///
    /// The measurement is looked up *live* during [`Self::compute`], so swapping it
    /// only needs a Taffy cache invalidation (this node + its ancestors) — the
    /// *layout-dirty* channel: the next `compute` re-measures the minimal region.
    pub fn set_measure(&mut self, id: WidgetId, measure: MeasureFn) {
        self.measures.insert(id, measure);
        if let Some(&t) = self.map.get(id) {
            let _ = self.taffy.mark_dirty(t);
        }
    }

    /// (Re)builds the Taffy node graph from the scene's tree for a subtree —
    /// called on structure change / mount (SOUL §4 mount may allocate).
    ///
    /// Idempotent: already-synced nodes get their style + child list refreshed;
    /// new nodes are created (content leaves carry their [`WidgetId`] as context).
    pub fn sync_tree(&mut self, scene: &Scene, root: WidgetId) {
        if scene.node(root).is_some() {
            self.sync_node(scene, root);
        }
    }

    /// Drops the layout-side records for scene nodes that no longer exist.
    ///
    /// Structural subtree replacement calls this before syncing the new branch;
    /// unaffected Taffy nodes and their cached measurements remain resident.
    pub fn remove_nodes(&mut self, nodes: &[WidgetId]) {
        for &id in nodes.iter().rev() {
            if let Some(node) = self.map.remove(id) {
                let _ = self.taffy.remove(node);
            }
            self.styles.remove(id);
            self.flexes.remove(id);
            self.measures.remove(id);
            self.fills.remove(id);
            self.responsive.remove(id);
            self.responsive_visible.remove(id);
        }
        self.scratch.retain(|(id, _)| !nodes.contains(id));
        self.responsive_scratch
            .retain(|(id, _)| !nodes.contains(id));
    }

    /// Syncs a newly built branch and reconnects only its direct parent.
    ///
    /// Existing siblings already have Taffy nodes, so refreshing the parent's
    /// child-id list avoids a whole-scene structural walk. If this is called
    /// before the first layout, it falls back to syncing that parent normally.
    pub fn sync_replacement(
        &mut self,
        scene: &Scene,
        new_root: WidgetId,
        parent: Option<WidgetId>,
    ) {
        let _ = self.sync_node(scene, new_root);
        let Some(parent) = parent else {
            return;
        };
        let Some(parent_node) = scene.node(parent) else {
            return;
        };
        let Some(parent_taffy) = self.map.get(parent).copied() else {
            let _ = self.sync_node(scene, parent);
            return;
        };
        let mut children = SmallVec::<[taffy::NodeId; 4]>::new();
        for child in &parent_node.children {
            let Some(child_taffy) = self.map.get(*child).copied() else {
                let _ = self.sync_node(scene, parent);
                return;
            };
            children.push(child_taffy);
        }
        let _ = self
            .taffy
            .set_style(parent_taffy, self.build_style(scene, parent));
        let _ = self.taffy.set_children(parent_taffy, &children);
        let _ = self.taffy.mark_dirty(parent_taffy);
    }

    /// Refreshes only scene nodes present in the layout-dirty queue.
    ///
    /// Post-mount style changes need their retained Taffy style updated, but do
    /// not require recursively revisiting clean siblings. Missing nodes (for a
    /// custom structural mutation) are synchronized with their descendants.
    pub fn sync_dirty_nodes(&mut self, scene: &Scene, nodes: &[WidgetId]) {
        for &id in nodes {
            let Some(scene_node) = scene.node(id) else {
                continue;
            };
            let Some(taffy_node) = self.map.get(id).copied() else {
                let _ = self.sync_node(scene, id);
                continue;
            };
            let _ = self
                .taffy
                .set_style(taffy_node, self.build_style(scene, id));
            if scene_node.kind.is_container() {
                let mut children = SmallVec::<[taffy::NodeId; 4]>::new();
                for &child in &scene_node.children {
                    let child = self
                        .map
                        .get(child)
                        .copied()
                        .unwrap_or_else(|| self.sync_node(scene, child));
                    children.push(child);
                }
                let _ = self.taffy.set_children(taffy_node, &children);
            }
            let _ = self.taffy.mark_dirty(taffy_node);
        }
    }

    /// Recursively mirror one scene node (and its subtree) into Taffy, returning the
    /// backing Taffy node id. Mount-time; may allocate (SOUL §4).
    fn sync_node(&mut self, scene: &Scene, id: WidgetId) -> taffy::NodeId {
        let (kind, children) = {
            let node = scene.node(id).expect("sync_node: id must be live");
            (node.kind, node.children.clone())
        };
        let style = self.build_style(scene, id);

        let t = match self.map.get(id).copied() {
            Some(t) => {
                let _ = self.taffy.set_style(t, style);
                t
            }
            None => {
                let t = if kind.is_container() {
                    self.taffy.new_leaf(style).expect("taffy new_leaf")
                } else {
                    // content leaf: carry the WidgetId so the measure pass can find
                    // its MeasureFn (SOUL §8.1 intrinsic-size seam).
                    self.taffy
                        .new_leaf_with_context(style, id)
                        .expect("taffy new_leaf_with_context")
                };
                self.map.insert(id, t);
                t
            }
        };

        let mut kids: SmallVec<[taffy::NodeId; 4]> = SmallVec::new();
        for child in children {
            kids.push(self.sync_node(scene, child));
        }
        let _ = self.taffy.set_children(t, &kids);
        t
    }

    /// Build the Taffy [`Style`](taffy::Style) for one node from its registered
    /// [`ContainerStyle`] (containers) or a bare auto-sized leaf style (content).
    fn build_style(&self, scene: &Scene, id: WidgetId) -> taffy::Style {
        let node = scene.node(id).expect("build_style: id must be live");
        let kind = node.kind;
        let parent_kind = node.parent.and_then(|p| scene.node(p)).map(|n| n.kind);

        let mut s = taffy::Style::DEFAULT;

        if kind.is_container() {
            let style = self
                .styles
                .get(id)
                .copied()
                .unwrap_or_else(|| ContainerStyle::new(default_container(kind)));
            apply_container(&mut s, &style);
        }
        // else: a content leaf keeps auto size — its MeasureFn supplies the
        // intrinsic size during compute. A *width-filling* leaf (wrapping text,
        // SOUL §8.1) instead takes 100% of the parent content width so the measure
        // pass is offered a definite width to wrap against; its height stays auto.
        if !kind.is_container() && self.fills.contains_key(id) {
            s.size.width = percent(1.0_f32);
        }

        // A child of a Stack overlays in Z: position it absolutely and pin all four
        // insets to 0 so it fills the stack's box (SOUL §8.1 `stack`) — unless the
        // child carries its own anchor, whose explicit inset must survive.
        let anchored = self.styles.get(id).is_some_and(|st| st.anchor.is_some());
        if parent_kind == Some(WidgetKind::Stack) && !anchored {
            s.position = taffy::Position::Absolute;
            s.inset = taffy::style_helpers::zero();
        }

        // Per-child flex factors last (SOUL §8.1): only set fields override the
        // base style above, so e.g. a Spacer's built-in grow survives an empty
        // FlexChild while an explicit `grow` re-weights it.
        if let Some(flex) = self.flexes.get(id) {
            apply_flex(&mut s, flex);
        }

        if self.responsive_visible.get(id).copied() == Some(false) {
            s.display = taffy::Display::None;
        }

        s
    }

    /// Computes layout for the smallest dirty subtree and writes each node's
    /// [`LayoutBox`] into the scene layout column (SOUL §8.1). `available` is the
    /// viewport (or subtree) constraint. Steady-state relayout reuses Taffy's
    /// caches — only the affected subtree recomputes, and only that subtree's boxes
    /// are (re)written, so clean siblings keep their geometry.
    pub fn compute(&mut self, scene: &mut Scene, root: WidgetId, available: Size) {
        self.compute_inner(scene, root, available, None);
    }

    /// Like [`Self::compute`] but threads a **width-aware** [`DynMeasure`] hook so a
    /// leaf whose intrinsic size depends on the offered width (wrapping text, SOUL
    /// §8.1) can shape on demand. The hook is tried first for every content leaf; a
    /// `None` result falls back to the node's registered fixed [`MeasureFn`], so
    /// static leaves (buttons, single-line text, images) keep their build-time size.
    pub fn compute_with(
        &mut self,
        scene: &mut Scene,
        root: WidgetId,
        available: Size,
        measurer: DynMeasure,
    ) {
        self.compute_inner(scene, root, available, Some(measurer));
    }

    fn compute_inner(
        &mut self,
        scene: &mut Scene,
        root: WidgetId,
        available: Size,
        mut measurer: Option<DynMeasure>,
    ) {
        let root_t = match self.map.get(root).copied() {
            Some(t) => t,
            None => return,
        };
        let avail = taffy::Size {
            width: taffy::AvailableSpace::Definite(available.width),
            height: taffy::AvailableSpace::Definite(available.height),
        };

        self.resolve_viewport_queries(scene, available);
        self.run_taffy(root_t, avail, &mut measurer);
        self.write_boxes(scene, root);

        // Parent/container queries need the parent's computed content box. Resolve
        // them after the first pass, then immediately recompute once if a rule
        // changed so callers never observe a one-frame layout flash.
        if self.resolve_container_queries(scene) {
            self.run_taffy(root_t, avail, &mut measurer);
            self.write_boxes(scene, root);
        }
    }

    fn run_taffy(
        &mut self,
        root: taffy::NodeId,
        available: taffy::Size<taffy::AvailableSpace>,
        measurer: &mut Option<DynMeasure>,
    ) {
        // Disjoint field borrows: the measure closure needs `measures`, the compute
        // needs `taffy`. Never hold anything across the closure that Taffy touches.
        let taffy = &mut self.taffy;
        let measures = &mut self.measures;
        let _ = taffy.compute_layout_with_measure(
            root,
            available,
            |known: taffy::Size<Option<f32>>,
             space: taffy::Size<taffy::AvailableSpace>,
             _node,
             ctx: Option<&mut WidgetId>,
             _style: &taffy::Style| {
                let measured = match ctx {
                    Some(wid) => {
                        let wid = *wid;
                        let aw = resolve_available(known.width, space.width);
                        let ah = resolve_available(known.height, space.height);
                        let avail = Size {
                            width: aw,
                            height: ah,
                        };
                        let out = measurer
                            .as_mut()
                            .and_then(|m| m(wid, avail))
                            .or_else(|| measures.get_mut(wid).map(|m| m(avail)));
                        match out {
                            Some(s) => taffy::Size {
                                width: s.width,
                                height: s.height,
                            },
                            None => taffy::Size {
                                width: 0.0,
                                height: 0.0,
                            },
                        }
                    }
                    None => taffy::Size {
                        width: 0.0,
                        height: 0.0,
                    },
                };
                taffy::Size {
                    width: known.width.unwrap_or(measured.width),
                    height: known.height.unwrap_or(measured.height),
                }
            },
        );
    }

    fn resolve_viewport_queries(&mut self, scene: &mut Scene, viewport: Size) {
        self.responsive_scratch.clear();
        for (id, query) in self.responsive.iter() {
            if query.target == ResponsiveTarget::Viewport {
                self.responsive_scratch.push((id, query.matches(viewport)));
            }
        }
        for index in 0..self.responsive_scratch.len() {
            let (id, visible) = self.responsive_scratch[index];
            self.apply_responsive_visibility(scene, id, visible);
        }
    }

    fn resolve_container_queries(&mut self, scene: &mut Scene) -> bool {
        self.responsive_scratch.clear();
        for (id, query) in self.responsive.iter() {
            if query.target != ResponsiveTarget::Viewport {
                let target = match query.target {
                    ResponsiveTarget::Viewport => None,
                    ResponsiveTarget::Parent => scene.node(id).and_then(|node| node.parent),
                    ResponsiveTarget::Component(reference) => scene
                        .resolve_ref(reference)
                        .filter(|target| is_strict_ancestor(scene, *target, id)),
                };
                let visible = target
                    .and_then(|target| scene.layout(target))
                    .map(|layout| Size {
                        width: layout.content.width,
                        height: layout.content.height,
                    })
                    .is_some_and(|size| query.matches(size));
                self.responsive_scratch.push((id, visible));
            }
        }
        let mut changed = false;
        for index in 0..self.responsive_scratch.len() {
            let (id, visible) = self.responsive_scratch[index];
            changed |= self.apply_responsive_visibility(scene, id, visible);
        }
        changed
    }

    fn apply_responsive_visibility(
        &mut self,
        scene: &mut Scene,
        id: WidgetId,
        visible: bool,
    ) -> bool {
        if self.responsive_visible.get(id).copied() == Some(visible) {
            scene.set_visible(id, visible);
            return false;
        }
        self.responsive_visible.insert(id, visible);
        scene.set_visible(id, visible);
        if let Some(&node) = self.map.get(id) {
            let style = self.build_style(scene, id);
            let _ = self.taffy.set_style(node, style);
            let _ = self.taffy.mark_dirty(node);
        }
        true
    }

    /// Walk the just-computed subtree and fold Taffy's parent-relative rects into
    /// absolute window-space [`LayoutBox`]es. A subtree relayout keeps `root` at its
    /// previously-computed origin so siblings above are not disturbed.
    fn write_boxes(&mut self, scene: &mut Scene, root: WidgetId) {
        let is_true_root =
            scene.root() == Some(root) || scene.node(root).and_then(|n| n.parent).is_none();
        let origin = if is_true_root {
            Point { x: 0.0, y: 0.0 }
        } else {
            scene
                .layout(root)
                .map(|b| Point {
                    x: b.rect.x,
                    y: b.rect.y,
                })
                .unwrap_or(Point { x: 0.0, y: 0.0 })
        };

        self.scratch.clear();
        self.scratch.push((root, origin));

        while let Some((wid, abs)) = self.scratch.pop() {
            let t = match self.map.get(wid).copied() {
                Some(t) => t,
                None => continue,
            };
            let layout = match self.taffy.layout(t) {
                Ok(l) => *l,
                Err(_) => continue,
            };

            let inset_l = layout.border.left + layout.padding.left;
            let inset_t = layout.border.top + layout.padding.top;
            let inset_r = layout.border.right + layout.padding.right;
            let inset_b = layout.border.bottom + layout.padding.bottom;

            let rect = Rect::new(abs.x, abs.y, layout.size.width, layout.size.height);
            let content = Rect::new(
                abs.x + inset_l,
                abs.y + inset_t,
                (layout.size.width - inset_l - inset_r).max(0.0),
                (layout.size.height - inset_t - inset_b).max(0.0),
            );
            scene.set_layout(wid, LayoutBox { rect, content });

            if let Some(node) = scene.node(wid) {
                for &child in &node.children {
                    if let Some(&ct) = self.map.get(child) {
                        if let Ok(cl) = self.taffy.layout(ct) {
                            self.scratch.push((
                                child,
                                Point {
                                    x: abs.x + cl.location.x,
                                    y: abs.y + cl.location.y,
                                },
                            ));
                        }
                    }
                }
            }
        }
    }

    /// The Taffy node backing a widget, if synced.
    pub fn taffy_node(&self, id: WidgetId) -> Option<taffy::NodeId> {
        self.map.get(id).copied()
    }
}

/// Resolve the concrete available length handed to a [`MeasureFn`]: prefer a
/// dimension Taffy already fixed, else the definite constraint, else the
/// min/max-content sentinels (0 / ∞) so a widget can decide.
#[inline]
fn resolve_available(known: Option<f32>, space: taffy::AvailableSpace) -> f32 {
    known.unwrap_or(match space {
        taffy::AvailableSpace::Definite(v) => v,
        taffy::AvailableSpace::MinContent => 0.0,
        taffy::AvailableSpace::MaxContent => f32::INFINITY,
    })
}

/// The default [`Container`] for a bare container [`WidgetKind`] with no registered
/// [`ContainerStyle`]. Content kinds never reach here (they are not containers).
fn default_container(kind: WidgetKind) -> Container {
    match kind {
        WidgetKind::Row => Container::Row,
        WidgetKind::Column => Container::Column,
        WidgetKind::Stack => Container::Stack,
        WidgetKind::Grid => Container::Grid,
        WidgetKind::Scroll => Container::Scroll,
        WidgetKind::Spacer => Container::Spacer,
        WidgetKind::Pad => Container::Pad(EdgeInsets::default()),
        // semantic containers (SOUL §8.1): a tab bar rows its tabs, a list
        // columns its items, a table columns its rows and a table row rows
        // its cells.
        WidgetKind::TabBar => Container::Row,
        WidgetKind::List => Container::Column,
        WidgetKind::Table => Container::Column,
        WidgetKind::TableRow => Container::Row,
        WidgetKind::DialogLayer => Container::Pad(EdgeInsets::default()),
        WidgetKind::Dialog => Container::Pad(EdgeInsets::default()),
        _ => Container::Column,
    }
}

/// Map a [`ContainerStyle`] onto a Taffy [`Style`](taffy::Style) (SOUL §8.1
/// container→Taffy mapping).
fn apply_container(s: &mut taffy::Style, style: &ContainerStyle) {
    let gap = taffy::Size {
        width: length(style.gap),
        height: length(style.gap),
    };
    match style.container {
        Container::Row => {
            s.display = taffy::Display::Flex;
            s.flex_direction = taffy::FlexDirection::Row;
            s.gap = gap;
            s.justify_content = Some(map_justify(style.justify));
            s.align_items = Some(map_align(style.align));
        }
        Container::Column => {
            s.display = taffy::Display::Flex;
            s.flex_direction = taffy::FlexDirection::Column;
            s.gap = gap;
            s.justify_content = Some(map_justify(style.justify));
            s.align_items = Some(map_align(style.align));
        }
        Container::Stack => {
            // Relative container; children are absolutely-positioned to fill it
            // (handled per-child in `build_style`).
            s.display = taffy::Display::Flex;
            s.align_items = Some(map_align(style.align));
        }
        Container::Grid => {
            s.display = taffy::Display::Grid;
            s.gap = gap;
            s.justify_content = Some(map_justify(style.justify));
            s.align_items = Some(map_align(style.align));
        }
        Container::Scroll => {
            s.display = taffy::Display::Flex;
            s.flex_direction = taffy::FlexDirection::Column;
            s.gap = gap;
            s.align_items = Some(map_align(style.align));
            // Vertical scroll viewport; content offset is applied downstream, not
            // by layout (geometry only, SOUL §8.1).
            s.overflow = taffy::geometry::Point {
                x: taffy::Overflow::Visible,
                y: taffy::Overflow::Scroll,
            };
        }
        Container::Pad(insets) => {
            // A single padded child: the child lives in the content box, offset by
            // the insets.
            s.display = taffy::Display::Flex;
            s.flex_direction = taffy::FlexDirection::Column;
            s.align_items = Some(taffy::AlignItems::STRETCH);
            s.padding = taffy::geometry::Rect {
                left: length(insets.left),
                right: length(insets.right),
                top: length(insets.top),
                bottom: length(insets.bottom),
            };
        }
        Container::Spacer => {
            // Flexible empty space: grows to fill the parent's main axis.
            s.flex_grow = 1.0;
            s.flex_shrink = 0.0;
            s.size = taffy::Size {
                width: length(0.0_f32),
                height: length(0.0_f32),
            };
        }
    }

    // Responsive flow (SOUL §8.1): overflowing children of a flex container wrap
    // onto additional lines instead of shrinking past their intrinsic size.
    // Meaningful for Row/Column; a no-op for the non-flex displays.
    if style.wrap {
        s.flex_wrap = taffy::FlexWrap::Wrap;
        // Pack the wrapped lines at the cross-axis start (separated only by `gap`),
        // like wrapped text. CSS's stretch default would instead distribute the
        // lines across a taller-than-content box — e.g. a viewport-filling card
        // row would smear its lines over the whole window height.
        s.align_content = Some(taffy::AlignContent::FLEX_START);
    }

    // `fill` first: 100% of the parent content box (the viewport at the root, so a
    // filled layout tracks the window across resizes — SOUL §8.1). Any definite
    // size below overrides it per axis.
    if style.fill {
        s.size = taffy::Size {
            width: percent(1.0_f32),
            height: percent(1.0_f32),
        };
    }

    if let Some(sz) = style.fixed_size {
        s.size = taffy::Size {
            width: length(sz.width),
            height: length(sz.height),
        };
    }
    // Per-axis definite sizes win over `fixed_size` on their axis; the other axis
    // stays content-sized — e.g. a wrapping row with a definite width derives its
    // height from however many lines the children flow into.
    if let Some(w) = style.width {
        s.size.width = length(w);
    }
    if let Some(h) = style.height {
        s.size.height = length(h);
    }
    if let Some(w) = style.min_width {
        s.min_size.width = length(w);
    }
    if let Some(h) = style.min_height {
        s.min_size.height = length(h);
    }

    // An anchored container floats out of flow (SOUL §8.1): absolutely positioned
    // at the given (left, top) inset within its parent's box, so siblings lay out
    // as if it were not there — the dropdown-popup geometry. (`build_style` skips
    // its Stack fill-the-box override for anchored children, so this survives.)
    if let Some(a) = style.anchor {
        s.position = taffy::Position::Absolute;
        s.inset = taffy::geometry::Rect {
            left: length(a.x),
            top: length(a.y),
            right: taffy::style_helpers::auto(),
            bottom: taffy::style_helpers::auto(),
        };
    }
}

/// Fold a node's [`FlexChild`] factors onto its Taffy style (SOUL §8.1): only the
/// set fields override, so the container/leaf base style keeps its defaults.
fn apply_flex(s: &mut taffy::Style, flex: &FlexChild) {
    if let Some(g) = flex.grow {
        s.flex_grow = g;
    }
    if let Some(sh) = flex.shrink {
        s.flex_shrink = sh;
    }
    if let Some(b) = flex.basis {
        s.flex_basis = length(b);
    }
    if let Some(w) = flex.min_width {
        s.min_size.width = length(w);
    }
    if let Some(h) = flex.min_height {
        s.min_size.height = length(h);
    }
    if let Some(w) = flex.max_width {
        s.max_size.width = length(w);
    }
    if let Some(h) = flex.max_height {
        s.max_size.height = length(h);
    }
}

/// Map main-axis distribution onto Taffy's flex-relative `JustifyContent`.
fn map_justify(j: Justify) -> taffy::JustifyContent {
    match j {
        Justify::Start => taffy::JustifyContent::FLEX_START,
        Justify::Center => taffy::JustifyContent::CENTER,
        Justify::End => taffy::JustifyContent::FLEX_END,
        Justify::SpaceBetween => taffy::JustifyContent::SPACE_BETWEEN,
        Justify::SpaceAround => taffy::JustifyContent::SPACE_AROUND,
        Justify::SpaceEvenly => taffy::JustifyContent::SPACE_EVENLY,
    }
}

/// Map cross-axis alignment onto Taffy's flex-relative `AlignItems`.
fn map_align(a: Align) -> taffy::AlignItems {
    match a {
        Align::Start => taffy::AlignItems::FLEX_START,
        Align::Center => taffy::AlignItems::CENTER,
        Align::End => taffy::AlignItems::FLEX_END,
        Align::Stretch => taffy::AlignItems::STRETCH,
    }
}
