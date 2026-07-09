//! The [`RichText`] viewer widget (SOUL §8.1): renders a [`RichDoc`] as real
//! glyph quads through the embedded faces, with a deterministic document theme.
//!
//! Like wrapping text, a rich document's line breaks (and therefore its glyphs
//! and height) depend on the available width, so paint is **deferred**: the
//! width-aware measure ([`measure_rich`]) flows the document during Taffy's
//! measure pass to report a size, and [`emit_rich_paint`] re-flows at the
//! laid-out width to emit the primitives at absolute positions. Both share one
//! flow function and a grow-only `(width → size)` cache, so a clean frame does
//! **zero** work here (SOUL §1; the layout block itself is skipped when nothing
//! is layout-dirty).

use schnellui_a11y::Role;
use schnellui_scene::{
    Color, DirtyFlags, PaintData, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind,
};
use schnellui_text::{
    FontFace, GlyphAtlas, RichShapedText, ShapeOptions, SpanSpec, TextShaper, WrapMode,
};
use smallvec::SmallVec;

use crate::{norm_scale, phys_size_px, theme, BuildCtx, Theme, View};

use super::doc::{ListMarker, RichBlock, RichDoc, RichSpan};

// ---------------------------------------------------------------------------
// the document theme
// ---------------------------------------------------------------------------

/// Default base font size (logical px) — paragraphs render at this size.
pub(crate) const RICH_BASE_SIZE: f32 = 15.0;
#[derive(Clone, Copy)]
struct RichPalette {
    ink: Color,
    muted: Color,
    link: Color,
    inline_code: Color,
    code_bg: Color,
    code_ink: Color,
    separator: Color,
}

impl From<Theme> for RichPalette {
    fn from(theme: Theme) -> Self {
        RichPalette {
            ink: theme.text,
            muted: theme.text_muted,
            link: theme.accent,
            inline_code: theme.attention,
            code_bg: theme.surface_muted,
            code_ink: theme.text,
            separator: theme.separator,
        }
    }
}

/// Heading size multiplier per level 1–6.
fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 1.9,
        2 => 1.5,
        3 => 1.25,
        4 => 1.1,
        5 => 1.0,
        _ => 0.9,
    }
}

/// Code renders slightly smaller than body text (mono runs wide).
const CODE_SIZE_RATIO: f32 = 0.92;
/// Code block inner padding, logical px.
const CODE_PAD: f32 = 10.0;
/// Quote bar width / gap to the quoted text, logical px.
const QUOTE_BAR_W: f32 = 3.0;
const QUOTE_INDENT: f32 = 12.0;
/// List indentation per nesting depth / marker column width, logical px.
const LIST_DEPTH_INDENT: f32 = 18.0;
const LIST_MARKER_COL: f32 = 22.0;

// ---------------------------------------------------------------------------
// the retained per-widget state (SOUL §3.3 registry)
// ---------------------------------------------------------------------------

/// A signal-bound document producer (`RichText::dynamic`, SOUL §3.3).
pub type DocFn = Box<dyn FnMut() -> RichDoc + 'static>;

/// One rich text view's retained state — the document plus the deferred paint
/// bookkeeping (mirrors `TextLayout` for wrapping text, SOUL §8.1).
pub(crate) struct RichState {
    pub(crate) doc: RichDoc,
    base_px: f32,
    scale: f32,
    palette: RichPalette,
    /// Fixed logical line advance for virtualized, non-wrapping code surfaces.
    code_line_height: Option<f32>,
    /// Signal-bound document producer. Its `WidgetId` is delivered by the
    /// runtime's targeted retained subscription queue.
    source_fn: Option<DocFn>,
    /// grow-only `(logical avail width → logical size)` measure cache (§4.4).
    cache: SmallVec<[(f32, Size); 4]>,
    /// the node's rect at the last paint emission (idempotence gate).
    last_emit: Option<Rect>,
    /// document changed since the last emission ⇒ force a re-flow + re-emit.
    dirty: bool,
}

