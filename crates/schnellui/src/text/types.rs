// # schnellui::text
//
// Shaping / measurement via **Parley + Fontique** and glyph rasterization via
// **swash** into a CPU-side **R8 glyph atlas with dirty sub-rects** (SOUL §8,
// §8.1). Widgets feed the measured metrics up to layout as their intrinsic size;
// the renderer uploads only the atlas dirty sub-rect via `write_texture`
// (SOUL §3.2 — `text_edit` is a budgeted grow-event, §4.1).
//
// The fonts are **embedded** (`include_bytes!`) for deterministic screenshots
// (SOUL §7.3) — the Liberation family (SIL OFL 1.1, embeddable; see the license
// note on [`EMBEDDED_FONT`]): Sans in four weights/styles for UI + rich-text
// emphasis, and Mono in two weights for code (SOUL §8.1). Uniform runs pick a
// face via [`ShapeOptions::face`]; mixed-style runs shape per-span through
// [`TextShaper::shape_spans`].
//
// ## Amortized zero-alloc (SOUL §4.1)
//
// [`TextShaper`] owns the pooled Parley `FontContext` + `LayoutContext`, a reused
// `Layout`, a swash `ScaleContext` + scratch `Image`, and a rasterized-glyph
// cache. Warm shaping re-uses those allocations (the `text_edit` budget), and a
// repeated glyph is a cache hit that touches neither the atlas nor the heap.

use rustc_hash::FxHashMap;

use smallvec::SmallVec;

use parley::FontData;

/// The brush type of our layouts. We don't paint through Parley (the GPU
/// renderer emits glyph quads itself), so a trivial RGBA brush suffices.
pub(crate) type Brush = [u8; 4];

/// The embedded, deterministic UI font (SOUL §7.3).
///
/// **Liberation Sans**, Copyright the Liberation project — licensed under the
/// **SIL Open Font License, Version 1.1**, which permits embedding in software.
/// Bundled at `assets/LiberationSans-Regular.ttf`.
pub static EMBEDDED_FONT: &[u8] = include_bytes!("assets/LiberationSans-Regular.ttf");

/// The additional embedded faces backing rich text (bold/italic emphasis and
/// monospace code — SOUL §7.3 determinism holds because every face ships in the
/// binary).
pub static EMBEDDED_FONT_BOLD: &[u8] = include_bytes!("assets/LiberationSans-Bold.ttf");
pub static EMBEDDED_FONT_ITALIC: &[u8] = include_bytes!("assets/LiberationSans-Italic.ttf");
pub static EMBEDDED_FONT_BOLD_ITALIC: &[u8] =
    include_bytes!("assets/LiberationSans-BoldItalic.ttf");
pub static EMBEDDED_FONT_MONO: &[u8] = include_bytes!("assets/LiberationMono-Regular.ttf");
pub static EMBEDDED_FONT_MONO_BOLD: &[u8] = include_bytes!("assets/LiberationMono-Bold.ttf");
/// Nerd Fonts 3.4 Symbols Mono fallback. This covers the extended private-use
/// glyph set used by modern prompts without depending on host-installed fonts.
/// See `assets/SymbolsNerdFont-LICENSE.txt`.
pub static EMBEDDED_FONT_NERD_SYMBOLS: &[u8] =
    include_bytes!("assets/SymbolsNerdFontMono-Regular.ttf");
/// DejaVu Sans fallback for standard Unicode symbols that prompt themes
/// commonly use outside the Nerd Fonts private-use ranges.
/// See `assets/DejaVuSans-LICENSE.txt`.
pub static EMBEDDED_FONT_UNICODE_SYMBOLS: &[u8] = include_bytes!("assets/DejaVuSans-Symbols.ttf");

/// A font handle within the shaper's Fontique collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct FontId(pub u32);

