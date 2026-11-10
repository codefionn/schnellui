//! # schnellui::charts
//!
//! Chart content leaves (SOUL §8.1): [`BarChart`], [`LineChart`], [`Sparkline`].
//! Each is a [`View`](schnellui::widgets::View) that inserts a
//! [`WidgetKind::Chart`](schnellui::scene::WidgetKind::Chart) node, paints GPU
//! primitives into the retained scene (SOUL §3.2), registers a fixed-size intrinsic
//! measure (SOUL §8.1), and carries a **first-class accessible summary** (SOUL §6.1).
//!
//! ## What a chart is (and is not)
//!
//! A chart is a *content leaf*: it draws pixels and carries a `Role::Chart` (mapped to
//! accesskit `Figure`), exactly like the built-in leaves in `schnellui::widgets`. It is
//! **semantic before it is visual** (SOUL §6.1): its accessible *value* is a
//! deterministic one-line summary of the series (see [`summary`]), so a screen reader —
//! and the agent loop (SOUL §6.5) — can read *what the chart is* without scraping
//! pixels.
//!
//! ## Reading values
//!
//! By default a chart is bare marks on a baseline, as it always was. Three opt-in
//! builders make it readable:
//! - `.axes(true)` adds a value axis: gridlines at 1-2-5 ticks, tick labels in the left
//!   gutter, and a zero line drawn *over* the bars. Add `.labels(..)` for category
//!   labels under the marks (thinned so they never overlap) and `.gutter(px)` to give
//!   several charts the same plot width.
//! - `.tooltips(true)` makes every bar or point a hover target that shows
//!   `"Label: value unit"` (see `.unit(..)`). The chart then builds as a fixed-size
//!   `Stack` holding the `Chart` leaf and one invisible hit node per mark; each hit
//!   node is also an accessible `Group` named with the same text.
//! - Text is real shaped text (tick and category labels, `text_muted`), painted through
//!   the same glyph path as `Text`. A visible *title* is still composed as a `Text`
//!   above the chart; `.title()` sets the accessible name.
//!
//! ## Design rules (the project data-viz standard)
//!
//! These are encoded as constants below, not scattered magic numbers:
//! - a single validated colorblind-safe categorical palette, [`SERIES`], in **fixed
//!   order** — never reordered, never cycled. Every entry is at least 3:1 against both
//!   white and the default page (WCAG 1.4.11, non-text contrast);
//! - a baseline / zero line in [`AXIS`] (the theme's `outline`, 3:1) at
//!   [`AXIS_WIDTH`] px, with `separator` gridlines under the marks;
//! - negative bars take their own color ([`BarChart::negative_color`], red by default)
//!   so sign never relies on position alone;
//! - line markers are round and the plot is inset by the marker radius, so a point on
//!   the minimum never sits half outside the plot.
//!
//! ## Determinism (SOUL §7.3)
//!
//! All geometry is a pure function of the inputs: non-finite values are skipped, an
//! empty series is a valid (empty-but-sized) box, and a degenerate range (single value
//! or all-equal data) collapses to the vertical center — never a divide-by-zero, never
//! a `NaN`. Charts register no dynamic slots and no signal bindings, so there is no
//! re-render path to allocate on (SOUL §1): they are built once at mount.
//!
//! ## Anchoring (SOUL §8.1)
//!
//! Like the built-in leaves, a chart emits its primitives at a **provisional local
//! origin** (`node_rect` returns an origin box before layout has run); the widgets
//! crate's `reposition_paint` pass then slides the whole primitive set — bars, lines,
//! labels *and* line endpoints — onto the node's laid-out origin. With axes on, a
//! transparent full-box rect is emitted first so the slide anchors on the box corner
//! rather than on the first label.

use std::borrow::Cow;

use schnellui::a11y::Role;
use schnellui::layout::{Container, ContainerStyle};
use schnellui::scene::{Color, PaintData, Point, Primitive, Rect, Size, WidgetId, WidgetKind};
use schnellui::text::{GlyphAtlas, ShapedText, TextShaper};
use schnellui::widgets::{node_rect, rasterize_and_push, BuildCtx, View};

mod axis;
use axis::{format_tick, format_value, nice_axis, with_unit};

// ---------------------------------------------------------------------------
// the data-viz standard, encoded as constants (SOUL §6.1)
// ---------------------------------------------------------------------------

/// The validated colorblind-safe **categorical** palette, in a **FIXED order** that is
/// never reordered and never cycled (the project data-viz standard). Single-series
/// charts default to `SERIES[0]` (blue); the array exists so a multi-chart dashboard
/// assigns distinct series colors *by entity in fixed order* — the same entity keeps
/// the same hue across every chart on the page.
///
/// Every entry has at least 3:1 contrast against white and against the default page
/// (`#e9ecf1`), the WCAG 1.4.11 minimum for graphics.
///
/// Order: `#2a78d6` blue, `#0e8a68` teal, `#b87400` amber, `#008300` green,
/// `#4a3aa7` violet, `#d53f3e` red, `#c8467f` magenta, `#d1531f` orange.
pub const SERIES: [Color; 8] = [
    Color::rgb(0x2a, 0x78, 0xd6), // 1 blue
    Color::rgb(0x0e, 0x8a, 0x68), // 2 teal
    Color::rgb(0xb8, 0x74, 0x00), // 3 amber
    Color::rgb(0x00, 0x83, 0x00), // 4 green
    Color::rgb(0x4a, 0x3a, 0xa7), // 5 violet
    Color::rgb(0xd5, 0x3f, 0x3e), // 6 red
    Color::rgb(0xc8, 0x46, 0x7f), // 7 magenta
    Color::rgb(0xd1, 0x53, 0x1f), // 8 orange
];

/// The baseline / zero-line color (`#7a8699`, 3.7:1 on white): the default theme's
/// `outline`. Charts take the ambient theme's `outline` at build, so this constant is
/// the light-theme value and the documented fallback.
pub const AXIS: Color = Color::rgb(0x7a, 0x86, 0x99);

/// Baseline / zero-line stroke width (logical px).
const AXIS_WIDTH: f32 = 1.5;
/// Gridline stroke width (logical px).
const GRID_WIDTH: f32 = 1.0;
/// Minimum gap between adjacent bars (logical px) — `≥ 2` per the data-viz standard.
const BAR_GAP: f32 = 2.0;
/// Share of a category slot a bar fills when axes are on (the rest is the gap).
const BAR_FILL: f32 = 0.72;
/// Per-point marker diameter (logical px) for [`LineChart`] markers (round).
const MARKER_SIZE: f32 = 9.0;
/// Edge of a line point's hover target (logical px).
const HIT_SIZE: f32 = 16.0;
/// A hovered point grows from the 4.5px marker to this radius, ringed by a halo of
/// [`HOVER_HALO`] px in the page colour so it lifts off the line.
const HOVER_POINT_RADIUS: f32 = 6.0;
const HOVER_HALO: f32 = 2.0;
/// Laid over a hovered bar: darkens it about 22% so it stands out from its row.
const MARK_HOVER_SHADE: Color = Color::rgba(0, 0, 0, 0x38);
/// Shortest a bar's hover target is, so a near-zero bar can still be reached.
const HIT_MIN: f32 = 10.0;
/// Tick and category label size (logical px).
const LABEL_SIZE: f32 = 12.0;
/// Gap between tick labels and the plot, and between the plot and category labels.
const LABEL_GAP: f32 = 6.0;
/// Space the tallest tick should take (logical px), which sets the tick count.
const TICK_SPACING: f32 = 34.0;
/// Default bar/line chart size.
const DEFAULT_SIZE: Size = Size {
    width: 240.0,
    height: 140.0,
};
/// Default [`Sparkline`] size.
const SPARK_SIZE: Size = Size {
    width: 80.0,
    height: 20.0,
};
/// Default [`LineChart`] stroke width (logical px).
const LINE_STROKE: f32 = 2.0;
/// Default [`Sparkline`] stroke width (logical px).
const SPARK_STROKE: f32 = 2.0;
/// Radius of a [`Sparkline`]'s end dot (logical px).
const END_DOT_RADIUS: f32 = 3.5;

