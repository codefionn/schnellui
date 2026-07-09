use super::base::build_one;
use super::*;

fn build_and_layout(
    runtime: &crate::Runtime,
    view: impl View,
    avail: Size,
) -> (Scene, LayoutEngine, TextShaper, GlyphAtlas, WidgetId) {
    let (mut scene, mut layout, text, atlas, id) = build_one(runtime, view);
    layout.sync_tree(&scene, id);
    layout.compute(&mut scene, id, avail);
    reposition_paint(runtime, &mut scene);
    (scene, layout, text, atlas, id)
}

/// A `w × h` scroll viewport wrapping a column of `n` labeled rows — content taller
/// than the box, so it is scrollable.
fn scroll_with_rows(n: usize, w: f32, h: f32) -> Scroll {
    let mut col = Column::new().gap(2.0);
    for i in 0..n {
        col = col.child(Text::new(format!("Row {i}")));
    }
    Scroll::new().size(w, h).child(col)
}

#[test]
fn scroll_builds_scrollview_role_actions_and_zero_value() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (scene, _l, _t, _a, id) = build_one(runtime, Scroll::new().size(320.0, 220.0));
    let a = scene.a11y(id).expect("scroll a11y column");
    assert_eq!(Role::from_u16(a.role), Role::ScrollView);
    assert!(ActionFlags(a.actions).contains(ActionFlags::SCROLL_UP));
    assert!(ActionFlags(a.actions).contains(ActionFlags::SCROLL_DOWN));
    assert!(ActionFlags(a.actions).contains(ActionFlags::FOCUS));
    assert_eq!(a.value.as_deref(), Some("0"));
}

#[test]
fn scrollbar_is_opt_in_and_thumb_tracks_the_offset() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let (scene, _layout, _text, _atlas, plain) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    assert!(scene
        .paint(plain)
        .is_none_or(|paint| paint.primitives.is_empty()));

    reset(runtime);
    let (mut scene, _layout, _text, _atlas, scroll) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0).scrollbar(true),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    let thumb_before = match scene.paint(scroll).unwrap().primitives[1] {
        Primitive::SolidRect { rect, .. } => rect,
        _ => panic!("scrollbar thumb is a solid rect"),
    };
    assert!(dispatch_scroll(runtime, &mut scene, scroll, 60.0));
    let thumb_after = match scene.paint(scroll).unwrap().primitives[1] {
        Primitive::SolidRect { rect, .. } => rect,
        _ => panic!("scrollbar thumb is a solid rect"),
    };
    assert!(thumb_after.y > thumb_before.y);
}

#[test]
fn scroll_can_start_at_a_virtualized_content_offset() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let (scene, _layout, _text, _atlas, scroll) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0).initial_offset(60.0),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );

    assert_eq!(scene.scroll_offset(scroll).y, 60.0);
    assert_eq!(
        scene.a11y(scroll).and_then(|a11y| a11y.value.as_deref()),
        Some("60")
    );
}

#[test]
fn scrollbar_thumb_drag_and_edge_auto_scroll_use_the_shared_scroll_path() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let (mut scene, _layout, _text, _atlas, scroll) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0)
            .scrollbar(true)
            .edge_auto_scroll(true),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    let metrics = scroll_metrics(&scene, scroll).unwrap();
    let (track, thumb) = scrollbar_rects(metrics).unwrap();
    let press = Point {
        x: thumb.x + thumb.width * 0.5,
        y: thumb.y + thumb.height * 0.5,
    };
    assert!(begin_scrollbar_pointer(runtime, &mut scene, press));
    assert!(scrollbar_pointer_active(runtime));
    assert!(update_scrollbar_pointer(
        runtime,
        &mut scene,
        Point {
            x: press.x,
            y: track.bottom(),
        }
    ));
    assert_eq!(scene.scroll_offset(scroll).y, metrics.max_offset);
    assert!(end_scrollbar_pointer(runtime));

    assert!(dispatch_scroll(
        runtime,
        &mut scene,
        scroll,
        -metrics.max_offset
    ));
    let viewport = scroll_metrics(&scene, scroll).unwrap().viewport;
    assert!(update_edge_auto_scroll(
        runtime,
        &scene,
        Point {
            x: viewport.x + 8.0,
            y: viewport.bottom() - 1.0,
        },
        true,
    ));
    assert!(has_active_edge_auto_scroll(runtime, &scene));
    assert!(tick_edge_auto_scroll(runtime, &mut scene));
    assert_eq!(scene.scroll_offset(scroll).y, EDGE_AUTO_SCROLL_STEP);
}

