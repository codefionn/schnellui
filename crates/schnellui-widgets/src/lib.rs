//! # schnellui-widgets
//!
//! The content-primitive **typed builder chain** (SOUL §3.3, §8.1) — the macro's
//! codegen target, frozen thoughtfully because `view!` expands to exactly these
//! calls: `Column::new().child(Text::new("…"))`, `Button::new(label).on_click(…)`,
//! `Text::dynamic(move || …)` for signal-bound slots.
//!
//! Widgets answer *what* is on screen (SOUL §8.1): they draw pixels, **always**
//! carry an AccessKit role + name/value/state/actions, and handle content input.
//! Layout (*where*/*how big*) is a separate crate; a widget only *measures* itself
//! and hands an intrinsic size up. Every builder resolves to a
//! [`WidgetId`](schnellui_scene::WidgetId) in the retained scene when `build`-t.
//!
//! ## The widget-interaction registry (SOUL §3.3, §6.3)
//!
//! The frozen [`View::build`] signature and [`BuildCtx`] give no channel to hand a
//! widget's **content-input handlers** and **dynamic render-effect slots** back to
//! the caller, yet both must be *retained* to fire pointer/action input (§6.3) and
//! to update signal-bound text in place (§3.3). They cannot live in the
//! [`Scene`](schnellui_scene::Scene) (it owns only render-ready ECS columns) nor in
//! the reactive arena (its compute cells are `Send`; widget closures are not). So
//! each SchnellUI `App` owns a [`Runtime`]: `build` populates its currently active
//! runtime,
//! [`dispatch_click`]/[`run_dynamic_slots`] drain it. Because every closure it holds
//! is `!Send`, it can never be part of the `Send` *rendering* path — so it does
//! **not** foreclose the multithreaded rendering Directive #7 protects (that rule
//! bans a `!Send`-forcing *signal* store; the render-ready scene stays `Send`).
//!
//! Dynamic widget producers are tracked by a per-runtime signal subscription
//! group. [`run_dynamic_slots`] drains only the resulting ready widget ids;
//! producer closures remain app-owned and `!Send`, never inside the signal
//! arena. The public surface ([`run_dynamic_slots`], [`dispatch_click`], [`dispatch_focus`],
//! [`dispatch_edit_key`], [`hit_test`], [`cursor_at`], [`reset`]) is what the umbrella's
//! `App::frame`/`dispatch_action` and the windowed event loop wire (SOUL §6.3).

mod basic;
pub use basic::*;
mod context_menu;
pub use context_menu::*;
mod context;
pub use context::Context;
mod dialog;
pub use dialog::*;
mod grouped_tabs;
pub use grouped_tabs::*;
mod panel;
pub use panel::*;
mod selection;
pub use selection::*;
mod svg;
pub use svg::*;
mod table;
pub use table::*;
mod template;
pub use template::SceneTemplate;
mod terminal_grid;
pub use terminal_grid::*;
mod rich;
pub use rich::*;
mod text_area;
pub use text_area::*;
mod text_edit;
pub use text_edit::*;
mod theme;
pub use theme::*;
mod virtual_list;
pub use virtual_list::*;

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub use schnellui_a11y::SortDirection;
use schnellui_a11y::{ActionFlags, Role, StateFlags};
use schnellui_layout::{Container, ContainerStyle, EdgeInsets, FlexChild, LayoutEngine};
use schnellui_scene::{
    Color, DirtyFlags, PaintData, Point, Primitive, Rect, Scene, Size, TexelRect, WidgetId,
    WidgetKind,
};
use schnellui_text::{GlyphAtlas, ShapeOptions, ShapedText, TextShaper};
use slotmap::SecondaryMap;
use smallvec::SmallVec;

/// Re-exported so `Text::wrap`/`Text::align` callers name the modes without a
/// direct `schnellui-text` dependency (SOUL §8.1).
pub use schnellui_text::{TextAlign, WrapMode};

pub use schnellui_layout::{em, px, Length, ResponsiveQuery, ResponsiveTarget};
/// Re-exported so container `justify`/`align` callers — and the `view!` macro's
/// keyword lowering (`row(justify = space_between)`) — name the flex enums without
/// a direct `schnellui-layout` dependency (SOUL §8.1).
pub use schnellui_layout::{Align, Justify};
pub use schnellui_scene::ComponentRef;

#[path = "lib/paint.rs"]
mod paint;
pub(crate) use paint::*;
pub use paint::{emit_text_paint, node_rect, rasterize_and_push, rasterize_lines_and_push};
#[path = "lib/frame.rs"]
mod frame;
pub use frame::*;
#[path = "lib/scroll.rs"]
mod scroll;
pub use scroll::*;
#[path = "lib/interaction.rs"]
mod interaction;
pub use interaction::*;
#[path = "lib/drag.rs"]
mod drag;
pub use drag::*;
#[path = "lib/layout.rs"]
mod layout;
pub use layout::*;
#[path = "lib/text.rs"]
mod text;
pub use text::*;
#[path = "lib/controls.rs"]
mod controls;
pub use controls::*;
#[path = "lib/media.rs"]
mod media;
use media::DynamicImageState;
pub use media::*;
#[path = "lib/reactivity.rs"]
mod reactivity;
use reactivity::RetainedReactivity;
#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;

/// Window-system-independent pointer cursor requested by the widget under the
/// pointer. Windowed hosts translate this semantic set to their native cursor
/// API; headless callers can inspect it in interaction tests. The variants
/// mirror CSS/native cursor names so embedded surfaces can preserve their cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorIcon {
    /// Hide the cursor while it is over the requesting surface.
    None,
    /// The platform's ordinary arrow cursor.
    #[default]
    Default,
    ContextMenu,
    Help,
    /// An enabled control that activates when pressed.
    Pointer,
    Progress,
    Wait,
    Cell,
    Crosshair,
    /// Editable text.
    Text,
    VerticalText,
    Alias,
    Copy,
    Move,
    NoDrop,
    NotAllowed,
    /// Movable dialog chrome before capture.
    Grab,
    /// Movable dialog chrome while captured.
    Grabbing,
    EResize,
    NResize,
    NeResize,
    NwResize,
    SResize,
    SeResize,
    SwResize,
    WResize,
    /// A bottom-right dialog resize handle.
    NwseResize,
    /// A horizontal value control such as a slider.
    EwResize,
    NsResize,
    NeswResize,
    ColResize,
    RowResize,
    AllScroll,
    ZoomIn,
    ZoomOut,
}

