//! A pointer press on a dropdown trigger focuses it, and the open-remount that
//! follows keeps that focus, so the keyboard keeps working: Down and Enter
//! pick, Escape closes and returns to the trigger, Tab continues from it.

use super::*;
use crate::windowed::headless::{HeadlessHost, HostKey, PointerButton, PointerRoute};
use crate::{Remount, WindowUpdate};
use schnellui::a11y::{Role, StateFlags};
use schnellui::scene::{Point, Size, WidgetId};
use schnellui::widgets::{Button, Column, Dropdown, DropdownOption, View};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy)]
struct Choice {
    open: bool,
    chosen: usize,
}

const NAMES: [&str; 3] = ["Compact", "Comfortable", "Spacious"];

fn view(state: Rc<Cell<Choice>>, dirty: Rc<Cell<bool>>) -> impl View {
    let now = state.get();
    let mut dropdown = Dropdown::new("Density").open(now.open);
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
        .child(Button::new("Before"))
        .child(dropdown)
        .child(Button::new("After"))
}

fn host() -> (HeadlessHost, Rc<Cell<Choice>>) {
    let state = Rc::new(Cell::new(Choice {
        open: false,
        chosen: 0,
    }));
    let dirty = Rc::new(Cell::new(false));
    let (s, d) = (state.clone(), dirty.clone());
    let mut app = App::mount_with_size(view(s, d), 640, 400);
    app.frame();
    let mut host = HeadlessHost::new(app);
    let (s, d) = (state.clone(), dirty);
    host.set_remount(move |size: Size, _| {
        if !d.replace(false) {
            return None;
        }
        let (s, d) = (s.clone(), d.clone());
        let app = App::mount_with_size(view(s, d), size.width as u32, size.height as u32);
        Some(WindowUpdate::Remount(Box::new(Remount::new(
            app, "dropdown",
        ))))
    });
    (host, state)
}

fn find(host: &HeadlessHost, role: Role, name: &str) -> WidgetId {
    host.app().find_widget(role, Some(name)).unwrap()
}

fn center(host: &HeadlessHost, id: WidgetId) -> Point {
    let r = host.app().scene().layout(id).unwrap().rect;
    Point {
        x: r.x + r.width / 2.0,
        y: r.y + r.height / 2.0,
    }
}

fn focused(host: &HeadlessHost) -> Option<String> {
    let id = host.app().focused_widget()?;
    host.app().scene().a11y(id)?.name.clone()
}

fn expanded(host: &HeadlessHost) -> bool {
    let t = find(host, Role::ComboBox, "Density");
    StateFlags(host.app().scene().a11y(t).unwrap().state).contains(StateFlags::EXPANDED)
}

fn click_trigger(host: &mut HeadlessHost) {
    let at = center(host, find(host, Role::ComboBox, "Density"));
    host.pointer_move(at);
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    host.frame();
}

fn key(host: &mut HeadlessHost, k: HostKey) {
    host.key(&k, false, false);
    host.frame();
}

#[test]
fn a_pointer_click_on_a_dropdown_trigger_focuses_it_across_the_open_remount() {
    let (mut host, _) = host();
    click_trigger(&mut host);
    assert!(expanded(&host), "the click opens the list");
    assert_eq!(focused(&host).as_deref(), Some("Density"));
}

#[test]
fn keyboard_after_a_pointer_open_picks_closes_and_tabs_on() {
    let (mut host, state) = host();
    click_trigger(&mut host);
    key(&mut host, HostKey::Down);
    key(&mut host, HostKey::Enter);
    assert_eq!(
        state.get().chosen,
        1,
        "Down then Enter picks the next option"
    );
    assert!(!expanded(&host));
    assert_eq!(focused(&host).as_deref(), Some("Density"));
    key(&mut host, HostKey::Tab);
    assert_eq!(focused(&host).as_deref(), Some("After"));

    // Escape closes and keeps the trigger focused; Tab continues from it.
    let (mut host, state) = self::host();
    click_trigger(&mut host);
    key(&mut host, HostKey::Escape);
    assert!(!state.get().open);
    assert!(!expanded(&host));
    assert_eq!(focused(&host).as_deref(), Some("Density"));
    key(&mut host, HostKey::Tab);
    assert_eq!(focused(&host).as_deref(), Some("After"));
}

#[test]
fn escape_closes_a_pointer_opened_list_with_the_ring_on_the_trigger() {
    let (mut host, _) = host();
    click_trigger(&mut host);
    assert!(!host.app().focus_ring_shown(), "the click shows no ring");
    key(&mut host, HostKey::Escape);
    assert_eq!(focused(&host).as_deref(), Some("Density"));
    assert!(host.app().focus_ring_shown(), "Escape shows the ring");
}

#[test]
fn clicking_an_option_closes_the_list_without_a_ring_on_the_trigger() {
    let (mut host, _) = host();
    click_trigger(&mut host);
    key(&mut host, HostKey::Down);
    let option = find(&host, Role::ListBoxOption, "Comfortable");
    let at = center(&host, option);
    host.pointer_move(at);
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    host.frame();
    assert_eq!(focused(&host).as_deref(), Some("Density"));
    assert!(
        !host.app().focus_ring_shown(),
        "a pointer pick shows no ring"
    );
}

#[test]
fn the_debug_route_click_focuses_the_trigger_too() {
    let (mut host, _) = host();
    host.set_route(PointerRoute::Debug);
    let at = center(&host, find(&host, Role::ComboBox, "Density"));
    host.click_at(at);
    host.frame();
    assert!(expanded(&host));
    assert_eq!(focused(&host).as_deref(), Some("Density"));
}
