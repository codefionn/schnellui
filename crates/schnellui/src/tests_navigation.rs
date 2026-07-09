use super::*;
use schnellui_a11y::Role;

/// With nothing focused, the scroll keys page the first viewport like a
/// browser scrolls its document: arrows a notch, Space/PageDown a page
/// (Shift+Space back), Home/End to the boundaries (SOUL §6.3).
#[test]
fn scroll_keys_page_the_viewport() {
    use schnellui_widgets::{Column, Scroll, Text};

    let view = {
        let mut col = Column::new().gap(2.0);
        for i in 0..40 {
            col = col.child(Text::new(format!("Row {i}")));
        }
        Scroll::new().size(320.0, 220.0).child(col)
    };
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();
    let sv = app.find_widget(Role::ScrollView, None).unwrap();
    let page = 220.0 - SCROLL_STEP;

    assert!(app.dispatch_key(UiKey::Down { shift: false }));
    assert_eq!(app.scene().scroll_offset(sv).y, SCROLL_STEP);
    assert!(app.dispatch_key(UiKey::Up { shift: false }));
    assert_eq!(app.scene().scroll_offset(sv).y, 0.0);
    assert!(app.dispatch_key(UiKey::PageDown));
    assert_eq!(app.scene().scroll_offset(sv).y, page);
    assert!(app.dispatch_key(UiKey::Space { shift: false }));
    assert_eq!(app.scene().scroll_offset(sv).y, 2.0 * page);
    assert!(app.dispatch_key(UiKey::Space { shift: true }));
    assert_eq!(app.scene().scroll_offset(sv).y, page);
    assert!(app.dispatch_key(UiKey::End { shift: false }));
    let max = app.scene().scroll_offset(sv).y;
    assert!(max > 2.0 * page, "End pins to the far end");
    assert!(!app.dispatch_key(UiKey::PageDown), "already at the end");
    assert!(app.dispatch_key(UiKey::Home { shift: false }));
    assert_eq!(app.scene().scroll_offset(sv).y, 0.0);
}

#[test]
fn content_remount_preserves_and_clamps_scroll_offset() {
    use schnellui_widgets::{Column, Scroll, Text};

    fn view(rows: usize) -> Scroll {
        let mut content = Column::new();
        for row in 0..rows {
            content = content.child(Text::new(format!("Row {row}")));
        }
        Scroll::new()
            .label("results")
            .size(240.0, 100.0)
            .scrollbar(true)
            .child(content)
    }

    let mut previous = App::mount_with_size(view(40), 320, 180);
    previous.frame();
    let previous_scroll = previous
        .find_widget(Role::ScrollView, Some("results"))
        .unwrap();
    assert!(schnellui_widgets::dispatch_scroll(
        &previous.widgets,
        &mut previous.scene,
        previous_scroll,
        240.0,
    ));

    let mut same_content = App::mount_with_size(view(40), 320, 180);
    same_content.inherit_scroll_offsets(&previous);
    same_content.frame();
    let same_scroll = same_content
        .find_widget(Role::ScrollView, Some("results"))
        .unwrap();
    assert_eq!(same_content.scene().scroll_offset(same_scroll).y, 240.0);

    let mut shorter = App::mount_with_size(view(8), 320, 180);
    shorter.inherit_scroll_offsets(&previous);
    shorter.frame();
    let shorter_scroll = shorter
        .find_widget(Role::ScrollView, Some("results"))
        .unwrap();
    let clamped = shorter.scene().scroll_offset(shorter_scroll).y;
    assert!(clamped < 240.0);
    assert!(clamped >= 0.0);
}

