use crate::render_wgpu::instances::Instances;
use crate::render_wgpu::renderer::{ensure_capacity, srgb_to_linear};
use crate::render_wgpu::shader::SHADER_SRC;
use crate::render_wgpu::types::*;
use schnellui::scene::{Color, Scene};
use schnellui::text::GlyphAtlas;

/// Uploads `scratch[lo..hi]` into the resident buffer, growing it first when the
/// vector outgrew it (then the whole vector is the span).
#[allow(clippy::too_many_arguments)]
fn upload_slice<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buf: &mut Option<wgpu::Buffer>,
    cap: &mut u32,
    scratch: &[T],
    lo: usize,
    hi: usize,
    label: &str,
    work: &mut GpuUploadWork,
) {
    if scratch.is_empty() {
        return;
    }
    let stride = std::mem::size_of::<T>();
    let grew = scratch.len() as u32 > *cap;
    ensure_capacity(device, buf, cap, scratch.len() as u32, stride as u64, label);
    let (lo, hi) = if grew {
        (0, scratch.len())
    } else {
        (lo, hi.min(scratch.len()))
    };
    if lo >= hi {
        return;
    }
    queue.write_buffer(
        buf.as_ref().expect("instance buffer is allocated"),
        (lo * stride) as u64,
        bytemuck::cast_slice(&scratch[lo..hi]),
    );
    work.instance_writes += 1;
    work.instances_written += hi - lo;
    work.bytes.instance_bytes += ((hi - lo) * stride) as u64;
}

/// The GPU-side glyph atlas: an R8 texture kept in sync with a CPU [`GlyphAtlas`]
/// via `write_texture` sub-rect uploads (SOUL §3.2).
pub(crate) struct AtlasGpu {
    pub(crate) texture: wgpu::Texture,
    pub(crate) bind_group: wgpu::BindGroup,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// the CPU atlas (identity, generation) this texture last matched.
    pub(crate) atlas_id: u64,
    pub(crate) generation: u64,
}

/// The GPU-side colour plane of the glyph atlas: an RGBA8-sRGB texture with the colour
/// glyphs (emoji) of a CPU [`GlyphAtlas`]. Colour glyphs are written when a text first
/// shows one, so the whole plane is uploaded again whenever it changed. A one texel
/// stand-in exists while the atlas has no colour glyph, so the glyph pipeline always has
/// something bound.
pub(crate) struct ColorAtlasGpu {
    pub(crate) texture: wgpu::Texture,
    pub(crate) bind_group: wgpu::BindGroup,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The CPU atlas and the colour generation this texture last matched.
    pub(crate) atlas_id: u64,
    pub(crate) generation: u64,
}

/// The GPU-side **image** atlas: an RGBA8-sRGB texture kept in sync with the scene's
/// CPU [`ImageAtlas`](schnellui::scene::ImageAtlas) (SOUL §3.2). Staleness is a
/// revision compare — the CPU atlas bumps its revision on every write, so the sync
/// needs only `&Scene` and an unchanged atlas costs nothing per frame.
pub(crate) struct ImageAtlasGpu {
    pub(crate) texture: wgpu::Texture,
    pub(crate) bind_group: wgpu::BindGroup,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// the CPU revision this texture was last uploaded at.
    pub(crate) revision: u64,
    /// identity of the CPU atlas that revision belongs to.
    pub(crate) atlas_id: u64,
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
    /// Layout of bind group 2 of the glyph pipeline: the colour plane texture.
    pub(crate) color_bgl: wgpu::BindGroupLayout,

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
    pub(crate) color_atlas: Option<ColorAtlasGpu>,
    pub(crate) image_atlas: Option<ImageAtlasGpu>,
    pub(crate) atlas_scratch: Vec<u8>,
    /// The CPU instance mirrors and the walk that fills them.
    pub(crate) inst: Instances,
    pub(crate) last_uniforms: Option<Uniforms>,
}

