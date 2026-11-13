use crate::render_wgpu::gpu_core::GpuCore;
use rustc_hash::FxHasher;
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

/// An instance's inclusive `[c0, c1, r0, r1]` tile span; `c0 == NO_TILES` when it
/// touches no tile.
type Span = [u16; 4];

const NO_TILES: u16 = u16::MAX;
const NO_SPAN: Span = [NO_TILES; 4];

/// The `GpuCore` state the cached instance entries were brought up to.
#[derive(Clone, Copy, PartialEq)]
struct Synced {
    upload: u64,
    gathers: u64,
    scale: u32,
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
    /// This frame's fingerprints while they are compared with the last frame's.
    next: Vec<u64>,
    dirty: Vec<bool>,
    /// Dirty tiles merged into `[x, y, width, height]` rectangles.
    regions: Vec<[u32; 4]>,
    valid: bool,
    key: Option<FrameKey>,
    pub stats: TileCacheStats,
    /// Per instance of the quad, glyph and image mirrors, index for index: the
    /// hash of its bytes and layer tag (plus the family for glyphs and images;
    /// the atlas resource key is mixed in at fold time) and its tile span. A
    /// changed frame re-hashes only the instances `GpuCore` reports as changed.
    hashes: [Vec<u64>; 3],
    spans: [Vec<Span>; 3],
    /// Each family's layer boundaries (the values `layer()` reads) when the
    /// entries were last brought up to date.
    bounds: [Vec<u32>; 3],
    bounds_now: Vec<u32>,
    synced: Option<Synced>,
    /// Tiles whose fingerprint must be refolded: those under the old or new
    /// span of an entry that changed since the last fold.
    refold: Vec<bool>,
    /// Every tile must be refolded (re-hashed entries, a new seed or atlas key).
    refold_all: bool,
    /// Seed and per-family resource mix of the last fold.
    folded: Option<(u64, [u64; 3])>,
    /// Instances hashed by the last `encode`: the work counter tests read.
    pub(crate) instances_hashed: usize,
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
            next: Vec::new(),
            dirty: Vec::new(),
            regions: Vec::new(),
            valid: false,
            key: None,
            stats: TileCacheStats::default(),
            hashes: Default::default(),
            spans: Default::default(),
            bounds: Default::default(),
            bounds_now: Vec::new(),
            synced: None,
            refold: vec![false; (width.div_ceil(TILE) * height.div_ceil(TILE)) as usize],
            refold_all: true,
            folded: None,
            instances_hashed: 0,
        }
    }

    // Grouping these into a parameter struct would only move the list around.
    #[allow(clippy::too_many_arguments)]
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
        // Always, even on an unchanged frame: the entries must follow every
        // upload, or the next frame would see a gap and re-hash everything.
        self.sync_instances(core, scale);
        if self.key == Some(key) && self.valid {
            self.stats.hits = self.fingerprints.len();
            return;
        }
        self.fingerprint_tiles(&key);
        self.dirty.clear();
        for (old, new) in self.fingerprints.iter().zip(&self.next) {
            let dirty = !self.valid || old != new;
            self.dirty.push(dirty);
            if dirty {
                self.stats.rasterized += 1;
            } else {
                self.stats.hits += 1;
            }
        }
        std::mem::swap(&mut self.fingerprints, &mut self.next);
        if self.stats.rasterized == 0 {
            self.key = Some(key);
            return;
        }
        merge_dirty_tiles(&self.dirty, self.width, self.height, &mut self.regions);
        // A single scratch clear is enough. Only dirty regions are copied.
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
        for &[x, y, w, h] in &self.regions {
            pass.set_scissor_rect(x, y, w, h);
            core.record_pass(&mut pass, counts.0, counts.1, counts.2);
        }
        drop(pass);
        for &[x, y, w, h] in &self.regions {
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
        }
        self.valid = true;
        self.key = Some(key);
    }

    /// Forgets the picture and every cached instance hash, so the next frame
    /// hashes everything and repaints every tile.
    #[cfg(test)]
    pub(crate) fn invalidate(&mut self) {
        self.valid = false;
        self.key = None;
        self.synced = None;
    }

    fn grid(&self, scale: f32) -> TileGrid {
        TileGrid {
            columns: self.width.div_ceil(TILE),
            rows: self.height.div_ceil(TILE),
            surface: Rect::new(
                0.0,
                0.0,
                self.width as f32 / scale,
                self.height as f32 / scale,
            ),
            // Physical tiles per logical pixel.
            per_px: scale / TILE as f32,
        }
    }

    /// Brings the per-instance entries up to date with `core`'s mirrors.
    ///
    /// When this cache saw the previous upload, under the same full gather and
    /// scale, only the instances in `core.changed_spans` are re-hashed. Bytes
    /// that did not change keep their hash and span unless their layer tag
    /// moved, so every entry from the lowest layer boundary that changed to the
    /// end is re-hashed too. Anything else (a first frame, a full gather, a
    /// missed upload, a new scale) re-hashes every instance and refolds every
    /// tile. Tiles under the old or new span of a re-hashed or dropped entry are
    /// marked in `refold`; marks persist until a fold consumes them.
    fn sync_instances(&mut self, core: &GpuCore, scale: f32) {
        self.instances_hashed = 0;
        let now = Synced {
            upload: core.upload_serial,
            gathers: core.gather_serial,
            scale: scale.to_bits(),
        };
        let incremental = match self.synced {
            Some(seen) if seen == now => return,
            Some(seen) => {
                seen.gathers == now.gathers
                    && seen.scale == now.scale
                    && seen.upload + 1 == now.upload
            }
            None => false,
        };
        if !incremental {
            self.refold_all = true;
        }
        let grid = self.grid(scale);
        let columns = grid.columns as usize;
        for family in 0..3u8 {
            let f = family as usize;
            let len = match family {
                0 => core.quad_scratch.len(),
                1 => core.glyph_scratch.len(),
                _ => core.image_scratch.len(),
            };
            layer_bounds(core, family, &mut self.bounds_now);
            let old = &self.bounds[f];
            // Instances below every changed boundary keep their layer tag: each
            // comparison `index < boundary` gives the same answer before and after.
            let moved = if old.len() != self.bounds_now.len() {
                0
            } else {
                old.iter()
                    .zip(&self.bounds_now)
                    .filter(|(a, b)| a != b)
                    .map(|(&a, &b)| a.min(b) as usize)
                    .min()
                    .unwrap_or(usize::MAX)
            };
            std::mem::swap(&mut self.bounds[f], &mut self.bounds_now);
            let (hashes, spans) = (&mut self.hashes[f], &mut self.spans[f]);
            // Entries past the old length are new; the mirror only grows through
            // a splice, which reports its tail anyway.
            let tail = if incremental {
                moved.min(spans.len()).min(len)
            } else {
                0
            };
            let mark = !self.refold_all;
            if mark {
                for &span in spans.get(len..).unwrap_or_default() {
                    mark_span(&mut self.refold, columns, span);
                }
            }
            hashes.resize(len, 0);
            spans.resize(len, NO_SPAN);
            let mut rehash = |index: usize, refold: &mut [bool]| {
                if mark {
                    mark_span(refold, columns, spans[index]);
                }
                (hashes[index], spans[index]) = hash_instance(core, &grid, family, index);
                if mark {
                    mark_span(refold, columns, spans[index]);
                }
            };
            if incremental {
                for &(lo, hi) in &core.changed_spans[f] {
                    for index in lo..hi.min(tail) {
                        rehash(index, &mut self.refold);
                    }
                    self.instances_hashed += hi.min(tail).saturating_sub(lo);
                }
            }
            for index in tail..len {
                rehash(index, &mut self.refold);
            }
            self.instances_hashed += len - tail;
        }
        self.synced = Some(now);
    }

    /// Fills `next` with one fingerprint per tile by folding the cached instance
    /// hashes, in draw order, into the tiles each one touches. No instance bytes
    /// are read here. The atlas resource key is mixed into every glyph or image
    /// hash, so a new atlas id or generation changes every tile that samples it.
    ///
    /// Only tiles marked in `refold` start again from the seed; the others keep
    /// last frame's fingerprint, which is exact because no entry that covers
    /// them changed. A new seed or resource mix refolds every tile.
    fn fingerprint_tiles(&mut self, key: &FrameKey) {
        let mut seed = FxHasher::default();
        [key.clear.r, key.clear.g, key.clear.b, key.clear.a].hash(&mut seed);
        key.scale.to_bits().hash(&mut seed);
        let seed = seed.finish();
        let resource = |family: u8, value: (u64, u64)| {
            let mut hash = FxHasher::default();
            family.hash(&mut hash);
            value.hash(&mut hash);
            hash.finish()
        };
        let mix = [0, resource(1, key.glyph), resource(2, key.image)];
        // With every tile marked, the plain fold is cheaper than the masked one.
        let all =
            self.refold_all || self.folded != Some((seed, mix)) || self.refold.iter().all(|&r| r);
        let grid = self.grid(key.scale);
        self.next.clear();
        if all {
            self.next.resize(self.fingerprints.len(), seed);
            for (f, &mix) in mix.iter().enumerate() {
                for (&span, &hash) in self.spans[f].iter().zip(&self.hashes[f]) {
                    if span[0] != NO_TILES {
                        grid.fold(&mut self.next, widen(span), hash ^ mix);
                    }
                }
            }
        } else {
            self.next.extend_from_slice(&self.fingerprints);
            // The marked tiles' bounding box lets most entries be skipped with
            // four compares.
            let columns = grid.columns as usize;
            let mut bbox = [u32::MAX, 0, u32::MAX, 0];
            for (tile, _) in self.refold.iter().enumerate().filter(|(_, &r)| r) {
                self.next[tile] = seed;
                let (c, r) = ((tile % columns) as u32, (tile / columns) as u32);
                bbox = [
                    bbox[0].min(c),
                    bbox[1].max(c),
                    bbox[2].min(r),
                    bbox[3].max(r),
                ];
            }
            if bbox[0] != u32::MAX {
                for (f, &mix) in mix.iter().enumerate() {
                    for (&span, &hash) in self.spans[f].iter().zip(&self.hashes[f]) {
                        let [c0, c1, r0, r1] = widen(span);
                        if span[0] == NO_TILES
                            || c0 > bbox[1]
                            || c1 < bbox[0]
                            || r0 > bbox[3]
                            || r1 < bbox[2]
                        {
                            continue;
                        }
                        grid.fold_marked(
                            &mut self.next,
                            &self.refold,
                            [
                                c0.max(bbox[0]),
                                c1.min(bbox[1]),
                                r0.max(bbox[2]),
                                r1.min(bbox[3]),
                            ],
                            hash ^ mix,
                        );
                    }
                }
            }
        }
        self.refold.fill(false);
        self.refold_all = false;
        self.folded = Some((seed, mix));
    }
}

