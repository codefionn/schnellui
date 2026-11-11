//! Input while a pointer drag holds the button. A remount lands at once and the
//! drag carries over to the replacement tree, so a key that opens a dropdown
//! shows its list immediately and the keys that follow act on it, all while
//! the thumb is still held.

use super::*;
use crate::windowed::headless::{ClipboardAction, HeadlessHost, HostKey, PointerButton};
use crate::{Remount, WindowUpdate};
use accesskit::{Action, TreeId};
use schnellui::a11y::{to_access_id, Role, StateFlags};
use schnellui::scene::{Point, Size};
use schnellui::widgets::{
    Button, Column, Dialog, Dropdown, DropdownOption, Scroll, Slider, Stack, Text, TextInput, View,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Copy)]
struct Choice {
    open: bool,
    chosen: usize,
}

const NAMES: [&str; 3] = ["Light", "Dark", "Auto"];

/// What the view reports back: the slider's value and the note field's text.
#[derive(Clone, Default)]
struct Seen {
    volume: Rc<Cell<f32>>,
    note: Rc<RefCell<String>>,
    /// Set when the slider is left out of the tree while the list is open.
    hide_slider: bool,
    /// How many times the list's toggle handler ran.
    toggles: Rc<Cell<u32>>,
}

fn view(state: Rc<Cell<Choice>>, dirty: Rc<Cell<bool>>, seen: Seen) -> impl View {
    let now = state.get();
    let mut dropdown = Dropdown::new("Theme").open(now.open);
    for (i, label) in NAMES.into_iter().enumerate() {
        let (state, dirty) = (state.clone(), dirty.clone());
        dropdown = dropdown.option(
            DropdownOption::new(label)
                .selected(i == now.chosen)
                .on_select(move || {
                    state.set(Choice {
                        open: false,
                        chosen: i,
                    });
                    dirty.set(true);
                }),
        );
    }
    let (toggle_state, toggle_dirty) = (state.clone(), dirty.clone());
    let toggles = seen.toggles.clone();
    dropdown = dropdown.on_toggle(move || {
        toggles.set(toggles.get() + 1);
        let now = toggle_state.get();
        toggle_state.set(Choice {
            open: !now.open,
            ..now
        });
        toggle_dirty.set(true);
    });
    let volume = seen.volume.clone();
    let note = seen.note.clone();
    let mut column = Column::new();
    if !(seen.hide_slider && now.open) {
        column = column.child(
            Slider::new(40.0, 0.0, 100.0)
                .name("Volume")
                .on_change(move |v| volume.set(v)),
        );
    }
    column.child(dropdown).child(
        TextInput::new("")
            .label("Note")
            .on_input(move |text: &str| *note.borrow_mut() = text.to_owned()),
    )
}

fn host_with(hide_slider: bool) -> (HeadlessHost, Rc<Cell<Choice>>, Seen) {
    let state = Rc::new(Cell::new(Choice {
        open: false,
        chosen: 0,
    }));
    let dirty = Rc::new(Cell::new(false));
    let seen = Seen {
        hide_slider,
        ..Seen::default()
    };
    let mut app = App::mount_with_size(view(state.clone(), dirty.clone(), seen.clone()), 640, 400);
    app.frame();
    let mut host = HeadlessHost::new(app);
    let (s, d, v) = (state.clone(), dirty, seen.clone());
    host.set_remount(move |size: Size, _| {
        if !d.replace(false) {
            return None;
        }
        let app = App::mount_with_size(
            view(s.clone(), d.clone(), v.clone()),
            size.width as u32,
            size.height as u32,
        );
        Some(WindowUpdate::Remount(Remount::new(app, "dropdown")))
    });
    (host, state, seen)
}

fn focus(host: &mut HeadlessHost, role: Role, name: &str) {
    let id = host.app().find_widget(role, Some(name)).unwrap();
    host.dispatch_action(&accesskit::ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(id),
        data: None,
    });
}

fn click_action(host: &mut HeadlessHost, role: Role, name: &str) {
    let id = host.app().find_widget(role, Some(name)).unwrap();
    host.dispatch_action(&accesskit::ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(id),
        data: None,
    });
}

fn listed(host: &HeadlessHost) -> bool {
    host.app()
        .find_widget(Role::ListBoxOption, Some("Dark"))
        .is_some()
}

/// Presses the slider thumb and drags it a little right.
fn grab_thumb(host: &mut HeadlessHost) -> Point {
    let thumb = center(host, Role::Slider, "Volume");
    host.pointer_move(thumb);
    host.press(PointerButton::Left);
    let to = Point {
        x: thumb.x + 17.0,
        y: thumb.y,
    };
    host.pointer_move(to);
    to
}

fn center(host: &HeadlessHost, role: Role, name: &str) -> Point {
    let id = host.app().find_widget(role, Some(name)).unwrap();
    let r = host.app().scene().layout(id).unwrap().rect;
    Point {
        x: r.x + r.width / 2.0,
        y: r.y + r.height / 2.0,
    }
}

