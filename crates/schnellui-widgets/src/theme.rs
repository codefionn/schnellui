//! The **design system** (SOUL §8.1): every colour the widget set paints comes
//! from the runtime-selected [`Theme`] — one struct of named tokens, not scattered
//! literals — so a whole visual identity swaps in one call. Ready-made theme
//! instances live in the separate `schnellui-theme` crate.
//!
//! The theme is read at **build and dispatch time** from the app-owned
//! [`crate::Runtime`]. [`ThemeProvider`] explicitly overrides that value for one
//! subtree and remembers it for later interaction-driven repaints.
//!
//! A mounted tree is a static skeleton (SOUL §3.3), so a whole-app change must
//! reconstruct the tree. Applications using the umbrella crate can opt into
//! `App::mount_themed*`; it retains a view factory and performs that reconstruction
//! automatically for explicit, reactive, animated, and native light/dark changes.
//! A one-shot mount supplies its theme through the app [`crate::Context`]. The
//! runtime-selected theme deliberately survives [`crate::reset`], so every
//! remount of that app inherits the choice.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use schnellui_scene::{Color, Scene, WidgetId};
use slotmap::SecondaryMap;

use crate::{BuildCtx, View};

/// The **physical** design tokens (SOUL §8.1): geometry is design too. A theme
/// that only recolours is a coat of paint; these four knobs reshape every
/// control — square it, pill it, thicken it, or float it on a hard shadow — so
/// a design system swap physically rebuilds the widgets, not just their fill.
/// Like the colour tokens, each names a *role* (how round, how dense, how
/// inked), never a widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// Corner-rounding multiplier on every control's classic radius. `1.0` is
    /// the classic look, `0.0` squares everything (pill tracks, knobs and dots
    /// included), large values grow toward pills — [`Shape::radius`] clamps at
    /// half-height so nothing over-rounds.
    pub roundness: f32,
    /// Control padding/size multiplier — the density axis. `1.0` classic,
    /// `>1` chunky, `<1` compact. Applied through [`Shape::pad`] to paddings
    /// and to the toggles' intrinsic boxes.
    pub density: f32,
    /// Ink-frame width around filled controls (button, badge, checkbox, text
    /// input), painted in the [`Theme::outline`] colour. `0.0` = frameless.
    pub frame: f32,
    /// Hard block-shadow offset (down-right, painted in the [`Theme::text`]
    /// ink — the neo-brutalist float). `0.0` = flat.
    pub shadow: f32,
}

impl Shape {
    /// The classic geometry every original palette wears — multiplies out to
    /// exactly the widget constants' historical values (the covenant with old
    /// shots, SOUL §7.3).
    pub const CLASSIC: Shape = Shape {
        roundness: 1.0,
        density: 1.0,
        frame: 0.0,
        shadow: 0.0,
    };

    /// A control's corner radius: its `classic` radius scaled by roundness,
    /// clamped to the pill limit (`height / 2`) so large roundness saturates
    /// into a pill instead of inverting the corners.
    pub fn radius(&self, classic: f32, height: f32) -> f32 {
        (classic * self.roundness).min(height * 0.5)
    }

    /// An intrinsically-round part's radius (switch track/knob, radio rings,
    /// badge pill): already a pill at `1.0`, so roundness only *squares* it —
    /// values above `1.0` clamp (a pill cannot get rounder).
    pub fn pill(&self, height: f32) -> f32 {
        height * 0.5 * self.roundness.min(1.0)
    }

    /// A classic padding/size scaled by the density axis.
    pub fn pad(&self, classic: f32) -> f32 {
        classic * self.density
    }

    /// Linearly interpolates physical design tokens. This is used by a themed
    /// [`schnellui::App`](https://docs.rs/schnellui) while rebuilding transition
    /// frames; geometry and colour therefore arrive at the same intermediate
    /// design instead of controls snapping at the end of a palette fade.
    pub fn lerp(self, other: Shape, amount: f32) -> Shape {
        let t = amount.clamp(0.0, 1.0);
        let f = |a: f32, b: f32| a + (b - a) * t;
        Shape {
            roundness: f(self.roundness, other.roundness),
            density: f(self.density, other.density),
            frame: f(self.frame, other.frame),
            shadow: f(self.shadow, other.shadow),
        }
    }
}

