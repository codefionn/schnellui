//! A cell-accurate terminal surface.
//!
//! Unlike [`crate::RichText`], this widget never reflows or concatenates the
//! terminal contents. Every background and grapheme is painted at its retained
//! row/column, so blank cells, wide glyph occupancy, cursor geometry and selection
//! overlays survive the trip from a terminal emulator to the renderer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use schnellui_a11y::{ActionFlags, Role};
use schnellui_scene::{
    Color, DirtyFlags, PaintData, Point, Primitive, Rect, Scene, Size, TexelRect, WidgetId,
    WidgetKind,
};
use schnellui_text::{FontFace, GlyphAtlas, ShapeOptions, ShapedText, TextShaper, WrapMode};
use slotmap::SecondaryMap;

mod interaction;
use crate::{norm_scale, phys_size_px, rasterize_and_push, BuildCtx, View};
pub use interaction::*;

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCell {
    pub grapheme: String,
    pub foreground: Color,
    pub background: Color,
    pub attrs: TerminalCellAttrs,
    pub width: TerminalCellWidth,
    /// OSC 8 target associated with this cell, if any.
    pub hyperlink: Option<String>,
}

impl TerminalCell {
    pub fn blank(foreground: Color, background: Color) -> Self {
        Self {
            grapheme: String::new(),
            foreground,
            background,
            attrs: TerminalCellAttrs::default(),
            width: TerminalCellWidth::Single,
            hyperlink: None,
        }
    }

    pub fn new(grapheme: impl Into<String>, foreground: Color, background: Color) -> Self {
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

    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        for row in 0..self.rows {
            if row > 0 {
                text.push('\n');
            }
            for column in 0..self.columns {
                let point = TerminalGridPoint::new(row, column);
                match self.cell(point) {
                    Some(cell) if cell.width == TerminalCellWidth::Continuation => {}
                    Some(cell) if !cell.grapheme.is_empty() => text.push_str(&cell.grapheme),
                    _ => text.push(' '),
                }
            }
        }
        text
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

enum TerminalGridSource {
    Static(TerminalGridModel),
    Dynamic(GridFn),
    Versioned {
        revision: GridRevisionFn,
        model: GridFn,
    },
}

enum DynamicTerminalGridSource {
    Unversioned(GridFn),
    Versioned {
        revision: GridRevisionFn,
        observed_revision: u64,
        model: GridFn,
    },
}

/// A fixed-cell terminal viewport with full-background paint and ScrollView
/// semantics. Use [`TerminalGrid::dynamic`] for a signal-backed model and
/// [`TerminalGrid::dynamic_versioned`] for a PTY/host-backed revision source.
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

struct TerminalGridState {
    model: TerminalGridModel,
    metrics: TerminalCellMetrics,
    font_family: Option<String>,
    scale: f32,
    source: Option<DynamicTerminalGridSource>,
    measured: Rc<RefCell<Size>>,
    image_cache: HashMap<u64, CachedImage>,
    glyph_shape_cache: HashMap<FontFace, HashMap<String, ShapedText>>,
    paint_cache: TerminalPaintCache,
    /// Rows whose model-dependent cell fragments need rebuilding. A row is the
    /// smallest unit that preserves merged background runs without scanning or
    /// rewriting neighboring rows.
    dirty_rows: Vec<bool>,
    images_dirty: bool,
    cursor_dirty: bool,
    full_paint_dirty: bool,
    last_work: TerminalPaintWork,
    last_emit: Option<Rect>,
}

#[derive(Default)]
struct TerminalGridRuntime {
    grids: RefCell<SecondaryMap<WidgetId, TerminalGridState>>,
    /// Reused because grid callbacks must run without a registry borrow held.
    key_scratch: Vec<WidgetId>,
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

    fn take_ids(&self, include: impl Fn(&TerminalGridState) -> bool) -> Vec<WidgetId> {
        let mut runtime = self.0.borrow_mut();
        let mut ids = std::mem::take(&mut runtime.key_scratch);
        ids.extend(
            runtime
                .grids
                .borrow()
                .iter()
                .filter(|(_, state)| include(state))
                .map(|(id, _)| id),
        );
        ids
    }

    fn return_ids(&self, mut ids: Vec<WidgetId>) {
        ids.clear();
        self.0.borrow_mut().key_scratch = ids;
    }
}

pub(crate) fn purge_nodes(runtime: &crate::Runtime, nodes: &[WidgetId]) {
    runtime.terminal_grid.with(|grids| {
        let mut grids = grids.borrow_mut();
        for &id in nodes {
            grids.remove(id);
        }
    });
    runtime
        .terminal_grid
        .0
        .borrow_mut()
        .key_scratch
        .retain(|id| !nodes.contains(id));
}

impl View for TerminalGrid {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::TerminalGrid, parent);
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
        };
        let metrics = this.metrics.unwrap_or_else(|| {
            TerminalCellMetrics::measure_with_family(
                ctx.text,
                this.font_size,
                ctx.scale,
                this.font_family.as_deref(),
            )
        });
        let measured = Rc::new(RefCell::new(metrics.grid_size(model.columns, model.rows)));
        let model_rows = model.rows;
        let measure_ref = measured.clone();
        ctx.layout
            .set_measure(id, Box::new(move |_available| *measure_ref.borrow()));
        ctx.layout.set_fill_width(id);