#[test]
fn scroll_clamps_at_top() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    // At the top (offset 0), scrolling up has nothing to reveal → no-op.
    assert!(!dispatch_scroll(runtime, &mut scene, id, -100.0));
    assert_eq!(scene.scroll_offset(id).y, 0.0);
}

#[test]
fn scroll_moves_offset_and_updates_a11y_value() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    assert_eq!(scene.node(id).unwrap().kind, WidgetKind::Scroll);
    assert!(dispatch_scroll(runtime, &mut scene, id, 30.0));
    assert_eq!(scene.scroll_offset(id).y, 30.0);
    // the accessible value tracks the rounded offset (SOUL §6.2)
    assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("30"));
}

#[test]
fn scroll_rewrites_the_preallocated_a11y_value_in_place() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    let (capacity, pointer) = {
        let value = scene.a11y(id).unwrap().value.as_ref().unwrap();
        (value.capacity(), value.as_ptr())
    };
    assert!(
        capacity >= 20,
        "scroll reserves enough room for any i64 offset"
    );

    assert!(dispatch_scroll(runtime, &mut scene, id, 48.0));
    let value = scene.a11y(id).unwrap().value.as_ref().unwrap();
    assert_eq!(value, "48");
    assert_eq!(
        value.as_ptr(),
        pointer,
        "wheel updates retain the String buffer"
    );
    assert_eq!(value.capacity(), capacity);
}

#[test]
fn fractional_scroll_with_the_same_rounded_a11y_value_stays_a11y_clean() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    scene.clear_dirty();
    let (capacity, pointer) = {
        let value = scene.a11y(id).unwrap().value.as_ref().unwrap();
        (value.capacity(), value.as_ptr())
    };

    assert!(dispatch_scroll(runtime, &mut scene, id, 0.25));
    let value = scene.a11y(id).unwrap().value.as_ref().unwrap();
    assert_eq!(value, "0");
    assert_eq!(value.as_ptr(), pointer);
    assert_eq!(value.capacity(), capacity);
    assert!(scene.a11y_dirty().is_empty());
    assert!(scene.layout_dirty().is_empty());
}

#[test]
fn scroll_clamps_at_bottom() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    // An overshoot clamps to max_offset (content_height − viewport_height).
    assert!(dispatch_scroll(runtime, &mut scene, id, 100_000.0));
    let max = scene.scroll_offset(id).y;
    assert!(max > 0.0, "content taller than the viewport is scrollable");
    assert_eq!(
        scene.a11y(id).unwrap().value.as_deref(),
        Some((max.round() as i64).to_string().as_str())
    );
    // Already at the bottom → another scroll down is a no-op.
    assert!(!dispatch_scroll(runtime, &mut scene, id, 100_000.0));
    assert_eq!(scene.scroll_offset(id).y, max);
}

#[test]
fn scroll_marks_paint_not_layout() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    scene.clear_dirty();
    assert!(dispatch_scroll(runtime, &mut scene, id, 24.0));
    let f = scene.dirty_flags(id);
    assert!(f.contains(DirtyFlags::PAINT));
    assert!(
        !f.contains(DirtyFlags::LAYOUT),
        "scroll re-composites, never relayouts (SOUL §3.2)"
    );
    // the offset is announced as a value, so A11Y is expected too (§6.2)
    assert!(f.contains(DirtyFlags::A11Y));
    assert!(scene.layout_dirty().is_empty());
}

