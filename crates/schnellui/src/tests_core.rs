use super::*;
use schnellui::a11y::Role;
use schnellui::scene::WidgetKind;

#[test]
fn terminal_content_repaints_without_layout_invalidation() {
    use widgets::{TerminalGrid, TerminalGridModel, TerminalGridPoint};

    let revision = schnellui::signal::create_signal(0_u64);
    let mut app = App::mount_with_size(
        TerminalGrid::dynamic_versioned(
            move || revision.get(),
            move || {
                let mut model = TerminalGridModel::new(2, 1);
                if revision.get() != 0 {
                    model
                        .cell_mut(TerminalGridPoint::new(0, 0))
                        .expect("terminal cell")
                        .grapheme = "X".into();
                }
                model
            },
        ),
        120,
        40,
    );
    app.frame();
    let terminal = app.scene().root().expect("terminal root");
    let blank_primitives = app.scene().paint(terminal).unwrap().primitives.len();

    revision.set(1);
    app.frame();

    assert!(
        app.scene().paint(terminal).unwrap().primitives.len() > blank_primitives,
        "paint-only terminal output must be emitted without a layout event"
    );
}

#[test]
fn versioned_image_replaces_pixels_without_layout_or_remount() {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use widgets::{DynamicImageFrame, Image};

    let revision = Rc::new(Cell::new(1_u64));
    let pixels = Rc::new(RefCell::new(DynamicImageFrame::new(2, 1, vec![0_u8; 8])));
    let revision_source = revision.clone();
    let frame_source = pixels.clone();
    let mut app = App::mount_with_size(
        Image::dynamic_rgba_versioned(
            move || revision_source.get(),
            move || Some(frame_source.borrow().clone()),
        )
        .size(80.0, 40.0),
        100,
        60,
    );
    app.frame();
    let atlas_revision = app.scene().images().revision();

    *pixels.borrow_mut() = DynamicImageFrame::new(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    revision.set(2);

    assert!(
        !app.settle_frame(),
        "a pixel-only update must not run layout"
    );
    assert!(app.scene().images().revision() > atlas_revision);
}

#[test]
fn remounted_images_give_their_atlas_regions_back() {
    use widgets::{
        Column, ComponentRef, Image, TerminalGrid, TerminalGridModel, TerminalImage,
        TerminalImagePlacement, View as _,
    };

    fn content() -> Column {
        let mut model = TerminalGridModel::new(4, 2);
        model.images.push(TerminalImage::new(
            7,
            96,
            96,
            vec![255; 96 * 96 * 4],
            TerminalImagePlacement::Cells {
                row: 0,
                column: 0,
                rows: 2,
                columns: 4,
            },
        ));
        Column::new()
            .child(Image::from_rgba(48, 48, vec![200; 48 * 48 * 4]))
            .child(TerminalGrid::new(model))
    }

    let slot = ComponentRef::new();
    let mut app = App::mount_with_size(Column::new().child(content().with_ref(slot)), 320, 240);
    app.frame();
    let size = (app.scene().images().width(), app.scene().images().height());
    // Without freeing, 40 remounts would need about 460k texels: far more than
    // the first atlas size.
    for _ in 0..40 {
        app.replace_subtree(slot, content()).unwrap();
        app.frame();
    }
    let images = app.scene().images();
    assert_eq!((images.width(), images.height()), size);
}

#[test]
fn subtree_replacement_preserves_parent_siblings_focus_and_scroll() {
    use widgets::{Column, Scroll, Text, TextInput, View as _};

    let editor_ref = scene::ComponentRef::new();
    let scroll_ref = scene::ComponentRef::new();
    let content_ref = scene::ComponentRef::new();
    let mut app = App::mount_with_size(
        Column::new()
            .child(
                TextInput::new("draft")
                    .placeholder("Composer")
                    .with_ref(editor_ref),
            )
            .child(
                Scroll::new()
                    .label("Transcript")
                    .size(180.0, 80.0)
                    .child(
                        Column::new()
                            .child(Text::new("old message"))
                            .with_ref(content_ref),
                    )
                    .with_ref(scroll_ref),
            ),
        240,
        160,
    );
    app.frame();
    let editor = app.resolve_ref(editor_ref).unwrap();
    let scroll = app.resolve_ref(scroll_ref).unwrap();
    let old_content = app.resolve_ref(content_ref).unwrap();
    assert!(app.focus(Some(editor)));
    let retained_offset = Point { x: 0.0, y: 17.0 };
    app.scene_mut().set_scroll_offset(scroll, retained_offset);

    let new_content = app
        .replace_subtree(
            content_ref,
            Column::new()
                .child(Text::new("new message"))
                .child(Text::new("stream tail")),
        )
        .unwrap();
    assert_eq!(app.scene().scroll_offset(scroll), retained_offset);
    app.frame();

    assert_ne!(new_content, old_content);
    assert!(app.scene().node(old_content).is_none());
    assert_eq!(app.resolve_ref(content_ref), Some(new_content));
    assert_eq!(app.resolve_ref(editor_ref), Some(editor));
    assert_eq!(app.resolve_ref(scroll_ref), Some(scroll));
    assert_eq!(app.focused_widget(), Some(editor));
    assert_eq!(
        app.scene().node(scroll).unwrap().children.as_slice(),
        [new_content]
    );
}

#[test]
fn subtree_replacement_keeps_end_following_viewports_pinned_to_the_end() {
    use widgets::{Column, Scroll, Text, View as _};

    let content_ref = scene::ComponentRef::new();
    let scroll_ref = scene::ComponentRef::new();
    fn transcript(
        content_ref: &scene::ComponentRef,
        scroll_ref: &scene::ComponentRef,
        rows: usize,
    ) -> Column {
        let mut content = Column::new();
        for row in 0..rows {
            content = content.child(Text::new(format!("Row {row}")));
        }
        Column::new().child(
            Scroll::new()
                .label("Transcript")
                .restoration_key("agent-a")
                .size(240.0, 100.0)
                .follow_end(true)
                .child(content.with_ref(content_ref.clone()))
                .with_ref(scroll_ref.clone()),
        )
    }

    let mut app = App::mount_with_size(transcript(&content_ref, &scroll_ref, 40), 320, 180);
    app.frame();
    let scroll = app.resolve_ref(scroll_ref.clone()).unwrap();
    assert!(schnellui::widgets::scroll_is_at_end(app.scene(), scroll));

    // Stream growth arrives as a subtree replacement of just the content.
    let mut grown = Column::new();
    for row in 0..60 {
        grown = grown.child(Text::new(format!("Row {row}")));
    }
    app.replace_subtree(content_ref.clone(), grown).unwrap();
    app.frame();
    assert!(
        schnellui::widgets::scroll_is_at_end(app.scene(), scroll),
        "an at-end follow_end viewport must stay pinned through replacement"
    );

    // Once the user scrolls away, replacement growth preserves the position.
    assert!(schnellui::widgets::dispatch_scroll(
        &app.widgets,
        &mut app.scene,
        scroll,
        -80.0,
    ));
    let reading_offset = app.scene().scroll_offset(scroll).y;
    let mut grown_again = Column::new();
    for row in 0..80 {
        grown_again = grown_again.child(Text::new(format!("Row {row}")));
    }
    app.replace_subtree(content_ref, grown_again).unwrap();
    app.frame();
    assert_eq!(
        app.scene().scroll_offset(scroll).y,
        reading_offset,
        "replacement must not pull a user away from a reading position"
    );
}

#[test]
fn unrelated_subtree_replacement_does_not_rearm_a_follow_end_scroll() {
    use widgets::{Column, ComponentRef, Scroll, Text, View as _};

    let header_ref = ComponentRef::new();
    let scroll_ref = ComponentRef::new();
    let mut rows = Column::new();
    for row in 0..40 {
        rows = rows.child(Text::new(format!("Row {row}")));
    }
    let mut app = App::mount_with_size(
        Column::new()
            .child(Text::new("Header A").with_ref(header_ref))
            .child(
                Scroll::new()
                    .size(240.0, 100.0)
                    .follow_end(true)
                    .child(rows)
                    .with_ref(scroll_ref),
            ),
        320,
        240,
    );
    app.frame();
    let scroll = app.resolve_ref(scroll_ref).unwrap();
    let before = app.scene().scroll_offset(scroll).y;
    assert!(before.is_finite() && before > 0.0);

    app.replace_subtree(header_ref, Text::new("Header B"))
        .unwrap();
    assert_eq!(
        app.scene().scroll_offset(scroll).y,
        before,
        "a sibling replacement must not arm an unrelated scroll sentinel"
    );
}

#[test]
fn virtual_list_retains_overlapping_rows_refreshes_one_key_and_bounds_mounts() {
    use widgets::{ComponentRef, Text, View as _, VirtualList, VirtualListController};

    let controller = VirtualListController::new(0_usize..1_000, 20.0);
    controller.overscan(0.0);
    let row_refs: Vec<_> = (0..1_000).map(|_| ComponentRef::new()).collect();
    let row_four_ref = row_refs[4];
    let scroll_ref = ComponentRef::new();
    let mut app = App::mount_with_size(
        VirtualList::new(controller.clone(), move |key| {
            Text::new(format!("row {key}")).with_ref(row_refs[*key])
        })
        .label("virtual transcript")
        .size(240.0, 100.0)
        .with_ref(scroll_ref),
        280,
        140,
    );
    app.frame();
    assert!(
        app.scene().len() < 32,
        "a 1,000 row transcript mounts only its pixel window"
    );

    let scroll = app.resolve_ref(scroll_ref).unwrap();
    let before = app.resolve_ref(row_four_ref).unwrap();
    assert!(schnellui::widgets::dispatch_scroll(
        &app.widgets,
        &mut app.scene,
        scroll,
        45.0,
    ));
    app.frame();
    let after_scroll = app.resolve_ref(row_four_ref).unwrap();
    assert_eq!(before, after_scroll, "overlapping keyed rows stay resident");

    controller.refresh(&4);
    app.frame();
    let after_refresh = app.resolve_ref(row_four_ref).unwrap();
    assert_ne!(after_scroll, after_refresh, "refresh replaces only its key");
}

#[test]
fn virtual_list_follow_end_rearms_when_streamed_rows_grow() {
    use widgets::{ComponentRef, Text, View as _, VirtualList, VirtualListController};

    let controller = VirtualListController::new(0_usize..80, 20.0);
    let scroll_ref = ComponentRef::new();
    let mut app = App::mount_with_size(
        VirtualList::new(controller.clone(), |key| Text::new(format!("row {key}")))
            .label("stream")
            .size(240.0, 100.0)
            .follow_end(true)
            .with_ref(scroll_ref),
        280,
        140,
    );
    app.frame();
    let scroll = app.resolve_ref(scroll_ref).unwrap();
    assert!(schnellui::widgets::scroll_is_at_end(app.scene(), scroll));
    controller.insert(80, 80);
    app.frame();
    assert!(
        schnellui::widgets::scroll_is_at_end(app.scene(), scroll),
        "a pinned virtual transcript follows controller growth"
    );
}

#[test]
fn virtual_list_tells_assistive_technology_where_an_insertion_moved_it() {
    use widgets::{ComponentRef, Text, View as _, VirtualList, VirtualListController};

    let controller = VirtualListController::new(10_usize..80, 20.0);
    let scroll_ref = ComponentRef::new();
    let mut app = App::mount_with_size(
        VirtualList::new(controller.clone(), |key| Text::new(format!("row {key}")))
            .label("anchored")
            .size(240.0, 100.0)
            .with_ref(scroll_ref),
        280,
        140,
    );
    app.frame();
    let scroll = app.resolve_ref(scroll_ref).unwrap();
    assert!(app.dispatch_scroll(scroll, 100.0));
    app.frame();
    let value = |app: &App| app.scene().a11y(scroll).and_then(|a| a.value.clone());
    assert_eq!(value(&app).as_deref(), Some("100"));

    // A row inserted above keeps the rows in view where they were, by moving
    // the offset a row down.
    controller.insert(0, 5);
    app.frame();
    let offset = app.scene().scroll_offset(scroll).y;
    assert!(offset > 100.0, "the insertion moved the offset to {offset}");
    assert_eq!(
        value(&app),
        Some(offset.round().to_string()),
        "assistive technology reads the offset the list moved to"
    );
}

#[test]
fn virtual_list_follow_end_bootstraps_the_tail_not_the_first_row() {
    use widgets::{ComponentRef, Text, View as _, VirtualList, VirtualListController};

    let controller = VirtualListController::new(0_usize..80, 20.0);
    controller.overscan(0.0);
    let rows: Vec<_> = (0..80).map(|_| ComponentRef::new()).collect();
    let first = rows[0];
    let last = rows[79];
    let mut app = App::mount_with_size(
        VirtualList::new(controller, move |key| {
            Text::new(format!("row {key}")).with_ref(rows[*key])
        })
        .size(240.0, 100.0)
        .follow_end(true),
        280,
        140,
    );
    app.frame();
    assert!(
        app.resolve_ref(last).is_some(),
        "tail is mounted at bootstrap"
    );
    assert!(
        app.resolve_ref(first).is_none(),
        "initial row is discarded after tail handoff"
    );
}

#[test]
fn virtual_list_follow_end_stays_on_tail_after_a_giant_row_measures() {
    use widgets::{Button, ComponentRef, View as _, VirtualList, VirtualListController};

    // Start with the giant first event so it is measured, then stream the tail.
    // This matches a transcript that is already following its end when later
    // events arrive.
    let controller = VirtualListController::new(0_usize..1, 20.0);
    controller.overscan(0.0);
    let rows: Vec<_> = (0..80).map(|_| ComponentRef::new()).collect();
    let first = rows[0];
    let last = rows[79];
    let scroll_ref = ComponentRef::new();
    let mut app = App::mount_with_size(
        VirtualList::new(controller.clone(), move |key| {
            Button::new(format!("row {key}"))
                .min_height(if *key == 0 { 25_000.0 } else { 20.0 })
                .with_ref(rows[*key])
        })
        .size(240.0, 100.0)
        .follow_end(true)
        .with_ref(scroll_ref),
        280,
        140,
    );

    app.frame();
    assert!(
        controller.estimated_height() >= 25_000.0,
        "first row was not measured: {}",
        controller.estimated_height()
    );
    controller.replace(0_usize..80);
    assert!(
        controller.estimated_height() >= 25_000.0,
        "replace lost the measured row: {}",
        controller.estimated_height()
    );
    app.frame();
    app.frame();

    let scroll = app.resolve_ref(scroll_ref).unwrap();
    assert!(
        schnellui::widgets::scroll_is_at_end(app.scene(), scroll),
        "the scene stays pinned after the first row expands far past its estimate"
    );
    assert!(
        app.resolve_ref(last).is_some(),
        "the tail remains mounted; estimate={}, offset={}, first={:?}",
        controller.estimated_height(),
        app.scene().scroll_offset(scroll).y,
        app.resolve_ref(first)
    );
    assert!(
        app.resolve_ref(first).is_none(),
        "the stale first-row window is reconciled away"
    );
}

#[test]
fn virtual_list_keeps_a_mounted_row_visible_during_a_long_upward_scroll() {
    use widgets::{Button, ComponentRef, View as _, VirtualList, VirtualListController};

    let controller = VirtualListController::new(0_usize..96, 64.0);
    controller.overscan(320.0);
    let rows: Vec<_> = (0..96).map(|_| ComponentRef::new()).collect();
    let row_refs = rows.clone();
    let scroll_ref = ComponentRef::new();
    let mut app = App::mount_with_size(
        VirtualList::new(controller, move |key| {
            Button::new(format!("row {key}"))
                .min_height(if *key % 9 == 0 { 1_900.0 } else { 48.0 })
                .with_ref(rows[*key])
        })
        .size(240.0, 400.0)
        .follow_end(true)
        .with_ref(scroll_ref),
        280,
        440,
    );
    app.frame();

    let scroll = app.resolve_ref(scroll_ref).unwrap();
    assert!(schnellui::widgets::scroll_is_at_end(app.scene(), scroll));
    let mut completed_steps = 0;
    for step in 0..512 {
        if !schnellui::widgets::dispatch_scroll(
            &app.widgets.clone(),
            app.scene_mut(),
            scroll,
            -240.0,
        ) {
            break;
        }
        app.frame();
        completed_steps += 1;

        let viewport = app.scene().layout(scroll).unwrap().rect;
        let offset = app.scene().scroll_offset(scroll).y;
        let row_intersects = row_refs.iter().any(|row| {
            app.resolve_ref(*row)
                .and_then(|id| app.scene().layout(id))
                .is_some_and(|layout| {
                    layout.rect.bottom() - offset > viewport.y
                        && layout.rect.y - offset < viewport.bottom()
                })
        });
        assert!(
            row_intersects,
            "the virtual window went blank at upward scroll step {step}, offset={offset}"
        );
    }
    assert!(
        completed_steps > 20,
        "the stress scroll must cross several variable-height windows"
    );
}

#[test]
fn dispatch_wheel_at_bubbles_past_an_unscrollable_nested_viewport() {
    use widgets::{Column, ComponentRef, Scroll, Text, View as _};

    let outer_ref = ComponentRef::new();
    let inner_ref = ComponentRef::new();
    let mut app = App::mount_with_size(
        Scroll::new()
            .size(240.0, 180.0)
            .child(
                Column::new()
                    .child(
                        Scroll::new()
                            .size(120.0, 80.0)
                            .child(Text::new("short inner content"))
                            .with_ref(inner_ref),
                    )
                    .child(Column::new().height(600.0)),
            )
            .with_ref(outer_ref),
        280,
        220,
    );
    app.frame();

    let outer = app.resolve_ref(outer_ref).unwrap();
    let inner = app.resolve_ref(inner_ref).unwrap();
    let rect = app.scene().layout(inner).unwrap().rect;
    let point = scene::Point {
        x: rect.x + 4.0,
        y: rect.y + 4.0,
    };

    assert!(app.dispatch_wheel_at(point, 48.0));
    assert_eq!(app.scene().scroll_offset(inner).y, 0.0);
    assert_eq!(app.scene().scroll_offset(outer).y, 48.0);

    assert!(app.dispatch_wheel_at(point, -48.0));
    assert_eq!(app.scene().scroll_offset(outer).y, 0.0);
}
#[test]
fn set_signal_routes_to_registered_setter() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut app = App::new(100, 100);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    app.register_signal("count", move |v| sink.borrow_mut().push(v));

    assert!(app.set_signal("count", 42));
    assert!(!app.set_signal("missing", 1));
    assert_eq!(*seen.borrow(), vec![TestValue::Int(42)]);
}

