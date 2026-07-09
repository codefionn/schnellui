//! Retained, variable-height virtual lists.
//!
//! A [`VirtualListController`] owns data identity and geometry; [`VirtualList`]
//! owns only the mounted shell.  The controller therefore stays useful outside a
//! view factory (streaming rows can be refreshed one key at a time), while the
//! mounted shell retains rows that overlap consecutive pixel windows.  The
//! implementation deliberately keeps the expensive pieces behind this small
//! interface: callers provide keys and a row factory, never spacers, ranges, or
//! layout feedback plumbing.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::rc::Rc;

use schnellui_a11y::Role;
use schnellui_layout::{Container, ContainerStyle, LayoutEngine};
use schnellui_scene::{DirtyFlags, Point, Scene, WidgetId, WidgetKind};
use schnellui_text::{GlyphAtlas, TextShaper};

use crate::{purge_nodes, scroll_metrics, AnyView, BuildCtx, Runtime, Scroll, View};

/// Pixels kept mounted above and below the visible viewport by default.
pub const DEFAULT_OVERSCAN: f32 = 320.0;

/// Cloneable, UI-thread handle for a virtualized sequence.
///
/// Keys are unique and define row identity.  `replace`, `insert`, `remove`, and
/// `refresh` only mark the controller; the next retained frame reconciles the
/// pixel window.  Rows whose key remains in that window retain their exact
/// [`WidgetId`] and all widget-local state.
pub struct VirtualListController<K> {
    inner: Rc<RefCell<Controller<K>>>,
}

impl<K> Clone for VirtualListController<K> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<K> VirtualListController<K>
where
    K: Clone + Eq + Hash + 'static,
{
    /// Creates a controller. Unknown rows use `estimated_height` until their
    /// retained row has completed one layout pass.
    pub fn new(keys: impl IntoIterator<Item = K>, estimated_height: f32) -> Self {
        let estimated_height = sane_height(estimated_height);
        let keys: Vec<_> = keys.into_iter().collect();
        assert_unique(&keys);
        Self {
            inner: Rc::new(RefCell::new(Controller::new(keys, estimated_height))),
        }
    }

    /// Replaces sequence order and membership. Existing keys keep their measured
    /// height; new keys start from the estimate.
    pub fn replace(&self, keys: impl IntoIterator<Item = K>) {
        let keys: Vec<_> = keys.into_iter().collect();
        assert_unique(&keys);
        self.inner.borrow_mut().replace(keys);
    }

    /// Inserts `key` at `index` (or at the end when the index is too large).
    pub fn insert(&self, index: usize, key: K) {
        self.inner.borrow_mut().insert(index, key);
    }

    /// Removes one key. Returns whether it was present.
    pub fn remove(&self, key: &K) -> bool {
        self.inner.borrow_mut().remove(key)
    }

    /// Requests a retained replacement of one currently mounted row. Offscreen
    /// rows are simply rebuilt when they next enter the pixel window.
    pub fn refresh(&self, key: &K) {
        self.inner.borrow_mut().refresh(key);
    }

    /// Requests retained refreshes for every mounted row.
    pub fn refresh_all(&self) {
        self.inner.borrow_mut().refresh_all();
    }

    /// Sets the retained overscan in logical pixels.
    pub fn overscan(&self, pixels: f32) {
        let mut inner = self.inner.borrow_mut();
        let pixels = pixels.max(0.0);
        if (inner.overscan - pixels).abs() > f32::EPSILON {
            inner.overscan = pixels;
            inner.dirty = true;
        }
    }

    /// Current estimated total height. This is O(1), even for long transcripts.
    pub fn estimated_height(&self) -> f32 {
        self.inner.borrow().heights.total()
    }

    /// Number of source rows, including rows that are currently offscreen.
    pub fn len(&self) -> usize {
        self.inner.borrow().keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn note_scroll(&self, offset: f32) {
        self.inner.borrow_mut().note_scroll(offset);
    }
}

/// A scroll viewport backed by a [`VirtualListController`].
///
/// `row` is called only for rows entering the retained pixel window, plus a
/// single bootstrap row before the first viewport layout. It may return any
/// regular SchnellUI view, including rich markdown cards.
pub struct VirtualList<K> {
    controller: VirtualListController<K>,
    row: Option<Box<dyn FnMut(&K) -> AnyView>>,
    scroll: Scroll,
}

impl<K> VirtualList<K>
where
    K: Clone + Eq + Hash + 'static,
{
    pub fn new<V>(
        controller: VirtualListController<K>,
        mut row: impl FnMut(&K) -> V + 'static,
    ) -> Self
    where
        V: View,
    {
        Self {
            controller,
            row: Some(Box::new(move |key| Box::new(row(key)))),
            scroll: Scroll::new(),
        }
    }

    pub fn label(mut self, name: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        self.scroll = self.scroll.label(name);
        self
    }

    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.scroll = self.scroll.size(width, height);
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.scroll = self.scroll.min_width(width);
        self
    }

    pub fn min_height(mut self, height: f32) -> Self {
        self.scroll = self.scroll.min_height(height);
        self
    }

    pub fn scrollbar(mut self, visible: bool) -> Self {
        self.scroll = self.scroll.scrollbar(visible);
        self
    }

    /// Pins the list to its end while it is already at the end, including when
    /// streamed data changes the estimated or measured content extent.
    pub fn follow_end(mut self, enabled: bool) -> Self {
        self.scroll = self.scroll.follow_end(enabled);
        self
    }

    pub fn restoration_key(mut self, key: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        self.scroll = self.scroll.restoration_key(key);
        self
    }
}

