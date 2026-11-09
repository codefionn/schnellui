//! Content-sized decorated surfaces.
//!
//! [`Panel`] is the flow-layout counterpart to a fixed [`Stack`](crate::widgets::Stack)
//! surface: its child determines its height (including width-aware text), while
//! the background, outline, and optional accent rail repaint from the resulting
//! post-layout rectangle.

use schnellui::a11y::Role;
use schnellui::layout::{Container, ContainerStyle, EdgeInsets};
use schnellui::scene::{Color, Primitive, Rect, WidgetId, WidgetKind};

use crate::widgets::{theme_for, AnyView, BuildCtx, View};

#[derive(Clone, Copy)]
pub(crate) struct PanelState {
    surface: Option<Color>,
    outline: Option<Color>,
    draw_outline: bool,
    rail: Option<Color>,
    rail_width: f32,
    frame: f32,
    radius: f32,
    shadow: bool,
}

/// A padded surface whose outer height follows its child's measured height.
///
/// Unlike composing a fixed-size painted leaf under content in a [`Stack`](crate::widgets::Stack),
/// a `Panel` participates in ordinary flow. This makes it suitable for wrapping
/// text, rich documents, and other content whose height changes with width or
/// reactive state.
pub struct Panel {
    child: Option<AnyView>,
    padding: EdgeInsets,
    width: Option<f32>,
    height: Option<f32>,
    min_width: Option<f32>,
    min_height: Option<f32>,
    surface: Option<Color>,
    outline: Option<Color>,
    draw_outline: bool,
    rail: Option<Color>,
    rail_width: f32,
    frame: f32,
    radius: f32,
    shadow: bool,
}

impl Panel {
    /// Creates a theme-colored, content-sized panel with no padding.
    pub fn new() -> Self {
        Self {
            child: None,
            padding: EdgeInsets::default(),
            width: None,
            height: None,
            min_width: None,
            min_height: None,
            surface: None,
            outline: None,
            draw_outline: true,
            rail: None,
            rail_width: 2.0,
            frame: 1.0,
            radius: 1.0,
            shadow: false,
        }
    }

    /// Sets or replaces the panel's single content child.
    pub fn child(mut self, child: impl View) -> Self {
        self.child = Some(Box::new(child));
        self
    }

    /// Applies uniform content padding.
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = EdgeInsets::all(padding.max(0.0));
        self
    }

    /// Applies per-edge content padding.
    pub fn insets(mut self, insets: EdgeInsets) -> Self {
        self.padding = insets;
        self
    }

    /// Fixes the outer width while leaving height content-sized.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width.max(0.0));
        self
    }

    /// Fixes the outer height. Omit this for dynamic flow content.
    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height.max(0.0));
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = Some(width.max(0.0));
        self
    }

    pub fn min_height(mut self, height: f32) -> Self {
        self.min_height = Some(height.max(0.0));
        self
    }

    /// Overrides the ambient theme surface color.
    pub fn surface(mut self, color: Color) -> Self {
        self.surface = Some(color);
        self
    }

    /// Overrides the ambient theme separator used for the outline.
    pub fn outline(mut self, color: Color) -> Self {
        self.outline = Some(color);
        self.draw_outline = true;
        self
    }

    /// Removes the outline and lets the surface occupy the full panel rectangle.
    pub fn no_outline(mut self) -> Self {
        self.draw_outline = false;
        self
    }

    /// Adds an accent rail inside the panel's leading edge.
    pub fn rail(mut self, color: Color) -> Self {
        self.rail = Some(color);
        self
    }

    pub fn rail_width(mut self, width: f32) -> Self {
        self.rail_width = width.max(0.0);
        self
    }

    pub fn frame(mut self, width: f32) -> Self {
        self.frame = width.max(0.0);
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius.max(0.0);
        self
    }

    /// Lifts the panel off the page with a shadow: soft layers in a flat
    /// design, the hard block offset of [`Shape::shadow`](crate::widgets::Shape)
    /// in a design that has one.
    pub fn shadow(mut self) -> Self {
        self.shadow = true;
        self
    }

    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Pad
    }
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Panel {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Pad, parent);
        ctx.scene.a11y_mut(id).role = Role::Group.as_u16();

        let mut style = ContainerStyle::new(Container::Pad(this.padding));
        style.width = this.width;
        style.height = this.height;
        style.min_width = this.min_width;
        style.min_height = this.min_height;
        ctx.layout.set_container(id, style);

        if let Some(child) = this.child {
            child.build(ctx, Some(id));
        }

        ctx.runtime.with(|runtime| {
            runtime.borrow_mut().panels.insert(
                id,
                PanelState {
                    surface: this.surface,
                    outline: this.outline,
                    draw_outline: this.draw_outline,
                    rail: this.rail,
                    rail_width: this.rail_width,
                    frame: this.frame,
                    radius: this.radius,
                    shadow: this.shadow,
                },
            );
        });
        emit_paint(&ctx.runtime, ctx.scene, id);
        id
    }
}

