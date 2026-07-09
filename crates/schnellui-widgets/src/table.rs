//! The table component (SOUL §8.1): [`Table`] + [`TableRow`], first-class in both
//! senses of the covenant.
//!
//! **First-class layout.** A table's defining property is *column alignment*:
//! every cell of a column shares one width. Because cell labels are static text
//! shaped at build (SOUL §8.1), the table computes per-column widths **at build
//! time** — one shaping pass over all cells, `col_width = max(label widths)` — and
//! registers each cell's measure as that fixed column width. Alignment then falls
//! out of plain nested flex (table = column of rows, row = row of cells): no grid
//! engine, no second layout pass, no per-frame work.
//!
//! **First-class accessibility (SOUL §6.1).** The retained structure *is* the
//! semantic structure: `Table` carries [`Role::Table`], each row [`Role::TableRow`],
//! each cell [`Role::Cell`] / [`Role::ColumnHeader`]. Row/column counts and per-cell
//! indices are **derived from the tree** by `schnellui_a11y::table_facts` and ride
//! the same `TreeUpdate`s — a screen reader gets real table navigation ("row 2 of
//! 3, Name column"), not a div soup.
//!
//! **Selection recolors, it never re-shapes** (SOUL Directive #3): selectable rows
//! carry `StateFlags::SELECTED` + the Click/Focus actions; clicking any cell — or
//! the row itself, via an inbound AccessKit `Click` `ActionRequest` (SOUL §6.3) —
//! selects its row with sibling exclusivity, mutating only each cell's background
//! primitive colour in place. Header rows advertise no actions and are inert.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use schnellui_a11y::{ActionFlags, Role, StateFlags};
use schnellui_layout::{Container, ContainerStyle};
use schnellui_scene::{
    Color, DirtyFlags, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind,
};
use schnellui_text::{GlyphAtlas, TextShaper};
use smallvec::SmallVec;

use crate::selection::{clear_selected, is_selected, set_selected};
use crate::{
    node_rect, norm_scale, phys_size_px, rasterize_and_push, theme_for, with_handlers, BuildCtx,
    SortDirection, View, PAD_H, PAD_V,
};

// ---------------------------------------------------------------------------
// visual + metric constants (deterministic for shots, SOUL §7.3)
// ---------------------------------------------------------------------------

/// Cell label font size — a step below button text, as tables usually set it.
const CELL_TEXT_SIZE: f32 = 14.0;
/// Row-separator hairline thickness.
const TABLE_LINE_W: f32 = 1.0;

/// A row-selection callback: fired with the selected **data-row index** (header
/// rows don't count). One callback per table, shared by every row's click handler.
type RowSelectFn = Box<dyn FnMut(usize) + 'static>;
/// A column-sort callback, fired with the new direction after a sortable header
/// is activated.
type ColumnSortFn = Box<dyn FnMut(SortDirection) + 'static>;

const SORT_ICON_SIZE: f32 = 10.0;
const SORT_ICON_GAP: f32 = 6.0;

struct TableCell {
    label: Cow<'static, str>,
    sort_direction: Option<SortDirection>,
    on_sort: Option<ColumnSortFn>,
}

impl TableCell {
    fn plain(label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            label: label.into(),
            sort_direction: None,
            on_sort: None,
        }
    }
}

/// A table column title with optional sorting behavior.
///
/// Plain strings still work with [`Table::columns`]. Use this descriptor only
/// when a title should expose sorting:
///
/// ```ignore
/// TableColumn::new("Name")
///     .sort(SortDirection::Ascending)
///     .on_sort(|direction| { /* reorder data, then remount if controlled */ })
/// ```
///
/// Activating a sortable title toggles ascending/descending and passes the new
/// direction to the callback. Without an initial [`sort`](Self::sort), the first
/// activation requests ascending order.
pub struct TableColumn {
    label: Cow<'static, str>,
    sort_direction: Option<SortDirection>,
    on_sort: Option<ColumnSortFn>,
}

impl TableColumn {
    pub fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            label: label.into(),
            sort_direction: None,
            on_sort: None,
        }
    }

    /// Declares the column's current ordering and paints its direction indicator.
    pub fn sort(mut self, direction: SortDirection) -> Self {
        self.sort_direction = Some(direction);
        self
    }

    /// Makes the title actionable. The callback receives the newly requested
    /// direction each time the title is activated.
    pub fn on_sort(mut self, f: impl FnMut(SortDirection) + 'static) -> Self {
        self.on_sort = Some(Box::new(f));
        self
    }
}

impl From<&'static str> for TableColumn {
    fn from(label: &'static str) -> Self {
        Self::new(label)
    }
}

impl From<String> for TableColumn {
    fn from(label: String) -> Self {
        Self::new(label)
    }
}

impl From<Cow<'static, str>> for TableColumn {
    fn from(label: Cow<'static, str>) -> Self {
        Self::new(label)
    }
}