#[test]
fn remount_restores_a_scroll_at_zero_over_a_new_initial_offset() {
    use schnellui_widgets::{Column, Scroll, Text};

    fn view() -> Scroll {
        let mut content = Column::new();
        for row in 0..20 {
            content = content.child(Text::new(format!("Row {row}")));
        }
        Scroll::new()
            .label("results")
            .restoration_key("results-scroll")
            .initial_offset(80.0)
            .size(240.0, 100.0)
            .child(content)
    }

    let mut previous = App::mount_with_size(view(), 320, 180);
    previous.frame();
    let previous_scroll = previous
        .find_widget(Role::ScrollView, Some("results"))
        .unwrap();
    assert!(schnellui_widgets::dispatch_scroll(
        &previous.widgets,
        &mut previous.scene,
        previous_scroll,
        -80.0,
    ));
    assert_eq!(previous.scene().scroll_offset(previous_scroll).y, 0.0);

    let mut replacement = App::mount_with_size(view(), 320, 180);
    replacement.inherit_remount_state(&previous);
    replacement.frame();
    let replacement_scroll = replacement
        .find_widget(Role::ScrollView, Some("results"))
        .unwrap();
    assert_eq!(replacement.scene().scroll_offset(replacement_scroll).y, 0.0);
    assert_eq!(
        replacement
            .scene()
            .a11y(replacement_scroll)
            .unwrap()
            .value
            .as_deref(),
        Some("0")
    );
}

#[test]
fn remount_scroll_offsets_follow_component_refs_across_reordering() {
    use schnellui_widgets::{Column, ComponentRef, Scroll, Text, View as _};

    fn scroll(reference: ComponentRef) -> impl View {
        Scroll::new()
            .label("duplicate")
            .size(180.0, 80.0)
            .child(
                Column::new()
                    .child(Text::new("row 1"))
                    .child(Text::new("row 2"))
                    .child(Text::new("row 3")),
            )
            .with_ref(reference)
    }

    fn view(first: ComponentRef, second: ComponentRef, reversed: bool) -> Column {
        if reversed {
            Column::new().child(scroll(second)).child(scroll(first))
        } else {
            Column::new().child(scroll(first)).child(scroll(second))
        }
    }

    let first_ref = ComponentRef::new();
    let second_ref = ComponentRef::new();
    let mut previous = App::mount(view(first_ref, second_ref, false));
    previous.frame();
    let first = previous.resolve_ref(first_ref).unwrap();
    let second = previous.resolve_ref(second_ref).unwrap();
    previous
        .scene_mut()
        .set_scroll_offset(first, Point { x: 0.0, y: 18.0 });
    previous
        .scene_mut()
        .set_scroll_offset(second, Point { x: 0.0, y: 42.0 });

    let mut replacement = App::mount(view(first_ref, second_ref, true));
    replacement.inherit_remount_state(&previous);
    let first = replacement.resolve_ref(first_ref).unwrap();
    let second = replacement.resolve_ref(second_ref).unwrap();
    assert_eq!(replacement.scene().scroll_offset(first).y, 18.0);
    assert_eq!(replacement.scene().scroll_offset(second).y, 42.0);
}

#[test]
fn content_remount_preserves_focused_text_area_selection() {
    use schnellui_widgets::TextArea;

    let mut previous = App::mount(TextArea::new("draft").placeholder("Message"));
    let previous_area = previous
        .find_widget(Role::MultilineTextInput, Some("Message"))
        .unwrap();
    previous.focus(Some(previous_area));
    assert!(previous.dispatch_key(UiKey::Home { shift: false }));
    assert!(previous.dispatch_key(UiKey::Right {
        shift: true,
        ctrl: false,
    }));
    assert!(previous.dispatch_key(UiKey::Right {
        shift: true,
        ctrl: false,
    }));
    assert_eq!(previous.selected_text().as_deref(), Some("dr"));

    let mut replacement = App::mount(TextArea::new("draft").placeholder("Message"));
    let replacement_area = replacement
        .find_widget(Role::MultilineTextInput, Some("Message"))
        .unwrap();
    replacement.inherit_remount_state(&previous);
    assert_eq!(replacement.focused_widget(), Some(replacement_area));
    assert_eq!(replacement.selected_text().as_deref(), Some("dr"));
    assert!(replacement.dispatch_key(UiKey::Char("X")));
    assert_eq!(
        replacement
            .scene()
            .a11y(replacement_area)
            .unwrap()
            .value
            .as_deref(),
        Some("Xaft")
    );

    let mut changed = App::mount(TextArea::new("server value").placeholder("Message"));
    let changed_area = changed
        .find_widget(Role::MultilineTextInput, Some("Message"))
        .unwrap();
    assert!(!schnellui_widgets::inherit_remount_state(
        &changed.widgets,
        &mut changed.scene,
        changed_area,
        &previous.widgets,
        previous_area,
    ));
}