// ---------------------------------------------------------------------------
// value normalization (pure, deterministic — SOUL §7.3; unit-tested directly)
// ---------------------------------------------------------------------------

/// The `[lo, hi]` data range a chart maps into its plot rect. `include_zero` folds zero
/// into the range so **bars anchor to a real baseline** (`lo = min(data_min, 0)`,
/// `hi = max(data_max, 0)`); lines/sparklines pass `false` for the plain
/// `[data_min, data_max]`. Non-finite values are skipped (SOUL §7.3); empty (or
/// all-non-finite) data collapses to the degenerate `(0.0, 0.0)` range.
fn value_range(values: &[f32], include_zero: bool) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &v in values {
        if v.is_finite() {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 0.0);
    }
    if include_zero {
        (lo.min(0.0), hi.max(0.0))
    } else {
        (lo, hi)
    }
}

/// Maps a data value to a `y` within `plot`. Screen `y` grows downward, so a *larger*
/// value maps *higher* (smaller `y`): `v == hi` → `plot.y` (top), `v == lo` →
/// `plot.bottom()`. A degenerate range (`hi == lo`: single value or all-equal data)
/// maps everything to the vertical center — no divide-by-zero, no `NaN` (SOUL §7.3).
/// Out-of-range values are clamped into the plot.
fn map_y(v: f32, lo: f32, hi: f32, plot: Rect) -> f32 {
    let span = hi - lo;
    if span.abs() <= f32::EPSILON {
        return plot.y + plot.height * 0.5;
    }
    let t = ((v - lo) / span).clamp(0.0, 1.0);
    plot.y + plot.height * (1.0 - t)
}

/// Evenly spaces sample `i` of `n` across the plot width (first on the left edge, last
/// on the right). A single point sits at the horizontal center (SOUL §7.3).
fn map_x(i: usize, n: usize, plot: Rect) -> f32 {
    if n <= 1 {
        plot.x + plot.width * 0.5
    } else {
        plot.x + plot.width * (i as f32 / (n - 1) as f32)
    }
}

/// `r` shrunk by `by` on every side, never below one logical pixel.
fn inset(r: Rect, by: f32) -> Rect {
    let w = (r.width - 2.0 * by).max(1.0);
    let h = (r.height - 2.0 * by).max(1.0);
    Rect::new(r.x + (r.width - w) * 0.5, r.y + (r.height - h) * 0.5, w, h)
}

/// The deterministic accessible summary of a series (SOUL §6.1). EXACT format
/// `"n={count} min={min} max={max} last={last}"` — each numeric field via
/// `format!("{v}")` on the `f32` — or `"n=0"` alone for empty data. `min`/`max` are
/// taken over the *finite* values (a series with no finite value reports `min=0 max=0`,
/// keeping the string clean and diffable); `last` is the final element verbatim.
pub fn summary(values: &[f32]) -> String {
    if values.is_empty() {
        return "n=0".to_string();
    }
    let count = values.len();
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for &v in values {
        if v.is_finite() {
            min = min.min(v);
            max = max.max(v);
        }
    }
    if !min.is_finite() {
        min = 0.0;
    }
    if !max.is_finite() {
        max = 0.0;
    }
    let last = *values.last().unwrap();
    format!("n={count} min={min} max={max} last={last}")
}

// ---------------------------------------------------------------------------
// primitive builders (pure — SOUL §3.2; unit-tested directly)
// ---------------------------------------------------------------------------

/// How a bar chart lays its bars across the plot: `(x of bar 0, bar width, slot pitch)`.
/// Plain mode packs bars edge to edge with a `BAR_GAP` gap (`bar_w = ((plot_w -
/// gap*(n-1)) / n).max(1.0)`); `spaced` (axes on) gives each bar a centered share of an
/// equal slot, so a category label centers under its bar.
fn bar_layout(plot: Rect, n: usize, spaced: bool) -> (f32, f32, f32) {
    let n = n.max(1) as f32;
    if spaced {
        let slot = plot.width / n;
        let bar_w = (slot * BAR_FILL).max(1.0);
        (plot.x + (slot - bar_w) * 0.5, bar_w, slot)
    } else {
        let bar_w = ((plot.width - BAR_GAP * (n - 1.0)) / n).max(1.0);
        (plot.x, bar_w, bar_w + BAR_GAP)
    }
}

/// The colors a chart paints with, resolved once from the ambient theme at build.
#[derive(Clone, Copy)]
struct Palette {
    /// zero line / baseline.
    axis: Color,
    /// gridlines.
    grid: Color,
    /// tick and category labels.
    label: Color,
    /// the page colour, ringing a hovered point.
    halo: Color,
}

fn hline(plot: Rect, y: f32, width: f32, color: Color) -> Primitive {
    Primitive::Line {
        from: Point { x: plot.x, y },
        to: Point { x: plot.right(), y },
        width,
        color,
    }
}

/// Builds a bar chart's primitives within `plot` over the value range `[lo, hi]`
/// (SOUL §3.2). Bars anchor to the zero baseline; negative values hang *below* it in
/// `negative`; corners are sharp (`corner_radius 0`); adjacent bars keep a `≥ BAR_GAP`
/// gap. The optional baseline is emitted **after** the bars so it stays visible across
/// them. Empty data yields only the baseline (if enabled) — still a valid box (SOUL
/// §7.3).
#[allow(clippy::too_many_arguments)]
fn bar_primitives(
    plot: Rect,
    values: &[f32],
    (lo, hi): (f32, f32),
    color: Color,
    negative: Color,
    baseline: bool,
    spaced: bool,
    pal: Palette,
) -> Vec<Primitive> {
    let baseline_y = map_y(0.0, lo, hi, plot);
    let mut prims = Vec::new();
    let n = values.len();
    if n > 0 {
        let (x0, bar_w, slot) = bar_layout(plot, n, spaced);
        for (i, &v) in values.iter().enumerate() {
            if !v.is_finite() {
                continue;
            }
            let x = x0 + i as f32 * slot;
            let val_y = map_y(v, lo, hi, plot);
            // The bar spans from the baseline to the value: positive above, negative
            // below. `top`/`height` handle both signs without a branch.
            let top = baseline_y.min(val_y);
            let height = (baseline_y - val_y).abs();
            prims.push(Primitive::SolidRect {
                rect: Rect::new(x, top, bar_w, height),
                color: if v < 0.0 { negative } else { color },
                corner_radius: 0.0,
            });
        }
    }
    if baseline {
        prims.push(hline(plot, baseline_y, AXIS_WIDTH, pal.axis));
    }
    prims
}

/// The finite points of a line, evenly spaced by *original* index (skipping a
/// non-finite value widens a segment instead of compressing the rest — SOUL §7.3).
/// Each point keeps its source index for labels and tooltips.
fn line_points(data: Rect, values: &[f32], (lo, hi): (f32, f32)) -> Vec<(usize, Point)> {
    let n = values.len();
    values
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_finite())
        .map(|(i, &v)| {
            (
                i,
                Point {
                    x: map_x(i, n, data),
                    y: map_y(v, lo, hi, data),
                },
            )
        })
        .collect()
}