#[test]
fn on_scroll_fires_with_clamped_value() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    use std::cell::RefCell;
    use std::rc::Rc;

    let seen: Rc<RefCell<Vec<f32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let view = {
        let mut col = Column::new().gap(2.0);
        for i in 0..25 {
            col = col.child(Text::new(format!("Row {i}")));
        }
        Scroll::new()
            .size(320.0, 220.0)
            .on_scroll(move |v| sink.borrow_mut().push(v))
            .child(col)
    };
    let (mut scene, _l, _t, _a, id) = build_and_layout(
        runtime,
        view,
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    // Overshoot: the handler sees the *clamped* offset, not the requested delta.
    assert!(dispatch_scroll(runtime, &mut scene, id, 100_000.0));
    let max = scene.scroll_offset(id).y;
    assert_eq!(seen.borrow().as_slice(), &[max]);
}

#[test]
fn debounced_scroll_resets_trailing_deadline_and_uses_latest_offset() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let (mut scene, _layout, _text, _atlas, scroll) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0).on_scroll_debounced(
            Duration::from_millis(100),
            Duration::from_secs(1),
            move |offset| sink.borrow_mut().push(offset),
        ),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    let start = std::time::Instant::now();
    assert_eq!(next_scroll_callback_deadline(runtime), None);
    assert!(!fire_due_scroll_callbacks(
        runtime,
        start + Duration::from_secs(60)
    ));
    assert!(seen.borrow().is_empty());
    assert!(dispatch_scroll_at(runtime, &mut scene, scroll, 20.0, start));
    assert_eq!(
        next_scroll_callback_deadline(runtime),
        Some(start + Duration::from_millis(100))
    );
    assert!(dispatch_scroll_at(
        runtime,
        &mut scene,
        scroll,
        30.0,
        start + Duration::from_millis(50)
    ));
    assert_eq!(
        next_scroll_callback_deadline(runtime),
        Some(start + Duration::from_millis(150))
    );
    assert!(!fire_due_scroll_callbacks(
        runtime,
        start + Duration::from_millis(149)
    ));
    assert!(fire_due_scroll_callbacks(
        runtime,
        start + Duration::from_millis(150)
    ));
    assert_eq!(*seen.borrow(), vec![50.0]);
    assert_eq!(next_scroll_callback_deadline(runtime), None);
}

#[test]
fn debounced_scroll_max_wait_bounds_continuous_gesture() {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let calls = Rc::new(Cell::new(0));
    let sink = calls.clone();
    let (mut scene, _layout, _text, _atlas, scroll) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0).on_scroll_debounced(
            Duration::from_millis(100),
            Duration::from_millis(250),
            move |_| sink.set(sink.get() + 1),
        ),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    let start = std::time::Instant::now();
    assert!(dispatch_scroll_at(runtime, &mut scene, scroll, 10.0, start));
    assert!(dispatch_scroll_at(
        runtime,
        &mut scene,
        scroll,
        10.0,
        start + Duration::from_millis(90)
    ));
    assert!(dispatch_scroll_at(
        runtime,
        &mut scene,
        scroll,
        10.0,
        start + Duration::from_millis(180)
    ));
    assert_eq!(
        next_scroll_callback_deadline(runtime),
        Some(start + Duration::from_millis(250))
    );
    assert!(fire_due_scroll_callbacks(
        runtime,
        start + Duration::from_millis(250)
    ));
    assert_eq!(calls.get(), 1);
}

