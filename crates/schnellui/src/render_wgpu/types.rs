// # schnellui::render_wgpu
//
// The WebGPU backend (SOUL §5, §7.2). A **headless one-frame renderer**: offscreen
// `Rgba8UnormSrgb` texture → render pass drawing **instanced solid-rect quads** and
// **glyph quads** → `copy_texture_to_buffer` → `map_async` → PNG bytes.
//
// It consumes the retained [`Scene`](schnellui::scene::Scene) primitives and the
// [`GlyphAtlas`](schnellui::text::GlyphAtlas), uploading only deltas
// (`write_buffer` / `write_texture`) — the incremental-upload discipline Vello
// declines and we depend on (SOUL §3.2).
//
// The **256-byte row alignment** gotcha (SOUL §7.2) is handled by
// [`padded_bytes_per_row`] + [`unpad_rows`], written down so nobody rediscovers it.
//
// ## Two targets, one render pass (opt-in windowed mode)
//
// The gather → upload → draw machinery is factored into a private [`GpuCore`] so it
// can target **either** the offscreen texture (the byte-identical PNG path,
// [`Renderer`]) **or** a live winit window surface ([`SurfaceRenderer`]). Headless
// stays the default; the windowed path is only reached through
// `schnellui::App::run_windowed`. The two differ solely in their color target — an
// offscreen `Rgba8UnormSrgb` texture vs the window's preferred sRGB surface format —
// so the shaders, blend, vertex layouts, and draw sequence are shared unchanged.

use std::collections::HashMap;
use std::fmt;

use schnellui::scene::{Color, Point, Rect, WidgetId};

/// `wgpu`'s `COPY_BYTES_PER_ROW_ALIGNMENT` (SOUL §7.2). `copy_texture_to_buffer`
/// requires `bytes_per_row` to be a multiple of this.
pub const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// Bytes per pixel for the `Rgba8UnormSrgb` readback format.
pub const BYTES_PER_PIXEL: u32 = 4;

/// The offscreen render-target / readback format (SOUL §7.2).
pub(crate) const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Rounds `width * bytes_per_pixel` up to the next multiple of
/// [`COPY_BYTES_PER_ROW_ALIGNMENT`] (SOUL §7.2). **Always pad** — never rely on the
/// width happening to be aligned.
pub fn padded_bytes_per_row(width: u32, bytes_per_pixel: u32) -> u32 {
    let unpadded = width * bytes_per_pixel;
    let align = COPY_BYTES_PER_ROW_ALIGNMENT;
    let rem = unpadded % align;
    if rem == 0 {
        unpadded
    } else {
        unpadded + (align - rem)
    }
}

/// Strips the per-row padding from a mapped readback buffer, producing tightly
/// packed RGBA8 rows ready for PNG encoding (SOUL §7.2).
///
/// `padded` is the mapped buffer; each source row is `padded_bpr` bytes, of which
/// the leading `width * BYTES_PER_PIXEL` are real pixels.
pub fn unpad_rows(padded: &[u8], width: u32, height: u32, padded_bpr: u32) -> Vec<u8> {
    let row_bytes = (width * BYTES_PER_PIXEL) as usize;
    let mut out = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * padded_bpr as usize;
        out.extend_from_slice(&padded[start..start + row_bytes]);
    }
    out
}

/// The sentinel "unclipped" clip rect (`[x, y, w, h]`, logical px). It covers all of
/// logical space, so the fragment shaders' per-instance clip test is a single
/// always-pass branch when a primitive is not inside a `Scroll` viewport (SOUL §3.2).
pub const UNCLIPPED_CLIP: [f32; 4] = [-1.0e9, -1.0e9, 2.0e9, 2.0e9];

/// Straight RGBA8 [`Color`] → normalized `[0,1]` float channels (shared by every
/// instance constructor; the shader decodes sRGB→linear per §7.2).
#[inline]
pub(crate) fn color_rgba_f32(color: Color) -> [f32; 4] {
    [
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        color.a as f32 / 255.0,
    ]
}

