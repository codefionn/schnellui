use super::*;
use schnellui::a11y::Role;
use schnellui::scene::WidgetKind;

#[test]
fn remount_focus_restoration_keeps_a_fresh_dropdown_open() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::widgets::{Button, Column, Dropdown, DropdownOption};

    let dismissals = Rc::new(Cell::new(0));
    let dismissal_count = dismissals.clone();
    let view = Column::new()
        .child(
            Dropdown::new("Theme")
                .open(true)
                .option(DropdownOption::new("Light").selected(true))
                .option(DropdownOption::new("Dark"))
                .on_toggle(move || dismissal_count.set(dismissal_count.get() + 1)),
        )
        .child(Button::new("Previously focused"));
    let mut app = App::mount_with_size(view, 400, 300);
    app.frame();
    let trigger = app.find_widget(Role::ComboBox, Some("Theme")).unwrap();
    let previous = app
        .find_widget(Role::Button, Some("Previously focused"))
        .unwrap();

    assert!(app.restore_focus_after_remount(RemountFocus {
        target: previous,
        ring_visible: true,
    }));
    assert_eq!(app.focused_widget(), Some(previous));
    assert_eq!(
        dismissals.get(),
        0,
        "restoring old focus is not an outside pointer interaction"
    );
    assert!(
        schnellui::a11y::StateFlags(app.scene().a11y(trigger).unwrap().state)
            .contains(schnellui::a11y::StateFlags::EXPANDED)
    );
}

#[test]
fn dialog_geometry_survives_closing_and_opening_a_peer() {
    use schnellui::scene::Point;
    use schnellui::widgets::{Dialog, Stack, Text};

    fn workspace(show_peer: bool) -> impl View {
        let mut dialogs = Stack::new().fill().child(
            Dialog::new("Issue board")
                .modeless()
                .non_fixed()
                .movable()
                .resizable()
                .at(30.0, 60.0)
                .size(300.0, 200.0)
                .child(Text::new("Issue")),
        );
        if show_peer {
            dialogs = dialogs.child(
                Dialog::new("Proof preview")
                    .modeless()
                    .non_fixed()
                    .movable()
                    .at(400.0, 80.0)
                    .size(260.0, 180.0)
                    .child(Text::new("Proof")),
            );
        }
        dialogs
    }

    let mut before_close = App::mount_with_size(workspace(true), 800, 600);
    before_close.frame();
    let panel = before_close
        .find_widget(Role::Dialog, Some("Issue board"))
        .unwrap();
    let initial = before_close.scene().layout(panel).unwrap().rect;
    let title = Point {
        x: initial.x + 20.0,
        y: initial.y + 18.0,
    };
    assert!(before_close.begin_dialog_pointer(title));
    assert!(before_close.update_dialog_pointer(Point {
        x: title.x + 70.0,
        y: title.y + 45.0,
    }));
    assert!(before_close.end_dialog_pointer());
    before_close.frame();
    let moved = before_close.scene().layout(panel).unwrap().rect;

    let mut after_close = App::mount_with_size(workspace(false), 800, 600);
    after_close.inherit_dialog_geometry(&before_close);
    after_close.frame();
    let panel = after_close
        .find_widget(Role::Dialog, Some("Issue board"))
        .unwrap();
    assert_eq!(after_close.scene().layout(panel).unwrap().rect, moved);

    let mut after_open = App::mount_with_size(workspace(true), 800, 600);
    after_open.inherit_dialog_geometry(&after_close);
    after_open.frame();
    let panel = after_open
        .find_widget(Role::Dialog, Some("Issue board"))
        .unwrap();
    assert_eq!(after_open.scene().layout(panel).unwrap().rect, moved);
}

#[test]
fn untouched_dialog_uses_replacement_authored_geometry() {
    use schnellui::widgets::{Dialog, Stack, Text};

    fn workspace(x: f32, width: f32) -> Stack {
        Stack::new().fill().child(
            Dialog::new("Inspector")
                .modeless()
                .non_fixed()
                .movable()
                .resizable()
                .at(x, 40.0)
                .size(width, 180.0)
                .child(Text::new("Content")),
        )
    }

    let mut previous = App::mount_with_size(workspace(20.0, 280.0), 900, 600);
    previous.frame();
    let mut authored = App::mount_with_size(workspace(360.0, 420.0), 900, 600);
    authored.frame();
    let authored_panel = authored
        .find_widget(Role::Dialog, Some("Inspector"))
        .unwrap();
    let authored_rect = authored.scene().layout(authored_panel).unwrap().rect;

    let mut replacement = App::mount_with_size(workspace(360.0, 420.0), 900, 600);
    replacement.inherit_remount_state(&previous);
    replacement.frame();

    let panel = replacement
        .find_widget(Role::Dialog, Some("Inspector"))
        .unwrap();
    let rect = replacement.scene().layout(panel).unwrap().rect;
    assert_eq!(rect, authored_rect);
}