fn widen([c0, c1, r0, r1]: Span) -> [u32; 4] {
    [c0 as u32, c1 as u32, r0 as u32, r1 as u32]
}

/// Marks the tiles `span` covers.
fn mark_span(refold: &mut [bool], columns: usize, span: Span) {
    if span[0] == NO_TILES {
        return;
    }
    let [c0, c1, r0, r1] = widen(span);
    for row in r0 as usize..=r1 as usize {
        refold[row * columns + c0 as usize..=row * columns + c1 as usize].fill(true);
    }
}

/// Hashes instance `index` of `family` (0 quads, 1 glyphs, 2 images) with its
/// layer tag and finds the tiles its clipped bounds touch.
fn hash_instance(core: &GpuCore, grid: &TileGrid, family: u8, index: usize) -> (u64, Span) {
    let (bounds, clip, bytes) = match family {
        0 => {
            let q = &core.quad_scratch[index];
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
            (bounds, q.clip, bytemuck::bytes_of(q))
        }
        _ => {
            let q = if family == 1 {
                &core.glyph_scratch[index]
            } else {
                &core.image_scratch[index]
            };
            (rect(q.rect), q.clip, bytemuck::bytes_of(q))
        }
    };
    let Some([c0, c1, r0, r1]) = grid.span(bounds.intersect(&rect(clip))) else {
        return (0, NO_SPAN);
    };
    let mut hash = FxHasher::default();
    layer(core, family, index as u32).hash(&mut hash);
    if family != 0 {
        family.hash(&mut hash);
    }
    hash.write(bytes);
    (hash.finish(), [c0 as u16, c1 as u16, r0 as u16, r1 as u16])
}

