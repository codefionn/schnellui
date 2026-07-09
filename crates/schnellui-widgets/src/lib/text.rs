use super::*;

pub struct Text {
    pub(crate) content: TextContent,
    pub(crate) size_px: f32,
    pub(crate) role: Role,
    /// line-break policy (SOUL §8.1). Default [`WrapMode::NoWrap`] — today's
    /// single-line behavior, bit-for-bit unchanged when unset.
    pub(crate) wrap: WrapMode,
    /// per-line horizontal alignment (SOUL §8.1). Default [`TextAlign::Start`].
    pub(crate) align: TextAlign,
    /// single-line ellipsis truncation to the available width (SOUL §8.1).
    pub(crate) ellipsis: bool,
}

/// The two text-slot flavors (SOUL §3.3).
pub enum TextContent {
    /// invariant text — part of the hoisted skeleton.
    Static(Cow<'static, str>),
    /// reactive text — re-evaluated on tracked change, node mutated in place.
    Dynamic(TextFn),
}

impl Text {
    /// A static text leaf (SOUL §3.3 static slot).
    pub fn new(text: impl Into<Cow<'static, str>>) -> Text {
        Text {
            content: TextContent::Static(text.into()),
            size_px: 16.0,
            role: Role::Label,
            wrap: WrapMode::NoWrap,
            align: TextAlign::Start,
            ellipsis: false,
        }
    }

    /// A signal-bound dynamic text leaf (SOUL §3.3 dynamic slot).
    pub fn dynamic(f: impl FnMut() -> String + 'static) -> Text {
        Text {
            content: TextContent::Dynamic(Box::new(f)),
            size_px: 16.0,
            role: Role::Label,
            wrap: WrapMode::NoWrap,
            align: TextAlign::Start,
            ellipsis: false,
        }
    }

    /// Sets the font size in pixels.
    pub fn size(mut self, size_px: f32) -> Text {
        self.size_px = size_px;
        self
    }

    /// Sets the line-break policy (SOUL §8.1). A wrapping text's height depends on
    /// its available width, so the layout pass measures it width-aware and re-wraps
    /// on resize; the config persists across `Text::dynamic` re-emits.
    pub fn wrap(mut self, wrap: WrapMode) -> Text {
        self.wrap = wrap;
        self
    }

    /// Sets the per-line horizontal alignment within the line box (SOUL §8.1).
    pub fn align(mut self, align: TextAlign) -> Text {
        self.align = align;
        self
    }

    /// Truncates to a **single line** with a trailing ellipsis when the text does not
    /// fit the available width (SOUL §8.1). Implies a width-aware, single-line layout.
    pub fn ellipsis(mut self) -> Text {
        self.ellipsis = true;
        self
    }

    /// Overrides the accessible role (e.g. `Role::Status` for a live value, §7.5).
    pub fn role(mut self, role: Role) -> Text {
        self.role = role;
        self
    }

    /// `true` if this is a dynamic (reactive) slot (SOUL §3.3).
    pub fn is_dynamic(&self) -> bool {
        matches!(self.content, TextContent::Dynamic(_))
    }

    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Text
    }
}

/// Writes a text string into the a11y column as the node's accessible surface
/// (SOUL §6.1): a live `Status` announces it as *value*, everything else as *name*.
pub fn write_text_semantics(scene: &mut Scene, id: WidgetId, role: Role, text: &str) {
    let a = scene.a11y_mut(id);
    a.role = role.as_u16();
    if role == Role::Status {
        a.value = Some(text.to_string());
    } else {
        a.name = Some(text.to_string());
    }
}

