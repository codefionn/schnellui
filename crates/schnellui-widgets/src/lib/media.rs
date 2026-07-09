use super::*;

pub enum ImageSource {
    /// A named source with no pixel data attached (there is no asset loader yet) —
    /// paints the placeholder box, exactly as `Image::new` always has.
    Placeholder(Cow<'static, str>),
    /// Raw RGBA8 pixels, row-major, `width * height * 4` bytes — inserted into the
    /// scene's shared [`ImageAtlas`](schnellui_scene::ImageAtlas) at build and drawn
    /// as a real [`Primitive::ImageQuad`] (SOUL §3.2).
    Rgba {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    /// Immutable local shared RGBA payload. Virtualized cards can re-enter the
    /// scene without cloning a full decoded raster before atlas upload.
    SharedRgba {
        width: u32,
        height: u32,
        pixels: Rc<[u8]>,
    },
    /// A retained RGBA source keyed by an inexpensive producer revision. New
    /// pixels replace the existing atlas region without rebuilding the view.
    DynamicRgba {
        revision: Box<dyn FnMut() -> u64 + 'static>,
        frame: Box<dyn FnMut() -> Option<DynamicImageFrame> + 'static>,
    },
}

/// Shared pixels for one retained dynamic image frame.
#[derive(Clone)]
pub struct DynamicImageFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Rc<[u8]>,
}

impl DynamicImageFrame {
    pub fn new(width: u32, height: u32, pixels: impl Into<Rc<[u8]>>) -> Self {
        Self {
            width,
            height,
            pixels: pixels.into(),
        }
    }
}

pub(crate) struct DynamicImageState {
    pub(crate) revision: Box<dyn FnMut() -> u64 + 'static>,
    pub(crate) observed_revision: u64,
    pub(crate) frame: Option<Box<dyn FnMut() -> Option<DynamicImageFrame> + 'static>>,
    pub(crate) display: Size,
    pub(crate) texels: Option<TexelRect>,
}

/// An image leaf (SOUL §8.1). `Role::Image` with alt text shared by its hover
/// label and accessible name.
/// Pixel-backed via [`Image::from_rgba`] / [`Image::from_png`]; the plain
/// [`Image::new`] stays the placeholder box (no asset loader yet — honest).
pub struct Image {
    pub(crate) source: ImageSource,
    pub(crate) alt: Option<Cow<'static, str>>,
    /// display size in logical px; defaults to the pixel dimensions (so a 64×64
    /// bitmap covers 64×64 logical px), or 64×64 for the placeholder.
    pub(crate) display: Option<Size>,
}

impl Image {
    /// A placeholder image (named source, no pixels attached).
    pub fn new(source: impl Into<Cow<'static, str>>) -> Image {
        Image {
            source: ImageSource::Placeholder(source.into()),
            alt: None,
            display: None,
        }
    }

    /// A rasterized image from raw RGBA8 pixels (row-major, `width * height * 4`
    /// bytes). Short or empty pixel data falls back to the placeholder at build.
    pub fn from_rgba(width: u32, height: u32, pixels: impl Into<Vec<u8>>) -> Image {
        Image {
            source: ImageSource::Rgba {
                width,
                height,
                pixels: pixels.into(),
            },
            alt: None,
            display: None,
        }
    }

    /// A pixel-backed image borrowing an immutable local shared payload.
    pub fn from_shared_rgba(width: u32, height: u32, pixels: Rc<[u8]>) -> Image {
        Image {
            source: ImageSource::SharedRgba {
                width,
                height,
                pixels,
            },
            alt: None,
            display: None,
        }
    }

    /// Creates a retained image whose pixels are read only when `revision`
    /// changes. The producer may share its backing bytes with the embedder.
    pub fn dynamic_rgba_versioned(
        revision: impl FnMut() -> u64 + 'static,
        frame: impl FnMut() -> Option<DynamicImageFrame> + 'static,
    ) -> Image {
        Image {
            source: ImageSource::DynamicRgba {
                revision: Box::new(revision),
                frame: Box::new(frame),
            },
            alt: None,
            display: None,
        }
    }

