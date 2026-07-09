use super::*;
use schnellui_scene::WidgetKind;

#[test]
fn role_tag_roundtrip() {
    for r in [
        Role::Group,
        Role::Label,
        Role::Button,
        Role::CheckBox,
        Role::Slider,
        Role::TextInput,
        Role::Image,
        Role::List,
        Role::Status,
        Role::ProgressIndicator,
        Role::Switch,
        Role::Radio,
        Role::ScrollView,
        Role::Chart,
        Role::Link,
        Role::Tab,
        Role::TabList,
        Role::ListItem,
        Role::Table,
        Role::TableRow,
        Role::Cell,
        Role::ColumnHeader,
        Role::ComboBox,
        Role::ListBoxOption,
        Role::Dialog,
        Role::AlertDialog,
        Role::Menu,
        Role::MenuItem,
        Role::PasswordInput,
    ] {
        assert_eq!(Role::from_u16(r.as_u16()), r);
    }
}

#[test]
fn new_roles_map_to_accesskit_and_have_stable_labels() {
    assert_eq!(
        Role::ProgressIndicator.to_accesskit(),
        accesskit::Role::ProgressIndicator
    );
    assert_eq!(Role::Switch.to_accesskit(), accesskit::Role::Switch);
    assert_eq!(Role::Radio.to_accesskit(), accesskit::Role::RadioButton);
    assert_eq!(Role::ScrollView.to_accesskit(), accesskit::Role::ScrollView);
    // no dedicated chart role in accesskit 0.24 → closest is Figure.
    assert_eq!(Role::Chart.to_accesskit(), accesskit::Role::Figure);
    // stable snake_case labels for the JSON dump / snapshot diffs (SOUL §6.5)
    assert_eq!(Role::ProgressIndicator.label(), "progress_indicator");
    assert_eq!(Role::Switch.label(), "switch");
    assert_eq!(Role::Radio.label(), "radio");
    assert_eq!(Role::ScrollView.label(), "scroll_view");
    assert_eq!(Role::Chart.label(), "chart");
    // stable u16 tags appended after Status = 8 (wire stability).
    assert_eq!(Role::ProgressIndicator.as_u16(), 9);
    assert_eq!(Role::Chart.as_u16(), 13);
    // navigation/selection roles appended after Chart = 13 (wire stability).
    assert_eq!(Role::Link.to_accesskit(), accesskit::Role::Link);
    assert_eq!(Role::Tab.to_accesskit(), accesskit::Role::Tab);
    assert_eq!(Role::TabList.to_accesskit(), accesskit::Role::TabList);
    assert_eq!(Role::ListItem.to_accesskit(), accesskit::Role::ListItem);
    assert_eq!(Role::Link.label(), "link");
    assert_eq!(Role::Tab.label(), "tab");
    assert_eq!(Role::TabList.label(), "tab_list");
    assert_eq!(Role::ListItem.label(), "list_item");
    assert_eq!(Role::Link.as_u16(), 14);
    assert_eq!(Role::ListItem.as_u16(), 17);
    // table roles appended after ListItem = 17 (wire stability).
    assert_eq!(Role::Table.to_accesskit(), accesskit::Role::Table);
    assert_eq!(Role::TableRow.to_accesskit(), accesskit::Role::Row);
    assert_eq!(Role::Cell.to_accesskit(), accesskit::Role::Cell);
    assert_eq!(
        Role::ColumnHeader.to_accesskit(),
        accesskit::Role::ColumnHeader
    );
    assert_eq!(Role::Table.label(), "table");
    assert_eq!(Role::TableRow.label(), "row");
    assert_eq!(Role::Cell.label(), "cell");
    assert_eq!(Role::ColumnHeader.label(), "column_header");
    assert_eq!(Role::Table.as_u16(), 18);
    assert_eq!(Role::ColumnHeader.as_u16(), 21);
    // dropdown roles appended after MultilineTextInput = 23 (wire stability).
    assert_eq!(Role::ComboBox.to_accesskit(), accesskit::Role::ComboBox);
    assert_eq!(
        Role::ListBoxOption.to_accesskit(),
        accesskit::Role::ListBoxOption
    );
    assert_eq!(Role::ComboBox.label(), "combo_box");
    assert_eq!(Role::ListBoxOption.label(), "list_box_option");
    assert_eq!(Role::ComboBox.as_u16(), 24);
    assert_eq!(Role::ListBoxOption.as_u16(), 25);
    // dialog roles append without disturbing any existing tag.
    assert_eq!(Role::Dialog.to_accesskit(), accesskit::Role::Dialog);
    assert_eq!(
        Role::AlertDialog.to_accesskit(),
        accesskit::Role::AlertDialog
    );
    assert_eq!(Role::Dialog.label(), "dialog");
    assert_eq!(Role::AlertDialog.label(), "alert_dialog");
    assert_eq!(Role::Dialog.as_u16(), 26);
    assert_eq!(Role::AlertDialog.as_u16(), 27);
    assert_eq!(Role::Menu.to_accesskit(), accesskit::Role::Menu);
    assert_eq!(Role::MenuItem.to_accesskit(), accesskit::Role::MenuItem);
    assert_eq!(Role::Menu.label(), "menu");
    assert_eq!(Role::MenuItem.label(), "menu_item");
    assert_eq!(Role::Menu.as_u16(), 28);
    assert_eq!(Role::MenuItem.as_u16(), 29);
    assert_eq!(
        Role::PasswordInput.to_accesskit(),
        accesskit::Role::PasswordInput
    );
    assert_eq!(Role::PasswordInput.label(), "password_input");
    assert_eq!(Role::PasswordInput.as_u16(), 30);
}

