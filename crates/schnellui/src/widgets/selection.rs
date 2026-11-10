//! Navigation/selection components (SOUL §8.1): [`TabBar`] + [`Tab`] and [`List`] +
//! [`ListItem`]. The bars/lists are **semantic containers** — like `Scroll`, they
//! derive geometry from their children but carry a role of their own
//! ([`Role::TabList`] / [`Role::List`], SOUL §6.1). The tabs/items are content
//! leaves with `StateFlags::SELECTED` and sibling-exclusive selection routed through
//! the *same* inbound path as an AccessKit `Click` `ActionRequest` (SOUL §6.3),
//! mirroring the radio group's discipline in [`crate::widgets::basic`].
//!
//! **Selection recolors, it never re-shapes** (SOUL Directive #3, §4): a tab/item's
//! paint is emitted once at build with its background (and the tab's indicator bar)
//! as dedicated leading primitives; a selection toggle mutates only those
//! primitives' colours in place — the label glyphs are untouched, so no shaper, no
//! atlas, and no heap are involved on the selection path.

use std::borrow::Cow;

use schnellui::a11y::{ActionFlags, Role, StateFlags};
use schnellui::layout::{Container, ContainerStyle, FlexChild, LayoutEngine};
use schnellui::scene::{
    Color, DirtyFlags, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind,
};
use schnellui::text::{GlyphAtlas, TextShaper};
use smallvec::SmallVec;

use crate::widgets::{
    label_top, node_rect, norm_scale, phys_size_px, rasterize_and_push, theme_for, with_handlers,
    AnyView, BuildCtx, ClickHandler, ContextMenu, ContextMenuItem, InputHandler, View,
    BUTTON_TEXT_SIZE, PAD_H, PAD_V,
};

mod navigation;
pub use navigation::*;
mod combobox;
pub use combobox::*;
mod dropdown;
pub use dropdown::*;
#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// visual + metric constants (deterministic for shots, SOUL §7.3)
// ---------------------------------------------------------------------------

/// The selected tab's bottom indicator bar thickness.
const TAB_INDICATOR: f32 = 2.0;
/// Horizontal space reserved for a tree disclosure chevron.
const TAB_DISCLOSURE_SPACE: f32 = 18.0;
const TAB_DISCLOSURE_HALF: f32 = 4.0;
const TAB_DISCLOSURE_STROKE: f32 = 1.75;
/// How far the chevron's arms spread from its centre line.
const TAB_DISCLOSURE_SPREAD: f32 = 2.0;
const TAB_CLOSE_HALF: f32 = 3.5;
const TAB_CLOSE_STROKE: f32 = 1.5;

/// Visual treatment for a [`Tab`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabAppearance {
    /// Filled tab surface with a bottom selection indicator, intended for
    /// horizontal [`TabBar`] chrome.
    #[default]
    Classic,
    /// Flat navigation row with a selected wash and a vertical accent rail.
    /// Resting rows are transparent, so nested/grouped tabs read as one coherent
    /// navigator instead of a stack of independent buttons.
    Navigation,
    /// Plain text label on a transparent surface; the selected tab carries a
    /// bottom indicator whose color and thickness [`Tab::indicator`] can set.
    Underline,
}

/// Per-tab overrides of the shared tab metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TabStyle {
    pub(crate) text_size: Option<f32>,
    pub(crate) pad_h: Option<f32>,
    pub(crate) min_height: Option<f32>,
    pub(crate) indicator: Option<(Color, f32)>,
    /// Empty room before the label, for an icon the caller lays over it.
    pub(crate) lead: f32,
    /// Fill of a selected underline tab, which is otherwise transparent.
    pub(crate) selected_fill: Option<Color>,
}

