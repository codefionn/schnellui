use super::*;

struct SvgJob {
    doc: Arc<SvgDoc>,
    pw: u32,
    ph: u32,
    widget: WidgetId,
    rect: TexelRect,
    generation: u64,
    cache_key: Option<SvgRasterKey>,
    mask: bool,
    reply: Sender<SvgDone>,
}

/// Converts authored RGBA into a tintable alpha mask. Source alpha (including
/// anti-aliasing and two-tone opacity) remains intact.
fn make_alpha_mask(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[0] = 0xff;
        pixel[1] = 0xff;
        pixel[2] = 0xff;
    }
}

/// The process-wide raster pool: a shared job queue drained by a few worker
/// threads, each owning its own [`TextShaper`] (the same embedded deterministic
/// font as the app's pooled shaper, so off-thread pixels are identical —
/// SOUL §7.3). Spawned on first use; `None` when no worker could be spawned,
/// in which case callers rasterize synchronously (degraded, never broken).
fn raster_pool() -> Option<&'static Mutex<Sender<SvgJob>>> {
    static POOL: OnceLock<Option<Mutex<Sender<SvgJob>>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = channel::<SvgJob>();
        let rx = Arc::new(Mutex::new(rx));
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().min(4))
            .unwrap_or(2);
        let mut spawned = 0usize;
        for _ in 0..workers {
            let rx = Arc::clone(&rx);
            let ok = std::thread::Builder::new()
                .name("schnellui-svg-raster".into())
                .spawn(move || {
                    let mut shaper = TextShaper::new();
                    loop {
                        // The classic Mutex<Receiver> queue: one idle worker blocks
                        // in recv holding the lock; taking a job releases it so the
                        // next worker moves up. Channel closed ⇒ workers exit.
                        let job = match rx.lock() {
                            Ok(guard) => match guard.recv() {
                                Ok(j) => j,
                                Err(_) => return,
                            },
                            Err(_) => return,
                        };
                        // A raster panic must not wedge `settle` (the reply would
                        // never arrive): degrade to empty pixels (`write_rect`
                        // rejects them ⇒ the region stays transparent) and rebuild
                        // the possibly-poisoned shaper.
                        let pixels: Arc<[u8]> =
                            match std::panic::catch_unwind(AssertUnwindSafe(|| {
                                let mut pixels =
                                    rasterize_svg_with_text(&job.doc, job.pw, job.ph, &mut shaper);
                                if job.mask {
                                    make_alpha_mask(&mut pixels);
                                }
                                pixels
                            })) {
                                Ok(px) => px.into(),
                                Err(_) => {
                                    shaper = TextShaper::new();
                                    Arc::from([])
                                }
                            };
                        if let Some(key) = job.cache_key {
                            retain_raster(key, Arc::clone(&pixels));
                        }
                        let _ = job.reply.send(SvgDone {
                            widget: job.widget,
                            rect: job.rect,
                            generation: job.generation,
                            pixels,
                        });
                    }
                })
                .is_ok();
            if ok {
                spawned += 1;
            }
        }
        (spawned > 0).then(|| Mutex::new(tx))
    })
    .as_ref()
}

/// Submits one parsed document for async rasterization into `rect`, stamped
/// with the current mount generation (a completion that outlives its tree is
/// dropped, never written into a reused [`WidgetId`]'s rect). Returns the doc
/// back when the pool is unavailable so the caller can rasterize synchronously.
fn submit_svg_raster(
    runtime: &crate::Runtime,
    doc: Arc<SvgDoc>,
    pw: u32,
    ph: u32,
    widget: WidgetId,
    rect: TexelRect,
    cache_key: Option<SvgRasterKey>,
    mask: bool,
) -> Result<(), Arc<SvgDoc>> {
    let Some(pool) = raster_pool() else {
        return Err(doc);
    };
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let generation = rt.svg_generation;
        let reply = rt.svg_reply.get_or_insert_with(channel).0.clone();
        let job = SvgJob {
            doc,
            pw,
            ph,
            widget,
            rect,
            generation,
            cache_key,
            mask,
            reply,
        };
        let sender = match pool.lock() {
            Ok(s) => s,
            Err(_) => return Err(job.doc),
        };
        match sender.send(job) {
            Ok(()) => {
                drop(sender);
                rt.svg_pending += 1;
                Ok(())
            }
            Err(e) => Err(e.0.doc),
        }
    })
}

