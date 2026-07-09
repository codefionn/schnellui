//! Runtime hygiene: decorations, dense indexes and caches stay proportional to
//! what changed.

use super::base::build_one;
use super::*;

fn laid_out(runtime: &crate::widgets::Runtime, view: impl View) -> (Scene, WidgetId) {
    let (mut scene, mut layout, _text, _atlas, root) = build_one(runtime, view);
    layout.sync_tree(&scene, root);
    layout.compute(
        &mut scene,
        root,
        Size {
            width: 300.0,
            height: 300.0,
        },
    );
    reposition_paint(runtime, &mut scene);
    scene.clear_dirty();
    (scene, root)
}

fn hover(runtime: &crate::widgets::Runtime, scene: &mut Scene, id: WidgetId) -> bool {
    let point = center(scene, id);
    update_pointer_proximity(runtime, scene, point)
}

fn center(scene: &Scene, id: WidgetId) -> Point {
    let rect = scene.layout(id).unwrap().rect;
    Point {
        x: rect.x + rect.width * 0.5,
        y: rect.y + rect.height * 0.5,
    }
}

/// Moving the pointer between two buttons repaints those two and leaves a third
/// button's focus ring alone (it is neither the old nor the new hover target).
#[test]
fn hover_move_touches_only_the_two_hovered_controls() {
    let runtime_handle = crate::widgets::Runtime::new();
    let runtime = &runtime_handle;
    set_theme(runtime, Theme::default());
    let (mut scene, root) = laid_out(
        runtime,
        Column::new()
            .child(Button::new("a"))
            .child(Button::new("b"))
            .child(Button::new("c")),
    );
    let [a, b, c] = [0, 1, 2].map(|i| scene.node(root).unwrap().children[i]);
    let mut text = TextShaper::new();
    let mut atlas = GlyphAtlas::new(256, 256);
    dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(c));
    assert!(focus_ring_visible(runtime, c));
    let ring_prims = scene.paint(c).unwrap().primitives.clone();
    let base_a = scene.paint(a).unwrap().primitives.len();
    scene.clear_dirty();

    assert!(hover(runtime, &mut scene, a));
    let hovered_a = scene.paint(a).unwrap().primitives.len();
    assert!(hovered_a > base_a, "hover decorates the button");
    scene.clear_dirty();

    assert!(hover(runtime, &mut scene, b));
    let mut dirty = scene.paint_dirty().to_vec();
    dirty.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(
        dirty, expected,
        "only the old and new hover targets repaint"
    );
    assert_eq!(scene.paint(a).unwrap().primitives.len(), base_a);
    assert!(scene.paint(b).unwrap().primitives.len() > base_a - 1);
    assert_eq!(
        scene.paint(c).unwrap().primitives,
        ring_prims,
        "the unrelated focus ring is untouched"
    );
    assert!(focus_ring_visible(runtime, c));
}

/// A hover decoration sharing a node with the focus ring keeps the ring on top
/// and strips back to the exact original paint.
#[test]
fn hover_over_the_focused_control_restores_paint_exactly() {
    let runtime_handle = crate::widgets::Runtime::new();
    let runtime = &runtime_handle;
    set_theme(runtime, Theme::default());
    let (mut scene, root) = laid_out(
        runtime,
        Column::new()
            .child(Button::new("a"))
            .child(Button::new("b")),
    );
    let [a, b] = [0, 1].map(|i| scene.node(root).unwrap().children[i]);
    let mut text = TextShaper::new();
    let mut atlas = GlyphAtlas::new(256, 256);
    let plain = scene.paint(a).unwrap().primitives.clone();
    dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(a));
    let focused = scene.paint(a).unwrap().primitives.clone();

    hover(runtime, &mut scene, a);
    let tail = &scene.paint(a).unwrap().primitives;
    assert!(
        tail[tail.len() - FOCUS_RING_PRIMS..]
            .iter()
            .all(|p| matches!(p, Primitive::Line { width, .. } if *width == FOCUS_RING_W)),
        "the ring stays the outermost decoration"
    );
    hover(runtime, &mut scene, b);
    assert_eq!(scene.paint(a).unwrap().primitives, focused);
    dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, None);
    update_pointer_proximity(runtime, &mut scene, Point { x: 290.0, y: 290.0 });
    assert_eq!(scene.paint(a).unwrap().primitives, plain);
}

