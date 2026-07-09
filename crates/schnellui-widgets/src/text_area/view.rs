use super::*;

impl View for TextArea {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::TextArea, parent);
        let context_menu = this.context_menu.unwrap_or_else(ContextMenu::default_text);

        // Semantics (SOUL §6.1): editable multi-line text; the placeholder is
        // the accessible name, the value the accessible value.
        let a = ctx.scene.a11y_mut(id);
        a.role = Role::MultilineTextInput.as_u16();
        a.value = Some(this.value.clone());
        if !this.placeholder.is_empty() {
            a.name = Some(this.placeholder.to_string());
        }
        let mut acts = ActionFlags::default();
        acts.insert(ActionFlags::FOCUS);
        if !this.read_only {
            acts.insert(ActionFlags::SET_VALUE);
        }
        if !context_menu.is_empty() {
            acts.insert(ActionFlags::SHOW_CONTEXT_MENU);
        }
        a.actions = acts.0;
        crate::context_menu::register_context_menu(&ctx.runtime, id, context_menu);

        // Retained edit state (caret parked at the end, like TextInput).
        let end = this.value.len();
        ctx.runtime.with(|rt| {
            rt.borrow_mut().areas.insert(
                id,
                AreaState {
                    value: this.value,
                    placeholder: this.placeholder.to_string(),
                    caret: end,
                    anchor: end,
                    size_px: this.size_px,
                    scale: ctx.scale,
                    min_rows: this.min_rows,
                    wrap: this.wrap,
                    line_numbers: this.line_numbers,
                    read_only: this.read_only,
                    goal_x: None,
                    highlight: this.highlight,
                    content: Size::default(),
                    last_rect: None,
                    pointer_origin: None,
                    secondary: SmallVec::new(),
                },
            );
        });
        if let Some(i) = this.on_input {
            with_handlers(&ctx.runtime, id, |h| h.input = Some(i));
        }

        // Mount-time content for wrapping editors depends on available width;
        // provisional height reserves min_rows; measure will recompute with real width.
        if this.wrap != WrapMode::NoWrap {
            let line_h = {
                let phys = phys_size_px(this.size_px, ctx.scale);
                let inv = 1.0 / norm_scale(ctx.scale);
                shape_area_line(&mut ctx.text, "Ag", phys).height * inv
            };
            let h = this.min_rows.max(1) as f32 * line_h + 2.0 * AREA_PAD;
            ctx.runtime.with(|rt| {
                if let Some(st) = rt.borrow_mut().areas.get_mut(id) {
                    st.content = Size {
                        width: 0.0,
                        height: h,
                    };
                }
            });
        }
        // First paint computes the content size the measure closure reads.
        emit_text_area_paint(&ctx.runtime, ctx.scene, ctx.text, ctx.atlas, id);
        let measure_runtime = ctx.runtime.clone();
        ctx.layout.set_measure(
            id,
            Box::new(move |avail| {
                measure_runtime
                    .with(|rt| {
                        let rt = rt.borrow();
                        let st = rt.areas.get(id)?;
                        if st.wrap != WrapMode::NoWrap
                            && avail.width.is_finite()
                            && avail.width > 1.0
                        {
                            let wrap = st.wrap;
                            let scale = st.scale;
                            let size_px = st.size_px;
                            let min_rows = st.min_rows;
                            let value = st.value.clone();
                            let line_numbers = st.line_numbers;
                            drop(rt);
                            let phys = phys_size_px(size_px, scale);
                            let inv = 1.0 / norm_scale(scale);
                            let mut shaper = TextShaper::new();
                            let gutter_w = line_number_gutter_width(
                                &mut shaper,
                                line_numbers,
                                value.split('\n').count(),
                                phys,
                                inv,
                            );
                            let inner_w =
                                (avail.width - 2.0 * AREA_PAD - 2.0 * INPUT_BORDER_W - gutter_w)
                                    .max(40.0);
                            let line_h = shape_area_line(&mut shaper, "Ag", phys).height * inv;
                            let max_w_phys = Some(inner_w * norm_scale(scale));
                            let mut visual_rows = 0usize;
                            for line in value.split('\n') {
                                if line.is_empty() {
                                    visual_rows += 1;
                                } else {
                                    let shaped = shape_area_line_wrapped(
                                        &mut shaper,
                                        line,
                                        phys,
                                        wrap,
                                        max_w_phys,
                                    );
                                    visual_rows += shaped.lines.len().max(1);
                                }
                            }
                            visual_rows = visual_rows.max(min_rows as usize);
                            return Some(Size {
                                width: avail.width,
                                height: visual_rows as f32 * line_h + 2.0 * AREA_PAD,
                            });
                        }
                        Some(st.content)
                    })
                    .unwrap_or_default()
            }),
        );
        id
    }
}