#[test]
fn remount_keeps_an_empty_focused_input_label_floated() {
    use schnellui_widgets::TextInput;

    let mut previous = App::mount(TextInput::new("").label("Project name"));
    let previous_input = previous
        .find_widget(Role::TextInput, Some("Project name"))
        .unwrap();
    assert!(previous.focus(Some(previous_input)));
    for _ in 0..16 {
        previous.frame();
    }
    assert!(!schnellui_widgets::has_floating_label_animations(
        &previous.widgets
    ));

    let mut replacement = App::mount(TextInput::new("").label("Project name"));
    let replacement_input = replacement
        .find_widget(Role::TextInput, Some("Project name"))
        .unwrap();
    replacement.inherit_remount_state(&previous);

    assert_eq!(replacement.focused_widget(), Some(replacement_input));
    assert!(
        !schnellui_widgets::has_floating_label_animations(&replacement.widgets),
        "restoring focus must not restart the label from its resting position"
    );
}

#[test]
fn focused_input_label_stays_floated_when_a_remount_clears_its_value() {
    use schnellui_widgets::TextInput;

    let mut previous = App::mount(TextInput::new("x").label("Project name"));
    let previous_input = previous
        .find_widget(Role::TextInput, Some("Project name"))
        .unwrap();
    assert!(previous.focus(Some(previous_input)));

    let mut replacement = App::mount(TextInput::new("").label("Project name"));
    let replacement_input = replacement
        .find_widget(Role::TextInput, Some("Project name"))
        .unwrap();
    replacement.inherit_remount_state(&previous);

    assert_eq!(replacement.focused_widget(), Some(replacement_input));
    assert!(
        !schnellui_widgets::has_floating_label_animations(&replacement.widgets),
        "clearing the controlled value must not restart a focused label"
    );
}

#[test]
fn remount_preserves_every_editor_and_uses_component_refs_across_reordering() {
    use schnellui_widgets::{Column, ComponentRef, TextInput, View as _};

    fn view(first: ComponentRef, second: ComponentRef, reversed: bool) -> Column {
        let first = TextInput::new("alpha").placeholder("Field").with_ref(first);
        let second = TextInput::new("bravo")
            .placeholder("Field")
            .with_ref(second);
        if reversed {
            Column::new().child(second).child(first)
        } else {
            Column::new().child(first).child(second)
        }
    }

    let first_ref = ComponentRef::new();
    let second_ref = ComponentRef::new();
    let mut previous = App::mount(view(first_ref, second_ref, false));
    let first = previous.scene().resolve_ref(first_ref).unwrap();
    let second = previous.scene().resolve_ref(second_ref).unwrap();
    assert!(previous.focus(Some(first)));
    assert!(previous.dispatch_key(UiKey::Home { shift: false }));
    assert!(previous.focus(Some(second)));
    assert!(previous.dispatch_key(UiKey::Home { shift: false }));
    assert!(previous.dispatch_key(UiKey::Right {
        shift: false,
        ctrl: false,
    }));
    assert!(previous.dispatch_key(UiKey::Right {
        shift: false,
        ctrl: false,
    }));

    let mut replacement = App::mount(view(first_ref, second_ref, true));
    replacement.inherit_remount_state(&previous);
    let first = replacement.scene().resolve_ref(first_ref).unwrap();
    let second = replacement.scene().resolve_ref(second_ref).unwrap();
    assert_eq!(replacement.focused_widget(), Some(second));
    assert!(replacement.dispatch_key(UiKey::Char("X")));
    assert_eq!(
        replacement.scene().a11y(second).unwrap().value.as_deref(),
        Some("brXavo")
    );

    assert!(replacement.focus(Some(first)));
    assert!(replacement.dispatch_key(UiKey::Char("Y")));
    assert_eq!(
        replacement.scene().a11y(first).unwrap().value.as_deref(),
        Some("Yalpha"),
        "an unfocused editor's caret must survive too"
    );
}