/// The shared build context threaded through the tree (SOUL §8.1). Holds the ECS
/// scene plus the layout + text engines and the shared glyph atlas, so a widget can
/// register its geometry, shape+rasterize its text into real glyph quads, and
/// register its intrinsic-size measure while it inserts its node.
pub struct BuildCtx<'a> {
    /// Application dependencies for this subtree. Child modules may explicitly
    /// derive and pass an inline scope without affecting siblings.
    pub context: Context,
    /// Retained non-rendering behavior owned by the application being built.
    pub runtime: Runtime,
    pub scene: &'a mut Scene,
    pub layout: &'a mut LayoutEngine,
    /// Pooled shaper (SOUL §8.1) — text is shaped through it at build + paint-dirty.
    pub text: &'a mut TextShaper,
    /// Shared R8 glyph atlas (SOUL §8.1, §3.2) — needed glyphs are rasterized here.
    pub atlas: &'a mut GlyphAtlas,
    /// Logical→physical scale factor (SOUL §7.1 `--scale`). Text is shaped and
    /// rasterized at `size_px * scale` for crisp physical pixels; positions stay
    /// logical (the renderer re-applies `scale` at draw). `1.0` for standard shots.
    pub scale: f32,
}

/// Everything the builder chain produces implements `View` (SOUL §3.3). Object-safe
/// (boxed `self`) so containers can hold heterogeneous children.
pub trait View: 'static {
    /// Inserts this view's node(s) into the scene under `parent`, wiring any
    /// dynamic slots to `create_effect`s (SOUL §3.3), and returns the root id.
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId;

    /// Sets a minimum outer width on any component without adding a scene node.
    ///
    /// The returned [`Flex`] wrapper registers the constraint directly on this
    /// view's root node, so it works for content leaves and containers alike.
    fn min_width(self, width: f32) -> Flex
    where
        Self: Sized,
    {
        Flex::new().min_width(width).child(self)
    }

    /// Sets a minimum outer height on any component without adding a scene node.
    fn min_height(self, height: f32) -> Flex
    where
        Self: Sized,
    {
        Flex::new().min_height(height).child(self)
    }

    /// Shows this component only while `query` matches the live viewport or its
    /// immediate parent container. The wrapper inserts no scene node.
    ///
    /// ```
    /// # use schnellui_widgets::{em, Button, ResponsiveQuery, View as _};
    /// let desktop_action = Button::new("Export").show_when(
    ///     ResponsiveQuery::viewport().min_width(em(48.0))
    /// );
    /// ```
    fn show_when(self, query: ResponsiveQuery) -> Responsive
    where
        Self: Sized,
    {
        Responsive::new(query).child(self)
    }

    /// Attaches a stable application-created reference to this component without
    /// inserting a scene node.
    fn with_ref(self, reference: ComponentRef) -> Referenced
    where
        Self: Sized,
    {
        Referenced::new(reference).child(self)
    }
}

/// A type-erased child in a container (SOUL §3.3).
pub type AnyView = Box<dyn View>;

/// A content-input handler — the identical closure a pointer *and* an inbound
/// AccessKit `ActionRequest` fire (SOUL §6.3).
pub type ClickHandler = Box<dyn FnMut() + 'static>;

/// A content-input handler for editable text widgets.
type InputHandler = Box<dyn FnMut(&str) + 'static>;

/// A signal-bound text producer for a dynamic slot (`Text::dynamic`, SOUL §3.3).
pub type TextFn = Box<dyn FnMut() -> String + 'static>;

/// A signal-bound value producer for numeric widgets.
pub type ValueFn<T> = Box<dyn FnMut() -> T + 'static>;

/// Result of releasing a pointer that may have started on a drag source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragRelease {
    /// No drag source owned the press.
    None,
    /// The pointer never crossed the drag threshold; activate this widget as a
    /// normal click.
    Click(WidgetId),
    /// A real drag ended. `accepted` reports whether it landed on a drop target.
    Drop { accepted: bool },
}

/// Read-only snapshot of pointer captures owned by the widget runtime.
///
/// Native hosts use this to diagnose interaction streams without exposing the
/// widget runtime's private drag and dialog implementation details.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InteractionDebugState {
    /// Widget that owns a possible or active content drag.
    pub content_drag_source: Option<WidgetId>,
    /// Whether that content drag has crossed the activation threshold.
    pub content_drag_active: bool,
    /// Whether dialog chrome currently owns the pointer for move/resize.
    pub dialog_pointer_capture: bool,
}