// ---------------------------------------------------------------------------
// the widget builder (SOUL §3.3 typed builder chain)
// ---------------------------------------------------------------------------

enum RichSource {
    Static(RichDoc),
    Dynamic(DocFn),
}

/// A formatted-document viewer (SOUL §8.1): renders a [`RichDoc`] read-only
/// with real bold/italic/mono faces and document furniture (headings, lists,
/// quotes, code surfaces, rules). Build the document yourself — importers for
/// Markdown / OpenDocument / code highlighting are application code for now:
///
/// ```ignore
/// RichText::new(
///     RichDoc::new()
///         .heading(1, ["Title"])
///         .paragraph(["Body with ".into(), RichSpan::bold("emphasis"), ".".into()]),
/// )
/// RichText::dynamic(move || build_doc(&source.get()))   // live preview
/// ```
///
/// Carries the AccessKit `Document` role; its accessible value is the
/// document's plain text and its name the first heading (SOUL §6.1/§6.2).
pub struct RichText {
    source: RichSource,
    size_px: f32,
    code_line_height: Option<f32>,
}

impl RichText {
    /// A viewer over a built document.
    pub fn new(doc: RichDoc) -> RichText {
        RichText {
            source: RichSource::Static(doc),
            size_px: RICH_BASE_SIZE,
            code_line_height: None,
        }
    }

    /// A signal-bound document: re-flowed whenever the produced document
    /// changes — the live-preview slot (SOUL §3.3).
    pub fn dynamic(f: impl FnMut() -> RichDoc + 'static) -> RichText {
        RichText {
            source: RichSource::Dynamic(Box::new(f)),
            size_px: RICH_BASE_SIZE,
            code_line_height: None,
        }
    }

    /// Sets the base (paragraph) font size; headings/code scale from it.
    pub fn size(mut self, size_px: f32) -> RichText {
        self.size_px = size_px;
        self
    }

    /// Uses a fixed line advance and disables wrapping inside code blocks.
    ///
    /// This is intended for virtualized source viewers: their spacer geometry
    /// must remain stable as the mounted line window moves through a document.
    /// Horizontal overflow is left for the containing viewport to clip.
    pub fn fixed_code_lines(mut self, line_height: f32) -> RichText {
        self.code_line_height = line_height.is_finite().then(|| line_height.max(1.0));
        self
    }

    pub fn role(&self) -> Role {
        Role::Document
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::RichText
    }
}

impl View for RichText {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::RichText, parent);
        let (doc, source_fn) = match this.source {
            RichSource::Static(d) => (d, None),
            RichSource::Dynamic(mut f) => (ctx.runtime.track_dynamic_initial(id, &mut f), Some(f)),
        };

        // Semantics (SOUL §6.1/§6.2): a Document whose value is the plain
        // text — what a screen reader reads — and whose name is the first
        // heading (or a generic label for anonymous content).
        let a = ctx.scene.a11y_mut(id);
        a.role = Role::Document.as_u16();
        a.name = Some(doc.title().unwrap_or_else(|| "document".to_string()));
        a.value = Some(doc.plain_text());

        // Geometry: fill the parent's width; height flows from the measure
        // hook (deferred paint, like wrapping text — SOUL §8.1).
        ctx.layout.set_fill_width(id);
        let palette = theme(&ctx.runtime).into();
        ctx.runtime.with(|rt| {
            rt.borrow_mut().rich.insert(
                id,
                RichState {
                    doc,
                    base_px: this.size_px,
                    scale: ctx.scale,
                    palette,
                    code_line_height: this.code_line_height,
                    source_fn,
                    cache: SmallVec::new(),
                    last_emit: None,
                    dirty: true,
                },
            );
        });
        id
    }
}

// ---------------------------------------------------------------------------
// measure + emit (the deferred-paint pair, SOUL §8.1)
// ---------------------------------------------------------------------------

