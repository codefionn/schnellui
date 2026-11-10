//! A focused control that becomes disabled stops wearing the focus ring, and
//! focus moves to the nearest enabled neighbour in tab order, as it does when
//! the control is removed. Every test checks the painted strokes as well as
//! `focused()`, so the pixels cannot drift from the accessibility tree.

use super::*;
use schnellui::a11y::{Role, StateFlags};
use schnellui::scene::{Primitive, WidgetId};
use schnellui::signal::create_signal;
use schnellui::widgets::{
    Attribute, Button, Checkbox, Column, Dialog, Dropdown, Radio, ReactiveAttr, Slider, Switch,
    Tab, TabBar, TextInput, View, FOCUS_RING_W,
};

/// Focus ring strokes painted anywhere in the scene, per node.
fn ringed(app: &App) -> Vec<WidgetId> {
    app.scene()
        .preorder()
        .filter(|id| {
            app.scene().paint(*id).is_some_and(|paint| {
                paint.primitives.iter().any(|primitive| {
                    matches!(primitive, Primitive::StrokeRect { width, .. } if *width == FOCUS_RING_W)
                })
            })
        })
        .collect()
}

fn name(app: &App, id: WidgetId) -> Option<String> {
    app.scene().a11y(id)?.name.clone()
}

fn focused_name(app: &App) -> Option<String> {
    name(app, app.focused_widget()?)
}

fn button(app: &App, label: &str) -> WidgetId {
    app.find_widget(Role::Button, Some(label)).unwrap()
}

fn is_disabled(app: &App, id: WidgetId) -> bool {
    StateFlags(app.scene().a11y(id).unwrap().state).contains(StateFlags::DISABLED)
}

/// The pixels and the tree agree: only `expected` wears a ring, and it is
/// the focused control.
fn assert_ring_matches_focus(app: &App, expected: Option<WidgetId>) {
    assert_eq!(app.focused_widget(), expected, "focused()");
    let rings = ringed(app);
    match expected {
        Some(id) => assert_eq!(rings, [id], "the ring is on the focused control only"),
        None => assert!(rings.is_empty(), "no ring without focus, found {rings:?}"),
    }
}

fn page(middle: impl View) -> Column {
    Column::new()
        .child(Button::new("Before"))
        .child(middle)
        .child(Button::new("After"))
}

fn mount(view: impl View) -> App {
    let mut app = App::mount_with_size(view, 500, 400);
    app.frame();
    app
}

fn remount(previous: &App, view: impl View, disable: Option<WidgetId>) -> App {
    let mut app = App::mount_with_size(view, 500, 400);
    if let Some(id) = disable {
        let state = app.scene().a11y(id).unwrap().state | StateFlags::DISABLED.0;
        app.scene_mut().set_a11y_state(id, state);
    }
    app.inherit_remount_state(previous);
    app.frame();
    app
}

/// The control between the two buttons.
fn middle(app: &App) -> WidgetId {
    schnellui::a11y::tab_order(app.scene())[1]
}

/// The exact sequence from the dialogs example: keyboard focus on a button,
/// the app rebuilds it disabled.
#[test]
fn a_remount_that_disables_the_focused_button_moves_the_ring_to_its_neighbour() {
    let view = |disabled| page(Button::new("Open").disabled(disabled));
    let mut previous = mount(view(false));
    let open = button(&previous, "Open");
    assert!(previous.focus(Some(open)));
    assert_ring_matches_focus(&previous, Some(open));

    let next = remount(&previous, view(true), None);
    let open = button(&next, "Open");
    assert!(is_disabled(&next, open));
    assert_eq!(focused_name(&next).as_deref(), Some("After"));
    assert_ring_matches_focus(&next, Some(button(&next, "After")));
}

#[test]
fn the_last_control_hands_focus_back_to_the_previous_one() {
    let view = |disabled| {
        Column::new()
            .child(Button::new("Before"))
            .child(Button::new("Open").disabled(disabled))
    };
    let mut previous = mount(view(false));
    let open = button(&previous, "Open");
    previous.focus(Some(open));
    let next = remount(&previous, view(true), None);
    assert_eq!(focused_name(&next).as_deref(), Some("Before"));
    assert_ring_matches_focus(&next, Some(button(&next, "Before")));
}

#[test]
fn a_pointer_focused_control_disabled_by_a_remount_leaves_no_ring_behind() {
    let view = |disabled| page(Button::new("Open").disabled(disabled));
    let mut previous = mount(view(false));
    let open = button(&previous, "Open");
    previous.focus_without_ring(Some(open));
    assert!(ringed(&previous).is_empty());
    let next = remount(&previous, view(true), None);
    assert_eq!(focused_name(&next).as_deref(), Some("After"));
    assert!(ringed(&next).is_empty(), "the modality carries over");
}

#[test]
fn nothing_to_hand_focus_to_leaves_no_focus_and_no_ring() {
    let view = |disabled| Column::new().child(Button::new("Only").disabled(disabled));
    let mut previous = mount(view(false));
    let only = button(&previous, "Only");
    previous.focus(Some(only));
    let next = remount(&previous, view(true), None);
    assert_ring_matches_focus(&next, None);
}

