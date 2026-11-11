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
        app.dispatch_key(UiKey::End {
            shift: false,
            ctrl: false,
        });
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
        fresh.dispatch_key(UiKey::End {
            shift: false,
            ctrl: false,
        });
        fresh.focus(None);
        fresh.frame();
        assert_eq!(
            prims(&app, id),
            prims(&fresh, fresh_id),
            "{role:?} after hover, press, type, release, leave, blur"
        );
    }
}

/// The focus ring of a stretched row, as a rect, and the row's laid-out box.
fn ring_and_box(app: &App, id: scene::WidgetId) -> (scene::Rect, scene::Rect) {
    let ring = prims(app, id)
        .into_iter()
        .rev()
        .find_map(|p| match p {
            Primitive::StrokeRect { rect, width, .. }
                if width == schnellui::widgets::FOCUS_RING_W =>
            {
                Some(rect)
            }
            _ => None,
        })
        .expect("the focused row wears a ring");
    (ring, app.scene().layout(id).unwrap().rect)
}

fn stretched_rows() -> impl View {
    use schnellui::layout::Align;
    use schnellui::widgets::{Column, List, ListItem};
    Column::new().fill().align(Align::Stretch).child(
        List::new()
            .child(ListItem::new("Inbox"))
            .child(ListItem::new("Archive")),
    )
}

/// Focus a row that stretches with the window, then resize the window. The
/// ring was sized from the row's paint when focus arrived and only slid
/// afterwards, so it kept the old width: wider than the row after a shrink,
/// a stub after a grow.
#[test]
fn a_focus_ring_follows_its_row_through_a_resize() {
    let mut app = App::mount_with_size(stretched_rows(), 400, 200);
    app.frame();
    let row = app.find_widget(Role::ListItem, Some("Archive")).unwrap();
    app.focus(Some(row));
    app.frame();
    let (ring, row_box) = ring_and_box(&app, row);
    assert_eq!(ring.width, row_box.width);
    for width in [260.0, 520.0] {
        app.resize(width, 200.0);
        app.frame();
        let (ring, row_box) = ring_and_box(&app, row);
        assert_eq!(
            (ring.x, ring.width),
            (row_box.x, row_box.width),
            "ring follows the row at window width {width}"
        );
    }
}

/// A remount restores focus on the replacement tree before its first layout.
/// The ring was sized from the row's unstretched paint and stayed that size
/// once layout stretched the row.
#[test]
fn a_focus_ring_restored_by_a_remount_fits_the_laid_out_row() {
    let mut old = App::mount_with_size(stretched_rows(), 400, 200);
    old.frame();
    let row = old.find_widget(Role::ListItem, Some("Archive")).unwrap();
    old.focus(Some(row));
    old.frame();
    let mut new = App::mount_with_size(stretched_rows(), 400, 200);
    new.inherit_remount_state(&old);
    new.frame();
    let row = new.find_widget(Role::ListItem, Some("Archive")).unwrap();
    let (ring, row_box) = ring_and_box(&new, row);
    assert_eq!((ring.x, ring.width), (row_box.x, row_box.width));
}

/// Hover a button that carries a tooltip, then narrow the window so the
/// button keeps its place but the window edge now cuts through the tooltip.
/// Tooltips were only placed again when their own button moved, so the
/// tooltip kept hanging past the new right edge.
#[test]
fn a_shown_tooltip_stays_inside_a_window_narrowed_under_it() {
    use schnellui::widgets::{Button, Pad, Row, Stack, Text};
    let mut app = mount(
        Stack::new().fill().child(
            Pad::all(40.0).child(
                Row::new()
                    .gap(8.0)
                    .child(Text::new("Spacer text"))
                    .child(Button::new("Archive overview").tooltip("Archive this overview")),
            ),
        ),
    );
    let id = app.find_widget(Role::Button, None).unwrap();
    let before = app.scene().layout(id).unwrap().rect;
    app.update_pointer_proximity(centre(&app, id));
    app.frame();
    app.resize(before.right() - 10.0, 200.0);
    app.frame();
    assert_eq!(
        app.scene().layout(id).unwrap().rect,
        before,
        "the button stays put"
    );
    let width = app.size().width;
    let tooltip = app
        .scene()
        .paint(id)
        .unwrap()
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::SolidRect { rect, color, .. }
                if rect.bottom() <= before.y && color.a > 0 =>
            {
                Some(*rect)
            }
            _ => None,
        })
        .expect("the tooltip shows above the button");
    assert!(
        tooltip.x >= 0.0 && tooltip.right() <= width,
        "tooltip {tooltip:?} leaves a window {width} wide"
    );
}