#[test]
fn application_shortcuts_dispatch_by_normalized_chord() {
    use std::cell::Cell;
    use std::rc::Rc;

    let mut app = App::new(100, 100);
    let calls = Rc::new(Cell::new(0));
    let sink = calls.clone();
    app.register_shortcut(Shortcut::command('K'), move || sink.set(sink.get() + 1));

    assert!(app.dispatch_shortcut(Shortcut::command('k')));
    assert_eq!(calls.get(), 1);
    assert!(!app.dispatch_shortcut(Shortcut::command(',')));
}

/// Sequence (calculator): type 1 + 2, press Enter. The display stayed at 2
/// although the app bound Enter to equals with a '\r' shortcut: Enter is a
/// named key, and only typed characters reached shortcuts. After clicking
/// the 7 key, Enter pressed 7 again instead.
#[test]
fn an_enter_shortcut_runs_before_the_focused_button_but_not_in_a_text_field() {
    use schnellui::widgets::{Button, Column, TextArea};
    use std::cell::Cell;
    use std::rc::Rc;

    let pressed = Rc::new(Cell::new(0));
    let p = pressed.clone();
    let mut app = App::mount_with_size(
        Column::new()
            .child(Button::new("7").on_click(move || p.set(p.get() + 1)))
            .child(TextArea::new("")),
        300,
        200,
    );
    app.frame();
    let equals = Rc::new(Cell::new(0));
    let e = equals.clone();
    app.register_shortcut(Shortcut::new('\r', false, false, false), move || {
        e.set(e.get() + 1)
    });

    assert!(app.dispatch_key(UiKey::Enter), "nothing focused");
    assert_eq!(equals.get(), 1);

    let seven = app.find_widget(Role::Button, Some("7")).unwrap();
    assert!(app.dispatch_click(seven));
    assert_eq!(pressed.get(), 1);
    assert!(app.focus(Some(seven)));
    assert!(app.dispatch_key(UiKey::Enter), "a focused button");
    assert_eq!(equals.get(), 2);
    assert_eq!(pressed.get(), 1, "Enter did not press 7 again");

    let area = app.find_widget(Role::MultilineTextInput, None).unwrap();
    assert!(app.focus(Some(area)));
    assert!(app.dispatch_key(UiKey::Enter));
    assert_eq!(equals.get(), 2, "Enter in a text area stays a newline");
    assert_eq!(app.scene().a11y(area).unwrap().value.as_deref(), Some("\n"));
}

#[test]
fn navigation_reaches_the_registered_handler() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut app = App::new(100, 100);
    assert!(!app.dispatch_navigation(Navigation::Back));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    app.register_navigation_handler(move |navigation| {
        sink.borrow_mut().push(navigation);
        navigation == Navigation::Back
    });

    assert!(app.dispatch_navigation(Navigation::Back));
    assert!(!app.dispatch_navigation(Navigation::Forward));
    assert_eq!(*seen.borrow(), [Navigation::Back, Navigation::Forward]);
}

#[test]
fn dump_a11y_reflects_scene() {
    let mut app = App::new(100, 100);
    let root = app.scene_mut().insert(WidgetKind::Button, None);
    app.scene_mut().set_root(root);
    app.scene_mut().a11y_mut(root).role = Role::Button.as_u16();
    app.scene_mut().a11y_mut(root).name = Some("increment".into());

    let json = app.a11y_json();
    assert!(json.contains("\"button\""));
    assert!(json.contains("increment"));
}

#[test]
fn dynamic_cursor_provider_is_scoped_to_its_widget_subtree() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::widgets::{CursorIcon, Image, Scroll};

    let mut app = App::mount_with_size(
        Scroll::new()
            .label("Browser viewport")
            .size(200.0, 100.0)
            .child(Image::new("page").size(200.0, 100.0)),
        320,
        180,
    );
    app.frame();
    let cursor = Rc::new(Cell::new(CursorIcon::Pointer));
    let current = cursor.clone();
    assert!(app
        .register_cursor_provider(Role::ScrollView, Some("Browser viewport"), move || current
            .get(),));

    assert_eq!(
        app.cursor_at(Point { x: 50.0, y: 50.0 }),
        CursorIcon::Pointer
    );
    cursor.set(CursorIcon::Text);
    assert_eq!(app.cursor_at(Point { x: 50.0, y: 50.0 }), CursorIcon::Text);
    assert_eq!(
        app.cursor_at(Point { x: 300.0, y: 150.0 }),
        CursorIcon::Default
    );
}

/// The windowed-mode contract (SOUL §8): a `fill` root IS the viewport, and
/// [`App::resize`] — what a winit `Resized` event calls — re-derives it, so the
/// laid-out tree always matches the real window size.
#[test]
fn resize_rederives_a_filled_root_to_the_new_viewport() {
    use schnellui::scene::Rect;
    use schnellui::widgets::{Column, Text};

    let view = Column::new().fill().child(Text::new("x"));
    let mut app = App::mount_with_size_scaled(view, 400, 300, 1.0);
    app.frame();
    let root = app.scene().root().unwrap();
    assert_eq!(
        app.scene().layout(root).unwrap().rect,
        Rect::new(0.0, 0.0, 400.0, 300.0)
    );

    // the exact call the windowed `Resized` handler makes.
    app.resize(640.0, 200.0);
    app.frame();
    assert_eq!(
        app.scene().layout(root).unwrap().rect,
        Rect::new(0.0, 0.0, 640.0, 200.0)
    );
}

#[test]
fn responsive_views_follow_viewport_queries_across_resize() {
    use schnellui::layout::{em, px, ResponsiveQuery};
    use schnellui::widgets::{Button, Column, View as _};

    let view = Column::new()
        .child(Button::new("wide").show_when(ResponsiveQuery::viewport().min_width(em(30.0))))
        .child(Button::new("compact").show_when(ResponsiveQuery::viewport().max_width(px(479.0))));
    let mut app = App::mount_with_size_scaled(view, 640, 240, 1.0);
    app.frame();

    let wide = app.find_widget(Role::Button, Some("wide")).unwrap();
    assert!(app.find_widget(Role::Button, Some("compact")).is_none());

    app.resize(420.0, 240.0);
    app.frame();
    assert!(app.find_widget(Role::Button, Some("wide")).is_none());
    assert!(app.find_widget(Role::Button, Some("compact")).is_some());
    assert!(!app.scene().is_effectively_visible(wide));
    assert!(!app.a11y_json().contains("\"wide\""));

    let order = schnellui::a11y::tab_order(app.scene());
    assert_eq!(order.len(), 1, "only the visible button is focusable");
}

#[test]
fn view_macro_accepts_show_when_on_any_component() {
    use schnellui::layout::ResponsiveQuery;

    let view = view! {
        button(show_when = ResponsiveQuery::viewport().min_width(500.0)) { "desktop" }
    };
    let mut app = App::mount_with_size_scaled(view, 400, 200, 1.0);
    app.frame();
    assert!(app.find_widget(Role::Button, Some("desktop")).is_none());
    app.resize(600.0, 200.0);
    app.frame();
    assert!(app.find_widget(Role::Button, Some("desktop")).is_some());
}

#[test]
fn component_refs_resolve_and_drive_named_container_queries() {
    use schnellui::layout::ResponsiveQuery;
    use schnellui::scene::ComponentRef;

    let card_ref = ComponentRef::new();
    let view = view! {
        component_ref(value = card_ref) {
            column(fill) {
                button(
                    show_when = ResponsiveQuery::component(card_ref).max_width(400.0)
                ) { "card action" }
            }
        }
    };
    let mut app = App::mount_with_size_scaled(view, 600, 240, 1.0);
    app.frame();

    assert_eq!(app.resolve_ref(card_ref), app.scene().root());
    assert!(app.find_widget(Role::Button, Some("card action")).is_none());

    app.resize(320.0, 240.0);
    app.frame();
    assert!(app.find_widget(Role::Button, Some("card action")).is_some());
}

#[test]
fn content_drag_preserves_clicks_and_previews_an_accepted_drop() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::scene::Point;
    use schnellui::widgets::{Button, DragRelease, Row};

    let clicks = Rc::new(Cell::new(0));
    let starts = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let accepted = Rc::new(Cell::new(false));
    let view = Row::new()
        .gap(80.0)
        .child(
            Button::new("source")
                .on_click({
                    let clicks = clicks.clone();
                    move || clicks.set(clicks.get() + 1)
                })
                .on_drag_start({
                    let starts = starts.clone();
                    move || starts.set(starts.get() + 1)
                })
                .on_drag_end({
                    let accepted = accepted.clone();
                    move |value| accepted.set(value)
                }),
        )
        .child(Button::new("target").on_drop({
            let drops = drops.clone();
            move || drops.set(drops.get() + 1)
        }));
    let mut app = App::mount_with_size(view, 400, 120);
    app.frame();

    let center = |app: &App, name: &str| {
        let id = app.find_widget(Role::Button, Some(name)).unwrap();
        let rect = app.scene().layout(id).unwrap().rect;
        (
            id,
            Point {
                x: rect.x + rect.width * 0.5,
                y: rect.y + rect.height * 0.5,
            },
        )
    };
    let (source, from) = center(&app, "source");
    let (target, to) = center(&app, "target");

    // A press/release without enough travel stays a normal click.
    assert!(app.begin_drag(from));
    assert_eq!(app.end_drag(from), DragRelease::Click(source));
    let widgets = app.widgets.clone();
    assert!(schnellui::widgets::dispatch_click(
        &widgets,
        app.scene_mut(),
        source
    ));
    assert_eq!(clicks.get(), 1);
    assert_eq!(starts.get(), 0);

    let before = app.scene().paint(target).unwrap().primitives.len();
    assert!(app.begin_drag(from));
    assert!(app.update_drag(to));
    assert_eq!(starts.get(), 1);
    assert_eq!(
        app.scene().paint(target).unwrap().primitives.len(),
        before + 4,
        "hovered target wears the four-edge preview ring"
    );
    assert_eq!(app.end_drag(to), DragRelease::Drop { accepted: true });
    assert_eq!(app.scene().paint(target).unwrap().primitives.len(), before);
    assert_eq!(drops.get(), 1);
    assert!(accepted.get());
    assert_eq!(clicks.get(), 1, "a real drag never also clicks");
}

