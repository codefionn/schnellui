use schnellui::scene::Color;
use schnellui::{signal::create_signal, App};
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn closure_attributes_update_retained_nodes_and_skip_clean_frames() {
    let width = create_signal(80.0f32);
    let color = create_signal(Color::BLACK);
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    let mut app = App::mount_with_size(
        schnellui::view! {
            column(gap = 3.0) {
                button(width = move || { seen.set(seen.get() + 1); width.get() }, color = move || color.get()) { "Retained" }
                text { "Sibling" }
            }
        },
        300,
        150,
    );
    let root = app.scene().root().unwrap();
    let nodes = app.scene().node(root).unwrap().children.clone();
    app.frame();
    assert_eq!(calls.get(), 1);
    for _ in 0..3 {
        app.frame();
    }
    assert_eq!(calls.get(), 1);
    width.set(120.0);
    color.set(Color::WHITE);
    app.frame();
    assert_eq!(calls.get(), 2);
    assert_eq!(app.scene().node(root).unwrap().children, nodes);
    assert_eq!(app.scene().layout(nodes[0]).unwrap().rect.width, 120.0);
    let primitives = &app.scene().paint(nodes[0]).unwrap().primitives;
    assert!(primitives.iter().any(|p| matches!(p, schnellui::scene::Primitive::SolidRect { color, .. } if *color == Color::WHITE)));
}

#[test]
fn checked_disabled_and_container_gap_are_tracked_independently() {
    let checked = create_signal(false);
    let disabled = create_signal(false);
    let gap = create_signal(4.0f32);
    let clicks = Rc::new(Cell::new(0));
    let clicked = clicks.clone();
    let mut app = App::mount_with_size(
        schnellui::view! {
            column(gap = move || gap.get()) {
                checkbox(checked = move || checked.get(), name = "Accept")
                button(disabled = move || disabled.get(), on:click = move || clicked.set(clicked.get() + 1)) { "Continue" }
            }
        },
        300,
        150,
    );
    app.frame();
    let root = app.scene().root().unwrap();
    let nodes = app.scene().node(root).unwrap().children.clone();
    checked.set(true);
    disabled.set(true);
    gap.set(20.0);
    app.frame();
    let checked_bit = schnellui::a11y::StateFlags::CHECKED.0;
    let disabled_bit = schnellui::a11y::StateFlags::DISABLED.0;
    assert_ne!(app.scene().a11y(nodes[0]).unwrap().state & checked_bit, 0);
    assert_ne!(app.scene().a11y(nodes[1]).unwrap().state & disabled_bit, 0);
    assert!(!app.dispatch_click(nodes[1]));
    assert_eq!(clicks.get(), 0);
    let first = app.scene().layout(nodes[0]).unwrap().rect;
    let second = app.scene().layout(nodes[1]).unwrap().rect;
    assert_eq!(second.y - first.y - first.height, 20.0);
    disabled.set(false);
    app.frame();
    assert!(app.dispatch_click(nodes[1]));
    assert_eq!(clicks.get(), 1);
}

#[test]
fn removing_a_bound_subtree_retires_its_producer() {
    use schnellui::widgets::{ComponentRef, Text};
    let signal = create_signal(50.0f32);
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    let reference = ComponentRef::new();
    let mut app = App::mount_with_size(
        schnellui::view! {
            column {
                component_ref(value = reference) {
                    button(width = move || { seen.set(seen.get()+1); signal.get() }) { "Old" }
                }
            }
        },
        300,
        100,
    );
    app.frame();
    assert_eq!(calls.get(), 1);
    app.replace_subtree(reference, Text::new("New")).unwrap();
    signal.set(75.0);
    app.frame();
    assert_eq!(calls.get(), 1);
}

#[test]
fn cached_color_survives_dynamic_text_and_resize_without_rerunning() {
    let text = create_signal(1u32);
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    let color = Color::rgb(120, 30, 60);
    let mut app = App::mount_with_size(
        schnellui::view! {
            text(color = move || { seen.set(seen.get()+1); color }) { (text.get()) }
        },
        300,
        100,
    );
    app.frame();
    let root = app.scene().root().unwrap();
    text.set(222);
    app.frame();
    app.resize(150.0, 100.0);
    app.frame();
    assert_eq!(calls.get(), 1);
    assert!(app.scene().paint(root).unwrap().primitives.iter().all(|p| {
        !matches!(p, schnellui::scene::Primitive::GlyphQuad { color: actual, .. } if *actual != color)
    }));
}

#[test]
fn closure_padding_changes_only_geometry() {
    let padding = create_signal(8.0f32);
    let mut app = App::mount_with_size(
        schnellui::view! {
            pad(all = move || padding.get()) { text { "Inside" } }
        },
        300,
        100,
    );
    app.frame();
    let root = app.scene().root().unwrap();
    let child = app.scene().node(root).unwrap().children[0];
    padding.set(20.0);
    app.frame();
    assert_eq!(app.scene().node(root).unwrap().children[0], child);
    let outer = app.scene().layout(root).unwrap().rect;
    let inner = app.scene().layout(child).unwrap().rect;
    assert_eq!(inner.x - outer.x, 20.0);
}

#[test]
fn font_size_updates_measure_and_survives_later_dynamic_text() {
    let size = create_signal(12.0f32);
    let value = create_signal(11u32);
    let mut app = App::mount_with_size(
        schnellui::view! {
            column {
                text(size = move || size.get()) { "Font" }
                text(font_size = move || size.get()) { (value.get()) }
            }
        },
        300,
        150,
    );
    app.frame();
    let root = app.scene().root().unwrap();
    let children = app.scene().node(root).unwrap().children.clone();
    let before = app.scene().layout(children[0]).unwrap().rect;
    size.set(24.0);
    app.frame();
    let after = app.scene().layout(children[0]).unwrap().rect;
    assert!(after.width > before.width * 1.5);
    assert!(after.height > before.height * 1.5);
    let dynamic_height = app.scene().layout(children[1]).unwrap().rect.height;
    value.set(22);
    app.frame();
    assert_eq!(
        app.scene().layout(children[1]).unwrap().rect.height,
        dynamic_height
    );
    assert_eq!(app.scene().node(root).unwrap().children, children);
}

#[cfg(feature = "count-allocations")]
#[test]
fn retained_color_attribute_allocates_nothing_after_warmup() {
    let color = create_signal(Color::BLACK);
    let mut app = App::mount_with_size(
        schnellui::view! {
            text(color = move || color.get()) { "Color" }
        },
        200,
        100,
    );
    app.frame();
    color.set(Color::WHITE);
    app.frame();
    color.set(Color::BLACK);
    app.frame();
    let allocations = allocation_counter::measure(|| {
        color.set(Color::WHITE);
        app.frame();
    });
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.bytes_total, 0);
}
