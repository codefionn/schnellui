use super::event_handler::raw_capture_target_survives;
use super::host::coalesce_subtree_replacements;
use super::{
    control_letter_from_text, hit_path_json, is_control_key, modifier_text_implies_control,
    pace_animation_redraw, rebase_redraw_after_frame, resolve_control_letter,
    special_key_from_physical,
};
use crate::{
    a11y::Role,
    scene::{ComponentRef, Point},
    widgets::Button,
    App, FocusedInputResult, SubtreeReplacement, UiKey,
};
use winit::keyboard::{Key, KeyCode, NamedKey, NativeKeyCode, PhysicalKey};

#[test]
fn animation_redraws_are_immediate_then_paced_and_reset_when_idle() {
    let start = std::time::Instant::now();
    let mut deadline = None;

    let (redraw, wake) = pace_animation_redraw(true, start, &mut deadline);
    assert!(redraw);
    let wake = wake.expect("active animation deadline");
    assert!(wake > start);

    let (redraw, same_wake) = pace_animation_redraw(true, start, &mut deadline);
    assert!(!redraw);
    assert_eq!(same_wake, Some(wake));

    let (redraw, next_wake) = pace_animation_redraw(true, wake, &mut deadline);
    assert!(redraw);
    assert!(next_wake.is_some_and(|next| next > wake));

    assert_eq!(
        pace_animation_redraw(false, wake, &mut deadline),
        (false, None)
    );
    assert_eq!(deadline, None);
}

#[test]
fn expensive_frames_rebase_periodic_deadlines_from_completion() {
    let requested_at = std::time::Instant::now();
    let interval = std::time::Duration::from_secs(2);
    let mut deadline = Some(requested_at + interval);
    let finished_at = requested_at + interval + std::time::Duration::from_secs(3);

    rebase_redraw_after_frame(&mut deadline, Some(interval), finished_at);
    assert_eq!(deadline, Some(finished_at + interval));

    rebase_redraw_after_frame(&mut deadline, None, finished_at);
    assert_eq!(deadline, None);
}

#[test]
fn modifier_aware_c0_text_recovers_control_letters() {
    assert_eq!(control_letter_from_text(Some("\u{17}")), Some('w'));
    assert_eq!(control_letter_from_text(Some("\u{4}")), Some('d'));
    assert_eq!(control_letter_from_text(Some("w")), None);
    assert_eq!(control_letter_from_text(Some("")), None);

    assert_eq!(resolve_control_letter(Some("\r"), None, false), None);
    assert_eq!(resolve_control_letter(Some("\u{8}"), None, false), None);
    assert_eq!(resolve_control_letter(Some("\t"), None, false), None);
    assert_eq!(
        resolve_control_letter(Some("\u{17}"), None, true),
        Some('w')
    );
    assert_eq!(resolve_control_letter(None, Some("d"), true), Some('d'));
    assert_eq!(resolve_control_letter(None, Some("\u{4}"), true), Some('d'));
}

#[test]
fn interaction_hit_path_carries_semantics_and_geometry() {
    let mut app = App::mount_with_size(Button::new("Save"), 200, 80);
    app.frame();
    let root = app.scene().root().expect("button root");
    let rect = app.scene().layout(root).expect("button layout").rect;
    let path = hit_path_json(
        &app,
        Point {
            x: rect.x + rect.width / 2.0,
            y: rect.y + rect.height / 2.0,
        },
    );

    assert_eq!(path[0]["role"], "button");
    assert_eq!(path[0]["name"], "Save");
    assert!(path[0]["actions"]
        .as_array()
        .is_some_and(|actions| !actions.is_empty()));
    assert!(path[0]["rect"]["width"].as_f64().unwrap() > 0.0);
}

#[test]
fn raw_capture_continues_only_on_the_same_remounted_surface() {
    fn raw_surface(label: &'static str, registered: bool) -> App {
        let mut app = App::mount_with_size(Button::new(label), 200, 80);
        let target = app
            .find_widget(Role::Button, Some(label))
            .expect("raw surface");
        app.focus(Some(target));
        if registered {
            app.register_focused_input_handler(Role::Button, Some(label), |_| {
                FocusedInputResult::Handled
            });
        }
        app
    }

    let previous = raw_surface("Browser viewport tab", true);
    assert!(raw_capture_target_survives(
        &previous,
        &raw_surface("Browser viewport tab", true)
    ));
    assert!(!raw_capture_target_survives(
        &previous,
        &raw_surface("Browser viewport other", true)
    ));
    assert!(!raw_capture_target_survives(
        &previous,
        &raw_surface("Browser viewport tab", false)
    ));
}

#[test]
fn controller_wake_is_acknowledged_before_structural_polling() {
    let signal = super::RedrawSignal::default();
    signal
        .0
        .pending
        .store(true, std::sync::atomic::Ordering::Release);
    let signal_during_poll = signal.clone();
    let mut polls = 0;

    // This models a worker request arriving while the host drains the update
    // associated with the first wake. It must be able to claim a fresh wake;
    // waiting for a compositor redraw here strands it behind an occluded window.
    signal.drain_controller_wake(|| {
        polls += 1;
        assert!(!signal_during_poll
            .0
            .pending
            .load(std::sync::atomic::Ordering::Acquire));
        signal_during_poll
            .0
            .pending
            .store(true, std::sync::atomic::Ordering::Release);
    });

    assert_eq!(polls, 1);
    assert!(signal.0.pending.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn worker_wakes_survive_a_full_remount() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let previous = App::mount_with_size(Button::new("Before"), 200, 80);
    previous.redraw_signal.install_headless(sender);
    // A worker thread keeps the handle it got before the remount.
    let worker = previous.redraw_signal();
    let mut replacement = App::mount_with_size(Button::new("After"), 200, 80);
    super::host::carry_redraw_signal(&previous, &mut replacement);
    drop(previous);

    worker.request_redraw();
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::PlatformEvent::RedrawRequested)
    ));
    // The host drains the running app's signal; the worker's next wake must
    // get through instead of being coalesced behind a flag nobody clears.
    replacement.redraw_signal().drain_controller_wake(|| {});
    worker.request_redraw();
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::PlatformEvent::RedrawRequested)
    ));
}

#[test]
fn headless_wake_coalesces_then_accepts_another_controller_update() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let signal = super::RedrawSignal::default();
    signal.install_headless(sender);
    signal.request_redraw();
    signal.request_redraw();
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::PlatformEvent::RedrawRequested)
    ));
    assert!(receiver.try_recv().is_err());

    signal.drain_controller_wake(|| signal.request_redraw());
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::PlatformEvent::RedrawRequested)
    ));
}

#[test]
fn debug_server_keys_undo_and_redo_typed_text() {
    use crate::debug_server::DebugKey;

    let mut app = App::mount(crate::widgets::TextInput::new(""));
    app.frame();
    let input = app.find_widget(Role::TextInput, None).unwrap();
    app.focus(Some(input));
    let mut host = windowed_host(app, None);
    let key = |name: &str, text: Option<&str>| DebugKey {
        key: name.into(),
        shift: false,
        ctrl: false,
        text: text.map(str::to_owned),
    };
    let value = |host: &super::WindowedApp| host.app.scene().a11y(input).unwrap().value.clone();
    host.handle_debug_key(key("text", Some("a")));
    host.handle_debug_key(key("text", Some("b")));
    assert_eq!(value(&host).as_deref(), Some("ab"));
    host.handle_debug_key(key("undo", None));
    assert_eq!(value(&host).as_deref(), Some(""));
    host.handle_debug_key(key("redo", None));
    assert_eq!(value(&host).as_deref(), Some("ab"));
}

#[test]
fn debug_server_text_key_types_each_character_through_shortcuts() {
    use crate::debug_server::DebugKey;
    use std::{cell::RefCell, rc::Rc};

    let mut app = App::mount(crate::widgets::TextInput::new(""));
    let pressed = Rc::new(RefCell::new(String::new()));
    for c in ['1', '2'] {
        let pressed = Rc::clone(&pressed);
        app.register_shortcut(crate::Shortcut::new(c, false, false, false), move || {
            pressed.borrow_mut().push(c)
        });
    }
    app.frame();
    let mut host = windowed_host(app, None);
    host.handle_debug_key(DebugKey {
        key: "text".into(),
        shift: false,
        ctrl: false,
        text: Some("12".into()),
    });
    assert_eq!(*pressed.borrow(), "12");
}

#[test]
fn headless_clipboard_stays_in_memory() {
    let mut clipboard = super::HostClipboard::Memory(None);
    assert!(clipboard.read_text().is_err());
    clipboard.write_text("copied".into()).unwrap();
    assert_eq!(clipboard.read_text().unwrap(), "copied");
}

#[test]
fn subtree_replacements_coalesce_duplicate_targets_with_latest_payload() {
    let first = ComponentRef::new();
    let second = ComponentRef::new();
    let replacements = coalesce_subtree_replacements(vec![
        SubtreeReplacement::new(first, Button::new("old first"), "old_first"),
        SubtreeReplacement::new(second, Button::new("second"), "second"),
        SubtreeReplacement::new(first, Button::new("new first"), "new_first"),
    ]);

    assert_eq!(replacements.len(), 2);
    assert_eq!(replacements[0].target, first);
    assert_eq!(replacements[0].reason, "new_first");
    assert_eq!(replacements[1].target, second);
    assert_eq!(replacements[1].reason, "second");
}

#[test]
fn physical_keys_recover_terminal_editing_keys() {
    assert_eq!(
        special_key_from_physical(PhysicalKey::Code(KeyCode::Backspace), false, false),
        Some(UiKey::Backspace)
    );
    assert_eq!(
        special_key_from_physical(PhysicalKey::Code(KeyCode::Delete), false, false),
        Some(UiKey::Delete)
    );
    assert_eq!(
        special_key_from_physical(PhysicalKey::Code(KeyCode::ArrowLeft), true, true),
        Some(UiKey::Left {
            shift: true,
            ctrl: true,
        })
    );
}

#[test]
fn logical_control_recovers_remapped_or_unidentified_control_keys() {
    assert!(is_control_key(
        &Key::Named(NamedKey::Control),
        PhysicalKey::Code(KeyCode::CapsLock)
    ));
    assert!(is_control_key(
        &Key::Named(NamedKey::Control),
        PhysicalKey::Unidentified(NativeKeyCode::Unidentified)
    ));
    assert!(!is_control_key(
        &Key::Character("w".into()),
        PhysicalKey::Code(KeyCode::KeyW)
    ));

    assert!(modifier_text_implies_control(
        Some("\u{17}"),
        PhysicalKey::Code(KeyCode::KeyW)
    ));
    assert!(!modifier_text_implies_control(
        Some("\r"),
        PhysicalKey::Code(KeyCode::Enter)
    ));
    assert!(!modifier_text_implies_control(
        Some("\u{8}"),
        PhysicalKey::Code(KeyCode::Backspace)
    ));
}

/// A dynamic label growing from "9" to "10" reflows its right neighbour. The host
/// must answer with an incremental update naming the changed text and the moved
/// sibling, not a rebuilt tree; a quiet frame after it sends nothing.
#[test]
fn width_change_publishes_an_incremental_tree_update() {
    use super::host::{plan_accessibility_update, AccessibilityPlan, AccessibilityPublished};
    use crate::widgets::{Column, Row, Text};

    let count = crate::signal::create_signal(9_i32);
    let mut col = Column::new().child(
        Row::new()
            .child(Text::dynamic(move || count.get().to_string()))
            .child(Text::new("tail")),
    );
    for i in 0..20 {
        col = col.child(Text::new(format!("unrelated {i}")));
    }
    let mut app = App::mount_with_size(col, 400, 400);
    app.frame();
    let published = AccessibilityPublished::of(app.scene());
    assert_eq!(
        plan_accessibility_update(app.scene(), false, &published),
        AccessibilityPlan::Skip,
        "a settled frame with unchanged focus sends nothing"
    );

    let text = app
        .find_widget(Role::Label, Some("9"))
        .expect("dynamic label");
    let tail = app
        .find_widget(Role::Label, Some("tail"))
        .expect("tail label");
    assert!(
        app.scene().a11y_scope_cache().is_some(),
        "plan filled the cache"
    );
    count.set(10);
    assert!(app.settle_frame(), "the wider label relayouts");
    assert!(
        app.scene().a11y_scope_cache().is_some(),
        "a text change keeps the cached modal root and focus"
    );
    assert_eq!(
        plan_accessibility_update(app.scene(), false, &published),
        AccessibilityPlan::Incremental
    );
    let update = schnellui::a11y::build_incremental_window_tree_update(app.scene(), 1.0, "t");
    let ids: Vec<_> = update.nodes.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&schnellui::a11y::to_access_id(text)));
    assert!(
        ids.contains(&schnellui::a11y::to_access_id(tail)),
        "the neighbour whose bounds moved is re-sent"
    );
    let full = schnellui::a11y::build_full_window_tree_update(app.scene(), 1.0, "t");
    assert!(update.tree.is_none());
    assert!(
        update.nodes.len() * 3 < full.nodes.len(),
        "{} of {} nodes",
        update.nodes.len(),
        full.nodes.len()
    );

    // Structure, a pending remount and a changed modal scope still go full.
    assert_eq!(
        plan_accessibility_update(app.scene(), true, &published),
        AccessibilityPlan::Full
    );
    let stale = AccessibilityPublished::default();
    assert_eq!(
        plan_accessibility_update(app.scene(), false, &stale),
        AccessibilityPlan::Full
    );
}

