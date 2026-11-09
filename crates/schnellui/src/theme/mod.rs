//! Ready-made design-system themes for schnellui.
//!
//! The [`Theme`](schnellui::widgets::Theme) and
//! [`Shape`](schnellui::widgets::Shape) abstractions, along with the app-owned
//! theme runtime, live in `schnellui::widgets`. This crate contains only
//! concrete theme instances so applications can opt into them independently.

use schnellui::scene::Color;
use schnellui::widgets::{
    ComponentInteractions, InteractionStates, InteractionStyle, Shape, Theme,
};

/// The default light design: a cool slate neutral ramp from ink to page.
pub const LIGHT: Theme = schnellui::widgets::DEFAULT_THEME;

/// Hover, focus and press for a palette: hover lays a faint wash of `ink`
/// over whatever it touches, press a stronger one, and focus is an accent
/// ring. Text fields answer hover with a darker border instead of a wash.
const fn states(ink: Color, hover: u8, press: u8, accent: Color) -> InteractionStates {
    InteractionStates {
        hover: InteractionStyle::background(Color::rgba(ink.r, ink.g, ink.b, hover)),
        focus: InteractionStyle::border(accent),
        active: InteractionStyle::background(Color::rgba(ink.r, ink.g, ink.b, press)),
    }
}

/// Component overrides: buttons take a stronger wash of `ink` than rows, so
/// the change shows on a filled button; fields answer hover with `border`,
/// a step between the rest outline and the ink, so hover stays below the
/// accent focus border.
const fn field_states(ink: Color, border: Color, accent: Color) -> ComponentInteractions {
    ComponentInteractions {
        button: Some(states(ink, 0x40, 0x60, accent)),
        editable: Some(InteractionStates {
            hover: InteractionStyle::border(border),
            focus: InteractionStyle::border(accent),
            active: InteractionStyle::NONE,
        }),
        ..ComponentInteractions::NONE
    }
}

/// A dark design with deep neutral surfaces and a lifted blue accent.
pub const DARK: Theme = Theme {
    text: Color::rgb(0xe6, 0xe8, 0xec),
    text_muted: Color::rgb(0xa0, 0xa6, 0xb0),
    surface: Color::rgb(0x2a, 0x2d, 0x33),
    surface_muted: Color::rgb(0x34, 0x38, 0x40),
    separator: Color::rgb(0x40, 0x45, 0x4e),
    outline: Color::rgb(0x7a, 0x82, 0x8f),
    accent: Color::rgb(0x5c, 0x8c, 0xe6),
    on_accent: Color::rgb(0x10, 0x14, 0x1c),
    selection: Color::rgb(0x30, 0x46, 0x70),
    interactions: states(Color::WHITE, 0x1c, 0x34, Color::rgb(0x5c, 0x8c, 0xe6)),
    component_interactions: field_states(
        Color::WHITE,
        Color::rgb(0xbc, 0xc1, 0xc7),
        Color::rgb(0x5c, 0x8c, 0xe6),
    ),
    text_selection: Color::rgb(0x2f, 0x4a, 0x75),
    disabled: Color::rgb(0x33, 0x36, 0x3c),
    positive: Color::rgb(0x3f, 0xae, 0x5e),
    attention: Color::rgb(0xc8, 0x45, 0x41),
    media: Color::rgb(0x4e, 0x54, 0x5e),
    page: Color::rgb(0x17, 0x19, 0x1d),
    shape: Shape::CLASSIC,
};

/// A warm light design with a green accent.
pub const FOREST: Theme = Theme {
    text: Color::rgb(0x1d, 0x2a, 0x22),
    text_muted: Color::rgb(0x56, 0x66, 0x5c),
    surface: Color::rgb(0xfb, 0xfd, 0xf9),
    surface_muted: Color::rgb(0xee, 0xf3, 0xea),
    separator: Color::rgb(0xcf, 0xd9, 0xcb),
    outline: Color::rgb(0x7d, 0x8c, 0x80),
    accent: Color::rgb(0x2e, 0x7d, 0x4f),
    on_accent: Color::WHITE,
    selection: Color::rgb(0xb0, 0xd2, 0xbd),
    interactions: states(
        Color::rgb(0x1d, 0x2a, 0x22),
        0x1c,
        0x38,
        Color::rgb(0x2e, 0x7d, 0x4f),
    ),
    component_interactions: field_states(
        Color::rgb(0x1d, 0x2a, 0x22),
        Color::rgb(0x4d, 0x5b, 0x51),
        Color::rgb(0x2e, 0x7d, 0x4f),
    ),
    text_selection: Color::rgb(0xbf, 0xe3, 0xcc),
    disabled: Color::rgb(0xe1, 0xe7, 0xde),
    positive: Color::rgb(0x1f, 0x6b, 0x40),
    attention: Color::rgb(0xb0, 0x4a, 0x2f),
    media: Color::rgb(0xb3, 0xc0, 0xb0),
    page: Color::rgb(0xe8, 0xee, 0xe5),
    shape: Shape::CLASSIC,
};