/// Showing a tooltip repaints the tooltip's node, not the whole scene root.
#[test]
fn showing_a_tooltip_marks_only_the_tooltip_node_paint_dirty() {
    let runtime_handle = crate::widgets::Runtime::new();
    let runtime = &runtime_handle;
    set_theme(runtime, Theme::default());
    let (mut scene, root) = laid_out(
        runtime,
        Column::new().child(Button::new("other")).child(
            Button::new("Settings")
                .icon_only()
                .tooltip("Settings")
                .width(22.0)
                .height(22.0),
        ),
    );
    let button = scene.node(root).unwrap().children[1];
    assert!(runtime.with(|rt| rt.borrow().hover_tooltips.contains_key(button)));

    assert!(hover(runtime, &mut scene, button));
    assert_eq!(scene.paint_dirty(), &[button]);
    scene.clear_dirty();
    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        Point { x: 290.0, y: 290.0 }
    ));
    assert_eq!(scene.paint_dirty(), &[button]);
}

#[test]
fn dense_indexes_follow_build_and_purge() {
    let runtime_handle = crate::widgets::Runtime::new();
    let runtime = &runtime_handle;
    let (mut scene, root) = laid_out(
        runtime,
        Column::new()
            .child(LoadingSpinner::new())
            .child(Button::new("t").icon_only().tooltip("t"))
            .child(TextInput::new("").placeholder("name"))
            .child(TerminalGrid::new(TerminalGridModel::new(4, 2))),
    );
    let grid_ids = |runtime: &crate::widgets::Runtime| runtime.terminal_grid.dense_len();
    assert_eq!(grid_ids(runtime), 1, "one dense id per built grid");
    let count = |f: fn(&WidgetRuntime) -> usize| runtime.with(|rt| f(&rt.borrow()));
    assert_eq!(count(|rt| rt.spinner_ids.len()), 1);
    assert_eq!(count(|rt| rt.tooltip_ids.len()), 1);
    assert_eq!(count(|rt| rt.edit_ids.len()), 1);
    assert!(has_loading_spinners(runtime, &scene));

    let nodes = scene.subtree_nodes(root);
    purge_nodes(runtime, &mut scene, &nodes);
    assert_eq!(count(|rt| rt.spinner_ids.len()), 0);
    assert_eq!(count(|rt| rt.tooltip_ids.len()), 0);
    assert_eq!(count(|rt| rt.edit_ids.len()), 0);
    assert_eq!(grid_ids(runtime), 0, "purge drops the grid's dense id");
    assert!(!has_floating_label_animations(runtime));
}

#[test]
fn purging_a_single_node_without_terminal_grids_is_a_no_op() {
    let runtime = crate::widgets::Runtime::new();
    let (mut scene, root) = laid_out(&runtime, Column::new().child(LoadingSpinner::new()));
    assert_eq!(runtime.terminal_grid.dense_len(), 0);
    purge_nodes(&runtime, &mut scene, &[root]);
    assert_eq!(runtime.terminal_grid.dense_len(), 0);
}

#[test]
fn measure_cache_overwrites_the_oldest_width_in_place() {
    let mut cache = MeasureCache::default();
    let size = |w: f32| Size {
        width: w,
        height: 1.0,
    };
    for w in 0..MEASURE_CACHE_LEN {
        cache.insert(w as f32, size(w as f32));
    }
    assert_eq!(cache.get(0.0), Some(size(0.0)));
    cache.insert(100.0, size(100.0));
    assert_eq!(cache.get(0.0), None, "the oldest width is overwritten");
    assert_eq!(cache.get(1.0), Some(size(1.0)));
    assert_eq!(cache.get(100.0), Some(size(100.0)));
    cache.clear();
    assert_eq!(cache.get(100.0), None);
}
