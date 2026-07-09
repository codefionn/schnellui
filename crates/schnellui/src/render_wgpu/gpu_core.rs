use crate::render_wgpu::renderer::{ensure_capacity, srgb_to_linear};
use crate::render_wgpu::shader::SHADER_SRC;
use crate::render_wgpu::types::*;
use schnellui::scene::{Color, Point, Primitive, Rect, Scene, WidgetId, WidgetKind, WidgetNode};
use schnellui::text::GlyphAtlas;

/// What a tree walk does with the fragments it visits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WalkMode {
    /// Full gather: create the fragments, pad every slot to its capacity.
    Gather,
    /// Re-walk of one scroll's content: refresh the existing fragments with exact
    /// (unpadded) ranges relative to the staging vectors.
    Region,
}

/// Slot capacity for `live` instances: the next power of two, so a node can gain a
/// few instances without moving anything behind it.
fn slot_capacity(live: usize) -> usize {
    if live == 0 {
        0
    } else {
        live.next_power_of_two()
    }
}

/// `rect` translated by the accumulated scroll `offset`.
fn shifted(rect: Rect, offset: Point) -> Rect {
    Rect::new(
        rect.x + offset.x,
        rect.y + offset.y,
        rect.width,
        rect.height,
    )
}

/// Converts one node's already-positioned display list into instances: quads and
/// lines into `quads`, glyphs into `glyphs`, images into `images`. `offset` is the
/// accumulated scroll translation and `clip` the accumulated viewport clip; a
/// primitive entirely outside `clip` is dropped (SOUL §3.2).
fn encode_prims(
    primitives: &[Primitive],
    offset: Point,
    clip: Rect,
    quads: &mut Vec<QuadInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    images: &mut Vec<GlyphInstance>,
) {
    let clip_arr = [clip.x, clip.y, clip.width, clip.height];
    for prim in primitives {
        match *prim {
            Primitive::SolidRect {
                rect,
                color,
                corner_radius,
            } => {
                let rect = shifted(rect, offset);
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
                let rect = shifted(rect, offset);
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
                let rect = shifted(rect, offset);
                if !rect.intersect(&clip).is_empty() {
                    // instances reuse the glyph layout: dest, uv, tint, clip.
                    images.push(GlyphInstance::glyph_clipped(rect, atlas_uv, tint, clip_arr));
                }
            }
        }
    }
}

/// Text and RichText are paint leaves whose glyph bounds are contained by their
/// final layout box. If that box is completely outside the inherited scroll clip,
/// the node is skipped before touching its potentially thousands of glyph
/// primitives. This is not applied to containers or arbitrary paint leaves: their
/// descendants/primitives may legitimately overflow their own layout box (shadows,
/// popups, custom drawing).
fn leaf_culled(scene: &Scene, id: WidgetId, node: &WidgetNode, offset: Point, clip: Rect) -> bool {
    node.children.is_empty()
        && matches!(node.kind, WidgetKind::Text | WidgetKind::RichText)
        && scene.layout(id).is_some_and(|layout| {
            !layout.rect.is_empty() && shifted(layout.rect, offset).intersect(&clip).is_empty()
        })
}

/// The `(offset, clip)` a `Scroll` node hands to its descendants: shifted by its
/// scroll offset and clipped to its laid-out viewport rect.
fn scroll_child_ctx(scene: &Scene, id: WidgetId, offset: Point, clip: Rect) -> (Point, Rect) {
    let so = scene.scroll_offset(id);
    let co = Point {
        x: offset.x - so.x,
        y: offset.y - so.y,
    };
    let cc = match scene.layout(id) {
        // the viewport frame itself is drawn at the parent `offset` (it is not
        // scrolled, only its children are), so offset the clip rect.
        Some(b) if !b.rect.is_empty() => clip.intersect(&shifted(b.rect, offset)),
        // no layout yet (pre-first-layout mount): keep the parent clip.
        _ => clip,
    };
    (co, cc)
}

/// Appends `v[start..]`'s padding up to its slot capacity when `pad`, and returns
/// the vector's new length and the number of live instances.
fn seal_vec<T: bytemuck::Pod>(v: &mut Vec<T>, start: usize, pad: bool) -> (usize, u32) {
    let live = v.len() - start;
    if pad {
        v.resize(
            start + slot_capacity(live),
            <T as bytemuck::Zeroable>::zeroed(),
        );
    }
    (v.len(), live as u32)
}

/// Overwrites `scratch[start..start + cap]` with `staged` followed by zero padding
/// and returns the span that actually changed, if any.
fn patch_slot<T: bytemuck::Pod + PartialEq>(
    scratch: &mut [T],
    start: usize,
    cap: usize,
    staged: &[T],
) -> Option<(usize, usize)> {
    let zero = <T as bytemuck::Zeroable>::zeroed();
    let mut span: Option<(usize, usize)> = None;
    for (i, cell) in scratch[start..start + cap].iter_mut().enumerate() {
        let new = staged.get(i).copied().unwrap_or(zero);
        if *cell != new {
            *cell = new;
            let at = start + i;
            span = Some(span.map_or((at, at + 1), |(lo, _)| (lo, at + 1)));
        }
    }
    span
}

/// What writing a node's fresh instances into its slot required.
enum SlotEdit {
    Same,
    /// The slot's contents changed within `[lo, hi)`; nothing moved.
    InPlace(usize, usize),
    /// The slot grew to `new_cap`; the CPU mirror is already spliced from `from`.
    Grow {
        new_cap: u32,
        from: usize,
    },
}

/// Writes `staged` into the slot `range` of `scratch`. Growth beyond the slot's
/// capacity splices the vector: the first overflow of a slot fits it exactly, a slot
/// that overflows again gets doubled headroom.
fn apply_slot<T: bytemuck::Pod + PartialEq>(
    scratch: &mut Vec<T>,
    range: &std::ops::Range<u32>,
    staged: &[T],
    overflowed: bool,
) -> SlotEdit {
    let start = range.start as usize;
    let cap = range.len();
    if staged.len() <= cap {
        return match patch_slot(scratch, start, cap, staged) {
            Some((lo, hi)) => SlotEdit::InPlace(lo, hi),
            None => SlotEdit::Same,
        };
    }
    let new_cap = if overflowed {
        slot_capacity(staged.len()).max(cap * 2)
    } else {
        staged.len()
    };
    scratch.splice(
        start..start + cap,
        staged.iter().copied().chain(std::iter::repeat_n(
            <T as bytemuck::Zeroable>::zeroed(),
            new_cap - staged.len(),
        )),
    );
    SlotEdit::Grow {
        new_cap: new_cap as u32,
        from: start,
    }
}

