use super::*;

#[test]
fn dock_position_maps_center_and_all_four_edges() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let rect = Rect::new(10.0, 20.0, 200.0, 100.0);
    assert_eq!(
        dock_position(rect, Point { x: 110.0, y: 70.0 }),
        DockPosition::Center
    );
    assert_eq!(
        dock_position(rect, Point { x: 15.0, y: 70.0 }),
        DockPosition::Left
    );
    assert_eq!(
        dock_position(rect, Point { x: 205.0, y: 70.0 }),
        DockPosition::Right
    );
    assert_eq!(
        dock_position(rect, Point { x: 110.0, y: 23.0 }),
        DockPosition::Top
    );
    assert_eq!(
        dock_position(rect, Point { x: 110.0, y: 116.0 }),
        DockPosition::Bottom
    );
}

/// Builds `view` into a fresh scene, returning the scene, layout engine, the
/// pooled shaper + glyph atlas (needed to drive `run_dynamic_slots`), and the
/// root id.
pub(super) fn build_one(
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

// --- builder-chain contract (frozen skeleton) ---

#[test]
fn column_child_chaining_accumulates() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let col = Column::new()
        .child(Text::new("Counter"))
        .child(Text::dynamic(|| "0".to_string()))
        .gap(4.0);
    assert_eq!(col.child_count(), 2);
    assert_eq!(col.kind(), WidgetKind::Column);
    assert_eq!(col.style.gap, 4.0);
}

#[test]
fn text_static_vs_dynamic() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let s = Text::new("hi");
    assert!(!s.is_dynamic());
    assert_eq!(s.role, Role::Label);
    let d = Text::dynamic(|| "x".to_string())
        .role(Role::Status)
        .size(20.0);
    assert!(d.is_dynamic());
    assert_eq!(d.role, Role::Status);
    assert_eq!(d.size_px, 20.0);
}

#[test]
fn button_handler_and_role() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    // handler stored, not yet fired
    let b = Button::new("increment").on_click(|| {});
    assert!(b.has_handler());
    assert_eq!(b.role(), Role::Button);
    assert_eq!(b.kind(), WidgetKind::Button);
}

#[test]
fn fixed_width_button_expands_surface_and_centers_label() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    set_theme(runtime, Theme::default());
    let (scene, _layout, _text, _atlas, id) = build_one(runtime, Button::new("1").width(80.0));
    let Primitive::SolidRect { rect, .. } = scene.paint(id).unwrap().primitives[0] else {
        panic!("button surface is not a solid rectangle");
    };
    assert_eq!(rect.width, 80.0);

    let glyph = scene
        .paint(id)
        .unwrap()
        .primitives
        .iter()
        .find_map(|primitive| match primitive {
            Primitive::GlyphQuad { rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("button label has a glyph");
    assert!(glyph.x > 30.0, "label is centered in the fixed-width key");
}

#[test]
fn ghost_button_keeps_button_semantics_with_transparent_chrome() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    set_theme(runtime, Theme::default());
    let (scene, _layout, _text, _atlas, id) = build_one(
        runtime,
        Button::new("Refresh").appearance(ButtonAppearance::Ghost),
    );
    let Primitive::SolidRect { color, .. } = scene.paint(id).unwrap().primitives[0] else {
        panic!("ghost button surface is not a solid rectangle");
    };
    assert_eq!(color, Color::TRANSPARENT);
    assert_eq!(Role::from_u16(scene.a11y(id).unwrap().role), Role::Button);
}

#[test]
fn icon_only_button_keeps_its_name_and_reveals_a_compact_hover_label() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    set_theme(runtime, Theme::default());
    let (mut scene, mut layout, _text, _atlas, id) = build_one(
        runtime,
        Button::new("Delete Notes")
            .icon_only()
            .tooltip("Delete Notes")
            .width(28.0)
            .height(28.0)
            .appearance(ButtonAppearance::Ghost),
    );
    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 200.0,
            height: 100.0,
        },
    );
    reposition_paint(runtime, &mut scene);

    assert_eq!(
        scene.a11y(id).and_then(|a11y| a11y.name.as_deref()),
        Some("Delete Notes")
    );
    let visible = |scene: &Scene| {
        scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter(|primitive| match primitive {
                Primitive::SolidRect { color, .. }
                | Primitive::GlyphQuad { color, .. }
                | Primitive::Line { color, .. } => color.a != 0,
                Primitive::ImageQuad { tint, .. } => tint.a != 0,
            })
            .count()
    };
    assert_eq!(visible(&scene), 0);
    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        Point { x: 14.0, y: 14.0 }
    ));
    assert!(visible(&scene) > 1);
    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        Point { x: 190.0, y: 90.0 }
    ));
    assert_eq!(visible(&scene), 0);
}

