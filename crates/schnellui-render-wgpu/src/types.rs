// # schnellui-render-wgpu
//
// The WebGPU backend (SOUL §5, §7.2). A **headless one-frame renderer**: offscreen
// `Rgba8UnormSrgb` texture → render pass drawing **instanced solid-rect quads** and
// **glyph quads** → `copy_texture_to_buffer` → `map_async` → PNG bytes.
//
// It consumes the retained [`Scene`](schnellui_scene::Scene) primitives and the
// [`GlyphAtlas`](schnellui_text::GlyphAtlas), uploading only deltas
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

use schnellui_scene::{Color, Point, Rect, WidgetId};

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

#[derive(Clone, Copy, Debug, Default)]
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

/// One terminal's stable ranges in the three resident instance buffers. Ranges are
/// captured during a full scene walk and reused only while tree/layout traversal is
/// unchanged. A normal terminal cell update can then write these subranges directly.
#[derive(Clone, Debug, Default)]
pub(crate) struct TerminalGpuFragment {
    pub(crate) quads: std::ops::Range<u32>,
    pub(crate) glyphs: std::ops::Range<u32>,
    pub(crate) images: std::ops::Range<u32>,
    pub(crate) offset: Point,
    pub(crate) clip: Rect,
    /// Stable walker order disambiguates zero-length ranges, which otherwise all
    /// share one buffer index before the first echoed glyph arrives.
    pub(crate) ordinal: u32,
}

/// Last upload's deterministic work counters. Tests inspect them after a render to
/// prove that a terminal delta did not fall back to a full tree gather.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GpuUploadWork {
    pub(crate) full_gathers: usize,
    pub(crate) terminal_fragments: usize,
    pub(crate) instance_writes: usize,
    pub(crate) instances_written: usize,
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
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
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

/// The GPU-side glyph atlas: an R8 texture kept in sync with a CPU [`GlyphAtlas`]
/// via `write_texture` sub-rect uploads (SOUL §3.2).
pub(crate) struct AtlasGpu {
    #[allow(dead_code)]
    pub(crate) texture: wgpu::Texture,
    #[allow(dead_code)]
    pub(crate) view: wgpu::TextureView,
    pub(crate) bind_group: wgpu::BindGroup,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// The GPU-side **image** atlas: an RGBA8-sRGB texture kept in sync with the scene's
/// CPU [`ImageAtlas`](schnellui_scene::ImageAtlas) (SOUL §3.2). Staleness is a
/// revision compare — the CPU atlas bumps its revision on every write, so the sync
/// needs only `&Scene` and an unchanged atlas costs nothing per frame.
pub(crate) struct ImageAtlasGpu {
    pub(crate) texture: wgpu::Texture,
    #[allow(dead_code)]
    pub(crate) view: wgpu::TextureView,
    pub(crate) bind_group: wgpu::BindGroup,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// the CPU revision this texture was last uploaded at.
    pub(crate) revision: u64,
}

/// The **shared GPU core** (SOUL §3.2, §7.2): device/queue, the uniform + glyph
/// pipelines, the resident grow-only instance buffers, and the GPU glyph atlas — the
/// render-pass machinery factored out of the (headless) [`Renderer`] so a window
/// [`SurfaceRenderer`] can drive the **identical** gather → upload → draw path against
/// a swapchain view instead of an offscreen texture. Pipelines are built for a
/// caller-supplied target format, so an offscreen `Rgba8UnormSrgb` target and a
/// window's preferred sRGB surface format run the same shaders unchanged.
pub(crate) struct GpuCore {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,

    pub(crate) uniform_buf: wgpu::Buffer,
    pub(crate) uniform_bg: wgpu::BindGroup,

    pub(crate) quad_pipeline: wgpu::RenderPipeline,
    pub(crate) glyph_pipeline: wgpu::RenderPipeline,
    pub(crate) image_pipeline: wgpu::RenderPipeline,