// ---------------------------------------------------------------------------
// TableRow — the row builder Table consumes (SOUL §3.3)
// ---------------------------------------------------------------------------

/// One row of a [`Table`] (SOUL §8.1): an ordered list of static cell labels, plus
/// the header flag. A plain builder — it is consumed by [`Table::push_row`], not
/// built on its own, because a cell's width is a *column* property only the table
/// can compute.
pub struct TableRow {
    cells: Vec<TableCell>,
    pub(crate) is_header: bool,
}

impl TableRow {
    /// A new empty row.
    pub fn new() -> TableRow {
        TableRow {
            cells: Vec::new(),
            is_header: false,
        }
    }
    /// Appends one cell label.
    pub fn cell(mut self, label: impl Into<Cow<'static, str>>) -> TableRow {
        self.cells.push(TableCell::plain(label));
        self
    }
    /// Appends a configured column title. This is intended for header rows; data
    /// rows should use [`cell`](Self::cell).
    pub fn column(mut self, column: TableColumn) -> TableRow {
        self.cells.push(TableCell {
            label: column.label,
            sort_direction: column.sort_direction,
            on_sort: column.on_sort,
        });
        self
    }
    /// Marks this row as the header row: its cells carry [`Role::ColumnHeader`],
    /// paint the header band, and the row is never selectable. In `view!` this is
    /// the valueless `header` flag: `table_row(header) { "Name" "Age" }`.
    pub fn header(mut self) -> TableRow {
        self.is_header = true;
        self
    }
    /// Number of cells configured.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }
}

impl Default for TableRow {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Table (SOUL §8.1)
// ---------------------------------------------------------------------------

/// A data table (SOUL §8.1). See the module docs for the design; the builder:
///
/// ```ignore
/// Table::new()
///     .columns(["Name", "Age"])            // sugar: a header TableRow
///     .row(["Ada Lovelace", "36"])         // sugar: a data TableRow
///     .push_row(TableRow::new().cell("…")) // the explicit form view! lowers to
///     .selected_row(0)                     // initial selection (data-row index)
///     .on_select_row(|i| { … })            // enables selection; fired with the index
/// ```
///
/// Rows become selectable when `selected_row` or `on_select_row` is set; only then
/// do data rows advertise the Click/Focus actions (SOUL §6.1 — a static table is
/// honestly inert).
pub struct Table {
    pub(crate) rows: Vec<TableRow>,
    pub(crate) selected: Option<usize>,
    pub(crate) on_select_row: Option<RowSelectFn>,
}

impl Table {
    /// A new empty table.
    pub fn new() -> Table {
        Table {
            rows: Vec::new(),
            selected: None,
            on_select_row: None,
        }
    }
    /// Appends the header row from column labels (sugar for a
    /// `TableRow::new().header().cell(…)…` push).
    pub fn columns<I, C>(mut self, columns: I) -> Table
    where
        I: IntoIterator<Item = C>,
        C: Into<TableColumn>,
    {
        let mut row = TableRow::new().header();
        for column in columns {
            row = row.column(column.into());
        }
        self.rows.push(row);
        self
    }
    /// Appends one data row from cell labels.
    pub fn row<I, S>(mut self, cells: I) -> Table
    where
        I: IntoIterator<Item = S>,
        S: Into<Cow<'static, str>>,
    {
        let mut row = TableRow::new();
        for c in cells {
            row = row.cell(c);
        }
        self.rows.push(row);
        self
    }
    /// Appends an explicitly-built [`TableRow`] (what `view!` lowers to).
    pub fn push_row(mut self, row: TableRow) -> Table {
        self.rows.push(row);
        self
    }
    /// Pre-selects a **data-row** index (header rows don't count) and makes the
    /// rows selectable.
    pub fn selected_row(mut self, index: usize) -> Table {
        self.selected = Some(index);
        self
    }
    /// Registers the row-selection callback, fired with the selected data-row
    /// index — the same handler an inbound AccessKit `Click` on the row fires
    /// (SOUL §6.3). Makes the rows selectable.
    pub fn on_select_row(mut self, f: impl FnMut(usize) + 'static) -> Table {
        self.on_select_row = Some(Box::new(f));
        self
    }
    /// `true` when rows carry selection state + actions (SOUL §6.1).
    pub fn selectable(&self) -> bool {
        self.selected.is_some() || self.on_select_row.is_some()
    }
    pub fn role(&self) -> Role {
        Role::Table
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Table
    }
    /// Number of rows configured (header included, pre-build).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Table {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let runtime = &ctx.runtime;
        let this = *self;
        let table_id = ctx.scene.insert(WidgetKind::Table, parent);
        ctx.scene.a11y_mut(table_id).role = Role::Table.as_u16();
        ctx.layout
            .set_container(table_id, ContainerStyle::new(Container::Column));

