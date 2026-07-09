use super::*;

pub fn node_rect(scene: &Scene, id: WidgetId, intrinsic: Size) -> Rect {
    match scene.layout(id) {
        Some(b) if !b.rect.is_empty() => b.rect,
        _ => Rect::new(0.0, 0.0, intrinsic.width, intrinsic.height),
    }
}

// ---------------------------------------------------------------------------
// paint-fragment emission (SOUL §3.2, §8.1 — cleared-and-refilled, §4.4)
// ---------------------------------------------------------------------------

/// Rasterizes each inked glyph of `shaped` into `atlas` and appends one
/// [`Primitive::GlyphQuad`] per glyph to `pd`, positioned relative to `origin`
/// (SOUL §3.2, §8.1). `phys_size` is the physical rasterization size (the atlas
/// key). Shaping happened at physical pixels, so all glyph metrics are physical and
/// are divided back to **logical** coordinates (`/ scale`) here — the renderer
/// re-applies `scale` at draw, so the physical atlas bitmap lands 1:1 on screen and
/// stays crisp under `--scale` (SOUL §7.1).
///
/// A repeated glyph is an atlas cache hit (no re-write, no heap — SOUL §4.1); empty
/// (space / zero-coverage) glyphs advance the pen but emit no quad.
///
/// Public so external widget crates (e.g. `schnellui-charts`, whose axis/legend labels
/// are real shaped text) reuse the exact glyph-emission path — same atlas, same
/// physical-to-logical scaling — rather than re-implementing it.
#[allow(clippy::too_many_arguments)]
pub fn rasterize_and_push(
    pd: &mut PaintData,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    shaped: &ShapedText,
    phys_size: u32,
    color: Color,
    scale: f32,
    origin: Point,
) {
    let inv = 1.0 / norm_scale(scale);
    let mut pen = 0.0f32; // physical pen advance along the baseline
    for g in &shaped.glyphs {
        // Each glyph carries Fontique's resolved face so fallback symbols are
        // rasterized from the same font that supplied their glyph ids.
        let key = shaper.glyph_key_in(g.font, g.glyph_id, phys_size);
        let rg = shaper.rasterize_glyph(key, atlas);
        if !rg.rect.is_empty() {
            // Physical placement of the coverage bitmap (swash/zeno: `top` is above
            // the baseline, `left` is the bearing from the pen origin).
            let gx = pen + g.x_offset + rg.left as f32;
            let gy = shaped.baseline + g.y_offset - rg.top as f32;
            pd.primitives.push(Primitive::GlyphQuad {
                rect: Rect::new(
                    origin.x + gx * inv,
                    origin.y + gy * inv,
                    rg.width as f32 * inv,
                    rg.height as f32 * inv,
                ),
                atlas_uv: Rect::new(
                    rg.rect.x as f32,
                    rg.rect.y as f32,
                    rg.rect.width as f32,
                    rg.rect.height as f32,
                ),
                color,
            });
        }
        pen += g.x_advance;
    }
}

/// Like [`rasterize_and_push`] but walks `shaped.lines`, so a **multi-line** (wrapped)
/// or **aligned** run paints each line from its own inline origin `line.x` (which
/// carries the [`TextAlign`] shift) at its own `line.baseline` (SOUL §8.1). `origin`
/// is the node's laid-out top-left; each glyph therefore lands at its **absolute**
/// aligned position — no post-hoc slide is needed (and [`reposition_node`] leaves
/// these nodes alone, so alignment offsets survive). Single-line runs (`lines.len() ==
/// 1`, `line.x == 0`) reduce to the same output as [`rasterize_and_push`].
///
/// Glyph advances in `shaped.glyphs` are line-local; a line indexes its slice via
/// `glyph_start`/`glyph_count`. Physical metrics are divided back to logical (`/scale`)
/// exactly as [`rasterize_and_push`] does.
#[allow(clippy::too_many_arguments)]
pub fn rasterize_lines_and_push(
    pd: &mut PaintData,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    shaped: &ShapedText,
    phys_size: u32,
    color: Color,
    scale: f32,
    origin: Point,
) {
    let inv = 1.0 / norm_scale(scale);
    for line in &shaped.lines {
        let g0 = line.glyph_start as usize;
        let g1 = g0 + line.glyph_count as usize;
        let mut pen = line.x; // physical inline origin, incl. alignment offset
        for g in &shaped.glyphs[g0..g1.min(shaped.glyphs.len())] {
            let key = shaper.glyph_key_in(g.font, g.glyph_id, phys_size);
            let rg = shaper.rasterize_glyph(key, atlas);
            if !rg.rect.is_empty() {
                let gx = pen + g.x_offset + rg.left as f32;
                let gy = line.baseline + g.y_offset - rg.top as f32;
                pd.primitives.push(Primitive::GlyphQuad {
                    rect: Rect::new(
                        origin.x + gx * inv,
                        origin.y + gy * inv,
                        rg.width as f32 * inv,
                        rg.height as f32 * inv,
                    ),
                    atlas_uv: Rect::new(
                        rg.rect.x as f32,
                        rg.rect.y as f32,
                        rg.rect.width as f32,
                        rg.rect.height as f32,
                    ),
                    color,
                });
            }
            pen += g.x_advance;
        }
    }
}

