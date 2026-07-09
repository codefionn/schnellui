use crate::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_union_treats_empty_as_identity() {
        let a = Rect::ZERO;
        let b = Rect::new(10.0, 10.0, 5.0, 5.0);
        assert_eq!(a.union(&b), b);
        assert_eq!(b.union(&a), b);
    }

    #[test]
    fn rect_union_and_intersect() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 15.0, 15.0));
        assert_eq!(a.intersect(&b), Rect::new(5.0, 5.0, 5.0, 5.0));
        let c = Rect::new(100.0, 100.0, 1.0, 1.0);
        assert!(a.intersect(&c).is_empty());
    }

    #[test]
    fn rect_contains() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point { x: 5.0, y: 5.0 }));
        assert!(!r.contains(Point { x: 10.0, y: 5.0 })); // half-open
    }

    #[test]
    fn dirty_flags_orthogonal() {
        let mut d = DirtyFlags::NONE;
        d.insert(DirtyFlags::PAINT);
        assert!(d.contains(DirtyFlags::PAINT));
        assert!(!d.contains(DirtyFlags::LAYOUT));
        d.insert(DirtyFlags::A11Y);
        assert!(d.contains(DirtyFlags::A11Y));
        assert!(d.contains(DirtyFlags::PAINT));
    }

    #[test]
    fn tree_insert_parents_and_marks_paint_damage() {
        let mut s = Scene::new();
        let root = s.insert(WidgetKind::Column, None);
        s.set_root(root);
        let child = s.insert(WidgetKind::Text, Some(root));
        assert_eq!(s.node(root).unwrap().children.as_slice(), &[child]);
        s.set_layout(
            child,
            LayoutBox {
                rect: Rect::new(0.0, 0.0, 20.0, 8.0),
                content: Rect::ZERO,
            },
        );
        s.mark_dirty(child, DirtyFlags::PAINT);
        assert_eq!(s.damage(), Rect::new(0.0, 0.0, 20.0, 8.0));
        s.clear_dirty();
        assert_eq!(s.damage(), Rect::ZERO);
        assert!(s.dirty_flags(child).is_empty());
    }

    #[test]
    fn a11y_dirty_tracked_once() {
        let mut s = Scene::new();
        let n = s.insert(WidgetKind::Button, None);
        s.mark_dirty(n, DirtyFlags::A11Y);
        s.mark_dirty(n, DirtyFlags::A11Y);
        assert_eq!(s.a11y_dirty(), &[n]);
    }

    #[test]
    fn visibility_change_invalidates_retained_renderer_traversal() {
        let mut scene = Scene::new();
        let terminal = scene.insert(WidgetKind::TerminalGrid, None);
        let visible_revision = scene.render_revision();

        scene.set_visible(terminal, false);
        let hidden_revision = scene.render_revision();
        assert_ne!(hidden_revision, visible_revision);

        scene.set_visible(terminal, false);
        assert_eq!(scene.render_revision(), hidden_revision);

        scene.set_visible(terminal, true);
        assert_ne!(scene.render_revision(), hidden_revision);
    }

    #[test]
    fn fresh_scenes_have_distinct_renderer_cache_keys() {
        let first = Scene::new();
        let second = Scene::new();

        assert_eq!(first.render_revision(), second.render_revision());
        assert_ne!(first.render_key(), second.render_key());
    }

    #[test]
    fn kind_container_classification() {
        assert!(WidgetKind::Column.is_container());
        assert!(!WidgetKind::Button.is_container());
    }

    // --- property-mutation channel routing (SOUL §3.2, §8.1) ---

    /// A painted leaf under a root, with a layout box and one solid-rect fragment.
    /// Returned already-clean (dirty channels reset) so tests start from a settled
    /// frame — mount is the "first frame may allocate" phase (SOUL §4).
    fn painted_node() -> (Scene, WidgetId, Rect) {
        let mut s = Scene::new();
        let root = s.insert(WidgetKind::Column, None);
        s.set_root(root);
        let n = s.insert(WidgetKind::Button, Some(root));
        let rect = Rect::new(4.0, 6.0, 20.0, 10.0);
        s.set_layout(
            n,
            LayoutBox {
                rect,
                content: Rect::ZERO,
            },
        );
        s.replace_primitives(
            n,
            [Primitive::SolidRect {
                rect,
                color: Color::WHITE,
                corner_radius: 0.0,
            }],
        );
        s.clear_dirty();
        (s, n, rect)
    }

    #[test]
    fn set_color_dirties_paint_only_and_damage_equals_rect() {
        let (mut s, n, rect) = painted_node();
        s.set_color(n, Color::BLACK);
        let f = s.dirty_flags(n);
        assert!(f.contains(DirtyFlags::PAINT));
        assert!(!f.contains(DirtyFlags::LAYOUT), "color must not relayout");
        assert!(!f.contains(DirtyFlags::A11Y), "color is not semantic");
        assert!(s.layout_dirty().is_empty());
        assert!(s.a11y_dirty().is_empty());
        // damage rect equals that node's rect.
        assert_eq!(s.damage(), rect);
    }

    #[test]
    fn idempotent_set_color_stays_clean() {
        let (mut s, n, _rect) = painted_node();
        s.set_color(n, Color::WHITE); // already white → no-op, no damage
        assert!(s.dirty_flags(n).is_empty());
        assert_eq!(s.damage(), Rect::ZERO);
    }

    #[test]
    fn clean_frame_yields_empty_damage() {
        let (mut s, n, _rect) = painted_node();
        // Settled: no mutation since the helper's clear_dirty.
        assert_eq!(s.damage(), Rect::ZERO);
        assert!(s.dirty_flags(n).is_empty());
        // A mutate → present → clear cycle returns to empty.
        s.set_color(n, Color::BLACK);
        assert!(!s.damage().is_empty());
        s.clear_dirty();
        assert_eq!(s.damage(), Rect::ZERO);
        assert!(s.a11y_dirty().is_empty());
        assert!(s.layout_dirty().is_empty());
    }

    #[test]
    fn set_rect_damages_old_and_new_paint_only() {
        let (mut s, n, old) = painted_node();
        let new = Rect::new(40.0, 6.0, 20.0, 10.0);
        s.set_rect(n, new);
        assert_eq!(s.layout(n).unwrap().rect, new);
        // A move must erase the old and paint the new → union of both.
        assert_eq!(s.damage(), old.union(&new));
        let f = s.dirty_flags(n);
        assert!(f.contains(DirtyFlags::PAINT));
        assert!(
            !f.contains(DirtyFlags::LAYOUT),
            "writing a rect is not a relayout"
        );
        assert!(s.layout_dirty().is_empty());
    }

    #[test]
    fn set_a11y_value_dirties_a11y_only_no_pixels() {
        let (mut s, n, _rect) = painted_node();
        s.set_a11y_value(n, Some("42".to_string()));
        let f = s.dirty_flags(n);
        assert!(f.contains(DirtyFlags::A11Y));
        assert!(
            !f.contains(DirtyFlags::PAINT),
            "semantic value change paints nothing"
        );
        assert!(!f.contains(DirtyFlags::LAYOUT));
        assert_eq!(s.a11y_dirty(), &[n]);
        assert_eq!(s.damage(), Rect::ZERO);
        assert_eq!(s.a11y(n).unwrap().value.as_deref(), Some("42"));
    }

    #[test]
    fn set_a11y_state_dirties_a11y_only() {
        let (mut s, n, _rect) = painted_node();
        s.set_a11y_state(n, 0b1);
        assert!(s.dirty_flags(n).contains(DirtyFlags::A11Y));
        assert!(!s.dirty_flags(n).contains(DirtyFlags::PAINT));
        assert_eq!(s.a11y_dirty(), &[n]);
        // Re-setting the same state is a no-op (stays out of the dirty set once).
        s.set_a11y_state(n, 0b1);
        assert_eq!(s.a11y_dirty(), &[n]);
    }

    #[test]
    fn set_text_content_dirties_paint_and_a11y_not_layout() {
        let (mut s, n, rect) = painted_node();
        s.set_text_content(n, "hello");
        let f = s.dirty_flags(n);
        assert!(f.contains(DirtyFlags::PAINT));
        assert!(f.contains(DirtyFlags::A11Y));
        assert!(
            !f.contains(DirtyFlags::LAYOUT),
            "label text: box unchanged (§8.1)"
        );
        assert!(s.layout_dirty().is_empty());
        assert_eq!(s.a11y_dirty(), &[n]);
        assert_eq!(s.damage(), rect);
        assert_eq!(s.a11y(n).unwrap().value.as_deref(), Some("hello"));
    }

    #[test]
    fn content_leaf_kinds_are_not_containers() {
        for k in [
            WidgetKind::ProgressBar,
            WidgetKind::LoadingSpinner,
            WidgetKind::Switch,
            WidgetKind::Radio,
            WidgetKind::Divider,
            WidgetKind::Chart,
            WidgetKind::Link,
            WidgetKind::Badge,
            WidgetKind::Tab,
            WidgetKind::ListItem,
            WidgetKind::TableCell,
            WidgetKind::Dropdown,
            WidgetKind::DropdownOption,
        ] {
            assert!(
                !k.is_container(),
                "{k:?} is a content leaf, not a container"
            );
        }
        // TabBar/List/Table/TableRow are semantic containers (like Scroll):
        // geometry from children, a role of their own.
        assert!(WidgetKind::TabBar.is_container());
        assert!(WidgetKind::List.is_container());
        assert!(WidgetKind::Table.is_container());
        assert!(WidgetKind::TableRow.is_container());
    }

    /// `set_color` reaches a `Line` primitive's colour like it does rects/glyphs
    /// (SOUL §3.2 — the line rides the quad family). Pure visual → paint-dirty only.
    #[test]
    fn set_color_updates_line_primitive() {
        let mut s = Scene::new();
        let root = s.insert(WidgetKind::Column, None);
        s.set_root(root);
        let n = s.insert(WidgetKind::Chart, Some(root));
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        s.set_layout(
            n,
            LayoutBox {
                rect,
                content: Rect::ZERO,
            },
        );
        s.replace_primitives(
            n,
            [Primitive::Line {
                from: Point { x: 0.0, y: 0.0 },
                to: Point { x: 40.0, y: 40.0 },
                width: 2.0,
                color: Color::WHITE,
            }],
        );
        s.clear_dirty();

        s.set_color(n, Color::BLACK);
        // colour actually changed on the line
        match s.paint(n).unwrap().primitives[0] {
            Primitive::Line { color, .. } => assert_eq!(color, Color::BLACK),
            ref p => panic!("expected a Line, got {p:?}"),
        }
        let f = s.dirty_flags(n);
        assert!(f.contains(DirtyFlags::PAINT));
        assert!(!f.contains(DirtyFlags::LAYOUT), "colour is not a relayout");
        assert!(!f.contains(DirtyFlags::A11Y));
        assert_eq!(s.damage(), rect);
    }

    // --- scroll offset: the v0 property-tree stand-in (SOUL §3.2/§8.1) ---

    #[test]
    fn scroll_offset_defaults_to_zero_and_roundtrips() {
        let (mut s, n, _rect) = painted_node();
        assert_eq!(s.scroll_offset(n), Point::default());
        let off = Point { x: 0.0, y: 12.0 };
        s.set_scroll_offset(n, off);
        assert_eq!(s.scroll_offset(n), off);
    }

    #[test]
    fn set_scroll_offset_dirties_paint_only_and_damages_node_rect() {
        let (mut s, n, rect) = painted_node();
        s.set_scroll_offset(n, Point { x: 0.0, y: 20.0 });
        let f = s.dirty_flags(n);
        assert!(f.contains(DirtyFlags::PAINT));
        assert!(
            !f.contains(DirtyFlags::LAYOUT),
            "scroll never relayouts (SOUL §3.2/§8.1)"
        );
        assert!(!f.contains(DirtyFlags::A11Y), "scroll is not semantic");
        assert!(s.layout_dirty().is_empty());
        assert!(s.a11y_dirty().is_empty());
        // the whole viewport (the node's laid-out rect) is the damage.
        assert_eq!(s.damage(), rect);
    }

    #[test]
    fn idempotent_set_scroll_offset_stays_clean() {
        let (mut s, n, _rect) = painted_node();
        let off = Point { x: 3.0, y: 7.0 };
        s.set_scroll_offset(n, off);
        s.clear_dirty();
        // same offset → no-op, no damage.
        s.set_scroll_offset(n, off);
        assert!(s.dirty_flags(n).is_empty());
        assert_eq!(s.damage(), Rect::ZERO);
    }

    /// The soul made executable (SOUL §1, §4.1): the second identical re-render
    /// cycle — mutate one property, read damage, clear — allocates **nothing**.
    /// Run with `cargo test -p schnellui-scene --features count-allocations`.
    #[cfg(feature = "count-allocations")]
    #[test]
    fn mutation_cycle_allocates_nothing() {
        let (mut s, n, _rect) = painted_node();
        // Warm capacity; first cycle may allocate — the covenant measures the 2nd.
        s.set_color(n, Color::BLACK);
        let _ = s.damage();
        s.clear_dirty();

        let info = allocation_counter::measure(|| {
            // WHITE differs from the warmup's BLACK → exercises the full
            // change→mark_dirty→damage-union→clear path.
            s.set_color(std::hint::black_box(n), std::hint::black_box(Color::WHITE));
            std::hint::black_box(s.damage());
            s.clear_dirty();
        });
        assert_eq!(info.count_total, 0, "allocs on steady-state re-render");
        assert_eq!(info.bytes_total, 0, "bytes on steady-state re-render");
    }

    /// The scroll path is a re-render row too (SOUL §4.1 `scroll` — literal zero):
    /// the second offset mutation cycle mutates the map slot in place and allocates
    /// **nothing**. The first set (which allocates the slot) is the warmup grow event
    /// (§4), deliberately excluded from the measured cycle.
    #[cfg(feature = "count-allocations")]
    #[test]
    fn scroll_offset_mutation_cycle_allocates_nothing() {
        let (mut s, n, _rect) = painted_node();
        // Warm capacity: the first set allocates the `scroll` map slot (grow, §4).
        s.set_scroll_offset(n, Point { x: 0.0, y: 10.0 });
        let _ = s.damage();
        s.clear_dirty();

        let info = allocation_counter::measure(|| {
            s.set_scroll_offset(
                std::hint::black_box(n),
                std::hint::black_box(Point { x: 0.0, y: 20.0 }),
            );
            std::hint::black_box(s.damage());
            s.clear_dirty();
        });
        assert_eq!(info.count_total, 0, "allocs on steady-state scroll");
        assert_eq!(info.bytes_total, 0, "bytes on steady-state scroll");
    }

    // --- image atlas (SOUL §3.2 — grow-only RGBA resource store) ---

    #[test]
    fn image_atlas_starts_empty_and_grows_on_first_insert() {
        let mut a = ImageAtlas::new_empty();
        assert!(a.is_empty());
        assert_eq!(a.pixels().len(), 0);
        assert_eq!(a.revision(), 0);

        let px = vec![0xffu8; 8 * 4 * 4];
        let r = a.insert(8, 4, &px).expect("first insert grows the store");
        assert!(!a.is_empty());
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 8, 4));
        assert!(a.revision() > 0, "insert bumps the revision");
        // the pixels landed at the packed rect
        let stride = a.width() as usize * 4;
        assert_eq!(a.pixels()[0], 0xff);
        assert_eq!(a.pixels()[3 * stride + 8 * 4 - 1], 0xff);
        // just outside the rect stays cleared
        assert_eq!(a.pixels()[8 * 4], 0x00);
    }

    #[test]
    fn image_atlas_packs_side_by_side_and_rejects_bad_input() {
        let mut a = ImageAtlas::new_empty();
        let px = vec![0x80u8; 16 * 16 * 4];
        let r1 = a.insert(16, 16, &px).unwrap();
        let r2 = a.insert(16, 16, &px).unwrap();
        assert_ne!((r1.x, r1.y), (r2.x, r2.y), "distinct slots");
        // short pixel buffer / zero dims / oversized are rejected
        assert!(a.insert(16, 16, &px[..10]).is_none());
        assert!(a.insert(0, 4, &px).is_none());
        assert!(a.insert(ImageAtlas::MAX_DIM + 1, 1, &[0u8; 8]).is_none());
    }

    #[test]
    fn image_atlas_grow_preserves_content_and_bumps_revision() {
        let mut a = ImageAtlas::new_empty();
        // fill a whole 256-wide shelf so the next insert must grow
        let wide = vec![0xaau8; 256 * 256 * 4];
        let r1 = a.insert(256, 256, &wide).unwrap();
        let (w0, rev0) = (a.width(), a.revision());
        let px = vec![0x55u8; 64 * 64 * 4];
        let r2 = a.insert(64, 64, &px).unwrap();
        assert!(
            a.width() > w0 || a.height() > 256,
            "backing store grew to fit"
        );
        assert!(a.revision() > rev0);
        // old content survived the re-stride at its original texel rect
        let stride = a.width() as usize * 4;
        let old = (r1.y as usize + 10) * stride + (r1.x as usize + 10) * 4;
        assert_eq!(a.pixels()[old], 0xaa);
        let new = (r2.y as usize + 1) * stride + (r2.x as usize + 1) * 4;
        assert_eq!(a.pixels()[new], 0x55);
    }

    /// The async image pipeline's atlas half (SOUL §8.1): `reserve` hands out a
    /// zeroed rect without bumping the revision (nothing visible changed), and a
    /// later `write_rect` lands the pixels exactly there and bumps it.
    #[test]
    fn image_atlas_reserve_then_write_rect_lands_pixels() {
        let mut a = ImageAtlas::new_empty();
        let r = a.reserve(8, 4).expect("reserve grows the store");
        let rev_grow = a.revision();
        assert!(rev_grow > 0, "the first reserve grew (re-stride ⇒ bump)");
        assert!(a.reserve(8, 4).is_some());
        assert_eq!(
            a.revision(),
            rev_grow,
            "a pure reservation changes no texels"
        );
        // the reserved region is transparent until the pixels land
        let stride = a.width() as usize * 4;
        let idx = r.y as usize * stride + r.x as usize * 4;
        assert_eq!(a.pixels()[idx], 0);

        let px = vec![0xcdu8; 8 * 4 * 4];
        assert!(a.write_rect(r, &px));
        assert!(a.revision() > rev_grow, "write bumps the revision");
        assert_eq!(a.pixels()[idx], 0xcd);
        assert_eq!(
            a.pixels()[(r.y as usize + 3) * stride + (r.x as usize + 7) * 4 + 3],
            0xcd
        );

        // short pixels / out-of-bounds rects write nothing
        assert!(!a.write_rect(r, &px[..10]));
        let oob = TexelRect {
            x: a.width(),
            y: 0,
            width: 8,
            height: 4,
        };
        assert!(!a.write_rect(oob, &px));
    }

    #[test]
    fn image_atlas_tracks_the_union_of_incremental_pixel_writes() {
        let mut atlas = ImageAtlas::new_empty();
        let first = atlas.reserve(8, 8).unwrap();
        // Initial growth changes the whole backing texture.
        assert_eq!(
            atlas.take_dirty(),
            Some(TexelRect {
                x: 0,
                y: 0,
                width: 256,
                height: 256,
            })
        );
        assert!(atlas.write_rect(first, &[0xff; 8 * 8 * 4]));
        let second = atlas.reserve(4, 4).unwrap();
        assert!(atlas.write_rect(second, &[0x80; 4 * 4 * 4]));

        assert_eq!(
            atlas.take_dirty(),
            Some(TexelRect {
                x: first.x.min(second.x),
                y: first.y.min(second.y),
                width: first
                    .x
                    .saturating_add(first.width)
                    .max(second.x.saturating_add(second.width))
                    - first.x.min(second.x),
                height: first
                    .y
                    .saturating_add(first.height)
                    .max(second.y.saturating_add(second.height))
                    - first.y.min(second.y),
            })
        );
        assert_eq!(atlas.take_dirty(), None);
    }

    #[test]
    fn image_atlas_cached_resources_share_one_allocation() {
        let mut atlas = ImageAtlas::new_empty();
        let key = ImageCacheKey::new("test-icons", "home", "outlined", 24, 24);
        let (first, allocated) = atlas
            .reserve_cached(key.clone(), 24, 24)
            .expect("first cached reservation");
        assert!(allocated);
        let revision = atlas.revision();

        let (second, allocated) = atlas.reserve_cached(key, 24, 24).expect("cached lookup");
        assert!(!allocated, "equal resource must not reserve twice");
        assert_eq!(first, second);
        assert_eq!(atlas.cached_len(), 1);
        assert_eq!(
            atlas.revision(),
            revision,
            "a cache hit changes neither pixels nor GPU revision"
        );

        let other = ImageCacheKey::new("test-icons", "home", "outlined", 24, 24).with_format(1);
        let (third, allocated) = atlas
            .reserve_cached(other, 24, 24)
            .expect("different pixel representation");
        assert!(allocated);
        assert_ne!(first, third);
        assert_eq!(atlas.cached_len(), 2);
    }

    /// `set_color` reaches an `ImageQuad`'s tint like it does the other primitive
    /// colours (SOUL §3.2). Pure visual → paint-dirty only.
    #[test]
    fn set_color_updates_image_quad_tint() {
        let mut s = Scene::new();
        let n = s.insert(WidgetKind::Image, None);
        s.set_root(n);
        let rect = Rect::new(0.0, 0.0, 32.0, 32.0);
        s.set_layout(
            n,
            LayoutBox {
                rect,
                content: Rect::ZERO,
            },
        );
        s.replace_primitives(
            n,
            [Primitive::ImageQuad {
                rect,
                atlas_uv: Rect::new(0.0, 0.0, 32.0, 32.0),
                tint: Color::WHITE,
            }],
        );
        s.clear_dirty();
        s.set_color(n, Color::rgb(0xff, 0x00, 0x00));
        match s.paint(n).unwrap().primitives[0] {
            Primitive::ImageQuad { tint, .. } => assert_eq!(tint, Color::rgb(0xff, 0x00, 0x00)),
            ref p => panic!("expected an ImageQuad, got {p:?}"),
        }
        assert!(s.dirty_flags(n).contains(DirtyFlags::PAINT));
        assert!(!s.dirty_flags(n).contains(DirtyFlags::LAYOUT));
    }

    #[test]
    fn component_refs_resolve_per_mount_and_clear_with_their_node() {
        let reference = ComponentRef::new();
        let mut scene = Scene::new();
        let node = scene.insert(WidgetKind::Text, None);

        scene.set_component_ref(node, reference);
        assert_eq!(scene.resolve_ref(reference), Some(node));
        assert_eq!(scene.component_ref(node), Some(reference));

        scene.remove(node);
        assert_eq!(scene.resolve_ref(reference), None);
        assert_eq!(scene.component_ref(node), None);
    }

    #[test]
    fn preorder_is_parent_first_and_preserves_sibling_order() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let first = scene.insert(WidgetKind::Text, Some(root));
        let branch = scene.insert(WidgetKind::Row, Some(root));
        let nested = scene.insert(WidgetKind::Button, Some(branch));
        let last = scene.insert(WidgetKind::TextInput, Some(root));

        assert_eq!(
            scene.preorder().collect::<Vec<_>>(),
            vec![root, first, branch, nested, last]
        );
    }

    #[test]
    fn preorder_handles_deep_trees_without_recursion() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let mut parent = root;
        for _ in 0..20_000 {
            parent = scene.insert(WidgetKind::Column, Some(parent));
        }

        assert_eq!(scene.preorder().count(), 20_001);
    }

    #[test]
    fn remove_subtree_reports_position_and_keeps_siblings() {
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let first = scene.insert(WidgetKind::Text, Some(root));
        let branch = scene.insert(WidgetKind::Row, Some(root));
        let nested = scene.insert(WidgetKind::Button, Some(branch));
        let last = scene.insert(WidgetKind::TextInput, Some(root));

        let removed = scene.remove_subtree(branch).unwrap();

        assert_eq!(removed.parent, Some(root));
        assert_eq!(removed.child_index, 1);
        assert_eq!(removed.nodes, vec![branch, nested]);
        assert_eq!(scene.node(root).unwrap().children.as_slice(), [first, last]);
        assert!(scene.node(branch).is_none());
        assert!(scene.node(nested).is_none());
    }
}