#[test]
fn hover_tooltip_stays_inside_viewport_at_top_left_edge() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    set_theme(runtime, Theme::default());
    let (mut scene, mut layout, _text, _atlas, root) = build_one(
        runtime,
        Stack::new().size(200.0, 100.0).child(
            Pad::all(4.0).child(
                Button::new("Settings  Ctrl+,")
                    .icon_only()
                    .tooltip("Settings  Ctrl+,")
                    .width(22.0)
                    .height(22.0)
                    .appearance(ButtonAppearance::Ghost),
            ),
        ),
    );
    layout.sync_tree(&scene, root);
    layout.compute(
        &mut scene,
        root,
        Size {
            width: 200.0,
            height: 100.0,
        },
    );
    reposition_paint(runtime, &mut scene);

    let pad = scene.node(root).unwrap().children[0];
    let button = scene.node(pad).unwrap().children[0];
    let tooltip = runtime.with(|registry| {
        *registry
            .borrow()
            .hover_tooltips
            .get(button)
            .expect("button registers its tooltip")
    });
    let Primitive::SolidRect { rect, .. } =
        scene.paint(button).unwrap().primitives[tooltip.background]
    else {
        panic!("tooltip background is a solid rectangle");
    };
    let viewport = scene.layout(root).unwrap().rect;
    let target = scene.layout(button).unwrap().rect;
    assert!(rect.x >= viewport.x + TOOLTIP_VIEWPORT_MARGIN);
    assert!(rect.right() <= viewport.right() - TOOLTIP_VIEWPORT_MARGIN);
    assert!(rect.y >= target.bottom(), "top-edge tooltip flips below");
    assert!(rect.bottom() <= viewport.bottom() - TOOLTIP_VIEWPORT_MARGIN);
}

#[test]
fn minimum_size_extension_constrains_any_component_without_a_scene_wrapper() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, mut layout, _text, _atlas, id) =
        build_one(runtime, Button::new("OK").min_width(160.0).min_height(44.0));
    assert_eq!(scene.node(id).unwrap().kind, WidgetKind::Button);
    assert!(scene.node(id).unwrap().parent.is_none());

    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 320.0,
            height: 200.0,
        },
    );

    let rect = scene.layout(id).unwrap().rect;
    assert_eq!(rect.width, 160.0);
    assert_eq!(rect.height, 44.0);
}

#[test]
fn pad_and_spacer_kinds() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let p = Pad::all(8.0).child(Spacer::new());
    assert_eq!(p.kind(), WidgetKind::Pad);
    assert_eq!(p.insets.left, 8.0);
    assert_eq!(Spacer::new().kind(), WidgetKind::Spacer);
}

#[test]
fn every_content_widget_has_a_role() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    assert_eq!(Button::new("b").role(), Role::Button);
    assert_eq!(Checkbox::new(false).role(), Role::CheckBox);
    assert_eq!(Slider::new(0.0, 0.0, 1.0).role(), Role::Slider);
    assert_eq!(TextInput::new("").role(), Role::TextInput);
    assert_eq!(Image::new("x").role(), Role::Image);
}

#[test]
fn text_input_width_expands_the_painted_field() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (scene, _layout, _text, _atlas, id) = build_one(
        runtime,
        TextInput::new("value").label("Endpoint").width(280.0),
    );
    let Primitive::SolidRect { rect, .. } = scene.paint(id).unwrap().primitives[0] else {
        panic!("text input border should be a solid rectangle");
    };
    assert_eq!(rect.width, 280.0);
}

#[test]
fn password_input_obscures_semantics_but_not_its_input_callback() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let seen = Rc::new(RefCell::new(Vec::new()));
    let callback_values = Rc::clone(&seen);
    let (mut scene, _layout, mut text, mut atlas, id) = build_one(
        runtime,
        PasswordInput::new("sécret")
            .label("API key")
            .on_input(move |value| callback_values.borrow_mut().push(value.to_owned())),
    );

    let semantics = scene.a11y(id).unwrap();
    assert_eq!(Role::from_u16(semantics.role), Role::PasswordInput);
    assert_eq!(semantics.name.as_deref(), Some("API key"));
    assert_eq!(semantics.value.as_deref(), Some("••••••"));
    assert!(dispatch_edit_key(
        runtime,
        &mut scene,
        &mut text,
        &mut atlas,
        id,
        EditKey::Insert("!")
    ));
    assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("•••••••"));
    assert_eq!(seen.borrow().as_slice(), ["sécret!"]);
}

// --- build-time semantics (SOUL §6.1 — no widget without a role) ---

