use crate::layout::*;

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::scene::{DirtyFlags, Rect, Scene, Size, WidgetId, WidgetKind};
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn edge_insets_math() {
        let e = EdgeInsets::all(8.0);
        assert_eq!(e.horizontal(), 16.0);
        assert_eq!(e.vertical(), 16.0);
        let s = EdgeInsets::symmetric(10.0, 4.0);
        assert_eq!(s.left, 10.0);
        assert_eq!(s.top, 4.0);
        assert_eq!(s.horizontal(), 20.0);
        assert_eq!(s.vertical(), 8.0);
    }

    #[test]
    fn container_style_defaults() {
        let cs = ContainerStyle::new(Container::Row);
        assert_eq!(cs.justify, Justify::Start);
        assert_eq!(cs.align, Align::Start);
        assert_eq!(cs.gap, 0.0);
        assert!(cs.fixed_size.is_none());
    }

    #[test]
    fn engine_starts_empty() {
        let e = LayoutEngine::new();
        let _ = e; // constructs a Taffy tree without panicking
    }

    #[test]
    fn measure_fn_is_callable() {
        let mut m: MeasureFn = Box::new(|avail: Size| Size {
            width: avail.width.min(20.0),
            height: 8.0,
        });
        let out = m(Size {
            width: 100.0,
            height: 100.0,
        });
        assert_eq!(
            out,
            Size {
                width: 20.0,
                height: 8.0
            }
        );
    }

    fn fixed_measure(w: f32, h: f32) -> MeasureFn {
        Box::new(move |_avail| Size {
            width: w,
            height: h,
        })
    }

    #[test]
    fn column_stacks_children_at_measured_heights() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let c0 = scene.insert(WidgetKind::Text, Some(root));
        let c1 = scene.insert(WidgetKind::Text, Some(root));
        let c2 = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Column);
        cs.fixed_size = Some(Size {
            width: 100.0,
            height: 100.0,
        });
        cs.align = Align::Start; // keep children's measured cross size, don't stretch
        eng.set_container(root, cs);
        eng.set_measure(c0, fixed_measure(30.0, 10.0));
        eng.set_measure(c1, fixed_measure(30.0, 10.0));
        eng.set_measure(c2, fixed_measure(30.0, 10.0));

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 100.0,
                height: 100.0,
            },
        );

        let b0 = scene.layout(c0).unwrap().rect;
        let b1 = scene.layout(c1).unwrap().rect;
        let b2 = scene.layout(c2).unwrap().rect;

        // Stacked top-to-bottom at their measured heights, no gaps.
        assert_eq!(b0, Rect::new(0.0, 0.0, 30.0, 10.0));
        assert_eq!(b1, Rect::new(0.0, 10.0, 30.0, 10.0));
        assert_eq!(b2, Rect::new(0.0, 20.0, 30.0, 10.0));
    }

    fn stack_with(justify: Justify, align: Align) -> (Scene, WidgetId) {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Stack, None);
        scene.set_root(root);
        let icon = scene.insert(WidgetKind::Image, Some(root));
        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Stack);
        cs.fixed_size = Some(Size {
            width: 30.0,
            height: 30.0,
        });
        cs.justify = justify;
        cs.align = align;
        eng.set_container(root, cs);
        eng.set_measure(icon, fixed_measure(20.0, 20.0));
        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 30.0,
                height: 30.0,
            },
        );
        (scene, icon)
    }

    #[test]
    fn stack_fills_children_by_default() {
        let (scene, icon) = stack_with(Justify::Start, Align::Start);
        assert_eq!(
            scene.layout(icon).unwrap().rect,
            Rect::new(0.0, 0.0, 30.0, 30.0)
        );
    }

    #[test]
    fn stack_centers_children_at_their_own_size() {
        let (scene, icon) = stack_with(Justify::Center, Align::Center);
        assert_eq!(
            scene.layout(icon).unwrap().rect,
            Rect::new(5.0, 5.0, 20.0, 20.0)
        );
        let (scene, icon) = stack_with(Justify::End, Align::Start);
        assert_eq!(
            scene.layout(icon).unwrap().rect,
            Rect::new(10.0, 0.0, 20.0, 30.0)
        );
    }

    #[test]
    fn column_gap_spaces_children() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let c0 = scene.insert(WidgetKind::Text, Some(root));
        let c1 = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Column);
        cs.fixed_size = Some(Size {
            width: 50.0,
            height: 100.0,
        });
        cs.align = Align::Start;
        cs.gap = 6.0;
        eng.set_container(root, cs);
        eng.set_measure(c0, fixed_measure(20.0, 10.0));
        eng.set_measure(c1, fixed_measure(20.0, 10.0));

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 50.0,
                height: 100.0,
            },
        );

        assert_eq!(scene.layout(c0).unwrap().rect.y, 0.0);
        // second child pushed down by first height + gap.
        assert_eq!(scene.layout(c1).unwrap().rect.y, 16.0);
    }

    #[test]
    fn pad_insets_offset_child_and_shrink_content() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Pad, None);
        scene.set_root(root);
        let child = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Pad(EdgeInsets::all(8.0)));
        cs.fixed_size = Some(Size {
            width: 100.0,
            height: 100.0,
        });
        eng.set_container(root, cs);
        eng.set_measure(child, fixed_measure(20.0, 10.0));

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 100.0,
                height: 100.0,
            },
        );

        let rootbox = *scene.layout(root).unwrap();
        assert_eq!(rootbox.rect, Rect::new(0.0, 0.0, 100.0, 100.0));
        // content box is the outer rect minus the 8px insets on every side.
        assert_eq!(rootbox.content, Rect::new(8.0, 8.0, 84.0, 84.0));

        let cb = scene.layout(child).unwrap().rect;
        // child sits at the content-box origin, stretched across the content width.
        assert_eq!(cb.x, 8.0);
        assert_eq!(cb.y, 8.0);
        assert_eq!(cb.width, 84.0);
        assert_eq!(cb.height, 10.0);
    }

    #[test]
    fn measure_closure_gets_definite_constraints() {
        let seen: Rc<Cell<Size>> = Rc::new(Cell::new(Size {
            width: -1.0,
            height: -1.0,
        }));
        let seen_w = seen.clone();

        let mut scene = Scene::new();
        let leaf = scene.insert(WidgetKind::Text, None);
        scene.set_root(leaf);

        let mut eng = LayoutEngine::new();
        eng.set_measure(
            leaf,
            Box::new(move |avail: Size| {
                seen_w.set(avail);
                Size {
                    width: avail.width.min(50.0),
                    height: 30.0,
                }
            }),
        );

        eng.sync_tree(&scene, leaf);
        eng.compute(
            &mut scene,
            leaf,
            Size {
                width: 200.0,
                height: 120.0,
            },
        );

        // The leaf was offered exactly the viewport constraint.
        assert_eq!(
            seen.get(),
            Size {
                width: 200.0,
                height: 120.0
            }
        );
        // ...and its returned intrinsic size became its box.
        assert_eq!(
            scene.layout(leaf).unwrap().rect,
            Rect::new(0.0, 0.0, 50.0, 30.0)
        );
    }

    #[test]
    fn dirty_subtree_relayout_leaves_clean_siblings_untouched() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Row, None);
        scene.set_root(root);
        let left = scene.insert(WidgetKind::Column, Some(root));
        let right = scene.insert(WidgetKind::Column, Some(root));
        let lt = scene.insert(WidgetKind::Text, Some(left));
        let rt = scene.insert(WidgetKind::Text, Some(right));

        let mut eng = LayoutEngine::new();
        let mut rootcs = ContainerStyle::new(Container::Row);
        rootcs.fixed_size = Some(Size {
            width: 200.0,
            height: 100.0,
        });
        rootcs.align = Align::Start;
        eng.set_container(root, rootcs);
        eng.set_container(left, ContainerStyle::new(Container::Column));
        eng.set_container(right, ContainerStyle::new(Container::Column));
        eng.set_measure(lt, fixed_measure(30.0, 10.0));
        eng.set_measure(rt, fixed_measure(40.0, 10.0));

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 200.0,
                height: 100.0,
            },
        );

        let right_before = *scene.layout(rt).unwrap();
        let left_before = *scene.layout(lt).unwrap();
        assert_eq!(left_before.rect.height, 10.0);

        // The left text grows; mark its measure dirty and relayout ONLY the left
        // subtree (SOUL §8.1 smallest-affected-subtree).
        eng.set_measure(lt, fixed_measure(30.0, 50.0));
        eng.compute(
            &mut scene,
            left,
            Size {
                width: 100.0,
                height: 100.0,
            },
        );

        let right_after = *scene.layout(rt).unwrap();
        let left_after = *scene.layout(lt).unwrap();

        // The clean sibling's box is byte-for-byte unchanged...
        assert_eq!(right_before, right_after);
        // ...while the dirty subtree picked up the new measured height.
        assert_eq!(left_after.rect.height, 50.0);
        assert_ne!(left_before.rect.height, left_after.rect.height);
    }

    #[test]
    fn flex_grow_shares_free_space_proportionally() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Row, None);
        scene.set_root(root);
        let a = scene.insert(WidgetKind::Text, Some(root));
        let b = scene.insert(WidgetKind::Text, Some(root));
        let c = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Row);
        cs.fixed_size = Some(Size {
            width: 300.0,
            height: 20.0,
        });
        eng.set_container(root, cs);
        eng.set_measure(a, fixed_measure(30.0, 10.0));
        eng.set_measure(b, fixed_measure(30.0, 10.0));
        eng.set_measure(c, fixed_measure(60.0, 10.0));
        // a and b flex from a zero basis at weights 1:2; c keeps its measured 60.
        eng.set_flex(
            a,
            FlexChild {
                grow: Some(1.0),
                basis: Some(0.0),
                ..FlexChild::default()
            },
        );
        eng.set_flex(
            b,
            FlexChild {
                grow: Some(2.0),
                basis: Some(0.0),
                ..FlexChild::default()
            },
        );

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 300.0,
                height: 20.0,
            },
        );

        // free space = 300 − 60 = 240, split 1:2 → 80 and 160.
        assert_eq!(scene.layout(a).unwrap().rect.width, 80.0);
        assert_eq!(scene.layout(b).unwrap().rect.width, 160.0);
        assert_eq!(scene.layout(c).unwrap().rect.width, 60.0);
        assert_eq!(scene.layout(c).unwrap().rect.x, 240.0);
    }

    #[test]
    fn flex_basis_and_shrink_absorb_overflow() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Row, None);
        scene.set_root(root);
        let a = scene.insert(WidgetKind::Text, Some(root));
        let b = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Row);
        cs.fixed_size = Some(Size {
            width: 100.0,
            height: 20.0,
        });
        eng.set_container(root, cs);
        eng.set_measure(a, fixed_measure(10.0, 10.0));
        eng.set_measure(b, fixed_measure(10.0, 10.0));
        // both start from a 100px basis in a 100px row → 100px overflow, shrunk
        // equally (weight 1:1) → 50px each.
        let f = FlexChild {
            shrink: Some(1.0),
            basis: Some(100.0),
            ..FlexChild::default()
        };
        eng.set_flex(a, f);
        eng.set_flex(b, f);

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 100.0,
                height: 20.0,
            },
        );

        assert_eq!(scene.layout(a).unwrap().rect.width, 50.0);
        assert_eq!(scene.layout(b).unwrap().rect.width, 50.0);
    }

    #[test]
    fn max_width_clamps_a_grown_child() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Row, None);
        scene.set_root(root);
        let a = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Row);
        cs.fixed_size = Some(Size {
            width: 300.0,
            height: 20.0,
        });
        eng.set_container(root, cs);
        eng.set_measure(a, fixed_measure(30.0, 10.0));
        eng.set_flex(
            a,
            FlexChild {
                grow: Some(1.0),
                max_width: Some(120.0),
                ..FlexChild::default()
            },
        );

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 300.0,
                height: 20.0,
            },
        );

        // grow wants all 300px; the max clamps the resolved size.
        assert_eq!(scene.layout(a).unwrap().rect.width, 120.0);
    }

    #[test]
    fn container_minimum_size_clamps_content_sized_axes() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let child = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut style = ContainerStyle::new(Container::Column);
        style.min_width = Some(80.0);
        style.min_height = Some(40.0);
        eng.set_container(root, style);
        eng.set_measure(child, fixed_measure(20.0, 10.0));

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 200.0,
                height: 100.0,
            },
        );

        let rect = scene.layout(root).unwrap().rect;
        assert_eq!(rect.width, 80.0);
        assert_eq!(rect.height, 40.0);
    }

    #[test]
    fn container_minimum_size_wins_over_smaller_definite_size() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Row, None);
        scene.set_root(root);

        let mut eng = LayoutEngine::new();
        let mut style = ContainerStyle::new(Container::Row);
        style.fixed_size = Some(Size {
            width: 20.0,
            height: 10.0,
        });
        style.min_width = Some(60.0);
        style.min_height = Some(30.0);
        eng.set_container(root, style);

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 200.0,
                height: 100.0,
            },
        );

        let rect = scene.layout(root).unwrap().rect;
        assert_eq!(rect.width, 60.0);
        assert_eq!(rect.height, 30.0);
    }

    #[test]
    fn wrap_flows_overflow_onto_next_line() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Row, None);
        scene.set_root(root);
        let a = scene.insert(WidgetKind::Text, Some(root));
        let b = scene.insert(WidgetKind::Text, Some(root));
        let c = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Row);
        // definite width only (per-axis): the height must derive from the number
        // of wrapped lines.
        cs.width = Some(100.0);
        cs.wrap = true;
        eng.set_container(root, cs);
        for id in [a, b, c] {
            eng.set_measure(id, fixed_measure(40.0, 10.0));
        }

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 100.0,
                height: 100.0,
            },
        );

        // two 40px children fit the 100px line; the third wraps to a second line.
        assert_eq!(scene.layout(a).unwrap().rect.y, 0.0);
        assert_eq!(scene.layout(b).unwrap().rect.y, 0.0);
        let c_box = scene.layout(c).unwrap().rect;
        assert_eq!(c_box.x, 0.0);
        assert_eq!(c_box.y, 10.0);
        // the row's height is the two line boxes, not a fixed size.
        assert_eq!(scene.layout(root).unwrap().rect.height, 20.0);
    }

    #[test]
    fn fill_root_tracks_the_viewport_across_resizes() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let leaf = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Column);
        cs.fill = true;
        eng.set_container(root, cs);
        eng.set_measure(leaf, fixed_measure(30.0, 10.0));

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 200.0,
                height: 120.0,
            },
        );
        // the filled root IS the viewport, not its 30×10 content.
        assert_eq!(
            scene.layout(root).unwrap().rect,
            Rect::new(0.0, 0.0, 200.0, 120.0)
        );

        // a "window resize": same tree, new available space → root re-derives.
        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 320.0,
                height: 80.0,
            },
        );
        assert_eq!(
            scene.layout(root).unwrap().rect,
            Rect::new(0.0, 0.0, 320.0, 80.0)
        );
    }

    #[test]
    fn fill_nested_takes_the_parent_content_box() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Pad, None);
        scene.set_root(root);
        let inner = scene.insert(WidgetKind::Column, Some(root));

        let mut eng = LayoutEngine::new();
        let mut pad = ContainerStyle::new(Container::Pad(EdgeInsets::all(10.0)));
        pad.fixed_size = Some(Size {
            width: 100.0,
            height: 100.0,
        });
        eng.set_container(root, pad);
        let mut cs = ContainerStyle::new(Container::Column);
        cs.fill = true;
        eng.set_container(inner, cs);

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 100.0,
                height: 100.0,
            },
        );
        // 100% of the padded parent's 80×80 content box, at the content origin.
        assert_eq!(
            scene.layout(inner).unwrap().rect,
            Rect::new(10.0, 10.0, 80.0, 80.0)
        );
    }

    #[test]
    fn filled_wrap_row_reflows_when_the_viewport_changes() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Row, None);
        scene.set_root(root);
        let a = scene.insert(WidgetKind::Text, Some(root));
        let b = scene.insert(WidgetKind::Text, Some(root));
        let c = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Row);
        cs.fill = true;
        cs.wrap = true;
        eng.set_container(root, cs);
        for id in [a, b, c] {
            eng.set_measure(id, fixed_measure(40.0, 10.0));
        }

        // narrow viewport: only two 40px children per line → the third wraps.
        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(scene.layout(c).unwrap().rect.y, 10.0);

        // widen the viewport (the windowed-resize path): all three fit one line.
        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 200.0,
                height: 100.0,
            },
        );
        assert_eq!(scene.layout(c).unwrap().rect.y, 0.0);
        assert_eq!(scene.layout(c).unwrap().rect.x, 80.0);
    }

    #[test]
    fn flex_reweights_a_spacer_without_erasing_its_grow() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Row, None);
        scene.set_root(root);
        let s1 = scene.insert(WidgetKind::Spacer, Some(root));
        let s2 = scene.insert(WidgetKind::Spacer, Some(root));

        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Row);
        cs.fixed_size = Some(Size {
            width: 300.0,
            height: 20.0,
        });
        eng.set_container(root, cs);
        eng.set_container(s1, ContainerStyle::new(Container::Spacer));
        eng.set_container(s2, ContainerStyle::new(Container::Spacer));
        // an empty FlexChild leaves the Spacer's built-in grow=1 intact; an
        // explicit grow re-weights the second spacer to twice the share.
        eng.set_flex(s1, FlexChild::default());
        eng.set_flex(
            s2,
            FlexChild {
                grow: Some(2.0),
                ..FlexChild::default()
            },
        );

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 300.0,
                height: 20.0,
            },
        );

        assert_eq!(scene.layout(s1).unwrap().rect.width, 100.0);
        assert_eq!(scene.layout(s2).unwrap().rect.width, 200.0);
    }

    #[test]
    fn parent_query_uses_the_parent_content_box_and_removes_layout_space() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Pad, None);
        scene.set_root(root);
        let child = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        let mut parent = ContainerStyle::new(Container::Pad(EdgeInsets::all(20.0)));
        parent.fixed_size = Some(Size {
            width: 400.0,
            height: 100.0,
        });
        eng.set_container(root, parent);
        eng.set_measure(child, fixed_measure(80.0, 20.0));
        eng.set_responsive(child, ResponsiveQuery::parent().max_width(px(320.0)));

        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 800.0,
                height: 600.0,
            },
        );
        // The 400px parent has a 360px content box after padding.
        assert!(!scene.is_visible(child));
        assert!(scene.layout(child).unwrap().rect.is_empty());

        parent.fixed_size = Some(Size {
            width: 300.0,
            height: 100.0,
        });
        eng.set_container(root, parent);
        eng.sync_tree(&scene, root);
        eng.compute(
            &mut scene,
            root,
            Size {
                width: 800.0,
                height: 600.0,
            },
        );
        assert!(scene.is_visible(child));
        assert_eq!(scene.layout(child).unwrap().rect.width, 260.0);
    }

    #[test]
    fn taffy_node_tracks_sync() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let child = scene.insert(WidgetKind::Text, Some(root));

        let mut eng = LayoutEngine::new();
        assert!(eng.taffy_node(root).is_none());
        eng.set_measure(child, fixed_measure(10.0, 10.0));
        eng.sync_tree(&scene, root);
        assert!(eng.taffy_node(root).is_some());
        assert!(eng.taffy_node(child).is_some());
    }

    /// root(fixed 400x400) -> [top(t1,t2), a(a1,a2), b(b1,b2)], every group a
    /// start-aligned column and every leaf a 30x10 fixed measure.
    struct Fixture {
        scene: Scene,
        eng: LayoutEngine,
        root: WidgetId,
        top: [WidgetId; 3],
        a: [WidgetId; 3],
        b: [WidgetId; 3],
    }

    fn fixture() -> Fixture {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let mut eng = LayoutEngine::new();
        let mut cs = ContainerStyle::new(Container::Column);
        cs.fixed_size = Some(Size {
            width: 400.0,
            height: 400.0,
        });
        cs.align = Align::Start;
        eng.set_container(root, cs);
        let group = |scene: &mut Scene, eng: &mut LayoutEngine| {
            let g = scene.insert(WidgetKind::Column, Some(root));
            let mut gcs = ContainerStyle::new(Container::Column);
            gcs.align = Align::Start;
            eng.set_container(g, gcs);
            let l1 = scene.insert(WidgetKind::Text, Some(g));
            let l2 = scene.insert(WidgetKind::Text, Some(g));
            eng.set_measure(l1, fixed_measure(30.0, 10.0));
            eng.set_measure(l2, fixed_measure(30.0, 10.0));
            [g, l1, l2]
        };
        let top = group(&mut scene, &mut eng);
        let a = group(&mut scene, &mut eng);
        let b = group(&mut scene, &mut eng);
        eng.sync_tree(&scene, root);
        let avail = Size {
            width: 400.0,
            height: 400.0,
        };
        eng.compute(&mut scene, root, avail);
        Fixture {
            scene,
            eng,
            root,
            top,
            a,
            b,
        }
    }

    const AVAIL: Size = Size {
        width: 400.0,
        height: 400.0,
    };

    #[test]
    fn first_layout_reports_every_node_as_changed() {
        let f = fixture();
        assert_eq!(f.scene.geometry_changed().len(), 10);
    }

    #[test]
    fn geometry_changed_names_only_the_moved_subtree() {
        let mut f = fixture();
        f.scene.clear_geometry_changed();
        f.scene.clear_dirty();

        // Growing a1 makes group a taller, which pushes group b (and its leaves) down.
        f.eng.set_measure(f.a[1], fixed_measure(30.0, 25.0));
        f.eng.compute(&mut f.scene, f.root, AVAIL);

        let mut changed: Vec<WidgetId> = f.scene.geometry_changed().to_vec();
        changed.sort();
        changed.dedup();
        let mut expected = vec![f.a[0], f.a[1], f.a[2], f.b[0], f.b[1], f.b[2]];
        expected.sort();
        assert_eq!(changed, expected);
        for untouched in [f.root, f.top[0], f.top[1], f.top[2]] {
            assert!(
                !f.scene.geometry_changed().contains(&untouched),
                "node above the change must not be reported"
            );
        }
        assert_eq!(f.scene.layout(f.b[1]).unwrap().rect.y, 20.0 + 25.0 + 10.0);
    }

    #[test]
    fn unchanged_layout_reports_nothing() {
        let mut f = fixture();
        f.scene.clear_geometry_changed();
        f.eng.set_measure(f.a[1], fixed_measure(30.0, 10.0));
        f.eng.compute(&mut f.scene, f.root, AVAIL);
        assert!(f.scene.geometry_changed().is_empty());
    }

    #[test]
    fn deep_change_under_unchanged_ancestors_is_still_written() {
        // A fixed-size ancestor keeps its own placement while a leaf below it
        // changes: the walk must still reach the leaf.
        let mut f = fixture();
        let mut cs = ContainerStyle::new(Container::Column);
        cs.align = Align::Start;
        cs.fixed_size = Some(Size {
            width: 100.0,
            height: 100.0,
        });
        f.eng.set_container(f.a[0], cs);
        f.eng.sync_tree(&f.scene, f.root);
        f.eng.compute(&mut f.scene, f.root, AVAIL);
        f.scene.clear_geometry_changed();
        f.eng.set_measure(f.a[2], fixed_measure(50.0, 10.0));
        f.eng.compute(&mut f.scene, f.root, AVAIL);
        assert_eq!(f.scene.geometry_changed(), &[f.a[2]]);
        assert_eq!(f.scene.layout(f.a[2]).unwrap().rect.width, 50.0);
    }

    #[test]
    fn subtree_relayout_keeps_invalidations_outside_the_subtree() {
        // An invalidation outside a subtree relayout must survive it, so the next
        // full pass still descends past unchanged ancestors and writes the box.
        let mut f = fixture();
        let mut cs = ContainerStyle::new(Container::Column);
        cs.align = Align::Start;
        cs.fixed_size = Some(Size {
            width: 100.0,
            height: 100.0,
        });
        f.eng.set_container(f.a[0], cs);
        f.eng.sync_tree(&f.scene, f.root);
        f.eng.compute(&mut f.scene, f.root, AVAIL);
        f.eng.set_measure(f.a[2], fixed_measure(50.0, 10.0));
        f.eng.compute(&mut f.scene, f.top[0], AVAIL);
        f.eng.compute(&mut f.scene, f.root, AVAIL);
        assert_eq!(f.scene.layout(f.a[2]).unwrap().rect.width, 50.0);
    }

    #[test]
    fn moved_node_is_paint_dirty() {
        let mut f = fixture();
        f.scene.clear_dirty();
        let old = f.scene.layout(f.b[1]).unwrap().rect;

        f.eng.set_measure(f.a[1], fixed_measure(30.0, 25.0));
        f.eng.compute(&mut f.scene, f.root, AVAIL);

        let new = f.scene.layout(f.b[1]).unwrap().rect;
        assert_ne!(old, new);
        let flags = f.scene.dirty_flags(f.b[1]);
        assert!(flags.contains(DirtyFlags::PAINT));
        assert!(
            !flags.contains(DirtyFlags::LAYOUT),
            "a layout result is not a relayout request"
        );
        assert!(f.scene.paint_dirty().contains(&f.b[1]));
        // The top group did not move and stays clean.
        assert!(f.scene.dirty_flags(f.top[1]).is_empty());
    }

    #[test]
    fn layout_writes_do_not_bump_the_structural_revision() {
        let mut f = fixture();
        let revision = f.scene.render_revision();
        let key = f.scene.render_key();
        f.eng.set_measure(f.a[1], fixed_measure(30.0, 25.0));
        f.eng.compute(&mut f.scene, f.root, AVAIL);
        assert_eq!(f.scene.render_revision(), revision);
        assert_ne!(
            f.scene.render_key(),
            key,
            "renderer caches must still see the move"
        );
    }

    #[test]
    fn dyn_measure_hook_skips_leaves_that_are_not_width_aware() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let button = scene.insert(WidgetKind::Button, Some(root));
        let text = scene.insert(WidgetKind::Text, Some(root));
        let mut eng = LayoutEngine::new();
        eng.set_measure(button, fixed_measure(40.0, 12.0));
        eng.set_measure(text, fixed_measure(1.0, 1.0));
        eng.set_width_aware(text);
        eng.sync_tree(&scene, root);

        let mut asked: Vec<WidgetId> = Vec::new();
        eng.compute_with(&mut scene, root, AVAIL, &mut |id, _| {
            asked.push(id);
            Some(Size {
                width: 70.0,
                height: 14.0,
            })
        });
        assert!(!asked.contains(&button), "hook must not see a Button");
        assert!(asked.contains(&text));
        // The button kept its registered fixed measure; the flagged leaf took the hook.
        assert_eq!(scene.layout(button).unwrap().rect.width, 40.0);
        assert_eq!(scene.layout(text).unwrap().rect.width, 70.0);
    }
}
