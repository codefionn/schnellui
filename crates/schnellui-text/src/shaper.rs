use crate::types::*;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use parley::fontique::{Blob, Collection, CollectionOptions, FamilyId, SourceCache};
use parley::FontData;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight,
    GenericFamily, Layout, LayoutContext, OverflowWrap, PositionedLayoutItem, StyleProperty,
    TextWrapMode,
};
use smallvec::SmallVec;
use swash::scale::image::Image;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;
use swash::FontRef;

/// The shaper/measurer, owning pooled Parley + Fontique contexts (SOUL §8.1). The
/// `LayoutContext`/`Layout` are pooled so warm shaping is amortized zero-alloc
/// (SOUL §4.1 `text_edit` budget); rasterized glyphs are cached by [`GlyphKey`].
pub struct TextShaper {
    font_cx: FontContext,
    layout_cx: LayoutContext<Brush>,
    /// Reused layout scratch (`build_into` clears + refills — §4.4).
    scratch_layout: Layout<Brush>,
    /// swash scaling caches + scratch buffers (pooled).
    scale_cx: ScaleContext,
    /// Reused glyph-image buffer (grow-only — §4.4).
    scratch_image: Image,
    /// Rasterized-glyph cache: a repeated glyph is a hit, no atlas re-write.
    glyph_cache: HashMap<GlyphKey, RasterGlyph>,
    /// The registered embedded family (kept for diagnostics / future queries).
    family_id: FamilyId,
    /// The registered embedded monospace family (Liberation Mono).
    mono_family_id: FamilyId,
    /// Bundled Nerd Fonts Symbols Mono fallback family.
    nerd_symbols_family_id: FamilyId,
    /// Bundled standard Unicode symbol fallback family.
    unicode_symbols_family_id: FamilyId,
    /// Font data used by swash, indexed by the stable [`FontId`] stored on shaped glyphs.
    font_resources: Vec<FontData>,
    /// Maps Fontique blob/index identities to the rasterizer-facing id above.
    font_ids: HashMap<(u64, u32), FontId>,
}

impl Default for TextShaper {
    fn default() -> Self {
        Self::new()
    }
}