/// Spatial result of dropping inside a [`DockArea`]. The center means "join this
/// pane"; edge positions mean "split the pane on this side".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockPosition {
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DropHover {
    target: WidgetId,
    preview: WidgetId,
    position: DockPosition,
    reorder: Option<TabReorderHover>,
    /// Number of preview primitives appended to `target` and removed on leave.
    preview_prims: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabReorderHover {
    bar: WidgetId,
    from: usize,
    to: usize,
}

#[derive(Clone, Copy, Debug)]
struct DragPointerCapture {
    source: WidgetId,
    origin: Point,
    active: bool,
    hovered: Option<DropHover>,
}

struct ScrollState {
    scrollbar: bool,
    edge_auto_scroll: bool,
    follow_end: bool,
    restoration_key: Option<Cow<'static, str>>,
    debounced: Option<DebouncedScroll>,
}

/// Trailing-edge scroll notification retained by its viewport. Keeping it beside
/// the viewport state makes detached scroll nodes cancel naturally when their
/// runtime record is purged.
struct DebouncedScroll {
    delay: Duration,
    max_wait: Duration,
    /// Taken before user code runs, then restored if the viewport still exists.
    /// `Option` avoids allocating a dummy callback at every due deadline.
    callback: Option<Box<dyn FnMut(f32) + 'static>>,
    burst_start: Option<Instant>,
    deadline: Option<Instant>,
    latest_offset: f32,
}

#[derive(Clone, Copy, Debug)]
struct ScrollbarPointerCapture {
    id: WidgetId,
    /// Pointer distance from the thumb's leading edge at capture time.
    grab_offset: f32,
}

#[derive(Clone, Copy, Debug)]
struct EdgeAutoScrollState {
    id: WidgetId,
    direction: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabReorderItem {
    bar: WidgetId,
    index: usize,
}

type TabReorderHandler = Box<dyn FnMut(usize, usize) + 'static>;

// ---------------------------------------------------------------------------
// visual + metric constants (deterministic for shots, SOUL §7.3)
// ---------------------------------------------------------------------------

/// Fallback line-box height as a fraction of the em — used only to reserve a
/// sensible box height for an *empty* editable field (real text takes its height
/// from the shaper, SOUL §8.1).
const EMPTY_LINE_RATIO: f32 = 1.2;
/// Button horizontal padding (each side).
const PAD_H: f32 = 8.0;
/// Button vertical padding (each side).
const PAD_V: f32 = 4.0;
/// Label font size for buttons.
const BUTTON_TEXT_SIZE: f32 = 16.0;
/// Checkbox edge length.
const CHECKBOX_SIZE: f32 = 18.0;
/// Native vertical scrollbar geometry, in logical pixels.
const SCROLLBAR_WIDTH: f32 = 10.0;
const SCROLLBAR_INSET: f32 = 2.0;
const SCROLLBAR_MIN_THUMB: f32 = 20.0;
/// Pointer-edge scrolling uses a deliberately small, stable activation strip.
const EDGE_AUTO_SCROLL_ZONE: f32 = 24.0;
const EDGE_AUTO_SCROLL_STEP: f32 = 12.0;

// Colours live in the app runtime's design system (SOUL §8.1): widgets read
// [`theme`] tokens at build/dispatch — no colour constants remain here.

// ---------------------------------------------------------------------------
// intrinsic text measurement + glyph emission (SOUL §8.1, §3.2)
// ---------------------------------------------------------------------------

/// Normalizes a scale factor to a strictly-positive value (defends against a `0`
/// or negative `--scale` divide-by-zero — SOUL §7.1).
#[inline]
fn norm_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The integer physical pixel size a `size_px` logical run is rasterized at under
/// `scale` (SOUL §7.1 `--scale`). Glyphs are shaped/rasterized at physical pixels
/// for crispness; the atlas key carries this size so distinct scales are distinct
/// atlas entries (SOUL §8.1).
#[inline]
fn phys_size_px(size_px: f32, scale: f32) -> f32 {
    (size_px * norm_scale(scale)).round().max(1.0)
}

/// The rect a node paints into: its computed [`LayoutBox`](schnellui_scene::LayoutBox)
/// rect once layout has run, else a provisional origin box of its intrinsic size
/// (mount happens before the first layout pass — SOUL §8.1 pass order).
///
/// Public so external widget crates (e.g. `schnellui-charts`) resolve their own paint
/// rect the same way the built-in leaves do, instead of re-deriving the layout/mount
/// fallback rule.
const TOOLTIP_GAP: f32 = 6.0;
const TOOLTIP_PAD_X: f32 = 7.0;
const TOOLTIP_PAD_Y: f32 = 4.0;
const TOOLTIP_VIEWPORT_MARGIN: f32 = 4.0;

#[derive(Clone, Copy)]
struct HoverTooltipState {
    base_primitive_end: usize,
    background: usize,
    glyph_start: usize,
    glyph_end: usize,
    size: Size,
    background_color: Color,
    text_color: Color,
    visible: bool,
}

fn hover_tooltip_origin(target: Rect, tooltip: Size, viewport: Rect) -> Point {
    let left = viewport.x + TOOLTIP_VIEWPORT_MARGIN;
    let right = (viewport.right() - TOOLTIP_VIEWPORT_MARGIN - tooltip.width).max(left);
    let x = (target.right() - tooltip.width).clamp(left, right);

    let top = viewport.y + TOOLTIP_VIEWPORT_MARGIN;
    let bottom = viewport.bottom() - TOOLTIP_VIEWPORT_MARGIN;
    let above = target.y - TOOLTIP_GAP - tooltip.height;
    let below = target.bottom() + TOOLTIP_GAP;
    let y = if above >= top {
        above
    } else if below + tooltip.height <= bottom {
        below
    } else {
        above.clamp(top, (bottom - tooltip.height).max(top))
    };

    Point { x, y }
}

/// Repositions only the tooltip tail of a node's paint after final layout is
/// known. The preferred placement remains right-aligned above the target, but
/// edge controls clamp horizontally and flip below when the top edge is tight.
fn position_hover_tooltip(runtime: &Runtime, scene: &mut Scene, id: WidgetId) {
    let tooltip = runtime.with(|registry| registry.borrow().hover_tooltips.get(id).copied());
    let Some(tooltip) = tooltip else { return };
    let Some(root) = scene.root() else { return };
    let Some(viewport) = scene.layout(root).map(|layout| layout.rect) else {
        return;
    };
    let Some(target) = scene.layout(id).map(|layout| layout.rect) else {
        return;
    };
    let Some(Primitive::SolidRect { rect, .. }) = scene
        .paint(id)
        .and_then(|paint| paint.primitives.get(tooltip.background))
    else {
        return;
    };
    let origin = hover_tooltip_origin(target, tooltip.size, viewport);
    let dx = origin.x - rect.x;
    let dy = origin.y - rect.y;
    if dx.abs() < 0.001 && dy.abs() < 0.001 {
        return;
    }
    let paint = scene.paint_mut(id);
    for primitive in &mut paint.primitives[tooltip.background..tooltip.glyph_end] {
        match primitive {
            Primitive::SolidRect { rect, .. }
            | Primitive::GlyphQuad { rect, .. }
            | Primitive::ImageQuad { rect, .. } => {
                rect.x += dx;
                rect.y += dy;
            }
            Primitive::Line { from, to, .. } => {
                from.x += dx;
                from.y += dy;
                to.x += dx;
                to.y += dy;
            }
        }
    }
}

/// Appends a pre-rasterized tooltip fragment in a transparent state. Pointer
/// proximity only recolors these stable primitives, so showing a tooltip needs
/// no shaping, atlas work, or allocation on the interaction path.
fn emit_hover_tooltip(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    label: &str,
    button_size: Size,
    scale: f32,
) -> HoverTooltipState {
    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(TOOLTIP_TEXT_SIZE, scale);
    let shaped = shaper.shape(label, phys, None);
    let text_size = Size {
        width: shaped.width * inv,
        height: shaped.height * inv,
    };
    let tooltip_size = Size {
        width: text_size.width + 2.0 * TOOLTIP_PAD_X,
        height: text_size.height + 2.0 * TOOLTIP_PAD_Y,
    };
    // Right-align above the hit target. This keeps compact actions at a
    // navigation row's trailing edge without spilling the label into adjacent
    // content columns.
    let origin = Point {
        x: button_size.width - tooltip_size.width,
        y: -tooltip_size.height - TOOLTIP_GAP,
    };
    let theme = theme_for(runtime, id);
    let pd = scene.paint_mut(id);
    let base_primitive_end = pd.primitives.len();
    let background = pd.primitives.len();
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(origin.x, origin.y, tooltip_size.width, tooltip_size.height),
        color: Color::TRANSPARENT,
        corner_radius: theme.shape.radius(4.0, tooltip_size.height),
    });
    let glyph_start = pd.primitives.len();
    rasterize_and_push(
        pd,
        shaper,
        atlas,
        &shaped,
        phys as u32,
        Color::TRANSPARENT,
        scale,
        Point {
            x: origin.x + TOOLTIP_PAD_X,
            y: origin.y + TOOLTIP_PAD_Y,
        },
    );
    HoverTooltipState {
        base_primitive_end,
        background,
        glyph_start,
        glyph_end: pd.primitives.len(),
        size: tooltip_size,
        background_color: theme.text,
        text_color: theme.surface,
        visible: false,
    }
}

