//! A focus hand-off (the focused control is disabled or removed) must not park
//! invisible focus on a control the user did not choose. Focus that came from a
//! pointer press is cleared instead, Enter then does nothing, and Tab continues
//! from where the lost control was, as in a browser. Keyboard focus keeps the
//! visible hand-off covered by `tests_disabled_focus`.

use super::*;
use crate::windowed::headless::{HeadlessHost, HostKey, PointerButton};
use schnellui::a11y::Role;
use schnellui::scene::{Point, WidgetId};
use schnellui::signal::create_signal;
use schnellui::widgets::{Attribute, Button, Column, ComponentRef, ReactiveAttr};
use std::cell::Cell;
use std::rc::Rc;

fn button(app: &App, label: &str) -> WidgetId {
    app.find_widget(Role::Button, Some(label)).unwrap()
}

fn focused_name(app: &App) -> Option<String> {
    app.scene().a11y(app.focused_widget()?)?.name.clone()
}

fn center(app: &App, id: WidgetId) -> Point {
    let r = app.scene().layout(id).unwrap().rect;
    Point {
        x: r.x + r.width / 2.0,
        y: r.y + r.height / 2.0,
    }
}

/// The todo sequence: a pointer press on "Clear" disables it (as Clear
/// completed does once nothing is left to clear), then Enter is pressed.
fn clear_view(presses: &Rc<Cell<u32>>, cleared: schnellui::signal::Signal<bool>) -> Column {
    let remove = |label: &str| {
        let presses = presses.clone();
        Button::new(label.to_string()).on_click(move || presses.set(presses.get() + 1))
    };
    Column::new()
        .child(remove("Remove A"))
        .child(ReactiveAttr::new(
            Button::new("Clear").on_click(move || cleared.set(true)),
            move || cleared.get(),
            Attribute::Disabled,
        ))
        .child(remove("Remove B"))
}

fn press_clear() -> (HeadlessHost, Rc<Cell<u32>>) {
    let presses = Rc::new(Cell::new(0));
    let cleared = create_signal(false);
    let mut host = HeadlessHost::new(App::mount_with_size(
        clear_view(&presses, cleared),
        400,
        300,
    ));
    host.frame();
    let at = center(host.app(), button(host.app(), "Clear"));
    host.pointer_move(at);
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    host.frame();
    (host, presses)
}

#[test]
fn a_pointer_press_that_disables_its_button_leaves_enter_with_nothing_to_activate() {
    let (mut host, presses) = press_clear();
    let clear = button(host.app(), "Clear");
    assert!(disabled(host.app(), clear));
    assert_eq!(host.app().focused_widget(), None, "no invisible focus");
    host.key(&HostKey::Enter, false, false);
    host.key(&HostKey::Space, false, false);
    assert_eq!(presses.get(), 0, "no control the user never touched fired");
}

#[test]
fn tab_after_such_a_press_continues_from_the_disabled_control() {
    let (mut host, _) = press_clear();
    host.key(&HostKey::Tab, false, false);
    assert_eq!(focused_name(host.app()).as_deref(), Some("Remove B"));
    assert!(host.app().focus_ring_shown(), "Tab shows the ring");

    let (mut host, _) = press_clear();
    host.key(&HostKey::Tab, true, false);
    assert_eq!(focused_name(host.app()).as_deref(), Some("Remove A"));
}

fn disabled(app: &App, id: WidgetId) -> bool {
    schnellui::a11y::StateFlags(app.scene().a11y(id).unwrap().state)
        .contains(schnellui::a11y::StateFlags::DISABLED)
}

fn page(middle: &str) -> Column {
    Column::new()
        .child(Button::new("Before"))
        .child(Button::new(middle.to_string()))
        .child(Button::new("After"))
}

fn remount_without_middle(previous: &App) -> App {
    let mut next = App::mount_with_size(
        Column::new()
            .child(Button::new("Before"))
            .child(Button::new("After")),
        400,
        300,
    );
    next.inherit_remount_state(previous);
    next.frame();
    next
}

#[test]
fn a_removed_pointer_focused_control_clears_focus_and_tab_continues() {
    for (backwards, expect) in [(false, "After"), (true, "Before")] {
        let mut previous = App::mount_with_size(page("Mid"), 400, 300);
        previous.frame();
        let mid = button(&previous, "Mid");
        assert!(previous.pointer_focus(Some(mid)));

        let mut next = remount_without_middle(&previous);
        assert_eq!(next.focused_widget(), None);
        assert!(!next.dispatch_key(UiKey::Enter), "Enter finds nothing");
        assert!(next.dispatch_key(UiKey::Tab { shift: backwards }));
        assert_eq!(focused_name(&next).as_deref(), Some(expect));
        assert!(next.focus_ring_shown());
    }
}