#[test]
fn build_button_carries_role_name_and_click_action() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (scene, _layout, _text, _atlas, id) = build_one(runtime, Button::new("increment"));
    let a = scene.a11y(id).expect("a11y column written at build");
    assert_eq!(Role::from_u16(a.role), Role::Button);
    assert_eq!(a.name.as_deref(), Some("increment"));
    assert!(ActionFlags(a.actions).contains(ActionFlags::CLICK));
    assert!(!StateFlags(a.state).contains(StateFlags::DISABLED));
    // paint fragments emitted (background SolidRect + label glyph quads), §8.1
    let prims = &scene.paint(id).unwrap().primitives;
    assert!(!prims.is_empty());
    // the background is a SolidRect; the label is rendered as real glyph quads
    assert!(
        matches!(prims[0], Primitive::SolidRect { .. }),
        "button bg is a SolidRect"
    );
    assert!(
        prims
            .iter()
            .any(|p| matches!(p, Primitive::GlyphQuad { .. })),
        "button label renders as glyph quads, not a solid block"
    );
}

// --- responsive flex (SOUL §8.1: Flex factors, wrap, per-axis width) ---

#[test]
fn flex_wraps_its_child_without_an_extra_node() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, mut layout, _text, _atlas, id) = build_one(
        runtime,
        Row::new()
            .size(300.0, 40.0)
            .child(Flex::new().grow(1.0).child(Button::new("stretch"))),
    );
    // no wrapper node: the button is the row's direct child.
    assert_eq!(scene.node(id).unwrap().children.len(), 1);
    let child = scene.node(id).unwrap().children[0];
    assert_eq!(scene.node(child).unwrap().kind, WidgetKind::Button);

    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 300.0,
            height: 40.0,
        },
    );
    // grow=1 claims the whole 300px main axis for the lone child.
    assert_eq!(scene.layout(child).unwrap().rect.width, 300.0);
}

#[test]
fn childless_flex_is_a_weighted_spacer() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, mut layout, _text, _atlas, id) = build_one(
        runtime,
        Row::new()
            .size(300.0, 20.0)
            .child(Flex::new().grow(1.0))
            .child(Flex::new().grow(2.0)),
    );
    let kids = scene.node(id).unwrap().children.clone();
    assert_eq!(scene.node(kids[0]).unwrap().kind, WidgetKind::Spacer);

    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 300.0,
            height: 20.0,
        },
    );
    // weights 1:2 over the 300px of free space.
    assert_eq!(scene.layout(kids[0]).unwrap().rect.width, 100.0);
    assert_eq!(scene.layout(kids[1]).unwrap().rect.width, 200.0);
}

#[test]
fn wrapped_row_with_definite_width_flows_lines() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    // `.width()` fixes only the line width; the height must come from the
    // number of wrapped lines (SOUL §8.1 responsive flow).
    let (mut scene, mut layout, _text, _atlas, id) = build_one(
        runtime,
        Row::new()
            .wrap()
            .width(140.0)
            .child(Image::new("a"))
            .child(Image::new("b"))
            .child(Image::new("c")),
    );
    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 140.0,
            height: 400.0,
        },
    );
    let kids = scene.node(id).unwrap().children.clone();
    let (a, b, c) = (
        scene.layout(kids[0]).unwrap().rect,
        scene.layout(kids[1]).unwrap().rect,
        scene.layout(kids[2]).unwrap().rect,
    );
    // two 64px placeholders fit the 140px line; the third wraps below.
    assert_eq!(a.y, 0.0);
    assert_eq!(b.y, 0.0);
    assert_eq!(c.x, 0.0);
    assert_eq!(c.y, a.height);
    // the row's own height is the wrapped content, not a fixed size.
    assert_eq!(scene.layout(id).unwrap().rect.height, a.height + c.height);
}

#[test]
fn justify_end_pushes_the_child_to_the_far_edge() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, mut layout, _text, _atlas, id) = build_one(
        runtime,
        Row::new()
            .size(300.0, 80.0)
            .justify(Justify::End)
            .child(Image::new("x")),
    );
    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 300.0,
            height: 80.0,
        },
    );
    let child = scene.node(id).unwrap().children[0];
    let b = scene.layout(child).unwrap().rect;
    assert_eq!(b.x, 300.0 - b.width);
}

#[test]
fn build_disabled_button_sets_state() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (scene, _layout, _text, _atlas, id) = build_one(runtime, Button::new("x").disabled(true));
    assert!(StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::DISABLED));
}

#[test]
fn container_builds_children_under_group() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (scene, _layout, _text, _atlas, id) = build_one(
        runtime,
        Column::new()
            .child(Text::new("Counter"))
            .child(Button::new("inc")),
    );
    assert_eq!(scene.node(id).unwrap().kind, WidgetKind::Column);
    assert_eq!(scene.node(id).unwrap().children.len(), 2);
    assert_eq!(Role::from_u16(scene.a11y(id).unwrap().role), Role::Group);
    // the label child is a Text leaf carrying its own role
    let child0 = scene.node(id).unwrap().children[0];
    assert_eq!(scene.node(child0).unwrap().kind, WidgetKind::Text);
    assert_eq!(scene.a11y(child0).unwrap().name.as_deref(), Some("Counter"));
}