impl View for Text {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Text, parent);
        let size_px = this.size_px;
        let role = this.role;
        let scale = ctx.scale;
        // A non-default line-break policy, alignment, or ellipsis makes the leaf's
        // size width-dependent (SOUL §8.1): its paint is deferred to the post-layout
        // `emit_wrapped_paint` and its height is measured width-aware. The default
        // (NoWrap + Start, no ellipsis) keeps the legacy single-line build-time path,
        // bit-for-bit unchanged.
        let wrapped =
            this.wrap != WrapMode::NoWrap || this.align != TextAlign::Start || this.ellipsis;
        let wrap = this.wrap;
        let align = this.align;
        let ellipsis = this.ellipsis;
        match this.content {
            TextContent::Static(s) => {
                let text: String = s.into_owned();
                write_text_semantics(ctx.scene, id, role, &text);
                if wrapped {
                    // Fill the parent width so the measure pass gets a definite wrap
                    // width; register the deferred-paint config. No build-time glyphs
                    // and no fixed measure — both are handled width-aware post-layout.
                    ctx.layout.set_fill_width(id);
                    register_text_layout(
                        &ctx.runtime,
                        id,
                        wrap,
                        align,
                        ellipsis,
                        size_px,
                        scale,
                        text,
                    );
                } else {
                    // Shape once → real glyph quads + the glyph-exact intrinsic size.
                    let sz = emit_text_paint(
                        ctx.scene,
                        ctx.text,
                        ctx.atlas,
                        id,
                        &text,
                        size_px,
                        theme_for(&ctx.runtime, id).text,
                        scale,
                    );
                    // Intrinsic measure returns the shaped size (SOUL §8.1) — invariant.
                    ctx.layout.set_measure(id, Box::new(move |_avail| sz));
                }
            }
            TextContent::Dynamic(mut f) => {
                // First synchronous render creates the node's initial content
                // (SOUL §3.3 — the dynamic slot runs once on build) while the
                // retained subscription records precisely the signals it reads.
                let initial = ctx.runtime.track_dynamic_initial(id, &mut f);
                write_text_semantics(ctx.scene, id, role, &initial);
                if wrapped {
                    // Deferred, width-aware paint + measure (as the static case), but
                    // the text is re-produced on signal change: the slot rewrites the
                    // TextLayout text + flags LAYOUT so the frame re-wraps and re-emits.
                    ctx.layout.set_fill_width(id);
                    register_text_layout(
                        &ctx.runtime,
                        id,
                        wrap,
                        align,
                        ellipsis,
                        size_px,
                        scale,
                        initial.clone(),
                    );
                    ctx.runtime.with(|rt| {
                        rt.borrow_mut().slots.insert(
                            id,
                            DynSlot {
                                f: Some(f),
                                last: initial,
                                shared: Rc::new(RefCell::new(Size {
                                    width: 0.0,
                                    height: 0.0,
                                })),
                                size_px,
                                role,
                                scale,
                                wrapped: true,
                            },
                        );
                    });
                } else {
                    let sz = emit_text_paint(
                        ctx.scene,
                        ctx.text,
                        ctx.atlas,
                        id,
                        &initial,
                        size_px,
                        theme_for(&ctx.runtime, id).text,
                        scale,
                    );
                    // Measure reads the cached shaped size, shared with the render slot
                    // so a text change re-measures (the layout-dirty channel, §8.1).
                    let shared = Rc::new(RefCell::new(sz));
                    let sh = shared.clone();
                    ctx.layout
                        .set_measure(id, Box::new(move |_avail| *sh.borrow()));
                    // Register the render effect (interim bridge to create_effect, §3.3).
                    ctx.runtime.with(|rt| {
                        rt.borrow_mut().slots.insert(
                            id,
                            DynSlot {
                                f: Some(f),
                                last: initial,
                                shared,
                                size_px,
                                role,
                                scale,
                                wrapped: false,
                            },
                        );
                    });
                }
            }
        }
        id
    }
}

/// Inserts a [`TextLayout`] into the app-owned runtime for a wrapping/aligned/
/// ellipsis text leaf (SOUL §8.1). `dirty` starts `true` so the first post-layout
/// `emit_wrapped_paint` emits its glyphs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn register_text_layout(
    runtime: &Runtime,
    id: WidgetId,
    wrap: WrapMode,
    align: TextAlign,
    ellipsis: bool,
    size_px: f32,
    scale: f32,
    text: String,
) {
    runtime.with(|rt| {
        rt.borrow_mut().text_layouts.insert(
            id,
            TextLayout {
                wrap,
                align,
                ellipsis,
                size_px,
                scale,
                text,
                cache: SmallVec::new(),
                last_emit: None,
                dirty: true,
            },
        );
    });
}