#[test]
fn the_starting_point_survives_a_further_remount() {
    let mut previous = App::mount_with_size(page("Mid"), 400, 300);
    previous.frame();
    let mid = button(&previous, "Mid");
    assert!(previous.pointer_focus(Some(mid)));
    let next = remount_without_middle(&previous);
    let mut third = remount_without_middle(&next);
    assert_eq!(third.focused_widget(), None);
    assert!(third.dispatch_key(UiKey::Tab { shift: true }));
    assert_eq!(focused_name(&third).as_deref(), Some("Before"));
}

#[test]
fn a_removed_last_control_continues_backwards_from_its_previous_neighbour() {
    let mut previous = App::mount_with_size(
        Column::new()
            .child(Button::new("A"))
            .child(Button::new("B")),
        400,
        300,
    );
    previous.frame();
    let b = button(&previous, "B");
    assert!(previous.pointer_focus(Some(b)));
    let mut next = App::mount_with_size(Column::new().child(Button::new("A")), 400, 300);
    next.inherit_remount_state(&previous);
    next.frame();
    assert_eq!(next.focused_widget(), None);
    assert!(next.dispatch_key(UiKey::Tab { shift: true }));
    assert_eq!(focused_name(&next).as_deref(), Some("A"));
}

#[test]
fn replace_subtree_clears_pointer_focus_and_keeps_the_place() {
    for (backwards, expect) in [(false, "After"), (true, "Before")] {
        let slot = ComponentRef::new();
        let mut app = App::mount_with_size(
            Column::new()
                .child(Button::new("Before"))
                .child(Button::new("Mid").with_ref(slot))
                .child(Button::new("After")),
            400,
            300,
        );
        app.frame();
        let mid = button(&app, "Mid");
        assert!(app.pointer_focus(Some(mid)));
        app.replace_subtree(slot, Column::new()).unwrap();
        app.frame();
        assert_eq!(app.focused_widget(), None);
        assert!(!app.dispatch_key(UiKey::Enter));
        assert!(app.dispatch_key(UiKey::Tab { shift: backwards }));
        assert_eq!(focused_name(&app).as_deref(), Some(expect));
    }
}

#[test]
fn a_focus_landing_elsewhere_drops_the_starting_point() {
    let (mut host, _) = press_clear();
    let before = button(host.app(), "Remove A");
    assert!(host.app_mut().focus(Some(before)));
    assert!(host.app_mut().focus(None));
    host.key(&HostKey::Tab, false, false);
    assert_eq!(
        focused_name(host.app()).as_deref(),
        Some("Remove A"),
        "walks from the top again, not from the old place"
    );
}

/// Buttons in a column, one per name.
fn buttons(names: &[&str]) -> Column {
    names.iter().fold(Column::new(), |column, name| {
        column.child(Button::new(name.to_string()))
    })
}

/// The grouped_tabs sequence: archiving Planning removes its Archive button
/// and moves Planning, with its children, into the Archive section far below.
const BEFORE_ARCHIVE: &[&str] = &[
    "Overview",
    "Planning",
    "Archive Planning",
    "Delete Planning",
    "Roadmap",
    "Research",
    "Build",
    "Archive Build",
    "Archive Notes",
    "Archive",
    "Undo",
];
const AFTER_ARCHIVE: &[&str] = &[
    "Overview",
    "Build",
    "Archive Build",
    "Archive Notes",
    "Archive",
    "Planning",
    "Restore Planning",
    "Roadmap",
    "Research",
    "Undo",
];

fn remount_focused(before: &[&str], after: &[&str], focused: &str, keyboard: bool) -> App {
    let mut previous = App::mount_with_size(buttons(before), 400, 300);
    previous.frame();
    let target = button(&previous, focused);
    if keyboard {
        assert!(previous.focus(Some(target)));
    } else {
        assert!(previous.pointer_focus(Some(target)));
    }
    let mut next = App::mount_with_size(buttons(after), 400, 300);
    next.inherit_remount_state(&previous);
    next.frame();
    next
}

#[test]
fn archiving_a_group_hands_keyboard_focus_to_the_row_that_took_its_place() {
    let next = remount_focused(BEFORE_ARCHIVE, AFTER_ARCHIVE, "Archive Planning", true);
    // Not Roadmap, which moved with Planning into the Archive section.
    assert_eq!(focused_name(&next).as_deref(), Some("Build"));
    assert!(next.focus_ring_shown());
}

#[test]
fn archiving_a_group_leaves_tab_and_shift_tab_around_the_gap() {
    for (backwards, expect) in [(false, "Build"), (true, "Overview")] {
        let mut next = remount_focused(BEFORE_ARCHIVE, AFTER_ARCHIVE, "Archive Planning", false);
        assert_eq!(next.focused_widget(), None);
        assert!(next.dispatch_key(UiKey::Tab { shift: backwards }));
        assert_eq!(focused_name(&next).as_deref(), Some(expect));
    }
}