/// Optional visual channels applied for one interaction state.
///
/// Keeping the channels independent lets a theme use a quiet border-only hover,
/// invert only a label, replace only the surface, or coordinate all three. Raw
/// content surfaces such as terminals and document editors deliberately opt out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InteractionStyle {
    pub background: Option<Color>,
    pub foreground: Option<Color>,
    pub border: Option<Color>,
}

impl InteractionStyle {
    pub const NONE: Self = Self {
        background: None,
        foreground: None,
        border: None,
    };

    pub const fn background(color: Color) -> Self {
        Self {
            background: Some(color),
            ..Self::NONE
        }
    }

    pub const fn foreground(color: Color) -> Self {
        Self {
            foreground: Some(color),
            ..Self::NONE
        }
    }

    pub const fn border(color: Color) -> Self {
        Self {
            border: Some(color),
            ..Self::NONE
        }
    }

    pub const fn all(background: Color, foreground: Color, border: Color) -> Self {
        Self {
            background: Some(background),
            foreground: Some(foreground),
            border: Some(border),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InteractionStates {
    pub hover: InteractionStyle,
    pub focus: InteractionStyle,
    pub active: InteractionStyle,
}

impl InteractionStates {
    pub const NONE: Self = Self {
        hover: InteractionStyle::NONE,
        focus: InteractionStyle::NONE,
        active: InteractionStyle::NONE,
    };
}

/// Coarse component families that can receive theme-level interaction overrides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionComponent {
    Button,
    Navigation,
    Toggle,
    Editable,
    RawSurface,
}

/// Optional component-specific interaction states. `None` inherits the theme's
/// global states; raw surfaces and editables retain safe state-specific defaults
/// unless explicitly overridden.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ComponentInteractions {
    pub button: Option<InteractionStates>,
    pub navigation: Option<InteractionStates>,
    pub toggle: Option<InteractionStates>,
    pub editable: Option<InteractionStates>,
    pub raw_surface: Option<InteractionStates>,
}

impl ComponentInteractions {
    pub const NONE: Self = Self {
        button: None,
        navigation: None,
        toggle: None,
        editable: None,
        raw_surface: None,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionState {
    Hover,
    Focus,
    Active,
}

/// The design-system tokens (SOUL §8.1). Each token names a *role* in the UI,
/// not a widget: the accent is one colour whether it fills a button, underlines
/// a tab, or tints a focused input's border — that sameness is what makes a
/// palette read as one design.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// primary content text.
    pub text: Color,
    /// secondary text: placeholders, captions.
    pub text_muted: Color,
    /// widget surfaces at rest (inputs, unselected tabs/items/cells, checkbox box).
    pub surface: Color,
    /// the alternate/raised surface (the table header row).
    pub surface_muted: Color,
    /// hairline separators (divider, table grid lines).
    pub separator: Color,
    /// control outlines at rest (the text input border).
    pub outline: Color,
    /// the brand accent: button fill, link text, tab indicator, slider/progress
    /// fill, switch-on track, radio dot, focused input border.
    pub accent: Color,
    /// text/glyphs sitting on an accent surface (the button label).
    pub on_accent: Color,
    /// the light accent wash behind anything selected (tab, list item, table
    /// row, dropdown option) and an open dropdown's trigger.
    pub selection: Color,
    /// global hover, keyboard-focus, and active/pressed visual channels.
    pub interactions: InteractionStates,
    /// optional interaction-state overrides for selected component families.
    pub component_interactions: ComponentInteractions,
    /// the text-selection highlight inside editable text.
    pub text_selection: Color,
    /// a disabled control's surface.
    pub disabled: Color,
    /// the affirmative mark (the checkbox check).
    pub positive: Color,
    /// the attention surface (the badge pill).
    pub attention: Color,
    /// the media placeholder fill (an image/icon with no pixels yet).
    pub media: Color,
    /// the page background behind the widget tree — hosts pass it to their
    /// clear color so the window matches the design.
    pub page: Color,
    /// the physical geometry tokens — roundness, density, frame, shadow
    /// (SOUL §8.1): a design system reshapes controls, not just recolours them.
    pub shape: Shape,
}

impl Theme {
    pub fn interaction_style(
        self,
        component: InteractionComponent,
        state: InteractionState,
    ) -> InteractionStyle {
        let override_states = match component {
            InteractionComponent::Button => self.component_interactions.button,
            InteractionComponent::Navigation => self.component_interactions.navigation,
            InteractionComponent::Toggle => self.component_interactions.toggle,
            InteractionComponent::Editable => self.component_interactions.editable,
            InteractionComponent::RawSurface => self.component_interactions.raw_surface,
        };
        let states = override_states.unwrap_or_else(|| match component {
            InteractionComponent::Editable => InteractionStates {
                hover: InteractionStyle {
                    border: self.interactions.hover.border,
                    ..InteractionStyle::NONE
                },
                focus: self.interactions.focus,
                active: InteractionStyle::NONE,
            },
            InteractionComponent::RawSurface => InteractionStates {
                hover: InteractionStyle::NONE,
                focus: self.interactions.focus,
                active: InteractionStyle::NONE,
            },
            _ => self.interactions,
        });
        let mut style = match state {
            InteractionState::Hover => states.hover,
            InteractionState::Focus => states.focus,
            InteractionState::Active => states.active,
        };
        if state == InteractionState::Focus && style.border == Some(self.accent) {
            style.border = Some(self.focus_color());
        }
        style
    }

