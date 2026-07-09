use crate::output::encode_png;
use crate::types::*;
use schnellui_scene::{Color, Scene};
use schnellui_text::GlyphAtlas;

pub struct Renderer {
    width: u32,
    height: u32,
    /// logical→physical scale (SOUL §7.1 `--scale`); `1.0` for standard shots. The
    /// target is `width × height` **physical** pixels; logical geometry is scaled up
    /// by this factor in the vertex stage.
    scale: f32,
    clear: Color,
    padded_bpr: u32,

    pub(crate) core: GpuCore,

    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

/// Whether to request a fallback (software) adapter — the `Software` backend or the
/// `SCHNELLUI_BACKEND=software` env override (SOUL §7.3).
fn wants_fallback(backend: Backend) -> bool {
    if let Ok(v) = std::env::var("SCHNELLUI_BACKEND") {
        if v.eq_ignore_ascii_case("software") {
            return true;
        }
    }
    matches!(backend, Backend::Software)
}

/// sRGB → linear for one 8-bit channel (used for the clear color; the shaders do the
/// same for instance colors so a straight-sRGB [`Color`] round-trips through the
/// `Rgba8UnormSrgb` target — SOUL §7.2).
pub(crate) fn srgb_to_linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Grows `buf` to hold at least `needed` instances of `stride` bytes, recreating it
/// (a grow event, §4) only when capacity is exceeded — never shrinks.
pub(crate) fn ensure_capacity(
    device: &wgpu::Device,
    buf: &mut Option<wgpu::Buffer>,
    cap: &mut u32,
    needed: u32,
    stride: u64,
    label: &str,
) {
    if needed > *cap {
        let new_cap = needed.next_power_of_two().max(64);
        *buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: new_cap as u64 * stride,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        *cap = new_cap;
    }
}

/// Requests an adapter + device, honoring the `Software`/`SCHNELLUI_BACKEND` override
/// (SOUL §7.3). `compatible_surface` is `None` for the headless path and `Some(..)`
/// for a windowed surface, so the chosen adapter can present to that surface.
fn request_adapter_device(
    instance: &wgpu::Instance,
    backend: Backend,
    compatible_surface: Option<&wgpu::Surface<'static>>,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), RendererError> {
    let fallback = wants_fallback(backend);
    let power_preference = if fallback {
        wgpu::PowerPreference::LowPower
    } else {
        wgpu::PowerPreference::HighPerformance
    };

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference,
        force_fallback_adapter: fallback,
        compatible_surface,
        ..Default::default()
    }))
    .map_err(|_| RendererError::NoAdapter)?;

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("schnellui.device"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .map_err(|e| RendererError::NoDevice(e.to_string()))?;

    Ok((adapter, device, queue))
}

impl Renderer {
    /// Builds a headless renderer for a fixed `width × height` viewport
    /// (SOUL §7.3 fixed viewport). Panics if no adapter is available; prefer
    /// [`Renderer::try_new`] where a graceful skip is wanted (SOUL §7.2 test note).
    pub fn new(width: u32, height: u32, backend: Backend) -> Renderer {
        Renderer::try_new(width, height, backend).expect("wgpu renderer init")
    }

    /// Fallible constructor (additive to the skeleton). Requests the adapter/device
    /// (async under `pollster`), creates the offscreen `RENDER_ATTACHMENT | COPY_SRC`
    /// target, the `MAP_READ` readback buffer, and the solid/glyph pipelines. Returns
    /// [`RendererError::NoAdapter`] so the headless harness can **skip gracefully**
    /// when there is no GPU (SOUL §7.2). Mount may allocate (§4).
    pub fn try_new(width: u32, height: u32, backend: Backend) -> Result<Renderer, RendererError> {
        let instance = wgpu::Instance::default();
        let (_adapter, device, queue) = request_adapter_device(&instance, backend, None)?;

        // --- offscreen render target (§7.2) ---
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("schnellui.target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        // --- readback buffer, padded to 256 (§7.2) ---
        let padded_bpr = padded_bytes_per_row(width, BYTES_PER_PIXEL);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("schnellui.readback"),
            size: padded_bpr as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let core = GpuCore::new(device, queue, TARGET_FORMAT);

        Ok(Renderer {
            width,
            height,
            scale: 1.0,
            clear: Color::default(),
            padded_bpr,
            core,
            target,
            target_view,
            readback,
        })
    }

    /// The viewport dimensions.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Sets the deterministic clear color (SOUL §7.3 fixed clear color).
    pub fn set_clear_color(&mut self, color: Color) {
        self.clear = color;
    }

    /// Sets the logical→physical scale (SOUL §7.1 `--scale`). The target must already
    /// be sized `width*scale × height*scale`; logical primitive coordinates are then
    /// multiplied by `scale` in the vertex stage so geometry fills the larger target.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
    }