/// The layer boundaries [`layer`] compares instance indices of `family` with.
fn layer_bounds(core: &GpuCore, family: u8, out: &mut Vec<u32>) {
    out.clear();
    match family {
        0 => {
            out.extend([core.base_chrome_start, core.base_quads]);
            for l in &core.overlay_layers {
                out.extend([l.chrome_start, l.chrome_end]);
            }
        }
        1 => {
            out.push(core.base_glyphs);
            out.extend(core.overlay_layers.iter().map(|l| l.glyph_end));
        }
        _ => {
            out.push(core.base_images);
            out.extend(core.overlay_layers.iter().map(|l| l.image_end));
        }
    }
}

/// Groups the dirty tiles into `[x, y, width, height]` rectangles: runs along a
/// row, joined to the run above when both span the same columns. A full repaint
/// is then one scissored draw of the scene instead of one per tile.
fn merge_dirty_tiles(dirty: &[bool], width: u32, height: u32, regions: &mut Vec<[u32; 4]>) {
    let columns = width.div_ceil(TILE);
    regions.clear();
    for row in 0..height.div_ceil(TILE) {
        let y = row * TILE;
        let h = TILE.min(height - y);
        let mut column = 0;
        while column < columns {
            if !dirty[(row * columns + column) as usize] {
                column += 1;
                continue;
            }
            let first = column;
            while column < columns && dirty[(row * columns + column) as usize] {
                column += 1;
            }
            let x = first * TILE;
            let w = (column * TILE).min(width) - x;
            let above = regions
                .iter_mut()
                .find(|r| r[0] == x && r[2] == w && r[1] + r[3] == y);
            match above {
                Some(region) => region[3] += h,
                None => regions.push([x, y, w, h]),
            }
        }
    }
}

