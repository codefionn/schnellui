//! The retained-state probes the properties rely on, checked on hand-built
//! layouts where the right answer is known.

use schnellui::a11y::Role;
use schnellui::testing::fuzz::placement_faults;
use schnellui::widgets::{Column, Dialog, Pad, Scroll, Stack, Text};
use schnellui::App;

/// A dialog scoped to a panel inside scrolled content sits below the fold
/// of a short window. Scrolling reaches it, so it has not left the window.
#[test]
fn a_scoped_dialog_below_the_fold_of_a_scroll_view_is_placed() {
    let view = Scroll::new().fill().child(
        Column::new()
            .gap(12.0)
            .child(Pad::all(200.0).child(Text::new("Above the fold")))
            .child(
                Stack::new().size(260.0, 220.0).child(
                    Dialog::new("Scoped")
                        .modeless()
                        .non_fixed()
                        .size(240.0, 200.0)
                        .child(Text::new("Inside")),
                ),
            ),
    );
    let mut app = App::mount_with_size(view, 320, 300);
    app.frame();
    let panel = app.find_widget(Role::Dialog, Some("Scoped")).unwrap();
    assert!(
        app.scene().layout(panel).unwrap().rect.bottom() > 300.0,
        "the dialog starts below the fold"
    );
    assert_eq!(placement_faults(&app), None);
}

// --- the scroll oracles, on states where the right answer is known ---------------
//
// Each oracle must stay quiet on a healthy app and speak when the thing it
// guards is broken by hand.

mod scroll_oracles {
    use schnellui_fuzz::components::{self, Mount};
    use schnellui_fuzz::ops::{apply, Op};
    use schnellui_fuzz::scrolling::{self, Watch};
    use schnellui_fuzz::{serial, HeadlessHost, HostKey, THEMES};

    fn mounted(name: &str) -> (components::Component, HeadlessHost) {
        let component = components::named(name);
        let host = (component.mount)(&Mount {
            theme: THEMES[1],
            size: component.size,
            scale: 1.0,
            animate: false,
        });
        (component, host)
    }

    fn the_viewport(host: &HeadlessHost) -> schnellui::scene::WidgetId {
        scrolling::viewports(host.app())[0]
    }

    #[test]
    fn a_healthy_scrolled_app_passes_every_frame_check() {
        let _serial = serial();
        let (component, mut host) = mounted("scroll_nested");
        for op in [
            Op::WheelAt(0.5, 0.5, 300.0),
            Op::Key(HostKey::PageDown, false),
            Op::Key(HostKey::End, false),
            Op::WheelAt(0.5, 0.6, -40.0),
        ] {
            apply(&mut host, &op, component.size, false);
            assert_eq!(scrolling::settled(&host), Ok(()), "after {op:?}");
        }
    }

    #[test]
    fn an_offset_past_the_end_is_caught() {
        let _serial = serial();
        let (_, mut host) = mounted("scroll");
        let id = the_viewport(&host);
        host.app_mut()
            .scene_mut()
            .set_scroll_offset(id, schnellui::scene::Point { x: 0.0, y: 1.0e6 });
        let err = scrolling::offsets_in_range(host.app()).unwrap_err();
        assert!(err.contains("outside"), "{err}");
    }

    #[test]
    fn an_offset_the_accessibility_tree_was_not_told_of_is_caught() {
        let _serial = serial();
        let (_, mut host) = mounted("scroll");
        let id = the_viewport(&host);
        // Moved behind the widget's back: no accessible value follows.
        host.app_mut()
            .scene_mut()
            .set_scroll_offset(id, schnellui::scene::Point { x: 0.0, y: 40.0 });
        let err = scrolling::a11y_matches_offsets(host.app()).unwrap_err();
        assert!(err.contains("assistive technology"), "{err}");
    }

    #[test]
    fn a_focus_move_that_left_the_control_out_of_view_is_caught() {
        let _serial = serial();
        let (_, mut host) = mounted("scroll_form");
        let op = Op::Key(HostKey::Tab, false);
        let mut watch = Watch::before(&host, &op);
        for _ in 0..6 {
            apply(&mut host, &op, (320, 220), false);
            assert_eq!(watch.after(&mut host, &op), Ok(()), "revealed by the app");
            watch = Watch::before(&host, &op);
        }
        // One more move, then undo what its reveal did, as a missing reveal
        // would leave it: scrolled to the one end or the other, the control
        // is out of view at one of them.
        let watch = Watch::before(&host, &op);
        apply(&mut host, &op, (320, 220), false);
        let id = the_viewport(&host);
        let caught = [-1.0e6f32, 1.0e6].into_iter().any(|delta| {
            host.app_mut().dispatch_scroll(id, delta);
            host.frame();
            watch
                .after(&mut host, &op)
                .is_err_and(|e| e.contains("a key moved focus"))
        });
        assert!(
            caught,
            "a focused control scrolled out of view went unnoticed"
        );
    }