/// The width-aware measure hook for rich views — called from
/// [`measure_text`](crate::measure_text) so the umbrella needs no extra wiring.
/// Cached per width; re-flows only on a genuine width or document change.
pub(crate) fn measure_rich(
    runtime: &crate::Runtime,
    id: WidgetId,
    avail: Size,
    shaper: &mut TextShaper,
) -> Option<Size> {
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let st = rt.rich.get_mut(id)?;
        let w = avail.width;
        if let Some(&(_, sz)) = st.cache.iter().find(|(cw, _)| *cw == w) {
            return Some(sz);
        }
        let sz = flow_doc(
            shaper,
            &st.doc,
            st.base_px,
            st.scale,
            w,
            st.palette,
            st.code_line_height,
            &mut None,
        );
        if st.cache.len() >= 8 {
            st.cache.remove(0);
        }
        st.cache.push((w, sz));
        Some(sz)
    })
}

/// The post-layout paint pass for rich views (SOUL §8.1) — runs from
/// [`emit_wrapped_paint`](crate::emit_wrapped_paint) right after layout.
/// Idempotent: a node whose box and document are both unchanged is skipped
/// without touching the heap (Directive #1/#3).
pub(crate) fn emit_rich_paint(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    let ids: SmallVec<[WidgetId; 8]> = runtime.with(|rt| rt.borrow().rich.keys().collect());
    for id in ids {
        if scene.node(id).is_none() {
            continue;
        }
        let rect = match scene.layout(id) {
            Some(b) if !b.rect.is_empty() => b.rect,
            _ => continue,
        };
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let Some(st) = rt.rich.get_mut(id) else {
                return;
            };
            if !st.dirty && st.last_emit == Some(rect) {
                return; // box and document unchanged ⇒ nothing to re-emit
            }
            let pd = scene.paint_mut(id);
            pd.primitives.clear();
            let origin = Point {
                x: rect.x,
                y: rect.y,
            };
            flow_doc(
                shaper,
                &st.doc,
                st.base_px,
                st.scale,
                rect.width,
                st.palette,
                st.code_line_height,
                &mut Some((pd, atlas, origin)),
            );
            st.last_emit = Some(rect);
            st.dirty = false;
            scene.mark_dirty(id, DirtyFlags::PAINT);
        });
    }
}

/// Re-evaluates one ready dynamic rich document. The ready id came from the
/// runtime's targeted subscription group, so this never enumerates all rich
/// producers after an unrelated signal write.
pub(crate) fn poll_dynamic_source(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
) -> bool {
    if scene.node(id).is_none() {
        return false;
    }
    let taken = runtime.with(|rt| {
        rt.borrow_mut()
            .rich
            .get_mut(id)
            .and_then(|st| st.source_fn.take())
    });
    let Some(mut f) = taken else { return false };
    // Run/re-track with no retained-registry borrow held.
    let cur = runtime.track_dynamic(id, &mut f);
    let semantics = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let st = rt.rich.get_mut(id)?;
        st.source_fn = Some(f);
        if st.doc == cur {
            return None; // equality gate: unchanged document, zero flags
        }
        st.doc = cur;
        st.cache.clear();
        st.dirty = true;
        Some((st.doc.title(), st.doc.plain_text()))
    });
    if let Some((title, plain)) = semantics {
        if let Some(t) = title {
            scene.set_a11y_name(id, Some(t));
        }
        scene.set_a11y_value(id, Some(plain));
        scene.mark_dirty(id, DirtyFlags::LAYOUT);
        scene.mark_dirty(id, DirtyFlags::PAINT);
        scene.mark_dirty(id, DirtyFlags::A11Y);
    }
    true
}

// ---------------------------------------------------------------------------
// the document flow (measure and paint share this — SOUL §8.1)
// ---------------------------------------------------------------------------

/// The paint sink: primitives + atlas + the node's laid-out origin. `None` ⇒
/// measure-only (shape for sizes, emit nothing).
type PaintSink<'a> = Option<(&'a mut PaintData, &'a mut GlyphAtlas, Point)>;