impl<K> View for VirtualList<K>
where
    K: Clone + Eq + Hash + 'static,
{
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let mut this = *self;
        this.controller.inner.borrow_mut().follow_end = this.scroll.follow_end;
        let controller = this.controller.clone();
        let callback_controller = controller.clone();
        this.scroll = this
            .scroll
            .on_scroll(move |offset| callback_controller.note_scroll(offset));
        let content = VirtualListContent {
            controller,
            row: this
                .row
                .take()
                .expect("VirtualList row factory is built once"),
        };
        Box::new(this.scroll.child(content)).build(ctx, parent)
    }
}

struct VirtualListContent<K> {
    controller: VirtualListController<K>,
    row: Box<dyn FnMut(&K) -> AnyView>,
}

impl<K> View for VirtualListContent<K>
where
    K: Clone + Eq + Hash + 'static,
{
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let scroll = parent.expect("VirtualListContent must be mounted below Scroll");
        let content = ctx.scene.insert(WidgetKind::Column, Some(scroll));
        ctx.scene.a11y_mut(content).role = Role::List.as_u16();
        ctx.layout
            .set_container(content, ContainerStyle::new(Container::Column));

        let before = spacer(ctx, content, 0.0);
        let after = spacer(ctx, content, self.controller.estimated_height());
        register_virtual_list(
            &ctx.runtime,
            Box::new(MountedList {
                controller: self.controller,
                row: self.row,
                scroll,
                content,
                before,
                after,
                mounted: HashMap::new(),
            }),
        );
        content
    }
}

fn spacer(ctx: &mut BuildCtx, parent: WidgetId, height: f32) -> WidgetId {
    let id = ctx.scene.insert(WidgetKind::Column, Some(parent));
    ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
    let mut style = ContainerStyle::new(Container::Column);
    style.height = Some(height.max(0.0));
    ctx.layout.set_container(id, style);
    id
}

/// Invoked by `App::frame` before layout. It updates only virtualized hosts whose
/// controller/window changed and leaves overlapping row roots resident.
pub fn reconcile_virtual_lists(
    runtime: &Runtime,
    context: &crate::Context,
    scene: &mut Scene,
    layout: &mut LayoutEngine,
    text: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    scale: f32,
) -> bool {
    let mut hosts = take_virtual_lists(runtime);
    let mut changed = false;
    for host in &mut hosts {
        changed |= host.reconcile(runtime, context, scene, layout, text, atlas, scale);
    }
    hosts.retain(|host| host.alive(scene));
    return_virtual_lists(runtime, hosts);
    changed
}

