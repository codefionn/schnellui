//! Expansion tests for `view!` (SOUL §3.3).
//!
//! These invoke the real proc-macro and compile its output against the frozen
//! `schnellui-widgets` builder API — proving the static/dynamic split lowers to
//! type-checking `Column::new().child(Text::new(…))` / `Text::dynamic(…)` /
//! `Button::new(…).on_click(…)` chains, not just that tokens are produced.

use schnellui_macro::view;
use schnellui_scene::WidgetKind;
use schnellui_signal::create_signal;

/// The SOUL §3.3 counter UI: a static title, a dynamic value slot bound to a
/// signal, and a button whose `on:click` mutates it.
#[test]
fn counter_view_expands_and_type_checks() {
    let count = create_signal(0i32);

    let ui = view! {
        column {
            text(size = 24.0) { "Counter" }
            text { (count.get()) }
            button(on:click = move || { count.set(count.get() + 1); }) { "increment" }
        }
    };

    // static skeleton built once (setup fn semantics, SOUL §3.3): three children.
    assert_eq!(ui.child_count(), 3);
    assert_eq!(ui.kind(), WidgetKind::Column);
}

/// A fully-static subtree lowers to plain builder calls (no reactive wiring).
#[test]
fn static_row_expands() {
    let ui = view! {
        row {
            "hello"                 // bare string child → implicit Text::new
            text { "world" }        // explicit text element
        }
    };
    assert_eq!(ui.child_count(), 2);
    assert_eq!(ui.kind(), WidgetKind::Row);
}

/// Nested containers compose; the inner column is a child of the outer.
#[test]
fn nested_containers_expand() {
    let ui = view! {
        column {
            text { "header" }
            column {
                text { "a" }
                text { "b" }
            }
        }
    };
    assert_eq!(ui.child_count(), 2);
}

#[test]
fn container_minimum_size_expands_and_type_checks() {
    let ui = view! {
        column(min_width = 240.0, min_height = 120.0) {
            text { "content" }
        }
    };
    assert_eq!(ui.child_count(), 1);
    assert_eq!(ui.kind(), WidgetKind::Column);
}

#[test]
fn leaf_minimum_size_expands_and_type_checks_in_any_attribute_order() {
    let _ui = view! {
        button(min_width = 160.0, on:click = || {}, min_height = 44.0) { "Continue" }
    };
}

#[test]
fn dialog_expands_and_type_checks() {
    let ui = view! {
        dialog(title = "Preferences", position = center, modeless, non_fixed, undecorated) {
            text { "Workspace settings" }
            button { "Done" }
        }
    };
    assert_eq!(ui.kind(), WidgetKind::Dialog);
    assert_eq!(ui.child_count(), 2);
}

/// A `text` element with attrs + a dynamic slot: attrs chain, the `(expr)`
/// becomes a `Text::dynamic` reactive leaf.
#[test]
fn dynamic_text_with_attrs_expands() {
    let value = create_signal(String::from("hi"));
    let t = view! {
        text(size = 18.0) { (value.get()) }
    };
    assert!(t.is_dynamic());
    assert_eq!(t.kind(), WidgetKind::Text);
}

/// A single bare dynamic root lowers to a dynamic Text leaf.
#[test]
fn bare_dynamic_root_expands() {
    let n = create_signal(7i32);
    let t = view! { (n.get()) };
    assert!(t.is_dynamic());
}

/// A static wrapped paragraph: `wrap = <keyword>` + `align = <keyword>` +
/// `ellipsis` lower to `.wrap(WrapMode::…).align(TextAlign::…).ellipsis()` and
/// type-check against the frozen `Text` builder API (SOUL §8.1).
#[test]
fn wrapped_text_view_expands_and_type_checks() {
    let t = view! {
        text(wrap = word, align = center) { "A long paragraph that should wrap onto lines" }
    };
    assert_eq!(t.kind(), WidgetKind::Text);
    assert!(!t.is_dynamic());
}

/// `ellipsis` (a valueless flag) + `wrap = anywhere` compile to the nullary
/// `.ellipsis()` and the `Anywhere` variant.
#[test]
fn ellipsis_flag_and_anywhere_wrap_expand() {
    let t = view! {
        text(wrap = anywhere, align = end, ellipsis) { "unbreakablelongtoken" }
    };
    assert_eq!(t.kind(), WidgetKind::Text);
}

/// Wrapping composes with a dynamic (signal-bound) text slot: the leaf is still a
/// `Text::dynamic` reactive node carrying the wrap/align config.
#[test]
fn dynamic_wrapped_text_expands_and_type_checks() {
    let msg = create_signal(String::from("hello"));
    let t = view! {
        text(wrap = word, align = justify) { (msg.get()) }
    };
    assert!(t.is_dynamic());
    assert_eq!(t.kind(), WidgetKind::Text);
}

