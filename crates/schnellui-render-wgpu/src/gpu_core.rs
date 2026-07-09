use crate::renderer::ensure_capacity;
use crate::shader::SHADER_SRC;
use crate::types::*;
use schnellui_scene::{Point, Primitive, Rect, Scene, WidgetKind};
use schnellui_text::GlyphAtlas;

/// Converts one already-positioned terminal display list into the three native GPU
/// instance families. This mirrors the regular tree walker but has no traversal or
/// allocation work beyond the retained scratch vectors supplied by `GpuCore`.
fn append_terminal_primitives(
    paint: &schnellui_scene::PaintData,
    offset: Point,
    clip: Rect,
    quads: &mut Vec<QuadInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    images: &mut Vec<GlyphInstance>,
) {
    let clip_arr = [clip.x, clip.y, clip.width, clip.height];
    for prim in &paint.primitives {
        match *prim {
            Primitive::SolidRect {
                rect,
                color,
                corner_radius,
            } => {
                let rect = Rect::new(
                    rect.x + offset.x,
                    rect.y + offset.y,
                    rect.width,
                    rect.height,
                );
                if !rect.intersect(&clip).is_empty() {
                    quads.push(QuadInstance::solid_clipped(
                        rect,
                        color,
                        corner_radius,
                        clip_arr,
                    ));
                }
            }
            Primitive::GlyphQuad {
                rect,
                atlas_uv,
                color,
            } => {
                let rect = Rect::new(
                    rect.x + offset.x,
                    rect.y + offset.y,
                    rect.width,
                    rect.height,
                );
                if !rect.intersect(&clip).is_empty() {
                    glyphs.push(GlyphInstance::glyph_clipped(
                        rect, atlas_uv, color, clip_arr,
                    ));
                }
            }
            Primitive::Line {
                from,
                to,
                width,
                color,
            } => {
                let from = Point {
                    x: from.x + offset.x,
                    y: from.y + offset.y,
                };
                let to = Point {
                    x: to.x + offset.x,
                    y: to.y + offset.y,
                };
                let half_width = width * 0.5;
                let bounds = Rect::new(
                    from.x.min(to.x) - half_width,
                    from.y.min(to.y) - half_width,
                    (from.x - to.x).abs() + width,
                    (from.y - to.y).abs() + width,
                );
                if !bounds.intersect(&clip).is_empty() {
                    quads.push(QuadInstance::line(from, to, width, color, clip_arr));
                }
            }
            Primitive::ImageQuad {
                rect,
                atlas_uv,
                tint,
            } => {
                let rect = Rect::new(
                    rect.x + offset.x,
                    rect.y + offset.y,
                    rect.width,
                    rect.height,
                );
                if !rect.intersect(&clip).is_empty() {
                    images.push(GlyphInstance::glyph_clipped(rect, atlas_uv, tint, clip_arr));
                }
            }
        }
    }
}

fn shift_fragment_range(
    range: &mut std::ops::Range<u32>,
    ordinal: u32,
    replaced_ordinal: u32,
    old: &std::ops::Range<u32>,
    new_len: u32,
    delta: i64,
) {
    if ordinal == replaced_ordinal {
        range.end = range.start + new_len;
    } else if (!old.is_empty() && range.start >= old.end)
        || (old.is_empty() && ordinal > replaced_ordinal)
    {
        range.start = (range.start as i64 + delta) as u32;
        range.end = (range.end as i64 + delta) as u32;
    } else {
        debug_assert!(
            range.end <= old.start,
            "terminal GPU fragments never overlap"
        );
    }
}

