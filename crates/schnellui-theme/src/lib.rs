//! Ready-made design-system themes for schnellui.
//!
//! The [`Theme`](schnellui_widgets::Theme) and
//! [`Shape`](schnellui_widgets::Shape) abstractions, along with the app-owned
//! theme runtime, live in `schnellui-widgets`. This crate contains only
//! concrete theme instances so applications can opt into them independently.

use schnellui_scene::Color;
use schnellui_widgets::{ComponentInteractions, InteractionStates, InteractionStyle, Shape, Theme};

/// The default light design: a cool slate neutral ramp from ink to page.
pub const LIGHT: Theme = Theme {
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

/// A dark design with deep neutral surfaces and a lifted blue accent.
pub const DARK: Theme = Theme {
    text: Color::rgb(0xe8, 0xe8, 0xe8),
    text_muted: Color::rgb(0x9a, 0x9a, 0x9a),
    surface: Color::rgb(0x2a, 0x2d, 0x33),
    surface_muted: Color::rgb(0x34, 0x38, 0x40),
    separator: Color::rgb(0x3d, 0x41, 0x48),
    outline: Color::rgb(0x5a, 0x60, 0x6b),
    accent: Color::rgb(0x5c, 0x8c, 0xe6),
    on_accent: Color::rgb(0x10, 0x14, 0x1c),
    selection: Color::rgb(0x2c, 0x3a, 0x55),
    interactions: InteractionStates {
        hover: InteractionStyle::all(
            Color::rgba(0x5c, 0x8c, 0xe6, 0x1c),
            Color::rgb(0xe8, 0xe8, 0xe8),
            Color::rgba(0x5c, 0x8c, 0xe6, 0xc0),
        ),
        focus: InteractionStyle::border(Color::rgb(0x5c, 0x8c, 0xe6)),
        active: InteractionStyle::background(Color::rgb(0x2c, 0x3a, 0x55)),
    },
    component_interactions: ComponentInteractions::NONE,
    text_selection: Color::rgb(0x2f, 0x4a, 0x75),
    disabled: Color::rgb(0x55, 0x55, 0x55),
    positive: Color::rgb(0x3f, 0xae, 0x5e),
    attention: Color::rgb(0xd9, 0x53, 0x4f),
    media: Color::rgb(0x44, 0x48, 0x4f),
    page: Color::rgb(0x1b, 0x1d, 0x22),
    shape: Shape::CLASSIC,
};

/// A warm light design with a green accent.
pub const FOREST: Theme = Theme {
    text: Color::rgb(0x1d, 0x2a, 0x22),
    text_muted: Color::rgb(0x6a, 0x7a, 0x70),
    surface: Color::rgb(0xfb, 0xfd, 0xf9),
    surface_muted: Color::rgb(0xee, 0xf3, 0xea),
    separator: Color::rgb(0xdd, 0xe5, 0xda),
    outline: Color::rgb(0xa8, 0xb5, 0xa8),
    accent: Color::rgb(0x2e, 0x7d, 0x4f),
    on_accent: Color::WHITE,
    selection: Color::rgb(0xd9, 0xec, 0xdf),
    interactions: InteractionStates {
        hover: InteractionStyle::all(
            Color::rgba(0x2e, 0x7d, 0x4f, 0x18),
            Color::rgb(0x1d, 0x2a, 0x22),
            Color::rgba(0x2e, 0x7d, 0x4f, 0xb8),
        ),
        focus: InteractionStyle::border(Color::rgb(0x2e, 0x7d, 0x4f)),
        active: InteractionStyle::background(Color::rgb(0xd9, 0xec, 0xdf)),
    },
    component_interactions: ComponentInteractions::NONE,
    text_selection: Color::rgb(0xbf, 0xe3, 0xcc),
    disabled: Color::rgb(0x74, 0x81, 0x79),
    positive: Color::rgb(0x1f, 0x6b, 0x40),
    attention: Color::rgb(0xc0, 0x56, 0x3a),
    media: Color::rgb(0xd3, 0xdc, 0xcf),
    page: Color::rgb(0x9a, 0xa8, 0x96),
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
        hover: InteractionStyle::all(
            Color::rgb(0xff, 0xe9, 0x4a),
            Color::rgb(0x14, 0x12, 0x0e),
            Color::rgb(0x14, 0x12, 0x0e),
        ),
        focus: InteractionStyle::border(Color::rgb(0x14, 0x12, 0x0e)),
        active: InteractionStyle::background(Color::rgb(0xff, 0xd5, 0x00)),
    },
    component_interactions: ComponentInteractions::NONE,
    text_selection: Color::rgb(0xff, 0xd5, 0x00),
    disabled: Color::rgb(0xb5, 0xae, 0x9c),
    positive: Color::rgb(0x0c, 0x83, 0x46),
    attention: Color::rgb(0xe6, 0x39, 0x46),
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
    text_muted: Color::rgb(0xa8, 0x8a, 0xa2),
    surface: Color::WHITE,
    surface_muted: Color::rgb(0xff, 0xee, 0xf7),
    separator: Color::rgb(0xf3, 0xd7, 0xe9),
    outline: Color::rgb(0xe8, 0xb4, 0xd8),
    accent: Color::rgb(0xff, 0x5c, 0xa8),
    on_accent: Color::WHITE,
    selection: Color::rgb(0xff, 0xd6, 0xea),
    interactions: InteractionStates {
        hover: InteractionStyle::all(
            Color::rgba(0xff, 0x5c, 0xa8, 0x20),
            Color::rgb(0x53, 0x2b, 0x4d),
            Color::rgba(0xff, 0x5c, 0xa8, 0xb8),
        ),
        focus: InteractionStyle::border(Color::rgb(0xff, 0x5c, 0xa8)),
        active: InteractionStyle::background(Color::rgb(0xff, 0xd6, 0xea)),
    },
    component_interactions: ComponentInteractions::NONE,
    text_selection: Color::rgb(0xff, 0xc2, 0xe0),
    disabled: Color::rgb(0xd8, 0xbf, 0xd0),
    positive: Color::rgb(0x2f, 0xbd, 0x8f),
    attention: Color::rgb(0xff, 0x7a, 0x59),
    media: Color::rgb(0xf6, 0xe3, 0xf0),
    page: Color::rgb(0xff, 0xe3, 0xf2),
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
    use schnellui_widgets::contrast_ratio;

    #[test]
    fn built_in_themes_are_distinct() {
        assert_ne!(LIGHT, DARK);
        assert_ne!(LIGHT, FOREST);
        assert_ne!(DARK, FOREST);
        assert_ne!(BRUTAL, CANDY);
        assert_eq!(LIGHT.accent, Color::rgb(0x2e, 0x63, 0xd4));
        assert_eq!(LIGHT.selection, Color::rgb(0xdb, 0xe6, 0xf9));
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
            for background in [
                theme.page,
                theme.surface,
                theme.surface_muted,
                theme.selection,
                theme.accent,
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
