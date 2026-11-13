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
        if this.required {
            let mut state = StateFlags::default();
            state.insert(StateFlags::REQUIRED);
            a.state = state.0;
        }
        crate::widgets::context_menu::register_context_menu(&ctx.runtime, id, context_menu);

        // Retained edit state (caret parked at the end, like TextInput).
        let end = this.value.len();
        crate::widgets::note_authored(&ctx.runtime, id, crate::widgets::text_bits(&this.value));
        let cache = AreaCache::new(&this.value);
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
                    cache,
                    history: EditHistory::default(),
                    compose: None,
                    preedit_box: None,
                },
            );
        });
        if let Some(i) = this.on_input {
            with_handlers(&ctx.runtime, id, |h| h.input = Some(i));
        }

        // Mount-time content for wrapping editors depends on available width;
        // provisional height reserves min_rows; measure will recompute with real width.
        if this.wrap != WrapMode::NoWrap {
            ctx.layout.set_width_aware(id);
            let line_h = {
                let phys = phys_size_px(this.size_px, ctx.scale);
                let inv = 1.0 / norm_scale(ctx.scale);
                shape_area_line(ctx.text, "Ag", phys).height * inv
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
                // The frame's width-aware hook (`measure_text`) normally answers
                // first with the app's shaper; this fallback serves bare
                // `LayoutEngine::compute` callers that thread no hook.
                measure_wrapped_area(&measure_runtime, id, avail, &mut TextShaper::new())
                    .or_else(|| {
                        measure_runtime.with(|rt| rt.borrow().areas.get(id).map(|st| st.content))
                    })
                    .unwrap_or_default()
            }),
        );
        id
    }
}

/// Width-aware measure of a wrapping text area: counts visual rows at the
/// offered width. `None` when `id` is not a text area, does not wrap, or the
/// width is indefinite (the retained content size then applies). The lines are
/// shaped into the area's line table, so the paint emitted at the same width
/// reuses these shapes.
pub(crate) fn measure_wrapped_area(
    runtime: &crate::widgets::Runtime,
    id: WidgetId,
    avail: Size,
    shaper: &mut TextShaper,
) -> Option<Size> {
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let st = rt.areas.get_mut(id)?;
        if st.wrap == WrapMode::NoWrap || !avail.width.is_finite() || avail.width <= 1.0 {
            return None;
        }
        let phys = phys_size_px(st.size_px, st.scale);
        let inv = 1.0 / norm_scale(st.scale);
        let line_count = st.cache.starts.len();
        let gutter_w = st.gutter_width(shaper, line_count);
        let inner_w = (avail.width - 2.0 * AREA_PAD - 2.0 * INPUT_BORDER_W - gutter_w).max(40.0);
        let line_h = st.line_height(shaper);
        let max_w_phys = Some(inner_w * norm_scale(st.scale));
        let mut visual_rows = if st.cache.usable() {
            let key = ShapeKey {
                phys,
                wrap: st.wrap,
                max_w: max_w_phys,
            };
            st.cache.set_shape_key(key);
            st.cache.shape_lines(&st.value, shaper, inv);
            st.cache
                .lines
                .iter()
                .map(|l| l.rows as usize)
                .sum::<usize>()
        } else {
            let mut rows = 0usize;
            for line in st.value.split('\n') {
                if line.is_empty() {
                    rows += 1;
                } else {
                    let shaped = shape_area_line_wrapped(shaper, line, phys, st.wrap, max_w_phys);
                    rows += shaped.lines.len().max(1);
                }
            }
            rows
        };
        visual_rows = visual_rows.max(st.min_rows as usize);
        Some(Size {
            width: avail.width,
            height: visual_rows as f32 * line_h + 2.0 * AREA_PAD,
        })
    })
}

// ---------------------------------------------------------------------------
// paint emission (retained, spliced in place, SOUL §3.2, §4.4)
// ---------------------------------------------------------------------------

/// What the line sync did to the line table, for the paint splice.
#[derive(Default)]
struct Plan {
    /// every line is new: rebuild the paint
    full: bool,
    window: Option<WindowPlan>,
}

/// One contiguous run of lines replaced by an edit.
struct WindowPlan {
    /// first replaced line (new numbering)
    first: usize,
    /// the lines now at `first..fresh_end`
    fresh_end: usize,
    /// text-block primitives the replaced old lines owned
    removed_prims: usize,
    /// visual rows the replaced old lines occupied
    old_rows: SmallVec<[u32; 8]>,
    /// widest of the replaced old lines, logical px
    removed_widest: f32,
}

/// Brings `cache.lines` in line with the edited value: reuses the lines the
/// edit did not touch (matched by hash from both ends of the edit window) and
/// creates fresh, unshaped entries for the rest.
fn sync_lines(cache: &mut AreaCache, value: &str) -> Plan {
    let mut plan = Plan::default();
    let pending = cache.pending.take();
    let n = cache.starts.len();
    let hash_of = |cache: &AreaCache, i: usize| {
        hash_bytes(&value.as_bytes()[cache.starts[i]..cache.line_end(i, value.len())])
    };
    let rebuild = |cache: &mut AreaCache| {
        cache.lines.clear();
        for i in 0..n {
            let h = hash_of(cache, i);
            cache.lines.push(LineInfo::new(h));
        }
        cache.owned = 0..0;
        cache.shape_dirty = true;
        Plan {
            full: true,
            window: None,
        }
    };
    if matches!(pending, Some(Pending::All)) || cache.lines.is_empty() {
        return rebuild(cache);
    }
    let Some(Pending::Window {
        first,
        old_last,
        new_last,
    }) = pending
    else {
        if cache.lines.len() != n {
            return rebuild(cache);
        }
        return plan;
    };
    let m = new_last - first + 1;
    let old_count = old_last - first + 1;
    let hashes: SmallVec<[u64; 8]> = (0..m).map(|j| hash_of(cache, first + j)).collect();
    let mut pre = 0;
    while pre < m.min(old_count) && hashes[pre] == cache.lines[first + pre].hash {
        pre += 1;
    }
    let mut suf = 0;
    while suf < (m - pre).min(old_count - pre)
        && hashes[m - 1 - suf] == cache.lines[old_last - suf].hash
    {
        suf += 1;
    }
    let remove = first + pre..old_last + 1 - suf;
    let removed_prims = cache.lines[remove.clone()]
        .iter()
        .map(|l| l.text_prims as usize)
        .sum();
    let old_rows = cache.lines[remove.clone()].iter().map(|l| l.rows).collect();
    let removed_widest = cache.lines[remove.clone()]
        .iter()
        .fold(0.0f32, |w, l| w.max(l.width));
    let fresh = hashes[pre..m - suf].iter().map(|&h| LineInfo::new(h));
    cache.lines.splice(remove.clone(), fresh);
    if cache.lines.len() != n {
        return rebuild(cache);
    }
    cache.owned = renumber_owned(cache.owned.clone(), remove.clone(), m - pre - suf);
    cache.shape_dirty = true;
    plan.window = Some(WindowPlan {
        first: remove.start,
        fresh_end: remove.start + (m - pre - suf),
        removed_prims,
        old_rows,
        removed_widest,
    });
    plan
}

