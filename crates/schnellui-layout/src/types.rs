// # schnellui-layout
//
// **Geometry only** (SOUL §8.1): a thin wrapper over **Taffy** (Flexbox / Grid /
// Flow). Layout answers *where* and *how big*, never *what* — it emits rects +
// transforms, draws no pixels, carries no role, and takes no content input.
//
// Containers: `row` `column` `stack` `grid` `scroll` `pad` `spacer`. Content
// leaves hand up an intrinsic size through a [`MeasureFn`]; layout hands a
// [`LayoutBox`](schnellui_scene::LayoutBox) back down the ECS layout column
// (SOUL §8.1). The layout column lives in `schnellui-scene`, so this crate depends
// on scene and writes into it — never the reverse.
//
// ## Contract implemented here
//
// * [`LayoutEngine::sync_tree`] mirrors a scene subtree into Taffy nodes (mount /
//   structure change — SOUL §4 mount may allocate). Content leaves are created
//   with their [`WidgetId`] as the Taffy *node context*, so the measure pass can
//   route back to the leaf's registered [`MeasureFn`].
// * [`LayoutEngine::compute`] runs Taffy over the **smallest dirty subtree only**
//   (SOUL §8.1) and writes an absolute-positioned [`LayoutBox`] into the scene
//   layout column for every node in that subtree — clean siblings are never
//   rewritten.
// * A change to a leaf's [`MeasureFn`] ([`LayoutEngine::set_measure`]) invalidates
//   just that node's Taffy cache (and its ancestors'), so the next `compute`
//   re-measures the minimal region — the *layout-dirty* channel of §8.1.

use schnellui_scene::{ComponentRef, Point, Scene, Size, WidgetId};

/// The CSS-compatible initial font size used to resolve [`Length::Em`] in
/// responsive queries. Media-query `em` units are relative to the initial font
/// size rather than a component's inherited text style.
pub const INITIAL_EM_PX: f32 = 16.0;

/// A responsive-query length in logical pixels or CSS-style `em` units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Px(f32),
    Em(f32),
}

impl Length {
    pub const fn px(value: f32) -> Self {
        Self::Px(value)
    }

    pub const fn em(value: f32) -> Self {
        Self::Em(value)
    }

    /// Resolves this length to logical pixels.
    pub fn to_px(self) -> f32 {
        match self {
            Self::Px(value) => value,
            Self::Em(value) => value * INITIAL_EM_PX,
        }
    }
}

impl From<f32> for Length {
    fn from(value: f32) -> Self {
        Self::Px(value)
    }
}

/// Convenience constructor for a responsive length in logical pixels.
pub const fn px(value: f32) -> Length {
    Length::Px(value)
}

/// Convenience constructor for a CSS-style responsive length in `em`.
pub const fn em(value: f32) -> Length {
    Length::Em(value)
}

/// Which box a [`ResponsiveQuery`] tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponsiveTarget {
    /// The whole logical application viewport (the live window in windowed mode).
    Viewport,
    /// The immediate parent's laid-out content box, analogous to a CSS container
    /// query. Give the parent a definite/fill-derived size to avoid intrinsic-size
    /// query cycles, just as CSS container queries use size containment.
    Parent,
    /// A specifically referenced ancestor component's content box. This is the
    /// retained equivalent of a named CSS container query.
    Component(ComponentRef),
}

/// A conjunction of responsive size bounds. Every configured bound must match for
/// the wrapped component to participate in layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResponsiveQuery {
    pub target: ResponsiveTarget,
    pub min_width: Option<Length>,
    pub max_width: Option<Length>,
    pub min_height: Option<Length>,
    pub max_height: Option<Length>,
}

impl ResponsiveQuery {
    /// Starts a query against the whole logical viewport.
    pub const fn viewport() -> Self {
        Self::new(ResponsiveTarget::Viewport)
    }

