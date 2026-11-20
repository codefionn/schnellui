//! The per-instance kernels. Each one is a CPU transcription of its fragment
//! shader in [`crate::render_wgpu`]'s WGSL: the same pixel-centre coverage, the
//! same signed-distance anti-aliasing, the same texel pick and the same
//! `SrcAlpha, OneMinusSrcAlpha` blend into an 8-bit sRGB picture.
//!
//! Pixels are visited in rows of eight-lane chunks. Ranges that the shader would
//! fill with full coverage (the inside of a rect) skip the distance field: an
//! opaque one is a `fill` of the packed colour, a translucent one a byte table
//! lookup per channel.

// `!(a > 0.0)` and `!(lo <= hi)` reject NaN along with the false case, as the
// shader's comparisons do.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use super::color::{pack, ColorLuts};
use super::simd::{Lanes, Portable, SimdLevel, LANES};
use crate::render_wgpu::instances::Instances;
use crate::render_wgpu::{GlyphInstance, QuadInstance};

/// `CLIP_SLACK` of the WGSL clip test.
const CLIP_SLACK: f32 = 0.001;
/// The texel nudge of `fs_glyph`.
const TEXEL_NUDGE: f32 = 0.003_906_25;
/// Interior areas at least this large blend through a per-instance byte table.
const TABLE_MIN_PIXELS: i64 = 2048;

/// A strided RGBA8 or R8 texel plane.
#[derive(Clone, Copy)]
pub(crate) struct Texels<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl Texels<'_> {
    /// Byte offset of texel `(x, y)` with `bpp` bytes per texel, or `None` when it is
    /// outside the plane (a robust `textureLoad` reads nothing there).
    #[inline(always)]
    fn offset(&self, x: i32, y: i32, bpp: usize) -> Option<usize> {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * bpp;
        (at + bpp <= self.data.len()).then_some(at)
    }
}

/// The texture planes a frame samples: glyph coverage, colour glyphs and images.
#[derive(Clone, Copy)]
pub(crate) struct Sources<'a> {
    pub(crate) glyphs: Texels<'a>,
    pub(crate) colors: Texels<'a>,
    pub(crate) images: Texels<'a>,
}

/// One frame's rasterization state.
pub(crate) struct Raster<'a> {
    pub(crate) pixels: &'a mut [u32],
    pub(crate) width: usize,
    pub(crate) scale: f32,
    pub(crate) src: Sources<'a>,
    /// Per-column texel indices of the glyph or image being drawn.
    pub(crate) cols: &'a mut Vec<i32>,
    pub(crate) luts: &'static ColorLuts,
}

