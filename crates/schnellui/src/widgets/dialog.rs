//! Dialog surfaces and their overlay layer.
//!
//! A dialog is built as three retained containers:
//! `DialogLayer (viewport/parent overlay) → positioning stage → Dialog (surface)`.
//! The split keeps each concern honest: the layer owns the scrim and modal input
//! capture, layout positions the stage, and the semantic surface owns the dialog
//! role and its content.

use std::borrow::Cow;

use schnellui::a11y::{Role, StateFlags};
use schnellui::layout::{
    Align, Container, ContainerStyle, EdgeInsets, FlexChild, Justify, LayoutEngine,
};
use schnellui::scene::{Color, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind};

use crate::widgets::{
    theme, theme_for, AnyView, BuildCtx, Button, ButtonAppearance, ClickHandler, Divider, Flex,
    Row, Text, View,
};

const MODELESS_OVERLAY_LEVEL: u8 = 10;
const MODAL_OVERLAY_LEVEL: u8 = 20;

/// Whether content behind a dialog remains interactive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DialogModality {
    /// Paints a scrim and captures pointer input outside the dialog.
    #[default]
    Modal,
    /// Floats above the application without blocking content outside the panel.
    Modeless,
}

/// Whether schnellui draws dialog chrome around the application content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DialogDecoration {
    /// Shows the title, separator, and accent edge.
    #[default]
    Decorated,
    /// Keeps the surface and semantics but omits title chrome.
    Undecorated,
}

/// The dialog surface's placement inside its containing viewport or parent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DialogPosition {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
    /// An explicit `(left, top)` offset inside the dialog layer's padded stage.
    At(Point),
}

impl DialogPosition {
    fn alignment(self) -> (Justify, Align) {
        match self {
            DialogPosition::TopLeft => (Justify::Start, Align::Start),
            DialogPosition::Top => (Justify::Start, Align::Center),
            DialogPosition::TopRight => (Justify::Start, Align::End),
            DialogPosition::Left => (Justify::Center, Align::Start),
            DialogPosition::Center | DialogPosition::At(_) => (Justify::Center, Align::Center),
            DialogPosition::Right => (Justify::Center, Align::End),
            DialogPosition::BottomLeft => (Justify::End, Align::Start),
            DialogPosition::Bottom => (Justify::End, Align::Center),
            DialogPosition::BottomRight => (Justify::End, Align::End),
        }
    }
}

/// Retained behavior and paint configuration for one dialog layer.
pub(crate) struct DialogLayerState {
    pub(crate) panel: WidgetId,
    pub(crate) modal: bool,
    pub(crate) backdrop: Option<Color>,
    pub(crate) dismiss_on_backdrop: bool,
    pub(crate) dismiss_on_escape: bool,
    pub(crate) on_dismiss: Option<ClickHandler>,
    /// The control inside this window that last had keyboard focus. Activating
    /// the window with the pointer gives it focus back.
    pub(crate) last_focus: Option<WidgetId>,
}

/// Paint configuration for the semantic dialog panel.
pub(crate) struct DialogPanelState {
    pub(crate) surface: Option<Color>,
    /// The visible title and separator that bound the decorated title bar.
    /// `None` is the deliberately chrome-free variant.
    pub(crate) chrome: Option<(WidgetId, WidgetId)>,
    pub(crate) movable: bool,
    pub(crate) resizable: bool,
    pub(crate) min_width: f32,
    pub(crate) min_height: f32,
    pub(crate) max_width: Option<f32>,
    pub(crate) max_height: Option<f32>,
    /// True once pointer interaction has changed this panel's anchor or size.
    /// Remounts preserve only geometry that has become user-owned this way.
    pub(crate) geometry_adjusted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogPointerMode {
    Move,
    Resize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DialogPointerCapture {
    pub(crate) panel: WidgetId,
    pub(crate) mode: DialogPointerMode,
    pub(crate) start_pointer: Point,
    pub(crate) start_anchor: Point,
    pub(crate) start_size: Size,
}

/// A floating dialog component.
///
/// The default is a centered, viewport-fixed modal dialog with a dimmed
/// backdrop. Use [`Dialog::non_fixed`] to scope it to its parent,
/// [`Dialog::modeless`] to keep the rest of the UI interactive, and
/// [`Dialog::persistent`] when backdrop/Escape dismissal must be disabled.
pub struct Dialog {
    title: Cow<'static, str>,
    children: Vec<AnyView>,
    modality: DialogModality,
    position: DialogPosition,
    fixed: bool,
    decoration: DialogDecoration,
    dismiss_on_backdrop: bool,
    dismiss_on_escape: bool,
    alert: bool,
    backdrop: Option<Color>,
    theme_backdrop: bool,
    surface: Option<Color>,
    padding: EdgeInsets,
    viewport_inset: f32,
    gap: f32,
    width: Option<f32>,
    height: Option<f32>,
    max_width: Option<f32>,
    max_height: Option<f32>,
    movable: bool,
    resizable: bool,
    min_width: f32,
    min_height: f32,
    on_dismiss: Option<ClickHandler>,
}

impl Dialog {
    /// Creates a centered modal dialog with `title` as its accessible name.
    pub fn new(title: impl Into<Cow<'static, str>>) -> Dialog {
        Dialog {
            title: title.into(),
            children: Vec::new(),
            modality: DialogModality::Modal,
            position: DialogPosition::Center,
            fixed: true,
            decoration: DialogDecoration::Decorated,
            dismiss_on_backdrop: true,
            dismiss_on_escape: true,
            alert: false,
            backdrop: None,
            theme_backdrop: true,
            surface: None,
            padding: EdgeInsets::all(24.0),
            viewport_inset: 24.0,
            gap: 12.0,
            width: Some(420.0),
            height: None,
            max_width: Some(640.0),
            max_height: None,
            movable: false,
            resizable: false,
            min_width: 220.0,
            min_height: 120.0,
            on_dismiss: None,
        }
    }

    /// Appends content to the panel.
    pub fn child(mut self, child: impl View) -> Dialog {
        self.children.push(Box::new(child));
        self
    }

    /// Uses modal behavior: outside pointer input is captured.
    pub fn modal(mut self) -> Dialog {
        self.modality = DialogModality::Modal;
        if self.backdrop.is_none() {
            self.theme_backdrop = true;
        }
        self
    }

    /// Uses modeless behavior: outside pointer input passes to underlying content.
    pub fn modeless(mut self) -> Dialog {
        self.modality = DialogModality::Modeless;
        self.backdrop = None;
        self.theme_backdrop = false;
        self
    }

    /// Sets modality from a value.
    pub fn modality(mut self, modality: DialogModality) -> Dialog {
        self.modality = modality;
        if modality == DialogModality::Modeless {
            self.backdrop = None;
            self.theme_backdrop = false;
        } else if self.backdrop.is_none() {
            self.theme_backdrop = true;
        }
        self
    }

    /// Portals the dialog layer to the retained-tree root (the default), making
    /// its position fixed to the viewport even when declared in nested content.
    pub fn fixed(mut self) -> Dialog {
        self.fixed = true;
        self
    }

    /// Keeps the overlay under its declared parent, useful for dialogs scoped to
    /// a definite-size panel or embedded workspace.
    pub fn non_fixed(mut self) -> Dialog {
        self.fixed = false;
        self
    }

    /// Uses schnellui's title chrome (the default).
    pub fn decorated(mut self) -> Dialog {
        self.decoration = DialogDecoration::Decorated;
        self
    }

    /// Omits visible title chrome while retaining the dialog surface and
    /// accessible name.
    pub fn undecorated(mut self) -> Dialog {
        self.decoration = DialogDecoration::Undecorated;
        self
    }

    /// Alias for [`Dialog::undecorated`].
    pub fn non_decorated(self) -> Dialog {
        self.undecorated()
    }

    pub fn decoration(mut self, decoration: DialogDecoration) -> Dialog {
        self.decoration = decoration;
        self
    }

    /// Sets one of the standard edge/corner placements.
    pub fn position(mut self, position: DialogPosition) -> Dialog {
        self.position = position;
        self
    }

    /// Positions the surface at an explicit parent-relative offset.
    pub fn at(mut self, left: f32, top: f32) -> Dialog {
        self.position = DialogPosition::At(Point { x: left, y: top });
        self
    }

    /// Marks the component as an urgent alert dialog.
    pub fn alert(mut self) -> Dialog {
        self.alert = true;
        self
    }

    /// Prevents both backdrop and Escape dismissal. The dialog's own buttons can
    /// still run application callbacks.
    pub fn persistent(mut self) -> Dialog {
        self.dismiss_on_backdrop = false;
        self.dismiss_on_escape = false;
        self
    }

    pub fn dismiss_on_backdrop(mut self, enabled: bool) -> Dialog {
        self.dismiss_on_backdrop = enabled;
        self
    }

    pub fn dismiss_on_escape(mut self, enabled: bool) -> Dialog {
        self.dismiss_on_escape = enabled;
        self
    }

    /// Registers the request-to-close callback shared by backdrop and Escape.
    /// Structural removal remains the host's responsibility (normally a signal
    /// change followed by the same remount pattern used by `Dropdown`).
    pub fn on_dismiss(mut self, handler: impl FnMut() + 'static) -> Dialog {
        self.on_dismiss = Some(Box::new(handler));
        self
    }

    /// Overrides the modal scrim color.
    pub fn backdrop(mut self, color: Color) -> Dialog {
        self.backdrop = Some(color);
        self.theme_backdrop = false;
        self
    }

    pub fn without_backdrop(mut self) -> Dialog {
        self.backdrop = None;
        self.theme_backdrop = false;
        self
    }

    /// Overrides the panel surface color; the theme surface is used by default.
    pub fn surface(mut self, color: Color) -> Dialog {
        self.surface = Some(color);
        self
    }

    pub fn padding(mut self, padding: f32) -> Dialog {
        self.padding = EdgeInsets::all(padding.max(0.0));
        self
    }

    pub fn viewport_inset(mut self, inset: f32) -> Dialog {
        self.viewport_inset = inset.max(0.0);
        self
    }

    pub fn gap(mut self, gap: f32) -> Dialog {
        self.gap = gap.max(0.0);
        self
    }

    pub fn width(mut self, width: f32) -> Dialog {
        self.width = Some(width.max(0.0));
        self
    }

    pub fn height(mut self, height: f32) -> Dialog {
        self.height = Some(height.max(0.0));
        self
    }

    pub fn size(mut self, width: f32, height: f32) -> Dialog {
        self.width = Some(width.max(0.0));
        self.height = Some(height.max(0.0));
        self
    }

    /// Lets both axes derive from content rather than the default 420px width.
    pub fn auto_size(mut self) -> Dialog {
        self.width = None;
        self.height = None;
        self
    }

    pub fn max_width(mut self, width: f32) -> Dialog {
        self.max_width = Some(width.max(0.0));
        self
    }

    pub fn max_height(mut self, height: f32) -> Dialog {
        self.max_height = Some(height.max(0.0));
        self
    }

    /// Lets a decorated dialog move when its title bar is dragged.
    pub fn movable(mut self) -> Dialog {
        self.movable = true;
        self
    }

    /// Alias for [`Dialog::movable`].
    pub fn draggable(self) -> Dialog {
        self.movable()
    }

    /// Adds a bottom-right resize handle. Resizing is opt-in so ordinary modal
    /// prompts keep stable application-chosen dimensions.
    pub fn resizable(mut self) -> Dialog {
        self.resizable = true;
        self
    }

    /// Common alternate spelling for [`Dialog::resizable`].
    pub fn resizeable(self) -> Dialog {
        self.resizable()
    }

    /// Sets the interactive resize floor.
    pub fn min_size(mut self, width: f32, height: f32) -> Dialog {
        self.min_width = width.max(0.0);
        self.min_height = height.max(0.0);
        self
    }

    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Dialog
    }

    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}

impl Default for Dialog {
    fn default() -> Self {
        Dialog::new("")
    }
}

impl View for Dialog {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;