/// The [`FontId`] of the embedded Liberation Sans font (SOUL §7.3). The shaper
/// registers this face first, so plain UI text carries this id.
pub const EMBEDDED_FONT_ID: FontId = FontId(0);
/// Stable rasterization identity of the bundled Nerd Fonts Symbols fallback: the
/// slot right after the six [`FontFace`] faces.
pub const NERD_SYMBOLS_FONT_ID: FontId = FontId(FontFace::MonoBold as u32 + 1);
/// Stable rasterization identity of the bundled standard Unicode symbol fallback.
pub const UNICODE_SYMBOLS_FONT_ID: FontId = FontId(FontFace::MonoBold as u32 + 2);

/// The embedded faces in registration order. `BaseFonts::build` registers them
/// in exactly this order and asserts each lands on the [`FontId`] with its index,
/// which is what [`FontFace::font_id`] and the two symbol-font ids above assume.
pub(crate) static EMBEDDED_FACES: [&[u8]; 8] = [
    EMBEDDED_FONT,
    EMBEDDED_FONT_BOLD,
    EMBEDDED_FONT_ITALIC,
    EMBEDDED_FONT_BOLD_ITALIC,
    EMBEDDED_FONT_MONO,
    EMBEDDED_FONT_MONO_BOLD,
    EMBEDDED_FONT_NERD_SYMBOLS,
    EMBEDDED_FONT_UNICODE_SYMBOLS,
];

/// One of the embedded typefaces (SOUL §7.3 — all faces ship in the binary, so
/// rich text stays deterministic). `Sans` is the UI default; the emphasis and
/// mono faces exist for rich text / code (SOUL §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FontFace {
    #[default]
    Sans,
    SansBold,
    SansItalic,
    SansBoldItalic,
    Mono,
    MonoBold,
}

impl FontFace {
    /// The stable [`FontId`] of this face (its rasterization identity — part of
    /// every [`GlyphKey`], so the same glyph id in two faces never aliases).
    #[inline]
    pub const fn font_id(self) -> FontId {
        FontId(self as u32)
    }

    /// Bold variant of this face's family (used for emphasis composition).
    #[inline]
    pub const fn bold(self) -> bool {
        matches!(
            self,
            FontFace::SansBold | FontFace::SansBoldItalic | FontFace::MonoBold
        )
    }

    /// Italic variant (the mono family is upright-only in the embedded set).
    #[inline]
    pub const fn italic(self) -> bool {
        matches!(self, FontFace::SansItalic | FontFace::SansBoldItalic)
    }

    /// Monospace family?
    #[inline]
    pub const fn mono(self) -> bool {
        matches!(self, FontFace::Mono | FontFace::MonoBold)
    }

    /// Compose a face from style axes (italic is dropped for mono — the
    /// embedded set has no mono-italic; color carries that distinction).
    pub const fn from_axes(bold: bool, italic: bool, mono: bool) -> FontFace {
        match (mono, bold, italic) {
            (true, true, _) => FontFace::MonoBold,
            (true, false, _) => FontFace::Mono,
            (false, true, true) => FontFace::SansBoldItalic,
            (false, true, false) => FontFace::SansBold,
            (false, false, true) => FontFace::SansItalic,
            (false, false, false) => FontFace::Sans,
        }
    }
}

pub(crate) fn font_id_for_data(
    font: &FontData,
    font_ids: &mut FxHashMap<(u64, u32), FontId>,
    font_resources: &mut Vec<FontData>,
) -> FontId {
    let key = (font.data.id(), font.index);
    if let Some(id) = font_ids.get(&key) {
        return *id;
    }
    let id = FontId(font_resources.len() as u32);
    font_resources.push(font.clone());
    font_ids.insert(key, id);
    id
}

/// The identity of a rasterized glyph — the atlas cache key (SOUL §8.1). Size and
/// subpixel bucket are part of the key so the same glyph at two sizes are distinct
/// atlas entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: FontId,
    pub glyph_id: u16,
    /// integer pixel size (subpixel AA off for determinism, §7.3).
    pub size_px: u32,
    /// horizontal subpixel bucket [0,3]; pinned to 0 for deterministic shots.
    pub subpixel: u8,
}

