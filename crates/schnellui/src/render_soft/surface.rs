//! Presents the CPU picture in a native window through `softbuffer`: the
//! windowed counterpart of [`SurfaceRenderer`](crate::render_wgpu::SurfaceRenderer)
//! when there is no GPU to present with.

use std::num::NonZeroU32;
use std::sync::Arc;

use winit::window::Window;

use super::SoftRenderer;
use crate::render_wgpu::{RendererError, TileCacheStats};
use schnellui::scene::{Color, Scene};
use schnellui::text::GlyphAtlas;

/// A [`SoftRenderer`] drawing into a window's `softbuffer` surface.
pub struct SoftSurface {
    renderer: SoftRenderer,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    _context: softbuffer::Context<Arc<Window>>,
    /// Dirty rectangles of the last two frames, newest first: what a buffer
    /// presented one or two frames ago lacks.
    history: [Vec<[u32; 4]>; 2],
}

fn non_zero(v: u32) -> NonZeroU32 {
    NonZeroU32::new(v.max(1)).unwrap_or(NonZeroU32::MIN)
}

fn surface_error(e: impl std::fmt::Display) -> RendererError {
    RendererError::NoSurface(e.to_string())
}

impl SoftSurface {
    /// Connects to `window`'s display and creates a `width × height` physical
    /// buffer for it.
    pub fn new(window: Arc<Window>, width: u32, height: u32) -> Result<SoftSurface, RendererError> {
        let (width, height) = (width.max(1), height.max(1));
        let context = softbuffer::Context::new(window.clone()).map_err(surface_error)?;
        let mut surface = softbuffer::Surface::new(&context, window).map_err(surface_error)?;
        surface
            .resize(non_zero(width), non_zero(height))
            .map_err(surface_error)?;
        Ok(SoftSurface {
            renderer: SoftRenderer::new(width, height),
            surface,
            _context: context,
            history: Default::default(),
        })
    }

    /// The physical buffer size.
    pub fn size(&self) -> (u32, u32) {
        self.renderer.size()
    }

    /// Tiles the last frame reused and repainted.
    pub fn last_tile_cache_stats(&self) -> TileCacheStats {
        self.renderer.last_tile_cache_stats()
    }

    pub fn set_clear_color(&mut self, color: Color) {
        self.renderer.set_clear_color(color);
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.renderer.set_scale(scale);
    }

    /// Resizes the buffer to a new **physical** size. No-op when unchanged.
    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == self.size() {
            return;
        }
        self.renderer.resize(width, height);
        if let Err(e) = self.surface.resize(non_zero(width), non_zero(height)) {
            eprintln!("windowed: softbuffer resize failed: {e}");
        }
    }

    /// Draws the damaged tiles of `scene` into the picture, copies what the
    /// window's buffer lacks, and presents it.
    pub fn render(&mut self, scene: &Scene, atlas: &GlyphAtlas) {
        self.renderer.render(scene, atlas);
        let [newest, older] = &mut self.history;
        std::mem::swap(newest, older);
        newest.clear();
        newest.extend_from_slice(self.renderer.last_regions());
        let width = self.renderer.size().0 as usize;
        let pixels = self.renderer.pixels();
        let Ok(mut buffer) = self.surface.buffer_mut() else {
            return;
        };
        if buffer.len() != pixels.len() {
            return;
        }
        // softbuffer wants 0RGB words; the picture holds RGBA bytes.
        let copy = |dst: &mut [u32], src: &[u32]| {
            for (d, &s) in dst.iter_mut().zip(src) {
                *d = ((s & 0xFF) << 16) | (s & 0xFF00) | ((s >> 16) & 0xFF);
            }
        };
        match buffer.age() {
            // The buffer holds the picture as of one or two frames ago.
            age @ (1 | 2) => {
                for regions in &self.history[..age as usize] {
                    for &[x, y, w, h] in regions {
                        for row in y as usize..(y + h) as usize {
                            let span = row * width + x as usize..row * width + (x + w) as usize;
                            copy(&mut buffer[span.clone()], &pixels[span]);
                        }
                    }
                }
            }
            _ => copy(&mut buffer, pixels),
        }
        if let Err(e) = buffer.present() {
            eprintln!("windowed: softbuffer present failed: {e}");
        }
    }
}