/// A window host with no window, renderer or accessibility adapter around
/// `app`, driven by calling its event handlers directly.
fn windowed_host(
    app: App,
    remount: Option<Box<dyn FnMut(crate::scene::Size) -> Option<crate::WindowUpdate>>>,
) -> super::WindowedApp {
    use super::{HostClipboard, WindowedApp};
    use std::collections::BTreeMap;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::keyboard::ModifiersState;

    let glyph_atlas_id = app.atlas.id();
    WindowedApp {
        app,
        clipboard: HostClipboard::Memory(None),
        headless: true,
        title: "test".into(),
        window: None,
        renderer: None,
        accessibility: None,
        event_loop_proxy: None,
        accessibility_full_update_pending: true,
        accessibility_published: Default::default(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        cursor_inside: false,
        cursor_icon: Default::default(),
        cursor_synced_key: None,
        ime_area: None,
        glyph_atlas_id,
        interaction_trace: None,
        _debug_server: None,
        remount_trigger: "startup",
        remount_count: 0,
        subtree_replacement_count: 0,
        remount_counts_by_reason: BTreeMap::new(),
        last_remount: None,
        init_phys: PhysicalSize::new(400, 500),
        deadline: None,
        interval_deadline: None,
        animation_deadline: None,
        modifiers: ModifiersState::default(),
        control_pressed: false,
        raw_pointer_capture: Vec::new(),
        drag_text: None,
        last_text_click: None,
        drag_slider: None,
        backdrop_press: None,
        left_pointer_down: false,
        remount,
    }
}

/// A host-controlled dropdown in a dialog, driven through the native pointer
/// path: press, release, then the remount poll `window_event` runs after each.
mod dropdown_pointer {
    use std::cell::Cell;
    use std::rc::Rc;

    use winit::dpi::PhysicalPosition;
    use winit::event::{ElementState, MouseButton};

    use super::super::event_handler::EventTail;
    use super::super::WindowedApp;
    use crate::a11y::{Role, StateFlags};
    use crate::scene::{ComponentRef, Size};
    use crate::widgets::{Button, Column, Dialog, Dropdown, DropdownOption, Stack, View};
    use crate::{App, Remount, SubtreeReplacement, WindowUpdate};

    #[derive(Clone, Copy)]
    struct State {
        open: bool,
        chosen: usize,
        dirty: bool,
    }

    /// How the host applies a state change.
    #[derive(Clone, Copy)]
    enum Update {
        Remount,
        Subtree,
    }

    fn dropdown(state: &Rc<Cell<State>>) -> Column {
        let current = state.get();
        let toggle = state.clone();
        let mut dropdown = Dropdown::new("Fruit")
            .open(current.open)
            .on_toggle(move || {
                let s = toggle.get();
                toggle.set(State {
                    open: !s.open,
                    dirty: true,
                    ..s
                });
            });
        for (index, label) in ["Apple", "Banana", "Cherry"].into_iter().enumerate() {
            let pick = state.clone();
            dropdown = dropdown.option(
                DropdownOption::new(label)
                    .selected(index == current.chosen)
                    .on_select(move || {
                        pick.set(State {
                            open: false,
                            chosen: index,
                            dirty: true,
                        })
                    }),
            );
        }
        Column::new().child(dropdown)
    }

    fn mount(state: &Rc<Cell<State>>, slot: ComponentRef) -> App {
        let content = Column::new()
            .gap(200.0)
            .child(dropdown(state).with_ref(slot))
            .child(Button::new("Elsewhere"));
        let mut app = App::mount_with_size(
            Stack::new()
                .fill()
                .child(Dialog::new("Settings").size(300.0, 380.0).child(content)),
            400,
            500,
        );
        app.frame();
        app
    }

    fn host(update: Update) -> (Rc<Cell<State>>, WindowedApp) {
        let state = Rc::new(Cell::new(State {
            open: true,
            chosen: 0,
            dirty: false,
        }));
        let slot = ComponentRef::new();
        let app = mount(&state, slot);
        let hook_state = state.clone();
        let remount: Box<dyn FnMut(Size) -> Option<WindowUpdate>> = Box::new(move |_| {
            let s = hook_state.get();
            if !s.dirty {
                return None;
            }
            hook_state.set(State { dirty: false, ..s });
            Some(match update {
                Update::Remount => {
                    WindowUpdate::Remount(Remount::new(mount(&hook_state, slot), "test"))
                }
                Update::Subtree => WindowUpdate::Subtrees(vec![SubtreeReplacement::new(
                    slot,
                    dropdown(&hook_state),
                    "test",
                )]),
            })
        });
        let host = super::windowed_host(app, Some(remount));
        assert!(expanded(&host));
        (state, host)
    }

    fn click(host: &mut WindowedApp, x: f32, y: f32) {
        press(host, x, y, MouseButton::Left);
    }

    fn press(host: &mut WindowedApp, x: f32, y: f32, button: MouseButton) {
        let scale = f64::from(host.app.scale());
        host.on_cursor_moved(PhysicalPosition::new(
            f64::from(x) * scale,
            f64::from(y) * scale,
        ));
        for state in [ElementState::Pressed, ElementState::Released] {
            if host.on_mouse_input(state, button) == EventTail::Normal {
                host.poll_remount();
            }
            host.on_redraw_requested();
        }
    }

    fn click_widget(host: &mut WindowedApp, role: Role, name: &str) {
        let id = host.app.find_widget(role, Some(name)).unwrap();
        let rect = host.app.scene().layout(id).unwrap().rect;
        click(host, rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
    }

    fn expanded(host: &WindowedApp) -> bool {
        let trigger = host.app.find_widget(Role::ComboBox, Some("Fruit")).unwrap();
        StateFlags(host.app.scene().a11y(trigger).unwrap().state).contains(StateFlags::EXPANDED)
    }

    const UPDATES: [Update; 2] = [Update::Remount, Update::Subtree];

    #[test]
    fn clicking_the_selected_option_closes_the_dropdown() {
        for update in UPDATES {
            let (state, mut host) = host(update);
            click_widget(&mut host, Role::ListBoxOption, "Apple");
            assert!(!state.get().open);
            assert!(!expanded(&host));
        }
    }

    #[test]
    fn clicking_another_option_selects_it_and_closes_the_dropdown() {
        for update in UPDATES {
            let (state, mut host) = host(update);
            click_widget(&mut host, Role::ListBoxOption, "Banana");
            assert_eq!(state.get().chosen, 1);
            assert!(!expanded(&host));
        }
    }

    #[test]
    fn clicking_another_widget_closes_the_dropdown() {
        for update in UPDATES {
            let (state, mut host) = host(update);
            click_widget(&mut host, Role::Button, "Elsewhere");
            assert!(!state.get().open);
            assert!(!expanded(&host));
        }
    }

    #[test]
    fn the_back_button_steps_back_without_clicking() {
        let (state, mut host) = host(Update::Remount);
        let steps = Rc::new(Cell::new(Vec::new()));
        let sink = steps.clone();
        host.app.register_navigation_handler(move |navigation| {
            let mut seen = sink.take();
            seen.push(navigation);
            sink.set(seen);
            true
        });
        let banana = host
            .app
            .find_widget(Role::ListBoxOption, Some("Banana"))
            .unwrap();
        let rect = host.app.scene().layout(banana).unwrap().rect;
        let (x, y) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        press(&mut host, x, y, MouseButton::Back);
        press(&mut host, x, y, MouseButton::Forward);
        assert_eq!(
            steps.take(),
            [crate::Navigation::Back, crate::Navigation::Forward]
        );
        assert_eq!(state.get().chosen, 0);
        assert!(expanded(&host));
    }

    #[test]
    fn clicking_blank_space_closes_the_dropdown() {
        for update in UPDATES {
            let (state, mut host) = host(update);
            click(&mut host, 395.0, 495.0);
            assert!(!state.get().open);
            assert!(!expanded(&host));
        }
    }
}

/// The platform switches light/dark while the pointer rests on a button, and
/// a click arrives before the next redraw. The rebuilt tree must already be
/// laid out: the click lands and the button keeps its hover treatment, as
/// after a structural remount.
mod platform_theme_change {
    use std::cell::Cell;
    use std::rc::Rc;

    use winit::dpi::PhysicalPosition;
    use winit::event::{ElementState, MouseButton};
    use winit::window::Theme as WinitTheme;

    use super::windowed_host;
    use crate::a11y::Role;
    use crate::widgets::{Button, Pad};
    use crate::{theme, App, ThemeMode};

    #[test]
    fn a_click_before_the_next_redraw_lands_on_the_rebuilt_tree() {
        let clicks = Rc::new(Cell::new(0));
        let counter = clicks.clone();
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::system(theme::LIGHT, theme::DARK),
            move || {
                let counter = counter.clone();
                Pad::all(20.0)
                    .child(Button::new("Save").on_click(move || counter.set(counter.get() + 1)))
            },
            300,
            200,
            1.0,
        );
        app.frame();
        let mut host = windowed_host(app, None);
        let save = host.app.find_widget(Role::Button, Some("Save")).unwrap();
        let r = host.app.scene().layout(save).unwrap().rect;
        host.on_cursor_moved(PhysicalPosition::new(
            f64::from(r.x + r.width / 2.0),
            f64::from(r.y + r.height / 2.0),
        ));
        host.on_redraw_requested();
        let hovered_prims = host.app.scene().paint(save).unwrap().primitives.len();

        host.on_theme_changed(WinitTheme::Dark);
        assert_eq!(host.app.active_theme(), theme::DARK);
        let save = host.app.find_widget(Role::Button, Some("Save")).unwrap();
        assert_eq!(
            host.app.scene().paint(save).unwrap().primitives.len(),
            hovered_prims,
            "the button under the resting pointer keeps its hover wash"
        );
        host.on_mouse_input(ElementState::Pressed, MouseButton::Left);
        host.on_mouse_input(ElementState::Released, MouseButton::Left);
        assert_eq!(
            clicks.get(),
            1,
            "the click before the redraw reached the button"
        );
    }

    /// The same through an animated change: every fade frame rebuilds the
    /// tree, and none of them may drop the hover under a resting pointer.
    #[test]
    fn a_fade_keeps_the_hover_under_a_resting_pointer() {
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::system(theme::LIGHT, theme::DARK),
            || Pad::all(20.0).child(Button::new("Save")),
            300,
            200,
            1.0,
        );
        app.set_theme_transition_duration(std::time::Duration::from_millis(100));
        let start = std::time::Instant::now();
        app.set_clock(Some(start));
        app.frame();
        let mut host = windowed_host(app, None);
        let save = host.app.find_widget(Role::Button, Some("Save")).unwrap();
        let r = host.app.scene().layout(save).unwrap().rect;
        host.on_cursor_moved(PhysicalPosition::new(
            f64::from(r.x + r.width / 2.0),
            f64::from(r.y + r.height / 2.0),
        ));
        host.on_redraw_requested();
        let hovered_prims = host.app.scene().paint(save).unwrap().primitives.len();

        host.on_theme_changed(WinitTheme::Dark);
        for ms in [30, 60, 120] {
            host.app
                .set_clock(Some(start + std::time::Duration::from_millis(ms)));
            host.on_redraw_requested();
            let save = host.app.find_widget(Role::Button, Some("Save")).unwrap();
            assert_eq!(
                host.app.scene().paint(save).unwrap().primitives.len(),
                hovered_prims,
                "{ms} ms into the fade the button keeps its hover wash"
            );
        }
        assert_eq!(host.app.active_theme(), theme::DARK);
    }

    /// The pointer rests on a button, then leaves the window. Neither a theme
    /// rebuild nor a structural remount may hover the button again from the
    /// last position the pointer had inside.
    #[test]
    fn a_rebuild_after_the_pointer_left_hovers_nothing() {
        let flagged = Rc::new(Cell::new(false));
        let mount = || {
            App::mount_themed_with_size_scaled(
                ThemeMode::system(theme::LIGHT, theme::DARK),
                || Pad::all(20.0).child(Button::new("Save")),
                300,
                200,
                1.0,
            )
        };
        let mut app = mount();
        app.frame();
        let hook_flag = flagged.clone();
        let remount: Box<dyn FnMut(crate::scene::Size) -> Option<crate::WindowUpdate>> =
            Box::new(move |_| {
                if !hook_flag.replace(false) {
                    return None;
                }
                let mut app = mount();
                app.frame();
                Some(crate::WindowUpdate::Remount(crate::Remount::new(
                    app, "test",
                )))
            });
        let mut host = windowed_host(app, Some(remount));
        let save = host.app.find_widget(Role::Button, Some("Save")).unwrap();
        let resting = host.app.scene().paint(save).unwrap().primitives.len();
        let r = host.app.scene().layout(save).unwrap().rect;
        host.on_cursor_moved(PhysicalPosition::new(
            f64::from(r.x + r.width / 2.0),
            f64::from(r.y + r.height / 2.0),
        ));
        host.on_redraw_requested();
        host.on_cursor_left();
        host.on_redraw_requested();
        let prims = |host: &super::super::WindowedApp| {
            let save = host.app.find_widget(Role::Button, Some("Save")).unwrap();
            host.app.scene().paint(save).unwrap().primitives.len()
        };
        assert_eq!(prims(&host), resting);

        host.on_theme_changed(WinitTheme::Dark);
        assert_eq!(prims(&host), resting, "a theme rebuild hovered the button");
        flagged.set(true);
        host.poll_remount();
        assert_eq!(prims(&host), resting, "a remount hovered the button");
    }

    /// Press a button that has a tooltip, drag out of the window and let go
    /// there. The button paints as it did before the pointer came: no hover
    /// wash and no tooltip for a pointer that is gone.
    #[test]
    fn a_release_outside_the_window_shows_no_tooltip() {
        let mut app = App::mount_with_size(
            Pad::all(20.0).child(Button::new("Save").tooltip("Save the draft")),
            300,
            200,
        );
        app.frame();
        let mut host = windowed_host(app, None);
        let save = host.app.find_widget(Role::Button, Some("Save")).unwrap();
        let resting = host.app.scene().paint(save).unwrap().primitives.clone();
        let r = host.app.scene().layout(save).unwrap().rect;
        host.on_cursor_moved(PhysicalPosition::new(
            f64::from(r.x + r.width / 2.0),
            f64::from(r.y + r.height / 2.0),
        ));
        host.on_redraw_requested();
        host.on_mouse_input(ElementState::Pressed, MouseButton::Left);
        host.on_redraw_requested();
        host.on_cursor_left();
        host.on_mouse_input(ElementState::Released, MouseButton::Left);
        host.on_redraw_requested();
        assert_eq!(
            host.app.scene().paint(save).unwrap().primitives,
            resting,
            "the button paints as if never hovered"
        );
    }
}

/// A wheel scrolls content under a pointer that stays where it is, so a
/// different control is under it afterwards. Hover follows the pointer, as it
/// does in a browser, without waiting for the pointer to move.
mod wheel_under_a_resting_pointer {
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseScrollDelta;

    use super::windowed_host;
    use crate::a11y::Role;
    use crate::scene::Point;
    use crate::widgets::probes::hover_disagrees;
    use crate::widgets::{List, ListItem, Scroll};
    use crate::{theme, App, ThemeMode};

    fn rows() -> super::super::WindowedApp {
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme::LIGHT),
            || {
                let mut list = List::new();
                for i in 0..20 {
                    list = list.child(ListItem::new(format!("Row {i}")));
                }
                Scroll::new().size(240.0, 100.0).child(list)
            },
            300,
            200,
            1.0,
        );
        app.frame();
        windowed_host(app, None)
    }

    fn pointer_on_row(host: &mut super::super::WindowedApp, name: &str) -> Point {
        let row = host.app.find_widget(Role::ListItem, Some(name)).unwrap();
        let r = host.app.scene().layout(row).unwrap().rect;
        host.on_cursor_moved(PhysicalPosition::new(
            f64::from(r.x + r.width / 2.0),
            f64::from(r.y + r.height / 2.0),
        ));
        host.on_redraw_requested();
        Point {
            x: r.x + r.width / 2.0,
            y: r.y + r.height / 2.0,
        }
    }

    fn disagreement(host: &super::super::WindowedApp, pointer: Point) -> Option<String> {
        hover_disagrees(&host.app.widgets, host.app.scene(), pointer, false)
    }

    #[test]
    fn hover_moves_to_the_row_the_wheel_brought_under_it() {
        let mut host = rows();
        let pointer = pointer_on_row(&mut host, "Row 1");
        assert_eq!(disagreement(&host, pointer), None, "hover starts on row 1");

        host.on_mouse_wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
            0.0, -48.0,
        )));
        assert_eq!(disagreement(&host, pointer), None, "right after the wheel");
        host.on_redraw_requested();
        assert_eq!(disagreement(&host, pointer), None, "after the frame");
    }

    #[test]
    fn a_wheel_that_moves_nothing_leaves_hover_alone() {
        let mut host = rows();
        let pointer = pointer_on_row(&mut host, "Row 1");
        // Already at the top: scrolling up changes nothing.
        host.on_mouse_wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
            0.0, 48.0,
        )));
        assert_eq!(disagreement(&host, pointer), None);
    }
}