// --- input handling: pointer and ActionRequest converge (SOUL §6.3) ---

#[test]
fn button_click_fires_stored_handler() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let count = create_signal(0i32);
    let (mut scene, _layout, _text, _atlas, id) = build_one(
        runtime,
        Button::new("inc").on_click(move || {
            count.update(|v| *v += 1);
        }),
    );
    assert!(dispatch_click(runtime, &mut scene, id)); // the same path a Click ActionRequest takes
    assert_eq!(count.get(), 1);
    assert!(dispatch_click(runtime, &mut scene, id));
    assert_eq!(count.get(), 2);
}

#[test]
fn disabled_button_is_inert() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let count = create_signal(0i32);
    let (mut scene, _layout, _text, _atlas, id) = build_one(
        runtime,
        Button::new("inc")
            .disabled(true)
            .on_click(move || count.update(|v| *v += 1)),
    );
    assert!(!dispatch_click(runtime, &mut scene, id));
    assert_eq!(count.get(), 0);
}

#[test]
fn checkbox_toggles_state_and_fires_handler() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let last = create_signal(false);
    let (mut scene, _layout, _text, _atlas, id) = build_one(
        runtime,
        Checkbox::new(false).on_toggle(move |b| last.set(b)),
    );
    assert!(!StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::CHECKED));
    assert!(dispatch_click(runtime, &mut scene, id));
    assert!(StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::CHECKED));
    assert!(last.get());
    assert!(scene.dirty_flags(id).contains(DirtyFlags::A11Y));
    assert!(scene.dirty_flags(id).contains(DirtyFlags::PAINT));
    // toggle back
    assert!(dispatch_click(runtime, &mut scene, id));
    assert!(!StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::CHECKED));
    assert!(!last.get());
}

/// A node whose paint is a `Line` (as charts emit) anchors by its stroke-inclusive
/// min-origin and slides both endpoints onto the laid-out origin (SOUL §3.2/§8.1).
#[test]
fn reposition_slides_line_endpoints_by_min_origin() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let mut scene = Scene::new();
    let id = scene.insert(WidgetKind::Chart, None);
    scene.set_root(id);
    // A line emitted at a provisional local origin, stroke width 4.
    scene.replace_primitives(
        id,
        [Primitive::Line {
            from: Point { x: 0.0, y: 0.0 },
            to: Point { x: 10.0, y: 20.0 },
            width: 4.0,
            color: Color::BLACK,
        }],
    );
    // Laid-out origin at (100, 50).
    scene.set_layout(
        id,
        LayoutBox {
            rect: Rect::new(100.0, 50.0, 30.0, 40.0),
            content: Rect::ZERO,
        },
    );
    reposition_node(runtime, &mut scene, id);
    match scene.paint(id).unwrap().primitives[0] {
        Primitive::Line {
            from, to, width, ..
        } => {
            // stroke-inclusive min-origin now lands exactly on the layout origin
            let min_x = from.x.min(to.x) - width * 0.5;
            let min_y = from.y.min(to.y) - width * 0.5;
            assert!((min_x - 100.0).abs() < 0.001);
            assert!((min_y - 50.0).abs() < 0.001);
            // both endpoints slid by the same delta (segment shape preserved)
            assert!((to.x - from.x - 10.0).abs() < 0.001);
            assert!((to.y - from.y - 20.0).abs() < 0.001);
        }
        ref p => panic!("expected a Line, got {p:?}"),
    }
    // idempotent: a second call is a no-op (already anchored, SOUL Directive #1)
    reposition_node(runtime, &mut scene, id);
    match scene.paint(id).unwrap().primitives[0] {
        Primitive::Line { from, .. } => assert!((from.x - 102.0).abs() < 0.001),
        ref p => panic!("expected a Line, got {p:?}"),
    }
}

#[test]
fn hit_test_resolves_leaf_after_layout() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (mut scene, _layout, _text, _atlas, id) = build_one(runtime, Button::new("hit me"));
    scene.set_layout(
        id,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, 100.0, 40.0),
            content: Rect::ZERO,
        },
    );
    assert_eq!(
        hit_test(runtime, &scene, Point { x: 10.0, y: 10.0 }),
        Some(id)
    );
    assert_eq!(hit_test(runtime, &scene, Point { x: 500.0, y: 10.0 }), None);
}