    /// Starts a query against the immediate parent's content box.
    pub const fn parent() -> Self {
        Self::new(ResponsiveTarget::Parent)
    }

    /// Starts a query against a referenced ancestor component's content box.
    pub const fn component(reference: ComponentRef) -> Self {
        Self::new(ResponsiveTarget::Component(reference))
    }

    pub const fn new(target: ResponsiveTarget) -> Self {
        Self {
            target,
            min_width: None,
            max_width: None,
            min_height: None,
            max_height: None,
        }
    }

    pub fn min_width(mut self, value: impl Into<Length>) -> Self {
        self.min_width = Some(value.into());
        self
    }

    pub fn max_width(mut self, value: impl Into<Length>) -> Self {
        self.max_width = Some(value.into());
        self
    }

    pub fn min_height(mut self, value: impl Into<Length>) -> Self {
        self.min_height = Some(value.into());
        self
    }

    pub fn max_height(mut self, value: impl Into<Length>) -> Self {
        self.max_height = Some(value.into());
        self
    }

    /// Whether `size` satisfies all configured inclusive bounds.
    pub fn matches(self, size: Size) -> bool {
        finite_bound(self.min_width).is_none_or(|v| size.width >= v)
            && finite_bound(self.max_width).is_none_or(|v| size.width <= v)
            && finite_bound(self.min_height).is_none_or(|v| size.height >= v)
            && finite_bound(self.max_height).is_none_or(|v| size.height <= v)
    }
}

fn finite_bound(value: Option<Length>) -> Option<f32> {
    value.map(Length::to_px).filter(|v| v.is_finite())
}

pub(crate) fn is_strict_ancestor(scene: &Scene, ancestor: WidgetId, descendant: WidgetId) -> bool {
    let mut current = scene.node(descendant).and_then(|node| node.parent);
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        current = scene.node(id).and_then(|node| node.parent);
    }
    false
}