        // Pass 1 — shape every cell label once and take per-column max widths
        // (SOUL §8.1: column alignment computed at build, since labels are static).
        let inv = 1.0 / norm_scale(ctx.scale);
        let phys = phys_size_px(CELL_TEXT_SIZE, ctx.scale);
        let mut col_widths: Vec<f32> = Vec::new();
        let mut sizes: Vec<Vec<Size>> = Vec::with_capacity(this.rows.len());
        for row in &this.rows {
            let mut rs = Vec::with_capacity(row.cells.len());
            for (c, cell) in row.cells.iter().enumerate() {
                let shaped = ctx.text.shape(&cell.label, phys, None);
                let sort_width =
                    if row.is_header && (cell.sort_direction.is_some() || cell.on_sort.is_some()) {
                        SORT_ICON_GAP + SORT_ICON_SIZE
                    } else {
                        0.0
                    };
                let sz = Size {
                    width: shaped.width * inv + sort_width,
                    height: shaped.height * inv,
                };
                if c >= col_widths.len() {
                    col_widths.push(0.0);
                }
                col_widths[c] = col_widths[c].max(sz.width);
                rs.push(sz);
            }
            sizes.push(rs);
        }

        // Selection plumbing: one shared callback; each selectable row's click
        // handler calls it with that row's data index (SOUL §6.3).
        let selectable = this.selectable();
        let shared: Option<Rc<RefCell<RowSelectFn>>> =
            this.on_select_row.map(|f| Rc::new(RefCell::new(f)));

        // Pass 2 — build rows and cells with the aligned column widths.
        let mut data_index = 0usize;
        for (r, row) in this.rows.into_iter().enumerate() {
            let row_id = ctx.scene.insert(WidgetKind::TableRow, Some(table_id));
            ctx.layout
                .set_container(row_id, ContainerStyle::new(Container::Row));
            ctx.scene.a11y_mut(row_id).role = Role::TableRow.as_u16();
            let is_header = row.is_header;
            let row_selected = !is_header && selectable && this.selected == Some(data_index);
            if !is_header && selectable {
                // Only selectable data rows advertise actions (SOUL §6.1); the
                // Click they advertise is the same path a pointer converges on.
                let a = ctx.scene.a11y_mut(row_id);
                let mut acts = ActionFlags::default();
                acts.insert(ActionFlags::CLICK);
                acts.insert(ActionFlags::FOCUS);
                a.actions = acts.0;
                if row_selected {
                    let mut st = StateFlags::default();
                    st.insert(StateFlags::SELECTED);
                    a.state = st.0;
                }
                if let Some(sh) = &shared {
                    let sh = sh.clone();
                    let idx = data_index;
                    with_handlers(&ctx.runtime, row_id, |h| {
                        h.click = Some(Box::new(move || (sh.borrow_mut())(idx)))
                    });
                }
            }
            for (c, cell) in row.cells.into_iter().enumerate() {
                let cell_id = ctx.scene.insert(WidgetKind::TableCell, Some(row_id));
                let role = if is_header {
                    Role::ColumnHeader
                } else {
                    Role::Cell
                };
                {
                    let a = ctx.scene.a11y_mut(cell_id);
                    a.role = role.as_u16();
                    a.name = Some(cell.label.to_string());
                    if is_header {
                        a.sort_direction = cell
                            .sort_direction
                            .map(SortDirection::as_u8)
                            .unwrap_or_default();
                        if cell.on_sort.is_some() {
                            let mut actions = ActionFlags::default();
                            actions.insert(ActionFlags::CLICK);
                            actions.insert(ActionFlags::FOCUS);
                            a.actions = actions.0;
                        }
                    }
                }
                let shape = theme_for(runtime, cell_id).shape;
                let intrinsic = Size {
                    width: col_widths[c] + 2.0 * shape.pad(PAD_H),
                    height: sizes[r][c].height + 2.0 * shape.pad(PAD_V),
                };
                emit_cell_paint(
                    runtime,
                    ctx.scene,
                    ctx.text,
                    ctx.atlas,
                    cell_id,
                    &cell.label,
                    is_header,
                    row_selected,
                    is_header && cell.on_sort.is_some(),
                    cell.sort_direction,
                    intrinsic,
                    ctx.scale,
                );
                ctx.layout
                    .set_measure(cell_id, Box::new(move |_avail| intrinsic));
                if is_header {
                    if let Some(sort) = cell.on_sort {
                        with_handlers(&ctx.runtime, cell_id, |handlers| handlers.sort = Some(sort));
                    }
                }
            }
            if !is_header {
                data_index += 1;
            }
        }
        table_id
    }
}

// ---------------------------------------------------------------------------
// paint-fragment emission + in-place recolor (SOUL §3.2, §8.1)
// ---------------------------------------------------------------------------

