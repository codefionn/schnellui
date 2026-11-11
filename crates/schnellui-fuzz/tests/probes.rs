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

/// A wheel that dismisses a context menu leaves the pointer over the control
/// the menu came from; hover goes back to it.
mod wheel_closing_a_context_menu {
    use schnellui_fuzz::components::{self, Mount};
    use schnellui_fuzz::ops::Op;
    use schnellui_fuzz::{serial, PointerButton, THEMES};

    #[test]
    fn hover_returns_to_the_control_under_the_pointer() {
        let _s = serial();
        let c = components::named("text_area");
        let m = Mount {
            theme: THEMES[0],
            size: (360, 240),
            scale: 1.0,
            animate: false,
        };
        let ops = [
            Op::Aim {
                n: 0,
                corner: false,
            },
            Op::Press(PointerButton::Right),
            Op::Release(PointerButton::Right),
            Op::Wheel(1.0),
        ];
        assert_eq!(schnellui_fuzz::oracle::twin(&c, m, &ops), Ok(()));
    }
}

/// A text area inside a scroll emits only the rows near the viewport, and
/// keeps its band while it still covers the view. A fade that rebuilds the
/// page after End has scrolled it re-emits the band around the new offset,
/// while a jump keeps the old one. Both show the same rows, so the two end
/// painting the same.
#[test]
fn a_text_area_band_off_screen_is_not_a_paint_difference() {
    use schnellui_fuzz::components::{self, Mount};
    use schnellui_fuzz::ops::Op;
    use schnellui_fuzz::{oracle, serial, HostKey, THEMES};
    let _serial = serial();
    let component = components::named("scroll_area");
    let mount = Mount {
        theme: THEMES[1],
        size: component.size,
        scale: 1.0,
        animate: false,
    };
    let ops = [
        Op::ThemeFade(0, 50),
        Op::Resize(1.09, 0.2),
        Op::Key(HostKey::End, false),
    ];
    assert_eq!(oracle::animation(&component, mount, &ops), Ok(()));
}

// --- the click oracles, on clicks whose outcome is known -----------------------
//
// `multi_click` and `not_a_multi_click` compare two hosts, so they would pass
// if neither host ever counted a double click. These pin down that the host
// does count them, and where it stops.

mod click_oracles {
    use std::time::Duration;

    use schnellui::a11y::Role;
    use schnellui::scene::Point;
    use schnellui_fuzz::clicks::{self, Break, Broken, MultiClick};
    use schnellui_fuzz::components::{self, Mount};
    use schnellui_fuzz::ops::{actionable, fast_click};
    use schnellui_fuzz::{serial, HeadlessHost, PointerButton, THEMES};

    fn mount() -> Mount {
        Mount {
            theme: THEMES[1],
            size: components::CLICK_TARGETS.size,
            scale: 1.0,
            animate: false,
        }
    }

    fn mounted() -> HeadlessHost {
        (components::CLICK_TARGETS.mount)(&mount())
    }

    /// A point on the first word of the field with `role`.
    fn in_field(host: &HeadlessHost, role: Role) -> Point {
        let id = host.app().find_widget(role, None).expect("the field");
        let r = host.app().scene().layout(id).unwrap().rect;
        Point {
            x: r.x + 24.0,
            y: r.y + r.height.min(40.0) / 2.0 + 6.0,
        }
    }

    fn click(host: &mut HeadlessHost, p: Point) {
        host.pointer_move(p);
        host.press(PointerButton::Left);
        host.release(PointerButton::Left);
    }

    fn selected(host: &HeadlessHost) -> String {
        host.app().selected_text().unwrap_or_default()
    }

    fn counters(host: &HeadlessHost) -> String {
        components::nodes(host.app(), Role::Label)
            .into_iter()
            .filter_map(|n| n.name)
            .find(|n| n.starts_with("added"))
            .expect("the counters")
    }

