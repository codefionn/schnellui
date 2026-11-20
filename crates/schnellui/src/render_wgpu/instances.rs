//! The renderer-neutral half of the retained scene upload (SOUL §3.2): the CPU
//! mirrors of the quad, glyph and image instance buffers, the per-node fragment
//! table that lets a paint change rewrite only its own slot, and the scene walk
//! that fills them. [`GpuCore`](super::gpu_core::GpuCore) uploads the spans this
//! reports as changed; the CPU rasterizer in [`crate::render_soft`] draws the
//! mirrors directly. Both see the same instances in the same draw order.

use crate::render_wgpu::types::*;
use schnellui::scene::{Point, Primitive, Rect, Scene, WidgetId, WidgetKind, WidgetNode};

/// What a tree walk does with the fragments it visits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WalkMode {
    /// Full gather: create the fragments, pad every slot to its capacity.
    Gather,
    /// Re-walk of one scroll's content: refresh the existing fragments with exact
    /// (unpadded) ranges relative to the staging vectors.
    Region,
}

/// Slot capacity for `live` instances: the next power of two, so a node can gain a
/// few instances without moving anything behind it.
fn slot_capacity(live: usize) -> usize {
    if live == 0 {
        0
    } else {
        live.next_power_of_two()
    }
}

/// `rect` translated by the accumulated scroll `offset`.
fn shifted(rect: Rect, offset: Point) -> Rect {
    Rect::new(
        rect.x + offset.x,
        rect.y + offset.y,
        rect.width,
        rect.height,
    )
}

/// Converts one node's already-positioned display list into instances: quads and
/// lines into `quads`, glyphs into `glyphs`, images into `images`. `offset` is the
/// accumulated scroll translation and `clip` the accumulated viewport clip; a
/// primitive entirely outside `clip` is dropped (SOUL §3.2).
fn encode_prims(
    primitives: &[Primitive],
    offset: Point,
    clip: Rect,
    quads: &mut Vec<QuadInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    images: &mut Vec<GlyphInstance>,
) {
    let clip_arr = [clip.x, clip.y, clip.width, clip.height];
    for prim in primitives {
        match *prim {
            Primitive::SolidRect {
                rect,
                color,
                corner_radius,
            } => {
                let rect = shifted(rect, offset);
                if !rect.intersect(&clip).is_empty() {
                    quads.push(QuadInstance::solid_clipped(
                        rect,
                        color,
                        corner_radius,
                        clip_arr,
                    ));
                }
            }
            Primitive::StrokeRect {
                rect,
                color,
                corner_radius,
                width,
            } => {
                let rect = shifted(rect, offset);
                if !rect.intersect(&clip).is_empty() {
                    quads.push(QuadInstance::stroke_clipped(
                        rect,
                        color,
                        corner_radius,
                        width,
                        clip_arr,
                    ));
                }
            }
            Primitive::GlyphQuad {
                rect,
                atlas_uv,
                color,
            } => {
                let rect = shifted(rect, offset);
                if !rect.intersect(&clip).is_empty() {
                    glyphs.push(GlyphInstance::glyph_clipped(
                        rect, atlas_uv, color, clip_arr,
                    ));
                }
            }
            Primitive::Line {
                from,
                to,
                width,
                color,
            } => {
                let from = Point {
                    x: from.x + offset.x,
                    y: from.y + offset.y,
                };
                let to = Point {
                    x: to.x + offset.x,
                    y: to.y + offset.y,
                };
                let half_width = width * 0.5;
                let bounds = Rect::new(
                    from.x.min(to.x) - half_width,
                    from.y.min(to.y) - half_width,
                    (from.x - to.x).abs() + width,
                    (from.y - to.y).abs() + width,
                );
                if !bounds.intersect(&clip).is_empty() {
                    quads.push(QuadInstance::line(from, to, width, color, clip_arr));
                }
            }
            Primitive::ImageQuad {
                rect,
                atlas_uv,
                tint,
            } => {
                let rect = shifted(rect, offset);
                if !rect.intersect(&clip).is_empty() {
                    // instances reuse the glyph layout: dest, uv, tint, clip.
                    images.push(GlyphInstance::glyph_clipped(rect, atlas_uv, tint, clip_arr));
                }
            }
        }
    }
}

/// Text and RichText are paint leaves whose glyph bounds are contained by their
/// final layout box. If that box is completely outside the inherited scroll clip,
/// the node is skipped before touching its potentially thousands of glyph
/// primitives. This is not applied to containers or arbitrary paint leaves: their
/// descendants/primitives may legitimately overflow their own layout box (shadows,
/// popups, custom drawing).
fn leaf_culled(scene: &Scene, id: WidgetId, node: &WidgetNode, offset: Point, clip: Rect) -> bool {
    node.children.is_empty()
        && matches!(node.kind, WidgetKind::Text | WidgetKind::RichText)
        && scene.layout(id).is_some_and(|layout| {
            !layout.rect.is_empty() && shifted(layout.rect, offset).intersect(&clip).is_empty()
        })
}

/// A node with this many children or more gets a [`ChildIndex`] when it sits in
/// scroll content (or is a `Scroll`).
const INDEX_MIN: usize = 16;

/// Bounds of a `Scroll` that has no viewport rect yet and so clips nothing.
const UNBOUNDED: Rect = Rect::new(-1.0e18, -1.0e18, 2.0e18, 2.0e18);

/// Where a walked node sits relative to the scroll content it belongs to.
#[derive(Clone, Copy)]
pub(crate) struct WalkCtx {
    /// The nearest `Scroll` above the node.
    scroll: Option<WidgetId>,
    /// That scroll's content origin: the offset it hands its children.
    origin: Point,
    /// The nearest indexed ancestor (`NO_INDEX` if none) and the child of it
    /// the node sits under.
    scope: u32,
    child: u32,
    /// No `Scroll` lies between the node and `scope`.
    bounded: bool,
}

impl WalkCtx {
    const ROOT: WalkCtx = WalkCtx {
        scroll: None,
        origin: Point { x: 0.0, y: 0.0 },
        scope: NO_INDEX,
        child: 0,
        bounded: false,
    };
}

/// One entry of the walk frontier: a node, the accumulated scroll `offset` and
/// `clip` it inherits, whether this is a `Scroll`'s exit marker, and its context.
#[derive(Clone, Copy)]
pub(crate) struct WalkItem {
    id: WidgetId,
    offset: Point,
    clip: Rect,
    after: bool,
    ctx: WalkCtx,
}

/// The local bounds of what one primitive can draw (before scroll offsets).
fn prim_bounds(prim: &Primitive) -> Rect {
    match *prim {
        Primitive::SolidRect { rect, .. }
        | Primitive::StrokeRect { rect, .. }
        | Primitive::GlyphQuad { rect, .. }
        | Primitive::ImageQuad { rect, .. } => rect,
        Primitive::Line {
            from, to, width, ..
        } => {
            let half_width = width * 0.5;
            Rect::new(
                from.x.min(to.x) - half_width,
                from.y.min(to.y) - half_width,
                (from.x - to.x).abs() + width,
                (from.y - to.y).abs() + width,
            )
        }
    }
}

/// Local bounds of everything node `id` itself can make visible: its primitives
/// in `prims` (all of them when `None`), a text leaf's layout box (outside the
/// clip it is culled whole, see [`leaf_culled`]), and a `Scroll`'s viewport,
/// which clips all of its content whatever its offset.
fn own_bounds(
    scene: &Scene,
    id: WidgetId,
    node: &WidgetNode,
    prims: Option<std::ops::Range<usize>>,
) -> Rect {
    let mut r = Rect::ZERO;
    if node.kind == WidgetKind::Scroll {
        r = scene.layout(id).map_or(UNBOUNDED, |b| b.rect);
    } else if node.children.is_empty()
        && matches!(node.kind, WidgetKind::Text | WidgetKind::RichText)
    {
        if let Some(b) = scene.layout(id).filter(|b| !b.rect.is_empty()) {
            return b.rect;
        }
    }
    if let Some(paint) = scene.paint(id) {
        let all = &paint.primitives;
        let prims = prims
            .and_then(|range| all.get(range))
            .unwrap_or(all.as_slice());
        for prim in prims {
            r = r.union(&prim_bounds(prim));
        }
    }
    r
}