impl GpuCore {
    /// Builds the uniform buffer/bind group and the two render pipelines targeting
    /// `format`. The pipeline color-target format is the only thing that varies
    /// between the offscreen and windowed paths; the shaders, blend, and vertex
    /// layouts are identical (SOUL §7.2).
    pub(crate) fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> GpuCore {
        // --- uniforms (viewport + atlas size), bind group 0 ---
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("schnellui.uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("schnellui.uniform_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("schnellui.uniform_bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        // --- glyph atlas bind group layout (texture + sampler), bind group 1 ---
        let glyph_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("schnellui.glyph_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // --- colour glyph plane (texture only, read texel by texel), bind group 2 ---
        let color_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("schnellui.color_glyph_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("schnellui.atlas_sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // --- shaders + pipelines ---
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("schnellui.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        // The three pipelines differ only in layout, shader entry points and the
        // instance stride; the vertex attributes are always four vec4s.
        let attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
        let pipeline =
            |label: &str, layout: &wgpu::PipelineLayout, vs: &str, fs: &str, stride: usize| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(vs),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: stride as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &attrs,
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(fs),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };

        // pipeline 1: instanced solid-color quads (bind group 0 only).
        let quad_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("schnellui.quad_layout"),
            bind_group_layouts: &[Some(&uniform_bgl)],
            immediate_size: 0,
        });
        let quad_pipeline = pipeline(
            "schnellui.quad_pipeline",
            &quad_layout,
            "vs_quad",
            "fs_quad",
            std::mem::size_of::<QuadInstance>(),
        );

        // pipeline 2: glyph quads sampling the R8 atlas, or the colour plane for a
        // colour glyph (bind groups 0 + 1 + 2).
        let glyph_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("schnellui.glyph_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&glyph_bgl), Some(&color_bgl)],
            immediate_size: 0,
        });
        let image_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("schnellui.image_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&glyph_bgl)],
            immediate_size: 0,
        });
        let glyph_stride = std::mem::size_of::<GlyphInstance>();
        let glyph_pipeline = pipeline(
            "schnellui.glyph_pipeline",
            &glyph_layout,
            "vs_glyph",
            "fs_glyph",
            glyph_stride,
        );

        // pipeline 3: image quads sampling the RGBA image atlas (bind groups 0 + 1;
        // the texture+sampler layout is identical to the glyph one, so that bind group
        // layout is shared). Instances reuse the glyph 4×vec4 layout: dest rect, texel uv,
        // tint colour, clip (SOUL §3.2).
        let image_pipeline = pipeline(
            "schnellui.image_pipeline",
            &image_layout,
            "vs_image",
            "fs_image",
            glyph_stride,
        );

        GpuCore {
            device,
            queue,
            uniform_buf,
            uniform_bg,
            quad_pipeline,
            glyph_pipeline,
            image_pipeline,
            sampler,
            glyph_bgl,
            color_bgl,
            quad_buf: None,
            quad_cap: 0,
            glyph_buf: None,
            glyph_cap: 0,
            image_buf: None,
            image_cap: 0,
            atlas: None,
            atlas_shadow: None,
            color_atlas: None,
            image_atlas: None,
            atlas_scratch: Vec::new(),
            inst: Instances::default(),
            last_uniforms: None,
        }
    }

    /// Brings the resident instance buffers up to date with `scene`, ensures the GPU
    /// atlases exist for glyphs + images, and writes the per-frame uniforms: the
    /// shared steps 1-3 both targets run before their render pass. `viewport_w/h`
    /// are the **physical** target dimensions; `scale` is the logical->physical
    /// factor the vertex stage applies (SOUL §7.1). Returns
    /// `(quad_count, glyph_count, image_count)`.
    ///
    /// [`Instances::prepare`] patches the CPU mirrors; only the spans it reports as
    /// changed are written to the GPU.
    pub(crate) fn upload_scene(
        &mut self,
        scene: &Scene,
        atlas: &GlyphAtlas,
        viewport_w: u32,
        viewport_h: u32,
        scale: f32,
    ) -> (u32, u32, u32) {
        self.inst.prepare(scene);
        for b in BUFS {
            for i in 0..self.inst.changed_spans[b as usize].len() {
                let (lo, hi) = self.inst.changed_spans[b as usize][i];
                self.write_range(b, lo, hi);
            }
        }
        // The retained instances are current, but atlases are shared with other
        // renderers and may have gained texels since this one last looked.
        if !self.inst.glyph_scratch.is_empty() {
            self.sync_atlas(atlas);
        }
        if !self.inst.image_scratch.is_empty() {
            self.ensure_image_atlas(scene.images());
        }
        self.write_uniforms(scene, atlas, viewport_w, viewport_h, scale);
        self.inst.counts()
    }