/// What a virtual list does is a function of its input, not of the seed of
/// the hash map that holds its mounted rows.
mod virtual_list_determinism {
    use super::super::headless::{HeadlessHost, HostKey};
    use crate::a11y::Role;
    use crate::widgets::{Button, Column, Pad, Row, VirtualList, VirtualListController};
    use crate::{theme, App, ThemeMode};

    /// Mounts a list of rows that measure taller than the estimate, changes
    /// the theme so they remount and measure again, and goes to the end.
    fn end_offset() -> f32 {
        let controller = VirtualListController::new(0_u32..60, 30.0);
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme::DARK),
            move || {
                Column::new()
                    .child(Pad::all(8.0).child(Row::new().child(Button::new("Append"))))
                    .child(
                        VirtualList::new(controller.clone(), |key: &u32| {
                            Button::new(format!("Row {key}"))
                        })
                        .label("Rows")
                        .size(300.0, 150.0),
                    )
            },
            340,
            240,
            1.0,
        );
        app.apply_reduced_motion(true);
        app.frame();
        let mut host = HeadlessHost::new(app);
        host.set_theme(theme::CANDY);
        host.key(&HostKey::End, false, false);
        let scroll = host
            .app()
            .find_widget(Role::ScrollView, Some("Rows"))
            .unwrap();
        host.app().scene().scroll_offset(scroll).y
    }

    #[test]
    fn the_same_input_ends_at_the_same_offset_every_time() {
        let ends: Vec<f32> = (0..120).map(|_| end_offset()).collect();
        assert!(
            ends.iter().all(|end| (end - ends[0]).abs() < 0.01),
            "the same input ended at different offsets: {ends:?}"
        );
    }
}

/// Rows a wheel mounts under a pointer at rest are hovered when they appear,
/// not when the pointer next moves.
mod hover_after_a_relayout {
    use super::super::headless::HeadlessHost;
    use crate::scene::Point;
    use crate::widgets::probes::hover_disagrees;
    use crate::widgets::{Button, Column, Flex, Row, VirtualList, VirtualListController};
    use crate::{theme, App, ThemeMode};

    #[test]
    fn a_fling_through_a_virtual_list_hovers_the_row_it_mounts_under_the_pointer() {
        let controller = VirtualListController::new(0_u32..200, 30.0);
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme::LIGHT),
            move || {
                VirtualList::new(controller.clone(), |key: &u32| {
                    Button::new(format!("Row {key}"))
                })
                .label("Rows")
                .size(300.0, 200.0)
            },
            340,
            240,
            1.0,
        );
        app.apply_reduced_motion(true);
        app.frame();
        let mut host = HeadlessHost::new(app);
        let pointer = Point { x: 40.0, y: 90.0 };
        host.pointer_move(pointer);
        // Far enough that none of the rows now in view is mounted yet.
        host.wheel(2500.0);
        assert_eq!(
            hover_disagrees(&host.app().widgets, host.app().scene(), pointer, false),
            None
        );
    }

    fn hover_fault(host: &HeadlessHost, pointer: Point, held: bool) -> Option<String> {
        hover_disagrees(&host.app().widgets, host.app().scene(), pointer, held)
    }

    /// Two buttons that each take half the width; a resize moves the boundary.
    fn halves(width: u32) -> App {
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme::LIGHT),
            || {
                Row::new()
                    .fill()
                    .child(Flex::new().grow(1.0).child(Button::new("Left")))
                    .child(Flex::new().grow(1.0).child(Button::new("Right")))
            },
            width,
            100,
            1.0,
        );
        app.apply_reduced_motion(true);
        app.frame();
        app
    }

    /// The name of the control the pointer is over.
    fn under(host: &HeadlessHost, pointer: Point) -> Option<String> {
        let scene = host.app().scene();
        let hit = crate::widgets::hit_test(&host.app().widgets, scene, pointer)?;
        scene
            .a11y(hit)
            .and_then(|a| a.name.as_ref().map(|n| n.to_string()))
    }

    #[test]
    fn a_resize_that_puts_another_control_under_the_pointer_moves_hover_to_it() {
        let mut host = HeadlessHost::new(halves(200));
        let pointer = Point { x: 150.0, y: 10.0 };
        host.pointer_move(pointer);
        let before = under(&host, pointer);
        assert_eq!(hover_fault(&host, pointer, false), None);
        host.resize(400.0, 100.0);
        assert_ne!(under(&host, pointer), before, "the resize moved a control");
        assert_eq!(hover_fault(&host, pointer, false), None);
    }

    #[test]
    fn a_resize_that_leaves_nothing_under_the_pointer_takes_hover_away() {
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme::LIGHT),
            || {
                Row::new()
                    .fill()
                    .child(Flex::new().grow(1.0))
                    .child(Button::new("Edge").width(100.0))
            },
            300,
            100,
            1.0,
        );
        app.apply_reduced_motion(true);
        app.frame();
        let mut host = HeadlessHost::new(app);
        let pointer = Point { x: 250.0, y: 10.0 };
        host.pointer_move(pointer);
        assert_eq!(hover_fault(&host, pointer, false), None);
        host.resize(500.0, 100.0);
        assert_eq!(under(&host, pointer), None);
        assert_eq!(hover_fault(&host, pointer, false), None);
    }

    #[test]
    fn a_resize_does_not_bring_hover_back_while_the_keyboard_holds_it() {
        let mut host = HeadlessHost::new(halves(200));
        let pointer = Point { x: 150.0, y: 10.0 };
        host.pointer_move(pointer);
        host.app_mut().hold_pointer_hover();
        host.resize(400.0, 100.0);
        assert_eq!(hover_fault(&host, pointer, true), None);
    }

    #[test]
    fn a_theme_switch_that_changes_sizes_moves_hover_with_the_content() {
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme::LIGHT),
            || {
                let mut column = Column::new();
                for i in 0..8 {
                    column = column.child(Button::new(format!("Row {i}")));
                }
                column
            },
            300,
            600,
            1.0,
        );
        app.apply_reduced_motion(true);
        app.frame();
        let mut host = HeadlessHost::new(app);
        let pointer = Point { x: 20.0, y: 150.0 };
        host.pointer_move(pointer);
        let before = under(&host, pointer);
        assert_eq!(hover_fault(&host, pointer, false), None);
        host.set_theme(theme::BRUTAL);
        assert_ne!(under(&host, pointer), before, "the theme moved a control");
        assert_eq!(hover_fault(&host, pointer, false), None);
    }

    #[test]
    fn content_that_grows_before_the_pointer_takes_hover_off_the_control_it_pushed_away() {
        use crate::widgets::Text;
        let letters = crate::signal::create_signal(1_usize);
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme::LIGHT),
            move || {
                Row::new()
                    .child(Text::dynamic(move || "m".repeat(letters.get())))
                    .child(Button::new("Go"))
            },
            400,
            100,
            1.0,
        );
        app.apply_reduced_motion(true);
        app.frame();
        let mut host = HeadlessHost::new(app);
        let go = host
            .app()
            .find_widget(crate::a11y::Role::Button, Some("Go"));
        let r = host.app().scene().layout(go.unwrap()).unwrap().rect;
        let pointer = Point {
            x: r.x + 4.0,
            y: r.y + r.height / 2.0,
        };
        host.pointer_move(pointer);
        assert_eq!(under(&host, pointer).as_deref(), Some("Go"));
        assert_eq!(hover_fault(&host, pointer, false), None);
        letters.set(30);
        host.frame();
        assert_ne!(under(&host, pointer).as_deref(), Some("Go"), "content grew");
        assert_eq!(hover_fault(&host, pointer, false), None);
    }
}

/// Input that arrives faster than frames are drawn: several pointer events
/// land between two redraws.
mod input_between_frames {
    use super::super::{event_handler::EventTail, WindowedApp};
    use crate::widgets::{Dialog, Stack, Text};
    use crate::{a11y::Role, App};
    use winit::dpi::PhysicalPosition;
    use winit::event::{ElementState, MouseButton};

    fn to(host: &mut WindowedApp, x: f32, y: f32) {
        host.on_cursor_moved(PhysicalPosition::new(f64::from(x), f64::from(y)));
    }

    fn button(host: &mut WindowedApp, state: ElementState) {
        if host.on_mouse_input(state, MouseButton::Left) == EventTail::Normal {
            host.poll_remount();
        }
    }

    /// Drag a dialog by its title, let go and grab it again before the next
    /// redraw. The second press was hit-tested against the layout from
    /// before the drag, missed the title and left the dialog where it was.
    #[test]
    fn a_dialog_grabbed_again_before_a_redraw_follows_the_pointer() {
        let mut app = App::mount_with_size(
            Stack::new().fill().child(
                Dialog::new("Board")
                    .modeless()
                    .non_fixed()
                    .movable()
                    .at(40.0, 60.0)
                    .size(240.0, 160.0)
                    .child(Text::new("Notes")),
            ),
            640,
            480,
        );
        app.frame();
        let mut host = super::windowed_host(app, None);
        let dialog = |host: &WindowedApp| {
            let id = host.app.find_widget(Role::Dialog, Some("Board")).unwrap();
            host.app.scene().layout(id).unwrap().rect
        };
        let start = dialog(&host);
        let (x, y) = (start.x + 60.0, start.y + 12.0);
        to(&mut host, x, y);
        button(&mut host, ElementState::Pressed);
        to(&mut host, x + 150.0, y + 80.0);
        button(&mut host, ElementState::Released);
        // No redraw: the next press comes within the same frame.
        button(&mut host, ElementState::Pressed);
        to(&mut host, x + 50.0, y + 30.0);
        button(&mut host, ElementState::Released);
        host.on_redraw_requested();
        let end = dialog(&host);
        assert_eq!((end.x - start.x, end.y - start.y), (50.0, 30.0));
    }
}

/// IME input through the window path: a focused text field turns the IME on
/// and places it over the field, and a commit types the composed text.
mod ime {
    use std::cell::RefCell;
    use std::rc::Rc;

    use winit::event::Ime;

    use crate::a11y::Role;
    use crate::widgets::{Button, Column, TextInput};
    use crate::App;

    #[test]
    fn a_focused_text_field_takes_ime_commits_and_places_the_ime() {
        let typed = Rc::new(RefCell::new(String::new()));
        let view = Column::new()
            .gap(8.0)
            .child(TextInput::new("").label("Name").on_input({
                let typed = typed.clone();
                move |text: &str| *typed.borrow_mut() = text.to_owned()
            }))
            .child(Button::new("Go").on_click(|| {}));
        let mut app = App::mount_with_size(view, 400, 200);
        app.frame();
        let mut host = super::windowed_host(app, None);
        host.sync_ime();
        assert_eq!(host.ime_area, None, "nothing focused, so no IME");

        let field = host.app.find_widget(Role::TextInput, Some("Name")).unwrap();
        assert!(host.app.focus(Some(field)));
        host.sync_ime();
        let area = host
            .ime_area
            .expect("a focused text field turns the IME on");
        let rect = host.app.scene().composited_rect(field);
        assert!(
            area.x >= rect.x - 0.5
                && area.y >= rect.y - 0.5
                && area.right() <= rect.right() + 0.5
                && area.bottom() <= rect.bottom() + 0.5,
            "IME area {area:?} sits on the field {rect:?}"
        );

        // A preedit is drawn, not typed, and moves the IME window to its
        // cursor. The commit types the composed text.
        host.on_ime(Ime::Enabled);
        host.on_ime(Ime::Preedit("にほん".into(), Some((3, 3))));
        assert_eq!(*typed.borrow(), "");
        host.sync_ime();
        let cursor = host.ime_area.expect("still on");
        assert!(
            cursor.width < area.width && cursor.x > area.x + 4.0,
            "IME window {cursor:?} follows the preedit cursor, not the field {area:?}"
        );
        host.on_ime(Ime::Preedit(String::new(), None));
        host.sync_ime();
        assert_eq!(host.ime_area, Some(area), "the field again");
        host.on_ime(Ime::Commit("日本".into()));
        assert_eq!(*typed.borrow(), "日本");
        host.on_ime(Ime::Commit("語".into()));
        assert_eq!(*typed.borrow(), "日本語");

        // Focus leaving the field turns the IME off, and a stray commit
        // types nowhere.
        let go = host.app.find_widget(Role::Button, Some("Go")).unwrap();
        assert!(host.app.focus(Some(go)));
        host.sync_ime();
        assert_eq!(host.ime_area, None);
        host.on_ime(Ime::Commit("x".into()));
        assert_eq!(*typed.borrow(), "日本語");
    }
}

/// Multi-click counting follows the app clock, so a host that pins time (the
/// headless fuzz host, a recorded replay) sees the gaps it chose.
#[test]
fn multi_click_counting_follows_the_app_clock() {
    use crate::widgets::{TextInput, TextPointerAction};
    use std::time::Duration;

    let mut app = App::mount_with_size(TextInput::new("hello world").label("Field"), 300, 80);
    app.frame();
    let mut host = windowed_host(app, None);
    let field = host
        .app
        .find_widget(Role::TextInput, Some("Field"))
        .unwrap();
    let at = Point { x: 20.0, y: 20.0 };
    let t0 = std::time::Instant::now();
    host.app.set_clock(Some(t0));
    assert_eq!(
        host.text_press_action(field, at),
        TextPointerAction::Place { extend: false }
    );
    // Two seconds of app time pass in no wall time: a single click again.
    host.app.set_clock(Some(t0 + Duration::from_secs(2)));
    assert_eq!(
        host.text_press_action(field, at),
        TextPointerAction::Place { extend: false },
        "a press two seconds later is not a double-click"
    );
    // A quick second press is a double-click.
    host.app.set_clock(Some(t0 + Duration::from_millis(2100)));
    assert_eq!(
        host.text_press_action(field, at),
        TextPointerAction::SelectWord
    );
}

mod context_menu_clipboard {
    use super::*;
    use crate::debug_server::DebugKey;
    use accesskit::{Action, ActionRequest, TreeId};

    fn field_host() -> (super::super::WindowedApp, crate::scene::WidgetId) {
        let mut app = App::mount(crate::widgets::TextInput::new("Buy oat milk"));
        app.frame();
        let input = app.find_widget(Role::TextInput, None).unwrap();
        app.focus(Some(input));
        let mut host = windowed_host(app, None);
        host.handle_debug_key(key("select_all"));
        (host, input)
    }

    fn key(name: &str) -> DebugKey {
        DebugKey {
            key: name.into(),
            shift: false,
            ctrl: false,
            text: None,
        }
    }

    fn value(host: &super::super::WindowedApp, id: crate::scene::WidgetId) -> String {
        host.app.scene().a11y(id).unwrap().value.clone().unwrap()
    }