#[test]
fn scroll_actions_have_names_and_map_into_a11y_node() {
    let mut a = ActionFlags::default();
    a.insert(ActionFlags::SCROLL_UP);
    a.insert(ActionFlags::SCROLL_DOWN);
    assert_eq!(
        a.names(),
        vec!["scroll_up".to_string(), "scroll_down".to_string()]
    );

    // a ScrollView node advertising the scroll actions surfaces them on the
    // assembled accesskit node (SOUL §6.1, §6.3).
    let mut scene = Scene::new();
    let sv = scene.insert(WidgetKind::Scroll, None);
    scene.set_root(sv);
    {
        let ax = scene.a11y_mut(sv);
        ax.role = Role::ScrollView.as_u16();
        ax.actions = a.0;
    }
    let update = build_full_tree_update(&scene);
    let (_, node) = &update.nodes[0];
    assert_eq!(node.role(), accesskit::Role::ScrollView);
    assert!(node.supports_action(accesskit::Action::ScrollUp));
    assert!(node.supports_action(accesskit::Action::ScrollDown));
}

#[test]
fn route_action_resolves_scroll_requests() {
    // route_action does not filter by action — a ScrollUp/ScrollDown request
    // resolves its target WidgetId exactly like Click (SOUL §6.3).
    let mut scene = Scene::new();
    let sv = scene.insert(WidgetKind::Scroll, None);
    scene.set_root(sv);
    scene.a11y_mut(sv).role = Role::ScrollView.as_u16();
    for action in [accesskit::Action::ScrollUp, accesskit::Action::ScrollDown] {
        let req = accesskit::ActionRequest {
            action,
            target_tree: accesskit::TreeId::ROOT,
            target_node: to_access_id(sv),
            data: None,
        };
        assert_eq!(route_action(&scene, &req), Some(sv));
    }
}

#[test]
fn state_and_action_names() {
    let mut s = StateFlags::default();
    s.insert(StateFlags::CHECKED);
    s.insert(StateFlags::FOCUSED);
    s.insert(StateFlags::MODAL);
    assert_eq!(
        s.names(),
        vec![
            "checked".to_string(),
            "focused".to_string(),
            "modal".to_string()
        ]
    );

    let mut a = ActionFlags::default();
    a.insert(ActionFlags::CLICK);
    assert_eq!(a.names(), vec!["click".to_string()]);

    assert_eq!(
        StateFlags::COLLAPSIBLE.names(),
        vec!["collapsed".to_string()]
    );
    assert_eq!(
        StateFlags(StateFlags::COLLAPSIBLE.0 | StateFlags::EXPANDED.0).names(),
        vec!["expanded".to_string()]
    );
}

