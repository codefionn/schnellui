//! A wheel turned while a dropdown or combo list is open. Over the list it
//! scrolls the list when the list overflows and otherwise does nothing: the
//! page beneath stays put and the list stays open. Anywhere else it closes the
//! list, picks nothing, and is spent doing so, as it is for a context menu.

use super::*;
use crate::widgets::probes::hover_disagrees;
use crate::windowed::headless::{HeadlessHost, HostKey, PointerButton};
use crate::{Remount, WindowUpdate};
use schnellui::a11y::{Role, StateFlags};
use schnellui::scene::{Point, Size, WidgetId};
use schnellui::widgets::{
    Button, Column, ComboBox, ComboBoxOption, Dropdown, DropdownOption, List, ListItem, Scroll,
    View,
};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Dropdown,
    Combo,
    CappedCombo,
}

#[derive(Clone, Copy)]
struct Choice {
    open: bool,
    picked: Option<usize>,
}

const NAMES: [&str; 3] = ["Compact", "Comfortable", "Spacious"];

fn view(kind: Kind, state: Rc<Cell<Choice>>, dirty: Rc<Cell<bool>>) -> impl View {
    let now = state.get();
    let pick = |i: usize| {
        let (state, dirty) = (state.clone(), dirty.clone());
        move || {
            state.set(Choice {
                open: false,
                picked: Some(i),
            });
            dirty.set(true);
        }
    };
    let toggle = {
        let (state, dirty) = (state.clone(), dirty.clone());
        move || {
            let now = state.get();
            state.set(Choice {
                open: !now.open,
                ..now
            });
            dirty.set(true);
        }
    };
    let control: Box<dyn View> = match kind {
        Kind::Dropdown => {
            let mut dropdown = Dropdown::new("Density").open(now.open).on_toggle(toggle);
            for (i, name) in NAMES.into_iter().enumerate() {
                dropdown = dropdown.option(DropdownOption::new(name).on_select(pick(i)));
            }
            Box::new(dropdown)
        }
        Kind::Combo | Kind::CappedCombo => {
            let mut combo = ComboBox::new("")
                .label("Density")
                .open(now.open)
                .on_toggle(toggle);
            let count = if kind == Kind::Combo { 3 } else { 30 };
            if kind == Kind::CappedCombo {
                combo = combo.max_popup_height(80.0);
            }
            for i in 0..count {
                combo = combo.option(ComboBoxOption::new(format!("Choice {i}")).on_select(pick(i)));
            }
            Box::new(combo)
        }
    };
    let mut rows = List::new();
    for i in 0..60 {
        rows = rows.child(ListItem::new(format!("Row {i}")));
    }
    Scroll::new().label("Page").size(640.0, 400.0).child(
        Column::new()
            .child(Button::new("Before"))
            .child(control)
            .child(rows),
    )
}

fn host(kind: Kind, open: bool) -> (HeadlessHost, Rc<Cell<Choice>>) {
    let state = Rc::new(Cell::new(Choice { open, picked: None }));
    let dirty = Rc::new(Cell::new(false));
    let (s, d) = (state.clone(), dirty.clone());
    let mut app = App::mount_with_size(view(kind, s, d), 640, 400);
    app.frame();
    let mut host = HeadlessHost::new(app);
    let (s, d) = (state.clone(), dirty);
    host.set_remount(move |size: Size, _| {
        if !d.replace(false) {
            return None;
        }
        let (s, d) = (s.clone(), d.clone());
        let app = App::mount_with_size(view(kind, s, d), size.width as u32, size.height as u32);
        Some(WindowUpdate::Remount(Remount::new(app, "dropdown")))
    });
    (host, state)
}

fn control(host: &HeadlessHost) -> WidgetId {
    host.app()
        .find_widget(Role::ComboBox, Some("Density"))
        .unwrap()
}

fn center_of(host: &HeadlessHost, id: WidgetId) -> Point {
    let r = host.app().scene().composited_rect(id);
    Point {
        x: r.x + r.width / 2.0,
        y: r.y + r.height / 2.0,
    }
}

fn is_open(host: &HeadlessHost) -> bool {
    StateFlags(host.app().scene().a11y(control(host)).unwrap().state).contains(StateFlags::EXPANDED)
}

fn group(host: &HeadlessHost, name: &str) -> WidgetId {
    host.app()
        .find_widget(Role::ScrollView, Some(name))
        .unwrap()
}