/// GPU instance data for one solid-rect quad (SOUL §7.2). Ordered by update
/// frequency in the resident buffer (static first) per §3.2. A `Primitive::Line`
/// also rides this instance as an **oriented** quad (a nonzero rotation in `params.y`);
/// axis-aligned rects keep rotation `0` and stay on the exact same fast path.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct QuadInstance {
    /// x, y, width, height in logical pixels. For a line this is the segment's
    /// **unrotated** bounding box (centre − ½·[len, width]); `params.y` rotates it.
    pub rect: [f32; 4],
    /// straight RGBA in [0,1].
    pub color: [f32; 4],
    /// `.0` = corner radius (px), `.1` = rotation (radians, about the rect centre);
    /// `.2..` reserved.
    pub params: [f32; 4],
    /// per-instance clip rect `[x, y, w, h]` in **logical** px (the same space as
    /// `rect` before the vertex stage applies scale). [`UNCLIPPED_CLIP`] disables it.
    pub clip: [f32; 4],
}

impl QuadInstance {
    /// Builds an **unclipped**, axis-aligned solid-rect instance from scene types.
    pub fn solid(rect: Rect, color: Color, corner_radius: f32) -> QuadInstance {
        QuadInstance::solid_clipped(rect, color, corner_radius, UNCLIPPED_CLIP)
    }

    /// Builds an axis-aligned (rotation `0`) solid-rect instance carrying a `clip`
    /// rect — the form the scroll-composited gather emits (SOUL §3.2).
    pub fn solid_clipped(
        rect: Rect,
        color: Color,
        corner_radius: f32,
        clip: [f32; 4],
    ) -> QuadInstance {
        QuadInstance {
            rect: [rect.x, rect.y, rect.width, rect.height],
            color: color_rgba_f32(color),
            params: [corner_radius, 0.0, 0.0, 0.0],
            clip,
        }
    }

    /// Builds a **line** instance: an oriented quad from `from` to `to` with stroke
    /// `width` (SOUL §3.2). Encoded as centre `(from+to)/2`, length `|to−from|`, and a
    /// rotation angle so the vertex shader places the unit quad along the segment.
    /// Sharp caps (corner radius `0`); axis-aligned when the segment is horizontal.
    pub fn line(from: Point, to: Point, width: f32, color: Color, clip: [f32; 4]) -> QuadInstance {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let length = (dx * dx + dy * dy).sqrt();
        let angle = dy.atan2(dx);
        let cx = (from.x + to.x) * 0.5;
        let cy = (from.y + to.y) * 0.5;
        QuadInstance {
            // unrotated bbox centred on the midpoint; the shader rotates it by `angle`.
            rect: [cx - length * 0.5, cy - width * 0.5, length, width],
            color: color_rgba_f32(color),
            params: [0.0, angle, 0.0, 0.0],
            clip,
        }
    }
}

/// GPU instance data for one glyph quad: a dest rect sampling the R8 atlas.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlyphInstance {
    pub rect: [f32; 4],
    /// atlas texel rect (x, y, w, h).
    pub atlas_uv: [f32; 4],
    pub color: [f32; 4],
    /// per-instance clip rect `[x, y, w, h]` in logical px ([`UNCLIPPED_CLIP`] off).
    pub clip: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct InstanceLayer {
    pub(crate) quad_start: u32,
    pub(crate) quad_end: u32,
    pub(crate) chrome_start: u32,
    pub(crate) chrome_end: u32,
    pub(crate) glyph_start: u32,
    pub(crate) glyph_end: u32,
    pub(crate) image_start: u32,
    pub(crate) image_end: u32,
}

/// The three instance buffers a fragment can own a range in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Buf {
    /// Solid quads and lines. Scroll chrome sits after each layer's content.
    Quad = 0,
    Glyph = 1,
    Image = 2,
}