/// Flows `doc` top-to-bottom at `avail_w` (logical px; non-finite ⇒
/// unconstrained), returning the logical size. With a paint sink, emits
/// primitives at absolute positions (origin + offsets) in paint order
/// (surfaces under glyphs).
fn flow_doc(
    shaper: &mut TextShaper,
    doc: &RichDoc,
    base_px: f32,
    scale: f32,
    avail_w: f32,
    palette: RichPalette,
    code_line_height: Option<f32>,
    paint: &mut PaintSink,
) -> Size {
    let finite = avail_w.is_finite() && avail_w > 0.0;
    let mut y = 0.0f32;
    let mut max_w = 0.0f32;
    let mut prev: Option<&RichBlock> = None;

    for block in &doc.blocks {
        y += block_gap(prev, block, base_px);
        let sz = match block {
            RichBlock::Paragraph(spans) => flow_inline(
                shaper,
                paint,
                spans,
                base_px,
                scale,
                avail_w,
                0.0,
                y,
                palette.ink,
                palette,
                false,
                false,
            ),
            RichBlock::Heading { level, spans } => {
                let size = base_px * heading_scale(*level);
                flow_inline(
                    shaper,
                    paint,
                    spans,
                    size,
                    scale,
                    avail_w,
                    0.0,
                    y,
                    palette.ink,
                    palette,
                    true,
                    false,
                )
            }
            RichBlock::Quote(spans) => {
                let x = QUOTE_BAR_W + QUOTE_INDENT;
                let inner = if finite { avail_w - x } else { avail_w };
                let sz = flow_inline(
                    shaper,
                    paint,
                    spans,
                    base_px,
                    scale,
                    inner,
                    x,
                    y,
                    palette.muted,
                    palette,
                    false,
                    true,
                );
                // the bar spans the quoted block (disjoint from the glyphs, so
                // emission order is irrelevant)
                if let Some((pd, _, origin)) = paint.as_mut() {
                    pd.primitives.push(Primitive::SolidRect {
                        rect: Rect::new(origin.x, origin.y + y, QUOTE_BAR_W, sz.height),
                        color: palette.separator,
                        corner_radius: 1.0,
                    });
                }
                Size {
                    width: x + sz.width,
                    height: sz.height,
                }
            }
            RichBlock::ListItem {
                depth,
                marker,
                spans,
            } => {
                let indent = f32::from(*depth) * LIST_DEPTH_INDENT;
                let marker_span = match marker {
                    ListMarker::Bullet => RichSpan::plain("•"),
                    ListMarker::Number(n) => RichSpan::plain(format!("{n}.")),
                };
                let m = flow_inline(
                    shaper,
                    paint,
                    std::slice::from_ref(&marker_span),
                    base_px,
                    scale,
                    f32::INFINITY,
                    indent,
                    y,
                    palette.ink,
                    palette,
                    false,
                    false,
                );
                let content_x = indent + LIST_MARKER_COL.max(m.width + 6.0);
                let inner = if finite { avail_w - content_x } else { avail_w };
                let sz = flow_inline(
                    shaper,
                    paint,
                    spans,
                    base_px,
                    scale,
                    inner,
                    content_x,
                    y,
                    palette.ink,
                    palette,
                    false,
                    false,
                );
                Size {
                    width: content_x + sz.width,
                    height: sz.height.max(m.height),
                }
            }
            RichBlock::CodeBlock { lines, .. } => flow_code_block(
                shaper,
                paint,
                lines,
                base_px,
                scale,
                avail_w,
                y,
                palette,
                code_line_height,
            ),
            RichBlock::Rule => {
                let h = base_px * 0.6;
                if let Some((pd, _, origin)) = paint.as_mut() {
                    let w = if finite { avail_w } else { max_w.max(64.0) };
                    let mid = origin.y + y + h * 0.5;
                    pd.primitives.push(Primitive::Line {
                        from: Point {
                            x: origin.x,
                            y: mid,
                        },
                        to: Point {
                            x: origin.x + w,
                            y: mid,
                        },
                        width: 1.0,
                        color: palette.separator,
                    });
                }
                Size {
                    width: if finite { avail_w } else { 0.0 },
                    height: h,
                }
            }
        };
        y += sz.height;
        max_w = max_w.max(sz.width);
        prev = Some(block);
    }

    Size {
        width: if finite { avail_w } else { max_w },
        height: y.max(base_px * 1.2),
    }
}