#[test]
fn escape_cancels_a_content_drag_before_it_drops() {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use schnellui::scene::Point;
    use schnellui::widgets::{Button, DragRelease, Row, TextInput};

    let clicks = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let ends = Rc::new(RefCell::new(Vec::new()));
    let escapes = Rc::new(Cell::new(0));
    let view = Row::new()
        .gap(80.0)
        .child(
            Button::new("source")
                .on_click({
                    let clicks = clicks.clone();
                    move || clicks.set(clicks.get() + 1)
                })
                .on_drag_start(|| {})
                .on_drag_end({
                    let ends = ends.clone();
                    move |accepted| ends.borrow_mut().push(accepted)
                }),
        )
        .child(Button::new("target").on_drop({
            let drops = drops.clone();
            move || drops.set(drops.get() + 1)
        }))
        .child(TextInput::new("").label("Note"));
    let mut app = App::mount_with_size(view, 600, 120);
    app.register_focused_key_handler(Role::TextInput, Some("Note"), {
        let escapes = escapes.clone();
        move |key| {
            let escape = key == UiKey::Escape;
            if escape {
                escapes.set(escapes.get() + 1);
            }
            escape
        }
    });
    app.frame();

    let center = |app: &App, role: Role, name: &str| {
        let id = app.find_widget(role, Some(name)).unwrap();
        let rect = app.scene().layout(id).unwrap().rect;
        (
            id,
            Point {
                x: rect.x + rect.width * 0.5,
                y: rect.y + rect.height * 0.5,
            },
        )
    };
    let (_, from) = center(&app, Role::Button, "source");
    let (target, to) = center(&app, Role::Button, "target");
    let (note, _) = center(&app, Role::TextInput, "Note");
    let before = app.scene().paint(target).unwrap().primitives.len();

    // Escape mid-drag drops nothing, even while a widget that handles
    // Escape itself holds focus, and the release that follows is inert.
    assert!(app.focus(Some(note)));
    assert!(app.begin_drag(from));
    assert!(app.update_drag(to));
    assert!(app.dispatch_key(UiKey::Escape), "Escape cancels the drag");
    assert_eq!(escapes.get(), 0, "the drag takes Escape first");
    assert_eq!(app.scene().paint(target).unwrap().primitives.len(), before);
    assert_eq!(*ends.borrow(), vec![false]);
    assert_eq!(app.end_drag(to), DragRelease::None);
    assert_eq!(drops.get(), 0);
    assert_eq!(clicks.get(), 0);

    // The window's Escape ladder cancels it the same way.
    assert!(app.begin_drag(from));
    assert!(app.update_drag(to));
    assert_eq!(app.dispatch_escape(), Some(true));
    assert_eq!(app.end_drag(to), DragRelease::None);
    assert_eq!(*ends.borrow(), vec![false, false]);
    assert_eq!(drops.get(), 0);

    // A press that never crossed the threshold is not a drag yet: Escape
    // leaves it alone and the release still clicks.
    assert!(app.begin_drag(from));
    assert!(app.dispatch_key(UiKey::Escape));
    assert_eq!(escapes.get(), 1);
    assert!(matches!(app.end_drag(from), DragRelease::Click(_)));
}

#[test]
fn dock_area_resolves_right_edge_and_previews_the_result_above_content() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::scene::Point;
    use schnellui::widgets::{Button, DockArea, DockPosition, DragRelease, Row};

    let result = Rc::new(Cell::new(None));
    let view = Row::new()
        .gap(40.0)
        .child(Button::new("source").on_drag_start(|| {}))
        .child(
            DockArea::new("target pane")
                .size(220.0, 120.0)
                .on_dock({
                    let result = result.clone();
                    move |position| result.set(Some(position))
                })
                .child(Button::new("pane content")),
        );
    let mut app = App::mount_with_size(view, 400, 160);
    app.frame();

    let source = app.find_widget(Role::Button, Some("source")).unwrap();
    let source_rect = app.scene().layout(source).unwrap().rect;
    let from = Point {
        x: source_rect.x + source_rect.width * 0.5,
        y: source_rect.y + source_rect.height * 0.5,
    };
    let dock = app.find_widget(Role::Group, Some("target pane")).unwrap();
    let dock_rect = app.scene().layout(dock).unwrap().rect;
    let to = Point {
        x: dock_rect.x + dock_rect.width * 0.95,
        y: dock_rect.y + dock_rect.height * 0.5,
    };
    let preview = *app.scene().node(dock).unwrap().children.last().unwrap();
    assert!(app
        .scene()
        .paint(preview)
        .is_none_or(|paint| paint.primitives.is_empty()));

    assert!(app.begin_drag(from));
    assert!(app.update_drag(to));
    assert_eq!(
        app.scene().paint(preview).unwrap().primitives.len(),
        5,
        "wash plus four-edge outline paint on the topmost preview layer"
    );
    assert_eq!(app.end_drag(to), DragRelease::Drop { accepted: true });
    assert_eq!(result.get(), Some(DockPosition::Right));
    assert_eq!(app.scene().paint(preview).unwrap().primitives.len(), 0);
}

/// Tab must hop over a disabled widget instead of stalling on it (SOUL §6.3):
/// a disabled button never takes focus, so if the tab order listed it, Tab
/// would clear focus there and the next Tab would restart at the top — the
/// exact stuck loop this guards against. Shift+Tab wraps past it too.
#[test]
fn focus_step_skips_disabled_widgets_and_wraps() {
    use schnellui::widgets::{Button, Column};

    let view = Column::new()
        .child(Button::new("first"))
        .child(Button::new("blocked").disabled(true))
        .child(Button::new("last"));
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();

    let first = app.find_widget(Role::Button, Some("first")).unwrap();
    let blocked = app.find_widget(Role::Button, Some("blocked")).unwrap();
    let last = app.find_widget(Role::Button, Some("last")).unwrap();

    assert!(app.focus_step(false)); // nothing focused → first
    assert_eq!(app.focused_widget(), Some(first));
    assert!(app.focus_step(false)); // skips the disabled button entirely
    assert_eq!(app.focused_widget(), Some(last));
    assert!(app.focus_step(false)); // wraps to the front, still skipping
    assert_eq!(app.focused_widget(), Some(first));
    assert!(app.focus_step(true)); // backwards wrap skips it too
    assert_eq!(app.focused_widget(), Some(last));
    assert_ne!(app.focused_widget(), Some(blocked));
}

/// A radio group is one Tab stop: Tab enters it at the checked radio and the
/// next Tab leaves it for the following control, never back to the top.
#[test]
fn focus_step_leaves_a_radio_group_for_the_next_control() {
    use schnellui::widgets::{Button, Radio, Row};

    let view = schnellui::widgets::Column::new()
        .child(Button::new("before"))
        .child(
            Row::new()
                .child(Radio::new(true).name("Daily"))
                .child(Radio::new(false).name("Weekly")),
        )
        .child(Button::new("after"));
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();

    let before = app.find_widget(Role::Button, Some("before")).unwrap();
    let daily = app.find_widget(Role::Radio, Some("Daily")).unwrap();
    let after = app.find_widget(Role::Button, Some("after")).unwrap();

    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(before));
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(daily));
    assert!(app.focus_step(false), "Tab leaves the group");
    assert_eq!(app.focused_widget(), Some(after));
    assert!(
        app.focus_step(true),
        "Shift+Tab re-enters at the checked radio"
    );
    assert_eq!(app.focused_widget(), Some(daily));
}

/// `arrows_select(false)`: arrows walk the group without selecting, Space selects.
#[test]
fn radios_without_arrow_selection_move_focus_only() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::widgets::{Column, Radio};

    let picks = Rc::new(Cell::new(0));
    let (a, b) = (picks.clone(), picks.clone());
    let view = Column::new()
        .child(
            Radio::new(true)
                .name("CPU")
                .arrows_select(false)
                .on_select(move || a.set(a.get() + 1)),
        )
        .child(
            Radio::new(false)
                .name("GPU")
                .arrows_select(false)
                .on_select(move || b.set(b.get() + 1)),
        );
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();

    let cpu = app.find_widget(Role::Radio, Some("CPU")).unwrap();
    let gpu = app.find_widget(Role::Radio, Some("GPU")).unwrap();
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(cpu));
    assert!(app.dispatch_key(UiKey::Down { shift: false }));
    assert_eq!(app.focused_widget(), Some(gpu));
    assert_eq!(picks.get(), 0, "an arrow only moves focus");
    assert!(app.dispatch_key(UiKey::Space { shift: false }));
    assert_eq!(picks.get(), 1, "Space selects");
}

#[test]
fn modal_dialog_traps_focus_blocks_background_actions_and_takes_escape() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::widgets::{Button, Column, Dialog};

    let background_clicks = Rc::new(Cell::new(0));
    let background_sink = background_clicks.clone();
    let dismissed = Rc::new(Cell::new(false));
    let dismiss_sink = dismissed.clone();
    let view = Column::new()
        .fill()
        .child(
            Button::new("background")
                .on_click(move || background_sink.set(background_sink.get() + 1)),
        )
        .child(
            Dialog::new("Confirm")
                .child(Button::new("cancel"))
                .child(Button::new("continue"))
                .on_dismiss(move || dismiss_sink.set(true)),
        );
    let mut app = App::mount_with_size(view, 800, 600);
    app.frame();

    let background = app.find_widget(Role::Button, Some("background")).unwrap();
    let cancel = app.find_widget(Role::Button, Some("cancel")).unwrap();
    let continue_button = app.find_widget(Role::Button, Some("continue")).unwrap();

    // A focus-grabbing dialog owns focus from its first mounted frame.
    assert_eq!(app.focused_widget(), Some(cancel));
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(continue_button));
    // The title bar's close button is the stop before the content.
    let close = app
        .find_widget(Role::Button, Some("Close Confirm"))
        .unwrap();
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(close));
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(cancel));

    // The dialog is the complete screen-reader tree while it is modal.
    let dump = schnellui::a11y::dump_tree(app.scene());
    let exposed = dump.root.expect("modal accessibility root");
    assert_eq!(exposed.role, "dialog");
    assert_eq!(exposed.name.as_deref(), Some("Confirm"));
    assert_eq!(dump.focus, Some(schnellui::a11y::to_access_id(cancel).0));
    fn contains_name(node: &schnellui::a11y::A11yNodeDump, name: &str) -> bool {
        node.name.as_deref() == Some(name)
            || node.children.iter().any(|child| contains_name(child, name))
    }
    assert!(!contains_name(&exposed, "background"));

    // Programmatic and assistive-technology paths cannot escape the modal.
    assert!(!app.focus(Some(background)));
    assert_eq!(app.focused_widget(), Some(cancel));
    let request = accesskit_action::ActionRequest {
        action: accesskit_action::Action::Click,
        target_tree: accesskit_reexport::TreeId::ROOT,
        target_node: schnellui::a11y::to_access_id(background),
        data: None,
    };
    assert!(!app.dispatch_action(&request));
    assert_eq!(background_clicks.get(), 0);

    assert!(app.dispatch_key(UiKey::Escape));
    assert!(dismissed.get());
}

#[test]
fn highest_modal_owns_focus_accessibility_and_input_above_dialog_peers() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::scene::Point;
    use schnellui::widgets::{Button, Column, Dialog};

    let background_clicks = Rc::new(Cell::new(0));
    let lower_clicks = Rc::new(Cell::new(0));
    let peer_clicks = Rc::new(Cell::new(0));
    let top_clicks = Rc::new(Cell::new(0));
    let lower_dismissed = Rc::new(Cell::new(false));
    let peer_dismissed = Rc::new(Cell::new(false));
    let top_dismissed = Rc::new(Cell::new(false));

    let view = Column::new()
        .fill()
        .child({
            let clicks = background_clicks.clone();
            Button::new("workspace action").on_click(move || clicks.set(clicks.get() + 1))
        })
        .child(
            Dialog::new("Lower modal")
                .child({
                    let clicks = lower_clicks.clone();
                    Button::new("lower action").on_click(move || clicks.set(clicks.get() + 1))
                })
                .on_dismiss({
                    let dismissed = lower_dismissed.clone();
                    move || dismissed.set(true)
                }),
        )
        .child(
            Dialog::new("Top confirmation")
                .alert()
                .child({
                    let clicks = top_clicks.clone();
                    Button::new("top cancel").on_click(move || clicks.set(clicks.get() + 1))
                })
                .child(Button::new("top continue"))
                .on_dismiss({
                    let dismissed = top_dismissed.clone();
                    move || dismissed.set(true)
                }),
        )
        // A later modeless peer remains visually below and semantically inert
        // until the focus-grabbing modal stack has closed.
        .child(
            Dialog::new("Side inspector")
                .modeless()
                .child({
                    let clicks = peer_clicks.clone();
                    Button::new("peer action").on_click(move || clicks.set(clicks.get() + 1))
                })
                .on_dismiss({
                    let dismissed = peer_dismissed.clone();
                    move || dismissed.set(true)
                }),
        );
    let mut app = App::mount_with_size(view, 800, 600);
    app.frame();

    let background = app
        .find_widget(Role::Button, Some("workspace action"))
        .unwrap();
    let lower = app.find_widget(Role::Button, Some("lower action")).unwrap();
    let peer = app.find_widget(Role::Button, Some("peer action")).unwrap();
    let top_cancel = app.find_widget(Role::Button, Some("top cancel")).unwrap();
    let top_continue = app.find_widget(Role::Button, Some("top continue")).unwrap();

    assert_eq!(app.focused_widget(), Some(top_cancel));
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(top_continue));
    // The title bar's close button is the stop before the content.
    let top_close = app
        .find_widget(Role::Button, Some("Close Top confirmation"))
        .unwrap();
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(top_close));
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(top_cancel));

    let exposed = schnellui::a11y::dump_tree(app.scene())
        .root
        .expect("top modal accessibility root");
    assert_eq!(exposed.role, "alert_dialog");
    assert_eq!(exposed.name.as_deref(), Some("Top confirmation"));

    let click = |id| accesskit_action::ActionRequest {
        action: accesskit_action::Action::Click,
        target_tree: accesskit_reexport::TreeId::ROOT,
        target_node: schnellui::a11y::to_access_id(id),
        data: None,
    };
    assert!(!app.dispatch_action(&click(background)));
    assert!(!app.dispatch_action(&click(lower)));
    assert!(!app.dispatch_action(&click(peer)));
    assert!(app.dispatch_action(&click(top_cancel)));
    assert_eq!(background_clicks.get(), 0);
    assert_eq!(lower_clicks.get(), 0);
    assert_eq!(peer_clicks.get(), 0);
    assert_eq!(top_clicks.get(), 1);

    assert!(!app.focus(Some(background)));
    assert_eq!(app.focused_widget(), Some(top_cancel));
    let scrim_hit =
        schnellui::widgets::hit_test(&app.widgets, app.scene(), Point { x: 1.0, y: 1.0 })
            .expect("active modal scrim captures outside pointer input");
    assert_eq!(
        app.scene().node(scrim_hit).map(|node| node.kind),
        Some(WidgetKind::DialogLayer)
    );

    assert!(app.dispatch_key(UiKey::Escape));
    assert!(top_dismissed.get());
    assert!(!lower_dismissed.get());
    assert!(!peer_dismissed.get());
}

#[test]
fn modeless_dialogs_can_coexist_in_the_global_focus_and_reader_order() {
    use schnellui::widgets::{Button, Column, Dialog};

    let view = Column::new()
        .fill()
        .child(Button::new("workspace"))
        .child(
            Dialog::new("Left inspector")
                .modeless()
                .non_fixed()
                .child(Button::new("left action")),
        )
        .child(
            Dialog::new("Right inspector")
                .modeless()
                .non_fixed()
                .child(Button::new("right action")),
        );
    let mut app = App::mount_with_size(view, 800, 600);
    app.frame();

    let workspace = app.find_widget(Role::Button, Some("workspace")).unwrap();
    let left = app.find_widget(Role::Button, Some("left action")).unwrap();
    let right = app.find_widget(Role::Button, Some("right action")).unwrap();
    assert_eq!(app.focused_widget(), None);
    assert_eq!(
        schnellui::a11y::tab_order(app.scene()),
        vec![workspace, left, right]
    );

    let dump = schnellui::a11y::dump_tree(app.scene());
    assert_ne!(
        dump.root.as_ref().and_then(|node| node.name.as_deref()),
        Some("Left inspector")
    );
    let json = schnellui::a11y::dump_json(app.scene());
    assert!(json.contains("Left inspector"));
    assert!(json.contains("Right inspector"));
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(workspace));
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(left));
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(right));
}

#[test]
fn test_value_conversions() {
    assert_eq!(TestValue::from(5i32), TestValue::Int(5));
    assert_eq!(TestValue::from(true), TestValue::Bool(true));
    assert_eq!(TestValue::from("x"), TestValue::Text("x".into()));
}

