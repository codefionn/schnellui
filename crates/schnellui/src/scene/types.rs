// # schnellui::scene
//
// The retained UI tree and its **ECS-split columns** (SOUL §4.4, §8.1). A single
// [`WidgetId`] slotmap is the primary tree; parallel [`slotmap::SecondaryMap`]
// columns hold layout / paint / a11y / binding data so that a relayout writes
// geometry without touching paint caches, and a repaint reads geometry without
// recomputing it.
//
// It also owns the **three orthogonal dirty channels** ([`DirtyFlags`]) as sparse
// per-node lists and epochs (SOUL §3.2, §8.1): a signal write flags only the
// channels it touches, and renderers patch only the nodes on those lists. There is
// no damage rectangle (see the `Scene` docs).
//
// Geometry ([`Rect`], [`Point`], [`Size`], [`LayoutBox`]) lives here — not in
// `schnellui::layout` — so the layout crate can write the layout column without a
// dependency cycle (layout → scene, never the reverse).

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use smallvec::SmallVec;

slotmap::new_key_type! {
    /// A retained-tree node id. By SOUL §6.2 this same id *is* the node's AccessKit
    /// id — pixels and semantics are two projections keyed identically.
    pub struct WidgetId;
}

static NEXT_COMPONENT_REF: AtomicU64 = AtomicU64::new(1);

/// A stable, copyable handle to a component across build order and remounts.
///
/// Unlike [`WidgetId`], this token is created by application code before mount.
/// Attach it with the widget/template `with_ref` wrapper, then reuse it in
/// responsive queries or resolve it through the mounted [`Scene`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ComponentRef(u64);

impl ComponentRef {
    /// Creates a new process-unique component reference.
    pub fn new() -> Self {
        Self(NEXT_COMPONENT_REF.fetch_add(1, Ordering::Relaxed))
    }

    /// Stable numeric identity for renderer adapters and diagnostic output.
    pub const fn id(self) -> u64 {
        self.0
    }
}

impl Default for ComponentRef {
    fn default() -> Self {
        Self::new()
    }
}

/// A 2D point in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A 2D size in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// An axis-aligned rectangle (logical pixels). Origin is top-left, `w`/`h` ≥ 0.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// A rect from origin + size.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// The empty rect (used as a damage accumulator identity, see [`Rect::union`]).
    pub const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    };

    /// `true` if the rect has no area.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    #[inline]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }
    #[inline]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// `true` if `p` is inside the rect (half-open on the far edges).
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// The smallest rect covering both inputs. Treats an empty rect as identity so
    /// it can seed a damage fold (SOUL §3.2 dirty-rect union).
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Rect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }

    /// The overlap of two rects, or an empty rect if they are disjoint.
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x || bottom <= y {
            Rect::ZERO
        } else {
            Rect {
                x,
                y,
                width: right - x,
                height: bottom - y,
            }
        }
    }
}

/// Straight (non-premultiplied) RGBA8 color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    pub const BLACK: Color = Color {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }
}

/// The three orthogonal dirty channels (SOUL §8.1). A signal write flags only the
/// channels it touched; the frame walks each channel over its dirty subtree only.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct DirtyFlags(u8);

impl DirtyFlags {
    pub const NONE: DirtyFlags = DirtyFlags(0);
    /// measured size changed → Taffy relayouts the smallest affected subtree.
    pub const LAYOUT: DirtyFlags = DirtyFlags(0b001);
    /// visual changed but not the box → re-raster one tile, no relayout.
    pub const PAINT: DirtyFlags = DirtyFlags(0b010);
    /// a semantic prop (name/value/state/focus) changed → one-node `TreeUpdate`.
    pub const A11Y: DirtyFlags = DirtyFlags(0b100);
    /// Structural change: geometry, pixels, and semantics may all differ.
    pub const ALL: DirtyFlags = DirtyFlags(0b111);