/// Drag a button over a drop target while the pointer hovers the target, in
/// either order, then move away. Hover strokes are found by counting from the
/// end of the paint, and the drop preview's lines were appended after them,
/// so taking either off removed the other's primitives and left a stray line.
#[test]
fn hover_and_a_drop_preview_come_off_in_any_order() {
    use schnellui::widgets::{Button, Row};
    for hover_first in [true, false] {
        let mut app = mount(
            Row::new()
                .gap(40.0)
                .child(Button::new("source").on_drag_start(|| {}))
                .child(Button::new("target").on_drop(|| {})),
        );
        let source = app.find_widget(Role::Button, Some("source")).unwrap();
        let target = app.find_widget(Role::Button, Some("target")).unwrap();
        let base = prims(&app, target);
        let over = centre(&app, target);
        assert!(app.begin_drag(centre(&app, source)));
        app.update_drag(Point { x: 2.0, y: 150.0 });
        if hover_first {
            app.update_pointer_proximity(over);
            app.update_drag(over);
        } else {
            app.update_drag(over);
            app.update_pointer_proximity(over);
        }
        app.frame();
        app.update_pointer_proximity(AWAY);
        app.update_drag(Point { x: 2.0, y: 150.0 });
        app.end_drag(Point { x: 2.0, y: 150.0 });
        app.frame();
        assert_eq!(
            prims(&app, target),
            base,
            "hover first: {hover_first}, the target keeps preview or hover ink"
        );
    }
}

/// Hover a scroll view of rich text, then resize the window a step at a
/// time. The hover border outlines the viewport and must follow it, even
/// when the text inside does not rewrap or already overflows the viewport.
#[test]
fn a_hovered_scroll_view_border_follows_a_resized_window() {
    use schnellui::widgets::{Button, Column, Pad, RichDoc, RichSpan, RichText, Scroll};
    let view = || {
        let doc = RichDoc::new()
            .heading(2, ["Release notes"])
            .paragraph([
                RichSpan::plain("This build has "),
                RichSpan::bold("bold"),
                RichSpan::plain(" words, inline "),
                RichSpan::code("code"),
                RichSpan::plain(" and a "),
                RichSpan::link("link"),
                RichSpan::plain(" in a paragraph long enough to wrap in a narrow window."),
            ])
            .bullet([RichSpan::plain("first point")])
            .bullet([RichSpan::plain(
                "second point, which also runs long enough to wrap",
            )])
            .quote([RichSpan::plain("A quoted line.")])
            .code_block("rust", [vec![RichSpan::code("fn main() {}")]])
            // Enough lines that the text overflows the viewport.
            .bullet([RichSpan::plain("third point")])
            .bullet([RichSpan::plain("fourth point")])
            .bullet([RichSpan::plain("fifth point")])
            .bullet([RichSpan::plain("sixth point")]);
        Column::new()
            .fill()
            .child(
                Scroll::new()
                    .fill()
                    .child(Pad::all(12.0).child(RichText::new(doc))),
            )
            .child(Pad::all(8.0).child(Button::new("Done")))
    };
    // Scroll views take the raw-surface style; give it every channel.
    let mut theme = layered_theme();
    let surface = InteractionStyle {
        background: Some(Color::rgba(200, 40, 90, 0x50)),
        foreground: Some(Color::rgb(200, 10, 200)),
        border: Some(Color::rgb(200, 120, 30)),
    };
    theme.component_interactions.raw_surface = Some(InteractionStates {
        hover: surface,
        focus: surface,
        active: surface,
    });
    let mut app = App::mount_with_theme_size_scaled(theme, view(), 420, 300, 1.0);
    app.apply_reduced_motion(true);
    app.frame();
    let scroll = app.find_widget(Role::ScrollView, None).unwrap();
    app.update_pointer_proximity(centre(&app, scroll));
    app.frame();
    let border = |app: &App| -> Vec<scene::Rect> {
        app.scene()
            .preorder()
            .filter_map(|id| app.scene().paint(id))
            .flat_map(|paint| paint.primitives.iter().copied())
            .filter_map(|p| match p {
                Primitive::StrokeRect { rect, color, .. } if color == Color::rgb(200, 120, 30) => {
                    Some(rect)
                }
                _ => None,
            })
            .collect()
    };
    for (w, h) in [
        (420.0, 301.0),
        (418.0, 298.0),
        (418.0, 289.0),
        (417.0, 279.0),
        (417.0, 290.0),
    ] {
        app.resize(w, h);
        app.frame();
        let shown = border(&app);
        app.update_pointer_proximity(AWAY);
        app.frame();
        app.update_pointer_proximity(centre(&app, scroll));
        app.frame();
        assert_eq!(
            shown,
            border(&app),
            "at {w}x{h} the border is sized as a fresh hover"
        );
    }
}