#[test]
fn adjusted_dialog_geometry_follows_component_refs_across_reordering() {
    use schnellui::widgets::{ComponentRef, Dialog, Stack, Text, View as _};

    fn dialog(reference: ComponentRef, x: f32) -> impl View {
        Dialog::new("Inspector")
            .modeless()
            .non_fixed()
            .movable()
            .resizable()
            .at(x, 40.0)
            .size(260.0, 180.0)
            .child(Text::new("Content"))
            .with_ref(reference)
    }

    fn workspace(first: ComponentRef, second: ComponentRef, reversed: bool) -> Stack {
        if reversed {
            Stack::new()
                .fill()
                .child(dialog(second, 420.0))
                .child(dialog(first, 20.0))
        } else {
            Stack::new()
                .fill()
                .child(dialog(first, 20.0))
                .child(dialog(second, 420.0))
        }
    }

    fn panel_in(scene: &Scene, root: WidgetId) -> Option<WidgetId> {
        if scene
            .node(root)
            .is_some_and(|node| node.kind == WidgetKind::Dialog)
        {
            return Some(root);
        }
        scene
            .node(root)?
            .children
            .iter()
            .find_map(|child| panel_in(scene, *child))
    }

    let first_ref = ComponentRef::new();
    let second_ref = ComponentRef::new();
    let mut previous = App::mount_with_size(workspace(first_ref, second_ref, false), 1000, 600);
    previous.frame();
    let first_layer = previous.resolve_ref(first_ref).unwrap();
    let first_panel = panel_in(previous.scene(), first_layer).unwrap();
    let initial = previous.scene().layout(first_panel).unwrap().rect;
    let title = Point {
        x: initial.x + 18.0,
        y: initial.y + 18.0,
    };
    assert!(previous.begin_dialog_pointer(title));
    assert!(previous.update_dialog_pointer(Point {
        x: title.x + 72.0,
        y: title.y + 34.0,
    }));
    assert!(previous.end_dialog_pointer());
    previous.frame();
    let adjusted = previous.scene().layout(first_panel).unwrap().rect;

    let mut replacement = App::mount_with_size(workspace(first_ref, second_ref, true), 1000, 600);
    replacement.inherit_remount_state(&previous);
    replacement.frame();
    let first_layer = replacement.resolve_ref(first_ref).unwrap();
    let first_panel = panel_in(replacement.scene(), first_layer).unwrap();
    assert_eq!(
        replacement.scene().layout(first_panel).unwrap().rect,
        adjusted
    );
}

#[test]
fn themed_mount_rebuilds_and_owns_the_page_color() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::scene::Primitive;
    use schnellui::widgets::Button;

    let builds = Rc::new(Cell::new(0));
    let count = builds.clone();
    let mut app = App::mount_themed(theme::LIGHT, move || {
        count.set(count.get() + 1);
        Button::new("themed")
    });
    assert_eq!(builds.get(), 1);
    assert_eq!(app.clear_color(), theme::LIGHT.page);
    assert!(app.set_theme(theme::DARK));
    assert_eq!(builds.get(), 2);
    assert_eq!(app.active_theme(), theme::DARK);
    assert_eq!(app.clear_color(), theme::DARK.page);
    let root = app.scene().root().unwrap();
    assert!(matches!(
        app.scene().paint(root).unwrap().primitives[0],
        Primitive::SolidRect { color, .. } if color == theme::DARK.accent
    ));
}