        // A fixed dialog is portalled to the top retained ancestor. This makes a
        // declaration deep inside a column behave like CSS `position: fixed`
        // without introducing a second scene tree.
        let attach_parent = if this.fixed {
            parent.map(|mut id| {
                while let Some(p) = ctx.scene.node(id).and_then(|n| n.parent) {
                    id = p;
                }
                id
            })
        } else {
            parent
        };

        let layer = ctx.scene.insert(WidgetKind::DialogLayer, attach_parent);
        ctx.scene.a11y_mut(layer).role = Role::Group.as_u16();
        ctx.scene.set_overlay_level(
            layer,
            if this.modality == DialogModality::Modal {
                MODAL_OVERLAY_LEVEL
            } else {
                MODELESS_OVERLAY_LEVEL
            },
        );
        let mut layer_style =
            ContainerStyle::new(Container::Pad(EdgeInsets::all(this.viewport_inset)));
        layer_style.fill = true;
        // As a child, an overlay must not consume a flex slot in its host.
        if attach_parent.is_some() {
            layer_style.anchor = Some(Point { x: 0.0, y: 0.0 });
        }
        ctx.layout.set_container(layer, layer_style);

        let stage = ctx.scene.insert(WidgetKind::Column, Some(layer));
        ctx.scene.a11y_mut(stage).role = Role::Group.as_u16();
        let (justify, align) = this.position.alignment();
        let mut stage_style = ContainerStyle::new(Container::Column);
        stage_style.fill = true;
        stage_style.justify = justify;
        stage_style.align = align;
        ctx.layout.set_container(stage, stage_style);

        let panel = ctx.scene.insert(WidgetKind::Dialog, Some(stage));
        let title = this.title.into_owned();
        {
            let a = ctx.scene.a11y_mut(panel);
            a.role = if this.alert {
                Role::AlertDialog
            } else {
                Role::Dialog
            }
            .as_u16();
            a.name = Some(title.clone());
            if this.modality == DialogModality::Modal {
                let mut state = StateFlags(a.state);
                state.insert(StateFlags::MODAL);
                a.state = state.0;
            }
        }
        let mut panel_style = ContainerStyle::new(Container::Pad(this.padding));
        // A definite size never outgrows the stage, so a fixed width wider
        // than the viewport shrinks to fit instead of clipping on both sides.
        // `max_*` folds into a definite size: a per-child max would replace the
        // stage cap on that axis and let the clamped size through again.
        panel_style.fit_parent = true;
        panel_style.width = match (this.width, this.max_width) {
            (Some(w), Some(max)) => Some(w.min(max)),
            (w, _) => w,
        };
        panel_style.height = match (this.height, this.max_height) {
            (Some(h), Some(max)) => Some(h.min(max)),
            (h, _) => h,
        };
        if let DialogPosition::At(point) = this.position {
            panel_style.anchor = Some(point);
        }
        ctx.layout.set_container(panel, panel_style);
        ctx.layout.set_flex(
            panel,
            FlexChild {
                max_width: this.width.is_none().then_some(this.max_width).flatten(),
                max_height: this.height.is_none().then_some(this.max_height).flatten(),
                ..FlexChild::default()
            },
        );

        let content = ctx.scene.insert(WidgetKind::Column, Some(panel));
        ctx.scene.a11y_mut(content).role = Role::Group.as_u16();
        let mut content_style = ContainerStyle::new(Container::Column);
        content_style.gap = this.gap;
        content_style.align = Align::Stretch;
        ctx.layout.set_container(content, content_style);
        let chrome = if this.decoration == DialogDecoration::Decorated && !title.is_empty() {
            let heading = Text::new(title.clone()).size(18.0).bold();
            // A dialog that Escape can close also shows a close button in its
            // title bar, so a pointer user need not hunt for the way out.
            let closable = this.on_dismiss.is_some() && this.dismiss_on_escape;
            let title = if closable {
                let runtime = ctx.runtime.clone();
                let close = Button::new(format!("Close {title}"))
                    .display("×")
                    .appearance(ButtonAppearance::Ghost)
                    .text_size(24.0)
                    .width(36.0)
                    .height(36.0)
                    .tooltip(format!("Close {title}"))
                    .on_click(move || {
                        fire_layer_dismiss(&runtime, layer, false);
                    });
                Box::new(
                    Row::new()
                        .align(Align::Center)
                        .gap(8.0)
                        .child(heading)
                        .child(Flex::new().grow(1.0))
                        .child(close),
                )
                .build(ctx, Some(content))
            } else {
                Box::new(heading).build(ctx, Some(content))
            };
            let divider = Box::new(Divider::new()).build(ctx, Some(content));
            Some((title, divider))
        } else {
            None
        };
        for child in this.children {
            child.build(ctx, Some(content));
        }

