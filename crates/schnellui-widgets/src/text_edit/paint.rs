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

// ---------------------------------------------------------------------------
// paint emission (cleared-and-refilled in place, SOUL §3.2, §4.4)
// ---------------------------------------------------------------------------

/// Emits a text input's full paint: border + surface, the selection wash, the
/// value glyphs, its Material-style floating label, and — when the node is
/// focused with a collapsed selection — the caret line. Returns the field's
/// complete logical intrinsic size for the build-time measure.
///
/// Before layout the primitives sit at a provisional `(0,0)` origin box (the
/// border rect *is* the min-origin, so [`reposition_node`](crate::reposition_paint)
/// slides the whole set — glyph insets preserved — exactly like a button); after
/// layout they are emitted at absolute laid-out coordinates and the slide is a
/// no-op (idempotent, SOUL §8.1).
pub(crate) fn emit_text_input_paint(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
) -> Size {
    let focused = scene
        .a11y(id)
        .map(|a| StateFlags(a.state).contains(StateFlags::FOCUSED))
        .unwrap_or(false);
    let Some((value, placeholder, selections, size_px, scale, label_progress, width, password)) =
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            rt.edits.get_mut(id).map(|e| {
                e.label_target = if e.placeholder.is_empty() || (!focused && e.value.is_empty()) {
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
                )
            })
        })
    else {
        return Size::default();
    };

    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(size_px, scale);
    let shown_value = display_value(&value, password);
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
    let intrinsic = Size {
        width: (text_sz.width.max(full_label_sz.width).max(MIN_FIELD_W) + 2.0 * pad_h)
            .max(width.unwrap_or(0.0)),
        height: if placeholder.is_empty() {
            content_h + 2.0 * pad_v
        } else {
            content_h + float_label_sz.height + 3.0 * pad_v
        },
    };
    let rect = node_rect(scene, id, intrinsic);
    let value_origin = Point {
        x: rect.x + pad_h,
        y: if placeholder.is_empty() {
            rect.y + pad_v
        } else {
            rect.y + float_label_sz.height + 2.0 * pad_v
        },
    };

    let t = crate::theme_for(runtime, id);
    let bw = input_border_w(runtime, id);
    let radius = t.shape.radius(3.0, rect.height);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    // field surface: border rect + inset background (two SolidRects — the renderer
    // has no stroked-rect primitive, SOUL §3.2)
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: if focused { t.accent } else { t.outline },
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
    // selection wash behind the glyphs (only meaningful while focused)
    if focused {
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
            pd.primitives.push(Primitive::SolidRect {
                rect: Rect::new(value_origin.x + x0, value_origin.y, x1 - x0, content_h),
                color: t.text_selection,
                corner_radius: 0.0,
            });
        }
    }
    // A label begins where a conventional placeholder would sit, then eases
    // upward and shrinks when focused or populated. Geometry remains inside the
    // field so themes with thick frames and hard shadows still compose cleanly.
    if !placeholder.is_empty() {
        let progress = label_progress.clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - progress).powi(3);
        let label_size_px = size_px + (float_size_px - size_px) * eased;
        let label_phys = phys_size_px(label_size_px, scale);
        let label = shaper.shape(&placeholder, label_phys, None);
        let rest_y = rect.y + (rect.height - full_label_sz.height) * 0.5;
        let float_y = rect.y + pad_v;
        let label_origin = Point {
            x: rect.x + pad_h,
            y: rest_y + (float_y - rest_y) * eased,
        };
        rasterize_and_push(
            pd,
            shaper,
            atlas,
            &label,
            label_phys as u32,
            if focused { t.accent } else { t.text_muted },
            scale,
            label_origin,
        );
    }
    // Once present, the value occupies the lower text line while the label stays
    // floated. An unlabelled input retains the original compact single-line box.
    if !value.is_empty() {
        rasterize_and_push(
            pd,
            shaper,
            atlas,
            &shaped,
            phys as u32,
            t.text,
            scale,
            value_origin,
        );
    }
    // the caret (collapsed selection only — a range selection shows the wash)
    if focused {
        for selection in &selections {
            if selection.caret != selection.anchor {
                continue;
            }
            let display_caret = display_byte_for_value_byte(&value, selection.caret, password);
            let x = value_origin.x + advance_before(&shaped, display_caret) * inv;
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
    crate::selection::append_combobox_caret(runtime, scene, id);
    crate::reapply_focus_ring(runtime, scene, id);
    intrinsic
}
