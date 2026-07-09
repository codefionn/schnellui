use crate::types::*;
use schnellui_signal::NodeId;
use slotmap::{SecondaryMap, SlotMap};
use smallvec::SmallVec;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCENE_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// Opaque identity for renderer caches derived from one [`Scene`].
///
/// The revision alone is not enough because each fresh scene starts its counter
/// at the same value. A renderer can outlive a whole-app remount, so the scene
/// identity must participate in every retained-cache comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneRenderKey {
    identity: u64,
    revision: u64,
}

pub struct Scene {
    render_identity: u64,
    tree: SlotMap<WidgetId, WidgetNode>,
    root: Option<WidgetId>,

    // --- ECS columns (§4.4, §8.1) ---
    layout: SecondaryMap<WidgetId, LayoutBox>,
    paint: SecondaryMap<WidgetId, PaintData>,
    a11y: SecondaryMap<WidgetId, A11yData>,
    dirty: SecondaryMap<WidgetId, DirtyFlags>,
    /// signal edges: which reactive [`NodeId`]s a node's dynamic slots depend on
    /// (§3.3). Used to route a signal change to the one node it mutates.
    bindings: SecondaryMap<WidgetId, SmallVec<[NodeId; 2]>>,
    /// per-node scroll offset — the **v0 stand-in for the SOUL §3.2 property tree**
    /// (there is no transform column yet). Applied downstream by the renderer when it
    /// composites a `Scroll` node's children; a change re-composites the viewport but
    /// never relayouts (§3.2/§8.1). See [`Scene::set_scroll_offset`].
    scroll: SecondaryMap<WidgetId, Point>,
    /// nodes rooting an **overlay layer** (SOUL §3.2 z-order): the renderer defers
    /// a flagged node's whole subtree and draws it *after* the base pass, so
    /// floating UI (a dropdown's open option list) paints above content that comes
    /// later in tree order, and hit-testing checks it first. Structural like the
    /// tree itself — set at build. Sparse: an overlay-less app pays nothing.
    /// Overlay plane level. Higher levels paint above lower ones; peers at the
    /// same level use the mutable [`Scene::overlay_order`].
    overlay: SecondaryMap<WidgetId, u8>,
    /// Mutable order within an overlay level. Focusing/pressing a modeless window
    /// raises its overlay root without changing its shared plane.
    overlay_order: SecondaryMap<WidgetId, u64>,
    next_overlay_order: u64,
    /// Roots hidden by responsive queries. Sparse and inherited by descendants:
    /// downstream tree walks stop at a hidden root, matching CSS `display: none`.
    hidden: SecondaryMap<WidgetId, ()>,
    /// Application-created component handles resolved to this mount's WidgetIds.
    component_refs: HashMap<ComponentRef, WidgetId>,
    component_refs_by_node: SecondaryMap<WidgetId, ComponentRef>,
    /// the shared RGBA image atlas (SOUL §3.2) — retained scene *resources*, owned
    /// here so [`Primitive::ImageQuad`]s resolve against `&Scene` alone. Starts
    /// empty; an imageless app pays zero bytes.
    images: ImageAtlas,

    // --- damage bookkeeping (§3.2) ---
    /// accumulated paint damage this frame (union of dirty paint rects).
    damage: Rect,
    /// nodes with a pending a11y change, drained into an incremental `TreeUpdate`.
    a11y_dirty: Vec<WidgetId>,
    /// nodes whose layout is dirty, roots of the smallest affected subtrees.
    layout_dirty: Vec<WidgetId>,
    /// every node whose flags went from clean to dirty this frame — the set
    /// [`Scene::clear_dirty`] drains so a clean frame is O(changed), never O(nodes)
    /// (SOUL §3.2/§8.1, Directive #3). The permanent per-node `dirty` flag column
    /// stays (it answers `dirty_flags` in O(1)); this list holds only the ids actually
    /// touched. Grow-only + `drain()` retains capacity ⇒ zero-alloc steady state (§4.4).
    dirtied: Vec<WidgetId>,
    /// Nodes with visual mutations, in mutation order. The renderer consumes this
    /// sparse list to update retained GPU fragments without scanning the tree.
    paint_dirty: Vec<WidgetId>,
    /// Changes that can invalidate cached renderer traversal or absolute geometry.
    /// Paint-only updates deliberately do not bump this counter.
    render_revision: u64,
}