#[test]
fn hit_test_tracks_scrolled_and_clipped_leaf_geometry() {
    let runtime = crate::Runtime::new();
    let mut scene = Scene::new();
    let scroll = scene.insert(WidgetKind::Scroll, None);
    scene.set_root(scroll);
    scene.set_layout(
        scroll,
        LayoutBox {
            rect: Rect::new(0.0, 0.0, 200.0, 100.0),
            content: Rect::ZERO,
        },
    );
    let button = scene.insert(WidgetKind::Button, Some(scroll));
    scene.a11y_mut(button).actions = ActionFlags::CLICK.0;
    scene.set_layout(
        button,
        LayoutBox {
            rect: Rect::new(20.0, 140.0, 120.0, 40.0),
            content: Rect::ZERO,
        },
    );
    scene.set_scroll_offset(scroll, Point { x: 0.0, y: 100.0 });

    // The renderer composites the button at y=40..80 after scrolling.
    assert_eq!(
        hit_test(&runtime, &scene, Point { x: 40.0, y: 60.0 }),
        Some(button)
    );
    assert_eq!(
        cursor_at(&runtime, &scene, Point { x: 40.0, y: 60.0 }),
        CursorIcon::Pointer
    );
    // Its stale layout-space position is outside the viewport and must not hit.
    assert_eq!(
        hit_test(&runtime, &scene, Point { x: 40.0, y: 160.0 }),
        None
    );
    assert_eq!(
        cursor_at(&runtime, &scene, Point { x: 40.0, y: 160.0 }),
        CursorIcon::Default
    );
}

fn cursor_for(runtime: &crate::Runtime, view: impl View) -> CursorIcon {
    let (mut scene, mut layout, _text, _atlas, id) = build_one(runtime, view);
    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 320.0,
            height: 120.0,
        },
    );
    reposition_paint(runtime, &mut scene);
    let rect = scene.layout(id).unwrap().rect;
    cursor_at(
        runtime,
        &scene,
        Point {
            x: rect.x + rect.width * 0.5,
            y: rect.y + rect.height * 0.5,
        },
    )
}

#[test]
fn semantic_cursors_cover_controls_editing_and_disabled_state() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    assert_eq!(
        cursor_for(runtime, Button::new("Save")),
        CursorIcon::Pointer
    );
    assert_eq!(
        cursor_for(runtime, Button::new("Unavailable").disabled(true)),
        CursorIcon::Default
    );
    assert_eq!(
        cursor_for(runtime, TextInput::new("draft")),
        CursorIcon::Text
    );
    assert_eq!(
        cursor_for(runtime, Slider::new(40.0, 0.0, 100.0)),
        CursorIcon::EwResize
    );
    assert_eq!(cursor_for(runtime, Text::new("plain")), CursorIcon::Default);
}

#[test]
fn clickable_drag_source_keeps_pointer_until_drag_starts() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    assert_eq!(
        cursor_for(
            runtime,
            Tab::new("Changes").on_select(|| {}).on_drag_start(|| {})
        ),
        CursorIcon::Pointer
    );
    assert_eq!(
        cursor_for(runtime, DragHandle::new("Move pane").on_drag_start(|| {})),
        CursorIcon::Grab
    );
}

#[test]
fn hit_test_follows_mutable_order_within_one_overlay_level() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Stack, None);
    scene.set_root(root);
    let lower_layer = scene.insert(WidgetKind::DialogLayer, Some(root));
    scene.set_overlay_level(lower_layer, 10);
    let lower = scene.insert(WidgetKind::Button, Some(lower_layer));
    let upper_layer = scene.insert(WidgetKind::DialogLayer, Some(root));
    scene.set_overlay_level(upper_layer, 10);
    let upper = scene.insert(WidgetKind::Button, Some(upper_layer));
    for id in [root, lower_layer, lower, upper_layer, upper] {
        scene.set_layout(
            id,
            LayoutBox {
                rect: Rect::new(0.0, 0.0, 100.0, 100.0),
                content: Rect::new(0.0, 0.0, 100.0, 100.0),
            },
        );
    }

    let point = Point { x: 20.0, y: 20.0 };
    assert_eq!(hit_test(runtime, &scene, point), Some(upper));
    assert!(scene.bring_overlay_to_front(lower_layer));
    assert_eq!(
        hit_test(runtime, &scene, point),
        Some(lower),
        "the same order that paints on top receives input"
    );
    assert_eq!(
        scene.overlay_level(lower_layer),
        scene.overlay_level(upper_layer)
    );
}

// --- dynamic slot: signal change mutates the retained node (SOUL §3.3) ---

