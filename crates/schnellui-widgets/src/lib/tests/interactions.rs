use super::base::build_one;
use super::*;

fn ring_lines(scene: &Scene, id: WidgetId) -> usize {
    let prims = &scene.paint(id).unwrap().primitives;
    prims
            .iter()
            .rev()
            .take_while(
                |primitive| matches!(primitive, Primitive::Line { width, .. } if *width == FOCUS_RING_W),
            )
            .count()
}

/// Focus wears the same inset box as native HTML; blur removes it; a
/// state re-emit on the focused widget keeps it (SOUL §6.3).
#[test]
fn focus_ring_applies_survives_toggle_and_clears_on_blur() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _l, mut text, mut atlas, root) = build_one(
        runtime,
        Column::new()
            .child(Button::new("go"))
            .child(Checkbox::new(false)),
    );
    let btn = scene.node(root).unwrap().children[0];
    let cb = scene.node(root).unwrap().children[1];

    let before = scene.paint(btn).unwrap().primitives.len();
    assert!(dispatch_focus(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        Some(btn)
    ));
    assert_eq!(ring_lines(&scene, btn), FOCUS_RING_PRIMS);
    // focus moves: the old ring is stripped exactly, the new one applied
    assert!(dispatch_focus(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        Some(cb)
    ));
    assert_eq!(scene.paint(btn).unwrap().primitives.len(), before);
    assert_eq!(ring_lines(&scene, cb), FOCUS_RING_PRIMS);
    // a toggle re-emits the checkbox paint — the ring must survive it
    assert!(dispatch_click(runtime, &mut scene, cb));
    assert_eq!(ring_lines(&scene, cb), FOCUS_RING_PRIMS);
    // blur removes it
    assert!(dispatch_focus(
        runtime, &mut scene, &mut text, &mut atlas, None
    ));
    assert_eq!(ring_lines(&scene, cb), 0);
}

#[test]
fn focused_editable_gets_browser_outline_and_keeps_it_while_editing() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let line_count = |scene: &Scene, id: WidgetId| {
        scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter(|p| matches!(p, Primitive::Line { .. }))
            .count()
    };
    let (mut scene, _l, mut text, mut atlas, id) = build_one(runtime, TextInput::new("hi"));
    // Browser `:focus-visible` applies to pointer-focused controls that
    // accept text input, because the interaction immediately needs a caret.
    assert!(dispatch_pointer_focus(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        Some(id)
    ));
    assert_eq!(
        line_count(&scene, id),
        1 + FOCUS_RING_PRIMS,
        "caret plus browser-equivalent outline"
    );
    assert!(dispatch_edit_key(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        id,
        EditKey::Insert("!")
    ));
    assert_eq!(line_count(&scene, id), 1 + FOCUS_RING_PRIMS);
    assert!(dispatch_focus(
        runtime, &mut scene, &mut text, &mut atlas, None
    ));
    assert_eq!(line_count(&scene, id), 0);
}

#[test]
fn focus_outline_matches_css_width_offset_and_contrast_color() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _layout, mut text, mut atlas, id) = build_one(runtime, Button::new("Save"));
    let bounds = paint_bounds(runtime, &scene, id).unwrap();
    assert!(dispatch_focus(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        Some(id)
    ));
    let outline = &scene.paint(id).unwrap().primitives;
    let top = outline[outline.len() - FOCUS_RING_PRIMS];
    let Primitive::Line {
        from,
        to,
        width,
        color,
    } = top
    else {
        panic!("focus outline starts with a line");
    };
    assert_eq!(width, 3.0);
    assert_eq!(color, theme_for(runtime, id).focus_color());
    assert_eq!(from.y - width * 0.5, bounds.y);
    assert_eq!(from.x - width * 0.5, bounds.x);
    assert_eq!(to.x + width * 0.5, bounds.right());
}