/// Vertical spacing between two consecutive blocks.
fn block_gap(prev: Option<&RichBlock>, next: &RichBlock, base_px: f32) -> f32 {
    match prev {
        None => 0.0,
        Some(RichBlock::ListItem { .. }) if matches!(next, RichBlock::ListItem { .. }) => {
            base_px * 0.28
        }
        Some(_) if matches!(next, RichBlock::Heading { .. }) => base_px * 1.0,
        Some(_) => base_px * 0.6,
    }
}

/// Shapes one inline block (a span list) at `size_px` within `inner_w`, and —
/// with a paint sink — emits its glyph quads + decorations at `(x_off, y)`
/// from the node origin. Returns the block's logical size.
#[allow(clippy::too_many_arguments)]
fn flow_inline(
    shaper: &mut TextShaper,
    paint: &mut PaintSink,
    spans: &[RichSpan],
    size_px: f32,
    scale: f32,
    inner_w: f32,
    x_off: f32,
    y: f32,
    default_ink: Color,
    palette: RichPalette,
    force_bold: bool,
    force_italic: bool,
) -> Size {
    let inv = 1.0 / norm_scale(scale);
    let sc = norm_scale(scale);
    let mut text = String::new();
    let mut specs: SmallVec<[SpanSpec; 8]> = SmallVec::new();
    for s in spans {
        let st = s.style;
        let ink = st.color.unwrap_or(if st.link {
            palette.link
        } else if st.code {
            palette.inline_code
        } else {
            default_ink
        });
        specs.push(SpanSpec {
            len: s.text.len(),
            face: FontFace::from_axes(st.bold || force_bold, st.italic || force_italic, st.code),
            color: [ink.r, ink.g, ink.b, ink.a],
            underline: st.underline || st.link,
            strikethrough: st.strike,
        });
        text.push_str(&s.text);
    }
    if text.is_empty() {
        return Size {
            width: 0.0,
            height: 0.0,
        };
    }
    let phys = phys_size_px(size_px, scale);
    let max_w = (inner_w.is_finite() && inner_w > 0.0).then(|| inner_w.max(1.0) * sc);
    let rich = shaper.shape_spans(&text, &specs, &ShapeOptions::new(phys).max_width(max_w));
    if let Some((pd, atlas, origin)) = paint.as_mut() {
        push_rich_glyphs(
            pd,
            shaper,
            atlas,
            &rich,
            phys as u32,
            scale,
            Point {
                x: origin.x + x_off,
                y: origin.y + y,
            },
        );
    }
    Size {
        width: rich.width * inv,
        height: rich.height * inv,
    }
}