/// Pre-rasterizes and registers hover text for a widget. Buttons opt into this
/// with [`Button::tooltip`]; meaningful raster and vector images register their
/// [`Image::alt`] / [`Svg::alt`](crate::Svg::alt) text automatically.
const TOOLTIP_OVERLAY_LEVEL: u8 = 5;

pub(crate) fn register_hover_tooltip(
    runtime: &Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    label: &str,
    target_size: Size,
    scale: f32,
) {
    if label.is_empty() {
        return;
    }
    let tooltip = emit_hover_tooltip(runtime, scene, shaper, atlas, id, label, target_size, scale);
    runtime.with(|registry| {
        registry.borrow_mut().hover_tooltips.insert(id, tooltip);
    });
    // Hover tooltips extend beyond their button's layout box and must
    // remain visible when the button sits at a panel edge (e.g. the
    // settings sidebar's trailing edge next to a second sidebar).
    // Painting the host widget as an overlay keeps the pre-rasterized
    // tooltip tail above sibling panels without a separate tooltip
    // layer or per-frame allocation.
    scene.set_overlay_level(id, TOOLTIP_OVERLAY_LEVEL);
}

/// A checkbox's intrinsic box under the ambient shape tokens: the classic edge
/// scaled by density, grown by the ink frame (SOUL §8.1).
fn checkbox_intrinsic(runtime: &Runtime, id: WidgetId) -> Size {
    let sh = theme_for(runtime, id).shape;
    let edge = sh.pad(CHECKBOX_SIZE) + 2.0 * sh.frame;
    Size {
        width: edge,
        height: edge,
    }
}

/// Emits a checkbox's paint: optional ink frame, box surface + check mark when
/// checked (SOUL §8.1). All geometry derives from the laid-out rect, so the
/// density-scaled box keeps its proportions.
fn emit_checkbox_paint(runtime: &Runtime, scene: &mut Scene, id: WidgetId, checked: bool) {
    let rect = node_rect(scene, id, checkbox_intrinsic(runtime, id));
    let t = theme_for(runtime, id);
    let sh = t.shape;
    let radius = sh.radius(3.0, rect.height);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    if sh.frame > 0.0 {
        pd.primitives.push(Primitive::SolidRect {
            rect,
            color: t.outline,
            corner_radius: radius,
        });
    }
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(
            rect.x + sh.frame,
            rect.y + sh.frame,
            (rect.width - 2.0 * sh.frame).max(0.0),
            (rect.height - 2.0 * sh.frame).max(0.0),
        ),
        color: t.surface,
        corner_radius: (radius - sh.frame).max(0.0),
    });
    if checked {
        let inset = sh.frame + sh.pad(4.0);
        pd.primitives.push(Primitive::SolidRect {
            rect: Rect::new(
                rect.x + inset,
                rect.y + inset,
                (rect.width - 2.0 * inset).max(0.0),
                (rect.height - 2.0 * inset).max(0.0),
            ),
            color: t.positive,
            corner_radius: sh.radius(1.0, rect.height - 2.0 * inset),
        });
    }
}

/// Slider intrinsic track size (SOUL §8.1).
const SLIDER_SIZE: Size = Size {
    width: 120.0,
    height: 20.0,
};

/// A slider's intrinsic track size under the ambient shape tokens: classic
/// width, density-scaled height (SOUL §8.1).
fn slider_intrinsic(runtime: &Runtime, id: WidgetId) -> Size {
    Size {
        width: SLIDER_SIZE.width,
        height: theme_for(runtime, id).shape.pad(SLIDER_SIZE.height),
    }
}

/// Emits a slider's paint: a centered rail, its accent fill, and a tactile thumb.
/// Cleared-and-refilled in place (§4.4); shared by build, keyboard/a11y adjustment,
/// and pointer scrubbing.
fn emit_slider_paint(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    frac: f32,
    disabled: bool,
) {
    let rect = node_rect(scene, id, slider_intrinsic(runtime, id));
    let t = theme_for(runtime, id);
    let thumb = t.shape.pad(16.0).min(rect.height).max(8.0);
    let rail_h = t.shape.pad(6.0).min(rect.height);
    let rail = Rect::new(
        rect.x + thumb * 0.5,
        rect.y + (rect.height - rail_h) * 0.5,
        (rect.width - thumb).max(0.0),
        rail_h,
    );
    let fill_w = rail.width * frac;
    let thumb_x = rail.x + fill_w;
    let radius = t.shape.pill(rail_h);
    let track_color = if disabled { t.surface_muted } else { t.media };
    let active_color = if disabled { t.disabled } else { t.accent };
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    pd.primitives.push(Primitive::SolidRect {
        rect: rail,
        color: track_color,
        corner_radius: radius,
    });
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(rail.x, rail.y, fill_w, rail.height),
        color: active_color,
        corner_radius: radius,
    });
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(
            thumb_x - thumb * 0.5,
            rect.y + (rect.height - thumb) * 0.5,
            thumb,
            thumb,
        ),
        color: active_color,
        corner_radius: t.shape.pill(thumb),
    });
}