/// An inbound `ScrollDown` action moves the viewport by one `SCROLL_STEP` and
/// updates its accessible value; `ScrollUp` at the top is a no-op (SOUL §6.3, §3.2).
/// The action is located by its `Role::ScrollView` and routed through the *same*
/// inbound path wheel input takes.
#[test]
fn scroll_action_moves_offset_and_up_at_top_is_noop() {
    use schnellui::a11y::accesskit_reexport::{Action, ActionRequest, TreeId};
    use schnellui::a11y::to_access_id;
    use schnellui::widgets::{Column, Scroll, Text};

    // A scroll viewport with content taller than the 220px box (25 rows).
    let view = {
        let mut col = Column::new().gap(2.0);
        for i in 0..25 {
            col = col.child(Text::new(format!("Row {i}")));
        }
        Scroll::new().size(320.0, 220.0).child(col)
    };
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame(); // lay out so the viewport + content have rects to scroll against

    let sv = app
        .find_widget(Role::ScrollView, None)
        .expect("mounted scroll view");
    let target = to_access_id(sv);

    // ScrollUp at the top (offset 0) reveals nothing → no-op returning false.
    let up = ActionRequest {
        action: Action::ScrollUp,
        target_tree: TreeId::ROOT,
        target_node: target,
        data: None,
    };
    assert!(!app.dispatch_action(&up));
    assert_eq!(app.scene().scroll_offset(sv).y, 0.0);

    // ScrollDown advances the offset by exactly one notch and updates the value.
    let down = ActionRequest {
        action: Action::ScrollDown,
        target_tree: TreeId::ROOT,
        target_node: target,
        data: None,
    };
    assert!(app.dispatch_action(&down));
    assert_eq!(app.scene().scroll_offset(sv).y, SCROLL_STEP);
    assert_eq!(app.scene().a11y(sv).unwrap().value.as_deref(), Some("48"));
}

/// An inbound `Focus` action focuses the text input, `SetValue` replaces its
/// value through the same edit path typing takes (firing `on_input`), and
/// `Blur` clears focus (SOUL §6.3 — assistive input is never a degraded path).
/// Keyboard editing is exercised headlessly through the identical
/// [`App::dispatch_edit_key`] the windowed loop calls (Directive #5).
#[test]
fn focus_and_set_value_actions_drive_the_text_input() {
    use schnellui::a11y::accesskit_reexport::{Action, ActionData, ActionRequest, TreeId};
    use schnellui::a11y::to_access_id;
    use schnellui::widgets::{Column, EditKey, TextInput};
    use std::cell::RefCell;
    use std::rc::Rc;

    let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let view = Column::new()
        .child(TextInput::new("hi").on_input(move |v| sink.borrow_mut().push(v.to_string())));
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();

    let input = app
        .find_widget(Role::TextInput, None)
        .expect("mounted text input");
    let target = to_access_id(input);

    // Focus lands on the input (the FOCUSED state the AccessKit tree reads).
    let focus = ActionRequest {
        action: Action::Focus,
        target_tree: TreeId::ROOT,
        target_node: target,
        data: None,
    };
    assert!(app.dispatch_action(&focus));
    assert_eq!(app.focused_widget(), Some(input));

    // Typing goes through the same EditKey path the windowed keyboard uses.
    assert!(app.dispatch_edit_key(EditKey::Insert("!")));
    assert_eq!(
        app.scene().a11y(input).unwrap().value.as_deref(),
        Some("hi!")
    );

    // SetValue replaces the whole value and fires on_input again.
    let set = ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: target,
        data: Some(ActionData::Value("bye".into())),
    };
    assert!(app.dispatch_action(&set));
    assert_eq!(
        app.scene().a11y(input).unwrap().value.as_deref(),
        Some("bye")
    );
    assert_eq!(*seen.borrow(), vec!["hi!".to_string(), "bye".to_string()]);

    // Blur clears focus; edit keys then fall on no target.
    let blur = ActionRequest {
        action: Action::Blur,
        target_tree: TreeId::ROOT,
        target_node: target,
        data: None,
    };
    assert!(app.dispatch_action(&blur));
    assert_eq!(app.focused_widget(), None);
    assert!(!app.dispatch_edit_key(EditKey::Insert("x")));
}

// --- standard browser keyboard controls (SOUL §6.3, Directive #5) ---

/// Tab enters and walks the tab order; Enter and Space activate a focused
/// button; Space (not Enter) toggles a focused checkbox — the browser matrix,
/// driven headlessly through the same [`App::dispatch_key`] the windowed loop
/// calls.
#[test]
fn tab_walks_focus_and_enter_space_activate() {
    use schnellui::widgets::{Button, Checkbox, Column};
    use std::cell::Cell;
    use std::rc::Rc;

    let count = Rc::new(Cell::new(0));
    let c2 = count.clone();
    let view = Column::new()
        .child(Button::new("increment").on_click(move || c2.set(c2.get() + 1)))
        .child(Checkbox::new(false));
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();

    let btn = app.find_widget(Role::Button, Some("increment")).unwrap();
    let cb = app.find_widget(Role::CheckBox, None).unwrap();

    // Tab enters the order at the button, then walks to the checkbox and wraps.
    assert!(app.dispatch_key(UiKey::Tab { shift: false }));
    assert_eq!(app.focused_widget(), Some(btn));
    assert!(app.dispatch_key(UiKey::Enter));
    assert!(app.dispatch_key(UiKey::Space { shift: false }));
    assert_eq!(count.get(), 2);

    assert!(app.dispatch_key(UiKey::Tab { shift: false }));
    assert_eq!(app.focused_widget(), Some(cb));
    // Enter does not toggle a checkbox (and there is nothing to scroll) …
    assert!(!app.dispatch_key(UiKey::Enter));
    assert!(
        !schnellui::a11y::StateFlags(app.scene().a11y(cb).unwrap().state)
            .contains(schnellui::a11y::StateFlags::CHECKED)
    );
    // … Space does.
    assert!(app.dispatch_key(UiKey::Space { shift: false }));
    assert!(
        schnellui::a11y::StateFlags(app.scene().a11y(cb).unwrap().state)
            .contains(schnellui::a11y::StateFlags::CHECKED)
    );
    // Shift+Tab walks back to the button.
    assert!(app.dispatch_key(UiKey::Tab { shift: true }));
    assert_eq!(app.focused_widget(), Some(btn));
}

#[test]
fn pointer_focus_is_semantic_but_keyboard_promotes_focus_visible() {
    use schnellui::scene::Primitive;
    use schnellui::widgets::Button;

    let mut app = App::mount_with_size(Button::new("Save"), 240, 120);
    app.frame();
    let button = app.find_widget(Role::Button, Some("Save")).unwrap();
    let line_count = |app: &App| {
        app.scene()
            .paint(button)
            .unwrap()
            .primitives
            .iter()
            .filter(|primitive| matches!(primitive, Primitive::StrokeRect { .. }))
            .count()
    };

    assert!(app.pointer_focus(Some(button)));
    assert_eq!(app.focused_widget(), Some(button));
    assert_eq!(line_count(&app), 0, "pointer focus hides keyboard ring");

    assert!(app.dispatch_key(UiKey::Enter));
    assert_eq!(line_count(&app), 1, "keyboard input reveals focus ring");

    assert!(app.pointer_focus(Some(button)));
    assert_eq!(app.focused_widget(), Some(button));
    assert_eq!(line_count(&app), 0);
}

#[test]
fn a_field_wears_the_focus_ring_however_it_got_focus() {
    use schnellui::widgets::{Button, Column, TextInput};

    let mut app = App::mount_with_size(
        Column::new()
            .child(Button::new("Save"))
            .child(TextInput::new("").hint("Path")),
        240,
        120,
    );
    app.frame();
    let button = app.find_widget(Role::Button, Some("Save")).unwrap();
    let field = app.find_widget(Role::TextInput, None).unwrap();
    assert!(app.pointer_focus(Some(button)));
    assert!(!schnellui::widgets::focus_ring_visible(
        &app.widgets,
        button
    ));
    // An overlay opened by pointer focuses its field the same way; typing
    // goes there next, so the field still wears the ring.
    assert!(app.focus_without_ring(Some(field)));
    assert_eq!(app.focused_widget(), Some(field));
    assert!(
        schnellui::widgets::focus_ring_visible(&app.widgets, field),
        "a field shows its ring however it got focus"
    );
}

#[test]
fn terminal_like_scroll_surface_focuses_from_content_and_receives_raw_keys() {
    use schnellui::widgets::{Scroll, Text};
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut app = App::mount_with_size(
        Scroll::new()
            .label("Terminal emulator")
            .size(240.0, 120.0)
            .child(Text::new("$ prompt")),
        320,
        180,
    );
    app.frame();

    let terminal = app
        .find_widget(Role::ScrollView, Some("Terminal emulator"))
        .unwrap();
    let prompt = app.find_widget(Role::Label, Some("$ prompt")).unwrap();
    let received = Rc::new(RefCell::new(Vec::<String>::new()));
    let sink = received.clone();
    app.register_focused_key_handler(Role::ScrollView, Some("Terminal emulator"), move |key| {
        match key {
            UiKey::Char(text) => {
                sink.borrow_mut().push(text.to_owned());
                true
            }
            UiKey::Enter => {
                sink.borrow_mut().push("<CR>".to_owned());
                true
            }
            UiKey::Tab { .. } => {
                sink.borrow_mut().push("<TAB>".to_owned());
                true
            }
            UiKey::Escape => {
                sink.borrow_mut().push("<ESC>".to_owned());
                true
            }
            UiKey::Control(character) => {
                sink.borrow_mut().push(format!("<CTRL-{character}>"));
                true
            }
            _ => false,
        }
    });

    assert!(app.pointer_focus(Some(prompt)));
    assert_eq!(app.focused_widget(), Some(terminal));
    assert!(app.dispatch_key(UiKey::Char("ls")));
    assert!(app.dispatch_key(UiKey::Enter));
    assert!(app.dispatch_key(UiKey::Tab { shift: false }));
    assert!(app.dispatch_key(UiKey::Escape));
    assert!(app.dispatch_key(UiKey::Control('c')));
    assert_eq!(
        &*received.borrow(),
        &["ls", "<CR>", "<TAB>", "<ESC>", "<CTRL-c>"]
    );
}

#[test]
fn full_fidelity_focused_input_routes_keys_pointer_focus_ime_and_clipboard() {
    use schnellui::widgets::{Scroll, Text};
    use std::cell::RefCell;
    use std::rc::Rc;
    use winit::keyboard::{Key, KeyCode, KeyLocation, NamedKey, PhysicalKey};

    let mut app = App::mount_with_size(
        Scroll::new()
            .label("Raw terminal")
            .size(240.0, 120.0)
            .child(Text::new("$ prompt")),
        320,
        180,
    );
    app.frame();
    let terminal = app
        .find_widget(Role::ScrollView, Some("Raw terminal"))
        .unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    app.register_focused_input_handler(Role::ScrollView, Some("Raw terminal"), move |event| {
        sink.borrow_mut().push(event.clone());
        match event {
            FocusedInputEvent::Clipboard(FocusedClipboardEvent::Copy) => {
                FocusedInputResult::CopyText("selected text".to_owned())
            }
            _ => FocusedInputResult::Handled,
        }
    });

    let rect = app.scene().layout(terminal).unwrap().rect;
    assert_eq!(app.focused_widget(), None);
    assert_eq!(
        app.dispatch_hover_pointer(
            Point {
                x: rect.x + 9.0,
                y: rect.y + 11.0,
            },
            RawModifiers::default(),
        ),
        FocusedInputResult::Handled
    );
    assert!(matches!(
        events.borrow().last(),
        Some(FocusedInputEvent::Pointer(RawPointerEvent {
            action: RawPointerAction::Move,
            ..
        }))
    ));
    events.borrow_mut().clear();

    assert!(app.focus(Some(terminal)));
    assert_eq!(
        events.borrow().first(),
        Some(&FocusedInputEvent::Focus(RawFocusEvent::WidgetGained))
    );

    let result = app.dispatch_focused_input(FocusedInputEvent::Key(RawKeyEvent {
        logical_key: Key::Named(NamedKey::F5),
        physical_key: PhysicalKey::Code(KeyCode::F5),
        key_without_modifiers: Key::Named(NamedKey::F5),
        location: KeyLocation::Standard,
        modifiers: RawModifiers {
            control: true,
            alt: true,
            ..RawModifiers::default()
        },
        state: RawInputState::Released,
        repeat: false,
        text: None,
        text_with_all_modifiers: None,
    }));
    assert_eq!(result, FocusedInputResult::Handled);

    assert_eq!(
        app.dispatch_focused_pointer(
            Point {
                x: rect.x + 17.0,
                y: rect.y + 23.0,
            },
            RawModifiers::default(),
            RawPointerAction::Button {
                button: RawPointerButton::Left,
                state: RawInputState::Pressed,
            },
            false,
        ),
        FocusedInputResult::Handled
    );
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        FocusedInputEvent::Pointer(RawPointerEvent {
            position: Point { x, y },
            ..
        }) if (*x - 17.0).abs() < f32::EPSILON && (*y - 23.0).abs() < f32::EPSILON
    )));

    assert_eq!(
        app.dispatch_focused_input(FocusedInputEvent::Clipboard(FocusedClipboardEvent::Copy)),
        FocusedInputResult::CopyText("selected text".to_owned())
    );
    assert_eq!(
        app.dispatch_focused_input(FocusedInputEvent::Ime(RawImeEvent::Preedit {
            text: "あ".to_owned(),
            cursor: Some((3, 3)),
        })),
        FocusedInputResult::Handled
    );
    assert!(app.focus(None));
    assert_eq!(
        events.borrow().last(),
        Some(&FocusedInputEvent::Focus(RawFocusEvent::WidgetLost))
    );
}

/// Browsers do not distinguish Ctrl on Home/End for sliders, so neither do we.
#[test]
fn slider_ignores_ctrl_on_home_and_end() {
    use schnellui::widgets::{Column, Slider};

    let mut app =
        App::mount_with_size(Column::new().child(Slider::new(50.0, 0.0, 100.0)), 400, 300);
    app.frame();
    let slider = app.find_widget(Role::Slider, None).unwrap();
    app.focus(Some(slider));
    let value = |app: &App| app.scene().a11y(slider).unwrap().value.clone();
    assert!(app.dispatch_key(UiKey::End {
        shift: false,
        ctrl: true
    }));
    assert_eq!(value(&app).as_deref(), Some("100"));
    assert!(app.dispatch_key(UiKey::Home {
        shift: true,
        ctrl: true
    }));
    assert_eq!(value(&app).as_deref(), Some("0"));
}