/// The `(offset, clip)` a `Scroll` node hands to its descendants: shifted by its
/// scroll offset and clipped to its laid-out viewport rect.
fn scroll_child_ctx(scene: &Scene, id: WidgetId, offset: Point, clip: Rect) -> (Point, Rect) {
    let so = scene.scroll_offset(id);
    let co = Point {
        x: offset.x - so.x,
        y: offset.y - so.y,
    };
    let cc = match scene.layout(id) {
        // the viewport frame itself is drawn at the parent `offset` (it is not
        // scrolled, only its children are), so offset the clip rect.
        Some(b) => clip.intersect(&shifted(b.rect, offset)),
        // no layout yet (pre-first-layout mount): keep the parent clip. A laid-out
        // empty viewport clips everything.
        None => clip,
    };
    (co, cc)
}

/// Appends `v[start..]`'s padding up to its slot capacity when `pad`, and returns
/// the vector's new length and the number of live instances.
fn seal_vec<T: bytemuck::Pod>(v: &mut Vec<T>, start: usize, pad: bool) -> (usize, u32) {
    let live = v.len() - start;
    if pad {
        v.resize(
            start + slot_capacity(live),
            <T as bytemuck::Zeroable>::zeroed(),
        );
    }
    (v.len(), live as u32)
}

/// Overwrites `scratch[start..start + cap]` with `staged` followed by zero padding
/// and returns the span that actually changed, if any.
fn patch_slot<T: bytemuck::Pod + PartialEq>(
    scratch: &mut [T],
    start: usize,
    cap: usize,
    staged: &[T],
) -> Option<(usize, usize)> {
    let zero = <T as bytemuck::Zeroable>::zeroed();
    let mut span: Option<(usize, usize)> = None;
    for (i, cell) in scratch[start..start + cap].iter_mut().enumerate() {
        let new = staged.get(i).copied().unwrap_or(zero);
        if *cell != new {
            *cell = new;
            let at = start + i;
            span = Some(span.map_or((at, at + 1), |(lo, _)| (lo, at + 1)));
        }
    }
    span
}

/// What writing a node's fresh instances into its slot required.
enum SlotEdit {
    Same,
    /// The slot's contents changed within `[lo, hi)`; nothing moved.
    InPlace(usize, usize),
    /// The slot grew to `new_cap`; the CPU mirror is already spliced from `from`.
    Grow {
        new_cap: u32,
        from: usize,
    },
}

/// Writes `staged` into the slot `range` of `scratch`. Growth beyond the slot's
/// capacity splices the vector: the first overflow of a slot fits it exactly, a slot
/// that overflows again gets doubled headroom.
fn apply_slot<T: bytemuck::Pod + PartialEq>(
    scratch: &mut Vec<T>,
    range: &std::ops::Range<u32>,
    staged: &[T],
    overflowed: bool,
) -> SlotEdit {
    let start = range.start as usize;
    let cap = range.len();
    if staged.len() <= cap {
        return match patch_slot(scratch, start, cap, staged) {
            Some((lo, hi)) => SlotEdit::InPlace(lo, hi),
            None => SlotEdit::Same,
        };
    }
    let new_cap = if overflowed {
        slot_capacity(staged.len()).max(cap * 2)
    } else {
        staged.len()
    };
    scratch.splice(
        start..start + cap,
        staged.iter().copied().chain(std::iter::repeat_n(
            <T as bytemuck::Zeroable>::zeroed(),
            new_cap - staged.len(),
        )),
    );
    SlotEdit::Grow {
        new_cap: new_cap as u32,
        from: start,
    }
}

/// A contiguous run of slots being replaced as one piece.
struct RunPlan {
    old_start: u32,
    old_end: u32,
    total: u32,
}

/// Writes `staged` (the run's live instances in draw order) over the run in `scratch`,
/// padding to the planned total. Returns the span to upload.
fn splice_run<T: bytemuck::Pod + PartialEq>(
    scratch: &mut Vec<T>,
    plan: &RunPlan,
    staged: &[T],
) -> (usize, usize) {
    let (start, end, total) = (
        plan.old_start as usize,
        plan.old_end as usize,
        plan.total as usize,
    );
    if total == end - start {
        return patch_slot(scratch, start, total, staged).unwrap_or((0, 0));
    }
    scratch.splice(
        start..end,
        staged.iter().copied().chain(std::iter::repeat_n(
            <T as bytemuck::Zeroable>::zeroed(),
            total - staged.len(),
        )),
    );
    (start, scratch.len())
}

/// Changed spans this many instances apart or closer are uploaded as one write.
const UPLOAD_MERGE_GAP: usize = 8;

/// Adds `delta` to every fragment's `b` range behind list position `from`.
fn shift_after(table: &mut FragmentTable, b: Buf, from: usize, delta: i64) {
    let FragmentTable { frags, order, .. } = table;
    for &fi in &order[b as usize][from.min(order[b as usize].len())..] {
        let r = frags[fi as usize].range_mut(b);
        r.start = (r.start as i64 + delta) as u32;
        r.end = (r.end as i64 + delta) as u32;
    }
}

fn shift_pos(v: &mut u32, delta: i64) {
    *v = (*v as i64 + delta) as u32;
}

/// Whether some proper ancestor of `id` is in `scrolls`.
fn covered_by(scene: &Scene, scrolls: &[WidgetId], id: WidgetId) -> bool {
    let mut cur = scene.node(id).and_then(|n| n.parent);
    while let Some(p) = cur {
        if scrolls.contains(&p) {
            return true;
        }
        cur = scene.node(p).and_then(|n| n.parent);
    }
    false
}

/// Where each primitive of one node lands in its instance slots: `offsets[i]`
/// counts the quad, glyph and image instances (indexed by [`Buf`]) produced by
/// primitives `[0, i)`. Valid only for the `offset` and `clip` it was built under.
#[derive(Default)]
pub(crate) struct PrimPrefix {
    /// The fragment it was built for.
    fi: u32,
    offsets: Vec<[u32; 3]>,
    offset: Point,
    clip: Rect,
}

/// The CPU instance mirrors and the retained bookkeeping behind them. Holds no
/// device resources, so any renderer can own one.
#[derive(Default)]
pub(crate) struct Instances {
    // cross-frame scratch (cleared-and-refilled, retains capacity — §4.4).
    pub(crate) quad_scratch: Vec<QuadInstance>,
    /// Fixed scroll chrome, appended after each layer's content so it draws after
    /// images and glyphs as well as ordinary quads.
    pub(crate) chrome_scratch: Vec<QuadInstance>,
    pub(crate) glyph_scratch: Vec<GlyphInstance>,
    /// image instances reuse the glyph layout (dest rect, texel uv, tint, clip).
    pub(crate) image_scratch: Vec<GlyphInstance>,
    /// pre-order walk frontier carrying each node's accumulated scroll `offset` and
    /// `clip` (SOUL §3.2). Items are `Copy`, so it stays a cleared-and-refilled
    /// grow-only `Vec` — zero steady-state allocation (§4.4).
    /// `after` marks a scroll's exit, used to composite fixed scroll chrome after
    /// the viewport's moving descendants.
    pub(crate) walk_stack: Vec<WalkItem>,
    /// overlay subtree roots deferred out of the base walk (SOUL §3.2 z-order),
    /// each with the (offset, clip) it inherited where it sits in the tree; walked
    /// after the base pass so their instances land in the overlay draw ranges.
    /// Cleared-and-refilled like the walk stack (§4.4); empty for overlay-less
    /// scenes, so the whole layer machinery costs nothing then.
    pub(crate) overlay_roots: Vec<(schnellui::scene::WidgetId, Point, Rect)>,
    /// instance counts at the base/overlay boundary: scratch `[0..base_*)` is the
    /// base layer, `[base_*..len)` the overlay layer (`GpuCore::record_pass`).
    pub(crate) base_quads: u32,
    pub(crate) base_chrome_start: u32,
    pub(crate) base_glyphs: u32,
    pub(crate) base_images: u32,
    /// Complete overlay draw ranges in bottom-to-top order. Surfaces, images,
    /// and text from one overlay are composited before the next overlay begins.
    pub(crate) overlay_layers: Vec<InstanceLayer>,
    /// Instance spans `[lo, hi)` changed in the CPU mirrors this frame, per
    /// buffer, waiting for [`Instances::merge_spans`].
    pending_uploads: [Vec<(usize, usize)>; 3],
    /// Instance spans `[lo, hi)` per buffer that the last [`Instances::prepare`]
    /// changed in the CPU mirrors: the merged upload runs, sorted and disjoint. A
    /// slot that grew or a run that changed size reports everything from the splice
    /// to the end, so instances that only moved are inside a span too. The tile
    /// cache re-hashes only these.
    pub(crate) changed_spans: [Vec<(usize, usize)>; 3],
    /// Bumped by every [`Instances::prepare`]. A reader that applied
    /// `changed_spans` at serial `n` may apply them again at `n + 1`; any other gap
    /// means it missed a frame's edits.
    pub(crate) upload_serial: u64,
    /// Bumped by every full gather, which rewrites the mirrors wholesale.
    pub(crate) gather_serial: u64,