/// A contiguous run of slots being replaced as one piece.
struct RunPlan {
    old_start: u32,
    old_end: u32,
    total: u32,
}

/// Writes `staged` (the run's live instances in draw order) over the run in `scratch`,
/// padding to the planned total. Returns the span to upload.
fn splice_run<T: bytemuck::Pod + PartialEq>(
    scratch: &mut Vec<T>,
    plan: &RunPlan,
    staged: &[T],
) -> (usize, usize) {
    let (start, end, total) = (
        plan.old_start as usize,
        plan.old_end as usize,
        plan.total as usize,
    );
    if total == end - start {
        return patch_slot(scratch, start, total, staged).unwrap_or((0, 0));
    }
    scratch.splice(
        start..end,
        staged.iter().copied().chain(std::iter::repeat_n(
            <T as bytemuck::Zeroable>::zeroed(),
            total - staged.len(),
        )),
    );
    (start, scratch.len())
}

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

/// Adds `delta` to every fragment's `b` range behind list position `from`.
fn shift_after(table: &mut FragmentTable, b: Buf, from: usize, delta: i64) {
    let FragmentTable { frags, order, .. } = table;
    for &fi in &order[b as usize][from.min(order[b as usize].len())..] {
        let r = frags[fi as usize].range_mut(b);
        r.start = (r.start as i64 + delta) as u32;
        r.end = (r.end as i64 + delta) as u32;
    }
}

fn shift_pos(v: &mut u32, delta: i64) {
    *v = (*v as i64 + delta) as u32;
}

/// Whether some proper ancestor of `id` is in `scrolls`.
fn covered_by(scene: &Scene, scrolls: &[WidgetId], id: WidgetId) -> bool {
    let mut cur = scene.node(id).and_then(|n| n.parent);
    while let Some(p) = cur {
        if scrolls.contains(&p) {
            return true;
        }
        cur = scene.node(p).and_then(|n| n.parent);
    }
    false
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
    pub(crate) walk_stack: Vec<(schnellui::scene::WidgetId, Point, Rect, bool)>,
    /// overlay subtree roots deferred out of the base walk (SOUL §3.2 z-order),
    /// each with the (offset, clip) it inherited where it sits in the tree; walked
    /// after the base pass so their instances land in the overlay draw ranges.
    /// Cleared-and-refilled like the walk stack (§4.4); empty for overlay-less
    /// scenes, so the whole layer machinery costs nothing then.
    pub(crate) overlay_roots: Vec<(schnellui::scene::WidgetId, Point, Rect)>,
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

    /// Per-node retained instance ranges from the most recent full gather. A
    /// paint-dirty node re-encodes only its own slot; tree, overlay-order and
    /// visibility changes rebuild the table with a full gather.
    pub(crate) fragments: FragmentTable,
    /// `(root, level, order)` of the overlays in draw order, as gathered.
    pub(crate) overlay_sig: Vec<(WidgetId, u8, u64)>,
    /// The overlay root being drawn as a layer of its own, which its walk must
    /// not defer again.
    overlay_root_walk: Option<WidgetId>,
    pub(crate) retained_scene_key: Option<schnellui::scene::SceneRenderKey>,
    /// `Scene::paint_epoch` the retained instance data was gathered or patched at.
    pub(crate) retained_paint_epoch: u64,
    /// Overlay index (0 = base) the walk is currently emitting into.
    pub(crate) walk_layer: u32,
    /// Scroll nodes whose chrome quads await the layer's chrome flush.
    pub(crate) chrome_pending: Vec<u32>,
    /// Generation stamped on fragments refreshed by a region re-walk.
    pub(crate) walk_stamp: u32,
    /// Scroll exits seen by the current region re-walk.
    pub(crate) walk_exits: u32,
    /// Staging copies of one node's (or one scroll region's) fresh instances.
    pub(crate) stage_quads: Vec<QuadInstance>,
    pub(crate) stage_chrome: Vec<QuadInstance>,
    pub(crate) stage_glyphs: Vec<GlyphInstance>,
    pub(crate) stage_images: Vec<GlyphInstance>,
    /// Dirty `Scroll` nodes of the current incremental update.
    pub(crate) dirty_scrolls: Vec<WidgetId>,
    pub(crate) last_uniforms: Option<Uniforms>,
    pub(crate) last_upload_work: GpuUploadWork,
    /// Per-primitive instance offsets for fragments of nodes that report dirty
    /// primitive ranges (`Scene::paint_dirty_ranges`). Built on demand; a full
    /// gather drops them all, a scroll re-walk drops those of the fragments it
    /// refreshed.
    pub(crate) prim_prefix: std::collections::HashMap<WidgetId, PrimPrefix>,
}

/// Where each primitive of one node lands in its instance slots: `offsets[i]`
/// counts the quad, glyph and image instances (indexed by [`Buf`]) produced by
/// primitives `[0, i)`. Valid only for the `offset` and `clip` it was built under.
#[derive(Default)]
pub(crate) struct PrimPrefix {
    /// The fragment it was built for.
    fi: u32,
    offsets: Vec<[u32; 3]>,
    offset: Point,
    clip: Rect,
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

