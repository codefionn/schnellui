//! A cell-accurate terminal surface.
//!
//! Unlike [`crate::widgets::RichText`], this widget never reflows or concatenates the
//! terminal contents. Every background and grapheme is painted at its retained
//! row/column, so blank cells, wide glyph occupancy, cursor geometry and selection
//! overlays survive the trip from a terminal emulator to the renderer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use schnellui::a11y::{ActionFlags, Role};
use schnellui::scene::{
    Color, DirtyFlags, PaintData, Point, Primitive, Rect, Scene, Size, TexelRect, WidgetId,
    WidgetKind,
};
use schnellui::text::{FontFace, GlyphAtlas, ShapeOptions, ShapedText, TextShaper, WrapMode};
use slotmap::SecondaryMap;

mod interaction;
mod patch;
use crate::widgets::{norm_scale, phys_size_px, rasterize_and_push, BuildCtx, View};
/// Inline small string used for terminal graphemes. Strings of up to 24 bytes
/// (22+ on every 64-bit target) live inside the value, so cloning a cell never
/// allocates. Re-exported so hosts can build cells without naming the crate.
pub use compact_str::CompactString;
pub use interaction::*;
pub use patch::TerminalGridPatch;

/// A row/column address in a terminal grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TerminalGridPoint {
    pub row: usize,
    pub column: usize,
}

impl TerminalGridPoint {
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// How a cell participates in fixed-width terminal layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalCellWidth {
    #[default]
    Single,
    /// The grapheme occupies this cell and the following cell.
    Wide,
    /// The cell is occupied by the wide grapheme immediately to its left.
    Continuation,
}

/// SGR-like visual attributes retained per cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCellAttrs {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub faint: bool,
    pub inverse: bool,
}

/// One exact terminal cell.
///
/// A cell owns no heap memory in the common case. The grapheme
/// is an inline [`CompactString`] (graphemes longer than 24 bytes spill to the
/// heap) and the hyperlink is a shared `Arc<str>`, so a run of cells carrying the
/// same OSC 8 target shares one allocation. `clone` and `clone_from` of such a
/// cell never allocate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCell {
    pub grapheme: CompactString,
    pub foreground: Color,
    pub background: Color,
    pub attrs: TerminalCellAttrs,
    pub width: TerminalCellWidth,
    /// OSC 8 target associated with this cell, if any.
    pub hyperlink: Option<Arc<str>>,
}

impl TerminalCell {
    pub fn blank(foreground: Color, background: Color) -> Self {
        Self {
            grapheme: CompactString::const_new(""),
            foreground,
            background,
            attrs: TerminalCellAttrs::default(),
            width: TerminalCellWidth::Single,
            hyperlink: None,
        }
    }

    pub fn new(grapheme: impl Into<CompactString>, foreground: Color, background: Color) -> Self {
        Self {
            grapheme: grapheme.into(),
            ..Self::blank(foreground, background)
        }
    }
}

/// Cursor geometry supported by ordinary terminal emulators.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalCursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
}

/// Retained cursor state. A blinking cursor paints when `blink_on` is true; the
/// host controls that phase without changing any cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursor {
    pub position: TerminalGridPoint,
    pub style: TerminalCursorStyle,
    pub color: Color,
    pub visible: bool,
    pub blinking: bool,
    pub blink_on: bool,
}

impl TerminalCursor {
    pub fn new(position: TerminalGridPoint, color: Color) -> Self {
        Self {
            position,
            style: TerminalCursorStyle::Block,
            color,
            visible: true,
            blinking: false,
            blink_on: true,
        }
    }

    fn paints(self) -> bool {
        self.visible && (!self.blinking || self.blink_on)
    }
}

/// Inclusive row-major terminal selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSelection {
    pub start: TerminalGridPoint,
    pub end: TerminalGridPoint,
    pub background: Color,
    pub foreground: Option<Color>,
}

impl TerminalSelection {
    pub fn contains(self, point: TerminalGridPoint) -> bool {
        let (start, end) = if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        };
        point >= start && point <= end
    }
}

/// Placement for a terminal graphics image. Pixel coordinates are logical pixels
/// relative to the terminal node; cell coordinates use the same fixed metrics as
/// text and cursor painting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TerminalImagePlacement {
    Cells {
        row: usize,
        column: usize,
        rows: usize,
        columns: usize,
    },
    Pixels {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
}

/// An owned RGBA terminal image, suitable for Kitty graphics placements.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalImage {
    /// Stable protocol/application identity. Reusing an id and dimensions updates
    /// the existing atlas allocation instead of growing the atlas.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub placement: TerminalImagePlacement,
    /// Negative/zero images paint behind text; positive images paint above it.
    pub z: i32,
    pub tint: Color,
}

impl TerminalImage {
    pub fn new(
        id: u64,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        placement: TerminalImagePlacement,
    ) -> Self {
        Self {
            id,
            width,
            height,
            rgba,
            placement,
            z: 0,
            tint: Color::WHITE,
        }
    }
}

/// Complete owned viewport consumed by [`TerminalGrid`]. `cells` is row-major;
/// absent entries are painted as blank cells using the model's default colors.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalGridModel {
    pub columns: usize,
    pub rows: usize,
    pub cells: Vec<TerminalCell>,
    pub foreground: Color,
    pub background: Color,
    pub cursor: Option<TerminalCursor>,
    pub selection: Option<TerminalSelection>,
    pub images: Vec<TerminalImage>,
}

impl TerminalGridModel {
    pub fn new(columns: usize, rows: usize) -> Self {
        Self::with_colors(columns, rows, Color::WHITE, Color::BLACK)
    }

    pub fn with_colors(columns: usize, rows: usize, foreground: Color, background: Color) -> Self {
        let len = columns.saturating_mul(rows);
        Self {
            columns,
            rows,
            cells: vec![TerminalCell::blank(foreground, background); len],
            foreground,
            background,
            cursor: None,
            selection: None,
            images: Vec::new(),
        }
    }

    pub fn index(&self, point: TerminalGridPoint) -> Option<usize> {
        (point.row < self.rows && point.column < self.columns)
            .then(|| point.row * self.columns + point.column)
    }

    pub fn cell(&self, point: TerminalGridPoint) -> Option<&TerminalCell> {
        self.index(point).and_then(|index| self.cells.get(index))
    }

    pub fn cell_mut(&mut self, point: TerminalGridPoint) -> Option<&mut TerminalCell> {
        let index = self.index(point)?;
        self.cells.get_mut(index)
    }

    pub fn set_cell(&mut self, point: TerminalGridPoint, cell: TerminalCell) -> bool {
        let Some(target) = self.cell_mut(point) else {
            return false;
        };
        *target = cell;
        true
    }

    /// Returns the hyperlink under a cell, treating a wide-cell continuation as
    /// part of its owner cell when the continuation carries no explicit URI.
    pub fn hyperlink_at(&self, point: TerminalGridPoint) -> Option<&str> {
        let cell = self.cell(point)?;
        if let Some(uri) = cell.hyperlink.as_deref() {
            return Some(uri);
        }
        if cell.width == TerminalCellWidth::Continuation && point.column > 0 {
            return self
                .cell(TerminalGridPoint::new(point.row, point.column - 1))
                .and_then(|owner| owner.hyperlink.as_deref());
        }
        None
    }

    /// The accessible text: rows joined by `\n`, blank cells as spaces, wide
    /// continuations skipped. Allocates; [`Self::plain_text_into`] reuses a buffer.
    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        self.plain_text_into(&mut text);
        text
    }

    /// Writes [`Self::plain_text`] into `out`, replacing its contents and keeping
    /// its capacity.
    pub fn plain_text_into(&self, out: &mut String) {
        out.clear();
        for row in 0..self.rows {
            if row > 0 {
                out.push('\n');
            }
            self.push_row_text(row, out);
        }
    }

    /// Appends one row's accessible text to `out`.
    fn push_row_text(&self, row: usize, out: &mut String) {
        for column in 0..self.columns {
            let point = TerminalGridPoint::new(row, column);
            match self.cell(point) {
                Some(cell) if cell.width == TerminalCellWidth::Continuation => {}
                Some(cell) if !cell.grapheme.is_empty() => out.push_str(&cell.grapheme),
                _ => out.push(' '),
            }
        }
    }
}

/// Fixed metrics shared by grid sizing, painting and pointer hit-testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalCellMetrics {
    pub width: f32,
    pub height: f32,
    pub font_size: f32,
}

impl TerminalCellMetrics {
    pub fn new(width: f32, height: f32, font_size: f32) -> Self {
        Self {
            width: width.max(1.0),
            height: height.max(1.0),
            font_size: font_size.max(1.0),
        }
    }

    /// Measures the bundled Liberation Mono face (whose fallback chain includes
    /// Nerd Fonts Symbols Mono) at the same physical scale used for painting.
    pub fn measure(shaper: &mut TextShaper, font_size: f32, scale: f32) -> Self {
        Self::measure_with_family(shaper, font_size, scale, None)
    }

    /// Measures the selected installed family, falling back to bundled mono.
    pub fn measure_with_family(
        shaper: &mut TextShaper,
        font_size: f32,
        scale: f32,
        font_family: Option<&str>,
    ) -> Self {
        let scale = norm_scale(scale);
        let physical_size = phys_size_px(font_size, scale);
        let opts = ShapeOptions::new(physical_size)
            .wrap(WrapMode::NoWrap)
            .face(FontFace::Mono);
        let shaped = match font_family {
            Some(family) => shaper.shape_with_family("M", &opts, family),
            None => shaper.shape_with("M", &opts),
        };
        Self::new(shaped.width / scale, shaped.height / scale, font_size)
    }

    pub fn grid_size(self, columns: usize, rows: usize) -> Size {
        Size {
            width: self.width * columns as f32,
            height: self.height * rows as f32,
        }
    }

    pub fn cell_rect(self, origin: Point, point: TerminalGridPoint) -> Rect {
        Rect::new(
            origin.x + point.column as f32 * self.width,
            origin.y + point.row as f32 * self.height,
            self.width,
            self.height,
        )
    }

    /// Maps a window-space point into the occupied grid. Right/bottom remainder
    /// space in a larger component intentionally returns `None`.
    pub fn point_to_cell(
        self,
        rect: Rect,
        point: Point,
        columns: usize,
        rows: usize,
    ) -> Option<TerminalGridPoint> {
        if !rect.contains(point) || self.width <= 0.0 || self.height <= 0.0 {
            return None;
        }
        let column = ((point.x - rect.x) / self.width).floor() as usize;
        let row = ((point.y - rect.y) / self.height).floor() as usize;
        (column < columns && row < rows).then(|| TerminalGridPoint::new(row, column))
    }
}

