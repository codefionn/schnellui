//! Navigation/selection components (SOUL §8.1): [`TabBar`] + [`Tab`] and [`List`] +
//! [`ListItem`]. The bars/lists are **semantic containers** — like `Scroll`, they
//! derive geometry from their children but carry a role of their own
//! ([`Role::TabList`] / [`Role::List`], SOUL §6.1). The tabs/items are content
//! leaves with `StateFlags::SELECTED` and sibling-exclusive selection routed through
//! the *same* inbound path as an AccessKit `Click` `ActionRequest` (SOUL §6.3),
//! mirroring the radio group's discipline in [`crate::basic`].
//!
//! **Selection recolors, it never re-shapes** (SOUL Directive #3, §4): a tab/item's
//! paint is emitted once at build with its background (and the tab's indicator bar)
//! as dedicated leading primitives; a selection toggle mutates only those
//! primitives' colours in place — the label glyphs are untouched, so no shaper, no
//! atlas, and no heap are involved on the selection path.

use std::borrow::Cow;

use schnellui_a11y::{ActionFlags, Role, StateFlags};
use schnellui_layout::{Container, ContainerStyle, LayoutEngine};
use schnellui_scene::{
    Color, DirtyFlags, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind,
};
use schnellui_text::{GlyphAtlas, TextShaper};
use smallvec::SmallVec;

use crate::{
    node_rect, norm_scale, phys_size_px, rasterize_and_push, theme_for, with_handlers, AnyView,
    BuildCtx, ClickHandler, ContextMenu, ContextMenuItem, InputHandler, View, BUTTON_TEXT_SIZE,
    PAD_H, PAD_V,
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
const TAB_DISCLOSURE_SPACE: f32 = 14.0;
const TAB_DISCLOSURE_HALF: f32 = 3.0;
const TAB_DISCLOSURE_STROKE: f32 = 1.5;
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

fn tab_appearance(runtime: &crate::Runtime, id: WidgetId) -> TabAppearance {
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
    runtime: &crate::Runtime,
    id: WidgetId,
    kind: WidgetKind,
    selected: bool,
) -> (Color, Option<Color>) {
    let t = theme_for(runtime, id);
    let bg = if selected {
        t.selection
    } else if kind == WidgetKind::Tab && tab_appearance(runtime, id) == TabAppearance::Navigation {
        Color::TRANSPARENT
    } else {
        t.surface
    };
    match kind {
        WidgetKind::Tab => {
            let ind = if selected {
                t.accent
            } else {
                Color::TRANSPARENT
            };
            (bg, Some(ind))
        }
        _ => (bg, None),
    }
}

/// The ink-frame width a selectable leaf wears: only dropdown options do — the
/// popup panel gets its edges from them (SOUL §8.1) — tabs and list items stay
/// flat chrome in every design system.
fn selectable_frame(runtime: &crate::Runtime, id: WidgetId, kind: WidgetKind) -> f32 {
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
/// ([`Shape::frame`](crate::Shape::frame)) a dropdown option also wears frame
/// strips *on top of* its surface — left + right panel edges and a bottom rule
/// (the panel's top edge is the trigger's own bottom frame) — so the surface
/// stays primitive `[0]`, the in-place recolor target of a selection toggle.
/// Returns the label's **logical** text size (for the intrinsic-measure closure).
#[allow(clippy::too_many_arguments)]
fn emit_selectable_paint(
    runtime: &crate::Runtime,
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
    let phys = phys_size_px(BUTTON_TEXT_SIZE, scale);
    let shaped = shaper.shape(label, phys, None);
    let ts = Size {
        width: shaped.width * inv,
        height: shaped.height * inv,
    };
    let sh = theme_for(runtime, id).shape;
    let (pad_h, pad_v) = (sh.pad(PAD_H), sh.pad(PAD_V));
    let f = selectable_frame(runtime, id, kind);
    let disclosure_width = if disclosure == TabDisclosure::None {
        0.0
    } else {
        TAB_DISCLOSURE_SPACE
    };
    let intrinsic = Size {
        width: (ts.width + 2.0 * (pad_h + f) + disclosure_width).max(min_width),
        height: ts.height + 2.0 * pad_v,
    };
    let rect = node_rect(scene, id, intrinsic);
    let (bg, indicator) = selection_colors(runtime, id, kind, selected);
    let appearance = if kind == WidgetKind::Tab {
        tab_appearance(runtime, id)
    } else {
        TabAppearance::Classic
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
    // system ([`Shape::frame`](crate::Shape::frame)) thickens it to match.
    if let Some(ind) = indicator {
        let thickness = TAB_INDICATOR + sh.frame;
        let indicator_rect = match appearance {
            TabAppearance::Classic => Rect::new(
                rect.x,
                rect.y + rect.height - thickness,
                rect.width,
                thickness,
            ),
            TabAppearance::Navigation => {
                let inset = sh.pad(4.0).min(rect.height * 0.25);
                Rect::new(
                    rect.x,
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
                    y: cy - 1.5,
                },
                Point { x: cx, y: cy + 1.5 },
                Point {
                    x: cx + TAB_DISCLOSURE_HALF,
                    y: cy - 1.5,
                },
            ]
        } else {
            [
                Point {
                    x: cx - 1.5,
                    y: cy - TAB_DISCLOSURE_HALF,
                },
                Point { x: cx + 1.5, y: cy },
                Point {
                    x: cx - 1.5,
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
            y: rect.y + pad_v,
        },
    );
    ts
}

/// Recolors a tab's / item's selection surfaces **in place** (SOUL Directive #3):
/// primitive `[0]` is the background, `[1]` (tabs only) the indicator bar. No
/// re-shape, no primitive growth, no heap touch.
fn recolor_selection(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    kind: WidgetKind,
    selected: bool,
) {
    let (bg, indicator) = selection_colors(runtime, id, kind, selected);
    let pd = scene.paint_mut(id);
    if let Some(Primitive::SolidRect { color, .. }) = pd.primitives.get_mut(0) {
        *color = bg;
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
pub(crate) fn resize_tab_surface(runtime: &crate::Runtime, scene: &mut Scene, id: WidgetId) {
    if scene
        .node(id)
        .is_none_or(|node| node.kind != WidgetKind::Tab)
    {
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
    } else if tab_appearance(runtime, id) != TabAppearance::Navigation {
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
        indicator_rect.x = rect.x;
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
    let a = scene.a11y_mut(id);
    let mut s = StateFlags(a.state);
    s.insert(StateFlags::SELECTED);
    a.state = s.0;
}

/// Clears `id`'s SELECTED bit (SOUL §6.1).
pub(crate) fn clear_selected(scene: &mut Scene, id: WidgetId) {
    let a = scene.a11y_mut(id);
    let mut s = StateFlags(a.state);
    s.0 &= !StateFlags::SELECTED.0;
    a.state = s.0;
}