fn shift_boundary(boundary: &mut u32, old_end: u32, delta: i64) {
    if *boundary >= old_end {
        *boundary = (*boundary as i64 + delta) as u32;
    }
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

        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let color_target = wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        };

        // pipeline 1: instanced solid-color quads (bind group 0 only).
        let quad_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("schnellui.quad_layout"),
            bind_group_layouts: &[Some(&uniform_bgl)],
            immediate_size: 0,
        });
        let quad_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("schnellui.quad_pipeline"),
            layout: Some(&quad_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_quad"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<QuadInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &quad_attrs,
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
                entry_point: Some("fs_quad"),
                compilation_options: Default::default(),
                targets: &[Some(color_target.clone())],
            }),
            multiview_mask: None,
            cache: None,
        });

        // pipeline 2: glyph quads sampling the R8 atlas (bind groups 0 + 1).
        let glyph_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("schnellui.glyph_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&glyph_bgl)],
            immediate_size: 0,
        });
        let glyph_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("schnellui.glyph_pipeline"),
            layout: Some(&glyph_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_glyph"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &glyph_attrs,
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
                entry_point: Some("fs_glyph"),
                compilation_options: Default::default(),
                targets: &[Some(color_target.clone())],
            }),
            multiview_mask: None,
            cache: None,
        });

        // pipeline 3: image quads sampling the RGBA image atlas (bind groups 0 + 1;
        // the texture+sampler layout is identical to the glyph one, so `glyph_bgl`
        // is shared). Instances reuse the glyph 4×vec4 layout: dest rect, texel uv,
        // tint colour, clip (SOUL §3.2).
        let image_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
        let image_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("schnellui.image_pipeline"),
            layout: Some(&glyph_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_image"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &image_attrs,
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
                entry_point: Some("fs_image"),
                compilation_options: Default::default(),
                targets: &[Some(color_target)],
            }),
            multiview_mask: None,
            cache: None,
        });

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
            quad_buf: None,
            quad_cap: 0,
            glyph_buf: None,
            glyph_cap: 0,
            image_buf: None,
            image_cap: 0,
            atlas: None,
            atlas_shadow: None,
            image_atlas: None,
            quad_scratch: Vec::new(),
            chrome_scratch: Vec::new(),
            glyph_scratch: Vec::new(),
            image_scratch: Vec::new(),
            walk_stack: Vec::new(),
            overlay_roots: Vec::new(),
            base_quads: 0,
            base_chrome_start: 0,
            base_glyphs: 0,
            base_images: 0,
            overlay_layers: Vec::new(),
            atlas_scratch: Vec::new(),
            terminal_fragments: std::collections::HashMap::new(),
            retained_scene_key: None,
            terminal_quad_scratch: Vec::new(),
            terminal_glyph_scratch: Vec::new(),
            terminal_image_scratch: Vec::new(),
            next_terminal_fragment_ordinal: 0,
            last_upload_work: GpuUploadWork::default(),
        }
    }

    /// Gathers the scene primitives, uploads the instance buffers, ensures the GPU
    /// atlases exist for glyphs + images, and writes the per-frame uniforms — the
    /// shared steps 1–3 both targets run before their render pass. `viewport_w/h`
    /// are the **physical** target dimensions; `scale` is the logical→physical
    /// factor the vertex stage applies (SOUL §7.1). Returns
    /// `(quad_count, glyph_count, image_count)`.
    pub(crate) fn upload_scene(
        &mut self,
        scene: &Scene,
        atlas: &GlyphAtlas,
        viewport_w: u32,
        viewport_h: u32,
        scale: f32,
    ) -> (u32, u32, u32) {
        self.last_upload_work = GpuUploadWork::default();
        if self.try_upload_terminal_deltas(scene, atlas) {
            self.write_uniforms(scene, atlas, viewport_w, viewport_h, scale);
            return (
                self.quad_scratch.len() as u32,
                self.glyph_scratch.len() as u32,
                self.image_scratch.len() as u32,
            );
        }

        // 1. Gather primitives from the retained tree into the scratch buffers.
        self.gather(scene);
        self.last_upload_work.full_gathers = 1;
        let quad_count = self.quad_scratch.len() as u32;
        let glyph_count = self.glyph_scratch.len() as u32;
        let image_count = self.image_scratch.len() as u32;

        // 2. Upload instance data into the resident grow-only buffers (§3.2).
        if quad_count > 0 {
            ensure_capacity(
                &self.device,
                &mut self.quad_buf,
                &mut self.quad_cap,
                quad_count,
                std::mem::size_of::<QuadInstance>() as u64,
                "schnellui.quad_buf",
            );
            self.queue.write_buffer(
                self.quad_buf.as_ref().unwrap(),
                0,
                bytemuck::cast_slice(&self.quad_scratch),
            );
            self.last_upload_work.instance_writes += 1;
            self.last_upload_work.instances_written += quad_count as usize;
        }
        if glyph_count > 0 {
            ensure_capacity(
                &self.device,
                &mut self.glyph_buf,
                &mut self.glyph_cap,
                glyph_count,
                std::mem::size_of::<GlyphInstance>() as u64,
                "schnellui.glyph_buf",
            );
            self.queue.write_buffer(
                self.glyph_buf.as_ref().unwrap(),
                0,
                bytemuck::cast_slice(&self.glyph_scratch),
            );
            self.last_upload_work.instance_writes += 1;
            self.last_upload_work.instances_written += glyph_count as usize;
            // Make sure the atlas texture exists for the glyph bind group.
            self.ensure_atlas(atlas);
        }
        if image_count > 0 {
            ensure_capacity(
                &self.device,
                &mut self.image_buf,
                &mut self.image_cap,
                image_count,
                std::mem::size_of::<GlyphInstance>() as u64,
                "schnellui.image_buf",
            );
            self.queue.write_buffer(
                self.image_buf.as_ref().unwrap(),
                0,
                bytemuck::cast_slice(&self.image_scratch),
            );
            self.last_upload_work.instance_writes += 1;
            self.last_upload_work.instances_written += image_count as usize;
            // Sync the GPU image atlas with the scene's (revision compare, §3.2).
            self.ensure_image_atlas(scene.images());
        }

        self.retained_scene_key = Some(scene.render_key());
        self.write_uniforms(scene, atlas, viewport_w, viewport_h, scale);

        (quad_count, glyph_count, image_count)
    }

    /// Writes the per-frame uniforms. Instance buffers are retained separately, but
    /// viewport scale and atlas dimensions remain frame properties.
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
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
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
        let base = InstanceLayer {
            quad_start: 0,
            quad_end: self.base_chrome_start,
            chrome_start: self.base_chrome_start,
            chrome_end: self.base_quads,
            glyph_start: 0,
            glyph_end: self.base_glyphs,
            image_start: 0,
            image_end: self.base_images,
        };
        for layer in std::iter::once(base).chain(self.overlay_layers.iter().copied()) {
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
                if let Some(atlas_gpu) = &self.atlas {
                    pass.set_pipeline(&self.glyph_pipeline);
                    pass.set_bind_group(0, &self.uniform_bg, &[]);
                    pass.set_bind_group(1, &atlas_gpu.bind_group, &[]);
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

    /// Uploads the glyph atlas's dirty sub-rect via `write_texture`, if any
    /// (SOUL §3.2). No-op in the steady state where nothing changed. Used by the
    /// windowed path, where new glyphs are rasterized *across* frames (the headless
    /// one-shot path rasterizes everything before its single render, so a first-frame
    /// full upload covers it).
    pub(crate) fn upload_atlas(&mut self, atlas: &mut GlyphAtlas) {
        // (Re)create the GPU texture on first sight or a resize (full upload).
        let created = self.ensure_atlas(atlas);
        if created {
            // The full upload already covered any pending dirty region.
            let _ = atlas.take_dirty();
            return;
        }
        if let Some(rect) = atlas.take_dirty() {
            self.write_atlas_subrect(atlas, rect);
        }
    }

    /// Applies paint-only updates when every dirty node is a base-layer terminal.
    /// Variable-length glyph updates splice the retained CPU ranges and rewrite only
    /// the affected suffix of each GPU buffer. Overlays and terminal images retain
    /// the conservative full-gather fallback because their draw ordering is less
    /// forgiving than ordinary prompt text.
    fn try_upload_terminal_deltas(&mut self, scene: &Scene, atlas: &GlyphAtlas) -> bool {
        if self.retained_scene_key != Some(scene.render_key()) || !scene.layout_dirty().is_empty() {
            return false;
        }
        let dirty = scene.paint_dirty();
        if dirty.is_empty() {
            return self.retained_scene_key.is_some();
        }
        if dirty.iter().any(|id| {
            scene.node(*id).map(|node| node.kind) != Some(WidgetKind::TerminalGrid)
                || self.terminal_fragments.get(id).is_none_or(|fragment| {
                    fragment.quads.end > self.base_chrome_start
                        || fragment.glyphs.end > self.base_glyphs
                        || fragment.images.end > self.base_images
                        || !fragment.images.is_empty()
                })
        }) {
            return false;
        }

        for &id in dirty {
            let fragment = self.terminal_fragments[&id].clone();
            let Some(paint) = scene.paint(id) else {
                return false;
            };
            let mut quads = std::mem::take(&mut self.terminal_quad_scratch);
            let mut glyphs = std::mem::take(&mut self.terminal_glyph_scratch);
            let mut images = std::mem::take(&mut self.terminal_image_scratch);
            quads.clear();
            glyphs.clear();
            images.clear();
            append_terminal_primitives(
                paint,
                fragment.offset,
                fragment.clip,
                &mut quads,
                &mut glyphs,
                &mut images,
            );
            if !images.is_empty() {
                self.terminal_quad_scratch = quads;
                self.terminal_glyph_scratch = glyphs;
                self.terminal_image_scratch = images;
                return false;
            }
            self.replace_terminal_fragment(id, fragment, &quads, &glyphs);
            self.terminal_quad_scratch = quads;
            self.terminal_glyph_scratch = glyphs;
            self.terminal_image_scratch = images;
            self.last_upload_work.terminal_fragments += 1;
        }

        // Instance counts may have grown, but the shared glyph atlas can also have
        // changed underneath a terminal update.
        if !self.glyph_scratch.is_empty() {
            self.ensure_atlas(atlas);
        }
        true
    }

    fn replace_terminal_fragment(
        &mut self,
        id: schnellui_scene::WidgetId,
        fragment: TerminalGpuFragment,
        quads: &[QuadInstance],
        glyphs: &[GlyphInstance],
    ) {
        let quads_changed =
            self.quad_scratch[fragment.quads.start as usize..fragment.quads.end as usize] != *quads;
        if quads_changed {
            let count_changed = fragment.quads.len() != quads.len();
            self.quad_scratch.splice(
                fragment.quads.start as usize..fragment.quads.end as usize,
                quads.iter().copied(),
            );
            self.shift_terminal_quad_ranges(id, fragment.quads.clone(), quads.len() as u32);
            self.upload_quad_delta(fragment.quads.start, fragment.quads.end, count_changed);
        }

        let glyphs_changed = self.glyph_scratch
            [fragment.glyphs.start as usize..fragment.glyphs.end as usize]
            != *glyphs;
        if glyphs_changed {
            let count_changed = fragment.glyphs.len() != glyphs.len();
            self.glyph_scratch.splice(
                fragment.glyphs.start as usize..fragment.glyphs.end as usize,
                glyphs.iter().copied(),
            );
            self.shift_terminal_glyph_ranges(id, fragment.glyphs.clone(), glyphs.len() as u32);
            self.upload_glyph_delta(fragment.glyphs.start, fragment.glyphs.end, count_changed);
        }
    }

    fn upload_quad_delta(&mut self, start: u32, end: u32, count_changed: bool) {
        let count = self.quad_scratch.len() as u32;
        if count == 0 {
            return;
        }
        let grew = count > self.quad_cap;
        ensure_capacity(
            &self.device,
            &mut self.quad_buf,
            &mut self.quad_cap,
            count,
            std::mem::size_of::<QuadInstance>() as u64,
            "schnellui.quad_buf",
        );
        let start = if grew { 0 } else { start } as usize;
        let end = if grew || count_changed {
            self.quad_scratch.len()
        } else {
            end as usize
        };
        let upload = &self.quad_scratch[start..end];
        if upload.is_empty() {
            return;
        }
        self.queue.write_buffer(
            self.quad_buf.as_ref().expect("quad buffer is allocated"),
            start as u64 * std::mem::size_of::<QuadInstance>() as u64,
            bytemuck::cast_slice(upload),
        );
        self.last_upload_work.instance_writes += 1;
        self.last_upload_work.instances_written += upload.len();
        debug_assert!(count_changed || upload.len() <= self.quad_scratch.len());
    }

    fn upload_glyph_delta(&mut self, start: u32, end: u32, count_changed: bool) {
        let count = self.glyph_scratch.len() as u32;
        if count == 0 {
            return;
        }
        let grew = count > self.glyph_cap;
        ensure_capacity(
            &self.device,
            &mut self.glyph_buf,
            &mut self.glyph_cap,
            count,
            std::mem::size_of::<GlyphInstance>() as u64,
            "schnellui.glyph_buf",
        );
        let start = if grew { 0 } else { start } as usize;
        let end = if grew || count_changed {
            self.glyph_scratch.len()
        } else {
            end as usize
        };
        let upload = &self.glyph_scratch[start..end];
        if upload.is_empty() {
            return;
        }
        self.queue.write_buffer(
            self.glyph_buf.as_ref().expect("glyph buffer is allocated"),
            start as u64 * std::mem::size_of::<GlyphInstance>() as u64,
            bytemuck::cast_slice(upload),
        );
        self.last_upload_work.instance_writes += 1;
        self.last_upload_work.instances_written += upload.len();
        debug_assert!(count_changed || upload.len() <= self.glyph_scratch.len());
    }

    fn shift_terminal_quad_ranges(
        &mut self,
        id: schnellui_scene::WidgetId,
        old: std::ops::Range<u32>,
        new_len: u32,
    ) {
        let delta = new_len as i64 - old.len() as i64;
        if delta == 0 {
            return;
        }
        let replaced_ordinal = self.terminal_fragments[&id].ordinal;
        for fragment in self.terminal_fragments.values_mut() {
            shift_fragment_range(
                &mut fragment.quads,
                fragment.ordinal,
                replaced_ordinal,
                &old,
                new_len,
                delta,
            );
        }
        shift_boundary(&mut self.base_chrome_start, old.end, delta);
        shift_boundary(&mut self.base_quads, old.end, delta);
        for layer in &mut self.overlay_layers {
            shift_boundary(&mut layer.quad_start, old.end, delta);
            shift_boundary(&mut layer.quad_end, old.end, delta);
            shift_boundary(&mut layer.chrome_start, old.end, delta);
            shift_boundary(&mut layer.chrome_end, old.end, delta);
        }
    }

    fn shift_terminal_glyph_ranges(
        &mut self,
        id: schnellui_scene::WidgetId,
        old: std::ops::Range<u32>,
        new_len: u32,
    ) {
        let delta = new_len as i64 - old.len() as i64;
        if delta == 0 {
            return;
        }
        let replaced_ordinal = self.terminal_fragments[&id].ordinal;
        for fragment in self.terminal_fragments.values_mut() {
            shift_fragment_range(
                &mut fragment.glyphs,
                fragment.ordinal,
                replaced_ordinal,
                &old,
                new_len,
                delta,
            );
        }
        shift_boundary(&mut self.base_glyphs, old.end, delta);
        for layer in &mut self.overlay_layers {
            shift_boundary(&mut layer.glyph_start, old.end, delta);
            shift_boundary(&mut layer.glyph_end, old.end, delta);
        }
    }

    /// Walks the retained tree pre-order from the root, refilling the scratch instance
    /// buffers from each node's paint primitives (§3.2). Cleared-and-refilled — no
    /// steady-state allocation once the scratch capacity is warm (§4.4).
    ///
    /// **Scroll composition (SOUL §3.2).** Each stack frame carries the accumulated
    /// `(offset, clip)`: the root starts unclipped at zero offset. Every emitted
    /// instance has `offset` added to its rect (or line endpoints) and carries `clip`.
    /// A [`WidgetKind::Scroll`] node shifts its children by `−scroll_offset(id)` and
    /// intersects the running clip with its own laid-out viewport rect — so scrolling
    /// re-composites the descendants without any relayout or per-node repaint.
    pub(crate) fn gather(&mut self, scene: &Scene) {
        self.quad_scratch.clear();
        self.chrome_scratch.clear();
        self.glyph_scratch.clear();
        self.image_scratch.clear();
        self.terminal_fragments.clear();
        self.next_terminal_fragment_ordinal = 0;
        self.walk_stack.clear();
        self.overlay_roots.clear();
        self.overlay_layers.clear();
        let unclipped = Rect::new(
            UNCLIPPED_CLIP[0],
            UNCLIPPED_CLIP[1],
            UNCLIPPED_CLIP[2],
            UNCLIPPED_CLIP[3],
        );
        if let Some(root) = scene.root() {
            self.walk_stack
                .push((root, Point::default(), unclipped, false));
        }
        // Base layer: the whole tree except overlay subtrees, which are deferred
        // with the (offset, clip) they inherit where they sit (SOUL §3.2 z-order).
        self.drain_walk(scene, true);
        self.base_chrome_start = self.quad_scratch.len() as u32;
        self.quad_scratch.extend_from_slice(&self.chrome_scratch);
        self.base_quads = self.quad_scratch.len() as u32;
        self.base_glyphs = self.glyph_scratch.len() as u32;
        self.base_images = self.image_scratch.len() as u32;
        // Overlay layer: mutable foreground order within an explicit stacking
        // level (modal dialogs > modeless peers > ordinary popups). The base walk's
        // LIFO traversal discovers siblings backwards; sorting by the scene's
        // `(level, order)` makes focus/press raises authoritative. Drain one subtree
        // at a time so later roots really emit later and paint on top.
        self.overlay_roots.reverse();
        self.overlay_roots
            .sort_by_key(|(id, _, _)| (scene.overlay_level(*id), scene.overlay_order(*id)));
        for i in 0..self.overlay_roots.len() {
            let root = self.overlay_roots[i];
            self.chrome_scratch.clear();
            let start = InstanceLayer {
                quad_start: self.quad_scratch.len() as u32,
                glyph_start: self.glyph_scratch.len() as u32,
                image_start: self.image_scratch.len() as u32,
                ..Default::default()
            };
            self.walk_stack.push((root.0, root.1, root.2, false));
            self.drain_walk(scene, false);
            let chrome_start = self.quad_scratch.len() as u32;
            self.quad_scratch.extend_from_slice(&self.chrome_scratch);
            self.overlay_layers.push(InstanceLayer {
                quad_end: chrome_start,
                chrome_start,
                chrome_end: self.quad_scratch.len() as u32,
                glyph_end: self.glyph_scratch.len() as u32,
                image_end: self.image_scratch.len() as u32,
                ..start
            });
        }
    }

    /// Drains [`Self::walk_stack`], emitting instances for every popped node and
    /// pushing its children — the shared body of both gather layers (SOUL §3.2).
    /// With `defer_overlays`, an overlay-flagged node is parked in
    /// [`Self::overlay_roots`] (subtree untouched) instead of emitted.
    fn drain_walk(&mut self, scene: &Scene, defer_overlays: bool) {
        while let Some((id, offset, clip, paint_after_children)) = self.walk_stack.pop() {
            if !scene.is_visible(id) {
                continue;
            }
            if !paint_after_children && defer_overlays && scene.is_overlay(id) {
                self.overlay_roots.push((id, offset, clip));
                continue;
            }
            let clip_arr = [clip.x, clip.y, clip.width, clip.height];
            let Some(node) = scene.node(id) else {
                continue;
            };
            let is_scroll = node.kind == WidgetKind::Scroll;
            let terminal_start = if node.kind == WidgetKind::TerminalGrid && !paint_after_children {
                let ordinal = self.next_terminal_fragment_ordinal;
                self.next_terminal_fragment_ordinal =
                    self.next_terminal_fragment_ordinal.saturating_add(1);
                Some(TerminalGpuFragment {
                    quads: self.quad_scratch.len() as u32..self.quad_scratch.len() as u32,
                    glyphs: self.glyph_scratch.len() as u32..self.glyph_scratch.len() as u32,
                    images: self.image_scratch.len() as u32..self.image_scratch.len() as u32,
                    offset,
                    clip,
                    ordinal,
                })
            } else {
                None
            };

            // Text and RichText are paint leaves whose glyph bounds are contained
            // by their final layout box. If that box is completely outside the
            // inherited scroll clip, skip the node before touching its potentially
            // thousands of glyph primitives. Do not apply this to containers or
            // arbitrary paint leaves: their descendants/primitives may legitimately
            // overflow their own layout box (shadows, popups, custom drawing).
            if !paint_after_children
                && !is_scroll
                && node.children.is_empty()
                && matches!(node.kind, WidgetKind::Text | WidgetKind::RichText)
                && scene.layout(id).is_some_and(|layout| {
                    !layout.rect.is_empty()
                        && Rect::new(
                            layout.rect.x + offset.x,
                            layout.rect.y + offset.y,
                            layout.rect.width,
                            layout.rect.height,
                        )
                        .intersect(&clip)
                        .is_empty()
                })
            {
                continue;
            }
            if is_scroll && !paint_after_children {
                // The viewport's own primitives are fixed chrome (the optional
                // scrollbar). Push an exit marker before its children so LIFO order
                // emits that chrome last, above content rather than beneath it.
                self.walk_stack.push((id, offset, clip, true));
            }
            if !is_scroll || paint_after_children {
                if let Some(pd) = scene.paint(id) {
                    for prim in &pd.primitives {
                        match *prim {
                            Primitive::SolidRect {
                                rect,
                                color,
                                corner_radius,
                            } => {
                                let r = Rect::new(
                                    rect.x + offset.x,
                                    rect.y + offset.y,
                                    rect.width,
                                    rect.height,
                                );
                                if r.intersect(&clip).is_empty() {
                                    continue;
                                }
                                let instance =
                                    QuadInstance::solid_clipped(r, color, corner_radius, clip_arr);
                                if paint_after_children {
                                    self.chrome_scratch.push(instance);
                                } else {
                                    self.quad_scratch.push(instance);
                                }
                            }
                            Primitive::GlyphQuad {
                                rect,
                                atlas_uv,
                                color,
                            } => {
                                let r = Rect::new(
                                    rect.x + offset.x,
                                    rect.y + offset.y,
                                    rect.width,
                                    rect.height,
                                );
                                if r.intersect(&clip).is_empty() {
                                    continue;
                                }
                                self.glyph_scratch.push(GlyphInstance::glyph_clipped(
                                    r, atlas_uv, color, clip_arr,
                                ));
                            }
                            Primitive::Line {
                                from,
                                to,
                                width,
                                color,
                            } => {
                                let a = Point {
                                    x: from.x + offset.x,
                                    y: from.y + offset.y,
                                };
                                let b = Point {
                                    x: to.x + offset.x,
                                    y: to.y + offset.y,
                                };
                                let half_width = width * 0.5;
                                let bounds = Rect::new(
                                    a.x.min(b.x) - half_width,
                                    a.y.min(b.y) - half_width,
                                    (a.x - b.x).abs() + width,
                                    (a.y - b.y).abs() + width,
                                );
                                if bounds.intersect(&clip).is_empty() {
                                    continue;
                                }
                                let instance = QuadInstance::line(a, b, width, color, clip_arr);
                                if paint_after_children {
                                    self.chrome_scratch.push(instance);
                                } else {
                                    self.quad_scratch.push(instance);
                                }
                            }
                            Primitive::ImageQuad {
                                rect,
                                atlas_uv,
                                tint,
                            } => {
                                let r = Rect::new(
                                    rect.x + offset.x,
                                    rect.y + offset.y,
                                    rect.width,
                                    rect.height,
                                );
                                if r.intersect(&clip).is_empty() {
                                    continue;
                                }
                                // instances reuse the glyph layout: dest, uv, tint, clip.
                                self.image_scratch.push(GlyphInstance::glyph_clipped(
                                    r, atlas_uv, tint, clip_arr,
                                ));
                            }
                        }
                    }
                }
            }
            if let Some(mut fragment) = terminal_start {
                fragment.quads.end = self.quad_scratch.len() as u32;
                fragment.glyphs.end = self.glyph_scratch.len() as u32;
                fragment.images.end = self.image_scratch.len() as u32;
                self.terminal_fragments.insert(id, fragment);
            }
            if paint_after_children {
                continue;
            }
            // Compute the child (offset, clip): a Scroll node shifts and clips its
            // descendants; every other node passes its state through unchanged (§3.2).
            let (child_offset, child_clip) =
                if scene.node(id).map(|n| n.kind) == Some(WidgetKind::Scroll) {
                    let so = scene.scroll_offset(id);
                    let co = Point {
                        x: offset.x - so.x,
                        y: offset.y - so.y,
                    };
                    let cc = match scene.layout(id) {
                        // the viewport frame itself is drawn at the parent `offset` (it is
                        // not scrolled — only its children are), so offset the clip rect.
                        Some(b) if !b.rect.is_empty() => {
                            let vp = Rect::new(
                                b.rect.x + offset.x,
                                b.rect.y + offset.y,
                                b.rect.width,
                                b.rect.height,
                            );
                            clip.intersect(&vp)
                        }
                        // no layout yet (pre-first-layout mount): keep the parent clip.
                        _ => clip,
                    };
                    (co, cc)
                } else {
                    (offset, clip)
                };
            // Push children reversed so they pop in tree order (parent painted
            // first, children on top).
            for child in node.children.iter().rev() {
                self.walk_stack
                    .push((*child, child_offset, child_clip, false));
            }
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
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("schnellui.glyph_atlas"),
            size: wgpu::Extent3d {
                width: aw,
                height: ah,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            atlas.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(aw),
                rows_per_image: Some(ah),
            },
            wgpu::Extent3d {
                width: aw,
                height: ah,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("schnellui.glyph_atlas_bg"),
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
        self.atlas = Some(AtlasGpu {
            texture,
            view,
            bind_group,
            width: aw,
            height: ah,
        });
        if let Some(shadow) = self.atlas_shadow.as_mut() {
            shadow.clear();
            shadow.extend_from_slice(atlas.pixels());
        }
        true
    }

    /// Uploads a single dirty sub-rect of the CPU atlas into the GPU texture via
    /// `write_texture` (SOUL §3.2). Rows are gathered into `atlas_scratch` because the
    /// source is strided by the full atlas width.
    fn write_atlas_subrect(&mut self, atlas: &GlyphAtlas, rect: schnellui_text::AtlasRect) {
        if rect.is_empty() {
            return;
        }
        let atlas_w = atlas.width();
        let src = atlas.pixels();
        let rw = rect.width as usize;
        let rh = rect.height as usize;
        self.atlas_scratch.clear();
        self.atlas_scratch.reserve(rw * rh);
        for row in 0..rh {
            let sy = rect.y as usize + row;
            let start = sy * atlas_w as usize + rect.x as usize;
            self.atlas_scratch
                .extend_from_slice(&src[start..start + rw]);
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas.as_ref().unwrap().texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: rect.x,
                    y: rect.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &self.atlas_scratch,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(rect.width),
                rows_per_image: Some(rect.height),
            },
            wgpu::Extent3d {
                width: rect.width,
                height: rect.height,
                depth_or_array_layers: 1,
            },
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
    fn ensure_image_atlas(&mut self, images: &schnellui_scene::ImageAtlas) {
        let (iw, ih) = (images.width(), images.height());
        if iw == 0 || ih == 0 {
            return;
        }
        // Steady state: same texture, same content.
        if let Some(a) = &self.image_atlas {
            if a.width == iw && a.height == ih && a.revision == images.revision() {
                return;
            }
        }
        // Same dimensions but new content: refresh the resident texture in place.
        let needs_recreate = match &self.image_atlas {
            Some(a) => a.width != iw || a.height != ih,
            None => true,
        };
        if needs_recreate {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("schnellui.image_atlas"),
                size: wgpu::Extent3d {
                    width: iw,
                    height: ih,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // sRGB: sampled texels decode to linear in-shader, and the sRGB
                // target re-encodes — image bytes round-trip exactly (SOUL §7.2).
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("schnellui.image_atlas_bg"),
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
            self.image_atlas = Some(ImageAtlasGpu {
                texture,
                view,
                bind_group,
                width: iw,
                height: ih,
                revision: 0,
            });
        }
        let dirty = images.take_dirty();
        if needs_recreate || dirty.is_none() {
            let gpu = self.image_atlas.as_ref().unwrap();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &gpu.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                images.pixels(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(iw * 4),
                    rows_per_image: Some(ih),
                },
                wgpu::Extent3d {
                    width: iw,
                    height: ih,
                    depth_or_array_layers: 1,
                },
            );
        } else if let Some(rect) = dirty {
            self.write_image_atlas_subrect(images, rect);
        }
        self.image_atlas.as_mut().unwrap().revision = images.revision();
    }

    /// Uploads one tightly packed RGBA sub-rectangle from the strided CPU atlas.
    fn write_image_atlas_subrect(
        &mut self,
        images: &schnellui_scene::ImageAtlas,
        rect: schnellui_scene::TexelRect,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let atlas_width = images.width() as usize;
        let row_bytes = rect.width as usize * BYTES_PER_PIXEL as usize;
        self.atlas_scratch.clear();
        self.atlas_scratch.reserve(row_bytes * rect.height as usize);
        for row in 0..rect.height as usize {
            let source_row = rect.y as usize + row;
            let start = (source_row * atlas_width + rect.x as usize) * BYTES_PER_PIXEL as usize;
            self.atlas_scratch
                .extend_from_slice(&images.pixels()[start..start + row_bytes]);
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.image_atlas.as_ref().unwrap().texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: rect.x,
                    y: rect.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &self.atlas_scratch,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(rect.width * BYTES_PER_PIXEL),
                rows_per_image: Some(rect.height),
            },
            wgpu::Extent3d {
                width: rect.width,
                height: rect.height,
                depth_or_array_layers: 1,
            },
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
    }
}

/// Returns the smallest texel rectangle containing every changed byte in two
/// equally-sized R8 images. Equal rows take one slice comparison and no scanning.
pub(crate) fn changed_r8_rect(
    previous: &[u8],
    replacement: &[u8],
    width: u32,
    height: u32,
) -> Option<schnellui_text::AtlasRect> {
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
    Some(schnellui_text::AtlasRect {
        x: min_x as u32,
        y: min_y as u32,
        width: (max_x - min_x + 1) as u32,
        height: (max_y - min_y + 1) as u32,
    })
}

// The headless renderer: owns a [`GpuCore`], the offscreen target, and the padded
// readback buffer (SOUL §7.2, §3.2, §4.4). This is the byte-identical PNG path.
