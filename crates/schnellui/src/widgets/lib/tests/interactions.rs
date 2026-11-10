use super::base::build_one;
use super::*;

fn ring_lines(scene: &Scene, id: WidgetId) -> usize {
    let prims = &scene.paint(id).unwrap().primitives;
    prims
            .iter()
            .rev()
            .take_while(
                |primitive| matches!(primitive, Primitive::StrokeRect { width, .. } if *width == FOCUS_RING_W),
            )
            .count()
}

/// The focus ring on `id`: its stroked rect and radius.
fn ring_stroke(scene: &Scene, id: WidgetId) -> (Rect, f32, f32, Color) {
    let prims = &scene.paint(id).unwrap().primitives;
    let Some(&Primitive::StrokeRect {
        rect,
        corner_radius,
        width,
        color,
    }) = prims.last()
    else {
        panic!("focus ring is the last primitive");
    };
    (rect, corner_radius, width, color)
}

/// Focus wears the same inset box as native HTML; blur removes it; a
/// state re-emit on the focused widget keeps it (SOUL §6.3).
#[test]
fn focus_ring_applies_survives_toggle_and_clears_on_blur() {
    let runtime_handle = crate::widgets::Runtime::new();
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
    let runtime_handle = crate::widgets::Runtime::new();
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
    let ring_count = |scene: &Scene, id: WidgetId| {
        scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter(|p| matches!(p, Primitive::StrokeRect { .. }))
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
    assert_eq!(line_count(&scene, id), 1, "the caret");
    assert_eq!(
        ring_count(&scene, id),
        FOCUS_RING_PRIMS,
        "browser-equivalent outline"
    );
    assert!(dispatch_edit_key(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        id,
        EditKey::Insert("!")
    ));
    assert_eq!(line_count(&scene, id), 1);
    assert_eq!(ring_count(&scene, id), FOCUS_RING_PRIMS);
    assert!(dispatch_focus(
        runtime, &mut scene, &mut text, &mut atlas, None
    ));
    assert_eq!(line_count(&scene, id), 0);
    assert_eq!(ring_count(&scene, id), 0);
}

#[test]
fn focus_outline_matches_css_width_offset_and_contrast_color() {
    let runtime_handle = crate::widgets::Runtime::new();
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
    let (rect, _radius, width, color) = ring_stroke(&scene, id);
    // The ring sits its width plus a gap outside the control.
    let out = FOCUS_RING_W + FOCUS_RING_GAP;
    assert_eq!(width, FOCUS_RING_W);
    assert_eq!(color, theme_for(runtime, id).focus_color());
    assert_eq!(rect.y, bounds.y - out);
    assert_eq!(rect.x, bounds.x - out);
    assert_eq!(rect.right(), bounds.right() + out);
    assert_eq!(rect.bottom(), bounds.bottom() + out);
}

/// The ring follows the control's own shape: a button's outset ring is its
/// corner radius plus the outset, a round radio gets a circle.
#[test]
fn focus_ring_follows_the_controls_corner_radius() {
    let runtime_handle = crate::widgets::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let out = FOCUS_RING_W + FOCUS_RING_GAP;

    let (mut scene, _l, mut text, mut atlas, id) = build_one(runtime, Button::new("Save"));
    let surface = scene
        .paint(id)
        .unwrap()
        .primitives
        .iter()
        .find_map(|p| match *p {
            Primitive::SolidRect {
                rect,
                corner_radius,
                ..
            } if rect.width > 0.0 => Some((rect, corner_radius)),
            _ => None,
        })
        .expect("button surface");
    assert!(surface.1 > 0.0, "the button is rounded");
    assert!(dispatch_focus(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        Some(id)
    ));
    let (_, radius, _, _) = ring_stroke(&scene, id);
    assert_eq!(radius, surface.1 + out);

    let (mut scene, _l, mut text, mut atlas, id) = build_one(runtime, Radio::new(true));
    let bounds = paint_bounds(runtime, &scene, id).unwrap();
    assert!(dispatch_focus(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        Some(id)
    ));
    let (rect, radius, _, _) = ring_stroke(&scene, id);
    assert_eq!(rect.width, bounds.width + 2.0 * out);
    assert!(
        radius >= rect.width * 0.5 && radius >= rect.height * 0.5,
        "a radio's ring is a circle: radius {radius}, ring {rect:?}"
    );
}

#[test]
fn outset_focus_outline_follows_retained_control_paint() {
    let runtime_handle = crate::widgets::Runtime::new();
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
    let Primitive::StrokeRect { rect, .. } = outline[outline.len() - FOCUS_RING_PRIMS] else {
        panic!("focus outline is a stroked rect");
    };
    assert_eq!(rect.y, target.y - FOCUS_RING_W - FOCUS_RING_GAP);
}

#[test]
fn pointer_hover_decorates_enabled_controls_and_clears_on_leave_or_disable() {
    let runtime_handle = crate::widgets::Runtime::new();
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
        base + 1,
        "hover lays one wash, no border"
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

// Label colours stay light: a state foreground that would drop below 4.5:1
// on the button fill is not applied (see `MIN_STATE_LABEL_CONTRAST`).
#[test]
fn interaction_channels_compose_and_restore_without_rebuilding_paint() {
    let runtime_handle = crate::widgets::Runtime::new();
    let runtime = &runtime_handle;
    let hover = InteractionStyle::all(
        Color::rgba(11, 22, 33, 44),
        Color::rgb(245, 246, 247),
        Color::rgb(88, 99, 110),
    );
    let focus = InteractionStyle::all(
        Color::rgba(12, 23, 34, 45),
        Color::rgb(246, 247, 248),
        Color::rgb(89, 100, 111),
    );
    let active = InteractionStyle::all(
        Color::rgba(13, 24, 35, 46),
        Color::rgb(247, 248, 249),
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
        // The wash keeps its colour; its alpha is re-weighted for the
        // linear-light blend over the button fill.
        let rgb = |c: Color| (c.r, c.g, c.b);
        assert!(paint.primitives.iter().any(
                |primitive| matches!(primitive, Primitive::SolidRect { color, .. } if style.background.map(rgb) == Some(rgb(*color)))
            ));
        assert!(paint.primitives.iter().any(
                |primitive| matches!(primitive, Primitive::GlyphQuad { color, .. } if Some(*color) == style.foreground)
            ));
        assert_eq!(
                paint
                    .primitives
                    .iter()
                    .filter(|primitive| matches!(primitive, Primitive::StrokeRect { width, color, .. } if *width == border_width && Some(*color) == style.border))
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
    let runtime_handle = crate::widgets::Runtime::new();
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
    let runtime_handle = crate::widgets::Runtime::new();
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

/// Scrubbing a hovered, pressed slider repaints it from scratch. The hover
/// and press halos recorded on the old paint must not strip the new rail and
/// fill, during the drag or after the pointer lets go and leaves.
#[test]
fn scrubbing_a_hovered_pressed_slider_keeps_its_rail() {
    let runtime_handle = crate::widgets::Runtime::new();
    let runtime = &runtime_handle;
    let (mut scene, _l, _t, _a, id) = build_one(runtime, Slider::new(40.0, 0.0, 100.0).step(5.0));
    scene.set_layout(
        id,
        LayoutBox {
            rect: Rect::new(10.0, 20.0, 120.0, 20.0),
            content: Rect::ZERO,
        },
    );
    reposition_paint(runtime, &mut scene);
    let rects = |scene: &Scene| -> Vec<Rect> {
        scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter_map(|p| match *p {
                Primitive::SolidRect { rect, .. } => Some(rect),
                _ => None,
            })
            .collect()
    };
    let base = rects(&scene);
    assert_eq!(base.len(), 3, "rail, fill, thumb");
    let rail = base[0];

    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        Point { x: 70.0, y: 30.0 }
    ));
    set_active_interaction(runtime, &mut scene, Some(id));
    for x in [80.0, 95.0, 110.0] {
        assert!(dispatch_slider_pointer(
            runtime,
            &mut scene,
            id,
            Point { x, y: 30.0 }
        ));
        assert!(
            rects(&scene)
                .iter()
                .any(|r| (r.width, r.height) == (rail.width, rail.height)),
            "the rail survives a scrub step at x={x}: {:?}",
            scene.paint(id).unwrap().primitives
        );
    }
    set_active_interaction(runtime, &mut scene, None);
    update_pointer_proximity(runtime, &mut scene, Point { x: 400.0, y: 400.0 });
    let after = rects(&scene);
    assert_eq!(
        after.len(),
        3,
        "only rail, fill and thumb remain: {after:?}"
    );
    assert_eq!((after[0].width, after[0].height), (rail.width, rail.height));
}

/// Hover a control, press it, then let a navigation key take hover away while
/// the button is still down (`hold_pointer_hover`). The hover wash comes off
/// from under the press wash, so the press wash moves down one primitive.
/// Releasing must then remove the press wash, not miss it and leave it
/// painted for good.
#[test]
fn press_wash_comes_off_after_hover_is_taken_from_under_it() {
    for view in [
        Box::new(Button::new("go")) as Box<dyn View>,
        Box::new(ListItem::new("Inbox")),
        Box::new(Checkbox::new(false)),
    ] {
        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let (mut scene, _l, _t, _a, id) = build_one(runtime, view);
        scene.set_layout(
            id,
            LayoutBox {
                rect: Rect::new(10.0, 20.0, 80.0, 24.0),
                content: Rect::ZERO,
            },
        );
        reposition_paint(runtime, &mut scene);
        let base = scene.paint(id).unwrap().primitives.clone();

        assert!(update_pointer_proximity(
            runtime,
            &mut scene,
            Point { x: 20.0, y: 30.0 }
        ));
        assert!(set_active_interaction(runtime, &mut scene, Some(id)));
        assert!(strip_hover_decoration(runtime, &mut scene));
        set_active_interaction(runtime, &mut scene, None);
        assert_eq!(
            scene.paint(id).unwrap().primitives,
            base,
            "{:?} keeps no wash after release",
            scene.node(id).map(|n| n.kind)
        );
    }
}

/// A toggle's hover halo and focus ring are concentric with the control, with
/// the same offset on every side, whatever sits beside it in its row.
#[test]
fn toggle_halo_and_ring_are_concentric_with_the_control() {
    for (kind, theme) in (0..3).flat_map(|kind| {
        [
            crate::theme::LIGHT,
            crate::theme::BRUTAL,
            crate::theme::CANDY,
        ]
        .map(|theme| (kind, theme))
    }) {
        let runtime_handle = crate::widgets::Runtime::new();
        let runtime = &runtime_handle;
        let toggle: Box<dyn View> = match kind {
            0 => Box::new(Checkbox::new(false)),
            1 => Box::new(Switch::new(false)),
            _ => Box::new(Radio::new(false)),
        };
        let (mut scene, mut layout, mut text, mut atlas, root) = build_one(
            runtime,
            ThemeProvider::new(
                theme,
                Row::new()
                    .gap(12.0)
                    .align(Align::Center)
                    .child(toggle)
                    .child(Text::new("Complete Book train tickets")),
            ),
        );
        layout.sync_tree(&scene, root);
        layout.compute(
            &mut scene,
            root,
            Size {
                width: 400.0,
                height: 80.0,
            },
        );
        reposition_paint(runtime, &mut scene);
        let id = scene.node(root).unwrap().children[0];
        let body = paint_bounds(runtime, &scene, id).unwrap();
        let around = |outer: Rect| {
            [
                body.x - outer.x,
                body.y - outer.y,
                outer.x + outer.width - (body.x + body.width),
                outer.y + outer.height - (body.y + body.height),
            ]
        };
        let equal =
            |sides: [f32; 4], want: f32| sides.iter().all(|side| (side - want).abs() < 0.01);

        let center = Point {
            x: body.x + body.width * 0.5,
            y: body.y + body.height * 0.5,
        };
        assert!(update_pointer_proximity(runtime, &mut scene, center));
        let halo = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .find_map(|p| match *p {
                Primitive::SolidRect { rect, .. } if rect.width > body.width + 1.0 => Some(rect),
                _ => None,
            })
            .expect("hover halo");
        assert!(
            equal(around(halo), TOGGLE_HALO),
            "kind {kind}: halo {halo:?} around {body:?}"
        );
        assert_eq!(paint_bounds(runtime, &scene, id).unwrap(), body);

        assert!(dispatch_focus(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            Some(id)
        ));
        let (ring, ..) = ring_stroke(&scene, id);
        assert!(
            equal(around(ring), FOCUS_RING_GAP + FOCUS_RING_W),
            "kind {kind}: ring {ring:?} around {body:?}"
        );
    }
}

/// A focus ring on a rounded navigation row has the row's own corner radius.
#[test]
fn navigation_focus_ring_follows_the_rows_fill_radius() {
    for kind in 0..3 {
        for theme in [crate::theme::LIGHT, crate::theme::CANDY, crate::theme::DARK] {
            let runtime_handle = crate::widgets::Runtime::new();
            let runtime = &runtime_handle;
            let row: Box<dyn View> = match kind {
                0 => Box::new(Tab::new("Inbox").appearance(TabAppearance::Navigation)),
                1 => Box::new(ListItem::new("Inbox").selected(true)),
                _ => Box::new(
                    Tab::new("Inbox")
                        .appearance(TabAppearance::Navigation)
                        .selected(true),
                ),
            };
            let (mut scene, mut layout, mut text, mut atlas, root) =
                build_one(runtime, ThemeProvider::new(theme, Column::new().child(row)));
            layout.sync_tree(&scene, root);
            layout.compute(
                &mut scene,
                root,
                Size {
                    width: 300.0,
                    height: 200.0,
                },
            );
            reposition_paint(runtime, &mut scene);
            let id = scene.node(root).unwrap().children[0];
            let fill = match scene.paint(id).unwrap().primitives[0] {
                Primitive::SolidRect { corner_radius, .. } => corner_radius,
                _ => panic!("row fill is first"),
            };
            assert!(dispatch_focus(
                runtime,
                &mut scene,
                &mut text,
                &mut atlas,
                Some(id)
            ));
            let (_, ring, ..) = ring_stroke(&scene, id);
            assert_eq!(ring, fill, "kind {kind}");
            assert!(update_pointer_proximity(
                runtime,
                &mut scene,
                Point { x: 20.0, y: 10.0 }
            ));
            let (_, ring, ..) = ring_stroke(&scene, id);
            assert_eq!(ring, fill, "kind {kind} hovered");
        }
    }
}

/// A button's tooltip comes down when the button opens its menu, and stays
/// down while the menu is open and the pointer rests or moves on the button.
#[test]
fn opening_a_buttons_menu_hides_its_tooltip() {
    let runtime_handle = crate::widgets::Runtime::new();
    let runtime = &runtime_handle;
    let (mut scene, mut layout, mut text, mut atlas, id) = build_one(
        runtime,
        Button::new("More")
            .tooltip("More actions")
            .width(60.0)
            .height(28.0)
            .menu(ContextMenu::new().item(ContextMenuItem::new("Rename"))),
    );
    let viewport = Size {
        width: 300.0,
        height: 200.0,
    };
    layout.sync_tree(&scene, id);
    layout.compute(&mut scene, id, viewport);
    reposition_paint(runtime, &mut scene);
    let shown = |runtime: &crate::widgets::Runtime| {
        runtime.with(|rt| rt.borrow().hover_tooltips.get(id).map(|t| t.visible))
    };
    let on_button = Point { x: 10.0, y: 10.0 };
    assert!(update_pointer_proximity(runtime, &mut scene, on_button));
    assert_eq!(shown(runtime), Some(true));

    assert!(open_context_menu(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        id,
        Point { x: 0.0, y: 28.0 },
        viewport,
        1.0,
        false,
    ));
    assert_eq!(shown(runtime), Some(false), "hidden as the menu opens");
    update_pointer_proximity(runtime, &mut scene, Point { x: 11.0, y: 10.0 });
    assert_eq!(shown(runtime), Some(false), "stays hidden while it is open");

    dismiss_context_menu(runtime, &mut scene);
    update_pointer_proximity(runtime, &mut scene, Point { x: 12.0, y: 10.0 });
    assert_eq!(shown(runtime), Some(true), "back once it closes");
}