pub(crate) const BUFS: [Buf; 3] = [Buf::Quad, Buf::Glyph, Buf::Image];

/// `GpuFragment::ord` value for a buffer the fragment has no list entry in (yet).
pub(crate) const NO_ORD: u32 = u32::MAX;

/// One node's retained slot in the three resident instance buffers, in draw order.
///
/// Every visited node owns one range per buffer, captured by a full scene walk. A
/// range is the slot's capacity: the first `live[b]` instances are the node's, the
/// rest is zero-area padding the GPU draws as nothing. A paint-dirty node re-encodes
/// only its own instances and rewrites only its own slot (`GpuCore::try_incremental`).
///
/// A `Scroll` node is special only in where its slots sit: its fixed chrome quads
/// land in the layer's chrome section (so `quads` is that range), and its glyphs and
/// images are emitted after its content. Two marker fragments (`region`) bracket the
/// content it composites, so a scroll can replace that run in one piece.
#[derive(Clone, Debug, Default)]
pub(crate) struct GpuFragment {
    pub(crate) quads: std::ops::Range<u32>,
    pub(crate) glyphs: std::ops::Range<u32>,
    pub(crate) images: std::ops::Range<u32>,
    /// Instances at the front of each range that are real (indexed by [`Buf`]).
    pub(crate) live: [u32; 3],
    /// Scroll offset and clip inherited where the node sits in the tree.
    pub(crate) offset: Point,
    pub(crate) clip: Rect,
    /// Position in each buffer's draw-order list ([`FragmentTable::order`]).
    pub(crate) ord: [u32; 3],
    /// 0 for the base layer, `n + 1` for the n-th overlay.
    pub(crate) layer: u32,
    /// The slot already outgrew its capacity once; the next growth adds headroom.
    pub(crate) overflowed: bool,
    /// `quads` is scroll chrome (drawn after the layer's content).
    pub(crate) chrome: bool,
    /// A scroll's (begin, end) marker fragment indices.
    pub(crate) region: Option<(u32, u32)>,
    /// Walk generation that last refreshed this fragment (region re-walks).
    pub(crate) stamp: u32,
}

impl GpuFragment {
    pub(crate) fn range(&self, b: Buf) -> &std::ops::Range<u32> {
        match b {
            Buf::Quad => &self.quads,
            Buf::Glyph => &self.glyphs,
            Buf::Image => &self.images,
        }
    }

    pub(crate) fn range_mut(&mut self, b: Buf) -> &mut std::ops::Range<u32> {
        match b {
            Buf::Quad => &mut self.quads,
            Buf::Glyph => &mut self.glyphs,
            Buf::Image => &mut self.images,
        }
    }
}

/// All fragments of the last full gather plus each buffer's draw order. The order
/// lists make "everything drawn after this slot" a slice, which is what a slot that
/// outgrows its capacity must shift; ties between empty ranges at one index resolve
/// by list position instead of by guessing.
#[derive(Default)]
pub(crate) struct FragmentTable {
    pub(crate) frags: Vec<GpuFragment>,
    pub(crate) by_node: HashMap<WidgetId, u32>,
    /// Fragment indices in draw order, per buffer.
    pub(crate) order: [Vec<u32>; 3],
}

impl FragmentTable {
    pub(crate) fn clear(&mut self) {
        self.frags.clear();
        self.by_node.clear();
        for list in &mut self.order {
            list.clear();
        }
    }

    /// The fragment retained for `id` by the last full gather.
    #[cfg(test)]
    pub(crate) fn get(&self, id: &WidgetId) -> Option<&GpuFragment> {
        self.by_node.get(id).map(|&i| &self.frags[i as usize])
    }
}

/// Bytes handed to `queue.write_buffer` / `write_texture` by one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadBytes {
    /// Quad, glyph and image instance data.
    pub instance_bytes: u64,
    /// Uniforms and atlas texels.
    pub other_bytes: u64,
}