/// Invoked after layout. Measured row heights feed the prefix index; if that
/// changes an anchor or spacer, the caller performs one bounded follow-up pass.
pub fn measure_virtual_lists(
    runtime: &Runtime,
    scene: &mut Scene,
    layout: &mut LayoutEngine,
) -> bool {
    let mut hosts = take_virtual_lists(runtime);
    let mut changed = false;
    for host in &mut hosts {
        changed |= host.measure(scene, layout);
    }
    hosts.retain(|host| host.alive(scene));
    return_virtual_lists(runtime, hosts);
    changed
}

pub(crate) trait MountedVirtualList {
    fn alive(&self, scene: &Scene) -> bool;
    fn reconcile(
        &mut self,
        runtime: &Runtime,
        context: &crate::Context,
        scene: &mut Scene,
        layout: &mut LayoutEngine,
        text: &mut TextShaper,
        atlas: &mut GlyphAtlas,
        scale: f32,
    ) -> bool;
    fn measure(&mut self, scene: &mut Scene, layout: &mut LayoutEngine) -> bool;
}

struct MountedList<K> {
    controller: VirtualListController<K>,
    row: Box<dyn FnMut(&K) -> AnyView>,
    scroll: WidgetId,
    content: WidgetId,
    before: WidgetId,
    after: WidgetId,
    mounted: HashMap<K, WidgetId>,
}