#[test]
fn inset_focus_outline_stays_inside_retained_control_paint() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _layout, mut text, mut atlas, id) = build_one(runtime, Button::new("Save"));
    assert!(dispatch_focus(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        Some(id)
    ));
    let target = Rect::new(40.0, 30.0, 160.0, 50.0);
    scene.set_layout(
        id,
        LayoutBox {
            rect: target,
            content: target,
        },
    );

    reposition_paint(runtime, &mut scene);

    let ordinary = paint_bounds(runtime, &scene, id).unwrap();
    assert_eq!(ordinary.x, target.x);
    assert_eq!(ordinary.y, target.y);
    let outline = &scene.paint(id).unwrap().primitives;
    let Primitive::Line { from, width, .. } = outline[outline.len() - FOCUS_RING_PRIMS] else {
        panic!("focus outline starts with a line");
    };
    assert_eq!(from.y - width * 0.5, target.y);
}

#[test]
fn pointer_hover_decorates_enabled_controls_and_clears_on_leave_or_disable() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, mut layout, _text, _atlas, root) = build_one(
        runtime,
        Column::new()
            .child(Button::new("enabled"))
            .child(Button::new("disabled").disabled(true)),
    );
    layout.sync_tree(&scene, root);
    layout.compute(
        &mut scene,
        root,
        Size {
            width: 320.0,
            height: 180.0,
        },
    );
    reposition_paint(runtime, &mut scene);
    let enabled = scene.node(root).unwrap().children[0];
    let disabled = scene.node(root).unwrap().children[1];
    let center = |scene: &Scene, id| {
        let rect = scene.layout(id).unwrap().rect;
        Point {
            x: rect.x + rect.width * 0.5,
            y: rect.y + rect.height * 0.5,
        }
    };
    let base = scene.paint(enabled).unwrap().primitives.len();
    let enabled_center = center(&scene, enabled);
    let disabled_center = center(&scene, disabled);

    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        enabled_center
    ));
    assert_eq!(
        scene.paint(enabled).unwrap().primitives.len(),
        base + 1 + HOVER_BORDER_PRIMS
    );

    // Moving onto a disabled control removes the old feedback but never
    // applies hover styling to the inert target.
    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        disabled_center
    ));
    assert_eq!(scene.paint(enabled).unwrap().primitives.len(), base);
    assert!(runtime.with(|rt| rt.borrow().hover.is_none()));

    assert!(!update_pointer_proximity(
        runtime,
        &mut scene,
        Point {
            x: f32::NEG_INFINITY,
            y: f32::NEG_INFINITY,
        }
    ));
}

#[test]
fn interaction_channels_compose_and_restore_without_rebuilding_paint() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let hover = InteractionStyle::all(
        Color::rgba(11, 22, 33, 44),
        Color::rgb(55, 66, 77),
        Color::rgb(88, 99, 110),
    );
    let focus = InteractionStyle::all(
        Color::rgba(12, 23, 34, 45),
        Color::rgb(56, 67, 78),
        Color::rgb(89, 100, 111),
    );
    let active = InteractionStyle::all(
        Color::rgba(13, 24, 35, 46),
        Color::rgb(57, 68, 79),
        Color::rgb(90, 101, 112),
    );
    let themed = Theme {
        component_interactions: ComponentInteractions {
            button: Some(InteractionStates {
                hover,
                focus,
                active,
            }),
            ..ComponentInteractions::NONE
        },
        ..Theme::default()
    };
    let (mut scene, mut layout, mut text, mut atlas, id) =
        build_one(runtime, ThemeProvider::new(themed, Button::new("state")));
    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 160.0,
            height: 48.0,
        },
    );
    reposition_paint(runtime, &mut scene);
    let base = scene.paint(id).unwrap().primitives.clone();
    let center = {
        let rect = scene.layout(id).unwrap().rect;
        Point {
            x: rect.x + rect.width * 0.5,
            y: rect.y + rect.height * 0.5,
        }
    };
    let assert_channels = |scene: &Scene, style: InteractionStyle, border_width: f32| {
        let paint = scene.paint(id).unwrap();
        assert!(paint.primitives.iter().any(
                |primitive| matches!(primitive, Primitive::SolidRect { color, .. } if Some(*color) == style.background)
            ));
        assert!(paint.primitives.iter().any(
                |primitive| matches!(primitive, Primitive::GlyphQuad { color, .. } if Some(*color) == style.foreground)
            ));
        assert_eq!(
                paint
                    .primitives
                    .iter()
                    .filter(|primitive| matches!(primitive, Primitive::Line { width, color, .. } if *width == border_width && Some(*color) == style.border))
                    .count(),
                FOCUS_RING_PRIMS
            );
    };

    assert!(update_pointer_proximity(runtime, &mut scene, center));
    assert_channels(&scene, hover, HOVER_BORDER_W);
    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        Point {
            x: f32::NEG_INFINITY,
            y: f32::NEG_INFINITY,
        }
    ));
    assert_eq!(scene.paint(id).unwrap().primitives, base);

    assert!(set_active_interaction(runtime, &mut scene, Some(id)));
    assert_channels(&scene, active, ACTIVE_BORDER_W);
    assert!(set_active_interaction(runtime, &mut scene, None));
    assert_eq!(scene.paint(id).unwrap().primitives, base);

    assert!(dispatch_focus(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        Some(id)
    ));
    assert_channels(&scene, focus, FOCUS_RING_W);
    assert!(dispatch_focus(
        runtime, &mut scene, &mut text, &mut atlas, None
    ));
    assert_eq!(scene.paint(id).unwrap().primitives, base);
}

