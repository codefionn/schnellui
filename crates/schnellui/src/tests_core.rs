use super::*;
use schnellui_a11y::Role;
use schnellui_scene::WidgetKind;

#[test]
fn terminal_content_repaints_without_layout_invalidation() {
    use widgets::{TerminalGrid, TerminalGridModel, TerminalGridPoint};

    let revision = schnellui_signal::create_signal(0_u64);
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
    assert!(schnellui_widgets::scroll_is_at_end(app.scene(), scroll));

    // Stream growth arrives as a subtree replacement of just the content.
    let mut grown = Column::new();
    for row in 0..60 {
        grown = grown.child(Text::new(format!("Row {row}")));
    }
    app.replace_subtree(content_ref.clone(), grown).unwrap();
    app.frame();
    assert!(
        schnellui_widgets::scroll_is_at_end(app.scene(), scroll),
        "an at-end follow_end viewport must stay pinned through replacement"
    );

    // Once the user scrolls away, replacement growth preserves the position.
    assert!(schnellui_widgets::dispatch_scroll(
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
    assert!(schnellui_widgets::dispatch_scroll(
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
    assert!(schnellui_widgets::scroll_is_at_end(app.scene(), scroll));
    controller.insert(80, 80);
    app.frame();
    assert!(
        schnellui_widgets::scroll_is_at_end(app.scene(), scroll),
        "a pinned virtual transcript follows controller growth"
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
        schnellui_widgets::scroll_is_at_end(app.scene(), scroll),
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
    assert!(schnellui_widgets::scroll_is_at_end(app.scene(), scroll));
    let mut completed_steps = 0;
    for step in 0..512 {
        if !schnellui_widgets::dispatch_scroll(
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

    use schnellui_widgets::{CursorIcon, Image, Scroll};

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
    use schnellui_scene::Rect;
    use schnellui_widgets::{Column, Text};

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
    use schnellui_layout::{em, px, ResponsiveQuery};
    use schnellui_widgets::{Button, Column, View as _};

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

    let order = schnellui_a11y::tab_order(app.scene());
    assert_eq!(order.len(), 1, "only the visible button is focusable");
}

#[test]
fn view_macro_accepts_show_when_on_any_component() {
    use schnellui_layout::ResponsiveQuery;

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
    use schnellui_layout::ResponsiveQuery;
    use schnellui_scene::ComponentRef;

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

    use schnellui_scene::Point;
    use schnellui_widgets::{Button, DragRelease, Row};

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
    assert!(schnellui_widgets::dispatch_click(
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
fn dock_area_resolves_right_edge_and_previews_the_result_above_content() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui_scene::Point;
    use schnellui_widgets::{Button, DockArea, DockPosition, DragRelease, Row};

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
    use schnellui_widgets::{Button, Column};

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

#[test]
fn modal_dialog_traps_focus_blocks_background_actions_and_takes_escape() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui_widgets::{Button, Column, Dialog};

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
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(cancel));

    // The dialog is the complete screen-reader tree while it is modal.
    let dump = schnellui_a11y::dump_tree(app.scene());
    let exposed = dump.root.expect("modal accessibility root");
    assert_eq!(exposed.role, "dialog");
    assert_eq!(exposed.name.as_deref(), Some("Confirm"));
    assert_eq!(dump.focus, Some(schnellui_a11y::to_access_id(cancel).0));
    fn contains_name(node: &schnellui_a11y::A11yNodeDump, name: &str) -> bool {
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
        target_node: schnellui_a11y::to_access_id(background),
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

    use schnellui_scene::Point;
    use schnellui_widgets::{Button, Column, Dialog};

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
    assert!(app.focus_step(false));
    assert_eq!(app.focused_widget(), Some(top_cancel));

    let exposed = schnellui_a11y::dump_tree(app.scene())
        .root
        .expect("top modal accessibility root");
    assert_eq!(exposed.role, "alert_dialog");
    assert_eq!(exposed.name.as_deref(), Some("Top confirmation"));

    let click = |id| accesskit_action::ActionRequest {
        action: accesskit_action::Action::Click,
        target_tree: accesskit_reexport::TreeId::ROOT,
        target_node: schnellui_a11y::to_access_id(id),
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
        schnellui_widgets::hit_test(&app.widgets, app.scene(), Point { x: 1.0, y: 1.0 })
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
    use schnellui_widgets::{Button, Column, Dialog};

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
        schnellui_a11y::tab_order(app.scene()),
        vec![workspace, left, right]
    );

    let dump = schnellui_a11y::dump_tree(app.scene());
    assert_ne!(
        dump.root.as_ref().and_then(|node| node.name.as_deref()),
        Some("Left inspector")
    );
    let json = schnellui_a11y::dump_json(app.scene());
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
    use schnellui_a11y::accesskit_reexport::{Action, ActionRequest, TreeId};
    use schnellui_a11y::to_access_id;
    use schnellui_widgets::{Column, Scroll, Text};

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
    use schnellui_a11y::accesskit_reexport::{Action, ActionData, ActionRequest, TreeId};
    use schnellui_a11y::to_access_id;
    use schnellui_widgets::{Column, EditKey, TextInput};
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
    use schnellui_widgets::{Button, Checkbox, Column};
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
        !schnellui_a11y::StateFlags(app.scene().a11y(cb).unwrap().state)
            .contains(schnellui_a11y::StateFlags::CHECKED)
    );
    // … Space does.
    assert!(app.dispatch_key(UiKey::Space { shift: false }));
    assert!(
        schnellui_a11y::StateFlags(app.scene().a11y(cb).unwrap().state)
            .contains(schnellui_a11y::StateFlags::CHECKED)
    );
    // Shift+Tab walks back to the button.
    assert!(app.dispatch_key(UiKey::Tab { shift: true }));
    assert_eq!(app.focused_widget(), Some(btn));
}

#[test]
fn pointer_focus_is_semantic_but_keyboard_promotes_focus_visible() {
    use schnellui_scene::Primitive;
    use schnellui_widgets::Button;

    let mut app = App::mount_with_size(Button::new("Save"), 240, 120);
    app.frame();
    let button = app.find_widget(Role::Button, Some("Save")).unwrap();
    let line_count = |app: &App| {
        app.scene()
            .paint(button)
            .unwrap()
            .primitives
            .iter()
            .filter(|primitive| matches!(primitive, Primitive::Line { .. }))
            .count()
    };

    assert!(app.pointer_focus(Some(button)));
    assert_eq!(app.focused_widget(), Some(button));
    assert_eq!(line_count(&app), 0, "pointer focus hides keyboard ring");

    assert!(app.dispatch_key(UiKey::Enter));
    assert_eq!(line_count(&app), 4, "keyboard input reveals focus ring");

    assert!(app.pointer_focus(Some(button)));
    assert_eq!(app.focused_widget(), Some(button));
    assert_eq!(line_count(&app), 0);
}

#[test]
fn terminal_like_scroll_surface_focuses_from_content_and_receives_raw_keys() {
    use schnellui_widgets::{Scroll, Text};
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
    use schnellui_widgets::{Scroll, Text};
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

/// A focused slider takes the range keys — arrows ±1 step, PageUp/PageDown
/// ±10, Home/End min/max — and an inbound AccessKit `Increment`/`Decrement`
/// action reaches the identical adjust path (SOUL §6.3).
#[test]
fn slider_keys_and_increment_actions_adjust_the_value() {
    use schnellui_a11y::accesskit_reexport::{Action, ActionData, ActionRequest, TreeId};
    use schnellui_a11y::to_access_id;
    use schnellui_widgets::{Column, Slider};

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
    assert!(app.dispatch_key(UiKey::End { shift: false }));
    assert_eq!(
        app.scene().a11y(slider).unwrap().value.as_deref(),
        Some("100")
    );
    // at the max, a further increment is a no-op
    assert!(!app.dispatch_key(UiKey::Up { shift: false }));
    assert!(app.dispatch_key(UiKey::Home { shift: false }));
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