#[test]
fn dump_walks_tree_and_reads_column() {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Column, None);
    scene.set_root(root);
    {
        let a = scene.a11y_mut(root);
        a.role = Role::Group.as_u16();
    }
    let btn = scene.insert(WidgetKind::Button, Some(root));
    {
        let a = scene.a11y_mut(btn);
        a.role = Role::Button.as_u16();
        a.name = Some("increment".into());
        a.actions = ActionFlags::CLICK.0 | ActionFlags::FOCUS.0;
    }
    let dump = dump_tree(&scene);
    let root_dump = dump.root.unwrap();
    assert_eq!(root_dump.role, "group");
    assert_eq!(root_dump.children.len(), 1);
    let b = &root_dump.children[0];
    assert_eq!(b.role, "button");
    assert_eq!(b.name.as_deref(), Some("increment"));
    assert!(b.actions.contains(&"click".to_string()));
}

#[test]
fn dump_json_is_valid_json() {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Button, None);
    scene.set_root(root);
    scene.a11y_mut(root).role = Role::Button.as_u16();
    let json = dump_json(&scene);
    let parsed: A11yTreeDump = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.root.unwrap().role, "button");
}

/// Builds a two-node scene: a `Column` (role `Group`) root containing one
/// clickable, focusable `Button` named "increment".
fn counter_scene() -> (Scene, WidgetId, WidgetId) {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Column, None);
    scene.set_root(root);
    scene.a11y_mut(root).role = Role::Group.as_u16();

    let btn = scene.insert(WidgetKind::Button, Some(root));
    let a = scene.a11y_mut(btn);
    a.role = Role::Button.as_u16();
    a.name = Some("increment".into());
    a.actions = ActionFlags::CLICK.0 | ActionFlags::FOCUS.0;
    (scene, root, btn)
}

#[test]
fn json_dump_roundtrips_and_carries_role_and_name() {
    let (scene, _root, _btn) = counter_scene();
    let json = dump_json(&scene);
    // round-trip through serde
    let parsed: A11yTreeDump = serde_json::from_str(&json).unwrap();
    let reser = serde_json::to_string_pretty(&parsed).unwrap();
    assert_eq!(json, reser);
    // role + name survive
    let root = parsed.root.unwrap();
    assert_eq!(root.role, "group");
    let btn = &root.children[0];
    assert_eq!(btn.role, "button");
    assert_eq!(btn.name.as_deref(), Some("increment"));
}

#[test]
fn full_tree_update_covers_whole_tree() {
    let (scene, root, btn) = counter_scene();
    let update = build_full_tree_update(&scene);
    // both nodes present, in pre-order
    assert_eq!(update.nodes.len(), 2);
    assert_eq!(update.nodes[0].0, to_access_id(root));
    assert_eq!(update.nodes[1].0, to_access_id(btn));
    // tree root set, focus falls back to root (nothing focused)
    assert_eq!(update.tree.as_ref().unwrap().root, to_access_id(root));
    assert_eq!(update.focus, to_access_id(root));
    // the button node carries its role, name, and Click action
    let (_, btn_node) = &update.nodes[1];
    assert_eq!(btn_node.role(), accesskit::Role::Button);
    assert_eq!(btn_node.label(), Some("increment"));
    assert!(btn_node.supports_action(accesskit::Action::Click));
    // and the root lists the button as its child
    let (_, root_node) = &update.nodes[0];
    assert_eq!(root_node.children(), &[to_access_id(btn)]);
}