/// The lines that may own paint after old lines `removed` were replaced by
/// `fresh` new ones (which own nothing yet): the old range in the new
/// numbering, minus the removed lines.
fn renumber_owned(
    owned: std::ops::Range<usize>,
    removed: std::ops::Range<usize>,
    fresh: usize,
) -> std::ops::Range<usize> {
    if owned.is_empty() {
        return 0..0;
    }
    let map = |x: usize| {
        if x >= removed.end {
            x - removed.len() + fresh
        } else {
            x.min(removed.start)
        }
    };
    map(owned.start)..map(owned.end)
}

/// Includes line `i` in the lines that may own paint.
fn own(cache: &mut AreaCache, i: usize) {
    cache.owned = if cache.owned.is_empty() {
        i..i + 1
    } else {
        cache.owned.start.min(i)..cache.owned.end.max(i + 1)
    };
}

/// Shapes the lines `range` (an edit's fresh lines) when every other line's
/// shape is current; [`AreaCache::shape_lines`] would scan the whole table.
fn shape_window(
    cache: &mut AreaCache,
    range: std::ops::Range<usize>,
    value: &str,
    shaper: &mut TextShaper,
    inv: f32,
) {
    let Some(key) = cache.shape_key else { return };
    let wrapping = key.wrap != WrapMode::NoWrap;
    for i in range {
        let (s, e) = (cache.starts[i], cache.line_end(i, value.len()));
        let line = &value[s..e];
        let shaped = if wrapping {
            shape_area_line_wrapped(shaper, line, key.phys, key.wrap, key.max_w)
        } else {
            shape_area_line(shaper, line, key.phys)
        };
        let info = &mut cache.lines[i];
        info.rows = if wrapping {
            shaped.lines.len().max(1) as u32
        } else {
            1
        };
        info.width = shaped.width * inv;
        info.shape = Some(Box::new(shaped));
        info.epoch = cache.epoch;
    }
    cache.shape_dirty = false;
}

/// The highlight spans of line `i`, when they concatenate back to the line.
fn effective_spans(hl: &[Vec<RichSpan>], i: usize, line_len: usize) -> Option<&[RichSpan]> {
    hl.get(i)
        .filter(|spans| spans.iter().map(|s| s.text.len()).sum::<usize>() == line_len)
        .map(|spans| spans.as_slice())
}

/// What a line's emitted primitives depend on: its text and its spans.
fn line_sig(hash: u64, spans: Option<&[RichSpan]>) -> u64 {
    let Some(spans) = spans else { return hash };
    let mut h = mix(hash, 0x5eed);
    for span in spans {
        let style = &span.style;
        let flags =
            u64::from(style.bold) | u64::from(style.underline) << 1 | u64::from(style.strike) << 2;
        let color = style.color.map_or(0, |c| {
            1 << 32
                | u64::from(c.r) << 24
                | u64::from(c.g) << 16
                | u64::from(c.b) << 8
                | u64::from(c.a)
        });
        h = mix(mix(mix(h, span.text.len() as u64), flags), color);
    }
    h
}

fn span_specs(spans: &[RichSpan], text: Color) -> SmallVec<[SpanSpec; 8]> {
    spans
        .iter()
        .map(|s| {
            let ink = s.style.color.unwrap_or(text);
            SpanSpec {
                len: s.text.len(),
                face: FontFace::from_axes(s.style.bold, false, true),
                color: [ink.r, ink.g, ink.b, ink.a],
                underline: s.style.underline,
                strikethrough: s.style.strike,
                tag: 0,
            }
        })
        .collect()
}

/// Appends one source line's glyph primitives at visual row `row`. Wrapped
/// lines emit their whole multi-row shape at once; the glyph y's already
/// include the visual row offsets.
#[allow(clippy::too_many_arguments)]
fn emit_line(
    pd: &mut PaintData,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    m: &Metrics,
    line: &str,
    shaped: &ShapedText,
    spans: Option<&[RichSpan]>,
    row: usize,
) {
    if line.is_empty() {
        return;
    }
    let line_origin = Point {
        x: m.origin.x,
        y: m.origin.y + row as f32 * m.line_h,
    };
    let is_wrapped = m.wrapping && shaped.lines.len() > 1;
    if let Some(spans) = spans {
        let specs = span_specs(spans, m.text);
        note_shape();
        let rich = if is_wrapped {
            shaper.shape_spans(
                line,
                &specs,
                &ShapeOptions::new(m.phys)
                    .max_width(m.max_w)
                    .wrap(m.wrap)
                    .face(FontFace::Mono)
                    .line_height(Some(super::AREA_LINE_HEIGHT)),
            )
        } else {
            shaper.shape_spans(
                line,
                &specs,
                &ShapeOptions::new(m.phys)
                    .wrap(WrapMode::NoWrap)
                    .line_height(Some(super::AREA_LINE_HEIGHT)),
            )
        };
        crate::widgets::rich::push_rich_glyphs(
            pd,
            shaper,
            atlas,
            &rich,
            m.phys as u32,
            m.scale,
            line_origin,
        );
    } else if is_wrapped {
        crate::widgets::rasterize_lines_and_push(
            pd,
            shaper,
            atlas,
            shaped,
            m.phys as u32,
            m.text,
            m.scale,
            line_origin,
        );
    } else {
        crate::widgets::rasterize_and_push(
            pd,
            shaper,
            atlas,
            shaped,
            m.phys as u32,
            m.text,
            m.scale,
            line_origin,
        );
    }
}

/// Shapes the gutter labels for lines `0..count` that are not cached yet.
fn ensure_labels(
    cache: &mut AreaCache,
    shaper: &mut TextShaper,
    line_numbers: LineNumbers,
    count: usize,
    phys: f32,
) {
    if cache.labels_key != Some((line_numbers.start_line, phys)) {
        cache.labels.clear();
        cache.labels_key = Some((line_numbers.start_line, phys));
    }
    while cache.labels.len() < count {
        let number = line_numbers.start_line.saturating_add(cache.labels.len());
        cache
            .labels
            .push(shape_area_line(shaper, &number.to_string(), phys));
    }
}

/// Appends the gutter label primitives for `lines`, recording each label's
/// primitive count in `counts`.
#[allow(clippy::too_many_arguments)]
fn emit_labels(
    pd: &mut PaintData,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    m: &Metrics,
    cache: &AreaCache,
    color: Color,
    lines: std::ops::Range<usize>,
    counts: &mut Vec<u32>,
) {
    for i in lines {
        let label = &cache.labels[i];
        let before = pd.primitives.len();
        crate::widgets::rasterize_and_push(
            pd,
            shaper,
            atlas,
            label,
            m.phys as u32,
            color,
            m.scale,
            Point {
                x: m.origin.x - GUTTER_GAP - label.width * m.inv,
                y: m.origin.y + cache.row_of(i, m.wrapping) as f32 * m.line_h,
            },
        );
        counts.push((pd.primitives.len() - before) as u32);
    }
}

