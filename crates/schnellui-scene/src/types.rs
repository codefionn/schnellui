// # schnellui-scene
//
// The retained UI tree and its **ECS-split columns** (SOUL §4.4, §8.1). A single
// [`WidgetId`] slotmap is the primary tree; parallel [`slotmap::SecondaryMap`]
// columns hold layout / paint / a11y / binding data so that a relayout writes
// geometry without touching paint caches, and a repaint reads geometry without
// recomputing it.
//
// It also owns the **three orthogonal dirty channels** ([`DirtyFlags`]) and the
// **damage region** (SOUL §3.2, §8.1): a signal write flags only the channels it
// touches, and only the union of invalidated rects is uploaded/repainted.
//
// Geometry ([`Rect`], [`Point`], [`Size`], [`LayoutBox`]) lives here — not in
// `schnellui-layout` — so the layout crate can write the layout column without a
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
    // containers (schnellui-layout §8.1)
    Row,
    Column,
    Stack,
    Grid,
    Scroll,
    Pad,
    Spacer,
    // content leaves (schnellui-widgets §8.1)
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
    /// `schnellui-widgets`; the scene stores only render-ready primitives.
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
/// `schnellui-layout`, read by paint; lives here to keep layout → scene acyclic.
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
/// renderer keeps resident on the GPU. Owned by the [`Scene`] — image pixels are
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
    /// simple shelf packer cursor (insert is a grow event, §4).
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    /// Resource identity → atlas allocation. Equal icon instances therefore
    /// share one CPU raster and one resident GPU atlas region.
    cached: HashMap<ImageCacheKey, TexelRect>,
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

    /// An empty atlas: zero dimensions, zero bytes (an imageless scene pays nothing).
    pub fn new_empty() -> ImageAtlas {
        ImageAtlas {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            revision: 0,
            dirty: Cell::new(None),
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            cached: HashMap::new(),
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
        Some(rect)
    }

    /// Reserves a `w×h` rect **without pixels** — the async image pipeline's
    /// placeholder (SOUL §8.1): the widget's quad gets its final texel rect at
    /// build (layout and UVs never move), the region stays transparent (the shelf
    /// packer never reuses space, so it is always zeroed), and the rasterized
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
        if rect.width == 0 || rect.height == 0 {
            return false;
        }
        if rect.x.saturating_add(rect.width) > self.width
            || rect.y.saturating_add(rect.height) > self.height
        {
            return false;
        }
        if rgba.len() < (rect.width as usize) * (rect.height as usize) * 4 {
            return false;
        }
        let stride = self.width as usize * 4;
        let row_bytes = rect.width as usize * 4;
        for row in 0..rect.height as usize {
            let dst = (rect.y as usize + row) * stride + rect.x as usize * 4;
            let src = row * row_bytes;
            self.pixels[dst..dst + row_bytes].copy_from_slice(&rgba[src..src + row_bytes]);
        }
        self.revision += 1;
        self.mark_dirty(rect);
        true
    }

    /// One shelf-packer allocation attempt (the same scheme as the glyph atlas).
    fn allocate(&mut self, w: u32, h: u32) -> Option<TexelRect> {
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
/// by `schnellui-a11y`; this is the retained-side source of truth it reads.
#[derive(Clone, Debug, Default)]
pub struct A11yData {
    /// AccessKit role discriminant (kept as a small tag to avoid a scene→accesskit
    /// dep; `schnellui-a11y` maps it to `accesskit::Role`).
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

// The retained scene: the primary tree plus the ECS columns and the dirty/damage
// bookkeeping (SOUL §3.2, §8.1). Long-lived; hot paths `clear()`-and-refill to
// retain capacity (§4.4), never reallocate in steady state.
//
// # Deferred past v0 (SOUL §3.2)
// The v0 scene is a plain retained tree + per-node paint fragments + a single
// window-level damage rect. The WebRender-grade "little change → little work"
// machinery is intentionally **not** here yet — each is a doc-TODO, not a stub:
// - **TODO(SOUL §3.2): content-addressed interning + epoch GC** — dedup unchanged
//   primitives so they are never re-hashed/re-uploaded/rebuilt. Today
//   [`PaintData`] owns its primitives outright.
// - **TODO(SOUL §3.2): tile / picture cache** — slice the scene by update
//   frequency, tile each slice, give each tile a dependency fingerprint, and let
//   the union of invalidated tiles (not one coarse rect) be the frame's dirty
//   region. Today [`Scene::damage`] is a single bounding rect.
// - **TODO(SOUL §3.2): property / transform trees** — route scroll/translate/
//   opacity to a mutate-and-recomposite path that dirties *neither* paint nor
//   layout (§8.1). The v0 stand-in is the [`Scene::set_scroll_offset`] column: a
//   `Scroll` node's offset drives a renderer-side recomposite of its children and
//   marks **paint-dirty only** (never layout), but the full property tree — nested
//   transforms, opacity, a dedicated recomposite pass that skips paint too — is
//   still deferred. Translates other than scroll still fold into [`Scene::set_rect`].
