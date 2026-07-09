//! The vector-image component (SOUL §8.1): [`Svg`] parses a **documented subset**
//! of SVG markup, CPU-rasterizes it with supersampled anti-aliasing at the
//! *physical* pixel scale, and draws the result as a real
//! [`Primitive::ImageQuad`] out of the scene's shared image atlas (SOUL §3.2) —
//! so vector icons stay crisp under `--scale` (SOUL §7.1) and the render path
//! needs no path tessellation.
//!
//! ## The async pipeline (SOUL §8.1)
//!
//! Rasterization happens **off the UI thread**: build reserves the atlas rect
//! and emits the quad (final geometry/UVs, transparent until the pixels land),
//! a worker pool rasterizes, and completions are pulled back in —
//! [`drain_svg_rasters`] per frame in windowed mode (icons pop in a frame or
//! two later), [`settle_svg_rasters`] blocking on the headless one-shot path so
//! the single deterministic frame contains every image (SOUL §7.3). Worker
//! shapers use the same embedded font, so the pixels are identical either way.
//!
//! ## The subset (honest scope, SOUL §11)
//!
//! - **Elements:** `svg` (viewBox / width / height), `rect` (`rx`/`ry` rounding),
//!   `circle`, `ellipse`, `line`, `polyline`, `polygon`, `path`, `g` (nested, with
//!   attribute inheritance), `defs`, `linearGradient` / `radialGradient` + `stop`,
//!   and `text` (see below). Unknown elements and attributes are skipped, so
//!   real-world icon files degrade gracefully.
//! - **Path data:** `M L H V C S Q T A Z` and their relative forms. Curves and
//!   arcs are flattened to line segments at parse time (fixed, generous
//!   subdivision — deterministic, SOUL §7.3).
//! - **Transforms:** `transform="translate(…) scale(…) rotate(a[,cx,cy])
//!   matrix(…) skewX(…) skewY(…)"` on shapes and groups, composed through the
//!   group stack and baked into the flattened geometry. Stroke widths scale by
//!   the transform's isotropic factor (`√|det|` — anisotropic strokes are
//!   approximated).
//! - **Paint:** `fill` (default **black**), `stroke` (default none),
//!   `stroke-width`, `fill-rule` (`nonzero` default / `evenodd`), `opacity` /
//!   `fill-opacity` / `stroke-opacity`, colors as `#rgb` / `#rrggbb` /
//!   `rgb(r,g,b)` / a small named set / `none`, and **gradients** via
//!   `fill="url(#id)"` — `linearGradient` / `radialGradient` with `offset` /
//!   `stop-color` / `stop-opacity` stops, in `objectBoundingBox` (default) or
//!   `userSpaceOnUse` units. Presentation attributes inherit through `g`.
//! - **Strokes** apply to *every* shape (all geometry flattens to contours);
//!   caps and joins are round (the coverage test is distance-to-segment).
//! - **Text:** `<text x y font-size text-anchor fill>content</text>` — shaped
//!   through the app's pooled [`TextShaper`] (the embedded deterministic font,
//!   SOUL §7.3) when rasterized via [`rasterize_svg_with_text`]; skipped by the
//!   shaper-less [`rasterize_svg`]. `y` is the baseline, per SVG.
//! - Still out of scope: CSS (`style="…"`), `use`/symbols, clips, masks,
//!   filters, `gradientTransform`, and stroke dash arrays.
//!
//! Rasterization is deterministic (SOUL §7.3): fixed 4×4 supersampling, shapes
//! composited src-over in document order, no wall clock, no randomness.

use std::borrow::Cow;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use schnellui_a11y::Role;
use schnellui_scene::{
    Color, DirtyFlags, ImageCacheKey, Scene, Size, TexelRect, WidgetId, WidgetKind,
};
use schnellui_text::{GlyphAtlas, TextShaper};

use crate::{emit_media_paint, BuildCtx, View};

mod parser;
pub use parser::parse_svg;
mod raster;
pub(crate) use raster::SvgDone;
pub use raster::{rasterize_svg, rasterize_svg_with_text};
mod async_raster;
pub use async_raster::*;
#[cfg(test)]
mod tests;

/// Supersampling grid per pixel axis (4×4 = 16 coverage samples).
const SS: u32 = 4;

/// Stable library identity used by the parsed-SVG, CPU-raster, and scene-atlas
/// caches. Icon adapters should create one key per icon and visual variant.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SvgCacheKey {
    pub library: Cow<'static, str>,
    pub name: Cow<'static, str>,
    pub variant: Cow<'static, str>,
}