#[test]
fn dynamic_text_updates_retained_content_on_signal_set_and_flush() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let count = create_signal(0i32);
    let (mut scene, _layout, mut text, mut atlas, id) = build_one(
        runtime,
        Text::dynamic(move || count.get().to_string()).role(Role::Status),
    );
    // first render populated the retained node
    assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("0"));
    let paint_before = scene.paint(id).unwrap().primitives.clone();

    // drive the signal, then settle: Runtime::flush() (the reactive pull) +
    // run_dynamic_slots (the widgets-side render-effect pull the paint pass runs)
    count.set(42);
    Runtime::flush();
    run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);

    // retained content mutated in place
    assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("42"));
    // paint re-shaped to new glyph quads + both dirty channels flagged (§6.2)
    assert_ne!(scene.paint(id).unwrap().primitives, paint_before);
    // the re-emitted primitives are real glyph quads, not solid placeholder blocks
    assert!(scene
        .paint(id)
        .unwrap()
        .primitives
        .iter()
        .all(|p| matches!(p, Primitive::GlyphQuad { .. })));
    assert!(scene.dirty_flags(id).contains(DirtyFlags::PAINT));
    assert!(scene.dirty_flags(id).contains(DirtyFlags::A11Y));
    // width changed ("0" → "42") ⇒ the layout channel is flagged too (§8.1)
    assert!(scene.dirty_flags(id).contains(DirtyFlags::LAYOUT));
}

#[test]
fn dynamic_text_no_change_is_a_no_op() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let count = create_signal(7i32);
    let (mut scene, _layout, mut text, mut atlas, id) =
        build_one(runtime, Text::dynamic(move || count.get().to_string()));
    scene.clear_dirty();
    // same value → producer runs, diff suppresses any mutation (§3.1 gate)
    count.set(7);
    Runtime::flush();
    run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);
    assert!(scene.dirty_flags(id).is_empty());
}

#[test]
fn unrelated_signal_does_not_invoke_another_dynamic_text_producer() {
    use std::cell::Cell;

    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let left = create_signal(0_i32);
    let right = create_signal(0_i32);
    let left_calls = Rc::new(Cell::new(0_u32));
    let right_calls = Rc::new(Cell::new(0_u32));
    let left_calls_for_view = left_calls.clone();
    let right_calls_for_view = right_calls.clone();
    let (mut scene, _layout, mut text, mut atlas, _id) = build_one(
        runtime,
        Column::new()
            .child(Text::dynamic(move || {
                left_calls_for_view.set(left_calls_for_view.get() + 1);
                left.get().to_string()
            }))
            .child(Text::dynamic(move || {
                right_calls_for_view.set(right_calls_for_view.get() + 1);
                right.get().to_string()
            })),
    );
    assert_eq!(left_calls.get(), 1);
    assert_eq!(right_calls.get(), 1);

    left.set(1);
    Runtime::flush();
    run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);

    assert_eq!(left_calls.get(), 2);
    assert_eq!(
        right_calls.get(),
        1,
        "unrelated producer must not be called"
    );
}

#[test]
fn purged_dynamic_text_discards_queued_subscription_work() {
    use std::cell::Cell;

    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let value = create_signal(0_i32);
    let calls = Rc::new(Cell::new(0_u32));
    let producer_calls = calls.clone();
    let (mut scene, _layout, mut text, mut atlas, id) = build_one(
        runtime,
        Text::dynamic(move || {
            producer_calls.set(producer_calls.get() + 1);
            value.get().to_string()
        }),
    );
    assert_eq!(calls.get(), 1);

    value.set(1);
    Runtime::flush();
    purge_nodes(runtime, &mut scene, &[id]);
    run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);

    assert_eq!(calls.get(), 1, "purged producer must not run");
}

// --- real text renders as glyph quads, measured by the shaper (SOUL §8.1) ---

#[test]
fn text_renders_as_glyph_quads_not_a_solid_block() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (scene, _layout, _text, _atlas, id) = build_one(runtime, Text::new("Hi"));
    let prims = &scene.paint(id).unwrap().primitives;
    assert!(!prims.is_empty(), "text emits paint");
    // every primitive is a per-glyph quad — the fake "text as block" is gone
    assert!(
        prims
            .iter()
            .all(|p| matches!(p, Primitive::GlyphQuad { .. })),
        "text must render as glyph quads, not SolidRect placeholders"
    );
    // two inked letters ⇒ at least two glyph quads, each sampling a real atlas rect
    assert!(prims.len() >= 2, "one quad per inked glyph");
    for p in prims {
        if let Primitive::GlyphQuad { atlas_uv, rect, .. } = p {
            assert!(!atlas_uv.is_empty(), "glyph samples a non-empty atlas rect");
            assert!(!rect.is_empty(), "glyph has a non-empty destination rect");
        }
    }
}

/// The intrinsic measure now comes from the shaper (glyph-exact), not a heuristic:
/// longer text lays out wider (SOUL §8.1).
#[test]
fn measure_uses_shaped_width() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    fn laid_out_width(runtime: &crate::Runtime, s: &'static str) -> f32 {
        let (mut scene, mut layout, _text, _atlas, id) = build_one(runtime, Text::new(s));
        layout.sync_tree(&scene, id);
        layout.compute(
            &mut scene,
            id,
            Size {
                width: 1000.0,
                height: 100.0,
            },
        );
        scene.layout(id).unwrap().rect.width
    }
    assert!(
        laid_out_width(runtime, "iiiiiiii") > laid_out_width(runtime, "i"),
        "shaped width grows with text length"
    );
}