    /// What the window's AccessKit handler does: dispatch, then drain.
    fn act(host: &mut super::super::WindowedApp, action: Action, id: crate::scene::WidgetId) {
        let request = ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: crate::a11y::to_access_id(id),
            data: None,
        };
        assert!(host.app.dispatch_action(&request));
        host.drain_pending_clipboard();
        host.app.frame();
    }

    fn pick(host: &mut super::super::WindowedApp, name: &str) {
        let item = host.app.find_widget(Role::MenuItem, Some(name)).unwrap();
        act(host, Action::Click, item);
    }

    #[test]
    fn accesskit_copy_then_paste() {
        let (mut host, input) = field_host();
        act(&mut host, Action::ShowContextMenu, input);
        pick(&mut host, "Copy");
        assert_eq!(host.clipboard.read_text().as_deref(), Ok("Buy oat milk"));
        host.handle_debug_key(key("end"));
        act(&mut host, Action::ShowContextMenu, input);
        pick(&mut host, "Paste");
        assert_eq!(value(&host, input), "Buy oat milkBuy oat milk");
    }

    #[test]
    fn accesskit_cut() {
        let (mut host, input) = field_host();
        act(&mut host, Action::ShowContextMenu, input);
        pick(&mut host, "Cut");
        assert_eq!(value(&host, input), "");
        assert_eq!(host.clipboard.read_text().as_deref(), Ok("Buy oat milk"));
    }

    #[test]
    fn debug_enter_on_focused_row_copies_and_pastes() {
        let (mut host, input) = field_host();
        act(&mut host, Action::ShowContextMenu, input);
        // Rows: Cut, Copy, Paste, Select All. Home lands on the first.
        host.handle_debug_key(key("home"));
        host.handle_debug_key(key("down"));
        host.handle_debug_key(key("enter"));
        assert_eq!(host.clipboard.read_text().as_deref(), Ok("Buy oat milk"));
        host.handle_debug_key(key("end"));
        act(&mut host, Action::ShowContextMenu, input);
        host.handle_debug_key(key("home"));
        host.handle_debug_key(key("down"));
        host.handle_debug_key(key("down"));
        host.handle_debug_key(key("enter"));
        assert_eq!(value(&host, input), "Buy oat milkBuy oat milk");
    }

    #[test]
    fn debug_ctrl_home_and_end_reach_the_text_area() {
        let mut app = App::mount(crate::widgets::TextArea::new("one\ntwo\nthree"));
        app.frame();
        let area = app.find_widget(Role::MultilineTextInput, None).unwrap();
        app.focus(Some(area));
        let mut host = windowed_host(app, None);
        let ctrl = |name: &str| DebugKey {
            ctrl: true,
            ..key(name)
        };
        host.handle_debug_key(ctrl("home"));
        host.app
            .dispatch_edit_key(crate::widgets::EditKey::Insert("^"));
        assert_eq!(value(&host, area), "^one\ntwo\nthree");
        host.handle_debug_key(ctrl("end"));
        host.app
            .dispatch_edit_key(crate::widgets::EditKey::Insert("$"));
        assert_eq!(value(&host, area), "^one\ntwo\nthree$");
        // Without Ctrl, Home stays on the line.
        host.handle_debug_key(key("home"));
        host.app
            .dispatch_edit_key(crate::widgets::EditKey::Insert("@"));
        assert_eq!(value(&host, area), "^one\ntwo\n@three$");
    }

    #[test]
    fn pointer_path_still_pastes() {
        let (mut host, input) = field_host();
        act(&mut host, Action::ShowContextMenu, input);
        pick(&mut host, "Copy");
        host.handle_debug_key(key("end"));
        act(&mut host, Action::ShowContextMenu, input);
        let item = host.app.find_widget(Role::MenuItem, Some("Paste")).unwrap();
        assert!(host.activate_context_menu_item(item));
        assert_eq!(value(&host, input), "Buy oat milkBuy oat milk");
    }
}

/// A decoration on a scroll view lays its recolours on the rows inside it. A
/// virtual list recycling those rows must take the records with them.
mod ring_on_a_virtual_list {
    use super::super::headless::{HeadlessHost, HostKey};
    use crate::scene::Color;
    use crate::widgets::{
        Button, Column, InteractionStates, InteractionStyle, Pad, Row, Theme, VirtualList,
        VirtualListController,
    };
    use crate::{theme, App, ThemeMode};
    use std::time::Duration;

    const fn every_channel() -> InteractionStates {
        let style = InteractionStyle {
            background: Some(Color::rgba(70, 40, 90, 0x50)),
            foreground: Some(Color::rgb(200, 30, 30)),
            border: Some(Color::rgb(30, 30, 200)),
        };
        InteractionStates {
            hover: style,
            focus: style,
            active: style,
        }
    }

    #[test]
    fn paging_a_ringed_virtual_list_leaves_no_record_on_a_recycled_row() {
        let loud = Theme {
            interactions: every_channel(),
            ..theme::LIGHT
        };
        let controller = VirtualListController::new(0_u32..60, 30.0);
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(loud),
            move || {
                Column::new()
                    .child(Pad::all(8.0).child(Row::new().child(Button::new("Append"))))
                    .child(
                        VirtualList::new(controller.clone(), |key: &u32| {
                            Button::new(format!("Row {key}"))
                        })
                        .label("Rows")
                        .size(300.0, 150.0)
                        .scrollbar(true),
                    )
            },
            340,
            240,
            1.0,
        );
        app.frame();
        let mut host = HeadlessHost::new(app);
        // Shift+Tab lands on the list's scroll view, which wears the ring.
        host.key(&HostKey::Tab, true, false);
        host.transition_theme(theme::LIGHT, Duration::from_millis(50));
        host.advance(Duration::from_millis(1));
        // Paging unmounts the rows the ring recoloured.
        host.key(&HostKey::Space, false, false);
        host.key(&HostKey::Space, false, false);
    }
}

/// Where a screen reader puts each node: its bounds taken through its own
/// transform and every ancestor's, up to the root.
fn accesskit_screen_rects(
    scene: &crate::scene::Scene,
) -> std::collections::HashMap<u64, (f64, f64, f64, f64)> {
    use std::collections::HashMap;
    let update = crate::a11y::build_full_tree_update(scene);
    let mut parent: HashMap<u64, u64> = HashMap::new();
    for (id, node) in &update.nodes {
        for child in node.children() {
            parent.insert(child.0, id.0);
        }
    }
    let nodes: HashMap<u64, &accesskit::Node> =
        update.nodes.iter().map(|(id, n)| (id.0, n)).collect();
    let mut out = HashMap::new();
    for (&id, node) in &nodes {
        let Some(bounds) = node.bounds() else {
            continue;
        };
        let mut rect = bounds;
        let mut at = Some(id);
        while let Some(current) = at {
            if let Some(t) = nodes[&current].transform() {
                rect = t.transform_rect_bbox(rect);
            }
            at = parent.get(&current).copied();
        }
        out.insert(id, (rect.x0, rect.y0, rect.x1, rect.y1));
    }
    out
}

fn debug_tree_rects(scene: &crate::scene::Scene) -> std::collections::HashMap<u64, [f64; 4]> {
    fn walk(value: &serde_json::Value, out: &mut std::collections::HashMap<u64, [f64; 4]>) {
        if let (Some(id), Some(r)) = (value["id"].as_u64(), value.get("rect")) {
            let f = |k: &str| r[k].as_f64().unwrap();
            out.insert(id, [f("x"), f("y"), f("width"), f("height")]);
        }
        for child in value["children"].as_array().into_iter().flatten() {
            walk(child, out);
        }
    }
    let mut out = std::collections::HashMap::new();
    walk(&super::debug_tree_json(scene)["root"], &mut out);
    out
}

/// The accessibility tree, the debug tree and the painter place every visible
/// widget at the same window position.
fn assert_three_rects_agree(app: &App, what: &str) {
    let scene = app.scene();
    let ak = accesskit_screen_rects(scene);
    let dbg = debug_tree_rects(scene);
    let mut checked = 0;
    for id in scene.preorder() {
        let key = crate::a11y::to_access_id(id).0;
        let Some(layout) = scene.layout(id) else {
            continue;
        };
        let drawn = scene.composited_rect(id);
        // Only widgets wholly inside every clip: a clipped box is cut on one
        // side and no longer says where the widget sits.
        if layout.rect.is_empty() || (drawn.width - layout.rect.width).abs() > 0.01 {
            continue;
        }
        if (drawn.height - layout.rect.height).abs() > 0.01 {
            continue;
        }
        let d = dbg[&key];
        let (x0, y0, x1, y1) = ak[&key];
        let close = |a: f64, b: f64| (a - b).abs() < 0.01;
        assert!(
            close(d[0], drawn.x as f64) && close(d[1], drawn.y as f64),
            "{what}: debug tree puts {id:?} at {:?} but it is drawn at {drawn:?}",
            &d[..2]
        );
        assert!(
            close(x0, drawn.x as f64)
                && close(y0, drawn.y as f64)
                && close(x1 - x0, drawn.width as f64)
                && close(y1 - y0, drawn.height as f64),
            "{what}: accessibility puts {id:?} at {:?} but it is drawn at {drawn:?}",
            (x0, y0, x1 - x0, y1 - y0)
        );
        checked += 1;
    }
    assert!(checked >= 1, "{what}: only {checked} widgets compared");
}

fn tall_buttons(n: usize) -> crate::widgets::Column {
    let mut col = crate::widgets::Column::new().gap(8.0);
    for i in 0..n {
        col = col.child(Button::new(format!("Button {i}")));
    }
    col
}

#[test]
fn tree_rects_follow_a_scroll_moved_by_tab_wheel_and_keys() {
    use crate::widgets::Scroll;
    let mut app = App::mount_with_size(
        Scroll::new().size(300.0, 120.0).child(tall_buttons(20)),
        400,
        300,
    );
    app.frame();
    assert_three_rects_agree(&app, "unscrolled");
    for i in 0..9 {
        assert!(app.dispatch_key(UiKey::Tab { shift: false }));
        app.frame();
        assert_three_rects_agree(&app, &format!("tab {i}"));
    }
    assert!(app.dispatch_wheel_at(Point { x: 50.0, y: 50.0 }, 37.0));
    app.frame();
    assert_three_rects_agree(&app, "wheel");
    assert!(app.dispatch_key(UiKey::PageDown));
    app.frame();
    assert_three_rects_agree(&app, "page down");
}

#[test]
fn tree_rects_follow_nested_scrolls() {
    use crate::widgets::{Column, Scroll};
    let inner = Scroll::new()
        .label("inner")
        .size(260.0, 100.0)
        .child(tall_buttons(12));
    let outer = Scroll::new().label("outer").size(300.0, 150.0).child(
        Column::new()
            .child(Button::new("Before"))
            .child(inner)
            .child(tall_buttons(10)),
    );
    let mut app = App::mount_with_size(outer, 400, 400);
    app.frame();
    for _ in 0..30 {
        assert!(app.dispatch_key(UiKey::Tab { shift: false }));
        app.frame();
        assert_three_rects_agree(&app, "nested tab");
    }
}

#[test]
fn tree_rects_follow_a_scroll_restored_after_a_remount() {
    use crate::widgets::Scroll;
    let build = || {
        Scroll::new()
            .label("page")
            .restoration_key("page")
            .size(300.0, 120.0)
            .child(tall_buttons(20))
    };
    let mut app = App::mount_with_size(build(), 400, 300);
    app.frame();
    let sv = app.find_widget(Role::ScrollView, None).unwrap();
    assert!(app.dispatch_scroll(sv, 80.0));
    let mut replacement = App::mount_with_size(build(), 400, 300);
    replacement.inherit_remount_state(&app);
    replacement.frame();
    let sv = replacement.find_widget(Role::ScrollView, None).unwrap();
    assert!(replacement.scene().scroll_offset(sv).y > 0.0);
    assert_three_rects_agree(&replacement, "restored");
}

#[test]
fn tree_rects_follow_list_rows_and_a_text_area_in_a_scroll() {
    use crate::widgets::{Column, List, ListItem, Scroll, TextArea};
    let mut list = List::new();
    for i in 0..30 {
        list = list.child(ListItem::new(format!("Item {i}")));
    }
    let mut app = App::mount_with_size(Scroll::new().size(300.0, 120.0).child(list), 400, 300);
    app.frame();
    assert!(app.dispatch_key(UiKey::Tab { shift: false }));
    for i in 0..25 {
        assert!(app.dispatch_key(UiKey::Down { shift: false }));
        app.frame();
        assert_three_rects_agree(&app, &format!("list row {i}"));
    }

    let area = Column::new()
        .child(tall_buttons(6))
        .child(TextArea::new("a\nb\nc\nd\ne\nf\ng\nh".repeat(4)));
    let mut app = App::mount_with_size(Scroll::new().size(300.0, 120.0).child(area), 400, 300);
    app.frame();
    for i in 0..8 {
        assert!(app.dispatch_key(UiKey::Tab { shift: false }));
        app.frame();
        assert_three_rects_agree(&app, &format!("text area tab {i}"));
    }
    assert!(app.dispatch_key(UiKey::PageDown));
    app.frame();
    assert_three_rects_agree(&app, "text area page");
}

/// The debug server's pointer commands reach the window's own press, release,
/// move and leave handlers, so a press, a drag and a release arrive in the
/// orders a real mouse produces them.
mod debug_pointer {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::super::WindowedApp;
    use super::windowed_host;
    use crate::a11y::Role;
    use crate::debug_server::{
        DebugButton, DebugCommand, DebugDrag, DebugKey, DebugPoint, DebugPress,
    };
    use crate::widgets::{Button, Column, Slider, TextInput};
    use crate::App;

    struct Rig {
        host: WindowedApp,
        clicks: Rc<Cell<u32>>,
        held_clicks: Rc<Cell<u32>>,
        slider: Rc<Cell<f32>>,
    }

    fn rig() -> Rig {
        let clicks = Rc::new(Cell::new(0));
        let held_clicks = Rc::new(Cell::new(0));
        let slider = Rc::new(Cell::new(0.0));
        let (c, h, s) = (clicks.clone(), held_clicks.clone(), slider.clone());
        let mut app = App::mount_with_size(
            Column::new()
                .gap(20.0)
                .child(Button::new("Go").on_click(move || c.set(c.get() + 1)))
                .child(
                    Button::new("Held")
                        .on_drag_start(|| {})
                        .on_click(move || h.set(h.get() + 1)),
                )
                .child(
                    Slider::new(0.0, 0.0, 100.0)
                        .name("Level")
                        .on_change(move |v| s.set(v)),
                )
                .child(TextInput::new("hello world")),
            400,
            500,
        );
        app.frame();
        Rig {
            host: windowed_host(app, None),
            clicks,
            held_clicks,
            slider,
        }
    }

    fn rect(host: &WindowedApp, role: Role, name: Option<&str>) -> crate::scene::Rect {
        let id = host.app.find_widget(role, name).unwrap();
        host.app.scene().layout(id).unwrap().rect
    }

    fn centre(r: crate::scene::Rect) -> (f32, f32) {
        (r.x + r.width / 2.0, r.y + r.height / 2.0)
    }

    fn press(x: Option<f32>, y: Option<f32>, button: DebugButton) -> DebugCommand {
        DebugCommand::PointerPress(DebugPress { x, y, button })
    }

    fn release(x: Option<f32>, y: Option<f32>, button: DebugButton) -> DebugCommand {
        DebugCommand::PointerRelease(DebugPress { x, y, button })
    }