// ---------------------------------------------------------------------------
// paint emission (cleared-and-refilled in place, SOUL §3.2, §4.4)
// ---------------------------------------------------------------------------

/// Emits a text area's full paint: border + surface, per-line selection
/// washes, the (optionally highlighted) mono lines — or the gray placeholder —
/// and the caret. Updates the retained content size (the measure source) and
/// returns it.
pub(crate) fn emit_text_area_paint(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
) -> Size {
    let Some((value, placeholder, selections, size_px, scale, min_rows, wrap, line_numbers)) =
        runtime.with(|rt| {
            let rt = rt.borrow();
            rt.areas.get(id).map(|st| {
                (
                    st.value.clone(),
                    st.placeholder.clone(),
                    selection_list(st.caret, st.anchor, &st.secondary),
                    st.size_px,
                    st.scale,
                    st.min_rows,
                    st.wrap,
                    st.line_numbers,
                )
            })
        })
    else {
        return Size::default();
    };
    let focused = scene
        .a11y(id)
        .map(|a| StateFlags(a.state).contains(StateFlags::FOCUSED))
        .unwrap_or(false);

    // Run the implementor's highlighter with no registry borrow held (§3.1).
    let hook = runtime.with(|rt| {
        rt.borrow_mut()
            .areas
            .get_mut(id)
            .and_then(|st| st.highlight.take())
    });
    let highlighted = hook.map(|mut f| {
        let lines = f(&value);
        runtime.with(|rt| {
            if let Some(st) = rt.borrow_mut().areas.get_mut(id) {
                st.highlight = Some(f);
            }
        });
        lines
    });

    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(size_px, scale);
    let wrapping = wrap != WrapMode::NoWrap;
    // Uniform mono line height (deterministic reference shape).
    let line_h = shape_area_line(shaper, "Ag", phys).height * inv;

    // Shape every raw line once. When wrapping, each logical line may become
    // multiple visual rows.
    let lines: Vec<&str> = value.split('\n').collect();
    let gutter_w = line_number_gutter_width(shaper, line_numbers, lines.len(), phys, inv);
    let mut line_shapes: Vec<schnellui_text::ShapedText> = Vec::with_capacity(lines.len());
    let max_w_phys = if wrapping {
        let laid_w = scene
            .layout(id)
            .map(|b| b.rect.width)
            .filter(|w| w.is_finite() && *w > 1.0);
        let avail_w = laid_w.unwrap_or_else(|| {
            // Before first layout use MIN_AREA_W as a conservative wrap width.
            MIN_AREA_W + 2.0 * AREA_PAD + 2.0 * INPUT_BORDER_W
        });
        let inner = (avail_w - 2.0 * AREA_PAD - 2.0 * INPUT_BORDER_W - gutter_w).max(40.0);
        Some(inner * norm_scale(scale))
    } else {
        None
    };
    let mut widest = 0.0f32;
    let mut total_visual_rows = 0usize;
    for l in &lines {
        let s = if wrapping {
            shape_area_line_wrapped(shaper, l, phys, wrap, max_w_phys)
        } else {
            shape_area_line(shaper, l, phys)
        };
        widest = widest.max(s.width * inv);
        total_visual_rows += s.lines.len().max(1);
        line_shapes.push(s);
    }
    let rows = if wrapping {
        total_visual_rows.max(min_rows as usize)
    } else {
        lines.len().max(min_rows as usize)
    };
    let content = if wrapping {
        Size {
            width: scene
                .layout(id)
                .map(|b| b.rect.width)
                .filter(|w| w.is_finite() && *w > 1.0)
                .unwrap_or(MIN_AREA_W + 2.0 * AREA_PAD + 2.0 * INPUT_BORDER_W),
            height: rows as f32 * line_h + 2.0 * AREA_PAD,
        }
    } else {
        Size {
            width: widest.max(MIN_AREA_W) + gutter_w + 2.0 * AREA_PAD,
            height: rows as f32 * line_h + 2.0 * AREA_PAD,
        }
    };
    runtime.with(|rt| {
        if let Some(st) = rt.borrow_mut().areas.get_mut(id) {
            st.content = content;
        }
    });

    let rect = node_rect(scene, id, content);
    runtime.with(|rt| {
        if let Some(st) = rt.borrow_mut().areas.get_mut(id) {
            st.last_rect = Some(rect);
        }
    });
    let origin = Point {
        x: rect.x + AREA_PAD + gutter_w,
        y: rect.y + AREA_PAD,
    };
    let th = crate::theme_for(runtime, id);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    // surface: border + inset background (same family as TextInput, §8.1) —
    // all from the ambient theme, like TextInput's own paint
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: if focused { th.accent } else { th.outline },
        corner_radius: 3.0,
    });
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(
            rect.x + INPUT_BORDER_W,
            rect.y + INPUT_BORDER_W,
            (rect.width - 2.0 * INPUT_BORDER_W).max(0.0),
            (rect.height - 2.0 * INPUT_BORDER_W).max(0.0),
        ),
        color: th.surface,
        corner_radius: 2.0,
    });

    // Source line numbers occupy a visual-only gutter. Wrapped continuation
    // rows deliberately remain unlabeled, like conventional code editors.
    if let Some(line_numbers) = line_numbers {
        let mut visual_row = 0usize;
        for (i, shaped) in line_shapes.iter().enumerate() {
            let number = line_numbers.start_line.saturating_add(i).to_string();
            let number_shape = shape_area_line(shaper, &number, phys);
            let number_w = number_shape.width * inv;
            crate::rasterize_and_push(
                pd,
                shaper,
                atlas,
                &number_shape,
                phys as u32,
                line_numbers.color,
                scale,
                Point {
                    x: origin.x - GUTTER_GAP - number_w,
                    y: origin.y + visual_row as f32 * line_h,
                },
            );
            visual_row += if wrapping {
                shaped.lines.len().max(1)
            } else {
                1
            };
        }
    }

    // per-line selection washes (before the glyphs)
    if focused {
        for selection in &selections {
            let (s0, s1) = selection.range();
            if s0 == s1 {
                continue;
            }
            let mut ls = 0usize;
            for (i, line) in lines.iter().enumerate() {
                let le = ls + line.len();
                let a = s0.max(ls);
                let b = s1.min(le);
                if a < b || (s0 <= le && s1 > le && a <= b) {
                    let shaped = &line_shapes[i];
                    let x0 = advance_before(shaped, a.saturating_sub(ls)) * inv;
                    let mut x1 = advance_before(shaped, b.saturating_sub(ls)) * inv;
                    // a selection running past this line's newline gets a marker
                    if s1 > le {
                        x1 += NEWLINE_SEL_W;
                    }
                    if x1 > x0 {
                        pd.primitives.push(Primitive::SolidRect {
                            rect: Rect::new(
                                origin.x + x0,
                                origin.y + i as f32 * line_h,
                                x1 - x0,
                                line_h,
                            ),
                            color: th.text_selection,
                            corner_radius: 0.0,
                        });
                    }
                }
                ls = le + 1;
            }
        }
    }

    // the text: highlighted spans when the hook's output lines up, else the
    // plain mono shapes (also the placeholder while empty). For wrapping,
    // each logical line's wrapped visual rows are emitted at stacked origins.
    if value.is_empty() && !placeholder.is_empty() {
        let ph = shaper.shape_with(
            &placeholder,
            &ShapeOptions::new(phys)
                .wrap(WrapMode::NoWrap)
                .face(FontFace::Mono),
        );
        crate::rasterize_and_push(
            pd,
            shaper,
            atlas,
            &ph,
            phys as u32,
            th.text_muted,
            scale,
            origin,
        );
    } else {
        // Track visual row offset for wrapping (each logical line can span multiple visual rows).
        let mut visual_row = 0usize;
        for (i, line) in lines.iter().enumerate() {
            let shaped = &line_shapes[i];
            let is_wrapped = wrapping && shaped.lines.len() > 1;
            if line.is_empty() {
                visual_row += 1;
                continue;
            }
            let spans = highlighted
                .as_ref()
                .and_then(|h| h.get(i))
                .filter(|spans| spans.iter().map(|s| s.text.len()).sum::<usize>() == line.len());
            if is_wrapped {
                // Wrapping: emit glyphs at absolute visual-row positions.
                // For wrapped lines we emit the whole (multi-row) shape at once at the
                // logical line's origin; glyph y's already include visual row offsets.
                let line_origin = Point {
                    x: origin.x,
                    y: origin.y + visual_row as f32 * line_h,
                };
                if let Some(spans) = spans {
                    let mut specs: SmallVec<[SpanSpec; 8]> = SmallVec::new();
                    for s in spans {
                        let ink = s.style.color.unwrap_or(th.text);
                        specs.push(SpanSpec {
                            len: s.text.len(),
                            face: FontFace::from_axes(s.style.bold, false, true),
                            color: [ink.r, ink.g, ink.b, ink.a],
                            underline: s.style.underline,
                            strikethrough: s.style.strike,
                        });
                    }
                    let rich = shaper.shape_spans(
                        line,
                        &specs,
                        &ShapeOptions::new(phys)
                            .max_width(max_w_phys)
                            .wrap(wrap)
                            .face(FontFace::Mono),
                    );
                    crate::rich::push_rich_glyphs(
                        pd,
                        shaper,
                        atlas,
                        &rich,
                        phys as u32,
                        scale,
                        line_origin,
                    );
                } else {
                    crate::rasterize_lines_and_push(
                        pd,
                        shaper,
                        atlas,
                        shaped,
                        phys as u32,
                        th.text,
                        scale,
                        line_origin,
                    );
                }
                visual_row += shaped.lines.len();
            } else {
                let line_origin = Point {
                    x: origin.x,
                    y: origin.y + visual_row as f32 * line_h,
                };
                if let Some(spans) = spans {
                    let mut specs: SmallVec<[SpanSpec; 8]> = SmallVec::new();
                    for s in spans {
                        let ink = s.style.color.unwrap_or(th.text);
                        specs.push(SpanSpec {
                            len: s.text.len(),
                            face: FontFace::from_axes(s.style.bold, false, true),
                            color: [ink.r, ink.g, ink.b, ink.a],
                            underline: s.style.underline,
                            strikethrough: s.style.strike,
                        });
                    }
                    let rich = shaper.shape_spans(
                        line,
                        &specs,
                        &ShapeOptions::new(phys).wrap(WrapMode::NoWrap),
                    );
                    crate::rich::push_rich_glyphs(
                        pd,
                        shaper,
                        atlas,
                        &rich,
                        phys as u32,
                        scale,
                        line_origin,
                    );
                } else {
                    crate::rasterize_and_push(
                        pd,
                        shaper,
                        atlas,
                        shaped,
                        phys as u32,
                        th.text,
                        scale,
                        line_origin,
                    );
                }
                visual_row += 1;
            }
        }
    }

    // the caret (collapsed selection only). Supports wrapped geometry.
    if focused {
        // Precompute visual row offsets for wrapping.
        let mut logical_visual_offsets: Vec<usize> = Vec::with_capacity(lines.len());
        let mut acc = 0usize;
        for s in &line_shapes {
            logical_visual_offsets.push(acc);
            acc += if wrapping { s.lines.len().max(1) } else { 1 };
        }
        for selection in &selections {
            if selection.caret != selection.anchor {
                continue;
            }
            let caret = selection.caret;
            let ls = value[..caret].rfind('\n').map(|p| p + 1).unwrap_or(0);
            let row = value[..caret].matches('\n').count();
            let shaped = &line_shapes[row.min(line_shapes.len().saturating_sub(1))];
            let rel = caret - ls;
            let (sub_row, x_rel) = if wrapping && shaped.lines.len() > 1 {
                caret_x_in_wrapped(shaped, rel, lines[row].len(), inv)
            } else {
                (0, advance_before(shaped, rel) * inv)
            };
            let visual_row = logical_visual_offsets[row] + sub_row;
            let x = origin.x + x_rel;
            let y0 = origin.y + visual_row as f32 * line_h;
            pd.primitives.push(Primitive::Line {
                from: Point { x, y: y0 },
                to: Point { x, y: y0 + line_h },
                width: CARET_W,
                color: th.text,
            });
        }
    }
    crate::reapply_focus_ring(runtime, scene, id);
    content
}