#[test]
fn remount_does_not_move_focus_to_an_unrelated_same_role_control() {
    use schnellui_widgets::Button;

    let mut previous = App::mount(Button::new("Removed"));
    let removed = previous.find_widget(Role::Button, Some("Removed")).unwrap();
    assert!(previous.focus(Some(removed)));

    let mut replacement = App::mount(Button::new("Unrelated"));
    replacement.inherit_remount_state(&previous);
    assert_eq!(replacement.focused_widget(), None);
}

#[test]
fn remount_keeps_new_modal_focus_instead_of_restoring_behind_it() {
    use schnellui_widgets::{Button, Dialog, Stack};

    let mut previous = App::mount(Stack::new().child(Button::new("Background")));
    let background = previous
        .find_widget(Role::Button, Some("Background"))
        .unwrap();
    assert!(previous.focus(Some(background)));

    let mut replacement = App::mount(
        Stack::new()
            .child(Button::new("Background"))
            .child(Dialog::new("Blocking").child(Button::new("Modal action"))),
    );
    replacement.inherit_remount_state(&previous);
    let modal_action = replacement
        .find_widget(Role::Button, Some("Modal action"))
        .unwrap();
    assert_eq!(replacement.focused_widget(), Some(modal_action));
}

#[test]
fn remount_restores_a_closed_option_to_its_own_trigger() {
    use schnellui_widgets::{Column, Dropdown, DropdownOption};

    fn view(first_open: bool) -> Column {
        Column::new()
            .child(
                Dropdown::new("First")
                    .open(first_open)
                    .option(DropdownOption::new("First A"))
                    .option(DropdownOption::new("First B")),
            )
            .child(
                Dropdown::new("Second")
                    .option(DropdownOption::new("Second A"))
                    .option(DropdownOption::new("Second B")),
            )
    }

    let mut previous = App::mount(view(true));
    let option = previous
        .find_widget(Role::ListBoxOption, Some("First B"))
        .unwrap();
    assert!(previous.focus(Some(option)));

    let mut replacement = App::mount(view(false));
    let first_trigger = replacement
        .find_widget(Role::ComboBox, Some("First"))
        .unwrap();
    replacement.inherit_remount_state(&previous);

    assert_eq!(replacement.focused_widget(), Some(first_trigger));
}