#[test]
fn theme_rebuild_keeps_the_atlas_without_allocating_another() {
    use schnellui::widgets::Text;

    let mut app = App::mount_themed(theme::LIGHT, || Text::new("glyphs to cache"));
    app.frame();
    let atlas_id = app.atlas.id();
    let rasterized = app.text.rasterized_glyph_count();
    assert!(rasterized > 0);
    let before = crate::app_new::FRESH_ENGINES.with(|c| c.get());
    let shapers = schnellui::text::TextShaper::full_builds();
    assert!(app.set_theme(theme::DARK));
    app.frame();
    assert_eq!(schnellui::text::TextShaper::full_builds(), shapers);
    assert_eq!(crate::app_new::FRESH_ENGINES.with(|c| c.get()), before);
    assert_eq!(app.atlas.id(), atlas_id);
    assert_eq!(app.text.rasterized_glyph_count(), rasterized);
}

#[test]
fn theme_remount_preserves_interaction_state_and_host_registrations() {
    use std::cell::Cell;
    use std::rc::Rc;

    use schnellui::widgets::{
        Column, ComponentRef, CursorIcon, Scroll, Text, TextInput, View as _,
    };

    let editor_ref = ComponentRef::new();
    let surface_ref = ComponentRef::new();
    let mut app = App::mount_themed(theme::LIGHT, move || {
        Column::new()
            .child(
                TextInput::new("alpha")
                    .placeholder("Editor")
                    .with_ref(editor_ref),
            )
            .child(
                Scroll::new()
                    .label("Raw surface")
                    .size(160.0, 80.0)
                    .child(Text::new("surface"))
                    .with_ref(surface_ref),
            )
    });
    app.frame();
    let editor = app.scene().resolve_ref(editor_ref).unwrap();
    let surface = app.scene().resolve_ref(surface_ref).unwrap();
    assert!(app.focus(Some(editor)));
    assert!(app.dispatch_key(UiKey::Home { shift: false }));
    assert!(app.focus(Some(surface)));

    let shortcuts = Rc::new(Cell::new(0));
    let invoked_shortcuts = shortcuts.clone();
    app.register_shortcut(Shortcut::command('r'), move || {
        invoked_shortcuts.set(invoked_shortcuts.get() + 1);
    });
    let raw_events = Rc::new(Cell::new(0));
    let observed_raw_events = raw_events.clone();
    app.register_focused_input_handler(Role::ScrollView, Some("Raw surface"), move |_| {
        observed_raw_events.set(observed_raw_events.get() + 1);
        FocusedInputResult::Handled
    });
    assert!(
        app.register_cursor_provider(Role::ScrollView, Some("Raw surface"), || {
            CursorIcon::Crosshair
        },)
    );

    assert!(app.set_theme(theme::DARK));
    app.frame();
    let editor = app.scene().resolve_ref(editor_ref).unwrap();
    let surface = app.scene().resolve_ref(surface_ref).unwrap();
    assert_eq!(app.focused_widget(), Some(surface));
    assert!(app.dispatch_shortcut(Shortcut::command('r')));
    assert_eq!(shortcuts.get(), 1);
    assert_eq!(
        app.dispatch_focused_input(FocusedInputEvent::Focus(RawFocusEvent::WindowGained)),
        FocusedInputResult::Handled
    );
    assert_eq!(raw_events.get(), 1);
    let rect = app.scene().layout(surface).unwrap().rect;
    assert_eq!(
        app.cursor_at(Point {
            x: rect.x + 2.0,
            y: rect.y + 2.0,
        }),
        CursorIcon::Crosshair
    );

    assert!(app.focus(Some(editor)));
    assert!(app.dispatch_key(UiKey::Char("Y")));
    assert_eq!(
        app.scene().a11y(editor).unwrap().value.as_deref(),
        Some("Yalpha")
    );
}