        let a11y = ctx.scene.a11y_mut(id);
        a11y.role = Role::ScrollView.as_u16();
        a11y.name = Some(this.label);
        a11y.value = Some(model.plain_text());
        let mut actions = ActionFlags::default();
        actions.insert(ActionFlags::FOCUS);
        actions.insert(ActionFlags::SCROLL_UP);
        actions.insert(ActionFlags::SCROLL_DOWN);
        a11y.actions = actions.0;

        ctx.runtime.terminal_grid.with(|grids| {
            grids.borrow_mut().insert(
                id,
                TerminalGridState {
                    model,
                    metrics,
                    font_family: this.font_family,
                    scale: ctx.scale,
                    source,
                    measured,
                    image_cache: HashMap::new(),
                    glyph_shape_cache: HashMap::new(),
                    paint_cache: TerminalPaintCache::default(),
                    dirty_rows: vec![true; model_rows],
                    images_dirty: true,
                    cursor_dirty: true,
                    full_paint_dirty: true,
                    last_work: TerminalPaintWork::default(),
                    last_emit: None,
                },
            );
        });
        id
    }
}

pub(crate) fn reset(runtime: &crate::Runtime) {
    runtime
        .terminal_grid
        .with(|grids| grids.borrow_mut().clear());
    runtime.terminal_grid.0.borrow_mut().key_scratch.clear();
}

