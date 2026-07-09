use crate::text::types::*;
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::FontData;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight,
    GenericFamily, Layout, LayoutContext, OverflowWrap, PositionedLayoutItem, RangedBuilder,
    StyleProperty, TextWrapMode,
};
use rustc_hash::FxHashMap;
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
    glyph_cache: FxHashMap<GlyphKey, RasterGlyph>,
    /// Glyph cache misses so far: each is one swash rasterization.
    rasterized: u64,
    /// Reused source-text buffer for [`TextShaper::truncate_to_width_into`].
    scratch_text: String,
    /// Reused cut-point list for [`TextShaper::truncate_to_width_into`].
    scratch_cuts: Vec<(u32, f32)>,
    /// Font data used by swash, indexed by the stable [`FontId`] stored on shaped glyphs.
    pub(crate) font_resources: Vec<FontData>,
    /// Maps Fontique blob/index identities to the rasterizer-facing id above.
    font_ids: FxHashMap<(u64, u32), FontId>,
}

thread_local! {
    /// Full shapers built on this thread (see [`TextShaper::full_builds`]).
    static FULL_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Default for TextShaper {
    fn default() -> Self {
        Self::new()
    }
}

/// The process-wide font setup every [`TextShaper`] starts from.
struct BaseFonts {
    collection: Collection,
    font_resources: Vec<FontData>,
    font_ids: FxHashMap<(u64, u32), FontId>,
}

impl BaseFonts {
    fn get() -> &'static BaseFonts {
        static BASE: OnceLock<BaseFonts> = OnceLock::new();
        BASE.get_or_init(BaseFonts::build)
    }

    fn build() -> BaseFonts {
        // UI text resolves through the deterministic embedded generics. System
        // fonts are loaded too: they serve callers that name a family and act as
        // glyph fallback behind the embedded faces.
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
        let mut font_ids = FxHashMap::default();
        let mut families = [None; EMBEDDED_FACES.len()];
        for (slot, bytes) in families.iter_mut().zip(EMBEDDED_FACES) {
            let blob = Blob::new(Arc::new(bytes) as Arc<dyn AsRef<[u8]> + Send + Sync>);
            let family = collection
                .register_fonts(blob.clone(), None)
                .first()
                .map(|(id, _)| *id)
                .expect("embedded face registered at least one family");
            // FontFace::font_id and the symbol-font ids are indices into
            // EMBEDDED_FACES, so registration order is part of their contract.
            let id = FontId(font_resources.len() as u32);
            font_ids.insert((blob.id(), 0), id);
            font_resources.push(FontData::new(blob, 0));
            *slot = Some((family, id));
        }
        let family_of = |id: FontId| families[id.0 as usize].expect("registered").0;
        let family_id = family_of(EMBEDDED_FONT_ID);
        let mono_family_id = family_of(FontFace::Mono.font_id());
        let nerd_symbols_family_id = family_of(NERD_SYMBOLS_FONT_ID);
        let unicode_symbols_family_id = family_of(UNICODE_SYMBOLS_FONT_ID);

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

        BaseFonts {
            collection,
            font_resources,
            font_ids,
        }
    }
}

impl TextShaper {
    /// Builds a shaper over a Fontique collection holding the embedded faces
    /// (deterministic, SOUL §7.3), mapped as every generic family so UI text
    /// resolves to Liberation Sans (Liberation Mono for monospace). The
    /// collection also loads the system fonts: they serve explicitly named
    /// families and glyph fallback for characters the embedded faces lack.
    ///
    /// The collection is built once per process and cloned: fontique keeps the
    /// system font index behind an `Arc`, so a clone shares the fontconfig scan
    /// instead of repeating it for every app, theme rebuild, or remount.
    pub fn new() -> TextShaper {
        FULL_BUILDS.with(|c| c.set(c.get() + 1));
        let base = BaseFonts::get();
        TextShaper {
            font_cx: FontContext {
                collection: base.collection.clone(),
                source_cache: SourceCache::default(),
            },
            layout_cx: LayoutContext::new(),
            scratch_layout: Layout::default(),
            scale_cx: ScaleContext::new(),
            scratch_image: Image::new(),
            glyph_cache: FxHashMap::default(),
            rasterized: 0,
            scratch_text: String::new(),
            scratch_cuts: Vec::new(),
            font_resources: base.font_resources.clone(),
            font_ids: base.font_ids.clone(),
        }
    }