/// Lands one completion: drops it when stale (a prior mount's generation, or a
/// widget no longer in the scene), otherwise writes the reserved rect (revision
/// bump ⇒ the renderer re-uploads, SOUL §3.2) and flags the widget paint-dirty.
fn land_svg_done(scene: &mut Scene, rt: &mut crate::WidgetRuntime, done: SvgDone) -> bool {
    rt.svg_pending = rt.svg_pending.saturating_sub(1);
    if done.generation != rt.svg_generation || scene.node(done.widget).is_none() {
        return false;
    }
    if !scene.images_mut().write_rect(done.rect, &done.pixels) {
        return false;
    }
    scene.mark_dirty(done.widget, DirtyFlags::PAINT);
    true
}

/// Drains every **finished** async rasterization into the scene's image atlas
/// (SOUL §8.1) without blocking — the windowed per-frame pull half of the
/// pipeline. The steady state (nothing pending) is one app-owned counter
/// check: no lock, no allocation, so the zero-alloc re-render covenant holds
/// (SOUL §4.1). Returns how many landed.
pub fn drain_svg_rasters(runtime: &crate::Runtime, scene: &mut Scene) -> usize {
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        if rt.svg_pending == 0 {
            return 0;
        }
        // Take the mailbox out while landing (landing needs `&mut rt`); moved
        // back below — moves only, no allocation.
        let Some(reply) = rt.svg_reply.take() else {
            return 0;
        };
        let mut landed = 0;
        while let Ok(done) = reply.1.try_recv() {
            if land_svg_done(scene, &mut rt, done) {
                landed += 1;
            }
        }
        rt.svg_reply = Some(reply);
        landed
    })
}

/// The number of async rasterizations still in flight for this thread — the
/// windowed loop keeps requesting redraws while this is nonzero, so completed
/// pixels reach the screen without any waker plumbing.
pub fn pending_svg_rasters(runtime: &crate::Runtime) -> usize {
    runtime.with(|rt| rt.borrow().svg_pending)
}

/// Blocks until every in-flight rasterization has landed, then returns how many
/// did. The **headless one-shot** path calls this before reading pixels back, so
/// the single deterministic frame contains every image (SOUL §7.3) — off-thread
/// rasterization changes *when* the pixels are computed, never *what* they are.
/// A 10s watchdog guards against a lost worker: on timeout the affected regions
/// stay transparent (degraded visibly, never a hang).
pub fn settle_svg_rasters(runtime: &crate::Runtime, scene: &mut Scene) -> usize {
    enum Step {
        Idle,
        Got(Box<SvgDone>),
        TimedOut(usize),
    }
    let mut landed = drain_svg_rasters(runtime, scene);
    loop {
        let step = runtime.with(|rt| {
            // A shared borrow held across the blocking recv is safe: this thread
            // does nothing else meanwhile, and workers never touch the UI runtime.
            let rt = rt.borrow();
            if rt.svg_pending == 0 {
                return Step::Idle;
            }
            let Some((_, rx)) = rt.svg_reply.as_ref() else {
                return Step::Idle;
            };
            match rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(d) => Step::Got(Box::new(d)),
                Err(_) => Step::TimedOut(rt.svg_pending),
            }
        });
        match step {
            Step::Idle => break,
            Step::TimedOut(n) => {
                eprintln!("schnellui: svg raster settle timed out with {n} in flight");
                break;
            }
            Step::Got(d) => runtime.with(|rt| {
                if land_svg_done(scene, &mut rt.borrow_mut(), *d) {
                    landed += 1;
                }
            }),
        }
    }
    landed
}

// ---------------------------------------------------------------------------
// the widget (SOUL §8.1 — draws pixels, carries Role::Image)
// ---------------------------------------------------------------------------