/// Builds a line chart's primitives (SOUL §3.2): the optional zero baseline first, then
/// `finite-1` [`Primitive::Line`] segments joining consecutive points, then one round
/// marker per point when `markers` is set. Points map into `data`, the plot inset by the
/// marker radius, so a marker on the extreme never leaves the plot.
#[allow(clippy::too_many_arguments)]
fn line_primitives(
    plot: Rect,
    data: Rect,
    values: &[f32],
    range: (f32, f32),
    color: Color,
    stroke_width: f32,
    markers: bool,
    baseline: bool,
    pal: Palette,
) -> Vec<Primitive> {
    let mut prims = Vec::new();
    if baseline {
        prims.push(hline(
            plot,
            map_y(0.0, range.0, range.1, data),
            AXIS_WIDTH,
            pal.axis,
        ));
    }
    let pts = line_points(data, values, range);
    for seg in pts.windows(2) {
        prims.push(Primitive::Line {
            from: seg[0].1,
            to: seg[1].1,
            width: stroke_width,
            color,
        });
    }
    if markers {
        for (_, p) in &pts {
            prims.push(dot(*p, MARKER_SIZE * 0.5, color));
        }
    }
    prims
}

/// A filled circle of radius `r` centered on `p`.
fn dot(p: Point, r: f32, color: Color) -> Primitive {
    Primitive::SolidRect {
        rect: Rect::new(p.x - r, p.y - r, 2.0 * r, 2.0 * r),
        color,
        corner_radius: r,
    }
}

// ---------------------------------------------------------------------------
// shared build path
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bars,
    Line,
    Spark,
}

/// Everything a chart view carries into [`build_chart`]; the three public types are
/// thin builders over this one description.
struct Spec {
    kind: Kind,
    values: Vec<f32>,
    title: Option<Cow<'static, str>>,
    size: Size,
    color: Color,
    negative: Color,
    stroke: f32,
    markers: bool,
    baseline: bool,
    axes: bool,
    labels: Vec<Cow<'static, str>>,
    unit: Option<Cow<'static, str>>,
    prefix: Option<Cow<'static, str>>,
    tooltips: bool,
    gutter: f32,
    end_dot: bool,
}

impl Spec {
    fn new(kind: Kind, values: Vec<f32>, size: Size, stroke: f32) -> Spec {
        Spec {
            kind,
            values,
            title: None,
            size,
            color: SERIES[0],
            negative: SERIES[5],
            stroke,
            markers: false,
            baseline: kind != Kind::Spark,
            axes: false,
            labels: Vec::new(),
            unit: None,
            prefix: None,
            tooltips: false,
            gutter: 0.0,
            end_dot: false,
        }
    }
}

/// Shapes `text` at the label size, returning the run and its logical width.
fn shape_label(text: &mut TextShaper, s: &str, scale: f32) -> (ShapedText, f32) {
    let phys = (LABEL_SIZE * scale).round().max(1.0);
    let shaped = text.shape(s, phys, None);
    let w = shaped.width / scale;
    (shaped, w)
}

/// Paints `shaped` with its left edge at `x` and its vertical center at `cy`.
#[allow(clippy::too_many_arguments)]
fn push_label(
    pd: &mut PaintData,
    text: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    shaped: &ShapedText,
    color: Color,
    scale: f32,
    x: f32,
    cy: f32,
) {
    let phys = (LABEL_SIZE * scale).round().max(1.0);
    let h = shaped.height / scale;
    rasterize_and_push(
        pd,
        text,
        atlas,
        shaped,
        phys as u32,
        color,
        scale,
        Point { x, y: cy - h * 0.5 },
    );
}

/// Inserts one invisible hover target at `rect` (chart-local, relative to the stack
/// root) that shows `label` as a tooltip. The node is a `Group` named with the same
/// text, so the readout is also in the accessibility tree.
/// Adds one hover target over a mark. `highlights` (at most two, in mark-local
/// coordinates and their shown colours) paint only while the tooltip shows, so the
/// hovered bar or point stands out from its neighbours.
fn add_mark(
    ctx: &mut BuildCtx,
    stack: WidgetId,
    rect: Rect,
    label: &str,
    highlights: [Option<Primitive>; 2],
) {
    let id = ctx.scene.insert(WidgetKind::Stack, Some(stack));
    ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
    ctx.scene.a11y_mut(id).name = Some(label.to_string());
    let size = Size {
        width: rect.width.max(1.0),
        height: rect.height.max(1.0),
    };
    let mut style = ContainerStyle::new(Container::Stack);
    style.fixed_size = Some(size);
    style.anchor = Some(Point {
        x: rect.x,
        y: rect.y,
    });
    // The box marks a data point, so it stays on the point.
    style.keep_in_view = false;
    ctx.layout.set_container(id, style);
    // One transparent rect: the node's ordinary paint, which anchors the tooltip
    // fragment appended after it and gives the repositioning pass an origin.
    ctx.scene.replace_primitives(
        id,
        [Primitive::SolidRect {
            rect: Rect::new(0.0, 0.0, size.width, size.height),
            color: Color::TRANSPARENT,
            corner_radius: 0.0,
        }],
    );
    let mut shown = [None; 2];
    for (slot, highlight) in shown.iter_mut().zip(highlights) {
        let Some(mut primitive) = highlight else {
            continue;
        };
        let color = match &mut primitive {
            Primitive::SolidRect { color, .. } | Primitive::StrokeRect { color, .. } => {
                std::mem::replace(color, Color::TRANSPARENT)
            }
            _ => continue,
        };
        let pd = ctx.scene.paint_mut(id);
        *slot = Some((pd.primitives.len(), color));
        pd.primitives.push(primitive);
    }
    schnellui::widgets::register_hover_tooltip(
        &ctx.runtime,
        ctx.scene,
        ctx.text,
        ctx.atlas,
        id,
        label,
        size,
        ctx.scale,
    );
    schnellui::widgets::set_tooltip_highlights(&ctx.runtime, id, shown);
    schnellui::widgets::elevate_tooltip_host(ctx.scene, id);
}

/// The smallest gap between two shown category labels, in logical px.
const CATEGORY_LABEL_MIN_GAP: f32 = 8.0;

/// How many category labels to step over so the shown ones keep
/// [`CATEGORY_LABEL_MIN_GAP`] between them: `1` shows every label, `2` every other
/// one. Counting runs back from the last label, so the latest category always shows.
/// `spans` holds each label's left edge and width, left to right.
fn category_label_stride(spans: &[(f32, f32)]) -> usize {
    let n = spans.len();
    (1..n.max(1))
        .find(|&k| {
            let shown: Vec<&(f32, f32)> = spans.iter().rev().step_by(k).collect();
            shown
                .windows(2)
                .all(|pair| pair[0].0 >= pair[1].0 + pair[1].1 + CATEGORY_LABEL_MIN_GAP)
        })
        .unwrap_or(n.max(1))
}

