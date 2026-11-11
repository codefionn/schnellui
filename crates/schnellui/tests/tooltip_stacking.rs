//! A tooltip with no room above its control flips below it. It must then paint
//! over the control below, even when that control carries a tooltip too and so
//! sits at the same overlay level, later in tree order.
//!
//! Skips when no GPU adapter is present (SOUL §7.2).

use schnellui::a11y::Role;
use schnellui::scene::{Color, Point, Primitive, Rect, WidgetId};
use schnellui::widgets::{Button, Column};
use schnellui::App;

fn pixel(app: &App, rgba: &[u8], x: f32, y: f32) -> [u8; 4] {
    let w = (app.size().width * app.scale()).round() as usize;
    let (x, y) = ((x * app.scale()) as usize, (y * app.scale()) as usize);
    let i = (y * w + x) * 4;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// The shown tooltip's background: the host's opaque rect outside its own box.
fn tooltip_background(app: &App, host: WidgetId) -> (Rect, Color) {
    let own = app.scene().layout(host).unwrap().rect;
    app.scene()
        .paint(host)
        .unwrap()
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::SolidRect { rect, color, .. }
                if color.a == 255 && rect.intersect(&own) != *rect =>
            {
                Some((*rect, *color))
            }
            _ => None,
        })
        .expect("the tooltip is shown")
}

#[test]
fn a_tooltip_flipped_below_paints_over_the_tooltip_control_below() {
    let mut app = App::mount_with_size(
        Column::new()
            .child(Button::new("Top").tooltip("The top button's tooltip"))
            .child(
                Button::new("Below")
                    .tooltip("The lower tooltip")
                    .width(300.0)
                    .height(60.0),
            ),
        320,
        200,
    );
    app.frame();
    let top = app.find_widget(Role::Button, Some("Top")).unwrap();
    let below = app.find_widget(Role::Button, Some("Below")).unwrap();
    let rect = app.scene().composited_rect(top);
    let at = Point {
        x: rect.x + rect.width / 2.0,
        y: rect.y + rect.height / 2.0,
    };
    app.update_pointer_proximity(Point {
        x: at.x + 1.0,
        ..at
    });
    app.frame();
    app.update_pointer_proximity(at);
    app.frame();

    let (tip, color) = tooltip_background(&app, top);
    let lower = app.scene().composited_rect(below);
    assert!(tip.y >= rect.bottom(), "{tip:?} flipped below {rect:?}");
    // A point in the tooltip's top padding, above its text, over the button below.
    let probe = Point {
        x: tip.x + tip.width / 2.0,
        y: tip.y + 1.5,
    };
    assert!(lower.contains(probe), "{probe:?} lies on {lower:?}");

    let Some(rgba) = app.render_rgba8() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let got = pixel(&app, &rgba, probe.x, probe.y);
    let near = |a: u8, b: u8| a.abs_diff(b) <= 2;
    assert!(
        near(got[0], color.r) && near(got[1], color.g) && near(got[2], color.b),
        "pixel {got:?} at {probe:?} is not the tooltip's {color:?}"
    );
}
