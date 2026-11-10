//! Keyboard focus and the text caret scroll every enclosing `Scroll` the least
//! distance that shows them, as a browser does. Pointer focus does not scroll,
//! because the target is already under the pointer.

use super::*;
use schnellui::a11y::Role;
use schnellui::scene::{Rect, WidgetId};
use schnellui::widgets::{
    Button, Column, List, ListItem, Radio, Scroll, Tab, TabBar, Table, TextArea, TextInput,
};

fn offset(app: &App, scroll: WidgetId) -> f32 {
    app.scene().scroll_offset(scroll).y
}

/// Whether `id`'s laid-out rect lies inside `scroll`'s visible window.
fn visible_in(app: &App, scroll: WidgetId, id: WidgetId) -> bool {
    let viewport = app.scene().layout(scroll).unwrap().rect;
    let rect = app.scene().layout(id).unwrap().rect;
    let top = viewport.y + offset(app, scroll);
    rect.y >= top - 0.5 && rect.bottom() <= top + viewport.height + 0.5
}

fn buttons(n: usize) -> Column {
    let mut col = Column::new().gap(8.0);
    for i in 0..n {
        col = col.child(Button::new(format!("Button {i}")));
    }
    col
}

fn button(app: &App, i: usize) -> WidgetId {
    let name = format!("Button {i}");
    app.find_widget(Role::Button, Some(name.as_str())).unwrap()
}

fn scroll_of(app: &App) -> WidgetId {
    app.find_widget(Role::ScrollView, None).unwrap()
}

fn caret_area_rect(app: &App, id: WidgetId) -> Rect {
    app.scene().layout(id).unwrap().rect
}

#[test]
fn tab_through_a_form_taller_than_its_viewport_keeps_the_focus_visible() {
    let mut app = App::mount_with_size(
        Scroll::new().size(300.0, 120.0).child(buttons(20)),
        400,
        300,
    );
    app.frame();
    let sv = scroll_of(&app);
    for i in 0..20 {
        assert!(app.dispatch_key(UiKey::Tab { shift: false }));
        assert_eq!(app.focused_widget(), Some(button(&app, i)));
        assert!(visible_in(&app, sv, button(&app, i)), "button {i} visible");
    }
    // Wrapping back to the first scrolls back up; Shift+Tab walks down again.
    assert!(app.dispatch_key(UiKey::Tab { shift: false }));
    assert_eq!(offset(&app, sv), 0.0);
    assert!(app.dispatch_key(UiKey::Tab { shift: true }));
    assert!(visible_in(&app, sv, button(&app, 19)));
    assert!(offset(&app, sv) > 0.0);
}

#[test]
fn a_visible_focus_target_does_not_move_the_viewport() {
    let mut app =
        App::mount_with_size(Scroll::new().size(300.0, 300.0).child(buttons(3)), 400, 400);
    app.frame();
    let sv = scroll_of(&app);
    assert!(app.focus(Some(button(&app, 2))));
    assert_eq!(offset(&app, sv), 0.0);
}

#[test]
fn programmatic_focus_and_the_accesskit_focus_action_scroll_to_the_control() {
    let mut app = App::mount_with_size(
        Scroll::new().size(300.0, 120.0).child(buttons(20)),
        400,
        300,
    );
    app.frame();
    let sv = scroll_of(&app);
    let last = button(&app, 19);
    assert!(app.focus(Some(last)));
    assert!(visible_in(&app, sv, last));
    assert!(app.focus(Some(button(&app, 0))));
    assert_eq!(offset(&app, sv), 0.0);

    let request = |action, target| accesskit_action::ActionRequest {
        action,
        target_tree: accesskit_reexport::TreeId::ROOT,
        target_node: schnellui::a11y::to_access_id(target),
        data: None,
    };
    let mid = button(&app, 12);
    assert!(app.dispatch_action(&request(accesskit_action::Action::Focus, mid)));
    assert!(visible_in(&app, sv, mid));
}