fn tab_style(runtime: &crate::widgets::Runtime, id: WidgetId) -> TabStyle {
    runtime.with(|runtime| {
        runtime
            .borrow()
            .tab_styles
            .get(id)
            .copied()
            .unwrap_or_default()
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TabDisclosure {
    #[default]
    None,
    Placeholder,
    Branch(bool),
}

// ---------------------------------------------------------------------------
// paint-fragment emission + in-place recolor (SOUL §3.2, §8.1)
// ---------------------------------------------------------------------------

fn tab_appearance(runtime: &crate::widgets::Runtime, id: WidgetId) -> TabAppearance {
    runtime.with(|runtime| {
        runtime
            .borrow()
            .tab_appearances
            .get(id)
            .copied()
            .unwrap_or_default()
    })
}

/// The background / indicator colours for a selection state. A classic tab rests
/// on the theme surface; a navigation tab rests transparent. The indicator is
/// **transparent** when unselected (the renderer alpha-blends, SOUL §3.2), so both
/// states carry the same primitive list and a toggle is a pure recolor.
fn selection_colors(
    runtime: &crate::widgets::Runtime,
    id: WidgetId,
    kind: WidgetKind,
    selected: bool,
) -> (Color, Option<Color>) {
    let t = theme_for(runtime, id);
    // A classic or underline tab marks selection with its bar alone, so the
    // neutral hover fill and the accent selection never look alike.
    let underline =
        kind == WidgetKind::Tab && tab_appearance(runtime, id) != TabAppearance::Navigation;
    let bg = if underline {
        if selected {
            tab_style(runtime, id)
                .selected_fill
                .unwrap_or(Color::TRANSPARENT)
        } else {
            Color::TRANSPARENT
        }
    } else if selected && kind == WidgetKind::DropdownOption {
        // An option row is a floating popup's only surface: a translucent
        // selection tint would show the content under the popup through it.
        over(t.selection, t.surface)
    } else if selected {
        t.selection
    } else if matches!(kind, WidgetKind::Tab | WidgetKind::ListItem) {
        // A resting tab or list row is just its label on whatever holds it:
        // a filled chip per row reads as a stack of buttons. Hover washes
        // it, selection fills it.
        Color::TRANSPARENT
    } else {
        t.surface
    };
    match kind {
        // A chosen list item carries an accent rail at its leading edge, so
        // selection reads apart from the hover wash.
        WidgetKind::ListItem => (
            bg,
            Some(if selected {
                t.indicator_color()
            } else {
                Color::TRANSPARENT
            }),
        ),
        WidgetKind::Tab => {
            let ind = if selected {
                tab_style(runtime, id)
                    .indicator
                    .map_or(t.indicator_color(), |(color, _)| color)
            } else {
                Color::TRANSPARENT
            };
            (bg, Some(ind))
        }
        _ => (bg, None),
    }
}

/// How far a navigation row's rail steps in from the leading edge so its ends
/// stay inside a rounded row: where the row's corner curve crosses the rail's
/// top, `inset` below the row top. Zero for a square row.
fn rail_indent(sh: crate::widgets::Shape, rect: Rect, inset: f32) -> f32 {
    let r = sh.radius(3.0, rect.height);
    if r <= inset {
        return 0.0;
    }
    let dy = r - inset;
    r - (r * r - dy * dy).max(0.0).sqrt()
}

/// Half the width of a dropdown option's check mark.
const OPTION_CHECK_HALF: f32 = 4.5;
const OPTION_CHECK_STROKE: f32 = 2.0;

fn option_check_color(runtime: &crate::widgets::Runtime, id: WidgetId, selected: bool) -> Color {
    if selected {
        theme_for(runtime, id).link_color()
    } else {
        Color::TRANSPARENT
    }
}

/// `top` composited over `base` (source-over), keeping `base`'s alpha.
fn over(top: Color, base: Color) -> Color {
    let a = u32::from(top.a);
    let mix = |t: u8, b: u8| ((u32::from(t) * a + u32::from(b) * (255 - a) + 127) / 255) as u8;
    Color::rgba(
        mix(top.r, base.r),
        mix(top.g, base.g),
        mix(top.b, base.b),
        base.a,
    )
}

/// The ink-frame width a selectable leaf wears: only dropdown options do — the
/// popup panel gets its edges from them (SOUL §8.1) — tabs and list items stay
/// flat chrome in every design system.
fn selectable_frame(runtime: &crate::widgets::Runtime, id: WidgetId, kind: WidgetKind) -> f32 {
    if kind == WidgetKind::DropdownOption {
        theme_for(runtime, id).shape.frame
    } else {
        0.0
    }
}

/// Emits a tab's / list item's paint: the background surface, the tab's indicator
/// bar (transparent when unselected), and the label as real glyph quads inset by
/// the shared button padding (SOUL §8.1). `min_width` widens the surface beyond
/// the label's intrinsic width — a dropdown option must span its popup's shared
/// width so the panel edge is straight and opaque. Under an inked design system
/// ([`Shape::frame`](crate::widgets::Shape::frame)) a dropdown option also wears frame
/// strips *on top of* its surface — left + right panel edges and a bottom rule
/// (the panel's top edge is the trigger's own bottom frame) — so the surface
/// stays primitive `[0]`, the in-place recolor target of a selection toggle.
/// Returns the label's **logical** text size (for the intrinsic-measure closure).
#[allow(clippy::too_many_arguments)]
fn emit_selectable_paint(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    kind: WidgetKind,
    label: &str,
    selected: bool,
    scale: f32,
    min_width: f32,
    disclosure: TabDisclosure,
) -> Size {
    let inv = 1.0 / norm_scale(scale);
    let style = if kind == WidgetKind::Tab || kind == WidgetKind::DropdownOption {
        tab_style(runtime, id)
    } else {
        TabStyle::default()
    };
    let phys = phys_size_px(style.text_size.unwrap_or(BUTTON_TEXT_SIZE), scale);
    let shaped = shaper.shape(label, phys, None);
    let ts = Size {
        width: shaped.width * inv,
        height: shaped.height * inv,
    };
    let sh = theme_for(runtime, id).shape;
    let (pad_h, pad_v) = (style.pad_h.unwrap_or(sh.pad(PAD_H)), sh.pad(PAD_V));
    let f = selectable_frame(runtime, id, kind);
    let disclosure_width = if disclosure == TabDisclosure::None {
        style.lead
    } else {
        TAB_DISCLOSURE_SPACE + style.lead
    };
    let intrinsic = Size {
        width: (ts.width + 2.0 * (pad_h + f) + disclosure_width).max(min_width),
        height: (ts.height + 2.0 * pad_v).max(style.min_height.unwrap_or(0.0)),
    };
    let rect = node_rect(scene, id, intrinsic);
    let (bg, indicator) = selection_colors(runtime, id, kind, selected);
    let appearance = match kind {
        WidgetKind::Tab => tab_appearance(runtime, id),
        WidgetKind::ListItem => TabAppearance::Navigation,
        _ => TabAppearance::Classic,
    };
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    // [0] background — the recolor target of a selection toggle.
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: bg,
        corner_radius: if appearance == TabAppearance::Navigation {
            sh.radius(3.0, rect.height)
        } else {
            0.0
        },
    });
    // [1] the tab's bottom indicator bar (always present so a toggle recolors
    // in place instead of inserting/removing primitives). An inked design
    // system ([`Shape::frame`](crate::widgets::Shape::frame)) thickens it to match.
    if let Some(ind) = indicator {
        let thickness = style
            .indicator
            .map_or(TAB_INDICATOR + sh.frame, |(_, thickness)| thickness);
        let indicator_rect = match appearance {
            TabAppearance::Classic | TabAppearance::Underline => Rect::new(
                rect.x,
                rect.y + rect.height - thickness,
                rect.width,
                thickness,
            ),
            TabAppearance::Navigation => {
                let inset = sh.pad(4.0).min(rect.height * 0.25);
                Rect::new(
                    rect.x + rail_indent(sh, rect, inset),
                    rect.y + inset,
                    thickness + 1.0,
                    (rect.height - 2.0 * inset).max(0.0),
                )
            }
        };
        pd.primitives.push(Primitive::SolidRect {
            rect: indicator_rect,
            color: ind,
            corner_radius: if appearance == TabAppearance::Navigation {
                sh.radius(1.5, indicator_rect.height)
            } else {
                0.0
            },
        });
    }
    // [1][2] a dropdown option's check mark at its trailing edge, drawn in
    // every row and transparent unless chosen, so selection stays a recolor.
    if kind == WidgetKind::DropdownOption {
        let cx = rect.x + rect.width - f - pad_h - OPTION_CHECK_HALF;
        let cy = rect.y + rect.height * 0.5;
        let color = option_check_color(runtime, id, selected);
        let (a, b, c) = (
            Point {
                x: cx - OPTION_CHECK_HALF,
                y: cy,
            },
            Point {
                x: cx - OPTION_CHECK_HALF * 0.3,
                y: cy + OPTION_CHECK_HALF * 0.7,
            },
            Point {
                x: cx + OPTION_CHECK_HALF,
                y: cy - OPTION_CHECK_HALF * 0.8,
            },
        );
        for (from, to) in [(a, b), (b, c)] {
            pd.primitives.push(Primitive::Line {
                from,
                to,
                width: OPTION_CHECK_STROKE,
                color,
            });
        }
    }
    // A dropdown option's frame strips (drawn over the surface so [0] keeps its
    // index): the popup panel's left/right edges plus a bottom rule per row —
    // the last row's rule is the panel's bottom edge.
    if f > 0.0 {
        let outline = theme_for(runtime, id).outline;
        for strip in [
            Rect::new(rect.x, rect.y, f, rect.height),
            Rect::new(rect.x + rect.width - f, rect.y, f, rect.height),
            Rect::new(rect.x, rect.y + rect.height - f, rect.width, f),
        ] {
            pd.primitives.push(Primitive::SolidRect {
                rect: strip,
                color: outline,
                corner_radius: 0.0,
            });
        }
    }
    if let TabDisclosure::Branch(expanded) = disclosure {
        let cx = rect.x + f + pad_h + TAB_DISCLOSURE_HALF;
        let cy = rect.y + rect.height * 0.5;
        let points = if expanded {
            [
                Point {
                    x: cx - TAB_DISCLOSURE_HALF,
                    y: cy - TAB_DISCLOSURE_SPREAD,
                },
                Point {
                    x: cx,
                    y: cy + TAB_DISCLOSURE_SPREAD,
                },
                Point {
                    x: cx + TAB_DISCLOSURE_HALF,
                    y: cy - TAB_DISCLOSURE_SPREAD,
                },
            ]
        } else {
            [
                Point {
                    x: cx - TAB_DISCLOSURE_SPREAD,
                    y: cy - TAB_DISCLOSURE_HALF,
                },
                Point {
                    x: cx + TAB_DISCLOSURE_SPREAD,
                    y: cy,
                },
                Point {
                    x: cx - TAB_DISCLOSURE_SPREAD,
                    y: cy + TAB_DISCLOSURE_HALF,
                },
            ]
        };
        for segment in points.windows(2) {
            pd.primitives.push(Primitive::Line {
                from: segment[0],
                to: segment[1],
                width: TAB_DISCLOSURE_STROKE,
                color: theme_for(runtime, id).text,
            });
        }
    }
    rasterize_and_push(
        pd,
        shaper,
        atlas,
        &shaped,
        phys as u32,
        theme_for(runtime, id).text,
        scale,
        Point {
            x: rect.x + f + pad_h + disclosure_width,
            y: label_top(
                rect.y,
                intrinsic.height,
                &shaped,
                inv,
                style.text_size.unwrap_or(BUTTON_TEXT_SIZE),
            ),
        },
    );
    ts
}

/// Recolors a tab's / item's selection surfaces **in place** (SOUL Directive #3):
/// primitive `[0]` is the background, `[1]` (tabs only) the indicator bar. No
/// re-shape, no primitive growth, no heap touch.
fn recolor_selection(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    kind: WidgetKind,
    selected: bool,
) {
    let (bg, indicator) = selection_colors(runtime, id, kind, selected);
    let check = option_check_color(runtime, id, selected);
    let pd = scene.paint_mut(id);
    if let Some(Primitive::SolidRect { color, .. }) = pd.primitives.get_mut(0) {
        *color = bg;
    }
    if kind == WidgetKind::DropdownOption {
        for index in 1..=2 {
            if let Some(Primitive::Line { color, .. }) = pd.primitives.get_mut(index) {
                *color = check;
            }
        }
    }
    if let Some(ind) = indicator {
        if let Some(Primitive::SolidRect { color, .. }) = pd.primitives.get_mut(1) {
            *color = ind;
        }
    }
}

/// Extends a navigation tab's retained selection surface to its final flex width,
/// and a closable tab's surface beneath its trailing close target. Glyphs and
/// disclosure lines remain left-anchored; only the background and selection
/// indicator consume newly assigned row space.
pub(crate) fn resize_tab_surface(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    id: WidgetId,
) {
    let kind = scene.node(id).map(|node| node.kind);
    if kind == Some(WidgetKind::ListItem) {
        // A list item spans its list's width (the list stretches its rows):
        // the surface follows the final box, the rail keeps its own width.
        let Some(rect) = scene.layout(id).map(|layout| layout.rect) else {
            return;
        };
        let pd = scene.paint_mut(id);
        if let Some(Primitive::SolidRect {
            rect: surface_rect, ..
        }) = pd.primitives.get_mut(0)
        {
            surface_rect.width = rect.width;
        }
        return;
    }
    if kind != Some(WidgetKind::Tab) {
        return;
    }
    let Some(mut rect) = scene.layout(id).map(|layout| layout.rect) else {
        return;
    };
    let close = runtime.with(|runtime| {
        runtime
            .borrow()
            .tab_close_buttons
            .get(id)
            .copied()
            .and_then(|close| scene.layout(close).map(|layout| layout.rect))
    });
    if let Some(close) = close {
        rect.width = (close.right() - rect.x).max(rect.width);
        rect.height = rect.height.max(close.height);
    } else if !matches!(
        tab_appearance(runtime, id),
        TabAppearance::Navigation | TabAppearance::Underline
    ) {
        return;
    }
    // A row with trailing actions (a grouped list's) fills under them: the
    // actions are drawn on top, so the selection and hover fill reads as the
    // full row.
    if close.is_none() {
        let row_right = scene
            .node(id)
            .and_then(|node| node.parent)
            .filter(|&parent| {
                scene
                    .node(parent)
                    .is_some_and(|node| node.kind == WidgetKind::Row)
            })
            .and_then(|parent| scene.layout(parent))
            .map(|layout| layout.rect.right());
        if let Some(right) = row_right {
            rect.width = rect.width.max(right - rect.x);
        }
    }
    if tab_appearance(runtime, id) == TabAppearance::Underline {
        let thickness = tab_style(runtime, id)
            .indicator
            .map_or(TAB_INDICATOR, |i| i.1);
        let pd = scene.paint_mut(id);
        if let Some(Primitive::SolidRect {
            rect: surface_rect, ..
        }) = pd.primitives.get_mut(0)
        {
            *surface_rect = rect;
        }
        if let Some(Primitive::SolidRect {
            rect: indicator_rect,
            ..
        }) = pd.primitives.get_mut(1)
        {
            *indicator_rect = Rect::new(rect.x, rect.bottom() - thickness, rect.width, thickness);
        }
        return;
    }
    let sh = theme_for(runtime, id).shape;
    let inset = sh.pad(4.0).min(rect.height * 0.25);
    let pd = scene.paint_mut(id);
    if let Some(Primitive::SolidRect {
        rect: surface_rect, ..
    }) = pd.primitives.get_mut(0)
    {
        *surface_rect = rect;
    }
    if let Some(Primitive::SolidRect {
        rect: indicator_rect,
        ..
    }) = pd.primitives.get_mut(1)
    {
        indicator_rect.x = rect.x + rail_indent(sh, rect, inset);
        indicator_rect.y = rect.y + inset;
        indicator_rect.height = (rect.height - 2.0 * inset).max(0.0);
    }
}

// ---------------------------------------------------------------------------
// SELECTED-bit helpers (SOUL §6.1 — packed bits in the a11y column)
// ---------------------------------------------------------------------------

/// `true` if `id`'s a11y column carries the SELECTED bit (SOUL §6.1).
#[inline]
pub(crate) fn is_selected(scene: &Scene, id: WidgetId) -> bool {
    scene
        .a11y(id)
        .map(|a| StateFlags(a.state).contains(StateFlags::SELECTED))
        .unwrap_or(false)
}

/// Sets `id`'s SELECTED bit (SOUL §6.1).
pub(crate) fn set_selected(scene: &mut Scene, id: WidgetId) {
    let mut s = StateFlags(scene.a11y(id).map_or(0, |a| a.state));
    s.insert(StateFlags::SELECTED);
    scene.set_a11y_state(id, s.0);
}

/// Clears `id`'s SELECTED bit (SOUL §6.1).
pub(crate) fn clear_selected(scene: &mut Scene, id: WidgetId) {
    let mut s = StateFlags(scene.a11y(id).map_or(0, |a| a.state));
    s.0 &= !StateFlags::SELECTED.0;
    scene.set_a11y_state(id, s.0);
}