// --- rasterized images: real pixels in the scene atlas (SOUL §3.2, §8.1) ---

#[test]
fn image_alt_is_shared_by_hover_text_and_the_accessibility_tree() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    // a 4×2 opaque red bitmap
    let mut px = Vec::new();
    for _ in 0..8 {
        px.extend_from_slice(&[0xff, 0x00, 0x00, 0xff]);
    }
    let (mut scene, mut layout, _t, _a, id) =
        build_one(runtime, Image::from_rgba(4, 2, px).alt("red strip"));

    // The retained semantics feed the concrete AccessKit node emitted to the
    // platform accessibility adapter.
    let a = scene.a11y(id).unwrap();
    assert_eq!(Role::from_u16(a.role), Role::Image);
    assert_eq!(a.name.as_deref(), Some("red strip"));
    let update = schnellui_a11y::build_full_tree_update(&scene);
    let node = update
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == schnellui_a11y::to_access_id(id))
        .map(|(_, node)| node)
        .expect("image is present in the AccessKit tree");
    assert_eq!(node.role(), schnellui_a11y::accesskit_reexport::Role::Image);
    assert_eq!(node.label(), Some("red strip"));

    // paint: one real ImageQuad, no placeholder SolidRect
    let prims = &scene.paint(id).unwrap().primitives;
    let tooltip = runtime.with(|runtime| {
        *runtime
            .borrow()
            .hover_tooltips
            .get(id)
            .expect("meaningful image registers its alt as hover text")
    });
    assert_eq!(tooltip.base_primitive_end, 1);
    let Primitive::ImageQuad {
        rect,
        atlas_uv,
        tint,
    } = prims[0]
    else {
        panic!("expected an ImageQuad, got {:?}", prims[0]);
    };
    assert_eq!(tint, Color::WHITE);
    // display size defaults to the pixel dimensions (logical px)
    assert_eq!((rect.width, rect.height), (4.0, 2.0));
    assert_eq!((atlas_uv.width, atlas_uv.height), (4.0, 2.0));
    // the pixels landed in the scene's shared atlas
    assert!(!scene.images().is_empty());
    let stride = scene.images().width() as usize * 4;
    let base = (atlas_uv.y as usize) * stride + (atlas_uv.x as usize) * 4;
    assert_eq!(
        &scene.images().pixels()[base..base + 4],
        &[0xff, 0, 0, 0xff]
    );

    // Hovering only recolors the pre-rasterized tooltip fragment; before
    // hover and after leaving it is fully transparent.
    let tooltip_is_visible = |scene: &Scene| {
        let prims = &scene.paint(id).unwrap().primitives;
        matches!(
            prims[tooltip.background],
            Primitive::SolidRect { color, .. } if color.a != 0
        ) && prims[tooltip.glyph_start..tooltip.glyph_end].iter().any(
            |primitive| matches!(primitive, Primitive::GlyphQuad { color, .. } if color.a != 0),
        )
    };
    assert!(!tooltip_is_visible(&scene));
    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 100.0,
            height: 100.0,
        },
    );
    reposition_paint(runtime, &mut scene);
    let image_rect = scene.layout(id).unwrap().rect;
    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        Point {
            x: image_rect.x + image_rect.width * 0.5,
            y: image_rect.y + image_rect.height * 0.5,
        }
    ));
    assert!(tooltip_is_visible(&scene));
    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        Point {
            x: image_rect.right() + 100.0,
            y: image_rect.bottom() + 100.0,
        }
    ));
    assert!(!tooltip_is_visible(&scene));
}

#[test]
fn dynamic_image_first_frame_preserves_its_hover_tooltip() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let revision = Rc::new(std::cell::Cell::new(0_u64));
    let frame = Rc::new(RefCell::new(None));
    let revision_source = Rc::clone(&revision);
    let frame_source = Rc::clone(&frame);
    let (mut scene, mut layout, _text, _atlas, id) = build_one(
        runtime,
        Image::dynamic_rgba_versioned(
            move || revision_source.get(),
            move || frame_source.borrow().clone(),
        )
        .alt("live browser frame")
        .size(80.0, 40.0),
    );
    layout.sync_tree(&scene, id);
    layout.compute(
        &mut scene,
        id,
        Size {
            width: 100.0,
            height: 60.0,
        },
    );
    reposition_paint(runtime, &mut scene);

    *frame.borrow_mut() = Some(DynamicImageFrame::new(
        2,
        1,
        vec![255, 0, 0, 255, 0, 255, 0, 255],
    ));
    revision.set(1);
    poll_dynamic_images(runtime, &mut scene);

    let image_rect = scene.layout(id).unwrap().rect;
    assert!(update_pointer_proximity(
        runtime,
        &mut scene,
        Point {
            x: image_rect.x + image_rect.width * 0.5,
            y: image_rect.y + image_rect.height * 0.5,
        }
    ));
    let tooltip = runtime.with(|runtime| {
        *runtime
            .borrow()
            .hover_tooltips
            .get(id)
            .expect("dynamic image keeps its tooltip registration")
    });
    let primitives = &scene.paint(id).unwrap().primitives;
    assert!(tooltip.glyph_end <= primitives.len());
    assert!(matches!(
        primitives[tooltip.background],
        Primitive::SolidRect { color, .. } if color.a != 0
    ));
    assert!(matches!(
        primitives[0],
        Primitive::ImageQuad { rect, .. } if rect.x == image_rect.x && rect.y == image_rect.y
    ));
}