#[test]
fn remount_preserves_animated_loading_spinner_phase() {
    use schnellui::scene::Primitive;
    use schnellui::widgets::LoadingSpinner;

    fn alphas(app: &App, spinner: WidgetId) -> Vec<u8> {
        app.scene().paint(spinner).unwrap().primitives[1..]
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::Line { color, .. } => Some(color.a),
                _ => None,
            })
            .collect()
    }

    let mut previous = App::mount(LoadingSpinner::new().name("Synchronizing").phase(3));
    previous.frame();
    let previous_spinner = previous
        .find_widget(Role::ProgressIndicator, Some("Synchronizing"))
        .unwrap();
    let advanced = alphas(&previous, previous_spinner);

    let mut replacement = App::mount(LoadingSpinner::new().name("Synchronizing").phase(9));
    replacement.inherit_remount_state(&previous);
    let replacement_spinner = replacement
        .find_widget(Role::ProgressIndicator, Some("Synchronizing"))
        .unwrap();
    assert_eq!(alphas(&replacement, replacement_spinner), advanced);
    // The first tick starts the spinner clock; the next one advances it.
    let start = Instant::now();
    replacement.frame_at(start);
    replacement.frame_at(start + Duration::from_millis(100));
    assert_ne!(alphas(&replacement, replacement_spinner), advanced);

    let mut static_replacement = App::mount(
        LoadingSpinner::new()
            .name("Synchronizing")
            .phase(9)
            .animated(false),
    );
    let static_spinner = static_replacement
        .find_widget(Role::ProgressIndicator, Some("Synchronizing"))
        .unwrap();
    let authored_static = alphas(&static_replacement, static_spinner);
    static_replacement.inherit_remount_state(&previous);
    assert_eq!(alphas(&static_replacement, static_spinner), authored_static);

    let static_previous = App::mount(
        LoadingSpinner::new()
            .name("Synchronizing")
            .phase(4)
            .animated(false),
    );
    let mut animated_replacement = App::mount(LoadingSpinner::new().name("Synchronizing").phase(9));
    let animated_spinner = animated_replacement
        .find_widget(Role::ProgressIndicator, Some("Synchronizing"))
        .unwrap();
    let authored_animated = alphas(&animated_replacement, animated_spinner);
    animated_replacement.inherit_remount_state(&static_previous);
    assert_eq!(
        alphas(&animated_replacement, animated_spinner),
        authored_animated
    );
}

#[test]
fn theme_remount_preserves_loading_spinner_phase_and_paint_bindings() {
    use schnellui::scene::Primitive;
    use schnellui::signal::create_signal;
    use schnellui::widgets::{Button, Column, LoadingSpinner};

    fn alphas(app: &App, spinner: WidgetId) -> Vec<u8> {
        app.scene().paint(spinner).unwrap().primitives[1..]
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::Line { color, .. } => Some(color.a),
                _ => None,
            })
            .collect()
    }

    let mut app = App::mount_themed(theme::LIGHT, || {
        Column::new()
            .child(LoadingSpinner::new().name("Synchronizing").phase(2))
            .child(Button::new("Bound paint"))
    });
    let paint_target = app.find_widget(Role::Button, Some("Bound paint")).unwrap();
    let color = create_signal(Color::rgb(40, 50, 60));
    app.bind_paint(paint_target, move || color.get());
    app.frame();
    let spinner = app
        .find_widget(Role::ProgressIndicator, Some("Synchronizing"))
        .unwrap();
    let advanced = alphas(&app, spinner);

    assert!(app.set_theme(theme::DARK));
    let spinner = app
        .find_widget(Role::ProgressIndicator, Some("Synchronizing"))
        .unwrap();
    assert_eq!(alphas(&app, spinner), advanced);

    let next = Color::rgb(70, 80, 90);
    color.set(next);
    app.frame();
    let paint_target = app.find_widget(Role::Button, Some("Bound paint")).unwrap();
    assert!(matches!(
        app.scene().paint(paint_target).unwrap().primitives[0],
        Primitive::SolidRect { color, .. } if color == next
    ));
}

#[test]
fn system_mode_resolves_platform_scheme() {
    use schnellui::widgets::Text;

    let mode = ThemeMode::system(theme::LIGHT, theme::DARK);
    let mut app = App::mount_themed(mode, || Text::new("system"));
    assert_eq!(app.color_scheme(), ColorScheme::Light);
    assert_eq!(app.active_theme(), theme::LIGHT);
    assert!(app.apply_color_scheme(ColorScheme::Dark));
    assert_eq!(app.color_scheme(), ColorScheme::Dark);
    assert_eq!(app.active_theme(), theme::DARK);
    assert_eq!(app.clear_color(), theme::DARK.page);
}

#[test]
fn reduced_motion_freezes_spinners_until_motion_is_allowed_again() {
    use schnellui::widgets::LoadingSpinner;

    let mut app = App::mount(LoadingSpinner::new().phase(3));
    app.frame();
    let spinner = app.scene().root().unwrap();
    let before = app.scene().paint(spinner).unwrap().primitives[1];

    assert!(app.apply_reduced_motion(true));
    assert!(!app.animations_enabled());
    app.frame();
    assert_eq!(
        app.scene().paint(spinner).unwrap().primitives[1],
        before,
        "reduced motion keeps the current visual frame"
    );

    assert!(app.apply_reduced_motion(false));
    assert!(app.animations_enabled());
    app.frame();
    assert_ne!(app.scene().paint(spinner).unwrap().primitives[1], before);
}