#[test]
fn remount_preserves_pointer_focus_modality_without_synthetic_focus_events() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui_widgets::{Button, Column, Scroll, Text};

    let mut pointer_previous = App::mount(Button::new("Pointer target"));
    let pointer_target = pointer_previous
        .find_widget(Role::Button, Some("Pointer target"))
        .unwrap();
    assert!(pointer_previous.pointer_focus(Some(pointer_target)));
    assert!(!schnellui_widgets::focus_ring_visible(
        &pointer_previous.widgets,
        pointer_target
    ));
    let mut pointer_replacement = App::mount(Button::new("Pointer target"));
    pointer_replacement.inherit_remount_state(&pointer_previous);
    let pointer_target = pointer_replacement
        .find_widget(Role::Button, Some("Pointer target"))
        .unwrap();
    assert_eq!(pointer_replacement.focused_widget(), Some(pointer_target));
    assert!(!schnellui_widgets::focus_ring_visible(
        &pointer_replacement.widgets,
        pointer_target
    ));

    let surface = || {
        Scroll::new()
            .label("Raw surface")
            .child(Column::new().child(Text::new("content")))
    };
    let mut raw_previous = App::mount(surface());
    let raw = raw_previous
        .find_widget(Role::ScrollView, Some("Raw surface"))
        .unwrap();
    assert!(raw_previous.focus(Some(raw)));

    let gains = Rc::new(Cell::new(0));
    let observed_gains = gains.clone();
    let mut raw_replacement = App::mount(surface());
    raw_replacement.register_focused_input_handler(
        Role::ScrollView,
        Some("Raw surface"),
        move |event| {
            if matches!(event, FocusedInputEvent::Focus(RawFocusEvent::WidgetGained)) {
                observed_gains.set(observed_gains.get() + 1);
            }
            FocusedInputResult::Ignored
        },
    );
    raw_replacement.inherit_remount_state(&raw_previous);
    assert_eq!(raw_replacement.focused_widget().is_some(), true);
    assert_eq!(gains.get(), 0, "remount continuity is not a focus gain");
}

#[test]
fn follow_end_tracks_growth_until_the_user_scrolls_up_and_isolates_keys() {
    use schnellui_widgets::{Column, Scroll, Text};

    fn view(key: &'static str, rows: usize) -> Scroll {
        let mut content = Column::new();
        for row in 0..rows {
            content = content.child(Text::new(format!("Row {row}")));
        }
        Scroll::new()
            .label("transcript")
            .restoration_key(key)
            .size(240.0, 100.0)
            .scrollbar(true)
            .follow_end(true)
            .child(content)
    }

    let mut original = App::mount_with_size(view("agent-a", 40), 320, 180);
    original.frame();
    let original_scroll = original
        .find_widget(Role::ScrollView, Some("transcript"))
        .unwrap();
    assert!(schnellui_widgets::scroll_is_at_end(
        original.scene(),
        original_scroll
    ));

    let mut grown = App::mount_with_size(view("agent-a", 60), 320, 180);
    grown.inherit_scroll_offsets(&original);
    grown.frame();
    let grown_scroll = grown
        .find_widget(Role::ScrollView, Some("transcript"))
        .unwrap();
    assert!(schnellui_widgets::scroll_is_at_end(
        grown.scene(),
        grown_scroll
    ));

    assert!(schnellui_widgets::dispatch_scroll(
        &grown.widgets,
        &mut grown.scene,
        grown_scroll,
        -80.0,
    ));
    let reading_offset = grown.scene().scroll_offset(grown_scroll).y;
    let mut grown_again = App::mount_with_size(view("agent-a", 80), 320, 180);
    grown_again.inherit_scroll_offsets(&grown);
    grown_again.frame();
    let grown_again_scroll = grown_again
        .find_widget(Role::ScrollView, Some("transcript"))
        .unwrap();
    assert_eq!(
        grown_again.scene().scroll_offset(grown_again_scroll).y,
        reading_offset,
        "new content must not pull a user away from an older reading position"
    );

    let mut other_agent = App::mount_with_size(view("agent-b", 50), 320, 180);
    other_agent.inherit_scroll_offsets(&grown);
    other_agent.frame();
    let other_scroll = other_agent
        .find_widget(Role::ScrollView, Some("transcript"))
        .unwrap();
    assert!(schnellui_widgets::scroll_is_at_end(
        other_agent.scene(),
        other_scroll
    ));
}