/// A half-open pixel box.
#[derive(Clone, Copy, Debug)]
struct Region {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

/// Clears `[x, y, w, h]` to `clear` and draws every instance into it in the GPU's
/// draw order: per layer, quads, images, glyphs, then scroll chrome.
pub(crate) fn draw(
    level: SimdLevel,
    raster: &mut Raster<'_>,
    inst: &Instances,
    region: [u32; 4],
    clear: u32,
) {
    match level {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: the renderer only keeps `Avx` after detecting AVX
        // (`SimdLevel::supported`).
        SimdLevel::Avx => unsafe { draw_avx(raster, inst, region, clear) },
        _ => draw_lanes::<Portable>(raster, inst, region, clear),
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
unsafe fn draw_avx(raster: &mut Raster<'_>, inst: &Instances, region: [u32; 4], clear: u32) {
    draw_lanes::<super::simd::Avx>(raster, inst, region, clear)
}

#[inline(always)]
fn draw_lanes<V: Lanes>(r: &mut Raster<'_>, inst: &Instances, [x, y, w, h]: [u32; 4], clear: u32) {
    let region = Region {
        x0: x as i32,
        y0: y as i32,
        x1: (x + w) as i32,
        y1: (y + h) as i32,
    };
    for row in region.y0..region.y1 {
        let start = row as usize * r.width;
        r.pixels[start + region.x0 as usize..start + region.x1 as usize].fill(clear);
    }
    let range = |lo: u32, hi: u32, len: usize| (lo as usize).min(len)..(hi as usize).min(len);
    let (quads, glyphs, images) = (&inst.quad_scratch, &inst.glyph_scratch, &inst.image_scratch);
    for layer in inst.layers() {
        for q in &quads[range(layer.quad_start, layer.quad_end, quads.len())] {
            quad::<V>(r, q, region);
        }
        for g in &images[range(layer.image_start, layer.image_end, images.len())] {
            image::<V>(r, g, region);
        }
        for g in &glyphs[range(layer.glyph_start, layer.glyph_end, glyphs.len())] {
            glyph::<V>(r, g, region);
        }
        for q in &quads[range(layer.chrome_start, layer.chrome_end, quads.len())] {
            quad::<V>(r, q, region);
        }
    }
}

/// `v` as a pixel index, saturated far outside any target.
#[inline(always)]
fn index(v: f32) -> i32 {
    if v.is_nan() {
        0
    } else {
        v.clamp(-1.0e9, 1.0e9) as i32
    }
}

/// The pixels `[i0, i1)` of `[lo, hi)` whose centre `i + 0.5` satisfies `inside`,
/// which must hold on one interval of centres. `(from, to)` estimates that
/// interval; the estimate only needs to be right to within a pixel, the bounds
/// are then settled with `inside` itself, so they match the per-pixel test exactly.
#[inline(always)]
fn span(lo: i32, hi: i32, (from, to): (f32, f32), inside: impl Fn(f32) -> bool) -> (i32, i32) {
    let mut i0 = (index((from - 0.5).ceil()) - 1).max(lo);
    let mut i1 = (index((to - 0.5).floor()) + 2).min(hi);
    while i0 < i1 && !inside(i0 as f32 + 0.5) {
        i0 += 1;
    }
    while i1 > i0 && !inside((i1 - 1) as f32 + 0.5) {
        i1 -= 1;
    }
    (i0, i1)
}

/// The columns and rows of `region` the instance clip `[x, y, w, h]` keeps: the
/// WGSL `clip_discard` on the pixel centre over the scale.
#[inline(always)]
fn clip_spans(region: Region, clip: [f32; 4], s: f32) -> ((i32, i32), (i32, i32)) {
    let axis = |lo: i32, hi: i32, start: f32, len: f32| {
        let c0 = start - CLIP_SLACK;
        let c1 = start + len + CLIP_SLACK;
        span(lo, hi, (c0 * s, c1 * s), |p| {
            let l = p / s;
            l >= c0 && l <= c1
        })
    };
    (
        axis(region.x0, region.x1, clip[0], clip[2]),
        axis(region.y0, region.y1, clip[1], clip[3]),
    )
}

/// A vertex coordinate on the rasterizer's sub-pixel grid (8 bits, as GPUs
/// commonly snap). Decides which pixel centres on a quad's edge it covers.
#[inline(always)]
fn snap(v: f32) -> f32 {
    (v * 256.0).round() / 256.0
}

/// WGSL `smoothstep(-0.5, 0.5, x)`.
#[inline(always)]
fn smooth<V: Lanes>(x: V) -> V {
    let t = (x + V::splat(0.5)).clamp(0.0, 1.0);
    t * t * (V::splat(3.0) - V::splat(2.0) * t)
}

/// Lanes `[0, n)` as a bit mask.
#[inline(always)]
fn live(n: usize) -> u32 {
    (1u32 << n) - 1
}

/// A constant colour: its linear channels and the packed word an opaque draw
/// stores.
#[derive(Clone, Copy)]
struct Paint {
    lin: [f32; 3],
    solid: u32,
}

impl Paint {
    fn new(luts: &ColorLuts, color: [f32; 4]) -> Paint {
        let byte = |c: f32| (c * 255.0).round().clamp(0.0, 255.0) as u8;
        let [r, g, b] = [byte(color[0]), byte(color[1]), byte(color[2])];
        Paint {
            lin: [
                luts.decode[r as usize],
                luts.decode[g as usize],
                luts.decode[b as usize],
            ],
            solid: pack(r, g, b, 255),
        }
    }
}

/// Blends `paint` at per-lane alpha `a` over `px[..n]`. Lanes with `a <= 0` are
/// left alone (the GPU blend leaves them unchanged too); lanes with `a >= 1` store
/// the packed colour, which is exactly what the blend arithmetic gives there.
#[inline(always)]
fn blend_paint<V: Lanes>(luts: &ColorLuts, px: &mut [u32], n: usize, a: V, paint: &Paint) {
    let mask = live(n);
    let drawn = V::splat(0.0).lt(a).bits() & mask;
    if drawn == 0 {
        return;
    }
    if V::splat(1.0).le(a).bits() & mask == mask {
        px[..n].fill(paint.solid);
        return;
    }
    let (mut dr, mut dg, mut db, mut da) = ([0.0; LANES], [0.0; LANES], [0.0; LANES], [0.0; LANES]);
    for k in 0..n {
        let [r, g, b, alpha] = px[k].to_le_bytes();
        dr[k] = luts.decode[r as usize];
        dg[k] = luts.decode[g as usize];
        db[k] = luts.decode[b as usize];
        da[k] = luts.unorm[alpha as usize];
    }
    let inv = V::splat(1.0) - a;
    let mut out = [[0.0; LANES]; 4];
    (V::splat(paint.lin[0]) * a + V::load(&dr) * inv).store(&mut out[0]);
    (V::splat(paint.lin[1]) * a + V::load(&dg) * inv).store(&mut out[1]);
    (V::splat(paint.lin[2]) * a + V::load(&db) * inv).store(&mut out[2]);
    (a + V::load(&da) * inv).store(&mut out[3]);
    let mut lanes = [0.0; LANES];
    a.store(&mut lanes);
    for k in 0..n {
        if drawn & (1 << k) == 0 {
            continue;
        }
        px[k] = if lanes[k] >= 1.0 {
            paint.solid
        } else {
            pack(
                luts.encode(out[0][k]),
                luts.encode(out[1][k]),
                luts.encode(out[2][k]),
                luts.encode_unorm(out[3][k]),
            )
        };
    }
}

/// Blends per-lane linear colour `(r, g, b)` at alpha `a` over `px[..n]`.
#[inline(always)]
fn blend_lanes<V: Lanes>(luts: &ColorLuts, px: &mut [u32], n: usize, rgb: [V; 3], a: V) {
    let drawn = V::splat(0.0).lt(a).bits() & live(n);
    if drawn == 0 {
        return;
    }
    let (mut dr, mut dg, mut db, mut da) = ([0.0; LANES], [0.0; LANES], [0.0; LANES], [0.0; LANES]);
    for k in 0..n {
        let [r, g, b, alpha] = px[k].to_le_bytes();
        dr[k] = luts.decode[r as usize];
        dg[k] = luts.decode[g as usize];
        db[k] = luts.decode[b as usize];
        da[k] = luts.unorm[alpha as usize];
    }
    let inv = V::splat(1.0) - a;
    let mut out = [[0.0; LANES]; 4];
    (rgb[0] * a + V::load(&dr) * inv).store(&mut out[0]);
    (rgb[1] * a + V::load(&dg) * inv).store(&mut out[1]);
    (rgb[2] * a + V::load(&db) * inv).store(&mut out[2]);
    (a + V::load(&da) * inv).store(&mut out[3]);
    for k in 0..n {
        if drawn & (1 << k) != 0 {
            px[k] = pack(
                luts.encode(out[0][k]),
                luts.encode(out[1][k]),
                luts.encode(out[2][k]),
                luts.encode_unorm(out[3][k]),
            );
        }
    }
}

/// The result of blending `paint` at alpha `a` over every possible destination
/// byte, per channel: the same arithmetic as [`blend_paint`], tabulated.
struct BlendTable([[u8; 256]; 4]);

impl BlendTable {
    fn new(luts: &ColorLuts, paint: &Paint, a: f32) -> BlendTable {
        let inv = 1.0 - a;
        let mut t = [[0u8; 256]; 4];
        for (c, row) in t.iter_mut().enumerate().take(3) {
            for (out, &d) in row.iter_mut().zip(luts.decode.iter()) {
                *out = luts.encode(paint.lin[c] * a + d * inv);
            }
        }
        for (out, &u) in t[3].iter_mut().zip(luts.unorm.iter()) {
            *out = luts.encode_unorm(a + u * inv);
        }
        BlendTable(t)
    }

    #[inline(always)]
    fn apply(&self, px: &mut [u32]) {
        let t = &self.0;
        for p in px {
            let [r, g, b, a] = p.to_le_bytes();
            *p = pack(
                t[0][r as usize],
                t[1][g as usize],
                t[2][b as usize],
                t[3][a as usize],
            );
        }
    }
}

/// The rounded-rect coverage of `fs_quad` at local offsets `(lx, ly)` from the
/// centre, times the colour's alpha.
#[inline(always)]
fn quad_alpha<V: Lanes>(lx: V, ly: V, g: &QuadGeom) -> V {
    let zero = V::splat(0.0);
    let qx = lx.abs() - V::splat(g.hx - g.r);
    let qy = ly.abs() - V::splat(g.hy - g.r);
    let mx = qx.max(zero);
    let my = qy.max(zero);
    let outside = (mx * mx + my * my).sqrt();
    let inside = qx.max(qy).min(zero);
    let sd = outside + inside - V::splat(g.r);
    let mut cov = V::splat(1.0) - smooth(sd);
    if g.stroke > 0.0 {
        cov = cov * smooth(sd + V::splat(g.stroke));
    }
    cov * V::splat(g.alpha)
}

/// One quad's physical geometry, as `vs_quad` hands it to `fs_quad`.
struct QuadGeom {
    /// Centre.
    cx: f32,
    cy: f32,
    /// Half size.
    hx: f32,
    hy: f32,
    /// Corner radius, at most the half size.
    r: f32,
    /// Stroke width; 0 fills.
    stroke: f32,
    alpha: f32,
}

#[inline(always)]
fn quad<V: Lanes>(r: &mut Raster<'_>, q: &QuadInstance, region: Region) {
    let alpha = q.color[3];
    if !(alpha > 0.0) {
        return;
    }
    let s = r.scale;
    let [x, y, w, h] = q.rect;
    let (cxl, cyl) = (x + w * 0.5, y + h * 0.5);
    let hx = w * 0.5 * s;
    let hy = h * 0.5 * s;
    let g = QuadGeom {
        cx: cxl * s,
        cy: cyl * s,
        hx,
        hy,
        r: (q.params[0] * s).min(hx.min(hy)),
        stroke: q.params[2] * s,
        alpha,
    };
    if !(g.hx > 0.0 && g.hy > 0.0) {
        return;
    }
    let ((cx0, cx1), (cy0, cy1)) = clip_spans(region, q.clip, s);
    let paint = Paint::new(r.luts, q.color);
    let angle = q.params[1];
    if angle != 0.0 {
        return rotated_quad::<V>(r, &g, angle, (cx0, cx1, cy0, cy1), &paint);
    }
    // The pixels the rasterizer shades: centres inside the quad's vertices, left
    // and top edges inclusive.
    let (left, right) = (snap((cxl + -0.5 * w) * s), snap((cxl + 0.5 * w) * s));
    let (top, bottom) = (snap((cyl + -0.5 * h) * s), snap((cyl + 0.5 * h) * s));
    let (qx0, qx1) = span(cx0, cx1, (left, right), |p| p >= left && p < right);
    let (qy0, qy1) = span(cy0, cy1, (top, bottom), |p| p >= top && p < bottom);
    if qx0 >= qx1 || qy0 >= qy1 {
        return;
    }
    // Inside this box a fill has coverage exactly 1 and a stroke exactly 0.
    let m = if g.stroke > 0.0 {
        g.r.max(g.stroke + 0.5)
    } else {
        g.r.max(0.5)
    };
    let (ex, ey) = (g.hx - m, g.hy - m);
    let (ix0, ix1) = span(qx0, qx1, (g.cx - ex, g.cx + ex), |p| (p - g.cx).abs() <= ex);
    let (iy0, iy1) = span(qy0, qy1, (g.cy - ey, g.cy + ey), |p| (p - g.cy).abs() <= ey);
    let solid = ix0 < ix1 && iy0 < iy1 && g.stroke <= 0.0;
    let table =
        (solid && alpha < 1.0 && (ix1 - ix0) as i64 * (iy1 - iy0) as i64 >= TABLE_MIN_PIXELS)
            .then(|| BlendTable::new(r.luts, &paint, alpha));
    let width = r.width;
    for row in qy0..qy1 {
        let ly = V::splat((row as f32 + 0.5) - g.cy);
        let px = &mut r.pixels[row as usize * width..(row as usize + 1) * width];
        let inner = row >= iy0 && row < iy1 && ix0 < ix1;
        let luts = r.luts;
        if !inner {
            quad_edge(luts, px, qx0, qx1, ly, &g, &paint);
            continue;
        }
        quad_edge(luts, px, qx0, ix0, ly, &g, &paint);
        if solid {
            let inside = &mut px[ix0 as usize..ix1 as usize];
            if alpha >= 1.0 {
                inside.fill(paint.solid);
            } else if let Some(table) = &table {
                table.apply(inside);
            } else {
                let a = V::splat(alpha);
                for chunk in inside.chunks_mut(LANES) {
                    let n = chunk.len();
                    blend_paint(luts, chunk, n, a, &paint);
                }
            }
        }
        quad_edge(luts, px, ix1, qx1, ly, &g, &paint);
    }
}

/// Pixels ~[a, b)~ of one row of an axis-aligned quad, through the distance
/// field. (A function, not a closure: a closure would not inherit the AVX
/// target feature, and the lane methods would stop inlining.)
#[inline(always)]
fn quad_edge<V: Lanes>(
    luts: &ColorLuts,
    px: &mut [u32],
    a: i32,
    b: i32,
    ly: V,
    g: &QuadGeom,
    paint: &Paint,
) {
    let mut i = a;
    while i < b {
        let n = ((b - i) as usize).min(LANES);
        let lx = V::ramp(i as f32 + 0.5) - V::splat(g.cx);
        let cov = quad_alpha(lx, ly, g);
        blend_paint(luts, &mut px[i as usize..], n, cov, paint);
        i += LANES as i32;
    }
}

/// A quad with a rotation (a `Primitive::Line`): its bounding box, lanes masked to
/// the centres inside the rotated quad.
#[inline(always)]
fn rotated_quad<V: Lanes>(
    r: &mut Raster<'_>,
    g: &QuadGeom,
    angle: f32,
    (cx0, cx1, cy0, cy1): (i32, i32, i32, i32),
    paint: &Paint,
) {
    let (sa, ca) = angle.sin_cos();
    let ex = (g.hx * ca).abs() + (g.hy * sa).abs();
    let ey = (g.hx * sa).abs() + (g.hy * ca).abs();
    let x0 = index((g.cx - ex - 0.5).floor()).max(cx0);
    let x1 = index((g.cx + ex + 0.5).ceil() + 1.0).min(cx1);
    let y0 = index((g.cy - ey - 0.5).floor()).max(cy0);
    let y1 = index((g.cy + ey + 0.5).ceil() + 1.0).min(cy1);
    let (vca, vsa) = (V::splat(ca), V::splat(sa));
    let (nhx, phx) = (V::splat(-g.hx), V::splat(g.hx));
    let (nhy, phy) = (V::splat(-g.hy), V::splat(g.hy));
    let width = r.width;
    for row in y0..y1 {
        let fy = (row as f32 + 0.5) - g.cy;
        // The row's centre line crosses the rotated quad in one interval of x:
        // |dx ca + dy sa| <= hx and |dy ca - dx sa| <= hy, each linear in dx.
        // One pixel of margin each side; the lanes are masked exactly below.
        let (mut lo, mut hi) = (f32::NEG_INFINITY, f32::INFINITY);
        let mut band = |k: f32, c: f32, h: f32| {
            if k.abs() > 1.0e-6 {
                let (a, b) = ((-h - c) / k, (h - c) / k);
                lo = lo.max(a.min(b));
                hi = hi.min(a.max(b));
            } else if c.abs() > h {
                hi = f32::NEG_INFINITY;
            }
        };
        band(ca, fy * sa, g.hx);
        band(-sa, fy * ca, g.hy);
        if !(lo <= hi) {
            continue;
        }
        let rx0 = index((g.cx + lo - 1.5).floor()).max(x0);
        let rx1 = index((g.cx + hi + 1.5).ceil()).min(x1);
        let dy = V::splat(fy);
        let px = &mut r.pixels[row as usize * width..(row as usize + 1) * width];
        let mut i = rx0;
        while i < rx1 {
            let n = ((rx1 - i) as usize).min(LANES);
            let dx = V::ramp(i as f32 + 0.5) - V::splat(g.cx);
            // The unrotated local frame of `vs_quad`.
            let lx = dx * vca + dy * vsa;
            let ly = dy * vca - dx * vsa;
            let inside = nhx.le(lx).and(lx.lt(phx)).and(nhy.le(ly)).and(ly.lt(phy));
            let cov = quad_alpha(lx, ly, g).and(inside);
            blend_paint(r.luts, &mut px[i as usize..], n, cov, paint);
            i += LANES as i32;
        }
    }
}

/// The pixel columns and rows a glyph or image quad shades, and its physical
/// origin and size.
/// Pixel columns and rows a textured rect covers, and its physical rect.
type TexturedSpans = ((i32, i32), (i32, i32), [f32; 4]);

#[inline(always)]
fn textured_spans(region: Region, rect: [f32; 4], clip: [f32; 4], s: f32) -> Option<TexturedSpans> {
    let [x, y, w, h] = rect;
    let (x0, x1) = (snap(x * s), snap((x + w) * s));
    let (y0, y1) = (snap(y * s), snap((y + h) * s));
    let ((cx0, cx1), (cy0, cy1)) = clip_spans(region, clip, s);
    let cols = span(cx0, cx1, (x0, x1), |p| p >= x0 && p < x1);
    let rows = span(cy0, cy1, (y0, y1), |p| p >= y0 && p < y1);
    // The texel pick measures from the unsnapped origin, as ~fs_glyph~ does.
    (cols.0 < cols.1 && rows.0 < rows.1).then_some((cols, rows, [x * s, y * s, w * s, h * s]))
}

/// `fs_glyph`: an R8 coverage texel (or a colour-plane texel for a colour glyph)
/// picked from the pixel's offset into the quad.
#[inline(always)]
fn glyph<V: Lanes>(r: &mut Raster<'_>, g: &GlyphInstance, region: Region) {
    let alpha = g.color[3];
    let [tx, ty, tw, th] = g.atlas_uv;
    if !(alpha > 0.0 && tw >= 1.0 && th >= 1.0) {
        return;
    }
    let Some(((qx0, qx1), (qy0, qy1), [x0, y0, ws, hs])) =
        textured_spans(region, g.rect, g.clip, r.scale)
    else {
        return;
    };
    let color = tx >= schnellui::text::COLOR_GLYPH_UV_BIAS;
    let bias = if color { 65536 } else { 0 };
    let texel = |p: f32, origin: f32, size: f32, start: f32, len: f32| {
        let local = (p - origin) / size * len;
        let t = (local + TEXEL_NUDGE).floor().max(0.0).min(len - 1.0);
        (start + t) as i32
    };
    r.cols.clear();
    for i in qx0..qx1 {
        r.cols.push(texel(i as f32 + 0.5, x0, ws, tx, tw) - bias);
    }
    let paint = Paint::new(r.luts, g.color);
    let luts = r.luts;
    let plane = if color { r.src.colors } else { r.src.glyphs };
    let width = r.width;
    for row in qy0..qy1 {
        let trow = texel(row as f32 + 0.5, y0, hs, ty, th);
        let px = &mut r.pixels[row as usize * width..(row as usize + 1) * width];
        for (c, cols) in r.cols.chunks(LANES).enumerate() {
            let start = qx0 as usize + c * LANES;
            let n = cols.len();
            if color {
                let mut lanes = [[0.0; LANES]; 4];
                for (k, &tx) in cols.iter().enumerate() {
                    if let Some(at) = plane.offset(tx, trow, 4) {
                        let t = &plane.data[at..at + 4];
                        lanes[0][k] = luts.decode[t[0] as usize];
                        lanes[1][k] = luts.decode[t[1] as usize];
                        lanes[2][k] = luts.decode[t[2] as usize];
                        lanes[3][k] = luts.unorm[t[3] as usize];
                    }
                }
                let rgb = [V::load(&lanes[0]), V::load(&lanes[1]), V::load(&lanes[2])];
                let a = V::load(&lanes[3]) * V::splat(alpha);
                blend_lanes(luts, &mut px[start..], n, rgb, a);
            } else {
                let mut cov = [0.0; LANES];
                let mut any = false;
                for (k, &tx) in cols.iter().enumerate() {
                    if let Some(at) = plane.offset(tx, trow, 1) {
                        let v = plane.data[at];
                        any |= v != 0;
                        cov[k] = luts.unorm[v as usize];
                    }
                }
                if any {
                    let a = V::load(&cov) * V::splat(alpha);
                    blend_paint(luts, &mut px[start..], n, a, &paint);
                }
            }
        }
    }
}

/// `fs_image`: the nearest image-atlas texel, decoded and tinted.
#[inline(always)]
fn image<V: Lanes>(r: &mut Raster<'_>, g: &GlyphInstance, region: Region) {
    let alpha = g.color[3];
    let [tx, ty, tw, th] = g.atlas_uv;
    let plane = r.src.images;
    if !(alpha > 0.0) || plane.width == 0 || plane.height == 0 {
        return;
    }
    let Some(((qx0, qx1), (qy0, qy1), [x0, y0, ws, hs])) =
        textured_spans(region, g.rect, g.clip, r.scale)
    else {
        return;
    };
    // Nearest sampling with clamp-to-edge addressing.
    let texel = |p: f32, origin: f32, size: f32, start: f32, len: f32, max: u32| {
        let u = (start + (p - origin) / size * len).floor();
        u.max(0.0).min(max as f32 - 1.0) as i32
    };
    r.cols.clear();
    for i in qx0..qx1 {
        r.cols
            .push(texel(i as f32 + 0.5, x0, ws, tx, tw, plane.width));
    }
    let luts = r.luts;
    // Untinted, an opaque texel blends to itself: decode, times 1, encode.
    let untinted = g.color == [1.0; 4];
    let opaque = alpha >= 1.0;
    let tint = Paint::new(luts, g.color).lin;
    let (tr, tg, tb) = (V::splat(tint[0]), V::splat(tint[1]), V::splat(tint[2]));
    let width = r.width;
    for row in qy0..qy1 {
        let trow = texel(row as f32 + 0.5, y0, hs, ty, th, plane.height);
        let px = &mut r.pixels[row as usize * width..(row as usize + 1) * width];
        let mut memo = (usize::MAX, 0u32);
        for (c, cols) in r.cols.chunks(LANES).enumerate() {
            let start = qx0 as usize + c * LANES;
            let mut lanes = [[0.0; LANES]; 4];
            let mut copied = 0u32;
            for (k, &tx) in cols.iter().enumerate() {
                if let Some(at) = plane.offset(tx, trow, 4) {
                    let t = &plane.data[at..at + 4];
                    if t[3] == 255 && opaque {
                        // Covers the destination: the result depends on the texel
                        // alone, and a scaled-up image repeats it along the row.
                        if at != memo.0 {
                            memo = (
                                at,
                                if untinted {
                                    pack(t[0], t[1], t[2], 255)
                                } else {
                                    pack(
                                        luts.encode(luts.decode[t[0] as usize] * tint[0]),
                                        luts.encode(luts.decode[t[1] as usize] * tint[1]),
                                        luts.encode(luts.decode[t[2] as usize] * tint[2]),
                                        255,
                                    )
                                },
                            );
                        }
                        px[start + k] = memo.1;
                        copied |= 1 << k;
                        continue;
                    }
                    lanes[0][k] = luts.decode[t[0] as usize];
                    lanes[1][k] = luts.decode[t[1] as usize];
                    lanes[2][k] = luts.decode[t[2] as usize];
                    lanes[3][k] = luts.unorm[t[3] as usize];
                }
            }
            if copied == live(cols.len()) {
                continue;
            }
            let rgb = [
                V::load(&lanes[0]) * tr,
                V::load(&lanes[1]) * tg,
                V::load(&lanes[2]) * tb,
            ];
            let a = V::load(&lanes[3]) * V::splat(alpha);
            blend_lanes(luts, &mut px[start..], cols.len(), rgb, a);
        }
    }
}