    #[inline]
    pub fn contains(self, f: DirtyFlags) -> bool {
        (self.0 & f.0) == f.0 && f.0 != 0
    }
    #[inline]
    pub fn insert(&mut self, f: DirtyFlags) {
        self.0 |= f.0;
    }
    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    #[inline]
    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

/// The dispatch tag for a retained node — enum dispatch, **never** `Box<dyn>`
/// churn (SOUL §4.4). Containers flow through layout; content leaves through paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WidgetKind {
    // containers (schnellui::layout §8.1)
    Row,
    Column,
    Stack,
    Grid,
    Scroll,
    Pad,
    Spacer,
    // content leaves (schnellui::widgets §8.1)
    Text,
    Button,
    Checkbox,
    Slider,
    TextInput,
    Image,
    Icon,
    // content leaves added for the widgets/charts groundwork (SOUL §8.1). Leaves,
    // not containers: they draw pixels + carry a role, so `is_container()` is false.
    ProgressBar,
    LoadingSpinner,
    Switch,
    Radio,
    Divider,
    Chart,
    // navigation/selection components (SOUL §8.1). `TabBar`/`List` are semantic
    // containers (like `Scroll`: geometry from children, a role of their own);
    // the rest are content leaves.
    TabBar,
    List,
    Link,
    Badge,
    Tab,
    ListItem,
    // the table component (SOUL §8.1): `Table` columns its rows, `TableRow` rows
    // its cells — both semantic containers; `TableCell` is the content leaf that
    // draws the cell surface + label and carries the Cell/ColumnHeader role.
    Table,
    TableRow,
    TableCell,
    // rich text (SOUL §8.1): `RichText` renders a formatted document (markdown /
    // code / open-document / plain) read-only; `TextArea` is the multi-line
    // source editor. Both content leaves — they draw pixels and carry a role.
    RichText,
    TextArea,
    /// A fixed-cell terminal surface. Its retained grid/model lives in
    /// `schnellui::widgets`; the scene stores only render-ready primitives.
    TerminalGrid,
    // the dropdown component (SOUL §8.1): `Dropdown` is the content leaf that
    // draws the collapsed trigger (value + caret) and carries the ComboBox role;
    // `DropdownOption` is one selectable entry of the open option list. The
    // widget wraps both in a plain `Column`, so no new container kind is needed.
    Dropdown,
    DropdownOption,
    // Dialogs are split into an overlay layer and a semantic surface. Both are
    // layout containers: the layer positions the surface and paints the optional
    // scrim; the surface pads/columns its content and carries the dialog role.
    DialogLayer,
    Dialog,
}

impl WidgetKind {
    /// `true` for layout containers (no pixels, no role of their own, §8.1).
    pub fn is_container(self) -> bool {
        matches!(
            self,
            WidgetKind::Row
                | WidgetKind::Column
                | WidgetKind::Stack
                | WidgetKind::Grid
                | WidgetKind::Scroll
                | WidgetKind::Pad
                | WidgetKind::Spacer
                | WidgetKind::TabBar
                | WidgetKind::List
                | WidgetKind::Table
                | WidgetKind::TableRow
                | WidgetKind::DialogLayer
                | WidgetKind::Dialog
        )
    }
}