/// Keys inside a focused text input belong to the input (browser: typing and
/// caret keys never scroll the page); PageDown still pages the enclosing
/// viewport, as it does in a browser field (SOUL §6.3).
#[test]
fn focused_text_input_consumes_keys_before_scrolling() {
    use schnellui_widgets::{Column, Scroll, Text, TextInput};

    let view = {
        let mut col = Column::new().gap(2.0).child(TextInput::new("ab"));
        for i in 0..40 {
            col = col.child(Text::new(format!("Row {i}")));
        }
        Scroll::new().size(320.0, 220.0).child(col)
    };
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();
    let sv = app.find_widget(Role::ScrollView, None).unwrap();
    let input = app.find_widget(Role::TextInput, None).unwrap();
    app.focus(Some(input));

    // Typing / caret keys mutate the input, never the viewport.
    assert!(app.dispatch_key(UiKey::Char("c")));
    assert!(app.dispatch_key(UiKey::Space { shift: false }));
    assert_eq!(
        app.scene().a11y(input).unwrap().value.as_deref(),
        Some("abc ")
    );
    assert!(app.dispatch_key(UiKey::Left {
        shift: false,
        ctrl: false
    }));
    assert!(app.dispatch_key(UiKey::Home { shift: false }));
    assert_eq!(
        app.scene().scroll_offset(sv).y,
        0.0,
        "no page scroll leaked"
    );
    // Enter is consumed as a (single-line) no-op — it must not activate or scroll.
    assert!(!app.dispatch_key(UiKey::Enter));
    assert_eq!(app.scene().scroll_offset(sv).y, 0.0);
    // PageDown falls through to the enclosing viewport, like a browser field.
    assert!(app.dispatch_key(UiKey::PageDown));
    assert!(app.scene().scroll_offset(sv).y > 0.0);
}

#[test]
fn clipboard_edit_operations_drive_text_inputs_and_areas() {
    use schnellui_widgets::{TextArea, TextInput};

    let mut input_app = App::mount(TextInput::new("café"));
    input_app.frame();
    let input = input_app.find_widget(Role::TextInput, None).unwrap();
    input_app.focus(Some(input));
    assert!(input_app.dispatch_key(UiKey::SelectAll));
    assert_eq!(input_app.selected_text().as_deref(), Some("café"));
    assert!(input_app.delete_text_selection());
    assert_eq!(
        input_app.scene().a11y(input).unwrap().value.as_deref(),
        Some("")
    );
    assert!(input_app.paste_text("one\r\ntwo\n"));
    assert_eq!(
        input_app.scene().a11y(input).unwrap().value.as_deref(),
        Some("onetwo"),
        "single-line paste strips line breaks"
    );

    let mut area_app = App::mount(TextArea::new("one\ntwo"));
    area_app.frame();
    let area = area_app
        .find_widget(Role::MultilineTextInput, None)
        .unwrap();
    area_app.focus(Some(area));
    assert!(area_app.dispatch_key(UiKey::SelectAll));
    assert_eq!(area_app.selected_text().as_deref(), Some("one\ntwo"));
    assert!(area_app.delete_text_selection());
    assert!(area_app.paste_text("a\r\nb\rc"));
    assert_eq!(
        area_app.scene().a11y(area).unwrap().value.as_deref(),
        Some("a\nb\nc"),
        "multi-line paste normalizes platform line endings"
    );
}