fn page(host: &HeadlessHost) -> f32 {
    host.app().scene().scroll_offset(group(host, "Page")).y
}

fn row_at(host: &HeadlessHost, name: &str) -> Point {
    let row = host.app().find_widget(Role::ListItem, Some(name)).unwrap();
    center_of(host, row)
}

/// Scrolls the page a little, then clicks the control open.
fn open_after_scrolling_the_page(kind: Kind) -> (HeadlessHost, Rc<Cell<Choice>>, f32) {
    let (mut host, state) = host(kind, false);
    host.wheel_at(Point { x: 300.0, y: 300.0 }, 40.0);
    let scrolled = page(&host);
    assert!(scrolled > 0.0, "the page scrolls with the list closed");
    let at = center_of(&host, control(&host));
    host.pointer_move(at);
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    host.frame();
    assert!(is_open(&host), "the click opened the list");
    assert_eq!(page(&host), scrolled);
    (host, state, scrolled)
}

fn first_option(host: &HeadlessHost) -> Point {
    let name = if host
        .app()
        .find_widget(Role::ListBoxOption, Some("Compact"))
        .is_some()
    {
        "Compact"
    } else {
        "Choice 0"
    };
    let id = host
        .app()
        .find_widget(Role::ListBoxOption, Some(name))
        .unwrap();
    center_of(host, id)
}

fn outside_closes(kind: Kind) {
    let (mut host, state, scrolled) = open_after_scrolling_the_page(kind);
    // Over a page row the list does not cover.
    let outside = row_at(&host, "Row 12");
    host.wheel_at(outside, 96.0);
    assert!(!is_open(&host), "{kind:?}: a wheel outside closes the list");
    assert!(state.get().picked.is_none(), "{kind:?}: nothing was picked");
    assert!(!state.get().open);
    assert_eq!(
        page(&host),
        scrolled,
        "{kind:?}: the wheel that closed the list does not also scroll the page"
    );
    assert_eq!(
        hover_disagrees(&host.app().widgets, host.app().scene(), outside, false),
        None,
        "{kind:?}: hover is on what is under the pointer"
    );
    // Focus is back on the trigger.
    assert_eq!(host.app().focused_widget(), Some(control(&host)));
    if kind == Kind::Dropdown {
        // A text field wears its ring for a click as well; a dropdown does not.
        assert!(
            !host.app().focus_ring_shown(),
            "opened by the pointer, no ring"
        );
    }
    // The next wheel scrolls the page.
    host.wheel(96.0);
    assert!(page(&host) > scrolled, "{kind:?}: the next wheel scrolls");
}

#[test]
fn a_wheel_outside_an_open_dropdown_closes_it_and_leaves_the_page() {
    outside_closes(Kind::Dropdown);
}

#[test]
fn a_wheel_outside_an_open_combo_closes_it_and_leaves_the_page() {
    outside_closes(Kind::Combo);
}

#[test]
fn a_wheel_outside_a_capped_combo_closes_it_and_leaves_the_page() {
    outside_closes(Kind::CappedCombo);
}

#[test]
fn a_wheel_closes_a_keyboard_opened_list_and_the_trigger_wears_the_ring() {
    for kind in [Kind::Dropdown, Kind::Combo] {
        let (mut host, state) = host(kind, false);
        let trigger = control(&host);
        assert!(host.app_mut().focus(Some(trigger)));
        host.key(&HostKey::Enter, false, false);
        host.frame();
        assert!(is_open(&host), "{kind:?}: Enter opened the list");
        host.wheel_at(row_at(&host, "Row 12"), 96.0);
        assert!(!is_open(&host), "{kind:?}");
        assert!(state.get().picked.is_none());
        assert_eq!(host.app().focused_widget(), Some(control(&host)));
        assert!(
            host.app().focus_ring_shown(),
            "{kind:?}: the keyboard opened it"
        );
    }
}

fn over_a_fitting_list_does_nothing(kind: Kind) {
    let (mut host, state, scrolled) = open_after_scrolling_the_page(kind);
    let over = first_option(&host);
    host.wheel_at(over, 96.0);
    assert!(is_open(&host), "{kind:?}: the list stays open");
    assert!(state.get().picked.is_none());
    assert_eq!(page(&host), scrolled, "{kind:?}: the page stays put");
    host.wheel(-96.0);
    assert!(is_open(&host));
    assert_eq!(page(&host), scrolled);
}

