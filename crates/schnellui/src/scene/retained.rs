use crate::scene::types::*;
use slotmap::{SecondaryMap, SlotMap};
use smallvec::SmallVec;
use std::collections::HashMap;
use std::fmt::Write;
use std::ops::Range;
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
    geometry: u64,
}

impl SceneRenderKey {
    /// Whether `other` names the same scene at the same structural revision, ignoring
    /// geometry writes. A renderer that tracks moved nodes through
    /// [`Scene::paint_dirty`] can keep its retained instances across such writes.
    pub fn same_structure(&self, other: &SceneRenderKey) -> bool {
        self.identity == other.identity && self.revision == other.revision
    }
}

/// `(active modal root, focused widget)`, cached by [`Scene::set_a11y_scope_cache`].
pub type A11yScope = (Option<WidgetId>, Option<WidgetId>);

/// State bits whose change invalidates the cached [`A11yScope`] (FOCUSED | MODAL).
/// Mirrors the bit values of the a11y crate's `StateFlags`.
const A11Y_SCOPE_BITS: u32 = (1 << 4) | (1 << 5);

/// State bits whose change invalidates the cached tab order (DISABLED | MODAL).
const TAB_ORDER_BITS: u32 = (1 << 1) | (1 << 5);

/// Children count from which a parent keeps a spatial index for hit testing.
const CHILD_INDEX_MIN: usize = 16;

/// Slack added to child-index queries so a float rounding difference between a
/// translated rect and a translated point can never drop a real candidate; the
/// caller re-tests every candidate exactly.
const CHILD_INDEX_SLACK: f32 = 0.5;

/// Subtree bounds that prune nothing. Used for a `Scroll` without a usable
/// viewport rect, whose descendants are then not clipped.
const UNBOUNDED: Rect = Rect {
    x: -1.0e18,
    y: -1.0e18,
    width: 2.0e18,
    height: 2.0e18,
};

/// Cached hit-test bounds of one subtree.
#[derive(Clone, Copy)]
struct BoundsEntry {
    rect: Rect,
    dirty: bool,
}

/// Children of one wide parent sorted by the top edge of their subtree bounds,
/// with the running maximum bottom edge, so a point query is a binary search plus
/// a short backward scan instead of a visit of every child.
struct ChildIndex {
    /// `(bounds, child position)` by ascending `bounds.y`.
    sorted: Vec<(Rect, u32)>,
    /// `max_bottom[i]` is the largest bottom edge among `sorted[..=i]`.
    max_bottom: Vec<f32>,
}

/// The retained scene: tree, ECS columns, and per-frame dirty bookkeeping.
///
/// # Renderer-owned damage
///
/// Paint changes are tracked by sparse dirty lists and epochs. The renderer's
/// tile dependency fingerprints compare old and new retained instances, so moved
/// and removed pixels invalidate their previous tiles without a scene-wide
/// damage accumulator.
///
/// # Geometry contract
///
/// A node's [`LayoutBox`] is written through [`Scene::set_layout`] (layout) or
/// [`Scene::set_rect`] (direct placement); both are the same operation. When the
/// box actually changes, the scene
///
/// - marks the node **paint-dirty** (not layout-dirty: this is a layout result, not
///   a request to relayout),
/// - appends the node to [`Scene::geometry_changed`], and
/// - advances the geometry epoch that is part of [`Scene::render_key`].
///
/// [`Scene::render_revision`] is reserved for **structural** changes (insert,
/// remove, reorder, root, visibility). A renderer that caches absolute geometry
/// compares the whole [`SceneRenderKey`], so a moved node still invalidates it.
/// Writing an identical box is a no-op. Consumers (for example the paint
/// repositioning pass) drain the changed list with [`Scene::clear_geometry_changed`]
/// after processing it; [`Scene::clear_dirty`] also clears it at frame end.
pub struct Scene {
    render_identity: u64,
    tree: SlotMap<WidgetId, WidgetNode>,
    root: Option<WidgetId>,

    // --- ECS columns (§4.4, §8.1) ---
    layout: SecondaryMap<WidgetId, LayoutBox>,
    paint: SecondaryMap<WidgetId, PaintData>,
    a11y: SecondaryMap<WidgetId, A11yData>,
    dirty: SecondaryMap<WidgetId, DirtyFlags>,
    /// Descendant translation for viewport property nodes. Scroll is resolved
    /// with inherited translation and clip state by rendering and hit testing.
    scroll: SecondaryMap<WidgetId, Point>,
    properties: SecondaryMap<WidgetId, CompositeProperties>,
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
    /// Dense list of overlay roots plus each root's position in it. A
    /// `SecondaryMap` iterates every slot up to the highest index ever inserted,
    /// so enumerating it costs O(scene size) once any overlay sits at a high slot.
    overlay_roots: Vec<WidgetId>,
    overlay_pos: SecondaryMap<WidgetId, u32>,
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

