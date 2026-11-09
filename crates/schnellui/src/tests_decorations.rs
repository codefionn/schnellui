//! Hover, press and focus decorations replayed through the app in the orders
//! people produce them. Each test ends with the pointer gone, the button up
//! and focus elsewhere, and then the widget must paint what it painted before.

use super::*;
use schnellui::a11y::Role;
use schnellui::scene::{Color, Point, Primitive};
use schnellui::widgets::{
    ComponentInteractions, InteractionStates, InteractionStyle, Theme, View, DEFAULT_THEME,
};

/// Every channel in every state, so washes, recolours and strokes stack.
fn layered_theme() -> Theme {
    let style = |r: u8| InteractionStyle {
        background: Some(Color::rgba(r, 40, 90, 0x50)),
        foreground: Some(Color::rgb(r, 10, 200)),
        border: Some(Color::rgb(r, 120, 30)),
    };
    let states = |r: u8| InteractionStates {
        hover: style(r),
        focus: style(r + 20),
        active: style(r + 40),
    };
    Theme {
        interactions: states(10),
        component_interactions: ComponentInteractions {
            button: Some(states(70)),
            navigation: Some(states(130)),
            toggle: None,
            editable: Some(states(190)),
            raw_surface: None,
        },
        ..DEFAULT_THEME
    }
}

fn mount(view: impl View + 'static) -> App {
    let mut app = App::mount_with_theme_size_scaled(layered_theme(), view, 400, 200, 1.0);
    app.apply_reduced_motion(true);
    app.frame();
    app
}

fn centre(app: &App, id: scene::WidgetId) -> Point {
    let r = app.scene().layout(id).unwrap().rect;
    Point {
        x: r.x + r.width / 2.0,
        y: r.y + r.height / 2.0,
    }
}

const AWAY: Point = Point {
    x: f32::NEG_INFINITY,
    y: f32::NEG_INFINITY,
};

fn prims(app: &App, id: scene::WidgetId) -> Vec<Primitive> {
    app.scene().paint(id).unwrap().primitives.clone()
}

/// Hover a row, press it, then press a navigation key while the button is
/// still down. The key takes hover away from under the press. The press had
/// recoloured the label on top of hover, so it recorded hover's colour as the
/// one to put back, and releasing left the label in hover's colour.
#[test]
fn a_key_under_a_held_press_leaves_no_hover_ink() {
    use schnellui::widgets::{Button, ListItem};
    for (view, role) in [
        (
            Box::new(ListItem::new("Inbox")) as Box<dyn View>,
            Role::ListItem,
        ),
        (Box::new(Button::new("Save")), Role::Button),
    ] {
        let mut app = mount(view);
        let id = app.find_widget(role, None).unwrap();
        let base = prims(&app, id);
        app.update_pointer_proximity(centre(&app, id));
        app.set_active_interaction(Some(id));
        app.hold_pointer_hover();
        app.set_active_interaction(None);
        app.update_pointer_proximity(AWAY);
        app.frame();
        assert_eq!(
            prims(&app, id),
            base,
            "{role:?} after hover, press, key, release"
        );
    }
}

/// Hover a field, press into it and type while the button is down, then let
/// go and leave. Typing clears and refills the field's paint. The hover,
/// press and focus records still pointed into the old paint, so taking them
/// off wrote the old label's colour over the new value's glyphs.
#[test]
fn typing_into_a_pressed_hovered_field_restores_its_own_colours() {
    use schnellui::widgets::{TextArea, TextInput};
    for (view, role) in [
        (
            Box::new(TextInput::new("Ada").label("Owner")) as Box<dyn View>,
            Role::TextInput,
        ),
        (
            Box::new(TextArea::new("one\ntwo").rows(3)),
            Role::MultilineTextInput,
        ),
    ] {
        let mut app = mount(view);
        let id = app.find_widget(role, None).unwrap();
        let p = centre(&app, id);
        app.update_pointer_proximity(p);
        app.set_active_interaction(Some(id));
        app.focus(Some(id));
        app.dispatch_key(UiKey::End { shift: false });
        app.dispatch_key(UiKey::Char("x"));
        app.frame();
        app.dispatch_key(UiKey::Backspace);
        app.frame();
        app.set_active_interaction(None);
        app.update_pointer_proximity(AWAY);
        app.focus(None);
        app.frame();

        let mut fresh = mount(match role {
            Role::TextInput => Box::new(TextInput::new("Ada").label("Owner")) as Box<dyn View>,
            _ => Box::new(TextArea::new("one\ntwo").rows(3)),
        });
        let fresh_id = fresh.find_widget(role, None).unwrap();
        fresh.focus(Some(fresh_id));
        fresh.dispatch_key(UiKey::End { shift: false });
        fresh.focus(None);
        fresh.frame();
        assert_eq!(
            prims(&app, id),
            prims(&fresh, fresh_id),
            "{role:?} after hover, press, type, release, leave, blur"
        );
    }
}