/// An integer texel rectangle inside the glyph atlas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AtlasRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl AtlasRect {
    #[inline]
    pub fn right(&self) -> u32 {
        self.x + self.width
    }
    #[inline]
    pub fn bottom(&self) -> u32 {
        self.y + self.height
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
    /// Smallest rect covering both (empty acts as identity — damage fold, §3.2).
    pub fn union(&self, other: &AtlasRect) -> AtlasRect {
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
        AtlasRect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
}

/// One positioned glyph from shaping (SOUL §8.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub glyph_id: u16,
    /// The resolved font this glyph rasterizes in. This may differ from the
    /// requested run face when Fontique selected a fallback family.
    pub font: FontId,
    /// pen advance after this glyph, logical px.
    pub x_advance: f32,
    /// glyph origin offset from the pen, logical px.
    pub x_offset: f32,
    pub y_offset: f32,
    /// byte index in the source string (for hit-testing / editing).
    pub cluster: u32,
}

/// How text breaks when a `max_width` constraint is supplied (SOUL §8.1). Maps
/// onto parley 0.11's line-breaking style properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// Never wrap: a single line regardless of `max_width` (parley
    /// `TextWrapMode::NoWrap`; no soft-break opportunities are taken).
    NoWrap,
    /// Wrap at soft-break opportunities (spaces / word boundaries) only, never
    /// mid-word. This is parley's default (`TextWrapMode::Wrap` +
    /// `OverflowWrap::Normal`) and matches the legacy [`TextShaper::shape`].
    #[default]
    Word,
    /// Word wrap plus emergency char-level breaks inside otherwise unbreakable
    /// runs (CJK, long URLs) — parley `OverflowWrap::Anywhere`.
    Anywhere,
}

/// Per-line horizontal alignment within the wrap width (SOUL §8.1). Maps onto
/// parley 0.11's [`parley::Alignment`], applied via `Layout::align`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextAlign {
    /// Left edge for LTR text (parley `Alignment::Start`).
    #[default]
    Start,
    /// Centered within the wrap width (parley `Alignment::Center`).
    Center,
    /// Right edge for LTR text (parley `Alignment::End`).
    End,
    /// Justify each non-last line to the wrap width by adjusting cluster
    /// advances (parley `Alignment::Justify`). Only has a visible effect with a
    /// finite `max_width` **and** more than one line; the last line stays
    /// `Start`-aligned. Because it mutates advances in place through the pooled
    /// `Layout` (parley calls `unjustify` before each re-break/re-align), it
    /// stays on the amortized-zero-alloc warm path like the other modes.
    Justify,
}

/// Parameters for a wrapped/aligned shape call (SOUL §8.1). A small `Copy`
/// struct param rather than a builder — construct with [`ShapeOptions::new`] and
/// the chainable setters, or with a struct literal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeOptions {
    /// Font size in (physical) px — same convention as [`TextShaper::shape`].
    pub size_px: f32,
    /// Wrap width in logical px; `None` = unconstrained (single line for any
    /// [`WrapMode`], since there is no width to break against).
    pub max_width: Option<f32>,
    pub wrap: WrapMode,
    pub align: TextAlign,
    /// The typeface the whole run is shaped in ([`FontFace::Sans`] by default —
    /// rich text picks emphasis/mono faces per span via [`SpanSpec`] instead).
    pub face: FontFace,
}