    // --- dirty bookkeeping (§3.2) ---
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
    /// Changed primitive ranges for nodes edited through [`Scene::paint_range_mut`].
    /// An entry is meaningful only while `active`; entries keep their `Vec`
    /// capacity across frames (SOUL §4.4).
    paint_ranges: SecondaryMap<WidgetId, PaintRanges>,
    /// Nodes whose [`LayoutBox`] changed since a consumer last cleared the list.
    /// Capacity is retained across frames (SOUL §4.4).
    geometry_changed: Vec<WidgetId>,
    /// Lazily computed `(active modal root, focused widget)` pair for the a11y
    /// layer; `None` when stale. Dropped by every mutation that can change either
    /// (tree shape, visibility, `a11y_mut`, MODAL/FOCUSED state bits).
    a11y_scope: std::cell::Cell<Option<A11yScope>>,
    /// Lazily built keyboard tab order; `None` when stale. Dropped by tree shape,
    /// visibility and `a11y_mut` changes and by DISABLED/MODAL state bits, but not
    /// by a focus move.
    tab_order: std::cell::RefCell<Option<Vec<WidgetId>>>,
    /// Bumped whenever any layout box changes. Never reset.
    geometry_epoch: u64,
    /// Bumped on every request for layout, including one on a node that is still
    /// flagged from an earlier request. Never reset; lets a consumer that already
    /// laid out tell whether anything asked again (see [`Scene::layout_epoch`]).
    layout_epoch: u64,
    /// Bumped on every paint mark. Unlike `paint_dirty` it is never cleared, so each
    /// renderer can compare it with the epoch it last drew.
    paint_epoch: u64,
    /// `paint_epoch` at the last [`Scene::clear_dirty`]: `paint_dirty` holds exactly
    /// the nodes painted after this epoch.
    dirty_base_epoch: u64,
    /// Structural changes that can invalidate cached renderer traversal: insert,
    /// remove, reorder, root and visibility. Paint-only updates and geometry writes
    /// deliberately do not bump this counter (see the geometry contract on [`Scene`]).
    render_revision: u64,
    /// Hit-test bounds of each node's subtree in layout space (see
    /// [`Scene::subtree_bounds`]). A missing entry counts as dirty; a dirty entry
    /// implies every ancestor's entry is dirty or missing too.
    bounds: std::cell::RefCell<SecondaryMap<WidgetId, BoundsEntry>>,
    /// Spatial indexes for wide parents, dropped whenever the parent's bounds are
    /// recomputed or its child list changes.
    child_index: std::cell::RefCell<SecondaryMap<WidgetId, ChildIndex>>,
    /// Elements touched by structural removal, for the linear-work tests.
    #[cfg(test)]
    removal_work: u64,
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
            scroll: SecondaryMap::new(),
            properties: SecondaryMap::new(),
            overlay: SecondaryMap::new(),
            overlay_order: SecondaryMap::new(),
            overlay_roots: Vec::new(),
            overlay_pos: SecondaryMap::new(),
            next_overlay_order: 1,
            hidden: SecondaryMap::new(),
            component_refs: HashMap::new(),
            component_refs_by_node: SecondaryMap::new(),
            images: ImageAtlas::new_empty(),
            a11y_dirty: Vec::new(),
            layout_dirty: Vec::new(),
            dirtied: Vec::new(),
            paint_dirty: Vec::new(),
            paint_ranges: SecondaryMap::new(),
            geometry_changed: Vec::new(),
            a11y_scope: std::cell::Cell::new(None),
            tab_order: std::cell::RefCell::new(None),
            geometry_epoch: 0,
            layout_epoch: 0,
            paint_epoch: 0,
            dirty_base_epoch: 0,
            render_revision: 1,
            bounds: std::cell::RefCell::new(SecondaryMap::new()),
            child_index: std::cell::RefCell::new(SecondaryMap::new()),
            #[cfg(test)]
            removal_work: 0,
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
        self.invalidate_a11y_caches();
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
        self.invalidate_a11y_caches();
        if let Some(p) = parent {
            if let Some(pn) = self.tree.get_mut(p) {
                pn.children.push(id);
            }
            self.mark_bounds_dirty(p);
        }
        self.render_revision = self.render_revision.wrapping_add(1);
        id
    }

    /// Elements touched by structural removal so far (test instrumentation).
    #[cfg(test)]
    pub(crate) fn removal_work(&self) -> u64 {
        self.removal_work
    }

    /// Drops one node's columns and tree slot. Does not touch its parent's child
    /// list or the dirty queues; the caller reconciles those once per operation.
    fn drop_node(&mut self, id: WidgetId) {
        self.tree.remove(id);
        self.bounds.get_mut().remove(id);
        self.child_index.get_mut().remove(id);
        self.layout.remove(id);
        self.paint.remove(id);
        self.a11y.remove(id);
        self.dirty.remove(id);
        self.scroll.remove(id);
        self.properties.remove(id);
        self.overlay.remove(id);
        if let Some(pos) = self.overlay_pos.remove(id) {
            let pos = pos as usize;
            self.overlay_roots.swap_remove(pos);
            if let Some(&moved) = self.overlay_roots.get(pos) {
                self.overlay_pos.insert(moved, pos as u32);
            }
        }
        self.overlay_order.remove(id);
        self.hidden.remove(id);
        if let Some(reference) = self.component_refs_by_node.remove(id) {
            self.component_refs.remove(&reference);
        }
    }

    /// Removes `root` and all of its descendants as one structural operation.
    ///
    /// The returned ids are suitable for purging parallel runtime/layout columns.
    /// Damage and dirty queues are reconciled here, so no stale id can leak into
    /// the next incremental frame. Work is linear in the subtree size, the root's
    /// sibling count and the length of the dirty queues.
    pub fn remove_subtree(&mut self, root: WidgetId) -> Option<RemovedSubtree> {
        let nodes = self.subtree_nodes(root);
        self.remove_subtree_nodes(root, nodes)
    }

    /// [`Scene::remove_subtree`] for a caller that already collected the branch
    /// with [`Scene::subtree_nodes`] (to purge its own columns first) and so does
    /// not want it walked twice. `nodes` must be that unmodified list.
    pub fn remove_subtree_nodes(
        &mut self,
        root: WidgetId,
        nodes: Vec<WidgetId>,
    ) -> Option<RemovedSubtree> {
        let node = self.tree.get(root)?;
        let parent = node.parent;
        let mut child_index = 0;

        self.invalidate_a11y_caches();
        // Only the subtree root is linked into a surviving parent; interior nodes
        // die with their parents, so they need no unlinking.
        if let Some(pn) = parent.and_then(|p| self.tree.get_mut(p)) {
            if let Some(index) = pn.children.iter().position(|child| *child == root) {
                child_index = index;
                pn.children.remove(index);
            }
            #[cfg(test)]
            {
                self.removal_work += pn.children.len() as u64 + 1;
            }
        }
        for &id in &nodes {
            self.drop_node(id);
        }
        if let Some(parent) = parent {
            self.mark_bounds_dirty(parent);
        }
        if self.root == Some(root) {
            self.root = None;
        }
        // One pass per queue, testing liveness instead of scanning the removed set.
        let tree = &self.tree;
        #[cfg(test)]
        {
            self.removal_work += (self.paint_dirty.len()
                + self.geometry_changed.len()
                + self.a11y_dirty.len()
                + self.layout_dirty.len()
                + self.dirtied.len()
                + nodes.len()) as u64;
        }
        self.paint_dirty.retain(|id| tree.contains_key(*id));
        self.geometry_changed.retain(|id| tree.contains_key(*id));
        self.a11y_dirty.retain(|id| tree.contains_key(*id));
        self.layout_dirty.retain(|id| tree.contains_key(*id));
        self.dirtied.retain(|id| tree.contains_key(*id));
        self.render_revision = self.render_revision.wrapping_add(1);

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
    /// A child already at its target position changes nothing, not even the
    /// render revision.
    pub fn move_child_to_index(&mut self, parent: WidgetId, child: WidgetId, index: usize) {
        let parent_id = parent;
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
        let target = index.min(parent.children.len() - 1);
        if target == current {
            return;
        }
        let child = parent.children.remove(current);
        parent.children.insert(target, child);
        self.mark_bounds_dirty(parent_id);
        // Tab order, modal stacking and focus lookup all follow sibling order.
        self.invalidate_a11y_caches();
        self.render_revision = self.render_revision.wrapping_add(1);
    }

    /// Rewrites `parent`'s child order to `order` in one pass.
    ///
    /// `order` lists the desired sequence of children. Ids that are not children
    /// of `parent` (and repeats) are ignored; children it leaves out keep their
    /// relative order after the listed ones. Returns `true` and bumps the render
    /// revision only if the order actually changed, so a caller can pass the full
    /// desired order every frame without paying for an unchanged one.
    pub fn set_child_order(&mut self, parent: WidgetId, order: &[WidgetId]) -> bool {
        let Some(node) = self.tree.get(parent) else {
            return false;
        };
        if node.children.as_slice() == order {
            return false;
        }
        let mut seen: std::collections::HashSet<WidgetId> =
            std::collections::HashSet::with_capacity(order.len());
        let mut next: SmallVec<[WidgetId; 4]> = SmallVec::with_capacity(node.children.len());
        for &id in order {
            let is_child = self.tree.get(id).is_some_and(|n| n.parent == Some(parent));
            if is_child && seen.insert(id) {
                next.push(id);
            }
        }
        for &id in &node.children {
            if !seen.contains(&id) {
                next.push(id);
            }
        }
        if next == node.children {
            return false;
        }
        if let Some(node) = self.tree.get_mut(parent) {
            node.children = next;
        }
        self.mark_bounds_dirty(parent);
        self.invalidate_a11y_caches();
        self.render_revision = self.render_revision.wrapping_add(1);
        true
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
    /// Writes a node's geometry (see the geometry contract on [`Scene`]). No-op if
    /// the box is unchanged.
    pub fn set_layout(&mut self, id: WidgetId, b: LayoutBox) {
        let old = self.layout.get(id).copied();
        if old == Some(b) {
            return;
        }
        self.layout.insert(id, b);
        self.mark_bounds_dirty(id);
        // Bounds are part of the node's AccessKit description, so a moved box
        // re-sends exactly that node instead of forcing a full tree.
        self.mark_dirty(id, DirtyFlags::A11Y);
        self.geometry_epoch = self.geometry_epoch.wrapping_add(1);
        if self.geometry_changed.last() != Some(&id) {
            self.geometry_changed.push(id);
        }
        self.mark_dirty(id, DirtyFlags::PAINT);
    }
    pub fn paint(&self, id: WidgetId) -> Option<&PaintData> {
        self.paint.get(id)
    }
    /// Mutable paint data for a node. Marks the node paint-dirty before handing out the reference, so an edit can never be missed by the
    /// renderer. Read first through [`Scene::paint`] and call this only when writing.
    pub fn paint_mut(&mut self, id: WidgetId) -> &mut PaintData {
        self.mark_dirty(id, DirtyFlags::PAINT);
        self.paint.entry(id).unwrap().or_default()
    }

    /// Mutable access to primitives `range` of a node, for an edit that keeps the
    /// primitive count. Marks the node paint-dirty and records `range`, so a
    /// renderer can re-encode just those primitives (see
    /// [`Scene::paint_dirty_ranges`]). If the node was already paint-dirty through
    /// any other path this frame, it stays whole-node dirty.
    ///
    /// # Panics
    /// When `range` is outside the node's primitive list.
    pub fn paint_range_mut(&mut self, id: WidgetId, range: Range<usize>) -> &mut [Primitive] {
        let whole = self.dirty_flags(id).contains(DirtyFlags::PAINT);
        let rec = range.start as u32..range.end as u32;
        match self.paint_ranges.get_mut(id) {
            Some(entry) if entry.active => {
                if !entry.whole {
                    entry.ranges.push(rec);
                }
            }
            Some(entry) => {
                entry.active = true;
                entry.whole = whole;
                entry.ranges.clear();
                if !whole {
                    entry.ranges.push(rec);
                }
            }
            None => {
                if let Some(slot) = self.paint_ranges.entry(id) {
                    slot.or_insert(PaintRanges {
                        active: true,
                        whole,
                        ranges: if whole { Vec::new() } else { vec![rec] },
                    });
                }
            }
        }
        self.mark_dirty_inner(id, DirtyFlags::PAINT);
        &mut self.paint.entry(id).unwrap().or_default().primitives[range]
    }

    /// The primitive ranges of `id` changed since the last [`Scene::clear_dirty`],
    /// or `None` when the node must be treated as changed as a whole (it was
    /// painted through [`Scene::paint_mut`], [`Scene::mark_dirty`] or any other
    /// writer, or not painted at all). Ranges may overlap and are in edit order.
    pub fn paint_dirty_ranges(&self, id: WidgetId) -> Option<&[Range<u32>]> {
        self.paint_ranges
            .get(id)
            .filter(|entry| entry.active && !entry.whole)
            .map(|entry| entry.ranges.as_slice())
    }

    /// Monotonic revision for renderer traversal caches. It changes on structural
    /// mutations only; geometry writes are tracked by [`Scene::geometry_changed`] and
    /// folded into [`Scene::render_key`].
    pub fn render_revision(&self) -> u64 {
        self.render_revision
    }

    /// Identity and revision for caches that can survive a whole-scene remount.
    pub fn render_key(&self) -> SceneRenderKey {
        SceneRenderKey {
            identity: self.render_identity,
            revision: self.render_revision,
            geometry: self.geometry_epoch,
        }
    }

    /// Nodes whose geometry changed since the list was last cleared, in write order
    /// (a node written twice in a row appears once). Layout uses this to tell the
    /// paint-repositioning pass exactly which nodes moved.
    pub fn geometry_changed(&self) -> &[WidgetId] {
        &self.geometry_changed
    }

    /// Empties [`Scene::geometry_changed`], keeping its capacity.
    pub fn clear_geometry_changed(&mut self) {
        self.geometry_changed.clear();
    }
    pub fn a11y(&self, id: WidgetId) -> Option<&A11yData> {
        self.a11y.get(id)
    }
    /// Mutable a11y data for a node. Marks the node a11y-dirty before handing out
    /// the reference. Read first through [`Scene::a11y`] and call this only when
    /// writing.
    pub fn a11y_mut(&mut self, id: WidgetId) -> &mut A11yData {
        self.invalidate_a11y_caches();
        self.mark_dirty(id, DirtyFlags::A11Y);
        self.a11y.entry(id).unwrap().or_default()
    }
    /// Mutable a11y data for a text-only write (name or value). Marks the node
    /// a11y-dirty but keeps the scope and tab-order caches, so the caller must
    /// not touch `role`, `state` or `actions` through it; use [`Scene::a11y_mut`]
    /// or [`Scene::set_a11y_state`] for those.
    pub fn a11y_text_mut(&mut self, id: WidgetId) -> &mut A11yData {
        self.mark_dirty(id, DirtyFlags::A11Y);
        self.a11y.entry(id).unwrap().or_default()
    }

    /// Sets direct visibility for a responsive subtree root. Hidden descendants
    /// remain retained but are excluded from layout participation, painting,
    /// hit-testing, and accessibility walks by their respective layers.
    pub fn set_visible(&mut self, id: WidgetId, visible: bool) {
        let was_visible = !self.hidden.contains_key(id);
        if was_visible == visible {
            return;
        }

        if visible {
            self.hidden.remove(id);
        } else {
            self.hidden.insert(id, ());
        }
        // Visibility changes whether the renderer walks this subtree. A hidden
        // terminal cannot reuse the instance range captured while it was visible.
        self.render_revision = self.render_revision.wrapping_add(1);
        self.invalidate_a11y_caches();
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
    // --- property mutation API (SOUL §3.2, §8.1) -----------------------------
    //
    // Each setter is the scene-side landing point for a dynamic slot's effect
    // (§3.3): it mutates ONE property on ONE retained node and flags ONLY the
    // dirty channels that property actually touches — the whole point of the
    // three-channel split (§8.1). A visual-only change never triggers a relayout;
    // a semantic-only change never repaints; the frame walks each channel over its
    // dirty subtree alone. Every setter compares before writing so an
    // idempotent write stays clean (no spurious paint dirt). Steady-state writes
    // mutate columns in place (grow-only `Vec`s, cleared-and-refilled) and
    // allocate nothing (§4).

    /// Overwrites the fill color of every primitive on a node (both `SolidRect`
    /// and `GlyphQuad`). A pure visual change → **paint-dirty only**, never
    /// layout (SOUL §8.1). No-op (and no paint dirt) if the color is unchanged or the
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

    /// Sets a node's final window-space rect, keeping its content rect. Same
    /// contract as [`Scene::set_layout`]: marks **paint-dirty** (never layout-dirty, SOUL §3.2/§8.1) and records the
    /// node in [`Scene::geometry_changed`]. No-op if unchanged.
    pub fn set_rect(&mut self, id: WidgetId, rect: Rect) {
        let mut b = self.layout.get(id).copied().unwrap_or_default();
        b.rect = rect;
        self.set_layout(id, b);
    }

    /// Sets a viewport's descendant translation without requesting layout or
    /// regenerating widget primitives. Rendering updates the visible instance run
    /// and invalidates changed raster tiles. Accessibility marks only the
    /// viewport dirty: it publishes the offset as its AccessKit transform, so
    /// descendant bounds (see [`Scene::accessibility_rect`]) do not change and a
    /// scroll step costs O(1) accessibility work. An identical offset is a no-op.
    /// The first write is a grow event; later updates retain column capacity.
    pub fn set_scroll_offset(&mut self, id: WidgetId, offset: Point) {
        if self.scroll.get(id).copied().unwrap_or_default() == offset {
            return;
        }
        self.scroll.insert(id, offset);
        self.mark_dirty(id, DirtyFlags::A11Y);
        self.mark_dirty(id, DirtyFlags::PAINT);
    }

    /// A node's scroll offset (SOUL §3.2). Defaults to the zero [`Point`] for a node
    /// that has never been scrolled.
    /// Changes inherited compositing properties without requesting layout or
    /// regenerating widget paint primitives. The renderer resolves the property
    /// tree through the retained parent links.
    pub fn set_composite_properties(&mut self, id: WidgetId, properties: CompositeProperties) {
        if self.composite_properties(id) == properties || !self.tree.contains_key(id) {
            return;
        }
        self.properties.insert(id, properties);
        self.mark_bounds_dirty(id);
        self.mark_composite_a11y(id);
        // Existing fragment contexts belong to the previous property tree.
        self.render_revision = self.render_revision.wrapping_add(1);
        self.mark_dirty(id, DirtyFlags::PAINT);
    }

    fn mark_composite_a11y(&mut self, id: WidgetId) {
        self.mark_dirty(id, DirtyFlags::A11Y);
        let count = self.tree.get(id).map_or(0, |node| node.children.len());
        for index in 0..count {
            let child = self.tree[id].children[index];
            self.mark_composite_a11y(child);
        }
    }

    pub fn composite_properties(&self, id: WidgetId) -> CompositeProperties {
        self.properties.get(id).copied().unwrap_or_default()
    }

    /// Accumulated translation for this node, including ancestor scroll offsets.
    /// A viewport's own scroll offset applies only to its descendants.
    pub fn composited_offset(&self, id: WidgetId) -> Point {
        self.composite_context(id).0
    }

    /// Accumulated translation and window-space clip for this property node.
    /// The clip includes ancestor viewport clips and this node's explicit clip.
    pub fn composite_context(&self, id: WidgetId) -> (Point, Rect) {
        let (mut offset, mut clip) = match self.tree.get(id).and_then(|node| node.parent) {
            Some(parent) => {
                let (mut offset, mut clip) = self.composite_context(parent);
                if self.tree[parent].kind == WidgetKind::Scroll {
                    if let Some(layout) = self.layout.get(parent).filter(|b| !b.rect.is_empty()) {
                        let mut viewport = layout.rect;
                        viewport.x += offset.x;
                        viewport.y += offset.y;
                        clip = clip.intersect(&viewport);
                    }
                    let scroll = self.scroll_offset(parent);
                    offset.x -= scroll.x;
                    offset.y -= scroll.y;
                }
                (offset, clip)
            }
            None => (Point::default(), UNBOUNDED),
        };
        let properties = self.composite_properties(id);
        offset.x += properties.translation.x;
        offset.y += properties.translation.y;
        if let Some(mut local) = properties.clip {
            local.x += offset.x;
            local.y += offset.y;
            clip = clip.intersect(&local);
        }
        (offset, clip)
    }

    /// Geometry in the accessibility coordinate space: like
    /// [`Scene::composited_rect`] but without scroll offsets and viewport clips.
    /// The accessibility tree expresses each viewport's offset as a transform on
    /// the viewport and clips its content with `clips_children`, so content
    /// bounds stay fixed while scrolling. Explicit property clips still apply
    /// within one viewport's content; crossing into a viewport starts a fresh
    /// clip, because ancestor clips live in the unscrolled space outside it.
    pub fn accessibility_rect(&self, id: WidgetId) -> Rect {
        let Some(layout) = self.layout.get(id) else {
            return Rect::ZERO;
        };
        let (offset, clip) = self.accessibility_context(id);
        let mut rect = layout.rect;
        rect.x += offset.x;
        rect.y += offset.y;
        rect.intersect(&clip)
    }

    fn accessibility_context(&self, id: WidgetId) -> (Point, Rect) {
        let (mut offset, mut clip) = match self.tree.get(id).and_then(|node| node.parent) {
            Some(parent) => {
                let (offset, clip) = self.accessibility_context(parent);
                if self.tree[parent].kind == WidgetKind::Scroll {
                    (offset, UNBOUNDED)
                } else {
                    (offset, clip)
                }
            }
            None => (Point::default(), UNBOUNDED),
        };
        let properties = self.composite_properties(id);
        offset.x += properties.translation.x;
        offset.y += properties.translation.y;
        if let Some(mut local) = properties.clip {
            local.x += offset.x;
            local.y += offset.y;
            clip = clip.intersect(&local);
        }
        (offset, clip)
    }

    /// Visible window-space geometry, shared by rendering and hit testing.
    pub fn composited_rect(&self, id: WidgetId) -> Rect {
        let Some(layout) = self.layout.get(id) else {
            return Rect::ZERO;
        };
        let (offset, clip) = self.composite_context(id);
        let mut rect = layout.rect;
        rect.x += offset.x;
        rect.y += offset.y;
        rect.intersect(&clip)
    }

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
        // A new overlay or a changed level reorders the draw passes without
        // touching any node's paint, so it is structural: retained renderers
        // rebuild and the hit/a11y caches are dropped.
        if self.overlay.insert(id, level) != Some(level) {
            self.render_revision = self.render_revision.wrapping_add(1);
            self.invalidate_a11y_caches();
        }
        if !self.overlay_pos.contains_key(id) {
            self.overlay_pos.insert(id, self.overlay_roots.len() as u32);
            self.overlay_roots.push(id);
        }
        if !self.overlay_order.contains_key(id) {
            self.overlay_order.insert(id, self.next_overlay_order);
            self.next_overlay_order = self.next_overlay_order.saturating_add(1);
        }
    }

    /// Every overlay root, in no particular order. Backed by a dense list, so the
    /// cost is the number of overlays, independent of scene size or slot history.
    pub fn overlays(&self) -> impl Iterator<Item = WidgetId> + '_ {
        self.overlay_roots.iter().copied()
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
        let flipped = {
            let a = self.a11y.entry(id).unwrap().or_default();
            let flipped = a.state ^ state;
            a.state = state;
            flipped
        };
        if flipped == 0 {
            return;
        }
        // Only focus, modality and disablement feed the cached a11y scope and
        // tab order.
        if flipped & A11Y_SCOPE_BITS != 0 {
            self.a11y_scope.set(None);
        }
        if flipped & TAB_ORDER_BITS != 0 {
            *self.tab_order.get_mut() = None;
        }
        self.mark_dirty(id, DirtyFlags::A11Y);
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
    /// paint-dirty list and epoch (SOUL §3.2). Records a11y/layout dirty roots too.
    pub fn mark_dirty(&mut self, id: WidgetId, flags: DirtyFlags) {
        if flags.contains(DirtyFlags::PAINT) {
            // An untracked paint write: renderers must re-encode the whole node.
            if let Some(entry) = self.paint_ranges.get_mut(id) {
                entry.whole = true;
            }
        }
        self.mark_dirty_inner(id, flags);
    }

    fn mark_dirty_inner(&mut self, id: WidgetId, flags: DirtyFlags) {
        let entry = self.dirty.entry(id).unwrap().or_insert(DirtyFlags::NONE);
        let was = *entry;
        entry.insert(flags);
        // Record the clean→dirty transition exactly once, so clear_dirty drains only
        // this node instead of sweeping the whole flag column (SOUL §3.2, Directive #3).
        if was.is_empty() && !flags.is_empty() {
            self.dirtied.push(id);
        }
        if flags.contains(DirtyFlags::PAINT) {
            self.paint_epoch += 1;
            if !was.contains(DirtyFlags::PAINT) {
                self.paint_dirty.push(id);
            }
        }
        if flags.contains(DirtyFlags::A11Y) && !was.contains(DirtyFlags::A11Y) {
            self.a11y_dirty.push(id);
        }
        if flags.contains(DirtyFlags::LAYOUT) {
            self.layout_epoch = self.layout_epoch.wrapping_add(1);
            if !was.contains(DirtyFlags::LAYOUT) {
                self.layout_dirty.push(id);
            }
        }
    }

    /// Marks `id` and its ancestors as needing a bounds recompute. Stops at the
    /// first existing dirty entry, since everything above it is already dirty.
    fn mark_bounds_dirty(&mut self, id: WidgetId) {
        let bounds = self.bounds.get_mut();
        let mut current = Some(id);
        while let Some(id) = current {
            match bounds.get_mut(id) {
                Some(entry) if entry.dirty => break,
                Some(entry) => entry.dirty = true,
                None => {}
            }
            current = self.tree.get(id).and_then(|node| node.parent);
        }
    }

    fn refresh_bounds(
        &self,
        bounds: &mut SecondaryMap<WidgetId, BoundsEntry>,
        index: &mut SecondaryMap<WidgetId, ChildIndex>,
        id: WidgetId,
    ) -> Rect {
        if let Some(entry) = bounds.get(id).filter(|entry| !entry.dirty) {
            return entry.rect;
        }
        let Some(node) = self.tree.get(id) else {
            return Rect::ZERO;
        };
        let own = self.layout.get(id).map(|layout| layout.rect);
        let mut all = own.unwrap_or(Rect::ZERO);
        for &child in &node.children {
            let mut child_bounds = self.refresh_bounds(bounds, index, child);
            let translation = self.composite_properties(child).translation;
            child_bounds.x += translation.x;
            child_bounds.y += translation.y;
            all = all.union(&child_bounds);
        }
        // A viewport clips every descendant to its own rect, so nothing below it
        // reaches outside. Without a usable rect there is no clip to rely on.
        let rect = if node.kind == WidgetKind::Scroll {
            own.filter(|rect| !rect.is_empty()).unwrap_or(UNBOUNDED)
        } else {
            all
        };
        let properties = self.composite_properties(id);
        let rect = properties.clip.map_or(rect, |clip| rect.intersect(&clip));
        bounds.insert(id, BoundsEntry { rect, dirty: false });
        index.remove(id);
        rect
    }

    /// The rect, in layout space (before any scroll offset is applied), outside of
    /// which no pointer hit can resolve into `id`'s subtree: the union of the
    /// subtree's rects, or just the viewport rect for a clipping `Scroll`. Hit
    /// testing prunes whole branches with it. Recomputed lazily and only along the
    /// ancestor chains of nodes whose geometry or structure changed.
    pub fn subtree_bounds(&self, id: WidgetId) -> Rect {
        let mut bounds = self.bounds.borrow_mut();
        let mut index = self.child_index.borrow_mut();
        self.refresh_bounds(&mut bounds, &mut index, id)
    }

    /// Collects into `out` the children of `parent` whose subtree bounds may
    /// contain `point` (layout space), top-most (last in child order) first.
    /// Returns `false` without touching `out` when `parent` has too few children to
    /// be worth indexing; the caller then walks the child list itself. The result is
    /// a superset filter: callers re-test each candidate exactly.
    pub fn child_hit_candidates(
        &self,
        parent: WidgetId,
        point: Point,
        out: &mut SmallVec<[WidgetId; 8]>,
    ) -> bool {
        let Some(node) = self.tree.get(parent) else {
            return false;
        };
        if node.children.len() < CHILD_INDEX_MIN {
            return false;
        }
        let mut bounds = self.bounds.borrow_mut();
        let mut indexes = self.child_index.borrow_mut();
        // Refreshing clears the stale index of every recomputed node, so an index
        // that is still present below is current.
        self.refresh_bounds(&mut bounds, &mut indexes, parent);
        if !indexes.contains_key(parent) {
            let mut sorted: Vec<(Rect, u32)> = node
                .children
                .iter()
                .enumerate()
                .filter_map(|(position, &child)| {
                    let mut rect = bounds.get(child)?.rect;
                    let translation = self.composite_properties(child).translation;
                    rect.x += translation.x;
                    rect.y += translation.y;
                    (!rect.is_empty()).then_some((rect, position as u32))
                })
                .collect();
            sorted.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
            let mut running = f32::NEG_INFINITY;
            let max_bottom = sorted
                .iter()
                .map(|(rect, _)| {
                    running = running.max(rect.bottom());
                    running
                })
                .collect();
            indexes.insert(parent, ChildIndex { sorted, max_bottom });
        }
        let index = &indexes[parent];
        let (x, y) = (point.x, point.y);
        let upper = index
            .sorted
            .partition_point(|(rect, _)| rect.y <= y + CHILD_INDEX_SLACK);
        let mut found: SmallVec<[u32; 8]> = SmallVec::new();
        for i in (0..upper).rev() {
            if index.max_bottom[i] < y - CHILD_INDEX_SLACK {
                break;
            }
            let (rect, position) = index.sorted[i];
            if rect.bottom() >= y - CHILD_INDEX_SLACK
                && rect.x <= x + CHILD_INDEX_SLACK
                && rect.right() >= x - CHILD_INDEX_SLACK
            {
                found.push(position);
            }
        }
        // The scan visits candidates by bounds order; paint order is child order.
        found.sort_unstable_by(|a, b| b.cmp(a));
        out.extend(
            found
                .iter()
                .map(|&position| node.children[position as usize]),
        );
        true
    }

    fn invalidate_a11y_caches(&mut self) {
        self.a11y_scope.set(None);
        *self.tab_order.get_mut() = None;
    }

    /// Runs `f` over the cached keyboard tab order, building it with `build` when
    /// stale. `build` must not call back into this method.
    pub fn with_tab_order_cache<R>(
        &self,
        build: impl FnOnce() -> Vec<WidgetId>,
        f: impl FnOnce(&[WidgetId]) -> R,
    ) -> R {
        let mut cache = self.tab_order.borrow_mut();
        let order = cache.get_or_insert_with(build);
        f(order)
    }

    /// The cached `(active modal root, focused widget)` pair, if still valid.
    pub fn a11y_scope_cache(&self) -> Option<A11yScope> {
        self.a11y_scope.get()
    }

    /// Stores the pair computed by the a11y layer. Valid until the next mutation
    /// that can change modality or focus.
    pub fn set_a11y_scope_cache(&self, scope: A11yScope) {
        self.a11y_scope.set(Some(scope));
    }

    /// The set of nodes whose semantics changed this frame (§6.2).
    pub fn a11y_dirty(&self) -> &[WidgetId] {
        &self.a11y_dirty
    }

    /// The set of layout-dirty subtree roots this frame (§8.1).
    pub fn layout_dirty(&self) -> &[WidgetId] {
        &self.layout_dirty
    }

    /// Count of layout requests. [`Scene::layout_dirty`] stays populated until
    /// [`Scene::clear_dirty`], so after a layout pass that list alone cannot say
    /// whether anything asked for layout again; a pass records this value when it
    /// runs and skips itself while it is unchanged.
    pub fn layout_epoch(&self) -> u64 {
        self.layout_epoch
    }

    /// Nodes whose pixels changed since the previous [`Scene::clear_dirty`].
    /// Renderers use this sparse list to update retained GPU fragments without
    /// scanning every node in the tree.
    pub fn paint_dirty(&self) -> &[WidgetId] {
        &self.paint_dirty
    }

    /// Monotonic count of paint marks. Never reset by [`Scene::clear_dirty`], so a
    /// renderer that remembers the value it last drew can tell whether anything was
    /// painted since, even when the frame pass already cleared the dirty lists.
    pub fn paint_epoch(&self) -> u64 {
        self.paint_epoch
    }

    /// The [`Scene::paint_epoch`] at the last [`Scene::clear_dirty`]. A renderer that
    /// drew at or after this epoch may patch just [`Scene::paint_dirty`]; an older
    /// one has missed nodes the cleared list no longer names.
    pub fn paint_dirty_base_epoch(&self) -> u64 {
        self.dirty_base_epoch
    }

    /// Clears all per-frame dirty state after a frame is presented
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
            if let Some(entry) = self.paint_ranges.get_mut(id) {
                entry.active = false;
                entry.ranges.clear();
            }
        }
        self.a11y_dirty.clear();
        self.layout_dirty.clear();
        self.paint_dirty.clear();
        self.geometry_changed.clear();
        self.dirty_base_epoch = self.paint_epoch;
    }
}

/// Per-node record behind [`Scene::paint_range_mut`] and
/// [`Scene::paint_dirty_ranges`].
#[derive(Clone, Debug, Default)]
struct PaintRanges {
    /// Set by the first ranged edit after a clear, reset by `clear_dirty`.
    active: bool,
    /// Some paint write this frame was not described by a range.
    whole: bool,
    ranges: Vec<Range<u32>>,
}