impl<K> MountedVirtualList for MountedList<K>
where
    K: Clone + Eq + Hash + 'static,
{
    fn alive(&self, scene: &Scene) -> bool {
        scene.node(self.scroll).is_some() && scene.node(self.content).is_some()
    }

    fn reconcile(
        &mut self,
        runtime: &Runtime,
        context: &crate::Context,
        scene: &mut Scene,
        layout: &mut LayoutEngine,
        text: &mut TextShaper,
        atlas: &mut GlyphAtlas,
        scale: f32,
    ) -> bool {
        if !self.alive(scene) {
            return false;
        }
        let (keys, range, refresh, pin_end, sync_offset) = {
            let mut state = self.controller.inner.borrow_mut();
            // The scene owns the final clamped position. A row can measure far
            // beyond its estimate, so use that live value before choosing the
            // next pixel window instead of trusting a stale controller offset.
            // A pending controller sync is deliberate, for example an anchor
            // restoration after an insertion, and must win over the old scene.
            if state.scroll_sync.is_none() && !(state.follow_end && state.at_end) {
                if let Some(metrics) = scroll_metrics(scene, self.scroll) {
                    state.note_live_scroll(metrics.offset, metrics.max_offset);
                }
            }
            let range = state.window();
            let same_window = state.mounted_range == Some(range);
            if !state.dirty && same_window && state.refresh.is_empty() {
                return false;
            }
            state.dirty = false;
            state.mounted_range = Some(range);
            let keys = state.keys[range.0..range.1].to_vec();
            let refresh = std::mem::take(&mut state.refresh);
            let pin_end = state.follow_end && state.at_end;
            let sync_offset = state.take_scroll_sync();
            (keys, range, refresh, pin_end, sync_offset)
        };

        let target: HashSet<_> = keys.iter().cloned().collect();
        let stale: Vec<_> = self
            .mounted
            .keys()
            .filter(|key| !target.contains(*key) || refresh.contains(*key))
            .cloned()
            .collect();
        for key in stale {
            if let Some(root) = self.mounted.remove(&key) {
                let nodes = scene.subtree_nodes(root);
                purge_nodes(runtime, scene, &nodes);
                layout.remove_nodes(&nodes);
                let _ = scene.remove_subtree(root);
            }
        }

        for key in &keys {
            if self.mounted.contains_key(key) {
                continue;
            }
            let view = (self.row)(key);
            let id = view.build(
                &mut BuildCtx {
                    context: context.clone(),
                    runtime: runtime.clone(),
                    scene,
                    layout,
                    text,
                    atlas,
                    scale,
                },
                Some(self.content),
            );
            self.mounted.insert(key.clone(), id);
        }

        // The spacer nodes stay permanently mounted. Moving retained row roots
        // between them preserves WidgetIds for the overlapping window.
        for (offset, key) in keys.iter().enumerate() {
            if let Some(&row) = self.mounted.get(key) {
                scene.move_child_to_index(self.content, row, offset + 1);
            }
        }
        scene.move_child_to_index(self.content, self.after, keys.len() + 1);
        let (before, after) = {
            let state = self.controller.inner.borrow();
            (
                state.heights.prefix(range.0),
                state.heights.total() - state.heights.prefix(range.1),
            )
        };
        set_spacer_height(layout, self.before, before);
        set_spacer_height(layout, self.after, after);
        layout.sync_dirty_nodes(scene, &[self.content, self.before, self.after]);
        let mut dirty = DirtyFlags::LAYOUT;
        dirty.insert(DirtyFlags::PAINT);
        dirty.insert(DirtyFlags::A11Y);
        scene.mark_dirty(self.content, dirty);
        if pin_end {
            scene.set_scroll_offset(
                self.scroll,
                Point {
                    x: 0.0,
                    y: f32::MAX,
                },
            );
        } else if let Some(offset) = sync_offset {
            scene.set_scroll_offset(self.scroll, Point { x: 0.0, y: offset });
        }
        true
    }

    fn measure(&mut self, scene: &mut Scene, layout: &mut LayoutEngine) -> bool {
        if !self.alive(scene) {
            return false;
        }
        let viewport = scene
            .layout(self.scroll)
            .map_or(0.0, |rect| rect.rect.height);
        let width = scene
            .layout(self.content)
            .map_or(0.0, |rect| rect.rect.width);
        let measured: Vec<_> = self
            .mounted
            .iter()
            .filter_map(|(key, id)| {
                scene
                    .layout(*id)
                    .map(|box_| (key.clone(), box_.rect.height))
            })
            .collect();
        let mut state = self.controller.inner.borrow_mut();
        // Layout has just clamped the viewport, which is more reliable than
        // the controller's estimate while variable-height rows settle.
        let mut changed = false;
        if state.scroll_sync.is_none() && !(state.follow_end && state.at_end) {
            if let Some(metrics) = scroll_metrics(scene, self.scroll) {
                changed |= state.note_live_scroll(metrics.offset, metrics.max_offset);
            }
        }
        changed |= state.set_viewport(viewport);
        changed |= state.invalidate_width(width);
        for (key, height) in measured {
            changed |= state.set_height(&key, height);
        }
        if !changed {
            return false;
        }
        let range = state.mounted_range.unwrap_or((0, 0));
        let before = state.heights.prefix(range.0);
        let after = state.heights.total() - state.heights.prefix(range.1);
        let pin_end = state.follow_end && state.at_end;
        let sync_offset = state.take_scroll_sync();
        drop(state);
        set_spacer_height(layout, self.before, before);
        set_spacer_height(layout, self.after, after);
        layout.sync_dirty_nodes(scene, &[self.content, self.before, self.after]);
        let mut dirty = DirtyFlags::LAYOUT;
        dirty.insert(DirtyFlags::PAINT);
        scene.mark_dirty(self.content, dirty);
        if pin_end {
            scene.set_scroll_offset(
                self.scroll,
                Point {
                    x: 0.0,
                    y: f32::MAX,
                },
            );
        } else if let Some(offset) = sync_offset {
            scene.set_scroll_offset(self.scroll, Point { x: 0.0, y: offset });
        }
        true
    }
}

fn set_spacer_height(layout: &mut LayoutEngine, id: WidgetId, height: f32) {
    let mut style = layout
        .container_style(id)
        .unwrap_or_else(|| ContainerStyle::new(Container::Column));
    let height = height.max(0.0);
    if style.height != Some(height) {
        style.height = Some(height);
        layout.set_container(id, style);
    }
}

fn register_virtual_list(runtime: &Runtime, host: Box<dyn MountedVirtualList>) {
    runtime.with(|rt| rt.borrow_mut().virtual_lists.push(host));
}

fn take_virtual_lists(runtime: &Runtime) -> Vec<Box<dyn MountedVirtualList>> {
    runtime.with(|rt| std::mem::take(&mut rt.borrow_mut().virtual_lists))
}