impl TextShaper {
    /// Builds a shaper with the embedded font registered in a system-font-free
    /// Fontique collection (deterministic, SOUL §7.3), mapped as every generic
    /// family so shaping always resolves to Liberation Sans.
    pub fn new() -> TextShaper {
        // UI text still resolves through the deterministic embedded generics;
        // system families are available only to callers that explicitly name one.
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: true,
        });

        // Register every embedded face from its static bytes (no copy). The
        // four sans faces share the family name "Liberation Sans", so fontique
        // folds them into one family and resolves bold/italic within it; the
        // two mono faces fold into "Liberation Mono" the same way. Nerd Fonts
        // Symbols follows it only as fallback, retaining stable terminal metrics.
        let mut font_resources = Vec::new();
        let mut font_ids = HashMap::new();
        let mut register = |bytes: &'static [u8]| -> FamilyId {
            let blob = Blob::new(Arc::new(bytes) as Arc<dyn AsRef<[u8]> + Send + Sync>);
            let family = collection
                .register_fonts(blob.clone(), None)
                .first()
                .map(|(id, _)| *id)
                .expect("embedded Liberation face registered at least one family");
            let id = FontId(font_resources.len() as u32);
            font_ids.insert((blob.id(), 0), id);
            font_resources.push(FontData::new(blob, 0));
            family
        };
        let family_id = register(EMBEDDED_FONT);
        register(EMBEDDED_FONT_BOLD);
        register(EMBEDDED_FONT_ITALIC);
        register(EMBEDDED_FONT_BOLD_ITALIC);
        let mono_family_id = register(EMBEDDED_FONT_MONO);
        register(EMBEDDED_FONT_MONO_BOLD);
        let nerd_symbols_family_id = register(EMBEDDED_FONT_NERD_SYMBOLS);
        let unicode_symbols_family_id = register(EMBEDDED_FONT_UNICODE_SYMBOLS);

        // Point the generic families at the embedded fonts so any resolution
        // path (including the default "sans-serif") lands on them. Monospace
        // resolves to Liberation Mono — the code face rich text shapes with.
        for generic in [
            GenericFamily::SansSerif,
            GenericFamily::Serif,
            GenericFamily::SystemUi,
            GenericFamily::UiSansSerif,
        ] {
            collection.set_generic_families(generic, std::iter::once(family_id));
        }
        collection.set_generic_families(
            GenericFamily::Monospace,
            [
                mono_family_id,
                nerd_symbols_family_id,
                unicode_symbols_family_id,
            ]
            .into_iter(),
        );

        let font_cx = FontContext {
            collection,
            source_cache: SourceCache::default(),
        };

        TextShaper {
            font_cx,
            layout_cx: LayoutContext::new(),
            scratch_layout: Layout::default(),
            scale_cx: ScaleContext::new(),
            scratch_image: Image::new(),
            glyph_cache: HashMap::new(),
            family_id,
            mono_family_id,
            nerd_symbols_family_id,
            unicode_symbols_family_id,
            font_resources,
            font_ids,
        }
    }

    /// Returns installed font family names, including the bundled families.
    pub fn font_families(&mut self) -> Vec<String> {
        let mut families: Vec<String> = self
            .font_cx
            .collection
            .family_names()
            .map(str::to_owned)
            .collect();
        families.sort_by_key(|family| family.to_lowercase());
        families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        families
    }

    /// Returns installed families whose font metadata declares fixed-width metrics.
    pub fn monospace_font_families(&mut self) -> Vec<String> {
        let families = self.font_families();
        let font_cx = &mut self.font_cx;
        families
            .into_iter()
            .filter(|family| {
                font_cx
                    .collection
                    .family_by_name(family)
                    .and_then(|family| family.default_font().cloned())
                    .and_then(|font| {
                        let index = font.index();
                        font.load(Some(&mut font_cx.source_cache))
                            .map(|blob| (blob, index))
                    })
                    .is_some_and(|(blob, index)| {
                        FontRef::from_index(blob.data(), index as usize).is_some_and(|font| {
                            font.metrics(&[]).is_monospace
                                && font.charmap().map('M') != 0
                                && font.charmap().map('0') != 0
                        })
                    })
            })
            .collect()
    }

    /// The Fontique family id of the embedded font (diagnostics / tests).
    pub fn embedded_family_id(&self) -> FamilyId {
        self.family_id
    }

    /// The Fontique family id of the embedded mono family (diagnostics / tests).
    pub fn embedded_mono_family_id(&self) -> FamilyId {
        self.mono_family_id
    }

    /// The Fontique family id of the bundled Nerd Fonts Symbols fallback.
    pub fn nerd_symbols_family_id(&self) -> FamilyId {
        self.nerd_symbols_family_id
    }

    /// The Fontique family id of the bundled standard Unicode symbol fallback.
    pub fn unicode_symbols_family_id(&self) -> FamilyId {
        self.unicode_symbols_family_id
    }

    /// Shapes `text` at `size_px`, returning positioned glyphs + intrinsic size
    /// (SOUL §8.1). Runs through the pooled Parley context (warm ⇒ amortized
    /// zero-alloc). `max_width` (if given) is the wrap width in logical px.
    ///
    /// Back-compat thin wrapper: equivalent to [`TextShaper::shape_with`] with
    /// [`WrapMode::Word`] + [`TextAlign::Start`] (parley's default word wrap), so
    /// existing callers (e.g. the widgets/chart glyph path) keep their behavior.
    pub fn shape(&mut self, text: &str, size_px: f32, max_width: Option<f32>) -> ShapedText {
        self.shape_core(text, &ShapeOptions::new(size_px).max_width(max_width), None)
    }

    /// Shapes `text` with explicit [`ShapeOptions`] (wrap mode + alignment +
    /// width), returning a [`ShapedText`] whose `lines` carry correct multi-line,
    /// aligned origins and whose `width`/`height` are the widest line / total
    /// height (SOUL §8.1). Warm re-shape of same/same-length text stays amortized
    /// zero-alloc through the pooled `LayoutContext`/`Layout` (§4.1).
    pub fn shape_with(&mut self, text: &str, opts: &ShapeOptions) -> ShapedText {
        self.shape_core(text, opts, None)
    }

    /// Shapes with an explicitly selected installed font family.
    pub fn shape_with_family(
        &mut self,
        text: &str,
        opts: &ShapeOptions,
        family: &str,
    ) -> ShapedText {
        self.shape_core(text, opts, Some(family))
    }

    /// The shared shaping core (SOUL §8.1). All wrap/align config flows through
    /// here; [`shape`](Self::shape)/[`shape_with`](Self::shape_with)/
    /// [`truncate_to_width`](Self::truncate_to_width) are thin callers.
    fn shape_core(
        &mut self,
        text: &str,
        opts: &ShapeOptions,
        font_family: Option<&str>,
    ) -> ShapedText {
        // Split-borrow the pooled fields so the builder can hold `layout_cx` +
        // `font_cx` while `build_into` fills `scratch_layout` (§4.1 pooling).
        let font_cx = &mut self.font_cx;
        let layout_cx = &mut self.layout_cx;
        let layout = &mut self.scratch_layout;

        {
            // scale = 1.0 (logical == physical for deterministic shots, §7.3);
            // quantize = true rounds advances for stable, diffable output.
            let mut builder = layout_cx.ranged_builder(font_cx, text, 1.0, true);
            builder.push_default(StyleProperty::FontSize(opts.size_px));
            let family = if let Some(family) = font_family {
                FontFamily::List(Cow::Owned(vec![
                    FontFamilyName::Named(Cow::Owned(family.to_owned())),
                    FontFamilyName::Generic(GenericFamily::Monospace),
                ]))
            } else if opts.face.mono() {
                FontFamily::from(GenericFamily::Monospace)
            } else {
                FontFamily::from(GenericFamily::SansSerif)
            };
            builder.push_default(StyleProperty::FontFamily(family));
            if opts.face.bold() {
                builder.push_default(StyleProperty::FontWeight(FontWeight::BOLD));
            }
            if opts.face.italic() {
                builder.push_default(StyleProperty::FontStyle(FontStyle::Italic));
            }
            // parley defaults are `TextWrapMode::Wrap` + `OverflowWrap::Normal`,
            // i.e. `WrapMode::Word` — so that variant pushes nothing extra.
            match opts.wrap {
                WrapMode::NoWrap => {
                    builder.push_default(StyleProperty::TextWrapMode(TextWrapMode::NoWrap));
                }
                WrapMode::Word => {}
                WrapMode::Anywhere => {
                    builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::Anywhere));
                }
            }
            builder.build_into(layout, text);
        }

        layout.break_all_lines(opts.max_width);

        // `Start` leaves offsets at 0 (parley's post-break default), so skip the
        // extra pass — this also keeps `shape()` byte-identical to the pre-wrap
        // behavior. Other alignments shift each line's `metrics.offset`.
        if opts.align != TextAlign::Start {
            let alignment = match opts.align {
                TextAlign::Start => Alignment::Start,
                TextAlign::Center => Alignment::Center,
                TextAlign::End => Alignment::End,
                TextAlign::Justify => Alignment::Justify,
            };
            layout.align(alignment, AlignmentOptions::default());
        }

        let mut out = ShapedText {
            width: layout.width(),
            height: layout.height(),
            font: opts.face.font_id(),
            ..ShapedText::default()
        };

        let mut first_baseline: Option<f32> = None;
        for line in layout.lines() {
            let m = line.metrics();
            let glyph_start = out.glyphs.len() as u32;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                // Single uniform style ⇒ each GlyphRun spans its whole (line-
                // scoped) Run, so walking the run's clusters (visual order) is
                // exact — and because the Run is scoped to this line, wrapping
                // never double-counts glyphs across lines. Cluster text ranges
                // attach the source byte index to every glyph.
                let run = glyph_run.run();
                let font =
                    font_id_for_data(run.font(), &mut self.font_ids, &mut self.font_resources);
                for cluster in run.visual_clusters() {
                    let cluster_byte = cluster.text_range().start as u32;
                    for g in cluster.glyphs() {
                        out.glyphs.push(ShapedGlyph {
                            glyph_id: g.id as u16,
                            font,
                            x_advance: g.advance,
                            x_offset: g.x,
                            y_offset: g.y,
                            cluster: cluster_byte,
                        });
                    }
                }
            }
            if first_baseline.is_none() {
                first_baseline = Some(m.baseline);
            }
            out.lines.push(ShapedLine {
                x: m.offset,
                baseline: m.baseline,
                top: m.block_min_coord,
                height: m.line_height,
                glyph_start,
                glyph_count: out.glyphs.len() as u32 - glyph_start,
            });
        }
        out.baseline = first_baseline.unwrap_or(0.0);
        if font_family.is_some() {
            if let Some(glyph) = out.glyphs.first() {
                out.font = glyph.font;
            }
        }
        out
    }

    /// Shapes `text` with **per-span styles** — the rich-text path (SOUL §8.1).
    /// `spans` tile the string in order (each covering its next `len` bytes);
    /// any tail bytes not covered by a span shape in the option's default face.
    /// Faces map to real embedded weights/styles through fontique resolution;
    /// span colors ride parley's brush and come back per glyph; underline /
    /// strikethrough resolve against the run's font metrics into [`RichDecor`]s.
    ///
    /// Returned positions are **absolute** within the layout (line origin and
    /// [`TextAlign`] shift applied), so the caller emits quads by adding only
    /// its own paint origin. Runs through the same pooled `LayoutContext` /
    /// `Layout` as [`shape_with`](Self::shape_with) — warm re-shapes stay in
    /// the `text_edit` budget class (§4.1); the output `Vec`s are the caller's
    /// to cache.
    pub fn shape_spans(
        &mut self,
        text: &str,
        spans: &[SpanSpec],
        opts: &ShapeOptions,
    ) -> RichShapedText {
        let font_cx = &mut self.font_cx;
        let layout_cx = &mut self.layout_cx;
        let layout = &mut self.scratch_layout;

        {
            let mut builder = layout_cx.ranged_builder(font_cx, text, 1.0, true);
            builder.push_default(StyleProperty::FontSize(opts.size_px));
            builder.push_default(StyleProperty::FontFamily(GenericFamily::SansSerif.into()));
            match opts.wrap {
                WrapMode::NoWrap => {
                    builder.push_default(StyleProperty::TextWrapMode(TextWrapMode::NoWrap));
                }
                WrapMode::Word => {}
                WrapMode::Anywhere => {
                    builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::Anywhere));
                }
            }
            let mut at = 0usize;
            for s in spans {
                let end = (at + s.len).min(text.len());
                let range = at..end;
                at = end;
                if range.is_empty() {
                    continue;
                }
                if s.face.mono() {
                    builder.push(
                        StyleProperty::FontFamily(GenericFamily::Monospace.into()),
                        range.clone(),
                    );
                }
                if s.face.bold() {
                    builder.push(StyleProperty::FontWeight(FontWeight::BOLD), range.clone());
                }
                if s.face.italic() {
                    builder.push(StyleProperty::FontStyle(FontStyle::Italic), range.clone());
                }
                builder.push(StyleProperty::Brush(s.color), range.clone());
                if s.underline {
                    builder.push(StyleProperty::Underline(true), range.clone());
                }
                if s.strikethrough {
                    builder.push(StyleProperty::Strikethrough(true), range.clone());
                }
            }
            builder.build_into(layout, text);
        }

        layout.break_all_lines(opts.max_width);
        if opts.align != TextAlign::Start {
            let alignment = match opts.align {
                TextAlign::Start => Alignment::Start,
                TextAlign::Center => Alignment::Center,
                TextAlign::End => Alignment::End,
                TextAlign::Justify => Alignment::Justify,
            };
            layout.align(alignment, AlignmentOptions::default());
        }

        let mut out = RichShapedText {
            width: layout.width(),
            height: layout.height(),
            ..RichShapedText::default()
        };
        for line in layout.lines() {
            let m = line.metrics();
            let glyph_start = out.glyphs.len() as u32;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font =
                    font_id_for_data(run.font(), &mut self.font_ids, &mut self.font_resources);
                let style = glyph_run.style();
                let color = style.brush;
                let rm = run.metrics();
                // Decoration offsets are typographic (positive = up from the
                // baseline); screen y is down, so subtract.
                if let Some(d) = &style.underline {
                    out.decors.push(RichDecor {
                        x: glyph_run.offset(),
                        y: glyph_run.baseline() - d.offset.unwrap_or(rm.underline_offset),
                        width: glyph_run.advance(),
                        thickness: d.size.unwrap_or(rm.underline_size).max(0.5),
                        color: d.brush,
                    });
                }
                if let Some(d) = &style.strikethrough {
                    out.decors.push(RichDecor {
                        x: glyph_run.offset(),
                        y: glyph_run.baseline() - d.offset.unwrap_or(rm.strikethrough_offset),
                        width: glyph_run.advance(),
                        thickness: d.size.unwrap_or(rm.strikethrough_size).max(0.5),
                        color: d.brush,
                    });
                }
                for g in glyph_run.positioned_glyphs() {
                    out.glyphs.push(RichGlyph {
                        glyph_id: g.id as u16,
                        x: g.x,
                        y: g.y,
                        font,
                        color,
                    });
                }
            }
            out.lines.push(ShapedLine {
                x: m.offset,
                baseline: m.baseline,
                top: m.block_min_coord,
                height: m.line_height,
                glyph_start,
                glyph_count: out.glyphs.len() as u32 - glyph_start,
            });
        }
        out
    }

    /// Single-line ellipsis truncation (SOUL §8.1). Shapes `text` on one line; if
    /// it fits `max_width` the full shape is returned unchanged, otherwise the
    /// longest char-boundary prefix whose `prefix + "…"` fits `max_width` is
    /// shaped and returned (ending in the ellipsis glyph). Parley has no native
    /// ellipsis, so this is hand-rolled via a binary search over char boundaries.
    ///
    /// **Cost:** this is *not* a steady-state path. The binary search re-shapes
    /// `O(log n)` prefixes through the pooled context (each warm-cheap) but builds
    /// a small scratch `String`, so truncation **MAY allocate** — acceptable per
    /// §4.1 (truncation is a one-shot, not a re-render). Fast path (text already
    /// fits) shapes once and allocates only its output.
    pub fn truncate_to_width(&mut self, text: &str, size_px: f32, max_width: f32) -> ShapedText {
        // Force a single line; alignment is irrelevant to truncation.
        let opts = ShapeOptions::new(size_px).wrap(WrapMode::NoWrap);

        let full = self.shape_core(text, &opts, None);
        if text.is_empty() || full.width <= max_width {
            return full;
        }

        const ELLIPSIS: &str = "…";

        // Candidate cut points: every char boundary strictly inside the string
        // (prefix = text[..cut], 0 < cut < len), ascending.
        let cuts: SmallVec<[usize; 32]> = text.char_indices().skip(1).map(|(i, _)| i).collect();

        // Binary search for the largest cut whose `prefix + "…"` fits.
        let mut buf = String::new();
        let mut best: Option<ShapedText> = None;
        let mut lo: isize = 0;
        let mut hi: isize = cuts.len() as isize - 1;
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let cut = cuts[mid as usize];
            buf.clear();
            buf.push_str(&text[..cut]);
            buf.push_str(ELLIPSIS);
            let s = self.shape_core(&buf, &opts, None);
            if s.width <= max_width {
                best = Some(s);
                lo = mid + 1; // try a longer prefix
            } else {
                hi = mid - 1;
            }
        }

        match best {
            Some(s) => s,
            // Not even one char + ellipsis fits: return the ellipsis alone (best
            // effort — the caller asked for a width smaller than "x…").
            None => {
                buf.clear();
                buf.push_str(ELLIPSIS);
                self.shape_core(&buf, &opts, None)
            }
        }
    }

    /// Measures `text` at `size_px` under an optional width constraint, returning
    /// only the intrinsic `(width, height)` handed up to layout (SOUL §8.1).
    pub fn measure(&mut self, text: &str, size_px: f32, max_width: Option<f32>) -> (f32, f32) {
        let s = self.shape(text, size_px, max_width);
        (s.width, s.height)
    }

    /// Rasterizes one glyph via swash into `atlas`, marking the written sub-rect
    /// dirty (SOUL §8.1, §3.2). Returns the atlas rect the glyph landed in. A
    /// repeated key is a cache hit (idempotent, no re-write — §4.1).
    pub fn rasterize(&mut self, key: GlyphKey, atlas: &mut GlyphAtlas) -> AtlasRect {
        self.rasterize_glyph(key, atlas).rect
    }

    /// Like [`rasterize`](Self::rasterize) but returns the full [`RasterGlyph`]
    /// (atlas rect + placement bearing) the renderer needs to position the quad.
    /// Additive to the skeleton API.
    pub fn rasterize_glyph(&mut self, key: GlyphKey, atlas: &mut GlyphAtlas) -> RasterGlyph {
        if let Some(g) = self.glyph_cache.get(&key) {
            return *g;
        }

        // The key's FontId selects the embedded face's bytes (index 0 — each
        // Liberation face is a single-font file). If a face ever fails to parse
        // we cache an empty glyph so we don't retry every frame.
        let resource = self
            .font_resources
            .get(key.font.0 as usize)
            .unwrap_or(&self.font_resources[EMBEDDED_FONT_ID.0 as usize]);
        let Some(font) = FontRef::from_index(resource.data.data(), resource.index as usize) else {
            let g = RasterGlyph::default();
            self.glyph_cache.insert(key, g);
            return g;
        };

        let img = &mut self.scratch_image;
        img.clear();

        // Deterministic: outline source, no hinting, 8-bit alpha (no subpixel AA).
        let mut scaler = self
            .scale_cx
            .builder(font)
            .size(key.size_px as f32)
            .hint(false)
            .build();
        let sources = [Source::Outline];
        let mut render = Render::new(&sources);
        render.format(Format::Alpha);
        let ok = render.render_into(&mut scaler, key.glyph_id, img);

        let g = if ok && !img.data.is_empty() {
            let w = img.placement.width;
            let h = img.placement.height;
            match atlas.allocate(w, h) {
                Some(rect) => {
                    atlas.write_coverage(rect, &img.data);
                    RasterGlyph {
                        rect,
                        left: img.placement.left,
                        top: img.placement.top,
                        width: w,
                        height: h,
                    }
                }
                // Atlas full: a grow event (§4). Report placement with an empty
                // rect so the caller knows to grow + retry.
                None => RasterGlyph {
                    rect: AtlasRect::default(),
                    left: img.placement.left,
                    top: img.placement.top,
                    width: w,
                    height: h,
                },
            }
        } else {
            // Empty glyph (e.g. space): no coverage, zero-size rect, no atlas write.
            RasterGlyph {
                rect: AtlasRect::default(),
                left: img.placement.left,
                top: img.placement.top,
                width: 0,
                height: 0,
            }
        };

        self.glyph_cache.insert(key, g);
        g
    }

    /// Convenience: the [`GlyphKey`] for `glyph_id` at `size_px` in the embedded
    /// font, with the deterministic subpixel bucket (0). Additive helper.
    pub fn glyph_key(&self, glyph_id: u16, size_px: u32) -> GlyphKey {
        self.glyph_key_in(EMBEDDED_FONT_ID, glyph_id, size_px)
    }

    /// Like [`glyph_key`](Self::glyph_key) but in an explicit face — the rich
    /// path, where a [`RichGlyph`]/[`ShapedText::font`] carries its [`FontId`].
    pub fn glyph_key_in(&self, font: FontId, glyph_id: u16, size_px: u32) -> GlyphKey {
        GlyphKey {
            font,
            glyph_id,
            size_px,
            subpixel: 0,
        }
    }
}