#[test]
fn purged_scroll_cancels_pending_debounced_callback() {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let calls = Rc::new(Cell::new(0));
    let sink = calls.clone();
    let (mut scene, _layout, _text, _atlas, scroll) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0).on_scroll_debounced(
            Duration::from_millis(1),
            Duration::from_secs(1),
            move |_| sink.set(sink.get() + 1),
        ),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    let start = std::time::Instant::now();
    assert!(dispatch_scroll_at(runtime, &mut scene, scroll, 10.0, start));
    purge_nodes(runtime, &mut scene, &[scroll]);
    assert_eq!(next_scroll_callback_deadline(runtime), None);
    assert!(!fire_due_scroll_callbacks(
        runtime,
        start + Duration::from_secs(1)
    ));
    assert_eq!(calls.get(), 0);
}

#[test]
fn unchanged_scroll_returns_false_and_stays_clean() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_and_layout(
        runtime,
        scroll_with_rows(25, 320.0, 220.0),
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    scene.clear_dirty();
    // A zero delta at any offset changes nothing → false, no dirty channels.
    assert!(!dispatch_scroll(runtime, &mut scene, id, 0.0));
    assert!(scene.dirty_flags(id).is_empty());
    assert_eq!(scene.damage(), Rect::ZERO);
}

#[test]
fn hit_test_scroll_picks_deepest_nested_scroll() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let mut scene = Scene::new();
    let outer = scene.insert(WidgetKind::Scroll, None);
    scene.set_root(outer);
    scene.set_layout(
        outer,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, 300.0, 300.0),
            content: Rect::ZERO,
        },
    );
    let inner = scene.insert(WidgetKind::Scroll, Some(outer));
    scene.set_layout(
        inner,
        LayoutBox {
            rect: Rect::new(50.0, 50.0, 100.0, 100.0),
            content: Rect::ZERO,
        },
    );
    // inside the inner viewport → the deeper scroll wins (SOUL §3.2 nested scroll)
    assert_eq!(
        hit_test_scroll(&scene, Point { x: 60.0, y: 60.0 }),
        Some(inner)
    );
    // inside the outer only → the outer scroll
    assert_eq!(
        hit_test_scroll(&scene, Point { x: 10.0, y: 10.0 }),
        Some(outer)
    );
    // outside both → miss
    assert_eq!(hit_test_scroll(&scene, Point { x: 400.0, y: 400.0 }), None);
}

#[test]
fn hit_test_scroll_prunes_descendants_clipped_outside_their_viewport() {
    let mut scene = Scene::new();
    let outer = scene.insert(WidgetKind::Scroll, None);
    scene.set_root(outer);
    scene.set_layout(
        outer,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            content: Rect::ZERO,
        },
    );
    // A descendant may retain geometry below its scrolled/clipped ancestor, but a
    // wheel outside the ancestor's viewport must never reach it.
    let inner = scene.insert(WidgetKind::Scroll, Some(outer));
    scene.set_layout(
        inner,
        LayoutBox {
            rect: Rect::new(0.0, 120.0, 100.0, 100.0),
            content: Rect::ZERO,
        },
    );

    assert_eq!(hit_test_scroll(&scene, Point { x: 20.0, y: 140.0 }), None);
}

#[test]
fn runtime_scroll_hit_test_uses_viewport_index_and_respects_visibility() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let nested_rows = Column::new()
        .child(Text::new("nested row 1"))
        .child(Text::new("nested row 2"));
    let nested = Scroll::new()
        .label("inner")
        .size(100.0, 100.0)
        .child(nested_rows);
    let (mut scene, _layout, _text, _atlas, outer) = build_and_layout(
        runtime,
        Scroll::new()
            .label("outer")
            .size(300.0, 300.0)
            .child(nested),
        Size {
            width: 400.0,
            height: 400.0,
        },
    );
    let inner = scene.node(outer).unwrap().children[0];
    let point = Point { x: 10.0, y: 10.0 };

    assert_eq!(hit_test_scroll_in(runtime, &scene, point), Some(inner));
    // The inner viewport is laid out at y=80 but visually shifts to y=40 once
    // its outer scroll offsets by 40. Indexed routing must use that composed rect.
    let inner_layout = scene.layout(inner).copied().unwrap();
    scene.set_layout(
        inner,
        LayoutBox {
            rect: Rect::new(0.0, 80.0, inner_layout.rect.width, inner_layout.rect.height),
            content: inner_layout.content,
        },
    );
    scene.set_scroll_offset(outer, Point { x: 0.0, y: 40.0 });
    assert_eq!(
        hit_test_scroll_in(runtime, &scene, Point { x: 10.0, y: 40.0 }),
        Some(inner)
    );
    scene.set_visible(inner, false);
    assert_eq!(
        hit_test_scroll_in(runtime, &scene, Point { x: 10.0, y: 40.0 }),
        Some(outer)
    );
    scene.set_visible(outer, false);
    assert_eq!(hit_test_scroll_in(runtime, &scene, point), None);
}

