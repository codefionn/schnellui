//! # schnellui-charts
//!
//! Chart content leaves (SOUL §8.1): [`BarChart`], [`LineChart`], [`Sparkline`].
//! Each is a [`View`](schnellui_widgets::View) that inserts a
//! [`WidgetKind::Chart`](schnellui_scene::WidgetKind::Chart) node, paints GPU
//! primitives into the retained scene (SOUL §3.2), registers a fixed-size intrinsic
//! measure (SOUL §8.1), and carries a **first-class accessible summary** (SOUL §6.1).
//!
//! ## What a chart is (and is not)
//!
//! A chart is a *content leaf*: it draws pixels and carries a `Role::Chart` (mapped to
//! accesskit `Figure`), exactly like the built-in leaves in `schnellui-widgets`. It is
//! **semantic before it is visual** (SOUL §6.1): its accessible *value* is a
//! deterministic one-line summary of the series (see [`summary`]), so a screen reader —
//! and the agent loop (SOUL §6.5) — can read *what the chart is* without scraping
//! pixels.
//!
//! ## Design rules (the project data-viz standard)
//!
//! These are encoded as constants below, not scattered magic numbers:
//! - a single validated colorblind-safe categorical palette, [`SERIES`], in **fixed
//!   order** — never reordered, never cycled;
//! - a recessive [`AXIS`] hairline for the zero baseline (1px); **no gridlines in v1**
//!   (keep marks dominant, chrome recessive);
//! - **no text of any kind is painted by charts in v1** — `.title()` sets *only* the
//!   accessible name; a visible title is composed as a `Text` above the chart.
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
//! crate's `reposition_paint` pass then slides the whole primitive set — bars, lines
//! *and* line endpoints — onto the node's laid-out origin.

use std::borrow::Cow;

use schnellui_a11y::Role;
use schnellui_scene::{Color, Point, Primitive, Rect, Size, WidgetId, WidgetKind};
use schnellui_widgets::{node_rect, BuildCtx, View};

// ---------------------------------------------------------------------------
// the data-viz standard, encoded as constants (SOUL §6.1)
// ---------------------------------------------------------------------------

/// The validated colorblind-safe **categorical** palette, in a **FIXED order** that is
/// never reordered and never cycled (the project data-viz standard). Single-series
/// charts default to `SERIES[0]` (blue); the array exists so a multi-chart dashboard
/// assigns distinct series colors *by entity in fixed order* — the same entity keeps
/// the same hue across every chart on the page.
///
/// Order: `#2a78d6` blue, `#1baf7a` aqua, `#eda100` yellow, `#008300` green,
/// `#4a3aa7` violet, `#e34948` red, `#e87ba4` magenta, `#eb6834` orange.
pub const SERIES: [Color; 8] = [
    Color::rgb(0x2a, 0x78, 0xd6), // 1 blue
    Color::rgb(0x1b, 0xaf, 0x7a), // 2 aqua
    Color::rgb(0xed, 0xa1, 0x00), // 3 yellow
    Color::rgb(0x00, 0x83, 0x00), // 4 green
    Color::rgb(0x4a, 0x3a, 0xa7), // 5 violet
    Color::rgb(0xe3, 0x49, 0x48), // 6 red
    Color::rgb(0xe8, 0x7b, 0xa4), // 7 magenta
    Color::rgb(0xeb, 0x68, 0x34), // 8 orange
];

/// The baseline / axis hairline color (`#c3c2b7`). Recessive chrome so the marks stay
/// dominant (the data-viz standard). **Gridlines are intentionally not drawn in v1.**
pub const AXIS: Color = Color::rgb(0xc3, 0xc2, 0xb7);

/// Baseline / axis hairline stroke width (logical px).
const AXIS_WIDTH: f32 = 1.0;
/// Minimum gap between adjacent bars (logical px) — `≥ 2` per the data-viz standard.
const BAR_GAP: f32 = 2.0;
/// Per-point marker square edge (logical px) for [`LineChart`] markers.
const MARKER_SIZE: f32 = 8.0;
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
/// Fixed [`Sparkline`] stroke width (logical px) — minimal, no builder.
const SPARK_STROKE: f32 = 1.5;

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

