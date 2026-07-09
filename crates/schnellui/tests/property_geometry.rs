use schnellui::a11y::{self, Role};
use schnellui::scene::{CompositeProperties, LayoutBox, Point, Rect, WidgetKind};
use schnellui::widgets::{self, CursorIcon};
use schnellui::App;

#[test]
fn translated_clipped_leaf_agrees_with_pointer_cursor_and_accessibility() {
    let mut app = App::new(300, 200);
    let root = app.scene_mut().insert(WidgetKind::Column, None);
    app.scene_mut().set_root(root);
    let branch = app.scene_mut().insert(WidgetKind::Column, Some(root));
    let button = app.scene_mut().insert(WidgetKind::Button, Some(branch));
    for (id, rect) in [
        (root, Rect::new(0.0, 0.0, 300.0, 200.0)),
        (branch, Rect::new(0.0, 0.0, 100.0, 100.0)),
        (button, Rect::new(10.0, 10.0, 40.0, 30.0)),
    ] {
        app.scene_mut().set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
    }
    let a = app.scene_mut().a11y_mut(button);
    a.role = Role::Button.as_u16();
    a.actions = a11y::ActionFlags::CLICK.0;
    app.scene_mut().set_composite_properties(
        branch,
        CompositeProperties {
            translation: Point { x: 120.0, y: 30.0 },
            clip: Some(Rect::new(0.0, 0.0, 35.0, 80.0)),
        },
    );
    assert_eq!(app.hit_test(Point { x: 140.0, y: 50.0 }), Some(button));
    assert_eq!(
        app.hit_test(Point { x: 160.0, y: 50.0 }),
        None,
        "property clip excludes the right side"
    );
    assert_eq!(
        app.hit_test(Point { x: 20.0, y: 20.0 }),
        None,
        "old layout coordinates must miss"
    );
    let runtime = widgets::Runtime::new();
    assert_eq!(
        widgets::cursor_at(&runtime, app.scene(), Point { x: 140.0, y: 50.0 }),
        CursorIcon::Pointer
    );
    let update = a11y::build_full_tree_update(app.scene());
    let node = update
        .nodes
        .iter()
        .find(|(id, _)| *id == a11y::to_access_id(button))
        .unwrap();
    let bounds = node.1.bounds().unwrap();
    assert_eq!(
        (bounds.x0, bounds.y0, bounds.x1, bounds.y1),
        (130.0, 40.0, 155.0, 70.0)
    );
}

#[test]
fn wheel_target_uses_translated_viewport_and_inherited_clip() {
    let mut scene = schnellui::scene::Scene::new();
    let root = scene.insert(WidgetKind::Column, None);
    scene.set_root(root);
    let viewport = scene.insert(WidgetKind::Scroll, Some(root));
    scene.set_layout(
        root,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, 300.0, 200.0),
            content: Rect::ZERO,
        },
    );
    scene.set_layout(
        viewport,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, 80.0, 60.0),
            content: Rect::ZERO,
        },
    );
    scene.set_composite_properties(
        root,
        CompositeProperties {
            translation: Point { x: 100.0, y: 20.0 },
            clip: Some(Rect::new(0.0, 0.0, 40.0, 100.0)),
        },
    );
    assert_eq!(
        widgets::hit_test_scroll(&scene, Point { x: 120.0, y: 40.0 }),
        Some(viewport)
    );
    assert_eq!(
        widgets::hit_test_scroll(&scene, Point { x: 160.0, y: 40.0 }),
        None
    );
    assert_eq!(
        widgets::hit_test_scroll(&scene, Point { x: 20.0, y: 20.0 }),
        None
    );
}

#[test]
fn scrolling_moves_accessibility_bounds_through_the_viewport_transform() {
    let mut scene = schnellui::scene::Scene::new();
    let root = scene.insert(WidgetKind::Column, None);
    scene.set_root(root);
    let viewport = scene.insert(WidgetKind::Scroll, Some(root));
    let rows: Vec<_> = (0..200)
        .map(|_| scene.insert(WidgetKind::Button, Some(viewport)))
        .collect();
    let place = |scene: &mut schnellui::scene::Scene, id, rect| {
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        )
    };
    place(&mut scene, root, Rect::new(0.0, 0.0, 300.0, 200.0));
    place(&mut scene, viewport, Rect::new(10.0, 20.0, 200.0, 100.0));
    for (i, &row) in rows.iter().enumerate() {
        place(
            &mut scene,
            row,
            Rect::new(10.0, 20.0 + 30.0 * i as f32, 200.0, 30.0),
        );
        let a = scene.a11y_mut(row);
        a.role = Role::Button.as_u16();
    }
    scene.clear_dirty();

    scene.set_scroll_offset(viewport, Point { x: 0.0, y: 50.0 });
    assert_eq!(
        scene.a11y_dirty(),
        &[viewport],
        "a scroll step re-publishes only the viewport, not its 200 rows"
    );

    let update = a11y::build_full_tree_update(&scene);
    let node = |id| {
        update
            .nodes
            .iter()
            .find(|(n, _)| *n == a11y::to_access_id(id))
            .map(|(_, node)| node.clone())
            .unwrap()
    };
    let view = node(viewport);
    let transform = *view
        .transform()
        .expect("scrolled viewport carries a transform");
    assert!(view.clips_children());
    // The viewport's own frame stays where layout put it.
    let frame = transform.transform_rect_bbox(view.bounds().unwrap());
    assert_eq!(
        (frame.x0, frame.y0, frame.x1, frame.y1),
        (10.0, 20.0, 210.0, 120.0)
    );
    // Row 2 sits at y = 80 unscrolled and is drawn 50 px higher.
    let row = transform.transform_rect_bbox(node(rows[2]).bounds().unwrap());
    let drawn = scene.composited_rect(rows[2]);
    assert_eq!((row.x0, row.y0), (drawn.x as f64, drawn.y as f64));
    assert_eq!(row.y0, 30.0);
}