#[test]
fn wheel_bubbles_to_scroll_ancestor_when_nested_viewport_is_at_an_edge() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let nested = scroll_with_rows(25, 100.0, 80.0);
    let (mut scene, _layout, _text, _atlas, outer) = build_and_layout(
        runtime,
        Scroll::new().size(240.0, 180.0).child(
            Column::new()
                .child(Column::new().height(100.0))
                .child(nested)
                .child(Column::new().height(600.0)),
        ),
        Size {
            width: 280.0,
            height: 220.0,
        },
    );
    let content = scene.node(outer).unwrap().children[0];
    let inner = scene.node(content).unwrap().children[1];
    let rect = scene.layout(inner).unwrap().rect;
    let point = Point {
        x: rect.x + 4.0,
        y: rect.y + 4.0,
    };

    assert!(dispatch_wheel_at(runtime, &mut scene, point, 48.0));
    assert_eq!(scene.scroll_offset(inner).y, 48.0);
    assert_eq!(scene.scroll_offset(outer).y, 0.0);

    assert!(dispatch_scroll(runtime, &mut scene, inner, f32::MAX));
    assert!(dispatch_wheel_at(runtime, &mut scene, point, 48.0));
    assert_eq!(scene.scroll_offset(outer).y, 48.0);

    assert!(dispatch_scroll(runtime, &mut scene, inner, -f32::MAX));
    let point_after_outer_scroll = Point {
        x: point.x,
        y: point.y - 48.0,
    };
    assert!(dispatch_wheel_at(
        runtime,
        &mut scene,
        point_after_outer_scroll,
        -48.0,
    ));
    assert_eq!(scene.scroll_offset(inner).y, 0.0);
    assert_eq!(scene.scroll_offset(outer).y, 0.0);
}

#[test]
fn runtime_scroll_hit_test_excludes_background_behind_a_modal() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let background_rows = Column::new()
        .child(Text::new("background row"))
        .child(Text::new("background row 2"));
    let modal_rows = Column::new()
        .child(Text::new("modal row"))
        .child(Text::new("modal row 2"));
    let (scene, _layout, _text, _atlas, root) = build_and_layout(
        runtime,
        Column::new()
            .child(
                Scroll::new()
                    .label("background scroll")
                    .size(300.0, 300.0)
                    .child(background_rows),
            )
            .child(
                Dialog::new("Modal").size(240.0, 180.0).child(
                    Scroll::new()
                        .label("modal scroll")
                        .size(180.0, 80.0)
                        .child(modal_rows),
                ),
            ),
        Size {
            width: 600.0,
            height: 500.0,
        },
    );
    let modal_scroll = scene
        .subtree_nodes(root)
        .into_iter()
        .find(|id| scene.a11y(*id).and_then(|a| a.name.as_deref()) == Some("modal scroll"))
        .expect("modal scroll retained");
    let rect = scene.layout(modal_scroll).unwrap().rect;
    let point = Point {
        x: rect.x + 1.0,
        y: rect.y + 1.0,
    };

    assert_eq!(
        hit_test_scroll_in(runtime, &scene, point),
        Some(modal_scroll)
    );
}

