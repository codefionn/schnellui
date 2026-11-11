//! Where focus goes when a window or modal closes with no opener to return to, and which
//! input decides whether the ring shows there.

use super::*;
use crate::windowed::headless::{HeadlessHost, PointerButton};
use schnellui::a11y::Role;
use schnellui::widgets::{Button, Column, Dialog, Stack};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Default)]
struct Page {
    opener: bool,
    modal: bool,
    a: bool,
    b: bool,
}

const CLOSED: Page = Page {
    opener: true,
    modal: false,
    a: false,
    b: false,
};

fn view(page: Page, closed: &Rc<Cell<bool>>) -> Stack {
    let dismiss = |closed: &Rc<Cell<bool>>| {
        let closed = closed.clone();
        move || closed.set(true)
    };
    let mut column = Column::new();
    if page.opener {
        column = column.child(Button::new("Open modal"));
    }
    column = column.child(Button::new("Elsewhere"));
    let mut column = Stack::new().fill().child(column);
    if page.modal {
        column = column.child(
            Dialog::new("Modal")
                .child(Button::new("Confirm"))
                .on_dismiss(dismiss(closed)),
        );
    }
    if page.a {
        column = column.child(
            Dialog::new("Window A")
                .modeless()
                .non_fixed()
                .child(Button::new("Assign"))
                .on_dismiss(dismiss(closed)),
        );
    }
    if page.b {
        column = column.child(
            Dialog::new("Window B")
                .modeless()
                .non_fixed()
                .child(Button::new("Zoom"))
                .on_dismiss(dismiss(closed)),
        );
    }
    column
}

fn mounted(page: Page, closed: &Rc<Cell<bool>>) -> App {
    let mut app = App::mount_with_size(view(page, closed), 900, 700);
    app.frame();
    app
}

fn remount(previous: &App, page: Page, closed: &Rc<Cell<bool>>) -> App {
    let mut app = App::mount_with_size(view(page, closed), 900, 700);
    app.inherit_remount_state(previous);
    app.frame();
    app
}

fn button(app: &App, name: &str) -> scene::WidgetId {
    app.find_widget(Role::Button, Some(name))
        .unwrap_or_else(|| panic!("no button {name}"))
}

fn focused_name(app: &App) -> Option<String> {
    let id = app.focused_widget()?;
    app.scene().a11y(id)?.name.clone()
}

/// The window point at the middle of a button, once a frame has laid it out.
fn centre(app: &App, name: &str) -> scene::Point {
    let r = app.scene().layout(button(app, name)).unwrap().rect;
    scene::Point {
        x: r.x + r.width / 2.0,
        y: r.y + r.height / 2.0,
    }
}

/// A press on the body of window A, on no control.
fn click_window_body(app: App) -> HeadlessHost {
    let mut host = HeadlessHost::new(app);
    let panel = {
        let app = host.app();
        let assign = button(app, "Assign");
        schnellui::widgets::enclosing_dialog_panel(&app.widgets, app.scene(), assign).unwrap()
    };
    let r = host.app().scene().layout(panel).unwrap().rect;
    host.pointer_move(scene::Point {
        x: r.x + r.width - 12.0,
        y: r.y + r.height / 2.0,
    });
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    assert!(
        host.app().focused_widget().is_none(),
        "the body takes no focus"
    );
    host
}

#[test]
fn escape_on_a_window_open_at_mount_after_a_press_on_its_body_lands_on_the_page() {
    let closed = Rc::new(Cell::new(false));
    let page = Page { a: true, ..CLOSED };
    let mut host = click_window_body(mounted(page, &closed));
    assert!(host.app_mut().dispatch_key(UiKey::Escape));
    assert!(closed.get());
    let after = remount(host.app(), CLOSED, &closed);
    assert_eq!(focused_name(&after).as_deref(), Some("Open modal"));
    assert!(after.focus_ring_shown(), "Escape shows the ring");
}