/// One paint primitive emitted by a content widget and consumed by the backend
/// (SOUL §3.2). Solid quads and glyph quads are the two instanced families the
/// headless renderer draws (§7.2); a [`Primitive::Line`] rides the quad family as an
/// oriented (rotated) quad, so it needs no third pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Primitive {
    /// A rounded solid rectangle.
    SolidRect {
        rect: Rect,
        color: Color,
        corner_radius: f32,
    },
    /// One glyph quad: a destination rect sampling a sub-rect of the R8 glyph atlas.
    GlyphQuad {
        rect: Rect,
        /// atlas texel rect (x, y, w, h) into the shared glyph atlas.
        atlas_uv: Rect,
        color: Color,
    },
    /// A solid line segment from `from` to `to` with a stroke width, drawn by the GPU
    /// as an oriented quad (SOUL §3.2). Used by charts (line/sparkline series).
    Line {
        from: Point,
        to: Point,
        width: f32,
        color: Color,
    },
    /// One image quad: a destination rect sampling a sub-rect of the scene's shared
    /// **RGBA** [`ImageAtlas`] (SOUL §3.2). The third instanced family — rasterized
    /// images and CPU-rasterized vector (SVG) content both ride it. `tint`
    /// multiplies the sampled texel ([`Color::WHITE`] = as-authored).
    ImageQuad {
        rect: Rect,
        /// atlas texel rect (x, y, w, h) into the shared image atlas.
        atlas_uv: Rect,
        tint: Color,
    },
}

/// Computed geometry for one node — the **layout column** value (§8.1). Written by
/// `schnellui::layout`, read by paint; lives here to keep layout → scene acyclic.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutBox {
    /// final rect in the window coordinate space (post-composition of ancestors).
    pub rect: Rect,
    /// content rect (rect minus padding/border), for children placement.
    pub content: Rect,
}

/// The paint column value: the primitives a content node draws, refilled in place
/// on paint-dirty (grow-only `Vec`, cleared-and-refilled — §4.4).
#[derive(Clone, Debug, Default)]
pub struct PaintData {
    pub primitives: Vec<Primitive>,
}

/// An integer texel rect inside the [`ImageAtlas`] (SOUL §3.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TexelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl TexelRect {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// A stable identity for a reusable image resource.
///
/// Image-producing libraries use this key to intern equal rasters in a scene's
/// [`ImageAtlas`]. The textual fields make identity exact (rather than relying on
/// a possibly-colliding content hash), while `width` and `height` distinguish
/// physical raster sizes. Borrowed static strings make the common icon-library
/// path allocation-free apart from the atlas map entry itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImageCacheKey {
    pub namespace: Cow<'static, str>,
    pub resource: Cow<'static, str>,
    pub variant: Cow<'static, str>,
    pub width: u32,
    pub height: u32,
    /// Producer-selected pixel representation. Zero is ordinary RGBA.
    pub format: u8,
}

impl ImageCacheKey {
    pub fn new(
        namespace: impl Into<Cow<'static, str>>,
        resource: impl Into<Cow<'static, str>>,
        variant: impl Into<Cow<'static, str>>,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            resource: resource.into(),
            variant: variant.into(),
            width,
            height,
            format: 0,
        }
    }

    /// Distinguishes alternate pixel representations of the same resource.
    pub fn with_format(mut self, format: u8) -> Self {
        self.format = format;
        self
    }
}

