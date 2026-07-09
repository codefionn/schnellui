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