impl SvgCacheKey {
    pub fn new(
        library: impl Into<Cow<'static, str>>,
        name: impl Into<Cow<'static, str>>,
        variant: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            library: library.into(),
            name: name.into(),
            variant: variant.into(),
        }
    }

    fn image_key(&self, width: u32, height: u32, mask: bool) -> ImageCacheKey {
        ImageCacheKey::new(
            self.library.clone(),
            self.name.clone(),
            self.variant.clone(),
            width,
            height,
        )
        .with_format(u8::from(mask))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SvgRasterKey {
    source: SvgCacheKey,
    width: u32,
    height: u32,
    mask: bool,
}

/// Process-wide icon/SVG cache occupancy, useful for diagnostics and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SvgCacheStats {
    pub parsed_documents: usize,
    pub rasterized_images: usize,
}

fn parsed_svg_cache() -> &'static Mutex<HashMap<SvgCacheKey, Arc<SvgDoc>>> {
    static CACHE: OnceLock<Mutex<HashMap<SvgCacheKey, Arc<SvgDoc>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn raster_svg_cache() -> &'static Mutex<HashMap<SvgRasterKey, Arc<[u8]>>> {
    static CACHE: OnceLock<Mutex<HashMap<SvgRasterKey, Arc<[u8]>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns cache occupancy without forcing either cache to initialize.
pub fn svg_cache_stats() -> SvgCacheStats {
    let parsed_documents = parsed_svg_cache()
        .lock()
        .map(|cache| cache.len())
        .unwrap_or(0);
    let rasterized_images = raster_svg_cache()
        .lock()
        .map(|cache| cache.len())
        .unwrap_or(0);
    SvgCacheStats {
        parsed_documents,
        rasterized_images,
    }
}

fn parse_svg_cached(key: &SvgCacheKey, markup: &str) -> Result<Arc<SvgDoc>, String> {
    if let Some(doc) = parsed_svg_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(key).cloned())
    {
        return Ok(doc);
    }
    // Parse outside the mutex: a cache miss can be expensive and unrelated icon
    // builds should not serialize behind it. A benign race may parse twice, while
    // the map still retains only one canonical Arc.
    let parsed = Arc::new(parse_svg(markup)?);
    match parsed_svg_cache().lock() {
        Ok(mut cache) => Ok(cache
            .entry(key.clone())
            .or_insert_with(|| Arc::clone(&parsed))
            .clone()),
        Err(_) => Ok(parsed),
    }
}

fn cached_raster(key: &SvgRasterKey) -> Option<Arc<[u8]>> {
    raster_svg_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(key).cloned())
}

fn retain_raster(key: SvgRasterKey, pixels: Arc<[u8]>) {
    if let Ok(mut cache) = raster_svg_cache().lock() {
        cache.entry(key).or_insert(pixels);
    }
}

// ---------------------------------------------------------------------------
// 2D affine transform (row-major 2×3: [a c e; b d f], SVG's matrix order)
// ---------------------------------------------------------------------------

/// An SVG affine transform `matrix(a b c d e f)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform2 {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for Transform2 {
    fn default() -> Self {
        Transform2::IDENTITY
    }
}

impl Transform2 {
    pub const IDENTITY: Transform2 = Transform2 {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// `self ∘ rhs` — apply `rhs` first, then `self` (SVG composition order:
    /// `transform="A B"` means A(B(p))).
    pub fn then(&self, rhs: &Transform2) -> Transform2 {
        Transform2 {
            a: self.a * rhs.a + self.c * rhs.b,
            b: self.b * rhs.a + self.d * rhs.b,
            c: self.a * rhs.c + self.c * rhs.d,
            d: self.b * rhs.c + self.d * rhs.d,
            e: self.a * rhs.e + self.c * rhs.f + self.e,
            f: self.b * rhs.e + self.d * rhs.f + self.f,
        }
    }

    pub fn apply(&self, p: (f32, f32)) -> (f32, f32) {
        (
            self.a * p.0 + self.c * p.1 + self.e,
            self.b * p.0 + self.d * p.1 + self.f,
        )
    }

    /// The isotropic scale factor (`√|det|`) — how lengths scale on average.
    pub fn scale_factor(&self) -> f32 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
}

/// Parses `transform="translate(…) rotate(…) …"` into one composed transform.
fn parse_transform(s: &str) -> Transform2 {
    let mut out = Transform2::IDENTITY;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let args: Vec<f32> = rest[open + 1..open + close]
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
            .filter_map(|t| t.parse().ok())
            .collect();
        rest = &rest[open + close + 1..];
        let t = match (name, args.as_slice()) {
            ("translate", [x]) => Transform2 {
                e: *x,
                ..Transform2::IDENTITY
            },
            ("translate", [x, y, ..]) => Transform2 {
                e: *x,
                f: *y,
                ..Transform2::IDENTITY
            },
            ("scale", [s]) => Transform2 {
                a: *s,
                d: *s,
                ..Transform2::IDENTITY
            },
            ("scale", [x, y, ..]) => Transform2 {
                a: *x,
                d: *y,
                ..Transform2::IDENTITY
            },
            ("rotate", [deg]) => rotation(*deg),
            ("rotate", [deg, cx, cy, ..]) => {
                // rotate about (cx, cy): T(c) ∘ R ∘ T(−c)
                let t1 = Transform2 {
                    e: *cx,
                    f: *cy,
                    ..Transform2::IDENTITY
                };
                let t2 = Transform2 {
                    e: -*cx,
                    f: -*cy,
                    ..Transform2::IDENTITY
                };
                t1.then(&rotation(*deg)).then(&t2)
            }
            ("matrix", [a, b, c, d, e, f, ..]) => Transform2 {
                a: *a,
                b: *b,
                c: *c,
                d: *d,
                e: *e,
                f: *f,
            },
            ("skewX", [deg]) => Transform2 {
                c: deg.to_radians().tan(),
                ..Transform2::IDENTITY
            },
            ("skewY", [deg]) => Transform2 {
                b: deg.to_radians().tan(),
                ..Transform2::IDENTITY
            },
            _ => Transform2::IDENTITY,
        };
        out = out.then(&t);
    }
    out
}

