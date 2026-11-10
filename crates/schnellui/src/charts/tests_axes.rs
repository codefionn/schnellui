//! Tests for the readable-chart features: palette contrast, axes, negative bars,
//! round inset markers, shared plot width and per-mark hover tooltips.

use schnellui::a11y::Role;
use schnellui::scene::{Color, Point, Primitive, Rect, Scene, WidgetId, WidgetKind};
use schnellui::widgets::{contrast_ratio, Column, View};
use schnellui::App;

use super::*;

const WHITE: Color = Color::WHITE;
const PAGE: Color = Color::rgb(0xe9, 0xec, 0xf1);

fn prims(scene: &Scene, id: WidgetId) -> Vec<Primitive> {
    scene.paint(id).unwrap().primitives.clone()
}

fn mount(view: impl View) -> App {
    let mut app = App::mount_with_size(view, 420, 320);
    app.frame();
    app
}

/// All nodes in pre-order.
fn walk(scene: &Scene, id: WidgetId, out: &mut Vec<WidgetId>) {
    out.push(id);
    for &c in &scene.node(id).unwrap().children {
        walk(scene, c, out);
    }
}

fn all_nodes(scene: &Scene) -> Vec<WidgetId> {
    let mut out = Vec::new();
    walk(scene, scene.root().unwrap(), &mut out);
    out
}

fn chart_leaf(scene: &Scene) -> WidgetId {
    *all_nodes(scene)
        .iter()
        .find(|&&id| scene.node(id).unwrap().kind == WidgetKind::Chart)
        .expect("a chart leaf")
}

/// Hover targets: the named `Group` nodes the chart adds per mark.
fn marks(scene: &Scene) -> Vec<(WidgetId, String)> {
    all_nodes(scene)
        .into_iter()
        .filter_map(|id| {
            let a = scene.a11y(id)?;
            (Role::from_u16(a.role) == Role::Group && a.name.is_some())
                .then(|| (id, a.name.clone().unwrap()))
        })
        .collect()
}

fn tooltip_visible(scene: &Scene, id: WidgetId) -> bool {
    prims(scene, id)
        .iter()
        .any(|p| matches!(p, Primitive::GlyphQuad { color, .. } if color.a != 0))
}

fn center(scene: &Scene, id: WidgetId) -> Point {
    let r = scene.layout(id).unwrap().rect;
    Point {
        x: r.x + r.width * 0.5,
        y: r.y + r.height * 0.5,
    }
}

// --- contrast (WCAG 1.4.11) ---

#[test]
fn every_series_color_has_3_to_1_contrast_on_white_and_the_page() {
    for (i, c) in SERIES.iter().enumerate() {
        assert!(contrast_ratio(*c, WHITE) >= 3.0, "SERIES[{i}] on white");
        assert!(contrast_ratio(*c, PAGE) >= 3.0, "SERIES[{i}] on the page");
    }
}

#[test]
fn baseline_color_has_3_to_1_contrast_and_is_the_theme_outline() {
    assert!(contrast_ratio(AXIS, WHITE) >= 3.0);
    assert!(contrast_ratio(AXIS, PAGE) >= 3.0);
    assert_eq!(AXIS, schnellui::widgets::DEFAULT_THEME.outline);
    const _: () = assert!(AXIS_WIDTH >= 1.5, "baseline is thicker than a hairline");
}

// --- bars ---

#[test]
fn negative_bars_have_their_own_color_and_the_zero_line_is_over_the_bars() {
    let app = mount(BarChart::new([3.0, -2.0, 5.0]));
    let p = prims(app.scene(), chart_leaf(app.scene()));
    let colors: Vec<Color> = p
        .iter()
        .filter_map(|p| match p {
            Primitive::SolidRect { color, .. } => Some(*color),
            _ => None,
        })
        .collect();
    assert_eq!(colors, vec![SERIES[0], SERIES[5], SERIES[0]]);
    let zero = p
        .iter()
        .position(|p| matches!(p, Primitive::Line { .. }))
        .unwrap();
    let last_bar = p
        .iter()
        .rposition(|p| matches!(p, Primitive::SolidRect { .. }))
        .unwrap();
    assert!(zero > last_bar, "zero line paints after (over) the bars");

    let app = mount(BarChart::new([3.0, -2.0]).negative_color(SERIES[2]));
    let p = prims(app.scene(), chart_leaf(app.scene()));
    assert!(p
        .iter()
        .any(|p| matches!(p, Primitive::SolidRect { color, .. } if *color == SERIES[2])));
}