#[test]
fn dispatch_scroll_on_non_scroll_is_false() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_one(runtime, Button::new("x"));
    scene.set_layout(
        id,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, 100.0, 40.0),
            content: Rect::ZERO,
        },
    );
    assert!(!dispatch_scroll(runtime, &mut scene, id, 20.0));
}

// --- keyboard control (SOUL §6.3 — standard browser semantics) ---

/// Arrows step a slider by 1% of its range, PageUp/PageDown by 10, Home/End
/// pin to the boundaries — clamped, no-op at the ends, `on_change` fired with
/// each real move and the accessible value kept in sync (SOUL §6.3).
#[test]
fn dispatch_adjust_steps_clamps_and_notifies() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    use std::cell::RefCell;
    use std::rc::Rc;

    let seen: Rc<RefCell<Vec<f32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let (mut scene, _l, _t, _a, id) = build_one(
        runtime,
        Slider::new(50.0, 0.0, 100.0).on_change(move |v| sink.borrow_mut().push(v)),
    );

    assert!(dispatch_adjust(runtime, &mut scene, id, Adjust::Steps(1)));
    assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("51"));
    assert!(dispatch_adjust(runtime, &mut scene, id, Adjust::Steps(-10)));
    assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("41"));
    assert!(dispatch_adjust(runtime, &mut scene, id, Adjust::ToMax));
    assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("100"));
    // already at the end → no-op, nothing fired
    assert!(!dispatch_adjust(runtime, &mut scene, id, Adjust::Steps(5)));
    assert!(!dispatch_adjust(runtime, &mut scene, id, Adjust::ToMax));
    assert!(dispatch_adjust(runtime, &mut scene, id, Adjust::ToMin));
    assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("0"));
    assert_eq!(*seen.borrow(), vec![51.0, 41.0, 100.0, 0.0]);
    // a non-slider target is inert
    let (mut scene, _l, _t, _a, btn) = build_one(runtime, Button::new("x"));
    assert!(!dispatch_adjust(runtime, &mut scene, btn, Adjust::Steps(1)));
}

#[test]
fn slider_respects_explicit_step_pointer_scrubbing_and_disabled_state() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) =
        build_one(runtime, Slider::new(0.0, 0.0, 10.0).step(2.0).name("Zoom"));
    scene.set_layout(
        id,
        LayoutBox {
            rect: Rect::new(10.0, 20.0, 120.0, 20.0),
            content: Rect::ZERO,
        },
    );
    assert!(dispatch_adjust(runtime, &mut scene, id, Adjust::Steps(1)));
    assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("2"));
    assert!(dispatch_slider_pointer(
        runtime,
        &mut scene,
        id,
        Point { x: 82.0, y: 30.0 }
    ));
    assert_eq!(
        scene.a11y(id).unwrap().value.as_deref(),
        Some("6"),
        "60% pointer position snaps to the 2-unit step"
    );
    assert_eq!(scene.a11y(id).unwrap().name.as_deref(), Some("Zoom"));
    assert_eq!(scene.paint(id).unwrap().primitives.len(), 3);

    let (mut disabled, _l, _t, _a, disabled_id) =
        build_one(runtime, Slider::new(5.0, 0.0, 10.0).disabled(true));
    let a = disabled.a11y(disabled_id).unwrap();
    assert!(StateFlags(a.state).contains(StateFlags::DISABLED));
    assert_eq!(a.actions, 0);
    assert!(!dispatch_adjust(
        runtime,
        &mut disabled,
        disabled_id,
        Adjust::Steps(1)
    ));
}