#[test]
fn reduced_motion_finishes_an_active_floating_label_transition() {
    use schnellui::widgets::TextInput;

    let mut app = App::mount(TextInput::new("").label("Project name"));
    app.frame();
    let input = app.scene().root().unwrap();
    assert!(app.focus(Some(input)));
    assert!(schnellui::widgets::has_floating_label_animations(
        &app.widgets
    ));
    app.frame();
    assert!(schnellui::widgets::has_floating_label_animations(
        &app.widgets
    ));

    assert!(app.apply_reduced_motion(true));
    assert!(!schnellui::widgets::has_floating_label_animations(
        &app.widgets
    ));
}

#[test]
fn reduced_motion_completes_and_bypasses_theme_transitions() {
    use schnellui::widgets::Text;

    let mut app = App::mount_themed(theme::LIGHT, || Text::new("motion"));
    assert!(app.transition_theme(theme::DARK, Duration::from_secs(60)));
    assert!(app.theme_transition_active());

    assert!(app.apply_reduced_motion(true));
    assert!(!app.theme_transition_active());
    assert_eq!(app.active_theme(), theme::DARK);

    assert!(app.transition_theme(theme::LIGHT, Duration::from_secs(60)));
    assert!(!app.theme_transition_active());
    assert_eq!(app.active_theme(), theme::LIGHT);
}

#[test]
fn reactive_theme_binding_rebuilds_on_the_next_frame() {
    use schnellui::signal::create_signal;
    use schnellui::widgets::Text;

    let dark = create_signal(false);
    let read_dark = dark;
    let mut app = App::mount_themed(theme::LIGHT, || Text::new("reactive"));
    assert!(app.bind_theme(move || {
        if read_dark.get() {
            theme::DARK
        } else {
            theme::LIGHT
        }
    }));
    app.frame();
    assert_eq!(app.active_theme(), theme::LIGHT);
    dark.set(true);
    app.frame();
    assert_eq!(app.active_theme(), theme::DARK);
    assert_eq!(app.clear_color(), theme::DARK.page);
}

/// Regression for the playground's Text tab: wrapped paint owns a mutable
/// widget-runtime borrow, so theme lookup must never re-borrow that registry.
#[test]
fn themed_wrapped_text_frames_before_and_after_a_theme_remount() {
    use schnellui::scene::Primitive;
    use schnellui::widgets::{Text, WrapMode};

    let mut app = App::mount_themed(theme::LIGHT, || {
        Text::new("Text tab wrapped-content regression").wrap(WrapMode::Word)
    });
    app.frame();
    assert!(app.set_theme(theme::DARK));
    app.frame();

    let root = app.scene().root().unwrap();
    assert!(app
        .scene()
        .paint(root)
        .unwrap()
        .primitives
        .iter()
        .any(|primitive| {
            matches!(
                primitive,
                Primitive::GlyphQuad { color, .. } if *color == theme::DARK.text
            )
        }));
}

#[test]
fn generic_template_mounts_through_the_retained_adapter() {
    let view = template::column()
        .gap(6.0)
        .child(template::Text::new("shared"))
        .child(template::Button::new("continue"));
    let mut app = App::mount_template_with_size_scaled(view, 320, 180, 1.0);
    app.frame();

    let root = app.scene().root().unwrap();
    let node = app.scene().node(root).unwrap();
    assert_eq!(node.kind, WidgetKind::Column);
    assert_eq!(node.children.len(), 2);
    assert_eq!(
        app.scene().node(node.children[1]).unwrap().kind,
        WidgetKind::Button
    );
}

#[test]
fn shared_scenario_action_drives_the_retained_accesskit_path() {
    use std::cell::Cell;
    use std::rc::Rc;

    let clicks = Rc::new(Cell::new(0));
    let sink = clicks.clone();
    let mut app = App::mount_template(template::Button::new("continue").on_click(move || {
        sink.set(sink.get() + 1);
    }));

    assert!(app.drive_action(&template::DriveAction::click(Role::Button, "continue")));
    assert_eq!(clicks.get(), 1);
}