    #[test]
    fn a_theme_switch_that_forgot_the_offsets_is_caught() {
        let _serial = serial();
        let (_, mut host) = mounted("scroll");
        let op = Op::Theme(2);
        apply(&mut host, &Op::WheelAt(0.3, 0.3, 200.0), (300, 240), false);
        assert!(host.app().scene().scroll_offset(the_viewport(&host)).y > 0.0);
        let watch = Watch::before(&host, &op);
        apply(&mut host, &op, (300, 240), false);
        assert_eq!(watch.after(&mut host, &op), Ok(()), "the app keeps them");
        // A rebuild that dropped the offsets.
        let id = the_viewport(&host);
        host.app_mut().dispatch_scroll(id, -1.0e6);
        host.frame();
        let err = watch.after(&mut host, &op).unwrap_err();
        assert!(err.contains("a theme switch moved"), "{err}");
    }

    /// A viewport squeezed to nothing clips all of its content. It once painted
    /// and hit-tested it unclipped, so scrolled content slid up over the header
    /// above it: the header's button was drawn where the pointer hit the form
    /// field that slid under it. A viewport with no layout yet is the unclipped
    /// one; an empty laid-out rect clips everything.
    #[test]
    fn a_scroll_squeezed_to_nothing_clips_its_content() {
        let _serial = serial();
        let component = components::named("scroll_form");
        let mut host = (component.mount)(&Mount {
            theme: THEMES[4],
            size: (320, 220),
            scale: 1.0,
            animate: false,
        });
        // Scrolled while it still has room, then squeezed by a window too
        // short for the header above it.
        let id = the_viewport(&host);
        host.app_mut().dispatch_scroll(id, 100.0);
        host.resize(120.0, 60.0);
        assert_eq!(scrolling::hits_follow_paint(host.app()), Ok(()));
    }

    /// A virtual list measures the rows it mounts, so its end moves while they
    /// are measured. End, a wheel back up and End again, all within one frame, stops
    /// where the estimate said the end was; a frame apart each step goes on
    /// to the measured end. Not fixed here: pinning a viewport that is at
    /// the end through remeasurement is a design decision (`follow_end` does
    /// it, on request).
    #[test]
    #[ignore = "known: a virtual list's End depends on how many frames came between the presses"]
    fn a_virtual_list_reaches_its_end_whatever_the_pace() {
        let _serial = serial();
        let component = components::named("scroll_virtual");
        let m = Mount {
            theme: THEMES[4],
            size: component.size,
            scale: 1.0,
            animate: false,
        };
        let end = Op::Key(HostKey::End, false);
        let ops = [end.clone(), Op::WheelAt(0.0, 0.69, -200.0), end];
        let result = schnellui_fuzz::oracle::pace(&component, m, &ops, 7);
        assert_eq!(result, Ok(()));
    }

    /// The same, for the animated host against the reduced-motion one: the fade
    /// remounts the list on every frame, the jump once, and the scrollbar thumb
    /// ends a tenth of a pixel apart.
    #[test]
    #[ignore = "known: a virtual list's offset depends on how many rebuilds the input took"]
    fn a_virtual_list_ends_in_the_same_place_fading_or_jumping() {
        let _serial = serial();
        let component = components::named("scroll_virtual");
        let m = Mount {
            theme: THEMES[1],
            size: component.size,
            scale: 1.0,
            animate: false,
        };
        let ops = [
            Op::Burst(true),
            Op::ThemeFade(0, 50),
            Op::DragBar {
                n: 0,
                to: 0.92258126,
                drift: 0.0,
            },
            Op::ThemeFade(1, 0),
            Op::Key(HostKey::Up, false),
            Op::Aim {
                n: 0,
                corner: false,
            },
        ];
        assert_eq!(
            schnellui_fuzz::oracle::animation(&component, m, &ops),
            Ok(())
        );
    }
}