/// Emits one cell's paint: `[0]` the background surface (header band / selection
/// tint / white), `[1]` the bottom separator hairline, then the label as real glyph
/// quads inset by the shared padding (SOUL §8.1). The background at index `[0]` is
/// the recolor target of a row-selection toggle.
#[allow(clippy::too_many_arguments)]
fn emit_cell_paint(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    label: &str,
    is_header: bool,
    selected: bool,
    sortable: bool,
    sort_direction: Option<SortDirection>,
    intrinsic: Size,
    scale: f32,
) {
    let phys = phys_size_px(CELL_TEXT_SIZE, scale);
    let shaped = shaper.shape(label, phys, None);
    let rect = node_rect(scene, id, intrinsic);
    let t = theme_for(runtime, id);
    let bg = if is_header {
        t.surface_muted
    } else if selected {
        t.selection
    } else {
        t.surface
    };
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: bg,
        corner_radius: 0.0,
    });
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(
            rect.x,
            rect.y + rect.height - TABLE_LINE_W,
            rect.width,
            TABLE_LINE_W,
        ),
        color: t.separator,
        corner_radius: 0.0,
    });
    rasterize_and_push(
        pd,
        shaper,
        atlas,
        &shaped,
        phys as u32,
        t.text,
        scale,
        Point {
            x: rect.x + t.shape.pad(PAD_H),
            y: rect.y + t.shape.pad(PAD_V),
        },
    );
    if is_header && (sortable || sort_direction.is_some()) {
        push_sort_indicator(
            pd,
            rect,
            t.shape.pad(PAD_H),
            sort_direction,
            t.accent,
            t.disabled,
        );
    }
}

fn sort_indicator_colors(
    direction: Option<SortDirection>,
    active: Color,
    inactive: Color,
) -> (Color, Color) {
    match direction {
        Some(SortDirection::Ascending) => (active, Color::TRANSPARENT),
        Some(SortDirection::Descending) => (Color::TRANSPARENT, active),
        None => (inactive, inactive),
    }
}

/// Appends a stable four-line up/down indicator. Keeping both chevrons in the
/// retained fragment means direction changes only recolor existing primitives.
fn push_sort_indicator(
    paint: &mut schnellui_scene::PaintData,
    rect: Rect,
    pad_h: f32,
    direction: Option<SortDirection>,
    active: Color,
    inactive: Color,
) {
    let (up_color, down_color) = sort_indicator_colors(direction, active, inactive);
    let left = rect.x + rect.width - pad_h - SORT_ICON_SIZE;
    let middle = left + SORT_ICON_SIZE * 0.5;
    let right = left + SORT_ICON_SIZE;
    let center = rect.y + rect.height * 0.5;
    let stroke = 1.4;
    let up_apex = center - 3.5;
    let up_base = center - 0.5;
    let down_base = center + 0.5;
    let down_apex = center + 3.5;
    for (from, to, color) in [
        (
            Point {
                x: left,
                y: up_base,
            },
            Point {
                x: middle,
                y: up_apex,
            },
            up_color,
        ),
        (
            Point {
                x: middle,
                y: up_apex,
            },
            Point {
                x: right,
                y: up_base,
            },
            up_color,
        ),
        (
            Point {
                x: left,
                y: down_base,
            },
            Point {
                x: middle,
                y: down_apex,
            },
            down_color,
        ),
        (
            Point {
                x: middle,
                y: down_apex,
            },
            Point {
                x: right,
                y: down_base,
            },
            down_color,
        ),
    ] {
        paint.primitives.push(Primitive::Line {
            from,
            to,
            width: stroke,
            color,
        });
    }
}

fn recolor_sort_indicator(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    header: WidgetId,
    direction: SortDirection,
) {
    let theme = theme_for(runtime, header);
    let (up, down) = sort_indicator_colors(Some(direction), theme.accent, theme.disabled);
    let paint = scene.paint_mut(header);
    let start = paint.primitives.len().saturating_sub(4);
    for (offset, primitive) in paint.primitives[start..].iter_mut().enumerate() {
        if let Primitive::Line { color, .. } = primitive {
            *color = if offset < 2 { up } else { down };
        }
    }
    scene.mark_dirty(header, DirtyFlags::PAINT);
}

/// Recolors every cell background of `row` for a selection state, **in place**
/// (SOUL Directive #3): only each cell's primitive `[0]` colour mutates — the
/// separator hairline and the label glyphs are untouched, no re-shape, no heap.
/// Marks each recolored cell paint-dirty so its rect joins the frame damage.
fn recolor_row(runtime: &crate::Runtime, scene: &mut Scene, row: WidgetId, selected: bool) {
    let cells: SmallVec<[WidgetId; 8]> = scene
        .node(row)
        .map(|n| {
            n.children
                .iter()
                .copied()
                .filter(|c| scene.node(*c).map(|cn| cn.kind) == Some(WidgetKind::TableCell))
                .collect()
        })
        .unwrap_or_default();
    for cell in cells {
        let t = theme_for(runtime, cell);
        let bg = if selected { t.selection } else { t.surface };
        let pd = scene.paint_mut(cell);
        if let Some(Primitive::SolidRect { color, .. }) = pd.primitives.get_mut(0) {
            *color = bg;
        }
        scene.mark_dirty(cell, DirtyFlags::PAINT);
    }
}