    /// Per-node retained instance ranges from the most recent full gather. A
    /// paint-dirty node re-encodes only its own slot; tree, overlay-order and
    /// visibility changes rebuild the table with a full gather.
    pub(crate) fragments: FragmentTable,
    /// `(root, level, order)` of the overlays in draw order, as gathered.
    pub(crate) overlay_sig: Vec<(WidgetId, u8, u64)>,
    /// The overlay root being drawn as a layer of its own, which its walk must
    /// not defer again.
    overlay_root_walk: Option<WidgetId>,
    pub(crate) retained_scene_key: Option<schnellui::scene::SceneRenderKey>,
    /// `Scene::paint_epoch` the retained instance data was gathered or patched at.
    pub(crate) retained_paint_epoch: u64,
    /// Overlay index (0 = base) the walk is currently emitting into.
    pub(crate) walk_layer: u32,
    /// Scroll nodes whose chrome quads await the layer's chrome flush.
    pub(crate) chrome_pending: Vec<u32>,
    /// Generation stamped on fragments refreshed by a region re-walk.
    pub(crate) walk_stamp: u32,
    /// Fragments a region re-walk refreshed, in draw order, each with whether it
    /// has an entry in the quad content list (a nested scroll's quads are chrome).
    walk_seq: Vec<(u32, bool)>,
    /// Nested scrolls a region re-walk left, in exit (chrome) order.
    exit_seq: Vec<u32>,
    /// Child indices of the last full gather; `indices[..index_len]` are live,
    /// the rest keep their capacity for the next gather.
    pub(crate) indices: Vec<ChildIndex>,
    index_len: usize,
    /// The children a region re-walk keeps for one indexed node.
    window_scratch: Vec<u32>,
    /// Staging copies of one node's (or one scroll region's) fresh instances.
    pub(crate) stage_quads: Vec<QuadInstance>,
    pub(crate) stage_chrome: Vec<QuadInstance>,
    pub(crate) stage_glyphs: Vec<GlyphInstance>,
    pub(crate) stage_images: Vec<GlyphInstance>,
    /// Dirty `Scroll` nodes of the current incremental update.
    pub(crate) dirty_scrolls: Vec<WidgetId>,
    pub(crate) last_upload_work: GpuUploadWork,
    /// Per-primitive instance offsets for fragments of nodes that report dirty
    /// primitive ranges (`Scene::paint_dirty_ranges`). Built on demand; a full
    /// gather drops them all, a scroll re-walk drops those of the fragments it
    /// refreshed.
    pub(crate) prim_prefix: std::collections::HashMap<WidgetId, PrimPrefix>,
}

impl Instances {
    /// Brings the mirrors up to date with `scene` and records the changed spans in
    /// `changed_spans`. When only paint changed since the last call, each
    /// paint-dirty node rewrites just its own retained slot
    /// ([`Instances::try_incremental`]); a structural change falls back to a full
    /// gather, which reports every instance as changed.
    pub(crate) fn prepare(&mut self, scene: &Scene) {
        self.last_upload_work = GpuUploadWork::default();
        self.upload_serial += 1;
        for spans in &mut self.pending_uploads {
            spans.clear();
        }
        for spans in &mut self.changed_spans {
            spans.clear();
        }
        if !self.try_incremental(scene) {
            self.gather(scene);
            self.last_upload_work.full_gathers = 1;
            for b in BUFS {
                let len = self.scratch_len(b);
                self.upload_range(b, 0, len);
            }
        }
        self.merge_spans();
        self.retained_scene_key = Some(scene.render_key());
        self.retained_paint_epoch = scene.paint_epoch();
    }

    /// `(quads, glyphs, images)` instance counts of the mirrors.
    pub(crate) fn counts(&self) -> (u32, u32, u32) {
        (
            self.quad_scratch.len() as u32,
            self.glyph_scratch.len() as u32,
            self.image_scratch.len() as u32,
        )
    }