        let backdrop = if this.theme_backdrop {
            let text = theme(&ctx.runtime).text;
            Some(Color::rgba(text.r, text.g, text.b, 150))
        } else {
            this.backdrop
        };
        ctx.runtime.with(|runtime| {
            let mut runtime = runtime.borrow_mut();
            runtime.dialog_layers.insert(
                layer,
                DialogLayerState {
                    panel,
                    modal: this.modality == DialogModality::Modal,
                    backdrop,
                    dismiss_on_backdrop: this.dismiss_on_backdrop,
                    dismiss_on_escape: this.dismiss_on_escape,
                    on_dismiss: this.on_dismiss,
                    last_focus: None,
                },
            );
            runtime.dialog_layer_ids.push(layer);
            runtime.dialog_panels.insert(
                panel,
                DialogPanelState {
                    surface: this.surface,
                    chrome,
                    movable: this.movable,
                    resizable: this.resizable,
                    min_width: this.min_width,
                    min_height: this.min_height,
                    max_width: this.max_width,
                    max_height: this.max_height,
                    geometry_adjusted: false,
                },
            );
        });

        // Reserve stable primitive capacity before the first layout pass.
        emit_layer_paint(&ctx.runtime, ctx.scene, layer);
        emit_panel_paint(&ctx.runtime, ctx.scene, panel);
        layer
    }
}

fn emit_layer_paint(runtime: &crate::widgets::Runtime, scene: &mut Scene, id: WidgetId) {
    let rect = scene.layout(id).map(|b| b.rect).unwrap_or(Rect::ZERO);
    let backdrop = runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_layers
            .get(id)
            .and_then(|state| state.backdrop)
    });
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    if let Some(color) = backdrop {
        pd.primitives.push(Primitive::SolidRect {
            rect,
            color,
            corner_radius: 0.0,
        });
    }
}

fn emit_panel_paint(runtime: &crate::widgets::Runtime, scene: &mut Scene, id: WidgetId) {
    let rect = scene.layout(id).map(|b| b.rect).unwrap_or(Rect::ZERO);
    let (surface, chrome, resizable) = runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_panels
            .get(id)
            .map(|state| (state.surface, state.chrome, state.resizable))
            .unwrap_or((None, None, false))
    });
    let t = theme_for(runtime, id);
    let shape = t.shape;
    // A very round design still keeps a dialog a rounded rectangle: past this
    // the corners would eat the title and the body copy.
    let radius = shape.radius(12.0, rect.height).min(DIALOG_MAX_RADIUS);
    // Window chrome includes a real outline even under otherwise frameless
    // themes. An undecorated dialog remains a plain raised surface.
    // A soft shadow alone barely lifts a surface off the page, so an
    // undecorated dialog keeps a hairline edge in the separator colour.
    let frame = if chrome.is_some() || shape.frame == 0.0 {
        shape.frame.max(1.0)
    } else {
        shape.frame
    };
    let edge = if chrome.is_none() && shape.frame == 0.0 {
        t.separator
    } else {
        t.outline
    };
    let inner = Rect::new(
        rect.x + frame,
        rect.y + frame,
        (rect.width - frame * 2.0).max(0.0),
        (rect.height - frame * 2.0).max(0.0),
    );
    let chrome_bar = chrome.map(|(_title, divider)| {
        let (divider_top, divider_height) = scene
            .layout(divider)
            .map_or((inner.y, 0.0), |layout| (layout.rect.y, layout.rect.height));
        (
            (divider_top - inner.y).clamp(0.0, inner.height),
            divider_height,
        )
    });
    // The window's content never paints or takes hits outside its frame, so a
    // window shrunk below its content clips it like a scroll viewport does. The
    // clip is a composite property (layout space), so layout is untouched and
    // growing the window back reveals the content again. Descendant paint,
    // focus rings and hit tests all inherit it.
    let content = scene
        .node(id)
        .and_then(|node| node.children.first().copied());
    if let Some(content) = content {
        let mut properties = scene.composite_properties(content);
        properties.clip = Some(inner);
        scene.set_composite_properties(content, properties);
    }
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    // A dialog floats over the page: a hard block in a design with one, a
    // soft fading shadow otherwise.
    crate::widgets::panel::push_shadow(&mut pd.primitives, rect, radius, t);
    if frame > 0.0 {
        pd.primitives.push(Primitive::SolidRect {
            rect,
            color: edge,
            corner_radius: radius,
        });
    }
    pd.primitives.push(Primitive::SolidRect {
        rect: inner,
        color: surface.unwrap_or(t.surface),
        corner_radius: (radius - frame).max(0.0),
    });

    // A decorated dialog reads as a framed window: a contrasting, full-width
    // title bar bounded by the real child divider, plus a strong accent rail.
    // The undecorated variant emits none of this chrome.
    if let Some((bar_height, rule_height)) = chrome_bar {
        let inner_radius = (radius - frame).max(0.0);
        pd.primitives.push(Primitive::SolidRect {
            rect: Rect::new(inner.x, inner.y, inner.width, bar_height),
            color: t.surface_muted,
            corner_radius: inner_radius,
        });
        if bar_height > 0.0 {
            // Square the title bar's lower edge where it meets the dialog body.
            let square_height = bar_height.min(inner_radius);
            pd.primitives.push(Primitive::SolidRect {
                rect: Rect::new(
                    inner.x,
                    inner.y + bar_height - square_height,
                    inner.width,
                    square_height,
                ),
                color: t.surface_muted,
                corner_radius: 0.0,
            });
        }
        // The bar's rule runs edge to edge; the child divider only spans
        // the padded content column.
        if rule_height > 0.0 {
            pd.primitives.push(Primitive::SolidRect {
                rect: Rect::new(inner.x, inner.y + bar_height, inner.width, rule_height),
                color: t.separator,
                corner_radius: 0.0,
            });
        }
    }
    if resizable {
        // Three diagonal grip marks make the opt-in resize target discoverable
        // without adding another semantic child to the dialog.
        for inset in [7.0, 11.0, 15.0] {
            pd.primitives.push(Primitive::Line {
                from: Point {
                    x: rect.right() - inset,
                    y: rect.bottom() - 4.0,
                },
                to: Point {
                    x: rect.right() - 4.0,
                    y: rect.bottom() - inset,
                },
                width: 1.0,
                color: t.text_muted,
            });
        }
    }
}

/// The largest corner radius a dialog takes.
const DIALOG_MAX_RADIUS: f32 = 16.0;

/// Re-sizes dialog paint after layout; generic repositioning only translates
/// primitives and therefore cannot update viewport scrims or auto-sized panels.
pub(crate) fn reposition(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    id: WidgetId,
) -> bool {
    match scene.node(id).map(|node| node.kind) {
        Some(WidgetKind::DialogLayer) => {
            emit_layer_paint(runtime, scene, id);
            true
        }
        Some(WidgetKind::Dialog) => {
            emit_panel_paint(runtime, scene, id);
            true
        }
        _ => false,
    }
}

pub(crate) fn layer_is_modal(runtime: &crate::widgets::Runtime, id: WidgetId) -> bool {
    runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_layers
            .get(id)
            .is_some_and(|state| state.modal)
    })
}

fn topmost_layer(scene: &Scene) -> Option<WidgetId> {
    fn walk(scene: &Scene, id: WidgetId, best: &mut Option<((u8, u64), WidgetId)>) {
        let Some(node) = scene.node(id) else { return };
        if node.kind == WidgetKind::DialogLayer {
            let key = (scene.overlay_level(id), scene.overlay_order(id));
            if best.as_ref().is_none_or(|(current, _)| key > *current) {
                *best = Some((key, id));
            }
            return;
        }
        for &child in &node.children {
            walk(scene, child, best);
        }
    }
    let mut best = None;
    walk(scene, scene.root()?, &mut best);
    best.map(|(_, layer)| layer)
}

/// The top-most modal dialog panel, used to constrain keyboard tab order.
pub fn active_modal_panel(scene: &Scene) -> Option<WidgetId> {
    schnellui::a11y::active_modal_root(scene)
}