    fn run(host: &mut WindowedApp, command: DebugCommand) -> serde_json::Value {
        let reply = host.handle_debug_command(None, command);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        serde_json::from_slice(&reply.body).unwrap()
    }

    #[test]
    fn press_then_release_clicks_a_button() {
        let mut rig = rig();
        let (x, y) = centre(rect(&rig.host, Role::Button, Some("Go")));
        let status = run(&mut rig.host, press(Some(x), Some(y), DebugButton::Left));
        assert_eq!(status["status"]["cursor"]["x"].as_f64().unwrap() as f32, x);
        run(&mut rig.host, release(None, None, DebugButton::Left));
        assert_eq!(rig.clicks.get(), 1);
    }

    #[test]
    fn a_plain_button_clicks_on_press() {
        let mut rig = rig();
        let (x, y) = centre(rect(&rig.host, Role::Button, Some("Go")));
        run(&mut rig.host, press(Some(x), Some(y), DebugButton::Left));
        assert_eq!(rig.clicks.get(), 1);
    }

    #[test]
    fn a_plain_button_does_not_click_again_on_release_elsewhere() {
        let mut rig = rig();
        let (x, y) = centre(rect(&rig.host, Role::Button, Some("Go")));
        run(&mut rig.host, press(Some(x), Some(y), DebugButton::Left));
        run(
            &mut rig.host,
            DebugCommand::PointerMove(DebugPoint { x: 390.0, y: 490.0 }),
        );
        let status = run(&mut rig.host, release(None, None, DebugButton::Left));
        assert_eq!(rig.clicks.get(), 1);
        assert_eq!(status["status"]["pointer"]["left_down"], false);
    }

    #[test]
    fn press_then_release_clicks_a_drag_source_button() {
        let mut rig = rig();
        let (x, y) = centre(rect(&rig.host, Role::Button, Some("Held")));
        run(&mut rig.host, press(Some(x), Some(y), DebugButton::Left));
        assert_eq!(rig.held_clicks.get(), 0);
        run(&mut rig.host, release(None, None, DebugButton::Left));
        assert_eq!(rig.held_clicks.get(), 1);
    }

    #[test]
    fn press_leave_release_ends_the_press() {
        let mut rig = rig();
        let (x, y) = centre(rect(&rig.host, Role::Slider, Some("Level")));
        let status = run(&mut rig.host, press(Some(x), Some(y), DebugButton::Left));
        assert_eq!(status["status"]["pointer"]["slider_drag"], true);
        let status = run(&mut rig.host, DebugCommand::PointerLeave);
        assert_eq!(status["status"]["cursor_inside"], false);
        let status = run(&mut rig.host, release(None, None, DebugButton::Left));
        assert_eq!(status["status"]["pointer"]["left_down"], false);
        assert_eq!(status["status"]["pointer"]["slider_drag"], false);
    }

    #[test]
    fn drag_scrubs_a_slider() {
        let mut rig = rig();
        let r = rect(&rig.host, Role::Slider, Some("Level"));
        let y = r.y + r.height / 2.0;
        let status = run(
            &mut rig.host,
            DebugCommand::PointerDrag(DebugDrag {
                x1: r.x + 2.0,
                y1: y,
                x2: r.x + r.width - 2.0,
                y2: y,
                steps: Some(5),
                button: DebugButton::Left,
            }),
        );
        assert!(status["status"]["cursor"]["x"].is_number());
        assert!(rig.slider.get() > 90.0, "value {}", rig.slider.get());
    }

    #[test]
    fn drag_across_text_selects_it() {
        let mut rig = rig();
        let r = rect(&rig.host, Role::TextInput, None);
        let y = r.y + r.height / 2.0;
        run(
            &mut rig.host,
            DebugCommand::PointerDrag(DebugDrag {
                x1: r.x + 3.0,
                y1: y,
                x2: r.x + r.width - 3.0,
                y2: y,
                steps: None,
                button: DebugButton::Left,
            }),
        );
        let key = DebugKey {
            key: "text".into(),
            shift: false,
            ctrl: false,
            text: Some("X".into()),
        };
        rig.host.handle_debug_key(key);
        let input = rig.host.app.find_widget(Role::TextInput, None).unwrap();
        let value = rig.host.app.scene().a11y(input).unwrap().value.clone();
        assert_eq!(value.as_deref(), Some("X"));
    }

    #[test]
    fn presses_close_in_app_time_double_click() {
        let mut rig = rig();
        let t0 = std::time::Instant::now();
        rig.host.app.set_clock(Some(t0));
        let r = rect(&rig.host, Role::TextInput, None);
        let (x, y) = (r.x + 6.0, r.y + r.height / 2.0);
        for _ in 0..2 {
            run(&mut rig.host, press(Some(x), Some(y), DebugButton::Left));
            run(&mut rig.host, release(None, None, DebugButton::Left));
        }
        let key = DebugKey {
            key: "text".into(),
            shift: false,
            ctrl: false,
            text: Some("X".into()),
        };
        rig.host.handle_debug_key(key);
        let input = rig.host.app.find_widget(Role::TextInput, None).unwrap();
        let value = rig.host.app.scene().a11y(input).unwrap().value.clone();
        assert_eq!(value.as_deref(), Some("X world"));
    }

    fn typed_x_value(host: &mut WindowedApp) -> Option<String> {
        let key = DebugKey {
            key: "text".into(),
            shift: false,
            ctrl: false,
            text: Some("X".into()),
        };
        host.handle_debug_key(key);
        let input = host.app.find_widget(Role::TextInput, None).unwrap();
        host.app.scene().a11y(input).unwrap().value.clone()
    }

    #[test]
    fn click_at_places_the_caret_in_a_text_input() {
        let mut rig = rig();
        let r = rect(&rig.host, Role::TextInput, None);
        let point = DebugPoint {
            x: r.x + 3.0,
            y: r.y + r.height / 2.0,
        };
        run(&mut rig.host, DebugCommand::PointerClick(point));
        assert_eq!(
            typed_x_value(&mut rig.host).as_deref(),
            Some("Xhello world")
        );
    }

    #[test]
    fn click_at_clicks_a_button_once() {
        let mut rig = rig();
        let (x, y) = centre(rect(&rig.host, Role::Button, Some("Go")));
        run(
            &mut rig.host,
            DebugCommand::PointerClick(DebugPoint { x, y }),
        );
        assert_eq!(rig.clicks.get(), 1);
    }

    #[test]
    fn click_at_an_empty_spot_blurs_the_focused_input() {
        let mut rig = rig();
        let (x, y) = centre(rect(&rig.host, Role::TextInput, None));
        run(
            &mut rig.host,
            DebugCommand::PointerClick(DebugPoint { x, y }),
        );
        assert!(rig.host.app.focused_widget().is_some());
        let status = run(
            &mut rig.host,
            DebugCommand::PointerClick(DebugPoint { x: 390.0, y: 490.0 }),
        );
        assert!(status["status"]["focused"].is_null(), "{status}");
        assert!(status["target"].is_null(), "{status}");
        assert_eq!(rig.host.app.focused_widget(), None);
    }

    #[test]
    fn click_at_names_the_widget_it_hit() {
        let mut rig = rig();
        let (x, y) = centre(rect(&rig.host, Role::Button, Some("Go")));
        let status = run(
            &mut rig.host,
            DebugCommand::PointerClick(DebugPoint { x, y }),
        );
        assert_eq!(status["target"]["name"], "Go", "{status}");
    }

    #[test]
    fn click_at_an_empty_spot_closes_an_open_dropdown() {
        use crate::widgets::{Dropdown, DropdownOption};
        let toggles = Rc::new(Cell::new(0));
        let t = toggles.clone();
        let mut app = App::mount_with_size(
            Column::new().child(
                Dropdown::new("Theme")
                    .open(true)
                    .on_toggle(move || t.set(t.get() + 1))
                    .option(DropdownOption::new("Light").selected(true))
                    .option(DropdownOption::new("Dark")),
            ),
            400,
            500,
        );
        app.frame();
        let mut host = windowed_host(app, None);
        run(
            &mut host,
            DebugCommand::PointerClick(DebugPoint { x: 390.0, y: 490.0 }),
        );
        assert_eq!(toggles.get(), 1, "the click away dismissed the list");
    }

    #[test]
    fn a_press_and_release_in_one_frame_places_the_caret() {
        let mut rig = rig();
        let r = rect(&rig.host, Role::TextInput, None);
        let scale = f64::from(rig.host.app.scale());
        rig.host.on_cursor_moved(winit::dpi::PhysicalPosition::new(
            f64::from(r.x + 3.0) * scale,
            f64::from(r.y + r.height / 2.0) * scale,
        ));
        rig.host.on_mouse_input(
            winit::event::ElementState::Pressed,
            winit::event::MouseButton::Left,
        );
        rig.host.on_mouse_input(
            winit::event::ElementState::Released,
            winit::event::MouseButton::Left,
        );
        assert_eq!(
            typed_x_value(&mut rig.host).as_deref(),
            Some("Xhello world")
        );
    }

    #[test]
    fn right_press_opens_the_context_menu() {
        let mut rig = rig();
        let (x, y) = centre(rect(&rig.host, Role::TextInput, None));
        run(&mut rig.host, press(Some(x), Some(y), DebugButton::Right));
        assert!(rig
            .host
            .app
            .find_widget(Role::MenuItem, Some("Copy"))
            .is_some());
    }
}

/// A modal backdrop dismisses like a browser click: the press and the release
/// both land on it.
mod backdrop_click {
    use std::cell::Cell;
    use std::rc::Rc;

    use winit::dpi::PhysicalPosition;
    use winit::event::{ElementState, MouseButton};

    use super::super::{event_handler::EventTail, WindowedApp};
    use crate::a11y::Role;
    use crate::widgets::{Button, Dialog, Stack, Text};
    use crate::App;

    fn host() -> (Rc<Cell<u32>>, WindowedApp) {
        let dismissed = Rc::new(Cell::new(0));
        let d = dismissed.clone();
        let mut app = App::mount_with_size(
            Stack::new().fill().child(
                Dialog::new("Approval")
                    .size(240.0, 160.0)
                    .child(Text::new("Sure?"))
                    .child(Button::new("Yes").on_click(|| {}))
                    .on_dismiss(move || d.set(d.get() + 1)),
            ),
            640,
            480,
        );
        app.frame();
        (dismissed, super::windowed_host(app, None))
    }

    fn to(host: &mut WindowedApp, x: f32, y: f32) {
        host.on_cursor_moved(PhysicalPosition::new(f64::from(x), f64::from(y)));
    }

    fn button(host: &mut WindowedApp, state: ElementState) {
        if host.on_mouse_input(state, MouseButton::Left) == EventTail::Normal {
            host.poll_remount();
        }
    }

    fn inside(host: &WindowedApp) -> (f32, f32) {
        let id = host
            .app
            .find_widget(Role::Dialog, Some("Approval"))
            .unwrap();
        let r = host.app.scene().layout(id).unwrap().rect;
        (r.x + r.width / 2.0, r.y + r.height - 10.0)
    }

    #[test]
    fn press_and_release_on_the_backdrop_dismisses_once() {
        let (dismissed, mut host) = host();
        to(&mut host, 10.0, 10.0);
        button(&mut host, ElementState::Pressed);
        to(&mut host, 30.0, 20.0);
        button(&mut host, ElementState::Released);
        assert_eq!(dismissed.get(), 1);
    }

    #[test]
    fn a_backdrop_press_released_inside_the_dialog_does_not_dismiss() {
        let (dismissed, mut host) = host();
        let (x, y) = inside(&host);
        to(&mut host, 10.0, 10.0);
        button(&mut host, ElementState::Pressed);
        to(&mut host, 120.0, 100.0);
        to(&mut host, x, y);
        button(&mut host, ElementState::Released);
        assert_eq!(dismissed.get(), 0);
    }

    #[test]
    fn a_dialog_press_released_on_the_backdrop_does_not_dismiss() {
        let (dismissed, mut host) = host();
        let (x, y) = inside(&host);
        to(&mut host, x, y);
        button(&mut host, ElementState::Pressed);
        to(&mut host, 10.0, 10.0);
        button(&mut host, ElementState::Released);
        assert_eq!(dismissed.get(), 0);
    }
}

/// A remount while a pointer button is held: the trace names each capture the
/// replacement dropped, and none it carried over.
mod remount_during_a_held_press {
    use std::cell::Cell;
    use std::rc::Rc;

    use serde_json::Value;
    use winit::dpi::PhysicalPosition;
    use winit::event::{ElementState, MouseButton};

    use super::super::{event_handler::EventTail, WindowedApp};
    use crate::a11y::Role;
    use crate::interaction_debug::{InteractionRecorder, InteractionTrace};
    use crate::widgets::{Button, Dialog, Stack, Text};
    use crate::{App, Remount, WindowUpdate};

    fn view(open: bool, modal: bool, dismissed: Rc<Cell<u32>>) -> Stack {
        let stack = Stack::new().fill();
        if !open {
            return stack.child(Text::new("Closed"));
        }
        let dialog = Dialog::new("Approval")
            .size(240.0, 160.0)
            .at(30.0, 60.0)
            .child(Text::new("Sure?"))
            .child(Button::new("Yes").on_click(|| {}))
            .on_dismiss(move || dismissed.set(dismissed.get() + 1));
        stack.child(if modal {
            dialog
        } else {
            dialog.modeless().non_fixed().movable()
        })
    }

    struct Rig {
        host: WindowedApp,
        /// What the next remount mounts: `None` for no remount, else whether
        /// the dialog is still open.
        next: Rc<Cell<Option<bool>>>,
        dismissed: Rc<Cell<u32>>,
        trace: std::path::PathBuf,
    }

    fn rig(tag: &str, modal: bool) -> Rig {
        let dismissed = Rc::new(Cell::new(0));
        let next = Rc::new(Cell::new(None::<bool>));
        let mut app = App::mount_with_size(view(true, modal, dismissed.clone()), 640, 480);
        app.frame();
        let (n, d) = (next.clone(), dismissed.clone());
        let mut host = super::windowed_host(
            app,
            Some(Box::new(move |size| {
                let open = n.take()?;
                let app = App::mount_with_size(
                    view(open, modal, d.clone()),
                    size.width as u32,
                    size.height as u32,
                );
                Some(WindowUpdate::Remount(Remount::new(app, "test")))
            })),
        );
        let trace = std::env::temp_dir().join(format!(
            "schnellui-remount-held-{}-{tag}.jsonl",
            std::process::id()
        ));
        host.interaction_trace =
            InteractionRecorder::open(Some(InteractionTrace::file(&trace))).unwrap();
        Rig {
            host,
            next,
            dismissed,
            trace,
        }
    }

    fn to(host: &mut WindowedApp, x: f32, y: f32) {
        host.on_cursor_moved(PhysicalPosition::new(f64::from(x), f64::from(y)));
    }

    fn button(host: &mut WindowedApp, state: ElementState) {
        if host.on_mouse_input(state, MouseButton::Left) == EventTail::Normal {
            host.poll_remount();
        }
    }