/// Flows a code block: surface first, then per-line mono glyphs (emission
/// order matters — the glyphs overlap the surface).
#[allow(clippy::too_many_arguments)]
fn flow_code_block(
    shaper: &mut TextShaper,
    paint: &mut PaintSink,
    lines: &[Vec<RichSpan>],
    base_px: f32,
    scale: f32,
    avail_w: f32,
    y: f32,
    palette: RichPalette,
    fixed_line_height: Option<f32>,
) -> Size {
    let inv = 1.0 / norm_scale(scale);
    let sc = norm_scale(scale);
    let finite = avail_w.is_finite() && avail_w > 0.0;
    let size = base_px * CODE_SIZE_RATIO;
    let phys = phys_size_px(size, scale);
    let inner_w = if finite {
        avail_w - 2.0 * CODE_PAD
    } else {
        avail_w
    };
    let max_w = fixed_line_height
        .is_none()
        .then(|| finite.then(|| inner_w.max(1.0) * sc))
        .flatten();

    // Shape every line first: the surface height/width must be known before
    // the surface is pushed (it sits *under* the glyphs).
    let line_h = fixed_line_height.unwrap_or_else(|| shaper.shape("Ag", phys, None).height * inv);
    let mut shaped: SmallVec<[(RichShapedText, f32); 8]> = SmallVec::new();
    let mut cursor = 0.0f32;
    let mut widest = 0.0f32;
    for line in lines {
        if line.is_empty() {
            shaped.push((RichShapedText::default(), cursor));
            cursor += line_h;
            continue;
        }
        let mut text = String::new();
        let mut specs: SmallVec<[SpanSpec; 8]> = SmallVec::new();
        for s in line {
            let ink = s.style.color.unwrap_or(palette.code_ink);
            specs.push(SpanSpec {
                len: s.text.len(),
                face: FontFace::from_axes(s.style.bold, false, true),
                color: [ink.r, ink.g, ink.b, ink.a],
                underline: false,
                strikethrough: false,
            });
            text.push_str(&s.text);
        }
        let rich = shaper.shape_spans(
            &text,
            &specs,
            &ShapeOptions::new(phys)
                .max_width(max_w)
                .wrap(WrapMode::Anywhere),
        );
        widest = widest.max(rich.width * inv);
        let h = fixed_line_height.unwrap_or_else(|| (rich.height * inv).max(line_h));
        shaped.push((rich, cursor));
        cursor += h;
    }
    let content_h = cursor.max(line_h);
    let block_w = if finite {
        avail_w
    } else {
        widest + 2.0 * CODE_PAD
    };
    let block_h = content_h + 2.0 * CODE_PAD;

    if let Some((pd, atlas, origin)) = paint.as_mut() {
        pd.primitives.push(Primitive::SolidRect {
            rect: Rect::new(origin.x, origin.y + y, block_w, block_h),
            color: palette.code_bg,
            corner_radius: 4.0,
        });
        for (rich, line_y) in &shaped {
            push_rich_glyphs(
                pd,
                shaper,
                atlas,
                rich,
                phys as u32,
                scale,
                Point {
                    x: origin.x + CODE_PAD,
                    y: origin.y + y + CODE_PAD + line_y,
                },
            );
        }
    }
    Size {
        width: block_w,
        height: block_h,
    }
}