/// Per-line selection washes for every selection (drawn before the glyphs).
fn build_washes(
    out: &mut Vec<Primitive>,
    cache: &AreaCache,
    value_len: usize,
    selections: &[TextSelection],
    m: &Metrics,
    color: Color,
) {
    for selection in selections {
        let (s0, s1) = selection.range();
        if s0 == s1 {
            continue;
        }
        let visible = cache.band_lines(m.band, m.wrapping, cache.lines.len());
        let from = cache.line_of(s0).max(visible.start);
        let to = cache.line_of(s1).min(visible.end.saturating_sub(1));
        if visible.is_empty() {
            continue;
        }
        for i in from..=to {
            let ls = cache.starts[i];
            let le = cache.line_end(i, value_len);
            let a = s0.max(ls);
            let b = s1.min(le);
            if !(a < b || (s0 <= le && s1 > le && a <= b)) {
                continue;
            }
            let Some(shaped) = cache.lines[i].shape.as_deref() else {
                continue;
            };
            let (a, b, len) = (a - ls, b - ls, le - ls);
            let row = cache.row_of(i, m.wrapping);
            let mut wash = |sub: usize, x0: f32, mut x1: f32, last: bool| {
                // a selection running past this line's newline gets a marker
                if last && s1 > le {
                    x1 += NEWLINE_SEL_W;
                }
                if x1 > x0 {
                    out.push(Primitive::SolidRect {
                        rect: Rect::new(
                            m.origin.x + x0,
                            m.origin.y + (row + sub) as f32 * m.line_h,
                            x1 - x0,
                            m.line_h,
                        ),
                        color,
                        corner_radius: 0.0,
                    });
                }
            };
            if !(m.wrapping && shaped.lines.len() > 1) {
                wash(
                    0,
                    advance_before(shaped, a) * m.inv,
                    advance_before(shaped, b) * m.inv,
                    true,
                );
                continue;
            }
            // A wrapped line gets one wash per row the selection touches.
            for sub in 0..shaped.lines.len() {
                let (start, end, glyphs) = super::wrapped_row(shaped, sub, len);
                let last = sub + 1 == shaped.lines.len();
                let (lo, hi) = (a.max(start), if last { b } else { b.min(end) });
                if lo > hi || (lo == hi && !(last && s1 > le)) {
                    continue;
                }
                let before = |byte: usize| -> f32 {
                    glyphs
                        .iter()
                        .filter(|g| (g.cluster as usize) < byte)
                        .map(|g| g.x_advance)
                        .sum()
                };
                let x = shaped.lines[sub].x;
                wash(
                    sub,
                    (x + before(lo)) * m.inv,
                    (x + before(hi)) * m.inv,
                    last,
                );
            }
        }
    }
}

/// The caret lines (collapsed selections only). Supports wrapped geometry.
fn build_carets(
    out: &mut SmallVec<[Primitive; 5]>,
    cache: &AreaCache,
    value_len: usize,
    selections: &[TextSelection],
    m: &Metrics,
    color: Color,
) {
    for selection in selections {
        if selection.caret != selection.anchor {
            continue;
        }
        let caret = selection.caret;
        let row = cache.line_of(caret);
        let ls = cache.starts[row];
        let Some(shaped) = cache.lines[row].shape.as_deref() else {
            continue;
        };
        let rel = caret - ls;
        let (sub_row, x_rel) = if m.wrapping && shaped.lines.len() > 1 {
            caret_x_in_wrapped(shaped, rel, cache.line_end(row, value_len) - ls, m.inv)
        } else {
            (0, advance_before(shaped, rel) * m.inv)
        };
        let visual_row = cache.row_of(row, m.wrapping) + sub_row;
        let x = m.origin.x + x_rel;
        let y0 = m.origin.y + visual_row as f32 * m.line_h;
        out.push(Primitive::Line {
            from: Point { x, y: y0 },
            to: Point {
                x,
                y: y0 + m.line_h,
            },
            width: CARET_W,
            color,
        });
    }
}

/// A composition's marks: an underline along the composed text on every row it
/// covers, a thicker segment under the input method's cursor range, and the
/// box of the cursor (where the input method's candidate window belongs).
/// Composition frames allocate scratch; frames without one never get here.
#[allow(clippy::too_many_arguments)]
fn build_preedit_marks(
    out: &mut SmallVec<[Primitive; 5]>,
    cache: &AreaCache,
    value_len: usize,
    lo: usize,
    len: usize,
    cursor: Option<(usize, usize)>,
    m: &Metrics,
    color: Color,
) -> Option<Rect> {
    let rules = |out: &mut SmallVec<[Primitive; 5]>, from: usize, to: usize, w: f32| {
        let mut rects = Vec::new();
        build_washes(
            &mut rects,
            cache,
            value_len,
            &[TextSelection {
                caret: to,
                anchor: from,
            }],
            m,
            color,
        );
        // The rule sits on a whole device pixel, so it stays crisp.
        let device = norm_scale(m.scale);
        for p in rects {
            if let Primitive::SolidRect { rect, color, .. } = p {
                let bottom = ((rect.y + rect.height) * device).round() / device;
                out.push(Primitive::SolidRect {
                    rect: Rect::new(rect.x, bottom - w, rect.width, w),
                    color,
                    corner_radius: 0.0,
                });
            }
        }
    };
    rules(out, lo, lo + len, PREEDIT_RULE_W);
    if let Some((a, b)) = cursor.filter(|(a, b)| a < b) {
        rules(out, lo + a, lo + b, PREEDIT_CURSOR_RULE_W);
    }
    let at = lo + cursor.map_or(0, |(a, _)| a);
    let mut probe: SmallVec<[Primitive; 5]> = SmallVec::new();
    build_carets(
        &mut probe,
        cache,
        value_len,
        &[TextSelection {
            caret: at,
            anchor: at,
        }],
        m,
        color,
    );
    probe.iter().find_map(|p| match p {
        Primitive::Line {
            from, to, width, ..
        } => Some(Rect::new(from.x, from.y, *width, to.y - from.y)),
        _ => None,
    })
}

type Bounds = [f32; 4];

/// Union of the bounds of the primitives that reach outside `rect`, if any.
fn overflows(rect: &Rect, prims: &[Primitive]) -> Option<Bounds> {
    const EPS: f32 = 0.01;
    let mut out: Option<Bounds> = None;
    for prim in prims {
        let (x0, y0, x1, y1) = match *prim {
            Primitive::SolidRect { rect, .. }
            | Primitive::StrokeRect { rect, .. }
            | Primitive::GlyphQuad { rect, .. }
            | Primitive::ImageQuad { rect, .. } => {
                (rect.x, rect.y, rect.x + rect.width, rect.y + rect.height)
            }
            Primitive::Line {
                from, to, width, ..
            } => (
                from.x.min(to.x) - width * 0.5,
                from.y.min(to.y) - width * 0.5,
                from.x.max(to.x) + width * 0.5,
                from.y.max(to.y) + width * 0.5,
            ),
        };
        if x0 < rect.x - EPS
            || y0 < rect.y - EPS
            || x1 > rect.x + rect.width + EPS
            || y1 > rect.y + rect.height + EPS
        {
            out = Some(match out {
                Some([a, b, c, d]) => [a.min(x0), b.min(y0), c.max(x1), d.max(y1)],
                None => [x0, y0, x1, y1],
            });
        }
    }
    out
}

