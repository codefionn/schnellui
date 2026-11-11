//! Keys typed while a slider drag holds the pointer. Structural remounts wait
//! for release, so a key that opens a dropdown cannot show its list yet; the
//! keys that follow must still act on it, in order, once it lands.

use super::*;
use crate::windowed::headless::{HeadlessHost, HostKey, PointerButton};
use crate::{Remount, WindowUpdate};
use accesskit::{Action, TreeId};
use schnellui::a11y::{to_access_id, Role, StateFlags};
use schnellui::scene::{Point, Size};
use schnellui::widgets::{Column, Dropdown, DropdownOption, Slider, View};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy)]
struct Choice {
    open: bool,
    chosen: usize,
}

const NAMES: [&str; 3] = ["Light", "Dark", "Auto"];

fn view(state: Rc<Cell<Choice>>, dirty: Rc<Cell<bool>>) -> impl View {
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
    dropdown = dropdown.on_toggle(move || {
        let now = toggle_state.get();
        toggle_state.set(Choice {
            open: !now.open,
            ..now
        });
        toggle_dirty.set(true);
    });
    Column::new()
        .child(Slider::new(40.0, 0.0, 100.0).name("Volume"))
        .child(dropdown)
}

fn host() -> (HeadlessHost, Rc<Cell<Choice>>) {
    let state = Rc::new(Cell::new(Choice {
        open: false,
        chosen: 0,
    }));
    let dirty = Rc::new(Cell::new(false));
    let mut app = App::mount_with_size(view(state.clone(), dirty.clone()), 640, 400);
    app.frame();
    let mut host = HeadlessHost::new(app);
    let (s, d) = (state.clone(), dirty);
    host.set_remount(move |size: Size, _| {
        if !d.replace(false) {
            return None;
        }
        let app = App::mount_with_size(
            view(s.clone(), d.clone()),
            size.width as u32,
            size.height as u32,
        );
        Some(WindowUpdate::Remount(Remount::new(app, "dropdown")))
    });
    (host, state)
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
    let (mut host, state) = host();
    let thumb = center(&host, Role::Slider, "Volume");
    host.pointer_move(thumb);
    host.press(PointerButton::Left);
    host.pointer_move(Point {
        x: thumb.x + 17.0,
        y: thumb.y,
    });
    let combo = host
        .app()
        .find_widget(Role::ComboBox, Some("Theme"))
        .unwrap();
    host.dispatch_action(&accesskit::ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(combo),
        data: None,
    });
    for k in [HostKey::Enter, HostKey::Down, HostKey::Enter] {
        host.key(&k, false, false);
    }
    host.release(PointerButton::Left);
    host.frame();
    assert_eq!(
        state.get().chosen,
        1,
        "Enter, Down, Enter picks the next option"
    );
    assert!(!state.get().open);
    assert!(!expanded(&host));
}