    /// The compositing layers in draw order: the base, then every overlay root.
    /// Within a layer the draw order is quads, images, glyphs, then scroll chrome.
    pub(crate) fn layers(&self) -> impl Iterator<Item = InstanceLayer> + '_ {
        let base = InstanceLayer {
            quad_start: 0,
            quad_end: self.base_chrome_start,
            chrome_start: self.base_chrome_start,
            chrome_end: self.base_quads,
            glyph_start: 0,
            glyph_end: self.base_glyphs,
            image_start: 0,
            image_end: self.base_images,
        };
        std::iter::once(base).chain(self.overlay_layers.iter().copied())
    }

    /// Forgets the retained instances so the next upload is a full gather.
    #[cfg(test)]
    pub(crate) fn invalidate_retained(&mut self) {
        self.retained_scene_key = None;
    }

    fn scratch_len(&self, b: Buf) -> usize {
        match b {
            Buf::Quad => self.quad_scratch.len(),
            Buf::Glyph => self.glyph_scratch.len(),
            Buf::Image => self.image_scratch.len(),
        }
    }

    /// Notes that instances `[lo, hi)` of buffer `b` changed in the CPU mirror.
    /// [`Instances::merge_spans`] turns the notes into `changed_spans` once the
    /// frame's edits are all in the mirror.
    fn upload_range(&mut self, b: Buf, lo: usize, hi: usize) {
        if lo < hi {
            self.pending_uploads[b as usize].push((lo, hi));
        }
    }

    /// Sorts and merges the frame's pending spans into `changed_spans`. Spans that
    /// touch or sit within [`UPLOAD_MERGE_GAP`] instances of each other become one
    /// span: each GPU `write_buffer` costs a staging allocation and a copy command,
    /// far more than the few instances in the gap.
    fn merge_spans(&mut self) {
        for b in BUFS {
            let mut spans = std::mem::take(&mut self.pending_uploads[b as usize]);
            let len = self.scratch_len(b);
            spans.sort_unstable();
            let changed = &mut self.changed_spans[b as usize];
            let mut run: Option<(usize, usize)> = None;
            for &(lo, hi) in &spans {
                // A later splice may have shrunk the mirror below this span.
                let hi = hi.min(len);
                if lo >= hi {
                    continue;
                }
                run = match run {
                    Some((run_lo, run_hi)) if lo <= run_hi + UPLOAD_MERGE_GAP => {
                        Some((run_lo, run_hi.max(hi)))
                    }
                    Some(done) => {
                        changed.push(done);
                        Some((lo, hi))
                    }
                    None => Some((lo, hi)),
                };
            }
            if let Some(done) = run {
                changed.push(done);
            }
            spans.clear();
            self.pending_uploads[b as usize] = spans;
        }
    }

    // --- incremental update: per-node retained instance slots -----------------

    /// Updates the retained instances for the nodes painted since this renderer last
    /// drew, without walking the scene. Returns `false` when a full gather is
    /// needed: a structural change, pending layout, a missed paint notification, a
    /// dirty overlay whose stacking position moved, or a scroll region that holds an
    /// overlay.
    ///
    /// Each paint-dirty node re-encodes its own primitives with the scroll offset and
    /// clip it was gathered under, and rewrites its slot. A node that outgrows its
    /// slot splices the CPU mirror and shifts every slot drawn after it, so draw
    /// order never changes; only the bytes behind the node are rewritten. A dirty
    /// `Scroll` node re-walks its whole content run instead, because its offset
    /// moved every descendant.
    fn try_incremental(&mut self, scene: &Scene) -> bool {
        let same = self
            .retained_scene_key
            .is_some_and(|key| key.same_structure(&scene.render_key()));
        if !same || !scene.layout_dirty().is_empty() {
            return false;
        }
        // Nothing painted since this renderer last drew: the frame pass may have
        // cleared the dirty lists, but the epoch still tells.
        if self.retained_paint_epoch == scene.paint_epoch() {
            return true;
        }
        // The dirty list only names nodes painted after the last clear. A renderer
        // that last drew before that clear missed some and must re-gather.
        if self.retained_paint_epoch < scene.paint_dirty_base_epoch() {
            return false;
        }
        let dirty = scene.paint_dirty();
        if dirty.is_empty() {
            return false;
        }

        self.dirty_scrolls.clear();
        for &id in dirty {
            let Some(node) = scene.node(id) else {
                return false;
            };
            // A scroll the last gather never reached (hidden) has no run to
            // re-walk and nothing drawn; it must not force a full gather.
            if node.kind == WidgetKind::Scroll && self.fragments.by_node.contains_key(&id) {
                self.dirty_scrolls.push(id);
            }
        }
        for &id in dirty {
            if self.fragments.by_node.contains_key(&id) {
                if scene.is_overlay(id) && !self.overlay_unchanged(scene, id) {
                    return false;
                }
            } else if scene.is_effectively_visible(id) {
                // A visible node the last gather never reached.
                return false;
            }
        }
        // Repainted nodes widen the child bounds they count towards first, so the
        // re-walks below and the dormancy test see where they can draw now.
        for &id in dirty {
            if let Some(&fi) = self.fragments.by_node.get(&id) {
                self.grow_bounds(scene, id, fi);
            }
        }

        let scrolls = std::mem::take(&mut self.dirty_scrolls);
        let mut ok = true;
        let stamp_before = self.walk_stamp;
        for &sid in &scrolls {
            if !covered_by(scene, &scrolls, sid) && !self.regather_scroll(scene, sid) {
                ok = false;
                break;
            }
        }
        let walks = self.walk_stamp.wrapping_sub(stamp_before);
        if walks > 0 && !self.prim_prefix.is_empty() {
            // A re-walk may have moved, re-clipped or re-encoded the fragments it
            // stamped; their primitive maps are stale.
            let frags = &self.fragments.frags;
            self.prim_prefix.retain(|_, prefix| {
                frags.get(prefix.fi as usize).is_some_and(|f| {
                    let d = f.stamp.wrapping_sub(stamp_before);
                    d == 0 || d > walks
                })
            });
        }
        if ok {
            for &id in dirty {
                let Some(&fi) = self.fragments.by_node.get(&id) else {
                    continue;
                };
                if !scrolls.is_empty() && covered_by(scene, &scrolls, id) {
                    continue;
                }
                // Outside a window its bounds cannot reach the clip: it draws
                // nothing, before and after, and its slots stay empty.
                if self.dormant(fi) {
                    continue;
                }
                let ranges = scene.paint_dirty_ranges(id);
                if ranges.is_some_and(|ranges| self.update_node_ranges(scene, id, fi, ranges)) {
                    self.last_upload_work.ranges_patched += 1;
                } else {
                    if !self.update_node(scene, id, fi) {
                        ok = false;
                        break;
                    }
                    // A node that reports ranges (or did before) gets its
                    // primitive-to-instance map rebuilt for the next ranged frame.
                    if ranges.is_some() || self.prim_prefix.contains_key(&id) {
                        self.rebuild_prefix(scene, id, fi);
                    }
                }
                self.last_upload_work.nodes_patched += 1;
            }
        }
        self.dirty_scrolls = scrolls;
        ok
    }

    /// Whether fragment `fi` lies outside the window of some [`ChildIndex`] above
    /// it. Such a node emits nothing (its subtree's bounds miss that index's clip)
    /// and owns only empty slots; its offset, clip and slot positions may be stale
    /// because no re-walk visited it since it left the window.
    fn dormant(&self, fi: u32) -> bool {
        let f = &self.fragments.frags[fi as usize];
        let (mut scope, mut child) = (f.scope, f.scope_child);
        while scope != NO_INDEX {
            let index = &self.indices[scope as usize];
            if !index
                .in_window
                .get(child as usize)
                .copied()
                .unwrap_or(false)
            {
                return true;
            }
            child = index.parent_child;
            scope = index.parent;
        }
        false
    }

    /// Unions repainted node `id`'s new bounds into the index leaves it counts
    /// towards. A leaf outside its window that now reaches the index's clip
    /// queues the index's scroll for a re-walk, which brings the child in.
    fn grow_bounds(&mut self, scene: &Scene, id: WidgetId, fi: u32) {
        let (bounded, scope, child, rel) = {
            let f = &self.fragments.frags[fi as usize];
            (f.bounded, f.scope, f.scope_child, f.rel)
        };
        if !bounded || scope == NO_INDEX {
            return;
        }
        let Some(node) = scene.node(id) else {
            return;
        };
        // A ranged edit only rewrote its ranges: the other primitives' bounds are
        // already in the leaf.
        let r = match scene.paint_dirty_ranges(id) {
            Some(ranges) => ranges.iter().fold(Rect::ZERO, |acc, range| {
                acc.union(&own_bounds(
                    scene,
                    id,
                    node,
                    Some(range.start as usize..range.end as usize),
                ))
            }),
            None => own_bounds(scene, id, node, None),
        };
        let (mut scope, mut child, mut r) = (scope, child, shifted(r, rel));
        loop {
            let index = &mut self.indices[scope as usize];
            if !index.grow(child, r) {
                break;
            }
            if !index.in_window[child as usize] && reaches(&index.leaf(child), &index.query) {
                let sid = index.scroll;
                if !self.dirty_scrolls.contains(&sid) && self.fragments.by_node.contains_key(&sid) {
                    self.dirty_scrolls.push(sid);
                }
            }
            let index = &self.indices[scope as usize];
            if !index.propagate || index.parent == NO_INDEX {
                break;
            }
            r = index.total();
            child = index.parent_child;
            scope = index.parent;
        }
    }

    /// Whether overlay root `id` still sits where the last gather put it.
    fn overlay_unchanged(&self, scene: &Scene, id: WidgetId) -> bool {
        self.overlay_sig.iter().any(|&(root, level, order)| {
            root == id && level == scene.overlay_level(id) && order == scene.overlay_order(id)
        })
    }

    /// Re-encodes only primitives `ranges` of a node whose primitive count did not
    /// change, writing them over the instances they produced last time. Returns
    /// `false` (having possibly rewritten some ranges already) when the caller must
    /// re-encode the whole node: no valid [`PrimPrefix`], a culled leaf, or a range
    /// whose primitives now produce a different number of instances per buffer.
    fn update_node_ranges(
        &mut self,
        scene: &Scene,
        id: WidgetId,
        fi: u32,
        ranges: &[std::ops::Range<u32>],
    ) -> bool {
        let (Some(node), Some(paint)) = (scene.node(id), scene.paint(id)) else {
            return false;
        };
        let (offset, clip, live, starts) = {
            let f = &self.fragments.frags[fi as usize];
            (
                f.offset,
                f.clip,
                f.live,
                [f.quads.start, f.glyphs.start, f.images.start],
            )
        };
        let Some(prefix) = self.prim_prefix.get(&id) else {
            return false;
        };
        let prims = &paint.primitives;
        if prefix.fi != fi
            || prefix.offsets.len() != prims.len() + 1
            || prefix.offset != offset
            || prefix.clip != clip
            || prefix.offsets.last() != Some(&live)
            || leaf_culled(scene, id, node, offset, clip)
        {
            return false;
        }
        let mut quads = std::mem::take(&mut self.stage_quads);
        let mut glyphs = std::mem::take(&mut self.stage_glyphs);
        let mut images = std::mem::take(&mut self.stage_images);
        let mut ok = true;
        for range in ranges {
            let (a, b) = (range.start as usize, range.end as usize);
            if a > b || b > prims.len() {
                ok = false;
                break;
            }
            quads.clear();
            glyphs.clear();
            images.clear();
            encode_prims(
                &prims[a..b],
                offset,
                clip,
                &mut quads,
                &mut glyphs,
                &mut images,
            );
            self.last_upload_work.primitives_encoded += b - a;
            let (from, to) = {
                let prefix = &self.prim_prefix[&id];
                (prefix.offsets[a], prefix.offsets[b])
            };
            let counts = [quads.len(), glyphs.len(), images.len()];
            if (0..3).any(|k| counts[k] != (to[k] - from[k]) as usize) {
                ok = false;
                break;
            }
            for b in BUFS {
                let k = b as usize;
                if counts[k] == 0 {
                    continue;
                }
                let start = (starts[k] + from[k]) as usize;
                let span = match b {
                    Buf::Quad => patch_slot(&mut self.quad_scratch, start, counts[k], &quads),
                    Buf::Glyph => patch_slot(&mut self.glyph_scratch, start, counts[k], &glyphs),
                    Buf::Image => patch_slot(&mut self.image_scratch, start, counts[k], &images),
                };
                if let Some((lo, hi)) = span {
                    self.upload_range(b, lo, hi);
                }
            }
        }
        self.stage_quads = quads;
        self.stage_glyphs = glyphs;
        self.stage_images = images;
        ok
    }

    /// Rebuilds fragment `fi`'s [`PrimPrefix`] from the node's current primitives,
    /// encoding them one at a time so the counts match `encode_prims` exactly. Runs
    /// once for a node that starts reporting ranges, and after any whole-node
    /// re-encode of such a node.
    fn rebuild_prefix(&mut self, scene: &Scene, id: WidgetId, fi: u32) {
        let (offset, clip) = {
            let f = &self.fragments.frags[fi as usize];
            (f.offset, f.clip)
        };
        let mut prefix = self.prim_prefix.remove(&id).unwrap_or_default();
        prefix.fi = fi;
        prefix.offsets.clear();
        prefix.offset = offset;
        prefix.clip = clip;
        let mut quads = std::mem::take(&mut self.stage_quads);
        let mut glyphs = std::mem::take(&mut self.stage_glyphs);
        let mut images = std::mem::take(&mut self.stage_images);
        let mut counts = [0_u32; 3];
        prefix.offsets.push(counts);
        if let Some(paint) = scene.paint(id) {
            for prim in &paint.primitives {
                quads.clear();
                glyphs.clear();
                images.clear();
                encode_prims(
                    std::slice::from_ref(prim),
                    offset,
                    clip,
                    &mut quads,
                    &mut glyphs,
                    &mut images,
                );
                counts[0] += quads.len() as u32;
                counts[1] += glyphs.len() as u32;
                counts[2] += images.len() as u32;
                prefix.offsets.push(counts);
            }
        }
        self.stage_quads = quads;
        self.stage_glyphs = glyphs;
        self.stage_images = images;
        self.prim_prefix.insert(id, prefix);
    }

    /// Re-encodes one node into its slots.
    fn update_node(&mut self, scene: &Scene, id: WidgetId, fi: u32) -> bool {
        let Some(node) = scene.node(id) else {
            return false;
        };
        let (offset, clip) = {
            let f = &self.fragments.frags[fi as usize];
            (f.offset, f.clip)
        };
        let mut quads = std::mem::take(&mut self.stage_quads);
        let mut glyphs = std::mem::take(&mut self.stage_glyphs);
        let mut images = std::mem::take(&mut self.stage_images);
        quads.clear();
        glyphs.clear();
        images.clear();
        if !leaf_culled(scene, id, node, offset, clip) {
            if let Some(paint) = scene.paint(id) {
                encode_prims(
                    &paint.primitives,
                    offset,
                    clip,
                    &mut quads,
                    &mut glyphs,
                    &mut images,
                );
                self.last_upload_work.primitives_encoded += paint.primitives.len();
            }
        }
        for b in BUFS {
            let (range, overflowed) = {
                let f = &self.fragments.frags[fi as usize];
                (f.range(b).clone(), f.overflowed)
            };
            let edit = match b {
                Buf::Quad => apply_slot(&mut self.quad_scratch, &range, &quads, overflowed),
                Buf::Glyph => apply_slot(&mut self.glyph_scratch, &range, &glyphs, overflowed),
                Buf::Image => apply_slot(&mut self.image_scratch, &range, &images, overflowed),
            };
            let live = match b {
                Buf::Quad => quads.len(),
                Buf::Glyph => glyphs.len(),
                Buf::Image => images.len(),
            };
            self.fragments.frags[fi as usize].live[b as usize] = live as u32;
            match edit {
                SlotEdit::Same => {}
                SlotEdit::InPlace(lo, hi) => self.upload_range(b, lo, hi),
                SlotEdit::Grow { new_cap, from } => {
                    self.last_upload_work.grown_slots += 1;
                    self.grow_slot(b, fi, new_cap);
                    let len = self.scratch_len(b);
                    self.upload_range(b, from, len);
                }
            }
        }
        self.stage_quads = quads;
        self.stage_glyphs = glyphs;
        self.stage_images = images;
        true
    }

    /// Records that fragment `fi`'s slot in `b` grew to `new_cap`: everything drawn
    /// after it moves back by the difference, as do the layer boundaries.
    fn grow_slot(&mut self, b: Buf, fi: u32, new_cap: u32) {
        let (old_cap, ord, layer, chrome) = {
            let f = &mut self.fragments.frags[fi as usize];
            let old_cap = f.range(b).len() as u32;
            f.range_mut(b).end = f.range(b).start + new_cap;
            f.overflowed = true;
            (
                old_cap,
                f.ord[b as usize],
                f.layer,
                f.chrome && b == Buf::Quad,
            )
        };
        let delta = new_cap as i64 - old_cap as i64;
        shift_after(&mut self.fragments, b, ord as usize + 1, delta);
        self.shift_boundaries(b, layer, chrome, delta);
    }

    /// Moves every layer boundary behind a resized slot. `layer` is the layer the
    /// slot sits in (0 = base) and `chrome` whether it is in that layer's chrome
    /// section. Boundaries are shifted by identity rather than by position, so empty
    /// slots sharing an index with a boundary cannot confuse it.
    fn shift_boundaries(&mut self, b: Buf, layer: u32, chrome: bool, delta: i64) {
        let layer = layer as usize;
        match b {
            Buf::Quad => {
                if layer == 0 {
                    if !chrome {
                        shift_pos(&mut self.base_chrome_start, delta);
                    }
                    shift_pos(&mut self.base_quads, delta);
                } else {
                    let l = &mut self.overlay_layers[layer - 1];
                    if !chrome {
                        shift_pos(&mut l.quad_end, delta);
                        shift_pos(&mut l.chrome_start, delta);
                    }
                    shift_pos(&mut l.chrome_end, delta);
                }
                for l in self.overlay_layers.iter_mut().skip(layer) {
                    shift_pos(&mut l.quad_start, delta);
                    shift_pos(&mut l.quad_end, delta);
                    shift_pos(&mut l.chrome_start, delta);
                    shift_pos(&mut l.chrome_end, delta);
                }
            }
            Buf::Glyph => {
                if layer == 0 {
                    shift_pos(&mut self.base_glyphs, delta);
                } else {
                    shift_pos(&mut self.overlay_layers[layer - 1].glyph_end, delta);
                }
                for l in self.overlay_layers.iter_mut().skip(layer) {
                    shift_pos(&mut l.glyph_start, delta);
                    shift_pos(&mut l.glyph_end, delta);
                }
            }
            Buf::Image => {
                if layer == 0 {
                    shift_pos(&mut self.base_images, delta);
                } else {
                    shift_pos(&mut self.overlay_layers[layer - 1].image_end, delta);
                }
                for l in self.overlay_layers.iter_mut().skip(layer) {
                    shift_pos(&mut l.image_start, delta);
                    shift_pos(&mut l.image_end, delta);
                }
            }
        }
    }

    /// Re-walks the content of dirty `Scroll` node `sid` and replaces its run of
    /// slots. The scroll offset (or viewport) changed, so every descendant moved and
    /// the visible set changed. The walk re-culls against the new clip, but at a
    /// node with a [`ChildIndex`] it visits only the children kept last time and
    /// those whose bounds reach the new clip: the rest drew nothing and still
    /// draw nothing. CPU cost is the visited nodes, uploads are the visible
    /// instances. Slots of visited fragments are rebuilt exact in draw order;
    /// skipped fragments keep their empty slots. The run keeps spare capacity at
    /// its end (the end marker's slot) so a scroll step whose visible count
    /// wobbles rewrites only the run. Returns `false` when the run cannot be
    /// rebuilt in place.
    fn regather_scroll(&mut self, scene: &Scene, sid: WidgetId) -> bool {
        let Some(&fi) = self.fragments.by_node.get(&sid) else {
            return false;
        };
        let (offset, clip, layer, region, has_overlay, ctx) = {
            let f = &self.fragments.frags[fi as usize];
            let ctx = WalkCtx {
                scope: f.scope,
                child: f.scope_child,
                bounded: f.bounded,
                ..WalkCtx::ROOT
            };
            (f.offset, f.clip, f.layer, f.region, f.has_overlay, ctx)
        };
        let Some((begin, end)) = region else {
            return false;
        };
        if has_overlay {
            return false;
        }
        // Outside a window the whole viewport is clipped away: its content draws
        // nothing at any offset, and its slots are empty.
        if self.dormant(fi) {
            return true;
        }
        let Some(node) = scene.node(sid) else {
            return false;
        };

        self.walk_stamp = self.walk_stamp.wrapping_add(1);
        self.walk_seq.clear();
        self.exit_seq.clear();
        self.chrome_pending.clear();
        std::mem::swap(&mut self.quad_scratch, &mut self.stage_quads);
        std::mem::swap(&mut self.chrome_scratch, &mut self.stage_chrome);
        std::mem::swap(&mut self.glyph_scratch, &mut self.stage_glyphs);
        std::mem::swap(&mut self.image_scratch, &mut self.stage_images);
        self.quad_scratch.clear();
        self.chrome_scratch.clear();
        self.glyph_scratch.clear();
        self.image_scratch.clear();
        self.walk_stack.clear();
        let (child_offset, child_clip) = scroll_child_ctx(scene, sid, offset, clip);
        let walked = self.push_children(fi, sid, node, child_offset, child_clip, ctx, true, false)
            && self.drain_walk(scene, false, WalkMode::Region);
        std::mem::swap(&mut self.quad_scratch, &mut self.stage_quads);
        std::mem::swap(&mut self.chrome_scratch, &mut self.stage_chrome);
        std::mem::swap(&mut self.glyph_scratch, &mut self.stage_glyphs);
        std::mem::swap(&mut self.image_scratch, &mut self.stage_images);
        if !walked {
            return false;
        }

        // Content run: everything between the begin marker and the end marker
        // (whose slot is the run's spare capacity).
        for b in BUFS {
            let (staged_len, old_end) = (
                match b {
                    Buf::Quad => self.stage_quads.len(),
                    Buf::Glyph => self.stage_glyphs.len(),
                    Buf::Image => self.stage_images.len(),
                },
                self.fragments.frags[end as usize].range(b).end,
            );
            let old_start = self.fragments.frags[begin as usize].range(b).end;
            let Some(plan) = self.plan_content(b, end, staged_len, layer, old_start, old_end)
            else {
                return false;
            };
            let (ulo, uhi) = match b {
                Buf::Quad => splice_run(&mut self.quad_scratch, &plan, &self.stage_quads),
                Buf::Glyph => splice_run(&mut self.glyph_scratch, &plan, &self.stage_glyphs),
                Buf::Image => splice_run(&mut self.image_scratch, &plan, &self.stage_images),
            };
            self.upload_range(b, ulo, uhi);
        }

        // Chrome of the nested scrolls the walk left: the slots just before this
        // scroll's own chrome slot, in exit order. Those of skipped nested
        // scrolls in between are empty.
        if !self.exit_seq.is_empty() {
            let Some(plan) = self.plan_chrome(fi, layer) else {
                return false;
            };
            let (ulo, uhi) = splice_run(&mut self.quad_scratch, &plan, &self.stage_chrome);
            self.upload_range(Buf::Quad, ulo, uhi);
        }
        self.last_upload_work.scroll_regions += 1;
        true
    }

    /// Assigns fresh ranges in buffer `b` to the fragments the region walk
    /// refreshed (`walk_seq`), packed from `old_start`, and gives end marker `end`
    /// the run's spare capacity: the run keeps its size when the new content
    /// fits, and grows with headroom otherwise. Everything drawn after the run
    /// shifts if its total changed. Skipped fragments own empty slots inside the
    /// run, so packing the refreshed ones tiles it exactly.
    fn plan_content(
        &mut self,
        b: Buf,
        end: u32,
        staged_len: usize,
        layer: u32,
        old_start: u32,
        old_end: u32,
    ) -> Option<RunPlan> {
        let old_total = old_end.checked_sub(old_start)?;
        let frags = &mut self.fragments.frags;
        let mut cursor = old_start;
        for &(e, in_quads) in &self.walk_seq {
            if b == Buf::Quad && !in_quads {
                continue;
            }
            let f = &mut frags[e as usize];
            let len = f.live[b as usize];
            *f.range_mut(b) = cursor..cursor + len;
            cursor += len;
        }
        let used = cursor - old_start;
        if used as usize != staged_len {
            return None;
        }
        let total = if used <= old_total {
            old_total
        } else {
            used + used / 4 + 4
        };
        let f = &mut frags[end as usize];
        *f.range_mut(b) = cursor..old_start + total;
        f.live[b as usize] = 0;
        let after = f.ord[b as usize] as usize + 1;
        let delta = total as i64 - old_total as i64;
        if delta != 0 {
            shift_after(&mut self.fragments, b, after, delta);
            self.shift_boundaries(b, layer, false, delta);
        }
        Some(RunPlan {
            old_start,
            old_end,
            total,
        })
    }

    /// Assigns exact chrome ranges to the nested scrolls the region walk left
    /// (`exit_seq`), packed to end where scroll `fi`'s own chrome slot starts, and
    /// shifts that slot and everything after it by the change in size.
    fn plan_chrome(&mut self, fi: u32, layer: u32) -> Option<RunPlan> {
        let (chrome_ord, old_end) = {
            let f = &self.fragments.frags[fi as usize];
            (f.ord[0], f.quads.start)
        };
        if chrome_ord == NO_ORD {
            return None;
        }
        let frags = &mut self.fragments.frags;
        let old_total: u32 = self
            .exit_seq
            .iter()
            .map(|&e| frags[e as usize].quads.len() as u32)
            .sum();
        let old_start = old_end.checked_sub(old_total)?;
        let mut cursor = old_start;
        for &e in &self.exit_seq {
            let f = &mut frags[e as usize];
            let len = f.live[0];
            f.quads = cursor..cursor + len;
            cursor += len;
        }
        let total = cursor - old_start;
        if total as usize != self.stage_chrome.len() {
            return None;
        }
        let delta = total as i64 - old_total as i64;
        if delta != 0 {
            shift_after(&mut self.fragments, Buf::Quad, chrome_ord as usize, delta);
            self.shift_boundaries(Buf::Quad, layer, true, delta);
        }
        Some(RunPlan {
            old_start,
            old_end,
            total,
        })
    }

    // --- full gather -----------------------------------------------------------

    fn new_frag(&mut self, node: Option<WidgetId>, offset: Point, clip: Rect) -> u32 {
        let t = &mut self.fragments;
        let fi = t.frags.len() as u32;
        t.frags.push(GpuFragment {
            offset,
            clip,
            ord: [NO_ORD; 3],
            layer: self.walk_layer,
            scope: NO_INDEX,
            index: NO_INDEX,
            ..Default::default()
        });
        if let Some(id) = node {
            t.by_node.insert(id, fi);
        }
        fi
    }

    /// Appends fragment `fi` to buffer `b`'s draw-order list.
    fn list_push(&mut self, fi: u32, b: Buf) {
        let t = &mut self.fragments;
        t.frags[fi as usize].ord[b as usize] = t.order[b as usize].len() as u32;
        t.order[b as usize].push(fi);
    }

    /// Puts marker fragment `mi` at the walk's current position in all buffers.
    fn place_marker(&mut self, mi: u32, gather: bool) {
        let (q, g, i) = (
            self.quad_scratch.len() as u32,
            self.glyph_scratch.len() as u32,
            self.image_scratch.len() as u32,
        );
        let stamp = self.walk_stamp;
        let f = &mut self.fragments.frags[mi as usize];
        // A region walk writes into staging vectors; the retained ranges stay put
        // until the run is re-planned from the live counts.
        if gather {
            f.quads = q..q;
            f.glyphs = g..g;
            f.images = i..i;
        }
        f.live = [0; 3];
        f.stamp = stamp;
        if gather {
            for b in BUFS {
                self.list_push(mi, b);
            }
        } else {
            self.walk_seq.push((mi, true));
        }
    }

    /// Records the instances a node just appended (from `starts` to the current
    /// vector ends) as fragment `fi`. When gathering, each slot is padded to its
    /// capacity, takes the range and joins the draw-order lists named in `lists`; a
    /// region walk records the live count and the fragment's place in `walk_seq`
    /// (and `exit_seq` for scroll chrome).
    fn seal_fragment(
        &mut self,
        fi: u32,
        starts: [usize; 3],
        chrome: bool,
        lists: [bool; 3],
        gather: bool,
    ) {
        for b in BUFS {
            let start = starts[b as usize];
            let (end, live) = match b {
                Buf::Quad if chrome => seal_vec(&mut self.chrome_scratch, start, gather),
                Buf::Quad => seal_vec(&mut self.quad_scratch, start, gather),
                Buf::Glyph => seal_vec(&mut self.glyph_scratch, start, gather),
                Buf::Image => seal_vec(&mut self.image_scratch, start, gather),
            };
            let f = &mut self.fragments.frags[fi as usize];
            if gather {
                *f.range_mut(b) = start as u32..end as u32;
            }
            f.live[b as usize] = live;
            f.chrome = chrome;
            if gather && lists[b as usize] {
                self.list_push(fi, b);
            }
        }
        if !gather {
            self.walk_seq.push((fi, !chrome));
            if chrome {
                self.exit_seq.push(fi);
            }
        }
    }

    /// Appends the pending scroll chrome after the layer's content and registers the
    /// chrome slots in draw order. Returns `(content_end, chrome_end)`.
    fn flush_chrome(&mut self) -> (u32, u32) {
        let content_end = self.quad_scratch.len() as u32;
        self.quad_scratch.extend_from_slice(&self.chrome_scratch);
        self.chrome_scratch.clear();
        let pending = std::mem::take(&mut self.chrome_pending);
        for &fi in &pending {
            let r = &mut self.fragments.frags[fi as usize].quads;
            r.start += content_end;
            r.end += content_end;
            self.list_push(fi, Buf::Quad);
        }
        self.chrome_pending = pending;
        self.chrome_pending.clear();
        (content_end, self.quad_scratch.len() as u32)
    }

    /// Walks the retained tree pre-order from the root, refilling the scratch instance
    /// buffers from each node's paint primitives (§3.2) and recording every node's
    /// slots. Cleared-and-refilled: no steady-state allocation once the scratch
    /// capacity is warm (§4.4).
    ///
    /// **Scroll composition (SOUL §3.2).** Each stack frame carries the accumulated
    /// `(offset, clip)`: the root starts unclipped at zero offset. Every emitted
    /// instance has `offset` added to its rect (or line endpoints) and carries `clip`.
    /// A [`WidgetKind::Scroll`] node shifts its children by `-scroll_offset(id)` and
    /// intersects the running clip with its own laid-out viewport rect, so scrolling
    /// re-composites the descendants without any relayout or per-node repaint.
    pub(crate) fn gather(&mut self, scene: &Scene) {
        self.gather_serial += 1;
        self.quad_scratch.clear();
        self.chrome_scratch.clear();
        self.glyph_scratch.clear();
        self.image_scratch.clear();
        self.fragments.clear();
        self.prim_prefix.clear();
        self.chrome_pending.clear();
        self.walk_stack.clear();
        self.overlay_roots.clear();
        self.overlay_layers.clear();
        self.overlay_sig.clear();
        self.walk_layer = 0;
        self.walk_stamp = self.walk_stamp.wrapping_add(1);
        self.index_len = 0;
        let unclipped = Rect::new(
            UNCLIPPED_CLIP[0],
            UNCLIPPED_CLIP[1],
            UNCLIPPED_CLIP[2],
            UNCLIPPED_CLIP[3],
        );
        if let Some(root) = scene.root() {
            self.walk_stack.push(WalkItem {
                id: root,
                offset: Point::default(),
                clip: unclipped,
                after: false,
                ctx: WalkCtx::ROOT,
            });
        }
        // Base layer: the whole tree except overlay subtrees, which are deferred
        // with the (offset, clip) they inherit where they sit (SOUL §3.2 z-order).
        self.drain_walk(scene, true, WalkMode::Gather);
        let (content_end, chrome_end) = self.flush_chrome();
        self.base_chrome_start = content_end;
        self.base_quads = chrome_end;
        self.base_glyphs = self.glyph_scratch.len() as u32;
        self.base_images = self.image_scratch.len() as u32;
        // Overlay layer: mutable foreground order within an explicit stacking
        // level (modal dialogs > modeless peers > ordinary popups). The base walk's
        // LIFO traversal discovers siblings backwards; sorting by the scene's
        // `(level, order)` makes focus/press raises authoritative. Drain one subtree
        // at a time so later roots really emit later and paint on top.
        self.overlay_roots.reverse();
        self.overlay_roots
            .sort_by_key(|(id, _, _)| (scene.overlay_level(*id), scene.overlay_order(*id)));
        // An overlay nested in another (a dropdown popup inside a dialog) is a
        // layer of its own, drawn right after its parent's layer so content
        // later in the parent's tree order cannot cover it. The list grows as
        // nested roots are found, so this is a `while`, not a range.
        let mut i = 0;
        while i < self.overlay_roots.len() {
            let root = self.overlay_roots[i];
            self.overlay_sig.push((
                root.0,
                scene.overlay_level(root.0),
                scene.overlay_order(root.0),
            ));
            self.walk_layer = i as u32 + 1;
            let start = InstanceLayer {
                quad_start: self.quad_scratch.len() as u32,
                glyph_start: self.glyph_scratch.len() as u32,
                image_start: self.image_scratch.len() as u32,
                ..Default::default()
            };
            let found = self.overlay_roots.len();
            self.walk_stack.push(WalkItem {
                id: root.0,
                offset: root.1,
                clip: root.2,
                after: false,
                ctx: WalkCtx::ROOT,
            });
            // Defer the overlays below this root, not the root itself.
            self.overlay_root_walk = Some(root.0);
            self.drain_walk(scene, true, WalkMode::Gather);
            self.overlay_root_walk = None;
            let (content_end, chrome_end) = self.flush_chrome();
            self.overlay_layers.push(InstanceLayer {
                quad_end: content_end,
                chrome_start: content_end,
                chrome_end,
                glyph_end: self.glyph_scratch.len() as u32,
                image_end: self.image_scratch.len() as u32,
                ..start
            });
            // Nested roots were found last-drawn-first. Order them by the same
            // `(level, order)` as the top-level roots, so a raised one (a shown
            // tooltip, a popup) paints over its peers in the parent, and put
            // them directly after this layer, without allocating.
            let nested = self.overlay_roots.len() - found;
            self.overlay_roots[found..].reverse();
            self.overlay_roots[found..]
                .sort_by_key(|(id, _, _)| (scene.overlay_level(*id), scene.overlay_order(*id)));
            self.overlay_roots[i + 1..].rotate_right(nested);
            i += 1;
        }
        self.finish_indices();
    }

    /// Builds the child indices the gather filled and takes each one's window
    /// from its bounds and clip. Inner indices come later in pre-order, so walking
    /// backwards folds each total into its parent's leaf before that is built.
    fn finish_indices(&mut self) {
        for ix in (0..self.index_len).rev() {
            let index = &mut self.indices[ix];
            index.build();
            index.refresh_window();
            let (parent, child, total) = (index.parent, index.parent_child, index.total());
            if index.propagate && parent != NO_INDEX {
                self.indices[parent as usize].add(child, total);
            }
        }
    }

    /// Takes a fresh [`ChildIndex`] from the pool for a node with `leaves`
    /// children inside the content of `scroll`.
    fn new_index(
        &mut self,
        scroll: WidgetId,
        leaves: usize,
        query: Rect,
        ctx: WalkCtx,
        propagate: bool,
    ) -> u32 {
        if self.index_len == self.indices.len() {
            self.indices.push(ChildIndex::default());
        }
        let ix = self.index_len;
        self.index_len += 1;
        let index = &mut self.indices[ix];
        index.reset(scroll, leaves, query);
        index.parent = ctx.scope;
        index.parent_child = ctx.child;
        index.propagate = propagate;
        ix as u32
    }

    /// Records where a node sits for the dormancy test and the bounds updates,
    /// and adds its own bounds to the index leaf it counts towards.
    fn record_scope(
        &mut self,
        scene: &Scene,
        fi: u32,
        id: WidgetId,
        node: &WidgetNode,
        ctx: WalkCtx,
    ) {
        let rel = {
            let f = &mut self.fragments.frags[fi as usize];
            f.scope = ctx.scope;
            f.scope_child = ctx.child;
            f.bounded = ctx.bounded;
            f.rel = Point {
                x: f.offset.x - ctx.origin.x,
                y: f.offset.y - ctx.origin.y,
            };
            f.rel
        };
        if ctx.bounded && ctx.scope != NO_INDEX {
            let r = own_bounds(scene, id, node, None);
            if !r.is_empty() {
                self.indices[ctx.scope as usize].add(ctx.child, shifted(r, rel));
            }
        }
    }

    /// Flags every `Scroll` above overlay root `id` whose content the overlay
    /// hangs off: a re-walk of it could not place the overlay's layer.
    fn mark_overlay_scrolls(&mut self, scene: &Scene, id: WidgetId) {
        let mut cur = scene.node(id).and_then(|n| n.parent);
        while let Some(p) = cur {
            let Some(node) = scene.node(p) else {
                break;
            };
            if node.kind == WidgetKind::Scroll {
                if let Some(&fi) = self.fragments.by_node.get(&p) {
                    self.fragments.frags[fi as usize].has_overlay = true;
                }
            }
            cur = node.parent;
        }
    }

    /// Pushes the children of walked node `id` (fragment `fi`) with the context
    /// they inherit. A full gather pushes every child and, for a wide node in
    /// scroll content, starts its [`ChildIndex`]. A region walk of an indexed
    /// node pushes only the children it kept last time and those whose bounds
    /// reach the new clip, then keeps the latter. Returns `false` when the
    /// index no longer matches the node.
    #[allow(clippy::too_many_arguments)]
    fn push_children(
        &mut self,
        fi: u32,
        id: WidgetId,
        node: &WidgetNode,
        child_offset: Point,
        child_clip: Rect,
        ctx: WalkCtx,
        is_scroll: bool,
        gather: bool,
    ) -> bool {
        let (scroll, origin) = if is_scroll {
            (Some(id), child_offset)
        } else {
            (ctx.scroll, ctx.origin)
        };
        let query = shifted(
            child_clip,
            Point {
                x: -origin.x,
                y: -origin.y,
            },
        );
        let mut ix = self.fragments.frags[fi as usize].index;
        if gather {
            if let Some(sid) = scroll.filter(|_| node.children.len() >= INDEX_MIN) {
                let propagate = !is_scroll && ctx.bounded;
                ix = self.new_index(sid, node.children.len(), query, ctx, propagate);
                self.fragments.frags[fi as usize].index = ix;
            }
        }
        let item = |child: WidgetId, ctx: WalkCtx| WalkItem {
            id: child,
            offset: child_offset,
            clip: child_clip,
            after: false,
            ctx,
        };
        if ix == NO_INDEX {
            let ctx = WalkCtx {
                scroll,
                origin,
                bounded: ctx.bounded && !is_scroll,
                ..ctx
            };
            for &child in node.children.iter().rev() {
                self.walk_stack.push(item(child, ctx));
            }
            return true;
        }
        let base = WalkCtx {
            scroll,
            origin,
            scope: ix,
            child: 0,
            bounded: true,
        };
        if gather {
            for (k, &child) in node.children.iter().enumerate().rev() {
                self.walk_stack.push(item(
                    child,
                    WalkCtx {
                        child: k as u32,
                        ..base
                    },
                ));
            }
            return true;
        }
        let Self {
            indices,
            window_scratch,
            walk_stack,
            ..
        } = self;
        let index = &mut indices[ix as usize];
        if index.leaves as usize != node.children.len() {
            return false;
        }
        index.query = query;
        window_scratch.clear();
        index.query_into(query, window_scratch);
        // Merge the old and the new window, descending, so the children pop in
        // tree order.
        let (old, new) = (&index.window, &*window_scratch);
        let (mut i, mut j) = (old.len(), new.len());
        while i > 0 || j > 0 {
            let k = match (
                i.checked_sub(1).map(|i| old[i]),
                j.checked_sub(1).map(|j| new[j]),
            ) {
                (Some(a), Some(b)) if a == b => {
                    i -= 1;
                    j -= 1;
                    a
                }
                (Some(a), Some(b)) if a > b => {
                    i -= 1;
                    a
                }
                (Some(a), None) => {
                    i -= 1;
                    a
                }
                (_, Some(b)) => {
                    j -= 1;
                    b
                }
                (None, None) => unreachable!(),
            };
            walk_stack.push(item(
                node.children[k as usize],
                WalkCtx { child: k, ..base },
            ));
        }
        for &k in &index.window {
            index.in_window[k as usize] = false;
        }
        for &k in window_scratch.iter() {
            index.in_window[k as usize] = true;
        }
        index.window.clear();
        index.window.extend_from_slice(window_scratch);
        true
    }

    /// Drains [`Self::walk_stack`], emitting instances for every popped node and
    /// pushing its children: the shared body of the gather layers and the region
    /// re-walk (SOUL §3.2). With `defer_overlays`, an overlay-flagged node is parked
    /// in [`Self::overlay_roots`] (subtree untouched) instead of emitted. A region
    /// re-walk returns `false` if it meets an overlay or a node it has no fragment
    /// for, because the rebuilt run could then not match the retained layout.
    fn drain_walk(&mut self, scene: &Scene, defer_overlays: bool, mode: WalkMode) -> bool {
        let gather = mode == WalkMode::Gather;
        while let Some(item) = self.walk_stack.pop() {
            let WalkItem {
                id,
                offset,
                clip,
                after,
                ctx,
            } = item;
            if !scene.is_visible(id) {
                continue;
            }
            if !after && scene.is_overlay(id) && self.overlay_root_walk != Some(id) {
                if defer_overlays {
                    // An overlay takes its clip from its layer: frame clips of
                    // the windows above it do not reach it.
                    let (_, inherited) = scene.overlay_inherited_context(id);
                    let unclipped = Rect::new(
                        UNCLIPPED_CLIP[0],
                        UNCLIPPED_CLIP[1],
                        UNCLIPPED_CLIP[2],
                        UNCLIPPED_CLIP[3],
                    );
                    self.overlay_roots
                        .push((id, offset, inherited.intersect(&unclipped)));
                    if gather {
                        self.mark_overlay_scrolls(scene, id);
                    }
                    continue;
                }
                if !gather {
                    return false;
                }
            }
            let Some(node) = scene.node(id) else {
                continue;
            };
            if !gather && !after {
                self.last_upload_work.walked_nodes += 1;
            }
            let (offset, clip) = if after {
                (offset, clip)
            } else {
                let properties = scene.composite_properties(id);
                let offset = Point {
                    x: offset.x + properties.translation.x,
                    y: offset.y + properties.translation.y,
                };
                let clip = properties
                    .clip
                    .map_or(clip, |local| clip.intersect(&shifted(local, offset)));
                (offset, clip)
            };
            let is_scroll = node.kind == WidgetKind::Scroll;

            let fi = if after {
                match self.fragments.by_node.get(&id) {
                    Some(&fi) => fi,
                    None => return false,
                }
            } else if gather {
                let fi = self.new_frag(Some(id), offset, clip);
                self.record_scope(scene, fi, id, node, ctx);
                fi
            } else {
                let stamp = self.walk_stamp;
                match self.fragments.by_node.get(&id) {
                    Some(&fi) => {
                        let f = &mut self.fragments.frags[fi as usize];
                        f.offset = offset;
                        f.clip = clip;
                        f.stamp = stamp;
                        fi
                    }
                    None => return false,
                }
            };

            if is_scroll {
                if !after {
                    // Bracket the content with marker fragments so the run can be
                    // replaced as one piece. The viewport's own primitives are
                    // fixed chrome (the optional scrollbar): an exit marker pushed
                    // before the children makes LIFO order emit them last, above
                    // content rather than beneath it.
                    let begin = if gather {
                        let m = self.new_frag(None, offset, clip);
                        self.fragments.frags[fi as usize].region = Some((m, u32::MAX));
                        m
                    } else {
                        match self.fragments.frags[fi as usize].region {
                            Some((m, _)) => m,
                            None => return false,
                        }
                    };
                    self.place_marker(begin, gather);
                    self.walk_stack.push(WalkItem {
                        after: true,
                        offset,
                        clip,
                        ..item
                    });
                } else {
                    let end = if gather {
                        let m = self.new_frag(None, offset, clip);
                        if let Some((_, e)) = self.fragments.frags[fi as usize].region.as_mut() {
                            *e = m;
                        }
                        m
                    } else {
                        match self.fragments.frags[fi as usize].region {
                            Some((_, m)) => m,
                            None => return false,
                        }
                    };
                    self.place_marker(end, gather);
                    let starts = [
                        self.chrome_scratch.len(),
                        self.glyph_scratch.len(),
                        self.image_scratch.len(),
                    ];
                    if let Some(paint) = scene.paint(id) {
                        encode_prims(
                            &paint.primitives,
                            offset,
                            clip,
                            &mut self.chrome_scratch,
                            &mut self.glyph_scratch,
                            &mut self.image_scratch,
                        );
                    }
                    self.seal_fragment(fi, starts, true, [false, true, true], gather);
                    if gather {
                        self.chrome_pending.push(fi);
                    }
                    continue;
                }
            } else {
                let starts = [
                    self.quad_scratch.len(),
                    self.glyph_scratch.len(),
                    self.image_scratch.len(),
                ];
                if !leaf_culled(scene, id, node, offset, clip) {
                    if let Some(paint) = scene.paint(id) {
                        encode_prims(
                            &paint.primitives,
                            offset,
                            clip,
                            &mut self.quad_scratch,
                            &mut self.glyph_scratch,
                            &mut self.image_scratch,
                        );
                    }
                }
                self.seal_fragment(fi, starts, false, [true; 3], gather);
            }

            // Compute the child (offset, clip): a Scroll node shifts and clips its
            // descendants; every other node passes its state through unchanged (§3.2).
            let (child_offset, child_clip) = if is_scroll {
                scroll_child_ctx(scene, id, offset, clip)
            } else {
                (offset, clip)
            };
            // Push children reversed so they pop in tree order (parent painted
            // first, children on top).
            if !self.push_children(
                fi,
                id,
                node,
                child_offset,
                child_clip,
                ctx,
                is_scroll,
                gather,
            ) {
                return false;
            }
        }
        true
    }
}