#[test]
fn native_window_tree_has_role_title_bounds_scale_and_toolkit_metadata() {
    let (mut scene, root, btn) = counter_scene();
    scene.set_layout(
        root,
        schnellui_scene::LayoutBox {
            rect: schnellui_scene::Rect {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 200.0,
            },
            ..Default::default()
        },
    );
    scene.set_layout(
        btn,
        schnellui_scene::LayoutBox {
            rect: schnellui_scene::Rect {
                x: 20.0,
                y: 40.0,
                width: 120.0,
                height: 32.0,
            },
            ..Default::default()
        },
    );

    let update = build_full_window_tree_update(&scene, 2.0, "counter");
    let tree = update.tree.as_ref().unwrap();
    assert_eq!(tree.toolkit_name.as_deref(), Some("schnellui"));
    assert_eq!(
        tree.toolkit_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );

    let root_node = &update.nodes[0].1;
    assert_eq!(root_node.role(), accesskit::Role::Window);
    assert_eq!(root_node.label(), Some("counter"));
    assert_eq!(root_node.transform(), Some(&accesskit::Affine::scale(2.0)));
    assert_eq!(
        root_node.bounds(),
        Some(accesskit::Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 400.0,
            y1: 200.0,
        })
    );
    assert_eq!(
        update.nodes[1].1.bounds(),
        Some(accesskit::Rect {
            x0: 20.0,
            y0: 40.0,
            x1: 140.0,
            y1: 72.0,
        })
    );
}

#[test]
fn label_change_produces_one_node_incremental_update() {
    let (mut scene, _root, btn) = counter_scene();
    // mount emits the full tree; now change only the button's label.
    scene.a11y_mut(btn).name = Some("decrement".into());
    scene.mark_dirty(btn, schnellui_scene::DirtyFlags::A11Y);

    let update = build_incremental_tree_update(&scene);
    // exactly one node — the changed button, never the whole tree (SOUL §6.2)
    assert_eq!(update.nodes.len(), 1);
    assert_eq!(update.nodes[0].0, to_access_id(btn));
    assert_eq!(update.nodes[0].1.label(), Some("decrement"));
    // incremental updates omit tree shape but still name focus
    assert!(update.tree.is_none());
    assert_eq!(update.focus, focus_node_id(&scene));
}

#[test]
fn route_action_resolves_known_and_rejects_unknown() {
    let (scene, _root, btn) = counter_scene();
    let req = accesskit::ActionRequest {
        action: accesskit::Action::Click,
        target_tree: accesskit::TreeId::ROOT,
        target_node: to_access_id(btn),
        data: None,
    };
    assert_eq!(route_action(&scene, &req), Some(btn));

    // an id that names no live node resolves to None
    let bogus = accesskit::ActionRequest {
        action: accesskit::Action::Click,
        target_tree: accesskit::TreeId::ROOT,
        target_node: accesskit::NodeId(0xDEAD_BEEF),
        data: None,
    };
    assert_eq!(route_action(&scene, &bogus), None);
}

#[test]
fn click_action_request_fires_the_handler() {
    use std::cell::Cell;
    use std::rc::Rc;

    let (scene, _root, btn) = counter_scene();
    let count = Rc::new(Cell::new(0i32));

    let mut router = ActionRouter::new();
    {
        let count = count.clone();
        // the SAME closure a pointer click would fire (SOUL §6.3)
        router.on(btn, accesskit::Action::Click, move |ctx| {
            assert_eq!(ctx.action, accesskit::Action::Click);
            count.set(count.get() + 1);
        });
    }
    assert!(router.has_handler(btn, accesskit::Action::Click));

    let req = accesskit::ActionRequest {
        action: accesskit::Action::Click,
        target_tree: accesskit::TreeId::ROOT,
        target_node: to_access_id(btn),
        data: None,
    };
    assert!(router.dispatch(&scene, &req));
    assert!(router.dispatch(&scene, &req));
    assert_eq!(count.get(), 2);

    // an action with no registered handler does not fire
    let focus_req = accesskit::ActionRequest {
        action: accesskit::Action::Focus,
        target_tree: accesskit::TreeId::ROOT,
        target_node: to_access_id(btn),
        data: None,
    };
    assert!(!router.dispatch(&scene, &focus_req));
    assert_eq!(count.get(), 2);
}