fn expanded(host: &HeadlessHost) -> bool {
    let t = host
        .app()
        .find_widget(Role::ComboBox, Some("Theme"))
        .unwrap();
    StateFlags(host.app().scene().a11y(t).unwrap().state).contains(StateFlags::EXPANDED)
}

#[test]
fn keys_during_a_slider_drag_open_move_and_pick_in_order() {
    let (mut host, state, seen) = host_with(false);
    let at = grab_thumb(&mut host);
    let held = seen.volume.get();
    assert!(held > 40.0, "the drag moved the thumb");
    focus(&mut host, Role::ComboBox, "Theme");
    host.key(&HostKey::Enter, false, false);
    // Applied at once: the popup is up while the thumb is still held.
    assert!(host.holding(PointerButton::Left));
    assert!(state.get().open);
    assert!(expanded(&host), "list expands before the release");
    assert!(listed(&host), "list is mounted before the release");
    host.key(&HostKey::Down, false, false);
    host.key(&HostKey::Enter, false, false);
    assert_eq!(state.get().chosen, 1, "Down, Enter picks before release");
    assert!(!state.get().open);
    assert!(!expanded(&host));
    // The drag survived the remounts: the thumb still follows the pointer.
    host.pointer_move(Point {
        x: at.x + 20.0,
        y: at.y,
    });
    let moved = seen.volume.get();
    assert!(
        moved > held,
        "slider follows the pointer: {held} -> {moved}"
    );
    host.release(PointerButton::Left);
    host.frame();
    assert_eq!(seen.volume.get(), moved, "release changes no value");
    assert_eq!(state.get().chosen, 1);
    assert!(!state.get().open);
    assert!(!expanded(&host));
}

#[test]
fn an_accesskit_click_during_a_slider_drag_opens_and_picks_at_once() {
    let (mut host, state, seen) = host_with(false);
    let at = grab_thumb(&mut host);
    let held = seen.volume.get();
    click_action(&mut host, Role::ComboBox, "Theme");
    assert!(state.get().open);
    assert!(expanded(&host), "click opens the popup before release");
    assert!(listed(&host));
    click_action(&mut host, Role::ListBoxOption, "Dark");
    assert_eq!(state.get().chosen, 1, "option click picks before release");
    assert!(!expanded(&host));
    host.pointer_move(Point {
        x: at.x + 20.0,
        y: at.y,
    });
    assert!(seen.volume.get() > held, "drag still moves the slider");
    host.release(PointerButton::Left);
    host.frame();
    assert_eq!(state.get().chosen, 1);
    assert!(!state.get().open);
}

#[test]
fn ime_and_paste_during_a_drag_land_in_the_field_of_the_new_tree() {
    let (mut host, _state, seen) = host_with(false);
    grab_thumb(&mut host);
    // Opening the list remounts under the held button.
    focus(&mut host, Role::ComboBox, "Theme");
    host.key(&HostKey::Enter, false, false);
    assert_eq!(host.remounts(), 1, "the remount does not wait for release");
    focus(&mut host, Role::TextInput, "Note");
    host.ime(crate::RawImeEvent::Commit("ab".into()));
    assert_eq!(&*seen.note.borrow(), "ab", "IME commit lands in the field");
    host.key(&HostKey::SelectAll, false, false);
    host.clipboard(ClipboardAction::Copy);
    host.key(&HostKey::End, false, false);
    host.clipboard(ClipboardAction::Paste);
    assert_eq!(&*seen.note.borrow(), "abab", "paste lands in the field");
    host.release(PointerButton::Left);
    host.frame();
    assert_eq!(&*seen.note.borrow(), "abab");
}

#[test]
fn a_remount_that_removes_the_dragged_slider_ends_the_drag_quietly() {
    let (mut host, state, seen) = host_with(true);
    let at = grab_thumb(&mut host);
    let held = seen.volume.get();
    focus(&mut host, Role::ComboBox, "Theme");
    host.key(&HostKey::Enter, false, false);
    assert!(host
        .app()
        .find_widget(Role::Slider, Some("Volume"))
        .is_none());
    assert!(expanded(&host));
    // Moving over where the slider was changes nothing, and nothing panics.
    host.pointer_move(Point {
        x: at.x + 30.0,
        y: at.y,
    });
    assert_eq!(seen.volume.get(), held);
    host.release(PointerButton::Left);
    host.frame();
    assert!(state.get().open, "the release clicked nothing");
    assert_eq!(state.get().chosen, 0);
    assert_eq!(seen.volume.get(), held);
}

fn scroll_view() -> impl View {
    let mut column = Column::new();
    for i in 0..30 {
        column = column.child(Button::new(format!("Row {i}")));
    }
    Scroll::new()
        .label("Rows")
        .size(200.0, 100.0)
        .scrollbar(true)
        .child(column)
}