/// A stack-free pre-order walk over one retained scene.
///
/// The iterator keeps an inline stack of ancestor cursors. It is linear in the
/// number of visited nodes, does not recurse, and avoids heap allocation for the
/// common case of trees no deeper than 32 nodes.
pub struct Preorder<'a> {
    scene: &'a Scene,
    next: Option<WidgetId>,
    ancestors: SmallVec<[(WidgetId, usize); 32]>,
}

impl Iterator for Preorder<'_> {
    type Item = WidgetId;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;
        if let Some(&first_child) = self.scene.node(current)?.children.first() {
            self.ancestors.push((current, 1));
            self.next = Some(first_child);
            return Some(current);
        }

        self.next = None;
        while let Some((parent, next_child)) = self.ancestors.last_mut() {
            let Some(children) = self.scene.node(*parent).map(|node| &node.children) else {
                self.ancestors.pop();
                continue;
            };
            if let Some(&sibling) = children.get(*next_child) {
                *next_child += 1;
                self.next = Some(sibling);
                break;
            }
            self.ancestors.pop();
        }
        Some(current)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.scene.len()))
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    /// An empty scene.
    pub fn new() -> Scene {
        let render_identity = NEXT_SCENE_IDENTITY.fetch_add(1, Ordering::Relaxed);
        assert_ne!(
            render_identity, 0,
            "exhausted the process-wide scene identity space"
        );
        Scene {
            render_identity,
            tree: SlotMap::with_key(),
            root: None,
            layout: SecondaryMap::new(),
            paint: SecondaryMap::new(),
            a11y: SecondaryMap::new(),
            dirty: SecondaryMap::new(),
            bindings: SecondaryMap::new(),
            scroll: SecondaryMap::new(),
            overlay: SecondaryMap::new(),
            overlay_order: SecondaryMap::new(),
            next_overlay_order: 1,
            hidden: SecondaryMap::new(),
            component_refs: HashMap::new(),
            component_refs_by_node: SecondaryMap::new(),
            images: ImageAtlas::new_empty(),
            damage: Rect::ZERO,
            a11y_dirty: Vec::new(),
            layout_dirty: Vec::new(),
            dirtied: Vec::new(),
            paint_dirty: Vec::new(),
            render_revision: 1,
        }
    }

    /// The root node, if mounted.
    pub fn root(&self) -> Option<WidgetId> {
        self.root
    }

    /// Visits the mounted tree in parent-before-children order.
    ///
    /// This is the canonical whole-tree traversal seam for consumers that need
    /// structural inspection (accessibility, remount restoration, diagnostics).
    /// It is linear-time and stack-safe, with the traversal stack stored inline
    /// for ordinary UI depths. Hot mutation paths should still use the scene's
    /// dirty channels instead of walking the tree.
    pub fn preorder(&self) -> Preorder<'_> {
        Preorder {
            scene: self,
            next: self.root,
            ancestors: SmallVec::new(),
        }
    }

    /// The shared RGBA image atlas (SOUL §3.2) — read by the renderer.
    pub fn images(&self) -> &ImageAtlas {
        &self.images
    }

    /// Mutable image atlas access — widgets insert their pixels here at build
    /// (a grow event, §4).
    pub fn images_mut(&mut self) -> &mut ImageAtlas {
        &mut self.images
    }

    /// Sets the root node (mount-time, §7).
    pub fn set_root(&mut self, id: WidgetId) {
        if self.root != Some(id) {
            self.root = Some(id);
            self.render_revision = self.render_revision.wrapping_add(1);
        }
    }

    /// Inserts a node with the given kind, initializing empty columns for it.
    /// Mount/first-frame may allocate (SOUL §4).
    pub fn insert(&mut self, kind: WidgetKind, parent: Option<WidgetId>) -> WidgetId {
        let id = self.tree.insert(WidgetNode {
            kind,
            parent,
            children: SmallVec::new(),
        });
        self.dirty.insert(id, DirtyFlags::NONE);
        if let Some(p) = parent {
            if let Some(pn) = self.tree.get_mut(p) {
                pn.children.push(id);
            }
        }
        self.render_revision = self.render_revision.wrapping_add(1);
        id
    }

    /// Removes a node (and detaches it from its parent). Its columns drop with it.
    pub fn remove(&mut self, id: WidgetId) {
        if let Some(node) = self.tree.get(id) {
            if let Some(p) = node.parent {
                if let Some(pn) = self.tree.get_mut(p) {
                    pn.children.retain(|c| *c != id);
                }
            }
        }
        self.tree.remove(id);
        self.layout.remove(id);
        self.paint.remove(id);
        self.a11y.remove(id);
        self.dirty.remove(id);
        self.bindings.remove(id);
        self.scroll.remove(id);
        self.overlay.remove(id);
        self.overlay_order.remove(id);
        self.hidden.remove(id);
        if let Some(reference) = self.component_refs_by_node.remove(id) {
            self.component_refs.remove(&reference);
        }
        self.paint_dirty.retain(|candidate| *candidate != id);
        self.render_revision = self.render_revision.wrapping_add(1);
    }

    /// Removes `root` and all of its descendants as one structural operation.
    ///
    /// The returned ids are suitable for purging parallel runtime/layout columns.
    /// Damage and dirty queues are reconciled here, so no stale id can leak into
    /// the next incremental frame.
    pub fn remove_subtree(&mut self, root: WidgetId) -> Option<RemovedSubtree> {
        let node = self.tree.get(root)?;
        let parent = node.parent;
        let child_index = parent
            .and_then(|parent| self.tree.get(parent))
            .and_then(|parent| parent.children.iter().position(|child| *child == root))
            .unwrap_or(0);

        let nodes = self.subtree_nodes(root);

        for &id in &nodes {
            if let Some(layout) = self.layout.get(id) {
                self.damage = self.damage.union(&layout.rect);
            }
        }
        for &id in nodes.iter().rev() {
            self.remove(id);
        }
        if self.root == Some(root) {
            self.root = None;
        }
        self.a11y_dirty.retain(|id| self.tree.contains_key(*id));
        self.layout_dirty.retain(|id| self.tree.contains_key(*id));
        self.dirtied.retain(|id| self.tree.contains_key(*id));

        Some(RemovedSubtree {
            parent,
            child_index,
            nodes,
        })
    }

    /// Collects one live branch in stable parent-before-children order.
    pub fn subtree_nodes(&self, root: WidgetId) -> Vec<WidgetId> {
        let mut nodes = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(node) = self.tree.get(id) else {
                continue;
            };
            nodes.push(id);
            stack.extend(node.children.iter().rev().copied());
        }
        nodes
    }

    /// Moves an already-attached child to `index` within its parent's child list.
    /// Used after a replacement view builds (and therefore appends) its new root.
    pub fn move_child_to_index(&mut self, parent: WidgetId, child: WidgetId, index: usize) {
        let Some(parent) = self.tree.get_mut(parent) else {
            return;
        };
        let Some(current) = parent
            .children
            .iter()
            .position(|candidate| *candidate == child)
        else {
            return;
        };
        let child = parent.children.remove(current);
        parent
            .children
            .insert(index.min(parent.children.len()), child);
        self.render_revision = self.render_revision.wrapping_add(1);
    }

    /// Number of live nodes.
    pub fn len(&self) -> usize {
        self.tree.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Immutable node access.
    pub fn node(&self, id: WidgetId) -> Option<&WidgetNode> {
        self.tree.get(id)
    }

    /// Attaches an application-created reference to one mounted component.
    ///
    /// A ref may target only one live component per scene; duplicate attachment
    /// is almost always an accidental copy/paste error and is rejected eagerly.
    pub fn set_component_ref(&mut self, id: WidgetId, reference: ComponentRef) {
        assert!(
            self.node(id).is_some(),
            "cannot attach a ComponentRef to a missing widget"
        );
        if let Some(existing) = self.component_refs.get(&reference).copied() {
            assert_eq!(
                existing, id,
                "the same ComponentRef cannot target two live components"
            );
            return;
        }
        if let Some(previous) = self.component_refs_by_node.insert(id, reference) {
            self.component_refs.remove(&previous);
        }
        self.component_refs.insert(reference, id);
    }

    /// Resolves a stable component reference to this scene mount's [`WidgetId`].
    pub fn resolve_ref(&self, reference: ComponentRef) -> Option<WidgetId> {
        self.component_refs
            .get(&reference)
            .copied()
            .filter(|id| self.node(*id).is_some())
    }

    /// Returns the application-created stable reference attached to `id`, if any.
    ///
    /// This is the inverse of [`Scene::resolve_ref`]. Hosts use it while replacing
    /// a mounted tree so a component explicitly keyed by the application can be
    /// paired with its counterpart without relying on build order or labels.
    pub fn component_ref(&self, id: WidgetId) -> Option<ComponentRef> {
        self.component_refs_by_node.get(id).copied()
    }

    // --- column accessors ---
    pub fn layout(&self, id: WidgetId) -> Option<&LayoutBox> {
        self.layout.get(id)
    }
    pub fn layout_mut(&mut self, id: WidgetId) -> Option<&mut LayoutBox> {
        self.layout.get_mut(id)
    }
    pub fn set_layout(&mut self, id: WidgetId, b: LayoutBox) {
        if self.layout.get(id).copied() != Some(b) {
            self.layout.insert(id, b);
            self.render_revision = self.render_revision.wrapping_add(1);
        }
    }
    pub fn paint(&self, id: WidgetId) -> Option<&PaintData> {
        self.paint.get(id)
    }
    pub fn paint_mut(&mut self, id: WidgetId) -> &mut PaintData {
        self.paint.entry(id).unwrap().or_default()
    }

    /// Monotonic revision for renderer traversal/geometry caches. It changes on
    /// tree and layout mutations, never on an ordinary paint-only update.
    pub fn render_revision(&self) -> u64 {
        self.render_revision
    }

    /// Identity and revision for caches that can survive a whole-scene remount.
    pub fn render_key(&self) -> SceneRenderKey {
        SceneRenderKey {
            identity: self.render_identity,
            revision: self.render_revision,
        }
    }
    pub fn a11y(&self, id: WidgetId) -> Option<&A11yData> {
        self.a11y.get(id)
    }
    pub fn a11y_mut(&mut self, id: WidgetId) -> &mut A11yData {
        self.a11y.entry(id).unwrap().or_default()
    }
    pub fn bindings(&self, id: WidgetId) -> Option<&SmallVec<[NodeId; 2]>> {
        self.bindings.get(id)
    }

    /// Sets direct visibility for a responsive subtree root. Hidden descendants
    /// remain retained but are excluded from layout participation, painting,
    /// hit-testing, and accessibility walks by their respective layers.
    pub fn set_visible(&mut self, id: WidgetId, visible: bool) {
        let was_visible = !self.hidden.contains_key(id);
        if was_visible == visible {
            return;
        }

        // Erase every previously-painted descendant, including content that may
        // overflow the root's own box. Becoming visible damages the same boxes so
        // the next presentation draws the restored subtree.
        fn subtree_damage(scene: &Scene, id: WidgetId) -> Rect {
            let mut damage = scene.layout(id).map(|b| b.rect).unwrap_or(Rect::ZERO);
            if let Some(node) = scene.node(id) {
                for &child in &node.children {
                    damage = damage.union(&subtree_damage(scene, child));
                }
            }
            damage
        }
        self.damage = self.damage.union(&subtree_damage(self, id));

        if visible {
            self.hidden.remove(id);
        } else {
            self.hidden.insert(id, ());
        }
        // Visibility changes whether the renderer walks this subtree. A hidden
        // terminal cannot reuse the instance range captured while it was visible.
        self.render_revision = self.render_revision.wrapping_add(1);
        self.mark_dirty(id, DirtyFlags::PAINT);
        self.mark_dirty(id, DirtyFlags::A11Y);
    }

    /// Whether this node itself is not a responsive hidden root.
    pub fn is_visible(&self, id: WidgetId) -> bool {
        self.node(id).is_some() && !self.hidden.contains_key(id)
    }

    /// Whether this node and every ancestor are visible.
    pub fn is_effectively_visible(&self, mut id: WidgetId) -> bool {
        loop {
            if !self.is_visible(id) {
                return false;
            }
            match self.node(id).and_then(|node| node.parent) {
                Some(parent) => id = parent,
                None => return true,
            }
        }
    }
    /// Records that node `id` depends on reactive `signal` (§3.3), so a change to
    /// that signal routes to this one node.
    pub fn bind(&mut self, id: WidgetId, signal: NodeId) {
        let list = self.bindings.entry(id).unwrap().or_default();
        if !list.contains(&signal) {
            list.push(signal);
        }
    }

    // --- property mutation API (SOUL §3.2, §8.1) -----------------------------
    //
    // Each setter is the scene-side landing point for a dynamic slot's effect
    // (§3.3): it mutates ONE property on ONE retained node and flags ONLY the
    // dirty channels that property actually touches — the whole point of the
    // three-channel split (§8.1). A visual-only change never triggers a relayout;
    // a semantic-only change never repaints; the frame walks each channel over its
    // dirty subtree alone. Every setter compares before writing so an
    // idempotent write stays clean (no spurious damage). Steady-state writes
    // mutate columns in place (grow-only `Vec`s, cleared-and-refilled) and
    // allocate nothing (§4).

    /// Overwrites the fill color of every primitive on a node (both `SolidRect`
    /// and `GlyphQuad`). A pure visual change → **paint-dirty only**, never
    /// layout (SOUL §8.1). No-op (and no damage) if the color is unchanged or the
    /// node has no paint column yet.
    pub fn set_color(&mut self, id: WidgetId, color: Color) {
        let mut changed = false;
        if let Some(pd) = self.paint.get_mut(id) {
            for p in pd.primitives.iter_mut() {
                let slot = match p {
                    Primitive::SolidRect { color: c, .. } => c,
                    Primitive::GlyphQuad { color: c, .. } => c,
                    Primitive::Line { color: c, .. } => c,
                    // an image recolors through its tint (WHITE = as-authored).
                    Primitive::ImageQuad { tint: c, .. } => c,
                };
                if *slot != color {
                    *slot = color;
                    changed = true;
                }
            }
        }
        if changed {
            self.mark_dirty(id, DirtyFlags::PAINT);
        }
    }

    /// Overwrites the corner radius of every `SolidRect` primitive on a node.
    /// Visual only → **paint-dirty only**.
    pub fn set_corner_radius(&mut self, id: WidgetId, radius: f32) {
        let mut changed = false;
        if let Some(pd) = self.paint.get_mut(id) {
            for p in pd.primitives.iter_mut() {
                if let Primitive::SolidRect { corner_radius, .. } = p {
                    if *corner_radius != radius {
                        *corner_radius = radius;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.mark_dirty(id, DirtyFlags::PAINT);
        }
    }

    /// Sets a node's final window-space rect (its geometry column). A move must
    /// both erase the old pixels and paint the new location, so this damages the
    /// **union of the old and new rect** and marks **paint-dirty**. It does *not*
    /// mark layout-dirty: this *is* the layout result being written down, not a
    /// request to relayout (SOUL §3.2/§8.1). No-op if unchanged.
    pub fn set_rect(&mut self, id: WidgetId, rect: Rect) {
        let old = self.layout.get(id).map(|b| b.rect).unwrap_or(Rect::ZERO);
        if old == rect {
            return;
        }
        {
            let b = self.layout.entry(id).unwrap().or_default();
            b.rect = rect;
        }
        // Damage the old rect explicitly; `mark_dirty(PAINT)` folds in the new one.
        self.damage = self.damage.union(&old);
        self.mark_dirty(id, DirtyFlags::PAINT);
    }

    /// Sets a `Scroll` node's scroll offset — the **v0 stand-in for the SOUL §3.2
    /// property tree** (the scene has no transform column yet). The renderer applies
    /// this offset to the node's descendants when it composites the viewport, so a
    /// scroll **re-composites but never relayouts** (SOUL §3.2/§8.1): this marks
    /// **paint-dirty only** and *never* layout-dirty. `mark_dirty(PAINT)` folds the
    /// node's own laid-out rect into the frame damage — the whole viewport is the
    /// damage, since every descendant may have moved. No-op (and no damage) when the
    /// offset is unchanged, so an idempotent set stays clean.
    ///
    /// The first set for a node may allocate its map slot (a grow event, SOUL §4);
    /// steady-state offset updates mutate the existing slot in place and allocate
    /// nothing.
    pub fn set_scroll_offset(&mut self, id: WidgetId, offset: Point) {
        if self.scroll.get(id).copied().unwrap_or_default() == offset {
            return;
        }
        self.scroll.insert(id, offset);
        self.mark_dirty(id, DirtyFlags::PAINT);
    }

    /// A node's scroll offset (SOUL §3.2). Defaults to the zero [`Point`] for a node
    /// that has never been scrolled.
    pub fn scroll_offset(&self, id: WidgetId) -> Point {
        self.scroll.get(id).copied().unwrap_or_default()
    }

    /// Flags `id` as an **overlay-layer root** (SOUL §3.2 z-order): the renderer
    /// draws its whole subtree after the base pass — above content later in tree
    /// order — and hit-testing checks it first. Structural (like the tree shape),
    /// so it is set at build and never toggled at runtime; a floating layer
    /// appearing or disappearing is a remount (SOUL §3.3).
    pub fn set_overlay(&mut self, id: WidgetId) {
        self.set_overlay_level(id, 0);
    }

    /// Flags `id` as an overlay at an explicit stacking level. This lets modal
    /// dialog layers remain above modeless peers regardless of declaration order.
    pub fn set_overlay_level(&mut self, id: WidgetId, level: u8) {
        self.overlay.insert(id, level);
        if !self.overlay_order.contains_key(id) {
            self.overlay_order.insert(id, self.next_overlay_order);
            self.next_overlay_order = self.next_overlay_order.saturating_add(1);
        }
    }

    /// `true` if `id` roots an overlay subtree (SOUL §3.2).
    pub fn is_overlay(&self, id: WidgetId) -> bool {
        self.overlay.contains_key(id)
    }

    /// The explicit overlay stacking level, or zero for base/default overlays.
    pub fn overlay_level(&self, id: WidgetId) -> u8 {
        self.overlay.get(id).copied().unwrap_or(0)
    }

    /// Monotonic within-level stacking order. Higher values paint and hit-test
    /// above lower values.
    pub fn overlay_order(&self, id: WidgetId) -> u64 {
        self.overlay_order.get(id).copied().unwrap_or(0)
    }

    /// Raises an existing overlay within its current level.
    pub fn bring_overlay_to_front(&mut self, id: WidgetId) -> bool {
        if !self.is_overlay(id) {
            return false;
        }
        self.overlay_order.insert(id, self.next_overlay_order);
        self.next_overlay_order = self.next_overlay_order.saturating_add(1);
        self.mark_dirty(id, DirtyFlags::PAINT);
        true
    }

    /// Replaces a node's paint fragments in place — `clear()`-then-`extend()` on
    /// the grow-only primitive `Vec` (§4.4), so re-emitting the same number of
    /// primitives reallocates nothing. Marks **paint-dirty**.
    pub fn replace_primitives<I>(&mut self, id: WidgetId, prims: I)
    where
        I: IntoIterator<Item = Primitive>,
    {
        {
            let pd = self.paint.entry(id).unwrap().or_default();
            pd.primitives.clear();
            pd.primitives.extend(prims);
        }
        self.mark_dirty(id, DirtyFlags::PAINT);
    }

    /// Sets the accessible **name** of a node (SOUL §6.1). Semantic only →
    /// **a11y-dirty only**; the pixels are untouched. No-op if unchanged.
    pub fn set_a11y_name(&mut self, id: WidgetId, name: Option<String>) {
        let changed = {
            let a = self.a11y.entry(id).unwrap().or_default();
            if a.name == name {
                false
            } else {
                a.name = name;
                true
            }
        };
        if changed {
            self.mark_dirty(id, DirtyFlags::A11Y);
        }
    }

    /// Sets the accessible **value** of a node (checked-text, slider now, input
    /// contents, …). Semantic only → **a11y-dirty only**. No-op if unchanged.
    pub fn set_a11y_value(&mut self, id: WidgetId, value: Option<String>) {
        let changed = {
            let a = self.a11y.entry(id).unwrap().or_default();
            if a.value == value {
                false
            } else {
                a.value = value;
                true
            }
        };
        if changed {
            self.mark_dirty(id, DirtyFlags::A11Y);
        }
    }

    /// Rewrites an accessible integer value in its retained string buffer.
    ///
    /// Scroll offsets are updated for every wheel event, so callers that create
    /// the value with room for an [`i64`] can use this path to avoid formatting a
    /// fresh `String` for each event. The first use on a node without a value is a
    /// normal grow event; steady-state callers retain the buffer in place.
    pub fn set_a11y_value_i64(&mut self, id: WidgetId, value: i64) {
        let changed = {
            let a = self.a11y.entry(id).unwrap().or_default();
            match a.value.as_mut() {
                Some(text) => {
                    // Keep the semantic dirty set incremental too: a fractional
                    // scroll update can retain the same rounded integer value.
                    if text.parse::<i64>().ok() == Some(value) {
                        return;
                    }
                    text.clear();
                    // `String` implements `fmt::Write`; a pre-capacitated scroll
                    // value (20 bytes for any i64) formats without heap traffic.
                    write!(text, "{value}").expect("writing to String cannot fail");
                    true
                }
                None => {
                    a.value = Some(value.to_string());
                    true
                }
            }
        };
        if changed {
            self.mark_dirty(id, DirtyFlags::A11Y);
        }
    }

    /// Overwrites the packed **state** bits (checked/disabled/expanded/selected/
    /// focused). Semantic only → **a11y-dirty only**. No-op if unchanged.
    pub fn set_a11y_state(&mut self, id: WidgetId, state: u32) {
        let changed = {
            let a = self.a11y.entry(id).unwrap().or_default();
            if a.state == state {
                false
            } else {
                a.state = state;
                true
            }
        };
        if changed {
            self.mark_dirty(id, DirtyFlags::A11Y);
        }
    }

    /// Sets a text node's content. The visible glyphs change *and* the accessible
    /// value changes, but — per the §8.1 label-text case — the box does **not**:
    /// this marks **paint-dirty + a11y-dirty**, never layout-dirty. If the
    /// measured size actually changed, the caller (which owns text metrics)
    /// additionally calls [`Scene::mark_dirty`] with [`DirtyFlags::LAYOUT`]. The
    /// caller refills the glyph primitives via [`Scene::replace_primitives`];
    /// this updates the accessible value and flags the tile for re-raster.
    pub fn set_text_content(&mut self, id: WidgetId, text: impl Into<String>) {
        let text = text.into();
        let changed = {
            let a = self.a11y.entry(id).unwrap().or_default();
            if a.value.as_deref() == Some(text.as_str()) {
                false
            } else {
                a.value = Some(text);
                true
            }
        };
        if changed {
            self.mark_dirty(id, DirtyFlags::PAINT);
            self.mark_dirty(id, DirtyFlags::A11Y);
        }
    }

    // --- dirty channels (§8.1) ---

    /// The current dirty flags for a node.
    pub fn dirty_flags(&self, id: WidgetId) -> DirtyFlags {
        self.dirty.get(id).copied().unwrap_or(DirtyFlags::NONE)
    }

    /// Marks a channel dirty on a node and, for paint, folds its rect into the
    /// frame damage region (SOUL §3.2). Records a11y/layout dirty roots too.
    pub fn mark_dirty(&mut self, id: WidgetId, flags: DirtyFlags) {
        let entry = self.dirty.entry(id).unwrap().or_insert(DirtyFlags::NONE);
        let was = *entry;
        entry.insert(flags);
        // Record the clean→dirty transition exactly once, so clear_dirty drains only
        // this node instead of sweeping the whole flag column (SOUL §3.2, Directive #3).
        if was.is_empty() && !flags.is_empty() {
            self.dirtied.push(id);
        }
        if flags.contains(DirtyFlags::PAINT) {
            if !was.contains(DirtyFlags::PAINT) {
                self.paint_dirty.push(id);
            }
            if let Some(b) = self.layout.get(id) {
                self.damage = self.damage.union(&b.rect);
            }
        }
        if flags.contains(DirtyFlags::A11Y) && !was.contains(DirtyFlags::A11Y) {
            self.a11y_dirty.push(id);
        }
        if flags.contains(DirtyFlags::LAYOUT) && !was.contains(DirtyFlags::LAYOUT) {
            self.layout_dirty.push(id);
        }
    }

    /// The accumulated paint damage rect for this frame (§3.2). Used as scissor +
    /// partial present.
    pub fn damage(&self) -> Rect {
        self.damage
    }

    /// The set of nodes whose semantics changed this frame (§6.2).
    pub fn a11y_dirty(&self) -> &[WidgetId] {
        &self.a11y_dirty
    }

    /// The set of layout-dirty subtree roots this frame (§8.1).
    pub fn layout_dirty(&self) -> &[WidgetId] {
        &self.layout_dirty
    }

    /// Nodes whose pixels changed since the previous [`Scene::clear_dirty`].
    /// Renderers use this sparse list to update retained GPU fragments without
    /// scanning every node in the tree.
    pub fn paint_dirty(&self) -> &[WidgetId] {
        &self.paint_dirty
    }

    /// Clears all per-frame dirty/damage state after a frame is presented
    /// (SOUL §3.2). Retains column capacity — no free (§4.4).
    pub fn clear_dirty(&mut self) {
        // Drain only the nodes actually dirtied this frame (O(changed)), never the whole
        // flag column (O(nodes)) — the clean-frame path must not pay for retained
        // document size (SOUL §3.2/§8.1, Directive #3). A stale id (node removed
        // mid-frame) has no flag slot; skip it. `drain` keeps the Vec's capacity, so a
        // steady-state clear allocates nothing (§4.4).
        for id in self.dirtied.drain(..) {
            if let Some(f) = self.dirty.get_mut(id) {
                f.clear();
            }
        }
        self.damage = Rect::ZERO;
        self.a11y_dirty.clear();
        self.layout_dirty.clear();
        self.paint_dirty.clear();
    }
}