fn emit_paint(
    runtime: &crate::widgets::Runtime,
    scene: &mut schnellui::scene::Scene,
    id: WidgetId,
) {
    let Some(state) = runtime.with(|runtime| runtime.borrow().panels.get(id).copied()) else {
        return;
    };
    let rect = scene.layout(id).map_or(Rect::ZERO, |layout| layout.rect);
    let theme = theme_for(runtime, id);
    let frame = if state.draw_outline {
        state.frame.min(rect.width * 0.5).min(rect.height * 0.5)
    } else {
        0.0
    };
    let inner = Rect::new(
        rect.x + frame,
        rect.y + frame,
        (rect.width - frame * 2.0).max(0.0),
        (rect.height - frame * 2.0).max(0.0),
    );
    let paint = scene.paint_mut(id);
    paint.primitives.clear();
    if state.shadow && !rect.is_empty() {
        push_shadow(&mut paint.primitives, rect, state.radius, theme);
    }
    if state.draw_outline && frame > 0.0 {
        paint.primitives.push(Primitive::SolidRect {
            rect,
            color: state.outline.unwrap_or(theme.separator),
            corner_radius: state.radius,
        });
    }
    paint.primitives.push(Primitive::SolidRect {
        rect: inner,
        color: state.surface.unwrap_or(theme.surface),
        corner_radius: (state.radius - frame).max(0.0),
    });
    if let Some(color) = state.rail {
        let rail_width = state.rail_width.min(inner.width);
        if rail_width > 0.0 && inner.height > 0.0 {
            paint.primitives.push(Primitive::SolidRect {
                rect: Rect::new(inner.x, inner.y, rail_width, inner.height),
                color,
                corner_radius: 0.0,
            });
        }
    }
}

/// The layers of a floating surface's soft shadow: how far each reaches past
/// the surface and its alpha. Wider layers are fainter.
const SOFT_SHADOW: [(f32, u8); 3] = [(6.0, 0x0a), (3.0, 0x12), (1.0, 0x22)];
/// How far a soft shadow sits below its surface, as if lit from above.
const SOFT_SHADOW_DROP: f32 = 3.0;

/// Pushes the shadow of a floating surface at `rect`: a hard ink block under
/// a design with [`Shape::shadow`](crate::widgets::Shape), soft fading layers
/// otherwise.
pub(crate) fn push_shadow(
    primitives: &mut Vec<Primitive>,
    rect: Rect,
    radius: f32,
    theme: crate::widgets::Theme,
) {
    if theme.shape.shadow > 0.0 {
        let offset = theme.shape.shadow;
        primitives.push(Primitive::SolidRect {
            rect: Rect::new(rect.x + offset, rect.y + offset, rect.width, rect.height),
            color: theme.text,
            corner_radius: radius,
        });
        return;
    }
    for (grow, alpha) in SOFT_SHADOW {
        primitives.push(Primitive::SolidRect {
            rect: Rect::new(
                rect.x - grow,
                rect.y - grow + SOFT_SHADOW_DROP,
                rect.width + 2.0 * grow,
                rect.height + 2.0 * grow,
            ),
            color: Color::rgba(0, 0, 0, alpha),
            corner_radius: radius + grow,
        });
    }
}