        // pipeline 2: glyph quads sampling the R8 atlas (bind groups 0 + 1).
        let glyph_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("schnellui.glyph_layout"),
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
        // the texture+sampler layout is identical to the glyph one, so the layout is
        // shared). Instances reuse the glyph 4×vec4 layout: dest rect, texel uv,
        // tint colour, clip (SOUL §3.2).
        let image_pipeline = pipeline(
            "schnellui.image_pipeline",
            &glyph_layout,
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
            fragments: FragmentTable::default(),
            overlay_sig: Vec::new(),
            overlay_root_walk: None,
            retained_scene_key: None,
            retained_paint_epoch: 0,
            walk_layer: 0,
            chrome_pending: Vec::new(),
            walk_stamp: 0,
            walk_exits: 0,
            stage_quads: Vec::new(),
            stage_chrome: Vec::new(),
            stage_glyphs: Vec::new(),
            stage_images: Vec::new(),
            dirty_scrolls: Vec::new(),
            last_uniforms: None,
            last_upload_work: GpuUploadWork::default(),
            prim_prefix: std::collections::HashMap::new(),
        }
    }

    /// Brings the resident instance buffers up to date with `scene`, ensures the GPU
    /// atlases exist for glyphs + images, and writes the per-frame uniforms: the
    /// shared steps 1-3 both targets run before their render pass. `viewport_w/h`
    /// are the **physical** target dimensions; `scale` is the logical->physical
    /// factor the vertex stage applies (SOUL §7.1). Returns
    /// `(quad_count, glyph_count, image_count)`.
    ///
    /// When only paint changed since this renderer last drew, each paint-dirty node
    /// rewrites just its own retained instance slot ([`GpuCore::try_incremental`]);
    /// a structural change falls back to a full gather and upload of all buffers.
    pub(crate) fn upload_scene(
        &mut self,
        scene: &Scene,
        atlas: &GlyphAtlas,
        viewport_w: u32,
        viewport_h: u32,
        scale: f32,
    ) -> (u32, u32, u32) {
        self.last_upload_work = GpuUploadWork::default();
        if !self.try_incremental(scene) {
            // 1. Gather primitives from the retained tree into the scratch buffers.
            self.gather(scene);
            self.last_upload_work.full_gathers = 1;
            // 2. Upload instance data into the resident grow-only buffers (§3.2).
            for b in BUFS {
                let len = self.scratch_len(b);
                self.upload_range(b, 0, len);
            }
        }
        self.retained_scene_key = Some(scene.render_key());
        self.retained_paint_epoch = scene.paint_epoch();
        // The retained instances are current, but atlases are shared with other
        // renderers and may have gained texels since this one last looked.
        if !self.glyph_scratch.is_empty() {
            self.sync_atlas(atlas);
        }
        if !self.image_scratch.is_empty() {
            self.ensure_image_atlas(scene.images());
        }
        self.write_uniforms(scene, atlas, viewport_w, viewport_h, scale);
        (
            self.quad_scratch.len() as u32,
            self.glyph_scratch.len() as u32,
            self.image_scratch.len() as u32,
        )
    }

    /// Forgets the retained instances so the next upload is a full gather.
    #[cfg(test)]
    pub(crate) fn invalidate_retained(&mut self) {
        self.retained_scene_key = None;
    }

    fn scratch_len(&self, b: Buf) -> usize {
        match b {
            Buf::Quad => self.quad_scratch.len(),
            Buf::Glyph => self.glyph_scratch.len(),
            Buf::Image => self.image_scratch.len(),
        }
    }

    /// Writes instances `[lo, hi)` of buffer `b` from the CPU mirror to the GPU,
    /// growing the buffer (and then writing all of it) when it is too small.
    fn upload_range(&mut self, b: Buf, lo: usize, hi: usize) {
        match b {
            Buf::Quad => upload_slice(
                &self.device,
                &self.queue,
                &mut self.quad_buf,
                &mut self.quad_cap,
                &self.quad_scratch,
                lo,
                hi,
                "schnellui.quad_buf",
                &mut self.last_upload_work,
            ),
            Buf::Glyph => upload_slice(
                &self.device,
                &self.queue,
                &mut self.glyph_buf,
                &mut self.glyph_cap,
                &self.glyph_scratch,
                lo,
                hi,
                "schnellui.glyph_buf",
                &mut self.last_upload_work,
            ),
            Buf::Image => upload_slice(
                &self.device,
                &self.queue,
                &mut self.image_buf,
                &mut self.image_cap,
                &self.image_scratch,
                lo,
                hi,
                "schnellui.image_buf",
                &mut self.last_upload_work,
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
        self.last_upload_work.bytes.other_bytes += std::mem::size_of::<Uniforms>() as u64;
    }

    // --- incremental update: per-node retained instance slots -----------------

    /// Updates the retained instances for the nodes painted since this renderer last
    /// drew, without walking the scene. Returns `false` when a full gather is
    /// needed: a structural change, pending layout, a missed paint notification, a
    /// dirty overlay whose stacking position moved, or a scroll region that holds an
    /// overlay.
    ///
    /// Each paint-dirty node re-encodes its own primitives with the scroll offset and
    /// clip it was gathered under, and rewrites its slot. A node that outgrows its
    /// slot splices the CPU mirror and shifts every slot drawn after it, so draw
    /// order never changes; only the bytes behind the node are rewritten. A dirty
    /// `Scroll` node re-walks its whole content run instead, because its offset
    /// moved every descendant.
    fn try_incremental(&mut self, scene: &Scene) -> bool {
        let same = self
            .retained_scene_key
            .is_some_and(|key| key.same_structure(&scene.render_key()));
        if !same || !scene.layout_dirty().is_empty() {
            return false;
        }
        // Nothing painted since this renderer last drew: the frame pass may have
        // cleared the dirty lists, but the epoch still tells.
        if self.retained_paint_epoch == scene.paint_epoch() {
            return true;
        }
        // The dirty list only names nodes painted after the last clear. A renderer
        // that last drew before that clear missed some and must re-gather.
        if self.retained_paint_epoch < scene.paint_dirty_base_epoch() {
            return false;
        }
        let dirty = scene.paint_dirty();
        if dirty.is_empty() {
            return false;
        }

        self.dirty_scrolls.clear();
        for &id in dirty {
            let Some(node) = scene.node(id) else {
                return false;
            };
            // A scroll the last gather never reached (hidden) has no run to
            // re-walk and nothing drawn; it must not force a full gather.
            if node.kind == WidgetKind::Scroll && self.fragments.by_node.contains_key(&id) {
                self.dirty_scrolls.push(id);
            }
        }
        for &id in dirty {
            if self.fragments.by_node.contains_key(&id) {
                if scene.is_overlay(id) && !self.overlay_unchanged(scene, id) {
                    return false;
                }
            } else if scene.is_effectively_visible(id) {
                // A visible node the last gather never reached.
                return false;
            }
        }

        let scrolls = std::mem::take(&mut self.dirty_scrolls);
        let mut ok = true;
        let stamp_before = self.walk_stamp;
        for &sid in &scrolls {
            if !covered_by(scene, &scrolls, sid) && !self.regather_scroll(scene, sid) {
                ok = false;
                break;
            }
        }
        let walks = self.walk_stamp.wrapping_sub(stamp_before);
        if walks > 0 && !self.prim_prefix.is_empty() {
            // A re-walk may have moved, re-clipped or re-encoded the fragments it
            // stamped; their primitive maps are stale.
            let frags = &self.fragments.frags;
            self.prim_prefix.retain(|_, prefix| {
                frags.get(prefix.fi as usize).is_some_and(|f| {
                    let d = f.stamp.wrapping_sub(stamp_before);
                    d == 0 || d > walks
                })
            });
        }
        if ok {
            for &id in dirty {
                let Some(&fi) = self.fragments.by_node.get(&id) else {
                    continue;
                };
                if !scrolls.is_empty() && covered_by(scene, &scrolls, id) {
                    continue;
                }
                let ranges = scene.paint_dirty_ranges(id);
                if ranges.is_some_and(|ranges| self.update_node_ranges(scene, id, fi, ranges)) {
                    self.last_upload_work.ranges_patched += 1;
                } else {
                    if !self.update_node(scene, id, fi) {
                        ok = false;
                        break;
                    }
                    // A node that reports ranges (or did before) gets its
                    // primitive-to-instance map rebuilt for the next ranged frame.
                    if ranges.is_some() || self.prim_prefix.contains_key(&id) {
                        self.rebuild_prefix(scene, id, fi);
                    }
                }
                self.last_upload_work.nodes_patched += 1;
            }
        }
        self.dirty_scrolls = scrolls;
        ok
    }

    /// Whether overlay root `id` still sits where the last gather put it.
    fn overlay_unchanged(&self, scene: &Scene, id: WidgetId) -> bool {
        self.overlay_sig.iter().any(|&(root, level, order)| {
            root == id && level == scene.overlay_level(id) && order == scene.overlay_order(id)
        })
    }

    /// Re-encodes only primitives `ranges` of a node whose primitive count did not
    /// change, writing them over the instances they produced last time. Returns
    /// `false` (having possibly rewritten some ranges already) when the caller must
    /// re-encode the whole node: no valid [`PrimPrefix`], a culled leaf, or a range
    /// whose primitives now produce a different number of instances per buffer.
    fn update_node_ranges(
        &mut self,
        scene: &Scene,
        id: WidgetId,
        fi: u32,
        ranges: &[std::ops::Range<u32>],
    ) -> bool {
        let (Some(node), Some(paint)) = (scene.node(id), scene.paint(id)) else {
            return false;
        };
        let (offset, clip, live, starts) = {
            let f = &self.fragments.frags[fi as usize];
            (
                f.offset,
                f.clip,
                f.live,
                [f.quads.start, f.glyphs.start, f.images.start],
            )
        };
        let Some(prefix) = self.prim_prefix.get(&id) else {
            return false;
        };
        let prims = &paint.primitives;
        if prefix.fi != fi
            || prefix.offsets.len() != prims.len() + 1
            || prefix.offset != offset
            || prefix.clip != clip
            || prefix.offsets.last() != Some(&live)
            || leaf_culled(scene, id, node, offset, clip)
        {
            return false;
        }
        let mut quads = std::mem::take(&mut self.stage_quads);
        let mut glyphs = std::mem::take(&mut self.stage_glyphs);
        let mut images = std::mem::take(&mut self.stage_images);
        let mut ok = true;
        for range in ranges {
            let (a, b) = (range.start as usize, range.end as usize);
            if a > b || b > prims.len() {
                ok = false;
                break;
            }
            quads.clear();
            glyphs.clear();
            images.clear();
            encode_prims(
                &prims[a..b],
                offset,
                clip,
                &mut quads,
                &mut glyphs,
                &mut images,
            );
            self.last_upload_work.primitives_encoded += b - a;
            let (from, to) = {
                let prefix = &self.prim_prefix[&id];
                (prefix.offsets[a], prefix.offsets[b])
            };
            let counts = [quads.len(), glyphs.len(), images.len()];
            if (0..3).any(|k| counts[k] != (to[k] - from[k]) as usize) {
                ok = false;
                break;
            }
            for b in BUFS {
                let k = b as usize;
                if counts[k] == 0 {
                    continue;
                }
                let start = (starts[k] + from[k]) as usize;
                let span = match b {
                    Buf::Quad => patch_slot(&mut self.quad_scratch, start, counts[k], &quads),
                    Buf::Glyph => patch_slot(&mut self.glyph_scratch, start, counts[k], &glyphs),
                    Buf::Image => patch_slot(&mut self.image_scratch, start, counts[k], &images),
                };
                if let Some((lo, hi)) = span {
                    self.upload_range(b, lo, hi);
                }
            }
        }
        self.stage_quads = quads;
        self.stage_glyphs = glyphs;
        self.stage_images = images;
        ok
    }

    /// Rebuilds fragment `fi`'s [`PrimPrefix`] from the node's current primitives,
    /// encoding them one at a time so the counts match `encode_prims` exactly. Runs
    /// once for a node that starts reporting ranges, and after any whole-node
    /// re-encode of such a node.
    fn rebuild_prefix(&mut self, scene: &Scene, id: WidgetId, fi: u32) {
        let (offset, clip) = {
            let f = &self.fragments.frags[fi as usize];
            (f.offset, f.clip)
        };
        let mut prefix = self.prim_prefix.remove(&id).unwrap_or_default();
        prefix.fi = fi;
        prefix.offsets.clear();
        prefix.offset = offset;
        prefix.clip = clip;
        let mut quads = std::mem::take(&mut self.stage_quads);
        let mut glyphs = std::mem::take(&mut self.stage_glyphs);
        let mut images = std::mem::take(&mut self.stage_images);
        let mut counts = [0_u32; 3];
        prefix.offsets.push(counts);
        if let Some(paint) = scene.paint(id) {
            for prim in &paint.primitives {
                quads.clear();
                glyphs.clear();
                images.clear();
                encode_prims(
                    std::slice::from_ref(prim),
                    offset,
                    clip,
                    &mut quads,
                    &mut glyphs,
                    &mut images,
                );
                counts[0] += quads.len() as u32;
                counts[1] += glyphs.len() as u32;
                counts[2] += images.len() as u32;
                prefix.offsets.push(counts);
            }
        }
        self.stage_quads = quads;
        self.stage_glyphs = glyphs;
        self.stage_images = images;
        self.prim_prefix.insert(id, prefix);
    }

    /// Re-encodes one node into its slots.
    fn update_node(&mut self, scene: &Scene, id: WidgetId, fi: u32) -> bool {
        let Some(node) = scene.node(id) else {
            return false;
        };
        let (offset, clip) = {
            let f = &self.fragments.frags[fi as usize];
            (f.offset, f.clip)
        };
        let mut quads = std::mem::take(&mut self.stage_quads);
        let mut glyphs = std::mem::take(&mut self.stage_glyphs);
        let mut images = std::mem::take(&mut self.stage_images);
        quads.clear();
        glyphs.clear();
        images.clear();
        if !leaf_culled(scene, id, node, offset, clip) {
            if let Some(paint) = scene.paint(id) {
                encode_prims(
                    &paint.primitives,
                    offset,
                    clip,
                    &mut quads,
                    &mut glyphs,
                    &mut images,
                );
                self.last_upload_work.primitives_encoded += paint.primitives.len();
            }
        }
        for b in BUFS {
            let (range, overflowed) = {
                let f = &self.fragments.frags[fi as usize];
                (f.range(b).clone(), f.overflowed)
            };
            let edit = match b {
                Buf::Quad => apply_slot(&mut self.quad_scratch, &range, &quads, overflowed),
                Buf::Glyph => apply_slot(&mut self.glyph_scratch, &range, &glyphs, overflowed),
                Buf::Image => apply_slot(&mut self.image_scratch, &range, &images, overflowed),
            };
            let live = match b {
                Buf::Quad => quads.len(),
                Buf::Glyph => glyphs.len(),
                Buf::Image => images.len(),
            };
            self.fragments.frags[fi as usize].live[b as usize] = live as u32;
            match edit {
                SlotEdit::Same => {}
                SlotEdit::InPlace(lo, hi) => self.upload_range(b, lo, hi),
                SlotEdit::Grow { new_cap, from } => {
                    self.last_upload_work.grown_slots += 1;
                    self.grow_slot(b, fi, new_cap);
                    let len = self.scratch_len(b);
                    self.upload_range(b, from, len);
                }
            }
        }
        self.stage_quads = quads;
        self.stage_glyphs = glyphs;
        self.stage_images = images;
        true
    }

    /// Records that fragment `fi`'s slot in `b` grew to `new_cap`: everything drawn
    /// after it moves back by the difference, as do the layer boundaries.
    fn grow_slot(&mut self, b: Buf, fi: u32, new_cap: u32) {
        let (old_cap, ord, layer, chrome) = {
            let f = &mut self.fragments.frags[fi as usize];
            let old_cap = f.range(b).len() as u32;
            f.range_mut(b).end = f.range(b).start + new_cap;
            f.overflowed = true;
            (
                old_cap,
                f.ord[b as usize],
                f.layer,
                f.chrome && b == Buf::Quad,
            )
        };
        let delta = new_cap as i64 - old_cap as i64;
        shift_after(&mut self.fragments, b, ord as usize + 1, delta);
        self.shift_boundaries(b, layer, chrome, delta);
    }

    /// Moves every layer boundary behind a resized slot. `layer` is the layer the
    /// slot sits in (0 = base) and `chrome` whether it is in that layer's chrome
    /// section. Boundaries are shifted by identity rather than by position, so empty
    /// slots sharing an index with a boundary cannot confuse it.
    fn shift_boundaries(&mut self, b: Buf, layer: u32, chrome: bool, delta: i64) {
        let layer = layer as usize;
        match b {
            Buf::Quad => {
                if layer == 0 {
                    if !chrome {
                        shift_pos(&mut self.base_chrome_start, delta);
                    }
                    shift_pos(&mut self.base_quads, delta);
                } else {
                    let l = &mut self.overlay_layers[layer - 1];
                    if !chrome {
                        shift_pos(&mut l.quad_end, delta);
                        shift_pos(&mut l.chrome_start, delta);
                    }
                    shift_pos(&mut l.chrome_end, delta);
                }
                for l in self.overlay_layers.iter_mut().skip(layer) {
                    shift_pos(&mut l.quad_start, delta);
                    shift_pos(&mut l.quad_end, delta);
                    shift_pos(&mut l.chrome_start, delta);
                    shift_pos(&mut l.chrome_end, delta);
                }
            }
            Buf::Glyph => {
                if layer == 0 {
                    shift_pos(&mut self.base_glyphs, delta);
                } else {
                    shift_pos(&mut self.overlay_layers[layer - 1].glyph_end, delta);
                }
                for l in self.overlay_layers.iter_mut().skip(layer) {
                    shift_pos(&mut l.glyph_start, delta);
                    shift_pos(&mut l.glyph_end, delta);
                }
            }
            Buf::Image => {
                if layer == 0 {
                    shift_pos(&mut self.base_images, delta);
                } else {
                    shift_pos(&mut self.overlay_layers[layer - 1].image_end, delta);
                }
                for l in self.overlay_layers.iter_mut().skip(layer) {
                    shift_pos(&mut l.image_start, delta);
                    shift_pos(&mut l.image_end, delta);
                }
            }
        }
    }

    /// Re-walks the content of dirty `Scroll` node `sid` and replaces its run of
    /// slots. The scroll offset (or viewport) changed, so every descendant moved and
    /// the visible set changed: the walk re-culls against the new clip. Slots in the
    /// run are rebuilt exact; the run keeps spare capacity at its end (the end
    /// marker's slot) so a scroll step whose visible count wobbles rewrites only the
    /// run. CPU cost is O(scroll content nodes), since the walk visits every
    /// descendant; uploads are O(visible). Returns `false` when the run cannot be
    /// rebuilt in place.
    fn regather_scroll(&mut self, scene: &Scene, sid: WidgetId) -> bool {
        let Some(&fi) = self.fragments.by_node.get(&sid) else {
            return false;
        };
        let (offset, clip, layer, region, chrome_ord) = {
            let f = &self.fragments.frags[fi as usize];
            (f.offset, f.clip, f.layer, f.region, f.ord[0])
        };
        let Some((begin, end)) = region else {
            return false;
        };
        let Some(node) = scene.node(sid) else {
            return false;
        };

        self.walk_stamp = self.walk_stamp.wrapping_add(1);
        let stamp = self.walk_stamp;
        self.walk_exits = 0;
        self.chrome_pending.clear();
        std::mem::swap(&mut self.quad_scratch, &mut self.stage_quads);
        std::mem::swap(&mut self.chrome_scratch, &mut self.stage_chrome);
        std::mem::swap(&mut self.glyph_scratch, &mut self.stage_glyphs);
        std::mem::swap(&mut self.image_scratch, &mut self.stage_images);
        self.quad_scratch.clear();
        self.chrome_scratch.clear();
        self.glyph_scratch.clear();
        self.image_scratch.clear();
        self.walk_stack.clear();
        let (child_offset, child_clip) = scroll_child_ctx(scene, sid, offset, clip);
        for child in node.children.iter().rev() {
            self.walk_stack
                .push((*child, child_offset, child_clip, false));
        }
        let walked = self.drain_walk(scene, false, WalkMode::Region);
        std::mem::swap(&mut self.quad_scratch, &mut self.stage_quads);
        std::mem::swap(&mut self.chrome_scratch, &mut self.stage_chrome);
        std::mem::swap(&mut self.glyph_scratch, &mut self.stage_glyphs);
        std::mem::swap(&mut self.image_scratch, &mut self.stage_images);
        if !walked {
            return false;
        }

        // Content run: everything between the begin marker and the end marker (whose
        // slot is the run's spare capacity).
        for b in BUFS {
            let (lo, hi, old_start, old_end) = {
                let t = &self.fragments;
                let (bg, en) = (&t.frags[begin as usize], &t.frags[end as usize]);
                (
                    bg.ord[b as usize] as usize + 1,
                    en.ord[b as usize] as usize + 1,
                    bg.range(b).end,
                    en.range(b).end,
                )
            };
            if !self.run_refreshed(b, lo, hi - 1, stamp) {
                return false;
            }
            let staged_len = match b {
                Buf::Quad => self.stage_quads.len(),
                Buf::Glyph => self.stage_glyphs.len(),
                Buf::Image => self.stage_images.len(),
            };
            let Some(plan) = self.plan_run(
                b, lo, hi, true, staged_len, layer, false, old_start, old_end,
            ) else {
                return false;
            };
            let (ulo, uhi) = match b {
                Buf::Quad => splice_run(&mut self.quad_scratch, &plan, &self.stage_quads),
                Buf::Glyph => splice_run(&mut self.glyph_scratch, &plan, &self.stage_glyphs),
                Buf::Image => splice_run(&mut self.image_scratch, &plan, &self.stage_images),
            };
            self.upload_range(b, ulo, uhi);
        }

        // Chrome of nested scrolls: the entries just before this scroll's own chrome
        // slot, in exit order.
        let nested = self.walk_exits as usize;
        if nested > 0 {
            let hi = chrome_ord as usize;
            if chrome_ord == NO_ORD || hi < nested {
                return false;
            }
            let lo = hi - nested;
            if !self.run_refreshed(Buf::Quad, lo, hi, stamp) {
                return false;
            }
            let (old_start, old_end) = {
                let t = &self.fragments;
                let order = &t.order[0];
                (
                    t.frags[order[lo] as usize].quads.start,
                    t.frags[order[hi - 1] as usize].quads.end,
                )
            };
            let staged_len = self.stage_chrome.len();
            let Some(plan) = self.plan_run(
                Buf::Quad,
                lo,
                hi,
                false,
                staged_len,
                layer,
                true,
                old_start,
                old_end,
            ) else {
                return false;
            };
            let (ulo, uhi) = splice_run(&mut self.quad_scratch, &plan, &self.stage_chrome);
            self.upload_range(Buf::Quad, ulo, uhi);
        }
        self.last_upload_work.scroll_regions += 1;
        true
    }

    /// Whether every fragment in list positions `lo..hi` of `b` was refreshed by the
    /// current region walk.
    fn run_refreshed(&self, b: Buf, lo: usize, hi: usize, stamp: u32) -> bool {
        let t = &self.fragments;
        t.order[b as usize]
            .get(lo..hi)
            .is_some_and(|run| run.iter().all(|&fi| t.frags[fi as usize].stamp == stamp))
    }

    /// Assigns fresh ranges to the run of list entries `lo..hi` of buffer `b` from
    /// the live counts the region walk recorded, and shifts everything drawn after
    /// the run if its total changed. With `pad_last` the final entry (the end marker)
    /// takes the run's spare capacity; the run keeps its size when the new content
    /// fits, and grows with headroom otherwise.
    #[allow(clippy::too_many_arguments)]
    fn plan_run(
        &mut self,
        b: Buf,
        lo: usize,
        hi: usize,
        pad_last: bool,
        staged_len: usize,
        layer: u32,
        chrome: bool,
        old_start: u32,
        old_end: u32,
    ) -> Option<RunPlan> {
        let old_total = old_end - old_start;
        let body = hi - lo - usize::from(pad_last);
        let delta;
        let total;
        {
            let FragmentTable { frags, order, .. } = &mut self.fragments;
            let entries = order[b as usize].get(lo..hi)?;
            let used: u32 = entries[..body]
                .iter()
                .map(|&e| frags[e as usize].live[b as usize])
                .sum();
            if used as usize != staged_len {
                return None;
            }
            total = if !pad_last {
                used
            } else if used <= old_total {
                old_total
            } else {
                used + used / 4 + 4
            };
            let mut cursor = old_start;
            for (k, &e) in entries.iter().enumerate() {
                let f = &mut frags[e as usize];
                if k < body {
                    let len = f.live[b as usize];
                    *f.range_mut(b) = cursor..cursor + len;
                    cursor += len;
                } else {
                    *f.range_mut(b) = cursor..old_start + total;
                    f.live[b as usize] = 0;
                }
            }
            delta = total as i64 - old_total as i64;
        }
        if delta != 0 {
            shift_after(&mut self.fragments, b, hi, delta);
            self.shift_boundaries(b, layer, chrome, delta);
        }
        Some(RunPlan {
            old_start,
            old_end,
            total,
        })
    }

    // --- full gather -----------------------------------------------------------

    fn new_frag(&mut self, node: Option<WidgetId>, offset: Point, clip: Rect) -> u32 {
        let t = &mut self.fragments;
        let fi = t.frags.len() as u32;
        t.frags.push(GpuFragment {
            offset,
            clip,
            ord: [NO_ORD; 3],
            layer: self.walk_layer,
            ..Default::default()
        });
        if let Some(id) = node {
            t.by_node.insert(id, fi);
        }
        fi
    }

    /// Appends fragment `fi` to buffer `b`'s draw-order list.
    fn list_push(&mut self, fi: u32, b: Buf) {
        let t = &mut self.fragments;
        t.frags[fi as usize].ord[b as usize] = t.order[b as usize].len() as u32;
        t.order[b as usize].push(fi);
    }

    /// Puts marker fragment `mi` at the walk's current position in all buffers.
    fn place_marker(&mut self, mi: u32, gather: bool) {
        let (q, g, i) = (
            self.quad_scratch.len() as u32,
            self.glyph_scratch.len() as u32,
            self.image_scratch.len() as u32,
        );
        let stamp = self.walk_stamp;
        let f = &mut self.fragments.frags[mi as usize];
        // A region walk writes into staging vectors; the retained ranges stay put
        // until the run is re-planned from the live counts.
        if gather {
            f.quads = q..q;
            f.glyphs = g..g;
            f.images = i..i;
        }
        f.live = [0; 3];
        f.stamp = stamp;
        if gather {
            for b in BUFS {
                self.list_push(mi, b);
            }
        }
    }

    /// Records the instances a node just appended (from `starts` to the current
    /// vector ends) as fragment `fi`. When gathering, each slot is padded to its
    /// capacity, takes the range and joins the draw-order lists named in `lists`; a
    /// region walk only records the live count.
    fn seal_fragment(
        &mut self,
        fi: u32,
        starts: [usize; 3],
        chrome: bool,
        lists: [bool; 3],
        gather: bool,
    ) {
        for b in BUFS {
            let start = starts[b as usize];
            let (end, live) = match b {
                Buf::Quad if chrome => seal_vec(&mut self.chrome_scratch, start, gather),
                Buf::Quad => seal_vec(&mut self.quad_scratch, start, gather),
                Buf::Glyph => seal_vec(&mut self.glyph_scratch, start, gather),
                Buf::Image => seal_vec(&mut self.image_scratch, start, gather),
            };
            let f = &mut self.fragments.frags[fi as usize];
            if gather {
                *f.range_mut(b) = start as u32..end as u32;
            }
            f.live[b as usize] = live;
            f.chrome = chrome;
            if gather && lists[b as usize] {
                self.list_push(fi, b);
            }
        }
    }

    /// Appends the pending scroll chrome after the layer's content and registers the
    /// chrome slots in draw order. Returns `(content_end, chrome_end)`.
    fn flush_chrome(&mut self) -> (u32, u32) {
        let content_end = self.quad_scratch.len() as u32;
        self.quad_scratch.extend_from_slice(&self.chrome_scratch);
        self.chrome_scratch.clear();
        let pending = std::mem::take(&mut self.chrome_pending);
        for &fi in &pending {
            let r = &mut self.fragments.frags[fi as usize].quads;
            r.start += content_end;
            r.end += content_end;
            self.list_push(fi, Buf::Quad);
        }
        self.chrome_pending = pending;
        self.chrome_pending.clear();
        (content_end, self.quad_scratch.len() as u32)
    }

    /// Walks the retained tree pre-order from the root, refilling the scratch instance
    /// buffers from each node's paint primitives (§3.2) and recording every node's
    /// slots. Cleared-and-refilled: no steady-state allocation once the scratch
    /// capacity is warm (§4.4).
    ///
    /// **Scroll composition (SOUL §3.2).** Each stack frame carries the accumulated
    /// `(offset, clip)`: the root starts unclipped at zero offset. Every emitted
    /// instance has `offset` added to its rect (or line endpoints) and carries `clip`.
    /// A [`WidgetKind::Scroll`] node shifts its children by `-scroll_offset(id)` and
    /// intersects the running clip with its own laid-out viewport rect, so scrolling
    /// re-composites the descendants without any relayout or per-node repaint.
    pub(crate) fn gather(&mut self, scene: &Scene) {
        self.quad_scratch.clear();
        self.chrome_scratch.clear();
        self.glyph_scratch.clear();
        self.image_scratch.clear();
        self.fragments.clear();
        self.prim_prefix.clear();
        self.chrome_pending.clear();
        self.walk_stack.clear();
        self.overlay_roots.clear();
        self.overlay_layers.clear();
        self.overlay_sig.clear();
        self.walk_layer = 0;
        self.walk_stamp = self.walk_stamp.wrapping_add(1);
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
        self.drain_walk(scene, true, WalkMode::Gather);
        let (content_end, chrome_end) = self.flush_chrome();
        self.base_chrome_start = content_end;
        self.base_quads = chrome_end;
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
        // An overlay nested in another (a dropdown popup inside a dialog) is a
        // layer of its own, drawn right after its parent's layer so content
        // later in the parent's tree order cannot cover it. The list grows as
        // nested roots are found, so this is a `while`, not a range.
        let mut i = 0;
        while i < self.overlay_roots.len() {
            let root = self.overlay_roots[i];
            self.overlay_sig.push((
                root.0,
                scene.overlay_level(root.0),
                scene.overlay_order(root.0),
            ));
            self.walk_layer = i as u32 + 1;
            let start = InstanceLayer {
                quad_start: self.quad_scratch.len() as u32,
                glyph_start: self.glyph_scratch.len() as u32,
                image_start: self.image_scratch.len() as u32,
                ..Default::default()
            };
            let found = self.overlay_roots.len();
            self.walk_stack.push((root.0, root.1, root.2, false));
            // Defer the overlays below this root, not the root itself.
            self.overlay_root_walk = Some(root.0);
            self.drain_walk(scene, true, WalkMode::Gather);
            self.overlay_root_walk = None;
            let (content_end, chrome_end) = self.flush_chrome();
            self.overlay_layers.push(InstanceLayer {
                quad_end: content_end,
                chrome_start: content_end,
                chrome_end,
                glyph_end: self.glyph_scratch.len() as u32,
                image_end: self.image_scratch.len() as u32,
                ..start
            });
            // Nested roots were found last-drawn-first; put them in tree order
            // directly after this layer, without allocating.
            let nested = self.overlay_roots.len() - found;
            self.overlay_roots[found..].reverse();
            self.overlay_roots[i + 1..].rotate_right(nested);
            i += 1;
        }
    }

    /// Drains [`Self::walk_stack`], emitting instances for every popped node and
    /// pushing its children: the shared body of the gather layers and the region
    /// re-walk (SOUL §3.2). With `defer_overlays`, an overlay-flagged node is parked
    /// in [`Self::overlay_roots`] (subtree untouched) instead of emitted. A region
    /// re-walk returns `false` if it meets an overlay or a node it has no fragment
    /// for, because the rebuilt run could then not match the retained layout.
    fn drain_walk(&mut self, scene: &Scene, defer_overlays: bool, mode: WalkMode) -> bool {
        let gather = mode == WalkMode::Gather;
        while let Some((id, offset, clip, after)) = self.walk_stack.pop() {
            if !scene.is_visible(id) {
                continue;
            }
            if !after && scene.is_overlay(id) && self.overlay_root_walk != Some(id) {
                if defer_overlays {
                    self.overlay_roots.push((id, offset, clip));
                    continue;
                }
                if !gather {
                    return false;
                }
            }
            let Some(node) = scene.node(id) else {
                continue;
            };
            let (offset, clip) = if after {
                (offset, clip)
            } else {
                let properties = scene.composite_properties(id);
                let offset = Point {
                    x: offset.x + properties.translation.x,
                    y: offset.y + properties.translation.y,
                };
                let clip = properties
                    .clip
                    .map_or(clip, |local| clip.intersect(&shifted(local, offset)));
                (offset, clip)
            };
            let is_scroll = node.kind == WidgetKind::Scroll;

            let fi = if after {
                match self.fragments.by_node.get(&id) {
                    Some(&fi) => fi,
                    None => return false,
                }
            } else if gather {
                self.new_frag(Some(id), offset, clip)
            } else {
                let stamp = self.walk_stamp;
                match self.fragments.by_node.get(&id) {
                    Some(&fi) => {
                        let f = &mut self.fragments.frags[fi as usize];
                        f.offset = offset;
                        f.clip = clip;
                        f.stamp = stamp;
                        fi
                    }
                    None => return false,
                }
            };

            if is_scroll {
                if !after {
                    // Bracket the content with marker fragments so the run can be
                    // replaced as one piece. The viewport's own primitives are
                    // fixed chrome (the optional scrollbar): an exit marker pushed
                    // before the children makes LIFO order emit them last, above
                    // content rather than beneath it.
                    let begin = if gather {
                        let m = self.new_frag(None, offset, clip);
                        self.fragments.frags[fi as usize].region = Some((m, u32::MAX));
                        m
                    } else {
                        match self.fragments.frags[fi as usize].region {
                            Some((m, _)) => m,
                            None => return false,
                        }
                    };
                    self.place_marker(begin, gather);
                    self.walk_stack.push((id, offset, clip, true));
                } else {
                    let end = if gather {
                        let m = self.new_frag(None, offset, clip);
                        if let Some((_, e)) = self.fragments.frags[fi as usize].region.as_mut() {
                            *e = m;
                        }
                        m
                    } else {
                        match self.fragments.frags[fi as usize].region {
                            Some((_, m)) => m,
                            None => return false,
                        }
                    };
                    self.place_marker(end, gather);
                    let starts = [
                        self.chrome_scratch.len(),
                        self.glyph_scratch.len(),
                        self.image_scratch.len(),
                    ];
                    if let Some(paint) = scene.paint(id) {
                        encode_prims(
                            &paint.primitives,
                            offset,
                            clip,
                            &mut self.chrome_scratch,
                            &mut self.glyph_scratch,
                            &mut self.image_scratch,
                        );
                    }
                    self.seal_fragment(fi, starts, true, [false, true, true], gather);
                    if gather {
                        self.chrome_pending.push(fi);
                    } else {
                        self.walk_exits += 1;
                    }
                    continue;
                }
            } else {
                let starts = [
                    self.quad_scratch.len(),
                    self.glyph_scratch.len(),
                    self.image_scratch.len(),
                ];
                if !leaf_culled(scene, id, node, offset, clip) {
                    if let Some(paint) = scene.paint(id) {
                        encode_prims(
                            &paint.primitives,
                            offset,
                            clip,
                            &mut self.quad_scratch,
                            &mut self.glyph_scratch,
                            &mut self.image_scratch,
                        );
                    }
                }
                self.seal_fragment(fi, starts, false, [true; 3], gather);
            }

            // Compute the child (offset, clip): a Scroll node shifts and clips its
            // descendants; every other node passes its state through unchanged (§3.2).
            let (child_offset, child_clip) = if is_scroll {
                scroll_child_ctx(scene, id, offset, clip)
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
        true
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

    /// Brings the GPU glyph atlas up to date with the CPU atlas (SOUL §3.2): a full
    /// upload when the texture is new, resized or belongs to another atlas, else only
    /// the rect written since this renderer last synced. Reads the atlas's write log
    /// instead of consuming a shared dirty marker, so several renderers (the headless
    /// one and a windowed one) each stay current. A steady-state no-op.
    pub(crate) fn sync_atlas(&mut self, atlas: &GlyphAtlas) {
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
        self.last_upload_work.bytes.other_bytes += aw as u64 * ah as u64;
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
        self.last_upload_work.bytes.other_bytes += (rw * rh) as u64;
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
            self.last_upload_work.bytes.other_bytes +=
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
        self.last_upload_work.bytes.other_bytes +=
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