/// Union of two optional bounds.
fn union_bounds(a: Option<Bounds>, b: Option<Bounds>) -> Option<Bounds> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Moves already-emitted primitives down by `dy` (lines below an edit that
/// added or removed visual rows).
fn shift_y(prims: &mut [Primitive], dy: f32) {
    for prim in prims {
        match prim {
            Primitive::SolidRect { rect, .. }
            | Primitive::StrokeRect { rect, .. }
            | Primitive::GlyphQuad { rect, .. }
            | Primitive::ImageQuad { rect, .. } => rect.y += dy,
            Primitive::Line { from, to, .. } => {
                from.y += dy;
                to.y += dy;
            }
        }
    }
}

/// Union of what the lines paint outside the surface rect (only owned lines
/// paint anything).
fn owned_overflow(cache: &AreaCache) -> Option<Bounds> {
    let n = cache.lines.len();
    cache.lines[cache.owned.start.min(n)..cache.owned.end.min(n)]
        .iter()
        .fold(None, |acc, l| union_bounds(acc, l.overflow))
}

#[cfg(test)]
thread_local! {
    /// Primitives the paint emission wrote or moved, so tests can prove an
    /// edit's cost does not grow with the lines after it.
    pub(super) static TOUCHED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts primitives written or moved; free outside tests.
#[inline]
fn note_touched(_n: usize) {
    #[cfg(test)]
    TOUCHED.with(|c| c.set(c.get() + _n as u64));
}

/// `prims.splice(range, with)`, counting the primitives it writes and, when the
/// count changes, the tail it moves.
fn splice_prims(
    prims: &mut Vec<Primitive>,
    range: std::ops::Range<usize>,
    with: impl ExactSizeIterator<Item = Primitive>,
) {
    let moved = if with.len() == range.len() {
        0
    } else {
        prims.len() - range.end
    };
    note_touched(with.len() + moved);
    prims.splice(range, with);
}

/// Applies a primitive-count change at or before `at` to the section ends.
fn grow(r: &mut PrimRanges, from: Section, delta: isize) {
    let add = |v: &mut usize| *v = (*v as isize + delta) as usize;
    if matches!(from, Section::Gutter) {
        add(&mut r.gutter_end);
    }
    if matches!(from, Section::Gutter | Section::Wash) {
        add(&mut r.wash_end);
    }
    if matches!(from, Section::Gutter | Section::Wash | Section::Text) {
        add(&mut r.text_end);
    }
    add(&mut r.caret_end);
}

#[derive(Clone, Copy)]
enum Section {
    Gutter,
    Wash,
    Text,
}

/// The surface: border + inset background (same family as TextInput, §8.1),
/// all from the ambient theme, like TextInput's own paint.
fn surface(rect: Rect, focused: bool, invalid: bool, th: &crate::widgets::Theme) -> [Primitive; 2] {
    [
        Primitive::SolidRect {
            rect,
            // A failed validation outranks focus; the focus ring still shows on top.
            color: if invalid {
                th.invalid_color()
            } else if focused {
                th.accent
            } else {
                th.outline
            },
            corner_radius: 3.0,
        },
        Primitive::SolidRect {
            rect: Rect::new(
                rect.x + INPUT_BORDER_W,
                rect.y + INPUT_BORDER_W,
                (rect.width - 2.0 * INPUT_BORDER_W).max(0.0),
                (rect.height - 2.0 * INPUT_BORDER_W).max(0.0),
            ),
            color: th.surface,
            corner_radius: 2.0,
        },
    ]
}

/// Where the surface's top edge goes. A field in a Scroll keeps its frame on
/// the viewport while the text moves under it, as a browser textarea's border
/// does: once the viewport has scrolled past the area's top, the top edge
/// follows the viewport's. The sides and the bottom stay where they are.
fn pinned_top(scene: &Scene, id: WidgetId, rect: Rect) -> f32 {
    let mut cur = scene.node(id).and_then(|n| n.parent);
    while let Some(p) = cur {
        let Some(node) = scene.node(p) else { break };
        if node.kind == WidgetKind::Scroll {
            let Some(view) = scene.layout(p).map(|l| l.rect).filter(|r| !r.is_empty()) else {
                break;
            };
            let top = view.y + scene.scroll_offset(p).y;
            return if top.is_finite() {
                // never past the bottom edge, so the box keeps some height
                top.clamp(
                    rect.y,
                    (rect.y + rect.height - 2.0 * INPUT_BORDER_W).max(rect.y),
                )
            } else {
                rect.y
            };
        }
        cur = node.parent;
    }
    rect.y
}

/// `rect` with its top edge moved down to `top`.
fn framed(rect: Rect, top: f32) -> Rect {
    Rect::new(
        rect.x,
        top,
        rect.width,
        (rect.y + rect.height - top).max(0.0),
    )
}

/// Whether the paint still has the layout `ranges` describes.
fn ranges_valid(prims: &[Primitive], r: &PrimRanges) -> bool {
    prims.len() >= r.caret_end
        && 2 <= r.gutter_end
        && r.gutter_end <= r.wash_end
        && r.wash_end <= r.text_end
        && r.text_end <= r.caret_end
        && prims[..2]
            .iter()
            .all(|p| matches!(p, Primitive::SolidRect { .. }))
        && prims[r.text_end..r.caret_end]
            .iter()
            .all(|p| matches!(p, Primitive::Line { width, .. } if *width == CARET_W))
}

/// The visual rows `[lo, hi)` of an area that can be on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Visible {
    /// what the nearest Scroll ancestor's viewport shows
    Scroll(usize, usize),
    /// what the host window shows; `step` rows is one window height
    Window { lo: usize, hi: usize, step: usize },
}

/// The visual rows of an area (whose first row starts at `origin_y`) that can
/// be on screen: those its nearest Scroll ancestor currently shows, or without
/// one, those inside the host `window` once the composited translation and
/// clips apply (rows below the window never show). `None` without a laid-out
/// Scroll ancestor or a known window. Scrolling and compositing move no
/// layout, so this is read at emit time and on every frame by
/// [`reemit_scrolled_areas`].
fn visible_rows(
    scene: &Scene,
    id: WidgetId,
    origin_y: f32,
    line_h: f32,
    window: Option<Size>,
) -> Option<Visible> {
    if !(line_h.is_finite() && line_h > 0.0) {
        return None;
    }
    let rows = |top: f32, bottom: f32| {
        let lo = ((top - origin_y) / line_h).floor().max(0.0);
        let hi = ((bottom - origin_y) / line_h).ceil().max(0.0);
        if !(lo.is_finite() && hi.is_finite()) {
            return None;
        }
        Some((lo as usize, (hi as usize).max(lo as usize)))
    };
    let plain = |n: WidgetId| scene.composite_properties(n) == Default::default();
    let mut composited = !plain(id);
    let mut cur = scene.node(id)?.parent;
    while let Some(p) = cur {
        let node = scene.node(p)?;
        if node.kind == WidgetKind::Scroll {
            let viewport = scene.layout(p)?.rect;
            if viewport.is_empty() {
                return None;
            }
            let top = viewport.y + scene.scroll_offset(p).y;
            let (lo, hi) = rows(top, top + viewport.height)?;
            return Some(Visible::Scroll(lo, hi));
        }
        composited |= !plain(p);
        cur = node.parent;
    }
    let window = window.filter(|w| w.width > 0.0 && w.height > 0.0)?;
    // Window-space clip, mapped back into the area's own (layout) space. With
    // no compositing properties on the chain there is no offset and no clip.
    let (top, bottom) = if composited {
        let (offset, clip) = scene.composite_context(id);
        (
            clip.y.max(0.0) - offset.y,
            (clip.y + clip.height).min(window.height) - offset.y,
        )
    } else {
        (0.0, window.height)
    };
    let (lo, hi) = rows(top, bottom.max(top))?;
    let step = (window.height / line_h).ceil();
    let step = if step.is_finite() { step as usize } else { 0 };
    Some(Visible::Window {
        lo,
        hi,
        step: step.max(8),
    })
}

/// The band to emit for what shows. In a Scroll: the previous band while it
/// still covers the viewport, otherwise the viewport plus a viewport of
/// overscan on each side (so a slow scroll re-emits once per viewport, not
/// once per frame). In the window: the two window heights from the window
/// step at or above the top row, a function of the visible rows alone, so a
/// resized or slid area paints like a fresh one.
fn pick_band(visible: Option<Visible>, prev: Option<(usize, usize)>) -> Option<(usize, usize)> {
    match visible? {
        Visible::Scroll(lo, hi) => {
            if prev.is_some_and(|b| b.0 <= lo && hi <= b.1) {
                return prev;
            }
            let overscan = (hi - lo).max(8);
            Some((lo.saturating_sub(overscan), hi.saturating_add(overscan)))
        }
        // `hi - lo` is at most `step + 1` rows and `lo < q + step`, so the band
        // covers them; it moves only when the top row crosses a step.
        Visible::Window { lo, hi, step } => {
            let q = lo / step * step;
            Some((q, (q + 2 * step).max(hi)))
        }
    }
}

/// Interaction decorations that index into the node's primitives (recolored
/// glyphs, an inserted background) would be displaced by a splice; the border
/// strokes the runtime appends at the end are not. A decoration on an ancestor
/// (a hovered ListItem or TableRow) recolors its whole subtree, so what counts
/// is whether any recorded index is owned by this node, not who the target is.
pub(super) fn has_indexed_decoration(runtime: &crate::widgets::Runtime, id: WidgetId) -> bool {
    runtime.with(|rt| {
        let rt = rt.borrow();
        rt.hover_tooltips.get(id).is_some()
            || [&rt.hover, &rt.active, &rt.ring]
                .into_iter()
                .flatten()
                .any(|d| {
                    (d.target == id && (d.background.is_some() || !d.foreground.is_empty()))
                        || d.foreground.iter().any(|r| r.owner == id)
                        || d.background.is_some_and(|(owner, _)| owner == id)
                })
    })
}

/// Emits a text area's paint: border + surface, the line-number gutter,
/// per-line selection washes, the (optionally highlighted) mono lines (or the
/// gray placeholder) and the carets. Updates the retained content size (the
/// measure source) and returns it.
///
/// The paint is retained in sections (`[surface][gutter][washes][text][carets]`)
/// and spliced in place: a caret or selection change rewrites only the washes
/// and carets, an edit re-emits only the lines it changed (below them the rows
/// slide when the edit added or removed any), and everything else, including
/// every line's shape, comes from the line table.
pub(crate) fn emit_text_area_paint(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
) -> Size {
    let taken = runtime.with(|rt| rt.borrow_mut().areas.remove(id));
    let Some(mut st) = taken else {
        return Size::default();
    };
    let (content, restyle) = emit_area(&mut st, runtime, scene, shaper, atlas, id);
    st.cache.restyles += u64::from(restyle != Restyle::None);
    runtime.with(|rt| {
        rt.borrow_mut().areas.insert(id, st);
    });
    match restyle {
        Restyle::None => {}
        // Spliced paint keeps every indexed decoration (an area that has
        // one is always rebuilt), so only the strokes move with the box.
        Restyle::Moved => crate::widgets::reapply_focus_ring(runtime, scene, id),
        // A rebuilt paint dropped every decoration primitive. Their records
        // point at whatever now sits at those indices.
        Restyle::Rebuilt => crate::widgets::redecorate_after_refill(runtime, scene, id),
    }
    content
}

/// How [`emit_area`] changed the paint the interaction decorations sit on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Restyle {
    /// Nothing the decorations depend on moved.
    None,
    /// Spliced in place, but the box moved or rows shifted.
    Moved,
    /// Cleared and refilled from scratch.
    Rebuilt,
}