impl ShapeOptions {
    /// Defaults matching the legacy [`TextShaper::shape`]: [`WrapMode::Word`],
    /// [`TextAlign::Start`], no width constraint.
    pub fn new(size_px: f32) -> ShapeOptions {
        ShapeOptions {
            size_px,
            max_width: None,
            wrap: WrapMode::default(),
            align: TextAlign::default(),
            face: FontFace::default(),
        }
    }
    #[inline]
    pub fn max_width(mut self, max_width: Option<f32>) -> ShapeOptions {
        self.max_width = max_width;
        self
    }
    #[inline]
    pub fn wrap(mut self, wrap: WrapMode) -> ShapeOptions {
        self.wrap = wrap;
        self
    }
    #[inline]
    pub fn align(mut self, align: TextAlign) -> ShapeOptions {
        self.align = align;
        self
    }
    #[inline]
    pub fn face(mut self, face: FontFace) -> ShapeOptions {
        self.face = face;
        self
    }
}

/// One styled span of a rich shape call ([`TextShaper::shape_spans`], SOUL §8.1).
/// Spans tile the source string in order: each covers the next `len` **bytes**
/// (must end on `char` boundaries). Color is an RGBA8 brush carried through
/// parley and handed back per run — the text crate stores it, never paints it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanSpec {
    /// Byte length of this span in the source string.
    pub len: usize,
    pub face: FontFace,
    /// Straight (non-premultiplied) RGBA8.
    pub color: [u8; 4],
    pub underline: bool,
    pub strikethrough: bool,
}

/// One positioned glyph of a rich (multi-style) shape (SOUL §8.1). Unlike
/// [`ShapedGlyph`], positions are **absolute** within the layout (alignment and
/// line origin already applied): `x` is the pen origin, `y` the baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RichGlyph {
    pub glyph_id: u16,
    /// Pen origin, absolute from the layout's left edge, logical px.
    pub x: f32,
    /// Baseline, absolute from the layout's top edge, logical px.
    pub y: f32,
    /// The face this glyph rasterizes in (part of its [`GlyphKey`]).
    pub font: FontId,
    /// The span's RGBA8 brush.
    pub color: [u8; 4],
}

/// A resolved underline/strikethrough segment of a rich shape — the renderer
/// draws it as a hairline; offsets follow the run's font metrics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RichDecor {
    /// Left edge, absolute, logical px.
    pub x: f32,
    /// Top of the decoration stroke, absolute, logical px.
    pub y: f32,
    pub width: f32,
    pub thickness: f32,
    pub color: [u8; 4],
}

/// A shaped multi-style (rich) run: absolute-positioned glyphs + decorations +
/// per-line boxes (SOUL §8.1). `lines` reuses [`ShapedLine`]; its
/// `glyph_start`/`glyph_count` index into `glyphs`.
#[derive(Clone, Debug, Default)]
pub struct RichShapedText {
    pub glyphs: Vec<RichGlyph>,
    pub decors: Vec<RichDecor>,
    pub lines: SmallVec<[ShapedLine; 4]>,
    /// Widest line, logical px.
    pub width: f32,
    /// Total height, logical px.
    pub height: f32,
}

impl RichShapedText {
    #[inline]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

/// One laid-out line within a [`ShapedText`] (SOUL §8.1). `glyph_start`/
/// `glyph_count` index into [`ShapedText::glyphs`]; `x` is the line's origin
/// along the inline axis **including the alignment offset**, so a renderer walks
/// each line's glyphs from `x` with a running pen and gets aligned, multi-line
/// positions. `baseline`/`top` are relative to the top of the whole layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShapedLine {
    /// Inline origin of the line (its left edge for LTR), logical px — carries
    /// the [`TextAlign`] shift.
    pub x: f32,
    /// Baseline offset from the top of the layout, logical px.
    pub baseline: f32,
    /// Top of the line box from the top of the layout, logical px.
    pub top: f32,
    /// Line-box height, logical px.
    pub height: f32,
    /// Index of this line's first glyph in [`ShapedText::glyphs`].
    pub glyph_start: u32,
    /// Number of glyphs on this line.
    pub glyph_count: u32,
}