    /// The `interrupted` list of every interrupted-by-remount record.
    fn interrupted(rig: &Rig) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(&rig.trace).unwrap();
        let _ = std::fs::remove_file(&rig.trace);
        text.lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|r| r["event"] == "interaction_interrupted_by_remount")
            .map(|r| {
                r["interrupted"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect()
            })
            .collect()
    }

    fn remount(rig: &mut Rig, open: bool) {
        rig.next.set(Some(open));
        rig.host.poll_remount();
    }

    #[test]
    fn a_backdrop_press_dropped_by_a_remount_is_traced() {
        let mut rig = rig("backdrop-dropped", true);
        to(&mut rig.host, 10.0, 10.0);
        button(&mut rig.host, ElementState::Pressed);
        remount(&mut rig, false);
        button(&mut rig.host, ElementState::Released);
        assert_eq!(rig.dismissed.get(), 0, "the release clicks nothing");
        assert_eq!(interrupted(&rig), [["backdrop_press"]]);
    }

    #[test]
    fn a_backdrop_press_carried_by_a_remount_is_not_traced() {
        let mut rig = rig("backdrop-carried", true);
        to(&mut rig.host, 10.0, 10.0);
        button(&mut rig.host, ElementState::Pressed);
        remount(&mut rig, true);
        button(&mut rig.host, ElementState::Released);
        assert_eq!(
            rig.dismissed.get(),
            1,
            "the press carried over and dismissed"
        );
        assert!(interrupted(&rig).is_empty());
    }

    #[test]
    fn a_dialog_drag_carried_by_a_remount_is_not_listed_as_interrupted() {
        let mut rig = rig("dialog-carried", false);
        let panel = rig
            .host
            .app
            .find_widget(Role::Dialog, Some("Approval"))
            .unwrap();
        let r = rig.host.app.scene().layout(panel).unwrap().rect;
        to(&mut rig.host, r.x + 20.0, r.y + 18.0);
        button(&mut rig.host, ElementState::Pressed);
        let held = schnellui::widgets::interaction_debug_state(&rig.host.app.widgets);
        assert!(
            held.dialog_pointer_capture,
            "the title bar grabbed the dialog"
        );
        remount(&mut rig, true);
        let carried = schnellui::widgets::interaction_debug_state(&rig.host.app.widgets);
        assert!(
            carried.dialog_pointer_capture,
            "the drag followed the dialog"
        );
        assert!(interrupted(&rig).is_empty());
    }

    #[test]
    fn a_dialog_drag_dropped_by_a_remount_is_listed() {
        let mut rig = rig("dialog-dropped", false);
        let panel = rig
            .host
            .app
            .find_widget(Role::Dialog, Some("Approval"))
            .unwrap();
        let r = rig.host.app.scene().layout(panel).unwrap().rect;
        to(&mut rig.host, r.x + 20.0, r.y + 18.0);
        button(&mut rig.host, ElementState::Pressed);
        remount(&mut rig, false);
        assert_eq!(interrupted(&rig), [["dialog_pointer"]]);
    }

    #[test]
    fn a_text_selection_dropped_by_a_subtree_replacement_is_traced() {
        use crate::scene::ComponentRef;
        use crate::widgets::{Column, TextInput, View};
        use crate::SubtreeReplacement;

        let slot = ComponentRef::new();
        let mut app = App::mount_with_size(
            Column::new().child(Column::new().child(TextInput::new("hello")).with_ref(slot)),
            400,
            200,
        );
        app.frame();
        let swap = Rc::new(Cell::new(false));
        let s = swap.clone();
        let mut host = super::windowed_host(
            app,
            Some(Box::new(move |_| {
                s.take().then(|| {
                    WindowUpdate::Subtrees(vec![SubtreeReplacement::new(
                        slot,
                        Text::new("gone"),
                        "test",
                    )])
                })
            })),
        );
        let trace = std::env::temp_dir().join(format!(
            "schnellui-subtree-held-{}.jsonl",
            std::process::id()
        ));
        host.interaction_trace =
            InteractionRecorder::open(Some(InteractionTrace::file(&trace))).unwrap();
        let id = host.app.find_widget(Role::TextInput, None).unwrap();
        let r = host.app.scene().layout(id).unwrap().rect;
        to(&mut host, r.x + 10.0, r.y + r.height / 2.0);
        button(&mut host, ElementState::Pressed);
        assert!(host.drag_text.is_some(), "the press began a selection");
        swap.set(true);
        host.poll_remount();
        assert!(host.drag_text.is_none());
        let rig = Rig {
            host,
            next: Rc::new(Cell::new(None)),
            dismissed: Rc::new(Cell::new(0)),
            trace,
        };
        assert_eq!(interrupted(&rig), [["text_selection"]]);
    }

    #[test]
    fn a_raw_capture_dropped_by_a_remount_is_traced() {
        use crate::RawPointerButton;
        let mut rig = rig("raw", true);
        rig.host.raw_pointer_capture.push(RawPointerButton::Left);
        remount(&mut rig, false);
        assert!(rig.host.raw_pointer_capture.is_empty());
        assert_eq!(interrupted(&rig), [["raw_pointer"]]);
    }
}

/// Activating a window with the pointer gives it keyboard focus, as on a
/// desktop: the control that last had focus in it, else nothing.
mod window_activation {
    use winit::dpi::PhysicalPosition;
    use winit::event::{ElementState, MouseButton};

    use super::super::{event_handler::EventTail, WindowedApp};
    use crate::a11y::Role;
    use crate::widgets::{Button, Dialog, Stack, Text};
    use crate::App;

    fn window(title: &'static str, button: &'static str, x: f32, y: f32) -> Dialog {
        Dialog::new(title)
            .modeless()
            .non_fixed()
            .movable()
            .at(x, y)
            .size(240.0, 160.0)
            .child(Text::new("Notes"))
            .child(Button::new(button).on_click(|| {}))
    }

    /// Two windows that overlap: the second one is on top, over the first's
    /// button.
    fn host() -> WindowedApp {
        let mut app = App::mount_with_size(
            Stack::new()
                .fill()
                .child(window("Back", "Assign", 20.0, 20.0))
                .child(window("Front", "Send", 120.0, 60.0)),
            640,
            480,
        );
        app.frame();
        super::windowed_host(app, None)
    }

    fn press_release(host: &mut WindowedApp, x: f32, y: f32) {
        host.on_cursor_moved(PhysicalPosition::new(f64::from(x), f64::from(y)));
        for state in [ElementState::Pressed, ElementState::Released] {
            if host.on_mouse_input(state, MouseButton::Left) == EventTail::Normal {
                host.poll_remount();
            }
        }
        host.on_redraw_requested();
    }

    fn rect(host: &WindowedApp, role: Role, name: &str) -> crate::scene::Rect {
        let id = host.app.find_widget(role, Some(name)).unwrap();
        host.app.scene().layout(id).unwrap().rect
    }

    fn click_button(host: &mut WindowedApp, name: &str) {
        let r = rect(host, Role::Button, name);
        press_release(host, r.x + r.width / 2.0, r.y + r.height / 2.0);
    }

    /// A point on a window's title bar, clear of the other window.
    fn title(host: &WindowedApp, name: &str) -> (f32, f32) {
        let r = rect(host, Role::Dialog, name);
        (r.x + 40.0, r.y + 12.0)
    }

    fn focused_name(host: &WindowedApp) -> Option<String> {
        let id = host.app.focused_widget()?;
        host.app
            .scene()
            .a11y(id)?
            .name
            .clone()
            .map(|n| n.to_string())
    }

    #[test]
    fn pressing_a_back_window_title_takes_focus_from_the_window_in_front() {
        let mut host = host();
        // Assign raised Back over Front; the right end of Send sticks out.
        let r = rect(&host, Role::Button, "Send");
        press_release(&mut host, r.right() - 4.0, r.y + 4.0);
        assert_eq!(focused_name(&host).as_deref(), Some("Send"));
        let (x, y) = title(&host, "Back");
        press_release(&mut host, x, y);
        assert_eq!(focused_name(&host), None);
    }

    #[test]
    fn pressing_a_back_window_body_takes_focus_from_the_window_in_front() {
        let mut host = host();
        click_button(&mut host, "Send");
        let r = rect(&host, Role::Dialog, "Back");
        press_release(&mut host, r.x + 20.0, r.y + 100.0);
        assert_eq!(focused_name(&host), None);
    }

    #[test]
    fn raising_a_window_restores_the_control_that_last_had_focus_in_it() {
        let mut host = host();
        // The corner of the button the front window does not cover.
        let r = rect(&host, Role::Button, "Assign");
        press_release(&mut host, r.x + 4.0, r.y + 4.0);
        assert_eq!(focused_name(&host).as_deref(), Some("Assign"));
        // Assign raised Back over Front; the right end of Send sticks out.
        let r = rect(&host, Role::Button, "Send");
        press_release(&mut host, r.right() - 4.0, r.y + 4.0);
        assert_eq!(focused_name(&host).as_deref(), Some("Send"));
        let (x, y) = title(&host, "Back");
        press_release(&mut host, x, y);
        assert_eq!(focused_name(&host).as_deref(), Some("Assign"));
        let id = host.app.focused_widget().unwrap();
        assert!(!crate::widgets::focus_ring_visible(&host.app.widgets, id));
        // The right end of Front's title bar sticks out from under Back.
        let r = rect(&host, Role::Dialog, "Front");
        press_release(&mut host, r.right() - 20.0, r.y + 12.0);
        assert_eq!(focused_name(&host).as_deref(), Some("Send"));
    }

    #[test]
    fn pressing_a_control_in_a_back_window_still_raises_and_focuses_it() {
        let mut host = host();
        click_button(&mut host, "Send");
        let r = rect(&host, Role::Button, "Assign");
        // The corner of the button the front window does not cover.
        press_release(&mut host, r.x + 4.0, r.y + 4.0);
        assert_eq!(focused_name(&host).as_deref(), Some("Assign"));
    }
}

#[test]
fn debug_status_hit_path_is_empty_once_the_pointer_has_left() {
    use winit::dpi::PhysicalPosition;

    let mut app = App::mount_with_size(Button::new("Save"), 200, 80);
    app.frame();
    let root = app.scene().root().expect("button root");
    let rect = app.scene().layout(root).expect("button layout").rect;
    let mut host = windowed_host(app, None);
    host.on_cursor_moved(PhysicalPosition::new(
        f64::from(rect.x + rect.width / 2.0),
        f64::from(rect.y + rect.height / 2.0),
    ));
    let inside = host.debug_status();
    assert_eq!(inside["hit_path"][0]["name"], "Save");

    host.on_cursor_left();
    let outside = host.debug_status();
    assert_eq!(outside["cursor_inside"], false);
    assert!(outside["hit_path"].as_array().unwrap().is_empty());
    let snapshot = host.build_interaction_snapshot(host.cursor_logical());
    assert!(snapshot["hit_path"].as_array().unwrap().is_empty());
}

#[test]
fn debug_ctrl_text_keys_run_the_editing_and_clipboard_chords() {
    use crate::debug_server::DebugKey;

    let mut app = App::mount(crate::widgets::TextInput::new("hello"));
    app.frame();
    let input = app.find_widget(Role::TextInput, None).unwrap();
    app.focus(Some(input));
    let mut host = windowed_host(app, None);
    let chord = |host: &mut super::WindowedApp, c: &str| {
        host.handle_debug_key(DebugKey {
            key: "text".into(),
            shift: false,
            ctrl: true,
            text: Some(c.into()),
        });
    };
    let value = |host: &super::WindowedApp| host.app.scene().a11y(input).unwrap().value.clone();

    chord(&mut host, "a");
    assert_eq!(value(&host).as_deref(), Some("hello"));
    chord(&mut host, "c");
    assert_eq!(host.clipboard.read_text().unwrap(), "hello");
    chord(&mut host, "x");
    assert_eq!(value(&host).as_deref(), Some(""));
    chord(&mut host, "v");
    assert_eq!(value(&host).as_deref(), Some("hello"));
    chord(&mut host, "z");
    assert_eq!(value(&host).as_deref(), Some(""));
    // No chord ever types its letter.
    chord(&mut host, "q");
    assert_eq!(value(&host).as_deref(), Some(""));
}

mod global_shortcuts {
    use super::windowed_host;
    use crate::debug_server::{DebugCommand, DebugGlobalShortcut};
    use crate::platform::{
        GlobalShortcut, GlobalShortcutBinding, GlobalShortcutEvent, GlobalShortcutPhase,
    };
    use crate::App;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn host_with_handler() -> (
        super::super::WindowedApp,
        Rc<RefCell<Vec<GlobalShortcutPhase>>>,
    ) {
        let mut app = App::new(64, 64);
        let seen = Rc::new(RefCell::new(Vec::new()));
        app.register_global_shortcut(GlobalShortcut::new("toggle", "Toggle"), {
            let seen = seen.clone();
            move |phase| seen.borrow_mut().push(phase)
        });
        (windowed_host(app, None), seen)
    }

    fn json(reply: crate::debug_server::DebugReply) -> serde_json::Value {
        serde_json::from_slice(&reply.body).unwrap()
    }

    #[test]
    fn the_debug_command_fires_the_handler_and_lists_the_registration() {
        let (mut host, seen) = host_with_handler();
        let listed = json(host.handle_debug_command(None, DebugCommand::GlobalShortcuts));
        assert_eq!(listed["registered"][0]["id"], "toggle");
        assert_eq!(listed["bindings"], serde_json::json!([]));

        let reply = host.handle_debug_command(
            None,
            DebugCommand::GlobalShortcutTrigger(DebugGlobalShortcut {
                id: "toggle".into(),
                phase: None,
            }),
        );
        assert_eq!(reply.status, 200);
        assert_eq!(
            *seen.borrow(),
            vec![GlobalShortcutPhase::Pressed, GlobalShortcutPhase::Released]
        );

        let missing = host.handle_debug_command(
            None,
            DebugCommand::GlobalShortcutTrigger(DebugGlobalShortcut {
                id: "nope".into(),
                phase: None,
            }),
        );
        assert_eq!(missing.status, 404);
        let bad = host.handle_debug_command(
            None,
            DebugCommand::GlobalShortcutTrigger(DebugGlobalShortcut {
                id: "toggle".into(),
                phase: Some("hold".into()),
            }),
        );
        assert_eq!(bad.status, 422);
    }

    #[test]
    fn backend_bindings_show_up_in_the_listing() {
        let (mut host, _) = host_with_handler();
        host.app
            .apply_global_shortcut_event(GlobalShortcutEvent::Changed(vec![
                GlobalShortcutBinding {
                    id: "toggle".into(),
                    description: "Toggle".into(),
                    trigger: "Ctrl+T".into(),
                },
            ]));
        let listed = json(host.handle_debug_command(None, DebugCommand::GlobalShortcuts));
        assert_eq!(listed["bindings"][0]["trigger"], "Ctrl+T");
    }

    #[test]
    fn a_host_without_an_event_loop_never_starts_a_session() {
        let (mut host, _) = host_with_handler();
        assert!(host.app.global_shortcuts_stale());
        host.sync_global_shortcuts();
        // Still stale: nothing was asked of the system.
        assert!(host.app.global_shortcuts_stale());
        assert!(host.app.global_shortcuts().is_empty());
    }
}