/// Returns the content size and how the interaction decorations must be
/// re-applied.
fn emit_area(
    st: &mut AreaState,
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
) -> (Size, Restyle) {
    let (focused, invalid) = scene.a11y(id).map_or((false, false), |a| {
        let state = StateFlags(a.state);
        (
            state.contains(StateFlags::FOCUSED),
            state.contains(StateFlags::INVALID),
        )
    });
    let th = crate::widgets::theme_for(runtime, id);
    let scale = st.scale;
    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(st.size_px, scale);
    let wrap = st.wrap;
    let wrapping = wrap != WrapMode::NoWrap;
    let line_numbers = st.line_numbers;
    let placeholder_mode = st.value.is_empty() && !st.placeholder.is_empty();
    if st.cache.starts.is_empty() {
        st.cache.rebuild_starts(&st.value);
    }
    let line_count = st.cache.starts.len();
    // Uniform mono line height (deterministic reference shape).
    let line_h = st.line_height(shaper);
    let gutter_w = st.gutter_width(shaper, line_count);
    let laid_w = scene
        .layout(id)
        .map(|b| b.rect.width)
        .filter(|w| w.is_finite() && *w > 1.0);
    let avail_w = laid_w.unwrap_or(MIN_AREA_W + 2.0 * AREA_PAD + 2.0 * INPUT_BORDER_W);
    let max_w = wrapping.then(|| {
        let inner = (avail_w - 2.0 * AREA_PAD - 2.0 * INPUT_BORDER_W - gutter_w).max(40.0);
        inner * norm_scale(scale)
    });

    // Bring the line table in line with the value and shape what is new.
    let cache = &mut st.cache;
    let shape = ShapeKey { phys, wrap, max_w };
    // Every shape is current and keyed like the last emission's, so an edit
    // leaves only its fresh lines to shape and the widest line to update.
    let settled = !cache.shape_dirty && cache.key.is_some_and(|k| k.shape == shape);
    cache.set_shape_key(shape);
    let settled = settled && !cache.shape_dirty;
    let plan = sync_lines(cache, &st.value);
    let lines_changed = plan.full || plan.window.is_some() || cache.shape_dirty;
    match plan.window.as_ref().filter(|_| settled) {
        Some(w) => shape_window(cache, w.first..w.fresh_end, &st.value, shaper, inv),
        None => {
            if cache.shape_dirty {
                cache.rows_stale = true;
            }
            cache.shape_lines(&st.value, shaper, inv);
        }
    }
    // The row prefix moves only when the edit changed how many rows its lines take.
    let window_rows_changed = plan.window.as_ref().is_some_and(|w| {
        let fresh = &cache.lines[w.first..w.fresh_end];
        fresh.len() != w.old_rows.len()
            || fresh.iter().map(|l| l.rows).ne(w.old_rows.iter().copied())
    });
    if wrapping
        && (cache.rows_stale
            || plan.full
            || window_rows_changed
            || cache.rows.len() != line_count + 1)
    {
        cache.rebuild_rows();
    }
    cache.rows_stale = false;

    // Highlighting: the hook sees the whole value and reports no per-line
    // state, so it reruns over the document on every text change. Lines whose
    // text and spans both match the previous run keep their primitives.
    let mut extras: SmallVec<[usize; 8]> = SmallVec::new();
    let mut too_many = false;
    if let Some(hook) = st.highlight.as_mut() {
        if cache.hl_stale {
            cache.hl = hook(&st.value);
            cache.hl_stale = false;
            for i in 0..line_count {
                let len = cache.line_end(i, st.value.len()) - cache.starts[i];
                let sig = line_sig(cache.lines[i].hash, effective_spans(&cache.hl, i, len));
                if cache.lines[i].sig != sig {
                    cache.lines[i].sig = sig;
                    let fresh = plan.full
                        || plan
                            .window
                            .as_ref()
                            .is_some_and(|w| (w.first..w.fresh_end).contains(&i));
                    if !fresh {
                        if extras.len() < extras.capacity() {
                            extras.push(i);
                        } else {
                            too_many = true;
                        }
                    }
                }
            }
        }
    } else {
        cache.hl_stale = false;
    }

    if lines_changed {
        cache.widest = match plan.window.as_ref().filter(|_| settled) {
            // Another line is at least as wide as the replaced ones.
            Some(w) if w.removed_widest < cache.widest || w.removed_widest <= 0.0 => cache.lines
                [w.first..w.fresh_end]
                .iter()
                .fold(cache.widest, |acc, l| acc.max(l.width)),
            _ => cache.lines.iter().fold(0.0f32, |w, l| w.max(l.width)),
        };
    }
    let widest = cache.widest;
    let total_rows = if wrapping {
        cache.rows[line_count] as usize
    } else {
        line_count
    };
    let rows = total_rows.max(st.min_rows as usize);
    let content = if wrapping {
        Size {
            width: avail_w,
            height: rows as f32 * line_h + 2.0 * AREA_PAD,
        }
    } else {
        Size {
            width: widest.max(MIN_AREA_W) + gutter_w + 2.0 * AREA_PAD,
            height: rows as f32 * line_h + 2.0 * AREA_PAD,
        }
    };
    st.content = content;

    let rect = node_rect(scene, id, content);
    let rect_moved = st.last_rect != Some(rect);
    st.last_rect = Some(rect);
    let origin = Point {
        x: rect.x + AREA_PAD + gutter_w,
        y: rect.y + AREA_PAD,
    };
    let band = if placeholder_mode {
        None
    } else {
        let window = runtime.with(|rt| rt.borrow().window);
        pick_band(
            visible_rows(scene, id, origin.y, line_h, window),
            cache.band,
        )
    };
    let key = EmitKey {
        band,
        origin,
        line_h,
        shape: ShapeKey { phys, wrap, max_w },
        scale,
        text: th.text,
        muted: th.text_muted,
        line_numbers: line_numbers.map(|l| (l.start_line, l.last_line, l.color)),
        placeholder: placeholder_mode,
    };
    let m = Metrics {
        phys,
        scale,
        inv,
        wrap,
        wrapping,
        max_w,
        line_h,
        origin,
        text: th.text,
        band,
    };
    let frame_top = pinned_top(scene, id, rect);
    let frame_moved = cache.frame_top != frame_top;
    cache.frame_top = frame_top;
    let head = surface(framed(rect, frame_top), focused, invalid, &th);
    // A composition shows as one caret at the input method's cursor (or none)
    // and an underline; the selection it replaced is hidden meanwhile.
    let comp = st
        .compose
        .as_ref()
        .map(|c| (c.lo, c.len, c.cursor, c.caret_shown()));
    let selections = match comp {
        None => selection_list(st.caret, st.anchor, &st.secondary),
        Some((_, _, _, true)) => selection_list(st.caret, st.caret, &[]),
        Some(_) => SmallVec::new(),
    };
    let value = st.value.as_str();

    // With a band, an edit that adds lines or visual rows moves which lines
    // the band holds and renumbers the gutter: re-emit the band for those.
    let structural = band.is_some() && window_rows_changed;
    let incremental = !plan.full
        && !too_many
        && !structural
        && !placeholder_mode
        && cache.key == Some(key)
        && cache.ranges.is_some_and(|r| {
            scene
                .paint(id)
                .is_some_and(|pd| ranges_valid(&pd.primitives, &r))
        })
        && !has_indexed_decoration(runtime, id);

    if incremental {
        let mut r = cache.ranges.unwrap();
        let mut scratch = std::mem::take(&mut cache.scratch);
        let mut washes = std::mem::take(&mut cache.washes);
        let prims = &mut scene.paint_mut(id).primitives;
        prims[0] = head[0];
        prims[1] = head[1];
        let text_start = r.wash_end;

        // The lines the edit replaced.
        let mut rows_changed = false;
        let mut shifted = false;
        if let Some(w) = &plan.window {
            let pos = text_start + cache.text_prims_before(w.first);
            scratch.primitives.clear();
            let mut fresh_rows: SmallVec<[u32; 8]> = SmallVec::new();
            for j in w.first..w.fresh_end {
                let (s, e) = (cache.starts[j], cache.line_end(j, value.len()));
                let spans = effective_spans(&cache.hl, j, e - s);
                let before = scratch.primitives.len();
                if !cache.in_band(j, wrapping, band) {
                    fresh_rows.push(cache.lines[j].rows);
                    continue;
                }
                let info = &cache.lines[j];
                emit_line(
                    &mut scratch,
                    shaper,
                    atlas,
                    &m,
                    &value[s..e],
                    info.shape.as_deref().unwrap(),
                    spans,
                    cache.row_of(j, wrapping),
                );
                fresh_rows.push(info.rows);
                let n = (scratch.primitives.len() - before) as u32;
                let out = overflows(&rect, &scratch.primitives[before..]);
                cache.lines[j].text_prims = n;
                cache.lines[j].overflow = out;
                own(cache, j);
            }
            let n_new = scratch.primitives.len();
            splice_prims(
                prims,
                pos..pos + w.removed_prims,
                scratch.primitives.drain(..),
            );
            grow(
                &mut r,
                Section::Text,
                n_new as isize - w.removed_prims as isize,
            );
            let old_sum: i64 = w.old_rows.iter().map(|&x| i64::from(x)).sum();
            let new_sum: i64 = fresh_rows.iter().map(|&x| i64::from(x)).sum();
            if new_sum != old_sum {
                shifted = true;
                note_touched(r.text_end - (pos + n_new));
                shift_y(
                    &mut prims[pos + n_new..r.text_end],
                    (new_sum - old_sum) as f32 * line_h,
                );
            }
            rows_changed = fresh_rows != w.old_rows;
        }

        // Lines whose highlight changed without their text changing.
        for &j in &extras {
            if !cache.in_band(j, wrapping, band) {
                continue;
            }
            own(cache, j);
            let pos = text_start + cache.text_prims_before(j);
            let old = cache.lines[j].text_prims as usize;
            let (s, e) = (cache.starts[j], cache.line_end(j, value.len()));
            scratch.primitives.clear();
            emit_line(
                &mut scratch,
                shaper,
                atlas,
                &m,
                &value[s..e],
                cache.lines[j].shape.as_deref().unwrap(),
                effective_spans(&cache.hl, j, e - s),
                cache.row_of(j, wrapping),
            );
            let n = scratch.primitives.len();
            cache.lines[j].overflow = overflows(&rect, &scratch.primitives);
            splice_prims(prims, pos..pos + old, scratch.primitives.drain(..));
            cache.lines[j].text_prims = n as u32;
            grow(&mut r, Section::Text, n as isize - old as isize);
        }

        // Selection washes.
        washes.clear();
        if focused {
            build_washes(
                &mut washes,
                cache,
                value.len(),
                &selections,
                &m,
                th.text_selection,
            );
        }
        let old_len = r.wash_end - r.gutter_end;
        splice_prims(prims, r.gutter_end..r.wash_end, washes.iter().copied());
        grow(
            &mut r,
            Section::Wash,
            washes.len() as isize - old_len as isize,
        );

        // Gutter: a no-wrap gutter only grows or shrinks at its tail; a wrapped
        // one moves with the rows, so it is rebuilt when they changed.
        // (With a band the gutter only changes with the band, which rebuilds.)
        if let Some(ln) = line_numbers.filter(|_| band.is_none()) {
            let mut counts = std::mem::take(&mut cache.gutter_counts);
            if !wrapping {
                let old_n = counts.len();
                if line_count > old_n {
                    ensure_labels(cache, shaper, ln, line_count, phys);
                    scratch.primitives.clear();
                    emit_labels(
                        &mut scratch,
                        shaper,
                        atlas,
                        &m,
                        cache,
                        ln.color,
                        old_n..line_count,
                        &mut counts,
                    );
                    let n = scratch.primitives.len();
                    splice_prims(
                        prims,
                        r.gutter_end..r.gutter_end,
                        scratch.primitives.drain(..),
                    );
                    grow(&mut r, Section::Gutter, n as isize);
                } else if line_count < old_n {
                    let removed: usize = counts[line_count..].iter().map(|&c| c as usize).sum();
                    counts.truncate(line_count);
                    splice_prims(
                        prims,
                        r.gutter_end - removed..r.gutter_end,
                        std::iter::empty(),
                    );
                    grow(&mut r, Section::Gutter, -(removed as isize));
                }
            } else if rows_changed || counts.len() != line_count {
                ensure_labels(cache, shaper, ln, line_count, phys);
                scratch.primitives.clear();
                counts.clear();
                emit_labels(
                    &mut scratch,
                    shaper,
                    atlas,
                    &m,
                    cache,
                    ln.color,
                    0..line_count,
                    &mut counts,
                );
                let n = scratch.primitives.len();
                let old = r.gutter_end - 2;
                splice_prims(prims, 2..r.gutter_end, scratch.primitives.drain(..));
                grow(&mut r, Section::Gutter, n as isize - old as isize);
            }
            cache.gutter_counts = counts;
        }

        // Carets.
        let mut carets: SmallVec<[Primitive; 5]> = SmallVec::new();
        if focused {
            build_carets(&mut carets, cache, value.len(), &selections, &m, th.text);
            if let Some((lo, len, cursor, _)) = comp {
                st.preedit_box = build_preedit_marks(
                    &mut carets,
                    cache,
                    value.len(),
                    lo,
                    len,
                    cursor,
                    &m,
                    th.text,
                );
            }
        }
        splice_prims(prims, r.text_end..r.caret_end, carets.iter().copied());
        r.caret_end = r.text_end + carets.len();

        // The ring hugs the painted bounds: it needs re-fitting when the box
        // moved, lines shifted, or the bounds of what spills out of the box
        // changed. A steady spill (an unbreakable token) costs nothing per move.
        if lines_changed || !extras.is_empty() {
            cache.line_out = owned_overflow(cache);
        }
        let out = union_bounds(
            cache.line_out,
            union_bounds(overflows(&rect, &washes), overflows(&rect, &carets)),
        );
        let restyle = if rect_moved || frame_moved || shifted || out != cache.painted_out {
            Restyle::Moved
        } else {
            Restyle::None
        };
        cache.painted_out = out;
        cache.scratch = scratch;
        cache.washes = washes;
        cache.ranges = Some(r);
        cache.key = Some(key);
        cache.band = band;
        return (content, restyle);
    }

    // Full rebuild of the paint (first emit, a moved/resized box, new theme).
    cache.rebuilds += 1;
    let mut washes = std::mem::take(&mut cache.washes);
    washes.clear();
    if focused {
        build_washes(
            &mut washes,
            cache,
            value.len(),
            &selections,
            &m,
            th.text_selection,
        );
    }
    let mut counts = std::mem::take(&mut cache.gutter_counts);
    counts.clear();
    let mut emit_lines = cache.band_lines(band, wrapping, line_count);
    // A wrapped band past the last row still yields the last line; it shows no
    // row in the band, and an edit (`in_band`) would not paint it either.
    if !emit_lines.is_empty() && !cache.in_band(emit_lines.start, wrapping, band) {
        emit_lines.start += 1;
    }
    if let Some(ln) = line_numbers {
        ensure_labels(cache, shaper, ln, emit_lines.end, phys);
    }
    let owned = cache.owned.start.min(line_count)..cache.owned.end.min(line_count);
    for l in &mut cache.lines[owned] {
        l.text_prims = 0;
        l.overflow = None;
    }
    cache.owned = emit_lines.clone();
    let mut pd = std::mem::take(scene.paint_mut(id));
    pd.primitives.clear();
    pd.primitives.extend(head);
    // Source line numbers occupy a visual-only gutter. Wrapped continuation
    // rows deliberately remain unlabeled, like conventional code editors.
    if let Some(ln) = line_numbers {
        emit_labels(
            &mut pd,
            shaper,
            atlas,
            &m,
            cache,
            ln.color,
            emit_lines.clone(),
            &mut counts,
        );
    }
    let gutter_end = pd.primitives.len();
    pd.primitives.extend(washes.iter().copied());
    let wash_end = pd.primitives.len();
    if placeholder_mode {
        let ph = shape_area_line(shaper, &st.placeholder, phys);
        let before = pd.primitives.len();
        crate::widgets::rasterize_and_push(
            &mut pd,
            shaper,
            atlas,
            &ph,
            phys as u32,
            th.text_muted,
            scale,
            origin,
        );
        cache.lines[0].text_prims = (pd.primitives.len() - before) as u32;
        cache.lines[0].overflow = overflows(&rect, &pd.primitives[before..]);
    } else {
        for i in emit_lines {
            let (s, e) = (cache.starts[i], cache.line_end(i, value.len()));
            let before = pd.primitives.len();
            emit_line(
                &mut pd,
                shaper,
                atlas,
                &m,
                &value[s..e],
                cache.lines[i].shape.as_deref().unwrap(),
                effective_spans(&cache.hl, i, e - s),
                cache.row_of(i, wrapping),
            );
            cache.lines[i].text_prims = (pd.primitives.len() - before) as u32;
            cache.lines[i].overflow = overflows(&rect, &pd.primitives[before..]);
        }
    }
    let text_end = pd.primitives.len();
    let mut carets: SmallVec<[Primitive; 5]> = SmallVec::new();
    if focused {
        build_carets(&mut carets, cache, value.len(), &selections, &m, th.text);
        if let Some((lo, len, cursor, _)) = comp {
            st.preedit_box = build_preedit_marks(
                &mut carets,
                cache,
                value.len(),
                lo,
                len,
                cursor,
                &m,
                th.text,
            );
        }
    }
    pd.primitives.extend(carets.iter().copied());
    let caret_end = pd.primitives.len();
    note_touched(pd.primitives.len());
    *scene.paint_mut(id) = pd;
    cache.line_out = owned_overflow(cache);
    cache.painted_out = union_bounds(
        cache.line_out,
        union_bounds(overflows(&rect, &washes), overflows(&rect, &carets)),
    );
    cache.washes = washes;
    cache.gutter_counts = counts;
    cache.ranges = Some(PrimRanges {
        gutter_end,
        wash_end,
        text_end,
        caret_end,
    });
    cache.key = Some(key);
    cache.band = band;
    (content, Restyle::Rebuilt)
}