/// Emits a solid placeholder box for media leaves (image/icon) (SOUL §8.1).
fn emit_media_paint(runtime: &Runtime, scene: &mut Scene, id: WidgetId, intrinsic: Size) {
    let rect = node_rect(scene, id, intrinsic);
    let t = theme_for(runtime, id);
    let radius = t.shape.radius(2.0, rect.height);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: t.media,
        corner_radius: radius,
    });
}

// ---------------------------------------------------------------------------
// the app-owned widget-interaction runtime
// ---------------------------------------------------------------------------

/// Retained content-input handlers for one widget (SOUL §6.3). One inbound path for
/// pointer *and* AccessKit `ActionRequest`.
#[derive(Default)]
struct Handlers {
    click: Option<ClickHandler>,
    /// Fired when a sortable table column header requests a new direction.
    sort: Option<Box<dyn FnMut(SortDirection) + 'static>>,
    toggle: Option<Box<dyn FnMut(bool) + 'static>>,
    change: Option<Box<dyn FnMut(f32) + 'static>>,
    input: Option<InputHandler>,
    /// Pointer-drag lifecycle for dockable/reorderable content. Drag input is
    /// deliberately separate from `click`: a short press still activates the
    /// widget, while crossing the movement threshold starts capture.
    drag_start: Option<ClickHandler>,
    drag_end: Option<Box<dyn FnMut(bool) + 'static>>,
    /// Fired when an active drag is released over this widget.
    drop: Option<ClickHandler>,
    /// Position-aware pane docking. Unlike a small explicit drop control, this
    /// turns the whole area into an implicit center/edge target.
    dock: Option<Box<dyn FnMut(DockPosition) + 'static>>,
    /// Pointer-transparent, last-painted child used for a dock area's preview.
    dock_preview: Option<WidgetId>,
    /// fired with the new vertical offset after a scroll (SOUL §6.3) — the same
    /// handler wheel input and an inbound `ScrollUp`/`ScrollDown` action both reach.
    scroll: Option<Box<dyn FnMut(f32) + 'static>>,
}

/// One dynamic text slot: a signal-bound producer whose value mutates the retained
/// node in place on change (SOUL §3.3 `RenderEffect`). Readiness and dependency
/// tracking live in the runtime's retained subscription module.
struct DynSlot {
    /// the producer; `take`-n out before running so no registry borrow is held
    /// across user code (mirrors the never-hold-lock-across-user-code rule, §3.1).
    f: Option<TextFn>,
    /// last produced string, for change suppression (§3.1 equality gate).
    last: String,
    /// the last *shaped logical size*, shared with this node's `MeasureFn` so a size
    /// change re-measures (§8.1). Re-shaping owns the metric now (glyph-exact), so
    /// the measure closure just reads this cached size — no heuristic.
    shared: Rc<RefCell<Size>>,
    size_px: f32,
    role: Role,
    /// logical→physical scale captured at build (SOUL §7.1 `--scale`).
    scale: f32,
    /// `true` when this slot's node is a **wrapping/aligned/ellipsis** text leaf
    /// (SOUL §8.1): its paint is owned by [`emit_wrapped_paint`] (a width-aware,
    /// post-layout pass), so on a value change this slot only updates the
    /// [`TextLayout`] text + flags the channels — it never runs the single-line
    /// [`emit_text_paint`]/[`reposition_node`] path.
    wrapped: bool,
}

/// The retained shaping config + text for a **wrapping / aligned / ellipsis** text
/// leaf (SOUL §8.1). Unlike a single-line leaf — whose glyphs are emitted once at
/// build and only *slid* onto their laid-out origin — a wrapped leaf's line breaks
/// (and therefore its glyphs and height) depend on the available width, which is not
/// known until layout runs. So its paint is deferred: the width-aware measure
/// ([`measure_text`]) shapes it during Taffy's measure pass to report a height, and
/// [`emit_wrapped_paint`] re-shapes at the laid-out width to emit the multi-line
/// glyphs. Both share this entry.
struct TextLayout {
    wrap: WrapMode,
    align: TextAlign,
    ellipsis: bool,
    size_px: f32,
    scale: f32,
    /// the text to shape; a `Text::dynamic` slot overwrites this on change.
    text: String,
    /// grow-only `(logical avail width → logical size)` measure cache (SOUL §4.4):
    /// a same-width relayout re-shapes nothing. Invalidated (cleared, capacity kept)
    /// when the text changes.
    cache: SmallVec<[(f32, Size); 4]>,
    /// the node's rect at the last paint emission, so [`emit_wrapped_paint`] re-emits
    /// only when the box actually moved/resized (idempotent, zero-alloc when stable).
    last_emit: Option<Rect>,
    /// text changed since the last emission ⇒ force a re-shape + re-emit.
    dirty: bool,
}

impl TextLayout {
    /// Shapes `self.text` for a given **logical** available width, honouring the wrap
    /// mode / alignment / ellipsis. Shaping happens at physical px (SOUL §7.1), so the
    /// width constraint is scaled up; callers divide the physical metrics back to
    /// logical themselves. An infinite/NaN width means unconstrained (single line).
    fn shape(&self, shaper: &mut TextShaper, avail_w_logical: f32) -> ShapedText {
        let phys = phys_size_px(self.size_px, self.scale);
        let sc = norm_scale(self.scale);
        let max_w = if avail_w_logical.is_finite() {
            Some(avail_w_logical.max(0.0) * sc)
        } else {
            None
        };
        if self.ellipsis {
            match max_w {
                Some(mw) => shaper.truncate_to_width(&self.text, phys, mw),
                None => shaper.shape(&self.text, phys, None),
            }
        } else {
            let opts = ShapeOptions::new(phys)
                .max_width(max_w)
                .wrap(self.wrap)
                .align(self.align);
            shaper.shape_with(&self.text, &opts)
        }
    }
}

/// A slider's retained range state (SOUL §6.3): what a keyboard/AccessKit
/// `Increment`/`Decrement` adjusts. Lives in the registry (not the scene) because
/// the scene columns carry only render-ready data; min/max never paint directly.
struct SliderState {
    value: f32,
    min: f32,
    max: f32,
    step: f32,
}