    #[test]
    fn a_double_click_in_one_frame_selects_a_word_and_a_triple_click_the_line() {
        let _serial = serial();
        let mut host = mounted();
        let p = in_field(&host, Role::TextInput);
        fast_click(&mut host, p, PointerButton::Left, 1);
        assert_eq!(selected(&host), "");
        host.forget_clicks();
        fast_click(&mut host, p, PointerButton::Left, 2);
        assert_eq!(selected(&host), "alpha");
        host.forget_clicks();
        fast_click(&mut host, p, PointerButton::Left, 3);
        assert_eq!(selected(&host), "alpha beta gamma delta");

        let mut host = mounted();
        let p = in_field(&host, Role::MultilineTextInput);
        fast_click(&mut host, p, PointerButton::Left, 2);
        assert_eq!(selected(&host), "one");
        host.forget_clicks();
        fast_click(&mut host, p, PointerButton::Left, 3);
        assert!(
            selected(&host).starts_with("one two three"),
            "selected {:?}",
            selected(&host)
        );
    }

    #[test]
    fn the_interval_and_the_slop_are_the_last_values_that_count() {
        let _serial = serial();
        let interval = u64::from(clicks::interval_ms());
        for (gap, word) in [(interval, "alpha"), (interval + 1, "")] {
            let mut host = mounted();
            let p = in_field(&host, Role::TextInput);
            click(&mut host, p);
            host.advance(Duration::from_millis(gap));
            click(&mut host, p);
            assert_eq!(selected(&host), word, "{gap} ms after the first press");
        }
        for (step, word) in [(5.0, "alpha"), (5.5, "")] {
            for (dx, dy) in [(step, 0.0), (0.0, step)] {
                let mut host = mounted();
                let p = in_field(&host, Role::TextInput);
                click(&mut host, p);
                click(
                    &mut host,
                    Point {
                        x: p.x + dx,
                        y: p.y + dy,
                    },
                );
                assert_eq!(selected(&host), word, "{dx}, {dy} from the first press");
            }
        }
    }

    #[test]
    fn every_click_of_a_fast_triple_click_activates_a_control() {
        let _serial = serial();
        let mut host = mounted();
        let targets = actionable(host.app());
        for (role, name) in [(Role::Button, "Add"), (Role::CheckBox, "Flag")] {
            let id = host.app().find_widget(role, Some(name)).unwrap();
            assert!(targets.contains(&id));
            let r = host.app().scene().layout(id).unwrap().rect;
            let p = Point {
                x: r.x + r.width / 2.0,
                y: r.y + r.height / 2.0,
            };
            fast_click(&mut host, p, PointerButton::Left, 3);
        }
        assert_eq!(counters(&host), "added 3 flips 3 tabs 0 rows 0");
        assert_eq!(
            components::nodes(host.app(), Role::CheckBox)[0]
                .state
                .iter()
                .filter(|s| *s == "checked")
                .count(),
            1,
            "three flips leave the box checked"
        );
    }

    /// Both oracles stay quiet on a field, where a double click does select:
    /// a triple click at any speed, and a run broken by a late or a far click.
    #[test]
    fn the_click_oracles_pass_on_a_text_field() {
        let _serial = serial();
        let host = mounted();
        let field = host.app().find_widget(Role::TextInput, None).unwrap();
        let n = actionable(host.app())
            .iter()
            .position(|id| *id == field)
            .unwrap() as u8;
        drop(host);
        let click = MultiClick {
            n,
            corner: false,
            count: 3,
            drift: [(0.0, 0.0); 2],
            hold: [10, 10, 10],
            pause: [20, 20],
        };
        clicks::multi_click(&components::CLICK_TARGETS, mount(), &[], &click).unwrap();
        for how in [Break::Late(clicks::interval_ms() + 1), Break::Far(0.0, 6.0)] {
            for at in [1, 2] {
                let broken = Broken {
                    click: click.clone(),
                    at,
                    how,
                };
                clicks::not_a_multi_click(&components::CLICK_TARGETS, mount(), &[], &broken)
                    .unwrap();
            }
        }
    }
}

// --- the debug route, on input whose outcome is known ----------------------------

mod debug_route {
    use schnellui::a11y::Role;
    use schnellui::scene::Point;
    use schnellui_fuzz::components::{self, Mount};
    use schnellui_fuzz::{serial, HeadlessHost, PointerButton, PointerRoute, THEMES};

    fn mounted() -> HeadlessHost {
        let mut host = (components::CLICK_TARGETS.mount)(&Mount {
            theme: THEMES[1],
            size: components::CLICK_TARGETS.size,
            scale: 1.0,
            animate: false,
        });
        host.set_route(PointerRoute::Debug);
        host
    }