#[test]
fn the_accesskit_scroll_into_view_action_reveals_any_node_without_clicking_it() {
    use std::cell::Cell;
    use std::rc::Rc;
    let clicks = Rc::new(Cell::new(0));
    let counted = clicks.clone();
    let mut col = Column::new().gap(8.0);
    for i in 0..20 {
        col = if i == 15 {
            let counted = counted.clone();
            col.child(Button::new("Target").on_click(move || counted.set(counted.get() + 1)))
        } else {
            col.child(Button::new(format!("Button {i}")))
        };
    }
    let mut app = App::mount_with_size(Scroll::new().size(300.0, 120.0).child(col), 400, 300);
    app.frame();
    let sv = scroll_of(&app);
    let target = app.find_widget(Role::Button, Some("Target")).unwrap();
    assert!(!visible_in(&app, sv, target));
    let request = accesskit_action::ActionRequest {
        action: accesskit_action::Action::ScrollIntoView,
        target_tree: accesskit_reexport::TreeId::ROOT,
        target_node: schnellui::a11y::to_access_id(target),
        data: None,
    };
    assert!(app.dispatch_action(&request));
    assert!(visible_in(&app, sv, target));
    assert_eq!(clicks.get(), 0, "revealing is not activating");
    assert_eq!(app.focused_widget(), None, "revealing is not focusing");
}

#[test]
fn nested_scrolls_are_revealed_innermost_first() {
    let inner = Scroll::new()
        .label("inner")
        .size(260.0, 100.0)
        .child(buttons(12));
    let outer = Scroll::new().label("outer").size(300.0, 150.0).child(
        Column::new()
            .child(Button::new("Before"))
            .child(inner)
            .child(buttons(10)),
    );
    let mut app = App::mount_with_size(outer, 400, 400);
    app.frame();
    let inner_id = app.find_widget(Role::ScrollView, Some("inner")).unwrap();
    let outer_id = app.find_widget(Role::ScrollView, Some("outer")).unwrap();
    let deep = button(&app, 11);
    assert!(app.focus(Some(deep)));
    assert!(offset(&app, inner_id) > 0.0, "inner scrolled");
    // The inner window sits inside the outer one: the control is on screen.
    let shift = offset(&app, inner_id);
    let viewport = app.scene().layout(outer_id).unwrap().rect;
    let rect = app.scene().layout(deep).unwrap().rect;
    let top = viewport.y + offset(&app, outer_id);
    assert!(rect.y - shift >= top - 0.5);
    assert!(rect.bottom() - shift <= top + viewport.height + 0.5);
}

#[test]
fn an_inner_scroll_that_is_already_scrolled_is_revealed_where_it_now_shows_the_control() {
    let inner = Scroll::new()
        .label("inner")
        .size(260.0, 100.0)
        .child(buttons(12));
    let outer = Scroll::new().label("outer").size(300.0, 90.0).child(
        Column::new()
            .child(Button::new("Before"))
            .child(inner)
            .child(buttons(10)),
    );
    let mut app = App::mount_with_size(outer, 400, 400);
    app.frame();
    let inner_id = app.find_widget(Role::ScrollView, Some("inner")).unwrap();
    let outer_id = app.find_widget(Role::ScrollView, Some("outer")).unwrap();
    // Down at the end of the inner list, with the outer scrolled to show it.
    assert!(app.focus(Some(button(&app, 11))));
    assert!(offset(&app, inner_id) > 0.0 && offset(&app, outer_id) > 0.0);
    // Back up to a row near the start of the inner list: the inner scroll
    // moves up to it, and the outer one has to follow to where the inner
    // window now shows it, which is above what the outer window shows.
    let target = button(&app, 6);
    assert!(app.focus(Some(target)));
    let drawn = app.scene().composited_rect(target);
    let laid = app.scene().layout(target).unwrap().rect;
    assert!(
        (drawn.height - laid.height).abs() < 0.5,
        "the control is drawn whole at {drawn:?}, laid out as {laid:?}"
    );
}