    /// Focus-indicator color shared by the native HTML and wgpu renderers.
    ///
    /// The brand accent is preserved when it already has at least 3:1 contrast
    /// against every common adjacent surface. Otherwise it is moved toward the
    /// black/white pole with the best worst-case contrast until it reaches that
    /// WCAG non-text contrast threshold. This keeps focus recognizable as an
    /// accent while preventing low-contrast palettes (notably yellow or pastel
    /// accents on light surfaces) from making keyboard focus disappear.
    pub fn focus_color(self) -> Color {
        const MIN_CONTRAST: f32 = 3.0;
        let backgrounds = [
            self.page,
            self.surface,
            self.surface_muted,
            self.selection,
            self.accent,
        ];
        let minimum_contrast = |candidate| {
            backgrounds
                .iter()
                .map(|background| contrast_ratio(candidate, *background))
                .fold(f32::INFINITY, f32::min)
        };
        if minimum_contrast(self.accent) >= MIN_CONTRAST {
            return self.accent;
        }

        let black_score = minimum_contrast(Color::BLACK);
        let white_score = minimum_contrast(Color::WHITE);
        let pole = if black_score >= white_score {
            Color::BLACK
        } else {
            Color::WHITE
        };
        for step in 1..=20 {
            let amount = step as f32 / 20.0;
            let candidate = mix_color(self.accent, pole, amount);
            if minimum_contrast(candidate) >= MIN_CONTRAST {
                return candidate;
            }
        }
        pole
    }

