pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("png header");
        writer.write_image_data(rgba).expect("png data");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_core::changed_r8_rect;
    use crate::renderer::srgb_to_linear;
    use crate::*;
    use schnellui_scene::{
        Color, DirtyFlags, LayoutBox, Point, Primitive, Rect, Scene, WidgetKind,
    };
    use schnellui_text::GlyphAtlas;

    #[test]
    fn padding_rounds_up_to_256() {
        // width 100 * 4 = 400 → next multiple of 256 is 512
        assert_eq!(padded_bytes_per_row(100, 4), 512);
        // width 64 * 4 = 256 → already aligned
        assert_eq!(padded_bytes_per_row(64, 4), 256);
        // width 1 * 4 = 4 → 256
        assert_eq!(padded_bytes_per_row(1, 4), 256);
    }

    #[test]
    fn unpad_strips_row_padding() {
        // 2x2 image, real row = 8 bytes, padded row = 12 bytes
        let padded_bpr = 12u32;
        let mut padded = vec![0u8; (padded_bpr * 2) as usize];
        // row 0 real pixels
        padded[0..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        // row 1 real pixels start at offset 12
        padded[12..20].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        let out = unpad_rows(&padded, 2, 2, padded_bpr);
        assert_eq!(out, (1u8..=16).collect::<Vec<u8>>());
    }

    #[test]
    fn glyph_atlas_diff_is_empty_for_identical_remount_content() {
        let atlas = vec![0_u8; 8 * 4];
        assert_eq!(changed_r8_rect(&atlas, &atlas, 8, 4), None);
    }

    #[test]
    fn glyph_atlas_diff_bounds_added_and_removed_coverage() {
        let mut previous = vec![0_u8; 8 * 4];
        previous[2 * 8 + 6] = 0xff;
        let mut replacement = previous.clone();
        replacement[2 * 8 + 6] = 0;
        replacement[1] = 0x80;
        assert_eq!(
            changed_r8_rect(&previous, &replacement, 8, 4),
            Some(schnellui_text::AtlasRect {
                x: 1,
                y: 0,
                width: 6,
                height: 3,
            })
        );
    }

    #[test]
    fn remount_reconciliation_retains_texture_and_consumes_fresh_dirty_state() {
        let mut renderer = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("skipping GPU atlas reconciliation test: {error}");
                return;
            }
        };
        renderer.core.atlas_shadow = Some(Vec::new());
        let rect = schnellui_text::AtlasRect {
            x: 3,
            y: 2,
            width: 2,
            height: 2,
        };
        let mut previous = GlyphAtlas::new(64, 64);
        previous.write_coverage(rect, &[1, 2, 3, 4]);
        assert!(renderer.core.ensure_atlas(&previous));

        let mut replacement = GlyphAtlas::new(64, 64);
        replacement.write_coverage(rect, &[1, 2, 3, 9]);
        renderer
            .core
            .reconcile_remounted_glyph_atlas(&mut replacement);

        assert!(renderer.core.atlas.is_some());
        assert_eq!(
            renderer.core.atlas_shadow.as_deref(),
            Some(replacement.pixels())
        );
        assert_eq!(replacement.take_dirty(), None);
    }

    #[test]
    fn quad_instance_from_scene_types() {
        let q = QuadInstance::solid(
            Rect::new(1.0, 2.0, 3.0, 4.0),
            Color::rgba(255, 0, 0, 255),
            2.0,
        );
        assert_eq!(q.rect, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(q.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(q.params[0], 2.0);
    }

    #[test]
    fn glyph_instance_from_scene_types() {
        let g = GlyphInstance::glyph(
            Rect::new(5.0, 6.0, 7.0, 8.0),
            Rect::new(0.0, 0.0, 4.0, 4.0),
            Color::rgb(0, 255, 0),
        );
        assert_eq!(g.rect, [5.0, 6.0, 7.0, 8.0]);
        assert_eq!(g.atlas_uv, [0.0, 0.0, 4.0, 4.0]);
        assert_eq!(g.color, [0.0, 1.0, 0.0, 1.0]);
        // default constructor is unclipped.
        assert_eq!(g.clip, UNCLIPPED_CLIP);
    }

    #[test]
    fn solid_and_glyph_default_to_unclipped_axis_aligned() {
        let q = QuadInstance::solid(Rect::new(1.0, 2.0, 3.0, 4.0), Color::WHITE, 0.0);
        assert_eq!(q.clip, UNCLIPPED_CLIP);
        assert_eq!(q.params[1], 0.0, "axis-aligned rect keeps rotation 0");
    }

    /// A line encodes as an oriented quad: horizontal ⇒ rotation 0 (the axis-aligned
    /// fast path), a 45° segment ⇒ rotation π/4, centred on the midpoint with length
    /// |A−B| (SOUL §3.2).
    #[test]
    fn line_instance_encodes_oriented_quad() {
        // horizontal line A(0,10)->B(100,10), width 4
        let h = QuadInstance::line(
            Point { x: 0.0, y: 10.0 },
            Point { x: 100.0, y: 10.0 },
            4.0,
            Color::WHITE,
            UNCLIPPED_CLIP,
        );
        assert!(h.params[1].abs() < 1e-6, "horizontal line has rotation 0");
        // centre (50,10), length 100, width 4 ⇒ bbox [0, 8, 100, 4]
        assert_eq!(h.rect, [0.0, 8.0, 100.0, 4.0]);
        assert_eq!(h.params[0], 0.0, "sharp caps (corner radius 0)");

        // 45° diagonal A(0,0)->B(10,10), width 2
        let d = QuadInstance::line(
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 10.0 },
            2.0,
            Color::WHITE,
            UNCLIPPED_CLIP,
        );
        assert!((d.params[1] - std::f32::consts::FRAC_PI_4).abs() < 1e-5);
        let len = 200.0f32.sqrt();
        assert!((d.rect[2] - len).abs() < 1e-4, "length = |A-B|");
        assert!((d.rect[3] - 2.0).abs() < 1e-6, "width preserved");
    }

    #[test]
    fn png_roundtrips_dimensions() {
        // 1x1 opaque red
        let png_bytes = encode_png(&[255, 0, 0, 255], 1, 1);
        assert_eq!(&png_bytes[1..4], b"PNG");
    }

    /// The image pipeline end to end (SOUL §3.2, §7.2): pixels inserted into the
    /// scene's RGBA atlas render through a `Primitive::ImageQuad` and read back
    /// byte-exact (sRGB texture → linear shading → sRGB target round-trip).
    #[test]
    fn image_quad_renders_atlas_pixels_headless() {
        let mut r = match Renderer::try_new(32, 32, Backend::Auto) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping GPU render test: {e}");
                return;
            }
        };
        r.set_clear_color(Color::rgba(0, 0, 0, 255));

        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Image, None);
        scene.set_root(root);
        // a 2×2 bitmap: red, green / blue, semi-transparent white
        let bitmap: Vec<u8> = vec![
            255, 0, 0, 255, /**/ 0, 255, 0, 255, //
            0, 0, 255, 255, /**/ 255, 255, 255, 128,
        ];
        let tex = scene.images_mut().insert(2, 2, &bitmap).expect("insert");
        // draw it magnified 8× at (8,8) so each texel covers an 8×8 pixel block
        scene.paint_mut(root).primitives.push(Primitive::ImageQuad {
            rect: Rect::new(8.0, 8.0, 16.0, 16.0),
            atlas_uv: Rect::new(tex.x as f32, tex.y as f32, 2.0, 2.0),
            tint: Color::WHITE,
        });
        scene.mark_dirty(root, DirtyFlags::PAINT);

        let atlas = GlyphAtlas::new(32, 32);
        let rgba = r.render_rgba8(&scene, &atlas);
        // block interiors, away from texel seams (nearest sampling)
        assert_eq!(px(&rgba, 32, 10, 10), [255, 0, 0, 255], "top-left texel");
        assert_eq!(px(&rgba, 32, 21, 10), [0, 255, 0, 255], "top-right texel");
        assert_eq!(px(&rgba, 32, 10, 21), [0, 0, 255, 255], "bottom-left texel");
        // Semi-transparent white over the black clear: blending runs in *linear*
        // space (the sRGB target re-encodes), so 50.2% linear white encodes to
        // ≈188 sRGB — not the naive 128 (SOUL §7.2).
        let blended = px(&rgba, 32, 21, 21);
        assert!(
            blended[0] > 170 && blended[0] < 205 && blended[0] == blended[1],
            "linear-space alpha blend over background: {blended:?}"
        );
        // outside the quad stays background
        assert_eq!(px(&rgba, 32, 2, 2), [0, 0, 0, 255], "background");
    }

    #[test]
    fn srgb_endpoints_are_identity() {
        assert_eq!(srgb_to_linear(0), 0.0);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-9);
    }

    /// Fetch a pixel from a tightly-packed RGBA8 buffer.
    fn px(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let off = ((y * width + x) * 4) as usize;
        [rgba[off], rgba[off + 1], rgba[off + 2], rgba[off + 3]]
    }

    #[test]
    fn renders_two_colored_rects_headless() {
        // SOUL §7.2 test: render two colored rects, read back, assert exact pixel
        // colors at known coords. Skip gracefully (not fail) if no adapter.
        let mut r = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping GPU render test: {e}");
                return;
            }
        };
        r.set_clear_color(Color::rgba(0, 0, 0, 255)); // opaque black background

        // Build a tiny scene: two solid rects painted on the root node.
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Stack, None);
        scene.set_root(root);
        {
            let pd = scene.paint_mut(root);
            pd.primitives.push(Primitive::SolidRect {
                rect: Rect::new(4.0, 4.0, 20.0, 20.0),
                color: Color::rgba(255, 0, 0, 255), // pure red
                corner_radius: 0.0,
            });
            pd.primitives.push(Primitive::SolidRect {
                rect: Rect::new(36.0, 36.0, 20.0, 20.0),
                color: Color::rgba(0, 0, 255, 255), // pure blue
                corner_radius: 0.0,
            });
        }
        scene.mark_dirty(root, DirtyFlags::PAINT);

        let atlas = GlyphAtlas::new(64, 64);
        let rgba = r.render_rgba8(&scene, &atlas);
        assert_eq!(rgba.len(), (64 * 64 * 4) as usize);

        // Interior of the red rect (center ~14,14).
        assert_eq!(px(&rgba, 64, 14, 14), [255, 0, 0, 255], "red rect interior");
        // Interior of the blue rect (center ~46,46).
        assert_eq!(
            px(&rgba, 64, 46, 46),
            [0, 0, 255, 255],
            "blue rect interior"
        );
        // Background between them stays the clear color.
        assert_eq!(px(&rgba, 64, 0, 0), [0, 0, 0, 255], "background top-left");
        assert_eq!(px(&rgba, 64, 63, 0), [0, 0, 0, 255], "background top-right");
    }

    /// An overlay-flagged subtree draws **above** content that comes later in tree
    /// order (SOUL §3.2 z-order): the earlier overlay rect wins the overlapping
    /// pixels a plain tree-order draw would give to the later sibling.
    #[test]
    fn overlay_subtree_draws_above_later_siblings() {
        let mut r = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping GPU render test: {e}");
                return;
            }
        };
        r.set_clear_color(Color::rgba(0, 0, 0, 255));

        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        // FIRST child: the overlay layer (a dropdown popup stand-in), red.
        let popup = scene.insert(WidgetKind::Column, Some(root));
        scene.set_overlay(popup);
        let popup_leaf = scene.insert(WidgetKind::Chart, Some(popup));
        scene
            .paint_mut(popup_leaf)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(8.0, 8.0, 24.0, 24.0),
                color: Color::rgba(255, 0, 0, 255),
                corner_radius: 0.0,
            });
        scene.mark_dirty(popup_leaf, DirtyFlags::PAINT);
        // SECOND child: base content overlapping the popup, blue — later in tree
        // order, so a plain walk would paint it on top.
        let below = scene.insert(WidgetKind::Chart, Some(root));
        scene
            .paint_mut(below)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(16.0, 16.0, 40.0, 40.0),
                color: Color::rgba(0, 0, 255, 255),
                corner_radius: 0.0,
            });
        scene.mark_dirty(below, DirtyFlags::PAINT);

        let atlas = GlyphAtlas::new(64, 64);
        let rgba = r.render_rgba8(&scene, &atlas);
        // In the overlap the overlay's red wins.
        assert_eq!(
            px(&rgba, 64, 20, 20),
            [255, 0, 0, 255],
            "overlap is overlay"
        );
        // Outside the popup the base blue still paints.
        assert_eq!(
            px(&rgba, 64, 50, 50),
            [0, 0, 255, 255],
            "base content shows"
        );
    }

    #[test]
    fn higher_overlay_level_draws_above_a_later_declared_peer() {
        let mut r = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping GPU render test: {e}");
                return;
            }
        };
        r.set_clear_color(Color::rgba(0, 0, 0, 255));

        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Stack, None);
        scene.set_root(root);

        // The modal stand-in is declared first, but its explicit stack level
        // keeps it above a modeless peer declared later.
        let modal = scene.insert(WidgetKind::DialogLayer, Some(root));
        scene.set_overlay_level(modal, 20);
        scene
            .paint_mut(modal)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(8.0, 8.0, 40.0, 40.0),
                color: Color::rgba(255, 0, 0, 255),
                corner_radius: 0.0,
            });
        scene.mark_dirty(modal, DirtyFlags::PAINT);

        let modeless = scene.insert(WidgetKind::DialogLayer, Some(root));
        scene.set_overlay_level(modeless, 10);
        scene
            .paint_mut(modeless)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(16.0, 16.0, 40.0, 40.0),
                color: Color::rgba(0, 0, 255, 255),
                corner_radius: 0.0,
            });
        scene.mark_dirty(modeless, DirtyFlags::PAINT);

        let atlas = GlyphAtlas::new(64, 64);
        let rgba = r.render_rgba8(&scene, &atlas);
        assert_eq!(
            px(&rgba, 64, 24, 24),
            [255, 0, 0, 255],
            "modal level wins above later modeless layer"
        );
        assert_eq!(
            px(&rgba, 64, 52, 52),
            [0, 0, 255, 255],
            "modeless peer still paints outside the modal"
        );
    }

    #[test]
    fn raising_an_overlay_changes_within_level_compositing_order() {
        let mut r = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping GPU render test: {e}");
                return;
            }
        };
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Stack, None);
        scene.set_root(root);
        let lower = scene.insert(WidgetKind::DialogLayer, Some(root));
        scene.set_overlay_level(lower, 10);
        scene
            .paint_mut(lower)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(8.0, 8.0, 40.0, 40.0),
                color: Color::rgba(255, 0, 0, 255),
                corner_radius: 0.0,
            });
        let upper = scene.insert(WidgetKind::DialogLayer, Some(root));
        scene.set_overlay_level(upper, 10);
        scene
            .paint_mut(upper)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(16.0, 16.0, 40.0, 40.0),
                color: Color::rgba(0, 0, 255, 255),
                corner_radius: 0.0,
            });

        let atlas = GlyphAtlas::new(64, 64);
        let initial = r.render_rgba8(&scene, &atlas);
        assert_eq!(px(&initial, 64, 24, 24), [0, 0, 255, 255]);

        assert!(scene.bring_overlay_to_front(lower));
        let raised = r.render_rgba8(&scene, &atlas);
        assert_eq!(
            px(&raised, 64, 24, 24),
            [255, 0, 0, 255],
            "raised peer paints on top without leaving its shared level"
        );
    }

    /// A `Primitive::Line` renders as an oriented quad: the diagonal midpoint is
    /// painted, a point far off the segment stays background (SOUL §3.2).
    #[test]
    fn line_primitive_paints_along_diagonal() {
        let mut r = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping GPU render test: {e}");
                return;
            }
        };
        r.set_clear_color(Color::rgba(0, 0, 0, 255));

        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Chart, None);
        scene.set_root(root);
        scene.paint_mut(root).primitives.push(Primitive::Line {
            from: Point { x: 8.0, y: 8.0 },
            to: Point { x: 56.0, y: 56.0 },
            width: 4.0,
            color: Color::rgba(255, 0, 0, 255),
        });
        scene.mark_dirty(root, DirtyFlags::PAINT);

        let atlas = GlyphAtlas::new(64, 64);
        let rgba = r.render_rgba8(&scene, &atlas);
        // On the diagonal midpoint the stroke is painted red.
        let on = px(&rgba, 64, 32, 32);
        assert!(
            on[0] > 100 && on[2] < 100,
            "line midpoint reddish, got {on:?}"
        );
        // Far off the segment (same along-axis position, ~25px perpendicular): clear.
        assert_eq!(
            px(&rgba, 64, 50, 14),
            [0, 0, 0, 255],
            "off-diagonal stays background"
        );
    }

    /// Content inside a `Scroll` node is clipped to the node's laid-out viewport rect:
    /// the portion below the viewport does not paint (SOUL §3.2 per-instance clip).
    #[test]
    fn scroll_clip_masks_content_outside_viewport() {
        let mut r = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping GPU render test: {e}");
                return;
            }
        };
        r.set_clear_color(Color::rgba(0, 0, 0, 255));

        let mut scene = Scene::new();
        // The scroll viewport is the top half of the target.
        let scroll = scene.insert(WidgetKind::Scroll, None);
        scene.set_root(scroll);
        scene.set_layout(
            scroll,
            LayoutBox {
                rect: Rect::new(0.0, 0.0, 64.0, 32.0),
                content: Rect::ZERO,
            },
        );
        // A child painting a full-target red rect; only its top half is in view.
        let child = scene.insert(WidgetKind::Chart, Some(scroll));
        scene
            .paint_mut(child)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(0.0, 0.0, 64.0, 64.0),
                color: Color::rgba(255, 0, 0, 255),
                corner_radius: 0.0,
            });
        scene
            .paint_mut(child)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(0.0, 80.0, 64.0, 16.0),
                color: Color::rgba(0, 0, 255, 255),
                corner_radius: 0.0,
            });
        scene.mark_dirty(child, DirtyFlags::PAINT);

        let atlas = GlyphAtlas::new(64, 64);
        let rgba = r.render_rgba8(&scene, &atlas);
        // Inside the viewport (top half): painted.
        assert_eq!(
            px(&rgba, 64, 32, 16),
            [255, 0, 0, 255],
            "inside viewport painted"
        );
        // Below the viewport rect: clipped away → background.
        assert_eq!(
            px(&rgba, 64, 32, 48),
            [0, 0, 0, 255],
            "below viewport clipped"
        );
        assert_eq!(
            r.core.quad_scratch.len(),
            1,
            "fully occluded primitives never enter the GPU instance buffer"
        );
    }

    #[test]
    fn scroll_culls_offscreen_rich_text_before_gathering_its_glyphs() {
        let mut renderer = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("skipping GPU render test: {error}");
                return;
            }
        };
        let mut scene = Scene::new();
        let scroll = scene.insert(WidgetKind::Scroll, None);
        scene.set_root(scroll);
        scene.set_layout(
            scroll,
            LayoutBox {
                rect: Rect::new(0.0, 0.0, 64.0, 32.0),
                content: Rect::ZERO,
            },
        );
        let visible = scene.insert(WidgetKind::RichText, Some(scroll));
        scene.set_layout(
            visible,
            LayoutBox {
                rect: Rect::new(0.0, 4.0, 64.0, 12.0),
                content: Rect::ZERO,
            },
        );
        let offscreen = scene.insert(WidgetKind::RichText, Some(scroll));
        scene.set_layout(
            offscreen,
            LayoutBox {
                rect: Rect::new(0.0, 96.0, 64.0, 12.0),
                content: Rect::ZERO,
            },
        );
        let glyph = |x, y| Primitive::GlyphQuad {
            rect: Rect::new(x, y, 4.0, 8.0),
            atlas_uv: Rect::new(0.0, 0.0, 4.0, 8.0),
            color: Color::WHITE,
        };
        scene
            .paint_mut(visible)
            .primitives
            .extend([glyph(0.0, 4.0), glyph(5.0, 4.0)]);
        for i in 0..256 {
            scene
                .paint_mut(offscreen)
                .primitives
                .push(glyph((i % 16) as f32 * 4.0, 96.0 + (i / 16) as f32 * 8.0));
        }

        renderer.core.gather(&scene);
        assert_eq!(renderer.core.glyph_scratch.len(), 2);
        assert_eq!(renderer.core.glyph_scratch[0].rect, [0.0, 4.0, 4.0, 8.0]);
        assert_eq!(renderer.core.glyph_scratch[1].rect, [5.0, 4.0, 4.0, 8.0]);
    }

    /// A scroll offset shifts a `Scroll` node's descendants by `−offset` at composite
    /// time — no relayout, no per-node repaint (SOUL §3.2).
    #[test]
    fn scroll_offset_shifts_content() {
        let mut r = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping GPU render test: {e}");
                return;
            }
        };
        r.set_clear_color(Color::rgba(0, 0, 0, 255));

        let mut scene = Scene::new();
        let scroll = scene.insert(WidgetKind::Scroll, None);
        scene.set_root(scroll);
        // Full-target viewport so clipping does not interfere with the shift check.
        scene.set_layout(
            scroll,
            LayoutBox {
                rect: Rect::new(0.0, 0.0, 64.0, 64.0),
                content: Rect::ZERO,
            },
        );
        let child = scene.insert(WidgetKind::Chart, Some(scroll));
        // A horizontal blue band at y in [24, 34).
        scene
            .paint_mut(child)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(0.0, 24.0, 64.0, 10.0),
                color: Color::rgba(0, 0, 255, 255),
                corner_radius: 0.0,
            });
        // Scroll down by 20 ⇒ children shift up by 20: band moves to y in [4, 14).
        scene.set_scroll_offset(scroll, Point { x: 0.0, y: 20.0 });
        scene.mark_dirty(child, DirtyFlags::PAINT);

        let atlas = GlyphAtlas::new(64, 64);
        let rgba = r.render_rgba8(&scene, &atlas);
        // Band now near the top (y ~9): blue.
        assert_eq!(
            px(&rgba, 64, 32, 9),
            [0, 0, 255, 255],
            "band shifted up by the scroll offset"
        );
        // Its original position (y ~29) is vacated → background.
        assert_eq!(
            px(&rgba, 64, 32, 29),
            [0, 0, 0, 255],
            "original band position vacated"
        );
    }

    #[test]
    fn scroll_chrome_is_partitioned_after_layer_content() {
        let mut renderer = match Renderer::try_new(64, 64, Backend::Auto) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("skipping GPU render test: {error}");
                return;
            }
        };
        let mut scene = Scene::new();
        let scroll = scene.insert(WidgetKind::Scroll, None);
        scene.set_root(scroll);
        scene
            .paint_mut(scroll)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(54.0, 0.0, 10.0, 32.0),
                color: Color::rgba(0, 0, 255, 255),
                corner_radius: 0.0,
            });
        let content = scene.insert(WidgetKind::Chart, Some(scroll));
        scene
            .paint_mut(content)
            .primitives
            .push(Primitive::SolidRect {
                rect: Rect::new(0.0, 0.0, 64.0, 32.0),
                color: Color::rgba(255, 0, 0, 255),
                corner_radius: 0.0,
            });

        renderer.core.gather(&scene);
        assert_eq!(renderer.core.base_chrome_start, 1);
        assert_eq!(renderer.core.base_quads, 2);
        assert_eq!(renderer.core.quad_scratch[0].color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(renderer.core.quad_scratch[1].color, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn terminal_append_splices_gpu_glyph_range_without_a_full_gather() {
        let mut renderer = match Renderer::try_new(64, 32, Backend::Auto) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("skipping GPU terminal delta test: {error}");
                return;
            }
        };
        let mut scene = Scene::new();
        let terminal = scene.insert(WidgetKind::TerminalGrid, None);
        scene.set_root(terminal);
        let glyph = |x| Primitive::GlyphQuad {
            rect: Rect::new(x, 4.0, 4.0, 8.0),
            atlas_uv: Rect::new(0.0, 0.0, 4.0, 8.0),
            color: Color::WHITE,
        };
        scene
            .paint_mut(terminal)
            .primitives
            .extend([glyph(0.0), glyph(5.0)]);
        scene.mark_dirty(terminal, DirtyFlags::PAINT);
        let atlas = GlyphAtlas::new(64, 64);
        let _ = renderer.render_rgba8(&scene, &atlas);
        scene.clear_dirty();

        // This is the normal prompt case: echoing one key adds one glyph rather
        // than replacing an existing glyph at a fixed instance count.
        scene.paint_mut(terminal).primitives.push(glyph(10.0));
        scene.mark_dirty(terminal, DirtyFlags::PAINT);
        let _ = renderer.render_rgba8(&scene, &atlas);

        assert_eq!(renderer.core.last_upload_work.full_gathers, 0);
        assert_eq!(renderer.core.last_upload_work.terminal_fragments, 1);
        assert_eq!(renderer.core.glyph_scratch.len(), 3);
        assert_eq!(renderer.core.base_glyphs, 3);
    }

    #[test]
    fn fresh_scene_invalidates_terminal_fragments_even_when_revisions_collide() {
        let mut renderer = match Renderer::try_new(64, 32, Backend::Auto) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("skipping GPU scene identity test: {error}");
                return;
            }
        };
        let build_scene = |scroll_offset: Point, viewport: Rect, glyph_xs: &[f32]| {
            let mut scene = Scene::new();
            let scroll = scene.insert(WidgetKind::Scroll, None);
            scene.set_root(scroll);
            scene.set_layout(
                scroll,
                LayoutBox {
                    rect: viewport,
                    content: viewport,
                },
            );
            scene.set_scroll_offset(scroll, scroll_offset);
            let terminal = scene.insert(WidgetKind::TerminalGrid, Some(scroll));
            scene
                .paint_mut(terminal)
                .primitives
                .extend(glyph_xs.iter().map(|&x| Primitive::GlyphQuad {
                    rect: Rect::new(x, 10.0, 4.0, 8.0),
                    atlas_uv: Rect::new(0.0, 0.0, 4.0, 8.0),
                    color: Color::WHITE,
                }));
            scene.mark_dirty(terminal, DirtyFlags::PAINT);
            (scene, terminal)
        };

        let (first, _) = build_scene(
            Point { x: 1.0, y: 1.0 },
            Rect::new(0.0, 0.0, 64.0, 32.0),
            &[8.0],
        );
        let (second, second_terminal) = build_scene(
            Point { x: 3.0, y: 2.0 },
            Rect::new(10.0, 4.0, 40.0, 20.0),
            &[16.0, 24.0],
        );
        assert_eq!(first.render_revision(), second.render_revision());
        assert_ne!(first.render_key(), second.render_key());

        let atlas = GlyphAtlas::new(64, 64);
        let _ = renderer.render_rgba8(&first, &atlas);
        let _ = renderer.render_rgba8(&second, &atlas);

        assert_eq!(renderer.core.last_upload_work.full_gathers, 1);
        assert_eq!(renderer.core.last_upload_work.terminal_fragments, 0);
        assert_eq!(renderer.core.glyph_scratch.len(), 2);
        let fragment = renderer
            .core
            .terminal_fragments
            .get(&second_terminal)
            .unwrap();
        assert_eq!(fragment.glyphs, 0..2);
        assert_eq!(fragment.offset, Point { x: -3.0, y: -2.0 });
        assert_eq!(fragment.clip, Rect::new(10.0, 4.0, 40.0, 20.0));
        assert_eq!(renderer.core.glyph_scratch[0].rect, [13.0, 8.0, 4.0, 8.0]);
        assert_eq!(renderer.core.glyph_scratch[1].rect, [21.0, 8.0, 4.0, 8.0]);
    }

    #[test]
    fn empty_terminal_append_shifts_only_later_terminal_ranges() {
        let mut renderer = match Renderer::try_new(64, 32, Backend::Auto) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("skipping GPU terminal range test: {error}");
                return;
            }
        };
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        scene.set_root(root);
        let first = scene.insert(WidgetKind::TerminalGrid, Some(root));
        let second = scene.insert(WidgetKind::TerminalGrid, Some(root));
        scene.mark_dirty(first, DirtyFlags::PAINT);
        scene.mark_dirty(second, DirtyFlags::PAINT);
        let atlas = GlyphAtlas::new(64, 64);
        let _ = renderer.render_rgba8(&scene, &atlas);
        scene.clear_dirty();

        let glyph = Primitive::GlyphQuad {
            rect: Rect::new(0.0, 4.0, 4.0, 8.0),
            atlas_uv: Rect::new(0.0, 0.0, 4.0, 8.0),
            color: Color::WHITE,
        };
        scene.paint_mut(first).primitives.push(glyph);
        scene.mark_dirty(first, DirtyFlags::PAINT);
        let _ = renderer.render_rgba8(&scene, &atlas);
        scene.clear_dirty();

        let second_fragment = renderer.core.terminal_fragments.get(&second).unwrap();
        assert_eq!(second_fragment.glyphs, 1..1);

        scene.paint_mut(second).primitives.push(glyph);
        scene.mark_dirty(second, DirtyFlags::PAINT);
        let _ = renderer.render_rgba8(&scene, &atlas);
        assert_eq!(renderer.core.last_upload_work.full_gathers, 0);
        assert_eq!(renderer.core.glyph_scratch.len(), 2);
        assert_eq!(
            renderer
                .core
                .terminal_fragments
                .get(&second)
                .unwrap()
                .glyphs,
            1..2
        );
    }

    #[test]
    fn renders_empty_scene_to_clear_color() {
        let mut r = match Renderer::try_new(32, 32, Backend::Auto) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping GPU render test: {e}");
                return;
            }
        };
        r.set_clear_color(Color::rgba(255, 255, 255, 255)); // opaque white
        let scene = Scene::new();
        let atlas = GlyphAtlas::new(32, 32);
        let rgba = r.render_rgba8(&scene, &atlas);
        // Every pixel is the white clear color.
        assert_eq!(px(&rgba, 32, 0, 0), [255, 255, 255, 255]);
        assert_eq!(px(&rgba, 32, 31, 31), [255, 255, 255, 255]);
        assert_eq!(px(&rgba, 32, 16, 16), [255, 255, 255, 255]);
    }
}