/// Retained animation state for one [`LoadingSpinner`]. The visual phase
/// advances in-place; `size` is retained so repaint never needs to inspect
/// builder data.
///
/// `phase` is a *continuous* rotation in `[0,1)` turns sampled from the shared
/// [`SPINNER_MOTION`] declaration against a monotonic clock, so the animation
/// speed is frame-rate independent (one full revolution per 900ms on every
/// display) instead of one discrete segment per frame.
pub(crate) struct SpinnerState {
    pub(crate) phase: f32,
    pub(crate) size: f32,
    pub(crate) animated: bool,
}

#[derive(Clone, Copy)]
struct ProximityRevealState {
    distance: f32,
    visible: bool,
}

#[derive(Clone, Copy)]
struct PrimitiveColorRestore {
    owner: WidgetId,
    index: usize,
    color: Color,
}

#[derive(Clone)]
struct AppliedInteraction {
    target: WidgetId,
    border_owner: Option<WidgetId>,
    foreground: Vec<PrimitiveColorRestore>,
    background: Option<(WidgetId, usize)>,
}

/// The retained widget-interaction data owned by one [`Runtime`].
#[derive(Default)]
struct WidgetRuntime {
    handlers: SecondaryMap<WidgetId, Handlers>,
    /// Per-tab visual treatment, retained so click-driven selection recolors use
    /// the same classic/navigation surface emitted at build.
    tab_appearances: SecondaryMap<WidgetId, TabAppearance>,
    /// Optional close button paired with a plain tab. The tab surface is extended
    /// beneath this sibling after layout so the two controls read as one tab.
    tab_close_buttons: SecondaryMap<WidgetId, WidgetId>,
    /// Reorder-enabled tab bars and the tab positions they control. Kept apart
    /// from ordinary drag handlers so local reordering can coexist with docking.
    tab_reorders: SecondaryMap<WidgetId, Option<TabReorderHandler>>,
    tab_reorder_items: SecondaryMap<WidgetId, TabReorderItem>,
    /// Targeted signal readiness for retained dynamic widget producers.  The
    /// closures themselves remain in their type-specific retained state below.
    reactivity: RetainedReactivity,
    slots: SecondaryMap<WidgetId, DynSlot>,
    /// Versioned pixel sources that update retained image atlas regions without
    /// rebuilding the surrounding view tree.
    dynamic_images: SecondaryMap<WidgetId, DynamicImageState>,
    /// Reused key snapshot for polling dynamic images without holding the runtime
    /// borrow across application callbacks. This grows only when a mount exceeds
    /// its previous image count, then stays allocation-free on clean frames.
    dynamic_image_scratch: Vec<WidgetId>,
    /// Retained slider range state (SOUL §6.3) — mutated by [`dispatch_adjust`].
    sliders: SecondaryMap<WidgetId, SliderState>,
    /// Per-viewport optional chrome and edge-scrolling behavior.
    scrolls: SecondaryMap<WidgetId, ScrollState>,
    /// Dense mounted-viewport index for wheel routing. `SecondaryMap::keys()`
    /// scans its sparse capacity (and therefore all text rows); this stays
    /// proportional to actual scroll containers instead.
    scroll_ids: Vec<WidgetId>,
    /// Dense mounted-dialog-layer index. Modal checks are part of pointer and
    /// wheel routing, so they must not scan a sparse map whose capacity follows
    /// the total scene size.
    dialog_layer_ids: Vec<WidgetId>,
    /// Active native scrollbar thumb drag, if any.
    scrollbar_pointer: Option<ScrollbarPointerCapture>,
    /// Held pointer currently inside an enabled viewport edge.
    edge_auto_scroll: Option<EdgeAutoScrollState>,
    /// Indeterminate progress indicators advanced by [`tick_loading_spinners`].
    spinners: SecondaryMap<WidgetId, SpinnerState>,
    /// Monotonic instant of the last spinner tick; `None` before the first.
    spinner_clock: Option<Instant>,
    /// Pointer-proximity visibility for unobtrusive drag handles.
    proximity_reveals: SecondaryMap<WidgetId, ProximityRevealState>,
    /// Pre-rasterized hover labels for compact buttons and meaningful images.
    hover_tooltips: SecondaryMap<WidgetId, HoverTooltipState>,
    /// The widget currently wearing the generic keyboard **focus ring** (SOUL §6.3)
    /// — `None` when nothing currently matches the browser-style
    /// `:focus-visible` modality.
    ring: Option<AppliedInteraction>,
    /// Enabled control currently under the pointer. Its lightweight accent wash
    /// and border mirror the native HTML renderer's `:hover` feedback.
    hover: Option<AppliedInteraction>,
    /// Control held by a pointer press. Uses the theme's active interaction
    /// channels and is cleared on release without rebuilding widget paint.
    active: Option<AppliedInteraction>,
    /// Deferred-paint config for wrapping/aligned/ellipsis text leaves (SOUL §8.1).
    text_layouts: SecondaryMap<WidgetId, TextLayout>,
    /// Retained editing state (value + caret/anchor) per text input (SOUL §6.3) —
    /// owned by [`text_edit`], populated at build, mutated by the edit dispatches.
    edits: SecondaryMap<WidgetId, text_edit::EditState>,
    /// Mounted searchable option rows keyed by their editable combo-box field.
    /// Filtering these retained rows avoids rebuilding the host view on each key.
    comboboxes: SecondaryMap<WidgetId, selection::ComboBoxState>,
    /// Retained document + deferred-paint state per rich text view (SOUL §8.1) —
    /// owned by [`rich`], populated at build, re-flowed by measure/emit.
    rich: SecondaryMap<WidgetId, rich::RichState>,
    /// Content-sized decorated containers repaint from their post-layout boxes.
    panels: SecondaryMap<WidgetId, panel::PanelState>,
    /// Retained virtual-list shells. The list module owns their keyed row
    /// reconciliation and post-layout height feedback; the runtime only owns
    /// their lifetime alongside every other mounted widget behaviour.
    virtual_lists: Vec<Box<dyn virtual_list::MountedVirtualList>>,
    /// Retained multi-line editing state per text area (SOUL §6.3) — owned by
    /// [`text_area`], populated at build, mutated by the edit dispatches.
    areas: SecondaryMap<WidgetId, text_area::AreaState>,
    /// Modal/modeless overlay behavior and paint for dialog layers.
    dialog_layers: SecondaryMap<WidgetId, dialog::DialogLayerState>,
    /// Surface paint configuration for semantic dialog panels.
    dialog_panels: SecondaryMap<WidgetId, dialog::DialogPanelState>,
    /// Active title-bar move or resize-handle pointer capture.
    dialog_pointer: Option<dialog::DialogPointerCapture>,
    /// A possible/active content drag. The source survives pointer movement even
    /// when the cursor leaves its laid-out rect; `hovered` is decorated as the
    /// current drop preview.
    drag_pointer: Option<DragPointerCapture>,
    /// async SVG rasterizations submitted by this runtime and not yet received
    /// back (SOUL §8.1 image pipeline) — the `drain`/`settle` progress gate.
    svg_pending: usize,
    /// the mount generation stamped on raster jobs; [`reset`] bumps it, so a
    /// completion that outlives its tree is dropped rather than written into a
    /// reused [`WidgetId`]'s atlas rect.
    svg_generation: u64,
    /// this runtime's completion mailbox (sender cloned into each job; created
    /// lazily with the first submission).
    svg_reply: Option<(
        std::sync::mpsc::Sender<svg::SvgDone>,
        std::sync::mpsc::Receiver<svg::SvgDone>,
    )>,
}

