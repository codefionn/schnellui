use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reset;
    use schnellui_layout::LayoutEngine;
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

    /// The `SolidRect` colour of the primitive at `idx` on a node.
    fn color_of(scene: &Scene, id: WidgetId, idx: usize) -> Color {
        match scene.paint(id).unwrap().primitives[idx] {
            Primitive::SolidRect { color, .. } => color,
            ref p => panic!("expected a SolidRect, got {p:?}"),
        }
    }

    // --- build-time semantics (SOUL §6.1 — no widget without a role) ---

    #[test]
    fn every_selection_widget_reports_its_role_and_kind() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        assert_eq!(TabBar::new().role(), Role::TabList);
        assert_eq!(TabBar::new().kind(), WidgetKind::TabBar);
        assert_eq!(Tab::new("x").role(), Role::Tab);
        assert_eq!(Tab::new("x").kind(), WidgetKind::Tab);
        assert_eq!(List::new().role(), Role::List);
        assert_eq!(List::new().kind(), WidgetKind::List);
        assert_eq!(ListItem::new("x").role(), Role::ListItem);
        assert_eq!(ListItem::new("x").kind(), WidgetKind::ListItem);
    }

    #[test]
    fn tabbar_builds_tablist_role_with_tab_children() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(
            runtime,
            TabBar::new()
                .child(Tab::new("general").selected(true))
                .child(Tab::new("privacy")),
        );
        assert_eq!(scene.node(id).unwrap().kind, WidgetKind::TabBar);
        assert!(scene.node(id).unwrap().kind.is_container());
        assert_eq!(Role::from_u16(scene.a11y(id).unwrap().role), Role::TabList);
        let kids = scene.node(id).unwrap().children.clone();
        assert_eq!(kids.len(), 2);
        let a = scene.a11y(kids[0]).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::Tab);
        assert_eq!(a.name.as_deref(), Some("general"));
        assert!(StateFlags(a.state).contains(StateFlags::SELECTED));
        assert!(ActionFlags(a.actions).contains(ActionFlags::CLICK));
        assert!(ActionFlags(a.actions).contains(ActionFlags::FOCUS));
        assert!(!StateFlags(scene.a11y(kids[1]).unwrap().state).contains(StateFlags::SELECTED));
    }

    #[test]
    fn tabbar_trailing_view_follows_tabs_but_stays_outside_the_tablist() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let added = create_signal(0usize);
        let bar = TabBar::new()
            .gap(4.0)
            .child(Tab::new("general").selected(true).on_close(|| {}))
            .child(Tab::new("privacy"))
            .trailing(
                crate::Button::new("Add tab")
                    .appearance(crate::ButtonAppearance::Ghost)
                    .on_click(move || added.update(|count| *count += 1)),
            );
        assert_eq!(bar.child_count(), 2);
        assert!(bar.has_trailing());

        let (mut scene, mut layout, _text, _atlas, root) = build_one(runtime, bar);
        assert_eq!(scene.node(root).unwrap().kind, WidgetKind::Row);
        assert_eq!(Role::from_u16(scene.a11y(root).unwrap().role), Role::Group);
        let outer_children = scene.node(root).unwrap().children.clone();
        assert_eq!(outer_children.len(), 2);
        let (tablist, add) = (outer_children[0], outer_children[1]);
        assert_eq!(scene.node(tablist).unwrap().kind, WidgetKind::TabBar);
        assert_eq!(
            Role::from_u16(scene.a11y(tablist).unwrap().role),
            Role::TabList
        );
        assert_eq!(scene.node(tablist).unwrap().children.len(), 2);
        assert_eq!(Role::from_u16(scene.a11y(add).unwrap().role), Role::Button);
        assert_eq!(scene.a11y(add).unwrap().name.as_deref(), Some("Add tab"));

        layout.sync_tree(&scene, root);
        layout.compute(
            &mut scene,
            root,
            Size {
                width: 400.0,
                height: 100.0,
            },
        );
        let tabs_rect = scene.layout(tablist).unwrap().rect;
        let add_rect = scene.layout(add).unwrap().rect;
        assert_eq!(add_rect.x, tabs_rect.right() + 4.0);

        assert!(crate::dispatch_click(runtime, &mut scene, add));
        assert_eq!(added.get(), 1);
        let selected_tabs = scene
            .node(tablist)
            .unwrap()
            .children
            .iter()
            .flat_map(|child| {
                let node = scene.node(*child).unwrap();
                if node.kind == WidgetKind::Tab {
                    vec![*child]
                } else {
                    node.children
                        .iter()
                        .copied()
                        .filter(|id| scene.node(*id).unwrap().kind == WidgetKind::Tab)
                        .collect()
                }
            })
            .filter(|id| is_selected(&scene, *id))
            .count();
        assert_eq!(
            selected_tabs, 1,
            "the trailing action must not change selection"
        );
    }

    #[test]
    fn list_builds_list_role_with_item_children() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(
            runtime,
            List::new()
                .child(ListItem::new("inbox").selected(true))
                .child(ListItem::new("archive")),
        );
        assert!(scene.node(id).unwrap().kind.is_container());
        assert_eq!(Role::from_u16(scene.a11y(id).unwrap().role), Role::List);
        let kids = scene.node(id).unwrap().children.clone();
        let a = scene.a11y(kids[0]).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::ListItem);
        assert_eq!(a.name.as_deref(), Some("inbox"));
        assert!(StateFlags(a.state).contains(StateFlags::SELECTED));
    }

    #[test]
    fn tab_paint_carries_bg_indicator_and_glyphs() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, Tab::new("general").selected(true));
        let prims = &scene.paint(id).unwrap().primitives;
        // [0] bg, [1] indicator, then real glyph quads (SOUL §8.1)
        assert!(prims.len() > 2);
        assert_eq!(color_of(&scene, id, 0), crate::Theme::default().selection);
        assert_eq!(color_of(&scene, id, 1), crate::Theme::default().accent);
        assert!(prims[2..]
            .iter()
            .all(|p| matches!(p, Primitive::GlyphQuad { .. })));
        // unselected: white bg, transparent indicator — same primitive list
        let (scene, _l, _t, _a, id) = build_one(runtime, Tab::new("general"));
        assert_eq!(color_of(&scene, id, 0), crate::Theme::default().surface);
        assert_eq!(color_of(&scene, id, 1), Color::TRANSPARENT);
    }

    #[test]
    fn navigation_tab_is_full_width_flat_and_uses_a_left_selection_rail() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(
            runtime,
            Tab::new("overview")
                .appearance(TabAppearance::Navigation)
                .width(180.0),
        );
        let prims = &scene.paint(id).unwrap().primitives;
        let Primitive::SolidRect {
            rect: background,
            color,
            ..
        } = prims[0]
        else {
            panic!("navigation background must be a solid rectangle");
        };
        let Primitive::SolidRect { rect: rail, .. } = prims[1] else {
            panic!("navigation rail must be a solid rectangle");
        };
        assert_eq!(background.width, 180.0);
        assert_eq!(color, Color::TRANSPARENT);
        assert!(rail.height > rail.width, "indicator is a vertical rail");

        let (scene, _l, _t, _a, id) = build_one(
            runtime,
            Tab::new("overview")
                .selected(true)
                .appearance(TabAppearance::Navigation)
                .width(180.0),
        );
        assert_eq!(color_of(&scene, id, 0), crate::Theme::default().selection);
        assert_eq!(color_of(&scene, id, 1), crate::Theme::default().accent);
    }

    // --- input handling: pointer and ActionRequest converge (SOUL §6.3) ---

    #[test]
    fn tab_click_is_exclusive_recolors_in_place_and_fires_handler() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let chosen = create_signal(0usize);
        let (mut scene, _l, _t, _a, bar) = build_one(
            runtime,
            TabBar::new()
                .child(
                    Tab::new("general")
                        .selected(true)
                        .on_select(move || chosen.set(0)),
                )
                .child(Tab::new("privacy").on_select(move || chosen.set(1))),
        );
        let kids = scene.node(bar).unwrap().children.clone();
        let (t0, t1) = (kids[0], kids[1]);
        let prims_before = scene.paint(t1).unwrap().primitives.len();

        scene.clear_dirty();
        // the same inbound path a `Click` ActionRequest takes (SOUL §6.3)
        assert!(crate::dispatch_click(runtime, &mut scene, t1));
        assert_eq!(chosen.get(), 1);
        // exclusivity: t0 cleared, t1 selected
        assert!(!StateFlags(scene.a11y(t0).unwrap().state).contains(StateFlags::SELECTED));
        assert!(StateFlags(scene.a11y(t1).unwrap().state).contains(StateFlags::SELECTED));
        // both are marked paint + a11y dirty
        for t in [t0, t1] {
            assert!(scene.dirty_flags(t).contains(DirtyFlags::PAINT), "{t:?}");
            assert!(scene.dirty_flags(t).contains(DirtyFlags::A11Y), "{t:?}");
        }
        // the toggle recolored in place: same primitive count, colours swapped
        assert_eq!(scene.paint(t1).unwrap().primitives.len(), prims_before);
        assert_eq!(color_of(&scene, t1, 0), crate::Theme::default().selection);
        assert_eq!(color_of(&scene, t1, 1), crate::Theme::default().accent);
        assert_eq!(color_of(&scene, t0, 0), crate::Theme::default().surface);
        assert_eq!(color_of(&scene, t0, 1), Color::TRANSPARENT);
        // layout untouched: a selection toggle never re-measures (SOUL §8.1)
        assert!(!scene.dirty_flags(t1).contains(DirtyFlags::LAYOUT));
    }

    #[test]
    fn navigation_tab_click_restores_transparent_resting_peer() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, _t, _a, bar) = build_one(
            runtime,
            TabBar::new()
                .child(
                    Tab::new("overview")
                        .selected(true)
                        .appearance(TabAppearance::Navigation),
                )
                .child(Tab::new("engine").appearance(TabAppearance::Navigation)),
        );
        let tabs = scene.node(bar).unwrap().children.clone();
        assert!(crate::dispatch_click(runtime, &mut scene, tabs[1]));
        assert_eq!(color_of(&scene, tabs[0], 0), Color::TRANSPARENT);
        assert_eq!(
            color_of(&scene, tabs[1], 0),
            crate::Theme::default().selection
        );
    }

    #[test]
    fn list_item_click_is_exclusive_and_fires_handler() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let count = create_signal(0i32);
        let (mut scene, _l, _t, _a, list) = build_one(
            runtime,
            List::new()
                .child(ListItem::new("inbox").selected(true))
                .child(ListItem::new("archive").on_select(move || count.update(|v| *v += 1))),
        );
        let kids = scene.node(list).unwrap().children.clone();
        let (i0, i1) = (kids[0], kids[1]);
        assert!(crate::dispatch_click(runtime, &mut scene, i1));
        assert_eq!(count.get(), 1);
        assert!(!StateFlags(scene.a11y(i0).unwrap().state).contains(StateFlags::SELECTED));
        assert!(StateFlags(scene.a11y(i1).unwrap().state).contains(StateFlags::SELECTED));
        assert_eq!(color_of(&scene, i1, 0), crate::Theme::default().selection);
        assert_eq!(color_of(&scene, i0, 0), crate::Theme::default().surface);
        // an already-selected item still fires its handler, no state re-dirty
        assert!(crate::dispatch_click(runtime, &mut scene, i1));
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn selected_tab_without_handler_reclick_is_a_no_op() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, _t, _a, bar) = build_one(
            runtime,
            TabBar::new().child(Tab::new("only").selected(true)),
        );
        let tab = scene.node(bar).unwrap().children[0];
        scene.clear_dirty();
        // already selected, no handler → nothing ran, nothing changed
        assert!(!crate::dispatch_click(runtime, &mut scene, tab));
        assert!(scene.dirty_flags(tab).is_empty());
    }

    // --- geometry: tabs measure like buttons; the bar rows them (SOUL §8.1) ---

    #[test]
    fn tabbar_lays_tabs_out_in_a_row() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _t, _a, bar) = build_one(
            runtime,
            TabBar::new()
                .child(Tab::new("general"))
                .child(Tab::new("privacy")),
        );
        layout.sync_tree(&scene, bar);
        layout.compute(
            &mut scene,
            bar,
            Size {
                width: 400.0,
                height: 100.0,
            },
        );
        let kids = scene.node(bar).unwrap().children.clone();
        let (a, b) = (
            scene.layout(kids[0]).unwrap().rect,
            scene.layout(kids[1]).unwrap().rect,
        );
        assert!(a.width > 0.0 && a.height > 0.0);
        assert_eq!(a.y, b.y, "tabs share a row");
        assert!(b.x >= a.x + a.width, "tabs sit side by side");
    }

    #[test]
    fn tab_reordering_is_opt_in_and_reports_final_indices() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        let reordered = Rc::new(RefCell::new(Vec::new()));
        let drag_starts = Rc::new(Cell::new(0));
        let drag_accepted = Rc::new(Cell::new(false));
        let ordinary_drops = Rc::new(Cell::new(0));
        let (mut scene, mut layout, _text, _atlas, bar) = build_one(
            runtime,
            TabBar::new()
                .on_reorder({
                    let reordered = reordered.clone();
                    move |from, to| reordered.borrow_mut().push((from, to))
                })
                .child(
                    Tab::new("one")
                        .on_drag_start({
                            let drag_starts = drag_starts.clone();
                            move || drag_starts.set(drag_starts.get() + 1)
                        })
                        .on_drag_end({
                            let drag_accepted = drag_accepted.clone();
                            move |accepted| drag_accepted.set(accepted)
                        }),
                )
                .child(Tab::new("two"))
                .child(Tab::new("three").on_drop({
                    let ordinary_drops = ordinary_drops.clone();
                    move || ordinary_drops.set(ordinary_drops.get() + 1)
                })),
        );
        layout.sync_tree(&scene, bar);
        layout.compute(
            &mut scene,
            bar,
            Size {
                width: 400.0,
                height: 100.0,
            },
        );
        let tabs = scene.node(bar).unwrap().children.clone();
        let center = |scene: &Scene, id: WidgetId, x_fraction: f32| {
            let rect = scene.layout(id).unwrap().rect;
            Point {
                x: rect.x + rect.width * x_fraction,
                y: rect.y + rect.height * 0.5,
            }
        };
        let from = center(&scene, tabs[0], 0.5);
        let to = center(&scene, tabs[2], 0.75);

        assert_eq!(
            crate::cursor_at(runtime, &scene, from),
            crate::CursorIcon::Grab
        );
        assert!(crate::begin_drag(runtime, &scene, from));
        assert!(crate::update_drag(runtime, &mut scene, to));
        assert_eq!(drag_starts.get(), 1);
        assert_eq!(
            crate::end_drag(runtime, &mut scene, to),
            crate::DragRelease::Drop { accepted: true }
        );
        assert_eq!(&*reordered.borrow(), &[(0, 2)]);
        assert!(drag_accepted.get());
        assert_eq!(
            ordinary_drops.get(),
            0,
            "a local reorder must not also run the tab's ordinary drop action"
        );

        let (scene, mut layout, _text, _atlas, bar) = build_one(
            runtime,
            TabBar::new().child(Tab::new("one")).child(Tab::new("two")),
        );
        let mut scene = scene;
        layout.sync_tree(&scene, bar);
        layout.compute(
            &mut scene,
            bar,
            Size {
                width: 300.0,
                height: 100.0,
            },
        );
        let first = scene.node(bar).unwrap().children[0];
        let point = center(&scene, first, 0.5);
        assert!(!crate::begin_drag(runtime, &scene, point));
    }

    #[test]
    fn reorder_enabled_source_still_drops_outside_its_tab_bar() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use std::cell::Cell;
        use std::rc::Rc;

        let local_reorders = Rc::new(Cell::new(0));
        let external_drops = Rc::new(Cell::new(0));
        let (mut scene, mut layout, _text, _atlas, root) = build_one(
            runtime,
            crate::Row::new()
                .gap(80.0)
                .child(
                    TabBar::new()
                        .on_reorder({
                            let local_reorders = local_reorders.clone();
                            move |_, _| local_reorders.set(local_reorders.get() + 1)
                        })
                        .child(Tab::new("source")),
                )
                .child(TabBar::new().child(Tab::new("external").on_drop({
                    let external_drops = external_drops.clone();
                    move || external_drops.set(external_drops.get() + 1)
                }))),
        );
        layout.sync_tree(&scene, root);
        layout.compute(
            &mut scene,
            root,
            Size {
                width: 500.0,
                height: 100.0,
            },
        );
        let bars = scene.node(root).unwrap().children.clone();
        let source = scene.node(bars[0]).unwrap().children[0];
        let external = scene.node(bars[1]).unwrap().children[0];
        let point = |scene: &Scene, id| {
            let rect = scene.layout(id).unwrap().rect;
            Point {
                x: rect.x + rect.width * 0.5,
                y: rect.y + rect.height * 0.5,
            }
        };
        let from = point(&scene, source);
        let to = point(&scene, external);
        assert!(crate::begin_drag(runtime, &scene, from));
        assert!(crate::update_drag(runtime, &mut scene, to));
        assert_eq!(
            crate::end_drag(runtime, &mut scene, to),
            crate::DragRelease::Drop { accepted: true }
        );
        assert_eq!(local_reorders.get(), 0);
        assert_eq!(external_drops.get(), 1);
    }

    #[test]
    fn unaccepted_clickable_tab_drag_falls_back_to_click_without_cursor_flip() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _text, _atlas, tab) = build_one(
            runtime,
            Tab::new("Changes").on_select(|| {}).on_drag_start(|| {}),
        );
        layout.sync_tree(&scene, tab);
        layout.compute(
            &mut scene,
            tab,
            Size {
                width: 300.0,
                height: 100.0,
            },
        );
        let rect = scene.layout(tab).unwrap().rect;
        let from = Point {
            x: rect.x + rect.width * 0.5,
            y: rect.y + rect.height * 0.5,
        };
        let moved = Point {
            x: from.x + 20.0,
            y: from.y + 20.0,
        };

        assert!(crate::begin_drag(runtime, &scene, from));
        assert!(crate::update_drag(runtime, &mut scene, moved));
        assert_eq!(
            crate::cursor_at(runtime, &scene, moved),
            crate::CursorIcon::Pointer
        );
        assert_eq!(
            crate::end_drag(runtime, &mut scene, moved),
            crate::DragRelease::Click(tab)
        );
    }

    #[test]
    fn closable_tab_builds_an_independent_accessible_close_target() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let closed = create_signal(0usize);
        let selected = create_signal(0usize);
        let (mut scene, mut layout, _text, _atlas, bar) = build_one(
            runtime,
            TabBar::new().child(
                Tab::new("report.rs")
                    .on_select(move || selected.update(|count| *count += 1))
                    .on_close(move || closed.update(|count| *count += 1)),
            ),
        );
        layout.sync_tree(&scene, bar);
        layout.compute(
            &mut scene,
            bar,
            Size {
                width: 400.0,
                height: 100.0,
            },
        );
        crate::reposition_paint(runtime, &mut scene);

        let wrapper = scene.node(bar).unwrap().children[0];
        let children = scene.node(wrapper).unwrap().children.clone();
        let (tab, close) = (children[0], children[1]);
        assert_eq!(Role::from_u16(scene.a11y(tab).unwrap().role), Role::Tab);
        assert_eq!(
            Role::from_u16(scene.a11y(close).unwrap().role),
            Role::Button
        );
        assert_eq!(
            scene.a11y(close).unwrap().name.as_deref(),
            Some("Close report.rs")
        );

        let close_rect = scene.layout(close).unwrap().rect;
        let Primitive::SolidRect {
            rect: tab_surface, ..
        } = scene.paint(tab).unwrap().primitives[0]
        else {
            panic!("tab surface must be a solid rectangle");
        };
        assert_eq!(tab_surface.right(), close_rect.right());
        assert_eq!(
            crate::hit_test(
                runtime,
                &scene,
                Point {
                    x: close_rect.x + close_rect.width * 0.5,
                    y: close_rect.y + close_rect.height * 0.5,
                },
            ),
            Some(close)
        );

        assert!(crate::dispatch_click(runtime, &mut scene, close));
        assert_eq!(closed.get(), 1);
        assert_eq!(selected.get(), 0, "closing must not select the tab");
        assert!(!is_selected(&scene, tab));
    }

    #[test]
    fn tab_context_menu_is_opt_in_and_runs_custom_commands() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let inspected = create_signal(0usize);
        let menu = ContextMenu::new().item(
            ContextMenuItem::new("Inspect")
                .on_select(move || inspected.update(|count| *count += 1)),
        );
        let (mut scene, _layout, mut text, mut atlas, tab) =
            build_one(runtime, Tab::new("report.rs").context_menu(menu));
        assert!(
            ActionFlags(scene.a11y(tab).unwrap().actions).contains(ActionFlags::SHOW_CONTEXT_MENU)
        );
        assert_eq!(crate::context_menu_source(runtime, &scene, tab), Some(tab));
        assert!(crate::open_context_menu(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            tab,
            Point { x: 10.0, y: 10.0 },
            Size {
                width: 400.0,
                height: 300.0,
            },
            1.0,
            false,
        ));
        let root = scene.root().unwrap();
        let menu_root = scene
            .node(root)
            .unwrap()
            .children
            .iter()
            .copied()
            .find(|id| Role::from_u16(scene.a11y(*id).unwrap().role) == Role::Menu)
            .expect("context menu root");
        assert_eq!(
            scene.a11y(menu_root).unwrap().name.as_deref(),
            Some("report.rs menu")
        );
        let item = scene.node(menu_root).unwrap().children[0];
        assert_eq!(
            crate::activate_context_menu_item(runtime, &mut scene, item)
                .unwrap()
                .action,
            crate::ContextMenuAction::Custom
        );
        assert_eq!(inspected.get(), 1);
    }

    #[test]
    fn list_lays_items_out_in_a_column() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _t, _a, list) = build_one(
            runtime,
            List::new()
                .child(ListItem::new("inbox"))
                .child(ListItem::new("archive")),
        );
        layout.sync_tree(&scene, list);
        layout.compute(
            &mut scene,
            list,
            Size {
                width: 400.0,
                height: 400.0,
            },
        );
        let kids = scene.node(list).unwrap().children.clone();
        let (a, b) = (
            scene.layout(kids[0]).unwrap().rect,
            scene.layout(kids[1]).unwrap().rect,
        );
        assert_eq!(a.x, b.x, "items share a column");
        assert!(b.y >= a.y + a.height, "items stack vertically");
    }

    // --- dropdown: trigger semantics, structural open, exclusive options (§8.1) ---

    #[test]
    fn searchable_combobox_filters_options_and_offers_free_text() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _layout, _text, _atlas, wrapper) = build_one(
            runtime,
            ComboBox::new("ap")
                .label("Fruit")
                .open(true)
                .allow_free_text(true)
                .option(DropdownOption::new("Apple"))
                .option(DropdownOption::new("Grape"))
                .option(DropdownOption::new("Banana")),
        );
        assert_eq!(ComboBox::new("").role(), Role::ComboBox);
        assert_eq!(ComboBox::new("").kind(), WidgetKind::TextInput);

        let children = scene.node(wrapper).unwrap().children.clone();
        let field = children[0];
        let semantics = scene.a11y(field).unwrap();
        assert_eq!(Role::from_u16(semantics.role), Role::ComboBox);
        assert_eq!(semantics.name.as_deref(), Some("Fruit"));
        assert_eq!(semantics.value.as_deref(), Some("ap"));
        assert!(StateFlags(semantics.state).contains(StateFlags::EXPANDED));
        assert!(ActionFlags(semantics.actions).contains(ActionFlags::SET_VALUE));

        let options = scene.node(children[1]).unwrap().children.clone();
        let names: Vec<_> = options
            .iter()
            .filter(|id| scene.is_effectively_visible(**id))
            .map(|id| scene.a11y(*id).unwrap().name.as_deref().unwrap())
            .collect();
        assert_eq!(names, ["Apple", "Grape", "Use “ap”"]);
    }

    #[test]
    fn exact_combobox_value_opens_the_complete_suggestion_list() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _layout, _text, _atlas, wrapper) = build_one(
            runtime,
            ComboBox::new("Apple")
                .open(true)
                .option(DropdownOption::new("Apple").selected(true))
                .option(DropdownOption::new("Banana")),
        );
        let popup = scene.node(wrapper).unwrap().children[1];
        assert_eq!(scene.node(popup).unwrap().children.len(), 2);
    }

    #[test]
    fn combobox_trigger_uses_the_shared_toggle_dispatch() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let toggles = create_signal(0i32);
        let (mut scene, _layout, _text, _atlas, wrapper) = build_one(
            runtime,
            ComboBox::new("")
                .label("Fruit")
                .on_toggle(move || toggles.update(|count| *count += 1)),
        );
        let field = scene.node(wrapper).unwrap().children[0];
        assert!(crate::dispatch_click(runtime, &mut scene, field));
        assert_eq!(toggles.get(), 1);
    }

    #[test]
    fn every_dropdown_widget_reports_its_role_and_kind() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        assert_eq!(Dropdown::new("x").role(), Role::ComboBox);
        assert_eq!(Dropdown::new("x").kind(), WidgetKind::Dropdown);
        assert_eq!(DropdownOption::new("x").role(), Role::ListBoxOption);
        assert_eq!(DropdownOption::new("x").kind(), WidgetKind::DropdownOption);
        assert_eq!(
            Dropdown::new("x")
                .option(DropdownOption::new("a"))
                .option(DropdownOption::new("b"))
                .option_count(),
            2
        );
    }

    #[test]
    fn closed_dropdown_builds_trigger_only_with_combobox_semantics() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, wrap) = build_one(
            runtime,
            Dropdown::new("Example")
                .option(DropdownOption::new("Gallery"))
                .option(DropdownOption::new("Images").selected(true)),
        );
        // the wrapper is a plain layout column (Group role)
        assert_eq!(scene.node(wrap).unwrap().kind, WidgetKind::Column);
        assert!(scene.node(wrap).unwrap().kind.is_container());
        // closed: the trigger is the only child — no option nodes in the skeleton
        let kids = scene.node(wrap).unwrap().children.clone();
        assert_eq!(kids.len(), 1);
        let a = scene.a11y(kids[0]).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::ComboBox);
        assert_eq!(a.name.as_deref(), Some("Example"));
        // the accessible value is the *chosen* option's label
        assert_eq!(a.value.as_deref(), Some("Images"));
        assert!(ActionFlags(a.actions).contains(ActionFlags::CLICK));
        assert!(ActionFlags(a.actions).contains(ActionFlags::FOCUS));
        assert!(!StateFlags(a.state).contains(StateFlags::EXPANDED));
    }

    #[test]
    fn open_dropdown_builds_expanded_trigger_and_overlay_popup() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, wrap) = build_one(
            runtime,
            Dropdown::new("Fruit")
                .open(true)
                .option(DropdownOption::new("Apple").selected(true))
                .option(DropdownOption::new("Banana")),
        );
        let kids = scene.node(wrap).unwrap().children.clone();
        assert_eq!(kids.len(), 2, "trigger + the popup column");
        let trigger = scene.a11y(kids[0]).unwrap();
        assert!(StateFlags(trigger.state).contains(StateFlags::EXPANDED));
        // the popup rides the overlay layer (SOUL §3.2 z-order — dialog-like)
        let popup = kids[1];
        assert!(scene.is_overlay(popup));
        assert!(!scene.is_overlay(kids[0]));
        let opts = scene.node(popup).unwrap().children.clone();
        assert_eq!(opts.len(), 2);
        for (i, (name, selected)) in [("Apple", true), ("Banana", false)].iter().enumerate() {
            let a = scene.a11y(opts[i]).unwrap();
            assert_eq!(Role::from_u16(a.role), Role::ListBoxOption);
            assert_eq!(a.name.as_deref(), Some(*name));
            assert_eq!(
                StateFlags(a.state).contains(StateFlags::SELECTED),
                *selected
            );
        }
    }

    #[test]
    fn trigger_paint_carries_surface_caret_lines_and_glyphs() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, wrap) = build_one(
            runtime,
            Dropdown::new("Fruit").option(DropdownOption::new("Apple")),
        );
        let trigger = scene.node(wrap).unwrap().children[0];
        let prims = &scene.paint(trigger).unwrap().primitives;
        // [0] surface, [1][2] the caret chevron, then real glyph quads
        assert!(prims.len() > 3);
        assert_eq!(
            color_of(&scene, trigger, 0),
            crate::Theme::default().surface
        );
        assert!(matches!(prims[1], Primitive::Line { .. }));
        assert!(matches!(prims[2], Primitive::Line { .. }));
        assert!(prims[3..]
            .iter()
            .all(|p| matches!(p, Primitive::GlyphQuad { .. })));
        // open: the surface takes the selection wash — same primitive list
        let (scene, _l, _t, _a, wrap) = build_one(
            runtime,
            Dropdown::new("Fruit")
                .open(true)
                .option(DropdownOption::new("Apple")),
        );
        let trigger = scene.node(wrap).unwrap().children[0];
        assert_eq!(
            color_of(&scene, trigger, 0),
            crate::Theme::default().selection
        );
    }

    #[test]
    fn trigger_click_fires_on_toggle() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let opened = create_signal(0i32);
        let (mut scene, _l, _t, _a, wrap) = build_one(
            runtime,
            Dropdown::new("Fruit")
                .option(DropdownOption::new("Apple"))
                .on_toggle(move || opened.update(|v| *v += 1)),
        );
        let trigger = scene.node(wrap).unwrap().children[0];
        // the same inbound path a `Click` ActionRequest takes (SOUL §6.3)
        assert!(crate::dispatch_click(runtime, &mut scene, trigger));
        assert_eq!(opened.get(), 1);
        // no structural mutation in place: still just the trigger under the wrapper
        assert_eq!(scene.node(wrap).unwrap().children.len(), 1);
    }

    #[test]
    fn option_click_is_exclusive_fires_handler_and_updates_trigger_value() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let chosen = create_signal(String::new());
        let (mut scene, _l, _t, _a, wrap) = build_one(
            runtime,
            Dropdown::new("Fruit")
                .open(true)
                .option(DropdownOption::new("Apple").selected(true))
                .option(
                    DropdownOption::new("Banana")
                        .on_select(move || chosen.set("Banana".to_string())),
                ),
        );
        let kids = scene.node(wrap).unwrap().children.clone();
        let (trigger, popup) = (kids[0], kids[1]);
        let opts = scene.node(popup).unwrap().children.clone();
        let (apple, banana) = (opts[0], opts[1]);
        assert_eq!(scene.a11y(trigger).unwrap().value.as_deref(), Some("Apple"));

        assert!(crate::dispatch_click(runtime, &mut scene, banana));
        assert_eq!(chosen.get(), "Banana");
        // exclusivity: Apple cleared, Banana selected (recolored in place)
        assert!(!StateFlags(scene.a11y(apple).unwrap().state).contains(StateFlags::SELECTED));
        assert!(StateFlags(scene.a11y(banana).unwrap().state).contains(StateFlags::SELECTED));
        assert_eq!(
            color_of(&scene, banana, 0),
            crate::Theme::default().selection
        );
        assert_eq!(color_of(&scene, apple, 0), crate::Theme::default().surface);
        // the trigger's accessible value mirrors the new choice (SOUL §6.1)
        assert_eq!(
            scene.a11y(trigger).unwrap().value.as_deref(),
            Some("Banana")
        );
        // the trigger itself never gains SELECTED (different kind, not a sibling peer)
        assert!(!StateFlags(scene.a11y(trigger).unwrap().state).contains(StateFlags::SELECTED));
    }

    #[test]
    fn outside_interaction_dismisses_open_dropdown() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use crate::{Button, Column};

        let toggles = create_signal(0i32);
        let (mut scene, _l, _t, _a, root) = build_one(
            runtime,
            Column::new()
                .child(
                    Dropdown::new("Fruit")
                        .open(true)
                        .option(DropdownOption::new("Apple"))
                        .on_toggle(move || toggles.update(|count| *count += 1)),
                )
                .child(Button::new("Other")),
        );
        let other = scene.node(root).unwrap().children[1];

        assert!(dismiss_open_dropdowns(runtime, &mut scene, Some(other)));
        assert_eq!(toggles.get(), 1);
    }

    #[test]
    fn interactions_inside_open_dropdown_do_not_dismiss_it() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let toggles = create_signal(0i32);
        let (mut scene, _l, _t, _a, wrapper) = build_one(
            runtime,
            Dropdown::new("Fruit")
                .open(true)
                .option(DropdownOption::new("Apple"))
                .on_toggle(move || toggles.update(|count| *count += 1)),
        );
        let trigger = scene.node(wrapper).unwrap().children[0];
        let popup = scene.node(wrapper).unwrap().children[1];
        let option = scene.node(popup).unwrap().children[0];

        assert!(!dismiss_open_dropdowns(runtime, &mut scene, Some(trigger)));
        assert!(!dismiss_open_dropdowns(runtime, &mut scene, Some(option)));
        assert_eq!(toggles.get(), 0);
    }

    #[test]
    fn blank_space_interaction_dismisses_open_dropdown() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let toggles = create_signal(0i32);
        let (mut scene, _l, _t, _a, _wrapper) = build_one(
            runtime,
            Dropdown::new("Fruit")
                .open(true)
                .option(DropdownOption::new("Apple"))
                .on_toggle(move || toggles.update(|count| *count += 1)),
        );

        assert!(dismiss_open_dropdowns(runtime, &mut scene, None));
        assert_eq!(toggles.get(), 1);
    }

    #[test]
    fn open_dropdown_floats_options_below_the_trigger_without_displacing() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use crate::{Column, Text};
        // the dropdown open, with a sibling text below it in the column
        let build = |open: bool| {
            build_one(
                runtime,
                Column::new()
                    .child(
                        Dropdown::new("Fruit")
                            .open(open)
                            .option(DropdownOption::new("Apple"))
                            .option(DropdownOption::new("Banana")),
                    )
                    .child(Text::new("below")),
            )
        };
        let mut below_y = [0.0f32; 2];
        for (i, open) in [false, true].into_iter().enumerate() {
            let (mut scene, mut layout, _t, _a, col) = build(open);
            layout.sync_tree(&scene, col);
            layout.compute(
                &mut scene,
                col,
                Size {
                    width: 400.0,
                    height: 400.0,
                },
            );
            let kids = scene.node(col).unwrap().children.clone();
            let wrap = kids[0];
            below_y[i] = scene.layout(kids[1]).unwrap().rect.y;
            if open {
                let wkids = scene.node(wrap).unwrap().children.clone();
                let t = scene.layout(wkids[0]).unwrap().rect;
                let opts = scene.node(wkids[1]).unwrap().children.clone();
                let a = scene.layout(opts[0]).unwrap().rect;
                let b = scene.layout(opts[1]).unwrap().rect;
                assert!(t.width > 0.0 && t.height > 0.0);
                assert!(a.y >= t.y + t.height, "first option sits below the trigger");
                assert!(b.y >= a.y + a.height, "options stack vertically");
                // the popup floats OVER the sibling text (dialog-like)
                assert!(
                    a.y < below_y[i] + 1.0,
                    "popup overlaps where the sibling sits"
                );
            }
        }
        // out of flow: opening the popup must not push the sibling down
        assert_eq!(below_y[0], below_y[1]);
    }

    /// The trigger and every option share one width — the widest label's — so the
    /// open popup is a flush-edged opaque panel (no ragged rows for content
    /// beneath to bleed through) and the trigger never resizes with the choice.
    #[test]
    fn open_dropdown_popup_rows_share_the_trigger_width() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _t, _a, wrap) = build_one(
            runtime,
            Dropdown::new("Fruit")
                .open(true)
                .option(DropdownOption::new("Fig"))
                .option(DropdownOption::new("Dragonfruit").selected(true))
                .option(DropdownOption::new("Kiwi")),
        );
        layout.sync_tree(&scene, wrap);
        layout.compute(
            &mut scene,
            wrap,
            Size {
                width: 400.0,
                height: 400.0,
            },
        );
        let wkids = scene.node(wrap).unwrap().children.clone();
        let trigger = scene.layout(wkids[0]).unwrap().rect;
        let opts = scene.node(wkids[1]).unwrap().children.clone();
        for &o in &opts {
            let r = scene.layout(o).unwrap().rect;
            assert_eq!(
                r.width, trigger.width,
                "every option row spans the trigger's width"
            );
            // the painted surface spans the full row, not just the label
            match scene.paint(o).unwrap().primitives[0] {
                Primitive::SolidRect { rect, .. } => assert_eq!(rect.width, r.width),
                ref p => panic!("expected the background SolidRect, got {p:?}"),
            }
        }
    }

    #[test]
    fn hit_test_resolves_the_floating_option_over_covered_content() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use crate::{Column, Text};
        let (mut scene, mut layout, _t, _a, col) = build_one(
            runtime,
            Column::new()
                .child(
                    Dropdown::new("Fruit")
                        .open(true)
                        .option(DropdownOption::new("Apple"))
                        .option(DropdownOption::new("Banana")),
                )
                .child(Text::new("covered by the popup")),
        );
        layout.sync_tree(&scene, col);
        layout.compute(
            &mut scene,
            col,
            Size {
                width: 400.0,
                height: 400.0,
            },
        );
        let kids = scene.node(col).unwrap().children.clone();
        let wrap = kids[0];
        let wkids = scene.node(wrap).unwrap().children.clone();
        let opts = scene.node(wkids[1]).unwrap().children.clone();
        let first = scene.layout(opts[0]).unwrap().rect;
        let text = scene.layout(kids[1]).unwrap().rect;
        // a point inside BOTH the first option and the covered text
        let overlap = Rect::new(first.x.max(text.x), first.y.max(text.y), 0.0, 0.0);
        assert!(
            first.contains(Point {
                x: overlap.x + 1.0,
                y: overlap.y + 1.0
            }) && text.contains(Point {
                x: overlap.x + 1.0,
                y: overlap.y + 1.0
            }),
            "test setup: popup must overlap the text ({first:?} vs {text:?})"
        );
        // the overlay layer wins the pointer (SOUL §3.2 z-order)
        assert_eq!(
            crate::hit_test(
                runtime,
                &scene,
                Point {
                    x: overlap.x + 1.0,
                    y: overlap.y + 1.0
                }
            ),
            Some(opts[0])
        );
    }

    // --- hit-testing: the bar/list is transparent, the tab/item is the target ---

    #[test]
    fn hit_test_resolves_tab_not_tabbar() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _t, _a, bar) =
            build_one(runtime, TabBar::new().child(Tab::new("general")));
        layout.sync_tree(&scene, bar);
        layout.compute(
            &mut scene,
            bar,
            Size {
                width: 400.0,
                height: 100.0,
            },
        );
        let tab = scene.node(bar).unwrap().children[0];
        let r = scene.layout(tab).unwrap().rect;
        let p = Point {
            x: r.x + r.width * 0.5,
            y: r.y + r.height * 0.5,
        };
        assert_eq!(crate::hit_test(runtime, &scene, p), Some(tab));
    }
}