pub(crate) fn contains(runtime: &crate::Runtime, id: WidgetId) -> bool {
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
        let texels = match state.image_cache.get(&image.id) {
            Some(cached) if cached.width == image.width && cached.height == image.height => {
                if state.images_dirty {
                    scene.images_mut().write_rect(cached.texels, &image.rgba);
                }
                cached.texels
            }
            _ => {
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
    images.sort_by_key(|(z, _)| *z);
    images
}

fn rebuild_background_row(
    model: &TerminalGridModel,
    metrics: TerminalCellMetrics,
    scale: f32,
    rect: Rect,
    row: usize,
    primitives: &mut Vec<Primitive>,
) {
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
}

fn rebuild_glyph_row(
    state: &mut TerminalGridState,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    rect: Rect,
    row: usize,
) {
    let scale = norm_scale(state.scale);
    let physical_size = phys_size_px(state.metrics.font_size, scale);
    let mut paint = PaintData {
        primitives: std::mem::take(&mut state.paint_cache.rows[row].glyphs),
    };
    paint.primitives.clear();

    for column in 0..state.model.columns {
        let point = TerminalGridPoint::new(row, column);
        // Clone the compact cell once. It frees the immutable model borrow before
        // touching the shape cache, which is what keeps cache hits allocation-free.
        let Some(cell) = state.model.cell(point).cloned() else {
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
        if let Some(selection) = state.model.selection.filter(|s| s.contains(point)) {
            foreground = selection.foreground.unwrap_or(foreground);
        }
        let cursor = state
            .model
            .cursor
            .filter(|cursor| cursor.position == point && cursor.paints());
        if cursor.is_some_and(|cursor| cursor.style == TerminalCursorStyle::Block) {
            foreground = background;
        }
        if cell.attrs.faint {
            foreground = color_with_alpha(foreground, 1, 2);
        }

        let face = if cell.attrs.bold {
            FontFace::MonoBold
        } else {
            // The embedded mono set has no italic face. Keeping the mono metrics
            // matters more than silently switching families.
            FontFace::Mono
        };
        let face_cache = state.glyph_shape_cache.entry(face).or_default();
        if !face_cache.contains_key(&cell.grapheme) {
            let opts = ShapeOptions::new(physical_size)
                .wrap(WrapMode::NoWrap)
                .face(face);
            let shaped = match state.font_family.as_deref() {
                Some(family) => shaper.shape_with_family(&cell.grapheme, &opts, family),
                None => shaper.shape_with(&cell.grapheme, &opts),
            };
            face_cache.insert(cell.grapheme.clone(), shaped);
        }
        let shaped = face_cache
            .get(&cell.grapheme)
            .expect("terminal shape inserted above");
        let span_columns = if cell.width == TerminalCellWidth::Wide {
            2
        } else {
            1
        };
        let span_width = state.metrics.width * span_columns as f32;
        let shaped_width = shaped.width / scale;
        let shaped_height = shaped.height / scale;
        let cell_rect = state.metrics.cell_rect(
            Point {
                x: rect.x,
                y: rect.y,
            },
            point,
        );
        let origin = Point {
            x: cell_rect.x + ((span_width - shaped_width) * 0.5).max(0.0),
            y: cell_rect.y + ((state.metrics.height - shaped_height) * 0.5).max(0.0),
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
            let y = cell_rect.y + state.metrics.height - 1.5;
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
            let y = cell_rect.y + state.metrics.height * 0.52;
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
    state.paint_cache.rows[row].glyphs = paint.primitives;
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
            &mut state.paint_cache.rows[row].backgrounds,
        );
        rebuild_glyph_row(state, shaper, atlas, rect, row);
        work.background_rows += 1;
        work.glyph_rows += 1;
        work.cells += state.model.columns;
    }
}

fn rebuild_dirty_rows(
    state: &mut TerminalGridState,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    rect: Rect,
    work: &mut TerminalPaintWork,
) -> bool {
    let scale = norm_scale(state.scale);
    let mut changed = false;
    for row in 0..state.model.rows {
        if !state.dirty_rows.get(row).copied().unwrap_or(false) {
            continue;
        }
        rebuild_background_row(
            &state.model,
            state.metrics,
            scale,
            rect,
            row,
            &mut state.paint_cache.rows[row].backgrounds,
        );
        rebuild_glyph_row(state, shaper, atlas, rect, row);
        work.background_rows += 1;
        work.glyph_rows += 1;
        work.cells += state.model.columns;
        changed = true;
    }
    changed
}

fn patch_dirty_rows(
    scene: &mut Scene,
    id: WidgetId,
    state: &mut TerminalGridState,
    work: &mut TerminalPaintWork,
) {
    // Patch glyphs first. The cached ranges still describe the scene's old display
    // list, while the cached row fragments already hold the new data. A glyph-size
    // change must therefore use its old range before a background splice shifts the
    // entire glyph section. Work backwards so a variable-length row never invalidates
    // ranges before it.
    let pd = scene.paint_mut(id);
    for row in (0..state.model.rows).rev() {
        if !state.dirty_rows.get(row).copied().unwrap_or(false) {
            continue;
        }
        replace_range(
            pd,
            state.paint_cache.ranges.glyphs[row].clone(),
            &state.paint_cache.rows[row].glyphs,
        );
        work.scene_range_writes += 1;
    }
    for row in (0..state.model.rows).rev() {
        if !state.dirty_rows.get(row).copied().unwrap_or(false) {
            continue;
        }
        replace_range(
            pd,
            state.paint_cache.ranges.backgrounds[row].clone(),
            &state.paint_cache.rows[row].backgrounds,
        );
        work.scene_range_writes += 1;
    }
    state.paint_cache.reindex();
}

fn emit_one(
    runtime: &crate::Runtime,
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
        let rows_dirty = state.dirty_rows.iter().any(|dirty| *dirty);
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
            let rows_changed = rebuild_dirty_rows(state, shaper, atlas, rect, &mut work);
            if rows_changed {
                patch_dirty_rows(scene, id, state, &mut work);
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
                replace_range(
                    scene.paint_mut(id),
                    state.paint_cache.ranges.cursor.clone(),
                    &state.paint_cache.cursor,
                );
                work.scene_range_writes += 1;
                state.paint_cache.reindex();
            }
        }

        state.last_emit = Some(rect);
        state.full_paint_dirty = false;
        state.dirty_rows.fill(false);
        state.images_dirty = false;
        state.cursor_dirty = false;
        state.last_work = work;
        scene.mark_dirty(id, DirtyFlags::PAINT);
    });
}