#[test]
fn set_value_passes_action_data_to_handler() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut scene = Scene::new();
    let input = scene.insert(WidgetKind::TextInput, None);
    scene.set_root(input);
    {
        let a = scene.a11y_mut(input);
        a.role = Role::TextInput.as_u16();
        a.actions = ActionFlags::SET_VALUE.0 | ActionFlags::FOCUS.0;
    }

    let seen: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let mut router = ActionRouter::new();
    {
        let seen = seen.clone();
        router.on(input, accesskit::Action::SetValue, move |ctx| {
            if let Some(accesskit::ActionData::Value(v)) = ctx.data {
                *seen.borrow_mut() = Some(v.to_string());
            }
        });
    }

    let req = accesskit::ActionRequest {
        action: accesskit::Action::SetValue,
        target_tree: accesskit::TreeId::ROOT,
        target_node: to_access_id(input),
        data: Some(accesskit::ActionData::Value("hello".into())),
    };
    assert!(router.dispatch(&scene, &req));
    assert_eq!(seen.borrow().as_deref(), Some("hello"));
}

#[test]
fn tab_order_follows_tree_and_wraps() {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Column, None);
    scene.set_root(root);
    scene.a11y_mut(root).role = Role::Group.as_u16();

    // three focusable buttons + one non-focusable label interleaved
    let b1 = scene.insert(WidgetKind::Button, Some(root));
    scene.a11y_mut(b1).actions = ActionFlags::FOCUS.0;
    let label = scene.insert(WidgetKind::Text, Some(root));
    scene.a11y_mut(label).role = Role::Label.as_u16(); // no Focus action
    let b2 = scene.insert(WidgetKind::Button, Some(root));
    scene.a11y_mut(b2).actions = ActionFlags::FOCUS.0;
    let b3 = scene.insert(WidgetKind::Button, Some(root));
    scene.a11y_mut(b3).actions = ActionFlags::FOCUS.0;

    assert_eq!(tab_order(&scene), vec![b1, b2, b3]);
    assert_eq!(next_in_tab_order(&scene, b1), Some(b2));
    assert_eq!(next_in_tab_order(&scene, b3), Some(b1)); // wrap
    assert_eq!(prev_in_tab_order(&scene, b1), Some(b3)); // wrap
    assert_eq!(prev_in_tab_order(&scene, b2), Some(b1));
    // a non-focusable node is not in the order
    assert_eq!(next_in_tab_order(&scene, label), None);
}

#[test]
fn tab_order_skips_disabled() {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Column, None);
    scene.set_root(root);
    scene.a11y_mut(root).role = Role::Group.as_u16();

    // enabled — disabled — enabled: Tab must hop straight over the middle one
    let b1 = scene.insert(WidgetKind::Button, Some(root));
    scene.a11y_mut(b1).actions = ActionFlags::FOCUS.0;
    let dis = scene.insert(WidgetKind::Button, Some(root));
    {
        let a = scene.a11y_mut(dis);
        a.actions = ActionFlags::FOCUS.0;
        a.state = StateFlags::DISABLED.0;
    }
    let b2 = scene.insert(WidgetKind::Button, Some(root));
    scene.a11y_mut(b2).actions = ActionFlags::FOCUS.0;

    assert_eq!(tab_order(&scene), vec![b1, b2]);
    assert_eq!(next_in_tab_order(&scene, b1), Some(b2));
    assert_eq!(next_in_tab_order(&scene, b2), Some(b1)); // wrap past the disabled one
    assert_eq!(prev_in_tab_order(&scene, b1), Some(b2)); // wrap backwards too
                                                         // the disabled widget itself is not a valid step origin
    assert_eq!(next_in_tab_order(&scene, dis), None);
}