/// The scene's shared CPU-side **RGBA8** image atlas (SOUL §3.2): rasterized images
/// and CPU-rasterized vector content are packed into one grow-only texture the
/// renderer keeps resident on the GPU. The texture never shrinks, but regions
/// given back with [`ImageAtlas::free`] are reused by later allocations. Owned by the [`Scene`] — image pixels are
/// retained scene *resources*, exactly like WebRender's resource cache — so the
/// render path needs no extra plumbing beyond `&Scene`.
///
/// **Allocation honesty (§4):** inserting an image is a *grow event* (mount-time
/// work, allowed to allocate); the steady-state re-render path never touches this.
/// The backing store starts empty (an imageless app pays zero bytes) and grows by
/// doubling up to [`ImageAtlas::MAX_DIM`], re-striding the existing pixels.
///
/// A monotonically increasing [`ImageAtlas::revision`] is bumped on every write and
/// the union of changed texels is retained for a sub-rectangle GPU upload. The
/// renderer therefore moves bytes proportional to changed image regions rather
/// than re-uploading the whole atlas.
#[derive(Clone, Debug)]
pub struct ImageAtlas {
    width: u32,
    height: u32,
    /// RGBA8, row-major, `width * height * 4` bytes.
    pixels: Vec<u8>,
    /// bumped on every pixel write; the renderer's staleness check (SOUL §3.2).
    revision: u64,
    /// Union of pixel writes since the renderer last synchronized this atlas.
    /// Interior mutability lets rendering consume the marker through `&Scene`,
    /// matching the existing immutable renderer interface.
    dirty: Cell<Option<TexelRect>>,
    /// Process-unique identity (a clone keeps it), so a renderer can tell a
    /// replaced atlas from a mutated one.
    id: u64,
    /// Recent `(revision, rect)` writes for per-renderer deltas, bounded to
    /// [`Self::WRITE_LOG_CAP`]. Unlike `dirty` it is never consumed.
    write_log: Vec<(u64, TexelRect)>,
    /// Revisions at or below this are not covered by `write_log` (a grow or an
    /// evicted entry): a consumer that old needs a full upload.
    log_floor: u64,
    /// simple shelf packer cursor (insert is a grow event, §4).
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    /// Resource identity → atlas allocation. Equal icon instances therefore
    /// share one CPU raster and one resident GPU atlas region.
    cached: HashMap<ImageCacheKey, TexelRect>,
    /// Regions given back through [`ImageAtlas::free`], cleared to transparent.
    /// Allocation takes from here before it opens new shelf space, so a widget
    /// that remounts with an image of the same size reuses its old region.
    free: Vec<TexelRect>,
}

static NEXT_IMAGE_ATLAS_ID: AtomicU64 = AtomicU64::new(1);

/// How much of an [`ImageAtlas`] a consumer must re-upload (see [`ImageAtlas::delta_since`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageDelta {
    Unchanged,
    Rect(TexelRect),
    /// Re-upload the whole atlas (grown, evicted from the log, or a different atlas).
    Full,
}

fn texel_union(a: TexelRect, b: TexelRect) -> TexelRect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    let right = a.x.saturating_add(a.width).max(b.x.saturating_add(b.width));
    let bottom =
        a.y.saturating_add(a.height)
            .max(b.y.saturating_add(b.height));
    TexelRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    }
}

impl Default for ImageAtlas {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl ImageAtlas {
    /// The hard cap on either atlas dimension (a conservative floor of every wgpu
    /// backend's guaranteed 2D texture limit).
    pub const MAX_DIM: u32 = 4096;
    /// The first non-empty backing size.
    const INITIAL_DIM: u32 = 256;
    /// Writes retained for [`ImageAtlas::delta_since`].
    const WRITE_LOG_CAP: usize = 64;