#[test]
fn mounted_apps_keep_independent_widget_runtimes_on_one_thread() {
    use std::cell::Cell;
    use std::rc::Rc;

    let first_clicks = Rc::new(Cell::new(0));
    let first_sink = first_clicks.clone();
    let mut first = App::mount(widgets::Button::new("first").on_click(move || {
        first_sink.set(first_sink.get() + 1);
    }));

    let second_clicks = Rc::new(Cell::new(0));
    let second_sink = second_clicks.clone();
    let mut second = App::mount(widgets::Button::new("second").on_click(move || {
        second_sink.set(second_sink.get() + 1);
    }));

    assert!(first.drive_action(&DriveAction::click(Role::Button, "first")));
    assert!(second.drive_action(&DriveAction::click(Role::Button, "second")));
    assert!(first.drive_action(&DriveAction::click(Role::Button, "first")));
    assert_eq!(first_clicks.get(), 2);
    assert_eq!(second_clicks.get(), 1);
}

#[test]
fn contextual_mount_passes_explicit_inline_scope_to_child() {
    #[derive(Clone)]
    struct Label(&'static str);

    fn child(context: &Context) -> widgets::Text {
        widgets::Text::new(context.require::<Label>().0)
    }

    let root = Context::new().with(Label("root"));
    let mut app = App::mount_with_context(root, |context| {
        let inline = context.with(Label("child"));
        child(&inline)
    });
    app.frame();

    let root = app.scene().root().expect("contextual view has a root");
    assert_eq!(
        app.scene().a11y(root).unwrap().name.as_deref(),
        Some("child")
    );
    assert_eq!(app.context().require::<Label>().0, "root");
}

fn glyph_rich_view() -> impl schnellui::widgets::View {
    use schnellui::widgets::{Column, Text};
    Column::new()
        .child(Text::new("Theme transition glyph cache"))
        .child(Text::new(
            "The quick brown fox jumps over the lazy dog 0123456789",
        ))
}

#[test]
fn theme_transition_frames_reuse_the_atlas_and_glyph_cache() {
    let mode = ThemeMode::system(theme::LIGHT, theme::DARK);
    let mut app = App::mount_themed(mode, glyph_rich_view);
    let duration = Duration::from_secs(60);
    app.set_theme_transition_duration(duration);
    let atlas_id = app.atlas.id();
    let rasterized = app.text.rasterized_glyph_count();
    assert!(rasterized > 0, "the first mount rasterizes its glyphs");

    assert!(app.apply_color_scheme(ColorScheme::Dark));
    assert!(app.theme_transition_active());
    let mut previous = app.active_theme();
    for frame in 1..=10u32 {
        // Pretend `frame / 10` of the transition has elapsed.
        let started = Instant::now() - duration * frame / 10;
        app.theme_transition.as_mut().unwrap().started = started;
        assert!(app.advance_theme_transition(), "frame {frame} rebuilds");
        assert_ne!(
            app.active_theme(),
            previous,
            "frame {frame} changes the theme"
        );
        previous = app.active_theme();
        assert_eq!(app.atlas.id(), atlas_id, "frame {frame} keeps the atlas");
        assert_eq!(
            app.text.rasterized_glyph_count(),
            rasterized,
            "frame {frame} re-rasterized an already cached glyph"
        );
    }
}

#[test]
fn hook_remount_through_with_remount_engine_keeps_atlas_and_cached_glyphs() {
    use schnellui::widgets::Text;

    let mut running = App::mount(glyph_rich_view());
    let atlas_id = running.atlas.id();
    let rasterized = running.text.rasterized_glyph_count();
    assert!(rasterized > 0);

    // The hook uses only public constructors, exactly as an application would;
    // `with_engine_donor` is the host's poll_remount wrapper around the hook call.
    let hook =
        || -> Option<App> { Some(App::with_remount_engine(|| App::mount(glyph_rich_view()))) };
    let (next, claimed) = crate::app_new::with_engine_donor(&mut running, hook);
    let next = next.expect("hook remounts");
    assert!(claimed);
    assert_eq!(next.atlas.id(), atlas_id, "remount keeps the atlas");
    assert_eq!(
        next.text.rasterized_glyph_count(),
        rasterized,
        "no glyph re-rasterized"
    );

    // A tree that adds one glyph rasterizes only that glyph.
    let mut next = next;
    let hook = || {
        Some(App::with_remount_engine(|| {
            App::mount(Text::new("The quick brown fox Q"))
        }))
    };
    let (third, _) = crate::app_new::with_engine_donor(&mut next, hook);
    let third = third.unwrap();
    assert_eq!(third.atlas.id(), atlas_id);
    assert_eq!(third.text.rasterized_glyph_count(), rasterized + 1);

    // Without the wrapper, a hook-built app keeps its own fresh engine.
    let plain = || Some(App::mount(Text::new("plain")));
    let (fresh, claimed) = crate::app_new::with_engine_donor(&mut next, plain);
    assert!(!claimed);
    assert_ne!(fresh.unwrap().atlas.id(), atlas_id);

    // Outside the host's hook call the wrapper is a plain constructor.
    let alone = App::with_remount_engine(|| App::mount(Text::new("alone")));
    assert_ne!(alone.atlas.id(), atlas_id);
}

#[test]
fn compat_hook_shape_keeps_atlas_without_calling_with_remount_engine() {
    let mut running = App::mount(glyph_rich_view());
    let atlas_id = running.atlas.id();
    let rasterized = running.text.rasterized_glyph_count();

    // An example-shaped hook: plain `App::mount`, no opt-in. The
    // `run_windowed_with*` wrappers open the claim around the hook call.
    let hook = || -> Option<App> { Some(App::mount(glyph_rich_view())) };
    let (next, claimed) = crate::app_new::with_engine_donor(&mut running, || {
        crate::app_new::claim_remount_engine(hook)
    });
    let next = next.unwrap();
    assert!(claimed);
    assert_eq!(next.atlas.id(), atlas_id);
    assert_eq!(next.text.rasterized_glyph_count(), rasterized);

    // A hook that returns None builds nothing and leaves the claim closed.
    let idle = || -> Option<App> { None };
    let (none, claimed) = crate::app_new::with_engine_donor(&mut running, || {
        crate::app_new::claim_remount_engine(idle)
    });
    assert!(none.is_none() && !claimed);
    let later = App::mount(glyph_rich_view());
    assert_ne!(later.atlas.id(), atlas_id, "claim closed after the hook");
}

#[test]
fn unused_claimant_leaves_running_engine_intact() {
    let mut running = App::mount(glyph_rich_view());
    let atlas_id = running.atlas.id();
    let rasterized = running.text.rasterized_glyph_count();
    let pixels = running.atlas.pixels().to_vec();

    // A compat-shaped hook that builds an app and then decides nothing changed.
    let hook = || -> Option<App> {
        let _prebuilt = App::mount(glyph_rich_view());
        None
    };
    let (none, claimed) = crate::app_new::with_engine_donor(&mut running, || {
        crate::app_new::claim_remount_engine(hook)
    });
    assert!(none.is_none() && claimed);
    assert_eq!(running.atlas.id(), atlas_id);
    assert_eq!(running.text.rasterized_glyph_count(), rasterized);
    assert_eq!(running.atlas.pixels(), &pixels[..]);
}

#[test]
fn full_atlas_is_not_carried_into_a_remount() {
    let mut running = App::mount(glyph_rich_view());
    let atlas_id = running.atlas.id();
    // Fill past the headroom threshold.
    while running.atlas.used_fraction() <= 0.5 {
        running.atlas.allocate(1024, 64).unwrap();
    }
    assert!(!running.atlas.has_headroom());

    let hook = || Some(App::mount(glyph_rich_view()));
    let (next, claimed) = crate::app_new::with_engine_donor(&mut running, || {
        crate::app_new::claim_remount_engine(hook)
    });
    let next = next.unwrap();
    assert!(!claimed);
    assert_ne!(next.atlas.id(), atlas_id, "full atlas starts over");
    assert!(
        next.text.rasterized_glyph_count() > 0,
        "fresh cache re-rasterizes"
    );

    let mut donor = App::mount(glyph_rich_view());
    while donor.atlas.used_fraction() <= 0.5 {
        donor.atlas.allocate(1024, 64).unwrap();
    }
    let mut fresh = App::new(100, 100);
    let before = fresh.atlas.id();
    assert!(!fresh.adopt_text_engine(&mut donor));
    assert_eq!(fresh.atlas.id(), before);
}