    /// Linearly interpolates all colour and physical tokens.
    ///
    /// Components are interpolated in straight sRGB byte space. The method is
    /// deterministic and clamps `amount` to `0..=1`.
    pub fn lerp(self, other: Theme, amount: f32) -> Theme {
        fn channel(a: u8, b: u8, t: f32) -> u8 {
            (a as f32 + (b as f32 - a as f32) * t).round() as u8
        }
        fn color(a: Color, b: Color, t: f32) -> Color {
            Color::rgba(
                channel(a.r, b.r, t),
                channel(a.g, b.g, t),
                channel(a.b, b.b, t),
                channel(a.a, b.a, t),
            )
        }
        fn optional_color(a: Option<Color>, b: Option<Color>, t: f32) -> Option<Color> {
            if t <= 0.0 {
                return a;
            }
            if t >= 1.0 {
                return b;
            }
            match (a, b) {
                (Some(a), Some(b)) => Some(color(a, b, t)),
                (Some(a), None) => Some(color(a, Color::rgba(a.r, a.g, a.b, 0), t)),
                (None, Some(b)) => Some(color(Color::rgba(b.r, b.g, b.b, 0), b, t)),
                (None, None) => None,
            }
        }
        fn style(a: InteractionStyle, b: InteractionStyle, t: f32) -> InteractionStyle {
            InteractionStyle {
                background: optional_color(a.background, b.background, t),
                foreground: optional_color(a.foreground, b.foreground, t),
                border: optional_color(a.border, b.border, t),
            }
        }
        fn states(a: InteractionStates, b: InteractionStates, t: f32) -> InteractionStates {
            InteractionStates {
                hover: style(a.hover, b.hover, t),
                focus: style(a.focus, b.focus, t),
                active: style(a.active, b.active, t),
            }
        }
        fn optional_states(
            a: Option<InteractionStates>,
            b: Option<InteractionStates>,
            t: f32,
        ) -> Option<InteractionStates> {
            match (a, b) {
                (Some(a), Some(b)) => Some(states(a, b, t)),
                (Some(a), None) if t < 1.0 => Some(a),
                (None, Some(b)) if t > 0.0 => Some(b),
                _ if t <= 0.0 => a,
                _ => b,
            }
        }

        let t = amount.clamp(0.0, 1.0);
        Theme {
            text: color(self.text, other.text, t),
            text_muted: color(self.text_muted, other.text_muted, t),
            surface: color(self.surface, other.surface, t),
            surface_muted: color(self.surface_muted, other.surface_muted, t),
            separator: color(self.separator, other.separator, t),
            outline: color(self.outline, other.outline, t),
            accent: color(self.accent, other.accent, t),
            on_accent: color(self.on_accent, other.on_accent, t),
            selection: color(self.selection, other.selection, t),
            interactions: states(self.interactions, other.interactions, t),
            component_interactions: ComponentInteractions {
                button: optional_states(
                    self.component_interactions.button,
                    other.component_interactions.button,
                    t,
                ),
                navigation: optional_states(
                    self.component_interactions.navigation,
                    other.component_interactions.navigation,
                    t,
                ),
                toggle: optional_states(
                    self.component_interactions.toggle,
                    other.component_interactions.toggle,
                    t,
                ),
                editable: optional_states(
                    self.component_interactions.editable,
                    other.component_interactions.editable,
                    t,
                ),
                raw_surface: optional_states(
                    self.component_interactions.raw_surface,
                    other.component_interactions.raw_surface,
                    t,
                ),
            },
            text_selection: color(self.text_selection, other.text_selection, t),
            disabled: color(self.disabled, other.disabled, t),
            positive: color(self.positive, other.positive, t),
            attention: color(self.attention, other.attention, t),
            media: color(self.media, other.media, t),
            page: color(self.page, other.page, t),
            shape: self.shape.lerp(other.shape, t),
        }
    }
}

fn mix_color(a: Color, b: Color, amount: f32) -> Color {
    let t = amount.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color::rgba(
        channel(a.r, b.r),
        channel(a.g, b.g),
        channel(a.b, b.b),
        channel(a.a, b.a),
    )
}

/// WCAG contrast ratio using relative luminance in linear-light sRGB.
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    fn linear(channel: u8) -> f32 {
        let value = channel as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    fn luminance(color: Color) -> f32 {
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

// The widget crate owns a deterministic fallback because runtime theme selection
// must work without depending on a concrete-theme crate. `schnellui-theme::LIGHT`
// is tested against this value so the separately packaged default stays aligned.
const DEFAULT_THEME: Theme = Theme {
    text: Color::rgb(0x1d, 0x24, 0x2e),
    text_muted: Color::rgb(0x5d, 0x6a, 0x7a),
    surface: Color::WHITE,
    surface_muted: Color::rgb(0xf1, 0xf3, 0xf7),
    separator: Color::rgb(0xd3, 0xd9, 0xe2),
    outline: Color::rgb(0x8a, 0x96, 0xa8),
    accent: Color::rgb(0x2e, 0x63, 0xd4),
    on_accent: Color::WHITE,
    selection: Color::rgb(0xdb, 0xe6, 0xf9),
    interactions: InteractionStates {
        hover: InteractionStyle::all(
            Color::rgba(0x2e, 0x63, 0xd4, 0x18),
            Color::rgb(0x1d, 0x24, 0x2e),
            Color::rgba(0x2e, 0x63, 0xd4, 0xb8),
        ),
        focus: InteractionStyle::border(Color::rgb(0x2e, 0x63, 0xd4)),
        active: InteractionStyle::background(Color::rgb(0xdb, 0xe6, 0xf9)),
    },
    component_interactions: ComponentInteractions::NONE,
    text_selection: Color::rgb(0xb7, 0xd2, 0xf8),
    disabled: Color::rgb(0x9d, 0xa7, 0xb5),
    positive: Color::rgb(0x17, 0x87, 0x45),
    attention: Color::rgb(0xcf, 0x3d, 0x3d),
    media: Color::rgb(0xc2, 0xc9, 0xd4),
    page: Color::rgb(0xe7, 0xea, 0xf0),
    shape: Shape::CLASSIC,
};

impl Default for Theme {
    fn default() -> Self {
        DEFAULT_THEME
    }
}

#[derive(Clone)]
pub(crate) struct Runtime {
    selected: Rc<Cell<Theme>>,
    scoped: Rc<RefCell<SecondaryMap<WidgetId, Theme>>>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            selected: Rc::new(Cell::new(DEFAULT_THEME)),
            scoped: Rc::new(RefCell::new(SecondaryMap::new())),
        }
    }
}

impl Runtime {
    fn with_scoped<R>(
        &self,
        access: impl FnOnce(&RefCell<SecondaryMap<WidgetId, Theme>>) -> R,
    ) -> R {
        access(&self.scoped)
    }
}

/// Sets one app runtime's design system for subsequent build and dispatch.
/// Deliberately survives [`crate::reset`] so remounts of that runtime inherit
/// the choice.
pub fn set_theme(runtime: &crate::Runtime, theme: Theme) {
    runtime.themes.selected.set(theme);
}

/// One app runtime's selected design system (SOUL §8.1). Widgets read their
/// colours here at build/dispatch time instead of from hardcoded constants.
pub fn theme(runtime: &crate::Runtime) -> Theme {
    runtime.themes.selected.get()
}

/// Runs `f` with `theme` selected in `runtime` and restores the previous value
/// afterwards, including when `f` unwinds.
pub fn with_theme<R>(runtime: &crate::Runtime, selected: Theme, f: impl FnOnce() -> R) -> R {
    struct Restore<'a>(&'a Cell<Theme>, Theme);
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            self.0.set(self.1);
        }
    }

    let cell = &runtime.themes.selected;
    let restore = Restore(cell, cell.replace(selected));
    let value = f();
    drop(restore);
    value
}