#[test]
fn a_leaf_that_stays_put_still_hands_off_to_its_nearest_neighbour() {
    // Deleting Notes moves nothing: the old rule and the new one agree.
    let before = ["Overview", "Archive Build", "Delete Notes", "Undo"];
    let after = ["Overview", "Archive Build", "Undo"];
    let mut next = remount_focused(&before, &after, "Delete Notes", false);
    assert!(next.dispatch_key(UiKey::Tab { shift: false }));
    assert_eq!(focused_name(&next).as_deref(), Some("Undo"));
    let next = remount_focused(&before, &after, "Delete Notes", true);
    assert_eq!(focused_name(&next).as_deref(), Some("Undo"));
}

#[test]
fn a_reorder_that_moves_the_neighbours_picks_one_that_stayed_together() {
    // D and E jump ahead of A and B; the gap F sat in is between B and D.
    let before = ["A", "B", "F", "D", "E"];
    let after = ["D", "E", "A", "B"];
    let next = remount_focused(&before, &after, "F", true);
    assert_eq!(focused_name(&next).as_deref(), Some("D"));
}

#[test]
fn restoring_a_group_hands_off_past_the_children_that_moved_with_it() {
    // The reverse trip: Restore Planning goes, Planning and its children
    // move back up above Build.
    let mut next = remount_focused(AFTER_ARCHIVE, BEFORE_ARCHIVE, "Restore Planning", false);
    assert!(next.dispatch_key(UiKey::Tab { shift: false }));
    assert_eq!(focused_name(&next).as_deref(), Some("Undo"));
}

fn tab(label: &str, selected: bool) -> schnellui::widgets::Tab {
    schnellui::widgets::Tab::new(label.to_string()).selected(selected)
}

#[test]
fn shift_tab_after_a_removed_list_control_lands_on_the_tab_bars_stop() {
    use schnellui::widgets::TabBar;
    let page = |names: &[&str]| {
        let list = names
            .iter()
            .fold(Column::new(), |c, n| c.child(Button::new(n.to_string())));
        Column::new()
            .child(
                TabBar::new()
                    .child(tab("Overview", false))
                    .child(tab("Engine", true)),
            )
            .child(list)
    };
    for backwards in [false, true] {
        let mut previous = App::mount_with_size(page(&["X", "Y"]), 400, 300);
        previous.frame();
        let x = button(&previous, "X");
        assert!(previous.pointer_focus(Some(x)));
        let mut next = App::mount_with_size(page(&["Y"]), 400, 300);
        next.inherit_remount_state(&previous);
        next.frame();
        assert!(next.dispatch_key(UiKey::Tab { shift: backwards }));
        let expect = if backwards { "Engine" } else { "Y" };
        assert_eq!(focused_name(&next).as_deref(), Some(expect));
    }
}

/// The grouped_tabs shape: the removed button sits inside the tab bar's span
/// and the bar's one stop (the selected tab) comes after it.
fn bar_page(with_archive: bool) -> Column {
    let bar = schnellui::widgets::TabBar::new().child(tab("Overview", false));
    let bar = if with_archive {
        bar.child(Button::new("Archive Planning"))
    } else {
        bar
    };
    // Undo follows the bar, so the bar's stop is not the last one to wrap to.
    Column::new()
        .child(
            bar.child(tab("Build", false))
                .child(Button::new("Archive Build"))
                .child(tab("Engine", true)),
        )
        .child(Button::new("Undo"))
}

/// Mounts `bar_page`, pointer-removes "Archive Planning" and remounts.
fn bar_page_after_pointer_remove() -> App {
    let mut previous = App::mount_with_size(bar_page(true), 400, 300);
    previous.frame();
    let gone = button(&previous, "Archive Planning");
    assert!(previous.pointer_focus(Some(gone)));
    let mut next = App::mount_with_size(bar_page(false), 400, 300);
    next.inherit_remount_state(&previous);
    next.frame();
    next
}

#[test]
fn a_hand_off_inside_a_tab_bar_steps_by_real_stop_positions() {
    // The bar's stop (Engine) sits after the gap, so no stop precedes it:
    // Shift+Tab wraps, exactly as it does from the first stop after the gap.
    let mut next = bar_page_after_pointer_remove();
    assert!(next.dispatch_key(UiKey::Tab { shift: false }));
    assert_eq!(focused_name(&next).as_deref(), Some("Archive Build"));
    assert!(next.dispatch_key(UiKey::Tab { shift: true }));
    let from_stop = focused_name(&next);

    let mut next = bar_page_after_pointer_remove();
    assert!(next.dispatch_key(UiKey::Tab { shift: true }));
    assert_eq!(focused_name(&next), from_stop, "same as from the next stop");
    assert_eq!(from_stop.as_deref(), Some("Undo"));
}

#[test]
fn a_keyboard_hand_off_lands_on_a_control_tab_can_reach() {
    let mut previous = App::mount_with_size(bar_page(true), 400, 300);
    previous.frame();
    let gone = button(&previous, "Archive Planning");
    assert!(previous.focus(Some(gone)));
    let mut next = App::mount_with_size(bar_page(false), 400, 300);
    next.inherit_remount_state(&previous);
    next.frame();
    // Not the Build tab, which Tab skips.
    assert_eq!(focused_name(&next).as_deref(), Some("Archive Build"));
}