fn return_virtual_lists(runtime: &Runtime, hosts: Vec<Box<dyn MountedVirtualList>>) {
    runtime.with(|rt| {
        let mut runtime = rt.borrow_mut();
        // A row factory can mount another list while its parent list is detached
        // for reconciliation. Preserve those new registrations on return.
        let mut nested = std::mem::take(&mut runtime.virtual_lists);
        let mut hosts = hosts;
        hosts.append(&mut nested);
        runtime.virtual_lists = hosts;
    });
}

struct Controller<K> {
    keys: Vec<K>,
    positions: HashMap<K, usize>,
    heights: HeightIndex,
    estimate: f32,
    overscan: f32,
    offset: f32,
    viewport: f32,
    width: f32,
    follow_end: bool,
    at_end: bool,
    dirty: bool,
    refresh: HashSet<K>,
    mounted_range: Option<(usize, usize)>,
    anchor: Option<(K, f32)>,
    scroll_sync: Option<f32>,
}

impl<K> Controller<K>
where
    K: Clone + Eq + Hash,
{
    fn new(keys: Vec<K>, estimate: f32) -> Self {
        let positions = positions(&keys);
        Self {
            heights: HeightIndex::new(keys.len(), estimate),
            keys,
            positions,
            estimate,
            overscan: DEFAULT_OVERSCAN,
            offset: 0.0,
            viewport: 0.0,
            width: 0.0,
            follow_end: false,
            at_end: true,
            dirty: true,
            refresh: HashSet::new(),
            mounted_range: None,
            anchor: None,
            scroll_sync: None,
        }
    }

    fn replace(&mut self, keys: Vec<K>) {
        self.capture_anchor();
        let old = std::mem::replace(&mut self.keys, keys);
        let old_positions = std::mem::take(&mut self.positions);
        let old_heights = std::mem::replace(&mut self.heights, HeightIndex::new(0, self.estimate));
        self.positions = positions(&self.keys);
        let mut heights = Vec::with_capacity(self.keys.len());
        for key in &self.keys {
            heights.push(
                old_positions
                    .get(key)
                    .map(|index| old_heights.value(*index))
                    .unwrap_or(self.estimate),
            );
        }
        self.heights = HeightIndex::from_values(heights);
        drop(old);
        self.refresh.clear();
        self.mounted_range = None;
        self.restore_anchor();
        self.dirty = true;
    }

    fn insert(&mut self, index: usize, key: K) {
        assert!(
            !self.positions.contains_key(&key),
            "VirtualList keys must be unique"
        );
        self.capture_anchor();
        let index = index.min(self.keys.len());
        self.keys.insert(index, key);
        self.positions = positions(&self.keys);
        self.heights.insert(index, self.estimate);
        self.mounted_range = None;
        self.restore_anchor();
        self.dirty = true;
    }

    fn remove(&mut self, key: &K) -> bool {
        let Some(index) = self.positions.remove(key) else {
            return false;
        };
        self.capture_anchor();
        self.keys.remove(index);
        self.positions = positions(&self.keys);
        self.heights.remove(index);
        self.refresh.remove(key);
        self.mounted_range = None;
        self.restore_anchor();
        self.dirty = true;
        true
    }

    fn refresh(&mut self, key: &K) {
        if self.positions.contains_key(key) {
            self.refresh.insert(key.clone());
            self.dirty = true;
        }
    }

    fn refresh_all(&mut self) {
        self.refresh.extend(self.keys.iter().cloned());
        self.dirty = true;
    }

    fn note_scroll(&mut self, offset: f32) {
        let offset = offset.max(0.0);
        let max = (self.heights.total() - self.viewport).max(0.0);
        self.at_end = max - offset <= 1.0;
        if (self.offset - offset).abs() > f32::EPSILON {
            self.offset = offset;
            self.scroll_sync = None;
            self.dirty = true;
        }
    }

    /// Reconciles the controller with the position that layout actually kept.
    /// Unlike [`note_scroll`], `at_end` comes from the scene's measured extent,
    /// which may temporarily differ from this controller's estimates.
    fn note_live_scroll(&mut self, offset: f32, max_offset: f32) -> bool {
        let offset = offset.max(0.0);
        let at_end = max_offset - offset <= 1.0;
        let changed = (self.offset - offset).abs() > f32::EPSILON || self.at_end != at_end;
        self.offset = offset;
        self.at_end = at_end;
        self.dirty |= changed;
        changed
    }

    fn set_viewport(&mut self, viewport: f32) -> bool {
        let viewport = viewport.max(0.0);
        if (self.viewport - viewport).abs() <= f32::EPSILON {
            return false;
        }
        self.viewport = viewport;
        if self.follow_end && self.at_end {
            self.offset = (self.heights.total() - viewport).max(0.0);
            self.scroll_sync = Some(self.offset);
        }
        self.dirty = true;
        true
    }

    fn invalidate_width(&mut self, width: f32) -> bool {
        let width = width.max(0.0);
        if (self.width - width).abs() <= 0.5 {
            return false;
        }
        self.capture_anchor();
        self.width = width;
        self.heights.reset(self.estimate);
        self.restore_anchor();
        self.dirty = true;
        true
    }

    fn set_height(&mut self, key: &K, height: f32) -> bool {
        let Some(&index) = self.positions.get(key) else {
            return false;
        };
        let height = sane_height(height);
        self.capture_anchor();
        if self.heights.set(index, height) {
            self.restore_anchor();
            self.dirty = true;
            true
        } else {
            false
        }
    }

    fn window(&self) -> (usize, usize) {
        if self.keys.is_empty() {
            return (0, 0);
        }
        if self.viewport <= 0.0 {
            return (0, 1);
        }
        let start = self
            .heights
            .lower_bound((self.offset - self.overscan).max(0.0));
        let end = self
            .heights
            .lower_bound(self.offset + self.viewport + self.overscan)
            .saturating_add(1)
            .min(self.keys.len());
        (
            start.min(self.keys.len()),
            end.max(start + 1).min(self.keys.len()),
        )
    }

    fn capture_anchor(&mut self) {
        if (self.follow_end && self.at_end) || self.keys.is_empty() {
            return;
        }
        let index = self
            .heights
            .lower_bound(self.offset)
            .min(self.keys.len() - 1);
        self.anchor = Some((
            self.keys[index].clone(),
            (self.offset - self.heights.prefix(index)).max(0.0),
        ));
    }

    fn restore_anchor(&mut self) {
        if self.follow_end && self.at_end {
            self.offset = (self.heights.total() - self.viewport).max(0.0);
            self.scroll_sync = Some(self.offset);
            return;
        }
        let Some((key, intra)) = self.anchor.take() else {
            return;
        };
        if let Some(&index) = self.positions.get(&key) {
            self.offset = self.heights.prefix(index) + intra;
            self.scroll_sync = Some(self.offset);
        }
    }

    fn take_scroll_sync(&mut self) -> Option<f32> {
        self.scroll_sync.take()
    }
}