/// Ctrl+Home and Ctrl+End act like Home and End in a single-line input.
#[test]
fn text_input_treats_ctrl_home_and_end_like_home_and_end() {
    use schnellui::widgets::{Column, EditKey, TextInput};

    let mut app = App::mount_with_size(Column::new().child(TextInput::new("hello")), 400, 300);
    app.frame();
    let input = app.find_widget(Role::TextInput, None).unwrap();
    app.focus(Some(input));
    app.dispatch_key(UiKey::Home {
        shift: false,
        ctrl: true,
    });
    assert!(app.dispatch_edit_key(EditKey::Insert("<")));
    assert_eq!(
        app.scene().a11y(input).unwrap().value.as_deref(),
        Some("<hello")
    );
    app.dispatch_key(UiKey::End {
        shift: false,
        ctrl: true,
    });
    assert!(app.dispatch_edit_key(EditKey::Insert(">")));
    assert_eq!(
        app.scene().a11y(input).unwrap().value.as_deref(),
        Some("<hello>")
    );
    app.dispatch_key(UiKey::Home {
        shift: true,
        ctrl: true,
    });
    app.dispatch_key(UiKey::End {
        shift: false,
        ctrl: true,
    });
    app.dispatch_key(UiKey::Home {
        shift: true,
        ctrl: true,
    });
    assert!(app.dispatch_edit_key(EditKey::Insert("x")));
    assert_eq!(app.scene().a11y(input).unwrap().value.as_deref(), Some("x"));
}

/// A focused slider takes the range keys — arrows ±1 step, PageUp/PageDown
/// ±10, Home/End min/max — and an inbound AccessKit `Increment`/`Decrement`
/// action reaches the identical adjust path (SOUL §6.3).
#[test]
fn slider_keys_and_increment_actions_adjust_the_value() {
    use schnellui::a11y::accesskit_reexport::{Action, ActionData, ActionRequest, TreeId};
    use schnellui::a11y::to_access_id;
    use schnellui::widgets::{Column, Slider};

    let view = Column::new().child(Slider::new(50.0, 0.0, 100.0));
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();
    let slider = app.find_widget(Role::Slider, None).unwrap();
    app.focus(Some(slider));

    assert!(app.dispatch_key(UiKey::Right {
        shift: false,
        ctrl: false
    }));
    assert_eq!(
        app.scene().a11y(slider).unwrap().value.as_deref(),
        Some("51")
    );
    assert!(app.dispatch_key(UiKey::Down { shift: false }));
    assert_eq!(
        app.scene().a11y(slider).unwrap().value.as_deref(),
        Some("50")
    );
    assert!(app.dispatch_key(UiKey::PageUp));
    assert_eq!(
        app.scene().a11y(slider).unwrap().value.as_deref(),
        Some("60")
    );
    assert!(app.dispatch_key(UiKey::End {
        shift: false,
        ctrl: false
    }));
    assert_eq!(
        app.scene().a11y(slider).unwrap().value.as_deref(),
        Some("100")
    );
    // at the max, a further increment is a no-op
    assert!(!app.dispatch_key(UiKey::Up { shift: false }));
    assert!(app.dispatch_key(UiKey::Home {
        shift: false,
        ctrl: false
    }));
    assert_eq!(
        app.scene().a11y(slider).unwrap().value.as_deref(),
        Some("0")
    );

    // AccessKit Increment/Decrement land on the same path (SOUL §6.3).
    let inc = ActionRequest {
        action: Action::Increment,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(slider),
        data: None,
    };
    assert!(app.dispatch_action(&inc));
    assert_eq!(
        app.scene().a11y(slider).unwrap().value.as_deref(),
        Some("1")
    );
    let dec = ActionRequest {
        action: Action::Decrement,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(slider),
        data: None,
    };
    assert!(app.dispatch_action(&dec));
    assert_eq!(
        app.scene().a11y(slider).unwrap().value.as_deref(),
        Some("0")
    );
    let set = ActionRequest {
        action: Action::SetValue,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(slider),
        data: Some(ActionData::Value("37".into())),
    };
    assert!(app.dispatch_action(&set));
    assert_eq!(
        app.scene().a11y(slider).unwrap().value.as_deref(),
        Some("37")
    );
}

#[test]
fn settling_twice_after_a_structural_replacement_lays_out_once() {
    use widgets::{Column, ComponentRef, Text, View as _};

    let header_ref = ComponentRef::new();
    let mut app = App::mount_with_size(
        Column::new()
            .child(Text::new("Header A").with_ref(header_ref))
            .child(Text::new("Body")),
        320,
        240,
    );
    app.frame();
    let runs = app.layout_runs();

    // The host settles right after a replacement to restore hover, then again when
    // it redraws. Only the first settle has anything to lay out.
    app.replace_subtree(header_ref, Text::new("Header B"))
        .unwrap();
    app.settle_frame();
    assert_eq!(app.layout_runs(), runs + 1, "the replacement lays out once");
    app.settle_frame();
    assert_eq!(
        app.layout_runs(),
        runs + 1,
        "the redraw's settle must not lay out again"
    );

    // A fresh request after the first layout still gets its own pass, even on a
    // node that was already flagged when the layout ran.
    let header = app.resolve_ref(header_ref).unwrap();
    app.scene_mut()
        .mark_dirty(header, scene::DirtyFlags::LAYOUT);
    app.settle_frame();
    assert_eq!(app.layout_runs(), runs + 2);
    app.scene_mut()
        .mark_dirty(header, scene::DirtyFlags::LAYOUT);
    app.settle_frame();
    assert_eq!(app.layout_runs(), runs + 3);
}

#[test]
fn virtual_list_scrolling_one_row_reorders_children_in_one_pass() {
    use widgets::{ComponentRef, Text, View as _, VirtualList, VirtualListController};

    let controller = VirtualListController::new(0_usize..1_000, 20.0);
    controller.overscan(0.0);
    let row_refs: Vec<_> = (0..1_000).map(|_| ComponentRef::new()).collect();
    let refs = row_refs.clone();
    let scroll_ref = ComponentRef::new();
    let mut app = App::mount_with_size(
        VirtualList::new(controller.clone(), move |key| {
            Text::new(format!("row {key}")).with_ref(refs[*key])
        })
        .label("virtual transcript")
        .size(240.0, 100.0)
        .with_ref(scroll_ref),
        280,
        140,
    );
    app.frame();
    let scroll = app.resolve_ref(scroll_ref).unwrap();
    let first = app.resolve_ref(row_refs[0]).unwrap();
    let content = app.scene().node(first).unwrap().parent.unwrap();
    let mounted = app.scene().node(content).unwrap().children.len();

    let revision = app.scene().render_revision();
    assert!(schnellui::widgets::dispatch_scroll(
        &app.widgets,
        &mut app.scene,
        scroll,
        20.0,
    ));
    app.frame();

    let children = app.scene().node(content).unwrap().children.clone();
    assert_eq!(children.len(), mounted, "one row left, one entered");
    let ids: Vec<_> = (1..=mounted - 2)
        .map(|key| app.resolve_ref(row_refs[key]).unwrap())
        .collect();
    assert_eq!(&children[1..children.len() - 1], ids.as_slice());
    // Per-row moves would bump the revision once per mounted row. The scroll costs
    // one removal, one insertion and a single reorder.
    assert!(
        app.scene().render_revision() - revision <= 3,
        "scrolling one row bumped the render revision {} times for {mounted} children",
        app.scene().render_revision() - revision,
    );
}

#[test]
fn focus_after_a_replacement_reaches_into_a_wrapper_node() {
    use schnellui::scene::ComponentRef;
    use widgets::{Column, ComboBox, ComboBoxOption, View};

    let region = ComponentRef::new();
    let picker = ComponentRef::new();
    let combo = |open: bool| {
        ComboBox::new("")
            .label("Font")
            .open(open)
            .option(ComboBoxOption::new("Mono"))
            .with_ref(picker)
    };
    let mut app = App::mount_with_size(
        Column::new().child(Column::new().child(combo(false)).with_ref(region)),
        240,
        160,
    );
    app.frame();

    app.apply_subtree_replacement(
        SubtreeReplacement::new(region, Column::new().child(combo(true)), "open")
            .with_focus_after(picker),
    )
    .unwrap();
    app.frame();
    let field = app.find_widget(Role::ComboBox, Some("Font"));
    assert!(field.is_some());
    assert_ne!(
        app.resolve_ref(picker),
        field,
        "the ref sits on the wrapper"
    );
    assert_eq!(app.focused_widget(), field);
}

/// A dropdown popup inside a modal dialog takes the pointer over the dialog
/// content it covers, which comes later in tree order.
#[test]
fn a_dropdown_popup_in_a_dialog_hits_above_the_dialog_content_below_it() {
    use schnellui::scene::Point;
    use schnellui::widgets::{Button, Column, Dialog, Dropdown, DropdownOption};
    let view = Column::new().fill().child(
        Dialog::new("Settings")
            .child(
                Dropdown::new("Keymap")
                    .open(true)
                    .option(DropdownOption::new("one").selected(true))
                    .option(DropdownOption::new("two"))
                    .option(DropdownOption::new("three")),
            )
            .child(Button::new("first below"))
            .child(Button::new("second below"))
            .child(Button::new("third below")),
    );
    let mut app = App::mount_with_size(view, 800, 600);
    app.frame();
    // Some option must sit over a button; which one depends on row heights.
    let (option, center) = ["one", "two", "three"]
        .into_iter()
        .filter_map(|name| app.find_widget(Role::ListBoxOption, Some(name)))
        .map(|option| {
            let rect = app.scene().composited_rect(option);
            let center = Point {
                x: rect.x + rect.width / 2.0,
                y: rect.y + rect.height / 2.0,
            };
            (option, center)
        })
        .find(|&(_, center)| {
            ["first below", "second below", "third below"]
                .into_iter()
                .filter_map(|name| app.find_widget(Role::Button, Some(name)))
                .any(|button| app.scene().composited_rect(button).contains(center))
        })
        .expect("the test needs a button under an option");
    assert_eq!(app.hit_test(center), Some(option));
}

#[test]
fn a_bold_label_is_shaped_in_the_bold_face() {
    use widgets::{Column, Text};
    let mut app = App::mount_with_size(
        Column::new()
            .child(Text::new("Allowed hosts ").size(14.0))
            .child(Text::new("Allowed hosts").size(14.0).bold()),
        400,
        200,
    );
    app.frame();
    let width = |name| {
        let id = app.find_widget(Role::Label, Some(name)).unwrap();
        app.scene().layout(id).unwrap().rect.width
    };
    // Liberation Sans Bold runs wider than the regular face, even with a
    // space added to the regular label so the two names differ.
    assert!(width("Allowed hosts") > width("Allowed hosts "));
}

/// The playground's Dialogs page: open the Theme dropdown above the
/// embedded "Review changes" dialog. The dialog covered the lower options,
/// which could be neither seen nor clicked.
#[test]
fn an_open_dropdown_on_the_page_is_above_a_modeless_dialog() {
    use schnellui::scene::Point;
    use schnellui::widgets::{Column, Dialog, Dropdown, DropdownOption, Stack, Text};
    let view = Column::new()
        .child(
            Dropdown::new("Theme")
                .open(true)
                .option(DropdownOption::new("Light").selected(true))
                .option(DropdownOption::new("Dark"))
                .option(DropdownOption::new("Forest"))
                .option(DropdownOption::new("Candy")),
        )
        .child(
            Stack::new().size(400.0, 300.0).child(
                Dialog::new("Review changes")
                    .modeless()
                    .non_fixed()
                    .size(380.0, 200.0)
                    .child(Text::new("Three files changed")),
            ),
        );
    let mut app = App::mount_with_size(view, 640, 600);
    app.frame();
    let dialog = app
        .find_widget(Role::Dialog, Some("Review changes"))
        .unwrap();
    let covered = app.scene().composited_rect(dialog);
    let option = app.find_widget(Role::ListBoxOption, Some("Candy")).unwrap();
    let rect = app.scene().composited_rect(option);
    let center = Point {
        x: rect.x + rect.width / 2.0,
        y: rect.y + rect.height / 2.0,
    };
    assert!(
        covered.contains(center),
        "the test needs the dialog under Candy"
    );
    assert_eq!(app.hit_test(center), Some(option));
}

/// Open a dropdown whose list covers a button with a tooltip, and point at
/// the option over the button. The button's tooltip came up beside the list
/// although the pointer was on the option, not the button.
#[test]
fn a_control_covered_by_an_open_list_shows_no_tooltip() {
    use schnellui::scene::{Point, Primitive};
    use schnellui::widgets::{Button, Column, Dropdown, DropdownOption};
    let view = Column::new()
        .child(
            Dropdown::new("Theme")
                .open(true)
                .option(DropdownOption::new("Light").selected(true))
                .option(DropdownOption::new("Dark"))
                .option(DropdownOption::new("Forest")),
        )
        .child(Button::new("Archive everything").tooltip("Archive it"));
    let mut app = App::mount_with_size(view, 640, 400);
    app.frame();
    let button = app
        .find_widget(Role::Button, Some("Archive everything"))
        .unwrap();
    let shown = |app: &App| {
        app.scene().paint(button).map_or(0, |p| {
            p.primitives
                .iter()
                .filter(|p| matches!(p, Primitive::GlyphQuad { color, .. } if color.a != 0))
                .count()
        })
    };
    let label_only = shown(&app);
    let covered = app.scene().composited_rect(button);
    let option = ["Light", "Dark", "Forest"]
        .into_iter()
        .filter_map(|name| app.find_widget(Role::ListBoxOption, Some(name)))
        .map(|id| app.scene().composited_rect(id))
        .map(|r| Point {
            x: r.x + 8.0,
            y: r.y + r.height / 2.0,
        })
        .find(|p| covered.contains(*p))
        .expect("the test needs an option over the button");
    assert_ne!(app.hit_test(option), Some(button));
    app.update_pointer_proximity(option);
    app.frame();
    assert_eq!(shown(&app), label_only, "the tooltip stays hidden");
}

/// Content that slides a control under a resting pointer hovers it, but its
/// tooltip waits for the pointer to move; one already shown stays while the
/// pointer is still on its control and goes when the control moves away.
#[test]
fn a_relayout_under_a_resting_pointer_hovers_but_never_raises_a_tooltip() {
    use schnellui::scene::{Point, Primitive};
    use schnellui::widgets::{Button, Row, Text};
    let letters = schnellui::signal::create_signal(30_usize);
    let view = Row::new()
        .child(Text::dynamic(move || "m".repeat(letters.get())))
        .child(Button::new("Go").tooltip("Hint"));
    let mut app = App::mount_with_size(view, 600, 100);
    app.frame();
    let go = app.find_widget(Role::Button, Some("Go")).unwrap();
    let tip = |app: &App| {
        app.scene().paint(go).map_or(0, |p| {
            p.primitives
                .iter()
                .filter(|p| matches!(p, Primitive::GlyphQuad { color, .. } if color.a != 0))
                .count()
        })
    };
    let label_only = tip(&app);
    // The pointer rests over the text, left of the button.
    let rest = Point { x: 20.0, y: 15.0 };
    assert_ne!(app.hit_test(rest), Some(go));
    app.update_pointer_proximity(rest);
    // The text shrinks: the button slides under the pointer.
    letters.set(1);
    app.frame();
    assert_eq!(app.hit_test(rest), Some(go));
    assert_eq!(
        tip(&app),
        label_only,
        "no tooltip for a pointer that did not move"
    );
    // Moving the pointer raises it; growing the text pushes the button away.
    app.update_pointer_proximity(Point {
        x: rest.x + 1.0,
        y: rest.y,
    });
    app.frame();
    let shown = tip(&app);
    assert!(shown > label_only, "the tooltip came up on a move");
    letters.set(1);
    app.frame();
    assert_eq!(
        tip(&app),
        shown,
        "a shown tooltip stays while its control does"
    );
    letters.set(30);
    app.frame();
    assert_eq!(
        tip(&app),
        label_only,
        "it goes when the control moves off the pointer"
    );
}