    /// An empty atlas: zero dimensions, zero bytes (an imageless scene pays nothing).
    pub fn new_empty() -> ImageAtlas {
        ImageAtlas {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            revision: 0,
            dirty: Cell::new(None),
            id: NEXT_IMAGE_ATLAS_ID.fetch_add(1, Ordering::Relaxed),
            write_log: Vec::new(),
            log_floor: 0,
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            cached: HashMap::new(),
            free: Vec::new(),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    /// `true` before the first insert (nothing to upload).
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
    /// The raw RGBA8 buffer (row-major, `width * height * 4` bytes).
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
    /// The write revision — bumped on every insert (including the grow-copy). The
    /// renderer re-uploads when this differs from its GPU copy's revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Process-unique identity of this atlas (shared by clones).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// What a consumer that last uploaded `revision` must re-upload to match this
    /// atlas. Reads the non-consuming write log, so several renderers can each
    /// synchronize independently of [`ImageAtlas::take_dirty`].
    pub fn delta_since(&self, revision: u64) -> ImageDelta {
        if revision == self.revision {
            return ImageDelta::Unchanged;
        }
        if revision < self.log_floor || revision > self.revision {
            return ImageDelta::Full;
        }
        let mut union: Option<TexelRect> = None;
        for (rev, rect) in &self.write_log {
            if *rev > revision {
                union = Some(union.map_or(*rect, |u| texel_union(u, *rect)));
            }
        }
        union.map_or(ImageDelta::Unchanged, ImageDelta::Rect)
    }

    fn log_write(&mut self, rect: TexelRect) {
        // Reserve the whole bounded log on first use, so a stream of frame
        // writes (a browser panel) never pays the doubling reallocations.
        if self.write_log.capacity() < Self::WRITE_LOG_CAP {
            self.write_log
                .reserve_exact(Self::WRITE_LOG_CAP - self.write_log.len());
        }
        if self.write_log.len() == Self::WRITE_LOG_CAP {
            self.log_floor = self.write_log.remove(0).0;
        }
        self.write_log.push((self.revision, rect));
    }

    /// Consumes the union of changed texels since the prior GPU synchronization.
    pub fn take_dirty(&self) -> Option<TexelRect> {
        self.dirty.take()
    }

    fn mark_dirty(&self, rect: TexelRect) {
        let dirty = self.dirty.get().map_or(rect, |current| {
            let x = current.x.min(rect.x);
            let y = current.y.min(rect.y);
            let right = current
                .x
                .saturating_add(current.width)
                .max(rect.x.saturating_add(rect.width));
            let bottom = current
                .y
                .saturating_add(current.height)
                .max(rect.y.saturating_add(rect.height));
            TexelRect {
                x,
                y,
                width: right - x,
                height: bottom - y,
            }
        });
        self.dirty.set(Some(dirty));
    }

    /// Returns the existing atlas allocation for a cached resource.
    pub fn cached(&self, key: &ImageCacheKey) -> Option<TexelRect> {
        self.cached.get(key).copied()
    }

    /// Number of unique cached resources resident in this atlas.
    pub fn cached_len(&self) -> usize {
        self.cached.len()
    }

    /// Reserves an atlas region for `key`, or returns its existing allocation.
    ///
    /// The boolean is `true` only when a new transparent region was allocated.
    /// Callers use it to ensure only one raster job is submitted when the same
    /// icon appears repeatedly in a tree.
    pub fn reserve_cached(
        &mut self,
        key: ImageCacheKey,
        w: u32,
        h: u32,
    ) -> Option<(TexelRect, bool)> {
        if let Some(rect) = self.cached(&key) {
            return Some((rect, false));
        }
        let rect = self.reserve(w, h)?;
        self.cached.insert(key, rect);
        Some((rect, true))
    }

    /// Inserts pixels for `key`, reusing an existing allocation when present.
    ///
    /// This is the synchronous/cache-hit counterpart of [`Self::reserve_cached`].
    pub fn insert_cached(
        &mut self,
        key: ImageCacheKey,
        w: u32,
        h: u32,
        rgba: &[u8],
    ) -> Option<(TexelRect, bool)> {
        if let Some(rect) = self.cached(&key) {
            return Some((rect, false));
        }
        let rect = self.insert(w, h, rgba)?;
        self.cached.insert(key, rect);
        Some((rect, true))
    }

    /// Inserts a `w×h` RGBA8 image (`rgba` is row-major, `w * h * 4` bytes),
    /// returning its texel rect, or `None` when the image can never fit
    /// ([`ImageAtlas::MAX_DIM`]) or `rgba` is short. Grows the backing store as
    /// needed — a mount-time grow event (§4), never steady-state work.
    pub fn insert(&mut self, w: u32, h: u32, rgba: &[u8]) -> Option<TexelRect> {
        if w == 0 || h == 0 || w > Self::MAX_DIM || h > Self::MAX_DIM {
            return None;
        }
        if rgba.len() < (w as usize) * (h as usize) * 4 {
            return None;
        }
        let rect = loop {
            if let Some(r) = self.allocate(w, h) {
                break r;
            }
            if !self.grow(w, h) {
                return None;
            }
        };
        // copy rows into the strided backing store
        let stride = self.width as usize * 4;
        let row_bytes = w as usize * 4;
        for row in 0..h as usize {
            let dst = (rect.y as usize + row) * stride + rect.x as usize * 4;
            let src = row * row_bytes;
            self.pixels[dst..dst + row_bytes].copy_from_slice(&rgba[src..src + row_bytes]);
        }
        self.revision += 1;
        self.mark_dirty(rect);
        self.log_write(rect);
        Some(rect)
    }

    /// Reserves a `w×h` rect **without pixels** — the async image pipeline's
    /// placeholder (SOUL §8.1): the widget's quad gets its final texel rect at
    /// build (layout and UVs never move), the region stays transparent (new shelf
    /// space is zeroed and [`ImageAtlas::free`] clears what it gives back), and the rasterized
    /// pixels land later via [`ImageAtlas::write_rect`]. Grows like
    /// [`ImageAtlas::insert`] (a mount-time grow event, §4); a pure reservation
    /// changes no texels, so only a grow bumps the revision.
    pub fn reserve(&mut self, w: u32, h: u32) -> Option<TexelRect> {
        if w == 0 || h == 0 || w > Self::MAX_DIM || h > Self::MAX_DIM {
            return None;
        }
        loop {
            if let Some(r) = self.allocate(w, h) {
                return Some(r);
            }
            if !self.grow(w, h) {
                return None;
            }
        }
    }

    /// Writes RGBA8 pixels into a previously reserved/inserted rect and bumps the
    /// revision (the renderer re-uploads on mismatch, SOUL §3.2). Returns `false` —
    /// writing nothing — when the rect falls outside the atlas or `rgba` is short.
    pub fn write_rect(&mut self, rect: TexelRect, rgba: &[u8]) -> bool {
        self.write_rect_strided(rect, rgba, rect.width)
    }

    /// Like [`ImageAtlas::write_rect`], but reads `rgba` as rows of `src_width`
    /// pixels and copies the top-left `rect.width x rect.height` block. A caller
    /// can therefore show the visible crop of a larger source image without first
    /// repacking it. Returns `false` when `src_width < rect.width`.
    pub fn write_rect_strided(&mut self, rect: TexelRect, rgba: &[u8], src_width: u32) -> bool {
        if rect.width == 0 || rect.height == 0 || src_width < rect.width {
            return false;
        }
        if rect.x.saturating_add(rect.width) > self.width
            || rect.y.saturating_add(rect.height) > self.height
        {
            return false;
        }
        let src_stride = src_width as usize * 4;
        let row_bytes = rect.width as usize * 4;
        if rgba.len() < (rect.height as usize - 1) * src_stride + row_bytes {
            return false;
        }
        let stride = self.width as usize * 4;
        for row in 0..rect.height as usize {
            let dst = (rect.y as usize + row) * stride + rect.x as usize * 4;
            let src = row * src_stride;
            self.pixels[dst..dst + row_bytes].copy_from_slice(&rgba[src..src + row_bytes]);
        }
        self.revision += 1;
        self.mark_dirty(rect);
        self.log_write(rect);
        true
    }

    /// Gives `rect` back for later allocations, for example when the widget
    /// that owned it unmounts. The region is cleared to transparent, so a later
    /// [`ImageAtlas::reserve`] still samples nothing until its pixels land. A
    /// cache entry pointing at `rect` is dropped with it. Freeing a rect twice,
    /// or one this atlas never handed out, is a caller bug; rects outside the
    /// atlas are ignored.
    pub fn free(&mut self, rect: TexelRect) {
        if rect.is_empty()
            || rect.x.saturating_add(rect.width) > self.width
            || rect.y.saturating_add(rect.height) > self.height
        {
            return;
        }
        let stride = self.width as usize * 4;
        let row_bytes = rect.width as usize * 4;
        for row in 0..rect.height as usize {
            let at = (rect.y as usize + row) * stride + rect.x as usize * 4;
            self.pixels[at..at + row_bytes].fill(0);
        }
        self.revision += 1;
        self.mark_dirty(rect);
        self.log_write(rect);
        self.cached.retain(|_, cached| *cached != rect);
        // Merge with free neighbours that share a full edge, so freeing the
        // pieces of a split region gives the whole region back.
        let mut merged = rect;
        while let Some(index) = self.free.iter().position(|other| {
            (other.x == merged.x
                && other.width == merged.width
                && (other.y + other.height == merged.y || merged.y + merged.height == other.y))
                || (other.y == merged.y
                    && other.height == merged.height
                    && (other.x + other.width == merged.x || merged.x + merged.width == other.x))
        }) {
            let other = self.free.swap_remove(index);
            merged = texel_union(merged, other);
        }
        self.free.push(merged);
    }

    /// Texels currently free for reuse, for tests and diagnostics.
    pub fn free_texels(&self) -> u64 {
        self.free
            .iter()
            .map(|rect| u64::from(rect.width) * u64::from(rect.height))
            .sum()
    }

    /// Takes the smallest freed region that holds `w×h` and returns what is
    /// left of it to the free list.
    fn allocate_freed(&mut self, w: u32, h: u32) -> Option<TexelRect> {
        let index = self
            .free
            .iter()
            .enumerate()
            .filter(|(_, rect)| rect.width >= w && rect.height >= h)
            .min_by_key(|(_, rect)| u64::from(rect.width) * u64::from(rect.height))
            .map(|(index, _)| index)?;
        let rect = self.free.swap_remove(index);
        // Guillotine split: the strip right of the allocation, then the full
        // width below it.
        if rect.width > w {
            self.free.push(TexelRect {
                x: rect.x + w,
                y: rect.y,
                width: rect.width - w,
                height: h,
            });
        }
        if rect.height > h {
            self.free.push(TexelRect {
                x: rect.x,
                y: rect.y + h,
                width: rect.width,
                height: rect.height - h,
            });
        }
        Some(TexelRect {
            x: rect.x,
            y: rect.y,
            width: w,
            height: h,
        })
    }

    /// One allocation attempt: a freed region first, then the shelf packer
    /// (the same scheme as the glyph atlas).
    fn allocate(&mut self, w: u32, h: u32) -> Option<TexelRect> {
        if let Some(rect) = self.allocate_freed(w, h) {
            return Some(rect);
        }
        if self.width == 0 || w > self.width {
            return None;
        }
        if self.shelf_x + w > self.width {
            self.shelf_y += self.shelf_h;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }
        if self.shelf_y + h > self.height {
            return None;
        }
        let rect = TexelRect {
            x: self.shelf_x,
            y: self.shelf_y,
            width: w,
            height: h,
        };
        self.shelf_x += w;
        self.shelf_h = self.shelf_h.max(h);
        Some(rect)
    }

    /// Grows the backing store so a `w×h` image can eventually fit: dimensions
    /// double (from [`ImageAtlas::INITIAL_DIM`]) up to [`ImageAtlas::MAX_DIM`],
    /// re-striding the existing pixels into the new buffer. Returns `false` when
    /// already at the cap. A grow event (§4).
    fn grow(&mut self, need_w: u32, need_h: u32) -> bool {
        if self.width >= Self::MAX_DIM && self.height >= Self::MAX_DIM {
            return false;
        }
        let mut nw = self.width.max(Self::INITIAL_DIM);
        let mut nh = self.height.max(Self::INITIAL_DIM);
        while nw < need_w {
            nw = (nw * 2).min(Self::MAX_DIM);
        }
        while nh < need_h {
            nh = (nh * 2).min(Self::MAX_DIM);
        }
        if nw == self.width && nh == self.height {
            // dimensions already fit the image; grow capacity instead
            if nh < Self::MAX_DIM {
                nh = (nh * 2).min(Self::MAX_DIM);
            } else if nw < Self::MAX_DIM {
                nw = (nw * 2).min(Self::MAX_DIM);
            } else {
                return false;
            }
        }
        let mut pixels = vec![0u8; (nw as usize) * (nh as usize) * 4];
        // re-stride the old content (top-left anchored, so texel rects stay valid)
        let old_stride = self.width as usize * 4;
        let new_stride = nw as usize * 4;
        for row in 0..self.height as usize {
            let src = row * old_stride;
            let dst = row * new_stride;
            pixels[dst..dst + old_stride].copy_from_slice(&self.pixels[src..src + old_stride]);
        }
        self.pixels = pixels;
        self.width = nw;
        self.height = nh;
        self.revision += 1;
        // A grow re-strides every texel: older consumers need a full upload.
        self.write_log.clear();
        self.log_floor = self.revision;
        self.dirty.set(Some(TexelRect {
            x: 0,
            y: 0,
            width: nw,
            height: nh,
        }));
        true
    }
}

/// The a11y column value (mirrors SOUL §6.1). The full AccessKit node is assembled
/// by `schnellui::a11y`; this is the retained-side source of truth it reads.
#[derive(Clone, Debug, Default)]
pub struct A11yData {
    /// AccessKit role discriminant (kept as a small tag to avoid a scene→accesskit
    /// dep; `schnellui::a11y` maps it to `accesskit::Role`).
    pub role: u16,
    pub name: Option<String>,
    pub value: Option<String>,
    /// packed state bits (checked/disabled/expanded/selected/focused).
    pub state: u32,
    /// packed supported-action bits (Click/Focus/SetValue/Increment/…).
    pub actions: u32,
    /// Sort-direction tag for sortable column headers: `0` = not currently
    /// sorted, `1` = ascending, `2` = descending. Kept as a small tag so the
    /// scene stays independent of AccessKit.
    pub sort_direction: u8,
}

/// One retained-tree node: identity + structure + dispatch tag (SOUL §8.1). All
/// heavy per-node data lives in the columns, not here (ECS split, §4.4).
#[derive(Clone, Debug)]
pub struct WidgetNode {
    pub kind: WidgetKind,
    pub parent: Option<WidgetId>,
    /// inline the common small-child-count case (§4.4 smallvec).
    pub children: SmallVec<[WidgetId; 4]>,
}

/// Structural information returned when a retained subtree is detached.
///
/// `nodes` is parent-before-children and includes the removed root. The original
/// parent and child index let a caller build a replacement at the exact same
/// structural position without reconstructing unaffected siblings.
#[derive(Debug)]
pub struct RemovedSubtree {
    pub parent: Option<WidgetId>,
    pub child_index: usize,
    pub nodes: Vec<WidgetId>,
}

// The retained scene: the primary tree plus the ECS columns and the dirty
// bookkeeping (SOUL §3.2, §8.1). Long-lived; hot paths `clear()`-and-refill to
// retain capacity (§4.4), never reallocate in steady state.
//
// Remaining retained-scene work (SOUL section 3.2):
// - Content-addressed primitive interning and epoch garbage collection.
//   PaintData still owns its primitives.
// - Cache slices grouped by update frequency. The renderer already caches a
//   picture in independently fingerprinted tiles, but changed frames scan
//   instance dependencies and clear a full scratch attachment.
// - Opacity/affine properties and compositor-only scroll reuse. Translation
//   and clip properties inherit through parent links today, without relayout,
//   but property edits still update instance geometry before tile raster.

/// Inherited compositing state. Translation is relative to the parent property
/// node. Clip coordinates use the same layout space as paint primitives and are
/// translated by the accumulated translation before intersecting ancestor clips.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CompositeProperties {
    pub translation: Point,
    pub clip: Option<Rect>,
}