    pub(crate) sampler: wgpu::Sampler,
    pub(crate) glyph_bgl: wgpu::BindGroupLayout,

    // resident, grow-only instance buffers (§3.2, §4.4): steady-state re-render
    // reuses them via `write_buffer`, never reallocates.
    pub(crate) quad_buf: Option<wgpu::Buffer>,
    pub(crate) quad_cap: u32,
    pub(crate) glyph_buf: Option<wgpu::Buffer>,
    pub(crate) glyph_cap: u32,
    pub(crate) image_buf: Option<wgpu::Buffer>,
    pub(crate) image_cap: u32,

    pub(crate) atlas: Option<AtlasGpu>,
    /// CPU mirror of the resident glyph texture. Besides making atlas ownership
    /// explicit, this lets a structural remount reconcile a freshly built atlas
    /// against what is actually on the GPU instead of destroying the texture.
    pub(crate) atlas_shadow: Option<Vec<u8>>,
    pub(crate) image_atlas: Option<ImageAtlasGpu>,

    // cross-frame scratch (cleared-and-refilled, retains capacity — §4.4).
    pub(crate) quad_scratch: Vec<QuadInstance>,
    /// Fixed scroll chrome, appended after each layer's content so it draws after
    /// images and glyphs as well as ordinary quads.
    pub(crate) chrome_scratch: Vec<QuadInstance>,
    pub(crate) glyph_scratch: Vec<GlyphInstance>,
    /// image instances reuse the glyph layout (dest rect, texel uv, tint, clip).
    pub(crate) image_scratch: Vec<GlyphInstance>,
    /// pre-order walk frontier carrying each node's accumulated scroll `offset` and
    /// `clip` (SOUL §3.2). Tuple elements are `Copy`, so it stays a cleared-and-refilled
    /// grow-only `Vec` — zero steady-state allocation (§4.4).
    /// The final boolean is an exit marker used to composite fixed scroll chrome
    /// after the viewport's moving descendants.
    pub(crate) walk_stack: Vec<(schnellui_scene::WidgetId, Point, Rect, bool)>,
    /// overlay subtree roots deferred out of the base walk (SOUL §3.2 z-order),
    /// each with the (offset, clip) it inherited where it sits in the tree; walked
    /// after the base pass so their instances land in the overlay draw ranges.
    /// Cleared-and-refilled like the walk stack (§4.4); empty for overlay-less
    /// scenes, so the whole layer machinery costs nothing then.
    pub(crate) overlay_roots: Vec<(schnellui_scene::WidgetId, Point, Rect)>,
    /// instance counts at the base/overlay boundary: scratch `[0..base_*)` is the
    /// base layer, `[base_*..len)` the overlay layer ([`GpuCore::record_pass`]).
    pub(crate) base_quads: u32,
    pub(crate) base_chrome_start: u32,
    pub(crate) base_glyphs: u32,
    pub(crate) base_images: u32,
    /// Complete overlay draw ranges in bottom-to-top order. Surfaces, images,
    /// and text from one overlay are composited before the next overlay begins.
    pub(crate) overlay_layers: Vec<InstanceLayer>,
    pub(crate) atlas_scratch: Vec<u8>,

    /// Stable terminal ranges from the most recent full gather. A dirty base-layer
    /// terminal can splice its instances and reuse every other node's CPU and GPU
    /// data. Tree, layout, scroll, overlay, and image changes use the normal gather.
    pub(crate) terminal_fragments: HashMap<WidgetId, TerminalGpuFragment>,
    pub(crate) retained_scene_key: Option<schnellui_scene::SceneRenderKey>,
    pub(crate) terminal_quad_scratch: Vec<QuadInstance>,
    pub(crate) terminal_glyph_scratch: Vec<GlyphInstance>,
    pub(crate) terminal_image_scratch: Vec<GlyphInstance>,
    pub(crate) next_terminal_fragment_ordinal: u32,
    pub(crate) last_upload_work: GpuUploadWork,
}