/// Returns the theme associated with a retained node.
///
/// Nodes outside a [`ThemeProvider`] use their runtime's selected theme. Scoped nodes keep
/// their provider's theme for later input-driven and dynamic repaints.
pub(crate) fn theme_for(runtime: &crate::Runtime, id: WidgetId) -> Theme {
    runtime
        .themes
        .with_scoped(|themes| themes.borrow().get(id).copied())
        .unwrap_or_else(|| theme(runtime))
}

pub(crate) fn clear_scoped_themes(runtime: &crate::Runtime) {
    runtime
        .themes
        .with_scoped(|themes| themes.borrow_mut().clear());
}

pub(crate) fn remember_node_theme(runtime: &crate::Runtime, id: WidgetId, selected: Theme) {
    runtime.themes.with_scoped(|themes| {
        themes.borrow_mut().insert(id, selected);
    });
}

pub(crate) fn forget_node_theme(runtime: &crate::Runtime, id: WidgetId) {
    runtime.themes.with_scoped(|themes| {
        themes.borrow_mut().remove(id);
    });
}

fn remember_subtree_theme(runtime: &crate::Runtime, scene: &Scene, id: WidgetId, selected: Theme) {
    // Inner providers finish first. `or_insert` semantics preserve their more
    // specific value when an outer provider subsequently walks the same subtree.
    runtime.themes.with_scoped(|themes| {
        let mut themes = themes.borrow_mut();
        if themes.get(id).is_none() {
            themes.insert(id, selected);
        }
    });
    if let Some(node) = scene.node(id) {
        for &child in &node.children {
            remember_subtree_theme(runtime, scene, child, selected);
        }
    }
}