/// Every focusable kind, rebuilt disabled while focused.
#[test]
fn every_focusable_kind_loses_focus_and_ring_when_a_remount_disables_it() {
    type Make = fn() -> Box<dyn View>;
    let kinds: [(&str, Make); 8] = [
        ("button", || Box::new(Button::new("Mid"))),
        ("checkbox", || Box::new(Checkbox::new(false))),
        ("switch", || Box::new(Switch::new(false))),
        ("radio", || Box::new(Radio::new(false))),
        ("slider", || Box::new(Slider::new(0.5, 0.0, 1.0))),
        ("tab", || Box::new(TabBar::new().child(Tab::new("Mid")))),
        ("dropdown", || Box::new(Dropdown::new("Mid"))),
        ("text input", || Box::new(TextInput::new("").label("Mid"))),
    ];
    for (kind, make) in kinds {
        let mut previous = mount(page(make()));
        let control = middle(&previous);
        assert!(previous.focus(Some(control)), "{kind}: focus");
        assert_ring_matches_focus(&previous, Some(control));

        let probe = App::mount_with_size(page(make()), 500, 400);
        let disabled = middle(&probe);
        let next = remount(&previous, page(make()), Some(disabled));
        assert!(is_disabled(&next, disabled), "{kind}: disabled");
        assert_eq!(focused_name(&next).as_deref(), Some("After"), "{kind}");
        assert_ring_matches_focus(&next, Some(button(&next, "After")));
    }
}

/// A reactive `disabled` binding flips the control in place, with no rebuild.
#[test]
fn a_reactive_disable_moves_the_ring_without_a_rebuild() {
    type Make = fn() -> Box<dyn View>;
    let kinds: [(&str, Make); 5] = [
        ("button", || Box::new(Button::new("Mid"))),
        ("checkbox", || Box::new(Checkbox::new(false))),
        ("switch", || Box::new(Switch::new(false))),
        ("radio", || Box::new(Radio::new(false))),
        ("slider", || Box::new(Slider::new(0.5, 0.0, 1.0))),
    ];
    for (kind, make) in kinds {
        let off = create_signal(false);
        let view = page(ReactiveAttr::new(
            make(),
            move || off.get(),
            Attribute::Disabled,
        ));
        let mut app = mount(view);
        let control = middle(&app);
        assert!(app.focus(Some(control)), "{kind}: focus");
        assert_ring_matches_focus(&app, Some(control));

        off.set(true);
        app.frame();
        assert!(is_disabled(&app, control), "{kind}: disabled");
        assert_eq!(focused_name(&app).as_deref(), Some("After"), "{kind}");
        assert_ring_matches_focus(&app, Some(button(&app, "After")));

        // Tab continues from the neighbour, and enabling again leaves focus.
        off.set(false);
        app.frame();
        assert_ring_matches_focus(&app, Some(button(&app, "After")));
    }
}

#[test]
fn a_reactive_disable_of_an_unfocused_control_leaves_focus_and_ring_alone() {
    let off = create_signal(false);
    let mut app = mount(page(ReactiveAttr::new(
        Button::new("Mid"),
        move || off.get(),
        Attribute::Disabled,
    )));
    let before = button(&app, "Before");
    app.focus(Some(before));
    off.set(true);
    app.frame();
    assert_ring_matches_focus(&app, Some(before));
}

/// A modal opened by the rebuild takes focus; the disabled opener must not
/// keep a ring behind it.
#[test]
fn a_modal_opening_over_a_control_that_becomes_disabled_leaves_one_ring() {
    let closed = || page(Button::new("Open"));
    let open = || {
        page(Button::new("Open").disabled(true)).child(
            Dialog::new("Preview")
                .child(Button::new("Close"))
                .on_dismiss(|| {}),
        )
    };
    let mut previous = mount(closed());
    let opener = button(&previous, "Open");
    previous.focus(Some(opener));
    let next = remount(&previous, open(), None);
    assert_eq!(focused_name(&next).as_deref(), Some("Close"));
    assert_ring_matches_focus(&next, Some(button(&next, "Close")));
}

/// A theme switch rebuilds the tree through the same inheritance path.
#[test]
fn a_theme_rebuild_that_disables_the_focused_button_moves_the_ring() {
    use std::cell::Cell;
    use std::rc::Rc;

    let disabled = Rc::new(Cell::new(false));
    let read = disabled.clone();
    let mut app = App::mount_themed(theme::LIGHT, move || {
        page(Button::new("Open").disabled(read.get()))
    });
    app.frame();
    let open = button(&app, "Open");
    assert!(app.focus(Some(open)));
    assert_ring_matches_focus(&app, Some(open));

    disabled.set(true);
    assert!(app.set_theme(theme::DARK));
    app.frame();
    assert!(is_disabled(&app, button(&app, "Open")));
    assert_eq!(focused_name(&app).as_deref(), Some("After"));
    assert_ring_matches_focus(&app, Some(button(&app, "After")));
}
