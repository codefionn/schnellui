//! Closing a window or modeless dialog returns focus to the control that
//! opened it, and never to another window's Close button.

use super::*;
use schnellui::a11y::Role;
use schnellui::widgets::{Button, Column, Dialog, Row};

const A: u8 = 1;
const B: u8 = 2;

/// The dialogs example's desktop: launchers that rest disabled while their
/// window is open, and one modeless window per bit of `open`.
fn view(open: u8) -> Column {
    let mut windows = Column::new();
    if open & A != 0 {
        windows = windows.child(
            Dialog::new("Window A")
                .modeless()
                .non_fixed()
                .child(Button::new("Assign"))
                .on_dismiss(|| {}),
        );
    }
    if open & B != 0 {
        windows = windows.child(
            Dialog::new("Window B")
                .modeless()
                .non_fixed()
                .child(Button::new("Zoom"))
                .on_dismiss(|| {}),
        );
    }
    Column::new()
        .child(
            Row::new()
                .child(Button::new("Open A").disabled(open & A != 0))
                .child(Button::new("Open B").disabled(open & B != 0)),
        )
        // Sits between the launchers and the windows, so the nearest
        // neighbour of a window's controls is never an opener.
        .child(Button::new("Elsewhere"))
        .child(windows)
}

fn button(app: &App, name: &str) -> scene::WidgetId {
    app.find_widget(Role::Button, Some(name))
        .unwrap_or_else(|| panic!("no button {name}"))
}

fn focused_name(app: &App) -> Option<String> {
    let id = app.focused_widget()?;
    app.scene().a11y(id)?.name.clone()
}

fn remount(previous: &App, open: u8) -> App {
    let mut app = App::mount_with_size(view(open), 900, 700);
    app.inherit_remount_state(previous);
    app.frame();
    app
}

fn mounted(open: u8) -> App {
    let mut app = App::mount_with_size(view(open), 900, 700);
    app.frame();
    app
}

/// Focus is on a control outside every window.
fn outside_windows(app: &App) -> bool {
    app.focused_widget().is_some_and(|id| {
        schnellui::widgets::enclosing_dialog_panel(&app.widgets, app.scene(), id).is_none()
    })
}

#[test]
fn closing_a_window_opened_with_the_keyboard_returns_a_ring_to_its_opener() {
    let mut base = mounted(0);
    let opener = button(&base, "Open A");
    assert!(base.focus(Some(opener)));
    let mut open = remount(&base, A);
    let assign = button(&open, "Assign");
    assert!(open.focus(Some(assign)));
    let closed = remount(&open, 0);
    assert_eq!(focused_name(&closed).as_deref(), Some("Open A"));
    assert!(closed.focus_ring_shown());
}

#[test]
fn closing_a_window_opened_with_the_pointer_returns_without_a_ring() {
    let mut base = mounted(0);
    let opener = button(&base, "Open A");
    assert!(base.pointer_focus(Some(opener)));
    let mut open = remount(&base, A);
    let assign = button(&open, "Assign");
    assert!(open.pointer_focus(Some(assign)));
    let closed = remount(&open, 0);
    assert_eq!(focused_name(&closed).as_deref(), Some("Open A"));
    assert!(!closed.focus_ring_shown());
}

#[test]
fn closing_one_of_two_button_opened_windows_returns_to_its_own_opener() {
    let mut base = mounted(0);
    let open_a = button(&base, "Open A");
    assert!(base.focus(Some(open_a)));
    let mut one = remount(&base, A);
    let open_b = button(&one, "Open B");
    let _ = one.focus(Some(open_b));
    let mut two = remount(&one, A | B);
    // Window A survives the remount and keeps its own opener.
    let assign = button(&two, "Assign");
    assert!(two.focus(Some(assign)));
    let closed = remount(&two, B);
    assert_eq!(focused_name(&closed).as_deref(), Some("Open A"));
}

#[test]
fn closing_a_window_open_at_mount_does_not_land_in_another_window() {
    for ring in [true, false] {
        let mut base = mounted(A | B);
        let assign = button(&base, "Assign");
        if ring {
            assert!(base.focus(Some(assign)));
        } else {
            assert!(base.pointer_focus(Some(assign)));
        }
        let closed = remount(&base, B);
        assert!(
            outside_windows(&closed),
            "ring={ring}: {:?}",
            focused_name(&closed)
        );
        assert_eq!(closed.focus_ring_shown(), ring);
    }
}