#[test]
fn a_wheel_over_a_fitting_dropdown_list_keeps_it_open_and_the_page_still() {
    over_a_fitting_list_does_nothing(Kind::Dropdown);
}

#[test]
fn a_wheel_over_a_fitting_combo_list_keeps_it_open_and_the_page_still() {
    over_a_fitting_list_does_nothing(Kind::Combo);
}

#[test]
fn a_wheel_over_an_overflowing_combo_list_scrolls_the_list_only() {
    let (mut host, _state, scrolled) = open_after_scrolling_the_page(Kind::CappedCombo);
    let list = group(&host, "Density suggestions");
    let over = first_option(&host);
    host.wheel_at(over, 40.0);
    assert!(is_open(&host), "the list stays open");
    assert!(
        host.app().scene().scroll_offset(list).y > 0.0,
        "the list scrolled"
    );
    assert_eq!(page(&host), scrolled, "the page did not");
    // At the end of the list the wheel is still the list's.
    host.wheel(5000.0);
    host.wheel(5000.0);
    assert!(is_open(&host));
    assert_eq!(page(&host), scrolled, "no chaining out of the popup");
}

#[test]
fn a_wheel_closes_a_context_menu_and_is_spent_on_it() {
    use schnellui::widgets::TextInput;
    let mut app = App::mount_with_size(
        Scroll::new().label("Page").size(640.0, 400.0).child(
            Column::new()
                .child(TextInput::new("").label("Name"))
                .child({
                    let mut rows = List::new();
                    for i in 0..60 {
                        rows = rows.child(ListItem::new(format!("Row {i}")));
                    }
                    rows
                }),
        ),
        640,
        400,
    );
    app.frame();
    let mut host = HeadlessHost::new(app);
    let field = host
        .app()
        .find_widget(Role::TextInput, Some("Name"))
        .unwrap();
    let at = center_of(&host, field);
    host.pointer_move(at);
    host.press(PointerButton::Right);
    host.release(PointerButton::Right);
    host.frame();
    assert!(host.app().context_menu_is_open());
    let before = page(&host);
    host.wheel_at(Point { x: 300.0, y: 300.0 }, 96.0);
    assert!(!host.app().context_menu_is_open());
    assert_eq!(page(&host), before, "the closing wheel does not scroll");
    host.wheel(96.0);
    assert!(page(&host) > before);
}

#[test]
fn a_wheel_that_slides_a_control_off_the_pointer_hides_its_tooltip() {
    use schnellui::scene::Primitive;
    let mut app = App::mount_with_size(
        Scroll::new().size(640.0, 400.0).child(
            Column::new()
                .child(Button::new("Header"))
                .child(Button::new("Save").tooltip("Saves the file"))
                .child({
                    let mut rows = List::new();
                    for i in 0..60 {
                        rows = rows.child(ListItem::new(format!("Row {i}")));
                    }
                    rows
                }),
        ),
        640,
        400,
    );
    app.frame();
    let mut host = HeadlessHost::new(app);
    let button = host.app().find_widget(Role::Button, Some("Save")).unwrap();
    let glyphs = |host: &HeadlessHost| {
        host.app().scene().paint(button).map_or(0, |p| {
            p.primitives
                .iter()
                .filter(|p| matches!(p, Primitive::GlyphQuad { color, .. } if color.a != 0))
                .count()
        })
    };
    let label_only = glyphs(&host);
    // Near the button's lower edge; its tooltip sits above.
    let rect = host.app().scene().composited_rect(button);
    let at = Point {
        x: rect.x + rect.width / 2.0,
        y: rect.y + rect.height - 3.0,
    };
    host.pointer_move(Point {
        x: at.x - 1.0,
        y: at.y,
    });
    host.pointer_move(at);
    host.advance(std::time::Duration::from_secs(2));
    host.frame();
    assert!(glyphs(&host) > label_only, "the tooltip came up");
    // The page moves up 12 px: a row is under the pointer now.
    host.wheel(12.0);
    host.frame();
    assert_ne!(host.app().hit_test(at), Some(button));
    eprintln!(
        "rect {:?} at {:?}",
        host.app().scene().composited_rect(button),
        at
    );
    assert_eq!(glyphs(&host), label_only, "a wheel hides it");
}