/// Emits all grids whose model or laid-out rectangle changed. Called by the
/// standard paint pass after any required layout has settled.
pub(crate) fn emit_terminal_grids(
    runtime: &crate::Runtime,
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
    use schnellui_layout::LayoutEngine;
    use schnellui_scene::LayoutBox;

    fn build_grid(
        runtime: &crate::Runtime,
        grid: TerminalGrid,
        rect: Rect,
    ) -> (Scene, TextShaper, GlyphAtlas, WidgetId) {
        crate::reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(1024, 1024);
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
        let runtime_handle = crate::Runtime::new();
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
    fn paints_nerd_and_standard_prompt_fallback_glyphs() {
        let runtime_handle = crate::Runtime::new();
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
            .filter(|primitive| matches!(primitive, Primitive::GlyphQuad { .. }))
            .count();
        assert_eq!(glyphs, symbols.len());
    }

    #[test]
    fn cell_background_edges_share_physical_pixel_boundaries() {
        let runtime_handle = crate::Runtime::new();
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

        let backgrounds: Vec<Rect> = scene.paint(id).unwrap().primitives[1..=6]
            .iter()
            .map(|primitive| match primitive {
                Primitive::SolidRect { rect, .. } => *rect,
                other => panic!("expected cell background, got {other:?}"),
            })
            .collect();

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
        let runtime_handle = crate::Runtime::new();
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
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let metrics = TerminalCellMetrics::new(8.0, 16.0, 13.0);
        let mut model = TerminalGridModel::new(3, 2);
        let mut owner = TerminalCell::new("界", Color::WHITE, Color::BLACK);
        owner.width = TerminalCellWidth::Wide;
        owner.hyperlink = Some("https://example.test".to_string());
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
        let runtime_handle = crate::Runtime::new();
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
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        crate::reset(runtime);
        let metrics = TerminalCellMetrics::new(9.0, 18.0, 14.0);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(256, 256);
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
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let color = schnellui_signal::create_signal(Color::rgb(10, 20, 30));
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
        schnellui_signal::Runtime::flush();
        crate::run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);
        assert!(!scene.dirty_flags(id).contains(DirtyFlags::LAYOUT));
        assert!(scene.dirty_flags(id).contains(DirtyFlags::PAINT));
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("X"));
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert!(matches!(
            scene.paint(id).unwrap().primitives.first(),
            Some(Primitive::SolidRect { color, .. }) if *color == Color::rgb(40, 50, 60)
        ));
    }

    #[test]
    fn unrelated_signal_does_not_invoke_a_dynamic_grid() {
        use std::cell::Cell;

        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let columns = schnellui_signal::create_signal(2_usize);
        let unrelated = schnellui_signal::create_signal(false);
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
        schnellui_signal::Runtime::flush();
        crate::run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);

        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn versioned_grid_skips_model_work_for_unrelated_signals() {
        use std::cell::Cell;

        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let revision = schnellui_signal::create_signal(0_u64);
        let unrelated = schnellui_signal::create_signal(false);
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
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let columns = schnellui_signal::create_signal(2_usize);
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
        schnellui_signal::Runtime::flush();
        crate::run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);
        assert!(scene.dirty_flags(id).contains(DirtyFlags::LAYOUT));
    }

    #[test]
    fn one_cell_update_rebuilds_one_row_and_matches_a_forced_full_emit() {
        use std::cell::Cell;

        let runtime_handle = crate::Runtime::new();
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
            state.full_paint_dirty = true;
            state.dirty_rows.fill(true);
            state.images_dirty = true;
            state.cursor_dirty = true;
        });
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(scene.paint(id).unwrap().primitives, incremental);
    }

    #[test]
    fn row_delta_with_removed_background_and_added_glyph_matches_full_emit() {
        use std::cell::Cell;

        let runtime_handle = crate::Runtime::new();
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
            state.full_paint_dirty = true;
            state.dirty_rows.fill(true);
            state.images_dirty = true;
            state.cursor_dirty = true;
        });
        emit_terminal_grids(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(scene.paint(id).unwrap().primitives, incremental);
    }

    #[test]
    fn cursor_move_rebuilds_one_row_and_cursor_fragment() {
        use std::cell::Cell;

        let runtime_handle = crate::Runtime::new();
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
}