/// Fenwick-backed prefix index. Point updates/searches are logarithmic; sequence
/// membership edits rebuild only the controller's compact numeric index.
struct HeightIndex {
    values: Vec<f32>,
    tree: Vec<f32>,
}

impl HeightIndex {
    fn new(len: usize, estimate: f32) -> Self {
        Self::from_values(vec![estimate; len])
    }
    fn from_values(values: Vec<f32>) -> Self {
        let mut index = Self {
            tree: vec![0.0; values.len() + 1],
            values,
        };
        for i in 0..index.values.len() {
            index.add(i, index.values[i]);
        }
        index
    }
    fn total(&self) -> f32 {
        self.prefix(self.values.len())
    }
    fn value(&self, index: usize) -> f32 {
        self.values[index]
    }
    fn prefix(&self, mut end: usize) -> f32 {
        let mut sum = 0.0;
        while end > 0 {
            sum += self.tree[end];
            end &= end - 1;
        }
        sum
    }
    fn set(&mut self, index: usize, value: f32) -> bool {
        if (self.values[index] - value).abs() <= 0.25 {
            return false;
        }
        let delta = value - self.values[index];
        self.values[index] = value;
        self.add(index, delta);
        true
    }
    fn reset(&mut self, value: f32) {
        self.values.fill(value);
        self.rebuild();
    }
    fn insert(&mut self, index: usize, value: f32) {
        self.values.insert(index, value);
        self.rebuild();
    }
    fn remove(&mut self, index: usize) {
        self.values.remove(index);
        self.rebuild();
    }
    fn rebuild(&mut self) {
        self.tree.clear();
        self.tree.resize(self.values.len() + 1, 0.0);
        for i in 0..self.values.len() {
            self.add(i, self.values[i]);
        }
    }
    fn add(&mut self, index: usize, delta: f32) {
        let mut i = index + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += i & i.wrapping_neg();
        }
    }
    /// First row whose trailing edge exceeds `offset`.
    fn lower_bound(&self, offset: f32) -> usize {
        if offset <= 0.0 {
            return 0;
        }
        let mut bit = 1usize;
        while bit < self.tree.len() {
            bit <<= 1;
        }
        let mut index = 0usize;
        let mut sum = 0.0;
        while bit != 0 {
            let next = index + bit;
            if next < self.tree.len() && sum + self.tree[next] <= offset {
                index = next;
                sum += self.tree[next];
            }
            bit >>= 1;
        }
        index.min(self.values.len())
    }
}

