use crate::render_wgpu::gpu_core::GpuCore;
use schnellui::scene::{Color, Rect, Scene, SceneRenderKey};
use schnellui::text::GlyphAtlas;
use std::hash::{Hash, Hasher};

const TILE: u32 = 128;

/// Raster work performed by the last frame. Hits reuse resident target texels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileCacheStats {
    pub hits: usize,
    pub rasterized: usize,
}

#[derive(Clone, Copy, PartialEq)]
struct FrameKey {
    scene: SceneRenderKey,
    paint: u64,
    glyph: (u64, u64),
    image: (u64, u64),
    scale: f32,
    clear: Color,
}

/// Persistent picture target with independently fingerprinted physical tiles.
/// A scratch attachment receives only dirty tile draws; copies replace those
/// tiles in the picture. This also clears removed and moved primitives' old area.
pub(crate) struct TileCache {
    pub texture: wgpu::Texture,
    scratch: wgpu::Texture,
    scratch_view: wgpu::TextureView,
    width: u32,
    height: u32,
    fingerprints: Vec<u64>,
    valid: bool,
    key: Option<FrameKey>,
    pub stats: TileCacheStats,
}

impl TileCache {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        let make = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let texture = make("schnellui.cached_picture");
        let scratch = make("schnellui.tile_scratch");
        let scratch_view = scratch.create_view(&Default::default());
        Self {
            texture,
            scratch,
            scratch_view,
            width,
            height,
            fingerprints: vec![0; (width.div_ceil(TILE) * height.div_ceil(TILE)) as usize],
            valid: false,
            key: None,
            stats: TileCacheStats::default(),
        }
    }

    pub fn encode(
        &mut self,
        core: &GpuCore,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        atlas: &GlyphAtlas,
        scale: f32,
        clear: Color,
        counts: (u32, u32, u32),
    ) {
        let key = FrameKey {
            scene: scene.render_key(),
            paint: scene.paint_epoch(),
            glyph: (atlas.id(), atlas.generation()),
            image: (scene.images().id(), scene.images().revision()),
            scale,
            clear,
        };
        self.stats = TileCacheStats::default();
        if self.key == Some(key) && self.valid {
            self.stats.hits = self.fingerprints.len();
            return;
        }
        let columns = self.width.div_ceil(TILE);
        for i in 0..self.fingerprints.len() {
            let x = i as u32 % columns * TILE;
            let y = i as u32 / columns * TILE;
            let w = TILE.min(self.width - x);
            let h = TILE.min(self.height - y);
            let tile = Rect::new(
                x as f32 / scale,
                y as f32 / scale,
                w as f32 / scale,
                h as f32 / scale,
            );
            let fingerprint = fingerprint(core, tile, &key);
            if self.valid && self.fingerprints[i] == fingerprint {
                self.stats.hits += 1;
            } else {
                self.stats.rasterized += 1;
                // Zero is reserved to identify dirty copies in the second pass.
                self.fingerprints[i] = 0;
            }
        }
        if self.stats.rasterized == 0 {
            self.key = Some(key);
            return;
        }
        // A single scratch clear is enough. Only dirty tile regions are copied.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("schnellui.cached_tiles"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.scratch_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: crate::render_wgpu::renderer::srgb_to_linear(clear.r),
                        g: crate::render_wgpu::renderer::srgb_to_linear(clear.g),
                        b: crate::render_wgpu::renderer::srgb_to_linear(clear.b),
                        a: clear.a as f64 / 255.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for i in 0..self.fingerprints.len() {
            if self.fingerprints[i] != 0 {
                continue;
            }
            let x = i as u32 % columns * TILE;
            let y = i as u32 / columns * TILE;
            pass.set_scissor_rect(x, y, TILE.min(self.width - x), TILE.min(self.height - y));
            core.record_pass(&mut pass, counts.0, counts.1, counts.2);
        }
        drop(pass);
        for i in 0..self.fingerprints.len() {
            if self.fingerprints[i] != 0 {
                continue;
            }
            let x = i as u32 % columns * TILE;
            let y = i as u32 / columns * TILE;
            let w = TILE.min(self.width - x);
            let h = TILE.min(self.height - y);
            let origin = wgpu::Origin3d { x, y, z: 0 };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.scratch,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            self.fingerprints[i] = fingerprint(
                core,
                Rect::new(
                    x as f32 / scale,
                    y as f32 / scale,
                    w as f32 / scale,
                    h as f32 / scale,
                ),
                &key,
            );
        }
        self.valid = true;
        self.key = Some(key);
    }
}

fn rect(a: [f32; 4]) -> Rect {
    Rect::new(a[0], a[1], a[2], a[3])
}
fn fingerprint(core: &GpuCore, tile: Rect, key: &FrameKey) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    [key.clear.r, key.clear.g, key.clear.b, key.clear.a].hash(&mut hash);
    key.scale.to_bits().hash(&mut hash);
    for (index, q) in core.quad_scratch.iter().enumerate() {
        let mut bounds = rect(q.rect);
        if q.params[1] != 0.0 {
            let radius = (bounds.width.hypot(bounds.height)) / 2.0;
            bounds = Rect::new(
                bounds.x + bounds.width / 2.0 - radius,
                bounds.y + bounds.height / 2.0 - radius,
                radius * 2.0,
                radius * 2.0,
            );
        }
        if !bounds.intersect(&rect(q.clip)).intersect(&tile).is_empty() {
            layer(core, 0, index as u32).hash(&mut hash);
            0u8.hash(&mut hash);
            bytemuck::bytes_of(q).hash(&mut hash);
        }
    }
    for (family, instances, resource) in [
        (1u8, &core.glyph_scratch, key.glyph),
        (2u8, &core.image_scratch, key.image),
    ] {
        for (index, q) in instances.iter().enumerate() {
            if !rect(q.rect)
                .intersect(&rect(q.clip))
                .intersect(&tile)
                .is_empty()
            {
                layer(core, family, index as u32).hash(&mut hash);
                family.hash(&mut hash);
                resource.hash(&mut hash);
                bytemuck::bytes_of(q).hash(&mut hash);
            }
        }
    }
    hash.finish().max(1)
}

fn layer(core: &GpuCore, family: u8, index: u32) -> (usize, bool) {
    let base_end = match family {
        0 => core.base_quads,
        1 => core.base_glyphs,
        _ => core.base_images,
    };
    if index < base_end {
        return (0, family == 0 && index >= core.base_chrome_start);
    }
    for (i, layer) in core.overlay_layers.iter().enumerate() {
        let end = match family {
            0 => layer.chrome_end,
            1 => layer.glyph_end,
            _ => layer.image_end,
        };
        if index < end {
            return (i + 1, family == 0 && index >= layer.chrome_start);
        }
    }
    (usize::MAX, false)
}