/// Runtime-indexed equivalent of [`active_modal_panel`] for hot input paths.
/// The accessibility tree walk remains the source of truth for tree export;
/// pointer/wheel routing only needs the handful of mounted dialog layers.
pub(crate) fn active_modal_panel_in(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
) -> Option<WidgetId> {
    runtime.with(|runtime| {
        let runtime = runtime.borrow();
        runtime
            .dialog_layer_ids
            .iter()
            .filter_map(|&layer| {
                let state = runtime.dialog_layers.get(layer)?;
                (state.modal && scene.is_effectively_visible(state.panel)).then(|| {
                    (
                        scene.overlay_level(layer),
                        scene.overlay_order(layer),
                        state.panel,
                    )
                })
            })
            .max_by_key(|(level, order, _)| (*level, *order))
            .map(|(_, _, panel)| panel)
    })
}

/// The layer owning [`active_modal_panel`]. Unlike [`topmost_layer`], this skips
/// modeless peers: a later inspector never releases or obscures an active modal's
/// input boundary.
pub(crate) fn active_modal_layer(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
) -> Option<WidgetId> {
    let panel = active_modal_panel_in(runtime, scene)?;
    runtime.with(|runtime| {
        let runtime = runtime.borrow();
        runtime.dialog_layer_ids.iter().copied().find(|&layer| {
            runtime
                .dialog_layers
                .get(layer)
                .is_some_and(|state| state.modal && state.panel == panel)
        })
    })
}

/// The panel of every mounted dialog, modal or modeless, in mount order.
pub fn dialog_panels(runtime: &crate::widgets::Runtime) -> Vec<WidgetId> {
    runtime.with(|runtime| {
        let runtime = runtime.borrow();
        runtime
            .dialog_layer_ids
            .iter()
            .filter_map(|&layer| runtime.dialog_layers.get(layer).map(|state| state.panel))
            .collect()
    })
}

/// The panel of the dialog or window `id` sits inside, if any.
pub fn enclosing_dialog_panel(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    id: WidgetId,
) -> Option<WidgetId> {
    runtime.with(|runtime| {
        let runtime = runtime.borrow();
        let mut current = Some(id);
        while let Some(node_id) = current {
            if runtime.dialog_panels.contains_key(node_id) {
                return Some(node_id);
            }
            current = scene.node(node_id).and_then(|node| node.parent);
        }
        None
    })
}

/// Whether `id` is the root of, or belongs to, `ancestor`'s retained subtree.
pub fn is_in_subtree(scene: &Scene, id: WidgetId, ancestor: WidgetId) -> bool {
    let mut current = Some(id);
    while let Some(node_id) = current {
        if node_id == ancestor {
            return true;
        }
        current = scene.node(node_id).and_then(|node| node.parent);
    }
    false
}

const RESIZE_HANDLE: f32 = 18.0;

fn pointer_mode_for_layer(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    layer: WidgetId,
    point: Point,
) -> Option<(WidgetId, DialogPointerMode)> {
    let panel =
        runtime.with(|runtime| runtime.borrow().dialog_layers.get(layer).map(|s| s.panel))?;
    let (chrome, movable, resizable) = runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_panels
            .get(panel)
            .map(|state| (state.chrome, state.movable, state.resizable))
    })?;
    let rect = scene.layout(panel)?.rect;
    if resizable {
        let handle = Rect::new(
            rect.right() - RESIZE_HANDLE,
            rect.bottom() - RESIZE_HANDLE,
            RESIZE_HANDLE,
            RESIZE_HANDLE,
        );
        if handle.contains(point) {
            return Some((panel, DialogPointerMode::Resize));
        }
    }
    if movable {
        let (_, divider) = chrome?;
        let title_bottom = scene
            .layout(divider)
            .map(|layout| layout.rect.bottom())
            .unwrap_or(rect.y);
        let titlebar = Rect::new(rect.x, rect.y, rect.width, (title_bottom - rect.y).max(0.0));
        if titlebar.contains(point) {
            return Some((panel, DialogPointerMode::Move));
        }
    }
    None
}

fn dialog_layer_for_widget(scene: &Scene, id: WidgetId) -> Option<WidgetId> {
    let mut current = Some(id);
    while let Some(node_id) = current {
        let node = scene.node(node_id)?;
        if node.kind == WidgetKind::DialogLayer {
            return Some(node_id);
        }
        current = node.parent;
    }
    None
}

/// Cursor for an active dialog capture. Capture wins outside the original
/// titlebar/handle so feedback remains stable for the whole drag gesture.
pub(crate) fn captured_cursor(
    runtime: &crate::widgets::Runtime,
) -> Option<crate::widgets::CursorIcon> {
    runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_pointer
            .map(|capture| match capture.mode {
                DialogPointerMode::Move => crate::widgets::CursorIcon::Grabbing,
                DialogPointerMode::Resize => crate::widgets::CursorIcon::NwseResize,
            })
    })
}

/// Cursor contributed by the topmost dialog chrome at `point`.
pub(crate) fn cursor_for_hit(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    hit: WidgetId,
    point: Point,
) -> Option<crate::widgets::CursorIcon> {
    let layer = dialog_layer_for_widget(scene, hit)?;
    let (_, mode) = pointer_mode_for_layer(runtime, scene, layer, point)?;
    if mode == DialogPointerMode::Move
        && scene
            .node(hit)
            .is_some_and(|node| node.kind == WidgetKind::Button)
    {
        return None;
    }
    Some(match mode {
        DialogPointerMode::Move => crate::widgets::CursorIcon::Grab,
        DialogPointerMode::Resize => crate::widgets::CursorIcon::NwseResize,
    })
}

/// Whether `id` sits in a decorated dialog's title bar, like its close
/// button. Opening a dialog focuses its content before its chrome.
pub fn is_dialog_chrome_control(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    id: WidgetId,
) -> bool {
    let Some(row) = scene.node(id).and_then(|node| node.parent) else {
        return false;
    };
    let Some(panel) = scene
        .node(row)
        .and_then(|node| node.parent)
        .and_then(|content| scene.node(content))
        .and_then(|content| content.parent)
    else {
        return false;
    };
    runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_panels
            .get(panel)
            .and_then(|state| state.chrome)
            .is_some_and(|(title, _)| title == row)
    })
}

/// Raises the dialog layer containing `id` within its existing overlay plane.
pub fn foreground_dialog_for_widget(scene: &mut Scene, id: WidgetId) -> bool {
    let Some(layer) = dialog_layer_for_widget(scene, id) else {
        return false;
    };
    scene.bring_overlay_to_front(layer)
}

/// Notes `focused` as the control that last had focus in the window holding it.
pub fn remember_dialog_focus(runtime: &crate::widgets::Runtime, scene: &Scene, focused: WidgetId) {
    let Some(layer) = dialog_layer_for_widget(scene, focused) else {
        return;
    };
    runtime.with(|runtime| {
        if let Some(state) = runtime.borrow_mut().dialog_layers.get_mut(layer) {
            state.last_focus = Some(focused);
        }
    });
}

/// The panel of the dialog or window a press on `hit` lands in, its backdrop
/// included. `None` outside every dialog.
pub fn pressed_dialog_panel(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    hit: WidgetId,
) -> Option<WidgetId> {
    let layer = dialog_layer_for_widget(scene, hit)?;
    runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_layers
            .get(layer)
            .map(|state| state.panel)
    })
}

/// The modeless window a press on `hit` activates, with the control that last
/// had focus in it. `None` when `hit` is outside every window, in a modal (which
/// owns focus already) or in the window that already holds focus.
pub fn window_to_activate(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    hit: WidgetId,
) -> Option<(WidgetId, Option<WidgetId>)> {
    let layer = dialog_layer_for_widget(scene, hit)?;
    if layer_is_modal(runtime, layer) {
        return None;
    }
    if crate::a11y::focused(scene).is_some_and(|focused| is_in_subtree(scene, focused, layer)) {
        return None;
    }
    let remembered = runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_layers
            .get(layer)
            .and_then(|state| state.last_focus)
    });
    Some((
        layer,
        remembered.filter(|&id| is_in_subtree(scene, id, layer)),
    ))
}

