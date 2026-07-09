use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reset;
    use schnellui_layout::LayoutEngine;
    use schnellui_scene::{Primitive, Scene};

    fn build_one(runtime: &crate::Runtime, view: impl View) -> (Scene, WidgetId) {
        reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(64, 64);
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
        (scene, id)
    }

    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    // --- parser: shapes, presentation, inheritance ---

    #[test]
    fn parses_viewbox_shapes_and_presentation() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r##"<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                  <rect x="2" y="2" width="20" height="8" rx="2" fill="#3366cc"/>
                  <circle cx="12" cy="16" r="5" fill="red"/>
                  <line x1="0" y1="0" x2="24" y2="24" stroke="#000" stroke-width="2"/>
                  <unknown-widget foo="bar"/>
                </svg>"##,
        )
        .expect("parses");
        assert_eq!((doc.width, doc.height), (24.0, 24.0));
        assert_eq!(doc.shapes.len(), 3, "unknown elements are skipped");
        assert_eq!(
            doc.shapes[0].fill,
            Some(Paint::Solid(Color::rgb(0x33, 0x66, 0xcc)))
        );
        assert_eq!(
            doc.shapes[1].fill,
            Some(Paint::Solid(Color::rgb(0xff, 0, 0)))
        );
        // a line strokes and never fills
        assert_eq!(doc.shapes[2].fill, None);
        assert_eq!(doc.shapes[2].stroke, Some(Paint::Solid(Color::BLACK)));
        assert_eq!(doc.shapes[2].stroke_width, 2.0);
    }

    #[test]
    fn fill_defaults_to_black_and_none_disables() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r#"<svg viewBox="0 0 4 4">
                 <rect width="4" height="4"/>
                 <rect width="4" height="4" fill="none" stroke="blue"/>
               </svg>"#,
        )
        .unwrap();
        assert_eq!(doc.shapes[0].fill, Some(Paint::Solid(Color::BLACK)));
        assert_eq!(doc.shapes[1].fill, None);
        assert_eq!(
            doc.shapes[1].stroke,
            Some(Paint::Solid(Color::rgb(0, 0, 0xff)))
        );
    }

    /// Presentation attributes inherit through nested `g` scopes and pop with
    /// them; group opacity multiplies down the stack.
    #[test]
    fn group_attributes_inherit_and_pop() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r#"<svg viewBox="0 0 10 10">
                 <g fill="red" opacity="0.5">
                   <rect width="2" height="2"/>
                   <g fill="blue" opacity="0.5">
                     <rect x="4" width="2" height="2"/>
                   </g>
                 </g>
                 <rect x="8" width="2" height="2"/>
               </svg>"#,
        )
        .unwrap();
        assert_eq!(
            doc.shapes[0].fill,
            Some(Paint::Solid(Color::rgb(0xff, 0, 0)))
        );
        assert!((doc.shapes[0].opacity - 0.5).abs() < 1e-6);
        assert_eq!(
            doc.shapes[1].fill,
            Some(Paint::Solid(Color::rgb(0, 0, 0xff)))
        );
        assert!(
            (doc.shapes[1].opacity - 0.25).abs() < 1e-6,
            "opacity multiplies"
        );
        // outside the groups: back to the defaults
        assert_eq!(doc.shapes[2].fill, Some(Paint::Solid(Color::BLACK)));
        assert!((doc.shapes[2].opacity - 1.0).abs() < 1e-6);
    }

    // --- transforms ---

    #[test]
    fn transforms_bake_into_geometry_and_compose_through_groups() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r#"<svg viewBox="0 0 20 20">
                 <g transform="translate(10,0)">
                   <rect transform="scale(2)" x="1" y="1" width="2" height="2"/>
                 </g>
               </svg>"#,
        )
        .unwrap();
        let SvgShapeKind::Path { contours } = &doc.shapes[0].kind else {
            panic!("rect flattens to a contour");
        };
        // corner (1,1) → scale(2) → (2,2) → translate(10,0) → (12,2)
        assert_eq!(contours[0].pts[0], (12.0, 2.0));
        // stroke widths scale by √|det| = 2
        assert!((doc.shapes[0].stroke_width - 2.0).abs() < 1e-5);
    }

    #[test]
    fn rotate_about_center_maps_points() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r#"<svg viewBox="0 0 10 10">
                 <polygon transform="rotate(90,5,5)" points="5,1 6,5 5,9"/>
               </svg>"#,
        )
        .unwrap();
        let SvgShapeKind::Path { contours } = &doc.shapes[0].kind else {
            panic!()
        };
        // (5,1) rotated 90° about (5,5) → (9,5)
        let (x, y) = contours[0].pts[0];
        assert!((x - 9.0).abs() < 1e-4 && (y - 5.0).abs() < 1e-4, "{x},{y}");
    }

    // --- path data: curves + arcs flatten ---

    #[test]
    fn cubic_and_quadratic_curves_flatten_through_midpoints() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r#"<svg viewBox="0 0 10 10">
                 <path d="M0 5 C 0 0, 10 0, 10 5" fill="none" stroke="black"/>
                 <path d="M0 5 Q 5 -5 10 5" fill="none" stroke="black"/>
               </svg>"#,
        )
        .unwrap();
        let SvgShapeKind::Path { contours } = &doc.shapes[0].kind else {
            panic!()
        };
        let pts = &contours[0].pts;
        assert!(pts.len() > 10, "cubic subdivided ({} pts)", pts.len());
        // symmetric cubic: the midpoint sits at x=5, y=5·(1/8·5+…) — just check
        // it bows upward (y < 5) at mid-x and ends exactly on the endpoint
        let mid = pts[pts.len() / 2];
        assert!((mid.0 - 5.0).abs() < 0.6 && mid.1 < 4.0, "{mid:?}");
        assert_eq!(*pts.last().unwrap(), (10.0, 5.0));
        // quadratic: control (5,−5) pulls the midpoint to y = 0.25·5+0.5·(−5)+0.25·5 = 0
        let SvgShapeKind::Path { contours } = &doc.shapes[1].kind else {
            panic!()
        };
        let pts = &contours[1 - 1].pts;
        let mid = pts[pts.len() / 2];
        assert!(mid.1 < 1.0, "quad pulled up: {mid:?}");
    }

    #[test]
    fn smooth_and_relative_curve_forms_parse() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        // s/S reflect the previous cubic control; t/T the quadratic one
        let doc = parse_svg(
            r#"<svg viewBox="0 0 20 10">
                 <path d="M0 5 c 0 -5, 10 -5, 10 0 s 10 5, 10 0" fill="none" stroke="black"/>
               </svg>"#,
        )
        .unwrap();
        let SvgShapeKind::Path { contours } = &doc.shapes[0].kind else {
            panic!()
        };
        assert_eq!(*contours[0].pts.last().unwrap(), (20.0, 5.0));
    }

    #[test]
    fn arcs_flatten_to_the_endpoint_through_the_sweep() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r#"<svg viewBox="0 0 10 10">
                 <path d="M1 5 A 4 4 0 0 1 9 5" fill="none" stroke="black"/>
               </svg>"#,
        )
        .unwrap();
        let SvgShapeKind::Path { contours } = &doc.shapes[0].kind else {
            panic!()
        };
        let pts = &contours[0].pts;
        assert_eq!(*pts.last().unwrap(), (9.0, 5.0), "lands on the endpoint");
        // sweep=1 goes clockwise (screen coords): the arc top passes near y=1
        let top = pts.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        assert!(top < 1.5, "arc bows through the top: {top}");
    }

    // --- fill rules + holes ---

    #[test]
    fn multi_subpath_fill_makes_holes_under_evenodd() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        // outer square + inner square, same winding: evenodd punches the hole
        let svg = |rule: &str| {
            parse_svg(&format!(
                r#"<svg viewBox="0 0 12 12">
                     <path fill-rule="{rule}" d="M1 1 H11 V11 H1 Z M4 4 H8 V8 H4 Z"/>
                   </svg>"#
            ))
            .unwrap()
        };
        let eo = rasterize_svg(&svg("evenodd"), 12, 12);
        assert_eq!(px(&eo, 12, 2, 6)[3], 0xff, "ring is filled");
        assert_eq!(px(&eo, 12, 6, 6)[3], 0, "evenodd punches the hole");
        // nonzero with the same winding fills straight through
        let nz = rasterize_svg(&svg("nonzero"), 12, 12);
        assert_eq!(px(&nz, 12, 6, 6)[3], 0xff, "nonzero same-winding fills");
    }

    // --- strokes on every shape ---

    #[test]
    fn circles_and_rects_take_strokes_now() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r#"<svg viewBox="0 0 16 16">
                 <circle cx="8" cy="8" r="6" fill="none" stroke="black" stroke-width="2"/>
               </svg>"#,
        )
        .unwrap();
        let buf = rasterize_svg(&doc, 16, 16);
        assert_eq!(px(&buf, 16, 8, 2)[3], 0xff, "on the ring");
        assert_eq!(px(&buf, 16, 8, 8)[3], 0, "hollow center");
        assert_eq!(px(&buf, 16, 0, 0)[3], 0, "outside");
    }

    // --- gradients ---

    #[test]
    fn linear_gradient_ramps_across_the_bbox() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r##"<svg viewBox="0 0 16 8">
                  <defs>
                    <linearGradient id="g">
                      <stop offset="0" stop-color="#000000"/>
                      <stop offset="1" stop-color="#ffffff"/>
                    </linearGradient>
                  </defs>
                  <rect width="16" height="8" fill="url(#g)"/>
                </svg>"##,
        )
        .unwrap();
        assert_eq!(doc.gradients.len(), 1);
        assert_eq!(doc.shapes[0].fill, Some(Paint::Gradient(0)));
        let buf = rasterize_svg(&doc, 16, 8);
        let l = px(&buf, 16, 1, 4)[0];
        let m = px(&buf, 16, 8, 4)[0];
        let r = px(&buf, 16, 14, 4)[0];
        assert!(l < 40, "left end dark: {l}");
        assert!(r > 215, "right end light: {r}");
        assert!(l < m && m < r, "monotonic ramp: {l} {m} {r}");
        assert_eq!(px(&buf, 16, 8, 4)[3], 0xff, "fully opaque");
    }

    #[test]
    fn radial_gradient_is_light_in_the_center() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r##"<svg viewBox="0 0 16 16">
                  <radialGradient id="r">
                    <stop offset="0" stop-color="#ffffff"/>
                    <stop offset="100%" stop-color="#000000"/>
                  </radialGradient>
                  <rect width="16" height="16" fill="url(#r)"/>
                </svg>"##,
        )
        .unwrap();
        let buf = rasterize_svg(&doc, 16, 16);
        let center = px(&buf, 16, 8, 8)[0];
        let corner = px(&buf, 16, 1, 1)[0];
        assert!(center > 200, "center light: {center}");
        assert!(corner < 80, "corner dark: {corner}");
    }

    // --- opacity ---

    #[test]
    fn opacity_scales_the_composited_alpha() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc =
            parse_svg(r#"<svg viewBox="0 0 4 4"><rect width="4" height="4" opacity="0.5"/></svg>"#)
                .unwrap();
        let buf = rasterize_svg(&doc, 4, 4);
        let a = px(&buf, 4, 2, 2)[3];
        assert!((120..=136).contains(&a), "≈50% alpha, got {a}");
    }

    // --- rasterizer basics (unchanged behavior) ---

    #[test]
    fn rasterizes_a_filled_rect_with_transparent_outside() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r##"<svg viewBox="0 0 8 8"><rect x="2" y="2" width="4" height="4" fill="#ff0000"/></svg>"##,
        )
        .unwrap();
        let buf = rasterize_svg(&doc, 8, 8);
        assert_eq!(
            px(&buf, 8, 4, 4),
            [0xff, 0, 0, 0xff],
            "inside is opaque red"
        );
        assert_eq!(px(&buf, 8, 0, 0)[3], 0, "outside stays transparent");
    }

    #[test]
    fn later_shapes_composite_over_earlier_ones() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r##"<svg viewBox="0 0 4 4">
                 <rect width="4" height="4" fill="#0000ff"/>
                 <rect width="2" height="4" fill="#ff0000"/>
               </svg>"##,
        )
        .unwrap();
        let buf = rasterize_svg(&doc, 4, 4);
        assert_eq!(px(&buf, 4, 1, 1), [0xff, 0, 0, 0xff], "red over blue");
        assert_eq!(
            px(&buf, 4, 3, 1),
            [0, 0, 0xff, 0xff],
            "blue where uncovered"
        );
    }

    #[test]
    fn markup_without_svg_size_is_an_error() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        assert!(parse_svg("<svg><rect width=\"2\" height=\"2\"/></svg>").is_err());
        assert!(parse_svg("just text").is_err());
    }

    // --- text through the shaper ---

    #[test]
    fn text_rasterizes_through_the_shaper_and_respects_anchor() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r#"<svg viewBox="0 0 64 24">
                 <text x="32" y="16" font-size="14" text-anchor="middle" fill="black">Hi</text>
               </svg>"#,
        )
        .unwrap();
        assert_eq!(doc.shapes.len(), 1);
        let SvgShapeKind::Text {
            anchor, content, ..
        } = &doc.shapes[0].kind
        else {
            panic!("text shape parsed");
        };
        assert_eq!(*anchor, TextAnchor::Middle);
        assert_eq!(content, "Hi");
        // without a shaper, text is skipped (blank buffer)
        let blank = rasterize_svg(&doc, 64, 24);
        assert!(blank.iter().all(|b| *b == 0));
        // with the shaper, ink lands near the anchor
        let mut shaper = TextShaper::new();
        let buf = rasterize_svg_with_text(&doc, 64, 24, &mut shaper);
        let ink: u32 = buf.chunks_exact(4).map(|p| p[3] as u32).sum();
        assert!(ink > 0, "glyph coverage landed");
        // middle-anchored: ink is roughly centered — compare left/right halves
        let half = |range: std::ops::Range<u32>| -> u32 {
            let mut s = 0;
            for y in 0..24 {
                for x in range.clone() {
                    s += px(&buf, 64, x, y)[3] as u32;
                }
            }
            s
        };
        let (l, r) = (half(0..32), half(32..64));
        let total = (l + r) as f32;
        assert!(
            (l as f32 / total) > 0.25 && (l as f32 / total) < 0.75,
            "{l} vs {r}"
        );
    }

    // --- the widget ---

    #[test]
    fn svg_widget_emits_image_quad_with_alt_name_and_hover_text() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, id) = build_one(
            runtime,
            Svg::new(
                r##"<svg viewBox="0 0 12 12"><circle cx="6" cy="6" r="5" fill="#3366cc"/></svg>"##,
            )
            .alt("logo"),
        );
        let a = scene.a11y(id).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::Image);
        assert_eq!(a.name.as_deref(), Some("logo"));
        assert!(runtime.with(|runtime| runtime.borrow().hover_tooltips.contains_key(id)));
        let prims = &scene.paint(id).unwrap().primitives;
        let Primitive::ImageQuad { rect, atlas_uv, .. } = prims[0] else {
            panic!("expected an ImageQuad, got {:?}", prims[0]);
        };
        assert_eq!((rect.width, rect.height), (12.0, 12.0));
        assert_eq!((atlas_uv.width, atlas_uv.height), (12.0, 12.0));
        assert!(!scene.images().is_empty());
    }

    #[test]
    fn themed_svg_masks_and_uses_the_text_token() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let selected = crate::Theme {
            text: Color::rgb(0x12, 0x34, 0x56),
            ..crate::Theme::default()
        };
        let (scene, id) = crate::with_theme(runtime, selected, || {
            build_one(
                runtime,
                Svg::new(r#"<svg viewBox="0 0 8 8"><rect width="8" height="8"/></svg>"#).themed(),
            )
        });
        assert!(matches!(
            scene.paint(id).unwrap().primitives[0],
            Primitive::ImageQuad { tint, .. } if tint == selected.text
        ));
    }

    #[test]
    fn svg_widget_rasterizes_at_physical_scale() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(64, 64);
        let id = {
            let mut ctx = BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 2.0,
            };
            Box::new(Svg::new(
                r#"<svg viewBox="0 0 10 10"><rect width="10" height="10"/></svg>"#,
            ))
            .build(&mut ctx, None)
        };
        scene.set_root(id);
        let Primitive::ImageQuad { rect, atlas_uv, .. } = scene.paint(id).unwrap().primitives[0]
        else {
            panic!("expected an ImageQuad");
        };
        assert_eq!((rect.width, rect.height), (10.0, 10.0));
        assert_eq!((atlas_uv.width, atlas_uv.height), (20.0, 20.0));
    }

    #[test]
    fn cached_svg_instances_share_raster_job_and_atlas_rect_across_tints() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        const MARKUP: &str = r#"<svg viewBox="0 0 10 10"><rect width="10" height="10"/></svg>"#;
        reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(64, 64);
        let root = scene.insert(WidgetKind::Row, None);
        let (a, b) = {
            let mut ctx = BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            let key = SvgCacheKey::new("cache-test", "shared", "plain");
            let a = Box::new(
                Svg::new(MARKUP)
                    .cache(key.clone())
                    .mask()
                    .tint(Color::rgb(255, 0, 0)),
            )
            .build(&mut ctx, Some(root));
            let b = Box::new(
                Svg::new(MARKUP)
                    .cache(key)
                    .mask()
                    .tint(Color::rgb(0, 0, 255)),
            )
            .build(&mut ctx, Some(root));
            (a, b)
        };
        scene.set_root(root);

        assert_eq!(scene.images().cached_len(), 1);
        assert_eq!(
            pending_svg_rasters(runtime,),
            1,
            "only the cache-miss owner submits work"
        );
        let Primitive::ImageQuad {
            atlas_uv: uv_a,
            tint: tint_a,
            ..
        } = scene.paint(a).unwrap().primitives[0]
        else {
            panic!("first icon quad");
        };
        let Primitive::ImageQuad {
            atlas_uv: uv_b,
            tint: tint_b,
            ..
        } = scene.paint(b).unwrap().primitives[0]
        else {
            panic!("second icon quad");
        };
        assert_eq!(uv_a, uv_b, "both instances sample one GPU atlas region");
        assert_ne!(tint_a, tint_b, "tint remains per-instance draw data");
        settle_svg_rasters(runtime, &mut scene);
        let stride = scene.images().width() as usize * 4;
        let mut found_ink = false;
        for y in uv_a.y as usize..(uv_a.y + uv_a.height) as usize {
            for x in uv_a.x as usize..(uv_a.x + uv_a.width) as usize {
                let index = y * stride + x * 4;
                let pixel = &scene.images().pixels()[index..index + 4];
                if pixel[3] != 0 {
                    assert_eq!(&pixel[..3], &[0xff, 0xff, 0xff]);
                    found_ink = true;
                }
            }
        }
        assert!(found_ink, "mask raster contains visible coverage");
    }

    #[test]
    fn cached_svg_raster_survives_remount_without_new_worker_job() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        const MARKUP: &str = r#"<svg viewBox="0 0 9 9"><circle cx="4.5" cy="4.5" r="4"/></svg>"#;
        let key = SvgCacheKey::new("cache-test", "remount", "unique");
        let (mut first, _) = build_one(runtime, Svg::new(MARKUP).cache(key.clone()));
        settle_svg_rasters(runtime, &mut first);
        assert_eq!(pending_svg_rasters(runtime,), 0);

        let (second, _) = build_one(runtime, Svg::new(MARKUP).cache(key));
        assert_eq!(
            pending_svg_rasters(runtime,),
            0,
            "process CPU cache avoids rasterizing again"
        );
        assert_eq!(second.images().cached_len(), 1);
    }

    #[test]
    fn compact_material_icon_path_rasterizes_full_shape() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let doc = parse_svg(
            r#"<svg viewBox="0 0 24 24"><path d="m12 21.35-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z"/></svg>"#,
        )
        .unwrap();
        let pixels = rasterize_svg(&doc, 40, 40);
        let ink: Vec<(usize, usize)> = pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[3] > 0)
            .map(|(index, _)| (index % 40, index / 40))
            .collect();
        let min_x = ink.iter().map(|point| point.0).min().unwrap();
        let max_x = ink.iter().map(|point| point.0).max().unwrap();
        let min_y = ink.iter().map(|point| point.1).min().unwrap();
        let max_y = ink.iter().map(|point| point.1).max().unwrap();
        assert!(
            max_x - min_x > 28 && max_y - min_y > 28,
            "material heart should fill most of its 40px raster; bbox={min_x},{min_y}..{max_x},{max_y}"
        );
    }

    /// The async pipeline's determinism contract (SOUL §7.3): pixels landed via
    /// the worker pool + [`settle_svg_rasters`] are byte-identical to the
    /// synchronous rasterizer's output, written exactly into the reserved rect.
    #[test]
    fn async_raster_lands_identical_pixels_via_settle() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        const MARKUP: &str =
            r##"<svg viewBox="0 0 12 12"><circle cx="6" cy="6" r="5" fill="#3366cc"/></svg>"##;
        let (mut scene, id) = build_one(runtime, Svg::new(MARKUP));
        let Primitive::ImageQuad { atlas_uv, .. } = scene.paint(id).unwrap().primitives[0] else {
            panic!("expected an ImageQuad");
        };
        settle_svg_rasters(runtime, &mut scene);
        assert_eq!(pending_svg_rasters(runtime,), 0);
        let expected = rasterize_svg(&parse_svg(MARKUP).unwrap(), 12, 12);
        let atlas = scene.images();
        let stride = atlas.width() as usize * 4;
        let (ax, ay) = (atlas_uv.x as usize, atlas_uv.y as usize);
        for row in 0..12usize {
            let got = &atlas.pixels()[(ay + row) * stride + ax * 4..][..12 * 4];
            assert_eq!(got, &expected[row * 12 * 4..][..12 * 4], "row {row}");
        }
    }

    /// A completion that outlives its mount (reset bumped the generation) is
    /// drained but never written — the reserved region stays transparent and
    /// nothing lands in a reused WidgetId's rect.
    #[test]
    fn stale_rasters_from_a_prior_mount_never_land() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, id) = build_one(
            runtime,
            Svg::new(r##"<svg viewBox="0 0 8 8"><rect width="8" height="8" fill="red"/></svg>"##),
        );
        let Primitive::ImageQuad { atlas_uv, .. } = scene.paint(id).unwrap().primitives[0] else {
            panic!("expected an ImageQuad");
        };
        reset(runtime); // remount semantics: the in-flight raster is now stale
        let landed = settle_svg_rasters(runtime, &mut scene);
        assert_eq!(landed, 0, "stale completion must be dropped");
        assert_eq!(pending_svg_rasters(runtime,), 0, "but still drained");
        let atlas = scene.images();
        let stride = atlas.width() as usize * 4;
        let (ax, ay) = (atlas_uv.x as usize, atlas_uv.y as usize);
        for row in 0..8usize {
            let got = &atlas.pixels()[(ay + row) * stride + ax * 4..][..8 * 4];
            assert!(got.iter().all(|b| *b == 0), "row {row} stays transparent");
        }
    }

    #[test]
    fn bad_markup_falls_back_to_placeholder() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, id) = build_one(runtime, Svg::new("<div>not svg</div>").alt("broken"));
        let prims = &scene.paint(id).unwrap().primitives;
        assert!(matches!(prims[0], Primitive::SolidRect { .. }));
        assert!(scene.images().is_empty());
        assert_eq!(scene.a11y(id).unwrap().name.as_deref(), Some("broken"));
    }
}