    /// Resizes the offscreen target + padded readback buffer to a new **physical**
    /// size (a grow/resize event — may allocate, SOUL §4). Recreates the render-target
    /// texture, its view, and the `MAP_READ` readback buffer; the per-frame uniforms
    /// pick up the new viewport on the next [`Renderer::render_rgba8`]. No-op if the
    /// size is unchanged, so a steady re-render over a stable size stays free.
    ///
    /// Without this, a cached renderer stayed pinned to its first-call size, so a
    /// frame taken *after* an [`App::resize`](../schnellui/struct.App.html) rendered at
    /// the old extent — presenting texels the pass never wrote for the grown region
    /// (SOUL §8 resize path).
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        self.target = self.core.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("schnellui.target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.target_view = self
            .target
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.padded_bpr = padded_bytes_per_row(width, BYTES_PER_PIXEL);
        self.readback = self.core.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("schnellui.readback"),
            size: self.padded_bpr as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
    }

    /// Uploads the glyph atlas's dirty sub-rect via `write_texture`, if any
    /// (SOUL §3.2). No-op in the steady state where nothing changed.
    pub fn upload_atlas(&mut self, atlas: &mut GlyphAtlas) {
        self.core.upload_atlas(atlas);
    }

    /// Renders exactly one synchronous frame of the scene's primitives into the
    /// offscreen target and reads it back as tightly-packed RGBA8 rows
    /// (SOUL §7.2). Uses [`padded_bytes_per_row`]/[`unpad_rows`] for the 256-byte
    /// alignment. `device.poll(wait)` before the map callback fires.
    pub fn render_rgba8(&mut self, scene: &Scene, atlas: &GlyphAtlas) -> Vec<u8> {
        // Steps 1–3 (gather + instance upload + uniforms) are shared with the
        // windowed path (SOUL §7.2).
        let (quad_count, glyph_count, image_count) =
            self.core
                .upload_scene(scene, atlas, self.width, self.height, self.scale);

        // 4. Encode one render pass + the padded readback copy.
        let mut encoder =
            self.core
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("schnellui.encoder"),
                });
        {
            let clear = wgpu::Color {
                r: srgb_to_linear(self.clear.r),
                g: srgb_to_linear(self.clear.g),
                b: srgb_to_linear(self.clear.b),
                a: self.clear.a as f64 / 255.0,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("schnellui.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.core
                .record_pass(&mut pass, quad_count, glyph_count, image_count);
        }

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bpr),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        self.core.queue.submit(Some(encoder.finish()));

        // 5. Map + poll(wait) + unpad (SOUL §7.2).
        let rgba;
        {
            let slice = self.readback.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |res| {
                let _ = tx.send(res);
            });
            let _ = self.core.device.poll(wgpu::PollType::wait_indefinitely());
            rx.recv()
                .expect("map callback")
                .expect("readback map failed");
            let data = slice.get_mapped_range().expect("get_mapped_range");
            rgba = unpad_rows(&data, self.width, self.height, self.padded_bpr);
        }
        self.readback.unmap();
        rgba
    }

    /// Renders one frame and encodes it as PNG bytes (SOUL §7.2). Convenience over
    /// [`Renderer::render_rgba8`] + the `png` encoder.
    pub fn render_to_png(&mut self, scene: &Scene, atlas: &GlyphAtlas) -> Vec<u8> {
        let rgba = self.render_rgba8(scene, atlas);
        encode_png(&rgba, self.width, self.height)
    }
}

/// The **windowed** renderer (opt-in, non-headless — SOUL §8): the same
/// [`GpuCore`] drawing path presented to a live winit window surface instead of the
/// offscreen texture. Created by `schnellui::App::run_windowed`; never touched by the
/// headless screenshotter (§7). The surface is configured to the window's **preferred
/// sRGB format** (so the shaders' straight-sRGB colors round-trip exactly as they do
/// on the offscreen `Rgba8UnormSrgb` target) with `PresentMode::AutoVsync` — reactive
/// redraws, no busy loop (SOUL Directive #3).
pub struct SurfaceRenderer {
    pub(crate) core: GpuCore,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// physical surface dimensions (the render viewport).
    width: u32,
    height: u32,
    /// logical→physical scale (SOUL §7.1); logical geometry is scaled up in the
    /// vertex stage exactly as in the headless path.
    scale: f32,
    clear: Color,
}