/// Builds a bar chart's primitives within `plot` (SOUL §3.2). Bars anchor to the zero
/// baseline (`lo = min(data_min, 0)`, `hi = max(data_max, 0)`); negative values hang
/// *below* the baseline; corners are sharp (`corner_radius 0`); adjacent bars keep a
/// `≥ BAR_GAP` gap (`bar_w = ((plot_w - gap*(n-1)) / n).max(1.0)`). The optional axis
/// hairline is emitted **first** so the bars sit atop it. Empty data yields only the
/// baseline (if enabled) — still a valid box (SOUL §7.3).
fn bar_primitives(plot: Rect, values: &[f32], color: Color, baseline: bool) -> Vec<Primitive> {
    let (lo, hi) = value_range(values, true);
    let baseline_y = map_y(0.0, lo, hi, plot);
    let mut prims = Vec::new();
    if baseline {
        prims.push(Primitive::Line {
            from: Point {
                x: plot.x,
                y: baseline_y,
            },
            to: Point {
                x: plot.right(),
                y: baseline_y,
            },
            width: AXIS_WIDTH,
            color: AXIS,
        });
    }
    let n = values.len();
    if n > 0 {
        let bar_w = ((plot.width - BAR_GAP * (n as f32 - 1.0)) / n as f32).max(1.0);
        for (i, &v) in values.iter().enumerate() {
            if !v.is_finite() {
                continue;
            }
            let x = plot.x + i as f32 * (bar_w + BAR_GAP);
            let val_y = map_y(v, lo, hi, plot);
            // The bar spans from the baseline to the value: positive above, negative
            // below. `top`/`height` handle both signs without a branch.
            let top = baseline_y.min(val_y);
            let height = (baseline_y - val_y).abs();
            prims.push(Primitive::SolidRect {
                rect: Rect::new(x, top, bar_w, height),
                color,
                corner_radius: 0.0,
            });
        }
    }
    prims
}

/// Builds a line chart's primitives within `plot` (SOUL §3.2): the optional zero
/// baseline hairline first, then `finite-1` [`Primitive::Line`] segments joining
/// consecutive **finite** points (non-finite points are skipped, so a gap just widens a
/// segment — SOUL §7.3), then one square marker per finite point when `markers` is set.
/// `x` is evenly spaced by *original* index (skipping preserves spacing); `y` maps the
/// plain `[data_min, data_max]` range.
fn line_primitives(
    plot: Rect,
    values: &[f32],
    color: Color,
    stroke_width: f32,
    markers: bool,
    baseline: bool,
) -> Vec<Primitive> {
    let (lo, hi) = value_range(values, false);
    let mut prims = Vec::new();
    if baseline {
        let baseline_y = map_y(0.0, lo, hi, plot);
        prims.push(Primitive::Line {
            from: Point {
                x: plot.x,
                y: baseline_y,
            },
            to: Point {
                x: plot.right(),
                y: baseline_y,
            },
            width: AXIS_WIDTH,
            color: AXIS,
        });
    }
    // Collect finite points, keeping index-based x so a skipped point widens the join.
    let n = values.len();
    let mut pts: Vec<Point> = Vec::new();
    for (i, &v) in values.iter().enumerate() {
        if v.is_finite() {
            pts.push(Point {
                x: map_x(i, n, plot),
                y: map_y(v, lo, hi, plot),
            });
        }
    }
    for seg in pts.windows(2) {
        prims.push(Primitive::Line {
            from: seg[0],
            to: seg[1],
            width: stroke_width,
            color,
        });
    }
    if markers {
        let half = MARKER_SIZE * 0.5;
        for p in &pts {
            prims.push(Primitive::SolidRect {
                rect: Rect::new(p.x - half, p.y - half, MARKER_SIZE, MARKER_SIZE),
                color,
                corner_radius: 0.0,
            });
        }
    }
    prims
}