#[test]
fn axes_add_gridlines_labels_and_an_anchor_without_moving_the_marks() {
    let app = mount(
        BarChart::new([3.0, 7.0, 2.0, 9.0])
            .axes(true)
            .labels(["a", "b", "c", "d"]),
    );
    let leaf = chart_leaf(app.scene());
    let p = prims(app.scene(), leaf);
    let layout = app.scene().layout(leaf).unwrap().rect;

    // anchor first: the slide lands on the box corner, so the bars did not move.
    assert!(matches!(
        p[0],
        Primitive::SolidRect { color, rect, .. } if color.a == 0 && rect == layout
    ));
    // ticks 0,5,10: the floor tick is the zero line (drawn over the bars), the others
    // are gridlines (2).
    let lines = p
        .iter()
        .filter(|p| matches!(p, Primitive::Line { .. }))
        .count();
    assert_eq!(lines, 3);
    // tick labels "0","5","10" + 4 category labels all paint glyphs, inside the box.
    let glyphs: Vec<Rect> = p
        .iter()
        .filter_map(|p| match p {
            Primitive::GlyphQuad { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert!(
        glyphs.len() >= 7,
        "labels paint glyphs, got {}",
        glyphs.len()
    );
    for g in &glyphs {
        assert!(
            g.x >= layout.x - 0.5 && g.right() <= layout.right() + 0.5,
            "{g:?}"
        );
        assert!(
            g.y >= layout.y - 0.5 && g.bottom() <= layout.bottom() + 0.5,
            "{g:?}"
        );
    }
    // the text is theme muted ink, never a mark color.
    assert!(p
        .iter()
        .all(|p| !matches!(p, Primitive::GlyphQuad { color, .. } if SERIES.contains(color))));
}

#[test]
fn axes_do_not_change_the_accessible_summary() {
    let app = mount(
        BarChart::new([3.0, 7.0, 2.0, 9.0, 4.0])
            .axes(true)
            .title("Sales"),
    );
    let a = app.scene().a11y(chart_leaf(app.scene())).unwrap();
    assert_eq!(a.name.as_deref(), Some("Sales"));
    assert_eq!(a.value.as_deref(), Some("n=5 min=2 max=9 last=4"));
}

#[test]
fn gutter_gives_charts_with_different_tick_labels_one_plot_width() {
    let plot_span = |values: Vec<f32>| {
        let app = mount(
            LineChart::new(values)
                .axes(true)
                .gutter(40.0)
                .baseline(false),
        );
        let leaf = chart_leaf(app.scene());
        let p = prims(app.scene(), leaf);
        let xs: Vec<(f32, f32)> = p
            .iter()
            .filter_map(|p| match p {
                Primitive::Line {
                    from, to, color, ..
                } if *color != SERIES[0] => Some((from.x, to.x)),
                _ => None,
            })
            .collect();
        assert!(!xs.is_empty());
        xs[0]
    };
    let narrow = plot_span(vec![1.0, 2.0, 3.0]);
    let wide = plot_span(vec![1000.0, 25000.0, 120000.0]);
    assert_eq!(narrow, wide, "same gutter -> same plot x-range");
}

// --- lines ---

#[test]
fn line_markers_are_round_and_the_plot_is_inset_by_their_radius() {
    let values = [4.0, 6.0, 5.0, 12.0];
    let app = mount(LineChart::new(values).markers(true).baseline(false));
    let leaf = chart_leaf(app.scene());
    let layout = app.scene().layout(leaf).unwrap().rect;
    let markers: Vec<(Rect, f32)> = prims(app.scene(), leaf)
        .iter()
        .filter_map(|p| match p {
            Primitive::SolidRect {
                rect,
                corner_radius,
                ..
            } => Some((*rect, *corner_radius)),
            _ => None,
        })
        .collect();
    assert_eq!(markers.len(), 4);
    for (r, radius) in &markers {
        assert!(
            (radius - MARKER_SIZE * 0.5).abs() < 1e-4,
            "round, not square"
        );
        assert!(r.x >= layout.x - 1e-3 && r.right() <= layout.right() + 1e-3);
        assert!(r.y >= layout.y - 1e-3 && r.bottom() <= layout.bottom() + 1e-3);
    }
    // the minimum (4.0) and maximum (12.0) touch the inset plot, not the box edge.
    assert!(
        (markers[0].0.x - layout.x).abs() < 1e-3,
        "first marker's edge, not its center, is on the box edge"
    );
    let lowest = markers.iter().map(|(m, _)| m.bottom()).fold(0.0, f32::max);
    assert!(
        (lowest - layout.bottom()).abs() < 1e-3,
        "lowest marker rests on the box bottom"
    );
}

#[test]
fn sparkline_end_dot_marks_the_latest_point() {
    let app = mount(
        Sparkline::new([1.0, 3.0, 2.0])
            .size(100.0, 30.0)
            .end_dot(true),
    );
    let p = prims(app.scene(), chart_leaf(app.scene()));
    let dots: Vec<_> = p
        .iter()
        .filter(|p| matches!(p, Primitive::SolidRect { corner_radius, .. } if *corner_radius > 0.0))
        .collect();
    assert_eq!(dots.len(), 1);
}

// --- tooltips ---

#[test]
fn tooltips_are_opt_in_and_keep_the_leaf_shape() {
    let app = mount(BarChart::new([1.0, 2.0]));
    assert!(marks(app.scene()).is_empty());
    assert_eq!(
        app.scene().node(app.scene().root().unwrap()).unwrap().kind,
        WidgetKind::Chart
    );
}

#[test]
fn each_bar_is_a_hover_target_with_its_own_label() {
    let mut app = mount(
        BarChart::new([3.0, -2.0, 5.0])
            .labels(["Mon", "Tue", "Wed"])
            .unit("ms")
            .tooltips(true)
            .axes(true),
    );
    let m = marks(app.scene());
    let names: Vec<&str> = m.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(names, ["Mon: 3 ms", "Tue: \u{2212}2 ms", "Wed: 5 ms"]);

    // The chart summary is still on the leaf inside the stack.
    let a = app.scene().a11y(chart_leaf(app.scene())).unwrap();
    assert_eq!(a.value.as_deref(), Some("n=3 min=-2 max=5 last=5"));

    for (id, _) in &m {
        assert!(!tooltip_visible(app.scene(), *id), "hidden until hovered");
    }
    // hovering one bar shows exactly that bar's tooltip.
    let (target, _) = m[1].clone();
    let p = center(app.scene(), target);
    assert!(app.update_pointer_proximity(p));
    for (id, _) in &m {
        assert_eq!(tooltip_visible(app.scene(), *id), *id == target);
    }
    // and leaving hides it.
    assert!(app.update_pointer_proximity(Point { x: 410.0, y: 310.0 }));
    assert!(!tooltip_visible(app.scene(), target));
}

#[test]
fn each_point_is_a_hover_target_and_skips_non_finite_values() {
    let app = mount(
        LineChart::new([1.0, f32::NAN, 3.0])
            .labels(["a", "b", "c"])
            .markers(true)
            .tooltips(true),
    );
    let names: Vec<String> = marks(app.scene()).into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["a: 1", "c: 3"]);
}

#[test]
fn hover_targets_sit_on_their_marks() {
    let app = mount(LineChart::new([1.0, 5.0, 3.0]).markers(true).tooltips(true));
    let leaf = chart_leaf(app.scene());
    let box_ = app.scene().layout(leaf).unwrap().rect;
    for (id, _) in marks(app.scene()) {
        let c = center(app.scene(), id);
        assert!(
            box_.contains(c),
            "target center {c:?} inside chart {box_:?}"
        );
    }
}

#[test]
fn a_tooltip_chart_composes_in_a_column() {
    let app = mount(
        Column::new()
            .child(BarChart::new([1.0, 2.0]).tooltips(true))
            .child(LineChart::new([1.0, 2.0]).tooltips(true)),
    );
    assert_eq!(marks(app.scene()).len(), 4);
}

#[test]
fn crowded_category_labels_thin_evenly_and_keep_the_latest() {
    // roomy labels all show
    assert_eq!(
        category_label_stride(&[(0.0, 10.0), (30.0, 10.0), (60.0, 10.0)]),
        1
    );
    // 4px apart: every other label, counted back from the last one
    let tight: Vec<(f32, f32)> = (0..12).map(|i| (i as f32 * 24.0, 20.0)).collect();
    assert_eq!(category_label_stride(&tight), 2);
    assert_eq!(category_label_stride(&[]), 1);
    assert_eq!(category_label_stride(&[(0.0, 50.0)]), 1);
}

/// Opaque non-glyph paint in a hover node beyond its transparent anchor: the
/// hovered-mark highlight.
fn highlight_count(scene: &Scene, id: WidgetId) -> usize {
    prims(scene, id)
        .iter()
        .filter(|p| matches!(p, Primitive::SolidRect { color, .. } if color.a != 0))
        .count()
}

#[test]
fn a_hovered_mark_is_highlighted_only_while_hovered() {
    for line in [false, true] {
        let mut app = if line {
            mount(LineChart::new([1.0, 4.0, 2.0]).markers(true).tooltips(true))
        } else {
            mount(BarChart::new([1.0, 4.0, 2.0]).tooltips(true))
        };
        let m = marks(app.scene());
        let (target, _) = m[1].clone();
        // the tooltip's own background is the one opaque rect it adds
        let base: Vec<usize> = m
            .iter()
            .map(|(id, _)| highlight_count(app.scene(), *id))
            .collect();
        assert!(
            base.iter().all(|&n| n == 0),
            "nothing shows before hover: {base:?}"
        );

        let p = center(app.scene(), target);
        assert!(app.update_pointer_proximity(p));
        let want = if line { 2 } else { 1 };
        // + 1 for the tooltip background
        assert_eq!(
            highlight_count(app.scene(), target),
            want + 1,
            "line={line}"
        );
        for (id, _) in &m {
            if *id != target {
                assert_eq!(highlight_count(app.scene(), *id), 0);
            }
        }
        assert!(app.update_pointer_proximity(Point { x: 410.0, y: 310.0 }));
        assert_eq!(highlight_count(app.scene(), target), 0);
    }
}
