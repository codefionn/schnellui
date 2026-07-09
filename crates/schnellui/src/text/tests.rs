use crate::text::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// True if two non-empty rects share a texel.
    fn overlaps(a: &AtlasRect, b: &AtlasRect) -> bool {
        !a.is_empty()
            && !b.is_empty()
            && a.x < b.right()
            && b.x < a.right()
            && a.y < b.bottom()
            && b.y < a.bottom()
    }

    #[test]
    fn embedded_font_is_a_real_ttf() {
        // sfnt magic for a TrueType outline font is 0x00010000.
        assert!(EMBEDDED_FONT.len() > 4);
        assert_eq!(&EMBEDDED_FONT[0..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn atlas_dirty_union_and_take() {
        let mut a = GlyphAtlas::new(64, 64);
        assert!(a.take_dirty().is_none());
        a.mark_dirty(AtlasRect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        });
        a.mark_dirty(AtlasRect {
            x: 10,
            y: 10,
            width: 4,
            height: 4,
        });
        let d = a.take_dirty().unwrap();
        assert_eq!(
            d,
            AtlasRect {
                x: 0,
                y: 0,
                width: 14,
                height: 14
            }
        );
        assert!(a.take_dirty().is_none()); // cleared after take
    }

    #[test]
    fn atlas_shelf_allocation() {
        let mut a = GlyphAtlas::new(16, 16);
        let r1 = a.allocate(8, 8).unwrap();
        assert_eq!(
            r1,
            AtlasRect {
                x: 0,
                y: 0,
                width: 8,
                height: 8
            }
        );
        let r2 = a.allocate(8, 8).unwrap();
        assert_eq!(
            r2,
            AtlasRect {
                x: 8,
                y: 0,
                width: 8,
                height: 8
            }
        );
        // next glyph wraps to a new shelf
        let r3 = a.allocate(8, 8).unwrap();
        assert_eq!(
            r3,
            AtlasRect {
                x: 0,
                y: 8,
                width: 8,
                height: 8
            }
        );
        // full
        assert!(a.allocate(8, 16).is_none());
    }

    #[test]
    fn atlas_backing_is_r8_sized() {
        let a = GlyphAtlas::new(10, 5);
        assert_eq!(a.pixels().len(), 50);
    }

    #[test]
    fn atlas_rect_overlap() {
        let a = AtlasRect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        let b = AtlasRect {
            x: 3,
            y: 3,
            width: 4,
            height: 4,
        };
        let c = AtlasRect {
            x: 4,
            y: 0,
            width: 4,
            height: 4,
        }; // touches at edge, no overlap
        let empty = AtlasRect::default();
        assert!(overlaps(&a, &b));
        assert!(!overlaps(&a, &c));
        assert!(!overlaps(&a, &empty));
    }

    #[test]
    fn write_coverage_copies_rows_and_marks_dirty() {
        let mut atlas = GlyphAtlas::new(8, 8);
        let rect = AtlasRect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        atlas.write_coverage(rect, &[10, 20, 30, 40]);
        // row 0 of the glyph -> atlas (1,1),(2,1)
        assert_eq!(atlas.pixels()[8 + 1], 10);
        assert_eq!(atlas.pixels()[8 + 2], 20);
        assert_eq!(atlas.pixels()[2 * 8 + 1], 30);
        assert_eq!(atlas.pixels()[2 * 8 + 2], 40);
        assert_eq!(atlas.take_dirty(), Some(rect));
    }

    #[test]
    fn shaping_yields_glyphs_with_advancing_x() {
        let mut shaper = TextShaper::new();
        let shaped = shaper.shape("Hello", 24.0, None);
        // Five latin letters -> five glyphs, all real (not .notdef) & advancing.
        assert_eq!(
            shaped.glyphs.len(),
            5,
            "expected one glyph per ASCII letter"
        );
        let mut pen = 0.0f32;
        for g in &shaped.glyphs {
            assert_ne!(g.glyph_id, 0, "resolved a real glyph, not .notdef");
            assert!(g.x_advance > 0.0, "each glyph advances the pen");
            pen += g.x_advance;
        }
        assert!(shaped.width > 0.0);
        assert!(shaped.height > 0.0);
        assert!(shaped.baseline > 0.0);
        // Accumulated advances approximate the reported width (quantized).
        assert!(
            (pen - shaped.width).abs() <= 2.0,
            "pen {pen} vs width {}",
            shaped.width
        );
    }

    #[test]
    fn measure_width_grows_with_text() {
        let mut shaper = TextShaper::new();
        let (w1, h1) = shaper.measure("i", 20.0, None);
        let (w4, h4) = shaper.measure("iiii", 20.0, None);
        let (w_long, _) = shaper.measure("iiiiiiiiii", 20.0, None);
        assert!(w1 > 0.0 && w4 > 0.0);
        assert!(w4 > w1, "more text is wider: {w4} > {w1}");
        assert!(w_long > w4, "still more text is wider: {w_long} > {w4}");
        // Single-line height is size-driven, not text-length-driven.
        assert!((h1 - h4).abs() < 0.001, "single-line height is stable");
    }

    #[test]
    fn atlas_packs_digits_without_overlap() {
        let mut shaper = TextShaper::new();
        let mut atlas = GlyphAtlas::new(256, 256);
        let size = 32u32;

        let mut rects: Vec<AtlasRect> = Vec::new();
        for d in b'0'..=b'9' {
            let s = (d as char).to_string();
            let shaped = shaper.shape(&s, size as f32, None);
            assert_eq!(shaped.glyphs.len(), 1, "one glyph per digit");
            let gid = shaped.glyphs[0].glyph_id;
            let key = shaper.glyph_key_in(EMBEDDED_FONT_ID, gid, size);
            let rg = shaper.rasterize_glyph(key, &mut atlas);
            assert!(
                !rg.rect.is_empty(),
                "digit '{}' rasterized to a non-empty rect",
                d as char
            );
            rects.push(rg.rect);
        }
        // No two digit slots overlap.
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                assert!(
                    !overlaps(&rects[i], &rects[j]),
                    "digit rects {i} {:?} and {j} {:?} overlap",
                    rects[i],
                    rects[j]
                );
            }
        }
        // Something was written -> the atlas has a pending dirty region.
        assert!(atlas.take_dirty().is_some());
    }

    #[test]
    fn same_input_twice_is_identical() {
        let mut shaper = TextShaper::new();
        let a = shaper.shape("schnell 0123", 18.0, Some(200.0));
        let b = shaper.shape("schnell 0123", 18.0, Some(200.0));
        assert_eq!(a.glyphs.as_slice(), b.glyphs.as_slice());
        assert_eq!(a.width.to_bits(), b.width.to_bits());
        assert_eq!(a.height.to_bits(), b.height.to_bits());
        assert_eq!(a.baseline.to_bits(), b.baseline.to_bits());
    }

    #[test]
    fn rasterize_is_cached_and_idempotent() {
        let mut shaper = TextShaper::new();
        let mut atlas = GlyphAtlas::new(128, 128);
        let shaped = shaper.shape("A", 28.0, None);
        let key = shaper.glyph_key_in(EMBEDDED_FONT_ID, shaped.glyphs[0].glyph_id, 28);
        let r1 = shaper.rasterize_glyph(key, &mut atlas).rect;
        // clear the dirty flag; a cached re-rasterize must NOT dirty again.
        let _ = atlas.take_dirty();
        let r2 = shaper.rasterize_glyph(key, &mut atlas).rect;
        assert_eq!(r1, r2, "same key -> same atlas rect");
        assert!(!r1.is_empty());
        assert!(atlas.take_dirty().is_none(), "cache hit re-writes nothing");
    }

    #[test]
    fn word_wrap_breaks_at_spaces_not_midword() {
        let mut shaper = TextShaper::new();
        let one = shaper.measure("wwww", 20.0, None).0;
        // Width fits exactly one word (+slack), never two.
        let opts = ShapeOptions::new(20.0)
            .max_width(Some(one + 4.0))
            .wrap(WrapMode::Word);
        let s = shaper.shape_with("wwww wwww wwww", &opts);
        assert_eq!(
            s.line_count(),
            3,
            "three words wrap to three lines at spaces"
        );

        // A single unbroken word never splits under Word wrap, even at tiny width.
        let long = shaper.shape_with(
            "wwwwwwwwww",
            &ShapeOptions::new(20.0)
                .max_width(Some(one / 2.0))
                .wrap(WrapMode::Word),
        );
        assert_eq!(long.line_count(), 1, "Word wrap never breaks mid-word");
    }

    #[test]
    fn anywhere_breaks_long_unbroken_string() {
        let mut shaper = TextShaper::new();
        let word = "wwwwwwwwwwwwwwww";
        let full = shaper.measure(word, 18.0, None).0;
        let s = shaper.shape_with(
            word,
            &ShapeOptions::new(18.0)
                .max_width(Some(full / 2.0))
                .wrap(WrapMode::Anywhere),
        );
        assert!(
            s.line_count() >= 2,
            "Anywhere breaks a long unbroken string (got {} lines)",
            s.line_count()
        );
        // The same input under Word wrap stays a single (overflowing) line.
        let w = shaper.shape_with(
            word,
            &ShapeOptions::new(18.0)
                .max_width(Some(full / 2.0))
                .wrap(WrapMode::Word),
        );
        assert_eq!(
            w.line_count(),
            1,
            "Word wrap keeps an unbroken run on one line"
        );
    }

    #[test]
    fn nowrap_yields_one_line_regardless_of_width() {
        let mut shaper = TextShaper::new();
        let text = "hello world foo bar";
        for w in [1.0f32, 10.0, 50.0] {
            let s = shaper.shape_with(
                text,
                &ShapeOptions::new(20.0)
                    .max_width(Some(w))
                    .wrap(WrapMode::NoWrap),
            );
            assert_eq!(s.line_count(), 1, "NoWrap stays one line at width {w}");
        }
    }

    #[test]
    fn wrapped_height_is_line_count_times_line_height() {
        let mut shaper = TextShaper::new();
        let one = shaper.measure("wwww", 20.0, None).0;
        let s = shaper.shape_with(
            "wwww wwww wwww",
            &ShapeOptions::new(20.0)
                .max_width(Some(one + 4.0))
                .wrap(WrapMode::Word),
        );
        assert_eq!(s.line_count(), 3);
        let lh = s.lines[0].height;
        assert!(lh > 0.0, "line height is positive");
        assert!(
            (s.height - lh * 3.0).abs() <= 0.5,
            "total height {} ≈ 3 × line_height {}",
            s.height,
            lh
        );
        for l in &s.lines {
            assert!(
                (l.height - lh).abs() < 0.001,
                "all lines share the line height"
            );
        }
    }

    #[test]
    fn alignment_shifts_line_x_origins() {
        let mut shaper = TextShaper::new();
        let one = shaper.measure("wwww", 20.0, None).0;
        // Wide enough that each line (one word ≈ `one`) has slack to shift.
        let width = one + 40.0;
        let base = ShapeOptions::new(20.0)
            .max_width(Some(width))
            .wrap(WrapMode::Word);

        let start = shaper.shape_with("wwww wwww", &base.align(TextAlign::Start));
        let center = shaper.shape_with("wwww wwww", &base.align(TextAlign::Center));
        let end = shaper.shape_with("wwww wwww", &base.align(TextAlign::End));

        assert!(start.line_count() >= 1);
        assert!(
            start.lines[0].x.abs() < 0.001,
            "start x ≈ 0 (got {})",
            start.lines[0].x
        );
        assert!(
            end.lines[0].x > start.lines[0].x + 1.0,
            "End shifts the line right: end {} > start {}",
            end.lines[0].x,
            start.lines[0].x
        );
        assert!(
            center.lines[0].x > start.lines[0].x + 0.5 && center.lines[0].x < end.lines[0].x,
            "Center sits between Start and End: start {} < center {} < end {}",
            start.lines[0].x,
            center.lines[0].x,
            end.lines[0].x
        );
    }

    #[test]
    fn ellipsis_fits_width_and_ends_with_ellipsis() {
        let mut shaper = TextShaper::new();
        let text = "hello world this is a long label";
        let full = shaper.measure(text, 20.0, None).0;
        let max = full / 2.0;
        let ell_gid = shaper.shape("…", 20.0, None).glyphs[0].glyph_id;

        let s = shaper.truncate_to_width(text, 20.0, max);
        assert_eq!(s.line_count(), 1, "truncation is single-line");
        assert!(s.width <= max, "truncated width {} <= max {}", s.width, max);
        assert_eq!(
            s.glyphs.last().unwrap().glyph_id,
            ell_gid,
            "truncated result ends with the ellipsis glyph"
        );

        // Fast path: text that already fits is returned unchanged (no ellipsis).
        let fits = shaper.truncate_to_width("hi", 20.0, 10_000.0);
        assert_ne!(
            fits.glyphs.last().unwrap().glyph_id,
            ell_gid,
            "a fitting string is not ellipsized"
        );
    }

    #[test]
    fn wrapped_shape_is_deterministic() {
        let mut shaper = TextShaper::new();
        let opts = ShapeOptions::new(18.0)
            .max_width(Some(120.0))
            .wrap(WrapMode::Word)
            .align(TextAlign::Center);
        let a = shaper.shape_with("schnell fast ui zero alloc", &opts);
        let b = shaper.shape_with("schnell fast ui zero alloc", &opts);
        assert_eq!(a.glyphs.as_slice(), b.glyphs.as_slice());
        assert_eq!(a.width.to_bits(), b.width.to_bits());
        assert_eq!(a.height.to_bits(), b.height.to_bits());
        assert_eq!(a.line_count(), b.line_count());
        assert!(a.line_count() >= 2, "the sample wraps to multiple lines");
        for (la, lb) in a.lines.iter().zip(b.lines.iter()) {
            assert_eq!(la, lb, "per-line origins are identical across runs");
        }
    }

    #[test]
    fn every_embedded_face_is_a_real_ttf() {
        for bytes in [
            EMBEDDED_FONT,
            EMBEDDED_FONT_BOLD,
            EMBEDDED_FONT_ITALIC,
            EMBEDDED_FONT_BOLD_ITALIC,
            EMBEDDED_FONT_MONO,
            EMBEDDED_FONT_MONO_BOLD,
            EMBEDDED_FONT_NERD_SYMBOLS,
            EMBEDDED_FONT_UNICODE_SYMBOLS,
        ] {
            assert!(bytes.len() > 4);
            assert_eq!(&bytes[0..4], &[0x00, 0x01, 0x00, 0x00]);
        }
    }

    #[test]
    fn monospace_shapes_extended_nerd_and_powerline_glyphs_with_bundled_fallback() {
        let mut shaper = TextShaper::new();
        // Powerline separator, Codicon terminal, and an extended Material icon.
        let text = "\u{e0b0}\u{ea85}\u{f0a9e}";
        let shaped = shaper.shape_with(text, &ShapeOptions::new(20.0).face(FontFace::Mono));
        assert_eq!(shaped.glyphs.len(), 3);
        assert!(shaped.glyphs.iter().all(|glyph| glyph.glyph_id != 0));
        assert!(shaped
            .glyphs
            .iter()
            .all(|glyph| glyph.font == NERD_SYMBOLS_FONT_ID));
    }

    #[test]
    fn monospace_shapes_standard_prompt_arrows_with_bundled_symbol_fallback() {
        let mut shaper = TextShaper::new();
        // Fish Pure uses these arrows to advertise an ahead/behind Git branch.
        let text = "\u{21e1}\u{21e3}";
        let shaped = shaper.shape_with(
            text,
            &ShapeOptions::new(20.0)
                .wrap(WrapMode::NoWrap)
                .face(FontFace::Mono),
        );
        assert_eq!(shaped.glyphs.len(), 2);
        assert!(shaped.glyphs.iter().all(|glyph| glyph.glyph_id != 0));
        assert!(shaped
            .glyphs
            .iter()
            .all(|glyph| glyph.font == UNICODE_SYMBOLS_FONT_ID));
    }

    #[test]
    fn named_monospace_family_is_discoverable_and_rasterizes() {
        let mut shaper = TextShaper::new();
        assert!(shaper
            .monospace_font_families()
            .iter()
            .any(|family| family == "Liberation Mono"));

        let shaped = shaper.shape_with_family(
            "M",
            &ShapeOptions::new(20.0).wrap(WrapMode::NoWrap),
            "Liberation Mono",
        );
        let glyph = shaped.glyphs.first().expect("named family shapes M");
        let mut atlas = GlyphAtlas::new(64, 64);
        let raster = shaper.rasterize_glyph(
            shaper.glyph_key_in(glyph.font, glyph.glyph_id, 20),
            &mut atlas,
        );
        assert!(!raster.rect.is_empty());
    }

    #[test]
    fn named_monospace_family_falls_back_for_the_fish_pure_prompt_symbol() {
        let mut shaper = TextShaper::new();
        let shaped = shaper.shape_with_family(
            "\u{276f}",
            &ShapeOptions::new(20.0)
                .wrap(WrapMode::NoWrap)
                .face(FontFace::Mono),
            "Liberation Mono",
        );

        let glyph = shaped
            .glyphs
            .first()
            .expect("Fish Pure prompt symbol shapes");
        assert_ne!(glyph.glyph_id, 0);
        assert_eq!(glyph.font, NERD_SYMBOLS_FONT_ID);
    }

    #[test]
    fn faces_shape_to_distinct_fonts_and_widths() {
        let mut shaper = TextShaper::new();
        let base = ShapeOptions::new(20.0);
        let plain = shaper.shape_with("Hello", &base);
        let bold = shaper.shape_with("Hello", &base.face(FontFace::SansBold));
        let mono = shaper.shape_with("Hello", &base.face(FontFace::Mono));
        assert_eq!(plain.font, FontFace::Sans.font_id());
        assert_eq!(bold.font, FontFace::SansBold.font_id());
        assert_eq!(mono.font, FontFace::Mono.font_id());
        // Bold Liberation Sans is wider than regular for the same string.
        assert!(
            bold.width > plain.width,
            "bold {} > regular {}",
            bold.width,
            plain.width
        );
        // The mono face advances every glyph identically.
        let advances: Vec<f32> = mono.glyphs.iter().map(|g| g.x_advance).collect();
        for a in &advances {
            assert!((a - advances[0]).abs() < 0.001, "monospace advances match");
        }
    }

    #[test]
    fn faces_rasterize_as_distinct_atlas_entries() {
        let mut shaper = TextShaper::new();
        let mut atlas = GlyphAtlas::new(256, 256);
        let plain = shaper.shape_with("A", &ShapeOptions::new(24.0));
        let bold = shaper.shape_with("A", &ShapeOptions::new(24.0).face(FontFace::SansBold));
        let kp = shaper.glyph_key_in(plain.font, plain.glyphs[0].glyph_id, 24);
        let kb = shaper.glyph_key_in(bold.font, bold.glyphs[0].glyph_id, 24);
        assert_ne!(kp, kb, "same char in two faces has two atlas keys");
        let rp = shaper.rasterize_glyph(kp, &mut atlas);
        let rb = shaper.rasterize_glyph(kb, &mut atlas);
        assert!(!rp.rect.is_empty() && !rb.rect.is_empty());
        assert!(!overlaps(&rp.rect, &rb.rect), "distinct atlas slots");
    }

    #[test]
    fn shape_spans_carries_per_span_face_color_and_decor() {
        let mut shaper = TextShaper::new();
        let text = "plain bold link";
        let spans = [
            SpanSpec {
                len: 6, // "plain "
                face: FontFace::Sans,
                color: [0, 0, 0, 255],
                underline: false,
                strikethrough: false,
            },
            SpanSpec {
                len: 5, // "bold "
                face: FontFace::SansBold,
                color: [0, 0, 0, 255],
                underline: false,
                strikethrough: false,
            },
            SpanSpec {
                len: 4, // "link"
                face: FontFace::Sans,
                color: [0, 0, 255, 255],
                underline: true,
                strikethrough: false,
            },
        ];
        let rich = shaper.shape_spans(text, &spans, &ShapeOptions::new(18.0));
        assert_eq!(rich.line_count(), 1);
        assert!(rich.width > 0.0 && rich.height > 0.0);
        // Some glyphs resolved to the bold face, the rest to regular.
        assert!(rich
            .glyphs
            .iter()
            .any(|g| g.font == FontFace::SansBold.font_id()));
        assert!(rich
            .glyphs
            .iter()
            .any(|g| g.font == FontFace::Sans.font_id()));
        // The link span's glyphs carry its brush color.
        assert!(rich.glyphs.iter().any(|g| g.color == [0, 0, 255, 255]));
        // Exactly one underline decoration, colored like the link.
        assert_eq!(rich.decors.len(), 1);
        let d = &rich.decors[0];
        assert_eq!(d.color, [0, 0, 255, 255]);
        assert!(d.width > 0.0 && d.thickness > 0.0);
        assert!(
            d.y > rich.lines[0].baseline,
            "underline sits below the baseline (y-down)"
        );
        // Glyph x positions advance monotonically on the single line.
        for w in rich.glyphs.windows(2) {
            assert!(w[1].x > w[0].x - 0.001);
        }
    }

    #[test]
    fn shape_spans_wraps_and_positions_absolutely() {
        let mut shaper = TextShaper::new();
        let one = shaper.measure("wwww", 20.0, None).0;
        let text = "wwww wwww wwww";
        let spans = [SpanSpec {
            len: text.len(),
            face: FontFace::Sans,
            color: [0, 0, 0, 255],
            underline: false,
            strikethrough: false,
        }];
        let rich = shaper.shape_spans(
            text,
            &spans,
            &ShapeOptions::new(20.0).max_width(Some(one + 4.0)),
        );
        assert_eq!(rich.line_count(), 3, "wraps like the uniform path");
        // Per-line glyph ranges tile the glyph list, and each later line's
        // baseline is lower (absolute y).
        let mut covered = 0u32;
        let mut last_baseline = f32::MIN;
        for l in &rich.lines {
            assert_eq!(l.glyph_start, covered);
            covered += l.glyph_count;
            assert!(l.baseline > last_baseline);
            last_baseline = l.baseline;
        }
        assert_eq!(covered as usize, rich.glyphs.len());
    }

    /// Warm re-shape pooling discipline is **unchanged by wrapping** (SOUL §4.1).
    ///
    /// parley 0.11 has a fixed, non-poolable per-call allocation floor: every
    /// `ranged_builder` call resolves the root style's font stack, which owns a
    /// small heap buffer (measured: 1 alloc / 32 bytes). That floor is identical
    /// for the legacy single-line `shape` and for a wrapped, aligned
    /// `shape_with` — this is exactly why SOUL's `text_edit` row is *small &
    /// budgeted*, not literal zero. What we hold the line on: a warm **wrapped**
    /// re-shape allocates **no more than the warm unwrapped baseline** — wrapping
    /// and alignment are allocation-neutral, and the sample's output `ShapedText`
    /// SmallVecs stay inline (≤16 glyphs, ≤4 lines) so they add nothing.
    #[cfg(feature = "count-allocations")]
    #[test]
    fn wrapped_reshape_is_alloc_neutral_vs_baseline() {
        let mut shaper = TextShaper::new();
        let a = shaper.measure("a", 16.0, None).0;
        let text = "a b c";
        let wrapped = ShapeOptions::new(16.0)
            .max_width(Some(a + 1.0))
            .wrap(WrapMode::Word)
            .align(TextAlign::Center);
        let unwrapped = ShapeOptions::new(16.0);

        // Warm both paths (first shape may grow the pooled Parley buffers, §4.1).
        let warm = shaper.shape_with(text, &wrapped);
        assert!(warm.line_count() >= 2, "sample wraps to multiple lines");
        let _ = shaper.shape_with(text, &unwrapped);

        // Baseline: the warm unwrapped single-line shape (parley's fixed floor).
        let base = allocation_counter::measure(|| {
            let s = shaper.shape_with(text, &unwrapped);
            std::hint::black_box(&s);
        });
        // Warm wrapped + aligned re-shape: must not allocate any more than that.
        let wrap = allocation_counter::measure(|| {
            let s = shaper.shape_with(text, &wrapped);
            std::hint::black_box(&s);
        });

        assert_eq!(
            wrap.count_total, base.count_total,
            "wrapping regressed allocs: wrapped {} vs baseline {} (SOUL §4.1)",
            wrap.count_total, base.count_total
        );
        assert_eq!(
            wrap.bytes_total, base.bytes_total,
            "wrapping regressed bytes: wrapped {} vs baseline {} (SOUL §4.1)",
            wrap.bytes_total, base.bytes_total
        );
        // Guard the baseline stays the small parley floor we documented (≤1 alloc);
        // if parley ever pools this, tighten both sides toward zero.
        assert!(
            base.count_total <= 1,
            "unexpected warm-shape baseline of {} allocs — parley floor changed",
            base.count_total
        );
    }

    /// The face a shaped run resolved to (the first glyph's FontId).
    fn face_of(shaper: &mut TextShaper, family: &str) -> FontId {
        let shaped = shaper.shape_with_family("Ag", &ShapeOptions::new(16.0), family);
        shaped
            .glyphs
            .first()
            .map(|g| g.font)
            .unwrap_or(EMBEDDED_FONT_ID)
    }

    #[test]
    fn forked_shaper_resolves_font_ids_like_its_donor() {
        // Two installed system families that are not the embedded faces. The test
        // needs a machine with system fonts; without two it has nothing to check.
        let mut donor = TextShaper::new();
        let base = face_of(&mut donor, "Liberation Sans");
        let families: Vec<String> = donor
            .font_families()
            .into_iter()
            .filter(|f| !f.starts_with("Liberation") && !f.contains("Symbols"))
            .collect();
        // One probe shaper hands out ids in first-seen order, so distinct ids mean
        // distinct faces.
        let mut probe = TextShaper::new();
        let mut picked = Vec::new();
        for family in &families {
            let id = face_of(&mut probe, family);
            if id != base && !picked.iter().any(|(_, i)| *i == id) {
                picked.push((family.clone(), id));
            }
            if picked.len() == 2 {
                break;
            }
        }
        let [(first, _), (second, _)] = picked.as_slice() else {
            return;
        };

        let donor_first = face_of(&mut donor, first);
        let mut fork = donor.fork();
        assert_eq!(face_of(&mut fork, first), donor_first, "same face, same id");
        let fork_second = face_of(&mut fork, second);
        assert_ne!(
            fork_second, donor_first,
            "a face the donor never met must not reuse the donor's id for another face"
        );
    }

    #[test]
    fn atlas_fork_keeps_identity_only_while_the_donor_is_unchanged() {
        let mut running = GlyphAtlas::new(64, 64);
        let rect = running.allocate(4, 4).unwrap();
        running.write_coverage(rect, &[255; 16]);

        // The donor did not write after the fork: the fork may keep its id.
        let mut fork = running.fork();
        let rect = fork.allocate(4, 4).unwrap();
        fork.write_coverage(rect, &[128; 16]);
        assert!(fork.resolve_fork(&running));
        assert_eq!(fork.id(), running.id());

        // The donor wrote into the same slot the fork used: ids must diverge.
        let mut fork = running.fork();
        let rect = fork.allocate(4, 4).unwrap();
        fork.write_coverage(rect, &[128; 16]);
        let rect = running.allocate(4, 4).unwrap();
        running.write_coverage(rect, &[7; 16]);
        assert!(!fork.resolve_fork(&running));
        assert_ne!(fork.id(), running.id());

        // A swapped (not forked) atlas keeps its identity.
        let mut moved = running.clone();
        assert!(moved.resolve_fork(&running));
    }

    /// The pre-rewrite truncation: a binary search over char boundaries that
    /// re-shapes `prefix + "…"` at every step.
    fn truncate_by_search(shaper: &mut TextShaper, text: &str, size: f32, max: f32) -> ShapedText {
        let opts = ShapeOptions::new(size).wrap(WrapMode::NoWrap);
        let full = shaper.shape_with(text, &opts);
        if text.is_empty() || full.width <= max {
            return full;
        }
        let cuts: Vec<usize> = text.char_indices().skip(1).map(|(i, _)| i).collect();
        let mut best = None;
        let (mut lo, mut hi) = (0isize, cuts.len() as isize - 1);
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let buf = format!("{}…", &text[..cuts[mid as usize]]);
            let s = shaper.shape_with(&buf, &opts);
            if s.width <= max {
                best = Some(s);
                lo = mid + 1;
            } else {
                hi = mid - 1;
            }
        }
        best.unwrap_or_else(|| shaper.shape_with("…", &opts))
    }

    #[test]
    fn truncate_matches_the_binary_search_it_replaced() {
        let mut shaper = TextShaper::new();
        let texts = [
            "hello world this is a long label",
            "WAVE AVAVAV Tyrant",
            "naïve café über straße",
            "日本語のテキストを切り詰める",
            // Base letters followed by combining marks.
            "e\u{301}e\u{301}e\u{301} a\u{308}a\u{308}a\u{308} done",
            "emoji 😀😀😀 and 👍🏽 text",
            "x",
            "mmmmmmmmmmmmmmmmmmmm",
            "שלום עולם זהו טקסט ארוך מאוד",
            "مرحبا بالعالم هذا نص عربي طويل جدا",
            "hello שלום world עולם end",
        ];
        for size in [14.0f32, 20.0] {
            for text in texts {
                let full = shaper.measure(text, size, None).0;
                let ell = shaper.measure("…", size, None).0;
                let mut widths = vec![
                    0.0,
                    ell * 0.5,
                    ell,
                    ell * 1.2,
                    full * 0.3,
                    full * 0.5,
                    full * 0.77,
                ];
                widths.extend([full - 0.01, full, full + 1.0]);
                for max in widths {
                    let new = shaper.truncate_to_width(text, size, max);
                    let old = truncate_by_search(&mut shaper, text, size, max);
                    assert_eq!(
                        new.glyphs.as_slice(),
                        old.glyphs.as_slice(),
                        "{text:?} at {size}px in {max}px"
                    );
                    assert_eq!(new.width.to_bits(), old.width.to_bits(), "{text:?} {max}");
                }
            }
        }
    }

    #[test]
    fn truncate_below_the_ellipsis_width_returns_the_ellipsis() {
        let mut shaper = TextShaper::new();
        let ell = shaper.shape("…", 20.0, None);
        let s = shaper.truncate_to_width("a long label", 20.0, ell.width * 0.5);
        assert_eq!(s.glyphs.as_slice(), ell.glyphs.as_slice());
    }

    #[test]
    fn symbol_font_ids_match_registration_order() {
        // BaseFonts::build asserts the order when it builds; this pins the ids.
        let mut shaper = TextShaper::new();
        assert_eq!(FontFace::Sans.font_id(), EMBEDDED_FONT_ID);
        assert_eq!(NERD_SYMBOLS_FONT_ID.0 as usize, 6);
        assert_eq!(UNICODE_SYMBOLS_FONT_ID.0 as usize, 7);
        assert_eq!(shaper.font_resources.len(), EMBEDDED_FACES.len());
        for (i, bytes) in EMBEDDED_FACES.iter().enumerate() {
            assert_eq!(shaper.font_resources[i].data.data(), *bytes, "face {i}");
        }
        // The ids resolved through fontique agree with the constants.
        let nerd = shaper.shape_with("\u{f015}", &ShapeOptions::new(20.0).face(FontFace::Mono));
        assert_eq!(nerd.glyphs[0].font, NERD_SYMBOLS_FONT_ID);
    }

    #[test]
    fn shape_into_matches_shape_with_and_reuses_the_buffer() {
        let mut shaper = TextShaper::new();
        let text = "the quick brown fox jumps over the lazy dog ".repeat(5);
        let opts = ShapeOptions::new(16.0).max_width(Some(300.0));
        let expect = shaper.shape_with(&text, &opts);
        let mut out = ShapedText::default();
        shaper.shape_into(&text, &opts, &mut out);
        shaper.shape_into(&text, &opts, &mut out);
        assert_eq!(out.glyphs.as_slice(), expect.glyphs.as_slice());
        assert_eq!(out.lines.as_slice(), expect.lines.as_slice());
        assert_eq!(out.width.to_bits(), expect.width.to_bits());
        // A shorter text afterwards leaves nothing of the longer one behind.
        shaper.shape_into("hi", &opts, &mut out);
        assert_eq!(out.glyphs.len(), 2);
        assert_eq!(out.line_count(), 1);
    }

    /// Shaping 200 glyphs into a reused `ShapedText` allocates nothing on the
    /// second call, where a fresh result spills its glyph storage every time.
    #[cfg(feature = "count-allocations")]
    #[test]
    fn shape_into_reused_buffer_allocates_nothing() {
        let mut shaper = TextShaper::new();
        let long = "abcdefghij".repeat(20);
        let opts = ShapeOptions::new(16.0).wrap(WrapMode::NoWrap);
        let mut out = ShapedText::default();
        shaper.shape_into(&long, &opts, &mut out);
        assert_eq!(out.glyphs.len(), 200);

        let reused = allocation_counter::measure(|| {
            shaper.shape_into(&long, &opts, &mut out);
            std::hint::black_box(&out);
        });
        let fresh = allocation_counter::measure(|| {
            let s = shaper.shape_with(&long, &opts);
            std::hint::black_box(&s);
        });
        assert_eq!(out.glyphs.len(), 200);
        assert_eq!(reused.count_total, 0, "reused buffer allocated");
        assert_eq!(reused.bytes_total, 0);
        assert!(fresh.count_total > 0, "a fresh result spills");
    }

    #[test]
    fn atlas_grow_keeps_texels_and_forces_a_full_upload() {
        let mut atlas = GlyphAtlas::new(8, 8);
        let rect = atlas.allocate(8, 8).unwrap();
        atlas.write_coverage(rect, &[200; 64]);
        let seen = atlas.generation();
        let id = atlas.id();
        assert!(atlas.allocate(4, 4).is_none());
        assert!(!atlas.has_headroom(), "a full atlas has no headroom");

        assert!(atlas.grow());
        assert_eq!((atlas.width(), atlas.height()), (8, 16));
        assert_eq!(atlas.id(), id, "growth keeps the identity");
        assert!(
            atlas.pixels()[..64].iter().all(|&p| p == 200),
            "texels kept"
        );
        assert!(atlas.pixels()[64..].iter().all(|&p| p == 0));
        assert_ne!(atlas.generation(), seen);
        assert_eq!(atlas.delta_since(seen), AtlasDelta::Full);
        assert!(atlas.has_headroom(), "growing restores headroom");
        assert!(atlas.allocate(4, 4).is_some(), "new rows are usable");
        // The dirty marker covers everything for the legacy `take_dirty` consumer.
        assert_eq!(atlas.take_dirty().map(|r| r.height), Some(16));
    }

    #[test]
    fn atlas_stops_growing_at_its_cap_and_reports_exhaustion() {
        let mut atlas = GlyphAtlas::new(4, GlyphAtlas::DEFAULT_MAX_HEIGHT);
        assert!(!atlas.grow());
        assert!(atlas.allocate(4, GlyphAtlas::DEFAULT_MAX_HEIGHT).is_some());
        assert!(atlas.allocate(4, 4).is_none());
        assert!(!atlas.has_headroom());
    }

    #[test]
    fn small_atlas_grows_so_no_glyph_is_dropped() {
        let mut shaper = TextShaper::new();
        let mut atlas = GlyphAtlas::new(32, 32);
        let text: String = ('!'..='~').collect();
        let shaped = shaper.shape(&text, 24.0, None);
        let mut rects = Vec::new();
        for g in &shaped.glyphs {
            let key = shaper.glyph_key_in(g.font, g.glyph_id, 24);
            let rg = shaper.rasterize_glyph(key, &mut atlas);
            assert!(!rg.rect.is_empty(), "glyph {} dropped", g.glyph_id);
            rects.push(rg.rect);
        }
        assert!(atlas.height() > 32, "the atlas had to grow");
        // Cached results from before a growth are still valid slots.
        for (g, first) in shaped.glyphs.iter().zip(&rects) {
            let key = shaper.glyph_key_in(g.font, g.glyph_id, 24);
            assert_eq!(shaper.rasterize_glyph(key, &mut atlas).rect, *first);
        }
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                assert!(!overlaps(&rects[i], &rects[j]));
            }
        }
    }

    #[test]
    fn a_glyph_that_cannot_be_placed_is_not_cached_as_empty() {
        let mut shaper = TextShaper::new();
        let g = shaper.shape("W", 40.0, None).glyphs[0];
        let key = shaper.glyph_key_in(g.font, g.glyph_id, 40);
        // Too narrow for the glyph at any height: cannot be fixed by growing.
        let mut tiny = GlyphAtlas::new(2, 2);
        assert!(shaper.rasterize_glyph(key, &mut tiny).rect.is_empty());
        assert_eq!(
            tiny.height(),
            2,
            "an unfixable failure must not grow the atlas"
        );
        assert_eq!(tiny.generation(), 0);
        let mut roomy = GlyphAtlas::new(128, 128);
        assert!(!shaper.rasterize_glyph(key, &mut roomy).rect.is_empty());
    }
}