mod global_menu {
    use super::windowed_host;
    use crate::debug_server::{DebugCommand, DebugGlobalMenuItem};
    use crate::platform::{GlobalMenuEvent, Menu, MenuAction, MenuBar, StandardItem};
    use crate::App;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn host_with_menu() -> (super::super::WindowedApp, Rc<RefCell<Vec<&'static str>>>) {
        let mut app = App::new(64, 64);
        let seen = Rc::new(RefCell::new(Vec::new()));
        app.set_global_menu(
            MenuBar::new()
                .menu(Menu::new("App").item(StandardItem::Quit))
                .menu(
                    Menu::new("File")
                        .item(MenuAction::new("open", "Open"))
                        .item(MenuAction::new("off", "Off").enabled(false))
                        .item(Menu::new("Recent").item(MenuAction::new("recent", "Recent file"))),
                ),
        )
        .unwrap();
        for id in ["open", "off", "recent"] {
            let seen = seen.clone();
            app.on_global_menu_item(id, move || seen.borrow_mut().push(id));
        }
        (windowed_host(app, None), seen)
    }

    fn json(reply: crate::debug_server::DebugReply) -> serde_json::Value {
        serde_json::from_slice(&reply.body).unwrap()
    }

    fn trigger(host: &mut super::super::WindowedApp, id: &str) -> u16 {
        host.handle_debug_command(
            None,
            DebugCommand::GlobalMenuTrigger(DebugGlobalMenuItem { id: id.into() }),
        )
        .status
    }

    #[test]
    fn the_debug_listing_shows_the_tree_and_support() {
        let (mut host, _) = host_with_menu();
        let listed = json(host.handle_debug_command(None, DebugCommand::GlobalMenu));
        assert_eq!(listed["supported"], App::global_menu_supported());
        assert_eq!(listed["menus"][0]["title"], "App");
        assert_eq!(listed["menus"][0]["items"][0]["item"], "Quit");
        let file = &listed["menus"][1]["items"];
        assert_eq!(file[0]["id"], "open");
        assert_eq!(file[1]["enabled"], false);
        assert_eq!(file[2]["type"], "submenu");
        assert_eq!(file[2]["items"][0]["id"], "recent");
    }

    #[test]
    fn the_debug_trigger_runs_enabled_items_and_refuses_the_rest() {
        let (mut host, seen) = host_with_menu();
        assert_eq!(trigger(&mut host, "open"), 200);
        assert_eq!(trigger(&mut host, "recent"), 200);
        assert_eq!(trigger(&mut host, "off"), 409);
        assert_eq!(trigger(&mut host, "nope"), 404);
        assert_eq!(*seen.borrow(), vec!["open", "recent"]);
    }

    #[test]
    fn a_backend_event_runs_the_handler_and_host_edit_paths() {
        let (mut host, seen) = host_with_menu();
        assert!(host.apply_global_menu_event(GlobalMenuEvent::Action("open".into())));
        assert!(!host.apply_global_menu_event(GlobalMenuEvent::Action("off".into())));
        assert!(!host.apply_global_menu_event(GlobalMenuEvent::Action("nope".into())));
        // Nothing is focused, so the edit items do nothing, but they reach the
        // host rather than being dropped.
        for item in [StandardItem::Cut, StandardItem::Copy, StandardItem::Paste] {
            assert!(!host.apply_global_menu_event(GlobalMenuEvent::Standard(item)));
        }
        assert_eq!(*seen.borrow(), vec!["open"]);
    }

    #[test]
    fn a_host_without_an_event_loop_never_calls_the_backend() {
        let (mut host, _) = host_with_menu();
        assert!(host.app.global_menu_stale());
        host.sync_global_menu();
        // Still stale: no session was started, nothing was asked of the system.
        assert!(host.app.global_menu_stale());
    }
}

/// Keys, typed text and accessibility actions sent through the debug server
/// land in the interaction trace, in order with the pointer records.
mod debug_input_trace {
    use super::super::WindowedApp;
    use super::windowed_host;
    use crate::a11y::Role;
    use crate::debug_server::{
        DebugAction, DebugButton, DebugCommand, DebugKey, DebugPoint, DebugPress, DebugTarget,
    };
    use crate::interaction_debug::{InteractionRecorder, InteractionTrace};
    use crate::widgets::{Column, Slider, TextInput};
    use crate::App;
    use serde_json::Value;
    use std::path::PathBuf;

    fn path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "schnellui-key-trace-{}-{tag}.jsonl",
            std::process::id()
        ))
    }

    fn host(trace: Option<InteractionTrace>) -> WindowedApp {
        let mut app = App::mount_with_size(
            Column::new()
                .gap(20.0)
                .child(Slider::new(0.0, 0.0, 100.0).name("Level"))
                .child(TextInput::new("hello")),
            400,
            300,
        );
        app.frame();
        let mut host = windowed_host(app, None);
        host.interaction_trace = InteractionRecorder::open(trace).unwrap();
        host
    }

    fn slider_centre(host: &WindowedApp) -> (f32, f32) {
        let id = host.app.find_widget(Role::Slider, Some("Level")).unwrap();
        let r = host.app.scene().layout(id).unwrap().rect;
        (r.x + r.width / 2.0, r.y + r.height / 2.0)
    }

    fn key(name: &str, text: Option<&str>, shift: bool, ctrl: bool) -> DebugCommand {
        DebugCommand::Key(DebugKey {
            key: name.into(),
            shift,
            ctrl,
            text: text.map(str::to_owned),
        })
    }

    /// Press the slider, send a key, text and an action, release.
    fn drive(host: &mut WindowedApp) {
        let (x, y) = slider_centre(host);
        let commands = [
            DebugCommand::PointerPress(DebugPress {
                x: Some(x),
                y: Some(y),
                button: DebugButton::Left,
            }),
            key("right", None, true, false),
            key("text", Some("s3cret"), false, false),
            DebugCommand::Action(DebugAction {
                action: "focus".into(),
                target: DebugTarget {
                    id: None,
                    role: Some("text_input".into()),
                    name: None,
                },
                value: None,
            }),
            DebugCommand::PointerRelease(DebugPress {
                x: None,
                y: None,
                button: DebugButton::Left,
            }),
        ];
        for command in commands {
            let reply = host.handle_debug_command(None, command);
            assert_eq!(
                reply.status,
                200,
                "{}",
                String::from_utf8_lossy(&reply.body)
            );
        }
    }

    fn records(path: &PathBuf) -> Vec<Value> {
        let text = std::fs::read_to_string(path).unwrap();
        let _ = std::fs::remove_file(path);
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn keys_text_and_actions_are_traced_in_order_between_pointer_records() {
        let p = path("order");
        let mut host = host(Some(
            InteractionTrace::file(&p).include_pointer_moves(false),
        ));
        drive(&mut host);
        let all = records(&p);
        let seq: Vec<&str> = all
            .iter()
            .map(|r| r["event"].as_str().unwrap())
            .filter(|e| matches!(*e, "pointer_button" | "key_input" | "a11y_action"))
            .collect();
        assert_eq!(
            seq,
            [
                "pointer_button",
                "key_input",
                "key_input",
                "a11y_action",
                "pointer_button"
            ]
        );
        let inputs: Vec<&Value> = all.iter().filter(|r| r["event"] == "key_input").collect();
        assert_eq!(inputs[0]["origin"], "debug");
        assert_eq!(inputs[0]["key"], "right");
        assert_eq!(inputs[0]["shift"], true);
        assert_eq!(inputs[0]["ctrl"], false);
        assert_eq!(inputs[1]["key"], "text");
        assert_eq!(inputs[1]["text_len"], 6);
        // Typed text is private: only its length is recorded by default.
        assert!(inputs[1].get("text").is_none());
        let action = all.iter().find(|r| r["event"] == "a11y_action").unwrap();
        assert_eq!(action["origin"], "debug");
        assert_eq!(action["action"], "focus");
        assert_eq!(action["target"]["role"], "text_input");
        assert!(action["target"]["id"].is_u64());
        // Sequence numbers stay strictly increasing across all record kinds.
        let seqs: Vec<u64> = all
            .iter()
            .map(|r| r["sequence"].as_u64().unwrap())
            .collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn text_is_recorded_only_when_the_trace_opts_in() {
        let p = path("optin");
        let mut host = host(Some(InteractionTrace::file(&p).include_text(true)));
        drive(&mut host);
        let all = records(&p);
        let typed = all
            .iter()
            .find(|r| r["event"] == "key_input" && r["key"] == "text")
            .unwrap();
        assert_eq!(typed["text"], "s3cret");
    }

    #[test]
    fn window_input_uses_the_same_records_as_debug_input() {
        use crate::windowed::KeyTrace;
        let p = path("window");
        let mut host = host(Some(InteractionTrace::file(&p)));
        host.trace_key_input(
            "window",
            || "Character(\"q\")".into(),
            KeyTrace {
                state: "released",
                repeat: false,
                shift: false,
                ctrl: true,
                alt: false,
                super_key: false,
                text: Some("q"),
            },
        );
        let id = host.app.find_widget(Role::TextInput, None).unwrap();
        host.trace_a11y_action(
            "accesskit",
            schnellui::a11y::accesskit_reexport::Action::SetValue,
            schnellui::a11y::to_access_id(id),
            Some("pw"),
        );
        let all = records(&p);
        let key = all.iter().find(|r| r["event"] == "key_input").unwrap();
        assert_eq!(key["origin"], "window");
        assert_eq!(key["state"], "released");
        assert_eq!(key["ctrl"], true);
        assert_eq!(key["text_len"], 1);
        assert!(key.get("text").is_none());
        let action = all.iter().find(|r| r["event"] == "a11y_action").unwrap();
        assert_eq!(action["origin"], "accesskit");
        assert_eq!(action["action"], "set_value");
        assert_eq!(action["target"]["role"], "text_input");
        assert_eq!(action["value_len"], 2);
        assert!(action.get("value").is_none());
    }

    #[test]
    fn debug_leave_over_a_text_field_resets_the_cursor_icon_and_is_traced() {
        let p = path("leave");
        let mut host = host(Some(
            InteractionTrace::file(&p).include_pointer_moves(false),
        ));
        let id = host.app.find_widget(Role::TextInput, None).unwrap();
        let r = host.app.scene().layout(id).unwrap().rect;
        let reply = host.handle_debug_command(
            None,
            DebugCommand::PointerMove(DebugPoint {
                x: r.x + r.width / 2.0,
                y: r.y + r.height / 2.0,
            }),
        );
        let inside: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(inside["status"]["cursor_icon"], "text");
        assert_eq!(inside["status"]["cursor_inside"], true);
        let reply = host.handle_debug_command(None, DebugCommand::PointerLeave);
        let out: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(out["status"]["cursor_inside"], false);
        assert_eq!(out["status"]["cursor_icon"], "default");
        assert_eq!(host.debug_status()["cursor_icon"], "default");
        let all = records(&p);
        assert!(all.iter().any(|r| r["event"] == "pointer_left"));
        let change = all
            .iter()
            .rfind(|r| r["event"] == "cursor_changed")
            .unwrap();
        assert_eq!(change["from"], "text");
        assert_eq!(change["to"], "default");
    }

    #[test]
    fn a_window_cursor_left_event_resets_the_cursor_icon_too() {
        use winit::dpi::PhysicalPosition;
        let mut host = host(None);
        let id = host.app.find_widget(Role::TextInput, None).unwrap();
        let r = host.app.scene().layout(id).unwrap().rect;
        host.on_cursor_moved(PhysicalPosition::new(
            f64::from(r.x + r.width / 2.0),
            f64::from(r.y + r.height / 2.0),
        ));
        host.sync_cursor();
        assert_eq!(host.debug_status()["cursor_icon"], "text");
        host.on_cursor_left();
        assert_eq!(host.debug_status()["cursor_icon"], "default");
        // A later sync with no event in between keeps it.
        host.sync_cursor();
        assert_eq!(host.debug_status()["cursor_icon"], "default");
    }

    #[test]
    fn tracing_off_records_nothing() {
        let mut host = host(None);
        assert!(host.interaction_trace.is_none());
        drive(&mut host);
        assert!(host.interaction_trace.is_none());
    }
}

/// `wheel`, `resize` and `ime` run the window's own event handlers, so each
/// leaves the app where the equivalent window event would.
mod debug_wheel_resize_ime {
    use std::cell::RefCell;
    use std::rc::Rc;

    use serde_json::Value;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::{Ime, MouseScrollDelta};

    use super::super::WindowedApp;
    use super::windowed_host;
    use crate::a11y::Role;
    use crate::debug_server::{DebugCommand, DebugIme, DebugResize, DebugWheel, DebugWheelUnit};
    use crate::scene::{Point, WidgetKind};
    use crate::widgets::probes::hover_disagrees;
    use crate::widgets::{Button, Column, Dialog, List, ListItem, Scroll, Stack, TextInput};
    use crate::{theme, App, FocusedInputEvent, FocusedInputResult, ThemeMode};

    fn rows(scale: f32) -> WindowedApp {
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme::LIGHT),
            || {
                let mut list = List::new();
                for i in 0..20 {
                    list = list.child(ListItem::new(format!("Row {i}")));
                }
                Scroll::new().size(240.0, 100.0).child(list)
            },
            300,
            200,
            scale,
        );
        app.frame();
        windowed_host(app, None)
    }

    fn run(host: &mut WindowedApp, command: DebugCommand) -> Value {
        let reply = host.handle_debug_command(None, command);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        serde_json::from_slice(&reply.body).unwrap()
    }

    fn refuse(host: &mut WindowedApp, command: DebugCommand) {
        assert_eq!(host.handle_debug_command(None, command).status, 422);
    }

    fn scroll_offset(host: &WindowedApp) -> Point {
        let scene = host.app.scene();
        let id = scene
            .preorder()
            .find(|id| scene.node(*id).map(|n| n.kind) == Some(WidgetKind::Scroll))
            .unwrap();
        scene.scroll_offset(id)
    }

    fn row_centre(host: &WindowedApp, name: &str) -> (f32, f32) {
        let row = host.app.find_widget(Role::ListItem, Some(name)).unwrap();
        let r = host.app.scene().layout(row).unwrap().rect;
        (r.x + r.width / 2.0, r.y + r.height / 2.0)
    }

    /// What a real window does for a wheel at a point: the pointer arrives,
    /// then the wheel.
    fn real_wheel(host: &mut WindowedApp, x: f32, y: f32, delta: MouseScrollDelta) {
        let scale = f64::from(host.app.scale());
        host.on_cursor_moved(PhysicalPosition::new(
            f64::from(x) * scale,
            f64::from(y) * scale,
        ));
        host.on_redraw_requested();
        host.on_mouse_wheel(delta);
        host.on_redraw_requested();
    }

    fn wheel(x: f32, y: f32, dx: f32, dy: f32, unit: DebugWheelUnit) -> DebugCommand {
        DebugCommand::PointerWheel(DebugWheel { x, y, dx, dy, unit })
    }

    #[test]
    fn a_line_wheel_scrolls_like_a_window_wheel_event() {
        let (mut debug, mut real) = (rows(1.0), rows(1.0));
        let (x, y) = row_centre(&debug, "Row 1");
        run(&mut debug, wheel(x, y, 0.0, -2.0, DebugWheelUnit::Lines));
        real_wheel(&mut real, x, y, MouseScrollDelta::LineDelta(0.0, -2.0));
        let moved = scroll_offset(&debug);
        assert!(moved.y > 0.0, "the wheel scrolled the list");
        assert_eq!(moved, scroll_offset(&real));
        assert_eq!(
            debug.debug_status()["hit_path"],
            real.debug_status()["hit_path"]
        );
    }

    #[test]
    fn a_pixel_wheel_is_logical_px_and_follows_the_scale() {
        let (mut debug, mut real) = (rows(2.0), rows(2.0));
        let (x, y) = row_centre(&debug, "Row 1");
        run(&mut debug, wheel(x, y, 0.0, -48.0, DebugWheelUnit::Pixels));
        // Winit reports physical pixels: 48 logical at scale 2 is 96.
        real_wheel(
            &mut real,
            x,
            y,
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -96.0)),
        );
        assert_eq!(scroll_offset(&debug).y, 48.0);
        assert_eq!(scroll_offset(&debug), scroll_offset(&real));
    }

    #[test]
    fn hover_follows_the_row_a_debug_wheel_brought_under_the_pointer() {
        let mut host = rows(1.0);
        let (x, y) = row_centre(&host, "Row 1");
        run(&mut host, wheel(x, y, 0.0, -2.0, DebugWheelUnit::Lines));
        let pointer = Point { x, y };
        assert_eq!(
            hover_disagrees(&host.app.widgets, host.app.scene(), pointer, false),
            None
        );
        assert_eq!(host.debug_status()["cursor_inside"], true);
    }

    #[test]
    fn a_wheel_outside_the_viewport_or_with_a_bad_delta_is_refused() {
        let mut host = rows(1.0);
        refuse(
            &mut host,
            wheel(5000.0, 5.0, 0.0, -1.0, DebugWheelUnit::Lines),
        );
        refuse(
            &mut host,
            wheel(5.0, 5.0, f32::NAN, -1.0, DebugWheelUnit::Lines),
        );
        assert_eq!(scroll_offset(&host).y, 0.0);
    }

    type Seen = Rc<RefCell<Vec<String>>>;

    /// A focused raw surface that records the events it is sent.
    fn surface() -> (WindowedApp, Seen) {
        let seen: Seen = Rc::default();
        let log = seen.clone();
        let mut app = App::mount_with_size(
            Column::new()
                .child(Button::new("Surface"))
                .child(TextInput::new("").label("Name")),
            300,
            200,
        );
        app.frame();
        let button = app.find_widget(Role::Button, Some("Surface")).unwrap();
        app.focus(Some(button));
        app.register_focused_input_handler(Role::Button, Some("Surface"), move |event| {
            if let FocusedInputEvent::Ime(_) | FocusedInputEvent::Pointer(_) = &event {
                log.borrow_mut().push(format!("{event:?}"));
            }
            FocusedInputResult::Handled
        });
        (windowed_host(app, None), seen)
    }

    #[test]
    fn a_wheel_reaches_a_raw_surface_as_the_window_sends_it() {
        let ((mut debug, debug_seen), (mut real, real_seen)) = (surface(), surface());
        run(
            &mut debug,
            wheel(20.0, 15.0, 1.0, -3.0, DebugWheelUnit::Lines),
        );
        real_wheel(
            &mut real,
            20.0,
            15.0,
            MouseScrollDelta::LineDelta(1.0, -3.0),
        );
        let wheels = |seen: &Seen| -> Vec<String> {
            seen.borrow()
                .iter()
                .filter(|e| e.contains("Wheel"))
                .cloned()
                .collect()
        };
        assert_eq!(wheels(&debug_seen).len(), 1);
        assert_eq!(wheels(&debug_seen), wheels(&real_seen));
    }

    #[test]
    fn ime_preedit_commit_and_cancel_reach_the_surface_like_window_ime_events() {
        let ((mut debug, debug_seen), (mut real, real_seen)) = (surface(), surface());
        run(
            &mut debug,
            DebugCommand::Ime(DebugIme::Preedit {
                text: "にほん".into(),
                cursor: Some((0, 9)),
            }),
        );
        real.on_ime(Ime::Preedit("にほん".into(), Some((0, 9))));
        run(
            &mut debug,
            DebugCommand::Ime(DebugIme::Commit {
                text: "日本".into(),
            }),
        );
        real.on_ime(Ime::Commit("日本".into()));
        run(&mut debug, DebugCommand::Ime(DebugIme::Cancel));
        real.on_ime(Ime::Preedit(String::new(), None));
        real.on_ime(Ime::Disabled);
        let events = debug_seen.borrow().clone();
        assert_eq!(events.len(), 4, "{events:?}");
        assert_eq!(events, *real_seen.borrow());
    }

    #[test]
    fn an_ime_commit_types_into_a_focused_field_like_a_window_commit() {
        let make = || {
            let (mut host, seen) = surface();
            let field = host.app.find_widget(Role::TextInput, Some("Name")).unwrap();
            host.app.focus(Some(field));
            host.sync_ime();
            (host, seen)
        };
        let ((mut debug, _), (mut real, _)) = (make(), make());
        run(
            &mut debug,
            DebugCommand::Ime(DebugIme::Preedit {
                text: "にほん".into(),
                cursor: None,
            }),
        );
        let value = |host: &WindowedApp| {
            let field = host.app.find_widget(Role::TextInput, Some("Name")).unwrap();
            host.app.scene().a11y(field).and_then(|a| a.value.clone())
        };
        assert_eq!(
            value(&debug),
            None.or(value(&real)),
            "a preedit types nothing"
        );
        run(
            &mut debug,
            DebugCommand::Ime(DebugIme::Commit {
                text: "日本".into(),
            }),
        );
        real.on_ime(Ime::Commit("日本".into()));
        assert_eq!(value(&debug).as_deref(), Some("日本"));
        assert_eq!(value(&debug), value(&real));
    }

    #[test]
    fn a_preedit_cursor_off_a_character_boundary_is_refused() {
        let (mut host, seen) = surface();
        refuse(
            &mut host,
            DebugCommand::Ime(DebugIme::Preedit {
                text: "にほん".into(),
                cursor: Some((1, 9)),
            }),
        );
        refuse(
            &mut host,
            DebugCommand::Ime(DebugIme::Preedit {
                text: "ab".into(),
                cursor: Some((0, 9)),
            }),
        );
        assert!(seen.borrow().is_empty());
    }

    fn dialog_host(scale: f32) -> WindowedApp {
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme::LIGHT),
            || {
                Stack::new().fill().child(
                    Dialog::new("Approval")
                        .size(240.0, 160.0)
                        .child(TextInput::new("hi").label("Name")),
                )
            },
            400,
            300,
            scale,
        );
        app.frame();
        let mut host = windowed_host(app, None);
        let field = host.app.find_widget(Role::TextInput, Some("Name")).unwrap();
        host.app.focus(Some(field));
        host
    }

    #[test]
    fn resize_runs_the_window_resize_path_and_keeps_live_state() {
        let (mut debug, mut real) = (dialog_host(1.0), dialog_host(1.0));
        let r = run(
            &mut debug,
            DebugCommand::Resize(DebugResize {
                width: 520.0,
                height: 380.0,
            }),
        );
        assert_eq!(r["status"]["viewport"]["width"], 520.0);
        assert_eq!(r["status"]["viewport"]["height"], 380.0);
        real.on_resized(PhysicalSize::new(520, 380));
        real.on_redraw_requested();
        let (d, w) = (debug.debug_status(), real.debug_status());
        assert_eq!(d["viewport"], w["viewport"]);
        assert_eq!(d["focused"], w["focused"]);
        assert!(
            debug
                .app
                .find_widget(Role::Dialog, Some("Approval"))
                .is_some(),
            "the dialog stays open"
        );
        let field = debug
            .app
            .find_widget(Role::TextInput, Some("Name"))
            .unwrap();
        assert_eq!(debug.app.focused_widget(), Some(field), "focus is kept");
        let dialog = |h: &WindowedApp| {
            let id = h.app.find_widget(Role::Dialog, Some("Approval")).unwrap();
            h.app.scene().layout(id).unwrap().rect
        };
        assert_eq!(dialog(&debug), dialog(&real));
    }

    #[test]
    fn resize_takes_logical_size_and_converts_by_the_scale() {
        let mut host = dialog_host(2.0);
        let r = run(
            &mut host,
            DebugCommand::Resize(DebugResize {
                width: 300.0,
                height: 200.0,
            }),
        );
        assert_eq!(r["status"]["viewport"]["width"], 300.0);
        assert_eq!(r["status"]["viewport"]["height"], 200.0);
        assert_eq!(r["status"]["scale"], 2.0);
    }

    #[test]
    fn resize_refuses_empty_non_finite_and_huge_sizes() {
        let mut host = dialog_host(1.0);
        for (width, height) in [
            (0.0, 100.0),
            (100.0, -1.0),
            (f32::NAN, 100.0),
            (100.0, f32::INFINITY),
            (1.0e6, 100.0),
        ] {
            refuse(
                &mut host,
                DebugCommand::Resize(DebugResize { width, height }),
            );
        }
        assert_eq!(host.debug_status()["viewport"]["width"], 400.0);
    }
}