    /// A rasterized image decoded from in-memory **PNG** bytes (any 8-bit color
    /// type; palette/gray expand to RGBA). Deterministic (SOUL §7.3): embed the
    /// bytes with `include_bytes!` rather than reading files at runtime.
    pub fn from_png(bytes: &[u8]) -> Result<Image, String> {
        let (width, height, pixels) = decode_png_rgba(bytes)?;
        Ok(Image::from_rgba(width, height, pixels))
    }

    /// Sets the image's hover text and accessible name (SOUL §6.1 — an image
    /// with meaning names itself). Leave unset for a decorative image.
    pub fn alt(mut self, alt: impl Into<Cow<'static, str>>) -> Image {
        self.alt = Some(alt.into());
        self
    }

    /// Overrides the display size in logical px (the bitmap scales to fit).
    pub fn size(mut self, width: f32, height: f32) -> Image {
        self.display = Some(Size { width, height });
        self
    }

    pub fn role(&self) -> Role {
        Role::Image
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Image
    }
}

/// Decodes PNG bytes to `(width, height, rgba8)` — palette, gray, and RGB inputs
/// are normalized to RGBA8 (SOUL §8.1).
pub fn decode_png_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("png header: {e}"))?;
    let buf_size = reader
        .output_buffer_size()
        .ok_or_else(|| "png dimensions overflow".to_string())?;
    let mut buf = vec![0u8; buf_size.max(1)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("png frame: {e}"))?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let n = (w as usize) * (h as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(n * 4);
            for px in buf.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 0xff]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(n * 4);
            for &g in &buf {
                out.extend_from_slice(&[g, g, g, 0xff]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity(n * 4);
            for px in buf.chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
            out
        }
        other => return Err(format!("unsupported png color type {other:?}")),
    };
    if rgba.len() < n * 4 {
        return Err("png pixel data shorter than dimensions imply".to_string());
    }
    Ok((w, h, rgba))
}

/// Inserts RGBA pixels into the scene image atlas and emits the node's
/// [`Primitive::ImageQuad`] at a local origin with the given **logical** display
/// size (SOUL §3.2). Returns `false` (so the caller falls back to the placeholder)
/// when the pixels don't fit the atlas.
pub fn emit_image_paint(
    scene: &mut Scene,
    id: WidgetId,
    width: u32,
    height: u32,
    pixels: &[u8],
    display: Size,
) -> bool {
    let Some(tex) = scene.images_mut().insert(width, height, pixels) else {
        return false;
    };
    push_image_quad(scene, id, tex, display, Color::WHITE);
    true
}

/// Reserves image-atlas space and emits the node's `ImageQuad` **before the
/// pixels exist** — the build half of the async image pipeline (SOUL §8.1).
/// Geometry and UVs are final immediately (the reserved region samples
/// transparent); the rasterized pixels land later through
/// `ImageAtlas::write_rect` ([`drain_svg_rasters`] / [`settle_svg_rasters`]).
/// Returns the reserved texel rect, or `None` when the atlas can never fit it
/// (the caller falls back to the placeholder).
pub fn emit_image_paint_deferred(
    scene: &mut Scene,
    id: WidgetId,
    width: u32,
    height: u32,
    display: Size,
) -> Option<TexelRect> {
    let tex = scene.images_mut().reserve(width, height)?;
    push_image_quad(scene, id, tex, display, Color::WHITE);
    Some(tex)
}

/// The shared tail of the sync/deferred image emits: one [`Primitive::ImageQuad`]
/// at a local origin with the given **logical** display size (SOUL §3.2).
pub fn push_image_quad(
    scene: &mut Scene,
    id: WidgetId,
    tex: TexelRect,
    display: Size,
    tint: Color,
) {
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    pd.primitives.push(Primitive::ImageQuad {
        rect: Rect::new(0.0, 0.0, display.width, display.height),
        atlas_uv: Rect::new(
            tex.x as f32,
            tex.y as f32,
            tex.width as f32,
            tex.height as f32,
        ),
        tint,
    });
}