#[test]
fn arrowing_down_a_list_in_a_scroll_follows_the_focused_row() {
    let mut list = List::new();
    for i in 0..30 {
        list = list.child(ListItem::new(format!("Item {i}")));
    }
    let mut app = App::mount_with_size(Scroll::new().size(300.0, 120.0).child(list), 400, 300);
    app.frame();
    let sv = scroll_of(&app);
    assert!(app.dispatch_key(UiKey::Tab { shift: false }));
    for _ in 0..29 {
        assert!(app.dispatch_key(UiKey::Down { shift: false }));
        let focused = app.focused_widget().unwrap();
        assert!(visible_in(&app, sv, focused), "focused row stays visible");
    }
    let last = app.focused_widget().unwrap();
    assert!(offset(&app, sv) > 0.0);
    assert!(app.dispatch_key(UiKey::Home {
        shift: false,
        ctrl: false
    }));
    assert!(visible_in(&app, sv, app.focused_widget().unwrap()));
    assert_ne!(app.focused_widget(), Some(last));
    assert_eq!(offset(&app, sv), 0.0);
}

#[test]
fn arrowing_down_a_table_in_a_scroll_follows_the_focus() {
    let mut table = Table::new().columns(["A", "B"]).selected_row(0);
    for i in 0..30 {
        table = table.row([format!("r{i}"), format!("c{i}")]);
    }
    let mut app = App::mount_with_size(Scroll::new().size(300.0, 120.0).child(table), 400, 300);
    app.frame();
    let sv = scroll_of(&app);
    assert!(app.dispatch_key(UiKey::Tab { shift: false }));
    for _ in 0..25 {
        app.dispatch_key(UiKey::Down { shift: false });
        assert!(visible_in(&app, sv, app.focused_widget().unwrap()));
    }
    assert!(offset(&app, sv) > 0.0);
}

#[test]
fn arrowing_through_radios_in_a_scroll_follows_the_focus() {
    let mut radios = Column::new().child(Column::new().gap(40.0));
    let mut group = Column::new().gap(30.0);
    for i in 0..8 {
        group = group.child(Radio::new(i == 0).name(format!("Opt {i}")).row(40.0));
    }
    radios = radios.child(group);
    let mut app = App::mount_with_size(Scroll::new().size(300.0, 120.0).child(radios), 400, 300);
    app.frame();
    let sv = scroll_of(&app);
    assert!(app.dispatch_key(UiKey::Tab { shift: false }));
    for _ in 0..6 {
        app.dispatch_key(UiKey::Down { shift: false });
        assert!(visible_in(&app, sv, app.focused_widget().unwrap()));
    }
    assert!(offset(&app, sv) > 0.0);
}

#[test]
fn tabbing_to_a_tab_bar_below_the_fold_scrolls_to_it() {
    let mut bar = TabBar::new();
    for i in 0..3 {
        bar = bar.child(Tab::new(format!("Tab {i}")));
    }
    let mut app = App::mount_with_size(
        Scroll::new()
            .size(300.0, 60.0)
            .child(Column::new().child(buttons(8)).child(bar)),
        400,
        300,
    );
    app.frame();
    let sv = scroll_of(&app);
    for _ in 0..9 {
        assert!(app.dispatch_key(UiKey::Tab { shift: false }));
        assert!(visible_in(&app, sv, app.focused_widget().unwrap()));
    }
}

#[test]
fn pointer_focus_does_not_scroll() {
    let mut app = App::mount_with_size(
        Scroll::new().size(300.0, 120.0).child(buttons(20)),
        400,
        300,
    );
    app.frame();
    let sv = scroll_of(&app);
    // A control half under the bottom edge: a click focuses it in place.
    let target = (0..20)
        .map(|i| button(&app, i))
        .find(|&b| !visible_in(&app, sv, b) && app.scene().layout(b).unwrap().rect.y < 120.0)
        .expect("a partly visible button");
    assert!(app.pointer_focus(Some(target)));
    assert_eq!(offset(&app, sv), 0.0);
}