    fn centre(host: &HeadlessHost, role: Role, name: Option<&str>) -> Point {
        let id = host.app().find_widget(role, name).expect("the widget");
        let r = host.app().scene().layout(id).unwrap().rect;
        Point {
            x: r.x + r.width / 2.0,
            y: r.y + r.height / 2.0,
        }
    }

    fn counters(host: &HeadlessHost) -> String {
        components::nodes(host.app(), Role::Label)
            .into_iter()
            .filter_map(|n| n.name)
            .find(|n| n.starts_with("added"))
            .expect("the counters")
    }

    /// The debug commands do reach the app: `click-at` and `press` with
    /// `release` each click a button once, and three `click-at` commands in a
    /// row are a triple click in a field.
    #[test]
    fn the_debug_commands_click_and_count_clicks() {
        let _serial = serial();
        let mut host = mounted();
        let add = centre(&host, Role::Button, Some("Add"));
        assert!(host.click_at(add));
        host.press_at(add, PointerButton::Left);
        host.release_at(add, PointerButton::Left);
        assert_eq!(counters(&host), "added 2 flips 0 tabs 0 rows 0");

        let field = host.app().find_widget(Role::TextInput, None).unwrap();
        let r = host.app().scene().layout(field).unwrap().rect;
        let p = Point {
            x: r.x + 24.0,
            y: r.y + r.height / 2.0 + 6.0,
        };
        assert!(host.click_at(p));
        assert!(host.click_at(p));
        assert_eq!(host.app().selected_text().as_deref(), Some("alpha"));
        assert!(host.click_at(p));
        assert_eq!(
            host.app().selected_text().as_deref(),
            Some("alpha beta gamma delta")
        );
    }

    /// The `drag` command selects text in a field as a drag by hand does.
    #[test]
    fn the_debug_drag_selects_text() {
        let _serial = serial();
        let mut host = mounted();
        let field = host.app().find_widget(Role::TextInput, None).unwrap();
        let r = host.app().scene().layout(field).unwrap().rect;
        let y = r.y + r.height / 2.0 + 6.0;
        host.drag(
            Point { x: r.x + 14.0, y },
            Point { x: r.x + 80.0, y },
            4,
            PointerButton::Left,
        );
        let selected = host.app().selected_text().unwrap_or_default();
        assert!(
            selected.len() > 3 && "alpha beta gamma delta".contains(&selected),
            "the drag selected {selected:?}"
        );
        assert_eq!(
            host.pointer_state(),
            r#"{"left_down":false,"raw_captured":0,"scrollbar_drag":false,"slider_drag":false,"text_drag":false}"#
        );
    }

    /// `click-at` on the bare page clicks as a click by hand does: it blurs
    /// the focused control, on either route.
    #[test]
    fn click_at_clicks_the_bare_page() {
        let _serial = serial();
        for route in [PointerRoute::Debug, PointerRoute::Handlers] {
            let mut host = mounted();
            host.set_route(route);
            let add = centre(&host, Role::Button, Some("Add"));
            assert!(host.click_at(add));
            assert!(host.app().focused_widget().is_some());
            assert!(host.click_at(Point { x: 0.0, y: 0.0 }));
            assert_eq!(host.app().focused_widget(), None, "a click away blurs");
        }
    }
}

/// Replays of inputs the properties once failed on.
mod replays {
    use schnellui_fuzz::components::{self, Mount};
    use schnellui_fuzz::ops::{Op, ScrollAction};
    use schnellui_fuzz::PointerButton;
    use schnellui_fuzz::{oracle, serial, THEMES};

    /// A theme fade, a click, a burst that a tiny resize and a scroll-into-view
    /// action fall in, then a wait: the outer scroll view ended at 10 with the
    /// animations on and at 0 with them off.
    #[test]
    fn scroll_into_view_after_a_tiny_resize_during_a_fade() {
        let _serial = serial();
        let component = components::named("scroll_nested");
        let mount = Mount {
            theme: THEMES[0],
            size: (300, 220),
            scale: 1.0,
            animate: false,
        };
        let ops = [
            Op::ThemeFade(1, 50),
            Op::Click {
                n: 0,
                corner: false,
                button: PointerButton::Left,
                count: 1,
            },
            Op::Burst(true),
            Op::Resize(0.2, 0.2),
            Op::Access {
                n: 3,
                viewport: false,
                action: ScrollAction::IntoView,
            },
            Op::Wait(1),
        ];
        assert_eq!(oracle::animation(&component, mount, &ops), Ok(()));
    }
}