/// App-owned retained widget behavior.
///
/// The scene remains render-ready and `Send`; this UI-thread handle owns the
/// non-`Send` callbacks and editing state associated with one mounted app. It is
/// passed explicitly through build and dispatch interfaces.
#[derive(Clone, Default)]
pub struct Runtime {
    inner: Rc<RefCell<WidgetRuntime>>,
    context_menu: context_menu::Runtime,
    terminal_grid: terminal_grid::Runtime,
    themes: theme::Runtime,
}

impl Runtime {
    pub fn new() -> Self {
        Self::default()
    }

    fn with<R>(&self, access: impl FnOnce(&RefCell<WidgetRuntime>) -> R) -> R {
        access(&self.inner)
    }

    /// Evaluates a just-mounted dynamic producer under a targeted signal
    /// subscription.  Its private counterpart drains only ready widget ids at
    /// frame time; public widget constructors stay unchanged.
    pub(crate) fn track_dynamic_initial<R>(&self, id: WidgetId, producer: impl FnOnce() -> R) -> R {
        let subscription = self.with(|rt| rt.borrow_mut().reactivity.subscribe(id));
        subscription.track(producer)
    }

    pub(crate) fn track_dynamic<R>(&self, id: WidgetId, producer: impl FnOnce() -> R) -> R {
        let subscription = self
            .with(|rt| rt.borrow().reactivity.subscription(id))
            .expect("mounted dynamic producer must retain its subscription");
        subscription.track(producer)
    }

    pub(crate) fn take_ready_dynamic_ids(&self) -> Vec<u64> {
        self.with(|rt| rt.borrow_mut().reactivity.take_ready())
    }

    pub(crate) fn return_ready_dynamic_ids(&self, ids: Vec<u64>) {
        self.with(|rt| rt.borrow_mut().reactivity.return_ready(ids));
    }
}

/// Clears one app's widget-interaction runtime (SOUL §3.3). Call before a fresh
/// mount (and in tests) so stale handlers/slots from a prior tree cannot alias
/// reused [`WidgetId`]s.
pub fn reset(runtime: &Runtime) {
    clear_scoped_themes(runtime);
    context_menu::reset(runtime);
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.handlers.clear();
        rt.tab_appearances.clear();
        rt.tab_close_buttons.clear();
        rt.tab_reorders.clear();
        rt.tab_reorder_items.clear();
        rt.reactivity.clear();
        rt.slots.clear();
        rt.dynamic_images.clear();
        rt.dynamic_image_scratch.clear();
        rt.sliders.clear();
        rt.scrolls.clear();
        rt.scroll_ids.clear();
        rt.scrollbar_pointer = None;
        rt.edge_auto_scroll = None;
        rt.spinners.clear();
        rt.spinner_clock = None;
        rt.proximity_reveals.clear();
        rt.hover_tooltips.clear();
        rt.ring = None;
        rt.hover = None;
        rt.active = None;
        rt.text_layouts.clear();
        terminal_grid::reset(runtime);
        rt.edits.clear();
        rt.comboboxes.clear();
        rt.rich.clear();
        rt.panels.clear();
        rt.virtual_lists.clear();
        rt.areas.clear();
        rt.dialog_layers.clear();
        rt.dialog_layer_ids.clear();
        rt.dialog_panels.clear();
        rt.dialog_pointer = None;
        rt.drag_pointer = None;
        // In-flight rasterizations belong to the torn-down tree: bump the generation
        // so their completions are drained-and-dropped, never written into a reused
        // WidgetId's rect. `svg_pending` stays — the replies still arrive.
        rt.svg_generation += 1;
    });
}