/// Result of terminal pointer hit-testing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalGridHit {
    pub position: TerminalGridPoint,
    pub hyperlink: Option<String>,
}

type GridFn = Box<dyn FnMut() -> TerminalGridModel + 'static>;
type GridRevisionFn = Box<dyn FnMut() -> u64 + 'static>;
type GridPatchFn = Box<dyn FnMut(&mut TerminalGridPatch<'_>) + 'static>;

enum TerminalGridSource {
    Static(TerminalGridModel),
    Dynamic(GridFn),
    Versioned {
        revision: GridRevisionFn,
        model: GridFn,
    },
    Patched {
        revision: GridRevisionFn,
        patch: GridPatchFn,
    },
}

enum DynamicTerminalGridSource {
    Unversioned(GridFn),
    Versioned {
        revision: GridRevisionFn,
        observed_revision: u64,
        model: GridFn,
    },
    Patched {
        revision: GridRevisionFn,
        observed_revision: u64,
        patch: GridPatchFn,
    },
}

impl DynamicTerminalGridSource {
    /// Sources polled every frame because an external clock (a PTY reader, a
    /// host event loop) advances them without writing a signal.
    fn is_externally_clocked(&self) -> bool {
        matches!(self, Self::Versioned { .. } | Self::Patched { .. })
    }
}

/// A fixed-cell terminal viewport with full-background paint and ScrollView
/// semantics. Use [`TerminalGrid::dynamic`] for a signal-backed model,
/// [`TerminalGrid::dynamic_versioned`] for a PTY/host-backed revision source that
/// rebuilds a complete model, and [`TerminalGrid::patched`] for a host that writes
/// changed rows straight into the retained model.
pub struct TerminalGrid {
    source: TerminalGridSource,
    font_size: f32,
    font_family: Option<String>,
    metrics: Option<TerminalCellMetrics>,
    label: String,
}

impl TerminalGrid {
    pub fn new(model: TerminalGridModel) -> Self {
        Self {
            source: TerminalGridSource::Static(model),
            font_size: 15.0,
            font_family: None,
            metrics: None,
            label: "Terminal".to_string(),
        }
    }

    pub fn dynamic(source: impl FnMut() -> TerminalGridModel + 'static) -> Self {
        Self {
            source: TerminalGridSource::Dynamic(Box::new(source)),
            font_size: 15.0,
            font_family: None,
            metrics: None,
            label: "Terminal".to_string(),
        }
    }

    /// Creates a dynamic terminal whose complete model is rebuilt only when its
    /// inexpensive revision source changes. This prevents unrelated application
    /// signals from cloning and comparing the full terminal viewport.
    pub fn dynamic_versioned(
        revision: impl FnMut() -> u64 + 'static,
        model: impl FnMut() -> TerminalGridModel + 'static,
    ) -> Self {
        Self {
            source: TerminalGridSource::Versioned {
                revision: Box::new(revision),
                model: Box::new(model),
            },
            font_size: 15.0,
            font_family: None,
            metrics: None,
            label: "Terminal".to_string(),
        }
    }

    /// Creates a terminal whose host edits the retained model in place.
    ///
    /// `revision` is polled every frame, like [`TerminalGrid::dynamic_versioned`].
    /// When it returns a new value, `patch` runs once with write access to the
    /// widget's own model through [`TerminalGridPatch`]. Nothing clones or diffs
    /// the whole grid: each `row_mut`, cursor or selection change marks only the
    /// rows it touches, and the frame re-emits only those rows. `patch` also runs
    /// once while the view is built, against an empty 0x0 model, so the host can
    /// `resize` and fill it before the first frame.
    ///
    /// While `patch` runs the widget's state is detached from the registry, so
    /// hit testing that terminal from inside the closure finds nothing.
    pub fn patched(
        revision: impl FnMut() -> u64 + 'static,
        patch: impl FnMut(&mut TerminalGridPatch<'_>) + 'static,
    ) -> Self {
        Self {
            source: TerminalGridSource::Patched {
                revision: Box::new(revision),
                patch: Box::new(patch),
            },
            font_size: 15.0,
            font_family: None,
            metrics: None,
            label: "Terminal".to_string(),
        }
    }

    pub fn font_size(mut self, font_size: f32) -> Self {
        self.font_size = font_size.max(1.0);
        self
    }

    /// Selects an installed font family for terminal glyphs.
    pub fn font_family(mut self, font_family: impl Into<String>) -> Self {
        self.font_family = Some(font_family.into());
        self
    }

    /// Uses explicit metrics, useful when the PTY has already been sized using
    /// a measured cell width/height.
    pub fn metrics(mut self, metrics: TerminalCellMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn kind(&self) -> WidgetKind {
        WidgetKind::TerminalGrid
    }
}

struct CachedImage {
    width: u32,
    height: u32,
    texels: TexelRect,
}

/// Render-ready fragments for one terminal row. Backgrounds and glyphs live in
/// separate vectors because the GPU consumes them in separate instanced batches.
/// Keeping the row fragments lets a cursor move or one echoed character replace
/// just that row instead of re-shaping the entire viewport.
#[derive(Default)]
struct TerminalRowPaint {
    backgrounds: Vec<Primitive>,
    glyphs: Vec<Primitive>,
    /// Grow-only fragment lengths (see [`pad_row_fragment`]).
    background_capacity: usize,
    glyph_capacity: usize,
}

/// Pads a row fragment with invisible 1x1 primitives up to a grow-only capacity.
///
/// Typing at a prompt adds a glyph to its row. Without padding the row's primitive
/// count would change, every later range would shift, and the renderer would have
/// to re-encode the whole grid. With spare slots the row keeps its length, the
/// edit stays a `Scene::paint_range_mut` range, and the renderer re-encodes one
/// row. Capacity grows by half again when exceeded and is never trimmed, so a row
/// pays O(log n) whole-node updates over its life. Rows that never held content
/// keep capacity 0, so a blank grid is still one backdrop primitive. Padding sits
/// inside the row (so it is never clipped away) and has zero alpha.
fn pad_row_fragment(prims: &mut Vec<Primitive>, capacity: &mut usize, anchor: Point, glyph: bool) {
    let live = prims.len();
    if live > *capacity {
        *capacity = live + live / 2 + 4;
    }
    let rect = Rect::new(anchor.x, anchor.y, 1.0, 1.0);
    let pad = if glyph {
        Primitive::GlyphQuad {
            rect,
            atlas_uv: Rect::new(0.0, 0.0, 0.0, 0.0),
            color: Color::TRANSPARENT,
        }
    } else {
        Primitive::SolidRect {
            rect,
            color: Color::TRANSPARENT,
            corner_radius: 0.0,
        }
    };
    prims.resize(*capacity, pad);
}

fn row_anchor(rect: Rect, metrics: TerminalCellMetrics, row: usize) -> Point {
    Point {
        x: rect.x,
        y: rect.y + row as f32 * metrics.height,
    }
}

#[derive(Default)]
struct TerminalPaintRanges {
    backgrounds: Vec<std::ops::Range<usize>>,
    negative_images: std::ops::Range<usize>,
    glyphs: Vec<std::ops::Range<usize>>,
    positive_images: std::ops::Range<usize>,
    cursor: std::ops::Range<usize>,
}

/// The retained terminal display list. The scene still owns the flattened list
/// consumed by generic backends, while this cache owns its stable row fragments
/// and the ranges at which they were installed. Incremental updates splice only
/// affected rows into that list; unchanged rows keep their existing primitives.
#[derive(Default)]
struct TerminalPaintCache {
    rows: Vec<TerminalRowPaint>,
    backdrop: Option<Primitive>,
    negative_images: Vec<Primitive>,
    positive_images: Vec<Primitive>,
    cursor: Vec<Primitive>,
    ranges: TerminalPaintRanges,
}

impl TerminalPaintCache {
    fn resize_rows(&mut self, rows: usize) {
        self.rows.resize_with(rows, TerminalRowPaint::default);
        self.ranges.backgrounds.resize(rows, 0..0);
        self.ranges.glyphs.resize(rows, 0..0);
    }

    /// Rebuild range metadata from cached fragment lengths. This is O(rows), not
    /// O(cells), and never copies a primitive.
    fn reindex(&mut self) {
        self.ranges.backgrounds.clear();
        self.ranges.glyphs.clear();
        let mut at = 1; // full-grid backdrop
        for row in &self.rows {
            let end = at + row.backgrounds.len();
            self.ranges.backgrounds.push(at..end);
            at = end;
        }
        self.ranges.negative_images = at..at + self.negative_images.len();
        at = self.ranges.negative_images.end;
        for row in &self.rows {
            let end = at + row.glyphs.len();
            self.ranges.glyphs.push(at..end);
            at = end;
        }
        self.ranges.positive_images = at..at + self.positive_images.len();
        at = self.ranges.positive_images.end;
        self.ranges.cursor = at..at + self.cursor.len();
    }
}

/// Deterministic accounting for the retained terminal path. Kept private because
/// it is a regression probe, not another widget API. Tests assert that a one-cell
/// model change rebuilds one row rather than the whole terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TerminalPaintWork {
    background_rows: usize,
    glyph_rows: usize,
    cells: usize,
    full_rebuilds: usize,
    scene_range_writes: usize,
}

/// A set of row indices with O(1) insert and membership, and iteration and
/// clearing proportional to the rows in the set rather than the grid height.
#[derive(Default)]
struct RowSet {
    flags: Vec<bool>,
    list: Vec<usize>,
}

impl RowSet {
    fn all(rows: usize) -> Self {
        Self {
            flags: vec![true; rows],
            list: (0..rows).collect(),
        }
    }

    /// Resizes to `rows` and marks every row. A grow event (a resize).
    fn reset_all(&mut self, rows: usize) {
        self.flags.clear();
        self.flags.resize(rows, true);
        self.list.clear();
        self.list.extend(0..rows);
    }

    /// Resizes to `rows` with no row marked.
    fn reset_none(&mut self, rows: usize) {
        self.flags.clear();
        self.flags.resize(rows, false);
        self.list.clear();
    }

    fn insert(&mut self, row: usize) {
        if let Some(flag) = self.flags.get_mut(row) {
            if !*flag {
                *flag = true;
                self.list.push(row);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    fn clear(&mut self) {
        for &row in &self.list {
            self.flags[row] = false;
        }
        self.list.clear();
    }

    /// Sorts the listed rows in place (no allocation) and returns them.
    fn sorted(&mut self) -> &[usize] {
        self.list.sort_unstable();
        &self.list
    }
}

struct TerminalGridState {
    model: TerminalGridModel,
    metrics: TerminalCellMetrics,
    font_family: Option<String>,
    scale: f32,
    source: Option<DynamicTerminalGridSource>,
    measured: Rc<RefCell<Size>>,
    image_cache: HashMap<u64, CachedImage>,
    glyph_shape_cache: HashMap<FontFace, HashMap<CompactString, ShapedText>>,
    paint_cache: TerminalPaintCache,
    /// Rows whose model-dependent cell fragments need rebuilding. A row is the
    /// smallest unit that preserves merged background runs without scanning or
    /// rewriting neighboring rows.
    dirty_rows: RowSet,
    images_dirty: bool,
    cursor_dirty: bool,
    full_paint_dirty: bool,
    /// Rows whose accessible text may have changed since the a11y value was last
    /// synchronized (`sync_a11y_value`).
    a11y_rows: RowSet,
    /// The a11y value must be rebuilt from every row (mount, resize).
    a11y_full: bool,
    /// Byte offset one past each row's text inside the node's a11y value. Lets a
    /// row update splice its text in place instead of rebuilding the whole value.
    a11y_row_ends: Vec<usize>,
    /// Reused buffer for one row's accessible text.
    a11y_scratch: String,
    last_work: TerminalPaintWork,
    last_emit: Option<Rect>,
}

impl TerminalGridState {
    /// Marks everything for rebuilding after the grid's dimensions changed or on
    /// first mount.
    fn mark_all_dirty(&mut self) {
        let rows = self.model.rows;
        self.full_paint_dirty = true;
        self.dirty_rows.reset_all(rows);
        self.a11y_rows.reset_all(rows);
        self.a11y_full = true;
        self.images_dirty = true;
        self.cursor_dirty = true;
    }
}

/// Brings the node's accessible value up to date with the model.
///
/// After mount or a resize the value is rebuilt from every row into the node's
/// existing `String`. Otherwise only rows on `a11y_rows` are re-rendered into a
/// scratch buffer and compared with their current byte range; a changed row is
/// spliced in place with `String::replace_range`. Equal-length rewrites touch
/// only that row. A length change also moves the tail bytes and shifts the later
/// row offsets, both plain memory work with no allocation once the value has
/// grown to its working size. Returns whether the value changed.
fn sync_a11y_value(scene: &mut Scene, id: WidgetId, state: &mut TerminalGridState) -> bool {
    let rows = state.model.rows;
    let current_len = scene
        .a11y(id)
        .and_then(|a11y| a11y.value.as_ref())
        .map(String::len);
    let consistent = !state.a11y_full
        && state.a11y_row_ends.len() == rows
        && current_len == Some(state.a11y_row_ends.last().copied().unwrap_or(0));
    if !consistent {
        let a11y = scene.a11y_text_mut(id);
        let value = a11y.value.get_or_insert_with(String::new);
        value.clear();
        state.a11y_row_ends.clear();
        for row in 0..rows {
            if row > 0 {
                value.push('\n');
            }
            state.model.push_row_text(row, value);
            state.a11y_row_ends.push(value.len());
        }
        state.a11y_full = false;
        state.a11y_rows.reset_none(rows);
        return true;
    }
    let mut changed = false;
    for index in 0..state.a11y_rows.list.len() {
        let row = state.a11y_rows.list[index];
        if row >= rows {
            continue;
        }
        let start = if row == 0 {
            0
        } else {
            state.a11y_row_ends[row - 1] + 1
        };
        let end = state.a11y_row_ends[row];
        state.a11y_scratch.clear();
        state.model.push_row_text(row, &mut state.a11y_scratch);
        let same = scene
            .a11y(id)
            .and_then(|a11y| a11y.value.as_deref())
            .is_some_and(|value| value.get(start..end) == Some(state.a11y_scratch.as_str()));
        if same {
            continue;
        }
        let value = scene
            .a11y_text_mut(id)
            .value
            .as_mut()
            .expect("checked consistent above");
        value.replace_range(start..end, &state.a11y_scratch);
        let new_end = start + state.a11y_scratch.len();
        if new_end != end {
            for later in &mut state.a11y_row_ends[row..] {
                *later = *later + new_end - end;
            }
        }
        changed = true;
    }
    state.a11y_rows.clear();
    changed
}

#[derive(Default)]
struct TerminalGridRuntime {
    grids: RefCell<SecondaryMap<WidgetId, TerminalGridState>>,
    /// Reused because grid callbacks must run without a registry borrow held.
    key_scratch: Vec<WidgetId>,
    /// Dense list of the mounted grids, so the per-frame snapshot does not scan
    /// the map's slot capacity (which follows the whole scene, not the grids).
    ids: Vec<WidgetId>,
}

#[derive(Clone, Default)]
pub(crate) struct Runtime(Rc<RefCell<TerminalGridRuntime>>);

impl Runtime {
    fn with<R>(
        &self,
        access: impl FnOnce(&RefCell<SecondaryMap<WidgetId, TerminalGridState>>) -> R,
    ) -> R {
        let runtime = self.0.borrow();
        access(&runtime.grids)
    }

    /// Number of ids in the dense grid list.
    #[cfg(test)]
    pub(crate) fn dense_len(&self) -> usize {
        self.0.borrow().ids.len()
    }

    fn take_ids(&self, include: impl Fn(&TerminalGridState) -> bool) -> Vec<WidgetId> {
        let mut runtime = self.0.borrow_mut();
        let mut ids = std::mem::take(&mut runtime.key_scratch);
        let grids = runtime.grids.borrow();
        ids.extend(
            runtime
                .ids
                .iter()
                .copied()
                .filter(|&id| grids.get(id).is_some_and(&include)),
        );
        drop(grids);
        ids
    }

    fn return_ids(&self, mut ids: Vec<WidgetId>) {
        ids.clear();
        self.0.borrow_mut().key_scratch = ids;
    }
}

pub(crate) fn purge_nodes(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    nodes: &[WidgetId],
) {
    runtime.terminal_grid.with(|grids| {
        let mut grids = grids.borrow_mut();
        for &id in nodes {
            // The grid's images go back to the atlas, so a remounted grid
            // reuses their regions instead of growing it.
            if let Some(state) = grids.remove(id) {
                for cached in state.image_cache.values() {
                    scene.images_mut().free(cached.texels);
                }
            }
        }
    });
    let mut state = runtime.terminal_grid.0.borrow_mut();
    if state.ids.is_empty() && state.key_scratch.is_empty() {
        return;
    }
    // A few nodes (the common purge) are cheaper to scan than to hash.
    if nodes.len() <= 8 {
        state.ids.retain(|id| !nodes.contains(id));
        state.key_scratch.retain(|id| !nodes.contains(id));
    } else {
        let removed: std::collections::HashSet<WidgetId> = nodes.iter().copied().collect();
        state.ids.retain(|id| !removed.contains(id));
        state.key_scratch.retain(|id| !removed.contains(id));
    }
}

impl View for TerminalGrid {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::TerminalGrid, parent);
        let mut initial_patch = None;
        let (model, source) = match this.source {
            TerminalGridSource::Static(model) => (model, None),
            TerminalGridSource::Dynamic(mut source) => {
                let model = ctx.runtime.track_dynamic_initial(id, &mut source);
                (model, Some(DynamicTerminalGridSource::Unversioned(source)))
            }
            TerminalGridSource::Versioned {
                mut revision,
                mut model,
            } => {
                let observed_revision = revision();
                let initial = model();
                (
                    initial,
                    Some(DynamicTerminalGridSource::Versioned {
                        revision,
                        observed_revision,
                        model,
                    }),
                )
            }
            TerminalGridSource::Patched {
                mut revision,
                patch,
            } => {
                let observed_revision = revision();
                initial_patch = Some(patch);
                (
                    TerminalGridModel::new(0, 0),
                    Some(DynamicTerminalGridSource::Patched {
                        revision,
                        observed_revision,
                        // Replaced below, once the initial patch has run.
                        patch: Box::new(|_| {}),
                    }),
                )
            }
        };
        let metrics = this.metrics.unwrap_or_else(|| {
            TerminalCellMetrics::measure_with_family(
                ctx.text,
                this.font_size,
                ctx.scale,
                this.font_family.as_deref(),
            )
        });
        let measured = Rc::new(RefCell::new(Size {
            width: 0.0,
            height: 0.0,
        }));
        let mut state = TerminalGridState {
            model,
            metrics,
            font_family: this.font_family,
            scale: ctx.scale,
            source,
            measured: measured.clone(),
            image_cache: HashMap::new(),
            glyph_shape_cache: HashMap::new(),
            paint_cache: TerminalPaintCache::default(),
            dirty_rows: RowSet::default(),
            images_dirty: true,
            cursor_dirty: true,
            full_paint_dirty: true,
            a11y_rows: RowSet::default(),
            a11y_full: true,
            a11y_row_ends: Vec::new(),
            a11y_scratch: String::new(),
            last_work: TerminalPaintWork::default(),
            last_emit: None,
        };
        if let Some(mut patch) = initial_patch {
            // The host fills the empty model before anything measures it.
            patch(&mut TerminalGridPatch::new(&mut state));
            if let Some(DynamicTerminalGridSource::Patched { patch: slot, .. }) =
                state.source.as_mut()
            {
                *slot = patch;
            }
        }
        state.dirty_rows = RowSet::all(state.model.rows);
        *measured.borrow_mut() = metrics.grid_size(state.model.columns, state.model.rows);
        let measure_ref = measured;
        ctx.layout
            .set_measure(id, Box::new(move |_available| *measure_ref.borrow()));
        ctx.layout.set_fill_width(id);

        let a11y = ctx.scene.a11y_mut(id);
        a11y.role = Role::ScrollView.as_u16();
        a11y.name = Some(this.label);
        let mut actions = ActionFlags::default();
        actions.insert(ActionFlags::FOCUS);
        actions.insert(ActionFlags::SCROLL_UP);
        actions.insert(ActionFlags::SCROLL_DOWN);
        a11y.actions = actions.0;
        state.a11y_full = true;
        sync_a11y_value(ctx.scene, id, &mut state);

        // The dense id list gains the id only on a first insert, like the other
        // dense indices, so a rebuilt id is never listed twice.
        let first = ctx
            .runtime
            .terminal_grid
            .with(|grids| grids.borrow_mut().insert(id, state).is_none());
        if first {
            ctx.runtime.terminal_grid.0.borrow_mut().ids.push(id);
        }
        id
    }
}

pub(crate) fn reset(runtime: &crate::widgets::Runtime) {
    runtime
        .terminal_grid
        .with(|grids| grids.borrow_mut().clear());
    let mut state = runtime.terminal_grid.0.borrow_mut();
    state.key_scratch.clear();
    state.ids.clear();
}

pub(crate) fn contains(runtime: &crate::widgets::Runtime, id: WidgetId) -> bool {
    runtime
        .terminal_grid
        .with(|grids| grids.borrow().contains_key(id))
}

/// Returns the retained metrics for a mounted terminal node.
fn color_with_alpha(color: Color, numerator: u8, denominator: u8) -> Color {
    Color {
        a: ((color.a as u16 * numerator as u16) / denominator.max(1) as u16) as u8,
        ..color
    }
}

fn snap_to_physical_pixel(value: f32, scale: f32) -> f32 {
    (value * scale).round() / scale
}

fn cell_run_paint_rect(
    metrics: TerminalCellMetrics,
    origin: Point,
    row: usize,
    start_column: usize,
    end_column: usize,
    scale: f32,
) -> Rect {
    let left = snap_to_physical_pixel(origin.x + start_column as f32 * metrics.width, scale);
    let right = snap_to_physical_pixel(origin.x + end_column as f32 * metrics.width, scale);
    let top = snap_to_physical_pixel(origin.y + row as f32 * metrics.height, scale);
    let bottom = snap_to_physical_pixel(origin.y + (row + 1) as f32 * metrics.height, scale);
    Rect::new(left, top, right - left, bottom - top)
}

fn effective_cell_colors(model: &TerminalGridModel, point: TerminalGridPoint) -> (Color, Color) {
    let cell = model.cell(point);
    let mut foreground = cell.map_or(model.foreground, |cell| cell.foreground);
    let mut background = cell.map_or(model.background, |cell| cell.background);
    if cell.is_some_and(|cell| cell.attrs.inverse) {
        std::mem::swap(&mut foreground, &mut background);
    }
    if let Some(selection) = model
        .selection
        .filter(|selection| selection.contains(point))
    {
        background = selection.background;
    }
    let cursor = model
        .cursor
        .filter(|cursor| cursor.position == point && cursor.paints());
    if cursor.is_some_and(|cursor| cursor.style == TerminalCursorStyle::Block) {
        background = cursor.expect("checked above").color;
    }
    (foreground, background)
}

fn image_rect(placement: TerminalImagePlacement, rect: Rect, metrics: TerminalCellMetrics) -> Rect {
    match placement {
        TerminalImagePlacement::Cells {
            row,
            column,
            rows,
            columns,
        } => Rect::new(
            rect.x + column as f32 * metrics.width,
            rect.y + row as f32 * metrics.height,
            columns as f32 * metrics.width,
            rows as f32 * metrics.height,
        ),
        TerminalImagePlacement::Pixels {
            x,
            y,
            width,
            height,
        } => Rect::new(rect.x + x, rect.y + y, width.max(0.0), height.max(0.0)),
    }
}

fn prepare_images(
    scene: &mut Scene,
    state: &mut TerminalGridState,
    rect: Rect,
) -> Vec<(i32, Primitive)> {
    let mut images = Vec::with_capacity(state.model.images.len());
    for image in &state.model.images {
        let required = image.width as usize * image.height as usize * 4;
        if image.width == 0 || image.height == 0 || image.rgba.len() < required {
            continue;
        }
        let cached = state
            .image_cache
            .get(&image.id)
            .map(|cached| (cached.width, cached.height, cached.texels));
        let texels = match cached {
            Some((width, height, texels)) if width == image.width && height == image.height => {
                if state.images_dirty {
                    scene.images_mut().write_rect(texels, &image.rgba);
                }
                texels
            }
            previous => {
                // An id that comes back at another size gives its old region back.
                if let Some((_, _, texels)) = previous {
                    scene.images_mut().free(texels);
                    state.image_cache.remove(&image.id);
                }
                let Some(texels) =
                    scene
                        .images_mut()
                        .insert(image.width, image.height, &image.rgba)
                else {
                    continue;
                };
                state.image_cache.insert(
                    image.id,
                    CachedImage {
                        width: image.width,
                        height: image.height,
                        texels,
                    },
                );
                texels
            }
        };
        images.push((
            image.z,
            Primitive::ImageQuad {
                rect: image_rect(image.placement, rect, state.metrics),
                atlas_uv: Rect::new(
                    texels.x as f32,
                    texels.y as f32,
                    texels.width as f32,
                    texels.height as f32,
                ),
                tint: image.tint,
            },
        ));
    }
    // Ids the model no longer places give their regions back.
    let model = &state.model;
    state.image_cache.retain(|id, cached| {
        let placed = model.images.iter().any(|image| image.id == *id);
        if !placed {
            scene.images_mut().free(cached.texels);
        }
        placed
    });
    images.sort_by_key(|(z, _)| *z);
    images
}

fn rebuild_background_row(
    model: &TerminalGridModel,
    metrics: TerminalCellMetrics,
    scale: f32,
    rect: Rect,
    row: usize,
    row_paint: &mut TerminalRowPaint,
) {
    let primitives = &mut row_paint.backgrounds;
    primitives.clear();
    let mut run_start = 0;
    while run_start < model.columns {
        let (_, color) = effective_cell_colors(model, TerminalGridPoint::new(row, run_start));
        let mut run_end = run_start + 1;
        while run_end < model.columns
            && effective_cell_colors(model, TerminalGridPoint::new(row, run_end)).1 == color
        {
            run_end += 1;
        }
        if color != model.background {
            primitives.push(Primitive::SolidRect {
                rect: cell_run_paint_rect(
                    metrics,
                    Point {
                        x: rect.x,
                        y: rect.y,
                    },
                    row,
                    run_start,
                    run_end,
                    scale,
                ),
                color,
                corner_radius: 0.0,
            });
        }
        run_start = run_end;
    }
    pad_row_fragment(
        primitives,
        &mut row_paint.background_capacity,
        row_anchor(rect, metrics, row),
        false,
    );
}

fn rebuild_glyph_row(
    state: &mut TerminalGridState,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    rect: Rect,
    row: usize,
) {
    // Split the state into disjoint field borrows: the model is read while the
    // shape cache and paint cache are written, with no per-cell clone.
    let TerminalGridState {
        model,
        metrics,
        font_family,
        scale,
        glyph_shape_cache,
        paint_cache,
        ..
    } = state;
    let metrics = *metrics;
    let scale = norm_scale(*scale);
    let physical_size = phys_size_px(metrics.font_size, scale);
    let mut paint = PaintData {
        primitives: std::mem::take(&mut paint_cache.rows[row].glyphs),
    };
    paint.primitives.clear();

    for column in 0..model.columns {
        let point = TerminalGridPoint::new(row, column);
        let Some(cell) = model.cell(point) else {
            continue;
        };
        if cell.width == TerminalCellWidth::Continuation || cell.grapheme.is_empty() {
            continue;
        }
        let mut foreground = cell.foreground;
        let mut background = cell.background;
        if cell.attrs.inverse {
            std::mem::swap(&mut foreground, &mut background);
        }
        if let Some(selection) = model.selection.filter(|s| s.contains(point)) {
            foreground = selection.foreground.unwrap_or(foreground);
        }
        let cursor = model
            .cursor
            .filter(|cursor| cursor.position == point && cursor.paints());
        if cursor.is_some_and(|cursor| cursor.style == TerminalCursorStyle::Block) {
            foreground = background;
        }
        if cell.attrs.faint {
            foreground = color_with_alpha(foreground, 1, 2);
        }
        if push_block_element(
            &mut paint.primitives,
            cell.grapheme.as_str(),
            metrics.cell_rect(
                Point {
                    x: rect.x,
                    y: rect.y,
                },
                point,
            ),
            foreground,
            scale,
        ) {
            continue;
        }

        let face = if cell.attrs.bold {
            FontFace::MonoBold
        } else {
            // The embedded mono set has no italic face. Keeping the mono metrics
            // matters more than silently switching families.
            FontFace::Mono
        };
        let face_cache = glyph_shape_cache.entry(face).or_default();
        // Lookups borrow the inline grapheme as `&str`; only a first-seen
        // grapheme clones it into the cache.
        if !face_cache.contains_key(cell.grapheme.as_str()) {
            let opts = ShapeOptions::new(physical_size)
                .wrap(WrapMode::NoWrap)
                .face(face);
            let shaped = match font_family.as_deref() {
                Some(family) => shaper.shape_with_family(&cell.grapheme, &opts, family),
                None => shaper.shape_with(&cell.grapheme, &opts),
            };
            face_cache.insert(cell.grapheme.clone(), shaped);
        }
        let shaped = face_cache
            .get(cell.grapheme.as_str())
            .expect("terminal shape inserted above");
        let span_columns = if cell.width == TerminalCellWidth::Wide {
            2
        } else {
            1
        };
        let span_width = metrics.width * span_columns as f32;
        let shaped_width = shaped.width / scale;
        let shaped_height = shaped.height / scale;
        let cell_rect = metrics.cell_rect(
            Point {
                x: rect.x,
                y: rect.y,
            },
            point,
        );
        let origin = Point {
            x: cell_rect.x + ((span_width - shaped_width) * 0.5).max(0.0),
            y: cell_rect.y + ((metrics.height - shaped_height) * 0.5).max(0.0),
        };
        rasterize_and_push(
            &mut paint,
            shaper,
            atlas,
            shaped,
            physical_size as u32,
            foreground,
            scale,
            origin,
        );

        if cell.attrs.underline {
            let y = cell_rect.y + metrics.height - 1.5;
            paint.primitives.push(Primitive::Line {
                from: Point { x: cell_rect.x, y },
                to: Point {
                    x: cell_rect.x + span_width,
                    y,
                },
                width: 1.0,
                color: foreground,
            });
        }
        if cell.attrs.strike {
            let y = cell_rect.y + metrics.height * 0.52;
            paint.primitives.push(Primitive::Line {
                from: Point { x: cell_rect.x, y },
                to: Point {
                    x: cell_rect.x + span_width,
                    y,
                },
                width: 1.0,
                color: foreground,
            });
        }
    }
    let row_paint = &mut paint_cache.rows[row];
    row_paint.glyphs = paint.primitives;
    pad_row_fragment(
        &mut row_paint.glyphs,
        &mut row_paint.glyph_capacity,
        row_anchor(rect, metrics, row),
        true,
    );
}

/// Draws a block element (U+2580 to U+259F) as rectangles on the cell's
/// pixel grid instead of a font glyph. Glyphs rarely fill the line height, so
/// half blocks and quadrants of neighboring cells, such as in Claude Code's
/// logo, would show gaps between them. Returns false for any other grapheme.
fn push_block_element(
    primitives: &mut Vec<Primitive>,
    grapheme: &str,
    cell: Rect,
    color: Color,
    scale: f32,
) -> bool {
    let mut chars = grapheme.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return false;
    };
    // Quadrant bits: upper left, upper right, lower left, lower right.
    const UL: u8 = 1;
    const UR: u8 = 2;
    const LL: u8 = 4;
    const LR: u8 = 8;
    // A rectangle in eighths of the cell, or quadrants, and the shade.
    let (eighths, quadrants, shade): ([u8; 4], u8, u8) = match ch as u32 {
        0x2580 => ([0, 0, 8, 4], 0, 4),
        n @ 0x2581..=0x2588 => ([0, 8 - (n - 0x2580) as u8, 8, 8], 0, 4),
        n @ 0x2589..=0x258F => ([0, 0, 8 - (n - 0x2588) as u8, 8], 0, 4),
        0x2590 => ([4, 0, 8, 8], 0, 4),
        n @ 0x2591..=0x2593 => ([0, 0, 8, 8], 0, (n - 0x2590) as u8),
        0x2594 => ([0, 0, 8, 1], 0, 4),
        0x2595 => ([7, 0, 8, 8], 0, 4),
        0x2596 => ([0; 4], LL, 4),
        0x2597 => ([0; 4], LR, 4),
        0x2598 => ([0; 4], UL, 4),
        0x2599 => ([0; 4], UL | LL | LR, 4),
        0x259A => ([0; 4], UL | LR, 4),
        0x259B => ([0; 4], UL | UR | LL, 4),
        0x259C => ([0; 4], UL | UR | LR, 4),
        0x259D => ([0; 4], UR, 4),
        0x259E => ([0; 4], UR | LL, 4),
        0x259F => ([0; 4], UR | LL | LR, 4),
        _ => return false,
    };
    let color = color_with_alpha(color, shade, 4);
    let mut push = |[x0, y0, x1, y1]: [u8; 4]| {
        let x = |e: u8| snap_to_physical_pixel(cell.x + cell.width * f32::from(e) / 8.0, scale);
        let y = |e: u8| snap_to_physical_pixel(cell.y + cell.height * f32::from(e) / 8.0, scale);
        primitives.push(Primitive::SolidRect {
            rect: Rect::new(x(x0), y(y0), x(x1) - x(x0), y(y1) - y(y0)),
            color,
            corner_radius: 0.0,
        });
    };
    if quadrants == 0 {
        push(eighths);
    } else {
        for (bit, part) in [
            (UL, [0, 0, 4, 4]),
            (UR, [4, 0, 8, 4]),
            (LL, [0, 4, 4, 8]),
            (LR, [4, 4, 8, 8]),
        ] {
            if quadrants & bit != 0 {
                push(part);
            }
        }
    }
    true
}

fn rebuild_cursor(state: &mut TerminalGridState, rect: Rect) {
    let cursor_primitives = &mut state.paint_cache.cursor;
    cursor_primitives.clear();
    let Some(cursor) = state.model.cursor.filter(|cursor| cursor.paints()) else {
        return;
    };
    if cursor.position.row >= state.model.rows
        || cursor.position.column >= state.model.columns
        || cursor.style == TerminalCursorStyle::Block
    {
        return;
    }
    let cell = state.metrics.cell_rect(
        Point {
            x: rect.x,
            y: rect.y,
        },
        cursor.position,
    );
    match cursor.style {
        TerminalCursorStyle::Bar => cursor_primitives.push(Primitive::SolidRect {
            rect: Rect::new(cell.x, cell.y, 2.0_f32.min(cell.width), cell.height),
            color: cursor.color,
            corner_radius: 0.0,
        }),
        TerminalCursorStyle::Underline => cursor_primitives.push(Primitive::SolidRect {
            rect: Rect::new(
                cell.x,
                cell.bottom() - 2.0_f32.min(cell.height),
                cell.width,
                2.0_f32.min(cell.height),
            ),
            color: cursor.color,
            corner_radius: 0.0,
        }),
        TerminalCursorStyle::Block => {}
    }
}

fn rebuild_images(scene: &mut Scene, state: &mut TerminalGridState, rect: Rect) {
    let images = prepare_images(scene, state, rect);
    state.paint_cache.negative_images.clear();
    state.paint_cache.positive_images.clear();
    for (z, primitive) in images {
        if z <= 0 {
            state.paint_cache.negative_images.push(primitive);
        } else {
            state.paint_cache.positive_images.push(primitive);
        }
    }
}

fn replace_range(pd: &mut PaintData, range: std::ops::Range<usize>, replacement: &[Primitive]) {
    pd.primitives.splice(range, replacement.iter().copied());
}

fn compose_full(pd: &mut PaintData, cache: &TerminalPaintCache) {
    pd.primitives.clear();
    pd.primitives
        .push(cache.backdrop.expect("terminal cache has a backdrop"));
    for row in &cache.rows {
        pd.primitives.extend_from_slice(&row.backgrounds);
    }
    pd.primitives.extend_from_slice(&cache.negative_images);
    for row in &cache.rows {
        pd.primitives.extend_from_slice(&row.glyphs);
    }
    pd.primitives.extend_from_slice(&cache.positive_images);
    pd.primitives.extend_from_slice(&cache.cursor);
}

fn rebuild_all_rows(
    state: &mut TerminalGridState,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    rect: Rect,
    work: &mut TerminalPaintWork,
) {
    state.paint_cache.resize_rows(state.model.rows);
    let scale = norm_scale(state.scale);
    for row in 0..state.model.rows {
        rebuild_background_row(
            &state.model,
            state.metrics,
            scale,
            rect,
            row,
            &mut state.paint_cache.rows[row],
        );
        rebuild_glyph_row(state, shaper, atlas, rect, row);
        work.background_rows += 1;
        work.glyph_rows += 1;
        work.cells += state.model.columns;
    }
}

/// Rebuilds the fragments of the rows on `dirty_rows` only. Returns whether any
/// row's primitive count changed, which is the only case that moves the scene
/// ranges of later rows.
fn rebuild_dirty_rows(
    state: &mut TerminalGridState,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    rect: Rect,
    work: &mut TerminalPaintWork,
) -> bool {
    let scale = norm_scale(state.scale);
    let mut lengths_changed = false;
    let rows = std::mem::take(&mut state.dirty_rows.list);
    for &row in &rows {
        if row >= state.model.rows {
            continue;
        }
        let before = (
            state.paint_cache.rows[row].backgrounds.len(),
            state.paint_cache.rows[row].glyphs.len(),
        );
        rebuild_background_row(
            &state.model,
            state.metrics,
            scale,
            rect,
            row,
            &mut state.paint_cache.rows[row],
        );
        rebuild_glyph_row(state, shaper, atlas, rect, row);
        let after = (
            state.paint_cache.rows[row].backgrounds.len(),
            state.paint_cache.rows[row].glyphs.len(),
        );
        lengths_changed |= before != after;
        work.background_rows += 1;
        work.glyph_rows += 1;
        work.cells += state.model.columns;
    }
    state.dirty_rows.list = rows;
    lengths_changed
}

fn patch_dirty_rows(
    scene: &mut Scene,
    id: WidgetId,
    state: &mut TerminalGridState,
    lengths_changed: bool,
    work: &mut TerminalPaintWork,
) {
    // Patch glyphs first. The cached ranges still describe the scene's old display
    // list, while the cached row fragments already hold the new data. A glyph-size
    // change must therefore use its old range before a background splice shifts the
    // entire glyph section. Work backwards so a variable-length row never invalidates
    // ranges before it. Sorting the dirty list is in place and O(dirty log dirty).
    let rows = state.model.rows;
    let TerminalGridState {
        dirty_rows,
        paint_cache,
        ..
    } = state;
    let dirty = dirty_rows.sorted();
    if !lengths_changed {
        // Every row kept its primitive counts, so each row overwrites its own
        // range in place. `paint_range_mut` records the ranges, which lets the
        // renderer re-encode just these primitives instead of the whole grid.
        for &row in dirty.iter().filter(|&&row| row < rows) {
            let cached = &paint_cache.rows[row];
            for (range, fragment) in [
                (
                    paint_cache.ranges.backgrounds[row].clone(),
                    &cached.backgrounds,
                ),
                (paint_cache.ranges.glyphs[row].clone(), &cached.glyphs),
            ] {
                if !range.is_empty() {
                    scene.paint_range_mut(id, range).copy_from_slice(fragment);
                }
                work.scene_range_writes += 1;
            }
        }
        return;
    }
    let pd = scene.paint_mut(id);
    for &row in dirty.iter().rev().filter(|&&row| row < rows) {
        replace_range(
            pd,
            paint_cache.ranges.glyphs[row].clone(),
            &paint_cache.rows[row].glyphs,
        );
        work.scene_range_writes += 1;
    }
    for &row in dirty.iter().rev().filter(|&&row| row < rows) {
        replace_range(
            pd,
            paint_cache.ranges.backgrounds[row].clone(),
            &paint_cache.rows[row].backgrounds,
        );
        work.scene_range_writes += 1;
    }
    // Equal-length rewrites (the common echo/redraw case) leave every range
    // where it was, so only a length change pays the O(rows) reindex.
    if lengths_changed {
        paint_cache.reindex();
    }
}

fn emit_one(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    rect: Rect,
) {
    runtime.terminal_grid.with(|grids| {
        let mut grids = grids.borrow_mut();
        let Some(state) = grids.get_mut(id) else {
            return;
        };
        let full = state.full_paint_dirty || state.last_emit != Some(rect);
        let rows_dirty = !state.dirty_rows.is_empty();
        if !full && !rows_dirty && !state.images_dirty && !state.cursor_dirty {
            return;
        }

        let mut work = TerminalPaintWork::default();
        if full {
            state.paint_cache.backdrop = Some(Primitive::SolidRect {
                // Covers all assigned space, including sub-cell remainder at the
                // right/bottom.
                rect,
                color: state.model.background,
                corner_radius: 0.0,
            });
            rebuild_all_rows(state, shaper, atlas, rect, &mut work);
            rebuild_images(scene, state, rect);
            rebuild_cursor(state, rect);
            state.paint_cache.reindex();
            compose_full(scene.paint_mut(id), &state.paint_cache);
            work.full_rebuilds = 1;
        } else {
            if rows_dirty {
                let lengths_changed = rebuild_dirty_rows(state, shaper, atlas, rect, &mut work);
                patch_dirty_rows(scene, id, state, lengths_changed, &mut work);
            }

            if state.images_dirty {
                rebuild_images(scene, state, rect);
                // Positive images are later in the display list, so replace them
                // before negative images can shift their range.
                {
                    let pd = scene.paint_mut(id);
                    replace_range(
                        pd,
                        state.paint_cache.ranges.positive_images.clone(),
                        &state.paint_cache.positive_images,
                    );
                    work.scene_range_writes += 1;
                    replace_range(
                        pd,
                        state.paint_cache.ranges.negative_images.clone(),
                        &state.paint_cache.negative_images,
                    );
                    work.scene_range_writes += 1;
                }
                state.paint_cache.reindex();
            }

            if state.cursor_dirty {
                rebuild_cursor(state, rect);
                let range = state.paint_cache.ranges.cursor.clone();
                if range.len() == state.paint_cache.cursor.len() {
                    if !range.is_empty() {
                        scene
                            .paint_range_mut(id, range.clone())
                            .copy_from_slice(&state.paint_cache.cursor);
                    }
                } else {
                    replace_range(
                        scene.paint_mut(id),
                        range.clone(),
                        &state.paint_cache.cursor,
                    );
                }
                work.scene_range_writes += 1;
                // The cursor is the last section, so its own range is the only
                // one a length change moves.
                state.paint_cache.ranges.cursor =
                    range.start..range.start + state.paint_cache.cursor.len();
            }
        }

        state.last_emit = Some(rect);
        state.full_paint_dirty = false;
        state.dirty_rows.clear();
        state.images_dirty = false;
        state.cursor_dirty = false;
        state.last_work = work;
        // Every write above marked the node paint-dirty itself, either as whole
        // (`paint_mut`) or as recorded ranges (`paint_range_mut`). A blanket
        // `mark_dirty` here would erase the ranges.
    });
}

/// Emits all grids whose model or laid-out rectangle changed. Called by the
/// standard paint pass after any required layout has settled.
pub(crate) fn emit_terminal_grids(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    let ids = runtime.terminal_grid.take_ids(|_| true);
    for &id in &ids {
        let Some(rect) = scene.layout(id).map(|layout| layout.rect) else {
            continue;
        };
        if !rect.is_empty() {
            emit_one(runtime, scene, shaper, atlas, id, rect);
        }
    }
    runtime.terminal_grid.return_ids(ids);
}

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::layout::LayoutEngine;
    use schnellui::scene::LayoutBox;

    fn build_grid(
        runtime: &crate::widgets::Runtime,
        grid: TerminalGrid,
        rect: Rect,
    ) -> (Scene, TextShaper, GlyphAtlas, WidgetId) {
        crate::widgets::reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(1024, 1024);
        let id = {
            let mut ctx = BuildCtx {
                context: crate::widgets::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(grid).build(&mut ctx, None)
        };
        scene.set_root(id);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        (scene, text, atlas, id)
    }

    #[test]
    fn paints_exact_backgrounds_selection_cursor_and_fixed_glyphs() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(10.0, 20.0, 14.0);
        let mut model = TerminalGridModel::with_colors(2, 1, Color::WHITE, Color::rgb(3, 4, 5));
        model.set_cell(
            TerminalGridPoint::new(0, 0),
            TerminalCell::new("A", Color::rgb(220, 220, 220), Color::rgb(10, 20, 30)),
        );
        model.selection = Some(TerminalSelection {
            start: TerminalGridPoint::new(0, 1),
            end: TerminalGridPoint::new(0, 1),
            background: Color::rgb(40, 50, 60),
            foreground: None,
        });
        model.cursor = Some(TerminalCursor::new(
            TerminalGridPoint::new(0, 0),
            Color::rgb(70, 80, 90),
        ));
        let rect = Rect::new(5.0, 7.0, 27.0, 24.0);
        let (mut scene, mut text, mut atlas, id) =
            build_grid(runtime, TerminalGrid::new(model).metrics(metrics), rect);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);

        let primitives = &scene.paint(id).unwrap().primitives;
        assert_eq!(
            primitives[0],
            Primitive::SolidRect {
                rect,
                color: Color::rgb(3, 4, 5),
                corner_radius: 0.0,
            }
        );
        assert_eq!(
            primitives[1],
            Primitive::SolidRect {
                rect: Rect::new(5.0, 7.0, 10.0, 20.0),
                color: Color::rgb(70, 80, 90),
                corner_radius: 0.0,
            }
        );
        assert_eq!(
            primitives[2],
            Primitive::SolidRect {
                rect: Rect::new(15.0, 7.0, 10.0, 20.0),
                color: Color::rgb(40, 50, 60),
                corner_radius: 0.0,
            }
        );
        assert!(primitives
            .iter()
            .any(|primitive| matches!(primitive, Primitive::GlyphQuad { rect, .. } if rect.x >= 5.0 && rect.x < 15.0)));
    }

    #[test]
    fn block_elements_fill_their_cells_without_glyphs() {
        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(10.0, 20.0, 14.0);
        let mut model = TerminalGridModel::with_colors(3, 1, Color::WHITE, Color::BLACK);
        let red = Color::rgb(200, 0, 0);
        for (column, block) in ["█", "▛", "▒"].into_iter().enumerate() {
            model.set_cell(
                TerminalGridPoint::new(0, column),
                TerminalCell::new(block, red, Color::BLACK),
            );
        }
        let rect = Rect::new(0.0, 0.0, 30.0, 20.0);
        let (mut scene, mut text, mut atlas, id) =
            build_grid(runtime, TerminalGrid::new(model).metrics(metrics), rect);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);

        let primitives = &scene.paint(id).unwrap().primitives;
        assert!(!primitives.iter().any(
            |primitive| matches!(primitive, Primitive::GlyphQuad { color, .. } if color.a != 0)
        ));
        let painted: Vec<(Rect, Color)> = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::SolidRect { rect, color, .. }
                    if *color != Color::BLACK && color.a != 0 =>
                {
                    Some((*rect, *color))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            painted,
            [
                (Rect::new(0.0, 0.0, 10.0, 20.0), red),
                (Rect::new(10.0, 0.0, 5.0, 10.0), red),
                (Rect::new(15.0, 0.0, 5.0, 10.0), red),
                (Rect::new(10.0, 10.0, 5.0, 10.0), red),
                (
                    Rect::new(20.0, 0.0, 10.0, 20.0),
                    color_with_alpha(red, 2, 4)
                ),
            ]
        );
    }

    #[test]
    fn paints_nerd_and_standard_prompt_fallback_glyphs() {
        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(12.0, 24.0, 18.0);
        let symbols = [
            '\u{e0b0}',
            '\u{ea85}',
            '\u{f0a9e}',
            '\u{21e1}',
            '\u{21e3}',
            '\u{276f}',
        ];
        let mut model =
            TerminalGridModel::with_colors(symbols.len(), 1, Color::WHITE, Color::BLACK);
        for (column, symbol) in symbols.into_iter().enumerate() {
            model.set_cell(
                TerminalGridPoint::new(0, column),
                TerminalCell::new(symbol.to_string(), Color::WHITE, Color::BLACK),
            );
        }
        let rect = Rect::new(
            0.0,
            0.0,
            metrics.width * symbols.len() as f32,
            metrics.height,
        );
        let (mut scene, mut text, mut atlas, id) = build_grid(
            runtime,
            TerminalGrid::new(model)
                .font_family("Liberation Mono")
                .metrics(metrics),
            rect,
        );

        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);

        let glyphs = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter(
                |primitive| matches!(primitive, Primitive::GlyphQuad { color, .. } if color.a != 0),
            )
            .count();
        assert_eq!(glyphs, symbols.len());
    }

    #[test]
    fn cell_background_edges_share_physical_pixel_boundaries() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(7.68, 15.36, 12.0);
        let mut model =
            TerminalGridModel::with_colors(3, 2, Color::WHITE, Color::rgb(102, 102, 102));
        for row in 0..2 {
            for column in 0..3 {
                model
                    .cell_mut(TerminalGridPoint::new(row, column))
                    .unwrap()
                    .background = Color::rgb((row * 3 + column + 1) as u8, 0, 0);
            }
        }
        let rect = Rect::new(20.1, 5.1, 30.0, 40.0);
        let (mut scene, mut text, mut atlas, id) =
            build_grid(runtime, TerminalGrid::new(model).metrics(metrics), rect);
        runtime
            .terminal_grid
            .with(|grids| grids.borrow_mut().get_mut(id).unwrap().scale = 1.25);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);

        // Skip the backdrop and the rows' invisible padding.
        let backgrounds: Vec<Rect> = scene.paint(id).unwrap().primitives[1..]
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::SolidRect { rect, color, .. } if color.a != 0 => Some(*rect),
                _ => None,
            })
            .take(6)
            .collect();
        assert_eq!(backgrounds.len(), 6);

        assert_eq!(backgrounds[0].right(), backgrounds[1].x);
        assert_eq!(backgrounds[1].right(), backgrounds[2].x);
        assert_eq!(backgrounds[0].bottom(), backgrounds[3].y);
        for background in backgrounds {
            for edge in [
                background.x,
                background.y,
                background.right(),
                background.bottom(),
            ] {
                assert!((edge * 1.25 - (edge * 1.25).round()).abs() < f32::EPSILON);
            }
        }
    }

    #[test]
    fn default_background_is_one_primitive_instead_of_one_per_cell() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(8.0, 16.0, 13.0);
        let model = TerminalGridModel::with_colors(80, 24, Color::WHITE, Color::BLACK);
        let rect = Rect::new(0.0, 0.0, 640.0, 384.0);
        let (mut scene, mut text, mut atlas, id) =
            build_grid(runtime, TerminalGrid::new(model).metrics(metrics), rect);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);

        assert_eq!(scene.paint(id).unwrap().primitives.len(), 1);
    }

    #[test]
    fn point_mapping_and_wide_continuation_resolve_hyperlink() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(8.0, 16.0, 13.0);
        let mut model = TerminalGridModel::new(3, 2);
        let mut owner = TerminalCell::new("界", Color::WHITE, Color::BLACK);
        owner.width = TerminalCellWidth::Wide;
        owner.hyperlink = Some("https://example.test".into());
        model.set_cell(TerminalGridPoint::new(1, 1), owner);
        model.cell_mut(TerminalGridPoint::new(1, 2)).unwrap().width =
            TerminalCellWidth::Continuation;
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
        let (scene, _text, _atlas, id) =
            build_grid(runtime, TerminalGrid::new(model).metrics(metrics), rect);
        let hit = terminal_grid_hit_test(runtime, &scene, id, Point { x: 29.0, y: 37.0 }).unwrap();
        assert_eq!(hit.position, TerminalGridPoint::new(1, 2));
        assert_eq!(hit.hyperlink.as_deref(), Some("https://example.test"));
        assert_eq!(
            metrics.point_to_cell(rect, Point { x: 39.0, y: 21.0 }, 3, 2),
            None,
            "right-side remainder is not a terminal cell"
        );
    }

    #[test]
    fn image_placement_uses_cell_geometry_and_reuses_atlas_slot() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(9.0, 18.0, 14.0);
        let mut model = TerminalGridModel::new(4, 3);
        model.images.push(TerminalImage::new(
            42,
            1,
            1,
            vec![255, 0, 0, 255],
            TerminalImagePlacement::Cells {
                row: 1,
                column: 2,
                rows: 2,
                columns: 2,
            },
        ));
        let rect = Rect::new(3.0, 5.0, 36.0, 54.0);
        let (mut scene, mut text, mut atlas, id) =
            build_grid(runtime, TerminalGrid::new(model).metrics(metrics), rect);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        let revision = scene.images().revision();
        assert!(scene.paint(id).unwrap().primitives.iter().any(|primitive| {
            matches!(primitive, Primitive::ImageQuad { rect, .. }
                if *rect == Rect::new(21.0, 23.0, 18.0, 36.0))
        }));
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(scene.images().revision(), revision);
    }

    #[test]
    fn layout_fills_available_width_and_keeps_exact_row_height() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        crate::widgets::reset(runtime);
        let metrics = TerminalCellMetrics::new(9.0, 18.0, 14.0);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(256, 256);
        let id = {
            let mut ctx = BuildCtx {
                context: crate::widgets::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(TerminalGrid::new(TerminalGridModel::new(5, 3)).metrics(metrics))
                .build(&mut ctx, None)
        };
        scene.set_root(id);
        layout.sync_tree(&scene, id);
        layout.compute(
            &mut scene,
            id,
            Size {
                width: 100.0,
                height: 200.0,
            },
        );
        let rect = scene.layout(id).unwrap().rect;
        assert_eq!(rect.width, 100.0);
        assert_eq!(rect.height, 54.0);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert!(matches!(
            scene.paint(id).unwrap().primitives.first(),
            Some(Primitive::SolidRect { rect: painted, .. }) if *painted == rect
        ));
    }

    #[test]
    fn dynamic_grid_updates_semantics_and_deferred_paint() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let color = schnellui::signal::create_signal(Color::rgb(10, 20, 30));
        let source = move || {
            let background = color.get();
            let mut model = TerminalGridModel::with_colors(1, 1, Color::WHITE, background);
            model.set_cell(
                TerminalGridPoint::new(0, 0),
                TerminalCell::new("X", Color::WHITE, background),
            );
            model
        };
        let rect = Rect::new(0.0, 0.0, 10.0, 20.0);
        let metrics = TerminalCellMetrics::new(10.0, 20.0, 14.0);
        let (mut scene, mut text, mut atlas, id) = build_grid(
            runtime,
            TerminalGrid::dynamic(source).metrics(metrics),
            rect,
        );
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        scene.clear_dirty();

        color.set(Color::rgb(40, 50, 60));
        schnellui::signal::Runtime::flush();
        crate::widgets::run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);
        assert!(!scene.dirty_flags(id).contains(DirtyFlags::LAYOUT));
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("X"));
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert!(scene.dirty_flags(id).contains(DirtyFlags::PAINT));
        assert!(matches!(
            scene.paint(id).unwrap().primitives.first(),
            Some(Primitive::SolidRect { color, .. }) if *color == Color::rgb(40, 50, 60)
        ));
    }

    #[test]
    fn unrelated_signal_does_not_invoke_a_dynamic_grid() {
        use std::cell::Cell;

        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let columns = schnellui::signal::create_signal(2_usize);
        let unrelated = schnellui::signal::create_signal(false);
        let calls = Rc::new(Cell::new(0_u32));
        let source_calls = calls.clone();
        let grid = TerminalGrid::dynamic(move || {
            source_calls.set(source_calls.get() + 1);
            TerminalGridModel::new(columns.get(), 1)
        })
        .metrics(TerminalCellMetrics::new(10.0, 20.0, 14.0));
        let (mut scene, mut text, mut atlas, _id) =
            build_grid(runtime, grid, Rect::new(0.0, 0.0, 20.0, 20.0));
        assert_eq!(calls.get(), 1);

        unrelated.set(true);
        schnellui::signal::Runtime::flush();
        crate::widgets::run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);

        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn versioned_grid_skips_model_work_for_unrelated_signals() {
        use std::cell::Cell;

        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let revision = schnellui::signal::create_signal(0_u64);
        let unrelated = schnellui::signal::create_signal(false);
        let model_calls = Rc::new(Cell::new(0_u32));
        let calls = model_calls.clone();
        let grid = TerminalGrid::dynamic_versioned(
            move || revision.get(),
            move || {
                calls.set(calls.get() + 1);
                TerminalGridModel::new(80, 24)
            },
        );
        let rect = Rect::new(0.0, 0.0, 640.0, 384.0);
        let (_scene, _text, _atlas, _id) = build_grid(runtime, grid, rect);
        assert_eq!(model_calls.get(), 1);

        unrelated.set(true);
        let mut scene = _scene;
        poll_dynamic_sources(runtime, &mut scene);
        assert_eq!(model_calls.get(), 1);

        revision.set(1);
        poll_dynamic_sources(runtime, &mut scene);
        assert_eq!(model_calls.get(), 2);
    }

    #[test]
    fn dynamic_dimension_change_is_the_only_terminal_layout_invalidation() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let columns = schnellui::signal::create_signal(2_usize);
        let source = move || TerminalGridModel::new(columns.get(), 1);
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let metrics = TerminalCellMetrics::new(10.0, 20.0, 14.0);
        let (mut scene, mut text, mut atlas, id) = build_grid(
            runtime,
            TerminalGrid::dynamic(source).metrics(metrics),
            rect,
        );
        scene.clear_dirty();

        columns.set(3);
        schnellui::signal::Runtime::flush();
        crate::widgets::run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);
        assert!(scene.dirty_flags(id).contains(DirtyFlags::LAYOUT));
    }

    #[test]
    fn one_cell_update_rebuilds_one_row_and_matches_a_forced_full_emit() {
        use std::cell::Cell;

        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(8.0, 16.0, 13.0);
        let revision = Rc::new(Cell::new(0_u64));
        let model = Rc::new(RefCell::new(TerminalGridModel::new(80, 24)));
        model.borrow_mut().set_cell(
            TerminalGridPoint::new(12, 40),
            TerminalCell::new("A", Color::WHITE, Color::BLACK),
        );
        let revision_source = revision.clone();
        let model_source = model.clone();
        let grid = TerminalGrid::dynamic_versioned(
            move || revision_source.get(),
            move || model_source.borrow().clone(),
        )
        .metrics(metrics);
        let rect = Rect::new(0.0, 0.0, 640.0, 384.0);
        let (mut scene, mut text, mut atlas, id) = build_grid(runtime, grid, rect);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        scene.clear_dirty();

        model.borrow_mut().set_cell(
            TerminalGridPoint::new(12, 40),
            TerminalCell::new("B", Color::WHITE, Color::BLACK),
        );
        revision.set(1);
        poll_dynamic_sources(runtime, &mut scene);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);

        let incremental = scene.paint(id).unwrap().primitives.clone();
        let work = runtime.terminal_grid.with(|grids| {
            grids
                .borrow()
                .get(id)
                .expect("terminal remains mounted")
                .last_work
        });
        assert_eq!(work.full_rebuilds, 0);
        assert_eq!(work.background_rows, 1);
        assert_eq!(work.glyph_rows, 1);
        assert_eq!(work.cells, 80);
        assert_eq!(work.scene_range_writes, 2);

        // The incremental path alters only cached ranges. Force the ordinary full
        // path afterwards and compare the generic scene display list byte-for-byte.
        runtime.terminal_grid.with(|grids| {
            let mut grids = grids.borrow_mut();
            let state = grids.get_mut(id).unwrap();
            state.mark_all_dirty();
        });
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(scene.paint(id).unwrap().primitives, incremental);
    }

    #[test]
    fn row_delta_with_removed_background_and_added_glyph_matches_full_emit() {
        use std::cell::Cell;

        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(8.0, 16.0, 13.0);
        let revision = Rc::new(Cell::new(0_u64));
        let model = Rc::new(RefCell::new(TerminalGridModel::new(3, 1)));
        model
            .borrow_mut()
            .cell_mut(TerminalGridPoint::new(0, 0))
            .unwrap()
            .background = Color::rgb(80, 20, 20);
        let revision_source = revision.clone();
        let model_source = model.clone();
        let grid = TerminalGrid::dynamic_versioned(
            move || revision_source.get(),
            move || model_source.borrow().clone(),
        )
        .metrics(metrics);
        let rect = Rect::new(0.0, 0.0, 24.0, 16.0);
        let (mut scene, mut text, mut atlas, id) = build_grid(runtime, grid, rect);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        scene.clear_dirty();

        // One cell removes a non-default background run while adding a glyph. The
        // two display-list sections change length in opposite directions.
        model.borrow_mut().set_cell(
            TerminalGridPoint::new(0, 0),
            TerminalCell::new("A", Color::WHITE, Color::BLACK),
        );
        revision.set(1);
        poll_dynamic_sources(runtime, &mut scene);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        let incremental = scene.paint(id).unwrap().primitives.clone();

        runtime.terminal_grid.with(|grids| {
            let mut grids = grids.borrow_mut();
            let state = grids.get_mut(id).unwrap();
            state.mark_all_dirty();
        });
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(scene.paint(id).unwrap().primitives, incremental);
    }

    #[test]
    fn cursor_move_rebuilds_one_row_and_cursor_fragment() {
        use std::cell::Cell;

        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(8.0, 16.0, 13.0);
        let revision = Rc::new(Cell::new(0_u64));
        let model = Rc::new(RefCell::new(TerminalGridModel::new(80, 24)));
        let revision_source = revision.clone();
        let model_source = model.clone();
        let grid = TerminalGrid::dynamic_versioned(
            move || revision_source.get(),
            move || model_source.borrow().clone(),
        )
        .metrics(metrics);
        let rect = Rect::new(0.0, 0.0, 640.0, 384.0);
        let (mut scene, mut text, mut atlas, id) = build_grid(runtime, grid, rect);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        scene.clear_dirty();

        let mut cursor = TerminalCursor::new(TerminalGridPoint::new(9, 6), Color::WHITE);
        cursor.style = TerminalCursorStyle::Bar;
        model.borrow_mut().cursor = Some(cursor);
        revision.set(1);
        poll_dynamic_sources(runtime, &mut scene);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);

        let work = runtime.terminal_grid.with(|grids| {
            grids
                .borrow()
                .get(id)
                .expect("terminal remains mounted")
                .last_work
        });
        assert_eq!(work.full_rebuilds, 0);
        assert_eq!(work.background_rows, 1);
        assert_eq!(work.glyph_rows, 1);
        assert_eq!(work.cells, 80);
        assert_eq!(work.scene_range_writes, 3);
    }

    /// Shared host side for patched-grid tests: the next patch applies `pending`.
    type Pending = Rc<RefCell<Vec<Box<dyn FnOnce(&mut TerminalGridPatch<'_>)>>>>;

    fn patched_grid(
        revision: &Rc<std::cell::Cell<u64>>,
        pending: &Pending,
        calls: &Rc<std::cell::Cell<u32>>,
    ) -> TerminalGrid {
        let revision = revision.clone();
        let pending = pending.clone();
        let calls = calls.clone();
        TerminalGrid::patched(
            move || revision.get(),
            move |patch| {
                calls.set(calls.get() + 1);
                for edit in pending.borrow_mut().drain(..) {
                    edit(patch);
                }
            },
        )
        .metrics(TerminalCellMetrics::new(8.0, 16.0, 13.0))
    }

    fn last_work(runtime: &crate::widgets::Runtime, id: WidgetId) -> TerminalPaintWork {
        runtime
            .terminal_grid
            .with(|grids| grids.borrow().get(id).expect("mounted").last_work)
    }

    fn model_text(runtime: &crate::widgets::Runtime, id: WidgetId) -> String {
        runtime
            .terminal_grid
            .with(|grids| grids.borrow().get(id).expect("mounted").model.plain_text())
    }

    fn write_row(text: &'static str, row: usize) -> Box<dyn FnOnce(&mut TerminalGridPatch<'_>)> {
        Box::new(move |patch| {
            let cells = patch.row_mut(row);
            for (cell, grapheme) in cells.iter_mut().zip(text.split_inclusive(|_| true)) {
                cell.grapheme = grapheme.into();
            }
        })
    }

    #[test]
    fn compact_cells_keep_typical_graphemes_inline() {
        let cell = TerminalCell::new("e\u{301}", Color::WHITE, Color::BLACK);
        assert!(!cell.grapheme.is_heap_allocated());
        let flag = "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}"; // 14 bytes
        let long = "a\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}\u{307}\u{308}\u{309}\u{30a}a";
        assert!(long.len() >= 22);
        for grapheme in [flag, long] {
            assert!(
                !CompactString::from(grapheme).is_heap_allocated(),
                "{grapheme}"
            );
        }
        let mut linked = cell.clone();
        linked.hyperlink = Some(Arc::from("https://example.test"));
        let copy = linked.clone();
        assert!(Arc::ptr_eq(
            linked.hyperlink.as_ref().unwrap(),
            copy.hyperlink.as_ref().unwrap()
        ));
    }

    #[test]
    fn patched_grid_fills_at_build_and_rebuilds_only_written_rows() {
        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let revision = Rc::new(std::cell::Cell::new(0_u64));
        let pending: Pending = Rc::default();
        let calls = Rc::new(std::cell::Cell::new(0_u32));
        pending
            .borrow_mut()
            .push(Box::new(|patch| patch.resize(80, 24)));
        pending.borrow_mut().push(write_row("hello", 3));
        let grid = patched_grid(&revision, &pending, &calls);
        let rect = Rect::new(0.0, 0.0, 640.0, 384.0);
        let (mut scene, mut text, mut atlas, id) = build_grid(runtime, grid, rect);
        assert_eq!(calls.get(), 1, "the initial patch runs at build");
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        scene.clear_dirty();
        assert_eq!(
            scene.a11y(id).unwrap().value.as_deref(),
            Some(model_text(runtime, id).as_str())
        );

        // An unchanged revision never runs the patch.
        poll_dynamic_sources(runtime, &mut scene);
        assert_eq!(calls.get(), 1);

        pending.borrow_mut().push(write_row("world", 12));
        revision.set(1);
        poll_dynamic_sources(runtime, &mut scene);
        assert_eq!(calls.get(), 2);
        assert!(!scene.dirty_flags(id).contains(DirtyFlags::LAYOUT));
        assert!(scene.dirty_flags(id).contains(DirtyFlags::A11Y));
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert!(scene.dirty_flags(id).contains(DirtyFlags::PAINT));
        // Glyphs were added to a blank row, so the node is dirty as a whole.
        assert_eq!(scene.paint_dirty_ranges(id), None);
        let work = last_work(runtime, id);
        assert_eq!(work.full_rebuilds, 0);
        assert_eq!(work.glyph_rows, 1);
        assert_eq!(work.cells, 80);
        assert_eq!(work.scene_range_writes, 2);
        let value = scene.a11y(id).unwrap().value.clone().unwrap();
        assert_eq!(value, model_text(runtime, id));
        assert_eq!(value.lines().nth(12).unwrap().trim_end(), "world");

        let incremental = scene.paint(id).unwrap().primitives.clone();
        runtime.terminal_grid.with(|grids| {
            grids.borrow_mut().get_mut(id).unwrap().mark_all_dirty();
        });
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(scene.paint(id).unwrap().primitives, incremental);
    }

    #[test]
    fn patched_rewrite_with_identical_text_leaves_a11y_clean() {
        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let revision = Rc::new(std::cell::Cell::new(0_u64));
        let pending: Pending = Rc::default();
        let calls = Rc::new(std::cell::Cell::new(0_u32));
        pending
            .borrow_mut()
            .push(Box::new(|patch| patch.resize(10, 3)));
        pending.borrow_mut().push(write_row("same", 1));
        let grid = patched_grid(&revision, &pending, &calls);
        let (mut scene, mut text, mut atlas, id) =
            build_grid(runtime, grid, Rect::new(0.0, 0.0, 80.0, 48.0));
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        scene.clear_dirty();

        pending.borrow_mut().push(write_row("same", 1));
        revision.set(1);
        poll_dynamic_sources(runtime, &mut scene);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert!(scene.dirty_flags(id).contains(DirtyFlags::PAINT));
        assert!(!scene.dirty_flags(id).contains(DirtyFlags::A11Y));
        // An equal-length row rewrite reaches the renderer as ranges: this row
        // has no background runs, so only its glyph range is recorded.
        assert_eq!(scene.paint_dirty_ranges(id).map(<[_]>::len), Some(1));
    }

    #[test]
    fn patched_a11y_splice_handles_multibyte_rows_in_any_order() {
        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let revision = Rc::new(std::cell::Cell::new(0_u64));
        let pending: Pending = Rc::default();
        let calls = Rc::new(std::cell::Cell::new(0_u32));
        pending
            .borrow_mut()
            .push(Box::new(|patch| patch.resize(6, 4)));
        let grid = patched_grid(&revision, &pending, &calls);
        let (mut scene, mut text, mut atlas, id) =
            build_grid(runtime, grid, Rect::new(0.0, 0.0, 48.0, 64.0));
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);

        for (step, edits) in [
            vec![write_row("äöü", 2), write_row("ab", 0)],
            vec![
                write_row("界界", 0),
                write_row("x", 3),
                write_row("abcdef", 2),
            ],
            vec![write_row("      ", 0)],
        ]
        .into_iter()
        .enumerate()
        {
            pending.borrow_mut().extend(edits);
            revision.set(step as u64 + 1);
            poll_dynamic_sources(runtime, &mut scene);
            assert_eq!(
                scene.a11y(id).unwrap().value.as_deref(),
                Some(model_text(runtime, id).as_str()),
                "step {step}"
            );
        }
    }

    #[test]
    fn patched_resize_relayouts_and_cursor_move_dirties_two_rows() {
        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let revision = Rc::new(std::cell::Cell::new(0_u64));
        let pending: Pending = Rc::default();
        let calls = Rc::new(std::cell::Cell::new(0_u32));
        pending
            .borrow_mut()
            .push(Box::new(|patch| patch.resize(20, 5)));
        pending.borrow_mut().push(write_row("keep", 0));
        let grid = patched_grid(&revision, &pending, &calls);
        let (mut scene, mut text, mut atlas, id) =
            build_grid(runtime, grid, Rect::new(0.0, 0.0, 160.0, 80.0));
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        scene.clear_dirty();

        pending
            .borrow_mut()
            .push(Box::new(|patch| patch.resize(30, 6)));
        revision.set(1);
        poll_dynamic_sources(runtime, &mut scene);
        assert!(scene.dirty_flags(id).contains(DirtyFlags::LAYOUT));
        let value = scene.a11y(id).unwrap().value.clone().unwrap();
        assert_eq!(value.lines().count(), 6);
        assert!(value.starts_with("keep"));
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        scene.clear_dirty();

        let mut cursor = TerminalCursor::new(TerminalGridPoint::new(1, 2), Color::WHITE);
        cursor.style = TerminalCursorStyle::Bar;
        pending
            .borrow_mut()
            .push(Box::new(move |patch| patch.set_cursor(Some(cursor))));
        revision.set(2);
        poll_dynamic_sources(runtime, &mut scene);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(last_work(runtime, id).glyph_rows, 1);
        assert!(!scene.dirty_flags(id).contains(DirtyFlags::A11Y));
        scene.clear_dirty();

        cursor.position = TerminalGridPoint::new(4, 0);
        pending
            .borrow_mut()
            .push(Box::new(move |patch| patch.set_cursor(Some(cursor))));
        revision.set(3);
        poll_dynamic_sources(runtime, &mut scene);
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        let work = last_work(runtime, id);
        assert_eq!(work.full_rebuilds, 0);
        assert_eq!(work.glyph_rows, 2);

        // Re-setting the same cursor is free.
        pending
            .borrow_mut()
            .push(Box::new(move |patch| patch.set_cursor(Some(cursor))));
        revision.set(4);
        scene.clear_dirty();
        poll_dynamic_sources(runtime, &mut scene);
        assert!(scene.dirty_flags(id).is_empty());
    }

    #[test]
    fn typing_into_a_row_keeps_its_primitive_count_and_reports_ranges() {
        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let revision = Rc::new(std::cell::Cell::new(0_u64));
        let pending: Pending = Rc::default();
        let calls = Rc::new(std::cell::Cell::new(0_u32));
        pending
            .borrow_mut()
            .push(Box::new(|patch| patch.resize(40, 4)));
        pending.borrow_mut().push(write_row("$ ", 1));
        let grid = patched_grid(&revision, &pending, &calls);
        let (mut scene, mut text, mut atlas, id) =
            build_grid(runtime, grid, Rect::new(0.0, 0.0, 320.0, 64.0));
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        scene.clear_dirty();
        let total = scene.paint(id).unwrap().primitives.len();

        // Each typed character adds a glyph inside the row's spare capacity.
        for (step, typed) in ["$ l", "$ ls", "$ ls "].into_iter().enumerate() {
            pending.borrow_mut().push(write_row(typed, 1));
            revision.set(step as u64 + 1);
            poll_dynamic_sources(runtime, &mut scene);
            emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
            assert_eq!(scene.paint(id).unwrap().primitives.len(), total, "{typed}");
            assert!(scene.paint_dirty_ranges(id).is_some(), "{typed}: ranged");
            scene.clear_dirty();
        }
        let incremental = scene.paint(id).unwrap().primitives.clone();
        runtime.terminal_grid.with(|grids| {
            grids.borrow_mut().get_mut(id).unwrap().mark_all_dirty();
        });
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(scene.paint(id).unwrap().primitives, incremental);
    }
}