/// Builds one chart: the `Chart` leaf with its a11y surface and primitives, and — with
/// tooltips on — a fixed-size `Stack` root around it plus one hover node per mark.
/// Returns the node to attach to the parent.
fn build_chart(ctx: &mut BuildCtx, parent: Option<WidgetId>, spec: Spec) -> WidgetId {
    let Spec {
        kind,
        values,
        title,
        size,
        color,
        negative,
        stroke,
        markers,
        baseline,
        axes,
        labels,
        unit,
        prefix,
        tooltips,
        gutter,
        end_dot,
    } = spec;
    let n = values.len();
    let scale = if ctx.scale.is_finite() && ctx.scale > 0.0 {
        ctx.scale
    } else {
        1.0
    };

    // Structure. With tooltips the leaf is pinned inside a fixed-size stack, next to
    // the hover nodes; otherwise the leaf alone is the chart, as it always was.
    let root = if tooltips {
        let stack = ctx.scene.insert(WidgetKind::Stack, parent);
        ctx.scene.a11y_mut(stack).role = Role::Group.as_u16();
        let mut style = ContainerStyle::new(Container::Stack);
        style.fixed_size = Some(size);
        ctx.layout.set_container(stack, style);
        Some(stack)
    } else {
        None
    };
    let id = ctx.scene.insert(WidgetKind::Chart, root.or(parent));
    {
        // Semantic before visual (SOUL §6.1): a figure with a deterministic summary.
        let a = ctx.scene.a11y_mut(id);
        a.role = Role::Chart.as_u16();
        a.name = title.map(|c| c.into_owned());
        a.value = Some(summary(&values));
    }
    // A chart is a fixed-size box; layout positions it, the chart never resizes itself.
    ctx.layout.set_measure(id, Box::new(move |_avail| size));
    let area = node_rect(ctx.scene, id, size);

    let pal = {
        let t = schnellui::widgets::theme_for(&ctx.runtime, id);
        Palette {
            axis: t.outline,
            grid: t.separator,
            label: t.text_muted,
            halo: t.surface,
        }
    };

    // Value range, extended to whole ticks when axes are on.
    let raw = value_range(&values, kind == Kind::Bars);
    let target = (((size.height - 2.0 * LABEL_SIZE) / TICK_SPACING).round() as i32).clamp(2, 6);
    let nice = axes.then(|| nice_axis(raw.0, raw.1, target as usize));
    let range = nice.as_ref().map_or(raw, |a| (a.lo, a.hi));

    // Label runs, shaped up front: their widths set the gutters.
    let tick_labels: Vec<(ShapedText, f32)> = nice
        .iter()
        .flat_map(|a| a.ticks.iter().map(|&t| format_tick(t, a.step)))
        .map(|s| shape_label(ctx.text, &s, scale))
        .collect();
    let cat_labels: Vec<(ShapedText, f32)> = if axes {
        labels
            .iter()
            .take(n)
            .map(|s| shape_label(ctx.text, s, scale))
            .collect()
    } else {
        Vec::new()
    };
    let text_h = tick_labels
        .first()
        .or(cat_labels.first())
        .map_or(LABEL_SIZE, |(s, _)| s.height / scale);

    // Plot rect: the whole box in plain mode; with axes the gutters make room for the
    // tick labels (left, shared via `.gutter`) and the category labels (bottom).
    let (left, top, bottom, right) = if axes {
        let tick_w = tick_labels.iter().fold(0.0f32, |m, (_, w)| m.max(*w));
        let left = (tick_w + LABEL_GAP).max(gutter);
        // The last category label is centered on its mark; keep what hangs past the
        // mark's own half-extent inside the box.
        let clearance = match kind {
            Kind::Bars => (area.width - left) / n.max(1) as f32 * 0.5,
            _ if markers => MARKER_SIZE * 0.5,
            _ => stroke * 0.5,
        };
        let right = cat_labels
            .last()
            .map_or(0.0, |(_, w)| (w * 0.5 - clearance).max(0.0).ceil());
        (
            left,
            (text_h * 0.5).ceil(),
            if cat_labels.is_empty() {
                (text_h * 0.5).ceil()
            } else {
                (text_h + LABEL_GAP).ceil()
            },
            right,
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    let plot = Rect::new(
        area.x + left,
        area.y + top,
        (area.width - left - right).max(1.0),
        (area.height - top - bottom).max(1.0),
    );
    // Lines map their points into the plot shrunk by the marker (or end dot, or half
    // stroke) radius, so nothing sits half outside it.
    let data = match kind {
        Kind::Bars => plot,
        Kind::Line if markers => inset(plot, MARKER_SIZE * 0.5),
        Kind::Line if axes => inset(plot, stroke * 0.5),
        Kind::Spark if end_dot => inset(plot, END_DOT_RADIUS.max(stroke * 0.5)),
        _ => plot,
    };

    let mut prims: Vec<Primitive> = Vec::new();
    if axes {
        // Anchor: the min-origin slide lands on the box corner, not on a label.
        prims.push(Primitive::SolidRect {
            rect: area,
            color: Color::TRANSPARENT,
            corner_radius: 0.0,
        });
        if let Some(a) = &nice {
            for &t in &a.ticks {
                // The zero line is drawn separately (and above the marks).
                let zero = t == 0.0 && baseline;
                let at_floor = t == a.lo && !(a.lo <= 0.0 && a.hi >= 0.0 && baseline);
                if zero {
                    continue;
                }
                let (w, c) = if at_floor {
                    (AXIS_WIDTH, pal.axis)
                } else {
                    (GRID_WIDTH, pal.grid)
                };
                prims.push(hline(plot, map_y(t, a.lo, a.hi, data), w, c));
            }
        }
    }
    // Zero line: over the bars, under the line; only where zero is in range once axes
    // pick the range (plain mode keeps the old clamp-to-edge behaviour).
    let zero_in_range = range.0 <= 0.0 && range.1 >= 0.0;
    let show_zero = baseline && (!axes || zero_in_range);
    match kind {
        Kind::Bars => prims.extend(bar_primitives(
            plot, &values, range, color, negative, show_zero, axes, pal,
        )),
        Kind::Line | Kind::Spark => {
            prims.extend(line_primitives(
                plot,
                data,
                &values,
                range,
                color,
                stroke,
                markers && kind == Kind::Line,
                show_zero,
                pal,
            ));
            if kind == Kind::Spark && end_dot {
                if let Some((_, p)) = line_points(data, &values, range).last() {
                    prims.push(dot(*p, END_DOT_RADIUS, color));
                }
            }
        }
    }
    ctx.scene.replace_primitives(id, prims);

    // Text: tick labels right-aligned against the plot, category labels centered under
    // their marks and thinned left to right so neighbours never touch.
    if axes {
        let pd = ctx.scene.paint_mut(id);
        if let Some(a) = &nice {
            for (&t, (shaped, w)) in a.ticks.iter().zip(&tick_labels) {
                let y = map_y(t, a.lo, a.hi, data);
                push_label(
                    pd,
                    ctx.text,
                    ctx.atlas,
                    shaped,
                    pal.label,
                    scale,
                    plot.x - LABEL_GAP - w,
                    y,
                );
            }
        }
        let (bar_x0, bar_w, slot) = bar_layout(plot, n, true);
        let spans: Vec<(f32, f32)> = cat_labels
            .iter()
            .enumerate()
            .map(|(i, (_, w))| {
                let cx = match kind {
                    Kind::Bars => bar_x0 + i as f32 * slot + bar_w * 0.5,
                    _ => map_x(i, n, data),
                };
                (
                    (cx - w * 0.5).clamp(area.x, (area.right() - w).max(area.x)),
                    *w,
                )
            })
            .collect();
        let stride = category_label_stride(&spans);
        let last = spans.len().saturating_sub(1);
        for (i, ((shaped, _), &(x, _))) in cat_labels.iter().zip(&spans).enumerate() {
            if !(last - i).is_multiple_of(stride) {
                continue;
            }
            push_label(
                pd,
                ctx.text,
                ctx.atlas,
                shaped,
                pal.label,
                scale,
                x,
                plot.bottom() + LABEL_GAP + text_h * 0.5,
            );
        }
    }

    // Hover targets, one per finite mark, in chart-local coordinates.
    if let Some(stack) = root {
        let text_for = |i: usize, v: f32| {
            let value = with_unit(&format_value(v), prefix.as_deref(), unit.as_deref());
            match labels.get(i) {
                Some(l) => format!("{l}: {value}"),
                None => value,
            }
        };
        let local = |r: Rect| Rect::new(r.x - area.x, r.y - area.y, r.width, r.height);
        match kind {
            Kind::Bars => {
                let (x0, bar_w, slot) = bar_layout(plot, n, axes);
                let baseline_y = map_y(0.0, range.0, range.1, plot);
                for (i, &v) in values.iter().enumerate() {
                    if !v.is_finite() {
                        continue;
                    }
                    let vy = map_y(v, range.0, range.1, plot);
                    let (mut top, mut h) = (baseline_y.min(vy), (baseline_y - vy).abs());
                    if h < HIT_MIN {
                        // grow away from the baseline so the target stays reachable
                        if v < 0.0 {
                            h = HIT_MIN;
                        } else {
                            top -= HIT_MIN - h;
                            h = HIT_MIN;
                        }
                    }
                    let x = x0 + i as f32 * slot - (slot - bar_w) * 0.5;
                    // The bar itself, in mark-local coordinates, darkened.
                    let bar = Rect::new(
                        (slot - bar_w) * 0.5,
                        baseline_y.min(vy) - top,
                        bar_w,
                        (baseline_y - vy).abs(),
                    );
                    add_mark(
                        ctx,
                        stack,
                        local(Rect::new(x, top, slot, h)),
                        &text_for(i, v),
                        [
                            Some(Primitive::SolidRect {
                                rect: bar,
                                color: MARK_HOVER_SHADE,
                                corner_radius: 0.0,
                            }),
                            None,
                        ],
                    );
                }
            }
            _ => {
                for (i, p) in line_points(data, &values, range) {
                    let r = Rect::new(
                        p.x - HIT_SIZE * 0.5,
                        p.y - HIT_SIZE * 0.5,
                        HIT_SIZE,
                        HIT_SIZE,
                    );
                    // A larger point with a halo in the page colour, centred on the mark.
                    let c = Point {
                        x: HIT_SIZE * 0.5,
                        y: HIT_SIZE * 0.5,
                    };
                    add_mark(
                        ctx,
                        stack,
                        local(r),
                        &text_for(i, values[i]),
                        [
                            Some(dot(c, HOVER_POINT_RADIUS + HOVER_HALO, pal.halo)),
                            Some(dot(c, HOVER_POINT_RADIUS, color)),
                        ],
                    );
                }
            }
        }
    }
    root.unwrap_or(id)
}

// ---------------------------------------------------------------------------
// BarChart
// ---------------------------------------------------------------------------

/// A single-series bar chart leaf (SOUL §8.1). Bars anchor to the zero baseline;
/// negative values hang below it in their own color. `.title()` sets **only** the
/// accessible name (SOUL §6.1) — it is *not* painted; compose a `Text` above the chart
/// for a visible title. `.axes(true)`, `.labels(..)` and `.tooltips(true)` make the
/// values readable (see the module docs).
pub struct BarChart {
    spec: Spec,
}

impl BarChart {
    /// A bar chart over `values` (default size 240×140, color `SERIES[0]`, negatives
    /// `SERIES[5]`, baseline on, no axes, no tooltips).
    pub fn new(values: impl Into<Vec<f32>>) -> BarChart {
        BarChart {
            spec: Spec::new(Kind::Bars, values.into(), DEFAULT_SIZE, 0.0),
        }
    }

    /// Sets the **accessible name** (SOUL §6.1). Not painted — compose a `Text` title
    /// above the chart for a visible one.
    pub fn title(mut self, title: impl Into<Cow<'static, str>>) -> BarChart {
        self.spec.title = Some(title.into());
        self
    }

    /// Sets the chart's fixed size in logical px (default 240×140).
    pub fn size(mut self, w: f32, h: f32) -> BarChart {
        self.spec.size = Size {
            width: w,
            height: h,
        };
        self
    }

    /// Sets the bar fill color (default `SERIES[0]`).
    pub fn color(mut self, color: Color) -> BarChart {
        self.spec.color = color;
        self
    }

    /// Sets the fill of bars below zero (default `SERIES[5]`, red), so a negative reads
    /// as one without comparing positions.
    pub fn negative_color(mut self, color: Color) -> BarChart {
        self.spec.negative = color;
        self
    }

    /// Toggles the zero-line [`AXIS`] (default `true`), drawn over the bars.
    pub fn baseline(mut self, baseline: bool) -> BarChart {
        self.spec.baseline = baseline;
        self
    }

    /// Adds a value axis: gridlines, tick labels and centered bars (default `false`).
    pub fn axes(mut self, axes: bool) -> BarChart {
        self.spec.axes = axes;
        self
    }

    /// Category labels, one per bar, shown under the bars (needs `.axes(true)`) and in
    /// the tooltips. Labels that would overlap are skipped.
    pub fn labels<S: Into<Cow<'static, str>>>(
        mut self,
        labels: impl IntoIterator<Item = S>,
    ) -> BarChart {
        self.spec.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    /// The unit appended to tooltip values, such as `"ms"`, `"%"` or `"k users"`.
    pub fn unit(mut self, unit: impl Into<Cow<'static, str>>) -> BarChart {
        self.spec.unit = Some(unit.into());
        self
    }

    /// Text put before tooltip values, such as `"$"`, so `.prefix("$").unit("k")`
    /// reads `Feb: $7k`.
    pub fn prefix(mut self, prefix: impl Into<Cow<'static, str>>) -> BarChart {
        self.spec.prefix = Some(prefix.into());
        self
    }

    /// Makes each bar a hover target showing `"Label: value unit"` (default `false`).
    pub fn tooltips(mut self, tooltips: bool) -> BarChart {
        self.spec.tooltips = tooltips;
        self
    }

    /// Minimum width of the tick-label gutter in px, so charts with different tick
    /// labels still share one plot width (needs `.axes(true)`).
    pub fn gutter(mut self, gutter: f32) -> BarChart {
        self.spec.gutter = gutter.max(0.0);
        self
    }

    /// The scene dispatch tag for this leaf.
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Chart
    }

    /// The accessible role (SOUL §6.1).
    pub fn role(&self) -> Role {
        Role::Chart
    }
}

impl View for BarChart {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        build_chart(ctx, parent, self.spec)
    }
}