    /// Forgets the retained instances so the next upload is a full gather.
    #[cfg(test)]
    pub(crate) fn invalidate_retained(&mut self) {
        self.inst.invalidate_retained();
    }

    /// Writes instances `[lo, hi)` of buffer `b` from the CPU mirror to the GPU,
    /// growing the buffer (and then writing all of it) when it is too small.
    fn write_range(&mut self, b: Buf, lo: usize, hi: usize) {
        match b {
            Buf::Quad => upload_slice(
                &self.device,
                &self.queue,
                &mut self.quad_buf,
                &mut self.quad_cap,
                &self.inst.quad_scratch,
                lo,
                hi,
                "schnellui.quad_buf",
                &mut self.inst.last_upload_work,
            ),
            Buf::Glyph => upload_slice(
                &self.device,
                &self.queue,
                &mut self.glyph_buf,
                &mut self.glyph_cap,
                &self.inst.glyph_scratch,
                lo,
                hi,
                "schnellui.glyph_buf",
                &mut self.inst.last_upload_work,
            ),
            Buf::Image => upload_slice(
                &self.device,
                &self.queue,
                &mut self.image_buf,
                &mut self.image_cap,
                &self.inst.image_scratch,
                lo,
                hi,
                "schnellui.image_buf",
                &mut self.inst.last_upload_work,
            ),
        }
    }

    /// Writes the per-frame uniforms when they changed. Instance buffers are
    /// retained separately, but viewport scale and atlas dimensions remain frame
    /// properties.
    fn write_uniforms(
        &mut self,
        scene: &Scene,
        atlas: &GlyphAtlas,
        viewport_w: u32,
        viewport_h: u32,
        scale: f32,
    ) {
        // The image atlas dims ride params.yz (the glyph atlas keeps its own
        // uniform slot).
        let atlas_w = atlas.width().max(1) as f32;
        let atlas_h = atlas.height().max(1) as f32;
        let img_w = scene.images().width().max(1) as f32;
        let img_h = scene.images().height().max(1) as f32;
        let uniforms = Uniforms {
            viewport: [viewport_w as f32, viewport_h as f32],
            atlas_size: [atlas_w, atlas_h],
            params: [scale, img_w, img_h, 0.0],
        };
        if self.last_uniforms == Some(uniforms) {
            return;
        }
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        self.last_uniforms = Some(uniforms);
        self.inst.last_upload_work.bytes.other_bytes += std::mem::size_of::<Uniforms>() as u64;
    }