/// Registers `id` (an already-inserted padded container) as a floating
/// popup surface: theme surface, outline, rounded corners and a shadow,
/// repainted from its final layout box like any [`Panel`].
pub(crate) fn register_popup_surface(
    runtime: &crate::widgets::Runtime,
    scene: &mut schnellui::scene::Scene,
    id: WidgetId,
) {
    let theme = theme_for(runtime, id);
    runtime.with(|runtime| {
        runtime.borrow_mut().panels.insert(
            id,
            PanelState {
                surface: None,
                outline: Some(theme.outline),
                draw_outline: true,
                rail: None,
                rail_width: 0.0,
                frame: theme.shape.frame.max(1.0),
                radius: theme.shape.radius(6.0, 24.0).min(POPUP_MAX_RADIUS),
                shadow: true,
            },
        );
    });
    emit_paint(runtime, scene, id);
}

/// The largest corner radius a popup takes, so a very round design still
/// leaves its rows square enough to read as a list.
const POPUP_MAX_RADIUS: f32 = 10.0;

/// Repaints a panel from its final layout rectangle.
pub(crate) fn reposition(
    runtime: &crate::widgets::Runtime,
    scene: &mut schnellui::scene::Scene,
    id: WidgetId,
) -> bool {
    if !runtime.with(|runtime| runtime.borrow().panels.contains_key(id)) {
        return false;
    }
    emit_paint(runtime, scene, id);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{measure_text, reposition_paint, Context, RichDoc, RichText, Runtime};
    use schnellui::layout::LayoutEngine;
    use schnellui::scene::{Scene, Size};
    use schnellui::text::{GlyphAtlas, TextShaper};

    #[test]
    fn panel_tracks_a_dynamic_rich_document_height_and_repaints_its_surface() {
        let runtime = Runtime::new();
        let content = schnellui::signal::create_signal(String::from("short"));
        let source = content;
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(1024, 1024);
        let root = {
            let mut ctx = BuildCtx {
                context: Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(
                Panel::new()
                    .width(150.0)
                    .padding(8.0)
                    .child(RichText::dynamic(move || RichDoc::plain(&source.get())).size(11.0)),
            )
            .build(&mut ctx, None)
        };
        scene.set_root(root);

        let layout_panel = |scene: &mut Scene,
                            layout: &mut LayoutEngine,
                            text: &mut TextShaper,
                            atlas: &mut GlyphAtlas| {
            layout.sync_tree(scene, root);
            layout.compute_with(
                scene,
                root,
                Size {
                    width: 150.0,
                    height: 600.0,
                },
                &mut |id, available| measure_text(&runtime, id, available, text),
            );
            crate::widgets::emit_wrapped_paint(&runtime, scene, text, atlas);
            reposition_paint(&runtime, scene);
        };
        layout_panel(&mut scene, &mut layout, &mut text, &mut atlas);
        crate::widgets::run_dynamic_slots(&runtime, &mut scene, &mut text, &mut atlas);
        let short_height = scene.layout(root).unwrap().rect.height;

        content.set("a much longer document that wraps across several lines in a narrow panel and makes the decorated container grow without any estimated fixed height".into());
        schnellui::signal::Runtime::flush();
        crate::widgets::run_dynamic_slots(&runtime, &mut scene, &mut text, &mut atlas);
        layout_panel(&mut scene, &mut layout, &mut text, &mut atlas);

        let panel_rect = scene.layout(root).unwrap().rect;
        assert!(panel_rect.height > short_height);
        let Primitive::SolidRect { rect, .. } = scene.paint(root).unwrap().primitives[0] else {
            panic!("panel outline must be a solid rectangle");
        };
        assert_eq!(rect, panel_rect);
    }
}