    /// A shaper with no fonts, for a stand-in that is dropped without shaping.
    /// It skips the collection clone, so it is cheap. Shaping with it falls back
    /// to whatever parley does with an empty collection.
    #[doc(hidden)]
    pub fn detached() -> TextShaper {
        TextShaper {
            font_cx: FontContext {
                collection: Collection::new(CollectionOptions {
                    shared: false,
                    system_fonts: false,
                }),
                source_cache: SourceCache::default(),
            },
            layout_cx: LayoutContext::new(),
            scratch_layout: Layout::default(),
            scale_cx: ScaleContext::new(),
            scratch_image: Image::new(),
            glyph_cache: FxHashMap::default(),
            rasterized: 0,
            scratch_text: String::new(),
            scratch_cuts: Vec::new(),
            font_resources: Vec::new(),
            font_ids: FxHashMap::default(),
        }
    }

    /// Number of full shapers ([`TextShaper::new`]) built on this thread; lets
    /// tests prove a path builds none.
    #[doc(hidden)]
    pub fn full_builds() -> usize {
        FULL_BUILDS.with(|c| c.get())
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

    /// Shapes `text` at `size_px`, returning positioned glyphs + intrinsic size
    /// (SOUL §8.1). Runs through the pooled Parley context (warm ⇒ amortized
    /// zero-alloc). `max_width` (if given) is the wrap width in logical px.
    ///
    /// Back-compat thin wrapper: equivalent to [`TextShaper::shape_with`] with
    /// [`WrapMode::Word`] + [`TextAlign::Start`] (parley's default word wrap), so
    /// existing callers (e.g. the widgets/chart glyph path) keep their behavior.
    pub fn shape(&mut self, text: &str, size_px: f32, max_width: Option<f32>) -> ShapedText {
        let mut out = ShapedText::default();
        self.shape_core(
            text,
            &ShapeOptions::new(size_px).max_width(max_width),
            None,
            &mut out,
        );
        out
    }

    /// Shapes `text` with explicit [`ShapeOptions`] (wrap mode + alignment +
    /// width), returning a [`ShapedText`] whose `lines` carry correct multi-line,
    /// aligned origins and whose `width`/`height` are the widest line / total
    /// height (SOUL §8.1). Warm re-shape of same/same-length text stays amortized
    /// zero-alloc through the pooled `LayoutContext`/`Layout` (§4.1).
    ///
    /// The returned value owns fresh glyph storage, which spills to the heap past
    /// 16 glyphs. Hot paths use [`shape_into`](Self::shape_into) instead.
    pub fn shape_with(&mut self, text: &str, opts: &ShapeOptions) -> ShapedText {
        let mut out = ShapedText::default();
        self.shape_core(text, opts, None, &mut out);
        out
    }

    /// [`shape_with`](Self::shape_with) into a caller-owned [`ShapedText`]. `out`
    /// is cleared first and keeps its glyph/line capacity, so a caller that holds
    /// one result per call site shapes without allocating once it has grown to
    /// the longest text it sees.
    pub fn shape_into(&mut self, text: &str, opts: &ShapeOptions, out: &mut ShapedText) {
        self.shape_core(text, opts, None, out);
    }

    /// Shapes with an explicitly selected installed font family.
    pub fn shape_with_family(
        &mut self,
        text: &str,
        opts: &ShapeOptions,
        family: &str,
    ) -> ShapedText {
        let mut out = ShapedText::default();
        self.shape_core(text, opts, Some(family), &mut out);
        out
    }

    /// Builds, breaks and aligns the pooled layout for `text`. `style` adds the
    /// font family and per-span properties; the size, wrap mode and alignment
    /// from `opts` are applied here, once, for every shaping entry point.
    fn layout_text(
        &mut self,
        text: &str,
        opts: &ShapeOptions,
        style: impl FnOnce(&mut RangedBuilder<'_, Brush>),
    ) {
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
            style(&mut builder);
            builder.build_into(layout, text);
        }

        layout.break_all_lines(opts.max_width);

        // `Start` leaves offsets at 0 (parley's post-break default), so skip the
        // extra pass — this also keeps `shape()` byte-identical to the pre-wrap
        // behavior. Other alignments shift each line's `metrics.offset`.
        let alignment = match opts.align {
            TextAlign::Start => return,
            TextAlign::Center => Alignment::Center,
            TextAlign::End => Alignment::End,
            TextAlign::Justify => Alignment::Justify,
        };
        layout.align(alignment, AlignmentOptions::default());
    }

    /// The shared shaping core (SOUL §8.1). All wrap/align config flows through
    /// here; [`shape`](Self::shape)/[`shape_with`](Self::shape_with)/
    /// [`truncate_to_width`](Self::truncate_to_width) are thin callers.
    fn shape_core(
        &mut self,
        text: &str,
        opts: &ShapeOptions,
        font_family: Option<&str>,
        out: &mut ShapedText,
    ) {
        let face = opts.face;
        self.layout_text(text, opts, |builder| {
            // Borrowed names: no per-call Vec/String for a named family.
            let named;
            let family = if let Some(family) = font_family {
                named = [
                    FontFamilyName::Named(Cow::Borrowed(family)),
                    FontFamilyName::Generic(GenericFamily::Monospace),
                ];
                FontFamily::List(Cow::Borrowed(&named))
            } else if face.mono() {
                FontFamily::from(GenericFamily::Monospace)
            } else {
                FontFamily::from(GenericFamily::SansSerif)
            };
            builder.push_default(StyleProperty::FontFamily(family));
            if face.bold() {
                builder.push_default(StyleProperty::FontWeight(FontWeight::BOLD));
            }
            if face.italic() {
                builder.push_default(StyleProperty::FontStyle(FontStyle::Italic));
            }
        });
        let layout = &self.scratch_layout;

        out.clear();
        out.width = layout.width();
        out.height = layout.height();
        out.font = opts.face.font_id();
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
        self.layout_text(text, opts, |builder| {
            builder.push_default(StyleProperty::FontFamily(GenericFamily::SansSerif.into()));
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
        });
        let layout = &self.scratch_layout;

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
    /// longest prefix whose `prefix + "…"` fits `max_width` is shaped and returned
    /// (ending in the ellipsis glyph). Parley has no native ellipsis, so the cut is
    /// found by walking the full shape's cluster advances.
    pub fn truncate_to_width(&mut self, text: &str, size_px: f32, max_width: f32) -> ShapedText {
        let mut out = ShapedText::default();
        self.truncate_to_width_into(text, size_px, max_width, &mut out);
        out
    }

    /// [`truncate_to_width`](Self::truncate_to_width) into a caller-owned result.
    ///
    /// **Cost:** three shapes at most for any text length: the full text, the
    /// ellipsis alone, and the chosen `prefix + "…"`. The cut comes from summing
    /// the full shape's glyph advances at cluster boundaries instead of
    /// re-shaping prefixes. Kerning across the cut can make that sum a hair off,
    /// so the final shape is verified and the cut backs off one cluster at a time
    /// until it fits. Warm calls reuse the shaper's scratch buffers.
    pub fn truncate_to_width_into(
        &mut self,
        text: &str,
        size_px: f32,
        max_width: f32,
        out: &mut ShapedText,
    ) {
        const ELLIPSIS: &str = "…";
        // Force a single line; alignment is irrelevant to truncation.
        let opts = ShapeOptions::new(size_px).wrap(WrapMode::NoWrap);

        self.shape_core(text, &opts, None, out);
        if text.is_empty() || out.width <= max_width {
            return;
        }

        // Cluster start offsets in logical order with the width of the text
        // before each. Glyphs come out in visual order, which runs backwards
        // through RTL runs, so sum advances per cluster byte offset and sort.
        let mut cuts = std::mem::take(&mut self.scratch_cuts);
        cuts.clear();
        cuts.extend(out.glyphs.iter().map(|g| (g.cluster, g.x_advance)));
        cuts.sort_unstable_by_key(|c| c.0);
        let (mut pen, mut w, mut r) = (0.0f32, 0usize, 0usize);
        while r < cuts.len() {
            let c = cuts[r].0;
            let mut adv = 0.0f32;
            while r < cuts.len() && cuts[r].0 == c {
                adv += cuts[r].1;
                r += 1;
            }
            cuts[w] = (c, pen);
            w += 1;
            pen += adv;
        }
        cuts.truncate(w);
        // A cut at byte 0 would be the ellipsis alone, handled below.
        cuts.retain(|c| c.0 != 0);

        // The ellipsis alone: its width is reserved, and it is the answer when
        // not even one character fits beside it.
        self.shape_core(ELLIPSIS, &opts, None, out);
        let budget = max_width - out.width;
        let mut fit = cuts.partition_point(|&(_, pen)| pen <= budget);

        // Verify with a real shape of `prefix + "…"`, backing off on a miss.
        let mut buf = std::mem::take(&mut self.scratch_text);
        let mut result_fits = false;
        while fit > 0 {
            let cut = cuts[fit - 1].0 as usize;
            buf.clear();
            buf.push_str(&text[..cut]);
            buf.push_str(ELLIPSIS);
            self.shape_core(&buf, &opts, None, out);
            if out.width <= max_width {
                result_fits = true;
                break;
            }
            fit -= 1;
        }
        if !result_fits {
            self.shape_core(ELLIPSIS, &opts, None, out);
        }
        self.scratch_text = buf;
        self.scratch_cuts = cuts;
    }

    /// Measures `text` at `size_px` under an optional width constraint, returning
    /// only the intrinsic `(width, height)` handed up to layout (SOUL §8.1).
    pub fn measure(&mut self, text: &str, size_px: f32, max_width: Option<f32>) -> (f32, f32) {
        let s = self.shape(text, size_px, max_width);
        (s.width, s.height)
    }

    /// Rasterizes one glyph via swash into `atlas`, marking the written sub-rect
    /// dirty (SOUL §8.1, §3.2), and returns the full [`RasterGlyph`] (atlas rect +
    /// placement bearing) the renderer needs to position the quad. A repeated key
    /// is a cache hit (idempotent, no re-write, §4.1).
    ///
    /// A full atlas grows ([`GlyphAtlas::grow`]) and the glyph is placed again. If
    /// the atlas cannot grow, the glyph comes back with an empty rect and is *not*
    /// cached, so it draws as soon as an atlas with room is used.
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
            // Growing keeps placed texels in place, so retrying is sound.
            let mut slot = atlas.allocate(w, h);
            while slot.is_none() && atlas.can_ever_fit(w, h) && atlas.grow() {
                slot = atlas.allocate(w, h);
            }
            let Some(rect) = slot else {
                // Atlas at its cap: report the placement but cache nothing, so
                // the glyph is not lost for good.
                return RasterGlyph {
                    rect: AtlasRect::default(),
                    left: img.placement.left,
                    top: img.placement.top,
                    width: w,
                    height: h,
                };
            };
            atlas.write_coverage(rect, &img.data);
            RasterGlyph {
                rect,
                left: img.placement.left,
                top: img.placement.top,
                width: w,
                height: h,
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

        self.rasterized += 1;
        self.glyph_cache.insert(key, g);
        g
    }

    /// A new shaper sharing the font setup with a copy of this one's glyph
    /// cache, valid against a clone of the atlas this shaper writes into.
    pub fn fork(&self) -> TextShaper {
        let mut forked = TextShaper::new();
        forked.glyph_cache = self.glyph_cache.clone();
        // Cached GlyphKeys name faces by FontId, a per-shaper index handed out in
        // first-seen order. The fork must resolve ids exactly as the donor did, or
        // a face it meets later reuses an id the donor gave another face and hits
        // that face's cached bitmap.
        forked.font_ids = self.font_ids.clone();
        // Share the loaded font blobs too, so a face the donor already loaded keeps
        // its blob identity (and thus its FontId and cached glyphs) in the fork.
        forked.font_cx = FontContext {
            collection: self.font_cx.collection.clone(),
            source_cache: self.font_cx.source_cache.clone(),
        };
        forked.font_resources = self.font_resources.clone();
        forked.rasterized = self.rasterized;
        forked
    }

    /// How many glyphs this shaper has rasterized (glyph cache misses). A
    /// shaper carried across a rebuild keeps its count, so a delta of zero
    /// proves the cache was reused.
    pub fn rasterized_glyph_count(&self) -> u64 {
        self.rasterized
    }

    /// The [`GlyphKey`] for `glyph_id` at `size_px` in `font`, with the
    /// deterministic subpixel bucket (0). The face comes from a
    /// [`ShapedGlyph::font`] / [`RichGlyph::font`].
    pub fn glyph_key_in(&self, font: FontId, glyph_id: u16, size_px: u32) -> GlyphKey {
        GlyphKey {
            font,
            glyph_id,
            size_px,
            subpixel: 0,
        }
    }
}