#[test]
fn loading_spinner_ticks_only_when_animated() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_one(runtime, LoadingSpinner::new().phase(3));
    let before = scene.paint(id).unwrap().primitives[1];
    assert!(has_loading_spinners(runtime, &scene));
    assert!(tick_loading_spinners(runtime, &mut scene));
    let after = scene.paint(id).unwrap().primitives[1];
    assert_ne!(before, after);
    assert!(scene.dirty_flags(id).contains(DirtyFlags::PAINT));
    scene.set_visible(id, false);
    assert!(!has_loading_spinners(runtime, &scene));
    assert!(!tick_loading_spinners(runtime, &mut scene));

    let (mut still, _l, _t, _a, still_id) =
        build_one(runtime, LoadingSpinner::new().animated(false).phase(3));
    let before = still.paint(still_id).unwrap().primitives[1];
    assert!(!has_loading_spinners(runtime, &still));
    assert!(!tick_loading_spinners(runtime, &mut still));
    assert_eq!(still.paint(still_id).unwrap().primitives[1], before);
}

/// The browser activation matrix (SOUL §6.3): button ← Enter and Space,
/// link ← Enter only, checkbox ← Space only; a label takes neither.
#[test]
fn key_activation_follows_the_browser_matrix() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    use std::cell::Cell;
    use std::rc::Rc;

    let count = Rc::new(Cell::new(0));
    let c2 = count.clone();
    let (mut scene, _l, _t, _a, id) = build_one(
        runtime,
        Button::new("go").on_click(move || c2.set(c2.get() + 1)),
    );
    assert_eq!(
        dispatch_key_activate(runtime, &mut scene, id, ActivateKey::Enter),
        Some(true)
    );
    assert_eq!(
        dispatch_key_activate(runtime, &mut scene, id, ActivateKey::Space),
        Some(true)
    );
    assert_eq!(count.get(), 2);

    let (mut scene, _l, _t, _a, link) = build_one(runtime, Link::new("docs").on_click(|| {}));
    assert_eq!(
        dispatch_key_activate(runtime, &mut scene, link, ActivateKey::Enter),
        Some(true)
    );
    // Space on a link is NOT consumed — it falls through to page scroll.
    assert_eq!(
        dispatch_key_activate(runtime, &mut scene, link, ActivateKey::Space),
        None
    );

    let (mut scene, _l, _t, _a, cb) = build_one(runtime, Checkbox::new(false));
    assert_eq!(
        dispatch_key_activate(runtime, &mut scene, cb, ActivateKey::Enter),
        None
    );
    assert_eq!(
        dispatch_key_activate(runtime, &mut scene, cb, ActivateKey::Space),
        Some(true)
    );
    assert!(StateFlags(scene.a11y(cb).unwrap().state).contains(StateFlags::CHECKED));

    let (mut scene, _l, _t, _a, label) = build_one(runtime, Text::new("plain"));
    assert_eq!(
        dispatch_key_activate(runtime, &mut scene, label, ActivateKey::Enter),
        None
    );
    assert_eq!(
        dispatch_key_activate(runtime, &mut scene, label, ActivateKey::Space),
        None
    );
}

/// Keyboard scrolling needs a viewport to aim at (SOUL §6.3): the enclosing
/// scroll of a focused widget, else the first viewport in tree order.
#[test]
fn scroll_targets_resolve_enclosing_then_first() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (scene, _l, _t, _a, root) = build_one(
        runtime,
        Column::new().child(Button::new("outside")).child(
            Scroll::new()
                .size(100.0, 100.0)
                .child(Button::new("inside")),
        ),
    );
    let sv = first_scroll(&scene).expect("a scroll viewport exists");
    assert_eq!(scene.node(sv).unwrap().kind, WidgetKind::Scroll);
    // the button inside the viewport resolves its enclosing scroll
    let inside = scene.node(sv).unwrap().children[0];
    assert_eq!(enclosing_scroll(&scene, inside), Some(sv));
    // the root and the outside button enclose no viewport
    assert_eq!(enclosing_scroll(&scene, root), None);
    let outside = scene.node(root).unwrap().children[0];
    assert_eq!(enclosing_scroll(&scene, outside), None);
}