// ---------------------------------------------------------------------------
// click / activation dispatch (SOUL §6.3 — one inbound path for pointer + a11y)
// ---------------------------------------------------------------------------

/// The click/activation dispatch hook for the table kinds (SOUL §6.3), called by
/// [`dispatch_click`](crate::dispatch_click) for `TableRow` and `TableCell`. A
/// pointer click lands on a **cell** (the leaf hit-testing resolves) and bubbles to
/// its row; an AccessKit `Click` `ActionRequest` targets the **row** directly —
/// both converge here. A sortable column header is handled as its own action:
/// direction and indicator toggle in place, then `on_sort` receives the new
/// direction. Data rows select with sibling exclusivity and fire `on_select`.
/// Returns `true` if an advertised table action ran or changed state.
pub(crate) fn dispatch_click_table(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    kind: WidgetKind,
) -> bool {
    // Unlike an ordinary cell, a sortable header owns its Click action instead of
    // bubbling to the row.
    if kind == WidgetKind::TableCell
        && scene
            .a11y(id)
            .is_some_and(|a| Role::from_u16(a.role) == Role::ColumnHeader)
    {
        let clickable = scene
            .a11y(id)
            .is_some_and(|a| ActionFlags(a.actions).contains(ActionFlags::CLICK));
        if !clickable {
            return false;
        }
        let next = scene
            .a11y(id)
            .and_then(|a| SortDirection::from_u8(a.sort_direction))
            .map(SortDirection::toggled)
            .unwrap_or(SortDirection::Ascending);
        scene.a11y_mut(id).sort_direction = next.as_u8();
        recolor_sort_indicator(runtime, scene, id, next);
        scene.mark_dirty(id, DirtyFlags::A11Y);

        let callback = runtime.with(|rt| {
            rt.borrow_mut()
                .handlers
                .get_mut(id)
                .and_then(|handlers| handlers.sort.take())
        });
        if let Some(mut callback) = callback {
            callback(next);
            runtime.with(|rt| {
                if let Some(handlers) = rt.borrow_mut().handlers.get_mut(id) {
                    handlers.sort = Some(callback);
                }
            });
        }
        return true;
    }

    // Resolve the target row: a cell bubbles to its parent row (SOUL §6.3).
    let row = match kind {
        WidgetKind::TableRow => id,
        WidgetKind::TableCell => {
            let Some(p) = scene.node(id).and_then(|n| n.parent) else {
                return false;
            };
            if scene.node(p).map(|n| n.kind) != Some(WidgetKind::TableRow) {
                return false;
            }
            p
        }
        _ => return false,
    };
    // Only rows that advertise Click are selectable (SOUL §6.1 honesty: header
    // rows and non-selectable tables advertise nothing and do nothing).
    let clickable = scene
        .a11y(row)
        .map(|a| ActionFlags(a.actions).contains(ActionFlags::CLICK))
        .unwrap_or(false);
    if !clickable {
        return false;
    }
    // Row exclusivity via the tree (SOUL §6.3): clear any selected sibling row.
    if let Some(table) = scene.node(row).and_then(|n| n.parent) {
        let siblings: SmallVec<[WidgetId; 8]> = scene
            .node(table)
            .map(|n| n.children.iter().copied().collect())
            .unwrap_or_default();
        for sib in siblings {
            if sib == row {
                continue;
            }
            let selected_sibling = scene.node(sib).map(|n| n.kind) == Some(WidgetKind::TableRow)
                && is_selected(scene, sib);
            if selected_sibling {
                clear_selected(scene, sib);
                recolor_row(runtime, scene, sib, false);
                scene.mark_dirty(sib, DirtyFlags::A11Y);
            }
        }
    }
    // Select the clicked row if it was not already (no re-dirty otherwise).
    let was_selected = is_selected(scene, row);
    if !was_selected {
        set_selected(scene, row);
        recolor_row(runtime, scene, row, true);
        scene.mark_dirty(row, DirtyFlags::A11Y);
    }
    // Fire the row's `on_select` (stored as its `click` handler), taken out of the
    // registry before it runs so no borrow is held across user code (§3.1).
    let cb = runtime.with(|rt| {
        rt.borrow_mut()
            .handlers
            .get_mut(row)
            .and_then(|h| h.click.take())
    });
    let fired = cb.is_some();
    if let Some(mut cb) = cb {
        cb();
        runtime.with(|rt| {
            if let Some(h) = rt.borrow_mut().handlers.get_mut(row) {
                h.click = Some(cb);
            }
        });
    }
    fired || !was_selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reset;
    use schnellui_a11y::table_facts;
    use schnellui_layout::LayoutEngine;
    use schnellui_scene::Color;
    use schnellui_signal::create_signal;

    /// Builds `view` into a fresh scene as the root (mirrors the crate-root tests).
    fn build_one(
        runtime: &crate::Runtime,
        view: impl View,
    ) -> (Scene, LayoutEngine, TextShaper, GlyphAtlas, WidgetId) {
        reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
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
            Box::new(view).build(&mut ctx, None)
        };
        scene.set_root(id);
        (scene, layout, text, atlas, id)
    }

    /// A 2-column table with a header and three data rows of deliberately
    /// different label widths (column alignment must come from the table).
    fn sample_table() -> Table {
        Table::new()
            .columns(["Name", "Age"])
            .row(["Ada Lovelace", "36"])
            .row(["Grace Hopper", "85"])
            .row(["Al", "7"])
    }

    /// The `SolidRect` colour of the primitive at `idx` on a node.
    fn color_of(scene: &Scene, id: WidgetId, idx: usize) -> Color {
        match scene.paint(id).unwrap().primitives[idx] {
            Primitive::SolidRect { color, .. } => color,
            ref p => panic!("expected a SolidRect, got {p:?}"),
        }
    }

    /// row ids of a built table.
    fn rows_of(scene: &Scene, table: WidgetId) -> Vec<WidgetId> {
        scene.node(table).unwrap().children.clone().into_vec()
    }

    /// cell ids of a row.
    fn cells_of(scene: &Scene, row: WidgetId) -> Vec<WidgetId> {
        scene.node(row).unwrap().children.clone().into_vec()
    }

    // --- build-time semantics (SOUL §6.1 — the tree IS the table semantics) ---

    #[test]
    fn table_builds_semantic_rows_headers_and_cells() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, sample_table());
        assert_eq!(scene.node(id).unwrap().kind, WidgetKind::Table);
        assert!(scene.node(id).unwrap().kind.is_container());
        assert_eq!(Role::from_u16(scene.a11y(id).unwrap().role), Role::Table);

        let rows = rows_of(&scene, id);
        assert_eq!(rows.len(), 4); // header + 3 data rows
        for &r in &rows {
            assert_eq!(scene.node(r).unwrap().kind, WidgetKind::TableRow);
            assert_eq!(Role::from_u16(scene.a11y(r).unwrap().role), Role::TableRow);
        }
        // header cells carry ColumnHeader + the label as accessible name
        let hcells = cells_of(&scene, rows[0]);
        assert_eq!(hcells.len(), 2);
        let ha = scene.a11y(hcells[0]).unwrap();
        assert_eq!(Role::from_u16(ha.role), Role::ColumnHeader);
        assert_eq!(ha.name.as_deref(), Some("Name"));
        // data cells carry Cell + name
        let dcells = cells_of(&scene, rows[1]);
        let da = scene.a11y(dcells[0]).unwrap();
        assert_eq!(Role::from_u16(da.role), Role::Cell);
        assert_eq!(da.name.as_deref(), Some("Ada Lovelace"));
        // header band vs data background (SOUL §8.1)
        assert_eq!(
            color_of(&scene, hcells[0], 0),
            crate::Theme::default().surface_muted
        );
        assert_eq!(
            color_of(&scene, dcells[0], 0),
            crate::Theme::default().surface
        );
        // every cell carries the separator hairline at [1]
        assert_eq!(
            color_of(&scene, dcells[0], 1),
            crate::Theme::default().separator
        );
    }

    /// The a11y integration end to end: the widget-built tree yields correct
    /// derived counts and indices through `schnellui_a11y::table_facts` (SOUL §6.1).
    #[test]
    fn built_table_derives_counts_and_indices() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, sample_table());
        let tf = table_facts(&scene, id);
        assert_eq!(tf.row_count, Some(4));
        assert_eq!(tf.column_count, Some(2));
        let rows = rows_of(&scene, id);
        assert_eq!(table_facts(&scene, rows[0]).row_index, Some(0));
        assert_eq!(table_facts(&scene, rows[2]).row_index, Some(2));
        let cell = cells_of(&scene, rows[2])[1];
        let cf = table_facts(&scene, cell);
        assert_eq!(cf.row_index, Some(2));
        assert_eq!(cf.column_index, Some(1));
    }

    #[test]
    fn non_selectable_table_rows_advertise_no_actions() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, sample_table());
        for r in rows_of(&scene, id) {
            assert_eq!(scene.a11y(r).unwrap().actions, 0);
        }
    }

    #[test]
    fn selectable_table_data_rows_advertise_click_focus_header_stays_inert() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, sample_table().selected_row(1));
        let rows = rows_of(&scene, id);
        // header: no actions, never selectable (SOUL §6.1 honesty)
        assert_eq!(scene.a11y(rows[0]).unwrap().actions, 0);
        for &r in &rows[1..] {
            let a = scene.a11y(r).unwrap();
            assert!(ActionFlags(a.actions).contains(ActionFlags::CLICK));
            assert!(ActionFlags(a.actions).contains(ActionFlags::FOCUS));
        }
        // data-row 1 (tree row 2) starts selected, tinted
        assert!(is_selected(&scene, rows[2]));
        assert_eq!(
            color_of(&scene, cells_of(&scene, rows[2])[0], 0),
            crate::Theme::default().selection
        );
        assert!(!is_selected(&scene, rows[1]));
    }

    #[test]
    fn sortable_column_header_exposes_direction_actions_and_toggles_in_place() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let requested = create_signal(SortDirection::Ascending);
        let table = Table::new()
            .columns([
                TableColumn::new("Name")
                    .sort(SortDirection::Ascending)
                    .on_sort(move |direction| requested.set(direction)),
                TableColumn::new("Age"),
            ])
            .row(["Ada", "36"]);
        let (mut scene, _l, _t, _a, id) = build_one(runtime, table);
        let headers = cells_of(&scene, rows_of(&scene, id)[0]);
        let sortable = headers[0];
        let plain = headers[1];

        let semantics = scene.a11y(sortable).unwrap();
        let actions = ActionFlags(semantics.actions);
        assert!(actions.contains(ActionFlags::CLICK));
        assert!(actions.contains(ActionFlags::FOCUS));
        assert_eq!(
            SortDirection::from_u8(semantics.sort_direction),
            Some(SortDirection::Ascending)
        );
        assert_eq!(scene.a11y(plain).unwrap().actions, 0);
        assert_eq!(scene.a11y(plain).unwrap().sort_direction, 0);

        scene.clear_dirty();
        assert!(crate::dispatch_click(runtime, &mut scene, sortable));
        assert_eq!(requested.get(), SortDirection::Descending);
        assert_eq!(
            SortDirection::from_u8(scene.a11y(sortable).unwrap().sort_direction),
            Some(SortDirection::Descending)
        );
        assert!(scene.dirty_flags(sortable).contains(DirtyFlags::PAINT));
        assert!(scene.dirty_flags(sortable).contains(DirtyFlags::A11Y));
        assert!(scene.layout_dirty().is_empty());

        scene.clear_dirty();
        assert!(crate::dispatch_click(runtime, &mut scene, sortable));
        assert_eq!(requested.get(), SortDirection::Ascending);
        assert_eq!(
            SortDirection::from_u8(scene.a11y(sortable).unwrap().sort_direction),
            Some(SortDirection::Ascending)
        );
    }

    #[test]
    fn sortable_column_without_declared_direction_requests_ascending_first() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let requested = create_signal(SortDirection::Descending);
        let table = Table::new()
            .columns([TableColumn::new("Name").on_sort(move |direction| requested.set(direction))])
            .row(["Ada"]);
        let (mut scene, _l, _t, _a, id) = build_one(runtime, table);
        let header = cells_of(&scene, rows_of(&scene, id)[0])[0];
        assert_eq!(scene.a11y(header).unwrap().sort_direction, 0);
        assert!(crate::dispatch_click(runtime, &mut scene, header));
        assert_eq!(requested.get(), SortDirection::Ascending);
    }

    #[test]
    fn declared_sort_direction_without_action_is_read_only() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let table = Table::new()
            .columns([TableColumn::new("Name").sort(SortDirection::Descending)])
            .row(["Ada"]);
        let (mut scene, _l, _t, _a, id) = build_one(runtime, table);
        let header = cells_of(&scene, rows_of(&scene, id)[0])[0];
        assert_eq!(
            SortDirection::from_u8(scene.a11y(header).unwrap().sort_direction),
            Some(SortDirection::Descending)
        );
        assert_eq!(scene.a11y(header).unwrap().actions, 0);
        assert!(!crate::dispatch_click(runtime, &mut scene, header));
    }

    // --- column alignment (SOUL §8.1 — the table's defining layout property) ---

    #[test]
    fn columns_align_across_rows_with_different_label_widths() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _t, _a, id) = build_one(runtime, sample_table());
        layout.sync_tree(&scene, id);
        layout.compute(
            &mut scene,
            id,
            Size {
                width: 600.0,
                height: 400.0,
            },
        );
        let rows = rows_of(&scene, id);
        // for each column: every row's cell shares the same x and width
        let first = cells_of(&scene, rows[0]);
        for (col, &first_cell) in first.iter().enumerate() {
            let anchor = scene.layout(first_cell).unwrap().rect;
            assert!(anchor.width > 0.0);
            for &r in &rows[1..] {
                let cell = cells_of(&scene, r)[col];
                let b = scene.layout(cell).unwrap().rect;
                assert_eq!(b.x, anchor.x, "column {col} x aligns");
                assert_eq!(b.width, anchor.width, "column {col} width aligns");
            }
        }
        // the second column starts where the first ends (row of cells, gap 0)
        let c0 = scene.layout(first[0]).unwrap().rect;
        let c1 = scene.layout(first[1]).unwrap().rect;
        assert_eq!(c1.x, c0.x + c0.width);
    }

    // --- input handling: pointer and ActionRequest converge on the row (§6.3) ---

    #[test]
    fn cell_click_bubbles_to_row_selects_exclusively_and_fires_index() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let picked = create_signal(usize::MAX);
        let (mut scene, _l, _t, _a, id) = build_one(
            runtime,
            sample_table()
                .selected_row(0)
                .on_select_row(move |i| picked.set(i)),
        );
        let rows = rows_of(&scene, id);
        scene.clear_dirty();
        // click a CELL of data-row 2 — the pointer path (hit_test resolves leaves)
        let cell = cells_of(&scene, rows[3])[0];
        assert!(crate::dispatch_click(runtime, &mut scene, cell));
        assert_eq!(picked.get(), 2, "handler sees the data-row index");
        // exclusivity: initial selection cleared, clicked row selected
        assert!(!is_selected(&scene, rows[1]));
        assert!(is_selected(&scene, rows[3]));
        // recolored in place: every cell of the two affected rows
        for c in cells_of(&scene, rows[3]) {
            assert_eq!(color_of(&scene, c, 0), crate::Theme::default().selection);
            assert!(scene.dirty_flags(c).contains(DirtyFlags::PAINT));
        }
        for c in cells_of(&scene, rows[1]) {
            assert_eq!(color_of(&scene, c, 0), crate::Theme::default().surface);
        }
        // rows carry the a11y state change; layout untouched (SOUL §8.1)
        assert!(scene.dirty_flags(rows[3]).contains(DirtyFlags::A11Y));
        assert!(scene.layout_dirty().is_empty());
    }

    #[test]
    fn row_click_via_action_request_path_selects_too() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let picked = create_signal(usize::MAX);
        let (mut scene, _l, _t, _a, id) = build_one(
            runtime,
            sample_table().on_select_row(move |i| picked.set(i)),
        );
        let rows = rows_of(&scene, id);
        // an AccessKit Click ActionRequest targets the ROW node itself (SOUL §6.3)
        assert!(crate::dispatch_click(runtime, &mut scene, rows[2]));
        assert_eq!(picked.get(), 1);
        assert!(is_selected(&scene, rows[2]));
    }

    #[test]
    fn header_and_non_selectable_rows_are_inert() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        // header row of a selectable table: inert
        let (mut scene, _l, _t, _a, id) = build_one(runtime, sample_table().selected_row(0));
        let rows = rows_of(&scene, id);
        assert!(!crate::dispatch_click(runtime, &mut scene, rows[0]));
        let header_cell = cells_of(&scene, rows[0])[0];
        assert!(!crate::dispatch_click(runtime, &mut scene, header_cell));
        assert!(!is_selected(&scene, rows[0]));

        // a non-selectable table: every row and cell inert
        let (mut scene, _l, _t, _a, id) = build_one(runtime, sample_table());
        let rows = rows_of(&scene, id);
        scene.clear_dirty();
        assert!(!crate::dispatch_click(runtime, &mut scene, rows[1]));
        let cell = cells_of(&scene, rows[1])[0];
        assert!(!crate::dispatch_click(runtime, &mut scene, cell));
        assert!(scene.dirty_flags(rows[1]).is_empty());
    }

    #[test]
    fn already_selected_row_reclick_refires_without_re_dirty() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let count = create_signal(0i32);
        let (mut scene, _l, _t, _a, id) = build_one(
            runtime,
            sample_table()
                .selected_row(1)
                .on_select_row(move |_| count.update(|v| *v += 1)),
        );
        let rows = rows_of(&scene, id);
        assert!(crate::dispatch_click(runtime, &mut scene, rows[2]));
        assert_eq!(count.get(), 1);
        scene.clear_dirty();
        assert!(crate::dispatch_click(runtime, &mut scene, rows[2]));
        assert_eq!(count.get(), 2);
        // state unchanged → no dirty channels re-flagged
        assert!(scene.dirty_flags(rows[2]).is_empty());
    }

    // --- hit-testing: containers transparent, the cell leaf is the target ---

    #[test]
    fn hit_test_resolves_cell_leaf() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _t, _a, id) =
            build_one(runtime, sample_table().selected_row(0));
        layout.sync_tree(&scene, id);
        layout.compute(
            &mut scene,
            id,
            Size {
                width: 600.0,
                height: 400.0,
            },
        );
        let rows = rows_of(&scene, id);
        let cell = cells_of(&scene, rows[1])[1];
        let r = scene.layout(cell).unwrap().rect;
        let p = Point {
            x: r.x + r.width * 0.5,
            y: r.y + r.height * 0.5,
        };
        assert_eq!(crate::hit_test(runtime, &scene, p), Some(cell));
    }

    #[test]
    fn ragged_rows_use_max_column_count() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(
            runtime,
            Table::new()
                .columns(["A", "B", "C"])
                .row(["1", "2"]) // one cell short
                .row(["x", "y", "z"]),
        );
        let tf = table_facts(&scene, id);
        assert_eq!(tf.row_count, Some(3));
        assert_eq!(tf.column_count, Some(3));
        let rows = rows_of(&scene, id);
        assert_eq!(cells_of(&scene, rows[1]).len(), 2);
    }
}