/// Host-level settings a remount hook builds its replacement without: the
/// running app's carry over, and what the hook sets itself wins.
mod remount_host_settings {
    use super::windowed_host;
    use crate::platform::{GlobalShortcut, GlobalShortcutPhase, Menu, MenuAction, MenuBar};
    use crate::scene::{Color, Size};
    use crate::{App, Navigation, Remount, Shortcut, WindowUpdate};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    fn plain() -> App {
        let mut app = App::mount_with_size(crate::widgets::Button::new("Go"), 200, 80);
        app.frame();
        app
    }

    /// A host running `first`, whose hook delivers `replacement` once.
    fn remounted(first: App, replacement: App) -> super::super::WindowedApp {
        let mut slot = Some(replacement);
        let hook: Box<dyn FnMut(Size) -> Option<WindowUpdate>> = Box::new(move |_| {
            slot.take()
                .map(|app| WindowUpdate::Remount(Remount::new(app, "t")))
        });
        let mut host = windowed_host(first, Some(hook));
        host.poll_remount();
        assert_eq!(host.remount_count, 1);
        host
    }

    /// What Escape does with no overlay open: exit when the app asked.
    fn escape_exits(host: &mut super::super::WindowedApp) -> bool {
        matches!(
            host.dispatch_escape_key(),
            super::super::host::EscapeKey::Exit
        )
    }

    #[test]
    fn escape_exits_survives_a_hook_that_does_not_set_it() {
        let mut first = plain();
        first.set_escape_exits(true);
        let mut host = remounted(first, plain());
        assert!(escape_exits(&mut host));
    }

    #[test]
    fn escape_stays_off_across_a_remount() {
        let mut host = remounted(plain(), plain());
        assert!(!escape_exits(&mut host));
    }

    #[test]
    fn a_hook_can_turn_escape_exits_on_itself() {
        let mut next = plain();
        next.set_escape_exits(true);
        let mut host = remounted(plain(), next);
        assert!(escape_exits(&mut host));
    }

    #[test]
    fn the_clear_color_survives_a_hook_that_does_not_set_it() {
        let mut first = plain();
        first.set_clear_color(Color::rgb(10, 20, 30));
        let host = remounted(first, plain());
        assert_eq!(host.app.clear_color(), Color::rgb(10, 20, 30));
    }

    #[test]
    fn a_clear_color_the_hook_sets_wins() {
        let mut first = plain();
        first.set_clear_color(Color::rgb(10, 20, 30));
        let mut next = plain();
        next.set_clear_color(Color::rgb(1, 2, 3));
        let host = remounted(first, next);
        assert_eq!(host.app.clear_color(), Color::rgb(1, 2, 3));
    }

    #[test]
    fn shortcuts_carry_and_the_hooks_own_win_per_chord() {
        let hits = Rc::new(Cell::new((0, 0, 0)));
        let a = Shortcut::new('a', true, false, false);
        let b = Shortcut::new('b', true, false, false);
        let mut first = plain();
        let h = hits.clone();
        first.register_shortcut(a, move || h.set((h.get().0 + 1, h.get().1, h.get().2)));
        let h = hits.clone();
        first.register_shortcut(b, move || h.set((h.get().0, h.get().1 + 1, h.get().2)));
        let mut next = plain();
        let h = hits.clone();
        next.register_shortcut(b, move || h.set((h.get().0, h.get().1, h.get().2 + 1)));
        let mut host = remounted(first, next);
        assert!(host.app.dispatch_shortcut(a));
        assert!(host.app.dispatch_shortcut(b));
        assert_eq!(hits.get(), (1, 0, 1));
    }

    #[test]
    fn the_navigation_handler_survives() {
        let seen = Rc::new(Cell::new(0));
        let mut first = plain();
        let s = seen.clone();
        first.register_navigation_handler(move |_| {
            s.set(s.get() + 1);
            true
        });
        let mut host = remounted(first, plain());
        assert!(host.app.dispatch_navigation(Navigation::Back));
        assert_eq!(seen.get(), 1);
    }

    #[test]
    fn global_shortcuts_survive_a_hook_that_does_not_register_them() {
        let seen = Rc::new(Cell::new(0));
        let mut first = plain();
        let s = seen.clone();
        first.register_global_shortcut(GlobalShortcut::new("go", "Go"), move |phase| {
            if phase == GlobalShortcutPhase::Pressed {
                s.set(s.get() + 1);
            }
        });
        let mut host = remounted(first, plain());
        assert!(host
            .app
            .dispatch_global_shortcut("go", GlobalShortcutPhase::Pressed));
        assert_eq!(seen.get(), 1);
        assert_eq!(host.app.registered_global_shortcuts().count(), 1);
    }

    #[test]
    fn the_global_menu_survives() {
        let seen = Rc::new(Cell::new(0));
        let mut first = plain();
        first
            .set_global_menu(
                MenuBar::new().menu(Menu::new("File").item(MenuAction::new("open", "Open"))),
            )
            .unwrap();
        let s = seen.clone();
        first.on_global_menu_item("open", move || s.set(s.get() + 1));
        let mut host = remounted(first, plain());
        assert!(host.app.dispatch_global_menu_item("open"));
        assert_eq!(seen.get(), 1);
    }

    #[test]
    fn redraw_pacing_title_and_input_observer_survive() {
        let touched = Rc::new(Cell::new(0));
        let mut first = plain();
        first.set_continuous_redraw(true);
        first.set_redraw_interval(Some(Duration::from_millis(40)));
        first.set_window_title_provider(|| "Live".to_owned());
        let t = touched.clone();
        first.set_input_observer(move || t.set(t.get() + 1));
        let mut host = remounted(first, plain());
        assert!(host.app.continuous_redraw);
        assert_eq!(host.app.redraw_interval, Some(Duration::from_millis(40)));
        assert_eq!(
            host.app.window_title_provider.as_mut().map(|p| p()),
            Some("Live".to_owned())
        );
        host.app.note_input();
        assert_eq!(touched.get(), 1);
    }
}