struct TileGrid {
    columns: u32,
    rows: u32,
    surface: Rect,
    per_px: f32,
}

impl TileGrid {
    /// The inclusive tile columns and rows `bounds` (logical px) touches, or
    /// `None` when it lies outside the surface.
    fn span(&self, bounds: Rect) -> Option<[u32; 4]> {
        let visible = bounds.intersect(&self.surface);
        if visible.is_empty() {
            return None;
        }
        let first = |v: f32, count: u32| ((v * self.per_px).floor() as u32).min(count - 1);
        let last =
            |v: f32, count: u32| (((v * self.per_px).ceil() as u32).max(1) - 1).min(count - 1);
        let (c0, r0) = (first(visible.x, self.columns), first(visible.y, self.rows));
        Some([
            c0,
            last(visible.right(), self.columns).max(c0),
            r0,
            last(visible.bottom(), self.rows).max(r0),
        ])
    }

    fn fold(&self, tiles: &mut [u64], [c0, c1, r0, r1]: [u32; 4], hash: u64) {
        for row in r0..=r1 {
            let start = (row * self.columns) as usize;
            for tile in &mut tiles[start + c0 as usize..=start + c1 as usize] {
                *tile = fold_one(*tile, hash);
            }
        }
    }

    /// [`TileGrid::fold`] restricted to the tiles set in `marked`.
    fn fold_marked(
        &self,
        tiles: &mut [u64],
        marked: &[bool],
        [c0, c1, r0, r1]: [u32; 4],
        hash: u64,
    ) {
        for row in r0..=r1 {
            let start = (row * self.columns) as usize;
            let range = start + c0 as usize..=start + c1 as usize;
            for (tile, _) in tiles[range.clone()]
                .iter_mut()
                .zip(&marked[range])
                .filter(|(_, &m)| m)
            {
                *tile = fold_one(*tile, hash);
            }
        }
    }
}