#[test]
fn dynamic_image_reuses_its_largest_allocation_after_shrinking() {
    let runtime_handle = crate::Runtime::new();
    let runtime = &runtime_handle;
    let revision = Rc::new(std::cell::Cell::new(0_u64));
    let frame = Rc::new(RefCell::new(Some(DynamicImageFrame::new(
        4,
        4,
        vec![0xff; 4 * 4 * 4],
    ))));
    let revision_source = Rc::clone(&revision);
    let frame_source = Rc::clone(&frame);
    let (mut scene, _layout, _text, _atlas, id) = build_one(
        runtime,
        Image::dynamic_rgba_versioned(
            move || revision_source.get(),
            move || frame_source.borrow().clone(),
        ),
    );
    let original = runtime.with(|runtime| {
        runtime
            .borrow()
            .dynamic_images
            .get(id)
            .unwrap()
            .texels
            .unwrap()
    });

    *frame.borrow_mut() = Some(DynamicImageFrame::new(2, 2, vec![0x80; 2 * 2 * 4]));
    revision.set(1);
    poll_dynamic_images(runtime, &mut scene);
    let retained = runtime.with(|runtime| {
        runtime
            .borrow()
            .dynamic_images
            .get(id)
            .unwrap()
            .texels
            .unwrap()
    });
    assert_eq!(retained, original, "shrinking retains atlas capacity");
    let Primitive::ImageQuad { atlas_uv, .. } = scene.paint(id).unwrap().primitives[0] else {
        panic!("dynamic image must remain an image quad");
    };
    assert_eq!((atlas_uv.width, atlas_uv.height), (2.0, 2.0));

    *frame.borrow_mut() = Some(DynamicImageFrame::new(4, 4, vec![0x40; 4 * 4 * 4]));
    revision.set(2);
    poll_dynamic_images(runtime, &mut scene);
    let regrown = runtime.with(|runtime| {
        runtime
            .borrow()
            .dynamic_images
            .get(id)
            .unwrap()
            .texels
            .unwrap()
    });
    assert_eq!(
        regrown, original,
        "regrowth within capacity allocates no shelf"
    );
}

#[test]
fn image_from_png_decodes_and_size_overrides_display() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    // encode a 2×2 RGB PNG in-memory (red, green / blue, white)
    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, 2, 2);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[
            0xff, 0, 0, 0, 0xff, 0, //
            0, 0, 0xff, 0xff, 0xff, 0xff,
        ])
        .unwrap();
    }
    let img = Image::from_png(&png_bytes)
        .expect("decode")
        .size(32.0, 32.0);
    let (scene, _l, _t, _a, id) = build_one(runtime, img);
    let Primitive::ImageQuad { rect, atlas_uv, .. } = scene.paint(id).unwrap().primitives[0] else {
        panic!("expected an ImageQuad");
    };
    // display size overridden; texel rect keeps the true pixel dims
    assert_eq!((rect.width, rect.height), (32.0, 32.0));
    assert_eq!((atlas_uv.width, atlas_uv.height), (2.0, 2.0));
    // RGB expanded to RGBA in the atlas: first texel is opaque red
    let stride = scene.images().width() as usize * 4;
    let base = (atlas_uv.y as usize) * stride + (atlas_uv.x as usize) * 4;
    assert_eq!(
        &scene.images().pixels()[base..base + 4],
        &[0xff, 0, 0, 0xff]
    );
    // garbage bytes are a clean error, not a panic
    assert!(Image::from_png(b"not a png").is_err());
}

#[test]
fn image_placeholder_still_paints_a_solid_box() {
    let runtime_handle = crate::Runtime::new();
    #[allow(unused_variables)]
    let runtime = &runtime_handle;
    let (scene, _l, _t, _a, id) = build_one(runtime, Image::new("photo"));
    let prims = &scene.paint(id).unwrap().primitives;
    assert!(matches!(prims[0], Primitive::SolidRect { .. }));
    assert!(
        scene.images().is_empty(),
        "no atlas bytes for a placeholder"
    );
}