/// The post-layout re-emit pass for text areas (SOUL §8.1): when an edit grew
/// (or shrank) the measured box, the frame's relayout hands the node a new
/// rect — this pass re-emits the paint into it. Idempotent: an area whose
/// laid-out rect matches its last emission is skipped without a heap touch
/// (Directive #1/#3; the enclosing layout block itself only runs when
/// something is layout-dirty).
pub(crate) fn reemit_moved_areas(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    let ids: SmallVec<[WidgetId; 8]> = runtime.with(|rt| rt.borrow().areas.keys().collect());
    for id in ids {
        if scene.node(id).is_none() {
            continue;
        }
        let rect = match scene.layout(id) {
            Some(b) if !b.rect.is_empty() => b.rect,
            _ => continue,
        };
        let last = runtime.with(|rt| rt.borrow().areas.get(id).and_then(|st| st.last_rect));
        if last == Some(rect) {
            continue; // box unchanged ⇒ paint already anchored there
        }
        emit_text_area_paint(runtime, scene, shaper, atlas, id);
        scene.mark_dirty(id, DirtyFlags::PAINT);
    }
}

// ---------------------------------------------------------------------------
// dispatch hooks (called from `text_edit`'s single inbound path, SOUL §6.3)
// ---------------------------------------------------------------------------
