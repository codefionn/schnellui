use super::color::{luts, pack};
use super::raster::{self, Raster, Sources, Texels};
use super::simd::SimdLevel;
use crate::render_wgpu::instances::Instances;
use crate::render_wgpu::tile_cache::TileDamage;
use crate::render_wgpu::{encode_png, TileCacheStats};
use schnellui::scene::{Color, Scene};
use schnellui::text::GlyphAtlas;

/// The CPU renderer: rasterises a laid-out [`Scene`] into a retained RGBA8 sRGB
/// picture with SIMD kernels, no GPU adapter involved.
///
/// It consumes the same retained instance mirrors as the wgpu [`Renderer`]
/// (a paint change re-encodes only its node's slot) and the same per-tile
/// fingerprints (only tiles whose instances changed are cleared and redrawn), so a
/// steady re-render costs work proportional to what changed and allocates
/// nothing. The kernels transcribe the WGSL shaders: the output matches the GPU's
/// to within rounding, and is bit for bit the same on every machine and
/// [`SimdLevel`].
///
/// The picture is `width × height` **physical** pixels; logical geometry is
/// multiplied by the scale.
///
/// [`Renderer`]: crate::render_wgpu::Renderer
pub struct SoftRenderer {
    width: u32,
    height: u32,
    scale: f32,
    clear: Color,
    level: SimdLevel,
    inst: Instances,
    damage: TileDamage,
    /// RGBA8 pixels, bytes in memory order.
    pixels: Vec<u32>,
    /// Per-column texel scratch for glyph and image quads.
    cols: Vec<i32>,
}

impl SoftRenderer {
    /// A renderer for a `width × height` physical picture, using the widest
    /// [`SimdLevel`] this CPU supports (`SCHNELLUI_SIMD=portable` forces the
    /// portable lanes).
    pub fn new(width: u32, height: u32) -> SoftRenderer {
        SoftRenderer::with_simd_level(width, height, SimdLevel::detect())
    }

    /// Like [`SoftRenderer::new`] with an explicit lane implementation. A level
    /// the CPU lacks falls back to [`SimdLevel::Portable`].
    pub fn with_simd_level(width: u32, height: u32, level: SimdLevel) -> SoftRenderer {
        let (width, height) = (width.max(1), height.max(1));
        // Build the colour tables now rather than inside the first frame.
        let _ = luts();
        SoftRenderer {
            width,
            height,
            scale: 1.0,
            clear: Color::default(),
            level: level.supported(),
            inst: Instances::default(),
            damage: TileDamage::new(width, height),
            pixels: vec![0; width as usize * height as usize],
            cols: Vec::new(),
        }
    }

    /// The lane implementation in use.
    pub fn simd_level(&self) -> SimdLevel {
        self.level
    }

    /// The picture dimensions.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Sets the background colour every frame starts from.
    pub fn set_clear_color(&mut self, color: Color) {
        self.clear = color;
    }

    /// Sets the logical->physical scale (SOUL §7.1 `--scale`).
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
    }

    /// Resizes the picture to a new **physical** size (a grow event, may
    /// allocate). The next frame repaints every tile. No-op when unchanged.
    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return;
        }
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(width as usize * height as usize, 0);
        self.damage = TileDamage::new(width, height);
    }

    /// Tiles the last frame reused (hits) and repainted (rasterized).
    pub fn last_tile_cache_stats(&self) -> TileCacheStats {
        self.damage.stats
    }

    /// Brings the picture up to date with `scene` and returns it as tightly packed
    /// RGBA8 rows. Allocates nothing once the scene's instance slots and the
    /// scratch have grown to size.
    pub fn render(&mut self, scene: &Scene, atlas: &GlyphAtlas) -> &[u8] {
        self.inst.prepare(scene);
        let regions = self
            .damage
            .plan(&self.inst, scene, atlas, self.scale, self.clear);
        if !regions.is_empty() {
            let images = scene.images();
            let mut raster = Raster {
                pixels: &mut self.pixels,
                width: self.width as usize,
                scale: self.scale,
                src: Sources {
                    glyphs: Texels {
                        data: atlas.pixels(),
                        width: atlas.width(),
                        height: atlas.height(),
                    },
                    colors: Texels {
                        data: atlas.color_pixels(),
                        width: atlas.color_width(),
                        height: atlas.color_height(),
                    },
                    images: Texels {
                        data: images.pixels(),
                        width: images.width(),
                        height: images.height(),
                    },
                },
                cols: &mut self.cols,
                luts: luts(),
            };
            let c = self.clear;
            let clear = pack(c.r, c.g, c.b, c.a);
            for &region in regions {
                raster::draw(self.level, &mut raster, &self.inst, region, clear);
            }
        }
        self.frame()
    }

    /// The picture's pixel words (RGBA bytes in memory order).
    pub(crate) fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// The physical rectangles the last frame repainted.
    pub(crate) fn last_regions(&self) -> &[[u32; 4]] {
        self.damage.regions()
    }

    /// The picture as of the last [`SoftRenderer::render`]: tightly packed RGBA8.
    pub fn frame(&self) -> &[u8] {
        bytemuck::cast_slice(&self.pixels)
    }

    /// Renders one frame and returns a copy of its RGBA8 bytes.
    pub fn render_rgba8(&mut self, scene: &Scene, atlas: &GlyphAtlas) -> Vec<u8> {
        self.render(scene, atlas).to_vec()
    }

    /// Renders one frame and encodes it as PNG bytes.
    pub fn render_to_png(&mut self, scene: &Scene, atlas: &GlyphAtlas) -> Vec<u8> {
        let (w, h) = (self.width, self.height);
        encode_png(self.render(scene, atlas), w, h)
    }
}