fn sane_height(height: f32) -> f32 {
    if height.is_finite() {
        height.max(1.0)
    } else {
        1.0
    }
}
fn positions<K: Clone + Eq + Hash>(keys: &[K]) -> HashMap<K, usize> {
    keys.iter()
        .cloned()
        .enumerate()
        .map(|(i, key)| (key, i))
        .collect()
}
fn assert_unique<K: Eq + Hash>(keys: &[K]) {
    let mut seen = HashSet::with_capacity(keys.len());
    assert!(
        keys.iter().all(|key| seen.insert(key)),
        "VirtualList keys must be unique"
    );
}

#[cfg(test)]
mod tests {
    use super::{Controller, HeightIndex};

    #[test]
    fn prefix_index_finds_mixed_height_and_giant_rows_without_scanning() {
        let mut index = HeightIndex::from_values(vec![12.0, 1_200.0, 18.0, 24.0]);
        assert_eq!(index.total(), 1_254.0);
        assert_eq!(index.lower_bound(0.0), 0);
        assert_eq!(index.lower_bound(11.0), 0);
        assert_eq!(index.lower_bound(12.0), 1);
        assert_eq!(index.lower_bound(500.0), 1, "a giant card remains one row");
        assert_eq!(index.lower_bound(1_212.0), 2);
        assert!(index.set(1, 40.0));
        assert_eq!(index.total(), 94.0);
        assert_eq!(index.lower_bound(51.0), 1);
        assert_eq!(index.lower_bound(52.0), 2);
    }

    #[test]
    fn controller_preserves_measured_heights_across_keyed_reorder() {
        let mut controller = Controller::new(vec!["a", "b", "c"], 20.0);
        assert!(controller.set_height(&"b", 91.0));
        controller.replace(vec!["c", "b", "d"]);
        assert_eq!(controller.heights.value(0), 20.0);
        assert_eq!(controller.heights.value(1), 91.0);
        assert_eq!(controller.heights.value(2), 20.0);
    }

    #[test]
    fn pixel_overscan_bounds_mounts_even_for_a_long_sequence() {
        let keys: Vec<_> = (0..10_000).collect();
        let mut controller = Controller::new(keys, 20.0);
        controller.set_viewport(100.0);
        controller.overscan = 40.0;
        controller.note_scroll(4_000.0);
        let range = controller.window();
        assert_eq!(range, (198, 208));
        assert!(range.1 - range.0 < 16);
    }

    #[test]
    fn keyed_reader_anchor_survives_insertions_above_it() {
        let mut controller = Controller::new(vec!["a", "b", "c", "d"], 10.0);
        controller.set_viewport(10.0);
        controller.note_scroll(25.0); // key c, five pixels into the row.
        controller.insert(0, "before");
        assert_eq!(controller.offset, 35.0);
        assert_eq!(controller.take_scroll_sync(), Some(35.0));
    }
}
