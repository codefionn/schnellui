//! Closing a modal dialog returns focus to the control that opened it.

use super::*;
use schnellui::a11y::Role;
use schnellui::widgets::{Button, Column, Dialog, DialogPosition};
use std::cell::Cell;
use std::rc::Rc;

/// Mirrors the dialogs example: a modeless peer holding the opener, plus a
/// stack of `levels` modals. The second one has its own opener inside the first.
fn view(levels: u8) -> Column {
    let mut dialogs = Column::new();
    if levels >= 1 {
        let mut first = Dialog::new("First modal")
            .width(420.0)
            .child(Button::new("Return"))
            .child(Button::new("Next level"))
            .on_dismiss(|| {});
        first = first.child(Button::new("Approve"));
        dialogs = dialogs.child(first);
    }
    if levels >= 2 {
        dialogs = dialogs.child(
            Dialog::new("Second modal")
                .width(300.0)
                .child(Button::new("Close second"))
                .on_dismiss(|| {}),
        );
    }
    dialogs.child(
        Dialog::new("Layer notes")
            .modeless()
            .position(DialogPosition::Right)
            .width(280.0)
            .child(Button::new("Reopen approval")),
    )
}

fn button(app: &App, name: &str) -> scene::WidgetId {
    app.find_widget(Role::Button, Some(name))
        .unwrap_or_else(|| panic!("no button {name}"))
}

fn focused_name(app: &App) -> Option<String> {
    let id = app.focused_widget()?;
    app.scene().a11y(id)?.name.clone()
}

fn ring(app: &App) -> bool {
    app.focus_ring_shown()
}

/// Mounts `levels` over `previous`, as a windowed remount does.
fn remount(previous: &App, levels: u8) -> App {
    let mut app = App::mount_with_size(view(levels), 900, 700);
    app.inherit_remount_state(previous);
    app.frame();
    app
}

fn opened_with_keyboard() -> (App, App) {
    let mut base = App::mount_with_size(view(0), 900, 700);
    base.frame();
    let opener = button(&base, "Reopen approval");
    assert!(base.focus(Some(opener)));
    assert!(ring(&base));
    let modal = remount(&base, 1);
    (base, modal)
}

#[test]
fn escape_returns_focus_and_ring_to_the_opener() {
    let (_, modal) = opened_with_keyboard();
    assert_eq!(focused_name(&modal).as_deref(), Some("Return"));
    let closed = remount(&modal, 0);
    assert_eq!(focused_name(&closed).as_deref(), Some("Reopen approval"));
    assert!(ring(&closed), "a keyboard-opened modal returns a ring");
}

#[test]
fn a_moved_focus_inside_the_modal_still_returns_to_the_opener() {
    let (_, mut modal) = opened_with_keyboard();
    let approve = button(&modal, "Approve");
    assert!(modal.focus(Some(approve)));
    let closed = remount(&modal, 0);
    assert_eq!(focused_name(&closed).as_deref(), Some("Reopen approval"));
}

#[test]
fn the_opener_survives_a_remount_with_the_modal_still_open() {
    let (_, modal) = opened_with_keyboard();
    let again = remount(&modal, 1);
    let closed = remount(&again, 0);
    assert_eq!(focused_name(&closed).as_deref(), Some("Reopen approval"));
}

#[test]
fn a_theme_rebuild_keeps_the_opener() {
    let open = Rc::new(Cell::new(false));
    let shown = open.clone();
    let mut base = App::mount_themed_with_size_scaled(
        ThemeMode::Fixed(theme::LIGHT),
        move || view(u8::from(shown.get())),
        900,
        700,
        1.0,
    );
    base.frame();
    let opener = button(&base, "Reopen approval");
    assert!(base.focus(Some(opener)));
    // The host's structural remount opens the modal ...
    open.set(true);
    let shown = open.clone();
    let mut modal = App::mount_themed_with_size_scaled(
        ThemeMode::Fixed(theme::LIGHT),
        move || view(u8::from(shown.get())),
        900,
        700,
        1.0,
    );
    modal.inherit_remount_state(&base);
    modal.frame();
    assert_eq!(focused_name(&modal).as_deref(), Some("Return"));
    // ... a theme switch rebuilds it ...
    assert!(modal.set_theme(theme::DARK));
    modal.frame();
    assert_eq!(focused_name(&modal).as_deref(), Some("Return"));
    // ... and closing it still lands on the opener.
    open.set(false);
    let closed = remount(&modal, 0);
    assert_eq!(focused_name(&closed).as_deref(), Some("Reopen approval"));
    assert!(ring(&closed));
}

#[test]
fn nested_modals_return_one_level_at_a_time() {
    let (_, first) = opened_with_keyboard();
    let mut first = first;
    let next = button(&first, "Next level");
    assert!(first.focus(Some(next)));
    let second = remount(&first, 2);
    assert_eq!(focused_name(&second).as_deref(), Some("Close second"));
    let back_to_first = remount(&second, 1);
    assert_eq!(focused_name(&back_to_first).as_deref(), Some("Next level"));
    let closed = remount(&back_to_first, 0);
    assert_eq!(focused_name(&closed).as_deref(), Some("Reopen approval"));
}

#[test]
fn a_pointer_opened_modal_returns_without_a_ring() {
    let mut base = App::mount_with_size(view(0), 900, 700);
    base.frame();
    let opener = button(&base, "Reopen approval");
    assert!(base.pointer_focus(Some(opener)));
    let modal = remount(&base, 1);
    assert_eq!(focused_name(&modal).as_deref(), Some("Return"));
    let closed = remount(&modal, 0);
    assert_eq!(focused_name(&closed).as_deref(), Some("Reopen approval"));
    assert!(!ring(&closed), "pointer focus returns without a ring");
}

#[test]
fn replacing_the_subtree_that_holds_a_modal_returns_to_the_opener() {
    let slot = schnellui::widgets::ComponentRef::new();
    let mut app = App::mount_with_size(
        Column::new()
            .child(Button::new("Reopen approval"))
            .child(Column::new().with_ref(slot)),
        900,
        700,
    );
    app.frame();
    let opener = button(&app, "Reopen approval");
    assert!(app.focus(Some(opener)));
    let modal = Column::new().child(
        // A fixed dialog attaches at the root, outside the replaced branch.
        Dialog::new("Final approval")
            .non_fixed()
            .child(Button::new("Return"))
            .on_dismiss(|| {}),
    );
    app.replace_subtree(slot, modal).unwrap();
    app.frame();
    assert!(schnellui::widgets::active_modal_panel(app.scene()).is_some());
    let ret = button(&app, "Return");
    assert!(app.focus(Some(ret)));
    app.replace_subtree(slot, Column::new()).unwrap();
    app.frame();
    assert_eq!(focused_name(&app).as_deref(), Some("Reopen approval"));
    assert!(ring(&app));
}

#[test]
fn a_modeless_dialog_keeps_focus_through_a_remount() {
    let mut base = App::mount_with_size(view(0), 900, 700);
    base.frame();
    let opener = button(&base, "Reopen approval");
    assert!(base.focus(Some(opener)));
    let again = remount(&base, 0);
    assert_eq!(focused_name(&again).as_deref(), Some("Reopen approval"));
}