#[test]
fn editable_context_menu_defaults_replace_append_and_clamp() {
    use schnellui_a11y::accesskit_reexport::{Action, ActionRequest, TreeId};
    use schnellui_a11y::to_access_id;
    use schnellui_a11y::{ActionFlags, StateFlags};
    use schnellui_widgets::{ContextMenu, ContextMenuItem, TextInput};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn menu_items(scene: &Scene) -> Vec<scene::WidgetId> {
        fn collect(scene: &Scene, id: scene::WidgetId, out: &mut Vec<scene::WidgetId>) {
            if scene
                .a11y(id)
                .is_some_and(|semantics| Role::from_u16(semantics.role) == Role::MenuItem)
            {
                out.push(id);
            }
            if let Some(node) = scene.node(id) {
                for child in &node.children {
                    collect(scene, *child, out);
                }
            }
        }

        let mut items = Vec::new();
        if let Some(root) = scene.root() {
            collect(scene, root, &mut items);
        }
        items
    }

    let mut app = App::mount_with_size(TextInput::new("draft"), 400, 300);
    app.frame();
    let input = app.find_widget(Role::TextInput, None).unwrap();
    assert!(ActionFlags(app.scene().a11y(input).unwrap().actions)
        .contains(ActionFlags::SHOW_CONTEXT_MENU));
    assert!(app.dispatch_action(&ActionRequest {
        action: Action::ShowContextMenu,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(input),
        data: None,
    }));
    assert!(app.context_menu_is_open());
    assert!(app.dispatch_key(UiKey::Escape));
    assert!(!app.context_menu_is_open());
    assert!(app.open_text_context_menu(input, Point { x: 399.0, y: 299.0 }, false));
    let menu = app.find_widget(Role::Menu, Some("Text editing")).unwrap();
    let menu_rect = app.scene().layout(menu).unwrap().rect;
    assert!(menu_rect.x >= 0.0 && menu_rect.y >= 0.0);
    assert!(menu_rect.x + menu_rect.width <= 400.0);
    assert!(menu_rect.y + menu_rect.height <= 300.0);
    let defaults = menu_items(app.scene());
    assert_eq!(defaults.len(), 4);
    assert_eq!(
        defaults
            .iter()
            .map(|id| app.scene().a11y(*id).unwrap().name.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["Cut", "Copy", "Paste", "Select All"]
    );
    for id in &defaults[..3] {
        assert!(StateFlags(app.scene().a11y(*id).unwrap().state).contains(StateFlags::DISABLED));
    }
    assert!(
        !StateFlags(app.scene().a11y(defaults[3]).unwrap().state).contains(StateFlags::DISABLED)
    );
    assert!(app.dismiss_context_menu());

    let custom_fired = Rc::new(RefCell::new(0));
    let sink = custom_fired.clone();
    let custom = ContextMenu::new()
        .item(ContextMenuItem::new("Inspect").on_select(move || *sink.borrow_mut() += 1));
    let mut custom_app = App::mount(TextInput::new("x").context_menu(custom));
    custom_app.frame();
    let custom_input = custom_app.find_widget(Role::TextInput, None).unwrap();
    assert!(custom_app.open_text_context_menu(custom_input, Point { x: 10.0, y: 10.0 }, true));
    let custom_items = menu_items(custom_app.scene());
    assert_eq!(custom_items.len(), 1, "replacement omits default commands");
    assert_eq!(
        custom_app
            .scene()
            .a11y(custom_items[0])
            .unwrap()
            .name
            .as_deref(),
        Some("Inspect")
    );
    assert_eq!(
        custom_app
            .activate_context_menu_item(custom_items[0])
            .unwrap()
            .action,
        widgets::ContextMenuAction::Custom
    );
    assert_eq!(*custom_fired.borrow(), 1);
    assert!(!custom_app.context_menu_is_open());

    let mut appended_app = App::mount(
        TextInput::new("x").context_menu_item(ContextMenuItem::new("Inspect").on_select(|| {})),
    );
    appended_app.frame();
    let appended_input = appended_app.find_widget(Role::TextInput, None).unwrap();
    assert!(appended_app.open_text_context_menu(appended_input, Point { x: 10.0, y: 10.0 }, true));
    assert_eq!(
        menu_items(appended_app.scene()).len(),
        5,
        "append keeps the four default commands"
    );
}

#[test]
fn tab_context_menu_opens_through_the_generic_accessibility_action() {
    use schnellui_a11y::accesskit_reexport::{Action, ActionRequest, TreeId};
    use schnellui_a11y::to_access_id;
    use schnellui_widgets::{ContextMenu, ContextMenuItem, Tab};

    let menu = ContextMenu::new().item(ContextMenuItem::new("Close others").on_select(|| {}));
    let mut app = App::mount_with_size(Tab::new("Editor").context_menu(menu), 400, 300);
    app.frame();
    let tab = app.find_widget(Role::Tab, Some("Editor")).unwrap();
    assert!(app.dispatch_action(&ActionRequest {
        action: Action::ShowContextMenu,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(tab),
        data: None,
    }));
    assert!(app.context_menu_is_open());
    assert!(app.find_widget(Role::Menu, Some("Editor menu")).is_some());
    assert!(app
        .find_widget(Role::MenuItem, Some("Close others"))
        .is_some());
}

#[test]
fn combobox_search_filters_retained_rows_without_a_remount() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use schnellui_widgets::{ComboBox, ComboBoxOption};

    let accepted = Rc::new(RefCell::new(None));
    let accepted_value = accepted.clone();
    let mut app = App::mount_with_size(
        ComboBox::new("")
            .label("Fruit")
            .open(true)
            .allow_free_text(true)
            .option(ComboBoxOption::new("Apple"))
            .option(ComboBoxOption::new("Banana"))
            .option(ComboBoxOption::new("Grape"))
            .on_accept_free_text(move |value| {
                *accepted_value.borrow_mut() = Some(value.to_owned());
            }),
        400,
        300,
    );
    app.frame();
    let field = app.find_widget(Role::ComboBox, Some("Fruit")).unwrap();

    assert!(app.set_text_value(field, "ban"));
    app.frame();
    assert!(app
        .find_widget(Role::ListBoxOption, Some("Banana"))
        .is_some());
    assert!(app
        .find_widget(Role::ListBoxOption, Some("Apple"))
        .is_none());
    assert!(app.focus(Some(field)));
    assert!(app.dispatch_key(UiKey::Down { shift: false }));
    let focused = app.focused_widget().unwrap();
    assert_eq!(
        app.scene().a11y(focused).unwrap().name.as_deref(),
        Some("Banana"),
        "keyboard navigation must skip filtered-out retained rows"
    );
    let custom = app
        .find_widget(Role::ListBoxOption, Some("Use “ban”"))
        .expect("the retained free-text row tracks the live query");
    assert!(app.dispatch_click(custom));
    assert_eq!(accepted.borrow().as_deref(), Some("ban"));
}