/// Shapes `text` at `size_px` through the pooled shaper and emits real per-glyph
/// [`Primitive::GlyphQuad`]s into `id`'s paint column (SOUL §8.1, §3.2), replacing
/// any previous fragments (`clear()`-then-refill retains capacity, §4.4). Glyphs are
/// positioned relative to a local `(0,0)` text-box origin; [`reposition_paint`]
/// later slides the whole set onto the node's laid-out origin. Returns the run's
/// **logical** intrinsic size, handed up to layout as the node's content size.
///
/// Public so external widget crates (e.g. `schnellui-charts`) emit their text labels
/// through the identical shape-and-rasterize path as the built-in leaves, keeping one
/// glyph pipeline and one intrinsic-size convention across the ecosystem.
#[allow(clippy::too_many_arguments)]
pub fn emit_text_paint(
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    text: &str,
    size_px: f32,
    color: Color,
    scale: f32,
) -> Size {
    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(size_px, scale);
    let shaped = shaper.shape(text, phys, None);
    let logical = Size {
        width: shaped.width * inv,
        height: shaped.height * inv,
    };
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    rasterize_and_push(
        pd,
        shaper,
        atlas,
        &shaped,
        phys as u32,
        color,
        scale,
        Point { x: 0.0, y: 0.0 },
    );
    logical
}

/// A button's intrinsic size for a given label size under the ambient shape
/// tokens (SOUL §8.1): padding scaled by density, plus the ink frame on every
/// side, plus the block-shadow offset (the shadow paints *inside* the layout
/// box so it never bleeds over siblings). Shared by paint and the measure
/// closure so they can never disagree.
fn button_intrinsic(
    runtime: &Runtime,
    id: WidgetId,
    ts: Size,
    appearance: ButtonAppearance,
) -> Size {
    let sh = theme_for(runtime, id).shape;
    let chrome = if appearance == ButtonAppearance::Solid {
        2.0 * sh.frame + sh.shadow
    } else {
        0.0
    };
    Size {
        width: ts.width + 2.0 * sh.pad(PAD_H) + chrome,
        height: ts.height + 2.0 * sh.pad(PAD_V) + chrome,
    }
}

/// Applies an optional fixed outer width to a button while preserving the label's
/// minimum intrinsic width. Paint and layout share this calculation so a keypad
/// surface stays exactly aligned with its hit target.
pub fn sized_button_intrinsic(
    runtime: &Runtime,
    id: WidgetId,
    ts: Size,
    fixed_width: Option<f32>,
    fixed_height: Option<f32>,
    appearance: ButtonAppearance,
) -> Size {
    let mut size = button_intrinsic(runtime, id, ts, appearance);
    if let Some(width) = fixed_width.filter(|width| width.is_finite()) {
        size.width = size.width.max(width.max(0.0));
    }
    if let Some(height) = fixed_height.filter(|height| height.is_finite()) {
        size.height = size.height.max(height.max(0.0));
    }
    size
}

/// Emits a button's paint (SOUL §8.1): optionally a hard block shadow and an ink
/// frame (the physical [`Shape`] tokens), the background surface, and the label
/// as real glyph quads inset by the (density-scaled) button padding. Returns the
/// label's **logical** text size (for the intrinsic-measure closure). With the
/// classic shape (frame/shadow zero) the primitive list is exactly the legacy
/// fill-then-glyphs — the covenant with old shots (SOUL §7.3).
#[allow(clippy::too_many_arguments)]
pub fn emit_button_paint(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    label: &str,
    show_label: bool,
    disabled: bool,
    fixed_width: Option<f32>,
    fixed_height: Option<f32>,
    appearance: ButtonAppearance,
    text_size: Option<f32>,
    scale: f32,
) -> Size {
    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(text_size.unwrap_or(BUTTON_TEXT_SIZE), scale);
    let shaped = shaper.shape(if show_label { label } else { "" }, phys, None);
    let ts = Size {
        width: shaped.width * inv,
        height: shaped.height * inv,
    };
    let rect = node_rect(
        scene,
        id,
        sized_button_intrinsic(runtime, id, ts, fixed_width, fixed_height, appearance),
    );
    let t = theme_for(runtime, id);
    let sh = t.shape;
    let solid = appearance == ButtonAppearance::Solid;
    let frame = if solid { sh.frame } else { 0.0 };
    let shadow = if solid { sh.shadow } else { 0.0 };
    let bg = match (disabled, appearance) {
        (true, _) => t.disabled,
        (false, ButtonAppearance::Solid) => t.accent,
        (false, ButtonAppearance::Ghost) => Color::TRANSPARENT,
    };
    // The body is the layout box minus the shadow's offset strip.
    let body = Rect::new(
        rect.x,
        rect.y,
        (rect.width - shadow).max(0.0),
        (rect.height - shadow).max(0.0),
    );
    let radius = sh.radius(4.0, body.height);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    if shadow > 0.0 {
        pd.primitives.push(Primitive::SolidRect {
            rect: Rect::new(body.x + shadow, body.y + shadow, body.width, body.height),
            color: t.text,
            corner_radius: radius,
        });
    }
    if frame > 0.0 {
        pd.primitives.push(Primitive::SolidRect {
            rect: body,
            color: t.outline,
            corner_radius: radius,
        });
    }
    // Background stays a SolidRect (only the fake "text as block" placeholder goes).
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(
            body.x + frame,
            body.y + frame,
            (body.width - 2.0 * frame).max(0.0),
            (body.height - 2.0 * frame).max(0.0),
        ),
        color: bg,
        corner_radius: (radius - frame).max(0.0),
    });
    // Label glyphs inset by the padding from the button's top-left; they ride along
    // with the background when reposition_paint slides the node (a11y §8.1).
    rasterize_and_push(
        pd,
        shaper,
        atlas,
        &shaped,
        phys as u32,
        if appearance == ButtonAppearance::Ghost {
            t.text
        } else {
            t.on_accent
        },
        scale,
        Point {
            x: body.x + (body.width - ts.width) * 0.5,
            y: body.y + frame + sh.pad(PAD_V),
        },
    );
    ts
}

pub const TOOLTIP_TEXT_SIZE: f32 = 12.0;