/// Inserts a `Chart` leaf, writes its first-class a11y surface (SOUL §6.1 — role
/// `Chart`, name = title, value = [`summary`]), registers the fixed-size intrinsic
/// measure (SOUL §8.1), and returns the node id plus its **provisional** plot rect
/// (`node_rect`: origin `(0,0)` at mount, later slid onto the laid-out origin by
/// `reposition_paint`). Shared by all three chart leaves so the semantic + measure
/// contract is written in exactly one place.
fn scaffold(
    ctx: &mut BuildCtx,
    parent: Option<WidgetId>,
    size: Size,
    name: Option<String>,
    values: &[f32],
) -> (WidgetId, Rect) {
    let id = ctx.scene.insert(WidgetKind::Chart, parent);
    {
        // Semantic before visual (SOUL §6.1): a figure with a deterministic summary.
        let a = ctx.scene.a11y_mut(id);
        a.role = Role::Chart.as_u16();
        a.name = name;
        a.value = Some(summary(values));
    }
    // A chart is a fixed-size box; layout positions it, the chart never resizes itself.
    ctx.layout.set_measure(id, Box::new(move |_avail| size));
    let plot = node_rect(ctx.scene, id, size);
    (id, plot)
}

// ---------------------------------------------------------------------------
// BarChart
// ---------------------------------------------------------------------------

/// A single-series bar chart leaf (SOUL §8.1). Bars anchor to the zero baseline;
/// negative values hang below it. `.title()` sets **only** the accessible name (SOUL
/// §6.1) — it is *not* painted; compose a `Text` above the chart for a visible title.
pub struct BarChart {
    values: Vec<f32>,
    title: Option<Cow<'static, str>>,
    size: Size,
    color: Color,
    baseline: bool,
}

impl BarChart {
    /// A bar chart over `values` (default size 240×140, color `SERIES[0]`, baseline on).
    pub fn new(values: impl Into<Vec<f32>>) -> BarChart {
        BarChart {
            values: values.into(),
            title: None,
            size: DEFAULT_SIZE,
            color: SERIES[0],
            baseline: true,
        }
    }

    /// Sets the **accessible name** (SOUL §6.1). Not painted — compose a `Text` title
    /// above the chart for a visible one.
    pub fn title(mut self, title: impl Into<Cow<'static, str>>) -> BarChart {
        self.title = Some(title.into());
        self
    }

    /// Sets the chart's fixed size in logical px (default 240×140).
    pub fn size(mut self, w: f32, h: f32) -> BarChart {
        self.size = Size {
            width: w,
            height: h,
        };
        self
    }

    /// Sets the bar fill color (default `SERIES[0]`).
    pub fn color(mut self, color: Color) -> BarChart {
        self.color = color;
        self
    }

    /// Toggles the zero-line [`AXIS`] hairline (default `true`).
    pub fn baseline(mut self, baseline: bool) -> BarChart {
        self.baseline = baseline;
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
        let this = *self;
        let name = this.title.map(|c| c.into_owned());
        let (id, plot) = scaffold(ctx, parent, this.size, name, &this.values);
        let prims = bar_primitives(plot, &this.values, this.color, this.baseline);
        ctx.scene.replace_primitives(id, prims);
        id
    }
}

// ---------------------------------------------------------------------------
// LineChart
// ---------------------------------------------------------------------------

/// A single-series line chart leaf (SOUL §8.1): `n-1` [`Primitive::Line`] segments
/// joining consecutive points, an optional zero baseline, and optional per-point
/// markers. `.title()` sets **only** the accessible name (SOUL §6.1).
pub struct LineChart {
    values: Vec<f32>,
    title: Option<Cow<'static, str>>,
    size: Size,
    color: Color,
    stroke_width: f32,
    markers: bool,
    baseline: bool,
}