/// Padding / border insets in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeInsets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl EdgeInsets {
    /// Uniform insets on all four edges.
    pub const fn all(v: f32) -> EdgeInsets {
        EdgeInsets {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
    /// Symmetric horizontal/vertical insets.
    pub const fn symmetric(horizontal: f32, vertical: f32) -> EdgeInsets {
        EdgeInsets {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }
    /// Total horizontal (left+right) inset.
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }
    /// Total vertical (top+bottom) inset.
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// How children of a container are arranged (SOUL §8.1). Maps to a Taffy style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Container {
    /// horizontal flex.
    Row,
    /// vertical flex.
    Column,
    /// overlay children in Z, all filling the box.
    Stack,
    /// CSS-grid flow.
    Grid,
    /// clipped, scrollable viewport with a content offset.
    Scroll,
    /// a single padded child.
    Pad(EdgeInsets),
    /// flexible empty space (grows to fill).
    Spacer,
}

/// Main-axis alignment / distribution for a flex container.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Cross-axis alignment for a flex container.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

/// The style inputs for one container node (SOUL §8.1).
#[derive(Clone, Copy, Debug)]
pub struct ContainerStyle {
    pub container: Container,
    pub justify: Justify,
    pub align: Align,
    /// gap between children on the main axis.
    pub gap: f32,
    /// fixed size, if the container is not sizing to content.
    pub fixed_size: Option<Size>,
    /// flex containers only: overflowing children wrap onto additional lines
    /// instead of shrinking/overflowing — the responsive-flow switch (SOUL §8.1).
    pub wrap: bool,
    /// size to **100% of the parent's content box** — and, on the layout root, to
    /// the viewport itself (the available space [`LayoutEngine::compute`] is given,
    /// which windowed mode re-derives from the window on every resize). This is
    /// what lets a layout *track* the real window instead of baking a pixel size.
    /// A definite `fixed_size` / `width` / `height` still wins on its axis.
    pub fill: bool,
    /// definite width with the height left content-sized. Per-axis override:
    /// takes precedence over [`Self::fixed_size`] on its axis.
    pub width: Option<f32>,
    /// definite height with the width left content-sized. Per-axis override:
    /// takes precedence over [`Self::fixed_size`] on its axis.
    pub height: Option<f32>,
    /// Minimum outer width in logical px. Content-sized and definite widths are
    /// both clamped to this lower bound.
    pub min_width: Option<f32>,
    /// Minimum outer height in logical px. Content-sized and definite heights are
    /// both clamped to this lower bound.
    pub min_height: Option<f32>,
    /// float this container **out of flow** at the given `(left, top)` inset
    /// within its parent (Taffy `Position::Absolute`) — a dropdown's option list
    /// anchored below its trigger. An anchored node never displaces its siblings;
    /// making it *paint* above later content is the scene's overlay flag, not
    /// layout's concern (SOUL §8.1 — the layers never bleed).
    pub anchor: Option<Point>,
}

impl ContainerStyle {
    /// A default-styled container of the given kind.
    pub fn new(container: Container) -> ContainerStyle {
        ContainerStyle {
            container,
            justify: Justify::default(),
            align: Align::default(),
            gap: 0.0,
            fixed_size: None,
            wrap: false,
            fill: false,
            width: None,
            height: None,
            min_width: None,
            min_height: None,
            anchor: None,
        }
    }
}

/// Per-child flex factors (SOUL §8.1): how one node — container or content leaf —
/// claims a *responsive* share of its flex parent's main axis. Every field is
/// optional; only set fields override the node's base style, so a `Spacer`'s
/// built-in `grow` survives an otherwise-empty `FlexChild`.
///
/// Semantics mirror CSS: `grow` distributes leftover space proportionally,
/// `shrink` absorbs overflow proportionally, `basis` replaces the node's
/// intrinsic main size as the starting point, and the min/max bounds clamp the
/// resolved size. Registered via [`LayoutEngine::set_flex`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlexChild {
    pub grow: Option<f32>,
    pub shrink: Option<f32>,
    /// starting main size in logical px before grow/shrink (CSS `flex-basis`).
    pub basis: Option<f32>,
    pub min_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_width: Option<f32>,
    pub max_height: Option<f32>,
}

/// A content leaf's intrinsic-size measurement (SOUL §8.1): given the available
/// space, return the size the content wants. Widgets feed Parley text metrics
/// (or image size) up through this — layout never knows *how* text is shaped,
/// only how big it came out.
pub type MeasureFn = Box<dyn FnMut(Size) -> Size + 'static>;

/// A **width-aware** measurement hook threaded through [`LayoutEngine::compute_with`]
/// (SOUL §8.1). Called first for every content leaf during the measure pass with the
/// leaf's [`WidgetId`] and the available space Taffy is offering; returning `Some`
/// supplies that node's intrinsic size (this is how a *wrapping* text leaf shapes at
/// the offered width and reports the resulting height), and returning `None` falls
/// back to the node's registered fixed [`MeasureFn`].
///
/// This is the protocol extension the wrapping feature needs: a leaf whose height
/// depends on the available width (§8.1) cannot answer from a size cached at build,
/// so the umbrella hands the shaper down through this hook and the widget shapes on
/// demand — caching the last `(width → size)` so a same-width relayout re-shapes
/// nothing (grow-only, zero-alloc steady state). The engine itself stays text-blind
/// (SOUL §8.1 — the layers never bleed): it only knows `(WidgetId, Size) -> Size`.
pub type DynMeasure<'a> = &'a mut dyn FnMut(WidgetId, Size) -> Option<Size>;

// The layout engine: wraps a Taffy tree and the `WidgetId ↔ taffy node` mapping,
// computing geometry into the scene's layout column (SOUL §8.1).
//
// The Taffy tree's *node context* is the [`WidgetId`], so the measure pass can
// look a leaf's [`MeasureFn`] back up in [`Self::measures`].