/// Raises the visually topmost dialog hit at `point`. This is used before a
/// title-bar drag begins, since title chrome itself is not focusable.
pub fn foreground_dialog_at(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    point: Point,
) -> bool {
    let Some(hit) = crate::widgets::hit_test(runtime, scene, point) else {
        return false;
    };
    foreground_dialog_for_widget(scene, hit)
}

/// Starts title-bar movement or bottom-right resizing for the top dialog under
/// `point`. Returns `true` when the pointer is captured by dialog chrome.
pub fn begin_dialog_pointer(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    point: Point,
) -> bool {
    // First resolve the foreground dialog at this exact pixel. Only that
    // dialog's chrome can capture the pointer; a title bar covered by a peer's
    // body is inert.
    let Some(hit) = crate::widgets::hit_test(runtime, scene, point) else {
        return false;
    };
    let Some(layer) = dialog_layer_for_widget(scene, hit) else {
        return false;
    };
    let Some((panel, mode)) = pointer_mode_for_layer(runtime, scene, layer, point) else {
        return false;
    };
    // A button in the title bar, such as close, takes its own press.
    if mode == DialogPointerMode::Move
        && scene
            .node(hit)
            .is_some_and(|node| node.kind == WidgetKind::Button)
    {
        return false;
    }
    let Some(rect) = scene.layout(panel).map(|layout| layout.rect) else {
        return false;
    };
    let Some(stage) = scene.node(panel).and_then(|node| node.parent) else {
        return false;
    };
    let Some(stage_rect) = scene.layout(stage).map(|layout| layout.rect) else {
        return false;
    };
    runtime.with(|runtime| {
        runtime.borrow_mut().dialog_pointer = Some(DialogPointerCapture {
            panel,
            mode,
            start_pointer: point,
            start_anchor: Point {
                x: rect.x - stage_rect.x,
                y: rect.y - stage_rect.y,
            },
            start_size: Size {
                width: rect.width,
                height: rect.height,
            },
        });
    });
    true
}

/// Applies a captured dialog move/resize to its retained layout style. The
/// caller performs the resulting layout pass before rendering.
pub fn update_dialog_pointer(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    layout: &mut LayoutEngine,
    point: Point,
) -> bool {
    let Some(capture) = runtime.with(|runtime| runtime.borrow().dialog_pointer) else {
        return false;
    };
    let Some(stage) = scene.node(capture.panel).and_then(|node| node.parent) else {
        return false;
    };
    let Some(stage_rect) = scene.layout(stage).map(|layout| layout.rect) else {
        return false;
    };
    let Some(mut style) = layout.container_style(capture.panel) else {
        return false;
    };
    let Some((min_width, min_height, max_width, max_height)) = runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_panels
            .get(capture.panel)
            .map(|state| {
                (
                    state.min_width,
                    state.min_height,
                    state.max_width,
                    state.max_height,
                )
            })
    }) else {
        return false;
    };
    let dx = point.x - capture.start_pointer.x;
    let dy = point.y - capture.start_pointer.y;
    match capture.mode {
        DialogPointerMode::Move => {
            let max_x = (stage_rect.width - capture.start_size.width).max(0.0);
            let max_y = (stage_rect.height - capture.start_size.height).max(0.0);
            style.anchor = Some(Point {
                x: (capture.start_anchor.x + dx).clamp(0.0, max_x),
                y: (capture.start_anchor.y + dy).clamp(0.0, max_y),
            });
        }
        DialogPointerMode::Resize => {
            let available_width = (stage_rect.width - capture.start_anchor.x).max(0.0);
            let available_height = (stage_rect.height - capture.start_anchor.y).max(0.0);
            // The size the window already has is always allowed. A window that
            // starts past the stage edge (or whose stage shrank under it) would
            // otherwise snap down to what fits on the first drag, even in the
            // dimension the pointer did not move. Growing then stops at the
            // current size, and shrinking works from it.
            let start = capture.start_size;
            let max_width = max_width
                .unwrap_or(available_width)
                .min(available_width)
                .max(start.width);
            let max_height = max_height
                .unwrap_or(available_height)
                .min(available_height)
                .max(start.height);
            let min_width = min_width.min(max_width).min(start.width);
            let min_height = min_height.min(max_height).min(start.height);
            style.anchor = Some(capture.start_anchor);
            style.width = Some((capture.start_size.width + dx).clamp(min_width, max_width));
            style.height = Some((capture.start_size.height + dy).clamp(min_height, max_height));
        }
    }
    if layout.container_style(capture.panel).is_some_and(|old| {
        old.anchor == style.anchor && old.width == style.width && old.height == style.height
    }) {
        return false;
    }
    layout.set_container(capture.panel, style);
    runtime.with(|runtime| {
        if let Some(panel) = runtime.borrow_mut().dialog_panels.get_mut(capture.panel) {
            panel.geometry_adjusted = true;
        }
    });
    true
}

/// Carries the user-geometry ownership marker from a remounted dialog panel.
///
/// The returned value tells the host whether it should copy the previous
/// panel's live anchor and dimensions. The replacement is marked identically,
/// so a later remount continues to treat that geometry as user-owned.
pub fn transfer_dialog_geometry_adjustment(
    replacement_runtime: &crate::widgets::Runtime,
    replacement: WidgetId,
    previous_runtime: &crate::widgets::Runtime,
    previous: WidgetId,
) -> bool {
    let adjusted = previous_runtime.with(|runtime| {
        runtime
            .borrow()
            .dialog_panels
            .get(previous)
            .is_some_and(|panel| panel.geometry_adjusted)
    });
    replacement_runtime.with(|runtime| {
        if let Some(panel) = runtime.borrow_mut().dialog_panels.get_mut(replacement) {
            panel.geometry_adjusted = adjusted;
        }
    });
    adjusted
}

/// Hands a move or resize in progress on `previous` to its counterpart in a
/// rebuilt tree, so a drag that a theme change or remount lands under keeps
/// holding the dialog until the button is released.
pub(crate) fn transfer_dialog_pointer(
    replacement_runtime: &crate::widgets::Runtime,
    replacement: WidgetId,
    previous_runtime: &crate::widgets::Runtime,
    previous: WidgetId,
) {
    let Some(capture) = previous_runtime
        .with(|runtime| runtime.borrow().dialog_pointer)
        .filter(|capture| capture.panel == previous)
    else {
        return;
    };
    replacement_runtime.with(|runtime| {
        runtime.borrow_mut().dialog_pointer = Some(DialogPointerCapture {
            panel: replacement,
            ..capture
        });
    });
}

/// Releases any active dialog pointer capture.
pub fn end_dialog_pointer(runtime: &crate::widgets::Runtime) -> bool {
    runtime.with(|runtime| runtime.borrow_mut().dialog_pointer.take().is_some())
}

fn fire_dismiss(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    layer: WidgetId,
    backdrop: bool,
) -> bool {
    // The callback may only set a signal; the host remount removes the dialog.
    let _ = scene;
    fire_layer_dismiss(runtime, layer, backdrop)
}

/// Runs a dialog layer's dismiss handler when the backdrop (`backdrop`) or
/// Escape and the title-bar close button may dismiss it.
fn fire_layer_dismiss(runtime: &crate::widgets::Runtime, layer: WidgetId, backdrop: bool) -> bool {
    let callback = runtime.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        let state = runtime.dialog_layers.get_mut(layer)?;
        let enabled = if backdrop {
            state.dismiss_on_backdrop
        } else {
            state.dismiss_on_escape
        };
        enabled.then(|| state.on_dismiss.take()).flatten()
    });
    let Some(mut callback) = callback else {
        return false;
    };
    callback();
    runtime.with(|runtime| {
        if let Some(state) = runtime.borrow_mut().dialog_layers.get_mut(layer) {
            state.on_dismiss = Some(callback);
        }
    });
    true
}

pub(crate) fn dispatch_backdrop(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    layer: WidgetId,
) -> bool {
    fire_dismiss(runtime, scene, layer, true)
}

