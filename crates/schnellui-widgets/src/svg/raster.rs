use super::*;

pub fn rasterize_svg(doc: &SvgDoc, pw: u32, ph: u32) -> Vec<u8> {
    raster_impl(doc, pw, ph, None)
}

/// Like [`rasterize_svg`], additionally rendering `<text>` runs through the
/// pooled [`TextShaper`] (the embedded deterministic font, SOUL §7.3).
pub fn rasterize_svg_with_text(doc: &SvgDoc, pw: u32, ph: u32, shaper: &mut TextShaper) -> Vec<u8> {
    raster_impl(doc, pw, ph, Some(shaper))
}

fn raster_impl(doc: &SvgDoc, pw: u32, ph: u32, mut shaper: Option<&mut TextShaper>) -> Vec<u8> {
    let mut buf = vec![0u8; (pw as usize) * (ph as usize) * 4];
    if doc.width <= 0.0 || doc.height <= 0.0 {
        return buf;
    }
    let sx = pw as f32 / doc.width;
    let sy = ph as f32 / doc.height;
    // scratch atlas for text glyph coverage (created lazily per raster call)
    let mut glyph_scratch: Option<GlyphAtlas> = None;

    for shape in &doc.shapes {
        match &shape.kind {
            SvgShapeKind::Path { contours } => {
                raster_path(&mut buf, pw, ph, doc, shape, contours, sx, sy);
            }
            SvgShapeKind::Text {
                x,
                y,
                size,
                anchor,
                content,
            } => {
                let Some(shaper) = shaper.as_deref_mut() else {
                    continue;
                };
                let scratch = glyph_scratch.get_or_insert_with(|| GlyphAtlas::new(512, 512));
                raster_text(
                    &mut buf, pw, ph, doc, shape, shaper, scratch, *x, *y, *size, *anchor, content,
                    sx, sy,
                );
            }
        }
    }
    buf
}

