//! An open dropdown opens below its trigger when the list fits there, and
//! above it when it only fits above, judged against what the window shows:
//! a page scrolled down moves the trigger up on screen.

use super::*;
use schnellui::a11y::Role;
use schnellui::scene::Rect;
use schnellui::widgets::{Column, Dropdown, DropdownOption, Flex, Scroll, View};

const NAMES: [&str; 6] = ["One", "Two", "Three", "Four", "Five", "Six"];

fn dropdown() -> Dropdown {
    let mut dropdown = Dropdown::new("Pick").open(true);
    for name in NAMES {
        dropdown = dropdown.option(DropdownOption::new(name));
    }
    dropdown
}

fn block(height: f32) -> Flex {
    Flex::new().min_height(height).min_width(10.0)
}

/// The trigger's and the open list's on-screen rects.
fn placement(root: impl View + 'static) -> (Rect, Rect) {
    placement_in(root, 400)
}

fn placement_in(root: impl View + 'static, height: u32) -> (Rect, Rect) {
    let mut app = App::mount_with_size(root, 640, height);
    app.frame();
    let trigger = app.find_widget(Role::ComboBox, Some("Pick")).unwrap();
    let scene = app.scene();
    let popup = scene
        .overlays()
        .find(|&o| scene.overlay_level(o) == crate::widgets::POPUP_OVERLAY_LEVEL)
        .expect("an open list");
    (scene.composited_rect(trigger), scene.composited_rect(popup))
}

#[test]
fn a_dropdown_with_room_below_opens_below() {
    let (trigger, popup) = placement(Column::new().child(block(20.0)).child(dropdown()));
    assert!(
        popup.y >= trigger.y + trigger.height,
        "{trigger:?} {popup:?}"
    );
}

#[test]
fn a_dropdown_near_the_window_bottom_opens_above() {
    let (trigger, popup) = placement(Column::new().child(block(330.0)).child(dropdown()));
    assert!(popup.y + popup.height <= trigger.y, "{trigger:?} {popup:?}");
}

#[test]
fn a_dropdown_scrolled_near_the_window_bottom_opens_above() {
    let page = Scroll::new().fill().initial_offset(600.0).child(
        Column::new()
            .child(block(930.0))
            .child(dropdown())
            .child(block(1000.0)),
    );
    let (trigger, popup) = placement(page);
    assert!(trigger.y > 250.0 && trigger.y < 400.0, "{trigger:?}");
    assert!(popup.y + popup.height <= trigger.y, "{trigger:?} {popup:?}");
}

#[test]
fn a_dropdown_scrolled_near_the_window_top_opens_below() {
    let page = Scroll::new().fill().initial_offset(600.0).child(
        Column::new()
            .child(block(620.0))
            .child(dropdown())
            .child(block(1000.0)),
    );
    let (trigger, popup) = placement(page);
    assert!(trigger.y < 100.0, "{trigger:?}");
    assert!(
        popup.y >= trigger.y + trigger.height,
        "{trigger:?} {popup:?}"
    );
}

#[test]
fn a_dropdown_at_a_scroll_viewports_lower_edge_opens_above_inside_it() {
    // The viewport cuts off a list that opens below; the window does not.
    let page = Scroll::new().size(300.0, 300.0).child(
        Column::new()
            .child(block(220.0))
            .child(dropdown())
            .child(block(1000.0)),
    );
    let (trigger, popup) = placement_in(page, 800);
    assert!(popup.y + popup.height <= trigger.y, "{trigger:?} {popup:?}");
}

#[test]
fn a_combo_box_near_the_window_bottom_opens_above() {
    use schnellui::widgets::ComboBox;
    let mut combo = ComboBox::new("").label("Pick").open(true);
    for name in NAMES {
        combo = combo.option(DropdownOption::new(name));
    }
    let (trigger, popup) = placement(Column::new().child(block(330.0)).child(combo));
    assert!(
        popup.y + popup.height <= trigger.y + 0.5,
        "{trigger:?} {popup:?}"
    );
}
