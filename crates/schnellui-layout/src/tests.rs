use crate::*;

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui_scene::{Rect, Scene, Size, WidgetKind};
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
}