/// A shaped, measured run of text (SOUL §8.1). `width`/`height` are the intrinsic
/// size handed up to layout; `glyphs` drive glyph-quad emission at paint. For
/// multi-line/aligned results, `lines` carries per-line origins (glyph advances
/// in `glyphs` stay line-local; a line's absolute x is `lines[i].x`).
#[derive(Clone, Debug, Default)]
pub struct ShapedText {
    pub glyphs: SmallVec<[ShapedGlyph; 16]>,
    /// Widest line, logical px (the intrinsic width handed up to layout).
    pub width: f32,
    /// Total height = line count × line height, logical px.
    pub height: f32,
    /// Baseline offset of the **first** line from the top, logical px (kept for
    /// back-compat single-line callers; per-line baselines live in `lines`).
    pub baseline: f32,
    /// One entry per laid-out line, in reading order.
    pub lines: SmallVec<[ShapedLine; 4]>,
    /// The requested primary face for the run. Individual fallback glyphs carry
    /// their resolved face in [`ShapedGlyph::font`].
    pub font: FontId,
}

impl ShapedText {
    /// Empties the result but keeps the glyph/line capacity, so shaping into it
    /// again ([`TextShaper::shape_into`]) allocates nothing once it has grown.
    pub fn clear(&mut self) {
        self.glyphs.clear();
        self.lines.clear();
        self.width = 0.0;
        self.height = 0.0;
        self.baseline = 0.0;
        self.font = FontId::default();
    }

    /// Number of laid-out lines.
    #[inline]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

/// A rasterized glyph: its slot inside the atlas plus the placement bearing the
/// renderer needs to position the quad (SOUL §8.1). `left`/`top` are the offset
/// of the coverage bitmap's top-left from the glyph origin/baseline (swash/zeno
/// convention: `top` is measured upward from the baseline).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RasterGlyph {
    /// Location of the coverage bitmap inside the atlas.
    pub rect: AtlasRect,
    /// Left bearing (px) of the bitmap from the pen origin.
    pub left: i32,
    /// Top bearing (px) of the bitmap above the baseline.
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

/// A CPU-side **R8** glyph atlas with a tracked dirty sub-rect (SOUL §8.1, §3.2).
/// One byte of coverage per texel; grow-only backing store (§4.4). The renderer
/// uploads only [`GlyphAtlas::take_dirty`]'s rect via `write_texture`.
#[derive(Clone)]
pub struct GlyphAtlas {
    width: u32,
    height: u32,
    /// R8 coverage, row-major, `width * height` bytes.
    pixels: Vec<u8>,
    /// union of texel rects written since the last upload (§3.2).
    dirty: AtlasRect,
    /// Process-unique identity, so a renderer can tell a replaced atlas from a
    /// mutated one.
    id: u64,
    /// Bumped on every written rect. Renderers remember the generation they last
    /// uploaded and ask [`GlyphAtlas::delta_since`], so each keeps its own view
    /// independent of the shared `dirty` marker.
    generation: u64,
    /// Recent `(generation, rect)` writes, bounded to [`Self::WRITE_LOG_CAP`].
    write_log: Vec<(u64, AtlasRect)>,
    /// Generations at or below this are no longer covered by `write_log`.
    log_floor: u64,
    /// simple shelf packer cursor (mount/grow may allocate, §4).
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    /// Tallest the atlas may grow to; see [`GlyphAtlas::grow`].
    max_height: u32,
    /// Set once an allocation failed that growth could not fix (the atlas is at
    /// its height cap, or the slot is wider than the atlas). Such a glyph is not
    /// drawn, and the atlas should not be carried into a replacement app.
    exhausted: bool,
    /// The donor's generation when this atlas was made by [`GlyphAtlas::fork`].
    /// The fork shares the donor's id, which is only sound while the donor has
    /// not written anything since; see [`GlyphAtlas::resolve_fork`].
    fork_base: Option<u64>,
}

static NEXT_GLYPH_ATLAS_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// How much of a [`GlyphAtlas`] a consumer must re-upload (see [`GlyphAtlas::delta_since`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtlasDelta {
    Unchanged,
    Rect(AtlasRect),
    /// The consumer is too far behind for the write log; re-upload everything.
    Full,
}