/// Applies a theme to one view subtree.
///
/// The provider is structural only: it does not add a scene/layout node. Nested
/// providers work as expected, and the runtime theme is restored after the child
/// finishes building.
pub struct ThemeProvider {
    selected: Theme,
    child: Box<dyn View>,
}

impl ThemeProvider {
    pub fn new(selected: Theme, child: impl View) -> ThemeProvider {
        ThemeProvider {
            selected,
            child: Box::new(child),
        }
    }
}

impl View for ThemeProvider {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let ThemeProvider { selected, child } = *self;
        let runtime = ctx.runtime.clone();
        let id = with_theme(&runtime, selected, || child.build(ctx, parent));
        remember_subtree_theme(&runtime, ctx.scene, id, selected);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambient_theme_defaults_and_swaps() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let default = Theme::default();
        let alternate = Theme {
            accent: Color::BLACK,
            ..default
        };
        assert_eq!(theme(runtime,), default);
        set_theme(runtime, alternate);
        assert_eq!(theme(runtime,), alternate);
        assert_eq!(theme(runtime,).accent, Color::BLACK);
        set_theme(runtime, default);
    }

    /// A widget built under a swapped theme paints that theme's tokens — the
    /// whole design system changes at the next mount (SOUL §3.3 remount rule).
    #[test]
    fn widgets_build_with_the_ambient_theme() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use crate::View;
        use schnellui_scene::{Primitive, Scene, WidgetKind};

        let build_button_bg = || {
            crate::reset(runtime);
            let mut scene = Scene::new();
            let mut layout = schnellui_layout::LayoutEngine::new();
            let mut text = schnellui_text::TextShaper::new();
            let mut atlas = schnellui_text::GlyphAtlas::new(256, 256);
            let id = {
                let mut ctx = crate::BuildCtx {
                    context: crate::Context::new(),
                    runtime: runtime.clone(),
                    scene: &mut scene,
                    layout: &mut layout,
                    text: &mut text,
                    atlas: &mut atlas,
                    scale: 1.0,
                };
                Box::new(crate::Button::new("ok")).build(&mut ctx, None)
            };
            assert_eq!(scene.node(id).unwrap().kind, WidgetKind::Button);
            match scene.paint(id).unwrap().primitives[0] {
                Primitive::SolidRect { color, .. } => color,
                ref p => panic!("expected the button surface, got {p:?}"),
            }
        };
        let default = Theme::default();
        assert_eq!(build_button_bg(), default.accent);
        let alternate = Theme {
            accent: Color::BLACK,
            ..default
        };
        set_theme(runtime, alternate);
        assert_eq!(build_button_bg(), Color::BLACK);
        set_theme(runtime, default);
    }

    /// The classic shape multiplies out to the identity — the geometry covenant
    /// with old shots (SOUL §7.3).
    #[test]
    fn shape_tokens_scale_and_clamp() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let classic = Shape::CLASSIC;
        assert_eq!(classic.radius(4.0, 24.0), 4.0);
        assert_eq!(classic.pill(20.0), 10.0);
        assert_eq!(classic.pad(8.0), 8.0);
        assert_eq!(Theme::default().shape, Shape::CLASSIC);

        let square = Shape {
            roundness: 0.0,
            density: 1.6,
            frame: 2.0,
            shadow: 4.0,
        };
        assert_eq!(square.radius(4.0, 24.0), 0.0);
        assert_eq!(square.pill(20.0), 0.0);
        assert!(square.pad(8.0) > 8.0);

        let round = Shape {
            roundness: 6.0,
            ..Shape::CLASSIC
        };
        assert_eq!(round.radius(4.0, 24.0), 12.0);
        assert_eq!(round.pill(20.0), 10.0);
    }

    #[test]
    fn interpolation_covers_color_and_shape_tokens() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let from = Theme::default();
        let to = Theme {
            accent: Color::BLACK,
            shape: Shape {
                roundness: 0.0,
                density: 2.0,
                frame: 4.0,
                shadow: 6.0,
            },
            ..from
        };
        assert_eq!(from.lerp(to, 0.0), from);
        assert_eq!(from.lerp(to, 1.0), to);
        let middle = from.lerp(to, 0.5);
        assert_eq!(middle.shape.density, 1.5);
        assert_eq!(middle.shape.frame, 2.0);
    }

    #[test]
    fn interaction_channels_and_component_overrides_resolve_independently() {
        let base = Theme::default();
        let button = InteractionStates {
            hover: InteractionStyle::foreground(Color::rgb(1, 2, 3)),
            focus: InteractionStyle::background(Color::rgb(4, 5, 6)),
            active: InteractionStyle::border(Color::rgb(7, 8, 9)),
        };
        let themed = Theme {
            component_interactions: ComponentInteractions {
                button: Some(button),
                ..ComponentInteractions::NONE
            },
            ..base
        };

        assert_eq!(
            themed.interaction_style(InteractionComponent::Button, InteractionState::Hover),
            button.hover
        );
        assert_eq!(
            themed.interaction_style(InteractionComponent::Button, InteractionState::Focus),
            button.focus
        );
        assert_eq!(
            themed.interaction_style(InteractionComponent::Button, InteractionState::Active),
            button.active
        );
        assert_eq!(
            themed.interaction_style(InteractionComponent::RawSurface, InteractionState::Hover,),
            InteractionStyle::NONE,
        );
    }

    #[test]
    fn with_theme_restores_the_ambient_value() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let original = theme(runtime);
        let alternate = Theme {
            accent: Color::BLACK,
            ..original
        };
        assert_eq!(with_theme(runtime, alternate, || theme(runtime)), alternate);
        assert_eq!(theme(runtime,), original);
    }

    #[test]
    fn provider_scopes_build_and_later_interaction_paint() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use crate::{Checkbox, View};
        use schnellui_scene::{Primitive, Scene};

        let outer = Theme::default();
        let scoped = Theme {
            surface: Color::rgb(4, 5, 6),
            positive: Color::rgb(7, 8, 9),
            ..outer
        };
        set_theme(runtime, outer);
        crate::reset(runtime);
        let mut scene = Scene::new();
        let mut layout = schnellui_layout::LayoutEngine::new();
        let mut text = schnellui_text::TextShaper::new();
        let mut atlas = schnellui_text::GlyphAtlas::new(256, 256);
        let id = {
            let mut ctx = crate::BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(ThemeProvider::new(scoped, Checkbox::new(false))).build(&mut ctx, None)
        };

        assert_eq!(theme(runtime,), outer);
        assert!(matches!(
            scene.paint(id).unwrap().primitives[0],
            Primitive::SolidRect { color, .. } if color == scoped.surface
        ));
        crate::dispatch_click(runtime, &mut scene, id);
        assert!(matches!(
            scene.paint(id).unwrap().primitives[1],
            Primitive::SolidRect { color, .. } if color == scoped.positive
        ));
    }

    /// Regression: deferred wrapped-text paint mutably borrows the widget
    /// runtime. Scoped theme lookup must use independent storage or this path
    /// panics with "RefCell already mutably borrowed".
    #[test]
    fn scoped_wrapped_text_resolves_theme_during_deferred_paint() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use crate::{Text, View, WrapMode};
        use schnellui_scene::{LayoutBox, Primitive, Rect, Scene};

        let scoped = Theme {
            text: Color::rgb(0x12, 0x34, 0x56),
            ..Theme::default()
        };
        crate::reset(runtime);
        let mut scene = Scene::new();
        let mut layout = schnellui_layout::LayoutEngine::new();
        let mut text = schnellui_text::TextShaper::new();
        let mut atlas = schnellui_text::GlyphAtlas::new(256, 256);
        let id = {
            let mut ctx = crate::BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(ThemeProvider::new(
                scoped,
                Text::new("wrapped theme regression").wrap(WrapMode::Word),
            ))
            .build(&mut ctx, None)
        };
        let rect = Rect::new(0.0, 0.0, 120.0, 60.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );

        crate::emit_wrapped_paint(runtime, &mut scene, &mut text, &mut atlas);

        assert!(scene.paint(id).unwrap().primitives.iter().any(|primitive| {
            matches!(
                primitive,
                Primitive::GlyphQuad { color, .. } if *color == scoped.text
            )
        }));
    }
}