/// Rasterizes one flattened multi-contour shape: fill (nonzero/evenodd across
/// **all** contours — donut holes work) then stroke, each supersampled.
///
/// **Scanline coverage, not per-sample winding.** The sample grid is unchanged
/// (4×4 per pixel, same sample points, same tie rule as the old per-point
/// winding test — `x < crossing` counts), but each sub-row computes its edge
/// crossings **once** and sweeps them across the samples in x order, so fill
/// cost is O(edges + samples) per sub-row instead of O(edges) *per sample*.
/// Strokes prune to the segments whose bbox (± the stroke reach) intersects
/// the pixel row before paying the distance test. Same pixels, ~100× fewer
/// edge tests on icon-sized documents.
#[allow(clippy::too_many_arguments)]
fn raster_path(
    buf: &mut [u8],
    pw: u32,
    ph: u32,
    doc: &SvgDoc,
    shape: &SvgShape,
    contours: &[Contour],
    sx: f32,
    sy: f32,
) {
    // device-space contours
    let dev: Vec<Contour> = contours
        .iter()
        .map(|c| Contour {
            pts: c
                .pts
                .iter()
                .map(|(x, y)| ((x - doc.min_x) * sx, (y - doc.min_y) * sy))
                .collect(),
            closed: c.closed,
        })
        .collect();
    let half_stroke = if shape.stroke.is_some() {
        shape.stroke_width * (sx + sy) * 0.5 * 0.5
    } else {
        0.0
    };
    // device bbox (+ stroke reach + AA margin)
    let mut b = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    for c in &dev {
        for (x, y) in &c.pts {
            b.0 = b.0.min(*x);
            b.1 = b.1.min(*y);
            b.2 = b.2.max(*x);
            b.3 = b.3.max(*y);
        }
    }
    if !b.0.is_finite() {
        return;
    }
    let m = half_stroke + 1.0;
    let px0 = (b.0 - m).floor().max(0.0) as u32;
    let py0 = (b.1 - m).floor().max(0.0) as u32;
    let px1 = ((b.2 + m).ceil() as i64).clamp(0, pw as i64) as u32;
    let py1 = ((b.3 + m).ceil() as i64).clamp(0, ph as i64) as u32;
    if px0 >= px1 || py0 >= py1 {
        return;
    }
    let width_px = (px1 - px0) as usize;

    // Fill edges: every non-horizontal segment, every contour treated closed
    // (per SVG fill semantics — identical to the old winding walk).
    struct FillEdge {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        /// +1 when the edge runs downward in y (the nonzero winding direction).
        dir: i32,
    }
    let mut edges: Vec<FillEdge> = Vec::new();
    if shape.fill.is_some() {
        for c in &dev {
            let n_pts = c.pts.len();
            if n_pts < 2 {
                continue;
            }
            let mut j = n_pts - 1;
            for i in 0..n_pts {
                let (xi, yi) = c.pts[i];
                let (xj, yj) = c.pts[j];
                if yi != yj {
                    edges.push(FillEdge {
                        x0: xi,
                        y0: yi,
                        x1: xj,
                        y1: yj,
                        dir: if yj > yi { 1 } else { -1 },
                    });
                }
                j = i;
            }
        }
    }
    // Stroke segments with their bboxes (open contours skip the closing segment,
    // exactly like the old stroke_hit walk).
    struct StrokeSeg {
        a: (f32, f32),
        b: (f32, f32),
        x_min: f32,
        x_max: f32,
        y_min: f32,
        y_max: f32,
    }
    let mut segs: Vec<StrokeSeg> = Vec::new();
    if shape.stroke.is_some() && half_stroke > 0.0 {
        for c in &dev {
            if c.pts.len() < 2 {
                continue;
            }
            let count = c.pts.len() - if c.closed { 0 } else { 1 };
            for i in 0..count {
                let a = c.pts[i];
                let bp = c.pts[(i + 1) % c.pts.len()];
                segs.push(StrokeSeg {
                    a,
                    b: bp,
                    x_min: a.0.min(bp.0),
                    x_max: a.0.max(bp.0),
                    y_min: a.1.min(bp.1),
                    y_max: a.1.max(bp.1),
                });
            }
        }
    }

    let n = (SS * SS) as f32;
    // per-row scratch, reused (raster is mount-time work — allocation is fine,
    // but per-sub-row churn isn't)
    let mut fill_cov: Vec<u16> = vec![0; width_px];
    let mut stroke_cov: Vec<u16> = vec![0; width_px];
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    let mut row_segs: Vec<usize> = Vec::new();

    for py in py0..py1 {
        fill_cov.fill(0);
        stroke_cov.fill(0);

        // ---- fill: one crossing sweep per sub-row ----
        for sj in 0..SS {
            if edges.is_empty() {
                break;
            }
            let y = py as f32 + (sj as f32 + 0.5) / SS as f32;
            crossings.clear();
            let mut total_winding = 0i32;
            for e in &edges {
                if (e.y0 > y) != (e.y1 > y) {
                    let t = (y - e.y0) / (e.y1 - e.y0);
                    crossings.push((e.x0 + t * (e.x1 - e.x0), e.dir));
                    total_winding += e.dir;
                }
            }
            if crossings.is_empty() {
                continue;
            }
            crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            // Sweep the samples left→right: `passed` accumulates crossings at or
            // left of the sample, so what remains to the right reproduces the old
            // `x < crossing` count/winding exactly.
            let total = crossings.len();
            let mut ptr = 0usize;
            let mut passed_winding = 0i32;
            for (k, px) in (px0..px1).enumerate() {
                for si in 0..SS {
                    let x = px as f32 + (si as f32 + 0.5) / SS as f32;
                    while ptr < total && crossings[ptr].0 <= x {
                        passed_winding += crossings[ptr].1;
                        ptr += 1;
                    }
                    let inside = match shape.fill_rule {
                        FillRule::NonZero => total_winding - passed_winding != 0,
                        FillRule::EvenOdd => (total - ptr) % 2 == 1,
                    };
                    if inside {
                        fill_cov[k] += 1;
                    }
                }
            }
        }

        // ---- stroke: distance tests against the row's pruned segment set ----
        if !segs.is_empty() {
            row_segs.clear();
            let (row_y0, row_y1) = (py as f32, py as f32 + 1.0);
            for (i, s) in segs.iter().enumerate() {
                if s.y_min - half_stroke <= row_y1 && s.y_max + half_stroke >= row_y0 {
                    row_segs.push(i);
                }
            }
            if !row_segs.is_empty() {
                for (k, px) in (px0..px1).enumerate() {
                    let mut cov = 0u16;
                    for sj in 0..SS {
                        for si in 0..SS {
                            let x = px as f32 + (si as f32 + 0.5) / SS as f32;
                            let y = py as f32 + (sj as f32 + 0.5) / SS as f32;
                            let hit = row_segs.iter().any(|&i| {
                                let s = &segs[i];
                                // conservative-exact x prune: farther than the
                                // reach in x alone can never be within it
                                if x < s.x_min - half_stroke || x > s.x_max + half_stroke {
                                    return false;
                                }
                                dist_to_segment(x, y, s.a, s.b) <= half_stroke
                            });
                            if hit {
                                cov += 1;
                            }
                        }
                    }
                    stroke_cov[k] = cov;
                }
            }
        }

        // ---- blend the row ----
        for (k, px) in (px0..px1).enumerate() {
            let (fc, sc) = (fill_cov[k], stroke_cov[k]);
            if fc == 0 && sc == 0 {
                continue;
            }
            let idx = ((py as usize) * (pw as usize) + px as usize) * 4;
            let pc = (px as f32 + 0.5, py as f32 + 0.5);
            if let (Some(p), true) = (shape.fill, fc > 0) {
                let c = eval_paint(p, doc, shape, &b, pc, sx, sy);
                blend_px(&mut buf[idx..idx + 4], c, fc as f32 / n * shape.opacity);
            }
            if let (Some(p), true) = (shape.stroke, sc > 0) {
                let c = eval_paint(p, doc, shape, &b, pc, sx, sy);
                blend_px(&mut buf[idx..idx + 4], c, sc as f32 / n * shape.opacity);
            }
        }
    }
}