fn area_text(lines: usize) -> String {
    (0..lines)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn caret_visible(app: &App, sv: WidgetId, area: WidgetId, row: usize, rows: usize) -> bool {
    let rect = caret_area_rect(app, area);
    let viewport = app.scene().layout(sv).unwrap().rect;
    // The first row sits under the area's top padding, the last above its
    // bottom padding; the fixed 12px padding and a ~20px line is enough for
    // the check to be about the row, not the whole box.
    let line_h = (rect.height - 24.0) / rows as f32;
    let top = rect.y + 12.0 + row as f32 * line_h;
    let visible_top = viewport.y + offset(app, sv);
    top >= visible_top - 0.5 && top + line_h <= visible_top + viewport.height + 0.5
}

#[test]
fn typing_and_arrowing_past_the_viewport_follow_the_caret_in_a_text_area() {
    let mut app = App::mount_with_size(
        Scroll::new()
            .size(300.0, 120.0)
            .child(TextArea::new(String::new())),
        400,
        300,
    );
    app.frame();
    let sv = scroll_of(&app);
    let area = app.find_widget(Role::MultilineTextInput, None).unwrap();
    assert!(app.focus(Some(area)));
    let n = 12;
    for i in 0..n {
        assert!(app.dispatch_key(UiKey::Char("x")));
        assert!(app.dispatch_key(UiKey::Enter));
        app.frame();
        assert!(caret_visible(&app, sv, area, i + 1, i + 2), "typed row {i}");
    }
    assert!(offset(&app, sv) > 0.0);
    // Up walks the caret back to the top; the viewport follows it.
    for r in (0..n).rev() {
        assert!(app.dispatch_key(UiKey::Up { shift: false }));
        app.frame();
        assert!(caret_visible(&app, sv, area, r, n + 1), "up to row {r}");
    }
    assert_eq!(offset(&app, sv), 0.0);
    for r in 1..=n {
        assert!(app.dispatch_key(UiKey::Down { shift: false }));
        app.frame();
        assert!(caret_visible(&app, sv, area, r, n + 1), "down to row {r}");
    }
}

#[test]
fn paste_in_a_text_area_follows_the_caret() {
    let mut app = App::mount_with_size(
        Scroll::new()
            .size(300.0, 120.0)
            .child(TextArea::new(String::new())),
        400,
        300,
    );
    app.frame();
    let sv = scroll_of(&app);
    let area = app.find_widget(Role::MultilineTextInput, None).unwrap();
    assert!(app.focus(Some(area)));
    assert!(app.paste_text(&area_text(30)));
    app.frame();
    assert!(
        caret_visible(&app, sv, area, 29, 30),
        "paste lands the caret in view"
    );
    assert!(offset(&app, sv) > 0.0);
}

#[test]
fn the_caret_of_a_single_line_input_in_a_scroll_is_revealed_by_typing() {
    let mut app = App::mount_with_size(
        Scroll::new()
            .size(300.0, 120.0)
            .child(Column::new().child(buttons(12)).child(TextInput::new(""))),
        400,
        300,
    );
    app.frame();
    let sv = scroll_of(&app);
    let input = app.find_widget(Role::TextInput, None).unwrap();
    // Focus by the pointer leaves the viewport; typing then brings it in.
    assert!(app.pointer_focus(Some(input)));
    assert_eq!(offset(&app, sv), 0.0);
    assert!(app.dispatch_key(UiKey::Char("x")));
    app.frame();
    assert!(visible_in(&app, sv, input));
}

#[test]
fn a_remount_keeps_the_restored_scroll_offset() {
    let build = || {
        Scroll::new()
            .label("page")
            .restoration_key("page")
            .size(300.0, 120.0)
            .child(buttons(20))
    };
    let mut app = App::mount_with_size(build(), 400, 300);
    app.frame();
    let sv = scroll_of(&app);
    assert!(app.dispatch_key(UiKey::Tab { shift: false }));
    assert!(app.dispatch_scroll(sv, 80.0));
    let at = offset(&app, sv);
    let mut replacement = App::mount_with_size(build(), 400, 300);
    replacement.inherit_remount_state(&app);
    replacement.frame();
    assert_eq!(offset(&replacement, scroll_of(&replacement)), at);
}

/// The outermost edge of the keyboard ring on `id`, as a rect.
fn ring_rect(app: &App, id: WidgetId) -> Option<Rect> {
    app.scene()
        .paint(id)?
        .primitives
        .iter()
        .rev()
        .find_map(|p| match *p {
            schnellui::scene::Primitive::StrokeRect { rect, width, .. }
                if width == schnellui::widgets::FOCUS_RING_W =>
            {
                Some(rect)
            }
            _ => None,
        })
}

/// The ring may poke out of the content itself, where the first and last
/// control sit at its ends and there is nothing further to scroll to; every
/// other ring has to lie inside the window.
fn assert_ring_shown(app: &App, sv: WidgetId, i: usize, what: &str) {
    let viewport = app.scene().layout(sv).unwrap().rect;
    let ring = ring_rect(app, button(app, i)).expect("ring");
    let top = viewport.y + offset(app, sv);
    if offset(app, sv) > 0.0 {
        assert!(
            ring.y >= top - 0.01,
            "{what} {i}: ring top {} < {top}",
            ring.y
        );
    }
    if i < 19 {
        let bottom = top + viewport.height;
        assert!(
            ring.bottom() <= bottom + 0.01,
            "{what} {i}: ring bottom {} past {bottom}",
            ring.bottom()
        );
    }
}

#[test]
fn tab_to_a_control_at_the_viewport_edge_shows_its_whole_ring() {
    let mut app = App::mount_with_size(
        Scroll::new().size(300.0, 120.0).child(buttons(20)),
        400,
        300,
    );
    app.frame();
    let sv = scroll_of(&app);
    for i in 0..20 {
        assert!(app.dispatch_key(UiKey::Tab { shift: false }));
        assert_ring_shown(&app, sv, i, "button");
    }
    for i in (0..19).rev() {
        assert!(app.dispatch_key(UiKey::Tab { shift: true }));
        assert_ring_shown(&app, sv, i, "back");
    }
}

fn rows_viewport(scrollbar: bool) -> Scroll {
    let mut col = Column::new().gap(2.0);
    for i in 0..40 {
        col = col.child(schnellui::widgets::Text::new(format!("Row {i}")));
    }
    Scroll::new()
        .label("Rows")
        .scrollbar(scrollbar)
        .size(200.0, 100.0)
        .child(col)
}

/// A viewport with nothing focusable inside is a Tab stop. Scrolling it, by
/// the keys it now answers to, repainted its scrollbar chrome over the ring.
#[test]
fn a_focused_scroll_view_keeps_its_ring_while_the_keyboard_scrolls_it() {
    for scrollbar in [false, true] {
        let mut app = App::mount_with_size(rows_viewport(scrollbar), 400, 300);
        app.frame();
        let sv = scroll_of(&app);
        assert!(app.dispatch_key(UiKey::Tab { shift: false }));
        assert_eq!(app.focused_widget(), Some(sv));
        let viewport = app.scene().layout(sv).unwrap().rect;
        let ring = |app: &App| ring_rect(app, sv);
        let at_rest = ring(&app).unwrap_or_else(|| panic!("no ring (scrollbar {scrollbar})"));
        assert!(app.focus_ring_shown());
        for _ in 0..3 {
            assert!(app.dispatch_key(UiKey::Down { shift: false }));
            app.frame();
            assert!(offset(&app, sv) > 0.0);
            assert!(app.focus_ring_shown(), "scrollbar {scrollbar}");
            // Fixed to the viewport frame, inside it, whatever the offset.
            assert_eq!(ring(&app), Some(at_rest), "scrollbar {scrollbar}");
            assert!(at_rest.x >= viewport.x && at_rest.right() <= viewport.right());
            assert!(at_rest.y >= viewport.y && at_rest.bottom() <= viewport.bottom());
        }
        // The ring and AccessKit agree.
        let access = app.scene().a11y(sv).unwrap();
        assert!(schnellui::a11y::StateFlags(access.state)
            .contains(schnellui::a11y::StateFlags::FOCUSED));
        // Blurring takes the ring, and the scrollbar chrome stays.
        assert!(app.dispatch_key(UiKey::Tab { shift: false }) || app.focus(None));
        assert!(ring(&app).is_none(), "scrollbar {scrollbar}");
        if scrollbar {
            assert_eq!(app.scene().paint(sv).unwrap().primitives.len(), 2);
        }
    }
}