#[test]
fn a_dropdown_is_at_least_its_min_width() {
    use schnellui::widgets::{Column, Dropdown, DropdownOption};
    let view = Column::new()
        .child(Dropdown::new("Short").option(DropdownOption::new("a").selected(true)))
        .child(
            Dropdown::new("Wide")
                .min_width(240.0)
                .option(DropdownOption::new("a").selected(true)),
        );
    let mut app = App::mount_with_size(view, 800, 600);
    app.frame();
    let width = |name| {
        let id = app.find_widget(Role::ComboBox, Some(name)).unwrap();
        app.scene().composited_rect(id).width
    };
    assert!(width("Short") < 240.0);
    assert!((width("Wide") - 240.0).abs() < 0.5, "{}", width("Wide"));
}

#[test]
fn a_hint_keeps_the_field_one_line_high_and_names_it() {
    use schnellui::widgets::{Column, TextInput};
    let view = Column::new()
        .child(TextInput::new("").label("Labelled"))
        .child(TextInput::new("").hint("Hinted"));
    let mut app = App::mount_with_size(view, 800, 600);
    app.frame();
    let height = |name| {
        let id = app.find_widget(Role::TextInput, Some(name)).unwrap();
        app.scene().composited_rect(id).height
    };
    assert!(height("Hinted") < height("Labelled"));
}

#[test]
fn keyboard_navigation_holds_hover_until_the_pointer_moves() {
    use widgets::{Button, Column};

    let mut app = App::mount_with_size(
        Column::new()
            .child(Button::new("first").width(100.0).height(30.0))
            .child(Button::new("second").width(100.0).height(30.0)),
        200,
        100,
    );
    app.frame();
    let rest = scene::Point { x: 20.0, y: 10.0 };
    // The pointer lights the row under it.
    assert!(app.update_pointer_proximity(rest));
    // A navigation key takes the hover away from the resting pointer.
    assert!(app.hold_pointer_hover());
    // A remount re-evaluates at the same point and lights nothing.
    assert!(!app.update_pointer_proximity(rest));
    // The first move hands hover back.
    assert!(app.update_pointer_proximity(scene::Point { x: 21.0, y: 10.0 }));
}

/// A dialog stretches its fields to its content width. A field paints across
/// the box layout gave it from the first frame, not at its intrinsic width
/// until a click re-emits it.
#[test]
fn a_stretched_text_field_paints_across_its_box_before_any_input() {
    use widgets::{Dialog, Stack, TextInput};

    let frame_width = |app: &App, role: Role| {
        let id = app.find_widget(role, None).unwrap();
        let laid = app.scene().layout(id).unwrap().rect.width;
        let painted = app
            .scene()
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .find_map(|p| match p {
                scene::Primitive::SolidRect { rect, .. } => Some(rect.width),
                _ => None,
            })
            .unwrap();
        (laid, painted)
    };
    let dialog = |width: f32| {
        Stack::new().fill().child(
            Dialog::new("Note")
                .modeless()
                .at(10.0, 10.0)
                .size(width, 220.0)
                .child(TextInput::new("draft")),
        )
    };
    let mut app = App::mount_with_size(dialog(300.0), 400, 300);
    app.frame();
    let (laid, painted) = frame_width(&app, Role::TextInput);
    assert!(laid > 200.0, "the dialog stretched the field");
    assert_eq!(painted, laid, "the field paints as wide as it is laid out");
}

/// Two dialogs scoped to side-by-side panels, as the playground's Dialogs
/// page shows them, in a window narrower than the panels. Each dialog layer
/// must stay on its own panel. It used to be pushed left to fit the window,
/// as a dropdown popup is, and covered the panel beside it.
#[test]
fn a_scoped_dialog_stays_on_its_panel_in_a_narrow_window() {
    use schnellui::widgets::{Dialog, Row, Stack, Text};

    let panel = |title: &'static str| {
        Stack::new().size(290.0, 250.0).child(
            Dialog::new(title)
                .modeless()
                .non_fixed()
                .size(240.0, 180.0)
                .child(Text::new(title)),
        )
    };
    let view = Row::new()
        .gap(12.0)
        .child(panel("Left"))
        .child(panel("Right"));
    let mut app = App::mount_with_size(view, 365, 400);
    app.frame();
    let scene = app.scene();
    for layer in scene.preorder().filter(|id| {
        scene
            .node(*id)
            .is_some_and(|n| n.kind == WidgetKind::DialogLayer)
    }) {
        let parent = scene.node(layer).and_then(|n| n.parent).unwrap();
        assert_eq!(
            scene.layout(layer).unwrap().rect,
            scene.layout(parent).unwrap().rect,
            "a scoped dialog layer covers exactly its panel"
        );
    }
}

/// A chart wider than the window keeps each data point's hover box on the
/// point. Layout used to push the boxes past the window edge back inside it,
/// as it does a popup, so hovering near the edge picked the wrong point.
#[test]
fn chart_marks_stay_on_their_points_in_a_narrow_window() {
    use crate::charts::LineChart;
    use widgets::Row;
    let view = Row::new().child(
        LineChart::new([28.0, 41.0, 37.0, 58.0, 52.0, 71.0, 66.0, 82.0])
            .labels(["Tue", "Wed", "Thu", "Fri", "Sat", "Sun", "Mon", "Today"])
            .tooltips(true)
            .size(400.0, 200.0),
    );
    let mut app = App::mount_with_size(view, 420, 300);
    app.frame();
    let marks = |app: &App| -> Vec<scene::Rect> {
        let scene = app.scene();
        scene
            .preorder()
            .filter(|id| {
                scene.node(*id).is_some_and(|n| n.kind == WidgetKind::Stack)
                    && scene
                        .a11y(*id)
                        .is_some_and(|a| Role::from_u16(a.role) == Role::Group && a.name.is_some())
            })
            .filter_map(|id| scene.layout(id).map(|l| l.rect))
            .collect()
    };
    let wide = marks(&app);
    assert_eq!(wide.len(), 8, "one hover box per point");
    app.resize(250.0, 300.0);
    app.frame();
    assert_eq!(marks(&app), wide);
}

// ---------------------------------------------------------------------------
// undo / redo in editable text (SOUL §6.3: one inbound edit path)
// ---------------------------------------------------------------------------

mod undo_redo {
    use super::*;
    use schnellui::widgets::{Column, EditKey, TextArea, TextInput};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn input_app(value: &str) -> (App, scene::WidgetId) {
        let mut app = App::mount(TextInput::new(value));
        app.frame();
        let id = app.find_widget(Role::TextInput, None).unwrap();
        app.focus(Some(id));
        (app, id)
    }

    fn area_app(value: &str) -> (App, scene::WidgetId) {
        let mut app = App::mount(TextArea::new(value));
        app.frame();
        let id = app.find_widget(Role::MultilineTextInput, None).unwrap();
        app.focus(Some(id));
        (app, id)
    }

    fn value(app: &App, id: scene::WidgetId) -> String {
        app.scene().a11y(id).unwrap().value.clone().unwrap()
    }

    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            let mut buf = [0u8; 4];
            assert!(app.dispatch_key(UiKey::Char(ch.encode_utf8(&mut buf))));
        }
    }

    fn left(app: &mut App) {
        app.dispatch_key(UiKey::Left {
            shift: false,
            ctrl: false,
        });
    }

    #[test]
    fn a_typed_run_is_one_undo_step() {
        for area in [false, true] {
            let (mut app, id) = if area { area_app("") } else { input_app("") };
            type_text(&mut app, "hello");
            assert_eq!(value(&app, id), "hello");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "");
            assert!(!app.dispatch_key(UiKey::Undo), "nothing older to undo");
            assert!(app.dispatch_key(UiKey::Redo));
            assert_eq!(value(&app, id), "hello");
            assert!(!app.dispatch_key(UiKey::Redo));
        }
    }

    #[test]
    fn whitespace_and_caret_moves_split_runs() {
        for area in [false, true] {
            let (mut app, id) = if area { area_app("") } else { input_app("") };
            type_text(&mut app, "ab cd");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "ab", "the space opens a new step");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "");

            assert!(app.dispatch_key(UiKey::Redo));
            assert!(app.dispatch_key(UiKey::Redo));
            assert_eq!(value(&app, id), "ab cd");
            left(&mut app);
            type_text(&mut app, "X");
            type_text(&mut app, "Y");
            assert_eq!(value(&app, id), "ab cXYd");
            left(&mut app);
            left(&mut app);
            type_text(&mut app, "Z");
            assert_eq!(value(&app, id), "ab cZXYd");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "ab cXYd", "a caret move ended the XY run");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "ab cd", "XY is one step");
        }
    }

    #[test]
    fn backspace_runs_are_one_step_and_restore_the_caret() {
        let (mut app, id) = input_app("hello");
        for _ in 0..3 {
            assert!(app.dispatch_key(UiKey::Backspace));
        }
        assert_eq!(value(&app, id), "he");
        type_text(&mut app, "y");
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(value(&app, id), "he");
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(value(&app, id), "hello");
        assert!(app.dispatch_key(UiKey::Char("!")));
        assert_eq!(value(&app, id), "hello!", "the caret came back to the end");
    }

    #[test]
    fn undo_restores_the_selection_and_redo_reapplies_the_edit() {
        let (mut app, id) = input_app("hello");
        assert!(app.dispatch_key(UiKey::SelectAll));
        type_text(&mut app, "x");
        assert_eq!(value(&app, id), "x");
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(value(&app, id), "hello");
        assert_eq!(app.selected_text().as_deref(), Some("hello"));
        assert!(app.dispatch_key(UiKey::Redo));
        assert_eq!(value(&app, id), "x");
        assert_eq!(app.selected_text(), None);
        assert!(app.dispatch_key(UiKey::Char("y")));
        assert_eq!(value(&app, id), "xy", "the caret follows the redone edit");
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        for area in [false, true] {
            let (mut app, id) = if area { area_app("") } else { input_app("") };
            type_text(&mut app, "a");
            assert!(app.dispatch_key(UiKey::Undo));
            type_text(&mut app, "b");
            assert!(!app.dispatch_key(UiKey::Redo));
            assert_eq!(value(&app, id), "b");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "");
        }
    }

    #[test]
    fn cut_paste_enter_and_ime_commits_undo() {
        for area in [false, true] {
            let (mut app, id) = if area {
                area_app("one two")
            } else {
                input_app("one two")
            };
            assert!(app.dispatch_key(UiKey::SelectAll));
            assert!(app.delete_text_selection());
            assert_eq!(value(&app, id), "");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "one two");
            assert_eq!(app.selected_text().as_deref(), Some("one two"));

            assert!(app.dispatch_key(UiKey::Redo));
            assert!(app.paste_text("ab"));
            assert!(app.paste_text("cd"));
            assert_eq!(value(&app, id), "abcd", "pastes never merge");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "ab");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "");
            assert!(app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "one two");
        }
        let (mut app, id) = area_app("ab");
        assert!(app.dispatch_key(UiKey::Enter));
        type_text(&mut app, "c");
        assert_eq!(value(&app, id), "ab\nc");
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(value(&app, id), "ab\n");
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(value(&app, id), "ab");
        let (mut app, id) = input_app("");
        assert_eq!(
            app.dispatch_ime(crate::RawImeEvent::Commit("日本".into())),
            FocusedInputResult::Handled
        );
        assert_eq!(value(&app, id), "日本");
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(value(&app, id), "");
    }

    #[test]
    fn undo_reports_through_on_input_and_a_controlled_field_stays_in_sync() {
        let state = schnellui::signal::create_signal(String::from("start"));
        let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let log = seen.clone();
        let mut app = App::mount_themed(theme::LIGHT, move || {
            let log = log.clone();
            Column::new().child(TextInput::new(state.get()).on_input(move |v| {
                log.borrow_mut().push(v.to_string());
                state.set(v.to_string());
            }))
        });
        app.frame();
        let id = app.find_widget(Role::TextInput, None).unwrap();
        app.focus(Some(id));
        type_text(&mut app, "!?");
        assert_eq!(state.get(), "start!?");
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(state.get(), "start", "undo wrote the app's state");
        assert_eq!(seen.borrow().last().map(String::as_str), Some("start"));
        assert!(app.dispatch_key(UiKey::Redo));
        assert_eq!(state.get(), "start!?");

        // A remount whose controlled value matches keeps the history.
        assert!(app.set_theme(theme::DARK));
        app.frame();
        let id = app.find_widget(Role::TextInput, None).unwrap();
        assert_eq!(app.focused_widget(), Some(id));
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(state.get(), "start");
        assert_eq!(value(&app, id), "start");
        assert!(app.dispatch_key(UiKey::Redo));
        assert_eq!(state.get(), "start!?");

        // The app rewriting the value drops the history instead of
        // resurrecting stale text.
        state.set(String::from("rewritten"));
        assert!(app.set_theme(theme::LIGHT));
        app.frame();
        let id = app.find_widget(Role::TextInput, None).unwrap();
        assert_eq!(value(&app, id), "rewritten");
        assert!(!app.dispatch_key(UiKey::Undo));
        assert!(!app.dispatch_key(UiKey::Redo));
        assert_eq!(state.get(), "rewritten");
    }

    #[test]
    fn set_value_drops_the_history() {
        for area in [false, true] {
            let (mut app, id) = if area { area_app("") } else { input_app("") };
            type_text(&mut app, "abc");
            assert!(schnellui::widgets::set_text_value(
                &app.widgets,
                &mut app.scene,
                &mut app.text,
                &mut app.atlas,
                id,
                "other",
            ));
            assert!(!app.dispatch_key(UiKey::Undo));
            assert_eq!(value(&app, id), "other");
        }
    }

    #[test]
    fn undo_and_redo_without_history_return_false() {
        for area in [false, true] {
            let (mut app, id) = if area {
                area_app("text")
            } else {
                input_app("text")
            };
            assert!(!app.dispatch_key(UiKey::Undo));
            assert!(!app.dispatch_key(UiKey::Redo));
            assert!(!app.dispatch_edit_key(EditKey::Undo));
            assert!(!app.dispatch_edit_key(EditKey::Redo));
            assert_eq!(value(&app, id), "text");
        }
        // Moving the caret is not an edit.
        let (mut app, _) = input_app("text");
        left(&mut app);
        assert!(!app.dispatch_key(UiKey::Undo));
    }

    #[test]
    fn history_is_per_widget_and_capped_at_one_hundred_steps() {
        let mut app = App::mount(
            Column::new()
                .child(TextInput::new("").placeholder("first"))
                .child(TextInput::new("").placeholder("second")),
        );
        app.frame();
        let first = app.find_widget(Role::TextInput, Some("first")).unwrap();
        let second = app.find_widget(Role::TextInput, Some("second")).unwrap();
        app.focus(Some(first));
        type_text(&mut app, "one");
        app.focus(Some(second));
        assert!(!app.dispatch_key(UiKey::Undo), "the second field has none");
        for _ in 0..150 {
            assert!(app.paste_text("p"));
        }
        let mut undone = 0;
        while app.dispatch_key(UiKey::Undo) {
            undone += 1;
        }
        assert_eq!(undone, 100);
        assert_eq!(value(&app, second), "p".repeat(50));
        assert_eq!(value(&app, first), "one");
        app.focus(Some(first));
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(value(&app, first), "");
    }

    #[test]
    fn read_only_areas_and_multi_caret_edits_undo_cleanly() {
        let mut app = App::mount(TextArea::new("fixed").read_only());
        app.frame();
        let id = app.find_widget(Role::MultilineTextInput, None).unwrap();
        app.focus(Some(id));
        assert!(!app.dispatch_key(UiKey::Undo));
        assert_eq!(value(&app, id), "fixed");

        let (mut app, id) = area_app("a\nb\nc");
        assert!(app.dispatch_key(UiKey::Home {
            shift: false,
            ctrl: false
        }));
        assert!(app.dispatch_edit_key(EditKey::Insert("> ")));
        assert_eq!(value(&app, id), "a\nb\n> c");
        assert!(app.dispatch_key(UiKey::Undo));
        assert_eq!(value(&app, id), "a\nb\nc");
    }
}

