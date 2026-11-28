//! A list row's keyboard ring sits on its edge, where its neighbours sit flush,
//! unless the row asks for it outside (`ListRow::focus_outset`), where it
//! stands a gap off the row as a button's ring does. A row's meta dot stands
//! just before its meta text.

use super::*;
use schnellui::a11y::Role;
use schnellui::scene::{Color, Primitive, Rect, WidgetId};
use schnellui::widgets::{Column, ListRow, FOCUS_RING_GAP, FOCUS_RING_W};

fn mount(rows: Column) -> App {
    let mut app = App::mount_with_size(rows, 400, 300);
    app.frame();
    app
}

fn row(app: &App, name: &str) -> WidgetId {
    app.find_widget(Role::Button, Some(name)).unwrap()
}

/// The focus ring stroke painted on `id`.
fn ring(app: &App, id: WidgetId) -> Rect {
    app.scene()
        .paint(id)
        .and_then(|paint| {
            paint
                .primitives
                .iter()
                .find_map(|primitive| match *primitive {
                    Primitive::StrokeRect { rect, width, .. } if width == FOCUS_RING_W => {
                        Some(rect)
                    }
                    _ => None,
                })
        })
        .expect("a focus ring")
}

#[test]
fn a_row_rings_focus_on_its_edge_by_default() {
    let mut app = mount(
        Column::new()
            .gap(8.0)
            .child(ListRow::new("general").width(200.0).on_click(|| {}))
            .child(ListRow::new("help").width(200.0).on_click(|| {})),
    );
    let help = row(&app, "help");
    assert!(app.focus(Some(help)));
    app.frame();
    let edge = app.scene().layout(help).unwrap().rect;
    let ring = ring(&app, help);
    assert!((ring.x - edge.x).abs() < 0.5 && (ring.width - edge.width).abs() < 0.5);
}

#[test]
fn an_outset_row_rings_focus_a_gap_outside_its_edge() {
    let mut app = mount(
        Column::new()
            .gap(8.0)
            .child(ListRow::new("general").width(200.0).on_click(|| {}))
            .child(
                ListRow::new("help")
                    .width(200.0)
                    .selected(true)
                    .focus_outset(true)
                    .on_click(|| {}),
            )
            // A row under it, so the ring has room below inside the column.
            .child(ListRow::new("random").width(200.0).on_click(|| {})),
    );
    let help = row(&app, "help");
    assert!(app.focus(Some(help)));
    app.frame();
    let edge = app.scene().layout(help).unwrap().rect;
    let ring = ring(&app, help);
    let reach = FOCUS_RING_W + FOCUS_RING_GAP;
    assert!(
        (ring.y - (edge.y - reach)).abs() < 0.5,
        "{ring:?} around {edge:?}"
    );
    assert!(
        (ring.height - (edge.height + 2.0 * reach)).abs() < 0.5,
        "{ring:?} around {edge:?}"
    );
}

#[test]
fn a_meta_dot_stands_just_before_the_meta_text() {
    let red = Color::rgb(0xC0, 0x10, 0x10);
    let app = mount(
        Column::new().child(
            ListRow::new("general")
                .width(240.0)
                .meta("Mon")
                .meta_dot(red)
                .on_click(|| {}),
        ),
    );
    let general = row(&app, "general");
    let edge = app.scene().layout(general).unwrap().rect;
    let paint = app.scene().paint(general).unwrap();
    let dot = paint
        .primitives
        .iter()
        .find_map(|primitive| match *primitive {
            Primitive::SolidRect { rect, color, .. } if color == red => Some(rect),
            _ => None,
        })
        .expect("the meta dot");
    // In the right half of the row, round, and inside the row's height.
    assert!(dot.x > edge.x + edge.width * 0.5);
    assert!((dot.width - dot.height).abs() < 0.01);
    assert!(dot.y > edge.y && dot.y + dot.height < edge.y + edge.height);
}