/// Rasterizes one `<text>` run: shape at the device font size, rasterize each
/// glyph's coverage through the scratch atlas, and blend it in the run's paint.
#[allow(clippy::too_many_arguments)]
fn raster_text(
    buf: &mut [u8],
    pw: u32,
    ph: u32,
    doc: &SvgDoc,
    shape: &SvgShape,
    shaper: &mut TextShaper,
    scratch: &mut GlyphAtlas,
    x: f32,
    y: f32,
    size: f32,
    anchor: TextAnchor,
    content: &str,
    sx: f32,
    sy: f32,
) {
    let Some(paint) = shape.fill else { return };
    // device anchor + device font size (isotropic factor over the two axes)
    let ax = (x - doc.min_x) * sx;
    let ay = (y - doc.min_y) * sy;
    let phys = (size * (sx + sy) * 0.5).round().max(1.0);
    let shaped = shaper.shape(content, phys, None);
    let x0 = match anchor {
        TextAnchor::Start => ax,
        TextAnchor::Middle => ax - shaped.width * 0.5,
        TextAnchor::End => ax - shaped.width,
    };
    // shape bbox for objectBoundingBox gradients on text
    let bbox = (
        x0,
        ay - shaped.baseline,
        x0 + shaped.width,
        ay - shaped.baseline + shaped.height,
    );
    let mut pen = 0.0f32;
    for g in &shaped.glyphs {
        let key = shaper.glyph_key_in(g.font, g.glyph_id, phys as u32);
        let rg = shaper.rasterize_glyph(key, scratch);
        if !rg.rect.is_empty() {
            let gx = (x0 + pen + g.x_offset + rg.left as f32).round() as i64;
            // y is the baseline (per SVG); the bitmap top sits `rg.top` above it
            let gy = (ay + g.y_offset - rg.top as f32).round() as i64;
            let stride = scratch.width() as usize;
            let cov_px = scratch.pixels();
            for row in 0..rg.height as i64 {
                let dy = gy + row;
                if dy < 0 || dy >= ph as i64 {
                    continue;
                }
                for col in 0..rg.width as i64 {
                    let dx = gx + col;
                    if dx < 0 || dx >= pw as i64 {
                        continue;
                    }
                    let cov = cov_px[(rg.rect.y as usize + row as usize) * stride
                        + rg.rect.x as usize
                        + col as usize];
                    if cov == 0 {
                        continue;
                    }
                    let idx = ((dy as usize) * (pw as usize) + dx as usize) * 4;
                    let c = eval_paint(
                        paint,
                        doc,
                        shape,
                        &bbox,
                        (dx as f32 + 0.5, dy as f32 + 0.5),
                        sx,
                        sy,
                    );
                    blend_px(
                        &mut buf[idx..idx + 4],
                        c,
                        (cov as f32 / 255.0) * shape.opacity,
                    );
                }
            }
        }
        pen += g.x_advance;
    }
}