mod uncontrolled_state_across_rebuilds {
    use super::*;
    use schnellui::signal::create_signal;
    use schnellui::widgets::{Checkbox, Column, Radio, Slider, Switch, TextArea, TextInput};

    fn a11y(app: &App, role: Role) -> (Option<String>, u32) {
        let id = app.find_widget(role, None).unwrap();
        let a = app.scene().a11y(id).unwrap();
        (a.value.clone(), a.state.into())
    }

    fn form() -> Column {
        Column::new()
            .child(Checkbox::new(false).name("Remember me"))
            .child(Switch::new(false))
            .child(Radio::new(false))
            .child(Slider::new(0.0, 0.0, 10.0).step(1.0).name("Volume"))
            .child(TextInput::new("").label("Draft"))
            .child(TextArea::new(""))
    }

    fn edit(app: &mut App) {
        for role in [Role::CheckBox, Role::Switch, Role::Radio] {
            let id = app.find_widget(role, None).unwrap();
            assert!(app.dispatch_click(id));
        }
        let slider = app.find_widget(Role::Slider, None).unwrap();
        app.focus(Some(slider));
        let right = UiKey::Right {
            shift: false,
            ctrl: false,
        };
        assert!(app.dispatch_key(right));
        assert!(app.dispatch_key(right));
        for (role, ch) in [(Role::TextInput, "x"), (Role::MultilineTextInput, "y")] {
            let id = app.find_widget(role, None).unwrap();
            app.focus(Some(id));
            assert!(app.dispatch_key(UiKey::Char(ch)));
        }
        app.frame();
    }

    const ROLES: [Role; 6] = [
        Role::CheckBox,
        Role::Switch,
        Role::Radio,
        Role::Slider,
        Role::TextInput,
        Role::MultilineTextInput,
    ];

    /// Sequence: tick a checkbox, flip a switch, pick a radio, nudge a
    /// slider and type into two fields, none of them bound to app state;
    /// then the system flips to dark mode. Every edit vanished, because the
    /// theme rebuild remounted each control from its authored value.
    #[test]
    fn a_theme_switch_keeps_what_the_user_set_on_unbound_controls() {
        let mut app = App::mount_themed(theme::LIGHT, form);
        app.frame();
        edit(&mut app);
        let edited: Vec<_> = ROLES.iter().map(|&r| a11y(&app, r)).collect();
        assert!(app.set_theme(theme::DARK));
        app.frame();
        let rebuilt: Vec<_> = ROLES.iter().map(|&r| a11y(&app, r)).collect();
        assert_eq!(rebuilt, edited);
    }

    /// The app still wins: a rebuild whose authored value changed shows the
    /// new value, not the user's stale edit.
    #[test]
    fn a_rebuild_with_a_new_authored_value_takes_it() {
        let reset = create_signal(false);
        let mut app = App::mount_themed(theme::LIGHT, move || {
            if reset.get() {
                Column::new()
                    .child(Checkbox::new(true).name("Remember me"))
                    .child(Switch::new(true))
                    .child(Radio::new(true))
                    .child(Slider::new(7.0, 0.0, 10.0).step(1.0).name("Volume"))
                    .child(TextInput::new("app").label("Draft"))
                    .child(TextArea::new("app"))
            } else {
                form()
            }
        });
        app.frame();
        edit(&mut app);
        reset.set(true);
        assert!(app.set_theme(theme::DARK));
        app.frame();
        let checked = |app: &App, role| a11y(app, role).1 & 1 == 1;
        assert!(checked(&app, Role::CheckBox));
        assert!(checked(&app, Role::Switch));
        assert!(checked(&app, Role::Radio));
        assert_eq!(a11y(&app, Role::Slider).0.as_deref(), Some("7"));
        assert_eq!(a11y(&app, Role::TextInput).0.as_deref(), Some("app"));
        assert_eq!(
            a11y(&app, Role::MultilineTextInput).0.as_deref(),
            Some("app")
        );
    }
    fn selected(app: &App, role: Role, name: &str) -> bool {
        let id = app.find_widget(role, Some(name)).unwrap();
        let state = app.scene().a11y(id).unwrap().state;
        schnellui::a11y::StateFlags(state).contains(schnellui::a11y::StateFlags::SELECTED)
    }

    /// The user picks another tab, item, row and dropdown option. None of
    /// them is bound to app state, and a rebuild keeps the pick, without
    /// calling the handlers that already ran when it was made.
    #[test]
    fn a_rebuild_keeps_an_unbound_selection() {
        use schnellui::widgets::{Dropdown, DropdownOption, List, ListItem, Tab, TabBar, Table};
        let mut app = App::mount_themed(theme::LIGHT, move || {
            Column::new()
                .child(
                    TabBar::new()
                        .child(Tab::new("One").selected(true))
                        .child(Tab::new("Two")),
                )
                .child(
                    List::new()
                        .child(ListItem::new("Inbox").selected(true))
                        .child(ListItem::new("Archive")),
                )
                .child(
                    Table::new()
                        .columns(["Name"])
                        .row(["Ann"])
                        .row(["Bob"])
                        .selected_row(0),
                )
                .child(
                    Dropdown::new("Mode")
                        .open(true)
                        .option(DropdownOption::new("Light").selected(true))
                        .option(DropdownOption::new("Dark")),
                )
        });
        app.frame();
        for (role, name) in [
            (Role::Tab, "Two"),
            (Role::ListItem, "Archive"),
            (Role::TableRow, "Bob"),
            (Role::ListBoxOption, "Dark"),
        ] {
            let id = app.find_widget(role, Some(name)).unwrap();
            assert!(app.dispatch_click(id), "{name}");
        }
        app.frame();
        assert!(app.set_theme(theme::DARK));
        app.frame();
        assert!(selected(&app, Role::Tab, "Two"));
        assert!(!selected(&app, Role::Tab, "One"));
        assert!(selected(&app, Role::ListItem, "Archive"));
        assert!(!selected(&app, Role::ListItem, "Inbox"));
        assert!(selected(&app, Role::TableRow, "Bob"));
        assert!(!selected(&app, Role::TableRow, "Ann"));
        assert!(selected(&app, Role::ListBoxOption, "Dark"));
        assert!(!selected(&app, Role::ListBoxOption, "Light"));
        let trigger = app.find_widget(Role::ComboBox, Some("Mode")).unwrap();
        assert_eq!(
            app.scene().a11y(trigger).unwrap().value.as_deref(),
            Some("Dark")
        );
    }

    /// Sequence (todo): type a title into a field bound through on_input,
    /// tick a bound checkbox and pick a bound tab, then the app clears the
    /// draft and resets both, back to the values the tree was first built
    /// with, and remounts. The field still showed the typed title, because
    /// the rebuild authored the same values as the first build and the
    /// user's edits were carried over the app's reset.
    #[test]
    fn a_bound_control_the_app_resets_shows_the_reset() {
        use schnellui::widgets::{Tab, TabBar};
        let draft = create_signal(String::new());
        let done = create_signal(false);
        let tab = create_signal(0_u8);
        let mut app = App::mount_themed(theme::LIGHT, move || {
            Column::new()
                .child(
                    TextInput::new(draft.get())
                        .label("Title")
                        .on_input(move |v| draft.set(v.to_string())),
                )
                .child(
                    Checkbox::new(done.get())
                        .name("Done")
                        .on_toggle(move |v| done.set(v)),
                )
                .child(
                    TabBar::new()
                        .child(Tab::new("One").selected(tab.get() == 0))
                        .child(
                            Tab::new("Two")
                                .selected(tab.get() == 1)
                                .on_select(move || tab.set(1)),
                        ),
                )
        });
        app.frame();
        let input = app.find_widget(Role::TextInput, None).unwrap();
        app.focus(Some(input));
        for ch in ["m", "i", "l", "k"] {
            assert!(app.dispatch_key(UiKey::Char(ch)));
        }
        let done_box = app.find_widget(Role::CheckBox, Some("Done")).unwrap();
        assert!(app.dispatch_click(done_box));
        let two = app.find_widget(Role::Tab, Some("Two")).unwrap();
        assert!(app.dispatch_click(two));
        app.frame();
        assert_eq!(draft.get(), "milk");
        assert!(done.get());
        assert_eq!(tab.get(), 1);

        draft.set(String::new());
        done.set(false);
        tab.set(0);
        assert!(app.set_theme(theme::DARK));
        app.frame();
        assert_eq!(a11y(&app, Role::TextInput).0.as_deref(), Some(""));
        assert_eq!(a11y(&app, Role::CheckBox).1 & 1, 0, "the app unticked it");
        assert!(selected(&app, Role::Tab, "One"));
        assert!(!selected(&app, Role::Tab, "Two"));
    }

    /// A control with a change handler is the app's, even when the handler
    /// only watches: the rebuild shows what the app authored, and nothing
    /// fires while rebuilding. A control the app bound shows the signal.
    #[test]
    fn a_control_with_a_change_handler_shows_what_the_app_authored() {
        let fired = std::rc::Rc::new(std::cell::Cell::new(0));
        let bound = create_signal(false);
        let f = fired.clone();
        let mut app = App::mount_themed(theme::LIGHT, move || {
            let (a, b, c) = (f.clone(), f.clone(), f.clone());
            Column::new()
                .child(
                    Checkbox::new(false)
                        .name("Watched")
                        .on_toggle(move |_| a.set(a.get() + 1)),
                )
                .child(
                    Checkbox::new(bound.get())
                        .name("Bound")
                        .on_toggle(move |v| bound.set(v)),
                )
                .child(
                    Slider::new(0.0, 0.0, 10.0)
                        .step(1.0)
                        .on_change(move |_| b.set(b.get() + 1)),
                )
                .child(
                    TextInput::new("")
                        .label("Draft")
                        .on_input(move |_| c.set(c.get() + 1)),
                )
        });
        app.frame();
        for name in ["Watched", "Bound"] {
            let id = app.find_widget(Role::CheckBox, Some(name)).unwrap();
            assert!(app.dispatch_click(id));
        }
        let slider = app.find_widget(Role::Slider, None).unwrap();
        app.focus(Some(slider));
        assert!(app.dispatch_key(UiKey::Right {
            shift: false,
            ctrl: false
        }));
        let input = app.find_widget(Role::TextInput, None).unwrap();
        app.focus(Some(input));
        assert!(app.dispatch_key(UiKey::Char("x")));
        let before = fired.get();
        assert_eq!(before, 3);
        assert!(bound.get());
        assert!(app.set_theme(theme::DARK));
        app.frame();
        assert_eq!(fired.get(), before, "no handler runs while rebuilding");
        let checked = |app: &App, name| {
            let id = app.find_widget(Role::CheckBox, Some(name)).unwrap();
            app.scene().a11y(id).unwrap().state & 1 == 1
        };
        assert!(!checked(&app, "Watched"), "what the app authored");
        assert!(checked(&app, "Bound"), "the signal the app wrote");
        assert_eq!(a11y(&app, Role::Slider).0.as_deref(), Some("0"));
        assert_eq!(a11y(&app, Role::TextInput).0.as_deref(), Some(""));
    }
}

/// Submittable forms: required fields, submit buttons, implicit submission.
mod forms {
    use super::*;
    use schnellui::a11y::{accesskit_reexport::Action, StateFlags};
    use schnellui::scene::{Color, Primitive, WidgetId};
    use schnellui::widgets::{Button, Checkbox, Column, Form, TextArea, TextInput};
    use std::cell::Cell;
    use std::rc::Rc;

    fn flags(app: &App, id: WidgetId) -> StateFlags {
        StateFlags(app.scene().a11y(id).unwrap().state)
    }

    fn invalid(app: &App, id: WidgetId) -> bool {
        flags(app, id).contains(StateFlags::INVALID)
    }

    fn input(app: &App, name: &str) -> WidgetId {
        app.find_widget(Role::TextInput, Some(name)).unwrap()
    }

    fn submit_button(app: &App) -> WidgetId {
        app.find_widget(Role::Button, Some("Send")).unwrap()
    }

    /// The colour of a field's outer border rect, its first primitive.
    fn border(app: &App, id: WidgetId) -> Color {
        match app.scene().paint(id).unwrap().primitives[0] {
            Primitive::SolidRect { color, .. } => color,
            ref other => panic!("expected a border rect, got {other:?}"),
        }
    }

    /// The colour of the focus ring stroke a field wears, if any.
    fn ring(app: &App, id: WidgetId) -> Option<Color> {
        app.scene()
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .rev()
            .find_map(|p| match p {
                Primitive::StrokeRect { color, .. } => Some(*color),
                _ => None,
            })
    }

    fn type_text(app: &mut App, text: &'static str) {
        for ch in text.char_indices() {
            let end = ch.0 + ch.1.len_utf8();
            assert!(app.dispatch_key(UiKey::Char(&text[ch.0..end])));
        }
    }

    /// Two required inputs, one optional input and a submit button.
    fn contact_form(submits: Rc<Cell<u32>>) -> Form {
        Form::new()
            .label("Contact")
            .on_submit(move || submits.set(submits.get() + 1))
            .child(TextInput::new("").label("Name").required(true))
            .child(TextInput::new("").label("Email").required(true))
            .child(TextInput::new("").label("Nickname"))
            .child(Button::new("Send").submit())
    }