impl GlyphAtlas {
    /// Writes retained for [`GlyphAtlas::delta_since`]; older consumers get a full upload.
    const WRITE_LOG_CAP: usize = 64;

    /// Height cap for [`GlyphAtlas::grow`] on atlases that start smaller. Within
    /// the 8192 texel limit every wgpu device guarantees.
    pub const DEFAULT_MAX_HEIGHT: u32 = 4096;

    /// A cleared atlas of the given texel dimensions.
    pub fn new(width: u32, height: u32) -> GlyphAtlas {
        GlyphAtlas {
            width,
            height,
            pixels: vec![0u8; (width as usize) * (height as usize)],
            dirty: AtlasRect::default(),
            id: NEXT_GLYPH_ATLAS_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            generation: 0,
            write_log: Vec::with_capacity(Self::WRITE_LOG_CAP),
            log_floor: 0,
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            max_height: height.max(Self::DEFAULT_MAX_HEIGHT),
            exhausted: false,
            fork_base: None,
        }
    }

    /// A copy that keeps this atlas's id and write log, so a renderer that has
    /// uploaded this atlas can keep its texture and upload only the fork's later
    /// writes. Call [`GlyphAtlas::resolve_fork`] before the fork replaces the
    /// atlas it was taken from.
    pub fn fork(&self) -> GlyphAtlas {
        let mut fork = self.clone();
        fork.fork_base = Some(self.generation);
        fork
    }