/// A vector-image leaf (SOUL §8.1): parses the SVG subset once at build,
/// reserves its rect in the scene's shared image atlas and emits one
/// [`Primitive::ImageQuad`] immediately, then rasterizes at the physical pixel
/// scale **off-thread** (the raster pool — text runs shaped through a worker's
/// deterministic shaper). The pixels land via [`drain_svg_rasters`] /
/// [`settle_svg_rasters`]. `Role::Image`; give meaningful graphics hover text
/// and an accessible name via [`Svg::alt`] (SOUL §6.1). Unparseable markup
/// falls back to the placeholder box — degraded visibly, never a panic.
///
/// [`Primitive::ImageQuad`]: schnellui_scene::Primitive::ImageQuad
pub struct Svg {
    pub(crate) markup: Cow<'static, str>,
    pub(crate) alt: Option<Cow<'static, str>>,
    pub(crate) display: Option<Size>,
    pub(crate) cache_key: Option<SvgCacheKey>,
    pub(crate) tint: Color,
    pub(crate) mask: bool,
    pub(crate) theme_tint: bool,
}

impl Svg {
    /// A vector image from SVG-subset markup (see the module docs for scope).
    pub fn new(markup: impl Into<Cow<'static, str>>) -> Svg {
        Svg {
            markup: markup.into(),
            alt: None,
            display: None,
            cache_key: None,
            tint: Color::WHITE,
            mask: false,
            theme_tint: false,
        }
    }
    /// Enables the reusable parsed-document, raster, and scene-atlas caches.
    ///
    /// The key must remain unique for the SVG content for the lifetime of the
    /// process. Icon-library adapters normally set it from library/name/style.
    pub fn cache(mut self, key: SvgCacheKey) -> Svg {
        self.cache_key = Some(key);
        self
    }
    /// Multiplies sampled SVG pixels by a draw-time color.
    ///
    /// Tint does not affect raster identity, so differently colored instances
    /// keep sharing the same CPU and GPU cache entries.
    pub fn tint(mut self, tint: Color) -> Svg {
        self.tint = tint;
        self.theme_tint = false;
        self
    }
    /// Treats the SVG as a monochrome icon whose draw-time color follows the
    /// subtree theme's [`Theme::text`](crate::Theme::text) token.
    ///
    /// Raster identity remains color-independent, so cached icons are reused
    /// across light/dark switches and nested [`ThemeProvider`](crate::ThemeProvider)s.
    pub fn themed(mut self) -> Svg {
        self.mask = true;
        self.theme_tint = true;
        self
    }
    /// Rasterizes the SVG as a white alpha mask for draw-time recoloring.
    ///
    /// Source alpha remains intact, preserving anti-aliasing and two-tone
    /// opacity. Ordinary colored SVGs keep their authored RGBA by default.
    pub fn mask(mut self) -> Svg {
        self.mask = true;
        self
    }
    /// Sets the vector image's hover text and accessible name (SOUL §6.1).
    /// Decorative icons may stay unnamed.
    pub fn alt(mut self, alt: impl Into<Cow<'static, str>>) -> Svg {
        self.alt = Some(alt.into());
        self
    }
    /// Overrides the display size in logical px (defaults to the viewBox size).
    pub fn size(mut self, width: f32, height: f32) -> Svg {
        self.display = Some(Size { width, height });
        self
    }
    /// Overrides the display width only (keeps the viewBox aspect for the height).
    pub fn width(mut self, width: f32) -> Svg {
        let h = self.display.map(|s| s.height).unwrap_or(0.0);
        self.display = Some(Size { width, height: h });
        self
    }
    /// Overrides the display height only.
    pub fn height(mut self, height: f32) -> Svg {
        let w = self.display.map(|s| s.width).unwrap_or(0.0);
        self.display = Some(Size { width: w, height });
        self
    }
    pub fn role(&self) -> Role {
        Role::Image
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Image
    }
}