/// Moves the pooled id scratch out of the runtime and fills it with every
/// area id; [`return_area_ids`] gives it back. Reusing one `Vec` keeps the
/// per-frame passes allocation-free however many areas are mounted.
fn take_area_ids(runtime: &crate::widgets::Runtime) -> Vec<WidgetId> {
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let mut ids = std::mem::take(&mut rt.area_scratch);
        ids.extend(rt.areas.keys());
        ids
    })
}

fn return_area_ids(runtime: &crate::widgets::Runtime, mut ids: Vec<WidgetId>) {
    ids.clear();
    runtime.with(|rt| rt.borrow_mut().area_scratch = ids);
}

/// The post-layout re-emit pass for text areas (SOUL §8.1): when an edit grew
/// (or shrank) the measured box, the frame's relayout hands the node a new
/// rect — this pass re-emits the paint into it. Idempotent: an area whose
/// laid-out rect matches its last emission is skipped without a heap touch
/// (Directive #1/#3; the enclosing layout block itself only runs when
/// something is layout-dirty).
pub(crate) fn reemit_moved_areas(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    let ids = take_area_ids(runtime);
    for &id in &ids {
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
    return_area_ids(runtime, ids);
}

/// Moves the painted border and fill to the viewport's top edge after a
/// scroll: two rects rewritten in place, nothing else touched.
fn repin_surface(runtime: &crate::widgets::Runtime, scene: &mut Scene, id: WidgetId) {
    let Some(rect) = runtime.with(|rt| rt.borrow().areas.get(id).and_then(|st| st.last_rect))
    else {
        return;
    };
    let top = pinned_top(scene, id, rect);
    let (focused, invalid) = scene.a11y(id).map_or((false, false), |a| {
        let state = StateFlags(a.state);
        (
            state.contains(StateFlags::FOCUSED),
            state.contains(StateFlags::INVALID),
        )
    });
    let th = crate::widgets::theme_for(runtime, id);
    let head = surface(framed(rect, top), focused, invalid, &th);
    let prims = &mut scene.paint_mut(id).primitives;
    // the surface is always the first two primitives (see `ranges_valid`)
    if prims.len() >= 2 {
        prims[0] = head[0];
        prims[1] = head[1];
    }
    runtime.with(|rt| {
        if let Some(st) = rt.borrow_mut().areas.get_mut(id) {
            st.cache.frame_top = top;
        }
    });
    // a ring or hover stroke was sized from the old frame
    crate::widgets::reapply_focus_ring(runtime, scene, id);
    scene.mark_dirty(id, DirtyFlags::PAINT);
}

/// The per-frame pass for text areas with a row band: scrolling, a compositor
/// slide or a window resize changes what shows without relayout, so an area
/// whose emitted row band no longer covers what its viewport (or the window)
/// shows is re-emitted for the new rows. An area still inside its band costs a
/// few ancestor lookups and no emission.
pub(crate) fn reemit_scrolled_areas(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    let ids = take_area_ids(runtime);
    for &id in &ids {
        if scene.node(id).is_none() {
            continue;
        }
        let (stale, repin) = runtime.with(|rt| {
            let rt = rt.borrow();
            let Some(st) = rt.areas.get(id) else {
                return (false, false);
            };
            let (Some(rect), Some((_, line_h))) = (st.last_rect, st.cache.line_h) else {
                return (false, false);
            };
            // the frame follows the viewport even when no row needs emitting
            let repin = pinned_top(scene, id, rect) != st.cache.frame_top;
            // the placeholder paints no lines and has no band
            if st.value.is_empty() && !st.placeholder.is_empty() {
                return (false, repin);
            }
            let visible = visible_rows(scene, id, rect.y + AREA_PAD, line_h, rt.window);
            let stale = pick_band(visible, st.cache.band) != st.cache.band;
            (stale, repin)
        });
        if stale {
            // the emission lays the frame at the viewport too
            emit_text_area_paint(runtime, scene, shaper, atlas, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
        } else if repin {
            repin_surface(runtime, scene, id);
        }
    }
    return_area_ids(runtime, ids);
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// dispatch hooks (called from `text_edit`'s single inbound path, SOUL §6.3)
// ---------------------------------------------------------------------------