/// Arrows on a focused radio move focus *and* selection within the group,
/// wrapping at the ends — the browser radio-group contract (SOUL §6.3).
#[test]
fn radio_arrows_move_and_select_within_the_group() {
    use schnellui_widgets::{Column, Radio};

    let view = Column::new()
        .child(Radio::new(true))
        .child(Radio::new(false))
        .child(Radio::new(false));
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();
    let root = app.scene().root().unwrap();
    let radios: Vec<_> = app.scene().node(root).unwrap().children.clone().to_vec();
    let checked = |app: &App, id| {
        schnellui_a11y::StateFlags(app.scene().a11y(id).unwrap().state)
            .contains(schnellui_a11y::StateFlags::CHECKED)
    };
    app.focus(Some(radios[0]));

    assert!(app.dispatch_key(UiKey::Down { shift: false }));
    assert_eq!(app.focused_widget(), Some(radios[1]));
    assert!(checked(&app, radios[1]) && !checked(&app, radios[0]));
    assert!(app.dispatch_key(UiKey::Right {
        shift: false,
        ctrl: false
    }));
    assert_eq!(app.focused_widget(), Some(radios[2]));
    assert!(checked(&app, radios[2]) && !checked(&app, radios[1]));
    // wraps forward to the first…
    assert!(app.dispatch_key(UiKey::Down { shift: false }));
    assert_eq!(app.focused_widget(), Some(radios[0]));
    assert!(checked(&app, radios[0]));
    // …and backward to the last.
    assert!(app.dispatch_key(UiKey::Up { shift: false }));
    assert_eq!(app.focused_widget(), Some(radios[2]));
    assert!(checked(&app, radios[2]));
}