// ---------------------------------------------------------------------------
// LineChart
// ---------------------------------------------------------------------------

/// A single-series line chart leaf (SOUL §8.1): `n-1` [`Primitive::Line`] segments
/// joining consecutive points, an optional zero baseline, and optional round per-point
/// markers. `.title()` sets **only** the accessible name (SOUL §6.1). Axes, category
/// labels and tooltips are opt-in exactly as on [`BarChart`].
pub struct LineChart {
    spec: Spec,
}

impl LineChart {
    /// A line chart over `values` (default size 240×140, color `SERIES[0]`, stroke 2.0,
    /// no markers, baseline on, no axes, no tooltips).
    pub fn new(values: impl Into<Vec<f32>>) -> LineChart {
        LineChart {
            spec: Spec::new(Kind::Line, values.into(), DEFAULT_SIZE, LINE_STROKE),
        }
    }

    /// Sets the **accessible name** (SOUL §6.1). Not painted.
    pub fn title(mut self, title: impl Into<Cow<'static, str>>) -> LineChart {
        self.spec.title = Some(title.into());
        self
    }

    /// Sets the chart's fixed size in logical px (default 240×140).
    pub fn size(mut self, w: f32, h: f32) -> LineChart {
        self.spec.size = Size {
            width: w,
            height: h,
        };
        self
    }