/// A neo-brutalist design with square controls, ink frames, and hard shadows.
pub const BRUTAL: Theme = Theme {
    text: Color::rgb(0x14, 0x12, 0x0e),
    text_muted: Color::rgb(0x57, 0x52, 0x48),
    surface: Color::WHITE,
    surface_muted: Color::rgb(0xff, 0xe9, 0x4a),
    separator: Color::rgb(0x14, 0x12, 0x0e),
    outline: Color::rgb(0x14, 0x12, 0x0e),
    accent: Color::rgb(0xff, 0xd5, 0x00),
    on_accent: Color::rgb(0x14, 0x12, 0x0e),
    selection: Color::rgb(0xff, 0xe9, 0x4a),
    interactions: InteractionStates {
        hover: InteractionStyle::background(Color::rgb(0xff, 0xf3, 0xa6)),
        focus: InteractionStyle::border(Color::rgb(0x14, 0x12, 0x0e)),
        active: InteractionStyle::background(Color::rgb(0xff, 0xd5, 0x00)),
    },
    component_interactions: ComponentInteractions {
        // A yellow wash would vanish on a yellow button: buttons darken.
        button: Some(states(
            Color::rgb(0x14, 0x12, 0x0e),
            0x1c,
            0x34,
            Color::rgb(0x14, 0x12, 0x0e),
        )),
        ..field_states(
            Color::rgb(0x14, 0x12, 0x0e),
            Color::rgb(0x14, 0x12, 0x0e),
            Color::rgb(0x14, 0x12, 0x0e),
        )
    },
    text_selection: Color::rgb(0xff, 0xd5, 0x00),
    disabled: Color::rgb(0xe6, 0xe1, 0xd3),
    positive: Color::rgb(0x0c, 0x83, 0x46),
    attention: Color::rgb(0xd0, 0x2f, 0x3c),
    media: Color::rgb(0xd9, 0xd2, 0xc0),
    page: Color::rgb(0xf2, 0xef, 0xe3),
    shape: Shape {
        roundness: 0.0,
        density: 1.6,
        frame: 2.0,
        shadow: 4.0,
    },
};

/// A candy design with pill-shaped controls and an airy pastel palette.
pub const CANDY: Theme = Theme {
    text: Color::rgb(0x53, 0x2b, 0x4d),
    text_muted: Color::rgb(0x7a, 0x5c, 0x73),
    surface: Color::WHITE,
    surface_muted: Color::rgb(0xff, 0xee, 0xf7),
    separator: Color::rgb(0xec, 0xc6, 0xdc),
    outline: Color::rgb(0xb8, 0x6e, 0xa2),
    accent: Color::rgb(0xd6, 0x24, 0x6e),
    on_accent: Color::WHITE,
    selection: Color::rgb(0xfa, 0xbf, 0xda),
    interactions: states(
        Color::rgb(0xd6, 0x24, 0x6e),
        0x1a,
        0x3c,
        Color::rgb(0xd6, 0x24, 0x6e),
    ),
    // Buttons and fields answer hover in the plum ink: a pink wash would
    // vanish on the pink button.
    component_interactions: field_states(
        Color::rgb(0x53, 0x2b, 0x4d),
        Color::rgb(0x86, 0x4c, 0x78),
        Color::rgb(0xd6, 0x24, 0x6e),
    ),
    text_selection: Color::rgb(0xff, 0xc2, 0xe0),
    disabled: Color::rgb(0xf5, 0xe8, 0xf0),
    positive: Color::rgb(0x1f, 0x8a, 0x66),
    attention: Color::rgb(0xc6, 0x3c, 0x22),
    media: Color::rgb(0xe9, 0xc3, 0xdb),
    page: Color::rgb(0xff, 0xf0, 0xf7),
    shape: Shape {
        roundness: 6.0,
        density: 1.35,
        frame: 0.0,
        shadow: 0.0,
    },
};

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::widgets::contrast_ratio;

    #[test]
    fn built_in_themes_are_distinct() {
        assert_ne!(LIGHT, DARK);
        assert_ne!(LIGHT, FOREST);
        assert_ne!(DARK, FOREST);
        assert_ne!(BRUTAL, CANDY);
        assert_eq!(LIGHT.accent, Color::rgb(0x2e, 0x63, 0xd4));
        assert_eq!(LIGHT.selection, Color::rgb(0xb5, 0xc9, 0xf3));
    }

    #[test]
    fn light_matches_the_widget_default() {
        assert_eq!(LIGHT, Theme::default());
    }

    #[test]
    fn physical_themes_reshape_controls() {
        assert_eq!(LIGHT.shape, Shape::CLASSIC);
        assert_eq!(DARK.shape, Shape::CLASSIC);
        assert_eq!(FOREST.shape, Shape::CLASSIC);

        assert_eq!(BRUTAL.shape.radius(4.0, 24.0), 0.0);
        assert_eq!(BRUTAL.shape.pill(20.0), 0.0);
        assert!(BRUTAL.shape.pad(8.0) > 8.0);

        assert_eq!(CANDY.shape.radius(4.0, 24.0), 12.0);
        assert_eq!(CANDY.shape.pill(20.0), 10.0);
    }

    #[test]
    fn focus_colors_keep_non_text_contrast_on_every_common_surface() {
        for theme in [LIGHT, DARK, FOREST, BRUTAL, CANDY] {
            let focus = theme.focus_color();
            // The ring sits outside a filled control, past a gap, so it
            // never meets the accent fill itself.
            for background in [
                theme.page,
                theme.surface,
                theme.surface_muted,
                theme.selection,
            ] {
                assert!(
                    contrast_ratio(focus, background) >= 3.0,
                    "focus {focus:?} lacks 3:1 contrast against {background:?}"
                );
            }
        }
        assert_ne!(
            BRUTAL.focus_color(),
            BRUTAL.accent,
            "yellow accent must be darkened for a light surround"
        );
    }
}