/// Resolves a paint to a concrete color at a device-space point: solid colors
/// pass through; gradients interpolate their stops (SOUL §8.1).
fn eval_paint(
    paint: Paint,
    doc: &SvgDoc,
    shape: &SvgShape,
    bbox: &(f32, f32, f32, f32),
    p: (f32, f32),
    sx: f32,
    sy: f32,
) -> Color {
    let Paint::Gradient(gi) = paint else {
        let Paint::Solid(c) = paint else {
            unreachable!()
        };
        return c;
    };
    let Some(g) = doc.gradients.get(gi) else {
        return Color::BLACK;
    };
    // map the gradient geometry to device space
    let to_dev = |(gx, gy): (f32, f32)| -> (f32, f32) {
        if g.object_units {
            // fractions of the painted shape's device bbox
            (
                bbox.0 + gx * (bbox.2 - bbox.0),
                bbox.1 + gy * (bbox.3 - bbox.1),
            )
        } else {
            // user space: through the shape's CTM, then the doc scale
            let (ux, uy) = shape.transform.apply((gx, gy));
            ((ux - doc.min_x) * sx, (uy - doc.min_y) * sy)
        }
    };
    let t = match g.kind {
        GradientKind::Linear { x1, y1, x2, y2 } => {
            let a = to_dev((x1, y1));
            let b = to_dev((x2, y2));
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len2 = dx * dx + dy * dy;
            if len2 <= f32::EPSILON {
                0.0
            } else {
                ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2
            }
        }
        GradientKind::Radial { cx, cy, r } => {
            let c = to_dev((cx, cy));
            // radius maps through the same spaces (isotropic approximation)
            let rd = if g.object_units {
                r * ((bbox.2 - bbox.0) + (bbox.3 - bbox.1)) * 0.5
            } else {
                r * shape.transform.scale_factor() * (sx + sy) * 0.5
            };
            if rd <= f32::EPSILON {
                1.0
            } else {
                (((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt()) / rd
            }
        }
    }
    .clamp(0.0, 1.0);
    sample_stops(&g.stops, t)
}

/// Linear interpolation across gradient stops (pad spread).
fn sample_stops(stops: &[(f32, Color)], t: f32) -> Color {
    match stops {
        [] => Color::BLACK,
        [only] => only.1,
        _ => {
            if t <= stops[0].0 {
                return stops[0].1;
            }
            for w in stops.windows(2) {
                let (o0, c0) = w[0];
                let (o1, c1) = w[1];
                if t <= o1 {
                    let f = if o1 > o0 { (t - o0) / (o1 - o0) } else { 1.0 };
                    let lerp = |a: u8, b: u8| -> u8 {
                        (a as f32 + (b as f32 - a as f32) * f).round() as u8
                    };
                    return Color::rgba(
                        lerp(c0.r, c1.r),
                        lerp(c0.g, c1.g),
                        lerp(c0.b, c1.b),
                        lerp(c0.a, c1.a),
                    );
                }
            }
            stops.last().unwrap().1
        }
    }
}

/// src-over of `color` at `cov` coverage onto one straight-alpha RGBA8 pixel.
fn blend_px(dst: &mut [u8], color: Color, cov: f32) {
    let a = (color.a as f32 / 255.0) * cov;
    if a <= 0.0 {
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let out_a = a + da * (1.0 - a);
    if out_a <= 0.0 {
        return;
    }
    for i in 0..3 {
        let c = [color.r, color.g, color.b][i] as f32;
        let d = dst[i] as f32;
        dst[i] = (((c * a) + d * da * (1.0 - a)) / out_a)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// Euclidean distance from a point to a segment.
fn dist_to_segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ax, ay) = a;
    let (bx, by) = b;
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let (px, py) = (ax + t * dx, ay + t * dy);
    ((x - px).powi(2) + (y - py).powi(2)).sqrt()
}

// ---------------------------------------------------------------------------
// the async raster pool (SOUL §8.1 — build submits, the frame drains)
// ---------------------------------------------------------------------------

/// A completed async rasterization: the pixels for a reserved atlas rect,
/// mailed back to the submitting thread and landed by [`drain_svg_rasters`] /
/// [`settle_svg_rasters`].
pub(crate) struct SvgDone {
    pub(crate) widget: WidgetId,
    pub(crate) rect: TexelRect,
    pub(crate) generation: u64,
    pub(crate) pixels: Arc<[u8]>,
}