    /// Sets the line color (default `SERIES[0]`).
    pub fn color(mut self, color: Color) -> LineChart {
        self.spec.color = color;
        self
    }

    /// Sets the line stroke width in logical px (default 2.0).
    pub fn stroke_width(mut self, width: f32) -> LineChart {
        self.spec.stroke = width;
        self
    }

    /// Toggles round 9 px per-point markers (default `false`). The plot is inset by the
    /// marker radius so edge points stay inside it.
    pub fn markers(mut self, markers: bool) -> LineChart {
        self.spec.markers = markers;
        self
    }

    /// Toggles the zero-line [`AXIS`] (default `true`).
    pub fn baseline(mut self, baseline: bool) -> LineChart {
        self.spec.baseline = baseline;
        self
    }

    /// Adds a value axis: gridlines and tick labels (default `false`).
    pub fn axes(mut self, axes: bool) -> LineChart {
        self.spec.axes = axes;
        self
    }

    /// Category labels, one per point, shown under the plot (needs `.axes(true)`) and in
    /// the tooltips. Labels that would overlap are skipped.
    pub fn labels<S: Into<Cow<'static, str>>>(
        mut self,
        labels: impl IntoIterator<Item = S>,
    ) -> LineChart {
        self.spec.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    /// The unit appended to tooltip values, such as `"ms"`, `"%"` or `"k users"`.
    pub fn unit(mut self, unit: impl Into<Cow<'static, str>>) -> LineChart {
        self.spec.unit = Some(unit.into());
        self
    }

    /// Text put before tooltip values, such as `"$"`, so `.prefix("$").unit("k")`
    /// reads `Feb: $7k`.
    pub fn prefix(mut self, prefix: impl Into<Cow<'static, str>>) -> LineChart {
        self.spec.prefix = Some(prefix.into());
        self
    }

    /// Makes each point a hover target showing `"Label: value unit"` (default `false`).
    pub fn tooltips(mut self, tooltips: bool) -> LineChart {
        self.spec.tooltips = tooltips;
        self
    }

    /// Minimum width of the tick-label gutter in px, so charts with different tick
    /// labels still share one plot width (needs `.axes(true)`).
    pub fn gutter(mut self, gutter: f32) -> LineChart {
        self.spec.gutter = gutter.max(0.0);
        self
    }

    /// The scene dispatch tag for this leaf.
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Chart
    }

    /// The accessible role (SOUL §6.1).
    pub fn role(&self) -> Role {
        Role::Chart
    }
}

impl View for LineChart {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        build_chart(ctx, parent, self.spec)
    }
}

// ---------------------------------------------------------------------------
// Sparkline
// ---------------------------------------------------------------------------

/// A minimal single-series sparkline leaf (SOUL §8.1): line segments only — no
/// baseline, no markers, stroke 2, default size 80×20 — and an optional end dot on the
/// latest value. Still carries the first-class a11y summary (SOUL §6.1) like every
/// chart. Pair it with a `Text` for the value it ends on.
pub struct Sparkline {
    spec: Spec,
}

impl Sparkline {
    /// A sparkline over `values` (default size 80×20, color `SERIES[0]`).
    pub fn new(values: impl Into<Vec<f32>>) -> Sparkline {
        Sparkline {
            spec: Spec::new(Kind::Spark, values.into(), SPARK_SIZE, SPARK_STROKE),
        }
    }

    /// Sets the line color (default `SERIES[0]`).
    pub fn color(mut self, color: Color) -> Sparkline {
        self.spec.color = color;
        self
    }

    /// Sets the sparkline's fixed size in logical px (default 80×20).
    pub fn size(mut self, w: f32, h: f32) -> Sparkline {
        self.spec.size = Size {
            width: w,
            height: h,
        };
        self
    }

    /// Sets the line stroke width in logical px (default 2.0).
    pub fn stroke_width(mut self, width: f32) -> Sparkline {
        self.spec.stroke = width;
        self
    }

    /// Marks the latest point with a dot (default `false`).
    pub fn end_dot(mut self, end_dot: bool) -> Sparkline {
        self.spec.end_dot = end_dot;
        self
    }

    /// Draws the zero line under the trend, pinned to the bottom edge when every value
    /// is above zero (default `false`). It gives the trend a floor to read against.
    pub fn baseline(mut self, baseline: bool) -> Sparkline {
        self.spec.baseline = baseline;
        self
    }

    /// The scene dispatch tag for this leaf.
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Chart
    }

    /// The accessible role (SOUL §6.1).
    pub fn role(&self) -> Role {
        Role::Chart
    }
}

impl View for Sparkline {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        // No title builder — a sparkline is nameless (minimal), value = summary only.
        build_chart(ctx, parent, self.spec)
    }
}