    /// Decides whether this atlas may keep `running`'s identity when it replaces
    /// it. A fork keeps the id only while `running` is still at the generation the
    /// fork was taken at; if `running` wrote more glyphs since, the two hold
    /// different texels in the same slots, so the fork gets a fresh id and the
    /// renderer treats it as a new atlas. Returns whether the identity was kept.
    pub fn resolve_fork(&mut self, running: &GlyphAtlas) -> bool {
        let kept =
            self.id == running.id && self.fork_base.is_none_or(|base| base == running.generation);
        if self.id == running.id && !kept {
            self.id = NEXT_GLYPH_ATLAS_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.fork_base = None;
        kept
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    /// The raw R8 coverage buffer (row-major).
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Folds a written rect into the pending dirty region (SOUL §3.2).
    pub fn mark_dirty(&mut self, rect: AtlasRect) {
        self.dirty = self.dirty.union(&rect);
        self.generation += 1;
        if self.write_log.len() == Self::WRITE_LOG_CAP {
            self.log_floor = self.write_log.remove(0).0;
        }
        self.write_log.push((self.generation, rect));
    }

    /// Process-unique identity of this atlas.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The current write generation (0 before any write).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What a consumer that last saw `generation` must upload to match this atlas.
    /// Does not touch the shared dirty marker, so any number of renderers can
    /// synchronize independently.
    pub fn delta_since(&self, generation: u64) -> AtlasDelta {
        if generation == self.generation {
            return AtlasDelta::Unchanged;
        }
        if generation < self.log_floor || generation > self.generation {
            return AtlasDelta::Full;
        }
        let mut union = AtlasRect::default();
        for (g, rect) in &self.write_log {
            if *g > generation {
                union = union.union(rect);
            }
        }
        if union.is_empty() {
            AtlasDelta::Unchanged
        } else {
            AtlasDelta::Rect(union)
        }
    }

    /// Returns and clears the pending dirty rect — called once per frame by the
    /// backend before `write_texture` (SOUL §3.2). Returns `None` if nothing
    /// changed (the steady-state, zero-upload case).
    pub fn take_dirty(&mut self) -> Option<AtlasRect> {
        if self.dirty.is_empty() {
            None
        } else {
            let d = self.dirty;
            self.dirty = AtlasRect::default();
            Some(d)
        }
    }

    /// Allocates a `w×h` shelf slot for a new glyph, returning its rect, or `None`
    /// if the atlas is full. The caller then calls [`GlyphAtlas::grow`] and retries
    /// (a grow event, §4); `allocate` itself never grows. A failure marks the atlas
    /// exhausted only when growing cannot help.
    pub fn allocate(&mut self, w: u32, h: u32) -> Option<AtlasRect> {
        if w > self.width {
            self.exhausted = true;
            return None;
        }
        if self.shelf_x + w > self.width {
            // advance to next shelf
            self.shelf_y += self.shelf_h;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }
        if self.shelf_y + h > self.height {
            if self.height >= self.max_height {
                self.exhausted = true;
            }
            return None;
        }
        let rect = AtlasRect {
            x: self.shelf_x,
            y: self.shelf_y,
            width: w,
            height: h,
        };
        self.shelf_x += w;
        self.shelf_h = self.shelf_h.max(h);
        Some(rect)
    }

    /// Whether a `w` x `h` slot could be placed in this atlas after growing:
    /// it must be no wider than the atlas and no taller than the height cap.
    /// Growing for a slot that fails this only wastes memory and re-uploads.
    pub fn can_ever_fit(&self, w: u32, h: u32) -> bool {
        w <= self.width && h <= self.max_height
    }

    /// Doubles the atlas height (up to the cap), keeping every texel and slot
    /// where it is, so rects already handed out stay valid. Returns `false`, and
    /// marks the atlas exhausted, when it is already at the cap.
    ///
    /// Growth bumps the generation and drops the write log, so a consumer synced
    /// before it gets [`AtlasDelta::Full`] and re-uploads. A renderer also sees
    /// new dimensions and recreates its texture. The partial shelf the packer
    /// is on keeps filling; rows below it are new space.
    pub fn grow(&mut self) -> bool {
        if self.height >= self.max_height {
            self.exhausted = true;
            return false;
        }
        let new_height = self
            .height
            .saturating_mul(2)
            .max(self.height + 1)
            .min(self.max_height);
        self.pixels
            .resize(self.width as usize * new_height as usize, 0);
        self.height = new_height;
        self.generation += 1;
        self.write_log.clear();
        self.log_floor = self.generation;
        self.dirty = AtlasRect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        };
        true
    }

    /// Fraction of the *current* atlas height consumed by shelves (0.0 to 1.0).
    /// Growing halves it, so a grown atlas reports headroom again.
    pub fn used_fraction(&self) -> f32 {
        if self.height == 0 {
            return 1.0;
        }
        ((self.shelf_y + self.shelf_h) as f32 / self.height as f32).min(1.0)
    }

    /// Whether this atlas may be carried into a replacement app: no allocation
    /// has failed past what growth could fix and at most half of its current
    /// height is used. The atlas never evicts, so a fuller one starts over fresh
    /// rather than growing toward its cap inside a long-lived app.
    pub fn has_headroom(&self) -> bool {
        !self.exhausted && self.used_fraction() <= 0.5
    }

    /// Copies row-major R8 coverage `src` into `rect` and folds it into the dirty
    /// region (SOUL §3.2). `src` must hold at least `rect.width * rect.height`
    /// bytes; a mismatch or empty rect is a no-op. Used by
    /// [`TextShaper::rasterize_glyph`].
    pub fn write_coverage(&mut self, rect: AtlasRect, src: &[u8]) {
        if rect.is_empty() {
            return;
        }
        let w = rect.width as usize;
        let h = rect.height as usize;
        if src.len() < w * h {
            return;
        }
        // Bounds guard: never write outside the backing store.
        if rect.right() > self.width || rect.bottom() > self.height {
            return;
        }
        let stride = self.width as usize;
        for row in 0..h {
            let dst = (rect.y as usize + row) * stride + rect.x as usize;
            let s = row * w;
            self.pixels[dst..dst + w].copy_from_slice(&src[s..s + w]);
        }
        self.mark_dirty(rect);
    }
}
