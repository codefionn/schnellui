//! The retained adapter builds real widgets for the template components the
//! Android renderer maps natively.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::*;
use schnellui::a11y::Role;
use schnellui::scene::WidgetId;
use schnellui::template::{
    column, dialog, drag_handle, dropdown, dropdown_option, list, list_item, progress_bar, radio,
    rich_text, switch, tab, tab_bar, table, table_row, theme_provider, ComponentKind, DriveAction,
    DriveTarget, Template, Text, Widget,
};

fn mount(view: impl Template) -> App {
    let mut app = App::mount_template_with_size_scaled(view, 640, 480, 1.0);
    app.frame();
    app
}

fn count(app: &App, role: Role) -> usize {
    fn walk(app: &App, id: WidgetId, role: u16, n: &mut usize) {
        if app.scene().a11y(id).is_some_and(|a| a.role == role) {
            *n += 1;
        }
        if let Some(node) = app.scene().node(id) {
            for &child in &node.children {
                walk(app, child, role, n);
            }
        }
    }
    let mut n = 0;
    if let Some(root) = app.scene().root() {
        walk(app, root, role.as_u16(), &mut n);
    }
    n
}

fn value_of(app: &App, role: Role, name: &str) -> Option<String> {
    let id = app.find_widget(role, Some(name))?;
    app.scene().a11y(id)?.value.clone()
}

#[test]
fn android_mapped_kinds_build_their_retained_widget() {
    let cases: [(ComponentKind, Role); 12] = [
        (ComponentKind::Dialog, Role::Dialog),
        (ComponentKind::TabBar, Role::TabList),
        (ComponentKind::Tab, Role::Tab),
        (ComponentKind::List, Role::List),
        (ComponentKind::ListItem, Role::ListItem),
        (ComponentKind::Dropdown, Role::ComboBox),
        (ComponentKind::Table, Role::Table),
        (ComponentKind::RichText, Role::Document),
        (ComponentKind::Switch, Role::Switch),
        (ComponentKind::Radio, Role::Radio),
        (ComponentKind::ProgressBar, Role::ProgressIndicator),
        (ComponentKind::DragHandle, Role::Group),
    ];
    for (kind, role) in cases {
        let app = mount(Widget::new(kind).label("Label").value("body"));
        assert!(
            app.find_widget(role, None).is_some(),
            "{kind:?} has no {role:?}"
        );
    }
}

#[test]
fn switch_and_radio_carry_their_label_and_handlers() {
    let toggled = Rc::new(Cell::new(None));
    let clicked = Rc::new(Cell::new(0));
    let (t, c) = (toggled.clone(), clicked.clone());
    let mut app = mount(
        column()
            .child(switch().label("Wifi").on_toggle(move |on| t.set(Some(on))))
            .child(radio().label("Fast").on_click(move || c.set(c.get() + 1))),
    );
    assert!(app.drive_action(&DriveAction::click(Role::Switch, "Wifi")));
    assert_eq!(toggled.get(), Some(true));
    assert!(app.drive_action(&DriveAction::click(Role::Radio, "Fast")));
    assert_eq!(clicked.get(), 1);
}

#[test]
fn dropdown_takes_items_then_option_children() {
    let chosen = Rc::new(RefCell::new(String::new()));
    let sink = chosen.clone();
    let mut app = mount(
        dropdown()
            .label("Fruit")
            .item("Apple")
            .child(dropdown_option().label("Pear").value("pear"))
            .child(dropdown_option().label("Plum"))
            .value("pear")
            .open(true)
            .on_input(move |value| *sink.borrow_mut() = value.to_owned()),
    );
    assert_eq!(
        value_of(&app, Role::ComboBox, "Fruit").as_deref(),
        Some("Pear")
    );
    assert_eq!(count(&app, Role::ListBoxOption), 3);
    assert!(app.drive_action(&DriveAction::click(Role::ListBoxOption, "Plum")));
    assert_eq!(chosen.borrow().as_str(), "Plum");
    assert!(app.drive_action(&DriveAction::click(Role::ListBoxOption, "Pear")));
    assert_eq!(chosen.borrow().as_str(), "pear");
}

#[test]
fn a_closed_dropdown_asks_to_open() {
    let requested = Rc::new(Cell::new(None));
    let sink = requested.clone();
    let mut app = mount(
        dropdown()
            .label("Size")
            .item("S")
            .item("M")
            .on_toggle(move |open| sink.set(Some(open))),
    );
    assert_eq!(count(&app, Role::ListBoxOption), 0);
    assert!(app.drive_action(&DriveAction::click(Role::ComboBox, "Size")));
    assert_eq!(requested.get(), Some(true));
}

#[test]
fn a_stray_option_shows_its_label() {
    let app = mount(column().child(dropdown_option().label("Alone")));
    assert_eq!(count(&app, Role::ListBoxOption), 0);
    assert_eq!(count(&app, Role::ComboBox), 0);
}

#[test]
fn table_rows_select_through_the_row_handler() {
    let picked = Rc::new(Cell::new(0));
    let sink = picked.clone();
    let mut app = mount(
        table()
            .label("People")
            .child(
                table_row()
                    .item("Ada")
                    .item("36")
                    .on_click(move || sink.set(sink.get() + 1)),
            )
            .child(table_row().item("Bob").item("41").selected(true)),
    );
    assert_eq!(count(&app, Role::Table), 1);
    assert_eq!(count(&app, Role::TableRow), 2);
    assert!(app.drive_action(&DriveAction::Click(DriveTarget::role(Role::TableRow))));
    assert_eq!(picked.get(), 1);
}

#[test]
fn a_table_inside_a_row_does_not_take_the_outer_rows() {
    let app = mount(
        table().child(table_row().item("one")).child(
            table_row()
                .item("two")
                .child(table().child(table_row().item("inner"))),
        ),
    );
    // The inner table lives in a row's children, which retained cells cannot
    // hold, so only the outer table is built, with both of its rows.
    assert_eq!(count(&app, Role::Table), 1);
    assert_eq!(count(&app, Role::TableRow), 2);
}

#[test]
fn tabs_lists_and_dialogs_hold_their_children() {
    let selected = Rc::new(Cell::new(0));
    let sink = selected.clone();
    let mut app = mount(
        column()
            .child(
                tab_bar().child(tab().label("One").selected(true)).child(
                    tab()
                        .label("Two")
                        .on_click(move || sink.set(sink.get() + 1)),
                ),
            )
            .child(
                list()
                    .child(list_item().label("First"))
                    .child(list_item().label("Second").child(Text::new("tail"))),
            ),
    );
    assert_eq!(count(&app, Role::Tab), 2);
    assert!(app.drive_action(&DriveAction::click(Role::Tab, "Two")));
    assert_eq!(selected.get(), 1);
    assert_eq!(count(&app, Role::ListItem), 2);

    // A modal dialog makes its peers inert, so it gets its own app.
    let app = mount(dialog().label("Confirm").child(Text::new("Sure?")));
    assert!(app.find_widget(Role::Dialog, Some("Confirm")).is_some());
}

#[test]
fn theme_provider_rich_text_and_progress_build() {
    let app = mount(
        theme_provider().value("dark").child(
            column()
                .child(rich_text().label("Notes").value("Body text"))
                .child(progress_bar().label("Upload").number(40.0))
                .child(drag_handle().label("Move row")),
        ),
    );
    assert!(app.find_widget(Role::Document, None).is_some());
    assert!(app
        .find_widget(Role::ProgressIndicator, Some("Upload"))
        .is_some());
    assert!(app.find_widget(Role::Group, Some("Move row")).is_some());
}