/// Routes Escape to the top-most dialog. `None` means no dialog consumed the key;
/// `Some(false)` means a persistent dialog consumed it without closing.
pub fn dispatch_dialog_escape(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
) -> Option<bool> {
    let layer = active_modal_layer(runtime, scene).or_else(|| topmost_layer(scene))?;
    Some(fire_dismiss(runtime, scene, layer, false))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::a11y::StateFlags;
    use schnellui::layout::LayoutEngine;
    use schnellui::scene::Size;
    use schnellui::text::{GlyphAtlas, TextShaper};

    use super::*;
    use crate::widgets::{dispatch_click, hit_test, reposition_paint, reset, Button, Column, Text};

    fn build(runtime: &crate::widgets::Runtime, view: impl View, size: Size) -> (Scene, WidgetId) {
        let (scene, _, root) = build_with_layout(runtime, view, size);
        (scene, root)
    }

    fn build_with_layout(
        runtime: &crate::widgets::Runtime,
        view: impl View,
        size: Size,
    ) -> (Scene, LayoutEngine, WidgetId) {
        reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let root = {
            let mut ctx = BuildCtx {
                context: crate::widgets::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(view).build(&mut ctx, None)
        };
        scene.set_root(root);
        layout.sync_tree(&scene, root);
        layout.compute(&mut scene, root, size);
        reposition_paint(runtime, &mut scene);
        (scene, layout, root)
    }

    fn find_kind(scene: &Scene, root: WidgetId, kind: WidgetKind) -> Option<WidgetId> {
        if scene.node(root).is_some_and(|node| node.kind == kind) {
            return Some(root);
        }
        for &child in &scene.node(root)?.children {
            if let Some(found) = find_kind(scene, child, kind) {
                return Some(found);
            }
        }
        None
    }

    fn count_kind(scene: &Scene, root: WidgetId, kind: WidgetKind) -> usize {
        let own = usize::from(scene.node(root).is_some_and(|node| node.kind == kind));
        own + scene
            .node(root)
            .map(|node| {
                node.children
                    .iter()
                    .map(|child| count_kind(scene, *child, kind))
                    .sum::<usize>()
            })
            .unwrap_or(0)
    }

    #[test]
    fn modal_dialog_builds_semantic_centered_surface_and_scrim() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, root) = build(
            runtime,
            Dialog::new("Preferences").child(Text::new("Appearance")),
            Size {
                width: 800.0,
                height: 600.0,
            },
        );
        assert_eq!(scene.node(root).unwrap().kind, WidgetKind::DialogLayer);
        assert!(scene.is_overlay(root));
        let panel = find_kind(&scene, root, WidgetKind::Dialog).unwrap();
        let a11y = scene.a11y(panel).unwrap();
        assert_eq!(Role::from_u16(a11y.role), Role::Dialog);
        assert_eq!(a11y.name.as_deref(), Some("Preferences"));
        assert!(StateFlags(a11y.state).contains(StateFlags::MODAL));
        let rect = scene.layout(panel).unwrap().rect;
        assert!((rect.x + rect.width * 0.5 - 400.0).abs() < 0.01);
        assert!((rect.y + rect.height * 0.5 - 300.0).abs() < 0.01);
        assert!(!scene.paint(root).unwrap().primitives.is_empty());
        assert!(!scene.paint(panel).unwrap().primitives.is_empty());
    }

    fn panel_rect(dialog: Dialog, size: Size) -> Rect {
        let runtime_handle = crate::widgets::Runtime::new();
        let (scene, root) = build(&runtime_handle, dialog, size);
        let panel = find_kind(&scene, root, WidgetKind::Dialog).unwrap();
        scene.layout(panel).unwrap().rect
    }

    #[test]
    fn fixed_width_dialog_wider_than_viewport_shrinks_inside_the_inset() {
        let size = Size {
            width: 444.0,
            height: 428.0,
        };
        for dialog in [
            Dialog::new("Quick command").undecorated().width(500.0),
            Dialog::new("Quick command").width(500.0),
            Dialog::new("Quick command").width(500.0).max_width(400.0),
            Dialog::new("Quick command").size(500.0, 900.0),
        ] {
            let rect = panel_rect(dialog.child(Text::new("Jump to a publishing task")), size);
            assert!(rect.x >= 24.0 - 0.01, "left edge off the inset: {rect:?}");
            assert!(rect.y >= 24.0 - 0.01, "top edge off the inset: {rect:?}");
            assert!(rect.x + rect.width <= 444.0 - 24.0 + 0.01, "{rect:?}");
            assert!(rect.y + rect.height <= 428.0 - 24.0 + 0.01, "{rect:?}");
        }
    }

    #[test]
    fn fixed_width_dialog_that_fits_keeps_its_width() {
        let rect = panel_rect(
            Dialog::new("Quick command")
                .width(300.0)
                .child(Text::new("Hi")),
            Size {
                width: 444.0,
                height: 428.0,
            },
        );
        assert!((rect.width - 300.0).abs() < 0.01, "{rect:?}");
    }

    #[test]
    fn modal_backdrop_captures_and_requests_dismissal() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let dismissed = Rc::new(Cell::new(false));
        let sink = dismissed.clone();
        let (mut scene, root) = build(
            runtime,
            Dialog::new("Confirm")
                .child(Text::new("Continue?"))
                .on_dismiss(move || sink.set(true)),
            Size {
                width: 800.0,
                height: 600.0,
            },
        );
        let hit = hit_test(runtime, &scene, Point { x: 4.0, y: 4.0 });
        assert_eq!(hit, Some(root));
        assert!(dispatch_click(runtime, &mut scene, root));
        assert!(dismissed.get());
    }

    #[test]
    fn modeless_dialog_passes_outside_pointer_to_base_content() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, root) = build(
            runtime,
            Column::new().fill().child(Button::new("behind")).child(
                Dialog::new("Inspector")
                    .modeless()
                    .child(Text::new("Tools")),
            ),
            Size {
                width: 800.0,
                height: 600.0,
            },
        );
        let button = find_kind(&scene, root, WidgetKind::Button).unwrap();
        assert_eq!(
            hit_test(runtime, &scene, Point { x: 2.0, y: 2.0 }),
            Some(button)
        );
    }

    #[test]
    fn fixed_portals_to_root_while_non_fixed_stays_in_parent() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (fixed_scene, fixed_root) = build(
            runtime,
            Column::new()
                .fill()
                .child(Column::new().size(300.0, 300.0).child(Dialog::new("Fixed"))),
            Size {
                width: 800.0,
                height: 600.0,
            },
        );
        let fixed_layer = find_kind(&fixed_scene, fixed_root, WidgetKind::DialogLayer).unwrap();
        assert_eq!(
            fixed_scene.node(fixed_layer).unwrap().parent,
            Some(fixed_root)
        );

        let (scoped_scene, scoped_root) = build(
            runtime,
            Column::new().fill().child(
                Column::new()
                    .size(300.0, 300.0)
                    .child(Dialog::new("Scoped").non_fixed()),
            ),
            Size {
                width: 800.0,
                height: 600.0,
            },
        );
        let scoped_layer = find_kind(&scoped_scene, scoped_root, WidgetKind::DialogLayer).unwrap();
        let scoped_parent = scoped_scene.node(scoped_layer).unwrap().parent.unwrap();
        assert_ne!(scoped_parent, scoped_root);
        assert_eq!(
            scoped_scene.node(scoped_parent).unwrap().kind,
            WidgetKind::Column
        );
    }

    #[test]
    fn persistent_dialog_consumes_escape_without_dismissal() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _) = build(
            runtime,
            Dialog::new("Working").persistent(),
            Size {
                width: 400.0,
                height: 300.0,
            },
        );
        assert_eq!(dispatch_dialog_escape(runtime, &mut scene), Some(false));
    }

    #[test]
    fn decorated_and_undecorated_variants_control_title_chrome() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let size = Size {
            width: 600.0,
            height: 400.0,
        };
        let (decorated, decorated_root) = build(
            runtime,
            Dialog::new("Decorated").child(Button::new("action")),
            size,
        );
        let decorated_panel = find_kind(&decorated, decorated_root, WidgetKind::Dialog).unwrap();
        assert_eq!(count_kind(&decorated, decorated_root, WidgetKind::Text), 1);
        assert_eq!(
            count_kind(&decorated, decorated_root, WidgetKind::Divider),
            1
        );

        let (plain, plain_root) = build(
            runtime,
            Dialog::new("Plain")
                .undecorated()
                .child(Button::new("action")),
            size,
        );
        let plain_panel = find_kind(&plain, plain_root, WidgetKind::Dialog).unwrap();
        assert_eq!(count_kind(&plain, plain_root, WidgetKind::Text), 0);
        assert_eq!(count_kind(&plain, plain_root, WidgetKind::Divider), 0);
        assert!(
            decorated.paint(decorated_panel).unwrap().primitives.len()
                >= plain.paint(plain_panel).unwrap().primitives.len() + 2,
            "decorated panels paint a title band and accent rail"
        );
        // Removing visible chrome never removes the semantic title.
        assert_eq!(
            plain.a11y(plain_panel).unwrap().name.as_deref(),
            Some("Plain")
        );
    }

    #[test]
    fn movable_and_resizable_dialog_chrome_reports_native_cursors() {
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, root) = build(
            runtime,
            Dialog::new("Workspace")
                .modeless()
                .non_fixed()
                .movable()
                .resizable()
                .size(360.0, 240.0)
                .child(Text::new("Content")),
            Size {
                width: 800.0,
                height: 600.0,
            },
        );
        let panel = find_kind(&scene, root, WidgetKind::Dialog).unwrap();
        let divider = find_kind(&scene, root, WidgetKind::Divider).unwrap();
        let panel_rect = scene.layout(panel).unwrap().rect;
        let divider_rect = scene.layout(divider).unwrap().rect;
        let title = Point {
            x: panel_rect.x + 12.0,
            y: (panel_rect.y + divider_rect.y) * 0.5,
        };
        let resize = Point {
            x: panel_rect.right() - 2.0,
            y: panel_rect.bottom() - 2.0,
        };

        assert_eq!(
            crate::widgets::cursor_at(runtime, &scene, title),
            crate::widgets::CursorIcon::Grab
        );
        assert_eq!(
            crate::widgets::cursor_at(runtime, &scene, resize),
            crate::widgets::CursorIcon::NwseResize
        );
        assert!(begin_dialog_pointer(runtime, &scene, title));
        assert_eq!(
            crate::widgets::cursor_at(runtime, &scene, Point { x: 1.0, y: 1.0 }),
            crate::widgets::CursorIcon::Grabbing,
            "captured cursor remains stable away from the titlebar"
        );
        assert!(end_dialog_pointer(runtime,));
        assert_eq!(
            crate::widgets::cursor_at(runtime, &scene, Point { x: 1.0, y: 1.0 }),
            crate::widgets::CursorIcon::Default
        );
    }

    #[test]
    fn untouched_dialog_geometry_is_not_transferred_across_remount() {
        let previous_runtime = crate::widgets::Runtime::new();
        let replacement_runtime = crate::widgets::Runtime::new();
        let size = Size {
            width: 800.0,
            height: 600.0,
        };
        let (previous_scene, previous_root) = build(
            &previous_runtime,
            Dialog::new("Workspace").movable().resizable(),
            size,
        );
        let (replacement_scene, replacement_root) = build(
            &replacement_runtime,
            Dialog::new("Workspace").movable().resizable(),
            size,
        );
        let previous = find_kind(&previous_scene, previous_root, WidgetKind::Dialog).unwrap();
        let replacement =
            find_kind(&replacement_scene, replacement_root, WidgetKind::Dialog).unwrap();

        assert!(!transfer_dialog_geometry_adjustment(
            &replacement_runtime,
            replacement,
            &previous_runtime,
            previous,
        ));
    }

    #[test]
    fn moved_or_resized_dialog_geometry_is_transferred_across_remount() {
        let size = Size {
            width: 800.0,
            height: 600.0,
        };

        for (mode, pointer_delta) in [
            (DialogPointerMode::Move, Point { x: 70.0, y: 45.0 }),
            (DialogPointerMode::Resize, Point { x: 60.0, y: 40.0 }),
        ] {
            let previous_runtime = crate::widgets::Runtime::new();
            let replacement_runtime = crate::widgets::Runtime::new();
            let successor_runtime = crate::widgets::Runtime::new();
            let (previous_scene, mut previous_layout, previous_root) = build_with_layout(
                &previous_runtime,
                Dialog::new("Workspace")
                    .modeless()
                    .movable()
                    .resizable()
                    .size(360.0, 240.0),
                size,
            );
            let (replacement_scene, replacement_root) = build(
                &replacement_runtime,
                Dialog::new("Workspace")
                    .modeless()
                    .movable()
                    .resizable()
                    .size(360.0, 240.0),
                size,
            );
            let (successor_scene, successor_root) = build(
                &successor_runtime,
                Dialog::new("Workspace")
                    .modeless()
                    .movable()
                    .resizable()
                    .size(360.0, 240.0),
                size,
            );
            let previous = find_kind(&previous_scene, previous_root, WidgetKind::Dialog).unwrap();
            let replacement =
                find_kind(&replacement_scene, replacement_root, WidgetKind::Dialog).unwrap();
            let successor =
                find_kind(&successor_scene, successor_root, WidgetKind::Dialog).unwrap();
            let rect = previous_scene.layout(previous).unwrap().rect;
            let pointer = match mode {
                DialogPointerMode::Move => {
                    let divider =
                        find_kind(&previous_scene, previous_root, WidgetKind::Divider).unwrap();
                    let divider_rect = previous_scene.layout(divider).unwrap().rect;
                    Point {
                        x: rect.x + 12.0,
                        y: (rect.y + divider_rect.y) * 0.5,
                    }
                }
                DialogPointerMode::Resize => Point {
                    x: rect.right() - 2.0,
                    y: rect.bottom() - 2.0,
                },
            };

            assert!(begin_dialog_pointer(
                &previous_runtime,
                &previous_scene,
                pointer
            ));
            assert!(update_dialog_pointer(
                &previous_runtime,
                &previous_scene,
                &mut previous_layout,
                Point {
                    x: pointer.x + pointer_delta.x,
                    y: pointer.y + pointer_delta.y,
                },
            ));
            assert!(transfer_dialog_geometry_adjustment(
                &replacement_runtime,
                replacement,
                &previous_runtime,
                previous,
            ));
            assert!(transfer_dialog_geometry_adjustment(
                &successor_runtime,
                successor,
                &replacement_runtime,
                replacement,
            ));
        }
    }
    /// A window that starts partly past the stage edge, such as a default
    /// position that puts its right edge outside a narrow viewport.
    fn overhanging(runtime: &crate::widgets::Runtime) -> (Scene, LayoutEngine, WidgetId, Rect) {
        let (mut scene, mut layout, root) = build_with_layout(
            runtime,
            Dialog::new("Proof")
                .modeless()
                .non_fixed()
                .movable()
                .resizable()
                .at(500.0, 40.0)
                .size(300.0, 200.0),
            Size {
                width: 640.0,
                height: 480.0,
            },
        );
        let panel = find_kind(&scene, root, WidgetKind::Dialog).unwrap();
        let rect = scene.layout(panel).unwrap().rect;
        let stage = scene.node(panel).and_then(|node| node.parent).unwrap();
        let stage_rect = scene.layout(stage).unwrap().rect;
        assert!(
            rect.right() > stage_rect.right(),
            "the window {rect:?} must start past the stage edge {stage_rect:?}"
        );
        layout.sync_tree(&scene, root);
        layout.compute(
            &mut scene,
            root,
            Size {
                width: 640.0,
                height: 480.0,
            },
        );
        (scene, layout, panel, rect)
    }

    fn drag_grip(
        runtime: &crate::widgets::Runtime,
        scene: &mut Scene,
        layout: &mut LayoutEngine,
        panel: WidgetId,
        rect: Rect,
        by: Point,
    ) -> Rect {
        let grip = Point {
            x: rect.right() - 2.0,
            y: rect.bottom() - 2.0,
        };
        assert!(begin_dialog_pointer(runtime, scene, grip));
        update_dialog_pointer(
            runtime,
            scene,
            layout,
            Point {
                x: grip.x + by.x,
                y: grip.y + by.y,
            },
        );
        end_dialog_pointer(runtime);
        let root = scene.root().unwrap();
        layout.sync_tree(scene, root);
        layout.compute(
            scene,
            root,
            Size {
                width: 640.0,
                height: 480.0,
            },
        );
        scene.layout(panel).unwrap().rect
    }

    #[test]
    fn a_vertical_grow_of_a_window_past_the_stage_edge_keeps_its_width() {
        let runtime = crate::widgets::Runtime::new();
        let (mut scene, mut layout, panel, rect) = overhanging(&runtime);
        let after = drag_grip(
            &runtime,
            &mut scene,
            &mut layout,
            panel,
            rect,
            Point { x: 0.0, y: 50.0 },
        );
        assert_eq!(after.width, rect.width);
        assert_eq!(after.height, rect.height + 50.0);
    }

    #[test]
    fn a_horizontal_grow_of_a_window_past_the_stage_edge_keeps_its_height_and_never_shrinks() {
        let runtime = crate::widgets::Runtime::new();
        let (mut scene, mut layout, panel, rect) = overhanging(&runtime);
        let after = drag_grip(
            &runtime,
            &mut scene,
            &mut layout,
            panel,
            rect,
            Point { x: 30.0, y: 0.0 },
        );
        assert_eq!(after.height, rect.height);
        assert!(
            after.width >= rect.width,
            "{} < {}",
            after.width,
            rect.width
        );
    }

    #[test]
    fn shrinking_a_window_past_the_stage_edge_still_shrinks_it() {
        let runtime = crate::widgets::Runtime::new();
        let (mut scene, mut layout, panel, rect) = overhanging(&runtime);
        let after = drag_grip(
            &runtime,
            &mut scene,
            &mut layout,
            panel,
            rect,
            Point { x: -40.0, y: -30.0 },
        );
        assert_eq!(after.width, rect.width - 40.0);
        assert_eq!(after.height, rect.height - 30.0);
    }
    #[test]
    fn a_vertical_grow_after_the_stage_shrank_under_the_window_keeps_its_width() {
        let runtime = crate::widgets::Runtime::new();
        let (mut scene, mut layout, panel, _) = overhanging(&runtime);
        let root = scene.root().unwrap();
        let small = Size {
            width: 320.0,
            height: 480.0,
        };
        layout.sync_tree(&scene, root);
        layout.compute(&mut scene, root, small);
        let rect = scene.layout(panel).unwrap().rect;
        let stage = scene.node(panel).and_then(|node| node.parent).unwrap();
        let stage_rect = scene.layout(stage).unwrap().rect;
        assert!(rect.width + (rect.x - stage_rect.x) > stage_rect.width);
        let grip = Point {
            x: rect.right() - 2.0,
            y: rect.bottom() - 2.0,
        };
        assert!(begin_dialog_pointer(&runtime, &scene, grip));
        update_dialog_pointer(
            &runtime,
            &scene,
            &mut layout,
            Point {
                x: grip.x,
                y: grip.y + 40.0,
            },
        );
        layout.sync_tree(&scene, root);
        layout.compute(&mut scene, root, small);
        let after = scene.layout(panel).unwrap().rect;
        assert_eq!(after.width, rect.width);
        assert_eq!(after.height, rect.height + 40.0);
    }

    fn settle(runtime: &crate::widgets::Runtime, scene: &mut Scene, layout: &mut LayoutEngine) {
        let root = scene.root().unwrap();
        let size = Size {
            width: 640.0,
            height: 480.0,
        };
        layout.sync_tree(scene, root);
        layout.compute(scene, root, size);
        reposition_paint(runtime, scene);
    }

    fn center(rect: Rect) -> Point {
        Point {
            x: rect.x + rect.width / 2.0,
            y: rect.y + rect.height / 2.0,
        }
    }

    /// Shrinking a window below its content must not let the content paint or
    /// take pointer hits outside the frame, and growing it back restores it.
    #[test]
    fn a_window_shrunk_below_its_content_clips_paint_and_hits_to_the_frame() {
        for decorated in [true, false] {
            let runtime = crate::widgets::Runtime::new();
            let mut column = Column::new();
            for n in 0..8 {
                column = column.child(Button::new(format!("Row {n}")).height(40.0));
            }
            let mut dialog = Dialog::new("Issue board")
                .modeless()
                .non_fixed()
                .movable()
                .resizable()
                .at(100.0, 60.0)
                .size(360.0, 400.0)
                .min_size(220.0, 120.0)
                .child(column);
            if !decorated {
                dialog = dialog.undecorated();
            }
            let (mut scene, mut layout, root) = build_with_layout(
                &runtime,
                dialog,
                Size {
                    width: 640.0,
                    height: 480.0,
                },
            );
            let panel = find_kind(&scene, root, WidgetKind::Dialog).unwrap();
            let rect = scene.layout(panel).unwrap().rect;
            let mut buttons = Vec::new();
            fn collect(scene: &Scene, id: WidgetId, out: &mut Vec<WidgetId>) {
                if scene.node(id).unwrap().kind == WidgetKind::Button {
                    out.push(id);
                }
                for &c in &scene.node(id).unwrap().children {
                    collect(scene, c, out);
                }
            }
            collect(&scene, panel, &mut buttons);
            let last = *buttons.last().unwrap();
            let last_point = center(scene.layout(last).unwrap().rect);
            assert_eq!(hit_test(&runtime, &scene, last_point), Some(last));

            let grip = Point {
                x: rect.right() - 2.0,
                y: rect.bottom() - 2.0,
            };
            assert!(begin_dialog_pointer(&runtime, &scene, grip));
            update_dialog_pointer(
                &runtime,
                &scene,
                &mut layout,
                Point {
                    x: grip.x - 400.0,
                    y: grip.y - 400.0,
                },
            );
            end_dialog_pointer(&runtime);
            settle(&runtime, &mut scene, &mut layout);
            let small = scene.layout(panel).unwrap().rect;
            assert_eq!((small.width, small.height), (220.0, 120.0));

            let outside = scene.layout(last).unwrap().rect;
            assert!(outside.y > small.bottom(), "{outside:?} vs {small:?}");
            let p = center(outside);
            assert_ne!(
                hit_test(&runtime, &scene, p),
                Some(last),
                "decorated={decorated}"
            );
            assert!(
                scene.composited_rect(last).is_empty(),
                "painted outside the frame: {:?}",
                scene.composited_rect(last)
            );
            assert!(scene.accessibility_rect(last).is_empty());
            // Every control keeps its own paint inside the frame.
            for &b in &buttons {
                let r = scene.composited_rect(b);
                assert!(
                    r.is_empty() || (r.y >= small.y && r.bottom() <= small.bottom()),
                    "{r:?} escapes {small:?}"
                );
            }

            // Growing the window back brings the control back.
            let grip = Point {
                x: small.right() - 2.0,
                y: small.bottom() - 2.0,
            };
            assert!(begin_dialog_pointer(&runtime, &scene, grip));
            update_dialog_pointer(
                &runtime,
                &scene,
                &mut layout,
                Point {
                    x: grip.x + 400.0,
                    y: grip.y + 400.0,
                },
            );
            end_dialog_pointer(&runtime);
            settle(&runtime, &mut scene, &mut layout);
            let p = center(scene.layout(last).unwrap().rect);
            assert_eq!(
                hit_test(&runtime, &scene, p),
                Some(last),
                "decorated={decorated}"
            );
        }
    }

    /// A fixed modal dialog shrunk to a small viewport keeps tall content
    /// inside its frame too.
    #[test]
    fn a_fixed_dialog_shrunk_to_the_viewport_clips_content_that_no_longer_fits() {
        let runtime = crate::widgets::Runtime::new();
        let mut column = Column::new();
        for n in 0..10 {
            column = column.child(Button::new(format!("Row {n}")).height(40.0));
        }
        let (scene, root) = build(
            &runtime,
            Dialog::new("Quick command")
                .size(300.0, 900.0)
                .child(column),
            Size {
                width: 444.0,
                height: 428.0,
            },
        );
        let panel = find_kind(&scene, root, WidgetKind::Dialog).unwrap();
        let frame = scene.layout(panel).unwrap().rect;
        let last = {
            let mut found = None;
            let mut stack = vec![panel];
            while let Some(id) = stack.pop() {
                let node = scene.node(id).unwrap();
                if node.kind == WidgetKind::Button {
                    found = Some(id);
                }
                stack.extend(node.children.iter().rev().copied());
            }
            found.unwrap()
        };
        let rect = scene.layout(last).unwrap().rect;
        assert!(rect.bottom() > frame.bottom(), "content must overflow");
        assert_ne!(hit_test(&runtime, &scene, center(rect)), Some(last));
        assert!(scene.composited_rect(last).is_empty());
    }
}