    fn find_dump<'a>(
        node: &'a schnellui::a11y::A11yNodeDump,
        role: &str,
        name: &str,
    ) -> Option<&'a schnellui::a11y::A11yNodeDump> {
        if node.role == role && node.name.as_deref() == Some(name) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_dump(child, role, name))
    }

    #[test]
    fn empty_required_fields_block_submit_then_enter_submits() {
        let submits = Rc::new(Cell::new(0));
        let mut app = App::mount(contact_form(submits.clone()));
        app.frame();
        let (name, email, nick) = (
            input(&app, "Name"),
            input(&app, "Email"),
            input(&app, "Nickname"),
        );
        assert!(flags(&app, name).contains(StateFlags::REQUIRED));
        assert!(flags(&app, email).contains(StateFlags::REQUIRED));
        assert!(!flags(&app, nick).contains(StateFlags::REQUIRED));
        assert!(app.find_widget(Role::Form, Some("Contact")).is_some());

        // Nothing filled: blocked, first required field focused with the ring.
        assert!(app.dispatch_click(submit_button(&app)));
        // The change reaches the incremental AccessKit update, not only a rebuild.
        let incremental = app.a11y_tree_update();
        for id in [name, email] {
            let key = schnellui::a11y::to_access_id(id);
            let (_, node) = incremental.nodes.iter().find(|(k, _)| *k == key).unwrap();
            assert_eq!(node.invalid(), Some(accesskit::Invalid::True));
        }
        assert!(incremental
            .nodes
            .iter()
            .all(|(k, _)| *k != schnellui::a11y::to_access_id(nick)));
        app.frame();
        assert_eq!(submits.get(), 0);
        assert_eq!(app.focused_widget(), Some(name));
        assert!(app.focus_ring_shown());
        assert!(invalid(&app, name) && invalid(&app, email));
        assert!(!invalid(&app, nick), "an optional field is never invalid");

        // The a11y dump and the real AccessKit tree agree with the scene.
        let dump = schnellui::a11y::dump_tree(app.scene()).root.unwrap();
        assert!(find_dump(&dump, "form", "Contact").is_some());
        for field in ["Name", "Email"] {
            let node = find_dump(&dump, "text_input", field).unwrap();
            assert!(node.state.contains(&"required".to_string()));
            assert!(node.state.contains(&"invalid".to_string()));
        }
        let nick_dump = find_dump(&dump, "text_input", "Nickname").unwrap();
        assert!(nick_dump.state.is_empty());
        let update = schnellui::a11y::build_full_tree_update(app.scene());
        let node_of = |id: WidgetId| {
            let key = schnellui::a11y::to_access_id(id);
            &update.nodes.iter().find(|(k, _)| *k == key).unwrap().1
        };
        assert!(node_of(name).is_required());
        assert_eq!(node_of(name).invalid(), Some(accesskit::Invalid::True));
        assert_eq!(node_of(nick).invalid(), None);
        assert_eq!(
            node_of(app.find_widget(Role::Form, None).unwrap()).role(),
            accesskit::Role::Form
        );

        // What is painted matches: error outline on exactly the invalid fields.
        let error = app.active_theme().invalid_color();
        assert_eq!(border(&app, name), error);
        assert_eq!(border(&app, email), error);
        assert_ne!(border(&app, nick), error);
        // The focus ring does not hide the error: it wears the error colour.
        assert_eq!(ring(&app, name), Some(error));

        // Typing a value clears that field's own INVALID, and only its own.
        type_text(&mut app, "Ann");
        assert!(!invalid(&app, name));
        assert!(invalid(&app, email));
        assert_ne!(border(&app, name), error);
        assert_ne!(ring(&app, name), Some(error));
        assert!(ring(&app, name).is_some(), "still focused, still ringed");

        // The next submit skips the filled field and focuses the next one.
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!(submits.get(), 0);
        assert_eq!(app.focused_widget(), Some(email));
        assert!(invalid(&app, email));

        // Fill it and press Enter in it: implicit submission, exactly once.
        type_text(&mut app, "a@b.c");
        assert!(app.dispatch_key(UiKey::Enter));
        assert_eq!(submits.get(), 1);
        for id in [name, email, nick] {
            assert!(!invalid(&app, id));
        }
        assert_ne!(border(&app, email), error);
    }

    #[test]
    fn required_checkbox_blocks_submission_and_takes_focus() {
        let submits = Rc::new(Cell::new(0));
        let s = submits.clone();
        let mut app = App::mount(
            Form::new()
                .on_submit(move || s.set(s.get() + 1))
                .child(TextInput::new("x").label("Name").required(true))
                .child(Checkbox::new(false).name("Agree").required(true))
                .child(Button::new("Send").submit()),
        );
        app.frame();
        let agree = app.find_widget(Role::CheckBox, Some("Agree")).unwrap();
        assert!(flags(&app, agree).contains(StateFlags::REQUIRED));
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!(submits.get(), 0);
        assert_eq!(app.focused_widget(), Some(agree));
        assert!(app.focus_ring_shown());
        assert!(invalid(&app, agree));
        assert!(
            !invalid(&app, input(&app, "Name")),
            "a filled field is valid"
        );
        assert_eq!(border(&app, agree), app.active_theme().invalid_color());

        // Ticking it clears INVALID; the form then submits.
        assert!(app.dispatch_key(UiKey::Space { shift: false }));
        assert!(!invalid(&app, agree));
        assert_ne!(border(&app, agree), app.active_theme().invalid_color());
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!(submits.get(), 1);

        // Unticking again makes it missing again.
        assert!(app.dispatch_click(agree));
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!(submits.get(), 1);
        assert!(invalid(&app, agree));
    }

    #[test]
    fn an_input_with_its_own_on_submit_keeps_enter() {
        let form = Rc::new(Cell::new(0));
        let own = Rc::new(Cell::new(0));
        let (f, o) = (form.clone(), own.clone());
        let mut app = App::mount(
            Form::new()
                .on_submit(move || f.set(f.get() + 1))
                .child(
                    TextInput::new("")
                        .label("Search")
                        .required(true)
                        .on_submit(move || o.set(o.get() + 1)),
                )
                .child(TextInput::new("").label("Plain")),
        );
        app.frame();
        let search = input(&app, "Search");
        app.focus(Some(search));
        assert!(app.dispatch_key(UiKey::Enter));
        assert_eq!((own.get(), form.get()), (1, 0));
        assert!(!invalid(&app, search), "its own handler does not validate");

        // The plain field, with nothing required missing, submits the form.
        type_text(&mut app, "q");
        let plain = input(&app, "Plain");
        app.focus(Some(plain));
        assert!(app.dispatch_key(UiKey::Enter));
        assert_eq!((own.get(), form.get()), (1, 1));
    }

    #[test]
    fn access_kit_click_on_submit_matches_a_pointer_click() {
        let click = |app: &mut App| {
            app.dispatch_action(&accesskit_action::ActionRequest {
                action: Action::Click,
                target_tree: accesskit_reexport::TreeId::ROOT,
                target_node: schnellui::a11y::to_access_id(submit_button(app)),
                data: None,
            })
        };
        let submits = Rc::new(Cell::new(0));
        let mut app = App::mount(contact_form(submits.clone()));
        app.frame();
        assert!(click(&mut app));
        assert_eq!(submits.get(), 0);
        assert_eq!(app.focused_widget(), Some(input(&app, "Name")));
        assert!(app.focus_ring_shown());
        assert!(invalid(&app, input(&app, "Name")));
        assert!(invalid(&app, input(&app, "Email")));

        for (field, text) in [("Name", "Ann"), ("Email", "a@b.c")] {
            let id = input(&app, field);
            assert!(app.dispatch_action(&accesskit_action::ActionRequest {
                action: Action::SetValue,
                target_tree: accesskit_reexport::TreeId::ROOT,
                target_node: schnellui::a11y::to_access_id(id),
                data: Some(accesskit_action::ActionData::Value(text.into())),
            }));
            assert!(!invalid(&app, id), "SetValue clears INVALID like typing");
        }
        assert!(click(&mut app));
        assert_eq!(submits.get(), 1);
    }

    #[test]
    fn keyboard_activates_the_submit_button() {
        let submits = Rc::new(Cell::new(0));
        let mut app = App::mount(contact_form(submits.clone()));
        app.frame();
        let send = submit_button(&app);
        app.focus(Some(send));
        assert!(app.dispatch_key(UiKey::Enter));
        assert_eq!(submits.get(), 0);
        assert_eq!(app.focused_widget(), Some(input(&app, "Name")));
        type_text(&mut app, "Ann");
        app.focus(Some(input(&app, "Email")));
        type_text(&mut app, "a@b.c");
        app.focus(Some(send));
        assert!(app.dispatch_key(UiKey::Space { shift: false }));
        assert_eq!(submits.get(), 1);
    }

    #[test]
    fn a_form_keeps_working_across_a_theme_switch() {
        let submits = Rc::new(Cell::new(0));
        let s = submits.clone();
        let mut app = App::mount_themed(theme::LIGHT, move || contact_form(s.clone()));
        app.frame();
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!(submits.get(), 0);
        assert_eq!(app.focused_widget(), Some(input(&app, "Name")));

        assert!(app.set_theme(theme::DARK));
        app.frame();
        let (name, email) = (input(&app, "Name"), input(&app, "Email"));
        assert!(flags(&app, name).contains(StateFlags::REQUIRED));
        // INVALID is not carried across a rebuild: the next submit marks it again.
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!(submits.get(), 0);
        assert!(invalid(&app, name) && invalid(&app, email));
        assert_eq!(app.focused_widget(), Some(name));
        assert_eq!(border(&app, name), app.active_theme().invalid_color());

        type_text(&mut app, "Ann");
        app.focus(Some(email));
        type_text(&mut app, "a@b.c");
        assert!(app.dispatch_key(UiKey::Enter));
        assert_eq!(submits.get(), 1);

        // A successful form survives another rebuild with its typed values.
        assert!(app.set_theme(theme::LIGHT));
        app.frame();
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!(submits.get(), 2);
    }

    #[test]
    fn a_submit_button_submits_only_its_own_form() {
        let (a_count, b_count) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let (a, b) = (a_count.clone(), b_count.clone());
        let mut app = App::mount(
            Column::new()
                .child(
                    Form::new()
                        .label("A")
                        .on_submit(move || a.set(a.get() + 1))
                        .child(TextInput::new("").label("A field").required(true))
                        .child(Button::new("Send A").submit()),
                )
                .child(
                    Form::new()
                        .label("B")
                        .on_submit(move || b.set(b.get() + 1))
                        .child(TextInput::new("").label("B field").required(true))
                        .child(Button::new("Send B").submit()),
                ),
        );
        app.frame();
        let send = |app: &App, name| app.find_widget(Role::Button, Some(name)).unwrap();
        assert!(app.dispatch_click(send(&app, "Send B")));
        assert_eq!((a_count.get(), b_count.get()), (0, 0));
        assert!(invalid(&app, input(&app, "B field")));
        assert!(
            !invalid(&app, input(&app, "A field")),
            "form A is untouched"
        );
        assert_eq!(app.focused_widget(), Some(input(&app, "B field")));

        // A filled field in A does not let B's button submit A, or B itself.
        app.focus(Some(input(&app, "A field")));
        type_text(&mut app, "x");
        assert!(app.dispatch_click(send(&app, "Send B")));
        assert_eq!((a_count.get(), b_count.get()), (0, 0));

        app.focus(Some(input(&app, "B field")));
        type_text(&mut app, "y");
        assert!(app.dispatch_click(send(&app, "Send B")));
        assert_eq!((a_count.get(), b_count.get()), (0, 1));
        assert!(app.dispatch_click(send(&app, "Send A")));
        assert_eq!((a_count.get(), b_count.get()), (1, 1));
    }

    #[test]
    fn nested_forms_submit_the_nearest_one() {
        let (outer, inner) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let (o, i) = (outer.clone(), inner.clone());
        let mut app = App::mount(
            Form::new()
                .label("Outer")
                .on_submit(move || o.set(o.get() + 1))
                .child(TextInput::new("").label("Outer field").required(true))
                .child(
                    Form::new()
                        .label("Inner")
                        .on_submit(move || i.set(i.get() + 1))
                        .child(TextInput::new("").label("Inner field").required(true))
                        .child(Button::new("Send").submit()),
                ),
        );
        app.frame();
        assert!(app.dispatch_click(submit_button(&app)));
        // Only the inner form's field is checked and focused.
        assert!(invalid(&app, input(&app, "Inner field")));
        assert!(!invalid(&app, input(&app, "Outer field")));
        app.focus(Some(input(&app, "Inner field")));
        type_text(&mut app, "x");
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!((outer.get(), inner.get()), (0, 1));
    }

    #[test]
    fn enter_in_a_text_area_inserts_a_newline_and_required_areas_count() {
        let submits = Rc::new(Cell::new(0));
        let s = submits.clone();
        let mut app = App::mount(
            Form::new()
                .on_submit(move || s.set(s.get() + 1))
                .child(TextArea::new("").placeholder("Notes").required(true))
                .child(Button::new("Send").submit()),
        );
        app.frame();
        let notes = app
            .find_widget(Role::MultilineTextInput, Some("Notes"))
            .unwrap();
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!(submits.get(), 0);
        assert_eq!(app.focused_widget(), Some(notes));
        assert!(invalid(&app, notes));
        assert_eq!(border(&app, notes), app.active_theme().invalid_color());
        assert_eq!(ring(&app, notes), Some(app.active_theme().invalid_color()));

        type_text(&mut app, "a");
        assert!(!invalid(&app, notes));
        assert_ne!(ring(&app, notes), Some(app.active_theme().invalid_color()));
        assert_ne!(border(&app, notes), app.active_theme().invalid_color());
        assert!(app.dispatch_key(UiKey::Enter));
        assert_eq!(submits.get(), 0, "Enter in a text area is a newline");
        assert_eq!(
            app.scene().a11y(notes).unwrap().value.as_deref(),
            Some("a\n")
        );
        assert!(app.dispatch_click(submit_button(&app)));
        assert_eq!(submits.get(), 1);
    }

    #[test]
    fn a_submit_button_outside_a_form_is_inert_and_a_plain_button_does_not_submit() {
        let submits = Rc::new(Cell::new(0));
        let clicks = Rc::new(Cell::new(0));
        let (s, c) = (submits.clone(), clicks.clone());
        let mut app = App::mount(
            Column::new()
                .child(
                    Button::new("Orphan")
                        .submit()
                        .on_click(move || c.set(c.get() + 1)),
                )
                .child(
                    Form::new()
                        .on_submit(move || s.set(s.get() + 1))
                        .child(Button::new("Plain").on_click(|| {})),
                ),
        );
        app.frame();
        let orphan = app.find_widget(Role::Button, Some("Orphan")).unwrap();
        assert!(app.dispatch_click(orphan));
        assert_eq!(clicks.get(), 1, "its own on_click still runs");
        let plain = app.find_widget(Role::Button, Some("Plain")).unwrap();
        assert!(app.dispatch_click(plain));
        assert_eq!(submits.get(), 0);
    }

    #[test]
    fn a_host_dispatching_through_the_widget_api_submits_on_its_next_frame() {
        let submits = Rc::new(Cell::new(0));
        let mut app = App::mount(contact_form(submits.clone()));
        app.frame();
        let send = submit_button(&app);
        let widgets = app.widgets.clone();
        // The windowed pointer path calls `dispatch_click` on the widget layer.
        assert!(schnellui::widgets::dispatch_click(
            &widgets,
            app.scene_mut(),
            send
        ));
        assert_eq!(app.focused_widget(), None, "nothing ran yet");
        app.frame();
        assert_eq!(app.focused_widget(), Some(input(&app, "Name")));
        assert!(invalid(&app, input(&app, "Email")));
        assert_eq!(submits.get(), 0);
    }
}