/// Removes every widget-runtime record owned by a detached scene subtree.
///
/// The scene and widget runtime deliberately use parallel columns. Structural
/// replacement must therefore retire both sides before a SlotMap slot can be
/// reused. State belonging to nodes outside `nodes` is left untouched.
pub fn purge_nodes(runtime: &Runtime, scene: &mut Scene, nodes: &[WidgetId]) {
    let removed = |id: WidgetId| nodes.contains(&id);

    let ring = runtime.with(|rt| rt.borrow().ring.as_ref().map(|state| state.target));
    if ring.is_some_and(&removed) {
        let _ = strip_focus_decoration(runtime, scene, ring.expect("checked above"));
    }
    let hover = runtime.with(|rt| rt.borrow().hover.as_ref().map(|state| state.target));
    if hover.is_some_and(&removed) {
        let _ = strip_hover_decoration(runtime, scene);
    }
    let active = runtime.with(|rt| rt.borrow().active.as_ref().map(|state| state.target));
    if active.is_some_and(&removed) {
        let _ = strip_active_decoration(runtime, scene);
    }

    context_menu::purge_nodes(runtime, nodes);
    terminal_grid::purge_nodes(runtime, nodes);
    for &id in nodes {
        forget_node_theme(runtime, id);
    }

    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        for &id in nodes {
            rt.handlers.remove(id);
            rt.tab_appearances.remove(id);
            rt.tab_close_buttons.remove(id);
            rt.tab_reorders.remove(id);
            rt.tab_reorder_items.remove(id);
            rt.reactivity.forget(id);
            rt.slots.remove(id);
            rt.dynamic_images.remove(id);
            rt.sliders.remove(id);
            rt.scrolls.remove(id);
            rt.scroll_ids.retain(|scroll| *scroll != id);
            rt.spinners.remove(id);
            rt.proximity_reveals.remove(id);
            rt.hover_tooltips.remove(id);
            rt.text_layouts.remove(id);
            rt.edits.remove(id);
            rt.comboboxes.remove(id);
            rt.rich.remove(id);
            rt.panels.remove(id);
            rt.areas.remove(id);
            rt.dialog_layers.remove(id);
            rt.dialog_layer_ids.retain(|layer| *layer != id);
            rt.dialog_panels.remove(id);
        }
        rt.dynamic_image_scratch.retain(|id| !removed(*id));
        if rt
            .scrollbar_pointer
            .is_some_and(|capture| removed(capture.id))
        {
            rt.scrollbar_pointer = None;
        }
        if rt
            .edge_auto_scroll
            .is_some_and(|capture| removed(capture.id))
        {
            rt.edge_auto_scroll = None;
        }
        if rt
            .dialog_pointer
            .is_some_and(|capture| removed(capture.panel))
        {
            rt.dialog_pointer = None;
        }
        if rt.drag_pointer.is_some_and(|capture| {
            removed(capture.source)
                || capture
                    .hovered
                    .is_some_and(|hover| removed(hover.target) || removed(hover.preview))
        }) {
            rt.drag_pointer = None;
        }
    });
}

/// Restores user-owned retained state for one semantic counterpart across a
/// structural remount. This includes editable caret/selection state and a
/// matching text input's in-flight floating-label position. It also carries an
/// animated loading spinner's current phase when its replacement is animated;
/// replacement-authored state remains authoritative for static spinners.
///
/// Call this before focusing or rendering the replacement. Caret/selection
/// state is copied only when both widgets have the same kind and controlled
/// value, so application-driven value changes keep their fresh selection.
pub fn inherit_remount_state(
    runtime: &Runtime,
    scene: &mut Scene,
    id: WidgetId,
    previous_runtime: &Runtime,
    previous_id: WidgetId,
) -> bool {
    let inherited_edit =
        text_edit::inherit_edit_selection(runtime, id, previous_runtime, previous_id)
            || text_area::inherit_area_selection(runtime, id, previous_runtime, previous_id);

    // The builder owns a spinner's size and whether it runs. The runtime owns
    // only its progressed phase, so transfer that phase exclusively when both
    // sides are animated. Re-emit immediately: a remount must never leave the
    // retained runtime and rendered fragment describing different frames.
    let previous_phase = previous_runtime.with(|rt| {
        rt.borrow()
            .spinners
            .get(previous_id)
            .filter(|spinner| spinner.animated)
            .map(|spinner| spinner.phase)
    });
    let inherited_spinner = previous_phase.and_then(|phase| {
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let spinner = rt.spinners.get_mut(id)?;
            if !spinner.animated {
                return None;
            }
            spinner.phase = phase;
            Some((spinner.size, phase))
        })
    });
    if let Some((size, phase)) = inherited_spinner {
        basic::emit_spinner_paint(runtime, scene, id, size, phase);
        scene.mark_dirty(id, DirtyFlags::PAINT);
    }

    inherited_edit || inherited_spinner.is_some()
}

/// Whether `id` currently wears the generic keyboard-visible focus decoration.
///
/// Semantic focus and focus visibility are deliberately separate: a pointer can
/// focus a control without producing a keyboard ring. Structural remounts query
/// this bit so they do not accidentally promote pointer focus to keyboard focus.
pub fn focus_ring_visible(runtime: &Runtime, id: WidgetId) -> bool {
    runtime.with(|rt| {
        rt.borrow()
            .ring
            .as_ref()
            .is_some_and(|decoration| decoration.target == id)
    })
}

/// Returns whether the mounted tree contains at least one visible, automatically
/// animated loading spinner. Hidden responsive/tab subtrees must not keep the
/// native event loop and full renderer awake.
pub fn has_loading_spinners(runtime: &Runtime, scene: &Scene) -> bool {
    runtime.with(|rt| {
        rt.borrow()
            .spinners
            .iter()
            .any(|(id, spinner)| spinner.animated && scene.is_effectively_visible(id))
    })
}

/// Advances every automatically animated [`LoadingSpinner`] by one deterministic
/// frame and repaints only those nodes. The spinner fragment has stable capacity,
/// so this steady animation path allocates nothing after mount.
pub fn tick_loading_spinners(runtime: &Runtime, scene: &mut Scene) -> bool {
    tick_loading_spinners_at(runtime, scene, Some(Instant::now()))
}

/// [`tick_loading_spinners`] with an explicit clock: the shared
/// [`SPINNER_MOTION`] declaration is sampled at the elapsed time between ticks,
/// so the rotation advances proportionally to real time (frame-rate
/// independent). Headless callers without a clock pass `None` and advance one
/// nominal frame.
pub fn tick_loading_spinners_at(
    runtime: &Runtime,
    scene: &mut Scene,
    now: Option<Instant>,
) -> bool {
    let frames: SmallVec<[(WidgetId, f32, f32); 8]> = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let step_ms = match now {
            Some(now) => rt
                .spinner_clock
                .and_then(|last| now.checked_duration_since(last))
                .map(|delta| delta.as_secs_f32() * 1000.0)
                .unwrap_or(SPINNER_FRAME_MS),
            None => SPINNER_FRAME_MS,
        };
        if now.is_some() {
            rt.spinner_clock = now;
        }
        let mut frames = SmallVec::new();
        for (id, spinner) in &mut rt.spinners {
            if !spinner.animated || !scene.is_effectively_visible(id) {
                continue;
            }
            let elapsed = spinner.phase * SPINNER_MOTION.duration_ms + step_ms;
            spinner.phase = SPINNER_MOTION.progress(elapsed);
            frames.push((id, spinner.size, spinner.phase));
        }
        frames
    });
    for &(id, size, phase) in &frames {
        basic::emit_spinner_paint(runtime, scene, id, size, phase);
        scene.mark_dirty(id, DirtyFlags::PAINT);
    }
    !frames.is_empty()
}