/// Rasterizes a rich shape's glyphs (per-glyph face + color) and appends the
/// quads + decoration hairlines to `pd`, positioned from `origin`. The rich
/// counterpart of [`rasterize_and_push`](crate::rasterize_and_push).
pub(crate) fn push_rich_glyphs(
    pd: &mut PaintData,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    rich: &RichShapedText,
    phys_size: u32,
    scale: f32,
    origin: Point,
) {
    let inv = 1.0 / norm_scale(scale);
    for g in &rich.glyphs {
        let key = shaper.glyph_key_in(g.font, g.glyph_id, phys_size);
        let rg = shaper.rasterize_glyph(key, atlas);
        if !rg.rect.is_empty() {
            pd.primitives.push(Primitive::GlyphQuad {
                rect: Rect::new(
                    origin.x + (g.x + rg.left as f32) * inv,
                    origin.y + (g.y - rg.top as f32) * inv,
                    rg.width as f32 * inv,
                    rg.height as f32 * inv,
                ),
                atlas_uv: Rect::new(
                    rg.rect.x as f32,
                    rg.rect.y as f32,
                    rg.rect.width as f32,
                    rg.rect.height as f32,
                ),
                color: Color::rgba(g.color[0], g.color[1], g.color[2], g.color[3]),
            });
        }
    }
    for d in &rich.decors {
        let yy = origin.y + d.y * inv;
        pd.primitives.push(Primitive::Line {
            from: Point {
                x: origin.x + d.x * inv,
                y: yy,
            },
            to: Point {
                x: origin.x + (d.x + d.width) * inv,
                y: yy,
            },
            width: (d.thickness * inv).max(0.75),
            color: Color::rgba(d.color[0], d.color[1], d.color[2], d.color[3]),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BuildCtx;
    use schnellui_layout::LayoutEngine;
    use schnellui_scene::LayoutBox;

    fn sample_doc() -> RichDoc {
        RichDoc::new()
            .heading(1, ["Title"])
            .paragraph([
                RichSpan::plain("Body with "),
                RichSpan::code("code"),
                RichSpan::plain(" and "),
                RichSpan::link("link"),
                RichSpan::plain("."),
            ])
            .code_block("rust", [vec![RichSpan::code("fn main() {}")]])
            .quote(["quoted"])
            .rule()
    }

    fn build_rich(
        runtime: &crate::Runtime,
        view: RichText,
    ) -> (Scene, LayoutEngine, TextShaper, GlyphAtlas, WidgetId) {
        crate::reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(1024, 1024);
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
            (Box::new(view) as Box<dyn View>).build(&mut ctx, None)
        };
        scene.set_root(id);
        (scene, layout, text, atlas, id)
    }

    fn lay_out(scene: &mut Scene, id: WidgetId, w: f32, h: f32) {
        let rect = Rect::new(0.0, 0.0, w, h);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
    }

    #[test]
    fn build_registers_document_semantics() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_rich(runtime, RichText::new(sample_doc()));
        let a = scene.a11y(id).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::Document);
        assert_eq!(a.name.as_deref(), Some("Title"));
        assert_eq!(
            a.value.as_deref(),
            Some("Title\nBody with code and link.\nfn main() {}\nquoted\n---")
        );
    }

    #[test]
    fn measure_flows_and_caches_by_width() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (_s, _l, mut text, _a, id) = build_rich(
            runtime,
            RichText::new(
                RichDoc::new()
                    .heading(1, ["T"])
                    .paragraph(["a paragraph that will wrap at narrow widths for sure"]),
            ),
        );
        let narrow = measure_rich(
            runtime,
            id,
            Size {
                width: 120.0,
                height: 0.0,
            },
            &mut text,
        )
        .unwrap();
        let wide = measure_rich(
            runtime,
            id,
            Size {
                width: 640.0,
                height: 0.0,
            },
            &mut text,
        )
        .unwrap();
        assert!(narrow.height > wide.height, "narrower wraps to more lines");
        // same width again hits the cache (identical result)
        let again = measure_rich(
            runtime,
            id,
            Size {
                width: 120.0,
                height: 0.0,
            },
            &mut text,
        )
        .unwrap();
        assert_eq!(again, narrow);
    }

    #[test]
    fn fixed_code_lines_keep_virtual_spacer_geometry_stable() {
        let runtime_handle = crate::Runtime::new();
        let runtime = &runtime_handle;
        let document = RichDoc::new().code_block(
            "rust",
            [
                vec![RichSpan::code(
                    "a very long source line that would normally wrap",
                )],
                Vec::new(),
                vec![RichSpan::code("third")],
            ],
        );
        let (_scene, _layout, mut text, _atlas, id) =
            build_rich(runtime, RichText::new(document).fixed_code_lines(17.0));
        let narrow = measure_rich(
            runtime,
            id,
            Size {
                width: 80.0,
                height: 0.0,
            },
            &mut text,
        )
        .unwrap();
        let wide = measure_rich(
            runtime,
            id,
            Size {
                width: 640.0,
                height: 0.0,
            },
            &mut text,
        )
        .unwrap();

        assert_eq!(narrow.height, 3.0 * 17.0 + 2.0 * CODE_PAD);
        assert_eq!(narrow.height, wide.height);
    }

    #[test]
    fn emit_paints_after_layout_and_is_idempotent() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_rich(runtime, RichText::new(sample_doc()));
        lay_out(&mut scene, id, 400.0, 600.0);
        emit_rich_paint(runtime, &mut scene, &mut text, &mut atlas);
        let prims = &scene.paint(id).unwrap().primitives;
        let palette = RichPalette::from(Theme::default());
        assert!(prims
            .iter()
            .any(|p| matches!(p, Primitive::GlyphQuad { .. })));
        // code surface + quote bar
        assert!(prims
            .iter()
            .any(|p| matches!(p, Primitive::SolidRect { color, .. } if *color == palette.code_bg)));
        assert!(prims.iter().any(
            |p| matches!(p, Primitive::SolidRect { color, .. } if *color == palette.separator)
        ));
        // underline (link) + rule hairlines
        let lines = prims
            .iter()
            .filter(|p| matches!(p, Primitive::Line { .. }))
            .count();
        assert!(lines >= 2, "link underline + thematic break, got {lines}");
        let count = prims.len();

        // second emit with the same box re-emits nothing (idempotence gate)
        scene.clear_dirty();
        emit_rich_paint(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(scene.paint(id).unwrap().primitives.len(), count);
        assert!(
            scene.dirty_flags(id).is_empty(),
            "clean re-emit flags nothing"
        );
    }

    #[test]
    fn code_surface_precedes_its_glyphs() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) = build_rich(
            runtime,
            RichText::new(RichDoc::new().code_block("rust", [vec![RichSpan::code("let x = 1;")]])),
        );
        lay_out(&mut scene, id, 400.0, 200.0);
        emit_rich_paint(runtime, &mut scene, &mut text, &mut atlas);
        let prims = &scene.paint(id).unwrap().primitives;
        let palette = RichPalette::from(Theme::default());
        let bg = prims
            .iter()
            .position(
                |p| matches!(p, Primitive::SolidRect { color, .. } if *color == palette.code_bg),
            )
            .unwrap();
        let first_glyph = prims
            .iter()
            .position(|p| matches!(p, Primitive::GlyphQuad { .. }))
            .unwrap();
        assert!(bg < first_glyph, "surface under the glyphs");
    }

    #[test]
    fn token_colors_reach_the_glyph_quads() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let accent = Color::rgb(0x88, 0x33, 0xaa);
        let (mut scene, _l, mut text, mut atlas, id) = build_rich(
            runtime,
            RichText::new(RichDoc::new().code_block(
                "rust",
                [vec![
                    RichSpan::token("fn", accent),
                    RichSpan::code(" main() {}"),
                ]],
            )),
        );
        lay_out(&mut scene, id, 400.0, 200.0);
        emit_rich_paint(runtime, &mut scene, &mut text, &mut atlas);
        let prims = &scene.paint(id).unwrap().primitives;
        let palette = RichPalette::from(Theme::default());
        assert!(prims
            .iter()
            .any(|p| matches!(p, Primitive::GlyphQuad { color, .. } if *color == accent)));
        assert!(prims.iter().any(
            |p| matches!(p, Primitive::GlyphQuad { color, .. } if *color == palette.code_ink)
        ));
    }

    #[test]
    fn dynamic_document_updates_on_change_only() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let sig = schnellui_signal::create_signal(String::from("first"));
        let (mut scene, _l, mut text, mut atlas, id) = build_rich(
            runtime,
            RichText::dynamic(move || RichDoc::plain(&sig.get())),
        );
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("first"));

        sig.set(String::from("second version"));
        schnellui_signal::Runtime::flush();
        crate::run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(
            scene.a11y(id).unwrap().value.as_deref(),
            Some("second version")
        );
        assert!(scene.dirty_flags(id).contains(DirtyFlags::LAYOUT));

        // unchanged document is a no-op (equality gate)
        scene.clear_dirty();
        crate::run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);
        assert!(scene.dirty_flags(id).is_empty());
    }

    #[test]
    fn unrelated_signal_does_not_invoke_a_dynamic_document() {
        use std::cell::Cell;
        use std::rc::Rc;

        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let document = schnellui_signal::create_signal(String::from("first"));
        let unrelated = schnellui_signal::create_signal(false);
        let calls = Rc::new(Cell::new(0_u32));
        let source_calls = calls.clone();
        let (mut scene, _l, mut text, mut atlas, _id) = build_rich(
            runtime,
            RichText::dynamic(move || {
                source_calls.set(source_calls.get() + 1);
                RichDoc::plain(&document.get())
            }),
        );
        assert_eq!(calls.get(), 1);

        unrelated.set(true);
        schnellui_signal::Runtime::flush();
        crate::run_dynamic_slots(runtime, &mut scene, &mut text, &mut atlas);

        assert_eq!(calls.get(), 1);
    }
}