/// Xor then an odd multiply is a bijection of the tile's state, so one changed
/// instance always changes the fingerprint, and the rotate keeps the fold
/// sensitive to draw order.
fn fold_one(tile: u64, hash: u64) -> u64 {
    (tile.rotate_left(5) ^ hash).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

fn rect(a: [f32; 4]) -> Rect {
    Rect::new(a[0], a[1], a[2], a[3])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(width: u32, height: u32, scale: f32) -> TileGrid {
        TileGrid {
            columns: width.div_ceil(TILE),
            rows: height.div_ceil(TILE),
            surface: Rect::new(0.0, 0.0, width as f32 / scale, height as f32 / scale),
            per_px: scale / TILE as f32,
        }
    }

    /// The tiles `bounds` overlaps, tested one tile at a time.
    fn overlapped(width: u32, height: u32, scale: f32, bounds: Rect) -> Vec<(u32, u32)> {
        let mut tiles = Vec::new();
        for row in 0..height.div_ceil(TILE) {
            for column in 0..width.div_ceil(TILE) {
                let (x, y) = (column * TILE, row * TILE);
                let tile = Rect::new(
                    x as f32 / scale,
                    y as f32 / scale,
                    TILE.min(width - x) as f32 / scale,
                    TILE.min(height - y) as f32 / scale,
                );
                if !bounds.intersect(&tile).is_empty() {
                    tiles.push((column, row));
                }
            }
        }
        tiles
    }

    #[test]
    fn span_names_exactly_the_overlapped_tiles() {
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move |range: f32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f32 / 10_000.0 * range
        };
        for (width, height, scale) in [(384, 256, 1.0), (1000, 700, 2.0), (130, 129, 1.5)] {
            let grid = grid(width, height, scale);
            let (w, h) = (width as f32 / scale, height as f32 / scale);
            let mut cases = vec![
                Rect::new(0.0, 0.0, 128.0 / scale, 128.0 / scale),
                Rect::new(128.0 / scale, 0.0, 128.0 / scale, 1.0),
                Rect::new(-1.0e9, -1.0e9, 2.0e9, 2.0e9),
                Rect::new(w + 1.0, 0.0, 50.0, 50.0),
                Rect::new(-50.0, -50.0, 50.0, 50.0),
                Rect::new(w - 0.25, h - 0.25, 10.0, 10.0),
                Rect::ZERO,
            ];
            for _ in 0..500 {
                let (x, y) = (next(w * 1.4) - w * 0.2, next(h * 1.4) - h * 0.2);
                cases.push(Rect::new(x, y, next(w * 0.5), next(h * 0.5)));
            }
            for bounds in cases {
                let mut spanned = Vec::new();
                if let Some([c0, c1, r0, r1]) = grid.span(bounds) {
                    for row in r0..=r1 {
                        for column in c0..=c1 {
                            spanned.push((column, row));
                        }
                    }
                }
                assert_eq!(
                    spanned,
                    overlapped(width, height, scale, bounds),
                    "{bounds:?} on {width}x{height} at {scale}"
                );
            }
        }
    }

    #[test]
    fn fold_depends_on_draw_order() {
        let grid = grid(256, 128, 1.0);
        let (mut ab, mut ba) = (vec![7u64; 2], vec![7u64; 2]);
        grid.fold(&mut ab, [0, 1, 0, 0], 11);
        grid.fold(&mut ab, [1, 1, 0, 0], 22);
        grid.fold(&mut ba, [1, 1, 0, 0], 22);
        grid.fold(&mut ba, [0, 1, 0, 0], 11);
        assert_eq!(ab[0], ba[0]);
        assert_ne!(ab[1], ba[1]);
    }

    #[test]
    fn dirty_tiles_merge_into_rectangles() {
        // 4 columns by 3 rows; the last column and row are partial tiles.
        let (width, height) = (3 * TILE + 40, 2 * TILE + 10);
        let merge = |dirty: [u8; 12]| {
            let mut regions = Vec::new();
            merge_dirty_tiles(&dirty.map(|d| d == 1), width, height, &mut regions);
            regions
        };
        assert_eq!(merge([1; 12]), [[0, 0, width, height]]);
        assert_eq!(merge([0; 12]), Vec::<[u32; 4]>::new());
        assert_eq!(
            merge([0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1]),
            [[3 * TILE, 0, 40, height]]
        );
        // The middle row's run is wider, so the three runs stay apart.
        assert_eq!(
            merge([1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0]),
            [
                [0, 0, 2 * TILE, TILE],
                [0, TILE, 3 * TILE, TILE],
                [0, 2 * TILE, 2 * TILE, 10]
            ]
        );
        // Two runs in a row, each continued below.
        assert_eq!(
            merge([1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0, 0]),
            [[0, 0, TILE, 2 * TILE], [2 * TILE, 0, TILE, 2 * TILE]]
        );
    }
}