impl View for Image {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Image, parent);
        let alt = this.alt.map(Cow::into_owned);
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::Image.as_u16();
            a.name = alt.clone();
        }
        let intrinsic = match &this.source {
            ImageSource::Rgba { width, height, .. }
            | ImageSource::SharedRgba { width, height, .. } => this.display.unwrap_or(Size {
                width: *width as f32,
                height: *height as f32,
            }),
            ImageSource::Placeholder(_) | ImageSource::DynamicRgba { .. } => {
                this.display.unwrap_or(Size {
                    width: 64.0,
                    height: 64.0,
                })
            }
        };
        let mut painted = false;
        match this.source {
            ImageSource::Rgba {
                width,
                height,
                pixels,
            } if width > 0 && height > 0 => {
                painted = emit_image_paint(ctx.scene, id, width, height, &pixels, intrinsic);
            }
            ImageSource::SharedRgba {
                width,
                height,
                pixels,
            } if width > 0 && height > 0 => {
                painted = emit_image_paint(ctx.scene, id, width, height, &pixels, intrinsic);
            }
            ImageSource::DynamicRgba {
                mut revision,
                mut frame,
            } => {
                let observed_revision = revision();
                let initial = frame();
                let texels = initial.as_ref().and_then(|initial| {
                    let texels = ctx.scene.images_mut().insert(
                        initial.width,
                        initial.height,
                        &initial.pixels,
                    )?;
                    push_image_quad(ctx.scene, id, texels, intrinsic, Color::WHITE);
                    painted = true;
                    Some(texels)
                });
                ctx.runtime.with(|runtime| {
                    runtime.borrow_mut().dynamic_images.insert(
                        id,
                        DynamicImageState {
                            revision,
                            observed_revision,
                            frame: Some(frame),
                            display: intrinsic,
                            texels,
                        },
                    );
                });
            }
            _ => {}
        }
        if !painted {
            emit_media_paint(&ctx.runtime, ctx.scene, id, intrinsic);
        }
        if let Some(alt) = alt.as_deref() {
            register_hover_tooltip(
                &ctx.runtime,
                ctx.scene,
                ctx.text,
                ctx.atlas,
                id,
                alt,
                intrinsic,
                ctx.scale,
            );
        }
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        id
    }
}

/// An icon leaf (SOUL §8.1). Decorative by default (`Role::Image` with no name), or
/// give it an accessible name to make it meaningful.
pub struct Icon {
    pub(crate) name: Cow<'static, str>,
}

impl Icon {
    pub fn new(name: impl Into<Cow<'static, str>>) -> Icon {
        Icon { name: name.into() }
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Icon
    }
}

impl View for Icon {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let _ = self.name;
        let id = ctx.scene.insert(WidgetKind::Icon, parent);
        // Decorative by default: Image role, no accessible name (SOUL §8.1).
        ctx.scene.a11y_mut(id).role = Role::Image.as_u16();
        let intrinsic = Size {
            width: 16.0,
            height: 16.0,
        };
        emit_media_paint(&ctx.runtime, ctx.scene, id, intrinsic);
        ctx.layout
            .set_measure(id, Box::new(move |_avail| intrinsic));
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_rgba_keeps_the_payload_allocation() {
        let pixels: Rc<[u8]> = Rc::from(vec![255; 16]);
        let expected = Rc::as_ptr(&pixels);
        let image = Image::from_shared_rgba(2, 2, Rc::clone(&pixels));
        let ImageSource::SharedRgba { pixels, .. } = image.source else {
            panic!("shared constructor must retain shared pixels");
        };
        assert_eq!(Rc::as_ptr(&pixels), expected);
    }
}
