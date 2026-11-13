use super::*;

pub const PASSWORD_GLYPH: &str = "•";

/// Produces the only representation of a protected value allowed to reach paint
/// or accessibility surfaces: one bullet per user-perceived character.
pub(crate) fn obscured_value(value: &str) -> String {
    PASSWORD_GLYPH.repeat(value.graphemes(true).count())
}

pub(crate) fn display_value(value: &str, password: bool) -> String {
    if password {
        obscured_value(value)
    } else {
        value.to_owned()
    }
}

pub fn display_byte_for_value_byte(value: &str, byte: usize, password: bool) -> usize {
    if !password {
        return byte;
    }
    value[..byte].graphemes(true).count() * PASSWORD_GLYPH.len()
}

pub(crate) fn value_byte_for_display_byte(value: &str, byte: usize, password: bool) -> usize {
    if !password {
        return byte;
    }
    let grapheme = byte / PASSWORD_GLYPH.len();
    value
        .grapheme_indices(true)
        .nth(grapheme)
        .map_or(value.len(), |(index, _)| index)
}

/// The caret byte index nearest a physical inline offset `x` (LTR): the boundary
/// past every glyph whose midpoint lies left of `x`.
pub(crate) fn byte_at_x(shaped: &ShapedText, len: usize, x: f32) -> usize {
    let mut pen = 0.0f32;
    for g in &shaped.glyphs {
        if x < pen + g.x_advance * 0.5 {
            return g.cluster as usize;
        }
        pen += g.x_advance;
    }
    len
}

/// The byte cluster of the glyph whose advance box contains `x`. Unlike
/// [`byte_at_x`], this identifies text *under* the pointer instead of the
/// nearest caret boundary, which is what word/line selection needs.
pub(crate) fn byte_under_x(shaped: &ShapedText, len: usize, x: f32) -> usize {
    let mut pen = 0.0f32;
    for g in &shaped.glyphs {
        if x < pen + g.x_advance {
            return g.cluster as usize;
        }
        pen += g.x_advance;
    }
    len
}

/// Where a composition sits in the text a field draws.
#[derive(Clone, Copy)]
pub(crate) struct Composition {
    /// The composed text, as display bytes.
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// The input method's cursor range, as display bytes.
    pub(crate) cursor: Option<(usize, usize)>,
    /// The byte the scroll must keep in view.
    pub(crate) reveal: usize,
}

/// The text a field draws while `preedit` replaces `lo..hi` of `value`, and
/// where the composition lies in it. A protected value stays obscured, and so
/// does what is composed into it.
pub(crate) fn composed_display(
    value: &str,
    lo: usize,
    hi: usize,
    preedit: &Preedit,
    password: bool,
) -> (String, Option<Composition>) {
    let mut shown = display_value(&value[..lo], password);
    let start = shown.len();
    let map = |byte: usize| {
        if password {
            display_byte_for_value_byte(&preedit.text, byte, true)
        } else {
            byte
        }
    };
    if password {
        shown.push_str(&obscured_value(&preedit.text));
    } else {
        shown.push_str(&preedit.text);
    }
    let end = shown.len();
    shown.push_str(&display_value(&value[hi..], password));
    let cursor = preedit
        .cursor
        .map(|(a, b)| (start + map(a), start + map(b)));
    let reveal = cursor.map_or(end, |(_, b)| b);
    (
        shown,
        Some(Composition {
            start,
            end,
            cursor,
            reveal,
        }),
    )
}

/// Crops the glyph quads in `primitives[first..]` to the column `x0..x1`,
/// trimming the atlas sample with the quad so a cut glyph keeps its scale, and
/// drops quads that fall wholly outside.
fn clip_glyphs_x(primitives: &mut Vec<Primitive>, first: usize, x0: f32, x1: f32) {
    let mut keep = first;
    for i in first..primitives.len() {
        let mut p = primitives[i];
        if let Primitive::GlyphQuad { rect, atlas_uv, .. } = &mut p {
            let left = (x0 - rect.x).max(0.0);
            let right = (rect.x + rect.width - x1).max(0.0);
            if left + right >= rect.width {
                continue;
            }
            let texels = atlas_uv.width / rect.width;
            rect.x += left;
            rect.width -= left + right;
            atlas_uv.x += left * texels;
            atlas_uv.width -= (left + right) * texels;
        }
        primitives[keep] = p;
        keep += 1;
    }
    primitives.truncate(keep);
}