#[test]
fn focus_bit_drives_tree_update_focus_and_dump() {
    let (mut scene, root, btn) = counter_scene();
    // nothing focused → focus == root
    assert_eq!(focused(&scene), None);
    assert_eq!(focus_node_id(&scene), to_access_id(root));

    // focus the button
    scene.a11y_mut(btn).state |= StateFlags::FOCUSED.0;
    assert_eq!(focused(&scene), Some(btn));
    assert_eq!(build_full_tree_update(&scene).focus, to_access_id(btn));
    // and it surfaces in the JSON dump's reading-order/focus field
    assert_eq!(dump_tree(&scene).focus, Some(to_access_id(btn).0));
}

#[test]
fn checkbox_checked_maps_to_toggled() {
    let mut scene = Scene::new();
    let cb = scene.insert(WidgetKind::Checkbox, None);
    scene.set_root(cb);
    {
        let a = scene.a11y_mut(cb);
        a.role = Role::CheckBox.as_u16();
        a.state = StateFlags::CHECKED.0;
    }
    let update = build_full_tree_update(&scene);
    assert_eq!(update.nodes[0].1.toggled(), Some(accesskit::Toggled::True));

    // unchecked checkbox announces the false state, not absence
    scene.a11y_mut(cb).state = 0;
    let update = build_full_tree_update(&scene);
    assert_eq!(update.nodes[0].1.toggled(), Some(accesskit::Toggled::False));
}

#[test]
fn collapsible_state_maps_both_expanded_values() {
    let mut scene = Scene::new();
    let branch = scene.insert(WidgetKind::Tab, None);
    scene.set_root(branch);
    {
        let a = scene.a11y_mut(branch);
        a.role = Role::Tab.as_u16();
        a.state = StateFlags::COLLAPSIBLE.0;
    }
    assert_eq!(
        build_full_tree_update(&scene).nodes[0].1.is_expanded(),
        Some(false)
    );

    scene.a11y_mut(branch).state |= StateFlags::EXPANDED.0;
    assert_eq!(
        build_full_tree_update(&scene).nodes[0].1.is_expanded(),
        Some(true)
    );
}

/// A hand-built 2×2 table (header row + one data row) for the table-facts tests.
/// Returns (scene, table, header_row, data_row, data_cells).
fn table_scene() -> (Scene, WidgetId, WidgetId, WidgetId, Vec<WidgetId>) {
    let mut scene = Scene::new();
    let table = scene.insert(WidgetKind::Table, None);
    scene.set_root(table);
    scene.a11y_mut(table).role = Role::Table.as_u16();
    let header = scene.insert(WidgetKind::TableRow, Some(table));
    scene.a11y_mut(header).role = Role::TableRow.as_u16();
    for name in ["Name", "Age"] {
        let c = scene.insert(WidgetKind::TableCell, Some(header));
        let a = scene.a11y_mut(c);
        a.role = Role::ColumnHeader.as_u16();
        a.name = Some(name.to_string());
    }
    let row = scene.insert(WidgetKind::TableRow, Some(table));
    scene.a11y_mut(row).role = Role::TableRow.as_u16();
    let mut cells = Vec::new();
    for v in ["Ada", "36"] {
        let c = scene.insert(WidgetKind::TableCell, Some(row));
        let a = scene.a11y_mut(c);
        a.role = Role::Cell.as_u16();
        a.name = Some(v.to_string());
        cells.push(c);
    }
    (scene, table, header, row, cells)
}

/// Table facts are derived from the retained tree (SOUL §6.1): counts on the
/// table, row indices on rows, row+column indices on cells and headers.
#[test]
fn table_facts_derive_counts_and_indices_from_the_tree() {
    let (scene, table, header, row, cells) = table_scene();
    let tf = table_facts(&scene, table);
    assert_eq!(tf.row_count, Some(2));
    assert_eq!(tf.column_count, Some(2));
    assert_eq!(tf.row_index, None);
    assert_eq!(table_facts(&scene, header).row_index, Some(0));
    assert_eq!(table_facts(&scene, row).row_index, Some(1));
    // second data cell: row 1, column 1
    let cf = table_facts(&scene, cells[1]);
    assert_eq!(cf.row_index, Some(1));
    assert_eq!(cf.column_index, Some(1));
    // header cells carry indices too (row 0)
    let hc = scene.node(header).unwrap().children[0];
    let hf = table_facts(&scene, hc);
    assert_eq!(hf.row_index, Some(0));
    assert_eq!(hf.column_index, Some(0));
}