#[cfg(test)]
mod tests_axes;

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::layout::LayoutEngine;
    use schnellui::scene::Scene;
    use schnellui::text::{GlyphAtlas, TextShaper};

    /// Builds `view` into a fresh scene, returning the scene + engines and the root id
    /// (our own copy of the widgets crate's private `build_one` helper). Layout is
    /// **not** computed, so primitives stay at their provisional `(0,0)` plot origin —
    /// geometry assertions read absolute coordinates within `[0,w] × [0,h]`.
    fn build_one(view: impl View) -> (Scene, WidgetId) {
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(256, 256);
        let id = {
            let mut ctx = BuildCtx {
                context: schnellui::widgets::Context::new(),
                runtime: schnellui::widgets::Runtime::new(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(view).build(&mut ctx, None)
        };
        scene.set_root(id);
        (scene, id)
    }

    fn prims_of(scene: &Scene, id: WidgetId) -> Vec<Primitive> {
        scene.paint(id).unwrap().primitives.clone()
    }

    fn solid_rects(prims: &[Primitive]) -> Vec<Rect> {
        prims
            .iter()
            .filter_map(|p| match p {
                Primitive::SolidRect { rect, .. } => Some(*rect),
                _ => None,
            })
            .collect()
    }

    fn lines(prims: &[Primitive]) -> Vec<(Point, Point, f32, Color)> {
        prims
            .iter()
            .filter_map(|p| match *p {
                Primitive::Line {
                    from,
                    to,
                    width,
                    color,
                } => Some((from, to, width, color)),
                _ => None,
            })
            .collect()
    }

    // --- palette (the data-viz standard, SOUL §6.1) ---

    #[test]
    fn series_palette_is_eight_in_fixed_order() {
        assert_eq!(SERIES.len(), 8);
        // First slot is the blue single-series default, verbatim from the standard.
        assert_eq!(SERIES[0], Color::rgb(0x2a, 0x78, 0xd6));
        // A couple of later slots to pin the fixed order.
        assert_eq!(SERIES[3], Color::rgb(0x00, 0x83, 0x00)); // green
        assert_eq!(SERIES[7], Color::rgb(0xd1, 0x53, 0x1f)); // orange
        assert_eq!(AXIS, Color::rgb(0x7a, 0x86, 0x99));
    }

    // --- value normalization (SOUL §7.3; tested directly) ---

    #[test]
    fn value_range_bars_include_zero_lines_do_not() {
        // all-positive: bars fold zero into lo, lines keep the plain range.
        assert_eq!(value_range(&[3.0, 7.0, 2.0, 9.0], true), (0.0, 9.0));
        assert_eq!(value_range(&[3.0, 7.0, 2.0, 9.0], false), (2.0, 9.0));
        // mixed sign: zero already inside → identical for both.
        assert_eq!(value_range(&[5.0, -3.0, 8.0], true), (-3.0, 8.0));
        // all-negative: bars fold zero into hi.
        assert_eq!(value_range(&[-2.0, -5.0], true), (-5.0, 0.0));
        assert_eq!(value_range(&[-2.0, -5.0], false), (-5.0, -2.0));
        // empty / non-finite → degenerate (0, 0), never NaN.
        assert_eq!(value_range(&[], false), (0.0, 0.0));
        assert_eq!(value_range(&[f32::NAN, f32::INFINITY], false), (0.0, 0.0));
        // a NaN mixed in is skipped, finite range survives.
        assert_eq!(value_range(&[3.0, f32::NAN, 9.0], false), (3.0, 9.0));
    }

    #[test]
    fn map_y_endpoints_and_degenerate_center() {
        let plot = Rect::new(0.0, 0.0, 100.0, 140.0);
        // hi maps to the top (y = plot.y), lo to the bottom (y = plot.bottom()).
        assert!((map_y(9.0, 0.0, 9.0, plot) - 0.0).abs() < 1e-4);
        assert!((map_y(0.0, 0.0, 9.0, plot) - 140.0).abs() < 1e-4);
        // midpoint.
        assert!((map_y(4.5, 0.0, 9.0, plot) - 70.0).abs() < 1e-4);
        // degenerate range → vertical center, no divide-by-zero.
        let c = map_y(5.0, 5.0, 5.0, plot);
        assert!((c - 70.0).abs() < 1e-4);
        assert!(c.is_finite());
        // out-of-range clamps into the plot.
        assert!((map_y(-100.0, 0.0, 9.0, plot) - 140.0).abs() < 1e-4);
        assert!((map_y(100.0, 0.0, 9.0, plot) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn map_x_spaces_evenly_and_centers_single() {
        let plot = Rect::new(0.0, 0.0, 100.0, 10.0);
        assert!((map_x(0, 5, plot) - 0.0).abs() < 1e-4);
        assert!((map_x(4, 5, plot) - 100.0).abs() < 1e-4);
        assert!((map_x(2, 5, plot) - 50.0).abs() < 1e-4);
        // a lone point sits at the horizontal center.
        assert!((map_x(0, 1, plot) - 50.0).abs() < 1e-4);
    }

    // --- accessible summary (exact strings, SOUL §6.1) ---

    #[test]
    fn summary_exact_strings() {
        assert_eq!(
            summary(&[3.0, 7.0, 2.0, 9.0, 4.0]),
            "n=5 min=2 max=9 last=4"
        );
        assert_eq!(summary(&[]), "n=0");
        assert_eq!(summary(&[5.0]), "n=1 min=5 max=5 last=5");
        assert_eq!(
            summary(&[5.0, -3.0, 8.0, -6.0, 2.0]),
            "n=5 min=-6 max=8 last=2"
        );
        // all-equal (flat) data.
        assert_eq!(summary(&[4.0, 4.0, 4.0]), "n=3 min=4 max=4 last=4");
        // a NaN is skipped for min/max but still counted and can be `last`.
        assert_eq!(summary(&[3.0, f32::NAN, 9.0]), "n=3 min=3 max=9 last=9");
    }

    // --- BarChart geometry (SOUL §3.2) ---

    #[test]
    fn bar_count_equals_len_and_shares_baseline_for_positive_data() {
        let values = [3.0, 7.0, 2.0, 9.0, 4.0];
        let (scene, id) = build_one(BarChart::new(values));
        let prims = prims_of(&scene, id);
        let bars = solid_rects(&prims);
        assert_eq!(bars.len(), values.len(), "one bar per value");

        // baseline hairline present (default on), spanning the full width at the bottom.
        let ls = lines(&prims);
        assert_eq!(ls.len(), 1, "one baseline hairline");
        let (from, to, w, col) = ls[0];
        assert_eq!(col, AXIS);
        assert!((w - AXIS_WIDTH).abs() < 1e-4);
        let baseline_y = from.y;
        assert!(
            (baseline_y - 140.0).abs() < 1e-4,
            "positive data → baseline at bottom"
        );
        assert!((from.x - 0.0).abs() < 1e-4 && (to.x - 240.0).abs() < 1e-4);

        // every positive bar's bottom sits on the shared baseline.
        for b in &bars {
            assert!(
                (b.bottom() - baseline_y).abs() < 1e-3,
                "bar bottom on baseline"
            );
        }
    }

    #[test]
    fn bar_max_value_spans_full_plot_height() {
        let (scene, id) = build_one(BarChart::new([3.0, 7.0, 2.0, 9.0, 4.0]));
        let bars = solid_rects(&prims_of(&scene, id));
        // the max (9, index 3) reaches the top (y ≈ 0) and spans the whole height.
        let tallest =
            bars.iter().cloned().fold(
                Rect::ZERO,
                |acc, b| {
                    if b.height > acc.height {
                        b
                    } else {
                        acc
                    }
                },
            );
        assert!((tallest.y - 0.0).abs() < 1e-3, "max bar reaches the top");
        assert!(
            (tallest.height - 140.0).abs() < 1e-3,
            "max bar spans full height"
        );
    }

    #[test]
    fn bar_gaps_are_at_least_two_px() {
        let (scene, id) = build_one(BarChart::new([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        let mut bars = solid_rects(&prims_of(&scene, id));
        bars.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        for pair in bars.windows(2) {
            let gap = pair[1].x - pair[0].right();
            assert!(
                gap >= BAR_GAP - 1e-3,
                "adjacent bars keep a ≥2px gap, got {gap}"
            );
        }
    }

    #[test]
    fn bar_negative_value_extends_below_baseline() {
        // mixed sign: baseline sits between; the negative bar hangs below it.
        let (scene, id) = build_one(BarChart::new([3.0, -2.0, 5.0]));
        let prims = prims_of(&scene, id);
        let baseline_y = lines(&prims)[0].0.y;
        let bars = solid_rects(&prims);
        // index 1 is the negative bar (second SolidRect after the baseline line).
        let neg = bars[1];
        assert!(
            (neg.y - baseline_y).abs() < 1e-3,
            "negative bar starts at the baseline"
        );
        assert!(
            neg.bottom() > baseline_y + 1e-3,
            "negative bar extends below the baseline"
        );
        // and a positive bar bottoms out on the baseline (above it).
        assert!((bars[0].bottom() - baseline_y).abs() < 1e-3);
        assert!(bars[0].y < baseline_y);
    }

    #[test]
    fn bar_baseline_toggle_off_drops_the_hairline() {
        let (scene, id) = build_one(BarChart::new([1.0, 2.0]).baseline(false));
        let prims = prims_of(&scene, id);
        assert!(lines(&prims).is_empty(), "no baseline when toggled off");
        assert_eq!(solid_rects(&prims).len(), 2);
    }

    #[test]
    fn bar_custom_color_and_size_applied() {
        let (scene, id) = build_one(
            BarChart::new([1.0, 2.0])
                .color(SERIES[2])
                .size(300.0, 100.0),
        );
        let bars = solid_rects(&prims_of(&scene, id));
        // bars fill the custom width (2 bars + 1 gap == 300).
        let span = bars.last().unwrap().right() - bars[0].x;
        assert!((span - 300.0).abs() < 1e-3);
        // color honored.
        for p in prims_of(&scene, id) {
            if let Primitive::SolidRect { color, .. } = p {
                assert_eq!(color, SERIES[2]);
            }
        }
    }

    // --- LineChart geometry (SOUL §3.2) ---

    #[test]
    fn line_emits_n_minus_one_segments_plus_markers_and_baseline() {
        let values = [
            4.0, 6.0, 5.0, 8.0, 7.0, 9.0, 6.0, 10.0, 8.0, 11.0, 9.0, 12.0,
        ];
        let n = values.len();
        let (scene, id) = build_one(
            LineChart::new(values)
                .markers(true)
                .baseline(true)
                .stroke_width(3.0),
        );
        let prims = prims_of(&scene, id);

        // segments: Lines with the series color + honored stroke width.
        let segs: Vec<_> = lines(&prims)
            .into_iter()
            .filter(|(_, _, _, c)| *c == SERIES[0])
            .collect();
        assert_eq!(segs.len(), n - 1, "n-1 joining segments");
        for (_, _, w, _) in &segs {
            assert!((w - 3.0).abs() < 1e-4, "stroke width honored");
        }

        // exactly one baseline hairline (AXIS colored).
        let base: Vec<_> = lines(&prims)
            .into_iter()
            .filter(|(_, _, _, c)| *c == AXIS)
            .collect();
        assert_eq!(base.len(), 1, "one baseline");

        // one 8×8 marker per point.
        let markers = solid_rects(&prims);
        assert_eq!(markers.len(), n, "one marker per point");
        for m in &markers {
            assert!((m.width - MARKER_SIZE).abs() < 1e-4 && (m.height - MARKER_SIZE).abs() < 1e-4);
        }
    }

    #[test]
    fn line_defaults_have_no_markers() {
        let (scene, id) = build_one(LineChart::new([1.0, 2.0, 3.0]));
        let prims = prims_of(&scene, id);
        assert!(solid_rects(&prims).is_empty(), "markers off by default");
        // 2 series segments + 1 baseline (default on).
        assert_eq!(lines(&prims).len(), 3);
    }

    #[test]
    fn line_skips_non_finite_points() {
        // 4 values, one NaN → 3 finite points → 2 segments; baseline still present.
        let (scene, id) = build_one(LineChart::new([1.0, f32::NAN, 3.0, 4.0]));
        let prims = prims_of(&scene, id);
        let segs: Vec<_> = lines(&prims)
            .into_iter()
            .filter(|(_, _, _, c)| *c == SERIES[0])
            .collect();
        assert_eq!(segs.len(), 2, "3 finite points → 2 segments");
    }

    // --- Sparkline (minimal — segments only) ---

    #[test]
    fn sparkline_emits_segments_only() {
        let values = [1.0, 3.0, 2.0, 5.0, 4.0, 6.0, 3.0, 7.0];
        let (scene, id) = build_one(Sparkline::new(values));
        let prims = prims_of(&scene, id);
        assert!(solid_rects(&prims).is_empty(), "no markers");
        let ls = lines(&prims);
        assert_eq!(ls.len(), values.len() - 1, "n-1 segments, no baseline");
        // no AXIS-colored hairline, fixed 1.5 stroke.
        for (_, _, w, c) in ls {
            assert_ne!(c, AXIS, "sparkline draws no baseline");
            assert!((w - SPARK_STROKE).abs() < 1e-4, "fixed 1.5 stroke");
        }
        // default minimal box.
        assert_eq!(
            scene.layout(id).map(|b| b.rect),
            None,
            "no layout computed in this helper"
        );
    }

    // --- edge cases: empty / single / flat / NaN all safe (SOUL §7.3) ---

    #[test]
    fn empty_bar_chart_emits_only_baseline() {
        let (scene, id) = build_one(BarChart::new(Vec::<f32>::new()));
        let prims = prims_of(&scene, id);
        assert!(solid_rects(&prims).is_empty(), "no bars for empty data");
        assert_eq!(lines(&prims).len(), 1, "just the baseline (a valid box)");
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("n=0"));
    }

    #[test]
    fn empty_line_and_sparkline_are_safe() {
        // no finite points → no segments, no panic, no NaN.
        let (scene, id) = build_one(LineChart::new(Vec::<f32>::new()));
        let segs: Vec<_> = lines(&prims_of(&scene, id))
            .into_iter()
            .filter(|(_, _, _, c)| *c == SERIES[0])
            .collect();
        assert!(segs.is_empty());

        let (scene, id) = build_one(Sparkline::new(Vec::<f32>::new()));
        assert!(
            prims_of(&scene, id).is_empty(),
            "empty sparkline is an empty box"
        );
    }

    #[test]
    fn single_and_flat_data_are_safe_and_finite() {
        // single value → one full-width, full-height bar, no divide-by-zero.
        let (scene, id) = build_one(BarChart::new([5.0]));
        let bars = solid_rects(&prims_of(&scene, id));
        assert_eq!(bars.len(), 1);
        assert!(
            (bars[0].width - 240.0).abs() < 1e-3,
            "lone bar fills the width"
        );
        for &c in &[bars[0].x, bars[0].y, bars[0].width, bars[0].height] {
            assert!(c.is_finite());
        }

        // flat data (all equal) → a flat line at the vertical center, finite coords.
        let (scene, id) = build_one(LineChart::new([4.0, 4.0, 4.0]).baseline(false));
        for (from, to, _, _) in lines(&prims_of(&scene, id)) {
            assert!((from.y - 70.0).abs() < 1e-3 && (to.y - 70.0).abs() < 1e-3);
        }
    }

    #[test]
    fn nan_only_data_does_not_panic_or_nan() {
        let (scene, id) = build_one(BarChart::new([f32::NAN, f32::INFINITY]));
        // no finite bar drawn, but the box + baseline are valid and finite.
        let prims = prims_of(&scene, id);
        assert!(solid_rects(&prims).is_empty());
        for (from, to, _, _) in lines(&prims) {
            assert!(from.y.is_finite() && to.y.is_finite());
        }
    }

    // --- accessibility (SOUL §6.1 — semantic before visual) ---

    #[test]
    fn build_writes_chart_role_name_and_summary_value() {
        let (scene, id) = build_one(BarChart::new([3.0, 7.0, 2.0, 9.0, 4.0]).title("Sales"));
        assert_eq!(scene.node(id).unwrap().kind, WidgetKind::Chart);
        let a = scene.a11y(id).expect("a11y column written at build");
        assert_eq!(Role::from_u16(a.role), Role::Chart);
        assert_eq!(a.name.as_deref(), Some("Sales"), "title → accessible name");
        assert_eq!(
            a.value.as_deref(),
            Some("n=5 min=2 max=9 last=4"),
            "value is the exact deterministic summary"
        );
    }

    #[test]
    fn untitled_chart_has_no_name_but_still_summarizes() {
        let (scene, id) = build_one(Sparkline::new([1.0, 2.0, 3.0]));
        let a = scene.a11y(id).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::Chart);
        assert!(a.name.is_none(), "sparkline is nameless (minimal)");
        assert_eq!(a.value.as_deref(), Some("n=3 min=1 max=3 last=3"));
    }

    #[test]
    fn title_is_not_painted() {
        // The title only names the figure — it emits no glyph quads (no text in v1).
        let (scene, id) = build_one(BarChart::new([1.0, 2.0]).title("Revenue"));
        for p in prims_of(&scene, id) {
            assert!(
                !matches!(p, Primitive::GlyphQuad { .. }),
                "charts paint no text in v1"
            );
        }
    }

    #[test]
    fn chart_kind_is_a_content_leaf() {
        assert_eq!(BarChart::new([1.0]).kind(), WidgetKind::Chart);
        assert_eq!(LineChart::new([1.0]).role(), Role::Chart);
        assert!(!WidgetKind::Chart.is_container());
    }
}