// ---------------------------------------------------------------------------
// paint emission (cleared-and-refilled in place, SOUL §3.2, §4.4)
// ---------------------------------------------------------------------------

/// Emits a text input's full paint: border + surface, the selection wash, the
/// value glyphs, its Material-style floating label, and — when the node is
/// focused with a collapsed selection — the caret line. Returns the field's
/// complete logical intrinsic size for the build-time measure.
///
/// Before layout the primitives sit at a provisional `(0,0)` origin box (the
/// border rect *is* the min-origin, so [`reposition_node`](crate::widgets::reposition_paint)
/// slides the whole set — glyph insets preserved — exactly like a button); after
/// layout they are emitted at absolute laid-out coordinates and the slide is a
/// no-op (idempotent, SOUL §8.1).
pub(crate) fn emit_text_input_paint(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
) -> Size {
    let (focused, invalid) = scene.a11y(id).map_or((false, false), |a| {
        let state = StateFlags(a.state);
        (
            state.contains(StateFlags::FOCUSED),
            state.contains(StateFlags::INVALID),
        )
    });
    let Some((
        value,
        placeholder,
        selections,
        size_px,
        scale,
        label_progress,
        width,
        password,
        scroll_x,
        hold_scroll,
        hint,
        min_height,
        preedit,
    )) = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.edits.get_mut(id).map(|e| {
            e.label_target =
                if e.hint || e.placeholder.is_empty() || (!focused && e.value.is_empty()) {
                    0.0
                } else {
                    1.0
                };
            (
                e.value.clone(),
                e.placeholder.clone(),
                selection_list(e.caret, e.anchor, &e.secondary),
                e.size_px,
                e.scale,
                e.label_progress,
                e.width,
                e.password,
                e.scroll_x,
                std::mem::take(&mut e.hold_scroll),
                e.hint,
                e.min_height,
                e.preedit.clone(),
            )
        })
    })
    else {
        return Size::default();
    };

    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(size_px, scale);
    // A composition shows in the field's own text, in place of the primary
    // selection. Everything below measures that composed text.
    let (shown_value, comp) = match preedit.as_ref().filter(|_| focused) {
        Some(p) => {
            let (lo, hi) = selections[0].range();
            composed_display(&value, lo, hi, p, password)
        }
        None => (display_value(&value, password), None),
    };
    let shaped = shaper.shape(&shown_value, phys, None);
    let text_sz = Size {
        width: shaped.width * inv,
        height: shaped.height * inv,
    };
    let content_h = if text_sz.height > 0.0 {
        text_sz.height
    } else {
        size_px * EMPTY_LINE_RATIO
    };
    let full_label = shaper.shape(&placeholder, phys, None);
    let full_label_sz = Size {
        width: full_label.width * inv,
        height: full_label.height * inv,
    };
    let float_size_px = size_px * FLOAT_LABEL_SIZE_RATIO;
    let float_phys = phys_size_px(float_size_px, scale);
    let float_label = shaper.shape(&placeholder, float_phys, None);
    let float_label_sz = Size {
        width: float_label.width * inv,
        height: float_label.height * inv,
    };
    let (pad_h, pad_v) = input_pads(runtime, id);
    // The width ignores the value, as an HTML `<input>` does. The build-time
    // measure is fixed while editing never relayouts, so a value-sized field
    // would be wider when mounted with text than after typing the same text.
    // A requested width also ignores the label, which is cut with an ellipsis
    // below, so a long label never pushes the field past its row.
    let intrinsic = Size {
        width: width.map_or(
            full_label_sz.width.max(MIN_FIELD_W) + 2.0 * pad_h,
            |width| width.max(MIN_FIELD_W + 2.0 * pad_h),
        ),
        height: if hint || placeholder.is_empty() {
            content_h + 2.0 * pad_v
        } else {
            content_h + float_label_sz.height + 3.0 * pad_v
        }
        .max(min_height),
    };
    let rect = node_rect(scene, id, intrinsic);
    // Glyph rows start on a whole device pixel. A field laid out at a
    // fractional y would otherwise sample the atlas between texel rows and
    // smear every glyph vertically.
    let snap = |v: f32| (v / inv).round() * inv;
    let value_origin = Point {
        x: rect.x + pad_h,
        y: snap(if hint || placeholder.is_empty() {
            rect.y + ((rect.height - content_h) * 0.5).max(pad_v)
        } else {
            rect.y + float_label_sz.height + 2.0 * pad_v
        }),
    };

    // Horizontal scroll (single line, LTR): the text box spans the padded
    // content width. While focused, the primary caret drags the view along the
    // value; the scroll never passes the end of the text, so deleting pulls the
    // value back in. Blurring returns to the start.
    let clip_x0 = value_origin.x;
    let clip_x1 = (rect.x + rect.width
        - pad_h
        - crate::widgets::selection::combobox_caret_reserve(scene, id))
    .max(clip_x0);
    let view_w = clip_x1 - clip_x0;
    let mut scroll_x = if focused { scroll_x } else { 0.0 };
    if focused && !hold_scroll {
        let display_caret = match comp {
            Some(c) => c.reveal,
            None => display_byte_for_value_byte(&value, selections[0].caret, password),
        };
        let caret_x = advance_before(&shaped, display_caret) * inv;
        if caret_x < scroll_x {
            scroll_x = caret_x;
        } else if caret_x + CARET_W > scroll_x + view_w {
            scroll_x = caret_x + CARET_W - view_w;
        }
    }
    scroll_x = scroll_x
        .min((text_sz.width + CARET_W - view_w).max(0.0))
        .max(0.0);
    runtime.with(|rt| {
        if let Some(e) = rt.borrow_mut().edits.get_mut(id) {
            e.scroll_x = scroll_x;
        }
    });
    let text_origin = Point {
        x: value_origin.x - scroll_x,
        y: value_origin.y,
    };

    let t = crate::widgets::theme_for(runtime, id);
    let bw = input_border_w(runtime, id);
    let radius = t.shape.radius(3.0, rect.height);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    // field surface: border rect + inset background (two SolidRects — the renderer
    // has no stroked-rect primitive, SOUL §3.2)
    pd.primitives.push(Primitive::SolidRect {
        rect,
        // A failed validation outranks focus; the focus ring still shows on top.
        color: if invalid {
            t.invalid_color()
        } else if focused {
            t.accent
        } else {
            t.outline
        },
        corner_radius: radius,
    });
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(
            rect.x + bw,
            rect.y + bw,
            (rect.width - 2.0 * bw).max(0.0),
            (rect.height - 2.0 * bw).max(0.0),
        ),
        color: t.surface,
        corner_radius: (radius - bw).max(0.0),
    });
    // selection wash behind the glyphs (only meaningful while focused; a
    // composition replaces the selection on screen)
    if focused && comp.is_none() {
        for selection in &selections {
            let (start, end) = selection.range();
            if start == end {
                continue;
            }
            let x0 = advance_before(
                &shaped,
                display_byte_for_value_byte(&value, start, password),
            ) * inv;
            let x1 =
                advance_before(&shaped, display_byte_for_value_byte(&value, end, password)) * inv;
            let x0 = (text_origin.x + x0).max(clip_x0);
            let x1 = (text_origin.x + x1).min(clip_x1);
            if x1 <= x0 {
                continue;
            }
            pd.primitives.push(Primitive::SolidRect {
                rect: Rect::new(x0, value_origin.y, x1 - x0, content_h),
                color: t.text_selection,
                corner_radius: 0.0,
            });
        }
    }
    // A label begins where a conventional placeholder would sit, then eases
    // upward and shrinks when focused or populated. Geometry remains inside the
    // field so themes with thick frames and hard shadows still compose cleanly.
    // A hint shows only in an empty field, at the rest position.
    if !placeholder.is_empty() && !(hint && !shown_value.is_empty()) {
        let progress = if hint {
            0.0
        } else {
            label_progress.clamp(0.0, 1.0)
        };
        let eased = 1.0 - (1.0 - progress).powi(3);
        let label_size_px = size_px + (float_size_px - size_px) * eased;
        let label_phys = phys_size_px(label_size_px, scale);
        // The label ends at the value's clip edge, with an ellipsis when cut.
        let label = shaper.truncate_to_width(&placeholder, label_phys, view_w / inv);
        let rest_y = rect.y + (rect.height - full_label_sz.height) * 0.5;
        let float_y = rect.y + pad_v;
        // A hint starts where typed text starts, as a browser's placeholder
        // does, so the first letter typed lands where the hint's began.
        let label_origin = Point {
            x: rect.x + pad_h,
            y: snap(rest_y + (float_y - rest_y) * eased),
        };
        rasterize_and_push(
            pd,
            shaper,
            atlas,
            &label,
            label_phys as u32,
            // Muted even when focused: the accent outline marks focus, and
            // an accent label over an empty field reads as typed text.
            t.text_muted,
            scale,
            label_origin,
        );
    }
    // Once present, the value occupies the lower text line while the label stays
    // floated. An unlabelled input retains the original compact single-line box.
    if !shown_value.is_empty() {
        let first = pd.primitives.len();
        rasterize_and_push(
            pd,
            shaper,
            atlas,
            &shaped,
            phys as u32,
            t.text,
            scale,
            text_origin,
        );
        clip_glyphs_x(&mut pd.primitives, first, clip_x0, clip_x1);
    }
    // a composition: its underline, the input method's cursor range as a
    // thicker segment, and the caret only where the cursor is a point
    let mut preedit_box = None;
    if let Some(c) = comp {
        let at = |byte: usize| text_origin.x + advance_before(&shaped, byte) * inv;
        // The line box of composed wide glyphs can be taller than the field
        // was built for, so the rule never sinks into the border.
        // It sits on a whole device pixel, so it stays crisp.
        let device = norm_scale(scale);
        let bottom = ((value_origin.y + content_h).min(rect.y + rect.height - bw - pad_v * 0.5)
            * device)
            .round()
            / device;
        let rule = |pd: &mut schnellui::scene::PaintData, from: usize, to: usize, w: f32| {
            let x0 = at(from).max(clip_x0);
            let x1 = at(to).min(clip_x1);
            if x1 > x0 {
                pd.primitives.push(Primitive::SolidRect {
                    rect: Rect::new(x0, bottom - w, x1 - x0, w),
                    color: t.text,
                    corner_radius: 0.0,
                });
            }
        };
        rule(pd, c.start, c.end, PREEDIT_RULE_W);
        if let Some((a, b)) = c.cursor {
            if a < b {
                rule(pd, a, b, PREEDIT_CURSOR_RULE_W);
            } else if (clip_x0..=clip_x1).contains(&at(a)) {
                let x = at(a);
                pd.primitives.push(Primitive::Line {
                    from: Point {
                        x,
                        y: value_origin.y,
                    },
                    to: Point {
                        x,
                        y: value_origin.y + content_h,
                    },
                    width: CARET_W,
                    color: t.text,
                });
            }
            preedit_box = Some(Rect::new(
                at(a).clamp(clip_x0, clip_x1),
                value_origin.y,
                CARET_W,
                content_h,
            ));
        } else {
            preedit_box = Some(Rect::new(
                at(c.start).clamp(clip_x0, clip_x1),
                value_origin.y,
                CARET_W,
                content_h,
            ));
        }
    }
    if comp.is_some() {
        runtime.with(|rt| {
            if let Some(e) = rt.borrow_mut().edits.get_mut(id) {
                e.preedit_box = preedit_box;
            }
        });
    }
    // the caret (collapsed selection only — a range selection shows the wash)
    if focused && comp.is_none() {
        for selection in &selections {
            if selection.caret != selection.anchor {
                continue;
            }
            let display_caret = display_byte_for_value_byte(&value, selection.caret, password);
            let x = text_origin.x + advance_before(&shaped, display_caret) * inv;
            if x < clip_x0 || x > clip_x1 {
                continue;
            }
            pd.primitives.push(Primitive::Line {
                from: Point {
                    x,
                    y: value_origin.y,
                },
                to: Point {
                    x,
                    y: value_origin.y + content_h,
                },
                width: CARET_W,
                color: t.text,
            });
        }
    }
    crate::widgets::selection::append_combobox_caret(runtime, scene, id);
    // The paint was cleared and refilled above, so the hover, press and focus
    // records of this field point at primitives that are gone.
    crate::widgets::redecorate_after_refill(runtime, scene, id);
    intrinsic
}