#[test]
fn a_scrollbar_drag_survives_a_remount_under_the_held_button() {
    let dirty = Rc::new(Cell::new(false));
    let mut app = App::mount_with_size(scroll_view(), 400, 300);
    app.frame();
    let mut host = HeadlessHost::new(app);
    let d = dirty.clone();
    host.set_remount(move |size: Size, _| {
        if !d.replace(false) {
            return None;
        }
        let app = App::mount_with_size(scroll_view(), size.width as u32, size.height as u32);
        Some(WindowUpdate::Remount(Remount::new(app, "rows")))
    });
    let offset = |host: &HeadlessHost| {
        let id = host
            .app()
            .find_widget(Role::ScrollView, Some("Rows"))
            .unwrap();
        host.app().scene().scroll_offset(id).y
    };
    let id = host
        .app()
        .find_widget(Role::ScrollView, Some("Rows"))
        .unwrap();
    let r = host.app().scene().layout(id).unwrap().rect;
    let grab = Point {
        x: r.right() - 7.0,
        y: r.y + 8.0,
    };
    host.pointer_move(grab);
    host.press(PointerButton::Left);
    host.pointer_move(Point {
        x: grab.x,
        y: grab.y + 20.0,
    });
    let before = offset(&host);
    assert!(before > 0.0, "the thumb drag scrolled");
    dirty.set(true);
    host.frame();
    assert_eq!(host.remounts(), 1, "the remount does not wait for release");
    host.pointer_move(Point {
        x: grab.x,
        y: grab.y + 40.0,
    });
    assert!(
        offset(&host) > before,
        "the thumb still follows the pointer after the remount"
    );
    host.release(PointerButton::Left);
    host.frame();
}

fn board_view() -> impl View {
    Stack::new().fill().child(
        Dialog::new("Board")
            .modeless()
            .non_fixed()
            .movable()
            .at(40.0, 60.0)
            .size(240.0, 160.0)
            .child(Text::new("Notes")),
    )
}

#[test]
fn a_window_drag_survives_a_remount_under_the_held_button() {
    let dirty = Rc::new(Cell::new(false));
    let mut app = App::mount_with_size(board_view(), 640, 480);
    app.frame();
    let mut host = HeadlessHost::new(app);
    let d = dirty.clone();
    host.set_remount(move |size: Size, _| {
        if !d.replace(false) {
            return None;
        }
        let app = App::mount_with_size(board_view(), size.width as u32, size.height as u32);
        Some(WindowUpdate::Remount(Remount::new(app, "board")))
    });
    let rect = |host: &HeadlessHost| {
        let id = host.app().find_widget(Role::Dialog, Some("Board")).unwrap();
        host.app().scene().layout(id).unwrap().rect
    };
    let start = rect(&host);
    let grab = Point {
        x: start.x + 60.0,
        y: start.y + 12.0,
    };
    host.pointer_move(grab);
    host.press(PointerButton::Left);
    host.pointer_move(Point {
        x: grab.x + 50.0,
        y: grab.y + 20.0,
    });
    dirty.set(true);
    host.frame();
    assert_eq!(host.remounts(), 1, "the remount does not wait for release");
    host.pointer_move(Point {
        x: grab.x + 120.0,
        y: grab.y + 40.0,
    });
    host.release(PointerButton::Left);
    host.frame();
    let end = rect(&host);
    assert_eq!((end.x - start.x, end.y - start.y), (120.0, 40.0));
}

#[test]
fn a_text_selection_drag_survives_a_remount_under_the_held_button() {
    let dirty = Rc::new(Cell::new(false));
    let field = || TextInput::new("hello world").label("Note");
    let mut app = App::mount_with_size(field(), 400, 100);
    app.frame();
    let mut host = HeadlessHost::new(app);
    let d = dirty.clone();
    host.set_remount(move |size: Size, _| {
        if !d.replace(false) {
            return None;
        }
        let app = App::mount_with_size(field(), size.width as u32, size.height as u32);
        Some(WindowUpdate::Remount(Remount::new(app, "field")))
    });
    let at = center(&host, Role::TextInput, "Note");
    host.pointer_move(at);
    host.press(PointerButton::Left);
    assert!(host.pointer_state().contains("\"text_drag\":true"));
    dirty.set(true);
    host.frame();
    assert_eq!(host.remounts(), 1);
    assert!(
        host.pointer_state().contains("\"text_drag\":true"),
        "the selection drag continues in the new field: {}",
        host.pointer_state()
    );
    host.release(PointerButton::Left);
    assert!(host.pointer_state().contains("\"text_drag\":false"));
}

/// Design item 3: Escape on an open list during a held press ran the handler
/// twice. The key runs it once and the remount that follows does not repeat it.
#[test]
fn escape_on_an_open_list_during_a_drag_toggles_once() {
    let (mut host, state, seen) = host_with(false);
    grab_thumb(&mut host);
    focus(&mut host, Role::ComboBox, "Theme");
    host.key(&HostKey::Enter, false, false);
    assert!(state.get().open);
    let before = seen.toggles.get();
    host.key(&HostKey::Escape, false, false);
    assert!(!state.get().open, "Escape closes the list");
    assert_eq!(seen.toggles.get() - before, 1, "handler ran once");
    host.release(PointerButton::Left);
    host.frame();
    assert_eq!(seen.toggles.get() - before, 1, "release does not repeat it");
    assert!(!state.get().open);
}