impl LineChart {
    /// A line chart over `values` (default size 240×140, color `SERIES[0]`, stroke 2.0,
    /// no markers, baseline on).
    pub fn new(values: impl Into<Vec<f32>>) -> LineChart {
        LineChart {
            values: values.into(),
            title: None,
            size: DEFAULT_SIZE,
            color: SERIES[0],
            stroke_width: LINE_STROKE,
            markers: false,
            baseline: true,
        }
    }

    /// Sets the **accessible name** (SOUL §6.1). Not painted.
    pub fn title(mut self, title: impl Into<Cow<'static, str>>) -> LineChart {
        self.title = Some(title.into());
        self
    }

    /// Sets the chart's fixed size in logical px (default 240×140).
    pub fn size(mut self, w: f32, h: f32) -> LineChart {
        self.size = Size {
            width: w,
            height: h,
        };
        self
    }

    /// Sets the line color (default `SERIES[0]`).
    pub fn color(mut self, color: Color) -> LineChart {
        self.color = color;
        self
    }

    /// Sets the line stroke width in logical px (default 2.0).
    pub fn stroke_width(mut self, width: f32) -> LineChart {
        self.stroke_width = width;
        self
    }

    /// Toggles 8×8 logical-px per-point square markers (default `false`).
    pub fn markers(mut self, markers: bool) -> LineChart {
        self.markers = markers;
        self
    }

    /// Toggles the zero-line [`AXIS`] hairline (default `true`).
    pub fn baseline(mut self, baseline: bool) -> LineChart {
        self.baseline = baseline;
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
        let this = *self;
        let name = this.title.map(|c| c.into_owned());
        let (id, plot) = scaffold(ctx, parent, this.size, name, &this.values);
        let prims = line_primitives(
            plot,
            &this.values,
            this.color,
            this.stroke_width,
            this.markers,
            this.baseline,
        );
        ctx.scene.replace_primitives(id, prims);
        id
    }
}

// ---------------------------------------------------------------------------
// Sparkline
// ---------------------------------------------------------------------------

/// A minimal single-series sparkline leaf (SOUL §8.1): line segments only — no
/// baseline, no markers, stroke 1.5, default size 80×20. Still carries the first-class
/// a11y summary (SOUL §6.1) like every chart.
pub struct Sparkline {
    values: Vec<f32>,
    size: Size,
    color: Color,
}

impl Sparkline {
    /// A sparkline over `values` (default size 80×20, color `SERIES[0]`).
    pub fn new(values: impl Into<Vec<f32>>) -> Sparkline {
        Sparkline {
            values: values.into(),
            size: SPARK_SIZE,
            color: SERIES[0],
        }
    }

    /// Sets the line color (default `SERIES[0]`).
    pub fn color(mut self, color: Color) -> Sparkline {
        self.color = color;
        self
    }

    /// Sets the sparkline's fixed size in logical px (default 80×20).
    pub fn size(mut self, w: f32, h: f32) -> Sparkline {
        self.size = Size {
            width: w,
            height: h,
        };
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
        let this = *self;
        // No title builder — a sparkline is nameless (minimal), value = summary only.
        let (id, plot) = scaffold(ctx, parent, this.size, None, &this.values);
        // Reuse the line path with baseline + markers off (SOUL §8.1): segments only.
        let prims = line_primitives(plot, &this.values, this.color, SPARK_STROKE, false, false);
        ctx.scene.replace_primitives(id, prims);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui_layout::LayoutEngine;
    use schnellui_scene::Scene;
    use schnellui_text::{GlyphAtlas, TextShaper};

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
                context: schnellui_widgets::Context::new(),
                runtime: schnellui_widgets::Runtime::new(),
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
        assert_eq!(SERIES[7], Color::rgb(0xeb, 0x68, 0x34)); // orange
        assert_eq!(AXIS, Color::rgb(0xc3, 0xc2, 0xb7));
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