#[test]
fn escape_on_a_modal_open_at_mount_lands_on_the_first_tab_stop_with_the_ring() {
    let closed = Rc::new(Cell::new(false));
    let page = Page {
        modal: true,
        ..CLOSED
    };
    let mut app = mounted(page, &closed);
    assert!(app.dispatch_key(UiKey::Escape));
    assert!(closed.get());
    let after = remount(&app, CLOSED, &closed);
    assert_eq!(focused_name(&after).as_deref(), Some("Open modal"));
    assert!(after.focus_ring_shown());
}

#[test]
fn a_modal_opened_by_click_and_closed_by_escape_returns_to_the_opener_with_the_ring() {
    let closed = Rc::new(Cell::new(false));
    let mut base = mounted(CLOSED, &closed);
    let opener = button(&base, "Open modal");
    assert!(base.pointer_focus(Some(opener)));
    let page = Page {
        modal: true,
        ..CLOSED
    };
    let mut open = remount(&base, page, &closed);
    assert_eq!(focused_name(&open).as_deref(), Some("Confirm"));
    assert!(open.dispatch_key(UiKey::Escape));
    let after = remount(&open, CLOSED, &closed);
    assert_eq!(focused_name(&after).as_deref(), Some("Open modal"));
    assert!(after.focus_ring_shown());
}

#[test]
fn a_modal_opened_by_keyboard_and_closed_by_clicking_close_returns_without_a_ring() {
    let closed = Rc::new(Cell::new(false));
    let mut base = mounted(CLOSED, &closed);
    let opener = button(&base, "Open modal");
    assert!(base.focus(Some(opener)));
    let page = Page {
        modal: true,
        ..CLOSED
    };
    let open = remount(&base, page, &closed);
    let mut host = HeadlessHost::new(open);
    // The host's first frame lays the overlay out.
    host.pointer_move(scene::Point { x: 1.0, y: 1.0 });
    let close = centre(host.app(), "Close Modal");
    host.pointer_move(close);
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    assert!(closed.get());
    let after = remount(host.app(), CLOSED, &closed);
    assert_eq!(focused_name(&after).as_deref(), Some("Open modal"));
    assert!(!after.focus_ring_shown(), "a pointer close shows no ring");
}

#[test]
fn a_window_whose_opener_was_removed_closes_to_the_first_tab_stop() {
    let closed = Rc::new(Cell::new(false));
    let mut base = mounted(CLOSED, &closed);
    let opener = button(&base, "Open modal");
    assert!(base.focus(Some(opener)));
    // The opener goes while the window is open.
    let page = Page {
        a: true,
        ..Page::default()
    };
    let mut open = remount(&base, page, &closed);
    let assign = button(&open, "Assign");
    assert!(open.focus(Some(assign)));
    let after = remount(&open, Page::default(), &closed);
    assert_eq!(focused_name(&after).as_deref(), Some("Elsewhere"));
    assert!(after.focus_ring_shown());
}

#[test]
fn a_window_with_no_opener_closes_to_the_front_windows_last_control() {
    let closed = Rc::new(Cell::new(false));
    let page = Page {
        a: true,
        b: true,
        ..CLOSED
    };
    let mut app = mounted(page, &closed);
    let zoom = button(&app, "Zoom");
    assert!(app.focus(Some(zoom)));
    let assign = button(&app, "Assign");
    assert!(app.focus(Some(assign)));
    let after = remount(&app, Page { b: true, ..CLOSED }, &closed);
    assert_eq!(focused_name(&after).as_deref(), Some("Zoom"));
    assert!(after.focus_ring_shown());
}

#[test]
fn replacing_a_subtree_with_a_window_open_at_mount_lands_on_the_page() {
    let slot = schnellui::widgets::ComponentRef::new();
    let mut app = App::mount_with_size(
        Column::new().child(Button::new("Open modal")).child(
            Column::new().with_ref(slot).child(
                Dialog::new("Window A")
                    .modeless()
                    .non_fixed()
                    .child(Button::new("Assign")),
            ),
        ),
        900,
        700,
    );
    app.frame();
    let assign = button(&app, "Assign");
    assert!(app.focus(Some(assign)));
    app.replace_subtree(slot, Column::new()).unwrap();
    app.frame();
    assert_eq!(focused_name(&app).as_deref(), Some("Open modal"));
    assert!(app.focus_ring_shown());
}