impl UploadBytes {
    pub fn total(&self) -> u64 {
        self.instance_bytes + self.other_bytes
    }
}

/// Last upload's deterministic work counters. Tests inspect them after a render to
/// prove that a paint change did not fall back to a full tree gather.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GpuUploadWork {
    pub(crate) full_gathers: usize,
    /// Fragments re-encoded in place (one per paint-dirty node).
    pub(crate) nodes_patched: usize,
    /// `Scroll` content runs re-walked.
    pub(crate) scroll_regions: usize,
    /// Slots that outgrew their capacity and shifted the slots behind them.
    pub(crate) grown_slots: usize,
    /// Paint-dirty nodes updated from their recorded primitive ranges only.
    pub(crate) ranges_patched: usize,
    /// Primitives re-encoded by incremental updates (whole nodes plus ranges).
    pub(crate) primitives_encoded: usize,
    pub(crate) instance_writes: usize,
    pub(crate) instances_written: usize,
    pub(crate) bytes: UploadBytes,
}

impl GlyphInstance {
    /// Builds an **unclipped** glyph-quad instance from scene types. `atlas_uv` is a
    /// **texel** rect into the shared R8 atlas (the shader normalizes it by the atlas
    /// size).
    pub fn glyph(rect: Rect, atlas_uv: Rect, color: Color) -> GlyphInstance {
        GlyphInstance::glyph_clipped(rect, atlas_uv, color, UNCLIPPED_CLIP)
    }

    /// Builds a glyph-quad instance carrying a `clip` rect — the form the
    /// scroll-composited gather emits for text inside a `Scroll` viewport (SOUL §3.2).
    pub fn glyph_clipped(
        rect: Rect,
        atlas_uv: Rect,
        color: Color,
        clip: [f32; 4],
    ) -> GlyphInstance {
        GlyphInstance {
            rect: [rect.x, rect.y, rect.width, rect.height],
            atlas_uv: [atlas_uv.x, atlas_uv.y, atlas_uv.width, atlas_uv.height],
            color: color_rgba_f32(color),
            clip,
        }
    }
}

/// The per-frame shader uniforms: viewport size (for the NDC transform), the
/// glyph-atlas texel dimensions (for UV normalization), and the logical→physical
/// scale (SOUL §7.1 `--scale`). The `viewport` is in **physical** pixels; incoming
/// primitive coordinates are **logical** and are multiplied by `params.x` (scale) in
/// the vertex stage, so logical geometry maps onto a `scale`×-larger target.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniforms {
    pub(crate) viewport: [f32; 2],
    pub(crate) atlas_size: [f32; 2],
    /// `params[0]` = scale; the rest is reserved padding (16-byte aligned for WGSL).
    pub(crate) params: [f32; 4],
}

/// Which wgpu adapter to request (SOUL §7.3 — software for cross-machine-stable
/// goldens).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    /// the fastest available hardware adapter.
    #[default]
    Auto,
    /// force a software rasterizer (lavapipe / SwiftShader / WARP).
    Software,
}

/// Why a [`Renderer`] / [`SurfaceRenderer`] could not be constructed (SOUL §7.2).
/// Distinct from a panic so the headless harness can **skip gracefully** when no
/// adapter is present, and the windowed path can surface a clean error.
#[derive(Debug)]
pub enum RendererError {
    /// no wgpu adapter matched the request (e.g. `Software` with no lavapipe/WARP).
    NoAdapter,
    /// an adapter was found but `request_device` failed.
    NoDevice(String),
    /// a window surface could not be created from the given window handle
    /// (windowed mode only).
    NoSurface(String),
}

impl fmt::Display for RendererError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RendererError::NoAdapter => write!(f, "no wgpu adapter available"),
            RendererError::NoDevice(e) => write!(f, "wgpu request_device failed: {e}"),
            RendererError::NoSurface(e) => write!(f, "wgpu create_surface failed: {e}"),
        }
    }
}

impl std::error::Error for RendererError {}