fn rotation(deg: f32) -> Transform2 {
    let r = deg.to_radians();
    Transform2 {
        a: r.cos(),
        b: r.sin(),
        c: -r.sin(),
        d: r.cos(),
        ..Transform2::IDENTITY
    }
}

// ---------------------------------------------------------------------------
// the parsed document model
// ---------------------------------------------------------------------------

/// One flattened contour: a polyline in user space (already transform-baked).
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    pub pts: Vec<(f32, f32)>,
    /// `true` joins last→first for stroking; fills always treat contours closed.
    pub closed: bool,
}

/// The fill rule (SVG default: nonzero).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// The text anchor (horizontal alignment about `x`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAnchor {
    #[default]
    Start,
    Middle,
    End,
}

/// A paint: a solid color or a reference into [`SvgDoc::gradients`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Solid(Color),
    Gradient(usize),
}

/// A gradient definition (`linearGradient` / `radialGradient`).
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    /// `true` = `objectBoundingBox` units (the default): coordinates are
    /// fractions of the painted shape's bbox. `false` = `userSpaceOnUse`.
    pub object_units: bool,
    /// `(offset 0..1, color)` in document order, offsets clamped monotonic.
    pub stops: Vec<(f32, Color)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientKind {
    Linear { x1: f32, y1: f32, x2: f32, y2: f32 },
    Radial { cx: f32, cy: f32, r: f32 },
}

/// One parsed shape with its resolved presentation attributes. Geometry is
/// flattened to [`Contour`]s at parse time (curves/arcs subdivided, transforms
/// baked); a `text` element keeps its run for shaping at raster time.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgShape {
    pub kind: SvgShapeKind,
    pub fill: Option<Paint>,
    pub stroke: Option<Paint>,
    /// stroke width in user units, already scaled by the transform's `√|det|`.
    pub stroke_width: f32,
    pub fill_rule: FillRule,
    /// composed `opacity × fill-opacity`-style alpha (0..1), inherited via `g`.
    pub opacity: f32,
    /// the composed transform (kept for `userSpaceOnUse` gradient mapping; the
    /// contour points already have it baked in).
    pub transform: Transform2,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SvgShapeKind {
    /// all vector geometry, flattened (multi-contour ⇒ fill-rule holes work).
    Path { contours: Vec<Contour> },
    /// a text run; `x`/`y` are the transform-baked anchor (y = baseline), `size`
    /// the font size scaled by the transform's isotropic factor.
    Text {
        x: f32,
        y: f32,
        size: f32,
        anchor: TextAnchor,
        content: String,
    },
}

/// A parsed SVG-subset document: viewBox, gradients, and shapes in paint order.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgDoc {
    pub min_x: f32,
    pub min_y: f32,
    pub width: f32,
    pub height: f32,
    pub gradients: Vec<Gradient>,
    pub shapes: Vec<SvgShape>,
}

// ---------------------------------------------------------------------------
// the subset parser (streaming tag scan with a group stack — no XML dependency)
// ---------------------------------------------------------------------------

/// Inheritable presentation state carried by the group stack (SOUL §8.1).
#[derive(Clone)]
struct Inherited {
    /// `None` = not set here (inherit further); `Some(None)` = `none`.
    fill: Option<Option<PaintRef>>,
    stroke: Option<Option<PaintRef>>,
    stroke_width: Option<f32>,
    fill_rule: Option<FillRule>,
    font_size: Option<f32>,
    text_anchor: Option<TextAnchor>,
    /// multiplied down the stack.
    opacity: f32,
    transform: Transform2,
}

impl Inherited {
    fn root() -> Inherited {
        Inherited {
            fill: None,
            stroke: None,
            stroke_width: None,
            fill_rule: None,
            font_size: None,
            text_anchor: None,
            opacity: 1.0,
            transform: Transform2::IDENTITY,
        }
    }
}

/// A paint before gradient resolution (`url(#id)` needs the full defs table).
#[derive(Clone, Debug, PartialEq)]
enum PaintRef {
    Solid(Color),
    Url(String),
}