#[test]
fn a_window_open_at_mount_keeps_focus_inside_when_a_sibling_closes() {
    let mut base = mounted(A | B);
    let zoom = button(&base, "Zoom");
    assert!(base.focus(Some(zoom)));
    let closed = remount(&base, B);
    assert_eq!(focused_name(&closed).as_deref(), Some("Zoom"));
}

#[test]
fn replacing_a_subtree_that_holds_a_window_returns_to_the_opener() {
    let slot = schnellui::widgets::ComponentRef::new();
    let mut app = App::mount_with_size(
        Column::new()
            .child(Button::new("Open A"))
            .child(Button::new("Elsewhere"))
            .child(Column::new().with_ref(slot)),
        900,
        700,
    );
    app.frame();
    let opener = button(&app, "Open A");
    assert!(app.focus(Some(opener)));
    let window = Column::new().child(
        Dialog::new("Window A")
            .modeless()
            .non_fixed()
            .child(Button::new("Assign")),
    );
    app.replace_subtree(slot, window).unwrap();
    app.frame();
    let assign = button(&app, "Assign");
    assert!(app.focus(Some(assign)));
    app.replace_subtree(slot, Column::new()).unwrap();
    app.frame();
    assert_eq!(focused_name(&app).as_deref(), Some("Open A"));
    assert!(app.focus_ring_shown());
}

/// Like [`view`], but the launcher stays enabled while its window is open.
fn view_enabled(open: bool) -> Column {
    let mut col = Column::new()
        .child(Button::new("Open A"))
        .child(Button::new("Elsewhere"));
    if open {
        col = col.child(
            Dialog::new("Window A")
                .modeless()
                .non_fixed()
                .child(Button::new("Assign")),
        );
    }
    col
}

#[test]
fn a_window_opened_with_the_keyboard_takes_focus_from_its_disabled_opener() {
    for ring in [true, false] {
        let mut base = mounted(0);
        let opener = button(&base, "Open A");
        if ring {
            assert!(base.focus(Some(opener)));
        } else {
            assert!(base.pointer_focus(Some(opener)));
        }
        let open = remount(&base, A);
        // Not the neighbour "Open B": the window's first control.
        assert_eq!(
            focused_name(&open).as_deref(),
            Some("Assign"),
            "ring={ring}"
        );
        assert_eq!(open.focus_ring_shown(), ring);
        // Closing it still returns to the opener.
        let closed = remount(&open, 0);
        assert_eq!(
            focused_name(&closed).as_deref(),
            Some("Open A"),
            "ring={ring}"
        );
        assert_eq!(closed.focus_ring_shown(), ring);
    }
}

#[test]
fn a_window_opened_beside_an_opener_that_keeps_focus_does_not_steal_it() {
    let mut base = App::mount_with_size(view_enabled(false), 900, 700);
    base.frame();
    let opener = button(&base, "Open A");
    assert!(base.focus(Some(opener)));
    let mut open = App::mount_with_size(view_enabled(true), 900, 700);
    open.inherit_remount_state(&base);
    open.frame();
    assert_eq!(focused_name(&open).as_deref(), Some("Open A"));
    assert!(open.focus_ring_shown());
}

#[test]
fn replacing_a_subtree_that_opens_a_window_over_a_removed_opener_focuses_the_window() {
    let slot = schnellui::widgets::ComponentRef::new();
    let mut app = App::mount_with_size(
        Column::new()
            .child(Button::new("Elsewhere"))
            .child(Column::new().with_ref(slot).child(Button::new("Open A"))),
        900,
        700,
    );
    app.frame();
    let opener = button(&app, "Open A");
    assert!(app.focus(Some(opener)));
    let window = Column::new().child(
        Dialog::new("Window A")
            .modeless()
            .non_fixed()
            .child(Button::new("Assign")),
    );
    app.replace_subtree(slot, window).unwrap();
    app.frame();
    assert_eq!(focused_name(&app).as_deref(), Some("Assign"));
    assert!(app.focus_ring_shown());
}