    /// Encodes one render pass into `view` that clears to `clear` (straight sRGB,
    /// decoded to linear for the sRGB target) and records the draws: the pass setup
    /// both targets share (SOUL §7.2).
    pub(crate) fn encode_clear_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        view: &wgpu::TextureView,
        clear: Color,
        counts: (u32, u32, u32),
    ) {
        let clear = wgpu::Color {
            r: srgb_to_linear(clear.r),
            g: srgb_to_linear(clear.g),
            b: srgb_to_linear(clear.b),
            a: clear.a as f64 / 255.0,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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
        self.record_pass(&mut pass, counts.0, counts.1, counts.2);
    }

    /// Records the quad + glyph + image draws into an already-begun render pass —
    /// shared by both targets so the draw sequence is identical (SOUL §7.2).
    ///
    /// The base and every overlay root are separate compositing layers. Within
    /// each layer, quad → image → glyph preserves normal widget rendering; then
    /// the next overlay's surface covers every primitive type from layers below.
    /// This is essential for stacked dialogs: text from a lower modeless panel
    /// must not leak above a modal surface.
    pub(crate) fn record_pass(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        quad_count: u32,
        glyph_count: u32,
        image_count: u32,
    ) {
        for layer in self.inst.layers() {
            let quads = layer.quad_start.min(quad_count)..layer.quad_end.min(quad_count);
            let chrome = layer.chrome_start.min(quad_count)..layer.chrome_end.min(quad_count);
            let images = layer.image_start.min(image_count)..layer.image_end.min(image_count);
            let glyphs = layer.glyph_start.min(glyph_count)..layer.glyph_end.min(glyph_count);
            if !quads.is_empty() {
                pass.set_pipeline(&self.quad_pipeline);
                pass.set_bind_group(0, &self.uniform_bg, &[]);
                pass.set_vertex_buffer(0, self.quad_buf.as_ref().unwrap().slice(..));
                pass.draw(0..6, quads);
            }
            if !images.is_empty() {
                if let Some(img) = &self.image_atlas {
                    pass.set_pipeline(&self.image_pipeline);
                    pass.set_bind_group(0, &self.uniform_bg, &[]);
                    pass.set_bind_group(1, &img.bind_group, &[]);
                    pass.set_vertex_buffer(0, self.image_buf.as_ref().unwrap().slice(..));
                    pass.draw(0..6, images);
                }
            }
            if !glyphs.is_empty() {
                if let (Some(atlas_gpu), Some(color_gpu)) = (&self.atlas, &self.color_atlas) {
                    pass.set_pipeline(&self.glyph_pipeline);
                    pass.set_bind_group(0, &self.uniform_bg, &[]);
                    pass.set_bind_group(1, &atlas_gpu.bind_group, &[]);
                    pass.set_bind_group(2, &color_gpu.bind_group, &[]);
                    pass.set_vertex_buffer(0, self.glyph_buf.as_ref().unwrap().slice(..));
                    pass.draw(0..6, glyphs);
                }
            }
            if !chrome.is_empty() {
                pass.set_pipeline(&self.quad_pipeline);
                pass.set_bind_group(0, &self.uniform_bg, &[]);
                pass.set_vertex_buffer(0, self.quad_buf.as_ref().unwrap().slice(..));
                pass.draw(0..6, chrome);
            }
        }
    }

    /// Brings the GPU glyph atlas up to date with the CPU atlas (SOUL §3.2): a full
    /// upload when the texture is new, resized or belongs to another atlas, else only
    /// the rect written since this renderer last synced. Reads the atlas's write log
    /// instead of consuming a shared dirty marker, so several renderers (the headless
    /// one and a windowed one) each stay current. A steady-state no-op.
    pub(crate) fn sync_atlas(&mut self, atlas: &GlyphAtlas) {
        self.sync_color_atlas(atlas);
        let created = self.ensure_atlas(atlas);
        let Some(gpu) = self.atlas.as_ref() else {
            return;
        };
        if !created {
            let delta = if gpu.atlas_id == atlas.id() {
                atlas.delta_since(gpu.generation)
            } else {
                schnellui::text::AtlasDelta::Full
            };
            match delta {
                schnellui::text::AtlasDelta::Unchanged => {}
                schnellui::text::AtlasDelta::Rect(rect) => self.write_atlas_subrect(atlas, rect),
                schnellui::text::AtlasDelta::Full => self.write_atlas_subrect(
                    atlas,
                    schnellui::text::AtlasRect {
                        x: 0,
                        y: 0,
                        width: atlas.width(),
                        height: atlas.height(),
                    },
                ),
            }
        }
        if let Some(gpu) = self.atlas.as_mut() {
            gpu.atlas_id = atlas.id();
            gpu.generation = atlas.generation();
        }
    }

    /// Brings the colour plane texture up to date with `atlas`. Nothing happens while the
    /// texture matches the atlas and its colour generation, which is every frame that
    /// shows no new emoji. Otherwise the plane is uploaded whole: it changes only when a
    /// colour glyph is drawn for the first time.
    pub(crate) fn sync_color_atlas(&mut self, atlas: &GlyphAtlas) {
        let (w, h) = (atlas.color_width().max(1), atlas.color_height().max(1));
        if let Some(gpu) = &self.color_atlas {
            if gpu.atlas_id == atlas.id()
                && gpu.generation == atlas.color_generation()
                && gpu.width == w
                && gpu.height == h
            {
                return;
            }
        }
        let resident = self
            .color_atlas
            .as_ref()
            .is_some_and(|gpu| gpu.width == w && gpu.height == h);
        if !resident {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("schnellui.color_glyph_atlas"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("schnellui.color_glyph_atlas_bg"),
                layout: &self.color_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            self.color_atlas = Some(ColorAtlasGpu {
                texture,
                bind_group,
                width: w,
                height: h,
                atlas_id: atlas.id(),
                generation: atlas.color_generation(),
            });
        }
        let gpu = self.color_atlas.as_mut().unwrap();
        gpu.atlas_id = atlas.id();
        gpu.generation = atlas.color_generation();
        let pixels = atlas.color_pixels();
        if pixels.len() == (w as usize) * (h as usize) * 4 {
            write_texels(&self.queue, &gpu.texture, 0, 0, w, h, w * 4, pixels);
        } else {
            // No colour glyph yet: the stand-in texel is transparent.
            write_texels(&self.queue, &gpu.texture, 0, 0, 1, 1, 4, &[0, 0, 0, 0]);
        }
    }

    /// (Re)creates the GPU atlas texture + bind group when absent or resized, doing a
    /// full upload of the R8 coverage. Returns `true` if it (re)created. A no-op when
    /// the GPU atlas already matches the CPU atlas dimensions.
    pub(crate) fn ensure_atlas(&mut self, atlas: &GlyphAtlas) -> bool {
        let (aw, ah) = (atlas.width(), atlas.height());
        if aw == 0 || ah == 0 {
            return false;
        }
        if let Some(a) = &self.atlas {
            if a.width == aw && a.height == ah {
                return false;
            }
        }
        let (texture, bind_group) = self.create_atlas_texture(
            "schnellui.glyph_atlas",
            "schnellui.glyph_atlas_bg",
            aw,
            ah,
            wgpu::TextureFormat::R8Unorm,
        );
        self.inst.last_upload_work.bytes.other_bytes += aw as u64 * ah as u64;
        write_texels(&self.queue, &texture, 0, 0, aw, ah, aw, atlas.pixels());
        self.atlas = Some(AtlasGpu {
            texture,
            bind_group,
            width: aw,
            height: ah,
            atlas_id: atlas.id(),
            generation: atlas.generation(),
        });
        if let Some(shadow) = self.atlas_shadow.as_mut() {
            shadow.clear();
            shadow.extend_from_slice(atlas.pixels());
        }
        true
    }

    /// Creates a sampled atlas texture of `format` plus the bind group (texture and
    /// the shared nearest sampler) that exposes it to the glyph and image pipelines.
    fn create_atlas_texture(
        &self,
        label: &str,
        bg_label: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> (wgpu::Texture, wgpu::BindGroup) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
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
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(bg_label),
            layout: &self.glyph_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        (texture, bind_group)
    }

    /// Uploads a single dirty sub-rect of the CPU atlas into the GPU texture via
    /// `write_texture` (SOUL §3.2). Rows are gathered into `atlas_scratch` because the
    /// source is strided by the full atlas width.
    fn write_atlas_subrect(&mut self, atlas: &GlyphAtlas, rect: schnellui::text::AtlasRect) {
        if rect.is_empty() {
            return;
        }
        let rw = rect.width as usize;
        let rh = rect.height as usize;
        self.inst.last_upload_work.bytes.other_bytes += (rw * rh) as u64;
        write_rect(
            &self.queue,
            &self.atlas.as_ref().unwrap().texture,
            &mut self.atlas_scratch,
            atlas.pixels(),
            atlas.width(),
            1,
            (rect.x, rect.y, rect.width, rect.height),
        );
        let atlas_w = atlas.width() as usize;
        if let Some(shadow) = self
            .atlas_shadow
            .as_mut()
            .filter(|shadow| shadow.len() == atlas.pixels().len())
        {
            for row in 0..rh {
                let start = (rect.y as usize + row) * atlas_w + rect.x as usize;
                shadow[start..start + rw].copy_from_slice(&atlas.pixels()[start..start + rw]);
            }
        }
    }

    /// Syncs the GPU image atlas with the scene's CPU [`ImageAtlas`] (SOUL §3.2):
    /// (re)creates the RGBA8-sRGB texture when absent or resized, then uploads only
    /// the union of texels written since the prior revision. A revision/dimension
    /// match is the steady state — no upload, no allocation, nothing.
    fn ensure_image_atlas(&mut self, images: &schnellui::scene::ImageAtlas) {
        let (iw, ih) = (images.width(), images.height());
        if iw == 0 || ih == 0 {
            return;
        }
        // Steady state: same texture, same content.
        if let Some(a) = &self.image_atlas {
            if a.width == iw
                && a.height == ih
                && a.revision == images.revision()
                && a.atlas_id == images.id()
            {
                return;
            }
        }
        // Same dimensions but new content: refresh the resident texture in place.
        let needs_recreate = match &self.image_atlas {
            Some(a) => a.width != iw || a.height != ih,
            None => true,
        };
        if needs_recreate {
            // sRGB: sampled texels decode to linear in-shader, and the sRGB target
            // re-encodes — image bytes round-trip exactly (SOUL §7.2).
            let (texture, bind_group) = self.create_atlas_texture(
                "schnellui.image_atlas",
                "schnellui.image_atlas_bg",
                iw,
                ih,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            );
            self.image_atlas = Some(ImageAtlasGpu {
                texture,
                bind_group,
                width: iw,
                height: ih,
                revision: 0,
                atlas_id: images.id(),
            });
        }
        let gpu = self.image_atlas.as_ref().unwrap();
        // Per-renderer delta from the atlas's write log, not the shared dirty marker.
        let delta = if needs_recreate || gpu.atlas_id != images.id() {
            schnellui::scene::ImageDelta::Full
        } else {
            images.delta_since(gpu.revision)
        };
        if matches!(delta, schnellui::scene::ImageDelta::Full) {
            self.inst.last_upload_work.bytes.other_bytes +=
                iw as u64 * ih as u64 * BYTES_PER_PIXEL as u64;
            let gpu = self.image_atlas.as_ref().unwrap();
            write_texels(
                &self.queue,
                &gpu.texture,
                0,
                0,
                iw,
                ih,
                iw * BYTES_PER_PIXEL,
                images.pixels(),
            );
        } else if let schnellui::scene::ImageDelta::Rect(rect) = delta {
            self.write_image_atlas_subrect(images, rect);
        }
        let gpu = self.image_atlas.as_mut().unwrap();
        gpu.revision = images.revision();
        gpu.atlas_id = images.id();
    }

    /// Uploads one tightly packed RGBA sub-rectangle from the strided CPU atlas.
    fn write_image_atlas_subrect(
        &mut self,
        images: &schnellui::scene::ImageAtlas,
        rect: schnellui::scene::TexelRect,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.inst.last_upload_work.bytes.other_bytes +=
            (rect.width as usize * BYTES_PER_PIXEL as usize * rect.height as usize) as u64;
        write_rect(
            &self.queue,
            &self.image_atlas.as_ref().unwrap().texture,
            &mut self.atlas_scratch,
            images.pixels(),
            images.width(),
            BYTES_PER_PIXEL,
            (rect.x, rect.y, rect.width, rect.height),
        );
    }

    /// Reconciles a newly mounted app's CPU glyph atlas with the resident GPU
    /// texture. Fresh apps restart their atlas bookkeeping, so revision/dirty
    /// state cannot establish continuity. The renderer-owned shadow can: scan the
    /// fixed-size R8 buffers, retain the texture and bind group, and upload only
    /// the bounding rectangle whose texels actually changed.
    pub(crate) fn reconcile_remounted_glyph_atlas(&mut self, atlas: &mut GlyphAtlas) {
        let compatible = self.atlas.as_ref().is_some_and(|gpu| {
            gpu.width == atlas.width()
                && gpu.height == atlas.height()
                && self
                    .atlas_shadow
                    .as_ref()
                    .is_some_and(|shadow| shadow.len() == atlas.pixels().len())
        });
        if !compatible {
            self.atlas = None;
            if let Some(shadow) = self.atlas_shadow.as_mut() {
                shadow.clear();
            }
            return;
        }

        let changed = changed_r8_rect(
            self.atlas_shadow.as_deref().unwrap(),
            atlas.pixels(),
            atlas.width(),
            atlas.height(),
        );
        // The fresh atlas marks every glyph inserted during mount as dirty. The
        // shadow-derived rectangle supersedes that marker and also covers texels
        // that became zero because content disappeared.
        let _ = atlas.take_dirty();
        if let Some(rect) = changed {
            self.write_atlas_subrect(atlas, rect);
        }
        if let Some(gpu) = self.atlas.as_mut() {
            gpu.atlas_id = atlas.id();
            gpu.generation = atlas.generation();
        }
    }
}

/// Writes a tightly packed `w × h` block of texels at `(x, y)` of `texture`.
#[allow(clippy::too_many_arguments)]
fn write_texels(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    bytes_per_row: u32,
    data: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
}

/// Uploads the `(x, y, w, h)` texel rectangle of a CPU atlas (`src`, `src_width`
/// texels per row, `bpp` bytes per texel) into `texture`. The source is strided by the
/// full atlas width, so the rows are gathered into `scratch` first.
fn write_rect(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    scratch: &mut Vec<u8>,
    src: &[u8],
    src_width: u32,
    bpp: u32,
    (x, y, w, h): (u32, u32, u32, u32),
) {
    let row_bytes = w as usize * bpp as usize;
    scratch.clear();
    scratch.reserve(row_bytes * h as usize);
    for row in 0..h as usize {
        let start = ((y as usize + row) * src_width as usize + x as usize) * bpp as usize;
        scratch.extend_from_slice(&src[start..start + row_bytes]);
    }
    write_texels(queue, texture, x, y, w, h, w * bpp, scratch);
}

/// Returns the smallest texel rectangle containing every changed byte in two
/// equally-sized R8 images. Equal rows take one slice comparison and no scanning.
pub(crate) fn changed_r8_rect(
    previous: &[u8],
    replacement: &[u8],
    width: u32,
    height: u32,
) -> Option<schnellui::text::AtlasRect> {
    let width_usize = width as usize;
    let height_usize = height as usize;
    if width == 0
        || height == 0
        || previous.len() != replacement.len()
        || previous.len() != width_usize.saturating_mul(height_usize)
    {
        return None;
    }

    let mut min_x = width_usize;
    let mut min_y = height_usize;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    for y in 0..height_usize {
        let start = y * width_usize;
        let old = &previous[start..start + width_usize];
        let new = &replacement[start..start + width_usize];
        if old == new {
            continue;
        }
        let first = old.iter().zip(new).position(|(a, b)| a != b).unwrap();
        let last = old.iter().zip(new).rposition(|(a, b)| a != b).unwrap();
        min_x = min_x.min(first);
        min_y = min_y.min(y);
        max_x = max_x.max(last);
        max_y = max_y.max(y);
    }
    if min_y == height_usize {
        return None;
    }
    Some(schnellui::text::AtlasRect {
        x: min_x as u32,
        y: min_y as u32,
        width: (max_x - min_x + 1) as u32,
        height: (max_y - min_y + 1) as u32,
    })
}