impl View for Svg {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Image, parent);
        let alt = this.alt.map(Cow::into_owned);
        let tint = if this.theme_tint {
            crate::theme(&ctx.runtime).text
        } else {
            this.tint
        };
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::Image.as_u16();
            a.name = alt.clone();
        }
        let parsed = match &this.cache_key {
            Some(key) => parse_svg_cached(key, &this.markup),
            None => parse_svg(&this.markup).map(Arc::new),
        };
        let intrinsic = match (&parsed, this.display) {
            (Ok(doc), Some(mut s)) => {
                if s.width <= 0.0 && s.height > 0.0 {
                    s.width = s.height * doc.width / doc.height.max(f32::EPSILON);
                }
                if s.height <= 0.0 && s.width > 0.0 {
                    s.height = s.width * doc.height / doc.width.max(f32::EPSILON);
                }
                s
            }
            (Ok(doc), None) => Size {
                width: doc.width,
                height: doc.height,
            },
            (Err(_), maybe) => maybe.unwrap_or(Size {
                width: 24.0,
                height: 24.0,
            }),
        };
        let painted = match parsed {
            Ok(doc) if intrinsic.width > 0.0 && intrinsic.height > 0.0 => {
                // rasterize at physical pixels so the icon is crisp under --scale
                let scale = crate::norm_scale(ctx.scale);
                let pw = ((intrinsic.width * scale).round() as u32).max(1);
                let ph = ((intrinsic.height * scale).round() as u32).max(1);
                // The async pipeline (SOUL §8.1): reserve the atlas rect and emit
                // the quad now — geometry and UVs are final, the region samples
                // transparent — then rasterize off-thread. The pixels land via
                // [`drain_svg_rasters`] (windowed, per frame) or
                // [`settle_svg_rasters`] (headless, before the one-shot readback).
                let raster_key = this.cache_key.clone().map(|source| SvgRasterKey {
                    source,
                    width: pw,
                    height: ph,
                    mask: this.mask,
                });
                let image_key = this
                    .cache_key
                    .as_ref()
                    .map(|key| key.image_key(pw, ph, this.mask));

                // Scene hit: reuse the exact resident atlas allocation. This is
                // also the in-flight dedup path: the first instance owns the
                // raster job while later instances can already point at its
                // transparent reserved region.
                if let Some(rect) = image_key
                    .as_ref()
                    .and_then(|key| ctx.scene.images().cached(key))
                {
                    crate::push_image_quad(ctx.scene, id, rect, intrinsic, tint);
                    true
                } else if let (Some(_), Some(pixels)) = (
                    raster_key.as_ref(),
                    raster_key.as_ref().and_then(cached_raster),
                ) {
                    // Process-wide CPU hit (for example after remount): pack once
                    // into this scene, with no parse or raster job.
                    let Some(image_key) = image_key else {
                        unreachable!("a raster cache key always has an image key");
                    };
                    match ctx
                        .scene
                        .images_mut()
                        .insert_cached(image_key, pw, ph, &pixels)
                    {
                        Some((rect, _)) => {
                            crate::push_image_quad(ctx.scene, id, rect, intrinsic, tint);
                            true
                        }
                        None => false,
                    }
                } else {
                    // Cache miss (or an ordinary uncached Svg): reserve final UVs,
                    // submit one off-thread raster, and tint only at draw time.
                    let reserved = match image_key {
                        Some(key) => ctx
                            .scene
                            .images_mut()
                            .reserve_cached(key, pw, ph)
                            .map(|(rect, _)| rect),
                        None => ctx.scene.images_mut().reserve(pw, ph),
                    };
                    match reserved {
                        Some(rect) => {
                            crate::push_image_quad(ctx.scene, id, rect, intrinsic, tint);
                            if let Err(doc) = submit_svg_raster(
                                &ctx.runtime,
                                doc,
                                pw,
                                ph,
                                id,
                                rect,
                                raster_key.clone(),
                                this.mask,
                            ) {
                                // no worker pool: synchronous fallback
                                let mut pixels = rasterize_svg_with_text(&doc, pw, ph, ctx.text);
                                if this.mask {
                                    make_alpha_mask(&mut pixels);
                                }
                                let pixels: Arc<[u8]> = pixels.into();
                                if let Some(key) = raster_key {
                                    retain_raster(key, Arc::clone(&pixels));
                                }
                                ctx.scene.images_mut().write_rect(rect, &pixels);
                            }
                            true
                        }
                        None => false,
                    }
                }
            }
            _ => false,
        };
        if !painted {
            emit_media_paint(&ctx.runtime, ctx.scene, id, intrinsic);
        }
        if let Some(alt) = alt.as_deref() {
            crate::register_hover_tooltip(
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
