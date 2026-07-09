use super::event_handler::{raw_capture_target_survives, remount_allowed};
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
fn structural_remount_waits_for_pointer_release() {
    assert!(!remount_allowed(true));
    assert!(remount_allowed(false));
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

/// A host-controlled dropdown in a dialog, driven through the native pointer
/// path: press, release, then the remount poll `window_event` runs after each.
mod dropdown_pointer {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::{ElementState, MouseButton};
    use winit::keyboard::ModifiersState;

    use super::super::event_handler::EventTail;
    use super::super::{HostClipboard, WindowedApp};
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
        let glyph_atlas_id = app.atlas.id();
        let host = WindowedApp {
            app,
            clipboard: HostClipboard::Memory(None),
            headless: true,
            title: "dropdown".into(),
            window: None,
            renderer: None,
            accessibility: None,
            event_loop_proxy: None,
            accessibility_full_update_pending: true,
            accessibility_published: Default::default(),
            cursor: PhysicalPosition::new(0.0, 0.0),
            cursor_icon: Default::default(),
            cursor_synced_key: None,
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
            left_pointer_down: false,
            remount: Some(remount),
        };
        assert!(expanded(&host));
        (state, host)
    }

    fn click(host: &mut WindowedApp, x: f32, y: f32) {
        let scale = f64::from(host.app.scale());
        host.on_cursor_moved(PhysicalPosition::new(
            f64::from(x) * scale,
            f64::from(y) * scale,
        ));
        for state in [ElementState::Pressed, ElementState::Released] {
            if host.on_mouse_input(state, MouseButton::Left) == EventTail::Normal {
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
    fn clicking_blank_space_closes_the_dropdown() {
        for update in UPDATES {
            let (state, mut host) = host(update);
            click(&mut host, 395.0, 495.0);
            assert!(!state.get().open);
            assert!(!expanded(&host));
        }
    }
}