/// A viewport owns its hover and focus decoration, and its scrollbar chrome
/// shares that paint. Scrolling rewrote the chrome over the hover wash and
/// left the record pointing at a primitive that was no longer a wash.
#[test]
fn scrolling_a_hovered_viewport_keeps_its_hover_wash() {
    use schnellui::widgets::{Column, Scroll, Text};
    for scrollbar in [false, true] {
        let states = |r: u8| {
            let style = |r: u8| InteractionStyle {
                background: Some(Color::rgba(r, 40, 90, 0x50)),
                foreground: None,
                border: Some(Color::rgb(r, 120, 30)),
            };
            InteractionStates {
                hover: style(r),
                focus: style(r + 20),
                active: style(r + 40),
            }
        };
        let mut theme = layered_theme();
        theme.component_interactions.raw_surface = Some(states(200));
        let mut col = Column::new();
        for i in 0..30 {
            col = col.child(Text::new(format!("Row {i}")));
        }
        let mut app = App::mount_with_theme_size_scaled(
            theme,
            Scroll::new()
                .scrollbar(scrollbar)
                .size(200.0, 80.0)
                .child(col),
            400,
            200,
            1.0,
        );
        app.apply_reduced_motion(true);
        app.frame();
        let sv = app.find_widget(Role::ScrollView, None).unwrap();
        app.update_pointer_proximity(centre(&app, sv));
        for _ in 0..2 {
            assert!(app.dispatch_scroll(sv, 20.0));
            app.frame();
            assert_eq!(
                schnellui::widgets::probes::decoration_faults(&app.widgets, &app.scene),
                None,
                "scrollbar {scrollbar}"
            );
        }
    }
}

/// A resize that takes a scrollbar viewport from fitting to overflowing and back
/// narrows and widens its rows, and the hover and focus decorations on a row go
/// with it instead of staying at the width it had.
#[test]
fn a_resize_across_the_scrollbar_gutter_moves_decorations_with_the_content() {
    use schnellui::widgets::{Align, Button, Column, Scroll};
    let mut col = Column::new().align(Align::Stretch);
    for i in 0..10 {
        col = col.child(Button::new(format!("Row {i}")));
    }
    let mut app = App::mount_with_theme_size_scaled(
        layered_theme(),
        Scroll::new().fill().fit_width().scrollbar(true).child(col),
        300,
        900,
        1.0,
    );
    app.apply_reduced_motion(true);
    app.frame();
    let row = app.find_widget(Role::Button, Some("Row 2")).unwrap();
    app.focus(Some(row));
    app.update_pointer_proximity(centre(&app, row));
    app.frame();
    let width = |app: &App| app.scene().layout(row).unwrap().rect.width;
    assert_eq!(width(&app), 300.0);
    for _ in 0..2 {
        for (height, want) in [(150.0, 288.0), (900.0, 300.0)] {
            app.resize(300.0, height);
            app.frame();
            assert_eq!(width(&app), want, "at {height} tall");
            assert_eq!(
                schnellui::widgets::probes::decoration_faults(&app.widgets, &app.scene),
                None,
                "at {height} tall"
            );
            let scene = app.scene();
            let ring = scene
                .paint(row)
                .map(|paint| {
                    paint
                        .primitives
                        .iter()
                        .filter_map(|p| match p {
                            Primitive::SolidRect { rect, .. } => Some(rect.right()),
                            _ => None,
                        })
                        .fold(0.0_f32, f32::max)
                })
                .unwrap_or(0.0);
            assert!(
                ring <= want + 0.5,
                "paint on the row reaches {ring}, past its {want} wide box at {height} tall"
            );
        }
    }
}

/// Content scrolled wholly below the fold is not cut at a side, however wide it
/// is: only what some of the window shows can be clipped sideways.
#[cfg(feature = "fuzzing")]
#[test]
fn sideways_clips_ignores_content_below_the_fold() {
    use schnellui::widgets::{Column, Scroll, Text, WrapMode};
    let mut col = Column::new();
    for _ in 0..30 {
        col = col.child(Text::new("row"));
    }
    col =
        col.child(Text::new("a line far wider than the window ".repeat(8)).wrap(WrapMode::NoWrap));
    let mut app = App::mount_with_size(Scroll::new().fill().child(col), 200, 100);
    assert_eq!(
        schnellui::testing::fuzz::sideways_clips(&mut app),
        Vec::<String>::new()
    );
}
