//! A reused App renderer must show every change made between renders, even though
//! `App::frame` clears the scene's dirty state before a headless render.
//!
//! Each test skips gracefully when no GPU adapter is present (SOUL §7.2).

use schnellui::a11y::Role;
use schnellui::scene::Color;
use schnellui::signal::create_signal;
use schnellui::widgets::{Button, Column, Text};
use schnellui::App;

fn pixel(app: &App, rgba: &[u8], x: f32, y: f32) -> [u8; 4] {
    let w = (app.size().width * app.scale()).round() as usize;
    let (x, y) = ((x * app.scale()) as usize, (y * app.scale()) as usize);
    let i = (y * w + x) * 4;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn paint_only_change_shows_in_second_render() {
    let mut app = App::mount_with_size(Column::new().child(Button::new("Bound paint")), 200, 120);
    let node = app.find_widget(Role::Button, Some("Bound paint")).unwrap();
    let color = create_signal(Color::rgb(200, 30, 30));
    app.bind_paint(node, move || color.get());
    app.frame();
    let Some(first) = app.render_rgba8() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let rect = app.scene().layout(node).unwrap().rect;
    let (x, y) = (rect.x + 3.0, rect.y + rect.height / 2.0);
    assert_eq!(pixel(&app, &first, x, y)[..3], [200, 30, 30]);

    color.set(Color::rgb(20, 160, 40));
    app.frame();
    let second = app.render_rgba8().unwrap();
    assert_eq!(pixel(&app, &second, x, y)[..3], [20, 160, 40]);
}

#[test]
fn glyphs_rasterized_after_first_render_appear() {
    let text = create_signal(".".to_string());
    let mut app = App::mount_with_size(
        Column::new().child(Text::dynamic(move || text.get())),
        300,
        120,
    );
    app.frame();
    let Some(first) = app.render_rgba8() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let background = pixel(&app, &first, 290.0, 110.0);

    text.set("WWWW MMMM".to_string());
    app.frame();
    let second = app.render_rgba8().unwrap();
    let w = (app.size().width * app.scale()) as usize;
    let inked = second
        .chunks_exact(4)
        .enumerate()
        .filter(|(i, px)| {
            let (x, y) = (i % w, i / w);
            x < 150 && y < 60 && px[..3] != background[..3]
        })
        .count();
    assert!(
        inked > 100,
        "new glyphs must draw, got {inked} inked pixels"
    );
}