/// The derived facts reach the real AccessKit nodes (SOUL §6.1): a screen
/// reader gets `row_count`/`column_count` on the table and `row_index`/
/// `column_index` on the cells — table navigation, not a div soup.
#[test]
fn table_facts_map_into_accesskit_nodes() {
    let (scene, table, _header, row, cells) = table_scene();
    let node = build_node(&scene, table, None);
    assert_eq!(node.role(), accesskit::Role::Table);
    assert_eq!(node.row_count(), Some(2));
    assert_eq!(node.column_count(), Some(2));
    let rn = build_node(&scene, row, None);
    assert_eq!(rn.role(), accesskit::Role::Row);
    assert_eq!(rn.row_index(), Some(1));
    let cn = build_node(&scene, cells[1], None);
    assert_eq!(cn.role(), accesskit::Role::Cell);
    assert_eq!(cn.row_index(), Some(1));
    assert_eq!(cn.column_index(), Some(1));
}

#[test]
fn column_header_sort_direction_maps_to_accesskit_and_dump() {
    let (mut scene, table, header, _row, _cells) = table_scene();
    let column = scene.node(header).unwrap().children[0];
    scene.a11y_mut(column).sort_direction = SortDirection::Descending.as_u8();
    scene.set_root(table);

    let node = build_node(&scene, column, None);
    assert_eq!(
        node.sort_direction(),
        Some(accesskit::SortDirection::Descending)
    );
    let dump = dump_tree(&scene);
    let header_dump = &dump.root.unwrap().children[0].children[0];
    assert_eq!(header_dump.sort_direction.as_deref(), Some("descending"));
}

/// The JSON dump exposes the same derived facts for the agent oracle
/// (SOUL §6.5), and non-table nodes serialize without the new fields.
#[test]
fn table_facts_surface_in_the_json_dump() {
    let (scene, _table, _header, _row, _cells) = table_scene();
    let dump = dump_tree(&scene);
    let root = dump.root.unwrap();
    assert_eq!(root.role, "table");
    assert_eq!(root.row_count, Some(2));
    assert_eq!(root.column_count, Some(2));
    let data_row = &root.children[1];
    assert_eq!(data_row.role, "row");
    assert_eq!(data_row.row_index, Some(1));
    let cell = &data_row.children[1];
    assert_eq!(cell.role, "cell");
    assert_eq!(cell.name.as_deref(), Some("36"));
    assert_eq!(cell.row_index, Some(1));
    assert_eq!(cell.column_index, Some(1));

    // a non-table dump stays byte-identical: no table keys serialized
    let mut plain = Scene::new();
    let btn = plain.insert(WidgetKind::Button, None);
    plain.set_root(btn);
    plain.a11y_mut(btn).role = Role::Button.as_u16();
    let json = dump_json(&plain);
    assert!(!json.contains("row_count"), "{json}");
    assert!(!json.contains("column_index"), "{json}");
}