/// The navigation/selection tags (SOUL §8.1): `tabs`/`tab` lower to the
/// `TabBar`/`Tab` builders, `selected = …` and `on:select = …` chain, and the
/// label follows the button convention.
#[test]
fn tabs_view_expands_and_type_checks() {
    let chosen = create_signal(0usize);
    let ui = view! {
        tabs(gap = 2.0) {
            tab(selected = true, on:select = move || { chosen.set(0); }) { "general" }
            tab(on:select = move || { chosen.set(1); }) { "privacy" }
        }
    };
    assert_eq!(ui.child_count(), 2);
    assert_eq!(ui.kind(), WidgetKind::TabBar);
}

#[test]
fn tabs_view_accepts_a_trailing_add_action() {
    let ui = view! {
        tabs(trailing = ::schnellui_widgets::Button::new("Add tab")) {
            tab(selected = true) { "general" }
            tab { "privacy" }
        }
    };
    assert_eq!(ui.child_count(), 2);
    assert!(ui.has_trailing());
}

#[test]
fn tabs_view_accepts_an_opt_in_reorder_handler() {
    let reordered = create_signal((0usize, 0usize));
    let ui = view! {
        tabs(on:reorder = move |from, to| { reordered.set((from, to)); }) {
            tab(selected = true) { "general" }
            tab { "privacy" }
        }
    };
    assert_eq!(ui.child_count(), 2);
    assert_eq!(ui.kind(), WidgetKind::TabBar);
}

/// Grouped tabs retain ordinary tab behavior while recursive `tab_node`s opt into
/// the tree presentation through a valueless builder flag.
#[test]
fn grouped_tabs_view_expands_and_type_checks() {
    let planning_open = create_signal(true);
    let ui = view! {
        grouped_tabs(tree, indent = 14.0, min_tab_width = 180.0) {
            tab_group(label = "Workspace") {
                tab_node(
                    label = "Editor",
                    selected = true,
                    on:toggle = move |open| { planning_open.set(open); },
                    action = ::schnellui_widgets::Button::new("Refresh")
                ) {
                    tab_node(label = "Outline") {}
                    tab { "Search" }
                }
                tab { "Terminal" }
            }
            tab_group(label = "Account") {
                tab_node(label = "Settings", expanded = false) {
                    tab { "Privacy" }
                }
            }
        }
    };

    assert_eq!(ui.kind(), WidgetKind::TabBar);
    assert_eq!(ui.group_count(), 2);
    assert_eq!(ui.tab_count(), 6);
}

/// `list`/`list_item` lower to the `List`/`ListItem` builders (SOUL §8.1).
#[test]
fn list_view_expands_and_type_checks() {
    let ui = view! {
        list {
            list_item(selected = true) { "inbox" }
            list_item { "archive" }
        }
    };
    assert_eq!(ui.child_count(), 2);
    assert_eq!(ui.kind(), WidgetKind::List);
}

/// `table`/`table_row` lower to the `Table`/`TableRow` builders (SOUL §8.1):
/// `header` marks the header row, `selected_row`/`on:select_row` enable row
/// selection through the same path an AccessKit `Click` takes (SOUL §6.3).
#[test]
fn table_view_expands_and_type_checks() {
    let picked = create_signal(0usize);
    let ui = view! {
        table(selected_row = 0, on:select_row = move |i| { picked.set(i); }) {
            table_row(header) { "Name" "Age" }
            table_row { "Ada Lovelace" "36" }
            table_row { "Grace Hopper" "85" }
        }
    };
    assert_eq!(ui.kind(), WidgetKind::Table);
    assert_eq!(ui.row_count(), 3);
    assert!(ui.selectable());
}

/// Header titles can independently opt into sorting and use the compact
/// `asc`/`desc` direction keywords.
#[test]
fn sortable_table_columns_expand_and_type_check() {
    let direction = create_signal(schnellui_widgets::SortDirection::Descending);
    let ui = view! {
        table {
            table_row(header) {
                table_column(
                    sort = asc,
                    on:sort = move |next| { direction.set(next); }
                ) { "Name" }
                th(sort = desc) { "Age" }
                "City"
            }
            table_row { "Ada" "36" "London" }
        }
    };
    assert_eq!(ui.kind(), WidgetKind::Table);
    assert_eq!(ui.row_count(), 2);
}

/// `svg` lowers to the `Svg` builder: the single static child is the markup, and
/// `alt`/`width`/`height` chain (SOUL §8.1).
#[test]
fn svg_view_expands_and_type_checks() {
    let ui = view! {
        row(gap = 4.0) {
            svg(alt = "logo") { r##"<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" fill="#3366cc"/></svg>"## }
            svg(width = 16.0, height = 16.0) { "<svg viewBox=\"0 0 8 8\"><rect width=\"8\" height=\"8\"/></svg>" }
        }
    };
    assert_eq!(ui.child_count(), 2);
    assert_eq!(ui.kind(), WidgetKind::Row);
}

/// `link` and `badge` lower to the `Link`/`Badge` builders; a link's `on:click`
/// binds like a button's (SOUL §6.3).
#[test]
fn link_and_badge_expand_and_type_check() {
    let clicked = create_signal(false);
    let ui = view! {
        row(gap = 4.0) {
            link(on:click = move || { clicked.set(true); }) { "docs" }
            badge { "3" }
        }
    };
    assert_eq!(ui.child_count(), 2);
    assert_eq!(ui.kind(), WidgetKind::Row);
}