impl SurfaceRenderer {
    /// Creates a surface renderer for `target` (a window handle — e.g. an
    /// `Arc<winit::window::Window>`) at an initial `width × height` **physical**
    /// pixel size. Picks the window's preferred sRGB surface format and configures it
    /// with `PresentMode::AutoVsync`. Returns a [`RendererError`] rather than
    /// panicking so the caller can exit cleanly if the compositor connection or an
    /// adapter is unavailable (e.g. under a sandbox).
    pub fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        backend: Backend,
    ) -> Result<SurfaceRenderer, RendererError> {
        let width = width.max(1);
        let height = height.max(1);
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(target)
            .map_err(|e| RendererError::NoSurface(e.to_string()))?;
        let (adapter, device, queue) = request_adapter_device(&instance, backend, Some(&surface))?;

        let caps = surface.get_capabilities(&adapter);
        // Prefer an sRGB format so the shaders' linear output re-encodes to the
        // original sRGB bytes, matching the offscreen `Rgba8UnormSrgb` path (§7.2).
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == wgpu::TextureFormat::Rgba8UnormSrgb)
            .or_else(|| caps.formats.iter().copied().find(|f| f.is_srgb()))
            .unwrap_or_else(|| caps.formats.first().copied().unwrap_or(TARGET_FORMAT));
        let alpha_mode = caps
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            // Historical/default behavior: sRGB formats present as sRGB (§7.2).
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let mut core = GpuCore::new(device, queue, format);
        // Only the native host swaps whole Apps while retaining a renderer.
        // Headless one-shot renderers keep their previous memory profile.
        core.atlas_shadow = Some(Vec::new());

        Ok(SurfaceRenderer {
            core,
            surface,
            config,
            width,
            height,
            scale: 1.0,
            clear: Color::default(),
        })
    }

    /// The current physical surface dimensions.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Sets the background clear color.
    pub fn set_clear_color(&mut self, color: Color) {
        self.clear = color;
    }

    /// Sets the logical→physical scale (SOUL §7.1). Logical geometry is scaled up by
    /// this factor in the vertex stage so it fills the physical surface.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
    }

    /// Drops the GPU-side glyph + image atlas caches so the next [`Self::render`]
    /// re-creates them with a **full upload** from the CPU atlases. A host that swaps
    /// in a whole new mounted tree (SOUL §8 — e.g. an in-window scenario switch) must
    /// call this: the fresh tree's atlases are new objects whose dimensions and
    /// revision counter can coincide with the cached textures', so the steady-state
    /// dirty-rect / revision compares alone cannot see the swap.
    pub fn invalidate_atlases(&mut self) {
        self.core.atlas = None;
        if let Some(shadow) = self.core.atlas_shadow.as_mut() {
            shadow.clear();
        }
        self.core.image_atlas = None;
    }

    /// Reconciles renderer resources after a whole-App structural remount.
    ///
    /// Glyph atlases have fixed dimensions and deterministic packing. Keeping a
    /// CPU mirror of the resident texture lets the renderer retain its texture and
    /// bind group, uploading only genuinely changed coverage. Image atlases can be
    /// resized and repacked by application content, so they remain conservatively
    /// invalidated until they gain the same renderer-owned continuity tracking.
    pub fn reconcile_remount_atlases(&mut self, glyphs: &mut GlyphAtlas) {
        self.core.reconcile_remounted_glyph_atlas(glyphs);
        self.core.image_atlas = None;
    }

    /// Reconfigures the swapchain to a new **physical** size (on a window resize).
    /// A zero dimension is clamped to 1 (a minimized window keeps a valid surface).
    /// No-op when the size is unchanged, so the render-time size reconcile (SOUL §8)
    /// can call it every frame without re-creating the swapchain in the steady state.
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.core.device, &self.config);
    }

    /// Renders one frame of the scene to the window surface (SOUL §8). Runs the same
    /// gather → upload → draw path as the headless renderer, then acquires the next
    /// swapchain image, clears, records the draws, submits, and presents. New glyphs
    /// rasterized since the last frame are pushed as an atlas sub-rect upload
    /// (SOUL §3.2). A transient `Outdated`/`Lost` surface is reconfigured and the
    /// frame skipped — the caller schedules another redraw.
    pub fn render(&mut self, scene: &Scene, atlas: &mut GlyphAtlas) {
        let (quad_count, glyph_count, image_count) =
            self.core
                .upload_scene(scene, atlas, self.width, self.height, self.scale);
        // Push any glyph atlas sub-rect grown since the last frame (windowed text
        // updates across frames, unlike the headless one-shot — SOUL §3.2).
        if glyph_count > 0 {
            self.core.upload_atlas(atlas);
        }

        // wgpu 30 returns a `CurrentSurfaceTexture` enum (not a `Result`). Present a
        // `Success`/`Suboptimal` frame; reconfigure on `Outdated`/`Lost` and retry
        // once; skip on `Timeout`/`Occluded`/`Validation` — the caller reschedules.
        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match self.surface.get_current_texture() {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Outdated | Cst::Lost => {
                self.surface.configure(&self.core.device, &self.config);
                match self.surface.get_current_texture() {
                    Cst::Success(f) | Cst::Suboptimal(f) => f,
                    _ => return,
                }
            }
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            self.core
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("schnellui.surface_encoder"),
                });
        {
            let clear = wgpu::Color {
                r: srgb_to_linear(self.clear.r),
                g: srgb_to_linear(self.clear.g),
                b: srgb_to_linear(self.clear.b),
                a: self.clear.a as f64 / 255.0,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("schnellui.surface_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.core
                .record_pass(&mut pass, quad_count, glyph_count, image_count);
        }

        self.core.queue.submit(Some(encoder.finish()));
        // wgpu 30: present is a `Queue` method taking the acquired surface texture.
        self.core.queue.present(frame);
    }
}

// The WGSL for both pipelines: solid rounded-rect quads and R8-atlas glyph quads.
// Colors are straight sRGB and decoded to linear here so the `Rgba8UnormSrgb` target
// re-encodes them back to the original sRGB bytes (SOUL §7.2).