#[test]
fn pointer_hover_never_washes_editors_or_terminal_surfaces() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let terminal = TerminalGridModel::with_colors(4, 2, Color::WHITE, Color::BLACK);
    for (view, expected_kind, border_only) in [
        (
            Box::new(TextArea::new("message")) as Box<dyn View>,
            WidgetKind::TextArea,
            true,
        ),
        (
            Box::new(TerminalGrid::new(terminal.clone()).label("Terminal")) as Box<dyn View>,
            WidgetKind::TerminalGrid,
            false,
        ),
    ] {
        let (mut scene, mut layout, _text, _atlas, id) = build_one(runtime, view);
        assert_eq!(scene.node(id).map(|node| node.kind), Some(expected_kind));
        layout.sync_tree(&scene, id);
        layout.compute(
            &mut scene,
            id,
            Size {
                width: 320.0,
                height: 180.0,
            },
        );
        reposition_paint(runtime, &mut scene);
        let before = scene.paint(id).map_or(0, |paint| paint.primitives.len());
        let solid_before = scene.paint(id).map_or(0, |paint| {
            paint
                .primitives
                .iter()
                .filter(|primitive| matches!(primitive, Primitive::SolidRect { .. }))
                .count()
        });
        let rect = scene.layout(id).expect("surface laid out").rect;
        assert_eq!(
            update_pointer_proximity(
                runtime,
                &mut scene,
                Point {
                    x: rect.x + 2.0,
                    y: rect.y + 2.0,
                },
            ),
            border_only
        );
        assert_eq!(
            scene.paint(id).map_or(0, |paint| {
                paint
                    .primitives
                    .iter()
                    .filter(|primitive| matches!(primitive, Primitive::SolidRect { .. }))
                    .count()
            }),
            solid_before,
            "hover must never add a wash to an editor or terminal",
        );
        let expected = before + if border_only { HOVER_BORDER_PRIMS } else { 0 };
        assert_eq!(
            scene.paint(id).map_or(0, |paint| paint.primitives.len()),
            expected
        );
    }
}

#[test]
fn focus_ring_covers_a_clickable_table_rows_descendant_paint() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _layout, mut text, mut atlas, table) = build_one(
        runtime,
        Table::new()
            .columns(["Name", "State"])
            .row(["Build", "Ready"])
            .on_select_row(|_| {}),
    );
    let row = scene.node(table).unwrap().children[1];
    let last_cell = *scene.node(row).unwrap().children.last().unwrap();

    assert!(dispatch_focus(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        Some(row)
    ));
    assert_eq!(ring_lines(&scene, last_cell), FOCUS_RING_PRIMS);
    assert!(dispatch_focus(
        runtime, &mut scene, &mut text, &mut atlas, None
    ));
    assert_eq!(ring_lines(&scene, last_cell), 0);
}