#[test]
fn highest_modal_is_the_only_accessible_and_actionable_subtree() {
    use schnellui_scene::DirtyFlags;

    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Column, None);
    scene.set_root(root);

    let background = scene.insert(WidgetKind::Button, Some(root));
    {
        let a = scene.a11y_mut(background);
        a.role = Role::Button.as_u16();
        a.name = Some("background".into());
        a.actions = ActionFlags::FOCUS.0 | ActionFlags::CLICK.0;
    }

    let lower = scene.insert(WidgetKind::Dialog, Some(root));
    {
        let a = scene.a11y_mut(lower);
        a.role = Role::Dialog.as_u16();
        a.name = Some("lower modal".into());
        a.state = StateFlags::MODAL.0;
    }
    let lower_button = scene.insert(WidgetKind::Button, Some(lower));
    scene.a11y_mut(lower_button).role = Role::Button.as_u16();
    scene.a11y_mut(lower_button).actions = ActionFlags::FOCUS.0;

    // Modeless peers can be later in declaration order, but never supersede
    // a focus-grabbing modal's accessibility boundary.
    let peer = scene.insert(WidgetKind::Dialog, Some(root));
    scene.a11y_mut(peer).role = Role::Dialog.as_u16();
    let peer_button = scene.insert(WidgetKind::Button, Some(peer));
    scene.a11y_mut(peer_button).role = Role::Button.as_u16();
    scene.a11y_mut(peer_button).actions = ActionFlags::FOCUS.0;

    let top = scene.insert(WidgetKind::Dialog, Some(root));
    {
        let a = scene.a11y_mut(top);
        a.role = Role::AlertDialog.as_u16();
        a.name = Some("top modal".into());
        a.state = StateFlags::MODAL.0;
    }
    let top_button = scene.insert(WidgetKind::Button, Some(top));
    {
        let a = scene.a11y_mut(top_button);
        a.role = Role::Button.as_u16();
        a.name = Some("top action".into());
        a.actions = ActionFlags::FOCUS.0 | ActionFlags::CLICK.0;
        a.state = StateFlags::FOCUSED.0;
    }

    let late_peer = scene.insert(WidgetKind::Dialog, Some(root));
    scene.a11y_mut(late_peer).role = Role::Dialog.as_u16();

    assert_eq!(active_modal_root(&scene), Some(top));
    assert_eq!(focused(&scene), Some(top_button));
    assert_eq!(tab_order(&scene), vec![top_button]);

    let dump = dump_tree(&scene);
    let exposed = dump.root.expect("modal accessibility root");
    assert_eq!(exposed.id, to_access_id(top).0);
    assert_eq!(exposed.role, "alert_dialog");
    assert_eq!(exposed.children.len(), 1);
    assert_eq!(exposed.children[0].name.as_deref(), Some("top action"));

    let full = build_full_tree_update(&scene);
    assert_eq!(full.tree.unwrap().root, to_access_id(top));
    assert_eq!(full.nodes.len(), 2);
    assert!(full
        .nodes
        .iter()
        .all(|(id, _)| { *id == to_access_id(top) || *id == to_access_id(top_button) }));

    let request = |id| accesskit::ActionRequest {
        action: accesskit::Action::Click,
        target_tree: accesskit::TreeId::ROOT,
        target_node: to_access_id(id),
        data: None,
    };
    assert_eq!(route_action(&scene, &request(background)), None);
    assert_eq!(route_action(&scene, &request(lower_button)), None);
    assert_eq!(route_action(&scene, &request(peer_button)), None);
    assert_eq!(route_action(&scene, &request(top_button)), Some(top_button));

    scene.mark_dirty(background, DirtyFlags::A11Y);
    scene.mark_dirty(top_button, DirtyFlags::A11Y);
    let incremental = build_incremental_tree_update(&scene);
    assert_eq!(incremental.nodes.len(), 1);
    assert_eq!(incremental.nodes[0].0, to_access_id(top_button));
}

#[test]
fn modeless_dialogs_share_the_normal_accessibility_tree() {
    let mut scene = Scene::new();
    let root = scene.insert(WidgetKind::Row, None);
    scene.set_root(root);
    for name in ["left inspector", "right inspector"] {
        let dialog = scene.insert(WidgetKind::Dialog, Some(root));
        let a = scene.a11y_mut(dialog);
        a.role = Role::Dialog.as_u16();
        a.name = Some(name.into());
        let button = scene.insert(WidgetKind::Button, Some(dialog));
        scene.a11y_mut(button).role = Role::Button.as_u16();
        scene.a11y_mut(button).actions = ActionFlags::FOCUS.0;
    }

    assert_eq!(active_modal_root(&scene), None);
    assert_eq!(dump_tree(&scene).root.unwrap().id, to_access_id(root).0);
    assert_eq!(tab_order(&scene).len(), 2);
    assert_eq!(build_full_tree_update(&scene).nodes.len(), 5);
}
